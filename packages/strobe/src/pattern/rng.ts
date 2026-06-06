// Deterministic, position-seeded RNG shared by the chance combinators (degrade, sometimes,
// somecycles) and the rand/perlin signals. Randomness must be a pure hash of position —
// never a stateful generator — so querying the same arc twice yields the same result
// (pattern-model.md "why pure functions"). Seed from the integer cycle + a hap index + a
// per-call salt.

/** 32-bit integer hash → float in [0, 1). Pure: same input ⇒ same output. */
export function hashFloat(seed: number): number {
  let x = seed | 0;
  x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
  x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
  x = (x ^ (x >>> 16)) >>> 0;
  return x / 0x100000000;
}

// Distinct salts keep different combinators from correlating.
export const SALT_DEGRADE = 0x1a2b3c4d;
export const SALT_SOMETIMES = 0x5e6f7a8b;
export const SALT_SOMECYCLES = 0x9c0d1e2f;

/** Combine a cycle, a hap index within the cycle, and a per-call salt into a draw. */
export const randAt = (cycle: number, index: number, salt: number): number =>
  hashFloat(Math.imul(cycle, 0x9e3779b1) ^ Math.imul(index, 0x85ebca6b) ^ salt);
