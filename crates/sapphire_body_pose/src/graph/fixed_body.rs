//! Isolated fixed-step body evaluation and immutable publication.

mod motion;

pub use motion::{BodyMotionBound, BodyMotionError};

use std::sync::Arc;

use sapphire_math::{Affine3A, Mat4, Transform, Vec3, det};

use super::canonical::Canonical;
use super::{ClipSource, FixedGraphHistory, GraphHistoryError, GraphInput, GraphStepStamp};
use crate::{
    AuthoredHeadLook, CharacterIk, CharacterPose, FeetInput, FootContact, FootSettings, GroundHit,
    GroundTurn, HeldWeapon, IkInput, JointLayout, LookInput, SolvedBody, graph_foot_contact, model,
};

/// The controller's world-space body placement at one fixed step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyRoot {
    pub translation: Vec3,
    /// Unwrapped yaw from the controller, in radians.
    pub yaw: f32,
    /// The rig's uniform scale.
    pub scale: f32,
}

impl BodyRoot {
    fn valid(self) -> bool {
        self.translation.is_finite() && self.yaw.is_finite() && self.scale.is_finite() && self.scale > 0.0
    }

    fn lerp(self, to: Self, fraction: f32) -> Self {
        Self {
            translation: self.translation.lerp(to.translation, fraction),
            yaw: self.yaw + (to.yaw - self.yaw) * fraction,
            scale: self.scale + (to.scale - self.scale) * fraction,
        }
    }

    /// Model placement shared by pose consumers.
    #[must_use]
    pub fn affine(self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(
            Vec3::splat(self.scale),
            det::quat_from_rotation_y(self.yaw),
            self.translation,
        )
    }

    fn write_canonical(self, out: &mut Canonical<'_, impl FnMut(&[u8])>) {
        out.vec3(self.translation);
        out.floats(&[self.yaw, self.scale]);
    }
}

/// Pre-resolution fixed inputs for one body. Owner time and cut come from the stamp.
#[derive(Clone, Copy, Debug)]
pub struct BodyStep {
    pub stamp: GraphStepStamp,
    pub graph: GraphInput,
    pub root: BodyRoot,
    pub movement_released: bool,
    pub fallback_contact: FootContact,
    pub look: LookInput,
    pub weapon: Option<HeldWeapon>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BodyStepError {
    #[error(transparent)]
    Graph(#[from] GraphHistoryError),
    #[error("invalid body placement")]
    Root,
}

/// Optional body policies selected with the immutable rig binding.
#[derive(Default)]
pub struct FixedBodyPolicy {
    pub ground_turn: bool,
    pub spine: Option<usize>,
    pub head_look: Option<AuthoredHeadLook>,
}

struct BodyLayout {
    joints: Arc<JointLayout>,
    morph_names: Option<Vec<String>>,
    rest_weights: Vec<f32>,
    protected: Arc<[bool]>,
}

struct BodyFrame {
    stamp: GraphStepStamp,
    pose_step: u64,
    root: BodyRoot,
    contact: Option<FootContact>,
    grounded: bool,
    locals: Vec<Transform>,
    model: Vec<Affine3A>,
    morphs: Vec<f32>,
}

impl BodyFrame {
    fn blank(joints: usize, morphs: usize) -> Self {
        Self {
            stamp: GraphStepStamp { step: 0, owner_now: sapphire_time::SimTime::ZERO, generation: 0 },
            pose_step: 0,
            root: BodyRoot { translation: Vec3::ZERO, yaw: 0.0, scale: 1.0 },
            contact: None,
            grounded: false,
            locals: vec![Transform::IDENTITY; joints],
            model: vec![Affine3A::IDENTITY; joints],
            morphs: vec![0.0; morphs],
        }
    }

    fn copy_solved(&mut self, body: SolvedBody<'_>) {
        self.locals.copy_from_slice(body.locals());
        self.model.copy_from_slice(body.model());
        self.morphs.copy_from_slice(body.morphs().1);
    }

    fn copy_pose(&mut self, other: &Self) {
        self.locals.copy_from_slice(&other.locals);
        self.model.copy_from_slice(&other.model);
        self.morphs.copy_from_slice(&other.morphs);
    }

    fn write_canonical(&self, out: &mut Canonical<'_, impl FnMut(&[u8])>) {
        self.stamp.write_canonical(out.0);
        out.u64(self.pose_step);
        self.root.write_canonical(out);
        out.optional(self.contact, |out, contact| contact.write_canonical(out));
        out.flag(self.grounded);
        out.transforms(&self.locals);
        out.len(self.morphs.len());
        out.floats(&self.morphs);
        // The published model is derived from these solved locals and the immutable hierarchy.
    }
}

/// The exact fixed-step endpoints and fraction used for a body read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyReadSpan {
    pub before: GraphStepStamp,
    pub after: GraphStepStamp,
    pub fraction: f32,
}

/// A preallocated body read. Cosmetic overrides can affect only joints outside the protected
/// contact ancestor closure. They change this consumer buffer, never a fixed-step endpoint.
pub struct BodySample {
    layout: Arc<BodyLayout>,
    span: Option<BodyReadSpan>,
    pose_key: Option<(u64, u64, f32)>,
    root: BodyRoot,
    grounded: bool,
    locals: Vec<Transform>,
    model: Vec<Affine3A>,
    morphs: Vec<f32>,
}

impl BodySample {
    /// Copies an already solved pose into a reusable read with the same immutable layout.
    pub fn copy_from_sample(&mut self, source: &Self) -> bool {
        if source.span.is_none() || !Arc::ptr_eq(&self.layout, &source.layout) {
            return false;
        }
        self.span = source.span;
        self.pose_key = source.pose_key;
        self.root = source.root;
        self.grounded = source.grounded;
        self.locals.copy_from_slice(&source.locals);
        self.model.copy_from_slice(&source.model);
        self.morphs.copy_from_slice(&source.morphs);
        true
    }

