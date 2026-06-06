# 02 — The sampler voice

## Purpose

This is the voice that actually *plays* a loaded sample. Given a buffer in the sample bank ([01-sample-decode-loading.md](01-sample-decode-loading.md)) and a `TrigSample` event off the ring, `voices/sampler.rs` reads the buffer back as audio with **variable speed/pitch** (a fractional read index with interpolation), **gain**, a **start offset**, **one-shot or looped** playback, and **reverse**. It is the sample-playback analogue of the Phase 0 kick voice ([phase-0/03](../phase-0/03-kick-voice.md)) and the Phase 1 synth/drum voices — another voice type in the same `voice_alloc` pool ([architecture.md](../../architecture.md#the-rust-engine-internals)).

The whole design is shaped by one rule: the voice **reads a shared immutable buffer and does nothing that allocates, locks, or blocks** ([architecture.md](../../architecture.md#audio-plane--rust-hard-real-time)). All the expensive preparation — decode, resample, and (in [03](03-timestretch-pitchshift.md)) time-stretch — already happened off the audio thread. By the time the voice runs, "playing a sample" is just: advance a fractional index by a per-sample increment, interpolate, scale by gain, mix. That is the property that lets a sampler live on the hard-real-time thread.

The voice is triggered exclusively by **`TrigSample` (`0x20`)**, whose args are fixed by the canonical protocol: `arg0` = speed/pitch, `arg1` = gain, `arg2` = start (frames), `arg3` = sample id ([event-protocol.md](../../event-protocol.md#opcodes)). Chop/slice ([04](04-chop-slice-granular.md)) is *entirely* expressed through those args — a slice is just a `start` offset plus a duration — so this one voice covers one-shots, chops, slices, and loops without special cases.

## Location

```
engine/
  src/
    voices/
      mod.rs           # + pub mod sampler;
      sampler.rs       # NEW — this feature: SamplerVoice
    sample/
      bank.rs          # (01) — SampleBank / SampleBuffer (read here)
    voice_alloc.rs     # (Phase 1) — SamplerVoice joins the pool
    scheduler.rs       # (Phase 0) — drains TrigSample → voice_alloc.trigger_sample(...)
    audio_io.rs        # (Phase 0) — render loop sums voices
```

This document owns `engine/src/voices/sampler.rs` and the `TrigSample` handling that routes into it.

## Design

### State

The voice holds a cloned `Arc<SampleBuffer>` handle (so the buffer survives eviction, [01](01-sample-decode-loading.md#the-engine-side-sample-bank-bankrs)) plus plain numeric playback state — no owned audio, nothing to allocate at trigger time.

```rust
// engine/src/voices/sampler.rs
use std::sync::Arc;
use crate::sample::bank::SampleBuffer;

#[derive(Clone, Copy, PartialEq)]
pub enum Interp { Linear, Cubic }

#[derive(Clone, Copy, PartialEq)]
pub enum LoopMode { OneShot, Loop }

pub struct SamplerVoice {
    // --- bound at trigger from a TrigSample (no alloc) ---
    buf: Option<Arc<SampleBuffer>>, // cloned handle; None = idle
    pos: f64,        // fractional read index, in source frames
    step: f64,       // per-output-frame increment = speed (and direction sign)
    gain: f32,
    region_start: f64, // start frame (arg2)  — slice/chop start
    region_end: f64,   // exclusive end frame — slice length / loop end / buffer end
    loop_mode: LoopMode,
    reverse: bool,
    interp: Interp,
    pending_offset: u32, // sub-buffer start offset (sample-accurate landing)
    active: bool,
    // a tiny fade to kill boundary clicks (start, loop-wrap, end)
    fade_frames: f64,    // e.g. 2 ms * sr, set once at trigger
}
```

### Triggering from `TrigSample`

`scheduler.rs` drains a `TrigSample`, looks the sample up in the bank, and calls `trigger_at`. Everything is arithmetic + an `Arc::clone` (a refcount bump, not an allocation):

```rust
impl SamplerVoice {
    /// One-shot/looped trigger from a TrigSample, at a sub-buffer sample offset.
    /// arg0=speed/pitch, arg1=gain, arg2=start(frames), arg3=sample id (already
    /// resolved to `buf` by the caller). NO allocation, NO lock.
    #[inline]
    pub fn trigger_at(
        &mut self,
        offset: usize,
        buf: Arc<SampleBuffer>,
        speed: f32, gain: f32, start_frame: f32,
        // packed flags / extra (see "Flags" below): loop, reverse, region length, interp
        opts: TrigOpts,
    ) {
        let frames = buf.frames as f64;
        self.reverse = opts.reverse;
        self.loop_mode = if opts.looped { LoopMode::Loop } else { LoopMode::OneShot };
        self.interp = opts.interp;
        self.gain = gain.clamp(0.0, 4.0);
        self.step = (speed.abs() as f64).max(1e-6);
        self.region_start = (start_frame as f64).clamp(0.0, frames);
        self.region_end = match opts.region_len {
            Some(len) => (self.region_start + len as f64).min(frames),
            None      => frames,
        };
        self.pos = if self.reverse { self.region_end - 1.0 } else { self.region_start };
        self.pending_offset = offset as u32;
        self.fade_frames = (0.002 * buf.sample_rate as f64).max(1.0);
        self.buf = Some(buf);
        self.active = true;
    }
}
```

> **Where `TrigOpts` comes from.** The base protocol fixes `arg0..arg3`. `reverse`, `loop`, `interp`, and an explicit `region_len` (slice length) are carried in the record's **`flags`** field (offset 28) and, for `region_len`, in a packed encoding the scheduler controls — see [event-protocol.md](../../event-protocol.md#event-record-layout) (the 32-byte record reserves `flags/seq`). Reverse and the slice length are exactly how `.rev()` and `.chop(n)` reach the voice ([04](04-chop-slice-granular.md#how-slices-map-to-trigsample)).

### The read / interpolation loop

The core is `tick()`: produce one stereo output frame, advance the fractional index, interpolate. This is the hot path; it is branch-light and allocation-free.

```rust
impl SamplerVoice {
    /// One output frame (mono summed to the caller's mix, or stereo pair).
    /// Returns (L, R). 0.0 when idle. Honors the sub-buffer start offset.
    #[inline]
    pub fn tick(&mut self) -> (f32, f32) {
        if !self.active { return (0.0, 0.0); }
        if self.pending_offset > 0 { self.pending_offset -= 1; return (0.0, 0.0); }

        let buf = match &self.buf { Some(b) => b, None => return (0.0, 0.0) };
        let nch = buf.channels as usize;

        // Sample each channel at the fractional position.
        let i = self.pos.floor() as isize;
        let frac = (self.pos - i as f64) as f32;
        let mut l = self.read_channel(buf, 0, i, frac);
        let mut r = if nch > 1 { self.read_channel(buf, 1, i, frac) } else { l };

        // Boundary fades (start-in, loop-wrap, end-out) to avoid clicks.
        let env = self.boundary_env() as f32;
        l *= self.gain * env;
        r *= self.gain * env;

        // Advance (reverse = negative step).
        self.pos += if self.reverse { -self.step } else { self.step };

        // Region / loop handling.
        if !self.reverse && self.pos >= self.region_end - 1.0 {
            match self.loop_mode {
                LoopMode::Loop => { self.pos = self.region_start + (self.pos - (self.region_end - 1.0)); }
                LoopMode::OneShot => { self.active = false; }
            }
        } else if self.reverse && self.pos <= self.region_start {
            match self.loop_mode {
                LoopMode::Loop => { self.pos = self.region_end - 1.0 - (self.region_start - self.pos); }
                LoopMode::OneShot => { self.active = false; }
            }
        }
        (l, r)
    }

    /// Interpolated read of one channel at integer index `i` + `frac`.
    #[inline]
    fn read_channel(&self, buf: &SampleBuffer, c: usize, i: isize, frac: f32) -> f32 {
        let ch = buf.channel(c);
        let n = ch.len() as isize;
        let at = |k: isize| -> f32 {
            if k < 0 || k >= n { 0.0 } else { ch[k as usize] }
        };
        match self.interp {
            Interp::Linear => {
                let a = at(i); let b = at(i + 1);
                a + (b - a) * frac
            }
            Interp::Cubic => {
                // Catmull-Rom 4-point interpolation: smoother, fewer HF artifacts
                // when pitching/slowing a sample.
                let x0 = at(i - 1); let x1 = at(i); let x2 = at(i + 1); let x3 = at(i + 2);
                let t = frac;
                let a0 = -0.5*x0 + 1.5*x1 - 1.5*x2 + 0.5*x3;
                let a1 =       x0 - 2.5*x1 + 2.0*x2 - 0.5*x3;
                let a2 = -0.5*x0           + 0.5*x2;
                let a3 =                 x1;
                ((a0*t + a1)*t + a2)*t + a3
            }
        }
    }
}
```

**Speed = pitch (varispeed).** The increment `step` *is* the speed ratio: `step = 2.0` reads twice as fast (octave up, half as long); `step = 0.5` reads half as fast (octave down, twice as long). This is "vinyl"/tape pitch — speed and pitch are coupled. **Decoupling** them (stretch without pitch, pitch without stretch) is the job of [03-timestretch-pitchshift.md](03-timestretch-pitchshift.md), which prepares a *new buffer* offline; the voice then plays it at `step = 1.0`. So this voice deliberately stays simple: it only does coupled varispeed, the cheap and artifact-free case.

`arg0` (speed/pitch) is the linear speed ratio. A semitone helper lives in TS: `ratio = 2^(semitones/12)` is computed in the control plane and sent as `arg0` ([04](04-chop-slice-granular.md) / [03](03-timestretch-pitchshift.md)), so the voice never does a `pow` per trigger.

### Interpolation choice (linear vs cubic)

- **Linear** — 2-point, cheapest, fine at `step ≈ 1.0` and for percussive material; some HF loss / aliasing when pitched far up.
- **Cubic (Catmull-Rom)** — 4-point, smoother, noticeably cleaner when slowing/pitching down or for sustained/tonal samples. Default to **cubic** for `sample()` playback (quality first; the cost is a handful of multiplies per frame), with `Linear` selectable via a flag for very high polyphony / granular clouds ([04](04-chop-slice-granular.md)) where many grains run at once.

### Boundary fades (de-click)

A raw sample whose region doesn't start/end at a zero crossing clicks. `boundary_env()` applies a short (~2 ms) linear fade-in at region start, fade-out approaching region end, and a fade across each loop wrap. This is the single most important artifact guard for chop/slice, where arbitrary slice boundaries are the norm.

```rust
#[inline]
fn boundary_env(&self) -> f64 {
    let from_start = (self.pos - self.region_start).abs();
    let to_end = (self.region_end - 1.0 - self.pos).abs();
    let edge = from_start.min(to_end).min(self.fade_frames);
    edge / self.fade_frames            // 0..1 ramp near edges, 1.0 in the middle
}
```

### Integration with `voice_alloc`

`SamplerVoice` joins the existing polyphony pool ([architecture.md](../../architecture.md#the-rust-engine-internals), Phase 1). `voice_alloc` already steals the oldest/quietest voice when the pool is full; the sampler participates identically. The scheduler routes a `TrigSample` like a one-shot `NoteOn`:

```rust
// engine/src/scheduler.rs (drain loop, excerpt)
Opcode::TrigSample => {
    let id = ev.arg3 as u32;                 // sample id
    if let Some(buf) = bank.lookup(id) {     // RT-safe ArcSwap load
        if let Some(v) = voice_alloc.alloc_sampler() {  // may steal oldest/quietest
            v.trigger_at(offset, buf, ev.arg0, ev.arg1, ev.arg2, TrigOpts::from_flags(ev.flags));
        }
    }
    // unknown id → no-op (counts a "missing sample" stat); never blocks.
}
```

Each active sampler voice's `(L, R)` is summed in the render loop alongside synth/drum voices, then the sum goes through the per-track effects chain ([roadmap.md#phase-3](../../roadmap.md#phase-3--effects-rack)) and the master limiter — so a sample feeds filters/delay/reverb/sidechain exactly like a synth voice, no sampler-specific routing.

### Real-time safety

- The voice **owns a cloned `Arc<SampleBuffer>`**, so eviction of its id in the bank does not free the memory it is reading ([01](01-sample-decode-loading.md#eviction-vs-in-flight-voices)).
- When a voice finishes (`active = false`), its `Arc` must be **dropped off the audio thread** (the last drop could `free`). The voice sets `buf = None` only via a hand-back channel: it pushes the old `Arc` to an SPSC "garbage" queue the setup thread drains and drops. The audio thread never calls `free`.

## Interfaces

- **Event protocol — consumes `TrigSample` (`0x20`):** `arg0`=speed/pitch ratio, `arg1`=gain, `arg2`=start frame, `arg3`=sample id; `flags` carries reverse / loop / interp / (packed) region length ([event-protocol.md](../../event-protocol.md#opcodes)). No `NoteOff` for one-shots; a *looped* sample is stopped either by its region ending (one-shot) or by a future explicit stop (a looped pad would use the same gating model as a gated note — out of Phase 4 scope; `.loop()` in [04](04-chop-slice-granular.md) loops a region for a held duration).
- **Sample bank ([01](01-sample-decode-loading.md)):** reads `bank.lookup(id) -> Option<Arc<SampleBuffer>>`, then `buffer.channel(c)`.
- **`voice_alloc` (Phase 1):** `alloc_sampler()` returns a free/stolen `SamplerVoice`; the sampler obeys the same stealing policy as every voice.
- **Pattern engine:** `sample("amen")` ([01](01-sample-decode-loading.md#ts-side-api-loadts)) compiles each `Hap` to a `TrigSample`. The chainable methods that set these args — `.speed()`, `.gain()`, `.begin()/.end()` (region), `.rev()`, `.loop()`, `.chop()/.slice()` — are documented in [04](04-chop-slice-granular.md); `.pitch()`/`.stretch()` (decoupled) in [03](03-timestretch-pitchshift.md).

## Edge cases & risks

- **Real-time safety (headline).** `tick`/`trigger_at` do only arithmetic + an `Arc::clone`; no decode, no resample, no stretch, no lock, no `free` on the audio thread. The `Arc` drop is deferred to the setup thread via the garbage SPSC. Covered by review + the stress check.
- **Boundary clicks.** Arbitrary chop/slice starts rarely hit zero crossings → the ~2 ms boundary fades (start, loop-wrap, end) suppress clicks. Verified by a null/threshold test on slice boundaries.
- **Interpolation artifacts.** Linear loses HF when pitched up; default to cubic for tonal material, linear for dense granular clouds. Extreme pitch-up will alias (no oversampling filter) — documented as a known limit; the decoupled pitch path ([03](03-timestretch-pitchshift.md)) is the artifact-managed alternative for large shifts.
- **Out-of-range reads.** `read_channel` clamps to silence outside `[0, frames)`; `region_start/end` are clamped at trigger; cubic reads of `i-1`/`i+2` near edges return `0.0` (and the fade masks the discontinuity).
- **Reverse loop math.** Reverse playback decrements `pos`; loop-wrap mirrors the region. Tested explicitly so a reversed loop doesn't drift or stall.
- **Denormals on the fade tail.** The fade reaches exactly `0.0` at the edge and the voice goes `active = false`, so idle output is exactly `0.0` (no denormal tail) — same guard as the kick ([phase-0/03](../phase-0/03-kick-voice.md#edge-cases--risks)).
- **Polyphony pressure.** Granular clouds ([04](04-chop-slice-granular.md)) can request many voices; `voice_alloc` steals oldest/quietest. A grain-pool variant (many cheap grain readers inside one voice) is the mitigation if per-grain `voice_alloc` pressure is too high — noted there.

## Acceptance criteria

- [ ] **Plays a sample at unity.** *Unit test:* `trigger_at(speed=1.0, start=0, full region)` of a known buffer renders output equal to the buffer (within interpolation tolerance); length matches `frames`.
- [ ] **Varispeed = pitch.** *Unit test:* `speed=2.0` halves the duration and (FFT) doubles the dominant frequency; `speed=0.5` doubles duration and halves frequency.
- [ ] **Start offset / slice.** *Unit test:* `start=N`, `region_len=L` plays exactly frames `[N, N+L)`; first audible sample corresponds to source frame `N`.
- [ ] **Reverse.** *Unit test:* `reverse=true` over a region renders the region's samples in reverse order (allowing the boundary fades).
- [ ] **Loop wraps cleanly.** *Unit test:* a looped region produces a continuous, periodic output across the wrap with no sample discontinuity above the fade threshold; reversed loop also wraps correctly.
- [ ] **De-click.** *Threshold test:* a slice whose boundaries are non-zero-crossings produces no inter-sample jump above the fade tolerance at start/end/wrap.
- [ ] **Sub-buffer offset honored.** *Unit test:* `trigger_at(offset=64,…)` emits exactly `0.0` for frames `0..64` of the buffer, first nonzero at `64` — sample-accurate landing (same guarantee as [phase-0/03](../phase-0/03-kick-voice.md)).
- [ ] **Real-time-safe.** *How to test:* review (no alloc/lock/decode/stretch/`free` in `tick`/`trigger_at`; `Arc` drop deferred) + the 60 s stress check triggering many overlapping samples with **zero underruns**.
