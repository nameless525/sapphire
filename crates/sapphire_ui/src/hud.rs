//! The combat HUD: its layout, its values and how it draws.
//!
//! # The look
//!
//! Nothing here is a generic mobile circle. Shapes lean: bars, digits and buttons share one slant,
//! so the whole frame reads as moving forward. Buttons are near-square with two corners cut hard,
//! inked nearly black with a crisp pale edge and a white glyph, so they read over a bright sky or a
//! dark street alike. Color is rationed and always means something: mint for the player's health,
//! rose for the enemy's, amber for poise and anything charging, ember for health just lost. Damage
//! numbers are large, heavy and slanted with a dark outline, and they punch in and float off.
//!
//! # Time
//!
//! Everything animates on the real clock: hitstop and slow motion freeze the world, never the
//! interface. The crate reads no clock (rule R2): the app passes
//! [`Platform::now`](sapphire_platform::Platform::now) to [`CombatHud::update`], and a test or a
//! capture passes any time it likes, which makes a frame's animation exact and repeatable.

use std::sync::Arc;
use std::time::Duration;

use sapphire_look::{UiAtlas, UiDrawList};
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;

use crate::atlas::Atlas;
use crate::layout::{Anchor, Rect, UiMetrics};
use crate::paint::{Align, Paint, Painter, faded};
use crate::touch::{Button, ButtonState, StickState, TouchActions};

/// Every shape's lean: points to the right per point of height, going up.
pub const SLANT: f32 = 0.3;

const INK: [u8; 4] = [9, 12, 20, 235];
const PALE: [u8; 4] = [226, 234, 240, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const MINT: [u8; 4] = [96, 243, 212, 255];
const ROSE: [u8; 4] = [255, 66, 92, 255];
const AMBER: [u8; 4] = [255, 192, 56, 255];
const EMBER: [u8; 4] = [255, 112, 58, 255];
const SHADOW: [u8; 4] = [0, 0, 0, 170];
const OUTLINE: [u8; 4] = [8, 10, 18, 255];

/// What a pause lays over the frame. Blending is in linear light, so it takes this much to read as
/// waiting at a glance, with what's held still there underneath.
const VEIL: [u8; 4] = [6, 8, 14, 190];
/// The pause mark's bars, in points: how long, how wide, and how far each stands from the middle.
const PAUSE_BAR_LENGTH: f32 = 76.0;
const PAUSE_BAR_WIDTH: f32 = 24.0;
const PAUSE_BAR_APART: f32 = 24.0;

/// A phone's short side in points, below which the whole HUD shrinks to fit (a small window).
const FULL_HEIGHT: f32 = 330.0;
const FULL_WIDTH: f32 = 470.0;
/// Space between the safe area's edge and what sits on it.
const MARGIN: f32 = 16.0;

/// Damage numbers alive at once: a new one past this replaces the oldest.
pub const MAX_DAMAGE_NUMBERS: usize = 16;
/// How long a damage number lives on the real clock.
const DAMAGE_LIFE: f32 = 0.95;
/// How long its punch-in takes.
const DAMAGE_POP: f32 = 0.14;
/// How far it floats, in points.
const DAMAGE_RISE: f32 = 30.0;
/// The share of its life it stays whole before fading.
const DAMAGE_WHOLE: f32 = 0.55;

/// How long a health bar's trailing segment holds before it catches up, in seconds.
const TRAIL_HOLD: f32 = 0.35;
/// How fast it catches up, in bars a second.
const TRAIL_RATE: f32 = 0.7;
/// How long a button's press eases, in seconds.
const PRESS_EASE: f32 = 0.07;
/// How long the skill's ready flash lasts, in seconds.
const READY_FLASH: f32 = 0.45;
/// How much of the skill's icon shows while it cools down.
const COOLING_ICON: f32 = 0.3;

/// A button's place: where it draws and where a finger takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ButtonLayout {
    pub center: Vec2,
    /// Half the drawn shape's side, in points.
    pub half: f32,
    /// The radius a landing takes it within, in points: larger than the shape, for a thumb.
    pub hit: f32,
}

