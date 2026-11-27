//! The benchmark's nature zone (M6 W1): a generated landscape of terrain, water, trees and grass, with a dusk
//! day, a weather plan that rains, a probe grid and reflection probes, which cooks as a zone the sandbox opens.
//! The unit every world-look, foliage, water, weather and environment package is priced and judged on, as the
//! district is for the city (m6-plan, W1).
//!
//! `bench/nature/` holds:
//!
//! | File | What |
//! |---|---|
//! | `nature.ron` | [`BenchNature`]: the hills, the named areas, the trees, the grass, the walks, the look, the probes and each preset's scale |
//! | `nature_day.ron` | The zone's day: the district's midday, dusk and night keys |
//! | `nature.weather.ron` | The weather states its plan and the units' arms name: the shipped presets |
//! | `surfaces/<name>.ptex.ron` | The grounds' procedural texture specs ([`crate::procedural`]) |
//!
//! # The layout
//!
//! The zone is `sapphire_synthetic_zone::nature`'s extent, 2,048 m east to west and 1,280 m north to south, in
//! cells of 64 m (+X east, +Z south). Its simulation half is that module's, written here as its code builds it:
//! the water (a lake, a river running into it and a frozen pond), the clearing's fight of six, its colliders.
//! Everything else is presentation:
//!
//! - **Terrain** as a mesh a cell (`terrain/tile_<x>_<z>.glb`, an instance each), until W12's height fields:
//!   rolling hills of value noise on integer hashes, shaped by each named area, cut under every water body and
//!   flat at 0 over the clearing, which the colliders stand for. Each tile draws the ground of the area its
//!   middle stands in.
//! - **The named areas** (`areas`): the forest block (1 km², grown with today's trees until G2b fills it), the
//!   coast, the cave, the valley and the frozen pond, each plain ground shaped as its row asks with a stone
//!   marker at its middle: the placeholders later packages fill, each in its own pull request.
//! - **Trees** (`trees`): the trees part's species at their ages (`bench/trees/`, by ID), instances on a jittered
//!   grid, dense in the forest block and sparse over the meadow, clear of water, the clearing and the walks.
//! - **Grass** (`grass`): one generated tuft of blades with W6's foliage stream (blades lit as the ground, by
//!   `NormalTransfer::Up`), instanced on a jittered grid near the walks and about the clearing. G1a's scatter cook
//!   takes this over.
//! - **The look:** `nature_day.ron` and a weather plan over `nature.weather.ron` (clear by day, rain from 19:00),
//!   wind and fog; a probe grid over the walks' ground ([`NatureGrid`]); reflection probes over the water, the
//!   clearing and the meadow, each body's named as the body is, which its surface reflects.
//! - **Walks** (`walks`): rails a zone walk follows (`--zone-walk <name>`), each point at the terrain's height:
//!   `walk` along the river and the lake to the clearing, `forest` into the forest block, `overlook` standing
//!   over it, `coast` along the shore and `coast_dive` down the beach.
//!
//! # Presets
//!
//! A preset changes only the grounds' texture size and how dense the trees and grass stand ([`NatureScale`]):
//! the layout, terrain, water, walks and the simulation half are every preset's, so one replay plays on any. The
//! phones' tree is the desktop's.
//!
//! # Determinism
//!
//! Integer hashes, the four operations, `floor` and `sqrt` only: no transcendental is reached, and every turn
//! is a whole multiple of 15 degrees.

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroU32;
use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    DistanceStylization, EnvironmentMaterial, FoliageGradient, FoliageMaterial, MaterialDocument, Terminator,
    Translucency, Wind,
};
use sapphire_reflect::Versioned;
use sapphire_synthetic_zone::nature as simulation;
use sapphire_zone::document::{Instance, Placement, Spline, ZoneDocument};
use sapphire_zone::grid::Grid;
use sapphire_zone::water::{WaterBody, WaterShape};
use sapphire_zone::weather::ZoneWeatherPlan;
use sapphire_zone::{ReflectionProbe, ZoneFog, ZoneWind};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{BenchError, Preset, Presets, Source, asset_id, document_bytes, read_document};
use crate::bake::foliage::{FoliageSettings, NormalTransfer};
use crate::colliders::ColliderSettings;
use crate::cook::Cook;
use crate::gltf_writer::{Geometry, GltfWriter};
use crate::material::MaterialSettings;
use crate::meta::{ImportSettings, MeshSettings};
use crate::optimize::LodSettings;
use crate::probes::{
    BakeBounds, BakeMesh, BakeTimeline, PerCook, ProbeGridDocument, ProbeGridSettings, ShOrder,
};
use crate::procedural::{self, ProceduralSettings, ProceduralTexture, SurfaceMap};
use crate::samples::noise;
use crate::texture::MaxSize;
use crate::zone::ZoneSettings;

