import { describe, it, expect } from "vitest";
import { p1, getTrack } from "./tracks";
import { bdEvery } from "./pattern";

describe("track slot", () => {
  it("p1 assigns the slot and getTrack(1) reflects the latest pattern", () => {
    const a = bdEvery(4);
    p1(a);
    expect(getTrack(1)).toBe(a);

    const b = bdEvery(8);
    p1(b); // reference replaced
    expect(getTrack(1)).toBe(b);
  });

  it("getTrack returns null for an unassigned slot", () => {
    expect(getTrack(5)).toBeNull();
  });
});
