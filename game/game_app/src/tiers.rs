//! The quality tier in the game (M4 T3): the graphics page's settings given to the stage's quality, heat to its
//! thermal ladder each frame, a tier change at a pause or a wipe, and the frame cap to the platform's pacing.
//!
//! **The battery saver** caps frames at 30 and holds the ladder at least as deep as `Light` heat would (its
//! first rung: a desktop's 0.85 scale, a phone's optional passes), whatever the device reports: the two
//! savings spec §6.2 and m4-plan §3's ladder name first, neither touching the look's pillars. Heat past `Light`
//! takes the ladder deeper as it would anyway.
//!
//! **Pacing:** a phone's cap is its frame rate, which its display link or limiter paces to
//! (`Platform::set_frame_rate`): the least of the tier's rate, the ladder's cap and the player's 30 or 60. A
//! desktop's player cap, under the ladder's, gates frames on the clock (`Platform::set_frame_cap`), and the
//! frame rate frames are measured against is that cap, or the tier's while there's none.
//!
//! What a frame does here is [`TierFollower::frame`], which `GameApp` calls before it draws and a lane can call
//! with heat of its own.

use std::num::NonZeroU32;

use game_flow::show::GameShow;
use game_ui::menus::{FrameCap, PlayerSettings, QualityChoice};
use sapphire_core::target::FormFactor;
use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_render::quality::{Quality, QualitySettings, TierId};
use sapphire_stage::Stage;

use crate::LOG_TARGET;

/// The graphics page's settings, as the device's class has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Graphics {
    pub class: FormFactor,
    pub quality: QualityChoice,
    pub frame_cap: FrameCap,
    pub dynamic_resolution: bool,
    pub battery_saver: bool,
}

impl Graphics {
    /// What `settings` say, on a device of `class`.
    #[must_use]
    pub fn of(settings: &PlayerSettings, class: FormFactor) -> Self {
        Self {
            class,
            quality: settings.quality,
            frame_cap: settings.frame_cap.on(class),
            dynamic_resolution: settings.dynamic_resolution,
            battery_saver: settings.battery_saver,
        }
    }

    /// The quality settings these make of `base` (the stage's, the sandbox's flags' elsewhere): the tier the
    /// player holds, if any, and dynamic resolution.
    #[must_use]
    pub fn quality_settings(self, base: QualitySettings) -> QualitySettings {
        QualitySettings {
            tier: self.quality.tier(self.class),
            dynamic_resolution: self.dynamic_resolution,
            ..base
        }
    }

    /// The heat the ladder follows: the device's, at least `Light` under the battery saver.
    #[must_use]
    pub fn heat(self, reading: ThermalReading) -> ThermalReading {
        if self.battery_saver && reading.state < ThermalState::Light {
            return ThermalReading { state: ThermalState::Light, ..reading };
        }
        reading
    }

    /// The player's cap: 30 under the battery saver.
    #[must_use]
    pub fn cap(self) -> Option<NonZeroU32> {
        if self.battery_saver { FrameCap::Thirty.rate() } else { self.frame_cap.rate() }
    }

    /// How frames are paced under `quality`'s tier and ladder.
    #[must_use]
    pub fn pacing(self, quality: &Quality) -> Pacing {
        let ladder = quality.ladder().state().frame_rate.and_then(NonZeroU32::new);
        pacing(self.class, quality.frame_rate(), ladder, self.cap())
    }
}

/// The quality a game on `show` starts with on a device of `class`: the player's tier, if they hold one, and
/// dynamic resolution, so that the first renderer made is the one they chose.
#[must_use]
pub fn starting_quality(show: &GameShow, class: FormFactor) -> QualitySettings {
    show.menus().map_or_else(QualitySettings::default, |menus| {
        Graphics::of(menus.settings(), class).quality_settings(QualitySettings::default())
    })
}

/// What the platform paces frames to: [`Pacing::rate`] for `Platform::set_frame_rate` (a phone's pace, a
/// desktop's measure) and [`Pacing::cap`] for `Platform::set_frame_cap` (a desktop's gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pacing {
    pub rate: Option<NonZeroU32>,
    pub cap: Option<NonZeroU32>,
}

/// How a device of `class` paces frames: `tier_rate` is the tier's rate under the ladder's cap
/// ([`Quality::frame_rate`]), `ladder` that cap alone, and `player` the player's cap (`None`: unlimited).
#[must_use]
pub fn pacing(
    class: FormFactor,
    tier_rate: Option<NonZeroU32>,
    ladder: Option<NonZeroU32>,
    player: Option<NonZeroU32>,
) -> Pacing {
    let least = |a: Option<NonZeroU32>, b: Option<NonZeroU32>| match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    match class {
        FormFactor::Mobile => Pacing { rate: least(tier_rate, player), cap: None },
        FormFactor::Desktop => {
            let cap = least(player, ladder);
            Pacing { rate: cap.or(tier_rate), cap }
        }
    }
}

/// The tier in the game, frame by frame (this module).
#[derive(Debug)]
pub struct TierFollower {
    graphics: Graphics,
    /// What the platform was last told, so it's told again only on a change.
    paced: Option<Pacing>,
    /// The tiers it changed to, in order: for a lane to read.
    changes: Vec<TierId>,
}

