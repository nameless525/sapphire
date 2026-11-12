//! [`Session`]: what a playtest build keeps as someone plays (the crate's docs have the format).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bevy_reflect::TypePath;
use sapphire_core::thermal::ThermalReading;
use sapphire_gameplay::replay::{Replay, Trail, is_valid_name};
use sapphire_reflect::Versioned;
use serde::Serialize;

use crate::document::{Build, Device, Dropped, Ending, KeptRun, Mark, SessionDocument, SettingsKept};
use crate::frames::{FRAMES_HEADER, OpenSecond};
use crate::writer::{Content, Disk, RunKept, Writer, WriterStats};
use crate::{
    DOCUMENT_FILE, FRAMES_FILE, KEEP_EVERY, LOG_TARGET, MAX_LABEL, MAX_MARKS, MAX_RUNS, MAX_SECONDS,
    MAX_SESSIONS, MAX_SETTINGS, MAX_TRIPS, RUNS_DIR,
};

/// What a session says of where it was kept, as it begins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct About {
    pub build: Build,
    pub device: Device,
    /// The device's calendar, in seconds since 1970, UTC, if it has one: what the session's named by.
    pub began: Option<u64>,
}

/// One session, from the app's start to its quit: see the crate's docs.
///
/// **Calls:** [`Self::frame`] once a frame; [`Self::run_ended`] for each run as it ends; [`Self::mark`],
/// [`Self::settings`] and [`Self::left_foreground`] as they happen; and whenever [`Self::wants_keep`],
/// [`Self::keep`] with the run standing as far as it's got. [`Self::flush`] keeps and waits, for an app that
/// may not come back; [`Self::end`] for a quit.
pub struct Session {
    dir: PathBuf,
    document: SessionDocument,
    /// `frames.csv` so far: its header and each keep's rows, which only grows, and the rows since the last keep.
    pieces: Vec<Arc<str>>,
    open_rows: String,
    rows: u32,
    second: OpenSecond,
    /// The session's clock: its frames' times summed.
    clock: Duration,
    /// The standing run's number, its tick as of the last frame and when it first had one.
    run: u32,
    tick: u64,
    run_began: Option<f64>,
    since_keep: Duration,
    /// Something happened that the next keep shouldn't wait for.
    asked: bool,
    writer: Writer,
}

impl Session {
    /// Begins a session in a directory of its own under `sessions`, which is made if it isn't there, removing
    /// the oldest past [`MAX_SESSIONS`]. Nothing's written until the first [`Self::keep`].
    pub fn begin(sessions: &Path, about: About, disk: Arc<dyn Disk>) -> io::Result<Self> {
        fs::create_dir_all(sessions)?;
        prune(sessions, MAX_SESSIONS - 1)?;
        let dir = free_dir(sessions, about.began);
        fs::create_dir(&dir)?;
        tracing::info!(target: LOG_TARGET, dir = %dir.display(), "a playtest session begins");
        let About { build, device, began } = about;
        Ok(Self {
            dir,
            document: SessionDocument {
                build,
                device,
                began,
                seconds: 0.0,
                ended: None,
                runs: Vec::new(),
                marks: Vec::new(),
                settings: Vec::new(),
                trips: Vec::new(),
                dropped: Dropped::default(),
                frames: Some(0),
            },
            pieces: Vec::new(),
            open_rows: format!("{FRAMES_HEADER}\n"),
            rows: 0,
            second: OpenSecond::default(),
            clock: Duration::ZERO,
            run: 1,
            tick: 0,
            run_began: None,
            since_keep: Duration::ZERO,
            asked: true,
            writer: Writer::new(disk),
        })
    }

    /// Its directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What `session.ron` holds as of the last keep, with whatever's happened since.
    #[must_use]
    pub fn document(&self) -> &SessionDocument {
        &self.document
    }

    /// The standing run's number.
    #[must_use]
    pub fn run(&self) -> u32 {
        self.run
    }

    #[must_use]
    pub fn stats(&self) -> WriterStats {
        self.writer.stats()
    }

    /// A frame that took `frame`, against the tier's interval `budget`, with the OS's `heat` at its end and the
    /// standing run at `tick`. Past [`MAX_SECONDS`] the session ends, and keeps once more.
    pub fn frame(&mut self, frame: Duration, budget: Duration, heat: ThermalReading, tick: u64) {
        self.frame_standing_for(frame, frame, budget, heat, tick);
    }

