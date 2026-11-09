//! What a timeline does to the simulation (spec §5.23, `docs/m3-plan.md` §3): its time scale and its
//! gameplay events, and nothing else of it. The rest is presentation, in `sapphire_sequencer`.
//!
//! A timeline is loaded with [`Simulation::add_timeline`](crate::Simulation::add_timeline), between steps,
//! where it allocates and goes in [`tables_hash`](crate::Simulation::tables_hash). From then on nothing of
//! a run allocates: [`advance_timeline`] walks its beats by index.
//!
//! # A skip can't be got wrong
//!
//! **Nothing here knows a skip exists,** and that's the design. There's no call that jumps a run, so
//! there's no state a jump could leave different from the state watching reached.
//!
//! - A cutscene ([`TimelineClock::Real`](sapphire_timeline::TimelineClock)) holds the simulation at a
//!   step's edge, and its run is no steps long: its gameplay events all fire on the first step after it.
//!   However long anyone watched, and wherever they skipped, the simulation saw the same single
//!   [`Simulation::play_timeline`](crate::Simulation::play_timeline) between the same two steps.
//! - A cinematic ([`TimelineClock::Steps`](sapphire_timeline::TimelineClock)) runs the simulation beneath
//!   it, with input held ([`TimelineRun::holds_input`]), so what anyone pressed can't reach it. To skip one
//!   is to run the steps it has left, in one frame: those are exactly the steps watching takes, each of
//!   the jumped time scale beats and gameplay events firing on its own step, in order.
//!   [`steps_held`](crate::Simulation::timeline_steps_held) says how many.
//!
//! The worst an app can do by forgetting the steps is have the cinematic go on playing out underneath
//! what it's showing, late; it can't reach a different state. Jumping instead (applying what a skip passed
//! over at once) would leave tick counts, the world clock's remainder and everything that ran under the
//! slowdown short of what watching reached, and the fight that follows on another checksum.
//!
//! # What's recorded
//!
//! A replay records the [`TimelineCommand`]s, and pins what they refer to through `tables_hash`. A run's
//! beats follow from those, on the steps they fell on, rather than each being recorded as it fired: that
//! would be recording what the simulation did, which a replay exists to check.
//!
//! # A move's cinematic
//!
//! A move can name a cinematic (M3 Q3b): a character's ultimate, played beneath the move. The fight loads
//! it by that name, [`Simulation::add_cinematic`](crate::Simulation::add_cinematic), before it loads the
//! sets that name it, and a scenario refuses a set naming one it hasn't loaded
//! ([`Simulation::check_cinematics`](crate::Simulation::check_cinematics)). Then [`start_cinematics`],
//! in [`CombatSet::Cinematics`](crate::CombatSet::Cinematics), after the cancels that start moves and
//! before anything reads one, starts the run on the move's first tick, with its owner. Nothing is
//! recorded for it: the move's start follows from the replay's input, and the run from the move.
//!
//! Its beats fire from the next step's [`CombatSet::Timeline`](crate::CombatSet::Timeline), as a run
//! started between steps does. A time scale beat slows the world clock and counter-scales its owner's,
//! as a perfect dodge does, so the others are held while the move plays through at its own speed.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_time::ScaleSource;
use sapphire_timeline::{Beat, BeatKind, TimelineError, TimelineSimulation};
use xxhash_rust::xxh3::xxh3_64_with_seed;

use crate::actors::{ActorId, ActorTable};
use crate::checksum::StateHasher;
use crate::moves::{MoveLibrary, MoveRunner};
use crate::resolve::counter_scale;

/// A loaded timeline's simulation half, by the order it was added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimelineId(u32);

impl TimelineId {
    /// The ID a run's `add_timeline`s gave it, from a replay's file or a script.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// What a replay holds of a timeline, between two steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineCommand {
    /// [`Simulation::play_timeline`](crate::Simulation::play_timeline).
    Play(TimelineId),
    /// Someone skipped, this far in. It changes nothing in the simulation, whose [`TimelineRun`] just runs
    /// out; it's here so a replay shown beside a renderer can show the skip.
    Skip { at_ms: u32 },
}

/// Why a timeline didn't start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TimelinePlayError {
    #[error("no timeline {} is loaded", .0.raw())]
    Unknown(TimelineId),
    #[error("timeline {} is still playing", .0.raw())]
    Busy(TimelineId),
}

