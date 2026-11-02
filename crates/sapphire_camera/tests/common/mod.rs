//! What the tests share: worlds, the scripted motion, and how to read a pose against them. Scripts are
//! functions of time, so runs at different frame rates see the same motion.

#![allow(dead_code, reason = "each test binary uses part of this")]

use sapphire_camera::{CameraDirector, CameraFrame, CameraPose, CameraTuning, Rig, Subject};
use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_math::{Quat, Vec2, Vec3};
use sapphire_physics::{
    ColliderId, Group, PhysicsWorld, Pose, QueryFilter, Shape, StaticCollider, StaticShape,
};

pub const ASPECT: f32 = 16.0 / 9.0;
pub const RATES: [u32; 3] = [30, 60, 120];

const ARENA: &str = include_str!("../../../../content/physics/test_arena.colliders.ron");

/// The committed test arena: ground, a crate, a pillar, a ball and a ramp, with walls only characters
/// meet.
pub fn arena() -> PhysicsWorld {
    let document = ColliderDocument::from_ron(ARENA).expect("the document reads");
    let bytes = document.cook().expect("it cooks").to_cooked().expect("it writes");
    let set = ColliderSet::read(&bytes).expect("the cooked set reads");
    PhysicsWorld::from_cooked(set, 0).expect("the world builds")
}

fn collider(id: u32, shape: Shape, position: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(shape),
        pose: Pose::at(position),
        memberships: Group::World.mask(),
    }
}

/// A ground wide enough never to leave, its top at y = 0, and `extra` boxes and capsules.
pub fn ground_with(extra: &[(Shape, Vec3)]) -> PhysicsWorld {
    let ground = (Shape::Box { half_extents: Vec3::new(400.0, 1.0, 400.0) }, Vec3::new(0.0, -1.0, 0.0));
    let colliders = std::iter::once(&ground)
        .chain(extra)
        .enumerate()
        .map(|(id, (shape, position))| collider(id as u32, *shape, *position))
        .collect();
    PhysicsWorld::new(colliders, 0).expect("the world builds")
}

pub fn open_ground() -> PhysicsWorld {
    ground_with(&[])
}

pub fn player_at(position: Vec3) -> Subject {
    Subject { position, height: 1.7, radius: 0.35 }
}

pub fn enemy_at(position: Vec3) -> Subject {
    Subject { position, height: 2.0, radius: 0.45 }
}

/// A moment of a script.
#[derive(Clone, Copy, Debug)]
pub struct Moment {
    pub player: Subject,
    pub player_yaw: f32,
    pub target: Option<Subject>,
    /// Look input as a rate, in radians a second: yaw toward +X, then pitch down.
    pub look_rate: Vec2,
}

impl Moment {
    pub fn alone(position: Vec3) -> Self {
        Self { player: player_at(position), player_yaw: 0.0, target: None, look_rate: Vec2::ZERO }
    }

    pub fn against(position: Vec3, target: Vec3) -> Self {
        Self { target: Some(enemy_at(target)), ..Self::alone(position) }
    }

    /// As a frame of `dt` seconds.
    pub fn frame(&self, dt: f32) -> CameraFrame {
        CameraFrame {
            dt,
            player: self.player,
            player_yaw: self.player_yaw,
            target: self.target,
            look: self.look_rate * dt,
            zoom: 0.0,
            aspect: ASPECT,
        }
    }
}

pub fn director() -> CameraDirector {
    CameraDirector::new(CameraTuning::default_tuning(), 7).expect("the shipped tuning is valid")
}

pub fn combat_director() -> CameraDirector {
    let mut camera = director();
    camera.set_rig(Rig::Combat, true);
    camera
}

pub fn lock_on_director() -> CameraDirector {
    let mut camera = combat_director();
    camera.set_rig(Rig::LockOn, true);
    camera
}

/// Runs `script` from `from` to `to` seconds at `rate` frames a second, starting on a whole frame, and
/// calls `each` with the time and pose of every frame.
pub fn run(
    camera: &mut CameraDirector,
    world: &PhysicsWorld,
    rate: u32,
    (from, to): (f32, f32),
    script: impl Fn(f32) -> Moment,
    mut each: impl FnMut(f32, &CameraPose, &Moment),
) {
    let dt = 1.0 / rate as f32;
    let frames = ((to - from) * rate as f32).round() as u32;
    let start = (from * rate as f32).round() as u32;
    for frame in 1..=frames {
        let time = (start + frame) as f32 / rate as f32;
        let moment = script(time);
        let pose = camera.update(&moment.frame(dt), world);
        each(time, &pose, &moment);
    }
}

/// Where `point` lands on the screen: −1 to 1 each way, and its depth along the view.
pub fn screen_of(pose: &CameraPose, point: Vec3) -> (f32, f32, f32) {
    let offset = point - pose.eye;
    let forward = pose.rotation * Vec3::Z;
    let right = pose.rotation * Vec3::NEG_X;
    let up = pose.rotation * Vec3::Y;
    let depth = offset.dot(forward);
    let half = (pose.fov_y * 0.5).tan();
    (offset.dot(right) / (depth * half * ASPECT), offset.dot(up) / (depth * half), depth)
}

