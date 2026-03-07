//! Per-user audio buffering and silence-based Voice Activity Detection (VAD).
//!
//! Songbird delivers decoded PCM per-user via `VoiceTick` events. This module
//! buffers audio per SSRC, detects silence gaps, and flushes complete utterances
//! for STT processing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Maximum single-utterance buffer duration (prevents memory bloat).
const MAX_BUFFER_DURATION_SECS: u64 = 30;

/// Discord voice uses 48 kHz stereo by default; Songbird decodes to mono 48 kHz i16.
const SAMPLE_RATE: u32 = 48000;

/// A completed utterance ready for STT.
#[derive(Debug)]
pub struct Utterance {
    /// SSRC that produced this utterance.
    pub ssrc: u32,
    /// Discord user ID (if resolved from SSRC mapping).
    pub user_id: Option<u64>,
    /// Mono PCM samples at 48 kHz.
    pub pcm: Vec<i16>,
    /// Sample rate of the PCM data.
    pub sample_rate: u32,
}

/// Per-user audio accumulator keyed by SSRC.
struct UserBuffer {
    pcm: Vec<i16>,
    last_voice_at: Instant,
    user_id: Option<u64>,
}

impl UserBuffer {
    fn new(user_id: Option<u64>) -> Self {
        Self {
            pcm: Vec::new(),
            last_voice_at: Instant::now(),
            user_id,
        }
    }
}

/// Manages per-user audio buffers and produces utterances on silence gaps.
pub struct AudioPipeline {
    buffers: HashMap<u32, UserBuffer>,
    silence_threshold: Duration,
    max_samples: usize,
}

impl AudioPipeline {
    pub fn new(silence_threshold_ms: u64) -> Self {
        let max_samples = SAMPLE_RATE as usize * MAX_BUFFER_DURATION_SECS as usize;
        Self {
            buffers: HashMap::new(),
            silence_threshold: Duration::from_millis(silence_threshold_ms),
            max_samples,
        }
    }

    /// Register an SSRC → user ID mapping (learned from voice state events).
    pub fn map_ssrc_to_user(&mut self, ssrc: u32, user_id: u64) {
        if let Some(buf) = self.buffers.get_mut(&ssrc) {
            buf.user_id = Some(user_id);
        } else {
            self.buffers.insert(ssrc, UserBuffer::new(Some(user_id)));
        }
    }

    /// Feed decoded PCM from a single user (identified by SSRC).
    ///
    /// Returns a completed `Utterance` if the buffer hit the max duration cap.
    pub fn push_audio(&mut self, ssrc: u32, samples: &[i16]) -> Option<Utterance> {
        let buf = self
            .buffers
            .entry(ssrc)
            .or_insert_with(|| UserBuffer::new(None));

        buf.pcm.extend_from_slice(samples);
        buf.last_voice_at = Instant::now();

        if buf.pcm.len() >= self.max_samples {
            return Some(self.flush_buffer(ssrc));
        }
        None
    }

    /// Check all buffers for silence gaps and return completed utterances.
    ///
    /// Call this periodically (e.g., every 100ms via a tick timer).
    pub fn drain_silent(&mut self) -> Vec<Utterance> {
        let now = Instant::now();
        let silent_ssrcs: Vec<u32> = self
            .buffers
            .iter()
            .filter(|(_, buf)| !buf.pcm.is_empty() && now.duration_since(buf.last_voice_at) >= self.silence_threshold)
            .map(|(ssrc, _)| *ssrc)
            .collect();

        let mut utterances = Vec::with_capacity(silent_ssrcs.len());
        for ssrc in silent_ssrcs {
            utterances.push(self.flush_buffer(ssrc));
        }
        utterances
    }

    /// Remove tracking for an SSRC (user left voice channel).
    pub fn remove_ssrc(&mut self, ssrc: u32) -> Option<Utterance> {
        let buf = self.buffers.remove(&ssrc)?;
        if buf.pcm.is_empty() {
            return None;
        }
        Some(Utterance {
            ssrc,
            user_id: buf.user_id,
            pcm: buf.pcm,
            sample_rate: SAMPLE_RATE,
        })
    }

    fn flush_buffer(&mut self, ssrc: u32) -> Utterance {
        let buf = self
            .buffers
            .get_mut(&ssrc)
            .map(|b| {
                let pcm = std::mem::take(&mut b.pcm);
                let user_id = b.user_id;
                (pcm, user_id)
            });

        let (pcm, user_id) = buf.unwrap_or_default();
        Utterance {
            ssrc,
            user_id,
            pcm,
            sample_rate: SAMPLE_RATE,
        }
    }

    /// Number of active buffers (users being tracked).
    pub fn active_count(&self) -> usize {
        self.buffers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_detection_flushes_buffer() {
        let mut pipeline = AudioPipeline::new(50);
        let samples = vec![100i16; 4800]; // 100ms at 48kHz

        assert!(pipeline.push_audio(1, &samples).is_none());
        assert_eq!(pipeline.active_count(), 1);

        // No silence yet
        let utterances = pipeline.drain_silent();
        assert!(utterances.is_empty());

        // Simulate silence by waiting
        std::thread::sleep(Duration::from_millis(60));

        let utterances = pipeline.drain_silent();
        assert_eq!(utterances.len(), 1);
        assert_eq!(utterances[0].ssrc, 1);
        assert_eq!(utterances[0].pcm.len(), 4800);
        assert_eq!(utterances[0].sample_rate, SAMPLE_RATE);
    }

    #[test]
    fn max_buffer_cap_triggers_flush() {
        let mut pipeline = AudioPipeline::new(800);
        let max_samples = SAMPLE_RATE as usize * MAX_BUFFER_DURATION_SECS as usize;

        // Push just under the cap
        let under_cap = vec![42i16; max_samples - 100];
        assert!(pipeline.push_audio(2, &under_cap).is_none());

        // Push over the cap
        let over = vec![42i16; 200];
        let utterance = pipeline.push_audio(2, &over);
        assert!(utterance.is_some());
        let u = utterance.unwrap();
        assert_eq!(u.ssrc, 2);
        assert_eq!(u.pcm.len(), max_samples + 100); // slightly over, that's fine
    }

    #[test]
    fn ssrc_user_mapping() {
        let mut pipeline = AudioPipeline::new(50);
        pipeline.map_ssrc_to_user(10, 12345);
        pipeline.push_audio(10, &[1, 2, 3]);

        std::thread::sleep(Duration::from_millis(60));
        let utterances = pipeline.drain_silent();
        assert_eq!(utterances.len(), 1);
        assert_eq!(utterances[0].user_id, Some(12345));
    }

    #[test]
    fn remove_ssrc_flushes_remaining() {
        let mut pipeline = AudioPipeline::new(5000);
        pipeline.push_audio(5, &[10, 20, 30]);

        let utterance = pipeline.remove_ssrc(5);
        assert!(utterance.is_some());
        assert_eq!(utterance.unwrap().pcm, vec![10, 20, 30]);
        assert_eq!(pipeline.active_count(), 0);
    }

    #[test]
    fn remove_empty_ssrc_returns_none() {
        let mut pipeline = AudioPipeline::new(800);
        assert!(pipeline.remove_ssrc(99).is_none());
    }

    #[test]
    fn empty_buffer_not_flushed() {
        let mut pipeline = AudioPipeline::new(10);
        // Insert an SSRC but with no audio
        pipeline.map_ssrc_to_user(7, 111);

        std::thread::sleep(Duration::from_millis(20));
        let utterances = pipeline.drain_silent();
        assert!(utterances.is_empty());
    }

}
