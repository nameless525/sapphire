//! Figures (M3 C2): skinned models in the anime character family, built from a spec rather than
//! from a model, and written as ordinary sources: a `.glb` (or a `.vrm`) and the maps its materials
//! read.
//!
//! An enemy that isn't derived from a pack's model starts as a spec, `<name>.figure.ron`, kept in
//! `tools/figures/`, beside `tools/motion/` where its clips are posed. `sapphire_pipeline gen-figures
//! --specs tools/figures --out content` writes each spec's files under the content directory, in the
//! folder the spec names, and they're committed there: the cook reads them as it reads any other
//! glTF and PNG, through their sidecars, and never this module. So no code names a figure: a spec
//! is found by its folder, and its files by the documents that bind them. The benchmark's cast
//! (`bench::cast`, M5 B3) builds its figures from specs too, at cook time, and commits none.
//!
//! | File (in the spec's `dir`) | What |
//! |---|---|
//! | `<name>.glb` | The model: skinned primitives at their bind pose, one a material, in metres, facing +Z, with a skeleton of its own ([`build`]) |
//! | `<name>.vrm` | The model instead, for a figure with a `character`: a VRM 1.0 file with its humanoid, expressions and springs ([`vrm`]) |
//! | `<stem>_palette.png` | The albedo: a row of cells, one a region, each a vertical gradient ([`paint::palette_png`]) |
//! | `<stem>_control.png` | What an `.ilm.ron` packs: each region's highlight, shadow bias, highlight size and material ID ([`paint::control_png`]) |
//! | `<stem>_layers.png` | A body material's layer mask: R the matcap, B emission, by region and down its cell ([`paint::layers_png`]) |
//! | `<stem>_threshold_<angle>.png` | A face material's threshold masks, baked from its normals (`face::masks`) |
//! | `<name>_matcap.png` | The warrior's painted gunmetal ([`combat::matcap_png`]), the figure's own copy, so a zone that reaches the figure reaches no asset the warrior's code names |
//!
//! A map's `<stem>` is `<name>` for a figure of one material, and `<name>_<material>` for each of a
//! figure's `materials`.
//!
//! # A spec
//!
//! ```ron
//! (
//!     type: "sapphire::pipeline::Figure",
//!     version: 2,
//!     data: (
//!         dir: "combat/lancer",
//!         name: "lancer",
//!         max_triangles: 8000,
//!         joints: [
//!             (name: "root", at: (0.0, 0.0, 0.0), aim: Some((0.0, 1.0, 0.0))),
//!             (name: "hips", parent: Some("root"), at: (0.0, 0.8, 0.0)),
//!             // ...
//!         ],
//!         regions: [
//!             (name: "cloth", albedo: [(0.0, "#2a3050"), (1.0, "#12142a")]),
//!         ],
//!         shapes: [
//!             Loft((region: "cloth", bones: ["spine", "chest"], from: (0.0, 0.85, 0.0), to: (0.0, 1.3, 0.0),
//!                   rings: [(at: 0.0, radius: (0.12, 0.09)), (at: 1.0, radius: (0.15, 0.1))])),
//!         ],
//!     ),
//! )
//! ```
//!
//! - **Joints** are listed parents first. Each one's rest frame aims its +Y at `aim`, or at its first
//!   child, as KayKit's rig does, rolled so its +Z leans toward the model's +Z (its +Y when the bone
//!   runs along Z). A leaf with no `aim` keeps its parent's frame. Names are the figure's own; an
//!   enemy uses KayKit's (`hips`, `upperarm.l`, `handslot.r`, ...), so the pack's clips retarget onto
//!   it by sidecar and anything that finds the warrior's joints by name finds these.
//! - **Regions,** at most [`MAX_REGIONS`] a material, are a material's regions in order: a region's
//!   index among its material's is its material ID. Each paints a cell of its material's maps.
//! - **Shapes** each belong to one region and are skinned to the `bones` they name ([`Shape`]).
//!   `mirror` adds a copy across X with `.l` and `.r` swapped in its bones' names. A bone ending in
//!   `*` names every joint whose name starts with what comes before it.
//!
//! The rest pose is a T-pose: arms out along X, which key-pose documents assume.
//!
//! # At a budget (M5 B3)
//!
//! Version 2 adds what the benchmark's cast needs to stand at spec §3.4's rows; each is optional,
//! and a spec without them builds the bytes it built before:
//!
//! - **`chains`** hang rows of joints from a joint, for hair, skirts and tails ([`Chain`]), turned
//!   about the model's Y axis as many times as `around` says. They're added after `joints`, level by
//!   level (every chain's first joint, then every chain's second, ...), so any prefix of the skeleton
//!   holds every joint's parent: a skeleton cut to its first N joints keeps the humanoid and the
//!   chains' upper joints.
//! - **`detail`** cuts every loft finer: its sides and the spans between its rings, times `detail`.
//!   **`triangles`** holds the model to exactly that many.
//! - **`materials`** split the regions among several materials ([`MaterialSpec`]), each its own
//!   primitive, slot and set of maps; a face material bakes threshold masks for a face SDF map from
//!   its own normals ([`face`]).
//! - **`maps`** paints the maps at a size, square, with a grain, and unwraps each shape across its
//!   cell (U around a loft or across a slab), so the maps' every texel is drawn.
//! - **`expressions`** are VRM 1.0's presets as morph targets: moves of the face material's vertices
//!   ([`ExpressionSpec`]).
//! - **`character`** makes the model a VRM ([`CharacterSpec`]): its humanoid, the expressions bound
//!   to their presets, and the chains as spring bones with colliders.
//!
//! # Determinism
//!
//! Every machine writes the same bytes. Rings are placed by `det::sin_cos`, joint rotations come
//! from `det::quat_from_mat3` and colours go through `det::pow`, so nothing reaches a platform's C
//! library or glam's SIMD backends. `generating_twice_is_byte_identical` holds one machine to itself,
//! and `the_committed_figures_are_generated` holds every OS CI tests to the committed bytes.

