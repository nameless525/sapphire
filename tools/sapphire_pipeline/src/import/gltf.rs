//! glTF 2.0 importer for `.gltf` and `.glb` files.
//!
//! Flattens the default scene into one [`ImportedMesh`]:
//!
//! - **Static primitives** take their node's world transform: positions by the matrix,
//!   normals by its inverse transpose. Mirrored transforms flip the triangle winding, as glTF
//!   requires (§3.7.4).
//! - **Skinned primitives** (a node with a mesh and a skin) stay in bind space, untransformed:
//!   glTF ignores a skinned mesh node's own transform (§3.7.3.3), and the skin's palette moves
//!   them. See "Skins" below.
//! - Each primitive (per node instance) becomes one submesh of LOD0.
//! - Each glTF material becomes a material slot, named after the material (`#<index>` for an
//!   unnamed one, `<name>#<index>` when two share a name, `#default` for glTF's default
//!   material), holding a generated [`AnimeCharacterMaterial`]: `baseColorFactor` is its base
//!   color, `alphaMode` and `alphaCutoff` its alpha, and material `extras` may set
//!   `shade_color` and `outline_color` (region 0's, linear RGB or RGBA, alpha ignored),
//!   `character_shading` (`[light-split threshold, softness, rim strength, outline width]`),
//!   `rim` (an object with any of the depth-offset rim's settings, M1 C5: `width`, `strength`,
//!   `threshold`, `fresnel`, `light_gate`, `back_light`) and `rim_regions` (`[[strength, width, overlap],
//!   …]`, one per region from region 0: the regions' rim scales, each region past the first a
//!   copy of region 0 but for them, which `_MATERIAL_ID` picks).
//!   Everything else keeps its default. In Blender these are custom properties on the
//!   material, exported with "Include → Custom Properties". A mesh's sidecar can bind slots to
//!   material documents instead (see [`sapphire_assets::material`]).
//! - `COLOR_0` RGB becomes the anime character vertex color, linear data as glTF defines it
//!   (`sapphire_assets::mesh`, "Vertex color and flags"): R ambient occlusion, G outline-width
//!   scale, B outline Z-offset scale. Its alpha is ignored. Without `COLOR_0` every channel
//!   is 1. Blender converts its byte color attributes from sRGB to linear on export, so data
//!   painted there belongs in a float color attribute.
//! - A `_MATERIAL_ID` attribute (SCALAR: float holding whole numbers, or unsigned byte or
//!   short, not normalized) gives each vertex a material ID from 0 to 7, the region its
//!   material shades it with when the material has no control map ([`VertexFlags`]). The
//!   renderer passes it on flat, from one vertex of each triangle, so a triangle's three
//!   vertices must share it ([`GltfImportError::MixedMaterialIds`]): vertices on a border
//!   between IDs are split, one per side. In Blender that is an integer attribute named
//!   `_MATERIAL_ID` on the face-corner domain (so the exporter splits them), exported with
//!   "Include → Attributes". Without one every vertex has ID 0.
//! - Missing normals are replaced by flat face normals.
//! - An `_OUTLINE_NORMAL` attribute (VEC3: float, or normalized byte or short) is an authored
//!   outline normal, in the same space as `NORMAL`. It wins over the welded bake
//!   ([`bake::outline_normals`](crate::bake::outline_normals)), which fills in every other
//!   vertex. Without either, a vertex's outline normal is its shading normal.
//!
//! # Skins
//!
//! When any node in the scene is skinned, the mesh gets one [`ImportedSkin`]:
//!
//! - **Skeleton:** every joint of every skin used, plus their ancestors, so model space is
//!   the scene's space and a non-joint parent (Blender's armature node) still transforms its
//!   joints. Joints are in depth-first order of the scene, children in glTF order, so parents
//!   come first; names are node names (`#<index>` for unnamed nodes) and the rest pose is
//!   each node's local transform.
//! - **Palette:** one entry per distinct (joint, inverse bind matrix) a skin uses, in first-use
//!   order. A missing `inverseBindMatrices` means identities.
//! - **Weights:** all `JOINTS_n`/`WEIGHTS_n` sets are read. Repeats of a joint are merged, and
//!   the four heaviest influences are kept and renormalized to sum to 1; any further ones are
//!   dropped (their weight goes to the kept four in proportion). A vertex with no positive
//!   weight is bound wholly to the skin's first joint.
//! - **Static parts of a skinned mesh** (weapons and accessories parented to a bone, props
//!   beside the character) are bound rigidly to their nearest ancestor joint, or to joint 0
//!   if they have none, through a palette entry that inverts that joint's rest transform. In
//!   the rest pose every palette entry is then the identity for them.
//!
//! # Morph targets
//!
//! Each target is named by the mesh's `extras.targetNames` (Blender writes them), or
//! `<mesh>/<index>` without one, and targets with one name merge across nodes and meshes:
//! a face, its eyelashes and its brows exported as separate objects then share one `blink`
//! weight. Two targets of one primitive with one name are an error. A target's default weight
//! is the node's `weights`, else the mesh's, else 0; a merged target takes the first one's.
//! Instances of one mesh whose nodes rest a target at different weights are the exception
//! (the review's AP-003): one target can't hold both, so the mesh's first instance merges as
//! above, and each other default gets a target of its own, named `<name>@<node>` (the node's
//! name, or `#<index>`; suffixed `#2`, `#3`… if taken), which that mesh's instances at that
//! default share. Every instance then rests as its node says, whatever the scene's order;
//! which one keeps the plain name follows the order. The scene import lists the targets each
//! mesh's target went to (`SceneImport::targets_of_morph`), so a VRM expression binding a
//! mesh's target drives it on every instance.
//! Position deltas take the static node's linear transform; normal deltas become the
//! difference of the transformed, normalized normals, exact at full weight. Vertices a target
//! moves by less than [`MORPH_POSITION_EPSILON`] and turns by less than
//! [`MORPH_NORMAL_EPSILON`] are left out. Tangent deltas are ignored: meshes carry no tangents.
//! Outline normals have no deltas of their own: the skinning pass turns each with its vertex's
//! morphed normal (M1 C4).
//!
//! glTF textures are ignored: they are separate sources (M1 A2) that material documents reference,
//! and VRM import (A6) extracts a file's embedded images as such sources. Animations are ignored
//! here too: a sidecar's `clips` setting cooks them, through [`clips`].
//! External buffers are part of the source: [`external_files`] lists them for the content
//! cache key.
//!
//! # Malformed files
//!
//! A bad file is an error saying what's wrong, and the cook's error names the file: never a panic,
//! a stack overflow or exhausted memory, so one bad model doesn't take a cook down with it. This
//! isn't a security boundary, only a promise to whoever adds a model.
//!
//! - **Accessors** are checked before a reader touches them, every one the importer reads: their
//!   types, counts, strides and byte ranges, sparse blocks included (`accessors`).
//! - **Nodes** form strict trees, as glTF requires (§3.5.3): a node reached twice is an error,
//!   since a small file could otherwise double its mesh at each level. The walk keeps its own
//!   stack, so a deep hierarchy can't overflow the thread's.
//! - **Counts** stay within [`SOURCE_BUDGET`], read from the accessors before their data is: a
//!   sparse accessor can claim any number of elements in a few hundred bytes.
//! - **Positions** are finite, in the mesh's space: a NaN would otherwise cook silently, to a
//!   corner of the mesh's bounds.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ::gltf::accessor::DataType;
use ::gltf::buffer::Source;
use ::gltf::json::validation::{self, Checked};
use ::gltf::mesh::{Mode, Semantic};
use ::gltf::{Accessor, Document, Gltf, Material, Mesh, Node, Primitive, Scene};
use glam::{Mat3, Mat4, Vec3};
use sapphire_assets::character_maps::{MATERIAL_IDS, MaterialId};
use sapphire_assets::material::{AlphaMode, AnimeCharacterMaterial, CharacterRegion};
use sapphire_assets::mesh::{Lod, SkinBind, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags};
use sapphire_assets::skeleton::{Joint, JointTransform};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use crate::cache::Stage;
use crate::color::unorm8;
use crate::import::{self, DEFAULT_SLOT};
use crate::mesh::{
    ImportedDelta, ImportedMaterial, ImportedMesh, ImportedMorphTarget, ImportedSkin, SkinWeights,
};

