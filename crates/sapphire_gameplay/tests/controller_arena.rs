//! A character walks around the committed test arena, through the simulation, as a game would run it:
//! the document cooked, the world built from it, a stick and a jump button. Everything a player's feet
//! would notice holds on every tick, and each leg ends where the geometry says it should.
//!
//! The arena (`content/physics/test_arena.colliders.ron`): ground with its top at y = 0, invisible walls
//! at ±12, a 1 m crate turned 30° at (3, 2), a capsule pillar at (−4, 0), a ball at (0, −5), and a sheet
//! ramp rising 1 in 4 from x = 5 to x = 9, across z = −7 to −5.
//!
//! Each leg prints what it measured: `cargo test -p sapphire_gameplay --test controller_arena -- --nocapture`.

#![allow(clippy::print_stdout, reason = "the legs report what they measured")]

use sapphire_colliders::ColliderDocument;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, ControllerProfile, GroundMode, SimConfig, Simulation, Stick,
    TickActions, actor_body,
};
use sapphire_math::{Vec2, Vec3};
use sapphire_physics::{ColliderSet, Group, PhysicsWorld, Pose, QueryFilter, Shape};

const ARENA: &str = include_str!("../../../content/physics/test_arena.colliders.ron");

/// From the lower sphere's center to what it rests on.
const REACH: f32 = 0.31;
const RUN: f32 = 6.0;
const EDGE: f32 = 12.0;

struct Walker {
    sim: Simulation,
    id: ActorId,
    capsule: Shape,
    deepest: f32,
}

impl Walker {
    fn new(feet: Vec3) -> Self {
        let cooked = ColliderDocument::from_ron(ARENA)
            .expect("the document reads")
            .cook()
            .expect("it cooks")
            .to_cooked()
            .expect("it writes");
        let set = ColliderSet::read(&cooked).expect("the cooked set reads");
        let mut sim = Simulation::new(SimConfig { max_actors: 8, ..SimConfig::default() });
        let world = PhysicsWorld::from_cooked(set, sim.body_capacity()).expect("the world builds");
        sim.set_physics_world(world).expect("it has room");
        let profile = ControllerProfile::default();
        let capsule = profile.resolve().expect("a good profile").capsule();
        let profile = sim.add_controller_profile(&profile).expect("a good profile");
        let id = sim
            .spawn(ActorSpawn { position: feet, controller: Some(profile), ..ActorSpawn::default() })
            .expect("room in the pool");
        let mut walker = Self { sim, id, capsule, deepest: 0.0 };
        walker.tick(Vec2::ZERO, Buttons::NONE);
        walker
    }

    fn feet(&self) -> Vec3 {
        self.sim.actor(self.id).expect("alive").position
    }

    fn mode(&self) -> GroundMode {
        self.sim.actor(self.id).expect("alive").controller.state.mode
    }

    /// One tick, and what must hold on every one of them.
    fn tick(&mut self, stick: Vec2, buttons: Buttons) {
        let mut actions = TickActions::NEUTRAL;
        let stick = Stick::from_axes(stick.x, stick.y);
        actions.set(self.id, ActorActions { buttons, stick }).expect("room");
        self.sim.step(&actions);

        let view = self.sim.actor(self.id).expect("alive");
        let feet = view.position;
        assert!(feet.is_finite(), "{feet}");
        assert!(feet.y >= -2e-4, "under the ground: {feet}");
        let inside = EDGE - 0.3 + 1e-3;
        assert!(feet.x.abs() <= inside && feet.z.abs() <= inside, "past the arena's walls: {feet}");
        let state = view.controller.state;
        if state.mode == GroundMode::Grounded {
            let speed = Vec2::new(state.velocity.x, state.velocity.z).length();
            assert!(speed <= RUN + 1e-3, "{speed} m/s");
            assert_eq!(state.velocity.y, 0.0);
        }
        // Never in anything, by more than a sweep's rounding.
        let pose = self.sim.physics().body_pose(actor_body(self.id)).expect("its body");
        let solid = QueryFilter::new(Group::World | Group::CharacterBlocker);
        if let Some(found) = self.sim.physics().penetration(&self.capsule, Pose::at(pose.position), &solid) {
            self.deepest = self.deepest.max(found.depth);
            assert!(found.depth < 2e-3, "{} m inside something at {feet}", found.depth);
        }
    }

