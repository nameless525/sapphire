//! The animation graph (spec §5.13, `docs/m2-plan.md`'s N3): one character's layered state machine,
//! plain and allocation-free once built, evaluated on its owner's clock.
//!
//! [`GraphBinding`] is a [document](sapphire_assets::graph) resolved for one skeleton and rig: indices
//! where the document has names, masks, and additive layers' reference poses. [`GraphPlayer`] plays a
//! binding: each frame the app hands it a plain [`GraphInput`] and it writes a [`CharacterPose`].
//!
//! # The layers, bottom to top
//!
//! 1. **Locomotion:** a state machine, each state one clip or a blend space, moved by
//!    [`LocomotionInput`]. A blend space's synced clips share one phase, so legs don't cross, and their
//!    playback scales to match the feet to the speed.
//! 2. **Combat:** [`CombatInput`] names the move playing and how far into it, and a table in the
//!    document names the clip. A move with no mask replaces everything below, which isn't sampled.
//!    `sapphire_anim` can't see the simulation (spec §5.13), so the app fills this from
//!    `ActorView::moves`.
//! 3. **Additive:** always-on layers such as breathing, and reactions the game starts, each the
//!    difference between a clip and its own first tick.
//! 4. **Morph weights:** whatever the playing clip keys, and a procedural blink.
//!
//! # Transitions
//!
//! A state's start, a move's start or end and a reaction's restart are all one thing: an
//! [inertialized](crate::inertia) transition. The new pose plays at full weight from that frame, with
//! the old pose's offset and velocity decaying out per joint, so nothing fades in. A transition is
//! taken from the pose last drawn, at the time it was drawn, so the first frame after one has already
//! decayed by that frame's time, and a transition inside a decay needs no special case.
//!
//! # Time
//!
//! Everything runs on the owner's clock, read as presented ticks ([`GraphInput::ticks`]), the same
//! number transforms draw at. A frozen clock with the same input is skipped outright ([`GraphPlayer::evaluate`]
//! returns `false`), so hitstop holds a pose at no cost and bit for bit. The key covers every input,
//! so a parameter changed on a frozen clock still shows, in full at once since no time passes to
//! smooth it over, and a transition on a frozen clock records itself, draws nothing different and
//! decays once the clock moves.
//!
//! **A pose and a hit volume agree on the same tick** because a move's clip position is the move's
//! own `elapsed`, tick for tick: the same number that reads its baked tables. [`CombatInput::position`]
//! says how to fill it. During a transition's decay the visible pose is off from them by what's left
//! of the offset, as `docs/m2-plan.md` §3 accepts; [`GraphPlayer::target`] is the pose without it.
//!
//! # A defeat
//!
//! A defeated actor's move ends and nothing more starts, so without more a character would get up when
//! it did. [`GraphInput::defeated`] keeps the last move: it plays on to its clip's end and stays there,
//! breathing and blink stopped, a cut or not. Past the end, with nothing decaying, it's a frozen clock's
//! case, so a body on the floor costs nothing.
//!
//! # Replacing a graph
//!
//! [`GraphPlayer::replace`] takes a new binding for live tuning (T2) and takes effect at the next
//! state's start, as a replaced move set does at the next move's: what's playing goes on as it was, and
//! the next start reads the new one, by name. A move that's playing keeps its clip to its end, so a
//! locomotion state's start under it doesn't take the new graph. A move the graph has no row for isn't
//! playing, so it doesn't keep anything waiting: the new graph comes in before its table's read, which
//! is how a graph that adds a row plays that move at all.
//!
//! # Signals
//!
//! [`GraphPlayer::teleports`] counts the frames the pose jumped on purpose: [`GraphInput::cut`], or a
//! clock that ran backwards. Secondary motion (N5) resets when it changes, whatever frames it missed.
//! Root motion stays with the simulation, so no loop of a graph moves the character.
//!
//! # Cost
//!
//! One frame samples the clips that weigh in, usually one or two and an additive, about 3 µs each at
//! 155 joints on an M5; a transition frame samples the whole target twice more. Nothing allocates once
//! a player exists (`tests/allocations.rs`).

use std::sync::Arc;

use sapphire_assets::AssetId;
use sapphire_assets::clip::{ArchivedClip, ClipError, DEFAULT_RATE};
use sapphire_assets::graph::{AnimGraph, Condition, GraphMask, MAX_BLEND_SAMPLES, StateMotion};
use sapphire_assets::rig::{ArchivedCharacterRig, HumanBone};
use sapphire_assets::skeleton::ArchivedSkeleton;
use sapphire_core::Name;
use sapphire_math::spring::damp;
use sapphire_math::{Transform, Vec2};

use crate::blend::{JointMask, add, blend, make_delta};
use crate::blend_space::{BlendPoint, blend_weights};
use crate::character::CharacterPose;
use crate::inertia::{Inertia, PoseRef, Transition};
use crate::sample::{self, overlay_weights};

/// A tick of the owner's clock, in the units [`ArchivedClip::rate`] counts in.
const CLOCK_RATE: f64 = DEFAULT_RATE as f64;

/// How far on a new pose is sampled to find its velocity at a transition, in ticks.
const VELOCITY_STEP: f32 = 1.0;

/// Where clips come from: the asset server, in an app, read in place with no copy. Looked up by ID so
/// two revisions of a graph can share the clips they share.
pub trait ClipSource: Sync {
    /// The clip with this ID, if it's loaded.
    fn clip(&self, id: AssetId) -> Option<&ArchivedClip>;
}

impl ClipSource for [(AssetId, &ArchivedClip)] {
    fn clip(&self, id: AssetId) -> Option<&ArchivedClip> {
        self.iter().find(|(candidate, _)| *candidate == id).map(|(_, clip)| *clip)
    }
}

/// What a character's feet are doing, in plain numbers the app fills: from a controller's state, or
/// from positions when there is none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocomotionInput {
    /// Metres a second over the ground.
    pub speed: f32,
    /// Which way, relative to where the character faces, in radians about +Y: 0 is straight ahead and
    /// a quarter turn is toward +X, the character's left, as yaw turns.
    pub direction: f32,
    /// Whether it's on the ground, not in the air.
    pub grounded: bool,
    /// Whether it landed since the last frame.
    pub landed: bool,
}

