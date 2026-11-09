//! The enemy (M2 O4): KayKit's skeleton warrior, re-proportioned and in its anime character materials
//! (`content/combat/enemy/`). `--look-dev enemy` squares it up to the character in the town under the
//! stage's day, so the two looks can be judged side by side at combat distance, and `--arena` fights
//! it (M2 G6, [`crate::arena`]).
//!
//! On the look-dev stage it holds one pose, as the character does without `--clip` ([`held`]): a
//! [`CharacterPose`] with the rig's bones turned from its T-pose by [`pose::hold`], found by name rather
//! than through a VRM's rig. The rig has no twist helpers, so no constraints follow. In a fight its
//! graph poses it ([`Look::graph`], [`driver`]), on clips that cook onto it by retarget
//! (`sapphire_pipeline::combat::warrior`), its feet and head solved over that ([`ik_rig`]), and with no
//! fight it holds that pose.
//!
//! - **What draws it** is its kind of enemy's document (`warrior.enemy.ron`'s `look`, M3 B3), cooked, which
//!   the stage reads as a [`Look`]: its mesh, graph and blade, the joint that holds the blade, its scale
//!   and the figure a camera frames. Every kind of enemy draws the same way from its own document (M3 B3b):
//!   the warrior is the kind a fight always has ([`KIND`]), which draws anyone whose kind nothing names.
//! - **Another kind** has its own skinned mesh, on a skeleton with the warrior's joint names, which are what
//!   its pose and IK find it by; it may hold nothing that isn't skinned in ([`Look::held`]).
//! - **Scale:** its document's 0.8, which `it_stands_a_head_taller_than_the_character_at_combat_distance`
//!   explains.
//! - **The blade** is a mesh of its own, placed at the rig's right hand slot ([`Look::held`]). No
//!   attachment system exists: when clips play, the blade moves by `place × model_from_joint[hand slot]`
//!   each frame (`ArenaActors::hold_blade`), and each clip's retarget keeps that slot in the clip's grip
//!   (`follow` in the pack's sidecars), so nothing corrects it here. It draws as a character with no
//!   body, so it casts no capsule of its own, and its light is judged at the warrior's head, so the
//!   two are lit alike.
//!
//! Hit volumes and timing are the moves' (`warrior.moves.ron`): nothing here reads them.

use std::sync::Arc;

use sapphire_anim::CharacterMotion;
use sapphire_anim::{ArchivedSkeleton, CharacterPose, GraphBinding, GraphPlayer, IkRig};
use sapphire_assets::AssetId;
use sapphire_assets::mesh::Skin;
use sapphire_assets::rig::HumanBone;
use sapphire_gameplay::body::{BodyGraphAssets, BodyProfile, BodyProfileOptions};
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_moves::{EnemyLook, HeldLook};
use sapphire_physics::PhysicsWorld;
use sapphire_render::{FaceExpression, MeshHandle};

use sapphire_stage::RenderPose;
use sapphire_stage::anim_graph::GraphDriver;
use sapphire_stage::ik::{IkControls, IkDriver};
use sapphire_stage::lookdev::{FigureJoints, FigureShape};
use sapphire_stage::pose::{self, RigJoints};
use sapphire_stage::scene::Figure;

/// The warrior as a kind of enemy, by its sidecar's ID (`content/combat/enemy/warrior.enemy.ron.meta`):
/// what the stage reads its [`Look`] from.
pub(crate) const KIND: AssetId = AssetId::from_text("0a8af012-0a05-463c-8998-333a9c5d1f42");

/// What its graph draws from, which has nothing to draw for yet: it has no blink.
const GRAPH_SEED: u32 = 2;

/// What draws a kind of enemy, resolved from its cooked [`EnemyLook`].
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// Its skinned mesh, whose skin names its skeleton.
    pub mesh: AssetId,
    /// The graph it plays in a fight.
    pub graph: AssetId,
    /// A mesh of its own that it holds, as the warrior does its blade, and the joint that grips it, in the
    /// rig's own names. A weapon skinned into its mesh needs none.
    pub held: Option<HeldLook>,
    /// How much bigger than its mesh it draws.
    pub scale: f32,
    /// How far its crown stands over its feet, in metres, as drawn: what a camera frames.
    pub crown: f32,
    /// How high its eyes stand over its feet, in metres: what the character looks at in a fight.
    pub eyes: f32,
    /// How wide it stands from its middle, in metres, shoulders and all, what a camera fits in its frame.
    /// Its capsule, which a body meets, is narrower. Clearing takes everyone to draw a tenth wider, for
    /// arms and blades.
    pub framed_radius: f32,
}

