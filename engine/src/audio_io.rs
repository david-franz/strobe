//! Audio I/O: the cpal output stream, the per-buffer render loop, and the running
//! frame counter that is the single source of truth for timing.
//!
//! This is the hard-real-time boundary. The callback fills its buffer on time, every
//! time: `render` drains events and renders the voice pool into preallocated stereo mix
//! buffers (no allocation, no locking, no JS), then interleaves to the device.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::dsp::effects::MasterBus;
use crate::error::AudioError;
use crate::protocol::ring::RingConsumer;
use crate::scheduler;
use crate::stats::EngineStatsShared;
use crate::voice_alloc::VoicePool;

/// Largest buffer (frames) we preallocate mix scratch for. cpal buffers are far smaller;
/// generous so the audio thread never reallocates.
const MAX_BUFFER: usize = 16_384;

#[derive(Debug, Clone, Copy)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub buffer_frames: u32,
    pub channels: u16,
}

pub fn open_output(
    req_sr: Option<u32>,
    req_bf: Option<u32>,
) -> Result<(cpal::Device, cpal::StreamConfig, AudioConfig), AudioError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(AudioError::NoDevice)?;

    let default = device
        .default_output_config()
        .map_err(|e| AudioError::DefaultConfig(e.to_string()))?;

    if default.sample_format() != cpal::SampleFormat::F32 {
        return Err(AudioError::UnsupportedFormat);
    }

    let sample_rate = req_sr.unwrap_or(default.sample_rate().0);
    let channels = default.channels().max(2);

    let mut config: cpal::StreamConfig = default.config();
    config.sample_rate = cpal::SampleRate(sample_rate);
    config.channels = channels;
    config.buffer_size = match req_bf {
        Some(bf) => cpal::BufferSize::Fixed(bf),
        None => cpal::BufferSize::Default,
    };

    let cfg = AudioConfig {
        sample_rate,
        buffer_frames: req_bf.unwrap_or(256),
        channels,
    };
    Ok((device, config, cfg))
}

/// State the audio thread owns. Constructed before the stream; no allocation after.
pub struct AudioState {
    pub channels: usize,
    pub sample_rate: f32,
    pub frame_clock: Arc<AtomicU64>,
    pub consumer: RingConsumer,
    pub pool: VoicePool,
    pub bus: MasterBus,
    pub stats: Arc<EngineStatsShared>,
    mix_l: Vec<f32>,
    mix_r: Vec<f32>,
}

impl AudioState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channels: usize,
        sample_rate: f32,
        frame_clock: Arc<AtomicU64>,
        consumer: RingConsumer,
        pool: VoicePool,
        bus: MasterBus,
        stats: Arc<EngineStatsShared>,
    ) -> Self {
        Self {
            channels,
            sample_rate,
            frame_clock,
            consumer,
            pool,
            bus,
            stats,
            mix_l: vec![0.0; MAX_BUFFER],
            mix_r: vec![0.0; MAX_BUFFER],
        }
    }
}

pub fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut st: AudioState,
) -> Result<cpal::Stream, AudioError> {
    let err_stats = st.stats.clone();
    let err_fn = move |_e: cpal::StreamError| {
        err_stats.underruns.fetch_add(1, Ordering::Relaxed);
    };

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                render(&mut st, data);
            },
            err_fn,
            None,
        )
        .map_err(|e| AudioError::BuildStream(e.to_string()))?;
    Ok(stream)
}

/// The per-buffer render loop. THIS IS THE HOT PATH.
#[inline]
pub fn render(st: &mut AudioState, data: &mut [f32]) {
    let ch = st.channels;
    debug_assert!(ch >= 1);
    let frames = data.len() / ch;
    debug_assert!(frames <= MAX_BUFFER);
    let frames = frames.min(MAX_BUFFER);
    let base = st.frame_clock.load(Ordering::Relaxed);

    // Zero the stereo mix for this buffer.
    for i in 0..frames {
        st.mix_l[i] = 0.0;
        st.mix_r[i] = 0.0;
    }

    // Drain events + render the pool in sample-accurate segments.
    scheduler::dispatch_and_render(
        &mut st.consumer,
        base,
        frames as u64,
        &mut st.pool,
        &mut st.bus,
        &st.stats,
        &mut st.mix_l[..frames],
        &mut st.mix_r[..frames],
    );

    // Master bus + look-ahead limiter: the final stage before the device.
    st.bus.process(&mut st.mix_l[..frames], &mut st.mix_r[..frames]);

    // Interleave to the device: stereo on ch 0/1, mono = (L+R)/2, extra channels silent.
    for f in 0..frames {
        let l = st.mix_l[f];
        let r = st.mix_r[f];
        let off = f * ch;
        if ch == 1 {
            data[off] = (l + r) * 0.5;
        } else {
            data[off] = l;
            data[off + 1] = r;
            for c in 2..ch {
                data[off + c] = 0.0;
            }
        }
    }

    st.frame_clock.store(base + frames as u64, Ordering::Relaxed);
}

