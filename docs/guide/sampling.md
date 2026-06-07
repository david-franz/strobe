[« strobe user guide](../guide.md)

# Sampling (Phase 4)

Loading a sound, chopping it into a pattern, stretching and pitching it — this is the heart of
hip-hop production, and the whole Phase 4 layer. The big idea: **a sample is just another voice.**
Once loaded, a sample triggers like a drum hit, carries the same control map, and rides the same
combinators and effects as everything else — `sample("amen").chop(8).rev().jux(x => x.speed(1.5))`
is an ordinary pattern.

Under the hood, the heavy work (decode, resample, time-stretch) happens **offline on the control
thread**; the audio thread only ever reads pre-loaded buffers. So nothing here can glitch the engine.

## Loading

| Function | Loads |
| --- | --- |
| `sample("path/to.wav")` | a local file → a one-shot trigger pattern (loads on first use) |
| `loadFile(path, name?)` | a local file → a `SampleHandle` (the lower-level call) |
| `loadPcm(float32, channels, sampleRate, opts?)` | already-decoded interleaved PCM → a `SampleHandle` |
| `sample.fromYouTube(url, { start, dur })` | a short clip from a URL (**opt-in** — see below) |

`sample(...)` is what you use most: pass a path (or a `SampleHandle`) and you get a pattern that
plays the buffer once per cycle, ready to chain. Decoding (WAV/MP3/FLAC/OGG/…) and resampling to
the engine rate happen in Rust, off the audio thread.

```ts
import { sample, p1 } from "strobe";

p1( sample("breaks/amen.wav") );          // play the whole break once per cycle
p1( sample("breaks/amen.wav").gain(0.8).room(0.2) );   // …with the usual modifiers + effects
```

**Loading is idempotent.** Re-running a song line during live coding never re-decodes the same
file — the handle is cached per source — so saves stay instant. Load samples *after* the runtime
has started (in a song run under `pnpm demo:phase4`, the engine is already live when your code runs).

## Speed (varispeed)

| Method | Effect |
| --- | --- |
| `.speed(x)` | playback rate **and** pitch together (tape/varispeed: `2` = octave up, half length) |

`.speed` is the classic coupled control — faster *is* higher, like pitching a turntable. For
**independent** pitch and time, use `.stretch`/`.pitch` below.

## Regions, chop & slice

A slice is just a trigger with a **start offset** and a **region length** — chop and slice compute
those per hap and ride the existing one-shot voice (no new machinery).

| Method | Effect |
| --- | --- |
| `.begin(f)` / `.end(f)` | play only the region `[f, …)` / `[…, f)` of the buffer (`f` is a `0..1` fraction) |
| `.chop(n)` | split into `n` equal slices laid out **in order** across the cycle |
| `.slice(n, "0 3 1 2")` | split into `n` slices; a mini-notation index pattern picks the **order** |
| `.loop()` | loop the region instead of playing it once (held beds, stutters) |
| `.rev()` | reverse — for a sample, both the **slice order** *and* each slice's **audio** |

```ts
p1( sample("amen").chop(8) );                 // the break's 8 eighths, in order (the classic chop)
p1( sample("amen").slice(8, "0 0 3 5 2 7") );  // re-sequence it into a new groove
p1( sample("amen").chop(8).rev() );            // play the whole break backwards, slice-exact
p1( sample("amen").chop(16).slice(16, "0 0 8 8").loop() );  // a stutter
```

`chop(n)` is the special case of `slice(n, "0 1 … n-1")`. Adjacent chop slices **share an exact
boundary frame**, so chopping a break and replaying it in order reconstructs it sample-exact (no
gap or click). Because the result is an ordinary pattern, `.fast(2)`, `.every(4, rev)`,
`.jux(x => x.speed(1.5))` all just work.

> **`.rev()` on a sample does both.** It reverses the order of the slices *and* sets each slice to
> play backwards — the producer-expected "reverse the break." On a non-sample pattern (synth
> notes, numbers) `.rev()` is the usual order-only reverse.

