//! Current-pose cheek clearance for the heroine's three short front locks.
use super::{Field, Node, Patch, Point, SurfaceVertex, Zone, layout, spread};
use crate::springs::clearance::{
    Affine3A, CharacterPose, Frame, MorphPoint, Probe, Quat, Strand, Vec3, limited_turn,
};
use sapphire_assets::{mesh::Mesh, skeleton::ArchivedSkeleton};
use sapphire_math::Mat3;

const MAX_PATCHES: usize = 768;
const SAMPLES: usize = 112;
const GAP: f32 = 0.002;
const MAX_TURN: f32 = 16.0;
const RATE: f32 = 4.0;
/// The face's locks: the strands either side of it and the one over it.
pub(in crate::springs::clearance) const LOCKS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    points: [Point; 3],
    rest: Vec3,
    allowance: f32,
    head_bind: Vec3,
    exposed: f32,
}

#[derive(Clone, Debug, PartialEq)]
struct Lock {
    strand: usize,
    root: usize,
    samples: Vec<Sample>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::springs::clearance) struct ShortFace {
    head: usize,
    skin: Vec<Point>,
    morphs: Vec<MorphPoint>,
    patches: Vec<Patch>,
    nodes: Vec<Node>,
    field: Field,
    locks: Vec<Lock>,
    drawn: [Quat; LOCKS],
    remembered: [[Option<usize>; SAMPLES]; LOCKS],
    posed: Vec<Vec3>,
}

