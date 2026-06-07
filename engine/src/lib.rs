//! `strobe` audio engine core.
//!
//! Two planes (architecture.md): this crate is the hard-real-time audio plane. It owns
//! the cpal output thread, synthesizes voices, and advances the frame counter that is the
//! single source of truth for timing. The TypeScript control plane drives it through the
//! napi bridge over a lock-free ring. Phase 1 widens the voice layer into a real
//! instrument: oscillators, filter, envelopes, a synth voice, a drum kit, and a
//! pre-allocated voice pool with stealing.

pub mod audio_io;
pub mod dsp;
pub mod error;
pub mod graph;
pub mod protocol;
pub mod sample;
pub mod scheduler;
pub mod stats;
#[cfg(test)]
mod test_util;
pub mod voice_alloc;
pub mod voices;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use audio_io::{AudioConfig, AudioState};
use dsp::envelope::AdsrConfig;
use dsp::filter::{SvfConfig, SvfMode};
use error::AudioError;
use graph::InstrumentConfig;
use protocol::ring::{ring, RingConsumer, RingProducer};
use sample::{decode, DecodedPcm, SampleBank, SampleBuffer, StretchEngine, Wsola};
use stats::{EngineStats, EngineStatsShared};
use voice_alloc::{InstrumentDef, VoicePool, SLOTS_PER_INSTRUMENT};
use voices::drum::DrumConfig;
use voices::oscillator::{OscConfig, OscKind};

const RING_CAPACITY: usize = 4096;

/// Built-in instruments, indexed by id. These cover the Phase 1 kit + a few synth presets
/// so a song can drive voices without a (Phase 2) dynamic `defineInstrument` round-trip.
/// Drums 3/4 (closed/open hat) share choke group 1.
fn builtin_instruments() -> Vec<(u32, InstrumentDef, u8)> {
    let synth = |kind: OscKind, cutoff: f32| {
        InstrumentDef::Synth(InstrumentConfig {
            osc: OscConfig { kind, ..Default::default() },
            svf: SvfConfig { mode: SvfMode::Lowpass, cutoff_hz: cutoff, q: 1.0, smooth_ms: 5.0 },
            amp_env: AdsrConfig { attack_s: 0.005, decay_s: 0.1, sustain: 0.7, release_s: 0.2, ..Default::default() },
            ..Default::default()
        })
    };
    vec![
        (0, InstrumentDef::Drum(DrumConfig::kick()), 0),
        (1, InstrumentDef::Drum(DrumConfig::snare()), 0),
        (2, InstrumentDef::Drum(DrumConfig::clap()), 0),
        (3, InstrumentDef::Drum(DrumConfig::closed_hat()), 1),
        (4, InstrumentDef::Drum(DrumConfig::open_hat()), 1),
        (5, synth(OscKind::Saw, 2000.0), 0),
        (6, synth(OscKind::Square, 2000.0), 0),
        (7, synth(OscKind::Sine, 8000.0), 0),
    ]
}

