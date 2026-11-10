//! `session.ron`: what a session says of itself (the crate's docs have the format).

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

/// A session, as `session.ron` holds it. Every `at` is seconds on the session's clock: its frames' times summed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::playtest"]
#[serde(deny_unknown_fields)]
pub struct SessionDocument {
    pub build: Build,
    pub device: Device,
    /// When it began by the device's calendar, in seconds since 1970, UTC. `None` where the device has none.
    pub began: Option<u64>,
    /// The seconds played as of this write.
    pub seconds: f64,
    /// How it ended. `None` for a session still going when this was written, or one whose process was killed:
    /// it holds what the last keep held.
    pub ended: Option<Ending>,
    /// Each run with a tick in it, in the order they began.
    pub runs: Vec<KeptRun>,
    /// The pause's marks, in the order they were set.
    pub marks: Vec<Mark>,
    /// The settings files, the first the settings the session began with.
    pub settings: Vec<SettingsKept>,
    /// When the app left the foreground.
    pub trips: Vec<f64>,
    /// What the limits left out.
    pub dropped: Dropped,
}

impl Versioned for SessionDocument {
    /// 1: M4 F1.
    const VERSION: u32 = 1;
}

impl SessionDocument {
    /// Reads `session.ron`'s text.
    pub fn from_ron(text: &str) -> Result<Self, sapphire_serialize::DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    /// The run numbered `number`, if the session kept it.
    #[must_use]
    pub fn run(&self, number: u32) -> Option<&KeptRun> {
        self.runs.iter().find(|run| run.number == number)
    }
}

/// The build that kept a session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    /// The app's name.
    pub app: String,
    /// Its version, as its package says.
    pub version: String,
    /// The build, as crash reports and the pipeline cache name it.
    pub id: String,
}

/// The device a session was played on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// The OS, as the platform names it: `MacOs`, `Ios`, `Android`, `Windows`, `Linux`.
    pub os: String,
    /// `Desktop` or `Mobile`.
    pub class: String,
    /// The iOS Simulator or the Android emulator.
    pub emulator: bool,
    /// The CPU, as Rust names it: `aarch64`, `x86_64`. A run's checksums are this architecture's.
    pub architecture: String,
    pub ram_mib: u32,
    pub cores: u32,
}

/// How a session ended.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Ending {
    /// The app quit, or the OS said it would end it.
    Quit { at: f64 },
    /// It reached [`MAX_SECONDS`](crate::MAX_SECONDS), and kept nothing after.
    Full { at: f64 },
}

/// A run the session kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptRun {
    /// From 1, in the order runs began. Marks and frames name runs by it.
    pub number: u32,
    /// The scenario it plays.
    pub scenario: String,
    /// Its file, from the session's directory: `runs/<n>/<scenario>-<n>.sreplay`.
    pub file: String,
    /// When its first kept tick was seen.
    pub began: f64,
    /// The ticks its file holds.
    pub ticks: u32,
    /// Its checksum at the last of them on the device, as 16 hex digits: what a replay of the file reaches.
    pub checksum: String,
    /// Whether it had ended. A run still going was kept as far as it had got.
    pub ended: bool,
}

/// A moment the player marked from the pause.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mark {
    /// The run's number.
    pub run: u32,
    /// The run's tick: replay its file to here.
    pub tick: u64,
    pub at: f64,
    /// What the player called it, if they did.
    pub label: Option<String>,
}

/// A settings file the session kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsKept {
    /// From the session's directory: `settings-<n>.ron`.
    pub file: String,
    pub at: f64,
    /// The run that was standing, and its tick.
    pub run: u32,
    pub tick: u64,
}

/// What a session's limits left out, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dropped {
    pub runs: u32,
    pub marks: u32,
    pub settings: u32,
    pub trips: u32,
}
