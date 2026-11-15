//! What the runner wrote of a unit when it ended (M5 Z1): `<unit>.status`, beside its log, one `key=value` a
//! line. A unit is complete when its status says it exited 0; the runner skips a complete unit, so a batch
//! cut short goes on where it stopped.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

/// A unit's end, as the runner wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// The sandbox's exit code, or `None` if it was stopped at its deadline or killed by a signal.
    pub exit: Option<i32>,
    /// Whether it ran out of time.
    pub timed_out: bool,
    /// Wall seconds it took.
    pub seconds: f64,
    /// The machine's one-minute load as it started and as it ended, where the OS says.
    pub load: Option<f64>,
    pub load_end: Option<f64>,
    /// Its command line, for the record.
    pub command: String,
}

impl Status {
    /// Whether the unit ran to its end and exited 0.
    pub fn complete(&self) -> bool {
        self.exit == Some(0) && !self.timed_out
    }

    /// What went wrong, for a unit that isn't complete.
    pub fn describe(&self) -> String {
        match (self.timed_out, self.exit) {
            (true, _) => format!("stopped at its deadline after {:.0} s", self.seconds),
            (false, Some(code)) => format!("exited {code}"),
            (false, None) => "killed by a signal".to_owned(),
        }
    }

    /// `<folder>/<unit>.status`, or `None` if there's none or it can't be read.
    pub fn read(folder: &Path, unit: &str) -> Option<Self> {
        Self::parse(&fs::read_to_string(path(folder, unit)).ok()?)
    }

    pub fn write(&self, folder: &Path, unit: &str) -> io::Result<()> {
        fs::write(path(folder, unit), self.format())
    }

    fn format(&self) -> String {
        let mut text = String::new();
        let number =
            |value: Option<f64>| value.map_or_else(|| "none".to_owned(), |value| format!("{value:.2}"));
        let _ =
            writeln!(text, "exit={}", self.exit.map_or_else(|| "none".to_owned(), |code| code.to_string()));
        let _ = writeln!(text, "timed_out={}", self.timed_out);
        let _ = writeln!(text, "seconds={:.1}", self.seconds);
        let _ = writeln!(text, "load={}", number(self.load));
        let _ = writeln!(text, "load_end={}", number(self.load_end));
        let _ = writeln!(text, "command={}", self.command);
        text
    }

    fn parse(text: &str) -> Option<Self> {
        let value = |key: &str| {
            text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('=')).map(str::to_owned)
        };
        let number = |key: &str| value(key).and_then(|text| text.parse::<f64>().ok());
        Some(Self {
            exit: value("exit")?.parse().ok(),
            timed_out: value("timed_out")? == "true",
            seconds: number("seconds")?,
            load: number("load"),
            load_end: number("load_end"),
            command: value("command").unwrap_or_default(),
        })
    }
}

fn path(folder: &Path, unit: &str) -> std::path::PathBuf {
    folder.join(format!("{unit}.status"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_reads_back_what_was_written_and_only_exit_0_is_complete() {
        let status = Status {
            exit: Some(0),
            timed_out: false,
            seconds: 12.5,
            load: Some(3.25),
            load_end: None,
            command: "sandbox --bench 200".to_owned(),
        };
        assert_eq!(Status::parse(&status.format()), Some(status.clone()));
        assert!(status.complete());
        let failed = Status { exit: Some(1), ..status.clone() };
        assert!(!failed.complete());
        assert_eq!(failed.describe(), "exited 1");
        let late = Status { exit: None, timed_out: true, ..status };
        assert!(!late.complete() && late.describe().starts_with("stopped at its deadline"));
        assert_eq!(Status::parse("exit=0\n"), None, "a status cut short isn't one");
    }
}
