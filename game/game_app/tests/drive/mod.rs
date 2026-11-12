//! What the game's lanes drive it with (M4 H3, H4): a frame of the game as its window plays one, with no window;
//! the keyboard as a player presses it; and the pause's screens walked by their rows, read from `menus.ron`, each
//! row that steps a setting pressed round all its values and each that opens a screen walked into and back.
//!
//! A lane says how a frame is drawn and what it checks ([`Draw`]); the driver says what the frame was for
//! ([`Driver::what`]), so a failure names where the walk was.

#![allow(dead_code, reason = "each lane uses part of this")]

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use game_app::GameShow;
use game_ui::menus::{MenuScreen, PlayerSettings, RowAction, Setting};
use sapphire_platform::{
    App, AppInitError, Event, KeyEvent, Keycode, Modifiers, RunOptions, Scancode, Timestamp, WindowId,
    run_with,
};
use sapphire_stage::Stage;
use sapphire_time::Stopwatch;

/// A frame's length on the game's clock: the window's 60 Hz.
pub const FRAME: u64 = 16_666_667;
/// More values than any setting has.
const MOST_VALUES: u32 = 32;
/// Longer than any screen's way in or out.
const SETTLE_FRAMES: u32 = 60;

/// How a lane draws a frame of the stage, and what it checks after.
pub trait Draw {
    /// Draws `stage`'s frame as it stands, `began` when the frame's input was read, for `what` (where the walk
    /// is, for a failure to name).
    fn draw(
        &mut self,
        stage: &mut Stage<GameShow>,
        began: &Stopwatch,
        what: &str,
    ) -> Result<(), AppInitError>;

    /// Where the walk is beyond `what`, for a failure to name: a tier, a rung.
    fn place(&self) -> String {
        String::new()
    }
}

/// The game on one stage, a frame at a time as the window plays it.
pub struct Driver<D> {
    pub stage: Stage<GameShow>,
    pub draw: D,
    /// The game's clock, in nanoseconds.
    pub clock: u64,
    /// What the frames are for.
    pub what: String,
    /// Every setting a row stepped, and every screen a row opened, since these were last cleared.
    pub stepped: Vec<Setting>,
    pub opened: Vec<String>,
    /// When this frame's input was read.
    began: Stopwatch,
}

impl<D: Draw> Driver<D> {
    pub fn new(stage: Stage<GameShow>, draw: D) -> Self {
        Self {
            stage,
            draw,
            clock: 1_000_000_000,
            what: String::new(),
            stepped: Vec::new(),
            opened: Vec::new(),
            began: Stopwatch::start(),
        }
    }

    pub fn settings(&self) -> &PlayerSettings {
        self.stage.show.menus().expect("the menus stand").settings()
    }

