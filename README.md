# strobe

**Write techno and hip-hop as code — live, with no compile step.**

`strobe` is a programmable music library. You start from a raw frequency or a drum click, sculpt it with every effect a producer expects, pull in short samples (including from YouTube), stretch and chop them, and layer loops by changing patterns and effects *while the sound is playing*. It is aimed at software engineers who want more flexibility and speed than a traditional DAW gives you.

```ts
import { setcps, p1, p2, p3, sound, note, sine } from "strobe";

setcps(130 / 60 / 4);                       // ~130 BPM, 4 beats per cycle

p1( sound("bd*4").gain(0.9) );              // four-on-the-floor kick
p2( sound("~ hh ~ hh").lpf(6000) );         // offbeat hats
p3( note("c2 c2 eb2 g2").saw()              // rolling bassline...
      .lpf(sine.range(300, 1200).slow(8))   // ...with an 8-cycle filter sweep
      .room(0.3)
      .sidechain("p1") );                   // ducked by the kick for that techno pump
```

Edit any line, save, and the change takes effect at the next cycle boundary — **no clicks, no dropouts, no restart.**

---

## Why strobe

This is the *live coding* / algorithmic-music space — the world of [TidalCycles](https://tidalcycles.org/), [Strudel](https://strudel.cc/), [SuperCollider](https://supercollider.github.io/), and [Sonic Pi](https://sonic-pi.net/). `strobe` borrows the proven ideas from that world and packages them for engineers:

- **A language you already know.** Songs are TypeScript. Interpreted, hot-reloadable, with full editor tooling and type checking.
- **A pattern model that scales.** Declarative patterns and a concise mini-notation (`"bd ~ sn bd"`), transformed by pure functions (`fast`, `every`, `jux`, `degrade`, …). Borrowed directly from TidalCycles because it is proven.
- **Sound you own.** A Rust DSP engine does all synthesis and effects on a dedicated real-time thread. No external sound server to install; genuinely good sound quality.
- **Built for live.** Named tracks (`p1`, `p2`, …) you reassign on the fly. The engine never stops; patterns swap seamlessly.
- **Sampling, including YouTube.** Grab a few seconds of audio, then chop, stretch, and pitch it — the heart of hip-hop production. (See the [note on YouTube & copyright](docs/roadmap.md#risks).)

## How it works (one diagram)

The single most important design decision: **the interpreted control plane is separated from the native audio plane.** Real-time DSP never runs in garbage-collected JavaScript — GC pauses cause audible glitches. Instead, TypeScript schedules *time-stamped events* slightly ahead of time and hands them to the Rust engine over a lock-free queue.

```text
┌─────────────────────────── TypeScript (control plane) ───────────────────────────┐
│  patterns  →  mini-notation  →  combinators  →  look-ahead scheduler              │
│  live REPL / file-watch hot-reload   •   named tracks p1..pN (reassign live)       │
└───────────────────────────────────────┬───────────────────────────────────────────┘
                       timestamped binary events │ (lock-free SPSC ring buffer)
┌───────────────────────────────────────▼───────────────────────────────────────────┐
│  Rust engine: voices (synth + drum + sampler) → per-track FX → master bus          │
│               → cpal audio I/O   •   [compiles to WASM + AudioWorklet for the web]  │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

Full detail in [docs/architecture.md](docs/architecture.md).

## Documentation

| Doc | What's in it |
| --- | --- |
| [docs/architecture.md](docs/architecture.md) | The control/audio plane split, the bridge, the look-ahead scheduler, why none of it glitches |
| [docs/pattern-model.md](docs/pattern-model.md) | `Pattern` / `Hap` / `TimeSpan`, mini-notation, the combinator vocabulary |
| [docs/event-protocol.md](docs/event-protocol.md) | The binary event format TS sends to the Rust engine |
| [docs/roadmap.md](docs/roadmap.md) | The 6-phase build plan and what "done" means for each |
| [docs/plan/](docs/plan/README.md) | The concrete, build-it-from-here plan — one detailed doc per feature, per phase |

## Project layout

```text
strobe/
  engine/            # Rust audio core (cpal, fundsp, symphonia, ssstretch)
  bindings/
    native/          # napi-rs → .node addon (owns the cpal audio thread)
    web/             # wasm-bindgen → WASM + AudioWorklet (later)
  packages/strobe/    # the TypeScript library you write songs in
  examples/          # runnable songs
  docs/              # architecture & design docs
```

## Status

Early development. Building from a working spine outward:

- ✅ **Phase 0 — proof of spine**: Rust engine makes sound; TS hot-swaps a kick loop at the cycle boundary with zero dropout. Lock-free SPSC event ring, look-ahead scheduler, napi bridge. → [`examples/phase0-kick.ts`](examples/phase0-kick.ts) (`pnpm demo`)
- ✅ **Phase 1 — synthesis core**: a real instrument layer — band-limited oscillators (sine/saw/square/tri/noise/wavetable), a resonant state-variable filter (LP/HP/BP/notch), amp + filter ADSR envelopes, a configurable subtractive **synth voice**, a synthesized **drum kit** (bd/sn/cp/hh/oh with hat choke), **polyphony** with oldest/quietest voice-stealing (16+ voices), and a transparent master **look-ahead limiter**. → [`examples/phase1-kit.ts`](examples/phase1-kit.ts) (`pnpm demo:phase1`)
- ⬜ Phases 2–6: pattern language → effects rack → sampling → live-coding DX → web backend.

> The README quickstart above shows the *target* `sound(...)`/`note(...)` pattern API, which lands with the mini-notation parser in **Phase 2**. Today you drive the engine with the Phase 1 builders (`every`, `euclid`, `seq`, preset names, `noteToHz`) — see the example.

Engine: 72 Rust tests; library: 45 TS tests; both gated on every commit. See [docs/roadmap.md](docs/roadmap.md) for the full plan.

## Built on

[cpal](https://github.com/RustAudio/cpal) (cross-platform audio I/O) · [fundsp](https://github.com/SamiPerttu/fundsp) (functional DSP) · [symphonia](https://github.com/pdeljanov/Symphonia) (audio decoding) · [ssstretch](https://github.com/bmisiak/ssstretch) / [Signalsmith Stretch](https://signalsmith-audio.co.uk/code/stretch/) (time-stretch & pitch-shift) · [napi-rs](https://napi.rs/) (Rust ↔ Node) · [wasm-bindgen](https://github.com/rustwasm/wasm-bindgen) (web target).

Pattern model inspired by [TidalCycles](https://tidalcycles.org/) and [Strudel](https://strudel.cc/).
