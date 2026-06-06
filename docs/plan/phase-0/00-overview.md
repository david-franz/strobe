# Phase 0 — Proof of spine (overview)

## Purpose

Phase 0 proves the single riskiest claim in the whole `tekno` design: **that TypeScript can drive a Rust audio engine, live, with zero glitches.** Everything else in the project — the synthesis core, the pattern language, the effects rack, sampling, the web backend — is breadth layered on top of this spine. If the spine is wrong (if hot-swapping a pattern clicks, if the audio thread underruns, if events arrive late), no amount of feature work fixes it. So before building any breadth we build the smallest possible *complete vertical slice* of the architecture and stress it.

That slice is: a `cpal` output stream rendering one synthesized kick voice; a lock-free SPSC ring buffer of fixed 32-byte event records carrying frame-stamped `NoteOn`s; a `napi-rs` addon exposing `start`/`stop`/`pushEvents`/`stats`; a TypeScript look-ahead scheduler with a cps clock and one track slot; and a file-watched example that lets you edit a four-on-the-floor kick and hear the change swap in seamlessly at the next cycle boundary. No polyphony, no filters, no effects, no samples, no mini-notation parser — just the two planes (see [architecture.md](../../architecture.md)) talking over the one protocol (see [event-protocol.md](../../event-protocol.md)) and producing on-time sound.

This document is the index for Phase 0. The design decisions are LOCKED to the canonical docs: two planes (TS soft-real-time control plane, Rust hard-real-time audio plane with **no alloc / locks / JS on the audio thread**), a lock-free SPSC ring of fixed 32-byte records, the "two clocks" look-ahead scheduler (~100 ms lookahead / ~25 ms tick), and cycle-based time with `cps` tempo. The crate choices are LOCKED to [architecture.md](../../architecture.md): `cpal`, `fundsp`, `napi-rs` (with `symphonia`, `ssstretch`, `wasm-bindgen` arriving in later phases).

## The features

| # | File | One-liner |
| --- | --- | --- |
| 01 | [01-audio-io-cpal.md](01-audio-io-cpal.md) | Open a `cpal` output stream; the audio callback and per-buffer render loop; the running `u64` frame counter that is the source of truth for timing. |
| 02 | [02-event-ring-buffer.md](02-event-ring-buffer.md) | The lock-free SPSC ring of fixed 32-byte records and the audio-thread consumer (`scheduler.rs`): drain `frame ≤ currentFrame + bufferLen`, apply at the right in-buffer offset, handle late and full. |
| 03 | [03-kick-voice.md](03-kick-voice.md) | A minimal synthesized kick as a `fundsp` graph (pitch envelope → sine → amp envelope: the classic click→body), one-shot-triggered from a `NoteOn`, allocation-free per trigger. |
| 04 | [04-napi-bridge.md](04-napi-bridge.md) | The `napi-rs` addon exposing the Phase 0 `Engine`: `start`, `stop`, `pushEvents`, `stats`; Rust owns the `cpal` thread; non-blocking `pushEvents`; build with no node-gyp. |
| 05 | [05-lookahead-scheduler.md](05-lookahead-scheduler.md) | The TS look-ahead scheduler: ~25 ms tick, query `[now, now+lookahead]`, cycle→seconds→frames, encode 32-byte records into a reusable `ArrayBuffer` via `DataView`, one `pushEvents` per tick. |
| 06 | [06-clock-and-tracks.md](06-clock-and-tracks.md) | The cps clock (`setcps`, `bpm = cps*4*60`), cycle↔wall-clock↔frame mapping, a single track slot `p1`, and the trivial "play `bd` every 1/4 cycle" pattern with the minimal `Pattern`/`Hap` shape. |
| 07 | [07-hot-reload-demo.md](07-hot-reload-demo.md) | `examples/phase0-kick.ts` plus the file-watcher runtime that re-evaluates the song and swaps `p1` at the next cycle boundary; the full end-to-end verification recipe. |

## Done when (from [roadmap.md](../../roadmap.md))

Phase 0 is **done** when all of the following hold:

- **Steady, on-time kick on macOS.** Running `examples/phase0-kick.ts` produces a four-on-the-floor kick that is audibly in time and does not drift. (Validated on macOS first; Linux/Windows confirmed in CI per the cross-cutting definition of done.)
- **Seamless hot-swap at the cycle boundary.** Editing the pattern in the watched file — e.g. `bd*4` → `bd*8`, or a Euclidean `bd(3,8)` — changes the loop **at the next cycle boundary** with **no clicks or dropouts**. The already-committed lookahead window is never retro-actively rewritten (see [architecture.md](../../architecture.md#tracks-and-hot-swapping)).
- **Clean telemetry.** Under normal load, `stats()` reports **zero underruns** and **zero late events** (`{ currentFrame, underruns, lateEvents, ringFullness }`; see [04-napi-bridge.md](04-napi-bridge.md)).
- **Provable real-time safety.** The audio thread provably does **no allocation, no locking, and no JS calls** — established by code review plus a stress check (see the acceptance sections of [01](01-audio-io-cpal.md), [02](02-event-ring-buffer.md), and [03](03-kick-voice.md)).

These four bullets are the gate. Nothing in Phase 0 exists except to satisfy them, and the layout fixed here (the 32-byte record, the `Engine` interface, the `Pattern`/`Hap` shape, the cps clock) is carried unchanged into every later phase.
