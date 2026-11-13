//! Figures (M3 C2): skinned models in the anime character family, built from a spec rather than
//! from a model, and written as ordinary sources: a `.glb` and the maps its material reads.
//!
//! An enemy that isn't derived from a pack's model starts as a spec, `<name>.figure.ron`, kept in
//! `tools/figures/`, beside `tools/motion/` where its clips are posed. `sapphire_pipeline gen-figures
//! --specs tools/figures --out content` writes each spec's files under the content directory, in the
//! folder the spec names, and they're committed there: the cook reads them as it reads any other
//! glTF and PNG, through their sidecars, and never this module. So no code names a figure: a spec
//! is found by its folder, and its files by the documents that bind them.
//!
//! | File (in the spec's `dir`) | What |
//! |---|---|
//! | `<name>.glb` | The model: one skinned primitive at its bind pose, in metres, facing +Z, with a skeleton of its own ([`build`]) |
//! | `<name>_palette.png` | The albedo: a row of cells, one a region, each a vertical gradient ([`paint::palette_png`]) |
//! | `<name>_control.png` | What an `.ilm.ron` packs: each region's highlight, shadow bias, highlight size and material ID ([`paint::control_png`]) |
//! | `<name>_layers.png` | The layer mask: R the matcap, B emission, by region and down its cell ([`paint::layers_png`]) |
//! | `<name>_matcap.png` | The warrior's painted gunmetal ([`combat::matcap_png`]), the figure's own copy, so a zone that reaches the figure reaches no asset the warrior's code names |
//!
//! # A spec
//!
//! ```ron
//! (
//!     type: "sapphire::pipeline::Figure",
//!     version: 1,
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
//! - **Regions,** at most [`MAX_REGIONS`], are the material's regions in order: a region's index is
//!   its material ID. Each paints a cell of the maps.
//! - **Shapes** each belong to one region and are skinned to the `bones` they name ([`Shape`]).
//!   `mirror` adds a copy across X with `.l` and `.r` swapped in its bones' names.
//!
//! The rest pose is a T-pose: arms out along X, which key-pose documents assume.
//!
//! # Determinism
//!
//! Every machine writes the same bytes. Rings are placed by `det::sin_cos`, joint rotations come
//! from `det::quat_from_mat3` and colours go through `det::pow`, so nothing reaches a platform's C
//! library or glam's SIMD backends. `generating_twice_is_byte_identical` holds one machine to itself,
//! and `the_committed_figures_are_generated` holds every OS CI tests to the committed bytes.

pub mod build;
pub mod paint;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
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
    /// The stem of its files, and the name of its model's material slot.
    pub name: String,
    /// The most triangles its model may have: its budget, which the build holds it to.
    pub max_triangles: u32,
    /// Its skeleton, parents first.
    pub joints: Vec<JointSpec>,
    /// Its material's regions, by material ID.
    pub regions: Vec<Region>,
    /// What it's made of.
    pub shapes: Vec<Shape>,
}

impl Versioned for Figure {
    /// 1: M3 C2.
    const VERSION: u32 = 1;
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

/// One region of the material, and the cell it paints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub name: String,
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
/// `v.0` at its highest `w` to `v.1` at its lowest.
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

/// A figure's files, relative to the content directory, with their bytes.
pub fn files(figure: &Figure) -> Result<Vec<(String, Vec<u8>)>, FigureError> {
    let built = build::build(figure)?;
    let path = |suffix: &str| format!("{}/{}{suffix}", figure.dir, figure.name);
    Ok(vec![
        (path(".glb"), built.to_glb()),
        (path("_palette.png"), paint::palette_png(figure)?),
        (path("_control.png"), paint::control_png(figure)?),
        (path("_layers.png"), paint::layers_png(figure)?),
        (path("_matcap.png"), combat::matcap_png()?),
    ])
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
