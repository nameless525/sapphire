//! IK in the sandbox (`--ik on`, M2 N4): the seam between `sapphire_anim`'s solvers, which take the
//! ground as plain data, and whoever knows the ground, here `sapphire_physics`.
//!
//! - **[`IkDriver`]** is what a [`ClipPlayer`](crate::animate::ClipPlayer) or a
//!   [`GraphDriver`](crate::anim_graph::GraphDriver) keeps beside its pose: a character's
//!   [`CharacterIk`], the world it casts for ground in, and its [`IkControls`], the plain inputs a graph
//!   or a game drives: how grounded the feet are, what to look at and how hard, what the hands hold.
//! - **Casts run in the character's job,** after sampling, which is the only place a foot's place is
//!   known. `PhysicsWorld` is `Sync` and its queries take `&self`, so jobs can share one; a test below
//!   holds that.
//! - **Its own key.** A position that repeats skips a pose's evaluation, at no cost. IK adds what else
//!   can move a pose ([`IkDriver::wants_evaluation`]). Under a clock that ran, that's a control or the
//!   character's place changing, or a solve still settling, so a target moving under a held position
//!   is followed. Under a frozen clock it's only what takes effect with no time passing, a grip: a
//!   look's rate limit and the feet's easing run on the owner's clock, so they can't move, and however
//!   a target moves a frozen character costs nothing and holds its bytes (`IkInput::frozen_key`).
//!   A graph has its own key, which covers all of that but a grip, since nothing's solved until it
//!   hands over a new pose: a grip moved under a frozen graph waits for the thaw
//!   ([`GraphDriver::pose_frame`](crate::anim_graph::GraphDriver::pose_frame)).
//! - **[`IkGround`]** is the test bed, `--ik-ground flat|step|slope`: colliders and stone blocks that
//!   match, in the look-dev square, with the character standing where the controller's capsule would
//!   ([`IkGround::stand`]), so a slope's round-foot gap is there to close.

use std::sync::Arc;

use sapphire_anim::{
    CharacterIk, CharacterPose, FeetInput, FootGround, FootSettings, GroundHit, HeldWeapon, IkInput, IkRig,
    LookInput, LookSettings,
};
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_physics::{Group, PhysicsWorld, Pose, QueryFilter, Shape, StaticCollider, StaticShape};
use sapphire_time::TICK_HZ;

/// The controller's capsule (`sapphire_gameplay`'s `ControllerProfile`), which a character stands on.
const CAPSULE_RADIUS: f32 = 0.3;
const CAPSULE_HEIGHT: f32 = 1.6;

/// The slope's angle, across the character's stance, and how far up it the character stands: far
/// enough that its lower foot is still on it and not on the paving.
const SLOPE_DEGREES: f32 = 30.0;
const SLOPE_RISE: f32 = 0.6;
/// The step's height, and how far left of the character's middle its edge is.
const STEP_HEIGHT: f32 = 0.18;
const STEP_EDGE: f32 = 0.05;
/// How far the blocks run, which is out of the close shots' frames.
const BLOCK_REACH: f32 = 6.0;

/// What the character stands on (`--ik-ground`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IkGround {
    /// The square's paving, as it is.
    #[default]
    Flat,
    /// A step under its left foot, with its right on the paving.
    Step,
    /// A slope rising to its left.
    Slope,
}

impl IkGround {
    pub const ALL: [Self; 3] = [Self::Flat, Self::Step, Self::Slope];

