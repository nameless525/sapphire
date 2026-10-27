//! The two-sided garment samples (`--two-sided`, M1 C8): open garment shapes on the character's
//! left whose insides show, the way a sailor collar's, a skirt's or a hair ribbon's do, each
//! drawn twice so the difference reads in one frame.
//!
//! - **Three garments, top to bottom:** a stand collar, an open band tilted toward the camera so
//!   the inside of its far wall shows through its top; a pleated skirt, an open cone tilted the
//!   same way; and a ribbon given half a twist along its length, so it turns from its front to
//!   its back.
//! - **Two columns:** the outer one draws each garment one-sided, as glTF's single-sided
//!   materials draw: the insides are culled, and the outline hull's inside shows through where
//!   they were. The inner one draws it two-sided (`double_sided`): the insides shade as the side
//!   the camera sees, with the normal turned around, and hide the hull behind them.
//!
//! The materials are embedded in the meshes, built here, as the rim rig's panels are; the shapes
//! are procedural. Like the swatches, which stand on the same side (the two are exclusive), they
//! are props, not listed characters: they take the frame's fallback light rig, and no rim.

use std::f32::consts::{PI, TAU};

use sapphire_assets::material::{AnimeCharacterMaterial, CharacterRegion, Material};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::AppInitError;
use sapphire_render::{MeshHandle, Renderer};

use crate::stage;

/// The garments, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Garment {
    Collar,
    Skirt,
    Ribbon,
}

impl Garment {
    pub const ALL: [Self; 3] = [Self::Collar, Self::Skirt, Self::Ribbon];

    /// Its base color and its shade's.
    fn colors(self) -> ([f32; 3], [f32; 3]) {
        match self {
            // A navy sailor collar, its shade a deeper blue.
            Self::Collar => ([0.08, 0.11, 0.3], [0.45, 0.5, 0.75]),
            // A crimson skirt, shading toward plum.
            Self::Skirt => ([0.62, 0.1, 0.12], [0.55, 0.4, 0.62]),
            // A gold ribbon, shading toward warm brown.
            Self::Ribbon => ([0.92, 0.66, 0.18], [0.62, 0.45, 0.45]),
        }
    }
}

/// The samples: each garment one-sided, then each two-sided, in [`Garment::ALL`] order.
pub(crate) const SAMPLES: usize = 2 * Garment::ALL.len();

/// Rings (along a garment's height or a ribbon's width) and segments (around it, or along the
/// ribbon) of the shapes.
const RINGS: u32 = 12;
const SEGMENTS: u32 = 72;
/// A garment's size (its unit shape's scale) and the column's spacing, as fractions of the
/// character's height.
const SIZE: f32 = 0.11;
const SPACING: f32 = 0.25;
/// How far the collar and the skirt lean their tops toward the camera, in radians.
const TILT: f32 = 0.6;

/// Sample `index`'s garment and whether it draws both sides.
pub(crate) fn sample(index: usize) -> (Garment, bool) {
    (Garment::ALL[index % Garment::ALL.len()], index >= Garment::ALL.len())
}

/// Where sample `index` stands beside a character whose object-space bounds are `bounds`: in
/// two columns on its left, the two-sided one nearer, from its shoulders down to its knees.
pub(crate) fn placement(bounds: [[f32; 3]; 2], index: usize) -> Mat4 {
    let [min, max] = bounds.map(Vec3::from);
    let height = (max.y - min.y).max(0.1);
    let (garment, two_sided) = sample(index);
    let row = garment as usize;
    let column = if two_sided { 0.3 } else { 0.64 };
    let center = Vec3::new(
        min.x - column * height,
        min.y + (0.75 - SPACING * row as f32) * height,
        (min.z + max.z) * 0.5,
    );
    let turn = match garment {
        Garment::Collar | Garment::Skirt => Quat::from_rotation_x(TILT),
        Garment::Ribbon => Quat::IDENTITY,
    };
    Mat4::from_scale_rotation_translation(Vec3::splat(SIZE * height), turn, center)
}

/// Uploads the samples, each with its embedded material, in [`SAMPLES`] order.
pub(crate) fn upload_samples(renderer: &mut Renderer) -> Result<[MeshHandle; SAMPLES], AppInitError> {
    let mut samples = Vec::with_capacity(SAMPLES);
    for index in 0..SAMPLES {
        let (garment, two_sided) = sample(index);
        samples.push(stage::upload(renderer, &mesh(garment, two_sided))?);
    }
    Ok(samples.try_into().expect("one mesh per sample"))
}

/// A garment's material: flat bands in its colors, the default outline, both sides when
/// `two_sided`.
pub(crate) fn material(garment: Garment, two_sided: bool) -> AnimeCharacterMaterial {
    let (base, shade) = garment.colors();
    AnimeCharacterMaterial {
        base_color: [base[0], base[1], base[2], 1.0],
        regions: vec![CharacterRegion { shade_color: shade, ..CharacterRegion::default() }],
        double_sided: two_sided,
        ..AnimeCharacterMaterial::default()
    }
}