    /// [`Self::frame`], for a frame that stands for `stands` of play however long it took: a capture's, which
    /// draws a tick a frame, so the session's clock keeps to the run's ticks rather than to how fast this
    /// machine draws (M4 F3). `frame` is what `frames.csv` holds the frame to.
    pub fn frame_standing_for(
        &mut self,
        stands: Duration,
        frame: Duration,
        budget: Duration,
        heat: ThermalReading,
        tick: u64,
    ) {
        if self.document.ended.is_some() {
            return;
        }
        self.clock += stands;
        self.since_keep += stands;
        self.tick = tick;
        if tick > 0 && self.run_began.is_none() {
            self.run_began = Some(self.at());
        }
        if let Some(row) = self.second.add((stands, frame), budget, heat, self.at(), (self.run, tick))
            && self.rows < MAX_SECONDS
        {
            row.write(&mut self.open_rows);
            self.rows += 1;
        }
        if self.clock >= Duration::from_secs(MAX_SECONDS.into()) {
            tracing::warn!(target: LOG_TARGET, seconds = MAX_SECONDS, "the session is as long as one may be: it keeps nothing more");
            self.document.ended = Some(Ending::Full { at: self.at() });
            self.asked = true;
        }
    }

    /// The standing run ended with `replay` and `trail`: it's kept as it ended, at once, and the next run has the
    /// next number. A run with no ticks, the title's, is kept nowhere and leaves the number to the next.
    pub fn run_ended(&mut self, replay: Replay, trail: Trail) {
        if replay.ticks() == 0 {
            return;
        }
        // A keep, with the document that names the run's last ticks: a run's record waiting to be written may be
        // overtaken only by one whose document overtakes the one waiting with it, or the document on the disk
        // could name ticks no record ends at.
        self.keep_run(replay, &trail, true);
        self.run += 1;
        self.tick = 0;
        self.run_began = None;
        self.keep(None);
    }

    /// The player marked this moment from the pause: the standing run's tick, with `label` if they gave one,
    /// trimmed and cut to [`MAX_LABEL`] characters.
    pub fn mark(&mut self, label: Option<&str>) {
        if self.document.ended.is_some() {
            return;
        }
        if self.document.marks.len() >= MAX_MARKS {
            self.document.dropped.marks += 1;
            return;
        }
        let label = label.map(clean_label).filter(|label| !label.is_empty());
        tracing::info!(target: LOG_TARGET, run = self.run, tick = self.tick, ?label, "marked");
        self.document.marks.push(Mark { run: self.run, tick: self.tick, at: self.at(), label });
        self.asked = true;
    }

    /// The player's settings, at the start and after each change, as the game keeps them.
    pub fn settings<T: Versioned + Serialize + TypePath>(&mut self, settings: &T) {
        if self.document.ended.is_some() {
            return;
        }
        if self.document.settings.len() >= MAX_SETTINGS {
            self.document.dropped.settings += 1;
            return;
        }
        let text = match sapphire_serialize::to_ron(settings) {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "the settings can't be kept");
                return;
            }
        };
        let file = format!("settings-{}.ron", self.document.settings.len());
        self.writer.write(self.dir.join(&file), Content::Text(text));
        self.document.settings.push(SettingsKept { file, at: self.at(), run: self.run, tick: self.tick });
        self.asked = true;
    }

    /// The app is leaving the foreground, and may never come back: [`Self::flush`] next.
    pub fn left_foreground(&mut self) {
        if self.document.ended.is_some() {
            return;
        }
        if self.document.trips.len() >= MAX_TRIPS {
            self.document.dropped.trips += 1;
        } else {
            self.document.trips.push(self.at());
        }
        self.asked = true;
    }

    /// Whether to [`Self::keep`] now: something happened, or [`KEEP_EVERY`] has passed since the last.
    #[must_use]
    pub fn wants_keep(&self) -> bool {
        self.asked || (self.document.ended.is_none() && self.since_keep >= KEEP_EVERY)
    }

    /// Writes what's changed since the last keep on a job, the run standing as far as it's got (`standing`, its
    /// file and its checksums) among it: only its new ticks, and only the new rows of frames. Also hears how
    /// the last keep went.
    pub fn keep(&mut self, standing: Option<(Replay, Trail)>) {
        self.writer.poll();
        if let Some((replay, trail)) = standing {
            self.keep_run(replay, &trail, false);
        }
        self.document.seconds = self.at();
        if !self.open_rows.is_empty() {
            self.pieces.push(Arc::from(std::mem::take(&mut self.open_rows)));
        }
        self.document.frames = Some(self.rows);
        // The writer puts the document after the frames, whose rows it counts, and the runs, whose ticks it does.
        self.writer.write(self.dir.join(FRAMES_FILE), Content::Pieces(self.pieces.clone()));
        self.writer.write(self.dir.join(DOCUMENT_FILE), Content::Document(Box::new(self.document.clone())));
        self.asked = false;
        self.since_keep = Duration::ZERO;
        self.writer.poll();
    }

    /// [`Self::keep`], and waits for every write: for an app the OS may end where it's going.
    pub fn flush(&mut self, standing: Option<(Replay, Trail)>) {
        self.keep(standing);
        self.writer.finish();
    }

    /// Ends the session as the app quits, keeping the run standing if one is and waiting for every write.
    pub fn end(mut self, standing: Option<(Replay, Trail)>) -> WriterStats {
        if self.document.ended.is_none() {
            self.document.ended = Some(Ending::Quit { at: self.at() });
        }
        self.flush(standing);
        let stats = self.writer.stats();
        tracing::info!(
            target: LOG_TARGET,
            dir = %self.dir.display(),
            runs = self.document.runs.len(),
            marks = self.document.marks.len(),
            seconds = self.document.seconds,
            ?stats,
            "the playtest session ends"
        );
        stats
    }

    /// The session's clock, in seconds to the millisecond.
    fn at(&self) -> f64 {
        self.clock.as_millis() as f64 / 1e3
    }

    /// Keeps the standing run's file and checksums in its journal as far as `replay` and `trail` go, if they've
    /// moved since the last keep, and says so in the document.
    fn keep_run(&mut self, replay: Replay, trail: &Trail, ended: bool) {
        let ticks = replay.ticks();
        if ticks == 0 {
            return;
        }
        let number = self.run;
        let index = match self.document.runs.iter().position(|run| run.number == number) {
            Some(index) => index,
            None if self.document.runs.len() >= MAX_RUNS => {
                if ended {
                    self.document.dropped.runs += 1;
                }
                return;
            }
            None => {
                let scenario = replay.scenario().to_owned();
                let named = format!("{scenario}-{number}");
                let name = if is_valid_name(&named) { named } else { format!("run-{number}") };
                self.document.runs.push(KeptRun {
                    number,
                    file: format!("{RUNS_DIR}/{number}/{name}.sreplay"),
                    journal: Some(format!("{RUNS_DIR}/{number}/{name}.journal")),
                    scenario,
                    began: self.run_began.unwrap_or_else(|| self.at()),
                    ticks: 0,
                    checksum: String::new(),
                    ended: false,
                });
                self.document.runs.len() - 1
            }
        };
        let kept = &mut self.document.runs[index];
        if kept.ticks == ticks && kept.ended == ended {
            return;
        }
        kept.ticks = ticks;
        kept.ended = ended;
        kept.checksum = format!("{:016x}", trail.end());
        let journal = self.dir.join(kept.journal.as_deref().expect("this session's runs each have one"));
        self.writer.write(journal, Content::Run(Box::new(RunKept { replay, trail: trail.clone(), ended })));
    }
}

