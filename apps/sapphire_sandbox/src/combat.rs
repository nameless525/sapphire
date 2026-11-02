//! `--replay` and `--record`: a combat simulation stepping beside the scene, one tick per fixed step
//! that isn't paused.
//!
//! The simulation has nothing to show yet, so what this holds now is the property everything later
//! leans on: a run inside the app, with the renderer beside it, reaches the same checksums as the
//! headless runner, through the same `sapphire_gameplay::replay` code. `--replay` compares with the
//! recorded checksums next to the file, where there are any, and fails the run if it differs, so a
//! golden or a bench from a recorded fight can't come from another fight.
//!
//! **Where a recording's actions come from:** [`action_source`], which returns an
//! [`ActionSource`]: a named script, with `--capture` or `--bench`, or in the windowed app the
//! bindings of whoever's playing, on any platform (`--record <file> --scenario <name>`). So the file
//! holds what was played, with each stick turned by the camera it was played under, and it's written
//! when the app quits. Everything before that is in memory, so a killed process leaves no file.
//!
//! `--replay` needs `--capture` or `--bench`: there's no fight to show in a window yet.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_gameplay::replay::{
    ARCHITECTURE, ActionSource, ChecksumFile, Divergence, Played, Player, RecordedTrail, Recording, Replay,
    ReplayError, SCENARIOS, SCRIPTS, checksums_file_name, find_scenario, find_script,
};
use sapphire_gameplay::{ActorId, TickActions};

use crate::LOG_TARGET;
use crate::options::{CaptureFrames, CombatOption};

/// A script's actions: a function of the tick.
type ScriptActions = fn(u64, &[ActorId]) -> TickActions;

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
        /// The script it plays. With none, it records the bindings it's given.
        script: Option<ScriptActions>,
        /// The tick limit, once reached.
        stopped: Option<ReplayError>,
    },
}

/// A simulation run alongside the scene.
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
            CombatOption::Record { path, scenario, script } => {
                Self::recording(path, scenario, script.as_deref())
            }
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

    fn recording(path: &Path, scenario: &str, script: Option<&str>) -> Result<Self, String> {
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
        let script = script
            .map(|name| {
                find_script(name).map(|script| script.actions).ok_or_else(|| {
                    let known: Vec<&str> = SCRIPTS.iter().map(|script| script.name).collect();
                    format!("--script: no script `{name}`; there's {known:?}")
                })
            })
            .transpose()?;
        let recording =
            Recording::new(scenario, scenario.config).map_err(|error| format!("--record: {error}"))?;
        let mode = Mode::Recording { recording, script, stopped: None };
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
    /// limit, it does nothing and counts. Returns whether it asked `bindings`.
    pub fn step(&mut self, bindings: &mut dyn ActionSource) -> bool {
        match &mut self.mode {
            Mode::Replaying { player, .. } => {
                if !player.step() {
                    self.steps_past_end += 1;
                }
                false
            }
            Mode::Recording { recording, script, stopped } => {
                if stopped.is_some() {
                    return false;
                }
                let asks = script.is_none();
                if let Err(error) = recording.step(action_source(script, bindings)) {
                    *stopped = Some(error);
                }
                asks
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

/// Asks `bindings` for one fixed step's input, exactly once: a recording of the player asks them
/// itself, and otherwise this does, because asking is what moves a press along. `player` is who a
/// step with no recording names.
pub(crate) fn read_bindings(combat: Option<&mut Combat>, bindings: &mut dyn ActionSource, player: ActorId) {
    let asked = combat.is_some_and(|combat| combat.step(bindings));
    if !asked {
        bindings.actions(0, &[player]);
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
