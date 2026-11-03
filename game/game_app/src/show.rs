//! What stands on the game's stage: a fight, its HUD and the bindings that play it, and the title over them
//! until someone presses.

use std::time::Duration;

use game_combat::combat::{CombatOption, read_bindings};
use game_ui::hud::{FightHud, HudMode, HudOptions, ShownFight};
use game_ui::title::TitleCard;
use sapphire_controls::{CombatControls, ControlsError};
use sapphire_gameplay::ActorId;
use sapphire_input::DeviceKind;
use sapphire_platform::{
    AppInitError, Event, GamepadButton, GamepadEvent, Scancode, Timestamp, TouchPhase, Window,
};
use sapphire_render::quality::TierId;
use sapphire_render::{PipelineInventory, Renderer, VfxFrame};
use sapphire_rhi::GpuContext;
use sapphire_stage::content::Content;
use sapphire_stage::scene::Scene;
use sapphire_stage::{ContentChanges, FrameLaps, SceneShow};
use sapphire_time::{FixedStepDriver, FixedSteps, Stopwatch};
use sapphire_ui::UiDrawList;

use crate::LOG_TARGET;

/// Who a step with no fight on it asks the bindings for: see `read_bindings`.
const PLAYER: ActorId = ActorId::from_raw(1).expect("not zero");
/// Android's Back, as SDL's scancode for it: what combat's bindings call `Hid(282)`.
const AC_BACK: Scancode = Scancode(282);
/// How often a fight says how it's going.
const REPORT_EVERY: Duration = Duration::from_secs(1);

/// What a platform event does to the title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleAnswer {
    /// Nothing that concerns it, or it isn't up.
    Nothing,
    /// A press: the fight opens, and the press goes no further, so it doesn't also swing.
    Open,
    /// Back or `Esc`, which is where a title goes.
    Leave,
}

/// See this module.
pub struct GameShow {
    pub shown: ShownFight,
    title: Option<TitleCard>,
    pub controls: CombatControls,
    /// Where the camera looked as this frame's steps began: what a tick's stick means by forward.
    camera_yaw: f32,
    /// How long since the fight last said how it's going.
    report_in: Duration,
    /// Pipelines frames have built that the load didn't, as last said.
    first_uses: u32,
}

impl GameShow {
    /// A show with nothing on the scene yet, at the title.
    pub fn new() -> Result<Self, ControlsError> {
        Ok(Self {
            shown: ShownFight::default(),
            title: Some(TitleCard::new()),
            controls: CombatControls::with_default_bindings()?,
            camera_yaw: 0.0,
            report_in: Duration::ZERO,
            first_uses: 0,
        })
    }

