//! Zones in the simulation (M3 W4): what a transition does to the fixed step.
//!
//! A zone is simulation data as well as cells: its colliders, its navmesh and the spawners it arrives at.
//! [`ZoneGround`] is those, built from a zone's cooked bytes between steps. A simulation holds what it might
//! enter on its shelf ([`Simulation::add_zone`](crate::Simulation::add_zone)), and
//! [`Simulation::enter_zone`](crate::Simulation::enter_zone) swaps one in, between two steps:
//!
//! 1. Everyone but the traveller and its party goes. Their IDs are never used again.
//! 2. The collision world takes the new zone's statics, keeping its bodies, and the navmesh is the new
//!    zone's, with no paths on it.
//! 3. The travellers stand at the spawner, as [`Simulation::teleport`](crate::Simulation::teleport) stands one.
//!
//! **It's a pure function of cooked bytes at a tick boundary,** which is why it stays deterministic however
//! long the load took: no step runs while a wipe covers a load, so nothing the frame does reaches the fixed
//! step, and the tick, the clocks and the random stream come through untouched. A replay records the entry,
//! at the tick it came before ([`ReplayCommand::EnterZone`](crate::replay::ReplayCommand::EnterZone)), by the
//! shelf's row and the zone's spawner.
//!
//! **What pins it:** every row's digest goes in [`Simulation::tables_hash`](crate::Simulation::tables_hash),
//! so a replay refuses to start on zones that aren't the ones it recorded, and from the first entry the
//! current zone's goes in [`Simulation::checksum`](crate::Simulation::checksum), so the first tick a run
//! differs at says so if it's in another. A simulation with an empty shelf hashes what it always did, so no
//! recorded replay moved for any of this.
//!
//! Nothing here runs in a step, so the shelf and a swap may allocate.

use sapphire_colliders::ColliderError;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_navmesh::NavMeshError;
use sapphire_physics::{ColliderSet, PhysicsWorld, WorldError};
use sapphire_serialize::AssetId;
use sapphire_zone::{Zone, ZoneError};

use crate::actions::Controls;
use crate::actors::{ActorId, ActorTable, Teleports};
use crate::brain::Brain;
use crate::checksum::StateHasher;
use crate::controller::{CharacterController, CharacterWorld, ControllerDrive, ControllerState, actor_body};
use crate::hits::HitQuery;
use crate::intents::IntentRing;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveOutput, MoveRunner, WarpTarget};
use crate::navigation::{SimNavigation, WalkPath};
use crate::party::Party;
use crate::resolve::Fighter;
use crate::timeline::TimelineRun;

/// A row of a simulation's shelf: what [`Simulation::add_zone`](crate::Simulation::add_zone) gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ZoneId(u16);

impl ZoneId {
    /// The row a run's `add_zone`s gave it, from a replay's file or a script.
    #[must_use]
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// Where a traveller arrives: a zone on the shelf, and which of its spawners, in [`ZoneGround::spawner`]'s order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ZoneEntry {
    pub zone: ZoneId,
    pub spawner: u16,
}

impl ZoneEntry {
    /// As a replay's file holds it.
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        (self.zone.0 as u32) << 16 | self.spawner as u32
    }

    /// From what [`Self::to_raw`] gave.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self { zone: ZoneId((raw >> 16) as u16), spawner: raw as u16 }
    }
}

