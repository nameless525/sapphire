//! Offline bakes: data derived from imported geometry.
//!
//! Spec §5.27 lists smoothed outline normals for both cooks, authored or baked by welding.
//! Vertex order, LODs and quantization are later stages ([`optimize`](crate::optimize),
//! [`ImportedMesh::pack`]).

use std::collections::HashMap;

use glam::Vec3;
use sapphire_assets::mesh::Vertex;
use serde::{Deserialize, Serialize};

use crate::mesh::ImportedMesh;

/// Default [`OutlineSettings::weld_tolerance`]: 0.1 mm. Far below modeling precision on
/// characters and props, and above `f32` rounding for object-space coordinates up to
/// about a kilometer (the spacing there is 61 µm).
pub const DEFAULT_WELD_TOLERANCE: f32 = 1e-4;

/// A triangle whose height is under this fraction of its longest edge, i.e. with
/// `|cross| < SLIVER_ASPECT × longest²`, is a sliver and contributes nothing. Its obtuse
/// corner would carry an angle weight near π, while its normal is set by rounding (or by
/// a sub-millimeter modeling wobble). Ear-clipped n-gons with collinear points make these.
const SLIVER_ASPECT: f32 = 1e-3;

/// When a welded position's weighted normal sum is shorter than this fraction of the sum
/// of its weights, its faces cancel out (the two sides of a double-sided sheet) and the
/// direction left over is rounding noise, so the shading normal is used instead.
const CANCELLATION_RATIO: f32 = 1e-3;

/// Settings for [`outline_normals`], part of a mesh's import settings
/// ([`MeshSettings`](crate::meta::MeshSettings)). Loading checks them as
/// [`Self::with_weld_tolerance`] does, and a sidecar may leave any of them out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "OutlineFields")]
pub struct OutlineSettings {
    weld_tolerance: f32,
}

/// [`OutlineSettings`] as written, before it is checked.
#[derive(Deserialize)]
#[serde(default)]
struct OutlineFields {
    weld_tolerance: f32,
}

impl Default for OutlineFields {
    fn default() -> Self {
        Self { weld_tolerance: DEFAULT_WELD_TOLERANCE }
    }
}

impl TryFrom<OutlineFields> for OutlineSettings {
    type Error = &'static str;

    fn try_from(fields: OutlineFields) -> Result<Self, Self::Error> {
        Self::with_weld_tolerance(fields.weld_tolerance)
            .ok_or("weld_tolerance must be a positive, finite number of meters")
    }
}

impl OutlineSettings {
    /// Welds vertices closer than `meters` (object space is in meters, as in glTF).
    /// Returns `None` unless `meters` is positive and finite.
    pub fn with_weld_tolerance(meters: f32) -> Option<Self> {
        (meters.is_finite() && meters > 0.0).then_some(Self { weld_tolerance: meters })
    }

    /// Distance in meters within which vertices share one outline normal.
    pub fn weld_tolerance(&self) -> f32 {
        self.weld_tolerance
    }
}

impl Default for OutlineSettings {
    fn default() -> Self {
        Self { weld_tolerance: DEFAULT_WELD_TOLERANCE }
    }
}

