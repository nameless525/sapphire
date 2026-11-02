//! The move system (spec §5.22, steps 2 and 3): cancels, moves on local clocks, and what a move
//! publishes for hits, resolve and presentation.
//!
//! # Exact time
//!
//! All of this runs on each actor's own clock, in whole sub-ticks, and every sub-tick the clock
//! passes belongs to exactly one move. A step's reading `e` of a clock last read at `p` covers
//! `p` to `e`, which may be a third of a tick under slow motion, five ticks under a high time scale
//! or nothing under hitstop. In that span:
//!
//! - **A cancel fires at the earliest instant it can,** with the old move ending there and the new
//!   one starting there: the rest of the span is the new move's. So a window a big step passes over
//!   still fires, and a string plays the same at any speed (give or take when it's read).
//! - **A frozen clock fires nothing.** Its presses wait in the ring, on the same frozen time, and
//!   go on the first step that moves. Only a forced start takes effect at once: what a hit's
//!   reaction wants.
//! - **Each stretch is read once** ([`Stretch`]): cues fire and windows count exactly as
//!   `sapphire_moves` defines, so a frozen step can't fire one twice and a big step can't skip one.
//!
//! # A counter window
//!
//! What a perfect dodge opens (the `resolve` module fires it) is a cancel window on the actor's own clock
//! rather than on a move's: [`MoveRunner::counter`] goes through the same [`OpenWindow`], at the same exact
//! instants, from the same ring, as what a move authors. It's scanned first, so at one instant a
//! press it allows starts the counter, not what the dodge's windows or a neutral start would do with
//! that press. It stays open past the dodge's end, and starting any move closes it: a counter, what a
//! press starts instead and what a hit forces all end the chance.
//!
//! # Order
//!
//! [`crate::CombatSet::Cancels`] ([`settle_cancels`]) walks everything that happens to an actor's
//! moves up to this step's reading, including one ending and a neutral start. [`crate::CombatSet::Moves`]
//! ([`advance_moves`]) reads what's running at this step's reading. Both read other actors'
//! transforms before anyone has moved this tick, so which actor goes first changes nothing.
//!
//! # Replacing a set
//!
//! [`MoveLibrary`] keeps a set's revisions. A move that's running goes on reading the revision it
//! started in, and the next move to start reads the newest, by name if a window of an older revision
//! is what started it. A name that's gone just doesn't fire.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};
use sapphire_moves::{
    Allow, BakedTables, CancelMode, Hitbox, HurtboxChange, Move, MoveSet, PathSample, PerfectDodgeWindow,
    Phase, RootSample, Shape, Span, Stretch, WarpWindow,
};
use sapphire_time::{SimDuration, SimTime};
use xxhash_rust::xxh3::xxh3_64;

use crate::actors::{ActorId, ActorTable};
use crate::cues::{CueEvent, CueQueue};
use crate::intents::{Intent, IntentRing};
use crate::movement::{ActorTransform, RootDelta};
use crate::resolve::Fighter;

/// Most hit volumes one actor publishes in a step. Past this they're counted
/// ([`crate::SimStats::volumes_dropped`]).
pub const MAX_ACTIVE_VOLUMES: usize = 8;

/// Most hurtbox changes one actor publishes in a step, counted the same way.
pub const MAX_ACTIVE_HURTBOXES: usize = 4;

/// Most moves one actor can start in a step, so a chain of zero-length cancels can't spin. Past this
/// the time left goes to whatever's running, and the rest wait for the next step.
const MAX_TRANSITIONS: u32 = 8;

/// Under this many metres of table motion left in a warp window, the warp spreads by time instead.
const MIN_WARP_ARC: f32 = 1e-4;

/// Under this many metres to a target, it gives no direction to turn to or stop short along.
const MIN_TARGET_DISTANCE: f32 = 1e-4;

/// Which set of moves, in a [`MoveLibrary`]. Handed out in the order sets were added, so a replay that
/// adds them in the same order gets the same IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MoveSetId(pub(crate) u32);

impl MoveSetId {
    /// This ID's number, as state checksums and tools show it.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// One move of one revision of a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MoveRef {
    pub set: MoveSetId,
    pub revision: u32,
    pub index: u32,
}

#[derive(Debug)]
struct Revision {
    number: u32,
    set: MoveSet,
    /// The hash of its cooked bytes, kept for [`crate::Simulation::tables_hash`].
    hash: u64,
}

/// The sets a simulation has loaded. Added and replaced between steps, read in them. A move that's
/// running goes on reading the revision it started in, and the next to start reads the newest.
#[derive(Resource, Debug, Default)]
pub struct MoveLibrary {
    /// Each set's revisions still in use, oldest first; the last is current.
    sets: Vec<Vec<Revision>>,
}

