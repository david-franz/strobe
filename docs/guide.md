# strobe — user guide

A practical, conceptual reference to everything you can write today (Phases 0–3). For the
design *behind* it, see [architecture.md](architecture.md) and [pattern-model.md](pattern-model.md).

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

## The mental model

Everything in strobe is built from one idea: **a pattern is a pure function from a span of
time to the events that happen in it.** You don't build a list of notes; you describe a
function, and the engine *queries* it for whatever slice of time it needs to play next.

Three consequences follow, and they explain why the rest of the API looks the way it does:

- **Patterns are infinite and lazy.** `sound("bd*4")` isn't four events — it's a recipe that,
  asked for cycle 9, returns cycle 9's four kicks. Nothing is precomputed, so a pattern can
  loop forever and combinators can transform *time itself* (play it twice as fast, reversed,
  every third cycle) just by composing functions.
- **They compose freely.** Because every combinator is `pattern → pattern`, you can stack
  transforms in any order and they always produce another valid pattern. There are no
  "invalid states" to guard against.
- **Hot-swapping is seamless.** Reassigning a track replaces one pure function with another.
  The engine simply queries the new function from the next cycle boundary on — no notes are
  "in flight" to clean up, so there's no click or dropout.

### The vocabulary

| Term | What it is |
| --- | --- |
| **Cycle** | The unit of musical time. A pattern is defined over one cycle (`0 → 1`) and repeats. Think of it as one bar/loop. |
| **cps** | **Cycles per second** — the tempo. `setbpm(130)` is just `setcps(130/60/4)`. |
| **Hap** | One event the pattern emits: a value plus *when* it happens (`whole` = its full timespan, `part` = the slice you queried). An onset is a hap whose `whole` starts inside the queried span. |
| **Pattern** | The pure time→haps function. `Pat<T>` is the chainable wrapper you actually use. |
| **Control map** | The value most patterns carry: `{ s, note, gain, lpf, … }` — *what to play and how it sounds.* |
| **Track** | A named slot (`p1`…`p8`) holding one pattern. Re-running `pN(...)` swaps it live. |

### Tempo

| Function | Does |
| --- | --- |
| `setcps(x)` | set tempo directly, in cycles per second |
| `setbpm(n)` | set tempo in BPM (assumes 4 beats per cycle: `cps = bpm/60/4`) |

