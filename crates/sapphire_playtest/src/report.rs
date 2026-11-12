//! What a session says (M4 F2): each run replayed here, its encounters, and what the player did in each.
//!
//! [`Report::read`] reads a session's directory as the crate's docs describe it and plays each run's file
//! through the simulation, as `sapphire_replay check` does, a step at a time. Everything it counts the run
//! already recorded: the simulation's own events and state after each step, the file's inputs and
//! `frames.csv`. Nothing new is kept on the device for it.
//!
//! - **Parts:** a run is cut into parts by its encounters. An encounter's part runs from the step its trigger
//!   started it to the step it stopped running: cleared, started over (a return to a checkpoint, or its zone
//!   left), or the run's end. The parts between are outside any encounter: walking, or a whole run of a
//!   scenario with no zone. Two encounters running at once count to the one that started first, and the
//!   second's part begins when the first's ends.
//! - **The player's side** is the scenario's first actor, whose input the file holds, and every member of
//!   its party, if it has one.
//! - **Damage taken and hits:** each hit landed on the player's side ([`HitCue`]'s `damage`, as the attack
//!   says), once a hit. **Falls:** each [`DEFEATED`] cue on the player's side.
//! - **Dodges:** each start of a move with a perfect-dodge window, by the player's side, seen as a new playing
//!   (`CurrentMove::serial`) after a step. A move that starts and is cut off within one step isn't seen.
//!   **Perfect dodges:** each [`PERFECT_DODGE`] cue on the player's side. **Counters:** the moves a counter
//!   window started (`SimStats::counters_fired`) in steps where a member of the player's side, whose counter
//!   window was open, started a move.
//! - **Presses and idle time** are the file's own input for the first actor: a press is a button going down
//!   (a touch's flag aside), and idle is every tick of a stretch of at least [`IDLE_AFTER`] with no button
//!   held and the stick at rest.
//! - **Frames and heat** are `frames.csv`'s rows of the run whose tick falls in the part.
//!
//! The report is [`REPORT_FILE`] beside the session, a [`Report`] document, and the same as text
//! ([`Report::to_text`]). A run that doesn't replay here says why, and the rest of the session reports.

use std::fmt::Write as _;
use std::path::Path;

use bevy_reflect::TypePath;
use sapphire_core::thermal::ThermalState;
use sapphire_gameplay::replay::{ARCHITECTURE, Player, Replay, Scenario, document::encounter_path};
use sapphire_gameplay::{
    ActorActions, ActorId, Buttons, DEFEATED, EncounterPhase, HitCue, PERFECT_DODGE, POISE_BROKEN,
    Simulation, Stick,
};
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_serialize::AssetId;
use sapphire_time::TICK_HZ;
use serde::{Deserialize, Serialize};

use crate::document::{Build, Device, Ending, KeptRun};
use crate::frames::FrameRow;
use crate::kept::{Kept, ReadError, RunFiles};

/// The report, beside the session's document.
pub const REPORT_FILE: &str = "report.ron";

/// The shortest stretch with no input that counts as idle, in ticks: a second. Shorter gaps are between
/// presses, not away from them.
pub const IDLE_AFTER: u32 = TICK_HZ;

/// A session that can't be reported at all: one that can't be read. A run that doesn't replay is reported as
/// such instead.
pub type ReportError = ReadError;

/// What a session says, as `report.ron` holds it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::playtest"]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// The session's directory's name.
    pub session: String,
    pub build: Build,
    pub device: Device,
    /// The architecture the runs were played on here. Where it isn't the device's, no run is held to the
    /// device's checksums, since those are that architecture's.
    pub counted_on: String,
    /// Seconds played, on the session's clock.
    pub seconds: f64,
    pub ended: Option<Ending>,
    /// Trips to the background, and settings files kept.
    pub trips: u32,
    pub settings: u32,
    pub runs: Vec<RunReport>,
    pub marks: Vec<MarkReport>,
    /// Every row of `frames.csv`, the title's and between runs' too. `None` with no rows.
    pub frames: Option<Frames>,
}

impl Versioned for Report {
    /// 1: M4 F2. 2: M4 F3, the hits the player's side landed and the damage it dealt (`Counts`).
    const VERSION: u32 = 2;

    /// Version 1 is version 2 counting no hits landed: it didn't count them.
    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1 => data.decode().map_err(VersionError::Decode),
            found => Err(VersionError::TooOld { found, oldest: 1 }),
        }
    }
}

