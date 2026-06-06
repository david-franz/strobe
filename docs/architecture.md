# Architecture

This document explains how `strobe` produces glitch-free sound while letting you change the music live from TypeScript. If you read only one design doc, read this one.

## The core principle: two planes

Real-time audio has a hard constraint that shapes everything else: **the audio callback must produce its next buffer of samples on time, every time.** A typical buffer at 48 kHz with 256 frames must be ready every ~5.3 ms. Miss that deadline and you get an audible click, pop, or dropout (a "buffer underrun").

JavaScript cannot make that guarantee. It is garbage-collected; a GC pause of even a few milliseconds at the wrong moment ruins the buffer. So we never run DSP in JS. We split the system into two planes:

### Control plane — TypeScript (soft real-time)

- Holds the *patterns* — the declarative description of the music.
- Runs the *scheduler*, which figures out which events happen when.
- Is where you spend 100% of your time, and is fully hot-reloadable.
- Tolerates jitter: it works *ahead of time*, so a 5 ms hiccup here is invisible.

### Audio plane — Rust (hard real-time)

- Owns the audio thread (driven by `cpal`).
- Synthesizes voices and runs effects using `fundsp` graphs.
- Touches no locks, no allocation, and no JS on the audio thread.
- Just consumes a time-ordered queue of events and renders samples.

The two planes communicate over a **lock-free single-producer/single-consumer (SPSC) ring buffer**: TS is the producer, the audio thread is the consumer. Events carry sample-accurate timestamps.

```
   TypeScript (Node thread)                 Rust (audio thread, cpal callback)
   ─────────────────────────                ──────────────────────────────────
   scheduler tick (every ~25ms)             for each frame in the buffer:
     query patterns for next 100ms     ►┐     drain ring buffer up to `now`
     encode events (binary)             │     apply events to voices/params
     push to SPSC ring buffer ──────────┘     render active voices → FX → master
                                              write frame to output
```

## The look-ahead scheduler ("two clocks")

This is the classic technique that makes browser/Node audio feel sample-accurate despite a jittery host clock. (It is the same pattern Chris Wilson described in *"A Tale of Two Clocks"* for the Web Audio API.)

- A coarse JS timer (`setInterval`, ~25 ms) wakes the scheduler.
- On each wake, the scheduler looks at a window from `now` to `now + lookahead` (default ~100 ms) and asks each active pattern: *what events fall in this window?*
- Each event is stamped with a precise audio-frame timestamp and pushed to the ring buffer **before** its play time.
- The audio thread, which has the only accurate clock (the sample count), plays each event exactly when its timestamp arrives.

The JS timer only needs to be *roughly* on time, because it is always working ~100 ms into the future. The actual musical timing is decided by sample counts on the audio thread.

```
lookahead window:   [ now ●─────────────► now+100ms ]
scheduler wakes:    ↑      ↑      ↑      ↑   (every ~25ms, overlapping windows)
events enqueued:        │ bd │   │ hh │      (stamped with exact frame numbers)
audio thread plays:                          (precisely, by sample count)
```

**Tradeoff:** lookahead is added latency between "you change the code" and "you hear it." 100 ms is a good default — short enough to feel live, long enough to absorb JS jitter. It is configurable.

## The clock and musical time

Time is **cycle-based**, following TidalCycles. The fundamental unit is the *cycle*, and tempo is *cycles per second* (`cps`).

- `setcps(x)` sets the tempo directly.
- A BPM helper converts: at 4 beats per cycle, `bpm = cps * 4 * 60`, so `setcps(130/60/4)` ≈ 130 BPM.

A pattern is defined over one cycle (the span `0 → 1`) and repeats. The scheduler maps cycle-time to wall-clock-time to audio-frames. Combinators like `fast(2)` or `slow(4)` warp a pattern's cycle-time; the mapping to real time is unchanged.

## Tracks and hot-swapping

Named tracks (`p1`, `p2`, … `pN`) are mutable slots that each hold one pattern. `p1(somePattern)` assigns the slot. When you re-run that line live:

1. The slot's pattern reference is replaced immediately (a plain variable assignment — cheap and atomic from JS's perspective).
2. The scheduler does **not** retro-actively change already-enqueued events (the next ~100 ms is already committed).
3. From the **next cycle boundary**, the scheduler queries the new pattern. Swapping at the cycle boundary is what keeps the music in time and click-free.

This is exactly the "spawn loops, change effects per loop" workflow: each track is a loop; reassigning it layers and reshapes the sound without ever stopping the engine.

## The bridge (TS ↔ Rust)

Two binding targets compile from the **same** `engine/` Rust core:

| Target | Crate | How audio runs | Used for |
| --- | --- | --- | --- |
| **Native** | `napi-rs` | Rust spawns and owns the `cpal` audio thread; TS calls in-process | Desktop / CLI (now) |
| **Web** | `wasm-bindgen` | Engine compiled to WASM, run inside an AudioWorklet | Browser frontend (later) |

