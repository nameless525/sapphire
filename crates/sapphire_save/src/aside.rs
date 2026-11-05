//! Keeping a damaged file rather than overwriting it, so a player or a bug report can still have
//! what it held.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::LOG_TARGET;

/// How many damaged copies of one file stay on disk, the newest. A file that keeps going bad must
/// not fill a phone.
pub const KEPT_ASIDE: usize = 8;

/// Copies `file` to `<file>.bad.<n>`, numbered past any earlier copy, and logs it. It's a copy so
/// that a process dying right after leaves the damaged file where the next load sees it again,
/// never no file at all. A failure is logged and returned as `None`: a player can't be stuck on a
/// file that can't even be copied.
pub(crate) fn keep_aside(file: &Path, why: &str) -> Option<PathBuf> {
    match copy_aside(file) {
        Ok(aside) => {
            tracing::warn!(
                target: LOG_TARGET,
                file = %file.display(),
                aside = %aside.display(),
                why,
                "kept a damaged file aside"
            );
            Some(aside)
        }
        Err(error) => {
            tracing::error!(
                target: LOG_TARGET,
                file = %file.display(),
                %error,
                why,
                "couldn't keep a damaged file aside; it will be replaced"
            );
            None
        }
    }
}

fn copy_aside(file: &Path) -> io::Result<PathBuf> {
    let dir = file.parent().ok_or_else(|| io::Error::other("the file has no directory"))?;
    let name = file.file_name().and_then(OsStr::to_str).ok_or_else(|| io::Error::other("not UTF-8"))?;
    let prefix = format!("{name}.bad.");
    let mut numbers: Vec<u64> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.strip_prefix(&prefix)?.parse().ok())
        .collect();
    numbers.sort_unstable();
    let number = numbers
        .last()
        .map_or(Some(1), |last| last.checked_add(1))
        .ok_or_else(|| io::Error::other("damaged-file numbering is exhausted"))?;
    let aside = dir.join(format!("{prefix}{number}"));
    fs::copy(file, &aside)?;
    // This new one and the newest before it make up the kept.
    let oldest_kept = (numbers.len() + 1).saturating_sub(KEPT_ASIDE);
    for old in &numbers[..oldest_kept] {
        if let Err(error) = fs::remove_file(dir.join(format!("{prefix}{old}"))) {
            tracing::warn!(target: LOG_TARGET, file = %file.display(), old, %error, "couldn't remove an old aside");
        }
    }
    Ok(aside)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn copies_are_numbered_and_only_the_newest_stay() {
        let dir = TestDir::new("aside");
        let file = dir.path().join("settings.ron");
        for round in 1..=KEPT_ASIDE + 3 {
            fs::write(&file, round.to_string()).expect("write");
            let aside = keep_aside(&file, "test").expect("keep");
            assert_eq!(aside, dir.path().join(format!("settings.ron.bad.{round}")));
            assert_eq!(fs::read_to_string(&file).expect("the original stays"), round.to_string());
        }
        let mut kept: Vec<String> = fs::read_dir(dir.path())
            .expect("list")
            .map(|entry| entry.expect("entry").file_name().into_string().expect("UTF-8"))
            .filter(|name| name.contains(".bad."))
            .collect();
        kept.sort_by_key(|name| name.rsplit('.').next().and_then(|n| n.parse::<u32>().ok()));
        assert_eq!(kept.len(), KEPT_ASIDE);
        assert_eq!(kept.first().map(String::as_str), Some("settings.ron.bad.4"));
        assert_eq!(fs::read_to_string(dir.path().join(&kept[KEPT_ASIDE - 1])).expect("read"), "11");
    }

    #[test]
    fn another_file_with_a_similar_name_is_left_alone() {
        let dir = TestDir::new("aside_similar");
        let file = dir.path().join("a.ron");
        fs::write(&file, "x").expect("write");
        fs::write(dir.path().join("a.ron.bad.notes"), "y").expect("write");
        fs::write(dir.path().join("aa.ron.bad.9"), "z").expect("write");
        assert_eq!(keep_aside(&file, "test"), Some(dir.path().join("a.ron.bad.1")));
    }

    #[test]
    fn a_file_that_cant_be_copied_is_only_logged() {
        let dir = TestDir::new("aside_missing");
        assert_eq!(keep_aside(&dir.path().join("gone"), "test"), None);
    }
}

#[cfg(test)]
mod exhaustion_tests {
    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn exhausted_numbering_leaves_all_existing_copies_alone() {
        let dir = TestDir::new("aside_exhausted");
        let file = dir.path().join("settings.ron");
        let maximum = dir.path().join(format!("settings.ron.bad.{}", u64::MAX));
        let first = dir.path().join("settings.ron.bad.0");
        for (path, bytes) in [(&file, "damaged"), (&maximum, "retained max"), (&first, "retained zero")] {
            fs::write(path, bytes).expect("seed");
        }
        assert_eq!(keep_aside(&file, "test"), None);
        assert_eq!(fs::read_to_string(file).expect("source"), "damaged");
        assert_eq!(fs::read_to_string(maximum).expect("max"), "retained max");
        assert_eq!(fs::read_to_string(first).expect("zero"), "retained zero");
    }
}