    pub fn name(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Step => "step",
            Self::Slope => "slope",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ground| ground.name() == name)
    }

    /// The boxes it adds to the paving, each by its half extents and its middle's place and turn.
    fn boxes(self) -> Vec<(Vec3, Vec3, Quat)> {
        match self {
            Self::Flat => Vec::new(),
            Self::Step => {
                let half = Vec3::new(BLOCK_REACH * 0.5, STEP_HEIGHT * 0.5, BLOCK_REACH * 0.5);
                vec![(half, Vec3::new(STEP_EDGE + half.x, half.y, 0.0), Quat::IDENTITY)]
            }
            Self::Slope => {
                // A slab with its top through where the character stands.
                let half = Vec3::new(BLOCK_REACH * 0.5, 0.5, BLOCK_REACH * 0.5);
                let turn = Quat::from_rotation_z(SLOPE_DEGREES.to_radians());
                vec![(half, Vec3::Y * SLOPE_RISE + turn * Vec3::new(0.0, -half.y, 0.0), turn)]
            }
        }
    }

    /// Where the stage's stone box goes, for each block: the box stands on its base's middle, a unit
    /// each way before these scales.
    pub(crate) fn blocks(self) -> Vec<Mat4> {
        self.boxes()
            .into_iter()
            .map(|(half, middle, turn)| {
                let base = middle + turn * Vec3::new(0.0, -half.y, 0.0);
                Mat4::from_scale_rotation_translation(half * 2.0, turn, base)
            })
            .collect()
    }

    /// Its colliders.
    pub(crate) fn world(self) -> PhysicsWorld {
        let collider = |id: u32, half: Vec3, pose: Pose| StaticCollider {
            id: sapphire_physics::ColliderId(id),
            shape: StaticShape::Convex(Shape::Box { half_extents: half }),
            pose,
            memberships: Group::World.mask(),
        };
        let mut colliders =
            vec![collider(0, Vec3::new(60.0, 0.5, 60.0), Pose::at(Vec3::new(0.0, -0.5, 0.0)))];
        for (place, (half, middle, turn)) in self.boxes().into_iter().enumerate() {
            colliders.push(collider(place as u32 + 1, half, Pose::new(middle, turn)));
        }
        PhysicsWorld::new(colliders, 0).expect("the test bed's colliders are valid")
    }

    /// How high over its surface the root of a character whose capsule stands at this place on the
    /// ground rests, in `world`: the controller's round foot, as `sapphire_gameplay` leaves it.
    pub(crate) fn stand(world: &PhysicsWorld, x: f32, z: f32) -> f32 {
        let half_height = CAPSULE_HEIGHT * 0.5 - CAPSULE_RADIUS;
        let shape = Shape::Capsule { half_height, radius: CAPSULE_RADIUS };
        let from = Vec3::new(x, 8.0, z);
        let filter = QueryFilter::new(Group::World.mask());
        let hit = world
            .cast_shape(&shape, Pose::at(from), Vec3::NEG_Y, 16.0, &filter)
            .expect("the test bed's ground is everywhere");
        from.y - hit.distance - CAPSULE_HEIGHT * 0.5
    }
}

/// `--ik on` and what goes with it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IkOptions {
    /// What the character stands on (`--ik-ground`).
    pub ground: IkGround,
    /// What it looks at (`--ik-look`).
    pub look: Option<IkLook>,
}

/// What a character looks at (`--ik-look`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IkLook {
    /// A place in the world (`x,y,z`), relative to where the character's feet would be on flat ground.
    Point(Vec3),
    /// Something circling it at head height, a turn every [`ORBIT_TICKS`], so it keeps crossing
    /// behind (`orbit`): what a bench needs of a target, and a way to watch one.
    Orbit,
}

/// How long [`IkLook::Orbit`] takes to go round, in ticks of the character's clock.
const ORBIT_TICKS: f64 = 480.0;
const ORBIT_RADIUS: f32 = 3.0;

impl IkLook {
    /// `--ik-look`'s value.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw == "orbit" {
            return Ok(Self::Orbit);
        }
        let numbers: Vec<f32> = raw.split(',').filter_map(|part| part.parse().ok()).collect();
        match numbers[..] {
            [x, y, z] if numbers.iter().all(|number| number.is_finite()) => {
                Ok(Self::Point(Vec3::new(x, y, z)))
            }
            _ => Err(format!("--ik-look expects x,y,z or orbit, got {raw:?}")),
        }
    }

    /// Where it is at `ticks` on the character's clock, with the character standing at `height`.
    pub(crate) fn at(self, ticks: f64, height: f32) -> Vec3 {
        match self {
            Self::Point(place) => place + Vec3::Y * height,
            Self::Orbit => {
                let angle = (ticks / ORBIT_TICKS).fract() as f32 * std::f32::consts::TAU;
                let (sine, cosine) = angle.sin_cos();
                Vec3::new(ORBIT_RADIUS * sine, height + 1.5, ORBIT_RADIUS * cosine)
            }
        }
    }
}