/// This part's folder, under `bench/` and in the output.
pub const PART: &str = "nature";
/// The zone's source, in the part's output: `--zone nature` opens it.
pub const ZONE: &str = "nature.zone.ron";
/// The clearing's fight, in the part's output.
pub const ENCOUNTER: &str = "clearing.encounter.ron";
/// The day the zone names, in the part's output.
pub const DAY: &str = "nature_day.ron";
/// The weather states the zone's plan names, in the part's output.
pub const WEATHER: &str = "nature.weather.ron";
/// The zone's colliders, in the part's output: what the clearing's fight stands on.
pub const COLLIDERS: &str = "nature.colliders.ron";
/// The zone's ambient probe grid, in the part's output.
pub const PROBES: &str = "nature.probes.ron";
/// The rail the nature walk follows (`--zone-walk walk`), along the river and the lake into the clearing.
pub const WALK: &str = "walk";
/// The terrain tiles' one slot.
pub const GROUND_SLOT: &str = "ground";
/// The grass tuft's one slot.
pub const BLADES_SLOT: &str = "blades";
/// The grass tuft's model, in the part's output.
pub const GRASS: &str = "grass/tuft.glb";

/// The most trees or grass tufts a preset may stand: well under a zone's entries, so a typo can't make millions.
const MAX_SCATTERED: usize = 100_000;
/// How far apart a walk's points are laid, in metres, each at the terrain's height.
const WALK_STEP: f32 = 8.0;
/// How far from a water body's edge nothing stands and no walk goes, in metres.
const WATER_CLEAR: f32 = 4.0;

/// The nature zone's document, `bench/nature/nature.ron` (`sapphire::pipeline::BenchNature`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct BenchNature {
    /// A cell's side, in metres: the simulation half's (`sapphire_synthetic_zone::nature::CELL_SIZE`).
    pub cell_size: f32,
    pub hills: Hills,
    /// The ground everywhere no area's is: a surface's stem in `surfaces/`.
    pub ground: String,
    pub areas: Vec<Area>,
    /// The tree models the zone stands, by the trees part's species and ages (`bench/trees/`).
    pub trees: Vec<TreeKind>,
    pub grass: Grass,
    /// Half the width kept clear of trees and grass along every walk, in metres.
    pub corridor: f32,
    pub walks: Vec<WalkRail>,
    /// The day the zone names: a timeline document in `bench/nature/`.
    pub day: String,
    /// The zone's weather plan; its `states` is a weather document in `bench/nature/`.
    pub weather_plan: ZoneWeatherPlan,
    #[serde(default)]
    pub wind: Option<ZoneWind>,
    #[serde(default)]
    pub fog: Option<ZoneFog>,
    pub probe_grid: NatureGrid,
    /// The zone's reflection probes; every water body's (`lake`, `river`, `frozen_pond`) among them.
    pub reflections: Vec<ReflectionProbe>,
    pub presets: Presets<NatureScale>,
}

impl Versioned for BenchNature {
    /// 1: M6 W1.
    const VERSION: u32 = 1;
}

/// The hills every tile's terrain rolls with: fractal value noise.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hills {
    /// From a hill's top to its foot, at most, in metres.
    pub amplitude: f32,
    /// The coarsest octave's cell, in metres.
    pub wavelength: f32,
    pub octaves: u32,
    pub seed: u32,
}

/// A named area of the zone: a rectangle a later package fills, plain ground until then.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Area {
    pub name: String,
    /// What fills it, and when: said in the report and the docs.
    pub about: String,
    /// Least `(x, z)` and greatest, in metres.
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub shape: AreaShape,
    /// Its ground: a surface's stem in `surfaces/`.
    pub ground: String,
    /// Whether the forest's trees stand in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forest: bool,
}

/// How an area shapes the hills, strongest at its middle and gone at its edge.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AreaShape {
    /// The hills as they are.
    Plain,
    /// Sunk this many metres at its middle.
    Bowl(f32),
    /// Raised this many metres at its middle.
    Mound(f32),
    /// Falling this many metres from its west edge to its east, the hills fading out toward the east.
    Slope(f32),
}

/// A tree model the zone stands: a species of the trees part, at each of `ages`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeKind {
    pub species: String,
    pub ages: Vec<String>,
}

/// The grass tuft: blades in a disc, and where tufts stand.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grass {
    pub blades: u32,
    /// The disc's radius, in metres.
    pub radius: f32,
    /// A blade's least and greatest height, and its width at the root, in metres.
    pub height: [f32; 2],
    pub width: f32,
    /// Linear RGB at a blade's root and tip.
    pub root: [f32; 3],
    pub tip: [f32; 3],
    pub wind: Wind,
    /// How far from a walk, or from the clearing's middle, tufts stand, in metres.
    pub reach: f32,
}

/// A rail through the zone, its points `(x, z)`: the terrain gives each its height.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkRail {
    pub name: String,
    pub points: Vec<[f32; 2]>,
    /// Whether trees and grass keep clear of it: a rail over open ground, which an overlook isn't.
    #[serde(default = "yes")]
    pub corridor: bool,
    /// How far over the terrain the rail runs, in metres: 0 on the ground, an overlook's eye above the canopy.
    #[serde(default)]
    pub lift: f32,
}

fn yes() -> bool {
    true
}

/// The zone's ambient probe grid: a [`ProbeGridDocument`]'s settings over a box of the zone.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NatureGrid {
    /// Least `(x, z)` and greatest, in metres.
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub floor: f32,
    pub top: f32,
    pub visibility: PerCook<ShOrder>,
    pub rays: PerCook<u32>,
    pub surfel_rays: u32,
    pub surfel_spacing: f32,
    pub bounces: u32,
    pub bounce_strength: f32,
}