Tempo is global and applies to every track. Changing it live re-anchors the clock smoothly
(the cycle you're in keeps its phase), so you can `setcps` from the REPL without a glitch.

---

## Mini-notation

The string passed to `sound`/`note`/`freq` is **mini-notation** — a tiny rhythm DSL borrowed
from TidalCycles. The core rule: **a sequence divides its slot of time evenly between its
steps, and every bracket nests another sequence inside one step.** That single rule, applied
recursively, gives you arbitrarily intricate rhythm from short strings.

```text
"bd sn hh cp"     →  4 steps, each 1/4 of the cycle
"bd [sn sn] hh"   →  3 steps; the middle 1/3 is split into two 1/6 hits
"bd*4"            →  one step that repeats 4× inside its slot (here, the whole cycle)
```

| Syntax | Meaning | Example |
| --- | --- | --- |
| `a b c` | sequence — split the cycle evenly between the steps | `"bd sn hh"` |
| `~` | a rest (silence) — holds its slot of time | `"bd ~ sn ~"` |
| `a*n` | speed a step up `n`× *within its slot* (repeats) | `"bd*2 sn"` |
| `a/n` | slow a step down — play it once every `n` cycles | `"bd/2"` |
| `[a b]` | group — subdivide one slot into its own sequence | `"bd [sn sn]"` |
| `<a b>` | alternate — pick one option per cycle, cycling round | `"bd <sn cp>"` |
| `a, b` | stack — layer simultaneously (inside `[]`/`<>` or at top level) | `"[bd*4, hh*8]"` |
| `a(p,s)` | Euclidean rhythm — `p` pulses spread as evenly as possible over `s` steps | `"bd(3,8)"` |
| `a(p,s,r)` | …rotated by `r` steps | `"bd(3,8,2)"` |
| `a!n` | replicate — `n` separate copies as sibling steps (lengthens the sequence) | `"bd!3 sn"` |
| `a?` / `a?p` | randomly drop (degrade) each hit, probability `0.5` (or `p`) | `"hh*8?"` |

**`*` vs `!` vs `[]` — a common confusion.** `bd*3` keeps *one* step slot and crams 3 hits
into it; `bd!3` makes *three* step slots (so `"bd!3 sn"` is a 4-step sequence); `[bd bd bd]`
is the same as `bd*3`. Use `*` for rolls/ratchets inside a step, `!` to lengthen the bar.

**Arguments can themselves be patterns.** Anywhere a number goes, you can put a `<…>`
alternation: `"bd*<2 3>"` plays 2 hits one cycle, 3 the next; `"bd(<3 5>,8)"` alternates the
Euclidean density. This is the same "everything is a pattern" idea reaching into the notation.

**Euclidean rhythms** (`(p,s)`) are worth knowing: they distribute `p` hits over `s` slots as
evenly as possible — the backbone of a huge amount of world and electronic music. `(3,8)` is
the tresillo `x..x..x.`; `(5,8)` is the cinquillo; `(2,5)`, `(7,16)` etc. all sound musical.

---

## sound / note / freq

These constructors turn a mini-notation string into a **pattern of control maps** — the
values your tracks play. Pick the one that matches how you're thinking about pitch:

| Constructor | Each token becomes | Use for |
| --- | --- | --- |
| `sound("…")` (alias `s`) | `{ s: token }` | drums + named instruments |
| `note("…")` | `{ note: semitone }` | melodies by note name/number |
| `freq("…")` | `{ freq: hz }` | melodies by raw frequency |

`note` accepts note names (`c3`, `eb2`, `f#4`; default octave **5**) or bare semitone numbers,
with `a4` = 440 Hz. `freq` is the escape hatch when you want an exact Hz (drones, detuning,
non-12-tone tunings). Built-in sounds: drums `bd sn cp hh oh`, synth waveforms `saw square
sine` (and via modifiers below).

**What a control map *is*.** Each hap carries a plain object like `{ s: "saw", note: 38,
lpf: 800, gain: 0.8 }`. The constructor sets one key; every modifier below adds or overrides
another. The scheduler reads this map at each onset and compiles it into engine events
(a note-on plus parameter sets). So "an instrument with effects" is really just "a control
map with more keys" — there's no separate object to construct.

### Modifiers (chain after a constructor)

Each returns a *new* pattern with one key set (patterns are immutable — chaining never
mutates). Values may be plain numbers, mini-notation strings, full patterns, or signals.

| Method | Sets | Range / note |
| --- | --- | --- |
| `.gain(x)` | level → note velocity | `0..1` (clamped) |
| `.pan(x)` | stereo position | `0` left … `1` right (`0.5` center) |
| `.adsr(a,d,s,r)` | amp envelope | attack/decay/release in **seconds**, sustain `0..1` |
| `.saw()/.square()/.sine()/.tri()` | synth waveform | shortcut that sets the `s` key |
| `.note(n)` | (re)tune — e.g. pitch a kick | semitone or note name |
| `.speed(x)` | sample playback speed/pitch | sampler — *Phase 4* |

The **filter, EQ, and the rest of the effects** (`.lpf`, `.room`, `.sidechain`, …) are track-level
and have their own section below.

```ts
note("c2 c2 eb2 g2").saw().lpf(800).resonance(0.4).adsr(0.01,0.1,0.6,0.2).gain(0.85).pan(0.4)
```

**Using modifiers effectively:**

- **`gain` is per-hit dynamics, not a master volume.** Pattern it for groove:
  `.gain("0.9 0.6 0.7 0.6")` accents the downbeat. The master limiter (see architecture)
  catches the sum, so you rarely have to think about clipping.
- **`lpf`/`hpf` shape timbre more than any single thing.** A static `lpf(800)` darkens a saw
  bass; a *moving* `lpf` (a signal — see below) is the classic filter sweep. Pair a low
  cutoff with high `resonance` for an acid/squelch character.
- **`adsr` is the difference between a pluck and a pad.** Short attack + short decay + low
  sustain = percussive; long attack + high sustain = a swell. Drums use a fixed one-shot
  envelope and ignore note-off, so `adsr` mostly matters for `note`/`freq` synth voices.

---

## Combinators

Pure `pattern → pattern` transforms — the verbs of the language. They *reshape time and
structure* without you ever touching individual events.

Most are available **both ways**: as a standalone function `fast(2, p)` and as a chainable
method `p.fast(2)`. In a song the **method form reads best** (`p.fast(2).rev()`). Two notes
on naming: **`every` is method-only** — call `p.every(4, rev)`, not `every(...)`, because the
bare `every` name is the Phase-1 builder; and **Euclidean rhythm is reached through
mini-notation** `"bd(3,8)"` rather than a combinator.

The `fn` argument some combinators take is itself a `pattern → pattern` function — usually
another combinator or a small arrow: `every(4, x => x.fast(2).rev(), p)`.

### Time

| Combinator | Effect | Reach for it when… |
| --- | --- | --- |
| `fast(n, p)` / `slow(n, p)` | compress / stretch in time | you want a part to run at a different rate from the cycle |
| `rev(p)` | reverse within each cycle (still loops) | a fill, or the right side of a `jux` |
| `hurry(n, p)` | `fast(n)` **and** raise speed/pitch by `n` | sped-up *and* pitched-up stabs (sampler-flavored) |
| `early(t, p)` / `late(t, p)` | nudge earlier / later by `t` cycles | push/pull feel, swing, pre-delay |
| `off(t, fn, p)` | overlay a copy shifted by `t`, transformed by `fn` | echoes, octave stabs, call-and-response |

`off` is the workhorse for thickening: `off(0.125, x => x.gain(0.5), p)` adds a quieter
eighth-note-late echo. `early`/`late` take *fractions of a cycle*, so `late(1/16, p)` is a
sixteenth.

### Structure & repetition

| Combinator | Effect |
| --- | --- |
| `every(n, fn, p)` | apply `fn` only on every `n`th cycle (cycle 0, n, 2n, …) |
| `whenmod(a, b, fn, p)` | apply `fn` whenever `cycle % a >= b` (e.g. "last 2 of every 8") |
| `iter(n, p)` | rotate the start point by `1/n` each cycle (a cycling phase shift) |
| `chunk(n, fn, p)` | apply `fn` to a different `1/n` slice of the cycle each cycle — a rotating fill |

`every` is how you get variation over time without writing it out: `p.every(4, rev)` keeps
things fresh every fourth bar. `chunk(4, x => x.fast(2), p)` "walks" a ratchet across the bar,
one quarter at a time — great for evolving hi-hats and breaks.

### Layering

| Combinator | Effect |
| --- | --- |
| `stack(...ps)` | play several patterns at once (a chord, or a full kit on one track) |
| `cat(...ps)` / `slowcat(...ps)` | one whole pattern per cycle, in turn |
| `fastcat(...ps)` | squeeze all the patterns into a single cycle (sequence builder) |
| `superimpose(fn, p)` | overlay `p` with an `fn`-transformed copy of itself |
| `jux(fn, p)` | split to stereo: original in the left ear, `fn`-transformed in the right |

`stack` layers; `cat` sequences across cycles; `fastcat` sequences within one cycle.
`superimpose(x => x.add?…)`-style overlays and `jux(rev)` (reversed in one ear) are instant
width. Note `jux` lives on `sound`/`note` patterns and pans the two copies hard L/R.

### Chance (deterministic)

| Combinator | Effect |
| --- | --- |
| `degrade(p)` / `degradeBy(prob, p)` | randomly drop hits (`0.5` / `prob`) |
| `sometimes(fn, p)` / `sometimesBy(prob, fn, p)` | apply `fn` to a random subset of hits |
| `somecycles(fn, p)` | apply `fn` on random whole cycles |

**Randomness here is deterministic** — a hash of cycle number + position, not a live RNG. So
a `?`/`degrade` pattern is *reproducible* (the same bar always plays the same), yet it
*realises differently at different cycle numbers*. That's the point: you get evolving,
non-repeating movement that is still exactly the same every time you run the song.
`sometimesBy(0.3, x => x.fast(2), p)` ratchets ~30% of hits; `degrade` thins a busy hat.

```ts
sound("bd*4").every(4, rev).off(0.125, x => x.gain(0.5)).jux(rev)
```

---

## Signals (continuous patterns)

Signals are **smooth, continuously-varying values** for *modulating parameters over time* —
the source of sweeps, LFOs, auto-pan, and slow evolution. Unlike note patterns (which emit
discrete haps), a signal has a value at *every* instant.

`sine` · `cosine` · `saw` · `isaw` (reverse saw) · `tri` · `square` · `rand` (white noise) ·
`perlin` (smooth noise).

Each outputs `0..1`, with **one period = one cycle** by default.

| Method | Effect |
| --- | --- |
| `.range(lo, hi)` | rescale `0..1` → `lo..hi` (the value you almost always want) |
| `.slow(n)` / `.fast(n)` | stretch / compress the period (`.slow(8)` = one sweep every 8 cycles) |

**How a signal is sampled matters** — and it's the most important conceptual point here:

- For **most parameters** (`gain`, `pan`, `resonance`, …), the signal is read **once at each
  note's onset**. So `pan(sine.fast(4))` gives each note a pan position; it does *not* glide
  the pan during a held note. To hear movement, you need notes for it to move *across*.
- For **filter cutoff** (`lpf`/`hpf`), strobe sends the signal as a **smooth ramp** to the
  engine, so the cutoff actually glides continuously between onsets — that's why the classic
  `lpf(sine.range(300,1200).slow(8))` sweep sounds liquid rather than stepped.

```ts
note("c2*8").lpf( sine.range(300,1200).slow(8) )   // a smooth 8-cycle filter sweep
note("c2*8").pan( saw.range(0,1).fast(2) )         // step the pan per note
note("c2*8").gain( "0.6 0.9" )                      // a mini-string also works (per note)
```

Reach for `slow(n)` to make a sweep span a whole phrase; combine signals with `.range` to aim
them at a parameter's useful span (a filter likes hundreds-to-thousands of Hz, not `0..1`).

---

## Effects (the rack)

Each track has its own **effect chain** — a fixed set of inserts processed in a sensible order —
plus two **shared send buses** (a reverb and a delay) that every track can feed. You don't build
the rack; it's always there. An effect simply **wakes up when you first use it** and sleeps (costs
nothing) when you don't. Every parameter is **smoothed**, so setting or sweeping one glides rather
than clicking, and any of them can be **automated by a signal** (see below).

