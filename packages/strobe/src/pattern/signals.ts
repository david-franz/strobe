// Continuous signals (pattern-model.md "parameters can themselves be patterns"): sine,
// cosine, saw, isaw, tri, square, rand, perlin — each a continuous Pat<number> (whole:
// undefined, 0..1), with `.range(lo,hi)` to rescale and `.slow`/`.fast` (combinators) to
// set the period. These are how you get movement: `.lpf(sine.range(300,1200).slow(8))`.

import { Pat, signal, queryArc } from "./pattern";
import { hashFloat } from "./rng";
import "./combinators"; // ensure .fast/.slow are installed (signals are chained with them)

const frac = (t: number): number => t - Math.floor(t);

export const saw = signal((t: number) => frac(t));
export const isaw = signal((t: number) => 1 - frac(t));
export const sine = signal((t: number) => 0.5 + 0.5 * Math.sin(2 * Math.PI * t));
export const cosine = signal((t: number) => 0.5 + 0.5 * Math.cos(2 * Math.PI * t));
export const tri = signal((t: number) => {
  const x = frac(t);
  return x < 0.5 ? 2 * x : 2 - 2 * x;
});
export const square = signal((t: number) => (frac(t) < 0.5 ? 0 : 1));

/** White noise, deterministic by time. */
export const rand = signal((t: number) => hashFloat(Math.floor(t * 1e6)));

const smoothstep = (x: number): number => x * x * (3 - 2 * x);
const smoothNoise = (t: number): number => {
  const i = Math.floor(t);
  const f = t - i;
  const a = hashFloat(i);
  const b = hashFloat(i + 1);
  return a + smoothstep(f) * (b - a);
};
/** Smoothed value noise — wanders continuously. */
export const perlin = signal((t: number) => smoothNoise(t));

// ── .range (mixed onto Pat) ──────────────────────────────────────────────────

declare module "./pattern" {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface Pat<T> {
    range(lo: number, hi: number): Pat<number>;
  }
}

let installed = false;
export function installSignalMethods(): void {
  if (installed) return;
  installed = true;
  Pat.prototype.range = function (this: Pat<number>, lo: number, hi: number): Pat<number> {
    return this.fmap((v) => lo + v * (hi - lo)); // value-only → preserves whole: undefined
  };
}

installSignalMethods();

/** True if `p` is a continuous signal (its haps have `whole === undefined`). The scheduler
 *  uses this to choose per-note sampling vs a ParamRamp, and to refuse a signal as a
 *  top-level (untriggerable) track pattern. */
export function isContinuousPattern(p: Pat<unknown>): boolean {
  const haps = queryArc(p, 0, 1);
  return haps.length > 0 && haps.every((h) => h.whole === undefined);
}
