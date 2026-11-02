//! The scene R6's test, the checksum and the bench share: 225 convex colliders and a mesh of 8,192
//! triangles, two character capsules with their hurtboxes, and 20 queries a step. Scripted by a
//! seeded generator, with nothing but IEEE arithmetic, so its floats depend only on the
//! architecture.

#![allow(dead_code, reason = "each test binary uses part of this")]

use glam::{Quat, Vec3};
use sapphire_physics::{
    BodyDesc, BodyId, ColliderId, CollisionGroups, Group, GroupMask, HitTarget, MoveOptions, PhysicsWorld,
    Pose, QueryFilter, Shape, ShapeHit, StaticCollider, StaticShape,
};

pub const CAPSULE: Shape = Shape::Capsule { half_height: 0.55, radius: 0.35 };
pub const CHARACTERS: [BodyId; 2] = [BodyId(1), BodyId(2)];
pub const DT: f32 = 1.0 / 60.0;

/// An xorshift generator: the same numbers everywhere.
pub struct Rng(pub u32);

impl Rng {
    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// In [`a`, `b`).
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        let unit = (self.next() >> 8) as f32 / (1u32 << 24) as f32;
        a + (b - a) * unit
    }
}

fn world_collider(id: u32, shape: Shape, position: Vec3, rotation: Quat) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id),
        shape: StaticShape::Convex(shape),
        pose: Pose::new(position, rotation),
        memberships: Group::World.mask(),
    }
}

/// The ground, 14 × 14 boxes, 12 spheres, 12 capsule pillars, walls, and a bumpy mesh patch.
pub fn statics() -> Vec<StaticCollider> {
    let mut rng = Rng(0x1234_5678);
    let mut out = Vec::new();
    let mut id = 0;
    let mut next_id = || {
        id += 1;
        id
    };
    out.push(world_collider(
        next_id(),
        Shape::Box { half_extents: Vec3::new(40.0, 0.5, 40.0) },
        Vec3::new(0.0, -0.5, 0.0),
        Quat::IDENTITY,
    ));
    for x in 0..14 {
        for z in 0..14 {
            let half = Vec3::new(rng.range(0.4, 1.2), rng.range(0.3, 1.8), rng.range(0.4, 1.2));
            // A turn about Y as a normalized quaternion, so no sine goes into the scene.
            let rotation = Quat::from_xyzw(0.0, rng.range(-0.38, 0.38), 0.0, 0.92).normalize();
            let position = Vec3::new(
                (x as f32 - 6.5) * 4.0 + rng.range(-0.5, 0.5),
                half.y,
                (z as f32 - 6.5) * 4.0 + rng.range(-0.5, 0.5),
            );
            out.push(world_collider(next_id(), Shape::Box { half_extents: half }, position, rotation));
        }
    }
    for i in 0..12 {
        let radius = rng.range(0.3, 0.9);
        let position = Vec3::new(rng.range(-26.0, 26.0), radius, rng.range(-26.0, 26.0));
        out.push(world_collider(next_id(), Shape::Sphere { radius }, position, Quat::IDENTITY));
        let half_height = rng.range(0.5, 2.0);
        let radius = rng.range(0.2, 0.5);
        let position = Vec3::new(
            (i as f32 - 5.5) * 4.0 + 2.0,
            half_height + radius,
            ((i * 7 % 12) as f32 - 5.5) * 4.0 + 2.0,
        );
        out.push(world_collider(next_id(), Shape::Capsule { half_height, radius }, position, Quat::IDENTITY));
    }
    for (position, half) in [
        (Vec3::new(30.0, 2.0, 0.0), Vec3::new(0.5, 2.0, 30.0)),
        (Vec3::new(-30.0, 2.0, 0.0), Vec3::new(0.5, 2.0, 30.0)),
        (Vec3::new(0.0, 2.0, 30.0), Vec3::new(30.0, 2.0, 0.5)),
        (Vec3::new(0.0, 2.0, -30.0), Vec3::new(30.0, 2.0, 0.5)),
    ] {
        out.push(world_collider(next_id(), Shape::Box { half_extents: half }, position, Quat::IDENTITY));
    }
    out.push(StaticCollider {
        id: ColliderId(next_id()),
        shape: mesh_patch(64),
        pose: Pose::at(Vec3::new(-16.0, 0.0, -16.0)),
        memberships: Group::World.mask(),
    });
    out
}