pub mod build;
pub mod face;
pub mod paint;
pub mod vrm;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::combat;
use crate::generators::Output;
use crate::samples::{Existing, Generated, write_files};

/// The most regions a figure's material has: what an enemy's look takes (M3 B3).
pub const MAX_REGIONS: usize = 6;

/// A spec's file name ends with this.
pub const SPEC_EXTENSION: &str = ".figure.ron";

/// A figure's spec: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct Figure {
    /// The folder its files go in, relative to the content directory, with `/` between parts.
    pub dir: String,
    /// The stem of its files, and the name of its model's material slot when it has one material.
    pub name: String,
    /// The most triangles its model may have: its budget, which the build holds it to.
    pub max_triangles: u32,
    /// Exactly how many triangles its model has, if it must have that many (M5 B3).
    #[serde(default)]
    pub triangles: Option<u32>,
    /// How finely its lofts are cut: their sides and the spans between their rings, times this.
    #[serde(default = "one")]
    pub detail: u32,
    /// Its skeleton, parents first.
    pub joints: Vec<JointSpec>,
    /// Chains of joints, added after `joints` level by level (M5 B3).
    #[serde(default)]
    pub chains: Vec<Chain>,
    /// Its materials, if more than one: each region names its own (M5 B3).
    #[serde(default)]
    pub materials: Vec<MaterialSpec>,
    /// Its materials' regions, by material ID within each material.
    pub regions: Vec<Region>,
    /// What it's made of.
    pub shapes: Vec<Shape>,
    /// The size its maps are painted at, if not the cells' own (M5 B3).
    #[serde(default)]
    pub maps: Option<Maps>,
    /// VRM 1.0's expression presets as morph targets of its face material (M5 B3).
    #[serde(default)]
    pub expressions: Vec<ExpressionSpec>,
    /// What makes it a VRM: its humanoid and its springs (M5 B3).
    #[serde(default)]
    pub character: Option<CharacterSpec>,
}

impl Versioned for Figure {
    /// 1: M3 C2. 2: M5 B3's chains, detail, materials, maps, expressions and character, each
    /// optional, so a version 1 spec reads as it is.
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1 => data.decode().map_err(VersionError::Decode),
            _ => Err(VersionError::TooOld { found: version, oldest: 1 }),
        }
    }
}

fn one() -> u32 {
    1
}

/// A joint at the bind pose, in the model's space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointSpec {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    /// Where it is, in metres.
    pub at: [f32; 3],
    /// Where its +Y points, if not at its first child.
    #[serde(default)]
    pub aim: Option<[f32; 3]>,
}

