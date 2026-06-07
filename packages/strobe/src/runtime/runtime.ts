// The hot-reload runtime: the long-lived process that wires the engine, clock, and
// scheduler together, loads a song file, and re-evaluates it on save. The cycle-boundary
// swap is owned by the track registry (assign stages → the scheduler promotes at the next
// integer cycle), so a save re-evaluates *immediately* and the change lands on the very
// next boundary — no extra deferral, and no dropped saves.

import { watch } from "node:fs";
import { pathToFileURL } from "node:url";
import { createNativeEngine } from "../backend/engine";
import { Clock } from "../clock/clock";
import { Scheduler } from "./scheduler";
import { setActiveClock } from "../clock/singleton";
import { setActiveSampleEngine } from "../sample/load";
import { beginReload, endReload, abortReload } from "./tracks";

export interface PlayOpts {
  sampleRate?: number;
  bufferFrames?: number;
}

export interface RunningSong {
  stop(): Promise<void>;
}

/** Re-import a song with a cache-busting URL so its top-level code (setcps/p1) runs
 *  again. Without the `?v=` query the cached ESM module would not re-execute. */
export async function evalSong(songPath: string): Promise<void> {
  const url = pathToFileURL(songPath).href + `?v=${Date.now()}`;
  await import(url);
}

/** Re-evaluate the song, tracking which tracks it (re)assigns so dropped tracks are
 *  silenced. A truncated save or a syntax/type error keeps the last good patterns (the
 *  engine never stops, so you hear no dropout while you fix it). */
export async function loadSongSafely(songPath: string): Promise<void> {
  beginReload();
  try {
    await evalSong(songPath);
    endReload(); // success → reap tracks this revision didn't reassign
  } catch (err) {
    abortReload(); // failure → keep every track as-is
    process.stderr.write(`\n[strobe] song error (keeping last good pattern): ${String(err)}\n`);
  }
}

/** Boot the engine, start the scheduler, load `songPath`, and hot-reload it on save. */
export async function play(songPath: string, opts: PlayOpts = {}): Promise<RunningSong> {
  const sampleRate = opts.sampleRate ?? 48000;
  const engine = createNativeEngine();
  await engine.start({ sampleRate, bufferFrames: opts.bufferFrames ?? 256 });

  const clock = new Clock(engine);
  clock.anchor(sampleRate);
  setActiveClock(clock); // the song's setcps()/p1() mutate the library singletons
  setActiveSampleEngine(engine); // the song's sample(...) loads into this engine

  const scheduler = new Scheduler(engine, clock);
  scheduler.start();

  await loadSongSafely(songPath); // first load → populates the tracks immediately

  // Watch + hot-reload. Debounce coalesces the multiple events editors fire per save (and
  // skips mid-write truncated reads); the latest save always wins and none are dropped.
  // Re-eval is immediate — the registry stages the swap for the next cycle boundary.
  let debounce: ReturnType<typeof setTimeout> | null = null;
  const watcher = watch(songPath, () => {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(() => {
      debounce = null;
      void loadSongSafely(songPath);
    }, 50);
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
      if (debounce) clearTimeout(debounce);
      clearInterval(statsTimer);
      watcher.close();
      scheduler.stop();
      setActiveSampleEngine(null);
      await engine.stop();
    },
  };
}
