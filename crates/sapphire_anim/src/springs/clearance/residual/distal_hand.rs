//! Current left-hand contact on the distal two links of front lock 11.
use sapphire_math::Mat3;

use super::{
    Affine3A, Angles, ArchivedSkeleton, Bounds, CharacterPose, Contact, DrawSample, Field, Link, LockProfile,
    MAX_ANGLE, MAX_RATE, Mesh, Node, Patch, Point, Quat, Sample, SamplePart, Strand, SurfaceVertex, Triangle,
    Vec3, WeightedPoint, Zone, jacobian, layout, place_links, posed_sample, sample_position,
};

const FAMILIES: [&str; 5] =
    ["J_Bip_L_Thumb", "J_Bip_L_Index", "J_Bip_L_Middle", "J_Bip_L_Ring", "J_Bip_L_Little"];
// The cooker drops the unweighted VRM fingertip end nodes. These local
// positions come from the five named end nodes of this heroine profile.
const TIP: [Vec3; 5] = [
    Vec3::new(0.022_038_998, -8.345_795e-8, 1.530_876_3e-7),
    Vec3::new(0.023_368_407, -8.568_288e-8, -8.913_02e-8),
    Vec3::new(0.024_743_27, 4.369_200_3e-7, 1.518_125_9e-8),
    Vec3::new(0.020_898_132, 2.448_655e-8, 4.258_938e-8),
    Vec3::new(0.019_241_221, -2.611_625_7e-7, -3.070_731_6e-8),
];
const FINGERS: usize = 15;
const PALM: &str = "J_Bip_L_Hand";
const MAX_PALM_FACES: usize = 192;
const REACH: f32 = 0.010;
const EXIT: f32 = 0.020;
const TERMINAL: f32 = 2.0;
// A shared material edge can use duplicate cooked vertices. Allow reconstruction
// error when matching its contact point, without broadening the contact volume.
const CREASE_POINT_TOLERANCE: f32 = 0.000_025;

#[derive(Clone, Copy, PartialEq)]
enum SupportChoice {
    Nearest,
    Opposite,
    Crease,
}

#[derive(Clone, Debug, PartialEq)]
struct Skin {
    patches: Vec<Patch>,
    nodes: Vec<Node>,
    points: Vec<Point>,
    current: Vec<Vec3>,
    field: Field,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Bone {
    a: Vec3,
    b: Vec3,
}

impl Bone {
    fn vector(self) -> Vec3 {
        self.b - self.a
    }

    fn point(self, t: f32) -> Vec3 {
        self.a + self.vector() * t
    }

    fn bounds(self) -> Bounds {
        Bounds { lo: self.a.min(self.b) - Vec3::splat(REACH), hi: self.a.max(self.b) + Vec3::splat(REACH) }
    }
}

fn overlap(a: Bounds, b: Bounds) -> bool {
    a.lo.cmple(b.hi).all() && b.lo.cmple(a.hi).all()
}

fn barycentric(triangle: Triangle, point: Vec3) -> Vec3 {
    let d = point - triangle.a;
    let ed = d.dot(triangle.e);
    let fd = d.dot(triangle.f);
    let s = (ed * triangle.ff - fd * triangle.ef) * triangle.inv_det;
    let t = (fd * triangle.ee - ed * triangle.ef) * triangle.inv_det;
    Vec3::new(1.0 - s - t, s, t)
}

fn interpolate(values: [Vec3; 3], weights: Vec3) -> Vec3 {
    values[0] * weights.x + values[1] * weights.y + values[2] * weights.z
}

#[derive(Clone, Copy)]
struct Support {
    triangle: usize,
    feature: usize,
    gap: f32,
    weights: Vec3,
}

fn consider(best: &mut Support, candidate: Support) {
    if candidate.gap < best.gap
        || (candidate.gap == best.gap
            && (candidate.feature, candidate.triangle) < (best.feature, best.triangle))
    {
        *best = candidate;
    }
}

fn closest(bone: Bone, tri: Triangle, index: usize, best: &mut Support) {
    let p = [tri.a, tri.a + tri.e, tri.a + tri.f];
    let mut save = |feature, gap, weights| consider(best, Support { triangle: index, feature, gap, weights });
    for (i, end) in [bone.a, bone.b].into_iter().enumerate() {
        let w = barycentric(tri, end);
        if w.min_element() >= 0.0 {
            save(i, end.distance_squared(interpolate(p, w)), w);
        }
    }
    let axis = bone.vector();
    let cross = tri.e.cross(tri.f);
    let den = cross.dot(axis);
    if den.abs() > 1.0e-12 {
        let t = cross.dot(tri.a - bone.a) / den;
        if (0.0..=1.0).contains(&t) {
            let w = barycentric(tri, bone.point(t));
            if w.min_element() >= 0.0 {
                save(2, 0.0, w);
            }
        }
    }
    for (edge_index, (i, j)) in [(0, 1), (1, 2), (2, 0)].into_iter().enumerate() {
        let edge = p[j] - p[i];
        let aa = axis.length_squared().max(1.0e-14);
        let bb = edge.length_squared().max(1.0e-14);
        let uv = edge.dot(axis);
        let d = bone.a - p[i];
        let dd = d.dot(axis);
        let ee = edge.dot(d);
        let det = aa * bb - uv * uv;
        let middle =
            if det > 1.0e-14 { ((uv * ee - bb * dd) / det, (aa * ee - uv * dd) / det) } else { (0.0, 0.0) };
        let features = [
            middle,
            (0.0, (ee / bb).clamp(0.0, 1.0)),
            (1.0, ((ee + uv) / bb).clamp(0.0, 1.0)),
            ((-dd / aa).clamp(0.0, 1.0), 0.0),
            (((uv - dd) / aa).clamp(0.0, 1.0), 1.0),
        ];
        for (k, (s, t)) in features.into_iter().enumerate() {
            if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
                let mut w = Vec3::ZERO;
                w[i] = 1.0 - t;
                w[j] = t;
                save(3 + edge_index * 5 + k, (d + axis * s - edge * t).length_squared(), w);
            }
        }
    }
}

