//! M2 G7: the party, through real steps. See the `party` module's docs for what it promises. Most of it runs
//! on members who differ ([`replay::party_document`]), with ticks worked out by hand in each test's comments
//! from `party.moves.ron`'s and `slice.moves.ron`'s numbers. The last few run the `party` scenario, on the
//! player's document against the warrior's brain. World space is metres, Y up, with a root's yaw turning
//! its +Z toward +X.

use std::f32::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::replay::{
    self, LiveMoves, Player, Replay, SCENARIOS, ScenarioError, compile_document, find_scenario,
};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, ActorView, Buttons, CharacterWorld, CombatSet, ControllerProfile,
    DEFEATED, HitOutcome, HurtboxId, PARTY_DEFEATED, PARTY_SWAP, PERFECT_DODGE, Party, PartyError,
    PartyTuning, ProfileId, SWAP_IN_MOVE, SWAP_INTENT, SimConfig, Simulation, Stick, TickActions, Tuned,
    WarpTarget, actor_body,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_physics::{
    ColliderId, ColliderSet, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape,
};
use sapphire_time::{ScaleSource, SimDuration};

const TUNING: PartyTuning = PartyTuning { swap_cooldown: 60, rest: 150 };

/// A party of three at the origin facing +Z, and an opponent `apart` metres ahead, facing it, each with a
/// capsule on a floor. Nobody has a target.
struct Fight {
    sim: Simulation,
    members: [ActorId; 3],
    opponent: ActorId,
}

fn floor(sim: &mut Simulation) -> ProfileId {
    let floor = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(100.0, 0.5, 100.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    };
    let world = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("builds");
    sim.set_physics_world(world).expect("nobody's in it yet");
    sim.add_controller_profile(&ControllerProfile::default()).expect("a good profile")
}

fn fight(apart: f32, tuning: PartyTuning) -> Fight {
    fight_of([replay::party_moves(0), replay::party_moves(1), replay::party_moves(2)], apart, tuning)
}

fn fight_of(
    sets: [Result<MoveSet, sapphire_moves::MoveSetError>; 3],
    apart: f32,
    tuning: PartyTuning,
) -> Fight {
    let mut sim = Simulation::new(SimConfig { max_actors: 8, ..SimConfig::default() });
    let controller = Some(floor(&mut sim));
    let spawned: Vec<ActorId> = sets
        .into_iter()
        .map(|set| {
            let moves = Some(sim.add_move_set(set.expect("compiles")));
            sim.spawn(ActorSpawn {
                position: Vec3::ZERO,
                yaw: 0.0,
                moves,
                controller,
                ..ActorSpawn::default()
            })
            .expect("room")
        })
        .collect();
    let members = [spawned[0], spawned[1], spawned[2]];
    let moves = Some(sim.add_move_set(replay::test_moves().expect("compiles")));
    let position = Vec3::new(0.0, 0.0, apart);
    let opponent = sim
        .spawn(ActorSpawn { position, yaw: PI, moves, controller, ..ActorSpawn::default() })
        .expect("room");
    sim.form_party(&members, tuning).expect("a good party");
    // Everyone lands.
    sim.step(&TickActions::NEUTRAL);
    Fight { sim, members, opponent }
}

impl Fight {
    /// A step with the party pressing these.
    fn press(&mut self, buttons: Buttons) {
        let party = self.members[0];
        self.buttons(party, buttons);
    }

    fn buttons(&mut self, actor: ActorId, buttons: Buttons) {
        let mut actions = TickActions::NEUTRAL;
        actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).expect("room");
        self.sim.step(&actions);
    }

    fn steps(&mut self, count: usize) {
        for _ in 0..count {
            self.sim.step(&TickActions::NEUTRAL);
        }
    }

    fn view(&self, actor: ActorId) -> ActorView {
        self.sim.actor(actor).expect("alive")
    }

    fn active(&self) -> ActorId {
        self.sim.party().active().expect("formed")
    }

    fn move_name(&self, actor: ActorId) -> Option<Name> {
        self.view(actor).moves.current.map(|current| current.name)
    }

    /// Whether a body with this ID is in the world.
    fn has_body(&self, body: sapphire_physics::BodyId) -> bool {
        self.sim.physics().body_shape(body).is_some()
    }

    /// How many of this actor's bodies are in the world: its capsule, its hurtboxes, any dodge volume.
    fn bodies_of(&self, actor: ActorId) -> usize {
        let capsule = usize::from(self.has_body(actor_body(actor)));
        let (first, last) = HurtboxId::bodies_of(actor).expect("room");
        let extra = (first.0..=last.0).filter(|&id| self.has_body(sapphire_physics::BodyId(id))).count();
        capsule + extra
    }

    /// How many presses `actor` has waiting.
    fn presses_in_ring(&self, actor: ActorId) -> usize {
        self.sim.intents(actor).expect("alive").len()
    }

    fn cues(&self) -> Vec<(ActorId, Name)> {
        self.sim.cues().map(|cue| (cue.actor, cue.cue)).collect()
    }
}

const LIGHT_1: Name = Name::new("light_1");
const SWAP_IN_CUE: Name = Name::new("swap_in");

// Forming one.

#[test]
fn a_party_takes_everyone_but_its_first_member_off_the_field() {
    let f = fight(4.0, TUNING);
    let [a, b, c] = f.members;
    assert_eq!(f.sim.party().active(), Some(a));
    assert_eq!(f.sim.party().active_slot(), Some(0));
    assert_eq!(f.sim.party().input_actor(), Some(a));
    assert_eq!(f.sim.party().slots().map(|slot| slot.actor).collect::<Vec<_>>(), [a, b, c]);
    assert!(!f.view(a).off_field);
    assert!(f.view(b).off_field && f.view(c).off_field);
    assert!(!f.view(f.opponent).off_field, "someone outside it isn't either");
    // A capsule and a hurtbox for each on the field, nothing for the others.
    assert_eq!(f.bodies_of(a), 2);
    assert_eq!(f.bodies_of(f.opponent), 2);
    assert_eq!((f.bodies_of(b), f.bodies_of(c)), (0, 0));
}

#[test]
fn a_party_refuses_what_it_cant_be() {
    let mut sim = Simulation::new(SimConfig::default());
    let controller = Some(floor(&mut sim));
    let member = Some(sim.add_move_set(replay::party_moves(0).expect("compiles")));
    let plain = Some(sim.add_move_set(replay::test_moves().expect("compiles")));
    let mut no_assist = replay::party_document(0).expect("reads");
    no_assist.moves.retain(|mv| mv.name != "swap_in");
    let no_assist = Some(sim.add_move_set(no_assist.compile().expect("compiles")));
    let mut spawn =
        |moves| sim.spawn(ActorSpawn { moves, controller, ..ActorSpawn::default() }).expect("room");
    let (a, b, c, d) = (spawn(member), spawn(member), spawn(member), spawn(member));
    let (setless, swapless, assistless) = (spawn(None), spawn(plain), spawn(no_assist));
    let gone = spawn(member);
    assert!(sim.despawn(gone));

    let mut refused = |members: &[ActorId], error| {
        assert_eq!(sim.form_party(members, TUNING), Err(error), "{members:?}");
    };
    refused(&[], PartyError::BadSize { count: 0 });
    refused(&[a, b, c, d], PartyError::BadSize { count: 4 });
    refused(&[a, b, a], PartyError::Twice { actor: a });
    refused(&[a, gone], PartyError::NotAlive { actor: gone });
    refused(&[a, setless], PartyError::NoSwapIntent { actor: setless });
    refused(&[a, swapless], PartyError::NoSwapIntent { actor: swapless });
    refused(&[a, assistless], PartyError::NoSwapInMove { actor: assistless });
    assert!(!sim.party().is_formed(), "and nothing of those stuck");

    // Fewer than three is a party, with slots to spare.
    assert_eq!(sim.form_party(&[a, b], TUNING), Ok(()));
    assert_eq!(sim.party().slots().count(), 2);
    assert_eq!(sim.form_party(&[c], TUNING), Err(PartyError::AlreadyFormed));
}

