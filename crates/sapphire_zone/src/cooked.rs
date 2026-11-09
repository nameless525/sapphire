//! The cooked zone (`.szone`): an rkyv archive the runtime reads in place.
//!
//! Two halves, because they live by different rules. [`ZoneSimulation`] loads before the first step and
//! stays, and a replay pins a digest of it ([`ArchivedZone::simulation_digest`]). [`ZonePresentation`]
//! gates what's drawn: the backdrop and the cells, whose meshes, instances and lights the world
//! activates by distance.
//!
//! No upgrades, like every cooked format: a changed layout takes a version bump and recooks from its
//! document.
//!
//! [`Zone::read`] takes bytes that came from a pack, which needn't have come from our cooker: bytecheck
//! rules out a bad archive, and [`ArchivedZone::validate`] everything the runtime would otherwise have
//! to trust, each entry's values, every order an ID or a search leans on, and that the places' ranges
//! share out the instances and lights exactly.

use bevy_reflect::TypePath;
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use xxhash_rust::xxh3::Xxh3;

use crate::document::{
    MAX_ENTRIES, MIN_EXTENT, ZoneError, check_color, check_finite, check_name, check_points, check_position,
    check_positive, check_scale, check_spot,
};
use crate::grid::{Bounds, CellCoord, Grid, MAX_COORDINATE};

/// Conventional file extension for cooked zones.
pub const ZONE_EXTENSION: &str = "szone";

/// A cooked zone. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::zone"]
pub struct Zone {
    pub simulation: ZoneSimulation,
    pub presentation: ZonePresentation,
}

impl Versioned for Zone {
    /// 1: M3 W1. 2: one navmesh in place of a list of tiles (P1). 3: exits (W4). 4: checkpoints (B4).
    const VERSION: u32 = 4;
}

/// What the simulation reads of a zone. Everything here, and everything it references, is in a
/// replay's digest.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ZoneSimulation {
    /// The zone's static colliders.
    pub colliders: Option<AssetId>,
    /// By ascending ID.
    pub spawners: Vec<CookedSpawner>,
    /// By ascending ID.
    pub triggers: Vec<CookedTrigger>,
    /// By ascending ID.
    pub checkpoints: Vec<CookedCheckpoint>,
    /// The zone's navmesh, one flat graph however many tiles it baked from.
    pub navigation: Option<AssetId>,
    /// Encounters, ascending.
    pub encounters: Vec<AssetId>,
    /// By ascending ID.
    pub exits: Vec<CookedExit>,
}

/// A way out of a zone ([`Exit`](crate::Exit)). Only the IDs and the pack's name say where it goes: nothing of
/// the zone it names is a reference, so nothing of it loads or goes in this zone's pack.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedExit {
    /// The name's hash ([`Name`]), which a replay and a save name it by.
    pub id: u64,
    pub name: String,
    /// The zone it leads to.
    pub to: AssetId,
    /// That zone's pack, for mounting before anything of it can be read.
    pub pack: String,
    /// The spawner it arrives at, in that zone: the hash of its name.
    pub arrive: u64,
    /// Whether that zone's pack comes in ahead of time. How soon loading starts is no business of a replay's,
    /// so it stays out of the digest.
    pub prefetch: bool,
}

/// A place something spawns at.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedSpawner {
    /// The name's hash ([`Name`]), which a replay and a save name it by.
    pub id: u64,
    pub name: String,
    /// Where its feet stand, in metres.
    pub position: [f32; 3],
    /// About +Y, in degrees; 0 faces +Z.
    pub yaw_degrees: f32,
    /// What spawns here, by a name gameplay knows.
    pub spawns: String,
}

/// A volume that raises an event when something enters it.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedTrigger {
    /// The name's hash ([`Name`]), which a replay and a save name it by.
    pub id: u64,
    pub name: String,
    /// The volume's center, in metres.
    pub position: [f32; 3],
    pub volume: CookedVolume,
    /// What it raises, by a name gameplay knows.
    pub event: String,
}

/// Somewhere a walk through the zone can start again: see the document's `Checkpoint`.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedCheckpoint {
    /// The name's hash ([`Name`]). A save holds the name, which outlives any one cook's order.
    pub id: u64,
    pub name: String,
    /// Where a player's feet stand when the zone starts here, in metres.
    pub position: [f32; 3],
    /// About +Y, in degrees; 0 faces +Z.
    pub yaw_degrees: f32,
    /// What reaches it, about its position.
    pub volume: CookedVolume,
}

/// A trigger's or a checkpoint's volume, about its position.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum CookedVolume {
    /// Along the zone's axes.
    Box {
        half_extents: [f32; 3],
    },
    Sphere {
        radius: f32,
    },
}

impl CookedVolume {
    /// Whether its extents are ones a test can use, or what's wrong with them.
    pub(crate) fn check(self) -> Result<(), String> {
        let good = |extent: f32| extent.is_finite() && (MIN_EXTENT..=MAX_COORDINATE).contains(&extent);
        match self {
            Self::Box { half_extents } if half_extents.iter().all(|&extent| good(extent)) => Ok(()),
            Self::Box { half_extents } => {
                Err(format!("half extents {half_extents:?} aren't from {MIN_EXTENT} to {MAX_COORDINATE} m"))
            }
            Self::Sphere { radius } if good(radius) => Ok(()),
            Self::Sphere { radius } => {
                Err(format!("radius {radius} isn't from {MIN_EXTENT} to {MAX_COORDINATE} m"))
            }
        }
    }
}

/// What only presentation reads of a zone.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ZonePresentation {
    /// A cell's side, in metres ([`Grid`]).
    pub cell_size: f32,
    /// What stays for as long as the zone does.
    pub backdrop: Option<PlaceContents>,
    /// Cells that hold anything, by ascending `[x, z]`.
    pub cells: Vec<ZoneCell>,
    /// Every place's, the backdrop's first, then the cells' in order.
    pub instances: Vec<CookedInstance>,
    /// Likewise.
    pub lights: Vec<CookedLight>,
    /// The zone's ambient probe grid.
    pub probes: Option<AssetId>,
    /// The time-of-day timeline's document, as RON text, its cook having checked it.
    pub day: Option<String>,
    /// The character light rig library's document, as RON text.
    pub rigs: Option<String>,
    /// Camera rails, by ascending ID.
    pub splines: Vec<CookedSpline>,
}

