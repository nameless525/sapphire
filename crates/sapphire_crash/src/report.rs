//! Report files in the dump directory: naming, atomic writes, panic reports and listing.
//!
//! Every file is named `<run>.<suffix>`, where `<run>` is the run ID of the process that crashed
//! (`<unix ms, 13 digits>-<pid>`, so names sort by start time) and the suffix says what it is:
//!
//! | File | Written by |
//! |---|---|
//! | `<run>.dmp`, then `<run>.<n>.dmp` | the desktop crash monitor, `n` counting its dumps within the run |
//! | `<run>.panic-<n>.txt` | the panic hook, `n` counting panics within the run |
//! | `<run>.native.txt` | Android's in-process signal handler |
//!
//! A run can leave several minidumps: the panic hook asks the monitor for one per panic (up to a
//! cap), and the app may recover from a panic and crash natively later.
//!
//! Files are written under a `.partial` name and renamed into place once complete, so readers
//! never see half a report. The writer holds an exclusive lock on its `.partial` file until the
//! rename is done, which is how a later launch tells an abandoned partial from one that a live
//! process (another sandbox, or a crashed app's monitor) is still writing.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::CrashError;

/// How long a `.partial` file must have gone unmodified, as of the current run's start, before
/// the startup cleanup considers it. Its lock is what proves the writer gone; the age covers the
/// moment between a writer creating its file and locking it, and filesystems without locks.
const STALE_PARTIAL_AGE: Duration = Duration::from_secs(10 * 60);

const PARTIAL_SUFFIX: &str = ".partial";
const MINIDUMP_SUFFIX: &str = "dmp";
const NATIVE_SUFFIX: &str = "native.txt";
const PANIC_PREFIX: &str = "panic-";
const TEXT_SUFFIX: &str = ".txt";

/// What kind of crash artifact a [`PendingReport`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReportKind {
    /// A minidump written by the desktop crash monitor (`<run>.dmp`, and `<run>.<n>.dmp` for a
    /// run's later dumps).
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
    /// The report's number among its run's reports of the same kind, which orders them.
    seq: u32,
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

/// Lists the complete crash reports in `dir`, oldest run first, then by kind, then in the order
/// the run wrote them.
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
        let Some((kind, run_id, seq)) = file_name.to_str().and_then(classify) else {
            continue;
        };
        reports.push(PendingReport { kind, run_id: run_id.to_owned(), seq, path: entry.path() });
    }
    reports.sort_by(|a, b| (&a.run_id, a.kind, a.seq, &a.path).cmp(&(&b.run_id, b.kind, b.seq, &b.path)));
    Ok(reports)
}

/// Splits a report file name into its kind, run ID and number within the run, or `None` if it
/// is not a finished report.
fn classify(file_name: &str) -> Option<(ReportKind, &str, u32)> {
    let (run_id, suffix) = file_name.split_once('.')?;
    if !is_valid_run_id(run_id) {
        return None;
    }
    let (kind, seq) = match suffix {
        MINIDUMP_SUFFIX => (ReportKind::Minidump, 0),
        NATIVE_SUFFIX => (ReportKind::NativeSignal, 0),
        _ => {
            if let Some(seq) = suffix.strip_suffix(MINIDUMP_SUFFIX).and_then(|seq| seq.strip_suffix('.')) {
                (ReportKind::Minidump, parse_seq(seq)?)
            } else {
                let seq = suffix.strip_prefix(PANIC_PREFIX)?.strip_suffix(TEXT_SUFFIX)?;
                (ReportKind::Panic, parse_seq(seq)?)
            }
        }
    };
    Some((kind, run_id, seq))
}

