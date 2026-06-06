# 03 — Time-stretch & pitch-shift (decoupled)

## Purpose

The sampler voice ([02-sampler-voice.md](02-sampler-voice.md)) does *varispeed*: changing playback speed also changes pitch (faster = higher, like a record sped up). That is cheap and artifact-free, but it is not what hip-hop and techno production needs most of the time. The producer's two essential, **decoupled** operations are:

- **Time-stretch** — change a sample's *duration/tempo* **without changing its pitch** (fit an Amen break to your BPM; slow a vocal without chipmunking it).
- **Pitch-shift** — change a sample's *pitch* **without changing its duration** (drop a bass hit two semitones; harmonize a chop).

This feature provides both, **decoupled**, via `ssstretch` / Signalsmith Stretch ([architecture.md](../../architecture.md#why-these-technology-choices)) — a high-quality phase-vocoder-style stretcher. Crucially it is kept behind a **swappable Rust `StretchEngine` trait** so the algorithm can change without touching song code or the voice — the explicit risk-mitigation called out in [roadmap.md#risks](../../roadmap.md#risks) ("kept behind a swappable interface so we can change algorithms without touching song code").

The work is done **offline, at load/prepare time, off the audio thread**. Time-stretch is expensive and stateful — exactly the kind of thing that must never run in the audio callback ([architecture.md](../../architecture.md#the-core-principle-two-planes)). `.stretch(x)` / `.pitch(s)` produce a *new prepared buffer* in the sample bank ([01](01-sample-decode-loading.md)); the voice then plays that buffer at `step = 1.0`, so playback stays the trivial, real-time-safe read loop of [02](02-sampler-voice.md).

## Location

```
engine/
  Cargo.toml             # + ssstretch (Signalsmith Stretch bindings)
  src/
    sample/
      mod.rs             # + pub mod stretch;
      stretch.rs         # NEW — StretchEngine trait + SignalsmithStretch impl
      bank.rs            # (01) — prepared buffers inserted here
bindings/native/
  src/lib.rs             # + napi: prepare_stretched(sampleId, ratio, semitones) -> u32
packages/tekno/
  src/
    sample/
      edit.ts            # (04) — `.stretch(x)` / `.pitch(s)` chainable methods live here
```

This document owns `engine/src/sample/stretch.rs` (the trait + the Signalsmith impl) and the `prepare_stretched` control call; the TS chainable surface is shared with [04-chop-slice-granular.md](04-chop-slice-granular.md) (both are methods on the sample pattern).

## Design

### The swappable trait

The whole point is that *only this file* knows it is Signalsmith. Everything else talks to the trait.

```rust
// engine/src/sample/stretch.rs

/// Parameters for one offline stretch/pitch render.
pub struct StretchSpec {
    /// Time ratio: output_len / input_len. 2.0 = twice as long (half tempo),
    /// 0.5 = half as long (double tempo). Pitch is UNCHANGED by this.
    pub time_ratio: f64,
    /// Pitch shift in semitones. +12 = one octave up, duration UNCHANGED.
    pub semitones: f64,
    pub channels: u16,
    pub sample_rate: u32,
}

/// A swappable time-stretch + pitch-shift backend. DECOUPLED: time_ratio and
/// semitones are independent. Runs OFFLINE (non-real-time) only.
pub trait StretchEngine: Send {
    /// Render `input` (planar, per-channel) into a new planar buffer under `spec`.
    /// Allocates and is slow — caller guarantees this is off the audio thread.
    fn process_offline(&mut self, input: &[Vec<f32>], spec: &StretchSpec) -> Vec<Vec<f32>>;

    /// Human-readable id for logging / golden tests.
    fn name(&self) -> &'static str;
}
```

### The Signalsmith implementation

```toml
# engine/Cargo.toml (excerpt)
ssstretch = "*"   # Signalsmith Stretch Rust bindings
```

```rust
// engine/src/sample/stretch.rs (cont.)
use ssstretch::Stretch;

pub struct SignalsmithStretch;

impl StretchEngine for SignalsmithStretch {
    fn name(&self) -> &'static str { "signalsmith" }

    fn process_offline(&mut self, input: &[Vec<f32>], spec: &StretchSpec) -> Vec<Vec<f32>> {
        let channels = input.len();
        let in_frames = input.first().map_or(0, |c| c.len());
        let out_frames = ((in_frames as f64) * spec.time_ratio).round() as usize;

        // Configure Signalsmith: it stretches time and shifts pitch INDEPENDENTLY.
        let mut stretch = Stretch::preset_default(channels, spec.sample_rate);
        stretch.set_transpose_semitones(spec.semitones as f32, /*tonality_limit*/ None);

        // Offline (non-realtime) processing: feed the whole input, read the whole
        // output. Signalsmith handles internal latency; we trim its priming.
        let mut output: Vec<Vec<f32>> = vec![vec![0.0; out_frames]; channels];
        stretch.process(input, in_frames, &mut output, out_frames);

        output
    }
}
```

> Signalsmith Stretch is a single algorithm that does **both** time-stretch and pitch-shift independently in one pass: `time_ratio` is realized by the output length you ask for, `semitones` by `set_transpose_semitones`. That is precisely the decoupling we need — pure stretch is `semitones = 0`, pure pitch is `time_ratio = 1.0`, and you can do both at once.

### Offline prepare → a new bank buffer

`prepare_stretched` runs on a worker thread, calls the engine, and inserts the result as a **new sample id** ([01](01-sample-decode-loading.md#the-engine-side-sample-bank-bankrs)). The original buffer is untouched (still triggerable), and the stretched one plays at `step = 1.0` on the unchanged voice.

```rust
// bindings/native/src/lib.rs (excerpt, runs on a worker thread)
fn prepare_stretched(bank: &SampleBank, engine: &mut dyn StretchEngine,
                     src_id: u32, time_ratio: f64, semitones: f64) -> Option<u32> {
    let src = bank.lookup(src_id)?;                       // Arc<SampleBuffer>
    // De-interleave/borrow planar channels from the (planar) source buffer.
    let planar: Vec<Vec<f32>> =
        (0..src.channels as usize).map(|c| src.channel(c).to_vec()).collect();
    let spec = StretchSpec { time_ratio, semitones,
                             channels: src.channels, sample_rate: src.sample_rate };
    let out = engine.process_offline(&planar, &spec);     // SLOW, off audio thread
    // Re-pack planar → SampleBuffer and insert as a NEW id.
    let frames = out.first().map_or(0, |c| c.len());
    let mut data = Vec::with_capacity(frames * out.len());
    for ch in &out { data.extend_from_slice(ch); }        // planar layout
    Some(bank.insert(Arc::new(SampleBuffer {
        channels: src.channels, sample_rate: src.sample_rate, frames, data: data.into(),
    })))
}
```

### Caching prepared renders

Stretching is expensive; live coding re-runs lines constantly. Each `(src_id, time_ratio, semitones)` render is **memoized** to a prepared sample id (a map in the control plane keyed by that triple), so re-evaluating `sample("x").stretch(2)` reuses the prior render rather than re-stretching. Cache entries are evictable under the same memory budget as raw samples ([01](01-sample-decode-loading.md#memory-management--eviction)).

### Offline vs real-time (and why offline wins here)

| Approach | When | Pros | Cons |
| --- | --- | --- | --- |
| **Offline prepare (chosen default)** | at `.stretch()/.pitch()` time, worker thread | RT-safe (audio thread plays a plain buffer at `step=1`); highest quality; fits the trait cleanly; memoizable | uses memory (a new buffer); a one-time latency hit on first prepare; the ratio is fixed once rendered |
| **Real-time stretch** (per-voice Signalsmith instance, streamed) | could run inside the voice | ratio modulatable per-cycle without re-render | **violates the no-alloc/no-block rule** unless very carefully pre-allocated; higher CPU; far harder to keep glitch-free; out of Phase 4 scope |
| **Granular pseudo-stretch** | [04](04-chop-slice-granular.md) | cheap, RT-friendly, "texture" character | not transparent; artifacts; a different aesthetic, not a faithful stretch |

Phase 4 ships **offline prepare** as the faithful stretch, plus **granular** ([04](04-chop-slice-granular.md)) as the cheap textural alternative. Real-time streaming stretch is explicitly deferred; the trait leaves room for it later (a future `process_streaming` method) without changing song code.

### Quality / artifact tradeoffs

- **Phase-vocoder artifacts.** Large time-stretch (e.g. 4×) on transient-rich material (drums) smears transients ("phasiness"). Signalsmith's transient handling mitigates this; it is still the hardest case ([roadmap.md#risks](../../roadmap.md#risks)).
- **Pitch vs formants.** Big pitch shifts move formants (the "chipmunk"/"monster" coloration). Acceptable for chops; documented.
- **Tonality limit.** Signalsmith exposes a tonality limit to keep high frequencies from shifting unnaturally — surfaced as an advanced option, sensible default.
- **Stretch then chop, not chop then stretch.** Prepare the stretched buffer first, then `.chop(n)` over it, so slice boundaries land on the final timeline. The edit chain ([04](04-chop-slice-granular.md)) orders these correctly.

## Interfaces

- **Control call (new, non-real-time):** `prepareStretched(sampleId: number, timeRatio: number, semitones: number): Promise<number>` → a **new sample id** for the prepared buffer. Sits on the same control-call surface as `loadSample` ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)); never touches the audio thread.
- **Event protocol:** none of its own. It produces a sample id played by the ordinary `TrigSample` (`0x20`) at `arg0` (speed) = `1.0` ([event-protocol.md](../../event-protocol.md#opcodes)). The decoupling is *baked into the buffer*, so the voice stays trivial.
- **TS sample API (in `edit.ts`, [04](04-chop-slice-granular.md)):**

  ```ts
  // Change duration/tempo, keep pitch. x>1 = longer/slower, x<1 = shorter/faster.
  sample("amen").stretch(2);          // half-tempo, same pitch
  // Change pitch, keep duration. semitones may be fractional.
  sample("vox").pitch(-3);            // down a minor third, same length
  // Both at once (decoupled):
  sample("loop").stretch(1.5).pitch(7);
  ```

  Each method (a) ensures the source is loaded ([01](01-sample-decode-loading.md)), (b) calls `prepareStretched` (memoized), (c) rebinds the pattern's `sampleId` to the prepared id. `stretch` accepts a number or a pattern; a *patterned* ratio renders the distinct values it takes (each memoized) and switches `sampleId` per `Hap` — not continuous modulation (that would need real-time stretch).
- **Pattern engine:** because `.stretch()/.pitch()` only swap the `sampleId` carried in the control map, they compose with every combinator ([pattern-model.md](../../pattern-model.md)) — `.rev()`, `.every()`, `.chop()` all work on the stretched buffer with no special handling.

## Edge cases & risks

- **Real-time safety (headline).** `process_offline` allocates and runs for many ms — it runs **only** inside `prepareStretched` on a worker thread. The audio thread plays the result at `step = 1.0` via the unchanged [02](02-sampler-voice.md) loop. Reviewed; no stretch code is reachable from `tick`.
- **Algorithm swappability.** The `StretchEngine` trait is the contract; `SignalsmithStretch` is one impl. A `NullStretch` (varispeed-only fallback) and a future alternative can drop in without touching the voice, the protocol, or song code — the [roadmap.md#risks](../../roadmap.md#risks) requirement.
- **Stretch quality** is the hardest DSP problem in the project. Acceptance is "artifacts in the range expected of Signalsmith," not "transparent" — large stretches of drums will smear. Granular ([04](04-chop-slice-granular.md)) is the alternative aesthetic.
- **Memory.** Each prepared render is a new buffer; the memoization cache and the shared budget/eviction ([01](01-sample-decode-loading.md#memory-management--eviction)) bound it. Distinct patterned ratios multiply renders — documented, and bounded by the cache cap.
- **Latency on first prepare.** The first `.stretch()` of a fresh ratio incurs the render time before the pattern can trigger that buffer; the scheduler treats the id as "pending" and simply doesn't trigger until ready (same pending behavior as a still-loading sample, [01](01-sample-decode-loading.md#interfaces)) — no scheduler block, no audio glitch.
- **Priming/latency trim.** Signalsmith has internal latency; the offline path feeds/flushes the whole signal and trims priming so the prepared buffer is sample-aligned to the source's start.

## Acceptance criteria

- [ ] **Time-stretch holds pitch.** *Golden/null test:* `stretch(2.0)` of a tonal sample yields ~2× the frames and a **dominant frequency within tolerance of the original** (FFT) — duration changed, pitch held. (This is the roadmap "done when": *time-stretch holds pitch while changing tempo with acceptable artifacts*.)
- [ ] **Pitch-shift holds duration.** *Golden/null test:* `pitch(12)` yields ~the same frame count and a dominant frequency ~2× the original; `pitch(-12)` → ~½×.
- [ ] **Decoupled together.** *Test:* `stretch(1.5).pitch(7)` yields ~1.5× frames **and** a +7-semitone dominant frequency — the two axes are independent.
- [ ] **Trait swap.** *Unit test:* swapping the bound `StretchEngine` for a stub (`NullStretch`) changes output but **no song code and no protocol** changes; `engine.name()` reflects the active impl.
- [ ] **Offline only / RT-safe.** *How to test:* review that no stretch call is reachable from the audio thread; the 60 s stress check prepares stretches on the worker thread while playing, with **zero underruns**.
- [ ] **Memoization.** *Unit test (TS):* repeating `sample("x").stretch(2)` calls `prepareStretched` once and reuses the prepared id.
- [ ] **Pending-then-plays.** *Integration test:* a pattern referencing a not-yet-prepared stretch triggers nothing until the render completes, then plays — with no scheduler stall and no glitch.