/// The plain inputs a character's IK runs on, which a graph or a game will drive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IkControls {
    /// 0 lets the feet go as animated, 1 grounds them: in the air, or a move that wants its feet, 0.
    pub(crate) feet: f32,
    /// What to look at, in the world.
    pub(crate) look_target: Option<Vec3>,
    /// How much of the look to show.
    pub(crate) look: f32,
    /// What the hands hold.
    pub(crate) weapon: Option<HeldWeapon>,
}

impl Default for IkControls {
    fn default() -> Self {
        Self { feet: 1.0, look_target: None, look: 1.0, weapon: None }
    }
}

/// One character's IK. See the [module docs](self).
#[derive(Clone)]
pub struct IkDriver {
    ik: CharacterIk,
    ground: Arc<PhysicsWorld>,
    controls: IkControls,
    /// Presented ticks on the character's clock when last asked.
    presented: Option<f64>,
    /// Seconds of its clock since then, which the next solve takes.
    dt: f32,
    /// What the last solve ran on: `None` before the first.
    solved: Option<(IkControls, Mat4)>,
}

impl std::fmt::Debug for IkDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IkDriver").field("controls", &self.controls).finish_non_exhaustive()
    }
}

impl IkDriver {
    /// IK for a character with `rig`'s bones, on `ground`.
    pub(crate) fn new(rig: IkRig, ground: Arc<PhysicsWorld>, controls: IkControls) -> Self {
        let ik = CharacterIk::new(rig, FootSettings::default(), LookSettings::default());
        Self { ik, ground, controls, presented: None, dt: 0.0, solved: None }
    }

    /// The inputs it runs on.
    pub(crate) fn controls(&self) -> IkControls {
        self.controls
    }

    pub(crate) fn set_controls(&mut self, controls: IkControls) {
        self.controls = controls;
    }

    /// Starts over, for a character that jumped rather than moved: the next solve comes up from the
    /// pose, as secondary motion does.
    pub(crate) fn reset(&mut self) {
        self.ik.reset();
        self.solved = None;
    }

    /// Whether anything it answers for wants the pose evaluated again, its clock now at `ticks` and the
    /// character at `transform`. See the [module docs](self).
    pub(crate) fn wants_evaluation(&mut self, ticks: f64, transform: Mat4) -> bool {
        self.take_time(ticks);
        let Some((controls, place)) = self.solved else { return true };
        if self.dt == 0.0 {
            return controls.weapon != self.controls.weapon;
        }
        controls != self.controls || place != transform || !self.ik.is_settled()
    }

    /// Its clock is now at `ticks`: the next solve takes the time since it was last told.
    pub(crate) fn take_time(&mut self, ticks: f64) {
        let last = self.presented.replace(ticks);
        self.dt = last.map_or(0.0, |last| ((ticks - last).max(0.0) / f64::from(TICK_HZ)) as f32);
    }

    /// Solves `pose`, just sampled, for a character at `transform`. [`CharacterPose::finish`] comes
    /// after. Allocates nothing.
    pub(crate) fn solve(&mut self, pose: &mut CharacterPose, transform: Mat4) {
        let world_from_model = Affine3A::from_mat4(transform);
        self.ik.prepare(pose);
        let mut feet = FeetInput { weight: self.controls.feet, ..FeetInput::default() };
        if let (Some(placement), Some(probes)) =
            (self.ik.feet(), self.ik.foot_probes(pose, &world_from_model))
            && (feet.weight > 0.0 || placement.fade() > 0.0)
        {
            let settings = placement.settings();
            let cast = |at: Vec3| ground_under(&self.ground, at, settings);
            for (ground, probe) in feet.ground.iter_mut().zip(probes) {
                *ground = FootGround { ankle: cast(probe.ankle), ball: probe.ball.and_then(cast) };
            }
        }
        self.ik.solve(
            pose,
            &IkInput {
                dt: self.dt,
                world_from_model,
                feet,
                look: LookInput { target: self.controls.look_target, weight: self.controls.look },
                weapon: self.controls.weapon,
            },
        );
        self.solved = Some((self.controls, transform));
        // A later solve in the same presented time, for a grip, must not take this time again.
        self.dt = 0.0;
    }
}