pub use self::accessors::AccessorFault;

pub mod clips;

/// This importer, as cache keys know it. Bump the version whenever import output changes
/// for the same file. 9: instances of one mesh resting a morph target at different weights
/// get a target each (the review's AP-003). 8: generated materials take the material's
/// `doubleSided` (M1 C8). 7:
/// generated materials carry the depth-offset rim's settings and per-region rim scales from
/// material extras (M1 C5). 6: generated materials carry outlines v2's settings,
/// and VRM's MToon maps its outline widths onto the width's curve and its outline lighting mix
/// (M1 C4).
pub const IMPORTER: Stage = Stage { name: "gltf", version: 9 };

/// Morph deltas whose position moves less than this (meters, on every axis) and whose normal
/// changes less than [`MORPH_NORMAL_EPSILON`] are dropped as noise: a micrometer is far below
/// the position quantization of any mesh.
pub const MORPH_POSITION_EPSILON: f32 = 1e-6;
/// See [`MORPH_POSITION_EPSILON`]: 1e-4 is under a hundredth of a degree.
pub const MORPH_NORMAL_EPSILON: f32 = 1e-4;

/// Joints and palette entries are addressed by u16.
const MAX_JOINTS: usize = 1 << 16;

/// How much of a mesh one source may bring into a cook, checked from accessors' counts before
/// their data is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshBudget {
    /// Vertices, every primitive and node instance of the scene together.
    pub vertices: usize,
    /// Indices, likewise.
    pub indices: usize,
    /// Morph target deltas, likewise.
    pub morph_deltas: usize,
    /// In one primitive, vertices × `JOINTS_n` sets. Sets may share accessors, and each holds four
    /// influences per vertex in memory until the heaviest are picked.
    pub influence_sets: usize,
}

/// The budget of every glTF and VRM source. At the limit its vertices alone take 768 MiB, about
/// what an image at `image::SOURCE_BUDGET` does. The largest committed sources are hundreds of
/// times under it: a town piece's 37,888 vertices and the VRM character's 109,410 indices and
/// 44,537 morph deltas.
pub const SOURCE_BUDGET: MeshBudget =
    MeshBudget { vertices: 16 << 20, indices: 48 << 20, morph_deltas: 16 << 20, influence_sets: 64 << 20 };

/// Identifies a mesh primitive in error messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimitiveRef {
    /// The mesh name, or `#<index>` when it has none.
    pub mesh: String,
    pub primitive: usize,
}

impl PrimitiveRef {
    fn new(mesh: &Mesh<'_>, primitive: &Primitive<'_>) -> Self {
        Self { mesh: describe(mesh.name(), Some(mesh.index())), primitive: primitive.index() }
    }
}

impl fmt::Display for PrimitiveRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mesh {} primitive {}", self.mesh, self.primitive)
    }
}

/// Errors from importing a glTF file.
#[derive(Debug, thiserror::Error)]
pub enum GltfImportError {
    #[error("failed to read {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("not a valid glTF 2.0 file")]
    Parse(#[source] ::gltf::Error),
    #[error("buffer {index} {problem}")]
    Buffer { index: usize, problem: String },
    #[error("the document has no scene to import")]
    NoScene,
    #[error("node {node} is reached twice; glTF node hierarchies must be strict trees")]
    NodeReachedTwice { node: usize },
    #[error("{at}: primitive mode {mode:?} is not supported; export triangles")]
    UnsupportedMode { at: PrimitiveRef, mode: Mode },
    #[error("{at}: has no POSITION attribute")]
    MissingPositions { at: PrimitiveRef },
    #[error("{at}: the {semantic} accessor has no readable data")]
    UnreadableAccessor { at: PrimitiveRef, semantic: String },
    #[error("{at}: {semantic} has {found} elements but POSITION has {expected}")]
    AttributeCount { at: PrimitiveRef, semantic: String, found: usize, expected: usize },
    #[error("{at}: the {semantic} accessor {fault}")]
    Accessor { at: PrimitiveRef, semantic: String, fault: AccessorFault },
    #[error("{at}: vertex {vertex}'s position {position:?} is not finite")]
    NonFinitePosition { at: PrimitiveRef, vertex: usize, position: [f32; 3] },
    #[error("{at}: {found} {what} is over the budget of {budget} for one source")]
    OverBudget { at: PrimitiveRef, what: &'static str, found: usize, budget: usize },
    #[error(
        "{at}: vertex {vertex} has _MATERIAL_ID {value}; material IDs are whole numbers below {MATERIAL_IDS}"
    )]
    MaterialIdOutOfRange { at: PrimitiveRef, vertex: usize, value: f32 },
    #[error(
        "{at}: triangle {triangle} has _MATERIAL_ID {ids:?} at its corners; a triangle's vertices must \
         share one ID (split vertices at region borders, as a face-corner attribute does)"
    )]
    MixedMaterialIds { at: PrimitiveRef, triangle: usize, ids: [u8; 3] },
    #[error("{at}: index count {count} is not a multiple of 3")]
    NotTriangles { at: PrimitiveRef, count: usize },
    #[error("{at}: index {index} is out of range for {vertex_count} vertices")]
    IndexOutOfRange { at: PrimitiveRef, index: u32, vertex_count: usize },
    #[error("material {material}: invalid material extras")]
    MaterialExtras {
        material: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("skin {skin}: joint node {node} is not in the imported scene")]
    JointNotInScene { skin: usize, node: usize },
    #[error("skin {skin} has no joints")]
    SkinWithoutJoints { skin: usize },
    #[error("skin {skin}: the inverseBindMatrices accessor {fault}")]
    InverseBindAccessor { skin: usize, fault: AccessorFault },
    #[error("skin {skin}: {found} inverse bind matrices for {expected} joints")]
    InverseBindMatrices { skin: usize, found: usize, expected: usize },
    #[error("the skeleton has more than {MAX_JOINTS} joints or palette entries")]
    TooManyJoints,
    #[error("{at}: vertex {vertex} names joint {joint}, but its skin has {joints}")]
    JointOutOfRange { at: PrimitiveRef, vertex: usize, joint: u16, joints: usize },
    #[error("{at}: two morph targets are both named {name:?}")]
    DuplicateMorphTarget { at: PrimitiveRef, name: String },
    #[error("joint node {node}'s rest transform has no inverse")]
    DegenerateJoint { node: usize },
    #[error("the scene contains no triangles")]
    NoGeometry,
    #[error("the scene has more than u32::MAX vertices or indices")]
    TooLarge,
}

/// Imports a `.gltf` or `.glb` file. External buffers resolve relative to the file.
pub fn import(path: &Path) -> Result<ImportedMesh, GltfImportError> {
    let bytes =
        std::fs::read(path).map_err(|source| GltfImportError::Io { path: path.to_owned(), source })?;
    import_slice(&bytes, path.parent())
}

/// Imports glTF or GLB bytes. `base_dir` resolves relative buffer URIs; with `None`, only
/// self-contained files (GLB, or `data:` URIs) load.
pub fn import_slice(bytes: &[u8], base_dir: Option<&Path>) -> Result<ImportedMesh, GltfImportError> {
    let Gltf { document, blob } = parse(bytes).map_err(GltfImportError::Parse)?;
    let buffers = load_buffers(&document, base_dir, blob)?;
    import_scene(&document, &buffers, &[]).map(|scene| scene.mesh)
}

/// Whether the mesh of `bytes` is skinned, so its cook writes a skeleton too: any node of the scene
/// with a mesh and a skin or, for a source that cooks clips, any node its animations move
/// (`clips::animated_nodes`). The importer's skeleton exists exactly then.
pub fn has_skeleton(bytes: &[u8], clips: bool) -> Result<bool, GltfImportError> {
    let Gltf { document, .. } = parse(bytes).map_err(GltfImportError::Parse)?;
    if document.skins().len() == 0 && !clips {
        return Ok(false);
    }
    let scene =
        document.default_scene().or_else(|| document.scenes().next()).ok_or(GltfImportError::NoScene)?;
    let nodes = flatten(&document, &scene)?;
    let gltf_node =
        |visit: &SceneNode| document.nodes().nth(visit.node).expect("flattened from this document");
    let skinned = nodes.iter().map(gltf_node).any(|node| node.mesh().is_some() && node.skin().is_some());
    let moved = || {
        let animated = clips::animated_nodes(&document);
        nodes.iter().any(|visit| animated.binary_search(&visit.node).is_ok())
    };
    Ok(skinned || (clips && moved()))
}

