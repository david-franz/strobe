# 01 — Audio I/O with cpal

## Purpose

This feature opens the audio device and runs the engine's heartbeat: a `cpal` output stream whose callback is invoked by the OS audio system once per buffer, forever, until we stop it. Inside that callback lives the **per-buffer render loop** that turns engine state into samples. This is the hard-real-time boundary described in [architecture.md](../../architecture.md#audio-plane--rust-hard-real-time): the callback **must** fill its buffer on time, every time, or the user hears a click. Everything else in the engine exists to be called from inside this loop without ever blocking it.

The second job of this feature is to maintain the **running `u64` frame counter** — the single source of truth for timing in the entire system. The JS clock is jittery and untrusted (see [architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks)); the sample count on the audio thread is exact. Every event's `frame` timestamp ([event-protocol.md](../../event-protocol.md#the-frame-timestamp)) is interpreted against this counter, and `stats().currentFrame` exports it so the TS scheduler can align its lookahead window.

## Location

```
engine/
  Cargo.toml                  # + cpal dependency
  src/
    lib.rs                    # Engine struct, ties modules together (shared with 02/03/04)
    audio_io.rs               # NEW — this feature: stream open, callback, render loop
    scheduler.rs              # (02) ring consumer, called from the render loop
    voices/
      drum.rs                 # (03) the kick voice, rendered in the render loop
```

This document owns `engine/src/audio_io.rs` and the `cpal` entry in `engine/Cargo.toml`. It defines the render-loop seam that [02-event-ring-buffer.md](02-event-ring-buffer.md) and [03-kick-voice.md](03-kick-voice.md) plug into.

## Design

### Cargo

```toml
# engine/Cargo.toml  (excerpt)
[dependencies]
cpal = "0.15"
```

### Device, sample rate, and buffer size selection

`start()` (driven from the napi bridge, [04-napi-bridge.md](04-napi-bridge.md)) takes optional `sampleRate` and `bufferFrames`. We honor them when the device supports the request and otherwise fall back to the device default, reporting what we actually got.

```rust
// engine/src/audio_io.rs
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct AudioConfig {
    pub sample_rate: u32,   // e.g. 48_000
    pub buffer_frames: u32, // e.g. 256  (a "frame" = one sample per channel)
    pub channels: u16,      // e.g. 2
}

pub fn open_output(req_sr: Option<u32>, req_bf: Option<u32>)
    -> Result<(cpal::Device, cpal::StreamConfig, AudioConfig), AudioError>
{
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(AudioError::NoDevice)?;

    // Find a supported f32 output config that matches the requested sample rate
    // if possible, else take the device's default output config.
    let default = device.default_output_config()?;          // a SupportedStreamConfig
    let sample_rate = req_sr.unwrap_or(default.sample_rate().0);
    let channels = default.channels().max(2);               // Phase 0: render mono → fan to N

    let mut config: cpal::StreamConfig = default.config();
    config.sample_rate = cpal::SampleRate(sample_rate);
    config.channels = channels;
    // Ask cpal for a fixed buffer size; some backends only support Default.
    config.buffer_size = match req_bf {
        Some(bf) => cpal::BufferSize::Fixed(bf),
        None => cpal::BufferSize::Default,
    };

    let cfg = AudioConfig { sample_rate, buffer_frames: req_bf.unwrap_or(256), channels };
    Ok((device, config, cfg))
}
```

Notes:
- A **frame** is one sample *per channel* (cpal terminology). The interleaved output slice has length `frames * channels`.
- We request `f32` interleaved output and only build an `f32` stream in Phase 0 (`device.build_output_stream::<f32, _, _>`). If a device cannot do `f32` we return `AudioError::UnsupportedFormat` rather than silently converting — Phase 0 keeps the path narrow.
- `BufferSize::Fixed` is a *request*; on CoreAudio (macOS, our first target) it is honored, but the callback must still cope with a variable `data.len()` because some hosts ignore it. The render loop below never assumes a fixed length.

### The audio callback and the per-buffer render loop

The callback is the closure handed to `build_output_stream`. It captures the shared audio-thread state by value (moved in once at stream creation) so it never touches a lock. The frame counter is an `AtomicU64` so the non-audio thread can read it for `stats()` without a lock; the audio thread is its only writer.

