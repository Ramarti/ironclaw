//! Discord voice channel — native Rust channel for voice input/output.
//!
//! Connects to Discord's voice gateway via Serenity + Songbird, listens to
//! voice conversations (STT), and optionally speaks responses (TTS).
//!
//! Feature-gated behind `discord-voice`.

pub mod audio;
pub mod config;
pub mod gateway;
pub mod stt;
pub mod tts;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serenity::all::{ChannelId, GuildId, UserId};
use tokio::sync::{RwLock, mpsc};

use crate::channels::{
    Channel, IncomingMessage, MessageStream, OutgoingResponse, StatusUpdate,
};
use crate::error::ChannelError;

use self::config::{DiscordVoiceConfig, VoiceMode};
use self::gateway::{GatewayCommand, GatewayEvent, GatewayHandle};
use self::stt::SttProvider;
use self::tts::TtsProvider;

/// Discord voice channel implementing the `Channel` trait.
///
/// Works in text-only mode when STT/TTS providers are not configured.
/// Voice features (listen + speak) require both providers.
pub struct DiscordVoiceChannel {
    config: DiscordVoiceConfig,
    stt: Option<Arc<dyn SttProvider>>,
    tts: Option<Arc<dyn TtsProvider>>,
    cmd_tx: RwLock<Option<mpsc::UnboundedSender<GatewayCommand>>>,
    bot_user_id: RwLock<Option<UserId>>,
    http_client: reqwest::Client,
    bot_token: String,
}

impl DiscordVoiceChannel {
    pub fn new(
        config: DiscordVoiceConfig,
        stt: Option<Arc<dyn SttProvider>>,
        tts: Option<Arc<dyn TtsProvider>>,
        bot_token: String,
    ) -> Self {
        Self {
            config,
            stt,
            tts,
            cmd_tx: RwLock::new(None),
            bot_user_id: RwLock::new(None),
            http_client: reqwest::Client::new(),
            bot_token,
        }
    }

    /// Send a DM to a Discord user via the REST API.
    async fn send_dm(&self, user_id: UserId, content: &str) -> Result<(), ChannelError> {
        // Create DM channel.
        let dm_response = self
            .http_client
            .post("https://discord.com/api/v10/users/@me/channels")
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&serde_json::json!({ "recipient_id": user_id.to_string() }))
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed {
                name: "discord-voice".into(),
                reason: format!("Failed to create DM channel: {e}"),
            })?;

        let dm_channel: serde_json::Value =
            dm_response.json().await.map_err(|e| ChannelError::SendFailed {
                name: "discord-voice".into(),
                reason: format!("Failed to parse DM channel response: {e}"),
            })?;

        let channel_id = dm_channel["id"]
            .as_str()
            .ok_or_else(|| ChannelError::SendFailed {
                name: "discord-voice".into(),
                reason: "Missing channel ID in DM response".into(),
            })?;

        // Send message.
        self.http_client
            .post(format!(
                "https://discord.com/api/v10/channels/{channel_id}/messages"
            ))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed {
                name: "discord-voice".into(),
                reason: format!("Failed to send DM: {e}"),
            })?;

        Ok(())
    }

    /// Speak audio in a guild's voice channel via TTS.
    /// Returns `false` if TTS is not configured.
    async fn speak_in_channel(
        &self,
        guild_id: GuildId,
        text: &str,
    ) -> Result<bool, ChannelError> {
        let Some(ref tts) = self.tts else {
            return Ok(false);
        };

        let opus_data = tts
            .synthesize(text)
            .await
            .map_err(|e| ChannelError::SendFailed {
                name: "discord-voice".into(),
                reason: format!("TTS synthesis failed: {e}"),
            })?;

        let cmd_tx = self.cmd_tx.read().await;
        if let Some(tx) = cmd_tx.as_ref() {
            tx.send(GatewayCommand::Speak {
                guild_id,
                opus_data,
            })
            .map_err(|_| ChannelError::Disconnected {
                name: "discord-voice".into(),
                reason: "Gateway command channel closed".into(),
            })?;
        }
        Ok(true)
    }

    /// Send a text reply to a Discord channel via the REST API.
    async fn send_channel_message(
        &self,
        channel_id: ChannelId,
        content: &str,
    ) -> Result<(), ChannelError> {
        self.http_client
            .post(format!(
                "https://discord.com/api/v10/channels/{}/messages",
                channel_id
            ))
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed {
                name: "discord-voice".into(),
                reason: format!("Failed to send message: {e}"),
            })?;
        Ok(())
    }
}

#[async_trait]
impl Channel for DiscordVoiceChannel {
    fn name(&self) -> &str {
        "discord-voice"
    }

