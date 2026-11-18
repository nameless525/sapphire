//! Prefabs as plain data (M6 T4): an assembly document's types, which places kit pieces, builtin shapes and
//! groups of placements into one static mesh drawn in the document's own material slots.
//!
//! A zone places an assembly as its prefab, whole or by its top-level placements; the cooker reads one
//! (`sapphire_pipeline`'s `assembly`, whose module docs say what every field does to the mesh) and the
//! editor opens one as a document. Neither may depend on the other, so the types live here, beside the zone
//! that places them. Checking one against its kits and shapes, building its triangles and the coplanar
//! check stay in the pipeline: they need its glTF importer and its geometry.
//!
//! An assembly's stable name is still the pipeline's, `sapphire::pipeline::Assembly`, as every committed
//! document's envelope says.

use std::collections::BTreeMap;

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::{Untagged, Versioned};
use serde::{Deserialize, Serialize};

pub mod shapes;

pub use shapes::Shape;

/// An assembly document (`sapphire::pipeline::Assembly`, version 1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone::prefab"]
#[reflect(Serialize, Deserialize, @Untagged)]
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
