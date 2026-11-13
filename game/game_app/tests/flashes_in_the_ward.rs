//! Exit test 6's flashes in the game that ships (M4 A7): the Lantern Ward's whole walk on the game's stage, as
//! `wins_the_ward` plays it (`tests/replays/ward-whole-walk.sreplay`: a new game, the market's, the garden's and
//! the court's fights, the court's gate cutscene, the player's ultimates whenever the meter's full, and the
//! boss on the terrace, to the victory), every frame at 60 fps judged by WCAG 2.3.1's general and red
//! flash tests as `sapphire_testkit::flashes` reads them, at the flashes setting a player can choose. It prints
//! the worst second of each fight, the gate's cutscene and each ultimate's cinematic, and of the walk as a whole,
//! and fails on more than six transitions in any second (WCAG's three flashes), or on more red transitions in a
//! second than `MOST_RED` (the project's bar is none; see there).
//!
//! A frame a step, as a device that keeps up draws them, at 480 × 270 on Desktop-High, as A3 measured the
//! arena's fights: the analyser sizes its field by the height, so a smaller capture judges the same frame.
//!
//! - `SAPPHIRE_FLASHES=<setting>[,<setting>]`: `normal` (the default setting) or `reduced`; unset, both.
//! - `SAPPHIRE_FLASH_PART=<i>/<n>`: judges only the seconds ending in the `i`th of `n` equal parts of the walk's
//!   frames, so that the parts together judge every second of the walk once. The frames before a part are
//!   played, prepared and recorded but not drawn (`Stage::lose_frames`); its last two seconds before it are
//!   drawn and read, so the picture's history and the analyser's are the walk's when its first second is judged.
//!   CI's lavapipe can't draw the whole walk inside one job's time, so its nightly run judges it in parts, a
//!   runner each.
//! - `SAPPHIRE_FLASH_PART=<from>..<to>`: judges the seconds ending on the walk's frames `from..to`, as a part
//!   does: to look at a second.
//! - `SAPPHIRE_FLASH_WRITE=<dir>`: writes each frame read to `<dir>/<setting>/frame-NNNNN.png`, numbered as the
//!   walk's frames, for `sapphire_testkit flashes <dir> --trace` and a look at what made a second's transitions.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test flashes_in_the_ward -- --ignored --nocapture`. About 3 minutes a setting
//! on an M5's Metal.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_flow::flow::{FlowCommand, FlowState, Run};
use game_flow::game::GameSetup;
use game_ui::menus::{Flashes, PlayerSettings};
use sapphire_gameplay::{EncounterPhase, Simulation};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, Platform, RunOptions, Timestamp, run_with,
};
use sapphire_render::quality::{QualitySettings, TierId};
use sapphire_rhi::wgpu;
use sapphire_stage::{Readback, Stage};
use sapphire_testkit::flashes::{FlashAnalyser, FlashReport, MOST_TRANSITIONS, Traced};
use sapphire_testkit::golden::Image;

const WIDTH: u32 = 480;
const HEIGHT: u32 = 270;
/// Frames a second: a frame a step.
const RATE: u32 = 60;
/// The replay, from the repository's root.
const WALK: &str = "tests/replays/ward-whole-walk.sreplay";
/// Longer than the walk: fifteen minutes of play, the cutscene's held frames among them.
const MOST_STEPS: u32 = 15 * 60 * 60;
/// The walk's frames, from the first after the title's new game to the victory's: what the parts split. The
/// whole walk, or its last part, fails naming the new count when the replay changes it.
const WALK_FRAMES: u32 = 14_823;
/// The ward's encounters.
const ENCOUNTERS: usize = 4;

/// Which of the ward's fights an encounter started at `position`, its trigger's (`lantern_ward.zone.ron`), is:
/// the market's gate is east of the hub, the garden's west, the court's south and the terrace's steps past it.
fn fight_at([x, _, z]: [f32; 3]) -> &'static str {
    if x > 20.0 {
        "the market's fight"
    } else if x < -20.0 {
        "the garden's fight"
    } else if z > -80.0 {
        "the court's fight"
    } else {
        "the boss on the terrace"
    }
}

