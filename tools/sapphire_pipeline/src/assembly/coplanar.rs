//! The coplanar check's search: triangles of different placements that face the same way and
//! share a plane where they overlap, which the depth buffer can't order (see the module docs
//! of [`super`]).
//!
//! Candidates come from buckets of like normals and, within a bucket, slabs of like offsets
//! along its normal, swept along an axis in its plane; only pairs whose extents meet reach the
//! exact test, [`fight`]. No pair is lost at a bucket's boundary: a triangle also joins as a
//! guest every bucket within [`ANGLE`] of its normal, and each pair is tested in one bucket,
//! the home of one of its triangles.

use std::collections::HashMap;

use glam::{IVec3, Vec2, Vec3};

/// Faces fight only when their normals are within this many degrees: a steeper crossing is an
/// edge the depth buffer resolves.
pub const ANGLE: f32 = 2.5;
/// Where such faces are nearer each other than this, in meters along the normal, they count as
/// one plane. Reverse-Z `Depth32Float` orders faces a hundredth of this apart at a kilometer;
/// the rest is margin for transforms and rasterizers that interpolate depth differently.
pub const GAP: f32 = 0.001;
/// A fight narrower than this, in meters, across every direction is none: pieces placed edge
/// to edge touch along a line.
pub const WIDTH: f32 = 0.002;
/// Faces in one slot fight only where they shade differently: occlusion more than this apart
/// (four of the vertex color's 255 levels: ambient light 1.6% apart, a level or so of a frame's
/// 255), ...
pub const OCCLUSION: f32 = 4.0 / 255.0;
/// ...normals more than this many degrees apart, ...
pub const NORMAL: f32 = 0.5;
/// ...or texture coordinates more than this many tiles apart, less whole tiles (the ground
/// builtins wrap theirs by blocks of tiles).
pub const TEXTURE: f32 = 1.0 / 1024.0;

/// Normal buckets are cubes this wide in normal space, about multiples of it, so an axis's
/// normal and its neighborhood (the chord of [`ANGLE`], under half a bucket) fall in one.
const CELL: f32 = 0.1;

/// A triangle as the check sees it.
#[derive(Clone, Copy, Debug)]
pub struct Face {
    /// In the assembly's space, wound counter-clockwise about `normal` (unit length).
    pub corners: [Vec3; 3],
    pub normal: Vec3,
    pub slot: usize,
    /// The placement it comes from.
    pub placement: usize,
    /// What it draws with at each corner.
    pub shading: [Shading; 3],
}

/// A corner's vertex attributes that its slot's material reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shading {
    pub normal: Vec3,
    pub uv: Vec2,
    pub occlusion: f32,
}

/// Two faces that fight: indices into the faces, the lower first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlap {
    pub faces: [usize; 2],
    /// The middle of the region where they fight, in the assembly's space.
    pub at: Vec3,
}

/// Every pair of faces from different placements that fight, ordered by their indices.
pub fn overlaps(faces: &[Face]) -> Vec<Overlap> {
    // A hair over the angle's chord, so rounding never hides a pair from the bucket that owes
    // it a test.
    let chord = 2.0 * (0.5 * ANGLE.to_radians()).sin() + 1e-4;
    // Each face is at home in its normal's bucket and a guest in the others its neighborhood
    // reaches.
    let mut buckets: HashMap<IVec3, Vec<(usize, bool)>> = HashMap::new();
    for (index, face) in faces.iter().enumerate() {
        let home = cell(face.normal);
        let (low, high) = (cell(face.normal - chord), cell(face.normal + chord));
        for x in low.x..=high.x {
            for y in low.y..=high.y {
                for z in low.z..=high.z {
                    let key = IVec3::new(x, y, z);
                    buckets.entry(key).or_default().push((index, key == home));
                }
            }
        }
    }
    let mut found = Vec::new();
    for (key, entries) in &buckets {
        if entries.len() > 1 && entries.iter().any(|&(_, home)| home) {
            bucket(faces, *key, entries, &mut found);
        }
    }
    // Buckets come in the map's order: sort, so the result doesn't depend on it.
    found.sort_unstable_by_key(|overlap| overlap.faces);
    found
}

