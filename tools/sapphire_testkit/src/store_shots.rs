//! The stores' screenshots (M4 S5), behind `cargo xtask store-shots`: golden scenes of the game in play, drawn
//! again at each store's sizes, as a phone's or a desktop's screen shows them.
//!
//! A shot borrows a golden scene's command line ([`scenes::scenes`], `tests/golden/scenes.ron`), so it draws by the same sandbox
//! `--capture` road and moves when its scene does. Each [`Size`] replaces the scene's size and tier and drops its
//! `--ao off`, a golden's pin, so the frame is drawn as the tier draws it, and stands the menus and the HUD on
//! the store's screen: its pixels per point, and on-screen controls only on a touch screen (the sandbox's
//! `--display-scale` and `--no-touch-controls`). A shot with words in it (the game's own show, `--game`) is drawn
//! in each language; one without is drawn once and copied to both.
//!
//! The pictures go to `<out>/<store>/<size>/<language>/<nn>-<shot>.png`, as RGB with no alpha, which Apple and
//! Play require, and each capture's log to `<out>/logs/`. Nothing here is committed: the docs in `docs/store/`
//! name the sizes, and a test holds them to [`SIZES`].

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sapphire_time::Stopwatch;

use crate::golden::Image;
use crate::scenes::{self, Scene, log_tail, run_with_deadline};

/// The languages a store's page is written in, as the game's own settings name them.
pub const LANGUAGES: [&str; 2] = ["en", "ja"];

/// A store, by its doc in `docs/store/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Store {
    Apple,
    GooglePlay,
    Steam,
}

impl Store {
    pub const ALL: [Self; 3] = [Self::Apple, Self::GooglePlay, Self::Steam];

    /// Its directory under the output, and its doc's name in `docs/store/`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::GooglePlay => "google-play",
            Self::Steam => "steam",
        }
    }
}

/// A size a store takes screenshots at, and the screen it stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub store: Store,
    /// Its name in its store's doc and its directory.
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    /// Pixels per point of the screen it stands for: the menus and the HUD are as large on the frame as there.
    pub scale: f32,
    /// The tier that screen's device draws at.
    pub tier: &'static str,
    /// Whether it's a touch screen, whose HUD shows the on-screen controls.
    pub touch: bool,
}

/// Every store's sizes, landscape since the game is: each doc's "Screenshots" table says why each is there and
/// where the store says so.
pub const SIZES: [Size; 7] = [
    // iPhone's 6.9" display, the largest Apple takes, and the 6.3" one, its medium display.
    phone(Store::Apple, "iphone-6.9", 2868, 1320),
    phone(Store::Apple, "iphone-6.3", 2622, 1206),
    // The 13" iPad, since the game runs on iPad (UIDeviceFamily 1 and 2).
    Size {
        store: Store::Apple,
        name: "ipad-13",
        width: 2752,
        height: 2064,
        scale: 2.0,
        tier: "mobile-high",
        touch: true,
    },
    // Play's phone at the 1920×1080 a game's landscape shots need for recommendations, at xxhdpi.
    phone(Store::GooglePlay, "phone", 1920, 1080),
    // Play's large screens, 7" and 10" tablets alike: 16:9, at least 1080 px a side, at xhdpi.
    Size {
        store: Store::GooglePlay,
        name: "tablet",
        width: 2560,
        height: 1440,
        scale: 2.0,
        tier: "mobile-high",
        touch: true,
    },
    // Steam's screenshots, 1920×1080 at least, as a desktop at one pixel a point draws them.
    Size {
        store: Store::Steam,
        name: "desktop",
        width: 1920,
        height: 1080,
        scale: 1.0,
        tier: "desktop-high",
        touch: false,
    },
    // The Steam Deck's own screen: the Deck self-check's evidence, not a store slot.
    Size {
        store: Store::Steam,
        name: "deck",
        width: 1280,
        height: 800,
        scale: 1.0,
        tier: "desktop-low",
        touch: false,
    },
];

const fn phone(store: Store, name: &'static str, width: u32, height: u32) -> Size {
    Size { store, name, width, height, scale: 3.0, tier: "mobile-high", touch: true }
}

/// A screenshot: a golden scene of the game in play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shot {
    /// Its file's name after its place in the store's order.
    pub name: &'static str,
    /// The golden scene whose command line it draws.
    pub scene: &'static str,
}

/// The shots, in the order a store's page shows them: the fight first, then the ward's story and its streets.
pub const SHOTS: [Shot; 6] = [
    // The Market Court's encounter, a warrior and a Lancer at once.
    Shot { name: "market-fight", scene: "fight-ward-market" },
    // A boss past half its health, the violet diamond over it marking the phase it crossed.
    Shot { name: "boss-phase", scene: "arena-boss" },
    // The ward's boss on its terrace.
    Shot { name: "terrace-boss", scene: "fight-ward-terrace" },
    // The court gate's cutscene, the warden mid-line with its subtitles.
    Shot { name: "gate-cutscene", scene: "game-ward-gate-wide-en" },
    // The court's checkpoint, as a continue stands the game.
    Shot { name: "court", scene: "game-ward-checkpoint" },
    // The ward's hub in the afternoon.
    Shot { name: "ward-hub", scene: "zone-ward-hub" },
];

