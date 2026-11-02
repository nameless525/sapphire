//! A clip playing on a character (M2 N2): the sandbox's way of driving `sapphire_anim` until the
//! animation graph (N3) takes over the choice of clip.
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
//! - **One job per character** ([`Evaluation`], [`sapphire_anim::evaluate_all`]), each writing only
//!   its own entity's components: the pose and morph weights that `Extract` sends when they change,
//!   and what follows the skeleton every frame: its [`Figure`] (the head and feet that its light rig,
//!   face basis, shadow and capsule AO read), its light rig's head, and its transform by the clip's
//!   root motion. See `Scene::animate`.
//! - **Looping** is this harness's: the position wraps at the clip's end, and root motion starts
//!   over with it, so a loop's first frame teleports back, which resets the springs. A graph (N3)
//!   chooses what plays next.
//! - **Spring bones** ([`SpringBones`], M2 N5) live beside the pose and have their own key: they run
//!   whenever their clock moved, whether or not the clip's position did, so hair goes on settling
//!   after a clip holds, until it has stopped. Their time is the pose's too, presented time, so hair
//!   never leads the body. They sleep off screen and past the tier's distance ([`SpringView`]), and
//!   coming back starts from the pose. `--springs off` leaves them out.

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
use crate::lookdev::FigureJoints;
use crate::scene::Figure;

const _: () = assert!(SPRING_TICK_HZ == TICK_HZ, "springs count the ticks clocks do");

/// How far from the middle of its body a character's hair and limbs can reach, in meters: what it
/// takes to be off screen. Hair that wakes as this sphere reaches the frame starts out of sight.
const CHARACTER_REACH: f32 = 1.5;

/// How a tier runs springs.
pub(crate) fn spring_quality(tier: TierId) -> SpringQuality {
    match tier {
        TierId::DesktopHigh | TierId::DesktopLow => SpringQuality::DESKTOP,
        TierId::MobileHigh | TierId::MobileLow => SpringQuality::MOBILE,
    }
}

/// What decides whose springs run this frame: the view and the tier's distance.
pub(crate) struct SpringView {
    frustum: Frustum,
    eye: Vec3,
    quality: SpringQuality,
}

impl SpringView {
    pub fn new(camera: &CameraData, quality: SpringQuality) -> Self {
        let frustum = Frustum::from_view_proj(&(camera.proj * camera.view));
        Self { frustum, eye: camera.position, quality }
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
pub(crate) struct ClipPlayer {
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
        }
    }

    /// This player, simulating `springs` beside its pose.
    #[must_use]
    pub fn with_springs(self, springs: Option<SpringBones>) -> Self {
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

    /// Starts the clip now: at `ticks` on the entity's clock, with the entity at `base`.
    pub fn start_at(&mut self, ticks: f64, base: Mat4) {
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
    }

    /// How many joints its pose has.
    pub fn joint_count(&self) -> usize {
        self.pose.joint_count()
    }

    /// Whether it can play `clip` into `render`. Neither holds for a moment when a new version of
    /// the character or the clip lands before the others: the character keeps the pose it has until
    /// they fit, and the frame's other players go on.
    pub fn fits(&self, clip: &ArchivedClip, render: &RenderPose) -> bool {
        clip.joint_count.to_native() as usize == self.pose.joint_count()
            && render.palette.len() == self.pose.palette_len()
            && render.morph_weights.len() == self.pose.weights().len()
    }

    /// What the character's pose looks like now, in the shape its [`RenderPose`] takes.
    pub fn render_pose(&self) -> RenderPose {
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

    /// The position to evaluate at `ticks` for `clip` at `version`, or `None` if that is what the
    /// character already draws, so nothing need move. A loop's first frame teleports the character
    /// back to where the clip started, if root motion moved it, so its springs start over.
    pub fn next_position(&mut self, clip: &ArchivedClip, version: u32, ticks: f64) -> Option<f32> {
        let (position, lap) = self.position(clip, ticks);
        if lap != self.lap {
            self.lap = lap;
            if self.root_motion
                && let Some(springs) = &mut self.springs
            {
                springs.reset();
            }
        }
        let drawing = Some((position.to_bits(), version));
        if self.drawn == drawing {
            return None;
        }
        self.drawn = drawing;
        Some(position)
    }

    /// Whether its springs run this frame, for a character `view` sees or doesn't, at the tier's
    /// `substeps`. Those that don't go to sleep at once, at no further cost.
    pub fn springs_run(&mut self, view: &SpringView, transform: &RenderTransform, figure: &Figure) -> bool {
        let Some(springs) = &mut self.springs else { return false };
        springs.set_substeps(view.substeps());
        let runs = view.sees(transform, figure, springs.is_running());
        if !runs {
            springs.sleep();
        }
        runs
    }

    /// Whether running springs have anything to do at `ticks` on a frame whose pose didn't move.
    pub fn springs_due(&self, ticks: f64, transform: &RenderTransform) -> bool {
        self.springs.as_ref().is_some_and(|springs| springs.is_due(ticks, &Affine3A::from_mat4(transform.0)))
    }

    /// How many sub-steps a tick its springs run: `None` without any.
    #[cfg(test)]
    pub fn spring_substeps(&self) -> Option<Substeps> {
        self.springs.as_ref().map(SpringBones::substeps)
    }

    /// Its springs, for tests that look at their tails.
    #[cfg(test)]
    pub fn springs(&self) -> Option<&SpringBones> {
        self.springs.as_ref()
    }
}

/// One character's job this frame: its player and everything that follows the skeleton.
pub(crate) struct Evaluation<'a> {
    pub player: Mut<'a, ClipPlayer>,
    pub render: Mut<'a, RenderPose>,
    pub figure: Mut<'a, Figure>,
    pub rig: Mut<'a, CharacterLightRig>,
    pub transform: Mut<'a, RenderTransform>,
    /// Where to evaluate the clip: `None` on a frame only the springs have work in.
    pub position: Option<f32>,
    /// The time this frame draws on the entity's clock, in ticks.
    pub ticks: f64,
    /// Whether its springs run this frame ([`ClipPlayer::springs_run`]).
    pub springs_run: bool,
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
            player.pose.finish();
            let figure = player.joints.moving(&player.pose);
            *self.figure = figure;
            self.rig.head = figure.head;
            if let Some(root) = root_motion(clip, position).filter(|_| player.root_motion) {
                self.transform.set_if_neq(RenderTransform(player.base * root.to_mat4()));
            }
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
