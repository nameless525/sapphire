//! Pose-driven right underarm fold relief on the exact cooked garment and skin.
use super::{MorphPoint, Point};
use crate::{Bind, CharacterPose};
use sapphire_assets::{mesh::Mesh, skeleton::ArchivedSkeleton};
use sapphire_math::{Affine3A, Vec3};

const TARGET: &str = "__r10_right_underarm";
const FOLD: f32 = 0.0003;
const PATCH_CENTER: Vec3 = Vec3::new(-0.21, 1.19, 0.01);
const PATCH_REACH: f32 = 0.23;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Underarm {
    target: usize,
    authored: [usize; 3],
    chest: Bind,
    upper: Bind,
    lower: usize,
    shape: Vec<MorphPoint>,
    /// The garment's triangles the pairs name, by their shirt points.
    cloth_triangles: Vec<[usize; 3]>,
    /// The skin's triangles the pairs name, by their points in `skin`.
    skin_triangles: Vec<[usize; 3]>,
    /// Two garment triangles that mustn't cut through each other, by index into `cloth_triangles`.
    cloth: Vec<(u32, u32)>,
    /// A garment triangle and a skin triangle, by index into `cloth_triangles` and `skin_triangles`.
    arm: Vec<(u32, u32)>,
    active_cloth: Vec<usize>,
    active_arm: Vec<usize>,
    skin: Vec<Point>,
    posed_skin: Vec<Vec3>,
    candidate: Vec<Vec3>,
    offsets: Vec<Vec3>,
    /// A solve's bounds of each of `cloth_triangles` as the garment stands, and as it stands or at the
    /// pose's limit, and of each of `skin_triangles`: taken once a solve, where a pair would take them
    /// for every pair it's in, about 160,000 of them on the heroine.
    stock_bounds: Vec<(Vec3, Vec3)>,
    swept_bounds: Vec<(Vec3, Vec3)>,
    skin_bounds: Vec<(Vec3, Vec3)>,
    /// The weight the last solve gave the relief, which a frame between solves holds.
    weight: f32,
}

