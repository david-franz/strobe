import { describe, it, expect } from "vitest";
import { sine, cosine, saw, isaw, tri, square, rand, perlin, isContinuousPattern } from "./signals";
import { queryArc } from "./pattern";

const at = (p: Parameters<typeof queryArc>[0], t: number) => queryArc(p, t, t)[0].value as number;

describe("continuous signals", () => {
  it("sine spans 0..1 with the expected phase", () => {
    expect(at(sine, 0)).toBeCloseTo(0.5, 9);
    expect(at(sine, 0.25)).toBeCloseTo(1.0, 9);
    expect(at(sine, 0.75)).toBeCloseTo(0.0, 9);
  });

  it("cosine, saw, isaw, tri, square", () => {
    expect(at(cosine, 0)).toBeCloseTo(1.0, 9);
    expect(at(saw, 0.25)).toBeCloseTo(0.25, 9);
    expect(at(isaw, 0.25)).toBeCloseTo(0.75, 9);
    expect(at(tri, 0.25)).toBeCloseTo(0.5, 9);
    expect(at(square, 0.1)).toBe(0);
    expect(at(square, 0.6)).toBe(1);
  });

  it("every signal is continuous (whole undefined), including after range/slow", () => {
    for (const sig of [sine, saw, rand, perlin]) {
      expect(queryArc(sig, 0, 1)[0].whole).toBeUndefined();
    }
    expect(queryArc(sine.range(300, 1200).slow(8), 0, 1)[0].whole).toBeUndefined();
    expect(isContinuousPattern(sine)).toBe(true);
    expect(isContinuousPattern(saw.range(0, 10))).toBe(true);
  });

  it(".range rescales 0..1 → lo..hi", () => {
    expect(at(sine.range(300, 1200), 0.25)).toBeCloseTo(1200, 6);
    expect(at(sine.range(300, 1200), 0)).toBeCloseTo(750, 6);
  });

  it("slow(8) stretches the period ×8", () => {
    // sampling sine.slow(8) at cycle 2 reads sine at 2/8 = 0.25 → peak (1.0)
    expect(at(sine.slow(8), 2)).toBeCloseTo(at(sine, 0.25), 9);
    expect(at(sine.slow(8), 0)).toBeCloseTo(at(sine, 0), 9);
  });

  it("rand and perlin are deterministic and in 0..1", () => {
    expect(at(rand, 0.314)).toBe(at(rand, 0.314));
    expect(at(perlin, 12.7)).toBe(at(perlin, 12.7));
    for (const t of [0.1, 0.5, 3.3, 9.9]) {
      expect(at(rand, t)).toBeGreaterThanOrEqual(0);
      expect(at(rand, t)).toBeLessThan(1);
    }
  });
});
