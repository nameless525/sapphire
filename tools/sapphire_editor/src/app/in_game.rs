//! Play in game (M6 T19): [`Command::PlayInGame`] starts a game as a child process ([`crate::game`]) and points the
//! link at it once it says where it listens; while it plays, every save goes to it (each document a game takes as
//! text, [`takes_live`]), and every pack a save's cook rewrites after that cook lands. [`Command::StopGame`] kills it
//! and puts the link back where it was. A game that stops by itself does the same, and the device pane shows its
//! status and the end of its log: nothing of the editor's (its documents, their undo, the journal) is touched.
//!
//! A script waits for the game: a [`Command::PlayInGame`] until the game answers, and each command after it until
//! what it pushed is answered, its saves' cooks landed first, so the next command sees what the game made of the
//! last, and fails on a push the game refused (`settle_game`). What's saved or cooked while a game starts is held
//! and sent once it says where it listens.

use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::cook::Cooked;
use crate::game::{GameTarget, Sandbox, takes_live};

/// How long a script waits for a game it started to answer: a cold start builds its scene's pipelines, slowly on a
/// CPU rasterizer.
pub const GAME_STARTUP: Duration = Duration::from_secs(300);
/// How long a script waits for what it pushed to a game to be answered: a pack's swap is up to a minute.
pub const GAME_ANSWER: Duration = Duration::from_secs(120);

/// What a save or a cook had for a game that hadn't said where it listens yet: sent once it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Held {
    /// A document, by its ID: its text as it is when it goes.
    Document(AssetId),
    /// A pack file a cook wrote.
    Pack(PathBuf),
}

impl Editor {
    /// Starts games with `launcher` from now on, rather than the dev sandbox: a test's stand-in.
    pub fn play_with(&mut self, launcher: Box<dyn Launcher>) {
        self.launcher = Some(launcher);
    }