impl ShortFace {
    pub(in crate::springs::clearance) fn new(
        mesh: &Mesh,
        skeleton: &ArchivedSkeleton,
        strands: &[Strand],
        vertex: impl Fn(usize) -> Option<(Vec3, Point)>,
        inverse_bind: impl Fn(usize) -> Option<Affine3A>,
        offsets: impl Fn(usize, Vec3) -> [Vec3; 4],
    ) -> Option<Self> {
        let lod = mesh.lods.first()?;
        let triangles = |name| {
            lod.submeshes
                .iter()
                .filter(move |part| mesh.materials[part.material as usize].name == name)
                .flat_map(|part| {
                    let start = part.first_index as usize;
                    let stop = start + part.index_count as usize;
                    mesh.indices[start..stop].as_chunks::<3>().0.iter().copied()
                })
        };
        let head = skeleton.names.iter().position(|name| name.as_str() == "J_Bip_C_Head")?;
        let from_rest = inverse_bind(head)?;
        let hair: Vec<_> = triangles("Hair_00_HAIR").collect();
        let mut locks = Vec::new();
        let mut support = Vec::new();
        for (strand, chain) in strands.iter().enumerate() {
            let root = chain.joints[0];
            if !matches!(
                skeleton.names[root].as_str(),
                "J_Sec_Hair1_02" | "J_Sec_Hair1_03" | "J_Sec_Hair1_04"
            ) {
                continue;
            }
            let pivot: Vec3 = inverse_bind(root)?.inverse().translation.into();
            let owned = |point: Point| {
                point
                    .parts
                    .iter()
                    .filter(|part| chain.joints.contains(&part.joint))
                    .map(|part| part.weight)
                    .sum::<f32>()
                    >= 0.60
            };
            let mut candidates = Vec::new();
            let mut seen = vec![false; mesh.streams.len()];
            for triangle in &hair {
                let [Some(a), Some(b), Some(c)] = triangle.map(|id| vertex(id as usize)) else { continue };
                for (id, (rest, point)) in triangle.iter().zip([a, b, c]) {
                    if !seen[*id as usize] && owned(point) {
                        seen[*id as usize] = true;
                        candidates.push((rest, [point; 3]));
                        support.push(rest);
                    }
                }
                if [a.1, b.1, c.1].into_iter().all(owned) {
                    candidates.push(((a.0 + b.0 + c.0) / 3.0, [a.1, b.1, c.1]));
                }
            }
            let samples = spread(
                &candidates
                    .iter()
                    .map(|&(rest, points)| {
                        let exposed = feather(rest.distance(pivot), 0.018, 0.036);
                        (
                            rest,
                            Sample {
                                points,
                                rest,
                                allowance: GAP,
                                head_bind: from_rest.transform_point3(rest),
                                exposed,
                            },
                        )
                    })
                    .collect::<Vec<_>>(),
                SAMPLES,
            );
            if samples.is_empty() {
                return None;
            }
            locks.push(Lock { strand, root, samples });
        }
        if locks.len() != LOCKS {
            return None;
        }

        let mut skin = Vec::new();
        let mut rest_skin = Vec::new();
        let mut mapped = vec![None; mesh.streams.len()];
        let mut patches = Vec::new();
        for source in triangles("Face_00_SKIN") {
            let [Some(a), Some(b), Some(c)] = source.map(|id| vertex(id as usize)) else { continue };
            let center = (a.0 + b.0 + c.0) / 3.0;
            // Cooked heroine rest-model coordinates. Exclude the rear scalp, and retain
            // the contiguous, outward-facing cheek around the actual ribbon footprint.
            if !(1.41..=1.55).contains(&center.y)
                || center.z < 0.023
                || (b.0 - a.0).cross(c.0 - a.0).z <= 0.0
                || !support.iter().any(|position| position.distance_squared(center) < 0.025 * 0.025)
            {
                continue;
            }
            let vertices = std::array::from_fn(|i| {
                let id = source[i] as usize;
                let (rest, point) = [a, b, c][i];
                SurfaceVertex::Skin(*mapped[id].get_or_insert_with(|| {
                    let slot = skin.len();
                    skin.push(point);
                    rest_skin.push(rest);
                    slot
                }))
            });
            patches.push(Patch { vertices, rest_center: center, source, zone: Zone::Arm });
        }
        if patches.is_empty() || patches.len() > MAX_PATCHES {
            return None;
        }
        let mut morphs = Vec::new();
        for (target, shape) in mesh.morph_targets.iter().enumerate() {
            let start = shape.first_delta as usize;
            for delta in &mesh.morph_deltas[start..start + shape.delta_count as usize] {
                let id = delta.vertex() as usize;
                let offset = Vec3::from_array(delta.position());
                if let Some(shirt_point) = mapped[id].filter(|_| offset != Vec3::ZERO) {
                    morphs.push(MorphPoint { shirt_point, target, joint_offsets: offsets(id, offset) });
                }
            }
        }
        let mut nodes = Vec::new();
        layout(&mut patches, 0, &mut nodes);
        let mut field = Field::new(patches.len(), nodes.len());
        field.refit(&nodes, &patches, |slot| rest_skin[skin_index(slot)]);
        for lock in &mut locks {
            for sample in &mut lock.samples {
                // Leave intentional rest crossings and near-attachment contact in place.
                let (contact, _) = field.contact(&nodes, None, sample.rest, None);
                sample.allowance = (super::MARGIN - contact.depth - 0.001).min(GAP);
            }
        }
        Some(Self {
            head,
            morphs,
            patches,
            nodes,
            field,
            locks,
            drawn: [Quat::IDENTITY; LOCKS],
            remembered: [[None; SAMPLES]; LOCKS],
            posed: vec![Vec3::ZERO; skin.len()],
            skin,
        })
    }

    pub(in crate::springs::clearance) fn reset(&mut self) {
        self.drawn = [Quat::IDENTITY; LOCKS];
        self.remembered = [[None; SAMPLES]; LOCKS];
    }

    pub(in crate::springs::clearance) fn sleep(&mut self) {
        self.remembered = [[None; SAMPLES]; LOCKS];
    }