/// Bakes [`Vertex::outline_normal`] for inverted-hull outlines, except where the source
/// authored one ([`ImportedMesh::authored_outline`]): authored outline normals always win.
///
/// Vertices are welded by position (within [`OutlineSettings::weld_tolerance`]), and
/// every welded position gets the angle-weighted average of the face normals around it.
/// Split vertices at a hard edge therefore extrude along the same direction, so the hull
/// stays closed where the shading normals diverge. Angle weighting makes the result
/// independent of how faces are triangulated.
///
/// Slivers (see [`SLIVER_ASPECT`]) are left out of the average. Where the faces around
/// a position cancel out (a double-sided sheet) or a vertex belongs to no usable
/// triangle, the vertex keeps its shading normal. Shading normals are never modified.
///
/// # Panics
///
/// If an index is out of range; validate untrusted meshes first.
pub fn outline_normals(mesh: &mut ImportedMesh, settings: &OutlineSettings) {
    let (group_of, group_count) = weld(&mesh.vertices, settings.weld_tolerance);
    let mut sums = vec![Vec3::ZERO; group_count];
    let mut weights = vec![0.0f32; group_count];
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let corners = triangle.map(|i| i as usize);
        let p = corners.map(|i| Vec3::from(mesh.vertices[i].position));
        let Some(face_normal) = face_normal(p) else {
            continue;
        };
        for k in 0..3 {
            let angle = (p[(k + 1) % 3] - p[k]).angle_between(p[(k + 2) % 3] - p[k]);
            let group = group_of[corners[k]];
            sums[group] += face_normal * angle;
            weights[group] += angle;
        }
    }
    for (index, (vertex, &group)) in mesh.vertices.iter_mut().zip(&group_of).enumerate() {
        if mesh.authored_outline.get(index).copied().unwrap_or(false) {
            continue;
        }
        let sum = sums[group];
        // Also false for a position with no usable triangles (zero weight).
        if sum.length() > CANCELLATION_RATIO * weights[group] {
            vertex.outline_normal = sum.normalize().to_array();
        } else {
            vertex.outline_normal = vertex.normal;
        }
    }
}

/// The unit normal of a triangle, or `None` for a sliver or a degenerate triangle.
fn face_normal([a, b, c]: [Vec3; 3]) -> Option<Vec3> {
    let cross = (b - a).cross(c - a);
    let longest_squared =
        (b - a).length_squared().max((c - b).length_squared()).max((a - c).length_squared());
    // Phrased so that NaN and all-coincident corners also count as slivers.
    (cross.length() > SLIVER_ASPECT * longest_squared).then_some(cross).and_then(Vec3::try_normalize)
}

/// Assigns every vertex a weld group; vertices within `tolerance` (positive, finite) of a
/// group's first vertex join it. Returns the group of each vertex and the group count.
///
/// Positions are bucketed on a grid whose cell size equals the tolerance, so any match
/// lies in one of the 27 surrounding cells. Searching those (rather than comparing
/// rounded positions) keeps near-identical positions together when they straddle a cell
/// boundary.
pub(crate) fn weld(vertices: &[Vertex], tolerance: f32) -> (Vec<usize>, usize) {
    let cell_of = |p: Vec3| (p / tolerance).floor().to_array().map(|c| c as i64);

    let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut representatives: Vec<Vec3> = Vec::new();
    let group_of = vertices
        .iter()
        .map(|v| {
            let p = Vec3::from(v.position);
            let cell = cell_of(p);
            let existing = neighbors(cell).find_map(|neighbor| {
                cells
                    .get(&neighbor)?
                    .iter()
                    .copied()
                    .find(|&group| representatives[group].distance_squared(p) <= tolerance * tolerance)
            });
            existing.unwrap_or_else(|| {
                representatives.push(p);
                let group = representatives.len() - 1;
                cells.entry(cell).or_default().push(group);
                group
            })
        })
        .collect();
    (group_of, representatives.len())
}

fn neighbors([x, y, z]: [i64; 3]) -> impl Iterator<Item = [i64; 3]> {
    // Saturating so huge coordinates relative to a tiny tolerance can't overflow.
    let around = |c: i64| [c.saturating_sub(1), c, c.saturating_add(1)];
    around(x).into_iter().flat_map(move |x| {
        around(y).into_iter().flat_map(move |y| around(z).into_iter().map(move |z| [x, y, z]))
    })
}

#[cfg(test)]
mod tests {
    use glam::{EulerRot, Quat};
    use sapphire_assets::mesh::{Lod, Submesh, ToonMaterialParams};

    use super::*;

