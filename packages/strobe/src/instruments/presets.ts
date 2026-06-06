// Built-in instrument ids, mirroring the engine's `builtin_instruments()` table. A song
// references these by name; the scheduler packs the id into each NoteOn's instrument
// field. (Dynamic user-defined instruments over the bridge arrive in Phase 2.)

export const PRESETS = {
  bd: 0,
  sn: 1,
  cp: 2,
  hh: 3,
  oh: 4,
  saw: 5,
  square: 6,
  sine: 7,
} as const;

export type PresetName = keyof typeof PRESETS;

/** The value a pattern hap carries, telling the scheduler what to play. */
export interface NoteValue {
  /** Instrument id (overrides `s`). */
  instrument?: number;
  /** A preset name (`"bd"`, `"saw"`, …); resolved to an instrument id. */
  s?: string;
  /** Frequency in Hz (0 = use the instrument's own pitch, e.g. a drum). */
  freq?: number;
  /** Velocity 0..1 (default 0.9). */
  vel?: number;
}

/** Resolve a hap value to the fields a NoteOn needs. */
export function resolveNote(value: unknown): { instrument: number; freq: number; vel: number } {
  const v = (value ?? {}) as NoteValue;
  let instrument = v.instrument;
  if (instrument === undefined && typeof v.s === "string") {
    instrument = (PRESETS as Record<string, number>)[v.s] ?? 0;
  }
  return { instrument: instrument ?? 0, freq: v.freq ?? 0, vel: v.vel ?? 0.9 };
}
