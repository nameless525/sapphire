//! Assemblies (M1 X1): kit pieces placed into one static mesh and given materials of the
//! project's own, from a RON document.
//!
//! An environment kit (Kenney's Fantasy Town Kit under `content/third_party/`) ships as small
//! glTF pieces in one flat palette: every triangle's texture coordinates point into a cell of a
//! shared color map. A world built from it needs the pieces placed, scaled to its characters,
//! and drawn in its own materials rather than the kit's palette. An assembly does that at cook
//! time, leaving the kit's files untouched:
//!
//! - **Placement.** `<name>.assembly.ron` ([`Assembly`]) places pieces (`"kit/model"`, a glTF
//!   file in a kit's directory) and groups of placements (`"@group"`) by a position, a turn about
//!   +Y and a scale, nested as deep as groups go. Three builtin shapes fill what kits lack:
//!   `#box`, a unit cube about its origin; `#quad`, a unit square in XZ facing +Y; and `#ground`,
//!   a disc one across in XZ facing +Y, for open land out to the horizon. Parametric builtins
//!   (a placement's `shape`, [`Shape`]) build East Asian architecture from numbers: roofs with
//!   modeled tiles, halls, walls, columns and lanterns, and a market's stalls and carts with
//!   their goods (`assembly/shapes.rs`). The pieces' triangles are transformed into the
//!   assembly's space and merged into one mesh.
//! - **Ground is cut small where cameras stand.** A triangle that crosses a camera's near plane
//!   is clipped there, to vertices far outside the screen (the near plane is centimeters from
//!   the eye), and some rasterizers lose it: WARP dropped a 2.4 km grass quad outright and
//!   shifted a 37 m paving quad's texture coordinates by a fraction of a cobble, where Metal and
//!   lavapipe drew both (X1's WARP run). So the ground builtins come in cells no larger than
//!   [`GROUND_CELL`] where a camera can stand: `#quad` in a grid of them (at most
//!   [`MAX_QUAD_CELLS`] a side; open land is a `#ground`), `#ground` in rings
//!   [`GROUND_CELL`] apart out to [`GROUND_FINE`] meters from its center, then each
//!   [`GROUND_GROWTH`] times the last out to its rim, every ring of [`GROUND_SEGMENTS`]
//!   segments, so neighbors share whole edges (no T-junctions to crack). A camera standing over
//!   them then only has its near plane clip cells too close to it to be seen. `#box` stays
//!   whole: it fills small gaps.
//! - **Surfaces.** Each triangle draws in one of the document's slots, picked by
//!   [`SurfaceRule`]s from the piece's name, the palette cell its texture coordinates fall in,
//!   the glTF material it uses, and the variant its placement asks for (a ground floor of stone
//!   where the same piece upstairs is plaster). A placement may put all its triangles in one slot.
//!   A triangle no rule claims fails the cook, naming the piece and the cell, so no texel of the
//!   kit's palette ever reaches the screen. The mesh's sidecar binds the slots to material
//!   documents, as a glTF's.
//! - **Texture coordinates** are made anew, in the assembly's meters, so a slot's tiling map
//!   keeps one density across pieces of any scale and runs on across neighbors without a seam
//!   ([`Projection`]): planar on each face's own plane, its U level and its V down the slope
//!   (walls, roofs), or boxed onto the nearest axis plane (curved stone, rocks). A builtin's
//!   triangles lose whole blocks of [`WRAP_TILES`] tiles, each starting in the first block: maps
//!   repeat, and a mesh's coordinates are unorm16 across their range, which a 4 km `#ground` in
//!   meters would spread to centimeters on every slot. Neighbors in one block share their
//!   corners.
//! - **Occlusion** goes into the vertex color's R, which the environment shader reads as
//!   ambient occlusion ([`Occlusion`]): walls darken toward the ground, and the kit's own
//!   painted gradients (cells that shade from their top to their bottom) carry over as shading
//!   rather than color.
//! - Identical triangles from overlapping pieces (two walls' shared corner post) are kept once,
//!   so they never fight in the depth buffer.
//! - **Overlaps in one plane fail the cook.** Pieces that meet share planes (a banner on its
//!   wall, a roof on its storey), and where two placements' triangles face the same way in one
//!   plane, the depth buffer can't order them: they fight, differently on every backend and
//!   from frame to frame. Once everything is placed, the cook finds every pair of triangles
//!   from different placements whose normals are within 2.5° and planes within 1 mm over a
//!   region more than 2 mm wide, and fails on the first, naming both pieces, their placements'
//!   paths in the document (`place[8] > gate[14]`), their slots and where they meet. Triangles
//!   in one slot fight only where they shade differently (other occlusion from the kit's
//!   gradient, say, beyond four levels of 255): identical ones look the same whichever wins.
//!   Back-to-back faces never fight, as back faces are culled. The document's `allow_coplanar`
//!   ([`CoplanarAllowance`]) lets through the overlaps that may stay, those no camera can see
//!   above all (undersides on the ground, attic floors under roofs), each entry naming pieces, a
//!   group or a region and saying why; an entry that lets nothing through fails the cook too,
//!   so the list stays as short as the assembly needs. The search buckets triangles by normal
//!   and plane and sweeps each plane, so it costs little beside the rest of the cook (14 ms of
//!   the look-dev town's 60–70 ms import, its 89k triangles).
//!
//! The cooked asset is a mesh like any glTF's (`town.assembly.ron` → `town.smesh`): outline
//! normals are baked, the mesh optimized and LODs built as its sidecar says (`importer:
//! Assembly((…))`, the glTF mesh settings). The pieces' files are dependencies of its cache key,
//! by the path the document names them with, so editing a piece or the document recooks it,
//! and the importer's version carries the glTF importer's ([`IMPORTER`]).
//!
//! # Later milestones
//!
//! - Zones (spec §5.21) place prefabs as instances at runtime, culled and streamed per cell;
//!   an assembly is one static mesh, the right shape for a slice that is seen whole.
//! - Painted vertex colors and per-piece texture offsets, once the environment family reads
//!   an albedo tint from the vertex (it reads ambient occlusion alone today).
//! - Telling hidden overlaps in one plane from open ones without the document's word: rays
//!   from each overlap past the assembly's own triangles would find the enclosed ones.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use sapphire_assets::material::AnimeCharacterMaterial;
use sapphire_assets::mesh::{Lod, Submesh, Vertex, VertexFlags};
use sapphire_reflect::{TypePath, Versioned};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;
use crate::character_maps;
use crate::color::unorm8;
use crate::import::gltf::{self, GltfImportError};
use crate::mesh::{ImportedMaterial, ImportedMesh};

