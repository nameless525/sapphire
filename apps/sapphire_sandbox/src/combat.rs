//! `--replay` and `--record`: a combat simulation stepping beside the scene, one tick per fixed step.
//!
//! The simulation has nothing to show yet, so what this holds now is the property everything later
//! leans on: a run inside the app, with the renderer beside it, reaches the same checksums as the
//! headless runner, through the same `sapphire_gameplay::replay` code. `--replay` compares with the
//! recorded checksums next to the file, where there are any, and fails the run if it differs, so a
//! golden or a bench from a recorded fight can't come from another fight.
//!
//! **Where a recording's actions come from:** [`action_source`], which returns an
//! [`ActionSource`]. Today that's a named script; the app's own bindings go there too.
//!
//! Both flags need `--capture` or `--bench`, so Android, whose windowed app refuses those, refuses
//! these as well, and nothing here runs outside an offscreen mode yet.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_gameplay::replay::{
    ARCHITECTURE, ActionSource, ChecksumFile, Divergence, Played, Player, RecordedTrail, Recording, Replay,
    ReplayError, SCENARIOS, SCRIPTS, Script, checksums_file_name, find_scenario, find_script,
};

use crate::LOG_TARGET;
use crate::options::{CaptureFrames, CombatOption};

/// Where a recording's actions come from: the one seam between what drives a fight and the file.
///
/// A script's function is a source. So is anything else with the same shape, such as a live player's
/// bindings read a tick at a time.
fn action_source(script: &Script) -> Box<dyn ActionSource> {
    Box::new(script.actions)
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
        source: Box<dyn ActionSource>,
        /// The tick limit, once reached.
        stopped: Option<ReplayError>,
    },
}

/// A simulation run alongside an offscreen mode.
pub(crate) struct Combat {
    /// The replay file, read or to write.
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
            CombatOption::Record { path, scenario, script } => Self::recording(path, scenario, script),
        }
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

    fn recording(path: &Path, scenario: &str, script: &str) -> Result<Self, String> {
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
        let Some(script) = find_script(script) else {
            let known: Vec<&str> = SCRIPTS.iter().map(|script| script.name).collect();
            return Err(format!("--script: no script `{script}`; there's {known:?}"));
        };
        let recording =
            Recording::new(scenario, scenario.config).map_err(|error| format!("--record: {error}"))?;
        let mode = Mode::Recording { recording, source: action_source(script), stopped: None };
        Ok(Self { path: path.to_owned(), mode, steps_past_end: 0 })
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
    /// limit, it does nothing and counts.
    pub fn step(&mut self) {
        match &mut self.mode {
            Mode::Replaying { player, .. } => {
                if !player.step() {
                    self.steps_past_end += 1;
                }
            }
            Mode::Recording { recording, source, stopped } => {
                if stopped.is_none()
                    && let Err(error) = recording.step(source.as_mut())
                {
                    *stopped = Some(error);
                }
            }
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
        }
        Ok(())
    }
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