pub fn play(stream: &cpal::Stream) -> Result<(), AudioError> {
    stream
        .play()
        .map_err(|e| AudioError::PlayStream(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::event::EventRecord;
    use crate::protocol::ring::{ring, RingProducer};
    use crate::voice_alloc::{InstrumentDef, VoicePool};
    use crate::voices::drum::DrumConfig;

    fn state(channels: usize) -> (AudioState, RingProducer) {
        let drops = Arc::new(AtomicU64::new(0));
        let (producer, consumer) = ring(256, drops);
        let pool = VoicePool::new(&[(0, InstrumentDef::Drum(DrumConfig::kick()), 0)], 48_000.0, 4);
        let st = AudioState::new(
            channels,
            48_000.0,
            Arc::new(AtomicU64::new(0)),
            consumer,
            pool,
            MasterBus::new(48_000.0),
            Arc::new(EngineStatsShared::default()),
        );
        (st, producer)
    }

    fn kick_at(frame: u32) -> EventRecord {
        EventRecord::note_on(1, frame, 1, 0.0, 1.0, 0.0, 0.0, 1)
    }

    #[test]
    fn frame_counter_is_exact_and_monotonic() {
        let (mut st, _p) = state(2);
        let mut buf = vec![0.0f32; 256 * 2];
        render(&mut st, &mut buf);
        assert_eq!(st.frame_clock.load(Ordering::Relaxed), 256);
        let mut buf2 = vec![0.0f32; 128 * 2];
        render(&mut st, &mut buf2);
        assert_eq!(st.frame_clock.load(Ordering::Relaxed), 384);
    }

    #[test]
    fn idle_pool_renders_silence() {
        let (mut st, _p) = state(2);
        let mut buf = vec![1.0f32; 16 * 2];
        render(&mut st, &mut buf);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn centered_voice_fans_equally_to_both_channels() {
        let (mut st, p) = state(2);
        p.try_push(kick_at(0));
        let mut buf = vec![0.0f32; 256 * 2];
        render(&mut st, &mut buf);
        for f in 0..256 {
            assert_eq!(buf[f * 2], buf[f * 2 + 1], "centered kick → L==R at {f}");
        }
        assert!(buf.iter().any(|&s| s.abs() > 1e-6), "kick produced sound");
    }

    #[test]
    fn mono_device_downmixes() {
        let (mut st, p) = state(1);
        p.try_push(kick_at(0));
        let mut buf = vec![0.0f32; 256];
        render(&mut st, &mut buf);
        assert!(buf.iter().any(|&s| s.abs() > 1e-6));
        assert_eq!(st.frame_clock.load(Ordering::Relaxed), 256);
    }

    #[test]
    #[ignore = "requires a real audio output device"]
    fn stream_opens_and_advances_clock() {
        use std::time::Duration;
        let (device, config, cfg) = open_output(Some(48_000), Some(256)).expect("open output");
        let frame_clock = Arc::new(AtomicU64::new(0));
        let drops = Arc::new(AtomicU64::new(0));
        let (_p, consumer) = ring(4096, drops);
        let pool = VoicePool::new(
            &[(0, InstrumentDef::Drum(DrumConfig::kick()), 0)],
            cfg.sample_rate as f32,
            4,
        );
        let st = AudioState::new(
            cfg.channels as usize,
            cfg.sample_rate as f32,
            frame_clock.clone(),
            consumer,
            pool,
            MasterBus::new(cfg.sample_rate as f32),
            Arc::new(EngineStatsShared::default()),
        );
        let stream = build_stream(&device, &config, st).expect("build");
        play(&stream).expect("play");
        std::thread::sleep(Duration::from_millis(200));
        let advanced = frame_clock.load(Ordering::Relaxed);
        let expected = (0.2 * cfg.sample_rate as f64) as u64;
        assert!(advanced.abs_diff(expected) < 4096);
    }
}
