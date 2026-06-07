// The sound/note/freq API (pattern-model.md "the chaining API"): turn a mini-notation
// string into a Pat<ControlMap>, and build that control map up one key at a time with the
// chainable modifier methods. Each modifier is an `fmap` that merges a key — pure
// value-mapping, never touching time, so combinators and modifiers compose freely.

import { Pat, pattern } from "./pattern";
import { parseMini } from "./mini";
import { parseNote } from "./control";
import type { ControlMap } from "./control";
import { fast } from "./combinators";

type Val = number | string | Pat<number>;

// ── Constructors ──────────────────────────────────────────────────────────────

/** "bd*4" → Pat<ControlMap> with value { s: token } per hap. */
export const sound = (str: string): Pat<ControlMap> => parseMini(str).fmap((tok) => ({ s: tok }));
export const s = sound;

/** "c2 eb2 g2" → Pat<ControlMap> with value { note: <semitone> }. */
export const note = (str: string): Pat<ControlMap> =>
  parseMini(str).fmap((tok) => ({ note: parseNote(tok) }));

/** "220 330" → Pat<ControlMap> with value { freq: <Hz> }. */
export const freq = (str: string): Pat<ControlMap> =>
  parseMini(str).fmap((tok) => ({ freq: parseFloat(tok) }));

// ── Value-aware combinators (need ControlMap) ─────────────────────────────────

/** Split to stereo: original hard-left, `fn`-transformed copy hard-right. */
export const jux = (
  fn: (p: Pat<ControlMap>) => Pat<ControlMap>,
  p: Pat<ControlMap>,
): Pat<ControlMap> =>
  pattern((span) => [
    ...p.fmap((v) => ({ ...v, pan: 0 })).query(span),
    ...fn(p)
      .fmap((v) => ({ ...v, pan: 1 }))
      .query(span),
  ]);

/** `fast(n)` plus raising playback speed/pitch by `n`. */
export const hurry = (n: number, p: Pat<ControlMap>): Pat<ControlMap> =>
  fast(n, p).fmap((v) => ({ ...v, speed: (typeof v.speed === "number" ? v.speed : 1) * n }));

// ── Modifier methods (mixed onto Pat.prototype) ───────────────────────────────

declare module "./pattern" {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface Pat<T> {
    gain(v: Val): Pat<ControlMap>;
    pan(v: Val): Pat<ControlMap>;
    speed(v: Val): Pat<ControlMap>;
    lpf(v: Val): Pat<ControlMap>;
    hpf(v: Val): Pat<ControlMap>;
    bpf(v: Val): Pat<ControlMap>;
    resonance(v: Val): Pat<ControlMap>;
    res(v: Val): Pat<ControlMap>;
    eqLow(v: Val): Pat<ControlMap>;
    eqMid(v: Val): Pat<ControlMap>;
    eqHigh(v: Val): Pat<ControlMap>;
    dist(v: Val): Pat<ControlMap>;
    crush(v: Val): Pat<ControlMap>;
    downsample(v: Val): Pat<ControlMap>;
    room(v: Val): Pat<ControlMap>;
    delay(v: Val): Pat<ControlMap>;
    note(v: number): Pat<ControlMap>;
    saw(): Pat<ControlMap>;
    square(): Pat<ControlMap>;
    sine(): Pat<ControlMap>;
    tri(): Pat<ControlMap>;
    adsr(a: number, d: number, sus: number, r: number): Pat<ControlMap>;
    sidechain(track: string): Pat<ControlMap>;
    jux(fn: (p: Pat<ControlMap>) => Pat<ControlMap>): Pat<ControlMap>;
    hurry(n: number): Pat<ControlMap>;
  }
}

const merge =
  (key: string) =>
  function (this: Pat<ControlMap>, v: Val): Pat<ControlMap> {
    return this.fmap((cm) => ({ ...cm, [key]: v }));
  };

const setWave = (wave: string, keepExisting = false) =>
  function (this: Pat<ControlMap>): Pat<ControlMap> {
    return this.fmap((cm) => ({ ...cm, s: keepExisting ? cm.s ?? wave : wave }));
  };

let installed = false;
export function installSoundMethods(): void {
  if (installed) return;
  installed = true;
  const proto = Pat.prototype;
  proto.gain = merge("gain");
  proto.pan = merge("pan");
  proto.speed = merge("speed");
  proto.lpf = merge("lpf");
  proto.hpf = merge("hpf");
  proto.bpf = merge("bpf");
  proto.resonance = merge("resonance");
  proto.res = merge("resonance");
  proto.eqLow = merge("eqLow");
  proto.eqMid = merge("eqMid");
  proto.eqHigh = merge("eqHigh");
  proto.dist = merge("dist");
  proto.crush = merge("crush");
  proto.downsample = merge("downsample");
  proto.room = merge("room");
  proto.delay = merge("delay");
  proto.note = function (this: Pat<ControlMap>, v: number) {
    return this.fmap((cm) => ({ ...cm, note: v }));
  };
  proto.saw = setWave("saw", true);
  proto.square = setWave("square");
  proto.sine = setWave("sine");
  proto.tri = setWave("tri");
  proto.adsr = function (this: Pat<ControlMap>, a: number, d: number, sus: number, r: number) {
    return this.fmap((cm) => ({ ...cm, attack: a, decay: d, sustain: sus, release: r }));
  };
  proto.sidechain = function (this: Pat<ControlMap>, track: string) {
    return this.fmap((cm) => ({ ...cm, sidechain: track }));
  };
  proto.jux = function (this: Pat<ControlMap>, fn: (p: Pat<ControlMap>) => Pat<ControlMap>) {
    return jux(fn, this);
  };
  proto.hurry = function (this: Pat<ControlMap>, n: number) {
    return hurry(n, this);
  };
}

installSoundMethods();
