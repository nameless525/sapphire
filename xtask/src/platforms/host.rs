//! Each run's own copy of what it built for the host.
//!
//! Host builds of one profile, whatever their features, leave their executables at one path
//! (`target/debug/sapphire_sandbox`), each replacing the last, and cargo's own lock only keeps the builds
//! from overlapping. So a run going on from its build to launch that path can launch something else:
//! `golden` launches it again for every scene, long after its build. A `cargo build` or `cargo test` by
//! hand with other features replaces it at any moment. The content's lock keeps xtask's own runs apart,
//! but for the moment a run lets go of it to take it shared. On Windows a running executable can't be
//! replaced at all, which would fail any build beside one (inferred: no Windows here).
//!
//! So a run takes [`HostExecutable`]: it claims the host's builds, builds, copies the executable to a path
//! of its own, lets go, and launches the copy. The claim ends there rather than when the app exits, or a
//! long `run --features dev` would hold up `golden` for as long as it plays. It's taken last and let go
//! first, after a platform's bundle and the content, and nothing else is taken while it's held, so nothing
//! can wait on it in a circle. A build by hand takes no claim, so one landing between a run's build and
//! its copy, a few milliseconds, still gets copied.
//!
//! A copy goes when its run does. One whose run was killed, which never got to remove it, goes with the
//! next host build: each holds a lock for as long as it's in use, which the operating system lets go of
//! however a run ends, so one nobody holds is nobody's.

use std::fs::{self, File, TryLockError};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use super::{App, Options, desktop};
use crate::workspace;

/// The host's executable as a run should launch it. Keep this until that's done.
#[derive(Debug)]
pub struct HostExecutable {
    path: PathBuf,
    built: PathBuf,
    _copy: Option<PrivateCopy>,
}

impl HostExecutable {
    /// The app's: `--exe` as given, or a fresh build's, copied for this run.
    pub(super) fn for_app(app: &App, options: &Options) -> anyhow::Result<Self> {
        let env = desktop::build_env()?;
        let build = || super::executable(app, None, &env, options);
        if options.exe.is_some() {
            let path = build()?;
            return Ok(Self { built: path.clone(), path, _copy: None });
        }
        Self::private(&workspace::output_dir(), build)
    }

    /// What `build` leaves, copied under `target/host-runs/`, with the host's builds claimed until then.
    fn private(target: &Path, build: impl FnOnce() -> anyhow::Result<PathBuf>) -> anyhow::Result<Self> {
        fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))?;
        let claim = workspace::lock_file(&target.join("host-build.lock"))?;
        workspace::wait_for_lock(&claim, File::try_lock, File::lock, "to finish building for the host")?;
        let built = build()?;
        let runs = target.join("host-runs");
        remove_abandoned(&runs);
        let copy = PrivateCopy::of(&built, &runs)?;
        let path = copy.executable.clone();
        Ok(Self { path, built, _copy: Some(copy) })
    }

    /// Where to launch it from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where the build left it, which stays after this run, though it may hold another's by then: what a
    /// `--dry-run` can tell someone to launch.
    pub(super) fn built(&self) -> &Path {
        &self.built
    }
}

/// A run's directory under `target/host-runs/`: the executable, what must lie beside it, and the lock
/// that says it's in use.
#[derive(Debug)]
struct PrivateCopy {
    dir: PathBuf,
    executable: PathBuf,
    /// `None` only while being dropped.
    in_use: Option<File>,
}

impl PrivateCopy {
    const LOCK: &str = "in-use.lock";

