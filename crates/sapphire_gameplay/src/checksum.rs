//! The state checksum: everything that can change a result, as canonical bytes, in a fixed hash.
//!
//! **A component that can change a result joins [`state_checksum`]'s loop.** Forgetting one doesn't
//! break a replay at once: the checksum only sees the difference when it reaches a field that's
//! written here, and later than it could.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::{HurtboxState, PathSample, RootSample, Shape};
use xxhash_rust::xxh3::Xxh3;

use crate::actions::{ActorActions, Controls};
use crate::actors::{ActorId, ActorRequests, ActorTable};
use crate::brain::{Brain, BrainState};
use crate::controller::{
    CharacterController, CharacterWorld, JUMP_REBOUND_WINDOW, MOVING_JUMP_MIN_ALIGNMENT,
    MOVING_JUMP_MIN_SPEED, MOVING_JUMP_PREPARATION, REBOUND_JUMP_PREPARATION, REBOUND_MIN_LANDING_SPEED,
    STANDING_JUMP_MAX_SPEED, STANDING_JUMP_PREPARATION,
};
use crate::cues::CueQueue;
use crate::encounter::SimZone;
use crate::faction::Faction;
use crate::hits::HitQuery;
use crate::intents::IntentRing;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveOutput, MoveRef, MoveRunner, Running, WarpTarget};
use crate::navigation::SimNavigation;
use crate::party::Party;
use crate::resolve::{Fighter, HitOutcome, Slide, SlowMotion};
use crate::simulation::SimRng;
use crate::targeting::{SoftLock, TargetingTuning};
use crate::timeline::{TimelineLibrary, TimelineRun};

/// Goes up when [`state_checksum`] starts writing something different, so a recorded checksum can't
/// match by accident. Recorded replays re-record then (`docs/m2-plan.md` §3). 2: M2 G2's moves. 3: M2
/// D4's hit queries. 4: M2 G1's controllers and their world. 5: M2 G3's fighters, slow motion and hit
/// cues. 6: M2 G4's perfect dodges and counter windows. 7: M2 G6's brains and a controller drive's facing.
/// 8: M2 G7's party, which writes nothing without one. M3 P2's paths write nothing without a navmesh
/// either, and none of what's recorded before them has one, so they left this where it was.
const CHECKSUM_LAYOUT: u32 = 8;

/// A hasher over canonical bytes: XXH3-64, a published, frozen algorithm, unlike `DefaultHasher`,
/// so a recorded checksum stays good.
///
/// Canonical means nothing depends on how the state lies in memory: integers are little-endian,
/// floats go by their bits (one NaN for all, since Rust doesn't promise a NaN's payload), actors
/// are in ID order and a struct's fields in the order the checksum writes them.
pub struct StateHasher(Xxh3);

impl StateHasher {
    /// A hasher with nothing written. On the stack: no call here allocates.
    #[must_use]
    pub fn new() -> Self {
        Self(Xxh3::new())
    }

    /// Writes raw bytes.
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    pub fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    /// Writes a float by its bits, so −0 and 0 differ, and any NaN as the same one.
    pub fn f32(&mut self, value: f32) {
        self.u32(if value.is_nan() { f32::NAN.to_bits() } else { value.to_bits() });
    }

    /// Writes each component by [`Self::f32`].
    pub fn f32s(&mut self, values: &[f32]) {
        for &value in values {
            self.f32(value);
        }
    }

    /// Writes a vector's components in order.
    pub fn vec3(&mut self, value: Vec3) {
        self.f32s(&value.to_array());
    }

    /// The hash of everything written so far.
    #[must_use]
    pub fn finish(&self) -> u64 {
        self.0.digest()
    }
}

impl Default for StateHasher {
    fn default() -> Self {
        Self::new()
    }
}

fn write_actions(hasher: &mut StateHasher, actions: ActorActions) {
    hasher.u16(actions.buttons.bits());
    hasher.bytes(&actions.stick.x().to_le_bytes());
    hasher.bytes(&actions.stick.y().to_le_bytes());
}

fn write_root_sample(hasher: &mut StateHasher, sample: RootSample) {
    hasher.f32s(&sample.translation);
    hasher.f32(sample.yaw);
}

fn write_name(hasher: &mut StateHasher, name: Name) {
    hasher.u64(name.as_u64());
}

fn write_move_ref(hasher: &mut StateHasher, at: MoveRef) {
    hasher.u32(at.set.raw());
    hasher.u32(at.revision);
    hasher.u32(at.index);
}

pub(crate) fn write_shape(hasher: &mut StateHasher, shape: Shape) {
    match shape {
        Shape::Sphere { radius } => {
            hasher.u8(0);
            hasher.f32(radius);
        }
        Shape::Capsule { radius, length } => {
            hasher.u8(1);
            hasher.f32s(&[radius, length]);
        }
        Shape::Box { half_extents } => {
            hasher.u8(2);
            hasher.f32s(&half_extents);
        }
    }
}

fn write_path_sample(hasher: &mut StateHasher, sample: PathSample) {
    hasher.f32s(&sample.translation);
    hasher.f32s(&sample.rotation);
}

fn write_ring(hasher: &mut StateHasher, ring: &IntentRing) {
    hasher.u16(ring.held());
    hasher.u32(ring.len() as u32);
    for press in ring.iter() {
        write_name(hasher, press.name);
        hasher.u8(press.priority);
        hasher.u64(press.pressed_at.as_subticks());
        hasher.u64(press.step_start.as_subticks());
        hasher.u64(press.expires_at.as_subticks());
    }
}

