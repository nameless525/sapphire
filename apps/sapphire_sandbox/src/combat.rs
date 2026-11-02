//! `--replay` and `--record`: a combat simulation stepping beside the scene, one tick per fixed step
//! that isn't paused.
//!
//! What this holds is the property everything later leans on: a run inside the app, with the renderer
//! beside it, reaches the same checksums as the headless runner, through the same
//! `sapphire_gameplay::replay` code. The arena's actors follow [`Combat::simulation`] (`crate::arena`),
//! and the combat views (`combat_views.rs`) draw what it holds; nothing else shows the fight. `--replay`
//! compares with the recorded checksums next to the file, where there are any, and fails the run if it
//! differs, so a golden or a bench from a recorded fight can't come from another fight.
//!
//! **Where a recording's actions come from:** [`action_source`], which returns an
//! [`ActionSource`]: a named script, with `--capture` or `--bench`, or in the windowed app the
//! bindings of whoever's playing, on any platform (`--record <file> --scenario <name>`). So the file
//! holds what was played, with each stick turned by the camera it was played under, and it's written
//! when the app quits. Everything before that is in memory, so a killed process leaves no file.
//!
//! `--replay` needs `--capture` or `--bench`, for now. `--arena --record` shows its fight in a window,
//! and with `--script`, `--offscreen` runs one for hot reload and live link.
//!
//! **Live edits (M2 T2, `tuning.rs`):** a replay plays what it recorded and refuses them. A recording's
//! fight takes them, and its file stops at the first that changes anything: `Recording` says why. A fight
//! with no file takes them too, and starts over from them.
//!
//! **A fight with no file** (`--arena --fight`, M2 G6): [`Mode::Fighting`] asks the same
//! [`action_source`] for its ticks and writes nothing, so it can go on for as long as anyone plays. When
//! someone falls it waits [`RESTART_AFTER`], logs how it went and makes the scenario again, and the key
//! that starts it over does the same at once ([`Combat::restart`]). A new fight is a new [`Simulation`],
//! from the same first tick: nothing carries over, and a script plays it exactly as it played the one
//! before. That's what the app's own test leans on (`tests/replays.rs`): each fight's checksum in the log
//! is the one a headless run of the same script reaches at that tick. Making one allocates, between steps,
//! outside what rule R6 counts ([`read_bindings`]'s caller keeps it so).

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_gameplay::replay::{
    ARCHITECTURE, ActionSource, ChecksumFile, Divergence, LiveMoves, Played, Player, RecordedTrail,
    Recording, Replay, ReplayError, SCENARIOS, SCRIPTS, Scenario, checksums_file_name, find_scenario,
    find_script,
};
use sapphire_gameplay::{ActorId, Simulation, TickActions};
#[cfg(feature = "dev")]
use sapphire_gameplay::{MoveSetId, Tuned};

use crate::LOG_TARGET;
use crate::options::{CaptureFrames, CombatOption};

/// A script's actions: a function of the tick.
type ScriptActions = fn(u64, &[ActorId]) -> TickActions;

/// How long a fight goes on after someone falls before it starts over, in ticks: long enough to see
/// them fall. The warrior's death lasts 150.
pub(crate) const RESTART_AFTER: u64 = 240;

/// Where a recording's actions come from: the one seam between what drives a fight and the file.
///
/// A script's function is a source. So are the player's bindings, read a tick at a time
/// (`sapphire_controls::ControlsSource`), which is what's left with no script.
fn action_source<'a>(
    script: &'a mut Option<ScriptActions>,
    bindings: &'a mut dyn ActionSource,
) -> &'a mut dyn ActionSource {
    match script {
        Some(actions) => actions,
        None => bindings,
    }
}

/// The bindings where there's no one to hold them: nothing asks this, since a recording without a
/// window always has a script (`options.rs`).
pub(crate) fn no_bindings(_tick: u64, _cast: &[ActorId]) -> TickActions {
    TickActions::NEUTRAL
}

enum Mode {
    Replaying {
        player: Player,
        /// The file's name without its extension, which is its name in checksum files.
        name: String,
        /// What it must reach here, if anything is recorded for this architecture.
        recorded: Option<RecordedTrail>,
    },
    Recording {
        recording: Recording,
        /// The scenario it runs.
        scenario: &'static str,
        /// The script it plays. With none, it records the bindings it's given.
        script: Option<ScriptActions>,
        /// The tick limit, once reached.
        stopped: Option<ReplayError>,
    },
    /// `--fight`: played, and kept nowhere.
    Fighting(LiveFight),
}

/// A fight with no file: see the [module docs](self).
struct LiveFight {
    scenario: &'static Scenario,
    /// The edits it's taken: what the next fight starts from.
    live: LiveMoves,
    sim: Simulation,
    cast: Vec<ActorId>,
    /// The script it plays. With none, it plays the bindings it's given.
    script: Option<ScriptActions>,
    /// The tick someone fell at.
    fell_at: Option<u64>,
    /// How many fights there have been, this one included.
    fights: u32,
}

impl LiveFight {
    fn new(
        scenario: &'static Scenario,
        script: Option<ScriptActions>,
        live: LiveMoves,
    ) -> Result<Self, String> {
        let (sim, cast) =
            live.start(scenario, scenario.config).map_err(|error| format!("--fight: {error}"))?;
        Ok(Self { scenario, live, sim, cast, script, fell_at: None, fights: 1 })
    }