/// A chain of `joints` joints, evenly spaced from `from` to `to`, hung from `parent`: a strand of
/// hair, a skirt's panel, a tail. With `around`, copies of it turned about the model's Y axis.
///
/// A chain's joints are `<name>_<k>` from 0 at `from`, or `<name>_<ii>_<k>` for copy `ii` of one
/// turned around; the copy itself, as a spring, is `<name>` or `<name>_<ii>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chain {
    pub name: String,
    pub parent: String,
    pub from: [f32; 3],
    pub to: [f32; 3],
    /// At least two.
    pub joints: u32,
    #[serde(default)]
    pub around: Option<Around>,
}

/// Copies of a chain, the `i`th turned `i × step` degrees about the model's Y axis, counter-clockwise
/// seen from above (from +X toward −Z).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Around {
    /// At least two.
    pub count: u32,
    pub step: f32,
}

/// A material of several (M5 B3): its regions are those that name it, in their order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialSpec {
    /// Its slot's name, and its maps' `<name>_<material>` stem.
    pub name: String,
    #[serde(default)]
    pub surface: Surface,
    /// Draws its back faces too, as a single layer of cloth does.
    #[serde(default)]
    pub double_sided: bool,
}

/// Which part of a character a material shades, as the anime character family's surfaces do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Surface {
    #[default]
    Body,
    /// Shaded by a face SDF map baked from its normals ([`face`]); the expressions move it.
    Face,
    Hair,
}

/// How a figure's maps are painted (M5 B3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Maps {
    /// Texels a side, square: a power of two from 64 to 4096.
    pub size: u32,
    /// How far each texel of the albedo strays from its gradient, as a share of full scale, by a
    /// hash of its place: painted texture, so a map compresses as a painted one does.
    #[serde(default)]
    pub grain: f32,
}

/// The sizes a map may be painted at.
pub const MAP_SIZES: std::ops::RangeInclusive<u32> = 64..=4096;

/// One region of a material, and the cell it paints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub name: String,
    /// The material it belongs to, when the figure has `materials`.
    #[serde(default)]
    pub material: Option<String>,
    /// The albedo down the cell, as `(v, "#rrggbb")` stops, ascending, blended in linear light.
    pub albedo: Vec<(f32, String)>,
    /// The highlight's strength (the control map's R).
    #[serde(default)]
    pub specular: f32,
    /// The highlight's size (the control map's B).
    #[serde(default = "half")]
    pub specular_size: f32,
    /// How readily it falls into shadow, 0.5 shading as its normals say (the control map's G).
    #[serde(default = "half")]
    pub shadow_bias: f32,
    /// How much of the material's matcap it shows (the layer mask's R).
    #[serde(default)]
    pub matcap: f32,
    /// How much of its region's emission shows down the cell, as `(v, share)` stops (the layer
    /// mask's B). None shows none.
    #[serde(default)]
    pub emission: Vec<(f32, f32)>,
}

fn half() -> f32 {
    0.5
}

/// Something a figure is made of. Every shape is skinned to its `bones`: each vertex to the
/// nearest bone, and to the others in proportion as they come within `blend` metres of as near,
/// at most four. A bone runs from its joint to its child, or to where it aims.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Loft(Loft),
    Slab(Slab),
}

/// Rings along an axis, joined: a limb, a cone, a bulb, or with `thickness` a shell open at its ends,
/// such as a sleeve, or with `arc` too a panel, such as a skirt's front.
///
/// Its frame is the axis `a` from `from` to `to`, `across` made square to it (the model's X by
/// default, or Z if the axis runs along X), and `a × across`. A ring's angle runs from `across`
/// (0°) toward `a × across` (90°): down a loft running down from the waist, 90° is the front (+Z).
/// Unwrapped ([`Maps`]), U runs with the angle across the region's cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loft {
    pub region: String,
    pub bones: Vec<String>,
    #[serde(default = "default_blend")]
    pub blend: f32,
    pub from: [f32; 3],
    pub to: [f32; 3],
    #[serde(default)]
    pub across: Option<[f32; 3]>,
    /// At least two, by increasing `at`.
    pub rings: Vec<Ring>,
    /// Faces around a whole ring, or across the arc.
    #[serde(default = "default_sides")]
    pub sides: u32,
    /// The part of each ring it covers, in degrees: only with `thickness`.
    #[serde(default)]
    pub arc: Option<(f32, f32)>,
    /// Makes it a shell this thick, open at its ends and its arc's edges.
    #[serde(default)]
    pub thickness: Option<f32>,
    /// A solid loft's ends are closed, unless this is false.
    #[serde(default = "yes")]
    pub caps: bool,
    /// Every face shades flat, as hammered plate does.
    #[serde(default)]
    pub faceted: bool,
    /// The cell's V at its first ring and at its last.
    #[serde(default = "default_v")]
    pub v: (f32, f32),
    #[serde(default)]
    pub mirror: bool,
}

