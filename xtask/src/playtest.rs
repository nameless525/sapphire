//! `cargo xtask playtest`: a playtest build on a device, and its sessions brought back here (M4 F1).
//!
//! A playtest build is the game (`game_app`) built for release with its `playtest` feature, which keeps a
//! session of each time someone plays it (`sapphire_playtest` has the format) under the player's directory,
//! in `sessions/`. `playtest --platform <p>` builds, installs and launches it as `run` does. `playtest pull`
//! copies every session on the device to `target/playtest/<platform>/sessions/`, whole, and replays each run
//! in it with `sapphire_replay`, which names the first tick where this machine's simulation differs from the
//! device's. `playtest report <session>` replays a session's runs and reads them out per encounter (M4 F2,
//! `sapphire_playtest`'s `report` module): text here, and `report.ron` beside the session.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use anyhow::{Context, bail, ensure};

use crate::cli::Args;
use crate::platforms::{self, App, Options, Platform};
use crate::{replay, workspace};

/// The game, as the playtest builds it.
const GAME: &str = "game_app";

/// The feature that keeps sessions.
const FEATURE: &str = "playtest";

/// `GameApp::METADATA`'s organization and name, which name the player's directory on Apple platforms
/// and desktops (`SDL_GetPrefPath`); a test holds them to `game/game_app/src/app.rs`.
const ORGANIZATION: &str = "Sapphire";
const NAME: &str = "Sapphire";

/// The directory the game keeps sessions in, under the player's (`sapphire_playtest::SESSIONS_DIR`).
const SESSIONS: &str = "sessions";

/// A session's runs, each in a directory of its own (`sapphire_playtest::RUNS_DIR`).
const RUNS: &str = "runs";

/// A session's document (`sapphire_playtest::DOCUMENT_FILE`).
const DOCUMENT: &str = "session.ron";

/// What `cargo xtask playtest` was asked to do.
#[derive(Debug)]
pub enum Command {
    /// Build, install and launch, with these arguments for the game.
    Install { platform: Platform, options: Options, args: Vec<String> },
    /// Bring every session back, and replay each run unless `replay` is false.
    Pull(Pull),
    /// Read a session out: the directory holding its `session.ron`.
    Report { session: PathBuf },
}

/// Where `playtest pull` takes sessions from.
#[derive(Debug, PartialEq, Eq)]
pub struct Pull {
    pub platform: Platform,
    /// The player's directory, instead of the platform's (desktop, or wherever `--save-dir` put them).
    pub from: Option<PathBuf>,
    /// ios-sim: the simulator (default: the booted one). android: the serial (default: `ANDROID_SERIAL`,
    /// else the one device). ios: the device, by name or UDID.
    pub device: Option<String>,
    pub replay: bool,
}

impl Command {
    pub fn parse(mut args: Args) -> anyhow::Result<Self> {
        if args.flag("report") {
            let session = args.command().context("playtest report needs a session's directory")?;
            args.finish(false)?;
            return Ok(Self::Report { session: PathBuf::from(session) });
        }
        let pull = args.flag("pull");
        let platform = args.required::<Platform>("--platform")?;
        if pull {
            let from = args.value::<PathBuf>("--from")?;
            let device = args.value::<String>("--device")?;
            let replay = !args.flag("--no-replay");
            args.finish(false)?;
            ensure!(
                from.is_none() || platform == Platform::Desktop,
                "--from applies only to --platform desktop"
            );
            ensure!(
                device.is_none() || platform != Platform::Desktop,
                "a desktop's sessions are on this machine"
            );
            return Ok(Self::Pull(Pull { platform, from, device, replay }));
        }
        let features = args.value::<String>("--features")?.unwrap_or_default();
        let mut features: Vec<String> =
            features.split(',').map(str::trim).filter(|f| !f.is_empty()).map(str::to_owned).collect();
        if !features.iter().any(|f| f == FEATURE) {
            features.push(FEATURE.to_owned());
        }
        let options = Options {
            release: true,
            features,
            identity: args.value::<String>("--identity")?,
            profile: args.value::<PathBuf>("--profile")?,
            device: args.value::<String>("--device")?,
            dry_run: args.flag("--dry-run"),
            ..Options::default()
        };
        let args = args.finish(true)?;
        Ok(Self::Install { platform, options, args })
    }
}

