//! Procedural primitives as glTF geometry.
//!
//! Every shape sits on its local origin, Y-up, with counter-clockwise front faces. Round
//! surfaces get analytic (smooth) normals; hard edges get split vertices.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::gltf_writer::Geometry;

/// One point of a surface-of-revolution profile, in the (radius, height) half-plane.
#[derive(Clone, Copy, Debug)]
struct ProfilePoint {
    radius: f32,
    y: f32,
    /// Outward normal in the (radius, height) half-plane.
    normal: [f32; 2],
}

/// A sphere of `radius` around the origin.
pub(crate) fn sphere(radius: f32, segments: u32, rings: u32) -> Geometry {
    capsule(radius, 0.0, segments, rings / 2)
}

/// A capsule along Y: a cylinder of `half_height` above and below the origin, capped by
/// hemispheres of `radius`, with `cap_rings` rings per hemisphere.
pub(crate) fn capsule(radius: f32, half_height: f32, segments: u32, cap_rings: u32) -> Geometry {
    let mut profile = arc(radius, half_height, 0.0, FRAC_PI_2, cap_rings);
    let bottom = arc(radius, -half_height, FRAC_PI_2, PI, cap_rings);
    // Without a cylinder the two equator rings coincide; keep one so no zero-height band
    // of degenerate triangles is emitted.
    let skip = usize::from(half_height == 0.0);
    profile.extend_from_slice(&bottom[skip..]);
    lathe(&[profile], segments)
}

/// The top of a sphere, from the pole down to `max_polar_angle` (radians from +Y), open
/// at the bottom.
pub(crate) fn sphere_cap(radius: f32, max_polar_angle: f32, segments: u32, rings: u32) -> Geometry {
    lathe(&[arc(radius, 0.0, 0.0, max_polar_angle, rings)], segments)
}

/// A cone with its base disc on the origin and its apex at `height`. The side is smooth;
/// the rim is a hard edge.
pub(crate) fn cone(radius: f32, height: f32, segments: u32) -> Geometry {
    let slant = (height * height + radius * radius).sqrt();
    let side_normal = [height / slant, radius / slant];
    let down = [0.0, -1.0];
    lathe(
        &[
            vec![
                ProfilePoint { radius: 0.0, y: height, normal: side_normal },
                ProfilePoint { radius, y: 0.0, normal: side_normal },
            ],
            vec![
                ProfilePoint { radius, y: 0.0, normal: down },
                ProfilePoint { radius: 0.0, y: 0.0, normal: down },
            ],
        ],
        segments,
    )
}

/// An axis-aligned box centered on the origin with split normals: four vertices per face.
pub(crate) fn cuboid(half_extents: [f32; 3]) -> Geometry {
    // (normal axis, u axis, v axis) with u × v = normal, so the corners wind CCW.
    const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let mut geometry = Geometry::default();
    for (normal, u, v) in FACES {
        let base = geometry.positions.len() as u32;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            geometry.positions.push(std::array::from_fn(|axis| {
                (normal[axis] + su * u[axis] + sv * v[axis]) * half_extents[axis]
            }));
            geometry.normals.push(normal);
            geometry.uvs.push([(su + 1.0) / 2.0, (1.0 - sv) / 2.0]);
        }
        geometry.indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
    }
    geometry
}

/// Points on a circular arc of `radius` centered at height `center_y`, by polar angle
/// from +Y, inclusive of both ends.
fn arc(radius: f32, center_y: f32, from: f32, to: f32, steps: u32) -> Vec<ProfilePoint> {
    (0..=steps)
        .map(|k| {
            let theta = from + (to - from) * k as f32 / steps as f32;
            let (sin, cos) = theta.sin_cos();
            // Snap the poles so `lathe` sees an exact point and emits triangles, not
            // slivers.
            let sin = if sin.abs() < 1e-6 { 0.0 } else { sin };
            ProfilePoint { radius: radius * sin, y: center_y + radius * cos, normal: [sin, cos] }
        })
        .collect()
}