    /// A key, down or up, as the window sends it, just after the last frame.
    pub fn key(&mut self, scancode: Scancode, down: bool) {
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
    pub fn input(&mut self) {
        self.began = Stopwatch::start();
        self.clock += FRAME;
        let now = Timestamp::from_nanos(self.clock);
        self.stage.show.update_menus(now);
        self.stage.show.follow_input(now);
    }

    fn draw(&mut self) -> Result<(), AppInitError> {
        self.draw.draw(&mut self.stage, &self.began, &self.what)
    }

    /// A frame of play: its input, a step, the view and a picture.
    pub fn play(&mut self) -> Result<(), AppInitError> {
        self.input();
        self.stage.step();
        self.stage.show.drive_view(Duration::from_nanos(FRAME));
        self.draw()
    }

    /// A frame with the run held: its input and a picture, and no step or view, so a cutscene's playhead stays
    /// where it is.
    pub fn held(&mut self) -> Result<(), AppInitError> {
        self.input();
        self.draw()
    }

    /// A key pressed and let go across two frames' input, then a picture.
    pub fn press(&mut self, scancode: Scancode) -> Result<(), AppInitError> {
        self.key(scancode, true);
        self.input();
        self.key(scancode, false);
        self.held()
    }

    /// Whether `screen` is on top and open to presses.
    pub fn is_open(&self, screen: &str) -> bool {
        let menus = self.stage.show.menus().expect("the menus stand");
        let screen = MenuScreen::named(screen);
        menus.is_open(screen) && menus.tree(screen).and_then(|tree| tree.state()) == Some("open")
    }

    /// Draws held frames until `done`, or fails naming `what` and where each screen stands.
    pub fn settle(&mut self, what: &str, done: impl Fn(&Self) -> bool) -> Result<(), AppInitError> {
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
                panic!("{what} never settled{}: {}", self.draw.place(), screens.join(", "));
            }
            self.held()?;
            frames += 1;
        }
        Ok(())
    }

    /// Moves focus on `screen` to `node` by the arrows, as a keyboard does.
    pub fn focus(&mut self, screen: &str, node: &str) -> Result<(), AppInitError> {
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
    /// each that opens a screen into it, walked, and back. A screen already opened since [`Self::opened`] was
    /// cleared isn't walked again.
    pub fn walk_screen(&mut self, screen: &str) -> Result<(), AppInitError> {
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
                    self.what = format!("the {opened} screen");
                    self.press(Scancode::RETURN)?;
                    self.settle(&format!("opening {opened}"), |walk| walk.is_open(&opened))?;
                    self.walk_screen(&opened)?;
                    self.what = format!("the {opened} screen, closing");
                    self.settle(&format!("{opened}, before it closes"), |walk| walk.is_open(&opened))?;
                    self.press(Scancode::ESCAPE)?;
                    // Gone, not just leaving, so that `screen` is on top again and takes the next press.
                    self.settle(&format!("closing {opened}"), |walk| {
                        let menus = walk.stage.show.menus().expect("the menus stand");
                        menus.tree(MenuScreen::named(&opened)).is_none() && walk.is_open(screen)
                    })?;
                }
                // A tab and remapping (M4 A4) change bindings, not what a frame declares: `game_ui`'s remap
                // tests walk them.
                RowAction::Switch(_)
                | RowAction::Remap(_)
                | RowAction::Swap
                | RowAction::Keep
                | RowAction::Reset(_)
                | RowAction::Command(_)
                | RowAction::Nothing => {}
            }
        }
        Ok(())
    }

    /// From `from`, open on top, opens screens by their rows as a player would until the one whose row steps
    /// `setting` is on top. Returns that screen and row, and the screens it opened, the last on top.
    pub fn open_to(
        &mut self,
        from: &str,
        setting: Setting,
    ) -> Result<(String, String, Vec<String>), AppInitError> {
        let menus = self.stage.show.menus().expect("the menus stand");
        let (path, row) = path_to(menus.screens(), from, setting)
            .unwrap_or_else(|| panic!("no row from {from} reaches one that steps {setting:?}"));
        let mut on = from.to_owned();
        let mut opened = Vec::new();
        for (node, screen) in path {
            self.focus(&on, &node)?;
            self.press(Scancode::RETURN)?;
            self.settle(&format!("opening {screen}"), |walk| walk.is_open(&screen))?;
            opened.push(screen.clone());
            on = screen;
        }
        Ok((on, row, opened))
    }

    /// Closes `opened`, the last on top, by the back key, until `under` is on top again.
    pub fn close(&mut self, opened: &[String], under: &str) -> Result<(), AppInitError> {
        for screen in opened.iter().rev() {
            self.press(Scancode::ESCAPE)?;
            self.settle(&format!("closing {screen}"), |walk| {
                let menus = walk.stage.show.menus().expect("the menus stand");
                menus.tree(MenuScreen::named(screen)).is_none()
            })?;
        }
        self.settle(&format!("{under}, again"), |walk| walk.is_open(under))
    }

    /// Presses `node` on `screen` until `done` says the settings are as wanted: a frame each value.
    pub fn step_until(
        &mut self,
        screen: &str,
        node: &str,
        setting: Setting,
        done: impl Fn(&PlayerSettings) -> bool,
    ) -> Result<(), AppInitError> {
        self.focus(screen, node)?;
        let mut values = 0;
        while !done(self.settings()) {
            assert!(values < MOST_VALUES, "{setting:?} never came to the value wanted");
            self.settle(&format!("{screen}, for {setting:?}"), |walk| walk.is_open(screen))?;
            self.key(Scancode::RETURN, true);
            self.input();
            self.key(Scancode::RETURN, false);
            self.what = format!("{setting:?} at {}", value_of(self.settings(), setting));
            self.held()?;
            values += 1;
        }
        Ok(())
    }

    /// Presses `node` on `screen`, which steps `setting`, until it's back where it began: a frame each value.
    pub fn step_round(&mut self, screen: &str, node: &str, setting: Setting) -> Result<(), AppInitError> {
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
            self.what = format!("{setting:?} at {}", value_of(self.settings(), setting));
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

/// The rows from `from` that open screens, each with the screen it opens, to the screen with the row that steps
/// `setting`, and that row: the shortest way, by the document.
fn path_to(
    screens: &game_ui::menus::MenuDocument,
    from: &str,
    setting: Setting,
) -> Option<(Vec<(String, String)>, String)> {
    let mut ways: Vec<(String, Vec<(String, String)>)> = vec![(from.to_owned(), Vec::new())];
    let mut seen = vec![from.to_owned()];
    while !ways.is_empty() {
        let mut next = Vec::new();
        for (name, path) in ways {
            let Some(entry) = screens.screens.iter().find(|entry| entry.name == name) else { continue };
            for row in &entry.rows {
                match &row.does {
                    RowAction::Step(stepped) if *stepped == setting => return Some((path, row.node.clone())),
                    RowAction::Open(opened) if !seen.contains(opened) => {
                        seen.push(opened.clone());
                        let mut way = path.clone();
                        way.push((row.node.clone(), opened.clone()));
                        next.push((opened.clone(), way));
                    }
                    _ => {}
                }
            }
        }
        ways = next;
    }
    None
}

/// Every row of the game's screens that steps a setting or opens a screen, as `stepped` and `opened` should have
/// them after the pause's walk: one the pause can't get to fails here, naming `place`, rather than going unwalked.
pub fn walked_every_row(
    screens: &game_ui::menus::MenuDocument,
    stepped: &[Setting],
    opened: &[String],
    place: &str,
) {
    for entry in &screens.screens {
        for row in &entry.rows {
            match &row.does {
                RowAction::Step(setting) => assert!(
                    stepped.contains(setting),
                    "{setting:?}, on {}, was never stepped {place}",
                    entry.name
                ),
                RowAction::Open(screen) => {
                    assert!(
                        opened.contains(screen),
                        "{screen}, from {}, was never opened {place}",
                        entry.name
                    );
                }
                RowAction::Switch(_)
                | RowAction::Remap(_)
                | RowAction::Swap
                | RowAction::Keep
                | RowAction::Reset(_)
                | RowAction::Command(_)
                | RowAction::Nothing => {}
            }
        }
    }
}

/// What `setting` is set to, as a failure names it.
pub fn value_of(settings: &PlayerSettings, setting: Setting) -> String {
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
        Setting::TimingAssist => format!("{:?}", settings.timing_assist),
        Setting::Hold(action) => format!("{:?} {:?}", action, settings.holds.mode(action)),
        Setting::Quality => format!("{:?}", settings.quality),
        Setting::FrameCap => format!("{:?}", settings.frame_cap),
        Setting::DynamicResolution => settings.dynamic_resolution.to_string(),
        Setting::BatterySaver => settings.battery_saver.to_string(),
    }
}

/// A lane's `main`: it runs `A` only with `--ignored`, as an `#[ignore]`d test does, since it needs a GPU and
/// cooked content (`target/content/desktop` unless `SAPPHIRE_CONTENT_DIR` says), on the platform's loop, which
/// must own the process's main thread (`harness = false`).
pub fn main<A: App>(lane: &str) -> ExitCode {
    if !std::env::args().any(|argument| argument == "--ignored") {
        eprintln!("{lane}: needs a GPU and cooked content; run it with --ignored");
        return ExitCode::SUCCESS;
    }
    sapphire_core::log::init(Default::default(), None).expect("initialize app test logging");
    if std::env::var_os("SAPPHIRE_CONTENT_DIR").is_none() {
        let cook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/content/desktop");
        // SAFETY: nothing else runs yet: the platform's loop and its threads start below.
        unsafe { std::env::set_var("SAPPHIRE_CONTENT_DIR", cook) };
    }
    run_with::<A>(RunOptions { background: true, headless: true })
}
