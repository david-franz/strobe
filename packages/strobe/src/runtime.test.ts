import { describe, it, expect, vi, afterEach } from "vitest";
import type { Engine } from "./engine";
import { Clock } from "./clock";
import { nextBoundaryDelayMs, swapAtCycleBoundary, loadSongSafely } from "./runtime";
import { p1, getTrack } from "./tracks";
import { bdEvery } from "./pattern";

function fakeEngine() {
  let currentFrame = 0;
  const engine: Engine = {
    start: async () => {},
    stop: async () => {},
    pushEvents: () => {},
    stats: () => ({ currentFrame, underruns: 0, lateEvents: 0, ringFullness: 0 }),
  };
  return { engine, setFrame: (f: number) => (currentFrame = f) };
}

afterEach(() => vi.useRealTimers());

describe("nextBoundaryDelayMs", () => {
  it("waits a full cycle from a boundary", () => {
    expect(nextBoundaryDelayMs(0, 1)).toBe(1000);
  });
  it("waits the remainder mid-cycle, scaled by tempo", () => {
    expect(nextBoundaryDelayMs(0.5, 2)).toBe(250); // half cycle left, 2 cyc/s
    expect(nextBoundaryDelayMs(0.25, 1)).toBe(750);
  });
  it("is near-zero just before a boundary", () => {
    expect(nextBoundaryDelayMs(3.999, 1)).toBeCloseTo(1, 5);
  });
});

describe("swapAtCycleBoundary", () => {
  it("defers the swap until the boundary, then reassigns the slot", async () => {
    vi.useFakeTimers();
    const { engine, setFrame } = fakeEngine();
    const clock = new Clock(engine);
    clock.cps = 1;
    clock.anchor(48000); // epochFrame = 0
    setFrame(12000); // nowCycle = 0.25

    const before = bdEvery(4);
    p1(before);
    expect(getTrack(1)).toBe(before);

    const done = swapAtCycleBoundary(clock, async () => {
      p1(bdEvery(8));
    });

    // Not yet applied — still waiting for the boundary.
    expect(getTrack(1)).toBe(before);

    await vi.advanceTimersByTimeAsync(nextBoundaryDelayMs(0.25, 1)); // 750 ms
    await done;

    expect(getTrack(1)).not.toBe(before);
  });
});

describe("loadSongSafely", () => {
  it("never rejects on a bad/missing song (keeps the last good pattern)", async () => {
    p1(bdEvery(4));
    const kept = getTrack(1);
    await expect(loadSongSafely("/nonexistent/strobe-song-does-not-exist.ts")).resolves.toBeUndefined();
    expect(getTrack(1)).toBe(kept); // unchanged — engine keeps playing
  });
});
