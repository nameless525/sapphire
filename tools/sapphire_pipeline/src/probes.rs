//! Ambient probe grids (M1 W5): `.probes.ron` documents baked to `.sprobe` (spec §5.27, the
//! renderer's `ambient` module for how the grid is lit and sampled).
//!
//! A probe grid source is a RON document ([`ProbeGridDocument`]) naming what the grid sees, the
//! zone's meshes by ID, each placed where the zone puts it, and the box the probes fill, with a
//! sidecar like any asset (`importer: ProbeGrid(())`). The bake reads the meshes as they cooked,
//! the materials their slots name and those materials' color maps, and time of day's default
//! day for the keys' light, all of it part of the cache key ([`Dependency::Asset`] for every
//! mesh and material, by key, and the keys as the bake reads them, resolved: each key's hour,
//! light, shadow tint, sky radiance and emission scale by their bits, so a change to the timeline
//! or to the code that resolves it, time of day's evaluation, its key light, the celestial sphere
//! or `sky_radiance`, bakes again): a changed mesh, material, map or key bakes the grid again,
//! and an unchanged tree takes it from the cache.
//!
//! # The bake ([`bake::bake`])
//!
//! On the CPU, on every core, deterministic: scalar `f32` arithmetic in a fixed order, a ray
//! set shared by every probe, turned per probe by its index, and every result written to its own
//! slot, so the same inputs give the same bytes on every machine.
//!
//! 1. **Bricks.** The box is cut into bricks of 4³ cells, [`ProbeGridDocument::spacing`] apart. A
//!    brick is kept when any triangle passes through its box grown by one cell on every side, so
//!    every surface has probes around it, on both sides of a thin wall, and the empty air above
//!    and the solid ground below get none. Kept bricks list their 5³ probes, each stored once.
//! 2. **Surfels** carry light between surfaces: points spread over every triangle within 6 m of
//!    the box, [`ProbeGridDocument::surfel_spacing`] apart. Each casts rays over its hemisphere
//!    for its own sky visibility and to find the surfels it sees.
//! 3. **Direct light**, per time-of-day key: the key's sun or moon through W4's environment
//!    shading (wrapped Lambert on the lit side, a shadow ray, and in shadow the key's shadow tint
//!    times the material's kept), plus the key's sky through the surfel's visibility, times the
//!    albedo, plus emission (the lanterns light their surroundings), as much of it as the key's
//!    emission scale shows (M1 W1's keyed emission) but for the material's steady share, as the
//!    renderer draws it: a lantern dark by day lights nothing in the day's keys. The sky is
//!    `sapphire_render::ambient::sky_radiance`: the key's gradient at its authored ambient
//!    strength, as the renderer relights it.
//! 4. **Bounces:** [`ProbeGridDocument::bounces`] − 1 passes in which every surfel adds the light
//!    the surfels it sees sent in the last pass, times its albedo. One bounce is surfaces lit
//!    directly; two adds light that went from one surface to another first.
//! 5. **Probes** cast [`ProbeGridDocument::rays`] rays over the sphere. Rays that escape make the
//!    sky visibility (L1 or L2 SH, [`ProbeGridDocument::visibility`]); rays that hit a surfel
//!    record its light under every key, projected per channel to L1 and convolved to irradiance:
//!    the bounce, times [`ProbeGridDocument::bounce_strength`]. Six more rays along the axes
//!    measure how far the probe sees, for the renderer's leak test.
//! 6. **Probes inside geometry** see backfaces (more than a tenth of their rays): they take the
//!    mean of their open neighbors and stay marked invalid.
//!
//! Hits outside the surfels' reach (the far hills) are lit directly where they are hit, under the
//! open sky.
//!
//! # What the bake sees of each material
//!
//! - **Fog cards** (M1 W3) not at all: transparent fog neither hides the sky nor bounces light.
//! - **Foliage's alpha-tested cards** (M1 W6) as the renderer draws them, never as solid quads:
//!   a ray reaching a card reads its coverage (the color map's alpha times the base color's) at
//!   the hit's texture coordinates, from the map's nearest texel, and passes below the cutoff,
//!   so a canopy hides the sky and bounces its leaves' color through its leaves alone, and its
//!   shadow rays cut out as its shadow maps do. Cards are two-sided, as they are drawn: a ray
//!   meeting a card's back meets leaves, not the inside of geometry, and the leak test's rays
//!   pass them, since a leaf is no wall between two probes. Surfels sit only where a card is
//!   drawn, facing the side its volume normal does. Their color is the covered texels' mean
//!   through the middle of the color gradient. A material nothing of which is drawn bakes as
//!   none.
//! - **Everything else** solid, by its base color times its color map's mean.

