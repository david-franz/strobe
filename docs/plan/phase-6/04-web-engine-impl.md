# Phase 6.04 — The web `Engine` implementation (`packages/tekno/src/engine.web.ts`)

## Purpose

This feature is the **second implementation of the same `Engine` interface** the native addon satisfies — the one the entire pattern engine and song API sit above ([architecture.md](../../architecture.md#the-bridge-ts--rust), [event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)). It ties the three preceding features together — WASM core ([01](01-wasm-compilation.md)), AudioWorklet host ([02](02-audioworklet-host.md)), SAB ring ([03](03-sharedarraybuffer-ring.md)) — into a `createWebEngine()` that exposes **`start`/`stop`/`loadSample`/`pushEvents`/`stats`**, structurally identical to `createNativeEngine()` ([phase-0/04-napi-bridge.md](../phase-0/04-napi-bridge.md)). It also defines **runtime backend selection** (native vs web) and the **parity harness** that proves the **README quickstart song runs UNCHANGED** in the browser — the entire payoff of Phase 6.

The guiding invariant: **everything above this interface is byte-for-byte the same code** as the native path — the cps clock, patterns, mini-notation, combinators, `sound()/note()`, and the look-ahead scheduler. Moving to the browser swaps `createNativeEngine` for `createWebEngine` and **changes no song code** ([architecture.md](../../architecture.md#the-bridge-ts--rust)).

## Location

```
packages/tekno/src/
  engine.ts            # (Phase 0) the Engine interface + createNativeEngine; ADD createEngine() selector
  engine.web.ts        # NEW — class WebEngine implements Engine (wires 01/02/03)
  web/
    audio-host.ts      # (02) AudioContext + AudioWorkletNode + gesture
    sab-ring.ts        # (03) SabRingProducer / PostMessageProducer
    sample-load.ts     # NEW — fetch + decodeAudioData (or symphonia-wasm) → Float32 PCM
  engine.web.test.ts   # NEW — interface parity + quickstart-parity harness
examples/web/
  index.html           # NEW — "click to start" page that plays the README quickstart
  song.ts              # NEW — IMPORTS THE EXACT README SNIPPET, unchanged
  server.mjs           # NEW — static server that sends COOP/COEP headers [03]
```

This document owns `engine.web.ts`, `web/sample-load.ts`, the `createEngine()` selector, and `examples/web/`.

## Design

### `WebEngine implements Engine`

```ts
// packages/tekno/src/engine.ts  (the shared contract — unchanged from Phase 0, plus loadSample)
export interface Engine {
  start(opts?: { sampleRate?: number; bufferFrames?: number }): Promise<void>;
  stop(): Promise<void>;
  loadSample(pcm: Float32Array, channels: number, sampleRate: number): Promise<number>;
  pushEvents(buffer: ArrayBuffer): void;   // hot path; non-blocking; once per tick
  stats(): EngineStats;
}
```

```ts
// packages/tekno/src/engine.web.ts
import { startAudioHost, AudioHost } from "./web/audio-host.js";          // [02]
import { SabRingProducer, PostMessageProducer, ringByteLength } from "./web/sab-ring.js"; // [03]
import { decodeToPcm } from "./web/sample-load.js";

export interface WebEngineOpts {
  wasmUrl: string;       // pkg/tekno_web_bg.wasm
  workletUrl: string;    // worklet/tekno-processor.js
  ringCapacity?: number; // records; default sized > one lookahead window
  channels?: number;     // default 2
}

export class WebEngine implements Engine {
  private host?: AudioHost;
  private producer?: SabRingProducer | PostMessageProducer;
  private sab?: SharedArrayBuffer;

  constructor(private opts: WebEngineOpts) {}

  async start(o?: { sampleRate?: number; bufferFrames?: number }): Promise<void> {
    const channels = this.opts.channels ?? 2;
    const capacity = this.opts.ringCapacity ?? 8192;        // power of two; » lookahead window
    const isolated = typeof SharedArrayBuffer !== "undefined" && (globalThis as any).crossOriginIsolated;

    if (isolated) {
      this.sab = new SharedArrayBuffer(ringByteLength(capacity));
      this.host = await startAudioHost({                    // [02] — must be in a user gesture
        wasmUrl: this.opts.wasmUrl, workletUrl: this.opts.workletUrl,
        ringSab: this.sab, ringPtr: HEADER_BYTES, ringLen: capacity, channels,
      });
      this.producer = new SabRingProducer(this.sab, capacity);
    } else {
      // fallback: still start the worklet, but ship events via postMessage [03]
      this.host = await startAudioHost({ /* …, ringSab omitted → worklet uses local ring */ });
      this.producer = new PostMessageProducer(this.host.node.port);
    }
  }

  async stop(): Promise<void> {
    await this.host?.ctx.suspend();
    this.host?.node.disconnect();
    await this.host?.ctx.close();
    this.host = undefined; this.producer = undefined;
  }

  // Hot path — identical signature & contract to native. Just routes to the ring producer. [03]
  pushEvents(buffer: ArrayBuffer): void { this.producer!.pushEvents(buffer); }

  // Non-real-time control call. Decoding happens BEFORE this; here we only upload PCM. 
  async loadSample(pcm: Float32Array, channels: number, sampleRate: number): Promise<number> {
    // Cross the bridge: copy PCM into WASM memory and register a sample id (between quanta). [01]
    return this.host!.uploadSample(pcm, channels, sampleRate);   // → WasmEngine.load_sample
  }

  stats(): EngineStats {
    const s = this.producer!.stats();          // header atomics (SAB) or last port message (fallback) [03]
    return { currentFrame: s.currentFrame, underruns: s.underruns,
             lateEvents: s.lateEvents, ringFullness: s.ringFullness };
  }
}

export function createWebEngine(opts: WebEngineOpts): Engine { return new WebEngine(opts); }
```

The scheduler calls `engine.pushEvents(buffer)` once per ~25 ms tick and reads `engine.stats().currentFrame` to align frames — **exactly as against the native engine** ([phase-0/05-lookahead-scheduler.md](../phase-0/05-lookahead-scheduler.md)). The web engine simply forwards into the SAB ring; the worklet's WASM consumer drains it.

### Backend selection at runtime

`packages/tekno` picks the backend without song code knowing which one:

```ts
// packages/tekno/src/engine.ts
export function createEngine(opts?: { web?: WebEngineOpts }): Engine {
  const isBrowser = typeof window !== "undefined" && typeof AudioContext !== "undefined";
  if (isBrowser) {
    if (!opts?.web) throw new Error("web backend needs wasm/worklet URLs");
    return createWebEngine(opts.web);          // [this file]
  }
  return createNativeEngine();                  // Node → napi addon [phase-0/04]
}
```

> Detection is environment-based (`window`/`AudioContext` ⇒ browser ⇒ web backend; otherwise Node ⇒ native). The song file calls neither directly — the runtime ([phase-5](../phase-5/00-overview.md), and the future click-to-code frontend) calls `createEngine()` and hands the result to the scheduler. **The song imports `setcps`, `p1`, `sound`, `note`, `sine` from `tekno` and never mentions a backend.**

### `loadSample` in the browser: fetch + decode on the main thread

The audio thread cannot `fetch` or decode ([02](02-audioworklet-host.md)), so sample ingest is a **main-thread, non-real-time** path that produces `Float32Array` PCM and then calls the same `loadSample(pcm, channels, sampleRate)` the native engine exposes ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).

```ts
// packages/tekno/src/web/sample-load.ts
export async function decodeToPcm(url: string, ctx: AudioContext): Promise<{
  pcm: Float32Array; channels: number; sampleRate: number;
}> {
  const buf = await fetch(url).then(r => r.arrayBuffer());     // main thread only
  // Path A (default): the browser's native decoder.
  const audio = await ctx.decodeAudioData(buf);
  const channels = audio.numberOfChannels, sr = audio.sampleRate, n = audio.length;
  const pcm = new Float32Array(n * channels);                  // interleave for the engine
  for (let ch = 0; ch < channels; ch++) {
    const data = audio.getChannelData(ch);
    for (let i = 0; i < n; i++) pcm[i * channels + ch] = data[i];
  }
  return { pcm, channels, sampleRate: sr };
  // Path B (Phase 4 alt): symphonia compiled to WASM decodes `buf` → same interleaved PCM,
  // for formats/control parity with the native decode. Kept behind the same return shape.
}
```

The decoded interleaved `Float32Array` matches exactly what `Engine.loadSample` expects on **both** backends, so the sampling song code ([phase-4](../phase-4/00-overview.md)) is unchanged; only the *ingest* differs (`yt-dlp`/`ffmpeg`+`symphonia` natively vs `fetch`+`decodeAudioData` in the browser).

### Proving parity — the README quickstart runs UNCHANGED

`examples/web/song.ts` is the **literal README snippet**, imported as-is:

```ts
// examples/web/song.ts  — byte-for-byte the README quickstart; NO web-specific code
import { setcps, p1, p2, p3, sound, note, sine } from "tekno";
setcps(130 / 60 / 4);
p1( sound("bd*4").gain(0.9) );
p2( sound("~ hh ~ hh").lpf(6000) );
p3( note("c2 c2 eb2 g2").saw()
      .lpf(sine.range(300, 1200).slow(8))
      .room(0.3)
      .sidechain("p1") );
```

```html
<!-- examples/web/index.html -->
<button id="start">click to start</button>
<script type="module">
  import { createEngine, attachScheduler } from "tekno";        // runtime glue
  document.getElementById("start").onclick = async () => {       // user gesture → autoplay [02]
    const engine = createEngine({ web: { wasmUrl: "/pkg/tekno_web_bg.wasm",
                                         workletUrl: "/worklet/tekno-processor.js" } });
    await engine.start();
    attachScheduler(engine);          // the SAME scheduler the native runtime uses
    await import("./song.ts");        // run the unchanged quickstart
  };
</script>
```

Parity is enforced two ways:
1. **Interface parity (compile-time).** `WebEngine` must satisfy `Engine` with no `any` escapes; a test asserts `createWebEngine` and `createNativeEngine` return the same structural type.
2. **Render parity (golden).** Feed an identical event stream (the quickstart's first 8 cycles, frame-stamped) to the native engine and to the WASM `render`; assert the output buffers match within DSP null-test tolerance (SIMD/threads must not change output; [01](01-wasm-compilation.md)). This proves "same engine" literally — same `EngineCore`, same samples.

### COOP/COEP example server

`examples/web/server.mjs` is a tiny static server that sets the cross-origin-isolation headers ([03](03-sharedarraybuffer-ring.md)) so SAB is available:

```js
// examples/web/server.mjs (excerpt)
res.setHeader("Cross-Origin-Opener-Policy",  "same-origin");
res.setHeader("Cross-Origin-Embedder-Policy", "require-corp");
res.setHeader("Cross-Origin-Resource-Policy", "same-origin");
```

## Interfaces

- **The `Engine` interface ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).** `WebEngine` implements **the same** `start`/`stop`/`loadSample`/`pushEvents`/`stats` the native addon does. The scheduler, tracks, and song API target this interface and are reused unchanged.
- **The 32-byte protocol ([event-protocol.md](../../event-protocol.md#event-record-layout)).** `pushEvents` forwards the scheduler's already-encoded N×32-byte `ArrayBuffer` straight into the SAB ring ([03](03-sharedarraybuffer-ring.md)); no re-encoding, identical bytes to native.
- **To [01](01-wasm-compilation.md):** `loadSample` → `WasmEngine.load_sample`; `stats()` reads the header atomics the worklet writes.
- **To [02](02-audioworklet-host.md):** `start`/`stop` create/tear down the `AudioContext` + worklet node; `start` must run in a user gesture.
- **To [03](03-sharedarraybuffer-ring.md):** chooses `SabRingProducer` (isolated) or `PostMessageProducer` (fallback); same `pushEvents` signature either way.
- **Parity with native ([phase-0/04-napi-bridge.md](../phase-0/04-napi-bridge.md)):** same interface, same `EngineStats` shape, same per-tick contract — the runtime swaps backends with one factory call.

## Edge cases & risks

- **`start()` outside a user gesture.** Autoplay policy leaves the context suspended. *Guardrail:* document/enforce that `start()` is invoked from a click handler; the example uses a "click to start" button ([02](02-audioworklet-host.md)).
- **Sample fetch is async / cross-origin.** `decodeToPcm` runs on the main thread (never the audio thread) and may hit CORS/CORP under cross-origin isolation. *Guardrail:* samples must be CORP/CORS-eligible; surface a clear error; decoding completes before any `TrigSample` event references the sample id.
- **Memory growth on `loadSample`.** Uploading PCM may grow WASM memory; doing so on the audio thread is forbidden. *Guardrail:* `loadSample` runs between quanta on the main thread; the worklet re-creates its memory view after a grow ([02](02-audioworklet-host.md)).
- **Backend mis-selection.** A bundler that defines `window` in Node, or SSR, could misroute. *Guardrail:* selection also checks `AudioContext`; allow an explicit override.
- **No SAB (fallback active).** Higher jitter via `postMessage`. *Guardrail:* the lookahead absorbs it (still ahead of time → glitch-free); recommend COOP/COEP for the SAB fast path; `stats()` still reports underruns so regressions are visible.
- **Song code accidentally importing a backend.** Would break the "no song code changes" guarantee. *Guardrail:* a lint/test asserts `examples/web/song.ts` imports **only** from `tekno`'s public song API and is identical to the README snippet.

## Acceptance criteria

- [ ] **`WebEngine` satisfies `Engine` structurally identical to native.** *Test:* TS compiles `engine.web.ts` with no `any`; a type-level test asserts `ReturnType<typeof createWebEngine>` and `ReturnType<typeof createNativeEngine>` are both `Engine`.
- [ ] **README quickstart plays UNCHANGED in a browser.** *Test:* serve `examples/web/` (COOP/COEP on), click start; the four-on-the-floor kick, offbeat hats, and sweeping bassline play. `examples/web/song.ts` is byte-for-byte the README snippet (a string-equality test guards it).
- [ ] **Render parity vs native (golden).** *Test:* identical frame-stamped event stream → native `render` and WASM `render`; output buffers match within null-test tolerance.
- [ ] **Live hot-swap with no dropout.** *Test:* re-run a track (`p2(...)`) live in the browser; it swaps at the next cycle boundary with no click; `stats().underruns === 0` throughout ([roadmap.md](../../roadmap.md#phase-6--web-backend-later)).
- [ ] **`loadSample` via fetch+decode works end-to-end.** *Test:* `decodeToPcm("/kick.wav", ctx)` → `engine.loadSample(pcm, ch, sr)` → a `TrigSample` event triggers it audibly; the PCM shape matches what native `loadSample` accepts.
- [ ] **Backend selection.** *Test:* `createEngine()` returns a `WebEngine` in a jsdom/browser env and a native engine in Node, with no song-code branch.
- [ ] **Clean telemetry.** *Test:* during the quickstart, `stats()` (read from the SAB header) reports zero underruns and zero late events under normal load.
