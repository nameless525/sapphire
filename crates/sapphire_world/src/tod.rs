//! Time of day (M1 W1; look pillar 1, M1 plan §2): one set of art-directed keys drives the sky
//! gradient and its layers (the sun's and moon's discs and glows, the stars, the clouds, M1 W2;
//! the nebula, M1 X1 part 2e),
//! the sun and moon, the key light, ambient light, fog, the shadow tint, the character rig's key
//! light, the day/night ramp blend and post-processing's exposure, bloom, grade and vignette, and
//! its compositing layers (the diffusion glow, characters' bloom, gradient layers and the lens
//! flare; M1 P2), so characters and world change together and read as one picture. As in
//! Wuthering Waves, it is a stylized day, not a physical atmosphere.
//!
//! The model, the keys, timelines and looks and how they resolve and blend, is
//! [`sapphire_tod`]'s, all of it re-exported here; this module adds time of day's runtime in the
//! world. The model is a crate of its own, depending on neither the renderer nor this crate, so
//! tools that need a day's light (the pipeline's probe bake) link it alone, and an edit to a
//! shader or to the default day this crate compiles in rebuilds none of them (M1 A8's follow-up).
//!
//! # Data
//!
//! - **[`TodTimeline`]** (a `TodTimeline` RON document): [`TodKey`]s at hours of the day, each
//!   setting groups of values ([`TodGroup`]), the optional ones ([`TodGroup::OPTIONAL`]) aside
//!   in the base timeline. [`default_day`] is the shipped one, compiled in until packs (A4) carry
//!   timelines as content.
//! - **[`TodVolume`]**: a zone's override, a timeline that sets every group or only some, blended
//!   over the base by its weight at a probe point (smoothly out over a blend distance around its
//!   sphere or box), lowest priority first.
//! - **[`TodCutscene`]** (a `TodCutscene` document): one shot's track, setting the time of day,
//!   overriding values over the shot's seconds, lighting its characters with a light rig of its
//!   own (M1 C2, [`crate::light_rig`]), or any of these together, easing in and out.
//!
//! # Evaluation
//!
//! As the model's ([`sapphire_tod`]'s "Evaluation"): each group blends between keys by the first
//! key's [`TodEase`], colors in OkLab, grades as post's two LUTs ([`GradeMix`]), and the key
//! light resolves from the sun and moon ([`TodLook::key_light`]), the character key fading with
//! it at the horizon ([`TodLook::character_key`]). Volumes and a cutscene's track
//! blend over the base timeline the same way, by their weights, and one at full weight
//! reproduces its keys' values bit for bit.
//!
//! # Clock and frame
//!
//! [`TimeOfDay`] (an ECS resource) holds the base timeline, the zone volumes and a playing
//! cutscene track (which runs on the world's clock), on a [`TodClock`]: a clock domain of its
//! own (spec §4.5), issued by the world clock so pause, slow motion and cinematics reach it,
//! with a cycle rate (a day in N seconds, or time standing still) and a developer pause of its
//! own. [`tick_time_of_day`] advances it in
//! the fixed step; once per frame [`TimeOfDay::resolve`] produces a [`TodFrame`], which the app
//! writes into the frame packet's scene constants ([`TodFrame::write`]). The renderer sees only
//! those plain values (rule R4), which are `sapphire_look`'s, beneath both.
//!
//! Resolving allocates nothing and costs under a microsecond of CPU on an Apple M5: about 0.1 µs
//! while the hour stands still and 0.55 µs while it runs (the budget is 0.05 ms);
//! [`TodStats`] reports its time, the segment, active volumes, the cutscene weight and any grade
//! weight post's two LUTs couldn't show (rule R9).
//!
//! # Consumers
//!
//! The packet's sky (its gradient and layers, the bodies and the celestial sphere's turn, W2), key
//! light, ambient light and post-processing with its compositing layers (P2) are drawn today, the ambient through the character
//! light rig's GI (C2), as is the character key light, which reaches characters through their
//! rigs ([`TodFrame::character_lighting`], [`crate::light_rig`]). The day/night factor is C1's ramp
//! blend (the frame uniforms' `day_night`), and fog is drawn over the world, the characters and
//! the sky, and lights fog cards (W3). The rest is carried for the packages that read it: ambient
//! on the environment (W4, W5) and the shadow tint (R4, W4).
//!
//! The emission scale (M1 W1's keyed emission) multiplies the environment's authored emission
//! (W4), but for the share a material keeps steady, so lanterns and lit windows rest pale by day
//! and glow at night under the same exposure; the probe bake (W5) lights each of its keys'
//! emission by that key's scale. Characters' own glow (C7's emission layers) is part of their
//! design and shows the same at every hour, so it never follows the scale; foliage and fog cards
//! have no emission.
//!
//! # Later milestones
//!
//! - **Weather** as partial overrides between zones and cutscenes (the research notes' weather
//!   layer): cloud cover and tones, a storm's sky. Scheduled switches with a fade (local lights on
//!   at night) rather than blends.
//! - **The sky's pole per zone** (its latitude) once zones are data; a constant until then.
//! - **More LUTs**: a zone that changes the grade while time crosses between keys of different
//!   grades can need three grades at once; post's two LUTs drop the lightest
//!   ([`TodStats::dropped_grade_weight`]). A third LUT slot, or baking the mix on the GPU, lifts
//!   that.
//! - **Rotated boxes and other volume shapes**, and volumes as zone data (M3's zones and cells),
//!   with streaming adding and removing them.
//! - **Curves smoother than per-segment easing** (Catmull-Rom through the keys) if per-segment
//!   kinks show in time-lapses.
//! - **Live link** of timelines (A9) through [`TimeOfDay::set_timeline`] and
//!   [`TodTimeline::from_ron`], as desktop hot reload already reloads the default day in
//!   development builds (A8), and timelines and cutscenes loaded from packs (A4).
//! - **The editor's** curve and gradient editors over the keys (M2–M3).

mod clock;
mod cutscene;
mod time_of_day;
mod volume;

#[cfg(test)]
mod tests;

pub use clock::TodClock;
pub use cutscene::{ShotTime, TodCutscene};
// The model, whole: the world's time of day is the model plus its runtime.
pub use sapphire_tod::*;
pub use time_of_day::{ShotRig, TimeOfDay, TodFrame, TodStats, tick_time_of_day};
pub use volume::{TodVolume, TodVolumeId, VolumeShape};

/// The shipped default day's document (see [`default_day`]).
const DEFAULT_DAY: &str = include_str!("../default_day.ron");

/// Sapphire's default day (`default_day.ron` beside this crate's manifest), compiled in: eight
/// keys from midnight through a blue hour, sunrise, the day, golden hour, sunset and dusk. At
/// 10:00 it is exactly the renderer's default look (the sun the sandbox has always drawn with,
/// [`sapphire_render::SkyGradient::default`] and [`sapphire_render::PostParams::default`]), so a
/// scene that doesn't choose a time looks as it did before time of day, but for its fog (M1 W3):
/// a light haze the renderer's default leaves out.
///
/// Only the runtime compiles it in: the pipeline's probe bake reads the same file from the
/// repository it was built in, so editing the day bakes again without rebuilding the tool.
pub fn default_day() -> TodTimeline {
    TodTimeline::from_ron(DEFAULT_DAY).expect("the shipped default day is checked by this module's tests")
}
