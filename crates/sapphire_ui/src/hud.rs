//! The combat HUD: its layout, its values and how it draws.
//!
//! # The look
//!
//! Nothing here is a generic mobile circle. Shapes lean: bars, digits and buttons share one slant,
//! so the whole frame reads as moving forward. Buttons are near-square with two corners cut hard,
//! inked nearly black with a crisp pale edge and a white glyph, so they read over a bright sky or a
//! dark street alike. Color is rationed and always means something: mint for the player's health,
//! rose for the enemy's, amber for poise and anything charging, ember for health just lost, violet for
//! the ultimate's energy, which goes nearly white once it's full. Damage numbers are large, heavy and
//! slanted with a dark outline, and they punch in and float off.
//!
//! # The ultimate (M3 Q3c)
//!
//! A fighter with an energy meter has [`HudValues::ultimate`]. Its meter is a violet bar under the
//! player's health digits, drawn whenever the bars are, and its button stands at the top of the right
//! thumb's cluster: the buttons' cut shape with an eight-point burst, ringed by its energy filling
//! clockwise from the top. Full, the ring closes and goes nearly white, the button's ink turns violet
//! and a violet halo breathes behind it; it flashes once as it gets there. With no ultimate nothing of
//! it draws, and `game_ui`'s zones don't take a finger there.
//!
//! # A boss
//!
//! A foe that's a boss ([`HudValues::boss`]) draws in the boss's place, wider and taller than any other
//! foe's, with an inked mark across its health where each of its phases falls (M3 B3). Its name, a key in
//! the game's words that its kind of enemy holds, isn't drawn here yet: the HUD's words come with M3 U7.
//!
//! # The party (M5 C9)
//!
//! A fight with a party ([`HudValues::party`]) has a slot for each member down the left, under the player's
//! name: a chip with the member's place in the party (1 to 3), the member's health beside it, and, off the
//! field, its rest. Whoever's on the field has a lit edge and a mint pointer at its chip, and its health is
//! the player's bars'. A member still resting has its chip darkened by a wedge closing clockwise, as the
//! skill's is, its number dimmed, and the whole seconds left beside its health. A fallen one is struck
//! through, its number dimmed. Who a press of the swap would bring in, and who has fallen, is said in words,
//! by the HUD's document (`game_ui`'s `HudModel::follow_fight`), on plates beside the slots.
//!
//! Its swap button stands at the top of the right thumb's cluster, two arrows passing: the cooldown of
//! whoever's on the field closes over it, with its seconds, as the skill's does, and with nobody to bring in
//! its arrows dim. With no party nothing of either draws, and `game_ui`'s zones don't take a finger there.
//!
//! # Time
//!
//! Everything animates on the real clock: hitstop and slow motion freeze the world, never the
//! interface. The crate reads no clock (rule R2): the app passes
//! [`Platform::now`](sapphire_platform::Platform::now) to [`CombatHud::update`], and a test or a
//! capture passes any time it likes, which makes a frame's animation exact and repeatable.

use std::sync::OnceLock;

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

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
/// The ultimate's energy, charging.
const VIOLET: [u8; 4] = [152, 120, 255, 255];
/// The ultimate's energy, full: violet gone nearly white.
const LIT: [u8; 4] = [236, 228, 255, 255];
/// The ultimate's button's ink once it's ready.
const VIOLET_INK: [u8; 4] = [44, 26, 92, 240];
/// The shipped layout (see [`HudDesign::shipped`]).
const SHIPPED_DESIGN: &str = include_str!("../default_hud.ron");

/// A health bar's trailing segment: what a hit just took.
const EMBER: [u8; 4] = [255, 112, 58, 255];
/// How wide the inked edge between a health bar's fill and its trailing segment is, in points.
const TRAIL_EDGE: f32 = 2.5;
const SHADOW: [u8; 4] = [0, 0, 0, 170];
const OUTLINE: [u8; 4] = [8, 10, 18, 255];

/// What a pause lays over the frame. Blending is in linear light, so it takes this much to read as
/// waiting at a glance, with what's held still there underneath.
const VEIL: [u8; 4] = [6, 8, 14, 190];
/// The pause mark's bars, in points: how long, how wide, and how far each stands from the middle.
const PAUSE_BAR_LENGTH: f32 = 76.0;
const PAUSE_BAR_WIDTH: f32 = 24.0;
const PAUSE_BAR_APART: f32 = 24.0;

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

/// How wide each of the other foes' small bars stands in its row, gap and all, and the gap, in points.
const OTHER_BAR_WIDTH: f32 = 44.0;
const OTHER_BAR_GAP: f32 = 8.0;

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
/// How much of the ultimate's icon shows while it charges.
const CHARGING_ICON: f32 = 0.45;
/// One breath of a ready ultimate's halo, on the real clock, in nanoseconds.
const BREATH_NANOS: u64 = 1_400_000_000;
/// How far outside the ultimate's button its energy ring stands, as a share of the button's size.
const ULTIMATE_RING: f32 = 1.18;
/// The ready halo's size, as a share of the button's.
const ULTIMATE_HALO: f32 = 1.36;
/// What a wait still to go lays over a button or a chip: a wedge that closes clockwise-backward toward the
/// top.
const WAITING: [u8; 4] = [4, 6, 12, 210];
/// How much of a party chip's number shows while its member rests or has fallen.
const RESTING_NUMBER: f32 = 0.35;

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

/// One party slot's row (M5 C9): the member's chip, its health beside it, and where the seconds of its rest
/// start, on the row's vertical middle. The HUD's document puts its words just after the health too.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartyRowLayout {
    pub chip: Vec2,
    pub hp: Rect,
    pub digits: Vec2,
}

/// The party's slots, a row each from the top.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartyLayout {
    pub rows: [PartyRowLayout; MAX_PARTY],
    /// Half the chip's side.
    pub chip_half: f32,
    /// How tall the chip's number is, and the rest's seconds, before the player's text size.
    pub digit_height: f32,
}

