//! Serenity Gateway + Songbird voice manager for Discord voice channels.
//!
//! Handles:
//! - Gateway connection with voice-related intents
//! - Voice state tracking (who's in which channel)
//! - Auto-join on @mention from a user in a voice channel
//! - Idle timeout to leave empty/quiet channels

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serenity::all::{
    ChannelId, Context, EventHandler, GatewayIntents, GuildId, Message, Ready, UserId,
    VoiceState,
};
use serenity::async_trait;
use serenity::Client;
use songbird::events::{Event, EventContext, EventHandler as VoiceEventHandler, TrackEvent};
use songbird::{Call, SerenityInit, Songbird};
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::channels::discord_voice::audio::{AudioPipeline, Utterance};

/// Voice state cache: Guild → (User → Channel).
type VoiceStateCache = Arc<RwLock<HashMap<GuildId, HashMap<UserId, ChannelId>>>>;

/// Tracks active voice connections per guild.
type ActiveCalls = Arc<RwLock<HashMap<GuildId, ActiveVoiceSession>>>;

struct ActiveVoiceSession {
    channel_id: ChannelId,
    last_activity: tokio::time::Instant,
}

/// Commands sent from the Channel impl to the gateway task.
#[derive(Debug)]
pub enum GatewayCommand {
    /// Speak audio (Opus bytes) in the given guild's active voice channel.
    Speak {
        guild_id: GuildId,
        opus_data: Vec<u8>,
    },
    /// Leave a specific guild's voice channel.
    Leave { guild_id: GuildId },
    /// Shut down the gateway.
    Shutdown,
}

/// Events produced by the gateway for the Channel impl.
#[derive(Debug)]
pub enum GatewayEvent {
    /// A user finished speaking (utterance ready for STT).
    UtteranceReady {
        guild_id: GuildId,
        channel_id: ChannelId,
        user_id: UserId,
        utterance: Utterance,
    },
    /// Gateway is connected and ready.
    Ready { bot_user_id: UserId },
}

/// Serenity event handler that tracks voice states and handles @mentions.
struct Handler {
    voice_states: VoiceStateCache,
    active_calls: ActiveCalls,
    songbird: Arc<Songbird>,
    event_tx: mpsc::UnboundedSender<GatewayEvent>,
    audio_pipeline: Arc<Mutex<AudioPipeline>>,
    bot_user_id: RwLock<Option<UserId>>,
    idle_timeout: Duration,
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        let bot_id = ready.user.id;
        *self.bot_user_id.write().await = Some(bot_id);
        let _ = self.event_tx.send(GatewayEvent::Ready {
            bot_user_id: bot_id,
        });
        tracing::info!(
            bot = %ready.user.name,
            "Discord voice gateway connected"
        );
    }

    async fn voice_state_update(&self, _ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        let Some(guild_id) = new.guild_id else {
            return;
        };
        let user_id = new.user_id;

        let mut states = self.voice_states.write().await;
        let guild_states = states.entry(guild_id).or_default();

        if let Some(channel_id) = new.channel_id {
            guild_states.insert(user_id, channel_id);
        } else {
            guild_states.remove(&user_id);
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }
        let Some(guild_id) = msg.guild_id else {
            return;
        };

        let bot_id = self.bot_user_id.read().await;
        let Some(bot_id) = *bot_id else { return };

        let mentioned = msg.mentions.iter().any(|u| u.id == bot_id);
        if !mentioned {
            return;
        }

        // Check if the mentioning user is in a voice channel.
        let states = self.voice_states.read().await;
        let voice_channel = states
            .get(&guild_id)
            .and_then(|gs| gs.get(&msg.author.id))
            .copied();

        let Some(target_channel) = voice_channel else {
            tracing::debug!(
                user = %msg.author.name,
                "User mentioned bot but is not in a voice channel"
            );
            return;
        };

        // Check if already in this channel.
        {
            let calls = self.active_calls.read().await;
            if let Some(session) = calls.get(&guild_id)
                && session.channel_id == target_channel
            {
                tracing::debug!("Already in the target voice channel");
                return;
            }
        }

        self.join_channel(&ctx, guild_id, target_channel).await;
    }
}

impl Handler {
    async fn join_channel(&self, _ctx: &Context, guild_id: GuildId, channel_id: ChannelId) {
        let join_result = self.songbird.join(guild_id, channel_id).await;
        match join_result {
            Ok(call_lock) => {
                tracing::info!(
                    guild = %guild_id,
                    channel = %channel_id,
                    "Joined voice channel"
                );

                {
                    let mut active = self.active_calls.write().await;
                    active.insert(
                        guild_id,
                        ActiveVoiceSession {
                            channel_id,
                            last_activity: tokio::time::Instant::now(),
                        },
                    );
                }

                // Register voice receive event handler.
                let mut call = call_lock.lock().await;
                let receiver = VoiceReceiver {
                    guild_id,
                    channel_id,
                    event_tx: self.event_tx.clone(),
                    audio_pipeline: Arc::clone(&self.audio_pipeline),
                    active_calls: Arc::clone(&self.active_calls),
                };
                call.add_global_event(Event::Track(TrackEvent::End), receiver);

                // Spawn idle timeout checker for this guild.
                let songbird = Arc::clone(&self.songbird);
                let active_calls = Arc::clone(&self.active_calls);
                let timeout = self.idle_timeout;
                tokio::spawn(async move {
                    idle_timeout_loop(songbird, active_calls, guild_id, timeout).await;
                });
            }
            Err(e) => {
                tracing::error!(
                    guild = %guild_id,
                    channel = %channel_id,
                    error = %e,
                    "Failed to join voice channel"
                );
            }
        }
    }
}

