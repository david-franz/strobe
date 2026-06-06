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
  lpf?: number | Pat<number>; // track low-pass cutoff (Hz) — the performance filter
  hpf?: number | Pat<number>; // track high-pass cutoff (Hz)
  bpf?: number | Pat<number>; // track band-pass cutoff (Hz)
  resonance?: number | Pat<number>; // track filter resonance / Q
  eqLow?: number | Pat<number>; // EQ low-shelf gain (dB)
  eqMid?: number | Pat<number>; // EQ peaking-mid gain (dB, at ~1 kHz)
  eqHigh?: number | Pat<number>; // EQ high-shelf gain (dB)
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
  | { kind: "trackFx"; slot: number; param: number }; // 0x30 TrackFx; id packs (fx_slot, fx_param)

// ── Effect-rack layout — mirrors engine/src/dsp/fx (mod.rs + filter_eq.rs) ─────────
const SLOT_FILTER = 0;
const SLOT_EQ = 1;
const SLOT_DUCK = 8;
const CHAIN_SLOT = 0xff; // chain-level params (send levels) rather than a numbered slot
const SEND_REVERB = 0;
const CHAIN_SEND_BASE = 0x10; // send `bus` level → CHAIN_SLOT param 0x10 + bus
// Performance-filter params: each cutoff id also selects the SVF mode (≥0x02: avoid FX_MIX/FX_BYPASS).
const PF_CUTOFF_LP = 0x02;
const PF_Q = 0x03;
const PF_CUTOFF_HP = 0x04;
const PF_CUTOFF_BP = 0x05;
// EQ band gain ids: band b → 0x10 + b*4 + 1. Bands: 0 = low shelf, 1 = peak, 2 = high shelf.
const EQ_LOW_GAIN = 0x11;
const EQ_MID_GAIN = 0x15;
const EQ_HIGH_GAIN = 0x19;

/** Pack a track-effect param id: high byte = fx slot, low byte = fx param. Matches `pack_fx`
 *  in engine/src/dsp/fx/mod.rs. */
export const packFx = (slot: number, param: number): number =>
  (((slot & 0xff) << 8) | (param & 0xff)) >>> 0;

// `paramSet` numbers align with the engine's ParamId enum (voices/mod.rs): AmpAttack=2…
// AmpRelease=5, Pan=13. The filter/EQ are *track* effects (TrackFx), not per-voice params.
export const PARAM_REGISTRY: Record<string, Routing> = {
  freq: { kind: "noteField", field: "freq" },
  note: { kind: "noteField", field: "freq" }, // via noteToFreq first
  gain: { kind: "noteField", field: "vel" }, // → velocity 0..1
  lpf: { kind: "trackFx", slot: SLOT_FILTER, param: PF_CUTOFF_LP },
  hpf: { kind: "trackFx", slot: SLOT_FILTER, param: PF_CUTOFF_HP },
  bpf: { kind: "trackFx", slot: SLOT_FILTER, param: PF_CUTOFF_BP },
  resonance: { kind: "trackFx", slot: SLOT_FILTER, param: PF_Q },
  eqLow: { kind: "trackFx", slot: SLOT_EQ, param: EQ_LOW_GAIN },
  eqMid: { kind: "trackFx", slot: SLOT_EQ, param: EQ_MID_GAIN },
  eqHigh: { kind: "trackFx", slot: SLOT_EQ, param: EQ_HIGH_GAIN },
  attack: { kind: "paramSet", param: 2 },
  decay: { kind: "paramSet", param: 3 },
  sustain: { kind: "paramSet", param: 4 },
  release: { kind: "paramSet", param: 5 },
  pan: { kind: "paramSet", param: 13 },
  speed: { kind: "trigField", field: "speed" },
  room: { kind: "trackFx", slot: CHAIN_SLOT, param: CHAIN_SEND_BASE + SEND_REVERB },
  sidechain: { kind: "trackFx", slot: SLOT_DUCK, param: 0x02 }, // source track (feature 06)
};

/** Keys the engine can ramp (the scheduler may emit a ParamRamp instead of a per-note
 *  ParamSet/TrackFx for a continuous signal on these). */
export const PARAM_RAMP_KEYS = new Set([
  "lpf",
  "hpf",
  "bpf",
  "resonance",
  "eqLow",
  "eqMid",
  "eqHigh",
  "pan",
  "gain",
  "room",
]);

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
