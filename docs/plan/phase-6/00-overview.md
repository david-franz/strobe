# Phase 6 — Web backend (overview)

## Purpose

Phase 6 runs **the same engine and the same song code in a browser**, unlocking the future click-to-code frontend at near-zero language cost. Nothing about the music changes: the cps clock, the patterns, the mini-notation, the combinators, the `sound()/note()` chaining API, the look-ahead scheduler — all of it is **pure TypeScript that sits entirely above a small `Engine` interface** ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)). Phase 0 satisfied that interface with a `napi-rs` native addon that owns a `cpal` audio thread ([phase-0/04-napi-bridge.md](../phase-0/04-napi-bridge.md)). Phase 6 satisfies the **exact same interface** with a second backend: the **same `engine/` Rust core compiled to WASM via `wasm-bindgen`, run inside an `AudioWorkletProcessor`**, fed by a `SharedArrayBuffer`-backed event ring that uses the **same 32-byte record layout** ([event-protocol.md](../../event-protocol.md#event-record-layout)).

This is the payoff of the two-plane architecture and the bridge abstraction laid down in [architecture.md](../../architecture.md#the-bridge-ts--rust):

> Because the user-facing API and the pattern engine sit entirely above this interface, **moving to the browser changes no song code** — which is what keeps the future click-to-code frontend cheap.

The locked facts for this phase (never contradict them):

- The **SAME** `engine/` Rust core compiles to two backends: **native** (`napi-rs`, Phase 0) and **web** (`wasm-bindgen` + `AudioWorklet`, this phase). One codebase, two `crate-type`s, the native-only deps (`cpal` native host, `napi`) feature-gated out of the wasm build.
- The TS library targets a small **`Engine`** interface (`start`/`stop`/`loadSample`/`pushEvents`/`stats`); the pattern engine and song API sit **ENTIRELY ABOVE** it.
- The event ring is a **lock-free SPSC of fixed 32-byte records**. In the web backend it is a **`SharedArrayBuffer`** shared with the AudioWorklet (producer = main-thread scheduler, consumer = worklet audio thread) — the browser analog of the native ring.
- **`cpal` has a Web Audio / AudioWorklet backend**, so the same I/O abstraction extends to the browser ([architecture.md](../../architecture.md#why-these-technology-choices)).
- Crates (LOCKED): `wasm-bindgen`, `cpal` (web backend), `fundsp`.

These Phase 6 docs are an **implementation plan**, not a redefinition. They never alter the pattern model, the protocol, or the `Engine` contract — they describe *how the second backend implements that contract* so the README quickstart plays unchanged.

## The features

| # | File | One-liner |
| --- | --- | --- |
| 01 | [01-wasm-compilation.md](01-wasm-compilation.md) | Compile `engine/` to WASM via `wasm-bindgen`: a `cdylib` web binding crate, feature-gating the native-only deps (`cpal` native host, `napi`) out of the wasm build, the exposed wasm API (a `render` function that fills sample buffers + drains the event ring), SIMD/threads, and the `wasm-pack`/`wasm-bindgen-cli` build pipeline. |
| 02 | [02-audioworklet-host.md](02-audioworklet-host.md) | Run the WASM DSP inside an `AudioWorkletProcessor`: the worklet instantiates the module at construction (never `fetch` in `process()`), its `process()` callback drives `render` to fill the output channels, the frame counter is the source of truth, and the main-thread `AudioWorkletNode` setup handles autoplay / user-gesture. The browser analog of the `cpal` audio thread. |
| 03 | [03-sharedarraybuffer-ring.md](03-sharedarraybuffer-ring.md) | The event ring as a `SharedArrayBuffer` shared between the main thread (producer, the TS scheduler) and the worklet (consumer): `Atomics` head/tail, the **same 32-byte record layout** as native, the **COOP/COEP** cross-origin-isolation headers required to enable SAB, and the `postMessage`-batch fallback when SAB is unavailable. |
| 04 | [04-web-engine-impl.md](04-web-engine-impl.md) | The web implementation of the **same** `Engine` interface (`start`/`stop`/`loadSample`/`pushEvents`/`stats`): wiring scheduler → SAB ring → worklet, `loadSample` via `fetch` + `decodeAudioData` (or the `symphonia`-wasm path), runtime backend selection (native vs web), and the parity proof that the README quickstart song runs **UNCHANGED**. |

## Dependency on Phases 0–5

Phase 6 adds **no new musical capability** — it re-hosts everything Phases 0–5 already built. It depends on them as follows:

- **Phase 0 — proof of spine.** Establishes the two planes, the 32-byte record ([event-protocol.md](../../event-protocol.md#event-record-layout)), the lock-free SPSC ring ([phase-0/02-event-ring-buffer.md](../phase-0/02-event-ring-buffer.md)), the look-ahead scheduler ([phase-0/05-lookahead-scheduler.md](../phase-0/05-lookahead-scheduler.md)), and the **`Engine` interface + native backend** ([phase-0/04-napi-bridge.md](../phase-0/04-napi-bridge.md)). Phase 6 is a *second implementation* of that bridge — it reuses the record layout, the ring semantics, the scheduler, and the interface unchanged.
- **Phase 1 — synthesis core.** The `fundsp` voice graphs (synth, drum) compiled to WASM. Phase 6 must verify `fundsp` builds for `wasm32-unknown-unknown` ([01](01-wasm-compilation.md)).
- **Phase 2 — pattern language.** Pure TS; runs identically in Node and the browser. This is *why* "no song code changes" holds.
- **Phase 3 — effects rack.** More `fundsp` graph in the WASM render path; no new bridge surface.
- **Phase 4 — sampling.** `loadSample(pcm, channels, sampleRate)` ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)) gains a browser ingest path: `fetch` → `decodeAudioData` (or the `symphonia`-wasm decode) on the **main thread**, then the same PCM upload to the engine ([04](04-web-engine-impl.md)).
- **Phase 5 — live-coding DX.** The REPL / file-watch hot-reload pattern becomes the browser editor that re-evaluates changed tracks and swaps them at the cycle boundary — the substrate for the future click-to-code frontend.

The bridge interface, the 32-byte record, the `Pattern`/`Hap`/`TimeSpan` shape, and the cps clock fixed in Phase 0 are carried into Phase 6 **without modification**.

## Done when (from [roadmap.md](../../roadmap.md#phase-6--web-backend-later))

Phase 6 is **done** when both of the following hold:

- **The README quickstart song plays unchanged in a browser tab.** The exact snippet from [README.md](../../../README.md) — `setcps(130/60/4)`, `p1(sound("bd*4").gain(0.9))`, `p2(sound("~ hh ~ hh").lpf(6000))`, `p3(note("c2 c2 eb2 g2").saw().lpf(sine.range(300,1200).slow(8)).room(0.3).sidechain("p1"))` — runs **byte-for-byte identical song code** against `createWebEngine()` and produces the intended sound in a browser. Not a port: the same `packages/strobe` modules, the same scheduler, selecting the web backend at runtime ([04](04-web-engine-impl.md)).
- **Editing patterns live in the browser hot-swaps with no dropout.** Re-running a track in the browser (REPL / editor) swaps it **at the next cycle boundary** with **no clicks or dropouts** — the same seamless hot-swap the native backend gives ([architecture.md](../../architecture.md#tracks-and-hot-swapping)), now driven through the `SharedArrayBuffer` ring into the worklet.

### Cross-cutting (every phase, from [roadmap.md](../../roadmap.md#cross-cutting-definition-of-done-every-phase))

- A runnable example exercises the capability: a static-served `examples/web/` page that loads `packages/strobe`, selects the web backend, and plays the quickstart on a user gesture.
- No audio underruns under the example's normal load — `stats()` (now read from worklet-written atomics; see [03](03-sharedarraybuffer-ring.md)/[04](04-web-engine-impl.md)) reports zero underruns and zero late events.
- The pure-TS logic added here (the SAB ring producer, the backend selector, the parity harness) has unit tests; the WASM render path is golden/null-tested against the native render of the same event stream.
- Validated on macOS (Chrome/Firefox first, where cross-origin isolation + AudioWorklet are well supported); other browsers confirmed before sign-off.

These two bullets are the gate. Everything in Phase 6 exists to satisfy them while changing **no song code** — which is the entire point of the phase.
