//! `--hud`: the combat HUD over whatever the scene draws (M2 I2), in two modes.
//!
//! - **`fixed`:** one frozen frame, the same on any device: synthetic touches on a made-up clock put
//!   a thumb on the stick and another dragging the attack, with a hit landing and numbers in the
//!   air. What the golden scenes capture. Real touches go to the app as if there were no HUD.
//! - **`live`:** real touches through the zones, on `Platform::now`. With a fight on (`--arena`), its
//!   bars are the fighters' ([`SandboxHud::show_fight`]), offscreen too, where the fight's own time is
//!   its clock. With none, no gameplay stands behind the zones, so a stand-in shows that input arrives:
//!   the attack spends the enemy's bars and throws a number, a held attack a heavier one, the dodge spends
//!   the player's, the skill starts a cooldown, and the camera's drag orbits. Every button event and each
//!   second's stick and camera are logged, so a run on a device can be read from its log.
//!
//! The renderer sees only the packet's draw list (rule R4). Offscreen modes have no window: their
//! metrics come from the target and `--hud-scale`, and `--hud-insets` makes up what a notch would
//! cover, on any platform.

use std::time::Duration;

use sapphire_math::Vec2;
use sapphire_platform::{Event, FingerId, Timestamp, TouchDeviceId, TouchPhase, Window};
use sapphire_time::Stopwatch;
use sapphire_ui::{
    Atlas, Button, ButtonEventKind, CombatHud, DamageKind, FighterValues, HudLayout, HudStats, HudValues,
    Insets, SkillValues, TouchControls, UiDrawList, UiMetrics,
};

use crate::LOG_TARGET;

/// How the HUD runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HudMode {
    #[default]
    Fixed,
    Live,
}

/// `--hud` and its companions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HudOptions {
    pub mode: HudMode,
    /// `--hud-insets`: in place of what the window reports, in points.
    pub insets: Option<Insets>,
    /// `--hud-scale`: pixels per point in an offscreen frame, or the player's UI scale on top of
    /// the display's in a window.
    pub scale: f32,
}

impl Default for HudOptions {
    fn default() -> Self {
        Self { mode: HudMode::default(), insets: None, scale: 1.0 }
    }
}

/// The fixed frame's moment on its made-up clock.
const FIXED_NOW: u64 = 1460;
/// Radians per point the camera's drag orbits, as the default bindings' touch drag does.
const CAMERA_PER_POINT: f32 = -0.006;
/// How long the live skill cools down.
const LIVE_COOLDOWN: Duration = Duration::from_secs(6);
/// How often live mode logs the stick and camera.
const LOG_EVERY: Duration = Duration::from_secs(1);

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

fn finger(id: u64) -> FingerId {
    FingerId::new(TouchDeviceId::from_raw(1), id)
}

/// The stand-in live mode runs on: what no gameplay is there to do.
#[derive(Clone, Copy, Debug)]
struct LiveValues {
    player_hp: f32,
    enemy_hp: f32,
    enemy_poise: f32,
    cooldown_until: Option<Timestamp>,
    hits: u32,
    /// The camera's drag since it was last logged.
    camera_logged: Vec2,
    logged_at: Timestamp,
}

impl Default for LiveValues {
    fn default() -> Self {
        Self {
            player_hp: 1.0,
            enemy_hp: 1.0,
            enemy_poise: 1.0,
            cooldown_until: None,
            hits: 0,
            camera_logged: Vec2::ZERO,
            logged_at: Timestamp::default(),
        }
    }
}

/// The HUD, its touch zones and what they show.
pub(crate) struct SandboxHud {
    options: HudOptions,
    atlas: Atlas,
    layout: HudLayout,
    controls: TouchControls,
    hud: CombatHud,
    values: HudValues,
    now: Timestamp,
    live: LiveValues,
    /// What a fight says of its fighters, in place of the stand-in's numbers.
    fight: Option<HudValues>,
    /// A window's metrics this frame, read by [`Self::update`]; offscreen frames have none.
    window_metrics: Option<UiMetrics>,
    /// The metrics the layout was made for.
    metrics: Option<UiMetrics>,
    last_stats: HudStats,
    last_cost: Duration,
}