/// Parses and validates a document as [`Gltf::from_slice`] does: every importer's way in. The
/// crate's validation panics on a primitive whose `POSITION` names no accessor (it indexes the
/// accessors with it, unchecked, to read its bounds), so that is checked first. It never visits an
/// animation channel's target, whose node and property the crate's readers then unwrap, so those
/// are checked here too.
pub(crate) fn parse(bytes: &[u8]) -> Result<Gltf, ::gltf::Error> {
    let Gltf { document, blob } = Gltf::from_slice_without_validation(bytes)?;
    let json = document.into_json();
    let mut faults = Vec::new();
    for (m, mesh) in json.meshes.iter().enumerate() {
        for (p, primitive) in mesh.primitives.iter().enumerate() {
            let position = primitive.attributes.get(&Checked::Valid(Semantic::Positions));
            if position.is_some_and(|accessor| accessor.value() >= json.accessors.len()) {
                let primitive =
                    ::gltf::json::Path::new().field("meshes").index(m).field("primitives").index(p);
                faults.push((
                    primitive.field("attributes").key("POSITION"),
                    validation::Error::IndexOutOfBounds,
                ));
            }
        }
    }
    for (a, animation) in json.animations.iter().enumerate() {
        for (c, channel) in animation.channels.iter().enumerate() {
            let target = || {
                let channel = ::gltf::json::Path::new().field("animations").index(a).field("channels");
                channel.index(c).field("target")
            };
            if channel.target.node.value() >= json.nodes.len() {
                faults.push((target().field("node"), validation::Error::IndexOutOfBounds));
            }
            if !matches!(channel.target.path, Checked::Valid(_)) {
                faults.push((target().field("path"), validation::Error::Invalid));
            }
        }
    }
    if !faults.is_empty() {
        return Err(::gltf::Error::Validation(faults));
    }
    Ok(Gltf { document: Document::from_json(json)?, blob })
}

/// A scene as imported, with where the importer put the document's nodes and materials: what
/// importers that build on this one (VRM, M1 A6) need to address the mesh by glTF index.
#[derive(Clone, Debug)]
pub(crate) struct SceneImport {
    pub mesh: ImportedMesh,
    /// The skeleton joint of each node that is one.
    pub joint_of_node: HashMap<usize, u16>,
    /// The material slot of each glTF material a primitive uses (`None`: glTF's default).
    pub slot_of_material: HashMap<Option<usize>, u32>,
    /// Per glTF mesh and morph target index, the merged targets its instances took, in
    /// increasing order: one, unless instances rest at different weights.
    pub targets_of_morph: HashMap<(usize, usize), Vec<u32>>,
}

/// Imports the default scene of a parsed document whose buffers are loaded. Nodes in
/// `extra_joints` join the skeleton, with their ancestors, as if a skin used them: a VRM's
/// spring bones, colliders and constraint targets may be nodes no vertex is weighted to. Nodes
/// that aren't in the scene are left out, for the caller to report.
pub(crate) fn import_scene(
    document: &Document,
    buffers: &[Vec<u8>],
    extra_joints: &[usize],
) -> Result<SceneImport, GltfImportError> {
    import_scene_within(document, buffers, extra_joints, SOURCE_BUDGET)
}

/// [`import_scene`], within `budget`.
fn import_scene_within(
    document: &Document,
    buffers: &[Vec<u8>],
    extra_joints: &[usize],
    budget: MeshBudget,
) -> Result<SceneImport, GltfImportError> {
    // Blender always writes `scene`; fall back to the first scene for files that don't.
    let scene =
        document.default_scene().or_else(|| document.scenes().next()).ok_or(GltfImportError::NoScene)?;
    // No axis conversion, deliberately: glTF 2.0 (§3.4) is +Y up, meters, with assets
    // facing +Z, and so is engine object space (as is VRM 1.0). Blender's exporter already
    // converts from its +Z-up space, so source coordinates pass through unchanged.
    let nodes = flatten(document, &scene)?;
    let skeleton = Skeleton::build(document, &nodes, buffers, extra_joints)?;
    let mut builder = Builder {
        buffers,
        mesh: ImportedMesh { lods: vec![Lod::default()], ..ImportedMesh::default() },
        materials: HashMap::new(),
        slot_names: slot_names(document),
        skeleton,
        targets: HashMap::new(),
        instances: HashMap::new(),
        budget,
        morph_deltas: 0,
    };
    for (instance, scene_node) in nodes.iter().enumerate() {
        let node = document.nodes().nth(scene_node.node).expect("flattened from this document");
        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                builder.add_primitive(&nodes, instance, &node, &mesh, &primitive)?;
            }
        }
    }
    let joint_of_node =
        builder.skeleton.as_ref().map(|skeleton| skeleton.joint_of.clone()).unwrap_or_default();
    let slot_of_material = builder.materials.clone();
    let targets_of_morph = builder
        .instances
        .iter()
        .map(|(&morph, instances)| {
            let mut targets: Vec<u32> = instances.iter().map(|&(_, target)| target as u32).collect();
            targets.sort_unstable();
            targets.dedup();
            (morph, targets)
        })
        .collect();
    Ok(SceneImport { mesh: builder.finish()?, joint_of_node, slot_of_material, targets_of_morph })
}

/// A scene's joints, parents first, and the joint of each node that is one.
pub(crate) type SceneJoints = (Vec<Joint>, HashMap<usize, u16>);

/// The skeleton [`import_scene`] would make, without its mesh, so a file's geometry can be left
/// unread (or be absent): the joints, parents first, and the joint of each node that is one.
/// `None` if nothing is skinned and `extra_joints` holds no node of the scene.
pub(crate) fn scene_joints(
    document: &Document,
    extra_joints: &[usize],
) -> Result<Option<SceneJoints>, GltfImportError> {
    let scene =
        document.default_scene().or_else(|| document.scenes().next()).ok_or(GltfImportError::NoScene)?;
    let nodes = flatten(document, &scene)?;
    let skeleton = Skeleton::joints(document, &nodes, extra_joints)?;
    Ok(skeleton.map(|(skeleton, _)| (skeleton.joints, skeleton.joint_of)))
}

/// One visit of a node in the scene's depth-first walk: its only one, since the nodes are trees.
struct SceneNode {
    node: usize,
    /// The visit of the parent node.
    parent: Option<usize>,
    world: Mat4,
}

/// The scene's nodes in depth-first order, children in glTF order, with world transforms. The
/// walk keeps its own stack, on which a node goes at most once: a hierarchy may be as deep as its
/// file is long.
fn flatten(document: &Document, scene: &Scene<'_>) -> Result<Vec<SceneNode>, GltfImportError> {
    let mut reached = vec![false; document.nodes().len()];
    let mut reach = |node: &Node<'_>| {
        if std::mem::replace(&mut reached[node.index()], true) {
            return Err(GltfImportError::NodeReachedTwice { node: node.index() });
        }
        Ok(())
    };
    let mut out: Vec<SceneNode> = Vec::new();
    // Each node's children go on in reverse, so its first child comes off next.
    let mut stack: Vec<(Node<'_>, Option<usize>)> = Vec::new();
    for node in scene.nodes() {
        reach(&node)?;
        stack.push((node, None));
    }
    stack.reverse();
    while let Some((node, parent)) = stack.pop() {
        let parent_world = parent.map_or(Mat4::IDENTITY, |parent| out[parent].world);
        let world = parent_world * Mat4::from_cols_array_2d(&node.transform().matrix());
        let visit_index = out.len();
        out.push(SceneNode { node: node.index(), parent, world });
        let first_child = stack.len();
        for child in node.children() {
            reach(&child)?;
            stack.push((child, Some(visit_index)));
        }
        stack[first_child..].reverse();
    }
    Ok(out)
}

/// The mesh's skeleton and palette while primitives are added.
struct Skeleton {
    joints: Vec<Joint>,
    /// Model-space rest transform per joint.
    rest_world: Vec<Mat4>,
    /// Skeleton joint of each node that is one.
    joint_of: HashMap<usize, u16>,
    binds: Vec<SkinBind>,
    /// Palette entry of each (joint, inverse bind matrix bits).
    bind_of: HashMap<(u16, [u32; 16]), u16>,
    /// Per glTF skin used: the palette entry of each of its joints, in skin order.
    skin_binds: HashMap<usize, Vec<u16>>,
    /// Per vertex added so far.
    weights: Vec<SkinWeights>,
}

