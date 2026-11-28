#![allow(clippy::print_stdout, reason = "the swing's measurements are worth reading with --nocapture")]

use std::collections::BTreeMap;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};

use sapphire_anim::{CharacterPose, Clip, HeroineClearance, Skeleton, SpringBones, SpringFrame};
use sapphire_assets::AssetId;
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::{
    CharacterRig, Collider, ColliderShape, HumanBone, HumanoidJoint, Spring, SpringJoint, SpringTip,
};
use sapphire_assets::skeleton::{Joint, JointTransform};
use sapphire_math::{Affine3A, Vec3};

use super::*;
use crate::clip::retarget::HumanMap;
use crate::clip::{ClipOptions, ClipTarget};
use crate::cook::{self, Cook};
use crate::import::vrm::{self, VrmSettings};
use crate::mesh::{ImportedMesh, SkinWeights};
use crate::meta::{self, ImportSettings};

const MOBILE_140: JointCap = JointCap { desktop: None, mobile: NonZeroU16::new(140) };

fn mobile(cap: u16) -> JointCap {
    JointCap { desktop: None, mobile: NonZeroU16::new(cap) }
}

fn joint(name: &str, parent: Option<u16>, translation: [f32; 3]) -> Joint {
    Joint { name: name.to_owned(), parent, rest: JointTransform { translation, ..JointTransform::IDENTITY } }
}

fn spring(name: &str, joints: &[u16]) -> Spring {
    Spring {
        name: name.to_owned(),
        joints: joints
            .iter()
            .map(|&joint| SpringJoint {
                joint,
                hit_radius: 0.02,
                stiffness: 1.0 + f32::from(joint),
                gravity_power: 0.0,
                gravity_dir: [0.0, -1.0, 0.0],
                drag_force: 0.4,
            })
            .collect(),
        tip: SpringTip::Joint,
        collider_groups: Vec::new(),
        center: None,
    }
}

/// Hips and a head, humanoid; a strand of four on the head, a bang of two, and a skirt of three on the
/// hips with a socket (no spring's joint) under its middle; a collider on the head.
///
/// ```text
/// 0 hips ─┬─ 1 head ─┬─ 2 hair_0 ─ 3 hair_1 ─ 4 hair_2 ─ 5 hair_3
///         │          └─ 6 bang_0 ─ 7 bang_1
///         └─ 8 skirt_0 ─ 9 skirt_1 ─┬─ 10 skirt_2
///                                   └─ 11 socket
/// ```
fn character() -> (Vec<Joint>, CharacterRig) {
    let joints = vec![
        joint("hips", None, [0.0, 1.0, 0.0]),
        joint("head", Some(0), [0.0, 0.6, 0.0]),
        joint("hair_0", Some(1), [0.0, 0.1, -0.1]),
        joint("hair_1", Some(2), [0.0, -0.1, -0.02]),
        joint("hair_2", Some(3), [0.0, -0.1, -0.01]),
        joint("hair_3", Some(4), [0.0, -0.1, 0.0]),
        joint("bang_0", Some(1), [0.0, 0.1, 0.1]),
        joint("bang_1", Some(6), [0.0, -0.05, 0.02]),
        joint("skirt_0", Some(0), [0.0, -0.1, 0.12]),
        joint("skirt_1", Some(8), [0.0, -0.15, 0.03]),
        joint("skirt_2", Some(9), [0.0, -0.15, 0.03]),
        joint("socket", Some(9), [0.05, 0.0, 0.0]),
    ];
    let rig = CharacterRig {
        joint_count: 12,
        humanoid: vec![
            HumanoidJoint { bone: HumanBone::Hips, joint: 0 },
            HumanoidJoint { bone: HumanBone::Head, joint: 1 },
        ],
        springs: vec![spring("hair", &[2, 3, 4, 5]), spring("bang", &[6, 7]), spring("skirt", &[8, 9, 10])],
        colliders: vec![Collider {
            joint: 1,
            shape: ColliderShape::Sphere { offset: [0.0; 3], radius: 0.1 },
        }],
        ..CharacterRig::default()
    };
    (joints, rig)
}

fn names(cut: &Cut, joints: &[Joint]) -> Vec<String> {
    cut.dropped(joints).into_iter().map(str::to_owned).collect()
}

