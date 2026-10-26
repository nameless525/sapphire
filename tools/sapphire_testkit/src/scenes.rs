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

/// The scenes, in the order they were added. Those from before the sky's layers (M1 W2) pin the
/// sky's gradient alone, `--plain-sky` (no discs, glows, stars or clouds), so they stay
/// pixel-identical to their goldens: they are about the character, and the sky has scenes of its
/// own. Those from before temporal AA (M1 R7) pin 4× MSAA (`--aa msaa`), which the desktop tiers
/// drew with until then, for the same reason. Those from before post's compositing layers (M1 P2)
/// pin post as it was, `--plain-post` (no diffusion glow, character bloom, gradient layers, lens
/// flare or vignette tint), which the post scenes show; the environment's (M1 W4) among them, which
/// draw the world as the desktop tiers do, under the whole sky and temporal AA. The outline scenes
/// (M1 C4) pin all three, so they show the lines alone. Those from before shadows (M1 R4), the
/// post and outline scenes included, pin them off (`--shadows off`, the player's quality setting),
/// so they stay pixel-identical too. The shadow scenes pin their tier, whose shadows they show
/// (the golden runners select Desktop-Low by device), and plain post, so they show shadows alone.
/// The screen effect scenes (M1 P3) push the sample presets held on one frame
/// (`--post-effect <preset>@<seconds>`) over the full look; no earlier scene pushes any, and an
/// empty effect stack draws exactly what it drew before, so nothing earlier needs pinning.
pub fn scenes() -> [Scene; 36] {
    [
        Scene {
            name: "turntable-start",
            args: &[
                "--capture-ticks",
                "0",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        Scene {
            name: "turntable",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Without MSAA or TAA every outline and terminator edge is a hard pixel step, the case
        // most sensitive to precision differences.
        Scene {
            name: "turntable-msaa-off",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "off",
                "--plain-sky",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Three colored local lights through the clustered path (M1 R5): their splits on the
        // head and arm, and a spot's pool on the pedestal.
        Scene {
            name: "turntable-lights",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--lights",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The scene at render scale 0.7 (448×252), upsampled by post's final pass (M1 R6).
        Scene {
            name: "turntable-render-scale",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--render-scale",
                "0.7",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The procedural stalk beside the character (M1 R2): compute skinning bends it and its
        // two morph targets swell its top and narrow its middle, drawn from its skinning outputs
        // with shading and outline.
        Scene {
            name: "turntable-skinned",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--skinned",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset from the default day (M1 W1), between its golden-hour and sunset keys: a low
        // orange key light from the right, the sky's horizon band, violet-tinted shade and
        // post's two grading LUTs blended (80% toward sunset's).
        Scene {
            name: "turntable-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "18.5",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The anime character shader swatches beside the character (M1 C1): ramp mode over the
        // cooked swatch ramp and control map (per-ID rows, a striped shadow bias, a painted
        // shadow), the same with a subsurface mix, and three bands in per-ID colors from vertex
        // material IDs.
        Scene {
            name: "turntable-swatches",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--swatches",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Night from the default day (M1 C2), halfway from dusk to midnight: the character lit by
        // its light rig's key (time of day's character key, brighter than the world's moon, from
        // the moon high on the camera's left) and stylized GI under a deep blue ambient, against
        // the night sky and two blended grading LUTs.
        Scene {
            name: "turntable-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "22",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The face look-dev rig (M1 C3): the cooked face test head in two rows, each lit by the
        // key light from its own angle in head space. A yaw sweep from 135° on the heads' right
        // round to 135° on their left (mirror-image SDF shadows, eyes and brows lit), then a top
        // light, light from above and to the side, a blush, and a gloom with a blush.
        Scene {
            name: "faces",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--faces",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The sky by day (M1 W2), looking up past the character toward the morning sun: its
        // white-hot disc and wide glow, cirrus streaks overhead, and the card clouds' band on the
        // horizon, two-tone (white tops over lavender shade) with a rim, at 9:00's coverage.
        Scene {
            name: "sky-day",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "9",
                "--view",
                "99,-22",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset (M1 W2): the sun low in a gap of the card clouds, its glow spread along the
        // horizon, gold-lit clouds over violet shade with bright rims toward the sun, and cirrus
        // catching the color.
        Scene {
            name: "sky-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "18.5",
                "--view",
                "-100,-6",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Night (M1 W2): the moon's disc and halo, procedural stars and the Milky Way on the
        // turning celestial sphere, and dark clouds with moonlit rims.
        Scene {
            name: "sky-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--time-of-day",
                "21",
                "--view",
                "112,-20",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Temporal AA (M1 R7), the desktop tiers' antialiasing: the turntable held still while
        // 32 jittered frames converge, so outlines and light splits are antialiased as 4× MSAA
        // draws them.
        Scene {
            name: "turntable-taa",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The turntable spinning at 3 rad/s, about 8 times its usual speed, for the 32 frames up
        // to the capture: outlines and two-tone edges must follow without ghost trails or smears.
        Scene {
            name: "turntable-taa-motion",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "taa",
                "--spin",
                "3",
                "--capture-frames",
                "32",
                "--capture-step",
                "1",
                "--plain-sky",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Temporal upsampling: the scene at two thirds of the output's size (427×240), resolved
        // to 640×360 over 32 still frames.
        Scene {
            name: "turntable-taa-upsampled",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--aa",
                "taa",
                "--render-scale",
                "0.667",
                "--capture-frames",
                "32",
                "--plain-sky",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The environment shader (M1 W4) by day: the character in the courtyard, under the morning
        // sun from the front left. Painted ground, a colonnade receding into rolling hills, a
        // plaster wall and boulders: soft wrapped light, lavender shadow sides with a warm band
        // along the pillars' terminators, terminators softening and colors greying with
        // distance, and the character reading on top.
        Scene {
            name: "environment-day",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset (18:30): the low orange sun from the right warms every lit face, the shadow
        // sides keep time of day's violet tint, and the lanterns begin to glow.
        Scene {
            name: "environment-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "18.5",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Night (22:00): the world under the moon and a deep blue ambient, the lanterns glowing
        // orange, and the demo lights' pools on the ground and the pedestal (local lights through
        // both families' shaders), the character lit by its own rig's brighter key.
        Scene {
            name: "environment-night",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "22",
                "--lights",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Post's compositing layers by day (M1 P2), through temporal AA as the desktop tiers draw:
        // the diffusion glow softening the clouds, the top of the frame deepened toward the sky's
        // blue, the ground's shade toward the bottom, a soft pool of light on the character, whose
        // own bloom is held back (the character mask).
        Scene {
            name: "post-day",
            args: &[
                "--capture-ticks",
                "90",
                "--time-of-day",
                "10",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Sunset through Mobile-Low's fused pass (4× MSAA, four bloom levels): a warm, dreamy
        // glow over the clouds and along the horizon, violet from above, the ground in a deep
        // violet shade below, the character crisp in front of it.
        Scene {
            name: "post-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--time-of-day",
                "18.5",
                "--tier",
                "mobile-low",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The lens flare: the sunset sun in view beside the character, its warm ring and streak,
        // colored ghosts down the axis through the middle of the frame, and the diffusion glow
        // around the sun.
        Scene {
            name: "post-flare",
            args: &[
                "--capture-ticks",
                "90",
                "--time-of-day",
                "18.5",
                "--view",
                "-100,-6",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Outline width over distance (M1 C4): more of the character from 4 m to 90 m behind it,
        // seen from nearly level. Their lines thin along the width's two-segment curve, hold
        // their width in the world far away, and are gone on the last, past the LOD cutoff.
        Scene {
            name: "outlines-distance",
            args: &[
                "--capture-ticks",
                "90",
                "--outline-row",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same row through a 20° lens from farther away, the character framed as before:
        // its lines keep their width, and the row's, nearer in effective distance, widen.
        Scene {
            name: "outlines-telephoto",
            args: &[
                "--capture-ticks",
                "90",
                "--outline-row",
                "--fov",
                "20",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The hard-edged sample (M1 C4): flat-shaded shapes with every edge split, whose cooked
        // smoothed outline normals keep each corner's hull closed.
        Scene {
            name: "outlines-hard-edges",
            args: &[
                "--capture-ticks",
                "90",
                "--hard-edges",
                "--plain-post",
                "--plain-sky",
                "--aa",
                "msaa",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // A close-up of the character turning at 3 rad/s under temporal AA (M1 C4): its hulls
        // write their line's velocity in the prepass, the silhouette's (M1 P1), and moving lines
        // resolve from the current frame, so they stay as crisp as 4× MSAA draws them.
        Scene {
            name: "outlines-turning-taa",
            args: &[
                "--capture-ticks",
                "90",
                "--zoom",
                "0.6",
                "--aa",
                "taa",
                "--spin",
                "3",
                "--capture-frames",
                "32",
                "--capture-step",
                "1",
                "--plain-post",
                "--plain-sky",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // The same close-up upsampled (M1 P1): the scene at two thirds of the output's size
        // (427×240 to 640×360), as Desktop-High draws 4K from 1440 rows. The turning lines keep
        // their width in output pixels and their darkness.
        Scene {
            name: "outlines-turning-taa-upsampled",
            args: &[
                "--capture-ticks",
                "90",
                "--zoom",
                "0.6",
                "--aa",
                "taa",
                "--render-scale",
                "0.667",
                "--spin",
                "3",
                "--capture-frames",
                "32",
                "--capture-step",
                "1",
                "--plain-post",
                "--plain-sky",
                "--shadows",
                "off",
            ],
            thresholds: Thresholds::default(),
        },
        // Shadows (M1 R4): the character on the stage's pedestal, split off as environment, on
        // Desktop-High at 10:00. Its own map's contact shadow falls on the pedestal around its
        // feet and its shadow runs on across the floor, the pedestal casts its own through the
        // cascades, and the figure shades without a mark of its own shadow: characters never
        // shadow themselves. Converged under temporal AA.
        Scene {
            name: "shadows-contact",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "floor",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
            ],
            thresholds: Thresholds::default(),
        },
        // A pillar between the sun and the character: its shadow covers the pedestal and the
        // character from the feet to the chest, a clean edge across the body in the character's
        // authored shade tones, with no acne on the pillar, the floor or the figure.
        Scene {
            name: "shadows-pillar",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "pillar",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
            ],
            thresholds: Thresholds::default(),
        },
        // The face rig (M1 C3) in a pillar's shadow: the faces it covers go into their authored
        // shadow tones whole, eyes and brows with them, while the lit ones keep their face-map
        // shadows; no shadow-map texels on any face.
        Scene {
            name: "shadows-faces",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--faces",
                "--stage",
                "pillar",
                "--time-of-day",
                "14",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
                "--plain-sky",
            ],
            thresholds: Thresholds::default(),
        },
        // The courtyard (W4) by day with shadows: the colonnade, the wall and the boulders cast
        // through the cascades onto the ground and each other, the character's own map casts on
        // the ground beyond its pedestal, and in every shadow the ground keeps time of day's
        // tint of the sun through the environment shader's terminator.
        Scene {
            name: "shadows-courtyard",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Toward sunset (18:00) in the courtyard: the low sun from the right draws the pillars' and
        // the lanterns' shadows long across the ground into the far cascades, tinted, their edges
        // clean and still. (By 18:30 the wall's shadow already covers the colonnade.)
        Scene {
            name: "shadows-sunset",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--environment",
                "--time-of-day",
                "18",
                "--tier",
                "desktop-high",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Mobile-Low's shadows: two cascades, the 3 × 3 filter and 4× MSAA, and the character's
        // capsule instead of a map of its own, a soft blob around its feet on the pedestal running
        // on across the floor.
        Scene {
            name: "shadows-capsule",
            args: &[
                "--capture-ticks",
                "90",
                "--plain-post",
                "--stage",
                "floor",
                "--tier",
                "mobile-low",
                "--plain-sky",
            ],
            thresholds: Thresholds::default(),
        },
        // A dash's screen effects (M1 P3): the radial blur streaking the frame toward its middle
        // and white speed lines radiating from outside a clear middle, which the character stays
        // in front of. Through temporal AA, still, on the default day.
        Scene {
            name: "effects-blur-lines",
            args: &[
                "--capture-ticks",
                "90",
                "--post-effect",
                "radial_blur@0.05",
                "--post-effect",
                "speed_lines@0.3",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // The impact frame (M1 P3): the world in a dark-and-cream duotone with black speed lines
        // behind the character, which keeps its own colors through the character mask, under the
        // radial blur and chromatic aberration.
        Scene {
            name: "effects-impact-frame",
            args: &[
                "--capture-ticks",
                "90",
                "--post-effect",
                "impact_frame@0.03",
                "--aa",
                "taa",
                "--capture-frames",
                "32",
            ],
            thresholds: Thresholds::default(),
        },
        // Chromatic aberration and the inversion through Mobile-Low's settings (M1 P3): red and
        // blue fringes growing toward the corners of the inverted frame.
        Scene {
            name: "effects-chromatic-inversion",
            args: &[
                "--capture-ticks",
                "90",
                "--post-effect",
                "chromatic_aberration@0.02",
                "--post-effect",
                "inversion@0.02",
                "--tier",
                "mobile-low",
            ],
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
