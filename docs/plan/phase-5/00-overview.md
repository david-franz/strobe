# Phase 5 — Live-coding DX (overview)

## Purpose

Phases 0–4 build the *capability*: a Rust audio engine that makes sound ([Phase 1](../phase-1/00-overview.md)), the full pattern language and combinator vocabulary ([Phase 2](../phase-2/00-overview.md)), a production-grade effects rack including sidechain ([Phase 3](../phase-3/00-overview.md)), and sampling/chopping/stretching including optional YouTube grabs ([Phase 4](../phase-4/00-overview.md)). All of that is reachable today only by hand-wiring the runtime, as the [Phase 0 hot-reload demo](../phase-0/07-hot-reload-demo.md) does. **Phase 5 makes it pleasant to actually perform and compose** — it turns the spine into a tool a person sits down with.

Concretely, Phase 5 delivers: a hardened hot-reload **runtime** that re-evaluates only the tracks you changed and swaps them at cycle boundaries without ever restarting audio; a small **CLI** (`strobe play`, `strobe repl`, `strobe render`, `strobe bounce`, `strobe devices`); a set of runnable, annotated **example songs** that show the end-to-end API; **recording** — both a deterministic **session** capture of a whole live performance and real-time **audio capture** of the master mix plus per-track **stems**; the **packaging** that lets a new user `npm i strobe` with no Rust toolchain (prebuilt per-OS napi-rs binaries); and the **getting-started + reference docs** that teach the language. None of this changes the two-plane architecture or the event protocol — it is purely the developer-experience layer that sits on top of the control plane in `packages/strobe/` (recording adds two non-real-time control calls, `startCapture`/`stopCapture`, but no event opcodes).