    pub(in crate::springs::clearance) fn continue_from(&mut self, old: &Self) {
        if self.head == old.head
            && self.skin == old.skin
            && self.morphs == old.morphs
            && self.patches == old.patches
            && self.locks == old.locks
        {
            self.drawn = old.drawn;
            self.remembered = old.remembered;
        } else {
            self.reset();
        }
    }

    pub(in crate::springs::clearance) fn protected_write(
        &self,
        conflicts: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        self.locks.iter().map(|lock| lock.root).find(|&joint| conflicts(joint))
    }

    /// Solves each lock over its `elapsed` ticks, `None` or none holding it as it was drawn, and writes
    /// them all. The locks read only the pose, so each may solve on a frame of its own.
    pub(in crate::springs::clearance) fn apply(
        &mut self,
        pose: &mut CharacterPose,
        strands: &[Strand],
        elapsed: [Option<f32>; LOCKS],
    ) {
        let dts = elapsed.map(|elapsed| elapsed.unwrap_or(0.0).clamp(0.0, 2.0));
        let model = pose.model();
        let head = Frame::of(&model[self.head]).rotation;
        let from_model = model[self.head].inverse();
        if dts.iter().any(|&dt| dt > 0.0) {
            for (out, &point) in self.posed.iter_mut().zip(&self.skin) {
                *out = from_model.transform_point3(point.model(model));
            }
            for morph in &self.morphs {
                let weight = pose.weights()[morph.target];
                if weight == 0.0 {
                    continue;
                }
                let mut delta = Vec3::ZERO;
                for (part, local) in self.skin[morph.shirt_point].parts.iter().zip(morph.joint_offsets) {
                    if part.weight > 0.0 {
                        delta += model[part.joint].transform_vector3(local) * part.weight;
                    }
                }
                self.posed[morph.shirt_point] += from_model.transform_vector3(delta) * weight;
            }
            self.field.refit(&self.nodes, &self.patches, |slot| self.posed[skin_index(slot)]);
        }
        let mut offsets = [Quat::IDENTITY; LOCKS];
        for (i, (lock, dt)) in self.locks.iter().zip(dts).enumerate() {
            let previous = self.drawn[i];
            let mut target = previous;
            if dt > 0.0 {
                target =
                    limited_turn(previous, Quat::IDENTITY.slerp(previous, (-dt / 4.0).exp2()), RATE * dt);
                if target.xyz().length_squared() < 1.0e-8 {
                    target = Quat::IDENTITY;
                }
                let pivot = from_model.transform_point3(model[lock.root].translation.into());
                let joints = &strands[lock.strand].joints;
                let mut probes = [Probe::default(); SAMPLES];
                let mut allowances = [0.0; SAMPLES];
                for ((out, allowance), sample) in probes.iter_mut().zip(&mut allowances).zip(&lock.samples) {
                    for point in sample.points {
                        let probe = point.split(model, from_model, pivot, joints);
                        out.fixed += probe.fixed / 3.0;
                        out.rotating += probe.rotating / 3.0;
                    }
                    // Test the uncorrected spring pose in the current head frame.
                    // Using the iterated correction here would change the target
                    // as it is solved and could hold the ribbon away after contact.
                    let departure = (out.fixed + out.rotating).distance(sample.head_bind);
                    *allowance = moving_allowance(sample.allowance, sample.exposed, departure);
                }
                for _ in 0..3 {
                    let mut gram = Mat3::from_diagonal(Vec3::splat(0.0015));
                    let mut rhs = Vec3::ZERO;
                    for (j, (probe, &allowance)) in
                        probes.iter().zip(&allowances).take(lock.samples.len()).enumerate()
                    {
                        let lever = target * probe.rotating;
                        let (contact, index) =
                            self.field.contact(&self.nodes, None, probe.fixed + lever, self.remembered[i][j]);
                        self.remembered[i][j] = index;
                        let depth = (contact.depth - super::MARGIN + allowance).max(0.0);
                        if depth > 0.0 {
                            let row = lever.cross(contact.normal);
                            gram += Mat3::from_cols(row * row.x, row * row.y, row * row.z);
                            rhs += row * depth;
                        }
                    }
                    let change = (gram.inverse() * rhs).clamp_length_max(0.12);
                    if change.length_squared() < 1.0e-9 {
                        break;
                    }
                    let desired = limited_turn(
                        Quat::IDENTITY,
                        (Quat::from_scaled_axis(change) * target).normalize(),
                        MAX_TURN,
                    );
                    target = limited_turn(previous, desired, RATE * dt);
                }
            }
            self.drawn[i] = target;
            if target == Quat::IDENTITY {
                continue;
            }
            let parent =
                pose.parent(lock.root).map_or(Quat::IDENTITY, |parent| Frame::of(&model[parent]).rotation);
            let basis = parent.inverse() * head;
            offsets[i] = basis * target * basis.inverse();
        }
        let mut wrote = false;
        for (lock, offset) in self.locks.iter().zip(offsets) {
            if offset != Quat::IDENTITY {
                let local = &mut pose.locals_mut()[lock.root].rotation;
                *local = (offset * *local).normalize();
                wrote = true;
            }
        }
        if wrote {
            pose.finish();
        }
    }
}

