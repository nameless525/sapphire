//! Reflection cubes (M5 L2): what a reflection probe sees of the scene under each time-of-day key, on
//! the grid's tracer, with the sky left out and how much of it each direction sees kept in alpha, so
//! the renderer adds the live sky itself (the pipeline's `reflections` module has the asset).
//!
//! # A texel
//!
//! Each texel of each face casts [`SUBSAMPLES`]² rays from the probe through fixed points of its
//! footprint, in the renderer's cube convention (`probes.wgsl`'s `cube_texel_direction`: faces +X, −X,
//! +Y, −Y, +Z, −Z, rows down the face). A ray that escapes adds its share to alpha and nothing to the
//! color. A ray that meets a surface's front adds its share of the light that surface sends back under
//! each key:
//!
//! - the key light **at the hit itself**, through W4's environment shading with its own shadow ray, so
//!   a reflected shadow's edge is as sharp as the cube's texels;
//! - the sky through the nearest surfel's visibility, and the light the surfels' last bounce brought
//!   there, both smooth enough for a surfel a metre apart to stand for;
//! - its emission under the key, as the renderer scales it (M1 W1's keyed emission);
//!
//! all of it times the material's albedo as the grid's bake sees it (its color map's mean), like a
//! surfel's light. A hit beyond the surfels' reach is lit under the open sky, as the grid's far hits are.
//! A ray that meets a one-sided surface's back adds nothing at all: it is inside something.
//!
//! What a probe holds is radiance in the renderer's normalization, linear RGB, before exposure. Local
//! lights are left out, as the grid leaves them out: a zone's lanterns reach a cube only through their
//! materials' emission.
//!
//! # Determinism
//!
//! Scalar `f32` arithmetic in a fixed order, no transcendental functions, and each texel computed on
//! its own into its own slot, so the same scene, keys and probes give the same bits on every machine
//! and whatever the number of threads.

#[cfg(test)]
mod tests;

use glam::Vec3;
use sapphire_math::sh::{self, SkyTransfer};

use super::{
    BakeError, BakeKey, BakeScene, Bvh, Lighting, RAY_OFFSET, Rays, SurfelLight, Surfels, Target, key_light,
    light_surfels, material, parallel_map, place_surfels, sky_irradiance,
};

/// Rays per texel along each side: four in all, at the quarters of its footprint.
pub const SUBSAMPLES: u32 = 2;
/// Meters between the surfels that carry the sky's light and the bounce to a hit.
const SURFEL_SPACING: f32 = 1.0;
/// Rays each surfel casts over its hemisphere.
const SURFEL_RAYS: u32 = 32;
/// Light bounces between surfaces, as every grid of the repository bakes.
const BOUNCES: u32 = 2;
/// The smallest and largest faces a cube may have: the renderer's mip chain stops at 8 texels.
pub const MIN_SIZE: u32 = 8;
pub const MAX_SIZE: u32 = 256;

/// A probe to bake: where it stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubeProbe {
    pub position: Vec3,
}

/// Bakes a cube of `size` texels a side for each probe and each key: per probe, per key, per face, per
/// row from the top, per texel from the left, linear RGB radiance and the share of the texel that sees
/// the sky. Surfels carry the sky's light and the bounce within `reach` (grown by the grid's margin), the
/// box the probes' surroundings fill. On up to `threads` threads; the bytes don't depend on how many.
pub fn bake_cubes(
    scene: &BakeScene,
    keys: &[BakeKey],
    probes: &[CubeProbe],
    reach: (Vec3, Vec3),
    size: u32,
    threads: usize,
) -> Result<Vec<[f32; 4]>, BakeError> {
    if !size.is_power_of_two() || !(MIN_SIZE..=MAX_SIZE).contains(&size) {
        return Err(BakeError::Settings("a cube's side is a power of two from 8 to 256"));
    }
    if probes.is_empty() || keys.is_empty() {
        return Err(BakeError::Settings("cubes are baked for at least one probe and one key"));
    }
    if !probes.iter().all(|probe| probe.position.is_finite()) {
        return Err(BakeError::Settings("a probe's position isn't finite"));
    }
    let (min, max) = reach;
    if !(min.is_finite() && max.is_finite() && min.cmplt(max).all()) {
        return Err(BakeError::Bounds { min, max });
    }
    let bvh = Bvh::new(scene.triangles.iter().map(|triangle| triangle.positions).collect());
    let rays = Rays { bvh: &bvh, scene };
    let surfels = place_surfels(scene, min, max, SURFEL_SPACING);
    let lighting = Lighting { surfel_rays: SURFEL_RAYS, bounces: BOUNCES };
    let light = light_surfels(scene, keys, lighting, &rays, &surfels, threads);
    let transfers: Vec<[SkyTransfer; 3]> =
        keys.iter().map(|key| key.sky.each_ref().map(SkyTransfer::new)).collect();
    let tracer = Tracer { scene, keys, rays: &rays, surfels: &surfels, light: &light, transfers: &transfers };

    let face_texels = (size * size) as usize;
    let per_probe = 6 * face_texels;
    // Each texel's keys, texels in probe, face, row, column order.
    let texels = parallel_map(threads, probes.len() * per_probe, |index| {
        let (probe, texel) = (index / per_probe, index % per_probe);
        let face = (texel / face_texels) as u32;
        let (row, column) = ((texel % face_texels) as u32 / size, (texel % face_texels) as u32 % size);
        tracer.texel(probes[probe].position, face, column, row, size)
    });
    // Reordered to probe, key, face, row, column: each key's six faces together, as they upload.
    let mut out = vec![[0.0; 4]; probes.len() * keys.len() * per_probe];
    for (index, values) in texels.into_iter().enumerate() {
        let (probe, texel) = (index / per_probe, index % per_probe);
        for (key, value) in values.into_iter().enumerate() {
            out[(probe * keys.len() + key) * per_probe + texel] = value;
        }
    }
    Ok(out)
}

