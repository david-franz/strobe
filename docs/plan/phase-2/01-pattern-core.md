# Phase 2.01 — Pattern core (`pattern.ts`)

## Purpose

`pattern.ts` is the foundation every other Phase 2 file builds on. It implements exactly the model from [pattern-model.md](../../pattern-model.md): the `Pattern<T>` representation, the three primitives (`TimeSpan`, `Hap`, `Pattern`), the `queryArc` test/scheduler entry point, the fundamental constructors (`pure`/`atom`, `silence`, `signal`), the **cycle-splitting** machinery that lets a one-cycle pattern be queried over any arbitrary span, and the **discrete-vs-continuous** distinction (`whole: undefined` for analog signals).

It defines *no* musical vocabulary — no mini-notation, no `fast`, no `sound`. Those all live in later files and are expressed in terms of the primitives here. The single most important invariant established here is **purity**: a pattern is a pure function of the query span, so the scheduler can query any window at any time with no side effects ([pattern-model.md](../../pattern-model.md#why-pure-functions-not-a-sequencer-object)).

## Location

```
packages/strobe/src/
  pattern.ts        # everything in this doc
  fraction.ts       # rational time helper (see "Fractional time precision")
  pattern.test.ts   # queryArc unit tests
```

`pattern.ts` has **no imports** from other Phase 2 files (it is the leaf dependency). `fraction.ts` is an internal helper it may use for exact rational time; the public types still expose `number` for ergonomics (see Edge cases).

## Design

### The three primitives (canonical — copied verbatim from the model)

```ts
// A half-open interval of cycle-time, [begin, end).
export interface TimeSpan {
  begin: number;   // in cycles, e.g. 0.0
  end: number;     // in cycles, e.g. 1.0
}

// One thing that happens.
export interface Hap<T> {
  whole: TimeSpan | undefined;  // full extent; undefined = continuous/"analog"
  part: TimeSpan;               // portion intersecting the query
  value: T;
}

// The one idea.
export type Pattern<T> = (span: TimeSpan) => Hap<T>[];
```

We keep `Pattern<T>` as a bare function type so it is trivially composable, but we also ship a thin **wrapper class** `Pat<T>` that carries the chainable methods (`.fast`, `.gain`, …). The query function is the source of truth; the class just forwards. See [03-combinators.md](03-combinators.md) and [04-sound-note-api.md](04-sound-note-api.md) for how methods attach.

```ts
export class Pat<T> {
  constructor(public readonly query: Pattern<T>) {}
  // Combinator methods are mixed in (Object.assign on the prototype) from
  // combinators.ts / sound.ts to avoid an import cycle. See those docs.
}

// Construct a Pat from a raw query function.
export function pattern<T>(query: Pattern<T>): Pat<T> {
  return new Pat(query);
}
```

Throughout, "a pattern" means a `Pat<T>`; `pat.query(span)` is the raw function. `queryArc` accepts either.

### `queryArc` — the universal entry point

`queryArc` is the function the canonical [testing section](../../pattern-model.md#testing-patterns) and the scheduler both use. It builds a `TimeSpan` and queries:

```ts
export function queryArc<T>(pat: Pat<T> | Pattern<T>, begin: number, end: number): Hap<T>[] {
  const query = typeof pat === "function" ? pat : pat.query;
  const haps = query({ begin, end });
  return haps;
}
```

The query must guarantee the canonical contract:

- Every returned `Hap.part` is **non-empty** (`part.begin < part.end`) and **clipped to the query span** (`part.begin >= begin`, `part.end <= end`).
- For discrete haps, `whole` is set and `part ⊆ whole`. The hap "onset" is `part.begin === whole.begin` (used by combinators that act on onsets only).
- For continuous haps, `whole === undefined` and `part` equals the (sub)span sampled.

### TimeSpan helpers

```ts
export const spanIntersect = (a: TimeSpan, b: TimeSpan): TimeSpan | undefined => {
  const begin = Math.max(a.begin, b.begin);
  const end = Math.min(a.end, b.end);
  if (begin > end) return undefined;            // disjoint
  if (begin === end && a.begin !== a.end && b.begin !== b.end) return undefined; // zero-width touch
  return { begin, end };
};

export const sameTime = (a: number, b: number) => Math.abs(a - b) < 1e-9;

// Split a span at every integer cycle boundary it crosses.
export function spanCycles(span: TimeSpan): TimeSpan[] {
  const out: TimeSpan[] = [];
  let { begin, end } = span;
  if (begin >= end) {                            // zero-width query (sampling a signal)
    return begin === end ? [span] : [];
  }
  while (begin < end) {
    const nextBoundary = Math.floor(begin) + 1;
    const segEnd = Math.min(nextBoundary, end);
    out.push({ begin, end: segEnd });
    begin = segEnd;
  }
  return out;
}

export const cyclePos = (t: number) => t - Math.floor(t);   // fractional part within a cycle
```

`spanCycles` is the core of "querying across multiple cycles": a pattern is defined over `0→1` and repeats, so to query `0.5→2.5` we split into `[0.5,1)`, `[1,2)`, `[2,2.5)`, evaluate each cycle independently, then reassemble. Every cycle-aware constructor and combinator uses this.

### Fundamental constructors

#### `pure` / `atom` — one value filling each cycle

A discrete pattern that plays `value` once per cycle, its `whole` spanning the full cycle:

```ts
export function pure<T>(value: T): Pat<T> {
  return pattern((span) =>
    spanCycles(span).map((sub) => {
      const cycle = Math.floor(sub.begin);
      const whole: TimeSpan = { begin: cycle, end: cycle + 1 };
      return { whole, part: sub, value };
    }),
  );
}
export const atom = pure;   // alias used by mini-notation
```

Worked example — `queryArc(pure("bd"), 0, 1)`:

```
spanCycles({0,1}) = [{0,1}]
cycle 0 → whole {0,1}, part {0,1}
→ [ { whole:{begin:0,end:1}, part:{begin:0,end:1}, value:"bd" } ]
```

Worked example — `queryArc(pure("bd"), 0.5, 2)` (crosses cycles):

```
spanCycles = [{0.5,1}, {1,2}]
{0.5,1}: cycle 0 → whole {0,1}, part {0.5,1}   ← a fragment (onset 0 < part.begin 0.5)
{1,2}:   cycle 1 → whole {1,2}, part {1,2}      ← a full onset
→ [ {whole:{0,1}, part:{0.5,1}, value:"bd"},
    {whole:{1,2}, part:{1,2},   value:"bd"} ]
```

This fragment behavior is exactly why `Hap` separates `whole` from `part`.

#### `silence` — the empty pattern

```ts
export const silence: Pat<never> = pattern(() => []);
```

Used by `~` in mini-notation and as the identity for `stack`.

#### `signal` — continuous patterns

A continuous pattern is sampled at a point in time; it has no `whole`. The generic constructor takes a function of (cycle-)time and emits one continuous hap per sub-span, sampled at the sub-span's **midpoint** (so a zero-width sample at `t` reads exactly `t`):

```ts
export function signal<T>(sample: (t: number) => T): Pat<T> {
  return pattern((span) => {
    if (span.begin === span.end) {                       // point sample
      return [{ whole: undefined, part: span, value: sample(span.begin) }];
    }
    // For a non-zero span, sample once at the midpoint and report the whole span as `part`.
    const mid = (span.begin + span.end) / 2;
    return [{ whole: undefined, part: span, value: sample(mid) }];
  });
}
```

The actual continuous *waveforms* (`sine`, `saw`, `rand`, …) are built on `signal` in [05-continuous-signals.md](05-continuous-signals.md). `pattern.ts` only ships the generic constructor.

> **Sampling convention.** When a parameter pattern (discrete or continuous) is sampled to fill a control value at a note's onset (see [05](05-continuous-signals.md) and [06](06-scheduler-integration.md)), the scheduler does a **zero-width query at the note's onset time** (`queryArc(param, t, t)`) and takes the resulting hap's value. `signal` returning `sample(span.begin)` for zero-width queries makes that exact and total.

### Internal time-manipulation helpers

Three private helpers underpin most combinators; they are defined here so [03](03-combinators.md) can reuse them without re-deriving the math. They map the **query span backward** through a time function, run the inner pattern, then map the **resulting haps forward**.

```ts
type TimeFn = (t: number) => number;

const withQueryTime = <T>(f: TimeFn, p: Pat<T>): Pattern<T> =>
  (span) => p.query({ begin: f(span.begin), end: f(span.end) });

const withHapTime = <T>(f: TimeFn, haps: Hap<T>[]): Hap<T>[] =>
  haps.map((h) => ({
    whole: h.whole ? { begin: f(h.whole.begin), end: f(h.whole.end) } : undefined,
    part: { begin: f(h.part.begin), end: f(h.part.end) },
    value: h.value,
  }));

// The composed primitive: query through fq, then map haps through fh.
export const withTime = <T>(fq: TimeFn, fh: TimeFn, p: Pat<T>): Pat<T> =>
  pattern((span) => withHapTime(fh, withQueryTime(fq, p)(span)));
```

`fast(n)` is then `withTime(t => t*n, t => t/n, p)`; `late(t)` is `withTime(x => x-t, x => x+t, p)`; etc. Keeping this in the core guarantees every time combinator shares one correct, tested implementation. See [03](03-combinators.md).

### `splitQueries` — make a pattern cycle-by-cycle

Any constructor that must behave differently per cycle (alternation `<>`, `every`, `iter`) wraps its query so each integer cycle is evaluated separately:

```ts
export const splitQueries = <T>(p: Pat<T>): Pat<T> =>
  pattern((span) => spanCycles(span).flatMap((sub) => p.query(sub)));
```

This is the second pillar (with `withTime`) that the whole combinator layer stands on.

### `filterHaps` and `filterOnsets`

```ts
export const filterHaps = <T>(pred: (h: Hap<T>) => boolean, p: Pat<T>): Pat<T> =>
  pattern((span) => p.query(span).filter(pred));

// Onset = the hap's part begins exactly at its whole's begin (no leading fragment).
export const hasOnset = <T>(h: Hap<T>): boolean =>
  h.whole !== undefined && sameTime(h.whole.begin, h.part.begin);

export const filterOnsets = <T>(p: Pat<T>): Pat<T> => filterHaps(hasOnset, p);
```

`filterOnsets` is what `degrade`, the scheduler, and "play only the start of each note" rely on, so a fragment created by cycle-splitting is never re-triggered.

## Interfaces

- **[02-mini-notation-parser.md](02-mini-notation-parser.md)** compiles its AST into patterns using `pure`, `silence`, `splitQueries`, and the sequence builder (`fastcat`, which lives in [03](03-combinators.md) but is itself written on `withTime`/`splitQueries` from here).
- **[03-combinators.md](03-combinators.md)** implements every time combinator with `withTime`, every cycle-conditional combinator with `splitQueries`, and chance combinators with `filterHaps`/`filterOnsets`.
- **[04-sound-note-api.md](04-sound-note-api.md)** maps over `Hap.value` to attach control maps; it never touches time, so it is pure value-mapping over the haps these primitives produce.
- **[05-continuous-signals.md](05-continuous-signals.md)** builds `sine`/`saw`/`rand` on `signal`, and `.range` maps their values.
- **[06-scheduler-integration.md](06-scheduler-integration.md)** calls `queryArc(trackPattern, windowBegin, windowEnd)` each tick and `filterOnsets` so only true note-starts become `NoteOn`/`TrigSample`; it samples parameter patterns with zero-width `queryArc`.

## Edge cases & risks

- **Purity (the cardinal invariant).** Query functions must be referentially transparent: same span in ⇒ same haps out, no captured mutable state, no `Date.now()`, no global RNG read at query time without a span-derived seed. Chance combinators ([03](03-combinators.md)) get determinism by seeding RNG from the **cycle number in the span**, never from wall-clock. A lint/test asserts that querying the same arc twice returns deep-equal results.
- **Fractional time precision.** Cycle-time is `number` (f64). Repeated `*n` / `/n` (e.g. nested `[a b]*3` inside `<>`) accumulates rounding error; an onset meant to land at `1/3` may come back as `0.33333333331`. Mitigations: (1) all boundary comparisons go through `sameTime` (epsilon `1e-9`); (2) `fraction.ts` provides an exact rational type used *internally* by the mini-notation sequence math and Euclid generation where exactness matters, converting to `number` only at the `Hap` boundary; (3) `queryArc` snaps part/whole edges within epsilon of an integer or simple fraction. Tests assert `bd*3` onsets are `sameTime` to `0, 1/3, 2/3`.
- **Infinite / continuous patterns.** A continuous pattern has no discrete onsets; querying it over a positive-width span returns exactly one hap (midpoint sample). It must **not** be cycle-split into unbounded haps and must never be turned into `NoteOn`s — only sampled. The scheduler guards this: continuous patterns are only legal as *parameter* arguments, never as a track's top-level trigger pattern (a top-level `signal` would produce no onsets, hence silence — we warn).
- **Zero-width queries.** Sampling at a point (`begin === end`) must return a value (for `signal`) or, for discrete patterns, the hap whose `whole` contains the point. `spanCycles` returns `[span]` for `begin === end` so `pure` still answers a point query. Tested.
- **Performance of querying.** `spanCycles` is O(cycles in span); the lookahead window is ~100 ms, so at typical cps it spans a small constant number of cycles per tick (often < 2). `flatMap`/`map` allocate per tick, which is fine on the soft-real-time control plane (the audio thread never runs this). We avoid per-*hap* closures in hot loops where cheap, but correctness and clarity win over micro-optimization here; the scheduler reuses one output `ArrayBuffer` ([06](06-scheduler-integration.md)) so pattern-side allocation does not reach the engine.

## Acceptance criteria

- `queryArc(pure("bd"), 0, 1)` returns `[{ whole:{begin:0,end:1}, part:{begin:0,end:1}, value:"bd" }]`.
- `queryArc(pure("bd"), 0, 2)` returns two haps with `whole` `{0,1}` and `{1,2}` (cycle splitting verified).
- `queryArc(pure("bd"), 0.5, 1)` returns one hap with `whole:{0,1}`, `part:{0.5,1}`, and `hasOnset(hap) === false` (fragment detection).
- `queryArc(silence, 0, 4)` returns `[]`.
- `queryArc(signal(t => t), 0.25, 0.25)` returns `[{ whole: undefined, part:{begin:0.25,end:0.25}, value:0.25 }]` (point sample of a continuous pattern, `whole` undefined).
- `spanCycles({begin:0.5, end:2.5})` returns `[{0.5,1},{1,2},{2,2.5}]`.
- `withTime(t => t*2, t => t/2, pure("x"))` queried over `0..1` yields two onsets at `whole` `{0,0.5}` and `{0.5,1}` (the `fast(2)` mechanic, exercised through the core helper).
- **Purity test:** for a fixed pattern and arc, two successive `queryArc` calls deep-equal each other.
- **Precision test:** the three onsets of a 3-element sequence over one cycle are `sameTime` to `0`, `1/3`, `2/3`.
