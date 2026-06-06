import { describe, it, expect } from "vitest";
import { sound, note, freq, jux } from "../../src/pattern/sound";
import { parseNote, noteToFreq, PARAM_REGISTRY } from "../../src/pattern/control";
import { rev } from "../../src/pattern/combinators";
import { queryArc, sameTime, pure } from "../../src/pattern/pattern";
import type { ControlMap } from "../../src/pattern/control";

const values = (haps: { value: ControlMap }[]) => haps.map((h) => h.value);

describe("sound / note / freq", () => {
  it("sound() carries the token in the s key", () => {
    expect(values(queryArc(sound("bd ~ sn ~"), 0, 1))).toEqual([{ s: "bd" }, { s: "sn" }]);
  });

  it("a modifier merges one key", () => {
    expect(queryArc(sound("bd").gain(0.9), 0, 1)[0].value).toEqual({ s: "bd", gain: 0.9 });
  });

  it("note() resolves names to semitones", () => {
    const o = queryArc(note("c2 eb2 g2"), 0, 1);
    expect(o.map((h) => h.value.note)).toEqual([36, 39, 43]);
    expect(o.map((h) => h.part.begin).every((t, i) => sameTime(t, i / 3))).toBe(true);
  });

  it("the README bass chain builds the full control map", () => {
    const v = queryArc(
      note("c2 c2 eb2 g2").saw().lpf(800).resonance(0.4).adsr(0.01, 0.1, 0.6, 0.2).room(0.3).gain(0.85).pan(0.4),
      0,
      1,
    )[0].value;
    expect(v).toMatchObject({
      note: 36,
      s: "saw",
      lpf: 800,
      resonance: 0.4,
      attack: 0.01,
      decay: 0.1,
      sustain: 0.6,
      release: 0.2,
      room: 0.3,
      gain: 0.85,
      pan: 0.4,
    });
  });

  it("freq() skips note math", () => {
    expect(queryArc(freq("220 330"), 0, 1).map((h) => h.value.freq)).toEqual([220, 330]);
  });

  it("note parsing and equal temperament", () => {
    expect(parseNote("a4")).toBe(69);
    expect(noteToFreq(69)).toBe(440);
    expect(parseNote("c5") - parseNote("c4")).toBe(12);
    expect(parseNote("eb2")).toBe(parseNote("d#2"));
    expect(parseNote("60")).toBe(60);
  });

  it("PARAM_REGISTRY routes keys per the contract", () => {
    expect(PARAM_REGISTRY.note).toMatchObject({ kind: "noteField", field: "freq" });
    expect(PARAM_REGISTRY.gain).toMatchObject({ kind: "noteField", field: "vel" });
    expect(PARAM_REGISTRY.lpf).toMatchObject({ kind: "trackFx", slot: 0 }); // track performance filter
    expect(PARAM_REGISTRY.resonance).toMatchObject({ kind: "trackFx", slot: 0 });
    expect(PARAM_REGISTRY.room).toMatchObject({ kind: "trackFx" });
  });

  it("jux pans original left and the transform right", () => {
    const haps = queryArc(jux(rev, sound("a b")), 0, 1);
    const left = haps.filter((h) => h.value.pan === 0).map((h) => h.value.s);
    const right = haps.filter((h) => h.value.pan === 1).map((h) => h.value.s);
    expect(left.sort()).toEqual(["a", "b"]);
    expect(right.sort()).toEqual(["a", "b"]);
  });

  it("a control value can be a deferred pattern (for signals)", () => {
    const v = queryArc(sound("saw").lpf(pure(440)), 0, 1)[0].value;
    expect(typeof v.lpf).toBe("object"); // a Pat<number>, sampled later by the scheduler
  });
});
