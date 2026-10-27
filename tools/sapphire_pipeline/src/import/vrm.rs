//! VRM importer for `.vrm` files (M1 A6): VRM 1.0, and VRM 0.x migrated to 1.0's semantics.
//!
//! A VRM file is a GLB whose extensions describe a character: `VRMC_vrm` (humanoid,
//! expressions, look-at, meta), `VRMC_materials_mtoon` per material, `VRMC_springBone` and
//! `VRMC_node_constraint` per node; VRM 0.x keeps the same ideas in one `VRM` extension. They
//! are read through pixiv's `vrm-spec` crate, except node constraints, which it has no types for
//! (the `data` module). A VRM source cooks to two assets:
//!
//! - **The mesh** (`.smesh`, the source's own ID): the scene through the glTF importer (A5),
//!   so skins, morph targets, LODs and outline normals work as for any glTF, with these
//!   differences:
//!   - Every node the VRM's data names (humanoid bones, spring joints and centers, colliders,
//!     constraints) is a joint of the skeleton, skinned or not.
//!   - MToon materials replace the glTF importer's generated materials, through the mapping in
//!     [`mtoon`]; as generated materials they reference no textures (A3), so they draw with
//!     their factors. `extract-materials` writes the textured ones out as documents with the
//!     file's images ([`crate::extract`]).
//!   - `COLOR_0` is ignored, as MToon ignores vertex colors: the vertex color's character data
//!     (`sapphire_assets::mesh`) is neutral, unoccluded with the material's Z offset, but for
//!     its outline-width scale (G), the material's outline width texture sampled at each
//!     vertex (or 1).
//! - **The rig** (`.srig`, a [sub-asset](crate::cook::SubAsset) with an ID derived from the
//!   source's): humanoid bones, expressions as morph-target weights, look-at, spring bones and
//!   constraints, by joint, morph target and material slot of that mesh
//!   (`sapphire_assets::rig`). Nothing simulates it until M2.
//!
//! **VRM 0.x** files (the `VRM` extension, without `VRMC_vrm`) migrate on import: their data
//! as the `v0` module describes, their materials as `Mtoon::from_v0` does ([`mtoon`]), and the
//! model turns half a turn about Y to face +Z as VRM 1.0 models do. A file with both extensions
//! is read as 1.0.
//!
//! **Dropped:** `meta` (licensing lives in `content/third_party/<name>/SOURCE.md`, M1 plan §3),
//! `firstPerson`'s mesh annotations (first-person views are out of scope), animations
//! (`VRMC_vrm_animation`, M2), `VRMC_springBone_extended_collider`, and what the MToon mapping
//! lists.
//!
//! # The sidecar
//!
//! `importer: Vrm((mesh: (…), version: Some(V0)))`: the mesh settings are glTF's
//! ([`MeshSettings`], material bindings included), and `version` notes which VRM the file is,
//! written when the sidecar is created. `V0` says the file is migrated from VRM 0.x on import, so
//! anyone reading the sidecar knows its materials and coordinates are converted. The note is
//! checked: a file that no longer matches it (a 0.x file re-exported as 1.0) fails to import
//! until the note is corrected, because its materials and rig then come out differently.

pub mod mtoon;

mod data;
mod rig;
#[cfg(test)]
pub(crate) mod samples;
#[cfg(test)]
mod tests;
mod v0;

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use gltf::image::Source;
use gltf::texture::WrappingMode;
use gltf::{Document, Gltf};
use sapphire_assets::material::{EyeLayer, EyeShading, HighlightMotion, IrisShape, RegionGi, ThroughHair};
use sapphire_assets::mesh::VERTEX_COLOR_NEUTRAL;
use sapphire_assets::rig::{CharacterRig, RIG_EXTENSION};
use sapphire_reflect::Versioned;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use vrm_spec::vrm_0_0::VRM0Schema;
use vrm_spec::vrmc_spring_bone_1_0::VrmcSpringBoneSchema;
use vrm_spec::vrmc_vrm_1_0::VRMCVrmSchema;

