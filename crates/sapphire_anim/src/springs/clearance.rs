//! Presentation contact for the heroine's long hair and deforming shirt.

mod residual;
mod underarm;

use residual::{Residual, short_face::ShortFace};
use sapphire_assets::mesh::Mesh;
use sapphire_assets::rig::{ArchivedCharacterRig, ArchivedColliderShape, HumanBone};
use sapphire_assets::skeleton::ArchivedSkeleton;
use sapphire_math::{Affine3A, Quat, Vec3};
use underarm::Underarm;

use super::{CharacterPose, Frame, Placed};
use crate::Bind;

const CELL: f32 = 0.01;
const GRID: usize = 128;
const HALF: f32 = GRID as f32 * CELL * 0.5;
const GARMENT_MARGIN: f32 = 0.012;
const RELEASE_HALF_LIFE: f32 = 4.0; // Presentation ticks.
const CLEAR: f32 = 0.0008;
const MAX_TURN_PER_TICK: f32 = 4.5;
const LOOKAHEAD_TICKS: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Influence {
    joint: usize,
    local: Vec3,
    weight: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Point {
    parts: [Influence; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MorphPoint {
    shirt_point: usize,
    target: usize,
    joint_offsets: [Vec3; 4],
}

impl Point {
    fn model(self, pose: &[Affine3A]) -> Vec3 {
        self.parts
            .iter()
            .filter(|p| p.weight > 0.0)
            .fold(Vec3::ZERO, |sum, p| sum + pose[p.joint].transform_point3(p.local) * p.weight)
    }

    /// Split skinning into the part rotated by the root and the part staying put.
    fn split(self, pose: &[Affine3A], chest_from_model: Affine3A, root: Vec3, joints: &[usize]) -> Probe {
        let mut probe = Probe::default();
        for p in self.parts.iter().filter(|p| p.weight > 0.0) {
            let point = chest_from_model.transform_point3(pose[p.joint].transform_point3(p.local));
            if joints.contains(&p.joint) {
                probe.rotating += (point - root) * p.weight;
                probe.fixed += root * p.weight;
            } else {
                probe.fixed += point * p.weight;
            }
        }
        probe
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Probe {
    fixed: Vec3,
    rotating: Vec3,
    radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Limb {
    joint: usize,
    offset: Vec3,
    tail: Vec3,
    radius: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Relief {
    direction: Vec3,
    rotation: Quat,
}

impl Relief {
    fn rotation(along: Vec3, direction: Vec3, degrees: f32) -> Quat {
        along
            .cross(direction)
            .try_normalize()
            .map_or(Quat::IDENTITY, |axis| Quat::from_axis_angle(axis, degrees.to_radians()))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Strand {
    joints: Vec<usize>,
    samples: Vec<Point>,
    side: f32,
    front: f32,
    shoulder_route: Option<usize>,
    relief: Relief,
    previous_probes: Vec<Probe>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Interval {
    lo: f32,
    hi: f32,
}

impl Interval {
    const EMPTY: Self = Self { lo: f32::INFINITY, hi: f32::NEG_INFINITY };

    fn add(&mut self, v: f32) {
        self.lo = self.lo.min(v);
        self.hi = self.hi.max(v);
    }
}

/// Orthographic depth ranges along the chest's local axes. A point must lie inside all
/// three projections to belong to the garment envelope.
#[derive(Clone, Debug, PartialEq)]
struct Garment {
    grids: [Vec<Interval>; 3],
    /// Per grid, the rows and columns the last refit wrote in, `None` if none: every other cell is
    /// empty, so a refit clears only these.
    written: [Option<Cells>; 3],
    min: Vec3,
    max: Vec3,
}

/// Columns `x` and rows `y` of a grid, inclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cells {
    x: (usize, usize),
    y: (usize, usize),
}

impl Cells {
    fn with(cells: Option<Self>, x: (usize, usize), y: (usize, usize)) -> Self {
        cells.map_or(Self { x, y }, |cells| Self {
            x: (cells.x.0.min(x.0), cells.x.1.max(x.1)),
            y: (cells.y.0.min(y.0), cells.y.1.max(y.1)),
        })
    }
}

impl Garment {
    fn new() -> Self {
        Self {
            grids: std::array::from_fn(|_| vec![Interval::EMPTY; GRID * GRID]),
            written: [None; 3],
            min: Vec3::ZERO,
            max: Vec3::ZERO,
        }
    }

    fn refit(&mut self, vertices: &[Vec3], triangles: &[[usize; 3]]) {
        self.min = vertices.iter().copied().fold(Vec3::splat(f32::INFINITY), Vec3::min);
        self.max = vertices.iter().copied().fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        for ((axis, grid), written) in self.grids.iter_mut().enumerate().zip(&mut self.written) {
            if let Some(cells) = written.take() {
                for y in cells.y.0..=cells.y.1 {
                    grid[y * GRID + cells.x.0..=y * GRID + cells.x.1].fill(Interval::EMPTY);
                }
            }
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            for triangle in triangles {
                let [a, b, c] = triangle.map(|i| vertices[i]);
                let min = a.min(b).min(c);
                let max = a.max(b).max(c);
                let index =
                    |x: f32| (((x + HALF) / CELL).floor() as isize).clamp(0, GRID as isize - 1) as usize;
                let coordinate = |i: usize| (i as f32 + 0.5) * CELL - HALF;
                let ab = b - a;
                let ac = c - a;
                let area = ab[u] * ac[v] - ab[v] * ac[u];
                if area.abs() < 1.0e-9 {
                    continue;
                }
                let inverse = area.recip();
                *written = Some(Cells::with(
                    *written,
                    (index(min[u]), index(max[u])),
                    (index(min[v]), index(max[v])),
                ));
                for y in index(min[v])..=index(max[v]) {
                    let dy = coordinate(y) - a[v];
                    for x in index(min[u])..=index(max[u]) {
                        let dx = coordinate(x) - a[u];
                        let s = (dx * ac[v] - dy * ac[u]) * inverse;
                        let t = (ab[u] * dy - ab[v] * dx) * inverse;
                        if s >= 0.0 && t >= 0.0 && s + t <= 1.0 {
                            grid[y * GRID + x].add(a[axis] + s * ab[axis] + t * ac[axis]);
                        }
                    }
                }
            }
        }
    }

    fn within(&self, point: Vec3, margin: f32) -> bool {
        let margin = Vec3::splat(margin);
        (self.min - point).cmple(margin).all() && (point - self.max).cmple(margin).all()
    }

    fn depth(&self, point: Vec3, radius: f32) -> f32 {
        if !self.within(point, GARMENT_MARGIN + radius) || point.abs().max_element() >= HALF - CELL {
            return 0.0;
        }
        let sample = (point + Vec3::splat(HALF)) / CELL - Vec3::splat(0.5);
        let mut depth = f32::INFINITY;
        for (axis, grid) in self.grids.iter().enumerate() {
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            let x = sample[u] as usize;
            let y = sample[v] as usize;
            let fx = sample[u] - x as f32;
            let fy = sample[v] - y as f32;
            let row = y * GRID + x;
            let at = |a: Interval| (point[axis] - a.lo).min(a.hi - point[axis]);
            // Fade the edge of the sampled silhouette over a cell so entry stays continuous.
            let interpolate = |a: f32, b: f32, weight: f32| a + (b - a) * weight;
            let at = |index: usize| at(grid[index]).clamp(-CELL * 2.0, 1.0);
            let top = interpolate(at(row), at(row + 1), fx);
            let bottom = interpolate(at(row + GRID), at(row + GRID + 1), fx);
            let distance = interpolate(top, bottom, fy);
            depth = depth.min(distance + GARMENT_MARGIN + radius);
            if depth <= 0.0 {
                return 0.0;
            }
        }
        depth
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Clearance {
    chest: usize,
    shirt: Vec<Point>,
    shirt_morphs: Vec<MorphPoint>,
    triangles: Vec<[usize; 3]>,
    limbs: Vec<Limb>,
    strands: Vec<Strand>,
    garment: Garment,
    ahead_garment: Garment,
    posed_shirt: Vec<Vec3>,
    previous_shirt: Vec<Vec3>,
    ahead_shirt: Vec<Vec3>,
    posed_limbs: Vec<Placed>,
    previous_limbs: Vec<Placed>,
    ahead_limbs: Vec<Placed>,
    probes: Vec<Probe>,
    ahead_probes: Vec<Probe>,
    has_history: bool,
    waking: bool,
    residual: Option<Residual>,
    short_face: Option<ShortFace>,
    underarm: Option<Underarm>,
}

impl Clearance {
    pub(super) fn new(mesh: &Mesh, skeleton: &ArchivedSkeleton, rig: &ArchivedCharacterRig) -> Option<Self> {
        let skin = mesh.skin.as_ref()?;
        if skin.joint_count as usize != skeleton.names.len()
            || skin.joint_count != rig.joint_count.to_native()
        {
            return None;
        }
        let binds = Bind::from_skin(&skin.binds);
        let point = |index: usize| {
            let position = Vec3::from_array(mesh.streams.vertex(index)?.position);
            let influence = skin.influences.get(index)?;
            Some((
                position,
                Point {
                    parts: std::array::from_fn(|i| {
                        let bind = binds[usize::from(influence.binds[i])];
                        Influence {
                            joint: usize::from(bind.joint),
                            local: bind.inverse_bind.transform_point3(position),
                            weight: f32::from(influence.weights[i]) / 255.0,
                        }
                    }),
                },
            ))
        };
        let indices = |material: &str| {
            mesh.lods.first()?.submeshes.iter().find_map(|part| {
                let slot = mesh.materials.get(part.material as usize)?;
                let start = part.first_index as usize;
                (slot.name == material).then(|| &mesh.indices[start..start + part.index_count as usize])
            })
        };
        let joint = |bone| {
            rig.humanoid
                .iter()
                .find(|entry| entry.bone == bone)
                .map(|entry| usize::from(entry.joint.to_native()))
        };
        let chest = joint(HumanBone::UpperChest)?;
        let mut shirt = Vec::new();
        let mut triangles = Vec::new();
        let mut mapped = vec![None; mesh.streams.len()];
        for face in indices("Tops_01_CLOTH")?.as_chunks::<3>().0 {
            let mut triangle = [0; 3];
            for (corner, &index) in triangle.iter_mut().zip(face) {
                let index = index as usize;
                *corner = if let Some(existing) = mapped[index] {
                    existing
                } else {
                    let next = shirt.len();
                    shirt.push(point(index)?.1);
                    mapped[index] = Some(next);
                    next
                };
            }
            triangles.push(triangle);
        }
        let mut shirt_morphs = Vec::new();
        for (target, shape) in mesh.morph_targets.iter().enumerate() {
            let start = shape.first_delta as usize;
            for delta in &mesh.morph_deltas[start..start + shape.delta_count as usize] {
                let index = delta.vertex() as usize;
                let offset = Vec3::from_array(delta.position());
                if let Some(shirt_point) = mapped[index].filter(|_| offset != Vec3::ZERO) {
                    let influence = skin.influences.get(index)?;
                    let joint_offsets = std::array::from_fn(|i| {
                        binds[usize::from(influence.binds[i])].inverse_bind.transform_vector3(offset)
                    });
                    shirt_morphs.push(MorphPoint { shirt_point, target, joint_offsets });
                }
            }
        }
        let underarm = Underarm::new(mesh, skeleton, &binds, &triangles, &mapped, point, &shirt_morphs);
        let mut hair = indices("Hair_00_HAIR")?.to_vec();
        hair.sort_unstable();
        hair.dedup();
        let points: Vec<_> = hair.into_iter().filter_map(|index| point(index as usize)).collect();
        let mut strands = Vec::new();
        for spring in rig.springs.iter().filter(|spring| spring.name.as_str() == "Hair") {
            let joints: Vec<_> = spring.joints.iter().map(|j| usize::from(j.joint.to_native())).collect();
            if joints.len() < 2 {
                return None;
            }
            let name = skeleton.names[joints[0]].as_str();
            let id: u8 = name.strip_prefix("J_Sec_Hair1_")?.parse().ok()?;
            let candidates: Vec<_> = points
                .iter()
                .copied()
                .filter(|(_, p)| {
                    p.parts.iter().filter(|p| joints.contains(&p.joint)).map(|p| p.weight).sum::<f32>() > 0.60
                })
                .collect();
            let side = if matches!(id, 3..=7 | 12) { 1.0 } else { -1.0 };
            let front = if matches!(id, 1..=4 | 11 | 12) { 1.0 } else { -1.0 };
            let shoulder_route = match id {
                5 => joint(HumanBone::LeftUpperArm),
                10 => joint(HumanBone::RightUpperArm),
                _ => None,
            };
            let samples = spread(&candidates);
            let probe_count = samples.len() + (joints.len() - 1) * 6;
            strands.push(Strand {
                joints,
                samples,
                side,
                front,
                shoulder_route,
                relief: Relief::default(),
                previous_probes: Vec::with_capacity(probe_count),
            });
        }
        if strands.len() != 12 || shirt.is_empty() || strands.iter().any(|strand| strand.samples.is_empty()) {
            return None;
        }
        let arm_joints = [
            HumanBone::LeftUpperArm,
            HumanBone::LeftLowerArm,
            HumanBone::LeftHand,
            HumanBone::RightUpperArm,
            HumanBone::RightLowerArm,
            HumanBone::RightHand,
        ]
        .map(joint);
        let limbs: Vec<_> = rig
            .colliders
            .iter()
            .filter_map(|collider| {
                let joint = usize::from(collider.joint.to_native());
                if !arm_joints.contains(&Some(joint)) {
                    return None;
                }
                let (offset, tail, radius) = match &collider.shape {
                    ArchivedColliderShape::Sphere { offset, radius } => {
                        let center = Vec3::from_array(offset.map(|v| v.to_native()));
                        (center, center, radius.to_native())
                    }
                    ArchivedColliderShape::Capsule { offset, tail, radius } => (
                        Vec3::from_array(offset.map(|v| v.to_native())),
                        Vec3::from_array(tail.map(|v| v.to_native())),
                        radius.to_native(),
                    ),
                };
                Some(Limb { joint, offset, tail, radius })
            })
            .collect();
        let residual = Residual::new(mesh, skeleton, &strands, &mapped, point);

        let short_face = {
            ShortFace::new(
                mesh,
                skeleton,
                &strands,
                point,
                |joint| {
                    binds.iter().find(|bind| usize::from(bind.joint) == joint).map(|bind| bind.inverse_bind)
                },
                |index, offset| {
                    let influence = skin.influences.get(index).expect("validated face skin point");
                    std::array::from_fn(|i| {
                        binds[usize::from(influence.binds[i])].inverse_bind.transform_vector3(offset)
                    })
                },
            )
        };
        let shirt_points = vec![Vec3::ZERO; shirt.len()];
        let placed = vec![Placed::default(); limbs.len()];
        let probe_count =
            strands.iter().map(|strand| strand.previous_probes.capacity()).max().unwrap_or_default();
        Some(Self {
            chest,
            shirt,
            shirt_morphs,
            triangles,
            limbs,
            strands,
            garment: Garment::new(),
            ahead_garment: Garment::new(),
            posed_shirt: shirt_points.clone(),
            previous_shirt: shirt_points.clone(),
            ahead_shirt: shirt_points,
            posed_limbs: placed.clone(),
            previous_limbs: placed.clone(),
            ahead_limbs: placed,
            probes: Vec::with_capacity(probe_count),
            ahead_probes: Vec::with_capacity(probe_count),
            has_history: false,
            waking: false,
            residual,
            short_face,
            underarm,
        })
    }

    pub(super) fn reset(&mut self) {
        if let Some(residual) = &mut self.residual {
            residual.reset();
        }
        if let Some(face) = &mut self.short_face {
            face.reset();
        }
        self.has_history = false;
        self.waking = false;
        for strand in &mut self.strands {
            strand.relief = Relief::default();
        }
    }

    pub(super) fn sleep(&mut self) {
        // The character may return before its owner clock has advanced. Keep the drawn
        // orientation and discard only the motion estimate, which has a gap now.
        self.has_history = false;
        self.waking = true;
        if let Some(residual) = &mut self.residual {
            residual.sleep();
        }
        if let Some(face) = &mut self.short_face {
            face.sleep();
        }
    }

    pub(super) fn settle(&mut self, pose: &mut CharacterPose, wake_elapsed: f32) {
        let elapsed = self.waking.then_some(wake_elapsed);
        self.waking = false;
        self.apply(pose, elapsed);
    }

    pub(super) fn continue_from(&mut self, old: &Self) {
        let same = self.chest == old.chest
            && self.shirt == old.shirt
            && self.shirt_morphs == old.shirt_morphs
            && self.triangles == old.triangles
            && self.limbs == old.limbs
            && self.strands.len() == old.strands.len()
            && self.strands.iter().zip(&old.strands).all(|(a, b)| {
                a.joints == b.joints && a.samples == b.samples && a.shoulder_route == b.shoulder_route
            });
        if !same {
            self.reset();
            self.waking = true;
            return;
        }
        if let (Some(residual), Some(old)) = (&mut self.residual, &old.residual) {
            residual.continue_from(old);
        }
        if let (Some(face), Some(old)) = (&mut self.short_face, &old.short_face) {
            face.continue_from(old);
        }
        self.has_history = old.has_history;
        self.waking = old.waking;
        self.previous_shirt.copy_from_slice(&old.previous_shirt);
        self.previous_limbs.copy_from_slice(&old.previous_limbs);
        for (new, old) in self.strands.iter_mut().zip(&old.strands) {
            new.relief = old.relief;
            new.previous_probes.clone_from(&old.previous_probes);
        }
    }

    pub(super) fn underarm_morph(&self) -> Option<(&'static str, usize)> {
        self.underarm.as_ref().map(Underarm::morph_slot)
    }

    pub(super) fn protected_write(&self, conflicts: impl Fn(usize) -> bool) -> Option<(&'static str, usize)> {
        self.strands
            .iter()
            .map(|strand| strand.joints[0])
            .find(|&joint| conflicts(joint))
            .map(|joint| ("garment relief", joint))
            .or_else(|| {
                self.residual
                    .as_ref()
                    .and_then(|residual| residual.protected_write(&conflicts))
                    .map(|joint| ("garment residual", joint))
            })
            .or_else(|| {
                self.short_face
                    .as_ref()?
                    .protected_write(conflicts)
                    .map(|joint| ("short face clearance", joint))
            })
    }

    pub(super) fn apply(&mut self, pose: &mut CharacterPose, elapsed: Option<f32>) {
        if let Some(underarm) = &self.underarm {
            underarm.clear(pose);
        }
        let forecast = elapsed
            .filter(|&dt| self.has_history && dt > 0.0 && dt < 4.0)
            .map_or(0.0, |dt| LOOKAHEAD_TICKS / dt.max(0.125) * ((4.0 - dt) * 0.5).min(1.0));
        let predict = |now: Vec3, before: Vec3| now + ((now - before) * forecast).clamp_length_max(0.07);
        let chest = Frame::of(&pose.model()[self.chest]);
        let chest_from_model = pose.model()[self.chest].inverse();
        for (out, &point) in self.posed_shirt.iter_mut().zip(&self.shirt) {
            *out = chest_from_model.transform_point3(point.model(pose.model()));
        }
        for morph in &self.shirt_morphs {
            let weight = pose.weights()[morph.target];
            if weight == 0.0 {
                continue;
            }
            let mut offset = Vec3::ZERO;
            for (part, local) in self.shirt[morph.shirt_point].parts.iter().zip(morph.joint_offsets) {
                if part.weight > 0.0 {
                    offset += pose.model()[part.joint].transform_vector3(local) * part.weight;
                }
            }
            self.posed_shirt[morph.shirt_point] += chest_from_model.transform_vector3(offset) * weight;
        }
        if let Some(underarm) = &mut self.underarm {
            underarm.solve(pose, chest_from_model, &self.shirt, &mut self.posed_shirt);
        }
        self.garment.refit(&self.posed_shirt, &self.triangles);
        if forecast > 0.0 {
            for ((ahead, &now), &before) in
                self.ahead_shirt.iter_mut().zip(&self.posed_shirt).zip(&self.previous_shirt)
            {
                *ahead = predict(now, before);
            }
            self.ahead_garment.refit(&self.ahead_shirt, &self.triangles);
        }
        for (out, limb) in self.posed_limbs.iter_mut().zip(&self.limbs) {
            let transform = chest_from_model * pose.model()[limb.joint];
            *out = Placed {
                start: transform.transform_point3(limb.offset),
                axis: transform.transform_vector3(limb.tail - limb.offset),
                radius: limb.radius * transform.matrix3.x_axis.length(),
            };
        }
        if forecast > 0.0 {
            for ((ahead, now), before) in
                self.ahead_limbs.iter_mut().zip(&self.posed_limbs).zip(&self.previous_limbs)
            {
                let start = predict(now.start, before.start);
                let end = predict(now.start + now.axis, before.start + before.axis);
                let axis =
                    (end - start).try_normalize().map_or(now.axis, |direction| direction * now.axis.length());
                *ahead = Placed { start, axis, radius: now.radius };
            }
        }
        let mut wrote = false;
        for strand in &mut self.strands {
            let joint = strand.joints[0];
            let pivot =
                |joint: usize| chest_from_model.transform_point3(pose.model()[joint].translation.into());
            let root = pivot(joint);
            let end = pivot(*strand.joints.last().expect("has a tail joint"));
            let Some(along) = (end - root).try_normalize() else { continue };
            self.probes.clear();
            self.probes.extend(
                strand.samples.iter().map(|p| p.split(pose.model(), chest_from_model, root, &strand.joints)),
            );
            for link in strand.joints.windows(2) {
                let a = pivot(link[0]);
                let b = pivot(link[1]);
                for sample in 1..=6 {
                    let at = a.lerp(b, sample as f32 / 6.0);
                    self.probes.push(Probe { fixed: root, rotating: at - root, radius: 0.002 });
                }
            }
            self.ahead_probes.clear();
            if forecast > 0.0 {
                self.ahead_probes.extend(self.probes.iter().zip(&strand.previous_probes).map(
                    |(now, before)| Probe {
                        fixed: predict(now.fixed, before.fixed),
                        rotating: predict(now.rotating, before.rotating),
                        radius: now.radius,
                    },
                ));
            }
            strand.previous_probes.clear();
            strand.previous_probes.extend_from_slice(&self.probes);
            let excursion = elapsed.map_or(0.55, |dt| (dt.min(2.0) * MAX_TURN_PER_TICK).to_radians());
            let filter = |probe: &Probe, garment: &Garment, limbs: &[Placed]| {
                let point = probe.fixed + strand.relief.rotation * probe.rotating;
                let margin = excursion * probe.rotating.length() + probe.radius + GARMENT_MARGIN;
                garment.within(point, margin)
                    || limbs.iter().any(|limb| {
                        point.distance_squared(limb.nearest(point)) <= (limb.radius + margin).powi(2)
                    })
            };
            self.probes.retain(|probe| filter(probe, &self.garment, &self.posed_limbs));
            self.ahead_probes.retain(|probe| filter(probe, &self.ahead_garment, &self.ahead_limbs));
            // Take the outer locks around the sleeve as it moves from back to front.
            // The forward edge of the upper arm is about 4 cm ahead of its pivot.
            let route = strand.shoulder_route.map(|arm| {
                let middle = pivot(strand.joints[strand.joints.len() / 2 - 1]);
                let fore = ((middle.z - pivot(arm).z - 0.04) / 0.06).clamp(-0.8, 0.8);
                Vec3::new(strand.side, 0.3, fore)
            });
            strand.relief =
                choose(strand.relief, elapsed, along, strand.side, strand.front, route, |rotation| {
                    let current = penetration(rotation, &self.garment, &self.posed_limbs, &self.probes);
                    let ahead =
                        penetration(rotation, &self.ahead_garment, &self.ahead_limbs, &self.ahead_probes);
                    (current, ahead)
                });
            if strand.relief.rotation == Quat::IDENTITY {
                continue;
            }
            let parent = pose.parent(joint).map_or(Quat::IDENTITY, |p| Frame::of(&pose.model()[p]).rotation);
            let chest_to_parent = parent.inverse() * chest.rotation;
            let offset = chest_to_parent * strand.relief.rotation * chest_to_parent.inverse();
            let local = &mut pose.locals_mut()[joint].rotation;
            *local = (offset * *local).normalize();
            wrote = true;
        }
        if wrote {
            pose.finish();
        }
        if let Some(residual) = &mut self.residual {
            residual.apply(
                pose,
                chest_from_model,
                &self.strands,
                &self.posed_shirt,
                &self.previous_shirt,
                elapsed,
            );
        }
        if let Some(face) = &mut self.short_face {
            face.apply(pose, &self.strands, elapsed);
        }
        self.has_history = true;
        self.previous_shirt.copy_from_slice(&self.posed_shirt);
        self.previous_limbs.copy_from_slice(&self.posed_limbs);
    }
}

/// Keep samples along the full strand, including its edges, without carrying thousands of
/// seam-duplicated vertices into the frame loop.
fn spread(candidates: &[(Vec3, Point)]) -> Vec<Point> {
    let mut result = Vec::new();
    let mut gaps = vec![f32::INFINITY; candidates.len()];
    let Some((mut next, _)) = candidates.iter().enumerate().min_by(|(_, a), (_, b)| a.0.y.total_cmp(&b.0.y))
    else {
        return result;
    };
    while result.len() < 100 {
        let (at, point) = candidates[next];
        result.push(point);
        for ((other, _), gap) in candidates.iter().zip(&mut gaps) {
            *gap = (*gap).min(at.distance_squared(*other));
        }
        let (farthest, &gap) =
            gaps.iter().enumerate().max_by(|(_, a), (_, b)| a.total_cmp(b)).expect("one point");
        if gap < 0.012_f32.powi(2) {
            break;
        }
        next = farthest;
    }
    result
}

fn penetration(rotation: Quat, garment: &Garment, limbs: &[Placed], probes: &[Probe]) -> f32 {
    if probes.is_empty() {
        return 0.0;
    }
    let mut worst = 0.0_f32;
    let mut sum = 0.0;
    for probe in probes {
        let point = probe.fixed + rotation * probe.rotating;
        let mut deep = garment.depth(point, probe.radius);
        for limb in limbs {
            let reach = limb.radius + probe.radius + 0.003;
            let delta = point - limb.nearest(point);
            if delta.length_squared() < reach * reach {
                deep = deep.max(reach - delta.length());
            }
        }
        worst = worst.max(deep);
        sum += deep * deep;
    }
    worst + (sum / probes.len() as f32).sqrt()
}

fn degrees_between(a: Quat, b: Quat) -> f32 {
    let relative = a.conjugate() * b;
    let sine = Vec3::new(relative.x, relative.y, relative.z).length();
    2.0 * sine.atan2(relative.w.abs()).to_degrees()
}

fn limited_turn(from: Quat, to: Quat, degrees: f32) -> Quat {
    let delta = degrees_between(from, to);
    if delta <= degrees { to } else { from.slerp(to, degrees / delta).normalize() }
}

fn choose(
    previous: Relief,
    elapsed: Option<f32>,
    along: Vec3,
    side: f32,
    front: f32,
    route: Option<Vec3>,
    depth: impl Fn(Quat) -> (f32, f32),
) -> Relief {
    if elapsed == Some(0.0) {
        return previous;
    }
    let limit = elapsed.map_or(28.0, |dt| dt.min(2.0) * MAX_TURN_PER_TICK);
    let fade = elapsed.map_or(0.0, |dt| (-dt / RELEASE_HALF_LIFE).exp2());
    let decayed = Quat::IDENTITY.slerp(previous.rotation, fade).normalize();
    let release = if degrees_between(decayed, Quat::IDENTITY) < 0.02 { Quat::IDENTITY } else { decayed };
    let decayed = limited_turn(previous.rotation, release, limit);
    let (present, ahead) = depth(decayed);
    let release_angle = degrees_between(decayed, Quat::IDENTITY);
    if present < CLEAR && ahead < 0.002 {
        if let Some(direction) = route
            && release_angle > 3.0
        {
            // Start following the free side while there is room. Waiting for another collision
            // to choose a direction can leave the sleeve between a strand and its escape path.
            let target = Relief::rotation(along, direction, release_angle);
            let rotation = limited_turn(previous.rotation, target, limit);
            let (now, ahead) = depth(rotation);
            if now < 0.003 && ahead < 0.003 {
                return Relief { direction, rotation };
            }
        }
        return Relief { rotation: decayed, ..previous };
    }
    let preferred = route.unwrap_or(Vec3::new(side * 0.65, 0.0, front));
    let tangent = (preferred - along * preferred.dot(along)).try_normalize();
    let lateral = 0.68 * (1.0 - along.x * along.x).max(0.0).sqrt();
    let cost = |q: Quat, present, ahead| {
        let route_cost = if route.is_some() {
            tangent.zip((q * along - along).try_normalize()).map_or(0.0, |(preferred, actual)| {
                14.0 * (1.0 - preferred.dot(actual)) + 82.0 * (lateral - side * actual.x).max(0.0)
            })
        } else {
            0.0
        };
        present * 2500.0
            + ahead * 700.0
            + degrees_between(q, Quat::IDENTITY)
            + degrees_between(q, previous.rotation) * 0.6
            + route_cost
    };
    let mut best = Relief { rotation: decayed, ..previous };
    let mut best_cost = cost(decayed, present, ahead);
    let (held_present, held_ahead) = depth(previous.rotation);
    let hold_cost = cost(previous.rotation, held_present, held_ahead);
    if hold_cost < best_cost {
        best = previous;
        best_cost = hold_cost;
    }
    let previous_angle = degrees_between(previous.rotation, Quat::IDENTITY);
    let wanted = (previous_angle + (present + ahead * 0.6) * 600.0 + 7.0).clamp(8.0, 28.0);
    let heading = if previous.direction == Vec3::ZERO { preferred } else { previous.direction };
    let opposite = Vec3::new(side, 0.0, -front);
    let directions = [heading, preferred, Vec3::Z * front, Vec3::X * side, opposite, Vec3::NEG_Z * front];
    let mut try_turn = |direction: Vec3, target| {
        let q = limited_turn(previous.rotation, target, limit);
        let (present, ahead) = depth(q);
        let bias = 1.6 * (1.0 - preferred.normalize().dot(direction.normalize()));
        let score = cost(q, present, ahead) + bias;
        if score < best_cost {
            best_cost = score;
            best = Relief { direction, rotation: q };
        }
    };
    for direction in directions {
        if elapsed.is_none() {
            // A cut can start with the strand already on the wrong side. Check intermediate
            // turns too, since the furthest turn may hit the other side of the sleeve.
            for degrees in [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 28.0] {
                try_turn(direction, Relief::rotation(along, direction, degrees));
            }
        } else {
            try_turn(direction, Relief::rotation(along, direction, wanted));
        }
    }
    if let Some(direction) = route
        && previous_angle > 3.0
    {
        try_turn(direction, Relief::rotation(along, direction, previous_angle));
    }
    // A change of contact side can follow the edge of the garment. Turning the old
    // correction's axis keeps its outward extent while changing the escape direction.
    if previous_angle > 2.0 {
        for sign in [-1.0, 1.0] {
            let around = Quat::from_axis_angle(along, (25.0_f32 * sign).to_radians());
            let direction = around * heading;
            try_turn(direction, around * previous.rotation * around.inverse());
        }
    }
    let strong = best;
    let release = Relief { rotation: decayed, ..previous };
    let mut fraction = 0.5;
    for _ in 0..3 {
        let candidate = release.rotation.slerp(strong.rotation, fraction).normalize();
        let (present, ahead) = depth(candidate);
        let score = cost(candidate, present, ahead);
        if score < best_cost {
            best_cost = score;
            best = Relief { rotation: candidate, ..strong };
            fraction *= 0.5;
        } else {
            fraction = (fraction + 1.0) * 0.5;
        }
    }
    best.rotation = limited_turn(previous.rotation, best.rotation, limit);
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Joint, JointTransform, Skeleton, SpringBones, SpringFrame};
    use sapphire_assets::rig::{CharacterRig, Spring, SpringJoint, SpringTip};

    #[test]
    fn garment_probe_sees_a_link_crossing_between_clear_ends() {
        let vertices = [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ]
        .map(|p| Vec3::from_array(p) * 0.06);
        let triangles = [
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
        ];
        let mut garment = Garment::new();
        garment.refit(&vertices, &triangles);
        let a = Vec3::NEG_X * 0.12;
        let b = Vec3::X * 0.12;
        assert_eq!(garment.depth(a, 0.0), 0.0);
        assert_eq!(garment.depth(b, 0.0), 0.0);
        assert!(garment.depth(a.lerp(b, 0.5), 0.0) > 0.05);
        assert_eq!(garment.depth(Vec3::Y * 0.12, 0.0), 0.0);
    }

    #[test]
    fn relief_uses_presented_time_and_releases_without_a_branch_pop() {
        let tip = Vec3::NEG_Y * 0.35;
        let forward = |q: Quat| ((0.060 - (q * tip).z).max(0.0), 0.0);
        let backward = |q: Quat| ((0.060 + (q * tip).z).max(0.0), 0.0);
        let mut relief = choose(Relief::default(), None, Vec3::NEG_Y, 1.0, 1.0, None, forward);
        assert!(forward(relief.rotation).0 < 0.004);
        assert_eq!(choose(relief, Some(0.0), Vec3::NEG_Y, 1.0, 1.0, None, backward), relief);
        for tick in 0..25 {
            let old = relief.rotation;
            relief = choose(relief, Some(0.25), Vec3::NEG_Y, 1.0, 1.0, None, backward);
            assert!(degrees_between(old, relief.rotation) < 1.14, "tick {tick}");
        }
        assert!(backward(relief.rotation).0 < 0.004);
        for _ in 0..48 {
            let before = degrees_between(relief.rotation, Quat::IDENTITY);
            relief = choose(relief, Some(1.0), Vec3::NEG_Y, 1.0, 1.0, None, |_| (0.0, 0.0));
            assert!(degrees_between(relief.rotation, Quat::IDENTITY) <= before);
        }
        assert_eq!(relief.rotation, Quat::IDENTITY);
    }

    #[test]
    fn a_clear_releasing_strand_turns_toward_the_outer_sleeve_route() {
        let along = Vec3::NEG_Y;
        let previous =
            Relief { direction: Vec3::NEG_Z, rotation: Relief::rotation(along, Vec3::NEG_Z, 18.0) };
        let route = Vec3::new(-1.0, 0.0, -0.5);
        let next = choose(previous, Some(1.0), along, -1.0, -1.0, Some(route), |_| (0.0, 0.0));
        assert!((next.rotation * along).x < -0.02);
        assert!(degrees_between(previous.rotation, next.rotation) <= 4.501);
        assert!(degrees_between(Quat::IDENTITY, next.rotation) < 18.0);
    }

    #[test]
    fn lod_wake_retains_drawn_relief_at_a_frozen_clock() {
        let joints = [
            Joint { name: "chest".into(), parent: None, rest: JointTransform::IDENTITY },
            Joint { name: "strand".into(), parent: Some(0), rest: JointTransform::IDENTITY },
            Joint {
                name: "end".into(),
                parent: Some(1),
                rest: JointTransform { translation: [0.0, -0.35, 0.0], ..JointTransform::IDENTITY },
            },
        ];
        let skeleton = Skeleton::new(&joints).to_cooked().unwrap();
        let mut pose = CharacterPose::new(Skeleton::read(&skeleton).unwrap(), &[], &[], Vec::new());
        let rig = CharacterRig {
            joint_count: 3,
            springs: vec![Spring {
                name: "strand".into(),
                joints: [1, 2]
                    .map(|joint| SpringJoint {
                        joint,
                        hit_radius: 0.0,
                        stiffness: 0.5,
                        gravity_power: 0.0,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.4,
                    })
                    .into(),
                tip: SpringTip::Joint,
                collider_groups: Vec::new(),
                center: None,
            }],
            ..CharacterRig::default()
        }
        .to_cooked()
        .unwrap();
        let mut springs = SpringBones::new(CharacterRig::read(&rig).unwrap(), &pose).unwrap();
        let mut frame =
            SpringFrame { world_from_model: Affine3A::IDENTITY, ticks: 8.0, active: true, pose_moved: false };
        springs.update(&mut pose, &frame);
        let mut clearance = Clearance {
            chest: 0,
            shirt: Vec::new(),
            shirt_morphs: Vec::new(),
            triangles: Vec::new(),
            limbs: Vec::new(),
            strands: vec![Strand {
                joints: vec![1, 2],
                samples: Vec::new(),
                side: 1.0,
                front: 1.0,
                shoulder_route: None,
                relief: Relief { direction: Vec3::Z, rotation: Quat::from_rotation_x(24.0_f32.to_radians()) },
                previous_probes: Vec::with_capacity(6),
            }],
            garment: Garment::new(),
            ahead_garment: Garment::new(),
            posed_shirt: Vec::new(),
            previous_shirt: Vec::new(),
            ahead_shirt: Vec::new(),
            posed_limbs: Vec::new(),
            previous_limbs: Vec::new(),
            ahead_limbs: Vec::new(),
            probes: Vec::with_capacity(6),
            ahead_probes: Vec::with_capacity(6),
            has_history: false,
            waking: false,
            residual: None,
            short_face: None,
            underarm: None,
        };
        clearance.apply(&mut pose, Some(0.0));
        springs.clearance = Some(Box::new(clearance));
        let drawn = pose.locals()[1].rotation;
        assert!(degrees_between(drawn, Quat::IDENTITY) > 23.9);

        springs.update(&mut pose, &SpringFrame { active: false, ..frame });
        springs.update(&mut pose, &frame);
        assert_eq!(pose.locals()[1].rotation, drawn);

        springs.update(&mut pose, &SpringFrame { active: false, ..frame });
        frame.ticks += 0.25;
        springs.update(&mut pose, &frame);
        let release = degrees_between(pose.locals()[1].rotation, drawn);
        assert!((0.5..=1.126).contains(&release), "released {release} degrees");

        springs.reset();
        springs.update(&mut pose, &frame);
        assert_eq!(pose.locals()[1].rotation, Quat::IDENTITY);
    }
}
