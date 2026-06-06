# 04 — The synth voice

## Purpose

This is the "base frequency" instrument assembled: a complete, configurable **synth voice**
that wires together the oscillator (01), the state-variable filter (02), and the ADSR
envelopes (03) into the classic subtractive chain **oscillator → filter → amp**, with an
**amp envelope** and a **filter envelope**, plus per-voice **gain** and **pan**. It is the
fundsp graph a `NoteOn` instantiates and triggers, and it is what the song API line
`note("c2 e2").saw().lpf(800).resonance(0.4).adsr(...)` ultimately drives.

## Location

```
engine/src/voices/synth.rs   # SynthVoice: the graph + trigger/release/render (new)
engine/src/graph.rs          # build_synth_voice(InstrumentConfig) → fundsp graph (extended)
engine/src/voices/mod.rs     # Voice trait impl for SynthVoice (new)
packages/tekno/src/synth.ts  # chainable modifiers + InstrumentConfig shape (new)
```

Instantiated/pooled by the voice allocator ([06-voice-allocation.md](06-voice-allocation.md));
summed into the master bus ([07-master-bus-limiter.md](07-master-bus-limiter.md)).

## Design

### The signal chain as a fundsp graph

```
                          ┌─────────────── filter env (03) ───────────────┐
                          │                                               ▼
   freq(arg0) ─► osc (01) ─► SVF (02, cutoff = base + env_amount*filt_env) ─► × ─► pan ─► L/R
                                                                            ▲
                          amp env (03) × velocity(arg1) × gain ─────────────┘
```

In fundsp operators (`>>` pipe, `|` stack, `*` ring-mod/amplitude, `+` sum):

```rust
use fundsp::hacker::*;

/// Built once per physical voice slot (pre-allocated pool). On NoteOn we set the
/// frequency/gate, not rebuild the graph, so steady-state retriggers don't allocate.
pub fn build_synth_voice(cfg: &InstrumentConfig, sr: f32) -> Box<dyn AudioUnit> {
    // --- oscillator (01) ---------------------------------------------------
    let osc = build_osc(cfg.osc, /*freq set via shared var*/ cfg.osc_freq_var.clone());

    // --- filter cutoff control (02 + 03) -----------------------------------
    // base cutoff smoothed (de-zippered) + filter-envelope * depth
    let base    = var(&cfg.cutoff_var) >> follow(cfg.svf.smooth_ms * 0.001);
    let fenv    = An(EnvNode::new(cfg.filter_env));            // 0-in/1-out control
    let cutoff  = base + fenv * constant(cfg.svf.env_amount_hz);

    // --- filter (02) -------------------------------------------------------
    let filtered = osc >> (pass() | cutoff | var(&cfg.q_var)) >> svf_node(cfg.svf.mode);

    // --- amp env (03) + velocity + gain ------------------------------------
    let aenv  = An(EnvNode::new(cfg.amp_env));                 // 0-in/1-out control
    let amped = filtered * (aenv * var(&cfg.vel_var) * var(&cfg.gain_var));

    // --- pan (constant-power) → stereo -------------------------------------
    Box::new(amped >> pan(cfg.pan))     // fundsp pan(): mono → stereo, constant power
}
```

Key real-time detail: pitch, cutoff, Q, velocity, gain are wired through fundsp **shared
variables** (`var()` / `An(Var…)`), so `NoteOn`/`ParamSet` update an atomic value rather than
rebuilding the graph. The graph itself is built once when the voice pool is created.

### `SynthVoice` and the `Voice` trait