/// Where everything goes, from the frame's metrics. All in points, all inside the safe area,
/// pinned to its corners and edges so each keeps its distance from them at any aspect ratio. It is
/// plain data, and `game_ui`'s touch zones hit-test against the same numbers it
/// draws from.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudLayout {
    pub metrics: UiMetrics,
    pub safe: Rect,
    pub stick: StickLayout,
    pub attack: ButtonLayout,
    pub dodge: ButtonLayout,
    pub skill: ButtonLayout,
    /// Only drawn and hit-tested while the HUD has an ultimate (M3 Q3c).
    pub ultimate: ButtonLayout,
    /// Only drawn and hit-tested while the HUD has a party (M5 C9).
    pub swap: ButtonLayout,
    /// The party's slots: drawn while the HUD has a party.
    pub party: PartyLayout,
    pub player: BarsLayout,
    pub enemy: BarsLayout,
    /// Where the enemy's bars go instead, when it's a boss.
    pub boss: BarsLayout,
    /// How wide a boss's phase marks are.
    pub mark_width: f32,
    /// The row under the enemy's bars that the other foes' small ones share, from its middle out.
    pub others: Rect,
    /// The player's energy meter, under its bars: drawn while the HUD has an ultimate.
    pub energy: Rect,
    /// Everything scales by this: 1 unless the frame is smaller than a phone.
    pub scale: f32,
}

impl HudLayout {
    /// Places the HUD by [`HudDesign::shipped`].
    pub fn new(metrics: UiMetrics) -> Self {
        Self::designed(metrics, HudDesign::shipped())
    }

    /// Places the HUD by `design`.
    pub fn designed(metrics: UiMetrics, design: &HudDesign) -> Self {
        let safe = metrics.safe();
        let [full_width, full_height] = design.full_size;
        let scale = (safe.height() / full_height).min(safe.width() / full_width).clamp(0.01, 1.0);
        let points = |value: f32| value * scale;
        let margin = points(design.margin);

        // The right thumb's cluster: the attack in the corner, the others on an arc around it.
        let attack_half = points(design.attack.half);
        let attack_center = Anchor::BOTTOM_RIGHT.place(safe, Vec2::splat(-(margin + attack_half)));
        let around = |button: &ButtonDesign| ButtonLayout {
            center: attack_center + Vec2::new(points(button.offset[0]), points(button.offset[1])),
            half: points(button.half),
            hit: points(button.hit),
        };
        let attack = around(&design.attack);
        let dodge = around(&design.dodge);
        let skill = around(&design.skill);
        let ultimate = around(&design.ultimate);
        let swap = around(&design.swap);

        // The left thumb's zone: the frame's left side, under the bars.
        let radius = points(design.stick.radius);
        let [rest_x, rest_y] = design.stick.rest;
        let rest = Anchor::BOTTOM_LEFT.place(safe, Vec2::new(points(rest_x), points(rest_y)));
        let zone = Rect::new(
            Vec2::new(safe.min.x, safe.min.y + safe.height() * design.stick.zone_top),
            Vec2::new(safe.min.x + safe.width() * design.stick.zone_width, safe.max.y),
        );

        // The bars, on the safe area's top: the player's in the corner, the enemy's in what's left
        // of the middle, so they can't meet at 4:3.
        let top_margin = points(design.bars_top);
        let player_width = (safe.width() * design.player.width).min(points(design.player.max_width));
        let left = Anchor::TOP_LEFT.place(safe, Vec2::new(margin, top_margin));
        let player = design.player.bars.place(left, player_width, Align::Left, &points);
        let meter = &design.energy;
        let energy = Rect::new(
            left + Vec2::new(player_width * meter.from, points(meter.top)),
            left + Vec2::new(player_width * meter.to, points(meter.bottom)),
        );
        let free = safe.width() - 2.0 * (margin + player_width + points(design.bars_gap));
        let enemy_width = free.min(points(design.enemy.max_width)).max(0.0);
        let top = Anchor::TOP_CENTER.place(safe, Vec2::new(-enemy_width * 0.5, top_margin));
        let enemy = design.enemy.bars.place(top, enemy_width, Align::Center, &points);
        let boss_width = free.min(points(design.boss.max_width)).max(0.0);
        let boss_top = Anchor::TOP_CENTER.place(safe, Vec2::new(-boss_width * 0.5, top_margin));
        let boss = design.boss.bars.place(boss_top, boss_width, Align::Center, &points);
        let others = Rect::new(
            top + Vec2::new(0.0, points(design.enemy.others_top)),
            top + Vec2::new(enemy_width, points(design.enemy.others_bottom)),
        );
        // The party's slots, down the left under the player's bars and name.
        let party = design.party.place(Anchor::TOP_LEFT.place(safe, Vec2::new(margin, 0.0)), &points);

        Self {
            metrics,
            safe,
            stick: StickLayout { rest, radius, zone },
            attack,
            dodge,
            skill,
            ultimate,
            swap,
            party,
            player,
            enemy,
            boss,
            mark_width: points(design.boss.mark_width),
            others,
            energy,
            scale,
        }
    }

    /// `button`'s place.
    pub fn button(&self, button: Button) -> &ButtonLayout {
        match button {
            Button::Attack => &self.attack,
            Button::Dodge => &self.dodge,
            Button::Skill => &self.skill,
            Button::Ultimate => &self.ultimate,
            Button::Swap => &self.swap,
        }
    }
}

