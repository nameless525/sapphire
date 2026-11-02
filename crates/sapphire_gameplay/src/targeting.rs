//! Who a soft lock picks (spec §5.22, "Targeting").
//!
//! A soft-locked target decides where a move warps, so choosing it is simulation: a pure function of
//! plain values, with `sapphire_math::det`'s math only and ties broken by [`ActorId`], so it gives one
//! answer whatever order the candidates come in. It holds no state. The target it chose last goes back
//! in as [`TargetQuery::current`], which is what keeps it from flickering between two enemies at equal
//! angles: the current one has to be beaten by [`TargetingWeights::hysteresis`], not merely matched.
//! Whoever owns that state (the moves' warp, when G2 and G3 wire this in) checksums it.
//!
//! A candidate scores by how near it is to where the player is pointing and how close it is:
//! `direction · (1 − angle / π) + distance · (1 − metres / range)`. Pointing means the stick, or the actor's
//! facing with the stick at rest, at the `facing` weight. Auto-target on touch (pillar 7) is the same
//! function with [`TargetingWeights::TOUCH`], where closeness counts for most.
//!
//! The camera doesn't choose anything: it's handed the chosen target's position.

use sapphire_math::{Vec3, det};

use crate::actions::Stick;
use crate::actors::ActorId;

/// A stick shorter than this, of a full 1, isn't pointing anywhere: it's a thumb at rest.
const STICK_AT_REST: f32 = 0.2;

/// Someone who could be targeted, in the world's metres. The caller leaves out the dead and untargetable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetCandidate {
    pub id: ActorId,
    pub position: Vec3,
}

/// How a choice is weighed. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetingWeights {
    /// How much it matters where the stick points.
    pub direction: f32,
    /// How much it matters where the actor faces, with the stick at rest.
    pub facing: f32,
    /// How much it matters how close they are.
    pub distance: f32,
    /// Nobody further than this is picked, in metres.
    pub range: f32,
    /// The current target stays a candidate out to here, so walking off doesn't drop it at a line.
    pub keep_range: f32,
    /// What the current target's score gets, which another has to beat. Against `direction`, each 0.01
    /// of it is worth about 1.8° of aim at a weight of 1.
    pub hysteresis: f32,
}

impl TargetingWeights {
    /// A gamepad's soft lock: the stick says who, and closeness decides what's left.
    pub const SOFT_LOCK: Self =
        Self { direction: 1.6, facing: 0.5, distance: 1.0, range: 12.0, keep_range: 16.0, hysteresis: 0.12 };

    /// Auto-target on touch: thumbs don't aim, so whoever's closest, mostly, with a longer reach.
    pub const TOUCH: Self =
        Self { direction: 0.3, facing: 0.15, distance: 1.5, range: 14.0, keep_range: 18.0, hysteresis: 0.15 };
}

/// What a choice is made from: one tick's plain values.
#[derive(Clone, Copy, Debug)]
pub struct TargetQuery<'a> {
    /// The actor choosing.
    pub position: Vec3,
    /// Which way they face, in radians: +Z at 0, toward +X.
    pub yaw: f32,
    /// Their stick, which is world-space: +x along world +X and +y along world +Z.
    pub stick: Stick,
    /// Who they've got now, if anyone.
    pub current: Option<ActorId>,
    /// Whoever could be chosen, in any order.
    pub candidates: &'a [TargetCandidate],
}