```rust
pub trait Voice {
    fn trigger(&mut self, freq: f32, vel: f32, dur_frames: u64);  // from NoteOn
    fn release(&mut self);                                        // from NoteOff
    fn is_finished(&self) -> bool;                                // amp env tail done
    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]);   // add into buffers
    fn set_param(&mut self, param: ParamId, value: f32);          // from ParamSet
}

pub struct SynthVoice {
    graph: Box<dyn AudioUnit>,        // built once (pool)
    freq_var: Shared, cutoff_var: Shared, q_var: Shared,
    vel_var: Shared, gain_var: Shared,
    amp_env: *mut Adsr, filter_env: *mut Adsr,  // handles into the graph's env nodes
    cfg: InstrumentConfig,
}

impl Voice for SynthVoice {
    fn trigger(&mut self, freq: f32, vel: f32, dur_frames: u64) {
        self.freq_var.set(freq.clamp(20.0, self.cfg.nyquist));
        self.vel_var.set(vel);
        if self.cfg.osc.phase_reset { self.graph.reset(); }     // 01: phase reset
        unsafe { (*self.amp_env).trigger(dur_frames);
                 (*self.filter_env).trigger(dur_frames); }
    }
    fn release(&mut self) {
        unsafe { (*self.amp_env).gate_off(); (*self.filter_env).gate_off(); }
    }
    fn is_finished(&self) -> bool { unsafe { (*self.amp_env).is_finished() } }
    /* render(): pull a block from `graph`, accumulate into out_l/out_r */
}
```

### `InstrumentConfig` (the `defineInstrument()` payload)

Sent once over the **control-call channel** (not the hot event path), per
[event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time). Returns an
instrument id that subsequent `NoteOn`s reference in **`arg3`**.

```ts
// packages/tekno/src/synth.ts
export interface InstrumentConfig {
  type: "synth";
  osc: { kind: "sine"|"saw"|"square"|"triangle"|"noise"|"wavetable";
         pulseWidth?: number; wavetable?: string; phaseReset?: boolean };
  filter: { mode: "lp"|"hp"|"bp"|"notch"; cutoff: number; q: number;
            smoothMs?: number; envAmount?: number };
  ampEnv:    { attack: number; decay: number; sustain: number; release: number };
  filterEnv: { attack: number; decay: number; sustain: number; release: number };
  gain?: number;   // 0..1, default 0.8
  pan?:  number;   // 0..1, 0.5 = center, default 0.5
}
```

### The TS song API mapping

The chain in the prompt maps element-by-element onto `InstrumentConfig` + per-note event
fields:

```ts
note("c2 e2")              // → NoteOn per hap; arg0 = freq(Hz) resolved from note name
  .saw()                   // → osc.kind = "saw"
  .lpf(800)                // → filter.mode="lp", filter.cutoff=800
  .resonance(0.4)          // → filter.q  (0.4 mapped onto the Q range; alias .res())
  .adsr(0.01, 0.1, 0.6, 0.2) // → ampEnv {a,d,s,r}
```

- Methods that configure the **instrument's timbre** (`.saw()`, base `.lpf()`, `.resonance()`,
  `.adsr()`, `.fenv()`, `.pan()`, default `.gain()`) are collected into one `InstrumentConfig`
  and registered with `defineInstrument()`. The returned instrument id is packed into every
  `NoteOn.arg3` for that pattern.
