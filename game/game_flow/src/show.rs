//! What stands on the game's stage (M3 S2, U7, G1): a fight, its HUD, the bindings that play it and the
//! menus, with the title over them all until someone starts.
//!
//! **With a game** ([`GameShow::with_game`]), its flow decides: the title stands the run it would open, a new
//! game or a continue plays it or restarts under the wipe, a pause can go back to the title, a fall restarts
//! at the save and a win goes back to the title. The flow acts on a run only between its steps, by what the
//! run records ([`Game`]). **With none,** the fight that stands is the run, as a replay's is: the title's
//! new game opens it, from its start if it's been played, and its continue goes on with it.
//!
//! **Conversations (M3 Q2b):** after each step the fight's dialogue runner looks for a trigger that opens one,
//! the panel's commands go to it, and the dialogue screen follows it before the menus draw. While one's open
//! the fight holds its steps, and the panel has the presses. [`GameShow::take_dialogue_ended`] says when one
//! closed. With a game, a beat's dialogue opens on the same runner, and its end is the flow's to go on from.

use std::time::Duration;

use game_combat::combat::{Combat, CombatOption, read_bindings};
use game_combat::dialogue::{DialogueEnded, DialogueRunError};
use game_combat::documents::{FightDocuments, error_chain};
use game_ui::dialogue::DialoguePanel;
use game_ui::hud::{FightHud, HudMode, HudOptions, ShownFight};
use game_ui::menus::{
    CharacterModel, GameMenus, HudModel, MenuCommand, MenuDocument, MenuScreen, MenuSetup, PlayerSettings,
    ScreenModel,
};
use game_ui::wipe::ZoneWipe;
use sapphire_controls::{CombatControls, ControlsError};
use sapphire_ecs::prelude::Entity;
use sapphire_gameplay::{ActorId, DialogueId};
use sapphire_input::InputMap;
use sapphire_platform::{AppInitError, ContentFs, Event, GamepadEvent, Os, Timestamp, TouchPhase, Window};
use sapphire_render::quality::TierId;
use sapphire_render::{PipelineInventory, Renderer, UiPageState, VfxFrame};
use sapphire_rhi::GpuContext;
use sapphire_stage::audio::CueSounds;
use sapphire_stage::content::Content;
use sapphire_stage::scene::Scene;
use sapphire_stage::{ContentChanges, FrameLaps, SceneShow};
use sapphire_time::{FixedStepDriver, FixedSteps, Stopwatch};

use sapphire_math::Vec2;
use sapphire_ui::{Atlas, UiDrawList};

use crate::LOG_TARGET;
use crate::flow::{After, FlowAction, FlowCommand, FlowState};
use crate::game::{FlowContent, Game, GameSetup};
use crate::menus::{flow_command, show_hud, show_title};

/// Who a step with no fight on it asks the bindings for: see `read_bindings`.
const PLAYER: ActorId = ActorId::from_raw(1).expect("not zero");
/// The inset the character screen's document shows the character in.
const CHARACTER_INSET: u16 = 0;
/// How often a fight says how it's going.
const REPORT_EVERY: Duration = Duration::from_secs(1);

/// See this module.
pub struct GameShow {
    pub shown: ShownFight,
    /// From [`Self::stand`] on.
    menus: Option<GameMenus>,
    /// What the menus start from.
    setup: MenuSetup,
    /// What the HUD and the menus draw from: a frame's list has one atlas.
    atlas: Atlas,
    /// Whether the title's been left for the fight.
    fight_open: bool,
    /// Whether the fight's been opened since the app began: from then on the title has it to continue.
    fight_begun: bool,
    /// What the menus asked this frame.
    commands: Vec<MenuCommand>,
    settings_changed: bool,
    pub controls: CombatControls,
    /// Where the camera looked as this frame's steps began: what a tick's stick means by forward.
    camera_yaw: f32,
    /// How long since the fight last said how it's going.
    report_in: Duration,
    /// Pipelines frames have built that the load didn't, as last said.
    first_uses: u32,
    /// The character standing in the character screen's inset while it's open (M3 U6).
    showcase: Option<Entity>,
    /// The dialogue screen on the fight's conversation (M3 Q2b).
    panel: DialoguePanel,
    /// The game's flow over its runs, if this is a game's show (M3 G1).
    game: Option<Game>,
    /// Whether the player's left, from the flow.
    left: bool,
    /// The conversation a beat opened, until it ends.
    beat_dialogue: Option<DialogueId>,
}

impl GameShow {
    /// A show with nothing on the scene yet, at the title, on `documents` (the game's own, or Sapphire's), with
    /// the game's `screens` (M3 U9) and as `settings` have it.
    pub fn new(
        documents: FightDocuments,
        screens: MenuDocument,
        settings: PlayerSettings,
    ) -> Result<Self, ControlsError> {
        let controls = match &documents.bindings {
            Some(bindings) => CombatControls::new(InputMap::new(bindings.clone())?)?,
            None => CombatControls::with_default_bindings()?,
        };
        let mut shown = ShownFight::default();
        shown.fight.documents = documents;
        // iOS's guidelines leave leaving to the home gesture.
        let setup = MenuSetup { screens, settings, can_leave: Os::CURRENT != Os::Ios, pseudo: false };
        Ok(Self {
            shown,
            menus: None,
            setup,
            atlas: Atlas::paint(),
            fight_open: false,
            fight_begun: false,
            commands: Vec::with_capacity(8),
            settings_changed: false,
            controls,
            showcase: None,
            panel: DialoguePanel::default(),
            camera_yaw: 0.0,
            report_in: Duration::ZERO,
            first_uses: 0,
            game: None,
            left: false,
            beat_dialogue: None,
        })
    }