#[test]
fn rounds_take_every_chains_tail_before_any_second_joint_and_never_a_root_or_a_holder() {
    let (joints, rig) = character();
    let dropped = |cap: u16| names(&plan(&joints, &rig, cap, Cook::Mobile).unwrap().unwrap(), &joints);
    // The furthest along their chains first: the strand's fourth, the skirt's third, the bang's second.
    assert_eq!(dropped(11), ["hair_3"]);
    assert_eq!(dropped(10), ["hair_3", "skirt_2"]);
    assert_eq!(dropped(9), ["hair_3", "bang_1", "skirt_2"]);
    // Then the second round: the skirt's middle holds the socket, so the strand's third, then its second.
    assert_eq!(dropped(8), ["hair_2", "hair_3", "bang_1", "skirt_2"]);
    assert_eq!(dropped(7), ["hair_1", "hair_2", "hair_3", "bang_1", "skirt_2"]);
    // A skeleton within its cap, or a cook with none, is left alone.
    assert_eq!(plan(&joints, &rig, 12, Cook::Mobile).unwrap(), None);
    let (mut same, mut same_rig) = (joints.clone(), rig.clone());
    assert_eq!(cut(&mut same, None, &mut same_rig, mobile(7), Cook::Desktop).unwrap(), None);
    assert_eq!((same, same_rig), (joints, rig));
}

#[test]
fn a_cap_past_what_the_chains_can_give_is_refused_by_its_field() {
    let (mut joints, mut rig) = character();
    let error = cut(&mut joints, None, &mut rig, mobile(6), Cook::Mobile).unwrap_err();
    assert_eq!(error, CutError::Unreachable { cook: Cook::Mobile, cap: 6, joints: 12, fewest: 7 });
    let message = error.to_string();
    assert!(
        message.starts_with("`joints.mobile` is 6") && message.contains("12 joints down to 7"),
        "{message}"
    );
    // Humanoid, collider, constraint and center joints are held: with the bang's tail a collider's, the
    // bang keeps it.
    let (joints, mut rig) = character();
    rig.colliders
        .push(Collider { joint: 7, shape: ColliderShape::Sphere { offset: [0.0; 3], radius: 0.01 } });
    let dropped = names(&plan(&joints, &rig, 9, Cook::Mobile).unwrap().unwrap(), &joints);
    assert_eq!(dropped, ["hair_2", "hair_3", "skirt_2"]);
}

#[test]
fn a_cut_renumbers_the_rig_and_ends_each_cut_chain_where_its_tail_stood() {
    let (mut joints, mut rig) = character();
    let source = joints.clone();
    let made = cut(&mut joints, None, &mut rig, mobile(8), Cook::Mobile).unwrap().unwrap();
    let kept: Vec<_> = joints.iter().map(|joint| joint.name.as_str()).collect();
    assert_eq!(kept, ["hips", "head", "hair_0", "hair_1", "bang_0", "skirt_0", "skirt_1", "socket"]);
    let parents: Vec<_> = joints.iter().map(|joint| joint.parent).collect();
    assert_eq!(parents, [None, Some(0), Some(1), Some(2), Some(1), Some(0), Some(5), Some(6)]);
    assert_eq!(made.into, [0, 1, 2, 3, 3, 3, 6, 6, 8, 9, 9, 11]);
    assert_eq!(rig.joint_count, 8);
    assert_eq!(rig.colliders[0].joint, 1);
    // Each chain keeps its joints' own settings, its root's among them, and swings toward the point its
    // first dropped joint rested at: one bone down, that bone's own translation, exactly.
    let [hair, bang, skirt] = &rig.springs[..] else { panic!("three chains") };
    assert_eq!(
        hair.joints.iter().map(|entry| (entry.joint, entry.stiffness)).collect::<Vec<_>>(),
        [(2, 3.0), (3, 4.0)]
    );
    assert_eq!(hair.tip, SpringTip::Point { offset: source[4].rest.translation });
    assert_eq!(bang.joints.iter().map(|entry| entry.joint).collect::<Vec<_>>(), [4]);
    assert_eq!(bang.tip, SpringTip::Point { offset: source[7].rest.translation });
    assert_eq!(skirt.joints.iter().map(|entry| entry.joint).collect::<Vec<_>>(), [5, 6]);
    assert_eq!(skirt.tip, SpringTip::Point { offset: source[10].rest.translation });
    // The humanoid here is two bones, short of what a rig must hold, so the shipped characters' test reads
    // their cut rigs back.
}

/// The model transform of every joint at rest, as matrices.
fn rest_model(joints: &[Joint]) -> Vec<Matrix> {
    let mut model: Vec<Matrix> = Vec::with_capacity(joints.len());
    for joint in joints {
        let own = local(&joint.rest);
        let world = joint.parent.map_or(own, |parent| multiply(&model[usize::from(parent)], &own));
        model.push(world);
    }
    model
}

