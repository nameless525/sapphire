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
//!   over with it, so a loop's first frame teleports back. A graph (N3) chooses what plays next.

use sapphire_anim::{ArchivedClip, CharacterPose, root_motion};
use sapphire_ecs::prelude::*;
use sapphire_math::Mat4;
use sapphire_time::TICK_HZ;
use sapphire_world::light_rig::CharacterLightRig;

use crate::extract::{RenderPose, RenderTransform};
use crate::lookdev::FigureJoints;
use crate::scene::Figure;

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
}

impl ClipPlayer {
    /// A clip player for a character with this `pose`, its figure found by `joints`.
    pub fn new(pose: CharacterPose, joints: FigureJoints) -> Self {
        Self { pose, joints, start: 0.0, base: Mat4::IDENTITY, root_motion: true, drawn: None }
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
    }

    /// Takes over another player's start, for a new version of the character under a clip that
    /// keeps playing.
    pub fn continue_from(&mut self, other: &Self) {
        self.start = other.start;
        self.base = other.base;
        self.root_motion = other.root_motion;
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
        let mut palette = vec![sapphire_math::Affine3A::IDENTITY; self.pose.palette_len()];
        self.pose.write_palette(&mut palette);
        RenderPose { palette, morph_weights: self.pose.weights().to_vec() }
    }

    /// Where `clip` is at `ticks` on the entity's clock, in the clip's own ticks: looping, so it
    /// wraps at the clip's end. A clip of no ticks is a held pose.
    fn position(&self, clip: &ArchivedClip, ticks: f64) -> f32 {
        let end = f64::from(clip.ticks.to_native());
        if end <= 0.0 {
            return 0.0;
        }
        let position = (ticks - self.start) * f64::from(clip.rate.to_native()) / f64::from(TICK_HZ);
        position.rem_euclid(end) as f32
    }

    /// The position to evaluate at `ticks` for `clip` at `version`, or `None` if that is what the
    /// character already draws, so nothing need move.
    pub fn next_position(&mut self, clip: &ArchivedClip, version: u32, ticks: f64) -> Option<f32> {
        let position = self.position(clip, ticks);
        let drawing = Some((position.to_bits(), version));
        if self.drawn == drawing {
            return None;
        }
        self.drawn = drawing;
        Some(position)
    }
}

/// One character's job this frame: its player and everything that follows the skeleton.
pub(crate) struct Evaluation<'a> {
    pub player: Mut<'a, ClipPlayer>,
    pub render: Mut<'a, RenderPose>,
    pub figure: Mut<'a, Figure>,
    pub rig: Mut<'a, CharacterLightRig>,
    pub transform: Mut<'a, RenderTransform>,
    pub position: f32,
}

impl Evaluation<'_> {
    /// Samples `clip`, finishes the pose and hands it over. Allocates nothing.
    pub fn run(&mut self, clip: &ArchivedClip) {
        let player = &mut *self.player;
        player.pose.sample(clip, self.position);
        player.pose.finish();
        let render = &mut *self.render;
        player.pose.write_palette(&mut render.palette);
        render.morph_weights.copy_from_slice(player.pose.weights());
        let figure = player.joints.moving(&player.pose);
        *self.figure = figure;
        self.rig.head = figure.head;
        if let Some(root) = root_motion(clip, self.position).filter(|_| player.root_motion) {
            self.transform.set_if_neq(RenderTransform(player.base * root.to_mat4()));
        }
    }
}
