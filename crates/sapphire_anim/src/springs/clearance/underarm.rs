//! Pose-driven right underarm fold relief on the exact cooked garment and skin.
use super::{MorphPoint, Point};
use crate::{Bind, CharacterPose};
use sapphire_assets::{mesh::Mesh, skeleton::ArchivedSkeleton};
use sapphire_math::{Affine3A, Vec3};

const TARGET: &str = "__r10_right_underarm";
const FOLD: f32 = 0.0003;
const PATCH_CENTER: Vec3 = Vec3::new(-0.21, 1.19, 0.01);
const PATCH_REACH: f32 = 0.23;
/// The half-life, in presented ticks, of the weight a solve gave up: a fold no gain clears, or one
/// that's gone, lets the relief go over a few frames instead of at once.
const RELEASE_HALF_LIFE: f32 = 4.0;

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
    /// The garment triangles the relief moves, which every pair in `cloth` and `arm` has one of, and
    /// per garment triangle its place among them if it's one.
    moving: Vec<u32>,
    moving_rank: Vec<Option<u32>>,
    /// Per moving triangle, a row of bits over `cloth_triangles`: those it comes first in a pair of
    /// `cloth` with, and those it comes second with; and one over `skin_triangles`, those it's paired
    /// with in `arm`.
    cloth_first: Bits,
    cloth_second: Bits,
    arm_partners: Bits,
    /// Trees over `cloth_triangles` and `skin_triangles`, grouped as they lie at rest, which find the
    /// triangles whose bounds meet a moving triangle's.
    cloth_tree: BoxTree,
    skin_tree: BoxTree,
    /// The pairs of `cloth` and `arm` whose bounds meet as the garment sweeps to the pose's limit.
    active_cloth: Vec<(u32, u32)>,
    active_arm: Vec<(u32, u32)>,
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
        let mut skin_rest = Vec::new();
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
                        skin_rest.push([pa, pb, pc][k]);
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
        let moving: Vec<u32> = (0..cloth_triangles.len() as u32)
            .filter(|&slot| cloth_triangles[slot as usize].iter().any(|&v| changed[v]))
            .collect();
        let mut moving_rank = vec![None; cloth_triangles.len()];
        for (rank, &slot) in moving.iter().enumerate() {
            moving_rank[slot as usize] = Some(rank as u32);
        }
        let mut cloth_first = Bits::new(moving.len(), cloth_triangles.len());
        let mut cloth_second = Bits::new(moving.len(), cloth_triangles.len());
        for &(a, b) in &cloth {
            if let Some(rank) = moving_rank[a as usize] {
                cloth_first.set(rank, b);
            }
            if let Some(rank) = moving_rank[b as usize] {
                cloth_second.set(rank, a);
            }
            debug_assert!(
                moving_rank[a as usize].is_some() || moving_rank[b as usize].is_some(),
                "a pair has a moving triangle"
            );
        }
        let mut arm_partners = Bits::new(moving.len(), skin_triangles.len());
        for &(a, b) in &arm {
            let rank = moving_rank[a as usize];
            debug_assert!(rank.is_some(), "an arm pair's garment triangle moves");
            if let Some(rank) = rank {
                arm_partners.set(rank, b);
            }
        }
        let centre =
            |points: &[Vec3], triangle: &[usize; 3]| triangle.map(|i| points[i]).iter().sum::<Vec3>() / 3.0;
        let cloth_tree = BoxTree::new(&cloth_triangles.iter().map(|t| centre(&rest, t)).collect::<Vec<_>>());
        let skin_tree =
            BoxTree::new(&skin_triangles.iter().map(|t| centre(&skin_rest, t)).collect::<Vec<_>>());
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
            moving,
            moving_rank,
            cloth_first,
            cloth_second,
            arm_partners,
            cloth_tree,
            skin_tree,
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

    /// The shader and hair pass share the same accepted weight and posed tee. The weight is the least
    /// gain that clears the fold, or what the last solve gave released over `elapsed` ticks if that's
    /// more, within the pose's limit: so a fold no gain clears doesn't drop the relief in one frame.
    pub(super) fn solve(
        &mut self,
        pose: &mut CharacterPose,
        chest_from_model: Affine3A,
        shirt: &[Point],
        stock: &mut [Vec3],
        elapsed: Option<f32>,
    ) {
        let limit = self.pose_limit(pose).max(0.0);
        let (chosen, placed) =
            if limit > 0.0 { self.choose(pose, chest_from_model, shirt, stock, limit) } else { (0.0, false) };
        let weight = settled_weight(chosen, self.weight, limit, elapsed);
        self.weight = weight;
        if weight <= 0.0 {
            return;
        }
        if !placed {
            self.place(pose, chest_from_model, shirt);
        }
        for (morph, offset) in self.shape.iter().zip(&self.offsets) {
            stock[morph.shirt_point] += *offset * weight;
        }
        pose.weights_mut()[self.target] = weight;
    }

    /// Where the skin is and what the relief moves each of its garment points by, at full weight.
    fn place(&mut self, pose: &CharacterPose, chest_from_model: Affine3A, shirt: &[Point]) {
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
    }

    /// The least gain up to `limit` that clears the garment's fold without cutting further into the
    /// arm, or 0 if there's no fold or none does; and whether it [placed](Self::place) the relief.
    fn choose(
        &mut self,
        pose: &CharacterPose,
        chest_from_model: Affine3A,
        shirt: &[Point],
        stock: &[Vec3],
        limit: f32,
    ) -> (f32, bool) {
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
            return (0.0, false);
        }
        self.place(pose, chest_from_model, shirt);
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
        self.cloth_tree.refit(&self.swept_bounds);
        self.skin_tree.refit(&self.skin_bounds);
        self.active_cloth.clear();
        self.active_arm.clear();
        // Every pair has a moving triangle, so the pairs whose bounds meet are found from those: a pair
        // of two moving triangles from the one that comes first in it.
        for (rank, &slot) in self.moving.iter().enumerate() {
            let rank = rank as u32;
            let bounds = self.swept_bounds[slot as usize];
            let (swept, moving_rank, active) =
                (&self.swept_bounds, &self.moving_rank, &mut self.active_cloth);
            let (first, second) = (&self.cloth_first, &self.cloth_second);
            self.cloth_tree.meeting(bounds, |other| {
                if !overlap(bounds, swept[other as usize]) {
                    return;
                }
                if first.get(rank, other) {
                    active.push((slot, other));
                }
                if second.get(rank, other) && moving_rank[other as usize].is_none() {
                    active.push((other, slot));
                }
            });
            let (skin, partners, active) = (&self.skin_bounds, &self.arm_partners, &mut self.active_arm);
            self.skin_tree.meeting(bounds, |other| {
                if overlap(bounds, skin[other as usize]) && partners.get(rank, other) {
                    active.push((slot, other));
                }
            });
        }
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
        (chosen, true)
    }

    fn clears(&mut self, stock: &[Vec3], gain: f32) -> bool {
        for (morph, offset) in self.shape.iter().zip(&self.offsets) {
            self.candidate[morph.shirt_point] = stock[morph.shirt_point] + *offset * gain;
        }
        let cloth = |triangle: u32| self.cloth_triangles[triangle as usize];
        let moved = |triangle: u32| cloth(triangle).map(|i| self.candidate[i]);
        if self.active_cloth.iter().any(|&(a, b)| cut(moved(a), moved(b)) >= FOLD) {
            return false;
        }
        self.active_arm.iter().all(|&(a, b)| {
            let skin = self.skin_triangles[b as usize].map(|i| self.posed_skin[i]);
            let new = cut(moved(a), skin);
            new < FOLD || new <= cut(cloth(a).map(|i| stock[i]), skin)
        })
    }
}