/// The unit direction through point `(s, t)` of cube face `face` (each −1 to 1, `t` down the face), as a
/// cube-map lookup reads it: `probes.wgsl`'s `cube_texel_direction`.
pub fn texel_direction(face: u32, s: f32, t: f32) -> Vec3 {
    let (right, up, forward) = match face {
        0 => (Vec3::NEG_Z, Vec3::Y, Vec3::X),
        1 => (Vec3::Z, Vec3::Y, Vec3::NEG_X),
        2 => (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        3 => (Vec3::X, Vec3::Z, Vec3::NEG_Y),
        4 => (Vec3::X, Vec3::Y, Vec3::Z),
        _ => (Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z),
    };
    (forward + s * right - t * up).normalize()
}

/// What a texel's rays read.
struct Tracer<'a> {
    scene: &'a BakeScene,
    keys: &'a [BakeKey],
    rays: &'a Rays<'a>,
    surfels: &'a Surfels,
    light: &'a SurfelLight,
    transfers: &'a [[SkyTransfer; 3]],
}

impl Tracer<'_> {
    /// Texel `(column, row)` of face `face` of a cube of `size` from `position`, per key.
    fn texel(&self, position: Vec3, face: u32, column: u32, row: u32, size: u32) -> Vec<[f32; 4]> {
        let share = 1.0 / (SUBSAMPLES * SUBSAMPLES) as f32;
        let mut color = vec![Vec3::ZERO; self.keys.len()];
        let mut sky = 0.0;
        let mut far = Vec::new();
        for sample in 0..SUBSAMPLES * SUBSAMPLES {
            let offset = |step: u32| (step as f32 + 0.5) / SUBSAMPLES as f32;
            let s = (column as f32 + offset(sample % SUBSAMPLES)) / size as f32 * 2.0 - 1.0;
            let t = (row as f32 + offset(sample / SUBSAMPLES)) / size as f32 * 2.0 - 1.0;
            let d = texel_direction(face, s, t);
            let Some(hit) = self.rays.closest(position, d, f32::INFINITY) else {
                sky += share;
                continue;
            };
            let point = position + d * hit.t;
            far.clear();
            let target = self.surfels.target(self.scene, &hit, point, &mut far);
            if target == Target::Blocked {
                continue;
            }
            let triangle = &self.scene.triangles[hit.triangle as usize];
            let [a, b, c] = triangle.positions;
            let w = 1.0 - hit.u - hit.v;
            let mut face_normal = (b - a).cross(c - a).normalize_or(Vec3::Y);
            let shading = triangle.normals[0] * w + triangle.normals[1] * hit.u + triangle.normals[2] * hit.v;
            let mut normal = shading.normalize_or(face_normal);
            // The back of a two-sided card is a face of its own, toward the ray.
            if !hit.front {
                face_normal = -face_normal;
                normal = -normal;
            }
            if normal.dot(face_normal) <= 0.0 {
                normal = face_normal;
            }
            let material = material(self.scene, triangle.material);
            let origin = point + face_normal * RAY_OFFSET;
            for (key, total) in color.iter_mut().enumerate() {
                let (visibility, gathered) = match target {
                    Target::Surfel(surfel) => {
                        (self.light.visibility[surfel as usize], self.light.gathered(surfel, key))
                    }
                    // Out of the surfels' reach: the open sky, and no bounce.
                    _ => (sh::project_constant(1.0), Vec3::ZERO),
                };
                let lit = key_light(self.rays, &self.keys[key], material, origin, normal)
                    + sky_irradiance(&self.transfers[key], &visibility, normal)
                    + gathered;
                *total += (material.albedo * lit + material.emission_under(&self.keys[key])) * share;
            }
        }
        color.into_iter().map(|color| [color.x, color.y, color.z, sky]).collect()
    }
}
