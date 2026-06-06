// The engine interface the rest of the library targets, plus the native backend.
//
// Nothing above this interface knows whether the backend is the native napi addon
// or (later) WASM/AudioWorklet — moving to the browser swaps `createNativeEngine`
// for `createWebEngine` and changes no song code (architecture.md "The bridge").

import { Engine as NativeEngine } from "@strobe/native";

export interface EngineStats {
  /** Engine frame counter — the source of truth for timing. */
  currentFrame: number;
  underruns: number;
  lateEvents: number;
  /** Ring occupancy, 0..1. */
  ringFullness: number;
}

export interface Engine {
  start(opts?: { sampleRate?: number; bufferFrames?: number }): Promise<void>;
  stop(): Promise<void>;
  /** Hot path; non-blocking; one call per scheduler tick. */
  pushEvents(buffer: ArrayBuffer): void;
  stats(): EngineStats;
}

/** Native backend: a thin wrapper over the napi addon. */
export function createNativeEngine(): Engine {
  const e = new NativeEngine();
  return {
    start: async (o) => {
      e.start(o);
    },
    stop: async () => {
      e.stop();
    },
    // Buffer.from(ArrayBuffer) is a zero-copy view; pushEvents copies into the ring
    // synchronously, so reusing the source ArrayBuffer across ticks is safe.
    pushEvents: (buf) => {
      e.pushEvents(Buffer.from(buf));
    },
    stats: () => e.stats(),
  };
}
