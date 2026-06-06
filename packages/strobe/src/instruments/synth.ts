// The synth song API (Phase 1 slice): the chainable timbre builder that produces an
// `InstrumentConfig` for `defineInstrument()`, plus note-name → Hz resolution. The full
// pattern integration (`note("c2 e2").saw()...` as a pattern) lands in Phase 2; Phase 1
// ships the instrument-config surface and the pitch math the engine needs.

export type OscName = "sine" | "saw" | "square" | "triangle" | "noise" | "wavetable";
export type FilterMode = "lp" | "hp" | "bp" | "notch";

export interface AdsrSpec {
  attack: number;
  decay: number;
  sustain: number;
  release: number;
}

export interface InstrumentConfig {
  type: "synth";
  osc: {
    kind: OscName;
    pulseWidth?: number;
    wavetable?: string;
    phaseReset?: boolean;
  };
  filter: {
    mode: FilterMode;
    cutoff: number;
    q: number;
    smoothMs?: number;
    envAmount?: number;
  };
  ampEnv: AdsrSpec;
  filterEnv: AdsrSpec;
  gain: number; // 0..1
  pan: number; // 0..1, 0.5 = center
}

const SEMITONES: Record<string, number> = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };

/** MIDI note number → frequency (A4 = MIDI 69 = 440 Hz, scientific pitch). */
export function midiToHz(midi: number): number {
  return 440 * 2 ** ((midi - 69) / 12);
}

/** Resolve a note name (`"c2"`, `"eb2"`, `"f#4"`) to frequency in Hz. A bare number is
 *  treated as a MIDI note number. C2 = 65.41 Hz, A4 = 440 Hz (scientific pitch). */
export function noteToHz(note: string | number): number {
  if (typeof note === "number") return midiToHz(note);
  const m = /^([a-gA-G])([#sb]?)(-?\d+)$/.exec(note.trim());
  if (!m) throw new Error(`invalid note name: ${note}`);
  const [, letter, accidental, octave] = m;
  let semi = SEMITONES[letter.toLowerCase()];
  if (accidental === "#" || accidental === "s") semi += 1;
  else if (accidental === "b") semi -= 1;
  const midi = (parseInt(octave, 10) + 1) * 12 + semi;
  return midiToHz(midi);
}

const DEFAULT_ADSR: AdsrSpec = { attack: 0.005, decay: 0.1, sustain: 0.7, release: 0.2 };

/** A chainable builder that accumulates an `InstrumentConfig`. */
export class SynthBuilder {
  private cfg: InstrumentConfig = {
    type: "synth",
    osc: { kind: "sine", phaseReset: true },
    filter: { mode: "lp", cutoff: 20000, q: 0.7 },
    ampEnv: { ...DEFAULT_ADSR },
    filterEnv: { attack: 0.005, decay: 0.1, sustain: 0, release: 0.2 },
    gain: 0.8,
    pan: 0.5,
  };

  sine(): this {
    this.cfg.osc.kind = "sine";
    return this;
  }
  saw(): this {
    this.cfg.osc.kind = "saw";
    return this;
  }
  square(pulseWidth = 0.5): this {
    this.cfg.osc.kind = "square";
    this.cfg.osc.pulseWidth = pulseWidth;
    return this;
  }
  tri(): this {
    this.cfg.osc.kind = "triangle";
    return this;
  }
  noise(): this {
    this.cfg.osc.kind = "noise";
    return this;
  }

  lpf(cutoff: number): this {
    this.cfg.filter.mode = "lp";
    this.cfg.filter.cutoff = cutoff;
    return this;
  }
  hpf(cutoff: number): this {
    this.cfg.filter.mode = "hp";
    this.cfg.filter.cutoff = cutoff;
    return this;
  }
  bpf(cutoff: number): this {
    this.cfg.filter.mode = "bp";
    this.cfg.filter.cutoff = cutoff;
    return this;
  }
  notch(cutoff: number): this {
    this.cfg.filter.mode = "notch";
    this.cfg.filter.cutoff = cutoff;
    return this;
  }
  resonance(q: number): this {
    this.cfg.filter.q = q;
    return this;
  }
  /** Alias for `resonance`. */
  res(q: number): this {
    return this.resonance(q);
  }

  adsr(attack: number, decay: number, sustain: number, release: number): this {
    this.cfg.ampEnv = { attack, decay, sustain, release };
    return this;
  }
  fenv(amount: number, attack: number, decay: number, sustain: number, release: number): this {
    this.cfg.filter.envAmount = amount;
    this.cfg.filterEnv = { attack, decay, sustain, release };
    return this;
  }

  gain(g: number): this {
    this.cfg.gain = g;
    return this;
  }
  pan(p: number): this {
    this.cfg.pan = p;
    return this;
  }

  /** The accumulated instrument config (a copy, so further chaining is isolated). */
  config(): InstrumentConfig {
    return structuredClone(this.cfg);
  }
}

/** Start a synth instrument builder. */
export function synth(): SynthBuilder {
  return new SynthBuilder();
}
