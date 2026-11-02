//! Static collision geometry as plain data: what the cooker and the physics crate agree on.
//!
//! The cooker must never link a physics engine (the lesson of #74: a tool links only what it
//! reads), so everything it needs to read and check a collider document lives here, beside the
//! collision groups those documents name. `sapphire_physics` builds its world from
//! [`ArchivedColliderSet`] and re-exports the groups, so gameplay code names them through it.
//!
//! # Owns
//!
//! - **Collision groups** ([`groups`]): [`Group`], [`GroupMask`] and [`CollisionGroups`].
//! - **The document** ([`document`]): `.colliders.ron`, a hand-authored list of named boxes,
//!   spheres, capsules and triangle meshes ([`ColliderDocument`]), and its checks, which name the
//!   offending entry.
//! - **The cooked format** ([`cooked`]): [`ColliderSet`], an rkyv archive (`.scol`), sorted by
//!   name so a collider's place in it is its stable ID, whatever order the document lists it in.
//!
//! # Does not own
//!
//! - Queries, bodies and motion: `sapphire_physics`.
//! - Reading sources and sidecars, cache keys and packs: `sapphire_pipeline`'s `colliders` module.
//! - Asset kinds and residency: `sapphire_assets`, which reads this crate's format as
//!   `AssetKind::Colliders`.
//!
//! # Determinism
//!
//! Cooked floats reach the simulation, whose checksums must agree between Linux and Windows
//! (spec §9, gate 2), and each OS cooks for itself. So a rotation authored as an axis and an angle
//! goes through `sapphire_math::det`, never `std`'s functions, which call each platform's C math
//! library. `[simulation]` in `xtask/layers.toml` holds this crate to the simulation's `clippy.toml`.

pub mod cooked;
pub mod document;
pub mod groups;

pub use cooked::{
    ArchivedColliderSet, ArchivedCookedCollider, ArchivedCookedMesh, ArchivedCookedShape,
    COLLIDERS_EXTENSION, ColliderSet, CookedCollider, CookedMesh, CookedShape,
};
pub use document::{ColliderDocument, ColliderEntry, ColliderError, ColliderShape, Rotation};
pub use groups::{CollisionGroups, Group, GroupMask};
