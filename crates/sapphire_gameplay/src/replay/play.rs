//! Running a replay ([`Player`]) and making one ([`Recording`]), a step at a time, so an app can step
//! either beside its renderer: a headless runner and an app go through this same code.

use sapphire_moves::MoveSet;

use super::checksums::Trail;
use super::file::{Cursor, MAX_TICKS, Recorder, Replay, ReplayError};
use super::scenario::{Scenario, ScenarioError, find_scenario};
use super::script::{ActionSource, Script};
use super::tuning::LiveMoves;
use crate::actions::TickActions;
use crate::actors::ActorId;
use crate::moves::{MoveSetId, Tuned};
use crate::simulation::{SimConfig, Simulation};
use crate::stats::SimStats;
use crate::targeting::TargetingTuning;
use crate::timeline::{TimelineCommand, TimelineId, TimelinePlayError};

/// A replay that can't start, or a recording that can't.
#[derive(Debug, thiserror::Error)]
pub enum PlayError {
    #[error("this replay starts from scenario `{name}`, and this build's are: {known}")]
    UnknownScenario { name: String, known: String },
    #[error("scenario `{name}` can't set up its world: {source}")]
    Setup { name: String, source: ScenarioError },
    #[error("scenario `{name}` can't bind its runtime profiles: {reason}")]
    Configure { name: String, reason: String },
    /// The move data, controller setup or collision world isn't what the replay was recorded against,
    /// so its actions mean something else.
    #[error(
        "the move data, controller setup or collision world changed since this replay was recorded (hash \
         {recorded:016x}, now {current:016x}), \
         so it would run a different fight: re-record it with `cargo xtask replay --update`, then review \
         what moved"
    )]
    MovesChanged { recorded: u64, current: u64 },
    #[error(transparent)]
    File(#[from] ReplayError),
    #[error(transparent)]
    Timeline(#[from] TimelinePlayError),
    #[error("the recording ended at tick {tick} after a live edit; it cannot record more ticks")]
    RecordingEnded { tick: u32 },
    /// A move's cinematic is the move's own steps, and the move has no way out (M3 Q3b).
    #[error("the timeline playing at tick {tick} is a move's cinematic, which can't be skipped")]
    Unskippable { tick: u32 },
}

/// Rule R6, broken: the fixed step allocated during a run (spec §9, gate 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("rule R6: the fixed step allocated {allocations} times during this run, first at tick {first_tick}")]
pub struct StepAllocated {
    /// The first tick that allocated, counting steps from 1.
    pub first_tick: u64,
    pub allocations: u64,
}

/// What a finished run did.
#[derive(Clone, Debug)]
pub struct Played {
    /// Its checksum at every tick.
    pub trail: Trail,
    /// The simulation's counters at its end.
    pub stats: SimStats,
    first_allocating_tick: Option<u64>,
}

impl Played {
    /// Spec §9's gate 4: an error if any step allocated. It sees nothing unless a counting allocator
    /// is installed, so a runner checks `sapphire_core::alloc::is_installed` first.
    pub fn allocation_gate(&self) -> Result<(), StepAllocated> {
        match self.first_allocating_tick {
            Some(first_tick) => Err(StepAllocated { first_tick, allocations: self.stats.allocations }),
            None => Ok(()),
        }
    }
}

/// A simulation with its checksums and its allocation gate, a tick at a time.
struct Tracked {
    sim: Simulation,
    trail: Trail,
    first_allocating_tick: Option<u64>,
}

impl Tracked {
    fn new(sim: Simulation, expected_ticks: u32) -> Self {
        let trail = Trail::new(sim.checksum(), expected_ticks);
        Self { sim, trail, first_allocating_tick: None }
    }

    fn step(&mut self, actions: &TickActions) {
        let before = self.sim.stats().allocations;
        self.sim.step(actions);
        if self.sim.stats().allocations > before && self.first_allocating_tick.is_none() {
            self.first_allocating_tick = Some(self.sim.tick());
        }
        self.trail.push(self.sim.checksum());
    }

    /// A step past what's kept: rule R6 still holds, and no checksum goes in the trail.
    fn step_unkept(&mut self, actions: &TickActions) {
        let before = self.sim.stats().allocations;
        self.sim.step(actions);
        if self.sim.stats().allocations > before && self.first_allocating_tick.is_none() {
            self.first_allocating_tick = Some(self.sim.tick());
        }
    }

    fn finish(self) -> Played {
        Played {
            trail: self.trail,
            stats: self.sim.stats(),
            first_allocating_tick: self.first_allocating_tick,
        }
    }
}

/// Runs a [`Replay`] from its scenario, one tick at a time.
pub struct Player {
    tracked: Tracked,
    replay: Replay,
    cursor: Cursor,
    /// The next of the replay's commands.
    next_command: usize,
    /// Ticks it has played.
    played: u32,
    cast: Vec<ActorId>,
}

impl Player {
    /// Sets up the replay's scenario, found among `scenarios`, and refuses to start if the setup
    /// isn't what it was recorded against. It never plays on regardless: see
    /// [`PlayError::MovesChanged`].
    pub fn new(replay: Replay, scenarios: &[Scenario]) -> Result<Self, PlayError> {
        Self::new_with_setup(replay, scenarios, |_, _, _| Ok(()))
    }

    /// Tries a tick-zero setup for recordings with a configured table hash.
    /// An exact unconfigured match never calls `setup`. Every other replay must match the
    /// configured tables before it can apply commands or take a step.
    pub fn new_with_setup(
        replay: Replay,
        scenarios: &[Scenario],
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, PlayError> {
        Self::tuned_with_setup(replay, scenarios, &LiveMoves::default(), setup)
    }

    /// [`Self::new`], from what `live` starts a fight from: what plays back a fight of a game with its own
    /// numbers, on those numbers.
    pub fn tuned(replay: Replay, scenarios: &[Scenario], live: &LiveMoves) -> Result<Self, PlayError> {
        Self::tuned_with_setup(replay, scenarios, live, |_, _, _| Ok(()))
    }

    /// [`Self::new_with_setup`] from what `live` starts a fight from.
    pub fn tuned_with_setup(
        replay: Replay,
        scenarios: &[Scenario],
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, PlayError> {
        let (scenario, mut sim, cast) = Self::setup(&replay, scenarios, live)?;
        let recorded = replay.tables_hash();
        if sim.tables_hash() != recorded {
            setup(scenario, &mut sim, &cast)
                .map_err(|reason| PlayError::Configure { name: scenario.name.to_owned(), reason })?;
            let current = sim.tables_hash();
            if current != recorded {
                return Err(PlayError::MovesChanged { recorded, current });
            }
        }
        Ok(Self::started(sim, cast, replay))
    }

    /// [`Self::new`], going ahead whatever the tables now are: a recording's actions as a test's "what if",
    /// with a script on them, say. No suite checks a replay this way, since its checksums mean nothing here.
    pub fn on_current_tables(mut replay: Replay, scenarios: &[Scenario]) -> Result<Self, PlayError> {
        let (_, sim, cast) = Self::setup(&replay, scenarios, &LiveMoves::default())?;
        replay.set_tables_hash(sim.tables_hash());
        Ok(Self::started(sim, cast, replay))
    }

    fn setup<'a>(
        replay: &Replay,
        scenarios: &'a [Scenario],
        live: &LiveMoves,
    ) -> Result<(&'a Scenario, Simulation, Vec<ActorId>), PlayError> {
        let name = replay.scenario();
        let Some(scenario) = find_scenario(scenarios, name) else {
            let names: Vec<&str> = scenarios.iter().map(|scenario| scenario.name).collect();
            return Err(PlayError::UnknownScenario { name: name.to_owned(), known: names.join(", ") });
        };
        let (sim, cast) = live
            .start(scenario, replay.config())
            .map_err(|source| PlayError::Setup { name: name.to_owned(), source })?;
        Ok((scenario, sim, cast))
    }

    fn started(sim: Simulation, cast: Vec<ActorId>, replay: Replay) -> Self {
        let mut player = Self {
            tracked: Tracked::new(sim, replay.ticks()),
            replay,
            cursor: Cursor::START,
            next_command: 0,
            played: 0,
            cast,
        };
        player.apply_commands();
        player
    }

    /// The actors the scenario's input drives, the player first: who presentation stands where.
    #[must_use]
    pub fn cast(&self) -> &[ActorId] {
        &self.cast
    }

    /// Plays one tick, then carries out the commands that come before the next, so that between two calls
    /// the simulation stands as the recording's did between those two steps: a cutscene started there is
    /// playing, and presentation beside it can hold the next step for it, as it did for whoever watched.
    /// Returns `false`, and does nothing, once the replay has ended.
    pub fn step(&mut self) -> bool {
        let Some(actions) = self.cursor.next(&self.replay) else { return false };
        self.played += 1;
        self.tracked.step(&actions);
        self.apply_commands();
        true
    }

    /// Carries out the commands at the boundary the run stands on: before its first step as it starts, and
    /// after each step. The format allows commands at the boundary after the last tick, which this reaches
    /// with no step past it.
    fn apply_commands(&mut self) {
        while let Some(&(tick, command)) = self.replay.commands().get(self.next_command)
            && tick == self.played
        {
            // A recording only holds what started, so a refusal here is a crafted file, and it refuses
            // the same every run. A skip is presentation's: see the `timeline` module.
            if let TimelineCommand::Play(id) = command {
                let _ = self.tracked.sim.play_timeline(id);
            }
            self.next_command += 1;
        }
    }

    /// Plays every tick left.
    pub fn run_to_end(&mut self) {
        while self.step() {}
    }

    /// The replay it plays.
    #[must_use]
    pub fn replay(&self) -> &Replay {
        &self.replay
    }

    /// The simulation as it stands, for presentation to read.
    #[must_use]
    pub fn simulation(&self) -> &Simulation {
        &self.tracked.sim
    }

    /// The simulation, to change between ticks: a script's reload, say. What that moves shows in the
    /// checksums as it would in any recording made without it.
    pub fn simulation_mut(&mut self) -> &mut Simulation {
        &mut self.tracked.sim
    }

    /// Its checksums so far.
    #[must_use]
    pub fn trail(&self) -> &Trail {
        &self.tracked.trail
    }

    /// Ends the run wherever it has got to.
    #[must_use]
    pub fn finish(self) -> Played {
        self.tracked.finish()
    }
}

/// Makes a [`Replay`] by running a scenario on actions from an [`ActionSource`].
///
/// **A live edit ends the file, not the fight.** A replay is one [`Replay::tables_hash`] and every tick's
/// actions, so it can only say what a run on one set of tables did. When a live edit replaces a set
/// ([`Self::tune_move_set`]), the file stops at the last tick played on what it recorded, and the run goes
/// on, unrecorded: what's written still plays back to what happened, up to there, and it never claims
/// more. Writing each revision into the file would hold the rest too, but at the cost of a new format,
/// its bytes in every file and a player that replaces sets, for sessions nobody can reproduce once the
/// documents move on, so that waits for someone who needs them.
pub struct Recording {
    tracked: Tracked,
    recorder: Recorder,
    cast: Vec<ActorId>,
    /// Where a live edit ended the file.
    edited_at: Option<u32>,
}

impl Recording {
    /// Sets up `scenario`'s world with `config`.
    pub fn new(scenario: &Scenario, config: SimConfig) -> Result<Self, PlayError> {
        Self::tuned(scenario, config, &LiveMoves::default())
    }

    /// Sets up `scenario`'s world with `config`, from `live`'s edits of its documents, as
    /// [`LiveMoves::start`] does. The file's hash is theirs, so it plays back only on a build that has
    /// those edits in its documents: until then a [`Player`] says the move data changed, which it did.
    pub fn tuned(scenario: &Scenario, config: SimConfig, live: &LiveMoves) -> Result<Self, PlayError> {
        Self::tuned_with_setup(scenario, config, live, |_, _, _| Ok(()))
    }

    /// Configures the tick-zero world before recording its exact tables.
    pub fn tuned_with_setup(
        scenario: &Scenario,
        config: SimConfig,
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, PlayError> {
        // Before the world, so a pool too big for a file is refused before it's built.
        let mut recorder = Recorder::new(scenario.name, config, 0)?;
        let (mut sim, cast) = live
            .start(scenario, config)
            .map_err(|source| PlayError::Setup { name: scenario.name.to_owned(), source })?;
        setup(scenario, &mut sim, &cast)
            .map_err(|reason| PlayError::Configure { name: scenario.name.to_owned(), reason })?;
        recorder.set_tables_hash(sim.tables_hash());
        Ok(Self { tracked: Tracked::new(sim, MAX_TICKS), recorder, cast, edited_at: None })
    }

    /// Replaces a set live, between steps, and ends the file if that changed anything: see the
    /// [type's docs](Self). `None` for an ID the simulation never gave.
    pub fn tune_move_set(&mut self, id: MoveSetId, set: MoveSet) -> Option<Tuned> {
        let tuned = self.tracked.sim.tune_move_set(id, set)?;
        if matches!(tuned, Tuned::Replaced { .. }) && self.edited_at.is_none() {
            self.edited_at = Some(self.recorder.ticks());
        }
        Some(tuned)
    }

    /// Changes the weights a soft lock is chosen by, between steps, and ends the file if that changed
    /// anything, as [`Self::tune_move_set`] does. Says whether it did.
    pub fn tune_targeting(&mut self, tuning: TargetingTuning) -> bool {
        let changed = self.tracked.sim.tune_targeting(tuning);
        if changed && self.edited_at.is_none() {
            self.edited_at = Some(self.recorder.ticks());
        }
        changed
    }

    /// The tick a live edit ended the file at, if one did: it holds this many ticks.
    #[must_use]
    pub const fn edited_at(&self) -> Option<u32> {
        self.edited_at
    }

    /// The scenario it runs, as its file will name it.
    #[must_use]
    pub fn scenario(&self) -> &str {
        self.recorder.scenario()
    }

    /// How many ticks the file holds so far.
    #[must_use]
    pub fn recorded_ticks(&self) -> u32 {
        self.recorder.ticks()
    }

    /// The actors the scenario's input drives, the player first: who presentation stands where.
    #[must_use]
    pub fn cast(&self) -> &[ActorId] {
        &self.cast
    }

    /// Starts a timeline before the next step, and records that.
    pub fn play_timeline(&mut self, id: TimelineId) -> Result<(), TimelinePlayError> {
        self.tracked.sim.play_timeline(id)?;
        if self.edited_at.is_none() {
            self.recorder.command(TimelineCommand::Play(id));
        }
        Ok(())
    }

    /// Records that someone skipped the timeline playing, `at_ms` into it, and runs the steps it holds
    /// input for, on neutral input: exactly the steps watching it would have run. See the `timeline`
    /// module, which is why this is all a skip is. A move's cinematic is refused, recording nothing.
    pub fn skip_timeline(&mut self, at_ms: u32) -> Result<(), PlayError> {
        if self.tracked.sim.timeline().is_some_and(|run| run.owner.is_some()) {
            return Err(PlayError::Unskippable { tick: self.recorded_ticks() });
        }
        if self.edited_at.is_none() {
            self.recorder.command(TimelineCommand::Skip { at_ms });
        }
        let mut neutral = |_: u64, _: &[ActorId]| TickActions::NEUTRAL;
        for _ in 0..self.tracked.sim.timeline_steps_held() {
            self.step(&mut neutral)?;
        }
        Ok(())
    }

    /// Runs `script`, its commands included, until the file holds at least `ticks` ticks.
    /// Refuses to extend a file ended by a live edit, without consuming any actions or commands.
    pub fn run_script(&mut self, script: &Script, ticks: u32) -> Result<(), PlayError> {
        if self.recorded_ticks() < ticks
            && let Some(tick) = self.edited_at
        {
            return Err(PlayError::RecordingEnded { tick });
        }
        let mut source = script.actions;
        let mut commands = script.commands.iter().peekable();
        while self.recorded_ticks() < ticks {
            while let Some(&(_, command)) = commands.next_if(|&&(at, _)| at <= self.recorded_ticks()) {
                match command {
                    TimelineCommand::Play(id) => self.play_timeline(id)?,
                    TimelineCommand::Skip { at_ms } => self.skip_timeline(at_ms)?,
                }
            }
            self.step(&mut source)?;
        }
        Ok(())
    }

    /// Asks `source` for the next tick's actions, records them and steps. Refuses at the file's limit
    /// on a run's length, with the recording still good up to there, and no step taken. Past a live
    /// edit it steps and records nothing: see the [type's docs](Self).
    pub fn step(&mut self, source: &mut dyn ActionSource) -> Result<(), ReplayError> {
        let actions = source.actions(self.tracked.sim.tick(), &self.cast);
        if self.edited_at.is_some() {
            self.tracked.step_unkept(&actions);
            return Ok(());
        }
        self.recorder.push(&actions)?;
        self.tracked.step(&actions);
        Ok(())
    }

    /// The simulation as it stands, for presentation to read.
    #[must_use]
    pub fn simulation(&self) -> &Simulation {
        &self.tracked.sim
    }

    /// The replay as far as it's got: what [`Self::finish`] would give now, with the run going on, for a
    /// file that outlives a process or a save that may fail.
    #[must_use]
    pub fn so_far(&self) -> Replay {
        self.recorder.clone().finish()
    }

    /// The finished replay, and what the run did.
    #[must_use]
    pub fn finish(self) -> (Replay, Played) {
        (self.recorder.finish(), self.tracked.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::super::script::{SCRIPTS, melee_sweep};
    use super::super::{MAX_ACTORS, SCENARIOS, ScenarioSetup};
    use super::*;
    use crate::actions::{ActorActions, Buttons};

    fn melee() -> &'static Scenario {
        &SCENARIOS[0]
    }

    fn record(ticks: u32, mut source: impl ActionSource) -> (Replay, Played) {
        let mut recording = Recording::new(melee(), melee().config).unwrap();
        for _ in 0..ticks {
            recording.step(&mut source).unwrap();
        }
        recording.finish()
    }

    /// What a recording holds so far is what it would have finished with at that tick, and asking changes
    /// nothing about the rest.
    #[test]
    fn a_recording_so_far_is_a_whole_replay_of_those_ticks() {
        let mut recording = Recording::new(melee(), melee().config).unwrap();
        let mut source = melee_sweep;
        for _ in 0..100 {
            recording.step(&mut source).unwrap();
        }
        let so_far = recording.so_far();
        assert_eq!(so_far.ticks(), 100);
        assert_eq!(recording.scenario(), melee().name);
        let (replay, _) = record(100, melee_sweep);
        assert_eq!(so_far.encode(), replay.encode());

        for _ in 0..100 {
            recording.step(&mut source).unwrap();
        }
        let (whole, _) = recording.finish();
        let (replay, _) = record(200, melee_sweep);
        assert_eq!(whole.encode(), replay.encode());
    }

    #[test]
    fn a_recording_plays_back_to_the_same_trail() {
        let (replay, recorded) = record(300, melee_sweep);
        assert_eq!(replay.ticks(), 300);
        assert_eq!(recorded.trail.ticks(), 300);

        let mut player = Player::new(replay.clone(), &SCENARIOS).unwrap();
        player.run_to_end();
        assert!(!player.step(), "past the end, it does nothing");
        assert_eq!(player.simulation().tick(), 300);
        assert_eq!(player.replay().ticks(), 300);
        let played = player.finish();
        assert_eq!(played.trail, recorded.trail);
        assert_eq!(played.stats.steps, 300);

        // And through the file.
        let decoded = Replay::decode(&replay.encode()).unwrap();
        assert_eq!(decoded, replay);
    }

    #[test]
    fn the_source_sees_each_tick_and_the_cast_in_order() {
        let mut seen = Vec::new();
        let (replay, _) = record(5, |tick: u64, cast: &[ActorId]| {
            seen.push((tick, cast.len()));
            TickActions::NEUTRAL
        });
        assert_eq!(seen, [(0, 3), (1, 3), (2, 3), (3, 3), (4, 3)]);
        assert_eq!(replay.actions().count(), 5);
    }

    #[test]
    fn a_scenario_this_build_lacks_is_named() {
        let scenario = Scenario { name: "gone", ..*melee() };
        let mut recording = Recording::new(&scenario, melee().config).unwrap();
        recording.step(&mut super::super::script::idle).unwrap();
        let (replay, _) = recording.finish();
        let error = Player::new(replay, &SCENARIOS).err().expect("refused");
        assert!(matches!(&error, PlayError::UnknownScenario { name, .. } if name == "gone"));
        assert!(error.to_string().contains("melee"), "it lists what there is: {error}");
    }

    #[test]
    fn too_big_a_pool_is_refused_before_anything_is_built() {
        let config = SimConfig { max_actors: MAX_ACTORS + 1, ..melee().config };
        assert!(matches!(
            Recording::new(melee(), config),
            Err(PlayError::File(ReplayError::TooManyActors { .. }))
        ));
    }

    #[test]
    fn a_standing_preparation_setup_change_refuses_old_replay() {
        use crate::replay::scenario::{PreparedFight, duel};

        fn prepared_duel(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
            let cast = duel(sim)?;
            sim.prepare_standing_jump(cast[0]);
            Ok(cast)
        }

        let before = *find_scenario(&SCENARIOS, "duel").unwrap();
        let after = Scenario { setup: ScenarioSetup::Code(prepared_duel), ..before };
        let record_jump = |scenario: &Scenario| {
            let mut recording = Recording::new(scenario, scenario.config).unwrap();
            let mut source = |tick, cast: &[ActorId]| {
                let mut actions = TickActions::NEUTRAL;
                if tick == 52 {
                    actions
                        .set(cast[0], ActorActions { buttons: Buttons::JUMP, ..Default::default() })
                        .unwrap();
                }
                actions
            };
            for _ in 0..55 {
                recording.step(&mut source).unwrap();
            }
            let (replay, played) = recording.finish();
            (Replay::decode(&replay.encode()).unwrap(), played)
        };
        let (old, _) = record_jump(&before);
        let (fresh, expected) = record_jump(&after);
        assert!(Player::new(old.clone(), &[before]).is_ok());
        let result = Player::new(old.clone(), &[after]);
        assert!(matches!(
            result,
            Err(PlayError::MovesChanged { recorded, current })
                if recorded == old.tables_hash() && current == fresh.tables_hash()
        ));
        assert!(old.actions().eq(fresh.actions()), "only the setup changes the replay header");
        let mut player = Player::new(fresh, &[after]).unwrap();
        player.run_to_end();
        assert_eq!(player.finish().trail, expected.trail);

        let scenario = find_scenario(&SCENARIOS, "arena-fight").unwrap();
        let mut fight = PreparedFight::new(false)
            .unwrap()
            .start_resettable(scenario.config, &Default::default())
            .unwrap();
        let hash = fight.tables_hash();
        fight.step(&TickActions::NEUTRAL);
        fight.restart().unwrap();
        assert_eq!(fight.tables_hash(), hash);
        assert_eq!(fight.tables_hash(), scenario.start(scenario.config).unwrap().0.tables_hash());
    }

    fn configure_tables(_: &Scenario, sim: &mut Simulation, cast: &[ActorId]) -> Result<(), String> {
        assert_eq!(sim.tick(), 0);
        assert!(!cast.is_empty());
        let profile = crate::ControllerProfile { run_speed: 7.0, ..Default::default() };
        sim.add_controller_profile(&profile).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[test]
    fn legacy_replay_keeps_the_exact_unconfigured_path() {
        let (replay, recorded) = record(30, melee_sweep);
        let mut player = Player::new_with_setup(replay, &SCENARIOS, |_, _, _| {
            panic!("legacy replay tried to load the optional profiles");
        })
        .unwrap();
        player.run_to_end();
        assert_eq!(player.finish().trail, recorded.trail);
    }

    #[test]
    fn configured_recording_requires_the_matching_setup_before_playback() {
        let mut recording =
            Recording::tuned_with_setup(melee(), melee().config, &LiveMoves::default(), configure_tables)
                .unwrap();
        for _ in 0..30 {
            recording.step(&mut melee_sweep).unwrap();
        }
        let (replay, recorded) = recording.finish();
        assert!(matches!(Player::new(replay.clone(), &SCENARIOS), Err(PlayError::MovesChanged { .. })));
        let unavailable = Player::new_with_setup(replay.clone(), &SCENARIOS, |_, _, _| {
            Err("the required mesh is missing".to_owned())
        });
        assert!(matches!(unavailable, Err(PlayError::Configure { .. })));
        let mut wrong = replay.clone();
        wrong.set_tables_hash(replay.tables_hash() ^ 1);
        assert!(matches!(
            Player::new_with_setup(wrong, &SCENARIOS, configure_tables),
            Err(PlayError::MovesChanged { .. })
        ));
        let mut player = Player::new_with_setup(replay, &SCENARIOS, configure_tables).unwrap();
        player.run_to_end();
        assert_eq!(player.finish().trail, recorded.trail);
    }

    #[test]
    fn zero_tick_commands_run_after_configured_hash_validation() {
        let scenario = Scenario {
            name: "setup-test",
            setup: ScenarioSetup::Code(crate::replay::scenario::timelines),
            ..*melee()
        };
        let mut recording =
            Recording::tuned_with_setup(&scenario, scenario.config, &LiveMoves::default(), configure_tables)
                .unwrap();
        recording.play_timeline(TimelineId::from_raw(0)).unwrap();
        let (replay, _) = recording.finish();
        assert_eq!(replay.ticks(), 0);
        assert_eq!(replay.commands().len(), 1);
        let player = Player::new_with_setup(replay, &[scenario], |scenario, sim, cast| {
            assert!(sim.timeline().is_none(), "a command ran before the setup");
            configure_tables(scenario, sim, cast)
        })
        .unwrap();
        assert!(player.simulation().timeline().is_some());
        assert_eq!(player.simulation().tick(), 0);
    }

    /// A move that changed on purpose: the replay still decodes, and refuses to run until it's
    /// stamped again. It never steps.
    #[test]
    fn a_mismatched_move_hash_refuses_and_says_re_record() {
        fn moved(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
            let cast = crate::replay::scenario::melee(sim)?;
            sim.add_move_set(crate::replay::scenario::test_moves()?);
            Ok(cast)
        }
        let (mut replay, played) = record(30, melee_sweep);
        let scenarios = [Scenario { setup: ScenarioSetup::Code(moved), ..*melee() }];

        let error = Player::new(replay.clone(), &scenarios).err().expect("refused");
        let PlayError::MovesChanged { recorded, current } = error else { panic!("{error}") };
        assert_eq!(recorded, replay.tables_hash());
        assert_ne!(recorded, current);
        let message = error.to_string();
        assert!(message.contains("re-record") && message.contains("--update"), "{message}");

        // The file decodes and carries the hash; only a run looks at it.
        let decoded = Replay::decode(&replay.encode()).unwrap();
        assert_eq!(decoded.tables_hash(), recorded);

        // Stamped with the new hash, it runs the same actions in the new world.
        replay.set_tables_hash(current);
        let mut player = Player::new(replay, &scenarios).unwrap();
        player.run_to_end();
        assert_eq!(player.finish().trail.ticks(), played.trail.ticks());
    }

    /// A changed controller profile refuses a replay as changed moves do: the same stick would carry
    /// it somewhere else.
    #[test]
    fn a_changed_profile_refuses_and_says_re_record() {
        fn retuned(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
            let cast = crate::replay::scenario::melee(sim)?;
            let run_speed = crate::ControllerProfile { run_speed: 7.0, ..Default::default() };
            sim.add_controller_profile(&run_speed)?;
            Ok(cast)
        }
        let (replay, _) = record(30, melee_sweep);
        let scenarios = [Scenario { setup: ScenarioSetup::Code(retuned), ..*melee() }];
        let error = Player::new(replay, &scenarios).err().expect("refused");
        assert!(matches!(error, PlayError::MovesChanged { .. }), "{error}");
        assert!(error.to_string().contains("--update"), "{error}");
    }

    /// A press that nothing but the checksum sees yet still names its tick.
    #[test]
    fn a_tick_changed_in_the_actions_is_named_by_the_recorded_trail() {
        use super::super::checksums::RecordedTrail;
        let (_, played) = record(200, melee_sweep);
        let recorded = RecordedTrail::from_trail(&played.trail);
        let actor = ActorId::from_raw(2).unwrap();
        for changed in [0_u64, 1, 59, 60, 61, 120, 199] {
            let (other, _) = record(200, move |tick: u64, cast: &[ActorId]| {
                let mut actions = melee_sweep(tick, cast);
                if tick == changed {
                    let was = actions.get(actor);
                    let pressed = ActorActions { buttons: was.buttons | Buttons::JUMP, ..was };
                    actions.set(actor, pressed).unwrap();
                }
                actions
            });
            let mut player = Player::new(other, &SCENARIOS).unwrap();
            player.run_to_end();
            let difference = recorded.first_difference(&player.finish().trail).expect("it differs");
            assert_eq!(u64::from(difference.tick), changed + 1, "actions of tick {changed} went in");
        }
        assert_eq!(recorded.first_difference(&played.trail), None);
    }

    #[test]
    fn every_script_records_and_plays_back_on_every_scenario() {
        for scenario in SCENARIOS.iter() {
            for script in SCRIPTS {
                let mut recording = Recording::new(scenario, scenario.config).unwrap();
                let mut source = script.actions;
                for _ in 0..200 {
                    recording.step(&mut source).unwrap();
                }
                let (replay, recorded) = recording.finish();
                let mut player = Player::new(replay, &SCENARIOS).unwrap();
                player.run_to_end();
                assert_eq!(player.finish().trail, recorded.trail, "{} {}", scenario.name, script.name);
            }
        }
    }
}