    fn mesh_from(vertices: Vec<Vertex>, indices: Vec<u32>) -> ImportedMesh {
        let submesh = Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 };
        ImportedMesh {
            authored_outline: vec![false; vertices.len()],
            lods: vec![Lod { vertex_count: vertices.len() as u32, error: 0.0, submeshes: vec![submesh] }],
            materials: vec![ToonMaterialParams::default()],
            vertices,
            indices,
            ..ImportedMesh::default()
        }
    }

    fn vertex(position: Vec3, normal: Vec3) -> Vertex {
        Vertex {
            position: position.to_array(),
            normal: normal.to_array(),
            outline_normal: normal.to_array(),
            ..Vertex::default()
        }
    }

    fn bake(mesh: &mut ImportedMesh) {
        outline_normals(mesh, &OutlineSettings::default());
    }

    /// Indices for triangle `[a, b, c]`, wound so its normal agrees with `n`.
    fn facing(positions: &[Vec3], [a, b, c]: [u32; 3], n: Vec3) -> [u32; 3] {
        let p = |i: u32| positions[i as usize];
        if (p(b) - p(a)).cross(p(c) - p(a)).dot(n) < 0.0 { [a, c, b] } else { [a, b, c] }
    }

    /// A unit cube with split normals: 4 vertices per face, 2 triangles per face.
    fn hard_edged_cube() -> ImportedMesh {
        // (normal, u, v) with u × v = normal, so the corners below wind counter-clockwise.
        let faces = [
            (Vec3::X, Vec3::Y, Vec3::Z),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::Z, Vec3::X),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y, Vec3::X),
        ];
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for (n, u, v) in faces {
            let base = vertices.len() as u32;
            vertices.extend([n - u - v, n + u - v, n + u + v, n - u + v].map(|p| vertex(p, n)));
            indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
        }
        mesh_from(vertices, indices)
    }

    #[test]
    fn cube_corners_get_the_diagonal() {
        let mut cube = hard_edged_cube();
        let shading: Vec<[f32; 3]> = cube.vertices.iter().map(|v| v.normal).collect();
        bake(&mut cube);

        for (v, shading) in cube.vertices.iter().zip(shading) {
            // Each face's split diagonal touches two of its corners twice (45° + 45°) and
            // the others once (90°). Only angle weighting gives every face equal say.
            let expected = Vec3::from(v.position).signum().normalize();
            assert!(Vec3::from(v.outline_normal).abs_diff_eq(expected, 1e-6), "{v:?}");
            assert_eq!(v.normal, shading, "shading normals must stay split");
        }
    }

    #[test]
    fn authored_outline_normals_win_over_the_bake() {
        let mut cube = hard_edged_cube();
        // The +X face (vertices 0–3) carries authored outline normals; the rest are baked.
        let authored = Vec3::new(1.0, 0.2, 0.0).normalize().to_array();
        for vertex in 0..4 {
            cube.authored_outline[vertex] = true;
            cube.vertices[vertex].outline_normal = authored;
        }
        bake(&mut cube);
        for (index, v) in cube.vertices.iter().enumerate() {
            if index < 4 {
                assert_eq!(v.outline_normal, authored, "vertex {index} is authored");
            } else {
                let expected = Vec3::from(v.position).signum().normalize();
                assert!(Vec3::from(v.outline_normal).abs_diff_eq(expected, 1e-6), "vertex {index}: {v:?}");
            }
        }
    }

    #[test]
    fn result_does_not_depend_on_triangulation() {
        let mut a = hard_edged_cube();
        let mut b = hard_edged_cube();
        // Flip the diagonal on every face of `b`.
        for face in b.indices.as_chunks_mut::<6>().0 {
            let base = face[0];
            *face = [0, 1, 3, 1, 2, 3].map(|i| base + i);
        }
        bake(&mut a);
        bake(&mut b);
        for (va, vb) in a.vertices.iter().zip(&b.vertices) {
            assert!(Vec3::from(va.outline_normal).abs_diff_eq(vb.outline_normal.into(), 1e-6));
        }
    }

    #[test]
    fn welds_near_duplicates_across_grid_cells() {
        let tolerance = DEFAULT_WELD_TOLERANCE;
        let boundary = (0.5 / tolerance).floor() * tolerance;
        let positions = [
            Vec3::new(boundary - 0.05 * tolerance, 0.0, 0.0),
            Vec3::new(boundary + 0.05 * tolerance, 0.0, 0.0),
            Vec3::new(boundary + 2.0 * tolerance, 0.0, 0.0),
        ];
        let cell = |p: Vec3| (p.x / tolerance).floor();
        assert_ne!(cell(positions[0]), cell(positions[1]), "test points must straddle a cell boundary");

        let vertices: Vec<Vertex> = positions.iter().map(|&p| vertex(p, Vec3::Y)).collect();
        let (groups, count) = weld(&vertices, tolerance);
        assert_eq!(groups[0], groups[1], "near-duplicates in neighboring cells must weld");
        assert_ne!(groups[2], groups[0], "points two tolerances apart must not weld");
        assert_eq!(count, 2);
    }

    #[test]
    fn weld_tolerance_is_absolute_and_configurable() {
        // A 1 km scene: a relative tolerance would weld millimeter-apart vertices here.
        let positions = [Vec3::ZERO, Vec3::new(0.001, 0.0, 0.0), Vec3::new(1000.0, 0.0, 0.0)];
        let vertices: Vec<Vertex> = positions.iter().map(|&p| vertex(p, Vec3::Y)).collect();
        assert_eq!(weld(&vertices, DEFAULT_WELD_TOLERANCE).1, 3);
        let coarse = OutlineSettings::with_weld_tolerance(0.01).unwrap();
        assert_eq!(weld(&vertices, coarse.weld_tolerance()).1, 2);

        assert_eq!(OutlineSettings::default().weld_tolerance(), 1e-4);
        for invalid in [0.0, -1e-4, f32::NAN, f32::INFINITY] {
            assert_eq!(OutlineSettings::with_weld_tolerance(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn double_sided_sheets_keep_their_shading_normals() {
        // A rotated, offset double-sided quad. The back is wound the other way and, as a
        // different triangulator may do, split along the other diagonal, so the two
        // sides' angle weights differ per triangle and cancel only up to rounding.
        let rotation = Quat::from_euler(EulerRot::XYZ, 0.31, -0.9, 2.2);
        let corners = [Vec3::ZERO, Vec3::X, Vec3::X + Vec3::Y, Vec3::Y]
            .map(|p| rotation * (p + Vec3::new(3.7, -1.3, 0.2)));
        let n = rotation * Vec3::Z;
        for back in [[4, 6, 5, 4, 7, 6], [4, 7, 5, 5, 7, 6]] {
            let mut vertices: Vec<Vertex> = corners.iter().map(|&p| vertex(p, n)).collect();
            vertices.extend(corners.iter().map(|&p| vertex(p, -n)));
            let mut indices = vec![0, 1, 2, 0, 2, 3];
            indices.extend(back);
            let mut sheet = mesh_from(vertices, indices);
            bake(&mut sheet);
            for v in &sheet.vertices {
                assert_eq!(v.outline_normal, v.normal, "back {back:?}: {v:?}");
            }
        }
    }

    #[test]
    fn slivers_do_not_steer_outline_normals() {
        // A flat 2 × 1 strip whose top edge has a midpoint M on the segment A–B, fanned
        // around D, plus the zero-area triangle (A, M, B) an ear-clipper emits. M is
        // collinear exactly, nearly in-plane, and nearly out-of-plane.
        let rotation = Quat::from_euler(EulerRot::XYZ, 0.7, 1.3, -0.4);
        let n = rotation * Vec3::Y;
        for midpoint in [Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 1e-6), Vec3::new(1.0, 1e-6, 0.0)] {
            let local = [Vec3::ZERO, midpoint, Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 1.0), Vec3::Z];
            let positions = local.map(|p| rotation * p);
            let indices: Vec<u32> = [[4, 1, 0], [4, 2, 1], [4, 3, 2], [0, 1, 2]]
                .into_iter()
                .flat_map(|t| facing(&positions, t, n))
                .collect();
            let mut strip = mesh_from(positions.iter().map(|&p| vertex(p, n)).collect(), indices);
            bake(&mut strip);
            for v in &strip.vertices {
                let deviation = Vec3::from(v.outline_normal).angle_between(n).to_degrees();
                assert!(deviation < 0.05, "midpoint {midpoint}: {deviation:.3}° off");
            }
        }
    }

    #[test]
    fn slivers_in_randomly_oriented_fans() {
        // The review's search: 2000 rotated, offset n-gon fans with a collinear midpoint
        // on the A–B edge, plus the (A, M, B) sliver. Before slivers were skipped, 1768
        // midpoints were more than 10° off and the worst was 89.9°.
        let mut seed = 12345u64;
        let mut random = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        let mut worst = 0.0f32;
        for _ in 0..2000 {
            let rotation = Quat::from_euler(EulerRot::XYZ, random() * 6.0, random() * 6.0, random() * 6.0);
            let offset = Vec3::new(random(), random(), random()) * 3.0;
            let n = rotation * Vec3::Y;
            let t = random();
            let local = [
                Vec3::ZERO,
                Vec3::new(t * 0.3, 0.0, 0.0),
                Vec3::new(0.3, 0.0, 0.0),
                Vec3::new(0.3, 0.0, 0.2),
                Vec3::new(0.0, 0.0, 0.2),
            ];
            let positions = local.map(|p| rotation * p + offset);
            let mut indices: Vec<u32> = [[4, 1, 0], [4, 2, 1], [4, 3, 2]]
                .into_iter()
                .flat_map(|t| facing(&positions, t, n))
                .collect();
            indices.extend([0, 1, 2]);
            let mut fan = mesh_from(positions.iter().map(|&p| vertex(p, n)).collect(), indices);
            bake(&mut fan);
            worst = worst.max(Vec3::from(fan.vertices[1].outline_normal).angle_between(n).to_degrees());
        }
        assert!(worst < 0.05, "worst midpoint deviation {worst:.3}°");
    }

    #[test]
    fn thin_but_real_triangles_still_count() {
        // Height 1% of the longest edge: well above the sliver cutoff.
        let n = Vec3::Z;
        let positions = [Vec3::ZERO, Vec3::X, Vec3::new(0.5, 0.01, 0.0)];
        assert!(face_normal(positions).is_some_and(|f| f.abs_diff_eq(n, 1e-6)));
        assert_eq!(face_normal([Vec3::ZERO, Vec3::X, Vec3::new(0.5, 1e-5, 0.0)]), None);
        assert_eq!(face_normal([Vec3::ONE; 3]), None);
        assert_eq!(face_normal([Vec3::ZERO, Vec3::X, Vec3::NAN]), None);

        // A vertex whose only triangle is thin but real still gets that face's normal.
        let mut thin = mesh_from(positions.iter().map(|&p| vertex(p, Vec3::Y)).collect(), vec![0, 1, 2]);
        bake(&mut thin);
        assert!(thin.vertices.iter().all(|v| Vec3::from(v.outline_normal).abs_diff_eq(n, 1e-6)));
    }

    #[test]
    fn handles_degenerate_input() {
        let mut point = mesh_from(vec![vertex(Vec3::ONE, Vec3::Y); 3], vec![0, 1, 2]);
        bake(&mut point);
        assert!(point.vertices.iter().all(|v| v.outline_normal == [0.0, 1.0, 0.0]));

        let mut empty = mesh_from(Vec::new(), Vec::new());
        bake(&mut empty);
    }
}
