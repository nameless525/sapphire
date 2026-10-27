//! FBX importer for `.fbx` files (M1 A10), through ufbx (`ufbx` 0.11, the C library 0.23).
//!
//! Imports to the same [`ImportedMesh`] as the [glTF importer](crate::import::gltf) does: a scene
//! exported from Blender as both FBX and glTF imports to the same mesh, which the tests check on
//! fixtures exported both ways (`tools/blender/fbx_fixtures.py`). Everything downstream (the
//! outline and foliage bakes, LODs, packing) is shared.
//!
//! - **Space.** ufbx converts the scene on load to engine object space, glTF's: +Y up, meters,
//!   right-handed, assets facing +Z. It folds the axis and unit conversion into the top-level
//!   nodes' transforms (`AdjustTransforms`), so geometry stays as authored, and the ×100 scale
//!   Blender puts on every top-level object to make up for its centimeter unit cancels out: a
//!   Blender export's skeleton comes out in meters with unit scales. A file authored in
//!   centimeters keeps its numbers under a 0.01 scale on its top-level nodes. A left-handed file
//!   is mirrored in Z, with its faces' winding turned so they stay front-facing. Geometric
//!   transforms (3ds Max's object offsets) are part of each mesh's geometry-to-world matrix, and
//!   Maya's segment scale compensation is turned into plain inheritance.
//! - **Flattening.** Every node with a mesh, in depth-first order of the scene, children in file
//!   order, adds its mesh in the node's geometry-to-world space: positions by the matrix, normals
//!   by its inverse transpose. Mirrored transforms flip the triangle winding. A mesh two nodes
//!   share is added twice. Polygons are triangulated by ufbx (ear clipping, so concave polygons
//!   come out right); faces of fewer than three corners (points, lines) are skipped.
//! - **Materials.** FBX gives each node instance its own materials, which faces index. Each
//!   material a node instance uses is one submesh of it, named after the material the way glTF's
//!   slots are (`import::slot_names`: its name, `<name>#<index>` when two share one, `#default` for
//!   faces without). Each slot holds a generated [`AnimeCharacterMaterial`], as glTF's do: the
//!   material's base (diffuse) color times its factor is its base color and the material's opacity
//!   its alpha, blended below 1; a material that says it is double-sided (ufbx's feature, from
//!   3ds Max's glTF material) is drawn on both sides (M1 C8), but Blender's FBX says nothing of
//!   it, though its glTF does; everything else keeps its default. Textures are left out of
//!   generated materials, as glTF's are: they are separate sources, which `extract-materials` binds
//!   in documents ([`crate::extract`]), from files the FBX names relative to itself or images it
//!   embeds. Blender writes its Principled BSDF as FBX Phong in a fixed way, which is read back as
//!   such. FBX has no counterpart to glTF's material extras: a sidecar binds documents instead.
//! - **Vertices.** FBX stores attributes per face corner; corners of one vertex whose normal,
//!   texture coordinates and color all agree become one vertex. Missing normals are replaced by
//!   flat face normals, as glTF's are. The first UV set gives the texture coordinates, with V
//!   flipped (FBX's V runs up the image, glTF's and the engine's down). The first color set is
//!   the anime character vertex color (R ambient occlusion, G outline-width scale, B outline
//!   Z-offset scale; alpha ignored), taken as stored: data, with no transfer function, as glTF's
//!   linear `COLOR_0` is. Blender's FBX exporter converts colors to sRGB unless told
//!   otherwise (Vertex Colors: Linear), so data painted there belongs in a float color attribute
//!   exported linear. FBX carries no material IDs or authored outline normals (glTF's
//!   `_MATERIAL_ID` and `_OUTLINE_NORMAL`): every vertex has ID 0, and the outline normals are
//!   baked. Tangents and further UV and color sets are ignored: cooked meshes carry none.
//!
//! # Skins
//!
//! When any mesh in the scene has a skin deformer, the mesh gets one [`ImportedSkin`], as glTF's
//! does:
//!
//! - **Skeleton:** the bone of every skin cluster used, plus their ancestors (the scene's root
//!   node aside, which the conversion leaves at the identity), in depth-first order of the scene,
//!   so parents come first. Names are node names (`#<id>` for unnamed ones); the rest pose is each
//!   node's local transform, which ufbx resolves from FBX's pre- and post-rotations and pivots.
//! - **Palette:** a skinned mesh's vertices are placed in model space by its node's
//!   geometry-to-world matrix `G`, like a static mesh's, and each cluster becomes the entry
//!   (bone, `geometry_to_bone · G⁻¹`): the bone's world transform times that entry is FBX's own
//!   skinning matrix, so the mesh deforms exactly as the file says whatever pose it was bound in,
//!   and stands where the file shows it. Entries repeat once per (joint, matrix).
//! - **Weights:** one skin deformer per mesh. The four heaviest influences are kept and
//!   renormalized, the rest dropped, and a vertex with no positive weight is bound wholly to the
//!   first cluster's entry. Every skin blends linearly: dual-quaternion weights are dropped.
//! - **Static parts of a skinned mesh** (meshes parented to a bone, props beside the character)
//!   are bound rigidly to their nearest ancestor joint, or to joint 0, as glTF's are.
//!
//! # Morph targets
//!
//! Each blend shape channel is a morph target named after the channel (Blender writes its shape
//! keys' names), merged with same-named targets of other meshes, as glTF's are; two channels of
//! one mesh with one name are an error. A target's default weight is its channel's. Each channel
//! keeps its full-weight shape: in-between shapes (Maya's) have no linear counterpart, and are
//! dropped. Blender's per-vertex shape weights (a shape key's vertex group) scale the offsets.
//! Position offsets take the node's linear transform. Normal offsets, where the file has them,
//! are added to each corner's normal, and the delta is the difference of the transformed,
//! normalized normals, as glTF's is. FBX offsets a vertex's normal, not a corner's: Blender
//! writes the change of each vertex's smooth normal, so at hard edges a corner turns as its
//! vertex does, which glTF's per-corner deltas don't. Vertices a target moves by less than
//! [`MORPH_POSITION_EPSILON`] and turns by less than [`MORPH_NORMAL_EPSILON`] are left out.
//!
//! Not imported: animation (clips are M2's, cooked to ozz from FBX through ufbx too), cameras,
//! lights, NURBS, LOD groups, constraints and visibility. The importer reads the file alone,
//! never the files it names, so its bytes are its whole cache key.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use glam::{DMat3, DMat4, DQuat, DVec3, DVec4};
use sapphire_assets::material::{AlphaMode, AnimeCharacterMaterial};
use sapphire_assets::mesh::{
    Joint, JointTransform, Lod, SkinBind, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};

