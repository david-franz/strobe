# Implementation plan

This is the concrete, build-it-from-here plan for `strobe`. Each phase has one document **per feature**, detailed enough to implement without guessing. For the high-level rationale and acceptance gates, see [../roadmap.md](../roadmap.md); for the foundational design, see [../architecture.md](../architecture.md), [../pattern-model.md](../pattern-model.md), and [../event-protocol.md](../event-protocol.md).

**Build order is spine-first, then breadth.** Phase 0 proves the entire architecture end-to-end with the smallest possible system. Every later phase widens one layer of that proven spine. Do not start a phase until the prior phase's "done when" criteria pass.

Every feature doc follows the same shape: **Purpose · Location · Design · Interfaces · Edge cases & risks · Acceptance criteria.**

---

## [Phase 0 — Proof of spine](phase-0/00-overview.md)
Prove "TS drives a Rust audio engine, live, with zero glitches" with the minimum viable system.

- [01 · Audio I/O (cpal)](phase-0/01-audio-io-cpal.md) — output stream, render loop, the frame counter
- [02 · Event ring buffer](phase-0/02-event-ring-buffer.md) — lock-free SPSC, 32-byte records, the audio-thread consumer
- [03 · Kick voice](phase-0/03-kick-voice.md) — a minimal synthesized kick as a fundsp graph
- [04 · napi-rs bridge](phase-0/04-napi-bridge.md) — `start`/`stop`/`pushEvents`/`stats`
- [05 · Look-ahead scheduler](phase-0/05-lookahead-scheduler.md) — the "two clocks" tick + event encoding
- [06 · Clock & tracks](phase-0/06-clock-and-tracks.md) — cps clock, the `p1` slot, the minimal pattern
- [07 · Hot-reload demo](phase-0/07-hot-reload-demo.md) — the file-watch runtime + end-to-end verification

## [Phase 1 — Synthesis core](phase-1/00-overview.md)
Make the engine genuinely musical: the "base frequency" synth and the "base drum click."

- [01 · Oscillators](phase-1/01-oscillators.md) — sine/saw/square/tri/noise/wavetable, band-limited
- [02 · Filters](phase-1/02-filters.md) — state-variable LP/HP/BP/notch + resonance
- [03 · Envelopes](phase-1/03-envelopes.md) — ADSR (amp + filter), gated and one-shot
- [04 · Synth voice](phase-1/04-synth-voice.md) — the full osc→filter→env→amp chain + song API
- [05 · Drum synthesis](phase-1/05-drum-synthesis.md) — kick/snare/clap/hats
- [06 · Voice allocation](phase-1/06-voice-allocation.md) — polyphony + voice stealing
- [07 · Master bus & limiter](phase-1/07-master-bus-limiter.md) — summing + transparent limiting

## [Phase 2 — Pattern language](phase-2/00-overview.md)
Implement the full declarative language from [../pattern-model.md](../pattern-model.md), in pure TypeScript.

- [01 · Pattern core](phase-2/01-pattern-core.md) — `Pattern`/`Hap`/`TimeSpan`, `queryArc`, constructors
- [02 · Mini-notation parser](phase-2/02-mini-notation-parser.md) — the `"bd ~ sn*2"` DSL
- [03 · Combinators](phase-2/03-combinators.md) — `fast`/`every`/`jux`/`degrade`/…
- [04 · sound/note API](phase-2/04-sound-note-api.md) — the chainable modifier API
- [05 · Continuous signals](phase-2/05-continuous-signals.md) — `sine`/`saw`/`rand` as parameters
- [06 · Scheduler integration](phase-2/06-scheduler-integration.md) — `Hap` → engine events
- [07 · Tracks & hot-swap](phase-2/07-tracks-hot-swap.md) — `p1..pN`, seamless boundary swaps

## [Phase 3 — Effects rack](phase-3/00-overview.md)
Comprehensive, production-grade effects, per-track and as sends.

