import { describe, it, expect } from "vitest";
import { seq, every, euclid } from "../../src/pattern/builders";
import { queryArc } from "../../src/pattern/pattern";

describe("pattern builders", () => {
  it("every(v, n) places n evenly-spaced hits", () => {
    const haps = queryArc(every({ s: "bd" }, 4), 0, 1);
    expect(haps.map((h) => h.whole?.begin)).toEqual([0, 0.25, 0.5, 0.75]);
    expect(haps.every((h) => (h.value as { s: string }).s === "bd")).toBe(true);
  });

  it("seq places one value per step and skips null rests", () => {
    const haps = queryArc(seq([{ s: "sn" }, null, { s: "sn" }, null]), 0, 1);
    expect(haps.map((h) => h.whole?.begin)).toEqual([0, 0.5]);
  });

  it("euclid spreads k hits over n slots", () => {
    const haps = queryArc(euclid({ s: "hh" }, 3, 8), 0, 1);
    expect(haps.map((h) => h.whole?.begin)).toEqual([0, 0.375, 0.75]);
  });

  it("carries arbitrary note values (instrument + freq)", () => {
    const haps = queryArc(seq([{ instrument: 5, freq: 110 }]), 0, 1);
    expect(haps[0].value).toEqual({ instrument: 5, freq: 110 });
  });
});
