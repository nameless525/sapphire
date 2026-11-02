//! Bounding volumes for the CPU's per-object work: per-object light lists (M1 R5), culling
//! shadow casters per cascade (M1 R4) and outline hulls past their cutoff (M1 C4). Each mesh's
//! object-space sphere comes from its cooked bounds and lives in its record in the mesh pool
//! (`mesh_pool::MeshRecord::bounds`).
//!
//! # Poses that leave them (M2 N4)
//!
//! A skinned mesh's cooked bounds are its bind pose's. An arm overhead or a body lying on the ground
//! leaves them, so the character's shadow map would cut it off. Each palette entry's vertices fill a
//! box in the bind pose ([`palette_reach`], once per upload), and a pose's bounds ([`posed_bounds`])
//! come from where the palette puts those boxes' corners: a skinned vertex is a weighted mean of its
//! images under its entries, each inside that entry's moved box, so a sphere holding the corners
//! holds it. That reads the palette, whatever made it (a clip, a graph, IK, springs), and needs
//! nothing of the app.
//!
//! Every box lies inside the box the mesh was cooked with, so at rest every corner is inside its
//! sphere. While they stay inside the answer is `None`, and every decision made on the bounds is the
//! bind pose's, bit for bit. A pose past it grows the sphere only by the corners that leave, so it
//! grows from the cooked sphere with no jump. Morph deltas aren't counted, as the cooked bounds don't
//! count them.

use glam::{Affine3A, Mat4, Vec3, Vec3A};
use sapphire_assets::mesh::{ArchivedMesh, VertexInfluence};

/// A bounding sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

impl Sphere {
    /// The sphere around an object-space box (`[min, max]`, as meshes are cooked with).
    pub(crate) fn around([min, max]: [[f32; 3]; 2]) -> Self {
        let (min, max) = (Vec3::from(min), Vec3::from(max));
        Self { center: (min + max) * 0.5, radius: (max - min).length() * 0.5 }
    }

    /// This sphere after `transform`, grown by its largest stretch ([`max_stretch`]) so it still
    /// holds the mesh.
    pub(crate) fn transformed(self, transform: Mat4) -> Self {
        Self { center: transform.transform_point3(self.center), radius: self.radius * max_stretch(transform) }
    }

    /// The smallest sphere holding both this one and `other`.
    pub(crate) fn union(self, other: Self) -> Self {
        let offset = other.center - self.center;
        let distance = offset.length();
        if distance + other.radius <= self.radius {
            return self;
        }
        if distance + self.radius <= other.radius {
            return other;
        }
        let radius = (distance + self.radius + other.radius) * 0.5;
        let center = self.center + offset * ((radius - self.radius) / distance);
        Self { center, radius }
    }
}

/// What one skinning-palette entry's vertices fill in the bind pose (the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaletteReach {
    pub entry: u32,
    pub center: Vec3A,
    /// Half the box's size, per axis.
    pub half: Vec3A,
}

/// The boxes of `mesh`'s palette entries that move any vertex, in palette order: none without a
/// skin. Reads every vertex, so an upload calls it, not a frame.
pub(crate) fn palette_reach(mesh: &ArchivedMesh) -> Vec<PaletteReach> {
    let Some(skin) = mesh.skin.as_ref() else { return Vec::new() };
    let streams = &mesh.streams;
    boxes(skin.binds.len(), streams.len(), |vertex| {
        Some((Vec3::from(streams.position(vertex)?), skin.influences.get(vertex)?))
    })
}

