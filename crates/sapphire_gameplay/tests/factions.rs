//! Several fighters at once (M3 B1) through the public API: who hits whom, a fallen capsule, a soft lock's
//! choice in the step and a brain's own pick of a foe. These run on a flat floor with the warrior's real
//! moves, except where a test wants someone who only stands there.

use std::alloc::System;

use sapphire_core::Name;
use sapphire_core::alloc::CountingAllocator;
use sapphire_gameplay::replay::{self, RIVALS, SCENARIOS, WARRIORS, find_scenario};
use sapphire_gameplay::targeting::{TargetingTuning, TargetingWeights};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, ActorTable, Buttons, ControllerProfile, DEFEATED, Faction, MoveSetId,
    ProfileId, SimConfig, Simulation, Stick, TickActions, WarpTarget, actor_body,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const SIDE_A: Faction = Faction::new(1).unwrap();
const SIDE_B: Faction = Faction::new(2).unwrap();

/// A flat floor and the warrior's moves, for fighters to stand on.
struct Floor {
    sim: Simulation,
    moves: Option<MoveSetId>,
    /// The same moves, on one point of health: a fighter a single hit defeats.
    frail: Option<MoveSetId>,
    controller: Option<ProfileId>,
}

impl Floor {
    fn new(config: SimConfig) -> Self {
        let mut sim = Simulation::new(config);
        let floor = StaticCollider {
            id: ColliderId(0),
            shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 0.5, 200.0) }),
            pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
            memberships: Group::World.mask(),
        };
        let world = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("builds");
        sim.set_physics_world(world).expect("room");
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("good"));
        let moves = Some(replay::add_warrior(&mut sim).expect("the warrior compiles"));
        let frail = Some(sim.add_move_set(frail_warrior()));
        Self { sim, moves, frail, controller }
    }

    fn fighter(&mut self, x: f32, z: f32, faction: Faction) -> ActorId {
        let spawn = ActorSpawn {
            position: Vec3::new(x, 0.0, z),
            yaw: 0.0,
            moves: self.moves,
            controller: self.controller,
            faction,
        };
        self.sim.spawn(spawn).expect("room")
    }

    fn frail_fighter(&mut self, x: f32, z: f32, faction: Faction) -> ActorId {
        let spawn = ActorSpawn {
            position: Vec3::new(x, 0.0, z),
            yaw: 0.0,
            moves: self.frail,
            controller: self.controller,
            faction,
        };
        self.sim.spawn(spawn).expect("room")
    }

    fn swing(&mut self, actor: ActorId) {
        if self.sim.actor(actor).expect("alive").moves.current.is_none() {
            self.sim.force_move(actor, Name::new("swipe"));
        }
        self.sim.step(&TickActions::NEUTRAL);
    }

    fn lock_of(&self, actor: ActorId) -> Option<ActorId> {
        self.sim.actor(actor).expect("alive").soft_lock.target()
    }

    fn brain_of(&self, actor: ActorId) -> Option<ActorId> {
        self.sim.actor(actor).expect("alive").brain.target()
    }

    fn neutral(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.sim.step(&TickActions::NEUTRAL);
        }
    }

    /// Someone on the other side steps up to this frail fighter and swings until it falls.
    fn fell(&mut self, actor: ActorId) {
        let view = self.sim.actor(actor).expect("alive");
        let other_side = if view.fighter.faction() == SIDE_A { SIDE_B } else { SIDE_A };
        let killer = self.fighter(view.position.x, view.position.z - 1.0, other_side);
        for _ in 0..600 {
            if self.sim.actor(actor).expect("alive").fighter.is_defeated() {
                self.sim.despawn(killer);
                return;
            }
            self.swing(killer);
        }
        panic!("it never fell");
    }
}

fn frail_warrior() -> MoveSet {
    let mut document = MoveSetDocument::parse(replay::WARRIOR_MOVES).expect("parses");
    document.fighter.as_mut().expect("it fights").health = 1;
    document.compile().expect("compiles")
}

fn aim_of(sim: &Simulation, actor: ActorId) -> WarpTarget {
    let world = sim.world();
    let entity = world.resource::<ActorTable>().entity(actor).expect("alive");
    *world.get::<WarpTarget>(entity).expect("every actor has one")
}

fn pushing(actor: ActorId, buttons: Buttons, x: f32, z: f32) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick: Stick::from_axes(x, z) }).expect("room");
    actions
}