## Decoupled stretch & pitch

These change time and pitch **independently** (offline WSOLA), unlike `.speed`:

| Method | Effect |
| --- | --- |
| `.stretch(factor)` | change duration at **constant pitch** (`2` = twice as long / half speed) |
| `.pitch(semitones)` | transpose at **constant duration** (`-5` = down a fourth, same length) |

```ts
p1( sample("amen").stretch(2) );          // half-time, same pitch
p1( sample("vocal").pitch(-3) );          // pitched down a minor third, same length
p1( sample("amen").stretch(2).chop(8) );  // stretch THEN chop — slices land on the new length
```

Each `.stretch`/`.pitch` prepares a new buffer in the engine once (cached per `(sample, factor,
semitones)`), so live-coding reloads reuse it rather than re-running the stretch. Order matters:
**stretch/pitch first, then chop/slice** so the slice boundaries land on the final timeline.

## Granular

`.grain(...)` plays a **cloud of short overlapping grains** — textures, pads, and a cheap
pseudo-stretch (hold a moment of the sample and spray grains over it). It's an engine-side voice:
one trigger starts a whole cloud, so the event stream stays light.

| Option | Meaning |
| --- | --- |
| `size` | grain length in **milliseconds** (e.g. `60`) |
| `density` | grains per second (overlap ≈ `size/1000 × density`) |
| `scatter` | `0..1` random jitter of each grain's read position |
| `spread` | `0..1` stereo pan scatter |

```ts
p4( sample("pad").begin(0.5).grain({ size: 70, density: 24, scatter: 0.2, spread: 0.7 })
      .gain(0.35).lpf(1600).room(0.4) );   // a shimmering granular bed around the middle of the buffer
```

`.begin(f)` sets where the cloud hovers; `density`/`size` control its thickness. Over-dense
requests are capped to a fixed grain pool (a friendly limit, never a glitch).

## YouTube ingest (opt-in)

> **Legal / ToS / copyright — read first.** Downloading audio from YouTube can conflict with
> YouTube's Terms of Service and with copyright. This path is **opt-in and never the headline
> feature**. **Local files (`sample("path.wav")`) and microphone input are the always-safe,
> recommended ways to sample.** Treat grabbed material as you would in any production — short,
> transformed, cleared.

`sample.fromYouTube(url, { start, dur })` shells out to **`yt-dlp`** (fetch) and **`ffmpeg`** (trim
to `dur` seconds from `start`, decode to the engine rate), caches the result on disk by
`(url, start, dur, rate)`, and routes it through the same load path as a local file. It returns a
`SampleHandle` that flows straight into the whole edit chain.

```ts
import { enableYouTubeSampling, sample, p1 } from "strobe";

enableYouTubeSampling({ iUnderstandToS: true });   // required once; acknowledges the posture above
const vox = await sample.fromYouTube("https://youtu.be/…", { start: 42, dur: 4 });
p1( vox.chop(8).rev().room(0.2) );
```

- **Opt-in is enforced.** Without `enableYouTubeSampling({ iUnderstandToS: true })`, `fromYouTube`
  throws and does nothing on the network.
- **Tools are checked first.** If `yt-dlp`/`ffmpeg` aren't on your `PATH`, you get a clear error
  with an install hint (`brew install yt-dlp ffmpeg` on macOS) — *before* any fetch — plus a pointer
  to the no-extra-tools local-file path.
- **Short grabs.** `dur` is hard-capped (the posture is a few seconds of a record).
- **Cached + safe.** A repeat grab is served from disk (no network); URLs are passed as array args
  (never a shell string, so a hostile URL can't inject); temp downloads are always cleaned up; and
  it's all async, so the scheduler and audio thread never block while a grab is in flight (the
  pattern simply stays silent until the handle resolves).

---

Prev: [« Effects](effects.md) · Next: [Tracks & live control »](tracks.md)