/// Why a zone's bytes make no [`ZoneGround`].
#[derive(Debug, thiserror::Error)]
pub enum ZoneLoadError {
    #[error(transparent)]
    Zone(#[from] ZoneError),
    #[error("its {what} {id} wasn't there to read")]
    NotLoaded { what: &'static str, id: AssetId },
    #[error(transparent)]
    Colliders(#[from] ColliderError),
    #[error(transparent)]
    World(#[from] WorldError),
    #[error(transparent)]
    NavMesh(#[from] NavMeshError),
}

/// Why an entry changed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ZoneEntryError {
    #[error("no zone {} is on the shelf", .0.0)]
    UnknownZone(ZoneId),
    #[error("zone {} has no spawner {spawner}", .zone.0)]
    UnknownSpawner { zone: ZoneId, spawner: u16 },
    #[error("actor {} isn't alive to travel", .0.raw())]
    NoTraveller(ActorId),
    #[error("a timeline is playing, whose cast wouldn't be here")]
    TimelinePlaying,
}

/// A place a zone can be arrived at.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SimSpawner {
    /// The hash of its name.
    id: u64,
    position: Vec3,
    yaw: f32,
}

/// What a simulation runs on while it's in a zone: see the [module docs](self).
pub struct ZoneGround {
    asset: AssetId,
    /// The pack it's in, for what stands a show beside this. No part of what it hashes.
    pack: String,
    digest: u64,
    /// Statics alone, while it's waiting on the shelf: its bodies are the simulation's.
    physics: PhysicsWorld,
    navigation: Option<SimNavigation>,
    /// By ascending ID, as the cook sorted them, so a row's index doesn't hang on the document's order.
    spawners: Vec<SimSpawner>,
}

impl std::fmt::Debug for ZoneGround {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZoneGround")
            .field("asset", &self.asset)
            .field("digest", &self.digest)
            .field("statics", &self.physics.static_count())
            .field("spawners", &self.spawners.len())
            .finish_non_exhaustive()
    }
}

impl ZoneGround {
    /// From the cooked zone `asset`'s bytes, and `read`, which finds what it references: its colliders and
    /// its navmesh. All of them must start 16-byte aligned, as the asset server holds them. `pack` is the
    /// pack a show finds it in.
    pub fn from_cooked<'a>(
        asset: AssetId,
        pack: &str,
        zone: &'a [u8],
        mut read: impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<Self, ZoneLoadError> {
        let archived = Zone::read(zone)?;
        let digest = archived.simulation_digest(&mut read)?;
        let simulation = &archived.simulation;
        let physics = match simulation.colliders.as_ref().copied() {
            Some(id) => {
                let bytes = read(id).ok_or(ZoneLoadError::NotLoaded { what: "colliders", id })?;
                PhysicsWorld::from_cooked(ColliderSet::read(bytes)?, 0)?
            }
            None => PhysicsWorld::new(Vec::new(), 0)?,
        };
        let navigation = match simulation.navigation.as_ref().copied() {
            Some(id) => {
                let bytes = read(id).ok_or(ZoneLoadError::NotLoaded { what: "navmesh", id })?;
                Some(SimNavigation::new(bytes)?)
            }
            None => None,
        };
        let spawners = simulation
            .spawners
            .iter()
            .map(|spawner| SimSpawner {
                id: spawner.id.to_native(),
                position: Vec3::from(spawner.position()),
                yaw: spawner.yaw(),
            })
            .collect();
        Ok(Self { asset, pack: pack.to_owned(), digest, physics, navigation, spawners })
    }

    /// The zone asset it came from.
    #[must_use]
    pub fn asset(&self) -> AssetId {
        self.asset
    }

    /// The pack it's in.
    #[must_use]
    pub fn pack(&self) -> &str {
        &self.pack
    }

    /// The ID of spawner `index`, the hash of its name.
    #[must_use]
    pub fn spawner_id(&self, index: u16) -> Option<u64> {
        self.spawners.get(usize::from(index)).map(|spawner| spawner.id)
    }

    /// A hash of what the simulation sees of it: `ArchivedZone::simulation_digest`.
    #[must_use]
    pub fn digest(&self) -> u64 {
        self.digest
    }

    /// Which spawner has this ID, the hash of its name: what an exit's `arrive` holds.
    #[must_use]
    pub fn spawner(&self, id: u64) -> Option<u16> {
        let at = self.spawners.binary_search_by_key(&id, |spawner| spawner.id).ok()?;
        u16::try_from(at).ok()
    }
}

/// The zones a simulation can enter, and which it's in. A resource.
#[derive(Resource, Default)]
pub(crate) struct ZoneShelf {
    /// A removed row stays, so the others keep their IDs, until an added zone takes it.
    rows: Vec<Option<ZoneGround>>,
    current: Option<ZoneId>,
}

impl ZoneShelf {
    pub(crate) fn add(&mut self, zone: ZoneGround) -> ZoneId {
        let at = self.rows.iter().position(Option::is_none).unwrap_or(self.rows.len());
        if at == self.rows.len() {
            self.rows.push(None);
        }
        self.rows[at] = Some(zone);
        ZoneId(u16::try_from(at).expect("far fewer zones than that"))
    }

    /// Takes a zone off the shelf, unless it's the one it's in, and says whether it did.
    pub(crate) fn remove(&mut self, id: ZoneId) -> bool {
        if self.current == Some(id) {
            return false;
        }
        self.rows.get_mut(usize::from(id.0)).is_some_and(|row| row.take().is_some())
    }

    pub(crate) fn current(&self) -> Option<ZoneId> {
        self.current
    }

    pub(crate) fn row(&self, id: ZoneId) -> Option<&ZoneGround> {
        self.rows.get(usize::from(id.0))?.as_ref()
    }

    /// The row holding this asset.
    pub(crate) fn find(&self, asset: AssetId) -> Option<ZoneId> {
        let at = self.rows.iter().flatten().position(|zone| zone.asset == asset)?;
        let row = self.rows.iter().enumerate().filter(|(_, row)| row.is_some()).nth(at)?.0;
        u16::try_from(row).ok().map(ZoneId)
    }

    /// For [`Simulation::tables_hash`](crate::Simulation::tables_hash): each row's digest. An empty shelf
    /// writes nothing.
    pub(crate) fn write_digests(&self, hasher: &mut StateHasher) {
        if self.rows.is_empty() {
            return;
        }
        hasher.u32(self.rows.len() as u32);
        for row in &self.rows {
            hasher.u64(row.as_ref().map_or(0, ZoneGround::digest));
        }
    }

    /// For the state checksum: which zone it's in, once it's in one.
    pub(crate) fn write_state(&self, hasher: &mut StateHasher) {
        if let Some(zone) = self.current.and_then(|current| self.row(current)) {
            hasher.u64(zone.digest);
        }
    }
}

/// Checks an entry can happen, and returns where it stands whoever travels. Changes nothing.
pub(crate) fn check_entry(
    world: &World,
    entry: ZoneEntry,
    traveller: ActorId,
) -> Result<(Vec3, f32), ZoneEntryError> {
    let zone =
        world.resource::<ZoneShelf>().row(entry.zone).ok_or(ZoneEntryError::UnknownZone(entry.zone))?;
    let spawner = zone
        .spawners
        .get(usize::from(entry.spawner))
        .ok_or(ZoneEntryError::UnknownSpawner { zone: entry.zone, spawner: entry.spawner })?;
    if world.resource::<ActorTable>().entity(traveller).is_none() {
        return Err(ZoneEntryError::NoTraveller(traveller));
    }
    if world.resource::<TimelineRun>().progress().is_some() {
        return Err(ZoneEntryError::TimelinePlaying);
    }
    Ok((spawner.position, spawner.yaw))
}

/// Puts the zone's statics and navmesh where the simulation reads them, and those of the one it was in back
/// on that one's row. What the simulation started on, which no row holds, stays in the current row's place.
pub(crate) fn swap_in(world: &mut World, entry: ZoneEntry) {
    world.resource_scope(|world, mut shelf: Mut<'_, ZoneShelf>| {
        let shelf = &mut *shelf;
        let (to, from) = (usize::from(entry.zone.0), shelf.current.map(|id| usize::from(id.0)));
        if from == Some(to) {
            return;
        }
        let incoming = shelf.rows[to].as_mut().expect("checked before anything changed");
        world.resource_mut::<CharacterWorld>().swap_statics(&mut incoming.physics);
        let leaving = world.remove_resource::<SimNavigation>();
        if let Some(navigation) = incoming.navigation.take() {
            world.insert_resource(navigation);
        }
        if let Some(from) = from {
            let (low, high) = shelf.rows.split_at_mut(from.max(to));
            if let (Some(a), Some(b)) = (low[from.min(to)].as_mut(), high[0].as_mut()) {
                std::mem::swap(&mut a.physics, &mut b.physics);
            }
            if let Some(row) = shelf.rows[from].as_mut() {
                row.navigation = leaving;
            }
        }
        shelf.current = Some(entry.zone);
    });
    // A path is polygons of the navmesh it was found on, so none outlives it: a brain finds its way again
    // on the one it's on now (M3 P3). By the table, since a query's state would allocate.
    for index in 0..world.resource::<ActorTable>().len() {
        let Some((_, entity)) = world.resource::<ActorTable>().iter().nth(index) else { break };
        if let Some(mut path) = world.get_mut::<WalkPath>(entity) {
            *path = WalkPath::default();
        }
    }
}

/// What [`place`] does with an actor's capsule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Capsule {
    /// A new one there, and its hurtboxes gone until its next move asks: a zone's entry, whose collision
    /// world changed under it. Allocates, as the entry does anyway.
    Anew,
    /// The one it has, moved there in the world it's in: a teleport, which takes nothing from the heap
    /// (M3 B4). Its hurtboxes follow as its next step places them.
    Moved,
}