    /// The show a game's flow decides (M3 G1): see the [module docs](self).
    #[must_use]
    pub fn with_game(mut self, game: Game) -> Self {
        self.game = Some(game);
        self
    }

    /// The show a game's `content` makes with `settings`, and the run it stands first: the flow the content
    /// names over its runs as `setup` says, or, where it names none, the arena's fight alone, played as
    /// `setup`'s script says.
    pub fn from_content(
        content: &ContentFs,
        settings: PlayerSettings,
        setup: GameSetup,
    ) -> Result<(Self, CombatOption), AppInitError> {
        let documents = FightDocuments::read(content).map_err(AppInitError::msg)?;
        let screens = MenuDocument::read(content).map_err(AppInitError::msg)?;
        let show = Self::new(documents, screens, settings)?;
        let read = |path: &str| content.read(path).map_err(|error| error_chain(&error));
        let Some(flow) = FlowContent::read(&read).map_err(AppInitError::msg)? else {
            return Ok((show, CombatOption::Fight { script: setup.script, party: false }));
        };
        let game =
            Game::new(flow, setup).map_err(|error| AppInitError::msg(format!("the game's flow: {error}")))?;
        let mut show = show.with_game(game);
        let first = show.opening_run().ok_or_else(|| AppInitError::msg("a game with no run to stand"))?;
        Ok((show, first))
    }

