import { describe, it, expect } from "vitest";
import { noteToHz, midiToHz, synth } from "./synth";

describe("noteToHz", () => {
  it("resolves the README pitches (scientific pitch)", () => {
    expect(noteToHz("c2")).toBeCloseTo(65.41, 1);
    expect(noteToHz("e2")).toBeCloseTo(82.41, 1);
    expect(noteToHz("g2")).toBeCloseTo(98.0, 1);
    expect(noteToHz("a4")).toBeCloseTo(440, 3);
  });

  it("handles accidentals", () => {
    expect(noteToHz("c#2")).toBeCloseTo(noteToHz("db2"), 4);
    expect(noteToHz("eb2")).toBeCloseTo(noteToHz("d#2"), 4);
    expect(noteToHz("f#4")).toBeCloseTo(midiToHz(66), 4);
  });

  it("treats a bare number as a MIDI note", () => {
    expect(noteToHz(69)).toBeCloseTo(440, 4);
    expect(noteToHz(60)).toBeCloseTo(261.63, 1); // middle C
  });

  it("rejects nonsense", () => {
    expect(() => noteToHz("h9")).toThrow();
  });
});

describe("SynthBuilder", () => {
  it("maps the README chain onto an InstrumentConfig", () => {
    const cfg = synth().saw().lpf(800).resonance(0.4).adsr(0.01, 0.1, 0.6, 0.2).config();
    expect(cfg.type).toBe("synth");
    expect(cfg.osc.kind).toBe("saw");
    expect(cfg.filter).toMatchObject({ mode: "lp", cutoff: 800, q: 0.4 });
    expect(cfg.ampEnv).toEqual({ attack: 0.01, decay: 0.1, sustain: 0.6, release: 0.2 });
  });

  it("square() carries pulse width; fenv() sets filter envelope + amount", () => {
    const cfg = synth().square(0.25).fenv(3000, 0.001, 0.2, 0, 0.1).config();
    expect(cfg.osc).toMatchObject({ kind: "square", pulseWidth: 0.25 });
    expect(cfg.filter.envAmount).toBe(3000);
    expect(cfg.filterEnv).toEqual({ attack: 0.001, decay: 0.2, sustain: 0, release: 0.1 });
  });

  it("config() returns an isolated copy", () => {
    const b = synth().saw();
    const a = b.config();
    b.sine();
    expect(a.osc.kind).toBe("saw"); // unaffected by later chaining
  });
});