impl Skeleton {
    /// The skeleton of every skin a scene node uses and of `extra_joints` (those in the scene),
    /// or `None` if no node is skinned and there are no extra joints.
    fn build(
        document: &Document,
        nodes: &[SceneNode],
        buffers: &[Vec<u8>],
        extra_joints: &[usize],
    ) -> Result<Option<Self>, GltfImportError> {
        let Some((mut skeleton, skins)) = Self::joints(document, nodes, extra_joints)? else {
            return Ok(None);
        };
        for skin in &skins {
            let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
            let matrices = inverse_bind_matrices(skin, joints.len(), buffers)?;
            let binds = joints
                .iter()
                .zip(matrices)
                .map(|(node, matrix)| skeleton.bind(skeleton.joint_of[node], matrix))
                .collect::<Result<_, _>>()?;
            skeleton.skin_binds.insert(skin.index(), binds);
        }
        Ok(Some(skeleton))
    }

    /// [`Self::build`]'s joints, reading no buffer, with the skins that use them. A source wanted
    /// for its animations stops here.
    fn joints<'a>(
        document: &'a Document,
        nodes: &[SceneNode],
        extra_joints: &[usize],
    ) -> Result<Option<(Self, Vec<::gltf::Skin<'a>>)>, GltfImportError> {
        let gltf_node = |index: usize| document.nodes().nth(index).expect("flattened from this document");
        let mut skins: Vec<::gltf::Skin<'_>> = Vec::new();
        for scene_node in nodes {
            let node = gltf_node(scene_node.node);
            if let (Some(_), Some(skin)) = (node.mesh(), node.skin())
                && !skins.iter().any(|known| known.index() == skin.index())
            {
                skins.push(skin);
            }
        }
        let visit_of: HashMap<usize, usize> =
            nodes.iter().enumerate().map(|(visit, scene_node)| (scene_node.node, visit)).collect();
        let extra_visits: Vec<usize> =
            extra_joints.iter().filter_map(|node| visit_of.get(node).copied()).collect();
        if skins.is_empty() && extra_visits.is_empty() {
            return Ok(None);
        }
        // Every joint and its ancestors, stopping at the first already in: otherwise many joints
        // deep in a hierarchy would each walk it.
        let mut included = vec![false; nodes.len()];
        let mut include = |mut visit: Option<usize>| {
            while let Some(current) = visit.filter(|&current| !included[current]) {
                included[current] = true;
                visit = nodes[current].parent;
            }
        };
        for skin in &skins {
            if skin.joints().next().is_none() {
                return Err(GltfImportError::SkinWithoutJoints { skin: skin.index() });
            }
            for joint in skin.joints() {
                include(Some(
                    *visit_of.get(&joint.index()).ok_or(GltfImportError::JointNotInScene {
                        skin: skin.index(),
                        node: joint.index(),
                    })?,
                ));
            }
        }
        for &visit in &extra_visits {
            include(Some(visit));
        }
        let mut skeleton = Self {
            joints: Vec::new(),
            rest_world: Vec::new(),
            joint_of: HashMap::new(),
            binds: Vec::new(),
            bind_of: HashMap::new(),
            skin_binds: HashMap::new(),
            weights: Vec::new(),
        };
        for (visit, scene_node) in nodes.iter().enumerate() {
            if !included[visit] {
                continue;
            }
            let index = u16::try_from(skeleton.joints.len()).map_err(|_| GltfImportError::TooManyJoints)?;
            let node = gltf_node(scene_node.node);
            let parent = scene_node.parent.map(|parent| skeleton.joint_of[&nodes[parent].node]);
            let (translation, rotation, scale) = node.transform().decomposed();
            skeleton.joints.push(Joint {
                name: node.name().map_or_else(|| format!("#{}", node.index()), str::to_owned),
                parent,
                rest: JointTransform { translation, rotation, scale },
            });
            skeleton.rest_world.push(scene_node.world);
            skeleton.joint_of.insert(scene_node.node, index);
        }
        Ok(Some((skeleton, skins)))
    }

    /// The palette entry for `joint` with `inverse_bind_matrix`, added if new.
    fn bind(&mut self, joint: u16, inverse_bind_matrix: [[f32; 4]; 4]) -> Result<u16, GltfImportError> {
        let bits: [u32; 16] = bytemuck::cast(inverse_bind_matrix.map(|column| column.map(f32::to_bits)));
        if let Some(&bind) = self.bind_of.get(&(joint, bits)) {
            return Ok(bind);
        }
        let bind = u16::try_from(self.binds.len()).map_err(|_| GltfImportError::TooManyJoints)?;
        self.binds.push(SkinBind { joint, inverse_bind_matrix });
        self.bind_of.insert((joint, bits), bind);
        Ok(bind)
    }

    /// The palette entry that binds a static part at scene visit `visit` rigidly: to its
    /// nearest ancestor joint (or itself), else joint 0.
    fn rigid_bind(&mut self, nodes: &[SceneNode], visit: usize) -> Result<u16, GltfImportError> {
        let mut current = Some(visit);
        let joint = loop {
            match current {
                Some(at) => match self.joint_of.get(&nodes[at].node) {
                    Some(&joint) => break joint,
                    None => current = nodes[at].parent,
                },
                None => break 0,
            }
        };
        let inverse = self.rest_world[usize::from(joint)].inverse();
        if !inverse.is_finite() {
            let node = self.joint_of.iter().find(|&(_, &j)| j == joint).map_or(0, |(&node, _)| node);
            return Err(GltfImportError::DegenerateJoint { node });
        }
        self.bind(joint, inverse.to_cols_array_2d())
    }
}

/// The inverse bind matrix of each of `skin`'s `joints`: identities if it names none.
fn inverse_bind_matrices(
    skin: &::gltf::Skin<'_>,
    joints: usize,
    buffers: &[Vec<u8>],
) -> Result<Vec<[[f32; 4]; 4]>, GltfImportError> {
    let Some(accessor) = skin.inverse_bind_matrices() else {
        return Ok(vec![Mat4::IDENTITY.to_cols_array_2d(); joints]);
    };
    accessors::check(&accessor, &accessors::INVERSE_BIND_MATRICES, buffers)
        .map_err(|fault| GltfImportError::InverseBindAccessor { skin: skin.index(), fault })?;
    if accessor.count() != joints {
        return Err(GltfImportError::InverseBindMatrices {
            skin: skin.index(),
            found: accessor.count(),
            expected: joints,
        });
    }
    let reader = skin.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
    Ok(reader.read_inverse_bind_matrices().expect("a checked accessor reads").collect())
}