/// The bucket a normal falls in.
fn cell(normal: Vec3) -> IVec3 {
    (normal / CELL).round().as_ivec3()
}

/// A face's extent in a bucket's frame: along its normal (`offset`) and its plane's axes.
struct Extent {
    face: usize,
    home: bool,
    offset: [f32; 2],
    u: [f32; 2],
    v: [f32; 2],
}

/// Tests the pairs a bucket is responsible for: those of two faces at home there, and those of
/// a face at home there and a guest when the one at home has the lower index (the pair also
/// meets in the guest's home, the other way round).
fn bucket(faces: &[Face], key: IVec3, entries: &[(usize, bool)], found: &mut Vec<Overlap>) {
    let normal = key.as_vec3().normalize();
    let u = normal.any_orthonormal_vector();
    let v = normal.cross(u);
    let range = |face: &Face, axis: Vec3| {
        let [a, b, c] = face.corners.map(|corner| corner.dot(axis));
        [a.min(b).min(c), a.max(b).max(c)]
    };
    let mut extents: Vec<Extent> = entries
        .iter()
        .map(|&(index, home)| {
            let face = &faces[index];
            Extent { face: index, home, offset: range(face, normal), u: range(face, u), v: range(face, v) }
        })
        .collect();
    // Slabs: runs of faces whose offsets overlap, within the gap.
    extents.sort_unstable_by(|a, b| a.offset[0].total_cmp(&b.offset[0]).then(a.face.cmp(&b.face)));
    let mut start = 0;
    while start < extents.len() {
        let mut end = start + 1;
        let mut reach = extents[start].offset[1];
        while end < extents.len() && extents[end].offset[0] <= reach + GAP {
            reach = reach.max(extents[end].offset[1]);
            end += 1;
        }
        // A slab of one placement's faces (a ground builtin's disc, say) holds no pair to test.
        let placement = faces[extents[start].face].placement;
        if extents[start + 1..end].iter().any(|extent| faces[extent.face].placement != placement) {
            sweep(faces, &mut extents[start..end], found);
        }
        start = end;
    }
}

/// Sweeps a slab along its bucket's U axis and tests the pairs whose extents meet.
fn sweep(faces: &[Face], slab: &mut [Extent], found: &mut Vec<Overlap>) {
    slab.sort_unstable_by(|a, b| a.u[0].total_cmp(&b.u[0]).then(a.face.cmp(&b.face)));
    for (index, first) in slab.iter().enumerate() {
        for second in &slab[index + 1..] {
            if second.u[0] > first.u[1] {
                break;
            }
            let apart = |a: [f32; 2], b: [f32; 2], margin: f32| a[0] > b[1] + margin || b[0] > a[1] + margin;
            if apart(first.v, second.v, 0.0) || apart(first.offset, second.offset, GAP) {
                continue;
            }
            let (low, high) = (first.face.min(second.face), first.face.max(second.face));
            let responsible = match (first.home, second.home) {
                (true, true) => true,
                (false, false) => false,
                (true, false) => first.face == low,
                (false, true) => second.face == low,
            };
            if !responsible || faces[low].placement == faces[high].placement {
                continue;
            }
            if let Some(at) = fight(&faces[low], &faces[high]) {
                found.push(Overlap { faces: [low, high], at });
            }
        }
    }
}

