//! The cooked zone (`.szone`): an rkyv archive the runtime reads in place.
//!
//! Two halves, because they live by different rules. [`ZoneSimulation`] loads before the first step and
//! stays, and a replay pins a digest of it ([`ArchivedZone::simulation_digest`]). [`ZonePresentation`]
//! gates what's drawn: the backdrop and the cells, whose meshes, instances and lights the world
//! activates by distance.
//!
//! No upgrades, like every cooked format: a changed layout takes a version bump and recooks from its
//! document. The simulation's half has a version of its own ([`ZoneSimulation`]'s), which the digest names,
//! so a change to the presentation's alone moves no replay.
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

use crate::atmosphere::CookedAtmosphere;
use crate::document::{
    MAX_ENTRIES, MIN_EXTENT, ZoneError, check_color, check_finite, check_name, check_points, check_position,
    check_positive, check_scale, check_spot,
};
use crate::grid::{Bounds, CellCoord, Grid, MAX_COORDINATE};
use crate::look::{ZoneFog, ZoneWeather, ZoneWind};
use crate::rooms::CookedRoom;
use crate::water::{
    ArchivedCookedWaterVolume, ArchivedWaterShape, CookedWaterSurface, CookedWaterVolume, MAX_BODIES,
    WaterShape, WaterSurface, check_shore_margin,
};
use crate::weather::CookedWeatherPlan;

mod v10;
mod v11;
mod v12;
mod v8;
mod v9;

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
    /// 1: M3 W1. 2: one navmesh in place of a list of tiles (P1). 3: exits (W4). 4: checkpoints (B4). 5:
    /// patrols, and who walks them (P3). 6: a sky material, wind, weather and fog in the presentation (M5 M6).
    /// 7: its reflection probes' asset (M5 L2). 8: a spot light's shadow flag (M5 L16). 9: water, in both
    /// halves (M6 W2). 10: rooms, in the presentation's (M6 U5). 11: a weather plan, in the presentation's
    /// (M6 W8a). 12: foliage's scatter sets, in the presentation's (M6 G1a). 13: atmosphere volumes, in the
    /// presentation's (M6 V1). Versions 8 to 12 are still read ([`Zone::upgrade`]).
    const VERSION: u32 = 13;
}

impl Versioned for ZoneSimulation {
    /// [`Zone::VERSION`] as of the last change to this half, which the digest names: a change to the
    /// presentation's alone bumps that and not this, so no replay's digest moves with it. 5: M3 P3. Water
    /// (M6 W2) left it there: the digest writes a water section only for a zone with water, so every zone
    /// without keeps its digest.
    const VERSION: u32 = 5;
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
    /// What spawners walk, by ascending ID: the points of the splines they name. A presentation's splines
    /// aren't in the digest, and these are.
    pub patrols: Vec<CookedPatrol>,
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
    /// Its water's volumes, by ascending ID, which [`WaterVolumes`](crate::water::WaterVolumes) asks.
    pub water: Vec<CookedWaterVolume>,
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
    /// The ID of what it walks, in [`ZoneSimulation::patrols`].
    pub patrol: Option<u64>,
}

/// Where a spawner's walker walks: the points of a spline it names, in order.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedPatrol {
    /// The name's hash ([`Name`]), as its spline's is.
    pub id: u64,
    pub name: String,
    /// Each on the zone's navmesh, and with a way to it from the one before: a cook checked.
    pub points: Vec<[f32; 3]>,
    /// Whether it goes on from its last point to its first, rather than turning back.
    pub closed: bool,
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
    /// Its reflection probes, baked by the cook beside its cells (`sapphire_assets::reflections`, M5 L2).
    pub reflections: Option<AssetId>,
    /// The time-of-day timeline's document, as RON text, its cook having checked it.
    pub day: Option<String>,
    /// The character light rig library's document, as RON text.
    pub rigs: Option<String>,
    /// Camera rails, by ascending ID.
    pub splines: Vec<CookedSpline>,
    /// The sky material its sky draws with, in place of the app's.
    pub sky: Option<AssetId>,
    /// The wind its plants sway in.
    pub wind: Option<ZoneWind>,
    /// How wet it is.
    pub weather: Option<ZoneWeather>,
    /// Its fog over its day's.
    pub fog: Option<ZoneFog>,
    /// Its water's surfaces, one for each of [`ZoneSimulation::water`], in its order.
    pub water: Vec<CookedWaterSurface>,
    /// Where its sound takes a reverb (M6 U5), by ascending ID: the [`rooms`](crate::rooms) module.
    pub rooms: Vec<CookedRoom>,
    /// Which weather states its sky shows (M6 W8a): the [`weather`](crate::weather) module.
    pub weather_plan: Option<CookedWeatherPlan>,
    /// What grows on it (M6 G1a): its foliage's kinds and a scatter set for each cell its layers reach, the
    /// [`foliage`](crate::foliage) module.
    pub foliage: Option<CookedZoneFoliage>,
    /// Its atmosphere volumes and the looks they name (M6 V1): the [`atmosphere`](crate::atmosphere) module.
    pub atmosphere: CookedAtmosphere,
}

/// A zone's foliage, cooked: the assets a renderer draws its plants from.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedZoneFoliage {
    /// The foliage document's kinds (`.sfoliage`), by its sidecar's ID.
    pub foliage: AssetId,
    /// A set for each cell its layers reach, some perhaps empty, by ascending `[x, z]`.
    pub cells: Vec<CookedScatterCell>,
}

