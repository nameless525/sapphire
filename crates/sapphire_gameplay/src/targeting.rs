//! Who a soft lock picks (spec §5.22, "Targeting").
//!
//! A soft-locked target decides where a move warps, so choosing it is simulation: a pure function of
//! plain values, with `sapphire_math::det`'s math only and ties broken by [`ActorId`], so it gives one
//! answer whatever order the candidates come in. It holds no state. The target it chose last goes back
//! in as [`TargetQuery::current`], which is what keeps it from flickering between two enemies at equal
//! angles: the current one has to be beaten by [`TargetingWeights::hysteresis`], not merely matched.
//!
//! [`SoftLock`] is that state, on the actor, in the checksum. `choose_soft_locks` asks once a step, in
//! [`crate::CombatSet::Intents`], for whoever has one ([`crate::Simulation::give_soft_lock`]), and writes
//! the answer to its [`WarpTarget`], which is all the moves read. Which of the two sets of weights comes
//! from the tick's own [`Buttons::TOUCH`], never from the device, so a replay chooses what its run chose.
//!
//! A candidate scores by how near it is to where the player is pointing and how close it is:
//! `direction · (1 − angle / π) + distance · (1 − metres / range)`. Pointing means the stick, or the actor's
//! facing with the stick at rest, at the `facing` weight. Auto-target on touch (pillar 7) is the same
//! function with [`TargetingTuning::touch`], where closeness counts for most. How hard it pulls is the player's
//! setting ([`AutoTarget`], M4 A1): its weak and strong strengths weigh by `touch_weak` and `touch_strong` in
//! `touch`'s place.
//!
//! The camera doesn't choose anything: it's handed the chosen target's position.
//!
//! **The weights are a document** (`default_targeting.ron`, [`TargetingTuning`]), compiled in because a
//! replay needs no assets. A [`Simulation`](crate::Simulation) holds the resolved value and its
//! [`tables_hash`](crate::Simulation::tables_hash) takes it, so a fight on other weights can't pass for one
//! on these. The strength is the run's too ([`crate::Simulation::set_auto_target`]): a player changes it
//! between steps, a recording keeps that as a command, and the hash takes the weights it chooses, so a run on
//! the normal strength hashes as one from before there was a setting.

use std::sync::OnceLock;

use bevy_reflect::{Reflect, TypePath};
use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};
use sapphire_reflect::{Decoder, VersionError, Versioned, decode_versioned};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::actions::{Buttons, Controls, Stick};
use crate::actors::{ActorId, ActorTable};
use crate::checksum::StateHasher;
use crate::movement::ActorTransform;
use crate::moves::{MoveOutput, WarpTarget};
use crate::party::Party;
use crate::resolve::Fighter;

/// The shipped weights (see [`TargetingTuning::shipped`]).
const SHIPPED: &str = include_str!("../default_targeting.ron");

/// A stick shorter than this, of a full 1, isn't pointing anywhere: it's a thumb at rest.
const STICK_AT_REST: f32 = 0.2;

/// Someone who could be targeted, in the world's metres. The caller leaves out the dead and untargetable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetCandidate {
    pub id: ActorId,
    pub position: Vec3,
}

/// How a choice is weighed. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Says what's wrong with these, as `section.field`. A weight may be 0 and never negative, and a range is
    /// a distance, so a choice can't divide by zero or keep less than it picks.
    fn check(&self, section: &'static str) -> Result<(), TargetingError> {
        let Self { direction, facing, distance, range, keep_range, hysteresis } = *self;
        let fail = |field, reason| Err(TargetingError::Field { section, field, reason });
        for (field, value) in
            [("direction", direction), ("facing", facing), ("distance", distance), ("hysteresis", hysteresis)]
        {
            if !value.is_finite() || value < 0.0 {
                return fail(field, format!("{value} isn't a weight, 0 or more"));
            }
        }
        if !range.is_finite() || range <= 0.0 {
            return fail("range", format!("{range} isn't a distance above 0"));
        }
        if !keep_range.is_finite() || keep_range < range {
            return fail("keep_range", format!("{keep_range} is short of `range`, {range}"));
        }
        Ok(())
    }

    fn write_canonical(self, hasher: &mut StateHasher) {
        let Self { direction, facing, distance, range, keep_range, hysteresis } = self;
        for value in [direction, facing, distance, range, keep_range, hysteresis] {
            hasher.f32(value);
        }
    }
}

