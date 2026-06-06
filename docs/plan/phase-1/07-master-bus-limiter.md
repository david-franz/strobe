# 07 — Master bus + limiter

## Purpose

Once Phase 1 can play 16+ voices and a full drum kit at once, their sum will routinely exceed
0 dBFS and **clip** at the `cpal` output — a harsh digital crackle. This feature is the
**master bus**: it sums every track/voice into one stereo signal, applies a **master gain**,
and runs a **transparent, look-ahead brick-wall limiter** so the final output can never clip,
while staying as clean as possible. It is the last stage before `cpal`, sitting exactly where
the [architecture diagram](../../architecture.md#the-rust-engine-internals) shows "master bus
+ limiter → cpal output".

## Location

```
engine/src/effects.rs   # MasterBus + Limiter (look-ahead brick-wall) (extended)
engine/src/audio_io.rs  # render loop: sum voices → master bus → cpal callback (extended)
```

Consumes the summed output of [06-voice-allocation.md](06-voice-allocation.md) (all voices)
and feeds the `cpal` stream Phase 0 already owns.

## Design

### Where it sits in the render loop

```
per buffer:
  zero the master accumulator (L/R)
  for each non-Free voice slot (06):
      voice.render(&mut tmp_l, &mut tmp_r)         # add voice into temp
      master_l += tmp_l;  master_r += tmp_r        # sum
  master_l *= master_gain;  master_r *= master_gain
  limiter.process(&mut master_l, &mut master_r)    # look-ahead brick-wall (this feature)
  write (master_l, master_r) → cpal output buffer
```

> In Phase 1 there is effectively one master bus; **per-track effect chains** and **send
> buses** are [Phase 3](../../roadmap.md#phase-3--effects-rack). The master bus is built now so
> that the per-track stage slots cleanly above it later (voices → [future per-track FX] →
> master bus → limiter).

### Headroom strategy

- Voices render at a conservative per-voice level (amp env peak ≤ ~0.8, velocity-scaled), and
  the bus runs with **internal headroom**: we aim for a nominal sum around −6 dBFS so transient
  peaks have room before the limiter engages.
- `master_gain` (default ~0.8) is applied **before** the limiter so the user's master fader and
  the limiter interact predictably.
- The limiter's **ceiling** sits just below 0 dBFS (default **−0.3 dBFS**) so inter-sample
  peaks don't reconstruct above full scale on conversion.
- A gentle **DC/sub high-pass** (~25 Hz, one-pole) on the master removes inaudible sub-bass
  (e.g. from the kick, [05](05-drum-synthesis.md)) that would otherwise eat headroom.

### The look-ahead brick-wall limiter

A look-ahead limiter delays the audio by a few milliseconds so it can *see* an upcoming peak
and begin reducing gain **before** the peak arrives — eliminating the overshoot a naive
limiter would clip. fundsp provides a `limiter` building block (an RMS/peak look-ahead
limiter with attack/release); we wrap it with our ceiling and a fixed look-ahead.

```rust
use fundsp::hacker::*;

/// Stereo look-ahead brick-wall limiter at `ceiling` (linear, e.g. 10^(-0.3/20)).
/// `attack`/`release` are the limiter time constants in seconds; look-ahead ≈ attack.
pub fn build_master_limiter(ceiling: f32, attack: f32, release: f32) -> Box<dyn AudioUnit> {
    // fundsp's limiter((attack, release)) gain-reduces to keep |x| <= 1; we scale around it
    // to hit our sub-unity ceiling and keep stereo linked (same gain on L & R).
    //   in (L|R) -> [normalize by 1/ceiling] -> limiter -> [* ceiling] -> out (L|R)
    let lim = (multi::<U2>() * constant(1.0 / ceiling))
            >> limiter_stereo((attack, release))      // stereo-linked look-ahead limiter
            >> (multi::<U2>() * constant(ceiling));
    Box::new(lim)
}
```

Design points:

- **Look-ahead:** a small fixed delay (default **2–5 ms**) lets the detector pre-empt
  transients. This adds a few ms of latency — acceptable on the master, and tiny next to the
  ~100 ms scheduler lookahead, so it does not change the live feel.
- **Stereo-linked:** L and R share one gain-reduction value so the stereo image doesn't shift
  when one channel peaks (a centered kick stays centered).
- **Attack ≈ instantaneous** (sub-ms, matched to look-ahead so reduction is in place by the
  time the peak arrives); **release** smooth (default ~50–100 ms) to avoid pumping/distortion
  on sustained loud passages.
- **Transparency:** the goal is a *safety* brick wall, not a loudness maximizer — with the
  −6 dBFS nominal headroom the limiter mostly idles and only catches occasional peaks, so it
  stays inaudible. It is not a creative compressor (that's [Phase 3](../../roadmap.md#phase-3--effects-rack)).

### Parameter list & ranges

| Param | Range | Default | Live-settable | Notes |
| --- | --- | --- | --- | --- |
| `master_gain` | 0 .. 2 | 0.8 | yes (`ParamSet`, track 0) | pre-limiter master fader |
| `ceiling_db` | −12 .. 0 | −0.3 | yes | brick-wall output ceiling |
| `lookahead_ms` | 0 .. 10 | 3 | per-stream (set at start) | transient pre-emption |
| `release_ms` | 10 .. 500 | 80 | yes | limiter recovery |
| `dc_hp_hz` | 0 .. 40 | 25 | per-stream | sub/DC headroom guard |

### Master params via the event protocol

Master/global parameters use **`track = 0`** (the protocol reserves "0 = master/global") with
`ParamSet` (`0x10`). So `setGain(0.7)` on the master compiles to a `ParamSet` on track 0 with a
master-gain param id.

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `ParamSet 0x10` with `track = 0` sets `master_gain`, `ceiling_db`, `release_ms`
    (master/global param ids).
  - All voice output that reaches the bus originates from `NoteOn`/`NoteOff`-driven voices
    ([06](06-voice-allocation.md)); the limiter sees only the summed signal.
- **Voice allocation:** the bus sums whatever [06](06-voice-allocation.md) renders; no coupling
  beyond "sum then limit."
- **Audio I/O:** the limiter runs inside the `cpal` callback's render path Phase 0 established,
  as the final stage before writing the output buffer.
- **Song API:** `setGain(x)` / a future `master.limiter({...})` map to track-0 `ParamSet`s; in
  Phase 1 the defaults are good enough that no song code is required to avoid clipping.

## Edge cases & risks

- **Clipping / overs:** the entire point. With the −0.3 dBFS ceiling and look-ahead the output
  is provably ≤ ceiling; acceptance verifies no sample exceeds it under a deliberately hot mix.
- **Real-time safety:** the limiter is a fixed fundsp graph + a small fixed look-ahead delay
  line, all **pre-allocated**; no allocation in the callback. Master `ParamSet`s update atomics,
  not structure.
- **Denormals:** the limiter's gain-reduction envelope can produce denormals on a long fade to
  silence; the global FTZ flag set at stream start ([02-filters.md](02-filters.md)) covers it.
- **Pumping / distortion:** avoided by adequate headroom (limiter mostly idle), stereo-linked
  gain, and a smooth release; an over-short release would audibly pump.
- **Latency:** look-ahead adds a few ms; bounded to ≤10 ms and dwarfed by scheduler lookahead,
  so the live feel is unchanged.
- **Inter-sample peaks:** sub-0 dBFS ceiling gives margin; true-peak/oversampled detection is a
  later refinement, not required for the Phase 1 safety bar.
- **CPU:** one stereo limiter + a one-pole DC high-pass on the master = negligible vs. the
  voice stack.

## Acceptance criteria

- [ ] With a deliberately hot mix (16+ loud voices summing well over 0 dBFS), the output **never
      exceeds the ceiling** (−0.3 dBFS) and has **no hard-clip artifacts**. **Test:** render the
      hot mix, assert `max(|sample|) <= ceiling`; FFT shows no clipping-induced broadband
      harmonics.
- [ ] The limiter is **transparent** on a normal-level mix (mostly idle, no audible pumping).
      **Test:** A/B a moderate mix with limiter bypassed vs engaged; assert near-identical
      spectra / minimal gain reduction.
- [ ] **Stereo-linked:** a hard centered kick stays centered through limiting (no image shift).
      **Test:** mono-summed correlation stays ~1.0 through a limited transient.
- [ ] `master_gain` via a **track-0 `ParamSet`** scales the output and interacts correctly with
      the limiter. **Test:** set gain 0.8→0.4, assert ~6 dB drop pre-limit, ceiling still
      enforced.
- [ ] The 25 Hz DC/sub high-pass removes sub-25 Hz energy without thinning the audible kick.
      **Test:** FFT a kick-only render; assert rolloff below 25 Hz, body intact.
- [ ] **No underruns** with the limiter in the render path under the 16-voice load. **Test:**
      `stats()` clean during the Phase 1 example.
- [ ] **No allocation** on the audio thread in the master/limiter stage. **Test:** alloc-counter
      stress check.
