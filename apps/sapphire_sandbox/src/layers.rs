//! The material layer samples (`--materials`, M1 C7): the anime character shader's layers on
//! shapes beside the character, opposite the swatches.
//!
//! - **A column of four spheres, top to bottom:** crimson lacquer (a crisp Blinn highlight and a
//!   little gloss matcap), gold (a tinted color matcap, a highlight taking the albedo, and the
//!   reflection add on the layer mask's metallic stripes), leather (the matcap's grayscale falloff
//!   multiplying the albedo, a soft GGX highlight) and a dark suit with breathing cyan light lines
//!   (emission, blooming as the world's light).
//! - **A leg beside them:** skin in a thigh-high black stocking, sheer facing the camera and
//!   opaque toward its silhouette, with a sheen along the leg and a fine weave, bare above the
//!   stocking's top where the layer mask leaves it.
//!
//! The materials, matcap and layer mask are content (the pipeline's `samples::materials` writes
//! their sources), loaded by asset ID from the shared pack with the textures they name
//! ([`Content::layer_materials`]). The shapes are procedural, as the swatches are: `u` runs
//! around them and `v` from the top down, the leg's from the thigh to the ankle, which the layer
//! mask is laid out by. They are props, not listed characters: they take the frame's fallback
//! light rig, and no rim.

use std::f32::consts::{PI, TAU};

use sapphire_assets::AssetId;
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

/// The samples: the four spheres top to bottom, then the leg.
pub(crate) const SAMPLES: usize = 5;
/// Spheres in the column; the leg is the last sample.
const SPHERES: usize = 4;

/// The samples' materials, in [`SAMPLES`] order, by their sidecars' IDs
/// (`content/samples/materials/{lacquer,gold,leather,glow,stockings}.material.ron.meta`).
pub(crate) const MATERIALS: [AssetId; SAMPLES] = [
    AssetId::from_text("069e09ce-189e-4a20-97b5-f23a9dc42329"),
    AssetId::from_text("dbec58b1-375e-492b-8f4e-fc85904fc269"),
    AssetId::from_text("0d4441bd-bb3c-4522-a644-6a93f892cf75"),
    AssetId::from_text("47681bba-997c-49c8-9b07-39ad73f9a40f"),
    AssetId::from_text("637dc594-fbc3-41c8-ba91-e227d774cf4b"),
];

/// Rings and segments of the shapes.
const RINGS: u32 = 48;
const SEGMENTS: u32 = 64;
/// A sphere's radius and the column's spacing, as fractions of the character's height.
const RADIUS: f32 = 0.1;
const SPACING: f32 = 0.235;
/// The leg's length and its radius along it, thigh to ankle (`v` 0 to 1), as fractions of the
/// character's height: knots at `(v, radius)`.
const LEG_LENGTH: f32 = 0.78;
const LEG_PROFILE: [(f32, f32); 6] =
    [(0.0, 0.075), (0.3, 0.06), (0.48, 0.042), (0.55, 0.046), (0.75, 0.034), (1.0, 0.026)];

/// Where sample `index` stands beside a character whose object-space bounds are `bounds`: the
/// spheres in a column right of it, from about a sixth of its height to its shoulders, the leg
/// standing beyond them on the floor the character stands on.
pub(crate) fn placement(bounds: [[f32; 3]; 2], index: usize) -> Mat4 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    let depth = (min.z + max.z) * 0.5;
    if index < SPHERES {
        let center = Vec3::new(
            max.x + 0.3 * height,
            min.y + (0.17 + SPACING * (SPHERES - 1 - index) as f32) * height,
            depth,
        );
        Mat4::from_scale_rotation_translation(Vec3::splat(RADIUS * height), Quat::IDENTITY, center)
    } else {
        // The leg's mesh runs from the thigh at the origin down to the ankle, in units of the
        // character's height.
        let top = Vec3::new(max.x + 0.52 * height, min.y + (LEG_LENGTH + 0.04) * height, depth);
        Mat4::from_scale_rotation_translation(Vec3::splat(height), Quat::IDENTITY, top)
    }
}

/// Loads the samples' materials, with the textures they name, onto `renderer` through `content`,
/// and uploads a shape drawn with each, in [`SAMPLES`] order.
pub(crate) fn upload_samples(
    content: &mut Content,
    gpu: &GpuContext,
    renderer: &mut Renderer,
) -> Result<[MeshHandle; SAMPLES], AppInitError> {
    let materials = content.layer_materials(renderer, gpu)?;
    let (sphere, leg) = (sphere().to_cooked()?, leg().to_cooked()?);
    let (sphere, leg) = (Mesh::read(&sphere)?, Mesh::read(&leg)?);
    let mut samples = Vec::with_capacity(SAMPLES);
    for (index, material) in materials.into_iter().enumerate() {
        let mesh = if index < SPHERES { sphere } else { leg };
        samples.push(renderer.upload_mesh(mesh, &[material])?);
    }
    Ok(samples.try_into().expect("one mesh per material"))
}

