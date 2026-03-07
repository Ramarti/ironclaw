use async_trait::async_trait;
use serde::Deserialize;

/// Error type for speech-to-text operations.
#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("HTTP request failed: {reason}")]
    HttpError { reason: String },

    #[error("Transcription failed: {reason}")]
    TranscriptionFailed { reason: String },

    #[error("Invalid audio: {reason}")]
    InvalidAudio { reason: String },
}

impl From<reqwest::Error> for SttError {
    fn from(e: reqwest::Error) -> Self {
        SttError::HttpError {
            reason: e.to_string(),
        }
    }
}

/// Trait for speech-to-text providers.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Transcribe PCM audio data to text.
    async fn transcribe(
        &self,
        pcm_data: &[i16],
        sample_rate: u32,
    ) -> Result<String, SttError>;
}

/// OpenAI Whisper speech-to-text provider.
pub struct OpenAiStt {
    client: reqwest::Client,
    api_key: String,
}

impl OpenAiStt {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }
}

/// Encode PCM i16 samples into a WAV file in memory.
///
/// Produces a standard 44-byte RIFF/WAVE header (PCM format, mono,
/// 16-bit) followed by the raw sample bytes in little-endian order.
pub fn encode_wav(pcm_data: &[i16], sample_rate: u32) -> Vec<u8> {
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = u32::from(num_channels)
        * sample_rate
        * u32::from(bits_per_sample / 8);
    let block_align = num_channels * (bits_per_sample / 8);
    let data_size = (pcm_data.len() * 2) as u32;
    let file_size = 36 + data_size; // total - 8 bytes for RIFF header

    let mut buf = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt sub-chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // sub-chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&num_channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data sub-chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &sample in pcm_data {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    buf
}

#[derive(Deserialize)]
struct WhisperResponse {
    text: String,
}

#[async_trait]
impl SttProvider for OpenAiStt {
    async fn transcribe(
        &self,
        pcm_data: &[i16],
        sample_rate: u32,
    ) -> Result<String, SttError> {
        if pcm_data.is_empty() {
            return Err(SttError::InvalidAudio {
                reason: "PCM data is empty".to_string(),
            });
        }

        let wav_bytes = encode_wav(pcm_data, sample_rate);

        let file_part = reqwest::multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| SttError::HttpError {
                reason: e.to_string(),
            })?;

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", "whisper-1");

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unable to read body".to_string());
            return Err(SttError::TranscriptionFailed {
                reason: format!("API returned {status}: {body}"),
            });
        }

        let whisper: WhisperResponse =
            response.json().await.map_err(|e| {
                SttError::TranscriptionFailed {
                    reason: format!(
                        "failed to parse response: {e}"
                    ),
                }
            })?;

        Ok(whisper.text)
    }
}

#[cfg(test)]
mod tests {
    use crate::channels::discord_voice::stt::{
        encode_wav, SttError,
    };

    #[test]
    fn wav_header_riff_magic() {
        let pcm = vec![0i16; 100];
        let wav = encode_wav(&pcm, 16000);

        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn wav_header_sizes() {
        let pcm = vec![0i16; 100];
        let wav = encode_wav(&pcm, 16000);

        let data_size = 100 * 2; // 100 samples * 2 bytes each
        let file_size = 36 + data_size;

        // RIFF chunk size at offset 4
        let riff_size =
            u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        assert_eq!(riff_size, file_size as u32);

        // data sub-chunk size at offset 40
        let data_chunk_size = u32::from_le_bytes([
            wav[40], wav[41], wav[42], wav[43],
        ]);
        assert_eq!(data_chunk_size, data_size as u32);

        // Total file length
        assert_eq!(wav.len(), 44 + data_size);
    }

    #[test]
    fn wav_fmt_chunk_values() {
        let wav = encode_wav(&[1i16, -1], 48000);

        // fmt sub-chunk size = 16
        let fmt_size = u32::from_le_bytes([
            wav[16], wav[17], wav[18], wav[19],
        ]);
        assert_eq!(fmt_size, 16);

        // audio format = 1 (PCM)
        let audio_format =
            u16::from_le_bytes([wav[20], wav[21]]);
        assert_eq!(audio_format, 1);

        // channels = 1
        let channels = u16::from_le_bytes([wav[22], wav[23]]);
        assert_eq!(channels, 1);

        // sample rate
        let sr = u32::from_le_bytes([
            wav[24], wav[25], wav[26], wav[27],
        ]);
        assert_eq!(sr, 48000);

        // bits per sample = 16
        let bps = u16::from_le_bytes([wav[34], wav[35]]);
        assert_eq!(bps, 16);
    }

    #[test]
    fn wav_empty_pcm() {
        let wav = encode_wav(&[], 16000);
        assert_eq!(wav.len(), 44);
        let data_size = u32::from_le_bytes([
            wav[40], wav[41], wav[42], wav[43],
        ]);
        assert_eq!(data_size, 0);
    }

    #[test]
    fn stt_error_display() {
        let err = SttError::HttpError {
            reason: "timeout".to_string(),
        };
        assert_eq!(err.to_string(), "HTTP request failed: timeout");

        let err = SttError::TranscriptionFailed {
            reason: "bad model".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Transcription failed: bad model"
        );

        let err = SttError::InvalidAudio {
            reason: "empty".to_string(),
        };
        assert_eq!(err.to_string(), "Invalid audio: empty");
    }
}
