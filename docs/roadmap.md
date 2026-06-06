# Roadmap

The build order is **spine first, then breadth**. Phase 0 proves the whole architecture end-to-end with the smallest possible system; every later phase widens one layer of that proven spine. Each phase ships a runnable example and has explicit "done" criteria.

## Phase 0 — Proof of spine *(current)*

**Goal:** prove the architecture is sound before building any breadth. The riskiest claim in the whole design is "TS can drive a Rust audio engine, live, with no glitches." Phase 0 demonstrates exactly that and nothing more.

**Scope**
- `engine/`: a `cpal` output stream; a minimal render loop; one synthesized kick voice; the SPSC ring buffer + the `frame`-stamped event consumer.
- `bindings/native/`: a `napi-rs` addon exposing `start()`, `stop()`, `pushEvents()`, `stats()`.
- `packages/strobe/`: the look-ahead scheduler, a cps clock, one track slot, and a trivial "play this every 1/4 cycle" pattern.
- `examples/phase0-kick.ts`: a four-on-the-floor kick read from a file the runtime watches.

**Done when**
- Running the example produces a steady, on-time kick on macOS.
- Editing the pattern in the watched file (e.g. `bd*4` → `bd*8` or a Euclid `bd(3,8)`) changes the loop **at the next cycle boundary** with **no clicks or dropouts**.
- `stats()` reports zero underruns and zero late events under normal load.
- The audio thread provably does no allocation/locking/JS calls (code review + a stress check).

## Phase 1 — Synthesis core

**Goal:** make the engine genuinely musical — the "base frequency" and "base drum click" the project is built around.

**Scope**
- `voices/synth.rs`: oscillator (sine/saw/square/tri/noise/wavetable) → state-variable filter (LP/HP/BP/notch + resonance) → ADSR amp & filter envelopes. Built as `fundsp` graphs.
- `voices/drum.rs`: synthesized kick (pitched sine + fast pitch/amp envelope), snare/clap (noise + tuned bandpass), hats (filtered noise). All parameters live-settable.
- `voice_alloc.rs`: polyphony with voice stealing (oldest/quietest first).
- Master bus + a transparent limiter to prevent clipping.

**Done when**
- `note("c2 e2 g2").saw().lpf(...)` plays correct pitches with audible filter/envelope shaping.
- A synthesized drum kit (bd/sn/hh/cp) sounds usable for a real beat.
- 16+ simultaneous voices play cleanly with no underruns.

## Phase 2 — Pattern language

**Goal:** the full declarative language from [pattern-model.md](pattern-model.md).

**Scope**
- `pattern.ts`: `Pattern`/`Hap`/`TimeSpan` core and `queryArc`.
- `mini.ts`: the mini-notation parser (sequences, `~`, `*` `/`, `[]`, `<>`, `,`, Euclid `(n,k)`, `!`, `?`).
- `combinators.ts`: time (`fast`/`slow`/`rev`/`hurry`/`off`), structure (`every`/`chunk`/`iter`), layering (`stack`/`cat`/`superimpose`/`jux`), chance (`degrade`/`sometimes`).
- `sound.ts`/`instrument.ts`: `sound()/note()/freq()` + the chainable modifier API; continuous signals (`sine`, `saw`, `rand`, …) usable as parameters.
- Scheduler upgraded to query arbitrary patterns across `p1..pN` and compile `Hap`s → events.

**Done when**
- The README quickstart song plays as written.
- Combinators and the parser have deterministic unit tests (query a span, assert `Hap`s).
- Parameters accept patterns/signals (e.g. `.lpf(sine.range(300,1200).slow(8))` audibly sweeps).

## Phase 3 — Effects rack

**Goal:** comprehensive, production-grade effects, per-track and as sends.

**Scope**
- `effects.rs`: filter, delay (incl. ping-pong), reverb, distortion/saturation, bitcrush, chorus/flanger/phaser, compressor, EQ, **sidechain** (the techno pump), pan, gain.
- Per-track effect chains + global send buses (e.g. a shared reverb).
- Any effect parameter is automatable by a control pattern/signal.

**Done when**
- A techno loop with a sidechained bass audibly "pumps" against the kick.
- Sends work (multiple tracks into one reverb).
- Sweeping an effect param via a signal is smooth and click-free.

