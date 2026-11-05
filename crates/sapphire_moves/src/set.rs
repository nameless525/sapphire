//! [`MoveSet`]: a character's moves as the simulation reads them, cooked from a
//! [`MoveSetDocument`](crate::MoveSetDocument). See the [crate docs](crate).

use std::collections::{BTreeMap, BTreeSet};

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize, TypePath};
use sapphire_colliders::GroupMask;
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::{self, AlignedVec, CookedError};
use sapphire_time::{NORMAL_SPEED, SimDuration};
use serde::{Deserialize, Serialize};

use crate::brain::{Against, BrainAttack, BrainDef, BrainPace, MAX_BRAIN_ATTACKS, Reach, Recent, Spacing};
use crate::document::{Issue, covered_runs, runs_cover};
use crate::movement::ControllerProfile;
use crate::{BakedTables, PathSample, TablesError};

/// Conventional file extension for cooked move sets.
pub const MOVE_SET_EXTENSION: &str = "smoves";

/// Most kinds of intent one set declares: the simulation keeps which are held as a bit each.
pub const MAX_INTENTS: usize = 16;

/// Longest an intent may stay buffered, in ticks: the deep end of spec §5.22's ring.
pub const MAX_BUFFER_TICKS: u32 = 15;

/// Longest move a set may hold, in ticks (a minute): past this it's a typo, and a keyed table's
/// samples would be its cost.
pub const MAX_MOVE_TICKS: u32 = 3600;

/// Farthest a move's root may go or a knockback push, in metres, and furthest a root may turn, in radians:
/// past this it's a typo, and past about 1e19 the character controller's own arithmetic runs out.
pub const MAX_MOVE_REACH: f32 = 1000.0;

/// The thinnest size a hit volume or a hurtbox may have, in metres. The physics world refuses anything
/// thinner, which would leave a fighter nobody can hit, with nothing to say so but a counter.
pub const MIN_SHAPE_EXTENT: f32 = 0.001;

/// Most hurtboxes one fighter has: `sapphire_gameplay` reserves their indices and one extra for its
/// perfect-dodge volume in each actor's body-ID range.
pub const MAX_HURTBOXES: usize = 16;

/// Longest one hit may freeze anyone, in ticks (two seconds): past this it's a typo.
pub const MAX_HITSTOP_TICKS: u16 = 120;

/// The cue a hit publishes when its [`Attack`] names none.
pub const HIT_LANDED: Name = Name::new("hit_landed");

/// Longest one perfect dodge may slow the world, in real fixed steps (ten seconds): past this it's a
/// typo.
pub const MAX_SLOW_MOTION_STEPS: u16 = 600;

/// The move's frame data as far as a designer divides it: what the debug view shows and N3's combat
/// layer can read. Nothing in the simulation depends on it; windows say what happens when.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Anticipation, before the move can do anything.
    Startup,
    /// The strike.
    Active,
    /// Follow-through, usually with its cancel windows.
    Recovery,
}

/// A verb on the input side: one of the simulation's `Buttons`, for a document to name. Its bit is
/// the same as that button's, which `sapphire_gameplay` tests.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[repr(u8)]
pub enum Button {
    Light,
    Heavy,
    Dodge,
    Skill,
    Ultimate,
    Swap,
    LockOn,
    Jump,
}

impl Button {
    /// This button's bit in `Buttons`' byte layout.
    #[must_use]
    pub const fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// A shape riding a path. A capsule's axis and a box's axes are the path's own, in its local space,
/// centred on the path's point.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum Shape {
    Sphere {
        radius: f32,
    },
    /// `length` along the path's local +Y between the two end spheres' centres.
    Capsule {
        radius: f32,
        length: f32,
    },
    Box {
        half_extents: [f32; 3],
    },
}

impl Shape {
    /// Whether the world can hold it: every number finite, and each size at least [`MIN_SHAPE_EXTENT`] (a
    /// capsule's length may be anything from zero: too short to be one, it's a sphere).
    #[must_use]
    pub fn is_valid(self) -> bool {
        let extent = |size: f32| size.is_finite() && size >= MIN_SHAPE_EXTENT;
        match self {
            Self::Sphere { radius } => extent(radius),
            Self::Capsule { radius, length } => extent(radius) && length.is_finite() && length >= 0.0,
            Self::Box { half_extents } => half_extents.into_iter().all(extent),
        }
    }
}

/// One stretch of a move's own time, evaluated exactly once: what lies between two readings of its
/// clock, or between its last reading and the cancel that ended it. See the [crate docs](crate).
///
/// A frozen clock reads what it read before, which is a stretch with nothing in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stretch {
    /// The reading before this one, or `None` if this is the move's first.
    previous: Option<SimDuration>,
    end: SimDuration,
    /// Whether the move goes on from `end`, having been read there. Otherwise something cut it off
    /// at `end`, and `end` itself belongs to whatever came next.
    reached: bool,
}

impl Stretch {
    /// A step's reading of a move that goes on from it.
    #[must_use]
    pub const fn read(previous: Option<SimDuration>, reading: SimDuration) -> Self {
        Self { previous, end: reading, reached: true }
    }

    /// A move's last stretch, cut off at `at` by a cancel or its own end. What happens exactly at
    /// `at` belongs to what follows.
    #[must_use]
    pub const fn cut(previous: Option<SimDuration>, at: SimDuration) -> Self {
        Self { previous, end: at, reached: false }
    }

    /// The reading before this stretch, or `None` if it starts the move.
    #[must_use]
    pub const fn previous(self) -> Option<SimDuration> {
        self.previous
    }

    /// Where this stretch ends.
    #[must_use]
    pub const fn end(self) -> SimDuration {
        self.end
    }

    /// The first sub-tick not evaluated before: time 0 on a move's first reading.
    const fn first(self) -> u64 {
        match self.previous {
            Some(previous) => previous.as_subticks().saturating_add(1),
            None => 0,
        }
    }

    /// Whether an instant lies in this stretch: true of it in exactly one stretch, whatever the
    /// clock did, unless a cut left it out.
    #[must_use]
    pub const fn contains(self, instant: SimDuration) -> bool {
        let instant = instant.as_subticks();
        self.first() <= instant
            && if self.reached { instant <= self.end.as_subticks() } else { instant < self.end.as_subticks() }
    }
}

/// Part of a move in whole ticks: `from` up to but not including `to`. Written `from..to`, a window
/// is open on ticks `from` to `to − 1`, so `5..8` is three ticks long, whichever clock runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Span {
    pub from: u32,
    pub to: u32,
}

impl Span {
    /// When it opens, into the move.
    #[must_use]
    pub const fn start(self) -> SimDuration {
        SimDuration::from_ticks(self.from as u64)
    }

    /// When it has closed, into the move.
    #[must_use]
    pub const fn end(self) -> SimDuration {
        SimDuration::from_ticks(self.to as u64)
    }

    /// The time of its last tick: where the arc of a hit volume riding it stops.
    #[must_use]
    pub const fn last_tick(self) -> SimDuration {
        SimDuration::from_ticks(self.to.saturating_sub(1) as u64)
    }

    /// Whether it's open at a time into the move.
    #[must_use]
    pub const fn contains(self, elapsed: SimDuration) -> bool {
        self.start().as_subticks() <= elapsed.as_subticks()
            && elapsed.as_subticks() < self.end().as_subticks()
    }

    /// Whether this window counts for a stretch: the stretch's reading lies in it, or it opened and
    /// closed without any reading landing in it, so this stretch is the one that passed it.
    ///
    /// A frozen clock stays in the windows it was in. At a steady 1× a window `a..b` counts on the
    /// steps that read ticks `a` to `b − 1`, and at any speed no window goes uncounted.
    #[must_use]
    pub const fn live(self, stretch: Stretch) -> bool {
        let (start, end) = (self.start().as_subticks(), self.end().as_subticks());
        let first = stretch.first();
        let last = stretch.end.as_subticks();
        if stretch.reached {
            start <= last && (last < end || first <= start)
        } else {
            first <= start && start < last
        }
    }

    /// What this stretch covers of the arc from this window's first tick to its last: the times
    /// into the move that a hit volume riding it sweeps through, or `None` if it isn't live here.
    ///
    /// Over a window's life these join up to exactly that arc at any speed, so a swing covers the
    /// same ground however fast the clock runs it. Past the arc's end, while the clock is still
    /// inside its last tick, the volume stays live and holds still.
    #[must_use]
    pub fn sweep(self, stretch: Stretch) -> Option<(SimDuration, SimDuration)> {
        let (start, end, arc_end) =
            (self.start().as_subticks(), self.end().as_subticks(), self.last_tick().as_subticks());
        let first = stretch.first();
        let last = stretch.end.as_subticks();
        let live = if stretch.reached {
            start <= last && (last < end || first <= arc_end)
        } else {
            first <= start && start < last
        };
        if !live {
            return None;
        }
        let along = |time: u64| SimDuration::from_subticks(time.clamp(start, arc_end));
        let from = stretch.previous.map_or(start, SimDuration::as_subticks);
        Some((along(from), along(last)))
    }
}

