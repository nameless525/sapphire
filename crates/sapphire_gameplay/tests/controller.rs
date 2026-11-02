//! Everything a player's feet would notice, each against an answer worked out by hand from the
//! geometry, through `step_controller` with no ECS around it. The capsule is the default profile's:
//! 1.6 m tall, radius 0.3, skin 0.01, so its lower sphere's center rests 0.31 m over flat ground.
//!
//! Each table prints what it measured: `cargo test -p sapphire_gameplay --test controller -- --nocapture`.

#![allow(clippy::print_stdout, reason = "these tables report what they measured")]

use sapphire_gameplay::{
    ActorTransform, ControllerDrive, ControllerEvents, ControllerInput, ControllerProfile, ControllerState,
    GroundMode, MovementMode, ResolvedProfile, step_controller,
};
use sapphire_math::{Quat, Vec2, Vec3, det};
use sapphire_physics::{
    BodyDesc, BodyId, ColliderId, CollisionGroups, Group, GroupMask, PhysicsWorld, Pose, Shape,
    StaticCollider, StaticShape,
};
use sapphire_time::SimDuration;

const BODY: BodyId = BodyId(1);
const TICK: SimDuration = SimDuration::from_ticks(1);
const DT: f32 = 1.0 / 60.0;
const RIGHT: Vec2 = Vec2::X;

// The default profile, by hand.
const GRAVITY: f32 = 22.0;
const JUMP_SPEED: f32 = 7.0;
const RUN: f32 = 6.0;
/// From the lower sphere's center to a surface it rests on, along the surface's normal.
const REACH: f32 = 0.31;
/// The capsule's height plus the skin under its feet: from feet to the top of the capsule.
const FEET_TO_TOP: f32 = 1.61;
const STANCE: f32 = 0.15;

// --- Geometry ---

/// A box between two corners, its ID from a counter nothing else depends on.
fn solid(id: u32, min: Vec3, max: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents: (max - min) / 2.0 }),
        pose: Pose::at((min + max) / 2.0),
        memberships: Group::World.mask(),
    }
}

/// Flat ground with its top at y = 0, 400 m across.
fn floor(id: u32) -> StaticCollider {
    solid(id, Vec3::new(-200.0, -1.0, -200.0), Vec3::new(200.0, 0.0, 200.0))
}

/// A two-sided quad from its corners.
fn quad(id: u32, corners: [Vec3; 4]) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Mesh { vertices: corners.to_vec(), indices: vec![[0, 2, 1], [0, 3, 2]] },
        pose: Pose::IDENTITY,
        memberships: Group::World.mask(),
    }
}

/// A slope rising along +x from `(x, y)` at `degrees`, over `run` metres of x, 40 m wide in z.
fn slope(id: u32, x: f32, y: f32, degrees: f32, run: f32) -> StaticCollider {
    let rise = run * det::tan(degrees.to_radians());
    quad(
        id,
        [
            Vec3::new(x, y, -20.0),
            Vec3::new(x + run, y + rise, -20.0),
            Vec3::new(x + run, y + rise, 20.0),
            Vec3::new(x, y, 20.0),
        ],
    )
}

/// Where the feet rest with the capsule's axis at `x` on a slope through `(x0, y0)` rising along +x.
fn feet_on_slope(x: f32, x0: f32, y0: f32, degrees: f32) -> f32 {
    let radians = degrees.to_radians();
    y0 + (x - x0) * det::tan(radians) + REACH / det::cos(radians) - REACH
}

// --- A character by itself ---

struct Rig {
    world: PhysicsWorld,
    profile: ResolvedProfile,
    transform: ActorTransform,
    state: ControllerState,
}

impl Rig {
    fn new(statics: Vec<StaticCollider>, feet: Vec3) -> Self {
        Self::with_profile(statics, feet, ControllerProfile::default())
    }

    fn with_profile(statics: Vec<StaticCollider>, feet: Vec3, profile: ControllerProfile) -> Self {
        let profile = profile.resolve().expect("a good profile");
        let mut world = PhysicsWorld::new(statics, 8).expect("the colliders build");
        world.add_body(profile.body(BODY, feet, 0.0)).expect("it fits");
        Self {
            world,
            profile,
            transform: ActorTransform { position: feet, yaw: 0.0 },
            state: ControllerState::default(),
        }
    }

    fn input(&mut self, input: ControllerInput) {
        step_controller(
            &mut self.world,
            BODY,
            &mut self.transform,
            &mut self.state,
            &self.profile,
            &input,
            TICK,
        )
        .expect("the body is there");
    }

    fn stick(&mut self, stick: Vec2) {
        self.input(ControllerInput { stick, ..ControllerInput::default() });
    }

    fn sticks(&mut self, stick: Vec2, ticks: u32) {
        for _ in 0..ticks {
            self.stick(stick);
        }
    }

    fn jump(&mut self, stick: Vec2) {
        self.input(ControllerInput { stick, jump: true, ..ControllerInput::default() });
    }

    fn drive(&mut self, drive: ControllerDrive) {
        self.input(ControllerInput { drive, ..ControllerInput::default() });
    }

    fn feet(&self) -> Vec3 {
        self.transform.position
    }

    fn mode(&self) -> GroundMode {
        self.state.mode
    }

    /// A first tick to find the ground, which is what a spawn does.
    fn settled(mut self) -> Self {
        self.stick(Vec2::ZERO);
        self
    }

    /// Runs `ticks` with `stick`, and returns how far the feet ranged over the last `tail` of them
    /// along each axis: what jitter would show as.
    fn jitter(&mut self, stick: Vec2, ticks: u32, tail: u32) -> f32 {
        self.sticks(stick, ticks - tail);
        let (mut low, mut high) = (self.feet(), self.feet());
        for _ in 0..tail {
            self.stick(stick);
            low = low.min(self.feet());
            high = high.max(self.feet());
        }
        (high - low).max_element()
    }
}

#[track_caller]
fn near(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!((actual - expected).abs() <= tolerance, "{what}: {actual} is not {expected} ± {tolerance}");
}

// --- Locomotion ---

/// Pillar 1: a press to visibly moving in at most 3 ticks. Acceleration × dt² / 2 × k²: 90 m/s² gives
/// 1.25 cm × k², until full speed at tick 4.
#[test]
fn a_run_start_moves_within_three_ticks() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    for tick in 1..=6 {
        rig.stick(RIGHT);
        let k = tick as f32;
        let expected = if tick <= 4 { 0.0125 * k * k } else { 0.2 + 0.1 * (k - 4.0) };
        println!("run start, tick {tick}: {:.4} m, {:.2} m/s", rig.feet().x, rig.state.velocity.x);
        near(rig.feet().x, expected, 1e-5, "distance");
    }
    assert_eq!(rig.state.velocity, Vec3::new(RUN, 0.0, 0.0));
    assert_eq!(rig.mode(), GroundMode::Grounded);
}

#[test]
fn the_stick_sets_the_speed() {
    let rows = [
        (Vec2::new(0.25, 0.0), 1.0),
        (Vec2::new(0.5, 0.0), 2.0),
        (Vec2::new(0.7, 0.0), 4.0),
        (Vec2::new(0.9, 0.0), 6.0),
        (Vec2::new(1.0, 0.0), 6.0),
        (Vec2::new(0.0, -1.0), 6.0),
        // Both axes full is past 1, and counts as 1.
        (Vec2::new(1.0, 1.0), 6.0),
        // A full tilt whose axes each rounded up: as `Stick::to_vec2` can give, a little past 1.
        (Vec2::new(90.0 / 127.0, 90.0 / 127.0), 6.0),
    ];
    for (stick, speed) in rows {
        let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
        rig.sticks(stick, 30);
        near(rig.state.velocity.length(), speed, 1e-5, "speed");
        let heading = Vec2::new(rig.state.velocity.x, rig.state.velocity.z).normalize();
        assert!((heading - stick.normalize()).length() < 1e-5, "{heading} for {stick}");
    }
}

/// Deceleration: 6 m/s at 60 m/s² stops in 6 ticks, over v² / 2a = 0.3 m. A reversal takes the larger
/// rate, 90: 12 m/s of change in 8 ticks.
#[test]
fn stopping_and_reversing_take_their_authored_rates() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 30);
    let from = rig.feet().x;
    rig.sticks(Vec2::ZERO, 5);
    assert!(rig.state.velocity.x > 0.0);
    rig.stick(Vec2::ZERO);
    assert_eq!(rig.state.velocity, Vec3::ZERO);
    println!("stopped in 6 ticks, {:.4} m", rig.feet().x - from);
    near(rig.feet().x - from, 0.3, 1e-4, "stopping distance");

    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 30);
    rig.sticks(-RIGHT, 7);
    assert!(rig.state.velocity.x > -RUN);
    rig.stick(-RIGHT);
    assert_eq!(rig.state.velocity, Vec3::new(-RUN, 0.0, 0.0));
}

