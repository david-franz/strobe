# Phase 4 — Sampling (overview)

> **Legal / ToS note, up front.** `strobe`'s YouTube ingest shells out to `yt-dlp`. That works *technically*, but downloading audio from YouTube can conflict with YouTube's Terms of Service and with copyright. We keep YouTube sampling **opt-in and clearly documented**, never the headline feature. **Local files and microphone input are the always-safe paths.** Treat sampled material as you would in any production: clear it or transform it appropriately. See [roadmap.md#risks](../../roadmap.md#risks) and [05-youtube-ingest.md](05-youtube-ingest.md) for the full posture.

## Purpose

Phase 4 makes `strobe` a sampler — the heart of hip-hop and a staple of techno. Up to here the engine has only ever *synthesized* sound (the kick of Phase 0, the synth/drum voices of Phase 1, sculpted by the pattern language of Phase 2 and the effects rack of Phase 3). Phase 4 adds the other half of the craft: take *recorded* audio — a drum break, a vocal chop, a few seconds grabbed from a record or a video — and **load it, chop it, stretch it, and pitch it** under the same pattern language and the same two-plane architecture.

The capability decomposes into a clean stack, each layer building on the one below:

1. **Decode + load** recorded audio (WAV/MP3/FLAC/OGG) to Float32 interleaved PCM, **off the audio thread**, resample it to the engine's sample rate, and hand it to the engine as an immutable buffer addressed by a numeric **sample id** (the `loadSample()` control call from [event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).
2. **Play** that buffer back as a real-time-safe voice (`voices/sampler.rs`) with variable speed/pitch, gain, start offset, one-shot/loop, and reverse — triggered by the **`TrigSample` (`0x20`)** opcode.
3. **Time-stretch + pitch-shift, decoupled** (change tempo without changing pitch and vice versa) via `ssstretch` / Signalsmith Stretch, kept behind a swappable Rust `StretchEngine` trait, done **offline at load/prepare time**.
4. **Chop / slice / granular** — `sample/edit.ts` plus engine support — splitting a buffer into slices triggerable across a pattern, and granular textures, all expressed in the same mini-notation and combinator vocabulary.
5. **YouTube ingest** — `sample/youtube.ts`'s `sample.fromYouTube(url, {start, dur})` — `yt-dlp` to fetch, `ffmpeg` to trim + decode, cached on disk, then `loadSample` into the engine.

The whole phase obeys the project's one non-negotiable rule: **the audio plane does no allocation, no locks, and no JS, and never decodes or stretches on the audio thread** (see [architecture.md](../../architecture.md#the-core-principle-two-planes)). All decoding, resampling, and time-stretch happen on a non-real-time setup thread; the audio thread only ever *reads an already-prepared immutable buffer*.

## Dependency on Phases 0–3

Phase 4 is breadth layered on the proven spine; it reuses every earlier layer rather than re-deriving it.

| Needs from | What |
| --- | --- |
| **Phase 0** ([plan/phase-0](../phase-0/00-overview.md)) | The two planes, the SPSC event ring + frame-stamped consumer (`scheduler.rs`), the fixed **32-byte record** layout, the `napi-rs` `Engine` bridge (`start`/`stop`/`pushEvents`/`stats`), the cps clock, and the look-ahead scheduler. `TrigSample` rides the same ring as `NoteOn`; `loadSample` is a control call on the same bridge. |
| **Phase 1** ([roadmap.md#phase-1](../../roadmap.md#phase-1--synthesis-core)) | `voice_alloc.rs` (polyphony + oldest/quietest voice stealing) — the sampler is just another voice type in the same pool. The master bus + limiter that catch any sample that runs hot. |
| **Phase 2** ([pattern-model.md](../../pattern-model.md)) | The `Pattern`/`Hap`/`TimeSpan` core, the mini-notation parser, the combinators (`rev`, `fast`, `every`, `jux`, …), and the chainable modifier API. `sample(...)`/`.chop(n)`/`.slice(n)` produce ordinary patterns of control maps; `.rev()` is just the existing `rev`. The scheduler compiling `Hap`s → events already exists; Phase 4 only adds the `TrigSample` encoding path. |
| **Phase 3** ([roadmap.md#phase-3](../../roadmap.md#phase-3--effects-rack)) | The per-track effects rack and sends. A chopped sample feeds the same filter/delay/reverb/sidechain chain as any synth voice; nothing in the effects layer is sampler-specific. |

Phase 4 introduces two **new crates** ([architecture.md](../../architecture.md#why-these-technology-choices)) and two **external CLIs**, all already named as canonical:

- `symphonia` — pure-Rust decoding of WAV/MP3/FLAC/OGG (no native codec deps).
- `ssstretch` (Signalsmith Stretch bindings) — high-quality decoupled time-stretch + pitch-shift, kept behind the `StretchEngine` trait.
- `yt-dlp` + `ffmpeg` — external CLIs, used **only** by the opt-in YouTube path, shelled out to from the TS control plane (never linked into the engine).

## The features

| # | File | One-liner |
| --- | --- | --- |
| 01 | [01-sample-decode-loading.md](01-sample-decode-loading.md) | `symphonia` decode of WAV/MP3/FLAC/OGG → Float32 interleaved PCM **off the audio thread**, resample to the engine rate, the `loadSample()` control call, and the engine-side sample bank (a `Vec` of `Arc<[f32]>` buffers, real-time-safe lookup by id) with memory management / eviction. |
| 02 | [02-sampler-voice.md](02-sampler-voice.md) | `voices/sampler.rs`: buffer playback with variable speed/pitch (fractional read index + linear/cubic interpolation), gain, start offset (`arg2`), one-shot vs looped, reverse — triggered by `TrigSample`, reading a shared immutable buffer, integrated with `voice_alloc`. |
| 03 | [03-timestretch-pitchshift.md](03-timestretch-pitchshift.md) | `ssstretch`/Signalsmith **decoupled** time-stretch + pitch-shift behind a swappable Rust `StretchEngine` trait, done **offline** at load/prepare time (vs real-time options); the `.stretch(x)` / `.pitch(semitones)` API; quality/artifact tradeoffs. |
| 04 | [04-chop-slice-granular.md](04-chop-slice-granular.md) | `sample/edit.ts` + engine support: `.chop(n)`, `.slice(n, pattern)`, `.loop()`, and granular playback (grain size/density/scatter); how slices map to `TrigSample` start offsets and to mini-notation. Worked example: `sample("amen").chop(8).rev()`. |
| 05 | [05-youtube-ingest.md](05-youtube-ingest.md) | `sample/youtube.ts`: `sample.fromYouTube(url, {start, dur})` — `yt-dlp` fetch → `ffmpeg` trim + decode to PCM → disk cache keyed by `(url, start, dur)` hash → `loadSample`. Dependency checks with friendly errors; the legal/ToS posture and the local-file + mic alternatives. |

## Done when (from [roadmap.md](../../roadmap.md#phase-4--sampling))

Phase 4 is **done** when all of the following hold:

- **Chop a local sample across a pattern.** `sample("amen").chop(8).rev()` (and the like) plays: a local break is split into 8 slices, distributed across the cycle, and reversed — audibly correct, click-free at slice boundaries. (See [04](04-chop-slice-granular.md).)
- **Time-stretch holds pitch while changing tempo, with acceptable artifacts.** `.stretch(x)` changes a sample's duration without changing its pitch, and `.pitch(semitones)` changes pitch without changing duration; both via the `StretchEngine` trait, with artifacts in the range expected of Signalsmith Stretch. (See [03](03-timestretch-pitchshift.md).)
- **YouTube grab produces a usable, cached, playable buffer.** `sample.fromYouTube(url, {start, dur})` fetches, trims, decodes, caches by `(url, start, dur)`, and `loadSample`s a buffer you can immediately `chop`/`stretch`/`pitch` — with `yt-dlp`/`ffmpeg` dependency checks and friendly errors, behind the opt-in legal posture. (See [05](05-youtube-ingest.md).)

Plus the **cross-cutting definition of done** that applies to every phase ([roadmap.md](../../roadmap.md#cross-cutting-definition-of-done-every-phase)): a runnable `examples/` song exercising sampling; **zero underruns** under that example's load (`stats()` clean — the sampler voice and the offline decode/stretch never starve the audio thread); pure-TS logic (the edit/chop math, the cache-key hashing, the slice→offset mapping) unit-tested and the DSP (interpolation, stretch) checked with golden/null tests where practical; validated on macOS first, Linux/Windows confirmed in CI.