/// Why a fight can't play a move's cinematic.
#[derive(Debug, thiserror::Error)]
pub enum CinematicError {
    #[error(transparent)]
    Timeline(#[from] TimelineError),
    #[error("a cinematic named `{0}` is loaded already")]
    Taken(String),
    #[error(
        "cinematic `{0}` holds the simulation, but a move's cinematic plays beneath its move: give it the \
         `Steps` clock"
    )]
    Held(String),
    #[error(
        "move `{mv}` of move set {set} plays the cinematic {name}, which this fight hasn't loaded: load it \
         with `add_cinematic` before the set"
    )]
    Missing { set: u32, mv: String, name: Name },
}

/// Something a timeline told gameplay, on the step it fell on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineEvent {
    /// The hash of its name: `sapphire_core::Name::new(name).as_u64()`.
    pub event: u64,
    pub value: i32,
}

/// Where a playing timeline has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineProgress {
    pub id: TimelineId,
    /// Steps it has taken.
    pub tick: u32,
    /// Its length in steps: 0 for one that holds the simulation.
    pub ticks: u32,
    /// The actor whose move started it, for a move's cinematic: the one its time scale leaves alone.
    pub owner: Option<ActorId>,
}

/// A loaded timeline: its simulation half, its digest and, for a move's cinematic, its name.
#[derive(Debug)]
struct Loaded {
    timeline: TimelineSimulation,
    digest: u64,
    name: Option<Name>,
}

/// The loaded timelines. Nothing here changes during a step.
#[derive(Resource, Debug, Default)]
pub struct TimelineLibrary {
    timelines: Vec<Loaded>,
}

impl TimelineLibrary {
    pub(crate) fn add(
        &mut self,
        timeline: TimelineSimulation,
        name: Option<Name>,
    ) -> Result<TimelineId, TimelineError> {
        timeline.validate()?;
        let id = TimelineId(self.timelines.len() as u32);
        let digest = timeline.digest();
        self.timelines.push(Loaded { timeline, digest, name });
        Ok(id)
    }

    fn get(&self, id: TimelineId) -> Option<&TimelineSimulation> {
        self.timelines.get(id.0 as usize).map(|loaded| &loaded.timeline)
    }

    /// The cinematic loaded under `name`, in no time and with no heap: a step looks one up.
    #[must_use]
    pub fn find(&self, name: Name) -> Option<TimelineId> {
        let at = self.timelines.iter().position(|loaded| loaded.name == Some(name))?;
        Some(TimelineId(at as u32))
    }

    /// The digest of the timeline `id`, as `tables_hash` takes it: what presentation finds its cooked copy by.
    #[must_use]
    pub fn digest(&self, id: TimelineId) -> Option<u64> {
        self.timelines.get(id.0 as usize).map(|loaded| loaded.digest)
    }

    fn beats(&self, id: TimelineId) -> &[Beat] {
        self.get(id).map_or(&[], |timeline| &timeline.beats)
    }