impl SandboxHud {
    pub fn new(options: HudOptions) -> Self {
        let painting = Stopwatch::start();
        let atlas = Atlas::paint();
        tracing::info!(
            target: LOG_TARGET,
            bytes = atlas.image.texels.len(),
            width = atlas.image.width,
            height = atlas.image.height,
            paint_us = painting.elapsed().as_micros() as u64,
            "painted the UI atlas"
        );
        let layout = HudLayout::default();
        Self {
            options,
            layout,
            controls: TouchControls::new(layout),
            hud: CombatHud::with_atlas(atlas.clone()),
            atlas,
            values: HudValues::default(),
            now: Timestamp::default(),
            live: LiveValues::default(),
            fight: None,
            window_metrics: None,
            metrics: None,
            last_stats: HudStats::default(),
            last_cost: Duration::ZERO,
        }
    }

    /// What the fight says of its fighters this frame, `time` into it: live mode shows it. Offscreen
    /// there's no other clock.
    pub fn show_fight(&mut self, values: HudValues, time: Duration) {
        self.fight = Some(values);
        if self.window_metrics.is_none() && self.options.mode == HudMode::Live {
            self.now = Timestamp::from_nanos(time.as_nanos() as u64);
        }
    }

    /// A platform event: whether the touch zones took it, which the rest of the input then never
    /// sees. Only live mode takes touches.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        self.options.mode == HudMode::Live && self.controls.handle_event(event)
    }

    /// Live mode's frame on the real clock, before the packet: the window's metrics, the touch zones
    /// and the stand-in. Returns the camera's orbit, in radians.
    pub fn update(&mut self, now: Timestamp, window: &Window) -> Vec2 {
        let mut metrics = UiMetrics::of_window(window, self.options.scale);
        if let Some(insets) = self.options.insets {
            metrics.insets = insets;
        }
        self.window_metrics = Some(metrics);
        if self.options.mode != HudMode::Live {
            return Vec2::ZERO;
        }
        self.now = now;
        self.relayout(metrics);
        let actions = self.controls.update(now);
        let camera = actions.camera_delta;
        self.live.camera_logged += camera;
        for event in &actions.events {
            let enemy = self.layout.metrics.size * Vec2::new(0.5, 0.42);
            let stand_in = self.fight.is_none();
            match (event.button, event.kind) {
                (Button::Attack, ButtonEventKind::Pressed) if stand_in => {
                    self.live.hits += 1;
                    self.live.enemy_hp = wrapped(self.live.enemy_hp - 0.06);
                    self.live.enemy_poise = wrapped(self.live.enemy_poise - 0.11);
                    let value = 240 + self.live.hits * 37 % 900;
                    self.hud.spawn_damage(value, enemy, DamageKind::Normal);
                }
                (Button::Attack, ButtonEventKind::Held) if stand_in => {
                    let value = 2400 + self.live.hits * 311 % 4000;
                    self.hud.spawn_damage(value, enemy + Vec2::new(0.0, -24.0), DamageKind::Heavy);
                }
                (Button::Dodge, ButtonEventKind::Pressed) if stand_in => {
                    self.live.player_hp = wrapped(self.live.player_hp - 0.09);
                }
                (Button::Skill, ButtonEventKind::Pressed)
                    if stand_in && self.live.cooldown_until.is_none() =>
                {
                    self.live.cooldown_until =
                        Some(Timestamp::from_nanos(now.as_nanos() + LIVE_COOLDOWN.as_nanos() as u64));
                }
                _ => {}
            }
            tracing::info!(
                target: LOG_TARGET,
                button = ?event.button,
                kind = ?event.kind,
                at_ms = event.at.as_nanos() / 1_000_000,
                drag = %event.drag,
                "hud button"
            );
        }
        if now.saturating_duration_since(self.live.logged_at) >= LOG_EVERY {
            let stick = actions.stick;
            if stick.active || self.live.camera_logged != Vec2::ZERO {
                tracing::info!(
                    target: LOG_TARGET,
                    stick = %stick.value,
                    thumb = %stick.thumb,
                    camera = %self.live.camera_logged,
                    fingers = self.controls.fingers(),
                    "hud touch"
                );
            }
            self.live.camera_logged = Vec2::ZERO;
            self.live.logged_at = now;
        }
        camera * CAMERA_PER_POINT
    }

    /// Whatever else the frame needs, then this frame's HUD into `list`.
    pub fn draw(&mut self, target: (u32, u32), list: &mut UiDrawList) {
        let stopwatch = Stopwatch::start();
        let metrics = self.window_metrics.unwrap_or_else(|| {
            let insets = self.options.insets.unwrap_or_default();
            UiMetrics::of_pixels(target, self.options.scale, insets)
        });
        self.relayout(metrics);
        match self.options.mode {
            HudMode::Fixed => {
                // The same state, at the same moment: every frame costs what a frame costs, and
                // draws the same.
                self.now = ms(FIXED_NOW);
            }
            HudMode::Live => {
                self.values = match self.fight {
                    Some(values) => values,
                    None => self.live_values(),
                };
            }
        }
        let actions = if self.options.mode == HudMode::Fixed {
            self.controls.update(self.now)
        } else {
            self.controls.actions()
        };
        self.hud.update(self.now, &self.values, actions);
        self.last_stats = self.hud.draw(&self.layout, actions, list);
        self.last_cost = stopwatch.elapsed();
    }

    /// What the last frame's draw cost, for the bench and the stats overlay (rule R9).
    pub fn last_frame(&self) -> (HudStats, Duration) {
        (self.last_stats, self.last_cost)
    }

    fn live_values(&mut self) -> HudValues {
        let cooldown = match self.live.cooldown_until {
            Some(until) if until > self.now => {
                let left = until.saturating_duration_since(self.now);
                (left.as_secs_f32() / LIVE_COOLDOWN.as_secs_f32(), Some(left.as_secs() as u32 + 1))
            }
            _ => {
                self.live.cooldown_until = None;
                (0.0, None)
            }
        };
        HudValues {
            player: FighterValues {
                hp: self.live.player_hp,
                poise: 0.7,
                hp_points: Some((self.live.player_hp * 6480.0) as u32),
            },
            enemy: Some(FighterValues {
                hp: self.live.enemy_hp,
                poise: self.live.enemy_poise,
                hp_points: None,
            }),
            skill: SkillValues { cooldown: cooldown.0, seconds: cooldown.1 },
        }
    }

    /// A new layout, if the metrics moved. A fixed frame poses itself again on it.
    fn relayout(&mut self, metrics: UiMetrics) {
        if self.metrics == Some(metrics) {
            return;
        }
        self.metrics = Some(metrics);
        self.layout = HudLayout::new(metrics);
        tracing::info!(target: LOG_TARGET, size = %metrics.size, insets = ?metrics.insets, scale = self.layout.scale, "hud layout");
        match self.options.mode {
            HudMode::Fixed => self.pose_fixed(),
            HudMode::Live => self.controls.set_layout(self.layout),
        }
    }

    /// The fixed frame: a hit just landed, a thumb on the stick, another dragging a held attack.
    fn pose_fixed(&mut self) {
        let layout = self.layout;
        self.controls = TouchControls::new(layout);
        self.hud = CombatHud::with_atlas(self.atlas.clone());
        let before = HudValues {
            player: FighterValues { hp: 0.78, poise: 0.82, hp_points: Some(5054) },
            enemy: Some(FighterValues { hp: 0.83, poise: 0.35, hp_points: None }),
            skill: SkillValues { cooldown: 0.7, seconds: Some(5) },
        };
        let after = HudValues {
            player: FighterValues { hp: 0.64, poise: 0.46, hp_points: Some(4147) },
            enemy: Some(FighterValues { hp: 0.71, poise: 0.62, hp_points: None }),
            skill: SkillValues { cooldown: 0.62, seconds: Some(4) },
        };
        let actions = self.controls.update(ms(900));
        self.hud.update(ms(900), &before, actions);
        // Numbers on the enemy, each born at its moment: fading, floating, punching in.
        let enemy = layout.safe.center() + Vec2::new(0.0, -40.0) * layout.scale;
        let scale = layout.scale;
        self.hud.spawn_damage(356, enemy + Vec2::new(-104.0, 34.0) * scale, DamageKind::Normal);

        let rest = layout.stick.rest;
        let attack = layout.attack.center;
        self.controls.touch(ms(1000), finger(1), TouchPhase::Down, rest + Vec2::new(10.0, -8.0) * scale);
        self.controls.touch(ms(1100), finger(2), TouchPhase::Down, attack);
        self.controls.touch(ms(1250), finger(1), TouchPhase::Moved, rest + Vec2::new(44.0, -30.0) * scale);
        self.controls.touch(ms(1300), finger(2), TouchPhase::Moved, attack + Vec2::new(-16.0, -30.0) * scale);
        let actions = self.controls.update(ms(1000));
        self.hud.update(ms(1000), &before, actions);
        self.hud.spawn_damage(2365, enemy + Vec2::new(112.0, 2.0) * scale, DamageKind::Normal);
        let actions = self.controls.update(ms(1400));
        self.hud.update(ms(1400), &after, actions);
        self.hud.spawn_damage(1284, enemy + Vec2::new(0.0, -12.0) * scale, DamageKind::Heavy);
        let actions = self.controls.update(ms(FIXED_NOW));
        self.hud.update(ms(FIXED_NOW), &after, actions);
        self.values = after;
    }
}

