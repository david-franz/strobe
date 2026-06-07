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
  beginReload,
  endReload,
  abortReload,
  p1,
} from "../../src/runtime/tracks";
import { setActiveClock } from "../../src/clock/singleton";
import { Clock } from "../../src/clock/clock";
import type { Engine } from "../../src/backend/engine";
import { sound } from "../../src/pattern/sound";
import { queryArc } from "../../src/pattern/pattern";

function clockAt(frame: number): Clock {
  const engine: Engine = {
    start: async () => {},
    stop: async () => {},
    pushEvents: () => {},
    stats: () => ({ currentFrame: frame, underruns: 0, lateEvents: 0, ringFullness: 0 }),
    loadSampleFile: () => ({ id: 0, channels: 0, frames: 0, sampleRate: 0 }),
    loadSamplePcm: () => ({ id: 0, channels: 0, frames: 0, sampleRate: 0 }),
    prepareStretched: () => ({ id: 0, channels: 0, frames: 0, sampleRate: 0 }),
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

  it("a reload silences tracks the new revision drops", () => {
    setActiveClock(clockAt(12000)); // cycle 0.25
    p1(sound("bd*4"));
    assign(2, sound("hh"));

    // Reload: the revision reassigns only track 1.
    beginReload();
    assign(1, sound("bd*8"));
    endReload();

    promotePending(1); // next boundary
    expect(queryArc(getTrack(1) as Parameters<typeof queryArc>[0], 0, 1).length).toBeGreaterThan(0);
    expect(queryArc(getTrack(2) as Parameters<typeof queryArc>[0], 0, 1)).toEqual([]); // dropped → silence
  });

  it("an aborted reload (song threw) keeps every track", () => {
    setActiveClock(clockAt(12000));
    p1(sound("bd*4"));
    assign(2, sound("hh"));
    const kept2 = getTrack(2);

    beginReload();
    assign(1, sound("bd*8")); // got partway, then "threw"
    abortReload();

    promotePending(99);
    expect(getTrack(2)).toBe(kept2); // untouched — not silenced
  });
});