    async fn start(&self) -> Result<MessageStream, ChannelError> {
        let handle = gateway::start_gateway(
            self.bot_token.clone(),
            self.config.silence_threshold_ms,
            self.config.idle_timeout_secs,
        )
        .await?;

        let GatewayHandle {
            mut event_rx,
            cmd_tx,
        } = handle;

        // Store the command sender for respond().
        *self.cmd_tx.write().await = Some(cmd_tx);

        let stt = self.stt.clone();
        let bot_user_id = Arc::new(RwLock::new(None::<UserId>));
        let bot_user_id_setter = Arc::clone(&bot_user_id);

        let (msg_tx, msg_rx) = mpsc::unbounded_channel::<IncomingMessage>();

        // Spawn event processing loop.
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    GatewayEvent::Ready { bot_user_id: id } => {
                        *bot_user_id_setter.write().await = Some(id);
                    }
                    GatewayEvent::TextMessage {
                        guild_id,
                        channel_id,
                        user_id,
                        content,
                    } => {
                        let mut incoming = IncomingMessage::new(
                            "discord-voice",
                            user_id.to_string(),
                            content,
                        )
                        .with_thread(channel_id.to_string())
                        .with_metadata(serde_json::json!({
                            "is_voice": false,
                            "channel_id": channel_id.to_string(),
                            "is_dm": guild_id.is_none(),
                        }));

                        if let Some(gid) = guild_id {
                            incoming.metadata["guild_id"] =
                                serde_json::json!(gid.to_string());
                        }

                        let _ = msg_tx.send(incoming);
                    }
                    GatewayEvent::UtteranceReady {
                        guild_id,
                        channel_id,
                        user_id,
                        utterance,
                    } => {
                        let Some(ref stt) = stt else {
                            tracing::debug!(
                                "Voice utterance received but STT not configured"
                            );
                            continue;
                        };
                        let stt = Arc::clone(stt);
                        let msg_tx = msg_tx.clone();
                        tokio::spawn(async move {
                            match stt
                                .transcribe(&utterance.pcm, utterance.sample_rate)
                                .await
                            {
                                Ok(text) if !text.trim().is_empty() => {
                                    let incoming = IncomingMessage::new(
                                        "discord-voice",
                                        user_id.to_string(),
                                        text,
                                    )
                                    .with_thread(channel_id.to_string())
                                    .with_metadata(serde_json::json!({
                                        "is_voice": true,
                                        "voice_channel_id": channel_id.to_string(),
                                        "guild_id": guild_id.to_string(),
                                        "voice_mode": "listen",
                                    }));

                                    let _ = msg_tx.send(incoming);
                                }
                                Ok(_) => {
                                    tracing::debug!(
                                        "STT returned empty text, skipping"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "STT transcription failed"
                                    );
                                }
                            }
                        });
                    }
                }
            }
        });

        // Store bot_user_id reference for respond().
        {
            let id = bot_user_id.read().await;
            if let Some(id) = *id {
                *self.bot_user_id.write().await = Some(id);
            }
        }

        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(msg_rx);
        Ok(Box::pin(stream))
    }

    async fn respond(
        &self,
        msg: &IncomingMessage,
        response: OutgoingResponse,
    ) -> Result<(), ChannelError> {
        let is_voice = msg
            .metadata
            .get("is_voice")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Text messages: reply in the same channel.
        if !is_voice {
            let channel_id_str = msg
                .metadata
                .get("channel_id")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            let channel_id = ChannelId::new(
                channel_id_str.parse::<u64>().unwrap_or(0),
            );
            return self
                .send_channel_message(channel_id, &response.content)
                .await;
        }

        // Voice messages: try TTS, fall back to text reply.
        let force_speak = response
            .metadata
            .get("force_speak")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let should_speak =
            self.config.mode == VoiceMode::ListenAndSpeak || force_speak;

        if should_speak {
            let guild_id_str = msg
                .metadata
                .get("guild_id")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            let guild_id = GuildId::new(
                guild_id_str.parse::<u64>().unwrap_or(0),
            );

            let spoke = self
                .speak_in_channel(guild_id, &response.content)
                .await?;

            if !spoke {
                // TTS not configured, fall back to text in the channel.
                let channel_id_str = msg
                    .metadata
                    .get("voice_channel_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                let channel_id = ChannelId::new(
                    channel_id_str.parse::<u64>().unwrap_or(0),
                );
                self.send_channel_message(channel_id, &response.content)
                    .await?;
            }
        } else {
            let user_id = UserId::new(
                msg.user_id.parse::<u64>().unwrap_or(0),
            );
            self.send_dm(user_id, &response.content).await?;
        }
        Ok(())
    }

    async fn send_status(
        &self,
        _status: StatusUpdate,
        _metadata: &serde_json::Value,
    ) -> Result<(), ChannelError> {
        // Voice channels don't support status updates.
        Ok(())
    }

    async fn health_check(&self) -> Result<(), ChannelError> {
        let cmd_tx = self.cmd_tx.read().await;
        if cmd_tx.is_some() {
            Ok(())
        } else {
            Err(ChannelError::Disconnected {
                name: "discord-voice".into(),
                reason: "Gateway not connected".into(),
            })
        }
    }

    fn conversation_context(&self, metadata: &serde_json::Value) -> HashMap<String, String> {
        let mut ctx = HashMap::new();
        if let Some(guild_id) = metadata.get("guild_id").and_then(|v| v.as_str()) {
            ctx.insert("guild".to_string(), guild_id.to_string());
        }
        if let Some(vc_id) = metadata.get("voice_channel_id").and_then(|v| v.as_str()) {
            ctx.insert("voice_channel".to_string(), vc_id.to_string());
        }
        let is_voice = metadata
            .get("is_voice")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        ctx.insert(
            "input_type".to_string(),
            if is_voice { "voice" } else { "text" }.to_string(),
        );
        ctx
    }

    async fn shutdown(&self) -> Result<(), ChannelError> {
        let cmd_tx = self.cmd_tx.read().await;
        if let Some(tx) = cmd_tx.as_ref() {
            let _ = tx.send(GatewayCommand::Shutdown);
        }
        Ok(())
    }
}
