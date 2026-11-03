//! What a script is to the simulation (spec §5.24, M3 L2): events that hooks answer, a fixed queue of
//! typed commands, and a fixed board of numbers. All of it is plain Rust with no idea of Luau, which is
//! `sapphire_script`'s: it stands above this crate, adds what fires hooks with
//! [`Simulation::add_combat_system`](crate::Simulation::add_combat_system), and reads and writes
//! [`ScriptWorld`].
//!
//! **A step's three points,** in order, late enough that everything a hook reads has settled:
//!
//! 1. [`CombatSet::HookEvents`](crate::CombatSet): `gather_hook_events` notes what happened and writes
//!    each actor's numbers into a [`HookFrame`]. On a quiet step that's the whole cost.
//! 2. [`CombatSet::Hooks`](crate::CombatSet): whoever runs hooks answers [`ScriptWorld::events`]. A hook
//!    reads the frame, which no hook can change but for the board, so each sees the same world, and its
//!    only outputs are commands and the board.
//! 3. [`CombatSet::Commands`](crate::CombatSet): `apply_script_commands` is the one point they take
//!    effect, in order, each going into a chain the checksum holds.
//!
//! **A simulation with no scripts has none of this:** [`ScriptWorld`] goes in with
//! [`Simulation::enable_scripts`](crate::Simulation::enable_scripts), and the checksum and `tables_hash`
//! write it only if it's there. So a fight with no script hashes as it did.
//!
//! Nothing of a VM's own state is relied on between hooks, so a reload can't change a replay: what lasts
//! is here.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::{Pcg32, Vec3};

use crate::actors::{ActorId, ActorRequests, ActorTable};
use crate::brain::Brain;
use crate::checksum::StateHasher;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveRunner};
use crate::resolve::{AppliedHits, Fighter, SlowMotion};
use crate::simulation::SimRng;
use crate::targeting::SoftLock;

/// Numbers that last between hooks: the only state a script has.
pub const BOARD_SLOTS: usize = 64;

/// Commands a step's hooks may leave, all told. A hook that would go past this leaves none of its own.
pub const QUEUE_LEN: usize = 64;

/// Events one step can hold. Past it they're counted, in the checksum, and go unanswered: room for a
/// crowd's hits on one tick with phases and defeats besides.
pub const MAX_HOOK_EVENTS: usize = 64;

/// How many phases a fighter's health holds: `on_phase` fires as it crosses each quarter. B3's documents
/// will choose where a boss's fall.
const PHASES: u32 = 4;

/// Something a hook can answer. Everything it carries is what happened, in the simulation's own terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HookEvent {
    /// A fighter's health crossed into a later phase, 1 being the first crossing.
    Phase { actor: ActorId, phase: u8, health: u32 },
    /// An actor started a move.
    MoveStarted { actor: ActorId, name: Name },
    /// A hit was applied.
    Hit { attacker: ActorId, target: ActorId, damage: u32 },
    /// A fighter fell.
    Defeated { actor: ActorId },
    /// The tick is a multiple of [`ScriptWorld::set_beat_every`]'s.
    Beat { tick: u64 },
}

/// Which events are worth gathering, because a hook is bound to them. What's watched moves on whatever
/// this says, so binding a hook later doesn't answer the past.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookWants {
    pub phases: bool,
    pub moves: bool,
    pub hits: bool,
    pub defeats: bool,
}

/// What a hook asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ScriptCommand {
    #[default]
    None,
    /// The world's clock runs at this rate for this many steps. [`SlowMotion::request`] has the rule.
    SlowMotion { per_mille: u32, steps: u16 },
    /// Nothing but a number in the checksum: how a hook leaves a mark that it ran.
    Mark(f64),
    /// Starts this move on the next step, whatever's running: [`MoveRunner::force`]. A fallen fighter stays
    /// down.
    ForceMove { actor: ActorId, name: Name },
    /// From the next step, this actor's set says how it fights, if it says so at all. One already fighting
    /// keeps its brain as it is, and a fallen fighter gets none.
    GiveBrain { actor: ActorId },
    /// From the next step, this actor chooses who it's aiming at. One already choosing keeps its choice.
    GiveSoftLock { actor: ActorId },
}

