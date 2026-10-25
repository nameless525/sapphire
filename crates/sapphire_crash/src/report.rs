//! Report files in the dump directory: naming, atomic writes, panic reports and listing.
//!
//! Every file is named `<run>.<suffix>`, where `<run>` is the run ID of the process that crashed
//! (`<unix ms, 13 digits>-<pid>`, so names sort by start time) and the suffix says what it is:
//!
//! | File | Written by |
//! |---|---|
//! | `<run>.dmp` | the desktop crash monitor |
//! | `<run>.panic-<n>.txt` | the panic hook, `n` counting panics within the run |
//! | `<run>.native.txt` | Android's in-process signal handler |
//!
//! Files are written under a `.partial` name and renamed into place once complete, so readers
//! never see half a report.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::CrashError;

const PARTIAL_SUFFIX: &str = ".partial";
const MINIDUMP_SUFFIX: &str = "dmp";
const NATIVE_SUFFIX: &str = "native.txt";
const PANIC_PREFIX: &str = "panic-";
const TEXT_SUFFIX: &str = ".txt";

/// What kind of crash artifact a [`PendingReport`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReportKind {
    /// A minidump written by the desktop crash monitor (`<run>.dmp`).
    Minidump,
    /// A panic report written by the panic hook (`<run>.panic-<n>.txt`, plain text).
    Panic,
    /// An Android signal record (`<run>.native.txt`, plain text).
    NativeSignal,
}

/// A crash artifact in the dump directory, waiting for an uploader (or the next launch) to
/// consume it. Consuming means deleting the file once it has been dealt with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingReport {
    kind: ReportKind,
    run_id: String,
    path: PathBuf,
}

impl PendingReport {
    /// What the file contains.
    pub fn kind(&self) -> ReportKind {
        self.kind
    }

    /// The run that wrote the report. Reports from one crash share it, for example a panic report
    /// and the minidump of the abort that followed under `panic = "abort"`.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Where the report is.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Lists the complete crash reports in `dir`, oldest run first.
///
/// This is what previous runs left behind: call it at startup to find reports to upload or show.
/// Files still being written and files this crate did not write are skipped. A missing directory
/// simply has no reports.
///
/// # Errors
///
/// [`CrashError::ReadDumpDir`] if the directory exists but cannot be read.
pub fn pending_reports(dir: impl AsRef<Path>) -> Result<Vec<PendingReport>, CrashError> {
    let dir = dir.as_ref();
    let read_error = |source| CrashError::ReadDumpDir { path: dir.to_owned(), source };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(read_error(err)),
    };

    let mut reports = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_error)?;
        if !entry.file_type().map_err(read_error)?.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some((kind, run_id)) = file_name.to_str().and_then(classify) else {
            continue;
        };
        reports.push(PendingReport { kind, run_id: run_id.to_owned(), path: entry.path() });
    }
    reports.sort_by(|a, b| (&a.run_id, a.kind, &a.path).cmp(&(&b.run_id, b.kind, &b.path)));
    Ok(reports)
}

/// Splits a report file name into its kind and run ID, or `None` if it is not a finished report.
fn classify(file_name: &str) -> Option<(ReportKind, &str)> {
    let (run_id, suffix) = file_name.split_once('.')?;
    if !is_valid_run_id(run_id) {
        return None;
    }
    let kind = match suffix {
        MINIDUMP_SUFFIX => ReportKind::Minidump,
        NATIVE_SUFFIX => ReportKind::NativeSignal,
        _ => {
            let seq = suffix.strip_prefix(PANIC_PREFIX)?.strip_suffix(TEXT_SUFFIX)?;
            if seq.is_empty() || !seq.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            ReportKind::Panic
        }
    };
    Some((kind, run_id))
}

/// A run ID: start time in Unix milliseconds (zero-padded so names sort) and the process ID.
pub(crate) fn new_run_id(unix_ms: u64, pid: u32) -> String {
    format!("{unix_ms:013}-{pid}")
}

pub(crate) fn is_valid_run_id(run_id: &str) -> bool {
    !run_id.is_empty() && run_id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
pub(crate) fn minidump_file_name(run_id: &str) -> String {
    format!("{run_id}.{MINIDUMP_SUFFIX}")
}

pub(crate) fn panic_file_name(run_id: &str, seq: u32) -> String {
    format!("{run_id}.{PANIC_PREFIX}{seq}{TEXT_SUFFIX}")
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn native_file_name(run_id: &str) -> String {
    format!("{run_id}.{NATIVE_SUFFIX}")
}

/// The temporary name a report is written under before it is renamed to `final_name`.
pub(crate) fn partial_name(final_name: &str) -> String {
    format!("{final_name}{PARTIAL_SUFFIX}")
}

/// Writes `contents` to `dir/name` through a `.partial` file, synced to disk before the rename.
pub(crate) fn write_atomically(dir: &Path, name: &str, contents: &[u8]) -> io::Result<PathBuf> {
    let final_path = dir.join(name);
    let partial_path = dir.join(partial_name(name));
    let result = File::create(&partial_path)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&partial_path, &final_path));
    match result {
        Ok(()) => Ok(final_path),
        Err(err) => {
            // Best effort: the partial file is never listed, it only takes up space.
            let _ = fs::remove_file(&partial_path);
            Err(err)
        }
    }
}