use crate::cache::Stage;
use crate::color::unorm8;
use crate::import::gltf::{MORPH_NORMAL_EPSILON, MORPH_POSITION_EPSILON};
use crate::import::{DEFAULT_SLOT, slot_names};
use crate::mesh::{
    ImportedDelta, ImportedMaterial, ImportedMesh, ImportedMorphTarget, ImportedSkin, SkinWeights,
};

#[cfg(test)]
mod tests;

/// This importer, as cache keys know it. The name carries ufbx's version, which a test keeps in
/// step with `Cargo.lock`: a ufbx upgrade can change what a file imports to. Bump the version
/// whenever import output changes for the same file.
///
/// 2: generated materials take a material's double-sided feature (M1 C8).
pub const IMPORTER: Stage = Stage { name: "fbx (ufbx 0.11.5)", version: 2 };

/// Joints and palette entries are addressed by u16.
const MAX_JOINTS: usize = 1 << 16;

/// Errors from importing an FBX file.
#[derive(Debug, thiserror::Error)]
pub enum FbxImportError {
    #[error("failed to read {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// ufbx's message, which names what it couldn't read.
    #[error("not a readable FBX file: {0}")]
    Parse(String),
    #[error("mesh {mesh} has {count} skin deformers; a mesh may have one")]
    MultipleSkins { mesh: String, count: usize },
    #[error("mesh {mesh} has two blend shape channels named {name:?}")]
    DuplicateMorphTarget { mesh: String, name: String },
    #[error("node {node}'s transform has no inverse")]
    DegenerateTransform { node: String },
    #[error("the skeleton has more than {MAX_JOINTS} joints or palette entries")]
    TooManyJoints,
    #[error("the scene contains no triangles")]
    NoGeometry,
    #[error("the scene has more than u32::MAX vertices or indices")]
    TooLarge,
}