/// An actor's numbers, as a hook's queries read them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorFacts {
    pub id: ActorId,
    pub health: u32,
    pub max_health: u32,
    pub defeated: bool,
    /// Its [`Faction`](crate::Faction)'s number.
    pub faction: u8,
    pub position: Vec3,
    /// Who its soft lock has chosen.
    pub soft_lock: Option<ActorId>,
    /// Who its brain has picked.
    pub brain_target: Option<ActorId>,
}

/// What a hook reads and writes: the world as this step left it, the board, the queue. Nothing here
/// points past the step.
#[derive(Debug)]
pub struct HookFrame {
    /// The simulation's tick.
    pub tick: u64,
    /// Every live actor, in ID order, up to `actors_len`: written once for a step, and read by a search.
    actors: Box<[Option<ActorFacts>]>,
    actors_len: usize,
    /// The simulation-owned numbers that last.
    pub board: [f64; BOARD_SLOTS],
    queue: [ScriptCommand; QUEUE_LEN],
    queued: usize,
    /// The simulation's randomness, copied in for a step's hooks and back after them.
    pub rng: Pcg32,
    /// A binding was handed something it can't use, or the queue ran out: this hook's commands don't count.
    pub fault: bool,
    /// What the bound chunk calls names: a command's number is a place in this, counting from 1, since
    /// a binding takes only numbers and a hash doesn't fit in one.
    names: Vec<Name>,
}

impl HookFrame {
    /// A frame with room for `actors` actors, and nothing asked for.
    #[must_use]
    pub fn new(actors: usize) -> Self {
        Self {
            tick: 0,
            actors: vec![None; actors].into_boxed_slice(),
            actors_len: 0,
            board: [0.0; BOARD_SLOTS],
            queue: [ScriptCommand::None; QUEUE_LEN],
            queued: 0,
            rng: Pcg32::new(0, 0),
            fault: false,
            names: Vec::new(),
        }
    }

    /// A live actor's numbers.
    #[must_use]
    pub fn actor(&self, raw: u32) -> Option<&ActorFacts> {
        let live = &self.actors[..self.actors_len];
        let found = live.binary_search_by_key(&raw, |facts| facts.map_or(0, |facts| facts.id.raw())).ok()?;
        live[found].as_ref()
    }

    /// Asks for something. Past the queue's end that's a fault, so a hook is whole or it's nothing.
    pub fn push(&mut self, command: ScriptCommand) {
        if let Some(slot) = self.queue.get_mut(self.queued) {
            *slot = command;
            self.queued += 1;
        } else {
            self.fault = true;
        }
    }

    /// What's been asked for and not yet applied.
    #[must_use]
    pub fn queued(&self) -> &[ScriptCommand] {
        &self.queue[..self.queued]
    }

    /// Takes back what's been asked for past the first `queued`: what a hook that ended badly left.
    pub fn truncate_queue(&mut self, queued: usize) {
        self.queued = self.queued.min(queued);
    }

    /// The chunk's list of names, whole.
    pub fn set_names(&mut self, names: Vec<Name>) {
        self.names = names;
    }

    /// The name at a chunk's place for it, counting from 1.
    #[must_use]
    pub fn name(&self, place: usize) -> Option<Name> {
        self.names.get(place.checked_sub(1)?).copied()
    }

    /// The place in the chunk's list of `name`, counting from 1, or 0 if it doesn't list it.
    #[must_use]
    pub fn place_of(&self, name: Name) -> usize {
        self.names.iter().position(|&listed| listed == name).map_or(0, |place| place + 1)
    }

