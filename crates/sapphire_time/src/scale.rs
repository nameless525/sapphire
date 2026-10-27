//! Time-scale stacks shared by [`crate::ScaledClock`] and [`crate::LocalClock`].

use crate::SUBTICKS_PER_TICK;

/// The per-mille factor for normal speed (1×).
pub const NORMAL_SPEED: u32 = 1000;

/// Fixed-point unit for scale factors: `FACTOR_ONE` means 1×. Each per-mille factor divides it
/// exactly, so up to six stacked scales (world and local combined) multiply without rounding.
pub(crate) const FACTOR_ONE: u128 = 1_000_000_000_000_000_000;

/// Factors saturate here (10⁹×), which keeps every intermediate product within `u128`.
const FACTOR_MAX: u128 = FACTOR_ONE * 1_000_000_000;

/// Why a time scale was pushed; lets systems clear their own scales without handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScaleSource {
    /// The pause menu or app backgrounding (spec §6.5). Normally a factor of 0.
    Pause,
    /// Perfect-dodge slow motion on the world and the player's counter-scale (spec §4.5).
    PerfectDodge,
    /// Cutscenes and skill cinematics (spec §5.23).
    Cinematic,
    /// Abilities and effects that change an entity's speed, such as slow fields or haste.
    Gameplay,
    /// Developer time controls (spec §5.20).
    Debug,
}

/// One entry in a scale stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimeScale {
    /// Who pushed it.
    pub source: ScaleSource,
    /// Speed in thousandths: [`NORMAL_SPEED`] (1000) is 1×, 200 is 0.2×, 0 stops time.
    pub per_mille: u32,
}

/// Identifies one clock within a world: the world clock is always id 0, and each
/// [`crate::LocalClock`] gets the next id from [`crate::ScaledClock::new_local_clock`]. Ids
/// come from that per-world counter, never from global state, so they replay identically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockId(pub(crate) u32);

impl ClockId {
    /// The world clock's id.
    pub const WORLD: Self = Self(0);
}

/// Identifies a pushed scale so exactly that entry can be popped later, whatever was pushed
/// after it.
///
/// A handle remembers the clock that issued it. Popping it on any other clock (including a
/// clock created later for a respawned entity) returns `None` and fails a debug assertion,
/// because it is always a caller bug. Popping a handle whose scale is already gone (popped, or
/// removed with `remove_source`) quietly returns `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScaleHandle {
    clock: ClockId,
    serial: u32,
}

/// A clock's scale stack has no free slot. Stacks are fixed-size so pushing never allocates
/// (rule R6); hitting the limit almost always means a scale was pushed and never popped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("time-scale stack is full ({capacity} entries); a pushed scale was probably never popped")]
pub struct ScaleStackFull {
    /// The stack's fixed capacity.
    pub capacity: usize,
}

/// A fixed-capacity, insertion-ordered set of scales with their cached product.
///
/// Removal shifts later entries down and clears the freed slot, so equal stacks compare and
/// hash equal however they were built.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ScaleStack<const N: usize> {
    owner: ClockId,
    entries: [Option<(ScaleHandle, TimeScale)>; N],
    len: usize,
    /// Serial of the next handle. Serials are never reused (until 2³² pushes on one clock), so
    /// a stale handle cannot pop a newer scale on the same clock.
    next_serial: u32,
    factor: u128,
}

impl<const N: usize> ScaleStack<N> {
    pub(crate) const fn new(owner: ClockId) -> Self {
        Self { owner, entries: [None; N], len: 0, next_serial: 0, factor: FACTOR_ONE }
    }

    pub(crate) fn owner(&self) -> ClockId {
        self.owner
    }

    pub(crate) fn push(
        &mut self,
        source: ScaleSource,
        per_mille: u32,
    ) -> Result<ScaleHandle, ScaleStackFull> {
        let slot = self.entries.get_mut(self.len).ok_or(ScaleStackFull { capacity: N })?;
        let handle = ScaleHandle { clock: self.owner, serial: self.next_serial };
        self.next_serial = self.next_serial.wrapping_add(1);
        *slot = Some((handle, TimeScale { source, per_mille }));
        self.len += 1;
        self.factor = apply_per_mille(self.factor, per_mille);
        Ok(handle)
    }

