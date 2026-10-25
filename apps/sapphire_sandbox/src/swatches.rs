//! The anime character shader swatches (`--swatches`, M1 C1): three spheres in a column beside
//! the character, one per way the shader colors the shadow side, over the maps a character
//! would bring.
//!
//! - **Top: ramp mode.** The swatch ramp and control map, through their own material: latitude
//!   bands of four material IDs, each with its own ramp row, and a striped shadow bias that makes
//!   the terminator jog, plus a painted shadow on the lit side and an always-lit patch on the
//!   shadow side.
//! - **Middle: ramp mode with WuWa's subsurface mix.** The same maps, the rows mixed 35% into
//!   per-region subsurface tints, mostly warm.
//! - **Bottom: band mode.** Three bands in per-region colors, the regions from the vertices'
//!   material IDs; no textures at all.
//!
//! The materials and maps are content (the pipeline's `samples` module writes their sources, A7's
//! tools cook the maps), loaded by asset ID from the shared pack with the textures they name
//! ([`Content::swatch_materials`]). The spheres are procedural, as the skinned stalk is: each is
//! cut into its bands at the seams, so a vertex's material ID is its band's, and outlines take
//! the region's color.

use std::f32::consts::{PI, TAU};

use sapphire_assets::AssetId;
use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{AnimeCharacterMaterial, Material};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{MeshHandle, Renderer};
use sapphire_rhi::GpuContext;

use crate::content::Content;

/// Spheres in the column, top to bottom.
pub(crate) const SWATCHES: usize = 3;

/// The swatches' materials, top to bottom, by their sidecars' IDs
/// (`content/samples/swatch_{ramp,subsurface,bands}.material.ron.meta`).
pub(crate) const MATERIALS: [AssetId; SWATCHES] = [
    AssetId::from_text("622db3b7-cad1-45c2-af22-a01a886d3bc2"),
    AssetId::from_text("c1975a50-65b9-4014-9604-994066fe25ea"),
    AssetId::from_text("8b6875d6-81eb-47a7-bded-495a2ca02352"),
];

/// Material IDs of the latitude bands, top to bottom, as the control map paints them.
const BANDS: [u8; 4] = [3, 0, 1, 2];
/// Rings per band and segments around.
const RINGS_PER_BAND: u32 = 8;
const SEGMENTS: u32 = 48;

/// A sphere's radius and the column's spacing, as fractions of the character's height.
const RADIUS: f32 = 0.13;
const SPACING: f32 = 0.3;

/// Where swatch `index` (0 at the top) stands beside a character whose object-space bounds are
/// `bounds`: left of it, in a column from about a quarter to three quarters of its height.
pub(crate) fn placement(bounds: [[f32; 3]; 2], index: usize) -> Mat4 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    let center = Vec3::new(
        min.x - 0.35 * height,
        min.y + (0.5 + SPACING * (1.0 - index as f32)) * height,
        (min.z + max.z) * 0.5,
    );
    Mat4::from_scale_rotation_translation(Vec3::splat(RADIUS * height), Quat::IDENTITY, center)
}

/// Loads the swatches' materials, with their maps, onto `renderer` through `content`, and uploads
/// a sphere drawn with each, top to bottom.
pub(crate) fn upload_swatches(
    content: &mut Content,
    gpu: &GpuContext,
    renderer: &mut Renderer,
) -> Result<[MeshHandle; SWATCHES], AppInitError> {
    let materials = content.swatch_materials(renderer, gpu)?;
    let mesh = sphere().to_cooked()?;
    let mesh = Mesh::read(&mesh)?;
    let mut swatches = Vec::with_capacity(SWATCHES);
    for material in materials {
        swatches.push(renderer.upload_mesh(mesh, &[material])?);
    }
    Ok(swatches.try_into().expect("one mesh per material"))
}

/// A unit UV sphere, `u` around and `v` from the top pole down, cut into four latitude bands at
/// their seams so each band's vertices carry its material ID.
fn sphere() -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let stride = SEGMENTS + 1;
    for (band, &id) in BANDS.iter().enumerate() {
        let flags = VertexFlags::with_material_id(MaterialId::new(id).expect("a material ID"));
        let first = vertices.len() as u32;
        for ring in 0..=RINGS_PER_BAND {
            let v =
                (band as u32 * RINGS_PER_BAND + ring) as f32 / (BANDS.len() as u32 * RINGS_PER_BAND) as f32;
            let theta = PI * v;
            for segment in 0..=SEGMENTS {
                let u = segment as f32 / SEGMENTS as f32;
                let phi = TAU * u;
                let n = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
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
        for ring in 0..RINGS_PER_BAND {
            for segment in 0..SEGMENTS {
                let a = first + ring * stride + segment;
                let (b, c, d) = (a + stride, a + 1, a + stride + 1);
                // Counter-clockwise seen from outside.
                indices.extend([a, c, b, c, d, b]);
            }
        }
    }
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    // A placeholder the draws never use: each swatch draws with its own uploaded material.
    let placeholder =
        Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("the default is valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded("swatch", placeholder)],
        PositionFormat::Unorm16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sphere_carries_each_bands_material_id() {
        let mesh = sphere();
        let bytes = mesh.to_cooked().expect("cook");
        let read = Mesh::read(&bytes).expect("a valid mesh");
        let per_band = ((RINGS_PER_BAND + 1) * (SEGMENTS + 1)) as usize;
        assert_eq!(read.streams.len(), BANDS.len() * per_band);
        for index in 0..read.streams.len() {
            let id = mesh.streams.vertex(index).expect("in range").flags.material_id().get();
            assert_eq!(id, BANDS[index / per_band], "vertex {index}");
        }
    }

    #[test]
    fn the_column_stands_left_of_the_character() {
        let bounds = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];
        let centers: Vec<Vec3> =
            (0..SWATCHES).map(|i| placement(bounds, i).transform_point3(Vec3::ZERO)).collect();
        assert!(centers.iter().all(|c| c.x + RADIUS * 2.0 < -0.5), "clear of the character: {centers:?}");
        assert!(
            centers.windows(2).all(|pair| pair[0].y - pair[1].y > 2.0 * RADIUS * 2.0),
            "apart: {centers:?}"
        );
    }
}