/// Loads every buffer of `document`: the GLB binary chunk, base64 `data:` URIs, or files
/// relative to `base_dir`. This is `gltf::import_buffers`, whose `import` feature would also
/// build an image decoder (`image`) the pipeline doesn't use until textures arrive in M1.
pub(crate) fn load_buffers(
    document: &Document,
    base_dir: Option<&Path>,
    mut blob: Option<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, GltfImportError> {
    let problem = |index, problem: String| GltfImportError::Buffer { index, problem };
    document
        .buffers()
        .map(|buffer| {
            let index = buffer.index();
            let mut data = match buffer.source() {
                Source::Bin => {
                    blob.take().ok_or_else(|| problem(index, "needs the GLB binary chunk".into()))?
                }
                Source::Uri(uri) => match uri.strip_prefix("data:") {
                    Some(data_uri) => decode_data_uri(data_uri)
                        .ok_or_else(|| problem(index, "has a malformed base64 data URI".into()))?,
                    None => {
                        let base = base_dir.ok_or_else(|| {
                            problem(
                                index,
                                format!("is the external file {uri:?}, but there is no base directory"),
                            )
                        })?;
                        let path = base.join(external_file(index, uri)?);
                        std::fs::read(&path).map_err(|source| GltfImportError::Io { path, source })?
                    }
                },
            };
            if data.len() < buffer.length() {
                return Err(problem(
                    index,
                    format!("holds {} bytes but declares {}", data.len(), buffer.length()),
                ));
            }
            // Pad to whole words, as `gltf::import_buffers` does: accessors may read the last one.
            data.resize(data.len().next_multiple_of(4), 0);
            Ok(data)
        })
        .collect()
}

/// The files a glTF or GLB document reads besides itself (its external buffers), as paths
/// relative to its directory, sorted and without duplicates.
pub fn external_files(bytes: &[u8]) -> Result<Vec<String>, GltfImportError> {
    let Gltf { document, .. } = parse(bytes).map_err(GltfImportError::Parse)?;
    let mut files = document
        .buffers()
        .filter_map(|buffer| match buffer.source() {
            Source::Uri(uri) if !uri.starts_with("data:") => Some(external_file(buffer.index(), uri)),
            Source::Uri(_) | Source::Bin => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    files.dedup();
    Ok(files)
}

/// The file a buffer URI other than `data:` names, relative to the document.
fn external_file(index: usize, uri: &str) -> Result<String, GltfImportError> {
    percent_decode(uri)
        .ok_or_else(|| GltfImportError::Buffer { index, problem: format!("has a malformed URI {uri:?}") })
}

/// The part of a `data:` URI after the scheme: `[<media type>];base64,<payload>`. Exporters only
/// write base64 payloads.
fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    let (header, payload) = uri.split_once(',')?;
    if !header.ends_with(";base64") {
        return None;
    }
    let sextet = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let payload = payload.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(payload.len() * 3 / 4);
    let (mut bits, mut pending) = (0u32, 0);
    for &c in payload {
        bits = (bits << 6) | u32::from(sextet(c)?);
        pending += 6;
        if pending >= 8 {
            pending -= 8;
            out.push((bits >> pending) as u8);
            bits &= (1 << pending) - 1;
        }
    }
    // A lone trailing character (length 1 mod 4) can't encode a byte.
    (pending < 6).then_some(out)
}

/// Decodes `%XX` escapes: glTF URIs follow RFC 3986, so a space in a file name arrives as `%20`.
fn percent_decode(uri: &str) -> Option<String> {
    let mut out = Vec::with_capacity(uri.len());
    let mut bytes = uri.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(byte);
        }
    }
    String::from_utf8(out).ok()
}

/// Anime character parameters authored as glTF material extras. Unknown keys are ignored,
/// because Blender exports every custom property on the material.
#[derive(Debug, Default, Deserialize)]
struct MaterialExtras {
    #[serde(default, deserialize_with = "linear_rgba")]
    shade_color: Option<[f32; 4]>,
    #[serde(default, deserialize_with = "linear_rgba")]
    outline_color: Option<[f32; 4]>,
    /// `[light-split threshold, softness, rim strength, outline width]`, as the sample
    /// mannequin carries them.
    character_shading: Option<[f32; 4]>,
    /// The depth-offset rim's settings (M1 C5), any of them.
    #[serde(default)]
    rim: RimExtras,
    /// Per region from region 0: `[rim strength, rim width, rim overlap]` scales (M1 C5).
    #[serde(default)]
    rim_regions: Vec<[f32; 3]>,
}

/// The `rim` material extra: any of [`Rim`](sapphire_assets::material::Rim)'s settings. Its
/// strength wins over `character_shading`'s.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RimExtras {
    width: Option<f32>,
    strength: Option<f32>,
    threshold: Option<f32>,
    fresnel: Option<f32>,
    light_gate: Option<f32>,
    back_light: Option<f32>,
}

/// Accepts RGB (opaque) or RGBA, since Blender color properties can be either.
fn linear_rgba<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<[f32; 4]>, D::Error> {
    match Vec::<f32>::deserialize(deserializer)?[..] {
        [r, g, b] => Ok(Some([r, g, b, 1.0])),
        [r, g, b, a] => Ok(Some([r, g, b, a])),
        ref other => Err(D::Error::invalid_length(other.len(), &"3 (RGB) or 4 (RGBA) numbers")),
    }
}

/// Mesh `extras` the importer reads: Blender's shape key names.
#[derive(Debug, Default, Deserialize)]
struct MeshExtras {
    #[serde(default, rename = "targetNames")]
    target_names: Vec<String>,
}

/// The names `mesh`'s extras give its morph targets, by index.
pub(crate) fn morph_target_names(mesh: &Mesh<'_>) -> Vec<String> {
    mesh.extras()
        .as_ref()
        .and_then(|raw| serde_json::from_str::<MeshExtras>(raw.get()).ok())
        .unwrap_or_default()
        .target_names
}

/// The name morph target `k` of `mesh` is imported (and merged) under: its name from the
/// mesh's extras (`names`, [`morph_target_names`]), or `<mesh>/<k>`.
pub(crate) fn morph_target_name(mesh: &Mesh<'_>, names: &[String], k: usize) -> String {
    names.get(k).cloned().unwrap_or_else(|| {
        format!("{}/{k}", mesh.name().map_or_else(|| format!("#{}", mesh.index()), str::to_owned))
    })
}

struct Builder<'a> {
    buffers: &'a [Vec<u8>],
    mesh: ImportedMesh,
    /// glTF material index (`None` = the default material) → cooked material slot.
    materials: HashMap<Option<usize>, u32>,
    /// Slot names by glTF material index.
    slot_names: Vec<String>,
    skeleton: Option<Skeleton>,
    /// Morph target name → index into `mesh.morph_targets`.
    targets: HashMap<String, usize>,
    /// Per glTF mesh and morph target index: each default weight (its bits) the mesh's instances
    /// give the target, and the merged target that takes it.
    instances: HashMap<(usize, usize), Vec<(u32, usize)>>,
    budget: MeshBudget,
    /// In `mesh.morph_targets`, all together.
    morph_deltas: usize,
}

/// A primitive's attributes, read and transformed into the mesh's space.
struct PrimitiveData {
    positions: Vec<Vec3>,
    /// Transformed and normalized.
    normals: Option<Vec<Vec3>>,
    /// As the file stores them, for morph targets' normal deltas.
    source_normals: Option<Vec<Vec3>>,
    uvs: Option<Vec<[f32; 2]>>,
    colors: Option<Vec<[u8; 3]>>,
    material_ids: Option<Vec<MaterialId>>,
    /// Normalized.
    outline_normals: Option<Vec<Vec3>>,
}