fn revision(number: u32, set: MoveSet) -> Revision {
    let cooked = set.to_cooked().expect("a set that compiled writes");
    Revision { number, hash: xxh3_64(&cooked), set }
}

impl MoveLibrary {
    pub(crate) fn add(&mut self, set: MoveSet) -> MoveSetId {
        let id = MoveSetId(u32::try_from(self.sets.len()).expect("fewer than 2³² sets"));
        self.sets.push(vec![revision(0, set)]);
        id
    }

    /// Makes `set` the current revision, and drops older ones `in_use` doesn't name. Returns the new
    /// revision's number, or `None` for an ID this library never gave.
    pub(crate) fn replace(
        &mut self,
        id: MoveSetId,
        set: MoveSet,
        in_use: impl Fn(u32) -> bool,
    ) -> Option<u32> {
        let revisions = self.sets.get_mut(id.0 as usize)?;
        let number = revisions.last().map_or(0, |current| current.number + 1);
        revisions.retain(|revision| in_use(revision.number));
        revisions.push(revision(number, set));
        Some(number)
    }

    fn revisions(&self, id: MoveSetId) -> &[Revision] {
        self.sets.get(id.0 as usize).map_or(&[], Vec::as_slice)
    }

    /// The set's current revision: what the next move starts from.
    #[must_use]
    pub fn current(&self, id: MoveSetId) -> Option<&MoveSet> {
        self.revisions(id).last().map(|revision| &revision.set)
    }

    /// The number of the set's current revision: 0 until it's replaced.
    #[must_use]
    pub fn current_revision(&self, id: MoveSetId) -> Option<u32> {
        self.revisions(id).last().map(|revision| revision.number)
    }

    /// A revision of a set that something still reads.
    #[must_use]
    pub fn revision(&self, id: MoveSetId, number: u32) -> Option<&MoveSet> {
        // From the newest: almost every move reads the current one.
        self.revisions(id)
            .iter()
            .rev()
            .find(|revision| revision.number == number)
            .map(|revision| &revision.set)
    }

    /// The move behind a reference.
    #[must_use]
    pub fn get(&self, at: MoveRef) -> Option<&Move> {
        self.revision(at.set, at.revision)?.move_at(at.index)
    }

    /// The hash of each set's current revision, in ID order.
    pub(crate) fn current_hashes(&self) -> impl ExactSizeIterator<Item = u64> + '_ {
        self.sets.iter().map(|revisions| revisions.last().map_or(0, |revision| revision.hash))
    }
}

/// Who a warp window should bend the move toward. Whoever does targeting writes this, in
/// [`crate::CombatSet::Intents`]; with none, a warp window leaves the table's motion alone.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub enum WarpTarget {
    #[default]
    None,
    Point(Vec3),
    /// Where this actor stood when the step began. One that's gone is no target.
    Actor(ActorId),
}

/// A window a perfect dodge left open, on the actor's own clock: see the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpenCounter {
    /// The dodge whose window says what it allows.
    pub(crate) at: MoveRef,
    /// Its place in that move's `perfect_dodge`.
    pub(crate) window: u32,
    pub(crate) opens: SimTime,
    pub(crate) closes: SimTime,
}

/// A running move's state. See [`CurrentMove`] for the plain read of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Running {
    pub(crate) at: MoveRef,
    pub(crate) started_at: SimTime,
    /// Which playing this is, of all the actor's: hit dedupe resets when it changes.
    pub(crate) serial: u32,
    /// Nothing of it has been read yet.
    pub(crate) fresh: bool,
    /// The table's root space: the yaw when it began, plus what warp has turned it.
    pub(crate) frame_yaw: f32,
    /// The table's last reading.
    pub(crate) reading: RootSample,
    /// The warp window the two counts below belong to.
    pub(crate) warp_window: Option<u32>,
    pub(crate) warp_distance: f32,
    pub(crate) warp_turn: f32,
    /// A perfect dodge has fired in this playing, so it won't again.
    pub(crate) dodged: bool,
}

/// What an actor's moves are doing. Reset for each new actor, and in the checksum, every field.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveRunner {
    pub(crate) set: Option<MoveSetId>,
    pub(crate) running: Option<Running>,
    /// How far along its own clock this actor's moves have been read.
    pub(crate) cursor: SimTime,
    /// Moves it has started, all told.
    pub(crate) started: u32,
    pub(crate) forced: Option<Name>,
    /// What a perfect dodge left open.
    pub(crate) counter: Option<OpenCounter>,
}

impl MoveRunner {
    pub(crate) fn new(set: Option<MoveSetId>) -> Self {
        Self { set, ..Self::default() }
    }

    /// The set it plays moves from.
    #[must_use]
    pub const fn set(&self) -> Option<MoveSetId> {
        self.set
    }