/// Where the combat HUD's pieces go, in points on a phone's frame (a `HudDesign` document, schema 1). It's
/// [`HudLayout`]'s numbers and no more: what a piece is and how it's drawn stay code until the retained tree
/// takes the HUD over.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::ui"]
#[serde(deny_unknown_fields)]
pub struct HudDesign {
    /// A safe area smaller than this, either way, shrinks the whole HUD to fit (a small window).
    pub full_size: [f32; 2],
    /// Between the safe area's edge and what sits on it.
    pub margin: f32,
    /// Stands in the bottom right corner, moved by its offset.
    pub attack: ButtonDesign,
    /// Around the attack.
    pub dodge: ButtonDesign,
    pub skill: ButtonDesign,
    /// The ultimate's button (M3 Q3c). A design written before it had one gets the shipped one.
    #[serde(default = "ButtonDesign::shipped_ultimate")]
    pub ultimate: ButtonDesign,
    /// The party's swap button (M5 C9). A design written before it had one gets the shipped one.
    #[serde(default = "ButtonDesign::shipped_swap")]
    pub swap: ButtonDesign,
    pub stick: StickDesign,
    /// How far under the safe area's top the bars hang.
    pub bars_top: f32,
    /// What stays between the player's bars and the enemy's.
    pub bars_gap: f32,
    pub player: PlayerBarsDesign,
    pub enemy: EnemyBarsDesign,
    /// The player's energy meter, under where the player's bars hang (M3 Q3c). A design written before it
    /// had one gets the shipped one.
    #[serde(default = "MeterDesign::shipped_energy")]
    pub energy: MeterDesign,
    pub boss: BossBarsDesign,
    /// The party's slots (M5 C9). A design written before it had them gets the shipped ones.
    #[serde(default = "PartyDesign::shipped")]
    pub party: PartyDesign,
}

/// A button: where it stands from the attack's corner place, how far it's drawn and how far it takes a touch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonDesign {
    pub offset: [f32; 2],
    pub half: f32,
    pub hit: f32,
}

impl ButtonDesign {
    /// The ultimate's place in `default_hud.ron`: what a design with none gets.
    pub const fn shipped_ultimate() -> Self {
        Self { offset: [-112.0, -112.0], half: 32.0, hit: 40.0 }
    }

    /// The swap's place in `default_hud.ron`: what a design with none gets.
    pub const fn shipped_swap() -> Self {
        Self { offset: [-6.0, -176.0], half: 26.0, hit: 34.0 }
    }
}

/// The party's slots, a row each down from `top` under the safe area's top, at the bars' margin from its left:
/// each row `row_height` tall with `row_gap` between, its chip `chip_half` each way from its middle, then
/// `bar_gap` to its health, `bar_width` by `bar_height`, then `bar_gap` again to the rest's seconds,
/// `digit_height` tall, as is the chip's number.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartyDesign {
    pub top: f32,
    pub row_height: f32,
    pub row_gap: f32,
    pub chip_half: f32,
    pub bar_gap: f32,
    pub bar_width: f32,
    pub bar_height: f32,
    pub digit_height: f32,
}

impl PartyDesign {
    /// The slots' place in `default_hud.ron`: what a design with none gets.
    pub const fn shipped() -> Self {
        Self {
            top: 108.0,
            row_height: 24.0,
            row_gap: 4.0,
            chip_half: 12.0,
            bar_gap: 8.0,
            bar_width: 64.0,
            bar_height: 6.0,
            digit_height: 14.0,
        }
    }

    fn place(&self, corner: Vec2, points: &impl Fn(f32) -> f32) -> PartyLayout {
        let (chip_half, bar_gap) = (points(self.chip_half), points(self.bar_gap));
        let rows = std::array::from_fn(|index| {
            let middle = corner.y
                + points(self.top + (self.row_height + self.row_gap) * index as f32)
                + points(self.row_height) * 0.5;
            let chip = Vec2::new(corner.x + chip_half, middle);
            let from = corner.x + 2.0 * chip_half + bar_gap;
            let half_bar = points(self.bar_height) * 0.5;
            let hp = Rect::new(
                Vec2::new(from, middle - half_bar),
                Vec2::new(from + points(self.bar_width), middle + half_bar),
            );
            PartyRowLayout { chip, hp, digits: Vec2::new(hp.max.x + bar_gap, middle) }
        });
        PartyLayout { rows, chip_half, digit_height: points(self.digit_height) }
    }
}

/// A meter under the player's bars: its top and bottom in points under where they hang, and where it runs
/// as shares of their width.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterDesign {
    pub top: f32,
    pub bottom: f32,
    pub from: f32,
    pub to: f32,
}

impl MeterDesign {
    /// The energy meter's place in `default_hud.ron`: what a design with none gets.
    pub const fn shipped_energy() -> Self {
        Self { top: 52.0, bottom: 58.0, from: 0.0, to: 0.62 }
    }
}

/// The left thumb's stick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StickDesign {
    pub radius: f32,
    /// Where it rests, from the safe area's bottom left.
    pub rest: [f32; 2],
    /// The zone it lands in starts this share of the safe area's height down, and takes this share of its
    /// width.
    pub zone_top: f32,
    pub zone_width: f32,
}

/// A fighter's bars, under where they hang: heights in points, and what runs along them as shares of their
/// width.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarsDesign {
    pub hp_height: f32,
    pub poise_top: f32,
    pub poise_bottom: f32,
    pub poise_from: f32,
    pub poise_to: f32,
    pub digits_top: f32,
    pub digits_along: f32,
    pub digit_height: f32,
}

impl BarsDesign {
    fn place(&self, at: Vec2, width: f32, align: Align, points: &impl Fn(f32) -> f32) -> BarsLayout {
        BarsLayout {
            hp: Rect::new(at, at + Vec2::new(width, points(self.hp_height))),
            poise: Rect::new(
                at + Vec2::new(width * self.poise_from, points(self.poise_top)),
                at + Vec2::new(width * self.poise_to, points(self.poise_bottom)),
            ),
            digits: at + Vec2::new(width * self.digits_along, points(self.digits_top)),
            digits_align: align,
            digit_height: points(self.digit_height),
        }
    }
}

/// The player's bars, in the corner: `width` is a share of the safe area's, up to `max_width`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerBarsDesign {
    pub width: f32,
    pub max_width: f32,
    pub bars: BarsDesign,
}