    /// Stands the fight, which waits at the title until [`Self::open_fight`].
    pub fn stand(
        &mut self,
        option: &CombatOption,
        scene: &mut Scene,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
        tier: TierId,
    ) -> Result<(), AppInitError> {
        let followed = self.shown.fight.stand(option, scene, content, renderer, gpu, tier)?;
        self.shown.hear(followed);
        // The title's and the HUD's interface with it, though neither's drawn yet.
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

    /// Whether the title's up: nothing plays while it is.
    pub fn on_title(&self) -> bool {
        self.title.is_some()
    }

    /// The title goes and the fight begins, with its HUD on.
    pub fn open_fight(&mut self) {
        if self.title.take().is_some() {
            let options = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
            self.shown.hud = Some(FightHud::for_fight(options));
            tracing::info!(target: LOG_TARGET, "the fight opens");
        }
    }

    /// A platform event for the game. The title takes what opens the fight or leaves; then the HUD's zones
    /// take what lands on them and the bindings the rest. While paused a landing is the pause's, since a tap
    /// is how a phone gets out of it, and the zones keep only the fingers they already hold.
    pub fn handle_event(&mut self, event: &Event) -> TitleAnswer {
        if self.on_title() {
            let answer = title_answer(event);
            match answer {
                TitleAnswer::Open => self.open_fight(),
                TitleAnswer::Leave => {}
                TitleAnswer::Nothing => self.controls.handle_event(event),
            }
            return answer;
        }
        let pause_takes_landing = self.controls.is_paused()
            && !self.controls.is_resuming()
            && matches!(event, Event::Touch(touch) if touch.phase == TouchPhase::Down);
        if pause_takes_landing || !self.shown.hud.as_mut().is_some_and(|hud| hud.handle_event(event)) {
            self.controls.handle_event(event);
        }
        TitleAnswer::Nothing
    }

    /// The frame's input as of `now`, in `window`: the zones' first, since what they read reaches this
    /// frame's steps through the bindings, as a pad's does. Returns whether the player left from the pause.
    pub fn read_input(&mut self, window: &Window, desktop: bool, now: Timestamp) -> bool {
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
        let touch = controls.input().last_device() == Some(DeviceKind::Touch);
        self.shown.fight.show.set_played_on_touch(touch);
        controls.wants_to_leave()
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
    /// in slow motion: nothing while paused, as the director's frame asks, and the HUD says it's waiting.
    /// Returns the seconds it moved by, and says how the fight's going once in a while.
    pub fn drive_view(&mut self, dt: Duration) -> f32 {
        let paused = self.controls.is_paused();
        if let Some(hud) = &mut self.shown.hud {
            hud.show_pause(paused);
        }
        if let Some(title) = &mut self.title {
            title.advance(dt);
        }
        let view_dt = if paused { 0.0 } else { dt.as_secs_f32() };
        let (look, zoom) = (self.controls.look(), self.controls.camera_zoom());
        self.shown.fight.show.drive_camera(view_dt, look, zoom);
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

fn title_answer(event: &Event) -> TitleAnswer {
    match event {
        Event::Key(key) if key.down && !key.repeat => match key.scancode {
            Scancode::ESCAPE | Scancode::BACKSPACE | AC_BACK => TitleAnswer::Leave,
            _ => TitleAnswer::Open,
        },
        Event::MouseButton(button) if button.down => TitleAnswer::Open,
        Event::Touch(touch) if touch.phase == TouchPhase::Down => TitleAnswer::Open,
        Event::Gamepad { event: GamepadEvent::Button { button, down: true }, .. } => match button {
            GamepadButton::Back => TitleAnswer::Leave,
            _ => TitleAnswer::Open,
        },
        _ => TitleAnswer::Nothing,
    }
}

impl SceneShow for GameShow {
    fn declare_pipelines(&self, inventory: &mut PipelineInventory) {
        self.shown.declare_pipelines(inventory);
        inventory.ui = true;
    }

    fn step(&mut self, scene: &mut Scene, laps: &mut FrameLaps) {
        if self.on_title() {
            return;
        }
        let mut stopwatch = Stopwatch::start();
        let mut bindings = self.controls.source(self.camera_yaw);
        read_bindings(self.shown.fight.combat.as_mut(), &mut bindings, PLAYER);
        laps.simulation += stopwatch.lap();
        let followed = self.shown.fight.follow_step(scene, laps);
        self.shown.hear(followed);
        self.shown.show_damage(scene);
    }

    fn end_steps(&mut self, scene: &mut Scene, dt: f32) {
        self.shown.end_steps(scene, dt);
    }

    fn follow_content(&mut self, scene: &mut Scene, content: &mut Content, changes: ContentChanges) {
        self.shown.follow_content(scene, content, changes);
    }

    fn animate(&mut self, scene: &mut Scene, content: &Content, alpha: f32) {
        self.shown.animate(scene, content, alpha);
    }

    fn present(&mut self, scene: &mut Scene, alpha: f32, aspect: Option<f32>) {
        self.shown.present(scene, alpha, aspect);
    }

    fn write_effects(&mut self, frame: &mut VfxFrame, alpha: f32) {
        self.shown.write_effects(frame, alpha);
    }

    fn draw_interface(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        self.shown.draw_interface(size, list);
        if let Some(title) = &self.title {
            title.draw(size, list);
        }
    }

    fn draw_debug(&mut self) {
        self.shown.draw_debug();
    }
}

#[cfg(test)]
mod tests {
    use sapphire_platform::{FingerId, KeyEvent, Keycode, Modifiers, TouchDeviceId, TouchEvent, WindowId};

    use super::*;

    fn key(scancode: Scancode, down: bool, repeat: bool) -> Event {
        Event::Key(KeyEvent {
            timestamp: Timestamp::default(),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat,
        })
    }

    #[test]
    fn a_press_of_anything_opens_and_back_leaves() {
        assert_eq!(title_answer(&key(Scancode::J, true, false)), TitleAnswer::Open);
        assert_eq!(title_answer(&key(Scancode::ESCAPE, true, false)), TitleAnswer::Leave);
        assert_eq!(title_answer(&key(AC_BACK, true, false)), TitleAnswer::Leave);
        assert_eq!(title_answer(&key(Scancode::J, false, false)), TitleAnswer::Nothing, "a release");
        assert_eq!(title_answer(&key(Scancode::J, true, true)), TitleAnswer::Nothing, "a repeat");
        let touch = |phase| {
            Event::Touch(TouchEvent {
                timestamp: Timestamp::default(),
                window: WindowId::from_raw(1),
                finger: FingerId::new(TouchDeviceId::from_raw(1), 1),
                phase,
                x: 0.5,
                y: 0.5,
                pressure: 1.0,
            })
        };
        assert_eq!(title_answer(&touch(TouchPhase::Down)), TitleAnswer::Open);
        assert_eq!(title_answer(&touch(TouchPhase::Moved)), TitleAnswer::Nothing);
    }

    #[test]
    fn the_press_that_opens_the_fight_doesnt_also_swing() {
        let mut show = GameShow::new().expect("the bindings parse");
        assert!(show.on_title());
        assert_eq!(show.handle_event(&key(Scancode::J, true, false)), TitleAnswer::Open);
        assert!(!show.on_title());
        assert!(show.shown.hud.is_some(), "the fight's HUD comes with it");
        show.controls.update(Timestamp::from_nanos(1));
        let actions = show.controls.next_actions(0.0);
        assert_eq!(actions.buttons, sapphire_gameplay::Buttons::default());
    }
}