impl TierFollower {
    /// Follows `settings` on a device of `class`, starting with the next [`Self::frame`].
    #[must_use]
    pub fn new(settings: &PlayerSettings, class: FormFactor) -> Self {
        Self { graphics: Graphics::of(settings, class), paced: None, changes: Vec::new() }
    }

    pub fn graphics(&self) -> Graphics {
        self.graphics
    }

    /// The tiers it has changed to.
    pub fn changes(&self) -> &[TierId] {
        &self.changes
    }

    /// A new stage stands (a boot, or a boot again after a lost device): its first frame tells the platform how
    /// to pace, whatever the last stage's did.
    pub fn stood(&mut self) {
        self.paced = None;
    }

    /// Takes the player's new settings: the quality's from the next frame, a tier they choose at the next pause
    /// or wipe once its renderer's built.
    pub fn follow_settings(&mut self, settings: &PlayerSettings, stage: &mut Stage<GameShow>) {
        self.graphics = Graphics::of(settings, self.graphics.class);
        let quality = self.graphics.quality_settings(stage.quality.settings());
        stage.quality.set_settings(quality);
    }

    /// Before a frame's drawn: `reading` (the platform's heat) to the ladder, the wanted tier's renderer built
    /// off the frame, and changed to while the show holds still. Returns how to pace frames when that changed.
    pub fn frame(&mut self, stage: &mut Stage<GameShow>, reading: ThermalReading) -> Option<Pacing> {
        stage.thermal = self.graphics.heat(reading);
        stage.prepare_tier_change();
        if stage.show.holds_still()
            && let Some(tier) = stage.take_tier_change()
        {
            tracing::info!(target: LOG_TARGET, tier = tier.name(), "the game changed its quality tier");
            self.changes.push(tier);
        }
        let pacing = self.graphics.pacing(&stage.quality);
        (self.paced != Some(pacing)).then(|| {
            self.paced = Some(pacing);
            tracing::info!(target: LOG_TARGET, ?pacing, "frame pacing");
            pacing
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(hz: u32) -> Option<NonZeroU32> {
        NonZeroU32::new(hz)
    }

    #[test]
    fn a_phone_paces_to_the_least_and_a_desktop_gates_by_its_cap() {
        use FormFactor::{Desktop, Mobile};
        // A phone: the tier's 60, the player's 30 or 60, the ladder's 30 inside the tier's rate.
        assert_eq!(pacing(Mobile, rate(60), None, rate(60)), Pacing { rate: rate(60), cap: None });
        assert_eq!(pacing(Mobile, rate(60), None, rate(30)), Pacing { rate: rate(30), cap: None });
        assert_eq!(pacing(Mobile, rate(30), rate(30), rate(60)), Pacing { rate: rate(30), cap: None });
        // A desktop, unlimited: frames as the display shows them, measured against the tier's rate, as before.
        assert_eq!(pacing(Desktop, rate(60), None, None), Pacing { rate: rate(60), cap: None });
        // Capped by the player, or the ladder's cap under it.
        assert_eq!(pacing(Desktop, rate(60), None, rate(120)), Pacing { rate: rate(120), cap: rate(120) });
        assert_eq!(pacing(Desktop, rate(30), rate(30), rate(120)), Pacing { rate: rate(30), cap: rate(30) });
        assert_eq!(pacing(Desktop, rate(30), rate(30), None), Pacing { rate: rate(30), cap: rate(30) });
    }

    #[test]
    fn the_battery_saver_caps_at_30_and_holds_light_heat() {
        let settings = PlayerSettings { battery_saver: true, ..PlayerSettings::default() };
        let graphics = Graphics::of(&settings, FormFactor::Desktop);
        assert_eq!(graphics.cap(), rate(30));
        let cool = ThermalReading { state: ThermalState::Nominal, ..ThermalReading::default() };
        assert_eq!(graphics.heat(cool).state, ThermalState::Light);
        assert_eq!(
            graphics.heat(ThermalReading::default()).state,
            ThermalState::Light,
            "a desktop's unknown"
        );
        let hot = ThermalReading { state: ThermalState::Severe, ..ThermalReading::default() };
        assert_eq!(graphics.heat(hot).state, ThermalState::Severe, "hotter goes deeper anyway");
        let off = Graphics::of(&PlayerSettings::default(), FormFactor::Desktop);
        assert_eq!((off.heat(cool), off.cap()), (cool, None));
    }

    #[test]
    fn the_settings_choose_a_tier_and_dynamic_resolution() {
        let settings = PlayerSettings {
            quality: QualityChoice::Low,
            dynamic_resolution: true,
            ..PlayerSettings::default()
        };
        let base = QualitySettings { render_scale: Some(0.9), ..QualitySettings::default() };
        let phone = Graphics::of(&settings, FormFactor::Mobile).quality_settings(base);
        assert_eq!(phone.tier, Some(TierId::MobileLow));
        assert!(phone.dynamic_resolution);
        assert_eq!(phone.render_scale, Some(0.9), "what else the base says stays");
        let automatic = Graphics::of(&PlayerSettings::default(), FormFactor::Desktop).quality_settings(base);
        assert_eq!((automatic.tier, automatic.dynamic_resolution), (None, false));
        // A desktop's file says unlimited; a phone reads it as its 60.
        assert_eq!(Graphics::of(&PlayerSettings::default(), FormFactor::Mobile).cap(), rate(60));
    }
}