// Intersect each face with the other face's plane. A transverse cut is
// the common interval on the line shared by those planes.
fn cut_interval(tri: Triangle, against: Triangle, direction: Vec3) -> Option<(f32, f32)> {
    let points = [tri.a, tri.a + tri.e, tri.a + tri.f];
    let signed = points.map(|p| (p - against.a).dot(against.normal));
    let [a, b, c] = signed;
    if a.min(b).min(c) >= -1.0e-5 || a.max(b).max(c) <= 1.0e-5 {
        return None;
    }
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for (i, j) in [(0, 1), (1, 2), (2, 0)] {
        if (signed[i] > 0.0) != (signed[j] > 0.0) {
            let t = signed[i] / (signed[i] - signed[j]);
            let coordinate = points[i].lerp(points[j], t).dot(direction);
            lo = lo.min(coordinate);
            hi = hi.max(coordinate);
        }
    }
    Some((lo, hi))
}

fn cut_length(a: Triangle, b: Triangle) -> f32 {
    let axis = a.normal.cross(b.normal);
    if axis.length_squared() < 1.0e-8 {
        return 0.0;
    }
    let direction = axis.normalize();
    let Some((lo_a, hi_a)) = cut_interval(a, b, direction) else {
        return 0.0;
    };
    let Some((lo_b, hi_b)) = cut_interval(b, a, direction) else {
        return 0.0;
    };
    (hi_a.min(hi_b) - lo_a.max(lo_b)).max(0.0)
}

impl Skin {
    fn ray(&self, node: usize, start: Vec3, direction: Vec3, best: &mut f32) {
        let bound = self.field.bounds[node];
        let mut lo: f32 = 0.0;
        let mut hi = *best;
        for i in 0..3 {
            if direction[i].abs() <= 1.0e-8 {
                if start[i] < bound.lo[i] || start[i] > bound.hi[i] {
                    return;
                }
            } else {
                let a = (bound.lo[i] - start[i]) / direction[i];
                let b = (bound.hi[i] - start[i]) / direction[i];
                lo = lo.max(a.min(b));
                hi = hi.min(a.max(b));
                if lo > hi {
                    return;
                }
            }
        }
        let node_data = self.nodes[node];
        if let Some(right) = node_data.right {
            self.ray(node + 1, start, direction, best);
            self.ray(right, start, direction, best);
        } else {
            for tri in &self.field.triangles[node_data.start..node_data.end] {
                let normal = tri.e.cross(tri.f);
                let den = normal.dot(direction);
                if den > 1.0e-10 {
                    let t = (tri.a - start).dot(normal) / den;
                    if t >= 0.0
                        && t < *best
                        && barycentric(*tri, start + direction * t).min_element() >= -1.0e-6
                    {
                        *best = t;
                    }
                }
            }
        }
    }