/// A surface of revolution around Y: `RINGS` rings from `v` 0 at the top to 1 at the bottom, each
/// at `(height, radius)` from `profile(v)`, `SEGMENTS` segments around in `u`. Normals come from
/// the profile's slope, so tapers shade as they turn.
fn revolve(profile: impl Fn(f32) -> (f32, f32)) -> Mesh {
    let mut vertices = Vec::new();
    let flags = VertexFlags::NONE;
    let stride = SEGMENTS + 1;
    for ring in 0..=RINGS {
        let v = ring as f32 / RINGS as f32;
        let (y, r) = profile(v);
        // The profile's slope, for the normal: dr/dy along the surface.
        let step = 0.5 / RINGS as f32;
        let (above, below) = (profile((v - step).max(0.0)), profile((v + step).min(1.0)));
        let (dy, dr) = (below.0 - above.0, below.1 - above.1);
        for segment in 0..=SEGMENTS {
            let u = segment as f32 / SEGMENTS as f32;
            let phi = TAU * u;
            let around = Vec3::new(phi.sin(), 0.0, phi.cos());
            // Perpendicular to the profile's tangent (dr outward, dy up) in the meridian plane.
            let n = (around * -dy + Vec3::Y * dr).try_normalize().unwrap_or(around).to_array();
            vertices.push(Vertex {
                position: (around * r + Vec3::Y * y).to_array(),
                normal: n,
                outline_normal: n,
                uv: [u, v],
                color: VERTEX_COLOR_NEUTRAL,
                flags,
            });
        }
    }
    let mut indices = Vec::new();
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            // Counter-clockwise seen from outside.
            indices.extend([a, b, c, c, b, d]);
        }
    }
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    // A placeholder the draws never use: each sample draws with its own uploaded material.
    let placeholder =
        Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("the default is valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded("sample", placeholder)],
        PositionFormat::Unorm16,
    )
}

/// A unit sphere, `v` from the top pole down.
fn sphere() -> Mesh {
    revolve(|v| {
        let theta = PI * v;
        (theta.cos(), theta.sin())
    })
}

/// The leg, from the thigh at the origin down to the ankle, closed by caps; its radius eases
/// between [`LEG_PROFILE`]'s knots.
fn leg() -> Mesh {
    revolve(|v| {
        let radius = LEG_PROFILE.windows(2).find(|pair| v <= pair[1].0).map_or(
            LEG_PROFILE[LEG_PROFILE.len() - 1].1,
            |pair| {
                let t = ((v - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0.0, 1.0);
                let smooth = t * t * (3.0 - 2.0 * t);
                pair[0].1 + (pair[1].1 - pair[0].1) * smooth
            },
        );
        // Rounded ends: the radius closes over the first and last few rings.
        let cap = 0.04;
        let end = (v / cap).min((1.0 - v) / cap).min(1.0);
        (-v * LEG_LENGTH, radius * (1.0 - (1.0 - end).powi(2)).sqrt())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shapes_run_v_from_the_top_down_with_normals_outward() {
        for (mesh, name) in [(sphere(), "sphere"), (leg(), "leg")] {
            let count = mesh.streams.len();
            let first = mesh.streams.vertex(0).expect("a vertex");
            let last = mesh.streams.vertex(count - 1).expect("a vertex");
            assert!(first.position[1] > last.position[1], "{name}: top first");
            assert!(first.uv[1] < 1e-4 && last.uv[1] > 1.0 - 1e-4, "{name}: v");
            let middle = mesh.streams.vertex((RINGS / 2 * (SEGMENTS + 1)) as usize).expect("a vertex");
            let outward = Vec3::new(middle.position[0], 0.0, middle.position[2]);
            assert!(Vec3::from(middle.normal).dot(outward) > 0.0, "{name}: outward");
        }
    }

    #[test]
    fn the_samples_stand_right_of_the_character_on_its_floor() {
        let bounds = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];
        let centers: Vec<Vec3> =
            (0..SPHERES).map(|i| placement(bounds, i).transform_point3(Vec3::ZERO)).collect();
        assert!(centers.iter().all(|c| c.x - RADIUS * 2.0 > 0.5), "clear of the character: {centers:?}");
        assert!(
            centers.windows(2).all(|pair| pair[0].y - pair[1].y > 2.0 * RADIUS * 2.0),
            "apart: {centers:?}"
        );
        let leg = placement(bounds, SPHERES);
        let ankle = leg.transform_point3(Vec3::new(0.0, -LEG_LENGTH, 0.0));
        assert!(ankle.y > 0.0 && ankle.y < 0.2, "the ankle just above the floor: {ankle}");
        assert!(ankle.x > centers[0].x, "beyond the spheres");
    }
}