impl ButtonLayout {
    /// The drawn shape, ignoring its slant and press.
    pub fn rect(&self) -> Rect {
        Rect::around(self.center, Vec2::splat(self.half))
    }
}

/// The floating stick's place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StickLayout {
    /// Where it rests with no thumb on it.
    pub rest: Vec2,
    /// How far from its middle the thumb reads as full, in points.
    pub radius: f32,
    /// Where a landing takes it.
    pub zone: Rect,
}

/// One fighter's bars.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BarsLayout {
    pub hp: Rect,
    pub poise: Rect,
    /// Where the health in points sits, by `digits_align` and on its vertical middle.
    pub digits: Vec2,
    pub digits_align: Align,
    pub digit_height: f32,
}

/// Where everything goes, from the frame's metrics. All in points, all inside the safe area,
/// pinned to its corners and edges so each keeps its distance from them at any aspect ratio. It is
/// plain data, and [`TouchControls`](crate::TouchControls) hit-tests against the same numbers it
/// draws from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudLayout {
    pub metrics: UiMetrics,
    pub safe: Rect,
    pub stick: StickLayout,
    pub attack: ButtonLayout,
    pub dodge: ButtonLayout,
    pub skill: ButtonLayout,
    pub player: BarsLayout,
    pub enemy: BarsLayout,
    /// Everything scales by this: 1 unless the frame is smaller than a phone.
    pub scale: f32,
}

impl HudLayout {
    pub fn new(metrics: UiMetrics) -> Self {
        let safe = metrics.safe();
        let scale = (safe.height() / FULL_HEIGHT).min(safe.width() / FULL_WIDTH).clamp(0.01, 1.0);
        let points = |value: f32| value * scale;

        // The right thumb's cluster: the attack in the corner, the others on an arc around it.
        let attack_half = points(46.0);
        let attack_center = Anchor::BOTTOM_RIGHT.place(safe, Vec2::splat(-(points(MARGIN) + attack_half)));
        let attack = ButtonLayout { center: attack_center, half: attack_half, hit: points(62.0) };
        let around = |x: f32, y: f32, half: f32| ButtonLayout {
            center: attack_center + Vec2::new(points(x), points(y)),
            half: points(half),
            hit: points(38.0),
        };
        let dodge = around(-22.0, -100.0, 30.0);
        let skill = around(-104.0, -30.0, 30.0);

        // The left thumb's zone: the frame's left side, under the bars.
        let radius = points(52.0);
        let rest = Anchor::BOTTOM_LEFT.place(safe, Vec2::new(points(92.0), -points(84.0)));
        let zone = Rect::new(
            Vec2::new(safe.min.x, safe.min.y + safe.height() * 0.28),
            Vec2::new(safe.min.x + safe.width() * 0.46, safe.max.y),
        );

        // The bars, on the safe area's top: the player's in the corner, the enemy's in what's left
        // of the middle, so they can't meet at 4:3.
        let top_margin = points(14.0);
        let player_width = (safe.width() * 0.27).min(points(210.0));
        let left = Anchor::TOP_LEFT.place(safe, Vec2::new(points(MARGIN), top_margin));
        let player = BarsLayout {
            hp: Rect::new(left, left + Vec2::new(player_width, points(13.0))),
            poise: Rect::new(
                left + Vec2::new(0.0, points(19.0)),
                left + Vec2::new(player_width * 0.72, points(25.0)),
            ),
            digits: left + Vec2::new(0.0, points(40.0)),
            digits_align: Align::Left,
            digit_height: points(16.0),
        };
        let free = safe.width() - 2.0 * (points(MARGIN) + player_width + points(26.0));
        let enemy_width = free.min(points(360.0)).max(0.0);
        let top = Anchor::TOP_CENTER.place(safe, Vec2::new(-enemy_width * 0.5, top_margin));
        let enemy = BarsLayout {
            hp: Rect::new(top, top + Vec2::new(enemy_width, points(11.0))),
            poise: Rect::new(
                top + Vec2::new(enemy_width * 0.14, points(17.0)),
                top + Vec2::new(enemy_width * 0.86, points(23.0)),
            ),
            digits: top + Vec2::new(enemy_width * 0.5, points(38.0)),
            digits_align: Align::Center,
            digit_height: points(15.0),
        };

        Self {
            metrics,
            safe,
            stick: StickLayout { rest, radius, zone },
            attack,
            dodge,
            skill,
            player,
            enemy,
            scale,
        }
    }