impl Default for LocomotionInput {
    fn default() -> Self {
        Self { speed: 0.0, direction: 0.0, grounded: true, landed: false }
    }
}

/// The move a character is playing, from the simulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombatInput {
    /// The move's name.
    pub name: Name,
    /// Which playing this is, of all the character's. A change of this or the name is a transition: a
    /// party's next member counts its own.
    pub serial: u32,
    /// How far into the move, at the time [`GraphInput::ticks`] says, in the owner clock's ticks: the
    /// move's `elapsed` at the latest step less how far behind that step this frame draws. Below 0
    /// reads as 0: the frame draws the instant before a move the simulation has already started.
    pub position: f64,
}

/// A reaction the game has asked for: a count that goes up by one each time, since a frame can miss
/// a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactionInput {
    /// The reaction asked for last.
    pub name: Name,
    /// How many have been asked for, all told.
    pub count: u32,
}

impl Default for ReactionInput {
    fn default() -> Self {
        Self { name: Name::new(""), count: 0 }
    }
}

/// Everything a frame asks of a graph. Plain data, compared whole to tell whether anything changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GraphInput {
    /// The owner clock's presented time, in ticks with a fraction.
    pub ticks: f64,
    pub locomotion: LocomotionInput,
    pub combat: Option<CombatInput>,
    pub reaction: ReactionInput,
    /// The character jumped, so this frame's pose stands alone: no decay from what was drawn, and
    /// secondary motion starts over. For a frame: a respawn, a camera cut.
    pub cut: bool,
    /// The character is out of the fight for good (`Fighter::is_defeated` in `sapphire_gameplay`).
    /// Nothing more will start, so the move it last played goes on to the end of its clip and stays
    /// there rather than locomotion taking over when the move ends, and the breathing and the blink
    /// stop. Back to false, for a new round, and locomotion takes over as it does after any move.
    pub defeated: bool,
}

impl GraphInput {
    /// With nothing that could poison a comparison or a pose. A clock that isn't a number stands at
    /// `drawn`.
    fn sanitized(self, drawn: f64) -> Self {
        let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
        let mut input = self;
        if !input.ticks.is_finite() {
            input.ticks = drawn;
        }
        input.locomotion.speed = finite(input.locomotion.speed).max(0.0);
        input.locomotion.direction = finite(input.locomotion.direction);
        if let Some(combat) = &mut input.combat
            && !combat.position.is_finite()
        {
            combat.position = 0.0;
        }
        input
    }
}