- Methods that vary **per note or over time** (`.gain("0.6 0.9")`, `.lpf(sine.range(...))`)
  compile to per-note packing or to `ParamSet`/`ParamRamp` events emitted alongside the note,
  exactly as the protocol's note describes ("the scheduler decides per parameter whether it is
  per-note (packed) or per-track (separate event)"). The bulk of this signal-as-parameter
  machinery is [Phase 2](../../roadmap.md#phase-2--pattern-language); Phase 1 ships fixed
  numeric params + the `defineInstrument()` round-trip.

### Param id namespace (for `ParamSet`)

A small fixed enum, shared with [02](02-filters.md)/[03](03-envelopes.md):

```rust
#[repr(u32)]
pub enum ParamId {
    Cutoff = 0, Resonance = 1, AmpAttack = 2, AmpDecay = 3, AmpSustain = 4,
    AmpRelease = 5, FiltAttack = 6, FiltDecay = 7, FiltSustain = 8, FiltRelease = 9,
    FiltEnvAmount = 10, PulseWidth = 11, Gain = 12, Pan = 13, // …drum params continue (05)
}
```

### How a `NoteOn` instantiates / triggers a voice

1. `scheduler.rs` drains a `NoteOn` (`0x01`) at its `frame`. It reads `arg3` (instrument id),
   `arg0` (freq), `arg1` (velocity), `arg2` (duration frames), and `voice` (the TS handle).
2. It asks `voice_alloc` ([06](06-voice-allocation.md)) for a physical voice for that
   instrument, stealing if the pool is full, and records `handle → physical voice`.
3. It calls `voice.trigger(freq, vel, dur_frames)` — which sets the shared vars, optionally
   resets osc phase, and triggers both envelopes (gated or one-shot per the instrument).
4. The render loop sums the voice into the master bus until `is_finished()`; a gated voice
   waits for its `NoteOff` (`0x02`), matched back through the handle map even if it was
   stolen.

## Interfaces

- **Event protocol:** `NoteOn 0x01` (arg0 freq, arg1 vel, arg2 dur-frames, arg3 instrument
  id, `voice`=handle); `NoteOff 0x02` (releases by voice id); `ParamSet 0x10` (live edits via
  `ParamId`). Instrument registered through `defineInstrument()` control call.
- **Voice allocation:** [06-voice-allocation.md](06-voice-allocation.md) owns the pool and the
  handle→voice map.
- **Sub-features:** oscillator [01](01-oscillators.md), filter [02](02-filters.md), envelopes
  [03](03-envelopes.md).
- **Master bus:** voice output is summed (post-pan, stereo) into the bus in
  [07-master-bus-limiter.md](07-master-bus-limiter.md).

## Edge cases & risks

- **Real-time safety:** graphs are built at pool-creation time; `trigger`/`release`/`set_param`
  only set atomics or flip env stages — **no allocation on the hot path**. The raw pointers to
  the env nodes are into the voice's own owned graph (stable for the voice's lifetime).
- **Zipper noise / clicks:** cutoff smoothing (02) + zero-start amp attack and epsilon release
  (03); pan changes via `ParamSet` should also be smoothed (route pan through a `follow()` if
  it's automated).
- **Pitch correctness:** freq is resolved in TS; engine only clamps to Nyquist. A unit test on
  the TS side asserts `note("c2")` → 65.41 Hz etc.
- **Denormals:** FTZ flag set at stream start (02); filter/env tails snap to zero.
- **CPU:** target is **16+ simultaneous voices** with no underruns (the roadmap's bar). Budget
  per synth voice ≈ band-limited osc + SVF + 2 envelopes + pan ≈ a few µs; 16 voices fit
  comfortably in a 5.3 ms buffer at 48 kHz/256.

## Acceptance criteria

- [ ] `note("c2 e2 g2").saw().lpf(800).resonance(0.4).adsr(0.01,0.1,0.6,0.2)` plays the
      **correct pitches** (65.41 / 82.41 / 98.00 Hz) with audible low-pass + envelope shaping.
      **Test:** render the pattern, FFT each note's fundamental; assert the filter rolloff and
      the ADSR amplitude contour.
- [ ] `defineInstrument()` round-trips: a config in → an instrument id out → `NoteOn.arg3`
      selects that timbre. **Test:** define two instruments, trigger each, assert distinct
      spectra.
- [ ] Gated note sustains until `NoteOff`; the voice frees only after the release tail.
      **Test:** NoteOn, hold, NoteOff; assert sustain plateau then release-to-silence.
- [ ] A live `ParamSet` (e.g. cutoff 400→2000) glides without zipper noise on a held note.
      **Test:** held note + ParamSet, assert smooth spectral change.
- [ ] **16 simultaneous** synth voices render with **zero underruns** and zero late events
      (`stats()` clean). **Test:** the Phase 1 example triggers a 16-note chord/arp under the
      stress harness.
- [ ] Filter envelope opens the cutoff on attack when `.fenv(...)` is set. **Test:** assert the
      −3 dB point rises during attack.