/// A grid of `RINGS` × `SEGMENTS` quads over `position(u, v)`, `u` around (or along) and `v`
/// down (or across), both 0 to 1. Normals are the cross of its slopes, toward the side whose
/// triangles run counter-clockwise: outside a shape whose `u` runs counter-clockwise seen from
/// above, the front of one whose `v` runs down while `u` runs right.
fn grid(position: impl Fn(f32, f32) -> Vec3) -> (Vec<Vertex>, Vec<u32>) {
    let (du, dv) = (0.25 / SEGMENTS as f32, 0.25 / RINGS as f32);
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let v = ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let u = segment as f32 / SEGMENTS as f32;
            let along_u = position(u + du, v) - position(u - du, v);
            let along_v = position(u, v + dv) - position(u, v - dv);
            let n = along_v.cross(along_u).try_normalize().unwrap_or(Vec3::Z).to_array();
            vertices.push(Vertex {
                position: position(u, v).to_array(),
                normal: n,
                outline_normal: n,
                uv: [u, v],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
        }
    }
    let stride = SEGMENTS + 1;
    let mut indices = Vec::new();
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, b, c, c, b, d]);
        }
    }
    (vertices, indices)
}

/// The point `around` (0 to 1) a circle of `radius` at `height`, counter-clockwise seen from
/// above.
fn ring(around: f32, radius: f32, height: f32) -> Vec3 {
    let phi = TAU * around;
    Vec3::new(phi.sin() * radius, height, phi.cos() * radius)
}

/// A garment's shape, at about unit size around the origin.
fn shape(garment: Garment) -> (Vec<Vertex>, Vec<u32>) {
    match garment {
        // A band flaring a little toward its top edge.
        Garment::Collar => grid(|u, v| ring(u, 1.0 + 0.15 * (1.0 - v), 0.3 - 0.6 * v)),
        // A cone from the waist to the hem, its pleats deepening toward the hem.
        Garment::Skirt => grid(|u, v| {
            let pleats = 1.0 + 0.07 * v * (12.0 * TAU * u).sin();
            ring(u, (0.55 + 0.55 * v) * pleats, 0.5 - v)
        }),
        // A strip along X, turning half a turn about its length, its middle bowed back a little.
        Garment::Ribbon => grid(|u, v| {
            let turn = PI * u;
            let across = 0.7 * (0.5 - v);
            let bow = -0.3 * (PI * u).sin();
            Vec3::new(2.2 * (u - 0.5), across * turn.cos(), across * turn.sin() + bow)
        }),
    }
}

/// A garment's mesh, with its material embedded.
fn mesh(garment: Garment, two_sided: bool) -> Mesh {
    let (vertices, indices) = shape(garment);
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&material(garment, two_sided), &swizzles)
        .expect("the garments' materials are valid");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let name = if two_sided { "two-sided garment" } else { "one-sided garment" };
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
    use sapphire_assets::material::MaterialFlags;

    use super::*;

    #[test]
    fn garments_face_outward_and_the_inner_column_draws_both_sides() {
        // The band's and the cone's far side (`u` = 0.5, at -Z) faces away from their axis.
        for garment in [Garment::Collar, Garment::Skirt] {
            let (vertices, _) = shape(garment);
            let far = &vertices[(SEGMENTS / 2) as usize];
            let outward = Vec3::new(far.position[0], 0.0, far.position[2]);
            assert!(Vec3::from(far.normal).dot(outward) > 0.0, "{garment:?}");
        }
        // The ribbon starts facing +Z and ends, half a turn on, facing -Z.
        let (vertices, _) = shape(Garment::Ribbon);
        let (start, end) = (Vec3::from(vertices[0].normal), Vec3::from(vertices[SEGMENTS as usize].normal));
        assert!(start.z > 0.9 && end.z < -0.9, "{start} {end}");
        let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
        for index in 0..SAMPLES {
            let (garment, two_sided) = sample(index);
            let cooked = Material::from_document(&material(garment, two_sided), &swizzles).expect("valid");
            let flags = MaterialFlags::from_bits(cooked.flags).expect("known bits");
            assert_eq!(flags.contains(MaterialFlags::DOUBLE_SIDED), two_sided, "{index}");
        }
    }

    #[test]
    fn the_samples_stand_left_of_the_character_in_two_columns() {
        let bounds = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];
        let centers: Vec<Vec3> =
            (0..SAMPLES).map(|i| placement(bounds, i).transform_point3(Vec3::ZERO)).collect();
        // A garment reaches 1.15 of its size across (the collar's top edge, the skirt's hem) and
        // less than its size up or down, tilted; the character is 2 m tall.
        let (across, up) = (1.15 * SIZE * 2.0, SIZE * 2.0);
        assert!(centers.iter().all(|c| c.x + across < -0.5), "clear of the character: {centers:?}");
        let rows = Garment::ALL.len();
        for row in 0..rows {
            let (one, two) = (centers[row], centers[rows + row]);
            assert!(one.x < two.x - 2.0 * across, "the one-sided column is outer: {one} {two}");
            assert_eq!(one.y, two.y);
        }
        assert!(centers.windows(2).take(rows - 1).all(|pair| pair[0].y - pair[1].y > 2.0 * up));
    }
}