### Filter & EQ — shape the whole track

| Method | Effect |
| --- | --- |
| `.lpf(hz)` / `.hpf(hz)` / `.bpf(hz)` | the **performance filter** — low/high/band-pass on the whole track |
| `.resonance(q)` (alias `.res`) | filter resonance / Q (`~0.5..20`; high Q = whistle/acid) |
| `.eqLow(db)` / `.eqMid(db)` / `.eqHigh(db)` | 3-band EQ shelf/peak gains in dB (`0` = flat) |

This filter sits on the *summed* track (distinct from each synth voice's own filter), so it's the
one you *play*: `p3(note("c2*8").saw().lpf(sine.range(300,1400).slow(8)))` sweeps the **whole**
bassline over 8 cycles. Pair a low cutoff with high `.resonance` for the classic acid squelch.

### Character — grit & lo-fi

| Method | Effect |
| --- | --- |
| `.dist(drive)` | saturation / overdrive (tanh by default — smooth at low drive, gnarly high) |
| `.crush(bits)` | bit-depth reduction (`1`=fuzzy … `16`=clean) — grimy digital fuzz |
| `.downsample(factor)` | sample-rate decimation (lo-fi aliasing; `1` = off) |

The staples of techno/hip-hop drums: `p1(sound("bd sn").crush(8).dist(3))`.

### Modulation — movement & width

| Method | Effect |
| --- | --- |
| `.chorus(depth)` | LFO-modulated short delays + dry → ensemble width/shimmer |
| `.flanger(depth)` | a single short modulated delay **with feedback** → the comb-filter "whoosh" |
| `.phaser(depth)` | swept all-pass notches → swirling sweep, no pitch wobble |

`depth` is `0..1` (default ~0.5–0.6). Reach for chorus to thicken keys/pads, phaser for slow
evolving sweeps on a saw line.

### Dynamics & the pump

| Method | Effect |
| --- | --- |
| `.comp(ratio)` | a compressor (default threshold/attack/release) — glue and punch |
| `.sidechain("p1")` | **the techno pump** — duck this track whenever track `p1` (the kick) hits |

`.sidechain("p1")` is the headline move: the bass/pads dip on every kick and breathe back, the
genre's signature rhythmic motion. Put it on the bass and feed it the kick track:
`p3(note("c2*8").saw().sidechain("p1"))`.

### Sends — shared space

| Method | Effect |
| --- | --- |
| `.room(amount)` | feed this track into the shared **reverb** (`amount` = how much) |
| `.delay(amount)` | feed this track into the shared **ping-pong delay** |

Sends are shared, so many tracks bloom into *one* reverb tail (cheap and cohesive) rather than a
separate reverb each. `.room(0.3)` is a tasteful wash; push it for a cavern.

### Automating an effect (signals)

Any effect parameter takes a **signal**, and a continuous signal on a filter/effect param is sent
as a smooth **audio-rate ramp** so it glides between notes, not just at onsets:

```ts
p3( note("c2*8").saw().lpf( sine.range(300,1200).slow(8) ).sidechain("p1") ); // swept + pumping
p2( sound("hh*8").crush( saw.range(12,4).slow(4) ) );                         // grit increasing
```

> **Order & scope.** The chain runs in a fixed order (filter → EQ → distortion → bitcrush →
> modulation → compressor → sidechain), not the order you write the methods — a sensible default,
> not authoring order. Removing an effect from a live pattern doesn't switch it off this version;
> use `hush()` or keep it in the pattern. Per-effect fine controls (delay time, reverb size, LFO
> rate) use built-in defaults for now.

## Tracks & live control

| Function | Effect |
| --- | --- |
| `p1(pat)` … `p8(pat)` | assign a track; re-running swaps at the **next cycle boundary** |
| `mute(id)` / `unmute(id)` | hide / show a track (`mute("p2")`) |
| `solo(id)` / `unsolo(id)` | play only soloed tracks |
| `hush()` | silence everything (the panic button) |

**The live-coding loop.** Tracks are how you perform: keep the file running under
`pnpm demo:phase2`, and every save is a new "take." Build up by adding `pN(...)` lines, vary
by editing them, drop parts with `mute`, and reset with `hush()`. Because swaps land on the
cycle boundary, edits stay in time no matter when you hit save.

- **Deleting a `pN(...)` line and saving stops that track.** A track persists until you mute,
  overwrite, or remove it.
- **A syntax error keeps the last good version playing** while you fix it — you can't crash
  the sound out from a typo. Fix the error, save, and it picks up again.
- `solo` is non-destructive (it just hides the rest); `unsolo` brings everyone back.

---

## How it fits together

Mini-notation gives you *what and when*; `sound`/`note`/`freq` attach *how it sounds*;
combinators *reshape* it; signals *move* its parameters; tracks *layer* and *swap* it live.
Because every pattern is a pure function, all of these compose freely and hot-swapping is
seamless:

```ts
p1( sound("bd(3,8), ~ cp").gain(0.9) );                       // euclid kick + backbeat clap
p2( sound("hh*8?").gain(0.5).pan(sine.fast(4)) );             // thinned, auto-panned hats
p3( note("<c2 eb2> g2 bb1 f2").saw().lpf(900).every(2, x => x.fast(2)) );  // bass, doubled every other bar
```

### A few starting recipes

```ts
// Four-on-the-floor with an opening hat and a rolling fill every 4 bars
p1( sound("bd*4").gain(0.95) );
p2( sound("~ oh ~ oh").gain(0.5).every(4, x => x.fast(2)) );

// Acid line: sawtooth + resonant filter swept by a slow LFO
p3( note("c2*8").saw().resonance(0.6).lpf(rand.range(200, 1500).slow(2)) );

// Chord stab (a stacked triad) with a quieter eighth-note echo
p4( note("c3,eb3,g3").saw().adsr(0.005, 0.08, 0, 0.1).off(0.125, x => x.gain(0.5)) );
```

> **Not audible yet:** `.speed()` and sampling are **Phase 4**. Everything else here — including
> the full effects rack (`.lpf` sweeps, `.room`/`.delay` sends, the `.sidechain` pump) — plays
> today. See [roadmap.md](roadmap.md) for what each phase adds.
