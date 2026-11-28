//! Every clip `content/` ships, as the desktop cook wrote it (M6 A1): each at the clip's current
//! version, each with a skeleton measured, and any with contacts carrying both feet's, left then right;
//! the clips on a skeleton without feet are listed, and counted.
//! Reads `target/content/loose/desktop`, so it says nothing until `cargo xtask content` has cooked it, and
//! skips a cook made at an older clip version. **CI proves it** (M6 A1's backfill): the Linux golden
//! inputs' job runs it right after its cook with `SAPPHIRE_REQUIRE_COOK=1`, which makes a missing or older
//! cook a failure.
//!
//! `cargo test -p sapphire_pipeline --test shipped_clips -- --nocapture` prints every clip's contacts and
//! report: the table the clip inspector and the content checks read.

#![allow(clippy::print_stdout, reason = "the report table, with --nocapture")]

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::clip::{ArchivedMarker, Clip};
use sapphire_assets::rig::HumanBone;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, schema_version};

fn cooked() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/content/loose/desktop")
}

fn clips(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> =
        entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            clips(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "sclip") {
            found.push(path);
        }
    }
}

fn aligned(path: &Path) -> AlignedVec {
    let mut bytes = AlignedVec::new();
    bytes.extend_from_slice(&fs::read(path).unwrap());
    bytes
}

/// Says why the cook can't be read: a failure where a job promised one (`SAPPHIRE_REQUIRE_COOK`), and
/// otherwise a skip.
fn skipped(why: &str) {
    if std::env::var_os("SAPPHIRE_REQUIRE_COOK").is_some() {
        panic!("SAPPHIRE_REQUIRE_COOK is set, and {why}");
    }
    println!("skipped: {why}");
}

#[test]
fn every_shipped_clip_cooks_at_this_version_with_its_contacts() {
    let mut paths = Vec::new();
    clips(&cooked(), &mut paths);
    if paths.is_empty() {
        skipped("there are no cooked clips under target/content; run `cargo xtask content` first");
        return;
    }
    let read: Vec<(PathBuf, AlignedVec)> = paths
        .into_iter()
        .map(|path| {
            let bytes = aligned(&path);
            (path, bytes)
        })
        .collect();
    if read.iter().any(|(_, bytes)| schema_version::<Clip>(bytes).ok() != Some(Clip::VERSION)) {
        skipped("target/content holds clips of an older version; run `cargo xtask content` again");
        return;
    }

    let root = cooked();
    let mut without_feet = Vec::new();
    println!(
        "{:<64} {:>5} {:>4} {:>4} {:>8} {:>8} {:>7} {:>7}",
        "clip", "ticks", "L", "R", "slide L", "slide R", "seam", "radius"
    );
    for (path, bytes) in &read {
        let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        let clip = Clip::read(bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        if clip.skeleton.is_none() {
            assert!(clip.contacts.is_empty() && clip.report.is_none(), "{name}");
            continue;
        }
        let report = clip.report.as_ref().unwrap_or_else(|| panic!("{name} wasn't measured"));
        let feet: Vec<HumanBone> = clip.contacts.iter().map(|track| track.foot).collect();
        if feet.is_empty() {
            without_feet.push(name.clone());
        } else {
            assert_eq!(feet, [HumanBone::LeftFoot, HumanBone::RightFoot], "{name}");
        }
        let side = |foot| {
            clip.contact(foot).map_or(("-".to_owned(), "-".to_owned()), |track| {
                let downs =
                    track.markers.iter().filter(|marker| marker.marker == ArchivedMarker::Down).count();
                let planted = if track.planted { "p" } else { "" };
                (format!("{planted}{downs}"), format!("{:.4}", track.slide.to_native()))
            })
        };
        let ((left, left_slide), (right, right_slide)) =
            (side(HumanBone::LeftFoot), side(HumanBone::RightFoot));
        println!(
            "{name:<64} {:>5} {left:>4} {right:>4} {left_slide:>8} {right_slide:>8} {:>7.4} {:>7.3}",
            clip.ticks.to_native(),
            report.seam.to_native(),
            report.radius.to_native()
        );
    }
    // A rig with no humanoid map (a prop's, a creature's) has no feet to find: listed, not refused, and
    // counted, so a humanoid whose feet stop being found can't hide among them.
    println!("{} clips on a skeleton without feet in its map: {without_feet:#?}", without_feet.len());
    assert_eq!(
        without_feet.len(),
        WITHOUT_FEET,
        "clips on a skeleton without feet: {without_feet:#?}; a new one changes WITHOUT_FEET on purpose"
    );
}

/// How many shipped clips play on a skeleton whose map has no feet: none yet, since every shipped rig is
/// a humanoid's.
const WITHOUT_FEET: usize = 0;