mod coplanar;
mod shapes;
#[cfg(test)]
mod tests;

pub use shapes::{
    Awning, Beam, Cart, ClothAwning, ClothKind, Column, Coping, Counter, Detail, Fill, Goods, GoodsKind,
    Hall, Lantern, LanternKind, Mount, Opening, OpeningKind, Part, Plinth, Posts, Ridge, Roof, RoofKind,
    Shape, Shelves, Side, Sign, Stall, Surface, TiledAwning, Tiles, Wall, Wheels,
};

/// This importer, as cache keys know it, with the glTF importer's version in its low 16 bits:
/// pieces are imported by it, so a bump there must recook assemblies too. Bump the high bits
/// whenever assembly output changes for the same document and pieces.
pub const IMPORTER: Stage = Stage { name: "assembly", version: (3 << 16) | gltf::IMPORTER.version };

/// The end of an assembly document's file name, lowercase.
pub const SOURCE_SUFFIX: &str = ".assembly.ron";

/// The builtin unit cube: `-0.5..0.5` on every axis.
pub const BOX: &str = "#box";
/// The builtin unit square: `-0.5..0.5` in X and Z, facing +Y.
pub const QUAD: &str = "#quad";
/// The builtin disc of ground: radius 0.5 in XZ about its origin, facing +Y.
pub const GROUND: &str = "#ground";

/// The ground builtins' cells, at most this many meters across where cameras stand (see the
/// module docs).
pub const GROUND_CELL: f32 = 2.0;
/// The most cells a `#quad` takes along a side: 256 m of ground, more than any camera crosses.
pub const MAX_QUAD_CELLS: u32 = 128;
/// `#ground`'s rings are [`GROUND_CELL`] apart out to this radius in meters...
pub const GROUND_FINE: f32 = 40.0;
/// ...then each this many times the last's radius.
pub const GROUND_GROWTH: f32 = 1.25;
/// The segments around each of `#ground`'s rings: about [`GROUND_CELL`] wide at
/// [`GROUND_FINE`].
pub const GROUND_SEGMENTS: u32 = 128;

/// Builtins' texture coordinates wrap by blocks of this many tiles (see the module docs).
pub const WRAP_TILES: f32 = 64.0;

/// Faces whose normal is within this of vertical take top-down texture coordinates.
const LEVEL_FACE: f32 = 0.9;
/// Groups may nest this deep: deeper is a cycle.
const MAX_DEPTH: usize = 16;
/// Positions of identical triangles agree to this, in meters.
const SAME_POSITION: f32 = 1e-4;
/// The assembly's own placement list, as placement paths name it.
const PLACE: &str = "place";

/// Whether `path` is an assembly document, by the end of its file name (in any case).
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|name| name.len() > SOURCE_SUFFIX.len() && name.ends_with(SOURCE_SUFFIX))
}

/// The cooked file an assembly at `relative` writes: `town.assembly.ron` → `town.<extension>`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// An assembly document (`sapphire::pipeline::Assembly`, version 1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct Assembly {
    /// Directories of glTF pieces by kit name, relative to the document's directory,
    /// `/`-separated. `"kenney/wall"` names `<kits["kenney"]>/wall.glb`.
    pub kits: BTreeMap<String, String>,
    /// How the kits' palette maps are divided into cells, which [`SurfaceRule::cell`] names.
    #[serde(default)]
    pub palette: Palette,
    /// The mesh's material slots, in order.
    pub slots: Vec<SlotSpec>,
    /// Which slot each triangle draws in. Later rules win over earlier ones.
    pub surfaces: Vec<SurfaceRule>,
    #[serde(default)]
    pub occlusion: Occlusion,
    /// Named placement lists, placed by `"@<name>"`.
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<Placement>>,
    /// The assembly.
    pub place: Vec<Placement>,
    /// Overlaps in one plane the cook lets through, each saying why (see the module docs).
    #[serde(default)]
    pub allow_coplanar: Vec<CoplanarAllowance>,
}

impl Versioned for Assembly {
    const VERSION: u32 = 1;
}

/// A grid over the kits' shared palette map: `columns` × `rows` cells, numbered from the
/// top-left, `[column, row]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Palette {
    pub columns: u32,
    pub rows: u32,
    /// Columns whose cells are painted gradients, light at the top to dark at the bottom: a
    /// triangle's place in such a cell is the kit's shading of it ([`Occlusion::gradient`]).
    pub gradient_columns: Vec<u32>,
}

impl Default for Palette {
    fn default() -> Self {
        Self { columns: 1, rows: 1, gradient_columns: Vec::new() }
    }
}

/// A material slot of the assembled mesh, and how its texture coordinates are made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotSpec {
    /// Unique; the sidecar binds it to a material by this name.
    pub name: String,
    /// Meters one repeat of the slot's maps covers, across (U) and down (V).
    #[serde(default = "SlotSpec::default_tile")]
    pub tile: [f32; 2],
    #[serde(default)]
    pub projection: Projection,
}

impl SlotSpec {
    fn default_tile() -> [f32; 2] {
        [1.0, 1.0]
    }
}

/// How a slot's texture coordinates follow its faces. Both measure the assembly's space in
/// meters, so maps keep their density whatever a piece's scale, and level faces (the normal
/// within about 25° of vertical) are mapped from above, U along +X and V along +Z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Projection {
    /// On each face's own plane: U level, to the right seen from the front, and V down the
    /// slope. Courses of stone and rows of tiles stay level on any wall or roof.
    #[default]
    Planar,
    /// As [`Self::Planar`] on the nearest of the axis planes: curved surfaces then break into a
    /// few large patches rather than a seam at every face.
    Box,
}

/// Picks a slot for the triangles it matches: those of pieces whose model name matches one of
/// `pieces` (`*` matches any run of characters; none matches every piece), whose texture
/// coordinates fall in palette `cell` and whose glTF material is `material` (when given), placed
/// with `variant` (when given).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceRule {
    #[serde(default)]
    pub pieces: Vec<String>,
    #[serde(default)]
    pub cell: Option<[u32; 2]>,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub variant: Option<String>,
    pub slot: String,
}

/// Ambient occlusion baked into the vertex color.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Occlusion {
    /// Faces that aren't level take `1 − ground` at `y = 0`, rising smoothly to 1 at
    /// `ground_height` meters: walls darken toward the ground they stand on.
    pub ground: f32,
    pub ground_height: f32,
    /// A triangle in a [`Palette::gradient_columns`] cell takes `1 − gradient × t`, `t` its
    /// vertices' place from the cell's top (0) to its bottom (1).
    pub gradient: f32,
}

