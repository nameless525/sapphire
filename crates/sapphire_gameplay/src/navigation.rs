//! A zone's navmesh in the simulation (M3 P2): the graph and the budget of a tick's searches, with what a
//! replay must know of them.
//!
//! A simulation with none of this runs and hashes exactly what it did before there was any: nothing here
//! reaches [`Simulation::tables_hash`](crate::Simulation::tables_hash) or
//! [`Simulation::checksum`](crate::Simulation::checksum) until
//! [`Simulation::set_navigation`](crate::Simulation::set_navigation) is called, so no recorded replay
//! moved for it.
//!
//! **Who calls it** is [`crate::brain`], which refills the budget once a tick ahead of everyone and asks
//! [`Navigation::way`] for each actor that's going somewhere, in actor order, since who gets the tick's
//! budget is part of what a replay checks. [`sapphire_nav::path`] says what comes back.
//!
//! **A path lives on its actor** ([`WalkPath`], in the pool), rather than in a list here: a spawn in a
//! step takes nothing from the heap, it goes when its actor does, and the checksum reads it in ID order
//! with everything else of the actor's. It costs a pooled actor 1.3 KB, which only a walker touches.

use sapphire_ecs::prelude::*;
use sapphire_nav::{NavStats, Navigation, Path};
use sapphire_navmesh::{NavMesh, NavMeshError};

use crate::checksum::StateHasher;

/// A simulation's navigation. A resource, there only for a simulation given a navmesh.
#[derive(Resource)]
pub struct SimNavigation {
    navigation: Navigation,
    /// The cooked bytes', for [`Self::digest`].
    digest: u64,
}

impl SimNavigation {
    /// From a cook's bytes, which must start 16-byte aligned.
    pub(crate) fn new(cooked: &[u8]) -> Result<Self, NavMeshError> {
        let mut hasher = StateHasher::new();
        hasher.bytes(cooked);
        Ok(Self { navigation: Navigation::new(NavMesh::read(cooked)?), digest: hasher.finish() })
    }

    /// A hash of the cooked navmesh. Its bytes are the same on every architecture, which
    /// `tools/sapphire_pipeline/tests/nav_baked.rs` holds to a bake on each of them.
    #[must_use]
    pub fn digest(&self) -> u64 {
        self.digest
    }

    #[must_use]
    pub fn navigation(&self) -> &Navigation {
        &self.navigation
    }

    /// For a brain: the tick's budget and its searches.
    pub(crate) fn navigation_mut(&mut self) -> &mut Navigation {
        &mut self.navigation
    }

    /// What paths have cost since the last call.
    pub fn take_stats(&mut self) -> NavStats {
        self.navigation.take_stats()
    }
}

/// An actor's way to wherever its brain is going, carried from tick to tick. Every pooled actor has one,
/// which holds nothing until its brain has to go round something.
#[derive(Component, Clone, Debug, Default)]
pub struct WalkPath(pub(crate) Path);

impl WalkPath {
    /// The path, as it stands.
    #[must_use]
    pub const fn path(&self) -> &Path {
        &self.0
    }

    /// Everything of it that decides what comes next, for the state checksum, when there's a navmesh.
    pub(crate) fn write_state(&self, hasher: &mut StateHasher) {
        self.0.write_state(&mut |bytes| hasher.bytes(bytes));
    }
}
