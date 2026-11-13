//! `--graph <script>`: the animation graph on the look-dev character, from a scripted input (M2 N3).
//!
//! What a game's frame will do, in the smallest form that shows all of it. A [`Simulation`] runs beside
//! the scene with one actor, a real controller on a floor, stepped once per fixed step; a
//! [`GraphScript`] holds its stick and presses its buttons through the step's actions, and presses its
//! intents, forces its moves and stops its clock inside the step, allocating nothing.
//! [`GraphShow::frame`] turns what the simulation says into the plain [`GraphInput`] a
//! [`GraphDriver`] plays, on the actor's own clock, with the character's transform following the actor:
//! the simulation owns position, and no root motion comes from the clips.
//!
//! - **The combat layer reads the simulation** ([`combat_input`]): `ActorView::moves` names the move and
//!   its playing, and its `elapsed`, less how far behind the latest step this frame draws, is the clip's
//!   position. So on a whole tick a pose reads the same tick as the move's tables, and a hit volume and
//!   the arm agree (`a_pose_and_a_hit_volume_read_the_same_tick`).
//! - **Locomotion reads the controller** ([`locomotion_of`]): its velocity against the actor's facing,
//!   and whether it's on the ground. A frozen clock leaves its state alone, so that holds too.
//! - **A defeat reads the fighter** ([`GraphInput::defeated`], from `Fighter::is_defeated`): its reaction
//!   arrives as any move does, and when the move ends the graph keeps the character down, with no more
//!   to do. A script has no fighter to fell, so [`GraphScript::Defeat`] says so itself.
//! - **IK** (`--ik on`, [`crate::ik`]) runs between the graph and [`GraphPlayer::finish`], feet off in
//!   the air, and its time comes from the same frame ([`GraphDriver::pose_frame`]).
//! - **Spring bones** run beside the pose as they do under a clip (`--springs`, [`GraphDriver`]), after
//!   the graph has finished it, and start over when the player reports a teleport.
//! - **A graph's replacement** arrives from hot reload or live link through
//!   [`Scene::follow_content`](crate::scene::Scene::follow_content), and takes effect at the next state's
//!   start ([`GraphPlayer::replace`]).
//!
//! The distance a script's stick covers is left out of where the character draws
//! ([`GraphShow::transform`]), so a shot keeps it framed: it runs on a treadmill. A move's root motion
//! and a jump's height are the simulation's own and stay in.

mod fixed_presentation;

pub use fixed_presentation::FixedBodyRenderer;

use std::time::Duration;

use sapphire_anim::{
    ArchivedClip, AuthoredHeadLook, ClipSource, CombatInput, FaceLayer, GraphInput, GraphPlayer, GroundTurn,
    LocomotionInput, ReactionInput, SpringBones, SpringFrame, SpringWork, graph_foot_contact,
};
use sapphire_assets::AssetId;
use sapphire_assets::graph::MAX_GRAPH_CLIPS;
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, ActorTransform, ActorView, Buttons, CombatSet, ControllerProfile,
    IntentRing, MoveLibrary, MoveRunner, SimConfig, Simulation, Stick, TickActions,
};
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_moves::MoveSetDocument;
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::{SUBTICKS_PER_TICK, SimDuration, SimTime, Stopwatch};
use sapphire_world::light_rig::CharacterLightRig;

use crate::animate::SpringView;
use crate::extract::{RenderPose, RenderTransform};
use crate::ik::{IkControls, IkDriver};
use crate::lookdev::FigureJoints;
use crate::scene::Figure;

/// The moves the scripts play, as the character's graph names them.
const SHOW_MOVES: &str = include_str!("../../../content/lookdev/character/show.moves.ron");

/// That document, from the repository's root: what live tuning goes by.
#[cfg(feature = "dev")]
pub const SHOW_MOVES_PATH: &str = "content/lookdev/character/show.moves.ron";

const STRIKE: Name = Name::new("strike");
const DODGE: Name = Name::new("dodge");
const DODGE_BACK: Name = Name::new("dodge_back");
const DEFEAT: Name = Name::new("defeat");

/// How far the show's floor reaches each way from the origin, in metres: five minutes' run.
const FLOOR_REACH: f32 = 2_000.0;

/// Which script drives the actor (`--graph`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphScript {
    /// Stands still: an idle, breathing and blinking.
    Stand,
    /// The controller's walk, 2 m/s, straight ahead.
    Walk,
    /// Its run, 6 m/s, straight ahead.
    Run,
    /// Its run at 60° to the character's left, holding its facing.
    Strafe,
    /// A step backwards at 1 m/s, holding its facing.
    Back,
    /// A jump at tick 10, and the landing when it comes.
    Jump,
    /// A strike at tick 10, cancelled into a dodge at tick 26.
    Cancel,
    /// A strike at tick 10 stopped by 8 ticks of hitstop at tick 30, with a dodge forced under it at
    /// tick 34, as a hit's reaction is.
    Hitstop,
    /// A flinch over the idle at tick 10, and another over it at tick 18.
    Flinch,
    /// A strike at tick 10 and a cut at tick 20: no decay, and a teleport for secondary motion.
    Cut,
    /// Felled at tick 20: `defeat` forced as a defeat's reaction is, lasting 120 ticks, and the
    /// character still down when it's over.
    Defeat,
}

impl GraphScript {
    pub const ALL: [Self; 11] = [
        Self::Stand,
        Self::Walk,
        Self::Run,
        Self::Strafe,
        Self::Back,
        Self::Jump,
        Self::Cancel,
        Self::Hitstop,
        Self::Flinch,
        Self::Cut,
        Self::Defeat,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Stand => "stand",
            Self::Walk => "walk",
            Self::Run => "run",
            Self::Strafe => "strafe",
            Self::Back => "back",
            Self::Jump => "jump",
            Self::Cancel => "cancel",
            Self::Hitstop => "hitstop",
            Self::Flinch => "flinch",
            Self::Cut => "cut",
            Self::Defeat => "defeat",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|script| script.name() == name)
    }

    /// The stick it holds: +x along world +X, +y along world +Z, which the character faces. Half a tilt
    /// is the controller's walk, a full tilt its run, and less than half a slower walk.
    fn stick(self) -> Stick {
        match self {
            Self::Walk => Stick::from_axes(0.0, 0.5),
            Self::Run => Stick::from_axes(0.0, 1.0),
            Self::Strafe => Stick::from_axes(0.866, 0.5),
            Self::Back => Stick::from_axes(0.0, -0.25),
            _ => Stick::CENTER,
        }
    }

    /// The buttons it holds at `tick`.
    fn buttons(self, tick: u32) -> Buttons {
        if self == Self::Jump && tick == 10 { Buttons::JUMP } else { Buttons::NONE }
    }

    /// Whether its stick takes the character anywhere.
    fn travels(self) -> bool {
        self.stick() != Stick::CENTER
    }

    /// Whether it faces where it started however it moves, as a lock-on does. The controller turns a body
    /// toward its motion, and no strafe or step back survives that.
    fn holds_facing(self) -> bool {
        matches!(self, Self::Strafe | Self::Back)
    }

    /// The intent pressed at `tick`.
    fn press(self, tick: u32) -> Option<Name> {
        match (self, tick) {
            (Self::Cancel | Self::Hitstop | Self::Cut, 10) => Some(STRIKE),
            (Self::Cancel, 26) => Some(DODGE),
            _ => None,
        }
    }

    /// The hitstop applied at `tick`, in ticks.
    fn hitstop(self, tick: u32) -> Option<u16> {
        (self == Self::Hitstop && tick == 30).then_some(8)
    }

    /// The move forced at `tick`.
    fn forced(self, tick: u32) -> Option<Name> {
        match (self, tick) {
            (Self::Hitstop, 34) => Some(DODGE_BACK),
            (Self::Defeat, 20) => Some(DEFEAT),
            _ => None,
        }
    }

    /// Whether the character's been felled, with `tick` steps taken.
    fn felled(self, tick: u32) -> bool {
        self == Self::Defeat && tick > 20
    }

    /// How many reactions it's asked for by `tick`.
    fn reactions(self, tick: u32) -> u32 {
        if self != Self::Flinch {
            return 0;
        }
        u32::from(tick >= 10) + u32::from(tick >= 18)
    }

    /// Whether it cuts at `tick`.
    fn cut(self, tick: u32) -> bool {
        self == Self::Cut && tick == 20
    }
}

