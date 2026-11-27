//! The effects' random numbers: PCG32 (O'Neill 2014), small and with no dependency. Seeded, so an
//! effect's particles land the same way in every run, which golden images need. Presentation
//! only: gameplay's checksum never reads it.

/// A PCG32 generator (XSH-RR, one stream).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rng {
    state: u64,
}

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
const INCREMENT: u64 = 1_442_695_040_888_963_407;
/// What sets a fork's state apart from its parent's.
const FORK: u64 = 0x666F_726B_6564;

impl Rng {
    /// A generator from `seed` and `stream`, which keep two effects spawned from one seed apart:
    /// the world's seed and how many effects it has spawned.
    pub(crate) fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Self { state: seed.wrapping_add(INCREMENT) ^ stream.wrapping_mul(MULTIPLIER) };
        rng.next_u32();
        rng.next_u32();
        rng
    }

    /// A generator of its own, from where this one is and `salt`: this one isn't advanced, so what it draws
    /// next is what it would have drawn without the fork (M6 N2, an emitter's forces).
    pub(crate) fn fork(&self, salt: u64) -> Self {
        Self::new(self.state ^ FORK, salt)
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        let state = self.state;
        self.state = state.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT);
        let xorshifted = (((state >> 18) ^ state) >> 27) as u32;
        xorshifted.rotate_right((state >> 59) as u32)
    }

    /// Uniform in [0, 1).
    pub(crate) fn unit(&mut self) -> f32 {
        // 24 bits fill an f32's mantissa, and stay below 1.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform between the range's ends.
    pub(crate) fn between(&mut self, (low, high): (f32, f32)) -> f32 {
        low + (high - low) * self.unit()
    }
}

/// SplitMix64's finalizer (Steele, Lea and Flood 2014): a key spread over all 64 bits, the same on every machine.
/// What a particle's key and an event's stream are made from (M6 N3), with no random number drawn.
pub(crate) fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_and_stream_give_one_sequence() {
        let sequence = |seed, stream| {
            let mut rng = Rng::new(seed, stream);
            [rng.next_u32(), rng.next_u32(), rng.next_u32()]
        };
        assert_eq!(sequence(7, 1), sequence(7, 1));
        assert_ne!(sequence(7, 1), sequence(7, 2));
        assert_ne!(sequence(7, 1), sequence(8, 1));
    }

    #[test]
    fn a_fork_leaves_its_parent_as_it_was() {
        let parent = Rng::new(7, 1);
        let mut fork = parent.fork(3);
        assert_eq!(parent, Rng::new(7, 1));
        let mut copy = parent;
        assert_ne!(fork.next_u32(), copy.next_u32());
        assert_ne!(parent.fork(3).next_u32(), parent.fork(4).next_u32());
        assert_eq!(parent.fork(3).next_u32(), parent.fork(3).next_u32());
    }

    #[test]
    fn units_stay_in_range_and_cover_it() {
        let mut rng = Rng::new(1, 1);
        let (mut low, mut high) = (1.0_f32, 0.0_f32);
        for _ in 0..4096 {
            let unit = rng.unit();
            assert!((0.0..1.0).contains(&unit));
            (low, high) = (low.min(unit), high.max(unit));
        }
        assert!(low < 0.01 && high > 0.99, "{low} to {high}");
    }

    #[test]
    fn between_spans_its_ends() {
        let mut rng = Rng::new(3, 9);
        for _ in 0..256 {
            let value = rng.between((2.0, 5.0));
            assert!((2.0..5.0).contains(&value));
        }
        assert_eq!(rng.between((4.0, 4.0)), 4.0);
    }
}