/// One cell of a zone's grid.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct ZoneCell {
    /// `[x, z]`.
    pub coord: [i32; 2],
    pub contents: PlaceContents,
}

/// What the backdrop or a cell holds.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct PlaceContents {
    /// The placed prefabs' geometry, merged and in the zone's space, so it draws with no transform. A
    /// place with only instances or lights has none.
    pub mesh: Option<AssetId>,
    /// Around the mesh, the instances and the lights' reach, in metres, in the zone's space: this may
    /// lie up to one cell past a cell's square ([`Grid::overreach`]).
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Its range in [`ZonePresentation::instances`].
    pub first_instance: u32,
    pub instance_count: u32,
    /// Its range in [`ZonePresentation::lights`].
    pub first_light: u32,
    pub light_count: u32,
}

/// One mesh asset drawn on its own.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedInstance {
    pub mesh: AssetId,
    pub position: [f32; 3],
    /// A unit quaternion: x, y, z, w.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// A local light, as the renderer's `LocalLight` takes one.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedLight {
    pub position: [f32; 3],
    /// Linear RGB.
    pub color: [f32; 3],
    pub intensity: f32,
    /// Distance in metres at which it has faded to nothing.
    pub range: f32,
    pub shape: CookedLightShape,
    /// Its intensity follows time of day's night, as a lamp's does.
    pub at_night: bool,
}

/// The shape of a [`CookedLight`]'s light.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum CookedLightShape {
    Point,
    /// A cone along `direction`, which needn't be unit length. Half angles, in radians.
    Spot {
        direction: [f32; 3],
        inner_angle: f32,
        outer_angle: f32,
    },
}

/// A curve through points, for a camera to ride.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedSpline {
    /// The name's hash ([`Name`]).
    pub id: u64,
    pub name: String,
    /// A Catmull-Rom's control points.
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
}

impl Zone {
    pub fn to_cooked(&self) -> Result<AlignedVec, ZoneError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value the runtime
    /// reads ([`ArchivedZone::validate`]).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedZone, ZoneError> {
        let zone = sapphire_serialize::access::<Self>(bytes)?;
        zone.validate()?;
        Ok(zone)
    }
}

fn native3(values: &rkyv::Archived<[f32; 3]>) -> [f32; 3] {
    values.each_ref().map(|value| value.to_native())
}

fn native4(values: &rkyv::Archived<[f32; 4]>) -> [f32; 4] {
    values.each_ref().map(|value| value.to_native())
}

impl ArchivedCookedSpawner {
    pub fn position(&self) -> [f32; 3] {
        native3(&self.position)
    }

    /// Its yaw in radians, as `ActorSpawn` takes it: an exact product, with no C math library in it.
    pub fn yaw(&self) -> f32 {
        self.yaw_degrees.to_native().to_radians()
    }
}

impl ArchivedCookedTrigger {
    pub fn position(&self) -> [f32; 3] {
        native3(&self.position)
    }
}

impl ArchivedCookedCheckpoint {
    pub fn position(&self) -> [f32; 3] {
        native3(&self.position)
    }

    /// Its yaw in radians, as `ActorSpawn` takes it: an exact product, with no C math library in it.
    pub fn yaw(&self) -> f32 {
        self.yaw_degrees.to_native().to_radians()
    }
}

impl ArchivedCookedVolume {
    pub fn to_native(&self) -> CookedVolume {
        match self {
            Self::Box { half_extents } => CookedVolume::Box { half_extents: native3(half_extents) },
            Self::Sphere { radius } => CookedVolume::Sphere { radius: radius.to_native() },
        }
    }

    /// Whether a point `offset` from the volume's center is inside it.
    pub fn holds(&self, offset: [f32; 3]) -> bool {
        match self.to_native() {
            CookedVolume::Box { half_extents } => (0..3).all(|axis| offset[axis].abs() <= half_extents[axis]),
            CookedVolume::Sphere { radius } => offset.iter().map(|v| v * v).sum::<f32>() <= radius * radius,
        }
    }
}

impl ArchivedPlaceContents {
    /// Around everything it holds, in the zone's space.
    pub fn bounds(&self) -> Bounds {
        Bounds { min: native3(&self.bounds_min), max: native3(&self.bounds_max) }
    }

    /// Its range in the presentation's instances.
    pub fn instances(&self) -> std::ops::Range<usize> {
        let first = self.first_instance.to_native() as usize;
        first..first + self.instance_count.to_native() as usize
    }

    /// Its range in the presentation's lights.
    pub fn lights(&self) -> std::ops::Range<usize> {
        let first = self.first_light.to_native() as usize;
        first..first + self.light_count.to_native() as usize
    }
}

impl ArchivedZoneCell {
    pub fn coord(&self) -> CellCoord {
        CellCoord { x: self.coord[0].to_native(), z: self.coord[1].to_native() }
    }
}

impl ArchivedCookedInstance {
    pub fn position(&self) -> [f32; 3] {
        native3(&self.position)
    }

    /// A unit quaternion: x, y, z, w.
    pub fn rotation(&self) -> [f32; 4] {
        native4(&self.rotation)
    }

    pub fn scale(&self) -> [f32; 3] {
        native3(&self.scale)
    }
}

impl ArchivedCookedLight {
    pub fn position(&self) -> [f32; 3] {
        native3(&self.position)
    }

    /// Linear RGB.
    pub fn color(&self) -> [f32; 3] {
        native3(&self.color)
    }

    pub fn shape(&self) -> CookedLightShape {
        match &self.shape {
            ArchivedCookedLightShape::Point => CookedLightShape::Point,
            ArchivedCookedLightShape::Spot { direction, inner_angle, outer_angle } => {
                CookedLightShape::Spot {
                    direction: native3(direction),
                    inner_angle: inner_angle.to_native(),
                    outer_angle: outer_angle.to_native(),
                }
            }
        }
    }
}

impl ArchivedCookedSpline {
    pub fn point(&self, index: usize) -> [f32; 3] {
        native3(&self.points[index])
    }
}

impl ArchivedZonePresentation {
    pub fn grid(&self) -> Grid {
        Grid { cell_size: self.cell_size.to_native() }
    }