impl Default for Occlusion {
    fn default() -> Self {
        Self { ground: 0.0, ground_height: 1.0, gradient: 0.0 }
    }
}

/// One placement: a piece (`"kit/model"`), a builtin shape (`"#box"`, `"#quad"`), a group
/// (`"@name"`) or, in place of a piece, a parametric `shape` ([`Shape`]: `#roof`, `#hall`, ...),
/// transformed by `at · turn · scale` within whatever places it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    #[serde(default)]
    pub piece: String,
    /// A parametric builtin, in place of `piece`.
    #[serde(default)]
    pub shape: Option<Shape>,
    #[serde(default)]
    pub at: [f32; 3],
    /// Degrees about +Y, counter-clockwise seen from above.
    #[serde(default)]
    pub turn: f32,
    #[serde(default)]
    pub scale: Scale,
    /// The surface variant for this placement and everything in it, unless they name their own.
    #[serde(default)]
    pub variant: Option<String>,
    /// Draws every triangle in this slot, whatever the rules say, and everything in it unless
    /// it names its own. The simple builtins (`#box`, `#quad`, `#ground`) need one, their own or
    /// one from a group they are in; a parametric shape's parts otherwise go by the rules.
    #[serde(default)]
    pub slot: Option<String>,
}

impl Placement {
    /// What it places, as rules, allowances and messages name it: its piece, or its shape's
    /// builtin name.
    pub fn name(&self) -> &str {
        match &self.shape {
            Some(shape) => shape.name(),
            None => &self.piece,
        }
    }
}

/// Lets through the overlaps in one plane (see the module docs) that match every filter it
/// gives, which must include `pieces`, `within` or `region`: by direction alone, it would let
/// through everything facing that way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoplanarAllowance {
    /// Overlaps between a placement of one piece and one of the other, as placements name them
    /// (`"kenney/planks"`, `"#box"`), `*` matching any run of characters.
    #[serde(default)]
    pub pieces: Option<(String, String)>,
    /// Overlaps between two placements inside one placement of this group (`*` patterns too).
    #[serde(default)]
    pub within: Option<String>,
    /// Overlaps whose faces face this way, within the check's angle.
    #[serde(default)]
    pub facing: Option<[f32; 3]>,
    /// Overlaps whose middle lies in this box of the assembly's space: its least and greatest
    /// corners, in meters.
    #[serde(default)]
    pub region: Option<([f32; 3], [f32; 3])>,
    /// Why they can stay: what hides them from every camera, as a rule.
    pub why: String,
}

/// A uniform scale, or one per axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scale {
    Uniform(f32),
    Axes([f32; 3]),
}

impl Default for Scale {
    fn default() -> Self {
        Self::Uniform(1.0)
    }
}

impl Scale {
    fn vector(self) -> Vec3 {
        match self {
            Self::Uniform(scale) => Vec3::splat(scale),
            Self::Axes(scale) => Vec3::from(scale),
        }
    }
}

/// Errors from reading an assembly document and assembling it.
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    #[error("the document isn't valid UTF-8")]
    NotUtf8,
    #[error("not a valid assembly document")]
    Document(#[source] Box<DocumentError>),
    #[error("{0}")]
    Invalid(String),
    #[error("piece {piece:?} names no kit of the document's ({})", kits.join(", "))]
    UnknownKit { piece: String, kits: Vec<String> },
    #[error("group {0:?} isn't defined")]
    UnknownGroup(String),
    #[error("groups nest deeper than {MAX_DEPTH} at {0:?}: a group places itself")]
    Cycle(String),
    #[error("{what} names slot {slot:?}, which isn't one of the document's")]
    UnknownSlot { what: String, slot: String },
    #[error("builtin {0:?} needs a slot")]
    BuiltinWithoutSlot(String),
    #[error("no surface rule claims the {part:?} part of {shape:?} (variant {variant:?}): add a rule for it")]
    UnclaimedPart { shape: String, part: String, variant: Option<String> },
    #[error("{shape} at {path}: {message}")]
    Shape { shape: String, path: String, message: String },
    #[error(
        "no surface rule claims the triangles of {piece:?} in palette cell {cell:?} \
         (glTF material {material:?}, variant {variant:?}): add a rule for them"
    )]
    Unclaimed { piece: String, cell: [u32; 2], material: String, variant: Option<String> },
    #[error("failed to read piece {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to import piece {}", path.display())]
    Import {
        path: PathBuf,
        #[source]
        source: GltfImportError,
    },
    #[error("piece path {0:?} isn't a path relative to the document's directory")]
    PiecePath(String),
    #[error("piece {0} is skinned or morphed; assemblies take static pieces")]
    Animated(String),
    #[error("the assembly places no triangles")]
    Empty,
    #[error("{0}")]
    Coplanar(Box<CoplanarFight>),
    #[error("allow_coplanar entry {index} lets no overlap through: remove it ({why:?})")]
    StaleAllowance { index: usize, why: String },
}

/// Two placements whose triangles fight in one plane, the first such pair by the document's
/// order, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct CoplanarFight {
    /// The pieces as the placements name them.
    pub pieces: [String; 2],
    /// The placements' paths in the document: `place[8] > gate[14]`.
    pub paths: [String; 2],
    pub slots: [String; 2],
    /// The middle of one place they fight, in the assembly's space.
    pub at: [f32; 3],
    /// How many more pairs of placements fight.
    pub more: usize,
}

impl std::fmt::Display for CoplanarFight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { pieces, paths, slots, at, more } = self;
        write!(
            f,
            "{} ({}) and {} ({}) draw in one plane around ({:.2}, {:.2}, {:.2}) m, in slots {:?} and {:?}, \
             and the depth buffer can't order them: move one off the other's plane, or if no camera can \
             see them, let them through in allow_coplanar saying why",
            pieces[0], paths[0], pieces[1], paths[1], at[0], at[1], at[2], slots[0], slots[1]
        )?;
        match more {
            0 => Ok(()),
            1 => write!(f, " (one more pair of placements fights too)"),
            _ => write!(f, " ({more} more pairs of placements fight too)"),
        }
    }
}

/// Reads an assembly document and checks it.
pub fn read(bytes: &[u8]) -> Result<Assembly, AssemblyError> {
    let text = std::str::from_utf8(bytes).map_err(|_| AssemblyError::NotUtf8)?;
    let assembly: Assembly =
        sapphire_serialize::from_ron(text).map_err(|source| AssemblyError::Document(Box::new(source)))?;
    assembly.validate()?;
    Ok(assembly)
}

