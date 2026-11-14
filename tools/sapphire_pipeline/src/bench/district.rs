//! The benchmark's district (M5 B2): a generated hub district at the spec's content budgets, from the
//! project's own parametric shapes and procedural textures, which cooks as a zone the sandbox opens.
//!
//! `bench/district/` holds:
//!
//! | File | What |
//! |---|---|
//! | `district.ron` | [`BenchDistrict`]: streets, building variants, props, lanterns, dressing, the look, and each preset's scale |
//! | `district_day.ron` | The district's day: a dusk key (19:45) and a night key (23:00) |
//! | `prefabs/<name>.assembly.ron` | The props, by file stem: parametric shapes and builtins only, since no kit is under `bench/` |
//! | `surfaces/<name>.ptex.ron` | Procedural texture specs ([`crate::procedural`]), the families texture sets are varied from |
//!
//! # The layout
//!
//! `cells` × `cells` cells of 32 m about the origin, an even count from four. Streets run along every cell's
//! edge: an avenue along each axis through the origin, streets on every other line, lanes on the rest, and an
//! alley inside a boundary wall round the edge. The four cells about the origin are the plaza, which the
//! simulation's half places its fight in (`sapphire_synthetic_zone::district`); every other cell is a block.
//! A block is a row of buildings along each side, facing the street there, a courtyard of props behind them,
//! and in front of them lantern posts every few metres, street furniture and, on avenues and streets, stalls.
//! Walls close some lane segments between two blocks, so a view down a lane ends a block or two away while
//! the avenues and streets run the district's length.
//!
//! # What it makes, under `district/`
//!
//! - `textures/set_NNN.{color,normal,mask}.ptex.ron` and `materials/set_NNN.material.ron`: a preset's
//!   `texture_sets` sets, set `n` varied from the `n`th surface family round (as B1 built them).
//! - `buildings/building_NN.assembly.ron`: `buildings.variants` variants, each a mesh of its own placed as
//!   instances (its footprint, storeys and roof from its number), its five slots bound to sets by its sidecar.
//! - `prefabs/`: the committed props, a cell's `ground`, and a lane's `closure` wall, which zones place and
//!   the cook merges into each cell's mesh, each placement drawing from its cell's palette of sets.
//! - `plaza.encounter.ron`: the plaza's fight, and `district_day.ron`, the day the zone names.
//! - `district.zone.ron`: the zone. Its simulation half (spawners, the plaza's volume, a checkpoint and the
//!   fight) is `sapphire_synthetic_zone::district`'s, written here as its code builds it; its presentation is
//!   the cells, buildings, lights, day, wind and fog, and the `tour` spline.
//!
//! # Seen by the tour
//!
//! The tour ([`TOUR`]) walks the street centrelines (+X is east, +Z south): north up the avenue from the
//! start through the plaza, east along a street to the district's end and back its whole length west (the
//! long view), south down a lane, past crossing lanes that walls close, east along the plaza's southern edge
//! and north into its middle. Nothing is placed within [`CORRIDOR`] of it. Lots are given
//! building variants nearest the tour first, so every variant stands beside it, and texture sets are given
//! out to the variants' slots and then to cells' palette entries, those whose nearest use is nearest the tour
//! first, so every set is drawn by something within [`SEEN`] of the tour: generation fails rather than make a
//! set it can't see.
//!
//! # Determinism
//!
//! Integer hashes, the four operations, `floor` and `sqrt` only: no transcendental is reached, and every turn
//! is a quarter.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::NonZeroU32;
use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_assets::AssetId;
use sapphire_assets::material::{DistanceStylization, EnvironmentMaterial, MaterialDocument, Terminator};
use sapphire_reflect::Versioned;
use sapphire_synthetic_zone::district as simulation;
use sapphire_zone::document::{Instance, Light, LightShape, Placement, Spline, ZoneDocument};
use sapphire_zone::grid::Grid;
use sapphire_zone::{ZoneFog, ZoneWind};
use serde::{Deserialize, Serialize};

use super::{BenchError, Preset, Presets, Source, asset_id, document_bytes, read_document};
use crate::assembly::{
    self, Assembly, Beam, Hall, Lantern, Mount, Occlusion, Opening, OpeningKind, Placement as Piece, Plinth,
    Posts, Projection, Roof, RoofKind, Scale, Shape, Side, SlotSpec, SurfaceRule, Wall,
};
use crate::character_maps::ramp::SrgbColor;
use crate::material::MaterialSettings;
use crate::meta::{ImportSettings, MeshSettings};
use crate::procedural::{self, ProceduralSettings, ProceduralTexture, SurfaceMap};
use crate::samples::noise;
use crate::texture::MaxSize;

/// This part's folder, under `bench/` and in the output.
pub const PART: &str = "district";
/// The zone's source, in the part's output: `--zone district` opens it.
pub const ZONE: &str = "district.zone.ron";
/// The spline a capture's and a bench's walk follows (`--zone-walk tour`).
pub const TOUR: &str = "tour";
/// The plaza's fight, in the part's output.
pub const ENCOUNTER: &str = "plaza.encounter.ron";
/// The day the zone names, in the part's output.
pub const DAY: &str = "district_day.ron";
/// A building variant's slots, which its sidecar binds to texture sets.
pub const BUILDING_SLOTS: [&str; 5] = ["wall", "stone", "roof", "wood", "paper"];
/// Half the width the tour keeps clear along its legs, in metres.
pub const CORRIDOR: f32 = 1.0;
/// How near the tour something must stand to count as seen from it, in metres: three quarters of a cell.
pub const SEEN: f32 = 24.0;

/// The most cells along a side: a zone's grid holds far more, but a benchmark this big would be a mistake.
const MAX_CELLS: u32 = 64;
/// The most texture sets.
const MAX_SETS: u32 = 1000;
/// The most building variants.
const MAX_VARIANTS: u32 = 256;
/// The narrowest courtyard a block keeps behind its rows, in metres.
const COURTYARD_MIN: f32 = 3.0;
/// How far a lantern post stands into the street from a block's edge, in metres.
const LANTERN_OUT: f32 = 1.0;
/// How far the tour's start and its east end stand in from the district's edge, in metres.
const TOUR_INSET: f32 = 4.0;

/// The district's document, `bench/district/district.ron` (`sapphire::pipeline::BenchDistrict`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct BenchDistrict {
    /// A cell's side, in metres: the simulation half's (`sapphire_synthetic_zone::district::CELL_SIZE`).
    pub cell_size: f32,
    pub streets: Streets,
    /// The share of lane segments between two blocks that a wall closes, 0 to 1.
    pub closures: f32,
    pub buildings: Buildings,
    /// Every prop prefab the district places, by its file's stem in `prefabs/`.
    pub props: BTreeMap<String, Prop>,
    pub lanterns: Lanterns,
    pub dressing: Dressing,
    /// Texture sets a cell's props and ground draw from.
    pub palette: u32,
    /// The day the zone names: a timeline document in `bench/district/`.
    pub day: String,
    #[serde(default)]
    pub wind: Option<ZoneWind>,
    #[serde(default)]
    pub fog: Option<ZoneFog>,
    pub presets: Presets<DistrictScale>,
}

impl Versioned for BenchDistrict {
    /// 1: M5 B1. 2: M5 B2, the district.
    const VERSION: u32 = 2;
}

/// The streets' widths, in metres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Streets {
    pub avenue: f32,
    pub street: f32,
    pub lane: f32,
    /// The alley between the outer blocks and the boundary wall.
    pub edge: f32,
}

/// The building variants: variant `n` takes footprint `n % footprints.len()`, storeys `n % storeys.len()` and
/// roof `n % roofs.len()`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Buildings {
    pub variants: u32,
    /// Width along the street and depth, in metres.
    pub footprints: Vec<[f32; 2]>,
    pub storeys: Vec<u32>,
    pub roofs: Vec<RoofKind>,
    pub storey_height: f32,
    /// Between neighbours in a row, least and most, in metres.
    pub gap: [f32; 2],
}