/// What a move looks like to the graph, from what the simulation reads of it, at a frame `behind` ticks
/// of its clock behind the latest step. On a whole tick (`behind` is 0, as every capture's is) this is
/// exactly `elapsed`: the same number the move's tables are read at, so a pose and a hit volume agree.
pub fn combat_input(view: &ActorView, behind: f64) -> Option<CombatInput> {
    view.moves.current.map(|current| CombatInput {
        name: current.name,
        serial: current.serial,
        position: duration_ticks(current.elapsed) - behind,
    })
}

/// `duration` in ticks with a fraction.
pub fn duration_ticks(duration: SimDuration) -> f64 {
    duration.as_subticks() as f64 / f64::from(SUBTICKS_PER_TICK)
}

/// `time` in ticks with a fraction.
pub fn time_ticks(time: SimTime) -> f64 {
    time.as_subticks() as f64 / f64::from(SUBTICKS_PER_TICK)
}

/// What an actor's feet are doing, from its controller's state as of its latest step: its velocity
/// across the ground against its facing, and whether there's ground under it. Whether it landed is an
/// event of a step, which [`GraphShow`] gathers for the frame.
pub fn locomotion_of(view: &ActorView) -> LocomotionInput {
    sapphire_gameplay::body::observe_motion(view).legacy_input()
}

/// Fixed-step air motion samples, kept by the presentation owner even if it draws less than once
/// per simulation tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocomotionSamples(sapphire_gameplay::body::MotionHistory);

impl LocomotionSamples {
    pub fn observe(&mut self, view: &ActorView) {
        self.0.observe(sapphire_gameplay::body::observe_motion(view));
    }

    pub fn input(&mut self, view: &ActorView) -> LocomotionInput {
        self.0.input(sapphire_gameplay::body::observe_motion(view))
    }
}

/// The moves a script plays: a document of the character's, read here since the simulation holds a
/// compiled set and the asset server hands out a view of a cooked one. Live tuning reaches them through
/// `GraphShow::tune`.
fn show_moves() -> sapphire_moves::MoveSet {
    MoveSetDocument::parse(SHOW_MOVES)
        .and_then(|document| document.compile())
        .expect("the show's moves are held good by a test")
}

