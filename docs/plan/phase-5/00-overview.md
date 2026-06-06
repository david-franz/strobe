# Phase 5 — Live-coding DX (overview)

## Purpose

Phases 0–4 build the *capability*: a Rust audio engine that makes sound ([Phase 1](../phase-1/00-overview.md)), the full pattern language and combinator vocabulary ([Phase 2](../phase-2/00-overview.md)), a production-grade effects rack including sidechain ([Phase 3](../phase-3/00-overview.md)), and sampling/chopping/stretching including optional YouTube grabs ([Phase 4](../phase-4/00-overview.md)). All of that is reachable today only by hand-wiring the runtime, as the [Phase 0 hot-reload demo](../phase-0/07-hot-reload-demo.md) does. **Phase 5 makes it pleasant to actually perform and compose** — it turns the spine into a tool a person sits down with.

Concretely, Phase 5 delivers: a hardened hot-reload **runtime** that re-evaluates only the tracks you changed and swaps them at cycle boundaries without ever restarting audio; a small **CLI** (`tekno play`, `tekno repl`, `tekno render`, `tekno devices`); a set of runnable, annotated **example songs** that show the end-to-end API; the **packaging** that lets a new user `npm i tekno` with no Rust toolchain (prebuilt per-OS napi-rs binaries); and the **getting-started + reference docs** that teach the language. None of this changes the two-plane architecture or the event protocol — it is purely the developer-experience layer that sits on top of the control plane in `packages/tekno/`.

This document is the index for Phase 5. Everything here is LOCKED to the canonical docs: the TS control plane lives in `packages/tekno/` ([architecture.md](../../architecture.md)); named tracks `p1..pN` are mutable slots that swap at the **next cycle boundary** ([architecture.md](../../architecture.md#tracks-and-hot-swapping)); the native engine is a `napi-rs` addon that owns the `cpal` thread ([architecture.md](../../architecture.md#the-bridge-ts--rust)); install is `npm i` via **prebuilt per-OS binaries**, the Rust toolchain being a contributor concern only ([roadmap.md](../../roadmap.md#risks)); and `yt-dlp`/`ffmpeg` are **optional** external CLIs for Phase 4 sampling, never required for playback.

## The features

| # | File | One-liner |
| --- | --- | --- |
| 01 | [01-hot-reload-runtime.md](01-hot-reload-runtime.md) | `runtime.ts`: the REPL + file-watcher that hot-reloads a song file, re-evaluating only the **changed** tracks and swapping them at cycle boundaries **without restarting audio**; isolated evaluation, track-assignment capture, diff-vs-previous, keep-last-good on a broken edit. Generalizes the [Phase 0 demo](../phase-0/07-hot-reload-demo.md). |
| 02 | [02-cli.md](02-cli.md) | The `tekno` CLI: `play song.ts` (load + watch + play), `repl` (interactive), `render song.ts out.wav` (offline bounce), `devices` (list audio devices). Arg parsing, config (sample rate / buffer / lookahead), graceful Ctrl-C. |
| 03 | [03-example-songs.md](03-example-songs.md) | `examples/`: a techno loop with sidechain, a hip-hop beat with a chopped sample, an evolving filter-sweep pad, and a getting-started "hello kick" — each a runnable, annotated `.ts` song exercising the API end-to-end. |
| 04 | [04-packaging-prebuilts.md](04-packaging-prebuilts.md) | Shipping prebuilt per-OS napi-rs binaries (macOS x64/arm64, Linux x64/arm64, Windows x64) via CI as `@tekno/engine-*` optional dependencies, so `npm i tekno` needs no Rust; source-build fallback for contributors; optional `yt-dlp`/`ffmpeg` presence check. |
| 05 | [05-getting-started-docs.md](05-getting-started-docs.md) | The user-facing doc set under `docs/guide/`: install, first song, mini-notation cheat sheet, chain-API reference, combinator reference, sampling how-to (+ YouTube legal note), troubleshooting (audio device / latency). |

## Dependency on earlier phases

- **[Phase 0](../phase-0/00-overview.md)** — the runtime, scheduler, clock, track slots, and napi `Engine` interface this phase generalizes. [07-hot-reload-demo.md](../phase-0/07-hot-reload-demo.md) is the direct precursor of [01](01-hot-reload-runtime.md).
- **[Phase 1](../phase-1/00-overview.md)** — synth/drum voices, so the example songs make real sound.
- **[Phase 2](../phase-2/00-overview.md)** — the mini-notation parser, combinators, and chain API the docs teach and the examples use.
- **[Phase 3](../phase-3/00-overview.md)** — the effects rack (`.lpf`, `.room`, `.sidechain`, …) the techno/pad examples lean on.
- **[Phase 4](../phase-4/00-overview.md)** — `sample()`, `.chop()`, and the optional `fromYouTube` ingest the hip-hop example and the sampling how-to use.

Phase 5 adds **no new audio capability**; it is the ergonomics and distribution layer over everything above. The web backend ([Phase 6](../../roadmap.md#phase-6--web-backend-later)) reuses the same runtime/CLI surface unchanged because it satisfies the same `Engine` interface.

## Done when (from [roadmap.md](../../roadmap.md#phase-5--live-coding-dx))

Phase 5 is **done** when both of the following hold:

- **`tekno play song.ts` runs; saving the file updates the music live with no restart.** The CLI ([02](02-cli.md)) boots the runtime ([01](01-hot-reload-runtime.md)), plays the song, watches it, and on every save re-evaluates only the changed tracks and swaps them at the next cycle boundary — the engine never stops, there are no clicks or dropouts, and `stats()` stays clean across the edit (the [Phase 0 verification recipe](../phase-0/07-hot-reload-demo.md#acceptance-criteria-the-phase-0-verification-recipe) extended to multi-track songs).
- **A new user can install with `npm i`, no Rust toolchain required.** `npm i tekno` pulls a prebuilt addon for the user's OS/arch via the `@tekno/engine-*` optional-dependency mechanism ([04](04-packaging-prebuilts.md)); there is no `cargo`/`node-gyp` step. Contributors can still build from source. `yt-dlp`/`ffmpeg` stay optional and are only needed for the Phase 4 sampling paths.

Per the cross-cutting definition of done ([roadmap.md](../../roadmap.md#cross-cutting-definition-of-done-every-phase)): every example in [03](03-example-songs.md) is runnable and underrun-free under normal load; the pure-TS runtime logic (diffing, isolated eval, boundary swap) has unit tests; and the whole thing is validated on macOS first, then Linux and Windows in CI before sign-off.
