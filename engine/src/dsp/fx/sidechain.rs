//! The cross-track sidechain bus — the one piece of cross-track routing in the rack. Each
//! track's pre-fader level is tapped into a fixed-size, single-writer-per-track buffer the
//! render loop fills *before* any duck reads it (lock-free, allocation-free). A
//! `SidechainDuck` slot (feature 06) reads `level[source][i]` per sample.

use super::{MAX_BLOCK, MAX_TRACKS};

/// A peak envelope follower with asymmetric attack/release. Shared by the sidechain tap and
/// the compressor (feature 06).
pub struct EnvelopeFollower {
    env: f32,
    atk: f32,
    rel: f32,
}

impl EnvelopeFollower {
    pub fn new(atk_ms: f32, rel_ms: f32, sr: f32) -> Self {
        let mut e = Self { env: 0.0, atk: 0.0, rel: 0.0 };
        e.set_times(atk_ms, rel_ms, sr);
        e
    }
    pub fn set_times(&mut self, atk_ms: f32, rel_ms: f32, sr: f32) {
        self.atk = (-1.0 / (atk_ms.max(0.01) * 0.001 * sr)).exp();
        self.rel = (-1.0 / (rel_ms.max(0.01) * 0.001 * sr)).exp();
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let rect = x.abs();
        let coef = if rect > self.env { self.atk } else { self.rel };
        self.env = coef * self.env + (1.0 - coef) * rect;
        self.env
    }
    pub fn reset(&mut self) {
        self.env = 0.0;
    }
}

/// Per-track source level for the current block. Indexed by track id (0..=MAX_TRACKS).
pub struct SidechainBus {
    level: Vec<Vec<f32>>,
    env: Vec<EnvelopeFollower>,
}

impl SidechainBus {
    pub fn new(sr: f32) -> Self {
        Self {
            level: (0..=MAX_TRACKS).map(|_| vec![0.0; MAX_BLOCK]).collect(),
            // a fast attack / medium release so the tap follows the kick transient.
            env: (0..=MAX_TRACKS).map(|_| EnvelopeFollower::new(1.0, 100.0, sr)).collect(),
        }
    }

    /// Run the follower over a source track's block and store its per-sample level.
    pub fn tap(&mut self, track: usize, l: &[f32], r: &[f32]) {
        if track > MAX_TRACKS {
            return;
        }
        let env = &mut self.env[track];
        let lv = &mut self.level[track];
        for i in 0..l.len().min(MAX_BLOCK) {
            lv[i] = env.process(0.5 * (l[i].abs() + r[i].abs()));
        }
    }

    /// The per-sample source level for `track` this block (a plain read, no sync).
    #[inline]
    pub fn level(&self, track: usize) -> &[f32] {
        &self.level[track.min(MAX_TRACKS)]
    }

    /// Zero a track's stored level (e.g. when it produced no audio this block).
    pub fn clear(&mut self, track: usize, n: usize) {
        if track <= MAX_TRACKS {
            for v in &mut self.level[track][..n.min(MAX_BLOCK)] {
                *v = 0.0;
            }
            self.env[track].reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SR: f32 = 48_000.0;

    #[test]
    fn follower_rises_on_signal_and_falls_on_silence() {
        let mut f = EnvelopeFollower::new(1.0, 50.0, SR);
        for _ in 0..2000 {
            f.process(1.0);
        }
        let loud = f.process(1.0);
        assert!(loud > 0.8, "tracks a loud signal ({loud})");
        for _ in 0..10000 {
            f.process(0.0);
        }
        assert!(f.process(0.0) < 0.1, "decays on silence");
    }

    #[test]
    fn tap_stores_per_track_level() {
        let mut bus = SidechainBus::new(SR);
        let loud = vec![1.0f32; 256];
        bus.tap(1, &loud, &loud);
        assert!(bus.level(1)[100] > 0.0, "track 1 has level");
        assert_eq!(bus.level(2)[100], 0.0, "untapped track 2 stays 0");
    }
}