/// What a preset makes of the zone.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NatureScale {
    /// Texels on a side of every ground map.
    pub texture_size: u32,
    /// Metres between the forest's trees, between the meadow's, and between grass tufts.
    pub forest_spacing: f32,
    pub meadow_spacing: f32,
    pub grass_spacing: f32,
    /// Terrain quads along a cell's side.
    pub tile_quads: u32,
    /// The probe grid's bricks' spacing, in metres, on each cook: `ci` proves the route with a coarse grid and
    /// small tiles, the budget measures at the detail a game ships.
    pub probe_spacing: PerCook<f32>,
}

/// Generates the nature zone from the documents in `dir` (`bench/nature/`) at `preset`'s scale, the same for
/// every cook.
pub fn generate(dir: &Path, preset: Preset, _cook: Cook) -> Result<Vec<Source>, BenchError> {
    let document_path = dir.join("nature.ron");
    let nature: BenchNature = read_document(&document_path)?;
    let scale = *nature.presets.get(preset);
    let invalid = |why: String| BenchError::Invalid { path: document_path.clone(), why };
    check(&nature, scale).map_err(|why| invalid(format!("{preset}: {why}")))?;
    let land = Land::new(&nature);

    let mut sources = Vec::new();
    let grounds = grounds(&dir.join("surfaces"), scale.texture_size, &mut sources)?;
    let ground = |stem: &str| {
        grounds.get(stem).copied().ok_or_else(|| invalid(format!("no surface `surfaces/{stem}.ptex.ron`")))
    };

    // The terrain, a tile a cell, each drawing its area's ground.
    let mut instances = Vec::new();
    let mut terrain = Vec::new();
    let [half_x, half_z] = simulation::HALF_EXTENT;
    let cell = nature.cell_size;
    let (columns, rows) = ((2.0 * half_x / cell) as i32, (2.0 * half_z / cell) as i32);
    for row in 0..rows {
        for column in 0..columns {
            let middle = [-half_x + (column as f32 + 0.5) * cell, -half_z + (row as f32 + 0.5) * cell];
            let stem = nature
                .areas
                .iter()
                .find(|area| area.contains(middle))
                .map_or(&nature.ground, |area| &area.ground);
            let path = format!("terrain/tile_{column:02}_{row:02}.glb");
            let settings = MeshSettings {
                lods: LodSettings::none(),
                materials: BTreeMap::from([(GROUND_SLOT.to_owned(), ground(stem)?)]),
                ..MeshSettings::default()
            };
            sources.push(Source::asset(
                &path,
                land.tile(middle, cell, scale.tile_quads),
                ImportSettings::Gltf(settings),
            ));
            let instance = Instance {
                mesh: asset_id(PART, &path),
                at: [middle[0], 0.0, middle[1]],
                turn: 0.0,
                scale: [1.0; 3],
                backdrop: false,
                colliders: Vec::new(),
            };
            terrain.push((middle, instance.mesh));
            instances.push(instance);
        }
    }

    // The walks, each point at the terrain's height.
    let walks: Vec<Spline> = nature
        .walks
        .iter()
        .map(|walk| Spline {
            name: walk.name.clone(),
            points: land.rail(&walk.points, walk.lift),
            closed: false,
        })
        .collect();
    let corridors: Vec<&WalkRail> = nature.walks.iter().filter(|walk| walk.corridor).collect();
    let near_walk =
        |at: [f32; 2], reach: f32| corridors.iter().any(|walk| from_polyline(at, &walk.points) < reach);
    // Where nothing scattered stands: water, the clearing's flat ground, a walk's corridor, the zone's edge.
    let open = |at: [f32; 2]| {
        land.water_reach(at) > WATER_CLEAR
            && !near_walk(at, nature.corridor)
            && (at[0] - simulation::CLEARING[0]).abs().max((at[1] - simulation::CLEARING[1]).abs())
                > simulation::FLAT_HALF + 4.0
            && at[0].abs() < half_x - 4.0
            && at[1].abs() < half_z - 4.0
    };

    // Trees: dense in the forest's areas, sparse in no area at all.
    let models: Vec<AssetId> = nature
        .trees
        .iter()
        .flat_map(|kind| {
            kind.ages
                .iter()
                .map(move |age| asset_id(super::trees::PART, &format!("{0}/{0}_{age}.glb", kind.species)))
        })
        .collect();
    let mut trees = Vec::new();
    for (index, area) in nature.areas.iter().enumerate().filter(|(_, area)| area.forest) {
        trees.extend(scatter(area.min, area.max, scale.forest_spacing, 0x7472_0000 ^ index as u32, &open));
    }
    trees.extend(
        scatter([-half_x, -half_z], [half_x, half_z], scale.meadow_spacing, 0x6d65_6164, &open)
            .into_iter()
            .filter(|&(at, _)| !nature.areas.iter().any(|area| area.contains(at))),
    );
    let grass_reach = nature.grass.reach;
    let grass: Vec<([f32; 2], u32)> =
        scatter([-half_x, -half_z], [half_x, half_z], scale.grass_spacing, 0x6772_6173, &|at| {
            open(at)
                && !nature.areas.iter().any(|area| area.contains(at))
                && (near_walk(at, grass_reach + nature.corridor)
                    || distance(at, simulation::CLEARING) < grass_reach + simulation::FLAT_HALF)
        });
    if trees.len() > MAX_SCATTERED || grass.len() > MAX_SCATTERED {
        return Err(invalid(format!(
            "{preset}: {} trees and {} tufts: at most {MAX_SCATTERED} each",
            trees.len(),
            grass.len()
        )));
    }
    let stand = |mesh: AssetId, (at, roll): ([f32; 2], u32)| Instance {
        mesh,
        at: [at[0], land.height(at), at[1]],
        turn: (roll % 24) as f32 * 15.0,
        scale: [1.0; 3],
        backdrop: false,
        colliders: Vec::new(),
    };
    let tree_instances: Vec<Instance> = trees
        .iter()
        .map(|&(at, roll)| stand(models[noise::hash(roll) as usize % models.len()], (at, roll)))
        .collect();
    instances.extend(tree_instances.iter().cloned());
    let grass_id = asset_id(PART, GRASS);
    instances.extend(grass.iter().map(|&spot| stand(grass_id, spot)));
    sources.push(grass_sources(&nature.grass)?);
    sources.push(Source::asset(
        "grass/grass.material.ron",
        document_bytes(&grass_material(&nature.grass)),
        ImportSettings::Material(MaterialSettings::default()),
    ));

    // The areas' markers: a stone block at each one's middle, merged into its cell.
    sources.push(Source::uncooked(
        MARKER_PREFAB,
        marker_prefab(),
        ImportSettings::Assembly(MeshSettings {
            materials: BTreeMap::from([("stone".to_owned(), ground("rock")?)]),
            ..MeshSettings::default()
        }),
    ));
    let placements: Vec<Placement> = nature
        .areas
        .iter()
        .map(|area| {
            let middle = area.middle();
            Placement {
                prefab: asset_id(PART, MARKER_PREFAB),
                at: [middle[0], land.height(middle), middle[1]],
                turn: 0.0,
                scale: [1.0; 3],
                materials: BTreeMap::new(),
                backdrop: false,
                split: None,
                colliders: Vec::new(),
            }
        })
        .collect();

    // The simulation half, as `sapphire_synthetic_zone::nature` builds it, and the look.
    let encounter = asset_id(PART, ENCOUNTER);
    sources.push(Source::asset(
        ENCOUNTER,
        document_bytes(&simulation::encounter()),
        ImportSettings::Encounter(Default::default()),
    ));
    sources.push(Source::asset(
        COLLIDERS,
        document_bytes(&simulation::colliders()),
        ImportSettings::Colliders(ColliderSettings::default()),
    ));
    sources.push(Source::input(DAY, fs::read(dir.join(&nature.day))?));
    sources.push(Source::input(WEATHER, fs::read(dir.join(&nature.weather_plan.states))?));
    let zone = ZoneDocument {
        grid: Grid { cell_size: nature.cell_size },
        placements,
        instances,
        lights: Vec::new(),
        probes: Some(asset_id(PART, PROBES)),
        reflections: nature.reflections.clone(),
        day: Some(DAY.to_owned()),
        rigs: None,
        sky: None,
        wind: nature.wind,
        weather: None,
        fog: nature.fog,
        colliders: Some(asset_id(PART, COLLIDERS)),
        spawners: simulation::spawners(),
        triggers: simulation::triggers(),
        checkpoints: simulation::checkpoints(),
        splines: walks,
        navigation: None,
        encounters: vec![encounter],
        exits: Vec::new(),
        ambient: None,
        water: simulation::water()
            .into_iter()
            .map(|body| WaterBody { probe: Some(body.name.clone()), ..body })
            .collect(),
        rooms: Vec::new(),
        weather_plan: Some(ZoneWeatherPlan { states: WEATHER.to_owned(), ..nature.weather_plan.clone() }),
        foliage: None,
        volumes: Vec::new(),
    };
    zone.check().map_err(|error| invalid(format!("{preset}: the zone it makes: {error}")))?;
    sources.push(Source::asset(
        PROBES,
        document_bytes(&probe_grid(&nature.probe_grid, scale.probe_spacing, &terrain, &tree_instances)),
        ImportSettings::ProbeGrid(ProbeGridSettings::default()),
    ));
    sources.push(Source::asset(ZONE, document_bytes(&zone), ImportSettings::Zone(ZoneSettings::default())));
    Ok(sources)
}

