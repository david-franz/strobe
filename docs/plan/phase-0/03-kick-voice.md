# 03 — The synthesized kick voice

## Purpose

Phase 0 needs exactly one sound: a kick drum. This feature builds it as a **`fundsp` graph** — the functional-DSP approach locked in by [architecture.md](../../architecture.md#why-these-technology-choices) — so that the proof-of-spine makes a real, recognizably-good techno/hip-hop kick rather than a bare beep. The classic 808/909 kick is a *pitched sine with a fast downward pitch envelope* (the "click" attack that snaps from a high frequency down to the body fundamental) shaped by a *fast amplitude envelope* (a sharp attack and an exponential-ish decay). That click→body shape is the whole character of the sound.

The voice is **one-shot triggered** by a `NoteOn` event drained from the ring ([02-event-ring-buffer.md](02-event-ring-buffer.md)): the trigger carries the body frequency (`arg0`), velocity (`arg1`), and a duration in frames (`arg2`) so no `NoteOff` is needed ([event-protocol.md](../../event-protocol.md#voice-ids)). Critically, it must be **allocation-free per trigger** — the graph is built once at construction; triggering only resets phase/envelope counters and arithmetic runs per sample. This is what keeps it legal on the hard-real-time audio thread ([architecture.md](../../architecture.md#audio-plane--rust-hard-real-time)). It is the direct precursor to the full `voices/drum.rs` synthesized kit in Phase 1 ([roadmap.md](../../roadmap.md#phase-1--synthesis-core)).

## Location

```
engine/
  Cargo.toml          # + fundsp dependency
  src/
    voices/
      mod.rs          # NEW — `pub mod drum;`
      drum.rs         # NEW — this feature: KickVoice
    scheduler.rs      # (02) calls kick.trigger_at(...)
    audio_io.rs       # (01) calls kick.tick() per frame
```

This document owns `engine/src/voices/drum.rs` and the `fundsp` dependency. Phase 0 has a single monophonic kick (no `voice_alloc`; that arrives in Phase 1).

## Design

### Cargo

```toml
# engine/Cargo.toml (excerpt)
[dependencies]
fundsp = "0.18"
```

### Parameters

| Field | Default | Meaning |
| --- | --- | --- |
| `body_hz` | from `NoteOn.arg0` (fallback 55.0) | the fundamental the kick settles to |
| `click_hz` | `body_hz * 8.0` (≈ a few hundred Hz) | the pitch the envelope starts at — the attack snap |
| `pitch_decay` | `0.03` s | time constant of the downward pitch sweep (fast → "click") |
| `amp_attack` | `0.001` s | near-instant attack |
| `amp_decay` | from `NoteOn.arg2` (dur_frames), default `0.18` s | body length |
| `drive` | `1.5` | gentle saturation for weight (a `tanh` shaper) |
| `gain` | from `NoteOn.arg1` (velocity) | output level |

### The fundsp graph

`fundsp` composes DSP nodes with operators (`>>` pipe, `|` stack, `*` multiply). The kick is: an exponentially-decaying **pitch envelope** drives the frequency input of a **sine oscillator**; the oscillator output is multiplied by an **amplitude envelope**; the product is softened by a `tanh` waveshaper. In `fundsp`'s closure-envelope style:

```rust
// Conceptual graph (built per-trigger params via shared() handles so we don't
// rebuild the graph). `t` is seconds-since-trigger, tracked by the envelope nodes.
//
//   pitch(t) = body + (click - body) * exp(-t / pitch_decay)
//   amp(t)   = attack-then-exp-decay window over [0, dur]
//
//   sine_hz( pitch(t) ) * amp(t) * gain >> shape(Tanh, drive)
//
// fundsp sketch:
//   (envelope(move |t| pitch_at(t))  >> sine())            // pitched body+click
//     * envelope(move |t| amp_at(t))                       // amp env
//     * dc(gain)                                           // velocity
//     >> shape(Shape::Tanh(drive))                         // weight/saturation
```

To keep this **allocation-free per trigger** we do *not* rebuild the graph on each hit. We use `fundsp`'s `shared()`/`var()` parameter handles for the per-trigger values and a single retriggerable envelope clock. In practice Phase 0's voice is simple enough that we hand-roll the envelopes as plain state and only use `fundsp` for the oscillator + shaper, which is the most robust way to guarantee no per-trigger allocation:

```rust
// engine/src/voices/drum.rs
use fundsp::hacker::*;

pub struct KickVoice {
    sr: f32,            // sample rate
    // --- pre-built fundsp node (constructed once) ---
    osc: An<...>,       // a sine oscillator node taking a frequency input
    shaper: An<...>,    // tanh waveshaper (built once)
    // --- per-trigger state (just numbers; reset on trigger, no alloc) ---
    active: bool,
    t: u32,             // frames since trigger
    pending_offset: u32,// sub-buffer start offset for this buffer (from drain)
    body_hz: f32,
    click_hz: f32,
    pitch_decay_frames: f32,
    amp_decay_frames: f32,
    amp_attack_frames: f32,
    gain: f32,
    drive: f32,
}

impl KickVoice {
    pub fn new(sample_rate: f32) -> Self {
        let osc = sine();          // built once; allocation happens here, at startup only
        let shaper = shape(Shape::Tanh(1.5));
        Self {
            sr: sample_rate, osc, shaper,
            active: false, t: 0, pending_offset: 0,
            body_hz: 55.0, click_hz: 440.0,
            pitch_decay_frames: 0.03 * sample_rate,
            amp_decay_frames: 0.18 * sample_rate,
            amp_attack_frames: 0.001 * sample_rate,
            gain: 0.9, drive: 1.5,
        }
    }

    /// One-shot trigger from a NoteOn, at a sub-buffer sample offset (02 passes this).
    /// Pure arithmetic + flag flips — NO allocation.
    #[inline]
    pub fn trigger_at(&mut self, offset: usize, freq: f32, vel: f32, dur_frames: f32) {
        self.body_hz = if freq > 0.0 { freq } else { 55.0 };
        self.click_hz = self.body_hz * 8.0;
        self.gain = vel.clamp(0.0, 1.0);
        if dur_frames > 0.0 { self.amp_decay_frames = dur_frames; }
        self.t = 0;
        self.pending_offset = offset as u32;
        self.active = true;
        self.osc.reset();  // resets phase; no allocation
    }

    #[inline] fn pitch_at(&self, t: f32) -> f32 {
        let k = (-t / self.pitch_decay_frames).exp();
        self.body_hz + (self.click_hz - self.body_hz) * k
    }
    #[inline] fn amp_at(&self, t: f32) -> f32 {
        if t < self.amp_attack_frames {
            t / self.amp_attack_frames                       // linear attack
        } else {
            let d = t - self.amp_attack_frames;
            (-d / (self.amp_decay_frames * 0.5)).exp()       // exp decay
        }
    }

    /// One sample of output. 0.0 when idle. Honors the sub-buffer start offset:
    /// emits silence until the trigger frame is reached within this buffer.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if !self.active { return 0.0; }
        if self.pending_offset > 0 { self.pending_offset -= 1; return 0.0; }

        let t = self.t as f32;
        let freq = self.pitch_at(t);
        let raw = self.osc.filter_mono(freq);   // sine at instantaneous freq
        let amp = self.amp_at(t);
        let mut s = raw * amp * self.gain;
        s = (s * self.drive).tanh();            // weight; (or self.shaper.filter_mono(s))

        self.t += 1;
        // End the voice once the amp envelope has decayed to silence.
        if t > self.amp_attack_frames + self.amp_decay_frames * 4.0 {
            self.active = false;
        }
        s
    }
}
```

How this realizes the click→body character:
- At `t = 0`, `pitch_at` returns `click_hz` (≈ `body_hz * 8`), so the very first cycles are a high-pitched snap.
- `pitch_at` decays toward `body_hz` with time-constant `pitch_decay` (~30 ms), so within a few tens of ms it has settled to the fundamental — exactly the 808/909 "drop."
- `amp_at` is a near-instant attack then an exponential decay over `amp_decay` (default ~180 ms, overridable by the event's duration), giving the punch-then-tail.
- `tanh` drive adds harmonics and glues transients, the standard trick for a kick that "hits."

### Why the per-buffer offset matters here

The render loop calls `tick()` once per output frame. Because `trigger_at` stores `pending_offset` (the in-buffer sample index from [02-event-ring-buffer.md](02-event-ring-buffer.md)), the voice emits silence until that exact sample, then begins the click. This makes the kick land on its **sample-accurate** frame, not merely at the start of whichever buffer it was drained in — which is what keeps the four-on-the-floor groove from jittering by up to a buffer (~5 ms) per hit.

### Monophonic, retriggerable

Phase 0 has one kick voice. A new `trigger_at` while a previous hit is still ringing simply resets the state (a "hard retrigger"), which is musically fine for four-on-the-floor where hits don't overlap. Polyphony and `voice_alloc` (oldest/quietest stealing) come in Phase 1.

## Interfaces

- **From [02-event-ring-buffer.md](02-event-ring-buffer.md):** `drain` calls `kick.trigger_at(offset, freq, vel, dur_frames)` for each `NoteOn`, where `freq = arg0`, `vel = arg1`, `dur_frames = arg2` ([event-protocol.md](../../event-protocol.md#opcodes)).
- **To [01-audio-io-cpal.md](01-audio-io-cpal.md):** the render loop calls `kick.tick() -> f32` once per output frame; the voice is owned by `AudioState`.
- **Event protocol:** consumes `NoteOn` (`0x01`). `arg3` (instrument id) is accepted but ignored in Phase 0 (there is only one instrument). The kick needs no `NoteOff` because it is a one-shot with an internal duration.
- **To Phase 1:** this file is the seed of `voices/drum.rs`; the `fundsp`-graph + envelope structure generalizes to snare/hat/clap and to live-settable params.

## Edge cases & risks

- **Per-trigger allocation.** The graph nodes (`sine()`, `shape()`) are built **once** in `new()`; `trigger_at` and `tick()` do only arithmetic and flag flips. *Risk* if a future change rebuilds a `fundsp` graph inside `trigger_at` — explicitly forbidden; covered by review and the stress check.
- **Denormals.** The exponential decays approach but never reach zero; tiny denormal floats can be slow on some CPUs. We terminate the voice (`active = false`) once well past the decay so output is *exactly* `0.0` when idle, avoiding a denormal tail. (A flush-to-zero guard is a Phase-1 hardening item.)
- **Clipping.** Single voice with `gain = velocity ≤ 1.0` and a `tanh` ceiling cannot exceed ~1.0, so Phase 0 needs no master limiter (Phase 1 adds it).
- **Zero/garbage freq.** `trigger_at` falls back to `55 Hz` if `freq <= 0`; `vel` is clamped to `[0,1]`; a non-positive `dur_frames` keeps the default decay.
- **Retrigger click.** A hard retrigger mid-decay can produce a tiny discontinuity; inaudible at kick rates here and acceptable for Phase 0. A 1-2 ms release ramp is a Phase-1 nicety.

## Acceptance criteria

- [ ] **Audible, recognizable kick.** *Audible check:* run `examples/phase0-kick.ts` ([07](07-hot-reload-demo.md)) and confirm it sounds like a techno/hip-hop kick (clear click attack, solid body, clean tail), not a beep.
- [ ] **Pitch envelope present.** *Unit test:* render 200 ms of one hit into a buffer; estimate instantaneous frequency at `t≈0` and `t≈100 ms` (zero-crossing or FFT) and assert it starts near `click_hz` and settles near `body_hz`.
- [ ] **Amp envelope shape.** *Unit test:* render one hit; assert peak occurs within the first few ms, then output decays monotonically (within tolerance) and is exactly `0.0` after the voice ends.
- [ ] **Sub-buffer offset honored.** *Unit test:* `trigger_at(offset=64, …)`; render a 128-frame buffer; assert samples `0..64` are exactly `0.0` and the first nonzero sample is at index `64`.
- [ ] **One-shot, no NoteOff.** *Unit test:* trigger once with `dur_frames = 0.05*sr`; assert `active` becomes `false` on its own after the decay, with no `NoteOff` sent.
- [ ] **Allocation-free per trigger.** *How to test:* code review (no graph rebuild / `Box`/`Vec` in `trigger_at`/`tick`), plus the 60 s stress check ([01](01-audio-io-cpal.md)) showing zero underruns while triggering 8+ hits/sec.
