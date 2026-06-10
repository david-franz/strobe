import { describe, it, expect } from "vitest";
import { renderScope } from "../../src/runtime/scope";

describe("renderScope", () => {
  it("produces height rows of width columns (height forced odd)", () => {
    const lines = renderScope(new Float32Array(100), { width: 40, height: 8 });
    expect(lines.length).toBe(9); // 8 → forced to an odd 9 for a true center
    expect(lines.every((l) => [...l].length === 40)).toBe(true);
  });

  it("zero-valued samples draw a flat line at the center", () => {
    const lines = renderScope(new Float32Array(64), { width: 10, height: 5 });
    expect(lines[2]).toBe("█".repeat(10)); // 0 → center row, plotted as a real sample
    lines.forEach((l, r) => {
      if (r !== 2) expect(l.trim()).toBe(""); // nothing elsewhere
    });
  });

  it("a full-scale wave fills the top and bottom rows", () => {
    const n = 200;
    const s = new Float32Array(n);
    for (let i = 0; i < n; i++) s[i] = i % 2 === 0 ? 1 : -1; // each column spans both extremes
    const lines = renderScope(s, { width: 20, height: 9 });
    expect(lines[0].includes("█")).toBe(true); // top = +1
    expect(lines[8].includes("█")).toBe(true); // bottom = -1
  });

  it("a positive DC sits as a flat line near the top", () => {
    const lines = renderScope(new Float32Array(64).fill(1), { width: 8, height: 9 });
    expect(lines[0]).toBe("█".repeat(8)); // +1 → top row, every column
    expect(lines[8].includes("█")).toBe(false); // nothing at the bottom
  });

  it("empty input is just the axis", () => {
    const lines = renderScope([], { width: 6, height: 3 });
    expect(lines[1]).toBe("─".repeat(6));
  });

  it("clamps out-of-range amplitude to the top/bottom rows (no overflow)", () => {
    const lines = renderScope(new Float32Array(16).fill(5), { width: 4, height: 5 });
    expect(lines.length).toBe(5);
    expect(lines[0]).toBe("█".repeat(4)); // +5 clamped to +1 → top
  });
});