/// A prop prefab.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prop {
    /// Half the rectangle it keeps clear about its origin, along its own X and Z (its front faces +Z), in metres.
    pub clear: [f32; 2],
}

/// The lantern posts, and the light each holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lanterns {
    pub prefab: String,
    /// Metres between posts along a side.
    pub spacing: f32,
    pub light: LanternLight,
}

/// A lantern's light.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanternLight {
    /// Over the post's foot, in metres.
    pub height: f32,
    /// Linear RGB.
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
}

/// What dresses the streets, courtyards and plaza.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dressing {
    /// Props against buildings' fronts, and between lantern posts.
    pub frontage: Vec<String>,
    /// Against each building's front, at most four.
    pub per_building: u32,
    pub stall: String,
    /// The share of buildings on avenues and streets with a stall in front.
    pub stalls: f32,
    pub courtyard: Vec<String>,
    /// Metres between a courtyard's grid points.
    pub courtyard_spacing: f32,
    /// The share of a courtyard's grid points that hold a prop.
    pub courtyard_fill: f32,
    /// Round the plaza's rim, between its lanterns.
    pub plaza: Vec<String>,
    pub boundary: String,
    /// A boundary wall prefab's length, in metres.
    pub boundary_length: f32,
}

/// What a preset makes of the district.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistrictScale {
    /// Cells along each side: even, from four.
    pub cells: u32,
    pub texture_sets: u32,
    /// Texels on a side of every map.
    pub texture_size: u32,
}

/// Generates the district from the documents in `dir` (`bench/district/`) at `preset`'s scale.
pub fn generate(dir: &Path, preset: Preset) -> Result<Vec<Source>, BenchError> {
    let document_path = dir.join("district.ron");
    let district: BenchDistrict = read_document(&document_path)?;
    let scale = *district.presets.get(preset);
    let invalid = |why: String| BenchError::Invalid { path: document_path.clone(), why };
    check(&district, scale).map_err(|why| invalid(format!("{preset}: {why}")))?;

    let mut sources = Vec::new();
    let materials = texture_sets(&dir.join("surfaces"), scale, &mut sources)?;

    // The props' prefabs and their slots.
    let mut slots: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut prefab_bytes = BTreeMap::new();
    for (stem, bytes) in files_ending(&dir.join("prefabs"), assembly::SOURCE_SUFFIX)? {
        let read = assembly::read(&bytes).map_err(|error| BenchError::Invalid {
            path: dir.join("prefabs").join(format!("{stem}{}", assembly::SOURCE_SUFFIX)),
            why: format!("{error:#}"),
        })?;
        slots.insert(stem.clone(), read.slots.into_iter().map(|slot| slot.name).collect());
        prefab_bytes.insert(stem, bytes);
    }
    let closure = closure_prefab(&district).map_err(|why| invalid(format!("a lane's closure: {why}")))?;
    for (stem, bytes) in [("ground", ground_prefab(district.cell_size)), ("closure", closure)] {
        if prefab_bytes.insert(stem.to_owned(), bytes).is_some() {
            return Err(invalid(format!(
                "`prefabs/{stem}.assembly.ron` is the district's to make: rename it"
            )));
        }
        let names = if stem == "ground" { vec!["ground".to_owned()] } else { prop_slots() };
        slots.insert(stem.to_owned(), names);
    }
    for name in district.props.keys() {
        if !prefab_bytes.contains_key(name) {
            return Err(invalid(format!("`props` names `{name}`, which `prefabs/` doesn't hold")));
        }
    }

    let plan = Plan::new(&district, scale.cells);
    let slot_counts: BTreeMap<String, usize> =
        slots.iter().map(|(stem, names)| (stem.clone(), names.len())).collect();
    let assignment =
        Assignment::new(&plan, &district.buildings, district.palette, scale.texture_sets, &slot_counts)
            .map_err(|why| invalid(format!("{preset}: {why}")))?;
    let set = |user: usize| materials[assignment.sets[user] as usize];

    for (stem, bytes) in prefab_bytes {
        // Each slot bound to a set by default; every placement binds its own.
        let bound =
            slots[&stem].iter().enumerate().map(|(i, name)| (name.clone(), materials[i % materials.len()]));
        let settings = MeshSettings { materials: bound.collect(), ..MeshSettings::default() };
        sources.push(Source::uncooked(prefab_path(&stem), bytes, ImportSettings::Assembly(settings)));
    }
    for variant in 0..district.buildings.variants {
        let building = building(&district.buildings, variant)
            .map_err(|why| invalid(format!("building variant {variant}: {why}")))?;
        let bound = BUILDING_SLOTS
            .iter()
            .enumerate()
            .map(|(slot, name)| ((*name).to_owned(), set(variant as usize * BUILDING_SLOTS.len() + slot)));
        let settings = MeshSettings { materials: bound.collect(), ..MeshSettings::default() };
        sources.push(Source::asset(
            building_path(variant),
            document_bytes(&building),
            ImportSettings::Assembly(settings),
        ));
    }

    // The zone: each cell's ground and props from its palette, the buildings as instances, the lanterns' lights.
    // A cell's ground draws its palette's first entry; a prop of rank `r` in its cell draws entries from `r` on.
    let placement = |prop: &Placed, rank: usize| {
        let palette = &assignment.palettes[&prop.cell];
        let bound = slots[&prop.prefab]
            .iter()
            .enumerate()
            .map(|(slot, name)| (name.clone(), set(palette[(rank + slot) % palette.len()])))
            .collect();
        Placement {
            prefab: asset_id(PART, &prefab_path(&prop.prefab)),
            at: [prop.at[0], 0.0, prop.at[1]],
            turn: prop.turn,
            scale: [1.0; 3],
            materials: bound,
            backdrop: false,
            split: None,
            colliders: Vec::new(),
        }
    };
    let mut placements: Vec<Placement> = plan
        .cells()
        .into_iter()
        .map(|cell| {
            placement(&Placed { prefab: "ground".to_owned(), cell, at: plan.cell_middle(cell), turn: 0.0 }, 0)
        })
        .collect();
    placements.extend(plan.ordered_props().into_iter().map(|(prop, rank)| placement(prop, rank)));
    let instances = plan
        .lots
        .iter()
        .enumerate()
        .map(|(lot, at)| Instance {
            mesh: asset_id(PART, &building_path(assignment.variants[lot])),
            at: [at.middle[0], 0.0, at.middle[1]],
            turn: at.turn,
            scale: [1.0; 3],
            backdrop: false,
            colliders: Vec::new(),
        })
        .collect();
    let light = district.lanterns.light;
    let lights = plan
        .lights
        .iter()
        .map(|at| Light {
            position: [at[0], light.height, at[1]],
            color: light.color,
            intensity: light.intensity,
            range: light.range,
            shape: LightShape::Point,
            at_night: true,
            backdrop: false,
        })
        .collect();

    let encounter = asset_id(PART, ENCOUNTER);
    sources.push(Source::asset(
        ENCOUNTER,
        document_bytes(&simulation::encounter()),
        ImportSettings::Encounter(Default::default()),
    ));
    let day = fs::read(dir.join(&district.day))?;
    sources.push(Source::input(DAY, day));
    let zone = ZoneDocument {
        grid: Grid { cell_size: district.cell_size },
        placements,
        instances,
        lights,
        probes: None,
        reflections: Vec::new(),
        day: Some(DAY.to_owned()),
        rigs: None,
        sky: None,
        wind: district.wind,
        weather: None,
        fog: district.fog,
        colliders: None,
        spawners: simulation::spawners(scale.cells),
        triggers: simulation::triggers(),
        checkpoints: simulation::checkpoints(),
        splines: vec![Spline {
            name: TOUR.to_owned(),
            points: plan.tour.iter().map(|at| [at[0], 0.0, at[1]]).collect(),
            closed: false,
        }],
        navigation: None,
        encounters: vec![encounter],
        exits: Vec::new(),
    };
    zone.check().map_err(|error| invalid(format!("{preset}: the zone it makes: {error}")))?;
    sources.push(Source::asset(ZONE, document_bytes(&zone), ImportSettings::Zone(Default::default())));
    Ok(sources)
}

