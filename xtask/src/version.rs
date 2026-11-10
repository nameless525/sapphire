//! One version and one build number for every platform (m4-plan §3, "Versions").
//!
//! The version is the workspace's (`[workspace.package]` in Cargo.toml). The build number is the count of
//! commits on `main` at the packaged commit: `main` lands only by fast-forward, so the count only grows and
//! nobody raises it by hand. It feeds `CFBundleVersion`, Android's `versionCode`, Windows' VERSIONINFO, the
//! pipeline cache's build ID and the package's own `package.json`, which the patch manifest reads.
//!
//! A number counted anywhere else would be a wrong one that looks right, so [`Build::for_package`] says
//! what it saw:
//!
//! - **A commit `main` has had, in a clean checkout:** a release build, numbered `git rev-list --count HEAD`,
//!   which is what `main` counted when it was at that commit. `main` is the remote's (`origin/main`, what CI
//!   fetches) or the local branch, whichever holds it.
//! - **Off `main`, or with changes not committed:** an error, unless the package is asked for as a dev
//!   build. Then it's numbered the same way, the count this commit would have if `main` fast-forwarded to
//!   it, and marked dev in its build ID (`0.1.0+4123.dev.1a2b3c4`), in `package.json` and on the console.
//! - **A shallow clone, or no git:** always an error, since the count can't be had.
//!
//! Development bundles (`bundle`, `run`) take [`development_number`], the same count with no checks, or 1
//! where there's none to take.

use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, bail, ensure};

/// The highest build number: the greatest `versionCode` Google Play takes, which also fits the `Int`
/// that `platform/android/app/build.gradle.kts` reads it as.
pub const MAX_BUILD: u32 = 2_100_000_000;

/// The environment variables a package's build of the app reads (game_app's build script compiles them
/// in): the number alone, for Windows' VERSIONINFO, and the whole build ID.
pub const BUILD_NUMBER_ENV: &str = "SAPPHIRE_BUILD_NUMBER";
pub const BUILD_ID_ENV: &str = "SAPPHIRE_BUILD_ID";

/// The refs that say what `main` is, in the order they're asked.
const MAIN_REFS: [&str; 2] = ["refs/remotes/origin/main", "refs/heads/main"];

/// A package's build: its number, whether it's a release's, and the commit it was counted at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub number: u32,
    pub kind: Kind,
    /// The full hash of the commit packaged.
    pub commit: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A commit `main` has had, in a clean checkout.
    Release,
    /// Anything else, packaged because a dev build was asked for: why it isn't a release's.
    Dev(String),
}

/// What git says about a checkout, which is all a build number depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    pub commit: String,
    /// Commits reachable from HEAD.
    pub count: u64,
    pub shallow: bool,
    /// The `main` ref that holds HEAD, if one does.
    pub on_main: Option<String>,
    /// What `git status --porcelain` printed: changes not committed, untracked files included, since a
    /// cook reads any file under `content/`.
    pub changes: String,
}

impl Checkout {
    /// Reads `repo`'s checkout through git.
    pub fn read(repo: &Path) -> anyhow::Result<Self> {
        let git = |args: &[&str]| -> anyhow::Result<String> {
            let output = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .stderr(Stdio::null())
                .output()
                .context("failed to run git: packaging counts commits with it")?;
            ensure!(output.status.success(), "`git {}` failed in {}", args.join(" "), repo.display());
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let commit = git(&["rev-parse", "HEAD"]).context("not a git checkout with a commit")?;
        let shallow = git(&["rev-parse", "--is-shallow-repository"])? == "true";
        let count = git(&["rev-list", "--count", "HEAD"])?.parse().context("git counted no number")?;
        let on_main = MAIN_REFS.into_iter().find(|main| {
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["merge-base", "--is-ancestor", "HEAD", main])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        });
        let changes = git(&["status", "--porcelain"])?;
        Ok(Self { commit, count, shallow, on_main: on_main.map(str::to_owned), changes })
    }
}