/// `value`, but starting over from full once it has run out: a stand-in's bars never stay empty.
fn wrapped(value: f32) -> f32 {
    if value <= 0.0 { 1.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With a fight on, live mode's bars are the fight's, on the fight's clock where there's no other.
    #[test]
    fn live_mode_shows_a_fights_values_in_place_of_the_stand_in() {
        let mut hud = SandboxHud::new(HudOptions { mode: HudMode::Live, ..HudOptions::default() });
        let mut list = UiDrawList::default();
        hud.draw((640, 360), &mut list);
        assert_eq!(hud.values.enemy.map(|enemy| enemy.hp), Some(1.0), "the stand-in, unhurt");

        let values = HudValues {
            player: FighterValues { hp: 0.5, poise: 0.25, hp_points: Some(150) },
            enemy: Some(FighterValues { hp: 0.75, poise: 1.0, hp_points: None }),
            skill: SkillValues::default(),
        };
        hud.show_fight(values, Duration::from_secs(2));
        hud.draw((640, 360), &mut list);
        assert_eq!(hud.values, values);
        assert_eq!(hud.now, ms(2000));
    }

    /// A fixed frame stays the same frame, whatever's going on behind it.
    #[test]
    fn a_fixed_frame_ignores_a_fight() {
        let mut hud = SandboxHud::new(HudOptions::default());
        let mut list = UiDrawList::default();
        hud.draw((640, 360), &mut list);
        let before = hud.values;
        let values = HudValues {
            player: FighterValues { hp: 0.1, ..FighterValues::default() },
            ..HudValues::default()
        };
        hud.show_fight(values, Duration::from_secs(2));
        hud.draw((640, 360), &mut list);
        assert_eq!(hud.values, before);
        assert_eq!(hud.now, ms(FIXED_NOW));
    }

    #[test]
    fn a_fixed_frame_draws_the_same_quads_every_frame() {
        let mut hud = SandboxHud::new(HudOptions::default());
        let draw = |hud: &mut SandboxHud| {
            let mut list = UiDrawList::default();
            hud.draw((640, 360), &mut list);
            list
        };
        let first = draw(&mut hud);
        assert_eq!(first.viewport, [640.0, 360.0]);
        assert!(!first.quads.is_empty());
        assert_eq!(draw(&mut hud), first);
        assert_eq!(hud.last_frame().0.quads as usize, first.quads.len());
        // A held attack past its hold and numbers in the air: the frame shows what it's for.
        assert_eq!(hud.last_frame().0.damage_numbers, 3);
        assert!(hud.controls.actions().button(Button::Attack).held);
        assert!(hud.controls.actions().button(Button::Attack).dragging);
        assert!(hud.controls.actions().stick.active);
        assert!(hud.controls.actions().stick.value.length() > 0.5);
    }

    #[test]
    fn a_fixed_frame_follows_its_target_and_takes_no_touches() {
        let mut hud = SandboxHud::new(HudOptions { scale: 3.0, ..HudOptions::default() });
        let mut list = UiDrawList::default();
        hud.draw((2622, 1206), &mut list);
        assert_eq!(list.viewport, [874.0, 402.0], "points, at 3 pixels each");
        let touch = Event::Touch(sapphire_platform::TouchEvent {
            timestamp: ms(1),
            window: sapphire_platform::WindowId::from_raw(1),
            finger: finger(9),
            phase: TouchPhase::Down,
            x: 0.5,
            y: 0.5,
            pressure: 1.0,
        });
        assert!(!hud.handle_event(&touch));
    }

    #[test]
    fn made_up_insets_move_a_fixed_frame_inside_them() {
        let insets = Insets { left: 62.0, top: 0.0, right: 62.0, bottom: 21.0 };
        let mut hud = SandboxHud::new(HudOptions { insets: Some(insets), ..HudOptions::default() });
        let mut list = UiDrawList::default();
        hud.draw((874, 402), &mut list);
        assert_eq!(hud.layout.safe.min.x, 62.0);
        assert_eq!(hud.layout.safe.max, Vec2::new(812.0, 381.0));
    }
}
