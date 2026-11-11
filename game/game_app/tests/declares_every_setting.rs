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

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use game_app::{GameApp, GameShow};
use game_flow::flow::{FlowCommand, FlowState};
use game_flow::game::GameSetup;
use game_flow::progress::{Progress, progress_store};
use game_ui::menus::{MenuScreen, PlayerSettings, RowAction, Setting};
use sapphire_gameplay::{FlagValues, ZoneStart};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, KeyEvent, Keycode, Modifiers, Platform, RunOptions,
    Scancode, Timestamp, WindowId, run_with,
};
use sapphire_render::quality::{Ladder, LadderState, QualitySettings, RUNG, TierId, TierTable};
use sapphire_rhi::wgpu;
use sapphire_serialize::AssetId;
use sapphire_stage::{Content, Stage, StageDesc, headless_gpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Large enough that every page's rows lay out as a player's do.
const SIZE: (u32, u32) = (480, 270);
const FRAME: u64 = 16_666_667;
/// The ward's zone, and the encounters a save facing the court's gate has fought: the market's and the garden's.
const WARD: &str = "f15b7d04-631d-59b5-8c42-338147fc4e86";
const MARKET: &str = "9c124ff2-ab3d-4b6c-a281-79dc6eade2ec";
const GARDEN: &str = "f2a2a203-7ac5-4a8b-91ec-4366daa62246";
/// Longer than the walk back through the gate and into the warden's line takes.
const MOST_FRAMES: u32 = 900;
/// More values than any setting has.
const MOST_VALUES: u32 = 32;
/// Longer than any screen's way in or out.
const SETTLE_FRAMES: u32 = 60;

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

/// What a frame is drawn for: what the walk is at, for a failure to name.
struct At {
    tier: TierId,
    rung: String,
    what: String,
}

/// The game on one renderer, a frame at a time as the window plays it, its ledger read after each.
struct Walk {
    stage: Stage<GameShow>,
    target: wgpu::TextureView,
    clock: u64,
    at: At,
    frames: u32,
    /// Every setting a row stepped, and every screen a row opened.
    stepped: Vec<Setting>,
    opened: Vec<String>,
    /// Frames drawn with screen effects, and with an inset.
    with_effects: u32,
    with_inset: u32,
}

impl Walk {
    /// The game's flow over a save facing the court's gate, stood on a renderer made for `tier`, at its title.
    fn stand(platform: &Platform, saves: &Path, tier: TierId) -> Result<Self, AppInitError> {
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
        Ok(Self {
            stage,
            target,
            clock: 1_000_000_000,
            at: At { tier, rung: "the top".to_owned(), what: "the title".to_owned() },
            frames: 0,
            stepped: Vec::new(),
            opened: Vec::new(),
            with_effects: 0,
            with_inset: 0,
        })
    }

    fn state(&self) -> FlowState {
        self.stage.show.game().expect("the content names a flow").flow().state()
    }

    fn settings(&self) -> &PlayerSettings {
        self.stage.show.menus().expect("the menus stand").settings()
    }

    /// A key, down or up, as the window sends it, just after the last frame.
    fn key(&mut self, scancode: Scancode, down: bool) {
        self.stage.show.handle_event(&Event::Key(KeyEvent {
            timestamp: Timestamp::from_nanos(self.clock + 1),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat: false,
        }));
    }

    /// The frame's input, as the window reads it.
    fn input(&mut self) {
        self.clock += FRAME;
        let now = Timestamp::from_nanos(self.clock);
        self.stage.show.update_menus(now);
        self.stage.show.follow_input(now);
    }

    /// Draws the stage as it stands, then reads the ledger: a pipeline this frame built fails the walk, named.
    fn draw(&mut self) -> Result<(), AppInitError> {
        let stage = &mut self.stage;
        stage.render(&self.target, 1.0, |_, _| {})?;
        stage.check_gpu()?;
        self.frames += 1;
        let stats = stage.renderer.stats();
        self.with_effects += u32::from(stats.post.effects.count() > 0);
        self.with_inset += u32::from(stats.insets.shown > 0);
        let built = stats.pipelines;
        if built.first_use_builds > 0 || built.frame_thread_builds > 0 {
            let At { tier, rung, what } = &self.at;
            panic!(
                "on {tier} at {rung}, {what} drew through pipelines nothing declared ({} first use, {} on the frame's \
                 thread): {}",
                built.first_use_builds,
                built.frame_thread_builds,
                stage.renderer.first_use_report()
            );
        }
        Ok(())
    }

    /// A frame of play: its input, a step, the view and a picture.
    fn play(&mut self) -> Result<(), AppInitError> {
        self.input();
        self.stage.step();
        self.stage.show.drive_view(Duration::from_nanos(FRAME));
        self.draw()
    }

    /// A frame with the run held at the pause: its input and a picture, and no step or view, so the cutscene's
    /// playhead stays on its line.
    fn held(&mut self) -> Result<(), AppInitError> {
        self.input();
        self.draw()
    }

    /// A key pressed and let go across two frames' input, then a picture.
    fn press(&mut self, scancode: Scancode) -> Result<(), AppInitError> {
        self.key(scancode, true);
        self.input();
        self.key(scancode, false);
        self.held()
    }

    /// Continues from the title, walks back through the court's gate into its cutscene, and plays on until a
    /// line's up.
    fn walk_into_a_line(&mut self) -> Result<(), AppInitError> {
        self.held()?;
        self.stage.show.choose(FlowCommand::Continue);
        assert_eq!(self.state(), FlowState::Playing);
        self.at.what = "the walk into the gate's cutscene".to_owned();
        // A continue stands the player facing the gate, so the stick's forward.
        self.key(Scancode::W, true);
        let mut frames = 0;
        while self.stage.show.shown.fight.show.timeline_player().is_none() {
            assert!(frames < MOST_FRAMES, "never into the cutscene: {:?}", self.state());
            self.play()?;
            frames += 1;
        }
        self.key(Scancode::W, false);
        let says = |walk: &Self| {
            let player = walk.stage.show.shown.fight.show.timeline_player();
            player.and_then(|player| player.subtitle()).is_some()
        };
        while !says(self) {
            assert!(frames < MOST_FRAMES, "the cutscene never said a line");
            self.play()?;
            frames += 1;
        }
        assert!(matches!(self.state(), FlowState::Cutscene { .. }), "{:?}", self.state());
        Ok(())
    }

    /// Pushes every screen effect the fight's cue table pushes, each held at its strongest, so every frame from
    /// here draws them as the player's flashes and camera motion let them. Pushed past the scene's
    /// `push_effect`, which would declare them: what declares them is the app's own load.
    fn push_the_fight_s_effects(&mut self) {
        let cues = self.stage.show.shown.fight.show.cue_show().expect("the fight fires cues");
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
        self.stage.scene.with_screen_effects(|stack, library| {
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

    /// Whether `screen` is on top and open to presses.
    fn is_open(&self, screen: &str) -> bool {
        let menus = self.stage.show.menus().expect("the menus stand");
        let screen = MenuScreen::named(screen);
        menus.is_open(screen) && menus.tree(screen).and_then(|tree| tree.state()) == Some("open")
    }

    /// Draws held frames until `done`, or fails naming `what` and where each screen stands.
    fn settle(&mut self, what: &str, done: impl Fn(&Self) -> bool) -> Result<(), AppInitError> {
        let mut frames = 0;
        while !done(self) {
            if frames >= SETTLE_FRAMES {
                let menus = self.stage.show.menus().expect("the menus stand");
                let screens: Vec<String> = menus
                    .screens()
                    .screens
                    .iter()
                    .filter_map(|entry| {
                        let screen = MenuScreen::named(&entry.name);
                        let state = menus.tree(screen)?.state().unwrap_or("none");
                        Some(format!("{} {state} (open: {})", entry.name, menus.is_open(screen)))
                    })
                    .collect();
                panic!(
                    "{what} never settled on {} at {}: {}",
                    self.at.tier,
                    self.at.rung,
                    screens.join(", ")
                );
            }
            self.held()?;
            frames += 1;
        }
        Ok(())
    }

    /// Moves focus on `screen` to `node` by the arrows, as a keyboard does.
    fn focus(&mut self, screen: &str, node: &str) -> Result<(), AppInitError> {
        for _ in 0..24 {
            let menus = self.stage.show.menus().expect("the menus stand");
            let tree = menus.tree(MenuScreen::named(screen)).expect("the screen's open");
            if tree.focus_name() == Some(node) {
                return Ok(());
            }
            let target = tree.screen_rect(tree.find(node).expect("the document's node"));
            let from = tree.screen_rect(tree.focus().expect("something has focus")).center();
            let delta = target.center() - from;
            // On another row, the way is up or down, however far apart a right-aligned column's centers are.
            let rows_apart = from.y < target.min.y || from.y > target.max.y;
            let key = if rows_apart || delta.y.abs() >= delta.x.abs() {
                if delta.y > 0.0 { Scancode::DOWN } else { Scancode::UP }
            } else if delta.x > 0.0 {
                Scancode::RIGHT
            } else {
                Scancode::LEFT
            };
            self.press(key)?;
        }
        panic!("{node} can't be reached on {screen} by the keyboard");
    }

    /// Walks `screen`, open on top: each row that steps a setting pressed round its values, a frame each, and
    /// each that opens a screen into it, walked, and back.
    fn walk_screen(&mut self, screen: &str) -> Result<(), AppInitError> {
        let menus = self.stage.show.menus().expect("the menus stand");
        let entry =
            menus.screens().screens.iter().find(|entry| entry.name == screen).expect("the document's");
        let rows = entry.rows.clone();
        for row in rows {
            match row.does {
                RowAction::Step(setting) => self.step_round(screen, &row.node, setting)?,
                RowAction::Open(opened) => {
                    if self.opened.contains(&opened) {
                        continue;
                    }
                    self.opened.push(opened.clone());
                    self.focus(screen, &row.node)?;
                    self.at.what = format!("the {opened} screen");
                    self.press(Scancode::RETURN)?;
                    self.settle(&format!("opening {opened}"), |walk| walk.is_open(&opened))?;
                    self.walk_screen(&opened)?;
                    self.at.what = format!("the {opened} screen, closing");
                    self.settle(&format!("{opened}, before it closes"), |walk| walk.is_open(&opened))?;
                    self.press(Scancode::ESCAPE)?;
                    // Gone, not just leaving, so that `screen` is on top again and takes the next press.
                    self.settle(&format!("closing {opened}"), |walk| {
                        let menus = walk.stage.show.menus().expect("the menus stand");
                        menus.tree(MenuScreen::named(&opened)).is_none() && walk.is_open(screen)
                    })?;
                }
                RowAction::Command(_) | RowAction::Nothing => {}
            }
        }
        Ok(())
    }

    /// Presses `node` on `screen`, which steps `setting`, until it's back where it began: a frame each value.
    fn step_round(&mut self, screen: &str, node: &str, setting: Setting) -> Result<(), AppInitError> {
        self.focus(screen, node)?;
        let began = self.settings().clone();
        let mut values = 0;
        loop {
            assert!(values < MOST_VALUES, "{setting:?} never came back round");
            // A value that remakes the screens (the language) opens them again.
            self.settle(&format!("{screen}, for {setting:?}"), |walk| walk.is_open(screen))?;
            self.key(Scancode::RETURN, true);
            self.input();
            self.key(Scancode::RETURN, false);
            self.at.what = format!("{setting:?} at {}", value_of(self.settings(), setting));
            self.held()?;
            values += 1;
            if *self.settings() == began {
                break;
            }
        }
        assert!(values > 1, "{setting:?} has more than one value");
        self.stepped.push(setting);
        Ok(())
    }
}

/// What `setting` is set to, as a failure names it.
fn value_of(settings: &PlayerSettings, setting: Setting) -> String {
    match setting {
        Setting::Language => settings.language.clone(),
        Setting::Size => settings.size.clone(),
        Setting::TextSize => settings.text_size.name().to_owned(),
        Setting::Shake => format!("{}%", settings.shake),
        Setting::Flashes => settings.flashes.name().to_owned(),
        Setting::CameraMotion => format!("{}%", settings.camera_motion),
        Setting::Sound => format!("{}%", settings.sound),
        Setting::Music => format!("{}%", settings.music),
        Setting::Effects => format!("{}%", settings.effects),
        Setting::Voice => format!("{}%", settings.voice),
        Setting::Mono => settings.mono.to_string(),
        Setting::Rumble => settings.rumble.to_string(),
        Setting::Subtitles => settings.subtitles.shown.to_string(),
        Setting::SubtitleSize => settings.subtitles.size.name().to_owned(),
        Setting::SubtitleBackground => settings.subtitles.background.name().to_owned(),
        Setting::Speakers => settings.subtitles.speakers.to_string(),
        Setting::AutoTarget => format!("{:?}", settings.auto_target),
        Setting::Hold(action) => format!("{:?} {:?}", action, settings.holds.mode(action)),
        Setting::Quality => format!("{:?}", settings.quality),
        Setting::FrameCap => format!("{:?}", settings.frame_cap),
        Setting::DynamicResolution => settings.dynamic_resolution.to_string(),
        Setting::BatterySaver => settings.battery_saver.to_string(),
    }
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
    let mut walk = Walk::stand(platform, saves, tier)?;
    walk.walk_into_a_line()?;
    walk.push_the_fight_s_effects();
    walk.stage.show.controls.set_paused(true);
    walk.at.what = "the pause".to_owned();
    walk.settle("the pause", |walk| walk.is_open("pause"))?;
    let ladder = walk.stage.quality.ladder().clone();
    let mut reached = Vec::new();
    for depth in 0..=ladder.in_place_depth() {
        walk.at.rung = rung_name(&ladder, depth);
        walk.at.what = "the pause, at the rung".to_owned();
        RUNG.set(depth as i32).map_err(AppInitError::msg)?;
        walk.held()?;
        let held = LadderState::of(&ladder.rungs()[..depth]);
        assert_eq!(walk.stage.renderer.shed(), held.shed, "{tier} at {}", walk.at.rung);
        walk.stepped.clear();
        walk.opened.clear();
        walk.walk_screen("pause")?;
        let (stepped, opened) = (walk.stepped.clone(), walk.opened.clone());
        reached.push(Reached { rung: walk.at.rung.clone(), stepped, opened });
    }
    // And back up to the top, as a ladder climbs.
    walk.at.rung = "the top again".to_owned();
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
            assert!(walk.with_effects > 0, "the fight's screen effects drew on {tier}");
            assert!(walk.with_inset > 0, "the character screen's inset drew on {tier}");
            // Every row the documents have that steps a setting, and every screen a row opens, was reached at
            // every rung: a new one the pause can't get to fails here rather than going unwalked.
            for Reached { rung, stepped, opened } in &reached {
                for entry in &screens.screens {
                    for row in &entry.rows {
                        match &row.does {
                            RowAction::Step(setting) => assert!(
                                stepped.contains(setting),
                                "{setting:?}, on {}, was never stepped on {tier} at {rung}",
                                entry.name
                            ),
                            RowAction::Open(screen) => assert!(
                                opened.contains(screen),
                                "{screen}, from {}, was never opened on {tier} at {rung}",
                                entry.name
                            ),
                            RowAction::Command(_) | RowAction::Nothing => {}
                        }
                    }
                }
            }
            eprintln!(
                "declares_every_setting: {tier}: {} rungs, each with {} settings stepped round and {} screens \
                 opened; {} frames ({} with screen effects, {} with an inset) in {seconds:.1} s; nothing built \
                 past the load",
                reached.len(),
                reached[0].stepped.len(),
                reached[0].opened.len(),
                walk.frames,
                walk.with_effects,
                walk.with_inset,
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
    if !std::env::args().any(|argument| argument == "--ignored") {
        eprintln!("declares_every_setting: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    sapphire_core::log::init(Default::default(), None).expect("initialize app test logging");
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<OutsideCaller>(RunOptions { background: true, headless: true })
}