The TypeScript library targets a small **engine interface** (start/stop, load sample buffer, push events, set master params). The native addon is the first implementation; the WASM/AudioWorklet backend is the second. Because the user-facing API and the pattern engine sit entirely above this interface, **moving to the browser changes no song code** — which is what keeps the future click-to-code frontend cheap.

See [event-protocol.md](event-protocol.md) for the exact bytes that cross the bridge.

## The Rust engine internals

```
                 ring buffer (events in)
                          │
                          ▼
                  ┌───────────────┐
   event dispatch │  scheduler.rs │  drains events ≤ current frame
                  └───────┬───────┘
                          ▼
   ┌──────────────────────────────────────────────┐
   │  voice_alloc.rs   (polyphony, voice stealing) │
   │   ┌─────────┐  ┌────────┐  ┌──────────┐        │
   │   │ synth   │  │ drum   │  │ sampler  │  ...   │   each = a fundsp graph
   │   └────┬────┘  └───┬────┘  └────┬─────┘        │
   └────────┼───────────┼───────────┼──────────────┘
            └───────────┴───────────┘
                        ▼
              per-track effects.rs        (filter, delay, reverb, dist, comp, sidechain…)
                        ▼
                  master bus + limiter
                        ▼
                   cpal output
```

- **`audio_io.rs`** — opens the `cpal` stream, picks a device/sample rate, runs the master limiter, and calls the render loop per buffer.
- **`scheduler.rs`** — the consumer side of the ring buffer; applies events at their exact frames.
- **`protocol/`** — `event.rs` (the fixed 32-byte record + decode) and `ring.rs` (the lock-free SPSC ring); the wire format that crosses the bridge.
- **`voices/`** — `oscillator.rs`, `synth.rs` (osc → filter → envelope → amp), `drum.rs` (synthesized kick/snare/hat/clap), and (later) `sampler.rs`.
- **`voice_alloc.rs`** — assigns incoming note events to a pool of voices; steals the oldest/quietest when full.
- **`dsp/`** — the signal-processing building blocks: `filter.rs` (SVF), `envelope.rs` (ADSR), `effects.rs` (master bus + limiter now; the per-track/send effects rack in Phase 3 — incl. the sidechain "pump").
- **`graph.rs`** — the `InstrumentConfig` voices are built from, sent by TS over the control channel.

## Why these technology choices

- **`cpal`** — the standard cross-platform (Win/Mac/Linux) audio I/O crate, and it has a Web Audio/AudioWorklet backend, so the same I/O abstraction extends to the browser.
- **`fundsp`** — functional DSP with a composable graph notation (`>>` pipe, `|` stack, `&` bus, `^` branch). Oscillators, filters, envelopes, and many effects come as zero-cost, strongly-typed building blocks. Its declarative style matches the library's philosophy and means we are not hand-writing every biquad.
- **`symphonia`** — pure-Rust decoding (MP3/WAV/FLAC/OGG…) for loaded samples; no native codec dependencies.
- **`ssstretch`** (Signalsmith Stretch bindings) — high-quality, decoupled time-stretch and pitch-shift. Kept behind an interface so the algorithm can be swapped.
- **`napi-rs`** — builds the Node addon with the pure Rust/JS toolchain (no node-gyp), and supports prebuilt per-OS binaries so end users install with `npm i`.

## What can go wrong (and the guardrails)

| Failure mode | Cause | Guardrail |
| --- | --- | --- |
| Clicks / dropouts | Audio thread misses its deadline | No locks/alloc/JS on the audio thread; pre-render ahead; reasonable buffer size |
| Late events | Scheduler lookahead too short for the host | Configurable lookahead (default ~100 ms); drop-and-warn if an event arrives past its frame |
| Timing drift | Trusting the JS clock | The sample count is the source of truth; JS only schedules ahead |
| High latency feel | Lookahead too long | Tunable; surface it as a setting |
| Backpressure | Ring buffer fills | Size it for » one lookahead window; warn if it saturates |

## Glossary

- **Cycle** — the fundamental unit of musical time; patterns are defined over one cycle and repeat.
- **cps** — cycles per second (the tempo).
- **Hap** — a single timed event produced by a pattern (a "happening": *what*, *when*, *how long*).
- **TimeSpan / Arc** — a `[begin, end)` interval of cycle-time that the scheduler queries a pattern over.
- **Voice** — one playing sound (a synth note, a drum hit, a sample). Polyphony = many voices at once.
- **SPSC ring buffer** — a lock-free queue with exactly one producer (TS) and one consumer (audio thread).
- **Underrun** — the audio thread failed to deliver a buffer on time → an audible glitch.