/// What can't be bound.
#[derive(Debug, thiserror::Error)]
pub enum GraphBindError {
    #[error("clip `{name}` ({id}) isn't loaded")]
    MissingClip { name: String, id: AssetId },
    #[error("clip `{name}` ({id}) doesn't fit this skeleton")]
    WrongSkeleton {
        name: String,
        id: AssetId,
        #[source]
        source: ClipError,
    },
    #[error("mask `{mask}` names {bone:?}, which this character's rig doesn't have")]
    NoSuchBone { mask: String, bone: HumanBone },
    #[error("blink's expression `{0}` isn't in this character's rig")]
    NoSuchExpression(String),
    #[error("{0} refers to a name its document doesn't hold: it didn't pass its check")]
    Unchecked(&'static str),
}

/// A state, resolved.
#[derive(Clone, Debug)]
struct State {
    name: Name,
    motion: Motion,
}

#[derive(Clone, Debug)]
enum Motion {
    Clip { clip: u16, looping: bool },
    Blend(Space),
}

#[derive(Clone, Debug)]
struct Space {
    points: Vec<BlendPoint>,
    samples: Vec<Sample>,
    rate: [f32; 2],
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    clip: u16,
    speed: f32,
    phase: f32,
    sync: bool,
}

#[derive(Clone, Copy, Debug)]
struct StateChange {
    from: u16,
    to: u16,
    when: Condition,
    decay: f32,
}

#[derive(Clone, Copy, Debug)]
struct MoveEntry {
    name: Name,
    clip: u16,
    mask: Option<u16>,
    decay: f32,
}

#[derive(Clone, Debug)]
struct Added {
    name: Name,
    clip: u16,
    mask: Option<u16>,
    weight: f32,
    fade_out: f32,
    restart: f32,
    /// The clip's first tick, which it's the difference from.
    reference: Vec<Transform>,
}

#[derive(Clone, Debug)]
struct BoundBlink {
    /// Which morph targets, and how far.
    targets: Vec<(u16, f32)>,
    period: f64,
    jitter: f64,
    close: f32,
    hold: f32,
    open: f32,
}

/// A graph resolved for one skeleton and rig. Immutable; players share it. Built when a graph loads or
/// is replaced, and allocating then, never after.
#[derive(Clone, Debug)]
pub struct GraphBinding {
    clips: Vec<AssetId>,
    masks: Vec<JointMask>,
    states: Vec<State>,
    changes: Vec<StateChange>,
    start: u16,
    smoothing: f32,
    /// Sorted by name.
    moves: Vec<MoveEntry>,
    combat_exit: f32,
    additive: Vec<Added>,
    reactions: Vec<Added>,
    blink: Option<BoundBlink>,
}

impl GraphBinding {
    /// Resolves `graph`, which has passed its check, for this character: its `skeleton`, its `rig` if
    /// masks or a blink need one, and how many morph targets its mesh has. Every clip must be loaded
    /// and cooked for this skeleton, and every mask's bones and blink's expression in its rig.
    pub fn new(
        graph: &AnimGraph,
        skeleton: &ArchivedSkeleton,
        rig: Option<&ArchivedCharacterRig>,
        morph_targets: usize,
        clips: &(impl ClipSource + ?Sized),
    ) -> Result<Self, GraphBindError> {
        for clip in &graph.clips {
            let name = || clip.name.clone();
            let cooked = clips
                .clip(clip.id)
                .ok_or_else(|| GraphBindError::MissingClip { name: name(), id: clip.id })?;
            cooked.check_skeleton(skeleton).map_err(|source| GraphBindError::WrongSkeleton {
                name: name(),
                id: clip.id,
                source,
            })?;
        }
        let clip_index = |name: &str| -> Result<u16, GraphBindError> {
            graph
                .clips
                .iter()
                .position(|clip| clip.name == name)
                .map(|index| index as u16)
                .ok_or(GraphBindError::Unchecked("a layer"))
        };
        let mask_index = |name: Option<&String>| -> Result<Option<u16>, GraphBindError> {
            name.map(|name| {
                graph
                    .masks
                    .iter()
                    .position(|mask| mask.name == *name)
                    .map(|index| index as u16)
                    .ok_or(GraphBindError::Unchecked("a layer's mask"))
            })
            .transpose()
        };
        let state_index = |name: &str| -> Result<u16, GraphBindError> {
            graph
                .locomotion
                .states
                .iter()
                .position(|state| state.name == name)
                .map(|index| index as u16)
                .ok_or(GraphBindError::Unchecked("a transition"))
        };
        let masks =
            graph.masks.iter().map(|mask| bind_mask(mask, skeleton, rig)).collect::<Result<Vec<_>, _>>()?;
        let mut states = Vec::with_capacity(graph.locomotion.states.len());
        for state in &graph.locomotion.states {
            let motion = match &state.plays {
                StateMotion::Clip { clip, looping } => {
                    Motion::Clip { clip: clip_index(clip)?, looping: *looping }
                }
                StateMotion::Blend(space) => {
                    let points: Vec<BlendPoint> = space
                        .samples
                        .iter()
                        .map(|sample| BlendPoint::new(sample.at[0], sample.at[1]))
                        .collect();
                    let mut samples = Vec::with_capacity(space.samples.len());
                    for (sample, point) in space.samples.iter().zip(&points) {
                        samples.push(Sample {
                            clip: clip_index(&sample.clip)?,
                            speed: point.speed(),
                            phase: sample.phase,
                            sync: sample.sync,
                        });
                    }
                    Motion::Blend(Space { points, samples, rate: space.rate })
                }
            };
            states.push(State { name: Name::intern(&state.name), motion });
        }
        let mut changes = Vec::with_capacity(graph.locomotion.transitions.len());
        for change in &graph.locomotion.transitions {
            changes.push(StateChange {
                from: state_index(&change.from)?,
                to: state_index(&change.to)?,
                when: change.when,
                decay: change.decay as f32,
            });
        }
        let mut moves = Vec::with_capacity(graph.combat.moves.len());
        for entry in &graph.combat.moves {
            moves.push(MoveEntry {
                name: Name::intern(&entry.name),
                clip: clip_index(&entry.clip)?,
                mask: mask_index(entry.mask.as_ref())?,
                decay: entry.decay.unwrap_or(graph.combat.decay) as f32,
            });
        }
        moves.sort_unstable_by_key(|entry| entry.name);
        let reference = |clip_name: &str| -> Result<Vec<Transform>, GraphBindError> {
            let clip = &graph.clips[usize::from(clip_index(clip_name)?)];
            let cooked = clips
                .clip(clip.id)
                .ok_or_else(|| GraphBindError::MissingClip { name: clip.name.clone(), id: clip.id })?;
            let rest = sample::rest_pose(skeleton);
            let mut pose = rest.clone();
            sample::sample(cooked, 0.0, &rest, &mut pose);
            Ok(pose)
        };
        let mut additive = Vec::with_capacity(graph.additive.len());
        for layer in &graph.additive {
            additive.push(Added {
                name: Name::intern(&layer.name),
                clip: clip_index(&layer.clip)?,
                mask: mask_index(layer.mask.as_ref())?,
                weight: layer.weight,
                fade_out: 0.0,
                restart: 0.0,
                reference: reference(&layer.clip)?,
            });
        }
        let mut reactions = Vec::with_capacity(graph.reactions.len());
        for layer in &graph.reactions {
            reactions.push(Added {
                name: Name::intern(&layer.name),
                clip: clip_index(&layer.clip)?,
                mask: mask_index(layer.mask.as_ref())?,
                weight: layer.weight,
                fade_out: layer.fade_out as f32,
                restart: layer.restart as f32,
                reference: reference(&layer.clip)?,
            });
        }
        let blink = graph.blink.as_ref().map(|blink| bind_blink(blink, rig, morph_targets)).transpose()?;
        Ok(Self {
            clips: graph.clips.iter().map(|clip| clip.id).collect(),
            masks,
            states,
            changes,
            start: state_index(&graph.locomotion.start)?,
            smoothing: graph.locomotion.smoothing,
            moves,
            combat_exit: graph.combat.exit as f32,
            additive,
            reactions,
            blink,
        })
    }

    fn move_entry(&self, name: Name) -> Option<u16> {
        self.moves.binary_search_by_key(&name, |entry| entry.name).ok().map(|index| index as u16)
    }

    fn reaction(&self, name: Name) -> Option<u16> {
        self.reactions.iter().position(|entry| entry.name == name).map(|index| index as u16)
    }

    fn state(&self, name: Name) -> Option<u16> {
        self.states.iter().position(|state| state.name == name).map(|index| index as u16)
    }

    fn clip<'a, C: ClipSource + ?Sized>(&self, clips: &'a C, index: u16) -> Option<&'a ArchivedClip> {
        clips.clip(self.clips[usize::from(index)])
    }
}

fn bind_mask(
    mask: &GraphMask,
    skeleton: &ArchivedSkeleton,
    rig: Option<&ArchivedCharacterRig>,
) -> Result<JointMask, GraphBindError> {
    let mut joints = JointMask::nothing(skeleton.len());
    for bone in &mask.bones {
        let joint = rig
            .and_then(|rig| rig.humanoid.iter().find(|entry| entry.bone == bone.bone))
            .map(|entry| usize::from(entry.joint.to_native()))
            .ok_or_else(|| GraphBindError::NoSuchBone { mask: mask.name.clone(), bone: bone.bone })?;
        joints.set_subtree(skeleton, joint, bone.weight);
    }
    Ok(joints)
}

fn bind_blink(
    blink: &sapphire_assets::graph::Blink,
    rig: Option<&ArchivedCharacterRig>,
    morph_targets: usize,
) -> Result<BoundBlink, GraphBindError> {
    let expression = rig
        .and_then(|rig| {
            rig.expressions.iter().find(|expression| expression.name.as_str() == blink.expression)
        })
        .ok_or_else(|| GraphBindError::NoSuchExpression(blink.expression.clone()))?;
    let targets = expression
        .morph_targets
        .iter()
        .map(|target| (target.target.to_native() as usize, target.weight.to_native()))
        .filter(|(target, _)| *target < morph_targets)
        .map(|(target, weight)| (target as u16, weight))
        .collect();
    let [least, most] = blink.every.map(f64::from);
    Ok(BoundBlink {
        targets,
        period: (least + most) * 0.5,
        jitter: (most - least) * 0.5,
        close: blink.close as f32,
        hold: blink.hold as f32,
        open: blink.open as f32,
    })
}

