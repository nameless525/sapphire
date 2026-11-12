//! Everything a run can reach is declared (M4 H3): every renderer a player's device can make, at every rung of
//! its thermal ladder, and every setting a player can step on it, draw with no pipeline the load didn't declare.
//!
//! The walk reads each axis from its document, so a new one is a list read from where it's written:
//!
//! - **Tiers:** every shipped tier (`tiers.ron`, `TierTable::shipped`), each a stand of the game on a renderer
//!   made for it, as a device that gets it makes one.
//! - **Rungs:** each tier's thermal ladder (`tiers.ron`'s `ladder`, M4 T2), held at every depth it takes in place,
//!   from the top down, through `r.rung`, which the stage follows each frame as heat would move it. A ladder's
//!   last rung, a lower tier, takes a new renderer: that tier's own stand walks it.
//! - **Settings:** every row of the game's screens (`menus.ron`) that steps one of `PlayerSettings`, found from
//!   the pause through the rows that open screens, each pressed round its values back to where it began: the
//!   language through every shipped one, the flashes, camera motion, text and interface sizes, subtitles on and
//!   off and every size and band, and the rest. Each value draws a frame of its page.
//! - **Screens:** every screen a row opens, the character screen's 3D inset among them, so insets draw on and off.
//!
//! Tiers, rungs and settings combine: every setting is walked at every rung of every tier, since the post
//! variants the flashes and camera motion choose draw with each tier's effect settings and the bloom chain a
//! rung shortens, and the inset's resolve with each tier's samples. The settings walk each alone over the
//! defaults: what one draws through doesn't change with another's value (the interface is one pipeline, and the
//! screen effects' switches are each set by one share).
//!
//! What's drawn behind the pages is what the settings change: the Lantern Ward's gate cutscene mid-line, so a
//! subtitle shows at each of its settings, under every screen effect the fight's cue table pushes, held, so the
//! flashes and camera motion scale them. After every frame the renderer's ledger is read, and a pipeline a frame
//! built, on first use or on the frame's thread, fails the walk naming the tier, the rung, the setting and its
//! value, and the pipeline.
//!
//! `SAPPHIRE_REACH_TIERS=<key>[,<key>]` walks only the tiers it names; unset, every one.
//!
//! The platform's loop must own the process's main thread, so this test supplies its own `main`
//! (`harness = false`). It needs a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p game_app --features dev --test declares_every_setting -- --ignored`.

mod drive;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use drive::{Draw, Driver};
use game_app::{GameApp, GameShow};
use game_flow::flow::{FlowCommand, FlowState};
use game_flow::game::GameSetup;
use game_flow::progress::{Progress, progress_store};
use game_ui::menus::{PlayerSettings, Setting};
use sapphire_gameplay::{FlagValues, ZoneStart};
use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform, Scancode};
use sapphire_render::quality::{Ladder, LadderState, QualitySettings, RUNG, TierId, TierTable};
use sapphire_rhi::wgpu;
use sapphire_serialize::AssetId;
use sapphire_stage::{Content, Stage, StageDesc, headless_gpu};
use sapphire_time::Stopwatch;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Large enough that every page's rows lay out as a player's do.
const SIZE: (u32, u32) = (480, 270);
/// The ward's zone, and the encounters a save facing the court's gate has fought: the market's and the garden's.
const WARD: &str = "f15b7d04-631d-59b5-8c42-338147fc4e86";
const MARKET: &str = "9c124ff2-ab3d-4b6c-a281-79dc6eade2ec";
const GARDEN: &str = "f2a2a203-7ac5-4a8b-91ec-4366daa62246";
/// Longer than the walk back through the gate and into the warden's line takes.
const MOST_FRAMES: u32 = 900;

fn id(raw: &str) -> AssetId {
    raw.parse().expect("an ID")
}

