//! The see-through and glass samples (`--glass`, M5 L11): two spheres on the character's left,
//! each banded into regions by its vertices' material IDs, so a see-through region and an
//! opaque one meet on one material, as a visor's lens meets its frame.
//!
//! - **The lens,** above: its caps an opaque frame, its middle band glass, drawing a third of its
//!   pixels through the fade's dither and reflecting the sky by Fresnel over them, so the
//!   courtyard shows through it under a sheen that brightens toward its edge.
//! - **The veil,** below: its caps opaque, its middle band a tinted fabric drawing half of its
//!   pixels, with no glass: what lies behind shows through it evenly.
//!
//! The materials are embedded in the meshes, built here, as the garments' are; the shapes are
//! procedural. They are props, not listed characters: they take the frame's fallback light rig,
//! and they stand where the garments do, so a scene shows one or the other.

use std::f32::consts::{PI, TAU};

use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{AnimeCharacterMaterial, CharacterRegion, Material};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{MeshHandle, Renderer};

use crate::studio;

/// The samples, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sample {
    Lens,
    Veil,
}

impl Sample {
    pub(crate) const ALL: [Self; 2] = [Self::Lens, Self::Veil];
}

/// How many samples there are, in [`Sample::ALL`] order.
pub const SAMPLES: usize = Sample::ALL.len();

/// Rings per band and segments around a sphere.
const RINGS: u32 = 8;
const SEGMENTS: u32 = 48;
/// Where the middle band starts and ends, as a share of the way from the top pole to the bottom.
const BAND: [f32; 2] = [0.3, 0.7];
/// A sphere's radius and the column's spacing, as fractions of the character's height.
const RADIUS: f32 = 0.09;
const SPACING: f32 = 0.25;

/// Where sample `index` stands beside a character whose object-space bounds are `bounds`: in a
/// column on its left, from its shoulders down.
pub(crate) fn placement(bounds: [[f32; 3]; 2], index: usize) -> Mat4 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    let center = Vec3::new(
        min.x - 0.35 * height,
        min.y + (0.7 - SPACING * index as f32) * height,
        (min.z + max.z) * 0.5,
    );
    Mat4::from_scale_rotation_translation(Vec3::splat(RADIUS * height), Quat::IDENTITY, center)
}

/// Uploads the samples, each with its embedded material, in [`SAMPLES`] order.
pub fn upload_samples(renderer: &mut Renderer) -> Result<[MeshHandle; SAMPLES], AppInitError> {
    let mut samples = Vec::with_capacity(SAMPLES);
    for sample in Sample::ALL {
        samples.push(studio::upload(renderer, &mesh(sample))?);
    }
    Ok(samples.try_into().expect("one mesh per sample"))
}

/// A sample's material: region 0 its opaque caps, region 1 its see-through band.
pub(crate) fn material(sample: Sample) -> AnimeCharacterMaterial {
    let (cap, band) = match sample {
        // A pale frame around a smoky blue lens.
        Sample::Lens => (
            CharacterRegion { shade_color: [0.55, 0.6, 0.75], ..CharacterRegion::default() },
            CharacterRegion {
                shade_color: [0.4, 0.5, 0.8],
                opacity: 0.35,
                glass: 1.0,
                roughness: 0.05,
                ..CharacterRegion::default()
            },
        ),
        // A rose veil between plum caps.
        Sample::Veil => (
            CharacterRegion { shade_color: [0.5, 0.35, 0.55], ..CharacterRegion::default() },
            CharacterRegion { shade_color: [0.7, 0.5, 0.7], opacity: 0.5, ..CharacterRegion::default() },
        ),
    };
    let base = match sample {
        Sample::Lens => [0.78, 0.8, 0.86, 1.0],
        Sample::Veil => [0.92, 0.62, 0.72, 1.0],
    };
    AnimeCharacterMaterial { base_color: base, regions: vec![cap, band], ..AnimeCharacterMaterial::default() }
}