/// Wall-clock milliseconds since the Unix epoch, or 0 if the clock is before it.
// Reading the OS clock outside sapphire_time is allowed here: crash reports carry wall-clock time
// so they can be matched with logs and server-side events. It is never simulation time (rule R2).
#[allow(clippy::disallowed_methods)]
pub(crate) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// A loaded module (executable or shared library) at crash time: enough to map a raw frame
/// instruction pointer back to a source location offline.
///
/// `debug_id` is the platform's stable build identifier (the Mach-O UUID, the ELF GNU build-id,
/// or the PE PDB GUID+age). `base` is the module's load address, which is what `atos -l` and
/// Sentry's symbolicator need together with the frame IPs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModuleId {
    pub base: u64,
    pub debug_id: String,
    pub name: String,
}

/// The contents of a `<run>.panic-<n>.txt` file.
///
/// The format is plain text so a human can read it straight from disk. It records raw frame
/// instruction pointers and the loaded-module table (base + debug id) so a stripped shipping
/// build can still be symbolicated offline, where the symbolized `backtrace` text is only useful
/// in a build that kept its symbols.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PanicReport {
    pub app_name: String,
    pub release: String,
    pub run_id: String,
    pub pid: u32,
    pub thread: String,
    pub location: String,
    pub unix_time_ms: u64,
    pub uptime_ms: u64,
    /// Loaded modules at crash time, for offline symbolication of [`Self::frames`].
    pub modules: Vec<ModuleId>,
    /// Raw instruction pointers of the panicking thread, innermost first.
    pub frames: Vec<u64>,
    pub message: String,
    pub backtrace: String,
}

const PANIC_REPORT_TITLE: &str = "Sapphire panic report v2";
// Each section begins with a blank line so the markers never collide with free-form content, and
// they appear in this fixed order, so parsing splits on the first occurrence of each in turn.
const MODULES_MARKER: &str = "\n\nmodules:\n";
const FRAMES_MARKER: &str = "\n\nframes:\n";
const MESSAGE_MARKER: &str = "\n\nmessage:\n";
const BACKTRACE_MARKER: &str = "\n\nbacktrace:\n";

impl PanicReport {
    pub fn render(&self) -> String {
        let one_line = |value: &str| value.replace(['\r', '\n'], " ");
        let mut out = String::with_capacity(512 + self.message.len() + self.backtrace.len());
        out.push_str(PANIC_REPORT_TITLE);
        out.push('\n');
        // Header values are kept to one line each; the message and backtrace keep their newlines.
        for (key, value) in [
            ("app", self.app_name.as_str()),
            ("release", &self.release),
            ("run", &self.run_id),
            ("pid", &self.pid.to_string()),
            ("thread", &self.thread),
            ("location", &self.location),
            ("unix_time_ms", &self.unix_time_ms.to_string()),
            ("uptime_ms", &self.uptime_ms.to_string()),
        ] {
            // Writing to a String cannot fail.
            let _ = writeln!(out, "{key}: {}", one_line(value));
        }

        // Modules: `0x<base> <debug_id> <name>`, name last because it may contain spaces.
        out.push_str(MODULES_MARKER);
        for module in &self.modules {
            let _ = writeln!(
                out,
                "{:#018x} {} {}",
                module.base,
                one_line(&module.debug_id),
                one_line(&module.name),
            );
        }
        out.push_str(FRAMES_MARKER);
        for ip in &self.frames {
            let _ = writeln!(out, "{ip:#018x}");
        }

        out.push_str(MESSAGE_MARKER);
        out.push_str(&self.message);
        out.push_str(BACKTRACE_MARKER);
        out.push_str(&self.backtrace);
        out
    }

    /// Parses [`Self::render`]'s output. Returns `None` for anything else.
    #[cfg(test)]
    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.strip_prefix(PANIC_REPORT_TITLE)?.strip_prefix('\n')?;
        // The sections appear in this order, so a first-match split peels them off one at a time.
        let (header, rest) = rest.split_once(MODULES_MARKER)?;
        let (modules, rest) = rest.split_once(FRAMES_MARKER)?;
        let (frames, rest) = rest.split_once(MESSAGE_MARKER)?;
        // The backtrace never contains the marker; a message might, so split at the last one.
        let (message, backtrace) = rest.rsplit_once(BACKTRACE_MARKER)?;