    /// Decays a retained impact offset on a presentation read. The source poses remain immutable.
    /// This uses the normal constrained-local interpolation and FK without solving constraints again.
    pub fn apply_impact_residual(&mut self, held: &Self, origin: &Self, retained: f32) -> bool {
        let (Some(live), Some(contact), Some(base)) = (self.span, held.span, origin.span) else {
            return false;
        };
        if !Arc::ptr_eq(&self.layout, &held.layout)
            || !Arc::ptr_eq(&self.layout, &origin.layout)
            || live.after.generation != contact.after.generation
            || base.before.generation != contact.after.generation
            || base.before.owner_now != contact.after.owner_now
            || self.root.scale.to_bits() != held.root.scale.to_bits()
            || self.root.scale.to_bits() != origin.root.scale.to_bits()
            || !(0.0..=1.0).contains(&retained)
        {
            return false;
        }
        if retained == 0.0 {
            return true;
        }
        if retained == 1.0 {
            let key = self.pose_key;
            self.copy_from_sample(held);
            self.span = Some(live);
            self.pose_key = key;
            return true;
        }
        if held.locals.iter().zip(&origin.locals).any(|(hold, base)| {
            hold.scale != base.scale
                && (base.scale.min_element() <= 0.0
                    || hold.scale.min_element() <= 0.0
                    || !(hold.scale / base.scale).is_finite())
        }) {
            return false;
        }
        if held.root.translation != origin.root.translation {
            self.root.translation += (held.root.translation - origin.root.translation) * retained;
        }
        if held.root.yaw != origin.root.yaw {
            self.root.yaw += (held.root.yaw - origin.root.yaw) * retained;
        }
        let mut locals_changed = false;
        for ((local, &hold), &base) in self.locals.iter_mut().zip(&held.locals).zip(&origin.locals) {
            if hold == base {
                continue;
            }
            let corrected = Transform {
                translation: local.translation + (hold.translation - base.translation),
                rotation: if hold.rotation == base.rotation {
                    local.rotation
                } else {
                    let offset = det::quat_mul(hold.rotation, det::quat_conjugate(base.rotation));
                    det::quat_mul(offset, local.rotation)
                },
                scale: if hold.scale == base.scale {
                    local.scale
                } else {
                    local.scale * (hold.scale / base.scale)
                },
            };
            let blended = motion::interpolate_local(*local, corrected, retained);
            if hold.translation != base.translation {
                local.translation = blended.translation;
            }
            if hold.rotation != base.rotation {
                local.rotation = blended.rotation;
            }
            if hold.scale != base.scale {
                local.scale = blended.scale;
            }
            locals_changed = true;
        }
        if locals_changed {
            model::walk(self.layout.joints.parents.iter().copied(), &self.locals, &mut self.model);
        }
        for ((weight, &hold), &base) in self.morphs.iter_mut().zip(&held.morphs).zip(&origin.morphs) {
            if hold != base {
                let shifted = *weight + (hold - base) * retained;
                *weight = if [*weight, hold, base].iter().all(|weight| (0.0..=1.0).contains(weight)) {
                    shifted.clamp(0.0, 1.0)
                } else {
                    shifted
                };
            }
        }
        true
    }