/// The settings `SAPPHIRE_FLASHES` names, or both.
fn settings() -> Result<Vec<Flashes>, String> {
    let Ok(names) = std::env::var("SAPPHIRE_FLASHES") else {
        return Ok(vec![Flashes::Normal, Flashes::Reduced]);
    };
    let mut chosen = Vec::new();
    for name in names.split(',').map(str::trim) {
        let flashes = match name {
            "normal" | "default" => Flashes::Normal,
            "reduced" => Flashes::Reduced,
            _ => return Err(format!("SAPPHIRE_FLASHES: {name:?} isn't normal or reduced")),
        };
        if !chosen.contains(&flashes) {
            chosen.push(flashes);
        }
    }
    Ok(chosen)
}

/// Frames read before the first second judged, the first of them drawing the picture's history.
const LEAD: u32 = 2 * RATE;

/// The most red transitions a second may hold: none, the project's bar, as A3 set it for the arena's fights,
/// which WCAG's three red flashes a second leave room for. A7 held the walk to one: in the court an enemy's
/// red-tasselled lance swept across the view, its holder half a metre from the eye, as an ultimate's
/// cinematic gave the view back (frames 11,829 to 11,832), and on the terrace the boss's red cloth moved
/// under the eye a metre from its armour (frame 14,476). Fighters near the eye fade now, and what they hold
/// with them (M4 C7, `game_combat::fight::FIGHTER_FADE`), and the walk has none.
const MOST_RED: u32 = 0;

/// The frames `SAPPHIRE_FLASH_PART` judges the seconds ending on, `from..to` (`to` past the walk for the last
/// part), or the whole walk.
fn part() -> Result<(u32, u32), String> {
    let Ok(raw) = std::env::var("SAPPHIRE_FLASH_PART") else {
        return Ok((0, u32::MAX));
    };
    if let Some((from, to)) = raw.split_once("..") {
        return match (from.trim().parse::<u32>(), to.trim().parse::<u32>()) {
            (Ok(from), Ok(to)) if from + RATE <= to => Ok((from, to)),
            _ => Err(format!("SAPPHIRE_FLASH_PART: {raw:?} isn't <from>..<to>, a second or more")),
        };
    }
    let parsed = raw
        .split_once('/')
        .and_then(|(i, n)| Some((i.trim().parse::<u32>().ok()?, n.trim().parse::<u32>().ok()?)));
    match parsed {
        Some((i, n)) if (1..=n).contains(&i) => {
            let from = (i - 1) * WALK_FRAMES / n;
            let to = if i == n { u32::MAX } else { i * WALK_FRAMES / n };
            Ok((from, to))
        }
        _ => Err(format!("SAPPHIRE_FLASH_PART: {raw:?} isn't <i>/<n> with 1 <= i <= n, or <from>..<to>")),
    }
}

fn state(stage: &Stage<GameShow>) -> FlowState {
    stage.show.game().expect("the content names a flow").flow().state()
}

fn simulation(stage: &Stage<GameShow>) -> &Simulation {
    stage.show.shown.fight.combat.as_ref().expect("a run stands").simulation()
}

/// A stretch of the walk worth its own line: a fight, a cutscene or a cinematic, by frames.
struct Stretch {
    name: String,
    from: u32,
    to: Option<u32>,
}

/// Follows each stretch as the walk goes.
#[derive(Default)]
struct Stretches {
    all: Vec<Stretch>,
    encounters: [Option<usize>; ENCOUNTERS],
    cutscene: Option<usize>,
    cinematic: Option<usize>,
    cinematics: u32,
}

impl Stretches {
    fn open(&mut self, name: String, frame: u32) -> usize {
        self.all.push(Stretch { name, from: frame, to: None });
        self.all.len() - 1
    }

    fn close(&mut self, open: Option<usize>, frame: u32) {
        if let Some(index) = open {
            self.all[index].to.get_or_insert(frame);
        }
    }