/// A cell's scatter set, a sub-asset of the zone (`town.scatter.2.-1.sscatter`).
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedScatterCell {
    /// `[x, z]`.
    pub coord: [i32; 2],
    pub scatter: AssetId,
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
    /// A cone along `direction`, which needn't be unit length. Half angles, in radians. `shadow`: its
    /// author asks it to cast shadows (M5 L16).
    Spot {
        direction: [f32; 3],
        inner_angle: f32,
        outer_angle: f32,
        shadow: bool,
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

    /// Cooked zone bytes of an older version as this version's, checked by [`Self::read`]; `None` for bytes
    /// already at this version, which are left for the reader to check. Version 8 reads as a zone with no water,
    /// no rooms and no weather plan, version 9 as one with no rooms and no plan, version 10 as one with no plan,
    /// version 11 as one with no foliage, version 12 as one with no atmosphere volumes, each with the digest it
    /// had (cooker rule 2, for one release); none of them has atmosphere volumes. Any other version is [`CookedError::Stale`](sapphire_serialize::CookedError::Stale).
    pub fn upgrade(bytes: &[u8]) -> Result<Option<AlignedVec>, ZoneError> {
        let found = sapphire_serialize::cooked::schema_version::<Self>(bytes)?;
        if found == Self::VERSION {
            return Ok(None);
        }
        let zone = match found {
            v8::VERSION => v8::read(bytes)?,
            v9::VERSION => v9::read(bytes)?,
            v10::VERSION => v10::read(bytes)?,
            v11::VERSION => v11::read(bytes)?,
            v12::VERSION => v12::read(bytes)?,
            _ => return Err(sapphire_serialize::CookedError::Stale { found, current: Self::VERSION }.into()),
        };
        let cooked = zone.to_cooked()?;
        Self::read(&cooked)?;
        Ok(Some(cooked))
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
            ArchivedCookedLightShape::Spot { direction, inner_angle, outer_angle, shadow } => {
                CookedLightShape::Spot {
                    direction: native3(direction),
                    inner_angle: inner_angle.to_native(),
                    outer_angle: outer_angle.to_native(),
                    shadow: *shadow,
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

impl ArchivedCookedPatrol {
    /// Its points, in order.
    pub fn points(&self) -> impl Iterator<Item = [f32; 3]> + '_ {
        self.points.iter().map(native3)
    }
}

impl ArchivedCookedSpawner {
    /// What it walks: an ID in [`ArchivedZoneSimulation::patrols`], which [`ArchivedZone::validate`] checked.
    pub fn patrol(&self) -> Option<Name> {
        self.patrol.as_ref().map(|id| Name::from_u64(id.to_native()))
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

    /// The patrol named `name`.
    pub fn patrol(&self, name: Name) -> Option<&ArchivedCookedPatrol> {
        let index = self.patrols.binary_search_by_key(&name.as_u64(), |entry| entry.id.to_native()).ok()?;
        self.patrols.get(index)
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

    /// What the zone names without holding: the meshes of its cells and instances, its probe grid, its
    /// reflection probes, its sky material, its water's materials and its foliage's kinds and scatter sets, for
    /// the world to ask for and let go of, and for a packer to place. Sorted, without repeats.
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
        let reflections = presentation.reflections.as_ref().copied();
        let sky = presentation.sky.as_ref().copied();
        let water = presentation.water.iter().filter_map(|surface| surface.material.as_ref().copied());
        let foliage = presentation.foliage.as_ref().into_iter().flat_map(|foliage| {
            std::iter::once(foliage.foliage).chain(foliage.cells.iter().map(|cell| cell.scatter))
        });
        let mut references: Vec<AssetId> = meshes
            .chain(instances)
            .chain(probes)
            .chain(reflections)
            .chain(sky)
            .chain(water)
            .chain(foliage)
            .collect();
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
        digest.u32(ZoneSimulation::VERSION);

        digest.u32(simulation.spawners.len() as u32);
        for spawner in simulation.spawners.iter() {
            digest.u64(spawner.id.to_native());
            digest.floats(&spawner.position());
            digest.floats(&[spawner.yaw_degrees.to_native()]);
            digest.bytes(spawner.spawns.as_bytes());
            digest.u64(spawner.patrol().map_or(0, Name::as_u64));
        }
        digest.u32(simulation.patrols.len() as u32);
        for patrol in simulation.patrols.iter() {
            digest.u64(patrol.id.to_native());
            digest.u32(patrol.points.len() as u32);
            for point in patrol.points() {
                digest.floats(&point);
            }
            digest.u32(u32::from(patrol.closed));
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
        // Only a zone with water writes this, so every zone without has the digest it had before water was.
        if !simulation.water.is_empty() {
            digest.bytes(b"water");
            digest.u32(simulation.water.len() as u32);
            for body in simulation.water.iter() {
                digest.water(body);
            }
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
        check_count("patrols", simulation.patrols.len())?;
        let patrols = simulation.patrols.iter().map(|entry| (entry.id.to_native(), entry.name.as_str()));
        check_ids("patrols", patrols)?;
        for patrol in simulation.patrols.iter() {
            check_points(patrol.points.len(), |point| native3(&patrol.points[point])).map_err(|problem| {
                ZoneError::invalid(format!("patrol `{}`", patrol.name.as_str()), problem)
            })?;
        }
        for spawner in simulation.spawners.iter() {
            if let Some(patrol) = spawner.patrol()
                && simulation.patrol(patrol).is_none()
            {
                let problem = "walks a patrol the zone doesn't hold";
                return Err(ZoneError::invalid(format!("spawner `{}`", spawner.name.as_str()), problem));
            }
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
        validate_water(simulation)
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
            if let CookedLightShape::Spot { direction, inner_angle, outer_angle, .. } = light.shape() {
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
        if let Some(wind) = presentation.wind.as_ref() {
            wind.to_native().check().map_err(|problem| ZoneError::invalid("wind", problem))?;
        }
        if let Some(weather) = presentation.weather.as_ref() {
            weather.to_native().check().map_err(|problem| ZoneError::invalid("weather", problem))?;
        }
        if let Some(fog) = presentation.fog.as_ref() {
            fog.to_native().check().map_err(|problem| ZoneError::invalid("fog", problem))?;
        }
        crate::rooms::validate(presentation)?;
        crate::weather::validate(presentation)?;
        crate::atmosphere::validate(presentation)?;
        self.validate_foliage()?;
        self.validate_water_surfaces()
    }

    /// Whether its scatter sets are at most [`MAX_CELLS`](crate::foliage::MAX_CELLS), by ascending cell, each
    /// once.
    fn validate_foliage(&self) -> Result<(), ZoneError> {
        let Some(foliage) = self.presentation.foliage.as_ref() else { return Ok(()) };
        if foliage.cells.len() > crate::foliage::MAX_CELLS {
            let problem = format!("{} scatter sets is more than a zone may hold", foliage.cells.len());
            return Err(ZoneError::invalid("foliage", problem));
        }
        let coords = foliage.cells.iter().map(|cell| cell.coord.each_ref().map(|value| value.to_native()));
        let coords: Vec<[i32; 2]> = coords.collect();
        if coords.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ZoneError::invalid(
                "foliage",
                "its scatter sets aren't by ascending cell, each once",
            ));
        }
        Ok(())
    }

    /// Whether each body has one surface, in the simulation's order, with a margin a cook would write and a
    /// probe only where the zone has reflections.
    fn validate_water_surfaces(&self) -> Result<(), ZoneError> {
        let (volumes, surfaces) = (&self.simulation.water, &self.presentation.water);
        if volumes.len() != surfaces.len() {
            let problem = format!("{} surfaces for {} bodies", surfaces.len(), volumes.len());
            return Err(ZoneError::invalid("water", problem));
        }
        for (volume, surface) in volumes.iter().zip(surfaces.iter()) {
            let invalid =
                |problem: String| ZoneError::invalid(format!("water `{}`", volume.name.as_str()), problem);
            if surface.id != volume.id {
                return Err(invalid("its surface is another body's".to_owned()));
            }
            check_shore_margin(surface.shore_margin.to_native()).map_err(invalid)?;
            if surface.probe.is_some() && self.presentation.reflections.is_none() {
                return Err(invalid("it reflects a probe, and the zone has none".to_owned()));
            }
        }
        Ok(())
    }
}

/// Whether the simulation's water is what a document's check and cook would make: few enough bodies and one
/// sea, each shape one the query can answer over, and bounds exactly the ones its shape has, since the query
/// trusts them to skip a body.
fn validate_water(simulation: &ArchivedZoneSimulation) -> Result<(), ZoneError> {
    if simulation.water.len() > MAX_BODIES {
        let problem =
            format!("{} bodies is more than the {MAX_BODIES} a zone may hold", simulation.water.len());
        return Err(ZoneError::invalid("water", problem));
    }
    check_ids("water", simulation.water.iter().map(|entry| (entry.id.to_native(), entry.name.as_str())))?;
    let mut seas = 0;
    for body in simulation.water.iter() {
        let invalid =
            |problem: String| ZoneError::invalid(format!("water `{}`", body.name.as_str()), problem);
        let shape = rkyv::deserialize::<WaterShape, rkyv::rancor::Error>(&body.shape)
            .map_err(|error| invalid(error.to_string()))?;
        shape.check().map_err(|(field, problem)| invalid(format!("{field}: {problem}")))?;
        if body.bounds() != shape.bounds() {
            return Err(invalid(format!("bounds {:?} aren't its shape's", body.bounds())));
        }
        if matches!(body.shape, ArchivedWaterShape::Ocean { .. }) {
            seas += 1;
            if seas > 1 {
                return Err(invalid("a zone has one sea at most".to_owned()));
            }
        }
    }
    Ok(())
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
pub(crate) fn check_ids<'a>(
    list: &str,
    entries: impl Iterator<Item = (u64, &'a str)>,
) -> Result<(), ZoneError> {
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

    /// A water body's ID, its shape's tag and values, its surface and whether it swims. Its bounds follow from
    /// its shape, and its name from its ID.
    fn water(&mut self, body: &ArchivedCookedWaterVolume) {
        self.u64(body.id.to_native());
        match &body.shape {
            ArchivedWaterShape::Lake { outline, height } => {
                self.u32(0);
                self.u32(outline.len() as u32);
                for point in outline.iter() {
                    self.floats(&point.each_ref().map(|value| value.to_native()));
                }
                self.floats(&[height.to_native()]);
            }
            ArchivedWaterShape::River { points } => {
                self.u32(1);
                self.u32(points.len() as u32);
                for point in points.iter() {
                    let point = point.to_native();
                    self.floats(&point.at);
                    self.floats(&[point.width, point.speed]);
                }
            }
            ArchivedWaterShape::Ocean { height } => {
                self.u32(2);
                self.floats(&[height.to_native()]);
            }
        }
        self.u32(u32::from(body.surface() == WaterSurface::Frozen));
        self.u32(u32::from(body.swim));
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
    use crate::atmosphere::AtmosphereVolume;
    use crate::document::{Volume, ZoneDocument, ZoneGeometry};
    use crate::grid::Place;
    use crate::rooms::{MAX_ROOMS, ZoneRoom};
    use crate::shape::ZoneShape;
    use crate::water::{RiverPoint, WaterBody, WaterKind, WaterVolumes};
    use crate::weather::{ArchivedWeatherSlot, WeatherChoice, WeatherRegion, WeatherSlot, ZoneWeatherPlan};

    const COLLIDERS: AssetId = AssetId::from_text("0aec06e8-ef8a-4f60-8dce-1e159cc277fa");
    const MESH: AssetId = AssetId::from_text("e94f690f-38e2-4630-aeb3-4d34d06fc233");
    const CELL_MESH: AssetId = AssetId::from_text("44f04da1-31ba-4d3f-9e65-3e38a30fff43");
    const BACKDROP_MESH: AssetId = AssetId::from_text("1e7721b7-7898-4d83-8d21-3ff9c59e5b6c");
    const PROBES: AssetId = AssetId::from_text("9df1c80b-9d2d-4100-a6cc-a16331f57cb5");
    const OTHER_ZONE: AssetId = AssetId::from_text("e8c21831-7d98-4a79-b592-912b806cf3e2");
    const NAVIGATION: AssetId = AssetId::from_text("6d7b7a5c-8c5e-4d7f-9b0e-3a1f2c4d5e6f");
    const SKY: AssetId = AssetId::from_text("5d1c7a2e-90b4-4e3f-8c61-2a7f0d9b4e15");
    const REFLECTIONS: AssetId = AssetId::from_text("0b3f6c1d-7a52-4e98-b4d1-93c2e8f05a67");
    const WATER_MATERIAL: AssetId = AssetId::from_text("7c2e9a41-5b3d-4f08-a6e1-0d94b2c7f318");

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
                    reflections: [(name: "gate", position: (3.2, 1.6, -20.0), parallax: (min: (-6.0, 0.0, -30.0), max: (12.0, 9.0, -10.0)))],
                    sky: Some("{SKY}"),
                    wind: Some((direction: (1.0, 0.0, 0.0), strength: 0.5, gustiness: 0.2)),
                    weather: Some((wetness: 0.6, puddles: 0.3)),
                    fog: Some((density: 1.5, height_base: Some(-0.5))),
                    colliders: Some("{COLLIDERS}"),
                    navigation: Some((min: (-64.0, -64.0), max: (64.0, 64.0))),
                    spawners: [
                        (name: "player_start", position: (0.1, 0.0, 3.3), yaw_degrees: 180.0, spawns: "player"),
                        (name: "north_gate", position: (0.7, 0.2, -17.3), yaw_degrees: 33.3, spawns: "warrior", patrol: "gate_walk"),
                    ],
                    triggers: [
                        (name: "gate", position: (3.2, 1.0, -18.0), volume: Box(half_extents: (3.0, 2.0, 1.0)), event: "gate_entered"),
                        (name: "well", position: (0.3, 0.1, 0.7), volume: Sphere(radius: 2.5), event: "well_entered"),
                    ],
                    checkpoints: [
                        (name: "gate_steps", position: (3.2, 0.0, -14.0), yaw_degrees: 180.0, volume: Sphere(radius: 2.0)),
                        (name: "well_side", position: (1.5, 0.0, 1.0), volume: Box(half_extents: (1.0, 1.5, 1.0))),
                    ],
                    splines: [
                        (name: "street_rail", points: [(3.2, 2.0, -20.0), (3.2, 2.5, -40.0)]),
                        (name: "gate_walk", points: [(0.7, 0.2, -17.3), (0.7, 0.2, -25.0)], closed: true),
                    ],
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
        cook(&document())
    }

    /// [`zone`] with water: a moat reflecting the gate's probe, a brook nobody swims in and the sea.
    fn water_zone() -> Zone {
        let mut document = document();
        document.water = vec![
            WaterBody {
                name: "moat".to_owned(),
                shape: WaterShape::Lake {
                    outline: vec![[-10.0, -30.0], [10.0, -30.0], [10.0, -24.0], [-10.0, -24.0]],
                    height: -0.5,
                },
                surface: WaterSurface::Liquid,
                material: Some(WATER_MATERIAL),
                swim: true,
                probe: Some("gate".to_owned()),
                shore_margin: 2.0,
            },
            WaterBody {
                name: "brook".to_owned(),
                shape: WaterShape::River {
                    points: vec![
                        RiverPoint { at: [20.0, 1.0, -40.0], width: 2.0, speed: 1.0 },
                        RiverPoint { at: [24.0, 0.5, -20.0], width: 3.0, speed: 1.5 },
                    ],
                },
                surface: WaterSurface::Liquid,
                material: None,
                swim: false,
                probe: None,
                shore_margin: 1.0,
            },
            WaterBody {
                name: "sea".to_owned(),
                shape: WaterShape::Ocean { height: -3.0 },
                surface: WaterSurface::Liquid,
                material: Some(WATER_MATERIAL),
                swim: true,
                probe: None,
                shore_margin: 4.0,
            },
        ];
        cook(&document)
    }

    /// What a cook makes of [`document`]'s geometry.
    fn geometry() -> ZoneGeometry {
        let boxed = |min: [f32; 3], max: [f32; 3]| Bounds { min, max };
        ZoneGeometry {
            meshes: [
                (Place::Backdrop, (BACKDROP_MESH, boxed([-2000.0, -1.0, -2000.0], [2000.0, 40.0, 2000.0]))),
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
            weather: None,
            navigation: Some(NAVIGATION),
            exit_packs: vec!["zone_market".to_owned(), "zone_market".to_owned()],
            reflections: Some(REFLECTIONS),
            foliage: None,
            volume_looks: Vec::new(),
        }
    }

    fn cook(document: &ZoneDocument) -> Zone {
        document.cook(geometry()).expect("cooks")
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
        assert_eq!(presentation.sky.as_ref().copied(), Some(SKY));
        assert_eq!(presentation.reflections.as_ref().copied(), Some(REFLECTIONS));
        let wind = presentation.wind.as_ref().expect("there").to_native();
        assert_eq!(wind, ZoneWind { direction: [1.0, 0.0, 0.0], strength: 0.5, gustiness: 0.2 });
        let weather = presentation.weather.as_ref().expect("there").to_native();
        assert_eq!(weather, ZoneWeather { wetness: 0.6, puddles: 0.3 });
        let fog = presentation.fog.as_ref().expect("there").to_native();
        assert_eq!(fog, ZoneFog { density: 1.5, height_base: Some(-0.5) });
    }

    #[test]
    fn loading_a_zone_holds_what_the_simulation_reads_and_only_names_the_rest() {
        let bytes = zone().to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        assert_eq!(read.references(), [COLLIDERS, NAVIGATION]);
        let mut expected = vec![MESH, CELL_MESH, BACKDROP_MESH, PROBES, REFLECTIONS, SKY];
        expected.sort_unstable();
        assert_eq!(read.presentation_references(), expected);
    }

    /// Whatever wrote these, `read` must refuse them: a pack's bytes needn't come from our cooker.
    #[test]
    fn cooked_data_the_document_would_refuse_is_refused() {
        type Tamper = fn(&mut Zone);
        let cases: [(&str, &str, Tamper); 32] = [
            ("cell size", "cell_size", |z| z.presentation.cell_size = 0.0),
            ("spawner order", "out of order", |z| z.simulation.spawners.reverse()),
            ("spawner id", "isn't its name's hash", |z| z.simulation.spawners[0].id ^= 1),
            ("spawner name", "empty", |z| z.simulation.spawners[0].name.clear()),
            ("spawner position", "finite", |z| z.simulation.spawners[1].position[0] = f32::NAN),
            ("spawner yaw", "finite", |z| z.simulation.spawners[1].yaw_degrees = f32::INFINITY),
            ("patrol's id", "isn't its name's hash", |z| z.simulation.patrols[0].id ^= 1),
            ("patrol's points", "finite", |z| z.simulation.patrols[0].points[1][2] = f32::NAN),
            ("spawner's patrol", "doesn't hold", |z| z.simulation.spawners[0].patrol = Some(3)),
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
            ("wind", "horizontal", |z| {
                z.presentation.wind.as_mut().expect("there").direction = [0.0, 1.0, 0.0]
            }),
            ("weather", "wetness", |z| z.presentation.weather.as_mut().expect("there").wetness = 2.0),
            ("fog", "density", |z| z.presentation.fog.as_mut().expect("there").density = f32::NAN),
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
        for spawner in zone.simulation.spawners.iter() {
            if let Some(patrol) = spawner.patrol() {
                assert!(zone.simulation.patrol(patrol).is_some_and(|walk| walk.points().count() > 1));
            }
        }
        let volumes = WaterVolumes::new(&zone.simulation, |_, from_y: f32, _, _| Some(from_y - 1.0));
        for x in [-20.0, 0.0, 21.0, 22.0, 9000.0] {
            for z in [-40.0, -27.0, -30.0, 0.0] {
                if let Some(sample) = volumes.at(x, -1.0, z) {
                    assert!(volumes.body(sample.body).is_some() && sample.surface_y.is_finite());
                    seen += 1;
                }
            }
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
        let good = water_zone().to_cooked().expect("write");
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
        let simulation: [(&str, Tamper); 13] = [
            ("a spawner's place", |z| z.simulation.spawners[0].position[1] += 0.25),
            ("a spawner's yaw", |z| z.simulation.spawners[0].yaw_degrees += 1.0),
            ("what spawns", |z| z.simulation.spawners[0].spawns.push('x')),
            ("who walks", |z| {
                let walker = z.simulation.spawners.iter_mut().find(|spawner| spawner.patrol.is_some());
                walker.expect("someone").patrol = None;
            }),
            ("a patrol's point", |z| z.simulation.patrols[0].points[0][0] += 0.25),
            ("whether it's closed", |z| z.simulation.patrols[0].closed ^= true),
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
        let presentation: [(&str, Tamper); 9] = [
            ("a cell's bounds", |z| z.presentation.cells[0].contents.bounds_max[1] += 1.0),
            ("the reflections", |z| z.presentation.reflections = None),
            ("a light", |z| z.presentation.lights[0].intensity += 1.0),
            ("the day", |z| z.presentation.day = None),
            ("a rail", |z| z.presentation.splines[0].points[0][0] += 1.0),
            ("the sky", |z| z.presentation.sky = None),
            ("the wind", |z| z.presentation.wind = None),
            ("the weather", |z| z.presentation.weather = None),
            ("the fog", |z| z.presentation.fog = None),
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
    /// `sapphire_zone digest`. The value moves when `ZoneSimulation::VERSION`, the text below or the collider
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
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "the digest moved; {line}");
    }

    /// [`digest_pins`] for a zone with water, which only such a zone's digest writes: the moat, the brook and
    /// the sea move it from the same zone's without, and it's pinned the same way (`sapphire_zone water
    /// digest` in a job's log). It moves when the water section's text or order changes.
    #[test]
    fn water_moves_the_digest_and_is_pinned() {
        use std::io::Write as _;

        let colliders = colliders();
        let digest_of = |zone: &Zone| {
            let bytes = zone.to_cooked().expect("write");
            Zone::read(&bytes).expect("read").simulation_digest(|_| Some(&colliders[..])).expect("digests")
        };
        let digest = digest_of(&water_zone());
        assert_ne!(digest, digest_of(&zone()), "water is in the digest");
        let line = format!("sapphire_zone water digest {digest:#018x} on {}\n", std::env::consts::ARCH);
        std::io::stderr().write_all(line.as_bytes()).expect("stderr takes it");
        assert_eq!(digest, 0x131b_a8cb_4d77_d2af, "the water digest moved; {line}");
    }

    /// Every value the query reads is in the digest, and nothing it doesn't: a body's material, probe and
    /// shore margin are drawing's.
    #[test]
    fn the_digest_follows_what_the_query_reads() {
        let colliders = colliders();
        let digest_of = |zone: &Zone| {
            let bytes = zone.to_cooked().expect("write");
            Zone::read(&bytes).expect("read").simulation_digest(|_| Some(&colliders[..])).expect("digests")
        };
        let base = digest_of(&water_zone());
        let at = |zone: &Zone, name: &str| {
            zone.simulation.water.iter().position(|body| body.name == name).expect("there")
        };
        type Tamper = fn(&mut Zone, usize, usize, usize);
        let simulation: [(&str, Tamper); 9] = [
            ("a lake's point", |z, moat, _, _| {
                let WaterShape::Lake { outline, .. } = &mut z.simulation.water[moat].shape else {
                    unreachable!()
                };
                outline[2][1] += 0.25;
            }),
            ("a lake's height", |z, moat, _, _| {
                let WaterShape::Lake { height, .. } = &mut z.simulation.water[moat].shape else {
                    unreachable!()
                };
                *height += 0.25;
            }),
            ("a river's point", |z, _, brook, _| {
                let WaterShape::River { points } = &mut z.simulation.water[brook].shape else {
                    unreachable!()
                };
                points[1].at[1] += 0.25;
            }),
            ("a river's width", |z, _, brook, _| {
                let WaterShape::River { points } = &mut z.simulation.water[brook].shape else {
                    unreachable!()
                };
                points[0].width += 0.25;
            }),
            ("a river's speed", |z, _, brook, _| {
                let WaterShape::River { points } = &mut z.simulation.water[brook].shape else {
                    unreachable!()
                };
                points[0].speed += 0.25;
            }),
            ("the sea's height", |z, _, _, sea| {
                z.simulation.water[sea].shape = WaterShape::Ocean { height: -2.0 }
            }),
            ("a frozen surface", |z, moat, _, _| z.simulation.water[moat].surface = WaterSurface::Frozen),
            ("whether it swims", |z, _, brook, _| z.simulation.water[brook].swim = true),
            ("a body fewer", |z, _, brook, _| {
                z.simulation.water.remove(brook);
                z.presentation.water.remove(brook);
            }),
        ];
        let zone = water_zone();
        let (moat, brook, sea) = (at(&zone, "moat"), at(&zone, "brook"), at(&zone, "sea"));
        for (what, tamper) in simulation {
            let mut moved = water_zone();
            tamper(&mut moved, moat, brook, sea);
            if let Some(body) = moved.simulation.water.get_mut(moat) {
                (body.bounds_min, body.bounds_max) = body.shape.bounds();
            }
            if let Some(body) = moved.simulation.water.get_mut(brook) {
                (body.bounds_min, body.bounds_max) = body.shape.bounds();
            }
            assert_ne!(digest_of(&moved), base, "{what}");
        }
        let presentation: [(&str, Tamper); 3] = [
            ("a material", |z, moat, _, _| z.presentation.water[moat].material = None),
            ("a probe", |z, moat, _, _| z.presentation.water[moat].probe = None),
            ("a shore margin", |z, _, _, sea| z.presentation.water[sea].shore_margin = 0.5),
        ];
        for (what, tamper) in presentation {
            let mut moved = water_zone();
            tamper(&mut moved, moat, brook, sea);
            assert_eq!(digest_of(&moved), base, "{what}");
        }
    }

    /// Water reads back in place: the query answers over the archive, and drawing finds each body's surface
    /// and its material among what the zone names.
    #[test]
    fn a_cooked_zones_water_reads_back() {
        let bytes = water_zone().to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        assert!(read.presentation_references().contains(&WATER_MATERIAL));
        assert!(!read.references().contains(&WATER_MATERIAL), "drawing's, never the simulation's");
        let volumes = WaterVolumes::new(&read.simulation, |_, from_y: f32, _, _| Some(from_y - 2.0));
        let moat = volumes.at(0.0, -1.0, -27.0).expect("the moat");
        assert_eq!((moat.kind, moat.surface_y, moat.depth), (WaterKind::Lake, -0.5, Some(2.0)));
        let surface = &read.presentation.water[usize::from(moat.body)];
        assert_eq!(surface.id, read.simulation.water[usize::from(moat.body)].id);
        assert_eq!(
            (surface.probe.as_ref().map(|probe| probe.to_native()), surface.shore_margin.to_native()),
            (Some(0), 2.0)
        );
        let brook = volumes.at(22.0, 0.0, -30.0).expect("the brook");
        assert_eq!((brook.kind, brook.swim), (WaterKind::River, false));
        assert_eq!(volumes.at(0.0, -4.0, 0.0).expect("the sea").kind, WaterKind::Ocean);
        assert_eq!(volumes.at(0.0, -2.0, 0.0), None, "over the sea, off the moat");
    }

    /// Whatever wrote these, `read` must refuse water the document would: a pack's bytes needn't come from our
    /// cooker, and the query trusts a body's bounds.
    #[test]
    fn cooked_water_the_document_would_refuse_is_refused() {
        type Tamper = fn(&mut Zone);
        let cases: [(&str, &str, Tamper); 10] = [
            ("order", "out of order", |z| z.simulation.water.reverse()),
            ("id", "isn't its name's hash", |z| z.simulation.water[0].id ^= 1),
            ("bounds", "aren't its shape's", |z| z.simulation.water[0].bounds_max[0] += 1.0),
            ("two seas", "one sea", |z| {
                let lake = z.simulation.water.iter_mut().find(|body| body.name == "moat").expect("there");
                lake.shape = WaterShape::Ocean { height: 0.0 };
                (lake.bounds_min, lake.bounds_max) = lake.shape.bounds();
            }),
            ("an outline", "cross or touch", |z| {
                let lake = z.simulation.water.iter_mut().find(|body| body.name == "moat").expect("there");
                let WaterShape::Lake { outline, .. } = &mut lake.shape else { unreachable!() };
                outline.swap(0, 1);
            }),
            ("a river", "width", |z| {
                let brook = z.simulation.water.iter_mut().find(|body| body.name == "brook").expect("there");
                let WaterShape::River { points } = &mut brook.shape else { unreachable!() };
                points[0].width = f32::NAN;
            }),
            ("surfaces", "surfaces for", |z| {
                z.presentation.water.pop();
            }),
            ("a surface's body", "another body's", |z| z.presentation.water.swap(0, 1)),
            ("a margin", "from 0 to", |z| z.presentation.water[0].shore_margin = f32::INFINITY),
            ("a probe", "has none", |z| {
                z.presentation.reflections = None;
                z.presentation.water.iter_mut().for_each(|surface| surface.probe = Some(0));
            }),
        ];
        for (what, expected, tamper) in cases {
            let mut tampered = water_zone();
            tamper(&mut tampered);
            let bytes = tampered.to_cooked().expect("write");
            let message = Zone::read(&bytes).expect_err(what).to_string();
            assert!(message.contains(expected), "{what}: `{message}` must say `{expected}`");
        }
        let mut crowded = water_zone();
        let body = crowded.simulation.water[0].clone();
        crowded.simulation.water = (0..=MAX_BODIES)
            .map(|index| {
                let name = format!("pool_{index}");
                CookedWaterVolume { id: Name::new(&name).as_u64(), name, ..body.clone() }
            })
            .collect();
        crowded.simulation.water.sort_unstable_by_key(|body| body.id);
        let message = Zone::read(&crowded.to_cooked().expect("write")).expect_err("17").to_string();
        assert!(message.contains("more than the 16"), "{message}");
    }

    /// Version 8 bytes, as M5 cooked them, read for one release (cooker rule 2): `read` refuses them as stale,
    /// and `upgrade` makes them this version's, a zone with no water whose digest is the one it had.
    #[test]
    fn a_version_8_zone_reads_as_one_with_no_water() {
        let current = zone();
        let old = v8::write(&current);
        let message = Zone::read(&old).expect_err("stale").to_string();
        assert!(message.contains("schema version 8") && message.contains("recook"), "{message}");

        let upgraded = Zone::upgrade(&old).expect("reads").expect("version 8 bytes are rewritten");
        let bytes = current.to_cooked().expect("write");
        assert_eq!(upgraded.as_slice(), bytes.as_slice(), "the very bytes a cook writes today");
        assert!(Zone::upgrade(&bytes).expect("reads").is_none(), "today's bytes are left as they are");

        // Water dropped on the way to version 8 comes back as none, and the digest is the zone's without it.
        let colliders = colliders();
        let from_water = Zone::upgrade(&v8::write(&water_zone())).expect("reads").expect("rewritten");
        let read = Zone::read(&from_water).expect("read");
        assert!(read.simulation.water.is_empty() && read.presentation.water.is_empty());
        assert_eq!(
            read.simulation_digest(|_| Some(&colliders[..])).expect("digests"),
            0x15dc_b5da_8e03_1351,
            "digest_pins's"
        );
    }

    /// [`zone`] with rooms (M6 U5): the street outdoors and a gatehouse over it.
    fn room_zone() -> Zone {
        let mut document = document();
        document.rooms = vec![
            ZoneRoom {
                name: "street".to_owned(),
                position: [0.0, 6.0, 0.0],
                volume: Volume::Box { half_extents: [60.0, 6.0, 60.0] },
                reverb: "street".to_owned(),
                priority: 0,
                outdoor: true,
                fade: 0.5,
            },
            ZoneRoom {
                name: "gatehouse".to_owned(),
                position: [3.2, 2.0, -20.0],
                volume: Volume::Sphere { radius: 4.0 },
                reverb: "shelter".to_owned(),
                priority: 2,
                outdoor: false,
                fade: 0.4,
            },
        ];
        cook(&document)
    }

    /// Rooms are presentation: they read back by ascending ID as written, and the digest is [`digest_pins`']
    /// for the same zone without them.
    #[test]
    fn rooms_read_back_and_leave_the_digest_alone() {
        let colliders = colliders();
        let zone = room_zone();
        let bytes = zone.to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        let rooms: Vec<_> = read.presentation.rooms.iter().map(|room| room.name.as_str()).collect();
        let mut expected = ["street", "gatehouse"];
        expected.sort_unstable_by_key(|name| Name::new(name).as_u64());
        assert_eq!(rooms, expected);
        let gatehouse =
            read.presentation.rooms.iter().find(|room| room.name == "gatehouse").expect("it's there");
        assert_eq!(gatehouse.position(), [3.2, 2.0, -20.0]);
        assert_eq!(gatehouse.volume.to_native(), CookedVolume::Sphere { radius: 4.0 });
        assert_eq!(
            (gatehouse.reverb.as_str(), gatehouse.priority, gatehouse.outdoor, gatehouse.fade.to_native()),
            ("shelter", 2, false, 0.4)
        );
        let digest = read.simulation_digest(|_| Some(&colliders[..])).expect("digests");
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "digest_pins's: rooms aren't the simulation's");
        assert_eq!(zone.simulation, self::zone().simulation);
    }

    #[test]
    fn cooked_rooms_the_document_would_refuse_are_refused() {
        type Tamper = fn(&mut Zone);
        let cases: [(&str, &str, Tamper); 5] = [
            ("an order", "out of order", |z| z.presentation.rooms.swap(0, 1)),
            ("an ID", "name's hash", |z| z.presentation.rooms[0].id ^= 1),
            ("a fade", "from 0 to", |z| z.presentation.rooms[0].fade = 12.0),
            ("a reverb", "empty", |z| z.presentation.rooms[0].reverb.clear()),
            ("a volume", "half extents", |z| {
                z.presentation
                    .rooms
                    .iter_mut()
                    .for_each(|room| room.volume = CookedVolume::Box { half_extents: [0.0; 3] });
            }),
        ];
        for (what, expected, tamper) in cases {
            let mut tampered = room_zone();
            tamper(&mut tampered);
            let bytes = tampered.to_cooked().expect("write");
            let message = Zone::read(&bytes).expect_err(what).to_string();
            assert!(message.contains(expected), "{what}: `{message}` must say `{expected}`");
        }
        let mut crowded = room_zone();
        let room = crowded.presentation.rooms[0].clone();
        crowded.presentation.rooms = (0..=MAX_ROOMS)
            .map(|index| {
                let name = format!("room_{index}");
                CookedRoom { id: Name::new(&name).as_u64(), name, ..room.clone() }
            })
            .collect();
        crowded.presentation.rooms.sort_unstable_by_key(|room| room.id);
        let message = Zone::read(&crowded.to_cooked().expect("write")).expect_err("257").to_string();
        assert!(message.contains("more than the 256"), "{message}");
    }

    /// Version 9 bytes, as M6 W2 cooked them, read for one release (cooker rule 2): `read` refuses them as stale,
    /// and `upgrade` makes them this version's, a zone with no rooms whose digest is the one it had.
    #[test]
    fn a_version_9_zone_upgrades_to_one_without_rooms() {
        let current = zone();
        let old = v9::write(&current);
        let message = Zone::read(&old).expect_err("stale").to_string();
        assert!(message.contains("schema version 9") && message.contains("recook"), "{message}");
        let upgraded = Zone::upgrade(&old).expect("reads").expect("version 9 bytes are rewritten");
        assert_eq!(upgraded.as_slice(), current.to_cooked().expect("write").as_slice(), "today's very bytes");

        let colliders = colliders();
        let from_rooms = Zone::upgrade(&v9::write(&room_zone())).expect("reads").expect("rewritten");
        let read = Zone::read(&from_rooms).expect("read");
        assert!(read.presentation.rooms.is_empty());
        let digest = read.simulation_digest(|_| Some(&colliders[..])).expect("digests");
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "digest_pins's");
        let water = Zone::upgrade(&v9::write(&water_zone())).expect("reads").expect("rewritten");
        assert_eq!(Zone::read(&water).expect("read").presentation.water.len(), 3, "water comes through");
    }

    /// A zone with a weather plan: two slots and two regions, over the shared document's text.
    fn weather_zone() -> Zone {
        let mut document = document();
        let choice = |state: &str, weight: u32| WeatherChoice { state: state.to_owned(), weight };
        document.weather_plan = Some(ZoneWeatherPlan {
            states: "presets.weather.ron".to_owned(),
            seed: 7,
            schedule: vec![
                WeatherSlot { from: 6.0, states: vec![choice("clear", 3), choice("rain", 1)] },
                WeatherSlot { from: 20.0, states: vec![choice("mist", 1)] },
            ],
            regions: vec![
                WeatherRegion {
                    name: "summit".to_owned(),
                    position: [0.0, 80.0, -120.0],
                    shape: ZoneShape::Ellipsoid { radii: [60.0, 30.0, 40.0], yaw: 20.0 },
                    state: "snow".to_owned(),
                    priority: 1,
                },
                WeatherRegion {
                    name: "basin".to_owned(),
                    position: [200.0, 0.0, 0.0],
                    shape: ZoneShape::Box { half_extents: [50.0, 20.0, 30.0], yaw: -35.0 },
                    state: "sandstorm".to_owned(),
                    priority: 0,
                },
            ],
        });
        let geometry = |weather| ZoneGeometry { weather, ..geometry() };
        assert!(document.cook(geometry(None)).is_err(), "a plan needs its states' text");
        document.cook(geometry(Some("(states text)".to_owned()))).expect("cooks")
    }

    /// A weather plan is presentation: it reads back as written, its regions by ascending ID, and the digest is
    /// [`digest_pins`'] for the same zone without it.
    #[test]
    fn a_weather_plan_reads_back_and_leaves_the_digest_alone() {
        let zone = weather_zone();
        let bytes = zone.to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        let plan = read.presentation.weather_plan.as_ref().expect("there");
        assert_eq!((plan.states.as_str(), plan.seed.to_native()), ("(states text)", 7));
        let slots: Vec<WeatherSlot> = plan.schedule.iter().map(ArchivedWeatherSlot::to_native).collect();
        assert_eq!(slots, zone.presentation.weather_plan.as_ref().expect("there").schedule);
        let mut expected = ["summit", "basin"];
        expected.sort_unstable_by_key(|name| Name::new(name).as_u64());
        let regions: Vec<_> = plan.regions.iter().map(|region| region.name.as_str()).collect();
        assert_eq!(regions, expected);
        let summit = plan.regions.iter().find(|region| region.name == "summit").expect("there");
        assert_eq!(summit.shape.to_native(), ZoneShape::Ellipsoid { radii: [60.0, 30.0, 40.0], yaw: 20.0 });
        assert_eq!(
            (summit.position(), summit.state.as_str(), summit.priority),
            ([0.0, 80.0, -120.0], "snow", 1)
        );
        let colliders = colliders();
        let digest = read.simulation_digest(|_| Some(&colliders[..])).expect("digests");
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "digest_pins's: weather isn't the simulation's");
    }

    #[test]
    fn a_cooked_plan_the_document_would_refuse_is_refused() {
        type Tamper = fn(&mut CookedWeatherPlan);
        let cases: [(&str, &str, Tamper); 5] = [
            ("an order", "out of order", |plan| plan.regions.swap(0, 1)),
            ("a shape", "extents", |plan| plan.regions[0].shape = ZoneShape::Sphere { radius: -1.0 }),
            ("a slot", "hours must rise", |plan| plan.schedule[1].from = 3.0),
            ("a weight", "weighs nothing", |plan| plan.schedule[0].states[0].weight = 0),
            ("no text", "empty", |plan| plan.states.clear()),
        ];
        for (what, expected, tamper) in cases {
            let mut tampered = weather_zone();
            tamper(tampered.presentation.weather_plan.as_mut().expect("there"));
            let bytes = tampered.to_cooked().expect("write");
            let message = Zone::read(&bytes).expect_err(what).to_string();
            assert!(message.contains(expected), "{what}: `{message}` must say `{expected}`");
        }
    }

    /// Version 10 bytes, as M6 U5 cooked them, read for one release (cooker rule 2): `read` refuses them as
    /// stale, and `upgrade` makes them this version's, a zone with no weather plan whose digest is the one it
    /// had.
    #[test]
    fn a_version_10_zone_upgrades_to_one_without_a_weather_plan() {
        let current = zone();
        let old = v10::write(&current);
        let message = Zone::read(&old).expect_err("stale").to_string();
        assert!(message.contains("schema version 10") && message.contains("recook"), "{message}");
        let upgraded = Zone::upgrade(&old).expect("reads").expect("version 10 bytes are rewritten");
        assert_eq!(upgraded.as_slice(), current.to_cooked().expect("write").as_slice(), "today's very bytes");
        let from_plan = Zone::upgrade(&v10::write(&weather_zone())).expect("reads").expect("rewritten");
        let read = Zone::read(&from_plan).expect("read");
        assert!(read.presentation.weather_plan.is_none());
        let colliders = colliders();
        let digest = read.simulation_digest(|_| Some(&colliders[..])).expect("digests");
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "digest_pins's");
        let rooms = Zone::upgrade(&v10::write(&room_zone())).expect("reads").expect("rewritten");
        assert_eq!(Zone::read(&rooms).expect("read").presentation.rooms.len(), 2, "rooms come through");
    }

    const FOLIAGE: AssetId = AssetId::from_text("6c1d2e3f-4a5b-4c6d-8e7f-90a1b2c3d4e5");

    /// [`zone`] with foliage: two cells' scatter sets.
    fn foliage_zone() -> Zone {
        let mut document = document();
        document.foliage = Some(FOLIAGE);
        let cells = [[0, -1], [1, -1]]
            .map(|coord| CookedScatterCell { coord, scatter: FOLIAGE.sub_asset(&format!("{coord:?}")) });
        let foliage = CookedZoneFoliage { foliage: FOLIAGE, cells: cells.to_vec() };
        let geometry = |foliage| ZoneGeometry { foliage, ..geometry() };
        assert!(document.cook(geometry(None)).is_err(), "foliage needs its sets");
        document.cook(geometry(Some(foliage))).expect("cooks")
    }

    /// Foliage is presentation: its kinds and sets are named, never held, and the digest is [`digest_pins`'].
    #[test]
    fn foliage_is_named_and_leaves_the_digest_alone() {
        let zone = foliage_zone();
        let bytes = zone.to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        let named = read.presentation_references();
        let foliage = zone.presentation.foliage.as_ref().expect("there");
        assert!(named.contains(&FOLIAGE) && foliage.cells.iter().all(|cell| named.contains(&cell.scatter)));
        assert!(!read.references().contains(&FOLIAGE), "loading the zone doesn't hold it");
        let colliders = colliders();
        let digest = read.simulation_digest(|_| Some(&colliders[..])).expect("digests");
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "digest_pins's: foliage isn't the simulation's");

        let mut unordered = zone.clone();
        unordered.presentation.foliage.as_mut().expect("there").cells.swap(0, 1);
        let message = Zone::read(&unordered.to_cooked().expect("write")).expect_err("refused").to_string();
        assert!(message.contains("ascending cell"), "{message}");
    }

    /// A zone without colliders has nothing for its plants to stand on.
    #[test]
    fn foliage_needs_colliders() {
        let text = format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), foliage: Some("{FOLIAGE}")))"#
        );
        let message = ZoneDocument::from_ron(&text).expect_err("refused").to_string();
        assert!(message.contains("`foliage`") && message.contains("colliders"), "{message}");
    }

    /// Version 11 bytes, as M6 W8a cooked them, read for one release (cooker rule 2): `upgrade` makes them this
    /// version's, a zone with no foliage whose digest and bytes are those it had at this version.
    #[test]
    fn a_version_11_zone_upgrades_to_one_without_foliage() {
        let current = weather_zone();
        let old = v11::write(&current);
        let message = Zone::read(&old).expect_err("stale").to_string();
        assert!(message.contains("schema version 11") && message.contains("recook"), "{message}");
        let upgraded = Zone::upgrade(&old).expect("reads").expect("version 11 bytes are rewritten");
        assert_eq!(upgraded.as_slice(), current.to_cooked().expect("write").as_slice(), "today's very bytes");
        let dropped = Zone::upgrade(&v11::write(&foliage_zone())).expect("reads").expect("rewritten");
        assert!(Zone::read(&dropped).expect("read").presentation.foliage.is_none());
    }

    /// [`zone`] with three atmosphere volumes (M6 V1), two of them naming one preset.
    fn atmosphere_zone() -> Zone {
        let mut document = document();
        let volume = |name: &str, shape, look: &str, priority| AtmosphereVolume {
            name: name.to_owned(),
            position: [4.0, 2.0, -12.0],
            shape,
            look: look.to_owned(),
            blend: 3.0,
            strength: 0.8,
            priority,
        };
        document.volumes = vec![
            volume("grotto", ZoneShape::Ellipsoid { radii: [8.0, 4.0, 6.0], yaw: 25.0 }, "cave_day.ron", 0),
            volume("inn", ZoneShape::Box { half_extents: [4.0, 2.0, 3.0], yaw: -10.0 }, "inn_day.ron", 2),
            volume("cellar", ZoneShape::Sphere { radius: 3.0 }, "cave_day.ron", 1),
        ];
        assert_eq!(document.volume_looks(), ["cave_day.ron", "inn_day.ron"]);
        let geometry = |volume_looks| ZoneGeometry { volume_looks, ..geometry() };
        assert!(document.cook(geometry(Vec::new())).is_err(), "volumes need their looks' text");
        document.cook(geometry(vec!["(cave text)".to_owned(), "(inn text)".to_owned()])).expect("cooks")
    }

    /// Atmosphere volumes are presentation: they read back as written, by ascending ID with their looks once each,
    /// name no asset, and the digest is [`digest_pins`'] for the same zone without them.
    #[test]
    fn atmosphere_volumes_read_back_and_leave_the_digest_alone() {
        let zone = atmosphere_zone();
        let bytes = zone.to_cooked().expect("write");
        let read = Zone::read(&bytes).expect("read");
        let atmosphere = read.presentation.atmosphere.to_native();
        assert_eq!(atmosphere, zone.presentation.atmosphere);
        assert_eq!(atmosphere.looks, ["(cave text)", "(inn text)"]);
        let mut expected = ["grotto", "inn", "cellar"];
        expected.sort_unstable_by_key(|name| Name::new(name).as_u64());
        let names: Vec<&str> = atmosphere.volumes.iter().map(|volume| volume.name.as_str()).collect();
        assert_eq!(names, expected);
        let inn = atmosphere.volumes.iter().find(|volume| volume.name == "inn").expect("there");
        assert_eq!(inn.shape, ZoneShape::Box { half_extents: [4.0, 2.0, 3.0], yaw: -10.0 });
        assert_eq!((inn.look, inn.priority, inn.blend, inn.strength), (1, 2, 3.0, 0.8));
        let orders: Vec<(&str, u32)> =
            atmosphere.volumes.iter().map(|volume| (volume.name.as_str(), volume.order)).collect();
        for (name, order) in [("grotto", 0), ("inn", 1), ("cellar", 2)] {
            assert!(orders.contains(&(name, order)), "each keeps its place in the document: {orders:?}");
        }
        let colliders = colliders();
        let digest = read.simulation_digest(|_| Some(&colliders[..])).expect("digests");
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "digest_pins's: atmosphere isn't the simulation's");
        let plain = zone_bytes();
        let plain = Zone::read(&plain).expect("read");
        assert_eq!(read.presentation_references(), plain.presentation_references(), "volumes name no asset");
        assert_eq!(read.references(), plain.references());
    }

    fn zone_bytes() -> AlignedVec {
        zone().to_cooked().expect("write")
    }

    #[test]
    fn cooked_volumes_the_document_would_refuse_are_refused() {
        type Tamper = fn(&mut CookedAtmosphere);
        let cases: [(&str, &str, Tamper); 9] = [
            ("an order", "out of order", |atmosphere| atmosphere.volumes.swap(0, 1)),
            ("a shared place", "isn't a place of its own", |atmosphere| {
                atmosphere.volumes[1].order = atmosphere.volumes[0].order;
            }),
            ("a place past the list", "isn't a place of its own", |atmosphere| {
                atmosphere.volumes[0].order = 9
            }),
            ("a shape", "extents", |atmosphere| {
                atmosphere.volumes[0].shape = ZoneShape::Sphere { radius: 0.0 }
            }),
            ("a blend", "isn't from 0", |atmosphere| atmosphere.volumes[0].blend = -1.0),
            ("a strength", "isn't from 0 to 1", |atmosphere| atmosphere.volumes[0].strength = 2.0),
            ("a look", "isn't one of the zone's", |atmosphere| atmosphere.volumes[0].look = 2),
            ("no text", "empty", |atmosphere| atmosphere.looks[1].clear()),
            ("an unnamed look", "no volume names it", |atmosphere| {
                atmosphere.looks.push("(more)".to_owned())
            }),
        ];
        for (what, expected, tamper) in cases {
            let mut tampered = atmosphere_zone();
            tamper(&mut tampered.presentation.atmosphere);
            let bytes = tampered.to_cooked().expect("write");
            let message = Zone::read(&bytes).expect_err(what).to_string();
            assert!(message.contains(expected), "{what}: `{message}` must say `{expected}`");
        }
    }

    /// Version 12 bytes, as M6 G1a cooked them, read for one release (cooker rule 2): `upgrade` makes them this
    /// version's, a zone with no atmosphere volumes whose bytes are those it has at this version, its foliage kept.
    #[test]
    fn a_version_12_zone_upgrades_to_one_without_atmosphere_volumes() {
        let current = foliage_zone();
        let old = v12::write(&current);
        let message = Zone::read(&old).expect_err("stale").to_string();
        assert!(message.contains("schema version 12") && message.contains("recook"), "{message}");
        let upgraded = Zone::upgrade(&old).expect("reads").expect("version 12 bytes are rewritten");
        assert_eq!(upgraded.as_slice(), current.to_cooked().expect("write").as_slice(), "today's very bytes");
        let dropped = Zone::upgrade(&v12::write(&atmosphere_zone())).expect("reads").expect("rewritten");
        let read = Zone::read(&dropped).expect("read");
        assert!(
            read.presentation.atmosphere.volumes.is_empty() && read.presentation.atmosphere.looks.is_empty()
        );
        let colliders = colliders();
        let digest = read.simulation_digest(|_| Some(&colliders[..])).expect("digests");
        assert_eq!(digest, 0x15dc_b5da_8e03_1351, "digest_pins's");
    }
}