impl Builder<'_> {
    fn add_primitive(
        &mut self,
        nodes: &[SceneNode],
        visit: usize,
        node: &Node<'_>,
        mesh: &Mesh<'_>,
        primitive: &Primitive<'_>,
    ) -> Result<(), GltfImportError> {
        let at = PrimitiveRef::new(mesh, primitive);
        if primitive.mode() != Mode::Triangles {
            return Err(GltfImportError::UnsupportedMode { at, mode: primitive.mode() });
        }
        let buffers = self.buffers;
        let get_buffer = |buffer: ::gltf::Buffer<'_>| buffers.get(buffer.index()).map(Vec::as_slice);
        let reader = primitive.reader(get_buffer);
        let skin = node.skin().filter(|_| self.skeleton.is_some());
        // Skinned vertices stay in bind space (glTF ignores the skinned node's transform).
        let transform = if skin.is_some() { Mat4::IDENTITY } else { nodes[visit].world };
        let normal_matrix = Mat3::from_mat4(transform).inverse().transpose();

        let Some(position_accessor) = primitive.get(&Semantic::Positions) else {
            return Err(GltfImportError::MissingPositions { at });
        };
        check_accessor(&at, "POSITION", &position_accessor, &accessors::FLOAT_VEC3, buffers)?;
        let vertex_count = position_accessor.count();
        within_budget(&at, "vertices", vertex_count, self.budget.vertices)?;
        let Some(positions) = reader.read_positions() else {
            return Err(GltfImportError::UnreadableAccessor { at, semantic: "POSITION".into() });
        };
        let positions: Vec<Vec3> = positions.map(|p| transform.transform_point3(Vec3::from(p))).collect();
        if let Some(vertex) = positions.iter().position(|position| !position.is_finite()) {
            let position = positions[vertex].to_array();
            return Err(GltfImportError::NonFinitePosition { at, vertex, position });
        }
        let attributes = Attributes { at: &at, buffers, vertex_count };
        let transform_normal = |n: Vec3| (normal_matrix * n).normalize_or(Vec3::Y);
        let source_normals =
            attributes.read("NORMAL", primitive.get(&Semantic::Normals), &accessors::FLOAT_VEC3, |_| {
                reader.read_normals().map(|iter| iter.map(Vec3::from).collect::<Vec<_>>())
            })?;
        let data = PrimitiveData {
            normals: source_normals
                .as_ref()
                .map(|normals| normals.iter().copied().map(transform_normal).collect()),
            source_normals,
            uvs: attributes.read(
                "TEXCOORD_0",
                primitive.get(&Semantic::TexCoords(0)),
                &accessors::TEX_COORD,
                |_| reader.read_tex_coords(0).map(|iter| iter.into_f32().collect()),
            )?,
            colors: attributes.read(
                "COLOR_0",
                primitive.get(&Semantic::Colors(0)),
                &accessors::COLOR,
                |_| reader.read_colors(0).map(|iter| iter.into_rgba_f32().map(vertex_color).collect()),
            )?,
            material_ids: read_material_ids(&attributes, primitive, &get_buffer)?,
            outline_normals: read_outline_normals(&attributes, primitive, &get_buffer)?
                .map(|normals| normals.into_iter().map(transform_normal).collect()),
            positions,
        };

        let index_count = match primitive.indices() {
            Some(accessor) => {
                check_accessor(&at, "indices", &accessor, &accessors::INDICES, buffers)?;
                accessor.count()
            }
            None => vertex_count,
        };
        let total_indices = self.mesh.indices.len().saturating_add(index_count);
        within_budget(&at, "indices", total_indices, self.budget.indices)?;
        let indices: Vec<u32> = match (primitive.indices(), reader.read_indices()) {
            (_, Some(iter)) => iter.into_u32().collect(),
            (Some(_), None) => {
                return Err(GltfImportError::UnreadableAccessor {
                    at: at.clone(),
                    semantic: "indices".into(),
                });
            }
            (None, None) => {
                (0..u32::try_from(vertex_count).map_err(|_| GltfImportError::TooLarge)?).collect()
            }
        };
        if !indices.len().is_multiple_of(3) {
            return Err(GltfImportError::NotTriangles { at: at.clone(), count: indices.len() });
        }
        if let Some(&index) = indices.iter().find(|&&i| i as usize >= vertex_count) {
            return Err(GltfImportError::IndexOutOfRange { at: at.clone(), index, vertex_count });
        }
        // Flat shading needs a vertex per face corner.
        let new_vertices = if data.normals.is_some() { vertex_count } else { indices.len() };
        within_budget(&at, "vertices", self.mesh.vertices.len() + new_vertices, self.budget.vertices)?;

        // A negative determinant mirrors the geometry, which turns counter-clockwise front
        // faces clockwise; swap two corners to restore the winding (glTF 2.0 §3.7.4).
        let mirrored = transform.determinant() < 0.0;
        let triangles: Vec<[u32; 3]> = indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[a, b, c]| if mirrored { [a, c, b] } else { [a, b, c] })
            .collect();
        if let Some(ids) = &data.material_ids {
            let corner_ids = |triangle: &[u32; 3]| triangle.map(|i| ids[i as usize].get());
            if let Some((triangle, corners)) =
                triangles.iter().map(corner_ids).enumerate().find(|(_, [a, b, c])| a != b || b != c)
            {
                return Err(GltfImportError::MixedMaterialIds { at: at.clone(), triangle, ids: corners });
            }
        }

        // `sources[i]` is the primitive vertex that new vertex `i` comes from, and `normals[i]`
        // its shading normal.
        let (sources, normals, local_indices): (Vec<u32>, Vec<Vec3>, Vec<u32>) = match &data.normals {
            Some(normals) => ((0..vertex_count as u32).collect(), normals.clone(), triangles.concat()),
            None => {
                // Flat shading needs a normal per face corner, so the primitive is unwelded.
                let mut sources = Vec::with_capacity(3 * triangles.len());
                let mut normals = Vec::with_capacity(3 * triangles.len());
                for &[a, b, c] in &triangles {
                    let [pa, pb, pc] = [a, b, c].map(|i| data.positions[i as usize]);
                    let normal = (pb - pa).cross(pc - pa).normalize_or(Vec3::Y);
                    sources.extend([a, b, c]);
                    normals.extend([normal; 3]);
                }
                let corners = (0..sources.len() as u32).collect();
                (sources, normals, corners)
            }
        };

        let base = u32::try_from(self.mesh.vertices.len()).map_err(|_| GltfImportError::TooLarge)?;
        let first_index = u32::try_from(self.mesh.indices.len()).map_err(|_| GltfImportError::TooLarge)?;
        for (&source, &normal) in sources.iter().zip(&normals) {
            let s = source as usize;
            let outline = data.outline_normals.as_ref().map_or(normal, |outline| outline[s]);
            self.mesh.vertices.push(Vertex {
                position: data.positions[s].to_array(),
                normal: normal.to_array(),
                outline_normal: outline.to_array(),
                uv: data.uvs.as_ref().map_or([0.0; 2], |uvs| uvs[s]),
                color: data.colors.as_ref().map_or(VERTEX_COLOR_NEUTRAL, |colors| colors[s]),
                flags: data
                    .material_ids
                    .as_ref()
                    .map_or(VertexFlags::NONE, |ids| VertexFlags::with_material_id(ids[s])),
            });
        }
        let authored = data.outline_normals.is_some();
        self.mesh.authored_outline.extend(std::iter::repeat_n(authored, sources.len()));
        self.mesh.indices.extend(local_indices.iter().map(|i| base + i));
        u32::try_from(self.mesh.indices.len()).map_err(|_| GltfImportError::TooLarge)?;

        if let Some(skeleton) = self.skeleton.as_mut() {
            let weights = match &skin {
                Some(skin) => read_weights(
                    &attributes,
                    primitive,
                    &reader,
                    &skeleton.skin_binds[&skin.index()],
                    self.budget.influence_sets,
                )?,
                None => vec![SkinWeights::rigid(skeleton.rigid_bind(nodes, visit)?); vertex_count],
            };
            skeleton.weights.extend(sources.iter().map(|&s| weights[s as usize]));
        }
        self.add_morph_targets(
            node,
            mesh,
            primitive,
            &get_buffer,
            &data,
            transform,
            &sources,
            base,
            &attributes,
        )?;

        let index_count = u32::try_from(self.mesh.indices.len()).map_err(|_| GltfImportError::TooLarge)?;
        let material = self.material_slot(&primitive.material())?;
        self.mesh.lods[0].submeshes.push(Submesh {
            first_index,
            index_count: index_count - first_index,
            material,
        });
        Ok(())
    }

    /// Adds the primitive's morph targets' deltas for its new vertices (`base..`, from
    /// `sources`), merging targets by name.
    #[expect(clippy::too_many_arguments, reason = "one primitive's state, used once")]
    fn add_morph_targets<'a, 's, F>(
        &mut self,
        node: &Node<'_>,
        mesh: &Mesh<'_>,
        primitive: &Primitive<'a>,
        get_buffer: &F,
        data: &PrimitiveData,
        transform: Mat4,
        sources: &[u32],
        base: u32,
        attributes: &Attributes<'_>,
    ) -> Result<(), GltfImportError>
    where
        F: Clone + Fn(::gltf::Buffer<'a>) -> Option<&'s [u8]>,
    {
        let at = attributes.at;
        let targets: Vec<_> = primitive.morph_targets().collect();
        if targets.is_empty() {
            return Ok(());
        }
        let names = morph_target_names(mesh);
        let defaults = node.weights().or(mesh.weights()).unwrap_or_default();
        let linear = Mat3::from_mat4(transform);
        let normal_matrix = linear.inverse().transpose();
        // The names of the targets this primitive has written to.
        let mut used = Vec::with_capacity(targets.len());
        for (k, target) in targets.into_iter().enumerate() {
            let read = |what: &str, accessor: Option<Accessor<'a>>| {
                attributes.read(
                    &format!("morph target {k} {what}"),
                    accessor,
                    &accessors::FLOAT_VEC3,
                    |accessor| {
                        ::gltf::accessor::Iter::<[f32; 3]>::new(accessor, get_buffer.clone())
                            .map(|iter| iter.collect::<Vec<_>>())
                    },
                )
            };
            let positions = read("POSITION", target.positions())?;
            let normal_deltas = read("NORMAL", target.normals())?;
            let name = morph_target_name(mesh, &names, k);
            // Two of one primitive's targets under one name would move the same vertices twice.
            if used.contains(&name) {
                return Err(GltfImportError::DuplicateMorphTarget { at: at.clone(), name });
            }
            let target = self.morph_target(node, mesh, k, &name, defaults.get(k).copied().unwrap_or(0.0));
            used.push(name);
            for (offset, &source) in sources.iter().enumerate() {
                let s = source as usize;
                let position = positions.as_ref().map_or(Vec3::ZERO, |p| linear * Vec3::from(p[s]));
                // Normals are per face corner without a NORMAL attribute; they don't morph then.
                let normal = match (&data.normals, &data.source_normals, &normal_deltas) {
                    (Some(normals), Some(source_normals), Some(deltas)) => {
                        // The difference of the transformed, normalized full-weight and base
                        // normals: exact at full weight once the shader renormalizes.
                        let morphed = normal_matrix * (source_normals[s] + Vec3::from(deltas[s]));
                        morphed.normalize_or(normals[s]) - normals[s]
                    }
                    _ => Vec3::ZERO,
                };
                if position.abs().max_element() < MORPH_POSITION_EPSILON
                    && normal.abs().max_element() < MORPH_NORMAL_EPSILON
                {
                    continue;
                }
                self.morph_deltas += 1;
                within_budget(at, "morph deltas", self.morph_deltas, self.budget.morph_deltas)?;
                self.mesh.morph_targets[target].deltas.push(ImportedDelta {
                    vertex: base + offset as u32,
                    position: position.to_array(),
                    normal: normal.to_array(),
                });
            }
        }
        Ok(())
    }

    /// The merged target that morph target `k` (named `name`) of `mesh`, instanced by `node` at
    /// rest at `default`, adds its deltas to, made if there is none yet (see the module docs):
    /// the target its mesh's instances at that default share, else for the mesh's first
    /// instance the target of that name, else a target of its own.
    fn morph_target(
        &mut self,
        node: &Node<'_>,
        mesh: &Mesh<'_>,
        k: usize,
        name: &str,
        default: f32,
    ) -> usize {
        // `+ 0.0` makes -0 and 0 one default.
        let bits = (default + 0.0).to_bits();
        let instances = self.instances.entry((mesh.index(), k)).or_default();
        if let Some(&(_, target)) = instances.iter().find(|&&(seen, _)| seen == bits) {
            return target;
        }
        let name = if instances.is_empty() {
            name.to_owned()
        } else {
            // Another instance of the mesh, at rest at another weight: one target can't hold both
            // defaults, so this instance's is a target of its own, named after its node.
            let node = node.name().map_or_else(|| format!("#{}", node.index()), str::to_owned);
            let split = format!("{name}@{node}");
            let mut unique = split.clone();
            for n in 2.. {
                if !self.targets.contains_key(&unique) {
                    break;
                }
                unique = format!("{split}#{n}");
            }
            unique
        };
        let target = *self.targets.entry(name.clone()).or_insert_with(|| {
            self.mesh.morph_targets.push(ImportedMorphTarget {
                name,
                default_weight: default,
                deltas: Vec::new(),
            });
            self.mesh.morph_targets.len() - 1
        });
        self.instances.entry((mesh.index(), k)).or_default().push((bits, target));
        target
    }

    fn material_slot(&mut self, material: &Material<'_>) -> Result<u32, GltfImportError> {
        if let Some(&slot) = self.materials.get(&material.index()) {
            return Ok(slot);
        }
        let slot = u32::try_from(self.mesh.materials.len()).map_err(|_| GltfImportError::TooLarge)?;
        let name =
            material.index().map_or_else(|| DEFAULT_SLOT.to_owned(), |index| self.slot_names[index].clone());
        self.mesh.materials.push(ImportedMaterial { name, generated: generated_material(material)? });
        self.materials.insert(material.index(), slot);
        Ok(slot)
    }

    fn finish(mut self) -> Result<ImportedMesh, GltfImportError> {
        if self.mesh.indices.is_empty() {
            return Err(GltfImportError::NoGeometry);
        }
        self.mesh.lods[0].vertex_count =
            u32::try_from(self.mesh.vertices.len()).map_err(|_| GltfImportError::TooLarge)?;
        if let Some(skeleton) = self.skeleton {
            if skeleton.binds.len() > MAX_JOINTS {
                return Err(GltfImportError::TooManyJoints);
            }
            self.mesh.skin = Some(ImportedSkin {
                joints: skeleton.joints,
                binds: skeleton.binds,
                weights: skeleton.weights,
            });
        }
        Ok(self.mesh)
    }
}