    /// The game's flow, if this is a game's show.
    #[must_use]
    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }

    /// The score's phase the game's flow asks for, where it has a say (M3 G1): at the title, in a defeat and
    /// in a win. Elsewhere, and with no game, the fight's encounters choose it.
    #[must_use]
    pub fn music_phase(&self) -> Option<&str> {
        self.game.as_ref()?.flow().music_phase()
    }

    /// What the fight's cues ask of an app's audio this frame, with the score's phase the flow's where it has a
    /// say ([`Self::music_phase`]): what the app's audio takes after the frame's steps.
    #[must_use]
    pub fn cue_sounds(&self) -> CueSounds<'_> {
        let mut sounds = self.shown.fight.show.cue_sounds();
        if let Some(phase) = self.music_phase() {
            sounds.music_phase = Some(phase);
        }
        sounds
    }

    /// The run a game stands first, behind its title: what [`Self::stand`] takes. Nothing for a show with no
    /// game, whose fight is its caller's to choose.
    pub fn opening_run(&mut self) -> Option<CombatOption> {
        self.game.as_mut().map(Game::first_run)
    }

    /// What the player chose, as a run nobody's playing chooses it (`--new-game`, `--continue`): as a menu's
    /// row would. A show with no game opens its fight for either.
    pub fn choose(&mut self, command: FlowCommand) {
        if self.game.is_none() {
            if matches!(command, FlowCommand::NewGame | FlowCommand::Continue) {
                self.open_fight();
            }
            return;
        }
        self.command_game(command);
        self.follow_flow();
    }

    /// Ends what's playing: the run, writing its file if it keeps one, and the game's save, waiting for its
    /// writes. What a quit calls.
    pub fn finish(&mut self) -> Result<(), String> {
        if let Some(game) = &mut self.game {
            game.finish();
        }
        self.shown.fight.finish()
    }

    /// A command for the flow, and what it asks of the show.
    fn command_game(&mut self, command: FlowCommand) {
        let Some(game) = &mut self.game else { return };
        match game.command(command) {
            Some(FlowAction::SkipTimeline) => {
                self.shown.fight.skip_timeline();
            }
            Some(FlowAction::Exit) => self.left = true,
            _ => {}
        }
        if command == FlowCommand::ToTitle {
            // The pause that asked for it closes as the wipe covers, and nothing steps meanwhile.
            self.controls.set_paused(false);
        }
    }

    /// The screens as the flow has them: the title while it's up, or covered on its way, and the HUD while a
    /// run plays. What's under a wipe that's covering stays as it was.
    fn follow_flow(&mut self) {
        let Some(game) = &self.game else { return };
        let title = match game.flow().state() {
            FlowState::Title { .. } | FlowState::Restarting { then: After::Title, covered: true, .. } => true,
            FlowState::Restarting { covered: false, .. } => return,
            _ => false,
        };
        match (title, self.fight_open) {
            (true, true) => self.back_to_title(),
            (false, false) => self.open_fight(),
            _ => {}
        }
        if let (Some(game), Some(menus)) = (&self.game, &mut self.menus) {
            show_title(game.flow(), menus);
            if let Some(model) = menus.model_mut::<HudModel>(MenuScreen::HUD) {
                show_hud(game.flow(), model);
            }
        }
    }

    /// The pause, whoever asked for it, reaches the flow, which holds what it paused.
    fn pause_flow(&mut self) {
        let Some(game) = &self.game else { return };
        // A pause on its way out is over, as far as the flow goes: its screen has closed.
        let paused = self.controls.is_paused() && !self.controls.is_resuming();
        let state = game.flow().state();
        // A fall and a win pause too, so the pause's rows (the title, a quit) mean what they say there.
        let command = match state {
            FlowState::Paused { .. } if !paused => FlowCommand::Resume,
            FlowState::Paused { .. } | FlowState::Title { .. } | FlowState::Restarting { .. } => return,
            _ if paused => FlowCommand::Pause,
            _ => return,
        };
        self.command_game(command);
    }

    /// After a step of the run: the flow's beats and save. A dialogue a beat opens opens on the fight's
    /// runner, and the flow waits in it until the runner says it ended ([`Self::follow_dialogue_end`]). One
    /// the run can't open has ended at once, so the flow goes on.
    fn follow_game_step(&mut self) {
        let (Some(game), Some(combat)) = (&mut self.game, &mut self.shown.fight.combat) else { return };
        let Some(asset) = game.after_step(combat) else { return };
        let opened = match game.dialogue_of(asset) {
            Some(id) => {
                self.shown.fight.dialogue.open(id, combat).map(|()| id).map_err(|error| error.to_string())
            }
            None => Err("the run loads no such dialogue".to_owned()),
        };
        match opened {
            Ok(id) => self.beat_dialogue = Some(id),
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %asset, %error, "a beat's dialogue didn't open");
                game.command(FlowCommand::DialogueEnded);
            }
        }
    }

    /// A conversation a beat opened that's ended, as the fight's runner says once: the flow goes on from it.
    /// One a trigger opened is the fight's own, and says nothing to the flow. A show with no game leaves the
    /// end for [`Self::take_dialogue_ended`].
    fn follow_dialogue_end(&mut self) {
        let Some(game) = &mut self.game else { return };
        let Some(ended) = self.shown.fight.dialogue.take_ended() else { return };
        if self.beat_dialogue.take_if(|&mut id| id == ended.dialogue).is_some() {
            game.command(FlowCommand::DialogueEnded);
        }
    }

    /// Where the wipe stands, for the flow: once it covers a restart, the run the flow asks for stands, in
    /// place of the one there, and the screens follow. While a zone trip's under way the wipe is the trip's,
    /// and the flow waits for it to end: a run stood under a trip's wipe would be carried through the rest of
    /// the trip.
    fn stand_asked_run(&mut self, scene: &mut Scene, content: &mut Content) {
        let Some(game) = &mut self.game else { return };
        let (covered, clear) = match &self.shown.wipe {
            _ if self.shown.fight.show.trip_phase().is_some() => (false, false),
            Some(wipe) => (wipe.covered(), wipe.clear()),
            None => (true, true),
        };
        let Some(option) = game.wipe(covered, clear) else {
            self.follow_flow();
            return;
        };
        let fight = &mut self.shown.fight;
        let stood = fight
            .prepare_stock_combat(&option, content)
            .map_err(|error| error.to_string())
            .and_then(|combat| fight.start_run(combat, scene, content));
        match stood {
            Ok(followed) => {
                self.beat_dialogue = None;
                self.shown.hear(followed);
                tracing::info!(target: LOG_TARGET, "a run stands");
            }
            Err(error) => tracing::error!(target: LOG_TARGET, %error, "the run didn't stand"),
        }
        self.follow_flow();
    }

    /// Stands the fight, which waits at the title until [`Self::open_fight`], and loads the menus.
    pub fn stand(
        &mut self,
        combat: Combat,
        scene: &mut Scene,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
        tier: TierId,
    ) -> Result<(), AppInitError> {
        let followed = self.shown.fight.stand_prepared(combat, scene, content, renderer, gpu, tier)?;
        self.shown.hear(followed);
        let assets = content.load_screens(&self.setup.screens.ids(), renderer, gpu)?;
        let mut menus = GameMenus::new(assets, self.atlas.clone(), self.setup.clone())
            .map_err(|error| AppInitError::msg(format!("the menus: {error}")))?;
        menus.open(MenuScreen::TITLE);
        self.menus = Some(menus);
        // A fight that goes between zones covers each trip's load (M3 W4), from the one atlas.
        let trips =
            game_ui::wipe::cover_trips(&mut self.shown, self.atlas.clone(), content, renderer, gpu, tier)?;
        // A game's restarts cover theirs with the same wipe (M3 G1).
        if self.game.is_some() && !trips {
            self.shown.wipe_trips(ZoneWipe::load(self.atlas.clone(), content, renderer, gpu)?);
        }
        // The menus' and the HUD's interface with it, though neither's drawn yet.
        scene.prewarm(&*self, renderer);
        let builds = renderer.stats().pipelines;
        tracing::info!(
            target: LOG_TARGET,
            prewarmed = builds.prewarmed,
            first_use = builds.first_use_builds,
            "what the game may draw is built"
        );
        Ok(())
    }

    /// Says so when a frame has built a pipeline the load didn't, naming it: exit test 7's count, which should
    /// stay 0 from the title through the fight. Call it after a frame.
    pub fn say_first_uses(&mut self, renderer: &Renderer) {
        let first_uses = renderer.stats().pipelines.first_use_builds;
        if first_uses != self.first_uses {
            self.first_uses = first_uses;
            let report = renderer.first_use_report();
            tracing::warn!(target: LOG_TARGET, first_uses, %report, "a frame built what the load didn't");
        }
    }

    /// Whether the title's up: nothing plays while it is. A game's screens follow its flow, so it's up while the
    /// flow's there, and while a wipe covers the way to it.
    pub const fn on_title(&self) -> bool {
        !self.fight_open
    }

    /// The title's new game: the fight from its start, over again if it's been played.
    fn new_game(&mut self) {
        if self.fight_begun
            && let Some(combat) = self.shown.fight.combat.as_mut()
        {
            combat.restart();
        }
        self.open_fight();
    }

    /// The pause's way back: the title over the fight, which stands where it was for a continue to go on with.
    fn back_to_title(&mut self) {
        if !self.fight_open {
            return;
        }
        self.fight_open = false;
        self.controls.set_paused(false);
        self.shown.hud = None;
        if let Some(menus) = &mut self.menus {
            menus.close_all();
            menus.close(MenuScreen::HUD);
            menus.set_can_continue(true);
            menus.open(MenuScreen::TITLE);
        }
        tracing::info!(target: LOG_TARGET, "back to the title");
    }

    /// The title goes and the fight begins, with its HUD on, in the player's size.
    pub fn open_fight(&mut self) {
        if self.fight_open {
            return;
        }
        self.fight_open = true;
        self.fight_begun = true;
        let scale = self.menus.as_ref().map_or(1.0, |menus| menus.settings().scale());
        let options = HudOptions { mode: HudMode::Live, scale, ..HudOptions::default() };
        let mut hud = FightHud::for_fight_with_atlas(options, self.atlas.clone());
        hud.set_design(self.shown.fight.documents.hud);
        self.shown.hud = Some(hud);
        if let Some(menus) = &mut self.menus {
            menus.close(MenuScreen::TITLE);
            menus.open_under(MenuScreen::HUD);
        }
        tracing::info!(target: LOG_TARGET, "the fight opens");
    }

    /// A platform event for the game. The menus hear them all, and a press with a menu up, or on the HUD's
    /// pause, goes no further, so it doesn't also swing. Then the HUD's zones take what lands on them, which
    /// with a menu up is only what's left of the fingers they hold, and the bindings the rest.
    pub fn handle_event(&mut self, event: &Event) {
        if let Some(menus) = &mut self.menus {
            menus.handle_event(event);
            let on_hud = matches!(event, Event::Touch(touch) if touch.phase == TouchPhase::Down
                && menus.hud_takes(Vec2::new(touch.x, touch.y)));
            if on_hud || (menus.is_modal() && is_press(event)) {
                return;
            }
        }
        if !self.shown.hud.as_mut().is_some_and(|hud| hud.handle_event(event)) {
            self.controls.handle_event(event);
        }
    }

    /// The menus' share of the frame's input, as of `now`, and what they asked: whether the player left.
    /// [`Self::read_input`] calls it; with no window, a run nobody's playing calls it alone.
    pub fn update_menus(&mut self, now: Timestamp) -> bool {
        let Some(menus) = &mut self.menus else { return false };
        menus.update(now);
        self.commands.extend(menus.take_commands());
        let mut left = false;
        for index in 0..self.commands.len() {
            let command = self.commands[index];
            match command {
                // The pause is the controls', which a game's flow follows (`pause_flow`).
                MenuCommand::Resume => self.controls.set_paused(false),
                MenuCommand::Pause => self.controls.set_paused(true),
                MenuCommand::SettingsChanged => self.follow_settings(),
                command @ (MenuCommand::DialogueNext | MenuCommand::DialogueChose(_)) => {
                    let fight = &mut self.shown.fight;
                    if let Some(combat) = &mut fight.combat {
                        self.panel.hear(command, &mut fight.dialogue, combat);
                    }
                }
                // The rest is a game's flow's, where there's one.
                _ if self.game.is_some() => {
                    if let Some(command) = flow_command(command) {
                        self.command_game(command);
                    }
                }
                MenuCommand::NewGame => self.new_game(),
                MenuCommand::Continue => self.open_fight(),
                MenuCommand::ToTitle => self.back_to_title(),
                // The arena plays no cutscene a player may skip.
                MenuCommand::Skip => {}
                MenuCommand::Leave => left = true,
            }
        }
        self.commands.clear();
        self.follow_dialogue_end();
        self.pause_flow();
        self.follow_flow();
        left || std::mem::take(&mut self.left)
    }

    fn follow_settings(&mut self) {
        self.settings_changed = true;
        if let (Some(menus), Some(hud)) = (&self.menus, &mut self.shown.hud) {
            hud.set_scale(menus.settings().scale());
        }
    }

    /// What the player's chosen, if they've changed it since this was last called: for the app to apply, and
    /// to keep.
    pub fn take_changed_settings(&mut self) -> Option<PlayerSettings> {
        if !std::mem::take(&mut self.settings_changed) {
            return None;
        }
        self.menus.as_ref().map(|menus| menus.settings().clone())
    }

    /// What the app's window is called in the player's language (M3 Z5b), once [`Self::stand`] has loaded the
    /// strings.
    #[must_use]
    pub fn window_title(&self) -> Option<&str> {
        self.menus.as_ref().map(GameMenus::window_title)
    }

    /// Opens the run's dialogue `id` between steps, as a game's flow or a script's beat asks: the panel comes
    /// up before the next frame draws, and the fight holds its steps until it closes.
    ///
    /// # Errors
    ///
    /// As `DialogueRunner::open`; and with no fight standing, as for a dialogue the run never loaded.
    pub fn open_dialogue(&mut self, id: DialogueId) -> Result<(), DialogueRunError> {
        let fight = &mut self.shown.fight;
        let combat = fight.combat.as_mut().ok_or(DialogueRunError::Unknown(id))?;
        fight.dialogue.open(id, combat)
    }

    /// The conversation that closed since this was last asked, once: what a game's flow goes on from.
    pub fn take_dialogue_ended(&mut self) -> Option<DialogueEnded> {
        self.shown.fight.dialogue.take_ended()
    }

    /// The pause's screen follows the pause, whoever asked for it: a key, the HUD's button, focus going. A
    /// conversation's panel is modal too, and stays: it isn't the pause's to close.
    fn follow_pause(&mut self) {
        let Some(menus) = self.menus.as_mut().filter(|_| self.fight_open) else { return };
        if self.panel.is_up() {
            return;
        }
        let paused = self.controls.is_paused() && !self.controls.is_resuming();
        if paused && !menus.is_modal() {
            menus.open(MenuScreen::PAUSE);
        } else if !paused && menus.is_modal() {
            menus.close_all();
        }
    }

    /// What the fight says, on the screens that show it.
    fn show_fight_on_menus(&mut self) {
        let Some(menus) = &mut self.menus else { return };
        let fight = &self.shown.fight;
        let values = fight.show.hud_values().map(|(values, _)| values);
        let boss_name = fight.combat.as_ref().and_then(|combat| fight.show.boss_name(combat.simulation()));
        let touch = self.shown.hud.as_ref().is_some_and(FightHud::shows_controls);
        if let Some(model) = menus.model_mut::<HudModel>(MenuScreen::HUD) {
            model.touch = touch;
            model.follow_fight(values.as_ref(), boss_name);
        }
        if let Some(values) = values {
            menus.visit_models(|model: &mut CharacterModel| {
                model.hp = (values.player.hp * model.max_hp as f32).round() as i32;
            });
        }
    }

    /// The frame's input as of `now`, in `window`: the menus' first, then the zones', since what they read
    /// reaches this frame's steps through the bindings, as a pad's does. Returns whether the player left.
    pub fn read_input(&mut self, window: &Window, desktop: bool, now: Timestamp) -> bool {
        if let Some(menus) = &mut self.menus {
            menus.follow_window(window);
        }
        let left = self.update_menus(now);
        let controls = &mut self.controls;
        controls.input_mut().set_points_per_coordinate(window.points_per_coordinate());
        if let Some(hud) = &mut self.shown.hud {
            let metrics = hud.metrics_of(window);
            hud.follow_devices(desktop, controls.input().active_gamepad().is_some());
            if let Some(touch) = hud.update(now, metrics) {
                controls.read_touch(touch, now);
            }
        }
        controls.update(now);
        self.pause_flow();
        self.follow_flow();
        self.follow_pause();
        self.show_fight_on_menus();
        left || self.controls.wants_to_leave()
    }

    /// This frame's fixed steps: none while paused, and no burst after. Takes where the camera looks, which
    /// is what they push along.
    pub fn advance(
        &mut self,
        scene: &Scene,
        driver: &mut FixedStepDriver,
        frame_time: Duration,
    ) -> FixedSteps {
        self.camera_yaw = self.shown.fight.show.camera_heading(scene);
        self.controls.advance(driver, frame_time)
    }

    /// The view's share of a frame `dt` real seconds after the last, on the real clock so it stays responsive
    /// in slow motion: nothing while paused, as the director's frame asks. Returns the seconds it moved by,
    /// and says how the fight's going once in a while.
    pub fn drive_view(&mut self, dt: Duration) -> f32 {
        let paused = self.controls.is_paused();
        let view_dt = if paused { 0.0 } else { dt.as_secs_f32() };
        let (look, zoom) = (self.controls.look(), self.controls.camera_zoom());
        self.shown.fight.show.drive_camera(view_dt, look, zoom);
        if let Some(game) = &mut self.game {
            // The flow's holds and banners are on the real clock, and stop with the pause.
            game.update(view_dt);
            self.shown.ask_wipe(game.flow().wipe_ask(), dt.as_secs_f32());
        }
        self.report(dt);
        view_dt
    }

    /// A line each second of a fight, so a run on a device can be read from its log.
    fn report(&mut self, dt: Duration) {
        self.report_in = self.report_in.saturating_sub(dt);
        if self.on_title() || !self.report_in.is_zero() {
            return;
        }
        self.report_in = REPORT_EVERY;
        let (Some(combat), Some((values, _))) =
            (&self.shown.fight.combat, self.shown.fight.show.hud_values())
        else {
            return;
        };
        tracing::info!(
            target: LOG_TARGET,
            tick = combat.simulation().tick(),
            player_hp = values.player.hp,
            enemy_hp = values.enemy.map(|enemy| enemy.hp),
            "the fight goes on"
        );
    }
}

