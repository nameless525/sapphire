//! The golden-image scenes and their runner, behind `cargo xtask golden` (spec §9 gate 3; M1
//! plan §3, open question Q6).
//!
//! Each scene is a sandbox `--capture` at 640×360 with fixed ticks: no window, no focus, and
//! fast enough on CPU rasterizers. The capture's adapter record names the backend family
//! ([`backend_key`]), and the capture is compared with `<golden dir>/<scene>.<family>.png`.
//!
//! - A failing comparison leaves `<scene>.<family>.actual.png` and `.diff.png` (the FLIP error
//!   map) in the output directory, which CI uploads.
//! - A scene with no golden for this family fails in CI (`CI` is set) and is skipped locally.
//!   Its capture is kept as `.actual.png`, ready to commit.
//! - [`Mode::Update`] writes this family's goldens from the captures instead of comparing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::golden::{self, Image, Thresholds, backend_key};

/// The environment variable the sandbox reads its cooked content from.
const CONTENT_DIR_ENV: &str = "SAPPHIRE_CONTENT_DIR";
/// Small enough for lavapipe and WARP to render each scene in a few seconds.
const SIZE: &str = "640x360";

pub struct Scene {
    pub name: &'static str,
    /// Sandbox arguments on top of `--capture <png> --size 640x360`.
    pub args: &'static [&'static str],
    pub thresholds: Thresholds,
}