    /// Writes what each hook will read, to a test's choosing: `gather_hook_events` does it in a step.
    pub fn set_actors(&mut self, actors: impl IntoIterator<Item = ActorFacts>) {
        self.actors_len = 0;
        for (slot, facts) in self.actors.iter_mut().zip(actors) {
            *slot = Some(facts);
            self.actors_len += 1;
        }
    }
}

/// The quarters `health` has fallen through, 0 to [`PHASES`], or `None` for a fighter with no health. In
/// `u64`, so a health near `u32::MAX` can't overflow; several crossed in one step make one event, at the
/// latest, so a hook never answers a phase the fighter has already passed.
fn phase_of(health: u32, max: u32) -> Option<u8> {
    (u64::from(max - health.min(max)) * u64::from(PHASES))
        .checked_div(u64::from(max))
        .map(|phase| phase as u8)
}

/// What one actor last showed: what events are the differences of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActorWatch {
    id: ActorId,
    /// Moves it had started.
    started: u32,
    phase: u8,
    defeated: bool,
}

/// How a hook ended, in the checksum: a hook that fails on one run and not another is a divergence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HookEnding {
    Returned = 0,
    /// It returned, but a binding had been handed something it can't use.
    Faulted = 1,
    OverBudget = 2,
    Failed = 3,
    /// The step's budget had gone before its turn, and it never ran.
    Skipped = 4,
}

/// A simulation's script state: see the [module docs](self). Everything that lasts is in the checksum.
#[derive(Resource, Debug)]
pub struct ScriptWorld {
    frame: HookFrame,
    events: [Option<HookEvent>; MAX_HOOK_EVENTS],
    events_len: usize,
    events_dropped: u64,
    /// By ID, up to `watched`: IDs only grow, so a step merges its live actors in place.
    watch: Box<[Option<ActorWatch>]>,
    watched: usize,
    wants: HookWants,
    beat_every: u64,
    /// The hash of the bound chunk's bytes, for `tables_hash`.
    chunk: u64,
    /// Every applied command so far, chained.
    commands_hash: u64,
    /// Every hook's end so far, chained.
    ends_hash: u64,
    commands_applied: u64,
}

impl ScriptWorld {
    pub(crate) fn new(actors: usize) -> Self {
        Self {
            frame: HookFrame::new(actors),
            events: [None; MAX_HOOK_EVENTS],
            events_len: 0,
            events_dropped: 0,
            watch: vec![None; actors].into_boxed_slice(),
            watched: 0,
            wants: HookWants::default(),
            beat_every: 0,
            chunk: 0,
            commands_hash: 0,
            ends_hash: 0,
            commands_applied: 0,
        }
    }

    /// Says which events to gather, and which chunk's answering: its hash goes in `tables_hash`, so a
    /// replay can tell a changed script from a broken simulation.
    pub fn bind(&mut self, wants: HookWants, chunk: u64, names: Vec<Name>) {
        self.wants = wants;
        self.chunk = chunk;
        self.frame.set_names(names);
    }

    /// Sends [`HookEvent::Beat`] every this many ticks, or never at 0. It's what a replay's scenario chose,
    /// so `tables_hash` holds it.
    pub const fn set_beat_every(&mut self, ticks: u64) {
        self.beat_every = ticks;
    }