// A swap.

/// The press's own step, from nothing: `swap_in` at its tick 0, `b` where `a` stood and as it faced, and
/// everything of `a` gone from the world before anything can read it.
#[test]
fn a_press_swaps_on_its_own_step() {
    let mut f = fight(4.0, TUNING);
    let [a, b, _] = f.members;
    let place = Vec3::new(1.5, 0.0, -2.0);
    // Walk `a` away from where the others last stood, so taking its place is a move, and turn it.
    let mut walk = TickActions::NEUTRAL;
    walk.set(a, ActorActions { buttons: Buttons::NONE, stick: Stick::new(90, -90) }).expect("room");
    while f.view(a).position.distance(place) > 1.0 {
        f.sim.step(&walk);
    }
    f.steps(20);
    let (stood, yaw) = (f.view(a).position, f.view(a).yaw);
    assert!(yaw != 0.0, "it turned as it walked");

    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    assert_eq!(f.sim.party().active_slot(), Some(1));
    let incoming = f.view(b);
    assert_eq!((incoming.position, incoming.yaw), (stood, yaw));
    assert!(!incoming.off_field && f.view(a).off_field);
    let current = incoming.moves.current.expect("its assist");
    assert_eq!((current.name, current.tick()), (SWAP_IN_MOVE, 0));
    assert_eq!(current.elapsed, SimDuration::ZERO);
    assert_eq!(f.bodies_of(a), 0);
    assert_eq!(f.bodies_of(b), 2, "a capsule and a hurtbox");
    let capsule = f.sim.physics().body_pose(actor_body(b)).expect("in the world");
    assert_eq!((capsule.position.x, capsule.position.z), (stood.x, stood.z));
    assert!(capsule.position.y > stood.y, "its centre, over its feet");

    // What it publishes, its swap first.
    let cues = f.cues();
    assert_eq!(cues, [(b, PARTY_SWAP), (b, SWAP_IN_CUE)]);
    assert_eq!(f.sim.stats().swaps, 1);
    assert_eq!(f.sim.stats().forced_swaps, 0);
    // It stands on the ground as `a` did, with no landing to show for it.
    assert!(!incoming.controller.state.events.landed);
    assert_eq!(incoming.controller.state.mode, sapphire_gameplay::GroundMode::Grounded);
}

/// `CombatSet::Party` comes after every press and before anything that reads a ring or a position, whatever
/// else the sets hold: this watches from inside each.
#[test]
fn a_swap_comes_between_intents_and_cancels() {
    let mut f = fight(4.0, TUNING);
    let [a, b, _] = f.members;
    let seen: Arc<[AtomicU32; 4]> = Arc::new(std::array::from_fn(|_| AtomicU32::new(0)));
    let watch = |slot: usize| {
        let seen = Arc::clone(&seen);
        move |party: Res<'_, Party>| {
            seen[slot].store(party.active().map_or(0, ActorId::raw), Ordering::Relaxed);
        }
    };
    f.sim.add_combat_system(CombatSet::Intents, watch(0));
    f.sim.add_combat_system(CombatSet::Cancels, watch(1));
    let (body_seen, also_body) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
    let (seen_in_movement, seen_in_hits) = (Arc::clone(&body_seen), Arc::clone(&also_body));
    f.sim.add_combat_system(CombatSet::Movement, move |world: Res<'_, CharacterWorld>| {
        let on_field = u32::from(world.physics().body_shape(actor_body(b)).is_some());
        seen_in_movement.store(on_field + 1, Ordering::Relaxed);
    });
    f.sim.add_combat_system(CombatSet::Hits, move |world: Res<'_, CharacterWorld>| {
        let on_field = u32::from(world.physics().body_shape(actor_body(b)).is_some());
        seen_in_hits.store(on_field + 1, Ordering::Relaxed);
    });
    f.press(Buttons::SWAP);
    assert_eq!(seen[0].load(Ordering::Relaxed), a.raw(), "Intents is before it");
    assert_eq!(seen[1].load(Ordering::Relaxed), b.raw(), "Cancels is after it");
    assert_eq!(body_seen.load(Ordering::Relaxed), 2, "so is Movement");
    assert_eq!(also_body.load(Ordering::Relaxed), 2, "and so is Hits");
}

/// A script or a player's bindings name the party by its first member, whoever's on the field.
#[test]
fn input_reaches_whoever_is_on_the_field() {
    let mut f = fight(8.0, TUNING);
    let [a, b, c] = f.members;
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    f.steps(40); // The assist plays out.
    assert_eq!(f.move_name(b), None);
    let z = f.view(b).position.z;
    let mut walk = TickActions::NEUTRAL;
    walk.set(a, ActorActions { buttons: Buttons::NONE, stick: Stick::new(0, 127) }).expect("room");
    f.sim.step(&walk);
    assert!(f.view(b).position.z > z, "the first member's ID moved the one on the field");
    assert_eq!(f.view(a).controller.state.velocity, Vec3::ZERO);
    assert_eq!(f.view(c).position, Vec3::ZERO, "and no one else");

    // A tick's entry for anyone else in the party reaches nobody: there's one player.
    f.steps(30);
    let z = f.view(b).position.z;
    let mut other = TickActions::NEUTRAL;
    other.set(b, ActorActions { buttons: Buttons::LIGHT, stick: Stick::new(0, 127) }).expect("room");
    other.set(c, ActorActions { buttons: Buttons::LIGHT, stick: Stick::new(0, 127) }).expect("room");
    f.sim.step(&other);
    assert_eq!(f.move_name(b), None);
    assert_eq!(f.view(b).position.z, z);
    assert_eq!(f.presses_in_ring(c), 0);
}

/// One press is one swap, and a button held across it isn't a press for who came in.
#[test]
fn a_held_button_isnt_a_press_for_whoever_comes_in() {
    let mut f = fight(8.0, TUNING);
    let [_, b, _] = f.members;
    // Holding a light through the swap, and the swap button itself for a while after it.
    f.press(Buttons::LIGHT);
    f.press(Buttons::LIGHT | Buttons::SWAP);
    assert_eq!(f.active(), b);
    for _ in 0..30 {
        f.press(Buttons::LIGHT | Buttons::SWAP);
    }
    assert_eq!(f.active(), b, "the swap button's still only one press");
    assert_eq!(f.sim.party().swaps(), 1);
    assert_eq!(f.presses_in_ring(b), 0, "and the light has gone in as no press");
    // Let go, and a press inside the assist's cancel window is one.
    f.press(Buttons::NONE);
    f.press(Buttons::LIGHT);
    assert_eq!(f.move_name(b), Some(LIGHT_1), "the assist's window took it");
}