/// Whether `event` starts something, which a menu that's up keeps to itself. What ends something goes on, so
/// the bindings hear the end of what they saw begin.
fn is_press(event: &Event) -> bool {
    match event {
        Event::Key(key) => key.down,
        Event::MouseButton(button) => button.down,
        Event::Touch(touch) => touch.phase == TouchPhase::Down,
        Event::Gamepad { event: GamepadEvent::Button { down, .. }, .. } => *down,
        _ => false,
    }
}

impl SceneShow for GameShow {
    fn declare_pipelines(&self, inventory: &mut PipelineInventory) {
        self.shown.declare_pipelines(inventory);
        inventory.ui = true;
        inventory.insets |= self.menus.as_ref().is_some_and(GameMenus::holds_insets);
    }

    fn step(&mut self, scene: &mut Scene, laps: &mut FrameLaps) {
        let runs = self.game.as_ref().map_or(!self.on_title(), |game| game.flow().runs_steps());
        if !runs || self.shown.fight.holds_step() {
            return;
        }
        let mut stopwatch = Stopwatch::start();
        let mut bindings = self.controls.source(self.camera_yaw);
        read_bindings(self.shown.fight.combat.as_mut(), &mut bindings, PLAYER);
        laps.simulation += stopwatch.lap();
        // The flow acts on the step before the stage follows it: a beat's timeline starts between this step
        // and the next, and the stage starts a run only on the step it came after, so one that holds the
        // simulation is held from its first frame rather than ended by the next step.
        self.follow_game_step();
        let followed = self.shown.fight.follow_step(scene, laps);
        self.shown.hear(followed);
        self.shown.show_damage(scene);
        let fight = &mut self.shown.fight;
        if let Some(combat) = &mut fight.combat
            && let Err(error) = fight.dialogue.follow(combat)
        {
            tracing::warn!(target: LOG_TARGET, %error, "a trigger's conversation didn't open");
        }
    }

