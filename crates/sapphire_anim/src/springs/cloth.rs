//! Chain cloth: skirts, coat tails and long hair as sheets of a character's spring chains (spec §5.13,
//! M5 C3).
//!
//! A [`ClothSheet`] names neighbouring chains of the rig, in order round the sheet. Each joint of a
//! chain is linked to the joint at the same depth of the next chain (a lateral link), and to the one
//! two chains along (a bend link, which keeps the sheet from folding up across itself). The links are
//! XPBD distance constraints between the chains' tails, with a compliance each: 0 is as stiff as the
//! iterations can make it, and larger is softer, in meters per newton on a tail of unit mass, so the
//! same compliance is the same stiffness at any sub-step count.
//!
//! # How it runs
//!
//! Inside each of [`SpringBones`]' sub-steps, after the springs' own step has moved every tail: each
//! iteration solves every link once, Gauss–Seidel in the sheets' order, then walks the chains again
//! from their roots, turning each joint toward its tail as moved, keeping the bone's length, out of the
//! chain's colliders (the rig's capsules and spheres) and inside the swing and bend limits, so the
//! springs' rules have the last word as they do on their own. What the links move a tail counts toward
//! its motion by the sub-step's share, as the bone's length and the colliders do (`Rate::carry`), so a
//! sheet takes the same path at any sub-step count.
//!
//! So the cloth shares everything with the springs: the frame, the owner's clock and its sub-steps,
//! reset, `continue_from`, sleep, the tier's distance and the off-screen test. A frozen clock runs
//! nothing, to the bit. Nothing of it reaches the simulation.
//!
//! A link's rest length is its two tails' distance in the animated pose, read with the pose and
//! interpolated between frames as the anchors are, so a clip that spreads a skirt keeps it spread.
//!
//! # Iterations
//!
//! [`SpringBones::set_cloth_iterations`]: more holds a sheet's links nearer their length. 0 turns the
//! cloth off, and the chains are springs again, to the bit.
//!
//! # Limits
//!
//! - A tail has a mass of one: a sheet's links pull both ends alike, and the walk from the root holds
//!   the root end.
//! - The grounded torso's last pass ([`SpringBones::update_with_torso_contact`]) is the springs' alone:
//!   a cloth chain's walk doesn't take it again after the links.
//! - Links join tails, so a capsule can pass between two chains without touching either.

use sapphire_assets::rig::ArchivedCharacterRig;
use sapphire_body_pose::Transform;
use sapphire_math::{Quat, Vec3};

use super::{Bone, Frame, NONE, SPRING_TICK_HZ, SpringBones, Tail};

/// Iterations a new cloth runs: Desktop-High's.
const DEFAULT_ITERATIONS: u32 = 4;
/// The most iterations a sub-step runs, whatever is asked.
pub const MAX_CLOTH_ITERATIONS: u32 = 16;

/// Neighbouring spring chains a character wears as one sheet: a skirt, a coat's tails, a fall of hair.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClothSheet {
    /// The rig's spring chains by name, in order round or across the sheet: at least two.
    pub chains: Vec<String>,
    /// Whether the last chain is linked back to the first, as a skirt's are round the body. A closed
    /// sheet needs three chains.
    pub closed: bool,
    /// How far a lateral link gives, in meters per newton on a tail of unit mass: 0 is stiff.
    pub lateral_compliance: f32,
    /// The same for a bend link, two chains along.
    pub bend_compliance: f32,
}

/// Why a cloth can't be made from a rig.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ClothError {
    #[error("the rig has no spring chain named {0:?}")]
    UnknownChain(String),
    #[error("the rig has more than one spring chain named {0:?}")]
    AmbiguousChain(String),
    #[error("spring chain {0:?} is named twice")]
    RepeatedChain(String),
    #[error("spring chain {0:?} has no swinging joint")]
    EmptyChain(String),
    #[error("a sheet needs at least {needs} chains, and this one has {has}")]
    TooFewChains { has: usize, needs: usize },
    #[error("{field} is {value}, where it must be 0 or more")]
    BadCompliance { field: &'static str, value: f32 },
}

/// Two tails a link holds apart: indices into [`Cloth::points`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Link {
    a: u16,
    b: u16,
    compliance: f32,
}

