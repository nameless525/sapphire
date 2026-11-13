//! The benchmark's district (M5 B1's placeholder; M5 B2 builds the district on it): a zone of cells, each
//! holding the same block of prefabs, drawn in texture sets rasterized from procedural specs.
//!
//! `bench/district/` holds:
//!
//! | File | What |
//! |---|---|
//! | `district.ron` | [`BenchDistrict`]: the grid, the block each cell holds, and each preset's scale |
//! | `prefabs/<name>.assembly.ron` | The prefabs the block places, by file stem: parametric shapes only, since no kit is under `bench/` |
//! | `surfaces/<name>.ptex.ron` | Procedural texture specs ([`crate::procedural`]), the families texture sets are varied from |
//!
//! It generates, under `district/` in the output:
//!
//! - `textures/set_NNN.{color,normal,mask}.ptex.ron`: a preset's `texture_sets` sets, set `n` from the
//!   `n`th surface family round, at the preset's `texture_size`, with a seed and colours of its own;
//! - `materials/set_NNN.material.ron`: an environment material reading set `n`'s three maps;
//! - `prefabs/`: the committed prefabs, and `ground.assembly.ron`, one cell's ground;
//! - `district.zone.ron`: `cells` × `cells` cells around the origin, each with its ground and the block,
//!   every placement's slots drawn in the next sets round, so the zone reaches every set, and a
//!   `player_start` spawner at the origin.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_assets::AssetId;
use sapphire_assets::material::{DistanceStylization, EnvironmentMaterial, MaterialDocument, Terminator};
use sapphire_reflect::Versioned;
use sapphire_zone::document::{Placement, Spawner, ZoneDocument};
use sapphire_zone::grid::Grid;
use serde::{Deserialize, Serialize};

use super::{BenchError, Preset, Presets, Source, asset_id, document_bytes, read_document};
use crate::assembly;
use crate::character_maps::ramp::SrgbColor;
use crate::material::MaterialSettings;
use crate::meta::{ImportSettings, MeshSettings};
use crate::procedural::{self, ProceduralSettings, ProceduralTexture, SurfaceMap};
use crate::samples::noise;

/// This part's folder, under `bench/` and in the output.
pub const PART: &str = "district";

/// The district's document, `bench/district/district.ron` (`sapphire::pipeline::BenchDistrict`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct BenchDistrict {
    /// A cell's side, in metres: the zone's grid, and the ground each cell has.
    pub cell_size: f32,
    /// What each cell holds, from its corner toward +X and +Z.
    pub block: Vec<BlockItem>,
    pub presets: Presets<DistrictScale>,
}

impl Versioned for BenchDistrict {
    /// 1: M5 B1.
    const VERSION: u32 = 1;
}

/// A prefab in the block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockItem {
    /// A prefab in `prefabs/`, by its file's stem.
    pub prefab: String,
    pub at: [f32; 3],
    /// Degrees about +Y.
    #[serde(default)]
    pub turn: f32,
}

/// What a preset makes of the district.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistrictScale {
    /// Cells along each side.
    pub cells: u32,
    pub texture_sets: u32,
    /// Texels on a side of every map.
    pub texture_size: u32,
}

/// The most cells along a side: a zone's grid holds far more, but a benchmark this big would be a mistake.
const MAX_CELLS: u32 = 64;
/// The most texture sets: twice the desktop pool's.
const MAX_SETS: u32 = 1000;

