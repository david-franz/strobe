# strobe

**A live-coding music library: write patterns and effects as TypeScript while a Rust engine renders the audio.**

You build loops from drum sounds, synth notes, and samples, shape them with a per-track effects rack, and edit the code *while it plays* — each change takes effect at the next cycle boundary. The pattern language is TidalCycles'; the synthesis, effects, and sampling run in a Rust audio engine. It's aimed at programmers comfortable in a code editor rather than a DAW.

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

## Where it fits

strobe is in the live-coding / algorithmic-music space alongside [TidalCycles](https://tidalcycles.org/), [Strudel](https://strudel.cc/), [SuperCollider](https://supercollider.github.io/), and [Sonic Pi](https://sonic-pi.net/). It reuses TidalCycles' pattern model and pairs it with its own Rust audio engine:

- **Songs are TypeScript** — interpreted, hot-reloaded on save, with normal editor tooling and type-checking.
- **Patterns are pure functions of time**, written in a concise mini-notation (`"bd ~ sn bd"`) and transformed by combinators (`fast`, `every`, `jux`, `degrade`, …) — the TidalCycles model.
- **A Rust DSP engine** runs synthesis, the effects rack, and sampling on a dedicated real-time thread; no external sound server.
- **Named tracks** (`p1`, `p2`, …) reassign live; the engine keeps running, so a swap lands on the next cycle boundary with no dropout.
- **Sampling** — load local files or short YouTube grabs, then chop / slice / stretch / pitch them. (On YouTube & copyright, see the [note in the roadmap](docs/roadmap.md#risks).)

## How it works

The core design decision is to **separate the interpreted control plane from the native audio plane.** Real-time DSP never runs in garbage-collected JavaScript — a GC pause would glitch the audio. Instead, TypeScript schedules *time-stamped events* slightly ahead of time and hands them to the Rust engine over a lock-free queue.

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
| [docs/guide.md](docs/guide.md) | **User guide** (indexed, one file per topic) — mini-notation, sound/note, modifiers, combinators, signals, effects, **sampling**, tracks |
| [docs/architecture.md](docs/architecture.md) | The control/audio plane split, the bridge, the look-ahead scheduler, why none of it glitches |
| [docs/pattern-model.md](docs/pattern-model.md) | `Pattern` / `Hap` / `TimeSpan`, mini-notation, the combinator vocabulary |
| [docs/event-protocol.md](docs/event-protocol.md) | The binary event format TS sends to the Rust engine |
| [docs/roadmap.md](docs/roadmap.md) | The 6-phase build plan and what "done" means for each |
| [docs/plan/](docs/plan/README.md) | The concrete, build-it-from-here plan — one detailed doc per feature, per phase |

## Project layout

```text
strobe/
  engine/                  # Rust audio core (cpal, fundsp, symphonia, rubato; built-in WSOLA stretch)
    src/
      audio_io.rs scheduler.rs voice_alloc.rs   # the engine spine
      graph.rs stats.rs error.rs lib.rs
      protocol/            # the wire format: event records + the lock-free ring
      dsp/                 # signal-processing blocks: filter, envelope, effects (master bus)
      voices/              # oscillator + synth voice + drum kit
  bindings/
    native/                # napi-rs → .node addon (owns the cpal audio thread)
    web/                   # wasm-bindgen → WASM + AudioWorklet (later)
  packages/strobe/         # the TypeScript library you write songs in
    src/
      backend/             # the Engine interface + native bridge
      clock/               # cps clock + cycle↔frame mapping
      pattern/             # Pat/Hap/TimeSpan, mini-notation parser, combinators, sound/note, signals
      instruments/         # synth + drum builders, preset ids
      runtime/             # look-ahead scheduler, track slots, file-watch hot-reload
      index.ts             # public surface
    tests/                 # vitest specs, mirroring src/ (not colocated)
  examples/                # runnable songs
  docs/                    # architecture & design docs
```

## Status

Early development. Building from a working spine outward:

- ✅ **Phase 0 — proof of spine**: Rust engine makes sound; TS hot-swaps a kick loop at the cycle boundary with zero dropout. Lock-free SPSC event ring, look-ahead scheduler, napi bridge. → [`examples/phase0-kick.ts`](examples/phase0-kick.ts) (`pnpm demo`)
- ✅ **Phase 1 — synthesis core**: a real instrument layer — band-limited oscillators (sine/saw/square/tri/noise/wavetable), a resonant state-variable filter (LP/HP/BP/notch), amp + filter ADSR envelopes, a configurable subtractive **synth voice**, a synthesized **drum kit** (bd/sn/cp/hh/oh with hat choke), **polyphony** with oldest/quietest voice-stealing (16+ voices), and a transparent master **look-ahead limiter**. → [`examples/phase1-kit.ts`](examples/phase1-kit.ts) (`pnpm demo:phase1`)
- ✅ **Phase 2 — pattern language**: the declarative language above is now real — `Pat<T>` (a pure function of time), the **mini-notation** parser (`"bd(3,8)"`, `~`, `*` `/`, `[]`, `<>`, `,`, `!`, `?`), the full **combinator** vocabulary (`fast`/`rev`/`every`/`iter`/`jux`/`off`/`degrade`/…, as functions and chainable methods), the **`sound()`/`note()`/`freq()`** API with chained modifiers, **continuous signals** (`sine`/`saw`/`rand`/… `.range().slow()`), the scheduler that compiles each `Hap<ControlMap>` into the 32-byte protocol, and **named tracks** with seamless cycle-boundary hot-swap + mute/solo/hush. → [`examples/phase2-song.ts`](examples/phase2-song.ts) (`pnpm demo:phase2`)
- ✅ **Phase 3 — effects rack**: per-track **effect chains** + global **send buses**, all pre-allocated and driven by smoothed params (no audio-thread allocation). A sweepable **performance filter** + 3-band **EQ**, **delay** & algorithmic **reverb** sends, **distortion** & **bitcrush**, **chorus/flanger/phaser**, a **compressor**, and the headline techno **sidechain pump** — plus audio-rate **parameter automation** so a signal can smoothly sweep any effect param. → [`examples/phase3-techno.ts`](examples/phase3-techno.ts) (`pnpm demo:phase3`)
- ✅ **Phase 4 — sampling**: load audio (WAV/MP3/FLAC/OGG via **symphonia**, resampled with **rubato**) into a lock-free, RT-safe **sample bank**; a polyphonic **sampler voice** (fractional read + cubic interp, speed/gain/region/loop/reverse); **chop**/**slice**/**reverse**/**loop** as pure pattern transforms over one `TrigSample` opcode; decoupled **time-stretch** & **pitch-shift** (a built-in WSOLA, offline); an engine-side **granular** voice (fixed grain pool, no audio-thread allocation); and **opt-in YouTube ingest** (`yt-dlp`+`ffmpeg`, disk-cached, gated). → [`examples/phase4-sampler.ts`](examples/phase4-sampler.ts) (`pnpm demo:phase4`)
- ⬜ Phases 5–6: live-coding DX → web backend.

Phases 0–4 are merged to `main`. The quickstart above runs as written (`pnpm demo:phase3`); sampling is shown in [`examples/phase4-sampler.ts`](examples/phase4-sampler.ts) (`pnpm demo:phase4`).

Engine: 139 Rust tests; library: 142 TS tests; both gated on every commit. See [docs/roadmap.md](docs/roadmap.md) for the full plan.

## Built on

[cpal](https://github.com/RustAudio/cpal) (cross-platform audio I/O) · [fundsp](https://github.com/SamiPerttu/fundsp) (functional DSP) · [symphonia](https://github.com/pdeljanov/Symphonia) (audio decoding) · [rubato](https://github.com/HEnquist/rubato) (sample-rate conversion) · a built-in WSOLA time-stretch / pitch-shift · [napi-rs](https://napi.rs/) (Rust ↔ Node) · [wasm-bindgen](https://github.com/rustwasm/wasm-bindgen) (web target) · optional [yt-dlp](https://github.com/yt-dlp/yt-dlp) + [ffmpeg](https://ffmpeg.org) for YouTube ingest.

Pattern model inspired by [TidalCycles](https://tidalcycles.org/) and [Strudel](https://strudel.cc/).
