//! Bounded presentation residual for the heroine's two long front locks.
use sapphire_assets::{mesh::Mesh, skeleton::ArchivedSkeleton};

use super::{Affine3A, CharacterPose, Frame, Point, Quat, Strand, Vec3, degrees_between, limited_turn};

mod distal_hand;
pub(super) mod short_face;
use distal_hand::DistalHand;

const LINKS: usize = 4;
const DOFS: usize = LINKS * 3;
const SAMPLES: usize = 100;
const FIND_RADIUS: f32 = 0.025;
const MARGIN: f32 = 0.006;
const HYSTERESIS: f32 = 0.002;
const MAX_ANGLE: [f32; LINKS] = [4.0, 16.0, 12.0, 8.0];
const MAX_RATE: [f32; LINKS] = [4.5, 2.5, 2.5, 2.0];

#[derive(Clone, Copy, Debug, PartialEq)]
enum SurfaceVertex {
    Tee(usize),
    Skin(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SamplePart {
    joint: usize,
    affected: Option<usize>,
    local: Vec3,
    weight: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    source: [u32; 3],
    parts: [SamplePart; 12],
}
type Angles = [Vec3; LINKS];

#[derive(Clone, Debug, PartialEq)]
struct LockProfile {
    strand: usize,
    joints: [usize; LINKS],
    samples: Vec<Sample>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Patch {
    vertices: [SurfaceVertex; 3],
    rest_center: Vec3,
    source: [u32; 3],
    zone: Zone,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    lo: Vec3,
    hi: Vec3,
}

impl Bounds {
    const EMPTY: Self = Self { lo: Vec3::splat(f32::INFINITY), hi: Vec3::splat(f32::NEG_INFINITY) };

    fn with_point(self, point: Vec3) -> Self {
        Self { lo: self.lo.min(point), hi: self.hi.max(point) }
    }

    fn union(self, other: Self) -> Self {
        Self { lo: self.lo.min(other.lo), hi: self.hi.max(other.hi) }
    }

    fn distance_squared(self, point: Vec3) -> f32 {
        (self.lo - point).max(point - self.hi).max(Vec3::ZERO).length_squared()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Node {
    start: usize,
    end: usize,
    right: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Triangle {
    a: Vec3,
    e: Vec3,
    f: Vec3,
    normal: Vec3,
    ee: f32,
    ef: f32,
    ff: f32,
    inv_det: f32,
}

impl Triangle {
    const EMPTY: Self = Self {
        a: Vec3::ZERO,
        e: Vec3::ZERO,
        f: Vec3::ZERO,
        normal: Vec3::ZERO,
        ee: 0.0,
        ef: 0.0,
        ff: 0.0,
        inv_det: 0.0,
    };

    fn new([a, b, c]: [Vec3; 3]) -> Self {
        let e = b - a;
        let f = c - a;
        let ee = e.length_squared();
        let ef = e.dot(f);
        let ff = f.length_squared();
        let normal = e.cross(f).normalize_or_zero();
        let inv_det = (ee * ff - ef * ef).max(1.0e-14).recip();
        Self { a, e, f, normal, ee, ef, ff, inv_det }
    }

    fn bounds(self) -> Bounds {
        Bounds::EMPTY.with_point(self.a).with_point(self.a + self.e).with_point(self.a + self.f)
    }

    fn distance(self, point: Vec3) -> (f32, f32) {
        let delta = point - self.a;
        let plane = delta.dot(self.normal);
        let ed = delta.dot(self.e);
        let fd = delta.dot(self.f);
        let s = (ed * self.ff - fd * self.ef) * self.inv_det;
        let t = (fd * self.ee - ed * self.ef) * self.inv_det;
        let edge = |d: Vec3, e: Vec3, ee: f32| {
            (d - e * (d.dot(e) / ee.max(1.0e-14)).clamp(0.0, 1.0)).length_squared()
        };
        let distance = if self.normal != Vec3::ZERO && s >= 0.0 && t >= 0.0 && s + t <= 1.0 {
            plane * plane
        } else {
            edge(delta, self.e, self.ee).min(edge(delta, self.f, self.ff)).min(edge(
                delta - self.e,
                self.f - self.e,
                self.ff + self.ee - 2.0 * self.ef,
            ))
        };
        (distance, plane)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Field {
    triangles: Vec<Triangle>,
    bounds: Vec<Bounds>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Contact {
    normal: Vec3,
    depth: f32,
}

impl Field {
    fn new(patches: usize, nodes: usize) -> Self {
        Self { triangles: vec![Triangle::EMPTY; patches], bounds: vec![Bounds::EMPTY; nodes] }
    }

    fn refit(&mut self, nodes: &[Node], patches: &[Patch], mut position: impl FnMut(SurfaceVertex) -> Vec3) {
        for (out, patch) in self.triangles.iter_mut().zip(patches) {
            *out = Triangle::new(patch.vertices.map(&mut position));
        }
        for (i, node) in nodes.iter().enumerate().rev() {
            self.bounds[i] = if let Some(right) = node.right {
                self.bounds[i + 1].union(self.bounds[right])
            } else {
                self.triangles[node.start..node.end]
                    .iter()
                    .fold(Bounds::EMPTY, |bound, triangle| bound.union(triangle.bounds()))
            };
        }
    }

    fn search(&self, nodes: &[Node], i: usize, point: Vec3, best: &mut (f32, Option<usize>)) {
        if self.bounds[i].distance_squared(point) > best.0 {
            return;
        }
        let node = nodes[i];
        if let Some(right) = node.right {
            let left = i + 1;
            if self.bounds[left].distance_squared(point) <= self.bounds[right].distance_squared(point) {
                self.search(nodes, left, point, best);
                self.search(nodes, right, point, best);
            } else {
                self.search(nodes, right, point, best);
                self.search(nodes, left, point, best);
            }
        } else {
            for index in node.start..node.end {
                let distance = self.triangles[index].distance(point).0;
                if distance < best.0 || (distance == best.0 && best.1.is_some_and(|old| index < old)) {
                    *best = (distance, Some(index));
                }
            }
        }
    }

    fn contact(
        &self,
        nodes: &[Node],
        leg_root: Option<usize>,
        point: Vec3,
        remembered: Option<usize>,
    ) -> (Contact, Option<usize>) {
        let mut best = (FIND_RADIUS * FIND_RADIUS, None);
        self.search(nodes, 0, point, &mut best);
        if let Some(root) = leg_root {
            self.search(nodes, root, point, &mut best);
        }
        let nearest = remembered
            .filter(|&i| self.triangles[i].distance(point).0.sqrt() <= best.0.sqrt() + HYSTERESIS)
            .or(best.1);
        let Some(i) = nearest else { return (Contact::default(), None) };
        let triangle = self.triangles[i];
        let (distance, signed) = triangle.distance(point);
        let distance = distance.sqrt();
        if distance >= FIND_RADIUS || distance - signed.abs() >= 0.004 || triangle.normal == Vec3::ZERO {
            return (Contact::default(), None);
        }
        (Contact { normal: triangle.normal, depth: (MARGIN - signed).clamp(0.0, 0.020) }, Some(i))
    }
}

fn layout(patches: &mut [Patch], start: usize, nodes: &mut Vec<Node>) {
    let end = start + patches.len();
    let at = nodes.len();
    nodes.push(Node { start, end, right: None });
    if patches.len() <= 6 {
        return;
    }
    let bound = patches.iter().fold(Bounds::EMPTY, |b, p| b.with_point(p.rest_center));
    let extent = bound.hi - bound.lo;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    patches.sort_unstable_by(|a, b| {
        a.rest_center[axis].total_cmp(&b.rest_center[axis]).then(a.source.cmp(&b.source))
    });
    let middle = patches.len() / 2;
    let (left, right) = patches.split_at_mut(middle);
    layout(left, start, nodes);
    nodes[at].right = Some(nodes.len());
    layout(right, start + middle, nodes);
}

#[derive(Clone, Debug, PartialEq)]
struct Profile {
    patches: Vec<Patch>,
    nodes: Vec<Node>,
    leg_root: Option<usize>,
    skin: Vec<Point>,
    locks: Vec<LockProfile>,
}

#[derive(Clone, Debug, PartialEq)]
struct State {
    angles: Angles,
    total_root: Option<Quat>,
    previous: Vec<Vec3>,
    patch: Vec<Option<usize>>,
}

impl State {
    fn new(samples: usize) -> Self {
        Self {
            angles: [Vec3::ZERO; LINKS],
            total_root: None,
            previous: vec![Vec3::ZERO; samples],
            patch: vec![None; samples],
        }
    }

    fn reset(&mut self) {
        self.angles = [Vec3::ZERO; LINKS];
        self.total_root = None;
        self.patch.fill(None);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Residual {
    profile: Profile,
    locks: Vec<State>,
    fields: [Field; 2],
    skin: Vec<Vec3>,
    previous_skin: Vec<Vec3>,
    history: bool,
    distal: Option<DistalHand>,
}

fn spread<T: Copy>(candidates: &[(Vec3, T)], limit: usize) -> Vec<T> {
    let Some((mut next, _)) = candidates.iter().enumerate().min_by(|(_, a), (_, b)| a.0.y.total_cmp(&b.0.y))
    else {
        return Vec::new();
    };
    let mut result = Vec::with_capacity(limit.min(candidates.len()));
    let mut distances = vec![f32::INFINITY; candidates.len()];
    for _ in 0..limit.min(candidates.len()) {
        let (center, value) = candidates[next];
        result.push(value);
        for (&(position, _), gap) in candidates.iter().zip(&mut distances) {
            *gap = gap.min(center.distance_squared(position));
        }
        distances[next] = -1.0;
        next = distances.iter().enumerate().reduce(|a, b| if a.1 >= b.1 { a } else { b }).unwrap().0;
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Tee,
    Arm,
    LeftLeg,
    RightLeg,
}

impl Zone {
    fn accepts(self, center: Vec3, points: [Point; 3], skeleton: &ArchivedSkeleton) -> bool {
        let parts = points.iter().flat_map(|p| p.parts).filter(|p| p.weight > 0.0);
        match self {
            Self::Tee => center.y >= 0.86,
            Self::Arm => {
                center.y >= 0.86
                    && parts.clone().any(|part| {
                        let name = skeleton.names[part.joint].as_str();
                        (name.contains("Arm") || name.contains("Shoulder") || name.contains("Hand"))
                            && parts.clone().filter(|p| p.joint == part.joint).map(|p| p.weight).sum::<f32>()
                                > 0.25
                    })
            }
            Self::LeftLeg | Self::RightLeg => {
                // The heroine's rest-space medial thigh and upper shin. A folded
                // knee carries these two skin strips into reach during recovery.
                let names = if self == Self::LeftLeg {
                    ["J_Bip_L_UpperLeg", "J_Bip_L_LowerLeg"]
                } else {
                    ["J_Bip_R_UpperLeg", "J_Bip_R_LowerLeg"]
                };
                (0.33..=0.63).contains(&center.y)
                    && center.x.abs() <= 0.08
                    && center.z.abs() <= 0.085
                    && parts
                        .filter(|p| names.contains(&skeleton.names[p.joint].as_str()))
                        .map(|p| p.weight)
                        .sum::<f32>()
                        > 1.5
            }
        }
    }
}

impl Residual {
    pub(super) fn new(
        mesh: &Mesh,
        skeleton: &ArchivedSkeleton,
        strands: &[Strand],
        tee_map: &[Option<usize>],
        vertex: impl Fn(usize) -> Option<(Vec3, Point)>,
    ) -> Option<Self> {
        let lod = mesh.lods.first()?;
        let triangles = |material: &'static str| {
            lod.submeshes
                .iter()
                .filter(move |p| mesh.materials[p.material as usize].name == material)
                .flat_map(|p| {
                    let start = p.first_index as usize;
                    let stop = start + p.index_count as usize;
                    mesh.indices[start..stop].as_chunks::<3>().0.iter().copied()
                })
        };
        let mut skin = Vec::new();
        let mut skin_map = vec![None; mesh.streams.len()];
        let mut patches = Vec::new();
        for (material, limit, zone) in [
            ("Tops_01_CLOTH", 720, Zone::Tee),
            ("Body_00_SKIN", 240, Zone::Arm),
            ("Body_00_SKIN", 48, Zone::LeftLeg),
            ("Body_00_SKIN", 48, Zone::RightLeg),
        ] {
            let mut candidates = Vec::new();
            for face in triangles(material) {
                let [Some(a), Some(b), Some(c)] = face.map(|i| vertex(i as usize)) else { continue };
                let center = (a.0 + b.0 + c.0) / 3.0;
                if zone.accepts(center, [a.1, b.1, c.1], skeleton) {
                    candidates.push((center, face));
                }
            }
            if candidates.is_empty() {
                return None;
            }
            for face in spread(&candidates, limit) {
                let mut out = [SurfaceVertex::Tee(0); 3];
                let mut center = Vec3::ZERO;
                for (&id, slot) in face.iter().zip(&mut out) {
                    let id = id as usize;
                    let (rest, point) = vertex(id)?;
                    center += rest / 3.0;
                    *slot = if zone == Zone::Tee {
                        SurfaceVertex::Tee(tee_map[id]?)
                    } else {
                        SurfaceVertex::Skin(*skin_map[id].get_or_insert_with(|| {
                            let i = skin.len();
                            skin.push(point);
                            i
                        }))
                    };
                }
                patches.push(Patch { vertices: out, rest_center: center, source: face, zone });
            }
        }
        let mut locks = Vec::new();
        let hair_faces: Vec<_> = triangles("Hair_00_HAIR").collect();
        let mut hair_ids: Vec<_> = hair_faces.iter().flatten().copied().collect();
        hair_ids.sort_unstable();
        hair_ids.dedup();
        for (strand_index, strand) in strands.iter().enumerate() {
            let name = skeleton.names[strand.joints[0]].as_str();
            if !matches!(name, "J_Sec_Hair1_11" | "J_Sec_Hair1_12") {
                continue;
            }
            let joints: [usize; LINKS] = strand.joints.get(..LINKS)?.try_into().ok()?;
            let within = |p: Point| {
                p.parts.iter().filter(|p| strand.joints.contains(&p.joint)).map(|p| p.weight).sum::<f32>()
                    >= 0.60
            };
            let mut candidates = Vec::new();
            for &id in &hair_ids {
                let (rest, point) = vertex(id as usize)?;
                if within(point) {
                    candidates.push((rest, ([id; 3], [point; 3])));
                }
            }
            for ids in &hair_faces {
                let [Some(a), Some(b), Some(c)] = ids.map(|id| vertex(id as usize)) else { continue };
                if [a.1, b.1, c.1].into_iter().all(within) {
                    candidates.push(((a.0 + b.0 + c.0) / 3.0, (*ids, [a.1, b.1, c.1])));
                }
            }
            let samples = spread(&candidates, SAMPLES)
                .into_iter()
                .map(|(source, points)| {
                    let parts = std::array::from_fn(|i| {
                        let p = points[i / 4].parts[i % 4];
                        SamplePart {
                            joint: p.joint,
                            affected: strand
                                .joints
                                .iter()
                                .position(|&j| j == p.joint)
                                .map(|j| j.min(LINKS - 1)),
                            local: p.local,
                            weight: p.weight / 3.0,
                        }
                    });
                    Sample { source, parts }
                })
                .collect();
            locks.push(LockProfile { strand: strand_index, joints, samples });
        }
        // This profile may use all current tee morphs. If a new revision adds a position
        // morph to one of these bare-arm or hair inputs, the fit needs a corresponding profile.
        let unsupported_morph = mesh.morph_deltas.iter().any(|delta| {
            let vertex = delta.vertex();
            (skin_map[vertex as usize].is_some() || hair_ids.binary_search(&vertex).is_ok())
                && Vec3::from_array(delta.position()) != Vec3::ZERO
        });
        if unsupported_morph
            || patches.is_empty()
            || locks.len() != 2
            || locks.iter().any(|lock| lock.samples.is_empty())
        {
            return None;
        }
        let distal = locks
            .iter()
            .find(|lock| skeleton.names[lock.joints[0]].as_str() == "J_Sec_Hair1_11")
            .and_then(|lock| {
                DistalHand::new(
                    mesh,
                    skeleton,
                    lock,
                    &strands[lock.strand],
                    &hair_faces,
                    triangles("Body_00_SKIN"),
                    &vertex,
                )
            });
        let primary_count = patches.iter().position(|p| p.zone == Zone::LeftLeg)?;
        let mut nodes = Vec::new();
        let (primary, legs) = patches.split_at_mut(primary_count);
        layout(primary, 0, &mut nodes);
        let leg_root = Some(nodes.len());
        layout(legs, primary_count, &mut nodes);
        let fields = std::array::from_fn(|_| Field::new(patches.len(), nodes.len()));
        let states = locks.iter().map(|lock| State::new(lock.samples.len())).collect();
        let posed = vec![Vec3::ZERO; skin.len()];
        Some(Self {
            profile: Profile { patches, nodes, leg_root, skin, locks },
            locks: states,
            fields,
            skin: posed.clone(),
            previous_skin: posed,
            history: false,
            distal,
        })
    }

    pub(super) fn reset(&mut self) {
        if let Some(distal) = &mut self.distal {
            distal.reset();
        }
        self.history = false;
        for state in &mut self.locks {
            state.reset();
        }
    }

    pub(super) fn sleep(&mut self) {
        if let Some(distal) = &mut self.distal {
            distal.sleep();
        }
        self.history = false;
        for state in &mut self.locks {
            state.patch.fill(None);
        }
    }

    pub(super) fn continue_from(&mut self, old: &Self) {
        if self.profile == old.profile {
            self.history = old.history;
            self.previous_skin.copy_from_slice(&old.previous_skin);
            self.locks.clone_from(&old.locks);
            if let (Some(distal), Some(before)) = (&mut self.distal, &old.distal) {
                distal.continue_from(before);
            }
        } else {
            self.reset();
        }
    }
}

#[derive(Clone, Copy)]
struct WeightedPoint {
    position: Vec3,
    affected: Option<usize>,
    weight: f32,
}

type DrawSample = [WeightedPoint; 12];

fn posed_sample(sample: &Sample, model: &[Affine3A], chest: Affine3A) -> DrawSample {
    sample.parts.map(|p| WeightedPoint {
        position: if p.weight == 0.0 {
            Vec3::ZERO
        } else {
            chest.transform_point3(model[p.joint].transform_point3(p.local))
        },
        affected: p.affected,
        weight: p.weight,
    })
}

#[derive(Clone, Copy)]
struct Link {
    pivot: Vec3,
    parent: Quat,
}

impl Link {
    fn current(joint: usize, pose: &CharacterPose, chest: Affine3A) -> Self {
        let pivot = chest.transform_point3(pose.model()[joint].translation.into());
        let parent =
            pose.parent(joint).map_or(Quat::IDENTITY, |p| Frame::of(&(chest * pose.model()[p])).rotation);
        Self { pivot, parent }
    }
}

#[derive(Clone, Copy)]
struct PlacedLink {
    pivot: Vec3,
    axes: [Vec3; 3],
    transform: Affine3A,
}

fn place_links(links: &[Link; LINKS], angles: Angles) -> [PlacedLink; LINKS] {
    let mut transform = Affine3A::IDENTITY;
    let mut carried = Quat::IDENTITY;
    std::array::from_fn(|i| {
        let parent = carried * links[i].parent;
        let pivot = transform.transform_point3(links[i].pivot);
        let angle = angles[i];
        let q = parent * Quat::from_scaled_axis(angle) * parent.inverse();
        let radians = angle.length();
        let (a, b) = if radians < 0.001 {
            (0.5, 1.0 / 6.0)
        } else {
            ((1.0 - radians.cos()) / radians.powi(2), (radians - radians.sin()) / radians.powi(3))
        };
        let axes = [Vec3::X, Vec3::Y, Vec3::Z].map(|axis| {
            let cross = angle.cross(axis);
            parent * (axis + a * cross + b * angle.cross(cross))
        });
        transform = Affine3A::from_rotation_translation(q, pivot - q * pivot) * transform;
        carried = q * carried;
        PlacedLink { pivot, axes, transform }
    })
}

fn sample_position(sample: DrawSample, links: &[PlacedLink; LINKS]) -> (Vec3, [Vec3; LINKS]) {
    let mut position = Vec3::ZERO;
    let mut lever = [Vec3::ZERO; LINKS];
    for part in sample {
        if part.weight == 0.0 {
            continue;
        }
        if let Some(last) = part.affected {
            let at = links[last].transform.transform_point3(part.position);
            position += at * part.weight;
            for i in 0..=last {
                lever[i] += (at - links[i].pivot) * part.weight;
            }
        } else {
            position += part.position * part.weight;
        }
    }
    (position, lever)
}

fn base_position(sample: DrawSample) -> Vec3 {
    sample.iter().fold(Vec3::ZERO, |sum, part| sum + part.position * part.weight)
}

fn jacobian(contact: Contact, lever: [Vec3; LINKS], links: &[PlacedLink; LINKS]) -> [f32; DOFS] {
    std::array::from_fn(|i| {
        let link = i / 3;
        contact.normal.dot(links[link].axes[i % 3].cross(lever[link]))
    })
}

fn predict(current: Vec3, previous: Vec3, horizon: f32) -> Vec3 {
    current + ((current - previous) * horizon).clamp(Vec3::splat(-0.07), Vec3::splat(0.07))
}

fn total_root(parent: Quat, angles: Angles, p8_root: Quat) -> Quat {
    (parent * Quat::from_scaled_axis(angles[0]) * parent.inverse() * p8_root).normalize()
}

fn project(
    mut target: Angles,
    previous: Angles,
    previous_root: Quat,
    parent: Quat,
    p8_root: Quat,
    dt: f32,
) -> Angles {
    for i in 0..LINKS {
        target[i] = target[i].clamp_length_max(MAX_ANGLE[i].to_radians());
        target[i] = previous[i] + (target[i] - previous[i]).clamp_length_max((MAX_RATE[i] * dt).to_radians());
    }
    let step = MAX_RATE[0] * dt;
    let candidate = limited_turn(previous_root, total_root(parent, target, p8_root), step);
    let feasible = |root| {
        degrees_between(root, p8_root) <= MAX_ANGLE[0] + 0.0001
            && degrees_between(root, Quat::IDENTITY) <= 28.0001
    };
    let root = if feasible(candidate) {
        candidate
    } else {
        // P8 already obeys the same rate and global extent. Moving from the
        // previous combined correction toward P8 gives a feasible anchor.
        let anchor = limited_turn(previous_root, p8_root, step);
        let (mut best, mut invalid) = (anchor, candidate);
        for _ in 0..6 {
            let middle = best.slerp(invalid, 0.5).normalize();
            if feasible(middle) {
                best = middle;
            } else {
                invalid = middle;
            }
        }
        best
    };
    target[0] = (parent.inverse() * root * p8_root.inverse() * parent).normalize().to_scaled_axis();
    target
}

#[derive(Clone, Copy, Default)]
struct Constraint {
    row: [f32; DOFS],
    depth: f32,
}

struct Fit {
    here: [Constraint; SAMPLES],
    ahead: [Constraint; SAMPLES],
    counts: [usize; 2],
}

impl Fit {
    fn new() -> Self {
        Self {
            here: [Constraint::default(); SAMPLES],
            ahead: [Constraint::default(); SAMPLES],
            counts: [0; 2],
        }
    }

    fn push(&mut self, ahead: bool, constraint: Constraint) {
        let i = usize::from(ahead);
        let list = if ahead { &mut self.ahead } else { &mut self.here };
        list[self.counts[i]] = constraint;
        self.counts[i] += 1;
    }

    fn solve(&self, old: Angles, target: Angles) -> Option<Angles> {
        if self.counts == [0; 2] {
            return None;
        }
        let mut matrix = [[0.0; DOFS]; DOFS];
        let mut rhs = [0.0; DOFS];
        for i in 0..DOFS {
            let stiff = 0.08_f32.powi(2);
            matrix[i][i] = stiff;
            rhs[i] = stiff * 0.65 * (old[i / 3][i % 3] - target[i / 3][i % 3]);
        }
        for (rows, count, importance) in
            [(&self.here, self.counts[0], 1.0_f32), (&self.ahead, self.counts[1], 0.4)]
        {
            let weight = importance.powi(2) / (count as f32 / 8.0).max(1.0);
            for c in &rows[..count] {
                for i in 0..DOFS {
                    let weighted = c.row[i] * weight;
                    rhs[i] += weighted * c.depth;
                    for (entry, &component) in matrix[i][..=i].iter_mut().zip(&c.row[..=i]) {
                        *entry += weighted * component;
                    }
                }
            }
        }
        let delta = cholesky(matrix, rhs)?;
        Some(std::array::from_fn(|i| Vec3::new(delta[3 * i], delta[3 * i + 1], delta[3 * i + 2])))
    }
}

fn cholesky(mut lower: [[f32; DOFS]; DOFS], mut rhs: [f32; DOFS]) -> Option<[f32; DOFS]> {
    for i in 0..DOFS {
        for j in 0..=i {
            let value = lower[i][j] - (0..j).map(|k| lower[i][k] * lower[j][k]).sum::<f32>();
            if i == j {
                if !value.is_finite() || value <= 0.0 {
                    return None;
                }
                lower[i][j] = value.sqrt();
            } else {
                lower[i][j] = value / lower[j][j];
            }
        }
        rhs[i] = (rhs[i] - (0..i).map(|j| lower[i][j] * rhs[j]).sum::<f32>()) / lower[i][i];
    }
    for i in (0..DOFS).rev() {
        rhs[i] = (rhs[i] - (i + 1..DOFS).map(|j| lower[j][i] * rhs[j]).sum::<f32>()) / lower[i][i];
    }
    Some(rhs)
}

impl Residual {
    pub(super) fn protected_write(&self, conflicts: impl Fn(usize) -> bool) -> Option<usize> {
        self.profile.locks.iter().flat_map(|lock| lock.joints).find(|&joint| conflicts(joint))
    }

    pub(super) fn apply(
        &mut self,
        pose: &mut CharacterPose,
        chest: Affine3A,
        strands: &[Strand],
        tee: &[Vec3],
        previous_tee: &[Vec3],
        elapsed: Option<f32>,
    ) {
        let dt = elapsed.unwrap_or(0.0).clamp(0.0, 2.0);
        let horizon = elapsed
            .filter(|&dt| self.history && dt > 0.0 && dt < 4.0)
            .map_or(0.0, |dt| 2.0 / dt.max(0.125) * ((4.0 - dt) * 0.5).min(1.0));
        for (at, point) in self.skin.iter_mut().zip(&self.profile.skin) {
            *at = chest.transform_point3(point.model(pose.model()));
        }
        let [here, ahead] = &mut self.fields;
        let profile = &self.profile;
        if dt > 0.0 {
            here.refit(&profile.nodes, &profile.patches, |vertex| match vertex {
                SurfaceVertex::Tee(i) => tee[i],
                SurfaceVertex::Skin(i) => self.skin[i],
            });
            if horizon > 0.0 {
                ahead.refit(&profile.nodes, &profile.patches, |vertex| match vertex {
                    SurfaceVertex::Tee(i) => predict(tee[i], previous_tee[i], horizon),
                    SurfaceVertex::Skin(i) => predict(self.skin[i], self.previous_skin[i], horizon),
                });
            }
        }
        let mut wrote = false;
        for (lock, state) in profile.locks.iter().zip(&mut self.locks) {
            let p8 = strands[lock.strand].relief.rotation;
            let parents = lock.joints.map(|joint| Link::current(joint, pose, chest));
            let prev_root = state.total_root.unwrap_or(p8);
            // Keep all weighted constituent points until after link placement. In particular,
            // averaging a triangle first would lose its blend across two different link pivots.
            let samples: [DrawSample; SAMPLES] = std::array::from_fn(|i| {
                lock.samples.get(i).map_or(
                    [WeightedPoint { position: Vec3::ZERO, affected: None, weight: 0.0 }; 12],
                    |sample| posed_sample(sample, pose.model(), chest),
                )
            });
            let mut target = state.angles;
            if dt > 0.0 {
                let release = (-dt / 4.0).exp2();
                target = state.angles.map(|angle| angle * release);
                for _ in 0..3 {
                    let links = place_links(&parents, target);
                    let mut fit = Fit::new();
                    for (i, &sample) in samples.iter().take(lock.samples.len()).enumerate() {
                        let (position, lever) = sample_position(sample, &links);
                        let (contact, patch) =
                            here.contact(&profile.nodes, profile.leg_root, position, state.patch[i]);
                        state.patch[i] = patch;
                        if contact.depth > 0.0 {
                            fit.push(
                                false,
                                Constraint { row: jacobian(contact, lever, &links), depth: contact.depth },
                            );
                        }
                        if horizon > 0.0 {
                            let base = base_position(sample);
                            let predicted = position + (predict(base, state.previous[i], horizon) - base);
                            let (contact, _) =
                                ahead.contact(&profile.nodes, profile.leg_root, predicted, state.patch[i]);
                            if contact.depth > 0.0 {
                                fit.push(
                                    true,
                                    Constraint {
                                        row: jacobian(contact, lever, &links),
                                        depth: contact.depth,
                                    },
                                );
                            }
                        }
                    }
                    let Some(delta) = fit.solve(state.angles, target) else { break };
                    target = project(
                        std::array::from_fn(|i| target[i] + delta[i] * 0.7),
                        state.angles,
                        prev_root,
                        parents[0].parent,
                        p8,
                        dt,
                    );
                }
            }
            state.angles = project(target, state.angles, prev_root, parents[0].parent, p8, dt);
            state.total_root = Some(total_root(parents[0].parent, state.angles, p8));
            if dt > 0.0 || elapsed.is_none() || !self.history {
                for (&sample, prior) in samples.iter().zip(&mut state.previous) {
                    *prior = base_position(sample);
                }
            }
            for (&joint, angle) in lock.joints.iter().zip(state.angles) {
                if angle.length_squared() <= 1.0e-12 {
                    continue;
                }
                let local = &mut pose.locals_mut()[joint].rotation;
                *local = (Quat::from_scaled_axis(angle) * *local).normalize();
                wrote = true;
            }
        }
        if dt > 0.0 || elapsed.is_none() || !self.history {
            self.previous_skin.copy_from_slice(&self.skin);
        }
        self.history = true;
        if wrote {
            pose.finish();
        }
        if let Some(distal) = &mut self.distal
            && let Some(index) = profile.locks.iter().position(|lock| lock.strand == distal.strand)
        {
            let lock = &profile.locks[index];
            distal.apply(pose, chest, lock, self.locks[index].angles, dt, |pose, links| {
                // The hand response may use only clearance the incumbent fit leaves
                // available on its current tee and bare-skin patches.
                lock.samples.iter().all(|sample| {
                    let posed = posed_sample(sample, pose.model(), chest);
                    let (before, _) =
                        here.contact(&profile.nodes, profile.leg_root, base_position(posed), None);
                    let after = sample_position(posed, links).0;
                    let (after, _) = here.contact(&profile.nodes, profile.leg_root, after, None);
                    after.depth <= before.depth + 0.001
                })
            });
        }
    }
}

#[cfg(test)]
mod tests;