pub fn run(command: &Command) -> anyhow::Result<()> {
    match command {
        Command::Install { platform, options, args } => {
            platforms::run(*platform, &App::load(GAME)?, options, args)
        }
        Command::Pull(pull) => {
            let dest = workspace::output_dir().join("playtest").join(pull.platform.name());
            let pulled = bring_back(pull, &dest.join(SESSIONS))?;
            if pulled.is_empty() {
                eprintln!("no sessions on {}", pull.platform);
                return Ok(());
            }
            if pull.replay { replay_all(&pulled, &dest.join("checks")) } else { Ok(()) }
        }
        Command::Report { session } => {
            ensure!(
                session.join(DOCUMENT).is_file(),
                "{} holds no {DOCUMENT}: name a session's directory, as `playtest pull` leaves them under \
                 target/playtest/<platform>/sessions/",
                session.display()
            );
            workspace::run(Process::new(replay::runner()?).arg("report").arg("--session").arg(session))
        }
    }
}

/// Copies every session to `dest`, each replacing what an earlier pull brought of it, and gives their
/// directories, oldest first.
fn bring_back(pull: &Pull, dest: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let app = App::load(GAME)?;
    // Each comes into a fresh directory, and moves over what's there once it's whole.
    let incoming = dest.with_file_name("incoming");
    workspace::remove_dir_if_exists(&incoming)?;
    fs::create_dir_all(&incoming).with_context(|| format!("creating {}", incoming.display()))?;
    match pull.platform {
        Platform::Desktop | Platform::MacOs => {
            let from = match &pull.from {
                Some(from) => from.clone(),
                None => desktop_user_dir()?,
            };
            let sessions = from.join(SESSIONS);
            if sessions.is_dir() {
                workspace::copy_dir(&sessions, &incoming)?;
            }
        }
        Platform::IosSim => {
            let device = pull.device.as_deref().unwrap_or("booted");
            let container = workspace::output(Process::new("xcrun").args([
                "simctl",
                "get_app_container",
                device,
                &app.bundle_id,
                "data",
            ]))?;
            let sessions = apple_user_dir(Path::new(container.trim())).join(SESSIONS);
            if sessions.is_dir() {
                workspace::copy_dir(&sessions, &incoming)?;
            }
        }
        Platform::Android => {
            // The game's files are its own: `run-as` reads them in a debuggable build, which a playtest's is.
            let program = platforms::adb()?;
            let adb = || {
                let mut adb = Process::new(&program);
                if let Some(serial) = &pull.device {
                    adb.args(["-s", serial]);
                }
                adb
            };
            let listing = workspace::output(adb().args(["shell", "run-as", &app.bundle_id, "ls", "files"]))?;
            if listing.lines().any(|line| line.trim() == SESSIONS) {
                let archive = incoming.with_extension("tar");
                let tar = adb()
                    .args(["exec-out", "run-as", &app.bundle_id, "tar", "-c", "-C", "files", SESSIONS])
                    .output()
                    .context("failed to run adb")?;
                ensure!(
                    tar.status.success(),
                    "adb exec-out tar failed: {}",
                    String::from_utf8_lossy(&tar.stderr)
                );
                fs::write(&archive, &tar.stdout).with_context(|| format!("writing {}", archive.display()))?;
                workspace::run(
                    Process::new("tar").arg("-x").arg("-f").arg(&archive).arg("-C").arg(&incoming),
                )?;
                fs::remove_file(&archive).ok();
                let unpacked = incoming.join(SESSIONS);
                for entry in read_dir_sorted(&unpacked)? {
                    let name = entry.file_name().context("a session has a name")?;
                    fs::rename(&entry, incoming.join(name))
                        .with_context(|| format!("moving {}", entry.display()))?;
                }
                fs::remove_dir(&unpacked).ok();
            }
        }
        Platform::Ios => {
            // Owed a device: written to Apple's documentation of `devicectl`, and never run here.
            let device = pull.device.as_deref().context("--platform ios needs --device <name or UDID>")?;
            let source =
                Path::new("Library/Application Support").join(ORGANIZATION).join(NAME).join(SESSIONS);
            workspace::run(
                Process::new("xcrun")
                    .args(["devicectl", "device", "copy", "from", "--device", device])
                    .args(["--domain-type", "appDataContainer", "--domain-identifier", &app.bundle_id])
                    .arg("--source")
                    .arg(&source)
                    .arg("--destination")
                    .arg(&incoming),
            )?;
        }
    }

    fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut pulled = Vec::new();
    for session in read_dir_sorted(&incoming)? {
        if !session.is_dir() {
            continue;
        }
        let name = session.file_name().context("a session has a name")?;
        let to = dest.join(name);
        workspace::remove_dir_if_exists(&to)?;
        fs::rename(&session, &to)
            .with_context(|| format!("moving {} to {}", session.display(), to.display()))?;
        eprintln!("pulled {}", to.display());
        pulled.push(to);
    }
    workspace::remove_dir_if_exists(&incoming)?;
    Ok(pulled)
}