use self::data::VrmData;
use self::mtoon::{Mtoon, OutlineWidth, TextureCheck};
use crate::cache::Stage;
use crate::color::unorm8;
use crate::cook::{Recipe, SubAsset};
use crate::import::gltf::{self as gltf_import, GltfImportError, SceneImport};
use crate::mesh::ImportedMesh;
use crate::meta::MeshSettings;

/// This importer, as cache keys know it, with the glTF importer's version in its low 16 bits:
/// the mesh is that importer's, so a bump there must recook VRM files too. Bump the high bits
/// whenever the VRM part of the output changes for the same file. 4: VRoid Studio's face, skin
/// and hair materials take the stylized GI's region weights (M1 C2 follow-up). 3: VRoid
/// Studio's eye materials map onto anime eyes' layers (M1 C6). 2: MToon's rim maps onto the
/// depth-offset rim's Fresnel gate and light gate (M1 C5).
pub const IMPORTER: Stage = Stage { name: "vrm", version: (4 << 16) | gltf_import::IMPORTER.version };

/// An iris's parallax depth (M1 C6): this share of its height, in meters.
const IRIS_PARALLAX: f32 = 0.1;

/// Packing a rig. Bump the version whenever it changes output for the same import.
pub const RIG_COOKER: Stage = Stage { name: "srig", version: 1 };

/// The vertex color channel holding the outline-width scale (`sapphire_assets::mesh`).
const OUTLINE_WIDTH_CHANNEL: usize = 1;

/// A VRM source's rig.
pub const RIG: SubAsset = SubAsset {
    name: "rig",
    recipe: Recipe {
        importer: IMPORTER,
        cooker: RIG_COOKER,
        format: CharacterRig::VERSION,
        extension: RIG_EXTENSION,
    },
};

/// Extension names.
const VRMC_VRM: &str = vrm_spec::vrmc_vrm_1_0::VRMC_VRM;
const VRM0: &str = vrm_spec::vrm_0_0::VRM;
const MTOON: &str = vrm_spec::vrmc_materials_mtoon_1_0::VRMC_MATERIALS_MTOON;
const SPRING_BONE: &str = vrm_spec::vrmc_spring_bone_1_0::VRMC_SPRING_BONE;
const CONSTRAINT: &str = "VRMC_node_constraint";
const UNLIT: &str = "KHR_materials_unlit";

/// Which VRM a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VrmVersion {
    /// VRM 0.x, migrated to 1.0 on import.
    V0,
    V1,
}

impl VrmVersion {
    /// The name used in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::V0 => "0.x",
            Self::V1 => "1.0",
        }
    }
}

/// Import settings for VRM files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VrmSettings {
    /// As for glTF meshes: outline bake, LODs, position format and material bindings.
    pub mesh: MeshSettings,
    /// Which VRM the file was when the sidecar was written; see the [module docs](self).
    /// `None` notes nothing and checks nothing.
    pub version: Option<VrmVersion>,
}

