# Phase 6.02 — The AudioWorklet host (`worklet/tekno-processor.js`)

## Purpose

This feature runs the WASM DSP ([01](01-wasm-compilation.md)) inside an **`AudioWorkletProcessor`** — the **browser analog of the `cpal` audio thread** ([architecture.md](../../architecture.md#the-bridge-ts--rust)). An AudioWorklet runs on a dedicated, high-priority audio rendering thread, isolated from the main thread's garbage collector and the DOM — exactly the property the architecture demands so that *real-time DSP never runs in garbage-collected JavaScript* ([architecture.md](../../architecture.md#the-core-principle-two-planes)). The worklet's `process()` callback is the web equivalent of cpal's buffer callback: each invocation, it drives `WasmEngine.render` to fill the output buffer, and the engine's running frame counter remains the single source of truth for timing ([event-protocol.md](../../event-protocol.md#the-frame-timestamp)).

Two browser-specific constraints shape the whole design: **the worklet cannot `fetch` or touch the DOM**, and `process()` must never block or allocate. So the WASM module is **instantiated once at construction** (with bytes handed in from the main thread), never fetched inside `process()`; and the SAB event ring ([03](03-sharedarraybuffer-ring.md)) is drained *inside* the WASM render with no JS allocation. The main thread sets up the `AudioWorkletNode` and starts audio **on a user gesture** to satisfy autoplay policy.

## Location

```
bindings/web/
  worklet/
    tekno-processor.js     # NEW — the AudioWorkletProcessor (registered in the worklet scope)
  pkg/                     # (01) generated WASM + ES glue, imported here and on main thread
packages/tekno/
  src/web/audio-host.ts    # NEW — main-thread: AudioContext, addModule, AudioWorkletNode, gesture
  src/engine.web.ts        # ([04]) — uses audio-host.ts to implement start()/stop()
```

This document owns the worklet processor and the main-thread `AudioWorkletNode` setup. The SAB ring it drains is [03](03-sharedarraybuffer-ring.md); the `Engine` impl that orchestrates it is [04](04-web-engine-impl.md).

## Design

### The browser audio-thread analogy

| | Native (Phase 0) | Web (Phase 6) |
| --- | --- | --- |
| Audio thread | `cpal` spawns/owns it | the AudioWorklet rendering thread |
| Per-buffer callback | cpal data callback | `AudioWorkletProcessor.process()` |
| Buffer size | device buffer (e.g. 256) | a render **quantum** (128 frames, fixed by spec) |
| Drives render | callback → `core.render(out)` | `process()` → `WasmEngine.render(128)` |
| Frame clock | `AtomicU64` in core | the same `frame` counter in the WASM core |
| Events in | SPSC ring (Rust heap) | SPSC ring (SharedArrayBuffer) [03] |

The quantum is **128 frames**, fixed by the Web Audio spec. `render(128)` is called once per quantum; the engine drains every ring event with `frame ≤ currentFrame + 128` and applies each at its exact in-buffer offset — **identical drain semantics to native** ([phase-0/02-event-ring-buffer.md](../phase-0/02-event-ring-buffer.md)).

### Loading WASM inside the worklet (no `fetch` in `process`)

The worklet scope has **no `fetch` and no `WebAssembly` streaming from a URL is needed** — and we must never do I/O on the audio thread. So the main thread fetches the `.wasm` bytes once, and ships them (plus the SAB ring and config) to the processor via the node's `port` *before* the first `process()`. The processor instantiates the module **synchronously in handling that first message / at construction**, then signals "ready".

```js
// bindings/web/worklet/tekno-processor.js  (runs in the AudioWorkletGlobalScope)
import initSync, { WasmEngine } from "../pkg/tekno_web.js"; // ES glue; initSync takes raw bytes

class TeknoProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.engine = null;
    this.ready = false;
    this.port.onmessage = (e) => this.onInit(e.data);
  }

  onInit(msg) {
    // msg = { wasmBytes: ArrayBuffer, ringSab: SharedArrayBuffer, ringPtr, ringLen, channels }
    // 1. Instantiate the WASM module from bytes — NO fetch, NO network. Synchronous.
    initSync({ module: msg.wasmBytes, memory: msg.sharedMemory /* if threads */ });
    // 2. Construct the engine, pointing its ring consumer at the SAB. [03]
    this.engine = new WasmEngine(sampleRate, msg.channels, msg.ringPtr, msg.ringLen);
    this.channels = msg.channels;
    this.ready = true;
    this.port.postMessage({ type: "ready" });
  }

  // THE audio-thread hot path — once per 128-frame quantum.
  process(_inputs, outputs, _params) {
    if (!this.ready) return true;          // keep node alive until WASM is up
    const out = outputs[0];                // out[ch] is a Float32Array(128)
    const frames = out[0].length;          // 128
    // Drive the engine: drains SAB ring ≤ frame+128, renders interleaved into WASM memory.
    const ptr = this.engine.render(frames);
    // Wrap the engine's scratch (WASM linear memory) and de-interleave into channels.
    const mem = new Float32Array(wasmMemory.buffer, ptr, frames * this.channels);
    for (let ch = 0; ch < out.length; ch++) {
      const dst = out[ch];
      for (let i = 0; i < frames; i++) dst[i] = mem[i * this.channels + ch];
    }
    return true;                           // keep processing
  }
}
registerProcessor("tekno-processor", TeknoProcessor);
```

- `process()` does **no allocation** (the `Float32Array` view aliases existing WASM memory; reuse one cached view if memory never grows during playback — see risks), **no `fetch`, no DOM, no `postMessage` on the hot path** (telemetry is written to a stats SAB the main thread polls; [03](03-sharedarraybuffer-ring.md)/[04](04-web-engine-impl.md)).
- Returning `true` keeps the processor alive even during silence (we never want the node to be GC'd while the song plays).
- `initSync` is used (not the async default) so instantiation completes deterministically before `ready`. The bytes come from the main thread's one-time `fetch`.

### Main-thread setup + user gesture / autoplay

The main thread creates the `AudioContext`, adds the worklet module, fetches the WASM bytes, creates the `AudioWorkletNode`, and ships init data over the node port. Browser **autoplay policy** requires this to start (or `resume()`) inside a **user gesture** (click/tap), so `start()` is wired to a button.

```ts
// packages/tekno/src/web/audio-host.ts
export interface AudioHost {
  ctx: AudioContext;
  node: AudioWorkletNode;
  readonly currentFrameApprox: number;   // ctx.currentTime * sampleRate (coarse; engine frame is truth)
}

export async function startAudioHost(opts: {
  wasmUrl: string;            // URL of pkg/tekno_web_bg.wasm
  workletUrl: string;         // URL of worklet/tekno-processor.js
  ringSab: SharedArrayBuffer; // the event ring [03]
  ringPtr: number; ringLen: number;
  channels: number;
}): Promise<AudioHost> {
  // MUST be called from within a user-gesture handler (click) for autoplay policy.
  const ctx = new AudioContext({ latencyHint: "interactive" });
  await ctx.resume();                                  // unlock audio on the gesture
  await ctx.audioWorklet.addModule(opts.workletUrl);   // loads the processor module (main thread)

  const wasmBytes = await fetch(opts.wasmUrl).then(r => r.arrayBuffer()); // main-thread fetch only

  const node = new AudioWorkletNode(ctx, "tekno-processor", {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [opts.channels],
  });
  node.connect(ctx.destination);

  // Ship init data to the worklet; wait for its "ready" before scheduling events.
  await new Promise<void>((resolve) => {
    node.port.onmessage = (e) => { if (e.data?.type === "ready") resolve(); };
    node.port.postMessage({
      wasmBytes, ringSab: opts.ringSab, ringPtr: opts.ringPtr,
      ringLen: opts.ringLen, channels: opts.channels,
    }, [/* wasmBytes is transferable; ringSab is shared, NOT transferred */]);
  });

  return { ctx, node, get currentFrameApprox() { return Math.round(ctx.currentTime * ctx.sampleRate); } };
}
```

> `ringSab` is a `SharedArrayBuffer` — it is **shared, not transferred** (a transfer would neuter it on the main thread). Only `wasmBytes` (a plain `ArrayBuffer`) may be listed in the transfer list. The scheduler keeps writing into `ringSab` on the main thread while the worklet reads it ([03](03-sharedarraybuffer-ring.md)).

### Frame counter = source of truth

`ctx.currentTime` is a coarse, jittery main-thread clock — like the JS host clock in the native design, it is only used for *scheduling ahead*, never for musical timing. The **engine's `frame` counter inside the WASM core is the source of truth** ([architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks)); the scheduler reads it via `stats().currentFrame` (written to a stats SAB by the worklet; [04](04-web-engine-impl.md)) and stamps every event's `frame` against it — exactly as Phase 0 does. The worklet never tries to reconcile `ctx.currentTime` with the engine frame on the hot path.

## Interfaces

- **`Engine.start()/stop()` ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).** `start()` ([04](04-web-engine-impl.md)) calls `startAudioHost` (within a gesture); `stop()` disconnects the node and `ctx.suspend()`/`close()`. The worklet's `process()` is the running render loop in between.
- **WASM API ([01](01-wasm-compilation.md)).** The processor calls `WasmEngine.render(frames) -> ptr` and reads `frames*channels` interleaved f32 from WASM linear memory. Construction passes `(sampleRate, channels, ringPtr, ringLen)`.
- **SAB ring ([03](03-sharedarraybuffer-ring.md)).** The processor's engine consumes from the SAB; the main-thread scheduler produces into it. The worklet never touches head/tail in JS — the WASM consumer does, with `Atomics`, inside `render`.
- **Parity with native.** `process()` ⇔ cpal callback; `render(128)` ⇔ `core.render(out)`; both drain the same 32-byte ring and advance the same frame clock.

## Edge cases & risks

- **No `fetch` / no DOM in the worklet.** `process()` and the processor scope cannot fetch or read the DOM. *Guardrail:* all I/O (WASM bytes, sample PCM) is done on the **main thread** and handed in via `port` / SAB; the worklet only computes.
- **WASM-in-worklet loading.** The module must be instantiated **before** the first real `process()`. *Guardrail:* `initSync` from bytes in the init message; `process()` returns `true` (silence) until `ready`; the main thread `await`s the `"ready"` message before the scheduler starts pushing events.
- **Autoplay / user gesture.** `AudioContext` starts suspended without a gesture. *Guardrail:* `start()` must be invoked from a click/tap handler and `await ctx.resume()`; surface a "click to start" affordance in the example page.
- **Memory growth invalidates views.** If WASM memory grows (e.g. a `loadSample`), any cached `Float32Array` over `memory.buffer` is detached. *Guardrail:* never grow memory during playback on the audio thread; `loadSample` grows between quanta on the main thread → on the next `process()` re-create the memory view (or always create it fresh from the returned `ptr`, which is cheap).
- **Quantum is 128 frames, not configurable.** Unlike a native buffer size, the worklet quantum is fixed. *Guardrail:* lookahead (~100 ms) absorbs this; the ring is sized for » one lookahead window of events ([event-protocol.md](../../event-protocol.md#the-event-ring-buffer)); underruns are still counted.
- **Cross-thread SAB requires isolation.** `process()` reading a `SharedArrayBuffer` requires the page be cross-origin isolated (COOP/COEP) — covered in [03](03-sharedarraybuffer-ring.md). Without it, fall back to the `postMessage` batch path ([03](03-sharedarraybuffer-ring.md)), which the processor receives on `port.onmessage` and copies into a local ring (still no fetch/alloc on the hot path beyond the copy).

## Acceptance criteria

- [ ] **The worklet instantiates WASM at construction, never in `process()`.** *Test:* code review + a runtime assert that `process()` performs no `WebAssembly.instantiate`/`fetch`; the processor posts `"ready"` only after `initSync`.
- [ ] **`process()` fills 128-frame output from `render`.** *Test:* feed a single `NoteOn` via the ring at a known frame; capture node output (an `OfflineAudioContext` or a capturing node) and assert a kick transient at the expected sample offset.
- [ ] **Audio starts only on a user gesture.** *Test:* calling `start()` outside a gesture leaves `ctx.state === "suspended"` and produces silence; calling it from a click resumes and plays.
- [ ] **No allocation on the hot path.** *Test:* run for 60 s; the audio thread does no per-quantum JS allocation (the de-interleave view is reused or aliases WASM memory) and `stats()` shows zero underruns.
- [ ] **Frame counter is the timing source.** *Test:* schedule four kicks at exact frames; assert they land at sample-accurate offsets regardless of `ctx.currentTime` jitter (golden-compare against the native render of the same event stream; [04](04-web-engine-impl.md)).
- [ ] **`stop()` tears down cleanly.** *Test:* `stop()` disconnects the node, suspends the context, and `process()` is no longer called; re-`start()` works.