    fn end_steps(&mut self, scene: &mut Scene, dt: f32) {
        self.shown.end_steps(scene, dt);
    }

    fn follow_content(&mut self, scene: &mut Scene, content: &mut Content, changes: ContentChanges) {
        self.stand_asked_run(scene, content);
        self.shown.follow_content(scene, content, changes);
    }

    fn animate(&mut self, scene: &mut Scene, content: &Content, alpha: f32) {
        self.shown.animate(scene, content, alpha);
    }

    fn begin_interface(&mut self, pages: UiPageState) {
        self.shown.begin_interface(pages);
        if let Some(menus) = &mut self.menus {
            menus.follow_pages(pages);
        }
    }

    fn present(&mut self, scene: &mut Scene, alpha: f32, aspect: Option<f32>) {
        self.shown.present(scene, alpha, aspect);
        // The character screen's inset shows a showcase of the character while it's open, and none after.
        let wanted = self.menus.as_ref().is_some_and(|menus| menus.shows(ScreenModel::Character));
        match (wanted, self.showcase) {
            (true, None) => self.showcase = Some(scene.spawn_showcase(scene.character(), CHARACTER_INSET)),
            (false, Some(showcase)) => {
                scene.despawn_showcase(showcase);
                self.showcase = None;
            }
            _ => {}
        }
    }