/// What a document can be wrong about that serde can't see.
fn check(district: &BenchDistrict, scale: DistrictScale) -> Result<(), String> {
    simulation::check_cells(scale.cells).map_err(|error| error.to_string())?;
    if scale.cells > MAX_CELLS || !(1..=MAX_SETS).contains(&scale.texture_sets) {
        return Err(format!("at most {MAX_CELLS} cells a side and 1 to {MAX_SETS} texture sets"));
    }
    if district.cell_size != simulation::CELL_SIZE {
        return Err(format!("`cell_size` is the simulation half's, {} m", simulation::CELL_SIZE));
    }
    let Streets { avenue, street, lane, edge } = district.streets;
    if ![avenue, street, lane, edge].iter().all(|width| width.is_finite() && (2.0..=16.0).contains(width)) {
        return Err("`streets` are 2 to 16 m wide".to_owned());
    }
    if !(0.0..=1.0).contains(&district.closures) {
        return Err("`closures` is a share, from 0 to 1".to_owned());
    }
    let buildings = &district.buildings;
    if !(1..=MAX_VARIANTS).contains(&buildings.variants)
        || buildings.footprints.is_empty()
        || buildings.storeys.is_empty()
        || buildings.roofs.is_empty()
    {
        return Err(format!(
            "1 to {MAX_VARIANTS} building variants, from at least a footprint, a storey count and a roof"
        ));
    }
    let combinations: BTreeSet<_> = (0..buildings.variants as usize)
        .map(|n| (n % buildings.footprints.len(), n % buildings.storeys.len(), n % buildings.roofs.len()))
        .collect();
    if combinations.len() != buildings.variants as usize {
        return Err(
            "two building variants come out the same: give `footprints`, `storeys` and `roofs` lengths \
             with no common factor"
                .to_owned(),
        );
    }
    let [least, most] = buildings.gap;
    if !(least.is_finite() && most.is_finite() && 0.5 <= least && least <= most) {
        return Err("`buildings.gap` is least then most, from half a metre".to_owned());
    }
    if !buildings.footprints.iter().flatten().all(|side| side.is_finite() && (4.0..=12.0).contains(side)) {
        return Err("`buildings.footprints` are 4 to 12 m a side".to_owned());
    }
    if !(buildings.storey_height.is_finite() && (2.8..=5.0).contains(&buildings.storey_height))
        || !buildings.storeys.iter().all(|storeys| (1..=4).contains(storeys))
    {
        return Err("1 to 4 storeys of 2.8 to 5 m".to_owned());
    }
    let dressing = &district.dressing;
    let named = [&district.lanterns.prefab, &dressing.stall, &dressing.boundary]
        .into_iter()
        .chain(&dressing.frontage)
        .chain(&dressing.courtyard)
        .chain(&dressing.plaza);
    for name in named {
        if !district.props.contains_key(name) {
            return Err(format!("`{name}` isn't one of `props`"));
        }
    }
    if dressing.frontage.is_empty() || dressing.courtyard.is_empty() || dressing.plaza.is_empty() {
        return Err("`dressing` has frontage, courtyard and plaza props".to_owned());
    }
    let shares = [dressing.stalls, dressing.courtyard_fill];
    if dressing.per_building > 4 || !shares.iter().all(|share| (0.0..=1.0).contains(share)) {
        return Err("at most four props a building, and shares from 0 to 1".to_owned());
    }
    let lengths = [district.lanterns.spacing, dressing.courtyard_spacing, dressing.boundary_length];
    if !lengths.iter().all(|length| length.is_finite() && *length >= 1.0) {
        return Err(
            "lantern spacing, courtyard spacing and the boundary's length are at least a metre".to_owned()
        );
    }
    let light = district.lanterns.light;
    let light_numbers = [light.height, light.intensity, light.range];
    if !light_numbers.iter().chain(&light.color).all(|v| v.is_finite() && *v >= 0.0) || light.range <= 0.0 {
        return Err("a lantern's light has a height, a colour, an intensity and a range".to_owned());
    }
    if !district.props.values().flat_map(|prop| prop.clear).all(|half| half.is_finite() && half > 0.0) {
        return Err("every prop keeps some room clear".to_owned());
    }
    if district.palette == 0 {
        return Err("a cell's palette holds at least a set".to_owned());
    }
    if district.day.contains(['/', '\\']) {
        return Err("`day` is a file in `bench/district/`".to_owned());
    }
    Ok(())
}

/// The kind of a line between cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// The district's edge: an alley inside the boundary wall.
    Edge,
    Avenue,
    Street,
    Lane,
}

/// A rectangle on the ground, `[x, z]` least and most corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Rect {
    fn about(middle: [f32; 2], half: [f32; 2]) -> Self {
        Self {
            min: [middle[0] - half[0], middle[1] - half[1]],
            max: [middle[0] + half[0], middle[1] + half[1]],
        }
    }

    fn overlaps(&self, other: &Self) -> bool {
        (0..2).all(|axis| self.min[axis] < other.max[axis] && other.min[axis] < self.max[axis])
    }

    pub fn contains(&self, at: [f32; 2]) -> bool {
        (0..2).all(|axis| self.min[axis] <= at[axis] && at[axis] <= self.max[axis])
    }

    fn size(&self, axis: usize) -> f32 {
        self.max[axis] - self.min[axis]
    }
}

/// A building's lot.
#[derive(Clone, Debug, PartialEq)]
pub struct Lot {
    pub cell: [u32; 2],
    /// Its footprint's middle.
    pub middle: [f32; 2],
    /// Degrees about +Y, so its front faces the street.
    pub turn: f32,
    /// Which of `buildings.footprints`.
    pub footprint: usize,
}

/// A placed prop (named so beside the zone's [`Placement`]).
#[derive(Clone, Debug, PartialEq)]
struct Placed {
    prefab: String,
    cell: [u32; 2],
    at: [f32; 2],
    turn: f32,
}

/// Where everything goes: a pure function of the document and the cell count.
pub struct Plan {
    cells: u32,
    cell_size: f32,
    half: f32,
    pub ways: Vec<Way>,
    widths: Streets,
    pub tour: Vec<[f32; 2]>,
    pub lots: Vec<Lot>,
    props: Vec<Placed>,
    /// Each lantern post's foot.
    pub lights: Vec<[f32; 2]>,
    /// The closed lane segments' walls.
    pub closures: Vec<Rect>,
    taken: Taken,
    next_footprint: usize,
}

/// One side of a block: where its edge starts, which way it runs and faces, and the street it faces.
#[derive(Clone, Copy, Debug)]
struct BlockSide {
    /// The edge's start, and the unit direction it runs along.
    start: [f32; 2],
    along: [f32; 2],
    /// Outward, toward the street.
    normal: [f32; 2],
    turn: f32,
    way: Way,
    /// How deep a building on it may be.
    depth: f32,
}

impl BlockSide {
    fn at(&self, along: f32, out: f32) -> [f32; 2] {
        [
            self.start[0] + self.along[0] * along + self.normal[0] * out,
            self.start[1] + self.along[1] * along + self.normal[1] * out,
        ]
    }
}

