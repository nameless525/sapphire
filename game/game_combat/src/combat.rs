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
//! when the app quits, and as far as it's got whenever the app leaves the foreground
//! ([`Combat::keep_so_far`]), since a phone kills what's in the background without saying so and an
//! iPhone's app never quits. Only what came after the last of those is lost to a killed process.
//!
//! **A session nobody can play again survives a bad save** ([`SessionFiles`]), on both of those routes.
//! Each write goes beside the file first, through a part of its own that a killed process leaves for
//! someone to find, so a kill in the middle of one leaves the last whole file standing, and it replaces
//! only what this session put there. A file that can't be written, for a
//! full disk, a directory gone since the run began or someone else's file at the name, sends the session
//! to one file in the temp directory instead, and says where. If that fails too, the overlay's recorder
//! goes on recording and asks to be tried again. Only when the app is already quitting is what came after
//! the last file lost, and that error says so.
//!
//! **A game's run** ([`CombatOption::Run`], M3 G1) is a recording in memory that writes a file only if it's
//! kept, through the same [`SessionFiles`], and goes on past a file's limit on a run's length, unrecorded
//! from there (`Recording::go_on_past_limit`). The flow's beats start timelines on it
//! ([`Combat::play_timeline`]) and a skip ends them ([`Combat::skip_timeline`]), both recorded, so a kept run
//! replays like any file.
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
//! that starts it over does the same at once ([`Combat::restart`]). A new fight has the state of a new
//! [`Simulation`], from the same first tick: nothing carries over, and a script plays it exactly as it played the one
//! before. That's what the app's own test leans on (`tests/replays.rs`): each fight's checksum in the log
//! is the one a headless run of the same script reaches at that tick. Only the player's hands outlast a
//! fight, so a button still down from the last one reads as up until it lets go. Immutable arena inputs
//! are prepared once per session. Its sealed built-in world resets complete gameplay state into retained
//! storage, so automatic restart stays inside the allocation-free public step. A script plays one in a
//! window too, so a phone can fight for as long as exit test 4 asks with no one holding it (M2 Z1).
//!
//! **Zones (M3 W4):** a replay names the zone entries it recorded, and [`Combat::due_zone_entry`] says one's
//! due before the next tick, so a show can cover the load first; [`Combat::enter_zone`] makes it, between two
//! steps. A recording enters zones its scenario put on the shelf, and records it. A fight with no file takes
//! any zone it's given, and a new fight starts in the last it entered, at the same door
//! ([`FightWorld::enter_zone`]).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sapphire_assets::AssetId;
use sapphire_dialogue::ConditionId;
use sapphire_gameplay::replay::{
    ARCHITECTURE, ActionSource, ChecksumFile, Divergence, FightWorld, KindOfEnemy, LiveMoves, Played, Player,
    PreparedFight, RecordedTrail, Recording, Replay, ReplayCommand, ReplayError, SCRIPTS, Scenario,
    ScenarioError, checksums_file_name, find_scenario, find_script,
};
use sapphire_gameplay::zone::{ZoneEntry, ZoneGround};
use sapphire_gameplay::{ActorId, Buttons, DialogueEffect, Simulation, TickActions, TimelineId};
#[cfg(feature = "dev")]
use sapphire_gameplay::{MoveSetId, Tuned};
use sapphire_script::ConditionSet;
/// Every scenario a fight here can start from, those with a script among them.
pub use sapphire_script::scenarios::ALL as SCENARIOS;
use sapphire_serialize::cooked::AlignedVec;

use crate::LOG_TARGET;

/// A script's actions: a function of the tick.
type ScriptActions = fn(u64, &[ActorId]) -> TickActions;

/// Whether `cast`'s player has fallen in `sim`, or all there was to fight has. A party falls with its last
/// member: until then, a fall only brings in the next.
fn is_over(sim: &Simulation, cast: &[ActorId]) -> bool {
    let party = sim.party();
    let Some(&first) = cast.first() else { return false };
    if party.input_actor() == Some(first) {
        return party.is_down() || foes_have_fallen(sim, party.on_field(first));
    }
    foes_have_fallen(sim, first) || sim.actor(first).is_some_and(|player| player.fighter.is_defeated())
}

/// Whether anyone stood against `player`, and all of them have fallen.
fn foes_have_fallen(sim: &Simulation, player: ActorId) -> bool {
    let Some(side) = sim.actor(player).map(|player| player.fighter.faction()) else { return false };
    let mut felled = false;
    for actor in sim.actors() {
        if actor.id == player || actor.off_field || !actor.fighter.is_fighter() {
            continue;
        }
        if actor.fighter.is_foe_of(side) {
            return false;
        }
        felled |= actor.fighter.faction().is_hostile_to(side);
    }
    felled
}

/// How long a fight goes on after someone falls before it starts over, in ticks: long enough to see
/// them fall. The warrior's death lasts 150.
pub(crate) const RESTART_AFTER: u64 = 240;

/// A combat simulation stepping beside the scene: see this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CombatOption {
    /// `--replay <file>`, with `--capture` or `--bench`: play it, and check it against the recorded
    /// checksums beside it.
    Replay {
        path: PathBuf,
        /// A capture with no `--capture-ticks` stops at the replay's end, which isn't known yet.
        to_end: bool,
    },
    /// `--record <file> --scenario <name>`: run the scenario and write the replay. With `--script
    /// <name>` and `--capture` or `--bench`, on the script's actions. In the windowed app, on the
    /// actions of whoever's playing, so it's `None` there.
    Record { path: PathBuf, scenario: String, script: Option<String> },
    /// `--arena --fight`: player against brain, with no file, starting over when someone falls. In the
    /// windowed app, on the actions of whoever's playing, so it's `None` there. With `--script <name>`
    /// and `--capture`, `--bench` or `--offscreen`, on the script's.
    Fight { script: Option<String>, party: bool },
    /// A game's run (M3 G1): the scenario from its start, recorded in memory in case it's kept, and written
    /// to `keep` only if there's one, when the run ends. It goes on past a file's limit on a run's length,
    /// unrecorded from there, and its timelines' plays and skips are recorded commands, as a recording's
    /// are. On the actions of whoever's playing, or with `script` and `--capture`, `--bench` or
    /// `--offscreen`, on the script's.
    Run { scenario: String, script: Option<String>, keep: Option<PathBuf> },
}

/// The frames a capture draws, the last of which it writes: one by default. Temporal AA needs
/// earlier frames to build its history from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureFrames {
    /// `--capture-frames`: how many.
    pub count: u32,
    /// `--capture-step`: fixed steps between consecutive frames. 0 draws them all at the
    /// capture's tick (the scene holds still while TAA converges); 1 plays the scene at 60 fps
    /// up to it.
    pub step: u32,
}

impl Default for CaptureFrames {
    fn default() -> Self {
        Self { count: 1, step: 0 }
    }
}

impl CaptureFrames {
    /// The ticks to simulate before the first frame, so the last lands on tick `ticks`.
    pub fn first_tick(&self, ticks: u32) -> Option<u32> {
        let intervals = self.count.checked_sub(1)?;
        ticks.checked_sub(self.step.checked_mul(intervals)?)
    }
}

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
pub fn no_bindings(_tick: u64, _cast: &[ActorId]) -> TickActions {
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
        /// The script it plays. With none, it records the bindings it's given.
        script: Option<ScriptActions>,
        /// The tick limit, once reached.
        stopped: Option<ReplayError>,
        /// The files its session has reached.
        files: SessionFiles,
        /// The timelines its script starts, each at the boundary before the step it names (M3 Q3).
        starts: ScriptStarts,
        /// What it's for: a file, or a game's run, which may keep none.
        kind: Kind,
    },
    /// `--fight`: played, and kept nowhere.
    Fighting(LiveFight),
}

/// What a recording is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// `--record`: a file, which stops at its limit.
    File,
    /// A game's run ([`CombatOption::Run`]), which goes on past it, and writes a file only if it `kept` one.
    Run { kept: bool },
}

impl Kind {
    /// Whether the session has a file to write.
    fn has_file(self) -> bool {
        matches!(self, Self::File | Self::Run { kept: true })
    }
}

/// A fight with no file: see the [module docs](self).
struct LiveFight {
    scenario: &'static Scenario,
    #[cfg(feature = "dev")]
    prepared: Box<PreparedFight>,
    /// The edits it's taken: what a transactional startup replacement starts from.
    #[cfg(feature = "dev")]
    live: LiveMoves,
    sim: FightWorld,
    /// The script it plays. With none, it plays the bindings it's given.
    script: Option<ScriptActions>,
    /// The tick someone fell at.
    fell_at: Option<u64>,
    /// How many fights there have been, this one included.
    fights: u32,
    /// The player's buttons as the bindings last read them.
    held: Buttons,
    /// What of them was down as this fight began and hasn't let go since.
    held_over: Buttons,
}

/// A zone's cooked bytes, as the asset server held them, to build its simulation's copy from.
struct ZoneCopy {
    asset: AssetId,
    pack: String,
    arrive: u64,
    /// The zone's own and what it references, each 16-byte aligned.
    bytes: Vec<(AssetId, AlignedVec)>,
}

impl ZoneCopy {
    /// Builds the zone from `read`, keeping what it read.
    fn read<'a>(
        asset: AssetId,
        pack: &str,
        arrive: u64,
        mut read: impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<Self, String> {
        let mut take = |id: AssetId| {
            let found = read(id)?;
            let mut aligned = AlignedVec::with_capacity(found.len());
            aligned.extend_from_slice(found);
            Some(aligned)
        };
        let zone = take(asset).ok_or_else(|| format!("zone {asset} isn't loaded"))?;
        let wanted = sapphire_zone::Zone::read(&zone).map_err(|error| error.to_string())?.references();
        let mut bytes = vec![(asset, zone)];
        for id in wanted {
            let held = take(id).ok_or_else(|| format!("zone {asset}'s {id} isn't loaded"))?;
            bytes.push((id, held));
        }
        Ok(Self { asset, pack: pack.to_owned(), arrive, bytes })
    }

    fn build(&self) -> Result<ZoneGround, String> {
        let find =
            |id: AssetId| self.bytes.iter().find(|(held, _)| *held == id).map(|(_, bytes)| bytes.as_slice());
        let zone = find(self.asset).ok_or("it holds its own bytes")?;
        ZoneGround::from_cooked(self.asset, &self.pack, zone, find).map_err(|error| error.to_string())
    }
}