/// What a script drives, from inside a step.
type Brains<'w, 's> =
    Query<'w, 's, (&'static mut IntentRing, &'static mut MoveRunner, &'static mut EntityClock)>;

/// A script's presses, forced moves and hitstop, as a brain's would be: from inside the step, where
/// they allocate nothing.
fn drive(script: GraphScript, mut tick: Local<'_, u32>, library: &MoveLibrary, mut actors: Brains<'_, '_>) {
    let now = *tick;
    *tick += 1;
    for (mut ring, mut runner, mut clock) in &mut actors {
        let Some(set) = runner.set() else { continue };
        if let Some(hitstop) = script.hitstop(now) {
            clock.0.apply_hitstop(hitstop);
        }
        if let Some(name) = script.forced(now) {
            runner.force(name);
        }
        let intent = script.press(now).and_then(|name| library.current(set)?.intent(name));
        if let Some((_, intent)) = intent {
            ring.press(intent, &clock.0);
        }
    }
}

/// Holds a script's facing where it started, once the controller has turned the body.
fn hold_facing(mut actors: Query<'_, '_, &mut ActorTransform>) {
    for mut transform in &mut actors {
        transform.yaw = 0.0;
    }
}

/// How many clips one character's graphs can need at once: the graph playing's, the one waiting for a
/// state's start, and those of a newest being bound, which may fail and leave the other two as they were.
pub(crate) const MAX_HELD_CLIPS: usize = 3 * MAX_GRAPH_CLIPS;

/// The clips a frame's evaluation reads: the asset server's views of them, gathered per frame so a
/// job can share them.
#[derive(Clone, Copy, Debug)]
pub struct ClipTable<'a> {
    clips: [Option<(AssetId, &'a ArchivedClip)>; MAX_HELD_CLIPS],
    version: u64,
}

impl<'a> ClipTable<'a> {
    /// No clips: what a graph that isn't loaded plays from.
    pub(crate) fn empty() -> Self {
        Self { clips: [None; MAX_HELD_CLIPS], version: 0 }
    }

    /// A borrowed source view at an owner-issued revision. The owner must change the revision
    /// when IDs/order, generations or availability change, including first residency at generation
    /// zero. A different source owner requires revision continuity or a newly bound/reset driver.
    pub fn new(revision: u64, clips: impl Iterator<Item = (AssetId, Option<&'a ArchivedClip>)>) -> Self {
        let mut table = Self { clips: [None; MAX_HELD_CLIPS], version: revision };
        for (slot, (id, clip)) in table.clips.iter_mut().zip(clips) {
            *slot = clip.map(|clip| (id, clip));
        }
        table
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }
}

impl ClipSource for ClipTable<'_> {
    fn clip(&self, id: AssetId) -> Option<&ArchivedClip> {
        self.clips.iter().flatten().find(|(candidate, _)| *candidate == id).map(|(_, clip)| *clip)
    }
}

/// A character playing a graph. See the [module docs](self).
#[derive(Component, Clone, Debug)]
pub struct GraphDriver {
    pub player: GraphPlayer,
    joints: FigureJoints,
    /// Where the character stands with its actor at the origin.
    base: Mat4,
    /// Its hair and cloth: `None` for a rig with none, or left out by [`Self::with_springs_on`].
    springs: Option<SpringBones>,
    /// False for `--springs off`, which a new version of the character keeps to.
    springs_on: bool,
    /// The player's teleports its springs and IK have started over for.
    teleports: u32,
    /// Its feet, look and hands: `None` without `--ik on` or a fight.
    ik: Option<IkDriver>,
    turn: Option<GroundTurn>,
    prepared_takeoff: bool,
    authored_head_look: Option<AuthoredHeadLook>,
    /// Its expressions, over the morph weights its pose makes (M5 C5): `None` for a character with no
    /// presets. What owns the character drives it ([`Self::face_mut`]); idle, it changes nothing.
    face: Option<FaceLayer>,
    /// Whether its graph has made a pose yet.
    posed: bool,
}

impl GraphDriver {
    /// Binds presentation to the simulator's body buffer, without creating a second owner.
    pub fn simulation_body_renderer(
        &mut self,
        clips: &ClipTable<'_>,
        versions: Option<(u32, u32, u32)>,
        sample: sapphire_anim::BodySample,
    ) -> Result<FixedBodyRenderer, String> {
        FixedBodyRenderer::bind_simulation(self, clips, versions, sample)
    }

    pub fn new(player: GraphPlayer, joints: FigureJoints) -> Self {
        Self {
            player,
            joints,
            base: Mat4::IDENTITY,
            springs: None,
            springs_on: true,
            teleports: 0,
            ik: None,
            turn: None,
            prepared_takeoff: false,
            authored_head_look: None,
            face: None,
            posed: false,
        }
    }

    /// This driver, finding the right hand at `joint`.
    pub fn with_right_hand(self, joint: usize) -> Self {
        Self { joints: self.joints.with_right_hand(joint), ..self }
    }

    /// The right hand's joint in the character's space, as its pose has it now. `None` for a rig without
    /// one, and before its graph has made a pose: until then the character holds what it has.
    pub fn right_hand(&self) -> Option<Mat4> {
        let joint = self.joints.right_hand().filter(|_| self.posed)?;
        Some(Mat4::from(self.player.pose().model()[joint]))
    }

    /// Runs its hair and cloth and its IK by `motion` from now on, wherever they've got to.
    pub(crate) fn set_motion(&mut self, motion: &sapphire_anim::CharacterMotion) {
        if let Some(springs) = &mut self.springs {
            springs.set_tuning(motion.springs);
        }
        if let Some(ik) = &mut self.ik {
            ik.set_motion(motion);
        }
    }

    /// Solves its IK by `body` from now on, wherever its feet and gaze have got to.
    pub(crate) fn set_body(&mut self, body: &sapphire_anim::BodyTuning) {
        if let Some(ik) = &mut self.ik {
            ik.set_body(body);
        }
    }

    /// This driver, simulating `springs` beside its pose.
    #[must_use]
    pub(crate) fn with_springs(self, springs: Option<SpringBones>) -> Self {
        Self { springs, ..self }
    }

    /// This driver, leaving its springs out if `on` is false (`--springs off`).
    #[must_use]
    pub fn with_springs_on(self, on: bool) -> Self {
        Self { springs_on: on, springs: self.springs.filter(|_| on), ..self }
    }

    /// This driver, solving `ik` over what the graph makes.
    #[must_use]
    pub fn with_ik(self, ik: IkDriver) -> Self {
        Self { ik: Some(ik), ..self }
    }

    /// This driver, showing `face`'s expressions over its pose's morph weights (M5 C5).
    #[must_use]
    pub fn with_face(self, face: Option<FaceLayer>) -> Self {
        Self { face, ..self }
    }

    /// Its face, for whatever decides what it shows to ask, and to move its clock.
    pub fn face_mut(&mut self) -> Option<&mut FaceLayer> {
        self.face.as_mut()
    }

    #[must_use]
    pub fn face(&self) -> Option<&FaceLayer> {
        self.face.as_ref()
    }

    /// Whether its face changed what it draws since this was last asked.
    pub(crate) fn take_face_moved(&mut self) -> bool {
        self.face.as_mut().is_some_and(FaceLayer::take_moved)
    }

    /// What it draws of its morphs: its pose's weights, with its face over them. The pose's own are left
    /// as they are, since a fighter's are the simulation's (`SolvedBody::morphs`). Allocates nothing.
    pub(crate) fn write_morphs(&self, weights: &mut [f32]) {
        weights.copy_from_slice(self.player.pose().weights());
        if let Some(face) = &self.face {
            face.apply(weights);
        }
    }

    /// Coordinates the hero's solved foot and chest response to fast grounded steering.
    #[must_use]
    pub fn with_ground_turn(self) -> Self {
        Self { turn: Some(GroundTurn::default()), ..self }
    }

    /// Retains the sampled head pose through a whole-body move. The four source ticks mark the
    /// start and end of the rise into authored control, and the start and end of the release.
    /// Eyes keep tracking their target throughout.
    #[must_use]
    pub fn with_authored_head_look(self, movement: Name, retained: f32, source_ticks: [f32; 4]) -> Self {
        let authored_head_look = Some(AuthoredHeadLook::new(movement, retained, source_ticks));
        Self { authored_head_look, ..self }
    }

    /// Its IK, for whatever drives it.
    pub fn ik_mut(&mut self) -> Option<&mut IkDriver> {
        self.ik.as_mut()
    }

    /// Whether it solves IK.
    pub fn has_ik(&self) -> bool {
        self.ik.is_some()
    }

    /// Stands the character at `base` when its actor is at the origin.
    pub(crate) fn at(self, base: Mat4) -> Self {
        Self { base, ..self }
    }

    /// Where the character stands with its actor at the origin.
    pub(crate) fn base(&self) -> Mat4 {
        self.base
    }

    /// Whether it can play into `render`. It may not for a moment, when a new version of the character
    /// lands before the others: the character keeps the pose it has until they fit.
    pub fn fits(&self, render: &RenderPose) -> bool {
        render.palette.len() == self.player.pose().palette_len()
            && render.morph_weights.len() == self.player.pose().weights().len()
    }

    /// The lighting/capsule shape of the same pose published with `render_pose`.
    pub(crate) fn render_figure(&self) -> Figure {
        self.joints.moving(self.player.pose())
    }

    /// What the character's pose looks like now, in the shape its [`RenderPose`] takes.
    pub(crate) fn render_pose(&self) -> RenderPose {
        let pose = self.player.pose();
        let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
        pose.write_palette(&mut palette);
        RenderPose { palette, morph_weights: pose.weights().to_vec() }
    }

    /// Takes over another driver's place, for a new version of the character under a graph that keeps
    /// playing.
    pub fn continue_from(&mut self, other: &Self) {
        self.player.continue_from(&other.player);
        self.base = other.base;
        self.teleports = other.teleports;
        self.springs_on = other.springs_on;
        self.authored_head_look = other.authored_head_look;
        if let Some(look) = &mut self.authored_head_look
            && !self.player.pose().compatible_with(other.player.pose())
        {
            look.reset();
        }
        match (&mut self.springs, &other.springs) {
            (Some(springs), Some(old)) if self.player.pose().compatible_with(other.player.pose()) => {
                springs.continue_from(old)
            }
            (springs, _) if !other.springs_on => *springs = None,
            _ => {}
        }
    }

    /// The teleports its springs have started over for, for tests.
    #[cfg(test)]
    pub(crate) fn springs_saw(&self) -> u32 {
        self.teleports
    }

    /// Whether its springs run this frame, for a character `view` sees or doesn't.
    pub(crate) fn springs_run(
        &mut self,
        view: &SpringView,
        transform: &RenderTransform,
        figure: &Figure,
    ) -> bool {
        self.springs.as_mut().is_some_and(|springs| view.decide(springs, transform, figure))
    }

    fn run_springs(
        &mut self,
        frame: &SpringFrame,
        grounded: bool,
        figure: &Figure,
    ) -> (SpringWork, Duration) {
        let Some(springs) = &mut self.springs else { return (SpringWork::default(), Duration::ZERO) };
        let started = Stopwatch::start();
        // Ease in torso contact as a grounded fall lowers the head into the body's reach.
        let torso_contact = if grounded { ((0.70 - figure.head.y) / 0.36).clamp(0.0, 1.0) } else { 0.0 };
        let work = springs.update_with_torso_contact(self.player.pose_mut(), frame, torso_contact);
        (work, started.elapsed())
    }

    /// The frame's pose, with the character standing at `place` as the ground sees it: the graph, then
    /// IK over what it made, then the rig's constraints and the model pose. Says whether the pose moved.
    ///
    /// When it didn't, the pose already in the character is solved, so nothing's solved again: that
    /// would be a solve over a solve. So a grip moved under a frozen graph waits for the thaw, where
    /// under a clip it shows at once (`IkDriver::wants_evaluation`, which resamples). A freeze is a
    /// few frames, nothing changes grip inside one yet, and hands aren't what a hit's freeze shows.
    /// A game that needs it can ask the player to hand over again. Allocates nothing.
    pub(crate) fn pose_frame(&mut self, input: &GraphInput, clips: &ClipTable<'_>, place: Mat4) -> bool {
        let mut turning = None;
        let mut receiving = None;
        if input.cut || input.locomotion.grounded {
            self.prepared_takeoff = !input.cut
                && (input.locomotion.preparing_jump || input.locomotion.moving_jump_preparation.is_some());
        }
        if let Some(ik) = &mut self.ik {
            ik.take_time(input.ticks);
            let feet = if input.locomotion.grounded { ik.feet_weight() } else { 0.0 };
            ik.set_controls(IkControls { feet, ..ik.controls() });
            if let Some(turn) = &mut self.turn {
                (turning, receiving) = turn.prepare(&self.player, clips, input, place, || ik.turn_support());
            }
        }
        let support = self.ik.as_ref().and_then(IkDriver::ground_support);
        let evaluated = self.player.evaluate_with_support(input, clips, clips.version(), receiving, support);
        let teleports = self.player.teleports();
        let jumped = teleports != self.teleports;
        if jumped {
            self.prepared_takeoff = false;
            self.teleports = teleports;
            if let Some(springs) = &mut self.springs {
                springs.reset();
            }
            if let Some(ik) = &mut self.ik {
                ik.reset();
            }
        }
        if evaluated {
            if let Some(ik) = &mut self.ik {
                let contact = graph_foot_contact(&self.player, clips, place, turning);
                if let Some(turn) = &mut self.turn {
                    turn.apply_spine(self.player.pose_mut(), self.joints.spine(), contact);
                }
                let head_look_weight = self.authored_head_look.as_mut().map_or(1.0, |look| {
                    let sample = self.player.whole_body_combat_position(look.movement(), clips);
                    look.weight(input.ticks, sample, jumped || input.cut)
                });
                ik.set_prepared_release(self.prepared_takeoff && !input.locomotion.grounded);
                ik.set_moving_takeoff(input.locomotion.moving_jump_preparation);
                ik.solve_with_contact(self.player.pose_mut(), place, contact, head_look_weight);
            }
            self.player.finish();
            self.posed = true;
        }
        evaluated
    }
}

/// One character's frame: its driver and everything that follows the skeleton.
pub struct GraphEvaluation<'a> {
    pub driver: Mut<'a, GraphDriver>,
    pub(crate) render: Mut<'a, RenderPose>,
    pub(crate) figure: Mut<'a, Figure>,
    pub(crate) rig: Mut<'a, CharacterLightRig>,
    pub(crate) input: GraphInput,
    /// Where the character is this frame.
    pub(crate) world_from_model: Affine3A,
    /// Where it stands as the ground sees it: on a treadmill, what's drawn rather than how far it's run.
    pub(crate) place: Mat4,
    /// Whether its springs run this frame ([`GraphDriver::springs_run`]).
    pub(crate) springs_run: bool,
    pub evaluated: bool,
    pub spring_work: SpringWork,
    pub spring_time: Duration,
}

impl<'a> GraphEvaluation<'a> {
    /// `driver`'s frame, for a character drawn at `place` and playing `input`, where `view` sees it.
    pub fn new(
        mut driver: Mut<'a, GraphDriver>,
        render: Mut<'a, RenderPose>,
        figure: Mut<'a, Figure>,
        rig: Mut<'a, CharacterLightRig>,
        input: GraphInput,
        place: Mat4,
        view: &SpringView,
    ) -> Self {
        let springs_run = driver.springs_run(view, &RenderTransform(place), &figure);
        Self {
            driver,
            render,
            figure,
            rig,
            input,
            world_from_model: Affine3A::from_mat4(place),
            place,
            springs_run,
            evaluated: false,
            spring_work: SpringWork::default(),
            spring_time: Duration::ZERO,
        }
    }

    /// Evaluates the graph, runs the springs and hands over what changed. Allocates nothing.
    pub fn run(&mut self, clips: &ClipTable<'_>) {
        let driver = &mut *self.driver;
        self.evaluated = driver.pose_frame(&self.input, clips, self.place);
        if self.evaluated {
            let figure = driver.joints.moving(driver.player.pose());
            *self.figure = figure;
            self.rig.head = figure.head;
        }
        let frame = SpringFrame {
            world_from_model: self.world_from_model,
            ticks: self.input.ticks,
            pose_moved: self.evaluated,
            active: self.springs_run,
        };
        (self.spring_work, self.spring_time) =
            driver.run_springs(&frame, self.input.locomotion.grounded, &self.figure);
        // `Extract` watches the pose: a frame with nothing to do must not touch it. A face that moved on
        // its own clock is something to do (M5 C5).
        let face_moved = driver.take_face_moved();
        if self.evaluated || self.spring_work.wrote_pose || face_moved {
            let render = &mut *self.render;
            if self.evaluated || self.spring_work.wrote_pose {
                driver.player.pose().write_palette(&mut render.palette);
            }
            driver.write_morphs(&mut render.morph_weights);
        }
    }
}

/// The simulation a script drives, and what a frame reads of it. See the [module docs](self).
pub struct GraphShow {
    simulation: Simulation,
    actor: ActorId,
    /// The set its moves come from.
    #[cfg(feature = "dev")]
    moves: sapphire_gameplay::MoveSetId,
    script: GraphScript,
    /// Fixed steps taken.
    tick: u32,
    previous: ActorView,
    current: ActorView,
    locomotion: LocomotionSamples,
    /// A cut since the last frame.
    cut: bool,
}

impl GraphShow {
    pub fn new(script: GraphScript) -> Self {
        let mut simulation = Simulation::new(SimConfig::default());
        let floor = StaticCollider {
            id: ColliderId(0),
            shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(FLOOR_REACH, 1.0, FLOOR_REACH) }),
            pose: Pose::at(Vec3::new(0.0, -1.0, 0.0)),
            memberships: Group::World.mask(),
        };
        let world = PhysicsWorld::new(vec![floor], simulation.body_capacity()).expect("one good collider");
        simulation.set_physics_world(world).expect("no actors yet");
        let profile =
            simulation.add_controller_profile(&ControllerProfile::default()).expect("the default profile");
        let set = simulation.add_move_set(show_moves());
        let actor = simulation
            .spawn(ActorSpawn { moves: Some(set), controller: Some(profile), ..ActorSpawn::default() })
            .expect("an empty pool");
        // A spawn starts in the air, and its first tick finds the floor.
        simulation.step(&TickActions::NEUTRAL);
        simulation.add_combat_system(
            CombatSet::Intents,
            move |tick: Local<'_, u32>, library: Res<'_, MoveLibrary>, actors: Brains<'_, '_>| {
                drive(script, tick, &library, actors);
            },
        );
        if script.holds_facing() {
            simulation.add_combat_system(CombatSet::Hits, hold_facing);
        }
        let view = simulation.actor(actor).expect("the actor just stepped in");
        Self {
            simulation,
            actor,
            #[cfg(feature = "dev")]
            moves: set,
            script,
            tick: 0,
            previous: view,
            current: view,
            locomotion: LocomotionSamples::default(),
            cut: false,
        }
    }

    /// Takes a live edit of the document with this file name, if its moves come from that (M2 T2): the
    /// move playing keeps what it started with, and the next start reads this.
    #[cfg(feature = "dev")]
    pub(crate) fn tune(
        &mut self,
        document: &str,
        set: sapphire_moves::MoveSet,
    ) -> Option<(sapphire_gameplay::MoveSetId, sapphire_gameplay::Tuned)> {
        if document != sapphire_gameplay::replay::document_file_name(SHOW_MOVES_PATH) {
            return None;
        }
        Some((self.moves, self.simulation.tune_move_set(self.moves, set)?))
    }

    /// The set that plays the document with this file name, if it plays that.
    #[cfg(feature = "dev")]
    pub fn plays(&self, document: &str) -> Option<sapphire_gameplay::MoveSetId> {
        (document == sapphire_gameplay::replay::document_file_name(SHOW_MOVES_PATH)).then_some(self.moves)
    }

    /// The simulation it runs, for live link's status and live tuning's report.
    #[cfg(feature = "dev")]
    pub fn simulation(&self) -> &Simulation {
        &self.simulation
    }

    /// Who it plays.
    #[cfg(feature = "dev")]
    pub fn cast(&self) -> &[ActorId] {
        std::slice::from_ref(&self.actor)
    }

    /// One fixed step. Allocates nothing, as a game's wouldn't.
    pub(crate) fn step(&mut self) {
        let mut actions = TickActions::NEUTRAL;
        let held = ActorActions { buttons: self.script.buttons(self.tick), stick: self.script.stick() };
        actions.set(self.actor, held).expect("room for one actor");
        self.simulation.step(&actions);
        let Some(view) = self.simulation.actor(self.actor) else { return };
        self.previous = self.current;
        self.current = view;
        self.locomotion.observe(&view);
        self.cut |= self.script.cut(self.tick);
        self.tick += 1;
    }

    /// What this frame asks of the graph, `alpha` of the way from the last step to the latest. A frozen
    /// clock's latest step didn't move it, so it reads the same at every `alpha`.
    pub(crate) fn frame(&mut self, alpha: f32) -> GraphInput {
        let now = time_ticks(self.current.clock_now);
        let step = now - time_ticks(self.previous.clock_now);
        let alpha = if alpha >= 0.0 { f64::from(alpha.min(1.0)) } else { 0.0 };
        let behind = (1.0 - alpha) * step;
        let locomotion = self.locomotion.input(&self.current);
        GraphInput {
            ticks: now - behind,
            locomotion,
            combat: combat_input(&self.current, behind),
            reaction: ReactionInput { name: Name::new("flinch"), count: self.script.reactions(self.tick) },
            cut: std::mem::take(&mut self.cut),
            defeated: self.current.fighter.is_defeated() || self.script.felled(self.tick),
        }
    }

    /// Where the actor is at `alpha`, in the world, less the ground its stick has covered.
    pub(crate) fn transform(&self, alpha: f32) -> Mat4 {
        let alpha = if alpha >= 0.0 { alpha.min(1.0) } else { 0.0 };
        let place = |view: &ActorView| {
            if self.script.travels() { Vec3::new(0.0, view.position.y, 0.0) } else { view.position }
        };
        let (before, after) = (place(&self.previous), place(&self.current));
        let yaw = self.previous.yaw + (self.current.yaw - self.previous.yaw) * alpha;
        Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), before.lerp(after, alpha))
    }

    /// The ground its stick has covered at `alpha`, which [`Self::transform`] leaves out. The treadmill is
    /// for the shot: the character's hair still feels the run.
    pub(crate) fn travel(&self, alpha: f32) -> Vec3 {
        if !self.script.travels() {
            return Vec3::ZERO;
        }
        let alpha = if alpha >= 0.0 { alpha.min(1.0) } else { 0.0 };
        let ground = |view: &ActorView| Vec3::new(view.position.x, 0.0, view.position.z);
        ground(&self.previous).lerp(ground(&self.current), alpha)
    }

    /// The latest view of the actor.
    #[cfg(test)]
    pub(crate) fn view(&self) -> &ActorView {
        &self.current
    }
}

