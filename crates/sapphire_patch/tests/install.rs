//! Installing a staged release: what it puts where, a fault at every step of it, what it refuses, and which
//! release an app starts on. V1's saves were proved the same way (`sapphire_save`'s `test_support`): each
//! step of the disk's either fails, as a full disk does, or dies there, as a killed process does, by a panic
//! that nothing after it runs past. `install_kill.rs` kills real processes.

#![allow(clippy::print_stdout, reason = "the fault test says how many steps it covered")]

mod common;

use std::fs::{self, File};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use common::server::Host;
use common::{
    APP_BUILD, Content, EXPIRES, NOW, ShippedDir, TRUSTED, TestDir, UNTRUSTED, assert_starts_on,
    first_release, listing, retries, second_release, stage, third_release,
};
use sapphire_assets::manifest::ManifestError;
use sapphire_patch::{
    Download, HttpTransport, InstallError, InstallOutcome, InstallSteps, Installation, Location, Outcome,
    Request, Roots, Timeouts,
};
use sapphire_platform::{OsSteps, ReplaceSteps};

/// An app that shipped `shipped` as release 1, under `dir`: where it shipped, its root and its staging.
struct App {
    shipped: PathBuf,
    root: PathBuf,
    staging: PathBuf,
}

impl App {
    fn new(dir: &TestDir, shipped: &Content) -> Self {
        let app = Self { shipped: dir.join("shipped"), root: dir.join("root"), staging: dir.join("staging") };
        shipped.write_install(&app.shipped, 1);
        app
    }

    fn open(&self) -> Installation<ShippedDir> {
        Installation::open(ShippedDir(self.shipped.clone()), &self.root, &[]).expect("opens")
    }

    fn install(&self) -> Result<InstallOutcome, InstallError> {
        self.open().install(&self.staging, NOW, APP_BUILD)
    }
}