impl BoundBlink {
    /// How shut the eyes are at `ticks`: a pure function of the clock and `seed`, so a frozen clock
    /// holds it and a jump lands where it should. Blinks start `period` apart, each up to `jitter` late,
    /// so the time from one's start to the next's stays within the document's `every`.
    fn amount(&self, ticks: f64, seed: u32) -> f32 {
        if self.period <= 0.0 {
            return 0.0;
        }
        let cell = (ticks / self.period).floor();
        // The cell's own blink, or the last one's running on into it.
        for cell in [cell, cell - 1.0] {
            let start = cell * self.period + self.jitter * jitter_of(cell as i64, seed);
            let amount = self.shape((ticks - start) as f32);
            if amount > 0.0 {
                return amount;
            }
        }
        0.0
    }

    /// How shut, `t` ticks from a blink's start: quick to close, held, slower to open.
    fn shape(&self, t: f32) -> f32 {
        if t < 0.0 {
            return 0.0;
        }
        let ease = |u: f32| u * u * (3.0 - 2.0 * u);
        if t < self.close {
            ease(t / self.close)
        } else if t <= self.close + self.hold {
            1.0
        } else if t < self.close + self.hold + self.open {
            1.0 - ease((t - self.close - self.hold) / self.open)
        } else {
            0.0
        }
    }
}

/// A number from 0 to 1 for this blink of this character. Stateless, so a blink's time doesn't depend
/// on when anyone looked.
fn jitter_of(cell: i64, seed: u32) -> f64 {
    let mut value = (cell as u64) ^ (u64::from(seed) << 32 | u64::from(seed));
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value >> 11) as f64 / (1_u64 << 53) as f64
}

/// A move being played.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Playing {
    entry: u16,
    serial: u32,
    position: f64,
}

/// A reaction being played.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reacting {
    entry: u16,
    started: f64,
}

/// What every layer is doing: everything a graph knows between frames but the poses.
#[derive(Clone, Debug)]
struct Layers {
    /// The locomotion state, and when it started on the clock.
    state: u16,
    started: f64,
    /// Where the synced clips are, 0 to 1.
    phase: f32,
    /// The blend space's position and how fast it's moving, following the input by a spring.
    velocity: Vec2,
    velocity_rate: Vec2,
    /// Each blend sample's weight now.
    blend: [f32; MAX_BLEND_SAMPLES],
    combat: Option<Playing>,
    reaction: Option<Reacting>,
    reaction_count: u32,
    /// [`GraphInput::defeated`], as of the last frame: no breathing and no blink.
    defeated: bool,
}

/// Where in time a target is wanted.
#[derive(Clone, Copy)]
struct Moment {
    ticks: f64,
    phase: f32,
    /// How far into the playing move.
    combat: f64,
}

/// A pose and its morph weights.
#[derive(Clone, Debug)]
struct Frame {
    joints: Vec<Transform>,
    weights: Vec<f32>,
}

impl Frame {
    fn of(joints: &[Transform], weights: &[f32]) -> Self {
        Self { joints: joints.to_vec(), weights: weights.to_vec() }
    }

    fn view(&self) -> PoseRef<'_> {
        PoseRef { joints: &self.joints, weights: &self.weights }
    }

    fn copy(&mut self, other: &Self) {
        self.joints.copy_from_slice(&other.joints);
        self.weights.copy_from_slice(&other.weights);
    }
}

/// The compared key: a frame with the same one has nothing to do.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    input: GraphInput,
    /// The clips' version: a new clip under a frozen clock still shows.
    clips: u32,
    /// How many replacements it's adopted.
    revision: u32,
}

/// What a player has done, all told (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphStats {
    /// Inertialized transitions.
    pub transitions: u32,
    /// Frames a move was playing that the table has no clip for: locomotion plays on.
    pub unmapped_moves: u32,
    /// Frames a clip the graph wanted wasn't loaded: it counts for nothing.
    pub missing_clips: u32,
}

/// Plays a [`GraphBinding`] on one character. See the [module docs](self).
#[derive(Clone, Debug)]
pub struct GraphPlayer {
    graph: Arc<GraphBinding>,
    replacement: Option<Arc<GraphBinding>>,
    revision: u32,
    pose: CharacterPose,
    layers: Layers,
    inertia: Inertia,
    /// The pose drawn last, with no blink: at `drawn`.
    output: Frame,
    /// The pose drawn before it, `earlier_step` ticks earlier: what the drawn pose's velocity comes from.
    earlier: Frame,
    earlier_step: f32,
    /// The target this frame, and a new target's neighbours at a transition.
    target: Frame,
    then: Frame,
    later: Frame,
    /// Scratch for a layer's clip and its delta.
    sampled: Vec<Transform>,
    delta: Vec<Transform>,
    /// When the pose was last drawn, on the clock. `None` before the first frame.
    drawn: Option<f64>,
    key: Option<Key>,
    teleports: u32,
    seed: u32,
    stats: GraphStats,
}

impl GraphPlayer {
    /// A player of `graph` on `pose`, which it owns from now on. `seed` makes this character's blinks
    /// its own. Allocates; nothing after does.
    #[must_use]
    pub fn new(graph: Arc<GraphBinding>, pose: CharacterPose, seed: u32) -> Self {
        let frame = Frame::of(pose.rest(), pose.rest_weights());
        let joints = pose.joint_count();
        let layers = Layers {
            state: graph.start,
            started: 0.0,
            phase: 0.0,
            velocity: Vec2::ZERO,
            velocity_rate: Vec2::ZERO,
            blend: [0.0; MAX_BLEND_SAMPLES],
            combat: None,
            reaction: None,
            reaction_count: 0,
            defeated: false,
        };
        Self {
            inertia: Inertia::new(joints, frame.weights.len()),
            output: frame.clone(),
            earlier: frame.clone(),
            earlier_step: 0.0,
            target: frame.clone(),
            then: frame.clone(),
            later: frame,
            sampled: pose.rest().to_vec(),
            delta: pose.rest().to_vec(),
            graph,
            replacement: None,
            revision: 0,
            pose,
            layers,
            drawn: None,
            key: None,
            teleports: 0,
            seed,
            stats: GraphStats::default(),
        }
    }

