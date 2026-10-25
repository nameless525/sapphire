//! Time of day (M1 W1; look pillar 1, M1 plan §2): one set of art-directed keys drives the sky
//! gradient, the sun and moon, the key light, ambient light, fog, the shadow tint, the
//! character rig's key light, the day/night ramp blend and post-processing's exposure, bloom,
//! grade and vignette, so characters and world change together and read as one picture. As in
//! Wuthering Waves, it is a stylized day, not a physical atmosphere.
//!
//! # Data
//!
//! - **[`TodTimeline`]** (a `TodTimeline` RON document): [`TodKey`]s at hours of the day, each
//!   setting groups of values ([`TodGroup`]: sun, moon, sky, ambient, fog, shadow tint,
//!   character, day/night, exposure, grade, bloom, vignette). The base timeline sets them all;
//!   [`TodTimeline::default_day`] is the shipped one, compiled in until packs (A4) carry
//!   timelines as content.
//! - **[`TodVolume`]**: a zone's override, a timeline that sets every group or only some, blended
//!   over the base by its weight at a probe point (smoothly out over a blend distance around its
//!   sphere or box), lowest priority first.
//! - **[`TodCutscene`]** (a `TodCutscene` document): one shot's track, setting the time of day,
//!   overriding values over the shot's seconds, or both, easing in and out.
//!
//! # Evaluation
//!
//! Between two keys, each group blends by the first key's [`TodEase`]: colors in OkLab
//! ([`sapphire_math::color`]), directions along the great circle at constant angular speed,
//! scalars linearly. Timelines wrap at midnight: the last key of the day blends into the first.
//! A time on a key, or a volume at full weight, reproduces the key's values bit for bit. Grades
//! are the exception: a grade is baked into a LUT, so neighboring keys' grades blend as post's
//! two LUTs ([`GradeMix`]) instead of field by field, and each key's grade is baked once.
//!
//! The key light the scene is shaded by is resolved from the sun and moon
//! ([`TodLook::key_light`]): whichever shines, and through twilight both, its direction turning
//! from one to the other with the moon's share of the light. A body gives light only from above
//! the horizon, its intensity fading out over its last degree, so the scene is never lit from
//! below whatever the keys say.
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
//! those plain values (rule R4).
//!
//! Resolving allocates nothing and costs under a microsecond of CPU on an Apple M5: about 0.1 µs
//! while the hour stands still and 0.55 µs while it runs (the budget is 0.05 ms);
//! [`TodStats`] reports its time, the segment, active volumes, the cutscene weight and any grade
//! weight post's two LUTs couldn't show (rule R9).
//!
//! # Consumers
//!
//! The packet's sky, key light and post-processing are drawn today. The rest is carried for the
//! packages that read it: the sun and moon discs (W2), fog (W3), ambient (W4, W5), the shadow
//! tint (R4, W4), the character key light (C2) and the day/night factor (C1's ramp rows, in the
//! frame uniforms as `day_night`).
//!
//! # Later milestones
//!
//! - **Weather** as partial overrides between zones and cutscenes (the research notes' weather
//!   layer), and scheduled switches with a fade (local lights on at night) rather than blends.
//! - **More LUTs**: a zone that changes the grade while time crosses between keys of different
//!   grades can need three grades at once; post's two LUTs drop the lightest
//!   ([`TodStats::dropped_grade_weight`]). A third LUT slot, or baking the mix on the GPU, lifts
//!   that.
//! - **Rotated boxes and other volume shapes**, and volumes as zone data (M3's zones and cells),
//!   with streaming adding and removing them.
//! - **Curves smoother than per-segment easing** (Catmull-Rom through the keys) if per-segment
//!   kinks show in time-lapses.
//! - **Hot reload and live link** of timelines (A8, A9) through [`TimeOfDay::set_timeline`] and
//!   [`TodTimeline::from_ron`], and timelines and cutscenes loaded from packs (A4).
//! - **The editor's** curve and gradient editors over the keys (M2–M3).

mod clock;
mod cutscene;
mod key;
mod look;
mod time_of_day;
mod timeline;
mod volume;

#[cfg(test)]
mod tests;

pub use clock::TodClock;
pub use cutscene::{ShotTime, TodCutscene};
pub use key::{
    AmbientKey, BloomKey, CelestialKey, CharacterKey, FogKey, FogLayerKey, GradeKey, HeightFogKey, Rgb,
    SkyKey, TodEase, TodGroup, TodGroups, TodKey, VignetteKey,
};
pub use look::{GradeMix, TodLook};
pub use time_of_day::{TimeOfDay, TodFrame, TodStats, tick_time_of_day};
pub use timeline::{DAY_HOURS, Segment, TodTimeline};
pub use volume::{TodVolume, TodVolumeId, VolumeShape};

/// A timeline or cutscene that isn't valid, or a timeline that can't be the base.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TodError {
    #[error("there are no keys")]
    NoKeys,
    #[error("key {key}: time {time} is outside {expected}")]
    TimeOutOfRange { key: usize, time: f32, expected: &'static str },
    #[error("key {key}: times must increase from one key to the next")]
    Unordered { key: usize },
    #[error("key {key} sets {found} but the first key sets {expected}: every key must set the same groups")]
    MixedGroups { key: usize, found: TodGroups, expected: TodGroups },
    #[error("key {key}: `{field}` {problem}")]
    Invalid { key: usize, field: &'static str, problem: &'static str },
    #[error("the base timeline must set every group, and its keys have no `{0}`")]
    Incomplete(TodGroup),
    #[error("cutscene: `{field}` {problem}")]
    Cutscene { field: &'static str, problem: &'static str },
}