    /// Identity of the solved pose(s) and mix used by a presentation read.
    #[must_use]
    pub fn pose_key(&self) -> Option<(u64, u64, f32)> {
        self.pose_key
    }

    /// Rig joints protected from secondary motion.
    #[must_use]
    pub fn protected_joints(&self) -> &[bool] {
        &self.layout.protected
    }

    /// Checks that a cosmetic consumer left the authoritative locals, model and morphs intact.
    #[must_use]
    pub fn protected_matches(&self, pose: &CharacterPose) -> bool {
        self.protected_matches_with_morph(pose, None)
    }

    /// Checks the contact closure and authored morphs, allowing one validated
    /// presentation-only slot whose authoritative value is exactly zero.
    #[must_use]
    pub fn protected_matches_with_morph(&self, pose: &CharacterPose, cosmetic_morph: Option<usize>) -> bool {
        let same = |a: &[f32], b: &[f32]| {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits())
        };
        cosmetic_morph.is_none_or(|slot| slot < self.morphs.len())
            && self.morphs.len() == pose.weights().len()
            && self.morphs.iter().zip(pose.weights()).enumerate().all(|(index, (a, b))| {
                if Some(index) == cosmetic_morph {
                    a.to_bits() == 0 && (0.0..=1.0).contains(b)
                } else {
                    a.to_bits() == b.to_bits()
                }
            })
            && self.layout.protected.iter().enumerate().all(|(joint, &protected)| {
                if !protected {
                    return true;
                }
                let (a, b) = (self.locals[joint], pose.locals()[joint]);
                same(&a.translation.to_array(), &b.translation.to_array())
                    && same(&a.rotation.to_array(), &b.rotation.to_array())
                    && same(&a.scale.to_array(), &b.scale.to_array())
                    && same(&self.model[joint].to_cols_array(), &pose.model()[joint].to_cols_array())
            })
    }

    /// The endpoint pair used for this read. A cut has the new stamp at both ends.
    #[must_use]
    pub fn span(&self) -> Option<BodyReadSpan> {
        self.span
    }

    #[must_use]
    pub fn root(&self) -> BodyRoot {
        self.root
    }

    #[must_use]
    pub fn joint_names(&self) -> &[String] {
        &self.layout.joints.names
    }

    #[must_use]
    pub fn locals(&self) -> &[Transform] {
        &self.locals
    }

    #[must_use]
    pub fn model(&self) -> &[Affine3A] {
        &self.model
    }

    #[must_use]
    pub fn morphs(&self) -> (Option<&[String]>, &[f32]) {
        (self.layout.morph_names.as_deref(), &self.morphs)
    }

    /// The newest fixed controller's grounded flag, even on a fractional presentation read.
    #[must_use]
    pub fn grounded(&self) -> bool {
        self.grounded
    }

    /// Binds an independently constructed presentation pose to the same immutable cooked layout.
    /// Call once before drawing. A raw joint, parent, rest or morph mismatch refuses the binding.
    pub fn bind_presentation(&self, pose: &mut CharacterPose) -> bool {
        pose.morph_names() == self.layout.morph_names.as_deref()
            && pose.rest_weights().iter().map(|w| w.to_bits()).eq(self
                .layout
                .rest_weights
                .iter()
                .map(|w| w.to_bits()))
            && pose.bind_body_layout(&self.layout.joints)
    }

