// YouTube ingest (Phase 4, feature 05) — OPT-IN, never the headline path.
//
// Legal / ToS / copyright: downloading audio from YouTube can conflict with YouTube's Terms of
// Service and with copyright. This path is therefore opt-in (enableYouTubeSampling) and clearly
// documented. Local files (sample("path.wav")) and mic input are the always-safe, recommended
// ways to sample. Treat grabbed material as you would in any production: short, transformed, cleared.
//
// Purely control-plane: shells out to yt-dlp (fetch) + ffmpeg (trim + decode to engine-rate f32
// WAV), caches by (url,start,dur,rate), then routes through the same loadFile path as a local file
// (feature 01). The CLIs run as async child processes (spawn with array args — no shell string, so
// a hostile URL cannot inject), so the scheduler/event loop is never blocked; nothing here is
// anywhere near the audio thread. A grab in flight simply leaves its pattern silent until it resolves.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { loadFile, type SampleHandle } from "./load";
import {
  ENGINE_RATE,
  canonicalizeUrl,
  grabKey,
  wavPath,
  cached,
  writeMeta,
} from "./cache";

/** A strobe-level error with an actionable message (distinct from arbitrary runtime errors). */
export class StrobeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "StrobeError";
  }
}

export interface YouTubeGrabOpts {
  start?: number; // seconds into the source to begin (default 0)
  dur?: number; // seconds to keep (default 8; hard-capped at MAX_DUR_SEC)
  name?: string; // display name (default derived from the hash)
  force?: boolean; // bypass the cache and re-fetch
  exactSeek?: boolean; // exact (slower) seek instead of the fast keyframe seek
}

/** Hard cap on grab length — the posture is *short* grabs (a few seconds of a record). */
export const MAX_DUR_SEC = 30;

let consented = false;

/** Opt in to the YouTube path (acknowledging the ToS/copyright posture). Required before the first
 *  grab; pass `{ iUnderstandToS: false }` to revoke. */
export function enableYouTubeSampling(opts: { iUnderstandToS: boolean }): void {
  consented = opts.iUnderstandToS === true;
}

/** Whether the YouTube path has been opted into this process (introspection/tests). */
export function youTubeSamplingEnabled(): boolean {
  return consented;
}

/** Probe for a CLI by running `<bin> --version`; resolves true iff it runs and exits 0.
 *  (Cross-platform `which` is unreliable; actually running the tool is the honest check.) */
export function hasBinary(bin: string): Promise<boolean> {
  return new Promise((resolve) => {
    try {
      const child = spawn(bin, ["--version"], { stdio: "ignore" });
      child.on("error", () => resolve(false));
      child.on("close", (code) => resolve(code === 0));
    } catch {
      resolve(false);
    }
  });
}

/** Build the friendly, OS-aware "install these tools" error. Pure (testable without the network). */
export function missingDepsError(missing: string[]): StrobeError {
  return new StrobeError(
    `YouTube sampling needs ${missing.join(" and ")} on your PATH.\n` +
      `  install:  brew install ${missing.join(" ")}   # macOS\n` +
      `            (or see https://github.com/yt-dlp/yt-dlp + https://ffmpeg.org)\n` +
      `Tip: local files (sample("path.wav")) and mic input need no extra tools.`,
  );
}

async function ensureDeps(): Promise<void> {
  const missing: string[] = [];
  if (!(await hasBinary("yt-dlp"))) missing.push("yt-dlp");
  if (!(await hasBinary("ffmpeg"))) missing.push("ffmpeg");
  if (missing.length) throw missingDepsError(missing);
}

/** yt-dlp args (array form — the URL is the final, un-interpolated element; no shell string). */
export function ytDlpArgs(url: string, outTemplate: string): string[] {
  return ["--no-playlist", "--quiet", "--no-warnings", "-f", "bestaudio", "-o", outTemplate, url];
}

/** ffmpeg args to trim [start, start+dur) and decode to engine-rate f32 stereo WAV. `-ss` before
 *  `-i` is a fast (keyframe) seek; after `-i` (exactSeek) is sample-accurate but slower. */
export function ffmpegArgs(
  input: string,
  out: string,
  start: number,
  dur: number,
  exactSeek: boolean,
): string[] {
  const seek = exactSeek ? ["-i", input, "-ss", String(start)] : ["-ss", String(start), "-i", input];
  return [
    "-hide_banner",
    "-loglevel",
    "error",
    ...seek,
    "-t",
    String(dur),
    "-ac",
    "2",
    "-ar",
    String(ENGINE_RATE),
    "-c:a",
    "pcm_f32le",
    "-y",
    out,
  ];
}

function run(bin: string, args: string[]): Promise<{ code: number; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, { stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    child.stderr?.on("data", (d: Buffer) => (stderr += d.toString()));
    child.on("error", (e) => reject(new StrobeError(`failed to run ${bin}: ${e.message}`)));
    child.on("close", (code) => resolve({ code: code ?? -1, stderr }));
  });
}

const tail = (s: string, n = 500): string => s.slice(-n).trim();

/** Pull a short clip from a URL and turn it into a playable, chop-able, stretch-able buffer.
 *  Opt-in (see enableYouTubeSampling). Returns a SampleHandle ready for sample()/.chop()/.stretch(). */
export async function fromYouTube(url: string, opts: YouTubeGrabOpts = {}): Promise<SampleHandle> {
  if (!consented) {
    throw new StrobeError(
      "YouTube sampling is opt-in. Call enableYouTubeSampling({ iUnderstandToS: true }) first.\n" +
        "Mind YouTube's Terms of Service and copyright; prefer short, transformed grabs.\n" +
        'Always-safe alternatives: sample("path/to.wav") (local file) and mic input.',
    );
  }
  const start = Math.max(0, opts.start ?? 0);
  const dur = Math.min(MAX_DUR_SEC, Math.max(0.1, opts.dur ?? 8));
  const canon = canonicalizeUrl(url); // validates + canonicalizes (throws on a bad URL)
  const hash = grabKey(url, start, dur);
  const name = opts.name ?? `yt:${hash}`;

  // Cache hit → straight to the normal load path, no network, no spawn.
  if (!opts.force && cached(hash)) return loadFile(wavPath(hash), name);

  await ensureDeps();

  const tmp = mkdtempSync(join(tmpdir(), "strobe-yt-"));
  try {
    const dl = await run("yt-dlp", ytDlpArgs(canon, join(tmp, "src.%(ext)s")));
    if (dl.code !== 0) {
      throw new StrobeError(
        `yt-dlp failed (exit ${dl.code}). YouTube may have changed — try \`yt-dlp -U\`.\n${tail(dl.stderr)}`,
      );
    }
    const srcFile = readdirSync(tmp).find((f) => f.startsWith("src."));
    if (!srcFile) throw new StrobeError("yt-dlp produced no output file.");

    const out = wavPath(hash);
    const ff = await run(
      "ffmpeg",
      ffmpegArgs(join(tmp, srcFile), out, start, dur, opts.exactSeek ?? false),
    );
    if (ff.code !== 0) throw new StrobeError(`ffmpeg failed (exit ${ff.code}).\n${tail(ff.stderr)}`);

    writeMeta(hash, {
      url: canon,
      start,
      dur,
      sampleRate: ENGINE_RATE,
      createdAt: new Date().toISOString(),
      title: opts.name,
    });
    return loadFile(out, name);
  } finally {
    rmSync(tmp, { recursive: true, force: true }); // always clean temp downloads
  }
}