/// Every way a soft lock is weighed (a `TargetingTuning` document, schema 2).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TypePath, Resource)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct TargetingTuning {
    /// A gamepad's or a keyboard's.
    pub soft_lock: TargetingWeights,
    /// Auto-target on touch (pillar 7), at its normal strength.
    pub touch: TargetingWeights,
    /// Auto-target on touch at the player's weak strength ([`AutoTarget::Weak`]).
    pub touch_weak: TargetingWeights,
    /// Auto-target on touch at the player's strong strength ([`AutoTarget::Strong`]).
    pub touch_strong: TargetingWeights,
}

impl Versioned for TargetingTuning {
    /// 1: M3 S3. 2: M4 A1's auto-target strengths, which a version 1 document weighs as its `touch`.
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        decode_versioned::<TargetingTuningV1, D>(version, data).map(|old| Self {
            soft_lock: old.soft_lock,
            touch: old.touch,
            touch_weak: old.touch,
            touch_strong: old.touch,
        })
    }
}

/// [`TargetingTuning`] version 1 (M3 S3): one set of weights on touch.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetingTuningV1 {
    soft_lock: TargetingWeights,
    touch: TargetingWeights,
}

impl Versioned for TargetingTuningV1 {
    const VERSION: u32 = 1;
}

/// How hard auto-target pulls on touch: a player's setting (spec §6.8, M4 A1), which a step reads, so the run
/// records it. Normal is what every run before the setting ran on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect, Resource)]
#[type_path = "sapphire::gameplay"]
pub enum AutoTarget {
    /// The stick counts for more, and the reach is shorter.
    Weak,
    #[default]
    Normal,
    /// Closeness counts for even more, and the reach is longer.
    Strong,
}

impl AutoTarget {
    /// Every strength, weakest first: the order a setting steps through them.
    pub const ALL: [Self; 3] = [Self::Weak, Self::Normal, Self::Strong];

    /// Its number in a replay's command.
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        self as u32
    }

    /// The strength `raw` numbers, if it numbers one.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Weak),
            1 => Some(Self::Normal),
            2 => Some(Self::Strong),
            _ => None,
        }
    }
}

/// A targeting document that can't be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TargetingError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("{section}.{field}: {reason}")]
    Field { section: &'static str, field: &'static str, reason: String },
}

impl TargetingTuning {
    /// Sapphire's weights (`default_targeting.ron` beside this crate's manifest), compiled in.
    #[must_use]
    pub fn shipped() -> Self {
        static PARSED: OnceLock<TargetingTuning> = OnceLock::new();
        *PARSED.get_or_init(|| Self::from_ron(SHIPPED).expect("this module's tests read the shipped weights"))
    }

    /// Reads a targeting document and checks it.
    pub fn from_ron(text: &str) -> Result<Self, TargetingError> {
        let tuning: Self = sapphire_serialize::from_ron(text)?;
        tuning.soft_lock.check("soft_lock")?;
        tuning.touch.check("touch")?;
        tuning.touch_weak.check("touch_weak")?;
        tuning.touch_strong.check("touch_strong")?;
        Ok(tuning)
    }

    /// What a thumb's soft lock weighs by at `strength`.
    #[must_use]
    pub const fn on_touch(&self, strength: AutoTarget) -> &TargetingWeights {
        match strength {
            AutoTarget::Weak => &self.touch_weak,
            AutoTarget::Normal => &self.touch,
            AutoTarget::Strong => &self.touch_strong,
        }
    }