impl std::fmt::Debug for GraphShow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphShow").field("script", &self.script).field("tick", &self.tick).finish()
    }
}

#[cfg(test)]
mod tests {
    use sapphire_anim::{
        AnimGraph, CharacterPose, Clip, GraphBinding, PackedRotation, RotationTrack, Skeleton,
    };
    use sapphire_assets::clip::DEFAULT_RATE;
    use sapphire_assets::skeleton::{Joint, JointTransform};
    use sapphire_moves::PathSample;
    use std::sync::Arc;

    use super::*;

    /// The sandbox's copy of what the cook holds of a document: it's a good one.
    /// What a character's graphs can name at once all reads, down to the last: a graph playing draws the
    /// rest pose in place of a clip that fell off the end.
    #[test]
    fn a_table_has_room_for_three_whole_graphs() {
        let bytes = clip().to_cooked().expect("cooked");
        let cooked = Clip::read(&bytes).expect("a valid clip");
        let id = |index: usize| AssetId::from_bytes([index as u8; 16]);
        let table = ClipTable::new(0, (0..MAX_HELD_CLIPS).map(|index| (id(index), Some(cooked))));
        assert!((0..MAX_HELD_CLIPS).all(|index| table.clip(id(index)).is_some()));
    }

    #[test]
    fn the_shows_moves_compile() {
        let set = show_moves();
        assert!(set.find(STRIKE).is_some() && set.find(DODGE_BACK).is_some());
    }