/// A character's sheets, as [`SpringBones`] solves them.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Cloth {
    /// Every link, lateral ones first.
    links: Vec<Link>,
    /// Each link's rest length in the model's units, as the last frame's pose had it and as this
    /// frame's does.
    rest_was: Vec<f32>,
    rest_now: Vec<f32>,
    /// Each link's rest length at the sub-step running, in the simulation's units.
    rest: Vec<f32>,
    /// Each link's XPBD multiplier through the sub-step running.
    lambda: Vec<f32>,
    /// The steps the walk from the roots goes over again, in the walk's order: the cloth chains' and
    /// every step below them.
    steps: Vec<u16>,
    /// Each swing's entry in [`Self::points`], or [`NONE`]: one for each swing among [`Self::steps`].
    point_of: Vec<u16>,
    /// The swings with a point, in the walk's order.
    swings: Vec<u16>,
    /// Each point's tail: as the sub-step began, as the springs' own step left it, and as the links
    /// and the walk have it now.
    from: Vec<Vec3>,
    sprung: Vec<Vec3>,
    points: Vec<Vec3>,
    pub(super) iterations: u32,
    /// The world's scale, which turns the model's units into the simulation's.
    pub(super) scale: f32,
    /// The next pose read starts its links' rest lengths over, with no previous frame to come from.
    fresh: bool,
}

impl Cloth {
    /// Whether it runs at all.
    pub(super) fn is_solving(&self) -> bool {
        self.iterations > 0
    }

    /// Whether swing `at` is one the cloth moves.
    pub(super) fn moves(&self, at: usize) -> bool {
        self.point_of[at] != NONE
    }

    /// Reads each link's rest length from a pose nothing has written over.
    pub(super) fn read_pose(&mut self, springs: &SpringBones, model: &[sapphire_math::Affine3A]) {
        let tail_of = |point: u16| {
            let swing = &springs.swings[usize::from(self.swings[usize::from(point)])];
            model[usize::from(swing.joint)].transform_point3(swing.tail * swing.length)
        };
        for (rest, link) in self.rest_now.iter_mut().zip(&self.links) {
            *rest = tail_of(link.a).distance(tail_of(link.b));
        }
        if self.fresh {
            self.rest_was.copy_from_slice(&self.rest_now);
            self.fresh = false;
        }
    }

    /// Takes over `old`'s iterations for a new version of the character, and its rest lengths if it
    /// links the same tails; otherwise they start over at the next pose read.
    pub(super) fn continue_from(&mut self, old: &Self) {
        self.iterations = old.iterations;
        if self.links == old.links {
            self.rest_was.clone_from(&old.rest_was);
            self.rest_now.clone_from(&old.rest_now);
            self.fresh = old.fresh;
        } else {
            self.fresh = true;
        }
    }

    /// Done with a frame: what it read is the next one's previous.
    pub(super) fn end_frame(&mut self) {
        self.rest_was.copy_from_slice(&self.rest_now);
    }

    /// Takes the tails as a sub-step begins, before the springs' own step.
    pub(super) fn begin(&mut self, tails: &[Tail]) {
        for (from, &swing) in self.from.iter_mut().zip(&self.swings) {
            *from = tails[usize::from(swing)].now;
        }
    }
}