```rust
// engine/src/audio_io.rs
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// State the audio thread owns/reads. Constructed before the stream,
/// moved into the callback. No allocation happens after this point.
pub struct AudioState {
    pub channels: usize,
    pub sample_rate: f32,
    pub frame_clock: Arc<AtomicU64>,   // shared *read* with stats(); audio thread is sole writer
    pub consumer: scheduler::RingConsumer, // (02) ring buffer consumer side
    pub kick: voices::drum::KickVoice,     // (03) the single Phase 0 voice
    pub stats: Arc<EngineStatsShared>,     // underruns / late counters (atomics)
}

pub fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut st: AudioState,
) -> Result<cpal::Stream, AudioError> {
    let err_fn = |e| {
        // Called by cpal on a stream error (e.g. device unplugged). Count it as an underrun.
        // No allocation, no panic.
    };

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
            render(&mut st, data);
        },
        err_fn,
        None, // timeout
    )?;
    Ok(stream)
}

/// The per-buffer render loop. THIS IS THE HOT PATH.
/// No allocation, no locking, no JS. Same code drives native and (later) WASM.
#[inline]
fn render(st: &mut AudioState, data: &mut [f32]) {
    let ch = st.channels;
    debug_assert!(ch >= 1);
    let frames = data.len() / ch;
    let base = st.frame_clock.load(Ordering::Relaxed); // engine frame at buffer start

    // 1. Drain the ring: apply every event with frame <= base + frames to the
    //    voices, at its correct in-buffer offset. (Detailed in 02.)
    scheduler::drain(&mut st.consumer, base, frames as u64, &mut st.kick, &st.stats);

    // 2. Render mono into the interleaved output, sample by sample.
    for f in 0..frames {
        let mono = st.kick.tick();          // (03) one f32 per frame; 0.0 when idle
        let out = mono; // Phase 0: no master limiter yet (Phase 1 adds it)
        let off = f * ch;
        for c in 0..ch {
            data[off + c] = out;            // fan mono to all channels (center)
        }
    }

    // 3. Advance the source-of-truth clock by exactly the frames we produced.
    st.frame_clock.store(base + frames as u64, Ordering::Relaxed);
}
```

Key properties:
- **The frame counter is advanced by exactly `frames`** after each buffer — never by wall-clock estimate. `base` is read once at the top so all events in this buffer resolve against a stable buffer-start frame; the counter is published once at the end. This makes `currentFrame` monotonic and exact.
- **Channel handling:** Phase 0 synthesizes mono and fans the same sample to every output channel (centered). Real panning arrives with the effects rack (Phase 3). Writing the loop as `for c in 0..ch` keeps it correct for mono, stereo, or surround devices.
- **f32 interleaved:** `data` is `&mut [f32]` laid out `[L0,R0,L1,R1,…]`. We index with `f * ch + c`.

### Why the callback must never block

