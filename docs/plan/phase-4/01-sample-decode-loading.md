# 01 — Sample decode & loading

## Purpose

Before any sample can play, recorded audio has to become a buffer the engine can read on the audio thread. This feature is that pipeline: **decode** a WAV/MP3/FLAC/OGG file (or in-memory bytes) to **Float32 interleaved PCM**, **resample** it to the engine's running sample rate, hand it to the engine through the **`loadSample()` control call** ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)), and store it in an engine-side **sample bank** that the sampler voice ([02-sampler-voice.md](02-sampler-voice.md)) can look up by numeric **sample id** in a real-time-safe way.

The defining constraint is the project's core rule: **decoding and resampling are not real-time work and must never touch the audio thread** ([architecture.md](../../architecture.md#the-core-principle-two-planes)). `symphonia` decode allocates, reads files, and runs for milliseconds-to-seconds; it runs entirely on the non-real-time setup thread. The audio thread only ever sees a finished, immutable buffer it can read with zero allocation and zero locking. The whole id-based, `Arc<[f32]>`-backed design below exists to make that handoff safe.

`symphonia` is the locked decode crate — pure Rust, no native codec dependencies ([architecture.md](../../architecture.md#why-these-technology-choices)). This feature is the foundation every other Phase 4 feature stands on: the sampler voice reads its buffers, chop/slice ([04](04-chop-slice-granular.md)) addresses regions of them, time-stretch ([03](03-timestretch-pitchshift.md)) produces new ones, and YouTube ingest ([05](05-youtube-ingest.md)) is just another producer that ends in `loadSample`.

## Location

```
engine/
  Cargo.toml              # + symphonia (with codec features), rubato (resampler)
  src/
    sample/
      mod.rs              # NEW — pub mod decode; pub mod bank;
      decode.rs           # NEW — symphonia decode → interleaved f32 + resample
      bank.rs             # NEW — SampleBank: Vec<Option<Arc<SampleBuffer>>>, lookup by id
    voices/
      sampler.rs          # (02) reads SampleBank buffers
bindings/native/
  src/lib.rs              # + napi method `load_sample(pcm, channels, sampleRate) -> u32`
packages/tekno/
  src/
    sample/
      load.ts             # NEW — decode-on-disk path + engine.loadSample wrapper, `sample()`
      youtube.ts          # (05) ends by calling the same loadSample path
```

This document owns `engine/src/sample/decode.rs`, `engine/src/sample/bank.rs`, the `load_sample` napi method, and `packages/tekno/src/sample/load.ts`.

## Design

### Where decode happens (TS vs Rust)

There are two valid producers of PCM, and the `loadSample` control call accepts both, so the bridge stays simple:

- **TS-side decode is the default for `tekno`'s own files and YouTube.** The TS control plane already has the bytes (a local file it read, or the WAV `ffmpeg` produced in [05](05-youtube-ingest.md)). For local files we can decode in Rust and only pass a path, but to keep one bridge shape, the canonical control call is **`loadSample(pcm: Float32Array, channels, sampleRate)`** ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)) — TS hands the engine ready PCM.
- **Rust-side decode is exposed too** (a thin `decodeFile(path) -> {pcm, channels, sampleRate}` napi helper) so the heavy `symphonia` work runs in Rust on a worker thread, and TS just forwards the result into `loadSample`. This is the path `sample()` uses for local files.

Either way, **`loadSample` is the single chokepoint into the engine**, it is non-real-time, and it returns a `sample id`.

### `symphonia` decode → interleaved f32 (`decode.rs`)

```toml
# engine/Cargo.toml (excerpt)
[dependencies]
symphonia = { version = "0.5", features = ["mp3", "aac", "flac", "isomp4", "ogg", "vorbis", "wav", "pcm"] }
rubato = "0.15"   # high-quality async/sinc resampler, pure Rust
```

```rust
// engine/src/sample/decode.rs
use symphonia::core::audio::SampleBuffer as SymSampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decoded audio, interleaved Float32, at its *source* sample rate.
pub struct DecodedPcm {
    pub samples: Vec<f32>,   // interleaved: [L0,R0,L1,R1,...] (or mono [s0,s1,...])
    pub channels: u16,
    pub sample_rate: u32,
}

/// Decode any container/codec symphonia supports from raw bytes.
/// NON-REAL-TIME: allocates, runs for ms..s. Call on a worker thread only.
pub fn decode_bytes(bytes: Vec<u8>, ext_hint: Option<&str>) -> Result<DecodedPcm, DecodeError> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext_hint { hint.with_extension(ext); }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())?;

    let mut out: Vec<f32> = Vec::new();
    let mut channels = 0u16;
    let mut sample_rate = 0u32;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(_)) => break, // EOF
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id { continue; }
        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        channels = spec.channels.count() as u16;
        sample_rate = spec.rate;
        // Convert whatever native sample format → interleaved f32 in [-1,1].
        let mut buf = SymSampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        out.extend_from_slice(buf.samples());
    }

    Ok(DecodedPcm { samples: out, channels, sample_rate })
}
```

### Resample to the engine rate (`rubato`)

The engine renders at one sample rate (chosen by `cpal` at `start()`; commonly 48 kHz). A sample decoded at 44.1 kHz **must be resampled to the engine rate at load time** so the sampler voice never has to do source-rate conversion on the audio thread — its only runtime rate change is the musical speed/pitch ratio ([02](02-sampler-voice.md)).

```rust
// engine/src/sample/decode.rs (cont.)
use rubato::{SincFixedIn, SincInterpolationParameters, Resampler};

/// Resample interleaved f32 from `src_rate` to `dst_rate`. NON-REAL-TIME.
/// Returns interleaved f32 at dst_rate. No-op (clone-free move) if rates match.
pub fn resample(pcm: DecodedPcm, dst_rate: u32) -> DecodedPcm {
    if pcm.sample_rate == dst_rate { return pcm; }
    let ch = pcm.channels as usize;
    // De-interleave into per-channel planes for rubato.
    let frames = pcm.samples.len() / ch;
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); ch];
    for f in 0..frames {
        for c in 0..ch { planar[c].push(pcm.samples[f * ch + c]); }
    }
    let ratio = dst_rate as f64 / pcm.sample_rate as f64;
    let mut rs = SincFixedIn::<f32>::new(
        ratio, 2.0, SincInterpolationParameters::default(), frames.max(1), ch,
    ).expect("resampler");
    let out_planar = rs.process(&planar, None).expect("resample");
    // Re-interleave.
    let out_frames = out_planar[0].len();
    let mut samples = Vec::with_capacity(out_frames * ch);
    for f in 0..out_frames {
        for c in 0..ch { samples.push(out_planar[c][f]); }
    }
    DecodedPcm { samples, channels: pcm.channels, sample_rate: dst_rate }
}
```

> We keep the source as **interleaved** across the bridge (matching the `loadSample(pcm, channels, sampleRate)` contract) and de-interleave only transiently inside the resampler. The bank stores whichever layout the voice prefers — see below.

### The engine-side sample bank (`bank.rs`)

The bank is the real-time-safe lookup the audio thread uses. The design goals are: (1) **O(1) lookup by id**, (2) the audio thread **never allocates and never blocks**, (3) loads/evictions from the setup thread are **visible to the audio thread without a lock on the hot path**.

```rust
// engine/src/sample/bank.rs
use std::sync::Arc;

/// An immutable, ready-to-play buffer. Stored de-interleaved (planar) so the
/// sampler voice reads one contiguous channel slice with a fractional index.
pub struct SampleBuffer {
    pub channels: u16,
    pub sample_rate: u32,        // == engine rate after resample
    pub frames: usize,           // per-channel length
    pub data: Box<[f32]>,        // planar: [ch0 frames...][ch1 frames...]
}

impl SampleBuffer {
    #[inline] pub fn channel(&self, c: usize) -> &[f32] {
        let n = self.frames;
        &self.data[c * n .. c * n + n]
    }
}

/// The bank. The audio thread holds an `Arc<BankSnapshot>`; the setup thread
/// builds a new snapshot on each load/evict and atomically swaps it in
/// (the double-buffered / atomically-swapped state from event-protocol.md).
pub struct BankSnapshot {
    /// Index = sample id. `None` = evicted/empty slot. Arc so the voice can
    /// clone a cheap handle and keep playing even if the bank is swapped.
    pub slots: Vec<Option<Arc<SampleBuffer>>>,
}

pub struct SampleBank {
    current: arc_swap::ArcSwap<BankSnapshot>,  // lock-free read on audio thread
    next_id: std::sync::atomic::AtomicU32,
    budget_frames: usize,                       // memory cap (see eviction)
}

impl SampleBank {
    /// NON-REAL-TIME: called from the control thread inside `load_sample`.
    /// Builds a new snapshot (copy of the Vec of Arcs — cheap, Arc clones only)
    /// and swaps it in. The audio thread sees old-or-new, never a torn state.
    pub fn insert(&self, buf: Arc<SampleBuffer>) -> u32 {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cur = self.current.load();
        let mut slots = cur.slots.clone();           // Vec<Option<Arc<..>>> clone = Arc refcount bumps
        if id as usize >= slots.len() { slots.resize((id as usize) + 1, None); }
        slots[id as usize] = Some(buf);
        self.current.store(Arc::new(BankSnapshot { slots }));
        id
    }

    /// REAL-TIME-SAFE: the audio thread calls this. One atomic load + index.
    /// Returns a borrowed handle valid for this buffer render; no alloc, no lock.
    #[inline]
    pub fn lookup(&self, id: u32) -> Option<Arc<SampleBuffer>> {
        let snap = self.current.load();              // arc_swap: lock-free, wait-free read
        snap.slots.get(id as usize).and_then(|s| s.clone())
    }
}
```

```toml
# engine/Cargo.toml (excerpt)
arc-swap = "1"
```

Why `arc_swap::ArcSwap` rather than a `Mutex<Vec<...>>`: the audio thread must read the bank with **no lock** ([architecture.md](../../architecture.md#audio-plane--rust-hard-real-time)). `ArcSwap` gives a wait-free `load()` on the reader side; the writer (setup thread) builds a fresh snapshot and `store`s it. A voice that has already cloned its `Arc<SampleBuffer>` keeps playing the *old* buffer to completion even if the bank is swapped or that slot evicted — the buffer's memory is freed only when the last `Arc` (including the voice's) drops, which happens off the audio thread (the voice hands the `Arc` back to the setup thread to drop, never dropping it itself on the hot path; see [02](02-sampler-voice.md#real-time-safety)).

### Memory management / eviction

Samples are big (a 10 s stereo clip at 48 kHz ≈ 3.8 MB). The bank enforces a configurable **`budget_frames`** cap.

- On `insert`, if total resident frames would exceed the budget, evict by policy: **LRU by last-trigger time** (the scheduler records a `lastUsedCycle` per sample id in TS and the engine tracks last-trigger frame; default policy = least-recently-triggered first).
- Eviction sets the slot to `None` in the next snapshot. **In-flight voices are unaffected** because they hold their own `Arc` (the buffer's memory survives until they finish).
- **Explicit control**: `engine.unloadSample(id)` (a control call) for deterministic freeing, and a TS `sample(...)` handle whose GC can trigger `unloadSample`.
- Pinning: samples loaded for the *current* song's tracks are pinned (not eligible for automatic eviction) so a live performance never loses a buffer mid-set; only ad-hoc/preview loads are evictable.

### TS-side API (`load.ts`)

```ts
// packages/tekno/src/sample/load.ts
export interface SampleHandle {
  id: number;               // engine sample id
  channels: number;
  frames: number;           // per-channel, at engine rate
  sampleRate: number;       // engine rate
  name?: string;
}

// Load a local file (Rust decodes on a worker thread, then loadSample).
export async function loadFile(path: string): Promise<SampleHandle>;

// Load already-decoded PCM (used by youtube.ts after ffmpeg, and tests).
export async function loadPcm(
  pcm: Float32Array, channels: number, sampleRate: number, name?: string,
): Promise<SampleHandle>;

// `sample("amen")` — resolves a name to a file via a sample search path / registry,
// loads it (cached by absolute path), and returns a *pattern* of a one-shot trigger.
// This is the entry point the chop/slice/stretch chain (04, 03) builds on.
export function sample(nameOrHandle: string | SampleHandle): SamplePattern;
```

`loadFile`/`loadPcm` are idempotent per source (an in-TS map from `absolutePath`/`pcm-hash` → `SampleHandle`) so re-running a song line during live coding does not reload or re-decode the same file.

## Interfaces

- **Control call (the bridge):** `loadSample(pcm: Float32Array, channels: number, sampleRate: number): Promise<number>` — exactly as declared in [event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time). Returns the **sample id**. Non-real-time; the engine de-interleaves, builds a `SampleBuffer`, and `bank.insert`s it. (`sampleRate` here is the *PCM's* rate; the engine resamples to its own rate if they differ — though `loadFile` already resamples in Rust, so PCM arriving via `loadPcm` from `ffmpeg` is requested at the engine rate to avoid a double pass.)
- **Companion control calls (new, same non-real-time surface):** `unloadSample(id)`, and a thin `decodeFile(path) -> {pcm, channels, sampleRate}` Rust helper for the `loadFile` path.
- **Event protocol:** this feature produces no events itself; it produces the **sample id** that `TrigSample` (`0x20`) carries in `arg3` ([event-protocol.md](../../event-protocol.md#opcodes)). The id is the contract between this feature and [02-sampler-voice.md](02-sampler-voice.md).
- **Pattern engine:** `sample(name)` returns a `Pattern` of control maps whose value carries `{ s: name, sampleId, ... }`. The scheduler compiles each `Hap` into a `TrigSample` ([04](04-chop-slice-granular.md) covers slice offsets). Sample loading is *async*; `sample(...)` returns a pattern immediately and the scheduler emits `TrigSample` only once the id resolves (it suppresses triggers for an unresolved sample, counting them as "pending", never blocking the scheduler tick).

## Edge cases & risks

- **Real-time safety (the headline).** `symphonia` decode and `rubato` resample **never run on the audio thread** — they run inside `load_sample`/`decodeFile` on a worker/setup thread ([architecture.md](../../architecture.md#audio-plane--rust-hard-real-time)). The audio thread's only bank interaction is `bank.lookup` = one wait-free `ArcSwap::load` + an index. Covered by code review and the stress check.
- **Sample-rate mismatch.** Always resample to the engine rate at load so the voice runs at the buffer's native rate when speed = 1.0. Skipping this would force per-sample rate conversion on the audio thread — forbidden.
- **Channel count.** Mono and stereo are first-class; >2 channels are downmixed to stereo at decode time (documented). The bank stores planar so the voice indexes channels uniformly.
- **Memory blow-up.** The `budget_frames` cap + eviction (LRU, pin current-song samples) bound resident memory; `unloadSample` gives explicit control. Loading a 2-hour file is rejected with a friendly error (a `maxDurationSec` guard, default e.g. 600 s) — the project's posture is *short* grabs.
- **Decode failure / unsupported codec.** `decode_bytes` returns `DecodeError`; the TS wrapper surfaces a friendly message ("couldn't decode <file>: unsupported codec") and the song keeps playing (the sample just never triggers).
- **Eviction vs in-flight voices.** Solved by `Arc`: a playing voice owns its buffer's lifetime; eviction only removes the *id→buffer* mapping, never frees memory a voice is reading. The `Arc` drop is deferred to the setup thread so no `free()` happens on the audio thread.
- **Double-resample.** PCM coming from `ffmpeg` ([05](05-youtube-ingest.md)) is requested already at the engine rate, so `resample` is a no-op move — avoiding a needless second sinc pass.

## Acceptance criteria

- [ ] **Round-trip decode.** *Unit test (Rust):* `decode_bytes` of a known WAV/MP3/FLAC/OGG fixture yields the expected frame count (±codec priming), 2 channels, and the file's sample rate; samples are finite and within `[-1, 1]`.
- [ ] **Resample correctness.** *Golden/null test:* resample a 44.1 kHz sine to 48 kHz; assert the dominant frequency is unchanged (FFT peak within tolerance) and length scales by `48000/44100`. Rates-equal input is returned unchanged.
- [ ] **`loadSample` returns a usable id.** *Integration test:* `loadPcm(pcm, 2, engineRate)` returns an id; `bank.lookup(id)` on the audio-thread side returns a buffer with matching `frames`/`channels`; an unknown id returns `None` without panicking.
- [ ] **Real-time-safe lookup.** *How to test:* code review that `bank.lookup` does only `ArcSwap::load` + index + `Arc::clone` (no alloc, no `Mutex`); the 60 s stress check ([phase-0/01](../phase-0/01-audio-io-cpal.md)) loads/evicts samples from the setup thread while triggering, with **zero underruns**.
- [ ] **Eviction respects in-flight voices.** *Integration test:* trigger a long sample, evict its id mid-playback, assert the voice plays to completion (no glitch, no `None` deref) and the buffer memory frees only after the voice ends — and the `free` happens off the audio thread.
- [ ] **Idempotent loads.** *Unit test (TS):* calling `loadFile(path)` twice returns the same `SampleHandle.id` and does not re-decode.
- [ ] **Friendly failures.** *Unit test (TS):* an unsupported/corrupt file rejects with a clear message and does not crash the engine; an over-long file is rejected by the `maxDurationSec` guard.