/// One run, replayed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunReport {
    pub number: u32,
    pub scenario: String,
    /// From the session's directory.
    pub file: String,
    pub ticks: u32,
    /// Whether it had ended on the device: one that hadn't was kept as far as it had got.
    pub ended: bool,
    pub check: Check,
    /// The whole run: every part's counts together.
    pub whole: Counts,
    /// In order, each tick in one.
    pub parts: Vec<Part>,
}

/// How a run's replay here compares with what the device reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Check {
    /// Its last tick's checksum is the device's.
    Same,
    /// It doesn't reach the device's: the counts are this machine's run, not the player's. `sapphire_replay
    /// check --dir <run>` names the first tick that differs.
    Differs { reached: String, ticks: u32 },
    /// It was played on another architecture, whose checksums these aren't.
    OtherArchitecture,
    /// It doesn't replay here at all, and why: nothing else is counted.
    Unread { reason: String },
}

/// A stretch of a run: an encounter, or the time outside one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    /// Its first and last ticks, from 1.
    pub from: u64,
    pub to: u64,
    /// Its ticks as seconds of the simulation's time.
    pub seconds: f64,
    /// The encounter, or `None` outside one.
    pub encounter: Option<Met>,
    pub counts: Counts,
    /// Its rows of `frames.csv`. `None` with none.
    pub frames: Option<Frames>,
}

/// An encounter a part is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Met {
    /// Its `.encounter.ron`'s ID.
    pub id: String,
    /// Its document, from the repository's root, where this build holds it.
    pub document: Option<String>,
    pub end: MetEnd,
}

/// How an encounter's part ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetEnd {
    /// Every wave came and fell.
    Cleared,
    /// It went back to waiting: a return to a checkpoint, or its zone left.
    StartedOver,
    /// The run ended with it going: a fall, a quit, or a kill.
    RunEnded,
}

/// What the player did in a stretch of play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    pub damage_taken: u64,
    pub hits_taken: u32,
    /// What the player's side dealt: each hit it landed on anyone else, and the damage the attack says.
    #[serde(default)]
    pub damage_dealt: u64,
    #[serde(default)]
    pub hits_landed: u32,
    pub falls: u32,
    pub dodges: u32,
    pub perfect_dodges: u32,
    pub counters: u32,
    pub presses: u32,
    pub presses_a_second: f64,
    pub idle_seconds: f64,
}

impl Counts {
    fn add(&mut self, other: &Self) {
        self.damage_taken += other.damage_taken;
        self.hits_taken += other.hits_taken;
        self.damage_dealt += other.damage_dealt;
        self.hits_landed += other.hits_landed;
        self.falls += other.falls;
        self.dodges += other.dodges;
        self.perfect_dodges += other.perfect_dodges;
        self.counters += other.counters;
        self.presses += other.presses;
        self.idle_seconds += other.idle_seconds;
    }

    fn finish(mut self, seconds: f64) -> Self {
        self.presses_a_second = if seconds > 0.0 { f64::from(self.presses) / seconds } else { 0.0 };
        self
    }
}

/// Rows of `frames.csv`, summed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frames {
    /// Rows: a second of play each.
    pub seconds: u32,
    pub frames: u64,
    pub mean_ms: f32,
    /// The longest frame.
    pub worst_ms: f32,
    /// Frames past one and a half of the tier's interval.
    pub late: u64,
    /// The tier's interval, as the last row had it.
    pub budget_ms: f32,
    /// The hottest of the OS's thermal states, as `ThermalState` names them.
    pub hottest: String,
    /// The least thermal headroom read, where the OS gives one.
    pub least_headroom: Option<f32>,
    /// Whether Low Power Mode was ever on.
    pub low_power: bool,
}