    fn write_effects(&mut self, frame: &mut VfxFrame, alpha: f32) {
        self.shown.write_effects(frame, alpha);
    }

    fn adjust_post(&mut self, post: &mut sapphire_render::PostParams) {
        self.shown.adjust_post(post);
    }

    fn draw_interface(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        self.shown.draw_interface(size, list);
        if let Some(menus) = &mut self.menus {
            self.panel.follow(menus, &self.shown.fight.dialogue);
            // What a cutscene says at the playhead this frame shows, so a line comes up with its frame and
            // its cue. A conversation's panel says its own lines, so none shows over it. The fight's words
            // step aside for a cutscene, as its bars and buttons do.
            let show = &self.shown.fight.show;
            let said = show.timeline_player().and_then(|player| player.subtitle());
            let said = said.filter(|_| !self.panel.is_up());
            if let Some(model) = menus.model_mut::<HudModel>(MenuScreen::HUD) {
                model.follow_subtitle(said.map(|line| line.key), said.and_then(|line| line.speaker));
                model.fight_words = if show.plays_cutscene() { 0.0 } else { 1.0 };
            }
            menus.draw(size, list);
        }
    }

    fn inset_shots(&self, visit: &mut dyn FnMut(sapphire_stage::InsetShot<'_>)) {
        self.shown.inset_shots(visit);
        if let Some(menus) = &self.menus {
            menus.inset_shots(visit);
        }
    }

    fn draw_debug(&mut self) {
        self.shown.draw_debug();
    }
}

#[cfg(test)]
mod tests {
    use game_ui::menus::SIZES;
    use sapphire_assets::ui::UiDocument;
    use sapphire_gameplay::Buttons;
    use sapphire_loc::StringTable;
    use sapphire_platform::{KeyEvent, Keycode, Modifiers, Scancode, WindowId};
    use sapphire_stage::content::ScreenAssets;

    use super::*;

    const SIZE: (u32, u32) = (1280, 720);
    /// Longer than anything a screen does.
    const SETTLE: u64 = 2_000_000_000;

    /// The game's screens.
    fn screens() -> MenuDocument {
        MenuDocument::from_ron(include_str!("../../../content/ui/menus.ron")).expect("reads")
    }

    /// A show with the game's documents up and no words in them, not yet drawn.
    fn undrawn() -> GameShow {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/ui");
        let documents = screens().screens.into_iter().map(|screen| {
            let text = std::fs::read_to_string(ui.join(format!("{}.ui.ron", screen.name))).unwrap();
            UiDocument::from_document(&text).unwrap()
        });
        let assets = ScreenAssets {
            documents: documents.collect(),
            tables: vec![StringTable { locale: "en".to_owned(), ..StringTable::default() }],
            fonts: Vec::new(),
        };
        let mut show = GameShow::new(FightDocuments::shipped(), screens(), PlayerSettings::default())
            .expect("the bindings parse");
        let mut menus = GameMenus::new(assets, show.atlas.clone(), show.setup.clone()).unwrap();
        menus.open(MenuScreen::TITLE);
        show.menus = Some(menus);
        show
    }

    /// The same drawn once, and its clock.
    fn show() -> (GameShow, u64) {
        let mut show = undrawn();
        let mut clock = 0;
        frame(&mut show, &mut clock);
        (show, clock)
    }

    /// What a frame does to the menus and the bindings, with time for what moves to settle. Returns whether
    /// the player left.
    fn frame(show: &mut GameShow, clock: &mut u64) -> bool {
        *clock += SETTLE;
        let now = Timestamp::from_nanos(*clock);
        let left = show.update_menus(now);
        show.controls.update(now);
        show.follow_pause();
        show.show_fight_on_menus();
        show.draw_interface(SIZE, &mut UiDrawList::default());
        left
    }