    /// Copies this bound read to a presentation pose without rerunning body constraints or IK.
    /// Returns false on a mismatched joint layout or morph channel count.
    pub fn publish_to(&self, pose: &mut CharacterPose) -> bool {
        self.span.is_some()
            && pose.morph_names() == self.layout.morph_names.as_deref()
            && pose.adopt_body_sample(
                &self.layout.joints,
                &self.layout.protected,
                &self.locals,
                &self.model,
                &self.morphs,
            )
    }

    /// Applies allowed cosmetic locals in one pass. Protected or out-of-range writes are skipped.
    /// Only unprotected model transforms are recomputed, without reapplying rig constraints.
    pub fn apply_cosmetic_locals(&mut self, updates: impl IntoIterator<Item = (usize, Transform)>) -> usize {
        let mut first = self.locals.len();
        let mut applied = 0;
        for (joint, local) in updates {
            if self.layout.protected.get(joint) == Some(&false) {
                self.locals[joint] = local;
                first = first.min(joint);
                applied += 1;
            }
        }
        for joint in first..self.locals.len() {
            if !self.layout.protected[joint] {
                self.model[joint] =
                    model::model_of(self.layout.joints.parents[joint], &self.locals[joint], &self.model);
            }
        }
        applied
    }
}

/// One owner of graph, support, look and the last two constrained body frames. The rig and clip
/// binding are immutable. There is no renderer or gameplay contact dependency.
pub struct FixedBody<C: ClipSource> {
    graph: FixedGraphHistory<C>,
    ik: CharacterIk,
    turn: Option<GroundTurn>,
    policy: FixedBodyPolicy,
    layout: Arc<BodyLayout>,
    previous: BodyFrame,
    current: BodyFrame,
    has_pair: bool,
    prepared_takeoff: bool,
}

impl<C: ClipSource> FixedBody<C> {
    /// Allocates frame storage and protects the declared contact joints and their ancestor/constraint closure.
    /// Static binding identity and the contact-joint selection belong to the caller's package.
    ///
    /// # Panics
    ///
    /// Panics if the graph has already stepped, IK was not bound to its fixed rig, or a contact index is out of range.
    #[must_use]
    pub fn new(
        graph: FixedGraphHistory<C>,
        ik: CharacterIk,
        policy: FixedBodyPolicy,
        contact_joints: &[usize],
    ) -> Self {
        assert!(graph.current().is_none(), "body owner requires a fresh graph");
        let pose = graph.pose();
        assert!(ik.matches_body(pose), "IK must be bound to the fixed body's pose and numeric policy");
        let joints = pose.joint_count();
        let morphs = pose.weights().len();
        let protected = pose.contact_closure(contact_joints).into();
        let layout = Arc::new(BodyLayout {
            joints: pose.joint_layout(),
            morph_names: pose.morph_names().map(<[String]>::to_vec),
            rest_weights: pose.rest_weights().to_vec(),
            protected,
        });
        Self {
            graph,
            ik,
            turn: policy.ground_turn.then(GroundTurn::default),
            policy,
            layout,
            previous: BodyFrame::blank(joints, morphs),
            current: BodyFrame::blank(joints, morphs),
            has_pair: false,
            prepared_takeoff: false,
        }
    }

    /// Advances and finishes at most once. When the graph holds its pose at frozen owner time,
    /// discrete changes stay in graph history and the last solved body and support are retained.
    pub fn advance(
        &mut self,
        step: BodyStep,
        ground_under: impl Fn(Vec3, &FootSettings) -> Option<GroundHit>,
    ) -> Result<bool, BodyStepError> {
        self.advance_step(step, ground_under, false)
    }

    /// Starts a new body history at a scene discontinuity. It may start at an earlier world
    /// step on a new run. No endpoint from the interrupted history is readable in its new pair.
    pub fn advance_after_cut(
        &mut self,
        step: BodyStep,
        ground_under: impl Fn(Vec3, &FootSettings) -> Option<GroundHit>,
    ) -> Result<bool, BodyStepError> {
        self.advance_step(step, ground_under, true)
    }

