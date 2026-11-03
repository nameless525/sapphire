//! A clip playing on a character (M2 N2): `--clip`, one clip with nothing choosing, which contact
//! sheets and benches want. A game's characters play a graph (N3, [`crate::anim_graph`]).
//!
//! - **[`ClipPlayer`]** is a component: a character's [`CharacterPose`], where the clip started on
//!   its [`EntityClock`], and which joints its [`Figure`] follows. A character without one holds
//!   whatever [`RenderPose`] it has, and nothing here touches it.
//! - **Time is the owner's.** A character's position comes from its clock's presented time
//!   ([`LocalClock::presented_ticks`](sapphire_time::LocalClock::presented_ticks)): `alpha` of the
//!   way from its previous fixed step to its latest, where transforms draw. A clock that didn't
//!   advance (hitstop, pause) gives the position it gave before, and a position already drawn is
//!   not evaluated again: the [`RenderPose`] isn't written, so it isn't extracted and the renderer
//!   doesn't skin. Hitstop freezes a pose at no cost, as it does the turntable.
//! - **A frame's work** ([`Evaluation`]) writes only the character's own components: the pose and morph
//!   weights that `Extract` sends when they change, and what follows the skeleton every frame: its
//!   [`Figure`] (the head and feet that its light rig, face basis, shadow and capsule AO read), its
//!   light rig's head, and its transform by the clip's root motion. See `Scene::animate`.
//! - **Looping** is this harness's: the position wraps at the clip's end, and root motion starts
//!   over with it, so a loop's first frame teleports back, which resets the springs. A graph (N3)
//!   chooses what plays next.
//! - **Spring bones** ([`SpringBones`], M2 N5) live beside the pose and have their own key: they run
//!   whenever their clock moved, whether or not the clip's position did, so hair goes on settling
//!   after a clip holds, until it has stopped. Their time is the pose's too, presented time, so hair
//!   never leads the body. They sleep off screen and past the tier's distance ([`SpringView`]), and
//!   coming back starts from the pose. `--springs off` leaves them out.
//! - **IK** ([`IkDriver`], `--ik on`, M2 N4) runs between sampling and `finish`, so the rig's helpers
//!   and the springs follow it, and everything [`Figure`] reads does too. It has a say in what's
//!   evaluated at all.

use std::time::Duration;

use sapphire_anim::{
    ArchivedClip, CharacterPose, SPRING_TICK_HZ, SpringBones, SpringFrame, SpringQuality, SpringWork,
    Substeps, root_motion,
};
use sapphire_ecs::prelude::*;
use sapphire_math::{Affine3A, Frustum, Mat4, Sphere, Vec3};
use sapphire_render::CameraData;
use sapphire_render::quality::TierId;
use sapphire_time::{Stopwatch, TICK_HZ};
use sapphire_world::light_rig::CharacterLightRig;

use crate::extract::{RenderPose, RenderTransform};
use crate::ik::IkDriver;
use crate::lookdev::FigureJoints;
use crate::scene::Figure;

const _: () = assert!(SPRING_TICK_HZ == TICK_HZ, "springs count the ticks clocks do");

/// How far from the middle of its body a character's hair and limbs can reach, in meters: what it
/// takes to be off screen. Hair that wakes as this sphere reaches the frame starts out of sight.
const CHARACTER_REACH: f32 = 1.5;

/// How a tier runs springs.
pub fn spring_quality(tier: TierId) -> SpringQuality {
    match tier {
        TierId::DesktopHigh | TierId::DesktopLow => SpringQuality::DESKTOP,
        TierId::MobileHigh | TierId::MobileLow => SpringQuality::MOBILE,
    }
}