/// 1080° a second is 18° a tick, so a half turn takes 10 ticks. The motion doesn't wait for it: the
/// body is already moving the new way from the first.
#[test]
fn facing_follows_the_stick_at_the_turn_rate() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    assert_eq!(rig.transform.yaw, 0.0);
    // 0 faces +Z, and the stick's y is +Z: straight back is a half turn.
    let back = Vec2::new(0.0, -1.0);
    rig.stick(back);
    assert!(rig.feet().z < 0.0, "already moving");
    near(rig.transform.yaw.abs(), 18.0_f32.to_radians(), 1e-5, "one tick's turn");
    rig.sticks(back, 8);
    near(rig.transform.yaw.abs(), 162.0_f32.to_radians(), 1e-4, "nine ticks' turn");
    rig.stick(back);
    near(rig.transform.yaw.abs(), std::f32::consts::PI, 1e-4, "ten ticks' turn");
    let turned = rig.world.body_pose(BODY).expect("there").rotation;
    let expected = det::quat_from_rotation_y(rig.transform.yaw);
    assert_eq!(turned, expected, "the body turned with it, through its pose");

    // A stick to the right is yaw +90°, the way sapphire_math's yaw runs.
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 20);
    near(rig.transform.yaw, std::f32::consts::FRAC_PI_2, 1e-5, "facing +X");
}

// --- Jumps, gravity, landings ---

/// y = v t − g t² / 2 at every tick, with the tick's average velocity making it exact. It would be
/// back at 2 v / g = 0.636 s, tick 38.2, and at tick 38 it's 2.1 cm up, within the 3 cm over its rest
/// that counts as touching, so it lands there.
#[test]
fn a_jump_follows_its_arc() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.jump(Vec2::ZERO);
    assert!(rig.state.events.jumped);
    assert_eq!(rig.mode(), GroundMode::Airborne);
    let mut highest = 0.0_f32;
    let mut landed_at = None;
    for tick in 1_u32..=60 {
        let t = tick as f32 * DT;
        if tick > 1 {
            rig.stick(Vec2::ZERO);
        }
        highest = highest.max(rig.feet().y);
        if rig.state.events.landed {
            landed_at = Some((tick, rig.state.events.landing_speed));
            break;
        }
        near(rig.feet().y, JUMP_SPEED * t - GRAVITY * t * t / 2.0, 1e-4, "the arc");
    }
    let apex = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
    // The arc's top falls between two ticks by at most g dt² / 8.
    assert!(highest <= apex + 1e-5 && highest >= apex - 0.0008, "{highest} against {apex}");
    let (tick, speed) = landed_at.expect("it came down");
    println!("jump: apex {highest:.4} m (v²/2g = {apex:.4}), landed at tick {tick} at {speed:.2} m/s");
    assert_eq!(tick, 38);
    near(speed, GRAVITY * 38.0 * DT - JUMP_SPEED, 1e-3, "landing speed");
    assert_eq!(rig.mode(), GroundMode::Grounded);
    near(rig.feet().y, 0.0, 1e-4, "on the ground");
}

/// Only ground jumps: a press in the air does nothing, and jumps again only after landing.
#[test]
fn a_jump_needs_ground() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.jump(Vec2::ZERO);
    rig.stick(Vec2::ZERO);
    let rising = rig.state.velocity.y;
    rig.jump(Vec2::ZERO);
    assert!(!rig.state.events.jumped);
    assert!(rig.state.velocity.y < rising, "still slowing");
    rig.sticks(Vec2::ZERO, 60);
    assert_eq!(rig.mode(), GroundMode::Grounded);
    rig.jump(Vec2::ZERO);
    assert!(rig.state.events.jumped);
}

/// The stick steers in the air at the air's rate, and a neutral one keeps the momentum.
#[test]
fn the_air_keeps_momentum_and_takes_steering() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 10);
    rig.jump(Vec2::ZERO);
    let speed = rig.state.velocity.x;
    assert_eq!(speed, RUN, "momentum carried into the jump");
    rig.sticks(Vec2::ZERO, 10);
    assert_eq!(rig.state.velocity.x, RUN);
    // 18 m/s² is 0.3 m/s a tick, turning back.
    rig.sticks(-RIGHT, 10);
    near(rig.state.velocity.x, RUN - 3.0, 1e-4, "steering");
}

/// A capsule top rises to the ceiling less its skin, and stops dead: feet at 2.6 − 1.61 − 0.01 = 0.98,
/// reached at about 12.5 ticks, then down from rest, sqrt(2 × 0.98 / 22) = 0.298 s more. An open sky
/// would have it in the air until tick 38.
#[test]
fn a_low_ceiling_cuts_a_jump() {
    let ceiling = solid(2, Vec3::new(-20.0, 2.6, -20.0), Vec3::new(20.0, 4.0, 20.0));
    let mut rig = Rig::new(vec![floor(1), ceiling], Vec3::ZERO).settled();
    rig.jump(Vec2::ZERO);
    let mut highest = rig.feet().y;
    let mut stopped_rising = None;
    let mut landed = None;
    for tick in 2..=60 {
        rig.stick(Vec2::ZERO);
        highest = highest.max(rig.feet().y);
        if stopped_rising.is_none() && rig.state.velocity.y <= 0.0 {
            stopped_rising = Some(tick);
        }
        if rig.state.events.landed {
            landed = Some(tick);
            break;
        }
    }
    let stopped = stopped_rising.expect("it stopped rising");
    let landed = landed.expect("it came down");
    println!("low ceiling: feet reached {highest:.4} m, stopped at tick {stopped}, landed at tick {landed}");
    near(highest, 2.6 - FEET_TO_TOP - 0.01, 1e-3, "the highest the feet got");
    assert!((12..=14).contains(&stopped), "stopped at tick {stopped}");
    assert!((30..=33).contains(&landed), "landed at tick {landed}");
}

// --- Slopes ---

fn slope_scene(degrees: f32) -> Vec<StaticCollider> {
    vec![floor(1), slope(2, 0.0, 0.0, degrees, 60.0)]
}

/// Up: at or under the limit, speed over the ground is the authored speed. Over it, the slope stops a
/// walker as a wall does: x = −0.31 tan(θ / 2), where the lower sphere's skin touches the plane.
#[test]
fn slopes_up_at_under_and_over_the_limit() {
    for degrees in [10.0, 30.0, 49.0, 49.9, 50.0, 50.5, 51.0, 60.0, 75.0] {
        let mut rig = Rig::new(slope_scene(degrees), Vec3::new(-2.0, 0.0, 0.0)).settled();
        if degrees <= 50.0 {
            // Past the foot of the slope and well up to speed, then a whole second.
            let mut grounded = true;
            for _ in 0..60 {
                rig.stick(RIGHT);
                grounded &= rig.mode() == GroundMode::Grounded;
            }
            let before = rig.feet();
            for _ in 0..30 {
                rig.stick(RIGHT);
                grounded &= rig.mode() == GroundMode::Grounded;
            }
            let moved = rig.feet() - before;
            let (sin, cos) = det::sin_cos(degrees.to_radians());
            println!(
                "up {degrees}°: {:.3} m/s over the ground, rise {:.3} against {:.3}",
                moved.length() * 2.0,
                moved.y,
                RUN * sin / 2.0
            );
            assert!(grounded, "{degrees}° stays grounded on the way up");
            near(moved.y, RUN * sin / 2.0, 0.01, "rise");
            near(moved.x, RUN * cos / 2.0, 0.01, "run");
            near(rig.feet().y, feet_on_slope(rig.feet().x, 0.0, 0.0, degrees), 1e-3, "rest");
            let n = rig.state.ground_normal;
            near(n.y, cos, 1e-4, "the ground's normal");
        } else {
            let wobble = rig.jitter(RIGHT, 120, 30);
            let expected = -REACH * det::tan((degrees / 2.0).to_radians());
            println!(
                "up {degrees}°: stopped at x = {:.4} (hand: {expected:.4}), wobble {wobble}",
                rig.feet().x
            );
            near(rig.feet().x, expected, 1e-3, "stopped at the foot");
            near(rig.feet().y, 0.0, 1e-4, "still on the flat");
            assert_eq!(rig.mode(), GroundMode::Grounded);
            assert!(wobble < 1e-4, "{degrees}° wobbles {wobble}");
        }
    }
}

/// Down: grounded on every tick, with the height never going back up: no bouncing. The plateau's edge
/// at x = 0 is where the slope starts down.
#[test]
fn slopes_down_under_and_at_the_limit_never_bounce() {
    for degrees in [10.0_f32, 30.0, 49.0, 49.9, 50.0] {
        for stick in [RIGHT, Vec2::new(0.5, 0.0)] {
            let run = 12.0;
            let height = run * det::tan(degrees.to_radians());
            let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, height, 20.0));
            // Falling along +x, from the plateau's edge to the floor.
            let down = quad(
                3,
                [
                    Vec3::new(0.0, height, -20.0),
                    Vec3::new(0.0, height, 20.0),
                    Vec3::new(run, 0.0, 20.0),
                    Vec3::new(run, 0.0, -20.0),
                ],
            );
            let mut rig = Rig::new(vec![floor(1), plateau, down], Vec3::new(-1.0, height, 0.0)).settled();
            let mut last = rig.feet().y;
            let mut biggest_pop = 0.0_f32;
            let mut ticks = 0;
            while rig.feet().x < run - 1.0 {
                rig.stick(stick);
                ticks += 1;
                assert!(ticks < 5_000, "{degrees}° never got down: {}", rig.feet());
                assert_eq!(
                    rig.mode(),
                    GroundMode::Grounded,
                    "{degrees}° left the ground at x = {}",
                    rig.feet().x
                );
                assert!(rig.feet().y <= last + 1e-4, "{degrees}° rose from {last} to {}", rig.feet().y);
                last = rig.feet().y;
                biggest_pop = biggest_pop.max(-rig.state.events.height_pop);
                assert!(ticks < 2000, "{degrees}° never got down");
            }
            // Partway down the slope, speed over the ground is the authored speed.
            let before = rig.feet();
            rig.sticks(stick, 6);
            let speed = (rig.feet() - before).length() * 10.0;
            let expected = stick.length() * if stick.x == 1.0 { RUN } else { 4.0 };
            println!(
                "down {degrees}° at {expected}: {speed:.3} m/s over the ground, {ticks} ticks, largest drop {biggest_pop:.4}"
            );
            near(speed, expected, 0.05 * expected.max(1.0), "speed");
        }
    }
}