    #[test]
    fn a_script_is_known_by_its_name() {
        for script in GraphScript::ALL {
            assert_eq!(GraphScript::from_name(script.name()), Some(script));
        }
        assert_eq!(GraphScript::from_name("dance"), None);
    }

    /// A move's start and its cancel read as new serials, with its elapsed the clip's position, on the
    /// ticks the script says.
    #[test]
    fn the_cancel_script_starts_a_strike_and_cancels_it_into_a_dodge() {
        let mut show = GraphShow::new(GraphScript::Cancel);
        let mut seen = Vec::new();
        for tick in 0..40 {
            show.step();
            let input = show.frame(1.0);
            if let Some(combat) = input.combat
                && seen.last().is_none_or(|(_, serial, _)| *serial != combat.serial)
            {
                seen.push((tick, combat.serial, combat.name));
            }
        }
        assert_eq!(seen.iter().map(|&(_, serial, _)| serial).collect::<Vec<_>>(), [1, 2], "{seen:?}");
        assert_eq!(seen[0].2, STRIKE);
        assert_eq!(seen[1].2, DODGE_BACK);
        // The golden scene `lookdev-graph-cancel` lands a frame after the second, on 28 steps.
        assert_eq!((seen[0].0, seen[1].0), (10, 26), "{seen:?}");
    }

