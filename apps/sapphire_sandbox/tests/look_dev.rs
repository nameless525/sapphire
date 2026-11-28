//! The look-dev stage's one path (M6 T15a): `sapphire_sandbox --look-dev hero --day <file> --capture` draws, at
//! every tier, for a complete day and for a partial one (a cave's, which `--day` lays over the look-dev day),
//! what `sapphire_stage::lookdev::capture` draws of the typed stand of the same description, to the
//! pixel, as two look-dev captures of one day agree (most often exactly; now and then by a code or two in a few
//! pixels, the renderer's own run-to-run difference, which the editor's `how_far_two_look_dev_captures_agree`
//! measures at each tier and its `assert_look_runs_agree` bounds: four codes, 24 pixels). The sandbox
//! builds that description from its flags and applies the rest where the stand leaves them, so this holds that its
//! flags don't change what a plain look-dev capture is; the editor's `viewport_stage` holds its look pane, after a
//! live edit, to the same typed capture. Together they hold that the pane draws a sandbox capture.
//!
//! It draws, so it needs a GPU and cooked content and is `#[ignore]`d: `cargo xtask content`, then
//! `cargo test -p sapphire_sandbox --test look_dev --locked -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use sapphire_platform::Host;
use sapphire_render::quality::TierId;
use sapphire_stage::headless_gpu;
use sapphire_stage::lookdev::{self, LookDevDesc, Shot};
use sapphire_stage::scene::TownLook;
use sapphire_world::tod::TodTimeline;

/// The captures' size, and how many frames each draws.
const SIZE: (u32, u32) = (480, 270);
const FRAMES: u32 = 16;
/// Between the afternoon's key and the golden hour's, so the edited key shows in the blend.
const HOUR: f32 = 17.5;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("apps/<app>").to_owned()
}

/// A PNG's pixels, as tightly packed RGBA8 rows.
fn read_png(path: &Path) -> Vec<u8> {
    let file = fs::File::open(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().expect("a PNG");
    let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut pixels).expect("its pixels");
    assert_eq!((info.width, info.height, info.color_type), (SIZE.0, SIZE.1, png::ColorType::Rgba));
    pixels.truncate(info.buffer_size());
    pixels
}

#[test]
#[ignore = "needs a GPU and a cook"]
#[allow(clippy::print_stdout, reason = "the counts are for a person reading --nocapture")]
fn a_look_dev_capture_of_a_day_draws_the_typed_stands_capture_at_every_tier() {
    let workspace = workspace();
    let cook = workspace.join("target").join("content").join("desktop");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("look_dev");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a folder");
    // The look-dev day with its golden hour's exposure raised, as an edit saved from the editor leaves it.
    let shipped = fs::read_to_string(workspace.join("content/lookdev/lookdev_day.ron")).expect("the day");
    assert_eq!(shipped.matches("exposure: -0.05,").count(), 1, "the golden hour's exposure");
    let text = shipped.replace("exposure: -0.05,", "exposure: 0.6,");
    let day = dir.join("edited_day.ron");
    fs::write(&day, &text).expect("a day");
    let look = TownLook { day: TodTimeline::from_ron(&text).expect("a day"), ..TownLook::default() };
    // A partial day, a biome's: `--day` lays it over the look-dev day, as the look pane does.
    let cave = workspace.join("content/looks/biomes/cave_day.ron");
    let partial = TodTimeline::from_ron(&fs::read_to_string(&cave).expect("the cave")).expect("a day");
    assert!(!partial.is_complete(), "a partial day");
    let cave_look = TownLook { over: Some(partial), ..TownLook::default() };
    let host = Host::with_content_at(&cook);
    let gpu = Arc::new(headless_gpu().expect("a GPU"));
    let mut apart = Vec::new();
    for ((name, day, look), tier) in [("edited", &day, &look), ("cave", &cave, &cave_look)]
        .into_iter()
        .flat_map(|case| TierId::ALL.map(|tier| (case, tier)))
    {
        let png = dir.join(format!("{name}_{}.png", tier.key()));
        let output = Command::new(env!("CARGO_BIN_EXE_sapphire_sandbox"))
            .current_dir(&workspace)
            .args(["--look-dev", "hero", "--tier", tier.key(), "--time-of-day", &HOUR.to_string()])
            .arg("--day")
            .arg(day)
            .arg("--capture")
            .arg(&png)
            .args(["--capture-ticks", "0", "--capture-frames", &FRAMES.to_string()])
            .args(["--size", &format!("{}x{}", SIZE.0, SIZE.1)])
            .env("SAPPHIRE_CONTENT_DIR", &cook)
            .env_remove("RUST_LOG")
            .output()
            .expect("the sandbox runs");
        assert!(output.status.success(), "{}: {}", tier.name(), String::from_utf8_lossy(&output.stderr));
        let sandbox = read_png(&png);
        let desc = LookDevDesc::new(
            &host,
            &gpu,
            (Shot::Hero, tier, HOUR),
            look.clone(),
            game_combat::playable::KIND,
        );
        let mut stage = lookdev::stand(&host, Arc::clone(&gpu), desc).expect("the typed stand");
        assert_eq!(stage.quality.active().id, tier);
        let typed = lookdev::capture(&mut stage, SIZE, FRAMES).expect("its capture");
        let pipelines = stage.renderer.stats().pipelines;
        assert_eq!(
            (pipelines.first_use_builds, pipelines.frame_thread_builds, pipelines.frame_waits),
            (0, 0, 0),
            "{}: {}",
            tier.name(),
            stage.renderer.first_use_report()
        );
        let (mut most, mut differ) = (0_u8, 0_usize);
        for (a, b) in sandbox.as_chunks::<4>().0.iter().zip(typed.as_chunks::<4>().0) {
            let by = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            most = most.max(by);
            differ += usize::from(by > 0);
        }
        println!("{name} {}: {differ} of {} pixels differ, by at most {most}", tier.name(), SIZE.0 * SIZE.1);
        apart.push((name, tier, most, differ));
    }
    // As two look-dev captures of one day agree: by a code or two in at most a few pixels.
    assert!(apart.iter().all(|(_, _, most, differ)| *most <= 4 && *differ <= 24), "{apart:?}");
}