    /// Whether a move is playing.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Moves it has started, all told.
    #[must_use]
    pub const fn started(&self) -> u32 {
        self.started
    }

    /// Starts this move on the next step, whatever is running and whatever the clock is doing: a
    /// hit's reaction (M2 G3), which no window has to allow. A name not in the current set does
    /// nothing. Call it between steps or from inside one.
    pub fn force(&mut self, name: Name) {
        self.forced = Some(name);
    }
}

/// A fixed list of what a step publishes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Published<T: Copy, const N: usize> {
    items: [Option<T>; N],
    len: u8,
}

impl<T: Copy, const N: usize> Default for Published<T, N> {
    fn default() -> Self {
        Self { items: [None; N], len: 0 }
    }
}

impl<T: Copy, const N: usize> Published<T, N> {
    /// The list, in the order published.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = T> + '_ {
        self.items[..usize::from(self.len)].iter().map(|item| item.expect("the first `len` are filled"))
    }

    /// How many were published.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Whether none were.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns whether there was room.
    fn push(&mut self, item: T) -> bool {
        let Some(slot) = self.items.get_mut(usize::from(self.len)) else { return false };
        *slot = Some(item);
        self.len += 1;
        true
    }

    /// The first, to change, for tests of what's watching them.
    #[cfg(test)]
    pub(crate) fn first_mut(&mut self) -> Option<&mut T> {
        self.items[..usize::from(self.len)].first_mut()?.as_mut()
    }

    /// One of the list, by its place.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<T> {
        self.items[..usize::from(self.len)].get(index).copied().flatten()
    }

    /// Empties the list, for tests of what's watching it.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// A hit volume this step: what D4 tests and T1 draws. Everything about it is in here, so reading
/// one needs no library.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveVolume {
    /// The move it belongs to: one this step's cancel may have ended.
    pub at: MoveRef,
    /// Which playing of a move it belongs to: targets hit are remembered per `(serial, name)`.
    pub serial: u32,
    /// When that playing began, on the actor's clock.
    pub started_at: SimTime,
    /// Its place in the move's [`Move::hitboxes`], where what it does when it lands is.
    pub hitbox: u32,
    /// The hit's name.
    pub name: Name,
    /// The path it rides.
    pub path: Name,
    pub shape: Shape,
    /// What this step covers of the volume's arc, in time into the move. One instant if it just
    /// appeared or the clock is frozen.
    pub from: SimDuration,
    pub to: SimDuration,
    /// The path at `from` and `to`, in the root's space at those times: see
    /// [`ActorTransform::point`].
    pub start: PathSample,
    pub end: PathSample,
}

/// A hurtbox change this step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveHurtbox {
    pub name: Name,
    pub state: sapphire_moves::HurtboxState,
}

/// Which windows count this step, as [`Span::live`] defines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveWindows {
    pub invulnerable: bool,
    /// A perfect-dodge window counts: [`MoveOutput::perfect_window`] says whether it can fire.
    pub perfect_dodge: bool,
    /// Published, and nothing reads it: M2's fight has no parry.
    pub parry: bool,
    pub cancel: bool,
    pub warp: bool,
    /// The highest threshold of the armour windows that count this step, or 0 with none: a hit taking
    /// less poise than this plays no reaction.
    pub armour: u32,
}

/// What an actor's moves published this step, for hits, resolve and presentation. Cleared each
/// step, and in the checksum, since a later step's hits read it the same tick.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveOutput {
    /// What moves want of the root this step: table motion and warp, in world space.
    pub root_delta: RootDelta,
    /// Where the root was before this step's motion. With [`ActorTransform`], what a hit sweep runs
    /// between.
    pub previous: ActorTransform,
    pub windows: LiveWindows,
    pub volumes: Published<ActiveVolume, MAX_ACTIVE_VOLUMES>,
    pub hurtboxes: Published<ActiveHurtbox, MAX_ACTIVE_HURTBOXES>,
    /// Which of the running move's perfect-dodge windows can fire this step: one with a volume that this
    /// step's reading counts in, while the clock moved, the actor is invulnerable and the playing hasn't
    /// fired one yet. So a hit and a perfect dodge never land on an actor in one step.
    pub perfect_window: Option<u32>,
}

/// The move an actor is playing, as plain data: what N3's combat layer and T1's view read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurrentMove {
    pub at: MoveRef,
    pub name: Name,
    /// Which playing this is, of all the actor's: 1 for its first move.
    pub serial: u32,
    pub phase: Phase,
    /// How far into the move, on the actor's own clock, in exact sub-ticks.
    pub elapsed: SimDuration,
    /// The move's whole length.
    pub ticks: u32,
    pub started_at: SimTime,
    /// The table's last reading of the root.
    pub reading: RootSample,
    /// The table's root space: the yaw it began at, plus what warp turned it.
    pub frame_yaw: f32,
    /// Whether a perfect dodge has fired in this playing.
    pub dodged: bool,
}