    /// Goes in `tables_hash`, and only with something to say, so a run on no timelines hashes as it did.
    pub(crate) fn write_digests(&self, hasher: &mut StateHasher) {
        if self.timelines.is_empty() {
            return;
        }
        hasher.u32(self.timelines.len() as u32);
        for loaded in &self.timelines {
            hasher.u64(loaded.digest);
            // What a move's cinematic finds it by is what the fight plays: only a named one says it, so a
            // run on unnamed timelines hashes as it did.
            if let Some(name) = loaded.name {
                hasher.u64(name.as_u64());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Playing {
    id: TimelineId,
    tick: u32,
    ticks: u32,
    /// The next beat.
    next: u32,
    /// Whose move started it, if a move did: its time scale counter-scales their clock.
    owner: Option<ActorId>,
}

/// The timeline a simulation is playing, and what its timelines have done.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineRun {
    playing: Option<Playing>,
    /// The beats the latest step fired, as a range of that timeline's.
    latest: Option<(TimelineId, u32, u32)>,
    played: u64,
    events_fired: u64,
    /// Every event fired, in order: if a run dropped or reordered one, the checksum says.
    events_hash: u64,
    /// Time scale beats the world clock's full stack turned away.
    scales_refused: u64,
    /// Moves' cinematics that couldn't start: another was playing, or the fight never loaded it.
    cinematics_refused: u64,
}

impl TimelineRun {
    /// Whether the player's input is being held: all through a timeline that runs the simulation beneath it.
    #[must_use]
    pub fn holds_input(&self) -> bool {
        self.steps_held() > 0
    }

    /// The steps a [`TimelineClock::Steps`](sapphire_timeline::TimelineClock) run has left to take, which a
    /// skip takes at once.
    #[must_use]
    pub fn steps_held(&self) -> u32 {
        self.playing.map_or(0, |playing| playing.ticks.saturating_sub(playing.tick))
    }

    #[must_use]
    pub fn progress(&self) -> Option<TimelineProgress> {
        self.playing.map(|playing| TimelineProgress {
            id: playing.id,
            tick: playing.tick,
            ticks: playing.ticks,
            owner: playing.owner,
        })
    }

    /// How many timelines have started.
    #[must_use]
    pub fn played(&self) -> u64 {
        self.played
    }

    /// How many gameplay events have fired.
    #[must_use]
    pub fn events_fired(&self) -> u64 {
        self.events_fired
    }

    /// Time scale beats the world clock's full stack turned away.
    #[must_use]
    pub fn scales_refused(&self) -> u64 {
        self.scales_refused
    }

    /// Moves' cinematics that couldn't start: another timeline was playing, or the fight never loaded it.
    #[must_use]
    pub fn cinematics_refused(&self) -> u64 {
        self.cinematics_refused
    }

    /// Who the playing timeline's move belongs to, if a move started it.
    #[must_use]
    pub fn owner(&self) -> Option<ActorId> {
        self.playing.and_then(|playing| playing.owner)
    }

    /// What the current step told gameplay, for a system of a later [`CombatSet`](crate::CombatSet) to read,
    /// or what the latest one did, between steps.
    pub fn events<'a>(&self, library: &'a TimelineLibrary) -> impl Iterator<Item = TimelineEvent> + 'a {
        let beats = self
            .latest
            .map_or::<&[Beat], _>(&[], |(id, from, to)| &library.beats(id)[from as usize..to as usize]);
        beats.iter().filter_map(|beat| match beat.what {
            BeatKind::Event { event, value } => Some(TimelineEvent { event, value }),
            BeatKind::Scale { .. } => None,
        })
    }

    /// Starts `id`, for `owner`'s move if a move started it.
    pub(crate) fn start(
        &mut self,
        library: &TimelineLibrary,
        id: TimelineId,
        owner: Option<ActorId>,
    ) -> Result<(), TimelinePlayError> {
        if let Some(playing) = self.playing {
            return Err(TimelinePlayError::Busy(playing.id));
        }
        let timeline = library.get(id).ok_or(TimelinePlayError::Unknown(id))?;
        self.playing = Some(Playing { id, tick: 0, ticks: timeline.ticks, next: 0, owner });
        self.played += 1;
        Ok(())
    }

    /// Writes nothing for a simulation that has never played one, so its checksums stay what they were.
    /// `latest` stays out: every step starts by replacing it, so no state outlives a step through it.
    pub(crate) fn write_canonical(&self, hasher: &mut StateHasher) {
        if self.played == 0 && self.cinematics_refused == 0 {
            return;
        }
        hasher.u64(self.played);
        hasher.u64(self.events_fired);
        hasher.u64(self.events_hash);
        hasher.u64(self.scales_refused);
        match self.playing {
            Some(playing) => {
                hasher.u8(1);
                hasher.u32(playing.id.0);
                hasher.u32(playing.tick);
                hasher.u32(playing.ticks);
                hasher.u32(playing.next);
                // Only a move's run has one, so a run between steps hashes as it did.
                if let Some(owner) = playing.owner {
                    hasher.u8(2);
                    hasher.u32(owner.raw());
                }
            }
            None => hasher.u8(0),
        }
        if self.cinematics_refused > 0 {
            hasher.u8(3);
            hasher.u64(self.cinematics_refused);
        }
    }
}

/// [`CombatSet::Cinematics`](crate::CombatSet::Cinematics): starts the cinematic of each move the cancels
/// just started, on its first tick, before [`CombatSet::Moves`](crate::CombatSet::Moves) reads any of it.
/// Actors go in [`ActorTable::iter`]'s order, so if two start one on a step, the first in it plays and
/// the other is counted. See the [module docs](self#a-moves-cinematic).
pub(crate) fn start_cinematics(
    table: Res<'_, ActorTable>,
    moves: Res<'_, MoveLibrary>,
    library: Res<'_, TimelineLibrary>,
    mut run: ResMut<'_, TimelineRun>,
    runners: Query<'_, '_, &MoveRunner>,
) {
    for (id, entity) in table.iter() {
        let Ok(runner) = runners.get(entity) else { continue };
        let Some(running) = runner.running.filter(|running| running.fresh) else { continue };
        let Some(name) = moves.get(running.at).and_then(sapphire_moves::Move::cinematic) else { continue };
        let started = library
            .find(name)
            .ok_or(())
            .and_then(|timeline| run.start(&library, timeline, Some(id)).map_err(|_| ()));
        if started.is_err() {
            run.cinematics_refused += 1;
        }
    }
}

/// Fires the beats of this step, first in the step's combat so that everything after can read its events.
/// A time scale takes effect from the next step's clocks, as a [`SlowMotion`](crate::SlowMotion)'s does.
pub(crate) fn advance_timeline(
    library: Res<'_, TimelineLibrary>,
    mut run: ResMut<'_, TimelineRun>,
    mut world_clock: ResMut<'_, WorldClock>,
    table: Res<'_, ActorTable>,
    mut clocks: Query<'_, '_, &mut EntityClock>,
) {
    run.latest = None;
    let Some(mut playing) = run.playing else { return };
    // A move's owner plays through its own cinematic's time scale, as a perfect dodge's dodger does.
    let mut owner_clock =
        playing.owner.and_then(|owner| table.entity(owner)).and_then(|entity| clocks.get_mut(entity).ok());
    let beats = library.beats(playing.id);
    let from = playing.next;
    while let Some(beat) = beats.get(playing.next as usize).filter(|beat| beat.tick == playing.tick) {
        match beat.what {
            BeatKind::Scale { per_mille } => {
                world_clock.0.remove_source(ScaleSource::Cinematic);
                // Refused, it's counted, and the same on every run: the stack's contents are in the checksum.
                let refused = world_clock.0.push_scale(ScaleSource::Cinematic, per_mille).is_err();
                run.scales_refused += u64::from(refused);
                if let Some(clock) = &mut owner_clock {
                    clock.0.remove_source(ScaleSource::Cinematic);
                    if !refused {
                        run.scales_refused += u64::from(
                            clock.0.push_scale(ScaleSource::Cinematic, counter_scale(per_mille)).is_err(),
                        );
                    }
                }
            }
            BeatKind::Event { event, value } => {
                run.events_fired += 1;
                let mut fired = [0; 12];
                fired[..8].copy_from_slice(&event.to_le_bytes());
                fired[8..].copy_from_slice(&value.to_le_bytes());
                run.events_hash = xxh3_64_with_seed(&fired, run.events_hash);
            }
        }
        playing.next += 1;
    }
    run.latest = Some((playing.id, from, playing.next));
    playing.tick += 1;
    if playing.tick >= playing.ticks {
        world_clock.0.remove_source(ScaleSource::Cinematic);
        if let Some(clock) = &mut owner_clock {
            clock.0.remove_source(ScaleSource::Cinematic);
        }
        run.playing = None;
    } else {
        run.playing = Some(playing);
    }
}

#[cfg(test)]
mod tests {
    use sapphire_core::Name;
    use sapphire_time::SimTime;
    use sapphire_timeline::{Beat, BeatKind, TimelineSimulation};