fn inverse(matrix: &Matrix) -> Matrix {
    let inverse = sapphire_math::det::mat4_inverse(&sapphire_math::Mat4::from_cols_array_2d(matrix));
    inverse.to_cols_array_2d()
}

fn near(a: &Matrix, b: &Matrix) -> bool {
    a.iter().flatten().zip(b.iter().flatten()).all(|(a, b)| (a - b).abs() < 1e-5)
}

#[test]
fn weights_fold_into_the_joint_above_and_still_sum_to_one() {
    let (mut joints, mut rig) = character();
    // Turned joints, so a carried inverse bind is more than a translation.
    joints[3].rest.rotation = [0.0, 0.0, 0.258_819, 0.965_926];
    joints[9].rest.rotation = [0.130_526, 0.0, 0.0, 0.991_445];
    let model = rest_model(&joints);
    // A palette entry on every joint but the strand's second and the skirt's middle, whose vertices bind
    // their children.
    let on: Vec<u16> = (0..12).filter(|joint| ![3, 9].contains(joint)).collect();
    let binds: Vec<SkinBind> = on
        .iter()
        .map(|&joint| SkinBind { joint, inverse_bind_matrix: inverse(&model[usize::from(joint)]) })
        .collect();
    let entry = |joint: u16| on.iter().position(|&j| j == joint).unwrap() as u16;
    let weights = vec![
        SkinWeights::strongest([(entry(5), 0.6), (entry(4), 0.4)]).unwrap(),
        SkinWeights::strongest([(entry(10), 0.5), (entry(8), 0.3), (entry(0), 0.2)]).unwrap(),
        SkinWeights::strongest([(entry(7), 0.7), (entry(6), 0.2), (entry(1), 0.1)]).unwrap(),
        SkinWeights::rigid(entry(11)),
    ];
    let weights_before = weights.clone();
    let mut skin = ImportedSkin { joints: Vec::new(), binds: binds.clone(), weights };
    let source = joints.clone();
    cut(&mut joints, Some(&mut skin), &mut rig, mobile(8), Cook::Mobile).unwrap().unwrap();

    // Every entry binds a kept joint, in its place; each dropped joint's entry is carried up the rest pose
    // to the kept joint above it: hair_1 for hair_2 and hair_3, bang_0 for bang_1, skirt_1 for skirt_2.
    assert!(skin.binds.iter().all(|bind| usize::from(bind.joint) < joints.len()));
    assert_eq!(skin.binds.len(), binds.len());
    let carried: Vec<_> = [4, 5, 7, 10].map(|joint| skin.binds[usize::from(entry(joint))].joint).to_vec();
    assert_eq!(carried, [3, 3, 4, 6]);
    // No vertex's influences moved: the cut leaves their bytes.
    assert_eq!(skin.weights, weights_before);
    // In the rest pose each palette entry is the identity, the carried ones as their originals were.
    let cut_model = rest_model(&joints);
    for bind in &skin.binds {
        assert!(near(&multiply(&cut_model[usize::from(bind.joint)], &bind.inverse_bind_matrix), &IDENTITY));
    }
    let sum = |weights: &SkinWeights| weights.weights.iter().sum::<f32>();
    for weights in &skin.weights {
        assert!((sum(weights) - 1.0).abs() <= 2.0 * f32::EPSILON, "{weights:?}");
    }
    // Each vertex's weight by the joint it now rides.
    let by_joint = |weights: &SkinWeights| {
        let mut joints = BTreeMap::<u16, f32>::new();
        for (&entry, &weight) in weights.binds.iter().zip(&weights.weights).filter(|(_, w)| **w > 0.0) {
            *joints.entry(skin.binds[usize::from(entry)].joint).or_default() += weight;
        }
        joints.into_iter().map(|(joint, weight)| (joint, (weight * 1e4).round() / 1e4)).collect::<Vec<_>>()
    };
    // hair_3 and hair_2 both fold into hair_1, all of the weight.
    assert_eq!(by_joint(&skin.weights[0]), [(3, 1.0)]);
    // skirt_2's half goes to skirt_1; skirt_0 and the hips keep theirs.
    assert_eq!(by_joint(&skin.weights[1]), [(0, 0.2), (5, 0.3), (6, 0.5)]);
    // bang_1 folds into bang_0, beside bang_0's own.
    assert_eq!(by_joint(&skin.weights[2]), [(1, 0.1), (4, 0.9)]);
    // The socket kept its own.
    assert_eq!(by_joint(&skin.weights[3]), [(7, 1.0)]);
    assert_eq!(source.len(), 12);
}

