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
//! - `baseColorFactor` becomes [`ToonMaterialParams::base_color`]. Material `extras` may
//!   set `shade_color`, `outline_color` (linear RGB or RGBA) and `toon_params`
//!   (`[shadow threshold, softness, rim strength, outline width]`); everything else keeps
//!   [`ToonMaterialParams::default`]. In Blender these are custom properties on the
//!   material, exported with "Include → Custom Properties".
//! - `COLOR_0` RGB becomes the sRGB vertex tint and its alpha the outline-width scale.
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
//! is the node's `weights`, else the mesh's, else 0.
//! Position deltas take the static node's linear transform; normal deltas become the
//! difference of the transformed, normalized normals, exact at full weight. Vertices a target
//! moves by less than [`MORPH_POSITION_EPSILON`] and turns by less than
//! [`MORPH_NORMAL_EPSILON`] are left out. Tangent deltas are ignored until meshes carry
//! tangents; outline normals are not morphed.
//!
//! Textures and animation are ignored until their packages land. External buffers are part
//! of the source: [`external_files`] lists them for the content cache key. Images join that
//! list when textures are imported (M1 A2).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ::gltf::accessor::{DataType, Dimensions};
use ::gltf::buffer::Source;
use ::gltf::mesh::{Mode, Semantic};
use ::gltf::{Accessor, Document, Gltf, Material, Mesh, Node, Primitive, Scene};
use glam::{Mat3, Mat4, Vec3};
use sapphire_assets::mesh::{Joint, JointTransform, Lod, SkinBind, Submesh, ToonMaterialParams, Vertex};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use crate::cache::Stage;
use crate::color::{linear_to_srgb, unorm8};
use crate::mesh::{ImportedDelta, ImportedMesh, ImportedMorphTarget, ImportedSkin, SkinWeights};

/// This importer, as cache keys know it. Bump the version whenever import output changes
/// for the same file.
pub const IMPORTER: Stage = Stage { name: "gltf", version: 2 };

/// Morph deltas whose position moves less than this (meters, on every axis) and whose normal
/// changes less than [`MORPH_NORMAL_EPSILON`] are dropped as noise: a micrometer is far below
/// the position quantization of any mesh.
pub const MORPH_POSITION_EPSILON: f32 = 1e-6;
/// See [`MORPH_POSITION_EPSILON`]: 1e-4 is under a hundredth of a degree.
pub const MORPH_NORMAL_EPSILON: f32 = 1e-4;

/// Joints and palette entries are addressed by u16.
const MAX_JOINTS: usize = 1 << 16;

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
    #[error("node {node} is its own ancestor; glTF node hierarchies must be trees")]
    NodeCycle { node: usize },
    #[error("{at}: primitive mode {mode:?} is not supported; export triangles")]
    UnsupportedMode { at: PrimitiveRef, mode: Mode },
    #[error("{at}: has no POSITION attribute")]
    MissingPositions { at: PrimitiveRef },
    #[error("{at}: the {semantic} accessor has no readable data")]
    UnreadableAccessor { at: PrimitiveRef, semantic: String },
    #[error("{at}: {semantic} has {found} elements but POSITION has {expected}")]
    AttributeCount { at: PrimitiveRef, semantic: String, found: usize, expected: usize },
    #[error("{at}: {semantic} as {dimensions:?} of {data_type:?} is not supported")]
    UnsupportedAccessor { at: PrimitiveRef, semantic: String, data_type: DataType, dimensions: Dimensions },
    #[error("{at}: index count {count} is not a multiple of 3")]
    NotTriangles { at: PrimitiveRef, count: usize },
    #[error("{at}: index {index} is out of range for {vertex_count} vertices")]
    IndexOutOfRange { at: PrimitiveRef, index: u32, vertex_count: usize },
    #[error("material {material}: invalid toon extras")]
    MaterialExtras {
        material: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("skin {skin}: joint node {node} is not in the imported scene")]
    JointNotInScene { skin: usize, node: usize },
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
    let Gltf { document, blob } = Gltf::from_slice(bytes).map_err(GltfImportError::Parse)?;
    let buffers = load_buffers(&document, base_dir, blob)?;
    // Blender always writes `scene`; fall back to the first scene for files that don't.
    let scene =
        document.default_scene().or_else(|| document.scenes().next()).ok_or(GltfImportError::NoScene)?;
    // No axis conversion, deliberately: glTF 2.0 (§3.4) is +Y up, meters, with assets
    // facing +Z, and so is engine object space (as is VRM 1.0). Blender's exporter already
    // converts from its +Z-up space, so source coordinates pass through unchanged.
    let nodes = flatten(&scene)?;
    let skeleton = Skeleton::build(&document, &nodes, &buffers)?;
    let mut builder = Builder {
        buffers: &buffers,
        mesh: ImportedMesh { lods: vec![Lod::default()], ..ImportedMesh::default() },
        materials: HashMap::new(),
        skeleton,
        targets: HashMap::new(),
    };
    for (instance, scene_node) in nodes.iter().enumerate() {
        let node = document.nodes().nth(scene_node.node).expect("flattened from this document");
        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                builder.add_primitive(&nodes, instance, &node, &mesh, &primitive)?;
            }
        }
    }
    builder.finish()
}

