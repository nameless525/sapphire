//! The look pane's model (M6 T15a): which view of the look-dev stage the viewport shows, at which tier and hour, of
//! which side of an A/B, and what the town runs on, from the current document. The pane itself is
//! `app::look`; the stage is the look-dev stage's one, stood by `sapphire_stage::lookdev::stand` from the
//! description a plain `sapphire_sandbox --look-dev` capture stands by ([`LookView::desc`]), drawn by
//! [`crate::viewport`].
//!
//! - **What a document gives:** a complete day (`lookdev_day.ron`, `default_day.ron`) is the town's day; a
//!   partial one (an atmosphere volume's look, `content/looks/biomes/`) is laid over the look-dev day across the
//!   whole town, as the volume's look is where the camera stands in it; a light rig library is the characters'
//!   presets over the look-dev day. Each shows as it reads now, every edit with no cook, since the stage takes
//!   values and cooks none of these.
//! - **A/B:** A is the document as saved, B as edited, at the same view; the pane shows one at a time, whole, and
//!   [`crate::commands::Command::Compare`] flips between them. Each side settles as a capture of it would.
//! - **The hour** is the day's own evaluation at it, as the game's clock gives it ([`sapphire_tod::TodTimeline`]):
//!   the stage stands still there, the same frame a sandbox capture at `--time-of-day <hour>` draws.

use std::sync::Arc;

use sapphire_platform::Host;
use sapphire_render::quality::TierId;
use sapphire_rhi::GpuContext;
use sapphire_stage::lookdev::{LookDevDesc, Shot};
use sapphire_stage::scene::TownLook;
use sapphire_tod::light_rig::LightRigLibrary;
use sapphire_tod::{DAY_HOURS, TodLook, TodTimeline};
use serde::{Deserialize, Serialize};

/// The look-dev shots the pane offers: the character's three-quarter view, its face, and across the square.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LookShot {
    #[default]
    Hero,
    Face,
    Wide,
}

impl LookShot {
    pub const ALL: [Self; 3] = [Self::Hero, Self::Face, Self::Wide];

    /// The stage's shot.
    #[must_use]
    pub const fn shot(self) -> Shot {
        match self {
            Self::Hero => Shot::Hero,
            Self::Face => Shot::Face,
            Self::Wide => Shot::Wide,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hero => "Hero",
            Self::Face => "Face",
            Self::Wide => "Wide",
        }
    }
}

/// Which side of the A/B shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// A: the document as its file holds it.
    Saved,
    /// B: as edited, unsaved edits and all.
    #[default]
    Edited,
}

impl Side {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Saved => "A: saved",
            Self::Edited => "B: edited",
        }
    }
}

/// How the look pane shows the stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookView {
    pub shot: LookShot,
    pub tier: TierId,
    /// Hours since midnight, `0 ≤ hour < 24`.
    pub hour: f32,
    pub side: Side,
}

impl Default for LookView {
    /// The hero shot on Desktop-High at 13:00, `lookdev-town-day`'s hour, as edited.
    fn default() -> Self {
        Self { shot: LookShot::Hero, tier: TierId::DesktopHigh, hour: 13.0, side: Side::Edited }
    }
}

impl LookView {
    /// The look-dev stage's description of it on `gpu` with `host`'s content, its town running on `look`: what
    /// `sapphire_sandbox --look-dev <shot> --tier <tier> --time-of-day <hour>` with `--day` and `--light-rigs` files
    /// holding `look` stands (a partial day's `--day` lies over the town's, as here), the heroine standing in it.
    #[must_use]
    pub fn desc(&self, host: &Host, gpu: &GpuContext, look: TownLook) -> LookDevDesc {
        let view = (self.shot.shot(), self.tier, self.hour);
        LookDevDesc::new(host, gpu, view, look, game_combat::playable::KIND)
    }
}

/// An hour as the pane takes it: within the day, refused if it isn't a number.
pub fn hour(hours: f32) -> Result<f32, String> {
    if !hours.is_finite() {
        return Err(format!("an hour is a number of hours, not {hours}"));
    }
    if !(0.0..DAY_HOURS).contains(&hours) {
        return Err(format!("an hour is from 0 to below 24, not {hours}"));
    }
    Ok(hours)
}

/// What a document gives the town to run on.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    Day(&'a TodTimeline),
    LightRigs(&'a LightRigLibrary),
}

/// The town's look with `source` in it: a complete day replacing `town`'s day, a partial one over it, or a library
/// replacing its rigs, the rest `town`'s, which is the town's own ([`TownLook::default`]).
#[must_use]
pub fn town_look(source: Source<'_>, town: &TownLook) -> TownLook {
    let mut look = town.clone();
    match source {
        Source::Day(day) => look.take_day(day.clone()),
        Source::LightRigs(rigs) => look.rigs = rigs.clone(),
    }
    look
}