    /// This step's events, in the order they're to be answered: each actor's in ID order, then hits as
    /// they applied, then a beat.
    pub fn events(&self) -> impl ExactSizeIterator<Item = HookEvent> + '_ {
        self.events[..self.events_len].iter().map(|event| event.expect("written up to the length"))
    }

    /// What hooks read and write.
    pub const fn frame_mut(&mut self) -> &mut HookFrame {
        &mut self.frame
    }

    /// The numbers that last.
    #[must_use]
    pub const fn board(&self) -> &[f64; BOARD_SLOTS] {
        &self.frame.board
    }

    /// Commands applied so far.
    #[must_use]
    pub const fn commands_applied(&self) -> u64 {
        self.commands_applied
    }

    /// Events no hook could answer, for want of room.
    #[must_use]
    pub const fn events_dropped(&self) -> u64 {
        self.events_dropped
    }

    /// Chains how a hook ended, for the event at this place in the step's list.
    pub fn record_end(&mut self, event: usize, ending: HookEnding) {
        let mut hash = StateHasher::new();
        hash.u64(self.ends_hash);
        hash.u64(self.frame.tick);
        hash.u32(event as u32);
        hash.u8(ending as u8);
        self.ends_hash = hash.finish();
    }

    /// What this adds to the checksum, alone: what a test reads to see that scripts are what moved.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hash = StateHasher::new();
        self.write_canonical(&mut hash);
        hash.finish()
    }

    /// Writes what lasts: the board by its bits, the chains, and what's being watched. A step's events
    /// and queue are gone by the step's end.
    pub(crate) fn write_canonical(&self, hash: &mut StateHasher) {
        for &value in &self.frame.board {
            hash.u64(if value.is_nan() { f64::NAN.to_bits() } else { value.to_bits() });
        }
        hash.u64(self.commands_hash);
        hash.u64(self.ends_hash);
        hash.u64(self.events_dropped);
        hash.u32(self.watched as u32);
        for watch in self.watch[..self.watched].iter().flatten() {
            hash.u32(watch.id.raw());
            hash.u32(watch.started);
            hash.u8(watch.phase);
            hash.u8(u8::from(watch.defeated));
        }
    }

    /// Writes what `tables_hash` holds of scripts.
    pub(crate) fn write_tables(&self, hash: &mut StateHasher) {
        hash.u64(self.chunk);
        hash.u64(self.beat_every);
    }

    fn push_event(&mut self, event: HookEvent) {
        if let Some(slot) = self.events.get_mut(self.events_len) {
            *slot = Some(event);
            self.events_len += 1;
        } else {
            self.events_dropped += 1;
        }
    }
}

type WatchedActor<'a> = (&'a Fighter, &'a MoveRunner, &'a ActorTransform, &'a SoftLock, &'a Brain);

/// Notes what this step did, against what each actor last showed. Runs whether or not a hook wants it,
/// so a reload that binds a new hook answers from then on, and never the past.
pub(crate) fn gather_hook_events(
    mut scripts: ResMut<'_, ScriptWorld>,
    table: Res<'_, ActorTable>,
    clock: Res<'_, WorldClock>,
    library: Res<'_, MoveLibrary>,
    applied: Res<'_, AppliedHits>,
    actors: Query<'_, '_, WatchedActor<'_>>,
) {
    let scripts = &mut *scripts;
    scripts.events_len = 0;
    let wants = scripts.wants;
    let was = std::mem::take(&mut scripts.watched);
    let mut read = 0;
    let mut facts = 0;
    for (id, entity) in table.iter() {
        let Ok((fighter, runner, transform, lock, brain)) = actors.get(entity) else { continue };
        // Those gone since sit between; a new ID is past them all, as IDs only grow.
        while read < was && scripts.watch[read].is_some_and(|watch| watch.id < id) {
            read += 1;
        }
        let seen = (read < was).then(|| scripts.watch[read]).flatten().filter(|watch| watch.id == id);
        let mut watch = seen.unwrap_or(ActorWatch { id, started: 0, phase: 0, defeated: false });

        let (health, max) = (fighter.health(), fighter.max_health());
        if let Some(phase) = phase_of(health, max)
            && phase > watch.phase
        {
            watch.phase = phase;
            if wants.phases {
                scripts.push_event(HookEvent::Phase { actor: id, phase, health });
            }
        }
        if runner.started != watch.started {
            watch.started = runner.started;
            if wants.moves
                && let Some(name) = runner.running_name(&library)
            {
                scripts.push_event(HookEvent::MoveStarted { actor: id, name });
            }
        }
        if fighter.is_defeated() && !watch.defeated {
            watch.defeated = true;
            if wants.defeats {
                scripts.push_event(HookEvent::Defeated { actor: id });
            }
        }
        // `watched` was taken: what's written here is the new list, and never past what's still to read.
        scripts.watch[scripts.watched] = Some(watch);
        scripts.watched += 1;

        scripts.frame.actors[facts] = Some(ActorFacts {
            id,
            health,
            max_health: max,
            defeated: fighter.is_defeated(),
            faction: fighter.faction().number(),
            position: transform.position,
            soft_lock: lock.target(),
            brain_target: brain.target(),
        });
        facts += 1;
    }
    scripts.frame.actors_len = facts;
    if wants.hits {
        for hit in &applied.0 {
            scripts.push_event(HookEvent::Hit {
                attacker: hit.event.attacker,
                target: hit.event.target,
                damage: hit.attack.damage,
            });
        }
    }
    let tick = clock.0.ticks();
    scripts.frame.tick = tick;
    if scripts.beat_every > 0 && tick.is_multiple_of(scripts.beat_every) {
        scripts.push_event(HookEvent::Beat { tick });
    }
}

