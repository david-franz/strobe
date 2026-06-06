import { describe, it, expect, beforeEach } from "vitest";
import {
  assign,
  getTrack,
  promotePending,
  activeTracks,
  setMuted,
  setSoloed,
  hush,
  clearAllTracks,
  p1,
} from "./tracks";
import { setActiveClock } from "../clock/singleton";
import { Clock } from "../clock/clock";
import type { Engine } from "../backend/engine";
import { sound } from "../pattern/sound";
import { queryArc } from "../pattern/pattern";

function clockAt(frame: number): Clock {
  const engine: Engine = {
    start: async () => {},
    stop: async () => {},
    pushEvents: () => {},
    stats: () => ({ currentFrame: frame, underruns: 0, lateEvents: 0, ringFullness: 0 }),
  };
  const clock = new Clock(engine);
  clock.cps = 1;
  clock.anchor(48000);
  return clock;
}

beforeEach(() => clearAllTracks());

describe("track registry & hot-swap", () => {
  it("first assignment takes effect immediately", () => {
    const a = sound("bd*4");
    p1(a);
    expect(getTrack(1)).toBe(a);
  });

  it("a live re-assignment is staged, not applied, until the boundary", () => {
    setActiveClock(clockAt(12000)); // cycle 0.25
    const a = sound("bd*4");
    const b = sound("bd*8");
    assign(1, a); // immediate
    assign(1, b); // staged (pendingFromCycle = ceil(0.25) = 1)
    expect(getTrack(1)).toBe(a); // current unchanged

    promotePending(0); // before the boundary → no promotion
    expect(getTrack(1)).toBe(a);

    promotePending(1); // at the boundary → swap
    expect(getTrack(1)).toBe(b);
  });

  it("activeTracks excludes muted and honours solo", () => {
    p1(sound("bd"));
    assign(2, sound("hh"));
    expect(activeTracks().map((t) => t.id).sort()).toEqual([1, 2]);

    setMuted(2, true);
    expect(activeTracks().map((t) => t.id)).toEqual([1]);

    setMuted(2, false);
    setSoloed(2, true);
    expect(activeTracks().map((t) => t.id)).toEqual([2]); // only soloed
  });

  it("hush silences every track", () => {
    p1(sound("bd*4"));
    hush();
    const tracks = activeTracks();
    // current is now `silence` → producing no onsets when queried.
    for (const t of tracks) {
      expect(queryArc(t.pattern as Parameters<typeof queryArc>[0], 0, 1)).toEqual([]);
    }
  });

  it("track ids map 1:1 to the protocol track field", () => {
    p1(sound("bd"));
    assign(3, sound("sn"));
    expect(activeTracks().map((t) => t.id).sort()).toEqual([1, 3]);
  });
});