impl Plan {
    /// Lays the district out: its tour, then its buildings, closures, stalls, lanterns, frontage, plaza,
    /// courtyards and boundary, each kept clear of what came before.
    pub fn new(district: &BenchDistrict, cells: u32) -> Self {
        let half = simulation::half_side(cells);
        let ways = (0..=cells)
            .map(|k| match k {
                0 => Way::Edge,
                k if k == cells => Way::Edge,
                k if k == cells / 2 => Way::Avenue,
                k if k.is_multiple_of(2) => Way::Street,
                _ => Way::Lane,
            })
            .collect();
        let mut plan = Self {
            cells,
            cell_size: district.cell_size,
            half,
            ways,
            widths: district.streets,
            tour: Vec::new(),
            lots: Vec::new(),
            props: Vec::new(),
            lights: Vec::new(),
            closures: Vec::new(),
            taken: Taken::new(half),
            next_footprint: 0,
        };
        plan.tour = plan.tour_points();
        for leg in plan.tour.windows(2) {
            let rect = Rect {
                min: [leg[0][0].min(leg[1][0]) - CORRIDOR, leg[0][1].min(leg[1][1]) - CORRIDOR],
                max: [leg[0][0].max(leg[1][0]) + CORRIDOR, leg[0][1].max(leg[1][1]) + CORRIDOR],
            };
            plan.taken.take(rect);
        }
        let blocks: Vec<[u32; 2]> = plan.cells().into_iter().filter(|&cell| !plan.is_plaza(cell)).collect();
        let mut sides = Vec::new();
        for &cell in &blocks {
            sides.push((cell, plan.rows(district, cell)));
        }
        plan.close_lanes(district);
        for (cell, rows) in &sides {
            plan.dress_rows(district, *cell, rows);
        }
        plan.dress_plaza(district);
        for &cell in &blocks {
            plan.courtyard(district, cell);
        }
        plan.boundary(district);
        plan
    }

    /// Every cell, by `[x, z]` index from the district's least corner, in row order.
    pub fn cells(&self) -> Vec<[u32; 2]> {
        let cells = self.cells;
        (0..cells).flat_map(|z| (0..cells).map(move |x| [x, z])).collect()
    }

    pub fn is_plaza(&self, [x, z]: [u32; 2]) -> bool {
        let middle = self.cells / 2;
        (middle - 1..=middle).contains(&x) && (middle - 1..=middle).contains(&z)
    }

    /// Line `k`'s place, on either axis.
    pub fn line(&self, k: u32) -> f32 {
        -self.half + k as f32 * self.cell_size
    }

    fn half_width(&self, k: u32) -> f32 {
        let Streets { avenue, street, lane, edge } = self.widths;
        match self.ways[k as usize] {
            Way::Edge => edge,
            Way::Avenue => avenue * 0.5,
            Way::Street => street * 0.5,
            Way::Lane => lane * 0.5,
        }
    }

    pub fn cell_middle(&self, [x, z]: [u32; 2]) -> [f32; 2] {
        [self.line(x) + self.cell_size * 0.5, self.line(z) + self.cell_size * 0.5]
    }

    /// The cell `at` lies in.
    pub fn cell_of(&self, at: [f32; 2]) -> [u32; 2] {
        let index = |v: f32| {
            (((v + self.half) / self.cell_size).floor() as i64).clamp(0, i64::from(self.cells) - 1) as u32
        };
        [index(at[0]), index(at[1])]
    }

    /// A block's ground inside its streets.
    pub fn block(&self, [x, z]: [u32; 2]) -> Rect {
        Rect {
            min: [self.line(x) + self.half_width(x), self.line(z) + self.half_width(z)],
            max: [self.line(x + 1) - self.half_width(x + 1), self.line(z + 1) - self.half_width(z + 1)],
        }
    }

    /// The plaza's paving inside the streets that edge it.
    pub fn plaza(&self) -> Rect {
        let middle = self.cells / 2;
        Rect {
            min: [self.line(middle - 1) + self.half_width(middle - 1); 2],
            max: [self.line(middle + 1) - self.half_width(middle + 1); 2],
        }
    }

    /// The tour's corners: up the north–south avenue through the plaza to the long street, east along it to
    /// the district's end, back west its length to the first lane, south down that lane, east along the
    /// plaza's southern edge, and north into its middle.
    fn tour_points(&self) -> Vec<[f32; 2]> {
        let start = self.half - TOUR_INSET;
        let long = self.line(self.cells / 4);
        let west = self.line(1);
        let back = self.line(self.cells / 2 + 1);
        let mut points = vec![
            [0.0, start],
            [0.0, long],
            [start, long],
            [west, long],
            [west, back],
            [0.0, back],
            [0.0, 0.0],
        ];
        points.dedup();
        points
    }

    /// The rows of buildings along `cell`'s block's four sides: north and south first, then west and east
    /// between them. Returns each side and how far its row reaches in from the block's edge.
    fn rows(&mut self, district: &BenchDistrict, cell: [u32; 2]) -> Vec<(BlockSide, f32)> {
        let block = self.block(cell);
        let [x, z] = cell;
        let deep = |extent: f32| (extent - COURTYARD_MIN) * 0.5;
        let north = BlockSide {
            start: [block.min[0], block.min[1]],
            along: [1.0, 0.0],
            normal: [0.0, -1.0],
            turn: 180.0,
            way: self.ways[z as usize],
            depth: deep(block.size(1)),
        };
        let south = BlockSide {
            start: [block.min[0], block.max[1]],
            normal: [0.0, 1.0],
            turn: 0.0,
            way: self.ways[z as usize + 1],
            ..north
        };
        let mut sides = Vec::new();
        let mut reach = [0.0_f32; 2];
        for (index, side) in [north, south].into_iter().enumerate() {
            reach[index] = self.row(district, cell, side, 0.0, block.size(0));
            sides.push((side, reach[index]));
        }
        let gap = district.buildings.gap[0];
        let west = BlockSide {
            start: [block.min[0], block.min[1]],
            along: [0.0, 1.0],
            normal: [-1.0, 0.0],
            turn: 270.0,
            way: self.ways[x as usize],
            depth: deep(block.size(0)),
        };
        let east = BlockSide {
            start: [block.max[0], block.min[1]],
            normal: [1.0, 0.0],
            turn: 90.0,
            way: self.ways[x as usize + 1],
            ..west
        };
        for side in [west, east] {
            let from = reach[0] + gap;
            let to = block.size(1) - reach[1] - gap;
            let depth = self.row(district, cell, side, from, to);
            sides.push((side, depth));
        }
        sides
    }

    /// Fills `side` from `from` to `to` metres along it with buildings, footprints taken round in turn where
    /// they fit, the row centred; returns its deepest.
    fn row(&mut self, district: &BenchDistrict, cell: [u32; 2], side: BlockSide, from: f32, to: f32) -> f32 {
        let buildings = &district.buildings;
        let footprints = &buildings.footprints;
        let mut lots: Vec<(f32, usize)> = Vec::new();
        let mut cursor = from;
        loop {
            let fits =
                (0..footprints.len()).map(|k| (self.next_footprint + k) % footprints.len()).find(|&f| {
                    let [width, depth] = footprints[f];
                    cursor + width <= to && depth <= side.depth
                });
            let Some(footprint) = fits else { break };
            self.next_footprint = footprint + 1;
            lots.push((cursor, footprint));
            let [least, most] = buildings.gap;
            let seed = seed(&[cell[0], cell[1], side.turn as u32, lots.len() as u32]);
            cursor += footprints[footprint][0] + least + (most - least) * unit(seed);
        }
        let Some(&(last, footprint)) = lots.last() else { return 0.0 };
        let shift = (to - (last + footprints[footprint][0])) * 0.5;
        let mut deepest = 0.0_f32;
        for (start, footprint) in lots {
            let [width, depth] = footprints[footprint];
            let middle = side.at(start + shift + width * 0.5, -depth * 0.5);
            let half =
                if side.along[0] != 0.0 { [width * 0.5, depth * 0.5] } else { [depth * 0.5, width * 0.5] };
            self.taken.take(Rect::about(middle, half));
            self.lots.push(Lot { cell, middle, turn: side.turn, footprint });
            deepest = deepest.max(depth);
        }
        deepest
    }