impl SpringBones {
    /// Wears `sheets` of `rig`'s chains as cloth from now on, or none for an empty list: see the
    /// `cloth` module's docs. `rig` is the one these springs were made from. It runs
    /// [`MAX_CLOTH_ITERATIONS`] at most, and 4 to begin with ([`Self::set_cloth_iterations`]).
    /// Allocates; updating doesn't.
    pub fn set_cloth(&mut self, rig: &ArchivedCharacterRig, sheets: &[ClothSheet]) -> Result<(), ClothError> {
        if sheets.is_empty() {
            self.cloth = None;
            return Ok(());
        }
        // Each sheet's chains as their swings, root first.
        let mut named: Vec<&str> = Vec::new();
        let mut chains: Vec<Vec<Vec<u16>>> = Vec::new();
        for sheet in sheets {
            for (field, value) in
                [("lateral_compliance", sheet.lateral_compliance), ("bend_compliance", sheet.bend_compliance)]
            {
                if !(value >= 0.0 && value.is_finite()) {
                    return Err(ClothError::BadCompliance { field, value });
                }
            }
            let needs = if sheet.closed { 3 } else { 2 };
            if sheet.chains.len() < needs {
                return Err(ClothError::TooFewChains { has: sheet.chains.len(), needs });
            }
            let mut swings_of = Vec::new();
            for name in &sheet.chains {
                if named.contains(&name.as_str()) {
                    return Err(ClothError::RepeatedChain(name.clone()));
                }
                named.push(name);
                let mut found = rig.springs.iter().filter(|spring| spring.name.as_str() == name);
                let spring = found.next().ok_or_else(|| ClothError::UnknownChain(name.clone()))?;
                if found.next().is_some() {
                    return Err(ClothError::AmbiguousChain(name.clone()));
                }
                let swings: Vec<u16> = spring
                    .joints
                    .iter()
                    .filter_map(|entry| {
                        let joint = entry.joint.to_native();
                        self.swings.binary_search_by_key(&joint, |swing| swing.joint).ok().map(|at| at as u16)
                    })
                    .collect();
                if swings.is_empty() {
                    return Err(ClothError::EmptyChain(name.clone()));
                }
                swings_of.push(swings);
            }
            chains.push(swings_of);
        }

        // The steps to walk again: the chains' swings, and whatever hangs below them.
        let mut in_cloth = vec![false; self.swings.len()];
        for &swing in chains.iter().flatten().flatten() {
            in_cloth[usize::from(swing)] = true;
        }
        let anchors = self.anchors.len();
        let mut walked_again = vec![false; self.steps.len()];
        let mut steps = Vec::new();
        let mut point_of = vec![NONE; self.swings.len()];
        let mut swings = Vec::new();
        for (index, step) in self.steps.iter().enumerate() {
            let parent = usize::from(step.parent);
            let below = parent >= anchors && walked_again[parent - anchors];
            if below || (step.swing != NONE && in_cloth[usize::from(step.swing)]) {
                walked_again[index] = true;
                steps.push(index as u16);
                if step.swing != NONE {
                    point_of[usize::from(step.swing)] = swings.len() as u16;
                    swings.push(step.swing);
                }
            }
        }

        // Lateral links between neighbours, then bend links two along, each pair once.
        let mut links: Vec<Link> = Vec::new();
        for (layer, along) in [(0, 1), (1, 2)] {
            for (sheet, chains) in sheets.iter().zip(&chains) {
                let count = chains.len();
                let pairs = if sheet.closed { count } else { count.saturating_sub(along) };
                let compliance = if layer == 0 { sheet.lateral_compliance } else { sheet.bend_compliance };
                for first in 0..pairs {
                    let (one, other) = (&chains[first], &chains[(first + along) % count]);
                    for (&a, &b) in one.iter().zip(other) {
                        let (a, b) = (point_of[usize::from(a)], point_of[usize::from(b)]);
                        let known =
                            links.iter().any(|link| (link.a, link.b) == (a, b) || (link.a, link.b) == (b, a));
                        if a != b && !known {
                            links.push(Link { a, b, compliance });
                        }
                    }
                }
            }
        }

        let (points, count) = (swings.len(), links.len());
        let iterations = self.cloth.as_ref().map_or(DEFAULT_ITERATIONS, |cloth| cloth.iterations);
        self.cloth = Some(Box::new(Cloth {
            links,
            rest_was: vec![0.0; count],
            rest_now: vec![0.0; count],
            rest: vec![0.0; count],
            lambda: vec![0.0; count],
            steps,
            point_of,
            swings,
            from: vec![Vec3::ZERO; points],
            sprung: vec![Vec3::ZERO; points],
            points: vec![Vec3::ZERO; points],
            iterations,
            scale: self.world.matrix3.x_axis.length(),
            fresh: true,
        }));
        // Its rest lengths come from the pose at the next update.
        self.targets_stale = true;
        Ok(())
    }

    /// Runs `iterations` of the cloth's links a sub-step from now on, at most
    /// [`MAX_CLOTH_ITERATIONS`]; 0 leaves its chains to the springs alone. A character with no cloth
    /// ignores it.
    pub fn set_cloth_iterations(&mut self, iterations: u32) {
        if let Some(cloth) = &mut self.cloth {
            cloth.iterations = iterations.min(MAX_CLOTH_ITERATIONS);
        }
    }

    /// The cloth's iterations a sub-step, or `None` for a character with no cloth.
    #[must_use]
    pub fn cloth_iterations(&self) -> Option<u32> {
        self.cloth.as_ref().map(|cloth| cloth.iterations)
    }

    /// How far the cloth's links are from their rest lengths as of the last sub-step: the largest
    /// stretch or squeeze, as a share of the rest length. 0 for a character with no cloth.
    #[must_use]
    pub fn cloth_strain(&self) -> f32 {
        let Some(cloth) = &self.cloth else { return 0.0 };
        cloth.links.iter().zip(&cloth.rest_now).fold(0.0, |most: f32, (link, &rest)| {
            let tail = |point: u16| self.tails[usize::from(cloth.swings[usize::from(point)])].now;
            let rest = rest * cloth.scale;
            if rest <= 0.0 {
                most
            } else {
                most.max((tail(link.a).distance(tail(link.b)) / rest - 1.0).abs())
            }
        })
    }