/// Errors from importing a VRM file.
#[derive(Debug, thiserror::Error)]
pub enum VrmImportError {
    #[error("not a valid glTF 2.0 file")]
    Parse(#[source] gltf::Error),
    #[error(transparent)]
    Gltf(#[from] GltfImportError),
    #[error("not a VRM file: it has neither the {VRMC_VRM} (VRM 1.0) nor the {VRM0} (VRM 0.x) extension")]
    NotVrm,
    #[error("{extension} version {found:?} is not supported; Sapphire reads 1.0")]
    SpecVersion { extension: &'static str, found: String },
    #[error("the {extension} extension is invalid")]
    Extension {
        extension: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{what} {problem}")]
    Reference { what: String, problem: String },
    #[error(
        "the sidecar notes a VRM {} file, but the file is VRM {}; correct `version` in the sidecar \
         (VRM 0.x files are migrated on import)",
        sidecar.name(),
        file.name()
    )]
    VersionMismatch { sidecar: VrmVersion, file: VrmVersion },
    #[error("image {index} {problem}")]
    Image { index: usize, problem: String },
    #[error("failed to decode image {index}")]
    Decode {
        index: usize,
        #[source]
        source: image::ImageError,
    },
}

impl VrmImportError {
    /// A reference to something that doesn't exist, or can't be what it names.
    pub(crate) fn reference(what: String, problem: String) -> Self {
        Self::Reference { what, problem }
    }
}

/// A VRM file, imported. See the [module docs](self).
#[derive(Debug)]
pub struct VrmImport {
    pub version: VrmVersion,
    pub mesh: ImportedMesh,
    pub rig: CharacterRig,
    /// Per material slot of `mesh`, the MToon material it maps (`None` for a material the
    /// glTF importer generated: neither MToon nor unlit).
    pub slots: Vec<Option<Mtoon>>,
    document: Document,
    buffers: Vec<Vec<u8>>,
}

/// An image embedded in a VRM file, as encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedImage<'a> {
    /// An index into the document's images.
    pub index: usize,
    /// The image's name, else its first texture's, if either has one.
    pub name: Option<String>,
    pub format: ImageFormat,
    pub bytes: &'a [u8],
}

/// The image formats glTF allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

/// The VRM version of a file, or `None` if it isn't a VRM file (or not glTF at all).
pub fn detect_version(bytes: &[u8]) -> Option<VrmVersion> {
    Gltf::from_slice(bytes).ok().and_then(|gltf| version_of(&gltf.document))
}

fn version_of(document: &Document) -> Option<VrmVersion> {
    if document.extension_value(VRMC_VRM).is_some() {
        Some(VrmVersion::V1)
    } else if document.extension_value(VRM0).is_some() {
        Some(VrmVersion::V0)
    } else {
        None
    }
}

/// Imports a VRM file: its mesh, rig and MToon materials. `version` is the sidecar's note,
/// which the file must match.
pub fn import(bytes: &[u8], version: Option<VrmVersion>) -> Result<VrmImport, VrmImportError> {
    let Read { document, buffers, version, data, v0, scene } = read(bytes, version)?;
    let rig = rig::build(&document, &data, &scene)?;
    let SceneImport { mut mesh, slot_of_material, .. } = scene;
    let images = Images { document: &document, buffers: &buffers };
    let materials = match &v0 {
        None => document
            .materials()
            .map(|material| material_v1(images, &material))
            .collect::<Result<Vec<_>, _>>()?,
        Some(vrm) => materials_v0(images, vrm)?,
    };
    let mut slots = vec![None; mesh.materials.len()];
    for (material, slot) in slot_of_material {
        if let Some(mut mtoon) = material.and_then(|material| materials[material].clone()) {
            mtoon.eye = vroid_eye(&mesh, slot);
            mtoon.gi = vroid_region_gi(&mesh.materials[slot as usize].name);
            mesh.materials[slot as usize].generated = mtoon.to_material(|_| None);
            slots[slot as usize] = Some(mtoon);
        }
    }
    let mut import = VrmImport { version, mesh, rig, slots, document, buffers };
    import.bake_outline_widths()?;
    if version == VrmVersion::V0 {
        turn_to_face_forward(&mut import.mesh);
    }
    Ok(import)
}

/// The eye layer VRoid Studio's name for a material says it draws (M1 C6), from the name's parts:
/// VRoid names materials `<model>_<Part>_<nn>_<GROUP>`, as `N00_000_00_EyeIris_00_EYE
/// (Instance)`. The eye white, eye line, lashes and brows are the eye itself; then the iris and
/// the highlights. Faces (`Face_00_SKIN`), mouths and everything else draw no eye.
pub fn vroid_eye_layer(name: &str) -> Option<EyeLayer> {
    name.split(|c: char| c == '_' || c.is_whitespace()).find_map(|part| match part {
        "EyeWhite" | "FaceEyeline" | "FaceEyelash" | "FaceBrow" => Some(EyeLayer::Eye),
        "EyeIris" => Some(EyeLayer::Iris),
        "EyeHighlight" => Some(EyeLayer::Highlight(HighlightMotion::default())),
        _ => None,
    })
}

/// The stylized GI's weights (M1 C2, [`RegionGi`]) for a material by VRoid Studio's name for
/// it, from the group the name ends in (`<model>_<Part>_<nn>_<GROUP>`; hair may number its
/// materials after it, as `N00_000_Hair_00_HAIR_01 (Instance)`):
///
/// - `HAIR`, the strands (the bangs among them) and `HairBack`, the cap under them: the head's
///   sphere, `(sphere 1, skin 0)`.
/// - `SKIN` on the `Face` part (`Face_00_SKIN`), the face's skin: the sphere and the skin
///   tint, `(1, 1)`.
/// - `SKIN` on any other part (`Body_00_SKIN`), the body's skin: the skin tint, `(0, 1)`.
/// - Anything else keeps `(0, 0)`, the GI its normals give it: clothes (`CLOTH`), the eyes
///   (`EYE`), and the `FACE` group, which holds what VRoid draws on or inside the face rather
///   than its skin (brows, lash line, lashes, the mouth's inside).
///
/// Neither VRM 1.0 nor MToon has a field saying which material is a face, skin or hair, so the
/// names are the only signal, and a name VRoid Studio didn't write (or an artist renamed) keeps
/// `(0, 0)` until the weights are set on its extracted document.
pub fn vroid_region_gi(name: &str) -> RegionGi {
    let parts = || name.split(|c: char| c == '_' || c.is_whitespace());
    let (sphere, skin) = match parts().find(|&part| part == "HAIR" || part == "SKIN") {
        Some("HAIR") => (1.0, 0.0),
        Some(_) if parts().any(|part| part == "Face") => (1.0, 1.0),
        Some(_) => (0.0, 1.0),
        None => (0.0, 0.0),
    };
    RegionGi { sphere, skin }
}

/// The eye settings of `mesh`'s slot `slot` (M1 C6), if VRoid Studio's name for it says it draws
/// eyes ([`vroid_eye_layer`]): drawn through the bangs at the default opacity. An iris takes the
/// ellipse its vertices' texture coordinates span, and a parallax depth of [`IRIS_PARALLAX`] of
/// their height.
fn vroid_eye(mesh: &ImportedMesh, slot: u32) -> Option<EyeShading> {
    let layer = vroid_eye_layer(&mesh.materials[slot as usize].name)?;
    let mut eye = EyeShading { layer, through_hair: Some(ThroughHair::default()), ..EyeShading::default() };
    if layer == EyeLayer::Iris {
        let (mut uv_min, mut uv_max) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        let (mut low, mut high) = (f32::INFINITY, f32::NEG_INFINITY);
        let submeshes = mesh.lods[0].submeshes.iter().filter(|submesh| submesh.material == slot);
        for submesh in submeshes {
            let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
            for &index in &mesh.indices[range] {
                let vertex = &mesh.vertices[index as usize];
                for axis in 0..2 {
                    uv_min[axis] = uv_min[axis].min(vertex.uv[axis]);
                    uv_max[axis] = uv_max[axis].max(vertex.uv[axis]);
                }
                low = low.min(vertex.position[1]);
                high = high.max(vertex.position[1]);
            }
        }
        if uv_max[0] > uv_min[0] && uv_max[1] > uv_min[1] {
            let center = [0.5 * (uv_min[0] + uv_max[0]), 0.5 * (uv_min[1] + uv_max[1])];
            let radius = [0.5 * (uv_max[0] - uv_min[0]), 0.5 * (uv_max[1] - uv_min[1])];
            eye.iris = Some(IrisShape { center, radius });
            eye.parallax = IRIS_PARALLAX * (high - low);
        }
    }
    Some(eye)
}

/// Imports only a VRM file's rig: [`import`] without its materials and vertex work.
pub fn import_rig(bytes: &[u8], version: Option<VrmVersion>) -> Result<CharacterRig, VrmImportError> {
    let Read { document, data, scene, .. } = read(bytes, version)?;
    rig::build(&document, &data, &scene)
}

/// What both imports start from.
struct Read {
    document: Document,
    buffers: Vec<Vec<u8>>,
    version: VrmVersion,
    data: VrmData,
    /// A VRM 0.x file's extension, whose material properties its materials migrate from.
    v0: Option<VRM0Schema>,
    scene: SceneImport,
}

fn read(bytes: &[u8], expected: Option<VrmVersion>) -> Result<Read, VrmImportError> {
    let Gltf { document, blob } = Gltf::from_slice(bytes).map_err(VrmImportError::Parse)?;
    // VRM files are GLB, so everything is in the file: no base directory.
    let buffers = gltf_import::load_buffers(&document, None, blob)?;
    let version = version_of(&document).ok_or(VrmImportError::NotVrm)?;
    if let Some(expected) = expected
        && expected != version
    {
        return Err(VrmImportError::VersionMismatch { sidecar: expected, file: version });
    }
    let (data, v0) = match version {
        VrmVersion::V1 => {
            let vrm: VRMCVrmSchema = extension(&document, VRMC_VRM)?.ok_or(VrmImportError::NotVrm)?;
            if vrm.spec_version != "1.0" {
                return Err(VrmImportError::SpecVersion { extension: VRMC_VRM, found: vrm.spec_version });
            }
            let springs: Option<VrmcSpringBoneSchema> = extension(&document, SPRING_BONE)?;
            (VrmData::from_v1(&document, &vrm, springs.as_ref())?, None)
        }
        VrmVersion::V0 => {
            let vrm: VRM0Schema = extension(&document, VRM0)?.ok_or(VrmImportError::NotVrm)?;
            (VrmData::from_v0(&document, &vrm)?, Some(vrm))
        }
    };
    let scene = gltf_import::import_scene(&document, &buffers, &data.nodes())?;
    Ok(Read { document, buffers, version, data, v0, scene })
}

/// The root extension `name`, if the document has it.
fn extension<T: DeserializeOwned>(
    document: &Document,
    name: &'static str,
) -> Result<Option<T>, VrmImportError> {
    document
        .extension_value(name)
        .map(|value| {
            T::deserialize(value).map_err(|source| VrmImportError::Extension { extension: name, source })
        })
        .transpose()
}

fn material_name(material: &gltf::Material<'_>) -> String {
    match (material.name(), material.index()) {
        (Some(name), _) => format!("{name:?}"),
        (None, index) => format!("#{}", index.unwrap_or_default()),
    }
}

/// A VRM 1.0 material: MToon, unlit, or neither.
fn material_v1(images: Images<'_>, material: &gltf::Material<'_>) -> Result<Option<Mtoon>, VrmImportError> {
    let name = material_name(material);
    if let Some(value) = material.extension_value(MTOON) {
        let schema = Deserialize::deserialize(value)
            .map_err(|source| VrmImportError::Extension { extension: MTOON, source })?;
        let check = TextureCheck { material: &name, images };
        return Mtoon::from_v1(material, &schema, &check).map(Some);
    }
    Ok(material.extension_value(UNLIT).map(|_| Mtoon::unlit(material)))
}

/// VRM 0.x's materials: each glTF material's `materialProperties` entry (by index) says its
/// shader. `VRM/MToon` migrates; UniVRM's unlit shaders are unlit; anything else (glTF
/// shading, `Standard`) is the glTF material, unlit if it says so.
fn materials_v0(images: Images<'_>, vrm: &VRM0Schema) -> Result<Vec<Option<Mtoon>>, VrmImportError> {
    let properties = vrm.material_properties.as_deref().unwrap_or_default();
    images
        .document
        .materials()
        .enumerate()
        .map(|(index, material)| {
            let shader = properties.get(index).and_then(|properties| properties.shader.as_deref());
            match shader {
                Some("VRM/MToon") => {
                    let name = material_name(&material);
                    let check = TextureCheck { material: &name, images };
                    Mtoon::from_v0(&properties[index], &check).map(Some)
                }
                Some(shader) if shader.starts_with("VRM/Unlit") || shader == "UniGLTF/UniUnlit" => {
                    Ok(Some(Mtoon::unlit(&material)))
                }
                _ => Ok(material.extension_value(UNLIT).map(|_| Mtoon::unlit(&material))),
            }
        })
        .collect()
}

/// A document's textures and images, read from its buffers.
#[derive(Clone, Copy)]
struct Images<'a> {
    document: &'a Document,
    buffers: &'a [Vec<u8>],
}