The OS calls this closure on a high-priority real-time thread with a hard deadline: at 48 kHz / 256 frames it has ~5.3 ms to return. Anything that can pause — a mutex (`lock()` may sleep on contention), a heap allocation (the allocator may take a global lock or hit the OS), a syscall, a `println!`, or a call back into the JS runtime — risks blowing the deadline and producing a **buffer underrun**: a click, pop, or dropout ([architecture.md](../../architecture.md#what-can-go-wrong-and-the-guardrails)). That is why:

- The ring consumer is lock-free (atomic head/tail; see [02-event-ring-buffer.md](02-event-ring-buffer.md)).
- The kick voice pre-allocates everything at construction and only does arithmetic per `tick()` (see [03-kick-voice.md](03-kick-voice.md)).
- Counters exposed to `stats()` are plain atomics, written with `Ordering::Relaxed`, never a lock.
- The error callback counts and returns; it never allocates or panics.

### Graceful start/stop

`cpal::Stream` plays from the moment `stream.play()` is called and stops when the `Stream` value is dropped. We hold the stream in the engine so its lifetime is explicit:

```rust
// engine/src/lib.rs (sketch — full version in 04)
pub struct Engine {
    stream: Option<cpal::Stream>,         // Some while playing; drop = stop
    frame_clock: Arc<AtomicU64>,
    stats: Arc<EngineStatsShared>,
    producer: scheduler::RingProducer,    // (02) kept on the engine for pushEvents
    cfg: AudioConfig,
}

impl Engine {
    pub fn start(&mut self, sr: Option<u32>, bf: Option<u32>) -> Result<(), AudioError> {
        if self.stream.is_some() { return Ok(()); }      // idempotent
        let (device, config, cfg) = audio_io::open_output(sr, bf)?;
        let state = self.build_audio_state(&cfg);         // moves consumer + kick in
        let stream = audio_io::build_stream(&device, &config, state)?;
        stream.play()?;
        self.stream = Some(stream);
        self.cfg = cfg;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stream = None; // dropping the Stream stops the callback cleanly
        // frame_clock is intentionally NOT reset: stats remain readable post-stop.
    }
}
```

- `start` is **idempotent** (a second call while playing is a no-op).
- `stop` drops the stream; cpal joins/tears down the audio thread on drop, so no callback runs after `stop` returns. No fade-out is needed in Phase 0 because dropping at a buffer boundary leaves silence, not a discontinuity in our own output.

## Interfaces

- **To [02-event-ring-buffer.md](02-event-ring-buffer.md):** the render loop calls `scheduler::drain(consumer, base, frames, &mut kick, stats)` once per buffer, passing the buffer-start frame `base` and length `frames`. The consumer half of the ring lives in `AudioState`; the producer half lives on the `Engine` for `pushEvents`.
- **To [03-kick-voice.md](03-kick-voice.md):** the render loop calls `kick.tick() -> f32` per frame and `kick.trigger(...)` (invoked by the drain step) on `NoteOn`. The voice is owned by `AudioState`.
- **To [04-napi-bridge.md](04-napi-bridge.md):** `Engine::start/stop` are the Rust functions the addon wraps. `frame_clock` and `stats` (both `Arc<Atomic…>`) are read by `Engine::stats()` from the non-audio thread.
- **Event protocol:** this feature defines `currentFrame` = the value of `frame_clock`, the engine-relative frame counter that every event's `frame` field ([event-protocol.md](../../event-protocol.md#the-frame-timestamp)) is measured against. It starts at 0 when the engine struct is created and advances only in `render`.

## Edge cases & risks

- **Variable buffer length.** Even with `BufferSize::Fixed`, some hosts deliver a different `data.len()`. The loop derives `frames = data.len()/ch` every call, so it is always correct; we never cache a buffer length.
- **Non-multiple buffer length.** `data.len()` should always be a multiple of `ch`; we `debug_assert!` it and, in release, the integer division simply ignores a stray partial frame (cannot happen on conformant backends).
- **Device default sample rate ≠ requested.** We report the actual rate via `AudioConfig`; the scheduler reads `stats()`/start-result and computes frames from the *real* sample rate, so timing stays correct ([05-lookahead-scheduler.md](05-lookahead-scheduler.md)).
- **Underrun under load.** If the loop ever misses its deadline, cpal/the OS produces a glitch; we cannot always detect it directly, so we treat the `err_fn` callback and any cpal stream error as an underrun increment, and rely on the stress check below to prove margin.
- **Frame-counter overflow.** `u64` frames at 48 kHz overflows in ~12 million years; not a concern.
- **No master limiter yet.** Phase 0 output is a single kick at controlled gain, so clipping is not a risk; the master bus + limiter is a Phase 1 item ([roadmap.md](../../roadmap.md#phase-1--synthesis-core)).

## Acceptance criteria

- [ ] **Stream opens on macOS.** `start({sampleRate:48000, bufferFrames:256})` returns Ok and audio is flowing. *Test:* a Rust integration test that opens the stream, lets it run 200 ms, and asserts `frame_clock` advanced by ≈ `0.2 * sample_rate` (± one buffer). Plus an **audible check**: a temporary test tone confirms sound comes out of the default device.
- [ ] **Frame counter is exact and monotonic.** *Test:* unit-test `render` directly with a stub buffer of known length and assert the clock advances by exactly `data.len()/channels` and never decreases across calls.
- [ ] **Correct interleaving / channel fan-out.** *Test:* feed `render` a 2-channel buffer with the kick forced to output a constant `0.5`; assert every even and odd index equals `0.5`.
- [ ] **Clean start/stop, idempotent.** *Test:* call `start` twice (second is a no-op), then `stop`; assert no callback runs after `stop` (a flag set in a wrapped voice tick stops changing) and the process exits without hang.
- [ ] **No allocation/locking on the hot path.** *How to test:* code review of `render` + drain + `tick` (no `Box`/`Vec`/`String`/`lock()` in the call tree), and a **stress check** — run the example under load for 60 s and confirm `stats()` reports zero underruns and zero late events (the roadmap "provably does no allocation/locking/JS" gate).
