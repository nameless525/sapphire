//! `cargo xtask release`: signs a cook's release and lays it out for a static host (M4 P1, m4-plan §3
//! "What's signed").
//!
//! Every cook writes an unsigned `release.smanifest` beside its packs. [`publish`] runs the pipeline's
//! `release` command on a cook, which signs that manifest's body with the keys in `SAPPHIRE_RELEASE_KEY` and
//! lays the release out under the output directory: `packs/` and `files/` named by their hashes,
//! `manifests/<version>.smanifest`, and `release.smanifest`, written last (`sapphire_pipeline::release` has the
//! layout). The key goes from this process's environment to the pipeline's and nowhere else: xtask never
//! reads it, and nothing prints it. The signed `release.smanifest` is also what an app's own content carries
//! in place of its cook's unsigned one, so `package` ships a copy of its cook with it in that place.
//!
//! `--patch-from <manifest>` makes the release a patch of an earlier one, whose signed manifest it names: the
//! pipeline refuses, before anything is written, a patch that changes a file the simulation reads (naming it)
//! or one an install of the earlier release wouldn't take (M4 P3, m4-plan §3 "What a patch may carry"). It
//! prints what an install of the earlier release fetches, by asset kind with each one's share of the cook, and
//! warns over `sapphire_pipeline::release::WARN_SHARE` of the cook or when the two releases were cooked on
//! different architectures (M4 S7: `PatchSize` there; each release records its in `manifests/<n>.arch`).
//!
//! The command itself is a thin entry, cooking and then publishing; `package` calls [`publish`] on the cook it
//! made, when it's given a release's options ([`Release::from_args`]).
//!
//! `cargo xtask release-key` ([`key`], M4 X4) is the pipeline's `release-key`: `new <file>` writes a new
//! release key to a file the maintainer names, refusing one that's there, and `public <file>` prints a key
//! file's public half as `platform/patch.toml` takes it. Here too the seed stays in the pipeline.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::bail;

use crate::cli;
use crate::content::{self, Cooked};
use crate::workspace;

/// What a release says beyond its content: the pipeline's `release` options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Higher than the output directory's current release's.
    pub version: u64,
    /// `YYYY-MM-DD`: the day, at midnight UTC, it stops being offered as current.
    pub expires: String,
    /// The oldest app build it runs on.
    pub min_build: u32,
    /// The public keys, in hex and comma-separated, that may sign the next release, if not the signing keys.
    pub next_keys: Option<String>,
    /// The earlier release's signed manifest, when this one is a patch of it: the pipeline refuses a patch
    /// that changes what the simulation reads, naming the file, or that an install of the earlier release
    /// wouldn't take.
    pub patch_from: Option<PathBuf>,
}

impl Release {
    /// The release options on a command line: `--release-version`, `--expires` and `--min-build`, with
    /// `--next-keys` and `--patch-from` if given. `None` if none of them is there; an error if some are and
    /// not all three that a release needs.
    pub fn from_args(args: &mut cli::Args) -> anyhow::Result<Option<Self>> {
        let version = args.value("--release-version")?;
        let expires = args.value("--expires")?;
        let min_build = args.value("--min-build")?;
        let next_keys = args.value("--next-keys")?;
        let patch_from = args.value("--patch-from")?;
        match (version, expires, min_build) {
            (Some(version), Some(expires), Some(min_build)) => {
                Ok(Some(Self { version, expires, min_build, next_keys, patch_from }))
            }
            (None, None, None) if next_keys.is_none() && patch_from.is_none() => Ok(None),
            _ => bail!("a release needs --release-version, --expires and --min-build, all three"),
        }
    }
}

/// Signs `cooked`'s release and lays it out in `out`. Returns the signed manifest, `<out>/release.smanifest`,
/// which an app's content carries too. Fails, with nothing published, unless `SAPPHIRE_RELEASE_KEY` holds the
/// keys.
pub fn publish(cooked: &Cooked, out: &Path, release: &Release) -> anyhow::Result<PathBuf> {
    let pipeline = content::pipeline()?;
    let mut command = Command::new(pipeline);
    command.args(arguments(&cooked.dir, out, release));
    workspace::run(&mut command)?;
    Ok(out.join(MANIFEST))
}

/// [`publish`]es the cook in `packs` signed with `seeds`, in place of `SAPPHIRE_RELEASE_KEY`'s: only test seeds,
/// which anyone may know, for `cargo xtask patch rehearse` (M4 P4).
pub fn publish_with_test_seeds(
    packs: &Path,
    out: &Path,
    release: &Release,
    seeds: &str,
) -> anyhow::Result<PathBuf> {
    let pipeline = content::pipeline()?;
    let mut command = Command::new(pipeline);
    command.args(arguments(packs, out, release)).env(KEY_VAR, seeds);
    workspace::run(&mut command)?;
    Ok(out.join(MANIFEST))
}