/// Enters `copy`'s zone in `sim`, putting it on the shelf if it isn't, with the player going: from then on
/// a new fight starts there.
fn enter_copy(sim: &mut FightWorld, copy: &ZoneCopy) -> Result<(), String> {
    let id = match sim.find_zone(copy.asset) {
        Some(id) => id,
        None => sim.add_zone(copy.build()?),
    };
    let row = sim.zone_ground(id).ok_or("it was just found")?;
    let spawner = row
        .spawner(copy.arrive)
        .ok_or_else(|| format!("zone {} has no spawner {:#x}", copy.asset, copy.arrive))?;
    sim.enter_zone(ZoneEntry { zone: id, spawner }).map_err(|error| error.to_string())
}

impl LiveFight {
    fn new(
        scenario: &'static Scenario,
        script: Option<ScriptActions>,
        live: LiveMoves,
        party: bool,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, String> {
        let prepared = PreparedFight::new(party).map_err(|error| format!("--fight: {error}"))?;
        let sim = prepared
            .start_resettable_with_setup(scenario.config, &live, |sim, cast| {
                setup(scenario, sim, cast).map_err(ScenarioError::RuntimeProfiles)
            })
            .map_err(|error| format!("--fight: {error}"))?;
        Ok(Self {
            scenario,
            #[cfg(feature = "dev")]
            prepared: Box::new(prepared),
            #[cfg(feature = "dev")]
            live,
            sim,
            script,
            fell_at: None,
            fights: 1,
            held: Buttons::NONE,
            held_over: Buttons::NONE,
        })
    }

    /// One tick, and whatever it ended. Returns whether it asked `bindings`.
    fn step(&mut self, bindings: &mut dyn ActionSource) -> bool {
        let asks = self.script.is_none();
        let mut actions = action_source(&mut self.script, bindings).actions(self.sim.tick(), self.sim.cast());
        if asks {
            self.hide_held_over(&mut actions);
        }
        self.sim.step(&actions);
        let tick = self.sim.tick();
        match self.fell_at {
            None if self.is_over() => self.fell_at = Some(tick),
            Some(at) if tick - at >= RESTART_AFTER => self.start_over(),
            _ => {}
        }
        asks
    }

    /// Takes out of a played tick what's been held since before this fight. A simulation reads a press as
    /// a button down that wasn't on the tick before, and a new one has no tick before, so to it a button
    /// held across the restart is a press nobody made. The stick is a level, so a thumb walks straight on.
    fn hide_held_over(&mut self, actions: &mut TickActions) {
        let Some(&player) = self.sim.cast().first() else { return };
        let mut read = actions.get(player);
        // Which device played isn't a button held, and a thumb goes on being one.
        self.held = read.buttons.minus(Buttons::TOUCH);
        // What's in `held_over` and still down: `Buttons` has no intersection.
        self.held_over = self.held_over.minus(self.held_over.minus(read.buttons));
        read.buttons = read.buttons.minus(self.held_over);
        actions.set(player, read).expect("the bindings just gave this actor a place");
    }

    /// Whether the player's fallen, or all there was to fight has ([`is_over`]).
    fn is_over(&self) -> bool {
        is_over(&self.sim, self.sim.cast())
    }

    /// Reports this fight and makes the next. If it can't, because a live edit took something the scenario
    /// needs, this one goes on and it tries again as long after, so a save that puts it back is taken.
    fn start_over(&mut self) {
        self.report();
        self.fell_at = None;
        match self.sim.restart() {
            Ok(()) => {
                self.fights += 1;
                self.held_over = self.held;
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
            let id = self.sim.cast().get(index).map(|id| self.sim.party().on_field(*id));
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
pub struct Combat {
    /// The replay file, read or to write. A fight with no file has none.
    path: PathBuf,
    mode: Mode,
    /// Fixed steps taken after a replay's last tick.
    steps_past_end: u64,
    last_step_allocations: u64,
}

impl Combat {
    /// Reads what `option` names and sets up the world, before any GPU work, so a bad file costs
    /// nothing and says why.
    pub fn new(option: &CombatOption) -> Result<Self, String> {
        Self::from_live(option, &LiveMoves::default())
    }

    /// [`Self::new`], starting from `live` rather than only what the build compiled in: a game's targeting
    /// document, and in a dev build whatever's been edited.
    pub fn from_live(option: &CombatOption, live: &LiveMoves) -> Result<Self, String> {
        Self::from_live_with_setup(option, live, None, |_, _, _| Ok(()))
    }

    /// `option` from `live`, set up by `setup` before its first tick. A fight takes `edits` in place of `live`,
    /// if they're given: what a build with those edits in its documents would fight, and its own from here, so
    /// whatever made them can go on changing. Only a [`CombatOption::Fight`] can: the others are their files'.
    pub(crate) fn from_live_with_setup(
        option: &CombatOption,
        live: &LiveMoves,
        edits: Option<LiveMoves>,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, String> {
        match option {
            CombatOption::Fight { script, party } => {
                Self::fighting(script.as_deref(), *party, edits.unwrap_or_else(|| live.clone()), setup)
            }
            _ if edits.is_some() => Err("only a fight starts from edits".to_owned()),
            CombatOption::Replay { path, .. } => Self::replaying(path, live, setup),
            CombatOption::Record { path, scenario, script } => {
                Self::recording_with_setup(path, scenario, script.as_deref(), live, setup)
            }
            CombatOption::Run { scenario, script, keep } => {
                Self::running(scenario, script.as_deref(), keep.as_deref(), live, setup)
            }
        }
    }

    /// A game's run: see [`CombatOption::Run`]. A file to keep it in takes the next free name, as a session
    /// someone plays does: a run is never refused for a file left from the last.
    fn running(
        scenario: &str,
        script: Option<&str>,
        keep: Option<&Path>,
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, String> {
        let path = match keep {
            Some(path)
                if path.parent().is_some_and(|parent| !parent.as_os_str().is_empty() && !parent.is_dir()) =>
            {
                return Err(format!("a run's file {}: its directory doesn't exist", path.display()));
            }
            Some(path) => free_name(path),
            None => PathBuf::new(),
        };
        let kind = Kind::Run { kept: keep.is_some() };
        Self::record(path, scenario, script, live, setup, kind).map_err(|error| format!("a run: {error}"))
    }

    fn fighting(
        script: Option<&str>,
        party: bool,
        moves: LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, String> {
        let name = if party { crate::arena::PARTY_SCENARIO } else { crate::arena::FIGHT_SCENARIO };
        let scenario =
            find_scenario(&SCENARIOS, name).ok_or_else(|| format!("--fight: no scenario `{name}`"))?;
        let fight = LiveFight::new(scenario, script.map(script_actions).transpose()?, moves, party, setup)?;
        Ok(Self {
            path: PathBuf::new(),
            mode: Mode::Fighting(fight),
            steps_past_end: 0,
            last_step_allocations: 0,
        })
    }

    fn replaying(
        path: &Path,
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, String> {
        let fail = |error: &dyn std::fmt::Display| format!("--replay {}: {error}", path.display());
        let bytes = fs::read(path).map_err(|error| fail(&error))?;
        let replay = Replay::decode(&bytes).map_err(|error| fail(&error))?;
        let player =
            Player::tuned_with_setup(replay, &SCENARIOS, live, setup).map_err(|error| fail(&error))?;
        let name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_owned();
        let recorded = recorded_trail(path, &name).map_err(|error| fail(&error))?;
        let mode = Mode::Replaying { player, name, recorded };
        Ok(Self { path: path.to_owned(), mode, steps_past_end: 0, last_step_allocations: 0 })
    }

    fn recording_with_setup(
        path: &Path,
        scenario: &str,
        script: Option<&str>,
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Self, String> {
        // Up front rather than after the run: it may be a session someone played, which no script
        // makes again. A script's run can come again, and what started it reads the name it gave. For
        // someone who's come to play, a name left from last time is no reason to stop, and on a phone,
        // where the only sign is the app closing, stopping looks like a crash.
        let free = match script {
            Some(_) if path.exists() => {
                return Err(format!("--record {}: it exists; delete it first to replace it", path.display()));
            }
            Some(_) => path.to_owned(),
            None => free_name(path),
        };
        if free != path {
            tracing::warn!(
                target: LOG_TARGET,
                taken = %path.display(),
                path = %free.display(),
                "the recording's name is taken, so this session has the next free one",
            );
        }
        let path = free.as_path();
        if path.parent().is_some_and(|parent| !parent.as_os_str().is_empty() && !parent.is_dir()) {
            return Err(format!("--record {}: its directory doesn't exist", path.display()));
        }
        Self::record(free, scenario, script, live, setup, Kind::File)
    }

    /// A recording of `scenario` for `kind`, to `path`, which is free to write.
    fn record(
        path: PathBuf,
        scenario: &str,
        script: Option<&str>,
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
        kind: Kind,
    ) -> Result<Self, String> {
        let Some(scenario) = find_scenario(&SCENARIOS, scenario) else {
            let known: Vec<&str> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
            return Err(format!("--scenario: no scenario `{scenario}`; there's {known:?}"));
        };
        let starts = ScriptStarts::new(script.and_then(find_script).map_or(&[], |script| script.commands));
        let script = script.map(script_actions).transpose()?;
        let mut recording = Recording::tuned_with_setup(scenario, scenario.config, live, setup)
            .map_err(|error| format!("--record: {error}"))?;
        if matches!(kind, Kind::Run { .. }) {
            recording = recording.go_on_past_limit();
        }
        starts.clone().apply(&mut recording).map_err(|error| format!("--record: {error}"))?;
        let files = SessionFiles::new(std::env::temp_dir());
        let mode =
            Mode::Recording { recording, script, stopped: None, files, starts: starts.after_start(), kind };
        Ok(Self { path, mode, steps_past_end: 0, last_step_allocations: 0 })
    }

    /// The scenario the run starts from.
    pub fn scenario(&self) -> &str {
        match &self.mode {
            Mode::Replaying { player, .. } => player.replay().scenario(),
            Mode::Recording { recording, .. } => recording.scenario(),
            Mode::Fighting(fight) => fight.scenario.name,
        }
    }

    /// The set that plays the document with this file name, in this fight (M2 T2): the first, when a second
    /// kind of enemy plays a copy of it (M3 B3b).
    #[cfg(feature = "dev")]
    pub(crate) fn plays(&self, document: &str) -> Option<MoveSetId> {
        find_scenario(&SCENARIOS, self.scenario())?.set_of(document)
    }

    /// Every set that plays the document with this file name, in this fight: a live edit goes to each.
    #[cfg(feature = "dev")]
    fn sets_playing(&self, document: &str) -> Vec<MoveSetId> {
        find_scenario(&SCENARIOS, self.scenario())
            .map_or(Vec::new(), |scenario| scenario.sets_of(document).collect())
    }

    /// Gives this fight a live edit of the document with this file name, between steps: what the set
    /// that plays it did with it, if one does. A replay refuses, since it plays what it recorded and
    /// [`Self::check`] would fail it for anything else. A recording ends its file at the first edit
    /// that changes anything.
    #[cfg(feature = "dev")]
    pub(crate) fn tune(
        &mut self,
        document: &str,
        set: sapphire_moves::MoveSet,
    ) -> Result<Option<(MoveSetId, Tuned)>, String> {
        if self.simulation().overhead_presentation_pair().is_some() {
            return Err("live move editing requires an unconfigured fight".into());
        }
        let sets = self.sets_playing(document);
        let Some((&id, copies)) = sets.split_first() else { return Ok(None) };
        match &mut self.mode {
            Mode::Replaying { name, .. } => {
                Err(format!("`{name}` is a replay, which plays what it recorded: it takes no live edits"))
            }
            Mode::Recording { recording, .. } => {
                let ended = recording.edited_at();
                for &copy in copies {
                    recording.tune_move_set(copy, set.clone());
                }
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
            Mode::Fighting(fight) => {
                for &copy in copies {
                    fight.sim.tune_move_set(copy, set.clone());
                }
                Ok(fight.tune(document, id, set).map(|tuned| (id, tuned)))
            }
        }
    }

    /// Gives this fight's script a new source, between steps, keeping its board (M3 L3). A replay refuses,
    /// as it does any edit. A recording ends its file if it changes anything, and a fight starts over on the
    /// newest.
    #[cfg(feature = "dev")]
    pub(crate) fn reload_script(
        &mut self,
        source_name: &str,
        source: &str,
    ) -> Result<sapphire_script::Reloaded, String> {
        let reload = |sim: &mut Simulation| {
            sapphire_script::reload(sim, source_name, source).map_err(|error| error.to_string())
        };
        match &mut self.mode {
            Mode::Replaying { name, .. } => {
                Err(format!("`{name}` is a replay, which plays what it recorded: it takes no live edits"))
            }
            Mode::Recording { recording, .. } => {
                let ended = recording.edited_at();
                let reloaded = recording.edit(reload)?;
                if let (None, Some(tick)) = (ended, recording.edited_at()) {
                    tracing::warn!(
                        target: LOG_TARGET,
                        script = source_name,
                        tick,
                        "a live edit ends the recording's file at this tick: the fight goes on, unrecorded",
                    );
                }
                Ok(reloaded)
            }
            // A fight with no file is the built-in arena or party, whose world takes no systems of its own,
            // a script host's included.
            Mode::Fighting(fight) => Err(format!(
                "`{}` runs no script: a fight with no file is the built-in arena or party, and `--record {}` \
                 is the fight a script's edit reaches",
                fight.scenario.name,
                sapphire_script::scenarios::SCRIPTED[0].name,
            )),
        }
    }

    /// Gives this fight new targeting weights, between steps. A replay refuses, as it does any edit: a file
    /// on other weights wouldn't start. A recording ends its file if they change anything.
    #[cfg(feature = "dev")]
    pub(crate) fn tune_targeting(
        &mut self,
        weights: sapphire_gameplay::targeting::TargetingTuning,
    ) -> Result<(), String> {
        match &mut self.mode {
            Mode::Replaying { name, .. } => {
                Err(format!("`{name}` is a replay, which plays what it recorded: it takes no live edits"))
            }
            Mode::Recording { recording, .. } => {
                recording.tune_targeting(weights);
                Ok(())
            }
            Mode::Fighting(fight) => {
                fight.sim.tune_targeting(weights);
                fight.live.set_targeting(weights);
                Ok(())
            }
        }
    }

    /// Starts a fight that hasn't stepped from `set`, if that isn't what it has: for what a cook made of
    /// this document, when it's newer than the simulation's own copy. Says whether it did. Nothing else
    /// has anywhere to start from: a replay plays what it recorded, and a recording's file says what it
    /// ran on.
    #[cfg(feature = "dev")]
    pub(crate) fn start_fight_from(&mut self, document: &str, set: sapphire_moves::MoveSet) -> bool {
        if self.simulation().overhead_presentation_pair().is_some() {
            return false;
        }
        let Some(id) = self.plays(document) else { return false };
        let Mode::Fighting(fight) = &mut self.mode else { return false };
        // A world in a zone keeps what it has, since a fresh one would start outside it.
        if fight.sim.tick() != 0
            || fight.sim.door().is_some()
            || fight.sim.move_library().is_current(id, &set)
        {
            return false;
        }
        let mut live = fight.live.clone();
        live.remember(document, set);
        match fight.prepared.start_resettable(fight.scenario.config, &live) {
            Ok(sim) => (fight.sim, fight.live) = (sim, live),
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

    /// Whether it plays a file: what it does between steps is the file's, so nothing else may.
    pub fn is_replay(&self) -> bool {
        matches!(self.mode, Mode::Replaying { .. })
    }

    /// Asks one of a dialogue's bound conditions of this run's VM, between steps, read-only (M3 Q2b): false
    /// for a fight with no file, which loads no dialogue, and wherever there's no VM.
    pub fn ask_condition(&mut self, conditions: &mut ConditionSet, id: ConditionId) -> bool {
        match &mut self.mode {
            Mode::Replaying { player, .. } => sapphire_script::ask(player, conditions, id),
            Mode::Recording { recording, .. } => sapphire_script::ask(recording, conditions, id),
            Mode::Fighting(_) => false,
        }
    }

    /// Applies a dialogue's effect between steps, through the run's command route (M3 Q2b): a recording
    /// records it as `ReplayCommand::Dialogue`, as it does its script's, so the choice replays. A replay
    /// applies only what it recorded, and a fight with no file loads no dialogue, so both refuse.
    pub fn apply_dialogue_effect(&mut self, effect: DialogueEffect) -> Result<(), String> {
        match &mut self.mode {
            Mode::Recording { recording, stopped: None, .. } => {
                recording.apply_dialogue_effect(effect).map_err(|error| error.to_string())
            }
            Mode::Recording { .. } => Err("the recording has reached its limit".to_owned()),
            Mode::Replaying { .. } => Err("a replay applies only the effects it recorded".to_owned()),
            Mode::Fighting(_) => Err("a fight with no file loads no dialogue".to_owned()),
        }
    }

    /// Whether someone's playing this through the bindings, rather than a file or a script driving it.
    pub fn is_played(&self) -> bool {
        match &self.mode {
            Mode::Replaying { .. } => false,
            Mode::Recording { script, .. } => script.is_none(),
            Mode::Fighting(fight) => fight.script.is_none(),
        }
    }

    /// The scenario's kinds of enemy, and the sets their fighters play: what the stage draws each as
    /// ([`crate::fight::FightShow::load_kinds`]).
    pub fn kinds_of_enemy(&self) -> Result<Vec<KindOfEnemy>, String> {
        let scenario = find_scenario(&SCENARIOS, self.scenario());
        let scenario = scenario.ok_or_else(|| format!("no scenario `{}`", self.scenario()))?;
        scenario.kinds_of_enemy().map_err(|error| error.to_string())
    }

    /// The actors the scenario's input drives, the player first.
    pub fn cast(&self) -> &[ActorId] {
        match &self.mode {
            Mode::Replaying { player, .. } => player.cast(),
            Mode::Recording { recording, .. } => recording.cast(),
            Mode::Fighting(fight) => fight.sim.cast(),
        }
    }

    /// How many ticks a replay has left: nothing for a fight or a recording, which have no end of their
    /// own.
    pub fn ticks_left(&self) -> Option<u64> {
        match &self.mode {
            Mode::Replaying { player, .. } => {
                Some(u64::from(player.replay().ticks()).saturating_sub(player.simulation().tick()))
            }
            Mode::Recording { .. } | Mode::Fighting(_) => None,
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
        let scope = sapphire_core::alloc::AllocScope::begin();
        let asks = self.step_inner(bindings);
        self.last_step_allocations = scope.counts().allocations;
        asks
    }

    /// Allocations in the complete last public step, including an automatic restart's reset.
    /// As with simulation counters, this requires an installed counting allocator.
    pub fn last_step_allocations(&self) -> u64 {
        self.last_step_allocations
    }

    fn step_inner(&mut self, bindings: &mut dyn ActionSource) -> bool {
        match &mut self.mode {
            Mode::Replaying { player, .. } => {
                if !player.step() {
                    self.steps_past_end += 1;
                }
                false
            }
            Mode::Recording { recording, script, stopped, starts, .. } => {
                if stopped.is_some() {
                    return false;
                }
                let asks = script.is_none();
                if let Err(error) = recording.step(action_source(script, bindings)) {
                    *stopped = Some(error);
                } else if let Err(error) = starts.apply(recording) {
                    tracing::warn!(target: LOG_TARGET, %error, "the script's command didn't take");
                }
                asks
            }
            Mode::Fighting(fight) => fight.step(bindings),
        }
    }

    /// Starts a timeline before the next step, as a beat of a game's flow does: a recording, a game's run
    /// among them, records it, and the stage plays it as it plays any the simulation starts. A replay plays
    /// only its own commands, and a fight with no file plays no timelines.
    pub fn play_timeline(&mut self, id: TimelineId) -> Result<(), String> {
        match &mut self.mode {
            Mode::Recording { recording, stopped: None, .. } => {
                recording.play_timeline(id).map_err(|error| error.to_string())
            }
            Mode::Recording { .. } => Err("the recording has stopped at its limit".to_owned()),
            Mode::Replaying { .. } => Err("a replay plays only the timelines it recorded".to_owned()),
            Mode::Fighting(_) => Err("a fight with no file plays no timelines".to_owned()),
        }
    }

    /// Someone skipped the timeline playing, `at_ms` into it, between steps: a recording records it, and runs
    /// the steps a cinematic held, as watching would have. A replay's skip is presentation's alone, and a fight
    /// with no file plays no timelines yet.
    pub fn skip_timeline(&mut self, at_ms: u32) {
        if let Mode::Recording { recording, stopped: None, .. } = &mut self.mode
            && let Err(error) = recording.skip_timeline(at_ms)
        {
            tracing::warn!(target: LOG_TARGET, %error, "the skip wasn't recorded");
        }
    }

    /// The zone entry a replay has due before its next tick, for a show to cover the load before
    /// [`Self::enter_zone`]. Nothing for a fight or a recording, whose entries are their show's to start. A
    /// step taken with one due enters it at once.
    pub fn due_zone_entry(&mut self) -> Option<ZoneEntry> {
        match &mut self.mode {
            Mode::Replaying { player, .. } => player.due_zone_entry(),
            Mode::Recording { .. } | Mode::Fighting(_) => None,
        }
    }

    /// Enters the zone `asset`, in pack `pack`, at its spawner `arrive`, between two steps, with the cast's
    /// first travelling: a replay the entry it has due, whatever it's given; a recording a zone its
    /// scenario's shelf has, which it records; a fight with no file any zone, which `read` gives the cooked
    /// bytes of, and what they reference, as the asset server holds them. An error says why it couldn't, or,
    /// for a replay, that it did and the content isn't what it recorded: a stale cook.
    pub fn enter_zone<'a>(
        &mut self,
        asset: AssetId,
        pack: &str,
        arrive: u64,
        read: impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<(), String> {
        match &mut self.mode {
            Mode::Replaying { player, .. } => {
                let entry = player.due_zone_entry().ok_or("the replay has no zone entry due")?;
                let recorded = player.simulation().zone_ground(entry.zone).map(ZoneGround::digest);
                player.enter_due_zone().map_err(|error| error.to_string())?;
                let built = ZoneCopy::read(asset, pack, arrive, read).and_then(|copy| copy.build())?;
                if recorded != Some(built.digest()) {
                    return Err(format!(
                        "zone {asset} isn't what the replay recorded: run `cargo xtask content`, or if its \
                         documents changed, take new copies (`zone_baked`'s docs)"
                    ));
                }
                Ok(())
            }
            Mode::Recording { recording, .. } => {
                let sim = recording.simulation();
                let id = sim.find_zone(asset).ok_or_else(|| {
                    format!("zone {asset} isn't on scenario `{}`'s shelf", recording.scenario())
                })?;
                let row = sim.zone_ground(id).ok_or("it was just found")?;
                let spawner =
                    row.spawner(arrive).ok_or_else(|| format!("zone {asset} has no spawner {arrive:#x}"))?;
                recording.enter_zone(ZoneEntry { zone: id, spawner }).map_err(|error| error.to_string())
            }
            Mode::Fighting(fight) => {
                let copy = ZoneCopy::read(asset, pack, arrive, read)?;
                enter_copy(&mut fight.sim, &copy)
            }
        }
    }

    /// Starts a fight with no file over, between steps. Nothing else has anywhere to start over from.
    pub fn restart(&mut self) {
        if let Mode::Fighting(fight) = &mut self.mode {
            fight.start_over();
        }
    }

    /// All the time its scripts have taken so far, hooks and collection, or `None` for a fight with no script
    /// host: what a bench reads §3.2's script row from (M3 Z1).
    pub fn script_time(&self) -> Option<std::time::Duration> {
        let host = self.simulation().world().get_resource::<sapphire_script::ScriptHost>()?;
        let stats = host.stats();
        Some(stats.hooks_time + stats.collections_time)
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
    pub fn status(&self) -> CombatStatus<'_> {
        match &self.mode {
            Mode::Replaying { player, name, .. } => {
                CombatStatus::Replaying { name, length: player.replay().ticks() }
            }
            Mode::Recording { recording, kind: Kind::Run { kept }, .. } => CombatStatus::Running {
                scenario: recording.scenario(),
                kept: kept.then_some(self.path.as_path()),
            },
            Mode::Recording { recording, script, stopped, .. } => CombatStatus::Recording {
                scenario: recording.scenario(),
                path: &self.path,
                at_limit: stopped.is_some(),
                scripted: script.is_some(),
            },
            Mode::Fighting(fight) => CombatStatus::Fighting { fights: fight.fights },
        }
    }

    /// An error if the run has left its recorded checksums, as far as it's got. Checked before a
    /// capture writes its picture, which then can't show another fight.
    pub(crate) fn check(&self) -> Result<(), String> {
        let Mode::Replaying { player, name, recorded: Some(recorded) } = &self.mode else { return Ok(()) };
        match recorded.first_difference(player.trail()) {
            Some(difference) => Err(diverged(name, &self.path, difference)),
            None => Ok(()),
        }
    }

    /// Writes what a recording of someone playing holds so far, for an app that may never come back from
    /// where it's going. A failure is logged and the fight goes on: there's no one to tell, and the next
    /// try, or the end, may do better. Nothing else has a file to keep.
    pub fn keep_so_far(&mut self) {
        let Mode::Recording { recording, script: None, files, kind, .. } = &mut self.mode else { return };
        if !kind.has_file() {
            return;
        }
        let replay = recording.so_far();
        let bytes = replay.encode();
        let size = bytes.len();
        match files.write(&self.path, bytes) {
            Ok(()) => tracing::info!(
                target: LOG_TARGET,
                path = %self.path.display(),
                ticks = replay.ticks(),
                bytes = size,
                checksum = %format_args!("{:016x}", recording.simulation().checksum()),
                "kept the recording so far",
            ),
            Err(unwritten) => tracing::warn!(
                target: LOG_TARGET,
                error = %unwritten.into_message(),
                "the recording so far didn't reach its file",
            ),
        }
    }

    /// Ends the run: checks a replay, or writes a recording, and logs where it got to. A session that
    /// didn't reach its file fails this with an error that says what became of it.
    pub fn finish(mut self) -> Result<(), String> {
        self.check()?;
        let written = self.write_recording();
        self.report_end(written.is_ok());
        written.map_err(Unwritten::into_message)
    }

    /// Writes a recording as it stands, with the run going on, so that a session that reaches no file at
    /// all can stay in memory and be tried again. Nothing else has a file to write.
    fn write_recording(&mut self) -> Result<(), Unwritten> {
        let Mode::Recording { recording, files, kind, .. } = &mut self.mode else { return Ok(()) };
        if !kind.has_file() {
            return Ok(());
        }
        files.write(&self.path, recording.so_far().encode())
    }

    /// Removes what a recording thrown away wrote on the way.
    #[cfg(any(feature = "dev", test))]
    fn remove_files(&self) {
        if let Mode::Recording { files, .. } = &self.mode {
            files.remove(&self.path);
        }
    }

    /// Logs where the run got to. A recording says so only if its file was `written`: what became of it
    /// otherwise is the error's to say.
    fn report_end(self, written: bool) {
        let Self { path, mode, steps_past_end, .. } = self;
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
            Mode::Recording { recording, stopped, kind, .. } => {
                if let Some(tick) = recording.filled_at() {
                    tracing::warn!(
                        target: LOG_TARGET,
                        tick,
                        played = recording.simulation().tick(),
                        "the run went on past the file's limit on a run's length: the file ends there",
                    );
                }
                if !kind.has_file() {
                    let (ticks, checksum) =
                        (recording.simulation().tick(), recording.simulation().checksum());
                    let (replay, played) = recording.finish();
                    log_gate(&played);
                    tracing::info!(
                        target: LOG_TARGET,
                        scenario = replay.scenario(),
                        ticks,
                        checksum = %format_args!("{checksum:016x}"),
                        allocations = played.stats.allocations,
                        "a run ended, kept nowhere",
                    );
                    return;
                }
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
                if written {
                    tracing::info!(
                        target: LOG_TARGET,
                        path = %path.display(),
                        ticks = replay.ticks(),
                        bytes = replay.encode().len(),
                        checksum = %format_args!("{:016x}", played.trail.end()),
                        allocations = played.stats.allocations,
                        "recorded a replay",
                    );
                }
            }
            Mode::Fighting(fight) => fight.report(),
        }
    }
}

/// The files a recording's session has reached. Someone played it and no script makes it again, so it
/// goes somewhere whatever happens, and each write knows what it may replace: `path` was free when the
/// recording began, so anything there now that this didn't put there isn't this session's.
struct SessionFiles {
    /// Where the session goes if its own file won't take it.
    rescue_dir: PathBuf,
    /// What this session last put at its own name.
    kept: Option<Vec<u8>>,
    /// Where it last went instead, and what it put there: always later than `kept`.
    rescued: Option<(PathBuf, Vec<u8>)>,
}

impl SessionFiles {
    fn new(rescue_dir: PathBuf) -> Self {
        Self { rescue_dir, kept: None, rescued: None }
    }

    /// Writes the session as `bytes` to `path`, or if it can't, to its one file in `rescue_dir`, however
    /// many trips to the background that takes. Reaching `path` again removes that file, which is only an
    /// older part of this now.
    fn write(&mut self, path: &Path, bytes: Vec<u8>) -> Result<(), Unwritten> {
        let error = match write_file(path, &bytes, self.kept.as_deref()) {
            Ok(()) => {
                if let Some((rescue, held)) = self.rescued.take()
                    && holds(&rescue, &held)
                {
                    let _ = fs::remove_file(rescue);
                }
                self.kept = Some(bytes);
                return Ok(());
            }
            Err(error) => error,
        };
        let failed = format!("--record {}: {error}", path.display());
        let (rescue, ours) = match &self.rescued {
            Some((rescue, held)) if holds(rescue, held) => (rescue.clone(), Some(held.as_slice())),
            _ => (self.free_rescue(path), None),
        };
        match write_file(&rescue, &bytes, ours) {
            Ok(()) => {
                let message = format!("{failed}. The session is in {} instead", rescue.display());
                self.rescued = Some((rescue, bytes));
                Err(Unwritten::Rescued(message))
            }
            Err(second) => {
                let left = self.last_file(path).map_or(String::new(), |file| {
                    format!(". {} still holds it as far as it was last kept", file.display())
                });
                Err(Unwritten::Nowhere(format!(
                    "{failed}. Nor would {} take the session: {second}{left}",
                    rescue.display()
                )))
            }
        }
    }

    /// A name for the session's file in `rescue_dir` that nothing has yet.
    fn free_rescue(&self, path: &Path) -> PathBuf {
        let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("session");
        (1_u32..)
            .map(|number| self.rescue_dir.join(format!("{stem}-unsaved-{number}.sreplay")))
            .find(|rescue| !rescue.exists())
            .expect("fewer than 2³² rescued sessions")
    }

    /// The file holding what was last written of the session, if that's still there as it was written.
    fn last_file<'a>(&'a self, path: &'a Path) -> Option<&'a Path> {
        let rescued = self.rescued.as_ref().filter(|(rescue, held)| holds(rescue, held));
        let kept = self.kept.as_ref().filter(|kept| holds(path, kept));
        rescued.map(|(rescue, _)| rescue.as_path()).or(kept.map(|_| path))
    }

    /// Removes what this session wrote, for one that's been thrown away. A file someone has changed since
    /// isn't its to remove.
    #[cfg(any(feature = "dev", test))]
    fn remove(&self, path: &Path) {
        if self.kept.as_ref().is_some_and(|kept| holds(path, kept)) {
            let _ = fs::remove_file(path);
        }
        if let Some((rescue, held)) = &self.rescued
            && holds(rescue, held)
        {
            let _ = fs::remove_file(rescue);
        }
    }
}

/// `path`, or if something's there, the first free name beside it, numbered from 2.
fn free_name(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_owned();
    }
    let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("session");
    (2_u32..)
        .map(|number| path.with_file_name(format!("{stem}-{number}.sreplay")))
        .find(|free| !free.exists())
        .expect("fewer than 2³² recordings")
}

/// How a session failed to reach the file it was meant for.
enum Unwritten {
    /// It's in another file, which this names.
    Rescued(String),
    /// None of it went anywhere this time, and only the run holds what came after the last file, if any.
    Nowhere(String),
}

impl Unwritten {
    fn into_message(self) -> String {
        match self {
            Self::Rescued(message) | Self::Nowhere(message) => message,
        }
    }
}

/// Writes `bytes` to `path` beside itself first, so a process killed mid-write leaves the last whole file
/// standing, and a full disk's part of a file never reads as a damaged replay. `ours` is what this
/// session last put at `path`, the one thing there it may replace: anything else stays, and this fails.
///
/// A rename replaces whatever it lands on, so a name not known to be ours is claimed first, which fails
/// if it's taken. Phones are why it isn't a hard link: Android refuses apps those. A kill between the
/// claim and the rename leaves an empty file at the name, with the session whole beside it, so what lies
/// beside a file may be all there is of a session: this goes through a part of its own ([`new_partial`]),
/// and removes that and nothing else.
fn write_file(path: &Path, bytes: &[u8], ours: Option<&[u8]>) -> std::io::Result<()> {
    let (partial, file) = new_partial(path)?;
    fill(file, &partial, bytes)?;
    let replacing = ours.is_some_and(|ours| holds(path, ours));
    let claimed = if replacing {
        Ok(())
    } else {
        fs::OpenOptions::new().write(true).create_new(true).open(path).map(drop)
    };
    let placed = claimed.and_then(|()| {
        fs::rename(&partial, path).inspect_err(|_| {
            if !replacing {
                let _ = fs::remove_file(path);
            }
        })
    });
    placed.inspect_err(|_| {
        let _ = fs::remove_file(&partial);
    })
}

/// How many names beside a file [`new_partial`] tries. Only kills leave parts behind, so this is a lot of
/// kills, and a directory that says every name is taken can't hold a write forever.
const PARTIAL_NAMES: u32 = 64;

/// The name beside `path` that a write's part takes, by how many are taken before it.
fn partial_name(path: &Path, taken: u32) -> PathBuf {
    match taken {
        0 => path.with_extension("partial"),
        _ => path.with_extension(format!("{taken}.partial")),
    }
}

/// Makes a file beside `path` that nothing else has: the first free [`partial_name`], since making it
/// fails if it's there, so two runs can't write through each other. Beside it, a rename stays on one
/// file system.
fn new_partial(path: &Path) -> std::io::Result<(PathBuf, fs::File)> {
    for taken in 0..PARTIAL_NAMES {
        let partial = partial_name(path, taken);
        match fs::OpenOptions::new().write(true).create_new(true).open(&partial) {
            Ok(file) => return Ok((partial, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("{PARTIAL_NAMES} parts of earlier writes are beside it"),
    ))
}

/// Writes `bytes` to `file`, the file at `path`, and removes it if they don't all go in.
fn fill(mut file: impl Write, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let written = file.write_all(bytes);
    // Closed first: Windows may refuse to remove a file that's open.
    drop(file);
    written.inspect_err(|_| {
        let _ = fs::remove_file(path);
    })
}

/// Whether the file at `path` holds exactly `bytes`.
fn holds(path: &Path, bytes: &[u8]) -> bool {
    fs::read(path).is_ok_and(|now| now == bytes)
}

/// What a [`Combat`] is running, as the overlay shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombatStatus<'a> {
    Replaying {
        name: &'a str,
        length: u32,
    },
    Recording {
        scenario: &'a str,
        path: &'a Path,
        at_limit: bool,
        scripted: bool,
    },
    Fighting {
        fights: u32,
    },
    /// A game's run, and the file it's kept in, if it's kept.
    Running {
        scenario: &'a str,
        kept: Option<&'a Path>,
    },
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
pub struct SessionRecorder {
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
    pub(crate) fn start(&mut self, combat: &mut Option<Combat>, scenario: &str) {
        self.start_tuned(combat, scenario, &LiveMoves::default());
    }

    /// Starts `scenario` over from `live`'s edits of its documents (M2 T2): a fight started after a
    /// save plays what was saved, and its file's hash says so.
    pub fn start_tuned(&mut self, combat: &mut Option<Combat>, scenario: &str, live: &LiveMoves) {
        self.start_tuned_with_setup(combat, scenario, live, |_, _, _| Ok(()));
    }

    pub(crate) fn start_tuned_with_setup(
        &mut self,
        combat: &mut Option<Combat>,
        scenario: &str,
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) {
        if combat.is_some() {
            self.note = Some("A fight is running: end it before starting another.".to_owned());
            return;
        }
        match self.new_recording(scenario, live, setup) {
            Ok(recording) => {
                *combat = Some(recording);
                self.note = None;
            }
            Err(error) => self.note = Some(error),
        }
    }

    #[cfg(feature = "dev")]
    pub(crate) fn note_error(&mut self, error: String) {
        self.note = Some(error);
    }

    fn new_recording(
        &self,
        scenario: &str,
        live: &LiveMoves,
        setup: impl FnOnce(&Scenario, &mut Simulation, &[ActorId]) -> Result<(), String>,
    ) -> Result<Combat, String> {
        // Up front: failing here costs nothing, and failing at the end would cost the session.
        fs::create_dir_all(&self.dir).map_err(|error| format!("{}: {error}", self.dir.display()))?;
        let path = (1_u32..)
            .map(|number| self.dir.join(format!("{scenario}-{number}.sreplay")))
            .find(|path| !path.exists())
            .expect("fewer than 2³² recordings");
        Combat::recording_with_setup(&path, scenario, None, live, setup)
    }

    /// Ends a recording and writes it. If the session can reach no file at all, the recording goes on,
    /// and saving again tries again.
    pub fn save(&mut self, combat: &mut Option<Combat>) {
        let Some(mut recording) = take_recording(combat) else { return };
        let edited_at = recording.edited_at();
        let ticks = edited_at.map_or(recording.simulation().tick(), u64::from);
        let path = recording.path.clone();
        let written = recording.write_recording();
        if let Err(Unwritten::Nowhere(error)) = written {
            self.note = Some(format!("{error}. It's still recording: stop and save to try again."));
            *combat = Some(recording);
            return;
        }
        recording.report_end(written.is_ok());
        self.note = Some(match (written, edited_at) {
            (Ok(()), None) => format!("Saved {ticks} ticks to {}", path.display()),
            (Ok(()), Some(_)) => {
                format!("Saved {ticks} ticks to {}: a live edit ended the file there", path.display())
            }
            (Err(error), _) => error.into_message(),
        });
    }

    /// Ends a recording, writes nothing, and removes what it kept on the way.
    pub fn discard(&mut self, combat: &mut Option<Combat>) {
        if let Some(recording) = take_recording(combat) {
            recording.remove_files();
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
pub fn read_bindings(combat: Option<&mut Combat>, bindings: &mut dyn ActionSource, player: ActorId) {
    let asked = combat.is_some_and(|combat| combat.step(bindings));
    if !asked {
        bindings.actions(0, &[player]);
    }
}

/// A script by name.
/// A recording script's timeline starts, returns to checkpoints and dialogue choices, carried out at their
/// boundaries as the recording goes (M3 Q3, B4, Q2). Its skips and zone entries aren't: on a stage, both are
/// presentation's, which tells the recording ([`Combat::skip_timeline`]).
#[derive(Clone, Copy, Debug)]
struct ScriptStarts {
    commands: &'static [(u32, ReplayCommand)],
    next: usize,
}

impl ScriptStarts {
    fn new(commands: &'static [(u32, ReplayCommand)]) -> Self {
        Self { commands, next: 0 }
    }

    /// These, with the boundary before the first step behind them.
    fn after_start(self) -> Self {
        let next = self.commands.iter().take_while(|(at, _)| *at == 0).count();
        Self { next, ..self }
    }

    /// Carries out what comes at the boundary `recording` stands on.
    fn apply(&mut self, recording: &mut Recording) -> Result<(), String> {
        while let Some(&(at, command)) = self.commands.get(self.next)
            && at <= recording.recorded_ticks()
        {
            self.next += 1;
            match command {
                ReplayCommand::PlayTimeline(id) => {
                    recording.play_timeline(id).map_err(|error| error.to_string())?;
                }
                ReplayCommand::ReturnTo(checkpoint) => {
                    recording.return_to_checkpoint(checkpoint).map_err(|error| error.to_string())?;
                }
                ReplayCommand::Dialogue(effect) => {
                    recording.apply_dialogue_effect(effect).map_err(|error| error.to_string())?;
                }
                ReplayCommand::SkipTimeline { .. } | ReplayCommand::EnterZone(_) => {}
            }
        }
        Ok(())
    }
}

fn script_actions(name: &str) -> Result<ScriptActions, String> {
    find_script(name).map(|script| script.actions).ok_or_else(|| {
        let known: Vec<&str> = SCRIPTS.iter().map(|script| script.name).collect();
        format!("--script: no script `{name}`; there's {known:?}")
    })
}

/// Rule R6, beside a renderer. The replay suite is the gate; here it's a finding to look at, since a
/// profiling build's zones may allocate where a plain one doesn't. Android counts per process, so a step
/// there takes in whatever other threads did meanwhile, and a shader build's say nothing of the step:
/// the same file played headlessly answers it.
fn log_gate(played: &Played) {
    if !sapphire_core::alloc::PER_THREAD_COUNTS {
        return;
    }
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
    use std::time::Duration;

    use sapphire_gameplay::replay::{Replay, find_script};
    use sapphire_gameplay::{ActorActions, Buttons, Stick};

    use super::*;

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

    /// A phone may kill what's in the background with no word, and an iPhone's app never quits, so
    /// leaving the foreground writes the file as far as it's got. The end writes the whole of it, and a
    /// script's recording, which any run can make again, keeps nothing.
    #[test]
    fn a_recording_someone_plays_keeps_what_it_has_when_the_app_leaves() {
        let scratch = Scratch::new("keeps");
        let mut combat = recording(&scratch.0, None);
        let mut neutral = no_bindings;
        for _ in 0..30 {
            combat.step(&mut neutral);
        }
        combat.keep_so_far();
        let at_30 = combat.simulation().checksum();
        let kept = Replay::decode(&fs::read(&scratch.0).expect("kept")).expect("a whole file");
        assert_eq!(kept.ticks(), 30);
        assert!(!scratch.0.with_extension("partial").exists(), "nothing's left beside it");
        let mut player = Player::new(kept, &SCENARIOS).expect("starts");
        player.run_to_end();
        assert_eq!(player.finish().trail.end(), at_30, "what it kept plays to where it was");

        // And it goes on, as if nothing had happened.
        for _ in 0..30 {
            combat.step(&mut neutral);
        }
        combat.finish().expect("written");
        assert_eq!(Replay::decode(&fs::read(&scratch.0).expect("written")).expect("reads").ticks(), 60);

        let scratch = Scratch::new("scripted");
        let mut combat = recording(&scratch.0, Some("idle"));
        combat.step(&mut neutral);
        combat.keep_so_far();
        assert!(!scratch.0.exists());
    }

    #[test]
    fn only_the_bindings_count_as_someone_playing() {
        let fight = |script: Option<&str>| {
            Combat::new(&CombatOption::Fight { script: script.map(str::to_owned), party: false })
                .expect("sets up")
        };
        assert!(fight(None).is_played());
        assert!(!fight(Some("idle")).is_played());

        let played = Scratch::new("played-by-someone");
        assert!(recording(&played.0, None).is_played());
        let scripted = Scratch::new("played-by-a-script");
        let mut combat = recording(&scripted.0, Some("idle"));
        assert!(!combat.is_played());
        combat.step(&mut no_bindings);
        combat.finish().expect("writes");
        let file = CombatOption::Replay { path: scripted.0.clone(), to_end: true };
        assert!(!Combat::new(&file).expect("reads").is_played());
    }

    /// What a bench reads §3.2's script row from (M3 Z1): a fight whose scenario binds hooks counts their time
    /// as it goes, and one with no script host has none to count.
    #[test]
    fn a_scripted_fight_counts_its_scripts_time_and_a_plain_one_has_none() {
        let committed = |name: &str| CombatOption::Replay {
            path: Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/replays/{name}.sreplay")),
            to_end: true,
        };
        let mut scripted = Combat::new(&committed("arena-scripted")).expect("reads");
        assert_eq!(scripted.script_time(), Some(Duration::ZERO), "nothing's run yet");
        while scripted.script_time() == Some(Duration::ZERO) {
            assert!(scripted.simulation().tick() < 1_700, "no hook or collection was timed");
            scripted.step(&mut no_bindings);
        }
        let plain = Combat::new(&committed("arena-fight-won")).expect("reads");
        assert_eq!(plain.script_time(), None);
    }

    fn run(scenario: &str, keep: Option<&Path>) -> Combat {
        let option =
            CombatOption::Run { scenario: scenario.to_owned(), script: None, keep: keep.map(Path::to_owned) };
        Combat::new(&option).expect("sets up")
    }

    /// A game's run writes nothing unless it's kept, and kept, it's a replay like any other, the beats'
    /// timeline plays and skips among its commands, which plays back to where the run got.
    #[test]
    fn a_run_keeps_a_file_only_when_asked_and_records_its_beats_commands() {
        let mut unkept = run(sapphire_gameplay::replay::WARD_COURT, None);
        assert_eq!(unkept.status(), CombatStatus::Running { scenario: "ward-court", kept: None });
        assert!(unkept.is_played());
        for _ in 0..5 {
            unkept.step(&mut no_bindings);
        }
        unkept.keep_so_far();
        unkept.finish().expect("nothing to write");

        let scratch = Scratch::new("run-kept");
        let mut kept = run(sapphire_gameplay::replay::WARD_COURT, Some(&scratch.0));
        assert_eq!(kept.status(), CombatStatus::Running { scenario: "ward-court", kept: Some(&scratch.0) });
        for _ in 0..5 {
            kept.step(&mut no_bindings);
        }
        kept.play_timeline(sapphire_gameplay::replay::COURT_GATE_ID).expect("a beat's timeline");
        assert!(kept.simulation().timeline().is_some());
        kept.step(&mut no_bindings);
        kept.skip_timeline(400);
        for _ in 0..5 {
            kept.step(&mut no_bindings);
        }
        let end = kept.simulation().checksum();
        kept.finish().expect("written");
        let replay = Replay::decode(&fs::read(&scratch.0).expect("kept")).expect("a whole file");
        let commands: Vec<ReplayCommand> = replay.commands().iter().map(|(_, command)| *command).collect();
        assert_eq!(
            commands,
            [
                ReplayCommand::PlayTimeline(sapphire_gameplay::replay::COURT_GATE_ID),
                ReplayCommand::SkipTimeline { at_ms: 400 }
            ]
        );
        let mut player = Player::new(replay, &SCENARIOS).expect("starts");
        player.run_to_end();
        assert_eq!(player.simulation().checksum(), end, "it plays back to where the run got");
    }

    /// Only what records takes a beat's timeline; a replay and a fight with no file say why not.
    #[test]
    fn only_a_recording_takes_a_beats_timeline() {
        let mut fight = Combat::new(&CombatOption::Fight { script: None, party: false }).expect("sets up");
        let refused = fight.play_timeline(sapphire_gameplay::replay::COURT_GATE_ID).expect_err("no file");
        assert!(refused.contains("no timelines"), "{refused}");
    }

    fn recording(path: &Path, script: Option<&str>) -> Combat {
        let option = CombatOption::Record {
            path: path.to_owned(),
            scenario: SCENARIOS[0].name.to_owned(),
            script: script.map(str::to_owned),
        };
        Combat::new(&option).expect("sets up")
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

    #[test]
    fn capture_frames_refuse_zero_count_and_overflowing_tick_spans() {
        assert_eq!(CaptureFrames { count: 0, step: 0 }.first_tick(0), None);
        assert_eq!(CaptureFrames { count: u32::MAX, step: u32::MAX }.first_tick(u32::MAX), None);
        assert_eq!(CaptureFrames { count: 3, step: 2 }.first_tick(4), Some(0));
        assert_eq!(CaptureFrames { count: 3, step: 2 }.first_tick(3), None);
        assert_eq!(CaptureFrames { count: 1, step: u32::MAX }.first_tick(7), Some(7));
    }

    fn fighting(script: Option<&str>) -> Combat {
        Combat::new(&CombatOption::Fight { script: script.map(str::to_owned), party: false })
            .expect("sets up")
    }

    /// One foe falling among several is no end of a fight: the rest are still there.
    #[test]
    fn a_fight_goes_on_while_a_foe_stands() {
        // A live fight is always the arena's two or a party, so this runs the skirmish's own world.
        let scenario = find_scenario(&SCENARIOS, crate::arena::SKIRMISH_SCENARIO).expect("listed");
        let (mut sim, cast) = scenario.start(scenario.config).expect("it starts");
        // The player runs for it, so the warriors' fight with each other is what fells someone first.
        let mut away = TickActions::NEUTRAL;
        let actions = ActorActions { buttons: Buttons::NONE, stick: Stick::new(0, -127) };
        away.set(cast[0], actions).expect("room");
        let standing =
            |sim: &Simulation, id: &ActorId| sim.actor(*id).is_some_and(|actor| !actor.fighter.is_defeated());
        for _ in 0..6000 {
            sim.step(&away);
            if !standing(&sim, &cast[0]) {
                break;
            }
            let foes = cast[1..].iter().filter(|id| standing(&sim, id)).count();
            if foes < cast.len() - 1 {
                assert!(!is_over(&sim, &cast), "{foes} left");
                return;
            }
        }
        panic!("nobody fell before the player did");
    }

    /// A fight with no file that went into a zone starts over at the door it came in by (M3 W4), in the
    /// storage the last one left: from the first restart on, one allocates nothing, and each is the same.
    #[test]
    fn a_fight_in_a_zone_starts_over_at_its_door_without_allocating() {
        let hub = &sapphire_gameplay::replay::BAKED_ZONES[0];
        let mut zone = AlignedVec::<16>::with_capacity(hub.zone.len());
        zone.extend_from_slice(hub.zone);
        let simulation = &sapphire_zone::Zone::read(&zone).expect("a cooked zone").simulation;
        let (colliders, navmesh) =
            (simulation.colliders.as_ref().copied(), simulation.navigation.as_ref().copied());
        let read = |id: AssetId| {
            if id == hub.asset {
                Some(hub.zone)
            } else if Some(id) == colliders {
                Some(hub.colliders)
            } else if Some(id) == navmesh {
                Some(hub.navmesh)
            } else {
                None
            }
        };
        let mut combat = fighting(Some("idle"));
        let door = sapphire_core::Name::new("from_ground").as_u64();
        combat.enter_zone(hub.asset, &sapphire_zone::pack_name(hub.stem), door, read).expect("it goes in");
        let player = combat.cast()[0];
        let standing = combat.simulation().actor(player).expect("the traveller").position;
        assert_eq!(combat.simulation().actors().len(), 1, "who stayed behind is gone");

        combat.restart();
        let at_door = combat.simulation().checksum();
        let mut trail = Vec::new();
        for _ in 0..60 {
            combat.step(&mut no_bindings);
            trail.push(combat.simulation().checksum());
        }
        let scope = sapphire_core::alloc::AllocScope::begin();
        combat.restart();
        let allocations = scope.counts().allocations;
        let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
        assert_eq!(fights, 3);
        assert_eq!(combat.simulation().tick(), 0, "it starts over");
        assert_eq!(combat.simulation().current_zone(), Some(sapphire_gameplay::replay::PROVING_HUB));
        assert_eq!(combat.simulation().actor(player).expect("at the door").position, standing);
        assert_eq!(combat.simulation().actors().len(), 1);
        assert_eq!(combat.simulation().checksum(), at_door, "each fight starts the same");
        if sapphire_core::alloc::PER_THREAD_COUNTS {
            assert_eq!(allocations, 0, "a restart in a zone is allocation-free");
        }
        for expected in &trail {
            combat.step(&mut no_bindings);
            assert_eq!(combat.simulation().checksum(), *expected, "at tick {}", combat.simulation().tick());
        }
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
        let scope = sapphire_core::alloc::AllocScope::begin();
        combat.step(&mut no_bindings);
        let allocations = scope.counts().allocations;
        let CombatStatus::Fighting { fights } = combat.status() else { panic!("{:?}", combat.status()) };
        assert_eq!(fights, 2);
        assert_eq!(combat.simulation().tick(), 0, "it starts over");
        assert!(sapphire_core::alloc::is_installed());
        assert_eq!(combat.last_step_allocations(), allocations, "the public counter includes the restart");
        if sapphire_core::alloc::PER_THREAD_COUNTS {
            assert_eq!(allocations, 0, "the complete automatic restart is allocation-free");
        }

        for expected in &trail {
            assert_eq!(combat.simulation().checksum(), *expected, "at tick {}", combat.simulation().tick());
            combat.step(&mut no_bindings);
        }
        assert_eq!(
            combat.simulation().stats().allocations,
            0,
            "simulation steps only; restart is counted above"
        );
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
        let scenario = find_scenario(&SCENARIOS, "duel").expect("there");
        let mut whole = Recording::new(scenario, scenario.config).expect("sets up");
        for _ in 0..3 {
            whole.step(&mut press_light).expect("short");
        }
        let (whole, played) = whole.finish();
        assert_eq!(replay, whole, "the same file, byte for byte");
        let mut player = Player::new(replay, &SCENARIOS).expect("starts");
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

    fn send_rescues_to(combat: &mut Combat, dir: PathBuf) {
        let Mode::Recording { files, .. } = &mut combat.mode else { panic!("a recording") };
        files.rescue_dir = dir;
    }

    /// A recording whose directory has gone since it began, as a full disk fails it too: all it has
    /// left is where it would rescue the session to.
    fn recording_with_nowhere_to_go(scratch: &ScratchDir, rescue: PathBuf) -> Combat {
        let inside = scratch.0.join("going");
        fs::create_dir_all(&inside).expect("makes");
        let mut combat = recording(&inside.join("played.sreplay"), None);
        for _ in 0..3 {
            combat.step(&mut press_light);
        }
        fs::remove_dir_all(&inside).expect("removes");
        send_rescues_to(&mut combat, rescue);
        combat
    }

    fn ticks_in(path: &Path) -> u32 {
        Replay::decode(&fs::read(path).expect("a file")).expect("a whole replay").ticks()
    }

    /// A recording of someone playing, in a directory of its own, and its file's name.
    fn played_in(scratch: &ScratchDir) -> (Combat, PathBuf) {
        fs::create_dir_all(&scratch.0).expect("makes");
        let path = scratch.0.join("played.sreplay");
        (recording(&path, None), path)
    }

    /// `steps` more ticks of `combat`, so each write has something new to hold.
    fn play(combat: &mut Combat, steps: u32) {
        for _ in 0..steps {
            combat.step(&mut press_light);
        }
    }

    /// The names of what's in `dir`, in order.
    fn listing(dir: &Path) -> Vec<String> {
        let entries = fs::read_dir(dir).expect("reads");
        let mut names: Vec<String> =
            entries.map(|entry| entry.expect("reads").file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// A killed app says nothing and writes nothing: what it last kept is all there is, whole, and the
    /// write it was in the middle of leaves only a part beside it, which no later run's write touches.
    #[test]
    fn a_session_kept_on_a_trip_survives_a_kill() {
        let scratch = ScratchDir::new("killed");
        let (mut combat, path) = played_in(&scratch);
        play(&mut combat, 5);
        combat.keep_so_far();
        play(&mut combat, 5);
        let left = path.with_extension("partial");
        fs::write(&left, b"part of a re").expect("writes");
        std::mem::forget(combat);
        assert_eq!(ticks_in(&path), 5);

        fs::remove_file(&path).expect("removes");
        let mut combat = recording(&path, None);
        play(&mut combat, 3);
        combat.keep_so_far();
        assert_eq!(ticks_in(&path), 3);
        assert_eq!(fs::read(&left).expect("kept"), b"part of a re");
        assert_eq!(listing(&scratch.0), ["played.partial", "played.sreplay"], "its own part is gone");
    }

    /// A kill between a write's claim and its rename leaves the name empty and the session whole beside
    /// it, so what's beside a file may be the only copy of a session nobody can play again.
    #[test]
    fn a_session_whole_beside_its_name_outlives_the_next_run() {
        let scratch = ScratchDir::new("killed-whole");
        let (mut combat, path) = played_in(&scratch);
        play(&mut combat, 5);
        let left = path.with_extension("partial");
        let Mode::Recording { recording: session, .. } = &mut combat.mode else { panic!("a recording") };
        fs::write(&left, session.so_far().encode()).expect("writes");
        std::mem::forget(combat);

        // The name's free again, and someone plays under it.
        let mut combat = recording(&path, None);
        play(&mut combat, 3);
        combat.finish().expect("written");
        assert_eq!(ticks_in(&path), 3);
        assert_eq!(ticks_in(&left), 5, "the first session is still there to be found");
    }

    /// A write takes away only what it made, whichever way it goes: to a free name, over its own file, or
    /// refused by someone else's.
    #[test]
    fn a_write_leaves_what_it_found_beside_its_file() {
        let scratch = ScratchDir::new("beside");
        let (mut combat, path) = played_in(&scratch);
        send_rescues_to(&mut combat, scratch.0.clone());
        let left = path.with_extension("partial");
        fs::write(&left, b"another run's").expect("writes");
        let kept = |expected: &[&str]| {
            assert_eq!(fs::read(&left).expect("kept"), b"another run's");
            assert_eq!(listing(&scratch.0), expected);
        };

        play(&mut combat, 2);
        combat.keep_so_far();
        assert_eq!(ticks_in(&path), 2, "a free name");
        kept(&["played.partial", "played.sreplay"]);

        play(&mut combat, 2);
        combat.keep_so_far();
        assert_eq!(ticks_in(&path), 4, "its own file");
        kept(&["played.partial", "played.sreplay"]);

        fs::write(&path, b"another session").expect("writes");
        play(&mut combat, 2);
        combat.keep_so_far();
        assert_eq!(fs::read(&path).expect("kept"), b"another session", "refused");
        assert_eq!(ticks_in(&scratch.0.join("played-unsaved-1.sreplay")), 6);
        kept(&["played-unsaved-1.sreplay", "played.partial", "played.sreplay"]);

        let error = write_file(&path, b"more", None).expect_err("refused");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        kept(&["played-unsaved-1.sreplay", "played.partial", "played.sreplay"]);
    }

    /// Two runs after one name: whichever writes first has it, and the other goes elsewhere, with neither
    /// writing through the other's part.
    #[test]
    fn two_sessions_after_one_name_keep_out_of_each_others_way() {
        let scratch = ScratchDir::new("two");
        let (mut first, path) = played_in(&scratch);
        let mut second = recording(&path, None);
        send_rescues_to(&mut first, scratch.0.clone());
        play(&mut first, 2);
        play(&mut second, 3);

        // The second writes while the first is part way through writing its part.
        let writing = path.with_extension("partial");
        fs::write(&writing, b"the first's").expect("writes");
        second.keep_so_far();
        assert_eq!(fs::read(&writing).expect("kept"), b"the first's");
        assert_eq!(ticks_in(&path), 3, "the second came first");
        fs::remove_file(&writing).expect("the first's part");

        first.keep_so_far();
        assert_eq!(ticks_in(&path), 3, "and it stays");
        assert_eq!(ticks_in(&scratch.0.join("played-unsaved-1.sreplay")), 2);
    }

    /// A file there from before the first trip isn't the session's, and the session stays in one file
    /// over every trip after, right to the error that names it.
    #[test]
    fn a_kept_session_never_replaces_what_appeared_at_its_name() {
        let scratch = ScratchDir::new("trip-appeared");
        let (mut combat, path) = played_in(&scratch);
        send_rescues_to(&mut combat, scratch.0.clone());
        play(&mut combat, 2);
        fs::write(&path, b"another session").expect("writes");

        combat.keep_so_far();
        let rescued = scratch.0.join("played-unsaved-1.sreplay");
        assert_eq!(ticks_in(&rescued), 2);
        play(&mut combat, 2);
        combat.keep_so_far();
        assert_eq!(ticks_in(&rescued), 4, "the same file");
        assert!(!scratch.0.join("played-unsaved-2.sreplay").exists());

        play(&mut combat, 2);
        let error = combat.finish().expect_err("its own file didn't take it");
        assert!(error.contains(&rescued.display().to_string()), "{error}");
        assert_eq!(ticks_in(&rescued), 6);
        assert_eq!(fs::read(&path).expect("kept"), b"another session");
        assert!(!path.with_extension("partial").exists() && !rescued.with_extension("partial").exists());
    }

    /// What it kept is the session's to replace only while it's what it kept.
    #[test]
    fn a_kept_file_someone_replaced_is_theirs() {
        let scratch = ScratchDir::new("trip-replaced");
        let (mut combat, path) = played_in(&scratch);
        send_rescues_to(&mut combat, scratch.0.clone());
        play(&mut combat, 2);
        combat.keep_so_far();
        assert_eq!(ticks_in(&path), 2);
        fs::write(&path, b"another session").expect("writes");

        play(&mut combat, 2);
        combat.keep_so_far();
        assert_eq!(fs::read(&path).expect("kept"), b"another session");
        assert_eq!(ticks_in(&scratch.0.join("played-unsaved-1.sreplay")), 4);
    }

    /// A kept file someone tidied away leaves the name free, which is all the session asks of it.
    #[test]
    fn a_kept_file_removed_since_is_written_again() {
        let scratch = ScratchDir::new("trip-removed");
        let (mut combat, path) = played_in(&scratch);
        play(&mut combat, 2);
        combat.keep_so_far();
        fs::remove_file(&path).expect("removes");
        play(&mut combat, 2);
        combat.finish().expect("the name's free");
        assert_eq!(ticks_in(&path), 4);
    }

    /// One session, one file: reaching its own name again takes the rescue back, unless someone has
    /// changed that since.
    #[test]
    fn a_session_back_at_its_own_name_leaves_no_rescue() {
        let scratch = ScratchDir::new("trip-back");
        let rescue = scratch.0.join("rescue");
        fs::create_dir_all(&rescue).expect("makes");
        let inside = scratch.0.join("going");
        fs::create_dir_all(&inside).expect("makes");
        let path = inside.join("played.sreplay");
        let mut combat = recording(&path, None);
        send_rescues_to(&mut combat, rescue.clone());
        play(&mut combat, 2);
        fs::remove_dir_all(&inside).expect("removes");
        combat.keep_so_far();
        let rescued = rescue.join("played-unsaved-1.sreplay");
        assert_eq!(ticks_in(&rescued), 2);

        fs::create_dir_all(&inside).expect("makes");
        play(&mut combat, 2);
        combat.keep_so_far();
        assert_eq!(ticks_in(&path), 4);
        assert!(!rescued.exists());
        combat.finish().expect("its own file");
        assert_eq!(ticks_in(&path), 4);
    }

    /// With no write landing, the last whole file stands, and the error says that's what's left.
    #[test]
    fn a_session_with_nowhere_to_go_says_what_it_kept() {
        let scratch = ScratchDir::new("trip-lost");
        let (mut combat, path) = played_in(&scratch);
        send_rescues_to(&mut combat, scratch.0.join("no-such"));
        play(&mut combat, 2);
        combat.keep_so_far();
        // Nothing can write beside it now, as a full disk would refuse.
        for taken in 0..PARTIAL_NAMES {
            fs::write(partial_name(&path, taken), b"in the way").expect("writes");
        }
        play(&mut combat, 2);

        let error = combat.finish().expect_err("nothing took it");
        assert!(error.contains("Nor would"), "{error}");
        assert!(error.contains(&format!("{} still holds it", path.display())), "{error}");
        assert_eq!(ticks_in(&path), 2);
    }

    /// Throwing a session away takes its kept files with it, and only those.
    #[test]
    fn discarding_a_recording_removes_what_it_kept() {
        let scratch = ScratchDir::new("trip-discarded");
        let mut recorder = SessionRecorder::new(scratch.0.clone());
        let mut combat = None;
        recorder.start(&mut combat, "duel");
        let running = combat.as_mut().expect("running");
        play(running, 2);
        running.keep_so_far();
        let path = scratch.0.join("duel-1.sreplay");
        assert_eq!(ticks_in(&path), 2);
        recorder.discard(&mut combat);
        assert!(!path.exists());

        recorder.start(&mut combat, "duel");
        let running = combat.as_mut().expect("running");
        play(running, 2);
        running.keep_so_far();
        fs::write(&path, b"another session").expect("writes");
        recorder.discard(&mut combat);
        assert_eq!(fs::read(&path).expect("kept"), b"another session");
    }

    /// `--record`'s last chance: the session goes elsewhere, it says where, and the run still fails.
    #[test]
    fn a_session_its_file_wont_take_goes_elsewhere_and_says_where() {
        let scratch = ScratchDir::new("rescued");
        let rescue = scratch.0.join("rescue");
        fs::create_dir_all(&rescue).expect("makes");
        let combat = recording_with_nowhere_to_go(&scratch, rescue.clone());
        let error = combat.finish().expect_err("its own file didn't take it");
        let rescued = rescue.join("played-unsaved-1.sreplay");
        assert!(error.contains(&rescued.display().to_string()), "{error}");
        let replay = Replay::decode(&fs::read(&rescued).expect("written")).expect("reads");
        assert_eq!((replay.scenario(), replay.ticks()), (SCENARIOS[0].name, 3));

        // A second one leaves the first where it is.
        let combat = recording_with_nowhere_to_go(&scratch, rescue.clone());
        let error = combat.finish().expect_err("its own file didn't take it");
        assert!(error.contains("played-unsaved-2.sreplay"), "{error}");
        assert_eq!(Replay::decode(&fs::read(&rescued).expect("kept")).expect("reads").ticks(), 3);
    }

    /// A disk that fills part way leaves no part of a file behind.
    #[test]
    fn a_full_disk_leaves_no_partial_file() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::StorageFull.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let scratch = Scratch::new("full");
        fs::write(&scratch.0, b"a part").expect("writes");
        let error = fill(Full, &scratch.0, b"the rest").expect_err("no room");
        assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
        assert!(!scratch.0.exists());
    }

    /// With nowhere to write at all, a run that's ending can only say what it lost.
    #[test]
    fn a_session_with_nowhere_to_go_says_so() {
        let scratch = ScratchDir::new("lost");
        let error = recording_with_nowhere_to_go(&scratch, scratch.0.join("no-such"))
            .finish()
            .expect_err("nothing took it");
        assert!(error.contains("Nor would"), "{error}");
    }

    /// The overlay's recorder has a running app to wait in: the session stays, and so does the fight,
    /// until a save finds somewhere for it.
    #[test]
    fn the_overlay_keeps_a_session_it_cant_save_and_tries_again() {
        let scratch = ScratchDir::new("retried");
        let mut recorder = SessionRecorder::new(scratch.0.clone());
        let mut combat = Some(recording_with_nowhere_to_go(&scratch, scratch.0.join("no-such")));

        recorder.save(&mut combat);
        let running = combat.as_mut().expect("it keeps the session");
        assert!(recorder.note().is_some_and(|note| note.contains("try again")), "{:?}", recorder.note());
        running.step(&mut press_light);
        let CombatStatus::Recording { path, .. } = running.status() else { panic!("still recording") };
        let path = path.to_owned();

        // The directory's back, and the session has everything played meanwhile.
        fs::create_dir_all(path.parent().expect("a directory")).expect("makes");
        recorder.save(&mut combat);
        assert!(combat.is_none(), "it's over");
        assert!(recorder.note().is_some_and(|note| note.contains("Saved 4 ticks")), "{:?}", recorder.note());
        assert_eq!(Replay::decode(&fs::read(&path).expect("written")).expect("reads").ticks(), 4);
    }

    /// Somewhere else's file isn't replaced, whatever appeared at the name since the run began. The
    /// overlay says where its session went, as `--record` does, and doesn't go on recording it.
    #[test]
    fn a_recording_never_replaces_what_appeared_at_its_name() {
        let scratch = ScratchDir::new("appeared");
        let mut recorder = SessionRecorder::new(scratch.0.clone());
        let mut combat = None;
        recorder.start(&mut combat, "duel");
        combat.as_mut().expect("running").step(&mut press_light);
        let appeared = scratch.0.join("duel-1.sreplay");
        fs::write(&appeared, b"another session").expect("writes");
        send_rescues_to(combat.as_mut().expect("running"), scratch.0.clone());

        recorder.save(&mut combat);
        assert!(combat.is_none(), "it went somewhere");
        assert_eq!(fs::read(&appeared).expect("kept"), b"another session");
        assert!(!appeared.with_extension("partial").exists(), "nothing's left beside it");
        let rescued = scratch.0.join("duel-1-unsaved-1.sreplay");
        let note = recorder.note().expect("it says");
        assert!(note.contains(&rescued.display().to_string()), "{note}");
        assert_eq!(Replay::decode(&fs::read(&rescued).expect("written")).expect("reads").ticks(), 1);
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
        assert!(refused(&scratch.0, SCENARIOS[0].name, Some("idle")).contains("it exists"));
    }

    /// Someone who's come to play gets to, under the next free name, and nothing is replaced. An app
    /// that closed on a name left from last time would look like a crash on a phone.
    #[test]
    fn a_session_someone_plays_takes_the_next_free_name() {
        let scratch = ScratchDir::new("reused");
        fs::create_dir_all(&scratch.0).expect("makes");
        let path = scratch.0.join("fight.sreplay");
        fs::write(&path, b"last time's").expect("writes");
        fs::write(scratch.0.join("fight-2.sreplay"), b"the time before").expect("writes");

        let mut combat = recording(&path, None);
        play(&mut combat, 2);
        let CombatStatus::Recording { path: playing, .. } = combat.status() else { panic!("recording") };
        let free = scratch.0.join("fight-3.sreplay");
        assert_eq!(playing, free, "it says which, so the overlay does too");
        combat.finish().expect("written");
        assert_eq!(ticks_in(&free), 2);
        assert_eq!(fs::read(&path).expect("kept"), b"last time's");

        // A name no one's used is the name it was given.
        let unused = scratch.0.join("new.sreplay");
        assert!(matches!(
            recording(&unused, None).status(),
            CombatStatus::Recording { path, .. } if path == unused
        ));
    }
}
