//! Easing: how a blend between two keys progresses (spec §5.4, "curves").
//!
//! An [`Ease`] maps a segment's progress `u`, from 0 at its first key to 1 at the next, to the
//! weight of the next key. Every ease is exactly 0 at `u = 0` and exactly 1 at `u = 1`, so a
//! curve evaluated on a key reproduces it, and none overshoots: weights stay within `[0, 1]`.

/// The shape of a blend between two keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Ease {
    /// Constant speed.
    #[default]
    Linear,
    /// Slow at both ends (smoothstep, `3u² − 2u³`): no kink at the keys when the segments on
    /// either side also ease.
    Smooth,
    /// Starts slowly and arrives at full speed (`u²`).
    In,
    /// Leaves at full speed and arrives slowly (`1 − (1 − u)²`).
    Out,
    /// Holds the first key's value until the next key: a switch rather than a blend.
    Hold,
}

impl Ease {
    /// The weight of the next key at progress `u`, clamped to `[0, 1]` first.
    #[must_use]
    pub fn apply(self, u: f32) -> f32 {
        // NaN progress counts as the segment's start.
        let u = if u.is_nan() { 0.0 } else { u.clamp(0.0, 1.0) };
        match self {
            Self::Linear => u,
            Self::Smooth => u * u * (3.0 - 2.0 * u),
            Self::In => u * u,
            Self::Out => u * (2.0 - u),
            Self::Hold => {
                if u >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

/// Smoothstep from `edge0` to `edge1`: 0 below `edge0`, 1 above `edge1`, and the [`Ease::Smooth`]
/// curve between. `edge0` may exceed `edge1` for a falling step. Equal edges make a hard step at
/// that value.
#[must_use]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge0 == edge1 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    Ease::Smooth.apply((x - edge0) / (edge1 - edge0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Ease; 5] = [Ease::Linear, Ease::Smooth, Ease::In, Ease::Out, Ease::Hold];

    #[test]
    fn every_ease_is_exact_at_the_keys_and_stays_in_range() {
        for ease in ALL {
            assert_eq!(ease.apply(0.0), 0.0, "{ease:?}");
            assert_eq!(ease.apply(1.0), 1.0, "{ease:?}");
            assert_eq!(ease.apply(-3.0), 0.0, "{ease:?} clamps");
            assert_eq!(ease.apply(7.0), 1.0, "{ease:?} clamps");
            assert_eq!(ease.apply(f32::NAN), 0.0, "{ease:?}");
            let mut previous = 0.0;
            for step in 0..=1000 {
                let weight = ease.apply(step as f32 / 1000.0);
                assert!((0.0..=1.0).contains(&weight), "{ease:?}");
                assert!(weight >= previous, "{ease:?} never turns back");
                previous = weight;
            }
        }
    }

    #[test]
    fn eases_have_their_shapes() {
        assert_eq!(Ease::Linear.apply(0.25), 0.25);
        assert_eq!(Ease::Smooth.apply(0.5), 0.5);
        assert!(Ease::Smooth.apply(0.1) < 0.1 && Ease::Smooth.apply(0.9) > 0.9, "slow at both ends");
        assert_eq!(Ease::In.apply(0.5), 0.25);
        assert_eq!(Ease::Out.apply(0.5), 0.75);
        assert_eq!(Ease::Hold.apply(0.999), 0.0);
    }

    #[test]
    fn smoothstep_rises_and_falls() {
        assert_eq!(smoothstep(1.0, 3.0, 0.0), 0.0);
        assert_eq!(smoothstep(1.0, 3.0, 2.0), 0.5);
        assert_eq!(smoothstep(1.0, 3.0, 5.0), 1.0);
        assert_eq!(smoothstep(3.0, 1.0, 5.0), 0.0, "falling");
        assert_eq!(smoothstep(2.0, 2.0, 1.9), 0.0);
        assert_eq!(smoothstep(2.0, 2.0, 2.0), 1.0);
    }
}
