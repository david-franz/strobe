# Phase 6.01 — Compiling `engine/` to WASM (`bindings/web/`)

## Purpose

This feature makes the **same** `engine/` Rust core that the native addon wraps compile to **WebAssembly via `wasm-bindgen`**, so it can run inside an AudioWorklet ([02](02-audioworklet-host.md)). It satisfies the **web** row of the bridge table in [architecture.md](../../architecture.md#the-bridge-ts--rust) — *Engine compiled to WASM, run inside an AudioWorklet* — and is the foundation for everything else in Phase 6. The hard requirement: **one Rust codebase, two backends.** The native-only dependencies (`napi`, the `cpal` *native* host) must be **feature-gated out** of the wasm build so `engine/` compiles cleanly for `wasm32-unknown-unknown`, while the DSP — `fundsp` voice/effect graphs, the SPSC ring, the scheduler/consumer — is shared verbatim.

What the WASM module exposes to the worklet is deliberately tiny and allocation-disciplined: **a `render` function that fills sample buffers and drains the event ring**, plus the control-call surface (`loadSample`, `stats`) wired to the same engine methods the native bridge calls. No song code, no pattern logic, no scheduler math lives here — those stay in pure TS ([phase-2](../phase-2/00-overview.md)) above the `Engine` interface.

## Location

```
engine/
  Cargo.toml             # MODIFIED — add [features]; gate native-only deps; add wasm deps
  src/lib.rs             # MODIFIED — cfg-gate the cpal-native host; keep DSP shared
  src/audio_io.rs        # MODIFIED — split: native cpal host vs wasm "render-only" host
bindings/web/
  Cargo.toml             # NEW — cdylib, wasm-bindgen, depends on `engine` (features = ["web"])
  src/lib.rs             # NEW — #[wasm_bindgen] WasmEngine over engine::Engine
  build.sh               # NEW — wasm-pack / wasm-bindgen-cli pipeline
  pkg/                   # generated — strobe_web_bg.wasm + strobe_web.js (glue) + .d.ts
packages/strobe/
  src/engine.web.ts      # NEW (owned by [04]) — loads pkg/, implements the Engine interface
```

This document owns `bindings/web/**` and the **feature-gating** of `engine/`. The worklet that calls into the generated `pkg/` is [02](02-audioworklet-host.md); the SAB ring it drains is [03](03-sharedarraybuffer-ring.md); the TS `Engine` impl is [04](04-web-engine-impl.md).

## Design

### One core, two `crate-type`s, feature flags

`engine/` stays a normal library crate. The two binding crates (`bindings/native`, `bindings/web`) are the `cdylib`s; each pulls `engine` with a different feature set. We add a feature axis to `engine/Cargo.toml` that selects the **host** (how audio I/O is driven) without touching the DSP:

```toml
# engine/Cargo.toml (excerpt)
[features]
default = []
# Native host: cpal owns a real OS audio thread (Phase 0).
native = ["cpal", "cpal/default"]
# Web host: NO cpal native backend; the AudioWorklet drives render() itself.
web = []

[dependencies]
fundsp = "0.x"                      # SHARED — must build for wasm32-unknown-unknown
# cpal is OPTIONAL and only enabled by the `native` feature.
cpal = { version = "0.15", optional = true }

[target.'cfg(target_arch = "wasm32")'.dependencies]
# wasm-only helpers live here so they never enter the native build.
wasm-bindgen = "0.2"
```

> **`cpal`'s web backend.** [architecture.md](../../architecture.md#why-these-technology-choices) notes cpal *has* a Web Audio/AudioWorklet backend. For Phase 6 the cleanest, lowest-risk path is to **not** route browser audio through cpal at all: inside an AudioWorklet the host *is* the worklet, and `process()` already hands us output buffers to fill. So the `web` feature compiles the engine **without `cpal`**, and our wasm host is a thin "render-only" shim that the worklet calls. (cpal's worklet backend remains a fallback if we later want cpal to own the worklet plumbing; it is not on the critical path.) Either way the DSP render loop is identical.

### Splitting the host: shared render, gated I/O

