// Disk cache for grabbed clips (Phase 4, feature 05). A YouTube grab is deterministic in
// (url, start, dur, rate) and expensive (network fetch + transcode), so the trimmed WAV is cached
// under an OS cache dir. Re-running a song line during live coding hits the cache, not the network.
//
// Pure Node/control-plane: nothing here touches the engine or the audio thread.

import { createHash } from "node:crypto";
import { homedir } from "node:os";
import { join } from "node:path";
import { mkdirSync, writeFileSync, renameSync, existsSync, readFileSync } from "node:fs";

/** The engine's default sample rate. ffmpeg renders to this so the load path needs no resample;
 *  it is also baked into the cache key so a cached WAV always matches. (If the live engine runs at
 *  a different rate, the Rust load path resamples — playback stays correct, the cache just keys on
 *  the canonical rate.) */
export const ENGINE_RATE = 48000;

export interface GrabMeta {
  url: string;
  start: number;
  dur: number;
  sampleRate: number;
  channels?: number;
  frames?: number;
  createdAt: string;
  title?: string;
}

/** Normalize a YouTube URL to a canonical watch URL so incidental params/format don't fork the
 *  cache (and to validate it before we ever spawn). Non-YouTube http(s) URLs pass through with
 *  query/hash stripped; anything that isn't a valid http(s) URL throws. */
export function canonicalizeUrl(url: string): string {
  let u: URL;
  try {
    u = new URL(url.trim());
  } catch {
    throw new Error(`strobe: not a valid URL: ${url}`);
  }
  if (u.protocol !== "http:" && u.protocol !== "https:") {
    throw new Error(`strobe: only http(s) URLs are supported: ${url}`);
  }
  const host = u.hostname.replace(/^www\./, "");
  let id: string | null = null;
  if (host === "youtu.be") id = u.pathname.slice(1).split("/")[0] || null;
  else if (host.endsWith("youtube.com")) id = u.searchParams.get("v");
  if (id) return `https://www.youtube.com/watch?v=${id}`;
  return `${u.protocol}//${host}${u.pathname}`;
}

/** Stable disk-cache key for a grab. Same (url,start,dur,rate) → same key; any change → new key. */
export function grabKey(url: string, start: number, dur: number, rate = ENGINE_RATE): string {
  return createHash("sha256")
    .update(`${canonicalizeUrl(url)}|${start}|${dur}|${rate}`)
    .digest("hex")
    .slice(0, 16);
}

/** OS cache dir for grabbed clips (XDG / macOS / Windows aware). Created on demand. */
export function cacheDir(): string {
  const base =
    process.env.XDG_CACHE_HOME ||
    (process.platform === "darwin"
      ? join(homedir(), "Library", "Caches")
      : process.platform === "win32"
        ? process.env.LOCALAPPDATA || join(homedir(), "AppData", "Local")
        : join(homedir(), ".cache"));
  const dir = join(base, "strobe", "youtube");
  mkdirSync(dir, { recursive: true });
  return dir;
}

export const wavPath = (hash: string): string => join(cacheDir(), `${hash}.wav`);
export const metaPath = (hash: string): string => join(cacheDir(), `${hash}.json`);

/** Cache hit iff both the WAV and its metadata exist. */
export function cached(hash: string): boolean {
  return existsSync(wavPath(hash)) && existsSync(metaPath(hash));
}

export function readMeta(hash: string): GrabMeta | null {
  try {
    return JSON.parse(readFileSync(metaPath(hash), "utf8")) as GrabMeta;
  } catch {
    return null;
  }
}

/** Atomic write: write to `<path>.tmp` then rename, so a crashed grab never leaves a half file. */
export function writeAtomic(path: string, data: string | Buffer): void {
  const tmp = `${path}.tmp`;
  writeFileSync(tmp, data);
  renameSync(tmp, path);
}

export function writeMeta(hash: string, meta: GrabMeta): void {
  writeAtomic(metaPath(hash), JSON.stringify(meta, null, 2));
}