#[test]
fn a_swing_lands_on_a_foe_and_passes_over_a_friend() {
    let mut floor = Floor::new(SimConfig::default());
    let swinger = floor.fighter(0.0, 0.0, SIDE_A);
    let friend = floor.fighter(0.0, 1.2, SIDE_A);
    let foe = floor.fighter(0.9, 1.0, SIDE_B);
    let (whole_friend, whole_foe) = (
        floor.sim.actor(friend).expect("alive").fighter.health(),
        floor.sim.actor(foe).expect("alive").fighter.health(),
    );
    for _ in 0..600 {
        floor.swing(swinger);
        for event in floor.sim.hits().events() {
            assert_ne!(event.target, friend, "a friend isn't even a contact");
        }
    }
    assert_eq!(floor.sim.actor(friend).expect("alive").fighter.health(), whole_friend);
    assert!(floor.sim.actor(foe).expect("alive").fighter.health() < whole_foe, "it should have landed");
}

/// No faction is what every fight was: the same swing, the same two standing there, and both are hit.
#[test]
fn no_faction_is_everyone_against_everyone() {
    let mut floor = Floor::new(SimConfig::default());
    let swinger = floor.fighter(0.0, 0.0, Faction::NONE);
    let beside = floor.fighter(0.0, 1.2, Faction::NONE);
    let whole = floor.sim.actor(beside).expect("alive").fighter.health();
    for _ in 0..600 {
        floor.swing(swinger);
    }
    assert!(floor.sim.actor(beside).expect("alive").fighter.health() < whole);
}

/// The fallen stop nobody and are still on the floor: a walker goes through where a capsule would stop them,
/// and one that fell doesn't sink.
#[test]
fn a_defeated_capsule_stops_nobody_and_keeps_its_floor() {
    let mut floor = Floor::new(SimConfig::default());
    let walker = floor.fighter(0.0, 0.0, SIDE_A);
    let standing = floor.frail_fighter(0.0, 3.0, SIDE_B);
    floor.neutral(2);

    // Up: it stops them short.
    let start = floor.sim.actor(walker).expect("alive").position;
    for _ in 0..120 {
        floor.sim.step(&pushing(walker, Buttons::NONE, 0.0, 1.0));
    }
    let stopped = floor.sim.actor(walker).expect("alive").position.z;
    assert!(stopped < 3.0, "a standing capsule is in the way: {stopped}");

    // Down: nothing is.
    let walker_back = |floor: &mut Floor| {
        for _ in 0..120 {
            floor.sim.step(&pushing(walker, Buttons::NONE, 0.0, -1.0));
        }
    };
    walker_back(&mut floor);
    floor.fell(standing);
    assert!(floor.sim.actor(walker).expect("alive").position.z < start.z + 3.0);
    for _ in 0..240 {
        floor.sim.step(&pushing(walker, Buttons::NONE, 0.0, 1.0));
    }
    let through = floor.sim.actor(walker).expect("alive").position.z;
    let corpse = floor.sim.actor(standing).expect("alive").position;
    assert!(through > corpse.z + 1.0, "walked through: {through} past {corpse}");
    assert!(corpse.y.abs() < 1e-3, "it's on its floor: {corpse}");

    // It's a body still, for the scenery's sake, in nobody's group.
    let groups = floor.sim.physics().body_groups(actor_body(standing)).expect("it stays a body");
    assert!(!groups.memberships.intersects(Group::Character.mask()));
    assert!(groups.filter.intersects(Group::World.mask()));
}

/// A replaced set's new capsule stays out of everyone's way, if the old one was.
#[test]
fn a_refit_keeps_the_fallen_out_of_the_way() {
    let mut floor = Floor::new(SimConfig::default());
    let fallen = floor.frail_fighter(0.0, 0.0, SIDE_B);
    floor.neutral(1);
    floor.fell(fallen);
    let mut document = MoveSetDocument::parse(replay::WARRIOR_MOVES).expect("parses");
    let fighter = document.fighter.as_mut().expect("it fights");
    fighter.health = 1;
    fighter.movement.as_mut().expect("it says how it moves").height += 0.2;
    let edited = document.compile().expect("compiles");
    floor.sim.replace_move_set(floor.frail.expect("a set"), edited).expect("loaded");
    let groups = floor.sim.physics().body_groups(actor_body(fallen)).expect("it stays a body");
    assert!(!groups.memberships.intersects(Group::Character.mask()));
}

