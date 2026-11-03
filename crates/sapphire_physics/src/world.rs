//! The world's contents: static colliders under a tree, and bodies sorted by ID.

use glam::{Quat, Vec3};
use parry3d::bounding_volume::Aabb;
use parry3d::math::Pose as ParryPose;
use parry3d::partitioning::{Bvh, BvhBuildStrategy};
use parry3d::shape::{TriMesh, TriMeshFlags};
use sapphire_colliders::document::{check_mesh, check_position};
use sapphire_colliders::{ArchivedColliderSet, ArchivedCookedShape, CollisionGroups, GroupMask};
use xxhash_rust::xxh3::Xxh3;

use crate::shape::{Convex, Pose, Shape};
use crate::stats::{Counters, PhysicsStats};

/// The deepest tree a traversal walks without its stack leaving the stack: `parry3d` keeps 32
/// entries inline, and pushes at most one per level.
const MAX_TREE_DEPTH: u32 = 32;

/// A static collider's stable ID. For a cooked set, its place in the set ([`ColliderSet`]'s
/// colliders are sorted by name, so this doesn't depend on how a document lists them).
///
/// [`ColliderSet`]: sapphire_colliders::ColliderSet
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColliderId(pub u32);

/// A kinematic body's ID, chosen by whoever adds it, normally the actor's stable ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BodyId(pub u32);

/// What a query hit. Sorts statics before bodies, then by ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HitTarget {
    Static(ColliderId),
    Body(BodyId),
}

/// A static collider, for building a world from code. Cooked documents go through
/// [`PhysicsWorld::from_cooked`].
#[derive(Clone, Debug, PartialEq)]
pub struct StaticCollider {
    pub id: ColliderId,
    pub shape: StaticShape,
    pub pose: Pose,
    pub memberships: GroupMask,
}

/// A static collider's geometry.
#[derive(Clone, Debug, PartialEq)]
pub enum StaticShape {
    Convex(Shape),
    /// A two-sided triangle mesh, in its collider's space.
    Mesh {
        vertices: Vec<Vec3>,
        indices: Vec<[u32; 3]>,
    },
}

/// A kinematic body to add.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyDesc {
    pub id: BodyId,
    pub shape: Shape,
    pub pose: Pose,
    pub groups: CollisionGroups,
}

/// A world that can't be built, or a body it refuses.
#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error("two static colliders have ID {}", .0.0)]
    DuplicateCollider(ColliderId),
    #[error("static collider {}: {problem}", id.0)]
    InvalidCollider { id: ColliderId, problem: String },
    /// A tree this deep would make its traversals allocate, against R6.
    #[error("the static colliders' tree is {depth} deep, past the {MAX_TREE_DEPTH} that stay off the heap")]
    TreeTooDeep { depth: u32 },
    #[error("a body with ID {} is already in the world", .0.0)]
    DuplicateBody(BodyId),
    #[error("no body has ID {}", .0.0)]
    UnknownBody(BodyId),
    #[error("body {}'s shape or pose isn't finite or is too small", .0.0)]
    InvalidBody(BodyId),
    /// Growing would allocate in the fixed step, so the world refuses instead.
    #[error("the world holds at most {capacity} bodies")]
    TooManyBodies { capacity: usize },
    /// Only spheres and capsules can move: see [`PhysicsWorld::move_and_collide`].
    #[error("body {} is a box, which `move_and_collide` can't resolve overlaps for", .0.0)]
    CannotMove(BodyId),
}

pub(crate) enum StaticGeometry {
    Convex(Convex),
    /// Boxed: a mesh's fields are far larger than a convex shape, which most colliders are.
    Mesh(Box<TriMesh>),
}

pub(crate) struct StaticEntry {
    pub(crate) id: ColliderId,
    pub(crate) geometry: StaticGeometry,
    pub(crate) pose: ParryPose,
    pub(crate) memberships: GroupMask,
    pub(crate) aabb: Aabb,
}

#[derive(Clone, Copy)]
pub(crate) struct BodyEntry {
    pub(crate) id: BodyId,
    pub(crate) shape: Shape,
    pub(crate) convex: Convex,
    pub(crate) pose: Pose,
    pub(crate) parry_pose: ParryPose,
    pub(crate) groups: CollisionGroups,
    pub(crate) aabb: Aabb,
}

impl BodyEntry {
    pub(crate) fn accepts_position(&self, position: Vec3) -> bool {
        if !position.is_finite() {
            return false;
        }
        let pose = ParryPose::from_parts(position, self.parry_pose.rotation);
        let aabb = self.convex.aabb(&pose);
        aabb.mins.is_finite() && aabb.maxs.is_finite()
    }