/// Whether a new lap of `clip` jumps the character: a run in place, as KayKit's are, doesn't, so its hair
/// swings on through the loop.
fn loop_teleports(clip: &ArchivedClip) -> bool {
    /// How far counts as a jump, in meters and radians.
    const JUMP: f32 = 0.01;
    let end = f32::from(clip.ticks.to_native());
    root_motion(clip, 0.0).zip(root_motion(clip, end)).is_some_and(|(start, end)| {
        start.translation.distance(end.translation) > JUMP
            || start.rotation.angle_between(end.rotation) > JUMP
    })
}

/// What decides whose springs run this frame: the view and the tier's distance.
pub struct SpringView {
    frustum: Frustum,
    eye: Vec3,
    quality: SpringQuality,
}

impl SpringView {
    pub(crate) fn new(camera: &CameraData, quality: SpringQuality) -> Self {
        let frustum = Frustum::from_view_proj(&(camera.proj * camera.view));
        Self { frustum, eye: camera.position, quality }
    }

    /// Whether `springs` run this frame, for a character standing as `transform` and `figure` say, at the
    /// tier's substeps. Those that don't go to sleep at once, at no further cost.
    pub(crate) fn decide(
        &self,
        springs: &mut SpringBones,
        transform: &RenderTransform,
        figure: &Figure,
    ) -> bool {
        springs.set_substeps(self.substeps());
        let runs = self.sees(transform, figure, springs.is_running());
        if !runs {
            springs.sleep();
        }
        runs
    }

    /// Whether the springs of a character standing as `transform` and `figure` say should run, given
    /// whether they ran last frame.
    fn sees(&self, transform: &RenderTransform, figure: &Figure, running: bool) -> bool {
        let center = transform.0.transform_point3((figure.head + figure.feet) * 0.5);
        self.quality.in_range(self.eye.distance(center), running)
            && self.frustum.intersects_sphere(&Sphere::new(center, CHARACTER_REACH))
    }

    fn substeps(&self) -> Substeps {
        self.quality.substeps
    }
}

/// Plays a clip on its entity. See the [module docs](self).
#[derive(Component, Debug)]
pub struct ClipPlayer {
    pose: CharacterPose,
    joints: FigureJoints,
    /// Where the clip started on the entity's clock, in ticks.
    start: f64,
    /// Where the entity stood then, which root motion goes on top of.
    base: Mat4,
    /// Whether its transform follows the clip's root motion.
    root_motion: bool,
    /// The position and clip version last evaluated: `None` before the first.
    drawn: Option<(u32, u32)>,
    /// How many times the clip has come round since it started.
    lap: i64,
    /// Its hair and cloth: `None` for a rig with none, or left out by [`Self::with_springs_on`].
    springs: Option<SpringBones>,
    /// False for `--springs off`, which a new version of the character keeps to.
    springs_on: bool,
    /// Loops whose first frame jumped the character back ([`Self::teleports`]).
    teleports: u32,
    /// Its IK, with `--ik on`.
    ik: Option<IkDriver>,
}

impl ClipPlayer {
    /// A clip player for a character with this `pose`, its figure found by `joints`.
    pub fn new(pose: CharacterPose, joints: FigureJoints) -> Self {
        Self {
            pose,
            joints,
            start: 0.0,
            base: Mat4::IDENTITY,
            root_motion: true,
            drawn: None,
            lap: 0,
            springs: None,
            springs_on: true,
            teleports: 0,
            ik: None,
        }
    }

    /// This player, simulating `springs` beside its pose.
    #[must_use]
    pub(crate) fn with_springs(self, springs: Option<SpringBones>) -> Self {
        Self { springs, ..self }
    }

    /// This player, leaving its springs out if `on` is false (`--springs off`).
    #[must_use]
    pub fn with_springs_on(self, on: bool) -> Self {
        Self { springs_on: on, springs: self.springs.filter(|_| on), ..self }
    }

    /// This player, moving its entity by the clip's root motion or not (`--clip-in-place`).
    #[must_use]
    pub fn with_root_motion(self, root_motion: bool) -> Self {
        Self { root_motion, ..self }
    }

    /// This player, solving IK over its clip.
    #[must_use]
    pub fn with_ik(self, ik: IkDriver) -> Self {
        Self { ik: Some(ik), ..self }
    }