/// Arguments of a golden scene's that a store's size replaces: `--size`, `--tier`, the scene's language, and
/// `--ao off`, which pins occlusion off where the tier would draw it.
const REPLACED: [&str; 4] = ["--size", "--tier", "--game-language", "--ao"];

/// The sandbox's arguments for `shot` at `size` in `language`, after `--capture <png>`, from its golden among
/// `scenes`. A scene's path under `tests/` is found under `tests`.
pub fn args(
    shot: &Shot,
    size: &Size,
    language: &str,
    scenes: &[Scene],
    tests: &Path,
) -> Result<Vec<String>, String> {
    let scene = scenes.iter().find(|scene| scene.name == shot.scene).ok_or_else(|| {
        format!("the shot {} draws the scene {}, which no golden has", shot.name, shot.scene)
    })?;
    let mut args = Vec::with_capacity(scene.args.len() + 8);
    let mut given = scene.args.iter().map(String::as_str);
    while let Some(arg) = given.next() {
        if REPLACED.contains(&arg) {
            given.next();
            continue;
        }
        args.push(match arg.strip_prefix("tests/") {
            Some(path) => tests.join(path).to_string_lossy().into_owned(),
            None => arg.to_owned(),
        });
    }
    args.extend(["--size".to_owned(), format!("{}x{}", size.width, size.height)]);
    args.extend(["--tier".to_owned(), size.tier.to_owned()]);
    let game = scene.has("--game");
    if game {
        args.extend(["--game-language".to_owned(), language.to_owned()]);
    }
    // The flags draw a screen's menus or HUD, so only a shot that has one takes them.
    if game || scene.has("--hud") {
        args.extend(["--display-scale".to_owned(), size.scale.to_string()]);
        if !size.touch {
            args.push("--no-touch-controls".to_owned());
        }
    }
    Ok(args)
}

/// Whether `shot` has words in it, so each language draws its own.
pub fn worded(shot: &Shot, scenes: &[Scene]) -> bool {
    scenes.iter().any(|scene| scene.name == shot.scene && scene.has("--game"))
}

/// Where the shots are made from and go.
pub struct Paths {
    pub sandbox: PathBuf,
    /// Cooked desktop content.
    pub content: PathBuf,
    /// `tests/`, for the replays and saves the scenes name.
    pub tests: PathBuf,
    /// Emptied first.
    pub out: PathBuf,
}

