//! The two solvers everything else here is built on, and the view of a pose they work through.
//!
//! [`SolvePose`] holds a pose's locals and model transforms side by side. A solver reads model
//! transforms, turns joints in model space and writes the turn back to their locals, so whatever
//! runs next ([`CharacterPose::finish`](crate::CharacterPose::finish)) sees an ordinary local pose
//! and the rig's twist helpers follow it. A solver keeps only the joints it names up to date in
//! `model`: everything else hanging from them is stale until the next [`SolvePose::refresh`] or
//! `finish`, which is what makes a solve cost a few joints rather than a skeleton.

use sapphire_assets::skeleton::NO_PARENT;
use sapphire_math::{Affine3A, Mat3A, Quat, Transform, Vec3};

use crate::model::model_of;

/// Bones and vectors shorter than this have no direction to work from.
const TINY: f32 = 1e-6;

/// Below this sine of its bend (about 20°), a chain is straight enough that its own plane stops being
/// trusted alone, and the goal's hint comes in as the bones line up. A bend the wrong way cancels the
/// hint only at half of this, about 10° of hyperextension, which no limb's animation should reach:
/// short of there the hinge never flips.
const STRAIGHT: f32 = 0.34;

/// A pose as solvers see it. See the [module docs](self).
#[derive(Debug)]
pub struct SolvePose<'a> {
    parents: &'a [u16],
    locals: &'a mut [Transform],
    model: &'a mut [Affine3A],
}

impl<'a> SolvePose<'a> {
    /// A view of `locals` and `model`, one entry per joint of a skeleton with `parents`. The joints a
    /// solver is asked to read must already be right in `model`: [`Self::refresh`] makes them so.
    #[must_use]
    pub fn new(parents: &'a [u16], locals: &'a mut [Transform], model: &'a mut [Affine3A]) -> Self {
        Self { parents, locals, model }
    }

    /// `joint`'s place in the model's space.
    #[must_use]
    pub fn position(&self, joint: usize) -> Vec3 {
        Vec3::from(self.model[joint].translation)
    }

    /// `joint`'s rotation in the model's space, whatever scale it carries.
    #[must_use]
    pub fn rotation(&self, joint: usize) -> Quat {
        rotation_of(&self.model[joint])
    }

    /// `joint`'s transform in the model's space.
    #[must_use]
    pub fn transform(&self, joint: usize) -> Affine3A {
        self.model[joint]
    }

    /// `joint`'s local transform.
    #[must_use]
    pub fn local(&self, joint: usize) -> Transform {
        self.locals[joint]
    }

    /// `joint`'s parent's rotation in the model's space.
    fn parent_rotation(&self, joint: usize) -> Quat {
        let parent = self.parents[joint];
        if parent == NO_PARENT { Quat::IDENTITY } else { self.rotation(usize::from(parent)) }
    }

    /// Brings `joints`, listed parents first, up to date from their locals and their parents.
    pub fn refresh(&mut self, joints: &[u16]) {
        for &joint in joints {
            let joint = usize::from(joint);
            self.model[joint] = model_of(self.parents[joint], &self.locals[joint], self.model);
        }
    }

    /// Brings `joint` and all its ancestors up to date, for a joint no list of reads holds, such as
    /// a socket under a hand. Allocates nothing: a chain deeper than a skeleton sensibly goes is
    /// left stale above its 64th joint.
    pub fn refresh_chain(&mut self, joint: usize) {
        let mut chain = [0u16; 64];
        let mut depth = 0;
        let mut at = joint as u16;
        while at != NO_PARENT && depth < chain.len() {
            chain[depth] = at;
            depth += 1;
            at = self.parents[usize::from(at)];
        }
        for &joint in chain[..depth].iter().rev() {
            self.refresh(&[joint]);
        }
    }

    /// The local rotation that puts a turn of `turn` in the model's space on `joint`. Its parent must
    /// be up to date.
    fn turned_local(&self, joint: usize, turn: Quat) -> Quat {
        let parent = self.parent_rotation(joint);
        (parent.inverse() * turn * parent * self.locals[joint].rotation).normalize()
    }

    /// Turns `joint` by `turn`, in the model's space, about its own place. Its parent must be up to
    /// date. Only `joint` itself is kept up to date.
    pub fn turn(&mut self, joint: usize, turn: Quat) {
        self.locals[joint].rotation = self.turned_local(joint, turn);
        self.model[joint] = model_of(self.parents[joint], &self.locals[joint], self.model);
    }