/// Over the limit, a walker coming over the top slides: gravity along the slope, g sin θ, to the
/// capped speed, and it lands on the flat at the bottom. Pressing up the slope doesn't help.
#[test]
fn slopes_over_the_limit_slide() {
    for degrees in [51.0_f32, 60.0, 75.0] {
        let run = 4.0;
        let tan = det::tan(degrees.to_radians());
        let height = run * tan;
        let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, height, 20.0));
        let down = quad(
            3,
            [
                Vec3::new(0.0, height, -20.0),
                Vec3::new(0.0, height, 20.0),
                Vec3::new(run, 0.0, 20.0),
                Vec3::new(run, 0.0, -20.0),
            ],
        );
        let statics = vec![floor(1), plateau, down];

        let mut rig = Rig::new(statics.clone(), Vec3::new(-0.5, height, 0.0)).settled();
        let mut slid = false;
        for _ in 0..240 {
            rig.stick(RIGHT);
            slid |= rig.mode() == GroundMode::Sliding;
            if rig.mode() == GroundMode::Grounded && rig.feet().y < height / 2.0 {
                break;
            }
        }
        assert!(slid, "{degrees}° should have slid");
        assert_eq!(rig.mode(), GroundMode::Grounded, "{degrees}° lands on the flat");
        near(rig.feet().y, 0.0, 1e-3, "at the bottom");

        // From rest on the slope, partway down, with the stick pressed back up it: s = g sin θ t² / 2.
        let (sin, cos) = det::sin_cos(degrees.to_radians());
        // On the slope at x = 1: its plane falls along +x from (0, height), its normal (sin, cos).
        let foot = Vec3::new(1.0, height - tan, 0.0) + Vec3::new(sin, cos, 0.0) * REACH;
        let mut rig = Rig::new(statics, foot - Vec3::Y * REACH);
        let mut start = None;
        let mut ticks = 0;
        let mut distance = 0.0;
        for _ in 0..60 {
            rig.stick(-RIGHT);
            if rig.mode() == GroundMode::Sliding && start.is_none() {
                start = Some((rig.feet(), rig.state.velocity.length()));
            }
            if let Some((from, _)) = start {
                ticks += 1;
                distance = (rig.feet() - from).length();
                if ticks == 12 {
                    break;
                }
            }
        }
        let (_, speed0) = start.expect("it slid");
        // After the tick that found the slope, 11 more ticks, from that tick's speed.
        let t = 11.0 * DT;
        let expected = speed0 * t + GRAVITY * sin * t * t / 2.0;
        println!("slide {degrees}°: {distance:.4} m in {t:.3} s against {expected:.4}");
        near(distance, expected, 0.03 * expected, "gravity along the slope");
        assert!(rig.state.velocity.x > 0.0, "the stick can't send it up");
    }
}

/// A run over flat ground doesn't follow the rounding in each tick's cast for the floor: what's left
/// of a normal's rounding in the motion may lift the feet by a few µm, and they come back only once
/// that passes 0.1 mm, so presentation almost never sees a pop to smooth.
#[test]
fn a_run_over_flat_ground_ignores_rounding() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::new(-30.0, 0.0, 3.0)).settled();
    let mut highest = 0.0_f32;
    let mut pops = 0;
    for tick in 0..600 {
        rig.stick(if tick % 100 < 50 { RIGHT } else { Vec2::new(0.7, 0.7) });
        highest = highest.max(rig.feet().y.abs());
        pops += usize::from(rig.state.events.height_pop != 0.0);
    }
    println!(
        "a run over flat ground: 600 ticks to {}, height off by at most {highest}, {pops} pops",
        rig.feet()
    );
    assert!(highest < 2e-4, "{highest}");
    assert!(pops < 60, "{pops} ticks of 600 moved the feet for rounding");
}

/// Standing still on a slope doesn't drift: the exact bits, over 1,000 ticks.
#[test]
fn standing_on_a_slope_doesnt_drift_in_1000_ticks() {
    for degrees in [0.1_f32, 10.0, 30.0, 45.0, 49.9, 50.0] {
        let x = 2.0;
        let feet = Vec3::new(x, feet_on_slope(x, 0.0, 0.0, degrees) + 0.05, 0.0);
        let mut rig = Rig::new(slope_scene(degrees), feet);
        rig.sticks(Vec2::ZERO, 10);
        assert_eq!(rig.mode(), GroundMode::Grounded, "{degrees}°");
        let settled = rig.feet();
        for _ in 0..1000 {
            rig.stick(Vec2::ZERO);
            assert_eq!(
                rig.feet().to_array().map(f32::to_bits),
                settled.to_array().map(f32::to_bits),
                "{degrees}°"
            );
            assert_eq!(rig.mode(), GroundMode::Grounded);
        }
        println!("still on {degrees}°: 1,000 ticks, moved 0, at y = {:.4}", settled.y);
        near(settled.y, feet_on_slope(settled.x, 0.0, 0.0, degrees), 1e-3, "rest height");
    }
}

// --- Steps ---

/// A step's face at x = 1, running from x = 0. Under the limit it's climbed to exactly its height at
/// any speed, the height never going down on the way. Over it, the feet stop where the capsule's skin
/// meets the edge: 1 − sqrt(0.31² − (0.31 − h)²), or 1 − 0.31 once the edge is level with the sphere's
/// center or above.
#[test]
fn steps_just_under_and_just_over_the_limit() {
    let stop_x = |h: f32| {
        let below = (REACH - h).max(0.0);
        1.0 - (REACH * REACH - below * below).sqrt()
    };
    let rows = [
        (0.05, 1.0, true, "rolled by the round foot"),
        (0.10, 1.0, true, "rolled by the round foot"),
        (0.13, 1.0, true, "a step-up"),
        (0.20, 1.0, true, "a step-up"),
        (0.29, 1.0, true, "just under"),
        (0.299, 1.0, true, "just under"),
        (0.29, 0.5, true, "at a walk"),
        (0.29, 0.1, true, "at a creep"),
        (0.20, 0.1, true, "at a creep"),
        (0.05, 0.1, true, "rolled at a creep"),
        (0.301, 1.0, false, "just over"),
        (0.31, 1.0, false, "just over"),
        (0.31, 0.5, false, "at a walk"),
        (0.35, 1.0, false, "well over"),
        (0.80, 1.0, false, "a wall"),
    ];
    for (height, tilt, climbs, what) in rows {
        let step = solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(40.0, height, 20.0));
        let mut rig = Rig::new(vec![floor(1), step], Vec3::ZERO).settled();
        let stick = Vec2::new(tilt, 0.0);
        let ticks = (240.0 / tilt) as u32;
        let mut last = 0.0_f32;
        let mut stepped = false;
        for _ in 0..ticks {
            rig.stick(stick);
            assert_eq!(rig.mode(), GroundMode::Grounded, "{height} m, {what}");
            assert!(rig.feet().y >= last - 1e-3, "{height} m, {what}: fell from {last} to {}", rig.feet().y);
            last = rig.feet().y;
            stepped |= rig.state.events.stepped_up;
        }
        println!(
            "step {height} m, {what}: feet at ({:.4}, {:.4}), step-up used: {stepped}",
            rig.feet().x,
            rig.feet().y,
        );
        if climbs {
            near(rig.feet().y, height, 1e-3, what);
            assert!(rig.feet().x > 1.0, "{height} m, {what}: on top at x = {}", rig.feet().x);
        } else {
            near(rig.feet().y, 0.0, 1e-4, what);
            near(rig.feet().x, stop_x(height), 2e-3, what);
            let wobble = rig.jitter(stick, 60, 30);
            assert!(wobble < 1e-5, "{height} m, {what} wobbles {wobble}");
        }
    }
}