        let field = |key: &str| {
            header.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": ")).map(str::to_owned)
        };
        Some(Self {
            app_name: field("app")?,
            release: field("release")?,
            run_id: field("run")?,
            pid: field("pid")?.parse().ok()?,
            thread: field("thread")?,
            location: field("location")?,
            unix_time_ms: field("unix_time_ms")?.parse().ok()?,
            uptime_ms: field("uptime_ms")?.parse().ok()?,
            modules: modules.lines().filter_map(parse_module_line).collect(),
            frames: frames.lines().filter_map(|line| parse_hex_u64(line.trim())).collect(),
            message: message.to_owned(),
            backtrace: backtrace.to_owned(),
        })
    }
}

#[cfg(test)]
fn parse_hex_u64(token: &str) -> Option<u64> {
    u64::from_str_radix(token.strip_prefix("0x").or_else(|| token.strip_prefix("0X"))?, 16).ok()
}

#[cfg(test)]
fn parse_module_line(line: &str) -> Option<ModuleId> {
    let mut parts = line.splitn(3, ' ');
    let base = parse_hex_u64(parts.next()?)?;
    let debug_id = parts.next()?.to_owned();
    let name = parts.next().unwrap_or("").to_owned();
    Some(ModuleId { base, debug_id, name })
}

