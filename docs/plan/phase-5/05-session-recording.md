# Phase 5 · Session recording

## Purpose

`strobe render` ([02](02-cli.md)) bounces a **static** song file: it renders the code exactly as it sits, for N cycles, to one master WAV. But a live-coded performance is not a static file — it *changes as you type*. Muting `p2`, sweeping a filter by hand, swapping `p3`'s pattern halfway through: none of that is in the file at the end. **Session recording captures the performance itself** — the realized stream of events over wall-clock time — so the whole take, with every live change in place, can be replayed and re-rendered to pristine audio (master + per-track stems) at any time.

The key enabling fact, from [architecture.md](../../architecture.md) and [event-protocol.md](../../event-protocol.md): the engine is **deterministic**. Its output is a pure function of `(sampleRate, instrument configs, sample buffers, the ordered/timestamped event stream)`. The scheduler already encodes every musical moment into 32-byte event records before calling `pushEvents` — so to record a performance we simply **tee that same byte stream to disk**, alongside the instrument definitions and sample buffers it references. Because we log the *realized* events (not the source code or RNG seeds), re-rendering is exactly reproducible regardless of any randomness in the patterns — the dice have already been rolled and written down.

This is the deterministic, lossless capture path. Its companion, [06 · Audio capture & stems](06-audio-capture-stems.md), captures the literal audio in real time (needed for non-deterministic input like a microphone, and for an immediate WAV without a re-render).

## Location

```
packages/strobe/src/record/
  session.ts        # SessionRecorder — tees pushEvents + control ops to a .strobesession bundle
  replay.ts         # SessionPlayer — drives the engine (offline) from a recorded bundle
  format.ts         # the .strobesession bundle layout + (de)serialization
cli/
  bounce.ts         # NEW `strobe bounce` command → re-render a session to master + stems
```

No engine (Rust) changes are required for *capture* — it is entirely a control-plane tee. Re-render reuses the offline engine mode from [02-cli.md `render`](02-cli.md) plus the stem sinks from [06](06-audio-capture-stems.md).

## Design

### What a session is

A `.strobesession` is a **directory bundle** (a folder, optionally zipped) so that everything needed to reproduce the take is self-contained — crucially the **sample buffers**, which must not depend on an external file or a YouTube cache that may later vanish.

```
my-set.strobesession/
  meta.json            # sampleRate, engine/protocol version, start time, duration (frames), track names
  instruments.json     # every InstrumentConfig registered during the take (id → config)
  samples/
    <sha256>.wav       # each sample buffer loaded during the take, by content hash
  journal.bin          # the ordered, frame-stamped event stream (the performance itself)
```

