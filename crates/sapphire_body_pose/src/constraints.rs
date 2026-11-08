//! Rig constraints (VRM's `VRMC_node_constraint`): twist and aim helpers the skin is weighted to,
//! which follow their source joints after the pose. Without them a lowered arm leaves its sleeve
//! in the T-pose. Clips don't bake them, so whatever makes a pose runs them after it.
//!
//! Each constrained joint follows its source: an aim joint turns its axis toward the source by the
//! shortest arc from its rest direction, under its parent's current orientation; a roll joint takes
//! the twist about its axis of the source's rotation from rest (its swing-twist decomposition), a
//! rotation joint the whole of it; each by the constraint's weight.
//!
//! [`Constraints::new`] works out once, with allocation, the order they run in and which joints
//! each aim reads, so [`Constraints::apply`] allocates nothing and costs each aim its two chains of
//! ancestors rather than the skeleton.

use std::ops::Range;

use sapphire_assets::rig::{AimAxis, Axis, Constraint, ConstraintKind};
use sapphire_assets::skeleton::NO_PARENT;
use sapphire_math::{Affine3A, Quat, Transform, Vec3};

use crate::model::model_of;
use crate::pose_math::PoseMath;

/// A rig and skeleton whose constraint dependencies cannot be evaluated in one pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("constraint on joint {joint} has a cyclic skeleton dependency")]
pub struct ConstraintCycle {
    pub joint: u16,
}

/// A rig's constraints, ordered for a skeleton.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Constraints {
    steps: Vec<Step>,
    math: PoseMath,
    /// Every aim's chain, one after another: the joints it reads and their ancestors, parents
    /// first.
    chains: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq)]
struct Step {
    joint: u16,
    source: u16,
    weight: f32,
    kind: ConstraintKind,
    /// Its chain in [`Constraints::chains`]: empty but for an aim.
    chain: Range<u32>,
}

impl Constraints {
    /// Orders constraints for a known-valid skeleton and rig. Out-of-range joints are dropped.
    ///
    /// # Panics
    ///
    /// If the combined hierarchy and constraint graph has a cycle. Use [`Self::try_new`] when
    /// binding authored assets so that the author can receive the error.
    #[must_use]
    pub fn new(parents: &[u16], constraints: &[Constraint]) -> Self {
        Self::try_new(parents, constraints).expect("constraints and skeleton must have no dependency cycle")
    }

    /// Orders a rig's actual read/write dependencies. Roll and rotation read their source's local
    /// rotation; aim also reads model-space positions and the destination parent's orientation.
    /// `parents` must be a valid skeleton hierarchy. Out-of-range constraints are dropped.
    pub fn try_new(parents: &[u16], constraints: &[Constraint]) -> Result<Self, ConstraintCycle> {
        let count = parents.len();
        let within = |ancestor: usize, mut joint: usize| loop {
            if joint == ancestor {
                return true;
            }
            match parents.get(joint) {
                Some(&parent) if parent != NO_PARENT => joint = usize::from(parent),
                _ => return false,
            }
        };
        let waits = |constraint: &Constraint, pending: &[&Constraint]| {
            let (joint, source) = (usize::from(constraint.joint), usize::from(constraint.source));
            pending.iter().any(|other| {
                let moved = usize::from(other.joint);
                if matches!(constraint.kind, ConstraintKind::Aim { .. }) {
                    // Its own rotation moves a descendant source, creating feedback even if no
                    // other constraint is present. It does not move its own model-space origin.
                    within(moved, source) || (moved != joint && within(moved, joint))
                } else {
                    moved == source
                }
            })
        };
        let mut steps = Vec::with_capacity(constraints.len());
        let mut chains = Vec::new();
        let mut pending: Vec<&Constraint> = constraints
            .iter()
            .filter(|c| usize::from(c.joint) < count && usize::from(c.source) < count)
            .collect();
        while !pending.is_empty() {
            let ready = pending
                .iter()
                .position(|constraint| !waits(constraint, &pending))
                .ok_or_else(|| ConstraintCycle { joint: pending[0].joint })?;
            let constraint = pending.remove(ready);
            let (joint, source) = (usize::from(constraint.joint), usize::from(constraint.source));
            let start = chains.len() as u32;
            if matches!(constraint.kind, ConstraintKind::Aim { .. }) {
                // In joint order, which is parents first.
                chains.extend(
                    (0..count)
                        .filter(|&ancestor| within(ancestor, joint) || within(ancestor, source))
                        .map(|ancestor| ancestor as u16),
                );
            }
            steps.push(Step {
                joint: constraint.joint,
                source: constraint.source,
                weight: constraint.weight.clamp(0.0, 1.0),
                kind: constraint.kind,
                chain: start..chains.len() as u32,
            });
        }
        Ok(Self { steps, chains, math: PoseMath::Legacy })
    }

