//! `cargo xtask release`: signs a cook's release and lays it out for a static host (M4 P1, m4-plan §3
//! "What's signed").
//!
//! Every cook writes an unsigned `release.smanifest` beside its packs. [`publish`] runs the pipeline's
//! `release` command on a cook, which signs that manifest's body with the keys in `SAPPHIRE_RELEASE_KEY` and
//! lays the release out under the output directory: `packs/` and `files/` named by their hashes,
//! `manifests/<version>.smanifest`, and `release.smanifest`, written last (`sapphire_pipeline::release` has the
//! layout). The key goes from this process's environment to the pipeline's and nowhere else: xtask never
//! reads it, and nothing prints it. The signed `release.smanifest` is also what an app's own content carries
//! in place of its cook's unsigned one, so `package` copies it over the content's after this.
//!
//! The command itself is a thin entry, cooking and then publishing; `package` calls [`publish`] on the cook it
//! made.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// The manifest's file name: `sapphire_assets::manifest::FILE_NAME`, which xtask doesn't link.
const MANIFEST: &str = "release.smanifest";

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
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_runs_the_pipelines_command_with_its_options() {
        let mut release =
            Release { version: 12, expires: "2027-06-01".to_owned(), min_build: 1300, next_keys: None };
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
        assert!(!line(&release).iter().any(|argument| argument.contains("KEY")), "no key on a command line");
    }
}
