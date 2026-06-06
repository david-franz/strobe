# Phase 1 — Synthesis core (overview)

> **Goal:** make the engine *genuinely musical* — the "base frequency" synth and the
> "base drum click" the whole project is built around. Phase 0 proved the spine could
> drive a single kick with no glitches; Phase 1 widens the **voice** layer of that spine
> into a real instrument: oscillators, a filter, envelopes, a configurable synth voice, a
> synthesized drum kit, real polyphony with voice stealing, and a master limiter that
> keeps it all from clipping.

This document is the index for the Phase 1 plan. Each feature has its own file with the
full **Purpose / Location / Design / Interfaces / Edge cases & risks / Acceptance
criteria** breakdown.

## Where this sits

Phase 1 is the second phase of the [roadmap](../../roadmap.md#phase-1--synthesis-core).
It builds directly on **Phase 0's proven spine** and changes nothing above the bridge that
Phase 0 already settled:

- The [two-plane architecture](../../architecture.md#the-core-principle-two-planes) (TS
  control plane / Rust audio plane) is fixed. **No allocation, no locks, and no JS run on
  the audio thread** — every data structure introduced here is pre-allocated and lives
  inside the engine.
- The [event protocol](../../event-protocol.md) and its **32-byte fixed record** are
  frozen. Phase 1 implements the opcodes Phase 0 stubbed: `NoteOn` (`0x01`),
  `NoteOff` (`0x02`), and `ParamSet` (`0x10`). It does *not* invent new opcodes.
- The existing spine — `audio_io.rs` (the `cpal` stream + render loop), the **SPSC ring
  buffer**, the `frame`-stamped event consumer in `scheduler.rs`, and the **napi-rs**
  bridge — is reused as-is. Phase 1 plugs richer voices into the render loop those
  components already drive.
- Time stays [cycle-based](../../architecture.md#the-clock-and-musical-time) with `cps`
  tempo; the look-ahead scheduler is unchanged. Phase 1 is almost entirely *below* the
  scheduler.

Everything synthesized in Phase 1 is a [`fundsp`](https://github.com/SamiPerttu/fundsp)
graph, composed with fundsp's operators: `>>` pipe, `|` stack, `&` bus, `^` branch. We do
not hand-write biquads or osc loops where fundsp gives us a typed, zero-cost building
block.

## What Phase 1 delivers (the feature files)

| # | Feature | File | One-line summary |
| --- | --- | --- | --- |
| 01 | Oscillator bank | [01-oscillators.md](01-oscillators.md) | sine / saw / square (PWM) / triangle / noise / wavetable generators, band-limited, frequency from `NoteOn.arg0` |
| 02 | State-variable filter | [02-filters.md](02-filters.md) | LP/HP/BP/notch SVF with resonance/Q, smoothed cutoff (no zipper noise) |
| 03 | ADSR envelopes | [03-envelopes.md](03-envelopes.md) | amp + filter ADSR, gate from NoteOn/NoteOff, one-shot mode driven by `NoteOn.arg2` duration |
| 04 | Synth voice | [04-synth-voice.md](04-synth-voice.md) | full `osc → SVF → amp` voice wiring 01–03 together; the `note(...).saw().lpf().adsr()` API and `defineInstrument()` mapping |
| 05 | Drum synthesis | [05-drum-synthesis.md](05-drum-synthesis.md) | synthesized kick / snare / clap / hats; maps to `sound("bd sn hh cp")` |
| 06 | Voice allocation | [06-voice-allocation.md](06-voice-allocation.md) | fixed voice pool, polyphony, oldest/quietest voice stealing, handle→physical-voice mapping for NoteOff |
| 07 | Master bus + limiter | [07-master-bus-limiter.md](07-master-bus-limiter.md) | sum all tracks/voices → look-ahead brick-wall limiter → master gain → `cpal` |

### How they fit together

```
NoteOn ──► voice_alloc (06) ──► synth voice (04)            drum voice (05)
                                  │  osc (01)                  │  kick/snare/hat/clap
                                  │   >> SVF (02)              │   = fundsp graphs
                                  │   * amp/filter ADSR (03)   │
                                  └──────────────┬────────────┘
                                                 ▼
                                       per-track sum (mixer)
                                                 ▼
                                    master bus + limiter (07)
                                                 ▼
                                          cpal output
```

This is the [engine-internals diagram](../../architecture.md#the-rust-engine-internals)
from the architecture doc, filled in for the voice layer.

## Files touched (engine-wide map)

All paths under `engine/src/`. Phase 0 already created `audio_io.rs`, `scheduler.rs`, and
the ring buffer; Phase 1 adds the voice machinery.

```
engine/src/
  voices/
    mod.rs          # Voice trait + dispatch (new)
    oscillator.rs   # feature 01 (new)
    synth.rs        # feature 04 — uses oscillator/filter/env (new)
    drum.rs         # feature 05 (new)
  filter.rs         # feature 02 — SVF wrapper + cutoff smoothing (new)
  envelope.rs       # feature 03 — ADSR AudioUnit (new)
  voice_alloc.rs    # feature 06 — pool + stealing (new)
  graph.rs          # build fundsp graphs from InstrumentConfig (extended)
  effects.rs        # feature 07 — master limiter lives here for now (extended)
  audio_io.rs       # render loop calls voice_alloc + master limiter (extended, Phase 0)
  scheduler.rs      # dispatch NoteOn/NoteOff/ParamSet to voice_alloc (extended, Phase 0)
```

TS side (`packages/strobe/`) gains the chainable synth/drum modifiers and the
`defineInstrument()` payload shape; the bulk of the pattern/chaining API lands in
[Phase 2](../../roadmap.md#phase-2--pattern-language), so Phase 1 ships only the thin slice
needed to drive these voices.

## Phase 1 "done when" (restated from the [roadmap](../../roadmap.md#phase-1--synthesis-core))

- `note("c2 e2 g2").saw().lpf(...)` plays **correct pitches** with audible filter/envelope
  shaping.
- A synthesized drum kit (**bd / sn / hh / cp**) sounds **usable for a real beat**.
- **16+ simultaneous voices** play cleanly with **no underruns**.

Plus the [cross-cutting definition of done](../../roadmap.md#cross-cutting-definition-of-done-every-phase):
a runnable `examples/` song exercises the kit, `stats()` is clean under that load, pure-TS
logic has unit tests, DSP has golden/null checks where practical, and it is validated on
macOS first (Linux/Windows via CI).