    fn cut_energy(&self, node: usize, triangle: Triangle, bounds: Bounds) -> f32 {
        if !overlap(self.field.bounds[node], bounds) {
            return 0.0;
        }
        let data = self.nodes[node];
        if let Some(right) = data.right {
            self.cut_energy(node + 1, triangle, bounds) + self.cut_energy(right, triangle, bounds)
        } else {
            self.field.triangles[data.start..data.end]
                .iter()
                .filter(|other| overlap(other.bounds(), bounds))
                .map(|&other| cut_length(triangle, other))
                .filter(|&length| length >= 0.0003)
                .map(|length| length * length)
                .sum()
        }
    }

    fn refit(&mut self, pose: &CharacterPose, chest: Affine3A) {
        for (out, p) in self.current.iter_mut().zip(&self.points) {
            *out = chest.transform_point3(p.model(pose.model()));
        }
        self.field.refit(&self.nodes, &self.patches, |v| match v {
            SurfaceVertex::Skin(i) => self.current[i],
            SurfaceVertex::Tee(_) => unreachable!(),
        });
    }
}

#[derive(Clone, Copy)]
struct Evaluation {
    crease: bool,
    gram: Mat3,
    rhs: Vec3,
    side: [Option<Vec3>; FINGERS],
}

type DistalAngles = [Vec3; 2];

#[derive(Clone, Copy, Default)]
struct CutEnergy {
    fingers: f32,
    palm: f32,
}

impl CutEnergy {
    fn total(self) -> f32 {
        self.fingers + self.palm
    }

    fn improves(self, score: f32, before: Self, before_score: f32) -> bool {
        self.palm <= before.palm && score < before_score
    }
}