    use super::*;
    use crate::actions::{ActorActions, Buttons, TickActions};
    use crate::actors::{ActorId, ActorSpawn};
    use crate::simulation::{CombatSet, SimConfig, Simulation};

    fn scale(tick: u32, per_mille: u32) -> Beat {
        Beat { tick, what: BeatKind::Scale { per_mille } }
    }

    fn event(tick: u32, name: &str, value: i32) -> Beat {
        Beat { tick, what: BeatKind::Event { event: Name::new(name).as_u64(), value } }
    }

    fn timeline(ticks: u32, beats: Vec<Beat>) -> TimelineSimulation {
        TimelineSimulation { ticks, beats }
    }

    fn world_now(sim: &Simulation) -> SimTime {
        sim.world().resource::<WorldClock>().0.now()
    }

    /// The brief's measure: a time scale's effect on the world clock, in the checksum.
    #[test]
    fn a_time_scale_runs_the_world_clock_at_its_speed_for_its_steps_and_is_in_the_checksum() {
        let mut slowed = Simulation::new(SimConfig::default());
        let id = slowed.add_timeline(timeline(10, vec![scale(0, 500)])).unwrap();
        let mut plain = Simulation::new(SimConfig::default());
        plain.add_timeline(timeline(10, Vec::new())).unwrap();
        slowed.play_timeline(id).unwrap();
        plain.play_timeline(id).unwrap();

        // The beat lands in step 0's combat, past that step's clocks, so step 1 is the first it slows.
        slowed.step(&TickActions::NEUTRAL);
        plain.step(&TickActions::NEUTRAL);
        assert_eq!(world_now(&slowed), world_now(&plain));
        let start = world_now(&slowed);
        for _ in 1..10 {
            slowed.step(&TickActions::NEUTRAL);
            plain.step(&TickActions::NEUTRAL);
        }
        assert_eq!(
            world_now(&slowed).since(start).as_subticks() * 2,
            world_now(&plain).since(start).as_subticks(),
            "nine steps at half speed"
        );
        assert_ne!(slowed.checksum(), plain.checksum());

        // Over at its end, on its last step's edge.
        let world = slowed.world().resource::<WorldClock>();
        assert_eq!(world.0.scales().count(), 0);
        assert!(slowed.timeline().is_none());
        let moved = (world_now(&slowed), world_now(&plain));
        slowed.step(&TickActions::NEUTRAL);
        plain.step(&TickActions::NEUTRAL);
        assert_eq!(world_now(&slowed).since(moved.0), world_now(&plain).since(moved.1));
    }