impl CurrentMove {
    /// Whole ticks into the move.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.elapsed.whole_ticks()
    }
}

/// What an actor's moves are doing, as plain data.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveRead {
    pub set: Option<MoveSetId>,
    pub current: Option<CurrentMove>,
    /// Moves it has started, all told.
    pub started: u32,
    pub windows: LiveWindows,
    pub volumes: Published<ActiveVolume, MAX_ACTIVE_VOLUMES>,
    pub hurtboxes: Published<ActiveHurtbox, MAX_ACTIVE_HURTBOXES>,
    /// What moves want of the root this step, warp included.
    pub root_delta: RootDelta,
    /// Where the root was before this step's motion. With [`ActorView`](crate::ActorView)'s position
    /// and yaw, what a hit sweep runs between.
    pub previous: ActorTransform,
    /// When a window a perfect dodge left open closes, on the actor's clock, if one is open.
    pub counter_closes_at: Option<SimTime>,
}

impl MoveRead {
    pub(crate) fn of(runner: &MoveRunner, output: &MoveOutput, library: &MoveLibrary) -> Self {
        let current = runner.running.and_then(|running| {
            let mv = library.get(running.at)?;
            let elapsed = runner.cursor.since(running.started_at);
            Some(CurrentMove {
                at: running.at,
                name: mv.name(),
                serial: running.serial,
                phase: mv.phase_at(elapsed),
                elapsed,
                ticks: mv.ticks(),
                started_at: running.started_at,
                reading: running.reading,
                frame_yaw: running.frame_yaw,
                dodged: running.dodged,
            })
        });
        Self {
            set: runner.set,
            current,
            started: runner.started,
            windows: output.windows,
            volumes: output.volumes,
            hurtboxes: output.hurtboxes,
            root_delta: output.root_delta,
            previous: output.previous,
            counter_closes_at: runner.counter.map(|counter| counter.closes),
        }
    }
}

/// The move system's counters, deterministic but not state: nothing reads them back.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MoveStats {
    pub(crate) intents_pressed: u64,
    pub(crate) intents_expired: u64,
    pub(crate) intents_overwritten: u64,
    pub(crate) moves_started: u64,
    pub(crate) cancels_fired: u64,
    pub(crate) counters_fired: u64,
    pub(crate) volumes_dropped: u64,
    pub(crate) transition_limit: u64,
}

/// What one actor's stage needs, gathered so the helpers below don't take a dozen arguments.
struct Stage<'a> {
    id: ActorId,
    tick: u64,
    library: &'a MoveLibrary,
    cues: &'a mut CueQueue,
    stats: &'a mut MoveStats,
    origin: ActorTransform,
    /// The warp target's position when the step began.
    target: Option<Vec3>,
    output: &'a mut MoveOutput,
}

fn target_position(
    target: WarpTarget,
    table: &ActorTable,
    transforms: &Query<'_, '_, &ActorTransform>,
) -> Option<Vec3> {
    match target {
        WarpTarget::None => None,
        WarpTarget::Point(point) => Some(point),
        WarpTarget::Actor(id) => {
            let entity = table.entity(id)?;
            Some(transforms.get(entity).ok()?.position)
        }
    }
}

/// [`crate::CombatSet::Cancels`]: everything that changes which move an actor is in, up to this
/// step's reading, and then what's left in its intent ring expires.
#[allow(clippy::type_complexity, reason = "a system's query reads as what it touches")]
pub(crate) fn settle_cancels(
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    world_clock: Res<'_, WorldClock>,
    mut cues: ResMut<'_, CueQueue>,
    mut stats: ResMut<'_, MoveStats>,
    mut actors: Query<
        '_,
        '_,
        (&EntityClock, &mut MoveRunner, &mut IntentRing, &mut MoveOutput, &WarpTarget, &Fighter),
    >,
    transforms: Query<'_, '_, &ActorTransform>,
) {
    let tick = world_clock.0.ticks();
    for (id, entity) in table.iter() {
        let Ok((clock, mut runner, mut ring, mut output, target, fighter)) = actors.get_mut(entity) else {
            continue;
        };
        *output = MoveOutput::default();
        let Some(set) = runner.set else { continue };
        // A defeated actor starts nothing more: with nothing in its ring, only a forced start can fire.
        if fighter.is_defeated() {
            ring.reset();
        }
        let Ok(origin) = transforms.get(entity) else { continue };
        let target = target_position(*target, &table, &transforms);
        let stage = Stage {
            id,
            tick,
            library: &library,
            cues: &mut cues,
            stats: &mut stats,
            origin: *origin,
            target,
            output: &mut output,
        };
        let frozen = clock.0.last_delta() == SimDuration::ZERO;
        settle(stage, set, &mut runner, &mut ring, clock.0.now(), frozen);
        if runner.counter.is_some_and(|counter| counter.closes <= clock.0.now()) {
            runner.counter = None;
        }
        stats.intents_expired += ring.expire(clock.0.now());
    }
}