struct Trial {
    score: f32,
    angles: (DistalAngles, DistalAngles),
    side: [Option<Vec3>; FINGERS],
    cut_energy: CutEnergy,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DistalHand {
    pub(super) strand: usize,
    fingers: [[usize; 2]; FINGERS],
    shaft: Vec3,
    skin: Vec<Skin>,
    samples: Vec<Sample>,
    faces: Vec<[usize; 3]>,
    points: Vec<Vec3>,
    preceding: Vec<Vec3>,
    triangle_bounds: Vec<Bounds>,
    previous_bones: [Bone; FINGERS],
    had_pose: bool,
    side: [Option<Vec3>; FINGERS],
    extra: DistalAngles,
    final_angles: Option<DistalAngles>,
}

impl DistalHand {
    pub(super) fn new(
        mesh: &Mesh,
        skeleton: &ArchivedSkeleton,
        lock: &LockProfile,
        strand: &Strand,
        hair_faces: &[[u32; 3]],
        body_faces: impl Iterator<Item = [u32; 3]>,
        vertex: impl Fn(usize) -> Option<(Vec3, Point)>,
    ) -> Option<Self> {
        let mut fingers = [[0; 2]; FINGERS];
        let mut skin = Vec::new();
        let body: Vec<_> = body_faces.collect();
        let mut unsupported = vec![false; mesh.streams.len()];
        for delta in &mesh.morph_deltas {
            if Vec3::from_array(delta.position()) != Vec3::ZERO {
                unsupported[delta.vertex() as usize] = true;
            }
        }
        'surface: for (family, prefix) in FAMILIES.into_iter().chain([PALM]).enumerate() {
            let palm = family == FAMILIES.len();
            if !palm {
                for k in 0..3 {
                    let name = format!("{prefix}{}", k + 1);
                    let next = if k == 2 { format!("{prefix}3_end") } else { format!("{prefix}{}", k + 2) };
                    let find = |name: &str| skeleton.names.iter().position(|n| n.as_str() == name);
                    let start = find(&name)?;
                    let end = if k == 2 { find(&next).unwrap_or(start) } else { find(&next)? };
                    fingers[family * 3 + k] = [start, end];
                }
            }
            let mut patches = Vec::new();
            let mut points = Vec::new();
            let mut map = vec![None; mesh.streams.len()];
            for &face in &body {
                let [Some(a), Some(b), Some(c)] = face.map(|i| vertex(i as usize)) else {
                    continue;
                };
                let parts = [a.1, b.1, c.1].into_iter().flat_map(|p| p.parts);
                let weight = |prefix: &str, exact: bool| {
                    parts
                        .clone()
                        .filter(|p| {
                            let name = skeleton.names[p.joint].as_str();
                            p.weight > 0.0 && if exact { name == prefix } else { name.starts_with(prefix) }
                        })
                        .map(|p| p.weight)
                        .sum::<f32>()
                };
                if weight(prefix, palm) < 1.5
                    || (palm && FAMILIES.iter().any(|prefix| weight(prefix, false) >= 1.5))
                {
                    continue;
                }
                if face.iter().any(|&i| unsupported[i as usize]) {
                    if palm {
                        // An unsupported palm may keep the established finger response.
                        break 'surface;
                    }
                    return None;
                }
                let vertices = std::array::from_fn(|i| {
                    SurfaceVertex::Skin(*map[face[i] as usize].get_or_insert_with(|| {
                        let index = points.len();
                        points.push([a.1, b.1, c.1][i]);
                        index
                    }))
                });
                patches.push(Patch {
                    vertices,
                    rest_center: (a.0 + b.0 + c.0) / 3.0,
                    source: face,
                    zone: Zone::Arm,
                });
            }
            let capacity = if palm { MAX_PALM_FACES } else { 384 };
            if patches.is_empty() || patches.len() > capacity {
                if palm {
                    break;
                }
                return None;
            }
            let mut nodes = Vec::new();
            layout(&mut patches, 0, &mut nodes);
            let field = Field::new(patches.len(), nodes.len());
            let current = vec![Vec3::ZERO; points.len()];
            skin.push(Skin { patches, nodes, points, current, field });
        }
        let mut map = vec![None; mesh.streams.len()];
        let mut samples = Vec::new();
        let mut faces = Vec::new();
        let affected =
            |joint| strand.joints.iter().position(|&j| j == joint).filter(|&i| i >= 2).map(|i| i.min(3));
        for &source in hair_faces {
            let [Some(a), Some(b), Some(c)] = source.map(|i| vertex(i as usize).map(|v| v.1)) else {
                continue;
            };
            let skin_points = [a, b, c];
            if !skin_points
                .iter()
                .flat_map(|p| p.parts)
                .any(|p| p.weight > 0.001 && affected(p.joint).is_some())
            {
                continue;
            }
            faces.push(std::array::from_fn(|i| {
                *map[source[i] as usize].get_or_insert_with(|| {
                    let mut parts = [SamplePart::default(); 12];
                    for (out, p) in parts.iter_mut().zip(skin_points[i].parts) {
                        *out = SamplePart {
                            joint: p.joint,
                            affected: affected(p.joint),
                            local: p.local,
                            weight: p.weight,
                        };
                    }
                    let index = samples.len();
                    samples.push(Sample { source: [source[i]; 3], parts });
                    index
                })
            }));
        }
        if faces.is_empty() || faces.len() > 768 || samples.len() > 512 {
            return None;
        }
        let points = vec![Vec3::ZERO; samples.len()];
        let preceding = points.clone();
        let triangle_bounds = vec![Bounds::EMPTY; faces.len()];
        let shaft =
            Vec3::from_array(skeleton.rest[lock.joints[3]].to_native().translation).normalize_or_zero();
        Some(Self {
            strand: lock.strand,
            fingers,
            shaft,
            skin,
            samples,
            faces,
            points,
            preceding,
            triangle_bounds,
            previous_bones: [Bone::default(); FINGERS],
            had_pose: false,
            side: [None; FINGERS],
            extra: [Vec3::ZERO; 2],
            final_angles: None,
        })
    }

    pub(super) fn reset(&mut self) {
        self.sleep();
        self.extra = [Vec3::ZERO; 2];
        self.final_angles = None;
    }

    pub(super) fn sleep(&mut self) {
        self.had_pose = false;
        self.side = [None; FINGERS];
    }

    pub(super) fn continue_from(&mut self, old: &Self) {
        if self.skin.len() == old.skin.len()
            && self.strand == old.strand
            && self.fingers == old.fingers
            && self.shaft == old.shaft
            && self.samples == old.samples
            && self.faces == old.faces
            && self.skin.iter().zip(&old.skin).all(|(a, b)| a.patches == b.patches && a.points == b.points)
        {
            self.preceding.copy_from_slice(&old.preceding);
            self.previous_bones = old.previous_bones;
            self.had_pose = old.had_pose;
            self.side = old.side;
            self.extra = old.extra;
            self.final_angles = old.final_angles;
        } else {
            self.reset();
        }
    }

    fn position_ribbon(&mut self, samples: &[DrawSample], links: &[super::PlacedLink; 4]) {
        for (point, sample) in self.points.iter_mut().zip(samples) {
            *point = sample_position(*sample, links).0;
        }
    }

