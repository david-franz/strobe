//! `strobe` audio engine core (Phase 0).
//!
//! Two planes (architecture.md): this crate is the hard-real-time audio plane. It
//! owns the cpal output thread, synthesizes voices, and advances the frame counter
//! that is the single source of truth for timing. The soft-real-time control plane
//! (TypeScript) drives it through the napi bridge (doc 04) over a lock-free ring.

pub mod audio_io;
pub mod envelope;
pub mod error;
pub mod event;
pub mod filter;
pub mod graph;
pub mod ring;
pub mod scheduler;
pub mod stats;
#[cfg(test)]
mod test_util;
pub mod voices;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use audio_io::{AudioConfig, AudioState};
use error::AudioError;
use ring::{ring, RingConsumer, RingProducer};
use stats::{EngineStats, EngineStatsShared};
use voices::drum::KickVoice;

/// Ring capacity in records — orders of magnitude over the ~tens of events a 100 ms
/// lookahead window holds at techno tempos (event-protocol.md / doc 02).
const RING_CAPACITY: usize = 4096;

/// The engine handle held by the bridge. `Some(stream)` while playing; dropping the
/// stream stops the audio thread cleanly.
pub struct Engine {
    stream: Option<cpal::Stream>,
    frame_clock: Arc<AtomicU64>,
    stats: Arc<EngineStatsShared>,
    full_drops: Arc<AtomicU64>,
    /// Producer half of the ring — used by `push_events` (doc 04 exposes it via napi).
    producer: RingProducer,
    /// Consumer half, moved into `AudioState` at `start`.
    consumer: Option<RingConsumer>,
    cfg: Option<AudioConfig>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Build the engine with the frame clock, stats, and ring, but no stream yet.
    pub fn new() -> Self {
        let full_drops = Arc::new(AtomicU64::new(0));
        let (producer, consumer) = ring(RING_CAPACITY, full_drops.clone());
        Engine {
            stream: None,
            frame_clock: Arc::new(AtomicU64::new(0)),
            stats: Arc::new(EngineStatsShared::default()),
            full_drops,
            producer,
            consumer: Some(consumer),
            cfg: None,
        }
    }

    /// Open the cpal output stream and start playing. Idempotent: a second call
    /// while already playing is a no-op.
    pub fn start(&mut self, sr: Option<u32>, bf: Option<u32>) -> Result<(), AudioError> {
        if self.stream.is_some() {
            return Ok(());
        }
        let consumer = self.consumer.take().expect("consumer available before start");
        let (device, config, cfg) = audio_io::open_output(sr, bf)?;
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
    /// reset so `stats()` stays readable after stop. A fresh ring is created so a
    /// subsequent `start` has a consumer to move in.
    pub fn stop(&mut self) {
        self.stream = None;
        let (producer, consumer) = ring(RING_CAPACITY, self.full_drops.clone());
        self.producer = producer;
        self.consumer = Some(consumer);
    }

    /// Hot path: validate and copy `bytes` (N×32) into the ring. Non-blocking; a full
    /// ring drops and counts rather than waiting (doc 02/04).
    pub fn push_events(&self, bytes: &[u8]) -> Result<(), EngineError> {
        let recs = event::records_from_bytes(bytes).map_err(|_| EngineError::BadLength)?;
        let dropped = self.producer.push_batch(recs);
        if dropped > 0 {
            self.full_drops
                .fetch_add(dropped as u64, Ordering::Relaxed);
        }
        Ok(())
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
            ring_fullness: self.producer.fullness(),
        }
    }
}

/// Errors from control-plane calls into the engine.
#[derive(Debug, PartialEq)]
pub enum EngineError {
    /// A `pushEvents` buffer whose length is not a multiple of 32 bytes.
    BadLength,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::BadLength => write!(f, "event buffer length is not a multiple of 32"),
        }
    }
}

impl std::error::Error for EngineError {}

#[cfg(test)]
mod tests {
    use super::*;
    use event::EventRecord;

    #[test]
    fn new_engine_is_idle_with_zero_clock() {
        let e = Engine::new();
        let s = e.stats();
        assert_eq!(s.current_frame, 0);
        assert_eq!(s.underruns, 0);
        assert_eq!(s.late_events, 0);
        assert_eq!(s.ring_fullness, 0.0);
        assert!(e.config().is_none());
    }

    #[test]
    fn push_events_rejects_bad_length() {
        let e = Engine::new();
        assert_eq!(e.push_events(&[0u8; 31]), Err(EngineError::BadLength));
    }

    #[test]
    fn push_events_fills_ring() {
        let e = Engine::new();
        let recs = [
            EventRecord::note_on(1, 100, 1, 55.0, 0.9, 0.0, 0.0, 0),
            EventRecord::note_on(1, 200, 2, 55.0, 0.9, 0.0, 0.0, 1),
        ];
        let bytes: &[u8] = bytemuck::cast_slice(&recs);
        e.push_events(bytes).expect("push ok");
        assert!(e.stats().ring_fullness > 0.0);
    }

    #[test]
    fn stop_without_start_is_harmless() {
        let mut e = Engine::new();
        e.stop();
        assert_eq!(e.stats().current_frame, 0);
    }
}