/// One ring of a loft.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ring {
    /// How far along the axis, 0 at `from` and 1 at `to`.
    pub at: f32,
    /// Its half widths along `across` and along `a × across`, in metres.
    pub radius: (f32, f32),
    /// How far its middle sits off the axis, along `across` and `a × across`.
    #[serde(default)]
    pub shift: (f32, f32),
}

/// A flat outline given thickness: a blade, a mask, a plate. The outline lies in the plane through
/// `origin` along `u` and `w`, in metres along each, star-shaped about its centroid. Its V runs from
/// `v.0` at its highest `w` to `v.1` at its lowest; unwrapped, its U runs across the cell with `u`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slab {
    pub region: String,
    pub bones: Vec<String>,
    #[serde(default = "default_blend")]
    pub blend: f32,
    pub origin: [f32; 3],
    pub u: [f32; 3],
    pub w: [f32; 3],
    /// At least three corners.
    pub outline: Vec<(f32, f32)>,
    pub thickness: f32,
    #[serde(default = "default_v")]
    pub v: (f32, f32),
    #[serde(default)]
    pub mirror: bool,
}

fn default_blend() -> f32 {
    0.05
}

fn default_sides() -> u32 {
    10
}

fn yes() -> bool {
    true
}

fn default_v() -> (f32, f32) {
    (0.0, 1.0)
}

/// One of VRM 1.0's expression presets as a morph target of the face material (M5 B3): its moves,
/// summed, displace the face's vertices at the bind pose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpressionSpec {
    /// The preset's VRM 1.0 name (`happy`, `aa`, `blinkLeft`, ...), which names its morph target.
    pub preset: String,
    pub moves: Vec<Move>,
    /// Shows whole or not at all.
    #[serde(default)]
    pub binary: bool,
    /// What it does to the blink, look-at and mouth presets while it shows: `none`, `block` or
    /// `blend`, as VRM 1.0 names them.
    #[serde(default)]
    pub override_blink: Option<String>,
    #[serde(default)]
    pub override_look_at: Option<String>,
    #[serde(default)]
    pub override_mouth: Option<String>,
}

/// A displacement of the vertices within `radius` of `at`, fading smoothly to nothing at
/// `radius` (`(1 − d²/r²)²`). `mirror` adds the same move across X.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Move {
    /// Moves them by `by`, in metres.
    Pull {
        at: [f32; 3],
        radius: f32,
        by: [f32; 3],
        #[serde(default)]
        mirror: bool,
    },
    /// Scales their distance from the plane through `at` square to `axis` by `scale`: 0 flattens
    /// an eye shut, 2 opens a mouth twice as wide.
    Squash {
        at: [f32; 3],
        radius: f32,
        axis: [f32; 3],
        scale: f32,
        #[serde(default)]
        mirror: bool,
    },
}

/// What makes a figure a VRM 1.0 character (M5 B3), so its model cooks a rig: humanoid bones, the
/// expressions, spring bones and their colliders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterSpec {
    /// Each humanoid bone's joint, by VRM 1.0's bone name (`hips`, `leftUpperArm`, ...).
    pub humanoid: BTreeMap<String, String>,
    /// Where the eyes look from, in the head joint's frame, for `lookAt`.
    #[serde(default)]
    pub look_from: [f32; 3],
    /// Every spring's colliders, one group.
    #[serde(default)]
    pub colliders: Vec<ColliderSpec>,
    /// The chains that are springs, and how they move.
    #[serde(default)]
    pub springs: Vec<SpringSpec>,
}

