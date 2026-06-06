// The drum song API (Phase 1 slice): `sound("bd sn hh cp")` token → drum instrument.
// Each token resolves to a DrumConfig with sensible defaults; chainable setters override
// them. The pattern integration (`sound("bd*4")` as a pattern) lands in Phase 2.

export type DrumName = "bd" | "sn" | "cp" | "hh" | "oh";
export type DrumKind = "kick" | "snare" | "clap" | "closedHat" | "openHat";

export interface DrumConfig {
  type: "drum";
  kind: DrumKind;
  gain: number;
  pan: number;
  // remaining kind-specific params carry engine defaults; chainable setters add overrides.
  params: Record<string, number>;
}

const NAME_TO_KIND: Record<DrumName, DrumKind> = {
  bd: "kick",
  sn: "snare",
  cp: "clap",
  hh: "closedHat",
  oh: "openHat",
};

/** True if `token` names a drum (`bd`/`sn`/`cp`/`hh`/`oh`). */
export function isDrumName(token: string): token is DrumName {
  return token in NAME_TO_KIND;
}

/** Resolve a drum token to its kind. */
export function drumKind(name: DrumName): DrumKind {
  return NAME_TO_KIND[name];
}

/** A chainable drum builder. */
export class DrumBuilder {
  private cfg: DrumConfig;

  constructor(name: DrumName) {
    this.cfg = { type: "drum", kind: NAME_TO_KIND[name], gain: 0.9, pan: 0.5, params: {} };
  }

  gain(g: number): this {
    this.cfg.gain = g;
    return this;
  }
  pan(p: number): this {
    this.cfg.pan = p;
    return this;
  }
  /** Set the kind's primary decay (kick body / hat tail / snare rattle / clap room). */
  decay(seconds: number): this {
    this.cfg.params.decay = seconds;
    return this;
  }
  /** Tune a drum (e.g. the kick body pitch), in Hz. */
  tone(hz: number): this {
    this.cfg.params.tone = hz;
    return this;
  }

  config(): DrumConfig {
    return structuredClone(this.cfg);
  }
}

/** Build a single drum instrument from a token. */
export function drum(name: DrumName): DrumBuilder {
  return new DrumBuilder(name);
}
