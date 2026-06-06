# Phase 2.03 — Combinators (`combinators.ts`)

## Purpose

`combinators.ts` implements the **full combinator vocabulary** from [pattern-model.md](../../pattern-model.md#combinators--transforming-patterns). Combinators are pure functions `Pattern → Pattern` (and `Pattern → Pattern` families parameterised by numbers, other patterns, or functions). They are "the heart of the expressiveness": you sculpt rhythm and structure by composing them ([pattern-model.md](../../pattern-model.md#combinators--transforming-patterns)).

Every combinator ships **twice**: as a standalone function (`fast(2, p)`) and as a chainable method on `Pat<T>` (`p.fast(2)`). Both are the same implementation; the method form just curries `this` in last.

This file owns the canonical list:

- **Time:** `fast`, `slow`, `rev`, `hurry`, `off`, `early`, `late`.
- **Structure & repetition:** `every`, `whenmod`, `chunk`, `iter`.
- **Layering:** `stack`, `cat` (= `slowcat`), `fastcat`, `superimpose`, `jux`.
- **Density & chance:** `degrade`, `degradeBy`, `sometimes`, `sometimesBy`, `somecycles`.
- (`range` is the continuous-signal rescaler; it lives in [05-continuous-signals.md](05-continuous-signals.md) but is listed in the model's chance table.)

All time math reuses the `withTime`/`splitQueries`/`filterHaps`/`filterOnsets` primitives from [01-pattern-core.md](01-pattern-core.md) — combinators never re-derive span mapping.

## Location

```
packages/strobe/src/
  combinators.ts     # all combinators (functions) + the chainable mixin
  rng.ts             # deterministic, cycle-seeded RNG (shared by chance combinators)
  combinators.test.ts
```

`combinators.ts` imports from `pattern.ts` ([01](01-pattern-core.md)) and `rng.ts`. The chainable methods are mixed onto `Pat.prototype` here (and in [04](04-sound-note-api.md)/[05](05-continuous-signals.md)) to avoid an import cycle with the core.

### Chainable mixin

```ts
// Attach standalone fns as methods that pass `this` as the trailing pattern arg.
export function installCombinatorMethods() {
  Pat.prototype.fast = function (n) { return fast(n, this); };
  Pat.prototype.slow = function (n) { return slow(n, this); };
  Pat.prototype.rev = function () { return rev(this); };
  Pat.prototype.every = function (n, fn) { return every(n, fn, this); };
  Pat.prototype.jux = function (fn) { return jux(fn, this); };
  // …one line per combinator. fn args take and return Pat<T>.
}
```

`installCombinatorMethods()` is called once at module load. (TS declaration-merging on `interface Pat<T>` declares the method signatures so chaining type-checks.)

### Patternified arguments

Per the model, "any parameter accepts a number **or another pattern**" ([pattern-model.md](../../pattern-model.md#parameters-can-themselves-be-patterns)). `fast`, `slow`, `every`, `degradeBy`, etc. accept `number | Pat<number>`. The shared helper samples a patterned argument **per cycle** at the cycle's start:

```ts
type NumArg = number | Pat<number>;
const sampleArgPerCycle = (a: NumArg, cycle: number): number =>
  typeof a === "number" ? a : (queryArc(a, cycle, cycle)[0]?.value ?? 0);
```

When an argument is a pattern, the combinator is wrapped in `splitQueries` ([01](01-pattern-core.md)) so each cycle reads its own value (this is how `"a*<2 3>"` from [02](02-mini-notation-parser.md) alternates speed per cycle).

## Design

### Deterministic RNG (`rng.ts`)

Chance combinators must be **pure**: querying the same arc twice yields the same dropped events ([pattern-model.md](../../pattern-model.md#why-pure-functions-not-a-sequencer-object)). So randomness is a *hash of position*, never a stateful generator. We seed from the integer cycle (and a salt per combinator instance / per hap index) and produce a deterministic `0..1`:

```ts
// 32-bit integer hash → float in [0,1). Pure: same input ⇒ same output.
export function hashFloat(seed: number): number {
  let x = seed | 0;
  x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
  x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
  x = (x ^ (x >>> 16)) >>> 0;
  return x / 0x100000000;
}
// Combine a cycle, a hap index within the cycle, and a per-call salt.
export const randAt = (cycle: number, index: number, salt: number) =>
  hashFloat((cycle * 0x9e3779b1) ^ (index * 0x85ebca6b) ^ salt);
```

The `rand`/`perlin` *signals* in [05](05-continuous-signals.md) use the same `hashFloat` core sampled by time, so "noise" is consistent across the system.

### Time combinators

#### `fast` / `slow`

```ts
export const fast = (n: NumArg, p: Pat<T>): Pat<T> =>
  withArg(n, (k) => withTime(t => t * k, t => t / k, p));   // [01] withTime
export const slow = (n: NumArg, p: Pat<T>): Pat<T> => fast(typeof n === "number" ? 1 / n : recip(n), p);
```

`fast(2)` compresses two cycles of `p` into one (query time scaled up, hap time scaled down). `slow(2)` is `fast(1/2)`.

```
queryArc(fast(2, pure("x")), 0, 1)
→ [ {whole:{0,0.5},  part:{0,0.5},  value:"x"},
    {whole:{0.5,1},  part:{0.5,1},  value:"x"} ]
```

#### `rev` — reverse within each cycle

Reverse must happen *per cycle* (so it loops), so wrap in `splitQueries` and reflect time around the cycle:

```ts
export const rev = (p: Pat<T>): Pat<T> => splitQueries(pattern((span) => {
  const cyc = Math.floor(span.begin);
  const reflect = (t: number) => cyc + (cyc + 1 - t);     // mirror within [cyc, cyc+1]
  const q = { begin: reflect(span.end), end: reflect(span.begin) };
  return p.query(q).map(h => ({
    whole: h.whole ? { begin: reflect(h.whole.end), end: reflect(h.whole.begin) } : undefined,
    part:  { begin: reflect(h.part.end), end: reflect(h.part.begin) },
    value: h.value,
  }));
}));
```

```
queryArc(rev(parseMini("a b c d")), 0, 1) → onsets d,c,b,a at 0,0.25,0.5,0.75
```

#### `hurry` — `fast` and raise pitch/speed together

`hurry(n)` is `fast(n)` plus multiplying each hap's `speed`/`s pitch` by `n` ([pattern-model.md](../../pattern-model.md#time)). It is value-aware, so it lives partly in the value layer; for `sound`/`note` values it multiplies the `speed` control key:

```ts
export const hurry = (n: number, p: Pat<ControlMap>): Pat<ControlMap> =>
  fast(n, p).fmap(v => ({ ...v, speed: (v.speed ?? 1) * n }));
```

(`ControlMap` and `fmap` come from [04](04-sound-note-api.md); for non-control values `hurry` falls back to `fast`.)

#### `early` / `late` — nudge timing

```ts
export const early = (t: NumArg, p: Pat<T>): Pat<T> =>
  withArg(t, (k) => withTime(x => x + k, x => x - k, p));    // shift earlier
export const late  = (t: NumArg, p: Pat<T>): Pat<T> =>
  withArg(t, (k) => withTime(x => x - k, x => x + k, p));
```

#### `off` — overlay a shifted, transformed copy

```ts
export const off = (t: NumArg, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  stack(p, fn(late(t, p)));
```

`off(0.125, x => x.gain(0.5))` plays `p` plus a copy 1/8-cycle later at half gain (the echo/stab idiom in [pattern-model.md](../../pattern-model.md#combinators--transforming-patterns)).

### Structure & repetition

#### `every` — apply `fn` only on every nth cycle

```ts
export const every = (n: number, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  splitQueries(pattern((span) => {
    const cyc = Math.floor(span.begin);
    return (cyc % n === 0 ? fn(p) : p).query(span);
  }));
```

```
queryArc(every(4, rev, parseMini("a b")), 0, 1) → reversed (cycle 0 % 4 === 0)
queryArc(every(4, rev, parseMini("a b")), 1, 2) → unchanged
```

#### `whenmod` — apply `fn` when `cycle % a >= b`

```ts
export const whenmod = (a: number, b: number, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  splitQueries(pattern((span) => {
    const cyc = Math.floor(span.begin);
    return ((cyc % a) >= b ? fn(p) : p).query(span);
  }));
```

#### `iter` — rotate start by 1/n each cycle

```ts
export const iter = (n: number, p: Pat<T>): Pat<T> =>
  splitQueries(pattern((span) => {
    const cyc = Math.floor(span.begin);
    return early((cyc % n) / n, p).query(span);   // shift earlier by k/n on cycle k
  }));
```

`iter(4)` over `"a b c d"` plays `a b c d` then `b c d a` then `c d a b`, … one rotation per cycle.

#### `chunk` — apply `fn` to a different 1/n slice each cycle

```ts
export const chunk = (n: number, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  splitQueries(pattern((span) => {
    const cyc = Math.floor(span.begin);
    const k = cyc % n;
    // Apply fn only to the haps whose onset falls in slice [k/n, (k+1)/n) of the cycle.
    const sliceBegin = k / n, sliceEnd = (k + 1) / n;
    const inSlice = (h: Hap<T>) => {
      const pos = cyclePos(h.part.begin);
      return pos >= sliceBegin - 1e-9 && pos < sliceEnd - 1e-9;
    };
    const base = p.query(span);
    const transformed = fn(p).query(span);
    return base.map(h => inSlice(h) ? (transformed.find(t => sameTime(t.part.begin, h.part.begin)) ?? h) : h);
  }));
```

(Rotating fills: a different quarter of the bar is transformed each cycle.)

### Layering

#### `stack` — simultaneous

```ts
export const stack = (...ps: Pat<T>[]): Pat<T> =>
  pattern((span) => ps.flatMap(p => p.query(span)));
```

#### `slowcat` (= `cat`) — one pattern per cycle, in turn

```ts
export const slowcat = (...ps: Pat<T>[]): Pat<T> => splitQueries(pattern((span) => {
  const cyc = Math.floor(span.begin);
  const i = ((cyc % ps.length) + ps.length) % ps.length;
  // Query the chosen pattern at *its* cycle 0-aligned time, then shift back.
  const shift = cyc - Math.floor(cyc / ps.length);          // map outer cycle → inner cycle
  return withTime(t => t - shift, t => t + shift, ps[i]).query(span);
}));
export const cat = slowcat;
```

`<a b>` in mini-notation is `slowcat(pure("a"), pure("b"))`.

#### `fastcat` — squeeze all into one cycle

```ts
export const fastcat = (...ps: Pat<T>[]): Pat<T> => fast(ps.length, slowcat(...ps));
```

This is the workhorse the sequence parser ([02](02-mini-notation-parser.md)) targets: N patterns, each compressed into its `1/N` slot of every cycle.

#### `superimpose` — overlay a transformed copy of itself

```ts
export const superimpose = (fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> => stack(p, fn(p));
```

#### `jux` — split to stereo (original left, `fn`-transformed right)

`jux` is value-aware: it pans the original hard left and the transformed copy hard right by writing the `pan` control key ([04](04-sound-note-api.md)):

```ts
export const jux = (fn: (p: Pat<ControlMap>) => Pat<ControlMap>, p: Pat<ControlMap>): Pat<ControlMap> =>
  stack(
    p.fmap(v => ({ ...v, pan: 0 })),
    fn(p).fmap(v => ({ ...v, pan: 1 })),
  );
```

### Density & chance (deterministic)

#### `degradeBy` / `degrade`

Drop each **onset** with probability `prob`, deterministically by position. We index haps within their cycle and consult `randAt`:

```ts
export const degradeBy = (prob: NumArg, p: Pat<T>): Pat<T> =>
  withArg(prob, (pr) => pattern((span) =>
    spanCycles(span).flatMap((sub) => {
      const cyc = Math.floor(sub.begin);
      return p.query(sub).filter((h, idx) => {
        if (!hasOnset(h)) return true;             // keep fragments; decided at the onset cycle
        return randAt(cyc, idx, SALT_DEGRADE) >= pr; // keep when the draw clears the threshold
      });
    })));
export const degrade = (p: Pat<T>): Pat<T> => degradeBy(0.5, p);
```

`"hh*8?"` keeps a deterministic ~half of the 8 hats, identically on every replay of cycle 0.

#### `sometimesBy` / `sometimes`

Apply `fn` to a random subset (probability `prob`), keep the rest unchanged — implemented as the union of "transformed where the draw hits" and "original where it misses":

```ts
export const sometimesBy = (prob: NumArg, fn: (p: Pat<T>) => Pat<T>, p: Pat<T>): Pat<T> =>
  stack(
    degradeBy(prob, p),                 // the (1-prob) kept-as-is portion (draw ≥ prob)
    undegradeBy(prob, fn(p)),           // the prob portion, transformed (draw < prob)
  );
export const sometimes = (fn: (p: Pat<T>) => Pat<T>, p: Pat<T>) => sometimesBy(0.5, fn, p);
```

`undegradeBy` is `degradeBy`'s complement (keeps exactly the haps `degradeBy` would drop, using the **same** `randAt` salt so the two partitions are disjoint and cover every onset).

#### `somecycles`

Apply `fn` on whole cycles chosen at random (deterministically per cycle):

```ts
export const somecycles = (fn: (p: Pat<T>) => Pat<T>, p: Pat<T>, prob = 0.5): Pat<T> =>
  splitQueries(pattern((span) => {
    const cyc = Math.floor(span.begin);
    return (randAt(cyc, 0, SALT_SOMECYCLES) < prob ? fn(p) : p).query(span);
  }));
```

### Euclid (for [02](02-mini-notation-parser.md))

Bjorklund's algorithm produces the canonical even distribution; `euclid` then masks a base pattern's steps:

```ts
export function bjorklund(pulses: number, steps: number): boolean[] { /* standard pairing algorithm */ }

export const euclid = (pulses: number, steps: number, rotation: number, p: Pat<T>): Pat<T> => {
  const bits = rotate(bjorklund(pulses, steps), rotation);
  // Lay `steps` even slots across the cycle; a slot is `p` if its bit is set, else silence.
  return fastcat(...bits.map(on => (on ? p : silence)));
};
```

`bd(3,8)` → bits `[1,0,0,1,0,0,1,0]` → onsets at 0, 3/8, 6/8.

## Interfaces

- **Core ([01](01-pattern-core.md)):** every combinator is built from `withTime`, `splitQueries`, `filterHaps`/`hasOnset`, `spanCycles`, `pure`, `silence`, `fmap`. No new span math.
- **Mini-notation ([02](02-mini-notation-parser.md)):** `fastcat`/`slowcat`/`stack`/`fast`/`slow`/`euclid`/`degrade` are the compile targets; patternified args feed `*<…>` etc.
- **Sound/note API ([04](04-sound-note-api.md)):** value-aware combinators (`hurry`, `jux`, `off` with gain transforms) read/write `ControlMap` keys via `fmap`. `fmap` itself is defined in [04](04-sound-note-api.md) and mixed onto `Pat`.
- **Signals ([05](05-continuous-signals.md)):** `fast`/`slow` apply to continuous signals too (`sine.slow(8)`), via the same `withTime` (the signal is just sampled at scaled time).
- **Scheduler ([06](06-scheduler-integration.md)):** transparent — it queries whatever pattern the combinators produced; `filterOnsets` ensures combinator-introduced fragments don't re-trigger.

## Edge cases & risks

- **Purity / determinism:** the **only** randomness source is `rng.ts`, seeded from cycle + hap index + per-call salt — never `Math.random()` or wall-clock. Distinct salts per combinator stop `degrade` and `sometimes` from correlating. A test queries each chance combinator twice over the same arc and asserts deep equality.
- **`sometimesBy` partition correctness:** the transformed and untransformed halves must be *complementary* (every onset appears exactly once). Sharing the salt between `degradeBy` and `undegradeBy` guarantees the same draw decides both sides; tested by asserting `count(sometimesBy result) === count(original)` for `fn = identity`.
- **Fractional precision:** `iter`/`chunk`/`euclid` compute `k/n`, `j/steps` boundaries; comparisons use `sameTime`, and slice math uses the exact-rational helper where an off-by-epsilon would mis-bin a hap. Tested for `chunk(4, …)` and `iter(3, …)`.
- **Continuous patterns:** chance combinators (`degrade`) assume onsets; applied to a continuous signal they are no-ops on filtering (a signal hap has no onset). `rev`/`fast`/`slow` work on signals (pure time remap). `jux`/`hurry`/`off`-with-gain require `ControlMap` values; calling them on a bare `Pat<string>` is a type error (caught by the compiler).
- **Negative / fractional args:** `fast(0)` is guarded (returns `silence` with a warning) to avoid divide-by-zero; `slow(0)` likewise. `early`/`late` accept fractional cycles freely.
- **Performance:** combinators add a constant factor of closures per query; lookahead spans few cycles, so per-tick cost is small and entirely on the control plane. `stack` of many layers and deep nesting are linear in total haps; acceptable.

## Acceptance criteria

- `queryArc(fast(2, pure("x")), 0, 1)` → two onsets at `{0,0.5}` and `{0.5,1}`.
- `queryArc(slow(2, parseMini("a b")), 0, 2)` → `a` over `{0,1}`, `b` over `{1,2}`.
- `queryArc(rev(parseMini("a b c d")), 0, 1)` → values `d,c,b,a` at `0,0.25,0.5,0.75`.
- `queryArc(every(3, rev, parseMini("a b")), 0, 1)` is reversed; `(…, 1, 2)` is not.
- `queryArc(iter(4, parseMini("a b c d")), 1, 2)` starts with `b` (one rotation in).
- `queryArc(off(0.25, x => x, pure("x")), 0, 1)` → two haps, the second's onset 0.25 later.
- `queryArc(fastcat(pure("a"), pure("b"), pure("c")), 0, 1)` → onsets at `0, 1/3, 2/3`.
- `queryArc(slowcat(pure("a"), pure("b")), 0, 2)` → `a` in cycle 0, `b` in cycle 1.
- `queryArc(degrade(parseMini("hh*8")), 0, 1)` drops a deterministic subset and is **deep-equal across two calls**.
- `queryArc(jux(rev, sound("a b")), 0, 1)` → original haps with `pan:0` plus reversed haps with `pan:1`.
- `queryArc(euclid(3, 8, 0, pure("bd")), 0, 1)` → onsets at `0, 3/8, 6/8`.
- Every standalone fn has an equivalent `.method()` returning an identical `queryArc` result.
