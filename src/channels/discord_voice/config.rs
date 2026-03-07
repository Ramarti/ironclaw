use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid voice mode: {value} (expected 'listen_and_speak' or 'listen_only')")]
    InvalidVoiceMode { value: String },

    #[error("invalid integer for {name}: {source}")]
    InvalidInt {
        name: String,
        source: std::num::ParseIntError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    ListenAndSpeak,
    ListenOnly,
}

impl FromStr for VoiceMode {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "listen_and_speak" | "listenandspeak" => Ok(Self::ListenAndSpeak),
            "listen_only" | "listenonly" => Ok(Self::ListenOnly),
            other => Err(ConfigError::InvalidVoiceMode {
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for VoiceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ListenAndSpeak => write!(f, "listen_and_speak"),
            Self::ListenOnly => write!(f, "listen_only"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordVoiceConfig {
    pub mode: VoiceMode,
    pub idle_timeout_secs: u64,
    pub silence_threshold_ms: u64,
    pub stt_provider: String,
    pub tts_provider: String,
    pub tts_voice: String,
}

impl DiscordVoiceConfig {
    /// Build config from environment variables.
    ///
    /// Bot token is handled separately via the secrets store.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mode = read_optional_env("DISCORD_VOICE_MODE")
            .map(|v| VoiceMode::from_str(&v))
            .transpose()?
            .unwrap_or(VoiceMode::ListenAndSpeak);
        let idle_timeout_secs = parse_optional_u64(
            "DISCORD_VOICE_IDLE_TIMEOUT_SECS",
            300,
        )?;
        let silence_threshold_ms = parse_optional_u64(
            "DISCORD_VOICE_SILENCE_THRESHOLD_MS",
            800,
        )?;
        let stt_provider = read_optional_env("DISCORD_VOICE_STT_PROVIDER")
            .unwrap_or_else(|| "openai".to_string());
        let tts_provider = read_optional_env("DISCORD_VOICE_TTS_PROVIDER")
            .unwrap_or_else(|| "openai".to_string());
        let tts_voice = read_optional_env("DISCORD_VOICE_TTS_VOICE")
            .unwrap_or_else(|| "alloy".to_string());

        Ok(Self {
            mode,
            idle_timeout_secs,
            silence_threshold_ms,
            stt_provider,
            tts_provider,
            tts_voice,
        })
    }
}

fn read_optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn parse_optional_u64(name: &str, default: u64) -> Result<u64, ConfigError> {
    match read_optional_env(name) {
        Some(v) => v.parse::<u64>().map_err(|e| ConfigError::InvalidInt {
            name: name.to_string(),
            source: e,
        }),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_mode_from_str_snake_case() {
        assert_eq!(
            VoiceMode::from_str("listen_and_speak").ok(),
            Some(VoiceMode::ListenAndSpeak)
        );
        assert_eq!(
            VoiceMode::from_str("listen_only").ok(),
            Some(VoiceMode::ListenOnly)
        );
    }

    #[test]
    fn voice_mode_from_str_no_underscore() {
        assert_eq!(
            VoiceMode::from_str("listenandspeak").ok(),
            Some(VoiceMode::ListenAndSpeak)
        );
        assert_eq!(
            VoiceMode::from_str("listenonly").ok(),
            Some(VoiceMode::ListenOnly)
        );
    }

    #[test]
    fn voice_mode_from_str_case_insensitive() {
        assert_eq!(
            VoiceMode::from_str("LISTEN_AND_SPEAK").ok(),
            Some(VoiceMode::ListenAndSpeak)
        );
        assert_eq!(
            VoiceMode::from_str("Listen_Only").ok(),
            Some(VoiceMode::ListenOnly)
        );
    }

    #[test]
    fn voice_mode_from_str_invalid() {
        let err = VoiceMode::from_str("push_to_talk");
        assert!(err.is_err());
        let msg = err.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(msg.contains("push_to_talk"));
    }

    #[test]
    fn voice_mode_display_roundtrip() {
        let modes = [VoiceMode::ListenAndSpeak, VoiceMode::ListenOnly];
        for mode in modes {
            let s = mode.to_string();
            let parsed = VoiceMode::from_str(&s);
            assert_eq!(parsed.ok(), Some(mode));
        }
    }

    #[test]
    fn parse_optional_u64_returns_default() {
        // Uses an env var unlikely to be set.
        let result = parse_optional_u64("__IRONCLAW_TEST_NONEXISTENT_VAR__", 42);
        assert_eq!(result.ok(), Some(42));
    }

    #[test]
    fn config_error_invalid_int_display() {
        let err = ConfigError::InvalidInt {
            name: "TIMEOUT".to_string(),
            source: "abc".parse::<u64>().unwrap_err(),
        };
        assert!(err.to_string().contains("TIMEOUT"));
    }
}