/// How near the frame's edge a subject comes, across and up: its feet and head, each a radius wide. A
/// point behind the eye reads as far outside.
pub fn frame_use(pose: &CameraPose, subject: &Subject) -> Vec2 {
    let mut worst = Vec2::ZERO;
    for point in [subject.position, subject.position + Vec3::Y * subject.height] {
        let (x, y, depth) = screen_of(pose, point);
        if depth <= 0.05 {
            return Vec2::splat(f32::MAX);
        }
        let half = (pose.fov_y * 0.5).tan();
        let across = x.abs() + subject.radius / (depth * half * ASPECT);
        worst = worst.max(Vec2::new(across, y.abs()));
    }
    worst
}

/// The filter and sphere the camera's own collision uses.
pub fn eye_overlaps(world: &PhysicsWorld, pose: &CameraPose, radius: f32) -> Option<f32> {
    let filter = QueryFilter::new(Group::World.mask() | Group::CameraBlocker.mask());
    world.penetration(&Shape::Sphere { radius }, Pose::at(pose.eye), &filter).map(|deep| deep.depth)
}

/// How fast a pose is moving and how fast that's changing, from frame to frame. All per second, so
/// the numbers read the same at any frame rate.
#[derive(Clone, Copy, Debug, Default)]
pub struct Motion {
    last: Option<(Vec3, Quat)>,
    last_speed: Option<(Vec3, Vec3)>,
    /// Metres a second.
    pub top_speed: f32,
    /// Radians a second.
    pub top_turn: f32,
    /// Metres a second squared.
    pub top_acceleration: f32,
    /// Radians a second squared.
    pub top_turn_acceleration: f32,
}

impl Motion {
    pub fn add(&mut self, pose: &CameraPose, dt: f32) {
        let now = (pose.eye, pose.rotation);
        if let Some((eye, rotation)) = self.last {
            let speed = (now.0 - eye) / dt;
            // The rotation from the last frame's to this one's, as a vector: its length the angle.
            let change = now.1 * rotation.inverse();
            let change = if change.w < 0.0 { -change } else { change };
            let turn = Vec3::new(change.x, change.y, change.z) * (2.0 / dt);
            self.top_speed = self.top_speed.max(speed.length());
            self.top_turn = self.top_turn.max(turn.length());
            if let Some((last_speed, last_turn)) = self.last_speed {
                self.top_acceleration = self.top_acceleration.max((speed - last_speed).length() / dt);
                self.top_turn_acceleration = self.top_turn_acceleration.max((turn - last_turn).length() / dt);
            }
            self.last_speed = Some((speed, turn));
        }
        self.last = Some(now);
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A value holding `keys`' values with smooth moves between: (time, value).
fn keyed(keys: &[(f32, f32)], t: f32) -> f32 {
    for pair in keys.windows(2) {
        if t < pair[1].0 {
            return pair[0].1 + (pair[1].1 - pair[0].1) * smooth((t - pair[0].0) / (pair[1].0 - pair[0].0));
        }
    }
    keys[keys.len() - 1].1
}

/// How long [`fight`] runs.
pub const FIGHT_SECONDS: f32 = 30.0;

/// A scripted duel on open ground: both fighters on the move, the player dashing twice, the enemy
/// circling at a distance, backing off, lunging in, hopping, and sprinting around the player.
pub fn fight(t: f32) -> Moment {
    // Walking about, up to about 3 m/s.
    let walk = Vec3::new(6.0 * (0.31 * t).sin(), 0.0, 5.0 * (0.47 * t + 1.0).sin());
    // Two dashes of a few metres, up to 15 m/s.
    let dash =
        Vec3::X * 4.0 * smooth((t - 6.0) / 0.4) + Vec3::new(-2.0, 0.0, 3.0) * smooth((t - 14.0) / 0.35);
    let player = walk + dash;

    let reach = keyed(
        &[
            (0.0, 3.5),
            (5.0, 3.5),
            (9.0, 9.0),
            (12.0, 9.0),
            (13.0, 1.8),
            (20.0, 1.8),
            (24.0, 7.0),
            (FIGHT_SECONDS, 7.0),
        ],
        t,
    );
    let angle = 1.0 * t + 0.8 * (0.5 * t).sin() + 2.2 * smooth((t - 13.0) / 2.0) * (t - 13.0).clamp(0.0, 7.0);
    let hop = 2.5 * (((t - 16.0) / 1.0).clamp(0.0, 1.0) * std::f32::consts::PI).sin();
    let enemy = player + Vec3::new(reach * angle.sin(), hop, reach * angle.cos());

    let velocity = walk_heading(t);
    Moment { player_yaw: velocity, ..Moment::against(player, enemy) }
}

/// The way the walk heads, which the player faces.
fn walk_heading(t: f32) -> f32 {
    let x = 6.0 * 0.31 * (0.31 * t).cos();
    let z = 5.0 * 0.47 * (0.47 * t + 1.0).cos();
    x.atan2(z)
}

/// Look input a player might give: both ways, up and down, in sweeps.
pub fn sweeping_look(t: f32) -> Vec2 {
    Vec2::new(1.4 * (0.7 * t).sin(), 0.6 * (1.3 * t + 0.5).sin())
}

/// Walking through waypoints at `speed`, as a position at `t`.
pub fn walk_through(points: &[Vec2], speed: f32, t: f32) -> Vec2 {
    let mut left = (t * speed).max(0.0);
    for pair in points.windows(2) {
        let leg = pair[0].distance(pair[1]);
        if left < leg {
            return pair[0].lerp(pair[1], left / leg);
        }
        left -= leg;
    }
    points[points.len() - 1]
}