/// See [`settle_cancels`] and the [module docs](self).
fn settle(
    mut stage: Stage<'_>,
    set: MoveSetId,
    runner: &mut MoveRunner,
    ring: &mut IntentRing,
    now: SimTime,
    frozen: bool,
) {
    let library = stage.library;
    let (Some(current), Some(current_revision)) = (library.current(set), library.current_revision(set))
    else {
        return;
    };
    let reference = |index: u32| MoveRef { set, revision: current_revision, index };

    if let Some(name) = runner.forced.take()
        && let Some((index, _)) = current.find(name)
    {
        let at = runner.cursor;
        end_running(&mut stage, runner, at);
        start_move(&mut stage, runner, reference(index), at);
    }
    if frozen {
        return;
    }

    for _ in 0..MAX_TRANSITIONS {
        match runner.running {
            None => {
                let window = OpenWindow {
                    // Open since the last move ended, which is as far back as `reached` can go.
                    opens: runner.cursor.as_subticks(),
                    closes: u64::MAX,
                    reached: runner.cursor.as_subticks(),
                    unread: runner.cursor.as_subticks().saturating_add(1),
                    limit: now.as_subticks(),
                };
                let mut best = None;
                scan_counter(runner, library, current, current_revision, &window, ring, &mut best);
                window.scan(current.neutral(), true, current, ring, &mut best);
                let Some(fire) = best else { return };
                take(&mut stage, runner, ring, fire, reference(fire.target));
            }
            Some(running) => {
                let Some(mv) = library.get(running.at) else {
                    runner.running = None;
                    return;
                };
                let same_revision = running.at.revision == current_revision;
                let ends_at = running.started_at + mv.duration();
                let reached = runner.cursor.as_subticks();
                let unread = if running.fresh { reached } else { reached.saturating_add(1) };
                let mut best = None;
                // A counter window can outlast the move: what's past the move's end waits for that.
                let limit = now.as_subticks().min(ends_at.as_subticks());
                let bounds = OpenWindow { opens: 0, closes: 0, reached, unread, limit };
                scan_counter(runner, library, current, current_revision, &bounds, ring, &mut best);
                for cancel in &mv.cancels {
                    let window = OpenWindow {
                        opens: (running.started_at + cancel.span.start()).as_subticks(),
                        closes: (running.started_at + cancel.span.end()).as_subticks(),
                        reached,
                        unread,
                        limit: now.as_subticks(),
                    };
                    window.scan(&cancel.allow, same_revision, current, ring, &mut best);
                }
                if let Some(fire) = best {
                    take(&mut stage, runner, ring, fire, reference(fire.target));
                    stage.stats.cancels_fired += 1;
                } else if ends_at <= now {
                    end_running(&mut stage, runner, ends_at);
                    runner.running = None;
                    runner.cursor = ends_at;
                } else {
                    return;
                }
            }
        }
    }
    stage.stats.transition_limit += 1;
}

/// A window as a moment on the actor's clock, checked against the ring.
struct OpenWindow {
    opens: u64,
    closes: u64,
    /// How far the actor's moves have been read: a cancel can't fire before this.
    reached: u64,
    /// The first instant not yet read.
    unread: u64,
    /// This step's reading. A window closes inside its move, so this needs no clamp to the move's end.
    limit: u64,
}

/// A cancel that could fire, with what decides between two of them.
#[derive(Clone, Copy)]
struct Fire {
    at: u64,
    priority: u8,
    /// Where its press is in the ring, oldest first; `None` for a hold.
    slot: Option<usize>,
    target: u32,
    /// Whether a counter window is what it fires from.
    counter: bool,
}

impl Fire {
    /// The earlier wins, then the higher priority, then the older press, then a press over a hold;
    /// among equals, what was authored first.
    fn beats(&self, other: &Self) -> bool {
        let rank = |fire: &Self| (fire.at, u8::MAX - fire.priority, fire.slot.unwrap_or(usize::MAX));
        rank(self) < rank(other)
    }
}

impl OpenWindow {
    /// When a press could fire in this window, if ever: never before it opens or before it was
    /// pressed (unless the step it came on passed the whole window over), and at the latest before
    /// the window closes and the press expires. See the [module docs](self).
    fn fire_time(&self, mode: CancelMode, press: &Intent) -> Option<u64> {
        let pressed = press.pressed_at.as_subticks();
        let could_fire = if pressed < self.closes {
            if mode == CancelMode::Instant && pressed < self.opens {
                return None;
            }
            self.opens.max(pressed)
        } else if press.step_start.as_subticks() < self.opens {
            self.opens
        } else {
            return None;
        };
        let at = could_fire.max(self.reached);
        (at <= self.limit && at < self.closes && at < press.expires_at.as_subticks()).then_some(at)
    }