    /// Gives `joint` this rotation in the model's space. Its parent must be up to date.
    pub fn set_rotation(&mut self, joint: usize, rotation: Quat) {
        let turn = rotation * self.rotation(joint).inverse();
        self.turn(joint, turn);
    }

    /// Turns `joint`'s local rotation by `turn`, given in the frame its parent has at rest, whose
    /// rotation in the model's space is `parent_rest`: a turn in rest model space, as VRM's eye bones
    /// take theirs, whatever the joint's own axes. Touches nothing in `model`.
    pub fn turn_at_rest(&mut self, joint: usize, parent_rest: Quat, turn: Quat) {
        let local = &mut self.locals[joint];
        local.rotation = (parent_rest.inverse() * turn * parent_rest * local.rotation).normalize();
    }

    /// Moves `joint` by `by`, in the model's space. Its parent must be up to date. Only `joint` itself
    /// is kept up to date.
    pub fn translate(&mut self, joint: usize, by: Vec3) {
        let parent = self.parents[joint];
        let local = if parent == NO_PARENT {
            by
        } else {
            self.model[usize::from(parent)].matrix3.inverse() * by
        };
        self.locals[joint].translation += local;
        self.model[joint] = model_of(self.parents[joint], &self.locals[joint], self.model);
    }

    /// Moves `joint`'s model transform by `carried`: what a turn of something above it did to it, so
    /// its local is still right.
    fn carry(&mut self, joint: usize, carried: Affine3A) {
        self.model[joint] = carried * self.model[joint];
    }
}

/// The rotation in `model`, with any scale taken out.
#[must_use]
pub(crate) fn rotation_of(model: &Affine3A) -> Quat {
    let m = model.matrix3;
    Quat::from_mat3a(&Mat3A::from_cols(m.x_axis.normalize(), m.y_axis.normalize(), m.z_axis.normalize()))
        .normalize()
}

/// A turn of `turn` about `pivot`, as a transform.
fn about(turn: Quat, pivot: Vec3) -> Affine3A {
    Affine3A::from_rotation_translation(turn, pivot - turn * pivot)
}

/// `Quat::IDENTITY` to `turn`, by `weight`, exact at the ends.
pub(crate) fn weighted(turn: Quat, weight: f32) -> Quat {
    if weight >= 1.0 { turn } else { Quat::IDENTITY.slerp(turn, weight.max(0.0)) }
}

/// A chain of two bones: a hip or shoulder, a knee or elbow, an ankle or wrist. `mid` hangs somewhere
/// below `root` and `end` somewhere below `mid`; joints between them just ride along, which is how a
/// rig's twist helpers can sit in a limb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TwoBone {
    pub root: usize,
    pub mid: usize,
    pub end: usize,
}

/// Where a [`TwoBone`] should go, in the model's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwoBoneGoal {
    /// Where the chain's end should arrive.
    pub target: Vec3,
    /// The direction the middle joint should bend toward, from the line between the chain's ends.
    /// `None` keeps the plane the pose already bends in, which is what keeps a clip's knees: a limb's
    /// solve then changes as little of the animation as it can.
    pub pole: Option<Vec3>,
    /// Which way the middle joint bends when the pose can't say, its bones nearly in a line: a knee's
    /// forward, an elbow's back. It fades in below about 20° of bend, so a limb straightening passes
    /// from its own plane to this one with no jump.
    pub hint: Vec3,
    /// The share of the chain's length, 0 to 1, past which a reach eases off rather than going on to
    /// a straight limb, which pops. 1 reaches exactly as far as the bones go.
    pub soften: f32,
    /// 0 leaves the pose as it is and 1 applies the whole solve.
    pub weight: f32,
}