/// A sphere, or a capsule to `to`, on a joint, placed in the model's space at the bind pose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColliderSpec {
    pub joint: String,
    pub at: [f32; 3],
    #[serde(default)]
    pub to: Option<[f32; 3]>,
    pub radius: f32,
}

/// Every copy of chain `chain` as a spring of its own, with these parameters on every joint
/// (VRM 1.0's, per frame at 60 Hz).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringSpec {
    pub chain: String,
    pub stiffness: f32,
    pub gravity_power: f32,
    pub drag_force: f32,
    pub hit_radius: f32,
}

/// Why a spec can't make a figure.
#[derive(Debug, thiserror::Error)]
pub enum FigureError {
    #[error("{path}: {source}")]
    Document { path: PathBuf, source: Box<DocumentError> },
    #[error("joint {joint:?}: {why}")]
    Joint { joint: String, why: String },
    #[error("region {region:?}: {why}")]
    Region { region: String, why: String },
    #[error("shape {index}: {why}")]
    Shape { index: usize, why: String },
    #[error("{0}")]
    Spec(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// One of a figure's materials, as its regions group into them.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialGroup {
    /// Its slot's name.
    pub name: String,
    /// Its maps' stem: `<name>` alone for a figure of one material.
    pub stem: String,
    pub surface: Surface,
    pub double_sided: bool,
    /// Its regions, by index into the figure's, in material ID order.
    pub regions: Vec<usize>,
}

impl Figure {
    /// Its materials, each with its regions: one named after the figure without `materials`.
    pub fn material_groups(&self) -> Result<Vec<MaterialGroup>, FigureError> {
        let region_error = |region: &Region, why: &str| FigureError::Region {
            region: region.name.clone(),
            why: why.to_owned(),
        };
        let groups: Vec<MaterialGroup> = if self.materials.is_empty() {
            if let Some(region) = self.regions.iter().find(|region| region.material.is_some()) {
                return Err(region_error(region, "names a material, but the figure lists none"));
            }
            vec![MaterialGroup {
                name: self.name.clone(),
                stem: self.name.clone(),
                surface: Surface::Body,
                double_sided: false,
                regions: (0..self.regions.len()).collect(),
            }]
        } else {
            let mut groups: Vec<MaterialGroup> = Vec::with_capacity(self.materials.len());
            for material in &self.materials {
                if groups.iter().any(|group| group.name == material.name) {
                    return Err(FigureError::Spec(format!("material {:?} is listed twice", material.name)));
                }
                groups.push(MaterialGroup {
                    name: material.name.clone(),
                    stem: format!("{}_{}", self.name, material.name),
                    surface: material.surface,
                    double_sided: material.double_sided,
                    regions: Vec::new(),
                });
            }
            for (index, region) in self.regions.iter().enumerate() {
                let name =
                    region.material.as_deref().ok_or_else(|| region_error(region, "names no material"))?;
                let group = groups.iter_mut().find(|group| group.name == name).ok_or_else(|| {
                    region_error(region, &format!("names material {name:?}, which isn't listed"))
                })?;
                group.regions.push(index);
            }
            groups
        };
        for group in &groups {
            if group.regions.is_empty() || group.regions.len() > MAX_REGIONS {
                return Err(FigureError::Spec(format!(
                    "material {:?} has one to {MAX_REGIONS} regions, not {}",
                    group.name,
                    group.regions.len()
                )));
            }
        }
        if groups.iter().filter(|group| group.surface == Surface::Face).count() > 1 {
            return Err(FigureError::Spec("a figure has one face material at most".to_owned()));
        }
        Ok(groups)
    }

    /// Its maps' size, checked: none paints each cell at its own size.
    fn checked_maps(&self) -> Result<Option<Maps>, FigureError> {
        match self.maps {
            Some(maps) if !maps.size.is_power_of_two() || !MAP_SIZES.contains(&maps.size) => {
                Err(FigureError::Spec(format!(
                    "maps are a power of two from {} to {} texels, not {}",
                    MAP_SIZES.start(),
                    MAP_SIZES.end(),
                    maps.size
                )))
            }
            Some(maps) if !(0.0..=0.5).contains(&maps.grain) => {
                Err(FigureError::Spec(format!("a map's grain is 0 to 0.5, not {}", maps.grain)))
            }
            maps => Ok(maps),
        }
    }
}

/// Reads a spec.
pub fn read(path: &Path) -> Result<Figure, FigureError> {
    let text = fs::read_to_string(path)?;
    sapphire_serialize::from_ron(&text)
        .map_err(|error| FigureError::Document { path: path.to_owned(), source: Box::new(error) })
}

/// Every spec in `dir`, by file name.
pub fn specs(dir: &Path) -> Result<Vec<PathBuf>, FigureError> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(SPEC_EXTENSION))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