/// What the root holds once a launch on `installation`'s current release is confirmed: the lock, and if
/// it's a patch's, its manifest and what it didn't ship.
fn settled(installation: &Installation<ShippedDir>) -> Vec<String> {
    let mut names = vec!["lock".to_owned()];
    if installation.is_patched() {
        names.push("release.smanifest".to_owned());
        let current = installation.current();
        let packs = current.packs.iter().map(|pack| installation.pack(&pack.name));
        let files = current.files.iter().map(|file| installation.file(&file.path));
        for location in packs.chain(files).flatten() {
            if let Location::Patched(path) = location {
                let relative = path.strip_prefix(installation.root()).expect("under the root");
                names.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    names.sort();
    names
}

/// A patch over what the app shipped, then a patch over that: each new pack assembled from kept and fetched
/// blocks, each unchanged one left where it was, and the release before removed only by the next confirmed
/// launch.
#[test]
fn an_install_switches_releases_and_the_next_good_launch_removes_the_old() {
    let dir = TestDir::new("install_switches");
    let (first, second, third) = (first_release(), second_release(), third_release());
    let app = App::new(&dir, &first);
    let mut fresh = app.open();
    assert_starts_on(&fresh, &app.shipped, &first, 1);
    assert!(!fresh.is_patched());
    assert_eq!(fresh.confirm_launch().expect("confirms"), Default::default(), "nothing to remove");

    stage(&app.staging, &first, &second, 2);
    let mut installation = app.open();
    assert_eq!(
        installation.install(&app.staging, NOW, APP_BUILD).expect("installs"),
        InstallOutcome::Installed { from: 1, to: 2 }
    );
    assert_starts_on(&installation, &app.shipped, &second, 2);
    assert_starts_on(&app.open(), &app.shipped, &second, 2);
    // Only what changed lives under the root; the rest is still what the app shipped.
    assert_eq!(installation.pack("fonts"), Some(Location::Shipped));
    assert!(matches!(installation.pack("shared"), Some(Location::Patched(_))));
    assert!(matches!(installation.pack("audio"), Some(Location::Patched(_))));
    assert!(matches!(installation.pack("audio_extra"), Some(Location::Patched(_))));
    assert_eq!(installation.file("documents/game.flow.ron"), Some(Location::Shipped));
    assert!(matches!(installation.file("documents/ui/menus.ron"), Some(Location::Patched(_))));
    assert_eq!(listing(&app.staging), ["lock"], "the staging is cleared");
    assert_eq!(
        installation.install(&app.staging, NOW, APP_BUILD).unwrap_err().to_string(),
        "nothing is staged to install"
    );

    // A launch on release 2 installs 3 and then says it launched well: 2's packs stay, since it may still
    // mount them, until a launch on 3 is confirmed.
    stage(&app.staging, &second, &third, 3);
    let mut on_two = app.open();
    on_two.confirm_launch().expect("confirms");
    let before = listing(&app.root);
    assert_eq!(
        on_two.install(&app.staging, NOW, APP_BUILD).expect("installs"),
        InstallOutcome::Installed { from: 2, to: 3 }
    );
    assert_starts_on(&on_two, &app.shipped, &third, 3);
    on_two.confirm_launch().expect("confirms");
    let after = listing(&app.root);
    assert!(before.iter().all(|name| after.contains(name)), "{before:?} {after:?}");
    let mut installation = app.open();
    assert_starts_on(&installation, &app.shipped, &third, 3);
    let cleaned = installation.confirm_launch().expect("confirms");
    assert_eq!(listing(&app.root), settled(&installation));
    assert_eq!(cleaned.left, 0);
    assert!(cleaned.removed >= 3, "release 2's shared, loc and audio_extra, and its screen: {cleaned:?}");
    assert_starts_on(&app.open(), &app.shipped, &third, 3);

    // The same release again is nothing to do, and clears what's staged.
    stage(&app.staging, &third, &third, 3);
    assert_eq!(app.install().expect("up to date"), InstallOutcome::UpToDate { version: 3 });
    assert_eq!(listing(&app.staging), ["lock"]);
}

/// A download from a localhost host, then the install of what it staged, end to end.
#[test]
fn what_a_download_stages_installs() {
    let dir = TestDir::new("install_downloaded");
    let (first, second) = (first_release(), second_release());
    let app = App::new(&dir, &first);
    let host = Host::start(second.host(&second.signed(2, EXPIRES, TRUSTED)));
    let download = |installation: &Installation<ShippedDir>| {
        let request = Request {
            installed: installation.installed().expect("reads the install"),
            staging: app.staging.clone(),
            now: NOW,
            app_build: APP_BUILD,
            retries: retries(),
        };
        let transport =
            HttpTransport::new(&host.url(), Roots::Platform, Timeouts::default()).expect("a host");
        Download::start(request, transport).expect("starts").join()
    };
    let mut installation = app.open();
    assert!(matches!(download(&installation).expect("downloads"), Outcome::Staged(_)));
    assert_eq!(
        installation.install(&app.staging, NOW, APP_BUILD).expect("installs"),
        InstallOutcome::Installed { from: 1, to: 2 }
    );
    assert_starts_on(&app.open(), &app.shipped, &second, 2);
    // The next check finds the installed release current, through the patched packs' tables.
    assert_eq!(download(&installation).expect("checks"), Outcome::UpToDate { version: 2 });
}

/// How a fault meets a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// The step returns an error, as a full disk's does, and the app lives on.
    Fail,
    /// The process dies there: a panic nothing cleans up after.
    Die,
}

struct Died;

/// The real steps, counted from 1, with a fault at the `at`-th: a write it hits writes half its bytes first.
/// Notes which step renamed the manifest into place.
struct Faulty {
    at: usize,
    fault: Fault,
    count: usize,
    pointer: PathBuf,
    commit: Option<usize>,
}

impl Faulty {
    fn new(at: usize, fault: Fault, root: &Path) -> Self {
        Self { at, fault, count: 0, pointer: root.join("release.smanifest"), commit: None }
    }

    fn hit(&mut self) -> io::Result<()> {
        self.count += 1;
        if self.count != self.at {
            return Ok(());
        }
        match self.fault {
            Fault::Fail => Err(io::Error::other("injected")),
            Fault::Die => panic::resume_unwind(Box::new(Died)),
        }
    }
}

impl ReplaceSteps for Faulty {
    fn write_temp(&mut self, temp: &mut File, bytes: &[u8]) -> io::Result<()> {
        let written = if self.count + 1 == self.at { bytes.len() / 2 } else { bytes.len() };
        OsSteps.write_temp(temp, &bytes[..written])?;
        self.hit()
    }

    fn sync_temp(&mut self, temp: &File) -> io::Result<()> {
        self.hit()?;
        OsSteps.sync_temp(temp)
    }

    fn rename(&mut self, temp: &Path, target: &Path) -> io::Result<()> {
        self.hit()?;
        if target == self.pointer {
            self.commit = Some(self.count);
        }
        OsSteps.rename(temp, target)
    }

    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        self.hit()?;
        OsSteps.sync_dir(dir)
    }
}

impl InstallSteps for Faulty {
    fn remove(&mut self, path: &Path) -> io::Result<()> {
        self.hit()?;
        fs::remove_file(path)
    }
}

/// Copies the directory `from` to `to`, made afresh.
fn copy_dir(from: &Path, to: &Path) {
    let _ = fs::remove_dir_all(to);
    for name in listing(from) {
        let target = to.join(&name);
        fs::create_dir_all(target.parent().expect("in a directory")).expect("makes a directory");
        fs::copy(from.join(&name), target).expect("copies");
    }
    fs::create_dir_all(to).expect("makes the directory");
}

/// Installs `next` over an app that shipped `releases[0]` as release 1 and installed each after it as the
/// next version, failing or dying at every step of the install in turn. After each, the app must start on
/// exactly one whole release: the current one until the manifest's rename, the new one from it on. Another
/// install must then finish the job, and a confirmed launch leave only the new release. Returns how many
/// steps an install took.
fn fault_every_step(name: &str, releases: &[&Content], next: &Content) -> usize {
    let dir = TestDir::new(name);
    let template = App::new(&dir, releases[0]);
    // The app as each run finds it: the earlier patches installed and confirmed, and `next` staged.
    for (index, pair) in releases.windows(2).enumerate() {
        stage(&template.staging, pair[0], pair[1], index as u64 + 2);
        template.install().expect("installs an earlier release");
    }
    template.open().confirm_launch().expect("confirms");
    let current = releases[releases.len() - 1];
    let version = releases.len() as u64 + 1;
    stage(&template.staging, current, next, version);

    let run = |at: usize, fault: Fault| {
        let app = App {
            shipped: template.shipped.clone(),
            root: dir.join(&format!("root-{at}-{fault:?}")),
            staging: dir.join(&format!("staging-{at}-{fault:?}")),
        };
        copy_dir(&template.root, &app.root);
        copy_dir(&template.staging, &app.staging);
        let mut steps = Faulty::new(at, fault, &app.root);
        let mut installation = app.open();
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            installation.install_with(&app.staging, NOW, APP_BUILD, &mut steps)
        }));
        (app, installation, result, steps)
    };

    let (clean, _, result, steps) = run(0, Fault::Fail);
    assert_eq!(
        result.ok().map(|result| result.expect("installs")),
        Some(InstallOutcome::Installed { from: version - 1, to: version })
    );
    let (total, commit) = (steps.count, steps.commit.expect("the manifest was renamed into place"));
    assert_starts_on(&clean.open(), &clean.shipped, next, version);
    let _ = fs::remove_dir_all(&clean.root);

    for at in 1..=total {
        for fault in [Fault::Fail, Fault::Die] {
            let (app, installation, result, _) = run(at, fault);
            let switched = at > commit;
            let (expected, expected_version) =
                if switched { (next, version) } else { (current, version - 1) };
            let context = format!("{fault:?} at step {at} of {total} (the manifest's rename is {commit})");
            match result {
                Err(payload) => assert!(payload.is::<Died>(), "{context}: another panic"),
                // A step that fails before the rename fails the install; one after it only leaves the staging.
                Ok(Err(error)) => {
                    assert!(matches!(error, InstallError::Io { .. }), "{context}: {error}");
                    assert!(!switched || at == commit + 1, "{context}: failed after the switch");
                    assert_eq!(installation.version(), expected_version, "{context}: what the app holds");
                }
                Ok(Ok(_)) => assert!(switched, "{context}: an install that failed said it didn't"),
            }
            let reopened = app.open();
            assert_starts_on(&reopened, &app.shipped, expected, expected_version);

            // The next try finishes it, from what was left.
            let mut resumed = app.open();
            let outcome = resumed.install(&app.staging, NOW, APP_BUILD);
            let outcome = outcome.unwrap_or_else(|error| panic!("{context}: the next install: {error}"));
            assert_eq!(
                matches!(outcome, InstallOutcome::UpToDate { .. }),
                switched,
                "{context}: {outcome:?}"
            );
            assert_starts_on(&resumed, &app.shipped, next, version);
            // The next launch, on the new release, says it went well.
            let mut relaunched = app.open();
            relaunched.confirm_launch().expect("confirms");
            assert_eq!(listing(&app.root), settled(&relaunched), "{context}: what a confirmed launch leaves");
            assert!(listing(&app.staging).iter().all(|name| name == "lock"), "{context}: the staging stays");
            let _ = fs::remove_dir_all(&app.root);
            let _ = fs::remove_dir_all(&app.staging);
        }
    }
    total
}