/// A soft lock's choice is made in the step, on hostile, standing fighters only, with the stick, and it's what
/// the moves read.
#[test]
fn a_soft_lock_chooses_in_the_step() {
    let mut floor = Floor::new(SimConfig::default());
    let player = floor.fighter(0.0, 0.0, SIDE_A);
    let friend = floor.fighter(0.0, 1.5, SIDE_A);
    let ahead = floor.fighter(0.0, 5.0, SIDE_B);
    let side = floor.frail_fighter(5.0, 0.0, SIDE_B);
    assert!(floor.sim.give_soft_lock(player));
    assert_eq!(floor.lock_of(player), None, "its first step chooses");

    floor.sim.step(&TickActions::NEUTRAL);
    assert_eq!(floor.lock_of(player), Some(ahead), "the one it faces, and never its friend");
    assert_ne!(floor.lock_of(player), Some(friend));

    for _ in 0..3 {
        floor.sim.step(&pushing(player, Buttons::NONE, 1.0, 0.0));
    }
    assert_eq!(floor.lock_of(player), Some(side), "the stick swung over");
    assert_eq!(aim_of(&floor.sim, player), WarpTarget::Actor(side), "and that's what a warp reads");

    floor.fell(side);
    floor.sim.step(&TickActions::NEUTRAL);
    assert_eq!(floor.lock_of(player), Some(ahead), "off the fallen, to who's left");
}

#[test]
fn a_soft_lock_with_nobody_in_reach_aims_at_nothing() {
    let mut floor = Floor::new(SimConfig::default());
    let player = floor.fighter(0.0, 0.0, SIDE_A);
    let far = floor.fighter(0.0, 40.0, SIDE_B);
    floor.sim.give_soft_lock(player);
    floor.sim.set_warp_target(player, WarpTarget::Actor(far));
    floor.sim.step(&TickActions::NEUTRAL);
    assert_eq!(floor.lock_of(player), None);
    assert_eq!(aim_of(&floor.sim, player), WarpTarget::None);
}

/// The weights are the simulation's and a tick says which set: one number moved in the tuning changes who it
/// picks, and the same scene with a thumb's bit picks by the other set, on the same simulation.
#[test]
fn a_soft_lock_weighs_by_the_simulations_weights_and_the_ticks_own_device() {
    let scene = |tuning: TargetingTuning, buttons: Buttons| {
        let mut floor = Floor::new(SimConfig::default());
        floor.sim.tune_targeting(tuning);
        let player = floor.fighter(0.0, 0.0, SIDE_A);
        let far_ahead = floor.fighter(0.0, 9.0, SIDE_B);
        let near_behind = floor.fighter(0.0, -3.0, SIDE_B);
        floor.sim.give_soft_lock(player);
        floor.sim.step(&pushing(player, buttons, 0.0, 1.0));
        (floor.lock_of(player), far_ahead, near_behind, floor.sim.checksum())
    };
    let shipped = TargetingTuning::shipped();
    let (pad, far_ahead, _, pad_checksum) = scene(shipped, Buttons::NONE);
    assert_eq!(pad, Some(far_ahead), "a pad's stick says who");
    let (thumb, _, near_behind, thumb_checksum) = scene(shipped, Buttons::TOUCH);
    assert_eq!(thumb, Some(near_behind), "a thumb's nearest");
    assert_ne!(pad_checksum, thumb_checksum);

    let closeness_only = TargetingWeights { direction: 0.0, facing: 0.0, ..shipped.soft_lock };
    let (other, _, near_behind, _) =
        scene(TargetingTuning { soft_lock: closeness_only, ..shipped }, Buttons::NONE);
    assert_eq!(other, Some(near_behind), "the tuning's weights are the ones it used");
}

/// A move bends toward one target from start to end: the lock can change its mind with the stick, and the
/// aim follows only once the window's closed.
#[test]
fn the_aim_holds_while_a_warp_is_open() {
    let scenario = find_scenario(&SCENARIOS, "arena-skirmish").expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
    let player = cast[0];
    let (mut disagreed, mut followed) = (false, false);
    let (mut held, mut was_open) = (None, false);
    for tick in 0..120 {
        let buttons = if tick == 1 { Buttons::LIGHT } else { Buttons::NONE };
        // The player spawns looking toward -Z, with a rival off to +X for the stick to swing to, once it's
        // too late to change what this move bends toward.
        let stick = if held.is_some() { 1.0 } else { 0.0 };
        sim.step(&pushing(player, buttons, stick, 0.0));
        let view = sim.actor(player).expect("alive");
        let aim = aim_of(&sim, player);
        let lock = view.soft_lock.target().map_or(WarpTarget::None, WarpTarget::Actor);
        if view.moves.windows.warp {
            let held = *held.get_or_insert(aim);
            assert_eq!(aim, held, "tick {tick}");
            disagreed |= lock != held;
        } else if disagreed && was_open {
            // The step that closed it read the window before the moves ran.
            assert_eq!(aim, held.expect("it opened"), "tick {tick}");
        } else if disagreed {
            assert_eq!(aim, lock, "tick {tick}");
            followed = true;
        }
        was_open = view.moves.windows.warp;
    }
    assert!(disagreed, "the lock changes its mind while the window's open");
    assert!(followed, "and the aim catches up when it closes");
}