/// Turns `chain` so its end reaches `goal`'s target, and returns how far short of it the end stopped.
/// Keeps `chain`'s three joints up to date in the view, and nothing else. Allocates nothing.
///
/// A target out of reach ends with the limb pointing at it; one inside the shorter bone's circle with
/// the limb folded as far as the bones allow. The bones keep their lengths.
pub fn two_bone(pose: &mut SolvePose<'_>, chain: TwoBone, goal: &TwoBoneGoal) -> f32 {
    let (a, b, c) = (pose.position(chain.root), pose.position(chain.mid), pose.position(chain.end));
    let short = goal.target.distance(c);
    if goal.weight <= 0.0 {
        return short;
    }
    let (upper, lower) = (b - a, c - b);
    let (upper_length, lower_length) = (upper.length(), lower.length());
    if upper_length < TINY || lower_length < TINY {
        return short;
    }
    let (along_upper, along_lower) = (upper / upper_length, lower / lower_length);
    let to_target = goal.target - a;
    let distance = to_target.length();
    let direction = if distance > TINY { to_target / distance } else { (c - a).normalize_or(along_upper) };

    // How far the chain will reach: softly out to the bones' length, and no nearer than they fold.
    // What the pose already reaches is never softened, so a target where the end already is moves
    // nothing, and a solve fading in starts from the pose.
    let length = upper_length + lower_length;
    let reach = soften(distance, length, goal.soften.max((c - a).length() / length))
        .max((upper_length - lower_length).abs() + TINY);

    // Bend the middle joint until the ends are that far apart, in the plane the pose bends in.
    let bend = along_upper.cross(along_lower);
    let hinted = goal.hint.cross(along_upper).normalize_or_zero();
    let hinge = (bend + hinted * (STRAIGHT - bend.length()).max(0.0))
        .try_normalize()
        .unwrap_or_else(|| along_upper.any_orthonormal_vector());
    let (sine, cosine) = ((upper_length * upper_length + lower_length * lower_length - reach * reach)
        / (2.0 * upper_length * lower_length))
        .clamp(-1.0, 1.0)
        .acos()
        .sin_cos();
    let bent = along_upper * -cosine + hinge.cross(along_upper) * sine;
    let mid_turn = Quat::from_rotation_arc(along_lower, bent);

    // Then swing the whole chain until the end lies on the line to the target.
    let ended = upper + mid_turn * lower;
    let from = ended.normalize_or(direction);
    let mut root_turn = Quat::from_rotation_arc(from, direction);
    if let Some(pole) = goal.pole {
        let have = (root_turn * upper).reject_from_normalized(direction).normalize_or_zero();
        let want = pole.reject_from_normalized(direction).normalize_or_zero();
        if have != Vec3::ZERO && want != Vec3::ZERO {
            let angle = direction.dot(have.cross(want)).atan2(have.dot(want));
            root_turn = (Quat::from_axis_angle(direction, angle) * root_turn).normalize();
        }
    }
    let (mid_turn, root_turn) = (weighted(mid_turn, goal.weight), weighted(root_turn, goal.weight));

    // Locals, the middle joint's first: its parent's rotation is still the one its local hangs from.
    pose.locals[chain.mid].rotation = pose.turned_local(chain.mid, mid_turn);
    pose.locals[chain.root].rotation = pose.turned_local(chain.root, root_turn);
    let carried_root = about(root_turn, a);
    let carried_mid = carried_root * about(mid_turn, b);
    pose.carry(chain.root, carried_root);
    pose.carry(chain.mid, carried_mid);
    pose.carry(chain.end, carried_mid);
    goal.target.distance(pose.position(chain.end))
}

/// `distance` eased off past `soften` of `length`, approaching `length` and never passing it.
fn soften(distance: f32, length: f32, soften: f32) -> f32 {
    let start = length * soften.clamp(0.0, 1.0);
    let room = length - start;
    if distance <= start || room <= TINY {
        return distance.min(length);
    }
    start + room * (1.0 - (-(distance - start) / room).exp())
}

/// What an [`aim`] does to its joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimGoal {
    /// What to point at, in the model's space.
    pub target: Vec3,
    /// The axis to point, in the joint's own space, unit length.
    pub forward: Vec3,
    /// Where that axis starts, in the joint's own space: an eye's or a muzzle's offset from its
    /// joint, which the turn accounts for, so the ray itself passes through the target.
    pub offset: Vec3,
    /// A roll to settle afterward: which of the joint's own axes (`.0`, unit length) should lean toward
    /// which direction in the model's space (`.1`). `None` takes the shortest turn, with no roll of
    /// its own.
    pub up: Option<(Vec3, Vec3)>,
    /// 0 leaves the joint as it is and 1 applies the whole turn.
    pub weight: f32,
}