    /// One tick, and whatever it ended. Returns whether it asked `bindings`.
    fn step(&mut self, bindings: &mut dyn ActionSource) -> bool {
        let asks = self.script.is_none();
        let actions = action_source(&mut self.script, bindings).actions(self.sim.tick(), &self.cast);
        self.sim.step(&actions);
        let tick = self.sim.tick();
        match self.fell_at {
            None if self.someone_fell() => self.fell_at = Some(tick),
            Some(at) if tick - at >= RESTART_AFTER => self.start_over(),
            _ => {}
        }
        asks
    }

    /// A party falls with its last member: until then, a fall only brings in the next.
    fn someone_fell(&self) -> bool {
        let party = self.sim.party();
        self.cast.iter().any(|id| {
            if party.input_actor() == Some(*id) {
                return party.is_down();
            }
            self.sim.actor(*id).is_some_and(|actor| actor.fighter.is_defeated())
        })
    }

    /// Reports this fight and makes the next. If it can't, which a scenario that started once has no
    /// reason for, this one goes on.
    fn start_over(&mut self) {
        self.report();
        match self.live.start(self.scenario, self.scenario.config) {
            Ok((sim, cast)) => {
                (self.sim, self.cast) = (sim, cast);
                self.fell_at = None;
                self.fights += 1;
            }
            Err(error) => tracing::error!(target: LOG_TARGET, %error, "the fight can't start over"),
        }
    }

    /// Takes a live edit: the next attempt reads it, and every fight after this one starts from it.
    #[cfg(feature = "dev")]
    fn tune(&mut self, document: &str, id: MoveSetId, set: sapphire_moves::MoveSet) -> Option<Tuned> {
        let tuned = self.sim.tune_move_set(id, set.clone())?;
        if matches!(tuned, Tuned::Replaced { .. }) {
            self.live.remember(document, set);
        }
        Some(tuned)
    }

    /// Says how this fight went.
    fn report(&self) {
        let health = |index: usize| {
            let id = self.cast.get(index).map(|id| self.sim.party().on_field(*id));
            id.and_then(|id| self.sim.actor(id)).map_or(0, |actor| actor.fighter.health())
        };
        tracing::info!(
            target: LOG_TARGET,
            fight = self.fights,
            ticks = self.sim.tick(),
            fell_at = ?self.fell_at,
            player_health = health(0),
            enemy_health = health(1),
            checksum = %format_args!("{:016x}", self.sim.checksum()),
            allocations = self.sim.stats().allocations,
            "a fight ended",
        );
    }
}

/// A simulation run alongside the scene.
pub(crate) struct Combat {
    /// The replay file, read or to write. A fight with no file has none.
    path: PathBuf,
    mode: Mode,
    /// Fixed steps taken after a replay's last tick.
    steps_past_end: u64,
}

impl Combat {
    /// Reads what `option` names and sets up the world, before any GPU work, so a bad file costs
    /// nothing and says why.
    pub fn new(option: &CombatOption) -> Result<Self, String> {
        match option {
            CombatOption::Replay { path, .. } => Self::replaying(path),
            CombatOption::Record { path, scenario, script } => {
                Self::recording(path, scenario, script.as_deref(), &LiveMoves::default())
            }
            CombatOption::Fight { script, party } => Self::fighting(script.as_deref(), *party),
        }
    }

    fn fighting(script: Option<&str>, party: bool) -> Result<Self, String> {
        let name = if party { crate::arena::PARTY_SCENARIO } else { crate::arena::FIGHT_SCENARIO };
        let scenario =
            find_scenario(SCENARIOS, name).ok_or_else(|| format!("--fight: no scenario `{name}`"))?;
        let fight = LiveFight::new(scenario, script.map(script_actions).transpose()?, LiveMoves::default())?;
        Ok(Self { path: PathBuf::new(), mode: Mode::Fighting(fight), steps_past_end: 0 })
    }

    fn replaying(path: &Path) -> Result<Self, String> {
        let fail = |error: &dyn std::fmt::Display| format!("--replay {}: {error}", path.display());
        let bytes = fs::read(path).map_err(|error| fail(&error))?;
        let replay = Replay::decode(&bytes).map_err(|error| fail(&error))?;
        let player = Player::new(replay, SCENARIOS).map_err(|error| fail(&error))?;
        let name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_owned();
        let recorded = recorded_trail(path, &name).map_err(|error| fail(&error))?;
        let mode = Mode::Replaying { player, name, recorded };
        Ok(Self { path: path.to_owned(), mode, steps_past_end: 0 })
    }

    /// Sets up a recording of `scenario`, from `live`'s edits of its documents.
    fn recording(
        path: &Path,
        scenario: &str,
        script: Option<&str>,
        live: &LiveMoves,
    ) -> Result<Self, String> {
        // Up front rather than after the run: it may be a session someone played, which no script
        // makes again.
        if path.exists() {
            return Err(format!("--record {}: it exists; delete it first to replace it", path.display()));
        }
        if path.parent().is_some_and(|parent| !parent.as_os_str().is_empty() && !parent.is_dir()) {
            return Err(format!("--record {}: its directory doesn't exist", path.display()));
        }
        let Some(scenario) = find_scenario(SCENARIOS, scenario) else {
            let known: Vec<&str> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
            return Err(format!("--scenario: no scenario `{scenario}`; there's {known:?}"));
        };
        let script = script.map(script_actions).transpose()?;
        let recording = Recording::tuned(scenario, scenario.config, live)
            .map_err(|error| format!("--record: {error}"))?;
        let mode = Mode::Recording { recording, scenario: scenario.name, script, stopped: None };
        Ok(Self { path: path.to_owned(), mode, steps_past_end: 0 })
    }