    fn advance_step(
        &mut self,
        step: BodyStep,
        ground_under: impl Fn(Vec3, &FootSettings) -> Option<GroundHit>,
        discontinuity: bool,
    ) -> Result<bool, BodyStepError> {
        if !step.root.valid() {
            return Err(BodyStepError::Root);
        }
        let prepared = self.graph.prepare(step.stamp, step.graph, discontinuity)?;
        let prior = self.graph.current().map(|(stamp, _)| stamp);
        if prepared.input.cut {
            self.ik.reset_body_history();
            if let Some(look) = &mut self.policy.head_look {
                look.reset();
            }
        }
        let locomotion = prepared.input.locomotion;
        if prepared.input.cut || locomotion.grounded {
            self.prepared_takeoff = !prepared.input.cut
                && (locomotion.preparing_jump || locomotion.moving_jump_preparation.is_some());
        }
        let teleports = self.graph.player.teleports();
        let world = step.root.affine();
        let place = Mat4::from(world);
        let (turning, receiving) = self.turn.as_mut().map_or((None, None), |turn| {
            turn.prepare(&self.graph.player, &self.graph.clips, &prepared.input, place, || {
                self.ik.feet().and_then(|feet| feet.turn_support())
            })
        });
        let support = self.ik.feet().and_then(|feet| feet.ground_support());
        let evaluated = self.graph.advance_prepared(prepared, receiving, support, step.movement_released);
        if self.graph.player.teleports() != teleports {
            self.prepared_takeoff = false;
        }
        std::mem::swap(&mut self.current, &mut self.previous);
        self.current.stamp = step.stamp;
        self.current.root = step.root;
        self.current.grounded = step.graph.locomotion.grounded;
        self.has_pair =
            !prepared.input.cut && prior.is_some_and(|stamp| stamp.generation == step.stamp.generation);
        if evaluated {
            let dt = prior
                .filter(|_| self.has_pair)
                .map_or(0.0, |stamp| step.stamp.owner_now.since(stamp.owner_now).as_secs_f32());
            let player = &mut self.graph.player;
            let contact = graph_foot_contact(player, &self.graph.clips, place, turning);
            if let Some(turn) = &mut self.turn {
                turn.apply_spine(player.pose_mut(), self.policy.spine, contact);
            }
            let head_weight = self.policy.head_look.as_mut().map_or(1.0, |look| {
                let sample = player.whole_body_combat_position(look.movement(), &self.graph.clips);
                look.weight(prepared.input.ticks, sample, prepared.input.cut)
            });
            let contact = contact.unwrap_or(step.fallback_contact);
            let input = IkInput {
                dt,
                world_from_model: world,
                feet: FeetInput { weight: f32::from(step.graph.locomotion.grounded), ..FeetInput::default() },
                look: step.look,
                weapon: step.weapon,
            };
            self.ik.set_prepared_release(self.prepared_takeoff && !locomotion.grounded);
            self.ik.set_moving_takeoff(locomotion.moving_jump_preparation);
            let pose = player.pose_mut();
            self.ik.solve_on_ground(pose, input, contact, head_weight, ground_under);
            self.current.copy_solved(pose.finish_body());
            self.current.pose_step = step.stamp.step;
            self.current.contact = Some(contact);
        } else {
            self.current.copy_pose(&self.previous);
            self.current.pose_step = self.previous.pose_step;
            self.current.contact = self.previous.contact;
        }
        Ok(evaluated)
    }

    #[cfg(test)]
    pub(super) fn prepared_support(&self) -> (bool, bool) {
        (self.prepared_takeoff, self.ik.feet().is_some_and(|feet| feet.prepared_release()))
    }

    #[cfg(test)]
    pub(super) fn has_landing_plant(&self) -> bool {
        self.ik.feet().is_some_and(|feet| feet.has_landing_plant())
    }

    /// Last fixed step and the contact policy used to solve its body.
    #[must_use]
    pub fn endpoint(&self) -> Option<(GraphStepStamp, FootContact)> {
        Some((self.graph.current()?.0, self.current.contact?))
    }

