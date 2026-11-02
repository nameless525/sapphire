//! Where a cue's variation comes from: randomness the caller owns, so a test can pin every pick.
//!
//! Sound is presentation, so its picks must never come from the simulation's stream: a cue fired
//! from gameplay would move every later roll, and a replay's checksum with it. The app keeps a
//! source of its own, a [`CueRng`] or anything else that gives numbers.

/// Gives the numbers a cue's picks are made from.
pub trait CueRandom {
    /// The next number, uniform over all of `u32`.
    fn next_u32(&mut self) -> u32;

    /// The next number, uniform in `0..1`.
    fn next_unit(&mut self) -> f32 {
        // 24 bits are all an f32 keeps below 1.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// A small seeded source (SplitMix64): the same seed gives the same picks on every platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueRng(u64);

impl CueRng {
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl CueRandom for CueRng {
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((mixed ^ (mixed >> 31)) >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_pins_the_sequence() {
        // SplitMix64's published first outputs for seed 0, upper halves.
        let mut rng = CueRng::new(0);
        assert_eq!(rng.next_u32(), 0xe220a839);
        assert_eq!(rng.next_u32(), 0x6e789e6a);
        assert_ne!(CueRng::new(1).next_u32(), CueRng::new(2).next_u32());
    }

    #[test]
    fn units_stay_below_one() {
        struct Ones;
        impl CueRandom for Ones {
            fn next_u32(&mut self) -> u32 {
                u32::MAX
            }
        }
        let unit = Ones.next_unit();
        assert!(unit < 1.0, "{unit}");
        let mut rng = CueRng::new(7);
        let mean = (0..4096).map(|_| rng.next_unit()).sum::<f32>() / 4096.0;
        assert!((mean - 0.5).abs() < 0.03, "{mean}");
    }
}