/// [`palette_reach`] over `vertices` vertices, each one's position and influences from `vertex`.
fn boxes(
    entries: usize,
    vertices: usize,
    vertex: impl Fn(usize) -> Option<(Vec3, VertexInfluence)>,
) -> Vec<PaletteReach> {
    let mut boxes = vec![(Vec3::INFINITY, Vec3::NEG_INFINITY); entries];
    for (position, influence) in (0..vertices).filter_map(vertex) {
        for (&entry, &weight) in influence.binds.iter().zip(&influence.weights) {
            if let Some((min, max)) = boxes.get_mut(usize::from(entry)).filter(|_| weight > 0) {
                (*min, *max) = (min.min(position), max.max(position));
            }
        }
    }
    let used = boxes.iter().enumerate().filter(|(_, (min, max))| min.x <= max.x);
    used.map(|(entry, &(min, max))| PaletteReach {
        entry: entry as u32,
        center: Vec3A::from((min + max) * 0.5),
        half: Vec3A::from((max - min) * 0.5),
    })
    .collect()
}

/// How far past a sphere a box's corner may lie and count as inside, as a share of the radius: a
/// tenth of a millimetre a metre, so that float rounding in a palette that is the identity to within
/// 1e-6, at a corner that touches the sphere, can't move a decision.
const INSIDE: f32 = 1.0 + 1e-4;

/// The object-space bounds of a skinned mesh under `palette`, when they aren't `cooked`, the bind
/// pose's: `None` while `reach` stays inside (the module docs). Allocates nothing.
pub(crate) fn posed_bounds(cooked: Sphere, reach: &[PaletteReach], palette: &[Affine3A]) -> Option<Sphere> {
    let mut grown = None;
    for &PaletteReach { entry, center, half } in reach {
        let Some(matrix) = palette.get(entry as usize) else { continue };
        let center = matrix.transform_point3a(center);
        let (x, y, z) =
            (matrix.matrix3.x_axis * half.x, matrix.matrix3.y_axis * half.y, matrix.matrix3.z_axis * half.z);
        // Nearly every box of nearly every pose is well inside, which this much says.
        let held = grown.unwrap_or(cooked);
        let (middle, limit) = (Vec3A::from(held.center), held.radius * INSIDE);
        if center.distance(middle) + (x.abs() + y.abs() + z.abs()).length() <= limit {
            continue;
        }
        for corner in 0..8 {
            let side = |axis: Vec3A, bit: u32| if corner & bit == 0 { -axis } else { axis };
            let corner = center + side(x, 1) + side(y, 2) + side(z, 4);
            let held = grown.unwrap_or(cooked);
            if corner.distance(Vec3A::from(held.center)) > held.radius * INSIDE {
                grown = Some(held.union(Sphere { center: corner.into(), radius: 0.0 }));
            }
        }
    }
    grown
}

/// How far past the longest column's squared length the bound below may run and still be
/// rounding: float rounding leaves the columns of rotations, and of chains of them, up to about
/// 1e-6 off right angles, which puts the bound up to 2e-6 past it. Taking the longest column
/// there falls short of the stretch by at most half this share: 10 µm a meter of radius.
const ROUNDING: f32 = 2e-5;

