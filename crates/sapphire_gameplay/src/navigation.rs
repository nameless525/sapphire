//! A zone's navmesh in the simulation (M3 P2): the graph, the budget of a tick's searches and the paths
//! of whoever's following one, with what a replay must know of each.
//!
//! A simulation with none of this runs and hashes exactly what it did before there was any: nothing here
//! reaches [`Simulation::tables_hash`](crate::Simulation::tables_hash) or
//! [`Simulation::checksum`](crate::Simulation::checksum) until
//! [`Simulation::set_navigation`](crate::Simulation::set_navigation) is called, so no recorded replay
//! moved for it.
//!
//! **What a brain calls** is [`SimNavigation::begin_tick`], once a tick ahead of everyone, and then
//! [`SimNavigation::follow`] for each actor that's going somewhere, in actor order, since who gets the
//! tick's budget is part of what a replay checks. [`sapphire_nav::path`] says what comes back. No brain
//! does yet: that's P3, and only the `nav-chase` scenario steers by this (`replay::chase`).

use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_nav::{NavStats, Navigation, Path, Steer};
use sapphire_navmesh::{NavMesh, NavMeshError};

use crate::checksum::StateHasher;

/// Which of a simulation's [`Path`]s: what [`SimNavigation::add_path`] gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathId(u32);

/// A simulation's navigation. A resource, there only for a simulation given a navmesh.
#[derive(Resource)]
pub struct SimNavigation {
    navigation: Navigation,
    /// The cooked bytes', for [`Self::digest`].
    digest: u64,
    /// One for each follower, by [`PathId`]. Allocated between steps.
    paths: Vec<Path>,
}

impl SimNavigation {
    /// From a cook's bytes, which must start 16-byte aligned.
    pub(crate) fn new(cooked: &[u8]) -> Result<Self, NavMeshError> {
        let mut hasher = StateHasher::new();
        hasher.bytes(cooked);
        Ok(Self {
            navigation: Navigation::new(NavMesh::read(cooked)?),
            digest: hasher.finish(),
            paths: Vec::new(),
        })
    }

    /// This, with no follower: a navmesh going back on a zone's shelf, whose followers stayed behind or
    /// stood somewhere else.
    pub(crate) fn without_paths(mut self) -> Self {
        self.paths = Vec::new();
        self
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

    /// What paths have cost since the last call.
    pub fn take_stats(&mut self) -> NavStats {
        self.navigation.take_stats()
    }

    /// A new follower's path, between steps. This allocates, and following it doesn't.
    pub fn add_path(&mut self) -> PathId {
        let id = PathId(u32::try_from(self.paths.len()).expect("far fewer paths than that"));
        self.paths.push(Path::new());
        id
    }

    /// A path, as it stands.
    #[must_use]
    pub fn path(&self, id: PathId) -> &Path {
        &self.paths[id.0 as usize]
    }

    /// Forgets a path's route: a follower with somewhere else to go, say.
    pub fn clear_path(&mut self, id: PathId) {
        self.paths[id.0 as usize].clear();
    }

    /// Refills the tick's budget. Once a tick, ahead of that tick's first [`Self::follow`].
    pub fn begin_tick(&mut self) {
        self.navigation.begin_tick();
    }

    /// Which way the follower of `id`, standing at `position`, should go to get to `target`: see
    /// [`Navigation::follow`].
    pub fn follow(&mut self, id: PathId, position: Vec3, target: Vec3) -> Steer {
        self.navigation.follow(&mut self.paths[id.0 as usize], position, target)
    }

    /// What decides what comes next, for the state checksum: every path. The budget's refilled each tick
    /// and the search's pool is scratch, so neither carries anything over.
    pub(crate) fn write_state(&self, hasher: &mut StateHasher) {
        hasher.u32(self.paths.len() as u32);
        for path in &self.paths {
            path.write_state(&mut |bytes| hasher.bytes(bytes));
        }
    }
}