/// What one of a figure's files is.
#[derive(Clone, Debug, PartialEq)]
pub enum FileKind {
    /// The model: a `.glb`, or a `.vrm` for a figure with a `character`.
    Model,
    /// A material's albedo, by its index among [`Figure::material_groups`].
    Palette(usize),
    /// A material's control map, which its ILM document packs.
    Control(usize),
    /// A material's layer mask.
    Layers(usize),
    /// A face material's threshold mask at this many degrees.
    FaceMask(usize, f32),
    /// The matcap every material shares.
    Matcap,
}

/// One of a figure's files, relative to the content directory, with its bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct FigureFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub kind: FileKind,
}

/// A figure's files, each with what it is, relative to the content directory.
pub fn figure_files(figure: &Figure) -> Result<Vec<FigureFile>, FigureError> {
    let built = build::build(figure)?;
    let groups = figure.material_groups()?;
    let maps = figure.checked_maps()?;
    let file = |path: String, bytes: Vec<u8>, kind: FileKind| FigureFile { path, bytes, kind };
    let in_dir = |name: String| format!("{}/{name}", figure.dir);
    let model = match &figure.character {
        Some(character) => file(
            in_dir(format!("{}.vrm", figure.name)),
            vrm::to_vrm(&built, figure, character)?,
            FileKind::Model,
        ),
        None if figure.expressions.is_empty() => {
            file(in_dir(format!("{}.glb", figure.name)), built.to_glb(), FileKind::Model)
        }
        None => return Err(FigureError::Spec("expressions need a character to bind them".to_owned())),
    };
    let mut files = vec![model];
    for (index, group) in groups.iter().enumerate() {
        let regions: Vec<&Region> = group.regions.iter().map(|&region| &figure.regions[region]).collect();
        let stem = |suffix: &str| in_dir(format!("{}{suffix}", group.stem));
        files.push(file(stem("_palette.png"), paint::palette_png(&regions, maps)?, FileKind::Palette(index)));
        files.push(file(stem("_control.png"), paint::control_png(&regions, maps)?, FileKind::Control(index)));
        // A face's and hair's material binds no layer mask: the shader reads their masks of their own.
        if group.surface == Surface::Body {
            files.push(file(
                stem("_layers.png"),
                paint::layers_png(&regions, maps)?,
                FileKind::Layers(index),
            ));
        }
        if group.surface == Surface::Face {
            for (angle, png) in face::masks(&built, index, maps)? {
                files.push(file(stem(&face::mask_suffix(angle)), png, FileKind::FaceMask(index, angle)));
            }
        }
    }
    files.push(file(in_dir(format!("{}_matcap.png", figure.name)), combat::matcap_png()?, FileKind::Matcap));
    Ok(files)
}

/// A figure's files, relative to the content directory, with their bytes.
pub fn files(figure: &Figure) -> Result<Vec<(String, Vec<u8>)>, FigureError> {
    Ok(figure_files(figure)?.into_iter().map(|file| (file.path, file.bytes)).collect())
}

/// Writes every spec's files under the content directory `out`, from the specs in `specs`. Only what's
/// missing is written, as [`write_files`] says, unless `existing` replaces what differs.
pub fn write_all(specs_dir: &Path, out: &Path, existing: Existing) -> Result<Generated, FigureError> {
    let mut generated = Generated::default();
    for path in specs(specs_dir)? {
        let figure = read(&path)?;
        let outputs = files(&figure)?.into_iter().map(|(name, bytes)| Output::plain(name, move || bytes));
        let written = write_files(out, outputs.collect(), existing)?;
        generated.created.extend(written.created);
        generated.replaced.extend(written.replaced);
        generated.kept.extend(written.kept);
        generated.unchanged.extend(written.unchanged);
    }
    Ok(generated)
}

#[cfg(test)]
mod tests;