/// A shipped VRM, read as a cook of `cook` reads its mesh, with its sidecar's settings and `cap`.
fn shipped(relative: &str, cap: JointCap, cook: Cook) -> (ImportedMesh, CharacterRig, PathBuf) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content").join(relative);
    let sidecar = meta::read(&meta::sidecar_path(&path)).unwrap();
    let ImportSettings::Vrm(settings) = sidecar.importer else { panic!("a VRM") };
    let importer = ImportSettings::Vrm(VrmSettings { joints: cap, ..settings.clone() });
    let bytes = std::fs::read(&path).unwrap();
    let mesh = cook::imported_mesh(&path, &bytes, &importer, cook).unwrap();
    let mut import = vrm::import_rig_with_slots(&bytes, settings.version).unwrap();
    cut(&mut import.joints, None, &mut import.rig, cap, cook).unwrap();
    (mesh, import.rig, path)
}

#[test]
fn the_shipped_characters_cut_to_the_phones_row_with_every_weight_kept() {
    for (relative, dropped, tails) in
        [("lookdev/character/character.vrm", 15, 15), ("combat/hero/hero.vrm", 60, 29)]
    {
        let (whole, whole_rig, _) = shipped(relative, JointCap::NONE, Cook::Mobile);
        let (mesh, rig, _) = shipped(relative, MOBILE_140, Cook::Mobile);
        let (desktop, desktop_rig, _) = shipped(relative, MOBILE_140, Cook::Desktop);
        assert_eq!(
            (&desktop, &desktop_rig),
            (&whole, &whole_rig),
            "{relative}: the desktop's is the source's"
        );
        let (skin, whole_skin) = (mesh.skin.as_ref().unwrap(), whole.skin.as_ref().unwrap());
        assert_eq!(skin.joints.len(), 140, "{relative}");
        assert_eq!(whole_skin.joints.len() - skin.joints.len(), dropped, "{relative}");
        assert_eq!(rig.joint_count, 140, "{relative}");
        // Every vertex keeps all its weight, on kept joints; and the rest of the mesh is the source's.
        for weights in &skin.weights {
            let sum: f32 = weights.weights.iter().sum();
            assert!((sum - 1.0).abs() <= 4.0 * f32::EPSILON, "{relative}: {weights:?} sums to {sum}");
            assert!(weights.binds.iter().all(|&bind| usize::from(bind) < skin.binds.len()));
        }
        assert!(skin.binds.iter().all(|bind| usize::from(bind.joint) < 140));
        assert_eq!(mesh.vertices, whole.vertices, "{relative}");
        // Humanoid bones name the same joints by name; every chain keeps its root and settings, and the
        // first round took every chain's tail.
        let name = |skin: &ImportedSkin, joint: u16| skin.joints[usize::from(joint)].name.clone();
        for (cut, before) in rig.humanoid.iter().zip(&whole_rig.humanoid) {
            assert_eq!((cut.bone, name(skin, cut.joint)), (before.bone, name(whole_skin, before.joint)));
        }
        assert_eq!(rig.springs.len(), whole_rig.springs.len());
        let mut cut_chains = 0;
        for (cut, before) in rig.springs.iter().zip(&whole_rig.springs) {
            assert_eq!(name(skin, cut.joints[0].joint), name(whole_skin, before.joints[0].joint));
            assert_eq!(cut.joints[0].stiffness, before.joints[0].stiffness);
            if cut.joints.len() < before.joints.len() {
                cut_chains += 1;
                assert!(matches!(cut.tip, SpringTip::Point { .. }), "{relative}: {}", cut.name);
            }
        }
        assert_eq!(cut_chains, tails, "{relative}");
        let cooked = rig.to_cooked().unwrap();
        CharacterRig::read(&cooked).expect("the cut rig reads as the runtime reads it");
    }
}

/// A character posed through a clip with its springs, the skin's vertices placed on the CPU.
struct Posed {
    pose: CharacterPose,
    springs: SpringBones,
    palette: Vec<Affine3A>,
}