    /// Its IK, for whatever drives it.
    pub fn ik_mut(&mut self) -> Option<&mut IkDriver> {
        self.ik.as_mut()
    }

    /// Whether it solves IK.
    pub fn has_ik(&self) -> bool {
        self.ik.is_some()
    }

    /// Its pose, as of its last evaluation.
    pub fn pose(&self) -> &CharacterPose {
        &self.pose
    }

    /// Starts the clip now: at `ticks` on the entity's clock, with the entity at `base`.
    pub(crate) fn start_at(&mut self, ticks: f64, base: Mat4) {
        self.start = ticks;
        self.base = base;
        self.drawn = None;
        self.lap = 0;
    }

    /// Takes over another player's start, for a new version of the character under a clip that
    /// keeps playing.
    pub fn continue_from(&mut self, other: &Self) {
        self.start = other.start;
        self.base = other.base;
        self.root_motion = other.root_motion;
        self.lap = other.lap;
        self.springs_on = other.springs_on;
        match (&mut self.springs, &other.springs) {
            (Some(springs), Some(old)) => springs.continue_from(old),
            (springs, _) if !other.springs_on => *springs = None,
            _ => {}
        }
        self.teleports = other.teleports;
    }

    /// How many times the character has jumped rather than moved: a loop of a clip with root motion
    /// on whose lap really moves its root, which starts its motion over and resets its springs.
    /// [`GraphPlayer::teleports`](sapphire_anim::GraphPlayer::teleports) says the same of a graph.
    pub(crate) fn teleports(&self) -> u32 {
        self.teleports
    }

    /// The right hand's joint in the character's space, as its pose has it now. `None` for a rig
    /// without one.
    pub fn right_hand(&self) -> Option<Mat4> {
        self.joints.right_hand().map(|joint| Mat4::from(self.pose.model()[joint]))
    }

    /// How many joints its pose has.
    pub(crate) fn joint_count(&self) -> usize {
        self.pose.joint_count()
    }

    /// Whether it can play `clip` into `render`. Neither holds for a moment when a new version of
    /// the character or the clip lands before the others: the character keeps the pose it has until
    /// they fit.
    pub fn fits(&self, clip: &ArchivedClip, render: &RenderPose) -> bool {
        clip.joint_count.to_native() as usize == self.pose.joint_count()
            && render.palette.len() == self.pose.palette_len()
            && render.morph_weights.len() == self.pose.weights().len()
    }

    /// What the character's pose looks like now, in the shape its [`RenderPose`] takes.
    pub(crate) fn render_pose(&self) -> RenderPose {
        let mut palette = vec![Affine3A::IDENTITY; self.pose.palette_len()];
        self.pose.write_palette(&mut palette);
        RenderPose { palette, morph_weights: self.pose.weights().to_vec() }
    }

    /// Where `clip` is at `ticks` on the entity's clock, in the clip's own ticks, and how many times
    /// it has come round by then: looping, so it wraps at the clip's end. A clip of no ticks is a
    /// held pose.
    fn position(&self, clip: &ArchivedClip, ticks: f64) -> (f32, i64) {
        let end = f64::from(clip.ticks.to_native());
        if end <= 0.0 {
            return (0.0, 0);
        }
        let position = (ticks - self.start) * f64::from(clip.rate.to_native()) / f64::from(TICK_HZ);
        (position.rem_euclid(end) as f32, position.div_euclid(end) as i64)
    }