/// Turns `joint` so `goal`'s axis points at its target, and returns the turn in the model's space. Its
/// parent must be up to date, and only `joint` is kept so. Allocates nothing. A target too close to
/// tell, or on the joint itself, turns it no further than the offset allows.
pub fn aim(pose: &mut SolvePose<'_>, joint: usize, goal: &AimGoal) -> Quat {
    let (position, rotation) = (pose.position(joint), pose.rotation(joint));
    let local = rotation.inverse() * (goal.target - position);
    let Some(direction) = local.try_normalize() else { return Quat::IDENTITY };
    // The ray from the offset reaches the target's distance after `along`, on the joint's sphere
    // through the target. Inside the offset there is no such point, and the axis itself is the best
    // there is.
    let side = goal.offset.dot(goal.forward);
    let room = side * side - goal.offset.length_squared() + local.length_squared();
    let ray = if room >= 0.0 {
        (goal.offset + goal.forward * (room.sqrt() - side)).normalize_or(goal.forward)
    } else {
        goal.forward
    };
    let mut turn = rotation * Quat::from_rotation_arc(ray, direction) * rotation.inverse();
    if let Some((axis, toward)) = goal.up {
        let line = (goal.target - position).normalize_or(Vec3::Y);
        let have = (turn * rotation * axis).reject_from_normalized(line).normalize_or_zero();
        let want = toward.reject_from_normalized(line).normalize_or_zero();
        if have != Vec3::ZERO && want != Vec3::ZERO {
            let angle = line.dot(have.cross(want)).atan2(have.dot(want));
            turn = (Quat::from_axis_angle(line, angle) * turn).normalize();
        }
    }
    let turn = weighted(turn, goal.weight);
    if goal.weight > 0.0 {
        pose.turn(joint, turn);
    }
    turn
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    use super::*;

    /// A straight arm along +X from a shoulder at the origin: upper 0.3, lower 0.25, and a joint
    /// riding between the middle and the end.
    fn straight() -> (Vec<u16>, Vec<Transform>, Vec<Affine3A>) {
        let parents = vec![NO_PARENT, 0, 1, 2];
        let locals = vec![
            Transform::IDENTITY,
            Transform::from_translation(Vec3::new(0.3, 0.0, 0.0)),
            Transform::from_translation(Vec3::new(0.1, 0.0, 0.0)),
            Transform::from_translation(Vec3::new(0.15, 0.0, 0.0)),
        ];
        let model = vec![Affine3A::IDENTITY; 4];
        (parents, locals, model)
    }

    const CHAIN: TwoBone = TwoBone { root: 0, mid: 1, end: 3 };
    const ALL: [u16; 4] = [0, 1, 2, 3];

    fn goal(target: Vec3) -> TwoBoneGoal {
        TwoBoneGoal { target, pole: None, hint: Vec3::Y, soften: 1.0, weight: 1.0 }
    }

    /// What the locals say, walked from the top: what `finish` will see.
    fn walked(parents: &[u16], locals: &mut [Transform], model: &mut [Affine3A]) -> Vec<Vec3> {
        let mut pose = SolvePose::new(parents, locals, model);
        pose.refresh(&ALL);
        ALL.iter().map(|&joint| pose.position(usize::from(joint))).collect()
    }

    #[test]
    fn it_reaches_a_target_within_reach_keeping_its_bones() {
        let (parents, mut locals, mut model) = straight();
        for target in [
            Vec3::new(0.3, 0.2, 0.0),
            Vec3::new(0.0, 0.3, 0.3),
            Vec3::new(-0.2, -0.1, 0.25),
            Vec3::new(0.1, 0.0, 0.0),
        ] {
            let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
            pose.refresh(&ALL);
            let short = two_bone(&mut pose, CHAIN, &goal(target));
            assert!(short < 1e-4, "{target}: {short}");
            let joints = walked(&parents, &mut locals, &mut model);
            assert!((joints[3] - target).length() < 1e-4, "the locals agree: {}", joints[3]);
            assert!((joints[1].distance(joints[0]) - 0.3).abs() < 1e-5);
            assert!((joints[3].distance(joints[1]) - 0.25).abs() < 1e-5);
            assert!((joints[2].distance(joints[1]) - 0.1).abs() < 1e-5, "what rides along rides");
        }
    }

    #[test]
    fn a_target_out_of_reach_points_the_limb_at_it() {
        let (parents, mut locals, mut model) = straight();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&ALL);
        let target = Vec3::new(0.0, 2.0, 0.0);
        let short = two_bone(&mut pose, CHAIN, &goal(target));
        assert!((short - 1.45).abs() < 1e-4, "{short}");
        assert!(pose.position(3).abs_diff_eq(Vec3::new(0.0, 0.55, 0.0), 1e-4), "{}", pose.position(3));
    }

    #[test]
    fn a_target_inside_the_fold_folds_the_limb_as_far_as_it_goes() {
        let (parents, mut locals, mut model) = straight();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&ALL);
        // The bones are 0.3 and 0.25, so the end can't come nearer than 0.05.
        let short = two_bone(&mut pose, CHAIN, &goal(Vec3::new(0.01, 0.0, 0.0)));
        assert!((pose.position(3).length() - 0.05).abs() < 1e-3, "{}", pose.position(3));
        assert!(short < 0.05);
        for joint in 0..4 {
            assert!(pose.position(joint).is_finite());
            assert!(pose.local(joint).rotation.is_normalized());
        }
    }

    /// A target on the root itself has no direction to point at.
    #[test]
    fn a_target_on_the_root_stays_finite() {
        let (parents, mut locals, mut model) = straight();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&ALL);
        two_bone(&mut pose, CHAIN, &goal(Vec3::ZERO));
        assert!(pose.position(3).is_finite());
        assert!(pose.local(0).rotation.is_normalized());
    }

    #[test]
    fn a_straight_limb_bends_toward_its_hint_and_a_pole_overrules_it() {
        for (hint, pole, side) in [
            (Vec3::Y, None, Vec3::Y),
            (Vec3::NEG_Y, None, Vec3::NEG_Y),
            (Vec3::Z, None, Vec3::Z),
            (Vec3::Y, Some(Vec3::NEG_Z), Vec3::NEG_Z),
        ] {
            let (parents, mut locals, mut model) = straight();
            let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
            pose.refresh(&ALL);
            let target = Vec3::new(0.4, 0.0, 0.0);
            two_bone(&mut pose, CHAIN, &TwoBoneGoal { hint, pole, ..goal(target) });
            let elbow = pose.position(1);
            assert!(pose.position(3).abs_diff_eq(target, 1e-4));
            assert!(elbow.dot(side) > 0.1, "{hint} {pole:?}: the elbow is at {elbow}");
            assert!(elbow.reject_from(side).x.abs() < elbow.x + 1e-3);
        }
    }

    /// With no pole, a solve keeps the plane the pose bends in: a clip's knee goes where the clip had
    /// it pointing, whichever way the hint says.
    #[test]
    fn a_bent_limb_keeps_its_own_plane() {
        let (parents, mut locals, mut model) = straight();
        locals[1].rotation = Quat::from_rotation_y(-1.0);
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&ALL);
        let before = pose.position(3);
        // It bends in the XZ plane. A new target in that plane must leave the elbow in it too.
        let target = before.lerp(Vec3::ZERO, 0.2);
        two_bone(&mut pose, CHAIN, &TwoBoneGoal { hint: Vec3::Y, ..goal(target) });
        assert!(pose.position(3).abs_diff_eq(target, 1e-4));
        assert!(pose.position(1).y.abs() < 1e-4, "stays in its plane: {}", pose.position(1));
    }

    /// Crossing "straight" is the case a hinge taken from the bones alone gets wrong: the hinge flips
    /// from one frame to the next and the elbow jumps.
    #[test]
    fn a_limb_passing_through_straight_never_jumps() {
        let mut last = None;
        for step in 0..=200 {
            let (parents, mut locals, mut model) = straight();
            // The animation's bend runs from slightly the wrong way to slightly the right way.
            let bend = -0.1 + 0.2 * step as f32 / 200.0;
            locals[1].rotation = Quat::from_rotation_z(bend);
            let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
            pose.refresh(&ALL);
            let target = Vec3::new(0.45, 0.0, 0.0);
            two_bone(&mut pose, CHAIN, &goal(target));
            assert!(pose.position(3).abs_diff_eq(target, 1e-4), "step {step}: {}", pose.position(3));
            let elbow = pose.position(1);
            if let Some(last) = last {
                let moved = elbow.distance(last);
                assert!(moved < 0.012, "step {step}: the elbow jumped {moved} m to {elbow}");
            }
            last = Some(elbow);
        }
    }

    /// Softening turns the last of a reach into an approach: the end never meets a straight limb at
    /// speed, and a target well inside it isn't touched.
    #[test]
    fn softening_eases_the_last_of_a_reach() {
        assert_eq!(soften(0.5, 1.0, 0.9), 0.5, "inside its start");
        assert_eq!(soften(5.0, 1.0, 1.0), 1.0, "none");
        let mut last = 0.9;
        for step in 1..1000 {
            let distance = 0.9 + 0.003 * step as f32;
            let eased = soften(distance, 1.0, 0.9);
            assert!(eased <= 1.0, "it never passes the bones' length: {distance} gave {eased}");
            assert!(eased >= last, "it keeps going");
            assert!(eased - last < 0.0031, "and never faster than the target");
            last = eased;
        }
        assert!(last > 0.999);
    }

    #[test]
    fn weight_blends_from_the_pose_and_none_leaves_its_bits() {
        let (parents, mut locals, mut model) = straight();
        let before = locals.clone();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&ALL);
        let target = Vec3::new(0.0, 0.4, 0.0);
        two_bone(&mut pose, CHAIN, &TwoBoneGoal { weight: 0.0, ..goal(target) });
        assert!(pose.position(3).abs_diff_eq(Vec3::new(0.55, 0.0, 0.0), 1e-6));
        assert_eq!(pose.local(0), before[0]);
        assert_eq!(pose.local(1), before[1]);
        // Half way there's somewhere between, off the line.
        two_bone(&mut pose, CHAIN, &TwoBoneGoal { weight: 0.5, ..goal(target) });
        let end = pose.position(3);
        assert!(end.distance(target) > 0.05 && end.distance(Vec3::new(0.55, 0.0, 0.0)) > 0.05, "{end}");
    }

    /// A parent's rotation and scale change nothing about where the end arrives.
    #[test]
    fn it_works_under_a_turned_and_scaled_parent() {
        let parents = vec![NO_PARENT, 0, 1, 2];
        let mut locals = vec![
            Transform {
                translation: Vec3::new(0.2, 1.0, -0.4),
                rotation: Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.3),
                scale: Vec3::splat(0.8),
            },
            Transform::from_translation(Vec3::new(0.0, -0.4, 0.0)),
            Transform::from_translation(Vec3::new(0.0, -0.4, 0.0)),
            Transform::from_translation(Vec3::new(0.0, -0.1, 0.05)),
        ];
        let mut model = vec![Affine3A::IDENTITY; 4];
        let chain = TwoBone { root: 1, mid: 2, end: 3 };
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&ALL);
        let target = pose.position(3) + Vec3::new(0.05, 0.08, 0.02);
        two_bone(&mut pose, chain, &TwoBoneGoal { hint: Vec3::Z, ..goal(target) });
        assert!(pose.position(3).abs_diff_eq(target, 1e-4), "{} against {target}", pose.position(3));
        let joints = walked(&parents, &mut locals, &mut model);
        assert!(joints[3].abs_diff_eq(target, 1e-4), "the locals agree: {}", joints[3]);
    }

    fn head() -> (Vec<u16>, Vec<Transform>, Vec<Affine3A>) {
        let parents = vec![NO_PARENT, 0];
        let locals = vec![
            Transform::from_translation(Vec3::new(0.0, 1.5, 0.0)),
            Transform {
                translation: Vec3::new(0.0, 0.1, 0.0),
                rotation: Quat::from_rotation_z(FRAC_PI_2),
                scale: Vec3::ONE,
            },
        ];
        (parents, locals, vec![Affine3A::IDENTITY; 2])
    }

    fn aim_goal(target: Vec3) -> AimGoal {
        // The joint's own +X is up after its rest turn, and its own +Z forward.
        AimGoal { target, forward: Vec3::Z, offset: Vec3::ZERO, up: None, weight: 1.0 }
    }

    #[test]
    fn aim_points_an_axis_at_a_target_under_any_rest_rotation() {
        let (parents, mut locals, mut model) = head();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&[0, 1]);
        let target = Vec3::new(2.0, 2.5, 3.0);
        aim(&mut pose, 1, &aim_goal(target));
        let along = pose.rotation(1) * Vec3::Z;
        let wanted = (target - pose.position(1)).normalize();
        assert!(along.abs_diff_eq(wanted, 1e-4), "{along} against {wanted}");
    }

    /// The offset ray itself passes through the target, not the axis through the joint.
    #[test]
    fn aim_accounts_for_the_offset_of_what_it_aims() {
        let (parents, mut locals, mut model) = head();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&[0, 1]);
        let target = Vec3::new(0.3, 1.8, 1.0);
        let offset = Vec3::new(0.06, 0.03, 0.1);
        aim(&mut pose, 1, &AimGoal { offset, ..aim_goal(target) });
        let (at, rotation) = (pose.position(1), pose.rotation(1));
        let origin = at + rotation * offset;
        let along = rotation * Vec3::Z;
        let line = target - origin;
        assert!(line.normalize().abs_diff_eq(along, 1e-3), "{line} against {along}");
    }

    #[test]
    fn aim_settles_its_roll_when_asked() {
        let (parents, mut locals, mut model) = head();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&[0, 1]);
        let target = Vec3::new(1.0, 1.6, 1.0);
        let up = Some((Vec3::X, Vec3::Y));
        aim(&mut pose, 1, &AimGoal { up, ..aim_goal(target) });
        let rotation = pose.rotation(1);
        let along = rotation * Vec3::Z;
        assert!(along.abs_diff_eq((target - pose.position(1)).normalize(), 1e-4));
        // Its own +X leans as far up as it can, square to the aim.
        let upward = rotation * Vec3::X;
        let best = Vec3::Y.reject_from(along).normalize();
        assert!(upward.abs_diff_eq(best, 1e-4), "{upward} against {best}");
    }

    #[test]
    fn aim_blends_by_weight_and_a_target_on_the_joint_does_nothing() {
        let (parents, mut locals, mut model) = head();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&[0, 1]);
        let target = pose.position(1) + Vec3::X;
        let start = pose.rotation(1);
        let turn = aim(&mut pose, 1, &AimGoal { weight: 0.5, ..aim_goal(target) });
        assert!(turn.abs_diff_eq(Quat::from_rotation_y(FRAC_PI_4), 1e-4), "half of a quarter: {turn}");
        assert!(pose.rotation(1).abs_diff_eq(turn * start, 1e-4));
        let here = pose.position(1);
        let on_it = aim(&mut pose, 1, &aim_goal(here));
        assert_eq!(on_it, Quat::IDENTITY);
    }

    #[test]
    fn a_turn_and_a_move_land_on_the_locals_under_a_turned_parent() {
        let parents = vec![NO_PARENT, 0];
        let mut locals = vec![
            Transform {
                translation: Vec3::new(1.0, 0.0, 0.0),
                rotation: Quat::from_rotation_y(FRAC_PI_2),
                scale: Vec3::splat(2.0),
            },
            Transform::from_translation(Vec3::new(0.0, 0.5, 0.0)),
        ];
        let mut model = vec![Affine3A::IDENTITY; 2];
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh(&[0, 1]);
        pose.translate(1, Vec3::new(0.0, -0.25, 0.0));
        assert!(pose.position(1).abs_diff_eq(Vec3::new(1.0, 0.75, 0.0), 1e-5), "{}", pose.position(1));
        pose.turn(1, Quat::from_rotation_x(0.5));
        let wanted = Quat::from_rotation_x(0.5) * Quat::from_rotation_y(FRAC_PI_2);
        assert!(pose.rotation(1).abs_diff_eq(wanted, 1e-5));
        pose.set_rotation(1, Quat::from_rotation_z(0.25));
        assert!(pose.rotation(1).abs_diff_eq(Quat::from_rotation_z(0.25), 1e-5));
        let (at, rotation) = (pose.position(1), pose.rotation(1));
        pose.refresh(&[0, 1]);
        assert!(pose.position(1).abs_diff_eq(at, 1e-6));
        assert!(pose.rotation(1).abs_diff_eq(rotation, 1e-6));
    }

    #[test]
    fn refreshing_a_chain_reaches_its_ancestors() {
        let (parents, mut locals, mut model) = straight();
        let mut pose = SolvePose::new(&parents, &mut locals, &mut model);
        pose.refresh_chain(3);
        assert!(pose.position(3).abs_diff_eq(Vec3::new(0.55, 0.0, 0.0), 1e-6));
    }
}