impl Posed {
    /// With `cooked`, the mesh as its cook writes it, the heroine's clearance fits it, as a boot does.
    fn new(
        mesh: &ImportedMesh,
        rig: &CharacterRig,
        motion: &sapphire_anim::CharacterMotion,
        cooked: Option<&sapphire_assets::mesh::Mesh>,
    ) -> Self {
        let skin = mesh.skin.as_ref().unwrap();
        let skeleton = Skeleton::new(&skin.joints).to_cooked().unwrap();
        let rig_bytes = rig.to_cooked().unwrap();
        let pose = CharacterPose::new(
            Skeleton::read(&skeleton).unwrap(),
            &skin.binds,
            &rig.constraints,
            vec![0.0; mesh.morph_targets.len()],
        );
        let mut springs = SpringBones::new(CharacterRig::read(&rig_bytes).unwrap(), &pose).unwrap();
        springs.set_tuning(motion.springs);
        springs.set_cloth(CharacterRig::read(&rig_bytes).unwrap(), &motion.cloth).unwrap();
        if let Some(cooked) = cooked {
            let fit = HeroineClearance::new(
                cooked,
                Skeleton::read(&skeleton).unwrap(),
                CharacterRig::read(&rig_bytes).unwrap(),
            );
            assert!(fit.is_some(), "the heroine's clearance fits her cut and whole alike");
            springs = springs.with_clearance(fit);
        }
        let palette = vec![Affine3A::IDENTITY; skin.binds.len()];
        Self { pose, springs, palette }
    }

    fn frame(&mut self, clip: &sapphire_anim::ArchivedClip, ticks: f64) {
        let position = (ticks as f32).min(f32::from(clip.ticks.to_native()));
        self.pose.sample(clip, position);
        self.pose.finish();
        let frame =
            SpringFrame { world_from_model: Affine3A::IDENTITY, ticks, pose_moved: true, active: true };
        self.springs.update(&mut self.pose, &frame);
        self.pose.write_palette(&mut self.palette);
    }

    fn vertex(&self, mesh: &ImportedMesh, index: usize) -> Vec3 {
        let skin = mesh.skin.as_ref().unwrap();
        let weights = skin.weights[index];
        let at = Vec3::from(mesh.vertices[index].position);
        (0..4).fold(Vec3::ZERO, |sum, slot| {
            sum + self.palette[usize::from(weights.binds[slot])].transform_point3(at) * weights.weights[slot]
        })
    }
}

