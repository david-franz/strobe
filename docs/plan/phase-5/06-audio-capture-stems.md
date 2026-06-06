# Phase 5 · Audio capture & stems

## Purpose

This is "hit record": capture the **actual audio** the engine produces to disk while you play — the master mix **and** every channel loop as its own file (stems), so the tracks stay separate for later mixing, remixing, or import into a DAW. It is the immediate, no-extra-step path ("give me the WAV now"), and the **only** path that captures non-deterministic input (a live microphone or external audio), which the deterministic [session recording](05-session-recording.md) cannot reproduce.

The same tap-and-sink machinery is reused by the offline `bounce` ([05](05-session-recording.md)) and `render` ([02](02-cli.md)) so that live capture, a session re-render, and a static bounce all emit identically-structured `master.wav` + `stems/pN.wav` output from one code path.

The hard requirement: **none of this may touch the audio thread's deadline** ([architecture.md](../../architecture.md#the-core-principle-two-planes)). Encoding and disk I/O happen on a separate writer thread; the audio thread only copies samples into pre-allocated lock-free rings.

## Location

```
engine/src/
  capture.rs        # NEW — tap ring buffers + the writer thread + WAV/FLAC encoding
  audio_io.rs       # tap points: per-track (post-FX, pre-master) and master (post-limiter)
bindings/native/
  lib.rs            # NEW control calls: startCapture(config), stopCapture()
packages/strobe/src/record/
  capture.ts        # TS API: record.start()/stop(); live-coding rec()/unrec() helpers
```

## Design

### Tap points (what a stem is)

The engine already sums each track's voices and runs them through that track's effect chain ([Phase 3 · effects framework](../phase-3/01-effects-framework.md)), then sums all tracks into the master bus + limiter ([Phase 1 · master bus](../phase-1/07-master-bus-limiter.md)). Capture inserts taps at exactly those existing boundaries:

```
 track p1 voices → p1 FX chain ─┬─────────────► [stem tap p1] → stems/p1.wav
 track p2 voices → p2 FX chain ─┼─┐                 …
            …                   │ │
                                ▼ ▼
                          master bus + limiter ──► [master tap] → master.wav
                                                 → cpal output (live) / discarded (offline)
```

- **Stems are post-track-FX but pre-master (pre-limiter), by design.** Each stem is the isolated, independently-mixable channel loop — the form you want in a DAW. (Because the master limiter is non-linear, stems will not bit-sum to the master; this is expected and documented. An optional `postMaster` mode can additionally tap each track *after* a master-matched gain if a user wants summing parity.)
- All taps read from the **same frame clock**, so every stem and the master are **sample-aligned** with each other automatically.

### Real-time-safe capture path

```
audio thread (cpal callback)          writer thread (std::thread)
────────────────────────────          ───────────────────────────
for each track + master:              loop:
  copy tapped frames into               for each active ring:
  that tap's SPSC ring   ───────►         drain available frames
  (pre-allocated, no I/O)                 encode → WAV/FLAC writer
                                          fwrite to disk
```

