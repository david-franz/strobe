# 05 — YouTube ingest

> **Legal / ToS / copyright — read first.** Downloading audio from YouTube can conflict with **YouTube's Terms of Service** and with **copyright**. `tekno` therefore keeps this path **opt-in and clearly documented**, **never the headline feature** ([roadmap.md#risks](../../roadmap.md#risks)). **Local files and microphone input are the always-safe paths** and are the primary, recommended way to sample. Treat any sampled material as you would in any production: clear it or transform it appropriately. This feature exists because grabbing a few seconds of a record is the practical reality of hip-hop sampling; it is built to be used responsibly, in short grabs, with the caveat surfaced to the user.

## Purpose

This feature lets a song pull a short clip from a URL and turn it into a playable, chop-able, stretch-able buffer:

```ts
const vox = await sample.fromYouTube("https://youtu.be/…", { start: 42, dur: 4 });
p1( vox.chop(8).rev() );
```

`sample.fromYouTube(url, {start, dur})` shells out to **`yt-dlp`** to fetch the audio, to **`ffmpeg`** to **trim to `dur` seconds starting at `start`** and decode to PCM/WAV, **caches the result on disk keyed by a `(url, start, dur)` hash**, and then routes it through the same **`loadSample`** path as any local file ([01-sample-decode-loading.md](01-sample-decode-loading.md)). It is purely a control-plane (TypeScript) feature — `yt-dlp`/`ffmpeg` are external CLIs invoked on a worker, never linked into the Rust engine and never anywhere near the audio thread ([architecture.md](../../architecture.md#the-core-principle-two-planes)).

`yt-dlp` + `ffmpeg` are the canonical ingest tools ([roadmap.md#phase-4](../../roadmap.md#phase-4--sampling)). The bootstrap that ships the engine binary also checks for them ([roadmap.md#phase-5](../../roadmap.md#phase-5--live-coding-dx)).

## Location

```
packages/tekno/
  src/
    sample/
      youtube.ts      # NEW — sample.fromYouTube(), dependency checks, shell pipeline
      load.ts         # (01) — loadPcm()/loadFile() reused as the final step
      cache.ts        # NEW — disk cache layout + (url,start,dur) hashing
```

This document owns `packages/tekno/src/sample/youtube.ts` and `packages/tekno/src/sample/cache.ts`. It produces no Rust code and no new opcode — it ends by calling the existing `loadSample` control call ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).

## Design

### Signature

```ts
// packages/tekno/src/sample/youtube.ts
export interface YouTubeGrabOpts {
  start?: number;      // seconds into the source to begin (default 0)
  dur?: number;        // seconds to keep (default 8; hard-capped, see maxDurSec)
  name?: string;       // display name (default derived from url)
  force?: boolean;     // bypass the cache and re-fetch
}

// Opt-in: throws a clear, documented error if yt-dlp/ffmpeg are missing or if
// the user hasn't acknowledged the ToS/copyright posture (see "consent gate").
export async function fromYouTube(
  url: string, opts?: YouTubeGrabOpts,
): Promise<SampleHandle>;   // SampleHandle from load.ts — ready to .chop/.stretch/.pitch
```

`fromYouTube` is attached to the `sample` namespace (`sample.fromYouTube(...)`) so it reads naturally next to `sample("local")`.

### The shell pipeline

The grab is two CLI stages, both `spawn`ed (never `exec` with an interpolated string — args passed as an array to avoid shell injection from the URL), run on a worker so the scheduler is never blocked:

```ts
// Conceptual pipeline (args as arrays; no shell string interpolation):

// 1) yt-dlp: fetch best audio-only stream to a temp file. -x extracts audio;
//    we ask for a compressed audio stream (small, fast) and let ffmpeg decode.
//    --no-playlist guards against a playlist URL; --quiet for clean logs.
yt-dlp \
  --no-playlist --quiet --no-warnings \
  -f "bestaudio" \
  -o "<tmp>/src.%(ext)s" \
  "<url>"

// 2) ffmpeg: seek to `start`, keep `dur` seconds, decode to a known PCM WAV:
//    32-bit float, the ENGINE sample rate (so loadPcm needs no resample, [01]),
//    stereo. -ss before -i = fast input seek; -t = duration.
ffmpeg -hide_banner -loglevel error \
  -ss <start> -t <dur> \
  -i "<tmp>/src.<ext>" \
  -ac 2 -ar <engineSampleRate> -c:a pcm_f32le \
  -y "<cacheDir>/<hash>.wav"
```

Then TS reads the resulting WAV bytes (or decodes via the Rust `decodeFile` helper) and calls the standard load path:

```ts
// 3) hand the trimmed PCM to the engine — same control call as any sample.
const handle = await loadFile(`${cacheDir}/${hash}.wav`);  // [01]: decode + loadSample
```

> Requesting `-ar <engineSampleRate>` and `pcm_f32le` from `ffmpeg` means the WAV is already at the engine rate in float — so the [01](01-sample-decode-loading.md#resample-to-the-engine-rate-rubato) resample step is a no-op and there is no double conversion.

### Dependency checks (friendly errors)

Before spawning anything, probe for the tools and fail with an actionable message — `yt-dlp`/`ffmpeg` are *not* npm deps; the user installs them ([roadmap.md#phase-5](../../roadmap.md#phase-5--live-coding-dx)):

```ts
async function ensureDeps(): Promise<void> {
  const missing: string[] = [];
  if (!(await hasBinary("yt-dlp")))  missing.push("yt-dlp");
  if (!(await hasBinary("ffmpeg")))  missing.push("ffmpeg");
  if (missing.length) {
    throw new TeknoError(
      `YouTube sampling needs ${missing.join(" and ")} on your PATH.\n` +
      `  install:  brew install ${missing.join(" ")}   # macOS\n` +
      `            (or see https://github.com/yt-dlp/yt-dlp + https://ffmpeg.org)\n` +
      `Tip: local files (sample("path.wav")) and mic input need no extra tools.`,
    );
  }
}
// hasBinary spawns `<bin> --version` and checks exit code (cross-platform `which` is unreliable).
```

### The consent gate (opt-in posture, enforced)

To make "opt-in" real rather than just documented, the first YouTube grab in a process requires explicit acknowledgement, so nobody downloads copyrighted audio by accident:

```ts
// One-time, per-process (or persisted in config): the user must opt in.
//   import { enableYouTubeSampling } from "tekno";
//   enableYouTubeSampling({ iUnderstandToS: true });
// If not enabled, fromYouTube throws a TeknoError pointing at the docs and the
// always-safe local-file / mic alternatives.
```

This keeps local files and the mic as the frictionless default and puts a deliberate, documented step in front of the network path.

### Disk cache (`cache.ts`)

Grabs are deterministic in `(url, start, dur)` and expensive (a network fetch + transcode), so they are cached. Re-running a song line during live coding hits the cache, not the network — fast and avoids hammering YouTube.

```ts
// packages/tekno/src/sample/cache.ts
import { createHash } from "node:crypto";

export function grabKey(url: string, start: number, dur: number): string {
  // Stable key independent of incidental URL noise; include engine rate so a
  // cached WAV always matches the current engine (rate is baked into the file).
  return createHash("sha256")
    .update(`${canonicalizeUrl(url)}|${start}|${dur}|${engineSampleRate}`)
    .digest("hex").slice(0, 16);
}
```

**Cache layout** (under an OS cache dir, e.g. `~/.cache/tekno/youtube/` / `%LOCALAPPDATA%/tekno/...`):

```
~/.cache/tekno/youtube/
  <hash>.wav          # the trimmed, engine-rate, f32 PCM — what loadFile reads
  <hash>.json         # metadata: { url, start, dur, sampleRate, channels, frames,
                      #             createdAt, ytDlpVersion, title? } for inspection/GC
```

- **Hit:** `<hash>.wav` exists and its `.json` matches → skip both CLIs, go straight to `loadFile`.
- **Miss / `force`:** run the pipeline, write `<hash>.wav` + `<hash>.json` atomically (write to `*.tmp`, then rename).
- **GC:** a size/age cap on the cache dir (LRU by `createdAt`/access), with a `tekno cache clear` CLI (Phase 5). Temp download files are always cleaned up, even on failure.

### Where it runs (real-time safety)

Everything here is TS control-plane work on a worker: spawning CLIs, reading files, hashing, and finally one `loadSample` control call ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)). **None of it touches the audio thread.** `fromYouTube` is `async`; while a grab is in flight the pattern referencing it is "pending" and simply triggers nothing until the handle resolves ([01](01-sample-decode-loading.md#interfaces)) — the scheduler never blocks and the engine never glitches.

## Interfaces

- **Final control call:** ends in `loadFile`/`loadPcm` → **`loadSample(pcm, channels, sampleRate)`** ([01](01-sample-decode-loading.md), [event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)), returning a `SampleHandle`/sample id. **No new opcode, no engine code.**
- **Pattern engine:** the returned `SampleHandle` flows straight into `sample(handle)` and the whole edit chain — `vox.chop(8)`, `.slice(...)`, `.stretch(2)`, `.pitch(-3)`, `.rev()` ([04](04-chop-slice-granular.md), [03](03-timestretch-pitchshift.md)). A YouTube grab is indistinguishable from a local sample downstream; only its *origin* differs.
- **External CLIs:** `yt-dlp` (fetch) and `ffmpeg` (trim + decode), `spawn`ed with array args on a worker. Probed via `ensureDeps()` before use.
- **Config / consent:** `enableYouTubeSampling({ iUnderstandToS: true })` gates the path; the bootstrap dependency check ([roadmap.md#phase-5](../../roadmap.md#phase-5--live-coding-dx)) surfaces missing tools at install time too.
- **Always-safe alternatives (documented alongside):** `sample("path/to.wav")` (local file, [01](01-sample-decode-loading.md)) and microphone input — neither needs `yt-dlp`/`ffmpeg` nor the consent gate, and both are the recommended default.

## Edge cases & risks

- **Legal / ToS / copyright (headline).** Restated up top: opt-in, gated by `enableYouTubeSampling`, never the headline feature, with local-file/mic always-safe alternatives front-and-center ([roadmap.md#risks](../../roadmap.md#risks)). The error and docs steer users toward responsible, short, transformed use.
- **Missing tools.** `ensureDeps()` fails *before* any network/spawn with an actionable, OS-aware install hint and a pointer to the no-extra-tools local path.
- **Command injection.** URLs and options are passed as **array args to `spawn`**, never interpolated into a shell string; the URL is validated/canonicalized first.
- **Real-time safety.** All work is on a worker; the only engine interaction is the non-real-time `loadSample`. A pending grab leaves the pattern silent, never stalling the scheduler or the audio thread.
- **Long downloads / huge sources.** `--no-playlist`, a `dur` hard cap (`maxDurSec`, default e.g. 30 s — the posture is *short* grabs), and `-t dur` keep the transcode tiny even if the source is long; the [01](01-sample-decode-loading.md) `maxDurationSec` guard is the backstop.
- **Network/tool failure.** `yt-dlp`/`ffmpeg` non-zero exit → a friendly `TeknoError` with the captured stderr tail; temp files cleaned; the song keeps playing (the sample just never resolves). `yt-dlp` breaks when YouTube changes — the error suggests `yt-dlp -U`.
- **Cache correctness.** Key includes the engine sample rate so a cached WAV always matches the running engine; atomic write-then-rename avoids half-written cache files; size/age GC bounds disk use.
- **Determinism of `start`.** `-ss` before `-i` is a fast (keyframe-ish) seek; for sample-accurate trims we can put `-ss` *after* `-i` (slower, exact) — default to the fast seek, with an `exactSeek` option for when frame-accuracy matters.

## Acceptance criteria

- [ ] **Produces a usable, cached, playable buffer (roadmap "done when").** *Integration test (network-gated / fixture):* `sample.fromYouTube(url, {start, dur})` returns a `SampleHandle` whose buffer is ~`dur` seconds at the engine rate and immediately plays via `TrigSample`; a second call with the same args is served from cache (no network) and returns the same content.
- [ ] **Trimmed correctly.** *Unit/integration test:* the produced WAV is `dur` (±a frame) seconds, `engineSampleRate`, `pcm_f32le`, stereo; the audio corresponds to `[start, start+dur)` of the source (fixture-based).
- [ ] **Dependency checks.** *Unit test:* with `yt-dlp`/`ffmpeg` absent from PATH, `fromYouTube` throws a clear `TeknoError` naming the missing tool, an install hint, and the local-file/mic alternative — *before* any spawn.
- [ ] **Opt-in enforced.** *Unit test:* without `enableYouTubeSampling(...)`, `fromYouTube` throws and does nothing on the network.
- [ ] **Cache key + layout.** *Unit test:* `grabKey` is stable for the same `(url,start,dur,rate)` and differs when any changes; a hit skips both CLIs; writes are atomic (`*.tmp` → rename); temp downloads are removed on success and on failure.
- [ ] **No injection / no shell string.** *Review + test:* CLIs are `spawn`ed with array args; a URL containing shell metacharacters cannot execute anything.
- [ ] **Real-time safe / non-blocking.** *Integration test:* triggering a grab while a song plays does not block the scheduler or cause an underrun; the pattern stays silent until the handle resolves, then plays. (`stats()` clean — cross-cutting DoD, [roadmap.md](../../roadmap.md#cross-cutting-definition-of-done-every-phase).)
- [ ] **Friendly failure.** *Unit test:* a `yt-dlp`/`ffmpeg` non-zero exit yields a `TeknoError` with the stderr tail and an update hint; the engine and song keep running.
