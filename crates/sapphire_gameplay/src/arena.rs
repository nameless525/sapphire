//! The arena's plain data (M2 O5): the collision world a duel stands in, and where it starts.
//!
//! The arena is the look-dev town's square as a place to fight. Its collision is a collider document
//! (`content/arena/square.colliders.ron`) and its spawn points the document beside it
//! (`content/arena/square.spawns.ron`). This module compiles both in, so a replay needs no assets and
//! no path: a headless runner on any OS has them, and [`crate::replay::SCENARIOS`]' `arena` builds its
//! world from them. The app's stage (`apps/sapphire_sandbox/src/arena.rs`) reads the same document,
//! cooked into its pack.
//!
//! **The same on every OS:** the document's numbers are parsed exactly, and a collider's turn goes
//! through `sapphire_math::det` (see `sapphire_colliders`), so its cooked bytes, and the statics' digest
//! that [`crate::Simulation::tables_hash`] takes, don't depend on where they were cooked. Moving a
//! collider or a spawn point re-records the `arena` replays: `cargo xtask replay --update`.
//!
//! `tests/arena.rs` holds the documents to each other and walks them with a character: each spawn point
//! on the ground and clear of everything, and no stick or jump that leaves.

use bevy_reflect::TypePath;
use sapphire_colliders::{ColliderDocument, ColliderError};
use sapphire_math::Vec3;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::AlignedVec;
use serde::{Deserialize, Serialize};

use crate::controller::ControllerProfile;

/// The collider document, as this build holds it.
const COLLIDERS: &str = include_str!("../../../content/arena/square.colliders.ron");

/// The spawn points' document, as this build holds it.
const SPAWNS: &str = include_str!("../../../content/arena/square.spawns.ron");

/// The arena's colliders, cooked as a pack holds them: `sapphire_physics::ColliderSet::read` takes them.
///
/// # Errors
///
/// Never for a build whose document this crate's tests have read.
pub fn cooked_colliders() -> Result<AlignedVec, ColliderError> {
    ColliderDocument::from_ron(COLLIDERS)?.cook()?.to_cooked()
}

/// How everyone in the arena moves, and how big they are: a capsule 1.6 m tall, the look-dev character's
/// height hair and all, and 0.3 m in radius, a little past the shoulders so an arm at rest stays out of
/// a wall. Starting tuning, for a game's own to replace.
#[must_use]
pub fn profile() -> ControllerProfile {
    ControllerProfile::default()
}

/// Where one character starts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnPoint {
    /// Where its feet stand: the root of its capsule's lowest point, in metres.
    pub position: [f32; 3],
    /// About +Y, in degrees; 0 faces +Z.
    pub yaw_degrees: f32,
}

impl SpawnPoint {
    /// Where its feet stand.
    #[must_use]
    pub fn position(&self) -> Vec3 {
        Vec3::from_array(self.position)
    }

    /// Its yaw in radians, as [`crate::ActorSpawn`] takes it: an exact product, with no C math
    /// library in it.
    #[must_use]
    pub fn yaw(&self) -> f32 {
        self.yaw_degrees.to_radians()
    }
}

/// The spawn points' document (`content/arena/square.spawns.ron`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct ArenaSpawns {
    pub player: SpawnPoint,
    pub enemy: SpawnPoint,
}

impl Versioned for ArenaSpawns {
    /// 1: M2 O5.
    const VERSION: u32 = 1;
}

/// The arena's spawn points.
///
/// # Panics
///
/// Never for a build whose document this module's test has read.
#[must_use]
pub fn spawns() -> ArenaSpawns {
    sapphire_serialize::from_ron(SPAWNS).expect("the spawn points are checked by this module's test")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_reads_and_the_duel_starts_face_to_face() {
        let spawns = spawns();
        let toward_enemy = spawns.enemy.position() - spawns.player.position();
        assert!((4.0..=8.0).contains(&toward_enemy.length()), "a duel's opening distance: {toward_enemy}");
        // Facing +Z at yaw 0, each faces the other along Z.
        assert!(toward_enemy.x.abs() < 1e-6 && toward_enemy.z < 0.0, "{toward_enemy}");
        assert!((spawns.player.yaw() - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(spawns.enemy.yaw(), 0.0);
    }

    /// What the module says of the capsule, as the look-dev character needs it.
    #[test]
    fn the_capsule_is_the_characters_height() {
        let profile = profile();
        assert_eq!((profile.height, profile.radius), (1.6, 0.3));
        assert!(profile.resolve().is_ok());
    }

    #[test]
    fn the_colliders_cook() {
        let cooked = cooked_colliders().expect("it cooks");
        assert!(!cooked.is_empty());
    }
}
