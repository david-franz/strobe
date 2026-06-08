// YouTube ingest (Phase 4, feature 05).
//
// Responsible use: downloading audio from YouTube can conflict with YouTube's Terms of Service and
// with copyright — prefer short, transformed grabs, and treat sampled material as you would in any
// production. Local files (sample("path.wav")) and mic input are always-safe alternatives.
//
// Purely control-plane: shells out to yt-dlp (fetch) + ffmpeg (trim + decode to engine-rate f32
// WAV), caches by (url,start,dur,rate), then routes through the same loadFile path as a local file
// (feature 01). Both CLIs ship bundled (ffmpeg-static + youtube-dl-exec) so a clean install works
// with no manual setup; a system install on PATH is preferred when present (so you can keep yt-dlp
// fresh with `yt-dlp -U`). The CLIs run as async child processes (spawn with array args — no shell
// string, so a hostile URL cannot inject); the scheduler/event loop is never blocked and nothing
// here is anywhere near the audio thread. A grab in flight simply leaves its pattern silent until
// it resolves.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, readdirSync, existsSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { loadFile, type SampleHandle } from "./load";
import { ENGINE_RATE, canonicalizeUrl, grabKey, wavPath, cached, writeMeta } from "./cache";

const require = createRequire(import.meta.url);

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

/** Probe for a CLI by trying to run `<bin> --version`. Presence is decided by whether the tool
 *  **spawns** (a failure to launch — ENOENT — means it isn't on PATH), NOT by the version flag's
 *  exit code: `ffmpeg --version` exits non-zero (its flag is `-version`) yet ffmpeg is clearly
 *  installed once it runs. So we resolve false only on the spawn `error`, true once it executes.
 *  (Cross-platform `which` is unreliable; actually running the tool is the honest check.) */
export function hasBinary(bin: string, probeArgs: string[] = ["--version"]): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    const settle = (v: boolean) => {
      if (!settled) {
        settled = true;
        resolve(v);
      }
    };
    try {
      const child = spawn(bin, probeArgs, { stdio: "ignore" });
      child.on("error", () => settle(false)); // ENOENT etc. → not on PATH
      child.on("close", () => settle(true)); // it executed → installed (any exit code)
    } catch {
      settle(false);
    }
  });
}

/** Path to the ffmpeg binary bundled via `ffmpeg-static`, or null if unavailable. */
export function bundledFfmpeg(): string | null {
  try {
    const p = require("ffmpeg-static") as string | null;
    return p && existsSync(p) ? p : null;
  } catch {
    return null;
  }
}

/** Path to the yt-dlp binary bundled via `youtube-dl-exec`, or null if unavailable. */
export function bundledYtDlp(): string | null {
  try {
    const mod = require("youtube-dl-exec") as { constants?: { YOUTUBE_DL_PATH?: string } };
    const p = mod.constants?.YOUTUBE_DL_PATH;
    return p && existsSync(p) ? p : null;
  } catch {
    return null;
  }
}

/** Resolve a tool to a runnable command: prefer a system install on PATH (so you can keep yt-dlp
 *  updated with `yt-dlp -U`), else the binary bundled with strobe (so a clean install just works). */
async function resolveTool(name: string, bundled: () => string | null): Promise<string | null> {
  if (await hasBinary(name)) return name;
  return bundled();
}

/** Build the friendly, OS-aware "tools unavailable" error. Pure (testable without the network). */
export function missingDepsError(missing: string[]): StrobeError {
  return new StrobeError(
    `YouTube sampling needs ${missing.join(" and ")}, but neither a system install nor the\n` +
      `bundled binary could be found.\n` +
      `  try:  pnpm install            # re-fetches the bundled ffmpeg/yt-dlp\n` +
      `        brew install ${missing.join(" ")}   # or install system-wide (macOS)\n` +
      `Tip: local files (sample("path.wav")) and mic input need no extra tools.`,
  );
}

/** Resolve both CLIs to runnable paths (PATH install preferred, bundled fallback). Throws a
 *  friendly StrobeError naming whatever couldn't be found. */
async function resolveTools(): Promise<{ ffmpeg: string; ytDlp: string }> {
  const [ffmpeg, ytDlp] = await Promise.all([
    resolveTool("ffmpeg", bundledFfmpeg),
    resolveTool("yt-dlp", bundledYtDlp),
  ]);
  const missing: string[] = [];
  if (!ytDlp) missing.push("yt-dlp");
  if (!ffmpeg) missing.push("ffmpeg");
  if (missing.length || !ffmpeg || !ytDlp) throw missingDepsError(missing);
  return { ffmpeg, ytDlp };
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
 *  Returns a SampleHandle ready for sample()/.chop()/.stretch(). Cached on disk by (url,start,dur). */
export async function fromYouTube(url: string, opts: YouTubeGrabOpts = {}): Promise<SampleHandle> {
  const start = Math.max(0, opts.start ?? 0);
  const dur = Math.min(MAX_DUR_SEC, Math.max(0.1, opts.dur ?? 8));
  const canon = canonicalizeUrl(url); // validates + canonicalizes (throws on a bad URL)
  const hash = grabKey(url, start, dur);
  const name = opts.name ?? `yt:${hash}`;

  // Cache hit → straight to the normal load path, no network, no spawn.
  if (!opts.force && cached(hash)) return loadFile(wavPath(hash), name);

  const { ffmpeg, ytDlp } = await resolveTools();

  const tmp = mkdtempSync(join(tmpdir(), "strobe-yt-"));
  try {
    const dl = await run(ytDlp, ytDlpArgs(canon, join(tmp, "src.%(ext)s")));
    if (dl.code !== 0) {
      throw new StrobeError(
        `yt-dlp failed (exit ${dl.code}). YouTube may have changed — try \`yt-dlp -U\`.\n${tail(dl.stderr)}`,
      );
    }
    const srcFile = readdirSync(tmp).find((f) => f.startsWith("src."));
    if (!srcFile) throw new StrobeError("yt-dlp produced no output file.");

    const out = wavPath(hash);
    const ff = await run(ffmpeg, ffmpegArgs(join(tmp, srcFile), out, start, dur, opts.exactSeek ?? false));
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