impl<'a> Images<'a> {
    /// Textures in the document.
    fn textures(self) -> usize {
        self.document.textures().len()
    }

    /// The image texture `texture` shows. The texture must exist.
    fn image_of(self, texture: usize) -> usize {
        self.document.textures().nth(texture).expect("texture references were checked").source().index()
    }

    fn embedded(self, index: usize) -> Result<EmbeddedImage<'a>, VrmImportError> {
        let image = self.document.images().nth(index).ok_or_else(|| VrmImportError::Image {
            index,
            problem: format!("doesn't exist; the document has {}", self.document.images().len()),
        })?;
        let problem = |problem: String| VrmImportError::Image { index, problem };
        let (bytes, mime): (&[u8], &str) = match image.source() {
            Source::View { view, mime_type } => {
                let buffer = &self.buffers[view.buffer().index()];
                let bytes = buffer
                    .get(view.offset()..view.offset() + view.length())
                    .ok_or_else(|| problem("lies outside its buffer".into()))?;
                (bytes, mime_type)
            }
            Source::Uri { uri, .. } => {
                // VRM exporters write buffer views; a URI (even a `data:` one) isn't a VRM image.
                let what = if uri.starts_with("data:") {
                    "a data: URI".to_owned()
                } else {
                    format!("the file {uri:?}")
                };
                return Err(problem(format!("is {what}; VRM files embed their images in buffer views")));
            }
        };
        let format = match mime {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            other => {
                return Err(problem(format!(
                    "is {other}; Sapphire reads PNG and JPEG images from VRM files"
                )));
            }
        };
        let name = image.name().map(str::to_owned).or_else(|| {
            self.document
                .textures()
                .filter(|texture| texture.source().index() == index)
                .find_map(|texture| texture.name().map(str::to_owned))
        });
        Ok(EmbeddedImage { index, name, format, bytes })
    }

    fn decode(self, index: usize) -> Result<image::DynamicImage, VrmImportError> {
        let embedded = self.embedded(index)?;
        let format = match embedded.format {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        };
        image::load_from_memory_with_format(embedded.bytes, format)
            .map_err(|source| VrmImportError::Decode { index, source })
    }

    /// One channel of image `index`, decoded (linear: data textures aren't sRGB).
    fn mask(self, index: usize, channel: mtoon::Channel) -> Result<Mask, VrmImportError> {
        let decoded = self.decode(index)?.into_rgba32f();
        let channel = match channel {
            mtoon::Channel::R | mtoon::Channel::Rgba => 0,
            mtoon::Channel::G => 1,
            mtoon::Channel::B => 2,
        };
        let values = decoded.pixels().map(|pixel| pixel.0[channel]).collect();
        Ok(Mask { width: decoded.width(), height: decoded.height(), values })
    }
}