// When it can't.

/// Time here is the owner's: a press on a frozen clock waits as any press does, and goes on the first step
/// that moves.
#[test]
fn a_press_on_a_frozen_clock_waits_for_it_to_thaw() {
    let mut f = fight(4.0, TUNING);
    let [a, b, _] = f.members;
    f.sim.clock_mut(a).expect("alive").0.apply_hitstop(5);
    f.press(Buttons::SWAP);
    assert_eq!(f.view(a).clock_delta, SimDuration::ZERO);
    for _ in 0..4 {
        assert_eq!(f.active(), a);
        assert_eq!(f.presses_in_ring(a), 1, "it's there, waiting");
        f.steps(1);
    }
    assert_eq!(f.view(a).clock_delta, SimDuration::ZERO, "the freeze's last step");
    assert_eq!(f.active(), a);
    f.steps(1);
    assert_eq!(f.active(), b, "it went on the first step that moved");
    assert_eq!(f.presses_in_ring(a), 0);
}

/// The cooldown is whoever came in's, on its own clock: a freeze on it holds the cooldown back.
#[test]
fn the_cooldown_runs_on_the_owners_clock() {
    let mut f = fight(4.0, TUNING);
    let [_, b, c] = f.members;
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    let slot = f.sim.party().slots().nth(1).expect("its slot");
    let whole = SimDuration::from_ticks(60);
    assert_eq!(f.sim.party_ready_in(slot), Some(whole));

    // Half way, then frozen for 20 steps, in which it doesn't run down.
    f.steps(30);
    assert_eq!(f.sim.party_ready_in(slot), Some(SimDuration::from_ticks(30)));
    f.sim.clock_mut(b).expect("alive").0.apply_hitstop(20);
    f.steps(20);
    assert_eq!(f.sim.party_ready_in(slot), Some(SimDuration::from_ticks(30)), "its clock stood still");
    f.steps(29);
    assert_eq!(f.sim.party_ready_in(slot), Some(SimDuration::from_ticks(1)));
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), c, "the press made on its last tick is buffered for its first ready one");
    assert_eq!(f.sim.party().swaps(), 2);
}

/// A press while it can't fire waits for its buffer, 12 ticks, and no longer.
#[test]
fn a_press_during_the_cooldown_lasts_its_buffer() {
    let mut f = fight(4.0, TUNING);
    let [_, b, c] = f.members;
    f.press(Buttons::SWAP); // `b` can leave 60 ticks on.
    assert_eq!(f.active(), b);
    f.steps(20);
    let expired = f.sim.stats().intents_expired;
    f.press(Buttons::SWAP); // 39 ticks early.
    assert_eq!(f.presses_in_ring(b), 1);
    f.steps(11);
    assert_eq!(f.presses_in_ring(b), 1, "on its last tick");
    assert_eq!(f.active(), b);
    f.steps(1);
    assert_eq!(f.presses_in_ring(b), 0, "and gone");
    assert_eq!(f.sim.stats().intents_expired, expired + 1);
    f.steps(25);
    // 58 ticks along. A press on the 59th fires a tick later, when the cooldown's up.
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    f.steps(1);
    assert_eq!(f.active(), c);
    assert_eq!(f.move_name(c), Some(SWAP_IN_MOVE));
}

/// The buffer's ends, where the cooldown ends with them: a press 12 ticks old has run out, as for any
/// other intent, and one tick younger fires on its last.
#[test]
fn a_press_that_runs_out_as_the_cooldown_ends_doesnt_swap() {
    let tuning = PartyTuning { swap_cooldown: 30, rest: 0 };
    let mut f = fight(8.0, tuning);
    let [_, b, _] = f.members;
    f.press(Buttons::SWAP); // `b` can leave 30 ticks on.
    f.steps(17);
    f.press(Buttons::SWAP); // 12 ticks before it can.
    f.steps(11);
    assert_eq!((f.active(), f.presses_in_ring(b)), (b, 1), "on its last tick, with the cooldown running");
    f.steps(1);
    assert_eq!((f.active(), f.presses_in_ring(b)), (b, 0), "the cooldown's over, and so is the press");

    let mut f = fight(8.0, tuning);
    let [_, b, c] = f.members;
    f.press(Buttons::SWAP);
    f.steps(18);
    f.press(Buttons::SWAP); // 11 ticks before it can.
    f.steps(10);
    assert_eq!(f.active(), b);
    f.steps(1);
    assert_eq!(f.active(), c, "on the press's last tick");
}

/// A press goes to the next member in slot order who's rested, round the party, and waits if nobody is.
#[test]
fn a_press_goes_to_the_next_rested_member() {
    let tuning = PartyTuning { swap_cooldown: 10, rest: 25 };
    let mut f = fight(8.0, tuning);
    let [a, b, c] = f.members;
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b, "the next slot");
    f.steps(10);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), c, "and the next");
    f.steps(10);
    // `c` can leave, but `a` has 3 ticks of rest to go and `b` 14: nobody's ready, so the press waits.
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), c);
    f.steps(2);
    assert_eq!(f.active(), c);
    f.steps(1);
    assert_eq!(f.active(), a, "`a`'s rest ran out with the press still good");

    // `a` has to stay 10. `b` left 25 ticks ago by then and is next, so it goes round in order.
    f.steps(10);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    // Now `c`'s next, and it left 11 ticks ago, so a press goes past it to the one who's rested: `a` isn't.
    f.steps(10);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b, "both have rest to go");
    f.steps(3);
    assert_eq!(f.active(), c, "the next to be ready");
    assert_eq!(f.sim.party().swaps(), 5);
    assert_eq!(f.sim.party().forced_swaps(), 0);
}

// The assist is a move.

/// `b`'s lunge warps to whoever `a` aimed at, and its volume lands, through everything a move's volume goes
/// through: damage, a cue, and hitstop on both.
#[test]
fn an_assist_warps_and_lands_as_a_move_does() {
    let mut f = fight(3.5, TUNING);
    let [a, b, _] = f.members;
    let opponent = f.opponent;
    f.sim.set_warp_target(a, WarpTarget::Actor(opponent));
    let health = f.view(opponent).fighter.health();
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    // Its blade opens on tick 6 and its warp closes on tick 10, by which it has crossed the gap.
    let mut landed = None;
    for _ in 0..12 {
        f.steps(1);
        if f.view(opponent).fighter.health() < health {
            landed = Some(f.view(b).moves.current.expect("still its assist").tick());
            break;
        }
    }
    let tick = landed.expect("it lands");
    assert!((6..=10).contains(&tick), "on tick {tick}");
    assert_eq!(f.view(opponent).fighter.health(), health - 16);
    assert!(f.sim.cues().any(|cue| cue.actor == b && cue.hit.is_some()), "with its cue");
    assert!(f.view(b).hitstop_ticks > 0 && f.view(opponent).hitstop_ticks > 0);
    assert_eq!(f.sim.stats().hits_applied, 1);
}