    /// Evaluates a frame. Returns whether the pose changed: `false` means the character holds what it
    /// has, bit for bit, so there's nothing to [`Self::finish`] or hand over. `clips_version` must
    /// change when any clip gets a new version. Allocates nothing.
    pub fn evaluate(
        &mut self,
        input: &GraphInput,
        clips: &(impl ClipSource + ?Sized),
        clips_version: u32,
    ) -> bool {
        let input = input.sanitized(self.drawn.unwrap_or(0.0));
        let key = Key { input, clips: clips_version, revision: self.revision };
        if self.key == Some(key) {
            return false;
        }
        if self.lies_still(&input, clips, clips_version) {
            self.drawn = Some(input.ticks);
            self.key = Some(key);
            return false;
        }
        let now = input.ticks;
        let Some(drawn) = self.drawn.filter(|&drawn| !input.cut && now >= drawn) else {
            let jumped = self.drawn.is_some();
            self.start_over(&input, clips);
            self.teleports += u32::from(jumped);
            self.key = Some(Key { revision: self.revision, ..key });
            return true;
        };
        let dt = (now - drawn) as f32;
        self.layers.follow(&input.locomotion, self.graph.smoothing, dt);
        let phase_then = self.layers.phase;
        let decay = self.settle(&input, drawn, clips);
        self.layers.update_blend(&self.graph);
        let speed = self.layers.velocity.length();
        let rate = self.layers.phase_rate(&self.graph, speed, clips);
        self.layers.phase = (phase_then + rate * dt).rem_euclid(1.0);
        let combat = self.layers.combat.map_or(0.0, |playing| playing.position);
        let now_moment = Moment { ticks: now, phase: self.layers.phase, combat };
        let missing = self.layers.write_target(
            &self.graph,
            clips,
            &self.pose,
            now_moment,
            Scratch { sampled: &mut self.sampled, delta: &mut self.delta, out: &mut self.target },
        );
        self.stats.missing_clips += u32::from(missing);
        if let Some(decay) = decay {
            // The new pose where the pose was drawn, and a tick on from there, for its velocity.
            let at_drawn = Moment { ticks: drawn, phase: phase_then, combat: combat - f64::from(dt) };
            self.layers.write_target(
                &self.graph,
                clips,
                &self.pose,
                at_drawn,
                Scratch { sampled: &mut self.sampled, delta: &mut self.delta, out: &mut self.then },
            );
            let later = Moment {
                ticks: drawn + f64::from(VELOCITY_STEP),
                phase: (phase_then + rate * VELOCITY_STEP).rem_euclid(1.0),
                combat: at_drawn.combat.max(0.0) + f64::from(VELOCITY_STEP),
            };
            self.layers.write_target(
                &self.graph,
                clips,
                &self.pose,
                later,
                Scratch { sampled: &mut self.sampled, delta: &mut self.delta, out: &mut self.later },
            );
            self.inertia.begin(&Transition {
                at: drawn,
                decay,
                drawn: self.output.view(),
                earlier: self.earlier.view(),
                drawn_step: self.earlier_step,
                target: self.then.view(),
                later: self.later.view(),
                target_step: VELOCITY_STEP,
            });
            self.stats.transitions += 1;
            if dt <= 0.0 {
                // On a frozen clock the pose is the pose drawn: it keeps its bits, and the decay
                // starts when the clock does.
                self.key = Some(Key { revision: self.revision, ..key });
                return false;
            }
        }
        if dt > 0.0 {
            std::mem::swap(&mut self.output, &mut self.earlier);
            self.earlier_step = dt;
        }
        self.output.copy(&self.target);
        self.inertia.apply(now, &mut self.output.joints, &mut self.output.weights);
        self.hand_over(now);
        self.key = Some(Key { revision: self.revision, ..key });
        true
    }

    /// Whether a defeated character has nothing left to move: a whole-body move has played to the end
    /// of its clip, and nothing's decaying or waiting. Its clock runs on, but no frame of it can change
    /// a bit, so it costs what a frozen clock costs.
    fn lies_still(&self, input: &GraphInput, clips: &(impl ClipSource + ?Sized), clips_version: u32) -> bool {
        let Some(playing) = self.layers.combat else { return false };
        let entry = self.graph.moves[usize::from(playing.entry)];
        input.defeated
            && self.layers.defeated
            && input.combat.is_none()
            && !input.cut
            && entry.mask.is_none()
            && self.layers.reaction.is_none()
            && self.replacement.is_none()
            && !self.is_decaying()
            && self.key.is_some_and(|key| key.clips == clips_version)
            && self.graph.clip(clips, entry.clip).is_some_and(|clip| playing.position >= clip_ticks(clip))
    }

    /// This frame stands alone: the layers take what `input` says at once, with nothing decaying.
    fn start_over(&mut self, input: &GraphInput, clips: &(impl ClipSource + ?Sized)) {
        self.adopt_replacement();
        let now = input.ticks;
        self.layers.velocity = velocity_of(&input.locomotion);
        self.layers.velocity_rate = Vec2::ZERO;
        self.layers.started = now;
        self.layers.reaction = None;
        self.layers.reaction_count = input.reaction.count;
        // A cut changes where it's seen from: it doesn't get a defeated character up.
        let lying = self.layers.combat.filter(|_| input.defeated && input.combat.is_none());
        self.layers.combat = input
            .combat
            .and_then(|combat| {
                let entry = self.graph.move_entry(combat.name)?;
                Some(Playing { entry, serial: combat.serial, position: combat.position })
            })
            .or(lying);
        self.layers.defeated = input.defeated;
        self.layers.update_blend(&self.graph);
        let combat = self.layers.combat.map_or(0.0, |playing| playing.position);
        let moment = Moment { ticks: now, phase: self.layers.phase, combat };
        self.layers.write_target(
            &self.graph,
            clips,
            &self.pose,
            moment,
            Scratch { sampled: &mut self.sampled, delta: &mut self.delta, out: &mut self.target },
        );
        self.inertia.clear();
        self.output.copy(&self.target);
        self.earlier.copy(&self.target);
        self.earlier_step = 0.0;
        self.hand_over(now);
    }