/// Where `a` and `b` fight, if they do: the middle of the region within both where their
/// normals are within [`ANGLE`] and their planes within [`GAP`], when it is wider than
/// [`WIDTH`] and, for faces in one slot, they shade differently somewhere in it.
pub fn fight(a: &Face, b: &Face) -> Option<Vec3> {
    let facing = a.normal.dot(b.normal);
    if facing < ANGLE.to_radians().cos() {
        return None;
    }
    // In `a`'s plane, `u × v` along its normal, so its corners wind counter-clockwise.
    let origin = a.corners[0];
    let u = a.normal.any_orthonormal_vector();
    let v = a.normal.cross(u);
    let flat = |p: Vec3| {
        let p = p - origin;
        Vec2::new(p.dot(u), p.dot(v))
    };
    let (a_flat, b_flat) = (a.corners.map(flat), b.corners.map(flat));
    // How far `b`'s plane is above `a`'s, along `a`'s normal: linear over the plane.
    let lift = |q: Vec2| b.normal.dot(b.corners[0] - origin - u * q.x - v * q.y) / facing;
    let mut region = Polygon::new(b_flat);
    for k in 0..3 {
        let (p, q) = (a_flat[k], a_flat[(k + 1) % 3]);
        region = region.clip(|x| (q - p).perp_dot(x - p));
    }
    region = region.clip(|x| GAP - lift(x));
    region = region.clip(|x| GAP + lift(x));
    if region.width() <= WIDTH {
        return None;
    }
    let points = &region.points[..region.len];
    // Attributes interpolate linearly over the region: alike at its corners, alike throughout.
    if a.slot == b.slot
        && points.iter().all(|&q| alike(shade(&a.shading, a_flat, q), shade(&b.shading, b_flat, q)))
    {
        return None;
    }
    let middle = points.iter().copied().sum::<Vec2>() / points.len() as f32;
    Some(origin + u * middle.x + v * middle.y + a.normal * (0.5 * lift(middle)))
}

/// The attributes at `q` of a face whose corners lie at `flat` in the plane.
fn shade(shading: &[Shading; 3], flat: [Vec2; 3], q: Vec2) -> Shading {
    let area = |p: Vec2, a: Vec2, b: Vec2| (a - p).perp_dot(b - p);
    let total = area(flat[0], flat[1], flat[2]);
    let weights = [
        area(q, flat[1], flat[2]) / total,
        area(flat[0], q, flat[2]) / total,
        area(flat[0], flat[1], q) / total,
    ];
    let blend =
        |pick: &dyn Fn(&Shading) -> Vec3| (0..3).map(|k| pick(&shading[k]) * weights[k]).sum::<Vec3>();
    Shading {
        normal: blend(&|corner| corner.normal).normalize_or_zero(),
        uv: blend(&|corner| corner.uv.extend(0.0)).truncate(),
        occlusion: blend(&|corner| Vec3::splat(corner.occlusion)).x,
    }
}

/// Whether two points shade alike (see [`OCCLUSION`]).
fn alike(a: Shading, b: Shading) -> bool {
    let uv = a.uv - b.uv;
    (a.occlusion - b.occlusion).abs() <= OCCLUSION
        && a.normal.dot(b.normal) >= NORMAL.to_radians().cos()
        && (uv - uv.round()).abs().max_element() <= TEXTURE
}

/// A convex polygon: a triangle clipped by at most five lines.
#[derive(Clone, Copy)]
struct Polygon {
    points: [Vec2; 8],
    len: usize,
}

impl Polygon {
    fn new(triangle: [Vec2; 3]) -> Self {
        let mut points = [Vec2::ZERO; 8];
        points[..3].copy_from_slice(&triangle);
        Self { points, len: 3 }
    }

    /// The part where `side`, linear, is at least zero.
    fn clip(&self, side: impl Fn(Vec2) -> f32) -> Self {
        let mut out = Self { points: [Vec2::ZERO; 8], len: 0 };
        for k in 0..self.len {
            let (p, q) = (self.points[k], self.points[(k + 1) % self.len]);
            let (at_p, at_q) = (side(p), side(q));
            // Each clip adds at most one corner: five never fill the eight.
            if at_p >= 0.0 {
                out.points[out.len] = p;
                out.len += 1;
            }
            if (at_p >= 0.0) != (at_q >= 0.0) {
                out.points[out.len] = p + (q - p) * (at_p / (at_p - at_q));
                out.len += 1;
            }
        }
        out
    }