/// `a`'s own assist is untouchable until its blow: hurtboxes leave the world for its i-frames. The third
/// member's guard is armoured: a light takes health and no reaction.
#[test]
fn an_assists_i_frames_and_armour_work() {
    // `a` in, from `c`: round the party with a short rest.
    let tuning = PartyTuning { swap_cooldown: 0, rest: 0 };
    let mut f = fight(1.2, tuning);
    let [a, b, c] = f.members;
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    f.press(Buttons::NONE);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), c);
    f.press(Buttons::NONE);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), a);
    assert_eq!(f.bodies_of(a), 1, "a capsule, and its hurtbox out for the assist's i-frames");
    f.steps(16); // Past tick 8, with the freeze its blow lands.
    assert_eq!(f.bodies_of(a), 2, "and back");

    // `c`'s guard: the opponent's `light_1`, whose blade opens on its tick 5, into its armour on ticks 0–15.
    let mut f = fight(1.2, tuning);
    let [_, b, c] = f.members;
    let opponent = f.opponent;
    f.buttons(opponent, Buttons::LIGHT);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    f.press(Buttons::NONE);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), c);
    let (health, serial) = (f.view(c).fighter.health(), f.view(c).moves.current.expect("its assist").serial);
    let mut outcome = None;
    for _ in 0..10 {
        f.steps(1);
        outcome = outcome.or_else(|| f.sim.cues().find_map(|cue| cue.hit).map(|hit| hit.outcome));
    }
    assert_eq!(outcome, Some(HitOutcome::Absorbed));
    assert!(f.view(c).fighter.health() < health, "it took the damage");
    assert_eq!(f.view(c).moves.current.expect("its assist").serial, serial, "and played on");
}

/// An assist's cancel windows take presses as any move's do.
#[test]
fn an_assist_cancels_as_a_move_does() {
    let mut f = fight(8.0, TUNING);
    let [_, b, _] = f.members;
    f.press(Buttons::SWAP);
    // `b`'s lunge has a cancel window on ticks 12–29. A press buffered on tick 6 fires as it opens.
    f.steps(5);
    f.press(Buttons::LIGHT);
    assert_eq!(f.move_name(b), Some(SWAP_IN_MOVE));
    f.steps(5);
    assert_eq!(f.move_name(b), Some(SWAP_IN_MOVE));
    f.steps(1);
    assert_eq!(f.move_name(b), Some(LIGHT_1), "at tick 12");
    assert_eq!(f.sim.stats().cancels_fired, 1);
}

// Off the field.

/// Off the field a clock runs on, and what's on it: poise refills, a rest runs down, a freeze on the one on
/// the field doesn't touch it.
#[test]
fn off_the_field_clocks_keep_ticking() {
    let mut f = fight(1.2, TUNING);
    let [a, b, _] = f.members;
    let opponent = f.opponent;
    // `a` takes a light in its poise. The refill comes 240 ticks after it, on its clock.
    f.buttons(opponent, Buttons::LIGHT);
    f.steps(6);
    let poise = f.view(a).fighter;
    assert!(poise.poise() < poise.max_poise(), "{poise:?}");
    f.steps(20);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);

    let slot = f.sim.party().slots().next().expect("its slot");
    let rest = f.sim.party_ready_in(slot).expect("alive");
    assert_eq!(rest, SimDuration::from_ticks(150));
    let at = f.view(a).clock_now;
    f.steps(100);
    assert_eq!(f.view(a).clock_now.since(at), SimDuration::from_ticks(100));
    assert_eq!(f.sim.party_ready_in(slot), Some(SimDuration::from_ticks(50)));

    // Frozen on the field, its clock stops and `a`'s doesn't.
    f.sim.clock_mut(b).expect("alive").0.apply_hitstop(10);
    let (at, b_at) = (f.view(a).clock_now, f.view(b).clock_now);
    f.steps(10);
    assert_eq!(f.view(a).clock_now.since(at), SimDuration::from_ticks(10));
    assert_eq!(f.view(b).clock_now, b_at);

    f.steps(250);
    let healed = f.view(a).fighter;
    assert_eq!(healed.poise(), healed.max_poise(), "its poise refilled while it was away");
}

/// A finish freeze stops the fight. An off-field member isn't in it.
#[test]
fn a_finish_freeze_passes_the_off_field_by() {
    let mut set = replay::party_document(0).expect("reads");
    set.fighter.as_mut().expect("a fighter").health = 1;
    let mut f = fight_of([set.compile(), replay::party_moves(1), replay::party_moves(2)], 1.2, TUNING);
    let [a, b, _] = f.members;
    let opponent = f.opponent;
    // `light_3`'s finish freeze is 16, but a `heavy` has 24.
    f.buttons(opponent, Buttons::HEAVY);
    let mut frozen = false;
    for _ in 0..30 {
        f.steps(1);
        if f.view(a).fighter.is_defeated() {
            frozen = true;
            break;
        }
    }
    assert!(frozen, "the heavy landed");
    assert_eq!(f.view(opponent).hitstop_ticks, 24);
    assert_eq!(f.view(a).hitstop_ticks, 24);
    assert_eq!(f.view(b).hitstop_ticks, 0);
    let at = f.view(b).clock_now;
    f.steps(5);
    assert_eq!(f.view(b).clock_now.since(at), SimDuration::from_ticks(5));
}

/// Off the field there's nothing to hit and nothing in the way: a swing through where a member stood lands
/// on whoever's there now, a run through where it stood isn't stopped, and a cast finds nothing of it.
#[test]
fn off_the_field_nothing_hits_it_and_it_blocks_nothing() {
    // `b` and `c` spawned at the origin and left at once, so anyone's free to run through there.
    let mut f = fight(4.0, TUNING);
    let [a, b, c] = f.members;
    let opponent = f.opponent;
    let mut run = TickActions::NEUTRAL;
    run.set(opponent, ActorActions { buttons: Buttons::NONE, stick: Stick::new(0, -127) }).expect("room");
    // Walk `a` out of the way first.
    let mut aside = run;
    aside.set(a, ActorActions { buttons: Buttons::NONE, stick: Stick::new(127, 0) }).expect("room");
    for _ in 0..20 {
        f.sim.step(&aside);
    }
    for _ in 0..80 {
        f.sim.step(&run);
    }
    assert!(f.view(opponent).position.z < -1.0, "it ran through: {}", f.view(opponent).position);
    assert_eq!(f.view(b).position, Vec3::ZERO);
    assert_eq!((f.bodies_of(b), f.bodies_of(c)), (0, 0));

    // A swing aimed at `b`'s old place finds nobody. Back to the origin, and swing there.
    let mut back = TickActions::NEUTRAL;
    back.set(opponent, ActorActions { buttons: Buttons::NONE, stick: Stick::new(0, 127) }).expect("room");
    while f.view(opponent).position.z < -1.0 {
        f.sim.step(&back);
    }
    f.steps(20);
    f.buttons(opponent, Buttons::LIGHT);
    let health = (f.view(b).fighter.health(), f.view(c).fighter.health());
    f.steps(20);
    assert_eq!((f.view(b).fighter.health(), f.view(c).fighter.health()), health);
    assert_eq!(f.sim.stats().hits_applied, 0);
}

/// Nothing starts off the field, whatever asks: and when it comes back it starts clean.
#[test]
fn nothing_starts_off_the_field() {
    let mut f = fight(4.0, TUNING);
    let [_, b, _] = f.members;
    assert!(f.sim.force_move(b, LIGHT_1));
    assert!(f.sim.press_intent(b, Name::new("light")));
    f.steps(1);
    assert_eq!(f.move_name(b), None);
    assert_eq!(f.presses_in_ring(b), 0);
    let view = f.view(b);
    assert!(view.moves.volumes.is_empty() && view.moves.root_delta.translation == Vec3::ZERO);
    assert_eq!(f.bodies_of(b), 0);
}

