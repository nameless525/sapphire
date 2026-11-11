//! End-to-end checks of desktop crash capture with its real monitor process (spec §5.2).
//!
//! A panic the app recovers from (an IO callback, an asset decode worker, an off-thread shader
//! build) must not use up native crash capture: each recovered panic leaves its report and its
//! own minidump, the monitor stays up, and a native crash afterwards still gets a minidump. A
//! run asks for at most `MAX_PANIC_DUMPS` panic dumps, so a panic loop can't fill the disk.
//!
//! The monitor is this test binary launched again with a private flag, so the test supplies its
//! own `main` (`harness = false`) whose first line is `run_monitor_if_requested`, as an app's
//! must be. Each scenario runs in a child process (this binary again, with
//! `SAPPHIRE_CRASH_TEST_SCENARIO` set), because one of them ends in a real native crash. The
//! child reports what it saw on stdout, one `key=value` line per step, and the parent checks it.
//! The monitor inherits the child's stdout, so stdout reaching end-of-file also proves that the
//! monitor exited once the child was gone.
//!
//! The native-crash scenarios run only on CI or with `SAPPHIRE_CRASH_TESTS=1`, since each run
//! leaves an OS crash report behind (see `native_crash_enabled`).

// The child's stdout is the channel back to the parent.
#![allow(clippy::print_stdout)]

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod desktop {
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;
    use std::{fs, panic, thread};

    use sapphire_crash::{CrashConfig, NativeCapture, ReportKind, pending_reports};

    const SCENARIO_ENV: &str = "SAPPHIRE_CRASH_TEST_SCENARIO";
    const DIR_ENV: &str = "SAPPHIRE_CRASH_TEST_DIR";

    /// How long a child (and the monitor it launched) may take. Minidumps take a fraction of a
    /// second each; this only keeps a regression from hanging the test run.
    const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

    /// Panic dumps a run asks for, mirrored from the crate (`panic_hook::MAX_PANIC_DUMPS`).
    const MAX_PANIC_DUMPS: usize = 8;
    /// Earlier runs whose reports `install` keeps, mirrored from the crate (`report::KEPT_RUNS`).
    const KEPT_RUNS: usize = 8;

    pub fn main() {
        if let Some(scenario) = std::env::var_os(SCENARIO_ENV) {
            let dir = PathBuf::from(std::env::var_os(DIR_ENV).expect("the parent sets the dump dir"));
            match scenario.to_str() {
                Some("panics-then-crash") => panics_then_crash(&dir),
                Some("panic-cap") => panic_cap(&dir),
                Some("prune") => prune(&dir),
                #[cfg(unix)]
                Some("monitor-gone") => monitor_gone(&dir),
                #[cfg(target_os = "linux")]
                Some("monitor-stopped") => monitor_stopped(&dir),
                other => panic!("unknown scenario {other:?}"),
            }
            return;
        }

        if native_crash_enabled() {
            two_recovered_panics_then_a_native_crash();
            #[cfg(target_os = "linux")]
            a_hung_monitor_still_ends_the_crash();
        } else {
            println!(
                "monitor: skipping the native-crash scenario (it leaves an OS crash report behind); \
                 set {NATIVE_CRASH_ENV}=1 to run it, as CI does"
            );
        }
        panic_dumps_stop_at_the_cap();
        install_prunes_old_runs();
        #[cfg(unix)]
        capture_reports_a_dead_monitor();
        println!("monitor: ok");
    }

    /// Asks for the scenario that ends in a real native crash outside CI.
    const NATIVE_CRASH_ENV: &str = "SAPPHIRE_CRASH_TESTS";

    /// Whether to run the scenario that ends in a real native crash: on CI (GitHub Actions sets
    /// `CI`), or when [`NATIVE_CRASH_ENV`] is `1`. On macOS each run leaves a crash report in
    /// `~/Library/Logs/DiagnosticReports`, which shouldn't pile up on a machine where the tests
    /// run over and over. The panic-only scenarios, recovered panics' dumps included, run
    /// everywhere.
    fn native_crash_enabled() -> bool {
        let ci =
            std::env::var_os("CI").is_some_and(|value| !value.is_empty() && value != "0" && value != "false");
        ci || std::env::var_os(NATIVE_CRASH_ENV).is_some_and(|value| value == "1")
    }

    // ---------------------------------------------------------------------------------------
    // Parent side.

    /// Two recovered panics, each with its own report and dump, then a native crash that still
    /// gets a dump of its own.
    fn two_recovered_panics_then_a_native_crash() {
        let dir = ScratchDir::new("panics-then-crash");
        let run = run_child("panics-then-crash", dir.path());
        let steps = &run.steps;
        let context = run.describe();

        assert!(!run.status.success(), "the child must die of its native crash\n{context}");
        for (step, dumps) in [("panic-0", 1), ("panic-1", 2)] {
            assert_eq!(
                steps.get(&format!("{step}.capture")).map(String::as_str),
                Some("Minidump"),
                "native capture must survive a recovered panic\n{context}",
            );
            assert_eq!(
                steps.get(&format!("{step}.dumps")),
                Some(&dumps.to_string()),
                "each recovered panic leaves its own dump\n{context}",
            );
        }
        assert_eq!(steps.get("crashing").map(String::as_str), Some("yes"), "{context}");

        let run_id = steps.get("run").expect("the child names its run");
        let reports = pending_reports(dir.path()).expect("list reports");
        let count = |kind| reports.iter().filter(|r| r.kind() == kind).count();
        assert_eq!(count(ReportKind::Panic), 2, "{reports:#?}\n{context}");
        assert_eq!(
            count(ReportKind::Minidump),
            3,
            "two panic dumps and the crash's: {reports:#?}\n{context}"
        );
        assert!(reports.iter().all(|r| r.run_id() == run_id), "{reports:#?}");
        for report in reports.iter().filter(|r| r.kind() == ReportKind::Minidump) {
            let header = fs::read(report.path()).expect("read dump");
            assert!(header.starts_with(b"MDMP"), "{} is not a minidump", report.path().display());
        }
        assert_no_partials(dir.path());
    }

    /// A run that keeps panicking stops asking for dumps at the cap, keeps writing panic
    /// reports in a bounded rolling slot, and keeps native capture up. Its monitor exits with it.
    fn panic_dumps_stop_at_the_cap() {
        let dir = ScratchDir::new("panic-cap");
        let run = run_child("panic-cap", dir.path());
        let context = run.describe();

        assert!(run.status.success(), "{context}");
        assert_eq!(run.steps.get("capture").map(String::as_str), Some("Minidump"), "{context}");
        let reports = pending_reports(dir.path()).expect("list reports");
        let count = |kind| reports.iter().filter(|r| r.kind() == kind).count();
        assert_eq!(count(ReportKind::Panic), MAX_PANIC_DUMPS + 1, "{reports:#?}");
        assert_eq!(count(ReportKind::Minidump), MAX_PANIC_DUMPS, "{reports:#?}");
        assert_no_partials(dir.path());
    }

    /// `install` deletes the reports of all but the newest earlier runs (review PN-4), which
    /// `report.rs` tests in detail.
    fn install_prunes_old_runs() {
        let dir = ScratchDir::new("prune");
        let report = |run: usize| dir.path().join(format!("{:013}-1.dmp", 1_000 + run));
        for run in 0..KEPT_RUNS + 2 {
            fs::write(report(run), b"an old run's report").expect("write a report");
        }
        let foreign = dir.path().join("notes.txt");
        fs::write(&foreign, b"not a report").expect("write a file");

        let run = run_child("prune", dir.path());
        let context = run.describe();
        assert!(run.status.success(), "{context}");
        assert_eq!(pending_reports(dir.path()).expect("list reports").len(), KEPT_RUNS, "{context}");
        assert!(!report(1).exists() && report(2).exists(), "the oldest go\n{context}");
        assert!(foreign.exists(), "{context}");
    }

    /// When the monitor dies, `native_capture` says so, and a panic's dump request fails at once
    /// instead of waiting on the dead connection.
    #[cfg(unix)]
    fn capture_reports_a_dead_monitor() {
        let dir = ScratchDir::new("monitor-gone");
        let run = run_child("monitor-gone", dir.path());
        let context = run.describe();

        assert!(run.status.success(), "{context}");
        let step = |key: &str| run.steps.get(key).map(String::as_str);
        assert_eq!(step("capture"), Some("Minidump"), "{context}");
        assert_eq!(step("killed.capture"), Some("None"), "{context}");
        assert_eq!(step("panic.capture"), Some("None"), "{context}");
        let reports = pending_reports(dir.path()).expect("list reports");
        let kinds: Vec<_> = reports.iter().map(|r| r.kind()).collect();
        assert_eq!(kinds, [ReportKind::Panic], "the panic is reported, with no dump: {reports:#?}");
    }

    /// A monitor that never answers (stopped here) costs a crash its dump, not its end: the request
    /// gives up after the crate's bound (10 s) and the process dies of its signal.
    #[cfg(target_os = "linux")]
    fn a_hung_monitor_still_ends_the_crash() {
        use std::os::unix::process::ExitStatusExt;

        const POLL: Duration = Duration::from_millis(50);
        let dir = ScratchDir::new("monitor-stopped");
        let mut child = spawn_child("monitor-stopped", dir.path());
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut waited = Duration::ZERO;
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll the scenario") {
                break Some(status);
            }
            if waited >= CHILD_TIMEOUT {
                let _ = child.kill();
                break None;
            }
            thread::sleep(POLL);
            waited += POLL;
        };
        // The stopped monitor still holds the pipe: end it, then read to the end.
        let mut lines = Vec::new();
        while let Ok(line) = receiver.recv_timeout(Duration::from_secs(1)) {
            if let Some(monitor) = line.strip_prefix("monitor=") {
                let _ = Command::new("kill").args(["-KILL", monitor]).status();
            }
            lines.push(line);
        }
        let context = format!("child exited with {status:?}; it reported:\n  {}", lines.join("\n  "));
        let Some(status) = status else {
            panic!("a crash waited on a hung monitor for {CHILD_TIMEOUT:?}\n{context}");
        };
        assert!(lines.iter().any(|line| line == "crashing=yes"), "{context}");
        assert_eq!(status.signal(), Some(11), "the crash ends in its own signal, SIGSEGV\n{context}");
        assert_no_partials(dir.path());
    }

    struct ChildRun {
        status: ExitStatus,
        lines: Vec<String>,
        steps: BTreeMap<String, String>,
    }

    impl ChildRun {
        fn describe(&self) -> String {
            format!("child exited with {}; it reported:\n  {}", self.status, self.lines.join("\n  "))
        }
    }

    fn spawn_child(scenario: &str, dir: &Path) -> Child {
        Command::new(std::env::current_exe().expect("current exe"))
            .env(SCENARIO_ENV, scenario)
            .env(DIR_ENV, dir)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn the scenario")
    }

    fn run_child(scenario: &str, dir: &Path) -> ChildRun {
        let mut child = spawn_child(scenario, dir);
        let stdout = child.stdout.take().expect("piped stdout");

        // Read until end-of-file, which needs the child and the monitor it launched (which
        // inherits this pipe) both gone.
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let lines: Vec<String> = BufReader::new(stdout).lines().map_while(Result::ok).collect();
            let _ = sender.send(lines);
        });
        let Ok(lines) = receiver.recv_timeout(CHILD_TIMEOUT) else {
            let _ = child.kill();
            panic!("scenario {scenario} (or its monitor) did not finish within {CHILD_TIMEOUT:?}");
        };
        let status = child.wait().expect("wait for the scenario");
        let steps = lines
            .iter()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        ChildRun { status, lines, steps }
    }

    fn assert_no_partials(dir: &Path) {
        let partials: Vec<_> = fs::read_dir(dir)
            .expect("list dump dir")
            .map(|entry| entry.expect("entry").file_name())
            .filter(|name| name.to_string_lossy().ends_with(".partial"))
            .collect();
        assert!(partials.is_empty(), "left behind: {partials:?}");
    }

    /// A scratch directory under the system temp dir, removed on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("sapphire_crash-monitor-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create scratch dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            // Keep the evidence when a check failed.
            if !thread::panicking() {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // Child side.

    fn install(dir: &Path) -> sapphire_crash::CrashGuard {
        // Keep the deliberate panics' messages out of the test output; the crate's hook still
        // runs first and chains to this one.
        panic::set_hook(Box::new(|_| {}));
        let guard = sapphire_crash::install(CrashConfig::new("monitor-test", "0.0.0", dir)).expect("install");
        report("run", guard.run_id());
        report("capture", &format!("{:?}", guard.native_capture()));
        guard
    }

    fn report(key: &str, value: &str) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{key}={value}");
        let _ = stdout.flush();
    }

    fn count(dir: &Path, kind: ReportKind) -> usize {
        pending_reports(dir).expect("list reports").iter().filter(|r| r.kind() == kind).count()
    }

    /// Recovers from a panic the way the engine's workers do: on a thread whose panic is
    /// caught and turned into an error.
    fn recovered_panic(index: usize) {
        let result = thread::Builder::new()
            .name(format!("worker-{index}"))
            .spawn(move || panic!("deliberate recovered panic {index}"))
            .expect("spawn")
            .join();
        assert!(result.is_err(), "the panic must reach join");
    }

    fn panics_then_crash(dir: &Path) {
        let guard = install(dir);

        // The first on this thread through `catch_unwind`, the second on a worker thread.
        let caught = panic::catch_unwind(|| panic!("deliberate recovered panic 0"));
        assert!(caught.is_err());
        report("panic-0.capture", &format!("{:?}", guard.native_capture()));
        report("panic-0.dumps", &count(dir, ReportKind::Minidump).to_string());

        recovered_panic(1);
        report("panic-1.capture", &format!("{:?}", guard.native_capture()));
        report("panic-1.dumps", &count(dir, ReportKind::Minidump).to_string());

        report("crashing", "yes");
        crash_natively();
    }

    fn panic_cap(dir: &Path) {
        let guard = install(dir);
        for index in 0..MAX_PANIC_DUMPS + 4 {
            recovered_panic(index);
        }
        report("capture", &format!("{:?}", guard.native_capture()));
        assert_eq!(guard.native_capture(), NativeCapture::Minidump);
        drop(guard);
    }

    fn prune(dir: &Path) {
        let guard = install(dir);
        // It runs on a thread of its own, so wait for it: up to a minute, well inside the timeout.
        for _ in 0..6_000 {
            if pending_reports(dir).expect("list reports").len() <= KEPT_RUNS {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        drop(guard);
    }

    /// The monitor's process ID: it is this process's only child.
    #[cfg(unix)]
    fn monitor_pid() -> String {
        let pgrep =
            Command::new("pgrep").args(["-P", &std::process::id().to_string()]).output().expect("run pgrep");
        let monitor = String::from_utf8(pgrep.stdout).expect("pgrep output").trim().to_owned();
        assert!(!monitor.is_empty() && !monitor.contains('\n'), "one monitor: {monitor:?}");
        monitor
    }

    #[cfg(unix)]
    fn monitor_gone(dir: &Path) {
        let guard = install(dir);
        let monitor = monitor_pid();
        let killed = Command::new("kill").args(["-KILL", &monitor]).status().expect("run kill");
        assert!(killed.success());
        // The kill is asynchronous; give it up to five seconds to land.
        for _ in 0..500 {
            if guard.native_capture() == NativeCapture::None {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        report("killed.capture", &format!("{:?}", guard.native_capture()));

        recovered_panic(0);
        report("panic.capture", &format!("{:?}", guard.native_capture()));
        drop(guard);
    }

    /// Stops the monitor, so it never answers, then crashes. The parent ends the monitor afterwards.
    #[cfg(target_os = "linux")]
    fn monitor_stopped(dir: &Path) {
        let _guard = install(dir);
        let monitor = monitor_pid();
        report("monitor", &monitor);
        let stopped = Command::new("kill").args(["-STOP", &monitor]).status().expect("run kill");
        assert!(stopped.success());
        report("crashing", "yes");
        crash_natively();
    }

    fn crash_natively() -> ! {
        // SAFETY: none, deliberately: a write through a null pointer raises a real access
        // violation (SIGSEGV, EXC_BAD_ACCESS), which is what this scenario checks. The write
        // is volatile so the compiler keeps it.
        unsafe { std::ptr::null_mut::<u32>().write_volatile(0xDEAD) };
        std::process::abort()
    }
}

fn main() {
    // First line of main: when this process is the crash monitor, this never returns.
    sapphire_crash::run_monitor_if_requested();

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    desktop::main();
}