#[test]
fn a_fault_at_every_step_of_an_install_over_the_shipped_release_leaves_one_whole_release() {
    let steps = fault_every_step("install_faults_shipped", &[&first_release()], &second_release());
    println!("installing release 2 over the shipped release: {steps} steps, each failed and died at");
    assert!(steps > 20, "{steps} steps: blocks, flushes, renames, the manifest and the staging");
}

#[test]
fn a_fault_at_every_step_of_an_install_over_a_patch_leaves_one_whole_release() {
    let (first, second) = (first_release(), second_release());
    let steps = fault_every_step("install_faults_patched", &[&first, &second], &third_release());
    println!("installing release 3 over release 2, a patch: {steps} steps, each failed and died at");
    assert!(steps > 20);
}

/// What an install won't do: and each time, the app still starts on its current release, whole.
#[test]
fn an_install_refuses_what_it_cant_trust_or_finish() {
    let dir = TestDir::new("install_refuses");
    let (first, second) = (first_release(), second_release());
    let app = App::new(&dir, &first);
    let refused = |error: InstallError| {
        assert_starts_on(&app.open(), &app.shipped, &first, 1);
        error
    };

    assert!(matches!(refused(app.install().unwrap_err()), InstallError::NothingStaged));

    // Staged, then expired before it was installed.
    stage(&app.staging, &first, &second, 2);
    let error = app.open().install(&app.staging, EXPIRES, APP_BUILD).unwrap_err();
    assert!(matches!(refused(error), InstallError::Refused(ManifestError::Expired { .. })));
    // Signed by a key the shipped release doesn't name.
    fs::write(app.staging.join("release.smanifest"), second.signed(2, EXPIRES, UNTRUSTED)).unwrap();
    assert!(matches!(refused(app.install().unwrap_err()), InstallError::Refused(ManifestError::Untrusted)));
    // Changing what the simulation reads.
    let mut flow = second.clone();
    flow.files.insert("documents/game.flow.ron".to_owned(), (b"(start: fight)".to_vec(), false));
    stage(&app.staging, &first, &flow, 2);
    let error = refused(app.install().unwrap_err());
    assert_eq!(
        error.to_string(),
        "release 2 changes documents/game.flow.ron, which the simulation reads, so only an app build may change it"
    );

    // A staged block lost, or torn: nothing switches, and what's half made goes at the next confirmed launch.
    let _ = fs::remove_dir_all(&app.staging);
    stage(&app.staging, &first, &second, 2);
    let blocks = app.staging.join("blocks");
    let block = fs::read_dir(&blocks).unwrap().next().unwrap().unwrap().path();
    fs::write(&block, b"torn").unwrap();
    assert!(matches!(refused(app.install().unwrap_err()), InstallError::Unavailable { .. }));
    assert!(!block.exists(), "a staged block that fails its hash goes, so a download fetches it again");
    let mut installation = app.open();
    installation.confirm_launch().expect("confirms");
    assert_eq!(listing(&app.root), ["lock"]);

    // A download running holds the staging.
    stage(&app.staging, &first, &second, 2);
    let held = sapphire_patch::Staging::open(&app.staging).expect("opens");
    assert!(matches!(refused(app.install().unwrap_err()), InstallError::Staging(_)));
    drop(held);
    assert_eq!(app.install().expect("installs"), InstallOutcome::Installed { from: 1, to: 2 });
}