    /// The controller's walk and run are what locomotion reads.
    #[test]
    fn locomotion_reads_the_controllers_speeds() {
        for (script, speed) in [(GraphScript::Walk, 2.0), (GraphScript::Run, 6.0), (GraphScript::Stand, 0.0)]
        {
            let mut show = GraphShow::new(script);
            for _ in 0..60 {
                show.step();
            }
            let input = show.frame(1.0);
            assert!((input.locomotion.speed - speed).abs() < 0.05, "{script:?}: {input:?}");
            assert!(input.locomotion.direction.abs() < 1e-3, "{script:?}: {input:?}");
            assert!(input.locomotion.grounded, "{script:?}");
        }
    }

    /// A strafe holds its facing while the controller carries it sideways, which is what a blend space's
    /// strafes need of the direction.
    #[test]
    fn locomotion_reads_a_strafe_against_the_facing() {
        let mut show = GraphShow::new(GraphScript::Strafe);
        for _ in 0..60 {
            show.step();
        }
        let input = show.frame(1.0);
        assert_eq!(show.view().yaw, 0.0);
        assert!((input.locomotion.speed - 6.0).abs() < 0.05, "{input:?}");
        assert!((input.locomotion.direction - 60_f32.to_radians()).abs() < 0.02, "{input:?}");
    }

    /// A jump leaves the ground, says so, rises where the character draws, and lands once.
    #[test]
    fn a_jump_leaves_the_ground_and_lands_once() {
        let mut show = GraphShow::new(GraphScript::Jump);
        let (mut highest, mut left, mut landings) = (0.0_f32, None, 0);
        for tick in 0..200 {
            show.step();
            let input = show.frame(1.0);
            if !input.locomotion.grounded && left.is_none() {
                left = Some(tick);
            }
            landings += u32::from(input.locomotion.landed);
            highest = highest.max(show.transform(1.0).transform_point3(Vec3::ZERO).y);
        }
        assert_eq!(left, Some(10), "off the ground on the tick it pressed");
        assert_eq!(landings, 1);
        assert!(highest > 1.0, "the controller's jump, where the character draws: {highest}");
        assert!(show.frame(1.0).locomotion.grounded);
    }

    #[test]
    fn locomotion_reads_a_walk_backward_as_behind_the_facing() {
        let mut show = GraphShow::new(GraphScript::Back);
        for _ in 0..40 {
            show.step();
        }
        let input = show.frame(1.0);
        assert!((input.locomotion.speed - 1.0).abs() < 0.05, "{input:?}");
        assert!((input.locomotion.direction.abs() - core::f32::consts::PI).abs() < 1e-2, "{input:?}");
    }

    /// A script's walk stays out of where the character draws, and a move's root motion stays in.
    #[test]
    fn the_walk_is_a_treadmill_and_root_motion_is_real() {
        let mut show = GraphShow::new(GraphScript::Run);
        for _ in 0..30 {
            show.step();
        }
        assert!(show.view().position.length() > 1.0);
        assert!(show.transform(1.0).transform_point3(Vec3::ZERO).length() < 1e-3);
        // What the shot leaves out, the character's hair still feels.
        let travel = show.travel(1.0);
        assert_eq!(travel, Vec3::new(show.view().position.x, 0.0, show.view().position.z));
        let mut show = GraphShow::new(GraphScript::Cancel);
        for _ in 0..40 {
            show.step();
        }
        // The dodge's 1.33 m back, among the rest.
        assert!(show.transform(1.0).transform_point3(Vec3::ZERO).z < -0.5);
    }

    /// What the graph on the look-dev stage has for the moves its show plays.
    #[test]
    fn the_characters_graph_plays_the_shows_moves() {
        let text = include_str!("../../../content/lookdev/character/character.animgraph.ron");
        let graph = AnimGraph::from_document(text).expect("a good document");
        let set = show_moves();
        for name in ["strike", "dodge_back"] {
            assert!(set.find(Name::new(name)).is_some(), "{name} isn't in the show's moves");
            assert!(graph.combat.moves.iter().any(|entry| entry.name == name), "{name} isn't in the graph");
        }
    }