    pub(crate) fn pop(&mut self, handle: ScaleHandle) -> Option<TimeScale> {
        debug_assert_eq!(
            handle.clock, self.owner,
            "popped a time-scale handle on a clock that did not issue it (wrong or respawned clock)"
        );
        if handle.clock != self.owner {
            return None;
        }
        let index =
            self.entries[..self.len].iter().position(|entry| entry.is_some_and(|(h, _)| h == handle))?;
        let (_, scale) = self.remove_at(index);
        self.recompute();
        Some(scale)
    }

    pub(crate) fn remove_source(&mut self, source: ScaleSource) -> usize {
        let mut removed = 0;
        let mut index = 0;
        while index < self.len {
            if self.entries[index].is_some_and(|(_, scale)| scale.source == source) {
                self.remove_at(index);
                removed += 1;
            } else {
                index += 1;
            }
        }
        if removed > 0 {
            self.recompute();
        }
        removed
    }

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = TimeScale> + '_ {
        self.entries[..self.len].iter().map(|entry| entry.expect("slots below len are occupied").1)
    }

    /// The product of every scale, as a fixed-point factor of [`FACTOR_ONE`].
    pub(crate) fn factor(&self) -> u128 {
        self.factor
    }

    fn remove_at(&mut self, index: usize) -> (ScaleHandle, TimeScale) {
        let entry = self.entries[index].take().expect("slots below len are occupied");
        self.entries[index..self.len].rotate_left(1);
        self.len -= 1;
        entry
    }

    /// Recomputes the product in push order, so the result never depends on removal history.
    fn recompute(&mut self) {
        self.factor = self.iter().fold(FACTOR_ONE, |factor, scale| apply_per_mille(factor, scale.per_mille));
    }
}

fn apply_per_mille(factor: u128, per_mille: u32) -> u128 {
    // `factor <= FACTOR_MAX` (1e27) times a `u32` stays below 4.3e36, well inside `u128`.
    (factor * u128::from(per_mille) / u128::from(NORMAL_SPEED)).min(FACTOR_MAX)
}

/// Multiplies two fixed-point factors exactly (rounding down), saturating at [`FACTOR_MAX`] only
/// when the true product exceeds it.
///
/// Normal speed and a stopped clock, the factors almost every clock has, return at once (review
/// FI-102): `1 × b` is `b` and `0 × b` is 0, saturated as [`combine_exact`] saturates them.
pub(crate) fn combine(a: u128, b: u128) -> u128 {
    if a == FACTOR_ONE {
        b.min(FACTOR_MAX)
    } else if b == FACTOR_ONE {
        a.min(FACTOR_MAX)
    } else if a == 0 || b == 0 {
        0
    } else {
        combine_exact(a, b)
    }
}

/// [`combine`] for any factors, the reference its shortcuts must match.
///
/// `a × b` needs up to 180 bits, so instead of a 256-bit multiply both factors are split at
/// `FACTOR_ONE`: with `a = a₁·ONE + a₀` and `b = b₁·ONE + b₀`,
/// `a·b / ONE = a₁·b₁·ONE + a₁·b₀ + a₀·b₁ + ⌊a₀·b₀ / ONE⌋`. Only the last term divides, so the
/// sum is exactly `⌊a·b / ONE⌋`, and for factors up to `FACTOR_MAX` (a₁, b₁ ≤ 10⁹; a₀, b₀ < 10¹⁸)
/// every term fits in `u128`.
fn combine_exact(a: u128, b: u128) -> u128 {
    let (a_whole, a_frac) = (a / FACTOR_ONE, a % FACTOR_ONE);
    let (b_whole, b_frac) = (b / FACTOR_ONE, b % FACTOR_ONE);
    // Saturating so out-of-range inputs clamp to FACTOR_MAX instead of wrapping.
    (a_whole.saturating_mul(b_whole).saturating_mul(FACTOR_ONE))
        .saturating_add(a_whole.saturating_mul(b_frac))
        .saturating_add(a_frac.saturating_mul(b_whole))
        .saturating_add(a_frac * b_frac / FACTOR_ONE)
        .min(FACTOR_MAX)
}