    /// What the frame's layers change into: returns the decay of a transition to record, from what was
    /// drawn at `drawn`.
    fn settle(&mut self, input: &GraphInput, drawn: f64, clips: &(impl ClipSource + ?Sized)) -> Option<f32> {
        let mut decay: Option<f32> = None;
        let shortest =
            |decay: &mut Option<f32>, new: f32| *decay = Some(decay.map_or(new, |old| old.min(new)));
        self.layers.defeated = input.defeated;
        // Combat first: a move's start or end is where a replaced graph comes in. It comes in before its
        // table's asked for a clip, since the row that's wanted may be why it was replaced. Nothing plays a
        // move with no row, so that counts as starting for as long as it runs.
        let had = self.layers.combat;
        let is_playing = |combat: &CombatInput| {
            had.is_some_and(|playing| {
                let name = self.graph.moves[usize::from(playing.entry)].name;
                playing.serial == combat.serial && name == combat.name
            })
        };
        match input.combat {
            Some(combat) if is_playing(&combat) => {
                if let Some(playing) = &mut self.layers.combat {
                    playing.position = combat.position;
                }
            }
            Some(combat) => {
                let adopted = self.adopt_replacement();
                match self.graph.move_entry(combat.name) {
                    Some(entry) => {
                        self.layers.combat =
                            Some(Playing { entry, serial: combat.serial, position: combat.position });
                        shortest(&mut decay, self.graph.moves[usize::from(entry)].decay);
                    }
                    // Locomotion takes over from the move, or plays on in what the new graph has it play.
                    None => {
                        self.stats.unmapped_moves += 1;
                        self.layers.combat = None;
                        if had.is_some() || adopted {
                            shortest(&mut decay, self.graph.combat_exit);
                        }
                    }
                }
            }
            None if had.is_none() => {}
            // The last move of a defeated character: it goes on to its clip's end, and stays.
            None if input.defeated => {
                let adopted = self.adopt_replacement();
                match &mut self.layers.combat {
                    Some(playing) => {
                        playing.position += input.ticks - drawn;
                        if adopted {
                            shortest(&mut decay, self.graph.combat_exit);
                        }
                    }
                    // The new graph has no clip for it.
                    None => shortest(&mut decay, self.graph.combat_exit),
                }
            }
            None => {
                self.adopt_replacement();
                self.layers.combat = None;
                shortest(&mut decay, self.graph.combat_exit);
            }
        }
        // Locomotion, whether it shows or not, so it's where it should be when a move ends.
        let speed = self.layers.velocity.length();
        let change = self
            .graph
            .changes
            .iter()
            .filter(|change| change.from == self.layers.state)
            .find(|change| {
                self.layers.holds(change.when, &input.locomotion, speed, input.ticks, &self.graph, clips)
            })
            .copied();
        if let Some(change) = change {
            let to = self.graph.states[usize::from(change.to)].name;
            // A move that's playing keeps its clip to its end: no new graph under it.
            if self.layers.combat.is_none() {
                self.adopt_replacement();
            }
            self.layers.state = self.graph.state(to).unwrap_or(self.graph.start);
            self.layers.started = drawn;
            let hidden = self
                .layers
                .combat
                .is_some_and(|playing| self.graph.moves[usize::from(playing.entry)].mask.is_none());
            if !hidden {
                shortest(&mut decay, change.decay);
            }
        }
        // Reactions: the first has nothing to fade in, but one starting over another jumps.
        if input.reaction.count != self.layers.reaction_count {
            self.layers.reaction_count = input.reaction.count;
            if let Some(entry) = self.graph.reaction(input.reaction.name) {
                if let Some(playing) = self.layers.reaction {
                    shortest(&mut decay, self.graph.reactions[usize::from(playing.entry)].restart);
                }
                self.layers.reaction = Some(Reacting { entry, started: drawn });
            }
        } else if let Some(playing) = self.layers.reaction {
            let entry = &self.graph.reactions[usize::from(playing.entry)];
            let length = self.graph.clip(clips, entry.clip).map_or(0.0, clip_ticks);
            if input.ticks - playing.started >= length {
                self.layers.reaction = None;
                if entry.fade_out <= 0.0 {
                    shortest(&mut decay, entry.restart);
                }
            }
        }
        decay
    }

    /// Takes the waiting replacement, if any: what's playing finds its place in it by name. Returns
    /// whether there was one, so the caller can decay what it may have changed. Allocates nothing.
    fn adopt_replacement(&mut self) -> bool {
        let Some(new) = self.replacement.take() else { return false };
        let old = std::mem::replace(&mut self.graph, new);
        let state = old.states[usize::from(self.layers.state)].name;
        self.layers.state = self.graph.state(state).unwrap_or(self.graph.start);
        self.layers.combat = self.layers.combat.and_then(|playing| {
            let name = old.moves[usize::from(playing.entry)].name;
            Some(Playing { entry: self.graph.move_entry(name)?, ..playing })
        });
        self.layers.reaction = self.layers.reaction.and_then(|playing| {
            let name = old.reactions[usize::from(playing.entry)].name;
            Some(Reacting { entry: self.graph.reaction(name)?, ..playing })
        });
        self.revision += 1;
        true
    }

    /// Writes the drawn pose into the character, with the blink on top.
    fn hand_over(&mut self, now: f64) {
        self.pose.locals_mut().copy_from_slice(&self.output.joints);
        let weights = self.pose.weights_mut();
        weights.copy_from_slice(&self.output.weights);
        if let Some(blink) = self.graph.blink.as_ref().filter(|_| !self.layers.defeated) {
            let amount = blink.amount(now, self.seed);
            if amount > 0.0 {
                for &(target, share) in &blink.targets {
                    let weight = &mut weights[usize::from(target)];
                    *weight = (*weight + amount * share).min(1.0);
                }
            }
        }
        self.drawn = Some(now);
    }

    /// Runs the rig's constraints and local-to-model over what [`Self::evaluate`] wrote, after whatever
    /// else solved it ([`Self::pose_mut`]). Allocates nothing.
    pub fn finish(&mut self) {
        self.pose.finish();
    }

    /// The character's pose.
    #[must_use]
    pub fn pose(&self) -> &CharacterPose {
        &self.pose
    }