    /// The scenario the run starts from.
    pub fn scenario(&self) -> &str {
        match &self.mode {
            Mode::Replaying { player, .. } => player.replay().scenario(),
            Mode::Recording { scenario, .. } => scenario,
            Mode::Fighting(fight) => fight.scenario.name,
        }
    }

    /// The set that plays the document with this file name, in this fight (M2 T2).
    #[cfg(feature = "dev")]
    pub fn plays(&self, document: &str) -> Option<MoveSetId> {
        find_scenario(SCENARIOS, self.scenario())?.set_of(document)
    }

    /// Gives this fight a live edit of the document with this file name, between steps: what the set
    /// that plays it did with it, if one does. A replay refuses, since it plays what it recorded and
    /// [`Self::check`] would fail it for anything else. A recording ends its file at the first edit
    /// that changes anything.
    #[cfg(feature = "dev")]
    pub fn tune(
        &mut self,
        document: &str,
        set: sapphire_moves::MoveSet,
    ) -> Result<Option<(MoveSetId, Tuned)>, String> {
        let Some(id) = self.plays(document) else { return Ok(None) };
        match &mut self.mode {
            Mode::Replaying { name, .. } => {
                Err(format!("`{name}` is a replay, which plays what it recorded: it takes no live edits"))
            }
            Mode::Recording { recording, .. } => {
                let ended = recording.edited_at();
                let tuned = recording.tune_move_set(id, set);
                if let (None, Some(tick)) = (ended, recording.edited_at()) {
                    tracing::warn!(
                        target: LOG_TARGET,
                        document,
                        tick,
                        "a live edit ends the recording's file at this tick: the fight goes on, unrecorded",
                    );
                }
                Ok(tuned.map(|tuned| (id, tuned)))
            }
            Mode::Fighting(fight) => Ok(fight.tune(document, id, set).map(|tuned| (id, tuned))),
        }
    }

