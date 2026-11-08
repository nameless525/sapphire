//! Damage-off observation of the overhead against the two authoritative body pairs.
//! The ordinary hit query and its records remain the sole writers.

mod delivery;
mod search;
mod socket;

use bevy_ecs::system::SystemState;
use sapphire_body_pose::{BodyMotionBound, BodyReadSpan, BodySample};
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::glam::DVec3;
use sapphire_math::{Mat4, Vec3};
use sapphire_physics::{
    ColliderId, Group, MeshAxisBinding, MeshAxisProbe, PhysicsWorld, Pose, StaticCollider, StaticShape,
};
use sapphire_time::{SUBTICKS_PER_TICK, SimDuration};

pub(crate) use delivery::AttackSource;
pub use delivery::OverheadAppliedContact;
pub(super) use delivery::{Candidate, Delivery, HIT};

use super::{HitQuery, MAX_SAG, sweep_hits};
use crate::actors::ActorTable;
use crate::body::BodyAuthority;
use crate::checksum::StateHasher;
use crate::moves::CurrentMove;
use crate::simulation::view;
use crate::{ActorId, CombatSet, Simulation};

use search::{Interval, Motion, Outcome, Policy, Probe};
use socket::{BRACKET_ROUNDOFF_M, Socket};

const CAPACITY: usize = 16;
pub(super) const OVERHEAD: Name = Name::new("overhead");
const FIRST: u64 = 45 * SUBTICKS_PER_TICK as u64;
const END: u64 = 56 * SUBTICKS_PER_TICK as u64;

/// One anatomical capsule endpoint in a named, protected joint's local space.
#[derive(Clone, Copy, Debug)]
pub struct ShadowEndpoint {
    pub joint: u16,
    pub name: Name,
    pub local: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct ShadowCapsule {
    pub ends: [ShadowEndpoint; 2],
    pub radius: f32,
}

/// Explicit comparison policies for the damage-off review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverheadShadowBracket {
    #[default]
    Strict,
    TimingOnly,
    EngineSpatial,
}

impl OverheadShadowBracket {
    fn limits(self) -> (f64, f64) {
        match self {
            Self::Strict => (1.0 / 1024.0, 0.002),
            Self::TimingOnly => (1.0 / 64.0, 0.002),
            Self::EngineSpatial => (1.0 / 64.0, f64::from(MAX_SAG)),
        }
    }
}