impl Frames {
    fn of<'a>(rows: impl IntoIterator<Item = &'a FrameRow>) -> Option<Self> {
        let mut rows = rows.into_iter().peekable();
        rows.peek()?;
        let (mut seconds, mut frames, mut total_ms, mut worst_ms, mut late) = (0, 0_u64, 0.0_f64, 0.0_f32, 0);
        let (mut budget_ms, mut hottest, mut least_headroom, mut low_power) =
            (0.0, ThermalState::Unknown, None::<f32>, false);
        for row in rows {
            seconds += 1;
            frames += u64::from(row.frames);
            total_ms += f64::from(row.mean_ms) * f64::from(row.frames);
            worst_ms = worst_ms.max(row.max_ms);
            late += u64::from(row.late);
            budget_ms = row.budget_ms;
            hottest = hottest.max(row.heat.state);
            if let Some(headroom) = row.heat.headroom {
                least_headroom = Some(least_headroom.map_or(headroom, |least| least.min(headroom)));
            }
            low_power |= row.heat.low_power == Some(true);
        }
        let mean_ms = if frames > 0 { (total_ms / frames as f64) as f32 } else { 0.0 };
        Some(Self {
            seconds,
            frames,
            mean_ms,
            worst_ms,
            late,
            budget_ms,
            hottest: format!("{hottest:?}"),
            least_headroom,
            low_power,
        })
    }
}

/// A mark the player set, where it is, and how to see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkReport {
    /// From 1, in the order they were set.
    pub number: u32,
    pub run: u32,
    pub tick: u64,
    /// On the session's clock.
    pub at: f64,
    pub label: Option<String>,
    /// The encounter it fell in, by its document (or ID), if it did.
    pub encounter: Option<String>,
    /// The run's file, from the session's directory, if the session kept the run.
    pub file: Option<String>,
    /// What draws the run's frame at that tick, on the playtest build, here: the game plays the file to it.
    pub command: Option<String>,
}

impl Report {
    /// Reads the session in `dir` and replays each of its runs on `scenarios`, which must be the game's: a
    /// runner's, with the scripts a scenario's kinds of enemy bind.
    pub fn read(dir: &Path, scenarios: &[Scenario]) -> Result<Self, ReportError> {
        Ok(Self::of(dir, Kept::read(dir)?, scenarios))
    }

    /// [`Self::read`], of a session already read from `dir`.
    #[must_use]
    pub fn of(dir: &Path, kept: Kept, scenarios: &[Scenario]) -> Self {
        let Kept { document, frames: rows, runs: files } = kept;
        let same_architecture = document.device.architecture == ARCHITECTURE;
        let runs: Vec<RunReport> = document
            .runs
            .iter()
            .zip(files)
            .map(|(run, files)| report_run(run, files, scenarios, same_architecture, &rows))
            .collect();
        let marks = document
            .marks
            .iter()
            .zip(1..)
            .map(|(mark, number)| {
                let run = runs.iter().find(|run| run.number == mark.run);
                let part = run
                    .and_then(|run| run.parts.iter().find(|part| (part.from..=part.to).contains(&mark.tick)));
                let encounter = part.and_then(|part| part.encounter.as_ref()).map(Met::name);
                let file = run.map(|run| run.file.clone());
                let command = file.as_ref().map(|file| view_command(dir, file, mark.tick, number));
                MarkReport {
                    number,
                    run: mark.run,
                    tick: mark.tick,
                    at: mark.at,
                    label: mark.label.clone(),
                    encounter,
                    file,
                    command,
                }
            })
            .collect();
        Self {
            session: dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
            build: document.build,
            device: document.device,
            counted_on: ARCHITECTURE.to_owned(),
            seconds: document.seconds,
            ended: document.ended,
            trips: document.trips.len() as u32,
            settings: document.settings.len() as u32,
            runs,
            marks,
            frames: Frames::of(&rows),
        }
    }

    /// `report.ron`'s text.
    pub fn to_ron(&self) -> Result<String, sapphire_serialize::DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// Reads `report.ron`'s text.
    pub fn from_ron(text: &str) -> Result<Self, sapphire_serialize::DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    /// The report for a person to read.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let Self { build, device, .. } = self;
        let _ = writeln!(
            out,
            "Session {}: {} {} ({}) on {} {}{}, {}, {} cores, {} MiB",
            self.session,
            build.app,
            build.version,
            build.id,
            device.os,
            device.class,
            if device.emulator { " (emulated)" } else { "" },
            device.architecture,
            device.cores,
            device.ram_mib,
        );
        let ended = match self.ended {
            Some(Ending::Quit { .. }) => "quit",
            Some(Ending::Full { .. }) => "full at four hours",
            None => "killed, or still going: as of its last keep",
        };
        let _ = writeln!(
            out,
            "Played {:.1} s, {ended}. {} runs, {} marks, {} settings files, {} trips to the background.",
            self.seconds,
            self.runs.len(),
            self.marks.len(),
            self.settings,
            self.trips,
        );
        if let Some(frames) = &self.frames {
            let _ = writeln!(out, "Frames: {}", frames.line());
        }
        for run in &self.runs {
            out.push('\n');
            run.write(&mut out, &self.device.architecture);
        }
        if !self.marks.is_empty() {
            out.push_str("\nMarks:\n");
        }
        for mark in &self.marks {
            let _ = write!(
                out,
                "  {}. run {}, tick {} ({:.1} s into the session)",
                mark.number, mark.run, mark.tick, mark.at
            );
            if let Some(encounter) = &mark.encounter {
                let _ = write!(out, ", in {encounter}");
            }
            if let Some(label) = &mark.label {
                let _ = write!(out, ": {label:?}");
            }
            out.push('\n');
            match &mark.command {
                Some(command) => {
                    let _ = writeln!(out, "     {command}");
                }
                None => out.push_str("     the session kept no such run\n"),
            }
        }
        out
    }
}