/// One visit of a node in the scene's depth-first walk. A node reached through two parents
/// (not valid glTF, but tolerated) is visited, and its mesh instanced, twice.
struct SceneNode {
    node: usize,
    /// The visit of the parent node.
    parent: Option<usize>,
    world: Mat4,
}

/// The scene's nodes in depth-first order, children in glTF order, with world transforms.
fn flatten(scene: &Scene<'_>) -> Result<Vec<SceneNode>, GltfImportError> {
    fn visit(
        node: &Node<'_>,
        parent: Option<usize>,
        parent_world: Mat4,
        ancestors: &mut Vec<usize>,
        out: &mut Vec<SceneNode>,
    ) -> Result<(), GltfImportError> {
        if ancestors.contains(&node.index()) {
            return Err(GltfImportError::NodeCycle { node: node.index() });
        }
        let world = parent_world * Mat4::from_cols_array_2d(&node.transform().matrix());
        let visit_index = out.len();
        out.push(SceneNode { node: node.index(), parent, world });
        ancestors.push(node.index());
        for child in node.children() {
            visit(&child, Some(visit_index), world, ancestors, out)?;
        }
        ancestors.pop();
        Ok(())
    }

    let mut out = Vec::new();
    for node in scene.nodes() {
        visit(&node, None, Mat4::IDENTITY, &mut Vec::new(), &mut out)?;
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
    /// The skeleton of every skin a scene node uses, or `None` if no node is skinned.
    fn build(
        document: &Document,
        nodes: &[SceneNode],
        buffers: &[Vec<u8>],
    ) -> Result<Option<Self>, GltfImportError> {
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
        if skins.is_empty() {
            return Ok(None);
        }
        let mut first_visit: HashMap<usize, usize> = HashMap::new();
        for (visit, scene_node) in nodes.iter().enumerate() {
            first_visit.entry(scene_node.node).or_insert(visit);
        }
        // Every joint and its ancestors.
        let mut included = vec![false; nodes.len()];
        for skin in &skins {
            for joint in skin.joints() {
                let mut visit =
                    Some(*first_visit.get(&joint.index()).ok_or(GltfImportError::JointNotInScene {
                        skin: skin.index(),
                        node: joint.index(),
                    })?);
                while let Some(current) = visit {
                    included[current] = true;
                    visit = nodes[current].parent;
                }
            }
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
            if !included[visit] || first_visit[&scene_node.node] != visit {
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
        for skin in &skins {
            let reader = skin.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
            let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
            let matrices: Vec<[[f32; 4]; 4]> = match reader.read_inverse_bind_matrices() {
                Some(matrices) => matrices.collect(),
                None => vec![Mat4::IDENTITY.to_cols_array_2d(); joints.len()],
            };
            if matrices.len() != joints.len() {
                return Err(GltfImportError::InverseBindMatrices {
                    skin: skin.index(),
                    found: matrices.len(),
                    expected: joints.len(),
                });
            }
            let binds = joints
                .iter()
                .zip(matrices)
                .map(|(node, matrix)| skeleton.bind(skeleton.joint_of[node], matrix))
                .collect::<Result<_, _>>()?;
            skeleton.skin_binds.insert(skin.index(), binds);
        }
        Ok(Some(skeleton))
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

/// Loads every buffer of `document`: the GLB binary chunk, base64 `data:` URIs, or files
/// relative to `base_dir`. This is `gltf::import_buffers`, whose `import` feature would also
/// build an image decoder (`image`) the pipeline doesn't use until textures arrive in M1.
fn load_buffers(
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
    let Gltf { document, .. } = Gltf::from_slice(bytes).map_err(GltfImportError::Parse)?;
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

/// Toon parameters authored as glTF material extras. Unknown keys are ignored, because
/// Blender exports every custom property on the material.
#[derive(Debug, Default, Deserialize)]
struct ToonExtras {
    #[serde(default, deserialize_with = "linear_rgba")]
    shade_color: Option<[f32; 4]>,
    #[serde(default, deserialize_with = "linear_rgba")]
    outline_color: Option<[f32; 4]>,
    toon_params: Option<[f32; 4]>,
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

struct Builder<'a> {
    buffers: &'a [Vec<u8>],
    mesh: ImportedMesh,
    /// glTF material index (`None` = the default material) → cooked material slot.
    materials: HashMap<Option<usize>, u32>,
    skeleton: Option<Skeleton>,
    /// Morph target name → index into `mesh.morph_targets`.
    targets: HashMap<String, usize>,
}

/// A primitive's attributes, read and transformed into the mesh's space.
struct PrimitiveData {
    positions: Vec<Vec3>,
    /// Transformed and normalized.
    normals: Option<Vec<Vec3>>,
    /// As the file stores them, for morph targets' normal deltas.
    source_normals: Option<Vec<Vec3>>,
    uvs: Option<Vec<[f32; 2]>>,
    colors: Option<Vec<[u8; 4]>>,
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
        let declared = |semantic: Semantic| primitive.get(&semantic).is_some();
        let skin = node.skin().filter(|_| self.skeleton.is_some());
        // Skinned vertices stay in bind space (glTF ignores the skinned node's transform).
        let transform = if skin.is_some() { Mat4::IDENTITY } else { nodes[visit].world };
        let normal_matrix = Mat3::from_mat4(transform).inverse().transpose();

        let positions: Vec<Vec3> = match reader.read_positions() {
            Some(iter) => iter.map(|p| transform.transform_point3(Vec3::from(p))).collect(),
            None if declared(Semantic::Positions) => {
                return Err(GltfImportError::UnreadableAccessor { at, semantic: "POSITION".into() });
            }
            None => return Err(GltfImportError::MissingPositions { at }),
        };
        let vertex_count = positions.len();
        let transform_normal = |n: Vec3| (normal_matrix * n).normalize_or(Vec3::Y);
        let source_normals =
            read_attribute(&at, "NORMAL", declared(Semantic::Normals), vertex_count, || {
                reader.read_normals().map(|iter| iter.map(Vec3::from).collect::<Vec<_>>())
            })?;
        let data = PrimitiveData {
            normals: source_normals
                .as_ref()
                .map(|normals| normals.iter().copied().map(transform_normal).collect()),
            source_normals,
            uvs: read_attribute(&at, "TEXCOORD_0", declared(Semantic::TexCoords(0)), vertex_count, || {
                reader.read_tex_coords(0).map(|iter| iter.into_f32().collect())
            })?,
            colors: read_attribute(&at, "COLOR_0", declared(Semantic::Colors(0)), vertex_count, || {
                reader.read_colors(0).map(|iter| iter.into_rgba_f32().map(vertex_color).collect())
            })?,
            outline_normals: read_outline_normals(&at, primitive, &get_buffer, vertex_count)?
                .map(|normals| normals.into_iter().map(transform_normal).collect()),
            positions,
        };

        let indices: Vec<u32> = match (primitive.indices(), reader.read_indices()) {
            (_, Some(iter)) => iter.into_u32().collect(),
            (Some(_), None) => {
                return Err(GltfImportError::UnreadableAccessor { at, semantic: "indices".into() });
            }
            (None, None) => {
                (0..u32::try_from(vertex_count).map_err(|_| GltfImportError::TooLarge)?).collect()
            }
        };
        if !indices.len().is_multiple_of(3) {
            return Err(GltfImportError::NotTriangles { at, count: indices.len() });
        }
        if let Some(&index) = indices.iter().find(|&&i| i as usize >= vertex_count) {
            return Err(GltfImportError::IndexOutOfRange { at, index, vertex_count });
        }
        if indices.is_empty() {
            return Ok(());
        }

        // A negative determinant mirrors the geometry, which turns counter-clockwise front
        // faces clockwise; swap two corners to restore the winding (glTF 2.0 §3.7.4).
        let mirrored = transform.determinant() < 0.0;
        let triangles: Vec<[u32; 3]> = indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[a, b, c]| if mirrored { [a, c, b] } else { [a, b, c] })
            .collect();

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
                color: data.colors.as_ref().map_or([u8::MAX; 4], |colors| colors[s]),
            });
        }
        let authored = data.outline_normals.is_some();
        self.mesh.authored_outline.extend(std::iter::repeat_n(authored, sources.len()));
        self.mesh.indices.extend(local_indices.iter().map(|i| base + i));
        u32::try_from(self.mesh.indices.len()).map_err(|_| GltfImportError::TooLarge)?;

        if let Some(skeleton) = self.skeleton.as_mut() {
            let weights = match &skin {
                Some(skin) => {
                    read_weights(&at, primitive, &reader, &skeleton.skin_binds[&skin.index()], vertex_count)?
                }
                None => vec![SkinWeights::rigid(skeleton.rigid_bind(nodes, visit)?); vertex_count],
            };
            skeleton.weights.extend(sources.iter().map(|&s| weights[s as usize]));
        }
        self.add_morph_targets(node, mesh, &reader, &data, transform, &sources, base, vertex_count, &at)?;

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
        reader: &::gltf::mesh::Reader<'a, 's, F>,
        data: &PrimitiveData,
        transform: Mat4,
        sources: &[u32],
        base: u32,
        vertex_count: usize,
        at: &PrimitiveRef,
    ) -> Result<(), GltfImportError>
    where
        F: Clone + Fn(::gltf::Buffer<'a>) -> Option<&'s [u8]>,
    {
        let targets: Vec<_> = reader.read_morph_targets().collect();
        if targets.is_empty() {
            return Ok(());
        }
        let names = mesh
            .extras()
            .as_ref()
            .and_then(|raw| serde_json::from_str::<MeshExtras>(raw.get()).ok())
            .unwrap_or_default()
            .target_names;
        let defaults = node.weights().or(mesh.weights()).unwrap_or_default();
        let linear = Mat3::from_mat4(transform);
        let normal_matrix = linear.inverse().transpose();
        // The merged targets this primitive has written to.
        let mut used = Vec::with_capacity(targets.len());
        for (k, (positions, normals, _tangents)) in targets.into_iter().enumerate() {
            let semantic = |what: &str| format!("morph target {k} {what}");
            let positions: Option<Vec<[f32; 3]>> = positions.map(Iterator::collect);
            let normal_deltas: Option<Vec<[f32; 3]>> = normals.map(Iterator::collect);
            for (what, values) in [("POSITION", &positions), ("NORMAL", &normal_deltas)] {
                if let Some(values) = values
                    && values.len() != vertex_count
                {
                    let (found, expected) = (values.len(), vertex_count);
                    return Err(GltfImportError::AttributeCount {
                        at: at.clone(),
                        semantic: semantic(what),
                        found,
                        expected,
                    });
                }
            }
            let name = names.get(k).cloned().unwrap_or_else(|| {
                format!("{}/{k}", mesh.name().map_or_else(|| format!("#{}", mesh.index()), str::to_owned))
            });
            let target = *self.targets.entry(name.clone()).or_insert_with(|| {
                self.mesh.morph_targets.push(ImportedMorphTarget {
                    name: name.clone(),
                    default_weight: defaults.get(k).copied().unwrap_or(0.0),
                    deltas: Vec::new(),
                });
                self.mesh.morph_targets.len() - 1
            });
            // Two of one primitive's targets under one name would move the same vertices twice.
            if used.contains(&target) {
                return Err(GltfImportError::DuplicateMorphTarget { at: at.clone(), name });
            }
            used.push(target);
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
                self.mesh.morph_targets[target].deltas.push(ImportedDelta {
                    vertex: base + offset as u32,
                    position: position.to_array(),
                    normal: normal.to_array(),
                });
            }
        }
        Ok(())
    }

    fn material_slot(&mut self, material: &Material<'_>) -> Result<u32, GltfImportError> {
        if let Some(&slot) = self.materials.get(&material.index()) {
            return Ok(slot);
        }
        let slot = u32::try_from(self.mesh.materials.len()).map_err(|_| GltfImportError::TooLarge)?;
        self.mesh.materials.push(toon_material(material)?);
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

/// Reads an optional per-vertex attribute, distinguishing "absent" from "declared but
/// unreadable" and checking its length against `POSITION`.
fn read_attribute<T>(
    at: &PrimitiveRef,
    semantic: &str,
    declared: bool,
    expected: usize,
    read: impl FnOnce() -> Option<Vec<T>>,
) -> Result<Option<Vec<T>>, GltfImportError> {
    match read() {
        Some(values) if values.len() != expected => Err(GltfImportError::AttributeCount {
            at: at.clone(),
            semantic: semantic.to_owned(),
            found: values.len(),
            expected,
        }),
        Some(values) => Ok(Some(values)),
        None if declared => {
            Err(GltfImportError::UnreadableAccessor { at: at.clone(), semantic: semantic.to_owned() })
        }
        None => Ok(None),
    }
}

/// Fails unless `accessor` is one of the `allowed` (component type, normalized) pairs with
/// `dimensions`. The `gltf` readers panic on component types the specification rules out, so
/// every accessor they read with a choice of types is checked first.
fn check_accessor(
    at: &PrimitiveRef,
    semantic: &str,
    accessor: &Accessor<'_>,
    dimensions: Dimensions,
    allowed: &[(DataType, bool)],
) -> Result<(), GltfImportError> {
    let kind = (accessor.data_type(), accessor.normalized());
    if accessor.dimensions() == dimensions && allowed.contains(&kind) {
        return Ok(());
    }
    Err(GltfImportError::UnsupportedAccessor {
        at: at.clone(),
        semantic: semantic.to_owned(),
        data_type: accessor.data_type(),
        dimensions: accessor.dimensions(),
    })
}

/// The primitive's `_OUTLINE_NORMAL` attribute, untransformed, if it has one.
fn read_outline_normals<'a, 's, F>(
    at: &PrimitiveRef,
    primitive: &Primitive<'a>,
    get_buffer: &F,
    vertex_count: usize,
) -> Result<Option<Vec<Vec3>>, GltfImportError>
where
    F: Clone + Fn(::gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    let Some(accessor) = primitive.get(&Semantic::Extras(OUTLINE_NORMAL.to_owned())) else {
        return Ok(None);
    };
    let semantic = "_OUTLINE_NORMAL";
    let allowed = [(DataType::F32, false), (DataType::I16, true), (DataType::I8, true)];
    check_accessor(at, semantic, &accessor, Dimensions::Vec3, &allowed)?;
    // Normalized signed integers decode as max(q / max, -1) (glTF 2.0 §3.11).
    fn snorm<T: Into<f32>>(values: [T; 3], max: f32) -> Vec3 {
        Vec3::from(values.map(|c| (c.into() / max).max(-1.0)))
    }
    let normals: Option<Vec<Vec3>> = match accessor.data_type() {
        DataType::I16 => ::gltf::accessor::Iter::<[i16; 3]>::new(accessor, get_buffer.clone())
            .map(|iter| iter.map(|v| snorm(v, 32767.0)).collect()),
        DataType::I8 => ::gltf::accessor::Iter::<[i8; 3]>::new(accessor, get_buffer.clone())
            .map(|iter| iter.map(|v| snorm(v, 127.0)).collect()),
        _ => ::gltf::accessor::Iter::<[f32; 3]>::new(accessor, get_buffer.clone())
            .map(|iter| iter.map(Vec3::from).collect()),
    };
    read_attribute(at, semantic, true, vertex_count, || normals)
}

/// Every vertex's influences from all `JOINTS_n`/`WEIGHTS_n` sets, as palette entries through
/// `binds` (the skin's joints' entries), reduced to the four heaviest.
fn read_weights<'a, 's, F>(
    at: &PrimitiveRef,
    primitive: &Primitive<'a>,
    reader: &::gltf::mesh::Reader<'a, 's, F>,
    binds: &[u16],
    vertex_count: usize,
) -> Result<Vec<SkinWeights>, GltfImportError>
where
    F: Clone + Fn(::gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    let mut influences: Vec<Vec<(u16, f32)>> = vec![Vec::new(); vertex_count];
    for set in 0.. {
        let Some(joints) = primitive.get(&Semantic::Joints(set)) else { break };
        let (joints_name, weights_name) = (format!("JOINTS_{set}"), format!("WEIGHTS_{set}"));
        check_accessor(
            at,
            &joints_name,
            &joints,
            Dimensions::Vec4,
            &[(DataType::U8, false), (DataType::U16, false)],
        )?;
        let weights = primitive.get(&Semantic::Weights(set)).ok_or_else(|| {
            GltfImportError::UnreadableAccessor { at: at.clone(), semantic: weights_name.clone() }
        })?;
        let allowed = [(DataType::F32, false), (DataType::U8, true), (DataType::U16, true)];
        check_accessor(at, &weights_name, &weights, Dimensions::Vec4, &allowed)?;
        let joints = read_attribute(at, &joints_name, true, vertex_count, || {
            reader.read_joints(set).map(|iter| iter.into_u16().collect::<Vec<_>>())
        })?
        .expect("a declared attribute reads or fails");
        let weights = read_attribute(at, &weights_name, true, vertex_count, || {
            reader.read_weights(set).map(|iter| iter.into_f32().collect::<Vec<_>>())
        })?
        .expect("a declared attribute reads or fails");
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
    // A skin has at least one joint (glTF 2.0 §5.27), whose entry takes unweighted vertices.
    let fallback = SkinWeights::rigid(binds.first().copied().unwrap_or(0));
    Ok(influences.into_iter().map(|list| SkinWeights::strongest(list).unwrap_or(fallback)).collect())
}

/// glTF `COLOR_0` is linear; the cooked tint is sRGB. Alpha is the outline-width scale,
/// which is a plain linear factor.
fn vertex_color([r, g, b, a]: [f32; 4]) -> [u8; 4] {
    [unorm8(linear_to_srgb(r)), unorm8(linear_to_srgb(g)), unorm8(linear_to_srgb(b)), unorm8(a)]
}

fn toon_material(material: &Material<'_>) -> Result<ToonMaterialParams, GltfImportError> {
    let mut params = ToonMaterialParams {
        base_color: material.pbr_metallic_roughness().base_color_factor(),
        ..ToonMaterialParams::default()
    };
    let Some(raw) = material.extras() else {
        return Ok(params);
    };
    let invalid = |source| GltfImportError::MaterialExtras {
        material: describe(material.name(), material.index()),
        source,
    };
    // glTF recommends, but doesn't require, extras to be an object; anything else can't
    // carry toon parameters.
    let value: serde_json::Value = serde_json::from_str(raw.get()).map_err(invalid)?;
    if !value.is_object() {
        return Ok(params);
    }
    let extras = ToonExtras::deserialize(value).map_err(invalid)?;
    if let Some(shade_color) = extras.shade_color {
        params.shade_color = shade_color;
    }
    if let Some(outline_color) = extras.outline_color {
        params.outline_color = outline_color;
    }
    if let Some(toon_params) = extras.toon_params {
        params.params = toon_params;
    }
    Ok(params)
}

fn describe(name: Option<&str>, index: Option<usize>) -> String {
    match (name, index) {
        (Some(name), _) => format!("{name:?}"),
        (None, Some(index)) => format!("#{index}"),
        (None, None) => "(default)".to_owned(),
    }
}

#[cfg(test)]
mod tests;