    /// `button`'s place.
    pub fn button(&self, button: Button) -> &ButtonLayout {
        match button {
            Button::Attack => &self.attack,
            Button::Dodge => &self.dodge,
            Button::Skill => &self.skill,
        }
    }
}

/// One fighter's values, plain, from whatever tracks them (G3's health and poise).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FighterValues {
    /// Health left, 0 to 1.
    pub hp: f32,
    /// Poise left, 0 to 1.
    pub poise: f32,
    /// Health in points, drawn as digits if given.
    pub hp_points: Option<u32>,
}

/// The skill button's values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkillValues {
    /// The share of its cooldown still to go, 0 (ready) to 1.
    pub cooldown: f32,
    /// Whole seconds still to go, drawn as digits on the button if given.
    pub seconds: Option<u32>,
}

/// What the HUD shows, from the app each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudValues {
    pub player: FighterValues,
    pub enemy: Option<FighterValues>,
    pub skill: SkillValues,
}

/// How heavy a damage number reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageKind {
    Normal,
    /// Bigger and amber.
    Heavy,
}

#[derive(Clone, Copy, Debug)]
struct DamageNumber {
    value: u32,
    kind: DamageKind,
    position: Vec2,
    born: Timestamp,
}

/// A health bar's trailing "damage taken" segment: when the value drops the segment holds at the
/// old value, then catches up. A rising value takes it along at once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Trail {
    value: f32,
    trail: f32,
    hold: f32,
    seen: bool,
}

impl Trail {
    fn update(&mut self, value: f32, dt: f32) {
        if !self.seen {
            *self = Self { value, trail: value, hold: 0.0, seen: true };
            return;
        }
        if value < self.value {
            self.hold = TRAIL_HOLD;
        }
        self.value = value;
        if value >= self.trail {
            self.trail = value;
            self.hold = 0.0;
            return;
        }
        // What's left of the frame after the hold, so it's a matter of time, not of frames.
        let moving = (dt - self.hold).max(0.0);
        self.hold = (self.hold - dt).max(0.0);
        self.trail = (self.trail - TRAIL_RATE * moving).max(value);
    }
}

/// What a frame's [`CombatHud::draw`] drew, for the stats overlay and Tracy (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HudStats {
    pub quads: u32,
    pub damage_numbers: u32,
}

/// The combat HUD's drawing half. Each frame the app calls [`Self::update`] with the clock, the
/// values and the touch controls' state, then [`Self::draw`] into the frame packet's draw list.
#[derive(Debug)]
pub struct CombatHud {
    atlas: Atlas,
    last: Option<Timestamp>,
    now: Timestamp,
    values: HudValues,
    player_trail: Trail,
    enemy_trail: Trail,
    /// How far each button's press has eased in, 0 to 1.
    press: [f32; Button::ALL.len()],
    /// The stick and buttons draw. A finger still works where they're hidden.
    controls_shown: bool,
    pause_shown: bool,
    /// When the skill last came off cooldown.
    ready_at: Option<Timestamp>,
    damage: [Option<DamageNumber>; MAX_DAMAGE_NUMBERS],
    /// Where the next damage number goes if none is free: the oldest.
    next_damage: usize,
}