    /// Copies `built` to a new directory under `runs`.
    fn of(built: &Path, runs: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(runs).with_context(|| format!("failed to create {}", runs.display()))?;
        let dir = new_dir(runs)?;
        let in_use = workspace::lock_file(&dir.join(Self::LOCK))?;
        // Nothing can have found it yet: looking for abandoned copies takes the same claim as this.
        in_use.try_lock().with_context(|| format!("failed to lock {}", dir.display()))?;
        // From here on, a failure removes it.
        let mut copy = Self { executable: PathBuf::new(), dir, in_use: Some(in_use) };
        let name = built.file_name().context("the executable path has no file name")?;
        copy.executable = copy.dir.join(name);
        fs::copy(built, &copy.executable)
            .with_context(|| format!("failed to copy {} for this run", built.display()))?;
        // Windows looks beside the executable for a DLL first: WARP's `d3d10warp.dll` lies beside a build,
        // and without it there the system's own, another version, would load.
        let beside = built.parent().context("the executable path has no directory")?;
        for entry in fs::read_dir(beside).with_context(|| format!("failed to read {}", beside.display()))? {
            let entry = entry?;
            let is_library =
                entry.path().extension().is_some_and(|extension| extension.eq_ignore_ascii_case("dll"));
            if is_library && entry.file_type()?.is_file() {
                fs::copy(entry.path(), copy.dir.join(entry.file_name()))
                    .with_context(|| format!("failed to copy {} for this run", entry.path().display()))?;
            }
        }
        Ok(copy)
    }
}

impl Drop for PrivateCopy {
    fn drop(&mut self) {
        // Windows won't remove a file that's open.
        self.in_use = None;
        if let Err(error) = workspace::remove_dir_if_exists(&self.dir) {
            eprintln!("warning: {error:#}");
        }
    }
}

