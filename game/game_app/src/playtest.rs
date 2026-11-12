//! A playtest build's session (M4 F1): only the `playtest` feature builds this, so a store build keeps nothing
//! and its pause has no mark.
//!
//! The session (`sapphire_playtest`) is kept beside the save, under `sessions/`: in the player's directory, or
//! `--save-dir`'s, and a capture keeps one only where `--save-dir` says, as it does a save. The show is asked
//! for the pause's mark and each run as it ends ([`asked`]); the window tells the session each frame's time
//! and heat, the player's settings, and when the OS moves the app; a quit ends it.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use game_combat::combat::{CombatOption, RunSoFar};
use game_flow::boot::BootUpdate;
use game_flow::capture::{Captured, GameCapture};
use game_flow::show::GameShow;
use game_ui::menus::PlayerSettings;
use sapphire_core::thermal::ThermalReading;
use sapphire_platform::{App, AppInitError, AppMetadata, Host, Platform, durable_append, durable_replace};
use sapphire_playtest::{About, Build, Device, Disk, Replay, SESSIONS_DIR, Session, Trail};
use sapphire_render::quality::Quality;
use sapphire_stage::Stage;
use sapphire_time::TICK_HZ;

use crate::LOG_TARGET;
use crate::app::GameApp;

/// The frame's interval where the tier sets none.
const UNPACED: Duration = Duration::from_micros(16_667);

/// What a capture's frame stands for: a tick, as `game_flow::capture` steps one a frame.
const CAPTURED_FRAME: Duration = Duration::from_nanos(1_000_000_000 / TICK_HZ as u64);

/// See this module.
pub(crate) struct Playtest {
    session: Session,
}

/// The disk as a session writes it: whole files replaced, and a run's journal and its frames grown, each so a
/// kill at any moment leaves what the session's document names.
struct Durable;

impl Disk for Durable {
    fn replace(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        durable_replace(path, bytes)
    }

    fn append(&self, path: &Path, at: u64, bytes: &[u8]) -> io::Result<()> {
        durable_append(path, at, bytes)
    }
}

/// `show`, asked for what a session keeps: the pause's mark, and each run as it ends. Before it stands.
pub(crate) fn asked(mut show: GameShow) -> GameShow {
    show.offer_marks();
    show.keep_ended_runs();
    show
}

/// A capture (`game_flow::capture`) with a session kept where it keeps a save: only under `save_dir`, which
/// `--save-dir` gives it. Each frame stands for a tick of play, as the capture steps it, so the session's clock
/// keeps to the run's ticks; `frames.csv` holds how long each took to draw. It has no heat to read.
pub(crate) fn capture(
    platform: &Platform,
    host: &Host,
    (show, first): (GameShow, &CombatOption),
    capture: &GameCapture,
    update: Option<BootUpdate>,
    save_dir: Option<&Path>,
) -> Result<Captured, AppInitError> {
    let mut playtest = save_dir
        .and_then(|dir| Playtest::begin(platform, &GameApp::METADATA, dir, &PlayerSettings::default()));
    let mut last = platform.now();
    let captured = game_flow::capture::capture_with(host, asked(show), first, capture, update, |stage| {
        let now = platform.now();
        let frame = now.saturating_duration_since(last);
        last = now;
        if let Some(playtest) = &mut playtest {
            playtest.frame_standing_for(stage, CAPTURED_FRAME, frame, ThermalReading::default());
        }
    })?;
    if let Some(playtest) = playtest {
        playtest.end(None);
    }
    Ok(captured)
}

/// The interval the tier paces frames to: what a frame's time is held against.
pub(crate) fn budget(quality: &Quality) -> Duration {
    quality.frame_rate().map_or(UNPACED, |rate| Duration::from_secs(1) / rate.get())
}

impl Playtest {
    /// Begins a session under `dir`'s `sessions/`, beginning with `settings`. None, logged, where it can't: the
    /// game goes on without one.
    pub(crate) fn begin(
        platform: &Platform,
        metadata: &AppMetadata,
        dir: &Path,
        settings: &PlayerSettings,
    ) -> Option<Self> {
        let info = platform.system_info();
        let about = About {
            build: Build {
                app: metadata.name.to_owned(),
                version: metadata.version.to_owned(),
                id: crate::build_info::BUILD_ID.to_owned(),
            },
            device: Device {
                os: format!("{:?}", info.os),
                class: format!("{:?}", info.device_class),
                emulator: info.is_emulator,
                architecture: std::env::consts::ARCH.to_owned(),
                ram_mib: info.ram_mib,
                cores: info.logical_cores,
            },
            // 0 is a clock set before 1970: no calendar to name the session by.
            began: Some(sapphire_time::calendar_seconds()).filter(|&seconds| seconds > 0),
        };
        match Session::begin(&dir.join(SESSIONS_DIR), about, Arc::new(Durable)) {
            Ok(mut session) => {
                session.settings(settings);
                Some(Self { session })
            }
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, "no playtest session: this play is kept nowhere");
                None
            }
        }
    }

    /// After a frame of `stage`'s that took `frame`, with the OS's `heat`: the runs that ended, a mark the pause
    /// set, the frame, and a keep if one's due.
    pub(crate) fn frame(&mut self, stage: &mut Stage<GameShow>, frame: Duration, heat: ThermalReading) {
        self.frame_standing_for(stage, frame, frame, heat);
    }

    /// [`Self::frame`], for a frame that stands for `stands` of play however long it took: a capture's.
    fn frame_standing_for(
        &mut self,
        stage: &mut Stage<GameShow>,
        stands: Duration,
        frame: Duration,
        heat: ThermalReading,
    ) {
        self.hear(&mut stage.show);
        let tick = stage.show.shown.fight.combat.as_ref().map_or(0, |combat| combat.simulation().tick());
        self.session.frame_standing_for(stands, frame, budget(&stage.quality), heat, tick);
        if self.session.wants_keep() {
            self.session.keep(standing(&stage.show));
        }
    }

    /// What the player's chosen, as the game keeps it.
    pub(crate) fn settings(&mut self, settings: &PlayerSettings) {
        self.session.settings(settings);
    }

    /// The app is leaving the foreground, and may never come back: what's kept is written, and waited for.
    pub(crate) fn left_foreground(&mut self, show: Option<&mut GameShow>) {
        self.session.left_foreground();
        self.flush(show);
    }

    /// Writes what's kept, and waits: the OS may end the app without a quit.
    pub(crate) fn flush(&mut self, show: Option<&mut GameShow>) {
        let standing = show.and_then(|show| {
            self.hear(show);
            standing(show)
        });
        self.session.flush(standing);
    }

    /// The runs a show that's ending ended, before it goes: a lost device's.
    pub(crate) fn take_runs(&mut self, show: &mut GameShow) {
        self.hear(show);
    }

    /// Ends the session as the app quits, after the show's finished.
    pub(crate) fn end(mut self, show: Option<&mut GameShow>) {
        let standing = show.and_then(|show| {
            self.hear(show);
            standing(show)
        });
        self.session.end(standing);
    }

    /// The runs `show` ended, and a mark its pause set, as of the last frame's tick.
    fn hear(&mut self, show: &mut GameShow) {
        for run in show.take_ended_runs() {
            self.session.run_ended(run.replay, run.trail);
        }
        if show.take_mark() {
            self.session.mark(None);
        }
    }
}

/// The run standing on `show`, as far as it's got, as a session takes it.
fn standing(show: &GameShow) -> Option<(Replay, Trail)> {
    let RunSoFar { replay, trail } = show.shown.fight.combat.as_ref()?.so_far()?;
    Some((replay, trail))
}