## Phase 4 — Sampling

**Goal:** load, chop, stretch, and pitch audio — including short YouTube grabs. Equally the heart of hip-hop.

**Scope**
- `symphonia` decode → `loadSample()` into engine buffers.
- `voices/sampler.rs`: buffer playback with pitch/speed; `ssstretch` time-stretch + pitch-shift (decoupled); slice/granular playback.
- `sample/youtube.ts`: `yt-dlp` + `ffmpeg` ingest (trim to N seconds, decode to PCM), with local caching by URL+range.
- `sample/edit.ts`: `.chop(n)`, `.slice(n)`, `.stretch(x)`, `.pitch(semitones)`, `.loop()`.

**Done when**
- A local sample can be chopped across a pattern (`sample("amen").chop(8).rev()` style).
- Time-stretch holds pitch while changing tempo with acceptable artifacts.
- `sample.fromYouTube(url, {start, dur})` produces a usable, cached, playable buffer.

## Phase 5 — Live-coding DX

**Goal:** make it pleasant to actually perform/compose with.

**Scope**
- `runtime.ts`: a REPL and a file-watcher that hot-reloads a song file, re-evaluating only changed tracks and swapping them at cycle boundaries.
- A small CLI (`strobe play song.ts`, `strobe repl`, `strobe render`, `strobe bounce`, `strobe devices`).
- **Recording:** a deterministic **session** capture (tee the realized event stream + instruments + sample buffers to a `.strobesession` bundle, re-renderable offline) and real-time **audio capture** of the master mix plus **per-track stems** (lock-free taps → writer thread, no audio-thread I/O). Re-render and live capture share one set of master/stem sinks; recording adds two control calls (`startCapture`/`stopCapture`) but no event opcodes.
- Example songs (techno loop with sidechain; hip-hop beat with a chopped sample; evolving filter-sweep pad).
- Getting-started docs (written last, once the features exist); bootstrap that ships a prebuilt engine binary and checks for `yt-dlp`/`ffmpeg`.

**Done when**
- `strobe play song.ts` runs; saving the file updates the music live with no restart.
- A new user can install with `npm i`, no Rust toolchain required (prebuilt addon).
- `strobe play song.ts --rec takes/ --stems` captures a live session to `master.wav` + per-track stems with no underruns; a `--session` recording re-renders deterministically via `strobe bounce` to byte-identical master + stems.

## Phase 6 — Web backend *(later)*

**Goal:** run the same engine and the same song code in a browser, unlocking the future click-to-code frontend at near-zero language cost.

**Scope**
- Compile `engine/` to WASM via `wasm-bindgen`; run it inside an AudioWorklet.
- A `SharedArrayBuffer`-backed event ring shared with the worklet.
- A web implementation of the same `Engine` interface the native addon satisfies.

**Done when**
- The README quickstart song plays unchanged in a browser tab.
- Editing patterns live in the browser hot-swaps with no dropout.

---

## Risks

- **YouTube sampling & copyright.** `yt-dlp` works technically, but downloading can conflict with YouTube's Terms of Service and with copyright. We keep it **opt-in** and clearly documented, never the headline feature; local files and microphone input are the always-safe paths. Treat sampled material as you would in any production: clear or transform it appropriately.
- **Time-stretch quality** is the hardest DSP problem here. `ssstretch` (Signalsmith) is strong, but kept behind a swappable interface so we can change algorithms without touching song code.
- **Native build toolchain.** End users should not need Rust. We ship **prebuilt per-OS binaries** (napi-rs supports this) so install is `npm i`; the Rust toolchain is a contributor concern only.
- **Latency vs. stability.** Lower buffer sizes and shorter scheduler lookahead feel more live but risk underruns. Both are configurable, with safe defaults (~100 ms lookahead).
- **Cross-platform parity.** Audio device behavior differs across OSes. Each phase is validated on macOS first, then Linux and Windows via CI before being called done.

## Cross-cutting definition of done (every phase)

- A runnable example in `examples/` exercises the new capability.
- No audio underruns under the example's normal load (`stats()` clean).
- Pure-TS logic (patterns, parser, scheduler math) has unit tests; DSP has golden/null-test checks where practical.
- Validated on macOS; Linux/Windows confirmed in CI before sign-off.