/// A named kind of intent: what the same system takes from a player's buttons or an enemy's brain.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IntentDef {
    name: u64,
    label: String,
    /// The button that presses it, if a player does.
    pub button: Option<Button>,
    /// Ticks of its owner's clock a press stays usable, from the tick it lands: pressed on tick `k`
    /// with a buffer of 12, it can still fire at tick `k + 11`.
    pub buffer_ticks: u32,
    /// Which one fires when two could at the same instant: the higher.
    pub priority: u8,
}

impl IntentDef {
    pub(crate) fn new(label: String, button: Option<Button>, buffer_ticks: u32, priority: u8) -> Self {
        Self { name: Name::intern(&label).as_u64(), label, button, buffer_ticks, priority }
    }

    /// The intent's name, by which documents and allowances refer to it.
    #[must_use]
    pub const fn name(&self) -> Name {
        Name::from_u64(self.name)
    }

    /// The name's text, for tools.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// How long a press stays usable.
    #[must_use]
    pub const fn buffer(&self) -> SimDuration {
        SimDuration::from_ticks(self.buffer_ticks as u64)
    }
}

/// How a press has to arrive for a window to take it.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum CancelMode {
    /// Pressed any time its buffer still lasts: it fires as the window opens, or at once if open.
    #[default]
    Buffered,
    /// Pressed while the window is open. A press from before stays in the buffer, unused.
    Instant,
    /// The intent's hold is down while the window is open; no press needed.
    Held,
}

/// One thing a window lets an intent do: start a move.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Allow {
    intent: u64,
    target: u32,
    target_name: u64,
    /// How the press has to arrive.
    pub mode: CancelMode,
}

impl Allow {
    pub(crate) const fn new(intent: Name, target: u32, target_name: Name, mode: CancelMode) -> Self {
        Self { intent: intent.as_u64(), target, target_name: target_name.as_u64(), mode }
    }

    /// The intent that may fire it.
    #[must_use]
    pub const fn intent(&self) -> Name {
        Name::from_u64(self.intent)
    }

    /// The move it starts, as an index into this revision's [`MoveSet::moves`].
    #[must_use]
    pub const fn target(&self) -> u32 {
        self.target
    }

    /// The move it starts, by name: what a window of an older revision goes by.
    #[must_use]
    pub const fn target_name(&self) -> Name {
        Name::from_u64(self.target_name)
    }
}

/// A cancel window: while open, an intent may end the move in another.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CancelWindow {
    pub span: Span,
    pub allow: Vec<Allow>,
}

/// How heavy a hit is: which reaction it asks of its target ([`Reactions`]), lightest first. When two
/// hits land on one target in one step, the heavier's reaction plays.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[repr(u8)]
pub enum Weight {
    #[default]
    Light,
    Medium,
    Heavy,
}

/// A push away from the attacker, on the target's own clock, so hitstop delays it and costs it nothing.
/// It eases out: quick at first, slowing to a stop.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Knockback {
    /// Metres. Zero is no push.
    pub distance: f32,
    /// How long it takes, in ticks of the target's clock.
    pub ticks: u32,
}

/// What a hit volume does to what it touches: M2 G3's resolve reads this.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Attack {
    /// Taken off the target's health.
    pub damage: u32,
    /// Taken off the target's poise, and what a move's [`ArmourWindow`] is measured against.
    pub poise: u32,
    /// Which reaction it asks for.
    pub weight: Weight,
    /// Ticks both owners' clocks freeze for. A hit the target's armour takes freezes only the attacker.
    pub hitstop: u16,
    /// Ticks every actor's clock freezes for, if this hit defeats its target. See `sapphire_gameplay`'s
    /// `resolve` module on why this isn't a clock of its own.
    pub finish_freeze: u16,
    pub knockback: Knockback,
    cue: u64,
}

impl Attack {
    pub(crate) const fn new(
        damage: u32,
        poise: u32,
        weight: Weight,
        hitstop: u16,
        finish_freeze: u16,
        knockback: Knockback,
        cue: Name,
    ) -> Self {
        Self { damage, poise, weight, hitstop, finish_freeze, knockback, cue: cue.as_u64() }
    }

    /// The cue it publishes when it lands, for M2 E3's table: authored, or [`HIT_LANDED`].
    #[must_use]
    pub const fn cue(&self) -> Name {
        Name::from_u64(self.cue)
    }
}

/// A hit volume's active window: a shape riding a named path of the move's tables.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Hitbox {
    name: u64,
    path: u64,
    pub span: Span,
    pub shape: Shape,
    /// What it does when it lands. With none it's a hit query and nothing more: events, and no effect.
    pub attack: Option<Attack>,
    /// Whole ticks into the move at which its hit group re-arms, ascending. Empty is once per playing.
    pub rearm: Vec<u32>,
    blocked_by: u32,
}

impl Hitbox {
    pub(crate) fn new(
        name: Name,
        path: Name,
        span: Span,
        shape: Shape,
        attack: Option<Attack>,
        rearm: Vec<u32>,
        blocked_by: GroupMask,
    ) -> Self {
        Self {
            name: name.as_u64(),
            path: path.as_u64(),
            span,
            shape,
            attack,
            rearm,
            blocked_by: blocked_by.bits(),
        }
    }

    /// What stops it: nothing, unless it says.
    #[must_use]
    pub const fn blocked_by(&self) -> GroupMask {
        GroupMask::from_bits(self.blocked_by)
    }

    /// The hit's name: what G3 looks its effect up by, and within one playing of a move what its
    /// windows share, so a target is hit once per name.
    #[must_use]
    pub const fn name(&self) -> Name {
        Name::from_u64(self.name)
    }

    /// The path in the move's tables that it rides.
    #[must_use]
    pub const fn path(&self) -> Name {
        Name::from_u64(self.path)
    }
}

/// What a hurtbox becomes while a window is open.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum HurtboxState {
    /// Nothing can hit it.
    Off,
    /// It takes this shape instead of its own.
    Shape(Shape),
}

/// A window changing one of the character's hurtboxes.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct HurtboxChange {
    name: u64,
    pub span: Span,
    pub state: HurtboxState,
}

impl HurtboxChange {
    pub(crate) const fn new(name: Name, span: Span, state: HurtboxState) -> Self {
        Self { name: name.as_u64(), span, state }
    }

    /// The hurtbox it changes.
    #[must_use]
    pub const fn name(&self) -> Name {
        Name::from_u64(self.name)
    }
}

/// A window of super armour: while it's open, a hit whose [`Attack::poise`] is below `threshold` does its
/// damage and takes its poise, and the actor plays no reaction. Enough poise taken still breaks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ArmourWindow {
    pub span: Span,
    pub threshold: u32,
}

/// What a live swing has to touch to count as dodged: a shape riding the root, the way a hurtbox does.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct DodgeVolume {
    /// Where it sits, in the root's space.
    pub local: PathSample,
    pub shape: Shape,
}

/// How a perfect dodge slows the world: a share of its speed, for an exact number of real fixed steps.
/// The one who dodged runs at its own speed through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct WorldSlowMotion {
    /// Thousandths of the world's speed: below 1000, and a divisor of 1,000,000, so the scale that
    /// brings the one who dodged back up is a whole number of thousandths and lands on exactly 1×.
    pub per_mille: u32,
    /// Real fixed steps, which hitstop and a slowed clock both leave alone.
    pub steps: u16,
}

/// What a perfect dodge lets an intent do afterwards: for `ticks` of the dodger's own clock from the
/// instant it fired, these start a move, as a cancel window's would, whether the dodge is still playing
/// or has ended. Starting any move closes it.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CounterWindow {
    pub ticks: u32,
    pub allow: Vec<Allow>,
}

impl CounterWindow {
    /// How long it stays open, on the dodger's clock.
    #[must_use]
    pub const fn duration(&self) -> SimDuration {
        SimDuration::from_ticks(self.ticks as u64)
    }
}

/// A perfect-dodge window: while it's open, a live swing touching [`Self::volume`] fires the dodge, once
/// per playing. `sapphire_gameplay`'s `resolve` module says what that does.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PerfectDodgeWindow {
    pub span: Span,
    /// With none it's a window and nothing more, for a view to show: nothing can fire it.
    pub volume: Option<DodgeVolume>,
    pub slow_motion: Option<WorldSlowMotion>,
    pub counter: Option<CounterWindow>,
}

/// A motion warp window: the move's own root motion bends toward a target within these limits.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct WarpWindow {
    pub span: Span,
    /// Most metres the warp may add to or take from the table's motion over this window, in all.
    pub max_distance: f32,
    /// Most radians the warp may turn the root over this window, in all.
    pub max_turn: f32,
    /// Metres short of the target's position to stop.
    pub keep: f32,
}

/// A presentation event at one instant of a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Cue {
    name: u64,
    /// The tick it fires on, inside the move.
    pub at: u32,
}