/// The best target, or `None` if nobody's in range. Allocates nothing and reads no state.
#[must_use]
pub fn choose_target(query: &TargetQuery<'_>, weights: &TargetingWeights) -> Option<ActorId> {
    let stick = query.stick.to_vec2();
    let pushed = stick.length_squared() >= STICK_AT_REST * STICK_AT_REST;
    let (aim, aim_weight) = if pushed {
        (Vec3::new(stick.x, 0.0, stick.y), weights.direction)
    } else {
        (det::direction_from_yaw(query.yaw), weights.facing)
    };

    let mut best: Option<(f32, ActorId)> = None;
    for candidate in query.candidates {
        let offset = candidate.position - query.position;
        let metres = offset.length();
        let is_current = query.current == Some(candidate.id);
        let reach = if is_current { weights.keep_range } else { weights.range };
        if metres.is_nan() || metres > reach {
            continue;
        }
        let ground = Vec3::new(offset.x, 0.0, offset.z);
        let aimed = 1.0 - det::angle_between(aim, ground) / std::f32::consts::PI;
        let near = (1.0 - metres / weights.range).max(0.0);
        let keep = if is_current { weights.hysteresis } else { 0.0 };
        let score = aim_weight * aimed + weights.distance * near + keep;
        let better = best.is_none_or(|(top, id)| score > top || (score == top && candidate.id < id));
        if better {
            best = Some((score, candidate.id));
        }
    }
    best.map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use sapphire_math::det::direction_from_yaw;

    use super::*;

    /// A row of a table: what it shows, facing, stick, the current target, the candidates and the answer.
    type Scene<'a> = (&'a str, f32, Stick, Option<u32>, &'a [TargetCandidate], Option<u32>);

    fn id(raw: u32) -> ActorId {
        ActorId::from_raw(raw).expect("not zero")
    }

    fn at(raw: u32, x: f32, z: f32) -> TargetCandidate {
        TargetCandidate { id: id(raw), position: Vec3::new(x, 0.0, z) }
    }

    /// A stick pushed the world's way: `x` along +X, `z` along +Z.
    fn pushed(x: f32, z: f32) -> Stick {
        Stick::from_axes(x, z)
    }

    fn choose(
        yaw: f32,
        stick: Stick,
        current: Option<u32>,
        candidates: &[TargetCandidate],
        weights: &TargetingWeights,
    ) -> Option<ActorId> {
        let query = TargetQuery { position: Vec3::ZERO, yaw, stick, current: current.map(id), candidates };
        choose_target(&query, weights)
    }

    /// The soft lock's table.
    #[test]
    fn soft_lock_scenes() {
        let at_rest = Stick::CENTER;
        let scenes: [Scene<'_>; 12] = [
            ("nobody there", 0.0, at_rest, None, &[], None),
            ("nobody in range", 0.0, at_rest, None, &[at(1, 0.0, 30.0)], None),
            ("only one", 0.0, at_rest, None, &[at(1, 3.0, 3.0)], Some(1)),
            ("the nearer of two ahead", 0.0, at_rest, None, &[at(1, 0.0, 9.0), at(2, 0.0, 4.0)], Some(2)),
            (
                "the one faced, at equal range",
                0.0,
                at_rest,
                None,
                &[at(1, 0.0, -5.0), at(2, 0.0, 5.0)],
                Some(2),
            ),
            (
                "facing turned round",
                std::f32::consts::PI,
                at_rest,
                None,
                &[at(1, 0.0, -5.0), at(2, 0.0, 5.0)],
                Some(1),
            ),
            (
                "the stick over facing",
                0.0,
                pushed(1.0, 0.0),
                None,
                &[at(1, 5.0, 0.0), at(2, 0.0, 5.0)],
                Some(1),
            ),
            (
                "the stick over closeness",
                0.0,
                pushed(0.0, 1.0),
                None,
                &[at(1, 0.0, 9.0), at(2, 0.0, -3.0)],
                Some(1),
            ),
            (
                "closeness over a small aim",
                0.0,
                pushed(0.0, 1.0),
                None,
                &[at(1, 1.8, 9.0), at(2, -1.0, 3.0)],
                Some(2),
            ),
            (
                "a thumb at rest doesn't aim",
                0.0,
                Stick::from_axes(0.1, 0.0),
                None,
                &[at(1, 5.0, 0.0), at(2, 0.0, 5.0)],
                Some(2),
            ),
            ("the current one, out of range but kept", 0.0, at_rest, Some(1), &[at(1, 0.0, 15.0)], Some(1)),
            ("and past what's kept", 0.0, at_rest, Some(1), &[at(1, 0.0, 17.0)], None),
        ];
        for (what, yaw, stick, current, candidates, answer) in scenes {
            let got = choose(yaw, stick, current, candidates, &TargetingWeights::SOFT_LOCK);
            assert_eq!(got, answer.map(id), "{what}");
        }
    }

    /// Touch's table: closeness first, so where the player faces hardly counts.
    #[test]
    fn auto_target_on_touch_scenes() {
        let touch = TargetingWeights::TOUCH;
        let at_rest = Stick::CENTER;
        let scenes: [Scene<'_>; 5] = [
            (
                "nearest, behind, over further, ahead",
                0.0,
                at_rest,
                None,
                &[at(1, 0.0, 9.0), at(2, 0.0, -3.0)],
                Some(2),
            ),
            (
                "a soft lock would disagree",
                0.0,
                pushed(0.0, 1.0),
                None,
                &[at(1, 0.0, 9.0), at(2, 0.0, -3.0)],
                Some(2),
            ),
            (
                "at equal range, the one ahead",
                0.0,
                at_rest,
                None,
                &[at(1, 0.0, -5.0), at(2, 0.0, 5.0)],
                Some(2),
            ),
            ("a longer reach than the soft lock's", 0.0, at_rest, None, &[at(1, 0.0, 13.0)], Some(1)),
            ("the current one, kept further", 0.0, at_rest, Some(1), &[at(1, 0.0, 17.0)], Some(1)),
        ];
        for (what, yaw, stick, current, candidates, answer) in scenes {
            assert_eq!(choose(yaw, stick, current, candidates, &touch), answer.map(id), "{what}");
        }
        let soft = choose(0.0, pushed(0.0, 1.0), None, scenes[1].4, &TargetingWeights::SOFT_LOCK);
        assert_eq!(soft, Some(id(1)), "the two weights must differ on that scene");
    }

    /// The same scene in every order gives one answer, and an exact tie goes to the lower ID.
    #[test]
    fn order_doesnt_matter_and_ties_go_to_the_lower_id() {
        // Exactly mirrored across the aim, so exactly equal.
        let (a, b, c) = (at(3, 4.0, 3.0), at(7, -4.0, 3.0), at(9, 0.0, -11.0));
        let orders = [[a, b, c], [a, c, b], [b, a, c], [b, c, a], [c, a, b], [c, b, a]];
        for order in &orders {
            for weights in [TargetingWeights::SOFT_LOCK, TargetingWeights::TOUCH] {
                assert_eq!(choose(0.0, Stick::CENTER, None, order, &weights), Some(id(3)), "{order:?}");
            }
        }
    }

    /// Two enemies at equal angles either side of the aim, each a hair ahead in turn from tick to tick: it
    /// holds who it has. Without the hysteresis it would change every tick.
    #[test]
    fn hysteresis_holds_the_target_between_two_at_equal_angles() {
        let weights = TargetingWeights::SOFT_LOCK;
        let wobble = |tick: u32| if tick.is_multiple_of(2) { 0.03 } else { -0.03 };
        let scene = |tick: u32| [at(1, 3.0 + wobble(tick), 4.0), at(2, -3.0 + wobble(tick), 4.0)];

        let mut current = None;
        let mut changes = 0;
        for tick in 0..120 {
            let got = choose(0.0, pushed(0.0, 1.0), current, &scene(tick), &weights);
            let got = got.map(|id| id.raw());
            if current.is_some() && got != current {
                changes += 1;
            }
            current = got;
        }
        assert_eq!(changes, 0);

        // The same ticks without it flicker, which is what the test above is there for.
        let plain = TargetingWeights { hysteresis: 0.0, ..weights };
        let mut current = None;
        let mut changes = 0;
        for tick in 0..120 {
            let got = choose(0.0, pushed(0.0, 1.0), current, &scene(tick), &plain).map(|id| id.raw());
            if current.is_some() && got != current {
                changes += 1;
            }
            current = got;
        }
        assert!(changes > 100, "{changes}");
    }

    /// Hysteresis doesn't stop a player meaning it: a stick swung over to the other changes the target,
    /// once, at one angle on the way there, and swung back changes it back at another, past the middle.
    #[test]
    fn a_deliberate_swing_still_changes_the_target_once() {
        let weights = TargetingWeights::SOFT_LOCK;
        let scene = [at(1, 3.0, 4.0), at(2, -3.0, 4.0)];
        let aim_at = |tick: i32| {
            // Swinging from well toward 1 (+X) to well toward 2, a degree a tick.
            let angle = (40 - tick) as f32 * std::f32::consts::PI / 180.0;
            let aim = direction_from_yaw(angle);
            pushed(aim.x, aim.z)
        };

        let mut current = choose(0.0, aim_at(0), None, &scene, &weights).map(|id| id.raw());
        assert_eq!(current, Some(1));
        let mut changed_at = Vec::new();
        for tick in 0..=80 {
            let got = choose(0.0, aim_at(tick), current, &scene, &weights).map(|id| id.raw());
            if got != current {
                changed_at.push(tick);
            }
            current = got;
        }
        assert_eq!(current, Some(2));
        assert_eq!(changed_at.len(), 1, "{changed_at:?}");
        assert!(changed_at[0] > 42, "it must pass the middle first: {changed_at:?}");

        let mut back_at = Vec::new();
        for tick in (0..=80).rev() {
            let got = choose(0.0, aim_at(tick), current, &scene, &weights).map(|id| id.raw());
            if got != current {
                back_at.push(tick);
            }
            current = got;
        }
        assert_eq!(current, Some(1));
        assert_eq!(back_at.len(), 1, "{back_at:?}");
        assert!(back_at[0] < changed_at[0], "two different places: {back_at:?} and {changed_at:?}");
    }

    /// Someone standing exactly where the actor stands has no direction, and overhead has none across the
    /// ground: both still score, and a NaN from upstream is just not a candidate.
    #[test]
    fn degenerate_positions() {
        let weights = TargetingWeights::SOFT_LOCK;
        let on_top = TargetCandidate { id: id(1), position: Vec3::ZERO };
        let overhead = TargetCandidate { id: id(2), position: Vec3::new(0.0, 3.0, 0.0) };
        let broken = TargetCandidate { id: id(3), position: Vec3::NAN };
        assert_eq!(choose(0.0, Stick::CENTER, None, &[on_top], &weights), Some(id(1)));
        assert_eq!(choose(0.0, Stick::CENTER, None, &[overhead], &weights), Some(id(2)));
        assert_eq!(choose(0.0, Stick::CENTER, None, &[broken], &weights), None);
        assert_eq!(choose(0.0, Stick::CENTER, None, &[broken, overhead], &weights), Some(id(2)));
    }
}
