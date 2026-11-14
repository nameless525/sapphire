//! A face material's threshold masks (M5 B3), baked from its own normals as
//! `tools/blender/bake_character_masks.py` bakes the character's: a `.facesdf.ron` turns them into
//! the face SDF map the anime character shader thresholds against the light's angle.
//!
//! The face's triangles are drawn into its maps' UV space at the bind pose, each texel taking its
//! interpolated normal. At each angle `a` from 0° to 180°, a light turning from head-forward (+Z)
//! toward head-right (−X: the figure's left is +X), a texel is lit where its normal faces the light,
//! and where it was lit at every angle before: once in shadow it stays there, which is the one
//! shape a threshold map describes. Texels no triangle covers are in shadow at every angle.
//!
//! The shader mirrors the map (U to 1 − U) for a light from the other side, so the face's skin must
//! run across the map symmetrically about its middle: the middle region of an odd number of them,
//! unwrapped by a head loft turned to start at the back (`across: (0.0, 0.0, -1.0)`), whose U is ½
//! down the front.

use glam::Vec3;
use sapphire_math::det;

use super::build::Built;
use super::{FigureError, Maps};
use crate::combat::png;

/// The masks' angles, in degrees: every 15° from the front to the back.
pub const ANGLES: [f32; 13] =
    [0.0, 15.0, 30.0, 45.0, 60.0, 75.0, 90.0, 105.0, 120.0, 135.0, 150.0, 165.0, 180.0];

/// A mask is this share of its material's maps a side, and at least `MIN_MASK` texels.
const MASK_SHARE: u32 = 4;
const MIN_MASK: u32 = 64;

/// A mask's file name after its material's stem: `_threshold_045.png`.
pub fn mask_suffix(angle: f32) -> String {
    format!("_threshold_{:03}.png", angle as u32)
}

/// The threshold masks of the face primitive `primitive`, each with its angle.
pub(crate) fn masks(
    built: &Built,
    primitive: usize,
    maps: Option<Maps>,
) -> Result<Vec<(f32, Vec<u8>)>, FigureError> {
    let maps = maps.ok_or_else(|| {
        FigureError::Spec(
            "a face material bakes its masks across unwrapped maps: give the figure `maps`".to_owned(),
        )
    })?;
    let size = (maps.size / MASK_SHARE).max(MIN_MASK);
    let normals = rasterize(&built.primitives[primitive].geometry, size);
    let mut lit: Vec<bool> = normals.iter().map(Option::is_some).collect();
    let mut masks = Vec::with_capacity(ANGLES.len());
    for angle in ANGLES {
        let (sin, cos) = det::sin_cos(angle.to_radians());
        let light = Vec3::new(-sin, 0.0, cos);
        for (texel, normal) in lit.iter_mut().zip(&normals) {
            *texel = *texel && normal.is_some_and(|normal| normal.dot(light) > 0.0);
        }
        let image = image::GrayImage::from_fn(size, size, |x, y| {
            image::Luma([if lit[(y * size + x) as usize] { 255 } else { 0 }])
        });
        masks.push((angle, png(image)?));
    }
    Ok(masks)
}

/// Each texel's normal where a triangle covers its centre, in `size × size` texels over UV space.
fn rasterize(geometry: &crate::gltf_writer::Geometry, size: u32) -> Vec<Option<Vec3>> {
    let mut texels = vec![None; (size * size) as usize];
    let scale = size as f32;
    for triangle in geometry.indices.as_chunks::<3>().0 {
        let corners = triangle.map(|index| index as usize);
        let uv = corners.map(|corner| {
            let [u, v] = geometry.uvs[corner];
            (u * scale, v * scale)
        });
        let normal = corners.map(|corner| Vec3::from(geometry.normals[corner]));
        let area = (uv[1].0 - uv[0].0) * (uv[2].1 - uv[0].1) - (uv[2].0 - uv[0].0) * (uv[1].1 - uv[0].1);
        if area.abs() < 1e-9 {
            continue;
        }
        let low = |axis: fn(&(f32, f32)) -> f32| {
            uv.iter().map(axis).fold(f32::MAX, f32::min).floor().max(0.0) as u32
        };
        let high = |axis: fn(&(f32, f32)) -> f32| {
            (uv.iter().map(axis).fold(f32::MIN, f32::max).ceil() as u32).min(size)
        };
        let (x0, x1, y0, y1) = (low(|p| p.0), high(|p| p.0), low(|p| p.1), high(|p| p.1));
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let edge = |a: (f32, f32), b: (f32, f32)| {
                    ((b.0 - a.0) * (py - a.1) - (px - a.0) * (b.1 - a.1)) / area
                };
                let weights = [edge(uv[1], uv[2]), edge(uv[2], uv[0]), edge(uv[0], uv[1])];
                if weights.iter().all(|&weight| weight >= -1e-6) {
                    let sum = normal[0] * weights[0] + normal[1] * weights[1] + normal[2] * weights[2];
                    texels[(y * size + x) as usize] = sum.try_normalize();
                }
            }
        }
    }
    texels
}
