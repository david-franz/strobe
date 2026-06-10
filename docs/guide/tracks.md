[« strobe user guide](../guide.md)

# Tracks & live control

| Function | Effect |
| --- | --- |
| `p1(pat)` … `p8(pat)` | assign a track; re-running swaps at the **next cycle boundary** |
| `mute(id)` / `unmute(id)` | hide / show a track (`mute("p2")`) |
| `solo(id)` / `unsolo(id)` | play only soloed tracks |
| `hush()` | silence everything (the panic button) |

**The live-coding loop.** Tracks are how you perform: keep the file running under
`pnpm demo:phase4` (or any phase demo), and every save is a new "take." Build up by adding
`pN(...)` lines, vary by editing them, drop parts with `mute`, and reset with `hush()`. Because
swaps land on the cycle boundary, edits stay in time no matter when you hit save.

- **Deleting a `pN(...)` line and saving stops that track.** A track persists until you mute,
  overwrite, or remove it.
- **A syntax error keeps the last good version playing** while you fix it — you can't crash
  the sound out from a typo. Fix the error, save, and it picks up again.
- `solo` is non-destructive (it just hides the rest); `unsolo` brings everyone back.
- **Live display.** Running a demo in a terminal shows a real-time **oscilloscope** of the master
  output (a faithful trace of the actual samples) above a stats line (frame / underruns / late /
  ring). Piping the output (no TTY) falls back to just the stats line.

## How it fits together

Mini-notation gives you *what and when*; `sound`/`note`/`freq` attach *how it sounds*;
combinators *reshape* it; signals *move* its parameters; samples bring *recorded sound* into the
same model; tracks *layer* and *swap* it live. Because every pattern is a pure function, all of
these compose freely and hot-swapping is seamless:

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

// Chop a break, re-sequence it, and flip it every 4th bar
p5( sample("amen").slice(8, "0 5 2 7 4 1 6 3").every(4, x => x.rev()).gain(0.7) );
```

Everything in this guide — the full effects rack and the Phase 4 sampler — plays today. See
[roadmap.md](../roadmap.md) for what each phase adds next (live-coding DX, then the web backend).

---

Prev: [« Sampling](sampling.md) · Back to the [guide index](../guide.md)