    /// Walls across lane segments between two blocks, by `closures`' share, where the tour doesn't pass.
    fn close_lanes(&mut self, district: &BenchDistrict) {
        let lane = district.streets.lane;
        for k in 1..self.cells {
            if self.ways[k as usize] != Way::Lane {
                continue;
            }
            for j in 0..self.cells {
                // A lane at x = line(k), between cells (k − 1, j) and (k, j); and one at z = line(k).
                for across in [false, true] {
                    let [a, b] = if across { [[j, k - 1], [j, k]] } else { [[k - 1, j], [k, j]] };
                    if self.is_plaza(a) || self.is_plaza(b) {
                        continue;
                    }
                    if unit(seed(&[k, j, u32::from(across), 0x6c61_6e65])) >= district.closures {
                        continue;
                    }
                    let along = self.line(j) + self.cell_size * 0.5;
                    let middle = if across { [along, self.line(k)] } else { [self.line(k), along] };
                    let half = if across { [0.3, lane * 0.5] } else { [lane * 0.5, 0.3] };
                    let wall = Rect::about(middle, half);
                    if !self.taken.free(&wall) {
                        continue;
                    }
                    self.taken.take(wall);
                    self.closures.push(wall);
                    let turn = if across { 90.0 } else { 0.0 };
                    self.props.push(Placed {
                        prefab: "closure".to_owned(),
                        cell: self.cell_of(middle),
                        at: middle,
                        turn,
                    });
                }
            }
        }
    }

    /// Places a prop of `prefab` at `at` facing `turn` if its room is free, and says whether it did.
    fn place(&mut self, district: &BenchDistrict, prefab: &str, at: [f32; 2], turn: f32) -> bool {
        let [across, along] = district.props[prefab].clear;
        let quarter = (turn / 90.0) as i32 % 2 != 0;
        let rect = Rect::about(at, if quarter { [along, across] } else { [across, along] });
        let ground = Rect { min: [-self.half; 2], max: [self.half; 2] };
        if !self.taken.free(&rect) || !ground.contains(at) {
            return false;
        }
        self.taken.take(rect);
        self.props.push(Placed { prefab: prefab.to_owned(), cell: self.cell_of(at), at, turn });
        true
    }

    /// Stalls, lantern posts and frontage along a block's sides.
    fn dress_rows(&mut self, district: &BenchDistrict, cell: [u32; 2], sides: &[(BlockSide, f32)]) {
        let dressing = &district.dressing;
        let lots: Vec<Lot> = self.lots.iter().filter(|lot| lot.cell == cell).cloned().collect();
        let on_side = |side: &BlockSide, lot: &Lot| lot.turn == side.turn;
        for (index, (side, _)) in sides.iter().enumerate() {
            let length =
                if side.along[0] != 0.0 { self.block(cell).size(0) } else { self.block(cell).size(1) };
            // Where each of this side's buildings stands along it, and its width.
            let fronts: Vec<(f32, f32)> = lots
                .iter()
                .filter(|lot| on_side(side, lot))
                .map(|lot| {
                    let along = (lot.middle[0] - side.start[0]) * side.along[0]
                        + (lot.middle[1] - side.start[1]) * side.along[1];
                    (along, district.buildings.footprints[lot.footprint][0])
                })
                .collect();
            // Stalls first, on avenues and streets, to one side of a door.
            if matches!(side.way, Way::Avenue | Way::Street) {
                for (n, &(middle, width)) in fronts.iter().enumerate() {
                    let roll = seed(&[cell[0], cell[1], index as u32, n as u32, 0x7374_616c]);
                    if width < 6.0 || unit(roll) >= dressing.stalls {
                        continue;
                    }
                    let offset = (width * 0.5 - 1.6) * if roll.is_multiple_of(2) { 1.0 } else { -1.0 };
                    let out = district.props[&dressing.stall].clear[1] + 0.05;
                    self.place(district, &dressing.stall.clone(), side.at(middle + offset, out), side.turn);
                }
            }
            // Lantern posts, and furniture halfway between them.
            let spacing = district.lanterns.spacing;
            let posts = ((length / spacing).floor() as u32).max(1);
            let first = (length - (posts - 1) as f32 * spacing) * 0.5;
            for post in 0..posts {
                let along = first + post as f32 * spacing;
                if self.place(
                    district,
                    &district.lanterns.prefab.clone(),
                    side.at(along, LANTERN_OUT),
                    side.turn,
                ) {
                    self.lights.push(side.at(along, LANTERN_OUT));
                }
                if post + 1 < posts {
                    let pick = seed(&[cell[0], cell[1], index as u32, post, 0x6675_726e]);
                    let prefab = dressing.frontage[pick as usize % dressing.frontage.len()].clone();
                    let out = district.props[&prefab].clear[1] + 0.05;
                    self.place(district, &prefab, side.at(along + spacing * 0.5, out), side.turn);
                }
            }
            // Against each front, either side of the door.
            for (n, &(middle, width)) in fronts.iter().enumerate() {
                let offsets = [-(width * 0.5 - 0.7), width * 0.5 - 0.7, -width * 0.3, width * 0.3];
                for (k, offset) in offsets.into_iter().take(dressing.per_building as usize).enumerate() {
                    let pick = seed(&[cell[0], cell[1], index as u32, n as u32, k as u32]);
                    let prefab = dressing.frontage[pick as usize % dressing.frontage.len()].clone();
                    let out = district.props[&prefab].clear[1] + 0.05;
                    self.place(district, &prefab, side.at(middle + offset, out), side.turn);
                }
            }
        }
    }

    /// Lantern posts round the plaza's rim a metre in, its furniture between them, the avenues' mouths open.
    fn dress_plaza(&mut self, district: &BenchDistrict) {
        let plaza = self.plaza();
        let side = plaza.size(0);
        let mouth = district.streets.avenue * 0.5 + 1.0;
        let spacing = district.lanterns.spacing;
        let posts = (side / spacing).floor() as u32;
        let first = (side - (posts - 1) as f32 * spacing) * 0.5;
        // Each rim's start, direction, inward normal and the turn that faces inward.
        let rims = [
            ([plaza.min[0], plaza.min[1]], [1.0, 0.0], [0.0, 1.0], 0.0),
            ([plaza.min[0], plaza.max[1]], [1.0, 0.0], [0.0, -1.0], 180.0),
            ([plaza.min[0], plaza.min[1]], [0.0, 1.0], [1.0, 0.0], 90.0),
            ([plaza.max[0], plaza.min[1]], [0.0, 1.0], [-1.0, 0.0], 270.0),
        ];
        for (index, (start, along, inward, turn)) in rims.into_iter().enumerate() {
            let at = |distance: f32, depth: f32| {
                [
                    start[0] + along[0] * distance + inward[0] * depth,
                    start[1] + along[1] * distance + inward[1] * depth,
                ]
            };
            for post in 0..posts {
                let distance = first + post as f32 * spacing;
                if (distance - side * 0.5).abs() < mouth {
                    continue;
                }
                if self.place(district, &district.lanterns.prefab.clone(), at(distance, LANTERN_OUT), turn) {
                    self.lights.push(at(distance, LANTERN_OUT));
                }
                let between = distance + spacing * 0.5;
                if post + 1 < posts && (between - side * 0.5).abs() >= mouth {
                    let prefab =
                        &district.dressing.plaza[(index + post as usize) % district.dressing.plaza.len()];
                    let depth = district.props[prefab].clear[1] + 0.3;
                    self.place(district, &prefab.clone(), at(between, depth), turn);
                }
            }
        }
    }

    /// A grid of props in a block's courtyard, behind its rows.
    fn courtyard(&mut self, district: &BenchDistrict, cell: [u32; 2]) {
        let dressing = &district.dressing;
        let block = self.block(cell);
        let spacing = dressing.courtyard_spacing;
        let across = |axis: usize| ((block.size(axis) / spacing).floor() as u32).max(1);
        let [columns, rows] = [across(0), across(1)];
        for row in 0..rows {
            for column in 0..columns {
                let roll = seed(&[cell[0], cell[1], column, row, 0x636f_7572]);
                if unit(roll) >= dressing.courtyard_fill {
                    continue;
                }
                let jitter = |salt: u32| (unit(seed(&[roll, salt])) - 0.5) * spacing * 0.3;
                let at = [
                    block.min[0] + (column as f32 + 0.5) * spacing + jitter(1),
                    block.min[1] + (row as f32 + 0.5) * spacing + jitter(2),
                ];
                let prefab = dressing.courtyard[(roll >> 8) as usize % dressing.courtyard.len()].clone();
                let turn = ((roll >> 4) % 4) as f32 * 90.0;
                // Only behind the rows: a prop on a block's edge would stand in its street.
                let inner = Rect {
                    min: [block.min[0] + 1.0, block.min[1] + 1.0],
                    max: [block.max[0] - 1.0, block.max[1] - 1.0],
                };
                if inner.contains(at) {
                    self.place(district, &prefab, at, turn);
                }
            }
        }
    }