/// The tiers `SAPPHIRE_REACH_TIERS` names, or every shipped one. A name that isn't a tier is an error, so a
/// misspelt choice can't pass by walking nothing.
fn tiers(table: &TierTable) -> Result<Vec<TierId>, String> {
    let shipped: Vec<TierId> = table.tiers().iter().map(|tier| tier.id).collect();
    let Ok(names) = std::env::var("SAPPHIRE_REACH_TIERS") else {
        return Ok(shipped);
    };
    let mut chosen = Vec::new();
    for name in names.split(',').map(str::trim) {
        match TierId::from_key(name).filter(|tier| shipped.contains(tier)) {
            Some(tier) if !chosen.contains(&tier) => chosen.push(tier),
            Some(_) => {}
            None => {
                let keys: Vec<&str> = shipped.iter().map(|tier| tier.key()).collect();
                return Err(format!(
                    "SAPPHIRE_REACH_TIERS: {name:?} isn't a shipped tier: {}",
                    keys.join(", ")
                ));
            }
        }
    }
    Ok(chosen)
}

/// What a rung of `ladder` at `depth` holds, as a failure names it.
fn rung_name(ladder: &Ladder, depth: usize) -> String {
    let state = LadderState::of(&ladder.rungs()[..depth]);
    format!("rung {depth} (scale {}, shed {})", state.scale, state.shed)
}

/// Each frame drawn into one target and its ledger read after: where the walk is, for a failure to name, and
/// what the frames drew with.
struct Ledger {
    target: wgpu::TextureView,
    tier: TierId,
    rung: String,
    frames: u32,
    /// Frames drawn with screen effects, and with an inset.
    with_effects: u32,
    with_inset: u32,
}

impl Draw for Ledger {
    /// Draws the stage as it stands, then reads the ledger: a pipeline this frame built fails the walk, named.
    fn draw(&mut self, stage: &mut Stage<GameShow>, _: &Stopwatch, what: &str) -> Result<(), AppInitError> {
        stage.render(&self.target, 1.0, |_, _| {})?;
        stage.check_gpu()?;
        self.frames += 1;
        let stats = stage.renderer.stats();
        self.with_effects += u32::from(stats.post.effects.count() > 0);
        self.with_inset += u32::from(stats.insets.shown > 0);
        let built = stats.pipelines;
        if built.first_use_builds > 0 || built.frame_thread_builds > 0 {
            panic!(
                "on {} at {}, {what} drew through pipelines nothing declared ({} first use, {} on the frame's \
                 thread): {}",
                self.tier,
                self.rung,
                built.first_use_builds,
                built.frame_thread_builds,
                stage.renderer.first_use_report()
            );
        }
        Ok(())
    }

    fn place(&self) -> String {
        format!(" on {} at {}", self.tier, self.rung)
    }
}

/// The game on one renderer, a frame at a time as the window plays it, its ledger read after each.
type Walk = Driver<Ledger>;

/// The game's flow over a save facing the court's gate, stood on a renderer made for `tier`, at its title.
fn stand(platform: &Platform, saves: &Path, tier: TierId) -> Result<Walk, AppInitError> {
    let store = progress_store(saves, |path, bytes| std::fs::write(path, bytes));
    let setup = GameSetup { store: Some(store), ..GameSetup::default() };
    let (mut show, first) = GameShow::from_content(platform.content(), PlayerSettings::default(), setup)?;
    let gpu = headless_gpu()?;
    let desc = game_combat::fight::fight_stage(StageDesc::new(
        platform.host(),
        &gpu,
        FORMAT,
        QualitySettings { tier: Some(tier), ..QualitySettings::default() },
    ));
    let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
    let combat = show.shown.fight.prepare_stock_combat(&first, &mut content)?;
    let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    assert_eq!(quality.active().id, tier, "the settings' tier");
    show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
    let target = stage.target(SIZE.0, SIZE.1, wgpu::TextureUsages::empty());
    let target = target.create_view(&wgpu::TextureViewDescriptor::default());
    let ledger =
        Ledger { target, tier, rung: "the top".to_owned(), frames: 0, with_effects: 0, with_inset: 0 };
    let mut walk = Driver::new(stage, ledger);
    walk.what = "the title".to_owned();
    Ok(walk)
}