impl Assembly {
    /// Checks what the types can't: unique slot names that rules and placements name, positive
    /// tiles and palette sizes, finite and invertible placements, and known kits and groups.
    pub fn validate(&self) -> Result<(), AssemblyError> {
        let invalid = |message: String| Err(AssemblyError::Invalid(message));
        if self.palette.columns == 0 || self.palette.rows == 0 {
            return invalid("the palette needs at least one column and row".to_owned());
        }
        let mut names = HashSet::new();
        for slot in &self.slots {
            if !names.insert(slot.name.as_str()) {
                return invalid(format!("slot {:?} is declared twice", slot.name));
            }
            if !slot.tile.iter().all(|&tile| tile.is_finite() && tile > 0.0) {
                return invalid(format!("slot {:?}'s tile must be positive, not {:?}", slot.name, slot.tile));
            }
        }
        let known = |slot: &str, what: &dyn Fn() -> String| {
            if names.contains(slot) {
                Ok(())
            } else {
                Err(AssemblyError::UnknownSlot { what: what(), slot: slot.to_owned() })
            }
        };
        for (index, rule) in self.surfaces.iter().enumerate() {
            known(&rule.slot, &|| format!("surface rule {index}"))?;
            if let Some([column, row]) = rule.cell
                && (column >= self.palette.columns || row >= self.palette.rows)
            {
                return invalid(format!(
                    "surface rule {index}'s cell {:?} is outside the palette",
                    rule.cell
                ));
            }
        }
        let Occlusion { ground, ground_height, gradient } = self.occlusion;
        if !((0.0..=1.0).contains(&ground) && (0.0..=1.0).contains(&gradient) && ground_height > 0.0) {
            return invalid(format!(
                "occlusion {:?} must have strengths in 0..=1 and a positive height",
                self.occlusion
            ));
        }
        for (index, allowance) in self.allow_coplanar.iter().enumerate() {
            let what = format!("allow_coplanar entry {index}");
            if allowance.why.trim().is_empty() {
                return invalid(format!("{what} must say why its overlaps can stay"));
            }
            if allowance.pieces.is_none() && allowance.within.is_none() && allowance.region.is_none() {
                return invalid(format!("{what} must name pieces, a group or a region"));
            }
            if let Some(facing) = allowance.facing.map(Vec3::from)
                && !(facing.is_finite() && facing.length_squared() > 0.0)
            {
                return invalid(format!("{what}'s facing {facing:?} must be a direction"));
            }
            if let Some((least, greatest)) = allowance.region
                && !(least.iter().chain(&greatest).all(|v| v.is_finite())
                    && least.iter().zip(&greatest).all(|(a, b)| a <= b))
            {
                return invalid(format!("{what}'s region must run from its least corner to its greatest"));
            }
        }
        let lists = self.groups.iter().map(|(name, list)| (format!("group {name:?}"), list));
        for (owner, list) in std::iter::once(("the assembly".to_owned(), &self.place)).chain(lists) {
            for placement in list {
                let what = || format!("{owner}'s placement of {:?}", placement.name());
                if let Some(slot) = &placement.slot {
                    known(slot, &what)?;
                }
                let scale = placement.scale.vector();
                let finite = placement.at.iter().all(|v| v.is_finite()) && placement.turn.is_finite();
                if !finite || !scale.is_finite() || scale.x * scale.y * scale.z == 0.0 {
                    return invalid(format!(
                        "{} needs a finite position and turn and a nonzero scale",
                        what()
                    ));
                }
                if let Some(shape) = &placement.shape {
                    if !placement.piece.is_empty() {
                        return invalid(format!("{} gives both a piece and a shape: give one", what()));
                    }
                    shape
                        .validate()
                        .map_err(|message| AssemblyError::Invalid(format!("{}: {message}", what())))?;
                    continue;
                }
                match placement.piece.chars().next() {
                    None => {
                        return invalid(format!("{owner} has a placement with neither a piece nor a shape"));
                    }
                    Some('@') if !self.groups.contains_key(&placement.piece[1..]) => {
                        return Err(AssemblyError::UnknownGroup(placement.piece[1..].to_owned()));
                    }
                    Some('#') if ![BOX, QUAD, GROUND].contains(&placement.piece.as_str()) => {
                        return invalid(format!(
                            "{} names no builtin shape ({BOX}, {QUAD}, {GROUND})",
                            what()
                        ));
                    }
                    Some('@' | '#') => {}
                    _ => {
                        self.piece_file(&placement.piece)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The file a piece placement names, as the document names it (relative to its directory),
    /// and its model name.
    fn piece_file<'a>(&self, piece: &'a str) -> Result<(String, &'a str), AssemblyError> {
        let unknown = || AssemblyError::UnknownKit {
            piece: piece.to_owned(),
            kits: self.kits.keys().cloned().collect(),
        };
        let (kit, model) = piece.split_once('/').ok_or_else(unknown)?;
        let dir = self.kits.get(kit).ok_or_else(unknown)?;
        if model.is_empty() || model.contains(['/', '\\']) {
            return Err(AssemblyError::PiecePath(piece.to_owned()));
        }
        Ok((format!("{}/{model}.glb", dir.trim_end_matches('/')), model))
    }

    /// Every piece file the assembly places, as the document names them (relative to its
    /// directory), sorted and without repeats: the inputs its cook reads.
    pub fn files(&self) -> Result<Vec<String>, AssemblyError> {
        let lists = std::iter::once(&self.place).chain(self.groups.values());
        let mut files: Vec<String> = lists
            .flatten()
            .filter(|placement| placement.shape.is_none() && !placement.piece.starts_with(['@', '#']))
            .map(|placement| self.piece_file(&placement.piece).map(|(file, _)| file))
            .collect::<Result<_, _>>()?;
        files.sort();
        files.dedup();
        Ok(files)
    }
}

/// Reads the assembly at `path` (`bytes`) and the pieces it places, and assembles its mesh.
pub fn import(path: &Path, bytes: &[u8], detail: Detail) -> Result<ImportedMesh, AssemblyError> {
    let assembly = read(bytes)?;
    assemble(&assembly, detail, &mut |file: &str| {
        let resolved = character_maps::resolve_input(path, file)
            .map_err(|_| AssemblyError::PiecePath(file.to_owned()))?;
        let bytes = std::fs::read(&resolved)
            .map_err(|source| AssemblyError::Read { path: resolved.clone(), source })?;
        gltf::import_slice(&bytes, resolved.parent())
            .map_err(|source| AssemblyError::Import { path: resolved, source })
    })
}

/// A piece's triangles, ready to place: three corners each, with the slot facts rules read.
struct Piece {
    triangles: Vec<PieceTriangle>,
}

#[derive(Clone, Copy)]
struct PieceTriangle {
    positions: [Vec3; 3],
    normals: [Vec3; 3],
    /// The palette cell of the corners' mean texture coordinates.
    cell: [u32; 2],
    /// Each corner's place in its cell, 0 at the top and 1 at the bottom.
    down: [f32; 3],
    /// Index into [`Piece`]'s source materials (a shape's [`Part::ALL`]).
    material: usize,
    /// How its texture coordinates are made.
    uv: ShapeUv,
    /// The piece's own ambient occlusion at each corner (1 open), times the assembly's.
    occlusion: [f32; 3],
}

/// A triangle's texture coordinates: projected by its slot (kit pieces, flat faces), or a
/// shape's own for surfaces that curve.
#[derive(Clone, Copy, Debug)]
enum ShapeUv {
    Project,
    /// In the shape's meters: scaled by the placement's (uniform) scale, then by the slot's tile.
    Meters([Vec2; 3]),
    /// In tiles, as they are: one repeat of the slot's maps per unit whatever the slot's tile
    /// (a lantern's paper, a rib a tile).
    Tiles([Vec2; 3]),
}

/// A piece's glTF material names.
struct PieceInfo {
    piece: Piece,
    materials: Vec<String>,
}

/// Assembles `assembly`, importing each piece file it names with `import_piece` (once each).
pub fn assemble(
    assembly: &Assembly,
    detail: Detail,
    import_piece: &mut dyn FnMut(&str) -> Result<ImportedMesh, AssemblyError>,
) -> Result<ImportedMesh, AssemblyError> {
    assembly.validate()?;
    let mut builder = Builder::new(assembly, detail);
    let mut pieces: HashMap<String, PieceInfo> = HashMap::new();
    for file in assembly.files()? {
        let mesh = import_piece(&file)?;
        if mesh.skin.is_some() || !mesh.morph_targets.is_empty() {
            return Err(AssemblyError::Animated(file));
        }
        pieces.insert(file, piece_info(&mesh, &assembly.palette));
    }
    let mut path = Vec::new();
    for (index, placement) in assembly.place.iter().enumerate() {
        path.push((PLACE, index));
        builder.place(placement, Mat4::IDENTITY, None, None, &mut path, &pieces)?;
        path.pop();
    }
    builder.finish()
}

/// The triangles of an imported piece, with their palette cells.
fn piece_info(mesh: &ImportedMesh, palette: &Palette) -> PieceInfo {
    let mut triangles = Vec::with_capacity(mesh.indices.len() / 3);
    let lod = &mesh.lods[0];
    for submesh in &lod.submeshes {
        let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
        for corners in mesh.indices[range].as_chunks::<3>().0 {
            let vertices = corners.map(|index| &mesh.vertices[index as usize]);
            let uv = vertices.map(|vertex| vertex.uv);
            let (u, v) = ((uv[0][0] + uv[1][0] + uv[2][0]) / 3.0, (uv[0][1] + uv[1][1] + uv[2][1]) / 3.0);
            let cell_of = |coordinate: f32, count: u32| {
                ((coordinate.rem_euclid(1.0) * count as f32) as u32).min(count - 1)
            };
            let cell = [cell_of(u, palette.columns), cell_of(v, palette.rows)];
            // In the repeat of the map the triangle's middle is in, so a corner on the cell's
            // bottom edge (v = 1) stays at the bottom.
            let top = v.floor() + cell[1] as f32 / palette.rows as f32;
            let down = uv.map(|[_, v]| ((v - top) * palette.rows as f32).clamp(0.0, 1.0));
            triangles.push(PieceTriangle {
                positions: vertices.map(|vertex| Vec3::from(vertex.position)),
                normals: vertices.map(|vertex| Vec3::from(vertex.normal)),
                cell,
                down,
                material: submesh.material as usize,
                uv: ShapeUv::Project,
                occlusion: [1.0; 3],
            });
        }
    }
    let materials = mesh.materials.iter().map(|material| material.name.clone()).collect();
    PieceInfo { piece: Piece { triangles }, materials }
}

/// A builtin's triangle, facing its corners' winding.
fn builtin_triangle(positions: [Vec3; 3], normal: Vec3) -> PieceTriangle {
    PieceTriangle {
        positions,
        normals: [normal; 3],
        cell: [0, 0],
        down: [0.0; 3],
        material: 0,
        uv: ShapeUv::Project,
        occlusion: [1.0; 3],
    }
}

/// The builtin shapes' triangles, the ground builtins cut for `transform`, which places them
/// (see the module docs). Fails if a `#quad` would take more than [`MAX_QUAD_CELLS`] a side.
fn builtin(name: &str, transform: Mat4) -> Result<Piece, AssemblyError> {
    // Meters along a unit step of each axis, as placed.
    let meters = |axis: Vec3| transform.transform_vector3(axis).length();
    match name {
        QUAD => {
            let cells = |axis: Vec3| (meters(axis) / GROUND_CELL).ceil().max(1.0) as u32;
            let (columns, rows) = (cells(Vec3::X), cells(Vec3::Z));
            if columns.max(rows) > MAX_QUAD_CELLS {
                return Err(AssemblyError::Invalid(format!(
                    "{QUAD} placed {:.0} × {:.0} m is larger than {MAX_QUAD_CELLS} cells of {GROUND_CELL} m a \
                     side: open land is a {GROUND}",
                    meters(Vec3::X),
                    meters(Vec3::Z)
                )));
            }
            // Each line's coordinate computed once, so neighbors share their corners exactly.
            let lines = |count: u32| (0..=count).map(|k| k as f32 / count as f32 - 0.5).collect::<Vec<_>>();
            let (xs, zs) = (lines(columns), lines(rows));
            let mut triangles = Vec::new();
            for z in zs.windows(2) {
                for x in xs.windows(2) {
                    let corners = [
                        Vec3::new(x[0], 0.0, z[1]),
                        Vec3::new(x[1], 0.0, z[1]),
                        Vec3::new(x[1], 0.0, z[0]),
                        Vec3::new(x[0], 0.0, z[0]),
                    ];
                    for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
                        triangles.push(builtin_triangle([corners[a], corners[b], corners[c]], Vec3::Y));
                    }
                }
            }
            Ok(Piece { triangles })
        }
        GROUND => Ok(ground(meters(Vec3::X).min(meters(Vec3::Z)))),
        _ => Ok(unit_box()),
    }
}

/// `#ground` for a disc `diameter` meters across: rings about its center (see the module docs),
/// in its unit space.
fn ground(diameter: f32) -> Piece {
    let radius = 0.5 * diameter;
    let mut radii = Vec::new();
    let mut r = GROUND_CELL;
    while r < radius {
        radii.push(r);
        r = if r < GROUND_FINE { r + GROUND_CELL } else { r * GROUND_GROWTH };
    }
    radii.push(radius);
    // Each direction computed once, so rings and segments share their corners exactly.
    let directions: Vec<Vec3> = (0..GROUND_SEGMENTS)
        .map(|segment| {
            let angle = std::f32::consts::TAU * segment as f32 / GROUND_SEGMENTS as f32;
            Vec3::new(angle.cos(), 0.0, angle.sin())
        })
        .collect();
    let point =
        |ring: usize, segment: usize| directions[segment % directions.len()] * (radii[ring] / diameter);
    let mut triangles = Vec::new();
    for segment in 0..directions.len() {
        // Angles grow from +X toward +Z, which faces -Y: the corners go the other way round.
        triangles.push(builtin_triangle([Vec3::ZERO, point(0, segment + 1), point(0, segment)], Vec3::Y));
        for ring in 1..radii.len() {
            let (inner, outer) = (ring - 1, ring);
            let (a, b) = (point(inner, segment), point(inner, segment + 1));
            let (c, d) = (point(outer, segment), point(outer, segment + 1));
            triangles.push(builtin_triangle([a, d, c], Vec3::Y));
            triangles.push(builtin_triangle([a, b, d], Vec3::Y));
        }
    }
    Piece { triangles }
}

/// `#box`: a unit cube about its origin, whole.
fn unit_box() -> Piece {
    let quad = |center: Vec3, right: Vec3, up: Vec3| {
        let normal = right.cross(up).normalize();
        let corners = [center - right - up, center + right - up, center + right + up, center - right + up];
        [[0, 1, 2], [0, 2, 3]].map(|[a, b, c]| builtin_triangle([corners[a], corners[b], corners[c]], normal))
    };
    let half = 0.5;
    let faces: Vec<[PieceTriangle; 2]> = vec![
        quad(Vec3::X * half, -Vec3::Z * half, Vec3::Y * half),
        quad(-Vec3::X * half, Vec3::Z * half, Vec3::Y * half),
        quad(Vec3::Y * half, Vec3::X * half, -Vec3::Z * half),
        quad(-Vec3::Y * half, Vec3::X * half, Vec3::Z * half),
        quad(Vec3::Z * half, Vec3::X * half, Vec3::Y * half),
        quad(-Vec3::Z * half, -Vec3::X * half, Vec3::Y * half),
    ];
    Piece { triangles: faces.into_iter().flatten().collect() }
}

/// How [`Builder::emit`] places a triangle.
#[derive(Clone, Copy)]
struct Emit {
    /// The triangle's palette cell shades from its top to its bottom: darken toward the bottom.
    gradient: bool,
    /// Take whole blocks of tiles off its texture coordinates (builtins; see the module docs).
    wrap: bool,
    /// The placement it comes from: an index into [`Builder::placements`].
    placed: usize,
}

/// Collects placed triangles by slot.
struct Builder<'a> {
    assembly: &'a Assembly,
    /// How finely shapes are built.
    detail: Detail,
    slots: HashMap<&'a str, usize>,
    /// Per slot: vertices and triangles, welded.
    vertices: Vec<Vec<Vertex>>,
    indices: Vec<Vec<u32>>,
    welds: Vec<HashMap<[u32; 9], u32>>,
    /// Triangles already placed, by slot and quantized corners in a canonical order.
    placed: HashSet<(usize, [[i32; 3]; 3])>,
    /// Every piece and builtin placed, in order, and every triangle kept, for the coplanar
    /// check.
    placements: Vec<Placed<'a>>,
    faces: Vec<coplanar::Face>,
}

/// A piece or builtin as placed.
struct Placed<'a> {
    placement: &'a Placement,
    /// The lists it was placed through, from the assembly's down: each one's name ([`PLACE`],
    /// or a group's) and the placement's index in it.
    path: Vec<(&'a str, usize)>,
}

impl Placed<'_> {
    /// Its path as messages print it: `place[45] > gate[14]`.
    fn path_text(&self) -> String {
        let steps: Vec<String> = self.path.iter().map(|(list, index)| format!("{list}[{index}]")).collect();
        steps.join(" > ")
    }

    /// Whether it and `other` were placed through one placement of a group `pattern` matches.
    fn within_one(&self, other: &Placed<'_>, pattern: &str) -> bool {
        // Step k's list is the group step k − 1 placed.
        (1..self.path.len().min(other.path.len()))
            .any(|k| self.path[..k] == other.path[..k] && matches_pattern(pattern, self.path[k].0))
    }
}

impl CoplanarAllowance {
    /// Whether its pieces and group name `placed`'s, as an overlap between them needs.
    fn names(&self, placed: [&Placed<'_>; 2]) -> bool {
        let [a, b] = placed.map(|placed| placed.placement.name());
        let pieces = |(first, second): &(String, String)| {
            (matches_pattern(first, a) && matches_pattern(second, b))
                || (matches_pattern(first, b) && matches_pattern(second, a))
        };
        self.pieces.as_ref().is_none_or(pieces)
            && self.within.as_deref().is_none_or(|group| placed[0].within_one(placed[1], group))
    }

    /// Whether its direction and region hold an overlap whose faces face `normal`, its middle
    /// `at`.
    fn holds(&self, normal: Vec3, at: Vec3) -> bool {
        self.facing.is_none_or(|facing| {
            Vec3::from(facing).normalize().dot(normal) >= coplanar::ANGLE.to_radians().cos()
        }) && self.region.is_none_or(|(least, greatest)| {
            at.cmpge(Vec3::from(least)).all() && at.cmple(Vec3::from(greatest)).all()
        })
    }
}

impl<'a> Builder<'a> {
    fn new(assembly: &'a Assembly, detail: Detail) -> Self {
        let count = assembly.slots.len();
        Self {
            assembly,
            detail,
            slots: assembly
                .slots
                .iter()
                .enumerate()
                .map(|(index, slot)| (slot.name.as_str(), index))
                .collect(),
            vertices: vec![Vec::new(); count],
            indices: vec![Vec::new(); count],
            welds: vec![HashMap::new(); count],
            placed: HashSet::new(),
            placements: Vec::new(),
            faces: Vec::new(),
        }
    }