    /// The boundary wall's lengths round the district's edge.
    fn boundary(&mut self, district: &BenchDistrict) {
        let length = district.dressing.boundary_length;
        let count = (self.half * 2.0 / length).floor() as u32;
        let first = -(count as f32 - 1.0) * length * 0.5;
        let inset = self.half - district.props[&district.dressing.boundary].clear[1];
        for n in 0..count {
            let along = first + n as f32 * length;
            for (at, turn) in [
                ([along, -inset], 0.0),
                ([along, inset], 180.0),
                ([-inset, along], 90.0),
                ([inset, along], 270.0),
            ] {
                self.props.push(Placed {
                    prefab: district.dressing.boundary.clone(),
                    cell: self.cell_of(at),
                    at,
                    turn,
                });
            }
        }
    }

    /// The cells the tour crosses, in the order it first reaches them.
    pub fn tour_cells(&self) -> Vec<[u32; 2]> {
        let mut cells = Vec::new();
        for leg in self.tour.windows(2) {
            let length = distance(leg[0], leg[1]);
            let steps = (length.ceil() as u32).max(1);
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                let at = [leg[0][0] + (leg[1][0] - leg[0][0]) * t, leg[0][1] + (leg[1][1] - leg[0][1]) * t];
                let cell = self.cell_of(at);
                if !cells.contains(&cell) {
                    cells.push(cell);
                }
            }
        }
        cells
    }

    /// How far `at` is from the tour, in metres.
    pub fn from_tour(&self, at: [f32; 2]) -> f32 {
        self.tour.windows(2).map(|leg| from_segment(at, leg[0], leg[1])).fold(f32::INFINITY, f32::min)
    }

    /// The props cell by cell, nearest the tour first, each with its rank in its cell, which picks its
    /// palette entries.
    fn ordered_props(&self) -> Vec<(&Placed, usize)> {
        let mut props: Vec<&Placed> = self.props.iter().collect();
        props.sort_by(|a, b| a.cell.cmp(&b.cell).then(self.from_tour(a.at).total_cmp(&self.from_tour(b.at))));
        let mut ranks = BTreeMap::<[u32; 2], usize>::new();
        props
            .into_iter()
            .map(|prop| {
                let rank = ranks.entry(prop.cell).or_default();
                *rank += 1;
                (prop, *rank - 1)
            })
            .collect()
    }

    /// How many props it places, by prefab.
    pub fn prop_counts(&self) -> BTreeMap<&str, usize> {
        let mut counts = BTreeMap::new();
        for prop in &self.props {
            *counts.entry(prop.prefab.as_str()).or_default() += 1;
        }
        counts
    }
}

/// Which variant stands on each lot, and which set each slot and palette entry draws.
struct Assignment {
    /// By lot.
    variants: Vec<u32>,
    /// By cell, its palette: users' indices into `sets`.
    palettes: BTreeMap<[u32; 2], Vec<usize>>,
    /// Each user's set: the variants' slots (variant × slot), then the palettes' entries, those whose
    /// nearest use is nearest the tour first.
    sets: Vec<u32>,
}

impl Assignment {
    /// `slot_counts` is each prop prefab's slots, by stem.
    fn new(
        plan: &Plan,
        buildings: &Buildings,
        palette: u32,
        sets: u32,
        slot_counts: &BTreeMap<String, usize>,
    ) -> Result<Self, String> {
        // Lots nearest the tour first, each the next variant of its footprint round: variant `n` has
        // footprint `n % footprints`.
        let variants = buildings.variants;
        let footprints = buildings.footprints.len();
        let mut order: Vec<usize> = (0..plan.lots.len()).collect();
        let distances: Vec<f32> = plan.lots.iter().map(|lot| plan.from_tour(lot.middle)).collect();
        order.sort_by(|&a, &b| distances[a].total_cmp(&distances[b]).then(a.cmp(&b)));
        let mut by_footprint: Vec<Vec<u32>> = vec![Vec::new(); footprints];
        for variant in 0..variants {
            by_footprint[variant as usize % footprints].push(variant);
        }
        let mut next = vec![0usize; footprints];
        let mut assigned = vec![0u32; plan.lots.len()];
        for lot in order {
            let footprint = plan.lots[lot].footprint;
            let list = &by_footprint[footprint];
            if list.is_empty() {
                return Err(format!("no building variant has footprint {footprint}"));
            }
            assigned[lot] = list[next[footprint] % list.len()];
            next[footprint] += 1;
        }

        // Each palette entry's nearest use to the tour: a cell's ground draws its first entry, and is seen
        // from anywhere in a cell the tour crosses; a prop of rank `r` in its cell draws entries `r`, `r + 1`
        // and on, a slot each.
        let palette = palette as usize;
        let tour = plan.tour_cells();
        let mut nearest = BTreeMap::new();
        for cell in plan.cells() {
            let ground = if tour.contains(&cell) { 0.0 } else { plan.from_tour(plan.cell_middle(cell)) };
            nearest.insert((cell, 0), ground);
            for entry in 1..palette {
                nearest.insert((cell, entry), f32::INFINITY);
            }
        }
        for (prop, rank) in plan.ordered_props() {
            let away = plan.from_tour(prop.at);
            for slot in 0..slot_counts.get(&prop.prefab).copied().unwrap_or(0) {
                let near = nearest.get_mut(&(prop.cell, (rank + slot) % palette)).expect("every entry");
                *near = away.min(*near);
            }
        }
        // Sets go to the variants' slots, then to palette entries nearest the tour first.
        let mut entries: Vec<(([u32; 2], usize), f32)> = nearest.into_iter().collect();
        entries.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let slots = variants as usize * BUILDING_SLOTS.len();
        let mut palettes: BTreeMap<[u32; 2], Vec<usize>> =
            plan.cells().into_iter().map(|cell| (cell, vec![0; palette])).collect();
        for (k, ((cell, entry), _)) in entries.iter().enumerate() {
            palettes.get_mut(cell).expect("every cell")[*entry] = slots + k;
        }
        let seen = slots + entries.iter().filter(|(_, away)| *away <= SEEN).count();
        if seen < sets as usize {
            return Err(format!(
                "the tour passes {seen} slots and palette entries, fewer than the {sets} texture sets, so it \
                 wouldn't see them all: more building variants, a larger palette, or fewer sets"
            ));
        }
        let users = slots + entries.len();
        Ok(Self {
            variants: assigned,
            palettes,
            sets: (0..users).map(|user| (user % sets as usize) as u32).collect(),
        })
    }
}