    /// The cell at `coord`, if it holds anything.
    pub fn cell_at(&self, coord: CellCoord) -> Option<&ArchivedZoneCell> {
        let index = self.cells.binary_search_by_key(&coord, ArchivedZoneCell::coord).ok()?;
        self.cells.get(index)
    }
}

impl ArchivedZoneSimulation {
    /// The spawner named `name`.
    pub fn spawner(&self, name: Name) -> Option<&ArchivedCookedSpawner> {
        let index = self.spawners.binary_search_by_key(&name.as_u64(), |entry| entry.id.to_native()).ok()?;
        self.spawners.get(index)
    }

    /// The trigger named `name`.
    pub fn trigger(&self, name: Name) -> Option<&ArchivedCookedTrigger> {
        let index = self.triggers.binary_search_by_key(&name.as_u64(), |entry| entry.id.to_native()).ok()?;
        self.triggers.get(index)
    }

    /// The checkpoint named `name`.
    pub fn checkpoint(&self, name: Name) -> Option<&ArchivedCookedCheckpoint> {
        let index =
            self.checkpoints.binary_search_by_key(&name.as_u64(), |entry| entry.id.to_native()).ok()?;
        self.checkpoints.get(index)
    }
}

impl ArchivedZone {
    /// What loading the zone loads: what the simulation reads. These are the zone's dependencies in a
    /// pack, sorted, without repeats.
    pub fn references(&self) -> Vec<AssetId> {
        let simulation = &self.simulation;
        let colliders = simulation.colliders.as_ref().copied();
        let mut references: Vec<AssetId> = colliders
            .into_iter()
            .chain(simulation.navigation.as_ref().copied())
            .chain(simulation.encounters.iter().copied())
            .collect();
        references.sort_unstable();
        references.dedup();
        references
    }

    /// What the zone names without holding: the meshes of its cells and instances and its probe grid,
    /// for the world to ask for and let go of, and for a packer to place. Sorted, without repeats.
    pub fn presentation_references(&self) -> Vec<AssetId> {
        let presentation = &self.presentation;
        let contents = presentation
            .backdrop
            .as_ref()
            .into_iter()
            .chain(presentation.cells.iter().map(|cell| &cell.contents));
        let meshes = contents.filter_map(|contents| contents.mesh.as_ref().copied());
        let instances = presentation.instances.iter().map(|instance| instance.mesh);
        let probes = presentation.probes.as_ref().copied();
        let mut references: Vec<AssetId> = meshes.chain(instances).chain(probes).collect();
        references.sort_unstable();
        references.dedup();
        references
    }

    /// The digest a replay pins: of everything the simulation reads of this zone, itself and the cooked
    /// bytes of what it references, which `read` hands over by ID. Moving a spawner or saving a
    /// collider changes it; moving a cell's mesh doesn't.
    ///
    /// It hashes values rather than the archive: floats by their bits, integers little-endian, lists
    /// with their lengths, in the archive's order, which IDs fix. A collider set goes in as its cooked
    /// bytes, as `.scol` is the same on every target. `tests::digest_pins` holds this across
    /// architectures.
    pub fn simulation_digest<'a>(
        &self,
        read: impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<u64, ZoneError> {
        self.digest(read, true)
    }

    /// [`Self::simulation_digest`] without the navmesh, for a simulation that takes that apart: its
    /// `tables_hash` holds a navmesh's own digest once it's been given one, and a run that isn't given
    /// the one a zone names reads nothing of it.
    pub fn digest_without_navigation<'a>(
        &self,
        read: impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<u64, ZoneError> {
        self.digest(read, false)
    }