/// Leaving ends what it was doing: its move, what's in its ring, a push and a window to counter from.
#[test]
fn leaving_ends_what_it_was_doing() {
    let mut f = fight(1.2, TUNING);
    let [a, b, _] = f.members;
    let opponent = f.opponent;
    // A push: `a` takes a light, whose knockback lasts 8 ticks after its 3 of hitstop.
    f.buttons(opponent, Buttons::LIGHT);
    while f.view(a).fighter.health() == f.view(a).fighter.max_health() {
        f.steps(1);
    }
    f.steps(3);
    assert!(f.view(a).fighter.is_sliding());
    assert_eq!(f.move_name(a), Some(Name::new("flinch")));
    // With a press buffered that its flinch has no window for.
    f.press(Buttons::LIGHT);
    assert_eq!(f.presses_in_ring(a), 1);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    let gone = f.view(a);
    assert_eq!(gone.moves.current, None);
    assert_eq!(f.presses_in_ring(a), 0);
    assert!(!gone.fighter.is_sliding());
    assert_eq!(gone.moves.counter_closes_at, None);
    assert_eq!(gone.controller.state, sapphire_gameplay::ControllerState::default());
    f.steps(30);
    assert_eq!(f.view(a).position, gone.position, "and it moves no more");
}

/// Everyone aiming at the one who left aims at the one who came, who takes over its aim.
#[test]
fn aim_follows_the_swap() {
    let mut f = fight(4.0, TUNING);
    let [a, b, _] = f.members;
    let opponent = f.opponent;
    f.sim.set_warp_target(a, WarpTarget::Point(Vec3::new(7.0, 0.0, 7.0)));
    f.sim.set_warp_target(opponent, WarpTarget::Actor(a));
    f.press(Buttons::SWAP);
    let target = |f: &Fight, actor: ActorId| {
        let entity = f.sim.world().resource::<sapphire_gameplay::ActorTable>().entity(actor).expect("alive");
        *f.sim.world().get::<WarpTarget>(entity).expect("every actor has one")
    };
    assert_eq!(target(&f, opponent), WarpTarget::Actor(b));
    assert_eq!(target(&f, b), WarpTarget::Point(Vec3::new(7.0, 0.0, 7.0)));
    assert_eq!(target(&f, a), WarpTarget::None);
}

/// A swap out of a perfect dodge's slow motion runs at full speed: what scaled the dodger back up comes
/// along, and the one who left is slowed with the world.
#[test]
fn a_perfect_dodges_scale_back_follows_the_swap() {
    let mut f = fight(1.2, PartyTuning { swap_cooldown: 0, rest: 0 });
    let [a, b, _] = f.members;
    let opponent = f.opponent;
    // The opponent's `light_1` opens on its tick 5, and reaches `a` from about its tick 6. `a`'s dodge's
    // perfect window is on its ticks 1 to 6.
    f.buttons(opponent, Buttons::LIGHT);
    f.steps(2);
    f.press(Buttons::DODGE);
    let mut fired = false;
    for _ in 0..10 {
        f.steps(1);
        fired |= f.sim.cues().any(|cue| cue.cue == PERFECT_DODGE && cue.actor == a);
    }
    assert!(fired, "the dodge is perfect");
    assert!(f.sim.slow_motion().is_running());
    let (a_scales, b_scales) = (f.sim.clock_mut(a).expect("alive").0.scales().count(), {
        f.sim.clock_mut(b).expect("alive").0.scales().count()
    });
    assert_eq!((a_scales, b_scales), (1, 0));

    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    let scaled = |f: &mut Fight, actor| {
        f.sim
            .clock_mut(actor)
            .expect("alive")
            .0
            .scales()
            .any(|scale| scale.source == ScaleSource::PerfectDodge)
    };
    assert!(scaled(&mut f, b) && !scaled(&mut f, a));
    let (a_at, b_at) = (f.view(a).clock_now, f.view(b).clock_now);
    f.steps(5);
    assert_eq!(f.view(b).clock_now.since(b_at), SimDuration::from_ticks(5), "at full speed");
    assert_eq!(f.view(a).clock_now.since(a_at), SimDuration::from_ticks(1), "with the world, at a fifth");
    // And when the slow motion ends, it stops scaling.
    while f.sim.slow_motion().is_running() {
        f.steps(1);
    }
    assert!(!scaled(&mut f, b));
}

// Falling.

/// A party whose first member takes one blow.
fn frail() -> Fight {
    let mut first = replay::party_document(0).expect("reads");
    first.fighter.as_mut().expect("a fighter").health = 1;
    fight_of([first.compile(), replay::party_moves(1), replay::party_moves(2)], 1.2, TUNING)
}

/// Waits for the opponent's blow to defeat `actor`.
fn until_defeated(f: &mut Fight, actor: ActorId) {
    for _ in 0..200 {
        if f.view(actor).fighter.is_defeated() {
            return;
        }
        f.steps(1);
    }
    panic!("it never fell");
}

/// The opponent walks up to `actor` and swings heavies until it falls: a knockdown or a stagger of its own
/// only puts that off.
fn fell(f: &mut Fight, actor: ActorId) {
    let opponent = f.opponent;
    for tick in 0..2000 {
        if f.view(actor).fighter.is_defeated() {
            return;
        }
        let apart = f.view(actor).position.z - f.view(opponent).position.z;
        let forward = if apart.abs() > 1.0 { apart.signum() * 127.0 } else { 0.0 };
        let buttons = if tick % 12 == 0 && apart.abs() <= 1.0 { Buttons::HEAVY } else { Buttons::NONE };
        let mut actions = TickActions::NEUTRAL;
        actions.set(opponent, ActorActions { buttons, stick: Stick::new(0, forward as i8) }).expect("room");
        f.sim.step(&actions);
    }
    panic!("it never fell");
}

/// A defeat forces the next member in, once its fall has played out, and takes no press and no rest to do it.
#[test]
fn a_defeat_forces_a_swap_when_its_fall_has_played_out() {
    let mut f = frail();
    let [a, b, _] = f.members;
    let opponent = f.opponent;
    f.buttons(opponent, Buttons::LIGHT);
    until_defeated(&mut f, a);
    assert!(f.sim.cues().any(|cue| cue.cue == DEFEATED));
    let freeze = f.view(a).hitstop_ticks;
    assert!(freeze > 0, "it holds through its freeze");
    for _ in 0..freeze {
        f.steps(1);
        assert_eq!(f.active(), a);
    }
    // The `defeat` reaction is 120 ticks, and it isn't cut.
    assert_eq!(f.move_name(a), Some(Name::new("defeat")));
    while f.move_name(a) == Some(Name::new("defeat")) {
        assert_eq!(f.active(), a);
        // A press can't hurry it.
        f.press(Buttons::SWAP);
        assert_eq!(f.active(), a);
    }
    // It ended on that last step. The next one swaps.
    f.steps(1);
    assert_eq!(f.active(), b);
    assert_eq!(f.sim.party().forced_swaps(), 1);
    assert_eq!(f.move_name(b), Some(SWAP_IN_MOVE));
    assert!(f.view(a).off_field && f.view(a).fighter.is_defeated());
}

