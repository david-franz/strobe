# strobe — user guide

A practical reference to everything you can write today (Phases 0–2). For the design behind
it, see [architecture.md](architecture.md) and [pattern-model.md](pattern-model.md).

A song is just TypeScript that assigns patterns to tracks. Run one with hot-reload:

```bash
pnpm demo:phase2          # plays examples/phase2-song.ts; edit + save to hear changes
```

```ts
import { setcps, p1, p2, p3, sound, note, sine } from "strobe";

setcps(130 / 60 / 4);                       // ~130 BPM, 4 beats per cycle
p1( sound("bd*4").gain(0.9) );              // kick
p2( sound("~ hh ~ hh").lpf(6000) );         // offbeat hats
p3( note("c2 c2 eb2 g2").saw().lpf(sine.range(300,1200).slow(8)) );  // bass + filter sweep
```

Edit any line and save: the change swaps in at the **next cycle boundary** with no dropout.

---

## Core concepts

- **Cycle** — the unit of musical time. A pattern is defined over one cycle (`0 → 1`) and
  repeats. Tempo is **cycles per second** (`cps`).
- **Pattern** — a pure function from a span of time to the events in it. Everything below
  builds or transforms patterns.
- **Track** — a named slot (`p1`…`p8`) holding one pattern. Re-running `pN(...)` swaps it.

### Tempo

| Function | Does |
| --- | --- |
| `setcps(x)` | set tempo directly, in cycles per second |
| `setbpm(n)` | set tempo in BPM (assumes 4 beats per cycle: `cps = bpm/60/4`) |

---

## Mini-notation

The string passed to `sound`/`note`/`freq` is **mini-notation** — a tiny rhythm DSL. A
sequence splits the cycle evenly; everything else nests inside.

| Syntax | Meaning | Example |
| --- | --- | --- |
| `a b c` | sequence — split the cycle evenly between the steps | `"bd sn hh"` |
| `~` | a rest (silence) | `"bd ~ sn ~"` |
| `a*n` | speed a step up `n`× within its slot | `"bd*2 sn"` |
| `a/n` | slow a step down (play every `n`th cycle) | `"bd/2"` |
| `[a b]` | group — subdivide one slot | `"bd [sn sn]"` |
| `<a b>` | alternate — one option per cycle, cycling | `"bd <sn cp>"` |
| `a, b` | stack — layers at once (inside `[]` or `<>` or at top level) | `"[bd*4, hh*8]"` |
| `a(p,s)` | Euclidean rhythm — `p` pulses spread over `s` steps | `"bd(3,8)"` |
| `a(p,s,r)` | …rotated by `r` steps | `"bd(3,8,2)"` |
| `a!n` | replicate — `n` separate copies as sibling steps | `"bd!3 sn"` |
| `a?` / `a?p` | randomly drop (degrade) each hit, prob `0.5` (or `p`) | `"hh*8?"` |

`n`/`p`/`s` can themselves be patterns: `"bd*<2 3>"` alternates the speed per cycle.

---

## sound / note / freq

These turn a mini-notation string into a pattern of **control maps** (`{ s, gain, lpf, … }`).

| Constructor | Each token becomes | Use for |
| --- | --- | --- |
| `sound("…")` (alias `s`) | `{ s: token }` | drums + named instruments |
| `note("…")` | `{ note: semitone }` | melodies by note name/number |
| `freq("…")` | `{ freq: hz }` | melodies by raw frequency |

`note` accepts note names (`c3`, `eb2`, `f#4`, default octave 5) or bare semitone numbers;
`a4` = 440 Hz. Built-in sounds: drums `bd sn cp hh oh`, synths `saw square sine`.

### Modifiers (chain after a constructor)

Each returns a new pattern with one key set; values may be numbers, patterns, or signals.

| Method | Sets | Range / note |
| --- | --- | --- |
| `.gain(x)` | level → velocity | `0..1` |
| `.pan(x)` | stereo position | `0` L … `1` R |
| `.lpf(hz)` / `.hpf(hz)` | filter cutoff | drives the voice's filter |
| `.resonance(q)` (alias `.res`) | filter resonance/Q | `~0.5..20` |
| `.adsr(a,d,s,r)` | amp envelope | seconds, sustain `0..1` |
| `.saw()/.square()/.sine()/.tri()` | synth waveform | sets the `s` key |
| `.note(n)` | tune (e.g. a kick) | semitone |
| `.speed(x)` | playback speed/pitch | sampler (Phase 4) |
| `.room(x)` | reverb send | *accepted now, audible in Phase 3* |
| `.sidechain("p1")` | duck against a track | *accepted now, audible in Phase 3* |