pub mod bake;
mod bvh;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use glam::{Quat, Vec2, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::material::{AnyMaterialDocument, FoliageMaterial, MaterialFamily};
use sapphire_assets::mesh::{Mesh, SlotMaterial};
use sapphire_assets::probes::{ProbeGrid, ProbeGridError};
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use sapphire_world::tod::TodTimeline;
use serde::{Deserialize, Serialize};

use crate::cache::{CacheKey, Dependency, Stage};
use crate::cook::Cook;
use crate::meta::ImportSettings;
use crate::texture::TextureClass;
use bake::{BakeError, BakeKey, BakeMaterial, BakeScene, BakeSettings, BakeTriangle, Cutout};

/// Reading the document, as cache keys know it. Bump the version whenever its output changes
/// for the same file.
pub const IMPORTER: Stage = Stage { name: "probe grid document", version: 1 };

/// The bake and the `.sprobe` writer. Bump the version whenever they change output for the same
/// document and inputs.
pub const COOKER: Stage = Stage { name: "sprobe bake", version: 2 };

/// What a probe grid source's file name ends with.
pub const SOURCE_SUFFIX: &str = ".probes.ron";

/// The default day's timeline as a dependency of every grid baked under it.
/// The resolved keys' place among the cache key's dependencies: hashed, not read from a file.
const KEYS_DEPENDENCY: &str = "resolved time-of-day keys";

/// Import settings for probe grids: none yet. A struct, so settings can join without changing
/// existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProbeGridSettings {}

/// A probe grid document (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct ProbeGridDocument {
    /// The world-space box the probes fill: the zone's playable space.
    pub bounds: BakeBounds,
    /// Meters between probes, per cook: phones get fewer (spec §5.27).
    pub spacing: PerCook<f32>,
    /// The sky visibility's SH order, per cook.
    pub visibility: PerCook<ShOrder>,
    /// Rays each probe casts, per cook.
    pub rays: PerCook<u32>,
    /// Rays each surfel casts over its hemisphere.
    pub surfel_rays: u32,
    /// Meters between surfels.
    pub surfel_spacing: f32,
    /// Light bounces between surfaces: 1 is surfaces lit directly, 2 adds a bounce between them.
    pub bounces: u32,
    /// Scale on the bounce light: 1 is what the bake computes, lower keeps shade darker.
    pub bounce_strength: f32,
    /// The time of day the bounce is baked under.
    pub timeline: BakeTimeline,
    /// What the grid sees.
    pub meshes: Vec<BakeMesh>,
}

impl Versioned for ProbeGridDocument {
    /// 1: M1 W5.
    const VERSION: u32 = 1;
}

/// A world-space box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakeBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// A value per cook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerCook<T> {
    pub desktop: T,
    pub mobile: T,
}

impl<T: Copy> PerCook<T> {
    pub fn get(&self, cook: Cook) -> T {
        match cook {
            Cook::Desktop => self.desktop,
            Cook::Mobile => self.mobile,
        }
    }
}

/// How many SH bands sky visibility keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShOrder {
    /// 4 coefficients: the direction the sky is open toward and how much.
    L1,
    /// 9: also the shape of the opening (a slot of sky between two walls, a window).
    L2,
}

impl ShOrder {
    fn coefficients(self) -> u32 {
        match self {
            Self::L1 => 4,
            Self::L2 => 9,
        }
    }
}