/// Building variant `variant`: footprint `n % footprints`, storeys `n % storeys` and roof `n % roofs` of the
/// document's lists. Each storey above the first stands 0.8 m in over a skirt roof, as the look-dev town's
/// shophouses do; posts divide each side into bays of about 2.2 m, each with a window, the front's middle
/// bay a door on the ground and, on some variants, a moon window above; two lanterns hang under the
/// lowest eave.
fn building(buildings: &Buildings, variant: u32) -> Result<Assembly, String> {
    let n = variant as usize;
    let [width, depth] = buildings.footprints[n % buildings.footprints.len()];
    let storeys = buildings.storeys[n % buildings.storeys.len()];
    let kind = buildings.roofs[n % buildings.roofs.len()];
    let height = buildings.storey_height;
    let style = noise::hash(variant ^ 0x6275_696c);
    let mut place = Vec::new();
    let mut add = |shape: Shape, at: [f32; 3]| -> Result<(), String> {
        shape.validate()?;
        place.push(Piece {
            piece: String::new(),
            shape: Some(shape),
            at,
            turn: 0.0,
            scale: Scale::default(),
            variant: None,
            slot: None,
        });
        Ok(())
    };
    let step = |storey: u32| storey as f32 * 0.8;
    for storey in 0..storeys {
        let size = [width - step(storey), depth - step(storey)];
        let ground = storey == 0;
        let bays = [((size[0] / 2.2).floor() as u32).max(1), ((size[1] / 2.4).floor() as u32).max(1)];
        let mut openings = Vec::new();
        for (side, count, extent) in [
            (Side::Front, bays[0], size[0]),
            (Side::Back, bays[0], size[0]),
            (Side::Left, bays[1], size[1]),
            (Side::Right, bays[1], size[1]),
        ] {
            let bay = extent / count as f32;
            // Clear of the posts either side by a margin the shapes' checks accept.
            let wide = (bay - 0.8).min(1.2);
            if wide < 0.5 {
                continue;
            }
            for k in 0..count {
                let middle = -extent * 0.5 + (k as f32 + 0.5) * bay;
                let front = side == Side::Front && k == count / 2;
                let opening = if ground && front {
                    Opening {
                        side,
                        kind: OpeningKind::Door,
                        at: [middle, 0.0],
                        size: [wide, 2.3],
                        lattice: [0, 0],
                    }
                } else if front && style.is_multiple_of(3) {
                    let round = wide.min(1.1);
                    Opening {
                        side,
                        kind: OpeningKind::Moon,
                        at: [middle, 0.9],
                        size: [round, round],
                        lattice: [3, 3],
                    }
                } else {
                    let lattice = if style.is_multiple_of(2) { [3, 2] } else { [2, 3] };
                    Opening { side, kind: OpeningKind::Window, at: [middle, 1.0], size: [wide, 1.2], lattice }
                };
                openings.push(opening);
            }
        }
        let plinth = if ground {
            Plinth { height: 0.45, proud: 0.06 }
        } else {
            Plinth { height: 0.0, ..Plinth::default() }
        };
        let hall = Hall {
            size: [size[0], size[1], height],
            plinth,
            posts: Some(Posts { width: 0.22, proud: 0.04, bays }),
            beams: vec![Beam { at: height, height: 0.26, proud: 0.07 }],
            openings,
            ..Hall::default()
        };
        add(Shape::Hall(hall), [0.0, storey as f32 * height, 0.0])?;
        if storey + 1 < storeys {
            // Its run is its overhang and the next storey's step in: 0.6 + 0.4.
            let skirt = Roof {
                size,
                kind: RoofKind::Skirt(1.0),
                rise: 0.8,
                overhang: 0.6,
                sweep: 0.2,
                sweep_reach: 1.4,
                ..Roof::default()
            };
            add(Shape::Roof(skirt), [0.0, (storey + 1) as f32 * height, 0.0])?;
        }
    }
    let top = [width - step(storeys - 1), depth - step(storeys - 1)];
    let roof = Roof {
        size: top,
        kind,
        rise: top[1] * 0.38,
        overhang: 0.7,
        barge: 0.4,
        sweep: 0.25,
        ..Roof::default()
    };
    add(Shape::Roof(roof), [0.0, storeys as f32 * height, 0.0])?;
    for x in [-(width * 0.5 - 0.7), width * 0.5 - 0.7] {
        let lantern = Lantern { size: [0.36, 0.44], mount: Mount::Hang(0.3), ..Lantern::default() };
        add(Shape::Lantern(lantern), [x, height - 0.3, depth * 0.5 + 0.45])?;
    }
    let slots = [
        ("wall", 3.2, Projection::Planar),
        ("stone", 2.0, Projection::Box),
        ("roof", 2.0, Projection::Planar),
        ("wood", 1.5, Projection::Planar),
        ("paper", 1.0, Projection::Planar),
    ];
    Ok(Assembly {
        kits: BTreeMap::new(),
        palette: Default::default(),
        slots: slots
            .into_iter()
            .map(|(name, tile, projection)| SlotSpec { name: name.to_owned(), tile: [tile; 2], projection })
            .collect(),
        surfaces: surfaces(&BUILDING_PARTS),
        occlusion: Occlusion { ground: 0.35, ground_height: 2.5, gradient: 0.3 },
        groups: BTreeMap::new(),
        place,
        allow_coplanar: Vec::new(),
    })
}

/// Every shape part drawn in a slot, as rules.
fn surfaces(parts: &[(&str, &str)]) -> Vec<SurfaceRule> {
    parts
        .iter()
        .map(|&(part, slot)| SurfaceRule {
            pieces: vec!["#*".to_owned()],
            cell: None,
            material: Some(part.to_owned()),
            variant: None,
            slot: slot.to_owned(),
        })
        .collect()
}

/// Which of a building's slots each shape part draws in.
const BUILDING_PARTS: [(&str, &str); 24] = [
    ("plaster", "wall"),
    ("gable", "wall"),
    ("plinth", "stone"),
    ("trim", "stone"),
    ("tiles", "roof"),
    ("roof_tile", "roof"),
    ("ridge", "roof"),
    ("eaves", "roof"),
    ("timber", "wood"),
    ("lattice", "wood"),
    ("door", "wood"),
    ("boards", "wood"),
    ("back", "wood"),
    ("frame", "wood"),
    ("sign", "wood"),
    ("window", "paper"),
    ("paper", "paper"),
    ("cloth", "paper"),
    ("valance", "paper"),
    ("wicker", "stone"),
    ("pottery", "stone"),
    ("iron", "stone"),
    ("produce", "stone"),
    ("fruit", "stone"),
];

/// Which of a prop's slots each shape part draws in, as the committed props' rules say.
const PROP_PARTS: [(&str, &str); 24] = [
    ("timber", "wood"),
    ("lattice", "wood"),
    ("door", "wood"),
    ("boards", "wood"),
    ("back", "wood"),
    ("frame", "wood"),
    ("sign", "wood"),
    ("plaster", "wood"),
    ("gable", "wood"),
    ("plinth", "stone"),
    ("trim", "stone"),
    ("tiles", "stone"),
    ("roof_tile", "stone"),
    ("ridge", "stone"),
    ("eaves", "stone"),
    ("iron", "stone"),
    ("pottery", "stone"),
    ("window", "cloth"),
    ("paper", "cloth"),
    ("cloth", "cloth"),
    ("valance", "cloth"),
    ("wicker", "cloth"),
    ("produce", "cloth"),
    ("fruit", "cloth"),
];

/// The slots of the props the district makes itself: the committed props' three.
fn prop_slots() -> Vec<String> {
    ["wood", "stone", "cloth"].map(str::to_owned).to_vec()
}

fn prefab_path(stem: &str) -> String {
    format!("prefabs/{stem}{}", assembly::SOURCE_SUFFIX)
}

/// Building variant `variant`'s source, in the part's output.
pub fn building_path(variant: u32) -> String {
    format!("buildings/building_{variant:02}{}", assembly::SOURCE_SUFFIX)
}

