//! The collision world (spec §5.14): static colliders, kinematic bodies, a move-and-collide that
//! character controllers build on, and ray, shape and overlap queries.
//!
//! Everything public here is an engine type. Geometry queries come from `parry3d`, which no public
//! signature names, so changing that (or moving to Jolt) touches only this crate.
//!
//! # Why `parry3d` and not Rapier
//!
//! The plan's default was `rapier3d` with its `enhanced-determinism` feature. M2 D3's spike
//! (m2-plan.md, the D3 row) measured it and took the plan's written fallback:
//!
//! - **`enhanced-determinism` changes glam for the whole process.** It turns on glam's `scalar-math`
//!   and `libm`, and Cargo unifies features on the workspace's one glam 0.33. `Vec4`, `Quat` and
//!   `Mat4` drop from 16-byte to 4-byte alignment, the renderer's trigonometry changes function,
//!   and 3 of the 95 Metal goldens changed, the same way on every run.
//! - **Rapier's controller and `step` allocate on every step, warmed:** 45 allocations a step on
//!   the spike's scene, from per-call `Vec`s in its controller, in parry's manifolds and EPA, and
//!   one in `PhysicsPipeline::step`. They're locals, so nothing can pre-size them, and R6 forbids
//!   allocating in the fixed step.
//! - **The slice has nothing for it to do.** No dynamic body exists, so a step has nothing to
//!   integrate, and what remains of Rapier is parry behind a wrapper.
//!
//! # Allocation (rule R6)
//!
//! Building a world allocates. After that, [`PhysicsWorld::move_and_collide`], every query, and
//! adding bodies within the capacity the world was built with allocate nothing, ever: no warmup,
//! no pools to grow. `tests/allocations.rs` holds this under the counting allocator.
//!
//! This is why the crate measures with GJK's distance alone rather than through
//! `parry3d::query::cast_shapes`: when two shapes overlap at the start of a cast, that function and
//! `query::contact` run EPA, which allocates afresh on every call. Its ray cast can also report a
//! false miss, which would tunnel. Sweeps and overlaps here measure between the shapes' cores
//! instead. A tree's traversal keeps its stack inline to a depth of 32, so building a world refuses
//! a tree deeper than that.
//!
//! # Determinism
//!
//! Single-threaded by construction: nothing here spawns work, and queries take `&self`, so
//! threads may share a world between steps. There is no step: a body's move or new pose takes
//! effect at once, so hit queries later in the same tick read this tick's positions, and the
//! fixed step's physics stage has nothing to do until something dynamic exists. Results are
//! functions of the world's contents, not its history:
//!
//! - Static colliders sort by [`ColliderId`] before the tree is built, and bodies stay sorted by
//!   [`BodyId`], so the order they were added in changes nothing.
//! - Lists of results are sorted by distance and [`HitTarget`], so a walk's order never shows.
//! - No `std` transcendental functions (`[simulation]` in `xtask/layers.toml` holds this crate to
//!   the simulation's `clippy.toml`), and `parry3d` never runs them on these paths.
//!
//! Replays reproduce per architecture (spec §9, gate 2): glam's NEON and SSE2 backends round
//! differently, so arm64 and x86-64 have a checksum each, and x86-64's must agree between Linux and
//! Windows. `tests/checksum.rs` holds both, and a simulation holding a world includes it in its own
//! checksum through [`PhysicsWorld::write_canonical`].
//!
//! # Conventions
//!
//! World space is metres, right-handed and Y up (`sapphire_math`). Capsules stand along local +Y.
//! A hit's normal always points away from what was hit, toward what hit it, in world space.
//!
//! # Budget (rule R9)
//!
//! [`STEP_BUDGET_DESKTOP_LOW_US`] and [`STEP_BUDGET_MOBILE_LOW_US`] are §3.2's physics budget.
//! [`PhysicsWorld::take_stats`] publishes the work done since it was last called. This crate never
//! reads a clock (rule R2): whoever runs the fixed step times it and compares.
//!
//! # Does not own
//!
//! - The character controller's behavior: grounded, slope and step-up handling (M2 G1), built on
//!   [`PhysicsWorld::move_and_collide`] and the queries.
//! - Hit queries and hurtboxes: `sapphire_gameplay`'s `hits` module (M2 D4), built on bodies,
//!   [`Group::Hurtbox`], [`QueryFilter::excluding_bodies`] and [`PhysicsWorld::cast_shape_all`].
//! - Dynamic bodies, joints and ragdolls: no package needs them yet.
//! - The document and cooked format: `sapphire_colliders`, which the cooker links without this
//!   crate.

// `enhanced-determinism` anywhere in a build turns on glam's `scalar-math` for every crate in it, and
// these are the layouts that changes: see the crate docs. This makes such a build fail to compile.
const _: () = {
    assert!(align_of::<glam::Vec4>() == 16);
    assert!(align_of::<glam::Quat>() == 16);
    assert!(align_of::<glam::Mat4>() == 16);
};

mod inspect;
mod mesh_axis;
mod mover;
mod narrow;
mod query;
mod shape;
mod stats;
mod world;

pub use inspect::{StaticGeometryView, StaticView};
pub use mesh_axis::{MeshAxisBinding, MeshAxisError, MeshAxisProbe, MeshAxisWitness};
pub use mover::{MAX_MOVE_HITS, MoveHit, MoveOptions, MoveOutcome};
pub use query::{Overlaps, Penetration, QueryFilter, RayHit, ShapeHit};
pub use sapphire_colliders::{ColliderSet, CollisionGroups, Group, GroupMask};
pub use shape::{Pose, Shape};
pub use stats::{PhysicsStats, STEP_BUDGET_DESKTOP_LOW_US, STEP_BUDGET_MOBILE_LOW_US};
pub use world::{
    BodyDesc, BodyId, ColliderId, HitTarget, PhysicsWorld, StaticCollider, StaticShape, WorldError,
};
