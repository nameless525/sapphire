//! A crash read back end to end, from this host's own linker output (M4 C2): a copy of this program crashes
//! under `sapphire_crash`'s real monitor, the way a shipping game does when asked (`ForcedCrash`), and the
//! minidump and panic report it leaves are read with the `.sym` written from this program's own symbols (a
//! dSYM `dsymutil` makes on macOS, the unstripped ELF on Linux, the PDB on Windows), found through a
//! `package.json` as a package's are. Each must name the forced crash's function, file and line.
//!
//! The monitor is this program launched again, so it owns `main` (`harness = false`). CI's test job runs it
//! on all three desktops; elsewhere it runs only with `SAPPHIRE_CRASH_TESTS=1`, since every crash leaves an
//! OS crash report behind, as `sapphire_crash`'s own monitor test does.

#![allow(clippy::print_stdout)]

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod desktop {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use sapphire_crash::{CrashConfig, ForcedCrash, ReportKind, pending_reports};
    use sapphire_symbols::crash::{self, Frame, PathMap};
    use sapphire_symbols::store::{self, Store};

    const KIND_ENV: &str = "SAPPHIRE_SYMBOLS_TEST_CRASH";
    const DIR_ENV: &str = "SAPPHIRE_SYMBOLS_TEST_DIR";

    pub fn main() {
        if let Some(kind) = std::env::var_os(KIND_ENV) {
            let dir = PathBuf::from(std::env::var_os(DIR_ENV).expect("the parent names the dump dir"));
            let crash = ForcedCrash::from_name(kind.to_str().expect("a kind")).expect("a kind of crash");
            let _guard =
                sapphire_crash::install(CrashConfig::new("crash_test", "0", dir)).expect("crash capture");
            crash.now();
        }
        let enabled =
            |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0");
        if !enabled("CI") && !enabled("SAPPHIRE_CRASH_TESTS") {
            println!(
                "crash: skipped (it leaves an OS crash report behind); set SAPPHIRE_CRASH_TESTS=1 to run it, as CI does"
            );
            return;
        }

        let dir = std::env::temp_dir().join(format!("sapphire_symbols-crash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let package = package_this_program(&dir.join("package"));
        let packaged = store::packaged(std::slice::from_ref(&package)).expect("the package's modules");
        let store = Store { root: dir.join("store") };
        let maps: Vec<PathMap> = Vec::new();

        let fault = crash_a_copy(&dir.join("fault"), ForcedCrash::Fault);
        let dump =
            fault.iter().find(|(kind, _)| *kind == ReportKind::Minidump).expect("the fault's minidump");
        let read = crash::read(&dump.1, &packaged, &store).expect("the minidump reads");
        println!("{}", crash::render(&read, &maps));
        let first = &read.threads[0].frames[0];
        assert_names(first, "forced_fault", "*address = ");

        let panicked = crash_a_copy(&dir.join("panic"), ForcedCrash::Panic);
        // Windows' panic report lists no modules: its minidump is what's read there.
        if !cfg!(windows) {
            let report =
                panicked.iter().find(|(kind, _)| *kind == ReportKind::Panic).expect("the panic report");
            let read = crash::read(&report.1, &packaged, &store).expect("the panic report reads");
            println!("{}", crash::render(&read, &maps));
            let frame = read.threads[0]
                .frames
                .iter()
                .find(|frame| {
                    frame.location.function.as_deref().is_some_and(|name| name.ends_with("forced_panic"))
                })
                .unwrap_or_else(|| panic!("no frame is the forced panic's: {read:#?}"));
            // Linux's panic report names `forced_panic` but not its line, the call into the panic handler
            // that ends it, though every other frame's line reads and the minidump's do (untraced; macOS
            // reads it whole).
            if std::env::consts::OS != "linux" {
                assert_names(frame, "forced_panic", "panic!(");
            }
        }
        fs::remove_dir_all(&dir).expect("the test's directory goes");
        println!("crash: ok");
    }

    /// Fails unless `frame` is `function` in `sapphire_crash`'s `forced.rs`, at the line that holds `code`.
    fn assert_names(frame: &Frame, function: &str, code: &str) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/sapphire_crash/src/forced.rs");
        let text = fs::read_to_string(&source).expect("forced.rs");
        let line = text.lines().position(|line| line.contains(code)).expect("the crash's line") + 1;
        let location = &frame.location;
        assert!(
            location.function.as_deref().is_some_and(|name| name.ends_with(&format!("forced::{function}"))),
            "{frame:#?}"
        );
        let file = location.file.as_deref().unwrap_or_default().replace('\\', "/");
        assert!(file.ends_with("sapphire_crash/src/forced.rs"), "{frame:#?}");
        assert_eq!(location.line, Some(u32::try_from(line).unwrap()), "{frame:#?}");
    }

    /// Runs this program again, asking it to crash with `kind` and keep its reports in `dir`, and returns
    /// what it left.
    fn crash_a_copy(dir: &Path, kind: ForcedCrash) -> Vec<(ReportKind, PathBuf)> {
        let exe = std::env::current_exe().expect("this program");
        let status =
            Command::new(exe).env(KIND_ENV, kind.name()).env(DIR_ENV, dir).status().expect("the copy runs");
        assert!(!status.success(), "a {} crash exited {status}", kind.name());
        // The monitor writes the dump before the crashed process goes on to die; give a slow disk up to 30 s.
        let mut tries = 300;
        loop {
            let reports = pending_reports(dir).expect("the dump dir reads");
            let has_dump = reports.iter().any(|report| report.kind() == ReportKind::Minidump);
            tries -= 1;
            if has_dump || tries == 0 {
                return reports.iter().map(|report| (report.kind(), report.path().to_owned())).collect();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// A package of this program as `cargo xtask package` makes one: a stripped copy, the symbols from its
    /// link in `symbols/`, and `package.json` listing the module.
    fn package_this_program(package: &Path) -> PathBuf {
        let exe = std::env::current_exe().expect("this program");
        let name = exe.file_name().expect("a name").to_owned();
        fs::create_dir_all(package.join("symbols")).expect("the package's directory");
        let shipped = package.join(&name);
        fs::copy(&exe, &shipped).expect("a copy to strip");
        let symbols = match std::env::consts::OS {
            "macos" => {
                run(Command::new("strip").arg(&shipped));
                let dsym = package.join("symbols").join(format!("{}.dSYM", name.to_string_lossy()));
                run(Command::new("dsymutil").arg(&exe).arg("-o").arg(&dsym));
                dsym
            }
            "linux" => {
                run(Command::new("strip").arg("--strip-all").arg(&shipped));
                let symbols = package.join("symbols").join(&name);
                fs::copy(&exe, &symbols).expect("the unstripped symbols");
                symbols
            }
            _ => {
                let pdb = exe.with_extension("pdb");
                let symbols = package.join("symbols").join(pdb.file_name().expect("a name"));
                fs::copy(&pdb, &symbols).expect("the PDB");
                symbols
            }
        };
        let module = sapphire_symbols::check(&shipped, &symbols).expect("the copy and its symbols match");
        let mut json = module.to_json();
        let relative = format!("symbols/{}", symbols.file_name().expect("a name").to_string_lossy());
        json["symbols"] = relative.into();
        let manifest = serde_json::json!({ "modules": [json] });
        fs::write(package.join("package.json"), manifest.to_string()).expect("package.json");
        package.to_owned()
    }

    fn run(command: &mut Command) {
        let status = command.status().unwrap_or_else(|error| panic!("{command:?} didn't start: {error}"));
        assert!(status.success(), "{command:?} failed: {status}");
    }
}

fn main() {
    // A monitor launched by the crashing copy serves its dump here and exits.
    sapphire_crash::run_monitor_if_requested();
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    desktop::main();
}