/// The one point a hook's commands take effect: in order, each hashed by its bits, whether or not it
/// came to anything, so a command at a gone actor can't pass unseen.
pub(crate) fn apply_script_commands(
    mut scripts: ResMut<'_, ScriptWorld>,
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    mut slow_motion: ResMut<'_, SlowMotion>,
    mut rng: ResMut<'_, SimRng>,
    mut actors: Query<'_, '_, (&mut MoveRunner, &mut Brain, &mut SoftLock, &EntityClock, &Fighter)>,
) {
    let scripts = &mut *scripts;
    let queued = std::mem::take(&mut scripts.frame.queued);
    for index in 0..queued {
        let command = scripts.frame.queue[index];
        let mut hash = StateHasher::new();
        hash.u64(scripts.commands_hash);
        hash.u64(scripts.frame.tick);
        match command {
            ScriptCommand::None => {}
            ScriptCommand::SlowMotion { per_mille, steps } => {
                // Refused here rather than in the binding, so a bad number can't reach time's rule: a
                // rate that divides time evenly, and only ever slower.
                if (1..=1000).contains(&per_mille) && 1_000_000 % per_mille == 0 {
                    slow_motion.request(per_mille, steps);
                }
                hash.u8(1);
                hash.u32(per_mille);
                hash.u16(steps);
            }
            ScriptCommand::Mark(value) => {
                hash.u8(2);
                hash.u64(if value.is_nan() { f64::NAN.to_bits() } else { value.to_bits() });
            }
            ScriptCommand::ForceMove { actor, name } => {
                if let Some(entity) = table.entity(actor)
                    && let Ok((mut runner, .., fighter)) = actors.get_mut(entity)
                    && !fighter.is_defeated()
                {
                    runner.force(name);
                }
                hash.u8(3);
                hash.u32(actor.raw());
                hash.u64(name.as_u64());
            }
            ScriptCommand::GiveBrain { actor } => {
                if let Some(entity) = table.entity(actor)
                    && let Ok((runner, mut brain, _, clock, fighter)) = actors.get_mut(entity)
                    && !brain.is_on()
                    && !fighter.is_defeated()
                    && let Some(def) =
                        runner.set().and_then(|set| library.current(set)).and_then(|set| set.brain())
                {
                    *brain = Brain::fighting(def, clock.0.now(), &mut rng.0);
                }
                hash.u8(4);
                hash.u32(actor.raw());
            }
            ScriptCommand::GiveSoftLock { actor } => {
                if let Some(entity) = table.entity(actor)
                    && let Ok((_, _, mut lock, ..)) = actors.get_mut(entity)
                    && !lock.is_on()
                {
                    *lock = SoftLock::ON;
                }
                hash.u8(5);
                hash.u32(actor.raw());
            }
        }
        scripts.commands_hash = hash.finish();
    }
    scripts.commands_applied += queued as u64;
}