    fn cut_energy(&self) -> CutEnergy {
        let overall = self.points.iter().fold(Bounds::EMPTY, |bounds, &p| bounds.with_point(p));
        let mut energy = CutEnergy::default();
        for (i, skin) in self.skin.iter().enumerate() {
            if !overlap(skin.field.bounds[0], overall) {
                continue;
            }
            let component = if i < FAMILIES.len() { &mut energy.fingers } else { &mut energy.palm };
            for indices in &self.faces {
                let points = indices.map(|i| self.points[i]);
                let bounds = points.into_iter().fold(Bounds::EMPTY, Bounds::with_point);
                if overlap(skin.field.bounds[0], bounds) {
                    *component += skin.cut_energy(0, Triangle::new(points), bounds);
                }
            }
        }
        energy
    }

    fn crease_support(&self, bone: Bone, bounds: Bounds, nearest: Support) -> Option<Support> {
        let indices = self.faces.get(nearest.triangle)?;
        let points = indices.map(|v| self.points[v]);
        let normal = Triangle::new(points).normal;
        if normal == Vec3::ZERO {
            return None;
        }
        let point = interpolate(points, nearest.weights);
        let mut crease = Support {
            triangle: usize::MAX,
            feature: usize::MAX,
            gap: (nearest.gap.sqrt() + CREASE_POINT_TOLERANCE).min(REACH).powi(2),
            weights: Vec3::ZERO,
        };
        for (i, (&tri_bounds, indices)) in self.triangle_bounds.iter().zip(&self.faces).enumerate() {
            if i == nearest.triangle || !overlap(tri_bounds, bounds) {
                continue;
            }
            let points = indices.map(|v| self.points[v]);
            let triangle = Triangle::new(points);
            if triangle.normal == Vec3::ZERO || triangle.normal.dot(normal).abs() >= 0.5 {
                continue;
            }
            let mut candidate = crease;
            closest(bone, triangle, i, &mut candidate);
            if candidate.triangle == i
                && interpolate(points, candidate.weights).distance_squared(point)
                    <= CREASE_POINT_TOLERANCE.powi(2)
            {
                crease = candidate;
            }
        }
        (crease.triangle != usize::MAX).then_some(crease)
    }

    fn evaluate(
        &mut self,
        samples: &[DrawSample],
        parents: &[Link; 4],
        bones: &[Bone; FINGERS],
        extra: DistalAngles,
        branch: [Option<Vec3>; FINGERS],
        choice: SupportChoice,
    ) -> Evaluation {
        let opposite = choice == SupportChoice::Opposite;
        let links = place_links(parents, [Vec3::ZERO, Vec3::ZERO, extra[0], extra[1]]);
        self.position_ribbon(samples, &links);
        let mut eval = Evaluation {
            crease: false,
            gram: Mat3::from_diagonal(Vec3::splat(0.0015)),
            rhs: Vec3::ZERO,
            side: [None; FINGERS],
        };
        let overall = self.points.iter().fold(Bounds::EMPTY, |bounds, &p| bounds.with_point(p));
        if !bones.iter().any(|bone| overlap(overall, bone.bounds())) {
            return eval;
        }
        for (out, ids) in self.triangle_bounds.iter_mut().zip(&self.faces) {
            *out = ids.iter().fold(Bounds::EMPTY, |bounds, &i| bounds.with_point(self.points[i]));
        }
        for (i, &bone) in bones.iter().enumerate() {
            let bounds = bone.bounds();
            if !overlap(overall, bounds) {
                continue;
            }
            let mut support = Support {
                triangle: usize::MAX,
                feature: usize::MAX,
                gap: REACH * REACH,
                weights: Vec3::ZERO,
            };
            for (j, (&tri_bound, ids)) in self.triangle_bounds.iter().zip(&self.faces).enumerate() {
                if overlap(tri_bound, bounds) {
                    closest(bone, Triangle::new(ids.map(|v| self.points[v])), j, &mut support);
                }
            }
            if choice == SupportChoice::Crease
                && branch[i].is_some()
                && let Some(crease) = self.crease_support(bone, bounds, support)
            {
                support = crease;
                eval.crease = true;
            }
            let Some(&ids) = self.faces.get(support.triangle) else {
                continue;
            };
            let tri = ids.map(|v| self.points[v]);
            let point = interpolate(tri, support.weights);
            let axis = bone.vector();
            let t = ((point - bone.a).dot(axis) / axis.length_squared().max(1.0e-14)).clamp(0.0, 1.0);
            let axis_point = bone.point(t);
            let mut direction = Triangle::new(tri).normal;
            if direction == Vec3::ZERO {
                continue;
            }
            let flip = if let Some(before) = branch[i] {
                direction.dot(before) < 0.0
            } else if self.had_pose {
                let before = ids.map(|v| self.preceding[v]);
                let mut normal = Triangle::new(before).normal;
                if normal.dot(direction) < 0.0 {
                    normal = -normal;
                }
                (interpolate(before, support.weights) - self.previous_bones[i].point(t)).dot(normal) < 0.0
            } else {
                (point - axis_point).dot(direction) < 0.0
            };
            if flip ^ opposite {
                direction = -direction;
            }
            let mut exit = EXIT;
            self.skin[i / 3].ray(0, axis_point, direction, &mut exit);
            let signed = (point - axis_point).dot(direction);
            let depth = (exit + 0.0015 - signed).clamp(0.0, 0.012);
            if exit == EXIT || signed.abs() > 0.018 || depth == 0.0 {
                continue;
            }
            let levers = ids.map(|v| sample_position(samples[v], &links).1);
            let lever = std::array::from_fn(|k| interpolate(levers.map(|l| l[k]), support.weights));
            let row = jacobian(Contact { normal: direction, depth }, lever, &links);
            let row = Vec3::new(row[6] + row[9], row[7] + row[10], row[8] + row[11]);
            eval.gram += Mat3::from_cols(row * row.x, row * row.y, row * row.z);
            eval.rhs += row * depth;
            eval.side[i] = Some(direction);
        }
        eval
    }