/// Receives decoded audio from Songbird and feeds it into the audio pipeline.
struct VoiceReceiver {
    guild_id: GuildId,
    channel_id: ChannelId,
    event_tx: mpsc::UnboundedSender<GatewayEvent>,
    audio_pipeline: Arc<Mutex<AudioPipeline>>,
    active_calls: ActiveCalls,
}

#[async_trait]
impl VoiceEventHandler for VoiceReceiver {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        if let EventContext::VoiceTick(tick) = ctx {
            let mut pipeline = self.audio_pipeline.lock().await;

            for (&ssrc, data) in &tick.speaking {
                if let Some(decoded) = &data.decoded_voice {
                    // Update activity timestamp.
                    {
                        let mut calls = self.active_calls.write().await;
                        if let Some(session) = calls.get_mut(&self.guild_id) {
                            session.last_activity = tokio::time::Instant::now();
                        }
                    }

                    if let Some(utterance) = pipeline.push_audio(ssrc, decoded) {
                        self.send_utterance(utterance);
                    }
                }
            }

            // Check for silence-completed utterances.
            for utterance in pipeline.drain_silent() {
                self.send_utterance(utterance);
            }
        }
        None
    }
}

impl VoiceReceiver {
    fn send_utterance(&self, utterance: Utterance) {
        let user_id = utterance
            .user_id
            .map(|id| UserId::new(id))
            .unwrap_or(UserId::new(0));

        let _ = self.event_tx.send(GatewayEvent::UtteranceReady {
            guild_id: self.guild_id,
            channel_id: self.channel_id,
            user_id,
            utterance,
        });
    }
}

/// Periodically checks if a voice session has been idle and leaves if so.
async fn idle_timeout_loop(
    songbird: Arc<Songbird>,
    active_calls: ActiveCalls,
    guild_id: GuildId,
    timeout: Duration,
) {
    let check_interval = Duration::from_secs(10);
    loop {
        tokio::time::sleep(check_interval).await;

        let should_leave = {
            let calls = active_calls.read().await;
            match calls.get(&guild_id) {
                Some(session) => session.last_activity.elapsed() >= timeout,
                None => return, // Session removed, stop checking.
            }
        };

        if should_leave {
            tracing::info!(
                guild = %guild_id,
                "Leaving voice channel due to idle timeout"
            );
            let _ = songbird.leave(guild_id).await;
            let mut calls = active_calls.write().await;
            calls.remove(&guild_id);
            return;
        }
    }
}

/// Start the Serenity gateway with Songbird voice support.
///
/// Returns channels for bidirectional communication with the gateway task.
pub async fn start_gateway(
    bot_token: String,
    silence_threshold_ms: u64,
    idle_timeout_secs: u64,
) -> Result<GatewayHandle, crate::error::ChannelError> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();

    let voice_states: VoiceStateCache = Arc::new(RwLock::new(HashMap::new()));
    let active_calls: ActiveCalls = Arc::new(RwLock::new(HashMap::new()));
    let audio_pipeline = Arc::new(Mutex::new(AudioPipeline::new(silence_threshold_ms)));

    let songbird = Songbird::serenity();
    let songbird_clone = Arc::clone(&songbird);

    let handler = Handler {
        voice_states,
        active_calls: Arc::clone(&active_calls),
        songbird: Arc::clone(&songbird),
        event_tx,
        audio_pipeline,
        bot_user_id: RwLock::new(None),
        idle_timeout: Duration::from_secs(idle_timeout_secs),
    };

    let intents = GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&bot_token, intents)
        .event_handler(handler)
        .register_songbird_with(Arc::clone(&songbird))
        .await
        .map_err(|e| crate::error::ChannelError::StartupFailed {
            name: "discord-voice".to_string(),
            reason: format!("Failed to build Serenity client: {e}"),
        })?;

    // Spawn the gateway connection task.
    let shard_manager = client.shard_manager.clone();
    tokio::spawn(async move {
        if let Err(e) = client.start().await {
            tracing::error!(error = %e, "Discord gateway error");
        }
    });

    // Spawn command handler.
    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                GatewayCommand::Speak { guild_id, opus_data } => {
                    if let Some(call_lock) = songbird_clone.get(guild_id) {
                        let mut call: tokio::sync::MutexGuard<'_, Call> =
                            call_lock.lock().await;
                        let input: songbird::input::Input = opus_data.into();
                        let track: songbird::tracks::Track = input.into();
                        call.play_only(track);
                    }
                }
                GatewayCommand::Leave { guild_id } => {
                    let _ = songbird_clone.leave(guild_id).await;
                    let mut calls = active_calls.write().await;
                    calls.remove(&guild_id);
                }
                GatewayCommand::Shutdown => {
                    shard_manager.shutdown_all().await;
                    break;
                }
            }
        }
    });

    Ok(GatewayHandle {
        event_rx,
        cmd_tx,
    })
}

/// Handle returned from `start_gateway` for bidirectional communication.
pub struct GatewayHandle {
    /// Receive events (utterances, ready status) from the gateway.
    pub event_rx: mpsc::UnboundedReceiver<GatewayEvent>,
    /// Send commands (speak, leave, shutdown) to the gateway.
    pub cmd_tx: mpsc::UnboundedSender<GatewayCommand>,
}
