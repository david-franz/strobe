// The hot-reload runtime: the long-lived process that wires the engine, clock, and
// scheduler together, loads a song file, and re-evaluates it on save — applying the
// swap at the next cycle boundary so the music never jumps mid-bar
// (docs/plan/phase-0/07-hot-reload-demo.md).

import { watch } from "node:fs";
import { pathToFileURL } from "node:url";
import { createNativeEngine } from "./engine";
import { Clock } from "./clock";
import { Scheduler } from "./scheduler";
import { setActiveClock } from "./clock-singleton";

export interface PlayOpts {
  sampleRate?: number;
  bufferFrames?: number;
}

export interface RunningSong {
  stop(): Promise<void>;
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** Milliseconds until the next integer cycle boundary from `nowCycle` at tempo `cps`.
 *  At an exact boundary this returns a full cycle (worst case is a one-cycle delay,
 *  never a glitch). */
export function nextBoundaryDelayMs(nowCycle: number, cps: number): number {
  const nextBoundary = Math.floor(nowCycle) + 1;
  const secsUntil = (nextBoundary - nowCycle) / cps;
  return Math.max(0, secsUntil * 1000);
}

/** Re-import a song with a cache-busting URL so its top-level code (setcps/p1) runs
 *  again. Without the `?v=` query the cached ESM module would not re-execute. */
export async function evalSong(songPath: string): Promise<void> {
  const url = pathToFileURL(songPath).href + `?v=${Date.now()}`;
  await import(url);
}

/** Import the song, keeping the last good pattern if it throws (a truncated save or a
 *  syntax/type error never stops the engine — you hear no dropout while you fix it). */
export async function loadSongSafely(songPath: string): Promise<void> {
  try {
    await evalSong(songPath);
  } catch (err) {
    process.stderr.write(`\n[strobe] song error (keeping last good pattern): ${String(err)}\n`);
  }
}

/** Defer `apply` (which reassigns p1) until the next cycle boundary so the swap lands
 *  exactly on a downbeat — click-free. The coarse wait need not be precise: it only
 *  governs when a variable is reassigned; the scheduler still stamps the first new
 *  event with its exact frame. */
export async function swapAtCycleBoundary(clock: Clock, apply: () => Promise<void>): Promise<void> {
  await sleep(nextBoundaryDelayMs(clock.nowCycle(), clock.cps));
  await apply();
}

/** Boot the engine, start the scheduler, load `songPath`, and hot-reload it on save. */
export async function play(songPath: string, opts: PlayOpts = {}): Promise<RunningSong> {
  const sampleRate = opts.sampleRate ?? 48000;
  const engine = createNativeEngine();
  await engine.start({ sampleRate, bufferFrames: opts.bufferFrames ?? 256 });

  const clock = new Clock(engine);
  clock.anchor(sampleRate);
  setActiveClock(clock); // the song's setcps()/p1() mutate the library singletons

  const scheduler = new Scheduler(engine, clock);
  scheduler.start();

  await loadSongSafely(songPath); // first load → populates p1 immediately

  // Watch + hot-reload, swapping at the next cycle boundary. The `reloading` guard
  // coalesces the multiple watch events some editors fire per save.
  let reloading = false;
  const watcher = watch(songPath, () => {
    if (reloading) return;
    reloading = true;
    void swapAtCycleBoundary(clock, () => loadSongSafely(songPath)).finally(() => {
      reloading = false;
    });
  });

  // Telemetry for the verification recipe.
  const statsTimer = setInterval(() => {
    const s = engine.stats();
    process.stdout.write(
      `frame=${s.currentFrame} underruns=${s.underruns} late=${s.lateEvents} ring=${s.ringFullness.toFixed(3)}\r`,
    );
  }, 500);

  return {
    stop: async () => {
      clearInterval(statsTimer);
      watcher.close();
      scheduler.stop();
      await engine.stop();
    },
  };
}