    /// Everything the combat slice's character plays and everything a hit forces has a row, so nothing
    /// that reaches the graph through `CombatInput` plays on as locomotion (`unmapped_moves`). The
    /// slice's fighter reacts with its own names, the enemy's document with G3's, and O3's moves will
    /// bring their own.
    #[test]
    fn the_characters_graph_has_a_row_for_all_a_fight_plays() {
        let text = include_str!("../../../content/lookdev/character/character.animgraph.ron");
        let graph = AnimGraph::from_document(text).expect("a good document");
        let has_row = |name: &str| graph.combat.moves.iter().any(|entry| entry.name == name);
        let slice = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
        let slice = MoveSetDocument::parse(slice).expect("it parses");
        assert!(slice.moves.len() >= 14, "{}", slice.moves.len());
        for entry in &slice.moves {
            assert!(has_row(&entry.name), "the slice's {} has no row", entry.name);
        }
        let enemy = include_str!("../../../content/combat/enemy/warrior.moves.ron");
        let enemy = MoveSetDocument::parse(enemy).expect("it parses");
        let reactions = enemy.fighter.as_ref().expect("a fighter").reactions.clone();
        let forced =
            [reactions.light, reactions.medium, reactions.heavy, reactions.poise_break, reactions.defeat];
        for name in forced.iter().flatten().map(String::as_str).chain(["recover"]) {
            assert!(has_row(name), "the enemy's {name} has no row");
        }
        // Both defeat intents stay down. A knockdown plays the separate stand-up sequence.
        let row = |name: &str| graph.combat.moves.iter().find(|entry| entry.name == name).expect(name);
        assert_eq!(row("defeat").clip, "defeat");
        assert_eq!(row("death").clip, "defeat");
        assert_eq!(row("knockdown").clip, "knockdown");
    }

    /// A defeat's move ends and the graph hears that it's still down: `combat` goes and `defeated` stays.
    #[test]
    fn a_felled_character_stays_felled_when_its_move_ends() {
        let mut show = GraphShow::new(GraphScript::Defeat);
        for _ in 0..=20 {
            assert!(!show.frame(1.0).defeated);
            show.step();
        }
        let falling = show.frame(1.0);
        assert!(falling.defeated);
        assert_eq!(falling.combat.expect("the move").name, DEFEAT);
        for _ in 0..125 {
            show.step();
        }
        let down = show.frame(1.0);
        assert!(down.defeated);
        assert_eq!(down.combat, None, "120 ticks on, the move is over");
    }

    /// The seam for a game's fight: a real defeat reads the same.
    #[test]
    fn a_fighter_at_rest_is_not_defeated() {
        let show = GraphShow::new(GraphScript::Stand);
        assert!(!show.view().fighter.is_defeated());
    }

    /// A frozen clock: locomotion holds, and the frame's input repeats exactly, at any alpha.
    #[test]
    fn a_frozen_clock_repeats_the_frames_input() {
        let mut show = GraphShow::new(GraphScript::Hitstop);
        for _ in 0..32 {
            show.step();
        }
        let hit = show.frame(1.0);
        show.step();
        assert_eq!(show.frame(1.0), hit, "stopped, a step later");
        assert_eq!(show.frame(0.25), hit, "stopped, whatever the frame's alpha");
    }

    /// A frame between steps draws behind the latest one, on the same clock as the move.
    #[test]
    fn a_frame_between_steps_reads_behind_the_latest_step() {
        let mut show = GraphShow::new(GraphScript::Cancel);
        for _ in 0..20 {
            show.step();
        }
        let latest = show.frame(1.0);
        let earlier = show.frame(0.25);
        assert_eq!(latest.ticks - earlier.ticks, 0.75);
        let (latest, earlier) = (latest.combat.expect("a strike"), earlier.combat.expect("a strike"));
        assert_eq!(latest.position - earlier.position, 0.75);
    }

