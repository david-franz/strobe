# 03 — Example songs

## Purpose

The examples are how a person *learns the instrument*. Each is a complete, runnable `.ts` song that exercises a real slice of the API end-to-end, annotated so the reader can see exactly which part of the language does what. They double as the [cross-cutting "runnable example"](../../roadmap.md#cross-cutting-definition-of-done-every-phase) for Phase 5 and as living regression tests: each must play underrun-free under `strobe play` and hot-reload seamlessly.

Four songs, increasing in depth, covering the project's two signature genres and its two signature techniques (the techno sidechain pump and the hip-hop chopped sample):

1. **`hello-kick.ts`** — the getting-started "hello world": a steady four-on-the-floor kick.
2. **`techno-sidechain.ts`** — a techno loop with a bassline ducked by the kick (the pump).
3. **`hiphop-chop.ts`** — a hip-hop beat with a chopped sample (heart of hip-hop production).
4. **`filter-pad.ts`** — an evolving filter-sweep pad (continuous-signal automation over many cycles).

Every song uses **only** the canonical API from [pattern-model.md](../../pattern-model.md) and matches the [README quickstart](../../../README.md) exactly — same imports, same `setcps`, same `p1..pN`, same chain methods. Nothing here introduces new API.

## Location

```
examples/
  hello-kick.ts          # NEW — getting-started "hello world"
  techno-sidechain.ts    # NEW — techno loop with sidechain pump
  hiphop-chop.ts         # NEW — hip-hop beat with a chopped sample (Phase 4 sampling)
  filter-pad.ts          # NEW — evolving filter-sweep pad
```

These join the existing [`examples/phase0-kick.ts`](../phase-0/07-hot-reload-demo.md#the-watched-song-file). Each runs with `strobe play examples/<name>.ts` ([02-cli.md](02-cli.md)).

## Design

### `hello-kick.ts` — the first song

The smallest possible *real* song, intentionally one track. It teaches: imports, `setcps`, `p1`, `sound`, mini-notation `bd*4`, and `.gain`.

```ts
// examples/hello-kick.ts
//
// Your first strobe song. Run it with:
//   strobe play examples/hello-kick.ts
// Then edit a line, save, and hear it change at the next cycle boundary.

import { setcps, p1, sound } from "strobe";

setcps(130 / 60 / 4);          // ~130 BPM, 4 beats per cycle (see architecture.md)

p1( sound("bd*4").gain(0.9) ); // four-on-the-floor kick — four evenly-spaced hits per cycle

// Try these edits (save after each):
//   sound("bd*8")             // double-time
//   sound("bd(3,8)")          // a Euclidean groove
//   sound("bd ~ bd bd")       // syncopation with a rest (~)
```

### `techno-sidechain.ts` — the pump

The README's signature example, fully fleshed out as three tracks: kick, offbeat hats, and a rolling bassline that is **sidechained to the kick** (`.sidechain("p1")`) and swept by a slow sine — the techno "pump" from [architecture.md](../../architecture.md#the-rust-engine-internals) and [Phase 3](../phase-3/00-overview.md). Teaches: multiple tracks, `note` + `.saw()`, `.lpf` with a continuous signal, `.room`, `.sidechain`.

```ts
// examples/techno-sidechain.ts
//
// A techno loop that "pumps": the bass ducks every time the kick hits.
// Edit any track and save — only the track you changed swaps, at the next bar.

import { setcps, p1, p2, p3, sound, note, sine } from "strobe";

setcps(130 / 60 / 4);                          // ~130 BPM

// p1 — four-on-the-floor kick. The sidechain source for the pump.
p1( sound("bd*4").gain(0.95) );

// p2 — offbeat hi-hats, slightly opened with a low-pass.
p2( sound("~ hh ~ hh").gain(0.6).lpf(6000) );

// p3 — rolling saw bassline, filter-swept over 8 cycles, reverb'd, and
//      ducked by p1 (the kick) for the classic techno pump.
p3( note("c2 c2 eb2 g2")
      .saw()
      .lpf( sine.range(300, 1200).slow(8) )    // cutoff sweeps 300→1200 over 8 cycles
      .resonance(0.3)
      .room(0.3)
      .sidechain("p1") );                       // duck whenever p1 fires

// Try:
//   p2( sound("hh*8").gain(0.5).degrade )      // busier, thinned-out hats
//   p3(... .every(4, rev) ...)                 // reverse the bass every 4th cycle
```

### `hiphop-chop.ts` — the chopped sample

A boom-bap beat built around a **chopped sample** — the core hip-hop move from [Phase 4](../phase-4/00-overview.md): load a break, `.chop(n)` it into slices, and pattern those slices across the cycle. Teaches: `sample()`, `.chop`, `.slice`/`.rev`, swung drums, and the optional YouTube ingest (clearly flagged opt-in per the [roadmap risk note](../../roadmap.md#risks)).

```ts
// examples/hiphop-chop.ts
//
// A hip-hop beat with a chopped break. The sample is chopped into 8 slices and
// re-sequenced; the drums sit under it with a bit of swing.
//
// NOTE on sampling: by default this loads a LOCAL file you provide. The optional
// fromYouTube path is OPT-IN and subject to copyright/ToS — see the legal note in
// docs/guide/sampling.md and roadmap.md#risks. Local files are the safe path.

import { setcps, p1, p2, p3, sound, sample } from "strobe";

setcps(90 / 60 / 4);                            // ~90 BPM, classic boom-bap tempo

// A break, chopped into 8 even slices, then re-sequenced across the cycle.
// Provide your own short loop at examples/assets/break.wav (a few seconds).
const break8 = sample("examples/assets/break.wav").chop(8);

// Optional, OPT-IN alternative (requires yt-dlp + ffmpeg installed; see sampling.md):
//   const break8 = sample.fromYouTube(URL, { start: 12, dur: 4 }).chop(8);

// p1 — the chopped break, played slice-by-slice, with every 4th cycle reversed.
p1( break8("0 1 2 3 4 5 6 7")                   // play slices in order…
      .every(4, rev)                            // …reversed on every 4th pass
      .gain(0.8) );

// p2 — kick + snare backbone with a touch of swing.
p2( sound("bd ~ ~ bd ~ ~ bd ~, ~ ~ sn ~ ~ ~ sn ~").gain(0.9) );

// p3 — closed hats, swung, occasionally dropped for groove.
p3( sound("hh*8").gain(0.5).sometimesBy(0.2, x => x.gain(0)) );

// Try:
//   p1( break8("0 1 [2 3] 4 5 6 7").chop(8) )  // ratchet a slice
//   p1(... .slice(8, "7 6 5 4 3 2 1 0") ...)   // reverse-ordered slices
```

> `sample("path").chop(8)` returns a slice-playable pattern factory: calling it with a mini-notation of slice indices sequences those slices (the [Phase 4 `.chop`/`.slice` API](../phase-4/00-overview.md)). The drums use the synthesized `bd/sn/hh` voices from [Phase 1](../phase-1/00-overview.md).

### `filter-pad.ts` — the evolving sweep

A single sustained pad whose filter cutoff, resonance, and stereo position all move under **continuous signals** over long timescales — showing that "parameters can themselves be patterns" ([pattern-model.md](../../pattern-model.md#parameters-can-themselves-be-patterns)) is where movement comes from. Teaches: `note` chords, `.adsr`, layered continuous modulation (`sine`/`saw`/`perlin`), `.slow`, `.room`.

```ts
// examples/filter-pad.ts
//
// An evolving pad: one chord, but the filter sweeps, the resonance breathes,
// and the stereo image drifts — all driven by slow continuous signals.
// Let it run for a minute; it never quite repeats.

import { setcps, p1, note, sine, saw, perlin } from "strobe";

setcps(120 / 60 / 4);                           // ~120 BPM

p1( note("<c3 eb3 g3 bb3>")                     // a chord, one note per cycle, cycling
      .saw()
      .adsr(0.6, 0.3, 0.7, 1.2)                 // slow attack/release → pad
      .lpf( sine.range(200, 2000).slow(16) )    // cutoff sweeps over 16 cycles
      .resonance( perlin.range(0.1, 0.5).slow(8) ) // organic resonance wobble
      .pan( saw.range(0, 1).slow(12) )          // slow auto-pan across the stereo field
      .room(0.6)                                // generous reverb
      .gain(0.5) );

// Try:
//   note("[c3,eb3,g3]")                         // play the whole chord at once (stack)
//   .lpf( sine.range(200, 2000).slow(16).add(saw.range(0,300).slow(2)) ) // layered motion
```

### Shared conventions

- Every file opens with a comment block: what it is, how to run it (`strobe play examples/<name>.ts`), and 2–3 "Try this" edits — so the file teaches the hot-reload workflow itself.
- Imports are explicit named imports from `"strobe"`, matching the [README](../../../README.md) exactly. No wildcard imports, no setup boilerplate.
- Tempos and note ranges are genre-appropriate (130 BPM techno, 90 BPM boom-bap, 120 BPM pad).
- `hiphop-chop.ts` references a local `examples/assets/break.wav` the repo ships (a short, clearly-licensed/royalty-free loop), so the example runs out of the box without any network or YouTube step.

## Interfaces

- **To the [CLI](02-cli.md):** each song is the argument to `strobe play examples/<name>.ts`; `hello-kick.ts` is the one the getting-started doc walks through.
- **To the [hot-reload runtime](01-hot-reload-runtime.md):** the multi-track songs (`techno-sidechain`, `hiphop-chop`) are the fixtures for the "only changed tracks swap" acceptance test — editing one `pN` line must swap only that track.
- **To [Phase 1–4](../../roadmap.md):** they exercise synth/drum voices (1), the parser + combinators + chain API + continuous signals (2), the effects rack incl. sidechain (3), and the sampler `.chop`/`.slice` (4). They are the de-facto integration test that those phases compose correctly.
- **To the [docs](07-getting-started-docs.md):** the getting-started guide builds `hello-kick.ts` line by line; the combinator and chain-API references cite snippets from `techno-sidechain.ts` and `filter-pad.ts`.

## Edge cases & risks

- **Missing sample asset.** If `examples/assets/break.wav` is absent (or a user points `hiphop-chop.ts` at a path that does not exist), `sample()` reports a clear "sample not found" error on load; under `strobe play` this is a load error surfaced by the CLI, and under a *live edit* it is caught and the last-good pattern keeps playing ([01](01-hot-reload-runtime.md#edge-cases--risks)). The drums (synth voices) still play regardless.
- **YouTube path is opt-in only.** The `fromYouTube` line is commented out and flagged; it requires `yt-dlp`/`ffmpeg` ([04](04-packaging-prebuilts.md#optional-external-clis-yt-dlpffmpeg)) and carries the copyright/ToS caveat ([roadmap.md](../../roadmap.md#risks)). No example *requires* it; local files are the default and the safe path.
- **Tempo/gain sanity.** Gains are kept under 1.0 and the master limiter ([Phase 1](../phase-1/00-overview.md)) guards clipping; examples are checked to not trip the limiter under normal playback.
- **Determinism for tests.** Examples that use chance combinators (`degrade`, `sometimesBy`) are still underrun-safe; the regression test asserts *playability and clean stats*, not exact audio, for those — deterministic examples (`hello-kick`, the pad's signal automation) can additionally be golden-checked via `strobe render` ([02](02-cli.md#render--offline-bounce-in-clirenderts)).
- **Cross-platform paths.** Songs use forward-slash relative paths; the runtime resolves them against the cwd so they work on Windows too.

## Acceptance criteria

- [ ] **All four play underrun-free.** Each `strobe play examples/<name>.ts` runs for ≥60 s with `stats()` reporting zero underruns and zero late events. *How to test:* the CI playback harness runs each headless on each OS and asserts clean stats.
- [ ] **Only-changed-track hot-reload.** Editing one `pN` line in `techno-sidechain.ts` swaps only that track at the next boundary with no glitch. *How to test:* the [runtime harness](01-hot-reload-runtime.md#acceptance-criteria) drives the edit and asserts the `SwapPlan`.
- [ ] **API fidelity.** Every method/function used appears in [pattern-model.md](../../pattern-model.md) (or the [Phase 4 sampling API](../phase-4/00-overview.md)); no example invents API. *How to test:* a lint/grep check cross-referencing example symbols against the exported surface.
- [ ] **Runs out of the box.** `hello-kick`, `techno-sidechain`, and `filter-pad` need no assets and no external CLIs; `hiphop-chop` runs against the shipped `examples/assets/break.wav` with no network. *How to test:* fresh-checkout CI run with `yt-dlp`/`ffmpeg` absent.
- [ ] **Render reproducibility.** Deterministic examples bounce identically via `strobe render`. *How to test:* compare two renders of `filter-pad.ts` for bit-equality (no chance combinators in it).
- [ ] **Cross-platform.** All of the above pass on macOS, Linux, and Windows in CI.