/// A row of bits for each of some items, over others.
#[derive(Clone, Debug, PartialEq)]
struct Bits {
    words: usize,
    bits: Vec<u64>,
}

impl Bits {
    fn new(rows: usize, columns: usize) -> Self {
        let words = columns.div_ceil(64);
        Self { words, bits: vec![0; rows * words] }
    }

    fn set(&mut self, row: u32, column: u32) {
        self.bits[row as usize * self.words + column as usize / 64] |= 1 << (column % 64);
    }

    fn get(&self, row: u32, column: u32) -> bool {
        self.bits[row as usize * self.words + column as usize / 64] & 1 << (column % 64) != 0
    }
}

/// A tree over items grouped by where they lie at rest, refit to their bounds as they're posed: each
/// node's left child follows it.
#[derive(Clone, Debug, PartialEq)]
struct BoxTree {
    /// The items, in the order of the leaves.
    items: Vec<u32>,
    /// Per node, its items and its right child if it has children.
    nodes: Vec<(usize, usize, Option<usize>)>,
    bounds: Vec<(Vec3, Vec3)>,
}

impl BoxTree {
    fn new(centres: &[Vec3]) -> Self {
        let mut items: Vec<_> = (0..centres.len() as u32).collect();
        let mut nodes = Vec::new();
        Self::layout(centres, &mut items, 0, &mut nodes);
        Self { items, bounds: vec![(Vec3::ZERO, Vec3::ZERO); nodes.len()], nodes }
    }