impl Look {
    /// The stage's copy of `look`.
    #[must_use]
    pub fn of(look: &EnemyLook) -> Self {
        Self {
            mesh: look.mesh,
            graph: look.graph,
            held: look.held.clone(),
            scale: look.scale,
            crown: look.crown(),
            eyes: look.eyes(),
            framed_radius: look.framed_radius,
        }
    }

    /// How big it draws.
    #[must_use]
    pub fn sizes(&self) -> Sizes {
        Sizes::of(self)
    }

    /// The warrior's, from its document as this build holds it
    /// ([`sapphire_gameplay::replay::warrior_enemy`]), for tests that cook nothing. A stage reads the cooked
    /// one ([`KIND`]).
    #[must_use]
    pub fn warrior() -> Self {
        Self::of(&sapphire_gameplay::replay::warrior_enemy().expect("it compiles").look)
    }
}

/// Where it stands on the stage, in metres from the character at the origin: to the character's right
/// and a little in front, at the distance its quick cut reaches from, turned toward the character and
/// partly toward the shot, so its eyes show.
const STANDS_AT: Vec3 = Vec3::new(-2.0, 0.0, 0.5);
const FACES: f32 = 52.0;

/// The rig's bones the pose turns, by the rig's own names.
const BONES: [(&str, HumanBone); 13] = [
    ("spine", HumanBone::Spine),
    ("chest", HumanBone::Chest),
    ("head", HumanBone::Head),
    ("upperarm.l", HumanBone::LeftUpperArm),
    ("lowerarm.l", HumanBone::LeftLowerArm),
    ("wrist.l", HumanBone::LeftHand),
    ("upperarm.r", HumanBone::RightUpperArm),
    ("lowerarm.r", HumanBone::RightLowerArm),
    ("wrist.r", HumanBone::RightHand),
    ("upperleg.l", HumanBone::LeftUpperLeg),
    ("upperleg.r", HumanBone::RightUpperLeg),
    ("foot.l", HumanBone::LeftFoot),
    ("foot.r", HumanBone::RightFoot),
];

/// The bones IK needs that the pose doesn't turn. Toes are what plant a foot's ball.
const LEG_BONES: [(&str, HumanBone); 5] = [
    ("hips", HumanBone::Hips),
    ("lowerleg.l", HumanBone::LeftLowerLeg),
    ("lowerleg.r", HumanBone::RightLowerLeg),
    ("toes.l", HumanBone::LeftToes),
    ("toes.r", HumanBone::RightToes),
];

/// The held pose's turns, in the model's rest space (facing +Z, its left toward +X): hunched forward
/// over a wide stance, the left arm hanging out from its side, and the right held out with the forearm
/// level and the blade raised, which makes the silhouette. In the rest pose the blade points along
/// +Z, so the last turn is a roll of the arm about its bone.
fn turns() -> [(HumanBone, Quat); 12] {
    let degrees = f32::to_radians;
    [
        (HumanBone::Spine, Quat::from_rotation_x(degrees(10.0))),
        (HumanBone::Chest, Quat::from_rotation_x(degrees(8.0)) * Quat::from_rotation_y(degrees(-10.0))),
        (HumanBone::Head, Quat::from_rotation_x(degrees(-4.0)) * Quat::from_rotation_y(degrees(8.0))),
        (
            HumanBone::LeftUpperArm,
            Quat::from_rotation_z(degrees(-62.0)) * Quat::from_rotation_y(degrees(-12.0)),
        ),
        (HumanBone::LeftLowerArm, Quat::from_rotation_y(degrees(-22.0))),
        (HumanBone::LeftHand, Quat::from_rotation_x(degrees(10.0))),
        (
            HumanBone::RightUpperArm,
            Quat::from_rotation_y(degrees(-8.0)) * Quat::from_rotation_z(degrees(52.0)),
        ),
        (
            HumanBone::RightLowerArm,
            Quat::from_rotation_z(degrees(-58.0)) * Quat::from_rotation_y(degrees(22.0)),
        ),
        (HumanBone::RightHand, Quat::from_rotation_x(degrees(-84.0))),
        (HumanBone::LeftUpperLeg, Quat::from_rotation_z(degrees(6.0))),
        (HumanBone::RightUpperLeg, Quat::from_rotation_z(degrees(-6.0))),
        (HumanBone::RightFoot, Quat::from_rotation_y(degrees(-14.0))),
    ]
}