impl Underarm {
    pub(super) fn new(
        mesh: &Mesh,
        skeleton: &ArchivedSkeleton,
        binds: &[Bind],
        shirt_triangles: &[[usize; 3]],
        mapped: &[Option<usize>],
        point: impl Fn(usize) -> Option<(Vec3, Point)>,
        shirt_morphs: &[MorphPoint],
    ) -> Option<Self> {
        let target_named = |name| mesh.morph_targets.iter().position(|target| target.name == name);
        let target = target_named(TARGET)?;
        let authored = [
            target_named("__sleeve_p8_left")?,
            target_named("__sleeve_p8_right_onset")?,
            target_named("__sleeve_p8_right_crest")?,
        ];
        let joint = |name| skeleton.names.iter().position(|joint| joint.as_str() == name);
        let bind = |name| {
            let joint = joint(name)?;
            binds.iter().find(|bind| usize::from(bind.joint) == joint).copied()
        };
        let chest = bind("J_Bip_C_UpperChest")?;
        let upper = bind("J_Bip_R_UpperArm")?;
        let lower = joint("J_Bip_R_LowerArm")?;
        let shape: Vec<_> = shirt_morphs.iter().copied().filter(|morph| morph.target == target).collect();
        if shape.is_empty() || shape.len() > 64 {
            return None;
        }
        let shirt_count = shirt_triangles.iter().flatten().copied().max()? + 1;
        let mut changed = vec![false; shirt_count];
        for entry in &shape {
            changed[entry.shirt_point] = true;
        }
        let mut rest = vec![Vec3::ZERO; shirt_count];
        for (index, slot) in mapped.iter().copied().enumerate() {
            if let Some(slot) = slot {
                rest[slot] = point(index)?.0;
            }
        }
        let weld = |at: Vec3| (at * 100_000.0).round().as_ivec3();
        let near = |at: Vec3| at.distance_squared(PATCH_CENTER) < PATCH_REACH * PATCH_REACH;
        let cloth_welds: Vec<_> = rest.iter().copied().map(weld).collect();
        // Each shirt triangle a pair names once, in `cloth_triangles`, the first time it's named.
        let mut cloth_triangles = Vec::new();
        let mut cloth_slots: Vec<Option<u32>> = vec![None; shirt_triangles.len()];
        let mut slot = |triangle: usize| {
            *cloth_slots[triangle].get_or_insert_with(|| {
                cloth_triangles.push(shirt_triangles[triangle]);
                (cloth_triangles.len() - 1) as u32
            })
        };
        let patch: Vec<usize> =
            (0..shirt_triangles.len()).filter(|&i| shirt_triangles[i].iter().any(|&v| changed[v])).collect();
        let mut cloth = Vec::new();
        for (i, &a) in shirt_triangles.iter().enumerate() {
            let moves_a = a.iter().any(|&v| changed[v]);
            if !a.iter().any(|&v| near(rest[v])) {
                continue;
            }
            for (j, &b) in shirt_triangles.iter().enumerate().skip(i + 1) {
                if (moves_a || b.iter().any(|&v| changed[v]))
                    && b.iter().any(|&v| near(rest[v]))
                    && !a.iter().any(|&v| b.iter().any(|&w| cloth_welds[v] == cloth_welds[w]))
                {
                    cloth.push((slot(i), slot(j)));
                }
            }
        }
        let mut skin = Vec::new();
        let mut skin_welds = Vec::new();
        let mut skin_slots = vec![None; mesh.streams.len()];
        let mut skin_triangles = Vec::new();
        let mut arm = Vec::new();
        let lod = mesh.lods.first()?;
        for part in
            lod.submeshes.iter().filter(|part| mesh.materials[part.material as usize].name == "Body_00_SKIN")
        {
            let start = part.first_index as usize;
            let end = start + part.index_count as usize;
            for &face in mesh.indices[start..end].as_chunks::<3>().0 {
                let [a, b, c] = face.map(|id| point(id as usize));
                let [(pa, a), (pb, b), (pc, c)] = [a?, b?, c?];
                if ![pa, pb, pc].into_iter().any(|p| near(p) || (p.x < -0.13 && p.y > 1.03)) {
                    continue;
                }
                let tri = std::array::from_fn(|k| {
                    *skin_slots[face[k] as usize].get_or_insert_with(|| {
                        let i = skin.len();
                        skin.push([a, b, c][k]);
                        skin_welds.push(weld([pa, pb, pc][k]));
                        i
                    })
                });
                let skin_triangle = skin_triangles.len() as u32;
                let mut named = false;
                for &triangle in &patch {
                    let cloth = shirt_triangles[triangle];
                    if !cloth.iter().any(|&v| tri.iter().any(|&w| cloth_welds[v] == skin_welds[w])) {
                        arm.push((slot(triangle), skin_triangle));
                        named = true;
                    }
                }
                if named {
                    skin_triangles.push(tri);
                }
            }
        }
        Some(Self {
            target,
            authored,
            chest,
            upper,
            lower,
            posed_skin: vec![Vec3::ZERO; skin.len()],
            offsets: vec![Vec3::ZERO; shape.len()],
            candidate: vec![Vec3::ZERO; shirt_count],
            shape,
            active_cloth: Vec::with_capacity(cloth.len()),
            active_arm: Vec::with_capacity(arm.len()),
            stock_bounds: Vec::with_capacity(cloth_triangles.len()),
            swept_bounds: Vec::with_capacity(cloth_triangles.len()),
            skin_bounds: Vec::with_capacity(skin_triangles.len()),
            weight: 0.0,
            cloth_triangles,
            skin_triangles,
            cloth,
            arm,
            skin,
        })
    }

    pub(super) fn morph_slot(&self) -> (&'static str, usize) {
        (TARGET, self.target)
    }

    pub(super) fn clear(&self, pose: &mut CharacterPose) {
        pose.weights_mut()[self.target] = 0.0;
    }

    /// Gives the relief the weight the last solve did.
    pub(super) fn hold(&self, pose: &mut CharacterPose) {
        pose.weights_mut()[self.target] = self.weight;
    }

