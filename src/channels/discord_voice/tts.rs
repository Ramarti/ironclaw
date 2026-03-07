use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("TTS HTTP error: {reason}")]
    HttpError { reason: String },
    #[error("TTS synthesis failed: {reason}")]
    SynthesisFailed { reason: String },
}

#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Synthesize text to audio. Returns Opus-encoded bytes.
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>, TtsError>;
}

pub struct OpenAiTts {
    client: reqwest::Client,
    api_key: String,
    voice: String,
}

impl OpenAiTts {
    pub fn new(api_key: String, voice: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            voice,
        }
    }
}

#[async_trait]
impl TtsProvider for OpenAiTts {
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>, TtsError> {
        let body = serde_json::json!({
            "model": "tts-1",
            "input": text,
            "voice": &self.voice,
            "response_format": "opus",
        });

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| TtsError::HttpError {
                reason: e.to_string(),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(TtsError::SynthesisFailed {
                reason: format!("HTTP {status}: {body}"),
            });
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| TtsError::HttpError {
                reason: e.to_string(),
            })
    }
}

/// Split text on sentence boundaries for streaming TTS.
///
/// Splits on `.`, `!`, or `?` followed by whitespace or end of string.
pub fn split_sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    let mut i = 0;
    while i < len {
        let ch = chars[i];
        if (ch == '.' || ch == '!' || ch == '?') && (i + 1 >= len || chars[i + 1].is_whitespace())
        {
            let byte_end = text[..start]
                .len()
                + chars[start..=i]
                    .iter()
                    .map(|c| c.len_utf8())
                    .sum::<usize>();
            let segment = text[start..byte_end].trim();
            if !segment.is_empty() {
                sentences.push(segment);
            }
            // Skip trailing whitespace after the punctuation
            i += 1;
            while i < len && chars[i].is_whitespace() {
                i += 1;
            }
            start = text[..start].len()
                + chars[start..i]
                    .iter()
                    .map(|c| c.len_utf8())
                    .sum::<usize>();
            continue;
        }
        i += 1;
    }

    // Remaining text after last sentence boundary
    let trailing = text[start..].trim();
    if !trailing.is_empty() {
        sentences.push(trailing);
    }

    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_error_display_http() {
        let err = TtsError::HttpError {
            reason: "connection refused".into(),
        };
        assert_eq!(err.to_string(), "TTS HTTP error: connection refused");
    }

    #[test]
    fn tts_error_display_synthesis() {
        let err = TtsError::SynthesisFailed {
            reason: "rate limited".into(),
        };
        assert_eq!(err.to_string(), "TTS synthesis failed: rate limited");
    }

    #[test]
    fn split_sentences_empty() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   ").is_empty());
    }

    #[test]
    fn split_sentences_single() {
        let result = split_sentences("Hello world.");
        assert_eq!(result, vec!["Hello world."]);
    }

    #[test]
    fn split_sentences_multiple() {
        let result = split_sentences("First. Second! Third?");
        assert_eq!(result, vec!["First.", "Second!", "Third?"]);
    }

    #[test]
    fn split_sentences_no_trailing_punctuation() {
        let result = split_sentences("Hello. World");
        assert_eq!(result, vec!["Hello.", "World"]);
    }

    #[test]
    fn split_sentences_trailing_space() {
        let result = split_sentences("Done. ");
        assert_eq!(result, vec!["Done."]);
    }

    #[test]
    fn split_sentences_multiple_spaces() {
        let result = split_sentences("One.  Two.  Three.");
        assert_eq!(result, vec!["One.", "Two.", "Three."]);
    }
}