/// How far `transform` stretches any direction (its largest singular value), or more, never
/// less: what a radius grows by so a sphere or capsule still holds what it held.
///
/// The bound is the square root of the Gershgorin bound on the largest eigenvalue of `MᵀM`: the
/// largest row sum of the columns' absolute dot products. Where the columns are at right angles,
/// as for rotation and scale (the transforms apps send), it is the longest column's length, and
/// that is kept bit for bit as it was before shear was handled, so no culling, cascade, outline
/// or light decision moves. Under shear (a rotated child under a non-uniformly scaled parent,
/// say) the longest column falls short (review RA-003) and the bound doesn't.
pub(crate) fn max_stretch(transform: Mat4) -> f32 {
    let (x, y, z) = (transform.x_axis.truncate(), transform.y_axis.truncate(), transform.z_axis.truncate());
    let (xx, yy, zz) = (x.length_squared(), y.length_squared(), z.length_squared());
    let (xy, xz, yz) = (x.dot(y).abs(), x.dot(z).abs(), y.dot(z).abs());
    let longest = xx.max(yy).max(zz);
    let bound = (xx + xy + xz).max(yy + xy + yz).max(zz + xz + yz);
    // A comparison with NaN is false: a transform that isn't finite keeps the old value too.
    let squared = if bound > longest * (1.0 + ROUNDING) { bound } else { longest };
    // A square root is correctly rounded and never decreases, so the root of the longest
    // squared length is the longest of the lengths, bit for bit.
    squared.sqrt()
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    use glam::Vec4;
    use sapphire_assets::mesh::quantize_weights;
    use sapphire_assets::rig::HumanBone;

    use super::*;
    use crate::CharacterBody;

    #[test]
    fn spheres_follow_their_transforms() {
        let sphere = Sphere::around([[-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]]);
        assert_eq!(sphere.center, Vec3::Y);
        assert!((sphere.radius - 3f32.sqrt()).abs() < 1e-6);
        let moved = sphere.transformed(Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 3.0, 2.0),
            glam::Quat::IDENTITY,
            Vec3::X,
        ));
        assert_eq!(moved.center, Vec3::new(1.0, 3.0, 0.0));
        assert!((moved.radius - 3.0 * 3f32.sqrt()).abs() < 1e-5, "grown by the largest scale");
    }

    /// A seeded generator for the transforms below: xorshift32, uniform in `[lo, hi)`.
    struct Random(u32);

    impl Random {
        fn next(&mut self, lo: f32, hi: f32) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            lo + (hi - lo) * (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }

        fn vec3(&mut self, lo: f32, hi: f32) -> Vec3 {
            Vec3::new(self.next(lo, hi), self.next(lo, hi), self.next(lo, hi))
        }

        fn rotation(&mut self) -> glam::Quat {
            glam::Quat::from_xyzw(
                self.next(-1.0, 1.0),
                self.next(-1.0, 1.0),
                self.next(-1.0, 1.0),
                self.next(-1.0, 1.0),
            )
            .normalize()
        }

        /// A positive scale per axis, from a hundredth to a hundred.
        fn scale(&mut self) -> Vec3 {
            Vec3::new(
                10f32.powf(self.next(-2.0, 2.0)),
                10f32.powf(self.next(-2.0, 2.0)),
                10f32.powf(self.next(-2.0, 2.0)),
            )
        }
    }

    /// The box's corners under `transform` all lie in the sphere around the box, transformed.
    fn assert_holds_its_box(bounds: [[f32; 3]; 2], transform: Mat4) {
        let moved = Sphere::around(bounds).transformed(transform);
        let [min, max] = bounds;
        for corner in 0..8 {
            let pick = |axis: usize| if corner & (1 << axis) == 0 { min[axis] } else { max[axis] };
            let point = transform.transform_point3(Vec3::new(pick(0), pick(1), pick(2)));
            // Float rounding in the center and the radius, relative to the transform's size.
            let slack = 1e-5 * (moved.radius + transform.w_axis.truncate().length());
            let distance = point.distance(moved.center);
            assert!(distance <= moved.radius + slack, "{distance} > {} under {transform:?}", moved.radius);
        }
    }

    #[test]
    fn spheres_hold_their_mesh_under_shear() {
        // x' = x + y: the box's corner (1, 1, 0) lands √5 from the center, past the longest
        // column (√2) × the box's half-diagonal (√2).
        let shear = Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, //
            1.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ]);
        assert_holds_its_box([[-1.0, -1.0, 0.0], [1.0, 1.0, 0.0]], shear);
        // A rotated child under a non-uniformly scaled parent: its largest stretch (3, which the
        // bound finds exactly here) exceeds every column's length (√5).
        let squashed = Mat4::from_scale(Vec3::new(1.0, 3.0, 1.0)) * Mat4::from_rotation_z(FRAC_PI_4);
        assert_holds_its_box([[-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]], squashed);
        assert!((max_stretch(squashed) - 3.0).abs() < 1e-5, "{}", max_stretch(squashed));
        // Random affine transforms, and random chains of parents and children.
        let mut random = Random(0x5eed);
        for _ in 0..2000 {
            let bounds = [random.vec3(-2.0, 0.0).to_array(), random.vec3(0.0, 2.0).to_array()];
            let affine = Mat4::from_cols(
                random.vec3(-3.0, 3.0).extend(0.0),
                random.vec3(-3.0, 3.0).extend(0.0),
                random.vec3(-3.0, 3.0).extend(0.0),
                random.vec3(-50.0, 50.0).extend(1.0),
            );
            assert_holds_its_box(bounds, affine);
            let mut chain = Mat4::IDENTITY;
            for _ in 0..3 {
                chain *= Mat4::from_scale_rotation_translation(
                    random.scale(),
                    random.rotation(),
                    random.vec3(-5.0, 5.0),
                );
            }
            assert_holds_its_box(bounds, chain);
        }
    }

    #[test]
    fn rotation_and_scale_keep_the_longest_column_exactly() {
        // The radius before shear was handled: culling, cascade, outline and light decisions
        // made on it must not move by a bit for the transforms apps send.
        let longest_column = |transform: Mat4| {
            transform
                .x_axis
                .truncate()
                .length()
                .max(transform.y_axis.truncate().length())
                .max(transform.z_axis.truncate().length())
        };
        let sphere = Sphere { center: Vec3::new(0.25, 1.0, -0.5), radius: 1.7 };
        let mut random = Random(0xb0b);
        let exact = |transform: Mat4| {
            let moved = sphere.transformed(transform);
            assert_eq!(
                moved.radius.to_bits(),
                (sphere.radius * longest_column(transform)).to_bits(),
                "{transform:?}"
            );
            assert_eq!(moved.center, transform.transform_point3(sphere.center));
        };
        for _ in 0..20_000 {
            let (rotation, translation) = (random.rotation(), random.vec3(-1000.0, 1000.0));
            exact(Mat4::from_scale_rotation_translation(random.scale(), rotation, translation));
            exact(Mat4::from_scale_rotation_translation(
                Vec3::splat(random.next(0.01, 100.0)),
                rotation,
                translation,
            ));
            // Uniformly scaled parents and children compose to rotation and scale.
            let parent = Mat4::from_scale_rotation_translation(
                Vec3::splat(random.next(0.1, 10.0)),
                random.rotation(),
                random.vec3(-100.0, 100.0),
            );
            exact(parent * Mat4::from_scale_rotation_translation(random.scale(), rotation, translation));
        }
        // Mirrored (unsupported for drawing, but its bounds hold), flattened and collapsed.
        exact(Mat4::from_scale(Vec3::new(-1.0, 2.0, 1.0)));
        exact(Mat4::from_scale(Vec3::new(1.0, 0.0, 3.0)));
        exact(Mat4::from_scale(Vec3::ZERO));
        assert_eq!(sphere.transformed(Mat4::from_scale(Vec3::ZERO)).radius, 0.0);
        // Transforms that aren't finite keep the old radius too, whatever it was.
        for broken in [
            Mat4::from_scale(Vec3::new(f32::NAN, 1.0, 2.0)),
            Mat4::from_scale(Vec3::new(f32::INFINITY, 1.0, 2.0)),
            Mat4::from_cols(Vec4::splat(f32::NAN), Vec4::Y, Vec4::Z, Vec4::W),
            Mat4::from_cols(Vec4::X, Vec4::new(1.0, f32::INFINITY, 0.0, 0.0), Vec4::Z, Vec4::W),
        ] {
            let radius = sphere.transformed(broken).radius;
            assert_eq!(radius.to_bits(), (sphere.radius * longest_column(broken)).to_bits(), "{broken:?}");
        }
    }

    #[test]
    fn character_bodies_hold_their_capsule_under_shear() {
        let body = CharacterBody::capsule(Vec3::new(0.0, 0.3, 0.0), Vec3::new(0.0, 1.6, 0.0), 0.3);
        // A body's capsule: every point within its radius of the segment from base to top.
        let gap = |body: &CharacterBody, point: Vec3| {
            let axis = body.top - body.base;
            let along = ((point - body.base).dot(axis) / axis.length_squared()).clamp(0.0, 1.0);
            point.distance(body.base + axis * along)
        };
        let squashed = Mat4::from_scale(Vec3::new(1.0, 3.0, 1.0)) * Mat4::from_rotation_z(FRAC_PI_4);
        let sheared = Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, //
            0.8, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            2.0, 0.0, -1.0, 1.0,
        ]);
        for transform in [squashed, sheared] {
            let moved = body.transformed(transform);
            // Points on the capsule's surface: around its two ends, and along its side.
            for i in 0..4000 {
                let y = 1.0 - 2.0 * (i as f32 + 0.5) / 4000.0;
                let ring = (1.0 - y * y).sqrt();
                let angle = i as f32 * 2.399_963;
                let direction = Vec3::new(ring * angle.cos(), y, ring * angle.sin());
                let along = body.base.lerp(body.top, (i % 7) as f32 / 6.0);
                let point = transform.transform_point3(along + direction * body.radius);
                assert!(gap(&moved, point) <= moved.radius * (1.0 + 1e-5), "{point} outside {moved:?}");
            }
        }
        // Rotation and scale keep the longest column exactly, as before: Y's, 2.5 long.
        let trs = Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 2.5, 0.5),
            glam::Quat::from_rotation_y(0.7),
            Vec3::X,
        );
        assert_eq!(body.transformed(trs).radius.to_bits(), (body.radius * 2.5).to_bits());
    }

    /// A skinned mesh in test: its positions, each vertex's influences and its cooked sphere.
    struct Skinned {
        positions: Vec<Vec3>,
        influences: Vec<VertexInfluence>,
        cooked: Sphere,
        reach: Vec<PaletteReach>,
    }

    impl Skinned {
        fn new(positions: Vec<Vec3>, influences: Vec<VertexInfluence>, entries: usize) -> Self {
            let reach =
                boxes(entries, positions.len(), |vertex| Some((positions[vertex], influences[vertex])));
            let min = positions.iter().fold(Vec3::INFINITY, |min, &position| min.min(position));
            let max = positions.iter().fold(Vec3::NEG_INFINITY, |max, &position| max.max(position));
            let cooked = Sphere::around([min.to_array(), max.to_array()]);
            Self { positions, influences, cooked, reach }
        }

        /// Vertex `index` under `palette`, as the skinning shader blends it.
        fn skinned(&self, index: usize, palette: &[Affine3A]) -> Vec3 {
            let influence = self.influences[index];
            let mut blended = Affine3A::ZERO;
            for (&entry, weight) in influence.binds.iter().zip(influence.weights) {
                let weight = f32::from(weight) / 255.0;
                blended.matrix3 += palette[usize::from(entry)].matrix3 * weight;
                blended.translation += palette[usize::from(entry)].translation * weight;
            }
            blended.transform_point3(self.positions[index])
        }

        /// How far the farthest vertex lies from `sphere`'s center, as a share of its radius.
        fn furthest_share(&self, sphere: Sphere, palette: &[Affine3A]) -> f32 {
            (0..self.positions.len())
                .map(|index| self.skinned(index, palette).distance(sphere.center) / sphere.radius)
                .fold(0.0, f32::max)
        }
    }

    /// A figure lying along +Y from the origin, 1.7 m tall and 0.5 m wide: `entries` segments up its
    /// height, each vertex blending the two nearest, as a body's limbs do.
    fn body(entries: usize) -> Skinned {
        let (mut positions, mut influences) = (Vec::new(), Vec::new());
        for step in 0..=200 {
            let along = step as f32 / 200.0;
            let at = along * (entries - 1) as f32;
            let lower = (at as usize).min(entries - 2);
            let upper = quantize_weights([1.0 - (at - lower as f32), at - lower as f32, 0.0, 0.0]);
            for side in [-0.25, 0.25] {
                positions.push(Vec3::new(side, along * 1.7, side * 0.5));
                influences
                    .push(VertexInfluence { binds: [lower as u16, lower as u16 + 1, 0, 0], weights: upper });
            }
        }
        Skinned::new(positions, influences, entries)
    }

    /// The whole figure turned `angle` about the X axis through the origin: falling.
    fn fallen(entries: usize, angle: f32) -> Vec<Affine3A> {
        vec![Affine3A::from_rotation_x(angle); entries]
    }

    #[test]
    fn each_entry_gets_the_box_of_its_own_vertices() {
        let body = body(6);
        assert_eq!(body.reach.len(), 6);
        for &PaletteReach { entry, center, half } in &body.reach {
            let mine = (0..body.positions.len()).filter(|&index| {
                let influence = body.influences[index];
                influence
                    .binds
                    .iter()
                    .zip(influence.weights)
                    .any(|(&bind, weight)| u32::from(bind) == entry && weight > 0)
            });
            let mut count = 0;
            for index in mine {
                count += 1;
                let apart = (Vec3A::from(body.positions[index]) - center).abs();
                assert!(apart.cmple(half + 1e-5).all(), "entry {entry}");
            }
            assert!(count > 0, "entry {entry} moves vertices");
            assert!(half.y < 0.5, "entry {entry}: a limb's box isn't the body's: {half}");
        }
        // An entry that moves nothing has no box, however far the palette sends it.
        let idle = Skinned::new(
            vec![Vec3::ZERO, Vec3::Y],
            vec![VertexInfluence { binds: [1, 0, 0, 0], weights: [255, 0, 0, 0] }; 2],
            3,
        );
        assert_eq!(idle.reach.iter().map(|reach| reach.entry).collect::<Vec<_>>(), [1]);
        let away = Affine3A::from_translation(Vec3::splat(100.0));
        assert_eq!(posed_bounds(idle.cooked, &idle.reach, &[away, Affine3A::IDENTITY, away]), None);
    }

    /// At rest, and for whatever stays in, nothing moves: the renderer goes on reading the cooked
    /// sphere itself, so every decision is the bind pose's, bit for bit.
    #[test]
    fn a_pose_that_stays_inside_keeps_the_cooked_bounds() {
        let body = body(4);
        let rest = vec![Affine3A::IDENTITY; 4];
        assert_eq!(posed_bounds(body.cooked, &body.reach, &rest), None, "at rest");
        // The palette a chain of rotations leaves: the identity, to within rounding.
        let noisy = Affine3A::from_rotation_y(0.3) * Affine3A::from_rotation_y(-0.3);
        assert_eq!(posed_bounds(body.cooked, &body.reach, &[noisy; 4]), None, "to within rounding");
        // The top segment bends in toward the middle.
        let mut bending = rest.clone();
        let knee = Vec3::new(0.0, 1.2, 0.0);
        bending[3] = Affine3A::from_translation(knee)
            * Affine3A::from_rotation_x(0.8)
            * Affine3A::from_translation(-knee);
        assert_eq!(posed_bounds(body.cooked, &body.reach, &bending), None, "bent inward");
    }

    #[test]
    fn a_body_lying_down_is_held_and_it_grows_from_the_cooked_bounds_with_no_jump() {
        let body = body(6);
        let mut before = body.cooked;
        let mut grew = false;
        // Fifteen degrees a second at 60 Hz, until it lies on its back.
        for step in 1..=360 {
            let angle = step as f32 / 360.0 * FRAC_PI_2;
            let palette = fallen(6, angle);
            let posed = posed_bounds(body.cooked, &body.reach, &palette);
            let sphere = posed.unwrap_or(body.cooked);
            assert!(body.furthest_share(sphere, &palette) <= 1.000_1, "at {angle} rad: {sphere:?}");
            assert!(
                (sphere.radius - before.radius).abs() < 0.01 && sphere.center.distance(before.center) < 0.02,
                "at {angle} rad it jumped from {before:?} to {sphere:?}"
            );
            before = sphere;
            grew |= posed.is_some();
        }
        assert!(grew, "lying down leaves the sphere");
        // The cooked sphere alone cuts it off, which is what this is for.
        let palette = fallen(6, FRAC_PI_2);
        assert!(body.furthest_share(body.cooked, &palette) > 1.1);
        assert!(before.radius > body.cooked.radius);
    }

    /// Random palettes, rigid and scaled, over random vertices and weights: every vertex stays in.
    #[test]
    fn a_poses_bounds_hold_every_vertex_however_it_is_posed() {
        let mut random = Random(0xd1ce);
        for _ in 0..200 {
            let entries = 2 + (random.next(0.0, 8.0) as usize);
            let count = 4 + (random.next(0.0, 60.0) as usize);
            let positions: Vec<Vec3> = (0..count).map(|_| random.vec3(-1.0, 1.0)).collect();
            let influences: Vec<VertexInfluence> = (0..count)
                .map(|_| {
                    let weights = quantize_weights([
                        random.next(0.0, 1.0),
                        random.next(0.0, 1.0),
                        random.next(0.0, 1.0),
                        random.next(-1.0, 1.0).max(0.0),
                    ]);
                    let binds = std::array::from_fn(|_| random.next(0.0, entries as f32) as u16);
                    VertexInfluence { binds, weights }
                })
                .collect();
            let skinned = Skinned::new(positions, influences, entries);
            let scaled = random.next(0.0, 1.0) > 0.5;
            let palette: Vec<Affine3A> = (0..entries)
                .map(|_| {
                    let scale = if scaled { Vec3::splat(1.0) + random.vec3(0.0, 1.0) } else { Vec3::ONE };
                    Affine3A::from_scale_rotation_translation(
                        scale,
                        random.rotation(),
                        random.vec3(-3.0, 3.0),
                    )
                })
                .collect();
            let sphere = posed_bounds(skinned.cooked, &skinned.reach, &palette).unwrap_or(skinned.cooked);
            let share = skinned.furthest_share(sphere, &palette);
            assert!(share <= INSIDE * 1.000_01, "{share} of {sphere:?} under {palette:?}");
        }
    }

    /// The character itself, as its VRM imports: arm work inside its sphere moves nothing, and what
    /// leaves it is held.
    #[test]
    fn the_character_keeps_its_cooked_bounds_until_it_leaves_them() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../content/third_party/vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm"
        );
        let bytes = std::fs::read(path).expect("the character");
        let rig = sapphire_pipeline::import::vrm::import_rig(&bytes, None).expect("its rig");
        let mesh = sapphire_pipeline::import::vrm::import(&bytes, None).expect("the character").mesh;
        let skin = mesh.skin.as_ref().expect("skinned");
        let positions = mesh.vertices.iter().map(|vertex| Vec3::from(vertex.position)).collect();
        let influences = skin
            .weights
            .iter()
            .map(|weights| VertexInfluence {
                binds: weights.binds,
                weights: quantize_weights(weights.weights),
            })
            .collect();
        let character = Skinned::new(positions, influences, skin.binds.len());

        let rest: Vec<Affine3A> =
            skin.joints.iter().enumerate().fold(Vec::new(), |mut model, (index, joint)| {
                let local = Affine3A::from_scale_rotation_translation(
                    joint.rest.scale.into(),
                    glam::Quat::from_array(joint.rest.rotation),
                    joint.rest.translation.into(),
                );
                let parent = joint.parent.map_or(Affine3A::IDENTITY, |parent| model[usize::from(parent)]);
                model.push(parent * local);
                debug_assert_eq!(model.len(), index + 1);
                model
            });
        // The palette with `turns`' joints and whatever hangs under them turned about where each stands.
        let palette = |turns: &[(HumanBone, glam::Quat)]| -> Vec<Affine3A> {
            let mut model = rest.clone();
            for &(bone, turn) in turns {
                let pivot = usize::from(rig.human_joint(bone).expect("the character has it"));
                let at = Vec3::from(rest[pivot].translation);
                let about = Affine3A::from_translation(at)
                    * Affine3A::from_quat(turn)
                    * Affine3A::from_translation(-at);
                for (index, joint) in skin.joints.iter().enumerate() {
                    let mut above = Some(index);
                    while let Some(joint_above) = above.filter(|&above| above != pivot) {
                        above = skin.joints[joint_above].parent.map(usize::from);
                    }
                    let _ = joint;
                    if above == Some(pivot) {
                        model[index] = about * model[index];
                    }
                }
            }
            let inverse = |bind: &sapphire_assets::mesh::SkinBind| {
                Affine3A::from_mat4(Mat4::from_cols_array_2d(&bind.inverse_bind_matrix))
            };
            skin.binds.iter().map(|bind| model[usize::from(bind.joint)] * inverse(bind)).collect()
        };
        let posed = |palette: &[Affine3A]| posed_bounds(character.cooked, &character.reach, palette);

        // A turn taking an arm, shoulder to hand, to point along `to`.
        let arm = |upper: HumanBone, hand: HumanBone, to: Vec3| {
            let at =
                |bone| Vec3::from(rest[usize::from(rig.human_joint(bone).expect("it has it"))].translation);
            (upper, glam::Quat::from_rotation_arc((at(hand) - at(upper)).normalize(), to))
        };
        let right = |to| arm(HumanBone::RightUpperArm, HumanBone::RightHand, to);
        let left = |to| arm(HumanBone::LeftUpperArm, HumanBone::LeftHand, to);

        assert_eq!(posed(&palette(&[])), None, "at rest");
        let down = Vec3::new(0.0, -1.0, 0.1).normalize();
        assert_eq!(posed(&palette(&[right(down), left(down)])), None, "arms at the sides");
        assert_eq!(posed(&palette(&[right(Vec3::Z)])), None, "an arm out in front");
        assert_eq!(posed(&palette(&[left(Vec3::NEG_Z)])), None, "an arm out behind");

        // Falling over backward from the feet, to lying on the ground.
        let lying: Vec<Affine3A> =
            palette(&[]).iter().map(|entry| Affine3A::from_rotation_x(-FRAC_PI_2) * *entry).collect();
        let sphere = posed(&lying).expect("a body lying down leaves its sphere");
        assert!(character.furthest_share(character.cooked, &lying) > 1.0);
        assert!(character.furthest_share(sphere, &lying) <= INSIDE * 1.000_01);
        // Not much bigger than it must be.
        let furthest = character.furthest_share(sphere, &lying) * sphere.radius;
        assert!(sphere.radius < furthest * 1.15, "{sphere:?} for a body reaching {furthest}");
        // An arm straight up only just stays in; off the ground, it leaves.
        assert_eq!(posed(&palette(&[right(Vec3::Y)])), None, "an arm overhead");
        let overhead: Vec<Affine3A> = palette(&[right(Vec3::Y)])
            .iter()
            .map(|entry| Affine3A::from_translation(Vec3::new(0.0, 0.3, 0.0)) * *entry)
            .collect();
        let sphere = posed(&overhead).expect("an arm overhead in a jump leaves its sphere");
        assert!(character.furthest_share(character.cooked, &overhead) > 1.0);
        assert!(character.furthest_share(sphere, &overhead) <= INSIDE * 1.000_01);
    }

    #[test]
    fn unions_hold_both_spheres_and_no_more() {
        let a = Sphere { center: Vec3::ZERO, radius: 1.0 };
        let b = Sphere { center: Vec3::new(4.0, 0.0, 0.0), radius: 1.0 };
        let both = a.union(b);
        assert_eq!(both, Sphere { center: Vec3::new(2.0, 0.0, 0.0), radius: 3.0 });
        let inside = Sphere { center: Vec3::new(0.5, 0.0, 0.0), radius: 0.2 };
        assert_eq!(a.union(inside), a, "already held");
        assert_eq!(inside.union(a), a, "holds it");
        assert_eq!(a.union(a), a);
    }
}