/// What `release-key` does with its key file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Writes a new key to a file that mustn't be there, and prints its public half.
    New,
    /// Prints the public half of the key in the file.
    Public,
}

impl std::str::FromStr for KeyAction {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> anyhow::Result<Self> {
        match text {
            "new" => Ok(Self::New),
            "public" => Ok(Self::Public),
            other => bail!("release-key takes `new` or `public`, not `{other}`"),
        }
    }
}

/// Runs the pipeline's `release-key` on `file`: a new key written there, or the public half of the one there,
/// printed as `platform/patch.toml`'s `release-keys` takes it.
pub fn key(action: KeyAction, file: &Path) -> anyhow::Result<()> {
    let pipeline = content::pipeline()?;
    workspace::run(Command::new(pipeline).args(key_arguments(action, file)))
}

/// The pipeline's command line for `release-key`.
fn key_arguments(action: KeyAction, file: &Path) -> [OsString; 3] {
    let action = match action {
        KeyAction::New => "new",
        KeyAction::Public => "public",
    };
    ["release-key".into(), action.into(), file.into()]
}

/// The manifest's file name: `sapphire_assets::manifest::FILE_NAME`, which xtask doesn't link.
pub const MANIFEST: &str = "release.smanifest";

/// The environment variable the pipeline reads the signing keys from, for messages: xtask never reads it.
pub const KEY_VAR: &str = "SAPPHIRE_RELEASE_KEY";

/// The pipeline's command line for a release of `packs` into `out`.
fn arguments(packs: &Path, out: &Path, release: &Release) -> Vec<OsString> {
    let mut arguments: Vec<OsString> =
        vec!["release".into(), "--packs".into(), packs.into(), "--out".into(), out.into()];
    arguments.extend(
        [
            ("--release-version", release.version.to_string()),
            ("--expires", release.expires.clone()),
            ("--min-build", release.min_build.to_string()),
        ]
        .into_iter()
        .chain(release.next_keys.clone().map(|keys| ("--next-keys", keys)))
        .flat_map(|(name, value)| [OsString::from(name), OsString::from(value)]),
    );
    if let Some(patch_from) = &release.patch_from {
        arguments.extend(["--patch-from".into(), patch_from.into()]);
    }
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_key_runs_the_pipelines_command_on_its_file() {
        let line = |action: &str| -> Vec<String> {
            key_arguments(action.parse().unwrap(), Path::new("offline/release.key"))
                .into_iter()
                .map(|argument| argument.into_string().unwrap())
                .collect()
        };
        assert_eq!(line("new"), ["release-key", "new", "offline/release.key"]);
        assert_eq!(line("public"), ["release-key", "public", "offline/release.key"]);
        assert!("print".parse::<KeyAction>().is_err());
    }

    #[test]
    fn a_release_runs_the_pipelines_command_with_its_options() {
        let mut release = Release {
            version: 12,
            expires: "2027-06-01".to_owned(),
            min_build: 1300,
            next_keys: None,
            patch_from: None,
        };
        let line = |release: &Release| -> Vec<String> {
            arguments(Path::new("cook"), Path::new("host"), release)
                .into_iter()
                .map(|argument| argument.into_string().unwrap())
                .collect()
        };
        assert_eq!(
            line(&release),
            [
                "release",
                "--packs",
                "cook",
                "--out",
                "host",
                "--release-version",
                "12",
                "--expires",
                "2027-06-01",
                "--min-build",
                "1300"
            ]
        );
        release.next_keys = Some("ab,cd".to_owned());
        assert_eq!(line(&release)[11..], ["--next-keys", "ab,cd"]);
        release.patch_from = Some("host/manifests/11.smanifest".into());
        assert_eq!(line(&release)[13..], ["--patch-from", "host/manifests/11.smanifest"]);
        assert!(!line(&release).iter().any(|argument| argument.contains("KEY")), "no key on a command line");
    }

    /// A package signs only when it's given a release's options, and then needs all three.
    #[test]
    fn release_options_come_all_three_or_not_at_all() {
        let parse = |line: &str| {
            let mut args = cli::Args::new(line.split_whitespace().map(str::to_owned));
            let release = Release::from_args(&mut args);
            release.and_then(|release| args.finish(false).map(|_| release))
        };
        assert_eq!(parse("").unwrap(), None);
        let release =
            parse("--release-version 4 --expires 2027-06-01 --min-build 1300 --patch-from m/3.smanifest")
                .unwrap()
                .expect("a release");
        assert_eq!(
            release,
            Release {
                version: 4,
                expires: "2027-06-01".to_owned(),
                min_build: 1300,
                next_keys: None,
                patch_from: Some("m/3.smanifest".into()),
            }
        );
        for partial in
            ["--release-version 4", "--expires 2027-06-01 --min-build 1", "--patch-from m/3.smanifest"]
        {
            let error = parse(partial).unwrap_err().to_string();
            assert!(error.contains("all three"), "{partial}: {error}");
        }
    }
}