/// `n` × `n` quads of half a metre, gently bumped by a fixed pattern.
fn mesh_patch(n: u32) -> StaticShape {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for z in 0..=n {
        for x in 0..=n {
            let height = ((x * 7 + z * 13) % 5) as f32 * 0.03;
            vertices.push(Vec3::new(x as f32 * 0.5, height, z as f32 * 0.5));
        }
    }
    let stride = n + 1;
    for z in 0..n {
        for x in 0..n {
            let a = z * stride + x;
            let (b, c, d) = (a + 1, a + stride, a + stride + 1);
            indices.push([a, c, b]);
            indices.push([b, c, d]);
        }
    }
    StaticShape::Mesh { vertices, indices }
}

pub fn character_groups() -> CollisionGroups {
    CollisionGroups::new(Group::Character.mask(), Group::World | Group::Character | Group::CharacterBlocker)
}

fn hurtbox_id(character: usize, part: usize) -> BodyId {
    BodyId(100 + (character * 2 + part) as u32)
}

/// The two capsules, each with two hurtboxes that follow it.
pub fn bodies() -> Vec<BodyDesc> {
    let mut out = Vec::new();
    for (character, x) in [0.0f32, 4.0].into_iter().enumerate() {
        out.push(BodyDesc {
            id: CHARACTERS[character],
            shape: CAPSULE,
            pose: Pose::at(Vec3::new(x, 1.2, 0.0)),
            groups: character_groups(),
        });
        for part in 0..2 {
            out.push(BodyDesc {
                id: hurtbox_id(character, part),
                shape: Shape::Sphere { radius: 0.3 },
                pose: Pose::at(Vec3::new(x, 1.0 + part as f32 * 0.6, 0.0)),
                groups: CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE),
            });
        }
    }
    out
}

pub fn world() -> PhysicsWorld {
    let mut world = PhysicsWorld::new(statics(), 16).expect("the scene builds");
    for body in bodies() {
        world.add_body(body).expect("the bodies fit");
    }
    world
}

/// One step's script: motion, and the buffers its queries fill.
pub struct Script {
    rng: Rng,
    velocity: [Vec3; 2],
    hits: [ShapeHit; 4],
    overlaps: [HitTarget; 16],
}

impl Default for Script {
    fn default() -> Self {
        Self {
            rng: Rng(0xCAFE_F00D),
            velocity: [Vec3::ZERO; 2],
            hits: [ShapeHit::NONE; 4],
            overlaps: [HitTarget::Body(BodyId(0)); 16],
        }
    }
}

