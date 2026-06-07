// Sample loading (Phase 4, feature 01): the TS side of the decode/load pipeline. A song calls
// `loadFile`/`loadPcm` (or `sample(...)`, built on these); the heavy decode + resample happen in
// Rust off the audio thread, and we get back a sample id + metadata. Loads are idempotent per
// source so re-running a song line during live coding never re-decodes the same file.
//
// v1 note: loading is synchronous (the napi call decodes on the Node thread, which blocks for a
// few ms on a short grab — fine at song-setup time). The async/worker-thread path the plan
// sketches is a later refinement.

import type { Engine, LoadedSample } from "../backend/engine";

export interface SampleHandle {
  /** Engine sample id — what `TrigSample` carries. */
  id: number;
  channels: number;
  /** Per-channel length, at the engine rate. */
  frames: number;
  sampleRate: number;
  name?: string;
}

let activeEngine: Engine | null = null;
const cache = new Map<string, SampleHandle>();

/** The runtime registers its live engine here so a song's `sample(...)` can load into it.
 *  Passing `null` (on stop) clears the idempotency cache — a fresh engine has a fresh bank. */
export function setActiveSampleEngine(engine: Engine | null): void {
  activeEngine = engine;
  if (engine === null) cache.clear();
}

function requireEngine(): Engine {
  if (!activeEngine) {
    throw new Error("strobe: no active engine — load samples after the runtime has started");
  }
  return activeEngine;
}

function handle(r: LoadedSample, name?: string): SampleHandle {
  return { id: r.id, channels: r.channels, frames: r.frames, sampleRate: r.sampleRate, name };
}

/** Load a local audio file (WAV/MP3/FLAC/OGG/…), decoded + resampled in Rust. Idempotent per
 *  absolute path. Throws a clear error if the file can't be read/decoded. */
export function loadFile(path: string, name?: string): SampleHandle {
  const key = `file:${path}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const h = handle(requireEngine().loadSampleFile(path), name ?? path);
  cache.set(key, h);
  return h;
}

/** Load already-decoded interleaved PCM (used by YouTube ingest after ffmpeg, and tests).
 *  Idempotent per `key` when one is given (e.g. a content hash). */
export function loadPcm(
  pcm: Float32Array,
  channels: number,
  sampleRate: number,
  opts: { name?: string; key?: string } = {},
): SampleHandle {
  if (opts.key) {
    const hit = cache.get(`pcm:${opts.key}`);
    if (hit) return hit;
  }
  const h = handle(requireEngine().loadSamplePcm(pcm, channels, sampleRate), opts.name);
  if (opts.key) cache.set(`pcm:${opts.key}`, h);
  return h;
}

/** Test/introspection: how many distinct sources are cached this session. */
export function loadedCount(): number {
  return cache.size;
}