    fn pose_limit(&self, pose: &CharacterPose) -> f32 {
        let model = pose.model();
        let upper = model[usize::from(self.upper.joint)];
        let from_model = (model[usize::from(self.chest.joint)] * self.chest.inverse_bind).inverse();
        let arm = from_model.transform_vector3(Vec3::from(model[self.lower].translation - upper.translation));
        let Some(axis) = arm.try_normalize() else {
            return 0.0;
        };
        let front =
            from_model.transform_vector3((upper * self.upper.inverse_bind).transform_vector3(Vec3::Z));
        let project = |v: Vec3| (v - axis * v.dot(axis)).try_normalize();
        let (Some(reference), Some(front)) = (project(Vec3::Z), project(front)) else {
            return 0.0;
        };
        let roll = axis.dot(reference.cross(front)).atan2(reference.dot(front)).to_degrees();
        let authored = self.authored.into_iter().map(|i| pose.weights()[i]).fold(0.0, f32::max);
        pose_limit(axis, roll, authored)
    }

    /// The shader and hair pass share the same accepted weight and posed tee.
    pub(super) fn solve(
        &mut self,
        pose: &mut CharacterPose,
        chest_from_model: Affine3A,
        shirt: &[Point],
        stock: &mut [Vec3],
    ) {
        self.weight = 0.0;
        let limit = self.pose_limit(pose);
        if limit <= 0.0 {
            return;
        }
        let triangles = &self.cloth_triangles;
        self.stock_bounds.clear();
        self.stock_bounds.extend(triangles.iter().map(|tri| bounds(tri.map(|i| stock[i]))));
        // `cut` is 0 for triangles whose bounds don't meet, so a pair whose bounds don't is no fold.
        let stock_bounds = &self.stock_bounds;
        let folds = |&(a, b): &(u32, u32)| {
            let (a, b) = (a as usize, b as usize);
            overlap(stock_bounds[a], stock_bounds[b])
                && cut(triangles[a].map(|i| stock[i]), triangles[b].map(|i| stock[i])) >= FOLD
        };
        if !self.cloth.iter().any(folds) {
            return;
        }
        for (out, &point) in self.posed_skin.iter_mut().zip(&self.skin) {
            *out = chest_from_model.transform_point3(point.model(pose.model()));
        }
        for (out, morph) in self.offsets.iter_mut().zip(&self.shape) {
            let mut offset = Vec3::ZERO;
            for (part, delta) in shirt[morph.shirt_point].parts.iter().zip(morph.joint_offsets) {
                offset += pose.model()[part.joint].transform_vector3(delta) * part.weight;
            }
            *out = chest_from_model.transform_vector3(offset);
        }
        self.candidate.copy_from_slice(stock);
        for (morph, offset) in self.shape.iter().zip(&self.offsets) {
            self.candidate[morph.shirt_point] += *offset * limit;
        }
        let candidate = &self.candidate;
        self.swept_bounds.clear();
        self.swept_bounds.extend(self.cloth_triangles.iter().zip(&self.stock_bounds).map(|(tri, a)| {
            let b = bounds(tri.map(|i| candidate[i]));
            (a.0.min(b.0), a.1.max(b.1))
        }));
        let posed_skin = &self.posed_skin;
        self.skin_bounds.clear();
        self.skin_bounds.extend(self.skin_triangles.iter().map(|tri| bounds(tri.map(|i| posed_skin[i]))));
        let swept = &self.swept_bounds;
        let skin = &self.skin_bounds;
        self.active_cloth.clear();
        self.active_cloth.extend(
            self.cloth
                .iter()
                .enumerate()
                .filter_map(|(i, &(a, b))| overlap(swept[a as usize], swept[b as usize]).then_some(i)),
        );
        self.active_arm.clear();
        self.active_arm.extend(
            self.arm
                .iter()
                .enumerate()
                .filter_map(|(i, &(a, b))| overlap(swept[a as usize], skin[b as usize]).then_some(i)),
        );
        let mut chosen = 0.0;
        let mut previous = 0.0;
        for step in 1..=6 {
            let gain = limit * step as f32 / 6.0;
            if self.clears(stock, gain) {
                let mut lo = previous;
                let mut hi = gain;
                for _ in 0..3 {
                    let mid = (lo + hi) * 0.5;
                    if self.clears(stock, mid) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                chosen = hi;
                break;
            }
            previous = gain;
        }
        if chosen > 0.0 {
            for (morph, offset) in self.shape.iter().zip(&self.offsets) {
                stock[morph.shirt_point] += *offset * chosen;
            }
            pose.weights_mut()[self.target] = chosen;
            self.weight = chosen;
        }
    }

    fn clears(&mut self, stock: &[Vec3], gain: f32) -> bool {
        for (morph, offset) in self.shape.iter().zip(&self.offsets) {
            self.candidate[morph.shirt_point] = stock[morph.shirt_point] + *offset * gain;
        }
        let cloth = |triangle: u32| self.cloth_triangles[triangle as usize];
        let moved = |triangle: u32| cloth(triangle).map(|i| self.candidate[i]);
        if self.active_cloth.iter().any(|&i| {
            let (a, b) = self.cloth[i];
            cut(moved(a), moved(b)) >= FOLD
        }) {
            return false;
        }
        self.active_arm.iter().all(|&i| {
            let (a, b) = self.arm[i];
            let skin = self.skin_triangles[b as usize].map(|i| self.posed_skin[i]);
            let new = cut(moved(a), skin);
            new < FOLD || new <= cut(cloth(a).map(|i| stock[i]), skin)
        })
    }
}

fn pose_limit(axis: Vec3, roll: f32, authored: f32) -> f32 {
    let ramp = |a: f32, b: f32, x: f32| {
        let u = ((x - a) / (b - a)).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    };
    ramp(0.440, 0.475, -axis.x)
        * ramp(0.69, 0.60, -axis.x)
        * ramp(0.60, 0.79, -axis.y)
        * ramp(0.36, 0.22, axis.z.abs())
        * ramp(42.0, 26.0, roll.abs())
        * ramp(0.07, 0.0, authored)
}

fn bounds(t: [Vec3; 3]) -> (Vec3, Vec3) {
    (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]))
}