    /// Starts a fight that hasn't stepped from `set`, if that isn't what it has: for what a cook made of
    /// this document, when it's newer than the simulation's own copy. Says whether it did. Nothing else
    /// has anywhere to start from: a replay plays what it recorded, and a recording's file says what it
    /// ran on.
    #[cfg(feature = "dev")]
    pub fn start_fight_from(&mut self, document: &str, set: sapphire_moves::MoveSet) -> bool {
        let Some(id) = self.plays(document) else { return false };
        let Mode::Fighting(fight) = &mut self.mode else { return false };
        if fight.sim.tick() != 0 || fight.sim.move_library().is_current(id, &set) {
            return false;
        }
        fight.live.remember(document, set);
        match fight.live.start(fight.scenario, fight.scenario.config) {
            Ok((sim, cast)) => (fight.sim, fight.cast) = (sim, cast),
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, "the fight keeps the moves it has");
                return false;
            }
        }
        true
    }

    /// The tick a live edit ended a recording's file at, if one did. A fight has no file to end.
    #[cfg(any(feature = "dev", test))]
    pub fn edited_at(&self) -> Option<u32> {
        match &self.mode {
            Mode::Replaying { .. } | Mode::Fighting(_) => None,
            Mode::Recording { recording, .. } => recording.edited_at(),
        }
    }

    /// The actors the scenario's input drives, the player first.
    pub fn cast(&self) -> &[ActorId] {
        match &self.mode {
            Mode::Replaying { player, .. } => player.cast(),
            Mode::Recording { recording, .. } => recording.cast(),
            Mode::Fighting(fight) => &fight.cast,
        }
    }

    /// The tick a capture stops at: the replay's end unless `--capture-ticks` said otherwise, and
    /// never past it, since the frames after would show no fight.
    pub fn capture_ticks(
        &self,
        option: &CombatOption,
        ticks: u32,
        frames: CaptureFrames,
    ) -> Result<u32, String> {
        let (Mode::Replaying { player, .. }, CombatOption::Replay { to_end, .. }) = (&self.mode, option)
        else {
            return Ok(ticks);
        };
        let length = player.replay().ticks();
        let ticks = if *to_end { length } else { ticks };
        if ticks > length {
            return Err(format!("--capture-ticks {ticks} is past the replay's end, tick {length}"));
        }
        if frames.first_tick(ticks).is_none() {
            return Err(format!(
                "--capture-frames {} --capture-step {} start before tick 0 for a capture at tick {ticks}",
                frames.count, frames.step
            ));
        }
        Ok(ticks)
    }

    /// One tick, with whatever else this fixed step does. Past a replay's end, or a recording's
    /// limit, it does nothing and counts. Returns whether it asked `bindings`.
    pub fn step(&mut self, bindings: &mut dyn ActionSource) -> bool {
        match &mut self.mode {
            Mode::Replaying { player, .. } => {
                if !player.step() {
                    self.steps_past_end += 1;
                }
                false
            }
            Mode::Recording { recording, script, stopped, .. } => {
                if stopped.is_some() {
                    return false;
                }
                let asks = script.is_none();
                if let Err(error) = recording.step(action_source(script, bindings)) {
                    *stopped = Some(error);
                }
                asks
            }
            Mode::Fighting(fight) => fight.step(bindings),
        }
    }

    /// Starts a fight with no file over, between steps. Nothing else has anywhere to start over from.
    pub fn restart(&mut self) {
        if let Mode::Fighting(fight) = &mut self.mode {
            fight.start_over();
        }
    }

    /// The simulation as it stands.
    pub fn simulation(&self) -> &Simulation {
        match &self.mode {
            Mode::Replaying { player, .. } => player.simulation(),
            Mode::Recording { recording, .. } => recording.simulation(),
            Mode::Fighting(fight) => &fight.sim,
        }
    }

    /// Draws the combat views that are on: once a frame, before its debug draws are collected, and
    /// after `Scene::animate`, so M2 N3's `VisiblePose` has the frame's pose to read.
    pub fn draw_views(&self) {
        crate::combat_views::draw_current(self.simulation());
    }

    /// What's running, for the overlay's recorder.
    #[cfg(any(feature = "dev", test))]
    pub fn status(&self) -> CombatStatus<'_> {
        match &self.mode {
            Mode::Replaying { player, name, .. } => {
                CombatStatus::Replaying { name, length: player.replay().ticks() }
            }
            Mode::Recording { scenario, script, stopped, .. } => CombatStatus::Recording {
                scenario,
                path: &self.path,
                at_limit: stopped.is_some(),
                scripted: script.is_some(),
            },
            Mode::Fighting(fight) => CombatStatus::Fighting { fights: fight.fights },
        }
    }

    /// An error if the run has left its recorded checksums, as far as it's got. Checked before a
    /// capture writes its picture, which then can't show another fight.
    pub fn check(&self) -> Result<(), String> {
        let Mode::Replaying { player, name, recorded: Some(recorded) } = &self.mode else { return Ok(()) };
        match recorded.first_difference(player.trail()) {
            Some(difference) => Err(diverged(name, &self.path, difference)),
            None => Ok(()),
        }
    }

    /// Ends the run: checks a replay, or writes a recording, and logs where it got to.
    pub fn finish(self) -> Result<(), String> {
        self.check()?;
        let Self { path, mode, steps_past_end } = self;
        match mode {
            Mode::Replaying { player, name, recorded } => {
                let length = player.replay().ticks();
                let played = player.finish();
                log_gate(&played);
                let ticks = played.trail.ticks();
                let end = played.trail.end();
                match recorded {
                    Some(_) => tracing::info!(
                        target: LOG_TARGET,
                        replay = name,
                        ticks,
                        of = length,
                        checksum = %format_args!("{end:016x}"),
                        allocations = played.stats.allocations,
                        steps_past_end,
                        "replay reached its recorded checksums",
                    ),
                    None => tracing::warn!(
                        target: LOG_TARGET,
                        replay = name,
                        ticks,
                        of = length,
                        checksum = %format_args!("{end:016x}"),
                        allocations = played.stats.allocations,
                        steps_past_end,
                        "replay played, with nothing recorded for it on {ARCHITECTURE} to compare with",
                    ),
                }
                if steps_past_end > 0 {
                    tracing::warn!(
                        target: LOG_TARGET,
                        steps_past_end,
                        "the replay ended early: those last fixed steps had no combat in them",
                    );
                }
            }
            Mode::Recording { recording, stopped, .. } => {
                if let Some(error) = stopped {
                    tracing::warn!(target: LOG_TARGET, %error, "the recording stopped at its limit");
                }
                if let Some(tick) = recording.edited_at() {
                    let played = recording.simulation().tick();
                    tracing::warn!(
                        target: LOG_TARGET,
                        tick,
                        played,
                        "the file ends where a live edit changed the moves: it can't answer for the rest",
                    );
                }
                let (replay, played) = recording.finish();
                log_gate(&played);
                let bytes = replay.encode();
                fs::write(&path, &bytes).map_err(|error| format!("--record {}: {error}", path.display()))?;
                tracing::info!(
                    target: LOG_TARGET,
                    path = %path.display(),
                    ticks = replay.ticks(),
                    bytes = bytes.len(),
                    checksum = %format_args!("{:016x}", played.trail.end()),
                    allocations = played.stats.allocations,
                    "recorded a replay",
                );
            }
            Mode::Fighting(fight) => fight.report(),
        }
        Ok(())
    }
}

/// What a [`Combat`] is running, as the overlay shows it.
#[cfg(any(feature = "dev", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CombatStatus<'a> {
    Replaying { name: &'a str, length: u32 },
    Recording { scenario: &'static str, path: &'a Path, at_limit: bool, scripted: bool },
    Fighting { fights: u32 },
}

/// The overlay's replay recorder: starts recording the player, and ends it, between frames.
///
/// **A recording always starts its scenario over.** A replay is a scenario's name, a seed and every
/// tick's actions from tick 0. It holds no state of its own, and the simulation has no way to write
/// one, so a tick in the middle of a fight has nothing a replay could begin from. So there's no
/// recording what's on screen: Start makes a new simulation of the scenario, and refuses while a fight
/// is running, which someone may want to keep.
///
/// **The bindings stay read once a step.** This only changes which [`Combat`] there is, in between
/// steps, and [`read_bindings`] asks them exactly once of whichever it finds.
///
/// Files go in `dir`, named after their scenario with the first free number, so nothing is overwritten
/// and a session is never lost to a name. Each is a replay like any other: to keep one, put it in
/// `tests/replays/` and run `cargo xtask replay --update`.
#[cfg(any(feature = "dev", test))]
pub(crate) struct SessionRecorder {
    dir: PathBuf,
    /// How the last save went, or why the last request failed.
    note: Option<String>,
}