/// Removes `*.partial` files left by earlier runs that crashed or were killed mid-write.
///
/// Only files whose embedded run ID is older than `current_run_id` are removed, so a monitor that
/// another live process started is never disturbed. Called from [`crate::install`], before this
/// run writes anything, so there are no partials of the current run to catch.
pub(crate) fn cleanup_stale_partials(dir: &Path, current_run_id: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(PARTIAL_SUFFIX)) else {
            continue;
        };
        // The stem is the final report name, e.g. `<run>.dmp`; its run ID is the first field.
        let Some((run_id, _)) = stem.split_once('.') else {
            continue;
        };
        if is_valid_run_id(run_id) && run_id < current_run_id {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;

    fn sample_report() -> PanicReport {
        PanicReport {
            app_name: "demo".into(),
            release: "1.2.3+abc".into(),
            run_id: new_run_id(1_761_300_000_123, 4242),
            pid: 4242,
            thread: "main (ThreadId(1))".into(),
            location: "src/main.rs:10:5".into(),
            unix_time_ms: 1_761_300_000_456,
            uptime_ms: 333,
            modules: vec![
                ModuleId {
                    base: 0x0001_0000_0000,
                    debug_id: "5351CD93-0F5D-347A-88AC-CBF7CBEF0492".into(),
                    name: "demo".into(),
                },
                ModuleId {
                    base: 0x7fff_2000_0000,
                    debug_id: "ABCD".into(),
                    name: "libsystem c.dylib".into(),
                },
            ],
            frames: vec![0x0001_0000_80ab, 0x7fff_2000_9444],
            // The message deliberately contains text that looks like later section markers.
            message: "first line\n\nbacktrace:\nnot really a backtrace".into(),
            backtrace: "   0: demo::main\n             at ./src/main.rs:10:5\n".into(),
        }
    }

    #[test]
    fn run_ids_sort_by_start_time() {
        assert_eq!(new_run_id(42, 7), "0000000000042-7");
        assert!(new_run_id(999, 1) < new_run_id(1000, 1));
        assert!(is_valid_run_id("0001761300000123-4242"));
        assert!(!is_valid_run_id(""));
        assert!(!is_valid_run_id("../evil"));
    }

    #[test]
    fn classifies_report_names() {
        let run = "0001761300000123-4242";
        assert_eq!(classify(&minidump_file_name(run)), Some((ReportKind::Minidump, run)));
        assert_eq!(classify(&panic_file_name(run, 0)), Some((ReportKind::Panic, run)));
        assert_eq!(classify(&panic_file_name(run, 17)), Some((ReportKind::Panic, run)));
        assert_eq!(classify(&native_file_name(run)), Some((ReportKind::NativeSignal, run)));

        assert_eq!(classify(&partial_name(&minidump_file_name(run))), None);
        assert_eq!(classify(&partial_name(&panic_file_name(run, 0))), None);
        assert_eq!(classify(&format!("{run}.panic-.txt")), None);
        assert_eq!(classify(&format!("{run}.panic-x.txt")), None);
        assert_eq!(classify(&format!("{run}.log")), None);
        assert_eq!(classify("notes.dmp"), None);
        assert_eq!(classify(".dmp"), None);
        assert_eq!(classify("README"), None);
    }

    #[test]
    fn panic_report_round_trips() {
        let report = sample_report();
        let text = report.render();
        assert!(text.starts_with("Sapphire panic report v2\napp: demo\n"));
        // The module base and frame IPs render as fixed-width hex.
        assert!(text.contains("0x0000000100000000 5351CD93-0F5D-347A-88AC-CBF7CBEF0492 demo\n"), "{text}");
        assert!(text.contains(&format!("\nframes:\n{:#018x}\n", report.frames[0])), "{text}");
        assert_eq!(PanicReport::parse(&text), Some(report));
    }

    #[test]
    fn panic_report_round_trips_without_frames_or_modules() {
        let mut report = sample_report();
        report.modules.clear();
        report.frames.clear();
        assert_eq!(PanicReport::parse(&report.render()), Some(report));
    }

    #[test]
    fn panic_report_header_stays_on_one_line_per_field() {
        let mut report = sample_report();
        report.thread = "evil\nlocation: forged".into();
        let parsed = PanicReport::parse(&report.render()).expect("parses");
        assert_eq!(parsed.thread, "evil location: forged");
        assert_eq!(parsed.location, report.location);
    }

    #[test]
    fn parse_rejects_other_text() {
        assert_eq!(PanicReport::parse(""), None);
        assert_eq!(PanicReport::parse("Sapphire panic report v2\napp: x\n"), None);
        assert_eq!(
            PanicReport::parse("hello\n\nmodules:\n\n\nframes:\n\n\nmessage:\nhi\n\nbacktrace:\n"),
            None
        );
    }

    #[test]
    fn cleanup_removes_only_older_partials() {
        let dir = ScratchDir::new("cleanup");
        let old_run = new_run_id(1_000, 10);
        let current_run = new_run_id(2_000, 20);
        let future_run = new_run_id(3_000, 30);
        let old_partial = partial_name(&minidump_file_name(&old_run));
        let current_partial = partial_name(&minidump_file_name(&current_run));
        let future_partial = partial_name(&minidump_file_name(&future_run));
        for name in [&old_partial, &current_partial, &future_partial, &"junk.partial".to_owned()] {
            fs::write(dir.path().join(name), b"x").expect("write");
        }
        let finished = minidump_file_name(&old_run);
        fs::write(dir.path().join(&finished), b"MDMP").expect("write");

        cleanup_stale_partials(dir.path(), &current_run);

        let mut names: Vec<_> = fs::read_dir(dir.path())
            .expect("list")
            .map(|entry| entry.expect("entry").file_name().into_string().expect("utf-8"))
            .collect();
        names.sort();
        // The older run's partial is gone; the current run's, a newer run's, junk and the
        // finished report are all kept.
        assert_eq!(names, {
            let mut kept = vec![current_partial, future_partial, "junk.partial".to_owned(), finished];
            kept.sort();
            kept
        });
    }

    #[test]
    fn atomic_write_leaves_only_the_final_file() {
        let dir = ScratchDir::new("atomic");
        let path = write_atomically(dir.path(), "x.txt", b"hello").expect("write");
        assert_eq!(fs::read(&path).expect("read back"), b"hello");
        let names: Vec<_> = fs::read_dir(dir.path())
            .expect("list")
            .map(|entry| entry.expect("entry").file_name().into_string().expect("utf-8"))
            .collect();
        assert_eq!(names, ["x.txt"]);
    }

    #[test]
    fn lists_pending_reports_in_run_order() {
        let dir = ScratchDir::new("pending");
        let old_run = new_run_id(1_000, 10);
        let new_run = new_run_id(2_000, 20);
        for name in [
            panic_file_name(&new_run, 0),
            minidump_file_name(&new_run),
            panic_file_name(&old_run, 1),
            native_file_name(&old_run),
            partial_name(&minidump_file_name(&old_run)),
            "unrelated.txt".to_owned(),
        ] {
            fs::write(dir.path().join(name), b"x").expect("write");
        }
        fs::create_dir(dir.path().join(minidump_file_name("123-4"))).expect("mkdir");

        let reports = pending_reports(dir.path()).expect("list");
        let summary: Vec<_> = reports.iter().map(|r| (r.run_id(), r.kind())).collect();
        assert_eq!(
            summary,
            [
                (old_run.as_str(), ReportKind::Panic),
                (old_run.as_str(), ReportKind::NativeSignal),
                (new_run.as_str(), ReportKind::Minidump),
                (new_run.as_str(), ReportKind::Panic),
            ]
        );
        assert!(reports.iter().all(|r| r.path().starts_with(dir.path())));
    }

    #[test]
    fn missing_dump_dir_has_no_reports() {
        let dir = ScratchDir::new("missing");
        let missing = dir.path().join("does-not-exist");
        assert_eq!(pending_reports(missing).expect("missing is fine"), []);
    }
}
