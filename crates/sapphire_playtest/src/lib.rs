//! A playtest's session on disk (M4 F1): what a playtest build keeps of each time someone plays it, and the
//! format a host reads it back in.
//!
//! A playtest build is the shipping game built with its `playtest` feature, which is all that links this
//! crate (`xtask/layers.toml` has it `dev_only`, so no runtime crate can, and a store build can't hold it).
//! The game makes a [`Session`] as it starts and tells it what happens: each frame's time and the OS's heat
//! reading, each run as it ends and, every few seconds, as far as it's got, the player's settings as they
//! change, the pause's marks and trips to the background. The session keeps it all in memory, bounded, and
//! writes it every [`KEEP_EVERY`] and at once after anything a person did, on a job, through the caller's
//! [`Disk`] (`sapphire_platform`'s `durable_replace` and `durable_append` in the game). A keep writes only what
//! came since the last: a run's new ticks and `frames.csv`'s new rows are added to files that only grow, and
//! `session.ron` is replaced whole, last, once everything it names is on the disk. So a process killed at any
//! moment, mid-keep too, leaves a session whose files agree as of the last keep or the one before it, and
//! loses at most the seconds since (M4 F3).
//!
//! # The format
//!
//! Under the game's directory for the player's files (`user://`, or `--save-dir`), `sessions/` holds one
//! directory per session, named by the device's calendar as it began, in UTC (`2026-11-10-210433`), or
//! `session-<n>` where the device has no calendar. A session runs from the app's start to its quit, its kill
//! or [`MAX_SECONDS`] of play. The oldest go once more than [`MAX_SESSIONS`] are there.
//!
//! - `session.ron`: a [`SessionDocument`] (version 2), rewritten whole at each keep. The build and the device,
//!   when it began, the seconds played as of this keep, how it ended (nothing for one still going or killed),
//!   each run with its ticks, each mark, each settings file, each trip to the background, and how many rows
//!   `frames.csv` holds. Every `at` is seconds on the session's own clock: the frames' times summed, which
//!   stops while the app is away. What the document doesn't count (a later keep's rows and records, written
//!   before a kill stopped its document) a reader leaves out.
//! - `frames.csv`: a header ([`FRAMES_HEADER`]), then one [`FrameRow`] a second of play: when it ended, the run
//!   and its tick, how many frames, their mean and longest times, how many were late (longer than one and a
//!   half of the tier's interval), the interval, and the OS's heat reading at its end. At most [`MAX_SECONDS`]
//!   rows. Each keep adds its rows to the end.
//! - `settings-<n>.ron`: the player's settings as the game keeps them (a settings document), at the start (`0`)
//!   and after each change, which `session.ron`'s `settings` names with its time, run and tick.
//! - `runs/<n>/<scenario>-<n>.journal`: the run numbered `n`, from 1, a record a keep, each holding the ticks
//!   since the last and their checksums (the `journal` module's docs have the format). A host reads it back
//!   ([`Kept`]) as the run's file, as `--record` writes one, and its checksums as `tests/replays/` keeps them,
//!   and writes them beside it ([`Kept::settle`]): `<scenario>-<n>.sreplay` and `checksums.<architecture>.txt`.
//!   So `sapphire_replay check --dir runs/<n> --out <dir>` plays the run here and names the first tick where it
//!   differs from the device's (`cargo xtask playtest pull` brings sessions back, settles them and does it). A
//!   run that hasn't ended is kept as far as it had got at the last keep. A run with no ticks (the title's) is
//!   kept nowhere. A session of version 1 (M4 F1) kept the `.sreplay` and the checksums itself, whole at every
//!   keep, and reads as it is.
//!
//! A mark is a run's number and a tick of it: replay the run's file to that tick to see the moment.
//!
//! **Bounds.** [`MAX_SECONDS`] of play, after which the session stops keeping and says so; at most
//! [`MAX_RUNS`] runs, [`MAX_MARKS`] marks, [`MAX_SETTINGS`] settings files and [`MAX_TRIPS`] trips, past which
//! [`Dropped`] counts what wasn't kept; labels of at most [`MAX_LABEL`] characters. A run's file is a replay's,
//! which takes at most `MAX_TICKS`, and the clock steps at most 60 ticks a second, so a session's runs take at
//! most [`MAX_SECONDS`] of ticks between them.
//!
//! # What a session says (M4 F2)
//!
//! [`Report::read`] replays each run of a session here and reads it out per encounter: its time, the damage
//! the player took and their falls, the hits they landed and the damage dealt (M4 F3), dodges, perfect dodges
//! and counters, presses a second and idle time, frame times and heat, and each mark with what draws its
//! moment. `cargo xtask playtest report <session>` prints it
//! and writes it beside the session as [`REPORT_FILE`]; the `report` module's docs say how each is counted.
//!
//! **Not here:** putting a playtest build on a device or bringing its sessions back (`xtask playtest`).

mod document;
mod frames;
pub mod journal;
mod kept;
mod report;
mod session;
mod writer;

use std::time::Duration;

pub use document::{Build, Device, Dropped, Ending, KeptRun, Mark, SessionDocument, SettingsKept};
pub use frames::{FRAMES_HEADER, FrameRow, FramesError};
pub use journal::JournalError;
pub use kept::{Kept, ReadError, RunFiles};
pub use report::{
    Check, Counts, Frames, IDLE_AFTER, MarkReport, Met, MetEnd, Part, REPORT_FILE, Report, ReportError,
    RunReport,
};
/// A run's file and its checksums, as a session takes them.
pub use sapphire_gameplay::replay::{Replay, Trail};
pub use session::{About, Session, session_name};
pub use writer::{Disk, WriterStats};

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