/// Draws every shot at every size, in each language, writing them under `paths.out`. Returns what failed; `Err`
/// stops the run (the output can't be written).
pub fn run(paths: &Paths, timeout: Duration) -> Result<Vec<String>, String> {
    crate::output_paths::disjoint(&paths.out, &[&paths.tests, &paths.content, &paths.sandbox])?;
    if paths.out.exists() {
        fs::remove_dir_all(&paths.out)
            .map_err(|e| format!("failed to empty {}: {e}", paths.out.display()))?;
    }
    let logs = paths.out.join("logs");
    fs::create_dir_all(&logs).map_err(|e| format!("failed to create {}: {e}", logs.display()))?;
    let scenes = scenes::scenes(&paths.tests.join("golden"))?;

    let clock = Stopwatch::start();
    let mut failures = Vec::new();
    let mut made = BTreeMap::new();
    for size in &SIZES {
        for (index, shot) in SHOTS.iter().enumerate() {
            let file = format!("{:02}-{}.png", index + 1, shot.name);
            let mut drawn: Option<PathBuf> = None;
            for language in LANGUAGES {
                let dir = paths.out.join(size.store.name()).join(size.name).join(language);
                fs::create_dir_all(&dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
                let into = dir.join(&file);
                match &drawn {
                    Some(first) if !worded(shot, &scenes) => {
                        fs::copy(first, &into)
                            .map_err(|e| format!("failed to write {}: {e}", into.display()))?;
                    }
                    _ => {
                        let label = format!("{}-{}-{language}-{}", size.store.name(), size.name, shot.name);
                        let args = args(shot, size, language, &scenes, &paths.tests)?;
                        match draw(args, paths, (&logs, &label), (size, timeout))? {
                            Ok(image) => write_rgb(&image, &into)?,
                            Err(why) => {
                                eprintln!("failed   {label}: {why}");
                                failures.push(format!("{label}: {why}"));
                                continue;
                            }
                        }
                        eprintln!(
                            "drew     {label} ({}x{}) {:.0} s",
                            size.width,
                            size.height,
                            clock.elapsed().as_secs_f64()
                        );
                        drawn = Some(into.clone());
                    }
                }
                *made.entry((size.store.name(), size.name)).or_insert(0) += 1;
            }
        }
    }
    for ((store, size), count) in &made {
        eprintln!("{store}/{size}: {count} screenshots");
    }
    eprintln!(
        "store screenshots in {:.1} min, under {}",
        clock.elapsed().as_secs_f64() / 60.0,
        paths.out.display()
    );
    Ok(failures)
}

/// One capture of `args` ([`args`]) at `size`, its PNG and log in `logs` named by `label`: the image, or why
/// this shot failed. `Err` stops the run.
fn draw(
    args: Vec<String>,
    paths: &Paths,
    (logs, label): (&Path, &str),
    (size, timeout): (&Size, Duration),
) -> Result<Result<Image, String>, String> {
    // Not `with_extension`: a size's name may hold a dot (`iphone-6.9`).
    let capture = logs.join(format!("{label}.png"));
    let log = logs.join(format!("{label}.log"));
    let mut command = Command::new(&paths.sandbox);
    command.env(scenes::CONTENT_DIR_ENV, &paths.content).arg("--capture").arg(&capture);
    command.args(args);
    let exit = run_with_deadline(&mut command, &log, timeout)
        .map_err(|e| format!("failed to run the sandbox ({}): {e}", log.display()))?;
    let failed =
        |what: String| Ok(Err(format!("{what}; its log, {}, ends:\n{}", log.display(), log_tail(&log))));
    match exit {
        None => return failed(format!("timed out after {} s", timeout.as_secs())),
        Some(exit) if !exit.success() => return failed(format!("the sandbox failed ({exit})")),
        Some(_) => {}
    }
    let image = match Image::read_png(&capture) {
        Ok(image) => image,
        Err(why) => return failed(why.to_string()),
    };
    if (image.width, image.height) != (size.width, size.height) {
        return failed(format!("drew {}x{}, not {}x{}", image.width, image.height, size.width, size.height));
    }
    Ok(Ok(image))
}

/// `image` as an RGB PNG at `path`: neither Apple nor Play takes an alpha channel.
fn write_rgb(image: &Image, path: &Path) -> Result<(), String> {
    let error = |e: &dyn std::fmt::Display| format!("failed to write {}: {e}", path.display());
    let file = File::create(path).map_err(|e| error(&e))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = image.rgba.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [r, g, b]).collect();
    let mut writer = encoder.write_header().map_err(|e| error(&e))?;
    writer.write_image_data(&rgb).map_err(|e| error(&e))?;
    writer.finish().map_err(|e| error(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/store/<store>.md`.
    fn doc(store: Store) -> String {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../docs/store/{}.md", store.name()));
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The text of a doc's `## <heading>` section, up to the next.
    fn section<'t>(text: &'t str, heading: &str, store: Store) -> &'t str {
        let start = text.split(&format!("\n## {heading}\n")).nth(1);
        let start = start.unwrap_or_else(|| panic!("docs/store/{}.md has no {heading}", store.name()));
        start.split("\n## ").next().unwrap_or_default()
    }

    /// The rows of a store doc's "Screenshots" table: a size's name in backticks, its pixels, its pixels per
    /// point, its tier, and `touch` or `none` for its controls.
    fn documented(store: Store) -> Vec<(String, u32, u32, f32, String, bool)> {
        let text = doc(store);
        section(&text, "Screenshots", store)
            .lines()
            .filter_map(|line| {
                let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(str::trim).collect();
                let name = cells.first()?.strip_prefix('`')?.strip_suffix('`')?;
                let (width, height) = cells.get(1)?.split_once('×')?;
                let touch = match *cells.get(4)? {
                    "touch" => true,
                    "none" => false,
                    other => panic!("{}: `{name}`'s controls are {other:?}, not touch or none", store.name()),
                };
                Some((
                    name.to_owned(),
                    width.parse().ok()?,
                    height.parse().ok()?,
                    cells.get(2)?.parse().ok()?,
                    (*cells.get(3)?).to_owned(),
                    touch,
                ))
            })
            .collect()
    }

    #[test]
    fn each_stores_doc_lists_the_sizes_the_command_draws() {
        for store in Store::ALL {
            let drawn: Vec<_> = SIZES
                .iter()
                .filter(|size| size.store == store)
                .map(|size| {
                    (
                        size.name.to_owned(),
                        size.width,
                        size.height,
                        size.scale,
                        size.tier.to_owned(),
                        size.touch,
                    )
                })
                .collect();
            assert!(!drawn.is_empty(), "{store:?} has no sizes");
            assert_eq!(
                documented(store),
                drawn,
                "docs/store/{}.md's Screenshots against SIZES",
                store.name()
            );
        }
    }

    /// Each draft in a store doc's "Store text" is a `#### <language>: <field> (<limit> characters|bytes)`
    /// heading and a `text` block, within its store's limit for that field: Apple counts keywords in bytes.
    #[test]
    fn each_stores_text_fits_its_limits() {
        for store in Store::ALL {
            let text = doc(store);
            let mut lines = section(&text, "Store text", store).lines();
            let mut fields = Vec::new();
            while let Some(line) = lines.next() {
                let Some(heading) = line.strip_prefix("#### ") else { continue };
                let limit = heading.rsplit_once(" (").and_then(|(_, limit)| limit.strip_suffix(')'));
                let (count, unit) = limit.and_then(|limit| limit.split_once(' ')).unwrap_or_else(|| {
                    panic!("{}: {heading:?} names no limit", store.name());
                });
                let count: usize = count.parse().unwrap_or_else(|_| panic!("{}: {heading:?}", store.name()));
                let fence = lines.by_ref().find(|line| !line.trim().is_empty());
                assert_eq!(fence, Some("```text"), "{}: {heading:?} has no text block", store.name());
                let body = lines.by_ref().take_while(|line| *line != "```").collect::<Vec<_>>().join("\n");
                let used = match unit {
                    "characters" => body.chars().count(),
                    "bytes" => body.len(),
                    other => panic!("{}: {heading:?} counts {other:?}", store.name()),
                };
                assert!(!body.trim().is_empty(), "{}: {heading:?} is empty", store.name());
                assert!(used <= count, "{}: {heading:?} has {used} {unit}", store.name());
                fields.push(heading.split_once(": ").map_or(heading, |(_, field)| field).to_owned());
            }
            let english: Vec<_> = fields.iter().take(fields.len() / 2).collect();
            let japanese: Vec<_> = fields.iter().skip(fields.len() / 2).collect();
            assert!(
                !english.is_empty() && english == japanese,
                "{}: each field in both languages",
                store.name()
            );
        }
    }

    #[test]
    fn every_size_is_one_its_store_takes() {
        for size in &SIZES {
            let (long, short) = (size.width.max(size.height), size.width.min(size.height));
            assert!(size.width > size.height, "{}: the game is landscape", size.name);
            match size.store {
                // Play: 320 to 3840 px a side, the long side at most twice the short.
                Store::GooglePlay => {
                    assert!(short >= 1080 && long <= 3840 && long <= 2 * short, "{}", size.name)
                }
                // Steam: 16:9 at 1920×1080 or more, and the Deck's own 1280×800.
                Store::Steam if size.name == "deck" => assert_eq!((size.width, size.height), (1280, 800)),
                Store::Steam => {
                    assert!(size.width * 9 == size.height * 16 && size.width >= 1920, "{}", size.name)
                }
                Store::Apple => {}
            }
            assert!(size.scale >= 1.0 && size.touch == size.tier.starts_with("mobile"), "{}", size.name);
        }
    }

    #[test]
    fn every_shot_draws_a_golden_scene_at_the_size_and_on_the_screen_asked() {
        let tests = Path::new("tests-dir");
        let scenes =
            scenes::scenes(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")).unwrap();
        let worded = |shot: &Shot| worded(shot, &scenes);
        for shot in &SHOTS {
            for size in &SIZES {
                let args = args(shot, size, "ja", &scenes, tests).unwrap();
                let after =
                    |flag: &str| args.iter().position(|arg| arg == flag).map(|i| args[i + 1].as_str());
                let count = |flag: &str| args.iter().filter(|arg| *arg == flag).count();
                // One of each, the size's: a scene's own would draw over it.
                for flag in ["--size", "--tier"] {
                    assert_eq!(count(flag), 1, "{} {flag}", shot.name);
                }
                assert_eq!(after("--size"), Some(format!("{}x{}", size.width, size.height).as_str()));
                assert_eq!(after("--tier"), Some(size.tier));
                assert_eq!(count("--ao"), 0, "{}: the tier's occlusion, not a golden's pin", shot.name);
                assert_eq!(after("--game-language"), worded(shot).then_some("ja"), "{}", shot.name);
                let screen = args.iter().any(|arg| arg == "--game" || arg == "--hud");
                assert_eq!(after("--display-scale").is_some(), screen, "{}", shot.name);
                assert_eq!(
                    args.contains(&"--no-touch-controls".to_owned()),
                    screen && !size.touch,
                    "{}",
                    shot.name
                );
                assert!(!args.iter().any(|arg| arg.starts_with("tests/")), "{}: {args:?}", shot.name);
            }
        }
        assert!(SHOTS.iter().filter(|shot| worded(shot)).count() >= 2, "the story's shots have words");
        assert!(SHOTS.len() >= 5, "Steam asks for five at least");
    }
}