impl Build {
    /// The build of the workspace's checkout, for a package; `dev` allows one that isn't a release's.
    pub fn for_package(dev: bool) -> anyhow::Result<Self> {
        Self::of(&Checkout::read(&crate::workspace::root())?, dev)
    }

    /// The build `checkout` makes (see the module's docs).
    pub fn of(checkout: &Checkout, dev: bool) -> anyhow::Result<Self> {
        ensure!(
            !checkout.shallow,
            "this is a shallow clone, so it can't count main's commits for the build number: fetch the whole \
             history (`git fetch --unshallow`, or `fetch-depth: 0` on CI)"
        );
        let number = u32::try_from(checkout.count)
            .ok()
            .filter(|number| (1..=MAX_BUILD).contains(number))
            .with_context(|| {
                format!("{} commits is past the highest build number, {MAX_BUILD}", checkout.count)
            })?;
        let why_dev = match (&checkout.on_main, checkout.changes.is_empty()) {
            (None, _) => Some(format!(
                "HEAD ({}) isn't a commit main has had, so main never counted it",
                short(&checkout.commit)
            )),
            (Some(_), false) => {
                Some(format!("the checkout has changes not committed:\n{}", checkout.changes))
            }
            (Some(_), true) => None,
        };
        let kind = match why_dev {
            None => Kind::Release,
            Some(why) if dev => Kind::Dev(why),
            Some(why) => bail!(
                "{why}\nA release's build number is main's count at a commit main has had, in a clean checkout. \
                 --dev packages it anyway, numbered {number} and marked dev"
            ),
        };
        Ok(Self { number, kind, commit: checkout.commit.clone() })
    }

    /// The build ID the app compiles in: the version and number, and for a dev build its commit.
    pub fn id(&self, version: &str) -> String {
        match self.kind {
            Kind::Release => format!("{version}+{}", self.number),
            Kind::Dev(_) => format!("{version}+{}.dev.{}", self.number, short(&self.commit)),
        }
    }