/// The installed patch is the current release only while it's usable; else the app starts on what it
/// shipped, and the next confirmed launch removes the patch.
#[test]
fn an_app_starts_on_what_it_shipped_when_the_patch_is_stale_or_broken() {
    let dir = TestDir::new("install_stale");
    let (first, second, third) = (first_release(), second_release(), third_release());
    let app = App::new(&dir, &first);
    stage(&app.staging, &first, &second, 2);
    app.install().expect("installs");
    assert_starts_on(&app.open(), &app.shipped, &second, 2);

    // A patched pack lost: the patch isn't whole, so the app starts on what it shipped.
    let Some(Location::Patched(shared)) = app.open().pack("shared") else { panic!("a patched pack") };
    let kept = fs::read(&shared).unwrap();
    fs::remove_file(&shared).unwrap();
    assert_starts_on(&app.open(), &app.shipped, &first, 1);
    fs::write(&shared, &kept[..kept.len() - 1]).unwrap();
    assert_starts_on(&app.open(), &app.shipped, &first, 1);
    fs::write(&shared, &kept).unwrap();
    assert_starts_on(&app.open(), &app.shipped, &second, 2);

    // An app update that ships release 3 starts on it, past the patch, and its first good launch removes it.
    let _ = fs::remove_dir_all(&app.shipped);
    third.write_install(&app.shipped, 3);
    // The shipped manifest is what the app build carries, signed as release 3.
    let mut updated = app.open();
    assert_starts_on(&updated, &app.shipped, &third, 3);
    assert!(!updated.is_patched());
    updated.confirm_launch().expect("confirms");
    assert_eq!(listing(&app.root), ["lock"]);

    // A patch whose simulation documents aren't what this app build shipped isn't started on either.
    let _ = fs::remove_dir_all(&app.shipped);
    first.write_install(&app.shipped, 1);
    stage(&app.staging, &first, &second, 2);
    app.install().expect("installs");
    let mut rebuilt = first.clone();
    rebuilt.files.insert("documents/game.flow.ron".to_owned(), (b"(start: fight)".to_vec(), false));
    rebuilt.write_install(&app.shipped, 1);
    let installation = app.open();
    assert_starts_on(&installation, &app.shipped, &rebuilt, 1);
}