impl RunReport {
    fn write(&self, out: &mut String, architecture: &str) {
        let ended = if self.ended { "ended" } else { "going when last kept" };
        let _ = write!(
            out,
            "Run {}, {}, {} ticks ({:.1} s), {ended}; ",
            self.number,
            self.scenario,
            self.ticks,
            seconds(u64::from(self.ticks))
        );
        match &self.check {
            Check::Same => out.push_str("replays to the device's checksum\n"),
            Check::Differs { reached, ticks } => {
                let _ = writeln!(
                    out,
                    "DIFFERS from the device: reached {reached} in {ticks} ticks, so these counts are this \
                     machine's run, not the player's (`sapphire_replay check --dir` names the tick)"
                );
            }
            Check::OtherArchitecture => {
                let _ = writeln!(out, "played on {architecture}, so not held to its checksums here");
            }
            Check::Unread { reason } => {
                let _ = writeln!(out, "doesn't replay here: {reason}");
                return;
            }
        }
        let _ = writeln!(out, "  whole run: {}", self.whole.line());
        for part in &self.parts {
            let what = match &part.encounter {
                Some(met) => {
                    let end = match met.end {
                        MetEnd::Cleared => "cleared",
                        MetEnd::StartedOver => "started over",
                        MetEnd::RunEnded => "going as the run ended",
                    };
                    format!("{}, {end}", met.name())
                }
                None => "outside any encounter".to_owned(),
            };
            let _ = writeln!(out, "  ticks {}–{} ({:.1} s): {what}", part.from, part.to, part.seconds);
            let _ = writeln!(out, "    {}", part.counts.line());
            if let Some(frames) = &part.frames {
                let _ = writeln!(out, "    frames: {}", frames.line());
            }
        }
    }
}

impl Counts {
    fn line(&self) -> String {
        format!(
            "{} damage in {} hits, {} falls; dealt {} in {} hits; {} dodges, {} perfect, {} counters; {:.2} presses a \
             second, idle {:.1} s",
            self.damage_taken,
            self.hits_taken,
            self.falls,
            self.damage_dealt,
            self.hits_landed,
            self.dodges,
            self.perfect_dodges,
            self.counters,
            self.presses_a_second,
            self.idle_seconds,
        )
    }
}

impl Frames {
    fn line(&self) -> String {
        let mut line = format!(
            "{} s, mean {:.2} ms, worst {:.2} ms, {} of {} late at {:.2} ms; heat {}",
            self.seconds, self.mean_ms, self.worst_ms, self.late, self.frames, self.budget_ms, self.hottest,
        );
        if let Some(headroom) = self.least_headroom {
            let _ = write!(line, ", least headroom {headroom:.2}");
        }
        if self.low_power {
            line.push_str(", Low Power Mode on");
        }
        line
    }
}

impl Met {
    /// Its document's file name, or its ID where this build doesn't hold it.
    fn name(&self) -> String {
        self.document.as_deref().map_or_else(
            || self.id.clone(),
            |document| document.rsplit('/').next().unwrap_or(document).to_owned(),
        )
    }
}

/// What draws run `file`'s frame at `tick`: the playtest build's capture, playing the file to that tick.
fn view_command(dir: &Path, file: &str, tick: u64, number: u32) -> String {
    format!(
        "cargo xtask playtest --platform desktop -- --replay {} --at-tick {tick} --capture mark-{number}.png",
        dir.join(file).display()
    )
}

fn seconds(ticks: u64) -> f64 {
    ticks as f64 / f64::from(TICK_HZ)
}

