//! glTF 2.0 importer for `.gltf` and `.glb` files.
//!
//! Flattens the default scene into one object-space [`MeshAsset`]:
//!
//! - Every node's world transform is applied to its mesh: positions by the matrix,
//!   normals by its inverse transpose. Mirrored transforms flip the triangle winding, as
//!   glTF requires (§3.7.4).
//! - Each mesh primitive (per node instance) becomes one [`Submesh`].
//! - `baseColorFactor` becomes [`ToonMaterialParams::base_color`]. Material `extras` may
//!   set `shade_color`, `outline_color` (linear RGB or RGBA) and `toon_params`
//!   (`[shadow threshold, softness, rim strength, outline width]`); everything else keeps
//!   [`ToonMaterialParams::default`]. In Blender these are custom properties on the
//!   material, exported with "Include → Custom Properties".
//! - `COLOR_0` RGB becomes the sRGB vertex tint and its alpha the outline-width scale.
//! - Missing normals are replaced by flat face normals.
//!
//! Outline normals are set to the shading normal here; [`bake::outline_normals`](crate::bake::outline_normals)
//! computes the real ones. Textures, skins, morph targets and animation are ignored
//! until the M1 pipeline.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use ::gltf::buffer::Data as BufferData;
use ::gltf::mesh::{Mode, Semantic};
use ::gltf::{Gltf, Material, Mesh, Node, Primitive};
use glam::{Mat3, Mat4, Vec3};
use sapphire_assets::mesh::{MeshAsset, MeshVertex, Submesh, ToonMaterialParams};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use crate::color::{linear_to_srgb, unorm8};

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
    #[error("failed to load buffer data")]
    Buffers(#[source] ::gltf::Error),
    #[error("the document has no scene to import")]
    NoScene,
    #[error("node {node} is its own ancestor; glTF node hierarchies must be trees")]
    NodeCycle { node: usize },
    #[error("{at}: primitive mode {mode:?} is not supported; export triangles")]
    UnsupportedMode { at: PrimitiveRef, mode: Mode },
    #[error("{at}: has no POSITION attribute")]
    MissingPositions { at: PrimitiveRef },
    #[error("{at}: the {semantic} accessor has no readable data")]
    UnreadableAccessor { at: PrimitiveRef, semantic: &'static str },
    #[error("{at}: {semantic} has {found} elements but POSITION has {expected}")]
    AttributeCount { at: PrimitiveRef, semantic: &'static str, found: usize, expected: usize },
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
    #[error("the scene contains no triangles")]
    NoGeometry,
    #[error("the scene has more than u32::MAX vertices or indices")]
    TooLarge,
}

/// Imports a `.gltf` or `.glb` file. External buffers resolve relative to the file.
pub fn import(path: &Path) -> Result<MeshAsset, GltfImportError> {
    let bytes =
        std::fs::read(path).map_err(|source| GltfImportError::Io { path: path.to_owned(), source })?;
    import_slice(&bytes, path.parent())
}

/// Imports glTF or GLB bytes. `base_dir` resolves relative buffer URIs; with `None`, only
/// self-contained files (GLB, or `data:` URIs) load.
pub fn import_slice(bytes: &[u8], base_dir: Option<&Path>) -> Result<MeshAsset, GltfImportError> {
    let Gltf { document, blob } = Gltf::from_slice(bytes).map_err(GltfImportError::Parse)?;
    let buffers = ::gltf::import_buffers(&document, base_dir, blob).map_err(GltfImportError::Buffers)?;
    // Blender always writes `scene`; fall back to the first scene for files that don't.
    let scene =
        document.default_scene().or_else(|| document.scenes().next()).ok_or(GltfImportError::NoScene)?;

    let mut builder = Builder { buffers: &buffers, mesh: MeshAsset::default(), materials: HashMap::new() };
    let mut ancestors = Vec::new();
    // No axis conversion, deliberately: glTF 2.0 (§3.4) is +Y up, meters, with assets
    // facing +Z, and so is engine object space (as is VRM 1.0). Blender's exporter already
    // converts from its +Z-up space, so source coordinates pass through unchanged.
    for node in scene.nodes() {
        builder.visit(&node, Mat4::IDENTITY, &mut ancestors)?;
    }
    builder.finish()
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

struct Builder<'a> {
    buffers: &'a [BufferData],
    mesh: MeshAsset,
    /// glTF material index (`None` = the default material) → cooked material slot.
    materials: HashMap<Option<usize>, u32>,
}

impl Builder<'_> {
    fn visit(
        &mut self,
        node: &Node<'_>,
        parent: Mat4,
        ancestors: &mut Vec<usize>,
    ) -> Result<(), GltfImportError> {
        if ancestors.contains(&node.index()) {
            return Err(GltfImportError::NodeCycle { node: node.index() });
        }
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                self.add_primitive(&mesh, &primitive, world)?;
            }
        }
        ancestors.push(node.index());
        for child in node.children() {
            self.visit(&child, world, ancestors)?;
        }
        ancestors.pop();
        Ok(())
    }

    fn add_primitive(
        &mut self,
        mesh: &Mesh<'_>,
        primitive: &Primitive<'_>,
        world: Mat4,
    ) -> Result<(), GltfImportError> {
        let at = PrimitiveRef::new(mesh, primitive);
        if primitive.mode() != Mode::Triangles {
            return Err(GltfImportError::UnsupportedMode { at, mode: primitive.mode() });
        }
        let buffers = self.buffers;
        let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|data| data.0.as_slice()));
        let declared = |semantic: Semantic| primitive.get(&semantic).is_some();

        let positions: Vec<Vec3> = match reader.read_positions() {
            Some(iter) => iter.map(|p| world.transform_point3(Vec3::from(p))).collect(),
            None if declared(Semantic::Positions) => {
                return Err(GltfImportError::UnreadableAccessor { at, semantic: "POSITION" });
            }
            None => return Err(GltfImportError::MissingPositions { at }),
        };
        let vertex_count = positions.len();

        let normals = read_attribute(&at, "NORMAL", declared(Semantic::Normals), vertex_count, || {
            reader.read_normals().map(|iter| iter.map(Vec3::from).collect())
        })?;
        let uvs = read_attribute(&at, "TEXCOORD_0", declared(Semantic::TexCoords(0)), vertex_count, || {
            reader.read_tex_coords(0).map(|iter| iter.into_f32().collect())
        })?;
        let colors = read_attribute(&at, "COLOR_0", declared(Semantic::Colors(0)), vertex_count, || {
            reader.read_colors(0).map(|iter| iter.into_rgba_f32().map(vertex_color).collect())
        })?;

        let indices: Vec<u32> = match (primitive.indices(), reader.read_indices()) {
            (_, Some(iter)) => iter.into_u32().collect(),
            (Some(_), None) => return Err(GltfImportError::UnreadableAccessor { at, semantic: "indices" }),
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
        let mirrored = world.determinant() < 0.0;
        let triangles =
            indices.as_chunks::<3>().0.iter().map(|&[a, b, c]| if mirrored { [a, c, b] } else { [a, b, c] });

        let vertex = |i: u32, normal: Vec3| {
            let i = i as usize;
            MeshVertex {
                position: positions[i].to_array(),
                normal: normal.to_array(),
                outline_normal: normal.to_array(),
                uv: uvs.as_ref().map_or([0.0; 2], |uvs| uvs[i]),
                color: colors.as_ref().map_or([u8::MAX; 4], |colors| colors[i]),
            }
        };

        let base = u32::try_from(self.mesh.vertices.len()).map_err(|_| GltfImportError::TooLarge)?;
        let first_index = u32::try_from(self.mesh.indices.len()).map_err(|_| GltfImportError::TooLarge)?;
        match normals {
            Some(normals) => {
                let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
                self.mesh.vertices.extend((0..vertex_count).map(|i| {
                    let normal = (normal_matrix * normals[i]).normalize_or(Vec3::Y);
                    vertex(i as u32, normal)
                }));
                for triangle in triangles {
                    self.mesh.indices.extend(triangle.map(|i| base + i));
                }
            }
            None => {
                // Flat shading needs a normal per face corner, so the primitive is unwelded.
                for (t, [a, b, c]) in triangles.enumerate() {
                    let [pa, pb, pc] = [a, b, c].map(|i| positions[i as usize]);
                    let normal = (pb - pa).cross(pc - pa).normalize_or(Vec3::Y);
                    self.mesh.vertices.extend([a, b, c].map(|i| vertex(i, normal)));
                    let corner = base + 3 * t as u32;
                    self.mesh.indices.extend([corner, corner + 1, corner + 2]);
                }
            }
        }

        let index_count = u32::try_from(self.mesh.indices.len()).map_err(|_| GltfImportError::TooLarge)?;
        let material_index = self.material_slot(&primitive.material())?;
        self.mesh.submeshes.push(Submesh {
            first_index,
            index_count: index_count - first_index,
            material_index,
            _reserved: 0,
        });
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

    fn finish(mut self) -> Result<MeshAsset, GltfImportError> {
        if self.mesh.indices.is_empty() {
            return Err(GltfImportError::NoGeometry);
        }
        let (min, max) = self
            .mesh
            .vertices
            .iter()
            .fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(min, max), v| {
                (min.min(v.position.into()), max.max(v.position.into()))
            });
        self.mesh.bounds = [min.to_array(), max.to_array()];
        Ok(self.mesh)
    }
}