fn write_running(hasher: &mut StateHasher, running: &Running) {
    write_move_ref(hasher, running.at);
    hasher.u64(running.started_at.as_subticks());
    hasher.u32(running.serial);
    hasher.u8(u8::from(running.fresh));
    hasher.f32(running.frame_yaw);
    write_root_sample(hasher, running.reading);
    // `u32::MAX` stands for none, which no window's index reaches.
    hasher.u32(running.warp_window.unwrap_or(u32::MAX));
    hasher.f32(running.warp_distance);
    hasher.f32(running.warp_turn);
    hasher.u8(u8::from(running.dodged));
}

fn write_runner(hasher: &mut StateHasher, runner: &MoveRunner) {
    // `u32::MAX` stands for none, which no set's ID reaches.
    hasher.u32(runner.set.map_or(u32::MAX, |set| set.raw()));
    hasher.u64(runner.cursor.as_subticks());
    hasher.u32(runner.started);
    match &runner.forced {
        Some(name) => {
            hasher.u8(1);
            write_name(hasher, *name);
        }
        None => hasher.u8(0),
    }
    match &runner.running {
        Some(running) => {
            hasher.u8(1);
            write_running(hasher, running);
        }
        None => hasher.u8(0),
    }
    match &runner.counter {
        Some(counter) => {
            hasher.u8(1);
            write_move_ref(hasher, counter.at);
            hasher.u32(counter.window);
            hasher.u64(counter.opens.as_subticks());
            hasher.u64(counter.closes.as_subticks());
        }
        None => hasher.u8(0),
    }
}

fn write_output(hasher: &mut StateHasher, output: &MoveOutput) {
    hasher.vec3(output.root_delta.translation);
    hasher.f32(output.root_delta.yaw);
    hasher.vec3(output.previous.position);
    hasher.f32(output.previous.yaw);
    let windows = output.windows;
    hasher.u8(u8::from(windows.invulnerable));
    hasher.u8(u8::from(windows.perfect_dodge));
    hasher.u8(u8::from(windows.parry));
    hasher.u8(u8::from(windows.cancel));
    hasher.u8(u8::from(windows.warp));
    hasher.u32(windows.armour);
    // `u32::MAX` stands for none, which no window's index reaches.
    hasher.u32(output.perfect_window.unwrap_or(u32::MAX));
    hasher.u32(output.volumes.len() as u32);
    for volume in output.volumes.iter() {
        write_move_ref(hasher, volume.at);
        hasher.u32(volume.serial);
        hasher.u64(volume.started_at.as_subticks());
        hasher.u32(volume.hitbox);
        write_name(hasher, volume.name);
        write_name(hasher, volume.path);
        write_shape(hasher, volume.shape);
        hasher.u64(volume.from.as_subticks());
        hasher.u64(volume.to.as_subticks());
        write_path_sample(hasher, volume.start);
        write_path_sample(hasher, volume.end);
    }
    hasher.u32(output.hurtboxes.len() as u32);
    for hurtbox in output.hurtboxes.iter() {
        write_name(hasher, hurtbox.name);
        match hurtbox.state {
            HurtboxState::Off => hasher.u8(0),
            HurtboxState::Shape(shape) => {
                hasher.u8(1);
                write_shape(hasher, shape);
            }
        }
    }
}

fn write_target(hasher: &mut StateHasher, target: WarpTarget) {
    match target {
        WarpTarget::None => hasher.u8(0),
        WarpTarget::Point(point) => {
            hasher.u8(1);
            hasher.vec3(point);
        }
        WarpTarget::Actor(id) => {
            hasher.u8(2);
            hasher.u32(id.raw());
        }
    }
}

fn write_slide(hasher: &mut StateHasher, slide: &Slide) {
    hasher.vec3(slide.direction);
    hasher.f32(slide.distance);
    hasher.u64(slide.length.as_subticks());
    hasher.u64(slide.elapsed.as_subticks());
    hasher.f32(slide.moved);
}

fn write_fighter(hasher: &mut StateHasher, fighter: &Fighter) {
    hasher.u32(fighter.health);
    hasher.u32(fighter.max_health);
    hasher.u32(fighter.poise);
    hasher.u32(fighter.max_poise);
    hasher.u64(fighter.poise_refill.as_subticks());
    match fighter.poise_refills_at {
        Some(at) => {
            hasher.u8(1);
            hasher.u64(at.as_subticks());
        }
        None => hasher.u8(0),
    }
    hasher.u8(u8::from(fighter.defeated));
    write_slide(hasher, &fighter.slide);
    // Nothing for no faction, which is a fight that names none: their checksums stay what they were.
    if fighter.faction != Faction::NONE {
        hasher.u8(fighter.faction.number());
    }
}

fn write_time(hasher: &mut StateHasher, time: Option<sapphire_time::SimTime>) {
    match time {
        Some(time) => {
            hasher.u8(1);
            hasher.u64(time.as_subticks());
        }
        None => hasher.u8(0),
    }
}

fn write_soft_lock(hasher: &mut StateHasher, lock: &SoftLock) {
    let SoftLock { on, target } = *lock;
    // Nothing for one that's off, which is most actors: their checksums stay what they were.
    if on {
        hasher.u32(target.map_or(0, |id| id.raw()));
    }
}

fn write_brain(hasher: &mut StateHasher, brain: &Brain) {
    let Brain { on, target, state, patience_until, circle, stick, last_used, encounter } = *brain;
    // Nothing for one that's off, which is most actors: their checksums stay what they were.
    if on {
        hasher.u8(1);
    }
    // Nor for one no encounter spawned.
    if let Some(encounter) = encounter {
        hasher.u8(encounter.wrapping_add(1));
    }
    hasher.u32(target.map_or(0, |id| id.raw()));
    match state {
        BrainState::Pacing => hasher.u8(0),
        BrainState::Attacking { serial } => {
            hasher.u8(1);
            hasher.u32(serial);
        }
        BrainState::Resting { until } => {
            hasher.u8(2);
            hasher.u64(until.as_subticks());
        }
        BrainState::Reacting { serial, rise_at } => {
            hasher.u8(3);
            hasher.u32(serial);
            // `u32::MAX` stands for none, which no move's length reaches.
            hasher.u32(rise_at.unwrap_or(u32::MAX));
        }
        BrainState::Defeated => hasher.u8(4),
    }
    hasher.u64(patience_until.as_subticks());
    hasher.bytes(&circle.to_le_bytes());
    hasher.bytes(&stick.x().to_le_bytes());
    hasher.bytes(&stick.y().to_le_bytes());
    for used in last_used {
        write_time(hasher, used);
    }
}