/// Stands an actor at `position`, facing `yaw`, as if it had always been there, and counts it for
/// presentation; false for one that isn't alive. See [`Simulation::teleport`](crate::Simulation::teleport).
pub(crate) fn place(world: &mut World, actor: ActorId, position: Vec3, yaw: f32, capsule: Capsule) -> bool {
    let Some(entity) = world.resource::<ActorTable>().entity(actor) else { return false };
    let on_field = !world.resource::<Party>().is_off_field(actor);
    world.resource_mut::<HitQuery>().forget_actor(actor);
    let mut entry = world.entity_mut(entity);
    if let Some(mut controls) = entry.get_mut::<Controls>() {
        *controls = Controls::default();
    }
    let now = entry.get::<EntityClock>().map(|clock| clock.0.now());
    let mut set = None;
    if let Some(mut runner) = entry.get_mut::<MoveRunner>() {
        runner.running = None;
        runner.forced = None;
        runner.counter = None;
        if let Some(now) = now {
            runner.cursor = now;
        }
        set = runner.set();
    }
    if let Some(mut ring) = entry.get_mut::<IntentRing>() {
        ring.reset();
    }
    if let Some(mut output) = entry.get_mut::<MoveOutput>() {
        *output = MoveOutput::default();
    }
    if let Some(mut fighter) = entry.get_mut::<Fighter>() {
        fighter.stop_slide();
    }
    if let Some(mut teleports) = entry.get_mut::<Teleports>() {
        teleports.0 = teleports.0.wrapping_add(1);
    }
    // Its way led from where it was (M3 P3).
    if let Some(mut path) = entry.get_mut::<WalkPath>() {
        *path = WalkPath::default();
    }
    let transform = ActorTransform { position, yaw };
    if let Some(mut placed) = entry.get_mut::<ActorTransform>() {
        *placed = transform;
    }
    let profile = entry.get::<CharacterController>().and_then(CharacterController::profile);
    if capsule == Capsule::Moved {
        if let Some(mut standing) = entry.get_mut::<CharacterController>() {
            standing.state = ControllerState::default();
            standing.drive = ControllerDrive::NONE;
        }
        // What moves it, whose capsule this is. One off the field has none to move, and gets one where it
        // stands as it comes on.
        let movement = set.and_then(|set| world.resource::<MoveLibrary>().movement(set)).copied();
        let resolved = movement
            .or_else(|| profile.and_then(|id| world.resource::<CharacterWorld>().profile(id)).copied());
        if on_field && let Some(resolved) = resolved {
            let body = resolved.body(actor_body(actor), position, yaw);
            let _ = world.resource_mut::<CharacterWorld>().physics_mut().set_body_pose(body.id, body.pose);
        }
        return true;
    }
    world.resource_scope(|world, mut characters: Mut<'_, CharacterWorld>| {
        let library = world.resource::<MoveLibrary>();
        let movement = set.and_then(|set| library.movement(set));
        // Its hurtboxes come back as its next move asks. One off the field has no bodies, and gets them as
        // it comes on.
        characters.despawned(actor);
        let controller = if on_field {
            characters.spawned(actor, profile, movement, &transform)
        } else {
            CharacterController::default()
        };
        if let Some(mut standing) = world.get_mut::<CharacterController>(entity) {
            if on_field {
                *standing = controller;
            } else {
                standing.state = ControllerState::default();
                standing.drive = ControllerDrive::NONE;
            }
        }
    });
    true
}

/// What whoever travelled was doing about those who stayed behind, who are gone: its aim and its brain's.
pub(crate) fn forget_who_stayed(world: &mut World, actor: ActorId) {
    let Some(entity) = world.resource::<ActorTable>().entity(actor) else { return };
    let mut entry = world.entity_mut(entity);
    if let Some(mut target) = entry.get_mut::<WarpTarget>() {
        *target = WarpTarget::None;
    }
    if let Some(mut brain) = entry.get_mut::<Brain>() {
        *brain = Brain::default();
    }
}
