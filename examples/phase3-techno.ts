// Phase 3 demo — the effects rack. The README techno loop, now fully audible: the performance
// filter sweeps the bass, the reverb/delay sends bloom, and the bass PUMPS against the kick.
//
//   pnpm build:native      # (re)build the engine addon so the new opcodes are decoded
//   pnpm demo:phase3
// then edit any line and save — the swap lands at the next cycle boundary, no dropout.
//
// What it exercises (Phase 3):
//   • performance filter + smooth automation (02/07): .lpf(sine.range(...).slow(8))
//   • send buses (03): .room(x) → shared reverb, .delay(x) → shared ping-pong delay
//   • distortion (04): .dist(x); modulation (05): .phaser/.chorus
//   • dynamics (06): .comp(ratio) and the headline .sidechain("p1") pump

import { setcps, p1, p2, p3, p4, sound, note, sine } from "strobe";

setcps(130 / 60 / 4); // ~130 BPM, 4 beats per cycle

p1(sound("bd*4").gain(0.95).comp(4)); // four-on-the-floor kick, lightly compressed for punch

p2(sound("~ hh ~ hh").gain(0.5).hpf(1200).delay(0.18)); // offbeat hats, into the delay send

p3(
  note("c2 c2 eb2 g2") // rolling bassline…
    .saw()
    .lpf(sine.range(300, 1400).slow(8)) // …with an audible 8-cycle filter sweep
    .resonance(6) // a little squelch
    .room(0.3) // reverb send
    .sidechain("p1"), // THE PUMP — ducked by the kick
);

p4(
  note("<c4 eb4 g4 bb4>") // a sparse chord stab per cycle
    .saw()
    .gain(0.35)
    .dist(3) // grit
    .phaser(0.7) // sweeping notches
    .room(0.45), // washed into the reverb
);

// Try while it plays:
//   p3( note("c2*8").saw().lpf(sine.range(200,2000).slow(4)).sidechain("p1") );
//   p2( sound("hh*8?").gain(0.5).crush(6) );          // bit-crushed offbeat hats
//   p4( note("c4 eb4 g4").saw().chorus(0.8).room(0.5) ); // lush chorus pad
