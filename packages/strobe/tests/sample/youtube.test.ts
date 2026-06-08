import { describe, it, expect } from "vitest";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  canonicalizeUrl,
  grabKey,
  wavPath,
  cached,
  writeAtomic,
  writeMeta,
  ENGINE_RATE,
} from "../../src/sample/cache";
import {
  missingDepsError,
  hasBinary,
  bundledFfmpeg,
  bundledYtDlp,
  ytDlpArgs,
  ffmpegArgs,
  MAX_DUR_SEC,
  StrobeError,
} from "../../src/sample/youtube";

describe("youtube cache keys", () => {
  it("canonicalizes youtu.be and watch URLs identically", () => {
    expect(canonicalizeUrl("https://youtu.be/abc123?t=10")).toBe(
      "https://www.youtube.com/watch?v=abc123",
    );
    expect(canonicalizeUrl("https://www.youtube.com/watch?v=abc123&list=xyz")).toBe(
      "https://www.youtube.com/watch?v=abc123",
    );
  });

  it("rejects non-http(s) and invalid URLs", () => {
    expect(() => canonicalizeUrl("ftp://x/y")).toThrow();
    expect(() => canonicalizeUrl("not a url")).toThrow();
  });

  it("grabKey is stable for the same inputs and differs when any change", () => {
    const a = grabKey("https://youtu.be/abc", 10, 4);
    expect(grabKey("https://www.youtube.com/watch?v=abc", 10, 4)).toBe(a); // canonical-equal
    expect(grabKey("https://youtu.be/abc", 11, 4)).not.toBe(a); // start
    expect(grabKey("https://youtu.be/abc", 10, 5)).not.toBe(a); // dur
    expect(grabKey("https://youtu.be/abc", 10, 4, 44100)).not.toBe(a); // rate
  });

  it("cached() sees an atomically written wav+meta pair", () => {
    const dir = mkdtempSync(join(tmpdir(), "strobe-cache-"));
    const prev = process.env.XDG_CACHE_HOME;
    process.env.XDG_CACHE_HOME = dir;
    try {
      const hash = "deadbeefcafe0001";
      expect(cached(hash)).toBe(false);
      writeAtomic(wavPath(hash), Buffer.from([1, 2, 3, 4]));
      writeMeta(hash, {
        url: "https://www.youtube.com/watch?v=x",
        start: 0,
        dur: 4,
        sampleRate: ENGINE_RATE,
        createdAt: "t",
      });
      expect(cached(hash)).toBe(true);
      expect(wavPath(hash).startsWith(dir)).toBe(true);
    } finally {
      if (prev === undefined) delete process.env.XDG_CACHE_HOME;
      else process.env.XDG_CACHE_HOME = prev;
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("youtube ingest (bundled tools, injection-safe)", () => {
  it("bundles runnable ffmpeg + yt-dlp binaries (installed via deps)", () => {
    const ff = bundledFfmpeg();
    const yt = bundledYtDlp();
    expect(ff, "ffmpeg-static binary present").toBeTruthy();
    expect(existsSync(ff!)).toBe(true);
    expect(yt, "youtube-dl-exec binary present").toBeTruthy();
    expect(existsSync(yt!)).toBe(true);
  });

  it("missingDepsError names the tools, an install hint, and the safe alternative", () => {
    const e = missingDepsError(["yt-dlp", "ffmpeg"]);
    expect(e).toBeInstanceOf(StrobeError);
    expect(e.message).toMatch(/yt-dlp and ffmpeg/);
    expect(e.message).toMatch(/brew install yt-dlp ffmpeg/);
    expect(e.message).toMatch(/local files/);
  });

  it("hasBinary is false for a nonsense binary, true for this node runtime", async () => {
    expect(await hasBinary("definitely-not-real-binary-xyzzy")).toBe(false);
    expect(await hasBinary(process.execPath)).toBe(true);
  });

  it("hasBinary reports a tool that runs but exits non-zero as present (the `ffmpeg --version` case)", async () => {
    // ffmpeg's version flag is `-version`; `ffmpeg --version` exits non-zero yet ffmpeg IS installed.
    // Presence must be decided by spawnability, not the probe's exit code.
    expect(await hasBinary(process.execPath, ["-e", "process.exit(8)"])).toBe(true);
  });

  it("CLI args are arrays with the URL un-interpolated (no shell injection)", () => {
    const evil = "https://evil/$(rm -rf ~); echo pwned";
    const args = ytDlpArgs(evil, "/tmp/src.%(ext)s");
    expect(Array.isArray(args)).toBe(true);
    expect(args[args.length - 1]).toBe(evil); // one verbatim element — never a shell string
    expect(args).toContain("--no-playlist");
  });

  it("ffmpegArgs trims to dur, renders engine-rate f32 stereo, honours seek order", () => {
    const fast = ffmpegArgs("/in.webm", "/out.wav", 12, 4, false);
    expect(fast).toEqual(
      expect.arrayContaining(["-t", "4", "-ar", String(ENGINE_RATE), "-c:a", "pcm_f32le", "-ac", "2"]),
    );
    expect(fast.indexOf("-ss")).toBeLessThan(fast.indexOf("-i")); // fast: -ss before -i
    const exact = ffmpegArgs("/in.webm", "/out.wav", 12, 4, true);
    expect(exact.indexOf("-i")).toBeLessThan(exact.indexOf("-ss")); // exact: -ss after -i
  });

  it("keeps the grab-length cap short", () => {
    expect(MAX_DUR_SEC).toBeLessThanOrEqual(60);
  });
});