    /// Splits `items` at the middle of their centres' widest axis until a leaf holds 8 or fewer.
    fn layout(
        centres: &[Vec3],
        items: &mut [u32],
        start: usize,
        nodes: &mut Vec<(usize, usize, Option<usize>)>,
    ) {
        let at = nodes.len();
        nodes.push((start, start + items.len(), None));
        if items.len() <= 8 {
            return;
        }
        let (lo, hi) = items
            .iter()
            .fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), &item| {
                (lo.min(centres[item as usize]), hi.max(centres[item as usize]))
            });
        let axis = (hi - lo).max_position();
        items.sort_unstable_by(|&a, &b| {
            centres[a as usize][axis].total_cmp(&centres[b as usize][axis]).then(a.cmp(&b))
        });
        let middle = items.len() / 2;
        let (left, right) = items.split_at_mut(middle);
        Self::layout(centres, left, start, nodes);
        nodes[at].2 = Some(nodes.len());
        Self::layout(centres, right, start + middle, nodes);
    }

    /// Bounds each node by its items' `bounds`.
    fn refit(&mut self, bounds: &[(Vec3, Vec3)]) {
        for (i, &(start, end, right)) in self.nodes.iter().enumerate().rev() {
            self.bounds[i] = match right {
                Some(right) => {
                    let (a, b) = (self.bounds[i + 1], self.bounds[right]);
                    (a.0.min(b.0), a.1.max(b.1))
                }
                None => self.items[start..end]
                    .iter()
                    .fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), &item| {
                        (lo.min(bounds[item as usize].0), hi.max(bounds[item as usize].1))
                    }),
            };
        }
    }

    /// Calls `found` with each item of every leaf whose bounds meet `bounds`: so with every item whose
    /// own bounds do, and some whose don't.
    fn meeting(&self, bounds: (Vec3, Vec3), mut found: impl FnMut(u32)) {
        let mut stack = [0; 64];
        let mut depth = 1;
        while depth > 0 {
            depth -= 1;
            let i = stack[depth];
            if !overlap(self.bounds[i], bounds) {
                continue;
            }
            match self.nodes[i] {
                (_, _, Some(right)) => {
                    stack[depth] = right;
                    stack[depth + 1] = i + 1;
                    depth += 2;
                }
                (start, end, None) => self.items[start..end].iter().for_each(|&item| found(item)),
            }
        }
    }
}

/// The relief's weight after a solve chose `chosen`, the last gave `held`, and `elapsed` ticks went by
/// (`None` across a cut): at least `chosen`, at once, and what's let go goes over
/// [`RELEASE_HALF_LIFE`], within the pose's `limit`.
fn settled_weight(chosen: f32, held: f32, limit: f32, elapsed: Option<f32>) -> f32 {
    let released = elapsed.map_or(0.0, |dt| held * (-dt.max(0.0) / RELEASE_HALF_LIFE).exp2());
    chosen.max(released.min(limit))
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
    use super::{BoxTree, cut, overlap, pose_limit, settled_weight};
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

    /// A solve whose fold no gain clears, or whose fold has gone, chooses 0; the relief then lets go
    /// of its weight over a few frames, where it used to drop from near its limit to 0 in one.
    #[test]
    fn the_relief_lets_go_over_frames_when_nothing_clears() {
        let limit = 0.9;
        let mut weight = settled_weight(0.8, 0.0, limit, Some(1.0));
        assert_eq!(weight, 0.8, "a gain that clears is taken at once");
        let mut frames = 0;
        while weight > 0.01 {
            let next = settled_weight(0.0, weight, limit, Some(1.0));
            assert!(next < weight && next > weight * 0.8, "{next} after {weight}");
            weight = next;
            frames += 1;
        }
        assert!((20..40).contains(&frames), "{frames} frames to let go");
        assert_eq!(settled_weight(0.0, 0.8, limit, Some(0.0)), 0.8, "no time, no change");
        assert_eq!(settled_weight(0.0, 0.8, 0.3, Some(1.0)), 0.3, "never past the pose's limit");
        assert_eq!(settled_weight(0.0, 0.8, 0.0, Some(1.0)), 0.0, "nor where the pose allows none");
        assert_eq!(settled_weight(0.2, 0.8, limit, None), 0.2, "a cut starts over");
        assert!(settled_weight(0.5, 0.8, limit, Some(1.0)) > 0.5);
    }

    /// The trees find the pairs whose bounds meet, so they must reach every item whose bounds meet the
    /// ones asked about, however far the items have moved from where the tree grouped them.
    #[test]
    fn a_box_tree_finds_every_item_whose_bounds_meet() {
        let mut seed = 0x1234_5678_u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let mut point = || Vec3::new(random(), random(), random()) - Vec3::splat(0.5);
        let centres: Vec<_> = (0..500).map(|_| point()).collect();
        let mut tree = BoxTree::new(&centres);
        for _ in 0..3 {
            let bounds: Vec<_> = centres
                .iter()
                .map(|&c| {
                    let (c, extent) = (c + point() * 0.3, point().abs() * 0.1);
                    (c - extent, c + extent)
                })
                .collect();
            tree.refit(&bounds);
            for _ in 0..200 {
                let (c, extent) = (point(), point().abs() * 0.2);
                let asked = (c - extent, c + extent);
                let mut found = vec![false; bounds.len()];
                tree.meeting(asked, |item| found[item as usize] = true);
                for (item, &own) in bounds.iter().enumerate() {
                    assert!(found[item] || !overlap(asked, own), "item {item} missed");
                }
            }
        }
    }
}
