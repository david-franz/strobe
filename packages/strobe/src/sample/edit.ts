// Sample editing methods (Phase 4, feature 03): decoupled time-stretch and pitch-shift, added
// to Pat<ControlMap> the same way sound.ts adds .gain/.lpf/etc.
//
// Unlike .speed (which retunes by resampling — faster = higher) these are independent: .stretch
// changes duration at constant pitch, .pitch transposes at constant duration. Both are done
// OFFLINE in the engine (WSOLA), so they can't run on the audio thread. We resolve them eagerly:
// at chain-build time we query the pattern's first cycle, ask the engine to prepare a variant
// buffer for each distinct sample id, then fmap each hap's sampleId/_frames onto its variant.
// The prepare is cached per (id, stretch, pitch) (see load.ts), so live-coding reloads reuse the
// already-stretched buffer instead of re-running WSOLA every edit.

import { Pat, queryArc } from "../pattern/pattern";
import type { ControlMap } from "../pattern/control";
import { prepareVariant } from "./load";

declare module "../pattern/pattern" {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface Pat<T> {
    /** Transpose by `semitones` at constant duration (offline pitch-shift). */
    pitch(semitones: number): Pat<ControlMap>;
    /** Scale duration by `factor` at constant pitch (offline time-stretch). `2` = twice as long. */
    stretch(factor: number): Pat<ControlMap>;
  }
}

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
}

installSampleEditMethods();