/// The radius of its body capsule (M1 R4), from its feet to its head.
const BODY_RADIUS: f32 = 0.3;
/// From the head joint to the middle of the skull, in the model's units.
const HEAD_CENTER: Vec3 = Vec3::new(0.0, 0.27, 0.03);
/// What its figure says that no pose moves.
const SHAPE: FigureShape = FigureShape {
    head_center: HEAD_CENTER,
    body_radius: BODY_RADIUS,
    expression: FaceExpression { blush: 0.0, gloom: 0.0 },
};

/// A kind of enemy's meshes on a renderer: its body, and what it holds, if that's a mesh of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnemyMeshes {
    pub warrior: MeshHandle,
    pub blade: Option<MeshHandle>,
}

/// How big a kind of enemy draws, and where a camera and a foe find it: its [`Look`]'s numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sizes {
    pub scale: f32,
    pub crown: f32,
    pub eyes: f32,
    pub framed_radius: f32,
}

impl Sizes {
    #[must_use]
    pub fn of(look: &Look) -> Self {
        Self { scale: look.scale, crown: look.crown, eyes: look.eyes, framed_radius: look.framed_radius }
    }
}

/// Where a kind's held mesh is: the joint that grips it, and where its pose has that joint, in its model
/// space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grip {
    pub hand: usize,
    pub at: Mat4,
}

/// The enemy, ready to place: its held pose, the figure it makes, and where the pose puts what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub pose: RenderPose,
    pub figure: Figure,
    /// Where it holds its blade, if it holds a mesh of its own.
    pub grip: Option<Grip>,
    /// How big it draws.
    pub sizes: Sizes,
}

/// Where the warrior stands on the stage, at `scale`: the model's space to the world.
pub(crate) fn place(scale: f32) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::splat(scale),
        Quat::from_rotation_y(FACES.to_radians()),
        STANDS_AT,
    )
}

/// The warrior's held pose and what it makes, from its cooked skin and skeleton, as `look` holds its blade.
/// Fails if the rig lacks a joint the pose needs.
pub(crate) fn held(skin: &Skin, skeleton: &ArchivedSkeleton, look: &Look) -> Result<Held, String> {
    check_counts(skin, skeleton)?;
    let hand = look.held.as_ref().map(|held| held.joint.as_str());
    let hand_slot = hand
        .map(|hand| skeleton.find(hand).ok_or_else(|| format!("the warrior's rig has no {hand:?}")))
        .transpose()?;
    let joints = rig_joints(skeleton)?;
    let head_joint = joints.joint(HumanBone::Head).ok_or("the warrior's rig has no head")?;
    let mut pose = CharacterPose::new(skeleton, &skin.binds, joints.constraints(), Vec::new());
    pose::hold(&mut pose, &joints, &turns());
    let (_, head_rotation, head) = pose.model()[head_joint].to_scale_rotation_translation();
    let figure = Figure {
        head: head + head_rotation * HEAD_CENTER,
        feet: Vec3::new(head.x, 0.0, head.z),
        body_radius: SHAPE.body_radius,
        face: Quat::IDENTITY,
        expression: SHAPE.expression,
    };
    let grip = hand_slot.map(|hand| Grip { hand, at: Mat4::from(pose.model()[hand]) });
    let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
    pose.write_palette(&mut palette);
    let pose = RenderPose { palette, morph_weights: Vec::new() };
    Ok(Held { pose, figure, grip, sizes: look.sizes() })
}

/// A driver for the warrior on `binding`, over its rest pose: what poses it in a fight
/// ([`crate::fight::FightShow::play_fighters`]). Fails if the rig lacks a joint the pose needs.
pub(crate) fn driver(
    skin: &Skin,
    skeleton: &ArchivedSkeleton,
    binding: Arc<GraphBinding>,
    ground: Arc<PhysicsWorld>,
    motion: &CharacterMotion,
) -> Result<GraphDriver, String> {
    check_counts(skin, skeleton)?;
    let joints = rig_joints(skeleton)?;
    let head = joints.joint(HumanBone::Head).ok_or("the warrior's rig has no head")?;
    let pose = CharacterPose::new(skeleton, &skin.binds, joints.constraints(), Vec::new());
    let figure = FigureJoints::new(&pose, &joints, head, 0.0, SHAPE);
    let ik = IkDriver::new(ik_rig(skeleton, &pose)?, ground, IkControls::of(motion), motion);
    Ok(GraphDriver::new(GraphPlayer::new(binding, pose, GRAPH_SEED), figure).with_ik(ik))
}