/// The enemy's bars, in what's left of the middle, up to `max_width`, with the row the other foes' small ones
/// share between `others_top` and `others_bottom` under where they hang.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnemyBarsDesign {
    pub max_width: f32,
    pub bars: BarsDesign,
    pub others_top: f32,
    pub others_bottom: f32,
}

/// A boss's bars, in place of the enemy's: in what's left of the middle, up to `max_width`, with a mark
/// `mark_width` wide across its health where each of its phases falls.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossBarsDesign {
    pub max_width: f32,
    pub bars: BarsDesign,
    pub mark_width: f32,
}

impl Versioned for HudDesign {
    /// 1: M3 S3. 2: M3 B3 added the boss's bars.
    const VERSION: u32 = 2;
}

/// A HUD design document that can't be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HudDesignError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("{field}: {value} {wanted}")]
    Field { field: &'static str, value: f32, wanted: &'static str },
}

impl HudDesign {
    /// Sapphire's layout (`default_hud.ron` beside this crate's manifest), compiled in.
    #[must_use]
    pub fn shipped() -> &'static Self {
        static PARSED: OnceLock<HudDesign> = OnceLock::new();
        PARSED.get_or_init(|| {
            Self::from_ron(SHIPPED_DESIGN).expect("this module's tests read the shipped layout")
        })
    }

    /// Reads a HUD design document and checks it.
    pub fn from_ron(text: &str) -> Result<Self, HudDesignError> {
        let design: Self = sapphire_serialize::from_ron(text)?;
        design.check()?;
        Ok(design)
    }

    /// Says which number can't be laid out: a size is more than 0, a place is any finite number, and a share
    /// of a width stays inside it.
    fn check(&self) -> Result<(), HudDesignError> {
        const SIZE: &str = "isn't a size above 0";
        const PLACE: &str = "isn't a finite number";
        const SHARE: &str = "isn't a share, 0 to 1";
        let fail = |field, value, wanted| Err(HudDesignError::Field { field, value, wanted });
        let Self {
            full_size,
            margin,
            attack,
            dodge,
            skill,
            ultimate,
            swap,
            stick,
            bars_top,
            bars_gap,
            player,
            enemy,
            energy,
            boss,
            party,
        } = self;
        let sizes = [
            ("full_size.0", full_size[0]),
            ("full_size.1", full_size[1]),
            ("attack.half", attack.half),
            ("attack.hit", attack.hit),
            ("dodge.half", dodge.half),
            ("dodge.hit", dodge.hit),
            ("skill.half", skill.half),
            ("skill.hit", skill.hit),
            ("ultimate.half", ultimate.half),
            ("ultimate.hit", ultimate.hit),
            ("swap.half", swap.half),
            ("swap.hit", swap.hit),
            ("stick.radius", stick.radius),
            ("player.max_width", player.max_width),
            ("player.hp_height", player.bars.hp_height),
            ("player.digit_height", player.bars.digit_height),
            ("enemy.max_width", enemy.max_width),
            ("enemy.hp_height", enemy.bars.hp_height),
            ("enemy.digit_height", enemy.bars.digit_height),
            ("boss.max_width", boss.max_width),
            ("boss.hp_height", boss.bars.hp_height),
            ("boss.digit_height", boss.bars.digit_height),
            ("boss.mark_width", boss.mark_width),
            ("party.row_height", party.row_height),
            ("party.chip_half", party.chip_half),
            ("party.bar_width", party.bar_width),
            ("party.bar_height", party.bar_height),
            ("party.digit_height", party.digit_height),
        ];
        for (field, value) in sizes {
            if !value.is_finite() || value <= 0.0 {
                return fail(field, value, SIZE);
            }
        }
        let places = [
            ("margin", *margin),
            ("attack.offset.0", attack.offset[0]),
            ("attack.offset.1", attack.offset[1]),
            ("dodge.offset.0", dodge.offset[0]),
            ("dodge.offset.1", dodge.offset[1]),
            ("skill.offset.0", skill.offset[0]),
            ("skill.offset.1", skill.offset[1]),
            ("ultimate.offset.0", ultimate.offset[0]),
            ("ultimate.offset.1", ultimate.offset[1]),
            ("swap.offset.0", swap.offset[0]),
            ("swap.offset.1", swap.offset[1]),
            ("stick.rest.0", stick.rest[0]),
            ("stick.rest.1", stick.rest[1]),
            ("bars_top", *bars_top),
            ("bars_gap", *bars_gap),
            ("player.poise_top", player.bars.poise_top),
            ("player.poise_bottom", player.bars.poise_bottom),
            ("player.digits_top", player.bars.digits_top),
            ("enemy.poise_top", enemy.bars.poise_top),
            ("enemy.poise_bottom", enemy.bars.poise_bottom),
            ("enemy.digits_top", enemy.bars.digits_top),
            ("enemy.others_top", enemy.others_top),
            ("enemy.others_bottom", enemy.others_bottom),
            ("energy.top", energy.top),
            ("energy.bottom", energy.bottom),
            ("boss.poise_top", boss.bars.poise_top),
            ("boss.poise_bottom", boss.bars.poise_bottom),
            ("boss.digits_top", boss.bars.digits_top),
            ("party.top", party.top),
            ("party.row_gap", party.row_gap),
            ("party.bar_gap", party.bar_gap),
        ];
        for (field, value) in places {
            if !value.is_finite() {
                return fail(field, value, PLACE);
            }
        }
        let shares = [
            ("stick.zone_top", stick.zone_top),
            ("stick.zone_width", stick.zone_width),
            ("player.width", player.width),
            ("player.poise_from", player.bars.poise_from),
            ("player.poise_to", player.bars.poise_to),
            ("player.digits_along", player.bars.digits_along),
            ("enemy.poise_from", enemy.bars.poise_from),
            ("enemy.poise_to", enemy.bars.poise_to),
            ("enemy.digits_along", enemy.bars.digits_along),
            ("energy.from", energy.from),
            ("energy.to", energy.to),
            ("boss.poise_from", boss.bars.poise_from),
            ("boss.poise_to", boss.bars.poise_to),
            ("boss.digits_along", boss.bars.digits_along),
        ];
        for (field, value) in shares {
            if !(0.0..=1.0).contains(&value) {
                return fail(field, value, SHARE);
            }
        }
        Ok(())
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

/// How many foes besides the one it shows whole the HUD has bars for.
pub const MAX_OTHER_FOES: usize = 7;

/// The foes a fight has besides [`HudValues::enemy`], each a small bar of its health. No list: the values
/// are copied each frame, and no fight here has more than the bars have room for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OtherFoes {
    hp: [f32; MAX_OTHER_FOES],
    count: u8,
}

