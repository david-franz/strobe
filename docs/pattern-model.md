# The pattern model

`tekno`'s language is built on the **pattern** abstraction from [TidalCycles](https://tidalcycles.org/) (and its JS port [Strudel](https://strudel.cc/)). We adopt it deliberately because it is the proven functional vocabulary for this kind of music. This doc defines the model and the API built on top of it.

## The one idea

> **A pattern is a pure function from a span of time to the events that happen in it.**

```ts
type Pattern<T> = (span: TimeSpan) => Hap<T>[];
```

Patterns are not lists of notes. They are *functions you can sample over any time window*. This is what makes them composable and infinitely transformable — you build complex music by feeding patterns through other functions (combinators), not by editing data.

### The three primitives

```ts
// A half-open interval of cycle-time, [begin, end).
interface TimeSpan {
  begin: number;   // in cycles, e.g. 0.0
  end: number;     // in cycles, e.g. 1.0
}

// One thing that happens: a value, when it happens, and (optionally)
// the part of it visible in the queried span.
interface Hap<T> {
  whole: TimeSpan | undefined;  // the event's full extent (undefined = "analog"/continuous)
  part: TimeSpan;               // the portion intersecting the query
  value: T;                     // what plays: a sound name, note, or control map
}
```

- **Time is measured in cycles**, not seconds. A cycle is the loop unit; tempo (`cps`) maps cycles to seconds at play time (see [architecture.md](architecture.md#the-clock-and-musical-time)).
- A pattern defined over `0 → 1` repeats every cycle. Querying span `3.0 → 4.0` returns the same shape shifted to cycle 3.
- **Discrete** patterns produce `Hap`s with a `whole` (drum hits, notes). **Continuous** patterns (like `sine`) have `whole: undefined` — they are signals you sample, used to modulate parameters.

## Mini-notation

Writing raw pattern functions is tedious, so most patterns start as a **mini-notation** string — a tiny DSL parsed into a `Pattern`. It is the same notation Tidal/Strudel users know:

| Syntax | Meaning | Example |
| --- | --- | --- |
| `a b c` | sequence — split the cycle evenly | `"bd sn hh"` → 3 events |
| `~` | rest (silence) | `"bd ~ sn ~"` |
| `a*2` | repeat/speed up within its slot | `"bd*2 sn"` |
| `a/2` | slow down (play every other cycle) | `"bd/2"` |
| `[a b]` | subdivide — group events into one slot | `"bd [sn sn]"` |
| `<a b>` | alternate — one per cycle, cycling | `"bd <sn cp>"` |
| `a,b` | stack — play layers at once | `"[bd*4, hh*8]"` |
| `a(3,8)` | Euclidean rhythm — 3 hits spread over 8 | `"bd(3,8)"` |
| `a!3` | replicate | `"bd!3 sn"` |
| `a?` | randomly drop (degrade) | `"hh*8?"` |

```ts
sound("bd(3,8), ~ hh ~ hh, [~ cp]*2")   // a full groove in one string
```

`note("c e g")` and `freq("220 330")` are the pitched equivalents; `note` accepts note names (`c3`, `eb2`, `f#4`) or numbers.

## The chaining API (where it feels like an instrument)

Mini-notation gives you *what and when*. Method chaining gives you *how it sounds*. Each method attaches a control value to every `Hap` in the pattern (the `value` becomes a control map like `{ s: "bd", lpf: 800, gain: 0.9 }`), which the engine reads when the event plays.

```ts
note("c2 c2 eb2 g2")     // pitches
  .saw()                 // oscillator: sawtooth
  .lpf(800)              // low-pass filter cutoff
  .resonance(0.4)        // filter resonance
  .adsr(0.01, 0.1, 0.6, 0.2)
  .room(0.3)             // reverb send
  .gain(0.85)
  .pan(0.4)
```

### Parameters can themselves be patterns

Any parameter accepts a number **or another pattern** — including continuous signals. This is how you get movement:

```ts
note("c2*8")
  .lpf( sine.range(300, 1200).slow(8) )   // cutoff sweeps over 8 cycles
  .pan( saw.range(0, 1).fast(2) )         // auto-pan
  .gain( "0.6 0.9" )                      // alternate loud/soft per note
```

Built-in continuous signals: `sine`, `cosine`, `saw`, `isaw`, `tri`, `square`, `rand`, `perlin`. They output `0..1` by default; `.range(lo, hi)` rescales.

## Combinators — transforming patterns

Combinators are pure functions `Pattern → Pattern`. They are the heart of the expressiveness: you sculpt rhythm and structure by composing them. Available both as standalone functions and as chainable methods.

### Time

| Combinator | Effect |
| --- | --- |
| `fast(n)` / `slow(n)` | compress / stretch a pattern in time (`"bd sn".fast(2)`) |
| `rev` | reverse the cycle |
| `hurry(n)` | `fast` **and** raise pitch/speed together |
| `off(t, fn)` | overlay a copy shifted by `t`, transformed by `fn` (echoes, stabs) |
| `early(t)` / `late(t)` | nudge timing |

### Structure & repetition

| Combinator | Effect |
| --- | --- |
| `every(n, fn)` | apply `fn` only every `n`th cycle (`every(4, rev)`) |
| `whenmod(a, b, fn)` | apply `fn` on cycles where `cycle % a >= b` |
| `chunk(n, fn)` | apply `fn` to a different 1/n slice each cycle (rotating fills) |
| `iter(n)` | rotate the pattern start by 1/n each cycle |

### Layering

| Combinator | Effect |
| --- | --- |
| `stack(...pats)` | play patterns simultaneously |
| `cat(...pats)` | one pattern per cycle, in turn |
| `fastcat(...pats)` | squeeze all patterns into one cycle |
| `superimpose(fn)` | overlay a transformed copy of itself |
| `jux(fn)` | split to stereo: original left, `fn`-transformed right |

### Density & chance

| Combinator | Effect |
| --- | --- |
| `degrade` / `degradeBy(p)` | randomly drop events (`p` = probability) |
| `sometimes(fn)` / `sometimesBy(p, fn)` | apply `fn` to a random subset |
| `somecycles(fn)` | apply `fn` on random whole cycles |
| `range(lo, hi)` | rescale a continuous pattern's output |

```ts
// A pattern that evolves: reversed every 4th cycle, with a ghost echo,
// hats thinned out, and a stereo-delayed variant on the right.
sound("bd*4")
  .every(4, rev)
  .off(0.125, x => x.gain(0.5).speed(1.5))
  .jux(rev);
```

## How this maps to live coding

- A **track** (`p1`, `p2`, …) holds one pattern. Re-running `p1(...)` swaps it at the next cycle boundary (see [architecture.md](architecture.md#tracks-and-hot-swapping)).
- The scheduler queries each track's pattern over the upcoming time window, turns each `Hap` into an engine event (see [event-protocol.md](event-protocol.md)), and stamps it with a precise frame.
- Because patterns are pure functions, the scheduler can query any window at any time with no side effects — which is exactly what makes seamless hot-swapping possible.

## Why pure functions, not a sequencer object

A traditional step sequencer stores state (the grid). To change it live you mutate shared state, which races with playback. Patterns store **no state** — they are recomputed from the current time every scheduler tick. Swapping a pattern is just pointing a track at a new function. This is the property that makes `tekno` safe and seamless to edit while it plays.

## Testing patterns

Because patterns are pure, they are trivially testable without any audio:

```ts
queryArc(sound("bd ~ sn ~"), 0, 1);
// → [ {value:{s:"bd"}, part:{begin:0,   end:0.25}},
//     {value:{s:"sn"}, part:{begin:0.5, end:0.75}} ]
```

Combinators and the mini-notation parser are covered by deterministic unit tests of this form (query a span, assert the `Hap` list).