/// What a foot's probe finds: the world's scenery, from a little above it to a little below. Something
/// the probe starts inside isn't ground under a foot, so a foot in a wall lets go of the ground.
fn ground_under(world: &PhysicsWorld, at: Vec3, settings: &FootSettings) -> Option<GroundHit> {
    let filter = QueryFilter::new(Group::World.mask());
    let from = at + Vec3::Y * settings.reach_up;
    let hit = world.cast_ray(from, Vec3::NEG_Y, settings.reach_up + settings.reach_down, &filter)?;
    (hit.distance > 0.0).then_some(GroundHit { point: hit.point, normal: hit.normal })
}

#[cfg(test)]
mod tests {
    use sapphire_anim::{
        AnimGraph, Clip, GraphBinding, GraphInput, GraphPlayer, Joint, JointTransform, Skeleton,
    };
    use sapphire_assets::AssetId;
    use sapphire_assets::clip::DEFAULT_RATE;
    use sapphire_assets::rig::HumanBone;

    use super::*;
    use crate::anim_graph::{ClipTable, GraphDriver};
    use crate::lookdev::FigureJoints;

    /// A humanoid facing +Z with its hips 0.9 m up and its ankles 0.1 m up, on a floor at 0.
    fn character_skeleton() -> Skeleton {
        const ANKLE: f32 = 0.1;
        let joint = |parent: Option<u16>, translation: [f32; 3]| Joint {
            name: String::new(),
            parent,
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        };
        let shin = (0.9 - ANKLE) * 0.5;
        let joints = [
            joint(None, [0.0, 0.9, 0.0]),
            joint(Some(0), [0.0, 0.8, 0.0]),
            joint(Some(0), [0.1, 0.0, 0.0]),
            joint(Some(2), [0.0, -shin, 0.03]),
            joint(Some(3), [0.0, -shin, -0.03]),
            joint(Some(4), [0.0, -ANKLE * 0.6, 0.13]),
            joint(Some(0), [-0.1, 0.0, 0.0]),
            joint(Some(6), [0.0, -shin, 0.03]),
            joint(Some(7), [0.0, -shin, -0.03]),
            joint(Some(8), [0.0, -ANKLE * 0.6, 0.13]),
        ];
        Skeleton::new(&joints)
    }

    fn character() -> (CharacterPose, IkRig) {
        let bytes = character_skeleton().to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
        let pose = CharacterPose::new(skeleton, &[], &[], Vec::new());
        let rig = rig_of(&pose);
        (pose, rig)
    }

    fn rig_of(pose: &CharacterPose) -> IkRig {
        let bones = [
            (HumanBone::Hips, 0),
            (HumanBone::Head, 1),
            (HumanBone::LeftUpperLeg, 2),
            (HumanBone::LeftLowerLeg, 3),
            (HumanBone::LeftFoot, 4),
            (HumanBone::LeftToes, 5),
            (HumanBone::RightUpperLeg, 6),
            (HumanBone::RightLowerLeg, 7),
            (HumanBone::RightFoot, 8),
            (HumanBone::RightToes, 9),
        ];
        IkRig::new(pose, &bones, Vec3::ZERO, None, 0.0)
    }

    fn world_from(colliders: Vec<StaticCollider>) -> Arc<PhysicsWorld> {
        Arc::new(PhysicsWorld::new(colliders, 0).expect("valid colliders"))
    }

