//! A fighter's `movement` section (M2 T2), through real steps: what a document says is what its actors
//! move by, a live edit is what the next step moves by and moves nobody by itself, and a party's members and
//! a brain's enemy each keep their own. The numbers by hand are the controller's: 60 ticks a second, so an
//! acceleration of 90 m/s² is 1.5 m/s a tick and a deceleration of 60 is 1.

#![allow(clippy::print_stdout, reason = "the far ends report what they measured")]

use sapphire_gameplay::replay::{self, LiveMoves, SCENARIOS, find_scenario, find_script};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, ControllerProfile, ControllerState, GroundMode, MoveSetId,
    PartyTuning, ResolvedProfile, SimConfig, Simulation, Stick, TickActions, Tuned, actor_body,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::{MAX_JUMP_REACH, MOVEMENT_LIMITS, MoveSet, MoveSetDocument};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};

const SLICE: &str = include_str!("../../sapphire_moves/test_content/slice.moves.ron");
const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");

const TODAY: ControllerProfile = ControllerProfile {
    height: 1.6,
    radius: 0.3,
    skin: 0.01,
    walk_speed: 2.0,
    run_speed: 6.0,
    walk_stick: 0.5,
    run_stick: 0.9,
    acceleration: 90.0,
    deceleration: 60.0,
    turn_rate_degrees: 1080.0,
    gravity: 22.0,
    jump_speed: 7.0,
    air_acceleration: 18.0,
    max_fall_speed: 30.0,
    coyote_ticks: 5,
    slope_limit_degrees: 50.0,
    step_height: 0.3,
    ground_snap: 0.3,
    stance_radius: 0.15,
    max_slide_speed: 8.0,
};

fn full_stick() -> Stick {
    Stick::new(0, 127)
}

fn slice_document(movement: Option<ControllerProfile>) -> MoveSetDocument {
    let mut document = MoveSetDocument::parse(SLICE).expect("parses");
    document.fighter.as_mut().expect("in the content").movement = movement;
    document
}

fn slice(movement: Option<ControllerProfile>) -> MoveSet {
    slice_document(movement).compile().expect("compiles")
}

fn ground() -> StaticCollider {
    StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 0.5, 200.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    }
}

fn solid(id: u32, min: Vec3, max: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents: (max - min) / 2.0 }),
        pose: Pose::at((min + max) / 2.0),
        memberships: Group::World.mask(),
    }
}

/// A two-sided slope rising along +x from the origin at `degrees`, over 20 m of x, 40 m wide.
fn slope(id: u32, degrees: f32) -> StaticCollider {
    let rise = 20.0 * det::tan(degrees.to_radians());
    let corners = [
        Vec3::new(0.0, 0.0, -20.0),
        Vec3::new(20.0, rise, -20.0),
        Vec3::new(20.0, rise, 20.0),
        Vec3::new(0.0, 0.0, 20.0),
    ];
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Mesh { vertices: corners.to_vec(), indices: vec![[0, 2, 1], [0, 3, 2]] },
        pose: Pose::IDENTITY,
        memberships: Group::World.mask(),
    }
}

/// One character on what's given, on a set of moves, spawned with a profile.
struct Stage {
    sim: Simulation,
    actor: ActorId,
    set: MoveSetId,
}

impl Stage {
    fn new(set: MoveSet) -> Self {
        Self::on(vec![ground()], set, &ControllerProfile::default(), Vec3::ZERO)
    }

    fn on(statics: Vec<StaticCollider>, set: MoveSet, spawned: &ControllerProfile, feet: Vec3) -> Self {
        let mut sim = Simulation::new(SimConfig { max_actors: 4, ..SimConfig::default() });
        let world = PhysicsWorld::new(statics, sim.body_capacity()).expect("builds");
        sim.set_physics_world(world).expect("nobody's in it yet");
        let controller = Some(sim.add_controller_profile(spawned).expect("a good profile"));
        let id = sim.add_move_set(set);
        let moves = Some(id);
        let actor = sim
            .spawn(ActorSpawn { position: feet, yaw: 0.0, moves, controller, ..ActorSpawn::default() })
            .expect("room");
        let mut stage = Self { sim, actor, set: id };
        stage.hold(Stick::CENTER, 2);
        stage
    }

    fn step(&mut self, stick: Stick, buttons: Buttons) {
        let mut actions = TickActions::NEUTRAL;
        actions.set(self.actor, ActorActions { buttons, stick }).expect("room");
        self.sim.step(&actions);
    }

    fn hold(&mut self, stick: Stick, ticks: u32) {
        for _ in 0..ticks {
            self.step(stick, Buttons::NONE);
        }
    }

    fn state(&self) -> ControllerState {
        self.sim.actor(self.actor).expect("alive").controller.state
    }

    fn feet(&self) -> Vec3 {
        self.sim.actor(self.actor).expect("alive").position
    }

    fn speed(&self) -> f32 {
        let velocity = self.state().velocity;
        Vec3::new(velocity.x, 0.0, velocity.z).length()
    }

    fn tune(&mut self, movement: Option<ControllerProfile>) -> Tuned {
        self.sim.tune_move_set(self.set, slice(movement)).expect("its set")
    }