    /// The narrowest width across, which a convex polygon has across one of its edges; zero for
    /// less than a triangle.
    fn width(&self) -> f32 {
        let points = &self.points[..self.len];
        let mut narrowest = 0.0f32;
        if self.len >= 3 {
            narrowest = f32::INFINITY;
            for k in 0..self.len {
                let edge = points[(k + 1) % self.len] - points[k];
                let length = edge.length();
                // Clipping leaves corners a hair apart: those edges have no direction.
                if length > 1e-6 {
                    let across =
                        points.iter().map(|&x| edge.perp_dot(x - points[k]).abs()).fold(0.0, f32::max);
                    narrowest = narrowest.min(across / length);
                }
            }
        }
        if narrowest.is_finite() { narrowest } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A triangle about `center` in the plane facing `normal`, `size` across, turned `spin`.
    fn face(center: Vec3, normal: Vec3, size: f32, spin: f32, slot: usize, placement: usize) -> Face {
        let u = normal.any_orthonormal_vector();
        let v = normal.cross(u);
        let corners = [0.0f32, 2.1, 4.2]
            .map(|angle| center + (u * (angle + spin).cos() + v * (angle + spin).sin()) * size);
        let shading = [Shading { normal, uv: Vec2::ZERO, occlusion: 1.0 }; 3];
        Face { corners, normal, slot, placement, shading }
    }

    /// The buckets find exactly the pairs that testing every pair finds: normals crowd a corner
    /// where buckets meet, planes lie a hair apart, and some faces tilt too far or face away.
    #[test]
    fn the_search_finds_what_testing_every_pair_finds() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32
        };
        // On the boundary between buckets along X and Z.
        let corner = Vec3::new(0.25, 0.0, 0.05);
        let corner = corner.with_y((1.0 - corner.length_squared()).sqrt());
        let u = corner.any_orthonormal_vector();
        let v = corner.cross(u);
        let mut faces = Vec::new();
        for index in 0..600 {
            let tilt = if index % 10 == 0 { 0.08 } else { 0.03 };
            let jitter = Vec3::new(random() - 0.5, random() - 0.5, random() - 0.5) * tilt;
            let mut normal = (corner + jitter).normalize();
            if index % 13 == 0 {
                normal = -normal;
            }
            let offset = (random() - 0.5) * 0.004;
            let center = corner * (5.0 + offset) + u * (random() * 3.0) + v * (random() * 3.0);
            let occlusion = if random() < 0.5 { 1.0 } else { 0.5 };
            let mut face = face(center, normal, 0.2 + random() * 0.8, random() * 6.0, index % 3, index % 7);
            face.shading.iter_mut().for_each(|corner| corner.occlusion = occlusion);
            faces.push(face);
        }
        let mut every_pair = Vec::new();
        for low in 0..faces.len() {
            for high in low + 1..faces.len() {
                if faces[low].placement != faces[high].placement
                    && let Some(at) = fight(&faces[low], &faces[high])
                {
                    every_pair.push(Overlap { faces: [low, high], at });
                }
            }
        }
        assert!(every_pair.len() > 100, "the case has fights to find: {}", every_pair.len());
        assert_eq!(overlaps(&faces), every_pair);
    }

    #[test]
    fn regions_are_as_wide_as_across_their_narrowest() {
        let square = Polygon::new([Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)])
            .clip(|x| 0.4 - x.y)
            .clip(|x| x.x - 0.5);
        assert!((square.width() - 0.4).abs() < 1e-6, "{}", square.width());
        let nothing = Polygon::new([Vec2::ZERO, Vec2::X, Vec2::Y]).clip(|x| x.x - 2.0);
        assert_eq!((nothing.len, nothing.width()), (0, 0.0));
    }
}
