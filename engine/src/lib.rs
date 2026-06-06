//! `strobe` audio engine core (Phase 0).
//!
//! Two planes (architecture.md): this crate is the hard-real-time audio plane. It
//! owns the cpal output thread, synthesizes voices, and advances the frame counter
//! that is the single source of truth for timing. The soft-real-time control plane
//! (TypeScript) drives it through the napi bridge (doc 04) over a lock-free ring.

pub mod audio_io;
pub mod error;
pub mod scheduler;
pub mod stats;
pub mod voices;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use audio_io::{AudioConfig, AudioState};
use error::AudioError;
use scheduler::RingConsumer;
use stats::{EngineStats, EngineStatsShared};
use voices::drum::KickVoice;

/// The engine handle held by the bridge. `Some(stream)` while playing; dropping the
/// stream stops the audio thread cleanly.
pub struct Engine {
    stream: Option<cpal::Stream>,
    frame_clock: Arc<AtomicU64>,
    stats: Arc<EngineStatsShared>,
    /// Consumer half of the ring, moved into `AudioState` at `start`. Doc 02 pairs
    /// this with a producer held on the `Engine` for `pushEvents`.
    consumer: Option<RingConsumer>,
    cfg: Option<AudioConfig>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Build the engine with the frame clock and stats, but no stream yet.
    pub fn new() -> Self {
        Engine {
            stream: None,
            frame_clock: Arc::new(AtomicU64::new(0)),
            stats: Arc::new(EngineStatsShared::default()),
            consumer: Some(RingConsumer::default()),
            cfg: None,
        }
    }

    /// Open the cpal output stream and start playing. Idempotent: a second call
    /// while already playing is a no-op.
    pub fn start(&mut self, sr: Option<u32>, bf: Option<u32>) -> Result<(), AudioError> {
        if self.stream.is_some() {
            return Ok(());
        }
        let (device, config, cfg) = audio_io::open_output(sr, bf)?;
        let consumer = self.consumer.take().unwrap_or_default();
        let state = AudioState {
            channels: cfg.channels as usize,
            sample_rate: cfg.sample_rate as f32,
            frame_clock: self.frame_clock.clone(),
            consumer,
            kick: KickVoice::new(cfg.sample_rate as f32),
            stats: self.stats.clone(),
        };
        let stream = audio_io::build_stream(&device, &config, state)?;
        audio_io::play(&stream)?;
        self.stream = Some(stream);
        self.cfg = Some(cfg);
        Ok(())
    }

    /// Stop playing by dropping the stream. The frame clock is intentionally not
    /// reset so `stats()` stays readable after stop.
    pub fn stop(&mut self) {
        self.stream = None;
        // The ring consumer was moved into the (now-dropped) stream; recreate a
        // placeholder so a subsequent `start` can run. Doc 02/04 rebuild the ring.
        self.consumer = Some(RingConsumer::default());
    }

    /// The current configuration, once started.
    pub fn config(&self) -> Option<AudioConfig> {
        self.cfg
    }

    /// A point-in-time telemetry snapshot for the control plane.
    pub fn stats(&self) -> EngineStats {
        EngineStats {
            current_frame: self.frame_clock.load(Ordering::Relaxed),
            underruns: self.stats.underruns.load(Ordering::Relaxed),
            late_events: self.stats.late_events.load(Ordering::Relaxed),
            ring_fullness: 0.0, // doc 02 computes this from ring head/tail
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_engine_is_idle_with_zero_clock() {
        let e = Engine::new();
        let s = e.stats();
        assert_eq!(s.current_frame, 0);
        assert_eq!(s.underruns, 0);
        assert_eq!(s.late_events, 0);
        assert!(e.config().is_none());
    }

    #[test]
    fn stop_without_start_is_harmless() {
        let mut e = Engine::new();
        e.stop();
        assert_eq!(e.stats().current_frame, 0);
    }
}