    /// The caller validates the pose's components before deriving geometry. Commit all fields
    /// together only after the derived bounds are checked, reusing those bounds for storage.
    pub(crate) fn set_pose(&mut self, pose: Pose) -> Result<(), WorldError> {
        let parry_pose = pose.to_parry();
        let aabb = self.convex.aabb(&parry_pose);
        if !aabb.mins.is_finite() || !aabb.maxs.is_finite() {
            return Err(WorldError::InvalidBody(self.id));
        }
        self.pose = pose;
        self.parry_pose = parry_pose;
        self.aabb = aabb;
        Ok(())
    }
}

/// The collision world: see the [crate docs](crate).
pub struct PhysicsWorld {
    pub(crate) statics: Vec<StaticEntry>,
    pub(crate) tree: Bvh,
    /// Sorted by ID. Holds at most `body_capacity` entries, within its reserved space.
    pub(crate) bodies: Vec<BodyEntry>,
    pub(crate) counters: Counters,
    body_capacity: usize,
    statics_digest: u64,
}

impl PhysicsWorld {
    /// Builds a world from static colliders, in any order, with room for `body_capacity` bodies.
    pub fn new(mut statics: Vec<StaticCollider>, body_capacity: usize) -> Result<Self, WorldError> {
        profiling::scope!("PhysicsWorld::new");
        statics.sort_by_key(|collider| collider.id);
        if let Some(pair) = statics.windows(2).find(|pair| pair[0].id == pair[1].id) {
            return Err(WorldError::DuplicateCollider(pair[0].id));
        }

        let mut digest = Xxh3::new();
        let mut entries = Vec::with_capacity(statics.len());
        let mut aabbs = Vec::with_capacity(statics.len());
        for collider in statics {
            let entry = build_static(collider, &mut digest)?;
            aabbs.push(entry.aabb);
            entries.push(entry);
        }

        // Built from the sorted list, so the same colliders give the same tree. The binned builder
        // needs no optimization pass, the one part of `parry3d`'s trees that takes a logarithm.
        let tree = Bvh::from_leaves(BvhBuildStrategy::Binned, &aabbs);
        check_tree_depth(&tree)?;
        for entry in &entries {
            if let StaticGeometry::Mesh(mesh) = &entry.geometry {
                check_tree_depth(mesh.bvh())?;
            }
        }

        Ok(Self {
            statics: entries,
            tree,
            bodies: Vec::with_capacity(body_capacity),
            counters: Counters::default(),
            body_capacity,
            statics_digest: digest.digest(),
        })
    }

    /// Builds a world from a cooked set, which `ColliderSet::read` has checked. A collider's ID is
    /// its place in the set.
    pub fn from_cooked(set: &ArchivedColliderSet, body_capacity: usize) -> Result<Self, WorldError> {
        let statics = set
            .colliders
            .iter()
            .enumerate()
            .map(|(index, collider)| {
                let id = ColliderId(index as u32);
                let shape = match &collider.shape {
                    ArchivedCookedShape::Box { half_extents } => StaticShape::Convex(Shape::Box {
                        half_extents: Vec3::from_array(half_extents.map(|v| v.to_native())),
                    }),
                    ArchivedCookedShape::Sphere { radius } => {
                        StaticShape::Convex(Shape::Sphere { radius: radius.to_native() })
                    }
                    ArchivedCookedShape::Capsule { half_height, radius } => {
                        StaticShape::Convex(Shape::Capsule {
                            half_height: half_height.to_native(),
                            radius: radius.to_native(),
                        })
                    }
                    ArchivedCookedShape::Mesh { mesh } => {
                        let mesh = set.meshes.get(mesh.to_native() as usize).ok_or_else(|| {
                            WorldError::InvalidCollider {
                                id,
                                problem: "names a mesh the set lacks".to_owned(),
                            }
                        })?;
                        StaticShape::Mesh {
                            vertices: (0..mesh.vertices.len())
                                .map(|i| Vec3::from_array(mesh.vertex(i)))
                                .collect(),
                            indices: (0..mesh.indices.len()).map(|i| mesh.triangle(i)).collect(),
                        }
                    }
                };
                Ok(StaticCollider {
                    id,
                    shape,
                    pose: Pose::new(
                        Vec3::from_array(collider.position()),
                        Quat::from_array(collider.rotation()),
                    ),
                    memberships: GroupMask::from_bits(collider.memberships.to_native()),
                })
            })
            .collect::<Result<Vec<_>, WorldError>>()?;
        Self::new(statics, body_capacity)
    }