`journal.bin` is a flat sequence of the **same 32-byte event records** defined in [event-protocol.md](../../event-protocol.md#event-record-layout) — no re-encoding, no translation. The `frame` field in each record is already engine-relative and monotonic, so the journal is self-timing. A small fixed header per flush block records the wall-clock anchor for UI scrubbing, but playback uses only `frame`.

```ts
// format.ts
export interface SessionMeta {
  version: 1;
  sampleRate: number;          // engine rate at record time
  startedAtMs: number;         // wall-clock anchor (UI only)
  durationFrames: number;      // filled in on stop()
  trackNames: Record<number, string>;  // 1:"p1", 2:"p2", … for stem file naming
}
```

### Recording: a tee on the control plane

The recorder wraps the `Engine` handle. Every call the scheduler/runtime already makes is forwarded **and** appended to the journal — there is no second code path producing events, so a recording can never diverge from what was heard.

```ts
// session.ts
export class SessionRecorder {
  constructor(private engine: Engine, private dir: string) {}

  start(): void;                     // open the bundle, snapshot already-registered instruments/samples
  stop(): Promise<void>;             // flush journal, write meta.json (durationFrames), close

  // Tees — the runtime calls these instead of the bare engine when recording is on:
  pushEvents(buf: ArrayBuffer): void {           // append raw 32-byte records, then forward
    this.journal.append(buf);
    this.engine.pushEvents(buf);
  }
  defineInstrument(cfg: InstrumentConfig): Promise<number> { /* log cfg + id, forward */ }
  loadSample(pcm: Float32Array, ch: number, sr: number): Promise<number> {
    // hash the PCM, write samples/<hash>.wav once, map sample id → hash, forward
  }
}
```

Wiring point: the runtime ([01](01-hot-reload-runtime.md)) and scheduler ([Phase 2 · scheduler integration](../phase-2/06-scheduler-integration.md)) hold the `Engine` behind one reference. When recording is enabled, that reference is the `SessionRecorder`, which implements the same surface. Nothing upstream knows the difference. Overhead is a memory copy of the per-tick event buffer (a few KB) plus an append — negligible, and entirely off the audio thread.

### Re-render: deterministic offline bounce

`strobe bounce my-set.strobesession out/ [--stems]` reproduces the take:

```ts
// replay.ts
export async function bounce(sessionDir: string, outDir: string, opts: BounceOptions) {
  const session = await openSession(sessionDir);          // meta + instruments + samples + journal
  const engine  = await createOfflineEngine({ sampleRate: session.meta.sampleRate });

  for (const cfg of session.instruments) await engine.defineInstrument(cfg);
  for (const s of session.samples)       await engine.loadSample(s.pcm, s.channels, s.sampleRate);

  engine.startCapture({ dir: outDir, master: true, stems: opts.stems });  // sinks from doc 07
  await engine.replayJournal(session.journal);            // feed records in frame order, render offline
  await engine.stopCapture();                             // finalize master.wav + stems/pN.wav
}
```

Because this runs the **same** Rust render loop as live playback — just with the audio sink swapped for file sinks and the clock free-running — it produces audio identical to the performance, faster than real time, with zero dropout risk. It is the quality path: even if the live machine glitched, the re-render is clean.

`replayJournal` is an offline control call: it drains the journal in `frame` order, applying each record exactly as the live consumer did ([Phase 0 · event ring buffer](../phase-0/02-event-ring-buffer.md)), advancing the offline clock between events.

## Interfaces

- **Event protocol** — consumes the existing 32-byte records verbatim; **adds no opcodes**. Capture toggles (`startCapture`/`stopCapture`) and `replayJournal` are non-real-time **control calls** on the `Engine` (see [event-protocol.md control-call surface](../../event-protocol.md#control-call-surface-non-real-time)), not per-sample events.
- **[06 · Audio capture & stems](06-audio-capture-stems.md)** — re-render writes through the exact same master/stem sinks, so a bounced session and a live capture produce identically-structured output.
- **[02 · CLI](02-cli.md)** — adds the `bounce` command; `play --rec` (doc 07) can enable session recording in parallel with live audio capture.
- **[Phase 1 · master bus](../phase-1/07-master-bus-limiter.md)** and **[Phase 3 · effects framework](../phase-3/01-effects-framework.md)** — supply the master and per-track tap points the stem sinks read.

## Edge cases & risks

- **Non-deterministic input.** A live microphone / external audio cannot be reproduced from an event journal. Sessions record *synthesized + sampled* material deterministically; for live input, rely on real-time [audio capture](06-audio-capture-stems.md). `meta.json` flags whether live input was present so `bounce` can warn that the re-render omits it.
- **Sample provenance.** Buffers are copied into the bundle by content hash at load time, so the session stays reproducible even if the original file or YouTube grab disappears. Bundles can therefore get large — dedupe by hash (already done) and offer FLAC for the `samples/` payload.
- **Version drift.** `meta.json` pins the protocol/engine version. `bounce` refuses (or warns + best-effort) if the current engine's protocol version differs from the recording's, per [event-protocol.md versioning](../../event-protocol.md#versioning).
- **Instrument/sample ordering.** The recorder must snapshot instruments/samples registered *before* `start()` (not just ones defined during the take), or an early `NoteOn` references an unknown id on replay. `start()` takes that snapshot.
- **Journal growth.** Events only — a dense techno set is on the order of KB/minute, trivial next to the audio. Flushed in blocks so a crash loses at most one block.

## Acceptance criteria

- **A live-coded session round-trips.** Record a ≥2-minute performance with live edits (mute/unmute tracks, hand-sweep a filter, swap a pattern). `strobe bounce set.strobesession out/` reproduces audio matching what was heard (sample-for-sample for synth/sample-only material; verified by null-test against a simultaneous real-time capture from [06](06-audio-capture-stems.md), within capture jitter).
- **Stems come out of a session.** `strobe bounce set.strobesession out/ --stems` writes `out/master.wav` plus `out/stems/p1.wav…` — *the channel loops kept in place* — all sample-aligned (see [06](06-audio-capture-stems.md)).
- **Self-contained & portable.** The `.strobesession` bundle re-renders correctly on a different machine with the original source files absent (samples travel inside the bundle).
- **Deterministic.** Bouncing the same session twice yields byte-identical master and stems.
- **Faster than real time, dropout-free.** A 2-minute session bounces in well under 2 minutes with `stats()` reporting zero overruns; tested on macOS, then Linux/Windows in CI.
- **Tee correctness (unit).** `SessionRecorder` forwards every call to the underlying engine unchanged while appending — asserted by a mock engine that records the calls it receives and comparing against the journal.