    /// Adds every ancestor and transitive rig source that can influence a protected model joint.
    /// This uses the validated constraint program rather than the unbound author document.
    pub(crate) fn contact_closure(&self, parents: &[u16], contacts: &[usize]) -> Vec<bool> {
        let mut protected = vec![false; parents.len()];
        let mark = |mut joint: usize, protected: &mut [bool]| {
            while !protected[joint] {
                protected[joint] = true;
                let parent = parents[joint];
                if parent == NO_PARENT {
                    break;
                }
                joint = usize::from(parent);
            }
        };
        for &contact in contacts {
            mark(contact, &mut protected);
        }
        // Adding a source's parents may expose a dependency that a prior pass could not see.
        loop {
            let mut changed = false;
            for step in self.steps.iter().rev() {
                let source = usize::from(step.source);
                if protected[usize::from(step.joint)] && !protected[source] {
                    mark(source, &mut protected);
                    changed = true;
                }
            }
            if !changed {
                return protected;
            }
        }
    }

    pub(crate) fn set_math(&mut self, math: PoseMath) {
        self.math = math;
    }

    /// How many run.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Makes `locals` follow the rig. `rest` is the skeleton's rest pose ([`crate::rest_pose`]), and
    /// `world` and `model` are scratch of one entry per joint, which an aim's chain writes as it
    /// reads. Allocates nothing.
    pub fn apply(
        &self,
        parents: &[u16],
        rest: &[Transform],
        locals: &mut [Transform],
        world: &mut [Quat],
        model: &mut [Affine3A],
    ) {
        self.apply_excluding(parents, rest, locals, world, model, &[]);
    }

    pub(crate) fn apply_excluding(
        &self,
        parents: &[u16],
        rest: &[Transform],
        locals: &mut [Transform],
        world: &mut [Quat],
        model: &mut [Affine3A],
        protected: &[bool],
    ) {
        let math = self.math;
        for step in &self.steps {
            let (joint, source) = (usize::from(step.joint), usize::from(step.source));
            if protected.get(joint) == Some(&true) {
                continue;
            }
            let rest_rotation = rest[joint].rotation;
            let target = match step.kind {
                ConstraintKind::Roll { axis } => {
                    // The source's rotation from rest in its parent's frame, seen from the joint's
                    // rest frame.
                    let source_turn = math.multiply(locals[source].rotation, rest[source].rotation.inverse());
                    let in_joint =
                        math.multiply(math.multiply(rest_rotation.inverse(), source_turn), rest_rotation);
                    math.multiply(rest_rotation, twist(in_joint, axis_of(axis), math))
                }
                ConstraintKind::Rotation => {
                    // The source's rotation from rest, in its own frame (VRMC_node_constraint 1.0).
                    math.multiply(
                        rest_rotation,
                        math.multiply(rest[source].rotation.inverse(), locals[source].rotation),
                    )
                }
                ConstraintKind::Aim { axis } => {
                    let chain = step.chain.start as usize..step.chain.end as usize;
                    for &ancestor in &self.chains[chain] {
                        let ancestor = usize::from(ancestor);
                        let parent = parents[ancestor];
                        let above =
                            if parent == NO_PARENT { Quat::IDENTITY } else { world[usize::from(parent)] };
                        world[ancestor] = math.multiply(above, locals[ancestor].rotation);
                        if protected.get(ancestor) != Some(&true) {
                            model[ancestor] = model_of(parent, &locals[ancestor], model);
                        }
                    }
                    let parent = parents[joint];
                    let parent =
                        if parent == NO_PARENT { Quat::IDENTITY } else { world[usize::from(parent)] };
                    let from = math.rotate(math.multiply(parent, rest_rotation), aim_axis(axis)).normalize();
                    let to =
                        Vec3::from(model[source].translation - model[joint].translation).normalize_or_zero();
                    if to == Vec3::ZERO {
                        rest_rotation
                    } else {
                        let turn = math.rotation_arc(from, to);
                        let turn = math.multiply(math.multiply(parent.inverse(), turn), parent);
                        math.multiply(turn, rest_rotation)
                    }
                }
            };
            locals[joint].rotation =
                math.normalize(math.slerp(rest_rotation, math.normalize(target), step.weight));
        }
    }
}