const fn outcome_code(outcome: HitOutcome) -> u8 {
    match outcome {
        HitOutcome::Flinched => 0,
        HitOutcome::Absorbed => 1,
        HitOutcome::PoiseBroken => 2,
        HitOutcome::Defeated => 3,
    }
}

fn write_cues(hasher: &mut StateHasher, cues: &CueQueue) {
    // Nothing simulated reads these, but a cue that goes missing or arrives twice is a break in a
    // replay that only here shows: what a player sees and hears (pillar 3).
    hasher.u32(cues.len() as u32);
    for event in cues.iter() {
        hasher.u32(event.actor.raw());
        write_name(hasher, event.cue);
        hasher.u64(event.tick);
        hasher.u64(event.local_time.as_subticks());
        hasher.u64(event.move_time.as_subticks());
        match event.hit {
            Some(hit) => {
                hasher.u8(1);
                hasher.u32(hit.target.raw());
                hasher.u8(hit.hurtbox);
                hasher.vec3(hit.point);
                hasher.vec3(hit.normal);
                hasher.u8(hit.weight as u8);
                hasher.u32(hit.damage);
                hasher.u32(hit.poise);
                hasher.u16(hit.hitstop);
                hasher.u8(outcome_code(hit.outcome));
            }
            None => hasher.u8(0),
        }
    }
    hasher.u64(cues.published());
    hasher.u64(cues.dropped());
}

/// The checksum of a simulation's whole state.
pub(crate) fn state_checksum(world: &World) -> u64 {
    let mut hasher = legacy_state(world);
    if let Some(body) = world.get_resource::<crate::body::BodyAuthority>() {
        body.write_state(&mut hasher);
    }
    crate::hits::overhead::write_delivery_identity(world, &mut hasher);
    hasher.finish()
}

pub(crate) fn legacy_state(world: &World) -> StateHasher {
    let mut hasher = StateHasher::new();
    hasher.u32(CHECKSUM_LAYOUT);

    // The tick count is in here, with the rest of the world clock's state.
    world.resource::<WorldClock>().0.write_canonical(&mut |bytes| hasher.bytes(bytes));
    for part in world.resource::<SimRng>().0.state() {
        hasher.u64(part);
    }
    hasher.u32(world.resource::<ActorRequests>().next_id());
    world.resource::<HitQuery>().write_canonical(&mut hasher);
    world.resource::<CharacterWorld>().physics().write_canonical(&mut |bytes| hasher.bytes(bytes));
    world.resource::<SlowMotion>().write_canonical(&mut hasher);
    world.resource::<Party>().write_canonical(&mut hasher);
    world.resource::<TimelineRun>().write_canonical(&mut hasher);

    let table = world.resource::<ActorTable>();
    hasher.u32(table.len() as u32);
    for (id, entity) in table.iter() {
        hasher.u32(id.raw());
        let actor = world.entity(entity);
        if let Some(clock) = actor.get::<EntityClock>() {
            clock.0.write_canonical(&mut |bytes| hasher.bytes(bytes));
        }
        if let Some(controls) = actor.get::<Controls>() {
            write_actions(&mut hasher, controls.current);
            write_actions(&mut hasher, controls.previous);
        }
        if let Some(transform) = actor.get::<ActorTransform>() {
            hasher.vec3(transform.position);
            hasher.f32(transform.yaw);
        }
        if let Some(runner) = actor.get::<MoveRunner>() {
            write_runner(&mut hasher, runner);
        }
        if let Some(ring) = actor.get::<IntentRing>() {
            write_ring(&mut hasher, ring);
        }
        if let Some(output) = actor.get::<MoveOutput>() {
            write_output(&mut hasher, output);
        }
        if let Some(target) = actor.get::<WarpTarget>() {
            write_target(&mut hasher, *target);
        }
        if let Some(controller) = actor.get::<CharacterController>() {
            controller.write_canonical(&mut hasher);
        }
        if let Some(fighter) = actor.get::<Fighter>() {
            write_fighter(&mut hasher, fighter);
        }
        if let Some(brain) = actor.get::<Brain>() {
            write_brain(&mut hasher, brain);
        }
        if let Some(lock) = actor.get::<SoftLock>() {
            write_soft_lock(&mut hasher, lock);
        }
    }
    write_cues(&mut hasher, world.resource::<CueQueue>());
    if let Some(navigation) = world.get_resource::<SimNavigation>() {
        navigation.write_state(&mut hasher);
    }
    if let Some(zone) = world.get_resource::<SimZone>() {
        zone.write_state(&mut hasher);
    }
    hasher
}