/// Replays every run of every session in `sessions`, writing what the runner makes under `checks`, and
/// fails if any run doesn't reach the device's checksums here.
fn replay_all(sessions: &[PathBuf], checks: &Path) -> anyhow::Result<()> {
    let runner = replay::runner()?;
    let checksums = format!("checksums.{}.txt", std::env::consts::ARCH);
    let mut played = 0_usize;
    let mut failed = Vec::new();
    for session in sessions {
        let name = session.file_name().context("a session has a name")?;
        for run in read_dir_sorted(&session.join(RUNS)).unwrap_or_default() {
            if !run.is_dir() {
                continue;
            }
            let label = format!(
                "{}/{RUNS}/{}",
                name.to_string_lossy(),
                run.file_name().unwrap_or_default().to_string_lossy()
            );
            if !run.join(&checksums).is_file() {
                let recorded = recorded_on(&run);
                bail!(
                    "{label} was recorded on {}, and this machine is {}: replay it on a host of the device's \
                     architecture",
                    recorded.join(", "),
                    std::env::consts::ARCH
                );
            }
            let out = checks.join(&label);
            let status = Process::new(&runner)
                .arg("check")
                .arg("--dir")
                .arg(&run)
                .arg("--out")
                .arg(&out)
                .status()
                .with_context(|| format!("failed to run {}", runner.display()))?;
            played += 1;
            if !status.success() {
                failed.push(label);
            }
        }
    }
    ensure!(
        failed.is_empty(),
        "{} of {played} runs differ from the device's: {}",
        failed.len(),
        failed.join(", ")
    );
    eprintln!("{played} runs replay to the device's checksums");
    Ok(())
}

/// The architectures a run's checksums name.
fn recorded_on(run: &Path) -> Vec<String> {
    read_dir_sorted(run)
        .unwrap_or_default()
        .iter()
        .filter_map(|path| {
            path.file_name()?.to_str()?.strip_prefix("checksums.")?.strip_suffix(".txt").map(str::to_owned)
        })
        .collect()
}

fn read_dir_sorted(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    paths.sort();
    Ok(paths)
}

/// The player's directory under an Apple app's data container, as `SDL_GetPrefPath` makes it.
fn apple_user_dir(container: &Path) -> PathBuf {
    container.join("Library/Application Support").join(ORGANIZATION).join(NAME)
}

/// The game's player directory on this machine, as `SDL_GetPrefPath` makes it.
fn desktop_user_dir() -> anyhow::Result<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    desktop_user_dir_for(std::env::consts::OS, var)
}