/// Sub-ticks to advance for one tick at `factor`, carrying the fractional sub-tick in
/// `remainder` (units of 1 / [`FACTOR_ONE`] sub-tick) so slow motion never loses time.
///
/// At normal speed a tick is exactly [`SUBTICKS_PER_TICK`] and at 0 nothing, and neither touches
/// the carry, which stays below one sub-tick (`FACTOR_ONE`): both return at once (review FI-102),
/// as [`advance_exact`] would.
pub(crate) fn advance(factor: u128, remainder: &mut u64) -> u64 {
    debug_assert!(u128::from(*remainder) < FACTOR_ONE, "the carry is below one sub-tick");
    match factor {
        FACTOR_ONE => u64::from(SUBTICKS_PER_TICK),
        0 => 0,
        _ => advance_exact(factor, remainder),
    }
}

/// [`advance`] for any factor, the reference its shortcuts must match.
fn advance_exact(factor: u128, remainder: &mut u64) -> u64 {
    // At most 1000 × 1e27 + 1e18, far inside `u128`.
    let total = u128::from(SUBTICKS_PER_TICK) * factor + u128::from(*remainder);
    // The remainder is below FACTOR_ONE (1e18), which fits in `u64`.
    *remainder = (total % FACTOR_ONE) as u64;
    // At most 1e12 sub-ticks per tick, which fits in `u64`.
    (total / FACTOR_ONE) as u64
}

