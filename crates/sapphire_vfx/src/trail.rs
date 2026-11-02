//! What a sword trail keeps and how it smooths it: two sockets' samples, on the owner's clock, and
//! a **centripetal Catmull-Rom** spline through each socket's path (Yuksel, Schaefer and Keyser,
//! 2011). Uniform Catmull-Rom overshoots and can loop where samples are unevenly spaced, as they
//! are where a swing starts and stops, and through a hitstop; the centripetal form's knots, the
//! square root of each chord's length, are what rule that out. Each rail gets its own knots, since
//! the tip travels far further than the base.

use glam::Vec3;

/// A sample of the two sockets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TrailSample {
    pub base: Vec3,
    pub tip: Vec3,
    /// When, on its effect's clock (the effect's age, so it holds still while the owner is frozen).
    pub time: f32,
}

/// The smallest knot interval, which keeps coincident samples from dividing by zero: a base socket
/// that holds still while the tip sweeps.
const MIN_KNOT: f32 = 1e-3;

/// One rail's stretch of spline between `p[1]` and `p[2]`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Segment {
    p: [Vec3; 4],
    t: [f32; 3],
}

impl Segment {
    /// The stretch from `p1` to `p2`, with `p0` and `p3` the samples either side.
    pub(crate) fn new(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3) -> Self {
        let knot = |a: Vec3, b: Vec3| a.distance(b).sqrt().max(MIN_KNOT);
        let t1 = knot(p0, p1);
        let t2 = t1 + knot(p1, p2);
        let t3 = t2 + knot(p2, p3);
        Self { p: [p0, p1, p2, p3], t: [t1, t2, t3] }
    }

    /// The point `u` of the way along, 0 at `p1` and 1 at `p2`, by the Barry-Goldman evaluation.
    pub(crate) fn at(&self, u: f32) -> Vec3 {
        let [p0, p1, p2, p3] = self.p;
        let [t1, t2, t3] = self.t;
        let t = t1 + (t2 - t1) * u;
        let a1 = p0.lerp(p1, t / t1);
        let a2 = p1.lerp(p2, (t - t1) / (t2 - t1));
        let a3 = p2.lerp(p3, (t - t2) / (t3 - t2));
        let b1 = a1.lerp(a2, t / t2);
        let b2 = a2.lerp(a3, (t - t1) / (t3 - t1));
        b1.lerp(b2, (t - t1) / (t2 - t1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(points: [[f32; 3]; 4]) -> Segment {
        let [p0, p1, p2, p3] = points.map(Vec3::from);
        Segment::new(p0, p1, p2, p3)
    }

    #[test]
    fn it_passes_through_its_samples() {
        let stretch = segment([[0.0, 0.0, 0.0], [1.0, 0.5, 0.0], [2.0, 2.0, 1.0], [2.5, 4.0, 1.5]]);
        assert!(stretch.at(0.0).distance(Vec3::new(1.0, 0.5, 0.0)) < 1e-5);
        assert!(stretch.at(1.0).distance(Vec3::new(2.0, 2.0, 1.0)) < 1e-5);
    }

    #[test]
    fn a_straight_line_stays_straight() {
        let stretch = segment([[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]]);
        for step in 0..=8 {
            let point = stretch.at(step as f32 / 8.0);
            assert!(point.y.abs() < 1e-6 && point.z.abs() < 1e-6);
            assert!((1.0 - 1e-5..=2.0 + 1e-5).contains(&point.x));
        }
    }

    /// Why centripetal: a long stretch beside a short one, as where a swing starts. Uniform
    /// Catmull-Rom swings far past the short stretch's ends; this stays close.
    #[test]
    fn uneven_spacing_barely_overshoots() {
        let stretch = segment([[-10.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.1, 0.1, 0.0], [0.2, -10.0, 0.0]]);
        let uniform = |u: f32| {
            let (p0, p1, p2, p3) = (
                Vec3::new(-10.0, 0.0, 0.0),
                Vec3::ZERO,
                Vec3::new(0.1, 0.1, 0.0),
                Vec3::new(0.2, -10.0, 0.0),
            );
            0.5 * (2.0 * p1
                + (p2 - p0) * u
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u * u
                + (3.0 * p1 - p0 - 3.0 * p2 + p3) * u * u * u)
        };
        let (mut ours, mut theirs) = (0.0_f32, 0.0_f32);
        for step in 0..=32 {
            let u = step as f32 / 32.0;
            // How far past the box of the stretch's two ends each strays.
            let stray = |point: Vec3| {
                let low = Vec3::ZERO.min(Vec3::new(0.1, 0.1, 0.0));
                let high = Vec3::ZERO.max(Vec3::new(0.1, 0.1, 0.0));
                (low - point).max(point - high).max(Vec3::ZERO).length()
            };
            ours = ours.max(stray(stretch.at(u)));
            theirs = theirs.max(stray(uniform(u)));
        }
        assert!(ours < 0.1 && theirs > 0.5, "ours {ours}, uniform {theirs}");
    }

    #[test]
    fn coincident_samples_stay_finite() {
        let stretch = segment([[1.0; 3], [1.0; 3], [1.0; 3], [1.0; 3]]);
        for step in 0..=4 {
            let point = stretch.at(step as f32 / 4.0);
            assert!(point.is_finite() && point.distance(Vec3::ONE) < 1e-5, "{point}");
        }
    }
}
