# 01 — Hot-reload runtime (REPL + file-watcher)

## Purpose

This is the heart of Phase 5: the long-lived process that boots the engine once and then lets you *change the music while it plays*. The [Phase 0 hot-reload demo](../phase-0/07-hot-reload-demo.md) proved a single track (`p1`) can swap seamlessly at a cycle boundary on save. Phase 5 generalizes that to a real song: many tracks (`p1..pN`), arbitrary patterns, and the property that **only the tracks you actually changed are re-evaluated and swapped** — an untouched track keeps its identical pattern reference, so the scheduler never even notices it changed, and there is never a global "restart."

The runtime exposes two entry points that share one core: `play(songPath)` (the file-watcher loop the CLI's `tekno play` drives) and `repl()` (an interactive prompt where each line you submit is evaluated against the same live engine/clock/scheduler). Both obey the cardinal rule from [architecture.md](../../architecture.md#the-core-principle-two-planes): **re-evaluation is a control-plane event; the audio plane is never touched.** A broken edit must print an error and keep the last-good music playing — the engine cannot stop, glitch, or drop a sample because of a typo.

## Location

```
packages/tekno/
  src/
    runtime.ts          # GENERALIZED from Phase 0 — play() + repl() + watch/eval loop
    runtime/
      evalSong.ts       # NEW — isolated evaluation of a song module, capturing track assignments
      trackDiff.ts      # NEW — diff captured assignments vs. previous; decide what to swap
      boundary.ts       # NEW — swapAtCycleBoundary() (lifted from Phase 0, multi-track)
      repl.ts           # NEW — the interactive REPL
    tracks.ts           # (Phase 0/2) the p1..pN slot registry — extended with capture hooks
    clock.ts            # (Phase 0) the cps Clock
    scheduler.ts        # (Phase 0/2) the look-ahead scheduler
    engine.ts           # (Phase 0) createNativeEngine()
```

This document owns `runtime.ts` and everything under `runtime/`. It builds directly on [Phase 0 / 07](../phase-0/07-hot-reload-demo.md) and consumes the same `Engine`, `Clock`, and `Scheduler`.

## Design

### The track registry, with capture (in `tracks.ts`)

`p1..pN` are module-level singletons (per [architecture.md](../../architecture.md#tracks-and-hot-swapping)). Each is a function that, when called, assigns a pattern into a slot. To diff "what did this evaluation set?" the registry can run in **capture mode**: while a song module is being evaluated, every `pN(pattern)` call is recorded into a fresh map instead of (or in addition to) being applied immediately.

```ts
// tracks.ts (extended)
export interface TrackAssignment {
  id: number;                 // 1 for p1, 2 for p2, …
  pattern: Pattern<ControlMap>;
  source?: string;            // optional: the source text of the call site, for diffing
}

let capture: Map<number, TrackAssignment> | null = null;

/** Start capturing pN() calls into a fresh map instead of applying them live. */
export function beginCapture() { capture = new Map(); }
export function endCapture(): Map<number, TrackAssignment> {
  const c = capture ?? new Map();
  capture = null;
  return c;
}

/** The slot function the song calls: p1(pattern), p2(pattern), … */
export function track(id: number) {
  return (pattern: Pattern<ControlMap>) => {
    const a: TrackAssignment = { id, pattern };
    if (capture) capture.set(id, a);   // capture mode: record, don't apply yet
    else applyTrack(a);                // direct mode (REPL one-liners): apply now
  };
}

export const p1 = track(1), p2 = track(2), p3 = track(3) /* … pN */;
```

`applyTrack` is the only thing that mutates the live slot the scheduler reads. In capture mode nothing is applied until the diff + boundary swap below decides to.

### Isolated evaluation of a song (in `runtime/evalSong.ts`)

A song file is plain TypeScript whose top-level code calls `setcps(...)`, `p1(...)`, `p2(...)`, etc. (exactly the README quickstart shape). To re-evaluate it we re-import the **song module** with a cache-busting URL — the `tekno` library itself stays loaded once, so the engine/clock/scheduler/track-registry singletons persist across reloads (the Phase 0 module-singleton strategy, generalized).

```ts
// runtime/evalSong.ts
import { pathToFileURL } from "node:url";
import { beginCapture, endCapture, type TrackAssignment } from "../tracks";

export interface EvalResult {
  assignments: Map<number, TrackAssignment>;
  cps?: number;                  // if the song called setcps()
}

/**
 * Re-import the song with a cache-busting query so its top-level code runs again,
 * capturing every pN() assignment instead of applying it. Throwing here is expected
 * for broken edits — the caller keeps the last-good state.
 */
export async function evalSong(songPath: string): Promise<EvalResult> {
  beginCapture();
  let cps: number | undefined;
  const capturedCps = (x: number) => { cps = x; };   // setcps() routes here during eval
  try {
    setcpsSink(capturedCps);
    const url = pathToFileURL(songPath).href + `?v=${Date.now()}`;
    await import(url);                                 // runs setcps()/p1()/p2()/…
    return { assignments: endCapture(), cps };
  } finally {
    endCapture();                                      // ensure capture is cleared even on throw
    setcpsSink(null);
  }
}
```

> **Why re-import and not `eval`.** Re-importing the file with a `?v=timestamp` URL is how Node actually re-runs ESM top-level code (the [Phase 0 cache-busting note](../phase-0/07-hot-reload-demo.md#module-singleton-strategy-so-reload-preserves-state)). The downside — leaked module instances accumulating in the loader cache — is bounded: each song reload adds one small module object; for a multi-hour session this is megabytes, acceptable for a creative tool, and can be mitigated with a worker (see Risks). The library modules are imported once with a stable URL and are *not* busted, so all singletons survive.

### Diffing captured assignments vs. the previous evaluation (in `runtime/trackDiff.ts`)

After a successful `evalSong`, we compare the new assignment map to the one from the last good evaluation and produce a swap plan: which tracks are **added**, **changed**, **unchanged**, or **removed**.

```ts
// runtime/trackDiff.ts
export interface SwapPlan {
  changed: TrackAssignment[];    // added or changed → swap at next boundary
  removed: number[];             // present before, absent now → silence at next boundary
  unchanged: number[];           // identical → do nothing (no re-query, no swap)
}

export function diffTracks(
  prev: Map<number, TrackAssignment>,
  next: Map<number, TrackAssignment>,
): SwapPlan {
  const changed: TrackAssignment[] = [];
  const removed: number[] = [];
  for (const [id, a] of next) {
    const old = prev.get(id);
    if (!old || !samePattern(old, a)) changed.push(a);
  }
  for (const id of prev.keys()) if (!next.has(id)) removed.push(id);
  const unchanged = [...next.keys()].filter(
    (id) => prev.has(id) && samePattern(prev.get(id)!, next.get(id)!),
  );
  return { changed, removed, unchanged };
}
```

**How `samePattern` decides "changed".** Patterns are opaque functions, so we cannot deep-compare them by value. Two cheap, reliable signals (use whichever the build wires up; default to source text):

1. **Source text of the assignment call (default).** A lightweight transform/wrapper records the source range of each `pN(...)` call as `assignment.source`. If a track's source text is byte-identical to the previous evaluation, it is unchanged — the most precise and intuitive rule ("you didn't touch that line ⇒ that track doesn't swap"). This is what gives the "re-evaluate ONLY changed tracks" guarantee.
2. **Reference identity fallback.** If source capture is unavailable, treat every track present in the new evaluation as potentially changed *but* still defer to the boundary swap, which is idempotent and click-free regardless. Correctness is preserved; the only cost is that an untouched track is needlessly re-queried from the next boundary (still seamless, since old and new patterns are identical over whole cycles).

### Boundary swap, applied per track (in `runtime/boundary.ts`)

We lift `swapAtCycleBoundary` from [Phase 0 / 07](../phase-0/07-hot-reload-demo.md#the-runtime-boot-watch-swap) and apply the whole `SwapPlan` atomically at the boundary.

```ts
// runtime/boundary.ts
export async function applyAtCycleBoundary(clock: Clock, plan: SwapPlan) {
  const now = clock.nowCycle();
  const nextBoundary = Math.floor(now) + 1;
  const secsUntil = (nextBoundary - now) / clock.cps;
  await sleep(Math.max(0, secsUntil * 1000));   // coarse wait; jitter is harmless (Phase 0)
  // Apply the entire plan at the boundary so all changed tracks switch on the same downbeat.
  for (const a of plan.changed) applyTrack(a);
  for (const id of plan.removed) clearTrack(id);
  // unchanged tracks: untouched — their slot reference never changes.
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
```

The scheduler's already-committed lookahead window is never rewritten ([architecture.md](../../architecture.md#tracks-and-hot-swapping)); the plan only affects queries from the next boundary on. Applying all changed tracks together means a multi-track edit lands on a single downbeat — the groove stays locked.

### The watch + eval loop (in `runtime.ts`)

```ts
// runtime.ts
import { watch } from "node:fs";
import { createNativeEngine } from "./engine";
import { Clock } from "./clock";
import { Scheduler } from "./scheduler";
import { setActiveClock } from "./clock-singleton";
import { evalSong } from "./runtime/evalSong";
import { diffTracks, type SwapPlan } from "./runtime/trackDiff";
import { applyAtCycleBoundary } from "./runtime/boundary";
import { snapshotTracks } from "./tracks";

export interface PlayOpts {
  sampleRate?: number;     // default 48000
  bufferFrames?: number;   // default 256
  lookaheadMs?: number;    // default 100 (architecture.md default)
  onStats?: (s: EngineStats) => void;
}

export async function play(songPath: string, opts: PlayOpts = {}) {
  const sampleRate = opts.sampleRate ?? 48000;
  const engine = createNativeEngine();
  await engine.start({ sampleRate, bufferFrames: opts.bufferFrames ?? 256 });

  const clock = new Clock(engine);
  clock.anchor(sampleRate);
  setActiveClock(clock);

  const scheduler = new Scheduler(engine, clock, { lookaheadMs: opts.lookaheadMs ?? 100 });
  scheduler.start();

  // First load: evaluate, then apply immediately (no need to wait a boundary on boot).
  let lastGood = (await evalSong(songPath)).assignments;
  applyInitial(clock, lastGood);

  let reloading = false;
  const onChange = async () => {
    if (reloading) return;            // coalesce duplicate watch events
    reloading = true;
    try {
      const next = await evalSong(songPath);          // throws on a broken edit
      if (next.cps != null) clock.setcps(next.cps);   // tempo change (rare; Tempo event)
      const plan = diffTracks(lastGood, next.assignments);
      if (plan.changed.length || plan.removed.length)
        await applyAtCycleBoundary(clock, plan);
      lastGood = next.assignments;                    // commit only after success
      logSwap(plan);
    } catch (err) {
      // Broken edit: keep last-good music playing, surface the error, do NOT touch audio.
      logError(songPath, err);
    } finally {
      reloading = false;
    }
  };

  watch(songPath, () => { void onChange(); });

  if (opts.onStats) setInterval(() => opts.onStats!(engine.stats()), 500);
  return { engine, clock, scheduler, stop: () => engine.stop() };
}
```

### The REPL (in `runtime/repl.ts`)

The REPL shares the same engine/clock/scheduler. Each submitted line is evaluated in **direct mode** (no capture): a `p2(...)` typed at the prompt assigns immediately and swaps at the next boundary via the same `applyAtCycleBoundary` path (called with a single-track plan). The REPL also accepts plain expressions (e.g. `queryArc(sound("bd ~ sn ~"), 0, 1)`) and prints their result, so it doubles as a pattern-inspection tool ([pattern-model.md](../../pattern-model.md#testing-patterns)).

```ts
// runtime/repl.ts (sketch)
import repl from "node:repl";
export function startRepl(ctx: { engine; clock; scheduler }) {
  const r = repl.start({ prompt: "tekno> ", useGlobal: false });
  // Inject the live API into the REPL context so p1/p2/sound/note/hush resolve.
  Object.assign(r.context, makeLiveApi(ctx));   // p1..pN, sound, note, setcps, hush, …
  // p-calls go through the registry in DIRECT mode → boundary swap, never restart.
  return r;
}
```

`hush()` (silence all tracks) and `p1.solo()` / `p1.mute()` are convenience controls exposed in the REPL context and usable from songs too; they are sugar over `clearTrack`/`applyTrack` and obey the same boundary-swap discipline.

## Interfaces

- **To the [CLI](02-cli.md):** `play(songPath, opts)` backs `tekno play`; `startRepl(ctx)` backs `tekno repl`. The CLI passes config (sample rate, buffer, lookahead) through `PlayOpts`.
- **To [Phase 0 / 04 napi-bridge](../phase-0/04-napi-bridge.md):** owns one `Engine` (`start`/`stop`/`stats`); never calls `pushEvents` itself — the scheduler does.
- **To [Phase 0 / 05 scheduler](../phase-0/05-lookahead-scheduler.md):** owns one `Scheduler`; relies on its committed-window frontier so reassignment cannot rewrite enqueued events.
- **To [Phase 0 / 06 clock + tracks](../phase-0/06-clock-and-tracks.md):** owns one `Clock`; the song's `setcps`/`p1..pN` mutate the library singletons through the capture/apply hooks above.
- **To the user's workflow:** the user edits a `.ts` song in any editor and saves, or types at the `tekno repl` prompt. Both routes funnel through the same diff → boundary-swap path. Nothing the user does can stop the audio thread.

## Edge cases & risks

- **A broken edit must not kill audio.** `evalSong` throws on a syntax/type/runtime error; the loop catches it, prints the error, and **does not update `lastGood`**, so the running tracks keep playing untouched. This is the single most important property and is tested explicitly (see Acceptance).
- **Partial / truncated writes & "save twice."** Editors often fire multiple `watch` events per save or briefly expose a truncated file. The `reloading` guard coalesces overlapping reloads; a transient parse error from a half-written file is caught and ignored, keeping the current patterns. A short (≈30 ms) debounce on the watch callback further absorbs duplicate fires.
- **Only-changed re-eval safety.** If `samePattern` ever *wrongly* reports a track as changed, the worst case is a redundant boundary swap to an identical pattern — still click-free, because old and new are equal over whole cycles. If it wrongly reports unchanged, the user's edit would not take — so the default is the precise **source-text** rule, and the conservative fallback errs toward swapping (never toward missing a change).
- **ESM module-cache growth.** Each reload leaks one song-module instance into Node's loader cache. Bounded and small; for very long sessions the runtime can optionally evaluate the song in a short-lived worker thread that returns only the captured `SwapPlan` (the patterns themselves are serialized as their source and re-built in the main context), discarding the worker per reload. This is an optimization, not required for "done."
- **Tempo changes mid-session.** A `setcps()` in a reloaded song emits a `Tempo` event ([event-protocol.md](../../event-protocol.md#opcodes)); we apply it at the boundary alongside the swap so timing changes land on a downbeat rather than mid-bar.
- **REPL vs. file-watch on the same track.** If a user has both a watched file and the REPL open and both assign `p1`, last-writer-wins at the next boundary — predictable and harmless, but documented so it is not surprising.
- **Cross-platform `fs.watch`.** `fs.watch` semantics differ (macOS coalesces; Linux inotify; Windows ReadDirectoryChangesW). The debounce + `reloading` guard normalize this; on platforms where `fs.watch` is unreliable for atomic-rename saves (some editors rename a temp file over the target), the runtime falls back to `fs.watchFile` polling (≈100 ms) for the song path. Validated on all three OSes per the [cross-cutting DoD](../../roadmap.md#cross-cutting-definition-of-done-every-phase).

## Acceptance criteria

- [ ] **Multi-track seamless swap.** Playing a 3-track song, editing `p2`'s line only and saving swaps **only** `p2` at the next cycle boundary with no clicks/dropouts; `p1` and `p3` are provably not re-applied (assert via the `SwapPlan.changed` containing exactly `{id:2}`). *How to test:* headless harness drives `play()`, mutates the file via `fs.writeFile`, asserts the logged plan and `stats()` clean across the edit.
- [ ] **Untouched tracks are not re-evaluated/swapped.** A no-op save (rewriting the file byte-identically) produces an empty `SwapPlan` and triggers no boundary swap. *How to test:* unit test `diffTracks` + an integration test asserting zero swaps on identical content.
- [ ] **Broken edit keeps last-good playing.** Introduce a syntax error and save: the error is printed, `lastGood` is unchanged, and audio continues with **zero** underruns/late events across the edit. Fix it: the corrected tracks swap in at the next boundary. *How to test:* the [Phase 0 "survives a bad edit"](../phase-0/07-hot-reload-demo.md#acceptance-criteria-the-phase-0-verification-recipe) recipe, extended to multiple tracks, run headlessly.
- [ ] **Audio never restarts.** Across any number of edits the engine's `currentFrame` increases monotonically with no `start()`/`stop()` calls after boot. *How to test:* spy on the `Engine` mock/native handle; assert `start` called exactly once and `stop` zero times during a 60 s edit session.
- [ ] **REPL parity.** Typing `p1( sound("bd*8") )` at `tekno repl` swaps `p1` at the next boundary identically to a file edit; typing `queryArc(sound("bd ~ sn ~"), 0, 1)` prints the expected `Hap` list ([pattern-model.md](../../pattern-model.md#testing-patterns)). *How to test:* feed lines to the REPL programmatically and assert the swap plan / printed output.
- [ ] **Clean telemetry throughout.** `stats()` reports `underruns === 0 && lateEvents === 0` for the whole session including across every edit and every REPL command. *How to test:* assert after each mutation in the headless harness.
- [ ] **Cross-platform.** All of the above pass on macOS, Linux, and Windows in CI, including the `fs.watchFile` polling fallback path.