/// The time of day a grid's bounce is baked under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BakeTimeline {
    /// `sapphire_world`'s default day, every key of it.
    DefaultDay,
}

/// A mesh the grid sees, placed in the zone.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakeMesh {
    pub mesh: AssetId,
    /// Meters.
    #[serde(default)]
    pub translation: [f32; 3],
    /// A turn about +Y, in degrees, before the translation.
    #[serde(default)]
    pub yaw: f32,
}

/// What the probe cook needs of another asset of the content tree.
#[derive(Clone, Copy, Debug)]
pub struct ProbeInput<'a> {
    pub source: &'a Path,
    pub settings: &'a ImportSettings,
    /// Its cooked file and key, once cooked.
    pub cooked: Option<(&'a Path, CacheKey)>,
}

/// Errors from cooking a probe grid.
#[derive(Debug, thiserror::Error)]
pub enum ProbeCookError {
    #[error("the document isn't UTF-8")]
    NotUtf8(#[from] std::str::Utf8Error),
    #[error("not a probe grid document")]
    Document(#[source] Box<DocumentError>),
    #[error("{id} is not a mesh of this content tree, cooked before the grid")]
    NotAMesh { id: AssetId },
    #[error("failed to read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cooked mesh {} doesn't read", path.display())]
    Mesh {
        path: PathBuf,
        #[source]
        source: sapphire_assets::mesh::MeshError,
    },
    #[error("{} is not a material document", path.display())]
    Material {
        path: PathBuf,
        #[source]
        source: Box<DocumentError>,
    },
    #[error("color map {} doesn't decode", path.display())]
    Image {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error(transparent)]
    Bake(#[from] BakeError),
    #[error("the baked grid is invalid")]
    Invalid(#[from] ProbeGridError),
}

/// Whether `path` names a probe grid source.
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(SOURCE_SUFFIX))
}

/// The cooked file a probe grid source at `relative` writes: `courtyard.probes.ron` →
/// `courtyard.sprobe`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// Reads a probe grid document.
pub fn read(bytes: &[u8]) -> Result<ProbeGridDocument, ProbeCookError> {
    sapphire_serialize::from_ron(std::str::from_utf8(bytes)?)
        .map_err(|error| ProbeCookError::Document(Box::new(error)))
}

/// Everything besides the document that baking it reads: every mesh and every material their
/// slots name, by their keys, and the timeline. Checks that each mesh is one of the tree's,
/// cooked.
pub fn dependencies<'a>(
    document: &ProbeGridDocument,
    input: impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Vec<Dependency>, ProbeCookError> {
    let mut dependencies = Vec::new();
    for placed in &document.meshes {
        let (path, key) = cooked_mesh(placed.mesh, &input)?;
        dependencies.push(Dependency::Asset { id: placed.mesh, key });
        for id in material_ids(&read_mesh(path)?) {
            if let Some((_, key)) = input(id).and_then(|material| material.cooked) {
                dependencies.push(Dependency::Asset { id, key });
            }
        }
    }
    dependencies.push(Dependency::File {
        path: KEYS_DEPENDENCY.to_owned(),
        hash: keys_hash(&keys(document.timeline)),
    });
    dependencies.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    dependencies.dedup();
    Ok(dependencies)
}

/// Bakes `document` for `cook`: the cooked grid's bytes, read back through the runtime's checks.
pub fn cook<'a>(
    document: &ProbeGridDocument,
    cook: Cook,
    input: impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Vec<u8>, ProbeCookError> {
    let scene = scene(document, &input)?;
    let keys = keys(document.timeline);
    let grid = bake::bake(&scene, &keys, &settings(document, cook))?;
    let bytes = grid.to_cooked().map_err(ProbeGridError::Cooked)?;
    ProbeGrid::read(&bytes)?;
    Ok(bytes.into_vec())
}

/// The bake's settings for `cook`.
pub fn settings(document: &ProbeGridDocument, cook: Cook) -> BakeSettings {
    BakeSettings {
        min: Vec3::from(document.bounds.min),
        max: Vec3::from(document.bounds.max),
        spacing: document.spacing.get(cook),
        visibility_coefficients: document.visibility.get(cook).coefficients(),
        probe_rays: document.rays.get(cook),
        surfel_rays: document.surfel_rays,
        surfel_spacing: document.surfel_spacing,
        bounces: document.bounces,
        bounce_strength: document.bounce_strength,
    }
}

/// The blake3 hash of the bits of `keys`, in order.
fn keys_hash(keys: &[BakeKey]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    for key in keys {
        let values = [key.hours]
            .into_iter()
            .chain(key.to_light.to_array())
            .chain(key.radiance.to_array())
            .chain(key.shadow_tint.to_array())
            .chain(key.sky.iter().flatten().copied())
            .chain([key.emission]);
        for value in values {
            hasher.update(&value.to_bits().to_le_bytes());
        }
    }
    hasher.finalize()
}

/// Every key of the timeline, lit as time of day lights it.
pub fn keys(timeline: BakeTimeline) -> Vec<BakeKey> {
    let timeline = match timeline {
        BakeTimeline::DefaultDay => TodTimeline::default_day(),
    };
    timeline
        .keys()
        .iter()
        .map(|key| {
            let look = timeline.evaluate(key.time);
            let light = look.key_light();
            BakeKey {
                hours: key.time,
                to_light: -light.direction.normalize_or(Vec3::NEG_Y),
                radiance: light.color * light.intensity.max(0.0),
                shadow_tint: look.shadow_tint,
                sky: sapphire_render::ambient::sky_radiance(&look.sky, &look.ambient),
                emission: look.emission,
            }
        })
        .collect()
}

fn cooked_mesh<'a>(
    id: AssetId,
    input: &impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<(&'a Path, CacheKey), ProbeCookError> {
    input(id)
        .filter(|input| input.settings.mesh().is_some())
        .and_then(|input| input.cooked)
        .ok_or(ProbeCookError::NotAMesh { id })
}

fn read_mesh(path: &Path) -> Result<Mesh, ProbeCookError> {
    let bytes =
        std::fs::read(path).map_err(|source| ProbeCookError::Read { path: path.to_owned(), source })?;
    let mut aligned = sapphire_serialize::cooked::AlignedVec::<16>::new();
    aligned.extend_from_slice(&bytes);
    Mesh::from_cooked(&aligned).map_err(|source| ProbeCookError::Mesh { path: path.to_owned(), source })
}

/// The material assets a mesh's slots name.
fn material_ids(mesh: &Mesh) -> Vec<AssetId> {
    mesh.materials
        .iter()
        .filter_map(|slot| match slot.material {
            SlotMaterial::Asset(id) => Some(id),
            SlotMaterial::Embedded(_) => None,
        })
        .collect()
}

/// The world-space triangles of every mesh, LOD0, with their slots' materials and their
/// cutouts, but those of slots the bake leaves out ([`bake_material`]: fog cards, and foliage
/// nothing of which is drawn).
fn scene<'a>(
    document: &ProbeGridDocument,
    input: &impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<BakeScene, ProbeCookError> {
    let mut scene = BakeScene::default();
    // Material assets met so far, each with its index in the scene's materials (none: left out).
    let mut known: Vec<(AssetId, Option<u32>)> = Vec::new();
    for placed in &document.meshes {
        let (path, _) = cooked_mesh(placed.mesh, input)?;
        let mesh = read_mesh(path)?;
        let rotation = Quat::from_rotation_y(placed.yaw.to_radians());
        let translation = Vec3::from(placed.translation);
        let vertices: Vec<(Vec3, Vec3, Vec2)> = (0..mesh.streams.len())
            .filter_map(|index| mesh.streams.vertex(index))
            .map(|vertex| {
                let position = rotation * Vec3::from(vertex.position) + translation;
                (position, rotation * Vec3::from(vertex.normal), Vec2::from(vertex.uv))
            })
            .collect();
        let mut slots = Vec::with_capacity(mesh.materials.len());
        for slot in &mesh.materials {
            let material = match &slot.material {
                SlotMaterial::Asset(id) => match known.iter().find(|(known, _)| known == id) {
                    Some(&(_, index)) => {
                        slots.push(index);
                        continue;
                    }
                    None => bake_material(*id, input)?,
                },
                // The importer's own materials: a mid grey, but for a fog card.
                SlotMaterial::Embedded(material) => {
                    (material.family != MaterialFamily::FogCard).then(|| (BakeMaterial::default(), None))
                }
            };
            let index = material.map(|(mut material, cutout)| {
                if let Some(cutout) = cutout {
                    material.cutout = Some(scene.cutouts.len() as u32);
                    scene.cutouts.push(cutout);
                }
                scene.materials.push(material);
                (scene.materials.len() - 1) as u32
            });
            if let SlotMaterial::Asset(id) = slot.material {
                known.push((id, index));
            }
            slots.push(index);
        }
        let lod = &mesh.lods[0];
        for submesh in &lod.submeshes {
            // A slot past the mesh's own takes the bake's default material, as the renderer's.
            let material = match slots.get(submesh.material as usize) {
                Some(&Some(index)) => index,
                Some(None) => continue,
                None => u32::MAX,
            };
            let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
            for triangle in mesh.indices[range].as_chunks::<3>().0 {
                let corners = [0, 1, 2].map(|corner| vertices[triangle[corner] as usize]);
                scene.triangles.push(BakeTriangle {
                    positions: corners.map(|(position, ..)| position),
                    normals: corners.map(|(_, normal, _)| normal),
                    uvs: corners.map(|(.., uv)| uv),
                    material,
                });
            }
        }
    }
    Ok(scene)
}

/// A material document's light response, as the bake sees it, with where it is cut out, or
/// `None` for a material the bake leaves out: fog cards (M1 W3), transparent fog that neither
/// occludes the sky nor bounces light, and foliage nothing of which is drawn. A material asset
/// the input doesn't have is a mid grey.
fn bake_material<'a>(
    id: AssetId,
    input: &impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Option<(BakeMaterial, Option<Cutout>)>, ProbeCookError> {
    let Some(source) = input(id).map(|input| input.source) else {
        return Ok(Some((BakeMaterial::default(), None)));
    };
    let bytes = std::fs::read(source)
        .map_err(|error| ProbeCookError::Read { path: source.to_owned(), source: error })?;
    let text = String::from_utf8_lossy(&bytes);
    let document = AnyMaterialDocument::from_ron(&text)
        .map_err(|error| ProbeCookError::Material { path: source.to_owned(), source: Box::new(error) })?;
    Ok(match document {
        AnyMaterialDocument::Environment(material) => {
            let base = Vec3::from_slice(&material.base_color[..3]);
            let emission = Vec3::from(material.emission);
            let material = BakeMaterial {
                albedo: base * mean_color(material.base_color_map, input)?,
                emission: emission * mean_color(material.emission_map, input)?,
                emission_steady: material.emission_steady,
                wrap: material.wrap,
                shadow_tint: Vec3::from(material.shadow_tint),
                ..BakeMaterial::default()
            };
            Some((material, None))
        }
        AnyMaterialDocument::AnimeCharacter(material) => {
            let base = Vec3::from_slice(&material.base_color[..3]);
            let albedo = base * mean_color(material.base_color_map, input)?;
            Some((BakeMaterial { albedo, ..BakeMaterial::default() }, None))
        }
        AnyMaterialDocument::Foliage(material) => foliage_material(&material, input)?,
        AnyMaterialDocument::Sky(_) => Some((BakeMaterial::default(), None)),
        AnyMaterialDocument::FogCard(_) => None,
    })
}

/// A foliage material (M1 W6) as the bake sees it: its leaves' mean color through the middle of
/// its color gradient, its wrap and shadow tint, and when alpha-tested two-sided and cut out
/// where its cards are clear (their map's alpha times the base color's against the cutoff), so
/// a card's leaves hide the sky and bounce light and its holes don't. `None` when nothing of it
/// is drawn.
fn foliage_material<'a>(
    material: &FoliageMaterial,
    input: &impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Option<(BakeMaterial, Option<Cutout>)>, ProbeCookError> {
    let gradient = 0.5 * (Vec3::from(material.gradient.root) + Vec3::from(material.gradient.tip));
    let tested = material.alpha_cutoff > 0.0;
    let lit = |albedo: Vec3| BakeMaterial {
        albedo: Vec3::from_slice(&material.base_color[..3]) * gradient * albedo,
        wrap: material.wrap,
        shadow_tint: Vec3::from(material.shadow_tint),
        two_sided: tested,
        ..BakeMaterial::default()
    };
    let alpha = material.base_color[3];
    let Some(map) = color_map(material.base_color_map, input)? else {
        // Without a map the base color's alpha covers every point or none.
        return Ok((!tested || alpha >= material.alpha_cutoff).then(|| (lit(Vec3::ONE), None)));
    };
    if !tested {
        return Ok(Some((lit(map.mean(|_| true)), None)));
    }
    let coverage: Vec<f32> = map.alpha.iter().map(|&texel| texel * alpha).collect();
    if coverage.iter().all(|&texel| texel < material.alpha_cutoff) {
        return Ok(None);
    }
    let albedo = map.mean(|texel| coverage[texel] >= material.alpha_cutoff);
    let cutout = Cutout { width: map.width, height: map.height, coverage, cutoff: material.alpha_cutoff };
    Ok(Some((lit(albedo), Some(cutout))))
}

/// A color map's texels as the bake reads them: linear color, and alpha.
struct ColorMap {
    width: u32,
    height: u32,
    color: Vec<[f32; 3]>,
    alpha: Vec<f32>,
}

impl ColorMap {
    /// The mean color of the texels `take` picks by index, or white if it picks none.
    fn mean(&self, take: impl Fn(usize) -> bool) -> Vec3 {
        let mut sum = [0.0f64; 3];
        let mut count = 0u32;
        for (_, texel) in self.color.iter().enumerate().filter(|&(index, _)| take(index)) {
            for (s, &c) in sum.iter_mut().zip(texel) {
                *s += f64::from(c);
            }
            count += 1;
        }
        if count == 0 {
            return Vec3::ONE;
        }
        Vec3::from(sum.map(|s| (s / f64::from(count)) as f32))
    }
}

/// The mean linear color of a color map, or white without one (or for a map that isn't an
/// image source, such as a generated one).
fn mean_color<'a>(
    map: Option<AssetId>,
    input: &impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Vec3, ProbeCookError> {
    Ok(color_map(map, input)?.map_or(Vec3::ONE, |map| map.mean(|_| true)))
}

/// A color map's texels, from its image source; `None` without a map or for a map that isn't
/// an image source.
fn color_map<'a>(
    map: Option<AssetId>,
    input: &impl Fn(AssetId) -> Option<ProbeInput<'a>>,
) -> Result<Option<ColorMap>, ProbeCookError> {
    let Some(texture) = map.and_then(input) else {
        return Ok(None);
    };
    let ImportSettings::Texture(settings) = texture.settings else {
        return Ok(None);
    };
    let srgb = matches!(settings.class, TextureClass::Color(_));
    let path = texture.source;
    let bytes =
        std::fs::read(path).map_err(|source| ProbeCookError::Read { path: path.to_owned(), source })?;
    let image = image::load_from_memory(&bytes)
        .map_err(|source| ProbeCookError::Image { path: path.to_owned(), source })?
        .to_rgba8();
    let mut color = Vec::with_capacity(image.pixels().len());
    let mut alpha = Vec::with_capacity(image.pixels().len());
    for pixel in image.pixels() {
        let linear = if srgb {
            crate::color::srgb8(pixel[0], pixel[1], pixel[2])
        } else {
            [pixel[0], pixel[1], pixel[2], 255].map(|c| f32::from(c) / 255.0)
        };
        color.push([linear[0], linear[1], linear[2]]);
        alpha.push(f32::from(pixel[3]) / 255.0);
    }
    Ok(Some(ColorMap { width: image.width(), height: image.height(), color, alpha }))
}
