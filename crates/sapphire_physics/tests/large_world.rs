//! What a zone's worth of static colliders does to a world's tree (M3 W1): the build's time, and the
//! depth against the limit `PhysicsWorld::new` refuses past. The simulation streams nothing, so a whole
//! zone's colliders go in one tree before its first step. The cooker may not link this crate, so a
//! zone's cook can't tell; `sapphire_colliders::MAX_COLLIDERS` is the most a set holds.
//!
//! `cargo test -p sapphire_physics --release --test large_world -- --nocapture` prints the numbers.

use std::time::Instant;

use glam::Vec3;
use sapphire_physics::{
    ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape, WorldError,
};

const SMALL_BOX: Shape = Shape::Box { half_extents: Vec3::new(0.5, 0.5, 0.5) };

fn collider(id: usize, position: Vec3) -> StaticCollider {
    StaticCollider {
        id: ColliderId(id as u32),
        shape: StaticShape::Convex(SMALL_BOX),
        pose: Pose::at(position),
        memberships: Group::World.mask(),
    }
}

/// A small xorshift, so a layout repeats.
struct Random(u64);

impl Random {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A town's layout: boxes in rows on a grid, in the space a zone takes, none on another.
fn grid(count: usize) -> Vec<StaticCollider> {
    let side = (count as f32).sqrt().ceil() as usize;
    let pitch = 8_000.0 / side as f32;
    (0..count)
        .map(|id| {
            let (x, z) = ((id % side) as f32, (id / side) as f32);
            collider(id, Vec3::new((x - side as f32 / 2.0) * pitch, 0.0, (z - side as f32 / 2.0) * pitch))
        })
        .collect()
}

/// A forest's: scattered at random over a zone, in clumps.
fn clumps(count: usize) -> Vec<StaticCollider> {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let centers: Vec<Vec3> = (0..64)
        .map(|_| {
            Vec3::new(
                random.unit() * 8_000.0 - 4_000.0,
                random.unit() * 40.0,
                random.unit() * 8_000.0 - 4_000.0,
            )
        })
        .collect();
    (0..count)
        .map(|id| {
            let offset = Vec3::new(random.unit() - 0.5, random.unit() - 0.5, random.unit() - 0.5) * 60.0;
            collider(id, centers[id % centers.len()] + offset)
        })
        .collect()
}

/// A tower's: all in one line, up.
fn stack(count: usize) -> Vec<StaticCollider> {
    let pitch = 16_000.0 / count as f32;
    (0..count).map(|id| collider(id, Vec3::new(0.0, id as f32 * pitch - 8_000.0, 0.0))).collect()
}

/// What no real zone is, the worst for a builder that splits on centers: every collider in one place.
fn pile(count: usize) -> Vec<StaticCollider> {
    (0..count).map(|id| collider(id, Vec3::ZERO)).collect()
}

/// A line whose spacing doubles, also no zone's: every split of it is lopsided.
fn doubling(count: usize) -> Vec<StaticCollider> {
    (0..count)
        .map(|id| collider(id, Vec3::new((1u32 << (id % 13)) as f32 + id as f32 * 0.001, 0.0, 0.0)))
        .collect()
}

/// A rolling ground of `side` × `side` quads, as one mesh collider.
fn ground(side: u32) -> StaticCollider {
    let at = |x: u32, z: u32| {
        let (x, z) = (x as f32, z as f32);
        Vec3::new(
            x - side as f32 / 2.0,
            ((x * 0.5).to_bits() % 7) as f32 * 0.1 + (z * 0.25) % 2.0,
            z - side as f32 / 2.0,
        )
    };
    let vertices = (0..=side).flat_map(|z| (0..=side).map(move |x| at(x, z))).collect();
    let index = |x: u32, z: u32| z * (side + 1) + x;
    let indices = (0..side)
        .flat_map(|z| {
            (0..side).flat_map(move |x| {
                [
                    [index(x, z), index(x, z + 1), index(x + 1, z)],
                    [index(x + 1, z), index(x, z + 1), index(x + 1, z + 1)],
                ]
            })
        })
        .collect();
    StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Mesh { vertices, indices },
        pose: Pose::at(Vec3::ZERO),
        memberships: Group::World.mask(),
    }
}

/// Builds a world from `colliders`, printing what it took and how deep its tree came out. A tree past
/// the limit is an answer here too, and prints as one.
fn measure(what: &str, colliders: Vec<StaticCollider>) -> Result<u32, WorldError> {
    let count = colliders.len();
    #[allow(clippy::disallowed_methods, reason = "a measurement to print, read by nothing in the simulation")]
    let start = Instant::now();
    let world = PhysicsWorld::new(colliders, 8);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    match &world {
        Ok(world) => eprintln!("{what} of {count}: {ms:.1} ms, depth {}", world.tree_depth()),
        Err(error) => eprintln!("{what} of {count}: {ms:.1} ms, {error}"),
    }
    world.map(|world| world.tree_depth())
}

/// What a real zone can look like, up to the most colliders a set can hold, must fit under the limit.
#[test]
fn a_zones_layouts_stay_within_the_depth_limit_at_any_size_a_set_can_hold() {
    for count in [1 << 10, 1 << 12, 1 << 14, 1 << 16] {
        for (what, layout) in [("grid", grid as fn(usize) -> _), ("clumps", clumps), ("stack", stack)] {
            let depth = measure(what, layout(count)).expect("the tree fits");
            let balanced = count.ilog2();
            assert!(depth <= 32 && depth >= balanced, "{what} of {count}: depth {depth}");
        }
    }
}

/// The largest mesh collider a world takes (262144 triangles) fits too, as a mesh's own tree has the limit.
#[test]
fn a_large_ground_mesh_stays_within_the_depth_limit() {
    let depth = measure("ground mesh of 2x362x362 triangles, as one collider", vec![ground(362)]);
    assert!(depth.is_ok(), "{depth:?}");
}

/// What the limit does past real layouts: either the tree fits or the world refuses by name. A world
/// must never be built that a query's stack can't walk.
#[test]
fn a_layout_no_zone_has_fits_or_is_refused_by_name() {
    for (what, layout) in [("pile", pile as fn(usize) -> _), ("doubling", doubling)] {
        for count in [1 << 10, 1 << 16] {
            match measure(what, layout(count)) {
                Ok(depth) => assert!(depth <= 32),
                Err(WorldError::TreeTooDeep { depth }) => assert!(depth > 32),
                Err(error) => panic!("{what} of {count}: {error}"),
            }
        }
    }
}