/// What a document can be wrong about that serde can't see.
fn check(nature: &BenchNature, scale: NatureScale) -> Result<(), String> {
    if nature.cell_size != simulation::CELL_SIZE {
        return Err(format!("`cell_size` is the simulation half's, {} m", simulation::CELL_SIZE));
    }
    let Hills { amplitude, wavelength, octaves, .. } = nature.hills;
    if !(0.0..=64.0).contains(&amplitude)
        || !(8.0..=1024.0).contains(&wavelength)
        || !(1..=8).contains(&octaves)
    {
        return Err("`hills` rise 0 to 64 m over 8 to 1,024 m in 1 to 8 octaves".to_owned());
    }
    let [half_x, half_z] = simulation::HALF_EXTENT;
    for area in &nature.areas {
        let inside = area.min[0] >= -half_x
            && area.min[1] >= -half_z
            && area.max[0] <= half_x
            && area.max[1] <= half_z;
        if !inside || area.min[0] >= area.max[0] || area.min[1] >= area.max[1] {
            return Err(format!("area `{}` isn't a rectangle inside the zone", area.name));
        }
        let fight = (
            simulation::CLEARING[0] - simulation::FLAT_HALF,
            simulation::CLEARING[1] - simulation::FLAT_HALF,
        );
        let far = (
            simulation::CLEARING[0] + simulation::FLAT_HALF,
            simulation::CLEARING[1] + simulation::FLAT_HALF,
        );
        if area.min[0] < far.0 && area.max[0] > fight.0 && area.min[1] < far.1 && area.max[1] > fight.1 {
            return Err(format!(
                "area `{}` reaches the clearing, whose ground the fight's colliders hold flat",
                area.name
            ));
        }
    }
    if nature.trees.is_empty() || nature.trees.iter().any(|kind| kind.ages.is_empty()) {
        return Err("`trees` names at least one species, each at an age".to_owned());
    }
    let Grass { blades, radius, height, width, reach, .. } = nature.grass;
    if !(1..=512).contains(&blades)
        || !(0.1..=4.0).contains(&radius)
        || !(0.05 <= height[0] && height[0] <= height[1] && height[1] <= 2.0)
        || !(0.005..=0.5).contains(&width)
        || !(0.0..=256.0).contains(&reach)
    {
        return Err("`grass` is 1 to 512 blades in a disc of 0.1 to 4 m, at most 2 m tall".to_owned());
    }
    if !nature.walks.iter().any(|walk| walk.name == WALK) {
        return Err(format!("`walks` has no `{WALK}`, the nature walk's"));
    }
    for walk in &nature.walks {
        if walk.points.len() < 2 {
            return Err(format!("walk `{}` has fewer than two points", walk.name));
        }
        for &point in &walk.points {
            if point[0].abs() >= half_x || point[1].abs() >= half_z {
                return Err(format!("walk `{}` leaves the zone at {point:?}", walk.name));
            }
        }
    }
    let NatureScale {
        texture_size,
        forest_spacing,
        meadow_spacing,
        grass_spacing,
        tile_quads,
        probe_spacing,
    } = scale;
    if !(1..=64).contains(&tile_quads) {
        return Err("`tile_quads` is 1 to 64".to_owned());
    }
    if [probe_spacing.desktop, probe_spacing.mobile].iter().any(|spacing| !(1.0..=32.0).contains(spacing)) {
        return Err("`probe_spacing` is 1 to 32 m".to_owned());
    }
    if !texture_size.is_power_of_two() || !(64..=4096).contains(&texture_size) {
        return Err("`texture_size` is a power of two from 64 to 4,096".to_owned());
    }
    if [forest_spacing, meadow_spacing, grass_spacing].iter().any(|spacing| !(1.0..=512.0).contains(spacing))
    {
        return Err("every spacing is 1 to 512 m".to_owned());
    }
    for body in simulation::water() {
        if !nature.reflections.iter().any(|probe| probe.name == body.name) {
            return Err(format!("`reflections` has no `{}`, which the water body reflects", body.name));
        }
    }
    Ok(())
}