/// A forced swap doesn't wait for anyone's rest. With no slot standing, the party is down.
#[test]
fn a_forced_swap_takes_whoever_is_standing_and_then_the_party_is_down() {
    let tuning = PartyTuning { swap_cooldown: 0, rest: 100_000 };
    let frail_set = |member| {
        let mut set = replay::party_document(member).expect("reads");
        set.fighter.as_mut().expect("a fighter").health = 1;
        set.compile()
    };
    let mut f = fight_of([frail_set(0), frail_set(1), frail_set(2)], 1.2, tuning);
    let [a, b, c] = f.members;
    // `b` is on the field, resting `a` for good.
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    // Felled, it must go to `c`, which hasn't been on: it has no rest.
    fell(&mut f, b);
    while f.active() == b {
        f.steps(1);
    }
    assert_eq!(f.active(), c);
    fell(&mut f, c);
    while f.active() == c {
        f.steps(1);
    }
    assert_eq!(f.active(), a, "its rest has years to go, and it's the only one left");
    assert_eq!(f.sim.party().forced_swaps(), 2);
    assert!(!f.sim.party().is_down());
    fell(&mut f, a);

    let mut down_cues = 0;
    let mut steps = 0;
    while !f.sim.party().is_down() {
        f.steps(1);
        steps += 1;
        down_cues += f.sim.cues().filter(|cue| cue.cue == PARTY_DEFEATED && cue.actor == a).count();
        assert!(steps < 400, "the party never went down");
    }
    assert_eq!(down_cues, 1);
    assert_eq!(f.active(), a, "whoever fell last stays where it fell");
    let swaps = f.sim.party().swaps();
    for _ in 0..20 {
        f.press(Buttons::SWAP | Buttons::LIGHT);
        down_cues += f.sim.cues().filter(|cue| cue.cue == PARTY_DEFEATED).count();
    }
    assert_eq!(down_cues, 1, "once");
    assert_eq!(f.sim.party().swaps(), swaps);
    assert_eq!(f.move_name(a), None, "and it does nothing more");
}

/// A fallen member can't be swapped to: a press goes past its slot.
#[test]
fn a_press_goes_past_a_defeated_slot() {
    let mut f = frail();
    let [a, b, c] = f.members;
    let opponent = f.opponent;
    f.buttons(opponent, Buttons::LIGHT);
    until_defeated(&mut f, a);
    while f.active() == a {
        f.steps(1);
    }
    assert_eq!(f.active(), b);
    f.steps(70);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), c);
    // `b` needs its rest, and `a`, next in order, can't come: it waits for `b`.
    f.steps(61);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), c, "`a`'s the next slot, and it's fallen");
    f.steps(100);
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    assert!(f.view(a).off_field);
}

/// Someone else despawning a member on the field is the same as it falling at once, with no place to take
/// over: the next stands where it last stood.
#[test]
fn a_despawned_member_on_the_field_is_replaced() {
    let mut f = fight(4.0, TUNING);
    let [a, b, _] = f.members;
    assert!(f.sim.despawn(a));
    f.steps(1);
    assert_eq!(f.active(), b);
    assert_eq!(f.sim.party().forced_swaps(), 1);
    assert_eq!(f.view(b).position, Vec3::ZERO);
    assert_eq!(f.bodies_of(b), 2);
    // Input still names the party by its first member, whatever happened to it.
    f.steps(40);
    let z = f.view(b).position.z;
    let mut walk = TickActions::NEUTRAL;
    walk.set(a, ActorActions { buttons: Buttons::NONE, stick: Stick::new(0, 127) }).expect("room");
    f.sim.step(&walk);
    assert!(f.view(b).position.z > z);
}

// Determinism.

/// A party on the arena's square, of three who differ, each with its own assist and health, against an
/// opponent on the test moves that a script drives, aiming warps at each other.
fn scripted_world() -> (Simulation, [ActorId; 2]) {
    let mut sim = Simulation::new(SimConfig { seed: 7, max_actors: 8, ..SimConfig::default() });
    let cooked = sapphire_gameplay::arena::cooked_colliders().expect("cooks");
    let set = ColliderSet::read(&cooked).expect("reads");
    let world = PhysicsWorld::from_cooked(set, sim.body_capacity()).expect("builds");
    sim.set_physics_world(world).expect("nobody's in it yet");
    let profile = sapphire_gameplay::arena::profile();
    let controller = Some(sim.add_controller_profile(&profile).expect("a good profile"));
    let spawns = sapphire_gameplay::arena::spawns();
    let (position, yaw) = (spawns.player.position(), spawns.player.yaw());
    let members = [0, 1, 2].map(|member| {
        let moves = Some(sim.add_move_set(replay::party_moves(member).expect("compiles")));
        sim.spawn(ActorSpawn { position, yaw, moves, controller, ..ActorSpawn::default() }).expect("room")
    });
    let moves = Some(sim.add_move_set(replay::test_moves().expect("compiles")));
    let (position, yaw) = (spawns.enemy.position(), spawns.enemy.yaw());
    let opponent =
        sim.spawn(ActorSpawn { position, yaw, moves, controller, ..ActorSpawn::default() }).expect("room");
    sim.form_party(&members, replay::PARTY_TUNING).expect("a good party");
    for member in members {
        sim.set_warp_target(member, WarpTarget::Actor(opponent));
    }
    sim.set_warp_target(opponent, WarpTarget::Actor(members[0]));
    (sim, [members[0], opponent])
}

/// Long enough for [`scripted`]'s last member to have fallen.
const SCRIPTED_TICKS: u64 = 2600;

/// What the party presses in [`scripted`]'s first part, whose input names its first member throughout.
const SCRIPTED_PARTY: [(u64, Buttons); 10] = [
    (40, Buttons::LIGHT), // `light_1`, whose blade reaches the opponent on tick 46 and freezes both for 3.
    (47, Buttons::SWAP), // Pressed through that freeze: it waits, and swaps to the second as the clock moves.
    (62, Buttons::LIGHT), // Buffered out of the second's assist, whose blow has pushed back its window.
    (80, Buttons::SWAP), // Long before the second's cooldown is up: it runs out unused.
    (108, Buttons::SWAP), // A few ticks before: it waits, and swaps to the third as it ends.
    (130, Buttons::LIGHT),
    (176, Buttons::SWAP), // The first has rest to go and the second more: it runs out unused.
    (212, Buttons::SWAP), // The first has rested: back to it.
    (240, Buttons::DODGE),
    (280, Buttons::LIGHT),
];

/// The opponent's light, into the third member's braced assist, which comes through it.
const SCRIPTED_OPPONENT: [(u64, Buttons); 1] = [(118, Buttons::LIGHT)];

/// Where [`scripted`]'s first part ends, and the opponent starts knocking members down.
const SCRIPTED_SECOND_PART: u64 = 340;

/// Ticks of its second part's round: a knockdown has to play out, and the two have to meet again.
const SCRIPTED_ROUND: u64 = 130;

