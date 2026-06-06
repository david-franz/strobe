# Event protocol

This is the contract between the TypeScript control plane and the Rust audio engine: the bytes that cross the bridge, and the rules for crossing it. It is intentionally small, fixed-layout, and allocation-free on the hot path.

## Goals

1. **Allocation-free on the audio thread.** The consumer reads fixed-size records straight out of the ring buffer; it never parses variable-length data or allocates.
2. **Sample-accurate.** Every event carries the exact audio frame it takes effect on.
3. **Backend-agnostic.** The same encoding works for the native (napi-rs) and web (WASM/AudioWorklet) backends.
4. **Forward-compatible.** A version byte and a typed opcode let us add event types without breaking older engines.

## Two channels

Data crosses the bridge on two distinct paths, because they have different real-time requirements:

| Channel | Direction | Carries | Real-time? |
| --- | --- | --- | --- |
| **Event ring** | TS → audio thread | timed musical events (note on/off, param set) | **Yes** — lock-free SPSC ring buffer |
| **Control calls** | TS ↔ engine (setup thread) | load a sample buffer, define an instrument, query status | No — ordinary async function calls |

Only the **event ring** is on the hot path. Control calls (e.g. uploading a decoded sample, registering a `fundsp` voice config) go through normal napi-rs/wasm-bindgen function calls on a non-audio thread, and the engine integrates their results between buffers.

## The event ring buffer