impl Area {
    fn contains(&self, at: [f32; 2]) -> bool {
        at[0] >= self.min[0] && at[0] < self.max[0] && at[1] >= self.min[1] && at[1] < self.max[1]
    }

    fn middle(&self) -> [f32; 2] {
        [(self.min[0] + self.max[0]) * 0.5, (self.min[1] + self.max[1]) * 0.5]
    }

    /// How strongly its shape reaches `at`: 1 over its middle half, easing to 0 at its edge.
    fn weight(&self, at: [f32; 2]) -> f32 {
        let middle = self.middle();
        let u = (at[0] - middle[0]).abs() / ((self.max[0] - self.min[0]) * 0.5);
        let v = (at[1] - middle[1]).abs() / ((self.max[1] - self.min[1]) * 0.5);
        1.0 - smoothstep(0.5, 1.0, u.max(v))
    }
}

/// The land: its height anywhere, from the hills, the areas, the water and the clearing.
struct Land<'a> {
    nature: &'a BenchNature,
    water: Vec<WaterBody>,
}

impl<'a> Land<'a> {
    fn new(nature: &'a BenchNature) -> Self {
        Self { nature, water: simulation::water() }
    }

    /// How far `at` is outside the nearest water body's edge, in metres: negative inside one. A lake's edge is
    /// its outline, the polygon its volume is; a river's, its banks.
    fn water_reach(&self, at: [f32; 2]) -> f32 {
        let mut reach = f32::MAX;
        for body in &self.water {
            let outside = match &body.shape {
                WaterShape::Lake { outline, .. } => from_outline(at, outline),
                WaterShape::River { points } => points
                    .windows(2)
                    .map(|leg| {
                        let (a, b) = (leg[0], leg[1]);
                        let (along, across) = segment(at, [a.at[0], a.at[2]], [b.at[0], b.at[2]]);
                        across - (a.width + (b.width - a.width) * along) * 0.5
                    })
                    .fold(f32::MAX, f32::min),
                WaterShape::Ocean { .. } => f32::MAX,
            };
            reach = reach.min(outside);
        }
        reach
    }