impl Cue {
    pub(crate) const fn new(name: Name, at: u32) -> Self {
        Self { name: name.as_u64(), at }
    }

    /// What it is: E3's table maps this to effects, sound, shake and rumble.
    #[must_use]
    pub const fn name(&self) -> Name {
        Name::from_u64(self.name)
    }

    /// When it fires, into the move.
    #[must_use]
    pub const fn time(&self) -> SimDuration {
        SimDuration::from_ticks(self.at as u64)
    }
}

/// One move's frame data and tables: every fact a designer tunes, in ticks. Tracks are sorted by
/// when they begin, in the order authored where that's equal.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Move {
    name: u64,
    label: String,
    /// Ticks of anticipation, from the move's first tick.
    pub startup: u32,
    /// Ticks of strike, after those.
    pub active: u32,
    /// Ticks to the move's end, after those.
    pub recovery: u32,
    /// Root motion and hit paths. May be shorter than the move, whose readings then hold the last
    /// sample, or longer, whose tail is never read.
    pub tables: BakedTables,
    pub hitboxes: Vec<Hitbox>,
    pub hurtboxes: Vec<HurtboxChange>,
    pub invulnerable: Vec<Span>,
    pub perfect_dodge: Vec<PerfectDodgeWindow>,
    pub parry: Vec<Span>,
    pub armour: Vec<ArmourWindow>,
    pub cancels: Vec<CancelWindow>,
    /// Never overlapping, so at most one bends the motion at any instant.
    pub warps: Vec<WarpWindow>,
    pub cues: Vec<Cue>,
}

impl Move {
    /// The move's name.
    #[must_use]
    pub const fn name(&self) -> Name {
        Name::from_u64(self.name)
    }

    /// The name's text, for tools.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The whole move, in ticks.
    #[must_use]
    pub const fn ticks(&self) -> u32 {
        self.startup + self.active + self.recovery
    }

    /// The whole move, on its owner's clock.
    #[must_use]
    pub const fn duration(&self) -> SimDuration {
        SimDuration::from_ticks(self.ticks() as u64)
    }

    /// The phase at a time into the move: its last, past the end.
    #[must_use]
    pub const fn phase_at(&self, elapsed: SimDuration) -> Phase {
        let tick = elapsed.whole_ticks();
        if tick < self.startup as u64 {
            Phase::Startup
        } else if tick < (self.startup + self.active) as u64 {
            Phase::Active
        } else {
            Phase::Recovery
        }
    }

    /// The hit volumes open on a tick, in track order: what a test or the editor asks of frame data.
    pub fn hitboxes_on(&self, tick: u32) -> impl Iterator<Item = &Hitbox> {
        let time = SimDuration::from_ticks(u64::from(tick));
        self.hitboxes.iter().filter(move |hitbox| hitbox.span.contains(time))
    }
}

/// Parts of a [`Move`] that [`MoveSetDocument`](crate::MoveSetDocument) gathers, to build one.
pub(crate) struct MoveParts {
    pub(crate) label: String,
    pub(crate) startup: u32,
    pub(crate) active: u32,
    pub(crate) recovery: u32,
    pub(crate) tables: BakedTables,
    pub(crate) hitboxes: Vec<Hitbox>,
    pub(crate) hurtboxes: Vec<HurtboxChange>,
    pub(crate) invulnerable: Vec<Span>,
    pub(crate) perfect_dodge: Vec<PerfectDodgeWindow>,
    pub(crate) parry: Vec<Span>,
    pub(crate) armour: Vec<ArmourWindow>,
    pub(crate) cancels: Vec<CancelWindow>,
    pub(crate) warps: Vec<WarpWindow>,
    pub(crate) cues: Vec<Cue>,
}

impl MoveParts {
    pub(crate) fn build(self) -> Move {
        let Self {
            label,
            startup,
            active,
            recovery,
            tables,
            mut hitboxes,
            mut hurtboxes,
            mut invulnerable,
            mut perfect_dodge,
            mut parry,
            mut armour,
            mut cancels,
            mut warps,
            mut cues,
        } = self;
        // Stable sorts: authored order stands among equals, so it's part of what a set means.
        hitboxes.sort_by_key(|hitbox| hitbox.span.from);
        hurtboxes.sort_by_key(|change| change.span.from);
        invulnerable.sort_by_key(|span| span.from);
        perfect_dodge.sort_by_key(|window| window.span.from);
        parry.sort_by_key(|span| span.from);
        armour.sort_by_key(|window| window.span.from);
        cancels.sort_by_key(|window| window.span.from);
        warps.sort_by_key(|window| window.span.from);
        cues.sort_by_key(|cue| cue.at);
        Move {
            name: Name::intern(&label).as_u64(),
            label,
            startup,
            active,
            recovery,
            tables,
            hitboxes,
            hurtboxes,
            invulnerable,
            perfect_dodge,
            parry,
            armour,
            cancels,
            warps,
            cues,
        }
    }
}

/// One of a fighter's own hurtboxes: where it rides on the root, and its shape when no window says
/// otherwise. Its place in [`FighterDef::hurtboxes`] is its number in the physics world.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FighterHurtbox {
    name: u64,
    /// Where it sits, in the root's space: it turns as the root does.
    pub local: PathSample,
    pub shape: Shape,
}

impl FighterHurtbox {
    pub(crate) const fn new(name: Name, local: PathSample, shape: Shape) -> Self {
        Self { name: name.as_u64(), local, shape }
    }

    /// Its name, by which a move's hurtbox windows change it.
    #[must_use]
    pub const fn name(&self) -> Name {
        Name::from_u64(self.name)
    }
}

/// The moves a fighter's reactions play, by name, forced through whatever's running. One left out is
/// no reaction: the hit does its damage, and whatever's running plays on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Reactions {
    light: Option<u64>,
    medium: Option<u64>,
    heavy: Option<u64>,
    poise_break: Option<u64>,
    defeat: Option<u64>,
}

impl Reactions {
    pub(crate) fn new(
        light: Option<Name>,
        medium: Option<Name>,
        heavy: Option<Name>,
        poise_break: Option<Name>,
        defeat: Option<Name>,
    ) -> Self {
        Self {
            light: light.map(Name::as_u64),
            medium: medium.map(Name::as_u64),
            heavy: heavy.map(Name::as_u64),
            poise_break: poise_break.map(Name::as_u64),
            defeat: defeat.map(Name::as_u64),
        }
    }

    /// The reaction to a hit of this weight.
    #[must_use]
    pub fn flinch(&self, weight: Weight) -> Option<Name> {
        match weight {
            Weight::Light => self.light,
            Weight::Medium => self.medium,
            Weight::Heavy => self.heavy,
        }
        .map(Name::from_u64)
    }

    /// The reaction to a hit that empties the poise meter.
    #[must_use]
    pub fn poise_break(&self) -> Option<Name> {
        self.poise_break.map(Name::from_u64)
    }

    /// The reaction to a hit that empties the health.
    #[must_use]
    pub fn defeat(&self) -> Option<Name> {
        self.defeat.map(Name::from_u64)
    }

    /// Whether a move of this name is one of these reactions.
    #[must_use]
    pub fn contains(&self, name: Name) -> bool {
        [self.light, self.medium, self.heavy, self.poise_break, self.defeat].contains(&Some(name.as_u64()))
    }
}

/// What makes a character something a hit can land on. It lives in the move set because everything a
/// fight depends on has to reach a replay's `tables_hash`, and a character's numbers and reactions are
/// tuned with its moves. A set without one plays moves and can't be hit.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FighterDef {
    /// Health at a spawn, at least 1.
    pub health: u32,
    /// The poise meter at a spawn. Zero is no meter: nothing breaks it.
    pub poise: u32,
    /// Ticks of its clock after the last hit that took poise before the meter is full again. Zero is
    /// never, short of a break, which fills it at once.
    pub poise_refill: u32,
    /// At least one, at most [`MAX_HURTBOXES`].
    pub hurtboxes: Vec<FighterHurtbox>,
    pub reactions: Reactions,
    /// How it moves, within [`MOVEMENT_LIMITS`](crate::MOVEMENT_LIMITS). `None` leaves its actors the
    /// profile they spawned with, which is what a document with no such section means.
    pub movement: Option<ControllerProfile>,
}

impl FighterDef {
    /// How long after the last hit that took poise the meter is full again: see [`Self::poise_refill`].
    #[must_use]
    pub const fn refill(&self) -> SimDuration {
        SimDuration::from_ticks(self.poise_refill as u64)
    }
}

/// A set of moves, cooked: what one character can do, with what starts what. See the
/// [crate docs](crate).
///
/// Every value holds the checks [`MoveSetDocument::compile`](crate::MoveSetDocument::compile) makes,
/// whichever way it was made, so the simulation reads it without any.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::moves"]
pub struct MoveSet {
    intents: Vec<IntentDef>,
    neutral: Vec<Allow>,
    fighter: Option<FighterDef>,
    brain: Option<BrainDef>,
    moves: Vec<Move>,
}