/// Replays one run and reports it, or says why it couldn't.
fn report_run(
    run: &KeptRun,
    files: Result<RunFiles, String>,
    scenarios: &[Scenario],
    same_architecture: bool,
    rows: &[FrameRow],
) -> RunReport {
    let mut report = RunReport {
        number: run.number,
        scenario: run.scenario.clone(),
        file: run.file.clone(),
        ticks: run.ticks,
        ended: run.ended,
        check: Check::OtherArchitecture,
        whole: Counts::default(),
        parts: Vec::new(),
    };
    let counted = files.and_then(|files| count(files.replay, scenarios));
    let (parts, end, ticks) = match counted {
        Ok(counted) => counted,
        Err(reason) => {
            report.check = Check::Unread { reason };
            return report;
        }
    };
    if same_architecture {
        let reached = format!("{end:016x}");
        report.check = if reached == run.checksum && ticks == run.ticks {
            Check::Same
        } else {
            Check::Differs { reached, ticks }
        };
    }
    report.parts = parts
        .into_iter()
        .map(|part| {
            let frames = Frames::of(
                rows.iter().filter(|row| row.run == run.number && (part.from..=part.to).contains(&row.tick)),
            );
            Part { frames, ..part }
        })
        .collect();
    let mut whole = Counts::default();
    for part in &report.parts {
        whole.add(&part.counts);
    }
    report.whole = whole.finish(seconds(u64::from(report.ticks)));
    report
}

/// Each tick's input, from the file: its presses, and whether it's in an idle stretch.
fn input_ticks(actions: &[ActorActions]) -> Vec<(u32, bool)> {
    let resting = |actions: &ActorActions| {
        actions.buttons.minus(Buttons::TOUCH).is_empty() && actions.stick == Stick::CENTER
    };
    let mut ticks = Vec::with_capacity(actions.len());
    let mut previous = ActorActions::NEUTRAL;
    for actions in actions {
        let down = actions.buttons.minus(previous.buttons).minus(Buttons::TOUCH);
        ticks.push((down.bits().count_ones(), false));
        previous = *actions;
    }
    // Every tick of a resting stretch at least `IDLE_AFTER` long.
    let mut start = 0;
    while start < actions.len() {
        if !resting(&actions[start]) {
            start += 1;
            continue;
        }
        let end = actions[start..]
            .iter()
            .position(|actions| !resting(actions))
            .map_or(actions.len(), |length| start + length);
        if end - start >= IDLE_AFTER as usize {
            for tick in &mut ticks[start..end] {
                tick.1 = true;
            }
        }
        start = end;
    }
    ticks
}

/// One of the player's side, as the last step left it.
#[derive(Clone, Copy, Debug)]
struct Seen {
    actor: ActorId,
    serial: u32,
    counter_open: bool,
}

/// The part being counted.
struct Open {
    from: u64,
    encounter: Option<AssetId>,
    counts: Counts,
    idle_ticks: u64,
}

impl Open {
    fn new(from: u64, encounter: Option<AssetId>) -> Self {
        Self { from, encounter, counts: Counts::default(), idle_ticks: 0 }
    }

    /// The part, ending at `to`, its encounter ended as `end` says. `None` if it holds no tick.
    fn close(self, to: u64, end: MetEnd) -> Option<Part> {
        if to < self.from {
            return None;
        }
        let seconds = seconds(to - self.from + 1);
        let mut counts = self.counts;
        counts.idle_seconds = self.idle_ticks as f64 / f64::from(TICK_HZ);
        Some(Part {
            from: self.from,
            to,
            seconds,
            encounter: self.encounter.map(|id| Met {
                id: id.to_string(),
                document: encounter_path(id).map(str::to_owned),
                end,
            }),
            counts: counts.finish(seconds),
            frames: None,
        })
    }
}

/// The player's side: the first actor, and its party's members.
fn side(sim: &Simulation, first: ActorId, into: &mut Vec<ActorId>) {
    into.clear();
    into.push(first);
    into.extend(sim.party().slots().map(|slot| slot.actor).filter(|&actor| actor != first));
}