    /// The terrain's height at `at`, in metres.
    fn height(&self, at: [f32; 2]) -> f32 {
        let nature = self.nature;
        let Hills { amplitude, wavelength, octaves, seed } = nature.hills;
        // Fractal value noise: each octave half the last's cell and weight.
        let (mut hills, mut size, mut weight, mut total) = (0.0, wavelength, 1.0, 0.0);
        for octave in 0..octaves {
            hills += weight * value_noise(at[0] / size, at[1] / size, seed ^ noise::hash(octave));
            total += weight;
            size *= 0.5;
            weight *= 0.5;
        }
        let mut height = amplitude * 0.5 * hills / total;
        for area in &nature.areas {
            let weight = area.weight(at);
            height += match area.shape {
                AreaShape::Plain => 0.0,
                AreaShape::Bowl(depth) => -depth * weight,
                AreaShape::Mound(rise) => rise * weight,
                AreaShape::Slope(fall) => {
                    let east = ((at[0] - area.min[0]) / (area.max[0] - area.min[0])).clamp(0.0, 1.0);
                    let inside = if area.contains(at) { 1.0 } else { 0.0 };
                    inside * (-fall * east - height * east)
                }
            };
        }
        // Under every body, a bed sloping down from its edge; beside it, a bank rising to the land: the ground
        // meets the surface at the body's edge, so no dry trench lies under a surface drawn to it.
        let reach = self.water_reach(at);
        let bed = if reach < 0.0 {
            simulation::WATER_HEIGHT + (0.25 * reach).max(-2.5)
        } else {
            simulation::WATER_HEIGHT + 0.1 * reach
        };
        let wet = 1.0 - smoothstep(0.0, 30.0, reach);
        height = height + (bed - height) * wet;
        // The clearing, flat at 0 over the colliders' ground and easing into the hills past it.
        let [x, z] = simulation::CLEARING;
        let out = (at[0] - x).abs().max((at[1] - z).abs()) - simulation::FLAT_HALF;
        height * smoothstep(0.0, 24.0, out)
    }

    /// The terrain's normal at `at`, from the heights a step either side.
    fn normal(&self, at: [f32; 2], step: f32) -> [f32; 3] {
        let dx = self.height([at[0] + step, at[1]]) - self.height([at[0] - step, at[1]]);
        let dz = self.height([at[0], at[1] + step]) - self.height([at[0], at[1] - step]);
        let n = [-dx, 2.0 * step, -dz];
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        n.map(|v| v / length)
    }

    /// The tile about `middle`, `size` metres on a side in `quads` quads each way, as a GLB in the tile's space:
    /// UVs in metres over four, so its ground tiles every 4 m across tile edges.
    fn tile(&self, middle: [f32; 2], size: f32, quads: u32) -> Vec<u8> {
        let step = size / quads as f32;
        let mut geometry = Geometry::default();
        for j in 0..=quads {
            for i in 0..=quads {
                let (x, z) = (i as f32 * step - size * 0.5, j as f32 * step - size * 0.5);
                let world = [middle[0] + x, middle[1] + z];
                geometry.positions.push([x, self.height(world), z]);
                geometry.normals.push(self.normal(world, step));
                geometry.uvs.push([world[0] * 0.25, world[1] * 0.25]);
            }
        }
        let row = quads + 1;
        for j in 0..quads {
            for i in 0..quads {
                let at = j * row + i;
                // Counter-clockwise seen from above.
                geometry.indices.extend([at, at + row, at + 1, at + 1, at + row, at + row + 1]);
            }
        }
        glb("tile", &[(GROUND_SLOT, &geometry)])
    }

    /// A rail through `points`, laid every [`WALK_STEP`] metres or less, each point at the terrain's height.
    fn rail(&self, points: &[[f32; 2]], lift: f32) -> Vec<[f32; 3]> {
        let mut rail = Vec::new();
        for leg in points.windows(2) {
            let steps = (distance(leg[0], leg[1]) / WALK_STEP).ceil().max(1.0) as u32;
            for step in 0..steps {
                let t = step as f32 / steps as f32;
                let at = [leg[0][0] + (leg[1][0] - leg[0][0]) * t, leg[0][1] + (leg[1][1] - leg[0][1]) * t];
                rail.push([at[0], self.height(at) + lift, at[1]]);
            }
        }
        let last = points[points.len() - 1];
        rail.push([last[0], self.height(last) + lift, last[1]]);
        rail
    }
}

/// The zone's probe grid: its box from the document over the walks' ground, seeing every terrain tile and tree
/// whose middle stands in it, under the zone's day. Grass is left out: a tuft is too small to light the grid.
fn probe_grid(
    grid: &NatureGrid,
    spacing: PerCook<f32>,
    terrain: &[([f32; 2], AssetId)],
    trees: &[Instance],
) -> ProbeGridDocument {
    let inside = |x: f32, z: f32, margin: f32| {
        x >= grid.min[0] - margin
            && x <= grid.max[0] + margin
            && z >= grid.min[1] - margin
            && z <= grid.max[1] + margin
    };
    let tiles = terrain
        .iter()
        .filter(|(middle, _)| inside(middle[0], middle[1], simulation::CELL_SIZE))
        .map(|&(middle, mesh)| BakeMesh { mesh, translation: [middle[0], 0.0, middle[1]], yaw: 0.0 });
    let trees = trees.iter().filter(|tree| inside(tree.at[0], tree.at[2], 8.0)).map(|tree| BakeMesh {
        mesh: tree.mesh,
        translation: tree.at,
        yaw: tree.turn,
    });
    ProbeGridDocument {
        bounds: BakeBounds {
            min: [grid.min[0], grid.floor, grid.min[1]],
            max: [grid.max[0], grid.top, grid.max[1]],
        },
        spacing,
        visibility: grid.visibility,
        rays: grid.rays,
        surfel_rays: grid.surfel_rays,
        surfel_spacing: grid.surfel_spacing,
        bounces: grid.bounces,
        bounce_strength: grid.bounce_strength,
        timeline: BakeTimeline::Document(DAY.to_owned()),
        meshes: tiles.chain(trees).collect(),
    }
}