fn desktop_user_dir_for(os: &str, var: impl Fn(&str) -> Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let home = || var("HOME").context("HOME is not set");
    let base = match os {
        "macos" => home()?.join("Library/Application Support"),
        "windows" => var("APPDATA").context("APPDATA is not set")?,
        _ => match var("XDG_DATA_HOME") {
            Some(data) => data,
            None => home()?.join(".local/share"),
        },
    };
    Ok(base.join(ORGANIZATION).join(NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> anyhow::Result<Command> {
        Command::parse(Args::new(line.split_whitespace().map(str::to_owned)))
    }

    #[test]
    fn it_installs_a_release_build_that_keeps_sessions() {
        let Command::Install { platform, options, args } =
            parse("--platform android --features dev -- --new-game").unwrap()
        else {
            panic!("not an install");
        };
        assert_eq!(platform, Platform::Android);
        assert!(options.release);
        assert_eq!(options.features, ["dev", FEATURE]);
        assert_eq!(args, ["--new-game"]);
        let Command::Install { options, .. } = parse("--platform desktop --features playtest").unwrap()
        else {
            panic!("not an install");
        };
        assert_eq!(options.features, [FEATURE]);
    }

    fn pull(line: &str) -> anyhow::Result<Pull> {
        match parse(line)? {
            Command::Pull(pull) => Ok(pull),
            Command::Install { .. } | Command::Report { .. } => anyhow::bail!("not a pull"),
        }
    }

    #[test]
    fn it_pulls() {
        assert_eq!(
            pull("pull --platform ios-sim --device ABCD --no-replay").unwrap(),
            Pull { platform: Platform::IosSim, from: None, device: Some("ABCD".to_owned()), replay: false }
        );
        assert_eq!(
            pull("pull --platform desktop --from saves").unwrap(),
            Pull {
                platform: Platform::Desktop,
                from: Some(PathBuf::from("saves")),
                device: None,
                replay: true
            }
        );
        assert!(parse("pull --platform android --from saves").is_err());
        assert!(parse("pull --platform desktop --device x").is_err());
        assert!(parse("pull --platform android -- --new-game").is_err());
    }

    #[test]
    fn it_reports_a_session() {
        let Command::Report { session } = parse("report target/playtest/desktop/sessions/s").unwrap() else {
            panic!("not a report");
        };
        assert_eq!(session, Path::new("target/playtest/desktop/sessions/s"));
        assert!(parse("report").is_err(), "which session?");
        assert!(parse("report a b").is_err());
    }

    #[test]
    fn it_finds_the_players_directory_as_sdl_does() {
        let var = |name: &str| match name {
            "HOME" => Some(PathBuf::from("/home/user")),
            "APPDATA" => Some(PathBuf::from("D:/roaming")),
            _ => None,
        };
        assert_eq!(
            desktop_user_dir_for("macos", var).unwrap(),
            Path::new("/home/user/Library/Application Support/Sapphire/Sapphire")
        );
        assert_eq!(
            desktop_user_dir_for("linux", var).unwrap(),
            Path::new("/home/user/.local/share/Sapphire/Sapphire")
        );
        assert_eq!(desktop_user_dir_for("windows", var).unwrap(), Path::new("D:/roaming/Sapphire/Sapphire"));
        assert_eq!(
            apple_user_dir(Path::new("/c")),
            Path::new("/c/Library/Application Support/Sapphire/Sapphire")
        );
    }

    #[test]
    fn the_names_are_the_games() {
        let app = fs::read_to_string(workspace::root().join("game/game_app/src/app.rs")).unwrap();
        assert!(app.contains(&format!("name: \"{NAME}\",")), "GameApp::METADATA's name moved");
        assert!(
            app.contains(&format!("organization: \"{ORGANIZATION}\",")),
            "GameApp::METADATA's organization moved"
        );
        let lib = fs::read_to_string(workspace::root().join("crates/sapphire_playtest/src/lib.rs")).unwrap();
        assert!(lib.contains(&format!("SESSIONS_DIR: &str = \"{SESSIONS}\"")));
        assert!(lib.contains(&format!("RUNS_DIR: &str = \"{RUNS}\"")));
        assert!(lib.contains(&format!("DOCUMENT_FILE: &str = \"{DOCUMENT}\"")));
    }
}