    fn block(id: u32, half: Vec3, middle: Vec3, turn: Quat) -> StaticCollider {
        StaticCollider {
            id: sapphire_physics::ColliderId(id),
            shape: StaticShape::Convex(Shape::Box { half_extents: half }),
            pose: Pose::new(middle, turn),
            memberships: Group::World.mask(),
        }
    }

    fn floor() -> StaticCollider {
        block(0, Vec3::new(30.0, 0.5, 30.0), Vec3::new(0.0, -0.5, 0.0), Quat::IDENTITY)
    }

    /// Solves `pose` on `world` for a character at `transform`, long enough for everything to settle.
    fn stand(world: &Arc<PhysicsWorld>, transform: Mat4) -> (CharacterPose, IkDriver) {
        let (mut pose, rig) = character();
        let mut driver = IkDriver::new(rig, world.clone(), IkControls::default());
        for tick in 0..120 {
            pose.reset();
            assert!(driver.wants_evaluation(f64::from(tick), transform) || driver.ik.is_settled());
            driver.solve(&mut pose, transform);
            pose.finish();
        }
        (pose, driver)
    }

    /// The ground straight below `at` in `world`, however far.
    fn below(world: &PhysicsWorld, at: Vec3) -> (f32, Vec3) {
        let filter = QueryFilter::new(Group::World.mask());
        let hit =
            world.cast_ray(Vec3::new(at.x, 20.0, at.z), Vec3::NEG_Y, 40.0, &filter).expect("there's ground");
        (hit.point.y, hit.normal)
    }

    const FEET: [usize; 2] = [4, 8];

    /// How far `pose`'s feet stand from what's under them, along the ground's normal, less the ankle's
    /// height.
    fn sole_gaps(pose: &CharacterPose, world: &PhysicsWorld, transform: Mat4) -> [f32; 2] {
        FEET.map(|foot| {
            let at = transform.transform_point3(Vec3::from(pose.model()[foot].translation));
            let (height, normal) = below(world, at);
            (at.y - height) * normal.y - 0.1
        })
    }

    /// A character stands on a slope as the controller leaves it, its capsule's round foot hovering;
    /// both soles land on the slope. M2 G1's gap, with the real capsule and casts.
    #[test]
    fn a_capsule_on_a_slope_and_feet_on_the_slope() {
        let ground = IkGround::Slope;
        let world = Arc::new(ground.world());
        let root = IkGround::stand(&world, 0.0, 0.0);
        let hover = CAPSULE_RADIUS * (1.0 / SLOPE_DEGREES.to_radians().cos() - 1.0);
        let over = root - SLOPE_RISE;
        assert!((over - hover).abs() < 2e-3, "the capsule hovers {over} m, as G1 worked out: {hover}");
        let transform = Mat4::from_translation(Vec3::new(0.0, root, 0.0));
        let (pose, driver) = stand(&world, transform);
        for gap in sole_gaps(&pose, &world, transform) {
            assert!(gap.abs() < 3e-3, "{gap} m off the slope");
        }
        assert!(driver.ik.is_settled());
        // And without IK they aren't.
        let (mut plain, _) = character();
        plain.reset();
        plain.finish();
        assert!(sole_gaps(&plain, &world, transform).iter().any(|gap| gap.abs() > 0.03));
    }

