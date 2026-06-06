//! Audio I/O: the cpal output stream, the per-buffer render loop, and the running
//! frame counter that is the single source of truth for timing.
//!
//! This is the hard-real-time boundary (architecture.md "Audio plane — Rust"). The
//! callback must fill its buffer on time, every time, so `render` does no allocation,
//! no locking, and no JS calls — only ring draining (doc 02), voice synthesis (doc 03),
//! and arithmetic.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::error::AudioError;
use crate::ring::RingConsumer;
use crate::scheduler;
use crate::stats::EngineStatsShared;
use crate::voices::drum::KickVoice;

/// The audio format we actually opened (may differ from the request).
#[derive(Debug, Clone, Copy)]
pub struct AudioConfig {
    /// Frames per second, e.g. 48_000.
    pub sample_rate: u32,
    /// Requested frames per callback buffer (a *request*; hosts may ignore it).
    pub buffer_frames: u32,
    /// Output channel count, e.g. 2.
    pub channels: u16,
}

/// Pick a default output device and an `f32` stream config, honoring the requested
/// sample rate / buffer size when possible and otherwise falling back to the
/// device default. Returns what we actually got.
pub fn open_output(
    req_sr: Option<u32>,
    req_bf: Option<u32>,
) -> Result<(cpal::Device, cpal::StreamConfig, AudioConfig), AudioError> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoDevice)?;

    let default = device
        .default_output_config()
        .map_err(|e| AudioError::DefaultConfig(e.to_string()))?;

    // Phase 0 only builds f32 streams; reject anything else rather than converting.
    if default.sample_format() != cpal::SampleFormat::F32 {
        return Err(AudioError::UnsupportedFormat);
    }

    let sample_rate = req_sr.unwrap_or(default.sample_rate().0);
    let channels = default.channels().max(2); // render mono → fan to N

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

/// State the audio thread owns/reads. Constructed before the stream, moved into the
/// callback. No allocation happens after this point.
pub struct AudioState {
    pub channels: usize,
    pub sample_rate: f32,
    /// Shared *read* with `stats()`; the audio thread is the sole writer.
    pub frame_clock: Arc<AtomicU64>,
    pub consumer: RingConsumer,
    pub kick: KickVoice,
    pub stats: Arc<EngineStatsShared>,
}

/// Build (but do not start) the cpal output stream around `st`.
pub fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut st: AudioState,
) -> Result<cpal::Stream, AudioError> {
    // The error callback runs on a cpal-internal thread on stream errors (e.g. the
    // device is unplugged). Count it as an underrun; never allocate or panic.
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
/// No allocation, no locking, no JS. Same code drives native and (later) WASM.
#[inline]
pub fn render(st: &mut AudioState, data: &mut [f32]) {
    let ch = st.channels;
    debug_assert!(ch >= 1);
    let frames = data.len() / ch;
    let base = st.frame_clock.load(Ordering::Relaxed); // engine frame at buffer start

    // 1. Drain the ring: apply every event due this buffer at its in-buffer offset (doc 02).
    scheduler::drain(&mut st.consumer, base, frames as u64, &mut st.kick, &st.stats);

    // 2. Render mono into the interleaved output, sample by sample.
    for f in 0..frames {
        let mono = st.kick.tick(); // one f32 per frame; 0.0 when idle
        let off = f * ch;
        for c in 0..ch {
            data[off + c] = mono; // fan mono to all channels (centered)
        }
    }

    // 3. Advance the source-of-truth clock by exactly the frames we produced.
    st.frame_clock.store(base + frames as u64, Ordering::Relaxed);
}

/// Start the stream playing.
pub fn play(stream: &cpal::Stream) -> Result<(), AudioError> {
    stream
        .play()
        .map_err(|e| AudioError::PlayStream(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(channels: usize) -> AudioState {
        let (_producer, consumer) =
            crate::ring::ring(64, Arc::new(AtomicU64::new(0)));
        AudioState {
            channels,
            sample_rate: 48_000.0,
            frame_clock: Arc::new(AtomicU64::new(0)),
            consumer,
            kick: KickVoice::new(48_000.0),
            stats: Arc::new(EngineStatsShared::default()),
        }
    }

    #[test]
    fn frame_counter_is_exact_and_monotonic() {
        let mut st = state(2);
        let mut buf = vec![0.0f32; 256 * 2]; // 256 frames, stereo

        render(&mut st, &mut buf);
        assert_eq!(st.frame_clock.load(Ordering::Relaxed), 256);

        // A different buffer length advances by exactly data.len()/channels.
        let mut buf2 = vec![0.0f32; 128 * 2];
        render(&mut st, &mut buf2);
        assert_eq!(st.frame_clock.load(Ordering::Relaxed), 256 + 128);

        // Never decreases.
        let before = st.frame_clock.load(Ordering::Relaxed);
        render(&mut st, &mut buf);
        assert!(st.frame_clock.load(Ordering::Relaxed) >= before);
    }

    #[test]
    fn channel_fan_out_is_centered() {
        let mut st = state(2);
        st.kick.force_constant(0.5);
        let mut buf = vec![0.0f32; 64 * 2];
        render(&mut st, &mut buf);
        // Every even (L) and odd (R) index carries the same mono sample.
        for f in 0..64 {
            assert_eq!(buf[f * 2], 0.5, "left channel frame {f}");
            assert_eq!(buf[f * 2 + 1], 0.5, "right channel frame {f}");
        }
    }

    #[test]
    fn mono_device_renders() {
        let mut st = state(1);
        st.kick.force_constant(0.25);
        let mut buf = vec![0.0f32; 32];
        render(&mut st, &mut buf);
        assert!(buf.iter().all(|&s| s == 0.25));
        assert_eq!(st.frame_clock.load(Ordering::Relaxed), 32);
    }

    #[test]
    fn idle_voice_renders_silence() {
        let mut st = state(2);
        let mut buf = vec![1.0f32; 16 * 2];
        render(&mut st, &mut buf);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    // Real-device tests: require a working default output device, so they are
    // #[ignore]d in CI/headless and run manually with `cargo test -- --ignored`.
    #[test]
    #[ignore = "requires a real audio output device"]
    fn stream_opens_and_advances_clock() {
        use std::time::Duration;
        let (device, config, cfg) =
            open_output(Some(48_000), Some(256)).expect("open default output");
        let frame_clock = Arc::new(AtomicU64::new(0));
        let (_producer, consumer) = crate::ring::ring(4096, Arc::new(AtomicU64::new(0)));
        let st = AudioState {
            channels: cfg.channels as usize,
            sample_rate: cfg.sample_rate as f32,
            frame_clock: frame_clock.clone(),
            consumer,
            kick: KickVoice::new(cfg.sample_rate as f32),
            stats: Arc::new(EngineStatsShared::default()),
        };
        let stream = build_stream(&device, &config, st).expect("build stream");
        play(&stream).expect("play");
        std::thread::sleep(Duration::from_millis(200));
        let advanced = frame_clock.load(Ordering::Relaxed);
        let expected = (0.2 * cfg.sample_rate as f64) as u64;
        // Within one large buffer of the expected frame count.
        assert!(
            advanced.abs_diff(expected) < 4096,
            "advanced {advanced}, expected ~{expected}"
        );
    }
}
