# Phase 6.03 — The SharedArrayBuffer event ring (`packages/tekno/src/web/sab-ring.ts`)

## Purpose

This feature implements the event ring — the lock-free SPSC queue that carries timed musical events from the control plane to the audio plane ([architecture.md](../../architecture.md#the-core-principle-two-planes)) — as a **`SharedArrayBuffer` (SAB)** shared between the **main thread (producer: the TS scheduler)** and the **AudioWorklet (consumer: the WASM engine)**. It is the browser realization of the exact statement in [event-protocol.md](../../event-protocol.md#the-event-ring-buffer):

> In the web backend, the `SharedArrayBuffer` is shared with the AudioWorklet directly.

The whole point is **zero translation above the bridge**: the SAB holds the **same fixed 32-byte records, same little-endian layout, same head/tail atomic indices, same SPSC semantics** as the native Rust ring ([phase-0/02-event-ring-buffer.md](../phase-0/02-event-ring-buffer.md)). The producer (`DataView` writes; identical to [event-protocol.md](../../event-protocol.md#encoding-in-typescript)) and the consumer (the WASM `bytemuck` cast; [01](01-wasm-compilation.md)) are the native ones, just pointed at SAB-backed memory.

Using a `SharedArrayBuffer` across threads requires the page to be **cross-origin isolated** (the **COOP/COEP** headers below). Where that is unavailable, this feature defines the **`postMessage` batch fallback** so the engine still runs (with slightly more latency/jitter) — keeping the `Engine` interface unchanged either way.

## Location

```
packages/tekno/src/web/
  sab-ring.ts          # NEW — SAB allocation, layout, Atomics producer, fallback producer
  sab-ring.test.ts     # NEW — round-trip + wrap + full + concurrency tests
  ring-layout.ts       # NEW — shared offsets/constants (REC=32, header layout) used by 01/04 too
```

The consumer side lives in Rust ([01](01-wasm-compilation.md), inside `WasmEngine`/`EngineCore`). The scheduler that calls the producer is the Phase 0/2 scheduler ([phase-0/05-lookahead-scheduler.md](../phase-0/05-lookahead-scheduler.md), [phase-2/06-scheduler-integration.md](../phase-2/06-scheduler-integration.md)) via the web `Engine.pushEvents` ([04](04-web-engine-impl.md)).

## Design

### Layout: header + records, the same 32-byte payload

The SAB is one contiguous buffer: a small fixed header (atomic indices + a stats block) followed by the ring of fixed 32-byte records.

```
SharedArrayBuffer
┌──────────── header (cache-line aligned) ─────────────┬──── ring of N×32-byte records ────┐
│ [0]  head   (u32, Atomics)   — consumer's read index │  rec0 (32B) rec1 (32B) ... recN-1  │
│ [1]  tail   (u32, Atomics)   — producer's write index│  ← SAME layout as event-protocol  │
│ [2]  capacity (u32)          — N (power of two)       │     0:ver 1:op 2:track 4:frame ... │
│ [3]  dropped (u32, Atomics)  — ring-full drop counter │                                    │
│ [4]  currentFrame_lo (u32)   — engine frame, written  │                                    │
│ [5]  currentFrame_hi (u32)     by the worklet         │                                    │
│ [6]  underruns (u32, Atomics)                          │                                    │
│ [7]  lateEvents (u32, Atomics)                         │                                    │
└───────────────────────────────────────────────────────┴────────────────────────────────────┘
```

```ts
// packages/tekno/src/web/ring-layout.ts
export const REC = 32;                          // bytes per record — SAME as native/event-protocol
export const HEADER_U32 = 16;                   // 16 u32 slots of header (room to spare)
export const HEADER_BYTES = HEADER_U32 * 4;
// header slot indices (Int32Array over the SAB)
export const H_HEAD = 0, H_TAIL = 1, H_CAP = 2, H_DROPPED = 3,
             H_FRAME_LO = 4, H_FRAME_HI = 5, H_UNDERRUNS = 6, H_LATE = 7;

export function ringByteLength(capacityRecords: number): number {
  return HEADER_BYTES + capacityRecords * REC;  // capacity must be a power of two
}
```

The header doubles as the **stats channel**: the worklet writes `currentFrame`/`underruns`/`lateEvents` here each quantum, and the main thread reads them for `stats()` ([04](04-web-engine-impl.md)) — no `postMessage` on the hot path.

### Producer (main thread) — Atomics, identical encoding

The producer is the scheduler's `pushEvents`: it writes N×32-byte records starting at `tail`, then publishes the new `tail` with a release store. The **record encoding is byte-for-byte the `DataView` writer from [event-protocol.md](../../event-protocol.md#encoding-in-typescript)** — only the destination is the SAB.

```ts
// packages/tekno/src/web/sab-ring.ts
export class SabRingProducer {
  private head32: Int32Array;     // Atomics view over header
  private bytes: Uint8Array;      // raw bytes for record region
  private view: DataView;         // for record fields (matches event-protocol writer)
  readonly capacity: number;      // power of two
  private readonly mask: number;

  constructor(public readonly sab: SharedArrayBuffer, capacity: number) {
    this.head32 = new Int32Array(sab, 0, HEADER_U32);
    Atomics.store(this.head32, H_CAP, capacity);
    this.capacity = capacity; this.mask = capacity - 1;
    this.bytes = new Uint8Array(sab, HEADER_BYTES);
    this.view  = new DataView(sab, HEADER_BYTES);
  }

  /** Push a whole encoded batch (the scheduler's reusable ArrayBuffer of M×32 bytes). */
  pushEvents(buffer: ArrayBuffer): void {
    const src = new Uint8Array(buffer);
    const m = (src.byteLength / REC) | 0;
    const tail = Atomics.load(this.head32, H_TAIL);          // producer owns tail
    const head = Atomics.load(this.head32, H_HEAD);          // acquire consumer's head
    const free = this.capacity - (tail - head);              // unsigned ring distance
    const n = Math.min(m, free);
    for (let i = 0; i < n; i++) {
      const slot = (tail + i) & this.mask;
      this.bytes.set(src.subarray(i * REC, i * REC + REC), slot * REC);
    }
    // Publish: release store so the consumer sees record bytes before the new tail.
    Atomics.store(this.head32, H_TAIL, tail + n);
    if (n < m) Atomics.add(this.head32, H_DROPPED, m - n);   // full → drop+count, never block
  }

  stats() {
    const lo = Atomics.load(this.head32, H_FRAME_LO) >>> 0;
    const hi = Atomics.load(this.head32, H_FRAME_HI) >>> 0;
    return {
      currentFrame: hi * 2 ** 32 + lo,
      underruns:    Atomics.load(this.head32, H_UNDERRUNS) >>> 0,
      lateEvents:   Atomics.load(this.head32, H_LATE) >>> 0,
      ringFullness: (Atomics.load(this.head32, H_TAIL) - Atomics.load(this.head32, H_HEAD)) / this.capacity,
      dropped:      Atomics.load(this.head32, H_DROPPED) >>> 0,
    };
  }
}
```

> `tail`/`head` are monotonically increasing `u32` counters; the slot is `index & mask`. Free space is `capacity - (tail - head)` in unsigned arithmetic — the same SPSC trick the native ring uses ([phase-0/02-event-ring-buffer.md](../phase-0/02-event-ring-buffer.md)). The producer reads `head` with an acquire-equivalent `Atomics.load` and publishes `tail` with a release-equivalent `Atomics.store`; one writer, so no CAS needed.

### Consumer (worklet / WASM) — the Phase 0 drain, on SAB memory

The consumer is **already implemented**: it is the native ring consumer ([phase-0/02-event-ring-buffer.md](../phase-0/02-event-ring-buffer.md)) compiled to WASM ([01](01-wasm-compilation.md)), reading head/tail as `AtomicU32` at the header offsets and casting each 32-byte slot to `EventRecord` with `bytemuck`. Inside `render`:

```rust
// engine/src/scheduler.rs (web host: ring lives in the SAB region at ring_ptr)
fn drain_ring_until(&mut self, limit_frame: u64) {
    let head = self.head.load(Ordering::Acquire);     // header H_HEAD
    let tail = self.tail.load(Ordering::Acquire);     // header H_TAIL (published by producer)
    let mut i = head;
    while i != tail {
        let rec = self.record_at(i & self.mask);      // bytemuck cast of 32 bytes — SAME layout
        if (rec.frame as u64) > limit_frame { break } // events stay frame-ordered; stop at horizon
        self.apply(rec);                              // NoteOn/ParamSet/... → voices/params
        i = i.wrapping_add(1);
    }
    self.head.store(i, Ordering::Release);            // header H_HEAD: release consumed slots
    // also writes currentFrame/underruns/lateEvents into the header stats slots
}
```

The Rust side uses `Ordering::Acquire`/`Release`; the JS side uses `Atomics.load`/`Atomics.store` (sequentially consistent), which interoperate correctly over the shared memory.

### COOP/COEP — enabling SharedArrayBuffer

`SharedArrayBuffer` (and `Atomics.wait`) are **only available on cross-origin-isolated pages**. The server hosting the app/example **must** send:

```
Cross-Origin-Opener-Policy:   same-origin
Cross-Origin-Embedder-Policy:  require-corp
```

and every sub-resource (the `.wasm`, the worklet `.js`, samples fetched cross-origin) must be CORP/CORS-eligible (`Cross-Origin-Resource-Policy: same-origin` / proper CORS). On a correctly isolated page, `self.crossOriginIsolated === true` and `SharedArrayBuffer` is constructible. The Phase 6 example dev server ([04](04-web-engine-impl.md)) sets these headers; production deployments must too. A `COOP/COEP` service-worker shim is the documented fallback for static hosts that cannot set headers directly.

### Fallback: `postMessage` batch (no SAB)

If `!self.crossOriginIsolated` (or `typeof SharedArrayBuffer === "undefined"`), `pushEvents` cannot share memory. The fallback keeps the **same `Engine` interface and the same 32-byte encoding**, but ships each batch to the worklet via `node.port.postMessage(buffer, [buffer])` (transferring the `ArrayBuffer`). The processor copies the transferred bytes into a **local (non-shared) ring** it owns, and `render` drains that ring identically.

```ts
// sab-ring.ts — chosen by the web engine [04] at start()
export class PostMessageProducer {
  constructor(private port: MessagePort) {}
  pushEvents(buffer: ArrayBuffer): void {
    // Copy out, because the scheduler reuses its ArrayBuffer each tick; transfer the copy.
    const copy = buffer.slice(0);
    this.port.postMessage({ type: "events", buf: copy }, [copy]);
  }
  // stats() comes back via a periodic port message instead of header reads.
}
```

Tradeoff (documented, matches the architecture's jitter tolerance): `postMessage` crosses the thread boundary with more latency/jitter than SAB, so it needs a slightly larger lookahead, but it is still *ahead of time*, so it stays glitch-free. SAB is strongly preferred and is the default whenever isolation is present.

## Interfaces

- **`Engine.pushEvents(buffer: ArrayBuffer)` ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).** The web engine ([04](04-web-engine-impl.md)) routes `pushEvents` to `SabRingProducer.pushEvents` (or the fallback). Same signature, same call-once-per-tick contract as native — the scheduler is unchanged.
- **The 32-byte protocol ([event-protocol.md](../../event-protocol.md#event-record-layout)).** Records in the SAB are bit-identical to native; the producer is the `DataView` writer from the protocol doc, the consumer is the `bytemuck` reader from Phase 0.
- **Stats channel.** `stats()` fields (`currentFrame`, `underruns`, `lateEvents`, `ringFullness`) are read from header atomics (SAB path) or from periodic port messages (fallback), surfacing the same `EngineStats` shape ([04](04-web-engine-impl.md)).
- **Parity with native.** Same SPSC discipline (single producer, single consumer, monotonic head/tail, full → drop+count). The only difference from native is *where the bytes live* (SAB vs Rust heap).

## Edge cases & risks

- **SAB unavailable (no COOP/COEP).** *Guardrail:* feature-detect `crossOriginIsolated`/`SharedArrayBuffer`; transparently use the `postMessage` fallback; log a one-time warning recommending the isolation headers. The `Engine` interface is identical either way.
- **Missing/incorrect headers in production.** *Guardrail:* document COOP/COEP as a hard deploy requirement; ship the example dev server with them set and a service-worker shim recipe for header-less static hosts.
- **Memory ordering bugs.** A producer that publishes `tail` before the record bytes are written would let the consumer read garbage. *Guardrail:* always write record bytes *then* `Atomics.store` the new `tail` (release); consumer loads `tail` (acquire) before reading slots. Covered by a stress/concurrency test.
- **Ring full / backpressure.** Sized for » one lookahead window ([event-protocol.md](../../event-protocol.md#the-event-ring-buffer)); on full, drop + increment `H_DROPPED`, never block the scheduler — identical to native.
- **`SharedArrayBuffer` accidentally transferred.** Listing the SAB in a `postMessage` transfer list would detach it. *Guardrail:* in the SAB path the buffer is **shared, never transferred** (only the fallback transfers a *copy* of a plain `ArrayBuffer`). Asserted in [02](02-audioworklet-host.md)'s init.
- **u64 frame split across two u32 slots.** Reading `lo`/`hi` non-atomically could tear at the ~once-per-eternity wrap. *Guardrail:* frame is monotonic and only the worklet writes it; reader tolerates a one-quantum-stale value (it is advisory for scheduling), and the rare tear is below audible relevance; if needed use a seqlock around the two slots.

## Acceptance criteria

- [ ] **Round-trip: producer → SAB → WASM consumer.** *Test:* write a known `NoteOn` batch via `SabRingProducer.pushEvents`; run one `render`; assert the WASM consumer applied exactly those events (golden output) with the **same 32-byte bytes** the native ring would carry.
- [ ] **Wrap-around correctness.** *Test:* push across the capacity boundary repeatedly; assert no record is lost or duplicated and `head`/`tail` stay consistent.
- [ ] **Full → drop + count, never block.** *Test:* push far past capacity while the consumer is paused; assert `pushEvents` returns immediately and `H_DROPPED` (via `stats().dropped`) increments by the overflow count.
- [ ] **Cross-origin isolation gates SAB.** *Test:* on a COOP/COEP page assert `crossOriginIsolated === true` and the SAB path is selected; on a non-isolated page assert the `postMessage` fallback is selected and still plays.
- [ ] **Stats header reflects the worklet.** *Test:* after running, `stats().currentFrame` advances ≈ elapsed·sampleRate and `underruns === 0` under normal load — read from the SAB header, matching native `stats()`.
- [ ] **Concurrency stress.** *Test:* a tight producer loop on the main thread against a real worklet consumer for 60 s shows zero corrupted records and zero underruns.