impl Versioned for MoveSet {
    /// 1: M2 G2. 2: M2 G3's attacks, armour, re-arms, walls and fighter. 3: M2 G4's perfect dodges.
    /// 4: M2 G6's brain. 5: M2 T2's movement.
    const VERSION: u32 = 5;
}

/// A move set that can't be built, read or written.
#[derive(Debug, thiserror::Error)]
pub enum MoveSetError {
    /// Everything wrong with a document, one line each.
    #[error("{}", list(.0))]
    Invalid(Vec<Issue>),
    /// Boxed: RON's errors are large.
    #[error(transparent)]
    Document(Box<DocumentError>),
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("move `{name}`'s tables: {source}")]
    Tables {
        name: String,
        #[source]
        source: TablesError,
    },
    /// Cooked data that isn't what any document compiles to.
    #[error("cooked move set is damaged: {0}")]
    Damaged(String),
}

fn ordered_by<T>(values: &[T], at: impl Fn(&T) -> u32) -> bool {
    values.windows(2).all(|pair| at(&pair[0]) <= at(&pair[1]))
}

fn list(issues: &[Issue]) -> String {
    let lines: Vec<String> = issues.iter().map(ToString::to_string).collect();
    lines.join("\n")
}

impl ArchivedMoveSet {
    /// The set as plain data, for a simulation to take: what an app does with a new version of a cooked
    /// set, which live tuning reloads through a pack.
    pub fn to_owned_set(&self) -> Result<MoveSet, MoveSetError> {
        let set = rkyv::deserialize::<MoveSet, rkyv::rancor::Error>(self)
            .map_err(|error| MoveSetError::Cooked(CookedError::from(error)))?;
        set.check()?;
        Ok(set)
    }
}

impl MoveSet {
    pub(crate) fn from_parts(
        intents: Vec<IntentDef>,
        neutral: Vec<Allow>,
        fighter: Option<FighterDef>,
        brain: Option<BrainDef>,
        moves: Vec<Move>,
    ) -> Self {
        Self { intents, neutral, fighter, brain, moves }
    }

    /// Its fighter, for a test to damage: what no document can make.
    #[cfg(test)]
    pub(crate) fn fighter_mut(&mut self) -> &mut FighterDef {
        self.fighter.as_mut().expect("a test's set has a fighter")
    }