/// The band of a unit sphere from `from` to `to` (shares of the way from the top pole to the
/// bottom), its vertices in material ID `id`, appended to `vertices` and `indices`.
fn band(from: f32, to: f32, id: u8, vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    let flags = VertexFlags::with_material_id(MaterialId::new(id).expect("a material ID"));
    let first = u32::try_from(vertices.len()).expect("a small mesh");
    for ring in 0..=RINGS {
        let v = from + (to - from) * ring as f32 / RINGS as f32;
        let theta = PI * v;
        for segment in 0..=SEGMENTS {
            let u = segment as f32 / SEGMENTS as f32;
            let phi = TAU * u;
            let n = Vec3::new(theta.sin() * phi.sin(), theta.cos(), theta.sin() * phi.cos()).to_array();
            vertices.push(Vertex {
                position: n,
                normal: n,
                outline_normal: n,
                uv: [u, v],
                color: VERTEX_COLOR_NEUTRAL,
                flags,
            });
        }
    }
    let stride = SEGMENTS + 1;
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = first + ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, b, c, c, b, d]);
        }
    }
}

/// A unit sphere: the caps in region 0 and the band in region 1, each its own vertices so the
/// regions meet at a clean edge.
fn shape() -> (Vec<Vertex>, Vec<u32>) {
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    band(0.0, BAND[0], 0, &mut vertices, &mut indices);
    band(BAND[0], BAND[1], 1, &mut vertices, &mut indices);
    band(BAND[1], 1.0, 0, &mut vertices, &mut indices);
    (vertices, indices)
}

/// A sample's mesh, with its material embedded.
fn mesh(sample: Sample) -> Mesh {
    let (vertices, indices) = shape();
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&material(sample), &swizzles)
        .expect("the glass samples' materials are valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let name = match sample {
        Sample::Lens => "glass lens",
        Sample::Veil => "see-through veil",
    };
    Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded(name, material)],
        PositionFormat::Unorm16,
    )
}

#[cfg(test)]
mod tests {
    use sapphire_assets::material::{AnimeCharacterParams, MaterialFlags};

    use super::*;

    #[test]
    fn the_bands_see_through_and_the_lens_alone_is_glass() {
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        for sample in Sample::ALL {
            let document = material(sample);
            let cooked = Material::from_document(&document, &swizzles).expect("valid");
            let flags = MaterialFlags::from_bits(cooked.flags).expect("known bits");
            assert!(flags.contains(MaterialFlags::ALPHA_TEST), "{sample:?} draws through the dither");
            let params = document.gpu_params(&swizzles);
            let glass = params.modes[0] & AnimeCharacterParams::FLAG_GLASS != 0;
            assert_eq!(glass, sample == Sample::Lens, "{sample:?}");
            assert_eq!(params.layer_regions[0].see_through, [0.0; 4], "{sample:?}: opaque caps");
        }
        // The band's vertices, and only theirs, are in region 1.
        let (vertices, _) = shape();
        let per_band = ((RINGS + 1) * (SEGMENTS + 1)) as usize;
        let ids: Vec<u8> = vertices.iter().map(|vertex| vertex.flags.material_id().get()).collect();
        assert!(ids[..per_band].iter().all(|&id| id == 0));
        assert!(ids[per_band..2 * per_band].iter().all(|&id| id == 1));
        assert!(ids[2 * per_band..].iter().all(|&id| id == 0));
    }

    #[test]
    fn the_samples_stand_left_of_the_character_in_a_column() {
        let bounds = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];
        let centers: Vec<Vec3> =
            (0..SAMPLES).map(|i| placement(bounds, i).transform_point3(Vec3::ZERO)).collect();
        let radius = RADIUS * 2.0;
        assert!(centers.iter().all(|c| c.x + radius < -0.5), "clear of the character: {centers:?}");
        assert!(centers[0].y - centers[1].y > 2.0 * radius, "apart: {centers:?}");
    }
}
