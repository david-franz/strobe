# 04 — Chop, slice & granular

## Purpose

Loading and pitching a sample is the raw capability; **chopping it into a pattern** is what makes it hip-hop. This feature is the producer-facing edit layer — `packages/tekno/src/sample/edit.ts` plus the small engine support it needs — that turns one buffer into musical material:

- **`.chop(n)`** — split a buffer into `n` equal slices and lay them out *in order across the cycle*, each slice triggerable as its own event (the classic "chop the break" move).
- **`.slice(n, pattern)`** — split into `n` slices but let a *mini-notation pattern* choose which slice plays in each step (re-sequence the break: `"0 3 1 2"`).
- **`.loop()`** — loop a region (a slice, or the whole buffer) for a held duration (sustained beds, stutters).
- **Granular playback** — synthesize a cloud of short overlapping **grains** (grain size / density / scatter) for textures and a cheap, RT-friendly pseudo-time-stretch alternative to [03](03-timestretch-pitchshift.md).

The unifying idea: **a slice is just a `TrigSample` with a start offset and a region length** ([02-sampler-voice.md](02-sampler-voice.md)). Chop/slice/loop add *no new opcode and no new voice* — they are pure pattern transforms in TS that compute `(start, length)` per `Hap` and ride the existing `TrigSample` (`0x20`) ([event-protocol.md](../../event-protocol.md#opcodes)). They compose with the whole combinator vocabulary ([pattern-model.md](../../pattern-model.md)) because the result is an ordinary `Pattern`. Granular needs a little engine help (a grain scheduler), described below.

## Location

```
packages/tekno/
  src/
    sample/
      edit.ts          # NEW — .chop/.slice/.loop/.grain + .begin/.end/.speed/.rev/.gain
      load.ts          # (01) — sample() returns the SamplePattern these extend
engine/
  src/
    voices/
      sampler.rs       # (02) — region start/length already supported via TrigOpts
      granular.rs      # NEW — grain pool voice (one voice = many short readers)
    scheduler.rs       # routes grain triggers; chop/slice need no engine change
```

This document owns `packages/tekno/src/sample/edit.ts` and `engine/src/voices/granular.rs`. Chop and slice are *entirely* TS; only granular adds engine code.

## Design

### The sample control map

A `sample()` pattern ([01](01-sample-decode-loading.md#ts-side-api-loadts)) carries a control map per `Hap`. The edit methods set fields the scheduler compiles into `TrigSample` args/flags:

```ts
interface SampleControl {
  s: string;            // sample name (for display)
  sampleId: number;     // resolved engine id (arg3)
  begin: number;        // region start, 0..1 fraction of the buffer
  end: number;          // region end,   0..1 fraction of the buffer
  speed: number;        // arg0 (varispeed ratio; default 1.0)
  gain: number;         // arg1
  rev: boolean;         // flags bit
  loop: boolean;        // flags bit
  interp?: "linear" | "cubic";
}
```

`begin`/`end` are stored as **0..1 fractions** in the control plane (resolution-independent); the scheduler converts to frames using the buffer's `frames` at compile time:

```ts
// in the scheduler, compiling a SampleControl Hap → TrigSample
const startFrame  = Math.round(c.begin * buf.frames);
const regionLen   = Math.round((c.end - c.begin) * buf.frames);
const speed       = c.rev ? c.speed : c.speed;     // direction is a flag, not sign of arg0
// emit TrigSample: arg0=speed, arg1=gain, arg2=startFrame, arg3=sampleId,
//                  flags = REV?|LOOP?|INTERP bits, plus packed regionLen
```

### `.chop(n)` — slices laid out across the cycle

`chop(n)` produces a pattern of `n` events filling the cycle, the *k*-th carrying `begin = k/n`, `end = (k+1)/n`. It is a `fastcat` of `n` single-slice copies of the source:

```ts
// packages/tekno/src/sample/edit.ts
export function chop(pat: SamplePattern, n: number): SamplePattern {
  // For each Hap of the source, replace it with n sub-Haps that subdivide its
  // time span AND its [begin,end) region into n consecutive slices.
  return pat.withHaps(hap => {
    const span = hap.part;                     // the Hap's time span
    const { begin = 0, end = 1 } = hap.value;
    const slices = [];
    for (let k = 0; k < n; k++) {
      const t0 = span.begin + (k / n) * (span.end - span.begin);
      const t1 = span.begin + ((k + 1) / n) * (span.end - span.begin);
      const b  = begin + (k / n) * (end - begin);
      const e  = begin + ((k + 1) / n) * (end - begin);
      slices.push(mkHap({ begin: t0, end: t1 }, { ...hap.value, begin: b, end: e }));
    }
    return slices;
  });
}
```

So `sample("amen").chop(8)` plays the break's 8 consecutive eighths across one cycle, in order. Because it is a normal pattern, `.fast(2)`, `.every(4, rev)`, `.jux(rev)` etc. all just work.

### `.slice(n, pattern)` — re-sequence the slices

`slice(n, idxPattern)` divides the buffer into `n` equal regions but takes the *order* from a mini-notation index pattern, so you can rearrange the break:

```ts
export function slice(pat: SamplePattern, n: number, idx: Pattern<number>): SamplePattern {
  // idx is e.g. mini("0 3 1 2 0 3 1 7"); each index k selects region [k/n,(k+1)/n).
  return idx.withValue(k => {
    const b = k / n, e = (k + 1) / n;
    return { ...pat.firstValue(), begin: b, end: e };
  });
}
// sugar: sample("amen").slice(8, "0 3 1 2")
```

`chop(n)` is the special case `slice(n, "0 1 2 … n-1")`. Both map identically onto `TrigSample` start offsets — the only difference is which slice each step selects.

### `.loop()` — hold a region

`loop()` sets the `loop` flag so the sampler voice ([02](02-sampler-voice.md)) wraps the region instead of stopping at its end. Combined with `.begin/.end` it loops a single slice; alone it loops the whole buffer. Useful for stutters (`sample("amen").chop(8).slice(...).loop()`) and sustained beds.

### `.rev()` — already free

`.rev()` is the *existing* `rev` combinator ([pattern-model.md](../../pattern-model.md#time)): it reverses the *order of events in the cycle*. For samples we *also* set the per-slice `rev` flag so each slice plays backwards too — `rev` on a chopped pattern reverses both the sequence and each slice's audio, which is the musically expected "reverse the break." (`.rev()` on the pattern reorders Haps; a `.slicerev()`/the flag handles intra-slice reversal — the default `.rev()` on a `SamplePattern` does both.)

### Worked example: `sample("amen").chop(8).rev()`

```ts
sample("amen")      // 1) load the Amen break → a one-shot SamplePattern (begin=0,end=1)
  .chop(8)          // 2) 8 consecutive slices across the cycle:
                    //    k=0 → begin .000 end .125 at cycle [.000,.125)
                    //    k=1 → begin .125 end .250 at cycle [.125,.250)  … k=7 → [.875,1.0)
  .rev();           // 3) reverse: slice order becomes 7,6,5,4,3,2,1,0 across the cycle
                    //    AND each slice's audio plays backwards (rev flag set)
```

What the scheduler emits per cycle: **8 `TrigSample` events**, one per `Hap`. For the reversed pattern, the `Hap` at cycle `[0,.125)` carries slice 7 (`begin .875,end 1.0`) with the `rev` flag; the engine triggers `arg2 = round(.875*frames)`, `regionLen = round(.125*frames)`, reverse on — the sampler voice ([02](02-sampler-voice.md)) plays that region backwards. No new opcode; eight ordinary `TrigSample`s ([event-protocol.md](../../event-protocol.md#opcodes)).

### Granular playback (`granular.rs` + `.grain(...)`)

Granular synthesis plays a **cloud of short overlapping grains** read from the buffer. It gives textures and a cheap pseudo-time-stretch: by holding the read position roughly fixed while spraying grains, you sustain a moment of the sample (a different aesthetic from the faithful Signalsmith stretch in [03](03-timestretch-pitchshift.md)).

```ts
// packages/tekno/src/sample/edit.ts
interface GrainOpts {
  size: number;       // grain length in ms (e.g. 50)
  density: number;    // grains per second (overlap = size_ms/1000 * density)
  scatter: number;    // 0..1 random jitter of each grain's read position
  pitch?: number;     // optional per-grain semitone shift (varispeed within a grain)
  spread?: number;    // 0..1 stereo pan scatter
}
export function grain(pat: SamplePattern, opts: GrainOpts): SamplePattern;
// sample("pad").grain({ size: 60, density: 30, scatter: 0.1 })
```

Two implementation strategies; we use **engine-side grains** to keep the event ring light:

1. **TS-side grains** — emit one `TrigSample` per grain from the scheduler. Simple, reuses everything, but a 30-grains/sec cloud floods the ring and pressures `voice_alloc` (each grain = a voice). Fine for sparse clouds.
2. **Engine-side grain voice (chosen)** — one `Granular` voice owns a small **pool of grain readers** and schedules grains itself from a few parameters. The scheduler sends *one* trigger that starts/parameterizes the cloud; the voice internally spawns grains (each a tiny fixed-size reader with a Hann window) at the configured density/scatter. No per-grain event, bounded voice usage.

```rust
// engine/src/voices/granular.rs (sketch)
struct Grain { pos: f64, step: f64, age: u32, len: u32, pan: f32, active: bool }

pub struct GranularVoice {
    buf: Option<Arc<SampleBuffer>>,
    grains: [Grain; MAX_GRAINS],   // FIXED-SIZE pool — no alloc on the audio thread
    center: f64,                   // read position the cloud hovers around
    size_frames: u32,
    interval_frames: u32,          // 1/density
    scatter: f32, spread: f32,
    rng: SmallRng,                 // pre-seeded; cheap, no alloc
    next_grain_in: u32,
    gain: f32, active: bool,
}

impl GranularVoice {
    #[inline]
    pub fn tick(&mut self) -> (f32, f32) {
        // 1) maybe spawn a grain when the interval elapses (reuse a dead slot — no alloc)
        // 2) sum all active grains, each Hann-windowed, read with interpolation ([02])
        // 3) advance ages; deactivate finished grains
        // ... all fixed-size arithmetic; RT-safe ...
    }
}
```

The grain reader reuses [02](02-sampler-voice.md)'s interpolation. The pool is a **fixed-size array** (`MAX_GRAINS`) so nothing allocates on the audio thread — the same discipline as every voice ([architecture.md](../../architecture.md#audio-plane--rust-hard-real-time)).

### How slices map to `TrigSample` start offsets (summary)

| Edit | Per-`Hap` control | `TrigSample` mapping |
| --- | --- | --- |
| `.chop(n)` | `begin=k/n, end=(k+1)/n`, in cycle order | `arg2=round(begin*frames)`, `regionLen=round((end-begin)*frames)`, `arg3=sampleId` |
| `.slice(n, idx)` | `begin=idx/n, end=(idx+1)/n`, order from `idx` | same mapping; `idx` chooses the region |
| `.loop()` | `loop=true` | `flags` LOOP bit set → voice wraps region |
| `.rev()` | reorders Haps; sets `rev` per slice | `flags` REV bit set → voice reads region backwards |
| `.grain(...)` | grain params | one trigger to the `GranularVoice` (engine schedules grains) |

## Interfaces

- **Event protocol:** chop/slice/loop produce ordinary **`TrigSample` (`0x20`)** events — `arg0`=speed, `arg1`=gain, `arg2`=start frame, `arg3`=sample id, with reverse/loop/interp + packed region length in `flags` ([event-protocol.md](../../event-protocol.md#opcodes), [02](02-sampler-voice.md#triggering-from-trigsample)). **No new opcode.** Granular uses one `TrigSample` to start a `GranularVoice` (a flag marks it granular and `arg2` is the center position).
- **Sampler voice ([02](02-sampler-voice.md)):** consumes `start` + region length + reverse/loop flags — already specified there; chop/slice add nothing to the voice.
- **Pattern engine ([pattern-model.md](../../pattern-model.md)):** `chop`/`slice` are `Pattern → Pattern` transforms returning normal patterns, so every combinator composes — `sample("amen").chop(8).rev().jux(x => x.speed(1.5)).every(4, fast(2))` all works. `slice`'s index argument is itself a mini-notation pattern, so re-sequences are first-class.
- **Stretch/pitch ([03](03-timestretch-pitchshift.md)):** `.stretch()/.pitch()` rebind `sampleId` to a prepared buffer; chop/slice over the result address the prepared timeline. Order is *stretch then chop* (slice boundaries land on the final length).

## Edge cases & risks

- **Slice-boundary clicks.** Slices rarely start/end at zero crossings → rely on the sampler voice's ~2 ms boundary fades ([02](02-sampler-voice.md#boundary-fades-de-click)). For very short slices (`chop(64)`) the fade is shortened proportionally so it never exceeds the slice.
- **Off-by-one region length.** `round(begin*frames)`/`round(end*frames)` can leave a 1-frame gap/overlap between adjacent chop slices; compute `end` of slice `k` as `begin` of slice `k+1` (shared boundary) so the break reconstructs sample-exact when chopped and replayed in order.
- **Reverse semantics.** `.rev()` on a `SamplePattern` reverses both Hap order *and* intra-slice audio; documented clearly so it matches producer expectation. (Pure event-order reversal without intra-slice flip is available as a plain `rev` on a non-sample pattern.)
- **Granular real-time safety.** The grain pool is a **fixed-size array**; grain spawn reuses dead slots; the RNG is pre-seeded and allocation-free. No `Vec` growth, no lock on the audio thread. Reviewed + stress-checked. Excessive `density` is clamped to `MAX_GRAINS` (a friendly cap, not an underrun).
- **Voice-pool pressure (TS-side grains).** The chosen engine-side grain voice avoids flooding the ring / `voice_alloc`; the TS-side fallback is only for sparse clouds and is documented as such.
- **Resolution independence.** `begin/end` are fractions, converted to frames against the *resolved* buffer (post-stretch). A still-loading/pending sample emits no triggers until ready ([01](01-sample-decode-loading.md#interfaces)) — chop of a pending sample simply stays silent, no error.

## Acceptance criteria

- [ ] **Chop lays out slices in order.** *Unit test (query a span):* `queryArc(sample("amen").chop(8), 0, 1)` returns 8 Haps with `begin/end` = `k/8 … (k+1)/8` at cycle positions `k/8`, in order.
- [ ] **`sample("amen").chop(8).rev()` (roadmap "done when").** *Unit test:* the 8 Haps appear in reverse cycle order (slice 7 first) and each carries the `rev` flag; *audible check:* the chopped break plays reversed and reconstructs the reversed break cleanly (no clicks at slice boundaries).
- [ ] **Slice re-sequences.** *Unit test:* `sample("amen").slice(8, "0 3 1 2")` selects regions 0,3,1,2 in those cycle steps; `chop(n)` equals `slice(n, "0..n-1")`.
- [ ] **Loop wraps a region.** *Integration test:* `sample("x").chop(8).slice(...).loop()` of one slice produces continuous looped audio across the wrap (no discontinuity above the de-click threshold) — relying on [02](02-sampler-voice.md)'s loop path.
- [ ] **Chop reconstructs sample-exact.** *Unit test:* chopping into `n` and replaying in order (no rev) yields adjacent regions with no gap/overlap (shared-boundary frame math).
- [ ] **Granular is RT-safe.** *How to test:* review that `GranularVoice` uses a fixed-size grain pool, no alloc / no lock / pre-seeded RNG; 60 s stress check with a dense grain cloud shows **zero underruns**; over-`density` is clamped, not glitched.
- [ ] **Composes with combinators.** *Unit test:* `sample("amen").chop(8).every(4, rev).jux(x=>x.speed(1.5))` queries to the expected Hap set — confirming chop/slice are ordinary patterns.