/// The custom attribute holding authored outline normals, without glTF's leading underscore
/// (`gltf` strips it).
const OUTLINE_NORMAL: &str = "OUTLINE_NORMAL";
/// The custom attribute holding per-vertex material IDs, likewise.
const MATERIAL_ID: &str = "MATERIAL_ID";

/// Fails if `found` of `what` is over `budget`.
fn within_budget(
    at: &PrimitiveRef,
    what: &'static str,
    found: usize,
    budget: usize,
) -> Result<(), GltfImportError> {
    if found > budget {
        return Err(GltfImportError::OverBudget { at: at.clone(), what, found, budget });
    }
    Ok(())
}

/// [`accessors::check`], naming the primitive and semantic the accessor belongs to.
fn check_accessor(
    at: &PrimitiveRef,
    semantic: &str,
    accessor: &Accessor<'_>,
    kind: &accessors::Kind,
    buffers: &[Vec<u8>],
) -> Result<(), GltfImportError> {
    accessors::check(accessor, kind, buffers).map_err(|fault| GltfImportError::Accessor {
        at: at.clone(),
        semantic: semantic.to_owned(),
        fault,
    })
}

/// One primitive's per-vertex accessors, which nothing reads until they pass these checks.
struct Attributes<'p> {
    at: &'p PrimitiveRef,
    buffers: &'p [Vec<u8>],
    /// `POSITION`'s count.
    vertex_count: usize,
}

impl Attributes<'_> {
    /// Reads a per-vertex attribute, if the primitive has it (`accessor`), once it can be read as
    /// `kind`, with an element per vertex.
    fn read<'a, T>(
        &self,
        semantic: &str,
        accessor: Option<Accessor<'a>>,
        kind: &accessors::Kind,
        read: impl FnOnce(Accessor<'a>) -> Option<Vec<T>>,
    ) -> Result<Option<Vec<T>>, GltfImportError> {
        let Some(accessor) = accessor else {
            return Ok(None);
        };
        check_accessor(self.at, semantic, &accessor, kind, self.buffers)?;
        if accessor.count() != self.vertex_count {
            return Err(GltfImportError::AttributeCount {
                at: self.at.clone(),
                semantic: semantic.to_owned(),
                found: accessor.count(),
                expected: self.vertex_count,
            });
        }
        read(accessor).map(Some).ok_or_else(|| GltfImportError::UnreadableAccessor {
            at: self.at.clone(),
            semantic: semantic.to_owned(),
        })
    }
}

/// The primitive's `_OUTLINE_NORMAL` attribute, untransformed, if it has one.
fn read_outline_normals<'a, 's, F>(
    attributes: &Attributes<'_>,
    primitive: &Primitive<'a>,
    get_buffer: &F,
) -> Result<Option<Vec<Vec3>>, GltfImportError>
where
    F: Clone + Fn(::gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    // Normalized signed integers decode as max(q / max, -1) (glTF 2.0 §3.11).
    fn snorm<T: Into<f32>>(values: [T; 3], max: f32) -> Vec3 {
        Vec3::from(values.map(|c| (c.into() / max).max(-1.0)))
    }
    let accessor = primitive.get(&Semantic::Extras(OUTLINE_NORMAL.to_owned()));
    attributes.read("_OUTLINE_NORMAL", accessor, &accessors::OUTLINE_NORMAL, |accessor| {
        match accessor.data_type() {
            DataType::I16 => ::gltf::accessor::Iter::<[i16; 3]>::new(accessor, get_buffer.clone())
                .map(|iter| iter.map(|v| snorm(v, 32767.0)).collect()),
            DataType::I8 => ::gltf::accessor::Iter::<[i8; 3]>::new(accessor, get_buffer.clone())
                .map(|iter| iter.map(|v| snorm(v, 127.0)).collect()),
            _ => ::gltf::accessor::Iter::<[f32; 3]>::new(accessor, get_buffer.clone())
                .map(|iter| iter.map(Vec3::from).collect()),
        }
    })
}