    /// When a hold could fire in this window, if ever. A hold is only known at a step's reading, so
    /// that's when: while the reading is in the window, or at its opening if this step passed over
    /// all of it, as [`Span::live`] counts it.
    fn hold_time(&self) -> Option<u64> {
        if self.limit < self.closes {
            (self.opens <= self.limit).then_some(self.limit)
        } else {
            (self.unread <= self.opens).then_some(self.opens)
        }
    }

    /// Considers everything `allows` lets the ring do in this window.
    fn scan(
        &self,
        allows: &[Allow],
        same_revision: bool,
        current: &MoveSet,
        ring: &IntentRing,
        best: &mut Option<Fire>,
    ) {
        for allow in allows {
            let target = if same_revision {
                Some(allow.target())
            } else {
                current.find(allow.target_name()).map(|(index, _)| index)
            };
            let Some(target) = target else { continue };
            let mut consider = |fire: Fire| {
                if best.is_none_or(|best| fire.beats(&best)) {
                    *best = Some(fire);
                }
            };
            if allow.mode == CancelMode::Held {
                let Some((bit, intent)) = current.intent(allow.intent()) else { continue };
                if ring.held() & (1 << bit) != 0
                    && let Some(at) = self.hold_time()
                {
                    consider(Fire { at, priority: intent.priority, slot: None, target, counter: false });
                }
                continue;
            }
            for (slot, press) in ring.iter().enumerate() {
                if press.name != allow.intent() {
                    continue;
                }
                if let Some(at) = self.fire_time(allow.mode, press) {
                    consider(Fire { at, priority: press.priority, slot: Some(slot), target, counter: false });
                }
            }
        }
    }
}

/// Considers what the actor's counter window lets the ring do, with `bounds` saying how far this step has
/// been read and may reach. Goes first, so among equals a counter stands. See the [module docs](self).
fn scan_counter(
    runner: &MoveRunner,
    library: &MoveLibrary,
    current: &MoveSet,
    current_revision: u32,
    bounds: &OpenWindow,
    ring: &IntentRing,
    best: &mut Option<Fire>,
) {
    let Some(counter) = runner.counter else { return };
    let window = library.get(counter.at).and_then(|mv| mv.perfect_dodge.get(counter.window as usize));
    let Some(allow) = window.and_then(|window| window.counter.as_ref()) else { return };
    let window = OpenWindow {
        opens: counter.opens.as_subticks(),
        closes: counter.closes.as_subticks(),
        reached: bounds.reached,
        unread: bounds.unread,
        limit: bounds.limit,
    };
    window.scan(&allow.allow, counter.at.revision == current_revision, current, ring, best);
    if let Some(fire) = best {
        fire.counter = true;
    }
}

/// Fires a cancel or a neutral start: its press is used up, whatever's running ends at that instant,
/// and the new move begins there.
fn take(stage: &mut Stage<'_>, runner: &mut MoveRunner, ring: &mut IntentRing, fire: Fire, target: MoveRef) {
    if let Some(slot) = fire.slot {
        ring.remove(slot);
    }
    stage.stats.counters_fired += u64::from(fire.counter);
    let at = SimTime::from_subticks(fire.at);
    end_running(stage, runner, at);
    start_move(stage, runner, target, at);
}

/// Reads what's left of the running move up to `at`, where something cuts it off.
fn end_running(stage: &mut Stage<'_>, runner: &mut MoveRunner, at: SimTime) {
    let Some(mut running) = runner.running else { return };
    let Some(mv) = stage.library.get(running.at) else { return };
    let previous = (!running.fresh).then(|| runner.cursor.since(running.started_at));
    let stretch = Stretch::cut(previous, at.since(running.started_at));
    read_stretch(stage, mv, &mut running, stretch);
    runner.running = Some(running);
}

/// Begins a move at `at` on the actor's clock, in the frame the root has reached by then.
fn start_move(stage: &mut Stage<'_>, runner: &mut MoveRunner, target: MoveRef, at: SimTime) {
    runner.started = runner.started.wrapping_add(1);
    runner.cursor = at;
    runner.counter = None;
    runner.running = Some(Running {
        at: target,
        started_at: at,
        serial: runner.started,
        fresh: true,
        frame_yaw: stage.origin.yaw + stage.output.root_delta.yaw,
        reading: RootSample::ZERO,
        warp_window: None,
        warp_distance: 0.0,
        warp_turn: 0.0,
        dodged: false,
    });
    stage.stats.moves_started += 1;
}

