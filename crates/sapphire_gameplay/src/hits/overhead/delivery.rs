//! A private, explicitly bound source for one resolved overhead contact.
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_physics::GroupMask;
use sapphire_time::{SimDuration, SimTime};

use super::{END, FIRST, OVERHEAD, OverheadShadowShape, OverheadShadowWitness};
use crate::checksum::{StateHasher, write_shape};
use crate::controller::CharacterWorld;
use crate::hits::{HitEvent, HurtboxId, hit_shape, published_hurtbox};
use crate::moves::{CurrentMove, MoveLibrary, MoveOutput, MoveRunner};
use crate::resolve::{AppliedHits, HitOutcome};
use crate::{ActorId, MoveRef, MoveSetId, Simulation};

pub(in crate::hits) const HIT: Name = Name::new("overhead_hit");
const BLADE: Name = Name::new("blade");
const BODY: Name = Name::new("body");

/// The exact resolved hit and the paired body samples that gave rise to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverheadAppliedContact {
    pub event: HitEvent,
    pub witness: OverheadShadowWitness,
    /// Zero when the hit did not uniquely stop both participants.
    pub pair_stop_ticks: u16,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::hits) struct Route {
    pub attacker: ActorId,
    pub target: HurtboxId,
    movement: MoveRef,
    target_set: MoveSetId,
    target_revision: u32,
    body: sapphire_moves::Shape,
    pub(super) fingerprint: u64,
}

impl Route {
    pub(super) fn bind(
        simulation: &Simulation,
        attacker: ActorId,
        target: ActorId,
        shape: &OverheadShadowShape,
        mesh_identities: [[u8; 32]; 2],
    ) -> Option<Self> {
        if simulation.actors().len() != 2 || simulation.party().is_formed() {
            return None;
        }
        let library = simulation.move_library();
        let set = simulation.actor(attacker)?.moves.set?;
        let revision = library.current_revision(set)?;
        let (index, movement) = library.current(set)?.find(OVERHEAD)?;
        let [volume] = &movement.hitboxes[..] else { return None };
        if movement.startup != 45
            || movement.active != 6
            || volume.name() != HIT
            || volume.span.from != 45
            || volume.span.to != 51
            || volume.path() != BLADE
            || volume.attack.is_none()
            || !volume.rearm.is_empty()
            || volume.blocked_by() != GroupMask::NONE
        {
            return None;
        }
        let target_set = simulation.actor(target)?.moves.set?;
        let target_revision = library.current_revision(target_set)?;
        let [body] = &library.current(target_set)?.fighter()?.hurtboxes[..] else { return None };
        if body.name() != BODY {
            return None;
        }
        let mut route = Self {
            attacker,
            target: HurtboxId::new(target, 0)?,
            movement: MoveRef { set, revision, index },
            target_set,
            target_revision,
            body: body.shape,
            fingerprint: 0,
        };
        route.fingerprint = route.profile_fingerprint(shape, mesh_identities);
        Some(route)
    }

    pub(super) fn source(self, world: &World, target: Entity, action: CurrentMove) -> Option<AttackSource> {
        if world.get::<MoveRunner>(target)?.set() != Some(self.target_set)
            || world.resource::<MoveLibrary>().current_revision(self.target_set) != Some(self.target_revision)
        {
            return None;
        }
        let output = world.get::<MoveOutput>(target)?;
        let shape = published_hurtbox(BODY, hit_shape(self.body), output)?;
        if action.at != self.movement
            || world.resource::<CharacterWorld>().physics().body_shape(self.target.body()) != Some(shape)
        {
            return None;
        }
        let target_clock = &world.get::<EntityClock>(target)?.0;
        Some(AttackSource {
            movement: action.at,
            started_at: action.started_at,
            serial: action.serial,
            target_free: target_clock.hitstop_ticks() == 0 && target_clock.last_delta() != SimDuration::ZERO,
        })
    }