impl VrmImport {
    fn images(&self) -> Images<'_> {
        Images { document: &self.document, buffers: &self.buffers }
    }

    /// The document's image that texture `texture` shows.
    pub fn image_of(&self, texture: usize) -> usize {
        self.images().image_of(texture)
    }

    /// Image `index` as the file embeds it. Only PNG and JPEG images in a buffer view, as VRM
    /// exporters write them, can be read.
    pub fn image(&self, index: usize) -> Result<EmbeddedImage<'_>, VrmImportError> {
        self.images().embedded(index)
    }

    /// Image `index`, decoded.
    pub fn decode(&self, index: usize) -> Result<image::DynamicImage, VrmImportError> {
        self.images().decode(index)
    }

    /// Sets every vertex color to neutral data but for the outline-width scale (G) its
    /// material's outline width texture gives it, sampled bilinearly at its UV with the
    /// texture's wrap modes.
    fn bake_outline_widths(&mut self) -> Result<(), VrmImportError> {
        for vertex in &mut self.mesh.vertices {
            vertex.color = VERTEX_COLOR_NEUTRAL;
        }
        let mut masks: HashMap<(usize, mtoon::Channel), Mask> = HashMap::new();
        let mut baked = vec![false; self.mesh.vertices.len()];
        for submesh in self.mesh.lods[0].submeshes.clone() {
            let Some(Some(mtoon)) = self.slots.get(submesh.material as usize) else { continue };
            let Some(texture) = mtoon.outline_width_multiply_texture else { continue };
            if mtoon.outline_width == OutlineWidth::None {
                continue;
            }
            let image = self.image_of(texture.texture);
            let mask = match masks.entry((image, texture.channel)) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(self.images().mask(image, texture.channel)?),
            };
            let sampler = self.document.textures().nth(texture.texture).expect("checked").sampler();
            let wrap = (sampler.wrap_s(), sampler.wrap_t());
            let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
            for &index in &self.mesh.indices[range] {
                let index = index as usize;
                if !std::mem::replace(&mut baked[index], true) {
                    let vertex = &mut self.mesh.vertices[index];
                    vertex.color[OUTLINE_WIDTH_CHANNEL] = unorm8(mask.sample(vertex.uv, wrap));
                }
            }
        }
        Ok(())
    }
}

