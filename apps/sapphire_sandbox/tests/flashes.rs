//! Exit test 6's flashes (M4 A3, held by a test since M4 Z3c): the scripted fight, every frame of it at
//! 60 fps with the fight's cues drawing their screen effects, passes WCAG 2.3.1's general and red flash tests
//! as `sapphire_testkit::flashes` reads them, with flashes reduced and, since A3, at the default too.
//!
//! The run draws, so it needs a GPU and cooked content and is `#[ignore]`d: `cargo xtask content`, then
//! `cargo test -p sapphire_sandbox --test flashes -- --ignored`. It captures at 480 × 270, as A3 measured:
//! the analyser sizes its field by the height, so a smaller capture judges the same frame.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sapphire_testkit::flashes::{FlashAnalyser, FlashReport};
use sapphire_testkit::golden::Image;

/// The scripted fight's replay, and its length in ticks (`tests/replays/checksums.*.txt`).
const REPLAY: &str = "tests/replays/arena-scripted.sreplay";
const TICKS: &str = "1700";
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

/// Every frame of the scripted fight at `flashes` (`normal` or `reduced`), as the analyser reads them.
fn the_fight_at(flashes: &str) -> FlashReport {
    let temp = Temp::new(flashes);
    let frames = temp.0.join("frames");
    fs::create_dir_all(&frames).expect("the frames' directory");
    let capture = temp.0.join("last.png");
    let workspace = workspace();
    let output = Command::new(env!("CARGO_BIN_EXE_sapphire_sandbox"))
        .current_dir(&workspace)
        .args([
            "--capture",
            capture.to_str().expect("UTF-8"),
            "--size",
            "480x270",
            "--capture-ticks",
            TICKS,
            "--capture-frames",
            TICKS,
            "--capture-step",
            "1",
            "--arena",
            "--replay",
            REPLAY,
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
    assert!(paths.len() >= 1600, "{} frames of the fight's {TICKS}", paths.len());
    let first = Image::read_png(&paths[0]).expect("a PNG");
    let mut analyser = FlashAnalyser::new(first.width, first.height, RATE).expect("a size and rate it reads");
    for path in &paths {
        analyser.push(&Image::read_png(path).expect("a PNG")).expect("the sequence's size");
    }
    analyser.report()
}

#[test]
#[ignore = "needs a GPU and cooked content (`cargo xtask content`)"]
#[allow(clippy::print_stdout, reason = "the scores are exit test 6's numbers, read with --nocapture")]
fn the_scripted_fight_passes_wcag_2_3_1_reduced_and_by_default() {
    let mut wrong = Vec::new();
    for flashes in ["reduced", "normal"] {
        let report = the_fight_at(flashes);
        println!(
            "{flashes}: {} frames, general {} a second (area {:.3}), red {} a second (area {:.3})",
            report.frames, report.general.rate, report.general.area, report.red.rate, report.red.area
        );
        if !report.passes() {
            wrong.push(format!("{flashes}: {report:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