/// The hash of what a run depends on, for a replay to check it runs against what it recorded: each
/// move set's current revision, whole, with its tables, in ID order, the controller profiles and the
/// collision world's statics. It hashes a set's cooked bytes, so no field can be forgotten here, and
/// those are the same on every target, as the statics' digest is: authored numbers, by their bits. A party's
/// tuning goes in too, if there's a party, a navmesh's digest if there's one of those, every loaded
/// timeline's simulation half, if there are any, targeting's weights, and what a zone says if there's
/// one. Opted-in standing jump preparation contributes the policy and setup actor IDs independently of
/// any jump's live countdown. Each of those is a resource of `world`, or isn't there.
pub(crate) fn tables_hash(world: &World, standing_jump_actors: &[ActorId]) -> u64 {
    let library = world.resource::<MoveLibrary>();
    let characters = world.resource::<CharacterWorld>();
    let party = world.resource::<Party>();
    let navigation = world.get_resource::<SimNavigation>();
    let timelines = world.resource::<TimelineLibrary>();
    let targeting = world.resource::<TargetingTuning>();
    let zone = world.get_resource::<SimZone>();
    let mut hasher = StateHasher::new();
    let hashes = library.current_hashes();
    hasher.u32(hashes.len() as u32);
    for hash in hashes {
        hasher.u64(hash);
    }
    hasher.u64(characters.profiles_hash());
    hasher.u64(characters.physics().statics_digest());
    party.write_tuning(&mut hasher);
    if let Some(navigation) = navigation {
        hasher.u64(navigation.digest());
    }
    timelines.write_digests(&mut hasher);
    if !standing_jump_actors.is_empty() {
        // Revise this tag when the preparation eligibility or release rules change.
        hasher.bytes(b"grounded-jump-preparation-v2");
        hasher.u64(STANDING_JUMP_PREPARATION.as_subticks());
        hasher.f32(STANDING_JUMP_MAX_SPEED);
        hasher.u64(MOVING_JUMP_PREPARATION.as_subticks());
        hasher.f32(MOVING_JUMP_MIN_SPEED);
        hasher.f32(MOVING_JUMP_MIN_ALIGNMENT);
        hasher.u64(REBOUND_JUMP_PREPARATION.as_subticks());
        hasher.u64(JUMP_REBOUND_WINDOW.as_subticks());
        hasher.f32(REBOUND_MIN_LANDING_SPEED);
        hasher.u32(standing_jump_actors.len() as u32);
        for actor in standing_jump_actors {
            hasher.u32(actor.raw());
        }
    }
    hasher.u64(targeting.digest());
    if let Some(zone) = zone {
        hasher.u64(zone.tables());
    }
    hasher.finish()
}

#[cfg(test)]
mod sensitivity_tests {
    //! Flip each piece of move state, one at a time, and watch the checksum change: a state left out of
    //! the checksum is a replay bug waiting, and only shows much later, if ever.

    use sapphire_moves::MoveSetDocument;
    use sapphire_time::{SimDuration, SimTime};

    use super::*;
    use crate::actions::{ActorActions, Buttons, Stick, TickActions};
    use crate::actors::{ActorId, ActorSpawn};
    use crate::moves::{ActiveHurtbox, OpenCounter};
    use crate::simulation::{SimConfig, Simulation};

    const SLICE: &str = include_str!("../../sapphire_moves/test_content/slice.moves.ron");