    #[test]
    fn a_scale_of_nothing_cant_stall_a_timeline() {
        let mut sim = Simulation::new(SimConfig::default());
        let id = sim.add_timeline(timeline(5, vec![scale(0, 0)])).unwrap();
        sim.play_timeline(id).unwrap();
        for _ in 0..5 {
            assert!(sim.timeline().is_some());
            sim.step(&TickActions::NEUTRAL);
        }
        assert!(sim.timeline().is_none(), "steps count, whatever the world clock does");
        assert_eq!(sim.world().resource::<WorldClock>().0.scales().count(), 0);
    }

    #[test]
    fn a_full_stack_is_counted_and_deterministic() {
        let run = || {
            let mut sim = Simulation::new(SimConfig::default());
            let id = sim.add_timeline(timeline(2, vec![scale(0, 500)])).unwrap();
            for source in 0..8 {
                sim.world_clock_mut().0.push_scale(ScaleSource::Debug, 900 + source).unwrap();
            }
            sim.play_timeline(id).unwrap();
            sim.step(&TickActions::NEUTRAL);
            (sim.stats().timeline_scales_refused, sim.checksum())
        };
        assert_eq!(run().0, 1);
        assert_eq!(run(), run());
    }

    #[test]
    fn events_fire_on_their_step_in_order_and_are_gone_on_the_next() {
        let mut sim = Simulation::new(SimConfig::default());
        let beats = vec![event(1, "first", 1), event(1, "second", 2), scale(1, 500), event(3, "third", -3)];
        let id = sim.add_timeline(timeline(5, beats)).unwrap();
        sim.play_timeline(id).unwrap();
        let mut seen = Vec::new();
        for _ in 0..6 {
            sim.step(&TickActions::NEUTRAL);
            seen.push(sim.timeline_events().map(|fired| fired.value).collect::<Vec<i32>>());
        }
        assert_eq!(seen, [vec![], vec![1, 2], vec![], vec![-3], vec![], vec![]]);
        assert_eq!(sim.stats().timeline_events, 3);
    }

    /// A system later in the step can read what fired, which is how gameplay is told.
    #[test]
    fn a_system_reads_this_steps_events_in_the_same_step() {
        #[derive(Resource, Default)]
        struct Told(Vec<(u64, i32)>);
        fn listen(library: Res<'_, TimelineLibrary>, run: Res<'_, TimelineRun>, mut told: ResMut<'_, Told>) {
            for fired in run.events(&library) {
                told.0.push((run.played(), fired.value));
            }
        }
        let mut sim = Simulation::new(SimConfig::default());
        sim.add_combat_system(CombatSet::Intents, listen);
        sim.ecs_mut().world_mut().insert_resource(Told::default());
        let id = sim.add_timeline(timeline(0, vec![event(0, "wake", 7)])).unwrap();
        sim.play_timeline(id).unwrap();
        sim.step(&TickActions::NEUTRAL);
        assert_eq!(sim.world().resource::<Told>().0, [(1, 7)]);
    }

    #[test]
    fn a_timeline_of_no_steps_fires_all_of_it_on_the_next_step_and_holds_no_input() {
        let mut sim = Simulation::new(SimConfig::default());
        let id = sim.add_timeline(timeline(0, vec![event(0, "wake", 1), event(0, "door", 2)])).unwrap();
        sim.play_timeline(id).unwrap();
        assert_eq!(sim.timeline_steps_held(), 0);
        sim.step(&TickActions::NEUTRAL);
        assert_eq!(sim.timeline_events().count(), 2);
        assert!(sim.timeline().is_none());
    }