    /// Reads a cooked set, whose bytes must start 16-byte aligned, after validating the archive and
    /// that its references hold. Copies it out, once, at load, so reading one is plain field access
    /// in the fixed step.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, MoveSetError> {
        let archived = cooked::access::<Self>(bytes)?;
        archived.to_owned_set()
    }

    /// Writes a cooked set: the same bytes for the same set, every time.
    pub fn to_cooked(&self) -> Result<AlignedVec, MoveSetError> {
        Ok(cooked::to_cooked(self)?)
    }

    /// What the simulation relies on without looking, for bytes that may not have come from a
    /// document: an archive can be well formed, point nowhere and say anything its numbers can hold. So
    /// this asks of a set what [`MoveSetDocument::compile`](crate::MoveSetDocument::compile) asks of a
    /// document, and nothing here trusts a number before it's been asked.
    fn check(&self) -> Result<(), MoveSetError> {
        if self.moves.is_empty() {
            return damaged("no moves");
        }
        self.check_intents()?;
        self.check_allows("a neutral start", &self.neutral)?;
        if let Some(fighter) = &self.fighter {
            self.check_fighter(fighter)?;
        }
        if let Some(brain) = &self.brain {
            self.check_brain(brain)?;
        }
        let mut names = BTreeSet::new();
        for mv in &self.moves {
            if !names.insert(mv.name) {
                return damaged(format!("two moves have the name `{}`", mv.label()));
            }
            self.check_move(mv)?;
        }
        Ok(())
    }

    fn check_intents(&self) -> Result<(), MoveSetError> {
        if self.intents.len() > MAX_INTENTS {
            return damaged(format!("{} intents, past the limit of {MAX_INTENTS}", self.intents.len()));
        }
        let mut names = BTreeSet::new();
        for intent in &self.intents {
            let label = intent.label();
            if !is_named(intent.name(), label) {
                return damaged(format!("an intent isn't what its label `{label}` says"));
            }
            if !names.insert(intent.name) {
                return damaged(format!("intent `{label}` appears twice"));
            }
            if !(1..=MAX_BUFFER_TICKS).contains(&intent.buffer_ticks) {
                let buffer = intent.buffer_ticks;
                return damaged(format!(
                    "intent `{label}` buffers {buffer} ticks, not 1 to {MAX_BUFFER_TICKS}"
                ));
            }
        }
        Ok(())
    }

    /// `what` allows these: each by an intent of this set, of a move of this set, and each once.
    fn check_allows(&self, what: &str, allows: &[Allow]) -> Result<(), MoveSetError> {
        let mut seen = BTreeSet::new();
        for allow in allows {
            let Some(target) = self.moves.get(allow.target as usize) else {
                return damaged(format!("{what} names no move"));
            };
            if target.name() != allow.target_name() {
                return damaged(format!("{what} names a move by another's name"));
            }
            if self.intent(allow.intent()).is_none() {
                return damaged(format!("{what} goes by an intent this set doesn't declare"));
            }
            if !seen.insert((allow.intent, allow.mode)) {
                return damaged(format!("{what} takes an intent twice"));
            }
        }
        Ok(())
    }

    fn check_fighter(&self, fighter: &FighterDef) -> Result<(), MoveSetError> {
        if fighter.health == 0 {
            return damaged("a fighter with no health");
        }
        if fighter.hurtboxes.is_empty() {
            return damaged("a fighter with no hurtbox");
        }
        if fighter.hurtboxes.len() > MAX_HURTBOXES {
            return damaged(format!("a fighter with more than {MAX_HURTBOXES} hurtboxes"));
        }
        let mut names = BTreeSet::new();
        for hurtbox in &fighter.hurtboxes {
            if !names.insert(hurtbox.name) {
                return damaged("a fighter with one hurtbox name twice");
            }
            if !hurtbox.shape.is_valid() || !hurtbox.local.is_finite() || !hurtbox.local.is_unit() {
                return damaged("a fighter with a hurtbox of no shape or no place");
            }
        }
        let Reactions { light, medium, heavy, poise_break, defeat } = fighter.reactions;
        for reaction in [light, medium, heavy, poise_break, defeat].into_iter().flatten() {
            if self.find(Name::from_u64(reaction)).is_none() {
                return damaged("a fighter's reaction is no move of this set");
            }
        }
        if let Some(movement) = &fighter.movement {
            let issues = movement.document_issues();
            if !issues.is_empty() {
                return damaged(format!("a fighter's movement: {}", issues.join("; ")));
            }
        }
        Ok(())
    }

    fn check_brain(&self, brain: &BrainDef) -> Result<(), MoveSetError> {
        let Spacing { near, far } = brain.spacing;
        if !(near.is_finite() && far.is_finite() && 0.0 <= near && near < far) {
            return damaged(format!("a brain's spacing {near}..{far} isn't a band of distances"));
        }
        let BrainPace { approach, retreat, circle, circle_share, turn_rate_degrees } = brain.pace;
        if ![approach, retreat, circle, circle_share].iter().all(|tilt| (0.0..=1.0).contains(tilt)) {
            return damaged("a brain's pace isn't within 0 to 1");
        }
        if !(turn_rate_degrees.is_finite() && turn_rate_degrees > 0.0) {
            return damaged(format!("a brain's turn rate is {turn_rate_degrees}, not above 0"));
        }
        for (what, range) in [("patience", brain.patience), ("rest", brain.rest), ("recover", brain.recover)]
        {
            if range.min > range.max || range.max > MAX_MOVE_TICKS {
                let (min, max) = (range.min, range.max);
                return damaged(format!(
                    "a brain's {what} is {min}..={max} ticks, not a range up to {MAX_MOVE_TICKS}"
                ));
            }
        }
        if brain.attacks.is_empty() {
            return damaged("a brain with no attack");
        }
        if brain.attacks.len() > MAX_BRAIN_ATTACKS {
            return damaged(format!("a brain with more than {MAX_BRAIN_ATTACKS} attacks"));
        }
        let mut seen = BTreeSet::new();
        for attack in &brain.attacks {
            let Some(intent) = self.intents.get(attack.intent() as usize) else {
                return damaged("a brain names no intent");
            };
            let name = intent.label();
            if !seen.insert(attack.intent()) {
                return damaged(format!("a brain's attack `{name}` appears twice"));
            }
            if !self.neutral.iter().any(|allow| allow.intent() == intent.name()) {
                return damaged(format!("a brain's attack `{name}` starts nothing from neutral"));
            }
            check_brain_attack(name, attack)?;
        }
        if let Some(rise) = &brain.rise {
            let Some(intent) = self.intents.get(rise.intent() as usize) else {
                return damaged("a brain names no intent");
            };
            let Some((_, during)) = self.find(rise.during()) else {
                return damaged("a brain's rise is during no move of this set");
            };
            let opens = during.cancels.iter().any(|window| {
                window.span.from <= rise.from
                    && rise.to <= window.span.to
                    && window.allow.iter().any(|allow| allow.intent() == intent.name())
            });
            if !opens || rise.from >= rise.to {
                return damaged("a brain's rise isn't inside a cancel window that takes its intent");
            }
        }
        Ok(())
    }

    fn check_move(&self, mv: &Move) -> Result<(), MoveSetError> {
        let label = mv.label();
        if !is_named(mv.name(), label) {
            return damaged(format!("a move isn't what its label `{label}` says"));
        }
        // First of all: every sum of its phases from here on, by anyone, relies on this one.
        let ticks = mv.startup.checked_add(mv.active).and_then(|sum| sum.checked_add(mv.recovery));
        let Some(ticks) = ticks.filter(|ticks| (1..=MAX_MOVE_TICKS).contains(ticks)) else {
            let (startup, active, recovery) = (mv.startup, mv.active, mv.recovery);
            return damaged(format!(
                "move `{label}`'s phases {startup}, {active} and {recovery} aren't 1 to {MAX_MOVE_TICKS} ticks"
            ));
        };
        mv.tables.check().map_err(|source| MoveSetError::Tables { name: label.to_owned(), source })?;
        if !mv.tables.root_within_reach() {
            return damaged(format!("move `{label}`'s root goes past the limit of {MAX_MOVE_REACH}"));
        }
        let good = |span: &Span| span.from < span.to && span.to <= ticks;
        let mut spans = mv
            .hitboxes
            .iter()
            .map(|hitbox| &hitbox.span)
            .chain(mv.hurtboxes.iter().map(|change| &change.span))
            .chain(&mv.invulnerable)
            .chain(mv.perfect_dodge.iter().map(|window| &window.span))
            .chain(&mv.parry)
            .chain(mv.armour.iter().map(|window| &window.span))
            .chain(mv.cancels.iter().map(|window| &window.span))
            .chain(mv.warps.iter().map(|window| &window.span));
        if !spans.all(good) {
            return damaged(format!("move `{label}` has a window outside its {ticks} ticks"));
        }
        check_hitboxes(mv)?;
        if let Some(fighter) = &self.fighter {
            let known =
                |change: &HurtboxChange| fighter.hurtboxes.iter().any(|hurtbox| hurtbox.name == change.name);
            if !mv.hurtboxes.iter().all(known) {
                return damaged(format!("a hurtbox change of move `{label}` isn't the fighter's"));
            }
        }
        let shaped = |change: &HurtboxChange| match change.state {
            HurtboxState::Off => true,
            HurtboxState::Shape(shape) => shape.is_valid(),
        };
        if !mv.hurtboxes.iter().all(shaped) {
            return damaged(format!("a hurtbox change of move `{label}` has no shape"));
        }
        self.check_perfect_dodges(mv)?;
        if !mv.armour.iter().all(|window| window.threshold > 0) {
            return damaged(format!("armour of move `{label}` has a threshold of 0"));
        }
        let what = format!("a cancel window of move `{label}`");
        for window in &mv.cancels {
            if window.allow.is_empty() {
                return damaged(format!("{what} allows nothing"));
            }
            self.check_allows(&what, &window.allow)?;
        }
        for warp in &mv.warps {
            let limits = [warp.max_distance, warp.max_turn, warp.keep];
            if !limits.iter().all(|limit| limit.is_finite() && *limit >= 0.0) {
                return damaged(format!(
                    "a warp of move `{label}` has a limit that's negative or not finite"
                ));
            }
            if warp.max_distance == 0.0 && warp.max_turn == 0.0 {
                return damaged(format!("a warp of move `{label}` can't move or turn anything"));
            }
        }
        if !mv.warps.windows(2).all(|pair| pair[0].span.to <= pair[1].span.from) {
            return damaged(format!("move `{label}`'s warp windows overlap or are out of order"));
        }
        if !mv.cues.iter().all(|cue| cue.at < ticks) {
            return damaged(format!("a cue of move `{label}` is past its last tick"));
        }
        for (track, ordered) in [
            ("hitboxes", ordered_by(&mv.hitboxes, |v| v.span.from)),
            ("hurtboxes", ordered_by(&mv.hurtboxes, |v| v.span.from)),
            ("invulnerable", ordered_by(&mv.invulnerable, |v| v.from)),
            ("perfect_dodge", ordered_by(&mv.perfect_dodge, |v| v.span.from)),
            ("parry", ordered_by(&mv.parry, |v| v.from)),
            ("armour", ordered_by(&mv.armour, |v| v.span.from)),
            ("cancels", ordered_by(&mv.cancels, |v| v.span.from)),
            ("cues", ordered_by(&mv.cues, |v| v.at)),
        ] {
            if !ordered {
                return damaged(format!("move `{label}`'s {track} are out of order"));
            }
        }

        Ok(())
    }

    fn check_perfect_dodges(&self, mv: &Move) -> Result<(), MoveSetError> {
        let label = mv.label();
        let invulnerable = covered_runs(&mv.invulnerable);
        for window in &mv.perfect_dodge {
            let Span { from, to } = window.span;
            match &window.volume {
                Some(volume) => {
                    if self.fighter.is_none() {
                        return damaged(format!(
                            "a perfect dodge of move `{label}` has a volume and no fighter"
                        ));
                    }
                    if !runs_cover(&invulnerable, from, to) {
                        return damaged(format!(
                            "a perfect dodge of move `{label}` has a volume, and isn't invulnerable throughout"
                        ));
                    }
                    if !volume.shape.is_valid() || !volume.local.is_finite() || !volume.local.is_unit() {
                        return damaged(format!(
                            "a perfect dodge of move `{label}` has a volume of no shape or place"
                        ));
                    }
                }
                None if window.slow_motion.is_some() || window.counter.is_some() => {
                    return damaged(format!(
                        "a perfect dodge of move `{label}` says what it does when it fires, with no volume to fire it"
                    ));
                }
                None => {}
            }
            if let Some(WorldSlowMotion { per_mille, steps }) = window.slow_motion {
                let undone = per_mille != 0 && (NORMAL_SPEED * NORMAL_SPEED).is_multiple_of(per_mille);
                if per_mille >= NORMAL_SPEED || !undone {
                    return damaged(format!("a perfect dodge of move `{label}` doesn't slow the world"));
                }
                if !(1..=MAX_SLOW_MOTION_STEPS).contains(&steps) {
                    return damaged(format!(
                        "a perfect dodge of move `{label}` slows the world for {steps} steps, not 1 to {MAX_SLOW_MOTION_STEPS}"
                    ));
                }
            }
            if let Some(counter) = &window.counter {
                let what = format!("a counter window of move `{label}`");
                if !(1..=MAX_MOVE_TICKS).contains(&counter.ticks) {
                    return damaged(format!(
                        "{what} lasts {} ticks, not 1 to {MAX_MOVE_TICKS}",
                        counter.ticks
                    ));
                }
                if counter.allow.is_empty() {
                    return damaged(format!("{what} allows nothing"));
                }
                self.check_allows(&what, &counter.allow)?;
            }
        }
        Ok(())
    }

    /// The kinds of intent, in document order.
    #[must_use]
    pub fn intents(&self) -> &[IntentDef] {
        &self.intents
    }

    /// The kind of intent with this name, and its position in [`Self::intents`], which is its hold's
    /// bit.
    #[must_use]
    pub fn intent(&self, name: Name) -> Option<(usize, &IntentDef)> {
        self.intents.iter().enumerate().find(|(_, intent)| intent.name() == name)
    }

    /// What starts a move from no move at all: a window that's always open.
    #[must_use]
    pub fn neutral(&self) -> &[Allow] {
        &self.neutral
    }

    /// What makes its actors something a hit can land on, if anything does.
    #[must_use]
    pub const fn fighter(&self) -> Option<&FighterDef> {
        self.fighter.as_ref()
    }

    /// How its actors move, if it says: see [`FighterDef::movement`].
    #[must_use]
    pub fn movement(&self) -> Option<&ControllerProfile> {
        self.fighter.as_ref()?.movement.as_ref()
    }

    /// How its actors fight on their own, if they can.
    #[must_use]
    pub const fn brain(&self) -> Option<&BrainDef> {
        self.brain.as_ref()
    }

    /// The moves, in document order: [`Allow::target`] indexes these.
    #[must_use]
    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    /// The move at this index.
    #[must_use]
    pub fn move_at(&self, index: u32) -> Option<&Move> {
        self.moves.get(index as usize)
    }

    /// The move with this name, and its index.
    #[must_use]
    pub fn find(&self, name: Name) -> Option<(u32, &Move)> {
        let index = self.moves.iter().position(|mv| mv.name() == name)?;
        Some((u32::try_from(index).ok()?, &self.moves[index]))
    }
}