/// [`crate::CombatSet::Moves`]: each move advances to this step's reading of its actor's clock.
#[allow(clippy::type_complexity, reason = "a system's query reads as what it touches")]
pub(crate) fn advance_moves(
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    world_clock: Res<'_, WorldClock>,
    mut cues: ResMut<'_, CueQueue>,
    mut stats: ResMut<'_, MoveStats>,
    mut actors: Query<'_, '_, (&EntityClock, &mut MoveRunner, &mut MoveOutput, &WarpTarget)>,
    transforms: Query<'_, '_, &ActorTransform>,
) {
    let tick = world_clock.0.ticks();
    for (id, entity) in table.iter() {
        let Ok((clock, mut runner, mut output, target)) = actors.get_mut(entity) else { continue };
        let now = clock.0.now();
        let Some(mut running) = runner.running else {
            runner.cursor = now;
            continue;
        };
        let Some(mv) = library.get(running.at) else { continue };
        let Ok(origin) = transforms.get(entity) else { continue };
        let target = target_position(*target, &table, &transforms);
        let mut stage = Stage {
            id,
            tick,
            library: &library,
            cues: &mut cues,
            stats: &mut stats,
            origin: *origin,
            target,
            output: &mut output,
        };
        let previous = (!running.fresh).then(|| runner.cursor.since(running.started_at));
        let elapsed = now.since(running.started_at);
        if elapsed >= mv.duration() {
            // Only past `MAX_TRANSITIONS`: otherwise a move's end was settled with the cancels.
            read_stretch(&mut stage, mv, &mut running, Stretch::cut(previous, mv.duration()));
            runner.running = None;
        } else {
            let stretch = Stretch::read(previous, elapsed);
            read_stretch(&mut stage, mv, &mut running, stretch);
            if !running.dodged && clock.0.last_delta() != SimDuration::ZERO && stage.output.windows.invulnerable {
                let open = |window: &PerfectDodgeWindow| window.volume.is_some() && window.span.live(stretch);
                stage.output.perfect_window = mv.perfect_dodge.iter().position(open).map(|at| at as u32);
            }
            running.fresh = false;
            runner.running = Some(running);
        }
        runner.cursor = now;
    }
}

/// Reads one stretch of a move: its cues fire, its windows publish, its root moves.
fn read_stretch(stage: &mut Stage<'_>, mv: &Move, running: &mut Running, stretch: Stretch) {
    for cue in &mv.cues {
        if stretch.contains(cue.time()) {
            stage.cues.push(CueEvent {
                actor: stage.id,
                cue: cue.name(),
                tick: stage.tick,
                local_time: running.started_at + cue.time(),
                move_time: cue.time(),
                hit: None,
            });
        }
    }

    let live = |spans: &[Span]| spans.iter().any(|span| span.live(stretch));
    let windows = &mut stage.output.windows;
    windows.invulnerable |= live(&mv.invulnerable);
    windows.perfect_dodge |= mv.perfect_dodge.iter().any(|window| window.span.live(stretch));
    windows.parry |= live(&mv.parry);
    windows.cancel |= mv.cancels.iter().any(|window| window.span.live(stretch));
    windows.warp |= mv.warps.iter().any(|window| window.span.live(stretch));
    for window in &mv.armour {
        if window.span.live(stretch) {
            windows.armour = windows.armour.max(window.threshold);
        }
    }

    for (index, hitbox) in mv.hitboxes.iter().enumerate() {
        publish_volume(stage, mv, running, index as u32, hitbox, stretch);
    }
    for change in &mv.hurtboxes {
        publish_hurtbox(stage, change, stretch);
    }
    walk_root(stage, mv, running, stretch);
}

fn publish_volume(
    stage: &mut Stage<'_>,
    mv: &Move,
    running: &Running,
    index: u32,
    hitbox: &Hitbox,
    stretch: Stretch,
) {
    let Some((from, to)) = hitbox.span.sweep(stretch) else { return };
    let Some(path) = mv.tables.path(hitbox.path()) else { return };
    let published = stage.output.volumes.push(ActiveVolume {
        at: running.at,
        serial: running.serial,
        started_at: running.started_at,
        hitbox: index,
        name: hitbox.name(),
        path: hitbox.path(),
        shape: hitbox.shape,
        from,
        to,
        start: path.at(from),
        end: path.at(to),
    });
    stage.stats.volumes_dropped += u64::from(!published);
}

fn publish_hurtbox(stage: &mut Stage<'_>, change: &HurtboxChange, stretch: Stretch) {
    if change.span.live(stretch) {
        let active = ActiveHurtbox { name: change.name(), state: change.state };
        let published = stage.output.hurtboxes.push(active);
        stage.stats.volumes_dropped += u64::from(!published);
    }
}