    fn profile_fingerprint(&self, shape: &OverheadShadowShape, mesh_identities: [[u8; 32]; 2]) -> u64 {
        let mut hash = StateHasher::new();
        hash.bytes(b"sapphire.applied-overhead.v2");
        for digest in mesh_identities {
            hash.bytes(&digest);
        }
        for value in [
            self.attacker.raw(),
            self.target.actor().raw(),
            self.movement.set.raw(),
            self.movement.revision,
            self.movement.index,
            self.target_set.raw(),
            self.target_revision,
        ] {
            hash.u32(value);
        }
        write_shape(&mut hash, self.body);
        let (time, space) = shape.bracket.limits();
        for value in [FIRST, END, time.to_bits(), space.to_bits()] {
            hash.u64(value);
        }
        hash.u16(shape.socket);
        hash.f32(shape.warrior_scale);
        hash.f32(shape.heroine_scale);
        hash.u64(shape.steel_vertices.len() as u64);
        for &point in &shape.steel_vertices {
            hash.vec3(point);
        }
        hash.u64(shape.steel_triangles.len() as u64);
        for triangle in &shape.steel_triangles {
            for &index in triangle {
                hash.u32(index);
            }
        }
        hash.u64(shape.anatomy.len() as u64);
        for capsule in &shape.anatomy {
            hash.f32(capsule.radius);
            for end in capsule.ends {
                hash.u16(end.joint);
                hash.u64(end.name.as_u64());
                hash.vec3(end.local);
            }
        }
        hash.finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AttackSource {
    pub(crate) movement: MoveRef,
    pub(crate) started_at: SimTime,
    serial: u32,
    target_free: bool,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::hits) struct Candidate {
    pub event: HitEvent,
    pub source: AttackSource,
    pub witness: OverheadShadowWitness,
}

impl Candidate {
    pub(super) fn new(route: Route, source: AttackSource, witness: OverheadShadowWitness) -> Option<Self> {
        let event = HitEvent {
            attacker: route.attacker,
            instance: source.serial,
            target: route.target.actor(),
            hurtbox: route.target.index(),
            group: HIT,
            volley: 0,
            volume: HitEvent::PRIVATE_VOLUME,
            point: sapphire_math::glam::DVec3::from_array(witness.steel_point).as_vec3(),
            normal: witness.normal,
            at: event_time(witness.source_subticks)?,
            // A private candidate cannot compete in the ordinary travel ordering.
            travel: 0.0,
        };
        Some(Self { event, source, witness })
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::hits) struct Delivery {
    pub route: Route,
    pub candidate: Option<Candidate>,
    pub recorded: Option<Candidate>,
}

fn event_time(source: f64) -> Option<SimDuration> {
    if !(FIRST as f64..END as f64).contains(&source) {
        return None;
    }
    Some(SimDuration::from_subticks((source.round() as u64).clamp(FIRST, END - 1)))
}

pub(super) fn applied(simulation: &Simulation, candidate: Candidate) -> Option<OverheadAppliedContact> {
    let Candidate { event, witness, source } = candidate;
    if witness.attacker.after.step != simulation.tick() || witness.target.after.step != simulation.tick() {
        return None;
    }
    let applied = &simulation.world().resource::<AppliedHits>().0;
    let hit = applied.iter().find(|hit| hit.event == event)?;
    let competing = applied.iter().any(|other| {
        other.event != event
            && ([event.attacker, event.target].contains(&other.event.attacker)
                || [event.attacker, event.target].contains(&other.event.target)
                || (other.outcome == HitOutcome::Defeated && other.attack.finish_freeze > 0))
    });
    let stop = if hit.outcome == HitOutcome::Defeated {
        hit.attack.hitstop.max(hit.attack.finish_freeze)
    } else {
        hit.attack.hitstop
    };
    let owns_stop = !competing
        && source.target_free
        && hit.outcome != HitOutcome::Absorbed
        && [event.attacker, event.target]
            .into_iter()
            .all(|actor| simulation.actor(actor).is_some_and(|view| view.hitstop_ticks == stop));
    Some(OverheadAppliedContact { event, witness, pair_stop_ticks: if owns_stop { stop } else { 0 } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn witnessed_time_stays_inside_the_half_open_source_window() {
        assert_eq!(event_time(44_999.99), None);
        assert_eq!(event_time(45_000.0), Some(SimDuration::from_subticks(45_000)));
        assert_eq!(event_time(45_000.75), Some(SimDuration::from_subticks(45_001)));
        assert_eq!(event_time(55_999.75), Some(SimDuration::from_subticks(55_999)));
        assert_eq!(event_time(56_000.0), None);
    }

    #[test]
    fn private_candidate_preserves_current_and_spent_dodges_and_competing_contacts() {
        use sapphire_body_pose::{BodyReadSpan, GraphStepStamp};
        use sapphire_math::Vec3;

        use crate::hits::{DodgedSwing, HitQuery};
        use crate::replay::{SCENARIOS, find_scenario};

        let scenario = find_scenario(SCENARIOS, "arena-fight").unwrap();
        let (simulation, cast) = scenario.start(scenario.config).unwrap();
        let [target, attacker] = cast[..2].try_into().unwrap();
        let set = simulation.actor(attacker).unwrap().moves.set.unwrap();
        let library = simulation.move_library();
        let (index, _) = library.current(set).unwrap().find(OVERHEAD).unwrap();
        let target_set = simulation.actor(target).unwrap().moves.set.unwrap();
        let movement = MoveRef { set, index, revision: library.current_revision(set).unwrap() };
        let route = Route {
            attacker,
            target: HurtboxId::new(target, 0).unwrap(),
            movement,
            target_set,
            target_revision: library.current_revision(target_set).unwrap(),
            body: sapphire_moves::Shape::Sphere { radius: 0.3 },
            fingerprint: 0,
        };
        let source =
            AttackSource { movement, started_at: SimTime::from_subticks(500), serial: 7, target_free: true };
        let stamp = GraphStepStamp { step: 54, owner_now: SimTime::from_subticks(54_000), generation: 0 };
        let span = BodyReadSpan { before: stamp, after: stamp, fraction: 1.0 };
        let candidate = Candidate::new(
            route,
            source,
            OverheadShadowWitness {
                earliest_fraction: 1.0,
                witnessed_fraction: 1.0,
                travel_bound: 0.0,
                source_subticks: 53_500.0,
                capsule: 0,
                triangle: 0,
                steel_point: [0.0, 1.0, 0.0],
                normal: Vec3::Y,
                attacker: span,
                target: span,
            },
        )
        .unwrap();
        let queue = |hits: &mut HitQuery, candidate| {
            hits.overhead_delivery = Some(Delivery { route, candidate: Some(candidate), recorded: None });
        };
        let dodge = DodgedSwing {
            dodger: target,
            attacker,
            instance: candidate.event.instance,
            group: HIT,
            volley: 0,
            volume: 0,
            at: SimDuration::from_subticks(49_000),
            travel: 0.3,
        };
        let mut hits = HitQuery::new();
        hits.swings.push(dodge);
        queue(&mut hits, candidate);
        hits.finish_step();
        assert!(hits.events().is_empty());
        assert_eq!(hits.dodged(), &[dodge]);
        assert_eq!(hits.record_count(), 0);

        hits.spend_swing(&dodge);
        queue(&mut hits, candidate);
        hits.finish_step();
        assert!(hits.events().is_empty());
        assert!(hits.dodged().is_empty());
        assert_eq!(hits.record_count(), 1);

        let later = Candidate::new(route, AttackSource { serial: 8, ..source }, candidate.witness).unwrap();
        let opposing = HitEvent {
            attacker: target,
            target: attacker,
            instance: 3,
            group: Name::new("light_hit"),
            volume: 0,
            ..candidate.event
        };
        assert!(hits.record_contact(opposing));
        queue(&mut hits, later);
        hits.finish_step();
        assert_eq!(hits.events(), &[opposing]);
        assert!(hits.private_attack_source(&later.event).is_none());

        queue(&mut hits, later);
        hits.finish_step();
        assert_eq!(hits.events(), &[later.event]);
        assert!(hits.private_attack_source(&later.event).is_some());
        assert_eq!(hits.record_count(), 2, "the new warrior instance retired its old record");
    }
}