- A pre-allocated, fixed-capacity SPSC ring of fixed-size event records.
- **Producer:** the TS scheduler (one writer).
- **Consumer:** the audio thread (one reader).
- No locks. Head/tail are atomic indices. Sized to comfortably exceed one lookahead window of events (see [architecture.md](architecture.md#the-look-ahead-scheduler-two-clocks)).
- If the producer ever finds it full, it drops the event and raises a warning counter (surfaced to TS) rather than blocking the scheduler.

In the native backend, TS writes into a `SharedArrayBuffer`/typed-array view that maps onto the Rust ring's memory (or via a thin napi push call in Phase 0; see "Phase 0 simplification" below). In the web backend, the `SharedArrayBuffer` is shared with the AudioWorklet directly.

## Event record layout

Every event is a fixed **32-byte** record, little-endian:

```
offset  size  field        notes
──────  ────  ───────────  ─────────────────────────────────────────────
  0      1    version      protocol version (currently 1)
  1      1    opcode       event type (see table)
  2      2    track        track id (p1=1 … ); 0 = master/global
  4      4    frame        u32 audio-frame timestamp (when to apply), engine-relative
  8      4    voice/param  u32: voice id (NoteOn/Off) or param id (ParamSet)
 12      4    arg0         f32 — primary value (freq, gain, cutoff, …)
 16      4    arg1         f32 — secondary (e.g. velocity, resonance)
 20      4    arg2         f32 — tertiary (e.g. duration in frames, pan)
 24      4    arg3         f32 — quaternary / instrument or sample id
 28      4    flags/seq    u32 — bit flags + a sequence counter
```

32 bytes is one cache-line-friendly record and covers every event type without variable length. Unused args are zero.

### Opcodes

| Opcode | Name | Meaning | Key fields |
| --- | --- | --- | --- |
| `0x01` | `NoteOn` | start a voice | `arg0`=freq(Hz), `arg1`=velocity(0..1), `arg2`=duration(frames), `arg3`=instrument id |
| `0x02` | `NoteOff` | release a voice | `voice` = voice id to release |
| `0x10` | `ParamSet` | set a parameter immediately | `param` id, `arg0` = value |
| `0x11` | `ParamRamp` | ramp a parameter linearly | `param` id, `arg0`=target, `arg2`=ramp-frames |
| `0x20` | `TrigSample` | trigger a loaded sample | `arg0`=speed/pitch, `arg1`=gain, `arg2`=start(frames), `arg3`=sample id |
| `0x30` | `TrackFx` | set/route a track effect param | `param` id encodes (fx-slot, fx-param), `arg0`=value |
| `0x40` | `Tempo` | update cps (rarely; for sync) | `arg0` = cps |
| `0x7F` | `Nop` | padding / heartbeat | — |

> Most chained methods in the song API (`.lpf()`, `.gain()`, `.room()`) compile to either fields packed into a `NoteOn`/`TrigSample`, or to `ParamSet`/`TrackFx` events emitted alongside the note. The scheduler decides per parameter whether it is per-note (packed) or per-track (separate event).

### The `frame` timestamp

- `frame` is the engine's running sample counter at which the event applies.
- The scheduler computes it from cycle-time: `frame = round((eventTimeSeconds) * sampleRate)`, offset by the engine's frame at scheduling time.
- The audio thread drains all events with `frame <= currentFrame + bufferLen`, applying each at its exact offset within the buffer.
- An event that arrives with `frame < currentFrame` (i.e. late) is applied immediately and counted as a "late event" warning, never silently dropped musically.

### Voice ids

- For `NoteOn`, TS supplies (in `voice`) a handle it will later use for `NoteOff`. The engine's `voice_alloc` maps that handle to a physical voice and remembers it, so `NoteOff` can find and release the right one even after voice stealing.
- Gated (sustained) notes use `NoteOn` + later `NoteOff`. One-shots (drums, samples) pass a `duration` in `arg2` and need no `NoteOff`.

## Control-call surface (non-real-time)

These are ordinary async methods on the engine handle (napi-rs / wasm-bindgen). They never touch the audio thread directly; they hand results to it via double-buffered, atomically-swapped state.

```ts
interface Engine {
  start(opts?: { sampleRate?: number; bufferFrames?: number }): Promise<void>;
  stop(): Promise<void>;

  // Register an instrument graph (a fundsp voice config) → returns an instrument id.
  defineInstrument(config: InstrumentConfig): Promise<number>;

  // Upload decoded PCM (Float32, interleaved) → returns a sample id.
  loadSample(pcm: Float32Array, channels: number, sampleRate: number): Promise<number>;

  // Push a batch of pre-encoded 32-byte event records into the ring.
  pushEvents(buffer: ArrayBuffer): void;       // hot path; non-blocking

  // Recording (Phase 5): tap the master and/or per-track outputs to disk.
  // Arms lock-free taps + a writer thread; no event opcodes, no audio-thread I/O.
  startCapture(config: CaptureConfig): Promise<void>;
  stopCapture(): Promise<CaptureResult>;       // finalizes files; returns written paths

  // Telemetry for the scheduler (underruns, late events, ring fullness, current frame,
  // and capture_dropped_frames once capturing).
  stats(): EngineStats;
}
```

> The `startCapture`/`stopCapture` pair is the only addition to this surface after Phase 0, and it is purely a control-plane concern — it adds **no event opcodes** and does not change the 32-byte record. The offline backends add a sibling `startOffline(...)` and `replayJournal(...)` used by `tekno render`/`bounce` ([Phase 5 · session recording](plan/phase-5/05-session-recording.md), [audio capture & stems](plan/phase-5/06-audio-capture-stems.md)). The web backend ([Phase 6](plan/phase-6/04-web-engine-impl.md)) may implement or stub capture independently; nothing above the `Engine` interface depends on which backend is active.

`pushEvents` takes an already-encoded `ArrayBuffer` of N×32 bytes so the scheduler batches a whole lookahead window in one call.

## Encoding in TypeScript

The scheduler writes records into a reusable `ArrayBuffer` via a `DataView`, never allocating per event:

```ts
const REC = 32;
function writeNoteOn(view: DataView, i: number, e: {
  track: number; frame: number; voice: number;
  freq: number; vel: number; durFrames: number; instrument: number;
}) {
  const o = i * REC;
  view.setUint8 (o + 0, 1);            // version
  view.setUint8 (o + 1, 0x01);         // NoteOn
  view.setUint16(o + 2, e.track, true);
  view.setUint32(o + 4, e.frame, true);
  view.setUint32(o + 8, e.voice, true);
  view.setFloat32(o + 12, e.freq, true);
  view.setFloat32(o + 16, e.vel, true);
  view.setFloat32(o + 20, e.durFrames, true);
  view.setFloat32(o + 24, e.instrument, true);
  view.setUint32(o + 28, e.seq, true);
}
```

The Rust side reads the mirror layout with `from_le_bytes` (or a `#[repr(C)]` struct + `bytemuck` cast).

## Versioning

- Byte 0 is the protocol version. The engine rejects records whose version it does not understand and reports it through `stats()`.
- New event types take new opcodes; existing layouts are never repurposed. Adding a field means a new opcode or a new record version, never silently overloading an arg.

## Phase 0 simplification

For the initial proof-of-spine, the bridge may use the simplest thing that honors the layout:

- A single `pushEvents(ArrayBuffer)` napi call per scheduler tick, copying into the Rust ring under the hood.
- Only `NoteOn` (one-shot drum) and `Tempo` opcodes implemented.

The zero-copy `SharedArrayBuffer` path and the full opcode set come online in later phases — but the **record layout above is fixed from Phase 0**, so nothing above the bridge changes when the transport is optimized.
