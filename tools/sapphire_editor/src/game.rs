//! Play in game (M6 T19): the `dev` sandbox started as a child process on a zone or a scenario, which the editor's
//! live link then talks to, so every save reaches a running game.
//!
//! A child, not a run on the editor's own stage, for two reasons. A graph, a cue table or a set of effects reaches a
//! fight only through live tuning, which is `game_combat`'s `dev` feature, and the editor can't turn that on without
//! turning it on in every workspace build that holds the editor ([`crate::play`] says so of the in-process run, which
//! takes moves alone). And a game that crashes takes only itself down: the editor's documents, their undo and the
//! journal are in another process, so they are as they were, and the game's log says what happened.
//!
//! **The port.** The child listens on `127.0.0.1:0`, so the OS picks a free port as it binds and holds it from that
//! moment, and writes the address it got to a file in the game's folder (the sandbox's `--live-link-address`), which
//! the editor reads to point its link there. Two editors, or an editor and a device on 7811, never meet: no port is
//! chosen before it's bound. A tool reaches the same child at that address, `cargo xtask link capture --addr <it>`
//! among them, and the device pane shows it.
//!
//! What a game is started with is a [`Launcher`]'s: the dev sandbox ([`Sandbox`]), always `--offscreen` (never a
//! window), with hot reload off so a push is the only way a save gets there, or a test's stand-in.

use std::fmt;
use std::fs::{self, File};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use sapphire_document::Kind;
use serde::{Deserialize, Serialize};

/// The size the game draws at, offscreen: small, since nobody looks at it but a capture.
pub const GAME_SIZE: &str = "640x360";
/// How many of its log's last lines a game that stopped by itself shows.
pub const LOG_LINES: usize = 40;
/// The most of its log's end that's read for those lines.
const LOG_TAIL_BYTES: u64 = 64 << 10;
/// The file in a game's folder its link's address is written to.
const ADDRESS_FILE: &str = "link.address";
/// Its log, in the same folder.
const LOG_FILE: &str = "game.log";

/// What a game plays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameTarget {
    /// A zone, by its stem, from its player start: `--zone <stem>`, with no fight.
    Zone(String),
    /// A scenario's fight, with its cues, by a script of the game's (`attempts`, say): `--record --scenario <name>
    /// --script <script> --cues`, recorded into the game's folder. An offscreen fight has no input but a script.
    Scenario { name: String, script: String },
    /// The arena's fight, the playable character against an enemy, with its cues, by a script of the game's:
    /// `--arena --fight --script <script> --cues`. Both fighters' graphs take edits there, which a recorded
    /// scenario's fight, on its bound stock content, refuses.
    Arena { script: String },
}

impl fmt::Display for GameTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zone(stem) => write!(f, "zone {stem}"),
            Self::Scenario { name, script } => write!(f, "scenario {name} by {script}"),
            Self::Arena { script } => write!(f, "the arena's fight by {script}"),
        }
    }
}

/// Whether a saved document of this kind is pushed to a game as its text: the kinds a game's live tuning takes by
/// name (a fight's moves, graphs, effects and cue table, a screen, the day and the light rigs). Every other kind (a zone, a
/// material, a prefab) reaches a game as a cook's packs, which the editor pushes once its cook lands.
#[must_use]
pub const fn takes_live(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Moves | Kind::Graph | Kind::Effects | Kind::CueTable | Kind::Ui | Kind::Day | Kind::LightRigs
    )
}

/// What starts a game: the dev sandbox ([`Sandbox`]), or a test's stand-in.
pub trait Launcher: fmt::Debug {
    /// The command that starts a game playing `target`, with no window, whose live link listens on a port the OS
    /// picks and writes its address to `address` once it does. What the game writes of its own (a recording) goes
    /// in `folder`.
    fn command(&self, target: &GameTarget, address: &Path, folder: &Path) -> Result<Command, String>;
}

/// The `dev` sandbox, on a cook's packs.
#[derive(Clone, Debug)]
pub struct Sandbox {
    binary: PathBuf,
    /// The packs it reads: the editor's cook, so a pack pushed after a save's cook is one it has a version of.
    content: Option<PathBuf>,
}

impl Sandbox {
    #[must_use]
    pub const fn new(binary: PathBuf, content: Option<PathBuf>) -> Self {
        Self { binary, content }
    }