/// The kill line's measure in numbers (the row's is a person's eye on two captures): each character
/// through its strike with its springs and cloth, cut and whole, every vertex placed both ways. Cutting a
/// chain's tail moves nothing but rounding, since a tail only marks the point its parent swings toward; a
/// second round loses a joint's own swing.
#[test]
fn a_cut_tail_swings_as_it_did_and_a_second_round_moves_only_its_own_chains() {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    // The look-dev character runs twice: her springs alone, and with the heroine's clearance fit to her
    // hair strands, which ends a cut strand at its `Point`. (While the cut renumbered the palette, her cut
    // mesh cooked its vertices in another order, the clearance fit other hair samples, and her hair moved
    // 10.5 mm: every entry now keeps its place.)
    let cases = [
        ("lookdev/character/character.vrm", "lookdev/character/strike.keypose.ron", None, false),
        ("lookdev/character/character.vrm", "lookdev/character/strike.keypose.ron", None, true),
        ("combat/hero/hero.vrm", "combat/hero/punch.keypose.ron", Some("combat/hero/hero.motion.ron"), false),
    ];
    for (relative, clip, motion, clearance) in cases {
        let motion = match motion {
            Some(path) => sapphire_anim::CharacterMotion::from_ron(
                &std::fs::read_to_string(content.join(path)).unwrap(),
            )
            .unwrap(),
            None => sapphire_anim::CharacterMotion::shipped().clone(),
        };
        let document = crate::keypose::read(&std::fs::read(content.join(clip)).unwrap()).unwrap();
        let mut runs = Vec::new();
        for cap in [JointCap::NONE, MOBILE_140] {
            let (mesh, rig, _) = shipped(relative, cap, Cook::Mobile);
            let skeleton = Skeleton::new(&mesh.skin.as_ref().unwrap().joints);
            let sampled =
                crate::keypose::sample(&document, "clip", &skeleton, &HumanMap::from_rig(&rig), &[]).unwrap();
            let target = ClipTarget {
                skeleton: Some((AssetId::from_bytes([7; 16]), &skeleton)),
                morph_defaults: &[],
                map: None,
            };
            let clip = crate::clip::cook(&sampled, &target, &ClipOptions::default()).to_cooked().unwrap();
            let cooked = clearance.then(|| {
                let (_, _, path) = shipped(relative, cap, Cook::Mobile);
                let sidecar = meta::read(&meta::sidecar_path(&path)).unwrap();
                let ImportSettings::Vrm(settings) = sidecar.importer else { panic!("a VRM") };
                let importer = ImportSettings::Vrm(VrmSettings { joints: cap, ..settings });
                cook::cook_mesh_as(&path, &importer, Cook::Mobile).unwrap().0
            });
            let posed = Posed::new(&mesh, &rig, &motion, cooked.as_ref());
            runs.push((mesh, posed, clip));
        }
        let [(whole, mut whole_posed, whole_clip), (cut, mut cut_posed, cut_clip)] =
            <[_; 2]>::try_from(runs).unwrap_or_else(|_| panic!("two runs"));
        let (whole_clip, cut_clip) = (Clip::read(&whole_clip).unwrap(), Clip::read(&cut_clip).unwrap());
        // Each vertex's family: the chain the cut shortened that the first of its weighted joints is in,
        // its trailing number dropped (`skirt_03` is a skirt's), or none for a vertex on no such chain.
        let (_, whole_rig, _) = shipped(relative, JointCap::NONE, Cook::Mobile);
        let (_, cut_rig, _) = shipped(relative, MOBILE_140, Cook::Mobile);
        let shortened: Vec<&str> = cut_rig
            .springs
            .iter()
            .filter(|spring| matches!(spring.tip, SpringTip::Point { .. }))
            .map(|spring| spring.name.as_str())
            .collect();
        let skin = whole.skin.as_ref().unwrap();
        let chain_of: BTreeMap<usize, &str> = whole_rig
            .springs
            .iter()
            .filter(|spring| shortened.contains(&spring.name.as_str()))
            .flat_map(|spring| {
                let family = spring.name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_');
                spring.joints.iter().map(move |entry| (usize::from(entry.joint), family))
            })
            .collect();
        let family = |index: usize| {
            let weights = &skin.weights[index];
            (0..4)
                .filter(|&slot| weights.weights[slot] > 0.0)
                .find_map(|slot| {
                    chain_of.get(&usize::from(skin.binds[usize::from(weights.binds[slot])].joint))
                })
                .copied()
        };
        let mut by_family = BTreeMap::<&str, f32>::new();
        let (mut worst, mut moved) = (0.0_f32, 0usize);
        let frames = u32::from(whole_clip.ticks.to_native()) + 60;
        for tick in 0..=frames {
            whole_posed.frame(whole_clip, f64::from(tick));
            cut_posed.frame(cut_clip, f64::from(tick));
            for index in 0..whole.vertices.len() {
                let apart = whole_posed.vertex(&whole, index).distance(cut_posed.vertex(&cut, index));
                worst = worst.max(apart);
                moved += usize::from(apart > 0.001);
                if apart > 0.001 {
                    let family = family(index).unwrap_or_else(|| {
                        panic!("{relative}: vertex {index} is on no chain the cut shortened")
                    });
                    let entry = by_family.entry(family).or_default();
                    *entry = entry.max(apart);
                }
            }
        }
        let samples = whole.vertices.len() * (frames as usize + 1);
        println!(
            "{relative} (clearance {clearance}): the cut moves a vertex {:.4} mm at most; {moved} of {samples} \
             vertex-frames over 1 mm",
            worst * 1000.0
        );
        for (family, worst) in &by_family {
            println!("  {family}: {:.3} mm", worst * 1000.0);
        }
        if relative.starts_with("lookdev") {
            assert!(worst <= 5e-7, "{relative}: a tail cut moved a vertex {worst} m");
        }
    }
}

/// Two skins on one joint (the importer keeps an entry per joint and inverse bind): hair_1 has an entry
/// bound at rest and one bound a centimeter off it, and hair_2's entry is the second skin's, off rest
/// alike. The fold carries hair_2's own bind up to hair_1 as a new entry, so its vertices stay where that
/// skin put them, where taking hair_1's first entry would move them the centimeter.
#[test]
fn a_fold_keeps_each_skins_bind_where_the_bind_pose_is_not_the_rest_pose() {
    let (mut joints, mut rig) = character();
    joints[3].rest.rotation = [0.0, 0.0, 0.258_819, 0.965_926];
    let model = rest_model(&joints);
    let off = |joint: usize| {
        let mut bound = model[joint];
        bound[3][0] += 0.01;
        inverse(&bound)
    };
    let mut binds: Vec<SkinBind> = (0..12)
        .map(|joint| SkinBind { joint, inverse_bind_matrix: inverse(&model[usize::from(joint)]) })
        .collect();
    binds[4].inverse_bind_matrix = off(4);
    binds.push(SkinBind { joint: 3, inverse_bind_matrix: off(3) });
    let weights = vec![SkinWeights::rigid(4), SkinWeights::rigid(12), SkinWeights::rigid(3)];
    let mut skin = ImportedSkin { joints: Vec::new(), binds: binds.clone(), weights };
    cut(&mut joints, Some(&mut skin), &mut rig, mobile(8), Cook::Mobile).unwrap().unwrap();
    let cut_model = rest_model(&joints);
    // A point's place at rest through an entry: its joint's rest transform after its inverse bind.
    let place = |model: &[Matrix], bind: &SkinBind| {
        multiply(&model[usize::from(bind.joint)], &bind.inverse_bind_matrix)
    };
    let entry = |vertex: usize| &skin.binds[usize::from(skin.weights[vertex].binds[0])];
    // hair_2's vertex rides hair_1 now, through an entry of its own, and stays where its skin put it.
    assert_eq!(entry(0).joint, 3);
    assert!(near(&place(&cut_model, entry(0)), &place(&model, &binds[4])));
    assert!(!near(&place(&cut_model, entry(0)), &IDENTITY), "bound a centimeter off rest");
    // hair_1's two entries keep theirs.
    assert!(near(&place(&cut_model, entry(1)), &place(&model, &binds[12])));
    assert!(near(&place(&cut_model, entry(2)), &IDENTITY));
    let [a, b, c] = [0, 1, 2].map(|vertex| skin.weights[vertex].binds[0]);
    assert!(a != b && a != c && b != c, "{a} {b} {c}");
}