/// Sweeps profile strips around +Y. Consecutive points of a strip are joined by a band
/// of quads; separate strips share no vertices, which is how hard edges are made. For
/// the outside to face outward, strips run downward along the outer surface (or inward
/// along a bottom cap).
fn lathe(strips: &[Vec<ProfilePoint>], segments: u32) -> Geometry {
    let length = |strip: &[ProfilePoint]| -> f32 {
        strip.windows(2).map(|w| (w[1].radius - w[0].radius).hypot(w[1].y - w[0].y)).sum()
    };
    let total_length: f32 = strips.iter().map(|s| length(s)).sum();
    let columns = segments + 1; // The seam column is duplicated for UVs.

    let mut geometry = Geometry::default();
    let mut travelled = 0.0;
    for strip in strips {
        let first_ring = geometry.positions.len() as u32;
        for (i, point) in strip.iter().enumerate() {
            if i > 0 {
                let prev = strip[i - 1];
                travelled += (point.radius - prev.radius).hypot(point.y - prev.y);
            }
            let [nr, ny] = point.normal;
            for j in 0..columns {
                let (sin, cos) = (TAU * j as f32 / segments as f32).sin_cos();
                geometry.positions.push([point.radius * sin, point.y, point.radius * cos]);
                geometry.normals.push([nr * sin, ny, nr * cos]);
                geometry.uvs.push([j as f32 / segments as f32, travelled / total_length]);
            }
        }
        for (i, band) in strip.windows(2).enumerate() {
            let upper = first_ring + i as u32 * columns;
            let lower = upper + columns;
            for j in 0..segments {
                let (a, b, c, d) = (upper + j, upper + j + 1, lower + j, lower + j + 1);
                // A ring collapsed to a point makes one triangle of each quad degenerate.
                if band[1].radius > 0.0 {
                    geometry.indices.extend([a, c, d]);
                }
                if band[0].radius > 0.0 {
                    geometry.indices.extend([a, d, b]);
                }
            }
        }
    }
    geometry
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    /// Checks unit normals, no degenerate triangles, and that every face of a convex
    /// shape faces away from `center` and agrees with its vertex normals.
    fn assert_well_formed(geometry: &Geometry, center: Vec3) {
        assert_eq!(geometry.normals.len(), geometry.positions.len());
        assert_eq!(geometry.uvs.len(), geometry.positions.len());
        for n in &geometry.normals {
            assert!((Vec3::from(*n).length() - 1.0).abs() < 1e-5, "normal {n:?} is not unit length");
        }
        for t in geometry.indices.as_chunks::<3>().0 {
            let p = t.map(|i| Vec3::from(geometry.positions[i as usize]));
            let face = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(face.length() > 1e-9, "degenerate triangle {t:?}");
            let centroid = (p[0] + p[1] + p[2]) / 3.0;
            assert!(face.dot(centroid - center) > 0.0, "triangle {t:?} faces inward");
            let normals: Vec3 = t.iter().map(|&i| Vec3::from(geometry.normals[i as usize])).sum();
            assert!(face.dot(normals) > 0.0, "triangle {t:?} disagrees with its vertex normals");
        }
    }

    #[test]
    fn shapes_are_well_formed() {
        assert_well_formed(&sphere(0.5, 16, 12), Vec3::ZERO);
        assert_well_formed(&capsule(0.25, 0.5, 16, 6), Vec3::ZERO);
        assert_well_formed(&cone(0.5, 1.0, 16), Vec3::new(0.0, 0.25, 0.0));
        assert_well_formed(&cuboid([1.0, 0.5, 2.0]), Vec3::ZERO);
        assert_well_formed(&sphere_cap(0.5, 1.8, 16, 8), Vec3::ZERO);
    }

    #[test]
    fn round_surfaces_have_smooth_normals() {
        let ball = sphere(2.0, 16, 12);
        for (p, n) in ball.positions.iter().zip(&ball.normals) {
            assert!(Vec3::from(*p).normalize().abs_diff_eq(Vec3::from(*n), 1e-5));
        }
        let pill = capsule(0.5, 1.0, 16, 4);
        let max_y = pill.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((max_y - 1.5).abs() < 1e-6);
    }

    #[test]
    fn hard_edges_have_split_normals() {
        // Each box corner position appears three times with three different normals.
        let block = cuboid([1.0; 3]);
        let corner = [1.0, 1.0, 1.0];
        let normals: Vec<[f32; 3]> = block
            .positions
            .iter()
            .zip(&block.normals)
            .filter(|(p, _)| **p == corner)
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(normals, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

        // A cone rim point (the first column, not its seam duplicate) exists once with the
        // slanted side normal and once facing down.
        let hat = cone(1.0, 1.0, 8);
        let rim: Vec<[f32; 3]> = hat
            .positions
            .iter()
            .zip(&hat.normals)
            .filter(|(p, _)| **p == [0.0, 0.0, 1.0])
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(rim.len(), 2);
        assert!(Vec3::from(rim[0]).abs_diff_eq(Vec3::new(0.0, 1.0, 1.0).normalize(), 1e-6));
        assert_eq!(rim[1], [0.0, -1.0, 0.0]);
    }
}