#[cfg(any(feature = "dev", test))]
impl SessionRecorder {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, note: None }
    }

    /// What the panel says under its controls.
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Starts `scenario` over, recording the player from its first tick. See the type's docs.
    #[cfg(test)]
    pub fn start(&mut self, combat: &mut Option<Combat>, scenario: &str) {
        self.start_tuned(combat, scenario, &LiveMoves::default());
    }

    /// Starts `scenario` over from `live`'s edits of its documents (M2 T2): a fight started after a
    /// save plays what was saved, and its file's hash says so.
    pub fn start_tuned(&mut self, combat: &mut Option<Combat>, scenario: &str, live: &LiveMoves) {
        if combat.is_some() {
            self.note = Some("A fight is running: end it before starting another.".to_owned());
            return;
        }
        match self.new_recording(scenario, live) {
            Ok(recording) => {
                *combat = Some(recording);
                self.note = None;
            }
            Err(error) => self.note = Some(error),
        }
    }

    fn new_recording(&self, scenario: &str, live: &LiveMoves) -> Result<Combat, String> {
        // Up front: failing here costs nothing, and failing at the end would cost the session.
        fs::create_dir_all(&self.dir).map_err(|error| format!("{}: {error}", self.dir.display()))?;
        let path = (1_u32..)
            .map(|number| self.dir.join(format!("{scenario}-{number}.sreplay")))
            .find(|path| !path.exists())
            .expect("fewer than 2³² recordings");
        Combat::recording(&path, scenario, None, live)
    }

    /// Ends a recording and writes it.
    pub fn save(&mut self, combat: &mut Option<Combat>) {
        let Some(recording) = take_recording(combat) else { return };
        let edited_at = recording.edited_at();
        let ticks = edited_at.map_or(recording.simulation().tick(), u64::from);
        let path = recording.path.clone();
        self.note = Some(match (recording.finish(), edited_at) {
            (Ok(()), None) => format!("Saved {ticks} ticks to {}", path.display()),
            (Ok(()), Some(_)) => {
                format!("Saved {ticks} ticks to {}: a live edit ended the file there", path.display())
            }
            (Err(error), _) => error,
        });
    }

    /// Ends a recording and writes nothing.
    pub fn discard(&mut self, combat: &mut Option<Combat>) {
        if let Some(recording) = take_recording(combat) {
            self.note = Some(format!("Discarded {} ticks", recording.simulation().tick()));
        }
    }
}

/// The recording of the player that's running, out of `combat`. A replay or a script's recording stays.
#[cfg(any(feature = "dev", test))]
fn take_recording(combat: &mut Option<Combat>) -> Option<Combat> {
    combat.take_if(|combat| matches!(combat.status(), CombatStatus::Recording { scripted: false, .. }))
}

/// Asks `bindings` for one fixed step's input, exactly once: a recording of the player asks them
/// itself, and otherwise this does, because asking is what moves a press along. `player` is who a
/// step with no recording names.
pub(crate) fn read_bindings(combat: Option<&mut Combat>, bindings: &mut dyn ActionSource, player: ActorId) {
    let asked = combat.is_some_and(|combat| combat.step(bindings));
    if !asked {
        bindings.actions(0, &[player]);
    }
}

/// A script by name.
fn script_actions(name: &str) -> Result<ScriptActions, String> {
    find_script(name).map(|script| script.actions).ok_or_else(|| {
        let known: Vec<&str> = SCRIPTS.iter().map(|script| script.name).collect();
        format!("--script: no script `{name}`; there's {known:?}")
    })
}

/// Rule R6, beside a renderer. The replay suite is the gate; here it's a finding to look at, since a
/// profiling build's zones may allocate where a plain one doesn't.
fn log_gate(played: &Played) {
    if let Err(error) = played.allocation_gate() {
        tracing::warn!(target: LOG_TARGET, %error, "the fixed step allocated");
    }
}

fn diverged(name: &str, path: &Path, difference: Divergence) -> String {
    format!(
        "--replay {}: `{name}` diverged from its recorded checksums on {ARCHITECTURE}: {difference}",
        path.display()
    )
}