/// One channel of a decoded image, linear.
struct Mask {
    width: u32,
    height: u32,
    /// Row by row from the top.
    values: Vec<f32>,
}

impl Mask {
    /// Bilinear at `uv` (glTF's: origin at the top left), texel centers at half texels.
    fn sample(&self, [u, v]: [f32; 2], (wrap_u, wrap_v): (WrappingMode, WrappingMode)) -> f32 {
        let axis = |coordinate: f32, size: u32, wrap: WrappingMode| {
            // Far-out UVs are clamped first, so the texel index can't overflow; at a billion
            // texels f32 has no fraction left to sample anyway.
            let x = (coordinate * size as f32 - 0.5).clamp(-1e9, 1e9);
            let floor = x.floor();
            let size = size as i64;
            let fold = |i: i64| -> usize {
                let i = match wrap {
                    WrappingMode::Repeat => i.rem_euclid(size),
                    WrappingMode::ClampToEdge => i.clamp(0, size - 1),
                    WrappingMode::MirroredRepeat => {
                        let m = i.rem_euclid(2 * size);
                        if m < size { m } else { 2 * size - 1 - m }
                    }
                };
                i as usize
            };
            // NaN UVs land on texel 0 rather than panicking.
            let base = if floor.is_finite() { floor as i64 } else { 0 };
            (fold(base), fold(base + 1), if floor.is_finite() { x - floor } else { 0.0 })
        };
        let (x0, x1, fx) = axis(u, self.width, wrap_u);
        let (y0, y1, fy) = axis(v, self.height, wrap_v);
        let at = |x: usize, y: usize| self.values[y * self.width as usize + x];
        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// Turns a VRM 0.x model half a turn about Y, `(x, y, z) → (−x, y, −z)`, to face +Z (see
/// [`v0`]): positions, normals and morph deltas turn, and joints' rest transforms and inverse
/// bind matrices are conjugated by the turn, so every joint's space turns with the model and
/// skinning gives the turned pose.
fn turn_to_face_forward(mesh: &mut ImportedMesh) {
    let turn = |[x, y, z]: [f32; 3]| [-x, y, -z];
    for vertex in &mut mesh.vertices {
        vertex.position = turn(vertex.position);
        vertex.normal = turn(vertex.normal);
        vertex.outline_normal = turn(vertex.outline_normal);
    }
    for target in &mut mesh.morph_targets {
        for delta in &mut target.deltas {
            delta.position = turn(delta.position);
            delta.normal = turn(delta.normal);
        }
    }
    if let Some(skin) = &mut mesh.skin {
        for joint in &mut skin.joints {
            joint.rest.translation = turn(joint.rest.translation);
            // The turn is the quaternion (0, 1, 0, 0); conjugating by it negates x and z.
            let [x, y, z, w] = joint.rest.rotation;
            joint.rest.rotation = [-x, y, -z, w];
        }
        // R · M · R with R = diag(−1, 1, −1, 1), column-major: entry (row, column) scales by
        // the sign of its row and of its column.
        const SIGN: [f32; 4] = [-1.0, 1.0, -1.0, 1.0];
        for bind in &mut skin.binds {
            for (column, values) in bind.inverse_bind_matrix.iter_mut().enumerate() {
                for (row, value) in values.iter_mut().enumerate() {
                    *value *= SIGN[row] * SIGN[column];
                }
            }
        }
    }
}