/// A new directory under `runs`, named for this process.
fn new_dir(runs: &Path) -> anyhow::Result<PathBuf> {
    let mut attempt = 0;
    loop {
        let dir = runs.join(format!("{}-{attempt}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => attempt += 1,
            Err(error) => return Err(error).with_context(|| format!("failed to create {}", dir.display())),
        }
    }
}

/// Removes what runs that ended without tidying up left under `runs`. Call it with the host's builds
/// claimed, so nothing is being made there. A copy that won't go, on Windows one a killed run's app
/// still runs, goes with a later build.
fn remove_abandoned(runs: &Path) {
    let Ok(entries) = fs::read_dir(runs) else { return };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(in_use) = workspace::lock_file(&dir.join(PrivateCopy::LOCK)) else { continue };
        match in_use.try_lock() {
            Ok(()) => {
                drop(in_use);
                eprintln!("removing {}, left by a run that was stopped", dir.display());
                if let Err(error) = workspace::remove_dir_if_exists(&dir) {
                    eprintln!("warning: {error:#}");
                }
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => {
                eprintln!("warning: failed to check {}: {error}", dir.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    /// A scratch `target/` of a test's own, removed afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("xtask-host-{name}-{}", std::process::id()));
            workspace::remove_dir_if_exists(&dir).unwrap();
            fs::create_dir_all(dir.join("debug")).unwrap();
            Self(dir)
        }

        /// A build that replaces the shared path, as cargo does.
        fn build(&self, contents: &str) -> anyhow::Result<PathBuf> {
            let shared = self.0.join("debug/app");
            let _ = fs::remove_file(&shared);
            fs::write(&shared, contents)?;
            Ok(shared)
        }

        fn runs(&self) -> Vec<PathBuf> {
            let Ok(entries) = fs::read_dir(self.0.join("host-runs")) else { return Vec::new() };
            entries.map(|entry| entry.unwrap().path()).collect()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = workspace::remove_dir_if_exists(&self.0);
        }
    }

    fn contents(executable: &HostExecutable) -> String {
        fs::read_to_string(executable.path()).unwrap()
    }

    /// Two runs of two builds with one path. The second can't build between the first's build and its copy,
    /// and once it has, the first still launches what it built, as often as it likes.
    #[test]
    fn a_run_launches_what_it_built_whatever_is_built_after() {
        let scratch = Scratch::new("two-runs");
        let scratch = &scratch;
        let (first_built, first_is_built) = mpsc::channel();
        let (second_built, second_is_built) = mpsc::channel();
        let (go_first, first_go) = mpsc::channel::<()>();
        let run = move |contents: &'static str, built: mpsc::Sender<()>, go: mpsc::Receiver<()>| {
            HostExecutable::private(&scratch.0, || {
                let path = scratch.build(contents)?;
                built.send(()).unwrap();
                // As a build that's slow to get to its copy: a dropped sender means go.
                let _ = go.recv();
                Ok(path)
            })
            .unwrap()
        };
        let (_, second_go) = mpsc::channel::<()>();
        // Everything moves in, so a failure here lets go of the runs' channels and they end with it.
        thread::scope(move |scope| {
            let first = scope.spawn(move || run("debug", first_built, first_go));
            first_is_built.recv().unwrap();
            let second = scope.spawn(move || run("dev", second_built, second_go));
            assert_eq!(
                second_is_built.recv_timeout(Duration::from_millis(300)),
                Err(mpsc::RecvTimeoutError::Timeout),
                "the second run built before the first had its copy"
            );
            go_first.send(()).unwrap();
            let first = first.join().unwrap();
            assert_eq!(contents(&first), "debug");
            second_is_built.recv().unwrap();
            let second = second.join().unwrap();
            assert_eq!(contents(&second), "dev");
            assert_eq!(contents(&first), "debug", "the second's build left the first's copy alone");
            assert_ne!(first.path(), second.path());
            assert_eq!(fs::read_to_string(first.built()).unwrap(), "dev", "the shared path moved on");
        });
    }

    #[test]
    fn a_run_leaves_nothing_behind_when_it_ends_or_fails() {
        let scratch = Scratch::new("tidy");
        let executable = HostExecutable::private(&scratch.0, || scratch.build("one")).unwrap();
        assert_eq!(scratch.runs().len(), 1);
        drop(executable);
        assert_eq!(scratch.runs(), Vec::<PathBuf>::new());

        // A build that fails has nothing to copy, and lets go of the claim.
        let failed = HostExecutable::private(&scratch.0, || anyhow::bail!("it didn't compile"));
        assert!(failed.is_err());
        // An executable that vanished fails its copy, which takes its directory with it.
        let missing = HostExecutable::private(&scratch.0, || Ok(scratch.0.join("debug/nothing")));
        assert!(missing.is_err());
        assert_eq!(scratch.runs(), Vec::<PathBuf>::new());
        assert!(HostExecutable::private(&scratch.0, || scratch.build("two")).is_ok());
        assert!(scratch.0.join("debug/app").is_file(), "the build's own stays");
    }

    /// A run that was killed never removed its copy, which the next build does, and it leaves alone a run
    /// that's still going. A run's lock going with its process is the operating system's.
    #[test]
    fn the_next_build_clears_a_killed_runs_copy_and_no_live_ones() {
        let scratch = Scratch::new("killed");
        let killed = scratch.0.join("host-runs/1-0");
        fs::create_dir_all(&killed).unwrap();
        fs::write(killed.join("app"), "abandoned").unwrap();
        fs::write(killed.join(PrivateCopy::LOCK), "").unwrap();
        let live = HostExecutable::private(&scratch.0, || scratch.build("live")).unwrap();
        assert!(!killed.exists(), "a copy nobody holds goes");

        let next = HostExecutable::private(&scratch.0, || scratch.build("next")).unwrap();
        assert_eq!(contents(&live), "live", "a copy in use stays");
        assert_eq!(contents(&next), "next");
        assert_eq!(scratch.runs().len(), 2);
    }

    /// Without its own `d3d10warp.dll` beside it, a Windows build quietly gets the system's.
    #[test]
    fn what_loads_from_beside_the_executable_comes_along() {
        let scratch = Scratch::new("beside");
        fs::write(scratch.0.join("debug/d3d10warp.dll"), "warp").unwrap();
        fs::write(scratch.0.join("debug/libsomething.d"), "not this").unwrap();
        fs::create_dir(scratch.0.join("debug/deps")).unwrap();
        let executable = HostExecutable::private(&scratch.0, || scratch.build("app")).unwrap();
        let dir = executable.path().parent().unwrap();
        assert_eq!(fs::read_to_string(dir.join("d3d10warp.dll")).unwrap(), "warp");
        assert!(!dir.join("libsomething.d").exists());
        assert!(!dir.join("deps").exists());
    }

    /// `--exe` is the caller's own file: nothing's claimed or copied.
    #[test]
    fn a_given_executable_is_launched_where_it_is() {
        let scratch = Scratch::new("given");
        let given = scratch.build("given").unwrap();
        let options = Options { exe: Some(given.clone()), ..Options::default() };
        let executable = HostExecutable::for_app(&crate::platforms::test_app(), &options).unwrap();
        assert_eq!(executable.path(), given);
        assert_eq!(scratch.runs(), Vec::<PathBuf>::new());
    }
}