/// A fight of [`scripted_world`]'s. Both walk straight at each other at the start of each round, whatever the
/// last did to where they stand, and a fallen member who's still there stops neither of them.
fn scripted(tick: u64, cast: [ActorId; 2], sim: &Simulation) -> TickActions {
    let pressed = |schedule: &[(u64, Buttons)]| {
        schedule.iter().find(|(at, _)| *at == tick).map_or(Buttons::NONE, |(_, buttons)| *buttons)
    };
    let (party, opponent, walking) = if tick < SCRIPTED_SECOND_PART {
        (pressed(&SCRIPTED_PARTY), pressed(&SCRIPTED_OPPONENT), tick <= 34)
    } else {
        // The opponent's heavies, and a swap press now and then that has to find whoever's still standing.
        let at = (tick - SCRIPTED_SECOND_PART) % SCRIPTED_ROUND;
        let party = if at == 100 { Buttons::SWAP } else { Buttons::NONE };
        let opponent = if at == 44 { Buttons::HEAVY } else { Buttons::NONE };
        (party, opponent, at <= 40)
    };
    let position = |actor| sim.actor(actor).map_or(Vec3::ZERO, |view| view.position);
    let apart = position(cast[1]) - sim.party().active().map_or(Vec3::ZERO, position);
    let (x, z) = (apart.x / apart.length(), apart.z / apart.length());
    let sticks =
        if walking { [Stick::from_axes(x, z), Stick::from_axes(-x, -z)] } else { [Stick::CENTER; 2] };
    let mut actions = TickActions::NEUTRAL;
    let turns = [(cast[0], party, sticks[0]), (cast[1], opponent, sticks[1])];
    for (actor, buttons, stick) in turns {
        actions.set(actor, ActorActions { buttons, stick }).expect("two actors fit a tick");
    }
    actions
}

#[test]
fn one_set_of_inputs_gives_one_checksum() {
    let play = || {
        let (mut sim, cast) = scripted_world();
        let checksums: Vec<u64> = (0..SCRIPTED_TICKS)
            .map(|tick| {
                let actions = scripted(tick, cast, &sim);
                sim.step(&actions);
                sim.checksum()
            })
            .collect();
        assert!(sim.party().swaps() > 0);
        checksums
    };
    assert_eq!(play(), play());
}

/// The members differ, so which is which shows in a checksum as soon as anyone has to be someone. Identical
/// ones would hide an order leak.
#[test]
fn the_members_are_told_apart() {
    let ordered = [0, 1, 2].map(|member| replay::party_document(member).expect("reads"));
    assert_ne!(ordered[0], ordered[1]);
    let by_order = |order: [usize; 3]| {
        let mut f = fight_of(order.map(replay::party_moves), 1.2, TUNING);
        f.press(Buttons::SWAP);
        f.steps(30);
        f.sim.checksum()
    };
    assert_eq!(by_order([0, 1, 2]), by_order([0, 1, 2]));
    assert_ne!(by_order([0, 1, 2]), by_order([0, 2, 1]));
}

/// The party's tuning is in the hash a replay records, so changed tuning can't look like a broken simulation.
/// A fight with no party is hashed as it always was: `the_test_contents_hash…` pins that.
#[test]
fn the_tuning_is_in_the_tables_hash() {
    let hash = |tuning| fight(4.0, tuning).sim.tables_hash();
    assert_eq!(hash(TUNING), hash(TUNING));
    assert_ne!(hash(TUNING), hash(PartyTuning { rest: 151, ..TUNING }));
    assert_ne!(hash(TUNING), hash(PartyTuning { swap_cooldown: 61, ..TUNING }));
}

// A whole fight.

/// [`scripted`], tick by tick: each swap and what it's refused, a blow through the third member's armour,
/// each member's fall and the party's end. If this moves, its comments do.
#[test]
fn a_scripted_fight_does_what_its_comments_say() {
    let (mut sim, cast) = scripted_world();
    let members: Vec<ActorId> = sim.party().slots().map(|slot| slot.actor).collect();
    let mut swaps = Vec::new();
    let mut assists_landed = 0;
    let mut absorbed = 0;
    let mut fell = Vec::new();
    let mut down_at = None;
    for tick in 0..SCRIPTED_TICKS {
        let actions = scripted(tick, cast, &sim);
        sim.step(&actions);
        for cue in sim.cues() {
            if cue.cue == PARTY_SWAP {
                swaps.push((sim.tick(), cue.actor));
            }
            if cue.cue == PARTY_DEFEATED {
                down_at = Some(sim.tick());
            }
            if let Some(hit) = cue.hit {
                let current = sim.actor(cue.actor).and_then(|view| view.moves.current).map(|mv| mv.name);
                assists_landed += usize::from(current == Some(SWAP_IN_MOVE));
                absorbed += usize::from(hit.outcome == HitOutcome::Absorbed);
                if cue.cue == DEFEATED {
                    fell.push((sim.tick(), hit.target));
                }
            }
        }
    }
    let [a, b, c] = members[..] else { panic!("{members:?}") };
    assert_eq!(sim.party().active(), Some(*fell.last().map(|(_, who)| who).expect("fell")));
    assert_eq!(cast[0], a);

    // The first part, by the script's comments.
    assert_eq!(&swaps[..3], &[(51, b), (120, c), (213, a)][..], "{swaps:?}");
    assert!(assists_landed >= 2, "{assists_landed}");
    assert!(absorbed >= 1, "{absorbed}");
    // The second: each falls once, each forcing the next, and then the party's down.
    assert_eq!(fell.len(), 3, "{fell:?}");
    let stats = sim.stats();
    assert_eq!(stats.forced_swaps, 2, "{stats:?}");
    assert!(stats.swaps >= 5, "{stats:?}");
    assert!(down_at.expect("the party went down") > fell.last().expect("fell").0);
    assert!(sim.party().is_down());
}

// The real fight: the `party` scenario.

/// That scenario's fight, as a test of it needs one.
fn real_fight() -> (Simulation, [ActorId; 2]) {
    let scenario = find_scenario(&SCENARIOS, "party").expect("listed");
    let (sim, cast) = scenario.start(scenario.config).expect("sets up");
    (sim, [cast[0], cast[1]])
}

/// A tick with the party pressing this.
fn pressing(party: ActorId, buttons: Buttons) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(party, ActorActions { buttons, stick: Stick::CENTER }).expect("room");
    actions
}

/// A brain fights whoever's on the field: it read the one who left, which would leave it staring at where that
/// one last stood.
#[test]
fn a_brain_turns_on_whoever_came_in() {
    let (mut sim, [first, warrior]) = real_fight();
    let brain = |sim: &Simulation| sim.actor(warrior).expect("alive").brain.target();
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(brain(&sim), Some(first));
    sim.step(&pressing(first, Buttons::SWAP));
    let second = sim.party().active().expect("formed");
    assert_ne!(second, first);
    assert_eq!(brain(&sim), Some(second));

    // It goes on to hit them: a target it couldn't see would leave it pacing about the spot where `first` was.
    let whole = sim.actor(second).expect("alive").fighter.health();
    for _ in 0..600 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(sim.party().active(), Some(second), "nobody pressed");
    assert!(sim.actor(second).expect("alive").fighter.health() < whole, "it was fought");
    assert_eq!(sim.actor(first).expect("alive").fighter.health(), whole, "and the one off the field wasn't");
}