    pub(super) fn apply(
        &mut self,
        pose: &mut CharacterPose,
        chest: Affine3A,
        lock: &LockProfile,
        base_angles: Angles,
        dt: f32,
        allowed: impl Fn(&CharacterPose, &[super::PlacedLink; 4]) -> bool,
    ) {
        if dt > 0.0 || !self.had_pose {
            let parents = lock.joints.map(|joint| Link::current(joint, pose, chest));
            let bones = std::array::from_fn(|i| {
                let [a, b] = self.fingers[i];
                let end = if a == b {
                    pose.model()[a].transform_point3(TIP[i / 3])
                } else {
                    pose.model()[b].translation.into()
                };
                Bone {
                    a: chest.transform_point3(pose.model()[a].translation.into()),
                    b: chest.transform_point3(end),
                }
            });
            // Small fixed rig patch. Its transient posed data is stack-backed.
            let empty = WeightedPoint { position: Vec3::ZERO, affected: None, weight: 0.0 };
            let mut posed = [[empty; 12]; 512];
            let posed = &mut posed[..self.samples.len()];
            for (out, sample) in posed.iter_mut().zip(&self.samples) {
                *out = posed_sample(sample, pose.model(), chest);
            }
            if dt > 0.0 {
                let base = [base_angles[2], base_angles[3]];
                let previous = self.final_angles.unwrap_or(base);
                let project = |request: DistalAngles| -> (DistalAngles, DistalAngles) {
                    let total = std::array::from_fn(|i| {
                        let angle = if request[i] == Vec3::ZERO {
                            base[i]
                        } else {
                            (Quat::from_scaled_axis(request[i]) * Quat::from_scaled_axis(base[i]))
                                .normalize()
                                .to_scaled_axis()
                        }
                        .clamp_length_max(MAX_ANGLE[i + 2].to_radians());
                        previous[i]
                            + (angle - previous[i]).clamp_length_max((MAX_RATE[i + 2] * dt).to_radians())
                    });
                    let local = std::array::from_fn(|i| {
                        if total[i] == base[i] {
                            Vec3::ZERO
                        } else {
                            (Quat::from_scaled_axis(total[i]) * Quat::from_scaled_axis(base[i]).inverse())
                                .normalize()
                                .to_scaled_axis()
                        }
                    });
                    (local, total)
                };
                let prior = self.extra;
                let release = (-dt / 4.0).exp2();
                let mut target = project(prior.map(|a| a * release));
                let prior_side = self.side;
                let mut side = prior_side;
                let mut active = [None; FINGERS];
                let near = posed
                    .iter()
                    .fold(Bounds::EMPTY, |bounds, sample| bounds.with_point(super::base_position(*sample)));
                if bones.iter().any(|bone| {
                    let mut bounds = bone.bounds();
                    // Allow room for the already retained two-link bend and its bounded step.
                    bounds.lo -= Vec3::splat(0.050);
                    bounds.hi += Vec3::splat(0.050);
                    overlap(near, bounds)
                }) {
                    for skin in &mut self.skin {
                        skin.refit(pose, chest);
                    }
                    let shared = (prior[0] + prior[1]) * 0.5;
                    let continuation = shared.normalize_or_zero() * (TERMINAL * dt).to_radians();
                    let limit = |request: DistalAngles| {
                        project(std::array::from_fn(|i| {
                            prior[i]
                                + (request[i].clamp_length_max(MAX_ANGLE[3].to_radians()) - prior[i])
                                    .clamp_length_max((TERMINAL * dt).to_radians())
                        }))
                    };
                    let side_at_entry = prior_side.iter().all(Option::is_none);
                    let have_palm = self.skin.len() > FAMILIES.len();
                    let shaft = self.shaft;
                    let release_angles = target.0;
                    let score = |solver: &mut Self,
                                 candidate: (DistalAngles, DistalAngles),
                                 route,
                                 best: &mut Trial| {
                        let links =
                            place_links(&parents, [Vec3::ZERO, Vec3::ZERO, candidate.0[0], candidate.0[1]]);
                        solver.position_ribbon(posed, &links);
                        let step =
                            (candidate.0[0] - release_angles[0] + candidate.0[1] - release_angles[1]) * 0.5;
                        let cuts = solver.cut_energy();
                        let score = cuts.total() + 0.001 * step.length_squared();
                        if cuts.improves(score, best.cut_energy, best.score) && allowed(pose, &links) {
                            *best = Trial { score, angles: candidate, side: route, cut_energy: cuts };
                        }
                    };
                    let mut settled_cuts = CutEnergy::default();
                    let mut checked_crease = false;
                    for _ in 0..3 {
                        let fit =
                            self.evaluate(posed, &parents, &bones, target.0, side, SupportChoice::Nearest);
                        let cuts = self.cut_energy();
                        settled_cuts = cuts;
                        if cuts.total() == 0.0 {
                            break;
                        }
                        let alternate = if side_at_entry && active.iter().all(Option::is_none) {
                            let fit = self.evaluate(
                                posed,
                                &parents,
                                &bones,
                                target.0,
                                side,
                                SupportChoice::Opposite,
                            );
                            let change = fit.gram.inverse() * fit.rhs;
                            Some((limit(target.0.map(|a| a + change)), fit.side))
                        } else {
                            None
                        };
                        let change = fit.gram.inverse() * fit.rhs;
                        let same = limit(target.0.map(|a| a + change));
                        let continuing = (continuation != Vec3::ZERO)
                            .then(|| (limit(prior.map(|a| a + continuation)), fit.side));
                        let opening = (continuation != Vec3::ZERO)
                            .then(|| (limit(prior.map(|a| a - continuation)), fit.side));
                        let release_offset =
                            (target.0[0] - release_angles[0] + target.0[1] - release_angles[1]) * 0.5;
                        let mut best = Trial {
                            score: cuts.total() + 0.001 * release_offset.length_squared(),
                            angles: target,
                            side: fit.side,
                            cut_energy: cuts,
                        };
                        for (candidate, route) in
                            [Some((same, fit.side)), alternate, continuing, opening].into_iter().flatten()
                        {
                            score(self, candidate, route, &mut best);
                        }
                        // Once an entry route has a valid side, compare the other
                        // face of a shared crease within the same motion and tee limits.
                        if have_palm && side_at_entry && !checked_crease && active.iter().any(Option::is_some)
                        {
                            checked_crease = true;
                            let crease =
                                self.evaluate(posed, &parents, &bones, target.0, side, SupportChoice::Crease);
                            if crease.crease {
                                let change = crease.gram.inverse() * crease.rhs;
                                let candidate = limit(target.0.map(|a| a + change));
                                score(self, candidate, crease.side, &mut best);
                            }
                        }
                        settled_cuts = best.cut_energy;
                        if best.angles == target {
                            remember(&mut side, &mut active, best.side);
                            break;
                        }
                        target = best.angles;
                        remember(&mut side, &mut active, best.side);
                    }
                    if settled_cuts.total() > 0.0 && shaft != Vec3::ZERO && side_at_entry {
                        // Compare the extra entry route only after the current
                        // fit has finished. A promising first step can keep a
                        // better retained-side trajectory from converging.
                        let extra = target.0;
                        let step = (extra[0] - prior[0] + extra[1] - prior[1]) * 0.5;
                        let bend = step - shaft * step.dot(shaft);
                        if bend.length_squared() > 1.0e-12 {
                            let bend = bend.normalize() * (TERMINAL * dt).to_radians();
                            let offset = (extra[0] - release_angles[0] + extra[1] - release_angles[1]) * 0.5;
                            let mut best = Trial {
                                score: settled_cuts.total() + 0.001 * offset.length_squared(),
                                angles: target,
                                side: active,
                                cut_energy: settled_cuts,
                            };
                            let opposite_route = active.map(|normal| normal.map(|n| -n));
                            score(self, limit(prior.map(|a| a + bend)), active, &mut best);
                            score(self, limit(prior.map(|a| a - bend)), opposite_route, &mut best);
                            if best.cut_energy.total() < settled_cuts.total() {
                                target = best.angles;
                                active = best.side;
                            }
                        }
                    }
                }
                self.extra = target.0;
                self.final_angles = Some(target.1);
                self.side = active;
            }
            let links = place_links(&parents, [Vec3::ZERO, Vec3::ZERO, self.extra[0], self.extra[1]]);
            for (point, sample) in self.preceding.iter_mut().zip(posed) {
                *point = sample_position(*sample, &links).0;
            }
            self.previous_bones = bones;
            self.had_pose = true;
        }
        let mut changed = false;
        for (&joint, angle) in lock.joints[2..].iter().zip(self.extra) {
            if angle.length_squared() > 1.0e-12 {
                let local = &mut pose.locals_mut()[joint].rotation;
                *local = (Quat::from_scaled_axis(angle) * *local).normalize();
                changed = true;
            }
        }
        if changed {
            pose.finish();
        }
    }
}