/// The primitive's `_MATERIAL_ID` attribute, if it has one.
fn read_material_ids<'a, 's, F>(
    attributes: &Attributes<'_>,
    primitive: &Primitive<'a>,
    get_buffer: &F,
) -> Result<Option<Vec<MaterialId>>, GltfImportError>
where
    F: Clone + Fn(::gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    let at = attributes.at;
    let accessor = primitive.get(&Semantic::Extras(MATERIAL_ID.to_owned()));
    let values: Option<Vec<f32>> =
        attributes.read("_MATERIAL_ID", accessor, &accessors::MATERIAL_ID, |accessor| {
            match accessor.data_type() {
                DataType::U8 => ::gltf::accessor::Iter::<u8>::new(accessor, get_buffer.clone())
                    .map(|iter| iter.map(f32::from).collect()),
                DataType::U16 => ::gltf::accessor::Iter::<u16>::new(accessor, get_buffer.clone())
                    .map(|iter| iter.map(f32::from).collect()),
                _ => ::gltf::accessor::Iter::<f32>::new(accessor, get_buffer.clone()).map(Iterator::collect),
            }
        })?;
    let Some(values) = values else {
        return Ok(None);
    };
    values
        .iter()
        .enumerate()
        .map(|(vertex, &value)| {
            // Whole numbers only: an ID can't blend, and a fraction means the attribute holds
            // something else.
            let id = (value.fract() == 0.0 && (0.0..f32::from(MATERIAL_IDS)).contains(&value))
                .then(|| MaterialId::new(value as u8))
                .flatten();
            id.ok_or(GltfImportError::MaterialIdOutOfRange { at: at.clone(), vertex, value })
        })
        .collect::<Result<_, _>>()
        .map(Some)
}

/// Every vertex's influences from all `JOINTS_n`/`WEIGHTS_n` sets, as palette entries through
/// `binds` (the skin's joints' entries), reduced to the four heaviest.
fn read_weights<'a, 's, F>(
    attributes: &Attributes<'_>,
    primitive: &Primitive<'a>,
    reader: &::gltf::mesh::Reader<'a, 's, F>,
    binds: &[u16],
    influence_sets: usize,
) -> Result<Vec<SkinWeights>, GltfImportError>
where
    F: Clone + Fn(::gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    let (at, vertex_count) = (attributes.at, attributes.vertex_count);
    let mut influences: Vec<Vec<(u16, f32)>> = vec![Vec::new(); vertex_count];
    for set in 0.. {
        let Some(joints) = primitive.get(&Semantic::Joints(set)) else { break };
        let sets = (set as usize + 1).saturating_mul(vertex_count);
        within_budget(at, "vertices × JOINTS sets", sets, influence_sets)?;
        let (joints_name, weights_name) = (format!("JOINTS_{set}"), format!("WEIGHTS_{set}"));
        let weights = primitive.get(&Semantic::Weights(set)).ok_or_else(|| {
            GltfImportError::UnreadableAccessor { at: at.clone(), semantic: weights_name.clone() }
        })?;
        let joints = attributes
            .read(&joints_name, Some(joints), &accessors::JOINTS, |_| {
                reader.read_joints(set).map(|iter| iter.into_u16().collect::<Vec<_>>())
            })?
            .expect("an accessor that's there reads or fails");
        let weights = attributes
            .read(&weights_name, Some(weights), &accessors::WEIGHTS, |_| {
                reader.read_weights(set).map(|iter| iter.into_f32().collect::<Vec<_>>())
            })?
            .expect("an accessor that's there reads or fails");
        for (vertex, (joints, weights)) in joints.iter().zip(&weights).enumerate() {
            for (&joint, &weight) in joints.iter().zip(weights) {
                // Exporters pad unused slots with weight 0 and any joint; only real ones count.
                if weight.is_nan() || weight <= 0.0 {
                    continue;
                }
                let bind = *binds.get(usize::from(joint)).ok_or_else(|| {
                    GltfImportError::JointOutOfRange { at: at.clone(), vertex, joint, joints: binds.len() }
                })?;
                influences[vertex].push((bind, weight));
            }
        }
    }
    // A skin has at least one joint (`Skeleton::build` checks), whose entry takes unweighted
    // vertices.
    let fallback = SkinWeights::rigid(binds.first().copied().unwrap_or(0));
    Ok(influences.into_iter().map(|list| SkinWeights::strongest(list).unwrap_or(fallback)).collect())
}

/// glTF `COLOR_0` is linear, as the cooked vertex color's data channels are. Alpha is unused.
fn vertex_color([r, g, b, _]: [f32; 4]) -> [u8; 3] {
    [unorm8(r), unorm8(g), unorm8(b)]
}

/// Every material's slot name, by glTF material index ([`import::slot_names`]).
fn slot_names(document: &Document) -> Vec<String> {
    let names: Vec<Option<&str>> = document.materials().map(|material| material.name()).collect();
    import::slot_names(&names)
}

/// A glTF material's `alphaMode` and `alphaCutoff`.
pub(crate) fn alpha_mode(material: &Material<'_>) -> AlphaMode {
    match material.alpha_mode() {
        ::gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
        ::gltf::material::AlphaMode::Mask => {
            // glTF allows any cutoff from 0; above 1 everything is discarded either way.
            AlphaMode::Mask { cutoff: material.alpha_cutoff().unwrap_or(0.5).clamp(0.0, 1.0) }
        }
        ::gltf::material::AlphaMode::Blend => AlphaMode::Blend,
    }
}

/// The anime character material a glTF material describes.
fn generated_material(material: &Material<'_>) -> Result<AnimeCharacterMaterial, GltfImportError> {
    let mut generated = AnimeCharacterMaterial {
        base_color: material.pbr_metallic_roughness().base_color_factor(),
        alpha: alpha_mode(material),
        double_sided: material.double_sided(),
        ..AnimeCharacterMaterial::default()
    };
    let Some(raw) = material.extras() else {
        return Ok(generated);
    };
    let invalid = |source| GltfImportError::MaterialExtras {
        material: describe(material.name(), material.index()),
        source,
    };
    // glTF recommends, but doesn't require, extras to be an object; anything else can't
    // carry material parameters.
    let value: serde_json::Value = serde_json::from_str(raw.get()).map_err(invalid)?;
    if !value.is_object() {
        return Ok(generated);
    }
    let extras = MaterialExtras::deserialize(value).map_err(invalid)?;
    let rgb = |[r, g, b, _]: [f32; 4]| [r, g, b];
    let region = &mut generated.regions[0];
    if let Some(shade_color) = extras.shade_color {
        region.shade_color = rgb(shade_color);
    }
    if let Some(outline_color) = extras.outline_color {
        region.outline_color = rgb(outline_color);
    }
    if let Some([threshold, softness, rim, outline_width]) = extras.character_shading {
        generated.light_split.threshold = threshold;
        generated.light_split.softness = softness;
        generated.rim.strength = rim;
        generated.outline.width = outline_width;
    }
    let RimExtras { width, strength, threshold, fresnel, light_gate, back_light } = extras.rim;
    let rim = &mut generated.rim;
    for (field, value) in [
        (&mut rim.width, width),
        (&mut rim.strength, strength),
        (&mut rim.threshold, threshold),
        (&mut rim.fresnel, fresnel),
        (&mut rim.light_gate, light_gate),
        (&mut rim.back_light, back_light),
    ] {
        if let Some(value) = value {
            *field = value;
        }
    }
    if !extras.rim_regions.is_empty() {
        let first = generated.regions[0];
        generated.regions = extras
            .rim_regions
            .iter()
            .map(|&[rim_strength, rim_width, rim_overlap]| CharacterRegion {
                rim_strength,
                rim_width,
                rim_overlap,
                ..first
            })
            .collect();
    }
    Ok(generated)
}

fn describe(name: Option<&str>, index: Option<usize>) -> String {
    match (name, index) {
        (Some(name), _) => format!("{name:?}"),
        (None, Some(index)) => format!("#{index}"),
        (None, None) => "(default)".to_owned(),
    }
}

mod accessors;
#[cfg(test)]
mod malformed_tests;
#[cfg(test)]
mod tests;