/// The twist of `rotation` about the unit `axis`: its swing-twist decomposition's twist.
fn twist(rotation: Quat, axis: Vec3, math: PoseMath) -> Quat {
    let projected = axis * rotation.xyz().dot(axis);
    let twist = Quat::from_xyzw(projected.x, projected.y, projected.z, rotation.w);
    if math.rotation_length_squared(twist) < 1e-12 { Quat::IDENTITY } else { math.normalize(twist) }
}

fn axis_of(axis: Axis) -> Vec3 {
    match axis {
        Axis::X => Vec3::X,
        Axis::Y => Vec3::Y,
        Axis::Z => Vec3::Z,
    }
}

fn aim_axis(axis: AimAxis) -> Vec3 {
    match axis {
        AimAxis::PositiveX => Vec3::X,
        AimAxis::NegativeX => Vec3::NEG_X,
        AimAxis::PositiveY => Vec3::Y,
        AimAxis::NegativeY => Vec3::NEG_Y,
        AimAxis::PositiveZ => Vec3::Z,
        AimAxis::NegativeZ => Vec3::NEG_Z,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, reason = "reference fixtures also exercise the legacy math policy")]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_feedback_is_refused_but_local_dependencies_are_allowed() {
        let parents = [NO_PARENT, 0, NO_PARENT, 2];
        let aim = |joint, source| Constraint {
            joint,
            source,
            weight: 1.0,
            kind: ConstraintKind::Aim { axis: AimAxis::PositiveX },
        };
        for constraints in [vec![aim(0, 3), aim(2, 1)], vec![aim(2, 1), aim(0, 3)], vec![aim(0, 1)]] {
            assert!(Constraints::try_new(&parents, &constraints).is_err());
        }
        let local = |joint, source| Constraint { joint, source, weight: 1.0, kind: ConstraintKind::Rotation };
        // Ancestor transforms do not affect a local rotation/roll read.
        assert!(Constraints::try_new(&parents, &[local(0, 3), local(2, 1)]).is_ok());
        assert!(Constraints::try_new(&parents, &[local(0, 2), local(2, 0)]).is_err());
    }

    #[test]
    fn twist_keeps_only_the_rotation_about_the_axis() {
        let rotation = Quat::from_rotation_x(0.6) * Quat::from_rotation_z(0.0);
        for math in [PoseMath::Legacy, PoseMath::Pinned] {
            assert!(twist(rotation, Vec3::X, math).abs_diff_eq(Quat::from_rotation_x(0.6), 1e-6));
            assert!(twist(Quat::from_rotation_z(0.8), Vec3::X, math).abs_diff_eq(Quat::IDENTITY, 1e-6));
        }
    }

    #[test]
    fn a_constraint_outside_the_skeleton_is_dropped() {
        let parents = [NO_PARENT, 0];
        let inside = Constraint { joint: 1, source: 0, weight: 1.0, kind: ConstraintKind::Rotation };
        let outside = Constraint { joint: 1, source: 9, weight: 1.0, kind: ConstraintKind::Rotation };
        assert_eq!(Constraints::new(&parents, &[outside, inside]).len(), 1);
    }

    #[test]
    fn an_aims_chain_lists_only_the_joints_it_reads_parents_first() {
        // 0 ─ 1 ─ 2 and 0 ─ 3 ─ 4, with 5 off to the side: 4 aims at 2.
        let parents = [NO_PARENT, 0, 1, 0, 3, 0];
        let aim = Constraint {
            joint: 4,
            source: 2,
            weight: 1.0,
            kind: ConstraintKind::Aim { axis: AimAxis::PositiveX },
        };
        let constraints = Constraints::new(&parents, &[aim]);
        assert_eq!(constraints.chains, [0, 1, 2, 3, 4]);
    }
}