    /// A character playing a graph, on `world`, in the rest pose of a clip with nothing in it.
    fn on_a_graph(
        world: &Arc<PhysicsWorld>,
        skeleton: &sapphire_anim::ArchivedSkeleton,
        table: &ClipTable<'_>,
        id: AssetId,
    ) -> GraphDriver {
        let mut graph = AnimGraph::from_document(
            r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
                clips: [(name: "idle", source: "15af86bb-67b3-4208-bc54-1618aabf450b")],
                locomotion: (start: "ground", states: [
                    (name: "ground", plays: Clip(clip: "idle", looping: true)),
                ]),
            ))"#,
        )
        .expect("a good document");
        graph.resolve(|_| id);
        let binding = GraphBinding::new(&graph, skeleton, None, 0, table).expect("it binds");
        let pose = CharacterPose::new(skeleton, &[], &[], Vec::new());
        let rig = rig_of(&pose);
        let joints = FigureJoints::for_test(&pose, 1, 0.0, FEET);
        let player = GraphPlayer::new(Arc::new(binding), pose, 1);
        GraphDriver::new(player, joints).with_ik(IkDriver::new(rig, world.clone(), IkControls::default()))
    }

    /// `--graph` with `--ik on`: the graph's pose takes a solve each frame it moves, over what the graph
    /// made and not over the last solve, and a frozen graph holds every bit, however its target and grip
    /// move: a grip waits for the thaw.
    #[test]
    fn a_graph_takes_ik_over_its_pose_and_a_frozen_one_holds_it() {
        let curb = block(1, Vec3::new(1.0, 0.06, 2.0), Vec3::new(1.03, 0.06, 0.0), Quat::IDENTITY);
        let world = world_from(vec![floor(), curb]);
        let skeleton_bytes = character_skeleton().to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
        let clip = Clip {
            name: "idle".to_owned(),
            skeleton: Some(AssetId::from_bytes([9; 16])),
            joint_count: 10,
            rate: DEFAULT_RATE,
            ticks: 60,
            ..Clip::default()
        };
        let clip_bytes = clip.to_cooked().expect("cooked");
        let id = AssetId::from_bytes([8; 16]);
        let table =
            ClipTable::new([(id, Some(Clip::read(&clip_bytes).expect("a valid clip")), 0)].into_iter());
        let mut driver = on_a_graph(&world, skeleton, &table, id);
        let place = Mat4::IDENTITY;
        let frame = |ticks: f64| GraphInput { ticks, ..GraphInput::default() };

        for tick in 0..120 {
            assert!(driver.pose_frame(&frame(f64::from(tick)), &table, place));
        }
        let gaps = sole_gaps(driver.player.pose(), &world, place);
        assert!(gaps.iter().all(|gap| gap.abs() < 3e-3), "on the curb and the floor: {gaps:?}");
        // A second run's solve on the same clip lands where the first did: nothing compounds.
        let settled = driver.player.pose().locals().to_vec();
        assert!(driver.pose_frame(&frame(120.0), &table, place));
        assert_eq!(driver.player.pose().locals(), settled);

        // Frozen, with everything that could tempt it moved.
        let ik = driver.ik_mut().expect("it has IK");
        ik.set_controls(IkControls {
            look_target: Some(Vec3::new(5.0, 1.5, 0.0)),
            weapon: Some(HeldWeapon {
                place: sapphire_anim::WeaponPlace::Model(Affine3A::IDENTITY),
                main: sapphire_anim::Hand::Right,
                main_grip: None,
                off_grip: None,
            }),
            ..ik.controls()
        });
        assert!(!driver.pose_frame(&frame(120.0), &table, place));
        assert_eq!(driver.player.pose().locals(), settled);

        // With the thaw, the look takes it up.
        assert!(driver.pose_frame(&frame(150.0), &table, place));
        assert_ne!(driver.player.pose().locals(), settled);
    }

    /// In the air the feet go as animated, and a cut starts the ground over as it does the springs.
    #[test]
    fn a_graphs_jump_lets_the_feet_go_and_a_cut_starts_the_ground_over() {
        let world =
            world_from(vec![block(0, Vec3::new(30.0, 0.5, 30.0), Vec3::new(0.0, -0.3, 0.0), Quat::IDENTITY)]);
        let skeleton_bytes = character_skeleton().to_cooked().expect("cooked");
        let skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
        let clip = Clip {
            name: "idle".to_owned(),
            skeleton: Some(AssetId::from_bytes([9; 16])),
            joint_count: 10,
            rate: DEFAULT_RATE,
            ticks: 60,
            ..Clip::default()
        };
        let clip_bytes = clip.to_cooked().expect("cooked");
        let id = AssetId::from_bytes([8; 16]);
        let table =
            ClipTable::new([(id, Some(Clip::read(&clip_bytes).expect("a valid clip")), 0)].into_iter());
        let mut driver = on_a_graph(&world, skeleton, &table, id);
        let place = Mat4::IDENTITY;
        let foot = |driver: &GraphDriver| driver.player.pose().model()[FEET[0]].translation.y;
        for tick in 0..120 {
            driver.pose_frame(&GraphInput { ticks: f64::from(tick), ..GraphInput::default() }, &table, place);
        }
        let standing = foot(&driver);
        assert!(
            (standing - 0.3).abs() < 3e-3,
            "the floor's raised 0.2 m under an ankle 0.1 m up: {standing}"
        );

        let airborne = |ticks: f64| GraphInput {
            ticks,
            locomotion: sapphire_anim::LocomotionInput { grounded: false, ..Default::default() },
            ..GraphInput::default()
        };
        for tick in 120..240 {
            driver.pose_frame(&airborne(f64::from(tick)), &table, place);
        }
        assert!((foot(&driver) - 0.1).abs() < 1e-3, "as animated: {}", foot(&driver));

        for tick in 240..360 {
            driver.pose_frame(&GraphInput { ticks: f64::from(tick), ..GraphInput::default() }, &table, place);
        }
        assert!((foot(&driver) - 0.3).abs() < 3e-3);
        let teleports = driver.player.teleports();
        let cut = GraphInput { ticks: 360.0, cut: true, ..GraphInput::default() };
        assert!(driver.pose_frame(&cut, &table, place));
        assert_eq!(driver.player.teleports(), teleports + 1);
        assert_eq!(driver.springs_saw(), teleports + 1);
    }

    /// One foot up a curb.
    #[test]
    fn a_curb_under_one_foot() {
        let curb = block(1, Vec3::new(1.0, 0.06, 2.0), Vec3::new(1.03, 0.06, 0.0), Quat::IDENTITY);
        let world = world_from(vec![floor(), curb]);
        let transform = Mat4::IDENTITY;
        let (pose, _) = stand(&world, transform);
        assert_eq!(sole_gaps(&pose, &world, transform).map(|gap| gap.abs() < 3e-3), [true, true]);
        assert!(pose.model()[4].translation.y > pose.model()[8].translation.y + 0.1);
    }

    /// Facing up a flight of stairs with one foot on each of two treads, and walking onto them: every
    /// foot stands on the tread it's over, wherever the character stands on the flight.
    #[test]
    fn a_flight_of_stairs() {
        // Rises of 0.15 and treads of 0.3 m, going up toward +Z from z 0.
        let mut colliders = vec![floor()];
        for tread in 0..8 {
            let top = 0.15 * (tread + 1) as f32;
            let half = Vec3::new(2.0, top * 0.5, 0.15);
            let middle = Vec3::new(0.0, top * 0.5, 0.15 + 0.3 * tread as f32);
            colliders.push(block(tread + 1, half, middle, Quat::IDENTITY));
        }
        let world = world_from(colliders);
        let filter = QueryFilter::new(Group::World.mask());
        let shape = Shape::Capsule { half_height: 0.5, radius: CAPSULE_RADIUS };
        for step in 0..24 {
            // Where the capsule rests, as it climbs.
            let z = -0.2 + step as f32 * 0.1;
            let hit = world
                .cast_shape(&shape, Pose::at(Vec3::new(0.0, 8.0, z)), Vec3::NEG_Y, 16.0, &filter)
                .expect("ground");
            let root = 8.0 - hit.distance - CAPSULE_HEIGHT * 0.5;
            let transform = Mat4::from_translation(Vec3::new(0.0, root, z));
            let (pose, _) = stand(&world, transform);
            for foot in FEET {
                let at = transform.transform_point3(Vec3::from(pose.model()[foot].translation));
                let ball = transform.transform_point3(Vec3::from(pose.model()[foot + 1].translation));
                let under = below(&world, at).0.max(below(&world, ball).0);
                assert!((at.y - 0.1 - under).abs() < 3e-3, "at {z}: {foot} at {at}, the tread at {under}");
            }
        }
    }

    /// A character walking through a run of ground doesn't jump, on screen.
    #[test]
    fn feet_stay_smooth_walking_onto_a_step() {
        let curb = block(1, Vec3::new(2.0, 0.08, 2.0), Vec3::new(0.0, 0.08, 2.5), Quat::IDENTITY);
        let world = world_from(vec![floor(), curb]);
        let (mut pose, rig) = character();
        let mut driver = IkDriver::new(rig, world.clone(), IkControls::default());
        let mut last = None;
        for tick in 0..240 {
            let z = tick as f32 * (2.0 / 60.0);
            let transform = Mat4::from_translation(Vec3::new(0.0, 0.0, z));
            pose.reset();
            driver.wants_evaluation(f64::from(tick), transform);
            driver.solve(&mut pose, transform);
            pose.finish();
            let at = transform.transform_point3(Vec3::from(pose.model()[4].translation)).y;
            if let Some(last) = last {
                let moved: f32 = at - last;
                // A 16 cm rise eases in over about 8 ticks, its first a quarter of the way.
                assert!(moved.abs() < 0.05, "tick {tick}: the ankle jumped {moved} m");
            }
            last = Some(at);
        }
    }

    /// Nothing a target does moves a frozen character, so nothing is asked for; a grip is.
    #[test]
    fn its_own_key_under_a_clock_that_stands_and_one_that_runs() {
        let world = Arc::new(IkGround::Flat.world());
        let (mut pose, rig) = character();
        let target = Vec3::new(1.0, 1.5, 2.0);
        let controls = IkControls { look_target: Some(target), ..IkControls::default() };
        let mut driver = IkDriver::new(rig, world, controls);
        let place = Mat4::IDENTITY;
        assert!(driver.wants_evaluation(0.0, place), "its first");
        for tick in 1..240 {
            driver.wants_evaluation(f64::from(tick), place);
            pose.reset();
            driver.solve(&mut pose, place);
            pose.finish();
        }
        // Running, settled, with nothing new: no need.
        assert!(driver.ik.is_settled());
        assert!(!driver.wants_evaluation(240.0, place));
        // Frozen: the target moves, and nothing wants to run, however far.
        for step in 0..4 {
            let moved = target + Vec3::X * (step + 1) as f32;
            driver.set_controls(IkControls { look_target: Some(moved), ..controls });
            assert!(!driver.wants_evaluation(240.0, place), "frozen, step {step}");
        }
        // The clock moving on with a target moved under a held position: it follows.
        assert!(driver.wants_evaluation(241.0, place));
        pose.reset();
        driver.solve(&mut pose, place);
        // The character carried somewhere else, likewise.
        driver.set_controls(controls);
        for tick in 242..500 {
            driver.wants_evaluation(f64::from(tick), place);
            pose.reset();
            driver.solve(&mut pose, place);
        }
        assert!(!driver.wants_evaluation(500.0, place));
        assert!(driver.wants_evaluation(501.0, Mat4::from_translation(Vec3::X)));
    }

    /// Casting from `evaluate_all`'s jobs needs this; a test rather than a comment, since a `Cell` in a
    /// world would lose it silently everywhere but here.
    #[test]
    fn jobs_can_share_a_physics_world() {
        fn sync_and_send<T: Sync + Send>() {}
        sync_and_send::<PhysicsWorld>();
        sync_and_send::<IkDriver>();
    }

    #[test]
    fn grounds_parse_by_name_and_their_blocks_match_their_colliders() {
        for ground in IkGround::ALL {
            assert_eq!(IkGround::from_name(ground.name()), Some(ground));
            let world = ground.world();
            for block in ground.blocks() {
                // The top middle of each block's unit box is on what its colliders say is ground.
                let top = block.transform_point3(Vec3::Y);
                let (height, _) = below(&world, top);
                assert!((height - top.y).abs() < 1e-3, "{ground:?}: {top} against {height}");
            }
        }
        assert_eq!(IkGround::from_name("hill"), None);
    }
}