impl Default for CombatHud {
    fn default() -> Self {
        Self::new()
    }
}

impl CombatHud {
    /// Paints the atlas: a load-time cost (see the crate docs).
    pub fn new() -> Self {
        Self::with_atlas(Atlas::paint())
    }

    /// With an atlas already painted.
    pub fn with_atlas(atlas: Atlas) -> Self {
        Self {
            atlas,
            last: None,
            now: Timestamp::default(),
            values: HudValues::default(),
            player_trail: Trail::default(),
            enemy_trail: Trail::default(),
            press: [0.0; Button::ALL.len()],
            controls_shown: true,
            pause_shown: false,
            ready_at: None,
            damage: [None; MAX_DAMAGE_NUMBERS],
            next_damage: 0,
        }
    }

    /// Whether the stick and buttons draw, where a pad has taken over or there's no touch to speak of
    /// (spec §6.6). They draw until told otherwise. The bars and numbers always do.
    pub fn show_controls(&mut self, shown: bool) {
        self.controls_shown = shown;
    }

    /// Whether the frame says the fight is waiting: a veil over all of it and a pause mark in the safe
    /// area's middle, or a held picture would read as a hung one. Off until told otherwise.
    pub fn show_pause(&mut self, shown: bool) {
        self.pause_shown = shown;
    }

    /// The atlas the HUD draws from.
    pub fn atlas(&self) -> &Arc<UiAtlas> {
        &self.atlas.image
    }

    /// Advances the animation to `now` on the real clock. Values outside their ranges are clamped,
    /// and one that isn't a number reads as 0.
    pub fn update(&mut self, now: Timestamp, values: &HudValues, actions: &TouchActions) {
        profiling::scope!("CombatHud::update");
        let dt = match self.last {
            Some(last) => now.saturating_duration_since(last),
            None => Duration::ZERO,
        }
        .as_secs_f32();
        self.last = Some(now);
        self.now = now;

        let was_cooling = self.values.skill.cooldown > 0.0;
        self.values = sanitized(values);
        if was_cooling && self.values.skill.cooldown <= 0.0 {
            self.ready_at = Some(now);
        }
        self.player_trail.update(self.values.player.hp, dt);
        if let Some(enemy) = self.values.enemy {
            self.enemy_trail.update(enemy.hp, dt);
        } else {
            self.enemy_trail = Trail::default();
        }
        for (press, state) in self.press.iter_mut().zip(&actions.buttons) {
            let aim = if state.down { 1.0 } else { 0.0 };
            let step = dt / PRESS_EASE;
            *press = if *press < aim { (*press + step).min(aim) } else { (*press - step).max(aim) };
        }
        for slot in &mut self.damage {
            if slot.is_some_and(|number| seconds_since(now, number.born) >= DAMAGE_LIFE) {
                *slot = None;
            }
        }
    }

    /// Shows `value` at `position` in UI points (the app projects it from the world). It lives on
    /// the real clock from the last [`Self::update`]'s time, so a number born in hitstop still
    /// floats away. Past [`MAX_DAMAGE_NUMBERS`] it replaces the oldest.
    pub fn spawn_damage(&mut self, value: u32, position: Vec2, kind: DamageKind) {
        let free = self.damage.iter().position(Option::is_none);
        let slot = free.unwrap_or_else(|| {
            let oldest = self.next_damage;
            self.next_damage = (oldest + 1) % MAX_DAMAGE_NUMBERS;
            oldest
        });
        self.damage[slot] = Some(DamageNumber { value, kind, position, born: self.now });
    }

    /// Someone else came in for a fighter, `[player, enemy]`: what they have isn't what the last one
    /// lost, so that fighter's trail starts over from what it next reads, and a swap never reads as a hit.
    pub fn fighters_came_in(&mut self, [player, enemy]: [bool; 2]) {
        if player {
            self.player_trail = Trail::default();
        }
        if enemy {
            self.enemy_trail = Trail::default();
        }
    }

