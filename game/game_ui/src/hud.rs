//! `--hud`: the combat HUD over whatever the scene draws (M2 I2), in two modes.
//!
//! - **`fixed`:** one frozen frame, the same on any device: synthetic touches on a made-up clock put
//!   a thumb on the stick and another dragging the attack, with a hit landing and numbers in the
//!   air. What the golden scenes capture. Real touches go to the app as if there were no HUD.
//! - **`live`:** real touches through the zones, on `Platform::now`. The zones say what the fingers did
//!   ([`FightHud::update`]), and the frame's input takes that through combat's bindings
//!   (`CombatControls::read_touch`), so a press and a drag reach a tick and the camera by the road a pad
//!   takes. With a fight on (`--arena`, [`FightHud::for_fight`]), its bars are the fighters'
//!   ([`FightHud::show_fight`]) and its numbers the hits that landed ([`FightHud::show_hit`]),
//!   offscreen too, where the fight's own time is its clock. With none, no gameplay stands behind the
//!   zones, so a stand-in shows that input arrives: the attack spends the enemy's bars and throws a
//!   number, a held attack a heavier one, the dodge spends the player's and the skill starts a cooldown.
//!   Every button event and each second's stick and camera are logged, so a run on a device can be read
//!   from its log.
//!
//! The renderer sees only the packet's draw list (rule R4). Offscreen modes have no window: their
//! metrics come from the target and `--hud-scale`, and `--hud-insets` makes up what a notch would
//! cover, on any platform.

use std::time::Duration;

use sapphire_gameplay::{CueEvent, DEFEATED, HitCue, POISE_BROKEN, Simulation};
use sapphire_look::UiPageState;
use sapphire_math::{Mat4, Vec2, Vec3};
use sapphire_moves::Weight;
use sapphire_platform::{Event, FingerId, Timestamp, TouchDeviceId, TouchPhase, Window};
use sapphire_time::Stopwatch;
use sapphire_ui::{
    Atlas, Button, ButtonEventKind, CombatHud, DamageKind, FighterValues, HudDesign, HudLayout, HudStats,
    HudValues, Insets, SkillValues, TouchActions, UiDrawList, UiMetrics, UltimateValues,
};

use crate::LOG_TARGET;
use crate::menus::HudModel;
use crate::screen::ScreenStack;
use crate::touch::TouchControls;
use crate::wipe::{WipeAsk, ZoneWipe};
use game_combat::fight::{Fight, Followed};
use sapphire_stage::FrameLaps;
use sapphire_stage::content::Content;
use sapphire_stage::scene::Scene;
use sapphire_stage::{ContentChanges, SceneShow};

/// How the HUD runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HudMode {
    #[default]
    Fixed,
    Live,
}

/// The screen a frame with no window stands for, as a capture's does: what a window would say of its display
/// (M4 S5's store screenshots). The default is what a capture has always drawn, one pixel a point on a touch
/// screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffscreenDisplay {
    /// Pixels per point, under the player's own scale: a phone's 3, so the interface covers as much of the
    /// frame as it does of the phone.
    pub scale: f32,
    /// A touch screen shows the on-screen controls. A desktop's shows none until a finger lands, and a pad's
    /// none at all, so a desktop's frame has none.
    pub touch: bool,
}

impl Default for OffscreenDisplay {
    fn default() -> Self {
        Self { scale: 1.0, touch: true }
    }
}

/// `--hud` and its companions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudOptions {
    pub mode: HudMode,
    /// `--hud-insets`: in place of what the window reports, in points.
    pub insets: Option<Insets>,
    /// `--hud-scale`: pixels per point in an offscreen frame, or the player's UI scale on top of
    /// the display's in a window.
    pub scale: f32,
    /// `--hud-paused`: draws a pause whatever the input says, since a capture has no input to pause it.
    pub paused: bool,
    /// `--hud-energy <share>`: the fixed frame's player has an ultimate, its energy meter this full (0 to
    /// 1), and a full one is ready. With none, the fixed frame has no ultimate, as it had none before.
    pub energy: Option<f32>,
}

impl Default for HudOptions {
    fn default() -> Self {
        Self { mode: HudMode::default(), insets: None, scale: 1.0, paused: false, energy: None }
    }
}

/// The fixed frame's moment on its made-up clock.
const FIXED_NOW: u64 = 1460;
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
}

impl Default for LiveValues {
    fn default() -> Self {
        Self { player_hp: 1.0, enemy_hp: 1.0, enemy_poise: 1.0, cooldown_until: None, hits: 0 }
    }
}