    fn key(clock: u64, scancode: Scancode, down: bool) -> Event {
        Event::Key(KeyEvent {
            timestamp: Timestamp::from_nanos(clock),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat: false,
        })
    }

    /// A key down and up, each its own frame. Returns whether the player left.
    fn press(show: &mut GameShow, clock: &mut u64, scancode: Scancode) -> bool {
        show.handle_event(&key(*clock + 1, scancode, true));
        let left = frame(show, clock);
        show.handle_event(&key(*clock + 1, scancode, false));
        frame(show, clock) || left
    }

    fn is_open(show: &GameShow, screen: MenuScreen) -> bool {
        show.menus.as_ref().unwrap().is_open(screen)
    }

    /// A fight up and playing.
    fn fight() -> (GameShow, u64) {
        let (mut show, mut clock) = show();
        press(&mut show, &mut clock, Scancode::RETURN);
        assert!(!show.on_title());
        (show, clock)
    }

    /// The character screen's inset is declared from the documents, before it opens, so the first open builds
    /// nothing.
    #[test]
    fn the_character_screen_s_inset_is_declared_before_it_opens() {
        let show = undrawn();
        assert!(!is_open(&show, MenuScreen::named("character")));
        let mut inventory = PipelineInventory::default();
        show.declare_pipelines(&mut inventory);
        assert!(inventory.insets && inventory.ui);
    }

    /// The menus and the HUD draw from one atlas, which opening the fight hands on rather than painting again:
    /// a frame's list has one. Opening twice keeps the HUD that's up.
    #[test]
    fn opening_the_fight_keeps_the_one_atlas() {
        let (mut show, mut clock) = show();
        let mut list = UiDrawList::default();
        show.draw_interface(SIZE, &mut list);
        let image = list.atlas.clone().expect("the title's atlas");
        show.open_fight();
        frame(&mut show, &mut clock);
        list.reset();
        show.draw_interface(SIZE, &mut list);
        assert!(std::sync::Arc::ptr_eq(&image, list.atlas.as_ref().expect("the HUD's atlas")));
        let first = list.quads.clone();
        show.open_fight();
        list.reset();
        show.draw_interface(SIZE, &mut list);
        assert_eq!(first, list.quads, "opening twice keeps the live HUD");
    }

    #[test]
    fn starting_from_the_title_opens_the_fight_and_the_press_doesnt_also_swing() {
        let (mut show, mut clock) = show();
        assert!(show.on_title() && is_open(&show, MenuScreen::TITLE));
        // Combat's light attack is nothing on a title, then or later.
        show.handle_event(&key(clock + 1, Scancode::J, true));
        frame(&mut show, &mut clock);
        assert!(show.on_title());
        show.handle_event(&key(clock + 1, Scancode::RETURN, true));
        frame(&mut show, &mut clock);
        assert!(!show.on_title());
        assert!(show.shown.hud.is_some(), "the fight's HUD comes with it");
        assert_eq!(show.controls.next_actions(0.0).buttons, Buttons::default());
        frame(&mut show, &mut clock);
        assert!(!is_open(&show, MenuScreen::TITLE) && is_open(&show, MenuScreen::HUD));
        // Once it's gone, the key's combat's.
        show.handle_event(&key(clock + 1, Scancode::J, false));
        frame(&mut show, &mut clock);
        show.handle_event(&key(clock + 1, Scancode::J, true));
        frame(&mut show, &mut clock);
        assert_ne!(show.controls.next_actions(0.0).buttons, Buttons::default());
    }

    #[test]
    fn back_on_the_title_leaves() {
        let (mut show, mut clock) = show();
        assert!(press(&mut show, &mut clock, Scancode::ESCAPE));
    }

    #[test]
    fn a_title_that_was_never_drawn_doesnt_come_up_over_the_fight() {
        // What `--open-fight` does.
        let mut show = undrawn();
        show.open_fight();
        frame(&mut show, &mut 0);
        assert!(!is_open(&show, MenuScreen::TITLE));
        assert!(is_open(&show, MenuScreen::HUD));
    }

    #[test]
    fn the_pause_follows_whoever_asks_and_back_goes_one_screen_at_a_time() {
        let (mut show, mut clock) = fight();
        press(&mut show, &mut clock, Scancode::ESCAPE);
        assert!(show.controls.is_paused(), "the key is combat's while no menu's up");
        assert!(is_open(&show, MenuScreen::PAUSE));

        // Settings over the pause: back leaves it, and the fight stays paused.
        show.menus.as_mut().unwrap().open(MenuScreen::named("settings"));
        frame(&mut show, &mut clock);
        press(&mut show, &mut clock, Scancode::ESCAPE);
        frame(&mut show, &mut clock);
        assert!(!is_open(&show, MenuScreen::named("settings")) && is_open(&show, MenuScreen::PAUSE));
        assert!(show.controls.is_paused());

        // Back from the pause resumes.
        press(&mut show, &mut clock, Scancode::ESCAPE);
        frame(&mut show, &mut clock);
        assert!(!show.controls.is_paused() && !is_open(&show, MenuScreen::PAUSE));

        // What pauses from outside: focus going, say.
        show.controls.set_paused(true);
        frame(&mut show, &mut clock);
        assert!(is_open(&show, MenuScreen::PAUSE));
        assert!(!press(&mut show, &mut clock, Scancode::J), "no menu row is J, and combat's isn't heard");
        assert!(show.controls.is_paused());
    }