    fn digest<'a>(
        &self,
        mut read: impl FnMut(AssetId) -> Option<&'a [u8]>,
        navigation: bool,
    ) -> Result<u64, ZoneError> {
        let simulation = &self.simulation;
        let mut digest = Digest::new();
        digest.bytes(b"sapphire zone simulation");
        digest.u32(Zone::VERSION);

        digest.u32(simulation.spawners.len() as u32);
        for spawner in simulation.spawners.iter() {
            digest.u64(spawner.id.to_native());
            digest.floats(&spawner.position());
            digest.floats(&[spawner.yaw_degrees.to_native()]);
            digest.bytes(spawner.spawns.as_bytes());
        }
        digest.u32(simulation.triggers.len() as u32);
        for trigger in simulation.triggers.iter() {
            digest.u64(trigger.id.to_native());
            digest.floats(&trigger.position());
            digest.volume(trigger.volume.to_native());
            digest.bytes(trigger.event.as_bytes());
        }
        digest.u32(simulation.checkpoints.len() as u32);
        for checkpoint in simulation.checkpoints.iter() {
            digest.u64(checkpoint.id.to_native());
            digest.floats(&checkpoint.position());
            digest.floats(&[checkpoint.yaw_degrees.to_native()]);
            digest.volume(checkpoint.volume.to_native());
        }
        let mut referenced = |what: &'static str, id: AssetId, digest: &mut Digest| {
            digest.bytes(&id.to_bytes());
            digest.bytes(read(id).ok_or(ZoneError::NotLoaded { what, id })?);
            Ok::<(), ZoneError>(())
        };
        digest.u32(u32::from(simulation.colliders.is_some()));
        if let Some(id) = simulation.colliders.as_ref() {
            referenced("collider set", *id, &mut digest)?;
        }
        if navigation {
            digest.u32(u32::from(simulation.navigation.is_some()));
            if let Some(id) = simulation.navigation.as_ref() {
                referenced("navmesh", *id, &mut digest)?;
            }
        }
        digest.u32(simulation.encounters.len() as u32);
        for id in simulation.encounters.iter() {
            referenced("encounter", *id, &mut digest)?;
        }
        digest.u32(simulation.exits.len() as u32);
        for exit in simulation.exits.iter() {
            digest.u64(exit.id.to_native());
            digest.bytes(&exit.to.to_bytes());
            digest.bytes(exit.pack.as_bytes());
            digest.u64(exit.arrive.to_native());
        }
        Ok(digest.finish())
    }

    /// Checks everything the runtime reads, whatever wrote these bytes.
    pub fn validate(&self) -> Result<(), ZoneError> {
        self.validate_simulation()?;
        self.validate_presentation()
    }

    fn validate_simulation(&self) -> Result<(), ZoneError> {
        let simulation = &self.simulation;
        check_count("spawners", simulation.spawners.len())?;
        let spawners = simulation.spawners.iter().map(|entry| (entry.id.to_native(), entry.name.as_str()));
        check_ids("spawners", spawners)?;
        for spawner in simulation.spawners.iter() {
            let invalid =
                |problem| ZoneError::invalid(format!("spawner `{}`", spawner.name.as_str()), problem);
            check_position(spawner.position()).map_err(invalid)?;
            check_finite(spawner.yaw_degrees.to_native()).map_err(invalid)?;
            check_name(spawner.spawns.as_str()).map_err(invalid)?;
        }
        check_count("triggers", simulation.triggers.len())?;
        let triggers = simulation.triggers.iter().map(|entry| (entry.id.to_native(), entry.name.as_str()));
        check_ids("triggers", triggers)?;
        for trigger in simulation.triggers.iter() {
            let invalid =
                |problem| ZoneError::invalid(format!("trigger `{}`", trigger.name.as_str()), problem);
            check_position(trigger.position()).map_err(invalid)?;
            trigger.volume.to_native().check().map_err(invalid)?;
            check_name(trigger.event.as_str()).map_err(invalid)?;
        }
        check_count("checkpoints", simulation.checkpoints.len())?;
        let checkpoints =
            simulation.checkpoints.iter().map(|entry| (entry.id.to_native(), entry.name.as_str()));
        check_ids("checkpoints", checkpoints)?;
        for checkpoint in simulation.checkpoints.iter() {
            let invalid =
                |problem| ZoneError::invalid(format!("checkpoint `{}`", checkpoint.name.as_str()), problem);
            check_position(checkpoint.position()).map_err(invalid)?;
            check_finite(checkpoint.yaw_degrees.to_native()).map_err(invalid)?;
            checkpoint.volume.to_native().check().map_err(invalid)?;
        }
        check_count("encounters", simulation.encounters.len())?;
        check_ascending("encounters", simulation.encounters.as_slice())?;
        check_count("exits", simulation.exits.len())?;
        let exits = simulation.exits.iter().map(|entry| (entry.id.to_native(), entry.name.as_str()));
        check_ids("exits", exits)?;
        for exit in simulation.exits.iter() {
            let invalid = |problem| ZoneError::invalid(format!("exit `{}`", exit.name.as_str()), problem);
            check_name(exit.pack.as_str()).map_err(invalid)?;
        }
        Ok(())
    }

    fn validate_presentation(&self) -> Result<(), ZoneError> {
        let presentation = &self.presentation;
        let grid = presentation.grid();
        grid.check().map_err(|problem| ZoneError::invalid("cell_size", problem))?;
        check_count("cells", presentation.cells.len())?;
        check_count("instances", presentation.instances.len())?;
        check_count("lights", presentation.lights.len())?;

        for (index, instance) in presentation.instances.iter().enumerate() {
            let invalid = |problem| ZoneError::invalid(format!("instance {index}"), problem);
            check_position(instance.position()).map_err(invalid)?;
            check_scale(instance.scale()).map_err(invalid)?;
            let rotation = instance.rotation();
            let length = rotation.iter().map(|v| v * v).sum::<f32>();
            if !length.is_finite() || (length - 1.0).abs() > 1e-3 {
                return Err(invalid(format!("rotation {rotation:?} isn't unit length")));
            }
        }
        for (index, light) in presentation.lights.iter().enumerate() {
            let invalid = |problem| ZoneError::invalid(format!("light {index}"), problem);
            check_position(light.position()).map_err(invalid)?;
            check_color(light.color()).map_err(invalid)?;
            check_positive(light.intensity.to_native()).map_err(invalid)?;
            check_positive(light.range.to_native()).map_err(invalid)?;
            if let CookedLightShape::Spot { direction, inner_angle, outer_angle } = light.shape() {
                check_spot(direction, inner_angle, outer_angle).map_err(invalid)?;
            }
        }

        // The places share out the instances and lights in order, exactly.
        let mut instances = 0;
        let mut lights = 0;
        let mut previous: Option<CellCoord> = None;
        if let Some(backdrop) = presentation.backdrop.as_ref() {
            check_contents("backdrop", backdrop, presentation, &mut instances, &mut lights)?;
        }
        for cell in presentation.cells.iter() {
            let coord = cell.coord();
            let name = format!("cell ({}, {})", coord.x, coord.z);
            if previous.is_some_and(|previous| previous >= coord) {
                return Err(ZoneError::invalid(name, "out of order"));
            }
            previous = Some(coord);
            check_contents(&name, &cell.contents, presentation, &mut instances, &mut lights)?;
            let bounds = cell.contents.bounds();
            if let Some(reach) = grid.overreach(coord, bounds) {
                let problem = format!("its bounds reach {reach} m past it, more than a cell");
                return Err(ZoneError::invalid(name, problem));
            }
            let positions = cell.contents.instances().map(|index| presentation.instances[index].position());
            let lights_at = cell.contents.lights().map(|index| presentation.lights[index].position());
            if positions.chain(lights_at).any(|position| grid.cell_of(position) != coord) {
                return Err(ZoneError::invalid(name, "holds an instance or light from another cell"));
            }
        }
        if instances != presentation.instances.len() {
            let problem = format!("places hold {instances} of {}", presentation.instances.len());
            return Err(ZoneError::invalid("instances", problem));
        }
        if lights != presentation.lights.len() {
            let problem = format!("places hold {lights} of {}", presentation.lights.len());
            return Err(ZoneError::invalid("lights", problem));
        }

        check_count("splines", presentation.splines.len())?;
        let splines = presentation.splines.iter().map(|entry| (entry.id.to_native(), entry.name.as_str()));
        check_ids("splines", splines)?;
        for spline in presentation.splines.iter() {
            check_points(spline.points.len(), |index| spline.point(index)).map_err(|problem| {
                ZoneError::invalid(format!("spline `{}`", spline.name.as_str()), problem)
            })?;
        }
        Ok(())
    }
}

fn check_count(list: &str, count: usize) -> Result<(), ZoneError> {
    if count > MAX_ENTRIES {
        return Err(ZoneError::invalid(list, format!("{count} is more than the {MAX_ENTRIES} it may hold")));
    }
    Ok(())
}