pub struct Engine {
    stream: Option<cpal::Stream>,
    frame_clock: Arc<AtomicU64>,
    stats: Arc<EngineStatsShared>,
    full_drops: Arc<AtomicU64>,
    producer: RingProducer,
    consumer: Option<RingConsumer>,
    registry: Vec<(u32, InstrumentDef, u8)>,
    cfg: Option<AudioConfig>,
    /// Shared with the audio thread (feature 02): the control thread inserts decoded buffers,
    /// the audio thread looks them up wait-free.
    sample_bank: Arc<SampleBank>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
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
            registry: builtin_instruments(),
            cfg: None,
            sample_bank: Arc::new(SampleBank::new()),
        }
    }

    /// The engine's running sample rate (after `start`); 48 kHz default before then.
    fn engine_rate(&self) -> u32 {
        self.cfg.map(|c| c.sample_rate).unwrap_or(48_000)
    }

    /// Load already-decoded interleaved PCM into the sample bank, resampling to the engine rate.
    /// NON-REAL-TIME (control thread). Returns the sample id.
    pub fn load_sample_pcm(
        &self,
        samples: Vec<f32>,
        channels: u16,
        src_rate: u32,
    ) -> Result<u32, EngineError> {
        let pcm = DecodedPcm { samples, channels, sample_rate: src_rate };
        let buf = decode::prepare(pcm, self.engine_rate()).map_err(|e| EngineError::Sample(e.to_string()))?;
        Ok(self.sample_bank.insert(Arc::new(buf)))
    }

    /// Decode a local audio file (WAV/MP3/FLAC/OGG/…) and load it. NON-REAL-TIME. Returns the id.
    pub fn load_sample_file(&self, path: &str) -> Result<u32, EngineError> {
        let bytes = std::fs::read(path).map_err(|e| EngineError::Sample(format!("read {path}: {e}")))?;
        let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str());
        let pcm = decode::decode_bytes(bytes, ext).map_err(|e| EngineError::Sample(e.to_string()))?;
        let buf = decode::prepare(pcm, self.engine_rate()).map_err(|e| EngineError::Sample(e.to_string()))?;
        Ok(self.sample_bank.insert(Arc::new(buf)))
    }

    /// Number of loaded samples (telemetry/tests).
    pub fn sample_count(&self) -> usize {
        self.sample_bank.len()
    }

    /// `(channels, frames, sample_rate)` for a loaded sample, or `None` if the id is unknown.
    pub fn sample_info(&self, id: u32) -> Option<(u16, usize, u32)> {
        self.sample_bank.lookup(id).map(|b| (b.channels, b.frames, b.sample_rate))
    }

    /// Prepare a decoupled time-stretched / pitch-shifted copy of a loaded sample (offline,
    /// NON-REAL-TIME). `stretch` scales duration; `pitch_semitones` shifts pitch. Returns a new id.
    pub fn prepare_stretched(
        &self,
        id: u32,
        stretch: f64,
        pitch_semitones: f64,
    ) -> Result<u32, EngineError> {
        let buf =
            self.sample_bank.lookup(id).ok_or_else(|| EngineError::Sample(format!("unknown sample id {id}")))?;
        let planes: Vec<Vec<f32>> = (0..buf.channels as usize).map(|c| buf.channel(c).to_vec()).collect();
        let out = Wsola.process(&planes, stretch, pitch_semitones);
        Ok(self.sample_bank.insert(Arc::new(SampleBuffer::from_planar(out, buf.sample_rate))))
    }

    /// Register an extra instrument (before `start`); returns its id. Dynamic definition
    /// while the stream is running (double-buffered pool swap) is a Phase 2 follow-up.
    pub fn define_instrument(&mut self, def: InstrumentDef, choke: u8) -> u32 {
        let id = self.registry.len() as u32;
        self.registry.push((id, def, choke));
        id
    }

    pub fn start(&mut self, sr: Option<u32>, bf: Option<u32>) -> Result<(), AudioError> {
        if self.stream.is_some() {
            return Ok(());
        }
        let consumer = self.consumer.take().expect("consumer available before start");
        let (device, config, cfg) = audio_io::open_output(sr, bf)?;
        let pool = VoicePool::new(&self.registry, cfg.sample_rate as f32, SLOTS_PER_INSTRUMENT);
        let bus = dsp::effects::MasterBus::new(cfg.sample_rate as f32);
        let state = AudioState::new(
            cfg.channels as usize,
            cfg.sample_rate as f32,
            self.frame_clock.clone(),
            consumer,
            pool,
            bus,
            self.stats.clone(),
            self.sample_bank.clone(),
        );
        let stream = audio_io::build_stream(&device, &config, state)?;
        audio_io::play(&stream)?;
        self.stream = Some(stream);
        self.cfg = Some(cfg);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stream = None;
        let (producer, consumer) = ring(RING_CAPACITY, self.full_drops.clone());
        self.producer = producer;
        self.consumer = Some(consumer);
    }

    pub fn push_events(&self, bytes: &[u8]) -> Result<(), EngineError> {
        let recs = protocol::event::records_from_bytes(bytes).map_err(|_| EngineError::BadLength)?;
        // Full-ring drops are counted inside the ring (try_push → full_drops); the returned
        // count is the same tally, so don't add it again here or every drop is double-counted.
        self.producer.push_batch(recs);
        Ok(())
    }

    pub fn config(&self) -> Option<AudioConfig> {
        self.cfg
    }

    pub fn stats(&self) -> EngineStats {
        EngineStats {
            current_frame: self.frame_clock.load(Ordering::Relaxed),
            underruns: self.stats.underruns.load(Ordering::Relaxed),
            late_events: self.stats.late_events.load(Ordering::Relaxed),
            ring_fullness: self.producer.fullness(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum EngineError {
    BadLength,
    /// Sample load/decode failure (friendly message).
    Sample(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::BadLength => write!(f, "event buffer length is not a multiple of 32"),
            EngineError::Sample(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for EngineError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::event::EventRecord;

    #[test]
    fn new_engine_is_idle_with_zero_clock() {
        let e = Engine::new();
        let s = e.stats();
        assert_eq!(s.current_frame, 0);
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
        let recs = [EventRecord::note_on(1, 100, 1, 220.0, 0.9, 0.0, 5.0, 0)];
        e.push_events(bytemuck::cast_slice(&recs)).expect("push ok");
        assert!(e.stats().ring_fullness > 0.0);
    }

    #[test]
    fn load_sample_pcm_returns_usable_id() {
        let e = Engine::new();
        let id = e.load_sample_pcm(vec![0.1f32; 9600], 2, 48_000).expect("load"); // 0.1s stereo
        assert_eq!(id, 0);
        let buf = e.sample_bank.lookup(id).expect("present");
        assert_eq!(buf.channels, 2);
        assert_eq!(buf.frames, 4800);
        let id2 = e.load_sample_pcm(vec![0.0f32; 100], 1, 48_000).expect("load");
        assert_eq!(id2, 1);
        assert_eq!(e.sample_count(), 2);
    }

    #[test]
    fn define_instrument_returns_next_id() {
        let mut e = Engine::new();
        let id = e.define_instrument(InstrumentDef::Drum(DrumConfig::kick()), 0);
        assert_eq!(id, 8, "after the 8 built-ins (0..7)");
    }
}
