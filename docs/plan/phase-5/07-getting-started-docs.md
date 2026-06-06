# 05 — Getting-started & reference docs

## Purpose

The design docs under `docs/` ([architecture](../../architecture.md), [pattern-model](../../pattern-model.md), [event-protocol](../../event-protocol.md), [roadmap](../../roadmap.md)) explain how `strobe` is *built*. They are not what a person reaches for when they want to *write a song*. Phase 5 adds the missing **user-facing** layer: a getting-started path that takes someone from `npm i` to a kick playing in under five minutes, plus concise reference sheets for the three things they'll consult constantly — mini-notation, the chain API, and the combinators — and the practical guides (sampling, troubleshooting) that the design docs deliberately keep out of scope.

These docs are the on-ramp the [example songs](03-example-songs.md) and the [CLI](02-cli.md) assume. They never restate or contradict the canonical model; they *teach* it and link back to it. (The README quickstart is the trailhead; these guides are the trail.)

## Location

A new user-facing tree, separate from the existing design docs:

```
docs/
  guide/
    index.md             # NEW — landing page: what to read in what order
    install.md           # NEW — npm i, prerequisites, verifying it works
    first-song.md        # NEW — build hello-kick.ts line by line, hot-reload it
    mini-notation.md     # NEW — the mini-notation cheat sheet
    chain-api.md         # NEW — sound()/note()/freq() + chainable modifiers reference
    combinators.md       # NEW — the combinator reference (fast/every/jux/…)
    sampling.md          # NEW — load/chop/stretch + the YouTube legal note
    troubleshooting.md   # NEW — audio device, latency, install, hot-reload issues
```