/// Stairs: 0.2 m rises on 0.3 m treads, which a walker takes without ever leaving the ground, and comes
/// down the same way, the feet following it.
#[test]
fn a_flight_of_stairs_goes_up_and_down() {
    let mut statics = vec![floor(1)];
    for tread in 0..8 {
        let x = 1.0 + tread as f32 * 0.3;
        let y = 0.2 * (tread + 1) as f32;
        statics.push(solid(2 + tread, Vec3::new(x, -1.0, -5.0), Vec3::new(x + 0.3, y, 5.0)));
    }
    // A landing at the top, and open air beyond it: down is back the way it came.
    statics.push(solid(20, Vec3::new(3.4, -1.0, -5.0), Vec3::new(12.0, 1.6, 5.0)));
    let mut rig = Rig::new(statics, Vec3::ZERO).settled();
    let mut last = 0.0_f32;
    for _ in 0..120 {
        rig.stick(RIGHT);
        assert_eq!(rig.mode(), GroundMode::Grounded);
        assert!(rig.feet().y >= last - 1e-3, "fell from {last} to {} on the way up", rig.feet().y);
        last = rig.feet().y;
    }
    near(rig.feet().y, 1.6, 1e-3, "at the top");
    let mut last = rig.feet().y;
    for _ in 0..200 {
        rig.stick(-RIGHT);
        assert_eq!(rig.mode(), GroundMode::Grounded, "at x = {}", rig.feet().x);
        assert!(rig.feet().y <= last + 1e-3, "rose from {last} to {} on the way down", rig.feet().y);
        last = rig.feet().y;
    }
    near(rig.feet().y, 0.0, 1e-3, "back down");
}

/// A step is a short ramp over its edge, not a pop: stopped partway, a character stands on the edge
/// to the bit, and goes on or comes back down the way it came. With the axis d short of the edge, the
/// sphere touches it h = sqrt(0.3² − d²) above it, and a skin held along the contact's normal is
/// 0.01 × 0.3 / h straight down, so its feet are at 0.2 + h + 0.003 / h − 0.31.
#[test]
fn a_step_taken_partway_stands_and_comes_back_down() {
    let step = solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(40.0, 0.2, 20.0));
    let mut rig = Rig::new(vec![floor(1), step], Vec3::ZERO).settled();
    let creep = Vec2::new(0.1, 0.0);
    let mut biggest = 0.0_f32;
    let mut last = 0.0;
    let mut checked = 0;
    let mut ticks = 0;
    while rig.feet().y < 0.1 {
        rig.stick(creep);
        ticks += 1;
        assert!(ticks < 1_000, "never got up: {}", rig.feet());
        let d = 1.0 - rig.feet().x;
        biggest = biggest.max(rig.feet().y - last);
        last = rig.feet().y;
        if rig.feet().y > 0.02 {
            let h = (0.3_f32 * 0.3 - d * d).sqrt();
            near(rig.feet().y, 0.2 + h + 0.003 / h - REACH, 1e-4, "the arc");
            checked += 1;
        }
    }
    println!(
        "a creep up a 0.2 m step: {checked} ticks on the arc, the biggest rise in a tick {biggest:.4} m"
    );
    assert!(checked >= 3, "{checked}");
    assert!(biggest < 0.05, "not a pop: {biggest}");

    // Stopped there, it stays.
    rig.sticks(Vec2::ZERO, 10);
    let stopped = rig.feet();
    rig.sticks(Vec2::ZERO, 300);
    assert_eq!(rig.feet(), stopped, "stands on the edge");
    assert_eq!(rig.mode(), GroundMode::Grounded);

    // Back the way it came, down the same arc, onto the floor.
    rig.sticks(-RIGHT, 30);
    near(rig.feet().y, 0.0, 1e-3, "back on the floor");
    assert_eq!(rig.mode(), GroundMode::Grounded);

    // And at a run it takes a few ticks, the first the biggest: the arc from 0.29 m short.
    let step = solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(40.0, 0.2, 20.0));
    let mut rig = Rig::new(vec![floor(1), step], Vec3::ZERO).settled();
    let mut rises = Vec::new();
    let mut last = 0.0;
    for _ in 0..30 {
        rig.stick(RIGHT);
        if rig.feet().y != last && rig.feet().y - last > 1e-3 {
            rises.push(rig.feet().y - last);
        }
        last = rig.feet().y;
    }
    println!("a run up a 0.2 m step rises by {rises:.4?} a tick");
    assert!((2..=4).contains(&rises.len()), "{rises:?}");
    near(rises.iter().sum::<f32>(), 0.2, 2e-3, "all the way up");
}

/// A ceiling too low over a step leaves no room to rise, so it stays a wall.
#[test]
fn a_low_overhang_refuses_a_step() {
    let step = solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(40.0, 0.2, 20.0));
    // Clearance to the feet is 1.7 m over the floor, 1.5 over the step: too low for 1.61.
    let overhang = solid(3, Vec3::new(0.0, 1.7, -20.0), Vec3::new(40.0, 3.0, 20.0));
    let mut rig = Rig::new(vec![floor(1), step, overhang], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 60);
    near(rig.feet().y, 0.0, 1e-4, "stays down");
    assert!(rig.feet().x < 1.0);
}

// --- Ledges ---

/// A plateau with its top at y = 1 ending at x = 0. The round foot starts over the edge at x = 0, its
/// center sqrt(0.31² − x²) over the top: at x = 0.1, feet 0.0166 down. Walking off goes airborne on
/// the tick the axis passes the stance radius, then falls freely from rest, y = y₀ − g t² / 2, with
/// its planar speed intact.
#[test]
fn walking_off_a_ledge_goes_airborne_and_lands() {
    let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0));
    let mut rig = Rig::new(vec![floor(1), plateau], Vec3::new(-2.0, 1.0, 0.0)).settled();
    let mut last = rig.feet();
    for _ in 0..120 {
        rig.stick(RIGHT);
        if rig.mode() != GroundMode::Grounded {
            break;
        }
        last = rig.feet();
    }
    let top = rig.feet().y;
    println!("ledge: last grounded at {last}, airborne at {}", rig.feet());
    assert_eq!(rig.mode(), GroundMode::Airborne);
    assert!(rig.state.events.left_ground);
    assert!(last.x <= STANCE && rig.feet().x > STANCE, "{last} then {}", rig.feet());
    near(last.y, 1.0 + (REACH * REACH - last.x * last.x).sqrt() - REACH, 1e-3, "over the edge");
    assert_eq!(rig.state.velocity.y, 0.0, "no launch, no pull down");

    for tick in 1..=120 {
        rig.stick(RIGHT);
        let t = tick as f32 * DT;
        if rig.state.events.landed {
            println!(
                "ledge: landed at tick {tick} at {:.3} m/s, x = {:.3}",
                rig.state.events.landing_speed,
                rig.feet().x
            );
            // The fall takes sqrt(2 y₀ / g), and it counts as touching 3 cm early.
            let time = (2.0 * (top - 0.03) / GRAVITY).sqrt();
            assert!((tick as f32 * DT - time).abs() <= 1.5 * DT, "tick {tick}, against {time} s");
            near(rig.feet().y, 0.0, 2e-4, "on the floor");
            near(rig.state.velocity.x, RUN, 1e-3, "planar speed intact");
            assert_eq!(rig.state.velocity.y, 0.0);
            return;
        }
        near(rig.feet().y, top - GRAVITY * t * t / 2.0, 1e-4, "free fall");
    }
    panic!("never landed");
}

/// A landing rests on the floor from any height, however a tick's motion falls against it.
#[test]
fn a_landing_rests_on_the_floor_from_any_height() {
    for tenths in 6..43 {
        let height = tenths as f32 / 10.0;
        let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, height, 20.0));
        let mut rig = Rig::new(vec![floor(1), plateau], Vec3::new(-2.0, height, 0.0)).settled();
        for _ in 0..300 {
            rig.stick(RIGHT);
            if rig.state.events.landed {
                break;
            }
        }
        assert!(rig.state.events.landed, "{height} m: at {}", rig.feet());
        near(rig.feet().y, 0.0, 2e-4, "on the floor");
    }
    println!("landings from 37 heights: feet on the floor each time");
}

/// A sweep keeps a skin from what it finds, and looks only 4 skins past its motion. At a shallow angle
/// that's less than a skin's gap, so a motion can end with the ground 4 to 10 mm under it unnoticed,
/// its feet under where they rest. They're lifted back that same tick, on a run onto level ground and
/// on a lunge coming down.
#[test]
fn a_shallow_approach_is_lifted_to_rest() {
    for degrees in [2.0_f32, 3.0, 5.0, 8.0, 12.0] {
        let x = 5.0;
        let feet = Vec3::new(x, feet_on_slope(x, 0.0, 0.0, degrees), 0.0);
        let mut rig = Rig::new(slope_scene(degrees), feet).settled();
        let mut lifted = 0.0_f32;
        for _ in 0..90 {
            rig.stick(-RIGHT);
            lifted = lifted.max(rig.state.events.height_pop);
        }
        println!("a run down {degrees}° onto level ground: lifted by {lifted:.4} m, feet at {}", rig.feet());
        assert!(rig.feet().x < -0.5, "{degrees}°: past the foot of the ramp: {}", rig.feet());
        near(rig.feet().y, 0.0, 2e-4, "on the floor");
        assert_eq!(rig.mode(), GroundMode::Grounded);
        assert!(degrees > 5.0 || lifted > 1e-3, "{degrees}°: this case is here for the lift: {lifted}");
    }

    // A metre a tick, a tenth of it down: 0.297, 0.197, 0.097, then −0.003, which is a gap of 7 mm.
    // Along a path at 5.7° that's 7 cm past the motion, out of the sweep's sight.
    let mut rig = Rig::new(vec![floor(1)], Vec3::new(0.0, 0.297, 0.0));
    let mut lifted = 0.0_f32;
    for _ in 0..4 {
        rig.drive(ControllerDrive::flight(Vec3::new(1.0, -0.1, 0.0)));
        lifted = lifted.max(rig.state.events.height_pop);
    }
    println!("a lunge coming down at 5.7°: lifted by {lifted:.4} m, feet at {}", rig.feet());
    near(lifted, 0.003, 1e-4, "the lift");
    near(rig.feet().y, 0.0, 2e-4, "on the floor");
}