    /// The pause's way back stands the fight behind the title, whose continue goes on with it.
    #[test]
    fn the_pause_goes_back_to_the_title_and_its_continue_goes_on_with_the_fight() {
        let (mut show, mut clock) = fight();
        press(&mut show, &mut clock, Scancode::ESCAPE);
        assert!(is_open(&show, MenuScreen::PAUSE));
        show.commands.push(MenuCommand::ToTitle);
        frame(&mut show, &mut clock);
        frame(&mut show, &mut clock);
        assert!(show.on_title() && is_open(&show, MenuScreen::TITLE));
        assert!(!is_open(&show, MenuScreen::PAUSE) && !is_open(&show, MenuScreen::HUD));
        assert!(show.shown.hud.is_none() && !show.controls.is_paused());
        let menus = show.menus.as_mut().unwrap();
        assert!(menus.model_mut::<game_ui::menus::MenuModel>(MenuScreen::TITLE).unwrap().can_continue);
        frame(&mut show, &mut clock);
        let focus = show.menus.as_ref().unwrap().tree(MenuScreen::TITLE).unwrap().focus_name();
        assert_eq!(focus, Some("continue"), "focus starts on the run to go on with");

        press(&mut show, &mut clock, Scancode::RETURN);
        assert!(!show.on_title() && show.shown.hud.is_some());
        frame(&mut show, &mut clock);
        assert!(is_open(&show, MenuScreen::HUD) && !is_open(&show, MenuScreen::TITLE));
    }

    #[test]
    fn what_the_player_chooses_reaches_the_hud_and_the_app_once() {
        let (mut show, mut clock) = fight();
        assert_eq!(show.take_changed_settings(), None);
        show.controls.set_paused(true);
        frame(&mut show, &mut clock);
        let menus = show.menus.as_mut().unwrap();
        menus.open(MenuScreen::named("settings"));
        frame(&mut show, &mut clock);
        // The first row's the language, the next the size.
        press(&mut show, &mut clock, Scancode::DOWN);
        press(&mut show, &mut clock, Scancode::RETURN);
        let chosen = show.take_changed_settings().expect("a row stepped");
        assert_eq!(chosen.scale(), SIZES[2].1, "normal steps to large");
        assert_eq!(show.take_changed_settings(), None, "once");
        let hud = show.shown.hud.as_ref().unwrap();
        assert_eq!(hud.scale(), SIZES[2].1);
    }

    #[test]
    fn the_hud_document_says_what_the_fight_says() {
        let (mut show, mut clock) = fight();
        let hud = show.shown.hud.as_mut().unwrap();
        hud.follow_devices(false, false);
        frame(&mut show, &mut clock);
        let menus = show.menus.as_mut().unwrap();
        assert!(menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap().touch, "a phone has a pause button");
        let hud = show.shown.hud.as_mut().unwrap();
        hud.follow_devices(false, true);
        frame(&mut show, &mut clock);
        let menus = show.menus.as_mut().unwrap();
        assert!(!menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap().touch, "a pad has its own");
    }

    /// A game's own bindings are what its show reads, and a document that lacks what combat reads stops it,
    /// saying what's missing.
    #[test]
    fn a_games_bindings_play_its_show() {
        let own = sapphire_controls::DEFAULT_BINDINGS.replace("source: Key(J)", "source: Key(U)");
        let documents = FightDocuments {
            bindings: Some(sapphire_serialize::from_ron(&own).expect("reads")),
            ..FightDocuments::shipped()
        };
        let mut show = GameShow::new(documents, screens(), PlayerSettings::default())
            .expect("they hold what combat reads");
        let light = |show: &mut GameShow, scancode, at| {
            show.controls.handle_event(&key(at, scancode, true));
            show.controls.update(Timestamp::from_nanos(at));
            show.controls.next_actions(0.0).buttons.contains(Buttons::LIGHT)
        };
        assert!(!light(&mut show, Scancode::J, 1), "the shipped key does nothing");
        assert!(light(&mut show, Scancode::U, 2), "its own does");

        let mut lacking: sapphire_input::InputBindings =
            sapphire_serialize::from_ron(sapphire_controls::DEFAULT_BINDINGS).expect("reads");
        lacking.contexts.retain(|context| context.name != sapphire_controls::PAUSE_CONTEXT);
        let documents = FightDocuments { bindings: Some(lacking), ..FightDocuments::shipped() };
        let refused =
            GameShow::new(documents, screens(), PlayerSettings::default()).err().expect("it can't pause");
        assert!(refused.to_string().contains(sapphire_controls::PAUSE_CONTEXT), "{refused}");
    }

    /// The fight's HUD opens laid out by the game's design.
    #[test]
    fn a_games_design_lays_its_hud_out() {
        let mut documents = FightDocuments::shipped();
        documents.hud.dodge.offset = [-30.0, -150.0];
        let mut show =
            GameShow::new(documents, screens(), PlayerSettings::default()).expect("the bindings parse");
        show.open_fight();
        let hud = show.shown.hud.as_mut().expect("it's open");
        let metrics = sapphire_ui::UiMetrics::new(Vec2::new(874.0, 402.0), sapphire_ui::Insets::default());
        let _ = hud.update(Timestamp::from_nanos(1), metrics);
        let layout = hud.layout();
        assert_eq!(layout.dodge.center, layout.attack.center + Vec2::new(-30.0, -150.0));
    }
}