    /// What `frame`, just played, shows.
    fn follow(&mut self, stage: &Stage<GameShow>, frame: u32) {
        let sim = simulation(stage);
        if let Some(zone) = sim.zone() {
            assert_eq!(zone.encounters().len(), ENCOUNTERS, "the ward's encounters");
            for (index, held) in zone.encounters().iter().enumerate() {
                if held.phase() != EncounterPhase::Waiting && self.encounters[index].is_none() {
                    let at = zone.start_volume(index).expect("a trigger starts it").position;
                    let opened = self.open(fight_at(at).to_owned(), frame);
                    self.encounters[index] = Some(opened);
                }
                if held.phase() == EncounterPhase::Cleared {
                    self.close(self.encounters[index], frame);
                }
            }
        }
        let cutscene = matches!(state(stage), FlowState::Cutscene { .. });
        match (cutscene, self.cutscene) {
            (true, None) => {
                let opened = self.open("the gate's cutscene".to_owned(), frame);
                self.cutscene = Some(opened);
            }
            (false, Some(_)) => {
                let open = self.cutscene.take();
                self.close(open, frame);
            }
            _ => {}
        }
        let cinematic = sim.timeline().is_some_and(|playing| playing.owner.is_some());
        match (cinematic, self.cinematic) {
            (true, None) => {
                self.cinematics += 1;
                let opened = self.open(format!("ultimate {}'s cinematic", self.cinematics), frame);
                self.cinematic = Some(opened);
            }
            (false, Some(_)) => {
                let open = self.cinematic.take();
                self.close(open, frame);
            }
            _ => {}
        }
    }
}

/// What one setting's walk came to.
struct Walked {
    report: FlashReport,
    /// The first frame read, and the frames the seconds judged end on.
    from: u32,
    judged: (u32, u32),
    /// Frames read.
    frames: u32,
    /// The walk's frames, if it was played to the victory.
    walk: Option<u32>,
    /// Each frame's simulation tick, from `from`; the traces too.
    ticks: Vec<u64>,
    general: Vec<Traced>,
    red: Vec<Traced>,
    stretches: Vec<Stretch>,
    seconds: f32,
}

