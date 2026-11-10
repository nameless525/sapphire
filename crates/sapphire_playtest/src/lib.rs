//! A playtest's session on disk (M4 F1): what a playtest build keeps of each time someone plays it, and the
//! format a host reads it back in.
//!
//! A playtest build is the shipping game built with its `playtest` feature, which is all that links this
//! crate (`xtask/layers.toml` has it `dev_only`, so no runtime crate can, and a store build can't hold it).
//! The game makes a [`Session`] as it starts and tells it what happens: each frame's time and the OS's heat
//! reading, each run as it ends and, every few seconds, as far as it's got, the player's settings as they
//! change, the pause's marks and trips to the background. The session keeps it all in memory, bounded, and
//! writes it every [`KEEP_EVERY`] and at once after anything a person did, each file whole through the
//! caller's `replace` (`sapphire_platform::durable_replace` in the game) on a job. So a process killed at any
//! moment leaves every file as the last keep wrote it, whole, and loses at most the seconds since.
//!
//! # The format
//!
//! Under the game's directory for the player's files (`user://`, or `--save-dir`), `sessions/` holds one
//! directory per session, named by the device's calendar as it began, in UTC (`2026-11-10-210433`), or
//! `session-<n>` where the device has no calendar. A session runs from the app's start to its quit, its kill
//! or [`MAX_SECONDS`] of play. The oldest go once more than [`MAX_SESSIONS`] are there.
//!
//! - `session.ron`: a [`SessionDocument`], rewritten whole at each keep. The build and the device, when it
//!   began, the seconds played as of this keep, how it ended (nothing for one still going or killed), each
//!   run, each mark, each settings file and each trip to the background. Every `at` is seconds on the session's
//!   own clock: the frames' times summed, which stops while the app is away.
//! - `frames.csv`: a header ([`FRAMES_HEADER`]), then one [`FrameRow`] a second of play: when it ended, the run
//!   and its tick, how many frames, their mean and longest times, how many were late (longer than one and a
//!   half of the tier's interval), the interval, and the OS's heat reading at its end. At most [`MAX_SECONDS`]
//!   rows.
//! - `settings-<n>.ron`: the player's settings as the game keeps them (a settings document), at the start (`0`)
//!   and after each change, which `session.ron`'s `settings` names with its time, run and tick.
//! - `runs/<n>/`: the run numbered `n`, from 1: `<scenario>-<n>.sreplay`, the run's file as `--record` writes
//!   one, and `checksums.<architecture>.txt`, the checksums the run reached on the device, as `tests/replays/`
//!   keeps them. So `sapphire_replay check --dir runs/<n> --out <dir>` plays the run here and names the first
//!   tick where it differs from the device's (`cargo xtask playtest pull` brings sessions back to do it). A
//!   run that hasn't ended is kept as far as it had got at the last keep. A run with no ticks (the title's) is
//!   kept nowhere.
//!
//! A mark is a run's number and a tick of it: replay the run's file to that tick to see the moment.
//!
//! **Bounds.** [`MAX_SECONDS`] of play, after which the session stops keeping and says so; at most
//! [`MAX_RUNS`] runs, [`MAX_MARKS`] marks, [`MAX_SETTINGS`] settings files and [`MAX_TRIPS`] trips, past which
//! [`Dropped`] counts what wasn't kept; labels of at most [`MAX_LABEL`] characters. A run's file is a replay's,
//! which takes at most `MAX_TICKS`, and the clock steps at most 60 ticks a second, so a session's runs take at
//! most [`MAX_SECONDS`] of ticks between them.
//!
//! **Not here:** what a session says per encounter, which `xtask playtest report` reads out of it (F2), and
//! putting a playtest build on a device or bringing its sessions back (`xtask playtest`).

mod document;
mod frames;
mod session;
mod writer;

use std::time::Duration;

pub use document::{Build, Device, Dropped, Ending, KeptRun, Mark, SessionDocument, SettingsKept};
pub use frames::{FRAMES_HEADER, FrameRow, FramesError};
/// A run's file and its checksums, as a session takes them.
pub use sapphire_gameplay::replay::{Replay, Trail};
pub use session::{About, Replace, Session, session_name};
pub use writer::WriterStats;

/// Where the logs go.
pub(crate) const LOG_TARGET: &str = "playtest";

/// The directory under the player's files that holds the sessions.
pub const SESSIONS_DIR: &str = "sessions";

/// A session's document, in its directory.
pub const DOCUMENT_FILE: &str = "session.ron";

/// A session's frames, in its directory.
pub const FRAMES_FILE: &str = "frames.csv";

/// The directory, in a session's, that holds a directory per run.
pub const RUNS_DIR: &str = "runs";

/// How often a session is written while nothing happens: a killed process loses at most this much.
pub const KEEP_EVERY: Duration = Duration::from_secs(10);

/// The longest session kept: four hours of play, as long as a replay may be.
pub const MAX_SECONDS: u32 = 4 * 60 * 60;

/// The most runs a session keeps.
pub const MAX_RUNS: usize = 1024;

/// The most marks a session keeps.
pub const MAX_MARKS: usize = 256;

/// The most settings files a session keeps, the first among them.
pub const MAX_SETTINGS: usize = 64;

/// The most trips to the background a session counts.
pub const MAX_TRIPS: usize = 256;

/// The longest label a mark keeps, in characters.
pub const MAX_LABEL: usize = 64;

/// The most sessions kept on a device: the oldest go as a new one begins.
pub const MAX_SESSIONS: usize = 16;