/// Two chains sharing their first joints, as the runtime allows (a joint in two swings for the first):
/// the shared joint can go only once the longer chain's joints below it have, so the planner passes again.
///
/// ```text
/// 0 hips ─ 1 root ─ 2 shared ─┬─ 3 short_tail
///                             └─ 4 long_1 ─ 5 long_2 ─ 6 long_tail
/// ```
#[test]
fn a_joint_two_chains_share_goes_once_both_have_gone_below_it() {
    let joints = vec![
        joint("hips", None, [0.0, 1.0, 0.0]),
        joint("root", Some(0), [0.0, 0.1, 0.0]),
        joint("shared", Some(1), [0.0, -0.1, 0.0]),
        joint("short_tail", Some(2), [0.0, -0.1, 0.0]),
        joint("long_1", Some(2), [0.0, -0.1, 0.01]),
        joint("long_2", Some(4), [0.0, -0.1, 0.0]),
        joint("long_tail", Some(5), [0.0, -0.1, 0.0]),
    ];
    let rig = CharacterRig {
        joint_count: 7,
        humanoid: vec![HumanoidJoint { bone: HumanBone::Hips, joint: 0 }],
        springs: vec![spring("short", &[1, 2, 3]), spring("long", &[1, 2, 4, 5, 6])],
        ..CharacterRig::default()
    };
    let made = plan(&joints, &rig, 2, Cook::Mobile).unwrap().expect("a cut");
    assert_eq!(names(&made, &joints), ["shared", "short_tail", "long_1", "long_2", "long_tail"]);
}

/// A source sharing a cut skeleton (`rig:`) maps by the joints the owner's cut dropped on that cook: a
/// key on one is ignored, and a joint missing from the shared skeleton for any other reason is refused
/// as it is uncut. With nothing cut (a cap the skeleton fits, or a cook the cap doesn't name) the map
/// is the plain one.
#[test]
fn a_source_sharing_a_cut_skeleton_ignores_only_the_joints_the_cut_dropped() {
    use crate::clip::source::{joint_remap, joint_remap_cut};
    let (source, rig) = character();
    let on_desktop = JointCap { desktop: NonZeroU16::new(8), mobile: None };
    for (cap, cook) in [(mobile(8), Cook::Mobile), (on_desktop, Cook::Desktop)] {
        let (mut joints, mut cut_rig) = (source.clone(), rig.clone());
        let made = cut(&mut joints, None, &mut cut_rig, cap, cook).unwrap().expect("a cut");
        let dropped = names(&made, &source);
        let shared = Skeleton::new(&joints);
        let remap = joint_remap_cut(&source, &shared, &dropped).unwrap();
        assert_eq!(remap, made.kept, "{cook}");
        // A joint renamed in the source isn't one the cut dropped, so the map still refuses it.
        let mut renamed = source.clone();
        renamed[6].name = "fringe_0".into();
        joint_remap_cut(&renamed, &shared, &dropped).expect_err("bang_0 isn't in the source");
    }
    // The cap that names the other cook, and a cap the skeleton fits, cut nothing: the plain map holds.
    for (cap, cook) in [(on_desktop, Cook::Mobile), (mobile(12), Cook::Mobile)] {
        let (mut joints, mut cut_rig) = (source.clone(), rig.clone());
        assert!(cut(&mut joints, None, &mut cut_rig, cap, cook).unwrap().is_none(), "{cook}");
        let shared = Skeleton::new(&joints);
        let plain: Vec<Option<u16>> = joint_remap(&source, &shared).unwrap().into_iter().map(Some).collect();
        assert_eq!(joint_remap_cut(&source, &shared, &[]).unwrap(), plain);
        // A joint the shared skeleton lacks is refused, not taken for a cut.
        let mut extra = source.clone();
        extra.push(joint("hair_4", Some(5), [0.0, -0.1, 0.0]));
        joint_remap_cut(&extra, &shared, &[]).expect_err("hair_4 is no joint of the shared skeleton");
    }
}