/// Generates the district from the documents in `dir` (`bench/district/`) at `preset`'s scale.
pub fn generate(dir: &Path, preset: Preset) -> Result<Vec<Source>, BenchError> {
    let document_path = dir.join("district.ron");
    let district: BenchDistrict = read_document(&document_path)?;
    let scale = *district.presets.get(preset);
    let invalid = |why: String| BenchError::Invalid { path: document_path.clone(), why };
    if !(1..=MAX_CELLS).contains(&scale.cells) || !(1..=MAX_SETS).contains(&scale.texture_sets) {
        return Err(invalid(format!(
            "{preset}: 1 to {MAX_CELLS} cells a side and 1 to {MAX_SETS} texture sets"
        )));
    }
    if !(district.cell_size.is_finite() && district.cell_size >= 1.0) {
        return Err(invalid("`cell_size` is at least a metre".to_owned()));
    }

    let mut sources = Vec::new();
    let materials = texture_sets(&dir.join("surfaces"), scale, &mut sources)?;

    // The committed prefabs, each slot bound to a set in turn by default, and the ground.
    let mut prefabs = BTreeMap::new();
    for (stem, bytes) in files_ending(&dir.join("prefabs"), assembly::SOURCE_SUFFIX)? {
        prefabs.insert(stem, bytes);
    }
    let ground = ground_prefab(district.cell_size);
    if prefabs.insert("ground".to_owned(), ground).is_some() {
        return Err(invalid("`prefabs/ground.assembly.ron` is the district's to make: rename it".to_owned()));
    }
    let mut slots = BTreeMap::new();
    for (stem, bytes) in &prefabs {
        let read = assembly::read(bytes).map_err(|error| BenchError::Invalid {
            path: dir.join("prefabs").join(format!("{stem}{}", assembly::SOURCE_SUFFIX)),
            why: format!("{error:#}"),
        })?;
        let names: Vec<String> = read.slots.into_iter().map(|slot| slot.name).collect();
        let bound = names.iter().enumerate().map(|(i, name)| (name.clone(), materials[i % materials.len()]));
        let settings = MeshSettings { materials: bound.collect(), ..MeshSettings::default() };
        let path = prefab_path(stem);
        sources.push(Source::uncooked(path, bytes.clone(), ImportSettings::Assembly(settings)));
        slots.insert(stem.clone(), names);
    }
    for item in &district.block {
        if !slots.contains_key(&item.prefab) {
            return Err(invalid(format!(
                "the block places `{}`, which `prefabs/` doesn't hold",
                item.prefab
            )));
        }
    }

    // The zone: each cell's ground and block, every slot in the next set round.
    let mut next = 0usize;
    let mut placement = |prefab: &str, at: [f32; 3], turn: f32| {
        let bound = slots[prefab]
            .iter()
            .map(|slot| {
                next += 1;
                (slot.clone(), materials[(next - 1) % materials.len()])
            })
            .collect();
        Placement {
            prefab: asset_id(PART, &prefab_path(prefab)),
            at,
            turn,
            scale: [1.0; 3],
            materials: bound,
            backdrop: false,
            split: None,
            colliders: Vec::new(),
        }
    };
    let size = district.cell_size;
    let first = -(scale.cells as f32) * size * 0.5;
    let mut placements = Vec::new();
    for z in 0..scale.cells {
        for x in 0..scale.cells {
            let corner = [first + x as f32 * size, 0.0, first + z as f32 * size];
            placements.push(placement("ground", [corner[0] + size * 0.5, 0.0, corner[2] + size * 0.5], 0.0));
            for item in &district.block {
                let at = [corner[0] + item.at[0], corner[1] + item.at[1], corner[2] + item.at[2]];
                placements.push(placement(&item.prefab, at, item.turn));
            }
        }
    }
    let zone = ZoneDocument {
        grid: Grid { cell_size: size },
        placements,
        instances: Vec::new(),
        lights: Vec::new(),
        probes: None,
        day: None,
        rigs: None,
        // The district's look is B2's; until then it takes the app's sky, wind, weather and fog.
        sky: None,
        wind: None,
        weather: None,
        fog: None,
        colliders: None,
        spawners: vec![Spawner {
            name: "player_start".to_owned(),
            position: [0.0; 3],
            yaw_degrees: 0.0,
            spawns: "player".to_owned(),
            patrol: None,
        }],
        triggers: Vec::new(),
        checkpoints: Vec::new(),
        splines: Vec::new(),
        navigation: None,
        encounters: Vec::new(),
        exits: Vec::new(),
    };
    sources.push(Source::asset(
        "district.zone.ron",
        document_bytes(&zone),
        ImportSettings::Zone(Default::default()),
    ));
    Ok(sources)
}

fn prefab_path(stem: &str) -> String {
    format!("prefabs/{stem}{}", assembly::SOURCE_SUFFIX)
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
            let settings = ImportSettings::ProceduralTexture(ProceduralSettings { map });
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