/// The walk at `flashes`, judging the seconds that end on frames `judged`.
fn walk(
    platform: &Platform,
    replay: &Path,
    flashes: Flashes,
    judged: (u32, u32),
) -> Result<Walked, AppInitError> {
    let (from, to) = (judged.0.saturating_sub(LEAD), judged.1);
    let setup = GameSetup { replay: Some(replay.to_owned()), ..GameSetup::default() };
    let settings = PlayerSettings { flashes, ..PlayerSettings::default() };
    let (show, first) = GameShow::from_content(platform.content(), settings, setup)?;
    let gpu = sapphire_stage::headless_gpu()?;
    let quality = QualitySettings { tier: Some(TierId::DesktopHigh), ..QualitySettings::default() };
    let desc = game_combat::fight::fight_stage(sapphire_stage::StageDesc::new(
        platform.host(),
        &gpu,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        quality,
    ));
    let mut content = sapphire_stage::Content::new(platform.host(), desc.character, desc.sky)?;
    let mut show = show;
    let combat = show.shown.fight.prepare_stock_combat(&first, &mut content)?;
    let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
    assert_eq!(state(&stage), FlowState::Title { stood: Run::New });
    assert_eq!(
        stage.show.shown.fight.show.effect_scale(),
        settings_scale(flashes),
        "the setting reaches the fight"
    );

    let target = stage.target(WIDTH, HEIGHT, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = Readback::new(&stage.gpu, WIDTH, HEIGHT);
    let mut analyser = FlashAnalyser::new(WIDTH, HEIGHT, RATE).map_err(AppInitError::msg)?;
    let write = std::env::var_os("SAPPHIRE_FLASH_WRITE").map(|dir| PathBuf::from(dir).join(flashes.name()));
    if let Some(dir) = &write {
        std::fs::create_dir_all(dir)?;
    }
    let mut ticks = Vec::new();
    let mut stretches = Stretches::default();
    let started = platform.now();
    let mut clock = 1_000_000_000_u64;
    // The title, then a new game: the frames counted from the first after it.
    let mut play = |stage: &mut Stage<GameShow>, draw: bool| -> Result<Option<Image>, AppInitError> {
        clock += 16_666_667;
        stage.show.update_menus(Timestamp::from_nanos(clock));
        stage.show.follow_input(Timestamp::from_nanos(clock));
        stage.step();
        stage.show.drive_view(Duration::from_millis(16));
        stage.lose_frames = !draw;
        if !draw {
            stage.render(&view, 1.0, |_, _| {})?;
            stage.lose_frames = false;
            stage.check_gpu()?;
            return Ok(None);
        }
        stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
        stage.check_gpu()?;
        let rgba = readback.read(&stage.gpu).map_err(AppInitError::msg)?;
        Ok(Some(Image { width: WIDTH, height: HEIGHT, rgba }))
    };
    play(&mut stage, from == 0)?;
    stage.show.choose(FlowCommand::NewGame);
    assert_eq!(state(&stage), FlowState::Playing);
    let mut frame = 0_u32;
    let mut won = false;
    while frame < to {
        assert!(frame < MOST_STEPS, "never won: {:?} at tick {}", state(&stage), simulation(&stage).tick());
        assert!(!matches!(state(&stage), FlowState::Defeat { .. }), "the player fell");
        // The victory's own frame is the walk's last.
        won = matches!(state(&stage), FlowState::Victory { .. });
        let image = play(&mut stage, frame >= from)?;
        stretches.follow(&stage, frame);
        if let Some(image) = image {
            if let Some(dir) = &write {
                let path = dir.join(format!("frame-{frame:05}.png"));
                image.write_png(&path).map_err(AppInitError::msg)?;
            }
            analyser.push(&image).map_err(AppInitError::msg)?;
            ticks.push(simulation(&stage).tick());
        }
        frame += 1;
        if won {
            break;
        }
    }
    let walk = won.then_some(frame);
    for open in [stretches.cutscene, stretches.cinematic].into_iter().chain(stretches.encounters) {
        stretches.close(open, frame - 1);
    }
    let report = analyser.report(None).map_err(AppInitError::msg)?;
    let [general, red] = analyser.trace();
    if won {
        // The replay's checksums, beside it: the walk on the stage is the walk the file recorded.
        stage.show.finish().map_err(AppInitError::msg)?;
    }
    Ok(Walked {
        report,
        from,
        judged,
        frames: u32::try_from(ticks.len()).expect("fewer frames than MOST_STEPS"),
        walk,
        ticks,
        general: general.to_vec(),
        red: red.to_vec(),
        stretches: stretches.all,
        seconds: platform.now().saturating_duration_since(started).as_secs_f32(),
    })
}

fn settings_scale(flashes: Flashes) -> sapphire_render::EffectScale {
    PlayerSettings { flashes, ..PlayerSettings::default() }.effect_scale()
}

/// The worst second ending in `from..=to` of the walk's frames, of those judged: its transitions, and the frame
/// and tick it ends on.
fn worst(walked: &Walked, trace: &[Traced], (from, to): (u32, u32)) -> Option<(u32, u32, u64)> {
    let first = from.max(walked.judged.0);
    let last = to.min(walked.judged.1.saturating_sub(1)).min(walked.from + walked.frames.checked_sub(1)?);
    (first <= last).then(|| {
        (first..=last)
            .map(|frame| {
                let at = (frame - walked.from) as usize;
                (trace[at].most, frame, walked.ticks[at])
            })
            .max_by_key(|&(most, frame, _)| (most, std::cmp::Reverse(frame)))
            .expect("a frame")
    })
}

/// Prints the walk's scores and each stretch's, and says whether they pass.
#[allow(clippy::print_stdout, reason = "the scores are exit test 6's numbers, read with --nocapture")]
#[allow(clippy::absurd_extreme_comparisons, reason = "`MOST_RED` is 0, the bar, and stays a bound to read")]
fn judge(flashes: Flashes, walked: &Walked) -> bool {
    let name = flashes.name();
    let report = &walked.report;
    let end = walked.judged.1.min(walked.from + walked.frames);
    println!(
        "flashes {name}: the seconds ending on the walk's frames {}..{end} ({} frames read{}), in {:.0} s",
        walked.judged.0,
        walked.frames,
        walked.walk.map_or(String::new(), |frames| format!(", the walk {frames}")),
        walked.seconds,
    );
    for stretch in &walked.stretches {
        // A second reaches back before the stretch's end: count each second that ends within a second after it.
        let to = stretch.to.unwrap_or(u32::MAX).saturating_add(RATE - 1);
        let (Some(general), Some(red)) = (
            worst(walked, &walked.general, (stretch.from, to)),
            worst(walked, &walked.red, (stretch.from, to)),
        ) else {
            continue;
        };
        println!(
            "flashes {name}: {}: frames {}..={} ({} frames), worst second {} transitions ending frame {} (tick {}), \
             red {}",
            stretch.name,
            stretch.from,
            stretch.to.map_or("end".to_owned(), |to| to.to_string()),
            stretch.to.map_or(0, |to| to + 1 - stretch.from),
            general.0,
            general.1,
            general.2,
            red.0,
        );
    }
    // Every second that holds no frame of a move's cinematic: the fights' own camera and cues.
    let cinematics: Vec<(u32, u32)> = walked
        .stretches
        .iter()
        .filter(|stretch| stretch.name.contains("cinematic"))
        .map(|stretch| (stretch.from, stretch.to.unwrap_or(u32::MAX).saturating_add(RATE - 1)))
        .collect();
    let outside = |trace: &[Traced]| {
        (walked.judged.0..end)
            .filter(|frame| !cinematics.iter().any(|&(from, to)| (from..=to).contains(frame)))
            .map(|frame| (trace[(frame - walked.from) as usize].most, frame))
            .max_by_key(|&(most, frame)| (most, std::cmp::Reverse(frame)))
    };
    // Where any field made a red transition: each is half a red flash, and none is what this test asks.
    let reds: Vec<String> = (walked.judged.0..end)
        .filter(|frame| walked.red[(frame - walked.from) as usize].transition)
        .map(|frame| frame.to_string())
        .collect();
    if !reds.is_empty() {
        println!("flashes {name}: red transitions on frames {}", reds.join(", "));
    }
    if let (Some(general), Some(red)) = (outside(&walked.general), outside(&walked.red)) {
        println!(
            "flashes {name}: outside the cinematics: worst second {} transitions ending frame {}, red {} ending \
             frame {}",
            general.0, general.1, red.0, red.1
        );
    }
    let (Some(general), Some(red)) =
        (worst(walked, &walked.general, (0, u32::MAX)), worst(walked, &walked.red, (0, u32::MAX)))
    else {
        return false;
    };
    println!(
        "flashes {name}: all judged: general {} a second ending frame {} (tick {}), red {} a second ending frame {} \
         (tick {}); the most of a field at once, of all read: general {:.3}, red {:.3}",
        general.0, general.1, general.2, red.0, red.1, red.2, report.general.area, report.red.area,
    );
    general.0 <= MOST_TRANSITIONS && red.0 <= MOST_RED
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Flashes Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_flashes_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let replay = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(WALK);
        let part = part().map_err(AppInitError::msg)?;
        let mut failed = Vec::new();
        for flashes in settings().map_err(AppInitError::msg)? {
            let walked = walk(platform, &replay, flashes, part)?;
            if !judge(flashes, &walked) {
                failed.push(flashes.name());
            }
            if let Some(frames) = walked.walk {
                assert_eq!(
                    frames, WALK_FRAMES,
                    "the walk's frames: set WALK_FRAMES to it, which the parts split"
                );
            }
        }
        if !failed.is_empty() {
            return Err(AppInitError::msg(format!(
                "WCAG 2.3.1 fails at {}: more than {MOST_TRANSITIONS} transitions in a second, or more than {MOST_RED} red",
                failed.join(" and ")
            )));
        }
        Ok(Self)
    }

    fn event(&mut self, _: &mut Platform, _: &Event) -> AppControl {
        AppControl::Continue
    }

    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        AppControl::Exit
    }
}

fn main() -> ExitCode {
    if !std::env::args().any(|argument| argument == "--ignored") {
        eprintln!("flashes_in_the_ward: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    if let Err(error) = settings().and_then(|_| part()) {
        eprintln!("flashes_in_the_ward: {error}");
        return ExitCode::FAILURE;
    }
    sapphire_core::log::init(Default::default(), None).expect("initialize app test logging");
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<OutsideCaller>(RunOptions { background: true, headless: true })
}