    /// The sandbox beside the editor's own binary, where cargo puts `sapphire_sandbox` built with `--features dev`
    /// in the same profile.
    #[must_use]
    pub fn beside_editor() -> Option<PathBuf> {
        let editor = std::env::current_exe().ok()?;
        Some(editor.parent()?.join(format!("sapphire_sandbox{}", std::env::consts::EXE_SUFFIX)))
    }
}

impl Launcher for Sandbox {
    fn command(&self, target: &GameTarget, address: &Path, folder: &Path) -> Result<Command, String> {
        if !self.binary.is_file() {
            return Err(format!(
                "no dev sandbox at {} to play in: `cargo build -p sapphire_sandbox --features dev` builds it",
                self.binary.display()
            ));
        }
        let mut command = Command::new(&self.binary);
        command
            .args(["--offscreen", "--size", GAME_SIZE, "--no-hot-reload", "--quit-on-stdin-eof"])
            .args(["--live-link", "listen=127.0.0.1:0", "--live-link-address"])
            .arg(address);
        match target {
            GameTarget::Zone(stem) => {
                command.args(["--zone", stem]);
            }
            GameTarget::Scenario { name, script } => {
                command.arg("--record").arg(folder.join("play.sreplay"));
                command.args(["--scenario", name, "--script", script, "--cues"]);
            }
            GameTarget::Arena { script } => {
                command.args(["--arena", "--fight", "--script", script, "--cues"]);
            }
        }
        if let Some(content) = &self.content {
            command.env("SAPPHIRE_CONTENT_DIR", content);
        }
        Ok(command)
    }
}

/// A game that stopped by itself: how, and the end of its log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameDied {
    pub target: GameTarget,
    /// Its exit status, in the OS's words.
    pub status: String,
    /// Its log's last [`LOG_LINES`] lines.
    pub log: Vec<String>,
    /// Where the whole log is: its folder is kept until the next Play ([`GameDied::forget`]).
    pub log_path: PathBuf,
}

impl GameDied {
    /// Takes its folder, log and all, away.
    pub fn forget(self) {
        if let Some(folder) = self.log_path.parent() {
            let _ = fs::remove_dir_all(folder);
        }
    }
}

/// A game running as a child process.
#[derive(Debug)]
pub struct Game {
    target: GameTarget,
    child: Child,
    /// Its standard input, which it quits at the end of (`--quit-on-stdin-eof`): held open until this goes, so the
    /// game goes when the editor does, even when the editor's process dies without unwinding.
    input: Option<ChildStdin>,
    /// Its log, its address and whatever it records: taken away when this goes, kept when it dies by itself.
    folder: PathBuf,
    keep_folder: bool,
    address: Option<SocketAddr>,
}

impl Game {
    /// Starts a game playing `target` by `launcher`'s command, with its output in a log in a folder of its own.
    pub fn start(launcher: &dyn Launcher, target: GameTarget) -> Result<Self, String> {
        let folder = new_folder()?;
        let started = (|| {
            let log =
                File::create(folder.join(LOG_FILE)).map_err(|error| format!("the game's log: {error}"))?;
            let mut command = launcher.command(&target, &folder.join(ADDRESS_FILE), &folder)?;
            let errors = log.try_clone().map_err(|error| format!("the game's log: {error}"))?;
            command.stdin(Stdio::piped()).stdout(log).stderr(errors);
            command.spawn().map_err(|error| format!("starting the game: {error}"))
        })();
        match started {
            Ok(mut child) => {
                tracing::info!(pid = child.id(), %target, folder = %folder.display(), "the game started");
                let input = child.stdin.take();
                Ok(Self { target, child, input, folder, keep_folder: false, address: None })
            }
            Err(why) => {
                let _ = fs::remove_dir_all(&folder);
                Err(why)
            }
        }
    }

    #[must_use]
    pub const fn target(&self) -> &GameTarget {
        &self.target
    }