    #[test]
    fn input_is_held_all_through_a_run_and_let_go_on_the_step_after() {
        let actor = |sim: &Simulation, id: ActorId| sim.actor(id).unwrap().controls.current;
        let mut sim = Simulation::new(SimConfig::default());
        let player = sim.spawn(ActorSpawn::default()).unwrap();
        let id = sim.add_timeline(timeline(3, Vec::new())).unwrap();
        let mut pressed = TickActions::NEUTRAL;
        let held = ActorActions { buttons: Buttons::LIGHT, ..ActorActions::NEUTRAL };
        pressed.set(player, held).unwrap();

        sim.step(&pressed);
        assert_eq!(actor(&sim, player), held);
        sim.play_timeline(id).unwrap();
        assert_eq!(sim.timeline_steps_held(), 3);
        for left in (0..3).rev() {
            sim.step(&pressed);
            assert_eq!(actor(&sim, player), ActorActions::NEUTRAL);
            assert_eq!(sim.timeline_steps_held(), left);
        }
        sim.step(&pressed);
        assert_eq!(actor(&sim, player), held);
    }

    #[test]
    fn a_play_is_refused_when_busy_or_unknown_and_changes_nothing() {
        let mut sim = Simulation::new(SimConfig::default());
        let id = sim.add_timeline(timeline(3, Vec::new())).unwrap();
        let other = TimelineId::from_raw(9);
        assert_eq!(sim.play_timeline(other), Err(TimelinePlayError::Unknown(other)));
        sim.play_timeline(id).unwrap();
        let checksum = sim.checksum();
        assert_eq!(sim.play_timeline(id), Err(TimelinePlayError::Busy(id)));
        assert_eq!(sim.checksum(), checksum);
    }

    #[test]
    fn a_timeline_that_would_break_a_run_is_refused_at_the_door() {
        let mut sim = Simulation::new(SimConfig::default());
        assert!(sim.add_timeline(timeline(3, vec![scale(5, 500)])).is_err());
        assert!(sim.add_timeline(timeline(3, vec![scale(2, 500), scale(1, 500)])).is_err());
        assert!(sim.add_timeline(timeline(0, vec![scale(0, 500)])).is_err());
        assert!(sim.add_timeline(timeline(3, vec![scale(0, u32::MAX)])).is_err());
    }

    /// A simulation that has loaded no timeline and played none hashes as it did before they existed: what
    /// keeps every committed replay's checksums and hash where they were.
    #[test]
    fn timelines_move_a_hash_only_once_theyre_there() {
        let mut sim = Simulation::new(SimConfig::default());
        let (tables, state) = (sim.tables_hash(), sim.checksum());
        let id = sim.add_timeline(timeline(3, Vec::new())).unwrap();
        assert_ne!(sim.tables_hash(), tables, "a replay pins what it could play");
        assert_eq!(sim.checksum(), state, "loading one isn't state");
        sim.play_timeline(id).unwrap();
        assert_ne!(sim.checksum(), state, "playing one is");
    }

    /// Each field of a run, flipped: one left out of the checksum is a replay bug waiting.
    #[test]
    fn every_field_of_a_run_is_in_the_checksum() {
        let mut sim = Simulation::new(SimConfig::default());
        let id = sim.add_timeline(timeline(9, vec![event(0, "wake", 1), event(4, "door", 2)])).unwrap();
        sim.play_timeline(id).unwrap();
        sim.step(&TickActions::NEUTRAL);
        let good = sim.checksum();
        let flips: &[fn(&mut TimelineRun)] = &[
            |run| run.played += 1,
            |run| run.events_fired += 1,
            |run| run.events_hash ^= 1,
            |run| run.scales_refused += 1,
            |run| run.playing.as_mut().unwrap().id.0 += 1,
            |run| run.playing.as_mut().unwrap().tick += 1,
            |run| run.playing.as_mut().unwrap().ticks += 1,
            |run| run.playing.as_mut().unwrap().next += 1,
            |run| run.playing = None,
        ];
        for flip in flips {
            let before = *sim.world().resource::<TimelineRun>();
            flip(&mut sim.ecs_mut().world_mut().resource_mut::<TimelineRun>());
            assert_ne!(sim.checksum(), good);
            *sim.ecs_mut().world_mut().resource_mut::<TimelineRun>() = before;
        }
        assert_eq!(sim.checksum(), good);
    }
}