The render loop — drain ring ≤ `currentFrame + bufferLen`, apply events to voices/params, mix voices → FX → master → fill output ([architecture.md](../../architecture.md#the-rust-engine-internals)) — is **shared, host-agnostic** code. Only *who calls it* differs.

```rust
// engine/src/lib.rs
pub struct Engine {
    core: EngineCore,                  // SHARED: ring consumer, voices, fx, frame clock, stats
    #[cfg(feature = "native")]
    stream: Option<cpal::Stream>,      // native only — cpal owns the audio thread
}

impl EngineCore {
    /// THE shared hot path. Host-agnostic. No alloc / no lock / no host calls.
    /// Drains events up to `frame_base + out_frames`, renders into interleaved `out`.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        let out_frames = out.len() / channels;
        self.drain_ring_until(self.frame + out_frames as u64); // [phase-0/02]
        self.render_voices_fx_master(out, channels);            // [phase-1/04],[phase-3]
        self.frame += out_frames as u64;
    }
}

#[cfg(feature = "native")]
impl Engine {
    pub fn start(&mut self, sr: Option<u32>, bf: Option<u32>) -> Result<(), EngineError> {
        // opens cpal stream; the cpal callback calls core.render(out, channels) per buffer
    }
}

// No cpal `start` under `web`; the WASM host (below) calls core.render() from the worklet.
```

The Phase 0 native bridge ([phase-0/04-napi-bridge.md](../phase-0/04-napi-bridge.md)) is unchanged: it builds `engine` with `features = ["native"]`.

### The exposed wasm API

`bindings/web/src/lib.rs` is a `#[wasm_bindgen]` wrapper exposing exactly what the worklet needs. The two non-negotiable methods are **`render` (fill buffers + drain ring)** and **a way to point the consumer at the `SharedArrayBuffer` ring** ([03](03-sharedarraybuffer-ring.md)). `loadSample` and `stats` mirror the native control-call surface ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).

```rust
// bindings/web/src/lib.rs
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WasmEngine {
    core: engine::EngineCore,        // the SAME core the native addon wraps
}

#[wasm_bindgen]
impl WasmEngine {
    /// Construct WITHOUT touching any host. Safe to call inside the worklet ctor.
    /// `sab_bytes` is the SharedArrayBuffer's WebAssembly.Memory-visible bytes that
    /// back the event ring; the consumer reads records straight out of it. [03]
    #[wasm_bindgen(constructor)]
    pub fn new(sample_rate: f32, channels: u32, ring_ptr: u32, ring_len: u32) -> WasmEngine {
        WasmEngine { core: engine::EngineCore::new_web(sample_rate, channels, ring_ptr, ring_len) }
    }

    /// THE audio-thread hot path. Called once per `process()` quantum from the worklet.
    /// Writes `frames * channels` interleaved f32 into the engine's WASM-linear-memory
    /// scratch buffer; the worklet copies it out into the per-channel output arrays. [02]
    #[wasm_bindgen]
    pub fn render(&mut self, frames: u32) -> *const f32 {
        self.core.render_into_scratch(frames as usize); // drains SAB ring + renders
        self.core.scratch_ptr()                          // pointer into WASM linear memory
    }

    /// Non-real-time control call: upload decoded interleaved PCM → sample id.
    /// Mirrors Engine.loadSample. [event-protocol.md], [04]
    #[wasm_bindgen]
    pub fn load_sample(&mut self, pcm: &[f32], channels: u32, sample_rate: f32) -> u32 {
        self.core.load_sample(pcm, channels, sample_rate)
    }

    /// Telemetry. Returns the same shape as native Stats, packed for JS. [04]
    #[wasm_bindgen]
    pub fn stats(&self) -> StatsJs { self.core.stats().into() }
}
```

> `render` returning a `*const f32` into WASM linear memory (rather than allocating/returning a JS array) keeps the audio thread **allocation-free**, matching the native invariant ([architecture.md](../../architecture.md#what-can-go-wrong-and-the-guardrails)). The worklet wraps that pointer in a `Float32Array` view over `WebAssembly.Memory.buffer` and de-interleaves into the output channels ([02](02-audioworklet-host.md)).

### Ring consumer in WASM linear memory

The native ring lives in Rust-owned heap; the web ring lives in a **`SharedArrayBuffer`** so the main-thread scheduler (producer) and the worklet (consumer) both see it ([03](03-sharedarraybuffer-ring.md)). When threads are enabled, the SAB *is* WASM linear memory (a `shared` `WebAssembly.Memory`), so the Rust consumer reads records by casting the byte slice at `ring_ptr` to `&[EventRecord]` with `bytemuck` — **the identical 32-byte layout and the identical drain logic as Phase 0** ([phase-0/02-event-ring-buffer.md](../phase-0/02-event-ring-buffer.md)). Head/tail are `AtomicU32`s at fixed offsets; the consumer uses `Acquire`/`Release` exactly as the native SPSC ring does.

### Build pipeline

Use **`wasm-pack`** (which wraps `wasm-bindgen-cli`) for the standard path, targeting `web` output (ES module glue, no bundler assumptions — important because the glue is `import`ed both on the main thread and inside the worklet):

```bash
# bindings/web/build.sh
set -euo pipefail
# 1. Build engine+web binding to wasm32 with the `web` feature (NO cpal native, NO napi).
# 2. Run wasm-bindgen to emit ES-module glue + .d.ts.
wasm-pack build bindings/web \
  --target web \
  --out-dir pkg \
  --release \
  -- --no-default-features --features web      # engine compiled with features=["web"]

# (Threads/SIMD variant — see below — adds RUSTFLAGS and nightly.)
```

### SIMD & threads

- **SIMD.** `fundsp` benefits from WASM SIMD; enable with `RUSTFLAGS="-C target-feature=+simd128"`. Feature-detect at load and fall back to the scalar build if a browser lacks it. SIMD is a pure speedup — it changes no audio output (golden tests in [04](04-web-engine-impl.md) must match the native render regardless).
- **Threads.** The cross-thread part of Phase 6 is the **producer/consumer split across the main thread and the worklet thread** — that is what the `SharedArrayBuffer` ring provides ([03](03-sharedarraybuffer-ring.md)), and it requires **cross-origin isolation (COOP/COEP)** to use SAB at all. Whether we also compile the engine with `wasm32-unknown-unknown` + `atomics` (shared memory, `wasm-bindgen-rayon`-style) is an optimization for *intra-render* parallelism only; the SPSC ring across threads works with a non-shared engine memory too, as long as the SAB that backs the ring is the shared region the worklet reads. Phase 6 ships **single-threaded render + shared-memory ring** first; intra-render threading is a later optimization behind the same `Engine` interface.

## Interfaces

- **The `Engine` interface ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).** The wasm exports here are the *engine-side half* of `start`/`stop`/`loadSample`/`pushEvents`/`stats`. `pushEvents` has **no wasm export** — the scheduler writes records straight into the SAB ring on the main thread ([03](03-sharedarraybuffer-ring.md)); the wasm consumer drains them inside `render`. `loadSample`/`stats` map to `WasmEngine.load_sample`/`stats`. `start`/`stop` are realized on the JS side by creating/closing the `AudioContext` + worklet node ([02](02-audioworklet-host.md), [04](04-web-engine-impl.md)).
- **The 32-byte protocol.** The wasm consumer reads the **identical** 32-byte record ([event-protocol.md](../../event-protocol.md#event-record-layout)) out of the SAB, `frame`-stamped and little-endian, with the same `from_le_bytes`/`bytemuck` cast as native.
- **Parity with native.** Both bindings wrap `engine::EngineCore`; `render` here and the cpal callback there call the **same** `render_into_scratch`/`render`. Same events in → same samples out (verified in [04](04-web-engine-impl.md)).

## Edge cases & risks

- **A native-only dep leaks into the wasm build.** If `napi` or `cpal` (native host) is reachable from the `web` feature, `wasm32-unknown-unknown` won't link (no OS threads/sockets). *Guardrail:* `cpal` is `optional`, only pulled by `native`; `napi` lives only in `bindings/native`; CI runs `cargo build -p strobe-web --target wasm32-unknown-unknown --no-default-features --features web`.
- **`fundsp` must build for wasm.** It is `no_std`-friendly pure DSP and should compile; *risk* is a transitive dep using `std::time`/threads. *Guardrail:* the CI wasm build above is the canary; pin/patch any offending transitive dep.
- **No `fetch` / no DOM on the audio thread.** The wasm module is *constructed* and *driven* inside the worklet, which has **no `fetch`, no DOM** ([02](02-audioworklet-host.md)). So `loadSample` decoding must happen on the **main thread** and only the decoded PCM crosses in ([04](04-web-engine-impl.md)); the wasm API here only *accepts* PCM, never fetches it.
- **WASM memory growth on the audio thread.** Growing `WebAssembly.Memory` mid-`process()` would allocate — forbidden on the audio thread. *Guardrail:* pre-size the engine's linear memory (initial pages cover voices + sample buffers + ring + scratch); `loadSample` (main thread, non-real-time) is the only path that may grow memory, and it does so *between* render quanta.
- **Module size / load time.** The `.wasm` must be fetched and instantiated before audio starts. *Guardrail:* instantiate during `start()` (a user gesture; [02](02-audioworklet-host.md)) and `await` it before the first `process()`; build with `--release` + `wasm-opt`.
- **SIMD/threads unsupported.** Some browsers lack `simd128` or cross-origin isolation. *Guardrail:* ship a scalar single-thread build as the baseline; feature-detect and upgrade.

## Acceptance criteria

- [ ] **`engine/` compiles for wasm with native deps gated out.** *Test:* `cargo build -p strobe-web --target wasm32-unknown-unknown --no-default-features --features web` succeeds; `cargo tree` shows **no `cpal`, no `napi`** in the wasm dependency graph.
- [ ] **Native build still green.** *Test:* `cargo build -p strobe-native` (engine `features=["native"]`) still builds and the Phase 0 example still plays — the feature split broke nothing.
- [ ] **`wasm-pack build` emits a usable `pkg/`.** *Test:* `bindings/web/build.sh` produces `pkg/strobe_web_bg.wasm`, `pkg/strobe_web.js` (ES module), and `pkg/strobe_web.d.ts` exposing `WasmEngine` with `new`, `render`, `load_sample`, `stats`.
- [ ] **`render` is allocation-free.** *Test:* call `render(128)` in a tight 100k-iteration loop in a headless wasm harness; assert WASM memory `byteLength` does not grow and no per-call JS allocation occurs.
- [ ] **`render` drains the SAB ring with the 32-byte layout.** *Test:* write a known `NoteOn` record into the SAB at a frame inside the next quantum; after `render`, assert a voice was triggered (audible/golden), proving the same layout + drain as native ([phase-0/02-event-ring-buffer.md](../phase-0/02-event-ring-buffer.md)).
- [ ] **SIMD on/off produce identical samples.** *Test:* golden-compare `render` output of the `+simd128` build vs the scalar build for the same event stream; bit-for-bit (or within DSP null-test tolerance) equal.