/// Fails if `skin` wasn't cooked for `skeleton`, as for a moment when a new version of one lands before
/// the other: a pose of one on binds of the other reads past its joints.
fn check_counts(skin: &Skin, skeleton: &ArchivedSkeleton) -> Result<(), String> {
    let (binds, joints) = (skin.joint_count as usize, skeleton.len());
    if binds == joints {
        return Ok(());
    }
    Err(format!("the warrior's mesh is for {binds} joints, and its skeleton has {joints}"))
}

/// What IK reads of `skeleton`, over `pose`, in the model's space with its soles at 0. No eyes and no
/// neck: its head alone turns. Fails if the rig lacks a joint.
pub(crate) fn ik_rig(skeleton: &ArchivedSkeleton, pose: &CharacterPose) -> Result<IkRig, String> {
    Ok(IkRig::new(pose, &ik_bones(skeleton)?, HEAD_CENTER, None, 0.0))
}

fn ik_bones(skeleton: &ArchivedSkeleton) -> Result<Vec<(HumanBone, usize)>, String> {
    let find = |name: &str| skeleton.find(name).ok_or_else(|| format!("the warrior's rig has no {name:?}"));
    BONES.iter().chain(&LEG_BONES).map(|&(name, bone)| Ok((bone, find(name)?))).collect()
}

pub(crate) fn simulation_profile(
    assets: BodyGraphAssets,
    look: &Look,
    look_height: f32,
) -> Result<BodyProfile, String> {
    let bones = ik_bones(assets.skeleton.view())?;
    // The joint holding its own mesh, if it holds one, is where it touches what it hits.
    let hand = look.held.as_ref().map(|held| held.joint.as_str());
    BodyProfile::bind_named(
        assets,
        &bones,
        HEAD_CENTER,
        BodyProfileOptions {
            floor: 0.0,
            scale: look.scale,
            morphs: &[],
            contact_joints: hand.as_slice(),
            seed: GRAPH_SEED,
            ground_turn: false,
            grounded_gather: false,
            look_height,
            look_weight: 1.0,
            head_authority: None,
        },
    )
    .map_err(|error| error.to_string())
}

/// The rig's bones, found by name.
fn rig_joints(skeleton: &ArchivedSkeleton) -> Result<RigJoints, String> {
    let find = |name: &str| skeleton.find(name).ok_or_else(|| format!("the warrior's rig has no {name:?}"));
    let bones =
        BONES.iter().map(|&(name, bone)| Ok((bone, find(name)?))).collect::<Result<Vec<_>, String>>()?;
    Ok(RigJoints::of_bones(bones))
}

/// Every joint name the warrior's rig needs, the hand slot last if it has one: for hot reload's test rigs.
#[cfg(all(test, feature = "dev"))]
pub(crate) fn joint_names(hand: Option<&str>) -> Vec<&str> {
    BONES.iter().chain(&LEG_BONES).map(|&(name, _)| name).chain(hand).collect()
}

/// The figure the blade draws as, in its own space, if it holds one: no body, so it adds no capsule to the
/// character's shadow or occlusion, and its head where the warrior's is, so the light rig lights it as it
/// does the warrior.
pub(crate) fn blade_figure(held: &Held) -> Option<Figure> {
    held.grip.map(|grip| held.figure.of_item_at(grip.at))
}

#[cfg(test)]
mod tests {
    use sapphire_anim::{Joint, JointTransform};
    use sapphire_assets::mesh::{Influences, SkinBind};

    use super::*;

    /// The rig's named joints in a chain, each 0.1 up its parent.
    fn rig() -> Vec<Joint> {
        let names = BONES.iter().map(|&(name, _)| name).chain(["handslot.r"]);
        names
            .enumerate()
            .map(|(index, name)| Joint {
                name: name.to_owned(),
                parent: index.checked_sub(1).map(|parent| parent as u16),
                rest: JointTransform { translation: [0.0, 0.1, 0.0], ..JointTransform::IDENTITY },
            })
            .collect()
    }