/// A slow walker's round foot would roll off slowly: it still goes at the stance radius, and comes
/// down off the edge however slowly it was going.
#[test]
fn a_creep_off_a_ledge_comes_off_too() {
    let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0));
    let mut rig = Rig::new(vec![floor(1), plateau], Vec3::new(-0.5, 1.0, 0.0)).settled();
    let stick = Vec2::new(0.1, 0.0);
    let mut left_at = None;
    for _ in 0..600 {
        rig.stick(stick);
        if rig.mode() != GroundMode::Grounded && left_at.is_none() {
            left_at = Some(rig.feet().x);
        }
        if rig.feet().y < 0.5 {
            break;
        }
    }
    let left = left_at.expect("it left");
    println!("creep off a ledge: left at x = {left:.4}, fell past 0.5 m at x = {:.4}", rig.feet().x);
    near(left, STANCE, 0.02, "the stance radius");
    assert!(rig.feet().y < 0.5, "got off the edge");
    rig.sticks(stick, 60);
    near(rig.feet().y, 0.0, 1e-3, "landed");
    assert_eq!(rig.mode(), GroundMode::Grounded);
}

/// Standing just inside the stance radius holds, however long.
#[test]
fn standing_at_the_stance_radius_holds() {
    let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0));
    let mut rig = Rig::new(vec![floor(1), plateau], Vec3::new(STANCE - 0.005, 1.0, 0.0));
    rig.sticks(Vec2::ZERO, 300);
    assert_eq!(rig.mode(), GroundMode::Grounded);
    near(rig.feet().x, STANCE - 0.005, 1e-5, "stayed");

    let mut rig = Rig::new(
        vec![floor(1), solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0))],
        Vec3::new(STANCE + 0.005, 1.0, 0.0),
    );
    rig.sticks(Vec2::ZERO, 120);
    near(rig.feet().y, 0.0, 1e-3, "came off");
}

/// Coyote time: 5 ticks of the owner's time after leaving a ledge, and a jump still works, and only
/// after walking off. A profile can turn it off.
#[test]
fn coyote_time_is_authored_and_ends() {
    let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0));
    for (coyote_ticks, after, jumps) in [
        (5, 1, true),
        (5, 3, true),
        (5, 5, true),
        (5, 6, false),
        (5, 8, false),
        (0, 1, false),
        (2, 2, true),
        (2, 3, false),
    ] {
        let profile = ControllerProfile { coyote_ticks, ..ControllerProfile::default() };
        let mut rig =
            Rig::with_profile(vec![floor(1), plateau.clone()], Vec3::new(-1.0, 1.0, 0.0), profile).settled();
        while rig.mode() == GroundMode::Grounded {
            rig.stick(RIGHT);
            assert!(rig.feet().x < 5.0, "never left the ledge: {}", rig.feet());
        }
        rig.sticks(RIGHT, after - 1);
        rig.jump(RIGHT);
        assert_eq!(rig.state.events.jumped, jumps, "coyote {coyote_ticks}, {after} ticks after leaving");
        if jumps {
            assert!(rig.state.velocity.y > 0.0);
            assert_eq!(rig.state.coyote, SimDuration::ZERO, "spent");
        }
    }
    // A jump doesn't earn any.
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.jump(Vec2::ZERO);
    rig.sticks(Vec2::ZERO, 30);
    assert_eq!(rig.state.coyote, SimDuration::ZERO);
}

/// A small drop is a step down, not a fall: the feet follow it, and a walker stays grounded.
#[test]
fn a_small_drop_is_followed_and_a_large_one_is_a_fall() {
    for (drop, stays) in [(0.1_f32, true), (0.25, true), (0.3, true), (0.6, false), (1.0, false)] {
        let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, drop, 20.0));
        let mut rig = Rig::new(vec![floor(1), plateau], Vec3::new(-1.0, drop, 0.0)).settled();
        let mut grounded = true;
        for _ in 0..60 {
            rig.stick(RIGHT);
            grounded &= rig.mode() == GroundMode::Grounded;
        }
        println!("a drop of {drop} m: stayed grounded: {grounded}");
        assert_eq!(grounded, stays, "a drop of {drop} m");
        near(rig.feet().y, 0.0, 1e-3, "at the bottom");
    }
}

// --- Walls and corners ---

/// A wall through (1, 0, 0) along `along`, tall and wide.
fn wall_along(id: u32, through: Vec3, along: Vec3) -> StaticCollider {
    let a = through - along * 60.0;
    let b = through + along * 60.0;
    quad(
        id,
        [
            Vec3::new(a.x, -1.0, a.z),
            Vec3::new(b.x, -1.0, b.z),
            Vec3::new(b.x, 6.0, b.z),
            Vec3::new(a.x, 6.0, a.z),
        ],
    )
}

/// Running along +z into a wall that leans across the path by α: the speed left is the part along the
/// wall, 6 cos α, and the gap from it stays the skin.
#[test]
fn a_wall_at_a_shallow_angle_keeps_the_tangential_speed() {
    for degrees in [5.0_f32, 15.0, 30.0, 60.0] {
        let (sin, cos) = det::sin_cos(degrees.to_radians());
        let along = Vec3::new(-sin, 0.0, cos);
        let through = Vec3::new(1.0, 0.0, 0.0);
        let mut rig = Rig::new(vec![floor(1), wall_along(2, through, along)], Vec3::ZERO).settled();
        let stick = Vec2::new(0.0, 1.0);
        // Until well into the slide along it.
        rig.sticks(stick, (2.0 / (RUN * sin).max(1.0) * 60.0) as u32 + 30);
        let before = rig.feet();
        rig.sticks(stick, 60);
        let moved = rig.feet() - before;
        let tangential = moved.dot(along);
        let across = moved.dot(Vec3::new(cos, 0.0, sin));
        println!(
            "a wall at {degrees}°: {tangential:.4} m/s along it (6 cos α = {:.4}), {across:.6} m across",
            RUN * cos
        );
        near(tangential, RUN * cos, 0.005 * RUN, "tangential speed");
        near(across, 0.0, 1e-3, "across");
        assert_eq!(rig.mode(), GroundMode::Grounded);
        // The capsule's axis stays clear of the plane by its radius, and within its reach. A sweep at
        // such a shallow angle doesn't hold the whole skin, only stays out of the wall.
        let gap = (through - (rig.feet() + Vec3::Y * 0.81)).dot(Vec3::new(cos, 0.0, sin));
        assert!(gap > 0.3 && gap <= REACH + 1e-3, "a wall at {degrees}°: the axis is {gap} from it");
    }
}

/// Into a corner of two walls: stops where both gaps are the skin, and stays there. A right angle's
/// spot is (1 − 0.31, 1 − 0.31). A 30° wedge's apex at z = 3 stops the axis 0.31 / sin 15° short.
#[test]
fn a_corner_of_two_walls_stops_without_jitter() {
    let (sin15, cos15) = det::sin_cos(15.0_f32.to_radians());
    let wedge = |stick: Vec2| {
        let apex = Vec3::new(0.0, 0.0, 3.0);
        let a = wall_half(2, apex, Vec3::new(-sin15, 0.0, -cos15));
        let b = wall_half(3, apex, Vec3::new(sin15, 0.0, -cos15));
        (vec![floor(1), a, b], stick, Vec2::new(0.0, 3.0 - REACH / sin15))
    };
    let square = |stick: Vec2| {
        (
            vec![
                floor(1),
                solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(2.0, 6.0, 20.0)),
                solid(3, Vec3::new(-20.0, -1.0, 1.0), Vec3::new(20.0, 6.0, 2.0)),
            ],
            stick,
            Vec2::new(1.0 - REACH, 1.0 - REACH),
        )
    };
    let rows = [
        ("a right angle, on the diagonal", square(Vec2::new(0.7, 0.7))),
        ("a right angle, mostly along x", square(Vec2::new(0.95, 0.2))),
        ("a right angle, mostly along z", square(Vec2::new(0.2, 0.95))),
        ("a wedge, straight in", wedge(Vec2::new(0.0, 1.0))),
        ("a wedge, off to one side", wedge(Vec2::new(0.2, 0.9))),
        ("a wedge, off to the other", wedge(Vec2::new(-0.2, 0.9))),
    ];
    for (name, (statics, stick, spot)) in rows {
        let mut rig = Rig::new(statics, Vec3::ZERO).settled();
        let wobble = rig.jitter(stick, 240, 120);
        let at = Vec2::new(rig.feet().x, rig.feet().z);
        println!("{name}: at {at:.4} (hand: {spot:.4}), wobble {wobble}");
        assert!((at - spot).length() < 3e-3, "{name}: {at} is not {spot}");
        assert!(wobble < 1e-5, "{name} wobbles {wobble}");
        assert_eq!(rig.mode(), GroundMode::Grounded, "{name}");
        near(rig.feet().y, 0.0, 1e-4, name);
    }
}