    /// What [`crate::Simulation::tables_hash`] takes of them at `strength`: the numbers a step can read, by
    /// their bits, the same on every architecture. The strengths a run isn't on aren't in it, so at the normal
    /// strength it's what it was before there were others.
    #[must_use]
    pub(crate) fn digest(&self, strength: AutoTarget) -> u64 {
        let mut hasher = StateHasher::new();
        self.soft_lock.write_canonical(&mut hasher);
        self.on_touch(strength).write_canonical(&mut hasher);
        hasher.finish()
    }
}

/// An actor's soft lock: whether it has one, and who it has now. Every pooled actor carries one, off until
/// [`crate::Simulation::give_soft_lock`]. See the [module docs](self).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoftLock {
    pub(crate) on: bool,
    pub(crate) target: Option<ActorId>,
}

impl SoftLock {
    /// On, with nobody chosen yet: its first step chooses.
    pub const ON: Self = Self { on: true, target: None };

    /// Whether this actor chooses at all.
    #[must_use]
    pub const fn is_on(&self) -> bool {
        self.on
    }

    /// Who it has, as of the last step.
    #[must_use]
    pub const fn target(&self) -> Option<ActorId> {
        self.target
    }
}

/// Room for one actor's candidates, so choosing allocates nothing: the pool's size bounds it.
#[derive(Resource, Debug)]
pub(crate) struct LockCandidates(Vec<TargetCandidate>);

impl LockCandidates {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }
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

