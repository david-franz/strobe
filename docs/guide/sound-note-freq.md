[« strobe user guide](../guide.md)

# sound / note / freq

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

## Modifiers (chain after a constructor)

Each returns a *new* pattern with one key set (patterns are immutable — chaining never
mutates). Values may be plain numbers, mini-notation strings, full patterns, or signals.

| Method | Sets | Range / note |
| --- | --- | --- |
| `.gain(x)` | level → note velocity | `0..1` (clamped) |
| `.pan(x)` | stereo position | `0` left … `1` right (`0.5` center) |
| `.adsr(a,d,s,r)` | amp envelope | attack/decay/release in **seconds**, sustain `0..1` |
| `.saw()/.square()/.sine()/.tri()` | synth waveform | shortcut that sets the `s` key |
| `.note(n)` | (re)tune — e.g. pitch a kick | semitone or note name |
| `.speed(x)` | sample playback speed/pitch (varispeed) | sampler — see [Sampling](sampling.md) |

The **filter, EQ, and the rest of the effects** (`.lpf`, `.room`, `.sidechain`, …) are track-level
and have their own chapter: [Effects](effects.md). The **sampler-specific** modifiers
(`.chop`, `.slice`, `.stretch`, `.pitch`, `.grain`, …) live in [Sampling](sampling.md).

```ts
note("c2 c2 eb2 g2").saw().lpf(800).resonance(0.4).adsr(0.01,0.1,0.6,0.2).gain(0.85).pan(0.4)
```

**Using modifiers effectively:**

- **`gain` is per-hit dynamics, not a master volume.** Pattern it for groove:
  `.gain("0.9 0.6 0.7 0.6")` accents the downbeat. The master limiter (see architecture)
  catches the sum, so you rarely have to think about clipping.
- **`lpf`/`hpf` shape timbre more than any single thing.** A static `lpf(800)` darkens a saw
  bass; a *moving* `lpf` (a signal — see [Signals](signals.md)) is the classic filter sweep. Pair a low
  cutoff with high `resonance` for an acid/squelch character.
- **`adsr` is the difference between a pluck and a pad.** Short attack + short decay + low
  sustain = percussive; long attack + high sustain = a swell. Drums use a fixed one-shot
  envelope and ignore note-off, so `adsr` mostly matters for `note`/`freq` synth voices.

---

Prev: [« Mini-notation](mini-notation.md) · Next: [Combinators »](combinators.md)