    /// Takes every damage number off at once: the fight they came from is over.
    pub fn clear_damage(&mut self) {
        self.damage = [None; MAX_DAMAGE_NUMBERS];
        self.next_damage = 0;
    }

    /// Appends this frame's HUD to `list`, naming its atlas and viewport. Allocates nothing once
    /// the list's vector has grown to hold a frame.
    pub fn draw(&self, layout: &HudLayout, actions: &TouchActions, list: &mut UiDrawList) -> HudStats {
        profiling::scope!("CombatHud::draw");
        list.viewport = layout.metrics.size.to_array();
        list.atlas = Some(Arc::clone(&self.atlas.image));
        let before = list.quads.len();
        let mut painter = Painter::new(&mut list.quads, &self.atlas.sprites);
        self.draw_fighter(
            &mut painter,
            &layout.player,
            &self.values.player,
            &self.player_trail,
            MINT,
            layout.scale,
        );
        if let Some(enemy) = &self.values.enemy {
            self.draw_fighter(&mut painter, &layout.enemy, enemy, &self.enemy_trail, ROSE, layout.scale);
        }
        let scale = layout.scale;
        if self.controls_shown {
            draw_stick(&mut painter, layout, &actions.stick);
            let sprites = painter.sprites();
            let icons = [sprites.sword, sprites.chevrons, sprites.spark];
            for ((button, icon), &press) in Button::ALL.into_iter().zip(icons).zip(&self.press) {
                // A skill cooling down gives its middle to the seconds.
                let cooling = button == Button::Skill && self.values.skill.cooldown > 0.0;
                let icon_share = if cooling { COOLING_ICON } else { 1.0 };
                draw_button(
                    &mut painter,
                    layout.button(button),
                    actions.button(button),
                    press,
                    icon,
                    icon_share,
                    scale,
                );
            }
            self.draw_skill_state(&mut painter, &layout.skill, scale);
        }
        let mut numbers = 0;
        for number in self.damage.iter().flatten() {
            numbers += u32::from(self.draw_damage(&mut painter, number, scale));
        }
        if self.pause_shown {
            draw_pause(&mut painter, layout);
        }
        HudStats { quads: (list.quads.len() - before) as u32, damage_numbers: numbers }
    }

    fn draw_fighter(
        &self,
        painter: &mut Painter<'_>,
        bars: &BarsLayout,
        values: &FighterValues,
        trail: &Trail,
        color: [u8; 4],
        scale: f32,
    ) {
        let edge = 1.5 * scale;
        painter.bar(bars.hp, SLANT, (0.0, 1.0), Paint::outlined(INK, PALE, edge));
        painter.bar(bars.hp, SLANT, (values.hp, trail.trail), Paint::fill(EMBER));
        painter.bar(bars.hp, SLANT, (0.0, values.hp), Paint::fill(color));
        painter.bar(bars.poise, SLANT, (0.0, 1.0), Paint::outlined(INK, faded(PALE, 0.7), scale));
        painter.bar(bars.poise, SLANT, (0.0, values.poise), Paint::fill(AMBER));
        if let Some(points) = values.hp_points {
            let digits = Paint::outlined(WHITE, OUTLINE, 1.6 * scale);
            painter.digits(points, bars.digits, bars.digit_height, bars.digits_align, SLANT, digits);
        }
    }

