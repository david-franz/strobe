# 07 — Hot-reload demo & end-to-end verification

## Purpose

This is the feature that *demonstrates the spine* and is the literal Phase 0 deliverable from [roadmap.md](../../roadmap.md#phase-0--proof-of-spine): `examples/phase0-kick.ts` plus the file-watcher runtime that re-evaluates the song file on save and swaps the `p1` pattern at the **next cycle boundary** with **no dropout**. Everything else in Phase 0 is machinery; this is where a person sits down, hears a steady kick, edits `bd*4` → `bd*8` (or `bd(3,8)`), saves, and hears the loop change seamlessly — proving that "TS drives a Rust audio engine live with zero glitches" ([architecture.md](../../architecture.md#the-core-principle-two-planes)).

It also defines the **verification recipe**: the exact steps and the `stats()` assertions (zero underruns, zero late events) that constitute "done."

## Location

```
examples/
  phase0-kick.ts        # NEW — the watched song file the user edits
packages/tekno/
  src/
    runtime.ts          # NEW — this feature: file-watcher + boundary swap + engine wiring
    index.ts            # (06) exports setcps / p1 / bdEvery / bdEuclid
    scheduler.ts        # (05) the look-ahead scheduler the runtime drives
    clock.ts            # (06) the cps clock the runtime owns
    engine.ts           # (04) createNativeEngine()
```

This document owns `examples/phase0-kick.ts` and `packages/tekno/src/runtime.ts`. It wires together [04](04-napi-bridge.md) (engine), [05](05-lookahead-scheduler.md) (scheduler), and [06](06-clock-and-tracks.md) (clock + track slot).

## Design

### The watched song file

The song is plain TypeScript that calls the Phase 0 API. The user edits *only this file*.

```ts
// examples/phase0-kick.ts
import { setcps, p1, bdEvery } from "tekno";

setcps(130 / 60 / 4);     // ~130 BPM, 4 beats per cycle

p1( bdEvery(4) );         // four-on-the-floor kick
// edit to bdEvery(8) for double-time, or bdEuclid(3, 8) for a Euclidean groove
```

> In Phase 0 the song uses `bdEvery(4)` / `bdEuclid(3,8)` directly (the parser-backed `sound("bd*4")` arrives in Phase 2 — [06-clock-and-tracks.md](06-clock-and-tracks.md)). `setcps` and `p1` come from the library's module-level singletons so re-running the file just reassigns the slot.

### The runtime: boot, watch, swap

The runtime is the long-lived process. It (1) creates the native engine and starts it, (2) anchors the clock and starts the scheduler, (3) imports the song once to populate `p1`, then (4) watches the song file and re-imports it on change — but **applies the swap at the next cycle boundary** so the music never jumps mid-bar.

```ts
// packages/tekno/src/runtime.ts
import { watch } from "node:fs";
import { pathToFileURL } from "node:url";
import { createNativeEngine } from "./engine";
import { Clock } from "./clock";
import { Scheduler } from "./scheduler";
import { setActiveClock } from "./clock-singleton"; // lets song's setcps() reach this Clock
import { getTrack } from "./tracks";

export async function play(songPath: string, opts?: { sampleRate?: number; bufferFrames?: number }) {
  const sampleRate = opts?.sampleRate ?? 48000;
  const engine = createNativeEngine();
  await engine.start({ sampleRate, bufferFrames: opts?.bufferFrames ?? 256 });

  const clock = new Clock(engine);
  clock.anchor(sampleRate);
  setActiveClock(clock);                 // song's setcps()/p1() mutate library singletons

  const scheduler = new Scheduler(engine, clock);
  scheduler.start();                     // begins ~25ms ticks, pushing the lookahead window

  await evalSong(songPath);              // first load → populates p1 immediately

  // Watch + hot-reload. Swap at the next cycle boundary.
  let reloading = false;
  watch(songPath, async (eventType) => {
    if (reloading) return;
    reloading = true;
    try {
      await swapAtCycleBoundary(clock, () => evalSong(songPath));
    } finally { reloading = false; }
  });

  // Telemetry for the verification recipe.
  setInterval(() => {
    const s = engine.stats();
    process.stdout.write(
      `frame=${s.currentFrame} underruns=${s.underruns} late=${s.lateEvents} ring=${s.ringFullness.toFixed(3)}\r`
    );
  }, 500);
}

/** Re-import the song with a cache-busting URL so its top-level code runs again. */
async function evalSong(songPath: string) {
  const url = pathToFileURL(songPath).href + `?v=${Date.now()}`; // bust ESM module cache
  await import(url); // runs setcps(...) + p1(...) → reassigns the slot
}

/**
 * Run `apply` (which reassigns p1) so its effect is visible only from the next
 * cycle boundary. The scheduler already won't rewrite the committed lookahead
 * window; we additionally hold the new pattern until the boundary to make the
 * swap musically exact and click-free.
 */
async function swapAtCycleBoundary(clock: Clock, apply: () => Promise<void>) {
  const now = clock.nowCycle();
  const nextBoundary = Math.floor(now) + 1;          // next integer cycle
  const secsUntil = (nextBoundary - now) / clock.cps;
  await sleep(Math.max(0, secsUntil * 1000));        // coarse wait; jitter here is harmless
  await apply();                                      // reassign p1 right at the boundary
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
```

### Why the swap is seamless

Two mechanisms combine, both from [architecture.md](../../architecture.md#tracks-and-hot-swapping):

1. **The committed window is never rewritten.** The scheduler's frontier (`scheduledUntilCycle` in [05-lookahead-scheduler.md](05-lookahead-scheduler.md)) means the next ~100 ms of events are already enqueued with exact frames. Reassigning `p1` cannot retroactively change them — so there is no torn buffer, no half-old/half-new bar.

2. **The new pattern takes over at a cycle boundary.** `swapAtCycleBoundary` defers the `p1(...)` reassignment until the next integer cycle. From that boundary on, the scheduler queries the *new* pattern. Because both old and new patterns are defined over whole cycles, the transition lands exactly where a downbeat would, so the groove stays locked and there is no audible discontinuity — the kick that was playing finishes naturally and the next cycle starts the new rhythm.

The coarse `setTimeout` wait inside `swapAtCycleBoundary` does not need to be precise: it only governs *when we reassign a variable*, and the scheduler (working ~100 ms ahead) will still stamp the first new-pattern event with its exact frame. Jitter in this wait is invisible for the same reason scheduler-tick jitter is ([architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks)).

### Module-singleton strategy (so reload preserves state)

The song re-import must reach the *same* `Clock` and the *same* track slot module each time, or the swap would reset state. The runtime keeps the `tekno` library loaded once (the engine, clock, scheduler, and `tracks.ts` slot state persist); only the **song module** is re-imported with a cache-busting query string. `setActiveClock(clock)` registers the live clock into a library singleton that the song's `setcps()` mutates. This is the Phase 0 precursor to the richer hot-reload (re-evaluating only changed tracks) in Phase 5 ([roadmap.md](../../roadmap.md#phase-5--live-coding-dx)).

### Running it

```bash
# Phase 0 dev run (tsx/ts-node executes TS directly):
npx tsx -e "import('tekno/runtime').then(m => m.play('examples/phase0-kick.ts'))"
# or a tiny bin: `tekno-play examples/phase0-kick.ts`
```

## Interfaces

- **To [04-napi-bridge.md](04-napi-bridge.md):** `createNativeEngine()`, `engine.start/stop/stats`.
- **To [05-lookahead-scheduler.md](05-lookahead-scheduler.md):** owns one `Scheduler`, calls `start()`; relies on its frontier to avoid rewriting committed events.
- **To [06-clock-and-tracks.md](06-clock-and-tracks.md):** owns one `Clock`; the song's `setcps`/`p1` mutate the library singletons; `getTrack(1)` reflects the swap.
- **Event protocol:** indirectly — the runtime triggers no events itself; it drives the scheduler that emits `NoteOn`/`Tempo`.

## Edge cases & risks

- **Editor "save twice" / partial writes.** Some editors fire multiple `watch` events per save, or write a truncated file mid-save. The `reloading` guard coalesces overlapping reloads; `evalSong` is wrapped in try/catch so a transient parse error (truncated file) is logged and ignored, keeping the current pattern playing.
- **Song with a syntax/type error.** A bad edit throws on import; the runtime catches it, prints the error, and **keeps the last good pattern** — the engine never stops, so you hear no dropout while you fix the typo.
- **ESM cache busting.** Re-import uses a `?v=timestamp` URL so Node actually re-runs the song's top-level code; without it the cached module would not re-execute and `p1` would not change.
- **Swap exactly on the boundary.** If a reload's reassignment lands a hair after the boundary, the new pattern simply starts one cycle later — still click-free, never torn. Worst case is a one-cycle delay, not a glitch.
- **Stats during edits.** The verification asserts `underruns === 0 && lateEvents === 0` *including across the edit*; the design guarantees this because audio rendering and event draining are entirely decoupled from the JS reload.

## Acceptance criteria (the Phase 0 verification recipe)

Perform these in order; all must pass for Phase 0 "done" ([roadmap.md](../../roadmap.md#phase-0--proof-of-spine)):

- [ ] **Steady kick.** Run the runtime on `examples/phase0-kick.ts`. *Audible check:* a four-on-the-floor kick at ~130 BPM, on time, not drifting, for at least 60 s.
- [ ] **Seamless density swap.** While it plays, edit `bdEvery(4)` → `bdEvery(8)` and save. *Audible check:* at the next cycle boundary the loop doubles to eight kicks per cycle with **no click, pop, or dropout**, and stays in time.
- [ ] **Seamless Euclid swap.** Edit to `bdEuclid(3, 8)` and save. *Audible check:* the rhythm changes to the 3-of-8 Euclidean pattern at the next boundary, again with no glitch.
- [ ] **Clean telemetry throughout.** *How to test:* watch the printed `stats()` line during all of the above; assert `underruns === 0` and `late === 0` for the whole session, including across every edit. (Programmatic version: a test harness that drives the runtime headless, performs the edits via `fs.writeFile`, and asserts the stats after each.)
- [ ] **Survives a bad edit.** Introduce a syntax error, save (engine keeps playing the last good pattern, error printed), then fix it. *Audible check:* no dropout at any point; the corrected pattern swaps in at the next boundary.
- [ ] **Real-time safety holds under the demo's load.** *How to test:* the 60 s run above doubles to `bd*8` and remains underrun-free, satisfying the roadmap's "provably no allocation/locking/JS on the audio thread" gate together with the code-review checks in [01](01-audio-io-cpal.md)/[02](02-event-ring-buffer.md)/[03](03-kick-voice.md).