    /// The cloth's part of a sub-step, `between` of the way from the last frame to this one, after the
    /// springs' own step: its links, then the walk from the roots, `iterations` times. Returns the
    /// square of the furthest a tail it moves went over the sub-step.
    pub(super) fn solve_cloth(&mut self, cloth: &mut Cloth, locals: &[Transform], between: f32) -> f32 {
        for (point, &swing) in cloth.points.iter_mut().zip(&cloth.swings) {
            *point = self.tails[usize::from(swing)].now;
        }
        cloth.sprung.copy_from_slice(&cloth.points);
        for ((rest, was), now) in cloth.rest.iter_mut().zip(&cloth.rest_was).zip(&cloth.rest_now) {
            *rest = (was + (now - was) * between) * cloth.scale;
        }
        cloth.lambda.fill(0.0);
        // Compliance over the square of the sub-step's length, in the owner clock's seconds.
        let rate = (SPRING_TICK_HZ * self.substeps.count()) as f32;
        let per_square = rate * rate;
        for iteration in 0..cloth.iterations {
            for ((link, rest), lambda) in cloth.links.iter().zip(&cloth.rest).zip(&mut cloth.lambda) {
                let (a, b) = (usize::from(link.a), usize::from(link.b));
                let apart = cloth.points[b] - cloth.points[a];
                let square = apart.length_squared();
                if square < 1e-12 {
                    continue;
                }
                let length = square.sqrt();
                let compliance = link.compliance * per_square;
                // Two tails of unit mass, each moved along the link.
                let delta = (rest - length - compliance * *lambda) / (2.0 + compliance);
                *lambda += delta;
                let push = apart * (delta / length);
                cloth.points[a] -= push;
                cloth.points[b] += push;
            }
            self.walk_cloth(cloth, locals, iteration + 1 == cloth.iterations);
        }
        cloth
            .points
            .iter()
            .zip(&cloth.from)
            .fold(0.0, |most: f32, (point, from)| most.max((*point - *from).length_squared()))
    }

    /// Walks the cloth's steps from their roots, turning each swing toward its point: out of its
    /// colliders, inside its limits, at its bone's length. The `last` walk of a sub-step also writes
    /// the tails and their turns.
    fn walk_cloth(&mut self, cloth: &mut Cloth, locals: &[Transform], last: bool) {
        let anchors = self.anchors.len();
        for &index in &cloth.steps {
            let index = usize::from(index);
            let step = self.steps[index];
            let parent = self.slots[usize::from(step.parent)];
            let local = &locals[usize::from(step.joint)];
            let position = parent.position + parent.rotation * (local.translation * parent.scale);
            let scale = parent.scale * local.scale.x;
            let rotation = if step.swing == NONE {
                parent.rotation * local.rotation
            } else {
                let at = usize::from(step.swing);
                let point = usize::from(cloth.point_of[at]);
                let swing = &self.swings[at];
                let pose_rotation = parent.rotation * self.targets[at];
                let toward_pose = pose_rotation * swing.tail;
                let length = swing.length * scale;
                let bone = Bone {
                    head: position,
                    length,
                    hit_radius: swing.hit_radius * scale,
                    fallback: toward_pose,
                };
                let wanted = position
                    + (cloth.points[point] - position).try_normalize().unwrap_or(toward_pose) * length;
                // From where the springs left it, against the colliders as they stand.
                let next =
                    self.collide(swing.colliders, &self.colliders_now, &bone, cloth.sprung[point], wanted);
                let aim = (next - position).try_normalize().unwrap_or(toward_pose);
                let aim = self
                    .bend_cone
                    .hold(self.swing_cone.hold(aim, self.posed[index] * swing.tail), toward_pose);
                let next = position + aim * length;
                cloth.points[point] = next;
                let rotation = Quat::from_rotation_arc(toward_pose, aim) * pose_rotation;
                if last {
                    // What the cloth moved it counts toward its motion by the sub-step's share.
                    let moved = next - cloth.sprung[point];
                    let tail = self.tails[at];
                    let carry = self.rates[at].carry;
                    self.tails[at] = Tail { now: next, before: tail.before + moved * (1.0 - carry) };
                    self.solved[at][1] = (parent.rotation.inverse() * rotation).normalize();
                }
                rotation
            };
            self.slots[anchors + index] = Frame { rotation, position, scale };
        }
    }
}

#[cfg(test)]
mod tests;