fn state(walk: &Walk) -> FlowState {
    walk.stage.show.game().expect("the content names a flow").flow().state()
}

/// Continues from the title, walks back through the court's gate into its cutscene, and plays on until a
/// line's up.
fn walk_into_a_line(walk: &mut Walk) -> Result<(), AppInitError> {
    walk.held()?;
    walk.stage.show.choose(FlowCommand::Continue);
    assert_eq!(state(walk), FlowState::Playing);
    walk.what = "the walk into the gate's cutscene".to_owned();
    // A continue stands the player facing the gate, so the stick's forward.
    walk.key(Scancode::W, true);
    let mut frames = 0;
    while walk.stage.show.shown.fight.show.timeline_player().is_none() {
        assert!(frames < MOST_FRAMES, "never into the cutscene: {:?}", state(walk));
        walk.play()?;
        frames += 1;
    }
    walk.key(Scancode::W, false);
    let says = |walk: &Walk| {
        let player = walk.stage.show.shown.fight.show.timeline_player();
        player.and_then(|player| player.subtitle()).is_some()
    };
    while !says(walk) {
        assert!(frames < MOST_FRAMES, "the cutscene never said a line");
        walk.play()?;
        frames += 1;
    }
    assert!(matches!(state(walk), FlowState::Cutscene { .. }), "{:?}", state(walk));
    Ok(())
}

/// Pushes every screen effect the fight's cue table pushes, each held at its strongest, so every frame from
/// here draws them as the player's flashes and camera motion let them. Pushed past the scene's `push_effect`,
/// which would declare them: what declares them is the app's own load.
fn push_the_fight_s_effects(stage: &mut Stage<GameShow>) {
    let cues = stage.show.shown.fight.show.cue_show().expect("the fight fires cues");
    let mut presets: Vec<String> = cues
        .table()
        .rows()
        .iter()
        .filter_map(|row| row.screen.as_ref())
        .map(|fire| fire.preset.clone())
        .collect();
    presets.sort();
    presets.dedup();
    assert!(!presets.is_empty(), "the fight's cues push screen effects");
    stage.scene.with_screen_effects(|stack, library| {
        for name in &presets {
            let preset =
                library.find(name).and_then(|id| library.preset(id)).expect("the cue table's preset");
            let strongest = (0..120)
                .map(|frame| frame as f32 / 60.0)
                .max_by(|a, b| {
                    let weight = |time: f32| {
                        let layers = preset.layers.iter();
                        layers.map(|layer| layer.envelope.weight(time - layer.delay, None)).sum::<f32>()
                    };
                    weight(*a).total_cmp(&weight(*b))
                })
                .expect("times");
            let push = stack.push(preset);
            stack.hold(push, Some(strongest));
        }
    });
}