    fn draw_skill_state(&self, painter: &mut Painter<'_>, skill: &ButtonLayout, scale: f32) {
        let sprites = painter.sprites();
        let press = 1.0 - 0.08 * self.press[2];
        let size = Vec2::splat(skill.half * 2.0 * press);
        let cooldown = self.values.skill;
        if cooldown.cooldown > 0.0 {
            // What's still to go, darkened: a wedge that closes clockwise-backward toward the top.
            let dark = Paint::fill([4, 6, 12, 210]).swept(cooldown.cooldown);
            painter.sprite(sprites.button, skill.center, size, SLANT, dark);
            if let Some(seconds) = cooldown.seconds {
                let digits = Paint::outlined(WHITE, OUTLINE, 1.6 * scale);
                painter.digits(seconds, skill.center, 26.0 * scale, Align::Center, SLANT, digits);
            }
        }
        if let Some(ready_at) = self.ready_at {
            let age = seconds_since(self.now, ready_at);
            if age < READY_FLASH {
                let share = 1.0 - age / READY_FLASH;
                let ring = Paint::outlined([0; 4], faded(AMBER, share), 3.0 * scale)
                    .soft(1.5 * scale)
                    .grown(age / READY_FLASH * 8.0 * scale);
                painter.sprite(sprites.button, skill.center, size, SLANT, ring);
            }
        }
    }

    /// Draws `number`, returning whether it still shows.
    fn draw_damage(&self, painter: &mut Painter<'_>, number: &DamageNumber, scale: f32) -> bool {
        let age = seconds_since(self.now, number.born);
        if age >= DAMAGE_LIFE {
            return false;
        }
        // It punches in big and settles, then drifts up, easing out, and fades at the end.
        let pop = (age / DAMAGE_POP).min(1.0);
        let punch = 1.0 + 0.7 * (1.0 - pop) * (1.0 - pop);
        let life = age / DAMAGE_LIFE;
        let rise = DAMAGE_RISE * scale * (1.0 - (1.0 - life) * (1.0 - life));
        let fade =
            if life <= DAMAGE_WHOLE { 1.0 } else { 1.0 - (life - DAMAGE_WHOLE) / (1.0 - DAMAGE_WHOLE) };
        let (height, color) = match number.kind {
            DamageKind::Normal => (26.0, WHITE),
            DamageKind::Heavy => (36.0, AMBER),
        };
        let height = height * scale * punch;
        let paint = Paint::outlined(faded(color, fade), faded(OUTLINE, fade), 2.2 * scale * punch);
        let at = number.position - Vec2::new(0.0, rise);
        painter.digits(number.value, at, height, Align::Center, SLANT, paint);
        true
    }
}

fn sanitized(values: &HudValues) -> HudValues {
    let fighter = |fighter: &FighterValues| FighterValues {
        hp: share(fighter.hp),
        poise: share(fighter.poise),
        ..*fighter
    };
    HudValues {
        player: fighter(&values.player),
        enemy: values.enemy.as_ref().map(fighter),
        skill: SkillValues { cooldown: share(values.skill.cooldown), ..values.skill },
    }
}

/// `value` clamped to 0..1; not a number reads as 0.
fn share(value: f32) -> f32 {
    if value.is_nan() { 0.0 } else { value.clamp(0.0, 1.0) }
}

fn seconds_since(now: Timestamp, then: Timestamp) -> f32 {
    now.saturating_duration_since(then).as_secs_f32()
}

/// The veil and the pause mark. The veil is the bar shape over more than the frame, its rounded corners
/// off it: half the frame's height all round is more than their radius, 3/16 of the shape's height.
fn draw_pause(painter: &mut Painter<'_>, layout: &HudLayout) {
    let size = layout.metrics.size;
    let margin = Vec2::splat(size.y * 0.5);
    painter.bar(Rect::new(-margin, size + margin), 0.0, (0.0, 1.0), Paint::fill(VEIL));
    let scale = layout.scale;
    let middle = layout.metrics.safe().center();
    // Leaning as everything else does: its top to the right.
    let along = Vec2::new(SLANT, -1.0).normalize();
    let paint = Paint::outlined(PALE, OUTLINE, 2.0 * scale);
    for side in [-1.0, 1.0] {
        let center = middle + Vec2::new(side * PAUSE_BAR_APART * scale, 0.0);
        painter.bar_along(center, along, PAUSE_BAR_LENGTH * scale, PAUSE_BAR_WIDTH * scale, paint);
    }
}

