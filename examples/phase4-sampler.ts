// Phase 4 demo — sampling. We synthesize a short 8-hit "break" in TS (so the demo is
// self-contained — no audio file to download), load it as a sample, then chop / re-sequence /
// reverse / stretch / pitch / granulate it. This is the heart of hip-hop production.
//
//   pnpm build:native    # (re)build so the TrigSample/TrigGrain opcodes are decoded
//   pnpm demo:phase4
// then edit any line and save — the swap lands at the next cycle boundary, no dropout.
//
// What it exercises (Phase 4):
//   • loadPcm + sample() (01/02): a loaded buffer triggered as a one-shot voice
//   • .chop(n) / .slice(n, "...") (04): slice the break, then re-sequence the slices
//   • .rev() (04): reverse the break end-to-end (slice order AND each slice's audio)
//   • .stretch(x) / .pitch(n) (03): decoupled time-stretch + pitch-shift (offline WSOLA)
//   • .grain({...}) (04): an engine-side granular cloud (texture / pseudo-stretch)
//   • it all composes with Phases 1–3: .gain/.lpf/.hpf/.room/.every/.jux …
//
// YouTube ingest (opt-in) is shown at the very bottom — local/synth samples are the always-safe
// default and need no extra tools.

import { setcps, p1, p2, p3, p4, sample, loadPcm, sound } from "strobe";

setcps(140 / 60 / 4); // ~140 BPM, 4 beats/cycle — boom-bap territory

// ── Synthesize an 8-hit "break" (2 s, stereo, 48 kHz) so .chop(8) lands on 8 clear slices ──
const SR = 48000;
const HITS = 8;
const SEG = Math.floor(SR / 4); // 0.25 s per hit
const frames = HITS * SEG;
const pcm = new Float32Array(frames * 2); // interleaved stereo
const contour = [0, 3, 5, 7, 10, 7, 5, 3]; // a little melodic shape across the 8 slices
for (let h = 0; h < HITS; h++) {
  const f0 = 110 * 2 ** (contour[h] / 12); // pitch of this hit
  for (let i = 0; i < SEG; i++) {
    const t = i / SR;
    const env = Math.exp(-t * 16); // percussive decay
    const attack = i < 240 ? (Math.random() * 2 - 1) * Math.exp(-i / 50) * 0.5 : 0; // noisy transient
    const s = Math.sin(2 * Math.PI * f0 * t) * env * 0.7 + attack;
    const j = (h * SEG + i) * 2;
    pcm[j] = s;
    pcm[j + 1] = s;
  }
}
const brk = sample(loadPcm(pcm, 2, SR, { name: "break", key: "phase4-break" }));

// ── The beat ──
p1(sound("bd*4").gain(0.95)); // four-on-the-floor kick under it all

p2(brk.chop(8).gain(0.7).room(0.12)); // the break, chopped into 8 slices in order (the classic move)

p3(
  brk
    .slice(8, "0 5 2 7 4 1 6 3") // re-sequence the slices into a new groove…
    .gain(0.5)
    .hpf(300)
    .every(4, (x) => x.rev()), // …flipped end-to-end every 4th cycle
);

p4(
  brk
    .begin(0.5) // hover the cloud around the middle of the break
    .grain({ size: 70, density: 22, scatter: 0.25, spread: 0.7 }) // a shimmering granular bed
    .gain(0.32)
    .lpf(1600)
    .room(0.4),
);

// Try while it plays (edit, save, hear it at the next boundary):
//   p2( brk.chop(8).jux(x => x.rev()) );                       // stereo: forward left, reversed right
//   p3( brk.chop(4).pitch(-5).stretch(1.5).gain(0.5).room(0.3) ); // detuned + time-stretched chops
//   p4( brk.chop(16).slice(16, "0 0 8 8 4 4 12 12").loop() );  // stutter-looped re-sequence
//
// YouTube ingest (opt-in — mind YouTube's ToS + copyright; local/synth samples are the safe default):
//   import { enableYouTubeSampling, sample } from "strobe";
//   enableYouTubeSampling({ iUnderstandToS: true });
//   const vox = await sample.fromYouTube("https://youtu.be/…", { start: 42, dur: 4 });
//   p2( vox.chop(8).rev().room(0.2) );
