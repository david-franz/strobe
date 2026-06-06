# Phase 2.04 — Sound / note API (`sound.ts`, `instrument.ts`)

## Purpose

Mini-notation gives you *what and when*; this API gives you *how it sounds* ([pattern-model.md](../../pattern-model.md#the-chaining-api-where-it-feels-like-an-instrument)). `sound()`, `note()`, and `freq()` are the entry points that turn a mini-notation string into a pattern whose `Hap.value` is a **control map** — `{ s: "bd", lpf: 800, gain: 0.9 }` — and the chainable modifier methods (`.gain`, `.lpf`, `.resonance`, `.adsr`, `.room`, `.pan`, `.speed`, …) build that map up one key at a time. The engine reads the control map when the event plays ([pattern-model.md](../../pattern-model.md#the-chaining-api-where-it-feels-like-an-instrument)).

This file also fixes the **mapping from control-map keys to engine params/opcodes** — which keys get packed into a `NoteOn`/`TrigSample` field versus emitted as a separate `ParamSet`/`TrackFx` ([event-protocol.md](../../event-protocol.md#opcodes)). The scheduler ([06-scheduler-integration.md](06-scheduler-integration.md)) consumes that mapping; this doc defines it.

## Location

```
packages/tekno/src/
  sound.ts        # sound(), note(), freq(); the ControlMap type; the modifier methods
  instrument.ts   # note-name/number parsing → frequency; the control-key → engine-param registry
  sound.test.ts
```

`sound.ts` imports `parseMini` ([02](02-mini-notation-parser.md)) and `Pat`/`fmap` ([01](01-pattern-core.md)/[03](03-combinators.md)); `instrument.ts` is leaf-ish (note math + the param registry table). The control-key registry is the single source of truth shared with [06](06-scheduler-integration.md).

## Design

### The control map

```ts
// Hap.value for a playable pattern. Open-ended but with known keys.
export interface ControlMap {
  s?: string;        // sound/instrument name ("bd", "saw", a sample name)
  note?: number;     // semitone (MIDI-ish) — converted to freq at compile
  freq?: number;     // explicit frequency in Hz (wins over note)
  gain?: number;
  pan?: number;      // 0..1 (0 = L, 1 = R)
  speed?: number;    // playback/pitch speed multiplier
  // filter
  lpf?: number; hpf?: number; resonance?: number;
  // envelope
  attack?: number; decay?: number; sustain?: number; release?: number;
  // sends / fx
  room?: number;     // reverb send (Phase 3 engine; key accepted now)
  sidechain?: string; // duck against named track (Phase 3; key accepted now)
  // …extensible; unknown keys pass through to the param registry.
  [k: string]: number | string | Pat<number> | undefined;
}
```

A control value may itself be a **pattern or signal** (`lpf: sine.range(...)`), per [pattern-model.md](../../pattern-model.md#parameters-can-themselves-be-patterns)). That `Pat<number>` rides inside the `ControlMap` until the scheduler samples it at the note onset ([05](05-continuous-signals.md), [06](06-scheduler-integration.md)).

### `fmap` — value mapping (defined here, mixed onto `Pat`)

```ts
declare module "./pattern" { interface Pat<T> { fmap<U>(f: (v: T) => U): Pat<U>; } }
Pat.prototype.fmap = function <T, U>(this: Pat<T>, f: (v: T) => U): Pat<U> {
  return pattern((span) => this.query(span).map(h => ({ ...h, value: f(h.value) })));
};
```

`fmap` only touches `value`, never time — so it preserves all `whole`/`part` structure the combinators rely on. Every modifier method below is `fmap` writing one key.

### Constructors

```ts
// "bd*4" → Pat<ControlMap> with value { s: "bd" } per hap.
export const sound = (s: string): Pat<ControlMap> =>
  parseMini(s).fmap(tok => ({ s: tok }));
export const s = sound;

// "c2 eb2 g2" → Pat<ControlMap> with value { note: <semitone> }.
export const note = (n: string): Pat<ControlMap> =>
  parseMini(n).fmap(tok => ({ note: parseNote(tok) }));

// "220 330" → Pat<ControlMap> with value { freq: <Hz> }.
export const freq = (f: string): Pat<ControlMap> =>
  parseMini(f).fmap(tok => ({ freq: parseFloat(tok) }));
```

Each starts the chain; subsequent methods merge more keys.

### Note-name & number parsing (`instrument.ts`)

`note` accepts note names (`c3`, `eb2`, `f#4`) or numbers ([pattern-model.md](../../pattern-model.md#mini-notation)). Parse to a semitone number (the `note` key); the scheduler converts to frequency with the standard equal-temperament formula at compile time:

```ts
const NAME_TO_SEMI: Record<string, number> = { c:0, d:2, e:4, f:5, g:7, a:9, b:11 };

export function parseNote(tok: string): number {
  if (/^-?\d+(\.\d+)?$/.test(tok)) return parseFloat(tok);   // bare number = semitone offset
  const m = /^([a-gA-G])([#sb]*)(-?\d+)?$/.exec(tok);
  if (!m) throw new Error(`bad note: ${tok}`);
  let semi = NAME_TO_SEMI[m[1].toLowerCase()];
  for (const acc of m[2]) semi += acc === "b" ? -1 : 1;      // # / s = sharp, b = flat
  const octave = m[3] === undefined ? 5 : parseInt(m[3], 10); // default octave 5
  return semi + (octave + 1) * 12;                            // MIDI-style: c5 → 60-ish; tunable
}

// Equal temperament: A4 (semitone 69) = 440 Hz.
export const noteToFreq = (semi: number): number => 440 * 2 ** ((semi - 69) / 12);
```

`c2 c2 eb2 g2` from the README parses to semitones, and `noteToFreq` yields the rolling bass frequencies. `freq("220 330")` skips note math entirely.

### The modifier methods

Each modifier is one line: `fmap` that merges a key. They are declared on `Pat<ControlMap>` and installed at module load (same mixin pattern as [03](03-combinators.md)).

```ts
type Val = number | Pat<number>;   // a control value may be a pattern/signal
const set = (key: string) => function (this: Pat<ControlMap>, v: Val) {
  return this.fmap(cm => ({ ...cm, [key]: v }));
};

Pat.prototype.gain      = set("gain");
Pat.prototype.pan       = set("pan");
Pat.prototype.speed     = set("speed");
Pat.prototype.lpf       = set("lpf");
Pat.prototype.hpf       = set("hpf");
Pat.prototype.resonance = set("resonance");
Pat.prototype.room      = set("room");

// oscillator selectors set the `s` key (synth waveform name)
Pat.prototype.saw    = function () { return this.fmap(cm => ({ ...cm, s: cm.s ?? "saw" })); };
Pat.prototype.square = function () { return this.fmap(cm => ({ ...cm, s: "square" })); };
Pat.prototype.sine   = function () { return this.fmap(cm => ({ ...cm, s: "sine" })); };
Pat.prototype.tri    = function () { return this.fmap(cm => ({ ...cm, s: "tri" })); };

// multi-arg sugar
Pat.prototype.adsr = function (a: number, d: number, sus: number, r: number) {
  return this.fmap(cm => ({ ...cm, attack: a, decay: d, sustain: sus, release: r }));
};
// Phase 3 keys accepted now so README parses; they just ride in the control map.
Pat.prototype.sidechain = function (track: string) { return this.fmap(cm => ({ ...cm, sidechain: track })); };
```

#### Worked chaining example (from the model)

```ts
note("c2 c2 eb2 g2")
  .saw()
  .lpf(800)
  .resonance(0.4)
  .adsr(0.01, 0.1, 0.6, 0.2)
  .room(0.3)
  .gain(0.85)
  .pan(0.4)
```

Each method is an `fmap`. After the chain, every hap's value is:

```ts
{ note: <semi>, s: "saw", lpf: 800, resonance: 0.4,
  attack: 0.01, decay: 0.1, sustain: 0.6, release: 0.2,
  room: 0.3, gain: 0.85, pan: 0.4 }
```

```
queryArc(note("c2 c2 eb2 g2").saw().gain(0.85), 0, 1)
→ 4 haps, each value e.g. { note: 36, s:"saw", gain:0.85 }, at 0, .25, .5, .75
```

The chain is pure value-mapping; timing (4 even onsets) comes entirely from `parseMini` + `fastcat` ([02](02-mini-notation-parser.md)). This is why combinators and modifiers compose freely.

### Control-key → engine-param registry

The registry (in `instrument.ts`) is the contract with the scheduler. For each key it records **how it crosses the bridge** ([event-protocol.md](../../event-protocol.md#opcodes)): packed into the note record, or emitted as a separate event.

```ts
type Routing =
  | { kind: "noteField"; field: "freq" | "vel" | "dur" | "instrument" }  // packed into NoteOn/TrigSample
  | { kind: "trigField"; field: "speed" | "gain" | "start" | "sample" }  // packed into TrigSample
  | { kind: "paramSet";  param: number }   // separate 0x10 ParamSet, per-voice
  | { kind: "trackFx";   param: number };  // separate 0x30 TrackFx, per-track

export const PARAM_REGISTRY: Record<string, Routing> = {
  // packed into the NoteOn (synth) — cheap, no extra event
  freq:      { kind: "noteField", field: "freq" },
  note:      { kind: "noteField", field: "freq" },   // converted via noteToFreq first
  gain:      { kind: "noteField", field: "vel"  },   // → velocity 0..1
  // per-voice params emitted alongside the note
  lpf:       { kind: "paramSet", param: 0x01 },
  hpf:       { kind: "paramSet", param: 0x02 },
  resonance: { kind: "paramSet", param: 0x03 },
  attack:    { kind: "paramSet", param: 0x04 },
  decay:     { kind: "paramSet", param: 0x05 },
  sustain:   { kind: "paramSet", param: 0x06 },
  release:   { kind: "paramSet", param: 0x07 },
  // per-track FX / sends
  pan:       { kind: "trackFx", param: 0x01 },
  room:      { kind: "trackFx", param: 0x02 },
  sidechain: { kind: "trackFx", param: 0x03 },   // Phase 3 semantics
  // sampler
  speed:     { kind: "trigField", field: "speed" },
};
```

Rationale, matching the protocol note that "the scheduler decides per parameter whether it is per-note (packed) or per-track (separate event)" ([event-protocol.md](../../event-protocol.md#opcodes)):

- **Packed into `NoteOn`/`TrigSample`** (`freq`/`note`→freq, `gain`→velocity, `speed`): one per voice trigger, no extra record. `s` selects the instrument id (resolved via `defineInstrument`, [event-protocol.md](../../event-protocol.md#control-call-surface-non-real-time)).
- **`ParamSet`** (`lpf`, envelope): per-voice settings that the engine applies to that voice; one extra 32-byte record per note, frame-stamped to the same onset.
- **`TrackFx`** (`pan`, `room`, `sidechain`): per-track routing, emitted on the track channel (track id ≠ 0), once per change.

If a value is itself a `Pat<number>` (a patterned/continuous param), the scheduler samples it at the onset before applying this routing — and may instead emit a `ParamRamp` for continuous modulation ([05](05-continuous-signals.md)). The registry is unchanged; only the value source differs.

## Interfaces

- **[02-mini-notation-parser.md](02-mini-notation-parser.md):** `sound`/`note`/`freq` call `parseMini`, then `fmap` the token to a `ControlMap`. A control value that is a mini string (`.gain("0.6 0.9")`) is itself `parseMini`'d into a `Pat<number>` and stored as the value (sampled later).
- **[03-combinators.md](03-combinators.md):** value-aware combinators (`jux`, `hurry`, `off(..., x => x.gain(0.5))`) read/write `ControlMap` keys via the same `fmap`. Modifier methods and combinator methods coexist on `Pat.prototype`.
- **[05-continuous-signals.md](05-continuous-signals.md):** signals are the most common patterned control values; `.lpf(sine.range(300,1200))` stores a continuous `Pat<number>` in the `lpf` slot.
- **[06-scheduler-integration.md](06-scheduler-integration.md):** consumes `PARAM_REGISTRY` and `noteToFreq` to compile each `Hap<ControlMap>` into the right mix of `NoteOn`/`TrigSample` + `ParamSet`/`TrackFx` records.

## Edge cases & risks

- **Purity:** modifiers are `fmap` (pure). A modifier overwriting a key (`.gain(0.5).gain(0.9)`) just yields the last write — deterministic. Patterned values stored in the map keep their own purity.
- **Fractional time precision:** not applicable here — modifiers never touch time. (Timing precision is owned by [01](01-pattern-core.md)/[02](02-mini-notation-parser.md).)
- **Continuous values:** a control value that is a continuous signal has no onset; storing it in the map is fine because it is only ever *sampled* (zero-width query at the note onset, [05](05-continuous-signals.md)), never triggered.
- **Note math edge cases:** octave-less names default to octave 5; `parseNote` must accept `eb2` (flat), `f#4` (sharp), `c#-1` (negative octave), bare numbers (`"60"` = semitone 60). Out-of-range frequencies are passed through; the engine clamps. Tested against the README's `c2 c2 eb2 g2`.
- **Unknown keys:** a modifier for a key not in `PARAM_REGISTRY` is allowed (open `ControlMap`); the scheduler warns and drops it rather than crashing, so future keys degrade gracefully.
- **`s` precedence:** `note(...).saw()` sets `s:"saw"` (synth) while keeping `note`; `sound("bd")` sets `s:"bd"` and has no `note`. A value with both `freq` and `note` uses `freq` (explicit wins), encoded in the registry order.
- **Performance:** each modifier wraps one more `fmap` closure; a deep chain is a small constant per hap. Control-plane only.

## Acceptance criteria

- `queryArc(sound("bd ~ sn ~"), 0, 1)` → values `{s:"bd"}` and `{s:"sn"}` at the canonical times (matches [pattern-model.md](../../pattern-model.md#testing-patterns) with the `s` key).
- `queryArc(sound("bd").gain(0.9), 0, 1)[0].value` deep-equals `{ s:"bd", gain:0.9 }`.
- `parseNote("a4") === 69` and `noteToFreq(69) === 440`; `parseNote("c5") - parseNote("c4") === 12`.
- `queryArc(note("c2 eb2 g2"), 0, 1)` → three haps with ascending `note` semitones at `0, 1/3, 2/3`.
- The full README bass chain (`note("c2 c2 eb2 g2").saw().lpf(800).resonance(0.4).adsr(...).room(0.3).gain(0.85).pan(0.4)`) yields a `ControlMap` with all those keys on every hap.
- `PARAM_REGISTRY` routes `note`/`gain` to packed note fields, `lpf`/`resonance`/envelope to `ParamSet`, `pan`/`room` to `TrackFx` (asserted in a table test that [06](06-scheduler-integration.md) reuses).
- `.lpf(sine.range(300,1200))` stores a `Pat<number>` in the `lpf` slot (its value is a function, deferred to sampling).