fn draw_button(
    painter: &mut Painter<'_>,
    layout: &ButtonLayout,
    state: &ButtonState,
    press: f32,
    icon: crate::atlas::Sprite,
    icon_share: f32,
    scale: f32,
) {
    let sprites = painter.sprites();
    let shrink = 1.0 - 0.08 * press;
    let size = Vec2::splat(layout.half * 2.0 * shrink);
    let shadow = Paint::fill(SHADOW).grown(2.0 * scale).soft(4.0 * scale);
    painter.sprite(sprites.button, layout.center, size, SLANT, shadow);
    // Pressed, the ink lifts and the edge goes white; held past its hold, the whole shape turns amber
    // and its icon goes dark, which still reads with a thumb over most of it.
    let (ink, edge, glyph) = if state.held {
        (AMBER, WHITE, OUTLINE)
    } else if state.down {
        ([38, 42, 56, 245], WHITE, WHITE)
    } else {
        (INK, PALE, WHITE)
    };
    painter.sprite(sprites.button, layout.center, size, SLANT, Paint::outlined(ink, edge, 1.8 * scale));
    painter.sprite(icon, layout.center, size * 0.62, SLANT * 0.5, Paint::fill(faded(glyph, icon_share)));
    if state.dragging && state.drag != Vec2::ZERO {
        // Where the drag points, just outside the button. `drag` is +y up, the screen's +y down.
        let up = Vec2::new(state.drag.x, -state.drag.y).normalize();
        let center = layout.center + up * (layout.half * 1.32);
        let wedge = Paint::outlined(AMBER, OUTLINE, 1.2 * scale);
        painter.turned(sprites.wedge, center, Vec2::splat(20.0 * scale), up, wedge);
    }
}

fn draw_stick(painter: &mut Painter<'_>, layout: &HudLayout, stick: &StickState) {
    let sprites = painter.sprites();
    let radius = layout.stick.radius;
    let scale = layout.scale;
    let ring_size = Vec2::splat(radius * 2.0);
    if !stick.active {
        // At rest, a faint ring where the thumb usually lands.
        let at = layout.stick.rest;
        painter.sprite(sprites.disc, at, ring_size, 0.0, Paint::fill(faded(INK, 0.3)));
        painter.sprite(sprites.disc, at, ring_size, 0.0, Paint::ring(faded(PALE, 0.55), 1.5 * scale));
        painter.sprite(
            sprites.button,
            at,
            ring_size * 0.4,
            SLANT,
            Paint::outlined(faded(PALE, 0.5), faded(OUTLINE, 0.5), scale),
        );
        return;
    }
    // Under the thumb, kept whole on screen; the value doesn't move with it.
    let inset = Vec2::splat(radius);
    let safe = layout.safe;
    let center = if safe.width() > radius * 2.0 && safe.height() > radius * 2.0 {
        stick.center.clamp(safe.min + inset, safe.max - inset)
    } else {
        stick.center
    };
    painter.sprite(sprites.disc, center, ring_size, 0.0, Paint::fill(faded(INK, 0.55)));
    painter.sprite(sprites.disc, center, ring_size, 0.0, Paint::ring(PALE, 2.0 * scale));
    let screen = Vec2::new(stick.value.x, -stick.value.y);
    let knob = center + screen * radius;
    // The knob is the buttons' cut shape, leaning with them, so the stick reads as one family.
    painter.sprite(sprites.button, knob, ring_size * 0.4, SLANT, Paint::outlined(MINT, OUTLINE, 1.5 * scale));
    if screen != Vec2::ZERO {
        let up = screen.normalize();
        let wedge = Paint::outlined(MINT, OUTLINE, 1.2 * scale);
        painter.turned(
            sprites.wedge,
            center + up * (radius + 13.0 * scale),
            Vec2::splat(18.0 * scale),
            up,
            wedge,
        );
    }
}

#[cfg(test)]
mod tests;