/// Refuses a cooked set, saying why.
fn damaged<T>(why: impl Into<String>) -> Result<T, MoveSetError> {
    Err(MoveSetError::Damaged(why.into()))
}

/// Whether a name's hash is its label's, and there's a label.
fn is_named(name: Name, label: &str) -> bool {
    !label.is_empty() && Name::new(label) == name
}

fn check_brain_attack(name: &str, attack: &BrainAttack) -> Result<(), MoveSetError> {
    if !(attack.weight.is_finite() && attack.weight >= 0.0) {
        return damaged(format!("a brain's attack `{name}` has a weight of {}", attack.weight));
    }
    let Reach { near, best, far } = attack.reach;
    if !(near.is_finite() && far.is_finite() && near <= best && best <= far && near < far) {
        return damaged(format!("a brain's attack `{name}` has a reach {near}, {best}, {far} out of order"));
    }
    let Against { free, swinging, opening, reacting, untouchable } = attack.against;
    if ![free, swinging, opening, reacting, untouchable].iter().all(|at| at.is_finite() && *at >= 0.0) {
        return damaged(format!("a brain's attack `{name}` has a multiplier below 0 or not finite"));
    }
    let Recent { ticks, floor } = attack.recent;
    if !(0.0..=1.0).contains(&floor) && ticks > 0 {
        return damaged(format!("a brain's attack `{name}` has a recent floor of {floor}"));
    }
    Ok(())
}

fn check_hitboxes(mv: &Move) -> Result<(), MoveSetError> {
    let label = mv.label();
    let mut groups = BTreeMap::new();
    for hitbox in &mv.hitboxes {
        let what = format!("a hitbox of move `{label}`");
        if !hitbox.shape.is_valid() {
            return damaged(format!("{what} has a shape the world can't hold"));
        }
        if mv.tables.path(hitbox.path()).is_none() {
            return damaged(format!("{what} names no path"));
        }
        if let Some(attack) = &hitbox.attack {
            check_attack(&what, attack)?;
        }
        if !hitbox.rearm.windows(2).all(|pair| pair[0] < pair[1]) {
            return damaged(format!("{what} re-arms out of order"));
        }
        if !hitbox.rearm.iter().all(|&tick| hitbox.span.from < tick && tick < hitbox.span.to) {
            return damaged(format!("{what} re-arms outside its window"));
        }
        if *groups.entry(hitbox.name).or_insert(&hitbox.rearm) != &hitbox.rearm {
            return damaged(format!("{what} shares a hit group and re-arms differently"));
        }
    }
    Ok(())
}