    /// The character's pose, for what solves on it between [`Self::evaluate`] and [`Self::finish`].
    /// What it writes doesn't count as drawn: the graph's transitions start from what the graph made.
    pub fn pose_mut(&mut self) -> &mut CharacterPose {
        &mut self.pose
    }

    /// The layers' pose this frame, before any decay: what a move's tables were baked from, where a
    /// transition has left the visible pose off them.
    #[must_use]
    pub fn target(&self) -> &[Transform] {
        &self.target.joints
    }

    /// Whether a transition's decay is still running at the time last drawn.
    #[must_use]
    pub fn is_decaying(&self) -> bool {
        self.drawn.is_some_and(|drawn| self.inertia.is_running(drawn))
    }

    /// How many times the pose has jumped on purpose. See the [module docs](self).
    #[must_use]
    pub fn teleports(&self) -> u32 {
        self.teleports
    }

    /// What it's done, all told.
    #[must_use]
    pub fn stats(&self) -> GraphStats {
        self.stats
    }

    /// The locomotion state playing, by name.
    #[must_use]
    pub fn state(&self) -> Name {
        self.graph.states[usize::from(self.layers.state)].name
    }

    /// The graph playing.
    #[must_use]
    pub fn graph(&self) -> &Arc<GraphBinding> {
        &self.graph
    }

    /// Replaces the graph, from the next state's start. See the [module docs](self). A graph waiting
    /// already is dropped for this one.
    pub fn replace(&mut self, graph: Arc<GraphBinding>) {
        self.replacement = Some(graph);
    }

    /// Whether a replacement is waiting for a state to start.
    #[must_use]
    pub fn has_replacement(&self) -> bool {
        self.replacement.is_some()
    }

    /// The clips this player may still read: its graph's, and from the next state's start the waiting
    /// one's. A clip source has to keep these, or what's playing draws the rest pose in their place.
    pub fn clips_in_use(&self) -> impl Iterator<Item = AssetId> + '_ {
        let waiting = self.replacement.iter().flat_map(|graph| graph.clips.iter());
        self.graph.clips.iter().chain(waiting).copied()
    }

    /// Takes over another player's place, for a new version of the character under a graph that keeps
    /// playing: what its layers are doing, and if the joints are the same ones, its pose and decay.
    pub fn continue_from(&mut self, other: &Self) {
        self.layers = other.layers.clone();
        // Its indices are its graph's, so they find their places in this one's by name, at once: the
        // joints may be others.
        self.replacement = Some(std::mem::replace(&mut self.graph, other.graph.clone()));
        self.adopt_replacement();
        self.drawn = other.drawn;
        self.teleports = other.teleports;
        self.stats = other.stats;
        self.key = None;
        let same = self.output.joints.len() == other.output.joints.len()
            && self.output.weights.len() == other.output.weights.len();
        if same {
            self.output.copy(&other.output);
            self.earlier.copy(&other.earlier);
            self.earlier_step = other.earlier_step;
            self.inertia = other.inertia.clone();
        } else {
            self.inertia.clear();
            self.earlier_step = 0.0;
        }
    }
}

fn velocity_of(locomotion: &LocomotionInput) -> Vec2 {
    let (sine, cosine) = locomotion.direction.sin_cos();
    Vec2::new(sine, cosine) * locomotion.speed
}

/// How long `clip` is on the owner's clock, in ticks.
fn clip_ticks(clip: &ArchivedClip) -> f64 {
    let rate = f64::from(clip.rate.to_native());
    if rate > 0.0 { f64::from(clip.ticks.to_native()) * CLOCK_RATE / rate } else { 0.0 }
}

/// `clip`'s position `ticks` of the owner's clock from its start, in its own ticks.
fn clip_position(clip: &ArchivedClip, ticks: f64) -> f64 {
    ticks.max(0.0) * f64::from(clip.rate.to_native()) / CLOCK_RATE
}

/// [`clip_position`], wrapping at the clip's end. A clip of no ticks is a held pose.
fn looped_position(clip: &ArchivedClip, ticks: f64) -> f32 {
    let end = f64::from(clip.ticks.to_native());
    if end <= 0.0 {
        return 0.0;
    }
    clip_position(clip, ticks).rem_euclid(end) as f32
}

/// The buffers a target writes into.
struct Scratch<'a> {
    sampled: &'a mut [Transform],
    delta: &'a mut [Transform],
    out: &'a mut Frame,
}

impl Layers {
    /// The blend space's position goes after `input`'s. A frozen clock has no time to smooth over, so
    /// it takes the value at once, or a parameter changed under it wouldn't show.
    fn follow(&mut self, input: &LocomotionInput, half_life: f32, dt: f32) {
        let target = velocity_of(input);
        if dt <= 0.0 {
            self.velocity = target;
            self.velocity_rate = Vec2::ZERO;
        } else {
            self.velocity = damp(self.velocity, target, &mut self.velocity_rate, half_life, dt);
        }
    }

    /// Each blend sample's weight at the position, if the state's a blend.
    fn update_blend(&mut self, graph: &GraphBinding) {
        if let Motion::Blend(space) = &graph.states[usize::from(self.state)].motion {
            let at = BlendPoint::new(self.velocity.x, self.velocity.y);
            blend_weights(&space.points, at, &mut self.blend);
        }
    }

    /// How fast the shared phase moves at this speed, in cycles a tick.
    fn phase_rate(&self, graph: &GraphBinding, speed: f32, clips: &(impl ClipSource + ?Sized)) -> f32 {
        let Motion::Blend(space) = &graph.states[usize::from(self.state)].motion else { return 0.0 };
        let (mut total, mut cycles, mut authored) = (0.0, 0.0, 0.0);
        for (sample, &weight) in space.samples.iter().zip(&self.blend) {
            if weight <= 0.0 || !sample.sync {
                continue;
            }
            let Some(clip) = graph.clip(clips, sample.clip) else { continue };
            let length = clip_ticks(clip) as f32;
            if length > 0.0 {
                total += weight;
                cycles += weight / length;
                authored += weight * sample.speed;
            }
        }
        if total <= 0.0 {
            return 0.0;
        }
        let [least, most] = space.rate;
        let scale = if authored > 0.0 { (speed * total / authored).clamp(least, most) } else { 1.0 };
        cycles / total * scale
    }

