// Sample editing methods (Phase 4, feature 04 + 03): the producer-facing edit layer added to
// Pat<ControlMap> the same way sound.ts adds .gain/.lpf/etc. Two groups:
//
//   • Decoupled stretch/pitch (03): .stretch / .pitch — resolved OFFLINE (WSOLA) by preparing a
//     variant buffer and remapping each hap onto it (see prepareVariant in load.ts).
//   • Chop / slice / loop / reverse / region + granular (04): pure pattern transforms that compute
//     a (begin, end) region per hap and ride the existing TrigSample opcode — no new voice. The
//     unifying idea: a slice is just a TrigSample with a start offset and a region length. Granular
//     sets grain params the scheduler compiles into a TrigGrain (engine-side grain cloud).
//
// Everything returns an ordinary Pattern, so the whole combinator vocabulary composes:
//   sample("amen").chop(8).rev().jux(x => x.speed(1.5)).every(4, fast(2))

import { Pat, pattern, queryArc, spanIntersect, type Hap, type TimeSpan } from "../pattern/pattern";
import type { ControlMap, GrainOpts } from "../pattern/control";
import { parseMini } from "../pattern/mini";
import { rev as revPat } from "../pattern/combinators";
import { prepareVariant } from "./load";

declare module "../pattern/pattern" {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface Pat<T> {
    /** Transpose by `semitones` at constant duration (offline pitch-shift). */
    pitch(semitones: number): Pat<ControlMap>;
    /** Scale duration by `factor` at constant pitch (offline time-stretch). `2` = twice as long. */
    stretch(factor: number): Pat<ControlMap>;
    /** Region start as a 0..1 fraction of the buffer. */
    begin(frac: number): Pat<ControlMap>;
    /** Region end as a 0..1 fraction of the buffer. */
    end(frac: number): Pat<ControlMap>;
    /** Split the buffer into `n` equal slices laid out in order across the cycle. */
    chop(n: number): Pat<ControlMap>;
    /** Split into `n` slices; an index pattern (`"0 3 1 2"` or a `Pat<number>`) picks the order. */
    slice(n: number, idx: string | Pat<number>): Pat<ControlMap>;
    /** Loop the region instead of playing it once (held beds, stutters). */
    loop(): Pat<ControlMap>;
    /** Granular cloud over the buffer (textures, pseudo-stretch). */
    grain(opts: GrainOpts): Pat<ControlMap>;
  }
}

// ── Stretch / pitch (feature 03) ──────────────────────────────────────────────

/** Resolve a stretch/pitch edit: prepare a variant buffer for each distinct sample id in the
 *  pattern's first cycle, then remap haps onto those variants. Non-sample haps pass through. */
function applyVariant(pat: Pat<ControlMap>, stretch: number, semitones: number): Pat<ControlMap> {
  const remap = new Map<number, { id: number; frames: number }>();
  for (const h of queryArc(pat, 0, 1)) {
    const sid = h.value.sampleId;
    if (sid != null && !remap.has(sid)) {
      const v = prepareVariant(sid, stretch, semitones);
      remap.set(sid, { id: v.id, frames: v.frames });
    }
  }
  return pat.fmap((cm) => {
    if (cm.sampleId == null) return cm;
    const v = remap.get(cm.sampleId);
    return v ? { ...cm, sampleId: v.id, _frames: v.frames } : cm;
  });
}

// ── Chop / slice (feature 04) ─────────────────────────────────────────────────

const numOr = (v: ControlMap["begin"], dflt: number): number =>
  typeof v === "number" ? v : dflt;

/** Split each hap into `n` consecutive slices, subdividing both its time span and its
 *  [begin,end) region. Query-safe: each sub-slice is intersected with the queried part, so
 *  onsets (part.begin === whole.begin) survive look-ahead window clipping. */
function chop(pat: Pat<ControlMap>, n: number): Pat<ControlMap> {
  if (n < 1) return pat;
  return pattern((span) =>
    pat.query(span).flatMap((h): Hap<ControlMap>[] => {
      if (!h.whole) return [h]; // continuous — leave alone
      const w = h.whole;
      const dur = w.end - w.begin;
      const b0 = numOr(h.value.begin, 0);
      const e0 = numOr(h.value.end, 1);
      const out: Hap<ControlMap>[] = [];
      for (let k = 0; k < n; k++) {
        const whole: TimeSpan = { begin: w.begin + (k / n) * dur, end: w.begin + ((k + 1) / n) * dur };
        const part = spanIntersect(whole, h.part);
        if (!part) continue;
        const begin = b0 + (k / n) * (e0 - b0);
        const end = b0 + ((k + 1) / n) * (e0 - b0);
        out.push({ whole, part, value: { ...h.value, begin, end } });
      }
      return out;
    }),
  );
}

const asIndexPattern = (idx: string | Pat<number>): Pat<number> =>
  typeof idx === "string" ? parseMini(idx).fmap((tok) => parseInt(tok, 10)) : idx;

/** The sample control map at cycle 0 (sampleId, _frames, plus any chained gain/speed/…). */
function firstSampleValue(pat: Pat<ControlMap>): ControlMap {
  const haps = queryArc(pat, 0, 1);
  return (haps.find((h) => h.value?.sampleId != null) ?? haps[0])?.value ?? {};
}

/** Divide the buffer into `n` equal regions; the index pattern chooses which plays each step. */
function slice(pat: Pat<ControlMap>, n: number, idx: string | Pat<number>): Pat<ControlMap> {
  const base = firstSampleValue(pat);
  return asIndexPattern(idx).fmap((k): ControlMap => {
    const i = (((Math.round(k) % n) + n) % n) || 0;
    return { ...base, begin: i / n, end: (i + 1) / n };
  });
}

// ── Method install ────────────────────────────────────────────────────────────

let installed = false;
export function installSampleEditMethods(): void {
  if (installed) return;
  installed = true;
  const proto = Pat.prototype;

  proto.pitch = function (this: Pat<ControlMap>, semitones: number) {
    return applyVariant(this, 1, semitones);
  };
  proto.stretch = function (this: Pat<ControlMap>, factor: number) {
    return applyVariant(this, factor, 0);
  };
  proto.begin = function (this: Pat<ControlMap>, frac: number) {
    return this.fmap((cm) => ({ ...cm, begin: frac }));
  };
  proto.end = function (this: Pat<ControlMap>, frac: number) {
    return this.fmap((cm) => ({ ...cm, end: frac }));
  };
  proto.chop = function (this: Pat<ControlMap>, n: number) {
    return chop(this, n);
  };
  proto.slice = function (this: Pat<ControlMap>, n: number, idx: string | Pat<number>) {
    return slice(this, n, idx);
  };
  proto.loop = function (this: Pat<ControlMap>) {
    return this.fmap((cm) => ({ ...cm, loop: true }));
  };
  proto.grain = function (this: Pat<ControlMap>, opts: GrainOpts) {
    return this.fmap((cm) => ({ ...cm, grain: opts }));
  };

  // Override the generic .rev(): reverse hap order AND, for sample haps, flip the per-slice audio
  // direction so a chopped break reverses end-to-end (the producer-expected "reverse the break").
  // Non-sample patterns (numbers, synth notes) are reordered exactly as before.
  proto.rev = function <T>(this: Pat<T>): Pat<T> {
    const flipped = this.fmap((v) => {
      if (v && typeof v === "object") {
        const cm = v as ControlMap;
        if (cm.sampleId != null) return { ...cm, rev: !cm.rev } as unknown as T;
      }
      return v;
    });
    return revPat(flipped);
  };
}

installSampleEditMethods();