/// Walks the root along the table through this stretch, bending it inside warp windows. Split at
/// their edges, so a big step reaches the same place as many small ones.
fn walk_root(stage: &mut Stage<'_>, mv: &Move, running: &mut Running, stretch: Stretch) {
    let end = stretch.end();
    let mut at = stretch.previous().unwrap_or(SimDuration::ZERO);
    if end <= at {
        return;
    }
    for (index, warp) in mv.warps.iter().enumerate() {
        if warp.span.end() <= at {
            continue;
        }
        if warp.span.start() >= end {
            break;
        }
        let from = warp.span.start().max(at);
        follow_table(stage, &mv.tables, running, from);
        let to = warp.span.end().min(end);
        bend_toward_target(stage, &mv.tables, running, index as u32, warp, from, to);
        follow_table(stage, &mv.tables, running, to);
        at = to;
    }
    follow_table(stage, &mv.tables, running, end);
}

/// Moves the root by the table's own motion from its last reading to `to`.
fn follow_table(stage: &mut Stage<'_>, tables: &BakedTables, running: &mut Running, to: SimDuration) {
    let reading = tables.root_at(to);
    let moved = reading.offset() - running.reading.offset();
    stage.output.root_delta.translation += det::rotate_y(moved, running.frame_yaw);
    stage.output.root_delta.yaw += reading.yaw - running.reading.yaw;
    running.reading = reading;
}

/// Adds this part of a warp window's correction: the difference between where the table will leave
/// the root at the window's end and where the target wants it, shared out in proportion to the
/// table's own motion, so the root glides only while it's moving anyway. Re-aimed every step, so a
/// moving target stays tracked, and the last step makes up what's left exactly, within the limits.
fn bend_toward_target(
    stage: &mut Stage<'_>,
    tables: &BakedTables,
    running: &mut Running,
    index: u32,
    warp: &WarpWindow,
    from: SimDuration,
    to: SimDuration,
) {
    let Some(goal) = stage.target else { return };
    if running.warp_window != Some(index) {
        running.warp_window = Some(index);
        running.warp_distance = 0.0;
        running.warp_turn = 0.0;
    }
    let window_end = warp.span.end();
    let share = motion_share(tables, from, to, window_end);
    let output = &mut *stage.output;
    let here = stage.origin.position + output.root_delta.translation;
    let at_end = tables.root_at(window_end);
    let to_goal = horizontal(goal - here);
    let distance = to_goal.length();

    if warp.max_turn > 0.0 && distance > MIN_TARGET_DISTANCE {
        let facing_at_end = stage.origin.yaw + output.root_delta.yaw + (at_end.yaw - running.reading.yaw);
        let left = (warp.max_turn - running.warp_turn).max(0.0);
        let error = det::wrap_angle(det::yaw_of_direction(to_goal) - facing_at_end);
        let turn = error.clamp(-left, left) * share;
        running.warp_turn += turn.abs();
        running.frame_yaw += turn;
        output.root_delta.yaw += turn;
    }

    if warp.max_distance > 0.0 {
        let table_left = det::rotate_y(at_end.offset() - running.reading.offset(), running.frame_yaw);
        let arrives = here + table_left;
        let stand =
            if distance > MIN_TARGET_DISTANCE { goal - to_goal * (warp.keep / distance) } else { here };
        let mut correction = horizontal(stand - arrives);
        let (length, left) = (correction.length(), (warp.max_distance - running.warp_distance).max(0.0));
        if length > left {
            correction *= left / length;
        }
        let shift = correction * share;
        running.warp_distance += shift.length();
        output.root_delta.translation += shift;
    }
}

const fn horizontal(vector: Vec3) -> Vec3 {
    Vec3::new(vector.x, 0.0, vector.z)
}

/// How much of what's left of a warp window, `from` to `window_end`, is `from` to `to`: by distance
/// along the table's motion, or by time if the table is barely moving. Exactly 1 at the window's end.
fn motion_share(tables: &BakedTables, from: SimDuration, to: SimDuration, window_end: SimDuration) -> f32 {
    if to >= window_end {
        return 1.0;
    }
    let done = arc(tables, from, to);
    let whole = done + arc(tables, to, window_end);
    if whole > MIN_WARP_ARC {
        done / whole
    } else {
        let time = (window_end - from).as_subticks().max(1);
        (to - from).as_subticks() as f32 / time as f32
    }
}

/// The length of the root's path through the table between two times.
fn arc(tables: &BakedTables, from: SimDuration, to: SimDuration) -> f32 {
    let mut length = 0.0;
    let mut previous = tables.root_at(from).offset();
    let mut tick = from.whole_ticks() + 1;
    while SimDuration::from_ticks(tick) < to {
        let here = tables.root_at(SimDuration::from_ticks(tick)).offset();
        length += (here - previous).length();
        previous = here;
        tick += 1;
    }
    length + (tables.root_at(to).offset() - previous).length()
}