/// Every surface in `dir` (`surfaces/`) as a material reading its three maps at `size` texels, by its stem.
fn grounds(
    dir: &Path,
    size: u32,
    sources: &mut Vec<Source>,
) -> Result<BTreeMap<String, AssetId>, BenchError> {
    let mut materials = BTreeMap::new();
    let mut names: Vec<_> = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    names.sort_by_key(|entry| entry.file_name());
    for entry in names {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(procedural::SOURCE_SUFFIX) else { continue };
        let path = entry.path();
        let surface = ProceduralTexture::read(&fs::read(&path)?)
            .map_err(|error| BenchError::Invalid { path: path.clone(), why: format!("{error:#}") })?;
        let surface = ProceduralTexture { size, ..surface };
        surface.check().map_err(|error| BenchError::Invalid {
            path: path.clone(),
            why: format!("at {size} texels: {error}"),
        })?;
        let text = document_bytes(&surface);
        let [color, normal, mask] = SurfaceMap::ALL.map(|map| {
            let path = format!("textures/{stem}.{}.ptex.ron", map_name(map));
            let id = asset_id(PART, &path);
            let max_size = MaxSize {
                mobile: NonZeroU32::new(super::district::MOBILE_SET_SIZE).filter(|cap| size > cap.get()),
                ..MaxSize::NONE
            };
            sources.push(Source::asset(
                path,
                text.clone(),
                ImportSettings::ProceduralTexture(ProceduralSettings { map, max_size }),
            ));
            id
        });
        let material = EnvironmentMaterial {
            base_color: [1.0; 4],
            base_color_map: Some(color),
            normal_map: Some(normal),
            mask_map: Some(mask),
            roughness: 1.0,
            roughness_range: [0.4, 1.0],
            specular_clamp: 0.25,
            wrap: 0.4,
            terminator: Terminator { width: 0.06, growth: 0.008, band: None },
            distance: DistanceStylization { start: 30.0, end: 400.0, desaturation: 0.4 },
            ..EnvironmentMaterial::default()
        };
        material
            .validate()
            .map_err(|error| BenchError::Invalid { path: path.clone(), why: format!("{error:#}") })?;
        let material_path = format!("materials/{stem}.material.ron");
        materials.insert(stem.to_owned(), asset_id(PART, &material_path));
        sources.push(Source::asset(
            material_path,
            document_bytes(&material),
            ImportSettings::Material(MaterialSettings::default()),
        ));
    }
    Ok(materials)
}

fn map_name(map: SurfaceMap) -> &'static str {
    match map {
        SurfaceMap::Color => "color",
        SurfaceMap::Normal => "normal",
        SurfaceMap::Mask => "mask",
    }
}

/// The grass tuft: `blades` blades in a disc, each a narrow triangle from a root on the ground to a leaning tip,
/// both faces wound outward (opaque foliage culls back faces), with W6's foliage stream, lit as the ground.
fn grass_sources(grass: &Grass) -> Result<Source, BenchError> {
    let mut geometry = Geometry::default();
    for blade in 0..grass.blades {
        let roll = |salt: u32| unit(seed(&[0x626c_6164, blade, salt]));
        // A point in the disc, by rejection-free square-root spread on a whole number of turns' sixteenths.
        let r = grass.radius * roll(0).sqrt();
        let [c, s] = sapphire_synthetic_zone::DIRECTIONS[(seed(&[blade, 1]) % 16) as usize];
        let root = [r * c, 0.0, r * s];
        let [lc, ls] = sapphire_synthetic_zone::DIRECTIONS[(seed(&[blade, 2]) % 16) as usize];
        let height = grass.height[0] + (grass.height[1] - grass.height[0]) * roll(3);
        let lean = height * 0.3 * roll(4);
        let tip = [root[0] + lc * lean, height, root[2] + ls * lean];
        // The blade's width lies across its lean.
        let half = [-ls * grass.width * 0.5, 0.0, lc * grass.width * 0.5];
        let base = geometry.positions.len() as u32;
        for at in
            [[root[0] - half[0], 0.0, root[2] - half[2]], [root[0] + half[0], 0.0, root[2] + half[2]], tip]
        {
            geometry.positions.push(at);
            geometry.normals.push([0.0, 1.0, 0.0]);
            geometry.uvs.push([0.5, 1.0 - at[1] / height]);
        }
        geometry.indices.extend([base, base + 1, base + 2, base, base + 2, base + 1]);
    }
    let mesh = MeshSettings {
        lods: LodSettings::none(),
        materials: BTreeMap::from([(BLADES_SLOT.to_owned(), asset_id(PART, "grass/grass.material.ron"))]),
        foliage: Some(FoliageSettings {
            normals: NormalTransfer::Up,
            slots: vec![BLADES_SLOT.to_owned()],
            blend: 1.0,
            canopy_from: 0.0,
        }),
        ..MeshSettings::default()
    };
    Ok(Source::asset(GRASS, glb("tuft", &[(BLADES_SLOT, &geometry)]), ImportSettings::Gltf(mesh)))
}