#[test]
fn a_brain_picks_the_nearest_foe_and_stays_on_it() {
    let mut floor = Floor::new(SimConfig::default());
    let brain = floor.fighter(0.0, 0.0, SIDE_A);
    let friend = floor.fighter(1.0, 0.0, SIDE_A);
    let near = floor.frail_fighter(0.0, 4.0, SIDE_B);
    let far = floor.fighter(0.0, -6.0, SIDE_B);
    assert!(floor.sim.give_brain(brain));
    assert_eq!(floor.brain_of(brain), None, "it looks as it thinks");
    floor.sim.step(&TickActions::NEUTRAL);
    assert_eq!(floor.brain_of(brain), Some(near), "the nearest foe, and past its friend");
    assert_ne!(floor.brain_of(brain), Some(friend));
    floor.fell(near);
    floor.neutral(1);
    assert_eq!(floor.brain_of(brain), Some(far), "the next, once that one's gone");
}

/// With nobody left it stands over who it beat, as a brain with one foe always did.
#[test]
fn a_brain_with_nobody_left_keeps_its_last_foe_and_stands() {
    let mut floor = Floor::new(SimConfig::default());
    let brain = floor.fighter(0.0, 0.0, SIDE_A);
    let foe = floor.frail_fighter(0.0, 4.0, SIDE_B);
    floor.sim.give_brain(brain);
    floor.neutral(1);
    floor.fell(foe);
    floor.neutral(2);
    assert_eq!(floor.brain_of(brain), Some(foe));
    assert_eq!(floor.sim.actor(brain).expect("alive").brain.stick(), Stick::CENTER);
}

/// A brain with no foe at all, from the start, thinks and asks nothing.
#[test]
fn a_brain_with_no_one_there_just_stands() {
    let mut floor = Floor::new(SimConfig::default());
    let brain = floor.fighter(0.0, 0.0, SIDE_A);
    floor.fighter(1.0, 0.0, SIDE_A);
    floor.sim.give_brain(brain);
    floor.neutral(300);
    let view = floor.sim.actor(brain).expect("alive");
    assert_eq!(view.brain.target(), None);
    assert!(view.moves.current.is_none(), "nothing to swing at");
}

/// Two sides of warriors and a player, all brains on: the sides fight each other, nobody ever touches their
/// own, and a step allocates nothing however many are fighting.
#[test]
fn the_skirmish_is_two_sides_and_a_player() {
    let scenario = find_scenario(&SCENARIOS, "arena-skirmish").expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
    assert_eq!(cast.len(), 8);
    let faction = |sim: &Simulation, actor: ActorId| sim.actor(actor).expect("alive").fighter.faction();
    assert_eq!(cast[1..5].iter().filter(|&&actor| faction(&sim, actor) == WARRIORS).count(), 4);
    assert_eq!(cast[5..].iter().filter(|&&actor| faction(&sim, actor) == RIVALS).count(), 3);

    let (mut across, mut defeats) = (0, 0);
    for _ in 0..4000 {
        sim.step(&TickActions::NEUTRAL);
        for cue in sim.cues() {
            let Some(hit) = cue.hit else { continue };
            assert_ne!(faction(&sim, cue.actor), faction(&sim, hit.target), "{cue:?}");
            let sides = [faction(&sim, cue.actor), faction(&sim, hit.target)];
            across += usize::from(sides.contains(&WARRIORS) && sides.contains(&RIVALS));
            defeats += usize::from(cue.cue == DEFEATED);
        }
    }
    assert!(across > 0, "the two sides should meet");
    assert!(defeats > 0, "someone should fall");
    assert_eq!(sim.stats().allocations, 0, "a step allocates nothing");
    for actor in sim.actors() {
        assert!(actor.position.y.abs() < 2.0, "{actor:?}");
    }
}