    /// Whether `when` holds for the state playing.
    fn holds(
        &self,
        when: Condition,
        input: &LocomotionInput,
        speed: f32,
        now: f64,
        graph: &GraphBinding,
        clips: &(impl ClipSource + ?Sized),
    ) -> bool {
        match when {
            Condition::Airborne => !input.grounded,
            Condition::Grounded => input.grounded,
            Condition::Landed => input.landed,
            Condition::Moving(limit) => speed >= limit,
            Condition::Stopped(limit) => speed < limit,
            Condition::Finished => match graph.states[usize::from(self.state)].motion {
                Motion::Clip { clip, looping: false } => {
                    graph.clip(clips, clip).is_some_and(|clip| now - self.started >= clip_ticks(clip))
                }
                _ => false,
            },
        }
    }

    /// The layers' pose at `moment`, into `scratch.out`. Returns whether a clip it wanted wasn't there.
    /// Reads nothing but its arguments and writes nothing but the scratch, so a transition can ask for
    /// the same layers at other times. Allocates nothing.
    fn write_target(
        &self,
        graph: &GraphBinding,
        clips: &(impl ClipSource + ?Sized),
        pose: &CharacterPose,
        moment: Moment,
        scratch: Scratch<'_>,
    ) -> bool {
        let Scratch { sampled, delta, out } = scratch;
        let rest = pose.rest();
        let mut missing = false;
        out.weights.copy_from_slice(pose.rest_weights());
        let entry = self.combat.map(|playing| graph.moves[usize::from(playing.entry)]);
        // A whole-body move replaces locomotion, which isn't sampled.
        match entry {
            Some(MoveEntry { mask: None, .. }) => {}
            _ => missing |= self.write_locomotion(graph, clips, rest, moment, sampled, out),
        }
        if let Some(entry) = entry {
            match graph.clip(clips, entry.clip) {
                Some(clip) => {
                    let position = clip_position(clip, moment.combat) as f32;
                    match entry.mask {
                        None => sample::sample(clip, position, rest, &mut out.joints),
                        Some(mask) => {
                            sample::sample(clip, position, rest, sampled);
                            blend(&mut out.joints, sampled, 1.0, Some(&graph.masks[usize::from(mask)]));
                        }
                    }
                    overlay_weights(clip, position, &mut out.weights);
                }
                None => {
                    missing = true;
                    if entry.mask.is_none() {
                        out.joints.copy_from_slice(rest);
                    }
                }
            }
        }
        for layer in graph.additive.iter().filter(|_| !self.defeated) {
            match graph.clip(clips, layer.clip) {
                Some(clip) => {
                    let position = looped_position(clip, moment.ticks);
                    add_layer(
                        graph,
                        clip,
                        position,
                        layer,
                        layer.weight,
                        rest,
                        sampled,
                        delta,
                        &mut out.joints,
                    );
                }
                None => missing = true,
            }
        }
        if let Some(playing) = self.reaction {
            let layer = &graph.reactions[usize::from(playing.entry)];
            match graph.clip(clips, layer.clip) {
                Some(clip) => {
                    let elapsed = moment.ticks - playing.started;
                    let left = clip_ticks(clip) - elapsed;
                    let fade = if layer.fade_out > 0.0 {
                        (left as f32 / layer.fade_out).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let weight = layer.weight * fade * fade * (3.0 - 2.0 * fade);
                    let position = clip_position(clip, elapsed) as f32;
                    add_layer(graph, clip, position, layer, weight, rest, sampled, delta, &mut out.joints);
                }
                None => missing = true,
            }
        }
        missing
    }

    /// The locomotion state's pose at `moment`, into `out`.
    fn write_locomotion(
        &self,
        graph: &GraphBinding,
        clips: &(impl ClipSource + ?Sized),
        rest: &[Transform],
        moment: Moment,
        sampled: &mut [Transform],
        out: &mut Frame,
    ) -> bool {
        match &graph.states[usize::from(self.state)].motion {
            Motion::Clip { clip, looping } => {
                let Some(clip) = graph.clip(clips, *clip) else {
                    out.joints.copy_from_slice(rest);
                    return true;
                };
                let elapsed = moment.ticks - self.started;
                let position = if *looping {
                    looped_position(clip, elapsed)
                } else {
                    clip_position(clip, elapsed) as f32
                };
                sample::sample(clip, position, rest, &mut out.joints);
                overlay_weights(clip, position, &mut out.weights);
                false
            }
            Motion::Blend(space) => {
                let mut missing = false;
                let mut sum = 0.0;
                let mut heaviest = (0.0, None);
                for (sample, &weight) in space.samples.iter().zip(&self.blend) {
                    if weight <= 0.0 {
                        continue;
                    }
                    let Some(clip) = graph.clip(clips, sample.clip) else {
                        missing = true;
                        continue;
                    };
                    let position = if sample.sync {
                        (moment.phase + sample.phase).rem_euclid(1.0) * f32::from(clip.ticks.to_native())
                    } else {
                        looped_position(clip, moment.ticks)
                    };
                    if sum == 0.0 {
                        sample::sample(clip, position, rest, &mut out.joints);
                    } else {
                        sample::sample(clip, position, rest, sampled);
                        blend(&mut out.joints, sampled, weight / (sum + weight), None);
                    }
                    sum += weight;
                    if weight > heaviest.0 {
                        heaviest = (weight, Some((clip, position)));
                    }
                }
                if sum == 0.0 {
                    out.joints.copy_from_slice(rest);
                }
                if let (_, Some((clip, position))) = heaviest {
                    overlay_weights(clip, position, &mut out.weights);
                }
                missing
            }
        }
    }
}

/// `clip` at `position`, less its first tick, over `pose`.
#[allow(clippy::too_many_arguments, reason = "the buffers of one step of one frame")]
fn add_layer(
    graph: &GraphBinding,
    clip: &ArchivedClip,
    position: f32,
    layer: &Added,
    weight: f32,
    rest: &[Transform],
    sampled: &mut [Transform],
    delta: &mut [Transform],
    pose: &mut [Transform],
) {
    if weight <= 0.0 {
        return;
    }
    sample::sample(clip, position, rest, sampled);
    make_delta(sampled, &layer.reference, delta);
    let mask = layer.mask.map(|mask| &graph.masks[usize::from(mask)]);
    add(pose, delta, weight, mask);
}

#[cfg(test)]
mod tests;