    /// Its process's ID.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Where its whole log is.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.folder.join(LOG_FILE)
    }

    /// Where its live link listens, once it has said.
    pub fn address(&mut self) -> Option<SocketAddr> {
        if self.address.is_none() {
            // Written whole through a rename, so a read sees all of it or no file.
            self.address = fs::read_to_string(self.folder.join(ADDRESS_FILE)).ok()?.trim().parse().ok();
        }
        self.address
    }

    /// Whether it has stopped by itself, and if it has, how and the end of its log. It's gone after that.
    pub fn poll(&mut self) -> Option<GameDied> {
        let status = match self.child.try_wait() {
            Ok(None) => return None,
            Ok(Some(status)) => status.to_string(),
            Err(error) => format!("lost: {error}"),
        };
        tracing::warn!(pid = self.child.id(), %status, log = %self.log_path().display(), "the game stopped by itself");
        self.keep_folder = true;
        Some(GameDied {
            target: self.target.clone(),
            status,
            log: log_tail(&self.log_path(), LOG_LINES),
            log_path: self.log_path(),
        })
    }

    /// Stops it: kills it, waits for it to be gone and takes its folder away.
    pub fn stop(self) {
        drop(self);
    }

    /// Closes its standard input, as the editor's process ending does: a game quits at that by itself.
    pub fn close_input(&mut self) {
        self.input = None;
    }

    fn kill(&mut self) {
        if matches!(self.child.try_wait(), Ok(None))
            && let Err(error) = self.child.kill()
        {
            tracing::warn!(pid = self.child.id(), %error, "couldn't stop the game");
        }
        let _ = self.child.wait();
    }
}

impl Drop for Game {
    /// A game never outlives what started it, however that ends, short of the editor's own process dying unwound.
    fn drop(&mut self) {
        self.input = None;
        self.kill();
        if !self.keep_folder {
            let _ = fs::remove_dir_all(&self.folder);
        }
    }
}

/// A folder of its own for a game: `<temp>/sapphire_editor_games/<editor's pid>-<n>`.
fn new_folder() -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let folder =
        std::env::temp_dir().join("sapphire_editor_games").join(format!("{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    Ok(folder)
}

/// The last `lines` lines of the log at `path`, read from at most its last [`LOG_TAIL_BYTES`].
fn log_tail(path: &Path, lines: usize) -> Vec<String> {
    let read = || -> std::io::Result<String> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    match read() {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].iter().map(|line| (*line).to_owned()).collect()
        }
        Err(error) => vec![format!("{}: {error}", path.display())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_plays_offscreen_on_a_port_the_os_picks() {
        let binary = std::env::current_exe().expect("this test's binary");
        let sandbox = Sandbox::new(binary, Some(PathBuf::from("packs")));
        let folder = Path::new("game");
        let args = |target| -> Vec<String> {
            let command = sandbox.command(&target, &folder.join("link.address"), folder).expect("a command");
            command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect()
        };
        let fight = args(GameTarget::Scenario { name: "duel".to_owned(), script: "attempts".to_owned() });
        let wanted =
            ["--offscreen", "--no-hot-reload", "--quit-on-stdin-eof", "listen=127.0.0.1:0", "--cues", "duel"];
        for wanted in wanted.into_iter().chain(["attempts"]) {
            assert!(fight.iter().any(|arg| arg == wanted), "{wanted} in {fight:?}");
        }
        assert!(!fight.iter().any(|arg| arg == "--show"), "never a window");
        let zone = args(GameTarget::Zone("lantern_ward".to_owned()));
        assert!(zone.windows(2).any(|pair| pair == ["--zone", "lantern_ward"]), "{zone:?}");
        assert!(!zone.iter().any(|arg| arg == "--cues" || arg == "--record"), "no fight: {zone:?}");
        let arena = args(GameTarget::Arena { script: "attempts".to_owned() });
        for wanted in ["--arena", "--fight", "--cues", "attempts"] {
            assert!(arena.iter().any(|arg| arg == wanted), "{wanted} in {arena:?}");
        }
        assert!(!arena.iter().any(|arg| arg == "--record"), "nothing recorded: {arena:?}");

        let missing = Sandbox::new(PathBuf::from("no/sandbox/here"), None);
        let why =
            missing.command(&GameTarget::Zone("z".to_owned()), Path::new("a"), folder).expect_err("none");
        assert!(why.contains("--features dev"), "{why}");
    }

    #[test]
    fn a_log_shows_its_last_lines() {
        let folder = new_folder().expect("a folder");
        let path = folder.join(LOG_FILE);
        let text: String = (0..100).map(|line| format!("line {line}\n")).collect();
        fs::write(&path, text).expect("a log");
        let tail = log_tail(&path, 3);
        assert_eq!(tail, ["line 97", "line 98", "line 99"]);
        assert!(log_tail(&folder.join("none"), 3)[0].contains("none"));
        fs::remove_dir_all(folder).expect("cleanup");
    }
}