- [01 · Effects framework](phase-3/01-effects-framework.md) — chains, send buses, the `Effect` trait
- [02 · Filters & EQ](phase-3/02-filters-eq.md) — performance filters + multiband EQ
- [03 · Delay & reverb](phase-3/03-delay-reverb.md) — incl. ping-pong + tempo-sync
- [04 · Distortion & bitcrush](phase-3/04-distortion-bitcrush.md) — saturation, waveshaping, decimation
- [05 · Modulation FX](phase-3/05-modulation-fx.md) — chorus/flanger/phaser
- [06 · Dynamics & sidechain](phase-3/06-dynamics-sidechain.md) — compressor + the techno pump
- [07 · Parameter automation](phase-3/07-parameter-automation.md) — signals → moving params

## [Phase 4 — Sampling](phase-4/00-overview.md)
Load, chop, stretch, and pitch audio — including short YouTube grabs. The heart of hip-hop.

- [01 · Decode & loading](phase-4/01-sample-decode-loading.md) — symphonia → engine buffers
- [02 · Sampler voice](phase-4/02-sampler-voice.md) — playback, pitch, interpolation
- [03 · Time-stretch & pitch-shift](phase-4/03-timestretch-pitchshift.md) — ssstretch, decoupled
- [04 · Chop/slice/granular](phase-4/04-chop-slice-granular.md) — `.chop(8)`, slicing, grains
- [05 · YouTube ingest](phase-4/05-youtube-ingest.md) — `yt-dlp`+`ffmpeg`, caching, the legal note

## [Phase 5 — Live-coding DX](phase-5/00-overview.md)
Make it pleasant to actually perform and compose.

- [01 · Hot-reload runtime](phase-5/01-hot-reload-runtime.md) — watch + re-eval only changed tracks
- [02 · CLI](phase-5/02-cli.md) — `strobe play` / `repl` / `render` / `bounce` / `devices`
- [03 · Example songs](phase-5/03-example-songs.md) — runnable techno/hip-hop/pad songs
- [04 · Packaging & prebuilts](phase-5/04-packaging-prebuilts.md) — `npm i`, no Rust toolchain
- [05 · Session recording](phase-5/05-session-recording.md) — capture a whole performance, re-render to stems
- [06 · Audio capture & stems](phase-5/06-audio-capture-stems.md) — record master + per-track stems live
- [07 · Getting-started docs](phase-5/07-getting-started-docs.md) — the user guide set (written last)

## [Phase 6 — Web backend](phase-6/00-overview.md) *(later)*
Run the same engine and the same song code in a browser — near-zero cost for the future frontend.

- [01 · WASM compilation](phase-6/01-wasm-compilation.md) — the engine core to WASM
- [02 · AudioWorklet host](phase-6/02-audioworklet-host.md) — the browser audio thread
- [03 · SharedArrayBuffer ring](phase-6/03-sharedarraybuffer-ring.md) — the event ring in the browser
- [04 · Web engine impl](phase-6/04-web-engine-impl.md) — the same `Engine` interface, song code unchanged

---

## Conventions that hold across every phase

- **No allocation, locks, or JS on the audio thread.** Everything real-time-critical is pre-allocated; config changes cross the bridge as double-buffered swaps.
- **The 32-byte event record is fixed from Phase 0.** New behavior takes new opcodes or new record versions; existing fields are never repurposed. See [../event-protocol.md](../event-protocol.md).
- **The engine sits behind one small `Engine` interface** (`start`/`stop`/`loadSample`/`pushEvents`/`stats`). The native (napi-rs) and web (WASM/AudioWorklet) backends both satisfy it, so song code never changes between them.
- **Patterns are pure functions of time.** This is what makes live hot-swapping seamless and testable.
- **Each phase ships a runnable example, clean `stats()` (no underruns), unit tests for pure-TS logic, and macOS-first validation with Linux/Windows confirmed in CI** before it is called done.