```ts
note("c2 c2 eb2 g2").saw().lpf(800).resonance(0.4).adsr(0.01,0.1,0.6,0.2).gain(0.85).pan(0.4)
```

---

## Combinators

Pure `pattern → pattern` transforms. Each is available **as a function** (`fast(2, p)`) and
**as a method** (`p.fast(2)`); `fn` arguments take and return a pattern.

### Time

| Combinator | Effect |
| --- | --- |
| `fast(n, p)` / `slow(n, p)` | compress / stretch in time |
| `rev(p)` | reverse within each cycle |
| `hurry(n, p)` | `fast(n)` **and** raise speed/pitch by `n` |
| `early(t, p)` / `late(t, p)` | nudge earlier / later by `t` cycles |
| `off(t, fn, p)` | overlay a copy shifted by `t`, transformed by `fn` (echoes/stabs) |

### Structure

| Combinator | Effect |
| --- | --- |
| `every(n, fn, p)` | apply `fn` only on every `n`th cycle |
| `whenmod(a, b, fn, p)` | apply `fn` when `cycle % a >= b` |
| `iter(n, p)` | rotate the start by `1/n` each cycle |
| `chunk(n, fn, p)` | apply `fn` to a different `1/n` slice each cycle |

### Layering

| Combinator | Effect |
| --- | --- |
| `stack(...ps)` | play patterns simultaneously |
| `cat(...ps)` / `slowcat(...ps)` | one pattern per cycle, in turn |
| `fastcat(...ps)` | squeeze all patterns into one cycle |
| `superimpose(fn, p)` | overlay a transformed copy of `p` |
| `jux(fn, p)` | split to stereo: original left, `fn`-transformed right |

### Chance (deterministic per cycle position)

| Combinator | Effect |
| --- | --- |
| `degrade(p)` / `degradeBy(prob, p)` | randomly drop hits (`0.5` / `prob`) |
| `sometimes(fn, p)` / `sometimesBy(prob, fn, p)` | apply `fn` to a random subset |
| `somecycles(fn, p)` | apply `fn` on random whole cycles |

Randomness is a hash of cycle + position, so a `?`/`degrade` pattern is reproducible — but
it realises *differently at different cycle numbers* (that's intended movement, not drift).

```ts
sound("bd*4").every(4, rev).off(0.125, x => x.gain(0.5)).jux(rev)
```

---

## Signals (continuous patterns)

Smooth values that move over time, for modulating parameters. Output `0..1`; one period =
one cycle by default.

`sine` · `cosine` · `saw` · `isaw` · `tri` · `square` · `rand` (white noise) · `perlin`
(smooth noise).

| Method | Effect |
| --- | --- |
| `.range(lo, hi)` | rescale `0..1` → `lo..hi` |
| `.slow(n)` / `.fast(n)` | stretch / compress the period (e.g. `.slow(8)` = 8-cycle sweep) |

Use one as a parameter value — it is sampled at each note's onset (or sent as a smooth
ramp for filter keys):

```ts
note("c2*8").lpf( sine.range(300,1200).slow(8) )   // an 8-cycle filter sweep
note("c2*8").pan( saw.range(0,1).fast(2) )         // auto-pan
note("c2*8").gain( "0.6 0.9" )                      // a mini-string also works (per note)
```

---

## Tracks & live control

| Function | Effect |
| --- | --- |
| `p1(pat)` … `p8(pat)` | assign a track; re-running swaps at the next cycle boundary |
| `mute(id)` / `unmute(id)` | hide / show a track |
| `solo(id)` / `unsolo(id)` | play only soloed tracks |
| `hush()` | silence everything (panic) |

Deleting a `pN(...)` line and saving stops that track. A syntax error keeps the last good
version playing while you fix it.

---

## How it fits together

Mini-notation gives you *what and when*; `sound`/`note` attach *how it sounds*; combinators
*reshape* it; signals *move* its parameters; tracks *layer* and *swap* it live. Because every
pattern is a pure function, all of these compose freely and hot-swapping is seamless:

```ts
p1( sound("bd(3,8), ~ cp").gain(0.9) );
p2( sound("hh*8?").gain(0.5).pan(sine.fast(4)) );
p3( note("<c2 eb2> g2 bb1 f2").saw().lpf(900).every(2, x => x.fast(2)) );
```

> **Not audible yet:** `room`, `sidechain`, and audio-rate filter ramps are parsed and
> compiled today but realised by the **Phase 3** effects rack. `speed`/sampling is **Phase 4**.