    /// The whole of the show's step, after a first run to warm it, allocates nothing: it runs inside the
    /// app's R6 check. Only exact where counts are per thread.
    #[test]
    fn the_shows_step_allocates_nothing() {
        for script in GraphScript::ALL {
            let mut show = GraphShow::new(script);
            for _ in 0..4 {
                show.step();
            }
            let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
                for _ in 0..80 {
                    show.step();
                    std::hint::black_box(show.frame(0.5));
                }
            });
            if sapphire_core::alloc::PER_THREAD_COUNTS {
                assert_eq!(counts.allocations, 0, "{script:?}: {counts:?}");
            }
        }
    }

    /// A hit's stop, end to end: the pose holds every bit with nothing evaluated, a move forced under
    /// it starts no decay until the clock moves, and then that move plays from its own first ticks.
    #[test]
    fn a_hit_stops_the_pose_and_a_move_forced_under_it_lands_when_it_ends() {
        let skeleton_bytes = skeleton().to_cooked().expect("cooked");
        let cooked_skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
        let clip_bytes = clip().to_cooked().expect("cooked");
        let cooked_clip = Clip::read(&clip_bytes).expect("a valid clip");
        let id = AssetId::from_bytes([8; 16]);
        let table = ClipTable::new(0, [(id, Some(cooked_clip))].into_iter());
        let mut graph = AnimGraph::from_document(
            r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
                clips: [(name: "strike", source: "15af86bb-67b3-4208-bc54-1618aabf450b")],
                locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "strike"))]),
                combat: (decay: 5, exit: 5, moves: [
                    (move: "strike", clip: "strike"),
                    (move: "dodge_back", clip: "strike"),
                ]),
            ))"#,
        )
        .expect("a good document");
        graph.resolve(|_| id);
        let binding =
            Arc::new(GraphBinding::new(&graph, cooked_skeleton, None, 0, &table).expect("it binds"));
        let pose = CharacterPose::new(cooked_skeleton, &[], &[], Vec::new());
        let mut player = GraphPlayer::new(binding, pose, 1);
        let mut show = GraphShow::new(GraphScript::Hitstop);

        let frame = |show: &mut GraphShow, player: &mut GraphPlayer| {
            show.step();
            let input = show.frame(1.0);
            let moved = player.evaluate(&input, &table, 0);
            if moved {
                player.finish();
            }
            (moved, input)
        };
        for _ in 0..31 {
            frame(&mut show, &mut player);
        }
        let strike = show.view().moves.current.expect("the strike").serial;
        let held: Vec<[f32; 12]> = player.pose().model().iter().map(|joint| joint.to_cols_array()).collect();
        let transitions = player.stats().transitions;

        // The stop, the forced move inside it included: nothing moves, whatever the input says.
        let mut stopped = 0;
        while show.view().hitstop_ticks > 0 || stopped == 0 {
            let (moved, _) = frame(&mut show, &mut player);
            assert!(!moved, "after {stopped} stopped ticks");
            let now: Vec<[f32; 12]> =
                player.pose().model().iter().map(|joint| joint.to_cols_array()).collect();
            assert_eq!(now, held, "every bit, after {stopped} stopped ticks");
            stopped += 1;
        }
        assert!(stopped >= 6, "{stopped}");
        let dodge = show.view().moves.current.expect("the forced move");
        assert_eq!(dodge.name, DODGE_BACK);
        assert_eq!(dodge.serial, strike + 1, "started under the stop");
        assert_eq!(player.stats().transitions, transitions + 1, "and it's a transition already");

        // The clock runs again: on the new move's clip at its own time, decaying from where it held.
        for tick in 1..=6 {
            let (moved, input) = frame(&mut show, &mut player);
            assert!(moved, "tick {tick}");
            let position = input.combat.expect("the dodge").position;
            let want = turn(position as f32);
            let target = player.target()[1].rotation;
            assert!(target.angle_between(want) < 1e-3, "tick {tick}: the target's the clip at {position}");
        }
        assert!(!player.is_decaying(), "five ticks' decay is over");
        let arm = player.pose().locals()[1].rotation;
        assert_eq!(arm.to_array(), player.target()[1].rotation.to_array(), "and it's on the clip's bits");
    }

    const JOINTS: usize = 3;
    const HAND: usize = 2;
    /// A move with the hand on a path: 30 ticks, the arm turning 1.2 rad about Z at a steady rate.
    const MOVE_TICKS: u16 = 30;

    fn skeleton() -> Skeleton {
        let joint = |parent: Option<u16>, translation| Joint {
            name: "joint".to_owned(),
            parent,
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        };
        Skeleton::new(&[
            joint(None, [0.0, 1.0, 0.0]),
            joint(Some(0), [0.0, 0.4, 0.0]),
            joint(Some(1), [0.6, 0.0, 0.0]),
        ])
    }

    fn turn(tick: f32) -> Quat {
        Quat::from_rotation_z(1.2 * tick / f32::from(MOVE_TICKS))
    }

    /// Keyed every tick, as cooked clips are at their fastest.
    fn clip() -> Clip {
        let ticks: Vec<u16> = (0..=MOVE_TICKS).collect();
        Clip {
            name: "strike".to_owned(),
            skeleton: Some(AssetId::from_bytes([9; 16])),
            joint_count: JOINTS as u32,
            rate: DEFAULT_RATE,
            ticks: MOVE_TICKS,
            rotations: vec![RotationTrack { joint: 1, count: MOVE_TICKS + 1, first: 0 }],
            rotation_keys: ticks
                .iter()
                .map(|&tick| PackedRotation::encode(turn(f32::from(tick)).to_array()))
                .collect(),
            rotation_ticks: ticks,
            ..Clip::default()
        }
    }

    /// `HAND`'s place at `tick`, as a table bakes it: the same pose by hand.
    fn hand_path(tick: u16) -> Vec3 {
        let at_arm = turn(f32::from(tick)) * Vec3::new(0.6, 0.0, 0.0);
        Vec3::new(0.0, 1.4, 0.0) + at_arm
    }

    /// A document with the hand's path keyed every tick, as `compile_with` will bake one from a clip.
    fn set_of_one_move() -> sapphire_moves::MoveSet {
        let keys: Vec<String> = (0..=MOVE_TICKS)
            .map(|tick| {
                let place = hand_path(tick);
                format!("(at: {tick}, offset: ({}, {}, {}))", place.x, place.y, place.z)
            })
            .collect();
        let text = format!(
            r#"(type: "sapphire::moves::MoveSetDocument", version: 1, data: (
                intents: [(name: "go", buffer: 8)],
                neutral: [(intent: "go", move: "strike")],
                moves: [(
                    name: "strike", startup: 10, active: 10, recovery: 10,
                    motion: (paths: [(name: "hand", keys: [{}])]),
                    hitboxes: [(name: "hit", from: 10, to: 20, path: "hand", shape: Sphere(radius: 0.1))],
                )],
            ))"#,
            keys.join(", ")
        );
        MoveSetDocument::parse(&text).expect("it parses").compile().expect("it compiles")
    }

    /// Item 4's whole point: at every tick a hit volume is live, the character's hand in the graph's pose
    /// is where the move's table says the volume is, through slow motion too, whose positions fall
    /// between ticks. The pose comes from the move's `elapsed`, and a table is read at the same number.
    #[test]
    fn a_pose_and_a_hit_volume_read_the_same_tick() {
        let skeleton_bytes = skeleton().to_cooked().expect("cooked");
        let cooked_skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
        let clip_bytes = clip().to_cooked().expect("cooked");
        let cooked_clip = Clip::read(&clip_bytes).expect("a valid clip");
        let id = AssetId::from_bytes([8; 16]);
        let table = ClipTable::new(0, [(id, Some(cooked_clip))].into_iter());
        let mut graph = AnimGraph::from_document(
            r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
                clips: [(name: "strike", source: "15af86bb-67b3-4208-bc54-1618aabf450b")],
                locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "strike"))]),
                combat: (decay: 0, exit: 0, moves: [(move: "strike", clip: "strike")]),
            ))"#,
        )
        .expect("a good document");
        graph.resolve(|_| id);
        for time_scale in [None, Some(500)] {
            let binding =
                Arc::new(GraphBinding::new(&graph, cooked_skeleton, None, 0, &table).expect("it binds"));
            let pose = CharacterPose::new(cooked_skeleton, &[], &[], Vec::new());
            let mut player = GraphPlayer::new(binding, pose, 1);
            let mut simulation = Simulation::new(SimConfig::default());
            let set = simulation.add_move_set(set_of_one_move());
            let actor =
                simulation.spawn(ActorSpawn { moves: Some(set), ..ActorSpawn::default() }).expect("room");
            simulation.step(&TickActions::default());
            if let Some(scale) = time_scale {
                let mut clock = simulation.clock_mut(actor).expect("a live actor");
                clock.0.push_scale(sapphire_time::ScaleSource::Gameplay, scale).expect("room for a scale");
            }
            simulation.press_intent(actor, Name::new("go"));
            let mut volumes = 0;
            for _ in 0..80 {
                simulation.step(&TickActions::default());
                let view = simulation.actor(actor).expect("live");
                let input = GraphInput {
                    ticks: time_ticks(view.clock_now),
                    combat: combat_input(&view, 0.0),
                    ..GraphInput::default()
                };
                player.evaluate(&input, &table, 0);
                player.finish();
                for volume in view.moves.volumes.iter() {
                    // A volume's sweep stops at the end of its window, and slow motion's steps can
                    // land past that: the arm goes on, and there's no volume there to agree with.
                    if view.moves.current.is_none_or(|current| current.elapsed != volume.to) {
                        continue;
                    }
                    let hand = Vec3::from(player.pose().model()[HAND].translation);
                    let PathSample { translation, .. } = volume.end;
                    let want = Vec3::from_array(translation);
                    assert!(
                        hand.distance(want) < 1e-3,
                        "at {:?}, time scale {time_scale:?}: the hand at {hand}, the volume at {want}",
                        volume.to,
                    );
                    volumes += 1;
                }
            }
            assert!(volumes >= 10, "the volume was live for {volumes} steps");
        }
    }
}