impl OtherFoes {
    /// One more, its health left from 0 to 1. Past [`MAX_OTHER_FOES`], it isn't shown.
    pub fn push(&mut self, hp: f32) {
        if let Some(bar) = self.hp.get_mut(usize::from(self.count)) {
            *bar = hp;
            self.count += 1;
        }
    }

    /// Each one's health left.
    pub fn bars(&self) -> &[f32] {
        &self.hp[..usize::from(self.count)]
    }
}

/// The player's ultimate (M3 Q3c): its energy meter and whether it can start.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UltimateValues {
    /// The meter's share, 0 (empty) to 1 (full).
    pub energy: f32,
    /// It has the energy to start: its button lights.
    pub ready: bool,
}

/// How many phase marks a boss's bar has room for.
pub const MAX_PHASE_MARKS: usize = 8;

/// What makes the foe shown a boss's bar: where its phases fall. No list: the values are copied each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BossBar {
    marks: [f32; MAX_PHASE_MARKS],
    count: u8,
}

impl BossBar {
    /// A mark where a phase falls, a share of its health from 0 to 1. One past [`MAX_PHASE_MARKS`], or at
    /// an end of the bar, where it would say nothing, isn't drawn.
    pub fn mark(&mut self, share: f32) {
        if share > 0.0
            && share < 1.0
            && let Some(mark) = self.marks.get_mut(usize::from(self.count))
        {
            *mark = share;
            self.count += 1;
        }
    }

    /// Where each mark falls.
    #[must_use]
    pub fn marks(&self) -> &[f32] {
        &self.marks[..usize::from(self.count)]
    }
}

/// How many members the party's slots have room for: the simulation's `PARTY_SLOTS`.
pub const MAX_PARTY: usize = 3;

/// One member of the player's party (M5 C9), as the simulation's party has it each step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartyMember {
    /// Health left, 0 to 1.
    pub hp: f32,
    /// On the field: the player's bars are this member's.
    pub on_field: bool,
    /// Fallen.
    pub down: bool,
    /// Off the field, the share of its rest still to go before it can come in: 0 (rested) to 1.
    pub rest: f32,
    /// Whole seconds of that rest still to go, drawn as digits beside its health if given.
    pub seconds: Option<u32>,
    /// A press of the swap brings it in, once whoever's on the field has stayed long enough.
    pub next: bool,
}

/// The player's party (M5 C9): a slot for each member in the party's order, and the swap's button. No list:
/// the values are copied each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartyValues {
    members: [PartyMember; MAX_PARTY],
    count: u8,
    /// What's left of the cooldown of whoever's on the field, which has to stay that long before a swap.
    pub swap: SkillValues,
}

impl PartyValues {
    /// One more member, in the party's order. Past [`MAX_PARTY`], it isn't shown.
    pub fn push(&mut self, member: PartyMember) {
        if let Some(slot) = self.members.get_mut(usize::from(self.count)) {
            *slot = member;
            self.count += 1;
        }
    }

    /// Each member, in the party's order.
    #[must_use]
    pub fn members(&self) -> &[PartyMember] {
        &self.members[..usize::from(self.count)]
    }

    /// Whether a press of the swap has anyone to bring in.
    #[must_use]
    pub fn has_next(&self) -> bool {
        self.members().iter().any(|member| member.next)
    }
}

