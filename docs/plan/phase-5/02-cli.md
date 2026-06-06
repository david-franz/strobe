# 02 — The `tekno` CLI

## Purpose

The CLI is the front door. A new user should be able to write a song file and bring it to life with one command, with no boilerplate runtime script (the [Phase 0 `npx tsx -e "..."` incantation](../phase-0/07-hot-reload-demo.md#running-it) was fine for proving the spine, but no one wants to type it to perform). The CLI wraps the [hot-reload runtime](01-hot-reload-runtime.md) in four small, predictable commands:

- `tekno play song.ts` — load the song, start the engine, watch the file, play live (the headline command; this is what satisfies the roadmap "saving the file updates the music live with no restart").
- `tekno repl` — drop into an interactive prompt against a live engine.
- `tekno render song.ts out.wav` — render the song offline to a `.wav` file (a non-real-time "bounce").
- `tekno devices` — list available audio output devices (so the user can pick one when the default is wrong).

It also owns the cross-cutting concerns a CLI must get right: argument parsing, configuration (sample rate, buffer size, lookahead), and **graceful Ctrl-C** that stops the engine cleanly so the audio device is released and no dangling thread remains.

## Location

```
packages/tekno/
  bin/
    tekno.ts            # NEW — the CLI entry (#!/usr/bin/env node shebang after build)
  src/
    cli/
      index.ts          # NEW — arg parsing + command dispatch
      play.ts           # NEW — `play` command → runtime.play()
      repl.ts           # NEW — `repl` command → runtime.startRepl()
      render.ts         # NEW — `render` command → offline bounce
      devices.ts        # NEW — `devices` command → engine.listDevices()
      config.ts         # NEW — resolve config (flags → env → tekno.config → defaults)
  package.json          # adds "bin": { "tekno": "dist/bin/tekno.js" }
```

`bin/tekno.ts` compiles to `dist/bin/tekno.js`; `package.json`'s `"bin"` field maps the `tekno` command to it, so `npm i -g tekno` (or a local `npx tekno`) puts `tekno` on the path. The CLI uses **no heavy framework** — Node's built-in `util.parseArgs` is sufficient and keeps the dependency surface minimal.

## Design

### Command shapes & help text

```
$ tekno --help
tekno — write techno and hip-hop as code, live.

USAGE
  tekno play <song.ts> [options]     load, watch, and play a song live
  tekno repl [song.ts] [options]     interactive live-coding prompt
  tekno render <song.ts> <out.wav>   render a song offline to a WAV file
  tekno devices                      list available audio output devices
  tekno --version                    print version
  tekno --help                       this help

OPTIONS (play / repl / render)
  --sample-rate <hz>     audio sample rate            (default 48000)
  --buffer <frames>      audio buffer size            (default 256)
  --lookahead <ms>       scheduler look-ahead         (default 100)
  --device <name|id>     audio output device          (default: system default)
  --cycles <n>           render: how many cycles to bounce (render only)
  --bpm <n>              convenience: setcps(bpm/60/4) before loading the song
  -q, --quiet            suppress the telemetry status line

EXAMPLES
  tekno play examples/techno-sidechain.ts
  tekno play song.ts --buffer 512 --lookahead 150   # safer on a busy machine
  tekno render song.ts mix.wav --cycles 16
  tekno devices
```

### Arg parsing & dispatch (in `cli/index.ts`)

```ts
import { parseArgs } from "node:util";

const COMMANDS = ["play", "repl", "render", "devices"] as const;
type Command = (typeof COMMANDS)[number];

export async function main(argv = process.argv.slice(2)) {
  if (argv[0] === "--version") return printVersion();
  if (!argv[0] || argv[0] === "--help") return printHelp();

  const cmd = argv[0] as Command;
  if (!COMMANDS.includes(cmd)) {
    process.stderr.write(`unknown command: ${cmd}\n`);
    printHelp();
    process.exitCode = 2;
    return;
  }

  const { values, positionals } = parseArgs({
    args: argv.slice(1),
    allowPositionals: true,
    options: {
      "sample-rate": { type: "string" },
      buffer:        { type: "string" },
      lookahead:     { type: "string" },
      device:        { type: "string" },
      cycles:        { type: "string" },
      bpm:           { type: "string" },
      quiet:         { type: "boolean", short: "q" },
    },
  });

  const config = resolveConfig(values);   // see config.ts below
  switch (cmd) {
    case "play":    return runPlay(positionals[0], config);
    case "repl":    return runRepl(positionals[0], config);
    case "render":  return runRender(positionals[0], positionals[1], config);
    case "devices": return runDevices();
  }
}
```

### Configuration resolution (in `cli/config.ts`)

Precedence, highest first: **CLI flags → environment variables → an optional `tekno.config.{ts,json}` in the cwd → built-in defaults**. The defaults match [architecture.md](../../architecture.md#the-look-ahead-scheduler-two-clocks): 48 kHz, 256-frame buffer, ~100 ms lookahead.

```ts
export interface Config {
  sampleRate: number;   // default 48000
  bufferFrames: number; // default 256
  lookaheadMs: number;  // default 100
  device?: string;      // default: system default device
  cycles?: number;      // render only
  bpm?: number;
  quiet: boolean;
}

export function resolveConfig(flags: Record<string, unknown>): Config {
  const fileCfg = loadConfigFile();   // optional tekno.config.* (may be absent)
  const num = (v: unknown, fallback: number) =>
    v != null ? Number(v) : fallback;
  return {
    sampleRate:   num(flags["sample-rate"] ?? process.env.TEKNO_SAMPLE_RATE ?? fileCfg?.sampleRate, 48000),
    bufferFrames: num(flags.buffer        ?? process.env.TEKNO_BUFFER       ?? fileCfg?.bufferFrames, 256),
    lookaheadMs:  num(flags.lookahead     ?? process.env.TEKNO_LOOKAHEAD    ?? fileCfg?.lookaheadMs, 100),
    device:       (flags.device as string) ?? process.env.TEKNO_DEVICE ?? fileCfg?.device,
    cycles:       flags.cycles != null ? Number(flags.cycles) : undefined,
    bpm:          flags.bpm != null ? Number(flags.bpm) : undefined,
    quiet:        Boolean(flags.quiet),
  };
}
```

### `play` (in `cli/play.ts`)

Thin wrapper over [`runtime.play()`](01-hot-reload-runtime.md#the-watch--eval-loop-in-runtimets). It resolves the song path, applies `--bpm` sugar before the first load if given, wires the telemetry line (unless `--quiet`), and installs the Ctrl-C handler.

```ts
export async function runPlay(songPath: string | undefined, cfg: Config) {
  if (!songPath) return fail("play requires a <song.ts> path");
  const onStats = cfg.quiet ? undefined : printStatsLine;   // frame/underruns/late/ring
  const session = await play(resolve(songPath), {
    sampleRate: cfg.sampleRate,
    bufferFrames: cfg.bufferFrames,
    lookaheadMs: cfg.lookaheadMs,
    device: cfg.device,
    bpm: cfg.bpm,
    onStats,
  });
  installGracefulShutdown(session);     // see below
  process.stdout.write(`▶ playing ${songPath} — edit & save to change it live. Ctrl-C to stop.\n`);
}
```

The telemetry line is the same `frame=… underruns=… late=… ring=…` line from [Phase 0 / 07](../phase-0/07-hot-reload-demo.md#the-runtime-boot-watch-swap), so the verification recipe is visible while you perform.

### `repl` (in `cli/repl.ts`)

Boots the engine/clock/scheduler exactly as `play` does (optionally loading a starter song first if a path is given), then hands control to [`runtime.startRepl()`](01-hot-reload-runtime.md#the-repl-in-runtimereplts). If a song path is passed, the REPL *also* watches it, so you can mix file-editing and live typing in one session.

### `render` — offline bounce (in `cli/render.ts`)

`render` does **not** open the `cpal` stream. Instead it drives the engine in an **offline / non-real-time** mode: the same Rust render loop runs as fast as the CPU allows, the scheduler queries the song's patterns cycle-by-cycle for a fixed number of cycles (`--cycles`, default inferred from the song or 8), and the rendered frames are written to a WAV file. This reuses the entire control plane — patterns, combinators, effects — with only the audio sink swapped.

```ts
export async function runRender(songPath: string, outPath: string, cfg: Config) {
  if (!songPath || !outPath) return fail("render requires <song.ts> <out.wav>");
  const engine = createNativeEngine();
  await engine.startOffline({ sampleRate: cfg.sampleRate });   // no cpal stream
  const clock = new Clock(engine); clock.anchor(cfg.sampleRate); setActiveClock(clock);
  const scheduler = new Scheduler(engine, clock, { lookaheadMs: cfg.lookaheadMs });

  await evalSong(resolve(songPath)).then((r) => applyInitial(clock, r.assignments));

  const cycles = cfg.cycles ?? inferCycles(songPath) ?? 8;
  const pcm = await renderCycles(scheduler, engine, clock, cycles);  // Float32, interleaved
  await writeWav(resolve(outPath), pcm, cfg.sampleRate, 2 /* stereo */);
  process.stdout.write(`✓ rendered ${cycles} cycles → ${outPath}\n`);
}
```

> `startOffline` is a small addition to the [napi `Engine` interface](../phase-0/04-napi-bridge.md): it allocates the same voice/FX graph but renders into a returned buffer instead of `cpal`. Because the audio path is identical, an offline bounce sounds exactly like the live output. WAV writing is a tiny pure-TS encoder (no dependency).

### `devices` (in `cli/devices.ts`)

Lists output devices by querying `cpal` through a new control call on the engine handle.

```ts
export async function runDevices() {
  const devices = await listDevices();   // [{ id, name, default, sampleRates }]
  for (const d of devices)
    process.stdout.write(`${d.default ? "* " : "  "}${d.id}  ${d.name}  (${d.sampleRates.join("/")} Hz)\n`);
  process.stdout.write(`\nUse: tekno play song.ts --device "<name|id>"\n`);
}
```

`listDevices()` is a non-real-time control call ([event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)) added to the engine handle; it enumerates `cpal` output devices and the default one (marked `*`).

### Graceful Ctrl-C

The single non-negotiable shutdown rule: stop the engine so `cpal` releases the device and the audio thread joins — otherwise the terminal can be left with a stuck stream.

```ts
function installGracefulShutdown(session: { stop: () => Promise<void> }) {
  let shuttingDown = false;
  const onSignal = async () => {
    if (shuttingDown) process.exit(130);   // second Ctrl-C → hard exit
    shuttingDown = true;
    process.stdout.write("\n⏹ stopping…\n");
    try { await session.stop(); } finally { process.exit(0); }
  };
  process.on("SIGINT", onSignal);
  process.on("SIGTERM", onSignal);
}
```

A second Ctrl-C forces exit (in case `stop()` ever hangs on a misbehaving device).

## Interfaces

- **To the [hot-reload runtime](01-hot-reload-runtime.md):** `play` → `runtime.play()`, `repl` → `runtime.startRepl()`. The CLI owns no audio logic; it only parses, configures, and delegates.
- **To the [napi engine](../phase-0/04-napi-bridge.md):** `render` adds `startOffline()`; `devices` adds `listDevices()` — both non-real-time control calls. `play`/`repl` use the standard `start`/`stop`/`stats`.
- **To the [scheduler](../phase-0/05-lookahead-scheduler.md) & [clock](../phase-0/06-clock-and-tracks.md):** passes through `lookaheadMs`, `sampleRate`, `bufferFrames`, and the `--bpm`→`setcps` sugar.
- **To the user's workflow:** one command to play; Ctrl-C to stop; flags for the few things (buffer/lookahead/device) that differ across machines. The [example songs](03-example-songs.md) are written to run verbatim under `tekno play`.

## Edge cases & risks

- **Missing / unreadable song path.** `play`/`render` fail fast with a clear message and a non-zero exit code; they never start the engine for a path that does not exist.
- **Broken song on first load.** Unlike a *live* edit, a song that throws on the *initial* `play` has no last-good to fall back to. The CLI prints the error and exits non-zero rather than starting silent audio — but the engine is stopped first so no device is left open.
- **Bad config values.** A non-numeric `--buffer`/`--sample-rate`, or a sample rate the device cannot provide, is reported by the engine on `start()`; the CLI surfaces it and suggests `tekno devices`. Out-of-range lookahead (too short) is warned per [architecture.md](../../architecture.md#what-can-go-wrong-and-the-guardrails).
- **Ctrl-C during a reload.** The `reloading` guard in the runtime and the `shuttingDown` guard here are independent; a Ctrl-C mid-reload still stops cleanly because `stop()` halts the audio thread regardless of the control-plane state.
- **`render` runtime vs. real time.** Offline render is decoupled from wall-clock, so a long `--cycles` bounce just takes CPU time; it cannot underrun (there is no deadline). Progress is printed periodically so a long render is not mistaken for a hang.
- **Cross-platform device names.** Device identifiers differ across OSes (CoreAudio vs. ALSA/PulseAudio/JACK vs. WASAPI). `--device` accepts either the human name or the index from `tekno devices`, so users are never forced to guess an opaque id. Validated on all three OSes in CI.
- **Shebang / line endings on Windows.** The published `bin` is the built JS with a `#!/usr/bin/env node` shebang; npm generates the `.cmd` shim on Windows, so `tekno` works in PowerShell/cmd without the user touching shebangs.

## Acceptance criteria

- [ ] **`tekno play song.ts` plays and hot-reloads.** Running it on an [example song](03-example-songs.md) produces sound; editing and saving the file swaps the changed tracks live with no restart and clean `stats()`. *How to test:* the [runtime acceptance harness](01-hot-reload-runtime.md#acceptance-criteria) invoked through the CLI surface.
- [ ] **Config flags take effect.** `--buffer 512 --lookahead 150 --sample-rate 44100` are observably passed to `engine.start()`/scheduler. *How to test:* unit test `resolveConfig` precedence (flag > env > file > default) and a spy asserting the values reach `start()`.
- [ ] **Graceful Ctrl-C.** Sending SIGINT stops the engine (one `stop()` call) and releases the device; a second SIGINT hard-exits. *How to test:* spawn the CLI as a child process, send SIGINT, assert clean exit code and that the device is free.
- [ ] **`tekno render song.ts out.wav` bounces offline.** Produces a valid stereo WAV of the requested cycle count whose content matches the live output. *How to test:* render a known song; assert WAV header + that the PCM equals an offline reference render bit-for-bit (deterministic patterns) within tolerance for any random combinators.
- [ ] **`tekno devices` lists devices.** Prints at least the default output device, marked `*`. *How to test:* assert non-empty list on each CI OS; assert `--device` from the list is accepted by `play`.
- [ ] **`--help` / `--version` / unknown command.** Help and version print and exit 0; an unknown command prints help and exits 2. *How to test:* snapshot the help text; assert exit codes.
- [ ] **Cross-platform.** All commands pass on macOS, Linux, and Windows in CI.