/// The HUD, its touch zones and what they show.
pub struct FightHud {
    options: HudOptions,
    /// Where its pieces go ([`Self::set_design`]).
    design: HudDesign,
    atlas: Atlas,
    layout: HudLayout,
    controls: TouchControls,
    hud: CombatHud,
    values: HudValues,
    now: Timestamp,
    /// What stands behind the zones with no fight on. A fight has none, so nothing in one can read it.
    stand_in: Option<LiveValues>,
    /// The camera's drag since it was last logged.
    camera_logged: Vec2,
    logged_at: Timestamp,
    /// What a fight says of its fighters.
    fight: Option<HudValues>,
    /// The tick whose hits it last read, in this fight.
    hits_read: Option<u64>,
    /// What an offscreen frame stands for ([`Self::stand_for`]).
    offscreen: OffscreenDisplay,
    /// A window's metrics this frame, read by [`Self::update`]; offscreen frames have none.
    window_metrics: Option<UiMetrics>,
    /// The metrics the layout was made for.
    metrics: Option<UiMetrics>,
    /// A finger has landed on the zones.
    touched: bool,
    /// Whether the on-screen controls show, as its [`CombatHud`] draws them: shown until
    /// [`Self::follow_devices`] asks, which a capture never does.
    shows_controls: bool,
    last_stats: HudStats,
    last_cost: Duration,
}