/// Plays `replay` to its end, counting as it goes. Gives the parts, the last checksum and the ticks played.
fn count(replay: Replay, scenarios: &[Scenario]) -> Result<(Vec<Part>, u64, u32), String> {
    let mut player = Player::new(replay, scenarios).map_err(|error| error.to_string())?;
    let first = *player.cast().first().ok_or("its scenario drives nobody")?;
    let actions: Vec<ActorActions> = player.replay().actions().map(|tick| tick.get(first)).collect();
    let input = input_ticks(&actions);

    let mut members = Vec::new();
    side(player.simulation(), first, &mut members);
    // What each was playing as the run began, so only what starts after counts.
    let mut seen: Vec<Seen> = members
        .iter()
        .filter_map(|&actor| player.simulation().actor(actor))
        .map(|view| Seen {
            actor: view.id,
            serial: view.moves.current.map_or(0, |current| current.serial),
            counter_open: view.moves.counter_closes_at.is_some(),
        })
        .collect();
    let mut running: Vec<AssetId> = Vec::new();
    let mut parts = Vec::new();
    let mut open = Open::new(1, None);
    let mut counters_before = player.simulation().stats().counters_fired;
    let mut last = 0;
    while player.step() {
        let sim = player.simulation();
        let tick = sim.tick();
        last = tick;

        // Which encounters run, in the order they started; the first owns the time.
        let owner = running.first().copied();
        let encounters = sim.zone().map(|zone| zone.encounters()).unwrap_or_default();
        running.retain(|id| {
            encounters.iter().any(|held| held.id() == *id && held.phase() == EncounterPhase::Running)
        });
        for held in encounters {
            if held.phase() == EncounterPhase::Running && !running.contains(&held.id()) {
                running.push(held.id());
            }
        }
        let now = running.first().copied();
        let mut ended = None;
        if owner != now {
            match owner {
                // Its last step is its own.
                Some(id) => {
                    let cleared = encounters
                        .iter()
                        .any(|held| held.id() == id && held.phase() == EncounterPhase::Cleared);
                    ended = Some(if cleared { MetEnd::Cleared } else { MetEnd::StartedOver });
                }
                // Its first step is its own.
                None => {
                    let previous = std::mem::replace(&mut open, Open::new(tick, now));
                    parts.extend(previous.close(tick - 1, MetEnd::RunEnded));
                }
            }
        }

        let counts = &mut open.counts;
        side(sim, first, &mut members);
        let mut countered = 0;
        for &actor in &members {
            let Some(view) = sim.actor(actor) else { continue };
            let serial = view.moves.current.map_or(0, |current| current.serial);
            let index = seen.iter().position(|seen| seen.actor == actor).unwrap_or_else(|| {
                // A member who joined the side since: whatever it plays now, it started.
                seen.push(Seen { actor, serial: 0, counter_open: false });
                seen.len() - 1
            });
            let before = &mut seen[index];
            if let Some(current) = view.moves.current
                && current.serial != before.serial
            {
                let library = sim.move_library();
                counts.dodges +=
                    u32::from(library.get(current.at).is_some_and(|mv| !mv.perfect_dodge.is_empty()));
                countered += u32::from(before.counter_open);
            }
            before.serial = serial;
            before.counter_open = view.moves.counter_closes_at.is_some();
        }
        let fired = sim.stats().counters_fired;
        counts.counters += countered.min((fired - counters_before) as u32);
        counters_before = fired;
        for cue in sim.cues() {
            match cue.hit {
                Some(HitCue { target, damage, .. }) if members.contains(&target) => {
                    if cue.cue == DEFEATED {
                        counts.falls += 1;
                    } else if cue.cue != POISE_BROKEN {
                        counts.hits_taken += 1;
                        counts.damage_taken += u64::from(damage);
                    }
                }
                Some(HitCue { damage, .. })
                    if members.contains(&cue.actor) && cue.cue != DEFEATED && cue.cue != POISE_BROKEN =>
                {
                    counts.hits_landed += 1;
                    counts.damage_dealt += u64::from(damage);
                }
                None if cue.cue == PERFECT_DODGE && members.contains(&cue.actor) => {
                    counts.perfect_dodges += 1
                }
                _ => {}
            }
        }
        if let Some(&(presses, idle)) = usize::try_from(tick - 1).ok().and_then(|index| input.get(index)) {
            counts.presses += presses;
            open.idle_ticks += u64::from(idle);
        }

        if let Some(end) = ended {
            let previous = std::mem::replace(&mut open, Open::new(tick + 1, now));
            parts.extend(previous.close(tick, end));
        }
    }
    parts.extend(open.close(last, MetEnd::RunEnded));
    let played = player.finish();
    Ok((parts, played.trail.end(), played.trail.ticks()))
}