/// Bound once outside the fixed step. The mesh uses the rendered asset's local units.
pub struct OverheadShadowShape {
    pub socket: u16,
    pub bracket: OverheadShadowBracket,
    pub warrior_scale: f32,
    pub heroine_scale: f32,
    pub steel_vertices: Vec<Vec3>,
    pub steel_triangles: Vec<[u32; 3]>,
    pub anatomy: Vec<ShadowCapsule>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverheadShadowWitness {
    pub earliest_fraction: f32,
    pub witnessed_fraction: f32,
    pub travel_bound: f64,
    pub source_subticks: f64,
    pub capsule: u8,
    pub triangle: u32,
    pub steel_point: [f64; 3],
    normal: Vec3,
    pub attacker: BodyReadSpan,
    pub target: BodyReadSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverheadShadowFailure {
    BodyInterval,
    UnsupportedSample,
    ProbeBudget,
    ContactBracket,
    NoWitness,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverheadShadowUncertainty {
    pub cause: OverheadShadowFailure,
    pub earliest_possible: Option<f32>,
    pub sampled_witness: Option<f32>,
    pub sampled_bracket_travel: Option<f64>,
    pub relative_rate: f64,
    pub numerical_margin: f64,
}

impl OverheadShadowUncertainty {
    fn at_endpoint(cause: OverheadShadowFailure) -> Self {
        Self {
            cause,
            earliest_possible: None,
            sampled_witness: None,
            sampled_bracket_travel: None,
            relative_rate: 0.0,
            numerical_margin: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverheadShadowResult {
    Inactive,
    Separated { probes: u16 },
    Witness { contact: OverheadShadowWitness, probes: u16 },
    Unresolved { detail: OverheadShadowUncertainty, probes: u16 },
}

#[derive(Clone, Copy)]
struct Travel {
    rate: f64,
    acceleration: f64,
    margin: f64,
    cut: bool,
}

impl Travel {
    const ZERO: Self = Self { rate: 0.0, acceleration: 0.0, margin: 0.0, cut: false };

    fn combine(self, other: Self) -> Self {
        Self {
            rate: self.rate + other.rate,
            acceleration: self.acceleration + other.acceleration,
            margin: self.margin + other.margin,
            cut: self.cut || other.cut,
        }
    }

    fn enclose(self, other: Self) -> Self {
        Self {
            rate: self.rate.max(other.rate),
            acceleration: self.acceleration.max(other.acceleration),
            margin: self.margin.max(other.margin),
            cut: self.cut || other.cut,
        }
    }
}

impl From<BodyMotionBound> for Travel {
    fn from(bound: BodyMotionBound) -> Self {
        match bound {
            BodyMotionBound::EndpointOnly => Self { cut: true, ..Self::ZERO },
            BodyMotionBound::Continuous { metres_per_step, metres_per_step_squared, numerical_margin } => {
                Self {
                    rate: metres_per_step,
                    acceleration: metres_per_step_squared,
                    margin: numerical_margin,
                    cut: false,
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct OwnerWindow {
    interval: Interval,
    before: u64,
    delta: u64,
}

impl OwnerWindow {
    fn from_step(after: SimDuration, delta: SimDuration) -> Option<Self> {
        let after = after.as_subticks();
        let delta = delta.as_subticks();
        if delta == 0 || after < FIRST {
            return None;
        }
        let before = after.checked_sub(delta)?;
        if before >= END {
            return None;
        }
        let fraction = |at: u64| (at - before) as f64 / delta as f64;
        let from = fraction(before.max(FIRST));
        let to = fraction(after.min(END));
        let mut begin = from as f32;
        let mut end = to as f32;
        if f64::from(begin) < from {
            begin = begin.next_up();
        }
        if f64::from(end) > to || (after >= END && f64::from(end) == to) {
            end = end.next_down();
        }
        (begin <= end).then_some(Self { interval: Interval { from: begin, to: end }, before, delta })
    }

    fn source_at(self, fraction: f32) -> f64 {
        self.before as f64 + f64::from(fraction) * self.delta as f64
    }
}

#[derive(Clone, Copy)]
struct ShapeFrame {
    fraction: f32,
    socket: Socket,
    axes: [[Vec3; 2]; CAPACITY],
}

impl ShapeFrame {
    fn travel(&self, after: &Self, region: Travel, index: usize, steel_radius: f64) -> f64 {
        let chord =
            self.socket.relative_chord(after.socket, self.axes[index], after.axes[index], steel_radius);
        self.travel_with_chord(after, region, chord)
    }

    fn bracket_travel(
        &self,
        after: &Self,
        region: Travel,
        index: usize,
        steel_radius: f64,
        steel_bounds: [DVec3; 2],
    ) -> f64 {
        let axis = self.axes[index];
        let after_axis = after.axes[index];
        let sphere_chord = self.socket.relative_chord(after.socket, axis, after_axis, steel_radius);
        let box_chord = self.socket.relative_box_chord(after.socket, axis, after_axis, steel_bounds);
        self.travel_with_chord(after, region, sphere_chord.min(box_chord)) + BRACKET_ROUNDOFF_M
    }

    fn travel_with_chord(&self, after: &Self, region: Travel, chord: f64) -> f64 {
        let width = (f64::from(after.fraction) - f64::from(self.fraction)).abs();
        let sag = region.acceleration * width * width / 8.0;
        (region.rate * width).min(chord + sag + 1e-9) + region.margin
    }
}

#[derive(Resource)]
struct Shadow {
    attacker: ActorId,
    target: ActorId,
    warrior: BodySample,
    heroine: BodySample,
    world: PhysicsWorld,
    mesh: MeshAxisBinding,
    socket: usize,
    bracket: OverheadShadowBracket,
    warrior_scale: f32,
    heroine_scale: f32,
    steel_radius: f32,
    steel_bounds: [DVec3; 2],
    anatomy: Vec<ShadowCapsule>,
    result: OverheadShadowResult,
    delivery: Option<delivery::Route>,
}

impl Shadow {
    fn bind(
        attacker: ActorId,
        target: ActorId,
        warrior: BodySample,
        heroine: BodySample,
        shape: OverheadShadowShape,
    ) -> Option<Self> {
        let socket = usize::from(shape.socket);
        if warrior.joint_names().get(socket).map(String::as_str) != Some("handslot.r")
            || warrior.protected_joints().get(socket) != Some(&true)
            || !(0.25..=4.0).contains(&shape.warrior_scale)
            || !(0.25..=4.0).contains(&shape.heroine_scale)
            || shape.anatomy.is_empty()
            || shape.anatomy.len() > CAPACITY
            || shape.anatomy.iter().any(|capsule| {
                !(0.001..=0.5).contains(&capsule.radius)
                    || capsule.ends.iter().any(|end| {
                        let joint = usize::from(end.joint);
                        heroine.protected_joints().get(joint) != Some(&true)
                            || heroine.joint_names().get(joint).is_none_or(|name| Name::new(name) != end.name)
                            || !end.local.is_finite()
                            || end.local.length() > 2.0
                    })
            })
        {
            return None;
        }
        let steel_radius =
            shape.steel_vertices.iter().copied().map(attached_radius).fold(0.0_f32, f32::max).next_up();
        let first = shape.steel_vertices.first()?.as_dvec3();
        let steel_bounds = shape.steel_vertices.iter().fold([first; 2], |[min, max], vertex| {
            let vertex = vertex.as_dvec3();
            [min.min(vertex), max.max(vertex)]
        });
        let id = ColliderId(0);
        let world = PhysicsWorld::new(
            vec![StaticCollider {
                id,
                shape: StaticShape::Mesh { vertices: shape.steel_vertices, indices: shape.steel_triangles },
                pose: Pose::IDENTITY,
                memberships: Group::World.mask(),
            }],
            0,
        )
        .ok()?;
        let mesh = world.bind_mesh_axis(id).ok()?;
        Some(Self {
            attacker,
            target,
            warrior,
            heroine,
            world,
            mesh,
            socket,
            bracket: shape.bracket,
            warrior_scale: shape.warrior_scale,
            heroine_scale: shape.heroine_scale,
            steel_radius,
            steel_bounds,
            anatomy: shape.anatomy,
            result: OverheadShadowResult::Inactive,
            delivery: None,
        })
    }

    fn travel(&self, bodies: &BodyAuthority) -> Option<([Travel; CAPACITY], Travel)> {
        let blade = Travel::from(bodies.motion_bound(self.attacker, self.socket, self.steel_radius).ok()?);
        let mut regions = [Travel::ZERO; CAPACITY];
        let mut overall = Travel::ZERO;
        for (capsule, region) in self.anatomy.iter().zip(&mut regions) {
            let endpoint = |end: ShadowEndpoint| {
                bodies
                    .motion_bound(self.target, usize::from(end.joint), attached_radius(end.local))
                    .ok()
                    .map(Travel::from)
            };
            *region = blade.combine(endpoint(capsule.ends[0])?.enclose(endpoint(capsule.ends[1])?));
            overall = overall.enclose(*region);
        }
        Some((regions, overall))
    }

    fn probe(
        &mut self,
        bodies: &BodyAuthority,
        window: OwnerWindow,
        regions: &[Travel; CAPACITY],
        fraction: f32,
        bounds: Option<[&ShapeFrame; 2]>,
        active: u16,
    ) -> Probe<OverheadShadowWitness, ShapeFrame> {
        if !bodies.read(self.attacker, fraction, &mut self.warrior)
            || !bodies.read(self.target, fraction, &mut self.heroine)
            || self.heroine.root().scale.to_bits() != self.heroine_scale.to_bits()
        {
            return Probe::Unsupported;
        }
        let (Some(attacker), Some(target)) = (self.warrior.span(), self.heroine.span()) else {
            return Probe::Unsupported;
        };
        if attacker.after.step != target.after.step
            || attacker.fraction.to_bits() != fraction.to_bits()
            || target.fraction.to_bits() != fraction.to_bits()
            || (attacker.before.step != attacker.after.step
                && attacker.after.owner_now.since(attacker.before.owner_now).as_subticks() != window.delta)
        {
            return Probe::Unsupported;
        }
        let root = |body: &BodySample| Mat4::from(body.root().affine());
        let matrix = root(&self.warrior) * Mat4::from(self.warrior.model()[self.socket]);
        let Some(socket) = Socket::bind(matrix, self.warrior_scale) else {
            return Probe::Unsupported;
        };
        let heroine = root(&self.heroine);
        let mut geometry = ShapeFrame { fraction, socket, axes: [[Vec3::ZERO; 2]; CAPACITY] };
        for (capsule, output) in self.anatomy.iter().zip(&mut geometry.axes) {
            *output = capsule.ends.map(|end| {
                let matrix = heroine * Mat4::from(self.heroine.model()[usize::from(end.joint)]);
                matrix.transform_point3(end.local)
            });
        }
        let mut possible = 0_u16;
        let mut witness = None;
        for (index, (capsule, region)) in self.anatomy.iter().zip(regions).enumerate() {
            let world = geometry.axes[index];
            let extra = bounds.map_or(0.0, |[a, b]| {
                let radius = f64::from(self.steel_radius);
                geometry.travel(a, *region, index, radius).max(geometry.travel(b, *region, index, radius))
            });
            let Some((axis, raw, outer)) = socket.capsule(world, f64::from(capsule.radius), extra) else {
                return Probe::Unsupported;
            };
            let region_bit = 1 << index;
            if active & region_bit == 0 {
                continue;
            }
            match self.world.probe_mesh_axis(self.mesh, axis, raw, outer) {
                Ok(MeshAxisProbe::Separated) => {}
                Ok(MeshAxisProbe::Possible) => possible |= region_bit,
                Ok(MeshAxisProbe::Witness(hit)) => {
                    let point = socket.world_point(DVec3::from_array(hit.mesh));
                    let source_subticks = window.source_at(fraction);
                    let offset = offset_from_axis(point, world.map(Vec3::as_dvec3));
                    let distance = offset.length();
                    if !(FIRST as f64..END as f64).contains(&source_subticks)
                        || distance + 1e-9 >= f64::from(capsule.radius)
                    {
                        return Probe::Unsupported;
                    }
                    possible |= region_bit;
                    witness.get_or_insert((
                        region_bit,
                        OverheadShadowWitness {
                            earliest_fraction: fraction,
                            witnessed_fraction: fraction,
                            travel_bound: 0.0,
                            source_subticks,
                            capsule: index as u8,
                            triangle: hit.triangle,
                            steel_point: point.to_array(),
                            normal: if distance > 1e-9 { (offset / distance).as_vec3() } else { Vec3::Y },
                            attacker,
                            target,
                        },
                    ));
                }
                Err(_) => return Probe::Unsupported,
            }
        }
        Probe::Tested { possible, witness, geometry }
    }

    fn inspect(&mut self, bodies: &BodyAuthority, window: OwnerWindow) -> OverheadShadowResult {
        let Some((regions, total)) = self.travel(bodies) else {
            return OverheadShadowResult::Unresolved {
                detail: OverheadShadowUncertainty::at_endpoint(OverheadShadowFailure::BodyInterval),
                probes: 0,
            };
        };
        let active = ((1_u32 << self.anatomy.len()) - 1) as u16;
        if total.cut {
            if window.interval.to != 1.0 {
                return OverheadShadowResult::Separated { probes: 0 };
            }
            return match self.probe(bodies, window, &[Travel::ZERO; CAPACITY], 1.0, None, active) {
                Probe::Tested { witness: Some((_, contact)), .. } => {
                    OverheadShadowResult::Witness { contact, probes: 1 }
                }
                Probe::Tested { possible: 0, .. } => OverheadShadowResult::Separated { probes: 1 },
                Probe::Tested { .. } => OverheadShadowResult::Unresolved {
                    detail: OverheadShadowUncertainty::at_endpoint(OverheadShadowFailure::NoWitness),
                    probes: 1,
                },
                Probe::Unsupported => OverheadShadowResult::Unresolved {
                    detail: OverheadShadowUncertainty::at_endpoint(OverheadShadowFailure::UnsupportedSample),
                    probes: 1,
                },
            };
        }
        let (max_bracket_fraction, max_bracket_travel) = self.bracket.limits();
        let policy = Policy {
            leaves: 12,
            max_probes: 128,
            max_bracket_fraction,
            max_bracket_travel,
            motion: regions
                .map(|region| Motion { rate: region.rate, margin: region.margin + BRACKET_ROUNDOFF_M }),
        };
        let steel_radius = f64::from(self.steel_radius);
        let steel_bounds = self.steel_bounds;
        let (result, probes) = search::sweep(
            window.interval,
            active,
            policy,
            |at, bounds, mask| self.probe(bodies, window, &regions, at, bounds, mask),
            |before, after, mask| {
                regions
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| mask & (1 << i) != 0)
                    .map(|(i, region)| before.bracket_travel(after, *region, i, steel_radius, steel_bounds))
                    .fold(0.0, f64::max)
            },
        );
        match result {
            Outcome::Separated => OverheadShadowResult::Separated { probes },
            Outcome::Witness { earliest_bound, travel_bound, evidence, .. } => {
                OverheadShadowResult::Witness {
                    contact: OverheadShadowWitness {
                        earliest_fraction: earliest_bound,
                        travel_bound,
                        ..evidence
                    },
                    probes,
                }
            }
            Outcome::Unresolved(detail) => OverheadShadowResult::Unresolved { detail, probes },
        }
    }
}

fn attached_radius(point: Vec3) -> f32 {
    (point.as_dvec3().length() as f32).next_up()
}

fn offset_from_axis(point: DVec3, [first, last]: [DVec3; 2]) -> DVec3 {
    let axis = last - first;
    let length_squared = axis.length_squared();
    let fraction = if length_squared == 0.0 {
        0.0
    } else {
        ((point - first).dot(axis) / length_squared).clamp(0.0, 1.0)
    };
    point - (first + axis * fraction)
}

fn active_window(world: &World) -> Option<(OwnerWindow, CurrentMove, Entity)> {
    let shadow = world.resource::<Shadow>();
    let table = world.resource::<ActorTable>();
    let attacker = view(world, shadow.attacker, table.entity(shadow.attacker)?);
    let target_entity = table.entity(shadow.target)?;
    let target = view(world, shadow.target, target_entity);
    if attacker.off_field
        || target.off_field
        || attacker.fighter.is_defeated()
        || target.fighter.is_defeated()
        || !target.fighter.is_fighter()
        || target.moves.windows.invulnerable
    {
        return None;
    }
    let action = attacker.moves.current.filter(|action| action.name == OVERHEAD)?;
    Some((OwnerWindow::from_step(action.elapsed, attacker.clock_delta)?, action, target_entity))
}

type AdvanceParams<'w, 's> = ParamSet<
    'w,
    's,
    (&'static World, (ResMut<'static, Shadow>, Res<'static, BodyAuthority>, ResMut<'static, HitQuery>)),
>;

fn advance(world: &mut World, cache: &mut SystemState<AdvanceParams<'_, '_>>) {
    let mut inputs = cache.get_mut(world).expect("the shadow is installed with both bodies");
    let world = inputs.p0();
    let window = active_window(world);
    let route = world.resource::<Shadow>().delivery;
    let source = route.and_then(|route| {
        let (_, action, target) = window?;
        if world.resource::<ActorTable>().len() != 2 {
            return None;
        }
        route.source(world, target, action)
    });
    let (mut shadow, bodies, mut hits) = inputs.p1();
    shadow.result =
        window.map_or(OverheadShadowResult::Inactive, |(window, _, _)| shadow.inspect(&bodies, window));
    if let Some(route) = route {
        let candidate = match (source, shadow.result) {
            (Some(source), OverheadShadowResult::Witness { contact, .. }) => {
                Candidate::new(route, source, contact)
            }
            _ => None,
        };
        hits.overhead_delivery = Some(Delivery { route, candidate, recorded: None });
    }
}

pub(crate) fn write_delivery_identity(world: &World, hash: &mut StateHasher) {
    if let Some(route) = world.get_resource::<Shadow>().and_then(|shadow| shadow.delivery) {
        hash.bytes(b"sapphire.applied-overhead.v1");
        hash.u64(route.fingerprint);
    }
}

impl Simulation {
    /// Attach an isolated read-only shadow before the first fixed step. Ordinary hit queries and
    /// their eligibility are unchanged. No result from this observer is applied to gameplay.
    #[doc(hidden)]
    pub fn attach_overhead_shadow(
        &mut self,
        attacker: ActorId,
        target: ActorId,
        shape: OverheadShadowShape,
    ) -> bool {
        self.attach_overhead(attacker, target, shape, None)
    }

    /// Binds the private contact route and its calibrated heroine and blade mesh identities.
    #[doc(hidden)]
    pub fn attach_overhead_application(
        &mut self,
        attacker: ActorId,
        target: ActorId,
        shape: OverheadShadowShape,
        cooked_meshes: [[u8; 32]; 2],
    ) -> bool {
        self.attach_overhead(attacker, target, shape, Some(cooked_meshes))
    }

    fn attach_overhead(
        &mut self,
        attacker: ActorId,
        target: ActorId,
        shape: OverheadShadowShape,
        cooked_meshes: Option<[[u8; 32]; 2]>,
    ) -> bool {
        if self.tick() != 0 || attacker == target || self.world().contains_resource::<Shadow>() {
            return false;
        }
        let (Some(warrior), Some(heroine)) =
            (self.body_sample_buffer(attacker), self.body_sample_buffer(target))
        else {
            return false;
        };
        let route = if let Some(meshes) = cooked_meshes {
            let Some(route) = delivery::Route::bind(self, attacker, target, &shape, meshes) else {
                return false;
            };
            Some(route)
        } else {
            None
        };
        let Some(mut shadow) = Shadow::bind(attacker, target, warrior, heroine, shape) else { return false };
        shadow.delivery = route;
        self.insert_resource(shadow);
        self.add_combat_system(CombatSet::Hits, advance.before(sweep_hits));
        true
    }

    #[doc(hidden)]
    pub fn overhead_shadow(&self) -> Option<OverheadShadowResult> {
        Some(self.world().get_resource::<Shadow>()?.result)
    }

    /// The target and attacker with a configured anatomical overhead route, in arena order.
    #[must_use]
    pub fn overhead_presentation_pair(&self) -> Option<[ActorId; 2]> {
        let route = self.world().get_resource::<Shadow>()?.delivery?;
        Some([route.target.actor(), route.attacker])
    }

    /// A matching applied hit from this exact fixed step.
    #[doc(hidden)]
    pub fn applied_overhead_contact(&self) -> Option<OverheadAppliedContact> {
        delivery::applied(self, self.hits().overhead_delivery?.recorded?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_chord_bound_cancels_shared_translation_but_keeps_full_turn_sag() {
        let identity = Socket::bind(Mat4::IDENTITY, 1.0).unwrap();
        let initial =
            ShapeFrame { fraction: 0.0, socket: identity, axes: [[Vec3::Y, Vec3::Y * 2.0]; CAPACITY] };
        let shift = Vec3::new(0.5, 0.25, -0.25);
        let translated = ShapeFrame {
            fraction: 1.0,
            socket: Socket::bind(Mat4::from_translation(shift), 1.0).unwrap(),
            axes: initial.axes.map(|axis| axis.map(|point| point + shift)),
        };
        let translation = Travel { rate: 2.0, margin: 0.0002, ..Travel::ZERO };
        assert!(initial.travel(&translated, translation, 0, 1.2) < 0.000201);
        let turn = std::f64::consts::TAU;
        let after_full_turn = ShapeFrame { fraction: 1.0, ..initial };
        let rotation = Travel { rate: turn, acceleration: turn * turn, ..Travel::ZERO };
        let bound = initial.travel(&after_full_turn, rotation, 0, 1.0);
        assert!((2.0..turn).contains(&bound));
    }

    #[test]
    fn timing_control_keeps_the_strict_spatial_limit() {
        let strict = OverheadShadowBracket::default().limits();
        let timing = OverheadShadowBracket::TimingOnly.limits();
        let engine = OverheadShadowBracket::EngineSpatial.limits();
        assert_eq!(strict, (1.0 / 1024.0, 0.002));
        assert_eq!(timing, (1.0 / 64.0, strict.1));
        assert_eq!(engine, (timing.0, f64::from(MAX_SAG)));
    }

    #[test]
    fn owner_window_clips_nonunit_steps_and_never_admits_a_frozen_attacker() {
        let at = SimDuration::from_subticks;
        assert!(OwnerWindow::from_step(at(47_000), SimDuration::ZERO).is_none());
        assert!(OwnerWindow::from_step(at(44_999), at(250)).is_none());

        let opening = OwnerWindow::from_step(at(45_125), at(250)).unwrap();
        assert_eq!(opening.interval.from, 0.5);
        assert_eq!(opening.source_at(opening.interval.from), 45_000.0);
        assert_eq!(opening.interval.to, 1.0);

        let closing = OwnerWindow::from_step(at(56_500), at(3_000)).unwrap();
        assert_eq!(closing.interval.from, 0.0);
        assert!(closing.source_at(closing.interval.to) < 56_000.0);
        assert!(closing.source_at(closing.interval.to.next_up()) >= 56_000.0);
        assert!(OwnerWindow::from_step(at(59_000), at(3_000)).is_none());
    }
}