`docs/guide/` sits alongside `docs/` design docs; the [README documentation table](../../../README.md#documentation) gains a "Guides" row pointing at `docs/guide/index.md`.

## Design

The doc set is deliberately small and task-oriented. Each page has one job. All API references are **generated-or-checked against the exported surface** where practical, so they cannot drift from [pattern-model.md](../../pattern-model.md).

### `index.md` — the map

A one-screen landing page: a "Start here → install → first song" path for newcomers, then a reference shelf (mini-notation, chain API, combinators) and how-to shelf (sampling, troubleshooting), with a link back to the [design docs](../../architecture.md) for readers who want to know *why*.

### `install.md` — get it running

- **Install:** `npm i strobe` (or `npm i -g strobe` for the global `strobe` CLI). The headline: **no Rust toolchain** — the engine ships as a [prebuilt per-OS binary](04-packaging-prebuilts.md). Lists the supported platforms (macOS x64/arm64, Linux x64/arm64, Windows x64).
- **Optional extras:** `yt-dlp` + `ffmpeg`, needed **only** for YouTube sampling, with per-OS one-line install hints and a pointer to the [legal note](#samplingmd--load-chop-stretch). Stated plainly: you don't need these to play or to use local samples.
- **Verify:** `strobe devices` (you should see your output) and `strobe play examples/hello-kick.ts` (you should hear a kick). If not → [troubleshooting](#troubleshootingmd--when-the-sound-is-wrong).

### `first-song.md` — from zero to a beat

A guided build of [`hello-kick.ts`](03-example-songs.md#hello-kickts--the-first-song), one concept at a time, ending with the hot-reload payoff:

1. Imports and `setcps(130/60/4)` — what a cycle is and how tempo maps to it (link to [architecture: the clock](../../architecture.md#the-clock-and-musical-time)).
2. `p1( sound("bd*4") )` — what a track is, what the mini-notation means.
3. `.gain(0.9)` — the chain API: attaching *how it sounds* (link to [pattern-model: chaining](../../pattern-model.md#the-chaining-api-where-it-feels-like-an-instrument)).
4. **The payoff:** run `strobe play`, edit `bd*4` → `bd(3,8)`, save, hear it swap at the next bar with no restart — the live-coding loop, which is the whole point ([architecture: tracks & hot-swapping](../../architecture.md#tracks-and-hot-swapping)).
5. "Where to go next": add a hi-hat (`p2`), then a bassline (`p3`) — graduating into [`techno-sidechain.ts`](03-example-songs.md#techno-sidechaints--the-pump).

### `mini-notation.md` — the cheat sheet

A single-page table of the mini-notation, lifted verbatim (same syntax, same meanings) from [pattern-model.md](../../pattern-model.md#mini-notation) so there is one source of truth, with one runnable example per row and a "putting it together" groove. Covers exactly: sequences, `~`, `*`/`/`, `[]`, `<>`, `,`, Euclid `(n,k)`, `!`, `?`, and the pitched forms `note`/`freq`.

```
"bd sn hh"          three events, splitting the cycle evenly
"bd ~ sn ~"         rests with ~
"bd*2 sn"           speed up a slot
"bd <sn cp>"        alternate one per cycle
"bd(3,8)"           Euclidean: 3 hits over 8
"[bd*4, hh*8]"      stack layers with ,
```

### `chain-api.md` — the modifier reference

The reference for `sound()`/`note()`/`freq()` and every chainable modifier, grouped by what it controls, each with a signature, range, and one-line example — and the key idea that **every parameter also accepts a pattern or continuous signal** ([pattern-model: parameters can be patterns](../../pattern-model.md#parameters-can-themselves-be-patterns)).

| Group | Methods (from the canonical API) |
| --- | --- |
| Source | `sound(s)`, `note(n)`, `freq(hz)`, oscillator selectors `.saw()` / `.sine()` / `.square()` / `.tri()` |
| Amplitude | `.gain(x)`, `.pan(x)`, `.adsr(a,d,s,r)` |
| Filter | `.lpf(x)`, `.hpf(x)`, `.resonance(x)` |
| Space/FX | `.room(x)` (reverb send), `.delay(...)`, `.sidechain("p1")` |
| Speed/pitch | `.speed(x)` |

Continuous signals usable as parameter values: `sine`, `cosine`, `saw`, `isaw`, `tri`, `square`, `rand`, `perlin`, each `0..1` with `.range(lo, hi)` — straight from [pattern-model.md](../../pattern-model.md#parameters-can-themselves-be-patterns). The page cites real snippets from [`techno-sidechain.ts`](03-example-songs.md#techno-sidechaints--the-pump) and [`filter-pad.ts`](03-example-songs.md#filter-padts--the-evolving-sweep).

### `combinators.md` — the transform reference

The combinator vocabulary, organized exactly as [pattern-model.md](../../pattern-model.md#combinators--transforming-patterns) groups them — Time, Structure & repetition, Layering, Density & chance — each as a function `Pattern → Pattern` available standalone and as a chain method, with a one-line example. It ends with the [pattern-model "evolving pattern"](../../pattern-model.md#combinators--transforming-patterns) example annotated line-by-line (`.every(4, rev).off(0.125, …).jux(rev)`), and a note that combinators are pure functions you can test with `queryArc` ([pattern-model: testing](../../pattern-model.md#testing-patterns)) — handy at the `strobe repl` prompt.

### `sampling.md` — load, chop, stretch (+ the YouTube legal note)

The how-to for the [Phase 4](../phase-4/00-overview.md) sampling API, with **local files as the default and safe path**:

- `sample("path.wav")` → load a local file; `.chop(n)` / `.slice(n, "...")` / `.stretch(x)` / `.pitch(semitones)` / `.loop()`, each with a runnable snippet (drawn from [`hiphop-chop.ts`](03-example-songs.md#hiphop-chopts--the-chopped-sample)).
- **The YouTube path is opt-in.** `sample.fromYouTube(url, { start, dur })` requires `yt-dlp` + `ffmpeg` on the PATH ([04: optional CLIs](04-packaging-prebuilts.md#optional-external-clis-yt-dlpffmpeg)); the doc shows the install hints and the lazy-check error you'll see if they're missing.
- **The legal note (prominent, not buried).** A clearly-boxed callout restating the [roadmap risk](../../roadmap.md#risks): downloading from YouTube can conflict with its Terms of Service and with copyright; keep it for short grabs, transform/clear material as you would in any production, and prefer local files and microphone input as the always-safe paths. This is the same wording the [README](../../../README.md) and roadmap use — never softened.

### `troubleshooting.md` — when the sound is wrong

A symptom → cause → fix table covering the failure modes the design already names ([architecture: what can go wrong](../../architecture.md#what-can-go-wrong-and-the-guardrails)):

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| No sound at all | Wrong output device | `strobe devices`, then `--device "<name>"` ([02](02-cli.md)) |
| Clicks / dropouts | Buffer too small for the machine | raise `--buffer` (e.g. 512), then `--lookahead 150` ([architecture](../../architecture.md#what-can-go-wrong-and-the-guardrails)) |
| Feels laggy when editing | Lookahead too long | lower `--lookahead` toward 100/80, watching for underruns |
| Underruns under load | Audio thread starved | larger buffer/lookahead; close other audio apps; check `stats()` line |
| `npm i` fails / no engine | Unsupported platform | build from source ([04: fallback](04-packaging-prebuilts.md#fallback-source-build-for-contributors)); supported list |
| `fromYouTube` errors | `yt-dlp`/`ffmpeg` missing | install them ([sampling](#samplingmd--load-chop-stretch)); or use a local file |
| Edit didn't change the music | Syntax error in the song | check the error printed by the runtime — last-good keeps playing ([01](01-hot-reload-runtime.md#edge-cases--risks)) |

Each row links to the relevant Phase 5 feature so the fix is one click away.

## Interfaces

- **To the [CLI](02-cli.md):** every command (`play`/`repl`/`render`/`devices`) and flag (`--buffer`/`--lookahead`/`--device`) the guides reference is the real CLI surface; troubleshooting fixes are CLI flags.
- **To the [example songs](03-example-songs.md):** `first-song.md` builds `hello-kick.ts`; the references cite `techno-sidechain.ts`/`filter-pad.ts`/`hiphop-chop.ts`. Examples and docs are kept in sync by construction.
- **To the [canonical design docs](../../pattern-model.md):** the mini-notation, chain-API, and combinator pages are the *same vocabulary* as [pattern-model.md](../../pattern-model.md), linked back, never divergent. The clock/tempo explanation links to [architecture.md](../../architecture.md#the-clock-and-musical-time).
- **To [packaging](04-packaging-prebuilts.md):** `install.md` states the "no Rust toolchain" promise and the optional-CLI contract; `troubleshooting.md` points unsupported platforms at the source-build fallback.
- **To the user's workflow:** these are the pages a user keeps open while live-coding — the cheat sheets answer "what's the syntax for X" without leaving the editor.

## Edge cases & risks

- **Docs drifting from the API.** The chain-API and combinator references list real exported symbols; a CI check (the same one used for [example fidelity](03-example-songs.md#acceptance-criteria)) cross-references the docs' symbol tables against the package's exports, failing the build if a method is documented that doesn't exist (or vice-versa for the core set).
- **The legal note must stay prominent.** The YouTube caveat is a required, non-collapsible callout in `sampling.md` (and echoed in `install.md`'s optional-extras section), worded identically to [roadmap.md](../../roadmap.md#risks) and the [README](../../../README.md). A lint check asserts the callout's presence so it can't be edited away.
- **Cross-platform instructions.** Install hints, device selection, and the `strobe` command shim differ per OS; each how-to gives macOS/Linux/Windows variants (matching the [packaging matrix](04-packaging-prebuilts.md#targets-the-build-matrix)).
- **Scope creep into design docs.** The guides deliberately do **not** re-explain the two-plane architecture or the event protocol; they link to them. This keeps one source of truth and avoids contradicting the LOCKED design.
- **Examples in docs must run.** All runnable snippets are extracted from the actual [example files](03-example-songs.md) or unit-tested via `queryArc`, so a copy-pasted doc snippet always works.

## Acceptance criteria

- [ ] **Five-minute on-ramp works.** A new user following `install.md` → `first-song.md` on a clean machine (no Rust toolchain) reaches an audible kick and a successful hot-reload edit. *How to test:* a scripted walkthrough on a fresh CI image per OS, asserting sound + a successful live edit.
- [ ] **Reference completeness.** `mini-notation.md` covers every row of the [canonical table](../../pattern-model.md#mini-notation); `combinators.md` covers every entry in the [canonical combinator tables](../../pattern-model.md#combinators--transforming-patterns); `chain-api.md` covers the documented modifiers and all continuous signals. *How to test:* the symbol cross-reference CI check.
- [ ] **No drift from canon.** Every API symbol named in the guides exists in the package exports and matches [pattern-model.md](../../pattern-model.md); no guide contradicts a design doc. *How to test:* the cross-reference check + a link-checker over all `docs/guide/*.md` relative links.
- [ ] **YouTube legal note present and prominent.** `sampling.md` and `install.md` carry the copyright/ToS callout, worded as in [roadmap.md](../../roadmap.md#risks). *How to test:* a lint assertion for the callout text.
- [ ] **Troubleshooting maps to real fixes.** Every fix references a real CLI flag/command or feature doc. *How to test:* link-checker + manual review that each `--flag` exists in [02](02-cli.md).
- [ ] **All snippets run.** Every runnable code block executes (via `strobe play`/`strobe repl`/`queryArc`) without error. *How to test:* a doc-snippet test harness that extracts fenced `ts` blocks marked runnable and evaluates them.