fn skin_index(vertex: SurfaceVertex) -> usize {
    match vertex {
        SurfaceVertex::Skin(index) => index,
        SurfaceVertex::Tee(_) => unreachable!(),
    }
}

fn feather(value: f32, low: f32, high: f32) -> f32 {
    let x = ((value - low) / (high - low)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn moving_allowance(bind: f32, exposed: f32, departure: f32) -> f32 {
    // Only the displaced, exposed fringe loses its raw-bind overlap allowance.
    bind + (GAP - bind) * exposed * feather(departure, 0.0025, 0.006)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bind_allowance_only_opens_for_moving_exposed_hair() {
        let bind = -0.003;
        assert_eq!(moving_allowance(bind, 0.0, 0.015), bind);
        assert_eq!(moving_allowance(bind, 1.0, 0.002), bind);
        assert!(moving_allowance(bind, 1.0, 0.005) > 0.0);
        assert!((moving_allowance(bind, 1.0, 0.008) - GAP).abs() < f32::EPSILON);
        assert!(moving_allowance(bind, 0.5, 0.005) < 0.0);
    }
    use crate::springs::clearance::{Influence, Relief};
    use crate::{Joint, JointTransform, Skeleton};

    fn fixture() -> (ShortFace, CharacterPose, [Strand; 3]) {
        let joints: [Joint; 4] = std::array::from_fn(|i| Joint {
            name: format!("joint-{i}"),
            parent: (i > 0).then_some(0),
            rest: JointTransform::IDENTITY,
        });
        let skeleton = Skeleton::new(&joints).to_cooked().unwrap();
        let pose = CharacterPose::new(Skeleton::read(&skeleton).unwrap(), &[], &[], vec![0.0]);
        let point = |joint, local| {
            let mut parts = [Influence::default(); 4];
            parts[0] = Influence { joint, local, weight: 1.0 };
            Point { parts }
        };
        let skin = [Vec3::new(-0.2, -0.2, 0.012), Vec3::new(0.2, -0.2, 0.012), Vec3::new(0.0, 0.2, 0.012)]
            .map(|v| point(0, v))
            .to_vec();
        let locks = (1..=3)
            .map(|root| Lock {
                strand: root - 1,
                root,
                samples: vec![Sample {
                    points: [point(root, Vec3::new(0.02, -0.08, 0.0)); 3],
                    rest: Vec3::new(0.02, -0.08, 0.0),
                    allowance: GAP,
                    head_bind: Vec3::new(0.02, -0.08, 0.0),
                    exposed: 1.0,
                }],
            })
            .collect();
        let short = ShortFace {
            head: 0,
            skin,
            morphs: vec![MorphPoint {
                shirt_point: 0,
                target: 0,
                joint_offsets: [Vec3::new(0.0, 0.0, -0.04); 4],
            }],
            patches: vec![Patch {
                vertices: [0, 1, 2].map(SurfaceVertex::Skin),
                rest_center: Vec3::ZERO,
                source: [0, 1, 2],
                zone: Zone::Arm,
            }],
            nodes: vec![Node { start: 0, end: 1, right: None }],
            field: Field::new(1, 1),
            locks,
            drawn: [Quat::IDENTITY; LOCKS],
            remembered: [[None; SAMPLES]; LOCKS],
            posed: vec![Vec3::ZERO; 3],
        };
        let strands = std::array::from_fn(|i| Strand {
            joints: vec![i + 1],
            samples: Vec::new(),
            side: 1.0,
            front: 1.0,
            shoulder_route: None,
            relief: Relief::default(),
            previous_probes: Vec::new(),
        });
        (short, pose, strands)
    }

    fn degrees(a: Quat, b: Quat) -> f32 {
        2.0 * a.dot(b).abs().clamp(0.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn full_apply_retains_contact_through_frozen_sleep_and_head_transport() {
        let (mut face, base, strands) = fixture();
        let mut pose = base.clone();
        face.apply(&mut pose, &strands, [None; LOCKS]);
        assert_eq!(face.drawn, [Quat::IDENTITY; LOCKS]);

        face.apply(&mut pose, &strands, [Some(1.0); LOCKS]);
        let contact = face.drawn;
        assert!(degrees(contact[0], Quat::IDENTITY) > 1.0);
        assert!(contact.into_iter().all(|q| degrees(q, Quat::IDENTITY) <= RATE + 0.001));
        let first = pose.locals()[1].rotation;

        face.sleep();
        let mut frozen = base.clone();
        face.apply(&mut frozen, &strands, [Some(0.0); LOCKS]);
        assert_eq!(face.drawn, contact);
        assert_eq!(frozen.locals()[1].rotation, first);

        let mut transported = base.clone();
        transported.locals_mut()[0].rotation = Quat::from_rotation_y(0.8);
        transported.finish();
        face.apply(&mut transported, &strands, [Some(0.0); LOCKS]);
        assert_eq!(face.drawn, contact);
        assert!(degrees(transported.locals()[1].rotation, first) < 0.05);

        let mut reload = fixture().0;
        reload.continue_from(&face);
        assert_eq!(reload.drawn, face.drawn);
        assert_eq!(reload.remembered, face.remembered);

        face.skin.iter_mut().for_each(|p| {
            p.parts[0].local.z -= 1.0;
        });
        let mut released = base.clone();
        face.apply(&mut released, &strands, [Some(0.25); LOCKS]);
        assert!(
            face.drawn
                .into_iter()
                .zip(contact)
                .all(|(now, before)| degrees(now, before) <= RATE * 0.25 + 0.001)
        );
        assert!(degrees(face.drawn[0], Quat::IDENTITY) < degrees(contact[0], Quat::IDENTITY));

        face.reset();
        let mut held = base.clone();
        face.apply(&mut held, &strands, [None; LOCKS]);
        assert_eq!(face.drawn, [Quat::IDENTITY; LOCKS]);
        assert_eq!(held.locals()[1].rotation, Quat::IDENTITY);
    }

    #[test]
    fn the_surface_query_uses_current_morph_weights_and_refuses_incompatible_history() {
        let (mut face, mut pose, strands) = fixture();
        pose.weights_mut()[0] = 0.5;
        face.apply(&mut pose, &strands, [Some(1.0); LOCKS]);
        assert!((face.posed[0].z + 0.008).abs() < 1.0e-6);
        let old = face.clone();
        face.skin[0].parts[0].local.x += 0.001;
        face.continue_from(&old);
        assert_eq!(face.drawn, [Quat::IDENTITY; LOCKS]);
    }
}