/// A wall from `apex` going `away`.
fn wall_half(id: u32, apex: Vec3, away: Vec3) -> StaticCollider {
    let far = apex + away * 60.0;
    quad(
        id,
        [
            Vec3::new(apex.x, -1.0, apex.z),
            Vec3::new(far.x, -1.0, far.z),
            Vec3::new(far.x, 6.0, far.z),
            Vec3::new(apex.x, 6.0, apex.z),
        ],
    )
}

/// A V of two slopes at θ, its crease along z at the origin. Too steep to stand on, a character falling
/// in comes to rest on both: axis on the crease, feet 0.31 / cos θ − 0.31 up. Walkable, it just stands
/// where it lands. Neither jitters, and a stick into either side doesn't change the steep one.
#[test]
fn a_wedge_of_two_slopes_holds_without_jitter() {
    let v = |degrees: f32| {
        let run = 20.0;
        let rise = run * det::tan(degrees.to_radians());
        vec![
            quad(
                1,
                [
                    Vec3::new(0.0, 0.0, -20.0),
                    Vec3::new(run, rise, -20.0),
                    Vec3::new(run, rise, 20.0),
                    Vec3::new(0.0, 0.0, 20.0),
                ],
            ),
            quad(
                2,
                [
                    Vec3::new(0.0, 0.0, -20.0),
                    Vec3::new(-run, rise, -20.0),
                    Vec3::new(-run, rise, 20.0),
                    Vec3::new(0.0, 0.0, 20.0),
                ],
            ),
        ]
    };
    for degrees in [60.0_f32, 70.0] {
        let cos = det::cos(degrees.to_radians());
        let rest = REACH / cos - REACH;
        for stick in [Vec2::ZERO, RIGHT, -RIGHT] {
            let mut rig = Rig::new(v(degrees), Vec3::new(0.2, 3.0, 0.0));
            let wobble = rig.jitter(stick, 360, 120);
            println!(
                "V of {degrees}° with stick {stick}: at ({:.4}, {:.4}), hand y {rest:.4}, wobble {wobble}, {:?}",
                rig.feet().x,
                rig.feet().y,
                rig.mode()
            );
            near(rig.feet().x, 0.0, 2e-3, "on the crease");
            near(rig.feet().y, rest, 2e-3, "resting on both");
            assert!(wobble < 1e-4, "V of {degrees}° wobbles {wobble}");
            assert_eq!(rig.mode(), GroundMode::Sliding);
        }
        // Wedged is not stuck: it can jump out.
        let mut rig = Rig::new(v(degrees), Vec3::new(0.2, 3.0, 0.0));
        rig.sticks(Vec2::ZERO, 240);
        rig.jump(Vec2::ZERO);
        assert!(rig.state.events.jumped, "wedged, it can jump");
    }
    for degrees in [20.0_f32, 30.0, 45.0] {
        // Lands on the +x side, and stays put.
        let mut rig = Rig::new(v(degrees), Vec3::new(0.2, 3.0, 0.0));
        let wobble = rig.jitter(Vec2::ZERO, 240, 120);
        println!("V of {degrees}°: at ({:.4}, {:.4}), wobble {wobble}", rig.feet().x, rig.feet().y);
        assert_eq!(rig.mode(), GroundMode::Grounded);
        assert_eq!(wobble, 0.0, "not a bit");
        // And crosses the crease in both directions, grounded and never under the V.
        for (stick, ticks) in [(-RIGHT, 60), (RIGHT, 120)] {
            let mut ticks_grounded = 0;
            for _ in 0..ticks {
                rig.stick(stick);
                ticks_grounded += u32::from(rig.mode() == GroundMode::Grounded);
                let lowest = REACH / det::cos(degrees.to_radians()) - REACH;
                assert!(rig.feet().y >= lowest - 1e-3, "{degrees}° went under at {}", rig.feet());
            }
            assert_eq!(ticks_grounded, ticks, "{degrees}° stays grounded across the crease");
            assert!(rig.feet().x * stick.x > 1.0, "{degrees}° got across and up");
        }
    }
}

/// However fast, a sweep can't pass a wall: a 4 mm box and a sheet with no thickness, from a run at
/// 200 m/s (3.3 m a tick) and root motion of 100 m in one tick. The stop is always the same spot.
#[test]
fn high_speed_cant_pass_a_thin_wall() {
    let thin_box = || solid(2, Vec3::new(4.0, -1.0, -20.0), Vec3::new(4.004, 6.0, 20.0));
    let sheet = || {
        quad(
            2,
            [
                Vec3::new(4.0, -1.0, -20.0),
                Vec3::new(4.0, -1.0, 20.0),
                Vec3::new(4.0, 6.0, 20.0),
                Vec3::new(4.0, 6.0, -20.0),
            ],
        )
    };
    let fast =
        ControllerProfile { run_speed: 200.0, acceleration: 2e5, deceleration: 2e5, ..Default::default() };
    for (name, wall) in [("a 4 mm box", thin_box()), ("a sheet", sheet())] {
        let mut rig = Rig::with_profile(vec![floor(1), wall.clone()], Vec3::ZERO, fast).settled();
        rig.sticks(RIGHT, 30);
        println!("{name}, 200 m/s: stopped at x = {:.4}", rig.feet().x);
        near(rig.feet().x, 4.0 - REACH, 1e-3, name);

        let mut rig = Rig::new(vec![floor(1), wall.clone()], Vec3::ZERO).settled();
        rig.drive(ControllerDrive::strike(Vec3::X * 100.0));
        println!("{name}, 100 m in a tick: stopped at x = {:.4}", rig.feet().x);
        near(rig.feet().x, 4.0 - REACH, 1e-3, name);
    }
    // In the air, down through a sheet floor at the fall's limit.
    let floor_sheet = quad(
        1,
        [
            Vec3::new(-20.0, 0.0, -20.0),
            Vec3::new(20.0, 0.0, -20.0),
            Vec3::new(20.0, 0.0, 20.0),
            Vec3::new(-20.0, 0.0, 20.0),
        ],
    );
    let mut rig = Rig::new(vec![floor_sheet], Vec3::new(0.0, 80.0, 0.0));
    rig.sticks(Vec2::ZERO, 300);
    near(rig.feet().y, 0.0, 1e-3, "lands on the sheet");
    assert_eq!(rig.mode(), GroundMode::Grounded);
}

// --- Root motion ---

/// On a flat floor a strike moves by exactly its table, drops any velocity it had, and ignores the
/// stick and the table's height, because the floor decides that.
#[test]
fn a_strike_moves_by_its_table_alone() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 20);
    let from = rig.feet();
    let yaw = rig.transform.yaw;
    for _ in 0..20 {
        rig.input(ControllerInput {
            stick: Vec2::new(0.0, 1.0),
            jump: false,
            drive: ControllerDrive::strike(Vec3::new(0.0, 0.05, 0.1)),
        });
        assert_eq!(rig.state.velocity, Vec3::ZERO);
        assert_eq!(rig.mode(), GroundMode::Grounded);
    }
    let moved = rig.feet() - from;
    near(moved.z, 2.0, 1e-4, "the table's distance");
    near(moved.x, 0.0, 1e-5, "no stick");
    near(moved.y, 0.0, 1e-4, "the floor decides the height");
    assert_eq!(rig.transform.yaw, yaw, "the stick didn't turn it either");
    // The next tick with no drive has the stick again.
    rig.stick(RIGHT);
    assert!(rig.state.velocity.x > 0.0);
}

/// A lunge meets a wall like anything else: stops, or slides along what's left.
#[test]
fn a_lunge_stops_at_a_wall() {
    let wall = solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(2.0, 6.0, 20.0));
    let mut rig = Rig::new(vec![floor(1), wall], Vec3::ZERO).settled();
    for _ in 0..10 {
        rig.drive(ControllerDrive::strike(Vec3::new(0.5, 0.0, 0.1)));
    }
    println!("a lunge at a wall: stopped at x = {:.4}, slid to z = {:.4}", rig.feet().x, rig.feet().z);
    near(rig.feet().x, 1.0 - REACH, 1e-3, "stopped");
    near(rig.feet().z, 1.0, 1e-3, "the part along the wall still moved");
}

