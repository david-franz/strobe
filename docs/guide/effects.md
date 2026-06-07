[« strobe user guide](../guide.md)

# Effects (the rack)

Each track has its own **effect chain** — a fixed set of inserts processed in a sensible order —
plus two **shared send buses** (a reverb and a delay) that every track can feed. You don't build
the rack; it's always there. An effect simply **wakes up when you first use it** and sleeps (costs
nothing) when you don't. Every parameter is **smoothed**, so setting or sweeping one glides rather
than clicking, and any of them can be **automated by a signal** (see below).

## Filter & EQ — shape the whole track

| Method | Effect |
| --- | --- |
| `.lpf(hz)` / `.hpf(hz)` / `.bpf(hz)` | the **performance filter** — low/high/band-pass on the whole track |
| `.resonance(q)` (alias `.res`) | filter resonance / Q (`~0.5..20`; high Q = whistle/acid) |
| `.eqLow(db)` / `.eqMid(db)` / `.eqHigh(db)` | 3-band EQ shelf/peak gains in dB (`0` = flat) |

This filter sits on the *summed* track (distinct from each synth voice's own filter), so it's the
one you *play*: `p3(note("c2*8").saw().lpf(sine.range(300,1400).slow(8)))` sweeps the **whole**
bassline over 8 cycles. Pair a low cutoff with high `.resonance` for the classic acid squelch.

## Character — grit & lo-fi

| Method | Effect |
| --- | --- |
| `.dist(drive)` | saturation / overdrive (tanh by default — smooth at low drive, gnarly high) |
| `.crush(bits)` | bit-depth reduction (`1`=fuzzy … `16`=clean) — grimy digital fuzz |
| `.downsample(factor)` | sample-rate decimation (lo-fi aliasing; `1` = off) |

The staples of techno/hip-hop drums: `p1(sound("bd sn").crush(8).dist(3))`.

## Modulation — movement & width

| Method | Effect |
| --- | --- |
| `.chorus(depth)` | LFO-modulated short delays + dry → ensemble width/shimmer |
| `.flanger(depth)` | a single short modulated delay **with feedback** → the comb-filter "whoosh" |
| `.phaser(depth)` | swept all-pass notches → swirling sweep, no pitch wobble |

`depth` is `0..1` (default ~0.5–0.6). Reach for chorus to thicken keys/pads, phaser for slow
evolving sweeps on a saw line.

## Dynamics & the pump

| Method | Effect |
| --- | --- |
| `.comp(ratio)` | a compressor (default threshold/attack/release) — glue and punch |
| `.sidechain("p1")` | **the techno pump** — duck this track whenever track `p1` (the kick) hits |

`.sidechain("p1")` is the headline move: the bass/pads dip on every kick and breathe back, the
genre's signature rhythmic motion. Put it on the bass and feed it the kick track:
`p3(note("c2*8").saw().sidechain("p1"))`.

## Sends — shared space

| Method | Effect |
| --- | --- |
| `.room(amount)` | feed this track into the shared **reverb** (`amount` = how much) |
| `.delay(amount)` | feed this track into the shared **ping-pong delay** |

Sends are shared, so many tracks bloom into *one* reverb tail (cheap and cohesive) rather than a
separate reverb each. `.room(0.3)` is a tasteful wash; push it for a cavern.

## Automating an effect (signals)

Any effect parameter takes a **signal**, and a continuous signal on a filter/effect param is sent
as a smooth **audio-rate ramp** so it glides between notes, not just at onsets:

```ts
p3( note("c2*8").saw().lpf( sine.range(300,1200).slow(8) ).sidechain("p1") ); // swept + pumping
p2( sound("hh*8").crush( saw.range(12,4).slow(4) ) );                         // grit increasing
```

> **Order & scope.** The chain runs in a fixed order (filter → EQ → distortion → bitcrush →
> modulation → compressor → sidechain), not the order you write the methods — a sensible default,
> not authoring order. Removing an effect from a live pattern doesn't switch it off this version;
> use `hush()` or keep it in the pattern. Per-effect fine controls (delay time, reverb size, LFO
> rate) use built-in defaults for now.

---

Prev: [« Signals](signals.md) · Next: [Sampling »](sampling.md)
