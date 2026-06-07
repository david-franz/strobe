# strobe — user guide

A practical, conceptual reference to everything you can write today (Phases 0–4). For the
design *behind* it, see [architecture.md](architecture.md) and [pattern-model.md](pattern-model.md).

A song is just TypeScript that assigns patterns to tracks. Run one with hot-reload:

```bash
pnpm demo:phase4          # plays examples/phase4-sampler.ts; edit + save to hear changes
```

```ts
import { setcps, p1, p2, p3, sound, note, sine } from "strobe";

setcps(130 / 60 / 4);                       // ~130 BPM, 4 beats per cycle
p1( sound("bd*4").gain(0.9) );              // kick
p2( sound("~ hh ~ hh").lpf(6000) );         // offbeat hats
p3( note("c2 c2 eb2 g2").saw().lpf(sine.range(300,1200).slow(8)) );  // bass + filter sweep
```

Edit any line and save: the change swaps in at the **next cycle boundary** with no dropout.

## The chapters

| # | Chapter | What's in it |
| --- | --- | --- |
| 1 | [The mental model](guide/mental-model.md) | the one idea (a pattern is a function of time), the vocabulary, tempo |
| 2 | [Mini-notation](guide/mini-notation.md) | the rhythm DSL — sequences, `~`, `*` `/`, `[]`, `<>`, `,`, `(p,s)`, `!`, `?` |
| 3 | [sound / note / freq](guide/sound-note-freq.md) | the constructors + the per-note modifiers (`.gain`, `.adsr`, waveforms, …) |
| 4 | [Combinators](guide/combinators.md) | `fast`/`rev`/`every`/`iter`/`jux`/`off`/`degrade`/… — reshaping time & structure |
| 5 | [Signals](guide/signals.md) | continuous values for sweeps/LFOs (`sine`/`rand`/… `.range().slow()`) |
| 6 | [Effects](guide/effects.md) | the per-track rack + shared sends — filter, EQ, dist, mod, comp, sidechain, reverb/delay |
| 7 | [Sampling](guide/sampling.md) | **Phase 4** — load, chop/slice, stretch/pitch, granular, YouTube ingest |
| 8 | [Tracks & live control](guide/tracks.md) | `p1`…`p8`, mute/solo/hush, the live-coding loop, recipes |

## The one idea

Everything is built from a single concept: **a pattern is a pure function from a span of time
to the events that happen in it.** You don't build a list of notes — you describe a function,
and the engine queries it for whatever slice of time it needs next. That's why patterns compose
freely and hot-swapping never clicks. The [mental model](guide/mental-model.md) chapter unpacks
why the rest of the API looks the way it does.

> **What plays today:** Phases 0–4 — synthesis, the pattern language, the full effects rack, and
> the sampler (chop/slice/stretch/pitch/granular + opt-in YouTube ingest). See
> [roadmap.md](roadmap.md) for what each phase adds.