fn check_attack(what: &str, attack: &Attack) -> Result<(), MoveSetError> {
    for (name, ticks) in [("hitstop", attack.hitstop), ("finish freeze", attack.finish_freeze)] {
        if ticks > MAX_HITSTOP_TICKS {
            return damaged(format!(
                "{what} has a {name} of {ticks} ticks, past the limit of {MAX_HITSTOP_TICKS}"
            ));
        }
    }
    let Knockback { distance, ticks } = attack.knockback;
    let good = distance.is_finite()
        && (0.0..=MAX_MOVE_REACH).contains(&distance)
        && (distance == 0.0 || (1..=MAX_MOVE_TICKS).contains(&ticks));
    if !good {
        return damaged(format!("{what} has a knockback of {distance} m over {ticks} ticks"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(subticks: u64) -> SimDuration {
        SimDuration::from_subticks(subticks)
    }

    fn read(previous: Option<u64>, reading: u64) -> Stretch {
        Stretch::read(previous.map(sub), sub(reading))
    }

    fn cut(previous: Option<u64>, at: u64) -> Stretch {
        Stretch::cut(previous.map(sub), sub(at))
    }

    const WINDOW: Span = Span { from: 6, to: 10 };

    /// Each row is one stretch against the window 6..10, ticks 6 to 9.
    #[test]
    fn a_window_counts_where_it_should() {
        #[rustfmt::skip]
        let cases = [
            // At 1×: exactly the steps that read its ticks.
            (read(Some(4000), 5000), false, "the tick before"),
            (read(Some(5000), 6000), true, "its first tick"),
            (read(Some(8000), 9000), true, "its last tick"),
            (read(Some(9000), 10_000), false, "the tick it has closed"),
            // Slow: any reading inside it, to the last sub-tick, and none outside.
            (read(Some(5900), 5999), false, "a sub-tick before it opens"),
            (read(Some(5999), 6000), true, "as it opens"),
            (read(Some(9998), 9999), true, "its last sub-tick"),
            (read(Some(9999), 10_000), false, "as it closes"),
            // Fast: a reading in it counts, and so does a step that passes it whole.
            (read(Some(3000), 7000), true, "a reading in it"),
            (read(Some(5000), 12_000), true, "passed whole"),
            (read(Some(5999), 10_000), true, "passed whole, by a sub-tick each way"),
            (read(Some(6000), 12_000), false, "already counted at the reading on its opening"),
            (read(Some(7000), 12_000), false, "already counted at the reading inside it"),
            (read(Some(10_000), 15_000), false, "long gone"),
            (read(None, 12_000), true, "a move's first reading passing it whole"),
            (read(None, 5000), false, "a move's first reading, not there yet"),
            // Frozen: the same reading again, which stays in what it was in, and reaches nothing new.
            (read(Some(7000), 7000), true, "frozen inside it"),
            (read(Some(5000), 5000), false, "frozen before it"),
            (read(Some(10_000), 10_000), false, "frozen after it"),
            // Cut off: only what opened in this last stretch, since the instant it's cut belongs to what's next.
            (cut(Some(5000), 6000), false, "cut as it opens"),
            (cut(Some(5000), 6001), true, "cut a sub-tick later"),
            (cut(Some(6000), 7000), false, "cut, and already counted"),
            (cut(Some(4000), 12_000), true, "cut past all of it"),
            (cut(None, 12_000), true, "a move's only stretch"),
            (cut(Some(7000), 7000), false, "cut at once"),
        ];
        for (stretch, expected, what) in cases {
            assert_eq!(WINDOW.live(stretch), expected, "{what}");
        }
        let first_tick = Span { from: 0, to: 1 };
        assert!(first_tick.live(read(None, 0)), "a move's first reading reads its first tick");
        assert!(first_tick.live(read(Some(0), 0)), "and stays in it, frozen");
        assert!(first_tick.live(read(Some(0), 999)), "or slowed");
        assert!(!first_tick.live(read(Some(999), 1000)));
        assert!(!first_tick.live(cut(None, 0)), "a move cut on its first instant reads nothing");
    }

    #[test]
    fn an_instant_lies_in_exactly_one_stretch() {
        let instant = sub(6000);
        assert!(!read(Some(5000), 5999).contains(instant));
        assert!(read(Some(5999), 6000).contains(instant));
        assert!(read(Some(5000), 6000).contains(instant));
        assert!(!read(Some(6000), 7000).contains(instant), "it was read already");
        assert!(!read(Some(6000), 6000).contains(instant), "frozen right on it");
        assert!(read(None, 6000).contains(instant));
        assert!(read(None, 0).contains(sub(0)));
        assert!(!read(Some(0), 1000).contains(sub(0)));
        assert!(!cut(Some(5000), 6000).contains(instant), "cut there, so it's what's next's");
        assert!(cut(Some(5000), 6001).contains(instant));
    }

    /// However the readings fall, a window's sweeps join up into its arc, from its first tick to its
    /// last, and at 1× it's the steps you'd expect.
    #[test]
    fn sweeps_cover_a_windows_arc_at_any_speed() {
        for step in [1, 7, 100, 250, 333, 500, 999, 1000, 1001, 1700, 2000, 3000, 3500, 4000, 5000, 20_000] {
            for offset in [0, 1, 400, 999] {
                let mut sweeps = Vec::new();
                let mut previous = None;
                let mut reading = offset;
                while reading < 25_000 {
                    if let Some(sweep) = WINDOW.sweep(read(previous, reading)) {
                        sweeps.push(sweep);
                    }
                    previous = Some(reading);
                    reading += step;
                }
                let (first, last) = (sweeps[0], sweeps[sweeps.len() - 1]);
                let what = format!("every {step} from {offset}");
                assert_eq!(first.0, sub(6000), "{what}");
                assert_eq!(last.1, sub(9000), "{what}");
                for pair in sweeps.windows(2) {
                    assert_eq!(pair[0].1, pair[1].0, "{what}: {sweeps:?}");
                }
            }
        }
        let at_one_times: Vec<_> = (0..14)
            .filter_map(|tick: u64| {
                let previous = tick.checked_sub(1).map(|tick| tick * 1000);
                WINDOW.sweep(read(previous, tick * 1000))
            })
            .collect();
        assert_eq!(
            at_one_times,
            [(6000, 6000), (6000, 7000), (7000, 8000), (8000, 9000)].map(|(a, b)| (sub(a), sub(b)))
        );
    }

    #[test]
    fn sweeps_stop_at_the_arcs_end_and_when_cut() {
        // Slow, past the last tick and still in the window: live, and holding still.
        assert_eq!(WINDOW.sweep(read(Some(9000), 9500)), Some((sub(9000), sub(9000))));
        assert_eq!(WINDOW.sweep(read(Some(9500), 10_000)), None);
        // Frozen on it.
        assert_eq!(WINDOW.sweep(read(Some(7500), 7500)), Some((sub(7500), sub(7500))));
        // Cut off by a cancel, with the window opening in that stretch.
        assert_eq!(WINDOW.sweep(cut(Some(5000), 7000)), Some((sub(6000), sub(7000))));
        // Cut on a tick it would have read: that tick is what's next's.
        assert_eq!(WINDOW.sweep(cut(Some(6000), 7000)), None);
        // A one-tick window's arc is one instant.
        let one = Span { from: 3, to: 4 };
        assert_eq!(one.sweep(read(Some(0), 9000)), Some((sub(3000), sub(3000))));
    }

    /// The test content with a brain as well, so that every part of a set is there to damage.
    fn whole() -> MoveSet {
        const SLICE: &str = include_str!("../test_content/slice.moves.ron");
        const BRAIN: &str = r#"brain: (
            spacing: (near: 1.0, far: 2.0),
            pace: (approach: 0.7, retreat: 0.3, circle: 0.3, circle_share: 0.5, turn_rate_degrees: 300.0),
            patience: (min: 10, max: 20),
            rest: (min: 10, max: 20),
            recover: (min: 5, max: 10),
            attacks: [(intent: "light", weight: 1.0, reach: (near: 0.0, best: 1.0, far: 2.0))],
            rise: (intent: "light", during: "light_1", from: 12, to: 20),
        ),
        fighter: ("#;
        crate::MoveSetDocument::parse(&SLICE.replacen("fighter: (", BRAIN, 1))
            .expect("parses")
            .compile()
            .expect("compiles")
    }

    fn named<'a>(set: &'a mut MoveSet, name: &str) -> &'a mut Move {
        set.moves.iter_mut().find(|mv| mv.label == name).expect("in the test content")
    }

    fn light(set: &mut MoveSet) -> &mut Move {
        named(set, "light_1")
    }

    fn dodge(set: &mut MoveSet) -> &mut Move {
        named(set, "dodge")
    }

    fn fighter(set: &mut MoveSet) -> &mut FighterDef {
        set.fighter.as_mut().expect("in the test content")
    }

    fn brain(set: &mut MoveSet) -> &mut BrainDef {
        set.brain.as_mut().expect("in the test content")
    }

    fn attack(set: &mut MoveSet) -> &mut Attack {
        light(set).hitboxes[0].attack.as_mut().expect("in the test content")
    }

    fn perfect(set: &mut MoveSet) -> &mut PerfectDodgeWindow {
        &mut dodge(set).perfect_dodge[0]
    }

    fn volume(set: &mut MoveSet) -> &mut DodgeVolume {
        perfect(set).volume.as_mut().expect("in the test content")
    }

    /// A bytecheck can't say a number means nothing: well-formed bytes holding what no document can make
    /// are a named error, every one, and never reach arithmetic that trusts them.
    #[test]
    fn a_set_holds_what_a_document_does_whichever_way_it_was_made() {
        type Damage = fn(&mut MoveSet);
        const BAD_SHAPE: Shape = Shape::Sphere { radius: f32::NAN };
        const NOT_UNIT: PathSample = PathSample { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 2.0] };
        const NOT_FINITE: PathSample =
            PathSample { translation: [f32::NAN, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0] };
        let good = whole();
        #[rustfmt::skip]
        let rows: &[(&str, Damage, &str)] = &[
            // The sum nothing checked, which a release build wraps.
            ("a startup at the top of a u32", |s| light(s).startup = u32::MAX, "`light_1`'s phases"),
            ("an active at the top of a u32", |s| light(s).active = u32::MAX, "`light_1`'s phases"),
            ("a recovery at the top of a u32", |s| light(s).recovery = u32::MAX, "`light_1`'s phases"),
            ("a startup just under it", |s| light(s).startup = u32::MAX - 1, "`light_1`'s phases"),
            ("phases that wrap to the move's own length", |s| {
                let mv = light(s);
                (mv.startup, mv.active, mv.recovery) = (u32::MAX, 3, 24);
            }, "`light_1`'s phases"),
            ("phases past a move's limit", |s| light(s).recovery = MAX_MOVE_TICKS, "`light_1`'s phases"),
            ("phases of no ticks", |s| {
                s.brain = None;
                let mv = light(s);
                (mv.startup, mv.active, mv.recovery) = (0, 0, 0);
                mv.hitboxes.clear();
                mv.hurtboxes.clear();
                mv.cancels.clear();
                mv.cues.clear();
            }, "`light_1`'s phases"),

            // Intents.
            ("an intent with no name", |s| s.intents[0].label = String::new(), "an intent"),
            ("an intent called what it isn't", |s| s.intents[0].label = "other".to_owned(), "an intent"),
            ("an intent twice", |s| s.intents[1] = s.intents[0].clone(), "appears twice"),
            ("an intent that buffers nothing", |s| s.intents[0].buffer_ticks = 0, "buffers"),
            ("an intent that buffers too long", |s| s.intents[0].buffer_ticks = MAX_BUFFER_TICKS + 1, "buffers"),

            // What starts what.
            ("a neutral start on no intent", |s| s.neutral[0].intent = Name::new("ghost").as_u64(), "a neutral start"),
            ("a neutral start by the wrong name", |s| s.neutral[0].target_name = Name::new("ghost").as_u64(), "a neutral start"),
            ("the same neutral start twice", |s| s.neutral[1] = s.neutral[0].clone(), "a neutral start"),
            ("a cancel on no intent", |s| light(s).cancels[0].allow[0].intent = Name::new("ghost").as_u64(), "a cancel window of move `light_1`"),
            ("a cancel by the wrong name", |s| light(s).cancels[0].allow[0].target_name = Name::new("ghost").as_u64(), "a cancel window of move `light_1`"),
            ("a cancel window allowing nothing", |s| light(s).cancels[0].allow.clear(), "a cancel window of move `light_1`"),

            // A fighter.
            ("a fighter with no health", |s| fighter(s).health = 0, "health"),
            ("a fighter with no hurtbox", |s| fighter(s).hurtboxes.clear(), "hurtbox"),
            ("a hurtbox twice", |s| {
                let body = fighter(s).hurtboxes[0];
                fighter(s).hurtboxes.push(body);
            }, "hurtbox"),
            ("a hurtbox of no shape", |s| fighter(s).hurtboxes[0].shape = BAD_SHAPE, "hurtbox"),
            ("a hurtbox nowhere", |s| fighter(s).hurtboxes[0].local = NOT_FINITE, "hurtbox"),
            ("a hurtbox turned by no rotation", |s| fighter(s).hurtboxes[0].local = NOT_UNIT, "hurtbox"),
            ("a reaction that isn't a move", |s| fighter(s).reactions.light = Some(Name::new("ghost").as_u64()), "reaction"),

            // A brain.
            ("spacing backwards", |s| brain(s).spacing = Spacing { near: 2.0, far: 1.0 }, "spacing"),
            ("spacing not finite", |s| brain(s).spacing.far = f32::NAN, "spacing"),
            ("a tilt past 1", |s| brain(s).pace.approach = 1.5, "pace"),
            ("a share not finite", |s| brain(s).pace.circle_share = f32::NAN, "pace"),
            ("a turn of nothing", |s| brain(s).pace.turn_rate_degrees = 0.0, "turn"),
            ("a range backwards", |s| brain(s).patience = crate::TickRange { min: 2, max: 1 }, "patience"),
            ("a range that would overflow its draw", |s| brain(s).rest = crate::TickRange { min: 0, max: u32::MAX }, "rest"),
            ("a recovery past a move's limit", |s| brain(s).recover = crate::TickRange { min: 0, max: MAX_MOVE_TICKS + 1 }, "recover"),
            ("a brain with no attack", |s| brain(s).attacks.clear(), "attack"),
            ("an attack twice", |s| {
                let attack = brain(s).attacks[0];
                brain(s).attacks.push(attack);
            }, "attack"),
            ("an attack nothing starts", |s| {
                let unstarted = s.intents.iter().position(|intent| intent.label == "light").unwrap();
                let light = s.intents[unstarted].name();
                s.neutral.retain(|allow| allow.intent() != light);
            }, "neutral"),
            ("an attack weighing nothing it can say", |s| brain(s).attacks[0].weight = f32::NAN, "weight"),
            ("a reach out of order", |s| brain(s).attacks[0].reach = Reach { near: 2.0, best: 1.0, far: 0.0 }, "reach"),
            ("a multiplier below zero", |s| brain(s).attacks[0].against.free = -1.0, "multiplier"),
            ("a memory floor past 1", |s| brain(s).attacks[0].recent = Recent { ticks: 10, floor: 2.0 }, "floor"),
            ("a rise from no move", |s| {
                let rise = brain(s).rise.as_mut().unwrap();
                *rise = crate::brain::BrainRise::new(rise.intent(), Name::new("ghost"), rise.from, rise.to);
            }, "rise"),
            ("a rise outside any cancel", |s| brain(s).rise.as_mut().unwrap().to = 900, "rise"),
            ("a rise of no ticks", |s| {
                let rise = brain(s).rise.as_mut().unwrap();
                rise.to = rise.from;
            }, "rise"),

            // Moves' names and root.
            ("a move with no name", |s| light(s).label = String::new(), "move"),
            ("a move called what it isn't", |s| light(s).label = "other".to_owned(), "move"),
            ("a move twice", |s| {
                let copy = s.moves[0].clone();
                s.moves.push(copy);
            }, "two moves"),

            // Hit volumes.
            ("a hit volume of no shape", |s| light(s).hitboxes[0].shape = BAD_SHAPE, "shape"),
            ("a hitstop past its limit", |s| attack(s).hitstop = MAX_HITSTOP_TICKS + 1, "hitstop"),
            ("a finish freeze past its limit", |s| attack(s).finish_freeze = MAX_HITSTOP_TICKS + 1, "freeze"),
            ("a knockback not finite", |s| attack(s).knockback.distance = f32::NAN, "knockback"),
            ("a knockback backwards", |s| attack(s).knockback.distance = -1.0, "knockback"),
            ("a knockback out of the world", |s| attack(s).knockback.distance = MAX_MOVE_REACH * 2.0, "knockback"),
            ("a knockback in no time", |s| attack(s).knockback = Knockback { distance: 1.0, ticks: 0 }, "knockback"),
            ("a knockback past any move's length", |s| attack(s).knockback = Knockback { distance: 1.0, ticks: MAX_MOVE_TICKS + 1 }, "knockback"),
            ("a re-arm repeated", |s| {
                let hitbox = &mut light(s).hitboxes[0];
                let tick = hitbox.span.from + 1;
                hitbox.span.to = hitbox.span.to.max(tick + 1);
                hitbox.rearm = vec![tick, tick];
            }, "re-arms"),
            ("a re-arm outside its window", |s| {
                let hitbox = &mut light(s).hitboxes[0];
                hitbox.rearm = vec![hitbox.span.to];
            }, "re-arms"),
            ("a hit group re-arming two ways", |s| {
                let mv = light(s);
                let mut other = mv.hitboxes[0].clone();
                other.rearm = vec![other.span.from + 1];
                other.span.to = other.span.to.max(other.span.from + 2);
                mv.hitboxes.push(other);
            }, "re-arm"),

            // Other windows.
            ("a hurtbox change that isn't the fighter's", |s| {
                let change = HurtboxChange::new(Name::new("ghost"), Span { from: 1, to: 2 }, HurtboxState::Off);
                light(s).hurtboxes.push(change);
            }, "hurtbox"),
            ("a hurtbox change to no shape", |s| {
                let change = HurtboxChange::new(Name::new("body"), Span { from: 1, to: 2 }, HurtboxState::Shape(BAD_SHAPE));
                light(s).hurtboxes.push(change);
            }, "shape"),
            ("armour that stops nothing", |s| {
                light(s).armour.push(ArmourWindow { span: Span { from: 1, to: 2 }, threshold: 0 });
            }, "armour"),
            ("a cue past the end", |s| {
                let cue = Cue::new(Name::new("late"), light(s).ticks());
                light(s).cues.push(cue);
            }, "cue"),
            ("a warp of no limit", |s| {
                let warp = WarpWindow { span: Span { from: 1, to: 2 }, max_distance: 0.0, max_turn: 0.0, keep: 0.0 };
                light(s).warps.push(warp);
            }, "warp"),
            ("a warp not finite", |s| {
                let warp = WarpWindow { span: Span { from: 1, to: 2 }, max_distance: f32::NAN, max_turn: 0.0, keep: 0.0 };
                light(s).warps.push(warp);
            }, "warp"),
            ("a warp backwards", |s| {
                let warp = WarpWindow { span: Span { from: 1, to: 2 }, max_distance: 1.0, max_turn: 0.0, keep: -1.0 };
                light(s).warps.push(warp);
            }, "warp"),

            // A perfect dodge.
            ("a volume without i-frames", |s| dodge(s).invulnerable.clear(), "invulnerable"),
            ("a volume with a gap in its i-frames", |s| {
                let window = perfect(s).span;
                dodge(s).invulnerable = vec![Span { from: window.from + 1, to: window.to }];
            }, "invulnerable"),
            ("a volume of no shape", |s| volume(s).shape = BAD_SHAPE, "volume"),
            ("a volume nowhere", |s| volume(s).local = NOT_FINITE, "volume"),
            ("a volume turned by no rotation", |s| volume(s).local = NOT_UNIT, "volume"),
            ("a counter with no volume", |s| {
                perfect(s).volume = None;
                perfect(s).slow_motion = None;
            }, "volume"),
            ("slow motion nothing can undo exactly", |s| perfect(s).slow_motion.as_mut().unwrap().per_mille = 300, "slow"),
            ("slow motion for no time", |s| perfect(s).slow_motion.as_mut().unwrap().steps = 0, "slow"),
            ("slow motion for too long", |s| perfect(s).slow_motion.as_mut().unwrap().steps = MAX_SLOW_MOTION_STEPS + 1, "slow"),
            ("a counter of no length", |s| perfect(s).counter.as_mut().unwrap().ticks = 0, "counter"),
            ("a counter past any move's length", |s| perfect(s).counter.as_mut().unwrap().ticks = MAX_MOVE_TICKS + 1, "counter"),
            ("a counter allowing nothing", |s| perfect(s).counter.as_mut().unwrap().allow.clear(), "counter"),
        ];
        let mut unrefused = Vec::new();
        for &(what, damage, says) in rows {
            let mut set = good.clone();
            damage(&mut set);
            let cooked = set.to_cooked().expect(what);
            match MoveSet::from_cooked(&cooked) {
                Err(MoveSetError::Damaged(text)) if text.contains(says) => {}
                other => unrefused.push(format!("{what}: {:?}", other.err())),
            }
        }
        assert!(unrefused.is_empty(), "{unrefused:#?}");
    }

    #[test]
    fn every_archive_conversion_checks_semantics_and_track_order() {
        let good = whole();
        type Damage = fn(&mut MoveSet);
        let cases: &[Damage] = &[
            |set| set.intents[0].buffer_ticks = 0,
            |set| light(set).cues = vec![Cue::new(Name::new("later"), 2), Cue::new(Name::new("earlier"), 1)],
            |set| light(set).invulnerable = vec![Span { from: 2, to: 3 }, Span { from: 1, to: 2 }],
        ];
        for damage in cases {
            let mut set = good.clone();
            damage(&mut set);
            let cooked = set.to_cooked().expect("structurally valid archive");
            let archived = cooked::access::<MoveSet>(&cooked).expect("valid pointers");
            assert!(archived.to_owned_set().is_err(), "owned views must not bypass validation");
            assert!(MoveSet::from_cooked(&cooked).is_err());
        }
        let cooked = good.to_cooked().expect("writes");
        assert_eq!(
            cooked::access::<MoveSet>(&cooked).expect("valid").to_owned_set().expect("semantic"),
            good
        );
        let mut equal = good;
        light(&mut equal).cues = vec![Cue::new(Name::new("first"), 1), Cue::new(Name::new("second"), 1)];
        assert!(
            MoveSet::from_cooked(&equal.to_cooked().expect("writes")).is_ok(),
            "equal-start authored order is valid"
        );
    }

    /// A root that runs out of the world is what the controller's arithmetic can't carry, by a cook's
    /// road as much as by bytes from anywhere.
    #[test]
    fn a_root_out_of_the_world_is_refused() {
        let mut set = whole();
        let samples = light(&mut set).tables.root_samples().len();
        let mut root = vec![crate::RootSample::ZERO; samples];
        root[1].translation[0] = MAX_MOVE_REACH * 2.0;
        light(&mut set).tables =
            BakedTables::new(root, light(&mut set).tables.paths().to_vec()).expect("finite");
        let cooked = set.to_cooked().expect("cooks");
        let refused = MoveSet::from_cooked(&cooked).err();
        assert!(
            matches!(&refused, Some(MoveSetError::Damaged(text)) if text.contains("root")),
            "{refused:?}"
        );
    }

    #[test]
    fn spans_read_as_ticks() {
        assert_eq!(WINDOW.start(), SimDuration::from_ticks(6));
        assert_eq!(WINDOW.end(), SimDuration::from_ticks(10));
        assert_eq!(WINDOW.last_tick(), SimDuration::from_ticks(9));
        assert!(WINDOW.contains(sub(9999)));
        assert!(!WINDOW.contains(sub(10_000)));
        assert!(!WINDOW.contains(sub(5999)));
    }
}
