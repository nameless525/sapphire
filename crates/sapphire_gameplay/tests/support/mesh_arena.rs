//! A mesh arena for what holds whatever the geometry: the controller's allocation test, its pinned
//! checksum and its bench. 8,192 triangles of gently bumped ground, with a ramp, a stair, a slope too
//! steep to stand on, walls and crates, and characters that differ in every way a profile can.

#![allow(dead_code, reason = "each test binary uses part of this")]

use sapphire_gameplay::{ActorActions, ActorId, Buttons, ControllerProfile, Stick, TickActions};
use sapphire_math::{Vec2, Vec3};
use sapphire_physics::{ColliderId, Group, GroupMask, Pose, Shape, StaticCollider, StaticShape};

const WORLD: GroupMask = Group::World.mask();

/// How many metres across the ground is, centered on the origin.
pub const SIZE: f32 = 32.0;

fn solid(id: u32, min: Vec3, max: Vec3, memberships: GroupMask) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(Shape::Box { half_extents: (max - min) / 2.0 }),
        pose: Pose::at((min + max) / 2.0),
        memberships,
    }
}

fn sheet(id: u32, corners: [Vec3; 4]) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Mesh { vertices: corners.to_vec(), indices: vec![[0, 2, 1], [0, 3, 2]] },
        pose: Pose::IDENTITY,
        memberships: WORLD,
    }
}

/// 64 × 64 quads of half a metre, bumped by a fixed pattern up to 12 cm: 8,192 triangles.
fn ground() -> StaticCollider {
    const N: u32 = 64;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for z in 0..=N {
        for x in 0..=N {
            let height = ((x * 7 + z * 13) % 5) as f32 * 0.03;
            vertices.push(Vec3::new(x as f32 * 0.5 - SIZE / 2.0, height, z as f32 * 0.5 - SIZE / 2.0));
        }
    }
    let stride = N + 1;
    for z in 0..N {
        for x in 0..N {
            let a = z * stride + x;
            let (b, c, d) = (a + 1, a + stride, a + stride + 1);
            indices.push([a, c, b]);
            indices.push([b, c, d]);
        }
    }
    StaticCollider {
        id: ColliderId(1),
        shape: StaticShape::Mesh { vertices, indices },
        pose: Pose::IDENTITY,
        memberships: WORLD,
    }
}

/// The ground and everything on it.
pub fn statics() -> Vec<StaticCollider> {
    let half = SIZE / 2.0;
    let mut out = vec![ground()];
    // Walls around it, for characters alone.
    let blocker = Group::CharacterBlocker.mask();
    out.push(solid(2, Vec3::new(-half, 0.0, half), Vec3::new(half, 4.0, half + 1.0), blocker));
    out.push(solid(3, Vec3::new(-half, 0.0, -half - 1.0), Vec3::new(half, 4.0, -half), blocker));
    out.push(solid(4, Vec3::new(half, 0.0, -half), Vec3::new(half + 1.0, 4.0, half), blocker));
    out.push(solid(5, Vec3::new(-half - 1.0, 0.0, -half), Vec3::new(-half, 4.0, half), blocker));
    // A walkable ramp, up along +x to a platform, and a stair down its far side.
    out.push(sheet(
        6,
        [
            Vec3::new(2.0, 0.0, -8.0),
            Vec3::new(6.0, 2.0, -8.0),
            Vec3::new(6.0, 2.0, -4.0),
            Vec3::new(2.0, 0.0, -4.0),
        ],
    ));
    out.push(solid(7, Vec3::new(6.0, 0.0, -8.0), Vec3::new(9.0, 2.0, -4.0), WORLD));
    for step in 0..7 {
        let x = 9.0 + step as f32 * 0.4;
        let top = 2.0 - 0.25 * (step + 1) as f32;
        out.push(solid(8 + step, Vec3::new(x, 0.0, -8.0), Vec3::new(x + 0.4, top, -4.0), WORLD));
    }
    // Too steep to stand on.
    out.push(sheet(
        20,
        [
            Vec3::new(-9.0, 0.0, 4.0),
            Vec3::new(-6.0, 6.0, 4.0),
            Vec3::new(-6.0, 6.0, 9.0),
            Vec3::new(-9.0, 0.0, 9.0),
        ],
    ));
    // Things to run into and over.
    for index in 0..12 {
        let x = -10.0 + index as f32 * 1.9;
        let z = if index % 2 == 0 { 0.5 } else { -1.5 };
        let height = 0.15 + (index % 4) as f32 * 0.25;
        let corner = Vec3::new(x, 0.0, z);
        out.push(solid(30 + index, corner, corner + Vec3::new(0.8, height, 0.8), WORLD));
    }
    out
}

/// Three profiles, so no two characters move alike.
pub fn profiles() -> [ControllerProfile; 3] {
    [
        ControllerProfile::default(),
        ControllerProfile {
            height: 1.9,
            radius: 0.45,
            run_speed: 5.0,
            acceleration: 60.0,
            step_height: 0.4,
            ground_snap: 0.4,
            stance_radius: 0.2,
            coyote_ticks: 2,
            ..ControllerProfile::default()
        },
        ControllerProfile {
            height: 1.4,
            radius: 0.25,
            run_speed: 7.5,
            acceleration: 120.0,
            deceleration: 80.0,
            gravity: 30.0,
            jump_speed: 9.0,
            slope_limit_degrees: 40.0,
            step_height: 0.2,
            ..ControllerProfile::default()
        },
    ]
}

/// Where the `index`th character starts, and which profile it has.
pub fn start(index: usize) -> (Vec3, usize) {
    let across = (index % 4) as f32 * 3.0 - 4.5;
    let down = (index / 4) as f32 * 3.0 - 11.0;
    (Vec3::new(across + 0.17 * index as f32, 0.2 + 0.1 * (index % 3) as f32, down), index % 3)
}

/// What the `index`th character does at a tick: a stick that sweeps on its own rhythm, stopping now
/// and then, and a jump every so often.
pub fn input(tick: u64, index: usize) -> (Vec2, bool) {
    let shifted = tick + index as u64 * 37;
    let stick = if shifted % 240 < 200 {
        // A triangle wave for each axis, with no float rounding to speak of.
        let wave = |period: u64| {
            let at = shifted % period;
            let half = period / 2;
            (if at < half { at as f32 } else { (period - at) as f32 }) / half as f32 * 2.0 - 1.0
        };
        Vec2::new(wave(311 + index as u64 * 16), wave(197 + index as u64 * 22))
    } else {
        Vec2::ZERO
    };
    (stick, shifted % 83 == 11)
}

/// One tick's input for the first actors a tick can hold, the rest standing idle.
pub fn tick_actions(tick: u64, actors: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    for (index, &actor) in actors.iter().take(TickActions::CAPACITY).enumerate() {
        let (stick, jump) = input(tick, index);
        let buttons = if jump { Buttons::JUMP } else { Buttons::NONE };
        let stick = Stick::from_axes(stick.x, stick.y);
        actions.set(actor, ActorActions { buttons, stick }).expect("few enough actors");
    }
    actions
}