    /// A world where every piece of move state is in use: one actor mid-swing with a volume live, a
    /// warp target, a warp under way and a press buffered, and another mid-dodge with a hurtbox off.
    fn world() -> (Simulation, ActorId, ActorId) {
        let set = MoveSetDocument::parse(SLICE).unwrap().compile().unwrap();
        let mut sim = Simulation::new(SimConfig::default());
        let moves = sim.add_move_set(set);
        let spawn = ActorSpawn { moves: Some(moves), ..ActorSpawn::default() };
        let (swinging, dodging) = (sim.spawn(spawn).unwrap(), sim.spawn(spawn).unwrap());
        sim.set_warp_target(swinging, WarpTarget::Point(Vec3::new(1.0, 0.0, 9.0)));
        sim.force_move(swinging, Name::new("dash_attack"));
        let press = |actor, buttons| {
            let mut actions = TickActions::NEUTRAL;
            actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).unwrap();
            actions
        };
        sim.step(&press(dodging, Buttons::DODGE));
        for _ in 0..3 {
            sim.step(&TickActions::NEUTRAL);
        }
        // Into the hit volume's window, and with a held press the dash attack's windows don't take.
        sim.step(&press(swinging, Buttons::SKILL));
        (sim, swinging, dodging)
    }

    /// Runs `edit` on `actor`'s `C`, and checks the checksum saw it.
    fn flips<C: Component<Mutability = bevy_ecs::component::Mutable> + Copy>(
        sim: &mut Simulation,
        actor: ActorId,
        what: &str,
        edit: impl FnOnce(&mut C),
    ) {
        let before = sim.checksum();
        let world = sim.ecs_mut().world_mut();
        let entity = world.resource::<ActorTable>().entity(actor).unwrap();
        let kept = *world.get::<C>(entity).unwrap();
        edit(&mut *world.get_mut::<C>(entity).unwrap());
        assert_ne!(sim.checksum(), before, "{what} isn't in the checksum");
        let world = sim.ecs_mut().world_mut();
        *world.get_mut::<C>(entity).unwrap() = kept;
        assert_eq!(sim.checksum(), before, "restoring {what} didn't restore the checksum");
    }

    fn running(runner: &mut MoveRunner) -> &mut Running {
        runner.running.as_mut().expect("a move is playing")
    }

    fn counter() -> OpenCounter {
        let at = MoveRef { set: crate::moves::MoveSetId(0), revision: 0, index: 3 };
        OpenCounter { at, window: 0, opens: SimTime::from_subticks(100), closes: SimTime::from_subticks(200) }
    }

    #[test]
    fn every_field_of_the_runner_is_in_the_checksum() {
        type Edit = fn(&mut MoveRunner);
        let edits: &[(&str, Edit)] = &[
            ("the set", |r| r.set = None),
            ("the cursor", |r| r.cursor = SimTime::from_subticks(r.cursor.as_subticks() + 1)),
            ("started", |r| r.started += 1),
            ("forced", |r| r.forced = Some(Name::new("x"))),
            ("running", |r| r.running = None),
            ("the move's index", |r| running(r).at.index += 1),
            ("the move's revision", |r| running(r).at.revision += 1),
            ("the move's set", |r| running(r).at.set = crate::moves::MoveSetId(7)),
            ("started_at", |r| {
                let at = running(r).started_at;
                running(r).started_at = SimTime::from_subticks(at.as_subticks() + 1);
            }),
            ("serial", |r| running(r).serial += 1),
            ("fresh", |r| running(r).fresh = !running(r).fresh),
            ("frame_yaw", |r| running(r).frame_yaw += 0.125),
            ("the reading's translation", |r| running(r).reading.translation[2] += 0.125),
            ("the reading's yaw", |r| running(r).reading.yaw += 0.125),
            ("the warp window", |r| running(r).warp_window = Some(9)),
            ("warp_distance", |r| running(r).warp_distance += 0.125),
            ("warp_turn", |r| running(r).warp_turn += 0.125),
            ("dodged", |r| running(r).dodged = !running(r).dodged),
            ("a counter window", |r| r.counter = Some(counter())),
        ];
        let (mut sim, swinging, _) = world();
        for (what, edit) in edits {
            flips::<MoveRunner>(&mut sim, swinging, what, edit);
        }

        // And each field of one that's open, against the one that is.
        let entity = sim.world().resource::<ActorTable>().entity(swinging).unwrap();
        sim.ecs_mut().world_mut().get_mut::<MoveRunner>(entity).unwrap().counter = Some(counter());
        type CounterEdit = fn(&mut OpenCounter);
        let counter_edits: &[(&str, CounterEdit)] = &[
            ("its dodge's index", |c| c.at.index += 1),
            ("its dodge's revision", |c| c.at.revision += 1),
            ("its dodge's set", |c| c.at.set = crate::moves::MoveSetId(7)),
            ("its window", |c| c.window += 1),
            ("when it opens", |c| c.opens = SimTime::from_subticks(c.opens.as_subticks() + 1)),
            ("when it closes", |c| c.closes = SimTime::from_subticks(c.closes.as_subticks() + 1)),
        ];
        for (what, edit) in counter_edits {
            flips::<MoveRunner>(&mut sim, swinging, what, |runner| edit(runner.counter.as_mut().unwrap()));
        }
    }

    #[test]
    fn every_field_of_the_ring_is_in_the_checksum() {
        let (mut sim, swinging, _) = world();
        let (intent, clock) = {
            let world = sim.ecs_mut().world_mut();
            let entity = world.resource::<ActorTable>().entity(swinging).unwrap();
            let set = world.get::<MoveRunner>(entity).unwrap().set().unwrap();
            let intent = world.resource::<MoveLibrary>().current(set).unwrap().intents()[0].clone();
            (intent, world.get::<EntityClock>(entity).unwrap().0.clone())
        };
        flips::<IntentRing>(&mut sim, swinging, "the holds", |ring| ring.set_held(ring.held() ^ 4));
        flips::<IntentRing>(&mut sim, swinging, "a press", |ring| {
            ring.press(&intent, &clock);
        });
        type Edit = fn(&mut crate::intents::Intent);
        let edits: &[(&str, Edit)] = &[
            ("a press's name", |press| press.name = Name::new("x")),
            ("a press's priority", |press| press.priority += 1),
            ("when it was pressed", |press| press.pressed_at = SimTime::from_subticks(1)),
            ("its step's start", |press| press.step_start = SimTime::from_subticks(1)),
            ("its expiry", |press| press.expires_at = SimTime::from_subticks(1)),
        ];
        for (what, edit) in edits {
            flips::<IntentRing>(&mut sim, swinging, what, |ring| edit(&mut ring.entries_mut()[0]));
        }
    }

    #[test]
    fn every_field_of_the_output_is_in_the_checksum() {
        let (mut sim, swinging, dodging) = world();
        type Edit = fn(&mut MoveOutput);
        let edits: &[(&str, Edit)] = &[
            ("the root's translation", |o| o.root_delta.translation.x += 0.125),
            ("the root's yaw", |o| o.root_delta.yaw += 0.125),
            ("the previous position", |o| o.previous.position.x += 0.125),
            ("the previous yaw", |o| o.previous.yaw += 0.125),
            ("invulnerable", |o| o.windows.invulnerable = !o.windows.invulnerable),
            ("perfect_dodge", |o| o.windows.perfect_dodge = !o.windows.perfect_dodge),
            ("parry", |o| o.windows.parry = !o.windows.parry),
            ("cancel", |o| o.windows.cancel = !o.windows.cancel),
            ("warp", |o| o.windows.warp = !o.windows.warp),
            ("armour", |o| o.windows.armour += 1),
            ("the perfect window", |o| o.perfect_window = Some(0)),
        ];
        for (what, edit) in edits {
            flips::<MoveOutput>(&mut sim, swinging, what, edit);
        }

        type VolumeEdit = fn(&mut crate::moves::ActiveVolume);
        let volume_edits: &[(&str, VolumeEdit)] = &[
            ("a volume's move", |v| v.at.index += 1),
            ("a volume's serial", |v| v.serial += 1),
            ("when its move began", |v| {
                v.started_at = SimTime::from_subticks(v.started_at.as_subticks() + 1)
            }),
            ("a volume's hitbox", |v| v.hitbox += 1),
            ("a volume's name", |v| v.name = Name::new("x")),
            ("a volume's path", |v| v.path = Name::new("x")),
            ("a volume's shape", |v| v.shape = Shape::Sphere { radius: 9.0 }),
            ("a volume's start", |v| v.from = SimDuration::from_subticks(1)),
            ("a volume's end", |v| v.to = SimDuration::from_subticks(1)),
            ("a volume's first sample", |v| v.start.translation[0] += 0.125),
            ("a volume's first rotation", |v| v.start.rotation[0] += 0.125),
            ("a volume's last sample", |v| v.end.translation[0] += 0.125),
            ("a volume's last rotation", |v| v.end.rotation[0] += 0.125),
        ];
        for (what, edit) in volume_edits {
            flips::<MoveOutput>(&mut sim, swinging, what, |output| edit(output.volumes.first_mut().unwrap()));
        }
        flips::<MoveOutput>(&mut sim, swinging, "the volume count", |output| output.volumes.clear());

        type HurtboxEdit = fn(&mut ActiveHurtbox);
        let hurtbox_edits: &[(&str, HurtboxEdit)] = &[
            ("a hurtbox's name", |h| h.name = Name::new("x")),
            ("a hurtbox's state", |h| h.state = HurtboxState::Shape(Shape::Sphere { radius: 1.0 })),
        ];
        for (what, edit) in hurtbox_edits {
            flips::<MoveOutput>(&mut sim, dodging, what, |output| {
                edit(output.hurtboxes.first_mut().unwrap())
            });
        }
        flips::<MoveOutput>(&mut sim, dodging, "the hurtbox count", |output| output.hurtboxes.clear());
    }

    #[test]
    fn the_warp_target_is_in_the_checksum() {
        let (mut sim, swinging, dodging) = world();
        flips::<WarpTarget>(&mut sim, swinging, "no target", |target| *target = WarpTarget::None);
        flips::<WarpTarget>(&mut sim, swinging, "another point", |target| {
            *target = WarpTarget::Point(Vec3::new(2.0, 0.0, 9.0));
        });
        flips::<WarpTarget>(&mut sim, swinging, "another actor", |target| {
            *target = WarpTarget::Actor(dodging)
        });
        flips::<WarpTarget>(&mut sim, dodging, "a first target", |target| {
            *target = WarpTarget::Actor(swinging)
        });
    }

    #[test]
    fn every_field_of_a_fighter_is_in_the_checksum() {
        type Edit = fn(&mut Fighter);
        let edits: &[(&str, Edit)] = &[
            ("health", |f| f.health += 1),
            ("max health", |f| f.max_health += 1),
            ("poise", |f| f.poise += 1),
            ("max poise", |f| f.max_poise += 1),
            ("the refill's length", |f| f.poise_refill = SimDuration::from_subticks(1)),
            ("when poise refills", |f| f.poise_refills_at = Some(SimTime::from_subticks(1))),
            ("defeated", |f| f.defeated = !f.defeated),
            ("a slide's direction", |f| f.slide.direction.x += 0.125),
            ("a slide's distance", |f| f.slide.distance += 0.125),
            ("a slide's length", |f| f.slide.length = SimDuration::from_subticks(1)),
            ("a slide's progress", |f| f.slide.elapsed = SimDuration::from_subticks(1)),
            ("how far a slide has gone", |f| f.slide.moved += 0.125),
            ("its side", |f| f.faction = Faction::new(1).unwrap()),
        ];
        let (mut sim, swinging, _) = world();
        for (what, edit) in edits {
            flips::<Fighter>(&mut sim, swinging, what, edit);
        }
        // And where it's a time that's already set, not just whether it is.
        let at = |f: &mut Fighter| f.poise_refills_at = Some(SimTime::from_subticks(7));
        flips::<Fighter>(&mut sim, swinging, "a set refill time", at);
        let world = sim.ecs_mut().world_mut();
        let entity = world.resource::<ActorTable>().entity(swinging).unwrap();
        at(&mut world.get_mut::<Fighter>(entity).unwrap());
        flips::<Fighter>(&mut sim, swinging, "another refill time", |f| {
            f.poise_refills_at = Some(SimTime::from_subticks(8));
        });
    }

    /// A lock that's off writes nothing, so a lock's target only counts once it's on.
    #[test]
    fn a_soft_lock_is_in_the_checksum_once_it_is_on() {
        let (mut sim, swinging, _) = world();
        flips::<SoftLock>(&mut sim, swinging, "being on", |lock| lock.on = true);
        let before = sim.checksum();
        sim.give_soft_lock(swinging);
        assert_ne!(sim.checksum(), before);
        flips::<SoftLock>(&mut sim, swinging, "who it has", |lock| lock.target = ActorId::from_raw(99));
    }

    #[test]
    fn every_field_of_a_brain_is_in_the_checksum() {
        let at = |subticks| SimTime::from_subticks(subticks);
        type Edit = fn(&mut Brain);
        let edits: &[(&str, Edit)] = &[
            ("being on", |b| b.on = !b.on),
            ("its target", |b| b.target = ActorId::from_raw(99)),
            ("when its patience runs out", |b| b.patience_until = SimTime::from_subticks(1)),
            ("which way it circles", |b| b.circle = 1),
            ("its stick's x", |b| b.stick = Stick::new(1, 0)),
            ("its stick's y", |b| b.stick = Stick::new(0, 1)),
            ("when it last used its first attack", |b| b.last_used[0] = Some(SimTime::from_subticks(1))),
            ("when it last used its last attack", |b| {
                b.last_used[sapphire_moves::MAX_BRAIN_ATTACKS - 1] = Some(SimTime::from_subticks(1));
            }),
        ];
        let (mut sim, swinging, _) = world();
        for (what, edit) in edits {
            flips::<Brain>(&mut sim, swinging, what, edit);
        }

        // Each state from every other, and what each holds.
        let states = [
            BrainState::Pacing,
            BrainState::Attacking { serial: 1 },
            BrainState::Attacking { serial: 2 },
            BrainState::Resting { until: at(1) },
            BrainState::Resting { until: at(2) },
            BrainState::Reacting { serial: 1, rise_at: None },
            BrainState::Reacting { serial: 2, rise_at: None },
            BrainState::Reacting { serial: 1, rise_at: Some(0) },
            BrainState::Reacting { serial: 1, rise_at: Some(1) },
            BrainState::Defeated,
        ];
        let sums: Vec<u64> = states
            .iter()
            .map(|&state| {
                let world = sim.ecs_mut().world_mut();
                let entity = world.resource::<ActorTable>().entity(swinging).unwrap();
                world.get_mut::<Brain>(entity).unwrap().state = state;
                sim.checksum()
            })
            .collect();
        for (index, sum) in sums.iter().enumerate() {
            assert!(!sums[..index].contains(sum), "{:?} looks like an earlier state", states[index]);
        }
    }

    #[test]
    fn a_drives_facing_is_in_the_checksum() {
        let (mut sim, swinging, _) = world();
        flips::<CharacterController>(&mut sim, swinging, "a drive's facing", |controller| {
            controller.drive.face = Some(0.25);
        });
        let world = sim.ecs_mut().world_mut();
        let entity = world.resource::<ActorTable>().entity(swinging).unwrap();
        world.get_mut::<CharacterController>(entity).unwrap().drive.face = Some(0.25);
        flips::<CharacterController>(&mut sim, swinging, "another facing", |controller| {
            controller.drive.face = Some(0.5);
        });
    }

    #[test]
    fn slow_motion_is_in_the_checksum() {
        type Edit = fn(&mut SlowMotion);
        let edits: &[(&str, Edit)] = &[
            ("a request", |s| s.requested = Some((200, 3))),
            ("steps left", |s| s.left += 1),
            ("refusals", |s| s.refused += 1),
        ];
        let (mut sim, ..) = world();
        for (what, edit) in edits {
            let before = sim.checksum();
            let kept = *sim.ecs_mut().world_mut().resource::<SlowMotion>();
            edit(&mut sim.ecs_mut().world_mut().resource_mut::<SlowMotion>());
            assert_ne!(sim.checksum(), before, "{what} isn't in the checksum");
            *sim.ecs_mut().world_mut().resource_mut::<SlowMotion>() = kept;
            assert_eq!(sim.checksum(), before, "restoring {what} didn't restore the checksum");
        }
        // Each number of a request, and which steps it asks for.
        sim.request_slow_motion(200, 3);
        let with_request = sim.checksum();
        sim.request_slow_motion(201, 3);
        assert_ne!(sim.checksum(), with_request);
        sim.request_slow_motion(200, 4);
        assert_ne!(sim.checksum(), with_request);
    }

    /// A party of three on the party scenario's moves, the second on the field, with the first resting.
    fn party_world() -> Simulation {
        let mut sim = Simulation::new(SimConfig::default());
        let mut members = Vec::new();
        for member in 0..crate::party::PARTY_SLOTS {
            let moves = Some(sim.add_move_set(crate::replay::party_moves(member).unwrap()));
            members.push(sim.spawn(ActorSpawn { moves, ..ActorSpawn::default() }).unwrap());
        }
        let tuning = crate::party::PartyTuning { swap_cooldown: 7, rest: 11 };
        sim.form_party(&members, tuning).unwrap();
        let mut swap = TickActions::NEUTRAL;
        swap.set(members[0], ActorActions { buttons: Buttons::SWAP, stick: Stick::CENTER }).unwrap();
        sim.step(&swap);
        assert_eq!(sim.party().active(), Some(members[1]));
        sim
    }

    #[test]
    fn every_field_of_the_party_is_in_the_checksum() {
        type Edit = fn(&mut crate::party::Formed);
        let edits: &[(&str, Edit)] = &[
            ("a slot's actor", |f| f.slots[0].as_mut().unwrap().actor = ActorId::from_raw(99).unwrap()),
            ("a slot's readiness", |f| {
                let slot = f.slots[0].as_mut().unwrap();
                slot.ready_at = SimTime::from_subticks(slot.ready_at.as_subticks() + 1);
            }),
            ("an emptied slot", |f| f.slots[2] = None),
            ("who's on the field", |f| f.active = 2),
            ("the cooldown", |f| f.tuning.swap_cooldown += 1),
            ("the rest", |f| f.tuning.rest += 1),
            ("swaps", |f| f.swaps += 1),
            ("forced swaps", |f| f.forced_swaps += 1),
            ("down", |f| f.down = !f.down),
        ];
        let mut sim = party_world();
        for (what, edit) in edits {
            let before = sim.checksum();
            let kept = *sim.ecs_mut().world_mut().resource::<Party>();
            let mut party = sim.ecs_mut().world_mut().resource_mut::<Party>();
            edit(party.formed.as_mut().unwrap());
            assert_ne!(sim.checksum(), before, "{what} isn't in the checksum");
            *sim.ecs_mut().world_mut().resource_mut::<Party>() = kept;
            assert_eq!(sim.checksum(), before, "restoring {what} didn't restore the checksum");
        }
    }

    /// What lets a fight with no party keep its checksums but for the layout's number.
    #[test]
    fn no_party_writes_nothing() {
        let write = |write: &dyn Fn(&mut StateHasher)| {
            let mut hasher = StateHasher::new();
            write(&mut hasher);
            hasher.finish()
        };
        let party = Party::default();
        let nothing = write(&|_| {});
        assert_eq!(write(&|hasher| party.write_canonical(hasher)), nothing);
        assert_eq!(write(&|hasher| party.write_tuning(hasher)), nothing);
    }

    /// A hurtbox is state between steps, and a query could read it.
    #[test]
    fn the_physics_worlds_bodies_are_in_the_checksum() {
        let (mut sim, swinging, _) = world();
        let before = sim.checksum();
        let body = crate::HurtboxId::new(swinging, 0).unwrap().body();
        let characters = sim.ecs_mut().world_mut().resource_mut::<CharacterWorld>().into_inner();
        let pose = characters.physics().body_pose(body).expect("it has a hurtbox");
        let moved = sapphire_physics::Pose::new(pose.position + Vec3::X, pose.rotation);
        characters.physics_mut().set_body_pose(body, moved).unwrap();
        assert_ne!(sim.checksum(), before);
    }

    /// Nothing reads the cue queue, so its effect on a run never shows except here.
    #[test]
    fn the_cues_are_in_the_checksum() {
        let (mut sim, swinging, _) = world();
        let event = crate::cues::CueEvent {
            actor: swinging,
            cue: Name::new("x"),
            tick: 0,
            local_time: SimTime::ZERO,
            move_time: SimDuration::ZERO,
            hit: None,
        };
        // world() advances a busy fight and already published cues; this oracle starts from the
        // same empty queue history that each field-specific sample below reconstructs.
        *sim.ecs_mut().world_mut().resource_mut::<CueQueue>() = CueQueue::default();
        let before = sim.checksum();
        sim.ecs_mut().world_mut().resource_mut::<CueQueue>().push(event);
        let published = sim.checksum();
        assert_ne!(published, before);
        // Replace the complete queue for each sample: clear preserves published/dropped history.
        let with_event = |sim: &mut Simulation, event| {
            let mut queue = CueQueue::default();
            queue.push(event);
            *sim.ecs_mut().world_mut().resource_mut::<CueQueue>() = queue;
            sim.checksum()
        };
        assert_eq!(with_event(&mut sim, event), published);
        // Each field of an event, in a queue of one.
        for (what, other) in [
            ("its actor", crate::cues::CueEvent { actor: ActorId::from_raw(99).unwrap(), ..event }),
            ("its name", crate::cues::CueEvent { cue: Name::new("y"), ..event }),
            ("its tick", crate::cues::CueEvent { tick: 1, ..event }),
            ("its local time", crate::cues::CueEvent { local_time: SimTime::from_subticks(1), ..event }),
            ("its move time", crate::cues::CueEvent { move_time: SimDuration::from_subticks(1), ..event }),
        ] {
            assert_ne!(with_event(&mut sim, other), published, "{what}");
        }

        // A hit's cue carries what it landed with, each field of which counts.
        let hit = crate::cues::HitCue {
            target: swinging,
            hurtbox: 0,
            point: Vec3::ZERO,
            normal: Vec3::Z,
            weight: sapphire_moves::Weight::Light,
            damage: 1,
            poise: 1,
            hitstop: 1,
            outcome: HitOutcome::Flinched,
        };
        let with =
            |sim: &mut Simulation, hit| with_event(sim, crate::cues::CueEvent { hit: Some(hit), ..event });
        let plain = with(&mut sim, hit);
        assert_ne!(plain, published);
        assert_eq!(with(&mut sim, hit), plain);
        use crate::resolve::HitOutcome::{Absorbed, Defeated, PoiseBroken};
        for (what, other) in [
            ("its target", crate::cues::HitCue { target: ActorId::from_raw(99).unwrap(), ..hit }),
            ("its hurtbox", crate::cues::HitCue { hurtbox: 1, ..hit }),
            ("its point", crate::cues::HitCue { point: Vec3::X, ..hit }),
            ("its normal", crate::cues::HitCue { normal: Vec3::X, ..hit }),
            ("its weight", crate::cues::HitCue { weight: sapphire_moves::Weight::Heavy, ..hit }),
            ("its damage", crate::cues::HitCue { damage: 2, ..hit }),
            ("its poise", crate::cues::HitCue { poise: 2, ..hit }),
            ("its hitstop", crate::cues::HitCue { hitstop: 2, ..hit }),
            ("absorbed", crate::cues::HitCue { outcome: Absorbed, ..hit }),
            ("broken", crate::cues::HitCue { outcome: PoiseBroken, ..hit }),
            ("defeated", crate::cues::HitCue { outcome: Defeated, ..hit }),
        ] {
            assert_ne!(with(&mut sim, other), plain, "{what}");
        }
    }

    /// The test above changes an actor; these say so for everyone: a second actor's moves count too.
    #[test]
    fn identical_actors_in_different_states_differ() {
        let (sim, _, _) = world();
        let (mut other, _, dodging) = world();
        assert_eq!(sim.checksum(), other.checksum());
        other.despawn(dodging);
        assert_ne!(sim.checksum(), other.checksum());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(write: impl FnOnce(&mut StateHasher)) -> u64 {
        let mut hasher = StateHasher::new();
        write(&mut hasher);
        hasher.finish()
    }

    /// XXH3-64 of these bytes by the algorithm's definition: a change here moves every recorded
    /// checksum. Streaming in pieces gives the same as hashing at once.
    #[test]
    fn the_hash_is_xxh3_of_the_bytes_in_order() {
        let bytes = [1u8, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12];
        let expected = xxhash_rust::xxh3::xxh3_64(&bytes);
        let streamed = hash(|hasher| {
            hasher.u8(1);
            hasher.u16(0x1234);
            hasher.u32(0x1234_5678);
        });
        assert_eq!(streamed, expected);
        assert_eq!(hash(|_| {}), 0x2d06_8005_38d3_94c2, "XXH3-64 of nothing, as published");
    }

    #[test]
    fn floats_go_by_their_bits() {
        let bits = hash(|hasher| hasher.u32(1.5_f32.to_bits()));
        assert_eq!(hash(|hasher| hasher.f32(1.5)), bits);
        assert_ne!(hash(|hasher| hasher.f32(0.0)), hash(|hasher| hasher.f32(-0.0)), "−0 can diverge later");
        let one_nan = hash(|hasher| hasher.f32(f32::NAN));
        let other_nan = f32::from_bits(f32::NAN.to_bits() | 0x8000_0123);
        assert_eq!(hash(|hasher| hasher.f32(other_nan)), one_nan, "a NaN's payload isn't promised");
    }

    #[test]
    fn vectors_go_in_order() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(hash(|hasher| hasher.vec3(v)), hash(|hasher| hasher.f32s(&[1.0, 2.0, 3.0])));
        assert_ne!(hash(|hasher| hasher.vec3(v)), hash(|hasher| hasher.f32s(&[3.0, 2.0, 1.0])));
    }
}
