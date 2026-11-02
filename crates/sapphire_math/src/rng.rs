//! Seeded randomness for the simulation (spec §5.4).
//!
//! [`Pcg32`] is PCG-XSH-RR 64/32, the family the spec names, written out in a few lines rather
//! than taken from `rand_pcg`: nothing else in the runtime needs `rand_core`'s traits, and the
//! simulation wants a plain value it can copy, compare and checksum. It reads no OS entropy and
//! keeps no global state, so a seed and a stream number give one sequence on every target: integer
//! arithmetic only, with the floats built from whole bits. The reference implementation's own
//! output is a test.
//!
//! Give each subsystem and encounter its own stream (spec §5.4), so one drawing more numbers
//! doesn't move what another gets.

/// PCG's 64-bit multiplier, from the reference implementation.
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// 2⁻²⁴: turns a whole number below 2²⁴, which an `f32` holds exactly, into [0, 1).
const UNIT_FROM_24_BITS: f32 = 1.0 / (1u32 << 24) as f32;

/// A small seeded random number generator. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pcg32 {
    state: u64,
    /// Odd: which of the 2⁶³ sequences this is.
    increment: u64,
}

impl Pcg32 {
    /// The generator for `seed` on sequence `stream`, as the reference's `pcg32_srandom_r` seeds it.
    #[must_use]
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Self { state: 0, increment: (stream << 1) | 1 };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// The next 32 random bits.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.increment);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        xorshifted.rotate_right((old >> 59) as u32)
    }

    /// A whole number in `[0, bound)`, unbiased, as the reference's `pcg32_boundedrand_r` gives it.
    /// 0 for a `bound` of 0, drawing nothing.
    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        // Numbers below 2³² % bound would make the low results likelier. Almost none are.
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let value = self.next_u32();
            if value >= threshold {
                return value % bound;
            }
        }
    }

    /// A float in [0, 1), from 24 random bits: exact, so no rounding differs between targets.
    pub fn unit_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * UNIT_FROM_24_BITS
    }

    /// The whole state, `[state, increment]`, for state checksums.
    #[must_use]
    pub fn state(&self) -> [u64; 2] {
        [self.state, self.increment]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first numbers of the reference implementation's `pcg32-demo` (seed 42, sequence 54).
    #[test]
    fn matches_the_reference_implementation() {
        let mut rng = Pcg32::new(42, 54);
        let numbers: [u32; 6] = std::array::from_fn(|_| rng.next_u32());
        assert_eq!(numbers, [0xa15c_02b7, 0x7b47_f409, 0xba1d_3330, 0x83d2_f293, 0xbfa4_784b, 0xcbed_606e]);
    }

    #[test]
    fn seeds_and_streams_give_their_own_sequences() {
        let first = |seed, stream| Pcg32::new(seed, stream).next_u32();
        assert_ne!(first(1, 0), first(2, 0));
        assert_ne!(first(1, 0), first(1, 1));
        assert_eq!(first(1, 0), first(1, 0));
    }

    #[test]
    fn state_captures_the_position_in_the_sequence() {
        let mut rng = Pcg32::new(7, 3);
        let before = rng.state();
        let copy = rng;
        rng.next_u32();
        assert_ne!(rng.state(), before);
        assert_eq!(copy.state(), before);
        assert_eq!(Pcg32::new(7, 3).state(), before);
    }

    #[test]
    fn bounded_numbers_stay_in_range_and_cover_it() {
        let mut rng = Pcg32::new(9, 1);
        let mut seen = [0u32; 7];
        for _ in 0..7000 {
            seen[rng.below(7) as usize] += 1;
        }
        // Each of 7 values about 1000 times, by any fair generator's spread.
        assert!(seen.iter().all(|&count| (850..1150).contains(&count)), "{seen:?}");
        assert_eq!(rng.below(1), 0);
        let before = rng.state();
        assert_eq!(rng.below(0), 0);
        assert_eq!(rng.state(), before, "a bound of 0 draws nothing");
    }

    #[test]
    fn units_stay_below_one_and_are_exact() {
        let mut rng = Pcg32::new(11, 2);
        let mut sum = 0.0;
        for _ in 0..10_000 {
            let unit = rng.unit_f32();
            assert!((0.0..1.0).contains(&unit), "{unit}");
            // 24 bits fit an f32's mantissa, so scaling back up is a whole number.
            assert_eq!((unit * (1u32 << 24) as f32).fract(), 0.0);
            sum += unit;
        }
        assert!((sum / 10_000.0 - 0.5).abs() < 0.02, "{sum}");
    }
}