/// Reads an optional per-vertex attribute, distinguishing "absent" from "declared but
/// unreadable" and checking its length against `POSITION`.
fn read_attribute<T>(
    at: &PrimitiveRef,
    semantic: &'static str,
    declared: bool,
    expected: usize,
    read: impl FnOnce() -> Option<Vec<T>>,
) -> Result<Option<Vec<T>>, GltfImportError> {
    match read() {
        Some(values) if values.len() != expected => {
            Err(GltfImportError::AttributeCount { at: at.clone(), semantic, found: values.len(), expected })
        }
        Some(values) => Ok(Some(values)),
        None if declared => Err(GltfImportError::UnreadableAccessor { at: at.clone(), semantic }),
        None => Ok(None),
    }
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
mod tests {
    use serde_json::json;

    use super::*;
    use crate::gltf_writer::{Geometry, GltfWriter, glb};
    use crate::test_util::TempDir;

    const EPS: f32 = 1e-5;

    fn assert_vec_eq(actual: [f32; 3], expected: Vec3) {
        assert!(Vec3::from(actual).abs_diff_eq(expected, EPS), "{actual:?} != {expected:?}");
    }

    /// One slanted triangle whose normal isn't axis-aligned, so non-uniform scale matters.
    fn slanted_triangle(with_normals: bool) -> Geometry {
        let normal = Vec3::new(0.0, -1.0, 1.0).normalize().to_array();
        Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]],
            normals: if with_normals { vec![normal; 3] } else { Vec::new() },
            indices: vec![0, 1, 2],
            ..Geometry::default()
        }
    }

    /// A document with one node per `(node JSON, geometry)`; each node's `mesh` is filled in.
    fn single_mesh_glb(node: serde_json::Value, geometry: &Geometry) -> Vec<u8> {
        let mut writer = GltfWriter::default();
        let mesh = writer.add_mesh("Mesh", &[(geometry, None)]);
        let mut node = node;
        node["mesh"] = mesh.into();
        let node = writer.add_node(node);
        writer.add_root(node);
        writer.to_glb()
    }

    fn face_normal(mesh: &MeshAsset, triangle: usize) -> Vec3 {
        let [a, b, c] =
            [0, 1, 2].map(|k| Vec3::from(mesh.vertices[mesh.indices[3 * triangle + k] as usize].position));
        (b - a).cross(c - a).normalize()
    }

    #[test]
    fn applies_hierarchy_transforms_to_positions_and_normals() {
        let mut writer = GltfWriter::default();
        let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), None)]);
        let child = writer.add_node(json!({ "name": "Child", "mesh": mesh, "scale": [1.0, 2.0, 1.0] }));
        let root =
            writer.add_node(json!({ "name": "Root", "translation": [0.0, 0.0, 5.0], "children": [child] }));
        writer.add_root(root);

        let imported = import_slice(&writer.to_glb(), None).unwrap();
        imported.validate().unwrap();
        assert_vec_eq(imported.vertices[2].position, Vec3::new(0.0, 2.0, 6.0));
        // Inverse transpose of diag(1, 2, 1) applied to (0, -1, 1), renormalized.
        let expected = Vec3::new(0.0, -1.0, 2.0).normalize();
        for v in &imported.vertices {
            assert_vec_eq(v.normal, expected);
            assert_vec_eq(v.outline_normal, expected);
        }
        assert!(face_normal(&imported, 0).abs_diff_eq(expected, EPS));
        assert_eq!(imported.bounds, [[0.0, 0.0, 5.0], [1.0, 2.0, 6.0]]);
    }

    #[test]
    fn mirrored_nodes_keep_front_faces_outward() {
        let glb = single_mesh_glb(json!({ "scale": [-1.0, 1.0, 1.0] }), &slanted_triangle(true));
        let imported = import_slice(&glb, None).unwrap();
        let normal = Vec3::from(imported.vertices[0].normal);
        assert_vec_eq(normal.to_array(), Vec3::new(0.0, -1.0, 1.0).normalize());
        assert!(face_normal(&imported, 0).dot(normal) > 0.99, "winding was not flipped");
    }

    #[test]
    fn computes_flat_normals_when_missing() {
        let mut quad = slanted_triangle(false);
        quad.positions.push([1.0, 1.0, 1.0]);
        quad.indices.extend([1, 3, 2]);
        let imported = import_slice(&single_mesh_glb(json!({}), &quad), None).unwrap();

        assert_eq!(imported.vertices.len(), 6, "flat shading unwelds every corner");
        assert_eq!(imported.indices, [0, 1, 2, 3, 4, 5]);
        for v in &imported.vertices {
            assert_vec_eq(v.normal, Vec3::new(0.0, -1.0, 1.0).normalize());
        }
    }

    #[test]
    fn converts_vertex_colors_and_reads_uvs() {
        let mut geometry = slanted_triangle(true);
        geometry.colors = vec![[128, 0, 255, 100]; 3];
        geometry.uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.25, 0.75]];
        let imported = import_slice(&single_mesh_glb(json!({}), &geometry), None).unwrap();
        // Linear 128/255 encodes to sRGB 188; alpha is linear and passes through.
        assert_eq!(imported.vertices[0].color, [188, 0, 255, 100]);
        assert_eq!(imported.vertices[2].uv, [0.25, 0.75]);

        let plain = import_slice(&single_mesh_glb(json!({}), &slanted_triangle(true)), None).unwrap();
        assert_eq!(plain.vertices[0].color, [255; 4], "no COLOR_0 means untinted, full outline");
        assert_eq!(plain.vertices[0].uv, [0.0, 0.0]);
    }

    #[test]
    fn maps_materials_and_toon_extras() {
        let mut writer = GltfWriter::default();
        let toon = writer.add_material(json!({
            "name": "Toon",
            "pbrMetallicRoughness": { "baseColorFactor": [0.5, 0.25, 0.125, 1.0] },
            "extras": {
                "shade_color": [0.1, 0.2, 0.3],
                "outline_color": [0.0, 0.0, 0.0, 0.5],
                "toon_params": [0.2, 0.05, 0.4, 0.006],
                "some_other_blender_property": "ignored",
            },
        }));
        let plain = writer.add_material(json!({ "name": "Plain", "extras": ["not", "an", "object"] }));
        let triangle = slanted_triangle(true);
        let mesh = writer.add_mesh(
            "Mesh",
            &[(&triangle, Some(toon)), (&triangle, Some(plain)), (&triangle, Some(toon)), (&triangle, None)],
        );
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);

        let imported = import_slice(&writer.to_glb(), None).unwrap();
        imported.validate().unwrap();
        let slots: Vec<u32> = imported.submeshes.iter().map(|s| s.material_index).collect();
        assert_eq!(slots, [0, 1, 0, 2], "materials are deduplicated in first-use order");
        assert_eq!(
            imported.materials[0],
            ToonMaterialParams {
                base_color: [0.5, 0.25, 0.125, 1.0],
                shade_color: [0.1, 0.2, 0.3, 1.0],
                outline_color: [0.0, 0.0, 0.0, 0.5],
                params: [0.2, 0.05, 0.4, 0.006],
            }
        );
        let defaults = ToonMaterialParams::default();
        assert_eq!(imported.materials[1], ToonMaterialParams { base_color: [1.0; 4], ..defaults });
        // The glTF default material is white.
        assert_eq!(imported.materials[2], ToonMaterialParams { base_color: [1.0; 4], ..defaults });
    }

    #[test]
    fn rejects_malformed_toon_extras() {
        let mut writer = GltfWriter::default();
        let material =
            writer.add_material(json!({ "name": "Broken", "extras": { "toon_params": [0.1, 0.2, 0.3] } }));
        let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), Some(material))]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);

        let err = import_slice(&writer.to_glb(), None).unwrap_err();
        assert!(matches!(&err, GltfImportError::MaterialExtras { material, .. } if material == "\"Broken\""));
        assert_eq!(err.to_string(), "material \"Broken\": invalid toon extras");

        let mut writer = GltfWriter::default();
        let material = writer.add_material(json!({ "extras": { "shade_color": [0.1, 0.2] } }));
        let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), Some(material))]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        let err = import_slice(&writer.to_glb(), None).unwrap_err();
        let GltfImportError::MaterialExtras { material, source } = err else { panic!("{err:?}") };
        assert_eq!(material, "#0");
        assert!(source.to_string().contains("3 (RGB) or 4 (RGBA)"), "{source}");
    }

    #[test]
    fn rejects_unsupported_input() {
        let mut writer = GltfWriter::default();
        let mesh = writer.add_mesh("Points", &[(&slanted_triangle(true), None)]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        let mut doc = writer.document(None);
        doc["meshes"][0]["primitives"][0]["mode"] = 0.into();
        let err = import_slice(&glb(&doc, writer.bin()), None).unwrap_err();
        assert!(matches!(err, GltfImportError::UnsupportedMode { mode: Mode::Points, .. }), "{err:?}");
        assert_eq!(
            err.to_string(),
            "mesh \"Points\" primitive 0: primitive mode Points is not supported; export triangles"
        );

        let mut bad_index = slanted_triangle(true);
        bad_index.indices = vec![0, 1, 7];
        let err = import_slice(&single_mesh_glb(json!({}), &bad_index), None).unwrap_err();
        assert!(matches!(err, GltfImportError::IndexOutOfRange { index: 7, vertex_count: 3, .. }));

        let mut empty = GltfWriter::default();
        let node = empty.add_node(json!({ "name": "Empty" }));
        empty.add_root(node);
        assert!(matches!(import_slice(&empty.to_glb(), None), Err(GltfImportError::NoGeometry)));

        assert!(matches!(import_slice(b"not gltf", None), Err(GltfImportError::Parse(_))));
    }

    #[test]
    fn rejects_node_cycles() {
        let mut writer = GltfWriter::default();
        let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), None)]);
        let a = writer.add_node(json!({ "mesh": mesh, "children": [1] }));
        writer.add_node(json!({ "children": [0] }));
        writer.add_root(a);
        assert!(matches!(import_slice(&writer.to_glb(), None), Err(GltfImportError::NodeCycle { node: 0 })));
    }

    #[test]
    fn imports_gltf_with_external_buffer() {
        let mut writer = GltfWriter::default();
        let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), None)]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        let json = serde_json::to_vec(&writer.document(Some("triangle.bin"))).unwrap();

        let dir = TempDir::new("external_buffer");
        std::fs::write(dir.path().join("triangle.gltf"), &json).unwrap();
        std::fs::write(dir.path().join("triangle.bin"), writer.bin()).unwrap();

        let imported = import(&dir.path().join("triangle.gltf")).unwrap();
        assert_eq!(imported.vertices.len(), 3);
        // Without a base directory the external reference can't resolve.
        assert!(matches!(import_slice(&json, None), Err(GltfImportError::Buffers(_))));
        assert!(matches!(import(&dir.path().join("missing.gltf")), Err(GltfImportError::Io { .. })));
    }
}