/// How many actors a world's scripts have to see: the pool, whoever's in it.
pub(crate) fn capacity_of(requests: &ActorRequests) -> usize {
    requests.capacity() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn id(raw: u32) -> ActorId {
        ActorId::from_raw(raw).unwrap()
    }

    #[test]
    fn phases_cover_all_of_u32s_health() {
        assert_eq!(phase_of(u32::MAX, u32::MAX), Some(0));
        assert_eq!(phase_of(u32::MAX / 4 * 3, u32::MAX), Some(1));
        assert_eq!(phase_of(0, u32::MAX), Some(4), "the latest quarter, not four");
        assert_eq!(phase_of(0, 0), None);
    }

    fn facts(raw: u32) -> ActorFacts {
        ActorFacts {
            id: id(raw),
            health: raw * 10,
            max_health: 100,
            defeated: false,
            faction: 0,
            position: Vec3::ZERO,
            soft_lock: None,
            brain_target: None,
        }
    }

    #[test]
    fn a_frame_finds_actors_by_id_and_nobody_else() {
        let mut frame = HookFrame::new(4);
        frame.set_actors([facts(2), facts(5), facts(9)]);
        assert_eq!(frame.actor(5).map(|facts| facts.health), Some(50));
        for missing in [0, 1, 3, 10] {
            assert_eq!(frame.actor(missing), None);
        }
    }

    #[test]
    fn a_full_queue_is_a_fault_rather_than_half_a_hook() {
        let mut frame = HookFrame::new(0);
        for _ in 0..QUEUE_LEN {
            frame.push(ScriptCommand::Mark(1.0));
        }
        assert!(!frame.fault);
        frame.push(ScriptCommand::Mark(2.0));
        assert!(frame.fault);
        assert_eq!(frame.queued().len(), QUEUE_LEN);
        frame.truncate_queue(3);
        assert_eq!(frame.queued().len(), 3);
    }

    #[test]
    fn places_in_a_list_of_names_count_from_one() {
        let mut frame = HookFrame::new(0);
        frame.set_names(vec![Name::new("roar"), Name::new("leap")]);
        assert_eq!(frame.place_of(Name::new("leap")), 2);
        assert_eq!(frame.place_of(Name::new("other")), 0);
        assert_eq!(frame.name(1), Some(Name::new("roar")));
        assert_eq!(frame.name(0), None);
        assert_eq!(frame.name(3), None);
    }

    /// A digest that doesn't see a piece of what lasts is how the first attack's digest failed.
    #[test]
    fn every_piece_of_what_lasts_moves_the_digest() {
        let fresh = || ScriptWorld::new(2);
        let base = fresh().digest();

        let mut board = fresh();
        board.frame.board[63] = 1.0;
        assert_ne!(board.digest(), base, "the board's last slot");

        let mut negative = fresh();
        negative.frame.board[0] = -0.0;
        assert_ne!(negative.digest(), base, "a number goes by its bits");

        let mut ends = fresh();
        ends.record_end(0, HookEnding::Returned);
        let returned = ends.digest();
        assert_ne!(returned, base, "a hook's end");
        let mut ended = fresh();
        ended.record_end(0, HookEnding::Failed);
        assert_ne!(ended.digest(), returned, "how it ended");

        let mut commands = fresh();
        commands.commands_hash = 1;
        assert_ne!(commands.digest(), base, "the commands' chain");

        let mut dropped = fresh();
        dropped.events_dropped = 1;
        assert_ne!(dropped.digest(), base, "an event nobody answered");

        let mut watched = fresh();
        watched.watch[0] = Some(ActorWatch { id: id(1), started: 0, phase: 0, defeated: false });
        watched.watched = 1;
        let one = watched.digest();
        assert_ne!(one, base, "who's watched");
        watched.watch[0] = Some(ActorWatch { id: id(1), started: 0, phase: 1, defeated: false });
        assert_ne!(watched.digest(), one, "a phase already answered");
    }
}