/// [`crate::CombatSet::Intents`], after the buttons and before the brains: each soft lock, in ID order,
/// chooses among whoever's hostile, up and on the field, as they stood at the end of the last step, and
/// aims its actor's warps at that. Nobody in reach aims them at nothing. It runs on a frozen clock too,
/// so a target that falls in a hit's own freeze doesn't stay chosen through it, and leaves the aim alone
/// while a warp window is open, so a move bends toward one target from start to end.
#[allow(clippy::type_complexity, reason = "a system's query reads as what it touches")]
pub(crate) fn choose_soft_locks(
    table: Res<'_, ActorTable>,
    party: Res<'_, Party>,
    weights: Res<'_, TargetingTuning>,
    strength: Res<'_, AutoTarget>,
    mut candidates: ResMut<'_, LockCandidates>,
    mut locks: Query<'_, '_, (&mut SoftLock, &mut WarpTarget, &Controls, &MoveOutput)>,
    bodies: Query<'_, '_, (&ActorTransform, &Fighter)>,
) {
    let candidates = &mut candidates.0;
    for (id, entity) in table.iter() {
        let Ok((mut lock, mut aim, controls, output)) = locks.get_mut(entity) else { continue };
        if !lock.on || party.is_off_field(id) {
            continue;
        }
        let Ok((transform, fighter)) = bodies.get(entity) else { continue };
        if fighter.is_defeated() {
            continue;
        }
        candidates.clear();
        for (other, entity) in table.iter() {
            let Ok((there, other_fighter)) = bodies.get(entity) else { continue };
            if other != id && other_fighter.is_foe_of(fighter.faction()) && !party.is_off_field(other) {
                candidates.push(TargetCandidate { id: other, position: there.position });
            }
        }
        let query = TargetQuery {
            position: transform.position,
            yaw: transform.yaw,
            stick: controls.current.stick,
            current: lock.target,
            candidates,
        };
        let by_thumb = controls.current.buttons.contains(Buttons::TOUCH);
        let weights = if by_thumb { weights.on_touch(*strength) } else { &weights.soft_lock };
        lock.target = choose_target(&query, weights);
        if !output.windows.warp {
            *aim = lock.target.map_or(WarpTarget::None, WarpTarget::Actor);
        }
    }
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
            let got = choose(yaw, stick, current, candidates, &TargetingTuning::shipped().soft_lock);
            assert_eq!(got, answer.map(id), "{what}");
        }
    }

    /// Touch's table: closeness first, so where the player faces hardly counts.
    #[test]
    fn auto_target_on_touch_scenes() {
        let touch = TargetingTuning::shipped().touch;
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
        let soft = choose(0.0, pushed(0.0, 1.0), None, scenes[1].4, &TargetingTuning::shipped().soft_lock);
        assert_eq!(soft, Some(id(1)), "the two weights must differ on that scene");
    }

    /// The same scene in every order gives one answer, and an exact tie goes to the lower ID.
    #[test]
    fn order_doesnt_matter_and_ties_go_to_the_lower_id() {
        // Exactly mirrored across the aim, so exactly equal.
        let (a, b, c) = (at(3, 4.0, 3.0), at(7, -4.0, 3.0), at(9, 0.0, -11.0));
        let orders = [[a, b, c], [a, c, b], [b, a, c], [b, c, a], [c, a, b], [c, b, a]];
        for order in &orders {
            for weights in [TargetingTuning::shipped().soft_lock, TargetingTuning::shipped().touch] {
                assert_eq!(choose(0.0, Stick::CENTER, None, order, &weights), Some(id(3)), "{order:?}");
            }
        }
    }

    /// Two enemies at equal angles either side of the aim, each a hair ahead in turn from tick to tick: it
    /// holds who it has. Without the hysteresis it would change every tick.
    #[test]
    fn hysteresis_holds_the_target_between_two_at_equal_angles() {
        let weights = TargetingTuning::shipped().soft_lock;
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
        let weights = TargetingTuning::shipped().soft_lock;
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
        let weights = TargetingTuning::shipped().soft_lock;
        let on_top = TargetCandidate { id: id(1), position: Vec3::ZERO };
        let overhead = TargetCandidate { id: id(2), position: Vec3::new(0.0, 3.0, 0.0) };
        let broken = TargetCandidate { id: id(3), position: Vec3::NAN };
        assert_eq!(choose(0.0, Stick::CENTER, None, &[on_top], &weights), Some(id(1)));
        assert_eq!(choose(0.0, Stick::CENTER, None, &[overhead], &weights), Some(id(2)));
        assert_eq!(choose(0.0, Stick::CENTER, None, &[broken], &weights), None);
        assert_eq!(choose(0.0, Stick::CENTER, None, &[broken, overhead], &weights), Some(id(2)));
    }

    #[test]
    fn the_shipped_document_holds_what_a_soft_lock_and_a_thumb_weigh() {
        let shipped = TargetingTuning::shipped();
        assert_eq!(TargetingTuning::from_ron(SHIPPED).expect("reads"), shipped);
        // A gamepad's stick says who, and on touch closeness counts for most.
        assert!(shipped.soft_lock.direction > shipped.soft_lock.distance);
        assert!(shipped.touch.distance > shipped.touch.direction);
        assert!(shipped.touch.range > shipped.soft_lock.range, "a thumb's reach is longer");
    }

    /// A typo would otherwise leave a number at whatever it was with nothing to say so.
    #[test]
    fn a_document_with_a_field_it_doesnt_know_or_a_bad_number_is_refused_by_name() {
        let unknown = SHIPPED.replacen("hysteresis:", "hystresis:", 1);
        let refused = TargetingTuning::from_ron(&unknown).expect_err("a typo");
        assert!(refused.to_string().contains("hystresis"), "{refused}");
        for (from, to, named) in [
            ("direction: 1.6", "direction: -1.6", "soft_lock.direction"),
            ("range: 14.0", "range: 0.0", "touch.range"),
            ("keep_range: 16.0", "keep_range: 1.0", "soft_lock.keep_range"),
            ("hysteresis: 0.15", "hysteresis: NaN", "touch.hysteresis"),
        ] {
            let refused = TargetingTuning::from_ron(&SHIPPED.replacen(from, to, 1)).expect_err(from);
            assert!(refused.to_string().contains(named), "{from}: {refused}");
        }
    }

    /// Whichever of them moved, the digest says so: this is what stands between other weights and a replay.
    #[test]
    fn the_digest_takes_every_number_of_both_sets_of_weights() {
        let shipped = TargetingTuning::shipped();
        let moves: [fn(&mut TargetingWeights); 6] = [
            |weights| weights.direction += 0.25,
            |weights| weights.facing += 0.25,
            |weights| weights.distance += 0.25,
            |weights| weights.range += 0.25,
            |weights| weights.keep_range += 0.25,
            |weights| weights.hysteresis += 0.25,
        ];
        let normal = AutoTarget::Normal;
        for moved in moves {
            let (mut soft, mut touch, mut weak, mut strong) = (shipped, shipped, shipped, shipped);
            moved(&mut soft.soft_lock);
            moved(&mut touch.touch);
            moved(&mut weak.touch_weak);
            moved(&mut strong.touch_strong);
            assert_ne!(soft.digest(normal), shipped.digest(normal));
            assert_ne!(touch.digest(normal), shipped.digest(normal));
            assert_ne!(soft.digest(normal), touch.digest(normal), "which of them it was counts too");
            assert_ne!(weak.digest(AutoTarget::Weak), shipped.digest(AutoTarget::Weak));
            assert_ne!(strong.digest(AutoTarget::Strong), shipped.digest(AutoTarget::Strong));
            assert_eq!(weak.digest(normal), shipped.digest(normal), "a row the run doesn't read");
            assert_eq!(strong.digest(normal), shipped.digest(normal), "a row the run doesn't read");
        }
        assert_ne!(shipped.digest(AutoTarget::Weak), shipped.digest(normal));
        assert_ne!(shipped.digest(AutoTarget::Strong), shipped.digest(normal));
        for strength in AutoTarget::ALL {
            assert_eq!(AutoTarget::from_raw(strength.to_raw()), Some(strength));
        }
        assert_eq!(AutoTarget::from_raw(3), None);
    }

    /// The strengths pull as their names say: a weak one listens to the stick more than the normal, a strong
    /// one less, and reaches further.
    #[test]
    fn the_strengths_pull_as_they_say() {
        let shipped = TargetingTuning::shipped();
        let pull = |weights: &TargetingWeights| weights.distance / weights.direction;
        assert!(pull(&shipped.touch_weak) < pull(&shipped.touch));
        assert!(pull(&shipped.touch_strong) > pull(&shipped.touch));
        assert!(shipped.touch_strong.range > shipped.touch.range);
        // A thumb pushed at the far one: the weak strength takes it, the strong the near one behind.
        let scene = [at(1, 0.0, 9.0), at(2, 2.0, -2.0)];
        let aimed = pushed(0.0, 1.0);
        assert_eq!(choose(0.0, aimed, None, &scene, &shipped.touch_weak), Some(id(1)));
        assert_eq!(choose(0.0, aimed, None, &scene, &shipped.touch_strong), Some(id(2)));
    }

    /// A game's own document from before the strengths reads, every strength weighing as its `touch`.
    #[test]
    fn a_version_1_document_weighs_every_strength_as_its_touch() {
        let start = SHIPPED.find("        // Weak").expect("the weak row");
        let end = SHIPPED.find("    ),\n)").expect("the document's end");
        let v1 = format!("{}{}", &SHIPPED[..start], &SHIPPED[end..]).replacen("version: 2", "version: 1", 1);
        let read = TargetingTuning::from_ron(&v1).expect("a version 1 document reads");
        let shipped = TargetingTuning::shipped();
        assert_eq!((read.soft_lock, read.touch), (shipped.soft_lock, shipped.touch));
        assert_eq!((read.touch_weak, read.touch_strong), (shipped.touch, shipped.touch));
    }
}