/// What each of the drive's flags does, on and off, each against the numbers it should give.
#[test]
fn the_drives_flags_say_what_applies() {
    // Locomotion on: the stick's motion and the table's add up.
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 10);
    let from = rig.feet().x;
    let keep_walking = ControllerDrive { locomotion: true, ..ControllerDrive::strike(Vec3::X * 0.05) };
    for _ in 0..10 {
        rig.input(ControllerInput { stick: RIGHT, drive: keep_walking, ..ControllerInput::default() });
    }
    near(rig.feet().x - from, 10.0 * (0.1 + 0.05), 1e-4, "stick and table");

    // Gravity on, in the air: it falls as usual, with the table's motion on top.
    let mut rig = Rig::new(vec![floor(1)], Vec3::new(0.0, 5.0, 0.0));
    for _ in 0..10 {
        rig.drive(ControllerDrive::strike(Vec3::X * 0.1));
    }
    let t = 10.0 * DT;
    near(rig.feet().y, 5.0 - GRAVITY * t * t / 2.0, 1e-4, "falls");
    near(rig.feet().x, 1.0, 1e-4, "the table's distance");

    // Gravity off, in the air: it hangs, then falls from rest when the drive ends.
    let mut rig = Rig::new(vec![floor(1)], Vec3::new(0.0, 5.0, 0.0));
    rig.sticks(Vec2::ZERO, 10);
    let hung_at = rig.feet().y;
    for _ in 0..30 {
        rig.drive(ControllerDrive::flight(Vec3::ZERO));
        assert_eq!(rig.state.velocity.y, 0.0);
    }
    assert_eq!(rig.feet().y, hung_at, "held");
    rig.stick(Vec2::ZERO);
    near(rig.feet().y, hung_at - GRAVITY * DT * DT / 2.0, 1e-5, "from rest");

    // Gravity off, on the ground: a table with height lifts off, and every bit of it counts.
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    for _ in 0..10 {
        rig.drive(ControllerDrive::flight(Vec3::new(0.05, 0.1, 0.0)));
    }
    assert_eq!(rig.mode(), GroundMode::Airborne);
    near(rig.feet().y, 1.0, 1e-4, "lifted by the table");
    near(rig.feet().x, 0.5, 1e-4, "and carried");
    // And a table coming down lands.
    for _ in 0..30 {
        rig.drive(ControllerDrive::flight(Vec3::new(0.0, -0.1, 0.0)));
    }
    assert_eq!(rig.mode(), GroundMode::Grounded);
    near(rig.feet().y, 0.0, 1e-3, "landed");

    // `jump` fires what a press would, where it can.
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    let buffered = ControllerDrive { jump: true, ..ControllerDrive::NONE };
    rig.input(ControllerInput { jump: buffered.jump, drive: buffered, ..ControllerInput::default() });
    assert!(rig.state.events.jumped);

    // Motion that isn't finite is dropped, rather than stopping the world.
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.drive(ControllerDrive::strike(Vec3::new(f32::NAN, 0.0, f32::INFINITY)));
    assert_eq!(rig.feet(), Vec3::ZERO);
}

// --- The combat movement mode ---

fn combat_step(height: f32) -> (Rig, ControllerDrive, ControllerDrive) {
    let step = solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(40.0, height, 20.0));
    let strike = ControllerDrive::strike(Vec3::X * 0.1);
    let free = ControllerDrive { mode: MovementMode::Free, ..strike };
    (Rig::new(vec![floor(1), step], Vec3::ZERO).settled(), strike, free)
}

/// The spec's point: a strike takes no step-up. The same motion in the free mode does.
#[test]
fn the_combat_mode_takes_no_step_up() {
    for height in [0.15_f32, 0.2, 0.29] {
        let (mut rig, strike, _) = combat_step(height);
        for _ in 0..60 {
            rig.drive(strike);
            assert!(!rig.state.events.stepped_up);
            near(rig.feet().y, 0.0, 1e-5, "no hop");
        }
        let below = REACH - height;
        let expected = 1.0 - (REACH * REACH - below * below).sqrt();
        println!("combat at a {height} m step: stopped at x = {:.4} (hand: {expected:.4})", rig.feet().x);
        near(rig.feet().x, expected, 2e-3, "stopped at the edge");

        let (mut rig, _, free) = combat_step(height);
        for _ in 0..60 {
            rig.drive(free);
        }
        near(rig.feet().y, height, 1e-3, "the free mode climbs");
    }
}

/// A curb low enough to roll over is rolled over, smoothly, with no pop.
#[test]
fn the_combat_mode_rolls_a_low_curb() {
    let (mut rig, strike, _) = combat_step(0.05);
    let mut last = 0.0;
    for _ in 0..60 {
        rig.drive(strike);
        let rise = rig.feet().y - last;
        assert!(rise.abs() < 0.04, "a pop of {rise}");
        last = rig.feet().y;
    }
    near(rig.feet().y, 0.05, 1e-3, "over it");
    assert!(rig.feet().x > 1.0);
}

/// A strike can't carry a character off a ledge: it stops at the last tick that kept it on, grounded,
/// where the free mode falls. A drop small enough to follow is followed.
#[test]
fn the_combat_mode_holds_a_ledge() {
    let plateau = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, 1.0, 20.0));
    for motion in [0.02_f32, 0.1, 0.25, 1.0] {
        let mut rig = Rig::new(vec![floor(1), plateau.clone()], Vec3::new(-1.0, 1.0, 0.0)).settled();
        let strike = ControllerDrive::strike(Vec3::X * motion);
        let mut held = false;
        let mut last_x = rig.feet().x;
        for _ in 0..200 {
            last_x = rig.feet().x;
            rig.drive(strike);
            held |= rig.state.events.ledge_held;
            assert_eq!(rig.mode(), GroundMode::Grounded, "{motion} m a tick");
            // Its round foot may start over the edge, by as much as the stance radius allows.
            let lowest = 1.0 + (REACH * REACH - STANCE * STANCE).sqrt() - REACH;
            assert!(rig.feet().y >= lowest - 1e-3, "{motion} m a tick: down to {}", rig.feet().y);
        }
        println!("combat at a ledge, {motion} m a tick: held at x = {:.4}, stance {STANCE}", rig.feet().x);
        assert!(held, "the ledge held it");
        assert!(rig.feet().x <= STANCE, "{}", rig.feet().x);
        assert!(rig.feet().x > STANCE - motion - 1e-4, "stopped within a tick's motion: {}", rig.feet().x);
        assert_eq!(rig.feet().x, last_x, "and stays exactly there");

        let free = ControllerDrive { mode: MovementMode::Free, ..strike };
        let mut rig = Rig::new(vec![floor(1), plateau.clone()], Vec3::new(-1.0, 1.0, 0.0)).settled();
        for _ in 0..200 {
            rig.drive(free);
        }
        near(rig.feet().y, 0.0, 1e-3, "the free mode falls");
    }

    // Over a small drop it keeps to the floor, grounded all the way down.
    let drop = solid(2, Vec3::new(-40.0, -1.0, -20.0), Vec3::new(0.0, 0.2, 20.0));
    let mut rig = Rig::new(vec![floor(1), drop], Vec3::new(-1.0, 0.2, 0.0)).settled();
    for _ in 0..40 {
        rig.drive(ControllerDrive::strike(Vec3::X * 0.1));
        assert_eq!(rig.mode(), GroundMode::Grounded);
        assert!(!rig.state.events.ledge_held);
    }
    near(rig.feet().y, 0.0, 1e-3, "followed it down");
}

// --- A spawn's first tick ---

/// A spawn inside a wall has a defined first tick: pushed out by the shortest way, a skin clear, and
/// then it's a character like any other.
///
/// The cube's half extent is 1, with its center at (0, 1, 0). Spawned at feet (0.2, 0, 0.1), the
/// capsule's core runs from y = 0.31 to 1.31, so the shortest way out of the cube for the whole core is
/// +x: 1 − 0.2 for the core, plus the radius, plus the skin: x = 1.31.
#[test]
fn a_spawn_inside_a_wall_is_pushed_out_on_its_first_tick() {
    let cube = solid(2, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0));
    let mut rig = Rig::new(vec![floor(1), cube.clone()], Vec3::new(0.2, 0.0, 0.1));
    rig.stick(Vec2::ZERO);
    println!("a spawn in a cube: first tick at {}", rig.feet());
    assert!(rig.state.events.pushed_out);
    near(rig.feet().x, 1.31, 1e-3, "out by +x");
    near(rig.feet().z, 0.1, 1e-4, "only that way");
    near(rig.feet().y, 0.0, 1e-3, "at floor height");
    assert_eq!(rig.mode(), GroundMode::Grounded);
    rig.stick(Vec2::ZERO);
    assert!(!rig.state.events.pushed_out, "once");
    // The same tick follows, whichever spawn it was: walks off and stops as usual.
    rig.sticks(RIGHT, 10);
    assert!(rig.feet().x > 1.31);

    // Half in, with the core outside: out by the depth, along the surface's normal.
    let wall = solid(2, Vec3::new(1.0, -1.0, -20.0), Vec3::new(2.0, 6.0, 20.0));
    let mut rig = Rig::new(vec![floor(1), wall], Vec3::new(0.9, 0.0, 0.0));
    rig.stick(Vec2::ZERO);
    assert!(rig.state.events.pushed_out);
    near(rig.feet().x, 1.0 - REACH, 1e-3, "a skin clear of it");

    // Under the floor: comes up onto it.
    let mut rig = Rig::new(vec![floor(1)], Vec3::new(0.0, -0.5, 0.0));
    rig.stick(Vec2::ZERO);
    assert!(rig.state.events.pushed_out);
    rig.stick(Vec2::ZERO);
    near(rig.feet().y, 0.0, 2e-3, "on top");
    assert_eq!(rig.mode(), GroundMode::Grounded);

    // Through a mesh with no inside: out by the side its core is nearer.
    let sheet = quad(
        2,
        [
            Vec3::new(0.0, -1.0, -20.0),
            Vec3::new(0.0, -1.0, 20.0),
            Vec3::new(0.0, 6.0, 20.0),
            Vec3::new(0.0, 6.0, -20.0),
        ],
    );
    let mut rig = Rig::new(vec![floor(1), sheet], Vec3::new(-0.1, 0.0, 0.0));
    rig.stick(Vec2::ZERO);
    assert!(rig.state.events.pushed_out);
    near(rig.feet().x, -REACH, 1e-3, "out the near side");

    // With no way out at all, narrower than the capsule, the result is still a number each tick.
    let squeezed = vec![
        floor(1),
        solid(2, Vec3::new(-2.0, -1.0, -20.0), Vec3::new(-0.2, 6.0, 20.0)),
        solid(3, Vec3::new(0.2, -1.0, -20.0), Vec3::new(2.0, 6.0, 20.0)),
    ];
    let mut rig = Rig::new(squeezed, Vec3::ZERO);
    rig.sticks(RIGHT, 60);
    assert!(rig.feet().is_finite(), "{}", rig.feet());
}