const RAYS: [Vec3; 8] = [
    Vec3::new(0.0, -1.0, 0.0),
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(-1.0, 0.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
    Vec3::new(0.0, 0.0, -1.0),
    Vec3::new(0.6, -0.8, 0.0),
    Vec3::new(-0.6, -0.8, 0.0),
    Vec3::new(0.0, 0.6, 0.8),
];

const OVERLAP_OFFSETS: [Vec3; 5] = [
    Vec3::new(1.0, 0.5, 0.0),
    Vec3::new(-1.0, 0.5, 0.0),
    Vec3::new(0.0, 0.5, 1.0),
    Vec3::new(0.0, 0.5, -1.0),
    Vec3::new(2.0, 1.0, 2.0),
];

fn write_vec3(value: Vec3, log: &mut dyn FnMut(&[u8])) {
    for component in value.to_array() {
        log(&component.to_bits().to_le_bytes());
    }
}

fn write_target(target: HitTarget, log: &mut dyn FnMut(&[u8])) {
    let (kind, id) = match target {
        HitTarget::Static(id) => (0u32, id.0),
        HitTarget::Body(id) => (1u32, id.0),
    };
    log(&kind.to_le_bytes());
    log(&id.to_le_bytes());
}

fn write_shape_hit(hit: &ShapeHit, log: &mut dyn FnMut(&[u8])) {
    write_target(hit.target, log);
    log(&hit.distance.to_bits().to_le_bytes());
    log(&hit.fraction.to_bits().to_le_bytes());
    write_vec3(hit.point, log);
    write_vec3(hit.normal, log);
    log(&[u8::from(hit.started_overlapping)]);
}

impl Script {
    /// Runs step `tick`: two moves, the hurtboxes following them, then 8 rays, 6 shape casts and 6
    /// overlaps from one capsule, handing every result to `log` as canonical bytes. Every 97 steps
    /// a capsule teleports into a pillar's path, so overlaps get resolved as well.
    pub fn step(&mut self, world: &mut PhysicsWorld, tick: u32, log: &mut dyn FnMut(&[u8])) {
        let options = MoveOptions::default();
        if tick % 97 == 96 {
            let character = CHARACTERS[(tick / 97) as usize % 2];
            // Half a metre sideways and down, into whatever's there and the ground.
            let pose = world.body_pose(character).expect("the capsule is there");
            let nudged = Pose::at(pose.position + Vec3::new(0.5, -0.4, 0.0));
            world.set_body_pose(character, nudged).expect("a valid pose");
        }
        for (index, &character) in CHARACTERS.iter().enumerate() {
            if tick.is_multiple_of(30) {
                let x = self.rng.range(-1.0, 1.0);
                let z = self.rng.range(-1.0, 1.0);
                self.velocity[index] = Vec3::new(x, 0.0, z).normalize_or_zero() * 6.0;
            }
            let motion = (self.velocity[index] + Vec3::new(0.0, -6.0, 0.0)) * DT;
            let outcome = world.move_and_collide(character, motion, &options).expect("a capsule moves");
            write_vec3(outcome.position, log);
            write_vec3(outcome.remaining, log);
            write_vec3(outcome.overlap_push, log);
            for hit in outcome.hits() {
                write_target(hit.target, log);
                write_vec3(hit.normal, log);
                write_vec3(hit.point, log);
                log(&hit.fraction.to_bits().to_le_bytes());
            }
            for part in 0..2 {
                let hurtbox = Pose::at(outcome.position + Vec3::new(0.0, -0.2 + part as f32 * 0.6, 0.0));
                world.set_body_pose(hurtbox_id(index, part), hurtbox).expect("a valid pose");
            }
        }

        let asker = CHARACTERS[(tick % 2) as usize];
        let from = world.body_pose(asker).expect("the capsule is there").position;
        let scenery = QueryFilter::new(Group::World | Group::Character).excluding(asker);
        let hurtboxes = QueryFilter::new(Group::Hurtbox.mask());

        for direction in RAYS {
            if let Some(hit) = world.cast_ray(from + Vec3::new(0.0, 0.2, 0.0), direction, 30.0, &scenery) {
                write_target(hit.target, log);
                log(&hit.distance.to_bits().to_le_bytes());
                write_vec3(hit.point, log);
                write_vec3(hit.normal, log);
            }
        }

        // A camera's sphere, a capsule looking ahead, a box, and a sweep over hurtboxes.
        let above = Pose::at(from + Vec3::new(0.0, 1.5, 0.0));
        let casts = [
            (Shape::Sphere { radius: 0.3 }, Vec3::X),
            (Shape::Sphere { radius: 0.3 }, Vec3::NEG_Z),
            (CAPSULE, Vec3::new(0.6, 0.0, 0.8)),
            (CAPSULE, Vec3::NEG_Y),
            (Shape::Box { half_extents: Vec3::new(0.3, 0.3, 0.6) }, Vec3::NEG_X),
        ];
        for (shape, direction) in casts {
            if let Some(hit) = world.cast_shape(&shape, above, direction, 6.0, &scenery) {
                write_shape_hit(&hit, log);
            }
        }
        let swing = world.cast_shape_all(
            &Shape::Sphere { radius: 0.4 },
            Pose::at(from + Vec3::new(-1.0, 0.5, 0.0)),
            Vec3::X,
            6.0,
            &hurtboxes,
            &mut self.hits,
        );
        log(&(swing.found as u32).to_le_bytes());
        for hit in &self.hits[..swing.written] {
            write_shape_hit(hit, log);
        }

        let shapes = [
            Shape::Sphere { radius: 0.75 },
            CAPSULE,
            Shape::Box { half_extents: Vec3::splat(0.5) },
            Shape::Sphere { radius: 2.0 },
            CAPSULE,
        ];
        for (shape, offset) in shapes.into_iter().zip(OVERLAP_OFFSETS) {
            let found = world.overlap(&shape, Pose::at(from + offset), &scenery, &mut self.overlaps);
            log(&(found.found as u32).to_le_bytes());
            for &target in &self.overlaps[..found.written] {
                write_target(target, log);
            }
        }
        if let Some(deepest) = world.penetration(&Shape::Sphere { radius: 0.75 }, Pose::at(from), &scenery) {
            write_target(deepest.target, log);
            write_vec3(deepest.normal, log);
            log(&deepest.depth.to_bits().to_le_bytes());
            write_vec3(deepest.point, log);
        }
    }
}