/// Writes `scale.texture_sets` sets of maps and their materials into `sources`, varied from the families in
/// `dir`, and returns the materials' IDs, by set.
fn texture_sets(
    dir: &Path,
    scale: DistrictScale,
    sources: &mut Vec<Source>,
) -> Result<Vec<AssetId>, BenchError> {
    let mut families = Vec::new();
    for (stem, bytes) in files_ending(dir, procedural::SOURCE_SUFFIX)? {
        let path = dir.join(format!("{stem}{}", procedural::SOURCE_SUFFIX));
        let family = ProceduralTexture::read(&bytes)
            .map_err(|error| BenchError::Invalid { path: path.clone(), why: format!("{error:#}") })?;
        families.push((path, family));
    }
    if families.is_empty() {
        return Err(BenchError::Invalid {
            path: dir.to_owned(),
            why: "holds no surface (`.ptex.ron`)".to_owned(),
        });
    }
    let mut materials = Vec::new();
    for set in 0..scale.texture_sets {
        let (path, family) = &families[set as usize % families.len()];
        let surface = vary(family, set, scale.texture_size);
        surface.check().map_err(|error| BenchError::Invalid {
            path: path.clone(),
            why: format!("at {} texels: {error}", scale.texture_size),
        })?;
        let text = document_bytes(&surface);
        let name = format!("set_{set:03}");
        let maps = SurfaceMap::ALL.map(|map| {
            let path = format!("textures/{name}.{}.ptex.ron", map_name(map));
            let id = asset_id(PART, &path);
            let settings = ImportSettings::ProceduralTexture(ProceduralSettings {
                map,
                max_size: set_cap(scale.texture_size),
            });
            sources.push(Source::asset(path, text.clone(), settings));
            id
        });
        let material = set_material(maps);
        material
            .validate()
            .map_err(|error| BenchError::Invalid { path: path.clone(), why: format!("{error:#}") })?;
        let path = format!("materials/{name}.material.ron");
        materials.push(asset_id(PART, &path));
        sources.push(Source::asset(
            path,
            document_bytes(&material),
            ImportSettings::Material(MaterialSettings::default()),
        ));
    }
    Ok(materials)
}

/// The longest side a set's maps store on the mobile cook (M5 M1): one level below the budget's 2K, the
/// level whose bytes over the budget's sets come nearest the mobile texture pool (spec §3.4: 600 MB
/// against the desktop's 2.5 GB, which the budget fills).
pub const MOBILE_SET_SIZE: u32 = 1024;

/// The cap of a set at `size` texels: [`MOBILE_SET_SIZE`] on mobile, written only when the set is over it.
fn set_cap(size: u32) -> MaxSize {
    MaxSize { mobile: NonZeroU32::new(MOBILE_SET_SIZE).filter(|cap| size > cap.get()), ..MaxSize::NONE }
}

fn map_name(map: SurfaceMap) -> &'static str {
    match map {
        SurfaceMap::Color => "color",
        SurfaceMap::Normal => "normal",
        SurfaceMap::Mask => "mask",
    }
}

/// Set `set` of `family`: its seed and colours its own, at `size` texels.
fn vary(family: &ProceduralTexture, set: u32, size: u32) -> ProceduralTexture {
    let seed = noise::hash(set ^ noise::hash(family.seed));
    // Each set shifts the family's colours by up to 12 levels a channel, the same way at every stop.
    let shift: [i32; 3] =
        std::array::from_fn(|c| (noise::hash(seed.wrapping_add(c as u32)) % 25) as i32 - 12);
    let tint = |SrgbColor(rgba): SrgbColor| {
        SrgbColor(std::array::from_fn(|c| {
            if c < 3 { (i32::from(rgba[c]) + shift[c]).clamp(0, 255) as u8 } else { rgba[c] }
        }))
    };
    ProceduralTexture {
        size,
        seed,
        colors: family.colors.iter().map(|&(tone, color)| (tone, tint(color))).collect(),
        joint_color: family.joint_color.map(tint),
        ..family.clone()
    }
}

/// An environment material reading a set's maps: its colour, normal and mask.
fn set_material([color, normal, mask]: [AssetId; 3]) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(color),
        normal_map: Some(normal),
        mask_map: Some(mask),
        roughness: 1.0,
        roughness_range: [0.4, 1.0],
        specular_clamp: 0.25,
        wrap: 0.4,
        terminator: Terminator { width: 0.06, growth: 0.008, band: None },
        distance: DistanceStylization { start: 15.0, end: 140.0, desaturation: 0.5 },
        ..EnvironmentMaterial::default()
    }
}

/// One cell's ground: a square `cell_size` on a side in one slot, which tiles every 4 m.
fn ground_prefab(cell_size: f32) -> Vec<u8> {
    format!(
        "// A cell's ground, generated by the benchmark's district (`sapphire_pipeline::bench::district`).
(
    type: \"sapphire::pipeline::Assembly\",
    version: 1,
    data: (
        kits: {{}},
        slots: [(name: \"ground\", tile: (4.0, 4.0))],
        surfaces: [],
        place: [(piece: \"#quad\", scale: {cell_size:?}, slot: Some(\"ground\"))],
    ),
)
"
    )
    .into_bytes()
}

/// A wall across a lane, as wide as the district's lanes and too tall to see over, in the props' three slots.
fn closure_prefab(district: &BenchDistrict) -> Result<Vec<u8>, String> {
    let wall = Wall {
        size: [district.streets.lane, 4.2, 0.6],
        plinth: Plinth { height: 0.5, proud: 0.05 },
        ..Wall::default()
    };
    let shape = Shape::Wall(wall);
    shape.validate()?;
    let slots =
        [("wood", 1.5, Projection::Planar), ("stone", 2.0, Projection::Box), ("cloth", 1.0, Projection::Box)];
    let closure = Assembly {
        kits: BTreeMap::new(),
        palette: Default::default(),
        slots: slots
            .into_iter()
            .map(|(name, tile, projection)| SlotSpec { name: name.to_owned(), tile: [tile; 2], projection })
            .collect(),
        surfaces: surfaces(&PROP_PARTS),
        occlusion: Occlusion { ground: 0.35, ground_height: 2.5, gradient: 0.3 },
        groups: BTreeMap::new(),
        place: vec![Piece {
            piece: String::new(),
            shape: Some(shape),
            at: [0.0; 3],
            turn: 0.0,
            scale: Scale::default(),
            variant: None,
            slot: None,
        }],
        allow_coplanar: Vec::new(),
    };
    Ok(document_bytes(&closure))
}

/// A 32-bit seed from several values, each hashed into the last.
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

/// How far `at` is from the segment from `a` to `b`.
fn from_segment(at: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let along = [b[0] - a[0], b[1] - a[1]];
    let length = along[0] * along[0] + along[1] * along[1];
    let t = if length == 0.0 {
        0.0
    } else {
        (((at[0] - a[0]) * along[0] + (at[1] - a[1]) * along[1]) / length).clamp(0.0, 1.0)
    };
    distance(at, [a[0] + along[0] * t, a[1] + along[1] * t])
}

/// What's been placed, as rectangles in 8 m buckets, so a prop can ask whether its room is free.
struct Taken {
    half: f32,
    side: usize,
    buckets: Vec<Vec<Rect>>,
}

impl Taken {
    const BUCKET: f32 = 8.0;

    fn new(half: f32) -> Self {
        let side = (half * 2.0 / Self::BUCKET).ceil() as usize + 1;
        Self { half, side, buckets: vec![Vec::new(); side * side] }
    }

    fn range(&self, rect: &Rect, axis: usize) -> std::ops::RangeInclusive<usize> {
        let index = |v: f32| (((v + self.half) / Self::BUCKET).floor().max(0.0) as usize).min(self.side - 1);
        index(rect.min[axis])..=index(rect.max[axis])
    }

    fn free(&self, rect: &Rect) -> bool {
        self.range(rect, 1).all(|z| {
            self.range(rect, 0)
                .all(|x| self.buckets[z * self.side + x].iter().all(|taken| !taken.overlaps(rect)))
        })
    }

    fn take(&mut self, rect: Rect) {
        for z in self.range(&rect, 1) {
            for x in self.range(&rect, 0) {
                self.buckets[z * self.side + x].push(rect);
            }
        }
    }
}

/// The files in `dir` whose names end with `suffix`, by stem, in name order.
fn files_ending(dir: &Path, suffix: &str) -> Result<Vec<(String, Vec<u8>)>, BenchError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        if let Some(stem) = name.strip_suffix(suffix).filter(|stem| !stem.is_empty()) {
            files.push((stem.to_owned(), fs::read(&path)?));
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests;
