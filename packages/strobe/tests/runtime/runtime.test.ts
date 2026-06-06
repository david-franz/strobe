import { describe, it, expect, beforeEach } from "vitest";
import { loadSongSafely } from "../../src/runtime/runtime";
import { p1, getTrack, clearAllTracks } from "../../src/runtime/tracks";
import { bdEvery } from "../../src/pattern/builders";

beforeEach(() => clearAllTracks());

describe("loadSongSafely", () => {
  it("never rejects on a bad/missing song and keeps the last good pattern", async () => {
    p1(bdEvery(4));
    const kept = getTrack(1);
    await expect(loadSongSafely("/nonexistent/strobe-song-does-not-exist.ts")).resolves.toBeUndefined();
    expect(getTrack(1)).toBe(kept); // unchanged — the engine keeps playing
  });
});
