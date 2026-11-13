//! Exit test 6's flashes in the arena (M4 A3, held by a test since M4 Z3c): the scripted fight, every frame of
//! it at 60 fps with the fight's cues drawing their screen effects, passes WCAG 2.3.1's general and red flash
//! tests as `sapphire_testkit::flashes` reads them, with flashes reduced and, since A3, at the default too; and
//! since M4 A7 so do the player's ultimate with its cinematic (`arena-ultimate`) and the stress fight
//! (`arena-skirmish`, eight fighters), each with no red transition; and since M5 L9 the boss fight
//! (`arena-boss`), whose captain's enraged phase fires the effects pack's glitch and CRT (`phase_shift`), as the
//! ultimate fires its halftone and flash. The ward's fights, as the game plays them, are `game_app`'s
//! `flashes_in_the_ward`.
//!
//! The run draws, so it needs a GPU and cooked content and is `#[ignore]`d: `cargo xtask content`, then
//! `cargo test -p sapphire_sandbox --test flashes -- --ignored --nocapture`, a test for each fight and setting
//! (the scripted fight about 70 s a setting on Metal here; CI's nightly run takes them on lavapipe). It
//! captures at 480 × 270, as A3 measured: the analyser sizes its field by the height, so a smaller capture
//! judges the same frame.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sapphire_testkit::flashes::{FlashAnalyser, FlashReport, Verdict};
use sapphire_testkit::golden::Image;

/// A fight's replay, and its length in ticks (`tests/replays/checksums.*.txt`): a frame each.
struct Fight {
    name: &'static str,
    replay: &'static str,
    ticks: u32,
}

const SCRIPTED: Fight =
    Fight { name: "the scripted fight", replay: "tests/replays/arena-scripted.sreplay", ticks: 1700 };
/// The ultimate pressed at tick 60, its cinematic holding the warrior while the chop lands, then the fight on.
const ULTIMATE: Fight =
    Fight { name: "the ultimate", replay: "tests/replays/arena-ultimate.sreplay", ticks: 450 };
const STRESS: Fight =
    Fight { name: "the stress fight", replay: "tests/replays/arena-skirmish.sreplay", ticks: 2400 };
/// The captain taken under half its health, crossing into its named phase, `enraged` (M5 L9).
const BOSS: Fight = Fight { name: "the boss fight", replay: "tests/replays/arena-boss.sreplay", ticks: 1700 };
/// The capture's rate: a frame a step.
const RATE: u32 = 60;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("apps/<app>").to_owned()
}

/// A directory of its own under the system's temporary one, gone when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_flashes_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory");
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Every frame of `fight` at `flashes` (`normal` or `reduced`), as the analyser reads them.
fn the_fight_at(fight: &Fight, flashes: &str) -> FlashReport {
    // By the replay, since two fights can be as long (the scripted fight and the boss fight).
    let stem = Path::new(fight.replay).file_stem().and_then(|stem| stem.to_str()).expect("a replay's name");
    let temp = Temp::new(&format!("{stem}_{flashes}"));
    let frames = temp.0.join("frames");
    fs::create_dir_all(&frames).expect("the frames' directory");
    let capture = temp.0.join("last.png");
    let workspace = workspace();
    let ticks = fight.ticks.to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_sapphire_sandbox"))
        .current_dir(&workspace)
        .args([
            "--capture",
            capture.to_str().expect("UTF-8"),
            "--size",
            "480x270",
            "--capture-ticks",
            &ticks,
            "--capture-frames",
            &ticks,
            "--capture-step",
            "1",
            "--arena",
            "--replay",
            fight.replay,
            "--cues",
            "--hud",
            "live",
            "--time-of-day",
            "10",
            "--tier",
            "desktop-high",
            "--blade",
            "on",
            "--effect-scale",
            &format!("{flashes}:1"),
            "--no-hot-reload",
            "--live-link",
            "off",
        ])
        .env("SAPPHIRE_LOG", "info")
        .env_remove("RUST_LOG")
        .env("SAPPHIRE_CONTENT_DIR", workspace.join("target").join("content").join("desktop"))
        .env("SAPPHIRE_CAPTURE_SEQUENCE", &frames)
        .output()
        .expect("the sandbox starts");
    let log =
        format!("{}{}", String::from_utf8_lossy(&output.stderr), String::from_utf8_lossy(&output.stdout));
    assert!(output.status.success(), "it exited with {}:\n{log}", output.status);
    let mut paths: Vec<PathBuf> = fs::read_dir(&frames)
        .expect("the frames")
        .map(|entry| entry.expect("a frame").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    paths.sort();
    let first = Image::read_png(paths.first().expect("a frame")).expect("a PNG");
    let mut analyser = FlashAnalyser::new(first.width, first.height, RATE).expect("a size and rate it reads");
    for path in &paths {
        analyser.push(&Image::read_png(path).expect("a PNG")).expect("the sequence's size");
    }
    analyser.report(Some(fight.ticks)).expect("every frame of the fight")
}

/// Holds `fight` at `flashes` to both tests, and to no red transition at all, printing its scores and the
/// frame its worst second ends on.
#[allow(clippy::print_stdout, reason = "the scores are exit test 6's numbers, read with --nocapture")]
fn passes_at(fight: &Fight, flashes: &str) {
    let report = the_fight_at(fight, flashes);
    let at = |verdict: &Verdict| {
        verdict.rate_at.map_or(String::new(), |(frame, _)| format!(" ending frame {frame}"))
    };
    println!(
        "{flashes}: {}: {} frames, general {} a second{} (area {:.3}), red {} a second{} (area {:.3})",
        fight.name,
        report.frames,
        report.general.rate,
        at(&report.general),
        report.general.area,
        report.red.rate,
        at(&report.red),
        report.red.area
    );
    assert!(report.passes() && report.red.rate == 0, "{} at {flashes}: {report:?}", fight.name);
}

// A test for each fight and setting, so CI's nightly run can judge them on runners at once.

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_scripted_fight_passes_wcag_2_3_1_with_flashes_reduced() {
    passes_at(&SCRIPTED, "reduced");
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_scripted_fight_passes_wcag_2_3_1_by_default() {
    passes_at(&SCRIPTED, "normal");
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_ultimate_passes_wcag_2_3_1_with_flashes_reduced() {
    passes_at(&ULTIMATE, "reduced");
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_ultimate_passes_wcag_2_3_1_by_default() {
    passes_at(&ULTIMATE, "normal");
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_stress_fight_passes_wcag_2_3_1_with_flashes_reduced() {
    passes_at(&STRESS, "reduced");
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_stress_fight_passes_wcag_2_3_1_by_default() {
    passes_at(&STRESS, "normal");
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_boss_fight_passes_wcag_2_3_1_with_flashes_reduced() {
    passes_at(&BOSS, "reduced");
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
fn the_boss_fight_passes_wcag_2_3_1_by_default() {
    passes_at(&BOSS, "normal");
}