impl FightHud {
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
        Self::with_atlas(options, atlas)
    }

    /// A HUD over `atlas`, already painted and shared with what shares a frame with it: a frame's list has one
    /// atlas.
    pub fn with_atlas(options: HudOptions, atlas: Atlas) -> Self {
        let layout = HudLayout::default();
        Self {
            options,
            design: *HudDesign::shipped(),
            layout,
            controls: TouchControls::new(layout),
            hud: CombatHud::with_atlas(atlas.clone()),
            atlas,
            values: HudValues::default(),
            now: Timestamp::default(),
            stand_in: Some(LiveValues::default()),
            camera_logged: Vec2::ZERO,
            logged_at: Timestamp::default(),
            fight: None,
            hits_read: None,
            offscreen: OffscreenDisplay::default(),
            window_metrics: None,
            metrics: None,
            touched: false,
            shows_controls: true,
            last_stats: HudStats::default(),
            last_cost: Duration::ZERO,
        }
    }

    /// The HUD of a fight, which never reads a stand-in, from its first frame on.
    pub fn for_fight(options: HudOptions) -> Self {
        Self { stand_in: None, ..Self::new(options) }
    }

    /// The same, over `atlas`: the screen's that preceded it, or the one it shares a frame with.
    pub fn for_fight_with_atlas(options: HudOptions, atlas: Atlas) -> Self {
        Self { stand_in: None, ..Self::with_atlas(options, atlas) }
    }

    /// Draws each frame with no window as `display` would show it: at its pixels per point, and with no
    /// on-screen controls unless it's a touch screen.
    pub fn stand_for(&mut self, display: OffscreenDisplay) {
        self.offscreen = display;
        if !display.touch {
            self.follow_devices(true, false);
        }
    }

    /// What it draws from, for what shares its frame.
    pub const fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    /// What the fight says of its fighters this frame, `time` into it: live mode shows it. Offscreen
    /// there's no other clock.
    pub(crate) fn show_fight(&mut self, values: HudValues, time: Duration) {
        self.fight = Some(values);
        // The ultimate's and the swap's buttons take a thumb while they're drawn: a live frame draws these
        // values.
        if self.options.mode == HudMode::Live {
            self.controls.show_ultimate(values.ultimate.is_some());
            self.controls.show_swap(values.party.is_some());
        }
        self.stand_in = None;
        self.fight_time(time);
    }

    fn fight_time(&mut self, time: Duration) {
        if self.window_metrics.is_none() && self.options.mode == HudMode::Live {
            self.now = Timestamp::from_nanos(time.as_nanos() as u64);
        }
    }

    /// A new fight: the last one's numbers go with it. Offscreen they'd otherwise stay until the new
    /// fight's clock caught up with when they were born. A fixed frame's are its own.
    pub(crate) fn start_over(&mut self) {
        self.hits_read = None;
        if self.options.mode == HudMode::Live {
            self.hud.clear_damage();
        }
    }

    /// Whether the step at `tick` still has hits to read: once a tick, in a fight.
    pub(crate) fn reads_hits_of(&mut self, tick: u64) -> bool {
        self.hits_read.replace(tick) != Some(tick)
    }

    /// Someone else came in for a fighter, `[player, enemy]`.
    pub(crate) fn fighters_came_in(&mut self, came_in: [bool; 2]) {
        self.hud.fighters_came_in(came_in);
    }

    /// Whether the frame says the fight is waiting.
    pub fn show_pause(&mut self, shown: bool) {
        self.hud.show_pause(shown);
    }

    /// A platform event: whether the touch zones took it, which the rest of the input then never
    /// sees. Only live mode takes touches.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        self.options.mode == HudMode::Live && self.controls.handle_event(event)
    }

    /// Whether this finger holds the stick or a button.
    #[cfg(feature = "dev")]
    pub fn holds_control(&self, finger: FingerId) -> bool {
        self.options.mode == HudMode::Live && self.controls.holds_control(finger)
    }

    /// `window`'s metrics, as this HUD draws on it.
    pub fn metrics_of(&self, window: &Window) -> UiMetrics {
        let mut metrics = UiMetrics::of_window(window, self.options.scale);
        if let Some(insets) = self.options.insets {
            metrics.insets = insets;
        }
        metrics
    }

    /// The player's UI size, from the next frame.
    pub fn set_scale(&mut self, scale: f32) {
        self.options.scale = scale;
    }

    /// The player's UI size.
    #[must_use]
    pub const fn scale(&self) -> f32 {
        self.options.scale
    }

    /// The player's text size (M4 A2), from the next frame: see [`CombatHud::set_text_scale`].
    pub fn set_text_scale(&mut self, scale: f32) {
        self.hud.set_text_scale(scale);
    }

    /// The player's text size, as [`Self::set_text_scale`] last said.
    #[must_use]
    pub const fn text_scale(&self) -> f32 {
        self.hud.text_scale()
    }

    /// Whether the on-screen controls show, as [`Self::follow_devices`] last said.
    #[must_use]
    pub const fn shows_controls(&self) -> bool {
        self.shows_controls
    }

    /// Who's holding what, for whether the on-screen controls show. A pad in use takes them away and a
    /// finger brings them back, and a desktop shows them only once a finger has landed. A pad merely
    /// connected changes nothing: the iOS simulator reports one nobody holds, and with no controls and
    /// no pad there'd be no way to play. Nothing that doesn't ask has them hidden.
    pub fn follow_devices(&mut self, desktop: bool, pad_in_hands: bool) {
        let touching = self.controls.fingers() > 0;
        self.touched |= touching;
        self.shows_controls = touching || (!pad_in_hands && (!desktop || self.touched));
        self.hud.show_controls(self.shows_controls);
    }

    /// Live mode's frame on the real clock, before the frame's input is judged: the window's metrics,
    /// the touch zones and the stand-in. Returns what the zones read, which that input takes; a fixed
    /// frame has none, since its touches are made up.
    pub fn update(&mut self, now: Timestamp, metrics: UiMetrics) -> Option<&TouchActions> {
        self.window_metrics = Some(metrics);
        if self.options.mode != HudMode::Live {
            return None;
        }
        self.now = now;
        self.relayout(metrics);
        let actions = self.controls.update(now);
        self.camera_logged += actions.camera_delta;
        for event in &actions.events {
            if let Some(stand_in) = &mut self.stand_in {
                let enemy = self.layout.metrics.size * Vec2::new(0.5, 0.42);
                stand_in.take(event.button, event.kind, now, enemy, &mut self.hud);
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
        if now.saturating_duration_since(self.logged_at) >= LOG_EVERY {
            let stick = actions.stick;
            if stick.active || self.camera_logged != Vec2::ZERO {
                tracing::info!(
                    target: LOG_TARGET,
                    stick = %stick.value,
                    thumb = %stick.thumb,
                    camera = %self.camera_logged,
                    fingers = self.controls.fingers(),
                    "hud touch"
                );
            }
            self.camera_logged = Vec2::ZERO;
            self.logged_at = now;
        }
        Some(self.controls.actions())
    }

    /// A hit that landed: its number where `view_projection` puts `point`, if that's in front of the
    /// camera. Only live mode shows them: a fixed frame's numbers are its own.
    pub(crate) fn show_hit(&mut self, damage: u32, point: Vec3, weight: Weight, view_projection: Mat4) {
        if self.options.mode != HudMode::Live {
            return;
        }
        let Some(position) = on_screen(point, view_projection, self.layout.metrics.size) else { return };
        let kind = if weight == Weight::Heavy { DamageKind::Heavy } else { DamageKind::Normal };
        self.hud.spawn_damage_at(self.now, damage, position, kind);
    }

    /// Whatever else the frame needs, then this frame's HUD into `list`.
    pub fn draw(&mut self, target: (u32, u32), list: &mut UiDrawList) {
        let stopwatch = Stopwatch::start();
        let metrics = self.window_metrics.unwrap_or_else(|| {
            let insets = self.options.insets.unwrap_or_default();
            UiMetrics::of_pixels(target, self.offscreen.scale * self.options.scale, insets)
        });
        self.relayout(metrics);
        match self.options.mode {
            HudMode::Fixed => {
                // The same state, at the same moment: every frame costs what a frame costs, and
                // draws the same.
                self.now = ms(FIXED_NOW);
            }
            HudMode::Live => {
                self.values = match (self.fight, &mut self.stand_in) {
                    (Some(values), _) => values,
                    (None, Some(stand_in)) => stand_in.values(self.now),
                    // A fight that hasn't said anything yet: the app tells its HUD before the first frame.
                    (None, None) => HudValues::default(),
                };
            }
        }
        let actions = if self.options.mode == HudMode::Fixed {
            self.controls.update(self.now)
        } else {
            self.controls.actions()
        };
        self.hud.update(self.now, &self.values, actions);
        if self.options.paused {
            self.hud.show_pause(true);
        }
        self.last_stats = self.hud.draw(&self.layout, actions, list);
        self.last_cost = stopwatch.elapsed();
    }

    /// What the last frame's draw cost, for the bench and the stats overlay (rule R9).
    pub fn last_frame(&self) -> (HudStats, Duration) {
        (self.last_stats, self.last_cost)
    }

    /// Where its pieces are.
    pub fn layout(&self) -> &HudLayout {
        &self.layout
    }

    /// Lays the HUD out by `design` from now on: a game's own, or an edit of it. A fixed frame poses itself
    /// again on it.
    pub fn set_design(&mut self, design: HudDesign) {
        self.design = design;
        if let Some(metrics) = self.metrics.take() {
            self.relayout(metrics);
        }
    }

    /// A new layout, if the metrics moved. A fixed frame poses itself again on it.
    fn relayout(&mut self, metrics: UiMetrics) {
        if self.metrics == Some(metrics) {
            return;
        }
        self.metrics = Some(metrics);
        self.layout = HudLayout::designed(metrics, &self.design);
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
        self.controls.show_ultimate(self.options.energy.is_some());
        let text_scale = self.hud.text_scale();
        self.hud = CombatHud::with_atlas(self.atlas.clone());
        self.hud.set_text_scale(text_scale);
        let before = HudValues {
            player: FighterValues { hp: 0.78, poise: 0.82, hp_points: Some(5054) },
            enemy: Some(FighterValues { hp: 0.83, poise: 0.35, hp_points: None }),
            skill: SkillValues { cooldown: 0.7, seconds: Some(5) },
            ultimate: self.options.energy.map(|energy| UltimateValues { energy, ready: energy >= 1.0 }),
            ..HudValues::default()
        };
        let after = HudValues {
            player: FighterValues { hp: 0.64, poise: 0.46, hp_points: Some(4147) },
            enemy: Some(FighterValues { hp: 0.71, poise: 0.62, hp_points: None }),
            skill: SkillValues { cooldown: 0.62, seconds: Some(4) },
            ..before
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

impl LiveValues {
    /// A button's edge, on the stand-in: what no gameplay is there to do.
    fn take(
        &mut self,
        button: Button,
        kind: ButtonEventKind,
        now: Timestamp,
        enemy: Vec2,
        hud: &mut CombatHud,
    ) {
        match (button, kind) {
            (Button::Attack, ButtonEventKind::Pressed) => {
                self.hits += 1;
                self.enemy_hp = wrapped(self.enemy_hp - 0.06);
                self.enemy_poise = wrapped(self.enemy_poise - 0.11);
                hud.spawn_damage_at(now, 240 + self.hits * 37 % 900, enemy, DamageKind::Normal);
            }
            (Button::Attack, ButtonEventKind::Held) => {
                let value = 2400 + self.hits * 311 % 4000;
                hud.spawn_damage_at(now, value, enemy + Vec2::new(0.0, -24.0), DamageKind::Heavy);
            }
            (Button::Dodge, ButtonEventKind::Pressed) => self.player_hp = wrapped(self.player_hp - 0.09),
            (Button::Skill, ButtonEventKind::Pressed) if self.cooldown_until.is_none() => {
                self.cooldown_until =
                    Some(Timestamp::from_nanos(now.as_nanos() + LIVE_COOLDOWN.as_nanos() as u64));
            }
            _ => {}
        }
    }

    /// What it shows at `now`.
    fn values(&mut self, now: Timestamp) -> HudValues {
        let cooldown = match self.cooldown_until {
            Some(until) if until > now => {
                let left = until.saturating_duration_since(now);
                (left.as_secs_f32() / LIVE_COOLDOWN.as_secs_f32(), Some(left.as_secs() as u32 + 1))
            }
            _ => {
                self.cooldown_until = None;
                (0.0, None)
            }
        };
        HudValues {
            player: FighterValues {
                hp: self.player_hp,
                poise: 0.7,
                hp_points: Some((self.player_hp * 6480.0) as u32),
            },
            enemy: Some(FighterValues { hp: self.enemy_hp, poise: self.enemy_poise, hp_points: None }),
            skill: SkillValues { cooldown: cooldown.0, seconds: cooldown.1 },
            ..HudValues::default()
        }
    }
}

/// The hit this cue says landed, once for each hit: what a broken guard or a defeat publishes after a
/// hit's own cue carries the same hit, and doesn't land it again.
pub fn landed(cue: &CueEvent) -> Option<HitCue> {
    cue.hit.filter(|_| cue.cue != POISE_BROKEN && cue.cue != DEFEATED)
}

/// Where `point` draws in a frame `size` points large, +y down, if it's in front of the camera. It may
/// be off the frame's edge: a number that starts there floats in.
fn on_screen(point: Vec3, view_projection: Mat4, size: Vec2) -> Option<Vec2> {
    let clip = view_projection * point.extend(1.0);
    (clip.w > 0.0).then(|| {
        let ndc = Vec2::new(clip.x, clip.y) / clip.w;
        Vec2::new((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5) * size
    })
}

/// `value`, but starting over from full once it has run out: a stand-in's bars never stay empty.
fn wrapped(value: f32) -> f32 {
    if value <= 0.0 { 1.0 } else { value }
}

/// A fight with its HUD over it, as one show. With no HUD, it's the fight.
#[derive(Default)]
pub struct ShownFight {
    pub fight: Fight,
    /// The combat HUD over the scene (`--hud`, M2 I2).
    pub hud: Option<FightHud>,
    /// Retained screens under the HUD (M3 U4), which only the sandbox hangs here: a game's menus take
    /// input, and its show holds them.
    pub screen: Option<ScreenStack>,
    /// Whatever else an app draws over the frame, after the screens and the HUD: its interface, which
    /// hears only of the interface ([`SceneShow::declare_pipelines`], [`SceneShow::begin_interface`]
    /// and [`SceneShow::draw_interface`]). The sandbox's display text sample (M3 U8), which draws
    /// straight from the shaper with no document, is one.
    pub over: Option<Box<dyn SceneShow>>,
    /// What covers a zone trip's load, over all of it ([`crate::wipe`], M3 W4). Install it with
    /// [`Self::wipe_trips`].
    pub wipe: Option<ZoneWipe>,
    /// What a game's flow asks of the wipe while no trip does, and the real time it's had since the wipe
    /// last took it (M3 G1): see [`Self::ask_wipe`].
    pub flow_wipe: (Option<WipeAsk>, f32),
}

impl ShownFight {
    /// Covers the fight's zone trips with `wipe` from now on: each waits for it to cover and to clear.
    pub fn wipe_trips(&mut self, wipe: ZoneWipe) {
        self.fight.show.take_trip_wipe();
        self.wipe = Some(wipe);
    }

    /// What a game's flow asks of the wipe this frame, `dt` real seconds after the last, which it follows
    /// while no trip asks anything of it (M3 G1). Once a frame, before the stage's frame.
    pub fn ask_wipe(&mut self, ask: Option<WipeAsk>, dt: f32) {
        self.flow_wipe = (ask, self.flow_wipe.1 + dt);
    }

    /// Tells the HUD what a step of the fight did.
    pub fn hear(&mut self, followed: Followed) {
        let Some(hud) = &mut self.hud else { return };
        hud.fighters_came_in(followed.came_in);
        if followed.started_over {
            hud.start_over();
        }
    }

    /// The hits the latest step landed, as the HUD's numbers, once: a run that's stopped takes no more
    /// steps, and its last one's cues stand.
    pub fn show_damage(&mut self, scene: &Scene) {
        let (Some(hud), Some(combat)) = (&mut self.hud, &self.fight.combat) else { return };
        show_damage(hud, scene, combat.simulation());
    }
}

/// Shows the hits `sim`'s latest step landed, once however often it's read.
pub fn show_damage(hud: &mut FightHud, scene: &Scene, sim: &Simulation) {
    // Offscreen events arrive before the next draw updates the HUD's clock.
    hud.fight_time(Duration::from_nanos(
        sim.tick().saturating_mul(1_000_000_000 / u64::from(sapphire_time::TICK_HZ)),
    ));
    if !hud.reads_hits_of(sim.tick()) {
        return;
    }
    let camera = scene.camera().camera_data(scene.view_aspect());
    let view_projection = camera.proj * camera.view;
    for cue in sim.cues() {
        let Some(hit) = landed(&cue) else { continue };
        hud.show_hit(hit.damage, hit.point, hit.weight, view_projection);
        // So a run on a device can be read from its log, as the HUD's buttons are.
        tracing::info!(
            target: LOG_TARGET,
            tick = cue.tick,
            attacker = ?cue.actor,
            target_actor = ?hit.target,
            damage = hit.damage,
            outcome = ?hit.outcome,
            "hit landed"
        );
    }
}

impl SceneShow for ShownFight {
    fn declare_pipelines(&self, inventory: &mut sapphire_render::PipelineInventory) {
        self.fight.declare_pipelines(inventory);
        inventory.ui |= self.hud.is_some() || self.screen.is_some() || self.wipe.is_some();
        inventory.insets |= self.screen.as_ref().is_some_and(ScreenStack::holds_insets);
        if let Some(over) = &self.over {
            over.declare_pipelines(inventory);
        }
    }

    fn step(&mut self, scene: &mut Scene, laps: &mut FrameLaps) {
        let followed = self.fight.step_alone(scene, laps);
        self.hear(followed);
    }

    fn tier_changed(&mut self, scene: &mut Scene, tier: sapphire_render::quality::TierId) {
        self.fight.tier_changed(scene, tier);
    }

    fn end_steps(&mut self, scene: &mut Scene, dt: f32) {
        self.fight.end_steps(scene, dt);
    }

    fn follow_content(&mut self, scene: &mut Scene, content: &mut Content, changes: ContentChanges) {
        if let Some(wipe) = &mut self.wipe {
            let show = &mut self.fight.show;
            let (flow, flow_time) = std::mem::take(&mut self.flow_wipe);
            match show.trip_phase() {
                Some(phase) => wipe.follow(Some(WipeAsk::of_trip(phase)), show.take_trip_time()),
                None => wipe.follow(flow, flow_time),
            }
            show.say_trip_wipe(wipe.covered(), wipe.clear());
            wipe.follow_warm_up(content.warm_up());
        }
        self.fight.follow_content(scene, content, changes);
    }

    fn animate(&mut self, scene: &mut Scene, content: &Content, alpha: f32) {
        self.fight.animate(scene, content, alpha);
    }

    fn begin_interface(&mut self, pages: UiPageState) {
        if let Some(screen) = &mut self.screen {
            screen.follow_pages(pages);
        }
        if let Some(over) = &mut self.over {
            over.begin_interface(pages);
        }
        if let Some(wipe) = &mut self.wipe {
            wipe.follow_pages(pages);
        }
    }

    fn present(&mut self, scene: &mut Scene, alpha: f32, aspect: Option<f32>) {
        self.fight.present(scene, alpha, aspect);
    }

    fn write_effects(&mut self, frame: &mut sapphire_render::VfxFrame, alpha: f32) {
        self.fight.write_effects(frame, alpha);
    }

    fn adjust_post(&mut self, post: &mut sapphire_render::PostParams) {
        self.fight.adjust_post(post);
    }

    fn draw_interface(&mut self, size: (u32, u32), list: &mut UiDrawList) {
        if let Some(screen) = &mut self.screen {
            // The HUD's document over a fight says what the fight says, its foe's name and its party's slots, as
            // a game's show has it say them.
            if let Some((values, _)) = self.fight.show.hud_values() {
                let foe_name = self.fight.show.foe_name();
                for index in 0..screen.len() {
                    if let Some(model) = screen.model_mut::<HudModel>(index) {
                        model.follow_fight(Some(&values), foe_name);
                    }
                }
            }
            screen.draw(size, list);
        }
        // A cutscene has the screen: its subtitles and its way out are the HUD document's.
        if let Some(hud) = &mut self.hud
            && !self.fight.show.plays_cutscene()
        {
            if let Some((values, time)) = self.fight.show.hud_values() {
                hud.show_fight(values, time);
            }
            hud.draw(size, list);
        }
        if let Some(over) = &mut self.over {
            over.draw_interface(size, list);
        }
        if let Some(wipe) = &mut self.wipe {
            wipe.draw(size, list);
        }
    }

    fn inset_shots(&self, visit: &mut dyn FnMut(sapphire_stage::InsetShot<'_>)) {
        if let Some(screen) = &self.screen {
            screen.inset_shots(visit);
        }
    }

    fn draw_debug(&mut self) {
        self.fight.draw_debug();
    }
}

#[cfg(test)]
mod fight_tests;
#[cfg(test)]
mod tests {
    use super::*;

    /// With a fight on, live mode's bars are the fight's, on the fight's clock where there's no other.
    #[test]
    fn live_mode_shows_a_fights_values_in_place_of_the_stand_in() {
        let mut hud = FightHud::new(HudOptions { mode: HudMode::Live, ..HudOptions::default() });
        let mut list = UiDrawList::default();
        hud.draw((640, 360), &mut list);
        assert_eq!(hud.values.enemy.map(|enemy| enemy.hp), Some(1.0), "the stand-in, unhurt");

        let values = HudValues {
            player: FighterValues { hp: 0.5, poise: 0.25, hp_points: Some(150) },
            enemy: Some(FighterValues { hp: 0.75, poise: 1.0, hp_points: None }),
            skill: SkillValues::default(),
            ..HudValues::default()
        };
        hud.show_fight(values, Duration::from_secs(2));
        hud.draw((640, 360), &mut list);
        assert_eq!(hud.values, values);
        assert_eq!(hud.now, ms(2000));
    }

    fn touch(id: u64, phase: TouchPhase, at_ms: u64, position: Vec2, size: Vec2) -> Event {
        Event::Touch(sapphire_platform::TouchEvent {
            timestamp: ms(at_ms),
            window: sapphire_platform::WindowId::from_raw(1),
            finger: finger(id),
            phase,
            x: position.x / size.x,
            y: position.y / size.y,
            pressure: 1.0,
        })
    }

    /// A fight's HUD has no stand-in to read, from before its first values on: a press throws no number,
    /// and its bars are what the fight says or nothing.
    #[test]
    fn a_fights_hud_reads_no_stand_in_from_its_first_frame() {
        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        let mut hud = FightHud::for_fight(live);
        let metrics = UiMetrics::new(Vec2::new(900.0, 400.0), Insets::default());
        assert!(hud.update(ms(1), metrics).is_some());
        let attack = hud.layout.attack.center;
        assert!(hud.handle_event(&touch(1, TouchPhase::Down, 2, attack, metrics.size)));
        // Past its hold, whose number is the stand-in's too.
        assert!(hud.update(ms(500), metrics).is_some_and(|touch| touch.button(Button::Attack).held));
        let mut list = UiDrawList::default();
        hud.draw((900, 400), &mut list);
        assert_eq!(hud.last_frame().0.damage_numbers, 0);
        assert_eq!(hud.values, HudValues::default());

        // The stand-in stops at a fight's first word, on a HUD that started without one.
        let mut hud = FightHud::new(live);
        hud.show_fight(HudValues::default(), Duration::ZERO);
        assert!(hud.stand_in.is_none());
    }

    /// The numbers are hits, where the camera sees them: the middle of the frame for what it looks at,
    /// +y down, nothing for what's behind it.
    #[test]
    fn a_hit_shows_its_number_where_the_camera_sees_it() {
        let camera = sapphire_render::CameraData::look_at(
            Vec3::new(0.0, 1.0, 5.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::Y,
            1.0,
            900.0 / 400.0,
            0.05,
        );
        let view_projection = camera.proj * camera.view;
        let size = Vec2::new(900.0, 400.0);
        let middle = on_screen(Vec3::new(0.0, 1.0, 0.0), view_projection, size).expect("in front");
        assert!(middle.distance(size * 0.5) < 1e-3, "{middle}");
        let above = on_screen(Vec3::new(0.0, 2.0, 0.0), view_projection, size).expect("in front");
        assert!(above.y < middle.y && above.x == middle.x, "{above}");
        let right = on_screen(Vec3::new(1.0, 1.0, 0.0), view_projection, size).expect("in front");
        assert!(right.x > middle.x, "{right}");
        assert_eq!(on_screen(Vec3::new(0.0, 1.0, 9.0), view_projection, size), None, "behind");

        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        let mut hud = FightHud::for_fight(live);
        let metrics = UiMetrics::new(size, Insets::default());
        let _ = hud.update(ms(1), metrics);
        hud.show_hit(30, Vec3::new(0.0, 1.0, 0.0), Weight::Light, view_projection);
        hud.show_hit(80, Vec3::new(0.0, 2.0, 0.0), Weight::Heavy, view_projection);
        hud.show_hit(80, Vec3::new(0.0, 1.0, 9.0), Weight::Heavy, view_projection);
        let mut list = UiDrawList::default();
        hud.draw((900, 400), &mut list);
        assert_eq!(hud.last_frame().0.damage_numbers, 2, "and none from behind");

        // A fixed frame's numbers are its own.
        let mut fixed = FightHud::new(HudOptions::default());
        fixed.draw((640, 360), &mut list);
        let before = fixed.last_frame().0.damage_numbers;
        fixed.show_hit(30, Vec3::new(0.0, 1.0, 0.0), Weight::Light, view_projection);
        fixed.draw((640, 360), &mut list);
        assert_eq!(fixed.last_frame().0.damage_numbers, before);
    }

    #[test]
    fn hits_use_the_event_frames_time_after_a_gap_in_drawing() {
        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        for windowed in [false, true] {
            let mut hud = FightHud::for_fight(live);
            let mut list = UiDrawList::default();
            hud.draw((640, 360), &mut list);
            if windowed {
                hud.update(ms(10_000), UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
            } else {
                hud.show_fight(HudValues::default(), Duration::from_secs(10));
            }
            hud.show_hit(30, Vec3::ZERO, Weight::Light, Mat4::IDENTITY);
            list.reset();
            hud.draw((640, 360), &mut list);
            assert_eq!(hud.last_frame().0.damage_numbers, 1, "a new hit survives its first draw");
            if windowed {
                hud.update(ms(12_000), UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
            } else {
                hud.show_fight(HudValues::default(), Duration::from_secs(12));
            }
            hud.draw((640, 360), &mut list);
            assert_eq!(hud.last_frame().0.damage_numbers, 0, "it still expires on its clock");
        }
    }

    /// A pad takes the on-screen controls away (spec §6.6), and a desktop shows them only once a finger
    /// has landed. The bars stay, and nothing that doesn't ask has them hidden: captures draw them.
    #[test]
    fn the_on_screen_controls_follow_who_is_holding_what() {
        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        let metrics = UiMetrics::new(Vec2::new(900.0, 400.0), Insets::default());
        let quads = |hud: &mut FightHud| {
            let mut list = UiDrawList::default();
            hud.draw((900, 400), &mut list);
            hud.last_frame().0.quads
        };
        let mut hud = FightHud::for_fight(live);
        let _ = hud.update(ms(1), metrics);
        let whole = quads(&mut hud);

        hud.follow_devices(false, false);
        assert_eq!(quads(&mut hud), whole, "a phone");
        hud.follow_devices(false, true);
        let bars = quads(&mut hud);
        assert!(bars > 0 && bars < whole, "a phone with a pad in use: {bars} of {whole}");
        hud.follow_devices(true, false);
        assert_eq!(quads(&mut hud), bars, "a desktop no finger has touched");

        let attack = hud.layout.attack.center;
        assert!(hud.handle_event(&touch(1, TouchPhase::Down, 2, attack, metrics.size)));
        hud.follow_devices(true, true);
        let _ = hud.update(ms(3), metrics);
        assert!(quads(&mut hud) >= whole, "a finger down shows them, pad or no pad");
    }

    /// A fixed frame stays the same frame, whatever's going on behind it.
    #[test]
    fn a_fixed_frame_ignores_a_fight() {
        let mut hud = FightHud::new(HudOptions::default());
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
        let mut hud = FightHud::new(HudOptions::default());
        let draw = |hud: &mut FightHud| {
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
        let mut hud = FightHud::new(HudOptions { scale: 3.0, ..HudOptions::default() });
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
        let mut hud = FightHud::new(HudOptions { insets: Some(insets), ..HudOptions::default() });
        let mut list = UiDrawList::default();
        hud.draw((874, 402), &mut list);
        assert_eq!(hud.layout.safe.min.x, 62.0);
        assert_eq!(hud.layout.safe.max, Vec2::new(812.0, 381.0));
    }
}