/// A factor as a float multiplier, for presentation (audio pitch, animation rate).
pub(crate) fn factor_to_f32(factor: u128) -> f32 {
    (factor as f64 / FACTOR_ONE as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_of_scales() {
        let mut stack = ScaleStack::<4>::new(ClockId::WORLD);
        assert_eq!(stack.factor(), FACTOR_ONE);
        let half = stack.push(ScaleSource::Debug, 500).expect("room");
        let fifth = stack.push(ScaleSource::PerfectDodge, 200).expect("room");
        assert_eq!(stack.factor(), FACTOR_ONE / 10);
        assert_eq!(stack.pop(half), Some(TimeScale { source: ScaleSource::Debug, per_mille: 500 }));
        assert_eq!(stack.factor(), FACTOR_ONE / 5);
        assert_eq!(stack.pop(half), None);
        assert!(stack.pop(fifth).is_some());
        assert_eq!(stack, ScaleStack::<4> { next_serial: 2, ..ScaleStack::new(ClockId::WORLD) });
    }

    #[test]
    fn full_stack_is_an_error() {
        let mut stack = ScaleStack::<2>::new(ClockId::WORLD);
        stack.push(ScaleSource::Debug, 1000).expect("room");
        stack.push(ScaleSource::Debug, 1000).expect("room");
        assert_eq!(stack.push(ScaleSource::Debug, 1000), Err(ScaleStackFull { capacity: 2 }));
        assert_eq!(stack.iter().len(), 2);
    }

    #[test]
    fn remove_source_keeps_others_in_order() {
        let mut stack = ScaleStack::<4>::new(ClockId::WORLD);
        stack.push(ScaleSource::Debug, 2000).expect("room");
        stack.push(ScaleSource::Cinematic, 500).expect("room");
        stack.push(ScaleSource::Debug, 3000).expect("room");
        stack.push(ScaleSource::Gameplay, 800).expect("room");
        assert_eq!(stack.remove_source(ScaleSource::Debug), 2);
        let left: Vec<_> = stack.iter().map(|scale| scale.source).collect();
        assert_eq!(left, [ScaleSource::Cinematic, ScaleSource::Gameplay]);
        assert_eq!(stack.factor(), FACTOR_ONE * 4 / 10);
        assert_eq!(stack.remove_source(ScaleSource::Pause), 0);
    }

    #[test]
    fn advance_carries_fractions() {
        // 0.3 × 0.333 = 0.0999× is 99.9 sub-ticks per tick.
        let factor = combine(FACTOR_ONE * 300 / 1000, FACTOR_ONE * 333 / 1000);
        let mut remainder = 0;
        let total: u64 = (0..1000).map(|_| advance(factor, &mut remainder)).sum();
        assert_eq!(total, 99_900);
        assert_eq!(remainder, 0);
    }

    #[test]
    fn combine_is_exact_across_the_range() {
        let x = |per_mille: u128| FACTOR_ONE * per_mille / 1000;
        assert_eq!(combine(x(1000), x(1000)), FACTOR_ONE);
        // Products far beyond u128 when multiplied directly (341× · 1× overflowed before).
        assert_eq!(combine(x(341_000), FACTOR_ONE), x(341_000));
        assert_eq!(combine(x(1_000_000), FACTOR_ONE), x(1_000_000));
        assert_eq!(combine(x(100_000), x(4000)), x(400_000));
        assert_eq!(combine(x(20_000), x(20_000)), x(400_000));
        // Fractional parts on both sides: 0.3 × 0.333 and 1234.567 × 0.001.
        assert_eq!(combine(x(300), x(333)), FACTOR_ONE * 999 / 10_000);
        assert_eq!(combine(x(1_234_567), x(1)), x(1_234_567) / 1000);
        // Floor of the exact product for awkward values (checked with arbitrary precision).
        assert_eq!(combine(FACTOR_ONE * 7 / 3, FACTOR_ONE * 11 / 7), 3_666_666_666_666_666_664);
        assert_eq!(combine(0, FACTOR_MAX), 0);
    }

    #[test]
    fn extreme_factors_saturate_instead_of_overflowing() {
        let mut stack = ScaleStack::<8>::new(ClockId::WORLD);
        for _ in 0..8 {
            stack.push(ScaleSource::Debug, u32::MAX).expect("room");
        }
        assert_eq!(stack.factor(), FACTOR_MAX);
        assert_eq!(combine(FACTOR_MAX, FACTOR_MAX), FACTOR_MAX);
        assert_eq!(combine(u128::MAX, u128::MAX), FACTOR_MAX);
        // 10⁵× · 10⁵× = 10¹⁰× is the first product past the cap.
        let hundred_thousand = FACTOR_ONE * 100_000;
        assert_eq!(combine(hundred_thousand, hundred_thousand / 10), FACTOR_MAX);
        assert_eq!(combine(hundred_thousand, hundred_thousand), FACTOR_MAX);
        let mut remainder = 0;
        assert_eq!(advance(FACTOR_MAX, &mut remainder), 1_000_000_000_000);
    }

    /// A fixed xorshift sequence, so the property tests check the same values on every run.
    fn sequence(mut state: u64) -> impl FnMut() -> u64 {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// Review FI-102: the normal-speed and stopped shortcuts give exactly what the general
    /// arithmetic gives, for every factor (out-of-range ones included) and every carry.
    #[test]
    fn shortcuts_match_the_exact_arithmetic() {
        let mut next = sequence(0x2545_f491_4f6c_dd1d);
        let mut factors = vec![
            0,
            1,
            FACTOR_ONE - 1,
            FACTOR_ONE,
            FACTOR_ONE + 1,
            FACTOR_ONE / 5,
            FACTOR_ONE * 5,
            FACTOR_MAX - 1,
            FACTOR_MAX,
            FACTOR_MAX + 1,
            u128::MAX,
        ];
        factors.extend((0..200).map(|i| {
            let wide = u128::from(next()) << 64 | u128::from(next());
            // Spread over every magnitude, up to past the cap.
            wide >> (i % 128)
        }));
        let carries = [0, 1, FACTOR_ONE as u64 - 1, FACTOR_ONE as u64 / 3, next() % FACTOR_ONE as u64];
        for &a in &factors {
            for &b in &factors {
                assert_eq!(combine(a, b), combine_exact(a, b), "{a} × {b}");
            }
            // Clocks only advance by factors their stacks and `combine` saturated.
            for carry in carries.into_iter().filter(|_| a <= FACTOR_MAX) {
                let (mut fast, mut exact) = (carry, carry);
                assert_eq!(advance(a, &mut fast), advance_exact(a, &mut exact), "{a} from {carry}");
                assert_eq!(fast, exact, "{a} from {carry}: the carry");
            }
        }
    }

    /// Review FI-102: a world and 64 entity clocks through 2000 steps of changing scales (stops,
    /// normal speed, slow motion with a carry, products of exactly 1×, extremes), hitstop and
    /// clocks with no scales of their own tick exactly as the general arithmetic alone does.
    #[test]
    fn mixed_scale_clocks_tick_as_the_exact_arithmetic_does() {
        const WORLD: [u32; 6] = [0, 1, 333, 1000, 5000, 1_000_000];
        // 200 under a world at 5000 makes exactly 1× from two factors that aren't.
        const LOCAL: [u32; 7] = [0, 1, 200, 250, 1000, 3000, 100_000];
        let mut world = crate::ScaledClock::new();
        let mut clocks: Vec<_> = (0..64).map(|_| world.new_local_clock()).collect();
        // The reference: the same stacks, ticked with the exact arithmetic only.
        let mut world_reference = (ScaleStack::<8>::new(ClockId::WORLD), 0_u64);
        let mut references = vec![(ScaleStack::<4>::new(ClockId(1)), 0_u64, 0_u16); clocks.len()];
        let (mut fast_paths, mut carried) = (0, 0);
        for step in 0..2000_usize {
            if step % 37 == 0 {
                let per_mille = WORLD[step / 37 % WORLD.len()];
                world.remove_source(ScaleSource::Gameplay);
                world.push_scale(ScaleSource::Gameplay, per_mille).expect("room");
                world_reference.0.remove_source(ScaleSource::Gameplay);
                world_reference.0.push(ScaleSource::Gameplay, per_mille).expect("room");
            }
            let delta = world.tick();
            let expected = advance_exact(world_reference.0.factor(), &mut world_reference.1);
            assert_eq!(delta.as_subticks(), expected, "world, step {step}");
            for (index, (clock, reference)) in clocks.iter_mut().zip(&mut references).enumerate() {
                // A quarter of the clocks never get a scale of their own.
                if step % 31 == 0 && index % 4 != 0 {
                    let per_mille = LOCAL[(index + step / 31) % LOCAL.len()];
                    clock.remove_source(ScaleSource::Cinematic);
                    clock.push_scale(ScaleSource::Cinematic, per_mille).expect("room");
                    reference.0.remove_source(ScaleSource::Cinematic);
                    reference.0.push(ScaleSource::Cinematic, per_mille).expect("room");
                }
                if (step + index) % 29 == 0 {
                    clock.apply_hitstop(3);
                    reference.2 = reference.2.max(3);
                }
                let expected = if reference.2 > 0 {
                    if world_reference.0.factor() != 0 {
                        reference.2 -= 1;
                    }
                    0
                } else {
                    let factor = combine_exact(world_reference.0.factor(), reference.0.factor());
                    fast_paths += usize::from(factor == FACTOR_ONE || factor == 0);
                    carried += usize::from(factor == FACTOR_ONE && reference.1 != 0);
                    advance_exact(factor, &mut reference.1)
                };
                assert_eq!(clock.tick(&world).as_subticks(), expected, "clock {index}, step {step}");
                assert_eq!(clock.hitstop_ticks(), reference.2, "clock {index}, step {step}");
            }
        }
        // The trace exercises the shortcuts, normal speed with a carry from slow motion included.
        assert!(fast_paths > 10_000 && carried > 1000, "{fast_paths} shortcuts, {carried} with a carry");
    }

    #[test]
    fn handles_are_bound_to_their_clock() {
        let mut world = ScaleStack::<4>::new(ClockId::WORLD);
        let mut local = ScaleStack::<4>::new(ClockId(1));
        let world_handle = world.push(ScaleSource::PerfectDodge, 200).expect("room");
        let local_handle = local.push(ScaleSource::Gameplay, 1500).expect("room");
        assert_ne!(world_handle, local_handle, "same serial, different clocks");
        assert_eq!(local.owner(), ClockId(1));
        let wrong = std::panic::catch_unwind(move || {
            let mut local = local;
            (local.pop(world_handle), local)
        });
        if cfg!(debug_assertions) {
            assert!(wrong.is_err(), "a foreign handle fails a debug assertion");
        } else {
            let (popped, local) = wrong.expect("release builds return None");
            assert_eq!(popped, None);
            assert_eq!(local.iter().len(), 1);
        }
        assert!(world.pop(world_handle).is_some());
        assert_eq!(world.pop(world_handle), None, "already popped is quietly None");
    }
}