    /// What a package's build of the app reads ([`BUILD_NUMBER_ENV`], [`BUILD_ID_ENV`]).
    pub fn env(&self, version: &str) -> [(&'static str, String); 2] {
        [(BUILD_NUMBER_ENV, self.number.to_string()), (BUILD_ID_ENV, self.id(version))]
    }
}

impl fmt::Display for Build {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            Kind::Release => write!(f, "build {} (main at {})", self.number, short(&self.commit)),
            Kind::Dev(_) => {
                write!(f, "DEV build {} ({}, not a commit of main's)", self.number, short(&self.commit))
            }
        }
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

/// A development bundle's build number: the checkout's count, as a package's, but with no checks, or 1
/// where git can't give one (a shallow clone, or no git).
pub fn development_number() -> u32 {
    Checkout::read(&crate::workspace::root())
        .ok()
        .filter(|checkout| !checkout.shallow)
        .and_then(|checkout| u32::try_from(checkout.count).ok())
        .filter(|number| (1..=MAX_BUILD).contains(number))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn checkout(on_main: bool, changes: &str) -> Checkout {
        Checkout {
            commit: "1a2b3c4d5e6f".into(),
            count: 4123,
            shallow: false,
            on_main: on_main.then(|| "refs/remotes/origin/main".into()),
            changes: changes.into(),
        }
    }

    #[test]
    fn a_commit_of_mains_is_a_release() {
        for dev in [false, true] {
            let build = Build::of(&checkout(true, ""), dev).unwrap();
            assert_eq!(build, Build { number: 4123, kind: Kind::Release, commit: "1a2b3c4d5e6f".into() });
            assert_eq!(build.id("0.4.0"), "0.4.0+4123");
            assert_eq!(
                build.env("0.4.0"),
                [(BUILD_NUMBER_ENV, "4123".into()), (BUILD_ID_ENV, "0.4.0+4123".into())]
            );
        }
    }

    /// Off main, or with changes, the number isn't a release's: refused, or marked dev wherever it goes.
    #[test]
    fn anything_else_is_refused_or_marked_dev() {
        for (on_main, changes, why) in
            [(false, "", "isn't a commit main has had"), (true, " M a.rs", "not committed")]
        {
            let error = Build::of(&checkout(on_main, changes), false).unwrap_err().to_string();
            assert!(error.contains(why) && error.contains("--dev"), "{error}");
            let build = Build::of(&checkout(on_main, changes), true).unwrap();
            assert_eq!(build.number, 4123);
            assert!(matches!(&build.kind, Kind::Dev(reason) if reason.contains(why)));
            assert_eq!(build.id("0.4.0"), "0.4.0+4123.dev.1a2b3c4");
            assert!(build.to_string().starts_with("DEV build 4123"));
        }
    }

    #[test]
    fn a_shallow_clone_or_a_count_past_the_limit_is_always_refused() {
        let shallow = Checkout { shallow: true, ..checkout(true, "") };
        for dev in [false, true] {
            assert!(Build::of(&shallow, dev).unwrap_err().to_string().contains("shallow clone"));
        }
        let long = Checkout { count: u64::from(MAX_BUILD) + 1, ..checkout(true, "") };
        assert!(Build::of(&long, true).is_err());
        let most = Checkout { count: u64::from(MAX_BUILD), ..checkout(true, "") };
        assert_eq!(Build::of(&most, false).unwrap().number, MAX_BUILD);
    }

    /// A repository of a test's own, removed afterwards.
    struct Repo(PathBuf);

    impl Repo {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("xtask-version-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let repo = Self(dir);
            repo.git(&["init", "-q", "-b", "main"]);
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        fn commit(&self, name: &str) {
            fs::write(self.0.join(name), name).unwrap();
            self.git(&["add", name]);
            self.git(&["commit", "-q", "--no-verify", "-m", name]);
        }

        fn build(&self, dev: bool) -> anyhow::Result<Build> {
            Build::of(&Checkout::read(&self.0)?, dev)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The numbers from a real git, through each case the module's docs name.
    #[test]
    fn counts_mains_commits_with_git() {
        let repo = Repo::new();
        for name in ["a", "b", "c"] {
            repo.commit(name);
        }
        assert_eq!(repo.build(false).unwrap().number, 3);
        assert_eq!(repo.build(false).unwrap().kind, Kind::Release);
        assert_eq!(repo.build(false).unwrap().commit, repo.git(&["rev-parse", "HEAD"]));

        // An older commit of main's keeps the number main gave it.
        repo.git(&["checkout", "-q", "--detach", "HEAD~1"]);
        assert_eq!(repo.build(false).unwrap().number, 2);
        repo.git(&["checkout", "-q", "main"]);

        // Untracked files count as changes: a cook reads them.
        fs::write(repo.0.join("new.txt"), "").unwrap();
        assert!(repo.build(false).is_err());
        assert!(matches!(repo.build(true).unwrap().kind, Kind::Dev(_)));
        fs::remove_file(repo.0.join("new.txt")).unwrap();

        // A branch past main is a dev build, numbered as it would be once main fast-forwards to it.
        repo.git(&["checkout", "-q", "-b", "feature"]);
        repo.commit("d");
        assert!(repo.build(false).unwrap_err().to_string().contains("isn't a commit main has had"));
        assert_eq!(repo.build(true).unwrap().number, 4);

        // The remote's main counts too, as on CI, where there's no local branch.
        let head = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["update-ref", "refs/remotes/origin/main", &head]);
        assert_eq!(repo.build(false).unwrap().kind, Kind::Release);

        // A shallow clone can't count.
        fs::write(repo.0.join(".git/shallow"), format!("{head}\n")).unwrap();
        assert!(repo.build(true).unwrap_err().to_string().contains("shallow clone"));
    }

    #[test]
    fn no_git_checkout_is_an_error() {
        let dir = std::env::temp_dir().join(format!("xtask-no-git-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert!(Checkout::read(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