/// Imports a `.fbx` file.
pub fn import(path: &Path) -> Result<ImportedMesh, FbxImportError> {
    let bytes = std::fs::read(path).map_err(|source| FbxImportError::Io { path: path.to_owned(), source })?;
    import_slice(&bytes)
}

/// Imports FBX bytes, binary or ASCII.
pub fn import_slice(bytes: &[u8]) -> Result<ImportedMesh, FbxImportError> {
    let scene = load(bytes, false)?;
    import_scene(&scene).map(|import| import.mesh)
}

/// An FBX file as imported, with the textures each material slot's source material references:
/// what `extract-materials` binds.
#[derive(Clone, Debug)]
pub(crate) struct FbxImport {
    pub mesh: ImportedMesh,
    /// Per material slot of `mesh`.
    pub slots: Vec<SlotTextures>,
}

/// The textures a material slot's FBX material references, by the use the anime character family
/// has for them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SlotTextures {
    pub base_color: Option<FbxTexture>,
    pub normal: Option<FbxTexture>,
}

/// A texture an FBX material references.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FbxTexture {
    /// The image file's stem, or the texture's name when the file records no file name.
    pub name: String,
    /// The image file, relative to the FBX, `/`-separated, as the file records it; empty when
    /// it records none.
    pub file: String,
    /// The image's bytes when the FBX embeds them.
    pub embedded: Option<Vec<u8>>,
}

/// Imports FBX bytes with the textures their materials reference, embedded images included.
pub(crate) fn import_with_textures(bytes: &[u8]) -> Result<FbxImport, FbxImportError> {
    let scene = load(bytes, true)?;
    import_scene(&scene)
}

/// Loads and converts a scene (see the [module docs](self)); embedded images only when asked.
fn load(bytes: &[u8], embedded: bool) -> Result<ufbx::SceneRoot, FbxImportError> {
    let options = ufbx::LoadOpts {
        // Animation is M2's; external files (textures, geometry caches) aren't part of the mesh.
        ignore_animation: true,
        ignore_embedded: !embedded,
        load_external_files: false,
        target_axes: ufbx::CoordinateAxes {
            right: ufbx::CoordinateAxis::PositiveX,
            up: ufbx::CoordinateAxis::PositiveY,
            front: ufbx::CoordinateAxis::PositiveZ,
        },
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::AdjustTransforms,
        handedness_conversion_axis: ufbx::MirrorAxis::Z,
        inherit_mode_handling: ufbx::InheritModeHandling::Compensate,
        use_blender_pbr_material: true,
        ..ufbx::LoadOpts::default()
    };
    ufbx::load_memory(bytes, options).map_err(|error| FbxImportError::Parse(format!("{error:?}")))
}

/// Imports a loaded scene.
fn import_scene(scene: &ufbx::Scene) -> Result<FbxImport, FbxImportError> {
    let nodes = flatten(scene);
    let skeleton = Skeleton::build(&nodes)?;
    let names: Vec<Option<&str>> = scene
        .materials
        .iter()
        .map(|material| Some(&*material.element.name).filter(|n| !n.is_empty()))
        .collect();
    let mut builder = Builder {
        mesh: ImportedMesh { lods: vec![Lod::default()], ..ImportedMesh::default() },
        materials: HashMap::new(),
        slot_names: slot_names(&names),
        slots: Vec::new(),
        skeleton,
        targets: HashMap::new(),
    };
    for visit in 0..nodes.len() {
        builder.add_instance(&nodes, visit)?;
    }
    builder.finish()
}

/// One node of the scene's depth-first walk.
struct SceneNode<'s> {
    node: &'s ufbx::Node,
    /// The visit of the parent node; `None` under the root.
    parent: Option<usize>,
}

/// The scene's nodes below the root, in depth-first order, children in file order.
fn flatten(scene: &ufbx::Scene) -> Vec<SceneNode<'_>> {
    let mut out = Vec::with_capacity(scene.nodes.len());
    let mut stack: Vec<(&ufbx::Node, Option<usize>)> =
        scene.root_node.children.iter().rev().map(|child| (&**child, None)).collect();
    while let Some((node, parent)) = stack.pop() {
        let visit = out.len();
        out.push(SceneNode { node, parent });
        stack.extend(node.children.iter().rev().map(|child| (&**child, Some(visit))));
    }
    out
}