- One **pre-allocated** lock-free SPSC ring per active tap (same primitive as the [event ring](../phase-0/02-event-ring-buffer.md), here carrying `f32` frames). The audio thread does a bounded `memcpy` into each ring and nothing else — no allocation, no locks, no syscalls.
- A dedicated **writer thread** drains the rings and encodes. WAV writing via the `hound` crate; optional FLAC via `flacenc`. Default format is **32-bit float WAV** at the engine sample rate (lossless, matches the engine's internal format exactly).
- If a ring ever fills because the disk stalls, the audio thread drops those frames and bumps a `capture_dropped_frames` counter (surfaced in `stats()`); it **never blocks**. The writer pads the gap to preserve sample alignment, and the runtime warns. (Rings are sized for seconds of slack, so this should not happen under normal load.)

### Control calls (engine surface)

Capture is toggled by non-real-time control calls — it allocates rings/files and flips an `AtomicBool` the audio thread reads. It adds **no event opcodes**.

```ts
interface CaptureConfig {
  dir: string;                       // output directory
  master?: boolean;                  // default true
  stems?: boolean;                   // default false → one file per assigned track
  format?: "wav-f32" | "wav-i24" | "flac";   // default "wav-f32"
  tracks?: number[];                 // optional subset; default = all assigned tracks
}
interface Engine {
  // …existing start/stop/loadSample/pushEvents/stats…
  startCapture(config: CaptureConfig): Promise<void>;
  stopCapture(): Promise<{ master?: string; stems: Record<string, string>; droppedFrames: number }>;
}
```

`startCapture` opens files and arms the taps at the next buffer boundary; `stopCapture` flushes the writer, finalizes WAV headers (back-patches the length field), and returns the written paths. Files are named from `meta.trackNames` → `stems/p1.wav`, `stems/p2.wav`, ….

### TypeScript API & CLI

```ts
// capture.ts
import { record } from "strobe";
record.start({ dir: "takes/2026-06-06", stems: true });   // begin capturing
// …perform / live-code…
const out = await record.stop();    // { master, stems: { p1, p2, … }, droppedFrames }
```

Live-coding helpers `rec()` / `unrec()` wrap these for the REPL. From the CLI ([02](02-cli.md)):

```
strobe play song.ts --rec takes/             # capture master while you perform
strobe play song.ts --rec takes/ --stems     # + one file per channel loop
strobe play song.ts --rec takes/ --session   # also write a .strobesession (doc 06)
```

When `--rec` and `--session` are combined, the runtime enables **both** the audio capture (this doc) and the session tee ([05](05-session-recording.md)) at once — you get the immediate WAVs *and* a re-renderable session.

## Interfaces

- **Engine interface** gains `startCapture`/`stopCapture` (noted in [event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)); the web backend ([Phase 6](../phase-6/04-web-engine-impl.md)) can implement them later via an `OfflineAudioContext`/`MediaRecorder` equivalent, or simply not advertise capture in-browser at first.
- **Tap points** are the per-track post-FX sums ([Phase 3](../phase-3/01-effects-framework.md)) and the master output ([Phase 1](../phase-1/07-master-bus-limiter.md)); capture adds the taps without altering those signal paths.
- **[05 · Session recording](05-session-recording.md)** drives these same sinks during offline `bounce`, so stems from a live take and stems from a re-rendered session are structurally identical.
- **`stats()`** ([Phase 0 · napi bridge](../phase-0/04-napi-bridge.md)) reports `capture_dropped_frames` so the runtime can warn on disk underflow.

## Edge cases & risks

- **Audio-thread safety is non-negotiable.** Only bounded `memcpy` into pre-allocated rings on the audio thread; all encoding/I/O on the writer thread. Reviewed like the event ring.
- **Stems vs master summing.** Stems are pre-limiter and therefore do **not** bit-sum to the master. This is intentional (independently mixable channels) and must be documented in the user guide ([07](07-getting-started-docs.md)); offer the optional `postMaster` tap mode for users who want parity.
- **Disk throughput & size.** Float-WAV stereo is ~11.5 MB/min per file; many stems multiply that. Mitigations: FLAC option, an `i24` option, generously-sized rings, and a clear `droppedFrames` warning if the disk can't keep up.
- **Alignment & latency.** All taps share the frame clock, so stems/master are inherently aligned. If the master limiter has look-ahead latency ([Phase 1](../phase-1/07-master-bus-limiter.md)), either compensate the stem taps by that delay or document the fixed offset so DAW import lines up.
- **Start/stop timing.** Arm/disarm at buffer boundaries to avoid a torn first/last frame; optionally quantize start to the next cycle boundary so a captured loop begins cleanly.
- **Filename collisions.** A `--rec` target that already has files errors unless `--force`, to avoid clobbering a previous take.

## Acceptance criteria

- **Master capture works.** `strobe play song.ts --rec takes/` produces `takes/master.wav` that, on playback, matches what was heard; `stats()` reports zero `capture_dropped_frames` under normal load.
- **Stems come out separated and aligned.** `--stems` writes `master.wav` + `stems/p1.wav…`, one per assigned channel loop. Each stem solos correctly (contains only that track), and importing all stems into a DAW lines them up sample-accurately.
- **No audio-thread regression.** Enabling capture does not introduce underruns: the kebab test from the [Phase 0 recipe](../phase-0/07-hot-reload-demo.md#acceptance-criteria-the-phase-0-verification-recipe) still passes with `--rec --stems` on.
- **Reused by offline bounce.** [05](05-session-recording.md)'s `bounce --stems` emits the same file structure via the same sinks (single code path verified by a shared test).
- **Graceful disk pressure (unit/integration).** Simulating a slow sink makes the audio thread drop-and-count rather than block; the run completes and the warning fires.
- **Cross-platform.** Validated on macOS, then Linux and Windows in CI; WAV headers are correct on all three.