    /// Allocates a reusable consumer buffer for this binding.
    #[must_use]
    pub fn sample_buffer(&self) -> BodySample {
        BodySample {
            layout: self.layout.clone(),
            span: None,
            pose_key: None,
            root: self.current.root,
            grounded: false,
            locals: self.current.locals.clone(),
            model: self.current.model.clone(),
            morphs: self.current.morphs.clone(),
        }
    }

    /// Reads the same constrained endpoints for presentation and future geometry consumers.
    /// Fractional reads blend locals and run FK only. Generation cuts expose only the new pose.
    /// Returns false for an unstepped owner or a buffer belonging to another binding.
    pub fn read(&self, fraction: f32, out: &mut BodySample) -> bool {
        if self.graph.current().is_none() || !Arc::ptr_eq(&self.layout, &out.layout) {
            return false;
        }
        let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
        out.span = Some(BodyReadSpan {
            before: if self.has_pair { self.previous.stamp } else { self.current.stamp },
            after: self.current.stamp,
            fraction: if self.has_pair { fraction } else { 1.0 },
        });
        out.grounded = self.current.grounded;
        out.root = if self.has_pair && fraction < 1.0 {
            if fraction == 0.0 {
                self.previous.root
            } else {
                self.previous.root.lerp(self.current.root, fraction)
            }
        } else {
            self.current.root
        };
        let held = self.previous.pose_step == self.current.pose_step;
        if !self.has_pair || fraction == 0.0 || fraction == 1.0 || held {
            let frame = if self.has_pair && fraction == 0.0 { &self.previous } else { &self.current };
            out.pose_key = Some((frame.pose_step, frame.pose_step, 0.0));
            out.locals.copy_from_slice(&frame.locals);
            out.model.copy_from_slice(&frame.model);
            out.morphs.copy_from_slice(&frame.morphs);
        } else {
            out.pose_key = Some((self.previous.pose_step, self.current.pose_step, fraction));
            for (joint, local) in out.locals.iter_mut().enumerate() {
                let from = &self.previous.locals[joint];
                let to = &self.current.locals[joint];
                *local = motion::interpolate_local(*from, *to, fraction);
            }
            model::walk(self.layout.joints.parents.iter().copied(), &out.locals, &mut out.model);
            for (i, morph) in out.morphs.iter_mut().enumerate() {
                let before = self.previous.morphs[i];
                *morph = before + (self.current.morphs[i] - before) * fraction;
            }
        }
        true
    }

    /// Bounds a protected joint's attached region, in its local units, over the same pair read by Stage.
    pub fn motion_bound(&self, joint: usize, radius: f32) -> Result<BodyMotionBound, BodyMotionError> {
        if self.graph.current().is_none() {
            return Err(BodyMotionError::Unavailable);
        }
        if self.layout.protected.get(joint) != Some(&true) || !radius.is_finite() || radius < 0.0 {
            return Err(BodyMotionError::Unsupported);
        }
        let before = if self.has_pair { &self.previous } else { &self.current };
        let bound = motion::region_bound(before, &self.current, &self.layout.joints.parents, joint, radius)?;
        Ok(if self.has_pair { bound } else { BodyMotionBound::EndpointOnly })
    }

    /// Dynamic body state plus the graph and the published endpoint history. A package checksum
    /// must separately bind the rig, clip, ground and body policy data.
    pub fn write_canonical(&self, sink: &mut impl FnMut(&[u8])) {
        sink(&5_u32.to_le_bytes());
        self.graph.write_canonical(sink);
        let mut out = Canonical(sink);
        self.ik.write_canonical(&mut out);
        out.optional(self.turn.as_ref(), |out, turn| turn.write_canonical(out));
        out.optional(self.policy.head_look.as_ref(), |out, look| look.write_canonical(out));
        out.flag(self.prepared_takeoff);
        out.flag(self.has_pair);
        if self.has_pair {
            self.previous.write_canonical(&mut out);
        }
        out.flag(self.graph.current().is_some());
        if self.graph.current().is_some() {
            self.current.write_canonical(&mut out);
        }
    }
}

