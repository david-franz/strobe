[« strobe user guide](../guide.md)

# Signals (continuous patterns)

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
- For **filter cutoff** (`lpf`/`hpf`) and other effect params, strobe sends the signal as a
  **smooth ramp** to the engine, so the value actually glides continuously between onsets —
  that's why the classic `lpf(sine.range(300,1200).slow(8))` sweep sounds liquid, not stepped.

```ts
note("c2*8").lpf( sine.range(300,1200).slow(8) )   // a smooth 8-cycle filter sweep
note("c2*8").pan( saw.range(0,1).fast(2) )         // step the pan per note
note("c2*8").gain( "0.6 0.9" )                      // a mini-string also works (per note)
```

Reach for `slow(n)` to make a sweep span a whole phrase; combine signals with `.range` to aim
them at a parameter's useful span (a filter likes hundreds-to-thousands of Hz, not `0..1`).

---

Prev: [« Combinators](combinators.md) · Next: [Effects »](effects.md)