/// The blades' material: opaque, its gradient root to tip, swaying in the zone's wind.
fn grass_material(grass: &Grass) -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        alpha_cutoff: 0.0,
        wrap: 0.5,
        terminator_width: 0.16,
        terminator_growth: 0.006,
        shadow_tint: [0.95, 0.95, 1.08],
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root: grass.root, tip: grass.tip },
        wind: grass.wind,
        distance: DistanceStylization { start: 15.0, end: 140.0, desaturation: 0.45 },
        ..FoliageMaterial::default()
    }
}

/// A named area's marker, a stone block 2 m a side and 3 m tall in one slot, which its cell merges.
const MARKER_PREFAB: &str = "prefabs/marker.assembly.ron";

fn marker_prefab() -> Vec<u8> {
    "// A named area's marker, generated by the benchmark's nature zone (`sapphire_pipeline::bench::nature`).
(
    type: \"sapphire::pipeline::Assembly\",
    version: 1,
    data: (
        kits: {},
        slots: [(name: \"stone\", tile: (2.0, 2.0))],
        surfaces: [],
        place: [(piece: \"#box\", scale: (2.0, 3.0, 2.0), at: (0.0, 1.5, 0.0), slot: Some(\"stone\"))],
    ),
)
"
    .as_bytes()
    .to_vec()
}

/// A GLB of one mesh, a primitive a slot.
fn glb(name: &str, parts: &[(&str, &Geometry)]) -> Vec<u8> {
    let mut gltf = GltfWriter::default();
    let materials: Vec<usize> = parts
        .iter()
        .map(|(slot, _)| {
            gltf.add_material(json!({
                "name": slot,
                "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0] },
            }))
        })
        .collect();
    let primitives: Vec<(&Geometry, Option<usize>)> =
        parts.iter().zip(&materials).map(|((_, geometry), material)| (*geometry, Some(*material))).collect();
    let mesh = gltf.add_mesh(name, &primitives);
    let root = gltf.add_node(json!({ "name": name, "mesh": mesh }));
    gltf.add_root(root);
    gltf.to_glb()
}

/// Points on a jittered grid of `spacing` over the rectangle, each kept where `keep` says, with a roll of its own.
fn scatter(
    min: [f32; 2],
    max: [f32; 2],
    spacing: f32,
    salt: u32,
    keep: &dyn Fn([f32; 2]) -> bool,
) -> Vec<([f32; 2], u32)> {
    let (columns, rows) =
        (((max[0] - min[0]) / spacing).floor() as u32, ((max[1] - min[1]) / spacing).floor() as u32);
    let mut points = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let roll = seed(&[salt, column, row]);
            let jitter = |salt: u32| (unit(noise::hash(roll ^ salt)) - 0.5) * 0.7;
            let at = [
                min[0] + (column as f32 + 0.5 + jitter(1)) * spacing,
                min[1] + (row as f32 + 0.5 + jitter(2)) * spacing,
            ];
            if keep(at) {
                points.push((at, roll));
            }
        }
    }
    points
}

/// Value noise at `(x, z)` in lattice cells, from -1 to 1: each lattice point's value an integer hash, eased
/// between by a smoothstep.
fn value_noise(x: f32, z: f32, salt: u32) -> f32 {
    let (fx, fz) = (x.floor(), z.floor());
    let (ix, iz) = (fx as i32, fz as i32);
    let corner = |dx: i32, dz: i32| {
        2.0 * unit(seed(&[salt, ix.wrapping_add(dx) as u32, iz.wrapping_add(dz) as u32])) - 1.0
    };
    let (tx, tz) = (smoothstep(0.0, 1.0, x - fx), smoothstep(0.0, 1.0, z - fz));
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * tx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * tx;
    top + (bottom - top) * tz
}

fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far along the segment from `a` to `b` the nearest point to `at` is, as a share, and how far `at` is from it.
fn segment(at: [f32; 2], a: [f32; 2], b: [f32; 2]) -> (f32, f32) {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let length = dx * dx + dz * dz;
    let along = if length > 0.0 {
        (((at[0] - a[0]) * dx + (at[1] - a[1]) * dz) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (along, distance(at, [a[0] + dx * along, a[1] + dz * along]))
}

/// How far `at` is outside the closed polygon `outline`, in metres: negative inside it (by crossings).
fn from_outline(at: [f32; 2], outline: &[[f32; 2]]) -> f32 {
    let mut inside = false;
    let mut nearest = f32::MAX;
    for (k, &a) in outline.iter().enumerate() {
        let b = outline[(k + 1) % outline.len()];
        nearest = nearest.min(segment(at, a, b).1);
        if (a[1] > at[1]) != (b[1] > at[1]) && at[0] < a[0] + (at[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            inside = !inside;
        }
    }
    if inside { -nearest } else { nearest }
}

/// How far `at` is from the polyline through `points`.
fn from_polyline(at: [f32; 2], points: &[[f32; 2]]) -> f32 {
    points.windows(2).map(|leg| segment(at, leg[0], leg[1]).1).fold(f32::MAX, f32::min)
}

fn seed(values: &[u32]) -> u32 {
    values.iter().fold(0x9e37_79b9, |seed, &value| noise::hash(seed ^ value))
}

/// A seed as a share from 0 to just under 1, exactly on every machine.
fn unit(seed: u32) -> f32 {
    (seed >> 8) as f32 / (1u32 << 24) as f32
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])).sqrt()
}

#[cfg(test)]
mod tests;
