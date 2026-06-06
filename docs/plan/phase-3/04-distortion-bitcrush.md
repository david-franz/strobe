# 04 — Distortion & bitcrush

## Purpose

The two **character / grit** effects that define a huge amount of the techno and hip-hop sound:

1. **Distortion / saturation** — non-linear **drive** via `tanh` / waveshaper / soft-clip, from
   gentle analog warmth (glue, harmonics, perceived loudness) to aggressive overdrive, with a
   **tone** control and **wet/dry**.
2. **Bitcrush** — **bit-depth reduction** (quantization → digital fuzz) and **sample-rate
   decimation** (downsampling → aliasing/“lo-fi” character), the staple of grimy hip-hop drums and
   harsh techno textures.

Both implement the [`Effect` trait](01-effects-framework.md#the-effect-trait) and are **chain
inserts** (they shape the track's own signal in place). The defining DSP risk for both is
**aliasing**, addressed below with oversampling.

## Location

```
engine/src/effects/distortion.rs   # Distort: tanh / waveshaper / soft-clip + tone (new)
engine/src/effects/bitcrush.rs     # Bitcrush: bit-depth + sample-rate decimation (new)
engine/src/graph.rs                # build graphs + oversampling wrapper (extended)
```

`EffectKind::Distort` / `EffectKind::Bitcrush`
([01-effects-framework.md](01-effects-framework.md#effect-kinds)).

## Design

### Distortion / saturation

A drive gain into a non-linear shaper, then a compensating output trim, with a tone filter and
wet/dry. Shapes:

| Shape | Function | Character |
| --- | --- | --- |
| `tanh` | `tanh(drive · x)` | smooth analog saturation, soft knee, odd harmonics |
| `softclip` | cubic soft clip `x − x³/3` (clamped) | warm overdrive, gentle |
| `hardclip` | `clamp(drive·x, −1, 1)` | harsh, lots of harmonics (use sparingly) |
| `wavefold` | `sin(drive·x)` / triangle fold | metallic, complex (FM-ish) |

```rust
#[derive(Clone, Copy, PartialEq)]
pub enum Shape { Tanh, SoftClip, HardClip, WaveFold }

pub struct Distort {
    shape: Shape,
    drive:    Smoothed<f32>,  // input gain into the non-linearity (1 .. ~50)
    tone_hz:  Smoothed<f32>,  // post-shaper one-pole tilt (LP/HP blend) for "tone"
    out_trim: Smoothed<f32>,  // makeup/attenuation to keep level sane as drive rises
    os: Oversampler<4>,       // pre-allocated 4× oversampling (anti-alias)
}
```

fundsp graph sketch (the non-linearity runs **inside** the oversampler):

```rust
// drive -> shaper -> tone tilt -> trim, all oversampled:
let core = (pass() * var(&drive)) >> shape_node() >> tone_tilt() * var(&out_trim);
// shape_node(): for tanh, fundsp `shape(Tanh)` / a map node x -> x.tanh();
//               soft/hard clip via `clip()` / a cubic map; wavefold via a sin map.
```

**Tone:** a simple post-shaper one-pole tilt (blend of LP and HP, or just an LP cutoff) tames the
extra high harmonics distortion generates — turn it down to dull, up to keep bite. **Out trim**
compensates the loudness increase so the wet/dry blend is musically usable and the limiter isn't
slammed.

### Bitcrush

Two independent degradations:

```rust
pub struct Bitcrush {
    bits:      Smoothed<f32>,  // 1 .. 16  (effective bit depth; quantization step = 2^-bits)
    downsample: Smoothed<f32>, // 1 .. 64  (sample-and-hold factor → effective SR / factor)
    hold_l: f32, hold_r: f32,  // last held sample
    phase: f32,                // fractional counter for the S&H
}
```

- **Bit-depth reduction:** quantize each sample to `2^bits` levels:
  `q = (2^bits); y = round(x · q) / q` (mid-tread). Fewer bits → coarser steps → audible
  quantization "fuzz". Smoothing `bits` lets you sweep the grit.
- **Sample-rate decimation:** a **sample-and-hold** that only updates the output every `downsample`
  input samples (fractional via a phase accumulator), holding the previous value between updates →
  the lo-fi, aliased downsampled sound.

```rust
for i in 0..n {
    self.phase += 1.0;
    if self.phase >= self.downsample.next() {       // time to grab a new sample
        self.phase -= self.downsample.cur();
        let q = (2.0f32).powf(self.bits.next());
        self.hold_l = (l[i] * q).round() / q;       // bit-crush + capture
        self.hold_r = (r[i] * q).round() / q;
    }
    l[i] = self.hold_l;                              // hold (decimate)
    r[i] = self.hold_r;
}
```

### Anti-aliasing (the key risk for both)

Any memoryless non-linearity (distortion) **generates harmonics above Nyquist** that fold back as
aliasing — inharmonic, unmusical artifacts, worst on high-pitched input. Bitcrush's S&H **deliberately**
aliases (that's the sound), but distortion should not unless asked.

- **Distortion: oversample.** Run the non-linearity at **4× (default; up to 8×)** the sample rate:
  upsample (polyphase FIR / fundsp `resample`), apply the shaper, low-pass below the original
  Nyquist, downsample. This pushes most generated harmonics above the folding point and filters them
  before decimation. A `clean`/`gritty` toggle lets the user *disable* oversampling for intentional
  aliased trash.
- **Bitcrush: aliasing is the feature**, so it is **not** oversampled. We do offer an optional
  post-crush low-pass (`tone`) to tame the harshest top end without removing the character, and we
  keep the decimation **after** any input-gain so levels stay predictable.
- **DC offset:** asymmetric shapers can introduce DC; a tiny output high-pass (~10 Hz) removes it so
  it doesn't eat headroom or thump the limiter.

### Parameter lists

**Distortion:**

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `shape` | build | tanh/soft/hard/fold | enum | tanh | |
| `drive` | `0x01` | 1 .. 50 | × | 1.0 | input gain into shaper |
| `tone` | `0x02` | 200 .. 20000 | Hz | 8000 | post-shaper tilt |
| `out_trim` | `0x03` | 0 .. 2 | × | auto | level compensation |
| `oversample` | build | 1/2/4/8 | × | 4 | 1 = intentional aliasing |
| `mix` | `0x00` | 0 .. 1 | — | 1.0 | wet/dry (chain) — parallel "NY"-style saturation |

**Bitcrush:**

| Param | `fx_param` | Range | Unit | Default | Notes |
| --- | --- | --- | --- | --- | --- |
| `bits` | `0x01` | 1 .. 16 | bits | 16 | 16 = transparent depth |
| `downsample` | `0x02` | 1 .. 64 | × | 1 | 1 = no decimation |
| `tone` | `0x03` | 200 .. 20000 | Hz | 20000 | optional post-crush LP |
| `mix` | `0x00` | 0 .. 1 | — | 1.0 | wet/dry |

## Interfaces

- **Event protocol** ([event-protocol.md](../../event-protocol.md)):
  - `TrackFx (0x30)` with `param = pack_fx(slot, fx_param)` sets any distortion/bitcrush param on its
    chain slot.
  - `ParamRamp (0x11)` automates drive/bits/downsample smoothly — e.g. a riser that increases drive
    over a bar, or a `.crush(rand.range(4,12))` per-note grit ([07](07-parameter-automation.md)).
- **TS chain API:**
  - `.dist(drive)` / `.drive(drive)` and `.shape("tanh"|"softclip"|"hardclip"|"fold")`,
    `.tone(hz)` — appends a `Distort` slot. `.sat(amount)` is a gentle preset (tanh, low drive,
    parallel mix).
  - `.crush(bits)` and `.downsample(factor)` (alias `.decimate()`) — appends a `Bitcrush` slot.
    `.crush(bits, factor)` sets both. Example (hip-hop drums): `p1(sound("bd sn").crush(8).dist(3))`.
- **Per-track chain vs send:** both are **inserts**. Order matters and is the authored order
  (`.dist().lpf()` tames the fizz after; `.lpf().dist()` drives a pre-filtered signal) — preserved by
  the chain slot index ([01](01-effects-framework.md)).

## Edge cases & risks

- **Aliasing (distortion):** the headline risk — mitigated by **4×–8× oversampling** around the
  non-linearity. Acceptance includes a high-frequency sine test asserting low inharmonic energy with
  oversampling on, and (by design) high with it off.
- **Aliasing (bitcrush):** intentional; documented as the effect's character, with an optional tone
  LP to tame extremes.
- **Loudness explosion:** drive raises level a lot; `out_trim` auto-compensates and the master
  limiter is the net, but the wet/dry `mix` is the musical control (parallel saturation keeps
  transients).
- **DC offset:** asymmetric/wavefold shapers add DC; output high-pass (~10 Hz) removes it.
- **Denormals:** memoryless shapers don't accumulate denormals; the optional tone filter is covered
  by the global FTZ flag.
- **Real-time safety:** oversampling FIR buffers and the S&H state are **pre-allocated** off-thread;
  `process` does no allocation. `bits`/`drive`/`downsample` changes are smoother target updates.
- **CPU:** oversampling is the cost — 4× FIR up/down per sample. Bounded and off by default for
  bitcrush; distortion's oversample factor is selectable so a cheap mode exists.

## Acceptance criteria

- [ ] `tanh` saturation adds harmonics and compresses peaks without hard edges. **Test:** sine in,
      FFT shows added odd harmonics; peak level compressed vs input gain.
- [ ] With **oversampling on**, a high-frequency sine (e.g. 9 kHz at 48 k) driven hard shows
      **minimal inharmonic/aliased energy**; with oversampling off, aliasing appears. **Test:** A/B
      FFT, assert aliased-bin energy drops ≥ ~20 dB with OS on.
- [ ] `drive` at 1.0 with `tanh` is near-transparent at low level (gentle saturation only). **Test:**
      low-level null-ish check; assert tiny THD at unity drive.
- [ ] Bitcrush `bits=4` produces audible quantization steps; `bits=16` is transparent. **Test:**
      ramp in, assert staircase quantization at 4 bits, null at 16.
- [ ] `downsample=8` decimates (sample-and-hold) producing the lo-fi/aliased sound at SR/8 effective
      rate. **Test:** assert the output is piecewise-constant over 8-sample groups; spectral aliasing
      present.
- [ ] Sweeping `drive` or `bits` via a signal is **click-free**. **Test:** ramp the param, assert no
      step transient above −60 dB.
- [ ] No DC offset on the output of an asymmetric/wavefold setting. **Test:** assert mean ≈ 0 after
      the output high-pass.
- [ ] No underruns with distortion (4× OS) + bitcrush on a track under the example load. **Test:**
      `stats()` clean.