    /// Places `placement`, the last step of `path`, and what it holds.
    fn place(
        &mut self,
        placement: &'a Placement,
        parent: Mat4,
        variant: Option<&str>,
        slot: Option<usize>,
        path: &mut Vec<(&'a str, usize)>,
        pieces: &HashMap<String, PieceInfo>,
    ) -> Result<(), AssemblyError> {
        if path.len() > MAX_DEPTH + 1 {
            return Err(AssemblyError::Cycle(placement.piece.clone()));
        }
        let local = Mat4::from_scale_rotation_translation(
            placement.scale.vector(),
            Quat::from_rotation_y(placement.turn.to_radians()),
            Vec3::from(placement.at),
        );
        let transform = parent * local;
        let variant = placement.variant.as_deref().or(variant);
        let slot = match &placement.slot {
            Some(name) => Some(self.slots[name.as_str()]),
            None => slot,
        };
        if let Some(group) = placement.piece.strip_prefix('@') {
            for (index, child) in self.assembly.groups[group].iter().enumerate() {
                path.push((group, index));
                self.place(child, transform, variant, slot, path, pieces)?;
                path.pop();
            }
            return Ok(());
        }
        let placed = self.placements.len();
        self.placements.push(Placed { placement, path: path.clone() });
        if let Some(shape) = &placement.shape {
            let triangles = shape.triangles(self.detail).map_err(|message| AssemblyError::Shape {
                shape: shape.name().to_owned(),
                path: self.placements[placed].path_text(),
                message,
            })?;
            for triangle in &triangles {
                let part = Part::ALL[triangle.material].name();
                let slot = match slot {
                    Some(slot) => slot,
                    None => self.rule_slot(shape.name(), [0, 0], part, variant).ok_or_else(|| {
                        AssemblyError::UnclaimedPart {
                            shape: shape.name().to_owned(),
                            part: part.to_owned(),
                            variant: variant.map(str::to_owned),
                        }
                    })?,
                };
                self.emit(triangle, transform, slot, Emit { gradient: false, wrap: true, placed });
            }
            return Ok(());
        }
        if placement.piece.starts_with('#') {
            let slot = slot.ok_or_else(|| AssemblyError::BuiltinWithoutSlot(placement.piece.clone()))?;
            let piece = builtin(&placement.piece, transform)?;
            for triangle in &piece.triangles {
                self.emit(triangle, transform, slot, Emit { gradient: false, wrap: true, placed });
            }
            return Ok(());
        }
        let (file, model) = self.assembly.piece_file(&placement.piece)?;
        let info = &pieces[&file];
        for triangle in &info.piece.triangles {
            let material = &info.materials[triangle.material];
            let slot = match slot {
                Some(slot) => slot,
                None => self.rule_slot(model, triangle.cell, material, variant).ok_or_else(|| {
                    AssemblyError::Unclaimed {
                        piece: placement.piece.clone(),
                        cell: triangle.cell,
                        material: material.clone(),
                        variant: variant.map(str::to_owned),
                    }
                })?,
            };
            let gradient = self.assembly.palette.gradient_columns.contains(&triangle.cell[0]);
            self.emit(triangle, transform, slot, Emit { gradient, wrap: false, placed });
        }
        Ok(())
    }

    /// The slot the last matching rule names.
    fn rule_slot(&self, model: &str, cell: [u32; 2], material: &str, variant: Option<&str>) -> Option<usize> {
        self.assembly
            .surfaces
            .iter()
            .rev()
            .find(|rule| {
                (rule.pieces.is_empty() || rule.pieces.iter().any(|pattern| matches_pattern(pattern, model)))
                    && rule.cell.is_none_or(|rule_cell| rule_cell == cell)
                    && rule.material.as_deref().is_none_or(|name| name == material)
                    && rule.variant.as_deref().is_none_or(|name| Some(name) == variant)
            })
            .map(|rule| self.slots[rule.slot.as_str()])
    }

    /// Places one triangle in `slot`.
    fn emit(&mut self, triangle: &PieceTriangle, transform: Mat4, slot: usize, how: Emit) {
        let linear = Mat3::from_mat4(transform);
        let normal_matrix = linear.inverse().transpose();
        let mut positions = triangle.positions.map(|p| transform.transform_point3(p));
        let mut normals = triangle.normals.map(|n| (normal_matrix * n).normalize_or_zero());
        let mut down = triangle.down;
        let mut own_uv = triangle.uv;
        let mut own_occlusion = triangle.occlusion;
        // A mirroring transform turns the winding inside out.
        if linear.determinant() < 0.0 {
            positions.swap(1, 2);
            normals.swap(1, 2);
            down.swap(1, 2);
            own_occlusion.swap(1, 2);
            match &mut own_uv {
                ShapeUv::Project => {}
                ShapeUv::Meters(uv) | ShapeUv::Tiles(uv) => uv.swap(1, 2),
            }
        }
        let face = (positions[1] - positions[0]).cross(positions[2] - positions[0]);
        if face.length_squared() < 1e-12 {
            return;
        }
        let face = face.normalize();
        // Keep one of each identical triangle: corners in a canonical rotation.
        let quantize = |p: Vec3| (p / SAME_POSITION).round().as_ivec3().to_array();
        let mut corners = positions.map(quantize);
        let first = (0..3).min_by_key(|&k| corners[k]).expect("three corners");
        corners.rotate_left(first);
        if !self.placed.insert((slot, corners)) {
            return;
        }

        let spec = &self.assembly.slots[slot];
        let (u_axis, v_axis) = projection_axes(spec.projection, face);
        let occlusion = &self.assembly.occlusion;
        let level = face.y.abs() > LEVEL_FACE;
        let mut uvs = match own_uv {
            ShapeUv::Project => {
                positions.map(|p| [p.dot(u_axis) / spec.tile[0], p.dot(v_axis) / spec.tile[1]])
            }
            ShapeUv::Meters(uv) => {
                // The placement's scale, as one factor (a uniform scale's).
                let scale = linear.determinant().abs().cbrt();
                uv.map(|uv| [uv.x * scale / spec.tile[0], uv.y * scale / spec.tile[1]])
            }
            ShapeUv::Tiles(uv) => uv.map(|uv| uv.to_array()),
        };
        if how.wrap {
            // Whole blocks of tiles off, which a repeating map can't tell apart: the triangle
            // starts in the first block.
            for axis in 0..2 {
                let least = uvs.iter().map(|uv| uv[axis]).fold(f32::INFINITY, f32::min);
                let whole = (least / WRAP_TILES).floor() * WRAP_TILES;
                uvs.iter_mut().for_each(|uv| uv[axis] -= whole);
            }
        }
        let mut corner_indices = [0u32; 3];
        let mut shading = [coplanar::Shading { normal: face, uv: Vec2::ZERO, occlusion: 1.0 }; 3];
        for k in 0..3 {
            let position = positions[k];
            let uv = uvs[k];
            let mut ao = own_occlusion[k].clamp(0.0, 1.0);
            if !level && occlusion.ground > 0.0 {
                ao *= 1.0 - occlusion.ground * (1.0 - smoothstep(0.0, occlusion.ground_height, position.y));
            }
            if how.gradient {
                ao *= 1.0 - occlusion.gradient * down[k];
            }
            let normal = if normals[k] == Vec3::ZERO { face } else { normals[k] };
            let vertex = Vertex {
                position: position.to_array(),
                normal: normal.to_array(),
                outline_normal: normal.to_array(),
                uv,
                color: [unorm8(ao), 255, 255],
                flags: VertexFlags::NONE,
            };
            shading[k] =
                coplanar::Shading { normal, uv: uv.into(), occlusion: f32::from(vertex.color[0]) / 255.0 };
            let key = weld_key(&vertex);
            let vertices = &mut self.vertices[slot];
            corner_indices[k] = *self.welds[slot].entry(key).or_insert_with(|| {
                vertices.push(vertex);
                (vertices.len() - 1) as u32
            });
        }
        self.indices[slot].extend(corner_indices);
        self.faces.push(coplanar::Face {
            corners: positions,
            normal: face,
            slot,
            placement: how.placed,
            shading,
        });
    }

    /// Fails on the overlaps in one plane no allowance lets through (naming the first pair of
    /// placements, by their order in the document), then on an allowance that lets none through.
    fn check_coplanar(&self) -> Result<(), AssemblyError> {
        let allowances = &self.assembly.allow_coplanar;
        let mut used = vec![false; allowances.len()];
        // The first fight of each pair of placements, which faces' order puts in order.
        let mut fights: BTreeMap<[usize; 2], (Vec3, [usize; 2])> = BTreeMap::new();
        // Which allowances name each pair of placements, matched once a pair.
        let mut named: HashMap<[usize; 2], Vec<bool>> = HashMap::new();
        for overlap in coplanar::overlaps(&self.faces) {
            let faces = overlap.faces.map(|face| &self.faces[face]);
            let pair = faces.map(|face| face.placement);
            let named = named.entry(pair).or_insert_with(|| {
                let placed = pair.map(|placement| &self.placements[placement]);
                allowances.iter().map(|allowance| allowance.names(placed)).collect()
            });
            let mut allowed = false;
            for (index, allowance) in allowances.iter().enumerate() {
                if named[index] && allowance.holds(faces[0].normal, overlap.at) {
                    (used[index], allowed) = (true, true);
                }
            }
            if !allowed {
                fights.entry(pair).or_insert((overlap.at, faces.map(|face| face.slot)));
            }
        }
        if let Some((&pair, &(at, slots))) = fights.iter().next() {
            let placed = pair.map(|placement| &self.placements[placement]);
            return Err(AssemblyError::Coplanar(Box::new(CoplanarFight {
                pieces: placed.map(|placed| placed.placement.name().to_owned()),
                paths: placed.map(Placed::path_text),
                slots: slots.map(|slot| self.assembly.slots[slot].name.clone()),
                at: at.to_array(),
                more: fights.len() - 1,
            })));
        }
        match used.iter().position(|&used| !used) {
            Some(index) => Err(AssemblyError::StaleAllowance { index, why: allowances[index].why.clone() }),
            None => Ok(()),
        }
    }

    /// The mesh: a submesh per slot that has triangles, in slot order, and a material slot for
    /// every declared slot.
    fn finish(self) -> Result<ImportedMesh, AssemblyError> {
        self.check_coplanar()?;
        let mut mesh = ImportedMesh::default();
        let mut submeshes = Vec::new();
        for (slot, (vertices, indices)) in self.vertices.into_iter().zip(self.indices).enumerate() {
            if indices.is_empty() {
                continue;
            }
            let base = mesh.vertices.len() as u32;
            submeshes.push(Submesh {
                first_index: mesh.indices.len() as u32,
                index_count: indices.len() as u32,
                material: slot as u32,
            });
            mesh.indices.extend(indices.into_iter().map(|index| base + index));
            mesh.vertices.extend(vertices);
        }
        if mesh.indices.is_empty() {
            return Err(AssemblyError::Empty);
        }
        mesh.authored_outline = vec![false; mesh.vertices.len()];
        mesh.lods = vec![Lod { vertex_count: mesh.vertices.len() as u32, error: 0.0, submeshes }];
        mesh.materials = self
            .assembly
            .slots
            .iter()
            .map(|slot| ImportedMaterial {
                name: slot.name.clone(),
                generated: AnimeCharacterMaterial::default(),
            })
            .collect();
        Ok(mesh)
    }
}

/// The U and V axes of a face with unit normal `face`: dot a position with them for its texture
/// coordinates (before the tile's scale).
fn projection_axes(projection: Projection, face: Vec3) -> (Vec3, Vec3) {
    let normal = match projection {
        Projection::Planar => face,
        Projection::Box => {
            let magnitude = face.abs();
            if magnitude.x >= magnitude.y && magnitude.x >= magnitude.z {
                Vec3::X * face.x.signum()
            } else if magnitude.y >= magnitude.z {
                Vec3::Y * face.y.signum()
            } else {
                Vec3::Z * face.z.signum()
            }
        }
    };
    if normal.y.abs() > LEVEL_FACE {
        return (Vec3::X, Vec3::Z);
    }
    // U level and to the right seen from the front; V down the slope, as image rows run.
    let u = Vec3::Y.cross(normal).normalize();
    let up = normal.cross(u);
    (u, -up)
}

/// Whether `name` matches `pattern`, where `*` matches any run of characters.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else { return false };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else { return rest.is_empty() };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// A vertex's bits, for welding exact repeats.
fn weld_key(vertex: &Vertex) -> [u32; 9] {
    let [x, y, z] = vertex.position.map(f32::to_bits);
    let [nx, ny, nz] = vertex.normal.map(f32::to_bits);
    let [u, v] = vertex.uv.map(f32::to_bits);
    let color = u32::from_le_bytes([vertex.color[0], vertex.color[1], vertex.color[2], 0]);
    [x, y, z, nx, ny, nz, u, v, color]
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
