# Phase 2 — Pattern language (overview)

## Purpose

Phase 2 builds **the full declarative language from [pattern-model.md](../../pattern-model.md)** — the heart of everything a user types. Phase 0 proved the spine (TS drives a Rust engine, live, with no glitches; see [phase-0/00-overview.md](../phase-0/00-overview.md)) and Phase 1 made the engine musical (real synth and drum voices). Phase 2 makes the *control plane* expressive: it implements `Pattern<T>`, the mini-notation parser, the full combinator vocabulary, the `sound()/note()/freq()` chaining API, continuous signals, and the scheduler upgrade that queries arbitrary patterns across every track and compiles each `Hap` into the 32-byte protocol of [event-protocol.md](../../event-protocol.md).

The one idea is fixed and **must not be redefined** (see [pattern-model.md](../../pattern-model.md#the-one-idea)):

> **A pattern is a pure function from a span of time to the events that happen in it.**

```ts
type Pattern<T> = (span: TimeSpan) => Hap<T>[];
```

These Phase 2 docs are the **implementation plan** for exactly that model. They specify *how to build* `Pattern`/`Hap`/`TimeSpan`, the mini-notation table, and the combinator vocabulary already defined in [pattern-model.md](../../pattern-model.md). They never restate or alter the semantics — `Hap {whole, part, value}`, `TimeSpan {begin, end}` in cycles, discrete-vs-continuous (`whole: undefined`), `queryArc` testing — those are canonical. Where this plan adds detail (algorithms, RNG seeding, control-map key routing) it stays 100% consistent with the canonical docs and with the LOCKED stack: Rust DSP engine, TypeScript control language, Tidal/Strudel-style pattern combinators.

Everything here is **pure TypeScript in `packages/tekno/src/`**. No DSP, no Rust. The only place Phase 2 touches the engine is the scheduler ([06](06-scheduler-integration.md)), which extends the Phase 0 look-ahead scheduler ([phase-0/05-lookahead-scheduler.md](../phase-0/05-lookahead-scheduler.md)) to query real patterns and batch-encode events through the Phase 0 `Engine.pushEvents` bridge.

## The features

| # | File | One-liner |
| --- | --- | --- |
| 01 | [01-pattern-core.md](01-pattern-core.md) | `pattern.ts`: the `Pattern<T>` representation, `TimeSpan`/`Hap` types, `queryArc`, the fundamental constructors (`pure`/`atom`, `silence`, `signal`), cycle splitting, and the discrete-vs-continuous distinction. The foundation every other file builds on. |
| 02 | [02-mini-notation-parser.md](02-mini-notation-parser.md) | `mini.ts`: tokenizer + recursive-descent parser for the full mini-notation (sequences, `~`, `*`/`/`, `[]`, `<>`, `,`, Euclid `(n,k)`, `!`, `?`), producing a `Pattern` via the core constructors. Grammar, AST, error reporting. |
| 03 | [03-combinators.md](03-combinators.md) | `combinators.ts`: the full vocabulary — time (`fast`/`slow`/`rev`/`hurry`/`off`/`early`/`late`), structure (`every`/`whenmod`/`chunk`/`iter`), layering (`stack`/`cat`/`fastcat`/`superimpose`/`jux`), chance (`degrade`/`sometimes`/`somecycles`). Standalone fns + chainable methods. Deterministic cycle-seeded RNG. |
| 04 | [04-sound-note-api.md](04-sound-note-api.md) | `sound.ts`/`instrument.ts`: `sound()`, `note()` (name + number → frequency), `freq()`; the chainable modifier API (`.gain`, `.lpf`, `.adsr`, `.room`, `.pan`, …) that attaches a control map to each `Hap`'s value; mapping control keys → engine params. |
| 05 | [05-continuous-signals.md](05-continuous-signals.md) | Continuous signals `sine`/`cosine`/`saw`/`isaw`/`tri`/`square`/`rand`/`perlin` as continuous patterns (`whole: undefined`), `0..1` with `.range(lo,hi)`/`.slow`/`.fast`; sampling them as parameter values; note-rate sampling in TS vs handing a `ParamRamp` to the engine. |
| 06 | [06-scheduler-integration.md](06-scheduler-integration.md) | Upgrade the Phase 0 scheduler: query arbitrary patterns across all tracks, compile each `Hap` → engine events (`NoteOn`/`TrigSample` + `ParamSet`/`TrackFx`), allocate voice handles, compute frames, batch-encode 32-byte records. The `Hap`→events function and packed-vs-separate split. |
| 07 | [07-tracks-hot-swap.md](07-tracks-hot-swap.md) | Named tracks `p1..pN`: the mutable slot API, swap-at-next-cycle-boundary semantics, `hush()`/`solo`/`mute`, and why pattern purity makes hot-swap seamless. The track registry and swap timing. |

## Dependency on Phase 0

Phase 2 sits entirely above the Phase 0 bridge and reuses, unchanged:

- **The `Engine` interface** (`start`/`stop`/`pushEvents`/`stats`; see [event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)). Phase 2 emits more opcodes (`NoteOn`, `TrigSample`, `ParamSet`, `ParamRamp`, `TrackFx`) but the 32-byte record layout is the one fixed in Phase 0.
- **The look-ahead scheduler skeleton** (~25 ms tick, query `[now, now+lookahead]`, cycle→seconds→frames, encode via `DataView`, one `pushEvents` per tick). [06](06-scheduler-integration.md) replaces the trivial Phase 0 "play `bd` every 1/4 cycle" query with `queryArc` over real patterns and many tracks.
- **The cps clock and the single track slot** (`setcps`, `bpm = cps*4*60`, cycle↔frame mapping). [07](07-tracks-hot-swap.md) generalizes the one Phase 0 slot to the `p1..pN` registry.

The `Pattern`/`Hap`/`TimeSpan` *shape* introduced minimally in Phase 0 ([phase-0/06-clock-and-tracks.md](../phase-0/06-clock-and-tracks.md)) is the same shape [01](01-pattern-core.md) fully fleshes out here — no breaking change, just breadth.

## Done when (from [roadmap.md](../../roadmap.md#phase-2--pattern-language))

Phase 2 is **done** when all of the following hold:

- **The README quickstart song plays as written.** The exact snippet from [README.md](../../../README.md) — `setcps`, `p1(sound("bd*4").gain(0.9))`, `p2(sound("~ hh ~ hh").lpf(6000))`, `p3(note("c2 c2 eb2 g2").saw().lpf(sine.range(300,1200).slow(8)).room(0.3).sidechain("p1"))` — parses, queries, compiles, and produces the intended sound (sidechain itself is a Phase 3 effect; Phase 2 must accept the call and route the control key).
- **Combinators and the parser have deterministic unit tests** of the canonical `queryArc(pat, begin, end)` form (query a span, assert the `Hap` list; see [pattern-model.md](../../pattern-model.md#testing-patterns)). Every combinator in [03](03-combinators.md) and every mini-notation construct in [02](02-mini-notation-parser.md) ships with such tests.
- **Parameters accept patterns/signals.** `.lpf(sine.range(300,1200).slow(8))` audibly sweeps; `.gain("0.6 0.9")` alternates per note. Continuous signals sample correctly at each `Hap`'s time ([05](05-continuous-signals.md)).
- **Cross-cutting (every phase):** a runnable example in `examples/` exercises the language; no underruns under normal load (`stats()` clean); the pure-TS logic (core, parser, combinators, scheduler math) has unit tests; validated on macOS, Linux/Windows in CI.

These bullets are the gate. The user-facing surface fixed here — `Pattern<T> = (TimeSpan) => Hap<T>[]`, the mini-notation table, the combinator vocabulary, the chaining API — is the API every later phase (effects, sampling, DX, web) decorates without redefining.