    /// The game playing, if one is.
    #[must_use]
    pub const fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }

    /// The last game that stopped by itself, until another plays.
    #[must_use]
    pub const fn game_died(&self) -> Option<&GameDied> {
        self.game_died.as_ref()
    }

    /// Starts a game on `target`, in place of any playing, on the editor's cook.
    pub(super) fn play_in_game(&mut self, target: &GameTarget) {
        if let Some(died) = self.game_died.take() {
            died.forget();
        }
        self.game_held.clear();
        self.game_pushes.clear();
        let replaced = self.game.take().map(Game::stop).is_some();
        let started = match &self.launcher {
            Some(launcher) => Game::start(launcher.as_ref(), target.clone()),
            None => match self.game_binary.clone().or_else(Sandbox::beside_editor) {
                Some(binary) => Game::start(&Sandbox::new(binary, self.content.clone()), target.clone()),
                None => Err("no dev sandbox to play in: --game names one".to_owned()),
            },
        };
        match started {
            Ok(game) => {
                // Until it says where it listens, the link talks to nobody.
                if self.link_before_game.is_none() {
                    self.link_before_game = Some(self.link.address());
                }
                self.link.set_address(None);
                self.game = Some(game);
                self.game_died = None;
                self.say(Notice { text: format!("playing {target} in the game"), trouble: false });
            }
            Err(why) => {
                // The one it replaced is gone, and its port with it.
                if replaced {
                    self.link_back();
                }
                self.refuse(why);
            }
        }
    }

    /// Stops the game playing.
    pub(super) fn stop_game(&mut self) {
        match self.game.take() {
            Some(game) => {
                game.stop();
                self.link_back();
                self.say(Notice { text: "stopped the game".to_owned(), trouble: false });
            }
            None => self.refuse("no game is playing"),
        }
    }

    /// Puts the link back where it was before a game took it.
    fn link_back(&mut self) {
        let address = self.link_before_game.take().flatten();
        self.address_text = address.map(|address| address.to_string()).unwrap_or_default();
        self.link.set_address(address);
        self.game_held.clear();
        self.game_pushes.clear();
    }

    /// Looks at the game playing, every pass: whether it has said where it listens, and whether it has stopped.
    pub(super) fn poll_game(&mut self) {
        let Some(game) = &mut self.game else { return };
        if let Some(died) = game.poll() {
            self.game = None;
            self.link_back();
            self.say(Notice {
                text: format!("the game stopped by itself ({}): the device pane has its log", died.status),
                trouble: true,
            });
            self.game_died = Some(died);
            return;
        }
        if self.link.address().is_none()
            && let Some(address) = game.address()
        {
            self.address_text = address.to_string();
            self.link.set_address(Some(address));
            for held in std::mem::take(&mut self.game_held) {
                match held {
                    Held::Document(id) => self.push_to_game(&[id]),
                    Held::Pack(file) => self.push_pack_file(file),
                }
            }
        }
    }

    /// Whether a game playing has document `id` from its save.
    pub(super) fn game_takes(&self, id: AssetId) -> bool {
        self.game.is_some() && self.documents.path(id).and_then(Kind::of).is_some_and(takes_live)
    }

    /// Documents `saved` were written: each a game playing takes as text goes to it, by its file's name.
    pub(super) fn push_to_game(&mut self, saved: &[AssetId]) {
        for id in saved {
            if !self.game_takes(*id) {
                continue;
            }
            if self.link.address().is_none() {
                let held = Held::Document(*id);
                if !self.game_held.contains(&held) {
                    self.game_held.push(held);
                }
                continue;
            }
            let (Some(path), Some(document)) = (self.documents.path(*id), self.documents.any(*id)) else {
                continue;
            };
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let text = document.text().to_owned();
            self.pushed_to_game(&name);
            self.link.push(name, text);
        }
    }

    /// A save's cook landed: each pack file it wrote goes to a game playing, which takes the assets that changed.
    pub(super) fn push_packs_to_game(&mut self, cooked: &Cooked) {
        if self.game.is_none() {
            return;
        }
        for file in &cooked.files {
            self.push_pack_file(file.clone());
        }
    }

    /// Pushes the pack file `file` by its stem, as `cargo xtask link push` does: a part of a pack in parts by its
    /// own (`shared.1`). Held while the game hasn't said where it listens.
    fn push_pack_file(&mut self, file: PathBuf) {
        if self.link.address().is_none() {
            let held = Held::Pack(file);
            if !self.game_held.contains(&held) {
                self.game_held.push(held);
            }
            return;
        }
        let Some(name) = file.file_stem().and_then(|stem| stem.to_str()).map(str::to_owned) else { return };
        self.pushed_to_game(&name);
        self.link.push_pack(name, file);
    }

    /// A script's wait for the game, after each command: while one plays, its saves' cooks land, then it waits until
    /// the game has answered at all and has answered everything pushed. `Err` if the game stopped by itself, or
    /// didn't answer in time, with the end of its log.
    pub(super) fn settle_game(&mut self) -> Result<(), String> {
        if self.game.is_none() {
            return Ok(());
        }
        self.finish_cooks();
        let started = Stopwatch::start();
        loop {
            self.link.poll();
            self.poll_game();
            let Some(game) = &self.game else {
                let died = self.game_died.as_ref().map_or_else(String::new, |died| {
                    format!("the game stopped by itself ({}):\n{}", died.status, died.log.join("\n"))
                });
                return Err(died);
            };
            let answered = self.link.device().is_some();
            if answered && !self.link.is_waiting() && self.game_held.is_empty() {
                return self.refused_by_game();
            }
            let patience = if answered { GAME_ANSWER } else { GAME_STARTUP };
            if started.elapsed() >= patience {
                let what = if answered { "answer what was pushed" } else { "answer" };
                return Err(format!(
                    "the game didn't {what} in {} s: {}; its log is {}",
                    patience.as_secs(),
                    self.link.trouble().unwrap_or("no trouble said"),
                    game.log_path().display()
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Notes that `origin` went to the game, once each, for a script's wait to read its answer.
    fn pushed_to_game(&mut self, origin: &str) {
        if !self.game_pushes.iter().any(|pushed| pushed == origin) {
            self.game_pushes.push(origin.to_owned());
        }
    }

    /// What the game made of what this command pushed it: `Err` naming each push it refused or that failed.
    fn refused_by_game(&mut self) -> Result<(), String> {
        let pushed = std::mem::take(&mut self.game_pushes);
        let refused: Vec<String> = pushed
            .iter()
            .filter_map(|origin| match self.link.answer(origin)? {
                Pushed::Took { .. } => None,
                Pushed::Refused(refusals) => {
                    let why: Vec<&str> = refusals.iter().map(|refusal| refusal.message.as_str()).collect();
                    Some(format!("the game refused {origin}: {}", why.join("; ")))
                }
                Pushed::Failed(why) => Some(format!("{origin} didn't reach the game: {why}")),
            })
            .collect();
        if refused.is_empty() { Ok(()) } else { Err(refused.join("\n")) }
    }

    /// The device pane's game: what plays, Play and Stop, what each file pushed to it came to, and the end of the log
    /// of one that stopped by itself.
    pub(super) fn game_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Game").small().strong());
            match &self.game {
                Some(game) => {
                    ui.label(format!("playing {}", game.target()));
                    if ui.button("Stop").clicked() {
                        self.asked.push(Command::StopGame);
                    }
                }
                None => {
                    if let Some(stem) = self.current_zone_stem()
                        && ui.button("Play this zone").clicked()
                    {
                        self.asked.push(Command::PlayInGame(GameTarget::Zone(stem)));
                    }
                    if ui.button("Play a fight").clicked() {
                        let fight = GameTarget::Arena { script: "attempts".to_owned() };
                        self.asked.push(Command::PlayInGame(fight));
                    }
                }
            }
        });
        if let Some(game) = &self.game
            && self.link.address().is_none()
        {
            ui.label(
                RichText::new(format!(
                    "starting (process {}): its log is {}",
                    game.id(),
                    game.log_path().display()
                ))
                .small()
                .color(Color32::from_gray(150)),
            );
        }
        if self.game.is_some() {
            for (file, pushed) in self.link.answers() {
                let (color, said) = match pushed {
                    Pushed::Took { owner, milliseconds } => (
                        Color32::from_rgb(120, 214, 130),
                        format!("{file}: took it in {milliseconds} ms: {owner}"),
                    ),
                    Pushed::Refused(refusals) => (
                        Color32::from_rgb(255, 130, 115),
                        format!("{file}: refused, {} problems: it keeps what it had", refusals.len()),
                    ),
                    Pushed::Failed(why) => {
                        (Color32::from_rgb(235, 190, 110), format!("{file}: not pushed: {why}"))
                    }
                };
                ui.label(RichText::new(said).small().color(color));
            }
        }
        if let Some(died) = &self.game_died {
            ui.colored_label(
                Color32::from_rgb(255, 130, 115),
                format!("The game ({}) stopped by itself: {}", died.target, died.status),
            );
            egui::ScrollArea::vertical().id_salt("game log").max_height(160.0).show(ui, |ui| {
                ui.label(RichText::new(died.log.join("\n")).small().monospace());
            });
            ui.label(
                RichText::new(format!("its whole log: {}", died.log_path.display()))
                    .small()
                    .color(Color32::from_gray(150)),
            );
        }
        ui.separator();
    }

    /// The current document's stem, if it's a zone: what `--zone` plays.
    fn current_zone_stem(&self) -> Option<String> {
        let file = self.current()?;
        let name = file.file_name();
        (Kind::of(file.path()) == Some(Kind::Zone)).then(|| name.trim_end_matches(".zone.ron").to_owned())
    }
}