/// Whether a place's bounds are real and its ranges start where the last place's ended and stay within
/// their lists, moving the running counts past them.
fn check_contents(
    name: &str,
    contents: &ArchivedPlaceContents,
    presentation: &ArchivedZonePresentation,
    instances: &mut usize,
    lights: &mut usize,
) -> Result<(), ZoneError> {
    if !contents.bounds().is_valid() {
        return Err(ZoneError::invalid(name, format!("bounds {:?} aren't a box", contents.bounds())));
    }
    if contents.first_instance.to_native() as usize != *instances
        || contents.first_light.to_native() as usize != *lights
    {
        return Err(ZoneError::invalid(name, "its ranges don't start where the last place's ended"));
    }
    *instances = contents.instances().end;
    *lights = contents.lights().end;
    if *instances > presentation.instances.len() || *lights > presentation.lights.len() {
        return Err(ZoneError::invalid(name, "its ranges run past the lists"));
    }
    Ok(())
}

/// Whether entries' names are good and each's ID is its name's hash, ascending: what looking one up by
/// a [`Name`] leans on, and what makes a save's ID mean the same thing again.
fn check_ids<'a>(list: &str, entries: impl Iterator<Item = (u64, &'a str)>) -> Result<(), ZoneError> {
    let mut previous = None;
    for (id, name) in entries {
        let invalid = |problem: String| ZoneError::invalid(format!("{list} `{name}`"), problem);
        check_name(name).map_err(invalid)?;
        if id != Name::new(name).as_u64() {
            return Err(invalid("its ID isn't its name's hash".to_owned()));
        }
        if previous.is_some_and(|previous| previous >= id) {
            return Err(invalid("out of order".to_owned()));
        }
        previous = Some(id);
    }
    Ok(())
}

fn check_ascending(list: &str, ids: &[AssetId]) -> Result<(), ZoneError> {
    if ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(ZoneError::invalid(list, "out of order"));
    }
    Ok(())
}

/// The digest's writer: each value's bytes, with what keeps two sequences of values from reading the
/// same.
struct Digest(Xxh3);

impl Digest {
    fn new() -> Self {
        Self(Xxh3::new())
    }

    fn u32(&mut self, value: u32) {
        self.0.update(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.update(&value.to_le_bytes());
    }

    fn floats(&mut self, values: &[f32]) {
        for value in values {
            self.u32(value.to_bits());
        }
    }

    /// Its shape's tag, then its extents.
    fn volume(&mut self, volume: CookedVolume) {
        match volume {
            CookedVolume::Box { half_extents } => {
                self.u32(0);
                self.floats(&half_extents);
            }
            CookedVolume::Sphere { radius } => {
                self.u32(1);
                self.floats(&[radius]);
            }
        }
    }

    /// With their length, so what follows can't read as part of them.
    fn bytes(&mut self, bytes: &[u8]) {
        self.u64(bytes.len() as u64);
        self.0.update(bytes);
    }

    fn finish(self) -> u64 {
        self.0.digest()
    }
}

#[cfg(test)]
mod tests {
    use sapphire_colliders::ColliderDocument;

    use super::*;
    use crate::document::{ZoneDocument, ZoneGeometry};
    use crate::grid::Place;

    const COLLIDERS: AssetId = AssetId::from_text("0aec06e8-ef8a-4f60-8dce-1e159cc277fa");
    const MESH: AssetId = AssetId::from_text("e94f690f-38e2-4630-aeb3-4d34d06fc233");
    const CELL_MESH: AssetId = AssetId::from_text("44f04da1-31ba-4d3f-9e65-3e38a30fff43");
    const BACKDROP_MESH: AssetId = AssetId::from_text("1e7721b7-7898-4d83-8d21-3ff9c59e5b6c");
    const PROBES: AssetId = AssetId::from_text("9df1c80b-9d2d-4100-a6cc-a16331f57cb5");
    const OTHER_ZONE: AssetId = AssetId::from_text("e8c21831-7d98-4a79-b592-912b806cf3e2");

    fn document() -> ZoneDocument {
        ZoneDocument::from_ron(&format!(
            r#"(
                type: "sapphire::zone::ZoneDocument",
                version: 1,
                data: (
                    grid: (cell_size: 32.0),
                    instances: [
                        (mesh: "{MESH}", at: (40.0, 0.0, -9.0), turn: 30.0),
                        (mesh: "{MESH}", at: (-3.0, 0.0, 70.0), scale: (2.0, 2.0, 2.0)),
                        (mesh: "{MESH}", at: (900.0, 0.0, 900.0), backdrop: true),
                    ],
                    lights: [
                        (position: (41.0, 2.5, -8.0), color: (1.0, 0.7, 0.4), intensity: 6.0, range: 9.0, at_night: true),
                        (
                            position: (-2.0, 3.0, 71.0), color: (1.0, 1.0, 1.0), intensity: 1.0, range: 4.0,
                            shape: Spot(direction: (0.0, -1.0, 0.0), inner_degrees: 20.0, outer_degrees: 30.0),
                        ),
                    ],
                    probes: Some("{PROBES}"),
                    colliders: Some("{COLLIDERS}"),
                    spawners: [
                        (name: "player_start", position: (0.1, 0.0, 3.3), yaw_degrees: 180.0, spawns: "player"),
                        (name: "north_gate", position: (0.7, 0.2, -17.3), yaw_degrees: 33.3, spawns: "warrior"),
                    ],
                    triggers: [
                        (name: "gate", position: (3.2, 1.0, -18.0), volume: Box(half_extents: (3.0, 2.0, 1.0)), event: "gate_entered"),
                        (name: "well", position: (0.3, 0.1, 0.7), volume: Sphere(radius: 2.5), event: "well_entered"),
                    ],
                    checkpoints: [
                        (name: "gate_steps", position: (3.2, 0.0, -14.0), yaw_degrees: 180.0, volume: Sphere(radius: 2.0)),
                        (name: "well_side", position: (1.5, 0.0, 1.0), volume: Box(half_extents: (1.0, 1.5, 1.0))),
                    ],
                    splines: [(name: "street_rail", points: [(3.2, 2.0, -20.0), (3.2, 2.5, -40.0)])],
                    exits: [
                        (name: "to_market", to: "{OTHER_ZONE}", arrive: "from_town", prefetch: true),
                        (name: "to_docks", to: "{OTHER_ZONE}", arrive: "player_start"),
                    ],
                ),
            )"#
        ))
        .expect("a good document")
    }