/// A pose that isn't right refuses, rather than panicking in a step.
#[test]
fn a_body_that_isnt_there_is_an_error() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO);
    let before = rig.state;
    let result = step_controller(
        &mut rig.world,
        BodyId(99),
        &mut rig.transform,
        &mut rig.state,
        &rig.profile,
        &ControllerInput::default(),
        TICK,
    );
    assert!(result.is_err());
    assert_eq!(rig.state, before);
}

// --- Other characters ---

fn second_capsule(world: &mut PhysicsWorld, profile: &ResolvedProfile, feet: Vec3) {
    let body: BodyDesc = profile.body(BodyId(2), feet, 0.0);
    world.add_body(body).expect("it fits");
}

/// Capsules block each other: a run stops a skin short of 2 radii away, then slides round it. They
/// don't push each other.
#[test]
fn characters_block_each_other() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    let profile = rig.profile;
    second_capsule(&mut rig.world, &profile, Vec3::new(2.0, 0.0, 0.0));
    rig.sticks(RIGHT, 60);
    println!("into another capsule: stopped at x = {:.4}", rig.feet().x);
    near(rig.feet().x, 2.0 - (2.0 * 0.3 + 0.01), 1e-3, "a skin from it");
    assert_eq!(rig.state.velocity.x, RUN - RUN, "it stopped, and no speed was left into it");
    let other = rig.world.body_pose(BodyId(2)).expect("there").position;
    assert_eq!(other, Vec3::new(2.0, 0.81, 0.0), "the other didn't move");

    // Aimed off center, it slides round.
    let mut rig = Rig::new(vec![floor(1)], Vec3::new(0.0, 0.0, 0.2)).settled();
    second_capsule(&mut rig.world, &profile, Vec3::new(2.0, 0.0, 0.0));
    rig.sticks(RIGHT, 120);
    assert!(rig.feet().x > 2.0, "it got round: {}", rig.feet());
    assert_eq!(rig.mode(), GroundMode::Grounded);
}

/// Another character's head isn't ground: from slightly off center it slides off and lands beside it.
#[test]
fn a_character_doesnt_stand_on_another() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::new(0.05, 3.0, 0.0));
    let profile = rig.profile;
    second_capsule(&mut rig.world, &profile, Vec3::ZERO);
    let mut stood = false;
    for _ in 0..300 {
        rig.stick(Vec2::ZERO);
        stood |= rig.mode() == GroundMode::Grounded && rig.feet().y > 1.0;
    }
    println!("onto another's head: ended at {} {:?}", rig.feet(), rig.mode());
    assert!(!stood, "it stood on a head");
    assert_eq!(rig.mode(), GroundMode::Grounded);
    near(rig.feet().y, 0.0, 1e-3, "on the floor beside it");
    assert!(Vec2::new(rig.feet().x, rig.feet().z).length() >= 2.0 * 0.3, "beside it");
}

// --- The profile ---

#[test]
fn a_profile_refuses_numbers_that_cant_work() {
    let good = ControllerProfile::default();
    assert!(good.resolve().is_ok());
    let bad = [
        ("radius", ControllerProfile { radius: 0.0, ..good }),
        ("radius", ControllerProfile { radius: f32::NAN, ..good }),
        ("height", ControllerProfile { height: 0.5, ..good }),
        ("skin", ControllerProfile { skin: 0.0, ..good }),
        ("skin", ControllerProfile { skin: 0.2, ..good }),
        ("walk_speed", ControllerProfile { walk_speed: -1.0, ..good }),
        ("run_speed", ControllerProfile { run_speed: 1.0, ..good }),
        ("walk_stick", ControllerProfile { walk_stick: 1.0, ..good }),
        ("run_stick", ControllerProfile { run_stick: 0.2, ..good }),
        ("run_stick", ControllerProfile { run_stick: 1.5, ..good }),
        ("acceleration", ControllerProfile { acceleration: 0.0, ..good }),
        ("deceleration", ControllerProfile { deceleration: f32::INFINITY, ..good }),
        ("turn_rate_degrees", ControllerProfile { turn_rate_degrees: 0.0, ..good }),
        ("gravity", ControllerProfile { gravity: -1.0, ..good }),
        ("jump_speed", ControllerProfile { jump_speed: -1.0, ..good }),
        ("air_acceleration", ControllerProfile { air_acceleration: f32::NAN, ..good }),
        ("max_fall_speed", ControllerProfile { max_fall_speed: 0.0, ..good }),
        ("slope_limit_degrees", ControllerProfile { slope_limit_degrees: 0.0, ..good }),
        ("slope_limit_degrees", ControllerProfile { slope_limit_degrees: 89.0, ..good }),
        ("step_height", ControllerProfile { step_height: -0.1, ..good }),
        ("ground_snap", ControllerProfile { ground_snap: -0.1, ..good }),
        ("stance_radius", ControllerProfile { stance_radius: 0.5, ..good }),
        ("max_slide_speed", ControllerProfile { max_slide_speed: 0.0, ..good }),
    ];
    for (field, profile) in bad {
        assert_eq!(profile.resolve().expect_err(field).field, field);
    }
}

/// A document names what it changes. The rest are the defaults, and a typo is refused.
#[test]
fn a_profile_reads_from_a_document() {
    let profile: ControllerProfile = ron::from_str("(run_speed: 7.5, coyote_ticks: 2)").expect("it reads");
    assert_eq!(
        profile,
        ControllerProfile { run_speed: 7.5, coyote_ticks: 2, ..ControllerProfile::default() }
    );
    assert!(ron::from_str::<ControllerProfile>("(run_sped: 7.5)").is_err());
    let text = ron::to_string(&profile).expect("it writes");
    assert_eq!(ron::from_str::<ControllerProfile>(&text).expect("it reads back"), profile);
}

/// The input's own guard: a stick past 1 counts as 1, and NaN as the middle.
#[test]
fn a_stick_out_of_range_is_guarded() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(Vec2::new(5.0, 0.0), 30);
    assert_eq!(rig.state.velocity.x, RUN);
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.stick(Vec2::new(f32::NAN, 0.0));
    assert_eq!(rig.feet(), Vec3::ZERO);
}

/// The frozen case at this level: a delta of zero changes nothing but last tick's events.
#[test]
fn a_zero_delta_changes_nothing() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.sticks(RIGHT, 3);
    rig.jump(RIGHT);
    let before = (rig.state, rig.transform, rig.world.body_pose(BODY));
    assert!(rig.state.events.jumped);
    let input = ControllerInput { stick: -RIGHT, jump: true, drive: ControllerDrive::strike(Vec3::X) };
    step_controller(
        &mut rig.world,
        BODY,
        &mut rig.transform,
        &mut rig.state,
        &rig.profile,
        &input,
        SimDuration::ZERO,
    )
    .expect("the body is there");
    assert_eq!(rig.state.events, ControllerEvents::default());
    assert_eq!(rig.state.velocity, before.0.velocity);
    assert_eq!(rig.state.mode, before.0.mode);
    assert_eq!(rig.state.coyote, before.0.coyote);
    assert_eq!(rig.transform, before.1);
    assert_eq!(rig.world.body_pose(BODY), before.2);
}

/// Hurtboxes and other bodies a character doesn't collide with don't get in its way.
#[test]
fn bodies_outside_its_groups_dont_block_it() {
    let mut rig = Rig::new(vec![floor(1)], Vec3::ZERO).settled();
    rig.world
        .add_body(BodyDesc {
            id: BodyId(100),
            shape: Shape::Sphere { radius: 0.5 },
            pose: Pose::new(Vec3::new(1.0, 0.8, 0.0), Quat::IDENTITY),
            groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
        })
        .expect("it fits");
    rig.sticks(RIGHT, 30);
    assert!(rig.feet().x > 2.0, "{}", rig.feet());
}