fn overlap(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
    a.0.cmple(b.1).all() && b.0.cmple(a.1).all()
}

/// Length of a transverse cut between two finite triangles.
fn cut(a: [Vec3; 3], b: [Vec3; 3]) -> f32 {
    if !overlap(bounds(a), bounds(b)) {
        return 0.0;
    }
    let mut first: Option<Vec3> = None;
    let mut length_squared = 0.0_f32;
    for (p, q) in [(a, b), (b, a)] {
        let Some(n) = (q[1] - q[0]).cross(q[2] - q[0]).try_normalize() else {
            return 0.0;
        };
        let dist = p.map(|x| (x - q[0]).dot(n));
        if dist.iter().all(|&d| d >= -0.000001) || dist.iter().all(|&d| d <= 0.000001) {
            return 0.0;
        }
        for i in 0..3 {
            let j = (i + 1) % 3;
            let (d, e) = (dist[i], dist[j]);
            if (d > 0.0) == (e > 0.0) {
                continue;
            }
            let hit = p[i].lerp(p[j], d / (d - e));
            let inside = (0..3).all(|k| {
                let start = q[k];
                let end = q[(k + 1) % 3];
                (end - start).cross(hit - start).dot(n) >= -1e-9
            });
            if inside {
                if let Some(first) = first {
                    length_squared = length_squared.max(first.distance_squared(hit));
                } else {
                    first = Some(hit);
                }
            }
        }
    }
    length_squared.sqrt()
}

#[cfg(test)]
mod tests {
    use super::{cut, pose_limit};
    use sapphire_math::Vec3;

    #[test]
    fn sleeve_fold_guard_detects_a_crossing_but_excludes_a_touch() {
        let a = [Vec3::ZERO, Vec3::X, Vec3::Y];
        let b = [Vec3::new(0.5, 0.0, -1.0), Vec3::new(0.5, 0.0, 1.0), Vec3::new(0.5, 0.5, 0.0)];
        assert!((cut(a, b) - 0.5).abs() < 1e-6);
        assert_eq!(cut(a, b.map(|point| point + Vec3::Z)), 0.0);
    }

    #[test]
    fn pose_window_tapers_closed_preparation_and_keeps_authored_sleeve_authority() {
        let preparation = Vec3::new(-0.448, -0.889, 0.097);
        let gathered = Vec3::new(-0.476, -0.875, 0.086);
        assert!(pose_limit(preparation, 8.22, 0.0) < 0.2);
        assert!(pose_limit(gathered, 6.46, 0.0) > 0.99);
        assert_eq!(pose_limit(gathered, 6.46, 0.5), 0.0);
        assert_eq!(pose_limit(gathered, 50.0, 0.0), 0.0);
    }
}