fn remember(
    branch: &mut [Option<Vec3>; FINGERS],
    active: &mut [Option<Vec3>; FINGERS],
    found: [Option<Vec3>; FINGERS],
) {
    for ((branch, active), side) in branch.iter_mut().zip(active).zip(found) {
        if side.is_some() {
            *branch = side;
            *active = side;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finger_improvement_cannot_deepen_the_palm_contact() {
        let before = CutEnergy { fingers: 0.000_282_49, palm: 0.000_008_41 };
        let finger_fit = CutEnergy { fingers: 0.000_211_12, palm: 0.000_055_67 };
        let continuation = CutEnergy { fingers: 0.000_283_15, palm: 0.0 };
        // The exact alternate clip improves even the combined scalar with the finger
        // fit. Requiring the palm to improve independently prevents that transfer.
        assert!(finger_fit.total() < before.total());
        assert!(!finger_fit.improves(finger_fit.total(), before, before.total()));
        assert!(continuation.fingers > before.fingers);
        assert!(continuation.improves(continuation.total() + 0.000_001_45, before, before.total()));
    }

    #[test]
    fn current_face_score_measures_only_shared_transverse_interval() {
        let v = Vec3::new;
        let ribbon = Triangle::new([v(-1.0, -1.0, 0.0), v(1.0, -1.0, 0.0), v(0.0, 1.0, 0.0)]);
        let finger = Triangle::new([v(0.0, -0.5, -1.0), v(0.0, -0.5, 1.0), v(0.0, 0.5, 0.0)]);
        assert!((cut_length(ribbon, finger) - 1.0).abs() < 1.0e-5);
        assert!((cut_length(finger, ribbon) - 1.0).abs() < 1.0e-5);
        let apart = Triangle::new([v(0.0, 2.0, -1.0), v(0.0, 2.0, 1.0), v(0.0, 3.0, 0.0)]);
        assert_eq!(cut_length(ribbon, apart), 0.0);
        assert_eq!(cut_length(ribbon, ribbon), 0.0);
    }

    #[test]
    fn segment_support_can_be_strictly_inside_a_ribbon_face() {
        let triangle = Triangle::new([Vec3::ZERO, Vec3::X, Vec3::Y]);
        let bone = Bone { a: Vec3::new(0.25, 0.25, -1.0), b: Vec3::new(0.25, 0.25, 1.0) };
        let mut support =
            Support { triangle: usize::MAX, feature: usize::MAX, gap: f32::INFINITY, weights: Vec3::ZERO };
        closest(bone, triangle, 3, &mut support);
        assert_eq!(support.triangle, 3);
        assert_eq!(support.feature, 2);
        assert_eq!(support.gap, 0.0);
        assert_eq!(support.weights, Vec3::new(0.5, 0.25, 0.25));
    }
}