    fn skin_over(count: usize) -> Skin {
        let binds = (0..count)
            .map(|joint| SkinBind {
                joint: joint as u16,
                inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            })
            .collect();
        Skin { skeleton: None, joint_count: count as u32, binds, influences: Influences::pack(&[], count) }
    }

    #[test]
    fn it_holds_a_pose_with_the_blade_at_the_hand_slot() {
        let bytes = sapphire_anim::Skeleton::new(&rig()).to_cooked().expect("cooked");
        let skeleton = sapphire_anim::Skeleton::read(&bytes).expect("a valid skeleton");
        let held =
            held(&skin_over(skeleton.len()), skeleton, &Look::warrior()).expect("every joint is there");
        assert_eq!(held.pose.palette.len(), skeleton.len());
        assert_eq!(held.figure.feet.y, 0.0);
        assert!(held.figure.head.y > 0.0);
        // The blade's figure names the warrior's head from the blade's own space.
        let (grip, carried) = (held.grip.expect("it holds its blade"), blade_figure(&held).expect("a blade"));
        assert_eq!(grip.hand, skeleton.len() - 1);
        let head = grip.at.transform_point3(carried.head);
        assert!(head.distance(held.figure.head) < 1e-3);
        assert_eq!(carried.body_radius, 0.0, "it adds no capsule");
    }

    /// A kind whose weapon is skinned into its mesh holds nothing else, and needs no hand slot.
    #[test]
    fn a_kind_that_holds_nothing_poses_without_a_hand_slot() {
        let joints: Vec<Joint> = rig().into_iter().filter(|joint| joint.name != "handslot.r").collect();
        let bytes = sapphire_anim::Skeleton::new(&joints).to_cooked().expect("cooked");
        let skeleton = sapphire_anim::Skeleton::read(&bytes).expect("a valid skeleton");
        let look = Look { held: None, ..Look::warrior() };
        let held = held(&skin_over(skeleton.len()), skeleton, &look).expect("every joint it needs is there");
        assert_eq!(held.grip, None);
        assert_eq!(blade_figure(&held), None);
        assert!(held.figure.head.y > 0.0);
    }

    #[test]
    fn a_rig_without_the_hand_slot_says_so() {
        let joints = [Joint { name: "hips".to_owned(), parent: None, rest: JointTransform::IDENTITY }];
        let bytes = sapphire_anim::Skeleton::new(&joints).to_cooked().expect("cooked");
        let skeleton = sapphire_anim::Skeleton::read(&bytes).expect("a valid skeleton");
        let pose = CharacterPose::new(skeleton, &[], &[], Vec::new());
        let Err(error) = ik_rig(skeleton, &pose) else { panic!("it has only hips") };
        assert!(error.contains("no \"spine\""), "{error}");
        let error = held(&skin_over(1), skeleton, &Look::warrior()).expect_err("no joints");
        assert!(error.contains("no \"handslot.r\""), "{error}");
    }

    /// A new version of the mesh or the skeleton can land before the other.
    #[test]
    fn a_mesh_for_other_joints_says_so() {
        let bytes = sapphire_anim::Skeleton::new(&rig()).to_cooked().expect("cooked");
        let skeleton = sapphire_anim::Skeleton::read(&bytes).expect("a valid skeleton");
        let error =
            held(&skin_over(skeleton.len() + 1), skeleton, &Look::warrior()).expect_err("one joint too many");
        assert!(error.contains("joints"), "{error}");
    }

    /// The derived model (`sapphire_pipeline::combat::warrior`) stands 2.33 units to its crown and 2.59 to
    /// its horns' tips. Its document's scale of 0.8 puts the crown at 1.87 m and the horns at 2.07 m against
    /// the character's 1.62 m: a head taller, as the reference fights' common enemies stand, with a blade
    /// whose reach, 1.8 m with the arm straight, is about what its moves author (`warrior.moves.ron`). And
    /// it stands within the reach of its quick cut.
    #[test]
    fn it_stands_a_head_taller_than_the_character_at_combat_distance() {
        let look = Look::warrior();
        let (crown, horns) = (look.crown, 2.59 * look.scale);
        assert!((1.75..1.95).contains(&crown) && horns < 2.2, "{crown} and {horns} m against 1.62 m");
        let distance = STANDS_AT.length();
        assert!((1.5..2.6).contains(&distance), "{distance} m from the character");
    }
}