/// The refusals of what reads a cut skeleton, on the look-dev character's mobile cut, each at the function
/// its cook calls (a whole tree of her playable or a mesh sharing her rig would cook all of `content/`):
/// a held mesh on a joint the cut dropped, by `look.held.joint`; a clip sharing her rig whose root is on a
/// dropped joint; and a mesh sharing her skeleton, whose own skin folds the dropped joints as hers did.
#[test]
fn what_reads_a_cut_skeleton_folds_through_it_or_is_refused_by_its_field() {
    use crate::clip::SampledClip;
    use crate::clip::source::{joint_remap, joint_remap_cut, remap_cut_tracks};
    let (whole, _, path) = shipped("lookdev/character/character.vrm", JointCap::NONE, Cook::Mobile);
    let (cut_mesh, _, _) = shipped("lookdev/character/character.vrm", MOBILE_140, Cook::Mobile);
    let source = whole.skin.as_ref().unwrap();
    let kept = &cut_mesh.skin.as_ref().unwrap().joints;
    let dropped: Vec<String> = source
        .joints
        .iter()
        .filter(|joint| !kept.iter().any(|k| k.name == joint.name))
        .map(|joint| joint.name.clone())
        .collect();
    assert_eq!(dropped.len(), 15);

    // A held mesh: refused on a dropped joint by its field and cook, kept on the hand.
    let error = cook::held_joint_kept(&path, &dropped[0], &dropped, Cook::Mobile).unwrap_err();
    assert!(
        matches!(&error, cook::CookError::HeldJointCut { joint, cook: Cook::Mobile, .. } if *joint == dropped[0])
    );
    assert!(error.to_string().contains("`look.held.joint`"), "{error}");
    cook::held_joint_kept(&path, "J_Bip_R_Hand", &dropped, Cook::Mobile).unwrap();

    // A clip sharing her rig: keys on dropped joints go, and a root on one is refused by that joint.
    let shared = Skeleton::new(kept);
    let remap = joint_remap_cut(&source.joints, &shared, &dropped).unwrap();
    let on = |name: &str| source.joints.iter().position(|joint| joint.name == name).unwrap() as u16;
    let hips = on("J_Bip_C_Hips");
    let tail = on(&dropped[0]);
    let clip = |root: u16| SampledClip {
        name: "sway".into(),
        rate: 30,
        ticks: 1,
        rotations: vec![(hips, vec![Default::default(); 2]), (tail, vec![Default::default(); 2])],
        root: Some(crate::clip::SampledRoot { joint: root, ..Default::default() }),
        ..SampledClip::default()
    };
    let kept_keys = remap_cut_tracks(clip(hips), &remap, &source.joints).unwrap();
    assert_eq!(
        kept_keys.rotations.iter().map(|(joint, _)| *joint).collect::<Vec<_>>(),
        [remap[usize::from(hips)].unwrap()]
    );
    let error = remap_cut_tracks(clip(tail), &remap, &source.joints).unwrap_err();
    assert!(error.to_string().contains(&dropped[0]), "{error}");

    // A mesh sharing her skeleton with the whole armature: its skin folds to the kept joints, each vertex
    // where it was at rest, and maps onto her cut skeleton.
    let mut sharing = source.clone();
    cut_shared(&mut sharing, &dropped);
    assert_eq!(sharing.joints, *kept);
    assert_eq!(sharing.weights, source.weights);
    joint_remap(&sharing.joints, &shared).unwrap();
    let (whole_model, cut_model) = (rest_model(&source.joints), rest_model(&sharing.joints));
    for (before, after) in source.binds.iter().zip(&sharing.binds) {
        let place = |model: &[Matrix], bind: &SkinBind| {
            multiply(&model[usize::from(bind.joint)], &bind.inverse_bind_matrix)
        };
        assert!(near(&place(&whole_model, before), &place(&cut_model, after)));
    }
}