    /// How deep its tree of statics goes: past `MAX_TREE_DEPTH`, [`Self::new`] would have refused it.
    pub fn tree_depth(&self) -> u32 {
        if self.tree.is_empty() { 0 } else { self.tree.subtree_depth(0) }
    }

    pub fn static_count(&self) -> usize {
        self.statics.len()
    }

    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// The most bodies this world holds.
    pub fn body_capacity(&self) -> usize {
        self.body_capacity
    }

    /// Adds a kinematic body. Allocates nothing: past [`Self::body_capacity`] it refuses instead.
    pub fn add_body(&mut self, desc: BodyDesc) -> Result<(), WorldError> {
        if !desc.shape.is_valid() || !desc.pose.is_valid() {
            return Err(WorldError::InvalidBody(desc.id));
        }
        let slot = match self.bodies.binary_search_by_key(&desc.id, |body| body.id) {
            Ok(_) => return Err(WorldError::DuplicateBody(desc.id)),
            Err(slot) => slot,
        };
        if self.bodies.len() >= self.body_capacity {
            return Err(WorldError::TooManyBodies { capacity: self.body_capacity });
        }
        let convex = desc.shape.to_convex();
        let parry_pose = desc.pose.to_parry();
        let entry = BodyEntry {
            id: desc.id,
            shape: desc.shape,
            convex,
            pose: desc.pose,
            parry_pose,
            groups: desc.groups,
            aabb: convex.aabb(&parry_pose),
        };
        if !entry.aabb.mins.is_finite() || !entry.aabb.maxs.is_finite() {
            return Err(WorldError::InvalidBody(desc.id));
        }
        self.bodies.insert(slot, entry);
        Ok(())
    }

    /// Removes a body, returning whether it was there.
    pub fn remove_body(&mut self, id: BodyId) -> bool {
        match self.body_slot(id) {
            Ok(slot) => {
                self.bodies.remove(slot);
                true
            }
            Err(_) => false,
        }
    }

    /// Where a body is.
    pub fn body_pose(&self, id: BodyId) -> Option<Pose> {
        Some(self.bodies[self.body_slot(id).ok()?].pose)
    }

    /// A body's shape.
    pub fn body_shape(&self, id: BodyId) -> Option<Shape> {
        Some(self.bodies[self.body_slot(id).ok()?].shape)
    }

    /// What a body belongs to and what it may touch.
    pub fn body_groups(&self, id: BodyId) -> Option<CollisionGroups> {
        Some(self.bodies[self.body_slot(id).ok()?].groups)
    }

    /// Changes what a body belongs to and what it may touch, and leaves it where it is.
    pub fn set_body_groups(&mut self, id: BodyId, groups: CollisionGroups) -> Result<(), WorldError> {
        let slot = self.body_slot(id).map_err(|_| WorldError::UnknownBody(id))?;
        self.bodies[slot].groups = groups;
        Ok(())
    }

    /// Moves a body without sweeping: a teleport, or a hurtbox following its table.
    pub fn set_body_pose(&mut self, id: BodyId, pose: Pose) -> Result<(), WorldError> {
        if !pose.is_valid() {
            return Err(WorldError::InvalidBody(id));
        }
        let slot = self.body_slot(id).map_err(|_| WorldError::UnknownBody(id))?;
        self.bodies[slot].set_pose(pose)
    }

    /// The world's size and the work done since this was last called, which it resets.
    pub fn take_stats(&self) -> PhysicsStats {
        self.counters.take(self.statics.len() as u32, self.bodies.len() as u32)
    }

    /// A digest of every static collider, in ID order.
    pub fn statics_digest(&self) -> u64 {
        self.statics_digest
    }

    /// Writes the world's state as canonical bytes for a simulation checksum: floats by their bits
    /// (one NaN for all), integers little-endian, no padding, bodies in ID order. Static colliders
    /// go in as [`Self::statics_digest`], since they never change. Allocates nothing.
    ///
    /// ```
    /// # use sapphire_physics::PhysicsWorld;
    /// # let world = PhysicsWorld::new(Vec::new(), 0).unwrap();
    /// let mut bytes = Vec::new();
    /// world.write_canonical(&mut |chunk| bytes.extend_from_slice(chunk));
    /// ```
    pub fn write_canonical(&self, sink: &mut impl FnMut(&[u8])) {
        sink(&self.statics_digest.to_le_bytes());
        sink(&(self.bodies.len() as u32).to_le_bytes());
        for body in &self.bodies {
            sink(&body.id.0.to_le_bytes());
            write_shape(&body.shape, sink);
            write_floats(&body.pose.position.to_array(), sink);
            write_floats(&body.pose.rotation.to_array(), sink);
            sink(&body.groups.memberships.bits().to_le_bytes());
            sink(&body.groups.filter.bits().to_le_bytes());
        }
    }