/// A report's number within its run: plain decimal digits.
fn parse_seq(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// A run ID: start time in Unix milliseconds (zero-padded so names sort) and the process ID.
pub(crate) fn new_run_id(unix_ms: u64, pid: u32) -> String {
    format!("{unix_ms:013}-{pid}")
}

pub(crate) fn is_valid_run_id(run_id: &str) -> bool {
    !run_id.is_empty() && run_id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

/// The name of a run's `seq`-th minidump, counting from 0: `<run>.dmp`, then `<run>.<seq>.dmp`.
/// The first keeps the name a run with a single crash always had.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
pub(crate) fn minidump_file_name(run_id: &str, seq: u32) -> String {
    if seq == 0 { format!("{run_id}.{MINIDUMP_SUFFIX}") } else { format!("{run_id}.{seq}.{MINIDUMP_SUFFIX}") }
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

/// Creates (or truncates) a `.partial` file and takes its writer's lock, which is held until the
/// returned file is dropped, so keep it open until the rename to the final name is done.
///
/// Locking is best effort: where the filesystem has no locks, the startup cleanup can't prove
/// the writer gone either, and it keeps the file.
pub(crate) fn create_partial(path: &Path) -> io::Result<File> {
    let file = File::create(path)?;
    let _ = file.try_lock();
    Ok(file)
}

/// Writes `contents` to `dir/name` through a `.partial` file, synced to disk before the rename.
pub(crate) fn write_atomically(dir: &Path, name: &str, contents: &[u8]) -> io::Result<PathBuf> {
    let final_path = dir.join(name);
    let partial_path = dir.join(partial_name(name));
    // The file, and with it the writer's lock, is dropped only after the rename.
    let result = create_partial(&partial_path).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&partial_path, &final_path)
    });
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

/// Removes `.partial` files whose writer is gone: runs that crashed or were killed mid-write.
///
/// The dump directory is shared by every process of the app (several sandboxes, captures, golden
/// scenes), and a run's start order says nothing about whether its writer still lives: a crashed
/// app's monitor may be writing its dump right now. So a file is removed only when all of these
/// hold:
///
/// - its name is one of this crate's reports plus `.partial`, from another run;
/// - it went unmodified for [`STALE_PARTIAL_AGE`] before this run started (taken from
///   `current_run_id`, so the cleanup reads no clock);
/// - its writer's lock is free (see [`create_partial`]): the lock goes when the writer closes the
///   file or dies, whichever comes first.
///
/// Called from [`crate::install`], before this run writes anything.
pub(crate) fn cleanup_stale_partials(dir: &Path, current_run_id: &str) {
    let Some(run_start) = run_start(current_run_id) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(PARTIAL_SUFFIX)) else {
            continue;
        };
        // The stem is the final report name, e.g. `<run>.dmp`; anything else isn't ours.
        let Some((_, run_id, _)) = classify(stem) else {
            continue;
        };
        if run_id == current_run_id {
            continue;
        }
        let path = entry.path();
        // Removed while still locked by this process, so a second cleanup can't lock it meanwhile.
        if let Some(_lock) = abandoned_partial(&path, run_start) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Opens `path` and takes its writer's lock if the file is stale as of `run_start` and no live
/// process holds the lock. `None` means the file must be kept.
fn abandoned_partial(path: &Path, run_start: SystemTime) -> Option<File> {
    let file = File::open(path).ok()?;
    let modified = file.metadata().and_then(|metadata| metadata.modified()).ok()?;
    // A file modified after this run started has a negative age and is kept.
    let age = run_start.duration_since(modified).ok()?;
    if age < STALE_PARTIAL_AGE {
        return None;
    }
    // `WouldBlock` means a writer is alive; any other error means the lock can't tell.
    file.try_lock().ok()?;
    Some(file)
}

/// When the run started, from its ID's millisecond field.
fn run_start(run_id: &str) -> Option<SystemTime> {
    let (unix_ms, _pid) = run_id.split_once('-')?;
    UNIX_EPOCH.checked_add(Duration::from_millis(unix_ms.parse().ok()?))
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
        assert_eq!(minidump_file_name(run, 0), format!("{run}.dmp"));
        assert_eq!(minidump_file_name(run, 3), format!("{run}.3.dmp"));
        assert_eq!(classify(&minidump_file_name(run, 0)), Some((ReportKind::Minidump, run, 0)));
        assert_eq!(classify(&minidump_file_name(run, 3)), Some((ReportKind::Minidump, run, 3)));
        assert_eq!(classify(&panic_file_name(run, 0)), Some((ReportKind::Panic, run, 0)));
        assert_eq!(classify(&panic_file_name(run, 17)), Some((ReportKind::Panic, run, 17)));
        assert_eq!(classify(&native_file_name(run)), Some((ReportKind::NativeSignal, run, 0)));

        assert_eq!(classify(&partial_name(&minidump_file_name(run, 0))), None);
        assert_eq!(classify(&partial_name(&minidump_file_name(run, 1))), None);
        assert_eq!(classify(&partial_name(&panic_file_name(run, 0))), None);
        assert_eq!(classify(&format!("{run}.panic-.txt")), None);
        assert_eq!(classify(&format!("{run}.panic-x.txt")), None);
        assert_eq!(classify(&format!("{run}..dmp")), None);
        assert_eq!(classify(&format!("{run}.x.dmp")), None);
        assert_eq!(classify(&format!("{run}.99999999999.dmp")), None);
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

    /// Creates `name` in `dir`, last modified `age` before `run` started. Its lock is free.
    fn partial_from(dir: &Path, name: &str, run: &str, age: Duration) -> File {
        let file = File::create(dir.join(name)).expect("create partial");
        let modified = run_start(run).expect("run start") - age;
        file.set_modified(modified).expect("set mtime");
        file
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .expect("list")
            .map(|entry| entry.expect("entry").file_name().into_string().expect("utf-8"))
            .collect();
        names.sort();
        names
    }

    fn sorted(mut names: Vec<String>) -> Vec<String> {
        names.sort();
        names
    }

    const HOUR: Duration = Duration::from_secs(60 * 60);
    /// A writer's run started at this Unix millisecond; the current run two hours later.
    const WRITER_START_MS: u64 = 1_761_300_000_000;
    const CURRENT_START_MS: u64 = WRITER_START_MS + 2 * 3_600_000;

    #[test]
    fn cleanup_removes_abandoned_partials() {
        let dir = ScratchDir::new("cleanup");
        let old_run = new_run_id(WRITER_START_MS, 10);
        let current_run = new_run_id(CURRENT_START_MS, 20);
        for name in [
            partial_name(&minidump_file_name(&old_run, 0)),
            partial_name(&minidump_file_name(&old_run, 2)),
            partial_name(&panic_file_name(&old_run, 3)),
            partial_name(&native_file_name(&old_run)),
        ] {
            drop(partial_from(dir.path(), &name, &current_run, HOUR));
        }
        // This run's own partial (none exist at install time, and one would be live) and a
        // finished report are kept.
        let current_partial = partial_name(&minidump_file_name(&current_run, 0));
        drop(partial_from(dir.path(), &current_partial, &current_run, HOUR));
        let finished = minidump_file_name(&old_run, 0);
        drop(partial_from(dir.path(), &finished, &current_run, HOUR));

        cleanup_stale_partials(dir.path(), &current_run);
        assert_eq!(names_in(dir.path()), sorted(vec![current_partial, finished]));
    }

    #[test]
    fn cleanup_keeps_a_partial_whose_writer_is_alive() {
        // PA-001: a process that started earlier is still writing its dump. Its file is old (a
        // long write), but the writer holds its lock, so the file must survive.
        let dir = ScratchDir::new("cleanup-live");
        let writer_run = new_run_id(WRITER_START_MS, 10);
        let current_run = new_run_id(CURRENT_START_MS, 20);
        let name = partial_name(&minidump_file_name(&writer_run, 0));
        let writer = create_partial(&dir.path().join(&name)).expect("the writer's partial");
        writer.set_modified(run_start(&current_run).expect("run start") - HOUR).expect("set mtime");

        cleanup_stale_partials(dir.path(), &current_run);
        assert_eq!(names_in(dir.path()), std::slice::from_ref(&name), "a live writer's partial must survive");

        // The writer finishes: its rename still finds the file, and with the file closed a later
        // cleanup leaves the finished report alone.
        let finished = minidump_file_name(&writer_run, 0);
        fs::rename(dir.path().join(&name), dir.path().join(&finished)).expect("rename");
        drop(writer);
        cleanup_stale_partials(dir.path(), &current_run);
        assert_eq!(names_in(dir.path()), [finished]);
    }

    #[test]
    fn cleanup_keeps_a_fresh_partial() {
        // A writer between creating its file and locking it: only the file's age protects it.
        let dir = ScratchDir::new("cleanup-fresh");
        let writer_run = new_run_id(WRITER_START_MS, 10);
        let current_run = new_run_id(WRITER_START_MS + 1, 20);
        let name = partial_name(&panic_file_name(&writer_run, 0));
        let age = STALE_PARTIAL_AGE - Duration::from_secs(1);
        let _writer = partial_from(dir.path(), &name, &current_run, age);

        cleanup_stale_partials(dir.path(), &current_run);
        assert_eq!(names_in(dir.path()), [name]);
    }

    #[test]
    fn cleanup_leaves_files_it_did_not_write() {
        let dir = ScratchDir::new("cleanup-foreign");
        let old_run = new_run_id(WRITER_START_MS, 10);
        let current_run = new_run_id(CURRENT_START_MS, 20);
        let foreign = vec![partial_name(&format!("{old_run}.log")), "junk.partial".to_owned()];
        for name in &foreign {
            drop(partial_from(dir.path(), name, &current_run, HOUR));
        }

        cleanup_stale_partials(dir.path(), &current_run);
        assert_eq!(names_in(dir.path()), sorted(foreign));
    }

    #[test]
    fn partial_writers_hold_their_lock_until_they_close() {
        let dir = ScratchDir::new("partial-lock");
        let path = dir.path().join("x.dmp.partial");
        let writer = create_partial(&path).expect("create");
        let other = File::open(&path).expect("open");
        assert!(other.try_lock().is_err(), "the writer's lock must be held");
        drop(writer);
        other.try_lock().expect("free once the writer has closed the file");
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
            panic_file_name(&new_run, 10),
            minidump_file_name(&new_run, 2),
            panic_file_name(&new_run, 2),
            minidump_file_name(&new_run, 10),
            minidump_file_name(&new_run, 0),
            panic_file_name(&old_run, 1),
            native_file_name(&old_run),
            partial_name(&minidump_file_name(&old_run, 0)),
            "unrelated.txt".to_owned(),
        ] {
            fs::write(dir.path().join(name), b"x").expect("write");
        }
        fs::create_dir(dir.path().join(minidump_file_name("123-4", 0))).expect("mkdir");

        let reports = pending_reports(dir.path()).expect("list");
        let summary: Vec<_> = reports.iter().map(|r| (r.run_id(), r.kind(), r.seq)).collect();
        // Within a run, reports of a kind come in the order the run wrote them (not by name, which
        // would put `.10.dmp` before `.2.dmp` and both before `.dmp`).
        assert_eq!(
            summary,
            [
                (old_run.as_str(), ReportKind::Panic, 1),
                (old_run.as_str(), ReportKind::NativeSignal, 0),
                (new_run.as_str(), ReportKind::Minidump, 0),
                (new_run.as_str(), ReportKind::Minidump, 2),
                (new_run.as_str(), ReportKind::Minidump, 10),
                (new_run.as_str(), ReportKind::Panic, 2),
                (new_run.as_str(), ReportKind::Panic, 10),
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