/// What's recorded for a replay in the checksum file beside it, for this architecture. A file that's
/// there and doesn't read is an error; none at all is a replay outside the suite.
fn recorded_trail(path: &Path, name: &str) -> Result<Option<RecordedTrail>, String> {
    let file = path.with_file_name(checksums_file_name(ARCHITECTURE));
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", file.display())),
    };
    let checksums = ChecksumFile::parse(&text).map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(checksums.get(name).cloned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sapphire_gameplay::replay::{Replay, find_script};
    use sapphire_gameplay::{ActorActions, Buttons};
    use sapphire_input::RawInput;
    use sapphire_platform::{Scancode, Size, Timestamp};

    use super::*;
    use crate::input::SandboxInput;

    /// A file name of this process's own in the temp directory, deleted again when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let file = format!("sapphire-sandbox-{}-{name}.sreplay", std::process::id());
            let path = std::env::temp_dir().join(file);
            let _ = fs::remove_file(&path);
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn recording(path: &Path, script: Option<&str>) -> Combat {
        let option = CombatOption::Record {
            path: path.to_owned(),
            scenario: SCENARIOS[0].name.to_owned(),
            script: script.map(str::to_owned),
        };
        Combat::new(&option).expect("sets up")
    }

    /// A fight played on the keyboard, through the sandbox's own input, reaches the file and plays
    /// back to the same checksums.
    #[test]
    fn a_player_is_recorded_through_the_bindings() {
        let scratch = Scratch::new("played");
        let mut combat = recording(&scratch.0, None);
        let cast = SCENARIOS[0].start(SCENARIOS[0].config).expect("sets up").1;
        let mut input = SandboxInput::new(false, true, Size { width: 640, height: 360 }).expect("binds");
        let frames: [&[(Scancode, bool)]; 4] = [
            &[(Scancode::W, true)],
            &[(Scancode::J, true), (Scancode::J, false)],
            &[(Scancode::W, false)],
            &[],
        ];
        // Looking along world +X, so forward is +x in the file.
        let camera_yaw = std::f32::consts::FRAC_PI_2;
        for (index, keys) in frames.iter().enumerate() {
            let frame = index as u64 * 16;
            for (offset, &(scancode, down)) in keys.iter().enumerate() {
                input.push(frame + 1 + offset as u64, RawInput::Key { scancode, down });
            }
            assert!(!input.update(Timestamp::from_nanos((frame + 16) * 1_000_000)));
            assert!(combat.step(&mut input.source(camera_yaw)), "it asks the bindings");
        }
        combat.finish().expect("writes");

        let replay = Replay::decode(&fs::read(&scratch.0).expect("written")).expect("reads");
        let read: Vec<_> = replay.actions().map(|tick| tick.get(cast[0])).collect();
        let forward = sapphire_gameplay::Stick::new(127, 0);
        assert_eq!(
            read,
            [
                ActorActions { buttons: Buttons::NONE, stick: forward },
                ActorActions { buttons: Buttons::LIGHT, stick: forward },
                ActorActions::NEUTRAL,
                ActorActions::NEUTRAL,
            ]
        );

        let mut played = Combat::new(&CombatOption::Replay { path: scratch.0.clone(), to_end: true })
            .expect("a recording reads as a replay");
        for _ in 0..read.len() {
            assert!(!played.step(&mut no_bindings), "a replay asks no one");
        }
        played.finish().expect("nothing recorded to differ from");
    }

    /// However the step goes, the bindings are read once, so no press waits or is lost.
    #[test]
    fn every_step_reads_the_bindings_exactly_once() {
        let player = ActorId::from_raw(1).expect("not zero");
        let scratch = (Scratch::new("once-player"), Scratch::new("once-script"), Scratch::new("once-replay"));
        let script = Some(find_script("idle").expect("a script").name);
        let mut played = recording(&scratch.0.0, None);
        let mut scripted = recording(&scratch.1.0, script);
        let mut for_replay = recording(&scratch.2.0, script);
        for _ in 0..3 {
            for_replay.step(&mut no_bindings);
        }
        for_replay.finish().expect("writes");
        let replay = CombatOption::Replay { path: scratch.2.0.clone(), to_end: true };
        let mut replaying = Combat::new(&replay).expect("reads");

        let count = |combat: Option<&mut Combat>| {
            let mut asked = 0;
            let mut bindings = |_: u64, _: &[ActorId]| {
                asked += 1;
                TickActions::NEUTRAL
            };
            read_bindings(combat, &mut bindings, player);
            asked
        };
        assert_eq!(count(None), 1, "nothing's recording");
        assert_eq!(count(Some(&mut played)), 1, "a recording of the player");
        assert_eq!(count(Some(&mut scripted)), 1, "a script's, which asks them nothing");
        assert_eq!(count(Some(&mut replaying)), 1, "a replay");
    }

    fn fighting(script: Option<&str>) -> Combat {
        Combat::new(&CombatOption::Fight { script: script.map(str::to_owned), party: false })
            .expect("sets up")
    }

    /// What `--arena --fight` does in a window, short of the window: the keys go through the sandbox's
    /// own input to the same step, the player's verbs land, the brain thinks, nothing's written, and
    /// the key that starts it over makes the same first tick again.
    #[test]
    fn a_fight_is_played_through_the_bindings_and_starts_over_on_its_key() {
        let mut combat = fighting(None);
        assert_eq!(combat.scenario(), crate::arena::FIGHT_SCENARIO);
        crate::arena::check_scenario(&combat, false).expect("it's the arena's");
        assert!(crate::arena::check_scenario(&combat, true).is_err(), "it isn't a party's");
        let cast = combat.cast().to_vec();
        let at_start = combat.simulation().checksum();
        let enemy = |combat: &Combat| combat.simulation().actor(cast[1]).expect("alive");
        let player = |combat: &Combat| combat.simulation().actor(cast[0]).expect("alive");
        assert!(enemy(&combat).brain.is_on(), "someone's thinking for it");

        let mut input = SandboxInput::new(false, true, Size { width: 640, height: 360 }).expect("binds");
        input.listen_for_fight_restart().expect("binds");
        let mut now = 0;
        let mut frame = |input: &mut SandboxInput, combat: &mut Combat, keys: &[(Scancode, bool)]| {
            for &(scancode, down) in keys {
                now += 1;
                input.push(now, RawInput::Key { scancode, down });
            }
            now += 16;
            assert!(!input.update(Timestamp::from_nanos(now * 1_000_000)));
            let actions = input.take_actions();
            if actions.restart_fight {
                combat.restart();
            }
            read_bindings(Some(combat), &mut input.source(0.0), cast[0]);
        };

        frame(&mut input, &mut combat, &[(Scancode::J, true)]);
        let current = player(&combat).moves.current.expect("it started a move");
        assert_eq!(current.name, sapphire_core::Name::new("light_1"));
        frame(&mut input, &mut combat, &[(Scancode::J, false)]);
        for _ in 0..30 {
            frame(&mut input, &mut combat, &[]);
        }
        assert_ne!(enemy(&combat).position, sapphire_gameplay::arena::spawns().enemy.position(), "it walked");
        let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
        assert_eq!(fights, 1);

        frame(&mut input, &mut combat, &[(Scancode::R, true)]);
        // The key lands between frames, and that frame's step is the new fight's first.
        assert_eq!(combat.simulation().tick(), 1);
        let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
        assert_eq!(fights, 2);
        let mut fresh = fighting(None);
        fresh.step(&mut no_bindings);
        assert_eq!(combat.simulation().checksum(), fresh.simulation().checksum());
        assert_ne!(at_start, combat.simulation().checksum(), "a tick on");
        combat.finish().expect("nothing to write");
    }

    /// Whoever falls, it goes on long enough to see, starts over from the same first tick, and plays
    /// the same fight: nothing carries over. The script stands still, so the player falls.
    #[test]
    fn a_fight_starts_over_after_someone_falls() {
        let mut combat = fighting(Some("idle"));
        let cast = combat.cast().to_vec();
        let mut trail = vec![combat.simulation().checksum()];
        let fallen = loop {
            assert!(!combat.step(&mut no_bindings), "a script asks no one");
            trail.push(combat.simulation().checksum());
            let player = combat.simulation().actor(cast[0]).expect("alive");
            if player.fighter.is_defeated() {
                break combat.simulation().tick();
            }
            assert!(combat.simulation().tick() < 10_000, "it never fell");
        };
        for _ in 1..RESTART_AFTER {
            combat.step(&mut no_bindings);
            trail.push(combat.simulation().checksum());
        }
        assert_eq!(combat.simulation().tick(), fallen + RESTART_AFTER - 1, "still the same fight");
        combat.step(&mut no_bindings);
        let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
        assert_eq!(fights, 2);
        assert_eq!(combat.simulation().tick(), 0, "it starts over");

        for expected in &trail {
            assert_eq!(combat.simulation().checksum(), *expected, "at tick {}", combat.simulation().tick());
            combat.step(&mut no_bindings);
        }
        assert_eq!(combat.simulation().stats().allocations, 0, "rule R6");
    }

    /// `--fight --party`: the swap button works, through the same bindings' path, and the fight goes on past
    /// a member's fall, until the last one's, and then starts over as a party.
    #[test]
    fn a_partys_fight_goes_on_until_its_last_member_falls() {
        let option = CombatOption::Fight { script: Some("party-swap".to_owned()), party: true };
        let mut combat = Combat::new(&option).expect("sets up");
        assert_eq!(combat.scenario(), crate::arena::PARTY_SCENARIO);
        crate::arena::check_scenario(&combat, true).expect("it's the party's");
        assert!(crate::arena::check_scenario(&combat, false).is_err(), "it takes three looks");

        let mut member_fell = false;
        while !combat.simulation().party().is_down() {
            combat.step(&mut no_bindings);
            let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
            assert_eq!(fights, 1, "a member's fall doesn't end it");
            member_fell |= combat.simulation().stats().forced_swaps > 0;
            assert!(combat.simulation().tick() < 20_000, "it never fell");
        }
        assert!(member_fell && combat.simulation().stats().swaps > 3);
        for _ in 0..RESTART_AFTER {
            combat.step(&mut no_bindings);
        }
        let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
        assert_eq!(fights, 2);
        let party = combat.simulation().party();
        assert_eq!((party.active_slot(), party.is_down()), (Some(0), false), "as a party again");
    }

    #[test]
    fn a_fight_has_no_bad_scripts_and_only_it_starts_over() {
        let error = Combat::new(&CombatOption::Fight { script: Some("no-such".to_owned()), party: false });
        assert!(error.err().expect("refused").contains("no-such"));

        let scratch = Scratch::new("no-restart");
        let mut recording = recording(&scratch.0, Some("idle"));
        recording.step(&mut no_bindings);
        recording.restart();
        assert_eq!(recording.simulation().tick(), 1, "a recording has a file to keep to");
    }

    /// A directory of this test's own, gone when it ends.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sapphire-sandbox-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn press_light(_tick: u64, cast: &[ActorId]) -> TickActions {
        let mut actions = TickActions::NEUTRAL;
        let pressed = ActorActions { buttons: Buttons::LIGHT, ..ActorActions::NEUTRAL };
        actions.set(cast[0], pressed).expect("room");
        actions
    }

    /// What the overlay's Record button does: a fresh scenario, at its first tick, whatever came
    /// before, and the file holds only what came after. A fight that ran first leaves no mark on it.
    #[test]
    fn the_overlay_records_from_the_scenarios_first_tick() {
        let scratch = ScratchDir::new("from-the-start");
        let mut recorder = SessionRecorder::new(scratch.0.clone());
        let mut combat = None;
        let player = ActorId::from_raw(1).expect("not zero");

        // The session before: bindings read with nothing recording them.
        for _ in 0..5 {
            read_bindings(combat.as_mut(), &mut press_light, player);
        }
        recorder.start(&mut combat, "duel");
        let running = combat.as_ref().expect("started");
        assert_eq!(running.simulation().tick(), 0, "the scenario starts over");
        let CombatStatus::Recording { scenario, scripted, at_limit, path } = running.status() else {
            panic!("{:?}", running.status());
        };
        assert_eq!((scenario, scripted, at_limit), ("duel", false, false));
        assert_eq!(path, scratch.0.join("duel-1.sreplay"));

        for tick in 1..=3 {
            read_bindings(combat.as_mut(), &mut press_light, player);
            assert_eq!(combat.as_ref().expect("running").simulation().tick(), tick);
        }
        recorder.save(&mut combat);
        assert!(combat.is_none(), "it's over");
        assert!(recorder.note().is_some_and(|note| note.contains("Saved 3 ticks")), "{:?}", recorder.note());

        let replay =
            Replay::decode(&fs::read(scratch.0.join("duel-1.sreplay")).expect("written")).expect("reads");
        assert_eq!((replay.scenario(), replay.ticks()), ("duel", 3));

        // It plays back to what a recording made in one go reaches.
        let scenario = find_scenario(SCENARIOS, "duel").expect("there");
        let mut whole = Recording::new(scenario, scenario.config).expect("sets up");
        for _ in 0..3 {
            whole.step(&mut press_light).expect("short");
        }
        let (whole, played) = whole.finish();
        assert_eq!(replay, whole, "the same file, byte for byte");
        let mut player = Player::new(replay, SCENARIOS).expect("starts");
        player.run_to_end();
        assert_eq!(player.finish().trail, played.trail);
    }

    /// The rule I1 left for this: however a recording starts and ends, each step reads the bindings
    /// once, since asking is what moves a press along.
    #[test]
    fn the_bindings_are_read_once_a_step_as_a_recording_starts_and_ends() {
        let scratch = ScratchDir::new("once");
        let mut recorder = SessionRecorder::new(scratch.0.clone());
        let mut combat = None;
        let player = ActorId::from_raw(1).expect("not zero");
        let step = |combat: &mut Option<Combat>| {
            let mut asked = 0;
            let mut bindings = |_: u64, _: &[ActorId]| {
                asked += 1;
                TickActions::NEUTRAL
            };
            read_bindings(combat.as_mut(), &mut bindings, player);
            asked
        };
        assert_eq!(step(&mut combat), 1, "nothing's recording");
        recorder.start(&mut combat, "duel");
        assert_eq!(step(&mut combat), 1, "the recording's first step");
        assert_eq!(step(&mut combat), 1, "and its next");
        recorder.save(&mut combat);
        assert_eq!(step(&mut combat), 1, "saved");
        recorder.start(&mut combat, "melee");
        assert_eq!(step(&mut combat), 1, "another");
        recorder.discard(&mut combat);
        assert_eq!(step(&mut combat), 1, "discarded");
    }

    #[test]
    fn the_overlay_never_overwrites_a_session_and_a_discard_writes_nothing() {
        let scratch = ScratchDir::new("names");
        let mut recorder = SessionRecorder::new(scratch.0.clone());
        let mut combat = None;
        let saved = |number: u32| scratch.0.join(format!("melee-{number}.sreplay"));

        recorder.start(&mut combat, "melee");
        recorder.discard(&mut combat);
        assert!(combat.is_none());
        assert!(!saved(1).exists(), "discarded");
        assert!(recorder.note().is_some_and(|note| note.contains("Discarded")), "{:?}", recorder.note());

        recorder.start(&mut combat, "melee");
        recorder.save(&mut combat);
        let first = fs::read(saved(1)).expect("written");
        recorder.start(&mut combat, "melee");
        recorder.save(&mut combat);
        assert!(saved(2).exists());
        assert_eq!(fs::read(saved(1)).expect("still there"), first);
    }

    #[test]
    fn the_overlay_leaves_a_running_fight_alone_and_says_why_it_cant_start() {
        let scratch = ScratchDir::new("refused");
        let mut recorder = SessionRecorder::new(scratch.0.clone());
        let mut combat = None;
        recorder.start(&mut combat, "duel");
        for _ in 0..2 {
            combat.as_mut().expect("running").step(&mut no_bindings);
        }
        recorder.start(&mut combat, "melee");
        let running = combat.as_ref().expect("kept");
        assert_eq!(running.simulation().tick(), 2, "a session isn't thrown away for another");
        assert!(recorder.note().is_some_and(|note| note.contains("running")), "{:?}", recorder.note());
        recorder.discard(&mut combat);

        // Nothing but a recording of the player is the panel's to end.
        let script = Scratch::new("script");
        let mut scripted = Some(recording(&script.0, Some("idle")));
        recorder.save(&mut scripted);
        recorder.discard(&mut scripted);
        assert!(scripted.is_some(), "a script's recording ends with its capture");

        // A scenario this build lacks, and a directory it can't make.
        let mut none = None;
        recorder.start(&mut none, "no-such");
        assert!(none.is_none());
        assert!(recorder.note().is_some_and(|note| note.contains("no-such")), "{:?}", recorder.note());
        let file = scratch.0.join("a-file");
        fs::create_dir_all(&scratch.0).expect("makes");
        fs::write(&file, b"in the way").expect("writes");
        let mut blocked = SessionRecorder::new(file.join("replays"));
        blocked.start(&mut none, "duel");
        assert!(none.is_none());
        assert!(blocked.note().is_some(), "it says why");
    }

    #[test]
    fn a_recording_refuses_to_overwrite_and_names_what_it_cant_find() {
        let scratch = Scratch::new("refused");
        let refused = |path: &Path, scenario: &str, script: Option<&str>| {
            let option = CombatOption::Record {
                path: path.to_owned(),
                scenario: scenario.to_owned(),
                script: script.map(str::to_owned),
            };
            Combat::new(&option).err().expect("refused")
        };
        assert!(refused(&scratch.0, "no-such", None).contains("no-such"));
        assert!(refused(&scratch.0, SCENARIOS[0].name, Some("no-such")).contains("no-such"));
        fs::write(&scratch.0, b"a session someone played").expect("writes");
        assert!(refused(&scratch.0, SCENARIOS[0].name, None).contains("it exists"));
    }
}
