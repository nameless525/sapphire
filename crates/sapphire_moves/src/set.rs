//! [`MoveSet`]: a character's moves as the simulation reads them, cooked from a
//! [`MoveSetDocument`](crate::MoveSetDocument). See the [crate docs](crate).

use bevy_reflect::TypePath;
use sapphire_colliders::GroupMask;
use sapphire_core::Name;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::{self, AlignedVec, CookedError};
use sapphire_time::SimDuration;
use serde::{Deserialize, Serialize};

use crate::document::Issue;
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

/// Most hurtboxes one fighter has: `sapphire_gameplay`'s hurtbox IDs hold four bits of index.
pub const MAX_HURTBOXES: usize = 16;

/// Longest one hit may freeze anyone, in ticks (two seconds): past this it's a typo.
pub const MAX_HITSTOP_TICKS: u16 = 120;

/// The cue a hit publishes when its [`Attack`] names none.
pub const HIT_LANDED: Name = Name::new("hit_landed");

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
    Clone, Copy, Debug, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
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
    /// Whether it encloses anything: every number finite, and each size above zero (a capsule's
    /// length may be zero: that's a sphere).
    #[must_use]
    pub fn is_valid(self) -> bool {
        match self {
            Self::Sphere { radius } => radius.is_finite() && radius > 0.0,
            Self::Capsule { radius, length } => {
                radius.is_finite() && radius > 0.0 && length.is_finite() && length >= 0.0
            }
            Self::Box { half_extents } => {
                half_extents.iter().all(|&extent| extent.is_finite() && extent > 0.0)
            }
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
    Clone, Copy, Debug, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
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
    pub perfect_dodge: Vec<Span>,
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
    pub(crate) perfect_dodge: Vec<Span>,
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
        perfect_dodge.sort_by_key(|span| span.from);
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
    moves: Vec<Move>,
}

impl Versioned for MoveSet {
    /// 1: M2 G2. 2: M2 G3's attacks, armour, re-arms, walls and fighter.
    const VERSION: u32 = 2;
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

fn list(issues: &[Issue]) -> String {
    let lines: Vec<String> = issues.iter().map(ToString::to_string).collect();
    lines.join("\n")
}

impl MoveSet {
    pub(crate) fn from_parts(
        intents: Vec<IntentDef>,
        neutral: Vec<Allow>,
        fighter: Option<FighterDef>,
        moves: Vec<Move>,
    ) -> Self {
        Self { intents, neutral, fighter, moves }
    }

    /// Reads a cooked set, whose bytes must start 16-byte aligned, after validating the archive and
    /// that its references hold. Copies it out, once, at load, so reading one is plain field access
    /// in the fixed step.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, MoveSetError> {
        let archived = cooked::access::<Self>(bytes)?;
        let set = rkyv::deserialize::<Self, rkyv::rancor::Error>(archived).map_err(CookedError::from)?;
        set.check()?;
        Ok(set)
    }

    /// Writes a cooked set: the same bytes for the same set, every time.
    pub fn to_cooked(&self) -> Result<AlignedVec, MoveSetError> {
        Ok(cooked::to_cooked(self)?)
    }

    /// What the simulation relies on without looking, for bytes that may not have come from a
    /// document: an archive can be well formed and still point nowhere.
    fn check(&self) -> Result<(), MoveSetError> {
        let damaged = |text: String| Err(MoveSetError::Damaged(text));
        if self.moves.is_empty() {
            return damaged("no moves".to_owned());
        }
        if self.intents.len() > MAX_INTENTS {
            return damaged(format!("{} intents, past the limit of {MAX_INTENTS}", self.intents.len()));
        }
        let moves = self.moves.len();
        let in_bounds = |allow: &Allow| (allow.target as usize) < moves;
        if !self.neutral.iter().all(in_bounds) {
            return damaged("a neutral start names no move".to_owned());
        }
        if self.fighter.as_ref().is_some_and(|fighter| fighter.hurtboxes.len() > MAX_HURTBOXES) {
            return damaged(format!("a fighter with more than {MAX_HURTBOXES} hurtboxes"));
        }
        for mv in &self.moves {
            let label = mv.label();
            mv.tables.check().map_err(|source| MoveSetError::Tables { name: label.to_owned(), source })?;
            let ticks = mv.ticks();
            let good = |span: &Span| span.from < span.to && span.to <= ticks;
            let spans = mv
                .hitboxes
                .iter()
                .map(|hitbox| &hitbox.span)
                .chain(mv.hurtboxes.iter().map(|change| &change.span))
                .chain(&mv.invulnerable)
                .chain(&mv.perfect_dodge)
                .chain(&mv.parry)
                .chain(mv.armour.iter().map(|window| &window.span))
                .chain(mv.cancels.iter().map(|window| &window.span))
                .chain(mv.warps.iter().map(|window| &window.span));
            let mut spans = spans;
            if !spans.all(good) {
                return damaged(format!("move `{label}` has a window outside its {ticks} ticks"));
            }
            if !mv.cancels.iter().all(|window| window.allow.iter().all(in_bounds)) {
                return damaged(format!("a cancel window of move `{label}` names no move"));
            }
            if !mv.hitboxes.iter().all(|hitbox| mv.tables.path(hitbox.path()).is_some()) {
                return damaged(format!("a hitbox of move `{label}` names no path"));
            }
            if !mv.warps.windows(2).all(|pair| pair[0].span.to <= pair[1].span.from) {
                return damaged(format!("move `{label}`'s warp windows overlap or are out of order"));
            }
            if !mv.hitboxes.iter().all(|hitbox| hitbox.rearm.is_sorted()) {
                return damaged(format!("a hitbox of move `{label}` re-arms out of order"));
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