/// What the HUD shows, from the app each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudValues {
    pub player: FighterValues,
    /// The foe the player has now.
    pub enemy: Option<FighterValues>,
    /// That foe is a boss: its bars draw in the boss's place.
    pub boss: Option<BossBar>,
    /// Who else stands against the player.
    pub others: OtherFoes,
    pub skill: SkillValues,
    /// The player's ultimate, for a player with an energy meter: its meter and its button. With none
    /// (the default), neither draws, and the button takes no finger.
    pub ultimate: Option<UltimateValues>,
    /// The player's party, for a fight with one (M5 C9): its slots and its swap's button. With none (the
    /// default), neither draws, and the button takes no finger.
    pub party: Option<PartyValues>,
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
    /// Its place among the numbers spawned: a frame's all have one `born`.
    order: u64,
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
    /// When the ultimate last became ready.
    ultimate_ready_at: Option<Timestamp>,
    damage: [Option<DamageNumber>; MAX_DAMAGE_NUMBERS],
    /// The next damage number's `order`.
    next_damage: u64,
    /// The player's text size (M4 A2), on the bars' numbers and the damage numbers.
    text_scale: f32,
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
            ultimate_ready_at: None,
            damage: [None; MAX_DAMAGE_NUMBERS],
            next_damage: 0,
            text_scale: 1.0,
        }
    }

    /// The player's text size (M4 A2), 1 as designed: the bars' numbers and the damage numbers grow by it.
    /// The skill's countdown, inside its button and twice the bars' numbers already, stays as designed.
    /// Anything that can't be a size reads as 1.
    pub fn set_text_scale(&mut self, scale: f32) {
        self.text_scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    }

    /// The player's text size, as [`Self::set_text_scale`] last said.
    #[must_use]
    pub const fn text_scale(&self) -> f32 {
        self.text_scale
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
        let was_charging = self.values.ultimate.is_some_and(|ultimate| !ultimate.ready);
        self.values = sanitized(values);
        if was_cooling && self.values.skill.cooldown <= 0.0 {
            self.ready_at = Some(now);
        }
        // It flashes as it gets there, not when it first shows up ready.
        match self.values.ultimate {
            Some(ultimate) if ultimate.ready => {
                if was_charging {
                    self.ultimate_ready_at = Some(now);
                }
            }
            _ => self.ultimate_ready_at = None,
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
        self.spawn_damage_at(self.now, value, position, kind);
    }

    /// Shows damage born at `now` on the same clock as [`Self::update`], even if no frame has
    /// updated the HUD yet. Past [`MAX_DAMAGE_NUMBERS`] it replaces the oldest.
    pub fn spawn_damage_at(&mut self, now: Timestamp, value: u32, position: Vec2, kind: DamageKind) {
        // An empty slot sorts before any number, and then the lowest `order` is the oldest.
        let by_age = |&slot: &usize| self.damage[slot].map(|number| number.order);
        let slot = (0..MAX_DAMAGE_NUMBERS).min_by_key(by_age).unwrap_or(0);
        let order = self.next_damage;
        self.next_damage += 1;
        self.damage[slot] = Some(DamageNumber { value, kind, position, born: now, order });
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
        if let Some(ultimate) = &self.values.ultimate {
            self.draw_energy(&mut painter, layout.energy, ultimate, layout.scale);
        }
        if let Some(enemy) = &self.values.enemy {
            let bars = if self.values.boss.is_some() { &layout.boss } else { &layout.enemy };
            self.draw_fighter(&mut painter, bars, enemy, &self.enemy_trail, ROSE, layout.scale);
        }
        if let (Some(_), Some(boss)) = (self.values.enemy, &self.values.boss) {
            draw_marks(&mut painter, layout, boss);
        }
        self.draw_others(&mut painter, layout);
        if let Some(party) = &self.values.party {
            draw_party(&mut painter, &layout.party, party, self.text_scale, layout.scale);
        }
        let scale = layout.scale;
        if self.controls_shown {
            draw_stick(&mut painter, layout, &actions.stick);
            let sprites = painter.sprites();
            let icons = [
                (Button::Attack, sprites.sword),
                (Button::Dodge, sprites.chevrons),
                (Button::Skill, sprites.spark),
            ];
            for (button, icon) in icons {
                let press = self.press[button.index()];
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
            if let Some(ultimate) = &self.values.ultimate {
                let press = self.press[Button::Ultimate.index()];
                let state = actions.button(Button::Ultimate);
                self.draw_ultimate(&mut painter, &layout.ultimate, state, press, ultimate, scale);
            }
            if let Some(party) = &self.values.party {
                let press = self.press[Button::Swap.index()];
                let state = actions.button(Button::Swap);
                draw_swap(&mut painter, &layout.swap, state, press, party, scale);
            }
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
        if trail.trail > values.hp {
            // Where what's left ends and what was just lost begins, as an inked edge rather than the
            // change from the fill's colour to ember alone (M4 A5, m4-plan §3 "Colour").
            let half = 0.5 * TRAIL_EDGE * scale / bars.hp.width().max(1.0);
            painter.bar(bars.hp, SLANT, (values.hp - half, values.hp + half), Paint::fill(INK));
        }
        painter.bar(bars.poise, SLANT, (0.0, 1.0), Paint::outlined(INK, faded(PALE, 0.7), scale));
        painter.bar(bars.poise, SLANT, (0.0, values.poise), Paint::fill(AMBER));
        if let Some(points) = values.hp_points {
            let digits = Paint::outlined(WHITE, OUTLINE, 1.6 * scale);
            let height = bars.digit_height * self.text_scale;
            painter.digits(points, bars.digits, height, bars.digits_align, SLANT, digits);
        }
    }

    /// A small bar for each other foe, side by side about the row's middle, with no trail: what it's for is
    /// to say how many are left and how they stand, and the big one says what a hit did.
    fn draw_others(&self, painter: &mut Painter<'_>, layout: &HudLayout) {
        let bars = self.values.others.bars();
        let scale = layout.scale;
        let row = layout.others;
        let (pitch, gap) = (OTHER_BAR_WIDTH * scale, OTHER_BAR_GAP * scale);
        // A frame too narrow for a whole row gets thinner bars rather than bars over the player's.
        let pitch = pitch.min(row.width() / MAX_OTHER_FOES as f32);
        let across = pitch * bars.len() as f32;
        let edge = row.min.x + (row.width() - across) * 0.5;
        for (index, hp) in bars.iter().enumerate() {
            let left = edge + pitch * index as f32;
            let bar = Rect::new(
                Vec2::new(left + gap * 0.5, row.min.y),
                Vec2::new(left + pitch - gap * 0.5, row.max.y),
            );
            painter.bar(bar, SLANT, (0.0, 1.0), Paint::outlined(INK, faded(PALE, 0.7), scale));
            painter.bar(bar, SLANT, (0.0, *hp), Paint::fill(ROSE));
        }
    }

    fn draw_skill_state(&self, painter: &mut Painter<'_>, skill: &ButtonLayout, scale: f32) {
        let sprites = painter.sprites();
        let press = 1.0 - 0.08 * self.press[2];
        let size = Vec2::splat(skill.half * 2.0 * press);
        let cooldown = self.values.skill;
        if cooldown.cooldown > 0.0 {
            // What's still to go, darkened: a wedge that closes clockwise-backward toward the top.
            let dark = Paint::fill(WAITING).swept(cooldown.cooldown);
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

    /// The player's energy meter: violet as it charges, nearly white with a violet glow once full.
    fn draw_energy(&self, painter: &mut Painter<'_>, rect: Rect, ultimate: &UltimateValues, scale: f32) {
        painter.bar(rect, SLANT, (0.0, 1.0), Paint::outlined(INK, faded(PALE, 0.7), scale));
        let fill = if ultimate.ready {
            Paint::outlined(LIT, faded(VIOLET, self.breath()), 1.6 * scale).soft(0.6 * scale)
        } else {
            Paint::fill(VIOLET)
        };
        painter.bar(rect, SLANT, (0.0, ultimate.energy), fill);
    }

    /// The ultimate's button: the buttons' shape with an eight-point burst, its energy a ring around it
    /// filling clockwise from the top. Ready, the ring closes nearly white, the ink turns violet and a
    /// halo breathes behind it.
    fn draw_ultimate(
        &self,
        painter: &mut Painter<'_>,
        layout: &ButtonLayout,
        state: &ButtonState,
        press: f32,
        ultimate: &UltimateValues,
        scale: f32,
    ) {
        let sprites = painter.sprites();
        let center = layout.center;
        let size = Vec2::splat(layout.half * 2.0 * (1.0 - 0.08 * press));
        if ultimate.ready {
            let halo = Paint::fill(faded(VIOLET, 0.25 + 0.35 * self.breath())).soft(5.0 * scale);
            painter.sprite(sprites.button, center, size * ULTIMATE_HALO, SLANT, halo);
        }
        let shadow = Paint::fill(SHADOW).grown(2.0 * scale).soft(4.0 * scale);
        painter.sprite(sprites.button, center, size, SLANT, shadow);
        let (ink, edge) = match (state.down, ultimate.ready) {
            (true, _) => ([38, 42, 56, 245], WHITE),
            (false, true) => (VIOLET_INK, LIT),
            (false, false) => (INK, PALE),
        };
        painter.sprite(sprites.button, center, size, SLANT, Paint::outlined(ink, edge, 1.8 * scale));
        // The burst: the skill's spark, and a smaller one turned an eighth behind its points.
        let (glyph, share) = if ultimate.ready { (LIT, 1.0) } else { (WHITE, CHARGING_ICON) };
        let glyph = Paint::fill(faded(glyph, share));
        let diagonal = Vec2::new(std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2);
        painter.turned(sprites.spark, center, size * 0.44, diagonal, glyph);
        painter.sprite(sprites.spark, center, size * 0.62, SLANT * 0.5, glyph);

        // The energy: a faint track all round, and what's held over it.
        let ring = size * ULTIMATE_RING;
        let width = 3.0 * scale;
        if ultimate.ready {
            painter.sprite(sprites.button, center, ring, SLANT, Paint::ring(LIT, width));
        } else {
            painter.sprite(sprites.button, center, ring, SLANT, Paint::ring(faded(PALE, 0.3), width));
            if ultimate.energy > 0.0 {
                let held = Paint::ring(VIOLET, width).swept(ultimate.energy);
                painter.sprite(sprites.button, center, ring, SLANT, held);
            }
        }
        if let Some(ready_at) = self.ultimate_ready_at {
            let age = seconds_since(self.now, ready_at);
            if age < READY_FLASH {
                let share = 1.0 - age / READY_FLASH;
                let flash = Paint::outlined([0; 4], faded(LIT, share), 3.0 * scale)
                    .soft(1.5 * scale)
                    .grown(age / READY_FLASH * 8.0 * scale);
                painter.sprite(sprites.button, center, ring, SLANT, flash);
            }
        }
    }

    /// A ready ultimate's glow, 0.4 to 1 and back once a breath, on the real clock: arithmetic alone, so
    /// every platform draws the same frame.
    fn breath(&self) -> f32 {
        let phase = (self.now.as_nanos() % BREATH_NANOS) as f32 / BREATH_NANOS as f32;
        let rise = 1.0 - (2.0 * phase - 1.0).abs();
        0.4 + 0.6 * rise * rise * (3.0 - 2.0 * rise)
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
        let height = height * scale * self.text_scale * punch;
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
        boss: values.boss,
        others: OtherFoes {
            hp: values.others.hp.map(share),
            count: values.others.count.min(MAX_OTHER_FOES as u8),
        },
        skill: SkillValues { cooldown: share(values.skill.cooldown), ..values.skill },
        ultimate: values
            .ultimate
            .map(|ultimate| UltimateValues { energy: share(ultimate.energy), ..ultimate }),
        party: values.party.map(|party| PartyValues {
            members: party.members.map(|member| PartyMember {
                hp: share(member.hp),
                rest: share(member.rest),
                ..member
            }),
            count: party.count.min(MAX_PARTY as u8),
            swap: SkillValues { cooldown: share(party.swap.cooldown), ..party.swap },
        }),
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
    let (size, glyph) = draw_button_shape(painter, layout, state, press, scale);
    painter.sprite(icon, layout.center, size * 0.62, SLANT * 0.5, Paint::fill(faded(glyph, icon_share)));
    if state.dragging && state.drag != Vec2::ZERO {
        // Where the drag points, just outside the button. `drag` is +y up, the screen's +y down.
        let up = Vec2::new(state.drag.x, -state.drag.y).normalize();
        let center = layout.center + up * (layout.half * 1.32);
        let wedge = Paint::outlined(AMBER, OUTLINE, 1.2 * scale);
        painter.turned(painter.sprites().wedge, center, Vec2::splat(20.0 * scale), up, wedge);
    }
}

/// A button's shadow and its cut shape, as `state` and its eased `press` have it, returning the size it's
/// drawn at and its glyph's colour. Pressed, the ink lifts and the edge goes white; held past its hold, the
/// whole shape turns amber and its glyph goes dark, which still reads with a thumb over most of it.
fn draw_button_shape(
    painter: &mut Painter<'_>,
    layout: &ButtonLayout,
    state: &ButtonState,
    press: f32,
    scale: f32,
) -> (Vec2, [u8; 4]) {
    let sprites = painter.sprites();
    let shrink = 1.0 - 0.08 * press;
    let size = Vec2::splat(layout.half * 2.0 * shrink);
    let shadow = Paint::fill(SHADOW).grown(2.0 * scale).soft(4.0 * scale);
    painter.sprite(sprites.button, layout.center, size, SLANT, shadow);
    let (ink, edge, glyph) = if state.held {
        (AMBER, WHITE, OUTLINE)
    } else if state.down {
        ([38, 42, 56, 245], WHITE, WHITE)
    } else {
        (INK, PALE, WHITE)
    };
    painter.sprite(sprites.button, layout.center, size, SLANT, Paint::outlined(ink, edge, 1.8 * scale));
    (size, glyph)
}

/// The swap's button: the buttons' cut shape with two arrows passing, the one up on the left. Whoever's
/// on the field still has to stay a while: the wait closes over it as the skill's cooldown does, with its
/// seconds. With nobody to bring in, its arrows dim.
fn draw_swap(
    painter: &mut Painter<'_>,
    layout: &ButtonLayout,
    state: &ButtonState,
    press: f32,
    party: &PartyValues,
    scale: f32,
) {
    let (size, glyph) = draw_button_shape(painter, layout, state, press, scale);
    let cooldown = party.swap;
    let waiting = cooldown.cooldown > 0.0;
    let share = if waiting || !party.has_next() { COOLING_ICON } else { 1.0 };
    let glyph = Paint::fill(faded(glyph, share));
    let side = size.x;
    // Along the icons' lean: their tops a half slant to the right.
    let up = Vec2::new(SLANT * 0.5, -1.0).normalize();
    let at = |across: f32, along: f32| layout.center + Vec2::new(across * side, 0.0) + up * (along * side);
    let sprites = painter.sprites();
    for (across, way) in [(-0.13, 1.0), (0.13, -1.0)] {
        // A stem, and a head at its far end.
        painter.bar_along(at(across, -0.11 * way), up, 0.34 * side, 0.09 * side, glyph);
        painter.turned(sprites.wedge, at(across, 0.16 * way), Vec2::splat(0.26 * side), up * way, glyph);
    }
    if waiting {
        let dark = Paint::fill(WAITING).swept(cooldown.cooldown);
        painter.sprite(sprites.button, layout.center, size, SLANT, dark);
        if let Some(seconds) = cooldown.seconds {
            let digits = Paint::outlined(WHITE, OUTLINE, 1.6 * scale);
            painter.digits(seconds, layout.center, 26.0 * scale, Align::Center, SLANT, digits);
        }
    }
}

/// The party's slots: for each member, a chip with its place in the party, its health beside it, and what's
/// left of its rest. Whoever's on the field has a lit edge and a pointer; a fallen member is struck through.
fn draw_party(
    painter: &mut Painter<'_>,
    layout: &PartyLayout,
    party: &PartyValues,
    text_scale: f32,
    scale: f32,
) {
    let sprites = painter.sprites();
    let size = Vec2::splat(layout.chip_half * 2.0);
    for ((place, member), row) in party.members().iter().enumerate().zip(&layout.rows) {
        let chip = row.chip;
        let shadow = Paint::fill(SHADOW).grown(1.5 * scale).soft(3.0 * scale);
        painter.sprite(sprites.button, chip, size, SLANT, shadow);
        let edge = if member.on_field {
            Paint::outlined(INK, WHITE, 2.4 * scale)
        } else {
            Paint::outlined(INK, faded(PALE, 0.8), 1.4 * scale)
        };
        painter.sprite(sprites.button, chip, size, SLANT, edge);
        let resting = member.rest > 0.0 && !member.on_field && !member.down;
        let shown = if member.down || resting { RESTING_NUMBER } else { 1.0 };
        let number = Paint::outlined(faded(WHITE, shown), faded(OUTLINE, shown), 1.2 * scale);
        painter.digits(place as u32 + 1, chip, layout.digit_height, Align::Center, SLANT, number);
        if resting {
            // Over its number, as the skill's wait is over its icon: on the chip's ink alone it wouldn't show.
            painter.sprite(sprites.button, chip, size, SLANT, Paint::fill(WAITING).swept(member.rest));
        }
        if member.on_field {
            // Pointing at it from the margin, in the player's colour.
            let wedge = Paint::outlined(MINT, OUTLINE, 1.0 * scale);
            let at = chip - Vec2::new(layout.chip_half + 9.0 * scale, 0.0);
            painter.turned(sprites.wedge, at, Vec2::splat(11.0 * scale), Vec2::X, wedge);
        }
        if member.down {
            // Struck through, leaning as everything does, so it reads without its colour.
            let along = Vec2::new(1.0, -1.0 + SLANT).normalize();
            let strike = Paint::outlined(ROSE, OUTLINE, 1.2 * scale);
            painter.bar_along(chip, along, size.x * 1.3, 3.5 * scale, strike);
        }
        painter.bar(row.hp, SLANT, (0.0, 1.0), Paint::outlined(INK, faded(PALE, 0.7), scale));
        painter.bar(row.hp, SLANT, (0.0, member.hp), Paint::fill(MINT));
        if let (Some(seconds), false) = (member.seconds, member.on_field || member.down) {
            let digits = Paint::outlined(WHITE, OUTLINE, 1.4 * scale);
            let height = layout.digit_height * text_scale;
            painter.digits(seconds, row.digits, height, Align::Left, SLANT, digits);
        }
    }
}

/// An inked mark across a boss's health where each of its phases falls, leaning as the bar does.
fn draw_marks(painter: &mut Painter<'_>, layout: &HudLayout, boss: &BossBar) {
    let hp = layout.boss.hp;
    let half = layout.mark_width * 0.5;
    for &share in boss.marks() {
        let x = hp.min.x + hp.width() * share;
        let mark = Rect::new(Vec2::new(x - half, hp.min.y), Vec2::new(x + half, hp.max.y));
        painter.bar(mark, SLANT, (0.0, 1.0), Paint::fill(INK));
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