This document is the index for Phase 5. Everything here is LOCKED to the canonical docs: the TS control plane lives in `packages/strobe/` ([architecture.md](../../architecture.md)); named tracks `p1..pN` are mutable slots that swap at the **next cycle boundary** ([architecture.md](../../architecture.md#tracks-and-hot-swapping)); the native engine is a `napi-rs` addon that owns the `cpal` thread ([architecture.md](../../architecture.md#the-bridge-ts--rust)); install is `npm i` via **prebuilt per-OS binaries**, the Rust toolchain being a contributor concern only ([roadmap.md](../../roadmap.md#risks)); and `yt-dlp`/`ffmpeg` are **optional** external CLIs for Phase 4 sampling, never required for playback.

## The features

| # | File | One-liner |
| --- | --- | --- |
| 01 | [01-hot-reload-runtime.md](01-hot-reload-runtime.md) | `runtime.ts`: the REPL + file-watcher that hot-reloads a song file, re-evaluating only the **changed** tracks and swapping them at cycle boundaries **without restarting audio**; isolated evaluation, track-assignment capture, diff-vs-previous, keep-last-good on a broken edit. Generalizes the [Phase 0 demo](../phase-0/07-hot-reload-demo.md). |
| 02 | [02-cli.md](02-cli.md) | The `strobe` CLI: `play song.ts` (load + watch + play, with `--rec`/`--stems`/`--session`), `repl` (interactive), `render song.ts out/` (offline bounce of a static song), `bounce session out/` (re-render a recorded session), `devices` (list audio devices). Arg parsing, config (sample rate / buffer / lookahead), graceful Ctrl-C. |
| 03 | [03-example-songs.md](03-example-songs.md) | `examples/`: a techno loop with sidechain, a hip-hop beat with a chopped sample, an evolving filter-sweep pad, and a getting-started "hello kick" — each a runnable, annotated `.ts` song exercising the API end-to-end. |
| 04 | [04-packaging-prebuilts.md](04-packaging-prebuilts.md) | Shipping prebuilt per-OS napi-rs binaries (macOS x64/arm64, Linux x64/arm64, Windows x64) via CI as `@strobe/engine-*` optional dependencies, so `npm i strobe` needs no Rust; source-build fallback for contributors; optional `yt-dlp`/`ffmpeg` presence check. |
| 05 | [05-session-recording.md](05-session-recording.md) | `record/`: deterministic capture of a whole live performance by teeing the realized 32-byte event stream (+ instruments + sample buffers) to a self-contained `.strobesession` bundle, and re-rendering it offline to pristine master + stems. Captures every live change, not just the static file. |
| 06 | [06-audio-capture-stems.md](06-audio-capture-stems.md) | `capture.rs` + `record/capture.ts`: real-time capture of the actual audio to disk — master mix **and per-track stems** (the channel loops kept separate) — via lock-free taps → a writer thread (no audio-thread I/O). Reused by offline `bounce`/`render`. |
| 07 | [07-getting-started-docs.md](07-getting-started-docs.md) | The user-facing doc set under `docs/guide/`: install, first song, mini-notation cheat sheet, chain-API reference, combinator reference, sampling how-to (+ YouTube legal note), recording/stems how-to, troubleshooting (audio device / latency). Written **last**, once every feature it documents exists. |

## Dependency on earlier phases

- **[Phase 0](../phase-0/00-overview.md)** — the runtime, scheduler, clock, track slots, and napi `Engine` interface this phase generalizes. [07-hot-reload-demo.md](../phase-0/07-hot-reload-demo.md) is the direct precursor of [01](01-hot-reload-runtime.md).
- **[Phase 1](../phase-1/00-overview.md)** — synth/drum voices, so the example songs make real sound.
- **[Phase 2](../phase-2/00-overview.md)** — the mini-notation parser, combinators, and chain API the docs teach and the examples use.
- **[Phase 3](../phase-3/00-overview.md)** — the effects rack (`.lpf`, `.room`, `.sidechain`, …) the techno/pad examples lean on.
- **[Phase 4](../phase-4/00-overview.md)** — `sample()`, `.chop()`, and the optional `fromYouTube` ingest the hip-hop example and the sampling how-to use.
- **[Phase 1 · master bus](../phase-1/07-master-bus-limiter.md)** and **[Phase 3 · effects framework](../phase-3/01-effects-framework.md)** — supply the master and per-track post-FX tap points that [audio capture & stems](06-audio-capture-stems.md) reads from.

Phase 5 adds **no new sound-generating capability**; it is the ergonomics, recording, and distribution layer over everything above. The web backend ([Phase 6](../../roadmap.md#phase-6--web-backend-later)) reuses the same runtime/CLI surface unchanged because it satisfies the same `Engine` interface.

## Done when (from [roadmap.md](../../roadmap.md#phase-5--live-coding-dx))

Phase 5 is **done** when both of the following hold:

- **`strobe play song.ts` runs; saving the file updates the music live with no restart.** The CLI ([02](02-cli.md)) boots the runtime ([01](01-hot-reload-runtime.md)), plays the song, watches it, and on every save re-evaluates only the changed tracks and swaps them at the next cycle boundary — the engine never stops, there are no clicks or dropouts, and `stats()` stays clean across the edit (the [Phase 0 verification recipe](../phase-0/07-hot-reload-demo.md#acceptance-criteria-the-phase-0-verification-recipe) extended to multi-track songs).
- **A new user can install with `npm i`, no Rust toolchain required.** `npm i strobe` pulls a prebuilt addon for the user's OS/arch via the `@strobe/engine-*` optional-dependency mechanism ([04](04-packaging-prebuilts.md)); there is no `cargo`/`node-gyp` step. Contributors can still build from source. `yt-dlp`/`ffmpeg` stay optional and are only needed for the Phase 4 sampling paths.
- **A performance can be recorded and re-rendered to master + stems.** `strobe play song.ts --rec takes/ --stems` captures the actual audio of a live session to `master.wav` plus one file per channel loop, with no audio-thread underruns ([06](06-audio-capture-stems.md)); and a `.strobesession` recorded with `--session` re-renders deterministically via `strobe bounce` to byte-identical master + stems ([05](05-session-recording.md)).

Per the cross-cutting definition of done ([roadmap.md](../../roadmap.md#cross-cutting-definition-of-done-every-phase)): every example in [03](03-example-songs.md) is runnable and underrun-free under normal load; the pure-TS runtime logic (diffing, isolated eval, boundary swap) has unit tests; and the whole thing is validated on macOS first, then Linux and Windows in CI before sign-off.