/// A label as a mark keeps it: on one line, trimmed, and at most [`MAX_LABEL`] characters.
fn clean_label(label: &str) -> String {
    let single: String =
        label.chars().map(|character| if character.is_control() { ' ' } else { character }).collect();
    single.trim().chars().take(MAX_LABEL).collect::<String>().trim_end().to_owned()
}

/// A session's name by the calendar's `seconds` since 1970, in UTC: `2026-11-10-210433`, which sorts in time.
#[must_use]
pub fn session_name(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let in_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}-{:02}{:02}{:02}", in_day / 3600, in_day / 60 % 60, in_day % 60)
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A directory for a new session under `sessions` that nothing has yet: named by the calendar where there is
/// one, numbered after that if a session began the same second, else `session-<n>`.
fn free_dir(sessions: &Path, began: Option<u64>) -> PathBuf {
    let base = match began {
        Some(seconds) => session_name(seconds),
        None => "session".to_owned(),
    };
    let first = began.map(|_| sessions.join(&base)).filter(|dir| !dir.exists());
    first.unwrap_or_else(|| {
        (1_u32..)
            .map(|number| sessions.join(format!("{base}-{number}")))
            .find(|dir| !dir.exists())
            .expect("fewer than 2³² sessions")
    })
}

/// Removes the oldest sessions under `sessions` until at most `keep` are left. Names sort in time.
fn prune(sessions: &Path, keep: usize) -> io::Result<()> {
    let mut kept = Vec::new();
    for entry in fs::read_dir(sessions)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            kept.push(entry.path());
        }
    }
    kept.sort_unstable();
    let surplus = kept.len().saturating_sub(keep);
    for old in &kept[..surplus] {
        tracing::info!(target: LOG_TARGET, dir = %old.display(), "the oldest session goes, to keep {MAX_SESSIONS}");
        fs::remove_dir_all(old)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_named_by_the_utc_calendar() {
        assert_eq!(session_name(0), "1970-01-01-000000");
        assert_eq!(session_name(1_794_344_673), "2026-11-10-210433");
        assert_eq!(session_name(951_868_799), "2000-02-29-235959");
    }

    #[test]
    fn a_label_is_one_short_line() {
        assert_eq!(clean_label("  felt\nlate  "), "felt late");
        assert_eq!(clean_label(&"a".repeat(100)).chars().count(), MAX_LABEL);
        assert_eq!(clean_label(&"é".repeat(100)).chars().count(), MAX_LABEL, "characters, not bytes");
    }
}