pub fn scenes() -> [Scene; 5] {
    [
        Scene { name: "turntable-start", args: &["--capture-ticks", "0"], thresholds: Thresholds::default() },
        Scene { name: "turntable", args: &["--capture-ticks", "90"], thresholds: Thresholds::default() },
        // Without MSAA every outline and terminator edge is a hard pixel step, the case most
        // sensitive to precision differences.
        Scene {
            name: "turntable-msaa-off",
            args: &["--capture-ticks", "90", "--msaa", "off"],
            thresholds: Thresholds::default(),
        },
        // Three colored local lights through the clustered path (M1 R5): their splits on the
        // head and arm, and a spot's pool on the pedestal.
        Scene {
            name: "turntable-lights",
            args: &["--capture-ticks", "90", "--lights"],
            thresholds: Thresholds::default(),
        },
        // The scene at render scale 0.7 (448×252), upsampled by post's final pass (M1 R6).
        Scene {
            name: "turntable-render-scale",
            args: &["--capture-ticks", "90", "--render-scale", "0.7"],
            thresholds: Thresholds::default(),
        },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Compare,
    Update,
}

/// Where the runner finds and puts things.
pub struct Paths {
    /// The sandbox executable.
    pub sandbox: PathBuf,
    /// Cooked desktop content.
    pub content: PathBuf,
    /// `tests/golden`.
    pub goldens: PathBuf,
    /// Captures, actuals and diffs; emptied first.
    pub out: PathBuf,
}

/// Runs the selected scenes and returns one message per failure (empty: all passed).
pub fn run(paths: &Paths, mode: Mode, only: Option<&str>) -> Result<Vec<String>, String> {
    let scenes = scenes();
    let selected: Vec<&Scene> =
        scenes.iter().filter(|scene| only.is_none_or(|name| name == scene.name)).collect();
    if selected.is_empty() {
        let names: Vec<&str> = scenes.iter().map(|scene| scene.name).collect();
        return Err(format!(
            "no golden scene named {:?}; scenes: {}",
            only.unwrap_or_default(),
            names.join(", ")
        ));
    }
    let io = |what: &str, path: &Path, error: std::io::Error| format!("{what} {}: {error}", path.display());
    if paths.out.exists() {
        fs::remove_dir_all(&paths.out).map_err(|e| io("failed to empty", &paths.out, e))?;
    }
    fs::create_dir_all(&paths.out).map_err(|e| io("failed to create", &paths.out, e))?;
    let in_ci = std::env::var_os("CI").is_some();

    let mut failures = Vec::new();
    for scene in selected {
        let capture = paths.out.join(format!("{}.png", scene.name));
        let output = Command::new(&paths.sandbox)
            .arg("--capture")
            .arg(&capture)
            .args(["--size", SIZE])
            .args(scene.args)
            .env(CONTENT_DIR_ENV, &paths.content)
            .output()
            .map_err(|e| io("failed to run", &paths.sandbox, e))?;
        if !output.status.success() {
            let log = String::from_utf8_lossy(&output.stderr);
            failures.push(format!("{}: the sandbox failed ({}):\n{log}", scene.name, output.status));
            continue;
        }
        let family = backend_family(&capture)?;
        let golden_path = paths.goldens.join(format!("{}.{family}.png", scene.name));
        let kept = paths.out.join(format!("{}.{family}.actual.png", scene.name));
        let keep = || fs::copy(&capture, &kept).map(drop).map_err(|e| io("failed to write", &kept, e));

        if mode == Mode::Update {
            fs::create_dir_all(&paths.goldens).map_err(|e| io("failed to create", &paths.goldens, e))?;
            fs::copy(&capture, &golden_path).map_err(|e| io("failed to write", &golden_path, e))?;
            eprintln!("updated  {}", golden_path.display());
            continue;
        }
        if !golden_path.is_file() {
            keep()?;
            let message = format!(
                "{} [{family}]: no golden yet; the capture is at {} (`cargo xtask golden --update` adds it)",
                scene.name,
                kept.display()
            );
            if in_ci {
                failures.push(message);
            } else {
                eprintln!("skipped  {message}");
            }
            continue;
        }

        let golden = Image::read_png(&golden_path).map_err(|e| e.to_string())?;
        let actual = Image::read_png(&capture).map_err(|e| e.to_string())?;
        let comparison = golden::compare(&golden, &actual, &scene.thresholds).map_err(|e| e.to_string())?;
        let summary = format!(
            "{} [{family}]: mean {:.4} (≤ {}), p{} {:.4} (≤ {}), max {:.3}",
            scene.name,
            comparison.mean,
            scene.thresholds.mean,
            comparison.percentile,
            comparison.percentile_value,
            scene.thresholds.percentile_max,
            comparison.max,
        );
        if comparison.passes(&scene.thresholds) {
            eprintln!("ok       {summary}");
        } else {
            keep()?;
            let diff = paths.out.join(format!("{}.{family}.diff.png", scene.name));
            comparison.error_map.write_png(&diff).map_err(|e| e.to_string())?;
            failures.push(format!("{summary}\n    see {} and {}", kept.display(), diff.display()));
        }
    }
    Ok(failures)
}

/// The backend family of a capture, from the `<capture>.adapter.txt` the sandbox writes.
fn backend_family(capture: &Path) -> Result<String, String> {
    let path = capture.with_extension("adapter.txt");
    let text = fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .ok_or_else(|| format!("{} has no `{key}=` line", path.display()))
    };
    Ok(backend_key(field("backend")?, field("adapter")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_names_are_unique_file_stems() {
        let scenes = scenes();
        let mut names: Vec<&str> = scenes.iter().map(|scene| scene.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), scenes.len());
        assert!(names.iter().all(|name| name.chars().all(|c| c.is_ascii_lowercase() || c == '-')));
    }

    #[test]
    fn reads_the_backend_family_from_the_adapter_record() {
        let dir = std::env::temp_dir().join(format!("sapphire_testkit_scenes_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let capture = dir.join("scene.png");
        fs::write(dir.join("scene.adapter.txt"), "backend=vulkan\nadapter=llvmpipe (LLVM 19)\n").unwrap();
        assert_eq!(backend_family(&capture).unwrap(), "vulkan-lavapipe");
        fs::write(dir.join("scene.adapter.txt"), "backend=metal\n").unwrap();
        assert!(backend_family(&capture).is_err(), "no adapter line");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_unknown_scenes() {
        let paths = Paths {
            sandbox: PathBuf::from("unused"),
            content: PathBuf::from("unused"),
            goldens: PathBuf::from("unused"),
            out: PathBuf::from("unused"),
        };
        assert!(run(&paths, Mode::Compare, Some("no-such-scene")).unwrap_err().contains("turntable"));
    }
}