    /// The position to evaluate at `ticks` for `clip` at `version`, with the character at
    /// `transform`, or `None` if that is what the character already draws, so nothing need move. A
    /// loop's first frame teleports the character back to where the clip started, if root motion moved
    /// it, so its springs and its IK start over.
    pub(crate) fn next_position(
        &mut self,
        clip: &ArchivedClip,
        version: u32,
        ticks: f64,
        transform: Mat4,
    ) -> Option<f32> {
        let (position, lap) = self.position(clip, ticks);
        if lap != self.lap {
            self.lap = lap;
            if self.root_motion && loop_teleports(clip) {
                self.teleports += 1;
                if let Some(springs) = &mut self.springs {
                    springs.reset();
                }
                if let Some(ik) = &mut self.ik {
                    ik.reset();
                }
            }
        }
        let drawing = Some((position.to_bits(), version));
        let solve = self.ik.as_mut().is_some_and(|ik| ik.wants_evaluation(ticks, transform));
        if self.drawn == drawing && !solve {
            return None;
        }
        self.drawn = drawing;
        Some(position)
    }

    /// Whether its springs run this frame, for a character `view` sees or doesn't.
    pub fn springs_run(&mut self, view: &SpringView, transform: &RenderTransform, figure: &Figure) -> bool {
        self.springs.as_mut().is_some_and(|springs| view.decide(springs, transform, figure))
    }

    /// Whether running springs have anything to do at `ticks` on a frame whose pose didn't move.
    pub(crate) fn springs_due(&self, ticks: f64, transform: &RenderTransform) -> bool {
        self.springs.as_ref().is_some_and(|springs| springs.is_due(ticks, &Affine3A::from_mat4(transform.0)))
    }

    /// How many sub-steps a tick its springs run: `None` without any.
    #[cfg(test)]
    pub(crate) fn spring_substeps(&self) -> Option<Substeps> {
        self.springs.as_ref().map(SpringBones::substeps)
    }

    /// Its springs, for tests that look at their tails.
    #[cfg(test)]
    pub(crate) fn springs(&self) -> Option<&SpringBones> {
        self.springs.as_ref()
    }
}

/// One character's work this frame: its player and everything that follows the skeleton.
pub(crate) struct Evaluation<'a> {
    pub(crate) player: Mut<'a, ClipPlayer>,
    pub(crate) render: Mut<'a, RenderPose>,
    pub(crate) figure: Mut<'a, Figure>,
    pub(crate) rig: Mut<'a, CharacterLightRig>,
    pub(crate) transform: Mut<'a, RenderTransform>,
    /// Where to evaluate the clip: `None` on a frame only the springs have work in.
    pub(crate) position: Option<f32>,
    /// The time this frame draws on the entity's clock, in ticks.
    pub(crate) ticks: f64,
    /// Whether its springs run this frame ([`ClipPlayer::springs_run`]).
    pub(crate) springs_run: bool,
    /// What its springs did.
    pub spring_work: SpringWork,
    pub spring_time: Duration,
}

impl Evaluation<'_> {
    /// Samples `clip`, finishes the pose, runs the springs and hands it over. Allocates nothing.
    pub fn run(&mut self, clip: &ArchivedClip) {
        let player = &mut *self.player;
        if let Some(position) = self.position {
            player.pose.sample(clip, position);
            if let Some(root) = root_motion(clip, position).filter(|_| player.root_motion) {
                self.transform.set_if_neq(RenderTransform(player.base * root.to_mat4()));
            }
            if let Some(ik) = &mut player.ik {
                ik.solve(&mut player.pose, self.transform.0);
            }
            player.pose.finish();
            let figure = player.joints.moving(&player.pose);
            *self.figure = figure;
            self.rig.head = figure.head;
        }
        if let Some(springs) = &mut player.springs {
            let started = Stopwatch::start();
            let frame = SpringFrame {
                world_from_model: Affine3A::from_mat4(self.transform.0),
                ticks: self.ticks,
                pose_moved: self.position.is_some(),
                active: self.springs_run,
            };
            self.spring_work = springs.update(&mut player.pose, &frame);
            self.spring_time = started.elapsed();
        }
        if self.position.is_some() || self.spring_work.wrote_pose {
            let render = &mut *self.render;
            player.pose.write_palette(&mut render.palette);
            render.morph_weights.copy_from_slice(player.pose.weights());
        }
    }
}