/// A forced swap turns it too, all the way to the party's end.
#[test]
fn a_brain_follows_each_defeat() {
    let (mut sim, [first, warrior]) = real_fight();
    let mut targets = vec![first];
    for _ in 0..20_000 {
        sim.step(&TickActions::NEUTRAL);
        let target = sim.actor(warrior).expect("alive").brain.target().expect("thinking");
        if targets.last() != Some(&target) {
            targets.push(target);
        }
        if sim.party().is_down() {
            break;
        }
    }
    assert!(sim.party().is_down(), "{:?}", sim.stats());
    assert_eq!(targets, sim.party().slots().map(|slot| slot.actor).collect::<Vec<_>>());
}

/// The party's document with this in place of that, as a person saves it: a live edit (M2 T2).
fn edited(from: &str, to: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../content/combat/player/player.moves.ron");
    let text = std::fs::read_to_string(path).expect("the moves");
    assert!(text.contains(from), "{from}");
    text.replacen(from, to, 1)
}

/// All three play one document, so a live edit reaches each, on the field or off it, and leaves those off it
/// off it.
#[test]
fn a_live_edit_reaches_the_whole_party() {
    let (mut sim, [first, _]) = real_fight();
    let scenario = find_scenario(&SCENARIOS, "party").expect("listed");
    assert_eq!(
        scenario.documents,
        ["content/combat/player/player.moves.ron", "content/combat/enemy/warrior.moves.ron"]
    );
    let id = scenario.set_of("player.moves.ron").expect("its first");
    let members: Vec<ActorId> = sim.party().slots().map(|slot| slot.actor).collect();
    sim.step(&pressing(first, Buttons::SWAP));
    // Past the assist's i-frames, so whoever's on the field has a hurtbox to show.
    for _ in 0..6 {
        sim.step(&TickActions::NEUTRAL);
    }

    let tougher = edited("health: 300,", "health: 600,");
    let set = compile_document(&tougher, sim.move_library().current(id)).expect("compiles").set;
    assert!(matches!(sim.tune_move_set(id, set.clone()), Some(Tuned::Replaced { .. })));
    sim.step(&TickActions::NEUTRAL);
    for member in &members {
        assert_eq!(sim.actor(*member).expect("alive").fighter.max_health(), 600, "{member:?}");
    }
    let hurtbox = |sim: &Simulation, member| {
        sim.physics().body_shape(HurtboxId::new(member, 0).expect("in range").body()).is_some()
    };
    assert_eq!(members.iter().map(|member| hurtbox(&sim, *member)).collect::<Vec<_>>(), [false, true, false]);

    // And the next fight starts from it, as a party.
    let mut live = LiveMoves::default();
    live.remember("player.moves.ron", set);
    let (again, _) = live.start(scenario, scenario.config).expect("sets up");
    assert_eq!(again.party().active(), Some(first));
    for slot in again.party().slots() {
        assert_eq!(again.actor(slot.actor).expect("alive").fighter.max_health(), 600);
    }
    assert!(again.actor(members[1]).expect("alive").off_field);
}

/// A live edit can take the assist away. The swap is the ability, so it still happens, with nothing to play.
#[test]
fn a_swap_with_no_assist_left_still_swaps() {
    let mut f = fight(4.0, TUNING);
    let [a, b, _] = f.members;
    let mut bare = replay::party_document(1).expect("reads");
    bare.moves.retain(|mv| mv.name != "swap_in");
    let id = f.sim.actor(b).expect("alive").moves.set.expect("its own");
    f.sim.replace_move_set(id, bare.compile().expect("compiles")).expect("its set");
    f.press(Buttons::SWAP);
    assert_eq!(f.active(), b);
    assert_eq!(f.move_name(b), None);
    assert_eq!(f.view(a).moves.current, None);
}

/// A new fight is what a build with the edit would make, and a party doesn't form on a set with no assist.
#[test]
fn a_new_fight_refuses_an_edit_that_leaves_no_assist() {
    let (sim, _) = real_fight();
    let scenario = find_scenario(&SCENARIOS, "party").expect("listed");
    let id = scenario.set_of("player.moves.ron").expect("its first");
    let mut document = MoveSetDocument::parse(&edited("", "")).expect("parses");
    document.moves.retain(|mv| mv.name != "swap_in");
    let set = document.compile_over(sim.move_library().current(id).expect("its set")).expect("compiles");
    let mut live = LiveMoves::default();
    live.remember("player.moves.ron", set);
    let refused = live.start(scenario, scenario.config).map(|_| ());
    assert!(matches!(refused, Err(ScenarioError::Party(PartyError::NoSwapInMove { .. }))), "{refused:?}");
}

/// The suite's `party-swap`, tick by tick: what [`replay`]'s script says of it. If this moves, its comments do.
#[test]
fn the_recorded_fight_does_what_its_script_says() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/replays/party-swap.sreplay"))
        .expect("in the suite");
    let mut player = Player::new(Replay::decode(&bytes).expect("decodes"), &SCENARIOS).expect("starts");
    let warrior = player.cast()[1];
    let members: Vec<ActorId> = player.simulation().party().slots().map(|slot| slot.actor).collect();
    let [a, b, c] = members[..] else { panic!("{members:?}") };
    let mut swaps = Vec::new();
    let mut assists_landed = 0;
    let mut fell = Vec::new();
    let mut down_at = None;
    while player.step() {
        let sim = player.simulation();
        for cue in sim.cues() {
            if cue.cue == PARTY_SWAP {
                swaps.push((sim.tick(), cue.actor));
            }
            if cue.cue == PARTY_DEFEATED {
                down_at = Some(sim.tick());
            }
            if let Some(hit) = cue.hit {
                let current = sim.actor(cue.actor).and_then(|view| view.moves.current).map(|mv| mv.name);
                assists_landed += usize::from(current == Some(SWAP_IN_MOVE));
                if cue.cue == DEFEATED {
                    fell.push(hit.target);
                }
            }
        }
    }
    // Three pressed, and a tick after the script's: a step is done when `tick` reads the next one.
    assert_eq!(&swaps[..3], &[(87, b), (201, c), (291, a)][..], "{swaps:?}");
    // Each falls in turn, and each fall brings in the next one standing.
    assert_eq!(fell, [a, b, c]);
    assert_eq!(&swaps[3..].iter().map(|(_, who)| *who).collect::<Vec<_>>(), &[b, c]);
    assert!(assists_landed >= 3, "{assists_landed}");
    let sim = player.simulation();
    assert_eq!((sim.party().swaps(), sim.party().forced_swaps()), (5, 2));
    assert!(down_at.is_some() && sim.party().is_down());
    assert_eq!(sim.party().active(), Some(c));
    let warrior = sim.actor(warrior).expect("alive");
    assert!(!warrior.fighter.is_defeated(), "it won");
    assert_eq!(warrior.brain.target(), Some(c));
}

/// A member is the test content with a swap and an assist more, and nothing less.
#[test]
fn the_party_document_is_the_slice_with_a_swap_and_an_assist() {
    let slice = MoveSetDocument::parse(include_str!("../../sapphire_moves/test_content/slice.moves.ron"))
        .expect("parses");
    let member = replay::party_document(2).expect("reads");
    assert_eq!(member.intents.len(), slice.intents.len() + 1);
    assert_eq!(member.moves.len(), slice.moves.len() + 1);
    let set = member.compile().expect("compiles");
    assert!(set.intent(SWAP_INTENT).is_some() && set.find(SWAP_IN_MOVE).is_some());
}