#[cfg(test)]
mod impact_release_tests {
    use super::*;
    use sapphire_assets::CpuResident;
    use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};
    use sapphire_time::SimTime;

    #[test]
    fn cosmetic_slot_preserves_the_exact_sampled_authored_morphs() {
        let skeleton =
            Skeleton::new(&[Joint { name: "root".to_owned(), parent: None, rest: JointTransform::IDENTITY }]);
        let skeleton = CpuResident::<Skeleton>::from_cooked(skeleton.to_cooked().unwrap()).unwrap();
        let names = vec!["cosmetic".to_owned(), "authored".to_owned()];
        let mut pose =
            CharacterPose::new(skeleton.view(), &[], &[], vec![0.0, 0.2]).with_morph_names(names.clone());
        let layout = Arc::new(BodyLayout {
            joints: pose.joint_layout(),
            morph_names: Some(names),
            rest_weights: vec![0.0, 0.2],
            protected: Arc::from([true]),
        });
        let stamp = GraphStepStamp { step: 1, owner_now: SimTime::from_subticks(1000), generation: 0 };
        let mut sample = BodySample {
            layout,
            span: Some(BodyReadSpan { before: stamp, after: stamp, fraction: 1.0 }),
            pose_key: None,
            root: BodyRoot { translation: Vec3::ZERO, yaw: 0.0, scale: 1.0 },
            grounded: true,
            locals: vec![Transform::IDENTITY],
            model: vec![Affine3A::IDENTITY],
            morphs: vec![0.0, 0.2],
        };
        assert!(sample.publish_to(&mut pose));
        pose.weights_mut()[0] = 0.75;
        assert!(sample.protected_matches_with_morph(&pose, Some(0)));
        assert!(!sample.protected_matches(&pose));
        assert!(!sample.protected_matches_with_morph(&pose, Some(1)));
        pose.weights_mut()[1] = 0.4;
        assert!(!sample.protected_matches_with_morph(&pose, Some(0)));
        pose.weights_mut()[1] = 0.2;
        sample.morphs[0] = 0.1;
        assert!(!sample.protected_matches_with_morph(&pose, Some(0)));
    }

    #[test]
    fn zero_release_debt_preserves_live_translation_rotation_scale_and_morph_bits() {
        let skeleton =
            Skeleton::new(&[Joint { name: "root".to_owned(), parent: None, rest: JointTransform::IDENTITY }]);
        let skeleton = CpuResident::<Skeleton>::from_cooked(skeleton.to_cooked().unwrap()).unwrap();
        let pose = CharacterPose::new(skeleton.view(), &[], &[], vec![0.2]);
        let layout = Arc::new(BodyLayout {
            joints: pose.joint_layout(),
            morph_names: None,
            rest_weights: vec![0.2],
            protected: Arc::from([true]),
        });
        let stamp =
            |step| GraphStepStamp { step, owner_now: SimTime::from_subticks(step * 1000), generation: 3 };
        let sample = |span, local: Transform, morph| BodySample {
            layout: Arc::clone(&layout),
            span: Some(span),
            pose_key: None,
            root: BodyRoot { translation: local.translation, yaw: 0.0, scale: 1.0 },
            grounded: true,
            locals: vec![local],
            model: vec![Affine3A::from_scale_rotation_translation(
                local.scale,
                local.rotation,
                local.translation,
            )],
            morphs: vec![morph],
        };
        let before = stamp(7);
        let after = stamp(8);
        let origin_span = BodyReadSpan { before, after, fraction: 0.0 };
        let held_span = BodyReadSpan { before, after: before, fraction: 1.0 };
        let held = sample(held_span, Transform::IDENTITY, 0.2);
        let origin = sample(origin_span, Transform::IDENTITY, 0.2);
        let span = BodyReadSpan { before, after, fraction: 1.0 };
        let changed = Transform {
            translation: Vec3::new(1.0, 0.5, -1.0),
            rotation: det::quat_from_rotation_y(0.8),
            scale: Vec3::new(1.25, 0.75, 2.0),
        };
        let mut live = sample(span, changed, 0.8);
        let expected = sample(span, changed, 0.8);
        assert!(live.apply_impact_residual(&held, &origin, 0.5));
        assert_eq!(live.root, expected.root);
        assert_eq!(live.locals, expected.locals);
        assert_eq!(live.model, expected.model);
        assert_eq!(live.morphs[0].to_bits(), expected.morphs[0].to_bits());
    }
}