    /// The capsule the world holds, and where its center is over the feet.
    fn body(&self) -> (Shape, Vec3) {
        let physics = self.sim.physics();
        let body = actor_body(self.actor);
        let shape = physics.body_shape(body).expect("in the world");
        (shape, physics.body_pose(body).expect("in the world").position - self.feet())
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// On flat ground, a new capsule's feet rest within the rounding the controller leaves alone (`SETTLE`).
fn on_the_ground(feet: Vec3) -> bool {
    feet.y.abs() <= 1e-4
}

// --- What a document says ---

/// The form that disturbs nothing: a document that says nothing has its actors go on as they spawned.
#[test]
fn with_no_section_an_actor_moves_by_the_profile_it_spawned_with() {
    let spawned = ControllerProfile { run_speed: 7.5, ..ControllerProfile::default() };
    let mut stage = Stage::on(vec![ground()], slice(None), &spawned, Vec3::ZERO);
    stage.hold(full_stick(), 20);
    assert!(close(stage.speed(), 7.5), "{}", stage.speed());
    assert_eq!(stage.sim.movement_of(stage.actor).expect("it has one").authored().run_speed, 7.5);
}

#[test]
fn a_section_moves_its_actors_by_its_own_numbers_whatever_they_spawned_with() {
    let movement = ControllerProfile { run_speed: 3.5, jump_speed: 5.0, gravity: 10.0, ..TODAY };
    let spawned = ControllerProfile { run_speed: 9.0, ..TODAY };
    let mut stage = Stage::on(vec![ground()], slice(Some(movement)), &spawned, Vec3::ZERO);
    stage.hold(full_stick(), 20);
    assert!(close(stage.speed(), 3.5), "{}", stage.speed());

    // It reaches 5² / (2 × 10) = 1.25 m, a little under for the sum of whole ticks.
    stage.step(Stick::CENTER, Buttons::JUMP);
    let mut highest: f32 = 0.0;
    for _ in 0..120 {
        stage.hold(Stick::CENTER, 1);
        highest = highest.max(stage.feet().y);
    }
    assert!((highest - 1.25).abs() < 0.05, "{highest}");
    assert_eq!(stage.state().mode, GroundMode::Grounded, "and comes down");
}

/// A capsule from a document stands in the world from its spawn, with its feet on the ground.
#[test]
fn a_section_gives_its_actors_their_capsule() {
    let movement = ControllerProfile { height: 2.0, radius: 0.4, skin: 0.02, ..TODAY };
    let mut stage = Stage::new(slice(Some(movement)));
    let (shape, over_feet) = stage.body();
    assert_eq!(shape, Shape::Capsule { half_height: 0.6, radius: 0.4 });
    assert_eq!(over_feet, Vec3::Y * 1.02);
    stage.hold(full_stick(), 30);
    assert!(on_the_ground(stage.feet()), "{}", stage.feet());
}

/// An empty section is the defaults, each number left out is, and they read through the text.
#[test]
fn a_section_names_only_what_it_changes() {
    let text = SLICE.replacen("fighter: (", "fighter: (\n movement: (run_speed: 7.5),", 1);
    let set = MoveSetDocument::parse(&text).expect("parses").compile().expect("compiles");
    assert_eq!(set.movement(), Some(&ControllerProfile { run_speed: 7.5, ..ControllerProfile::default() }));
    let text = SLICE.replacen("fighter: (", "fighter: (\n movement: (),", 1);
    let set = MoveSetDocument::parse(&text).expect("parses").compile().expect("compiles");
    assert_eq!(set.movement(), Some(&ControllerProfile::default()));
    assert_eq!(slice(None).movement(), None);
}

/// [`TODAY`] is written out here so a change of the defaults is seen: they're what every document
/// without a section plays by, so they move what no document names.
#[test]
fn the_warrior_keeps_the_defaults_and_the_player_has_its_own_profile() {
    const PLAYER_PROFILE: ControllerProfile =
        ControllerProfile { acceleration: 54.0, deceleration: 42.0, turn_rate_degrees: 720.0, ..TODAY };
    assert_eq!(ControllerProfile::default(), TODAY);
    for (name, text, expected) in [("warrior", WARRIOR, TODAY), ("player", PLAYER, PLAYER_PROFILE)] {
        let document = MoveSetDocument::parse(text).expect("parses");
        let fighter = document.fighter.expect("a fighter");
        assert_eq!(fighter.movement, Some(expected), "{name}'s document has its complete movement profile");
    }
    let baked = replay::player_moves().expect("the simulation's copy reads");
    assert_eq!(baked.movement(), Some(&PLAYER_PROFILE), "the simulation uses the player's authored profile");
}

/// The brief's first rule, in one place: today's numbers in a section are no change to any fight. The
/// replay suite holds the same of the committed documents, tick by tick (`tests/replays`).
#[test]
fn todays_numbers_in_a_section_change_no_tick_of_a_fight() {
    let run = |scenario: &str, script: &str, edits: &[(&str, MoveSet)]| {
        let scenario = find_scenario(&SCENARIOS, scenario).expect("listed");
        let script = find_script(script).expect("listed");
        let mut live = LiveMoves::default();
        for (name, set) in edits {
            live.remember(name, set.clone());
        }
        let (mut sim, cast) = live.start(scenario, scenario.config).expect("sets up");
        let checksums: Vec<u64> = (0..u64::from(script.ticks))
            .map(|tick| {
                sim.step(&(script.actions)(tick, &cast));
                sim.checksum()
            })
            .collect();
        checksums
    };
    let with_and_without = |text: &str| {
        let mut document = MoveSetDocument::parse(text).expect("parses");
        let fighter = document.fighter.as_mut().expect("a fighter");
        fighter.movement = None;
        let without = document.compile();
        let fighter = document.fighter.as_mut().expect("a fighter");
        fighter.movement = Some(ControllerProfile::default());
        (without, document.compile())
    };

    let (without, with) = with_and_without(WARRIOR);
    let name = "warrior.moves.ron";
    let (without, with) = (without.expect("compiles"), with.expect("compiles"));
    assert_eq!(
        run("arena-fight", "fight-back", &[(name, without)]),
        run("arena-fight", "fight-back", &[(name, with)])
    );

    let (without, with) = with_and_without(SLICE);
    let name = "slice.moves.ron";
    let (without, with) = (without.expect("compiles"), with.expect("compiles"));
    assert_eq!(run("duel", "fight", &[(name, without)]), run("duel", "fight", &[(name, with)]));
}

// --- A live edit ---

/// The rule itself: from the next step, each way.
#[test]
fn the_next_step_moves_by_an_edit() {
    let mut stage = Stage::new(slice(Some(TODAY)));
    stage.hold(full_stick(), 10);
    assert_eq!(stage.speed(), 6.0);

    assert_eq!(
        stage.tune(Some(ControllerProfile { run_speed: 3.0, ..TODAY })),
        Tuned::Replaced { revision: 1 }
    );
    assert_eq!(stage.speed(), 6.0, "an edit moves nothing by itself");
    stage.hold(full_stick(), 1);
    assert!(close(stage.speed(), 5.0), "slowing by its deceleration: {}", stage.speed());
    stage.hold(full_stick(), 2);
    assert!(close(stage.speed(), 3.0), "{}", stage.speed());

    stage.tune(Some(ControllerProfile { run_speed: 9.0, ..TODAY }));
    stage.hold(full_stick(), 1);
    assert!(close(stage.speed(), 4.5), "speeding up by its acceleration: {}", stage.speed());
    stage.hold(full_stick(), 3);
    assert!(close(stage.speed(), 9.0), "{}", stage.speed());

    stage.tune(Some(ControllerProfile { acceleration: 6.0, run_speed: 3.0, ..TODAY }));
    stage.hold(Stick::CENTER, 12);
    assert_eq!(stage.speed(), 0.0, "stopped");
    stage.hold(full_stick(), 1);
    assert!(close(stage.speed(), 0.1), "and off again at the new rate: {}", stage.speed());
}

/// What a person sees of each number, on the very next press that reads it.
#[test]
fn the_next_jump_and_the_next_turn_read_an_edit() {
    let mut stage = Stage::new(slice(Some(TODAY)));
    stage.tune(Some(ControllerProfile { jump_speed: 3.0, ..TODAY }));
    stage.step(Stick::CENTER, Buttons::JUMP);
    assert!(stage.state().velocity.y < 3.0 && stage.state().velocity.y > 2.0, "{:?}", stage.state());
    stage.hold(Stick::CENTER, 60);

    stage.tune(Some(ControllerProfile { turn_rate_degrees: 60.0, ..TODAY }));
    stage.hold(Stick::new(127, 0), 1);
    let yaw = stage.sim.actor(stage.actor).expect("alive").yaw;
    assert!(close(yaw, 1.0_f32.to_radians()), "a degree a tick: {yaw}");
}

/// A jump begun under one gravity finishes under the next, to the bit, as long as that's a gravity its
/// speed could have been a jump under. The random attack found what happens past that: a jump at today's
/// numbers met a typed gravity of 2 and rose 7 m more, over the arena's walls, with no number at fault.
#[test]
fn a_gravity_too_light_for_a_jump_in_the_air_cuts_what_it_carries_and_moves_nobody() {
    let rising = || {
        let mut stage = Stage::new(slice(Some(TODAY)));
        stage.step(Stick::CENTER, Buttons::JUMP);
        stage.hold(Stick::CENTER, 3);
        assert_eq!(stage.state().mode, GroundMode::Airborne);
        assert!(stage.state().velocity.y > 5.0, "{:?}", stage.state());
        stage
    };

    let mut stage = rising();
    let was = stage.state();
    stage.tune(Some(ControllerProfile { gravity: 12.0, ..TODAY }));
    assert_eq!(stage.state(), was, "this gravity could have thrown it that fast");

    let mut stage = rising();
    let (feet, was) = (stage.feet(), stage.state());
    let light = ControllerProfile { gravity: 2.0, jump_speed: 3.0, run_speed: 2.0, ..TODAY };
    assert!(light.document_issues().is_empty(), "{:?}", light.document_issues());
    stage.tune(Some(light));
    assert_eq!(stage.feet(), feet, "it moves nobody");
    let cut = stage.state().velocity.y;
    assert!(close(cut, (2.0_f32 * 2.0 * MAX_JUMP_REACH).sqrt()), "{cut}, from {}", was.velocity.y);
    let mut highest = feet.y;
    for _ in 0..240 {
        stage.step(Stick::CENTER, Buttons::NONE);
        highest = highest.max(stage.feet().y);
    }
    assert!(highest - feet.y <= MAX_JUMP_REACH + 0.01, "it rose {} m more", highest - feet.y);
    assert!(highest - feet.y > 2.0, "and still finishes a jump: {} m", highest - feet.y);

    // Going across it stays, since nothing in the air turns it upward, and a fall is the fall limit's to
    // hold.
    let numbers = ResolvedProfile::new(&light).expect("a good profile");
    let carried = numbers.carried(Vec3::new(12.0, 1.0, -9.0));
    assert_eq!(carried, Vec3::new(12.0, 1.0, -9.0));
    assert_eq!(numbers.carried(Vec3::new(1.0, -30.0, -1.0)), Vec3::new(1.0, -30.0, -1.0));
}

/// A member takes over the motion of the one before it, and its own gravity decides how much of that it
/// can carry: one taking over a jump at its peak speed under light numbers doesn't leave the world, and
/// one with the same numbers takes every bit of it.
#[test]
fn a_swap_in_the_air_carries_only_what_the_next_members_gravity_can() {
    let light = ControllerProfile { gravity: 2.0, jump_speed: 3.0, run_speed: 2.0, ..TODAY };
    for (next, same) in [(light, false), (TODAY, true)] {
        let mut party = Party::of([TODAY, next, TODAY]);
        party.step(Stick::CENTER, Buttons::NONE);
        party.step(Stick::CENTER, Buttons::JUMP);
        party.step(Stick::CENTER, Buttons::NONE);
        let first = party.sim.actor(party.on_field()).expect("alive");
        let (feet, rising) = (first.position, first.controller.state.velocity.y);
        assert!(rising > 5.0, "{rising}");
        party.step(Stick::CENTER, Buttons::SWAP);
        assert_eq!(party.on_field(), party.members[1], "the swap fired");
        let taken = party.sim.actor(party.members[1]).expect("alive").controller.state.velocity.y;
        if same {
            assert!(taken > 5.0, "under its own numbers it keeps what it was given: {taken}");
        } else {
            assert!(taken <= (2.0_f32 * 2.0 * MAX_JUMP_REACH).sqrt(), "{taken}, from {rising}");
        }
        let mut highest = feet.y;
        for _ in 0..240 {
            party.step(Stick::CENTER, Buttons::NONE);
            highest = highest.max(party.sim.actor(party.members[1]).expect("alive").position.y);
        }
        assert!(highest - feet.y <= MAX_JUMP_REACH + 0.2, "it rose {} m more", highest - feet.y);
    }
}

/// Saving a file again is no edit, and a body stays the very body it was.
#[test]
fn the_same_bytes_are_no_edit() {
    let mut stage = Stage::new(slice(Some(TODAY)));
    assert_eq!(stage.tune(Some(TODAY)), Tuned::Unchanged { revision: 0 });
    assert_eq!(
        stage.tune(Some(ControllerProfile { run_speed: 6.5, ..TODAY })),
        Tuned::Replaced { revision: 1 }
    );
    assert_eq!(
        stage.tune(Some(ControllerProfile { run_speed: 6.5, ..TODAY })),
        Tuned::Unchanged { revision: 1 }
    );
    assert_eq!(stage.sim.move_library().current_revision(stage.set), Some(1));
}

/// A document can gain a section or lose one, and an actor moves by what it then says, spawned profile and
/// capsule included.
#[test]
fn taking_a_section_away_goes_back_to_the_spawned_profile() {
    let spawned = ControllerProfile { run_speed: 7.5, ..TODAY };
    let mut stage = Stage::on(vec![ground()], slice(None), &spawned, Vec3::ZERO);
    stage.hold(full_stick(), 20);

    stage.tune(Some(ControllerProfile { run_speed: 3.0, height: 2.0, ..TODAY }));
    stage.hold(full_stick(), 10);
    assert!(close(stage.speed(), 3.0), "{}", stage.speed());
    assert_eq!(stage.body().0, Shape::Capsule { half_height: 0.7, radius: 0.3 });

    stage.tune(None);
    stage.hold(full_stick(), 10);
    assert!(close(stage.speed(), 7.5), "{}", stage.speed());
    assert_eq!(stage.body().0, Shape::Capsule { half_height: 0.5, radius: 0.3 });
    assert!(on_the_ground(stage.feet()), "{}", stage.feet());
}

/// Today's numbers with one of them at each end of what a document may say, and a third of the way along.
/// Numbers that hold each other back take their partners along, so each stands by itself.
fn every_edit() -> Vec<(&'static str, f32, ControllerProfile)> {
    let mut edits = Vec::new();
    for limit in MOVEMENT_LIMITS {
        for value in [limit.min, limit.max, limit.min + (limit.max - limit.min) / 3.0] {
            let mut profile = TODAY;
            set_number(&mut profile, limit.field, value);
            match limit.field {
                "radius" => {
                    profile.height = profile.height.max(2.0 * value + 0.1);
                    profile.skin = profile.skin.min(value / 4.0);
                    profile.stance_radius = profile.stance_radius.min(value);
                }
                "height" => {
                    profile.radius = profile.radius.min(value / 2.0 - 0.01);
                    profile.skin = profile.skin.min(profile.radius / 4.0);
                    profile.stance_radius = profile.stance_radius.min(profile.radius);
                }
                "skin" => profile.radius = profile.radius.max(value * 4.0),
                "walk_speed" => profile.run_speed = profile.run_speed.max(value),
                "run_speed" => profile.walk_speed = profile.walk_speed.min(value),
                "walk_stick" => profile.run_stick = profile.run_stick.max(value + 0.01),
                "run_stick" => profile.walk_stick = profile.walk_stick.min(value - 0.01),
                "gravity" => {
                    profile.jump_speed = profile.jump_speed.min((value).sqrt());
                    profile.run_speed = profile.run_speed.min((2.0 * value).sqrt());
                    profile.walk_speed = profile.walk_speed.min(profile.run_speed);
                }
                "stance_radius" => {
                    profile.radius = profile.radius.max(value);
                    profile.height = profile.height.max(2.0 * value + 0.1);
                }
                _ => {}
            }
            if limit.field != "gravity" {
                // What's in the air, and what can be thrown up, take a heavier world at the far ends.
                let (run, jump) = (profile.run_speed, profile.jump_speed);
                profile.gravity =
                    profile.gravity.max(((run * run + jump * jump) / 7.0).max(jump * jump / 4.5));
            }
            assert_eq!(profile.document_issues(), Vec::<String>::new(), "{} at {value}", limit.field);
            edits.push((limit.field, value, profile));
        }
    }
    edits
}

fn set_number(profile: &mut ControllerProfile, field: &str, value: f32) {
    match field {
        "height" => profile.height = value,
        "radius" => profile.radius = value,
        "skin" => profile.skin = value,
        "walk_speed" => profile.walk_speed = value,
        "run_speed" => profile.run_speed = value,
        "walk_stick" => profile.walk_stick = value,
        "run_stick" => profile.run_stick = value,
        "acceleration" => profile.acceleration = value,
        "deceleration" => profile.deceleration = value,
        "turn_rate_degrees" => profile.turn_rate_degrees = value,
        "gravity" => profile.gravity = value,
        "jump_speed" => profile.jump_speed = value,
        "air_acceleration" => profile.air_acceleration = value,
        "max_fall_speed" => profile.max_fall_speed = value,
        "coyote_ticks" => profile.coyote_ticks = value as u32,
        "slope_limit_degrees" => profile.slope_limit_degrees = value,
        "step_height" => profile.step_height = value,
        "ground_snap" => profile.ground_snap = value,
        "stance_radius" => profile.stance_radius = value,
        "max_slide_speed" => profile.max_slide_speed = value,
        other => panic!("no number called {other}"),
    }
}

/// How a character can be when an edit lands.
#[derive(Clone, Copy, Debug)]
enum Doing {
    Standing,
    Running,
    Rising,
    Falling,
    Sliding,
    InAMove,
}

const EVERYTHING: [Doing; 6] =
    [Doing::Standing, Doing::Running, Doing::Rising, Doing::Falling, Doing::Sliding, Doing::InAMove];

impl Doing {
    /// A stage whose character is doing this now, and the stick it goes on holding.
    fn stage(self) -> (Stage, Stick) {
        match self {
            Self::Standing => (Stage::new(slice(Some(TODAY))), Stick::CENTER),
            Self::Running => {
                let mut stage = Stage::new(slice(Some(TODAY)));
                stage.hold(full_stick(), 20);
                (stage, full_stick())
            }
            Self::Rising | Self::Falling => {
                let mut stage = Stage::new(slice(Some(TODAY)));
                stage.hold(full_stick(), 20);
                stage.step(full_stick(), Buttons::JUMP);
                stage.hold(full_stick(), if matches!(self, Self::Rising) { 5 } else { 30 });
                assert_eq!(stage.state().mode, GroundMode::Airborne);
                let rising = stage.state().velocity.y > 0.0;
                assert_eq!(rising, matches!(self, Self::Rising));
                (stage, full_stick())
            }
            Self::Sliding => {
                // A slope too steep for today's numbers, well above the floor.
                let statics = vec![ground(), slope(1, 65.0)];
                let feet = Vec3::new(8.0, 20.0, 0.0);
                let mut stage = Stage::on(statics, slice(Some(TODAY)), &ControllerProfile::default(), feet);
                for _ in 0..600 {
                    if stage.state().mode == GroundMode::Sliding && stage.speed() > 1.0 {
                        break;
                    }
                    stage.hold(Stick::CENTER, 1);
                }
                assert_eq!(stage.state().mode, GroundMode::Sliding);
                (stage, Stick::CENTER)
            }
            Self::InAMove => {
                let mut stage = Stage::new(slice(Some(TODAY)));
                stage.step(Stick::CENTER, Buttons::LIGHT);
                stage.hold(Stick::CENTER, 5);
                assert!(stage.sim.actor(stage.actor).expect("alive").moves.current.is_some());
                (stage, Stick::CENTER)
            }
        }
    }
}

/// How far a tick at these velocities carries a character: what it would have gone anyway.
fn tick_of(before: Vec3, after: Vec3) -> f32 {
    (before.length() + after.length()) * 0.5 / 60.0
}

/// An edit lands on a character doing anything, and it neither jumps nor teleports: the edit moves nobody,
/// and the next step goes no further than its velocities carry it. A capsule's own change adds what the
/// surfaces it's on can push it by, no more than it grew. Every number at each end of its limits.
#[test]
fn an_edit_moves_nobody_and_the_next_step_only_as_far_as_its_speed() {
    let mut widest = 0.0_f32;
    for doing in EVERYTHING {
        for (field, value, profile) in every_edit() {
            let (mut stage, stick) = doing.stage();
            let (feet, was) = (stage.feet(), stage.state());
            let radius = (profile.radius - TODAY.radius).abs();
            let height = (profile.height - TODAY.height).abs();

            let same = profile == TODAY;
            let tuned = stage.tune(Some(profile));
            assert_eq!(matches!(tuned, Tuned::Unchanged { .. }), same, "{doing:?}, {field} at {value}");
            let label = format!("{doing:?}, {field} at {value}");
            assert_eq!(stage.feet(), feet, "{label}: the edit moved it");
            let mut carrying = was;
            carrying.velocity = ResolvedProfile::new(&profile).expect("a good profile").carried(was.velocity);
            assert_eq!(stage.state(), carrying, "{label}: the edit changed what it's doing");
            let (shape, over_feet) = stage.body();
            assert_eq!(
                shape,
                Shape::Capsule { half_height: profile.height / 2.0 - profile.radius, radius: profile.radius },
                "{label}: its capsule"
            );
            assert!(
                (over_feet.y - (profile.height / 2.0 + profile.skin)).abs() < 1e-4 && over_feet.x == 0.0,
                "{label}: its capsule stands on its feet: {over_feet}"
            );

            stage.step(stick, Buttons::NONE);
            let moved = (stage.feet() - feet).length();
            let carried = tick_of(was.velocity, stage.state().velocity);
            // Ground that has just become walkable takes a slider's foot down to it, by centimetres, and a
            // move's table carries it on top of what its velocity says.
            let room = 0.05 + 0.15 * f32::from(matches!(doing, Doing::InAMove)) + 2.0 * radius + height;
            assert!(moved <= carried + room, "{label}: went {moved} m, carried {carried} m");
            assert!(stage.feet().is_finite() && stage.state().velocity.is_finite(), "{label}");
            widest = widest.max(moved - carried);

            // And it goes on: nothing has left it stuck, or sent it anywhere.
            stage.hold(stick, 30);
            assert!(stage.feet().is_finite() && stage.feet().y > -1.0, "{label}: {}", stage.feet());
        }
    }
    println!("the most a step went past its speed after an edit: {widest} m");
}

/// A frozen character stays frozen across an edit, and thaws onto the new numbers.
#[test]
fn an_edit_during_hitstop_waits_for_the_thaw() {
    let mut sim = Simulation::new(SimConfig { max_actors: 4, ..SimConfig::default() });
    let world = PhysicsWorld::new(vec![ground()], sim.body_capacity()).expect("builds");
    sim.set_physics_world(world).expect("nobody's in it yet");
    let controller = Some(sim.add_controller_profile(&TODAY).expect("a good profile"));
    let id = sim.add_move_set(slice(Some(TODAY)));
    let moves = Some(id);
    let attacker = sim
        .spawn(ActorSpawn { position: Vec3::ZERO, yaw: 0.0, moves, controller, ..ActorSpawn::default() })
        .expect("room");
    let target = sim
        .spawn(ActorSpawn {
            position: Vec3::new(0.0, 0.0, 1.0),
            yaw: std::f32::consts::PI,
            moves,
            controller,
            ..ActorSpawn::default()
        })
        .expect("room");
    let mut press = TickActions::NEUTRAL;
    press.set(attacker, ActorActions { buttons: Buttons::LIGHT, stick: Stick::CENTER }).expect("room");
    sim.step(&press);
    let mut frozen = false;
    for _ in 0..60 {
        sim.step(&TickActions::NEUTRAL);
        if sim.actor(target).expect("alive").clock_delta.as_subticks() == 0 {
            frozen = true;
            break;
        }
    }
    assert!(frozen, "the light hit lands, and freezes both");
    let held = sim.actor(target).expect("alive").position;
    let edited = slice(Some(ControllerProfile { height: 2.2, run_speed: 2.0, ..TODAY }));
    sim.tune_move_set(id, edited).expect("its set");
    assert_eq!(sim.actor(target).expect("alive").position, held);
    sim.step(&TickActions::NEUTRAL);
    if sim.actor(target).expect("alive").clock_delta.as_subticks() == 0 {
        assert_eq!(sim.actor(target).expect("alive").position, held, "frozen, it doesn't move");
    }
    let spot = sim.physics().body_shape(actor_body(target)).expect("in the world");
    assert_eq!(
        spot,
        Shape::Capsule { half_height: 0.8, radius: 0.3 },
        "though its capsule has the new shape"
    );
}

/// Growing beside a wall: the next move pushes it out, as a spawn inside a wall is, by what it grew.
#[test]
fn a_bigger_capsule_beside_a_wall_is_pushed_out_by_what_it_grew() {
    let wall = solid(1, Vec3::new(0.32, 0.0, -50.0), Vec3::new(1.0, 5.0, 50.0));
    let mut stage = Stage::on(vec![ground(), wall], slice(Some(TODAY)), &TODAY, Vec3::ZERO);
    stage.hold(Stick::new(127, 0), 30);
    let by_the_wall = stage.feet().x;
    assert!(by_the_wall > 0.0 && by_the_wall <= 0.02 + 1e-3, "resting a skin off it: {by_the_wall}");

    stage.tune(Some(ControllerProfile { radius: 0.5, ..TODAY }));
    assert_eq!(stage.feet().x, by_the_wall, "the edit moves it nowhere");
    stage.hold(full_stick(), 1);
    assert!(
        stage.feet().x <= by_the_wall - 0.19 && stage.feet().x >= by_the_wall - 0.25,
        "{}",
        stage.feet().x
    );
    assert!(on_the_ground(stage.feet()), "{}", stage.feet());
    stage.hold(full_stick(), 30);
    assert!((stage.feet().x - (by_the_wall - 0.2)).abs() < 0.03, "and stays off it: {}", stage.feet().x);
}

/// A taller capsule than the room it's in: no number in a document can send it through the floor or out
/// of the world, though it has nowhere right to be.
#[test]
fn a_taller_capsule_under_a_low_roof_stays_in_the_world() {
    let roof = solid(1, Vec3::new(-20.0, 2.0, -20.0), Vec3::new(20.0, 3.0, 20.0));
    let mut stage = Stage::on(vec![ground(), roof], slice(Some(TODAY)), &TODAY, Vec3::ZERO);
    stage.hold(full_stick(), 10);

    stage.tune(Some(ControllerProfile { height: 3.0, ..TODAY }));
    let (mut lowest, mut highest) = (f32::MAX, f32::MIN);
    for tick in 0..600 {
        stage.step(full_stick(), if tick % 40 == 0 { Buttons::JUMP } else { Buttons::NONE });
        assert!(stage.feet().is_finite(), "{}", stage.feet());
        lowest = lowest.min(stage.feet().y);
        highest = highest.max(stage.feet().y);
    }
    println!("under the roof, its feet were between {lowest} and {highest}");
    assert!(lowest > -2.0 && highest < 3.0, "{lowest} to {highest}");

    stage.tune(Some(TODAY));
    stage.hold(full_stick(), 120);
    assert!(on_the_ground(stage.feet()), "and with the right numbers it's standing again: {}", stage.feet());
    assert_eq!(stage.state().mode, GroundMode::Grounded);
}

// --- A party and a brain ---

fn member(index: usize, movement: ControllerProfile) -> MoveSet {
    let mut document = replay::party_document(index).expect("reads");
    document.fighter.as_mut().expect("a fighter").movement = Some(movement);
    document.compile().expect("compiles")
}

struct Party {
    sim: Simulation,
    members: [ActorId; 3],
    sets: [MoveSetId; 3],
}

impl Party {
    fn of(movements: [ControllerProfile; 3]) -> Self {
        let mut sim = Simulation::new(SimConfig { max_actors: 4, ..SimConfig::default() });
        let world = PhysicsWorld::new(vec![ground()], sim.body_capacity()).expect("builds");
        sim.set_physics_world(world).expect("nobody's in it yet");
        let controller = Some(sim.add_controller_profile(&TODAY).expect("a good profile"));
        let sets = [0, 1, 2].map(|index| sim.add_move_set(member(index, movements[index])));
        let members = sets.map(|set| {
            sim.spawn(ActorSpawn {
                position: Vec3::ZERO,
                yaw: 0.0,
                moves: Some(set),
                controller,
                ..ActorSpawn::default()
            })
            .expect("room")
        });
        sim.form_party(&members, PartyTuning { swap_cooldown: 10, rest: 10 }).expect("a good party");
        Self { sim, members, sets }
    }

    fn step(&mut self, stick: Stick, buttons: Buttons) {
        let mut actions = TickActions::NEUTRAL;
        actions.set(self.members[0], ActorActions { buttons, stick }).expect("room");
        self.sim.step(&actions);
    }

    fn on_field(&self) -> ActorId {
        self.sim.party().on_field(self.members[0])
    }

    /// How fast whoever's on the field is going, once it has had 40 ticks of a full stick.
    fn run_speed(&mut self) -> f32 {
        for _ in 0..40 {
            self.step(full_stick(), Buttons::NONE);
        }
        let velocity = self.sim.actor(self.on_field()).expect("alive").controller.state.velocity;
        Vec3::new(velocity.x, 0.0, velocity.z).length()
    }

    /// Swaps to the next member and plays out its assist.
    fn swap(&mut self) {
        let before = self.on_field();
        self.step(Stick::CENTER, Buttons::SWAP);
        assert_ne!(self.on_field(), before, "the swap fired");
        for _ in 0..90 {
            self.step(Stick::CENTER, Buttons::NONE);
        }
    }

    fn capsule(&self, actor: ActorId) -> Option<Shape> {
        self.sim.physics().body_shape(actor_body(actor))
    }
}

#[test]
fn each_member_of_a_party_moves_by_its_own_numbers_through_a_swap() {
    let movements = [
        ControllerProfile { run_speed: 4.0, ..TODAY },
        ControllerProfile { run_speed: 8.0, height: 2.0, ..TODAY },
        ControllerProfile { run_speed: 5.0, radius: 0.4, ..TODAY },
    ];
    let mut party = Party::of(movements);
    party.step(Stick::CENTER, Buttons::NONE);
    assert!(close(party.run_speed(), 4.0));

    party.swap();
    assert_eq!(party.on_field(), party.members[1]);
    assert!(close(party.run_speed(), 8.0));
    assert_eq!(party.capsule(party.members[1]), Some(Shape::Capsule { half_height: 0.7, radius: 0.3 }));
    assert_eq!(party.capsule(party.members[0]), None, "off the field, nobody's in the world");
    assert!(on_the_ground(party.sim.actor(party.members[1]).expect("alive").position));

    party.swap();
    assert_eq!(party.on_field(), party.members[2]);
    assert!(close(party.run_speed(), 5.0));
    assert_eq!(party.capsule(party.members[2]), Some(Shape::Capsule { half_height: 0.4, radius: 0.4 }));

    party.swap();
    assert_eq!(party.on_field(), party.members[0]);
    assert!(close(party.run_speed(), 4.0));
    assert_eq!(party.capsule(party.members[0]), Some(Shape::Capsule { half_height: 0.5, radius: 0.3 }));
}

/// An edit of who's on the field moves it now, and of who isn't waits for it to come on, numbers and
/// capsule alike, with nobody put in the world early.
#[test]
fn an_edit_reaches_a_member_off_the_field_as_it_comes_on() {
    let mut party = Party::of([TODAY; 3]);
    party.step(Stick::CENTER, Buttons::NONE);
    assert!(close(party.run_speed(), 6.0));

    let edited = member(1, ControllerProfile { run_speed: 8.0, height: 2.2, ..TODAY });
    assert_eq!(party.sim.tune_move_set(party.sets[1], edited), Some(Tuned::Replaced { revision: 1 }));
    assert_eq!(party.capsule(party.members[1]), None, "still off the field");
    assert!(close(party.run_speed(), 6.0), "and the one on the field runs as it did");

    party.swap();
    assert_eq!(party.capsule(party.members[1]), Some(Shape::Capsule { half_height: 0.8, radius: 0.3 }));
    assert!(close(party.run_speed(), 8.0));
    assert!(on_the_ground(party.sim.actor(party.members[1]).expect("alive").position));

    let edited = member(1, ControllerProfile { run_speed: 3.0, ..TODAY });
    party.sim.tune_move_set(party.sets[1], edited).expect("its set");
    assert_eq!(party.capsule(party.members[1]), Some(Shape::Capsule { half_height: 0.5, radius: 0.3 }));
    assert!(close(party.run_speed(), 3.0));
}

/// The warrior walks in on its brain's stick, so its speed is the document's numbers at its `approach`'s
/// tilt of 0.7, or 89/127 as a stick holds it: a little past halfway from the walk to the run. Its opponent
/// keeps its own, across an edit of either.
#[test]
fn a_brains_enemy_moves_by_its_own_numbers() {
    let warrior = |movement: ControllerProfile| {
        let mut document = MoveSetDocument::parse(WARRIOR).expect("parses");
        document.fighter.as_mut().expect("a fighter").movement = Some(movement);
        document.compile().expect("compiles")
    };
    let mut sim = Simulation::new(SimConfig { seed: 3, max_actors: 4, ..SimConfig::default() });
    let world = PhysicsWorld::new(vec![ground()], sim.body_capacity()).expect("builds");
    sim.set_physics_world(world).expect("nobody's in it yet");
    let controller = Some(sim.add_controller_profile(&TODAY).expect("a good profile"));
    let warrior_set = sim.add_move_set(warrior(ControllerProfile { run_speed: 3.0, ..TODAY }));
    sim.add_enemy(warrior_set, replay::warrior_enemy().expect("compiles").mind).expect("it fits");
    let player_set = sim.add_move_set(slice(Some(ControllerProfile { run_speed: 7.0, ..TODAY })));
    let enemy = sim
        .spawn(ActorSpawn {
            position: Vec3::ZERO,
            yaw: 0.0,
            moves: Some(warrior_set),
            controller,
            ..ActorSpawn::default()
        })
        .expect("room");
    let player = sim
        .spawn(ActorSpawn {
            position: Vec3::new(0.0, 0.0, 80.0),
            yaw: 0.0,
            moves: Some(player_set),
            controller,
            ..ActorSpawn::default()
        })
        .expect("room");
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.give_brain(enemy), "the warrior's set has a brain");

    let enemy_speed = |sim: &mut Simulation| {
        for _ in 0..20 {
            sim.step(&TickActions::NEUTRAL);
        }
        let velocity = sim.actor(enemy).expect("alive").controller.state.velocity;
        Vec3::new(velocity.x, 0.0, velocity.z).length()
    };
    let speed = enemy_speed(&mut sim);
    assert!((speed - 2.5).abs() < 0.01, "from its walk of 2 to its run of 3: {speed}");
    assert_eq!(sim.movement_of(enemy).expect("it has one").authored().run_speed, 3.0);
    assert_eq!(sim.movement_of(player).expect("it has one").authored().run_speed, 7.0);

    let edited = warrior(ControllerProfile { run_speed: 10.0, ..TODAY });
    assert_eq!(sim.tune_move_set(warrior_set, edited), Some(Tuned::Replaced { revision: 1 }));
    let speed = enemy_speed(&mut sim);
    assert!((speed - 6.0).abs() < 0.03, "from its walk of 2 to its run of 10: {speed}");
    assert_eq!(sim.movement_of(player).expect("it has one").authored().run_speed, 7.0);
}