    fn ticks(&mut self, stick: Vec2, ticks: u32) {
        for _ in 0..ticks {
            self.tick(stick, Buttons::NONE);
        }
    }

    /// Runs straight at a point on the ground plane, as far as `limit` ticks, and stops there.
    fn run_to(&mut self, x: f32, z: f32, limit: u32) {
        for _ in 0..limit {
            let feet = self.feet();
            let toward = Vec2::new(x - feet.x, z - feet.z);
            if toward.length() < 0.15 {
                break;
            }
            self.tick(toward.normalize(), Buttons::NONE);
        }
        self.ticks(Vec2::ZERO, 10);
    }
}

#[test]
fn a_character_walks_the_arena() {
    let mut walker = Walker::new(Vec3::ZERO);
    assert_eq!(walker.mode(), GroundMode::Grounded);
    assert!(walker.feet().y.abs() < 2e-4);
    println!("spawned: first tick at {}", walker.feet());

    // Up the ramp, through its middle. On it, the feet stand over its surface by the round foot's
    // reach: 0.31 / cos θ − 0.31, with tan θ = 1/4.
    walker.run_to(3.0, -6.0, 200);
    let lift = REACH * ((1.0 + 1.0 / 16.0_f32).sqrt() - 1.0);
    let mut checked = 0;
    let mut left_at = None;
    for _ in 0..240 {
        walker.tick(Vec2::X, Buttons::NONE);
        let feet = walker.feet();
        if feet.x > 5.5 && feet.x < 8.5 {
            let surface = (feet.x - 5.0) / 4.0;
            assert_eq!(walker.mode(), GroundMode::Grounded, "on the ramp at {feet}");
            assert!((feet.y - (surface + lift)).abs() < 2e-3, "{feet} over {surface} + {lift}");
            checked += 1;
        }
        if walker.mode() != GroundMode::Grounded && left_at.is_none() {
            left_at = Some(feet);
        }
        if feet.x >= EDGE - REACH - 1e-2 {
            break;
        }
    }
    let left = left_at.expect("the ramp's end is a ledge");
    println!("up the ramp: {checked} ticks on it on the surface, left its top at {left}");
    assert!(checked > 20, "{checked}");
    assert!(left.x > 9.0 && left.x < 9.5, "the ramp's top is at x = 9: {left}");

    // Off the top, down, and on into the wall at x = 12, a skin away.
    walker.ticks(Vec2::X, 60);
    println!("into the east wall: {}", walker.feet());
    assert_eq!(walker.mode(), GroundMode::Grounded);
    assert!((walker.feet().x - (EDGE - REACH)).abs() < 1e-3, "{}", walker.feet());
    assert!(walker.feet().y.abs() < 2e-4, "{}", walker.feet());

    // Along it, into a corner of two invisible walls: the skin's gap to one, and at most that to the
    // other, which a shallow slide along it doesn't hold.
    walker.ticks(Vec2::new(0.7, 0.7), 300);
    println!("into the corner: {}", walker.feet());
    for axis in [walker.feet().x, walker.feet().z] {
        assert!((EDGE - REACH - 1e-3..EDGE - 0.3).contains(&axis), "{}", walker.feet());
    }
    let settled = walker.feet();
    walker.ticks(Vec2::new(0.7, 0.7), 120);
    assert!((walker.feet() - settled).length() < 1e-5, "{} then {}", settled, walker.feet());

    // Straight at the crate along z = 2, too tall to step on: round it and on.
    walker.run_to(0.0, 2.0, 400);
    let mut y_high = 0.0_f32;
    for _ in 0..120 {
        walker.tick(Vec2::X, Buttons::NONE);
        y_high = y_high.max(walker.feet().y);
    }
    println!("round the crate: reached {}, feet never over {y_high}", walker.feet());
    assert!(walker.feet().x > 4.0, "got round it: {}", walker.feet());
    assert!(y_high < 2e-4, "{y_high}");

    // A running jump onto it. The face along z = 2 is at x = 3 − 0.5 / cos 30° = 2.423, so the capsule's
    // axis meets it at 2.113. Taken at x = 0.3, the feet are over 1 m from t = 0.217 s to 0.42 s, at
    // 6 m/s from x = 1.6 to 2.8: clear of the face, and landing on top.
    walker.run_to(-2.0, 2.0, 400);
    let mut landed_on_top = false;
    let mut jumped = false;
    for _ in 0..240 {
        let x = walker.feet().x;
        let press = !jumped && x >= 0.3;
        jumped |= press;
        let stick = if x < 2.95 { Vec2::X } else { Vec2::ZERO };
        walker.tick(stick, if press { Buttons::JUMP } else { Buttons::NONE });
        if walker.sim.actor(walker.id).expect("alive").controller.state.events.landed && walker.feet().y > 0.5
        {
            landed_on_top = true;
            break;
        }
    }
    println!("onto the crate: {landed_on_top}, at {}", walker.feet());
    assert!(landed_on_top, "at {}", walker.feet());
    walker.ticks(Vec2::ZERO, 30);
    assert!((walker.feet().y - 1.0).abs() < 2e-3, "{}", walker.feet());
    assert_eq!(walker.mode(), GroundMode::Grounded);

    // And off its far side, onto the ground.
    walker.ticks(Vec2::X, 60);
    println!("off the crate: {}", walker.feet());
    assert!(walker.feet().y.abs() < 2e-4, "{}", walker.feet());
    assert!(walker.feet().x > 4.0, "{}", walker.feet());

    // At the pillar, its foot too round and too tall to step on: round it, not up it.
    walker.run_to(-7.0, 0.1, 400);
    let mut y_high = 0.0_f32;
    for _ in 0..120 {
        walker.tick(Vec2::X, Buttons::NONE);
        y_high = y_high.max(walker.feet().y);
    }
    println!("round the pillar: reached {}, feet never over {y_high}", walker.feet());
    assert!(walker.feet().x > -3.0, "got round it: {}", walker.feet());
    assert!(y_high < 2e-3, "{y_high}");

    // And round the ball, whose lower half overhangs the capsule's foot.
    walker.run_to(0.05, -9.0, 400);
    let mut y_high = 0.0_f32;
    for _ in 0..120 {
        walker.tick(Vec2::Y, Buttons::NONE);
        y_high = y_high.max(walker.feet().y);
    }
    println!("round the ball: reached {}, feet never over {y_high}", walker.feet());
    assert!(walker.feet().z > -3.5, "got round it: {}", walker.feet());
    assert!(y_high < 2e-3, "{y_high}");

    // On the ramp again, stopped halfway: it stays, to the bit.
    walker.run_to(7.0, -6.0, 400);
    let held = walker.feet();
    assert!((held.y - ((held.x - 5.0) / 4.0 + lift)).abs() < 2e-3, "{held}");
    walker.ticks(Vec2::ZERO, 600);
    assert_eq!(walker.feet(), held, "standing on the ramp");

    let stats = walker.sim.stats();
    println!(
        "{} ticks; deepest overlap {} m; physics in the last step: {:?}",
        stats.steps, walker.deepest, stats.physics
    );
    assert_eq!(stats.controllers_refused, 0);
}

/// The arena's ID is a name's place in the set, so which collider stops the walker is plain data. The
/// walker's whole run is one checksum per tick, twice over.
#[test]
fn the_same_walk_gives_the_same_checksums() {
    let run = || {
        let mut walker = Walker::new(Vec3::new(0.3, 0.1, 0.2));
        let mut checksums = Vec::new();
        for tick in 0..900_u32 {
            let t = tick as f32;
            let stick = Vec2::new(((t * 0.013).fract() - 0.5) * 2.0, ((t * 0.007).fract() - 0.5) * 2.0);
            let jump = tick % 97 == 13;
            walker.tick(stick, if jump { Buttons::JUMP } else { Buttons::NONE });
            checksums.push(walker.sim.checksum());
        }
        checksums
    };
    let (a, b) = (run(), run());
    assert_eq!(a, b);
}