/// What the viewport shows in the look pane: the view, and the town's look at it, shared with the pane, which reads
/// it again only when its document changes.
#[derive(Clone, Debug, PartialEq)]
pub struct LookShown {
    pub view: LookView,
    pub look: Arc<TownLook>,
    /// Why the stage shows a look other than the document's side: it doesn't read, so the stage stays up on the
    /// last look that read (or the town's own), and the bar says so.
    pub note: Option<String>,
}

/// One curve the pane draws through a day: a number of the day's look.
#[derive(Clone, Copy, Debug)]
pub struct Channel {
    pub name: &'static str,
    pub of: fn(&TodLook) -> f32,
}

/// The day's numbers the pane draws, each over its own range: the sun's height (its direction's downward share),
/// the exposure in stops, day to night, the stars and the fog's near density.
pub const CHANNELS: [Channel; 5] = [
    Channel { name: "Sun height", of: |look| -look.sun.direction.normalize_or_zero().y },
    Channel { name: "Exposure", of: |look| look.exposure },
    Channel { name: "Day to night", of: |look| look.day_night },
    Channel { name: "Stars", of: |look| look.stars },
    Channel { name: "Fog", of: |look| look.fog.near.density },
];

/// How many points a curve takes through a day: one each 10 minutes.
pub const SAMPLES: usize = 144;

/// A channel's curve through `day`, [`SAMPLES`] points from midnight, and its least and greatest.
#[must_use]
pub fn curve(day: &TodTimeline, channel: Channel) -> (Vec<f32>, f32, f32) {
    let values: Vec<f32> = (0..SAMPLES)
        .map(|sample| (channel.of)(&day.evaluate(sample as f32 * DAY_HOURS / SAMPLES as f32)))
        .collect();
    let least = values.iter().copied().fold(f32::INFINITY, f32::min);
    let most = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    (values, least, most)
}

/// A key's time as a drag writes it, `hours` held to the day and to the minute, between `before` and `after`, the
/// key's neighbours' times, if it has them: a key dragged onto or past one stops a minute short of it, so the keys'
/// times still increase, as a day must.
#[must_use]
pub fn dragged_time(hours: f32, before: Option<f32>, after: Option<f32>) -> f32 {
    const MINUTE: f32 = 1.0 / 60.0;
    let low = before.map_or(0.0, |time| time + MINUTE);
    let high = after.map_or(DAY_HOURS - MINUTE, |time| time - MINUTE);
    let minutes = (hours.clamp(low, high.max(low)) * 60.0).round();
    (minutes / 60.0).clamp(low, high.max(low))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOKDEV_DAY: &str = include_str!("../../../content/lookdev/lookdev_day.ron");
    const CAVE: &str = include_str!("../../../content/looks/biomes/cave_day.ron");

    /// A complete day is the town's day, a partial one lies over the town's own, and a library is its rigs.
    #[test]
    fn a_document_gives_the_town_what_it_is() {
        let town = TownLook::default();
        let day = TodTimeline::from_ron(LOOKDEV_DAY).expect("the look-dev day");
        let look = town_look(Source::Day(&day), &town);
        assert_eq!((look.day, look.over), (day, None));
        let cave = TodTimeline::from_ron(CAVE).expect("the cave");
        let look = town_look(Source::Day(&cave), &town);
        assert_eq!(look.over.as_ref(), Some(&cave), "a partial day lies over");
        assert_eq!(look.day, TownLook::default().day, "the town's own day under it");
    }

    /// An hour is within the day and a number.
    #[test]
    fn an_hour_is_within_the_day() {
        assert_eq!(hour(0.0), Ok(0.0));
        assert_eq!(hour(23.5), Ok(23.5));
        for wrong in [24.0, -0.5, f32::NAN, f32::INFINITY] {
            assert!(hour(wrong).is_err(), "{wrong}");
        }
    }

    /// A drag lands on a minute, inside the day, short of the key's neighbours.
    #[test]
    fn a_drag_stops_short_of_the_neighbours() {
        assert_eq!(dragged_time(13.254, Some(9.0), Some(16.0)), 13.25);
        assert!((dragged_time(20.0, Some(9.0), Some(16.0)) - (16.0 - 1.0 / 60.0)).abs() < 1e-5);
        assert!((dragged_time(1.0, Some(9.0), Some(16.0)) - (9.0 + 1.0 / 60.0)).abs() < 1e-5);
        assert_eq!(dragged_time(-3.0, None, Some(4.75)), 0.0);
        assert!(dragged_time(30.0, Some(19.75), None) < DAY_HOURS);
    }

    /// Each curve is a number through the day, and the sun is up at noon and down at midnight.
    #[test]
    fn curves_follow_the_day() {
        let day = TodTimeline::from_ron(LOOKDEV_DAY).expect("the look-dev day");
        for channel in CHANNELS {
            let (values, least, most) = curve(&day, channel);
            assert_eq!(values.len(), SAMPLES);
            assert!(least.is_finite() && most.is_finite() && least <= most, "{}", channel.name);
        }
        let (sun, _, _) = curve(&day, CHANNELS[0]);
        assert!(sun[SAMPLES / 2] > 0.0 && sun[0] < 0.0, "noon {} midnight {}", sun[SAMPLES / 2], sun[0]);
    }
}