    pub(crate) fn body_slot(&self, id: BodyId) -> Result<usize, usize> {
        self.bodies.binary_search_by_key(&id, |body| body.id)
    }
}

fn check_tree_depth(tree: &Bvh) -> Result<(), WorldError> {
    if tree.is_empty() {
        return Ok(());
    }
    let depth = tree.subtree_depth(0);
    if depth > MAX_TREE_DEPTH {
        return Err(WorldError::TreeTooDeep { depth });
    }
    Ok(())
}

fn build_static(collider: StaticCollider, digest: &mut Xxh3) -> Result<StaticEntry, WorldError> {
    let id = collider.id;
    let invalid = |problem: String| WorldError::InvalidCollider { id, problem };
    check_position(collider.pose.position.to_array()).map_err(invalid)?;
    if !collider.pose.is_valid() {
        return Err(invalid(format!("rotation {} isn't a unit quaternion", collider.pose.rotation)));
    }
    if collider.memberships == GroupMask::NONE {
        return Err(invalid("belongs to no group, so nothing could ever hit it".to_owned()));
    }

    digest.update(&id.0.to_le_bytes());
    digest.update(&collider.memberships.bits().to_le_bytes());
    let mut sink = |bytes: &[u8]| digest.update(bytes);
    write_floats(&collider.pose.position.to_array(), &mut sink);
    write_floats(&collider.pose.rotation.to_array(), &mut sink);

    let pose = collider.pose.to_parry();
    let (geometry, aabb) = match collider.shape {
        StaticShape::Convex(shape) => {
            if !shape.is_valid() {
                return Err(invalid(format!("{shape:?} isn't finite or is too small")));
            }
            write_shape(&shape, &mut sink);
            let convex = shape.to_convex();
            let aabb = convex.aabb(&pose);
            (StaticGeometry::Convex(convex), aabb)
        }
        StaticShape::Mesh { mut vertices, mut indices } => {
            let flat: Vec<[f32; 3]> = vertices.iter().map(|v| v.to_array()).collect();
            check_mesh(&flat, &indices).map_err(invalid)?;
            sink(&2_000u32.to_le_bytes());
            sink(&(flat.len() as u32).to_le_bytes());
            sink(&(indices.len() as u32).to_le_bytes());
            for vertex in &flat {
                write_floats(vertex, &mut sink);
            }
            for triangle in &indices {
                for corner in triangle {
                    sink(&corner.to_le_bytes());
                }
            }
            // No flags: pseudo-normals and topology build through hash maps and trigonometry, and
            // only contact manifolds, which nothing here uses, read them.
            drop(flat);
            // The immutable mesh doesn't need caller spare capacity.
            vertices.shrink_to_fit();
            indices.shrink_to_fit();
            let mesh = TriMesh::with_flags(vertices, indices, TriMeshFlags::empty())
                .map_err(|error| invalid(format!("the mesh won't build: {error}")))?;
            let aabb = mesh.aabb(&pose);
            (StaticGeometry::Mesh(Box::new(mesh)), aabb)
        }
    };
    if !aabb.mins.is_finite() || !aabb.maxs.is_finite() {
        return Err(invalid("world bounds aren't finite".to_owned()));
    }
    Ok(StaticEntry { id, geometry, pose, memberships: collider.memberships, aabb })
}

fn write_floats(values: &[f32], sink: &mut dyn FnMut(&[u8])) {
    for value in values {
        // Rust doesn't promise a NaN's payload, so every NaN writes as one.
        let bits = if value.is_nan() { f32::NAN.to_bits() } else { value.to_bits() };
        sink(&bits.to_le_bytes());
    }
}

fn write_shape(shape: &Shape, sink: &mut dyn FnMut(&[u8])) {
    match *shape {
        Shape::Box { half_extents } => {
            sink(&1u32.to_le_bytes());
            write_floats(&half_extents.to_array(), sink);
        }
        Shape::Sphere { radius } => {
            sink(&2u32.to_le_bytes());
            write_floats(&[radius], sink);
        }
        Shape::Capsule { half_height, radius } => {
            sink(&3u32.to_le_bytes());
            write_floats(&[half_height, radius], sink);
        }
    }
}