    fn zone() -> Zone {
        let boxed = |min: [f32; 3], max: [f32; 3]| Bounds { min, max };
        document()
            .cook(ZoneGeometry {
                meshes: [
                    (
                        Place::Backdrop,
                        (BACKDROP_MESH, boxed([-2000.0, -1.0, -2000.0], [2000.0, 40.0, 2000.0])),
                    ),
                    (
                        Place::Cell(CellCoord { x: 1, z: -1 }),
                        (CELL_MESH, boxed([33.0, 0.0, -30.0], [60.0, 9.0, -2.0])),
                    ),
                ]
                .into(),
                instances: vec![
                    boxed([38.0, 0.0, -11.0], [42.0, 5.0, -7.0]),
                    boxed([-5.0, 0.0, 68.0], [-1.0, 8.0, 72.0]),
                    boxed([800.0, 0.0, 800.0], [1000.0, 300.0, 1000.0]),
                ],
                day: Some("a day's text".to_owned()),
                rigs: None,
                navigation: None,
                exit_packs: vec!["zone_market".to_owned(), "zone_market".to_owned()],
            })
            .expect("cooks")
    }

    fn colliders() -> AlignedVec {
        ColliderDocument::from_ron(
            r#"(type: "sapphire::colliders::ColliderDocument", version: 1, data: (colliders: [
                (name: "ground", shape: Box(half_extents: (20.3, 0.5, 20.7)), position: (0.1, -0.5, 0.3)),
                (name: "crate", shape: Box(half_extents: (0.5, 0.5, 0.5)), position: (2.0, 0.5, 1.0),
                    rotation: AxisAngle(axis: (0.0, 1.0, 0.0), degrees: 33.3)),
            ]))"#,
        )
        .expect("parses")
        .cook()
        .expect("cooks")
        .to_cooked()
        .expect("writes")
    }

    #[test]
    fn a_cooked_zone_reads_back_in_place() {
        let bytes = zone().to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");

        let spawner = read.simulation.spawner(Name::new("player_start")).expect("there");
        assert_eq!(spawner.position(), [0.1, 0.0, 3.3]);
        assert_eq!(spawner.yaw(), 180f32.to_radians());
        assert_eq!(spawner.spawns.as_str(), "player");
        assert!(read.simulation.spawner(Name::new("nobody")).is_none());
        let trigger = read.simulation.trigger(Name::new("gate")).expect("there");
        assert_eq!(trigger.event.as_str(), "gate_entered");
        assert!(trigger.volume.holds([2.9, -1.9, 0.9]) && !trigger.volume.holds([3.1, 0.0, 0.0]));
        let well = read.simulation.trigger(Name::new("well")).expect("there");
        assert!(well.volume.holds([0.0, 2.4, 0.0]) && !well.volume.holds([2.0, 2.0, 0.0]));
        let steps = read.simulation.checkpoint(Name::new("gate_steps")).expect("there");
        assert_eq!(steps.position(), [3.2, 0.0, -14.0]);
        assert_eq!(steps.yaw(), 180f32.to_radians());
        assert!(steps.volume.holds([0.0, 1.9, 0.0]) && !steps.volume.holds([0.0, 2.1, 0.0]));
        assert!(
            read.simulation.checkpoint(Name::new("gate")).is_none(),
            "a trigger's name isn't a checkpoint's"
        );

        let presentation = &read.presentation;
        assert_eq!(presentation.day.as_ref().map(|day| day.as_str()), Some("a day's text"));
        let backdrop = presentation.backdrop.as_ref().expect("there");
        assert_eq!(backdrop.mesh.as_ref().copied(), Some(BACKDROP_MESH));
        assert_eq!(backdrop.instances(), 0..1);
        let cell = presentation.cell_at(CellCoord { x: 1, z: -1 }).expect("there");
        assert_eq!(cell.contents.mesh.as_ref().copied(), Some(CELL_MESH));
        assert_eq!(cell.contents.lights().len(), 1);
        assert_eq!(presentation.instances[cell.contents.instances().start].position(), [40.0, 0.0, -9.0]);
        assert_eq!(cell.contents.bounds().min[0], 32.0, "a light's reach counts");
        // A cell with an instance and a light and no mesh of its own.
        let other = presentation.cell_at(CellCoord { x: -1, z: 2 }).expect("there");
        assert!(other.contents.mesh.is_none());
        assert_eq!(other.contents.instances().len(), 1);
        assert!(presentation.cell_at(CellCoord { x: 0, z: 0 }).is_none());
        assert!(matches!(presentation.lights[0].shape(), CookedLightShape::Spot { .. }));
        assert_eq!(presentation.splines[0].point(1), [3.2, 2.5, -40.0]);
    }

    #[test]
    fn loading_a_zone_holds_what_the_simulation_reads_and_only_names_the_rest() {
        let bytes = zone().to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        assert_eq!(read.references(), [COLLIDERS]);
        let mut expected = vec![MESH, CELL_MESH, BACKDROP_MESH, PROBES];
        expected.sort_unstable();
        assert_eq!(read.presentation_references(), expected);
    }

    /// Whatever wrote these, `read` must refuse them: a pack's bytes needn't come from our cooker.
    #[test]
    fn cooked_data_the_document_would_refuse_is_refused() {
        type Tamper = fn(&mut Zone);
        let cases: [(&str, &str, Tamper); 26] = [
            ("cell size", "cell_size", |z| z.presentation.cell_size = 0.0),
            ("spawner order", "out of order", |z| z.simulation.spawners.reverse()),
            ("spawner id", "isn't its name's hash", |z| z.simulation.spawners[0].id ^= 1),
            ("spawner name", "empty", |z| z.simulation.spawners[0].name.clear()),
            ("spawner position", "finite", |z| z.simulation.spawners[1].position[0] = f32::NAN),
            ("spawner yaw", "finite", |z| z.simulation.spawners[1].yaw_degrees = f32::INFINITY),
            ("trigger order", "out of order", |z| z.simulation.triggers.reverse()),
            ("trigger volume", "extents", |z| {
                z.simulation.triggers[0].volume = CookedVolume::Box { half_extents: [0.0; 3] };
            }),
            ("trigger radius", "radius", |z| {
                z.simulation.triggers[0].volume = CookedVolume::Sphere { radius: f32::NAN };
            }),
            ("trigger event", "empty", |z| z.simulation.triggers[0].event.clear()),
            ("checkpoint order", "out of order", |z| z.simulation.checkpoints.reverse()),
            ("checkpoint id", "isn't its name's hash", |z| z.simulation.checkpoints[0].id ^= 1),
            ("checkpoint yaw", "finite", |z| z.simulation.checkpoints[1].yaw_degrees = f32::NAN),
            ("checkpoint volume", "radius", |z| {
                let gate =
                    z.simulation.checkpoints.iter_mut().find(|c| c.name == "gate_steps").expect("there");
                gate.volume = CookedVolume::Sphere { radius: 0.0 };
            }),
            ("encounters", "out of order", |z| z.simulation.encounters = vec![MESH, MESH]),
            ("cell order", "out of order", |z| z.presentation.cells[1].coord = z.presentation.cells[0].coord),
            ("bounds", "aren't a box", |z| z.presentation.cells[0].contents.bounds_min = [f32::NAN; 3]),
            ("bounds order", "aren't a box", |z| z.presentation.cells[0].contents.bounds_min[1] = 1e3),
            ("far bounds", "more than a cell", |z| z.presentation.cells[0].contents.bounds_max[0] = 9000.0),
            ("instance range", "ranges", |z| z.presentation.cells[0].contents.first_instance = 9),
            ("instance count", "places hold", |z| z.presentation.cells[1].contents.instance_count = 0),
            ("huge count", "ranges", |z| z.presentation.cells[0].contents.light_count = u32::MAX),
            ("instance turn", "unit length", |z| z.presentation.instances[0].rotation = [2.0, 0.0, 0.0, 0.0]),
            ("instance scale", "nonzero", |z| z.presentation.instances[0].scale = [0.0; 3]),
            ("light", "isn't positive", |z| z.presentation.lights[0].range = -1.0),
            ("spline", "points", |z| z.presentation.splines[0].points.clear()),
        ];
        for (what, expected, tamper) in cases {
            let mut tampered = zone();
            tamper(&mut tampered);
            let bytes = tampered.to_cooked().expect("write");
            let message = Zone::read(&bytes).expect_err(what).to_string();
            assert!(message.contains(expected), "{what}: `{message}` must say `{expected}`");
        }
    }

    /// An instance filed under a cell it doesn't stand in would stream at the wrong time.
    #[test]
    fn a_cell_holding_another_cells_instance_is_refused() {
        let mut tampered = zone();
        let instances = &mut tampered.presentation.instances;
        let (first, last) = (instances[1].position, instances[2].position);
        (instances[1].position, instances[2].position) = (last, first);
        let bytes = tampered.to_cooked().expect("write");
        let message = Zone::read(&bytes).expect_err("swapped").to_string();
        assert!(message.contains("another cell"), "{message}");
    }

    #[test]
    fn another_types_bytes_and_a_stale_version_are_refused() {
        #[derive(Clone, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
        #[type_path = "sapphire::zone"]
        struct Other {
            value: u32,
        }
        impl Versioned for Other {
            const VERSION: u32 = 1;
        }
        let other = sapphire_serialize::to_cooked(&Other { value: 7 }).expect("write");
        assert!(Zone::read(&other).is_err());

        let bytes = zone().to_cooked().expect("write");
        assert!(Zone::read(&bytes[..16]).is_err());
        assert!(Zone::read(&[]).is_err());
    }

    /// A small xorshift, so a failure repeats.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, limit: usize) -> usize {
            (self.next() % limit as u64) as usize
        }
    }

    /// Reads what's been let through: every accessor the runtime would use, so a value `validate` should
    /// have refused panics here. The reader's whole bargain is that this can't.
    fn use_all_of(zone: &ArchivedZone) {
        let mut seen = zone.references().len() + zone.presentation_references().len();
        let presentation = &zone.presentation;
        let contents = presentation
            .backdrop
            .as_ref()
            .into_iter()
            .chain(presentation.cells.iter().map(|cell| &cell.contents));
        for contents in contents {
            seen += presentation.instances[contents.instances()].len();
            seen += presentation.lights[contents.lights()].len();
        }
        for spline in presentation.splines.iter() {
            assert!(spline.point(spline.points.len() - 1)[0].is_finite());
        }
        for cell in presentation.cells.iter() {
            assert!(presentation.cell_at(cell.coord()).is_some(), "cells search by coordinate");
        }
        for spawner in zone.simulation.spawners.iter() {
            assert!(zone.simulation.spawner(Name::from_u64(spawner.id.to_native())).is_some());
        }
        for trigger in zone.simulation.triggers.iter() {
            assert!(zone.simulation.trigger(Name::from_u64(trigger.id.to_native())).is_some());
        }
        let colliders = colliders();
        let digest = zone.simulation_digest(|_| Some(&colliders[..]));
        assert!(digest.is_ok(), "{digest:?}");
        std::hint::black_box(seen);
    }

    /// A fuzz in small: bit flips, byte changes and cuts of a good archive, in the header and the
    /// payload. Each must be refused or read as safely as a good one, and never panic. The fixed seed
    /// repeats a failure. 40,000 flips at about a thousand bytes covers each bit several times over.
    #[test]
    fn damaged_bytes_are_refused_or_safe_and_never_panic() {
        let good = zone().to_cooked().expect("write");
        let mut random = Random(0x5a17_e0ff_1ce5_d00d);
        let mut still_read = 0;
        for round in 0..40_000 {
            let mut damaged = good.clone();
            match round % 4 {
                0 => {
                    let at = random.below(damaged.len());
                    damaged[at] ^= 1 << random.below(8);
                }
                1 => {
                    let at = random.below(damaged.len());
                    damaged[at] = random.next() as u8;
                }
                2 => {
                    // A word of all ones or zeroes, as a length or an offset might become.
                    let at = random.below(damaged.len() - 4);
                    let word = if random.next() & 1 == 0 { [0xff; 4] } else { [0; 4] };
                    damaged[at..at + 4].copy_from_slice(&word);
                }
                _ => {
                    damaged = AlignedVec::new();
                    damaged.extend_from_slice(&good[..random.below(good.len())]);
                }
            }
            if let Ok(zone) = Zone::read(&damaged) {
                still_read += 1;
                use_all_of(zone);
            }
        }
        // Some flips land in values that were never checked to be anything else (a mesh's ID).
        assert!(still_read > 0, "the test damaged nothing it could still read");
    }

    /// A digest is of what the simulation reads, and of nothing else.
    #[test]
    fn the_digest_follows_what_the_simulation_reads() {
        let digest_of = |zone: &Zone, colliders: &[u8]| {
            let bytes = zone.to_cooked().expect("write");
            Zone::read(&bytes).expect("read").simulation_digest(|_| Some(colliders)).expect("digests")
        };
        let colliders = colliders();
        let base = digest_of(&zone(), &colliders);

        type Tamper = fn(&mut Zone);
        let simulation: [(&str, Tamper); 10] = [
            ("a spawner's place", |z| z.simulation.spawners[0].position[1] += 0.25),
            ("a spawner's yaw", |z| z.simulation.spawners[0].yaw_degrees += 1.0),
            ("what spawns", |z| z.simulation.spawners[0].spawns.push('x')),
            ("a trigger's place", |z| z.simulation.triggers[1].position[2] += 0.25),
            ("a trigger's volume", |z| {
                z.simulation.triggers[1].volume = CookedVolume::Sphere { radius: 0.25 }
            }),
            ("a trigger's event", |z| z.simulation.triggers[1].event.push('x')),
            ("a checkpoint's place", |z| z.simulation.checkpoints[0].position[0] += 0.25),
            ("a checkpoint's yaw", |z| z.simulation.checkpoints[0].yaw_degrees += 1.0),
            ("a checkpoint's volume", |z| {
                z.simulation.checkpoints[1].volume = CookedVolume::Sphere { radius: 0.25 }
            }),
            ("no colliders", |z| z.simulation.colliders = None),
        ];
        for (what, tamper) in simulation {
            let mut moved = zone();
            tamper(&mut moved);
            assert_ne!(digest_of(&moved, &colliders), base, "{what}");
        }
        let presentation: [(&str, Tamper); 4] = [
            ("a cell's bounds", |z| z.presentation.cells[0].contents.bounds_max[1] += 1.0),
            ("a light", |z| z.presentation.lights[0].intensity += 1.0),
            ("the day", |z| z.presentation.day = None),
            ("a rail", |z| z.presentation.splines[0].points[0][0] += 1.0),
        ];
        for (what, tamper) in presentation {
            let mut moved = zone();
            tamper(&mut moved);
            assert_eq!(digest_of(&moved, &colliders), base, "{what}");
        }

        let mut saved = colliders.to_vec();
        saved[colliders.len() - 1] ^= 1;
        assert_ne!(digest_of(&zone(), &saved), base, "the colliders' bytes are in it");
    }

    /// An exit names a zone without leaning on it: nothing of it loads with this one or ships with it. A
    /// replay does care where it leads, and doesn't care how soon the pack comes in.
    #[test]
    fn an_exit_is_no_reference_and_its_way_is_in_the_digest() {
        let mut cooked = zone();
        let bytes = cooked.to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        assert!(!read.references().contains(&OTHER_ZONE));
        assert!(!read.presentation_references().contains(&OTHER_ZONE));
        assert_eq!(cooked.simulation.exits.len(), 2);
        assert!(cooked.simulation.exits.is_sorted_by_key(|exit| exit.id));
        let id = Name::new("to_market").as_u64();
        let at = cooked.simulation.exits.iter().position(|exit| exit.id == id).expect("cooked");
        assert_eq!(cooked.simulation.exits[at].pack, "zone_market");
        assert_eq!(cooked.simulation.exits[at].arrive, Name::new("from_town").as_u64());
        assert!(cooked.simulation.exits[at].prefetch);

        let colliders = colliders();
        let digest = |zone: &Zone| {
            let bytes = zone.to_cooked().expect("write");
            Zone::read(&bytes).expect("read").simulation_digest(|_| Some(&colliders[..])).expect("digests")
        };
        let before = digest(&cooked);
        cooked.simulation.exits[at].prefetch = false;
        assert_eq!(digest(&cooked), before, "how soon a pack loads is nobody's business but the frame's");
        cooked.simulation.exits[at].arrive = Name::new("elsewhere").as_u64();
        assert_ne!(digest(&cooked), before, "where it arrives is");
    }

    #[test]
    fn a_digest_says_what_it_lacks() {
        let bytes = zone().to_cooked().expect("write");
        let error = Zone::read(&bytes).expect("read").simulation_digest(|_| None).unwrap_err();
        assert!(
            matches!(error, ZoneError::NotLoaded { what: "collider set", id } if id == COLLIDERS),
            "{error}"
        );
    }

    /// A pack an exit names has to be one a mount can ask for.
    #[test]
    fn a_bad_pack_name_in_an_exit_is_refused_on_read() {
        let mut bad = zone();
        bad.simulation.exits[0].pack = String::new();
        let error = Zone::read(&bad.to_cooked().expect("write")).expect_err("refused").to_string();
        assert!(error.contains("exit `"), "{error}");
    }

    /// The test CI's Linux, Windows and macOS jobs read, on x86-64 and arm64. The digest goes through
    /// the process's stderr, past the test harness's capture, so a passing job's log shows it, and it's
    /// pinned, so a different one fails the job with the number. Search a job's log for
    /// `sapphire_zone digest`. The value moves when `Zone::VERSION`, the text below or the collider
    /// cook's output changes: set it to what this prints.
    #[test]
    fn digest_pins() {
        use std::io::Write as _;

        let colliders = colliders();
        let bytes = zone().to_cooked().expect("write");
        let digest =
            Zone::read(&bytes).expect("read").simulation_digest(|_| Some(&colliders[..])).expect("digests");
        let colliders_hash = xxhash_rust::xxh3::xxh3_64(&colliders);
        let line = format!(
            "sapphire_zone digest {digest:#018x} colliders {colliders_hash:#018x} on {}\n",
            std::env::consts::ARCH
        );
        std::io::stderr().write_all(line.as_bytes()).expect("stderr takes it");
        assert_eq!(digest, 0xb01a_6234_d8d2_2d78, "the digest moved; {line}");
    }
}