/// The mesh's skeleton and palette while instances are added.
struct Skeleton {
    joints: Vec<Joint>,
    /// Model-space rest transform per joint.
    rest_world: Vec<DMat4>,
    /// Skeleton joint of each node (by ufbx's node ID) that is one.
    joint_of: HashMap<u32, u16>,
    binds: Vec<SkinBind>,
    /// Palette entry of each (joint, inverse bind matrix bits).
    bind_of: HashMap<(u16, [u32; 16]), u16>,
    /// Per vertex added so far.
    weights: Vec<SkinWeights>,
}

impl Skeleton {
    /// The skeleton of every skin a mesh instance uses, or `None` if no mesh is skinned.
    fn build(nodes: &[SceneNode<'_>]) -> Result<Option<Self>, FbxImportError> {
        let visit_of: HashMap<u32, usize> = nodes
            .iter()
            .enumerate()
            .map(|(visit, scene_node)| (scene_node.node.element.typed_id, visit))
            .collect();
        let mut included = vec![false; nodes.len()];
        let mut skinned = false;
        for scene_node in nodes {
            let Some(skin) = scene_node.node.mesh.as_deref().and_then(skin_of) else { continue };
            for cluster in &skin.clusters {
                skinned = true;
                let bone = cluster.bone_node.as_deref().expect("ufbx lists only clusters with a bone");
                // A bone outside the root's tree (not possible without `allow_nodes_out_of_root`)
                // would have no visit.
                let mut visit = visit_of.get(&bone.element.typed_id).copied();
                while let Some(current) = visit {
                    if std::mem::replace(&mut included[current], true) {
                        break;
                    }
                    visit = nodes[current].parent;
                }
            }
        }
        if !skinned {
            return Ok(None);
        }
        let mut skeleton = Self {
            joints: Vec::new(),
            rest_world: Vec::new(),
            joint_of: HashMap::new(),
            binds: Vec::new(),
            bind_of: HashMap::new(),
            weights: Vec::new(),
        };
        for (visit, scene_node) in nodes.iter().enumerate() {
            if !included[visit] {
                continue;
            }
            let node = scene_node.node;
            let index = u16::try_from(skeleton.joints.len()).map_err(|_| FbxImportError::TooManyJoints)?;
            let parent =
                scene_node.parent.map(|parent| skeleton.joint_of[&nodes[parent].node.element.typed_id]);
            // Under the root, the node's world transform is its local one (the root is the
            // identity after conversion); taking the world one keeps that exact regardless.
            let rest = match parent {
                Some(_) => node.local_transform,
                None => ufbx::matrix_to_transform(&node.node_to_world),
            };
            skeleton.joints.push(Joint {
                name: name_of(&node.element),
                parent,
                rest: joint_transform(&rest),
            });
            skeleton.rest_world.push(dmat4(&node.node_to_world));
            skeleton.joint_of.insert(node.element.typed_id, index);
        }
        Ok(Some(skeleton))
    }

    /// The palette entry for `joint` with `inverse_bind_matrix`, added if new.
    fn bind(&mut self, joint: u16, inverse_bind_matrix: DMat4) -> Result<u16, FbxImportError> {
        let matrix = inverse_bind_matrix.as_mat4().to_cols_array_2d();
        let bits: [u32; 16] = bytemuck::cast(matrix.map(|column| column.map(f32::to_bits)));
        if let Some(&bind) = self.bind_of.get(&(joint, bits)) {
            return Ok(bind);
        }
        let bind = u16::try_from(self.binds.len()).map_err(|_| FbxImportError::TooManyJoints)?;
        self.binds.push(SkinBind { joint, inverse_bind_matrix: matrix });
        self.bind_of.insert((joint, bits), bind);
        Ok(bind)
    }

    /// The palette entry that binds a static part at scene visit `visit` rigidly: to its
    /// nearest ancestor joint (or itself), else joint 0.
    fn rigid_bind(&mut self, nodes: &[SceneNode<'_>], visit: usize) -> Result<u16, FbxImportError> {
        let mut current = Some(visit);
        let joint = loop {
            match current {
                Some(at) => match self.joint_of.get(&nodes[at].node.element.typed_id) {
                    Some(&joint) => break joint,
                    None => current = nodes[at].parent,
                },
                None => break 0,
            }
        };
        let inverse = self.rest_world[usize::from(joint)].inverse();
        if !inverse.is_finite() {
            let node = self.joints[usize::from(joint)].name.clone();
            return Err(FbxImportError::DegenerateTransform { node });
        }
        self.bind(joint, inverse)
    }
}

/// The skin deformer of a mesh that is skinned (one with clusters).
fn skin_of(mesh: &ufbx::Mesh) -> Option<&ufbx::SkinDeformer> {
    mesh.skin_deformers.first().map(|skin| &**skin).filter(|skin| !skin.clusters.is_empty())
}

struct Builder {
    mesh: ImportedMesh,
    /// FBX material (by ufbx's material ID; `None` for faces without one) → cooked material slot.
    materials: HashMap<Option<u32>, u32>,
    /// Slot names by ufbx's material ID.
    slot_names: Vec<String>,
    /// Per slot.
    slots: Vec<SlotTextures>,
    skeleton: Option<Skeleton>,
    /// Morph target name → index into `mesh.morph_targets`.
    targets: HashMap<String, usize>,
}

/// Face-corner data that one vertex holds: corners agreeing in all of it share a vertex.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CornerKey {
    /// Index into the FBX mesh's vertices (positions).
    vertex: u32,
    /// The bits of the source normal (or the face's flat normal), texture coordinates and color.
    normal: [u32; 3],
    uv: [u32; 2],
    color: [u32; 4],
}

/// Where an added vertex came from, for its skin weights and morph deltas.
#[derive(Clone, Copy)]
struct VertexSource {
    /// Index into the FBX mesh's vertices.
    vertex: u32,
    /// The normal as the file stores it, `None` for a flat one.
    source_normal: Option<DVec3>,
    /// Transformed and normalized.
    normal: DVec3,
}

impl Builder {
    /// Adds the mesh of the node at scene visit `visit`, if it has one.
    fn add_instance(&mut self, nodes: &[SceneNode<'_>], visit: usize) -> Result<(), FbxImportError> {
        let node = nodes[visit].node;
        let Some(mesh) = node.mesh.as_deref() else { return Ok(()) };
        let mesh_name = name_of(&mesh.element);
        if mesh.skin_deformers.len() > 1 {
            return Err(FbxImportError::MultipleSkins { mesh: mesh_name, count: mesh.skin_deformers.len() });
        }
        let world = dmat4(&node.geometry_to_world);
        let linear = DMat3::from_mat4(world);
        let normal_matrix = linear.inverse().transpose();
        // A negative determinant mirrors the geometry, which turns counter-clockwise front
        // faces clockwise; swap two corners to restore the winding.
        let mirrored = world.determinant() < 0.0;
        let positions: Vec<DVec3> = mesh.vertices.iter().map(|&p| world.transform_point3(dvec3(p))).collect();
        let transform_normal = |n: DVec3| (normal_matrix * n).normalize_or(DVec3::Y);

        let base = u32::try_from(self.mesh.vertices.len()).map_err(|_| FbxImportError::TooLarge)?;
        let mut sources: Vec<VertexSource> = Vec::new();
        let mut triangle = vec![0u32; 3 * mesh.max_face_triangles];
        for part in &mesh.material_parts {
            if part.num_triangles == 0 {
                continue;
            }
            let index = part.index as usize;
            let material = node.materials.get(index).or_else(|| mesh.materials.get(index)).map(|m| &**m);
            let first_index = u32::try_from(self.mesh.indices.len()).map_err(|_| FbxImportError::TooLarge)?;
            // Corners already added in this part.
            let mut added: HashMap<CornerKey, u32> = HashMap::new();
            for &face in &part.face_indices {
                let face = mesh.faces[face as usize];
                if face.num_indices < 3 {
                    continue;
                }
                let count = ufbx::triangulate_face(&mut triangle, mesh, face) as usize;
                for &[a, b, c] in triangle[..3 * count].as_chunks::<3>().0 {
                    let corners = if mirrored { [a, c, b] } else { [a, b, c] };
                    let flat = (!mesh.vertex_normal.exists).then(|| {
                        let [pa, pb, pc] =
                            corners.map(|corner| positions[mesh.vertex_indices[corner as usize] as usize]);
                        (pb - pa).cross(pc - pa).normalize_or(DVec3::Y)
                    });
                    for corner in corners {
                        let corner = corner as usize;
                        let vertex = mesh.vertex_indices[corner];
                        let source_normal =
                            mesh.vertex_normal.exists.then(|| dvec3(mesh.vertex_normal[corner]));
                        let uv = if mesh.vertex_uv.exists {
                            let uv = mesh.vertex_uv[corner];
                            [uv.x as f32, (1.0 - uv.y) as f32]
                        } else {
                            [0.0; 2]
                        };
                        let color = mesh.vertex_color.exists.then(|| {
                            let c = mesh.vertex_color[corner];
                            [c.x, c.y, c.z, c.w].map(|channel| channel as f32)
                        });
                        let key_normal = source_normal.or(flat).expect("one of them").as_vec3().to_array();
                        let key = CornerKey {
                            vertex,
                            normal: key_normal.map(f32::to_bits),
                            uv: uv.map(f32::to_bits),
                            color: color.unwrap_or([1.0; 4]).map(f32::to_bits),
                        };
                        let index = match added.get(&key) {
                            Some(&index) => index,
                            None => {
                                let index = u32::try_from(self.mesh.vertices.len())
                                    .map_err(|_| FbxImportError::TooLarge)?;
                                let normal = source_normal
                                    .map_or_else(|| flat.expect("one of them"), transform_normal);
                                let normal32 = normal.as_vec3().to_array();
                                self.mesh.vertices.push(Vertex {
                                    position: positions[vertex as usize].as_vec3().to_array(),
                                    normal: normal32,
                                    outline_normal: normal32,
                                    uv,
                                    color: color.map_or(VERTEX_COLOR_NEUTRAL, |[r, g, b, _]| {
                                        [unorm8(r), unorm8(g), unorm8(b)]
                                    }),
                                    flags: VertexFlags::NONE,
                                });
                                self.mesh.authored_outline.push(false);
                                sources.push(VertexSource { vertex, source_normal, normal });
                                added.insert(key, index);
                                index
                            }
                        };
                        self.mesh.indices.push(index);
                    }
                }
            }
            let index_count = u32::try_from(self.mesh.indices.len()).map_err(|_| FbxImportError::TooLarge)?;
            if index_count == first_index {
                continue;
            }
            let material = self.material_slot(material)?;
            self.mesh.lods[0].submeshes.push(Submesh {
                first_index,
                index_count: index_count - first_index,
                material,
            });
        }

        if let Some(skeleton) = self.skeleton.as_mut() {
            let weights = match skin_of(mesh) {
                Some(skin) => {
                    let inverse_world = world.inverse();
                    if !inverse_world.is_finite() {
                        return Err(FbxImportError::DegenerateTransform { node: name_of(&node.element) });
                    }
                    let binds = skin
                        .clusters
                        .iter()
                        .map(|cluster| {
                            let bone =
                                cluster.bone_node.as_deref().expect("ufbx lists only clusters with a bone");
                            let joint = skeleton.joint_of[&bone.element.typed_id];
                            skeleton.bind(joint, dmat4(&cluster.geometry_to_bone) * inverse_world)
                        })
                        .collect::<Result<Vec<u16>, _>>()?;
                    sources.iter().map(|source| vertex_weights(skin, &binds, source.vertex)).collect()
                }
                None => vec![SkinWeights::rigid(skeleton.rigid_bind(nodes, visit)?); sources.len()],
            };
            skeleton.weights.extend(weights);
        }
        self.add_morph_targets(mesh, &mesh_name, linear, normal_matrix, &sources, base)
    }

    /// Adds the mesh's blend shape channels' deltas for its new vertices (`base..`, from
    /// `sources`), merging targets by name.
    fn add_morph_targets(
        &mut self,
        mesh: &ufbx::Mesh,
        mesh_name: &str,
        linear: DMat3,
        normal_matrix: DMat3,
        sources: &[VertexSource],
        base: u32,
    ) -> Result<(), FbxImportError> {
        // The merged targets this mesh has written to.
        let mut used = Vec::new();
        let channels = mesh.blend_deformers.iter().flat_map(|deformer| deformer.channels.iter());
        for (k, channel) in channels.enumerate() {
            let Some(shape) = channel.target_shape.as_deref() else { continue };
            let name = match &*channel.element.name {
                "" => format!("{mesh_name}/{k}"),
                name => name.to_owned(),
            };
            let target = *self.targets.entry(name.clone()).or_insert_with(|| {
                self.mesh.morph_targets.push(ImportedMorphTarget {
                    name: name.clone(),
                    default_weight: channel.weight as f32,
                    deltas: Vec::new(),
                });
                self.mesh.morph_targets.len() - 1
            });
            // Two of one mesh's channels under one name would move the same vertices twice.
            if used.contains(&target) {
                return Err(FbxImportError::DuplicateMorphTarget { mesh: mesh_name.to_owned(), name });
            }
            used.push(target);
            // Offsets by mesh vertex: position and, if the file has them, normal.
            let mut offsets: HashMap<u32, (DVec3, Option<DVec3>)> = HashMap::with_capacity(shape.num_offsets);
            for (i, &vertex) in shape.offset_vertices.iter().enumerate() {
                let weight = shape.offset_weights.get(i).copied().unwrap_or(1.0);
                let position =
                    shape.position_offsets.get(i).map_or(DVec3::ZERO, |&offset| dvec3(offset) * weight);
                let normal = shape.normal_offsets.get(i).map(|&offset| dvec3(offset) * weight);
                offsets.insert(vertex, (position, normal));
            }
            for (offset, source) in sources.iter().enumerate() {
                let Some(&(position, normal_offset)) = offsets.get(&source.vertex) else { continue };
                let position = (linear * position).as_vec3();
                let normal = match (source.source_normal, normal_offset) {
                    // The difference of the transformed, normalized full-weight and base normals:
                    // exact at full weight once the shader renormalizes.
                    (Some(source_normal), Some(normal_offset)) => {
                        let morphed = normal_matrix * (source_normal + normal_offset);
                        (morphed.normalize_or(source.normal) - source.normal).as_vec3()
                    }
                    _ => glam::Vec3::ZERO,
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

    fn material_slot(&mut self, material: Option<&ufbx::Material>) -> Result<u32, FbxImportError> {
        let id = material.map(|material| material.element.typed_id);
        if let Some(&slot) = self.materials.get(&id) {
            return Ok(slot);
        }
        let slot = u32::try_from(self.mesh.materials.len()).map_err(|_| FbxImportError::TooLarge)?;
        let name = id.map_or_else(|| DEFAULT_SLOT.to_owned(), |id| self.slot_names[id as usize].clone());
        self.mesh.materials.push(ImportedMaterial { name, generated: generated_material(material) });
        self.slots.push(material.map(slot_textures).unwrap_or_default());
        self.materials.insert(id, slot);
        Ok(slot)
    }

    fn finish(mut self) -> Result<FbxImport, FbxImportError> {
        if self.mesh.indices.is_empty() {
            return Err(FbxImportError::NoGeometry);
        }
        self.mesh.lods[0].vertex_count =
            u32::try_from(self.mesh.vertices.len()).map_err(|_| FbxImportError::TooLarge)?;
        if let Some(skeleton) = self.skeleton {
            self.mesh.skin = Some(ImportedSkin {
                joints: skeleton.joints,
                binds: skeleton.binds,
                weights: skeleton.weights,
            });
        }
        Ok(FbxImport { mesh: self.mesh, slots: self.slots })
    }
}

/// Mesh vertex `vertex`'s influences through `binds` (each cluster's palette entry), reduced to
/// the four heaviest.
fn vertex_weights(skin: &ufbx::SkinDeformer, binds: &[u16], vertex: u32) -> SkinWeights {
    let influences = skin.vertices.get(vertex as usize).map_or(&[][..], |weights| {
        let start = weights.weight_begin as usize;
        skin.weights.get(start..start + weights.num_weights as usize).unwrap_or_default()
    });
    let strongest =
        SkinWeights::strongest(influences.iter().map(|w| (binds[w.cluster_index as usize], w.weight as f32)));
    // A skin has at least one cluster (`skin_of`), whose entry takes unweighted vertices.
    strongest.unwrap_or(SkinWeights::rigid(binds[0]))
}

/// The anime character material an FBX material describes (see the [module docs](self)); white
/// and opaque for faces without one, as glTF's default material is.
fn generated_material(material: Option<&ufbx::Material>) -> AnimeCharacterMaterial {
    let Some(material) = material else {
        return AnimeCharacterMaterial {
            base_color: [1.0; 4],
            alpha: AlphaMode::Opaque,
            ..Default::default()
        };
    };
    let pbr = &material.pbr;
    let factor = if pbr.base_factor.has_value { pbr.base_factor.value_vec4.x } else { 1.0 };
    let color = pbr.base_color.value_vec4;
    let opacity = if pbr.opacity.has_value { pbr.opacity.value_vec4.x.clamp(0.0, 1.0) } else { 1.0 };
    let base_color = [color.x * factor, color.y * factor, color.z * factor, opacity].map(|c| c as f32);
    AnimeCharacterMaterial {
        base_color,
        alpha: if opacity < 1.0 { AlphaMode::Blend } else { AlphaMode::Opaque },
        double_sided: material.features.double_sided.enabled,
        ..AnimeCharacterMaterial::default()
    }
}

/// The textures `material` references for the uses the anime character family has.
fn slot_textures(material: &ufbx::Material) -> SlotTextures {
    let texture = |map: &ufbx::MaterialMap| map.texture.as_deref().and_then(fbx_texture);
    let pbr = &material.pbr;
    SlotTextures {
        base_color: texture(&pbr.base_color).or_else(|| texture(&material.fbx.diffuse_color)),
        normal: texture(&pbr.normal_map).or_else(|| texture(&material.fbx.normal_map)),
    }
}

/// A texture's image: its own for a file texture, its first file's for a layered or procedural one.
fn fbx_texture(texture: &ufbx::Texture) -> Option<FbxTexture> {
    let file = match texture.type_ {
        ufbx::TextureType::File => texture,
        _ => texture.file_textures.first().map(|file| &**file)?,
    };
    let relative = match &*file.relative_filename {
        "" => &*file.filename,
        relative => relative,
    };
    let relative = relative.replace('\\', "/");
    // The file's stem says more than the texture's name, which exporters make up
    // (Blender's are `base_color_texture` and the like).
    let name = match Path::new(&relative).file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) if !stem.is_empty() => stem.to_owned(),
        _ => file.element.name.to_string(),
    };
    let embedded = (!file.content.is_empty()).then(|| file.content.to_vec());
    (!relative.is_empty() || embedded.is_some()).then_some(FbxTexture { name, file: relative, embedded })
}

/// An element's name, or `#<id>` (ufbx's ID within its type) when it has none.
fn name_of(element: &ufbx::Element) -> String {
    match &*element.name {
        "" => format!("#{}", element.typed_id),
        name => name.to_owned(),
    }
}

fn dvec3(v: ufbx::Vec3) -> DVec3 {
    DVec3::new(v.x, v.y, v.z)
}

/// ufbx's affine 3 × 4 matrix, column by column.
fn dmat4(m: &ufbx::Matrix) -> DMat4 {
    DMat4::from_cols(
        DVec4::new(m.m00, m.m10, m.m20, 0.0),
        DVec4::new(m.m01, m.m11, m.m21, 0.0),
        DVec4::new(m.m02, m.m12, m.m22, 0.0),
        DVec4::new(m.m03, m.m13, m.m23, 1.0),
    )
}

fn joint_transform(transform: &ufbx::Transform) -> JointTransform {
    let ufbx::Transform { translation: t, rotation: r, scale: s } = *transform;
    JointTransform {
        translation: dvec3(t).as_vec3().to_array(),
        rotation: DQuat::from_xyzw(r.x, r.y, r.z, r.w).normalize().as_quat().to_array(),
        scale: dvec3(s).as_vec3().to_array(),
    }
}

impl fmt::Display for FbxTexture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.embedded, self.file.as_str()) {
            (Some(_), "") => write!(f, "{} (embedded)", self.name),
            (Some(_), file) => write!(f, "{file} (embedded)"),
            (None, file) => f.write_str(file),
        }
    }
}
