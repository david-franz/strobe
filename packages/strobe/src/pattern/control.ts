// The control map and the control-key → engine-param registry. A playable pattern's
// Hap.value is a ControlMap ({ s: "bd", lpf: 800, gain: 0.9 }); the registry is the
// contract with the scheduler (06) for how each key crosses the 32-byte bridge —
// packed into the note record, or emitted as a separate ParamSet/TrackFx.

import type { Pat } from "./pattern";

/** Hap.value for a playable pattern. Open-ended, with known keys. A value may itself be a
 *  `Pat<number>` (a patterned/continuous parameter), sampled by the scheduler at onset. */
export interface ControlMap {
  s?: string; // sound/instrument name or synth waveform
  note?: number; // semitone (converted to freq at compile)
  freq?: number | Pat<number>; // explicit Hz (wins over note)
  gain?: number | Pat<number>;
  pan?: number | Pat<number>;
  speed?: number | Pat<number>;
  lpf?: number | Pat<number>;
  hpf?: number | Pat<number>;
  resonance?: number | Pat<number>;
  attack?: number;
  decay?: number;
  sustain?: number;
  release?: number;
  room?: number | Pat<number>; // reverb send (Phase 3 engine; key accepted now)
  sidechain?: string; // duck against named track (Phase 3; key accepted now)
  [k: string]: number | string | Pat<number> | undefined;
}

export type Routing =
  | { kind: "noteField"; field: "freq" | "vel" | "dur" | "instrument" } // packed into NoteOn
  | { kind: "trigField"; field: "speed" | "gain" | "start" | "sample" } // packed into TrigSample
  | { kind: "paramSet"; param: number } // separate 0x10 ParamSet (per-voice)
  | { kind: "trackFx"; param: number }; // separate 0x30 TrackFx (per-track; Phase 3 engine)

// Param numbers align with the engine's ParamId enum (voices/mod.rs) so a ParamSet
// actually reaches the voices: Cutoff=0, Resonance=1, AmpAttack=2…AmpRelease=5, Pan=13.
export const PARAM_REGISTRY: Record<string, Routing> = {
  freq: { kind: "noteField", field: "freq" },
  note: { kind: "noteField", field: "freq" }, // via noteToFreq first
  gain: { kind: "noteField", field: "vel" }, // → velocity 0..1
  lpf: { kind: "paramSet", param: 0 }, // Cutoff
  hpf: { kind: "paramSet", param: 0 }, // Cutoff (instrument's filter mode decides)
  resonance: { kind: "paramSet", param: 1 },
  attack: { kind: "paramSet", param: 2 },
  decay: { kind: "paramSet", param: 3 },
  sustain: { kind: "paramSet", param: 4 },
  release: { kind: "paramSet", param: 5 },
  pan: { kind: "paramSet", param: 13 },
  speed: { kind: "trigField", field: "speed" },
  room: { kind: "trackFx", param: 2 }, // Phase 3 engine
  sidechain: { kind: "trackFx", param: 3 }, // Phase 3 engine
};

/** Keys the engine can ramp (the scheduler may emit a ParamRamp instead of a per-note
 *  ParamSet for a continuous signal on these). */
export const PARAM_RAMP_KEYS = new Set(["lpf", "hpf", "resonance", "pan", "gain", "room"]);

const NAME_TO_SEMI: Record<string, number> = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };

/** Parse a note token (`c3`, `eb2`, `f#4`, or a bare number) to a semitone (MIDI-style,
 *  default octave 5). c2 → 36, a4 → 69. */
export function parseNote(tok: string): number {
  if (/^-?\d+(\.\d+)?$/.test(tok)) return parseFloat(tok); // bare number = semitone
  const m = /^([a-gA-G])([#sb]*)(-?\d+)?$/.exec(tok);
  if (!m) throw new Error(`bad note: ${tok}`);
  let semi = NAME_TO_SEMI[m[1].toLowerCase()];
  for (const acc of m[2]) semi += acc === "b" ? -1 : 1; // # / s = sharp, b = flat
  const octave = m[3] === undefined ? 5 : parseInt(m[3], 10);
  return semi + (octave + 1) * 12;
}

/** Equal temperament: A4 (semitone 69) = 440 Hz. */
export const noteToFreq = (semi: number): number => 440 * 2 ** ((semi - 69) / 12);