/// A directory of its own for the `n`th run's save, holding one facing the court's gate: each run continues
/// from it as it was, whatever the run before saved.
fn saves(n: usize) -> Result<PathBuf, AppInitError> {
    let dir =
        std::env::temp_dir().join(format!("game_app-declares-every-setting-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut cleared = vec![id(MARKET), id(GARDEN)];
    cleared.sort_unstable();
    let saved = Progress {
        zone: id(WARD),
        start: ZoneStart { checkpoint: Some("court".to_owned()), cleared },
        flags: FlagValues::default(),
    };
    let store = progress_store(&dir, |path, bytes| std::fs::write(path, bytes));
    store.write(&store.snapshot(&saved).map_err(AppInitError::msg)?).map_err(AppInitError::msg)?;
    Ok(dir)
}

/// The game on `tier`: into the gate's line with every effect held, then at the pause, at each rung its ladder
/// takes in place, every screen and setting. Returns the walk and, for each rung, what it stepped and opened.
fn walk(platform: &Platform, saves: &Path, tier: TierId) -> Result<(Walk, Vec<Reached>), AppInitError> {
    RUNG.set(-1).map_err(AppInitError::msg)?;
    let mut walk = stand(platform, saves, tier)?;
    walk_into_a_line(&mut walk)?;
    push_the_fight_s_effects(&mut walk.stage);
    walk.stage.show.controls.set_paused(true);
    walk.what = "the pause".to_owned();
    walk.settle("the pause", |walk| walk.is_open("pause"))?;
    let ladder = walk.stage.quality.ladder().clone();
    let mut reached = Vec::new();
    for depth in 0..=ladder.in_place_depth() {
        walk.draw.rung = rung_name(&ladder, depth);
        walk.what = "the pause, at the rung".to_owned();
        RUNG.set(depth as i32).map_err(AppInitError::msg)?;
        walk.held()?;
        let held = LadderState::of(&ladder.rungs()[..depth]);
        assert_eq!(walk.stage.renderer.shed(), held.shed, "{tier} at {}", walk.draw.rung);
        walk.stepped.clear();
        walk.opened.clear();
        walk.walk_screen("pause")?;
        let (stepped, opened) = (walk.stepped.clone(), walk.opened.clone());
        reached.push(Reached { rung: walk.draw.rung.clone(), stepped, opened });
    }
    // And back up to the top, as a ladder climbs.
    walk.draw.rung = "the top again".to_owned();
    RUNG.set(0).map_err(AppInitError::msg)?;
    walk.held()?;
    RUNG.set(-1).map_err(AppInitError::msg)?;
    let player = walk.stage.show.shown.fight.show.timeline_player();
    assert!(player.and_then(|player| player.subtitle()).is_some(), "the line stayed up under the pages");
    Ok((walk, reached))
}

/// What a walk at one rung pressed and opened.
struct Reached {
    rung: String,
    stepped: Vec<Setting>,
    opened: Vec<String>,
}

/// Does it all in `init`, and the first frame exits.
struct OutsideCaller;

impl App for OutsideCaller {
    const METADATA: AppMetadata = AppMetadata {
        name: "Game Declares Every Setting Test",
        organization: "Sapphire",
        identifier: "dev.sapphire.game_declares_every_setting_test",
        version: GameApp::METADATA.version,
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let screens = game_ui::menus::MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
        for (n, tier) in tiers(&TierTable::shipped()).map_err(AppInitError::msg)?.into_iter().enumerate() {
            let started = platform.now();
            let saves = saves(n)?;
            let (mut walk, reached) = walk(platform, &saves, tier)?;
            let seconds = platform.now().saturating_duration_since(started).as_secs_f32();
            assert!(walk.draw.with_effects > 0, "the fight's screen effects drew on {tier}");
            assert!(walk.draw.with_inset > 0, "the character screen's inset drew on {tier}");
            // Every row the documents have that steps a setting, and every screen a row opens, was reached at
            // every rung: a new one the pause can't get to fails here rather than going unwalked.
            for Reached { rung, stepped, opened } in &reached {
                drive::walked_every_row(&screens, stepped, opened, &format!("on {tier} at {rung}"));
            }
            eprintln!(
                "declares_every_setting: {tier}: {} rungs, each with {} settings stepped round and {} screens \
                 opened; {} frames ({} with screen effects, {} with an inset) in {seconds:.1} s; nothing built \
                 past the load",
                reached.len(),
                reached[0].stepped.len(),
                reached[0].opened.len(),
                walk.draw.frames,
                walk.draw.with_effects,
                walk.draw.with_inset,
            );
            // The run's save writes, waited for before its directory goes.
            walk.stage.show.finish().map_err(AppInitError::msg)?;
            drop(walk);
            std::fs::remove_dir_all(&saves)?;
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
    drive::main::<OutsideCaller>("declares_every_setting")
}
