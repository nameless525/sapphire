//! Time of day's model (M1 W1; look pillar 1, M1 plan §2): the art-directed keys of a day, the
//! timelines they make, and how they resolve and blend into the frame's look. One set of keys
//! drives the sky gradient and its layers (the sun's and moon's discs and glows, the stars, the
//! clouds, M1 W2; the nebula, M1 X1 part 2e), the sun and moon, the key light, ambient light, fog, the shadow tint, the
//! character rig's key light, the day/night ramp blend and post-processing's exposure, bloom,
//! grade and vignette, and its compositing layers (the diffusion glow, characters' bloom,
//! gradient layers and the lens flare; M1 P2), so characters and world change together and read
//! as one picture. As in Wuthering Waves, it is a stylized day, not a physical atmosphere.
//!
//! The looks are `sapphire_look`'s plain data, which the renderer draws (rule R4). The runtime,
//! the clock, zone volumes, cutscene tracks, the default day compiled in and resolving a frame
//! into the frame packet, is `sapphire_world::tod`'s, which re-exports all of this. Tools that
//! need a day's light without drawing it, such as the pipeline's probe bake, depend on this crate
//! alone, so an edit to a shader or to the default day's document rebuilds neither them nor this
//! crate (M1 A8's follow-up).
//!
//! # Data
//!
//! - **[`TodTimeline`]** (a `TodTimeline` RON document): [`TodKey`]s at hours of the day, each
//!   setting groups of values ([`TodGroup`]: sun, moon, sky, ambient, fog, shadow tint,
//!   character, day/night, exposure, grade, bloom, vignette, the sky's layers: discs, stars, clouds
//!   and the nebula, post's layers: diffusion, character bloom, gradients and lens flare, the world's
//!   emission scale and the characters' shadow hue). The base timeline sets them all but the sky's
//!   and post's layers, the emission scale and the shadow hue, which are optional
//!   ([`TodGroup::OPTIONAL`]): a timeline without them, as every timeline before them was, draws the
//!   sky's gradient alone, post as it was ([`sapphire_look::PostParams::plain`]), emission as
//!   authored and characters' shadow sides in the key light's color. A zone's override and a
//!   cutscene's track are keys too, setting every group or only some ([`check_keys`] checks
//!   either).
//! - **[`TodLook`]**: the look at one moment, every group resolved, as plain values.
//!
//! # Evaluation
//!
//! Between two keys, each group blends by the first key's [`TodEase`]: colors in OkLab
//! ([`sapphire_math::color`]), directions along the great circle at constant angular speed,
//! scalars linearly. Timelines wrap at midnight: the last key of the day blends into the first.
//! A time on a key, or an override at full weight, reproduces the key's values bit for bit
//! ([`mix`]). Grades are the exception: a grade is baked into a LUT, so neighboring keys' grades
//! blend as post's two LUTs ([`GradeMix`]) instead of field by field, and each key's grade is
//! baked once. Gradient layers (M1 P2) blend slot by slot: a layer with the same kind of shape
//! and blend mode as the one in its place blends field by field; one that differs fades out by
//! halfway and the other in after it, and a layer only one side has fades from or to no opacity
//! ([`GradientKey`]), so the picture never jumps.
//!
//! The key light the scene is shaded by is resolved from the sun and moon
//! ([`TodLook::key_light`]): whichever shines, and through twilight both, its direction turning
//! from one to the other with the moon's share of the light. A body gives light only from above
//! the horizon, its intensity fading out over its last degree, so the scene is never lit from
//! below whatever the keys say. The character key fades with it ([`TodLook::character_key`]):
//! characters keep the ratio to the world's light their keys set while a body sets or rises,
//! where keys, blending in straight lines, can't follow the fade.
//!
//! # The celestial sphere
//!
//! The stars and the Milky Way are fixed to a sphere that turns once a day about the sky's pole,
//! 35° up over the far horizon (−Z, the default day's north), so they rise on the left and set
//! on the right as the default day's sun does: [`celestial_sphere`] of the look's hour, which
//! cutscenes that set their own hour blend toward like any other value.
//!
//! # Does not own
//!
//! - The clock, zone volumes, cutscene tracks with their light rigs, the default day and
//!   resolving a frame: `sapphire_world::tod`.
//! - Drawing: the renderer reads only the resolved values the world writes into its frame packet.
//! - Loading timelines from packs (A4).

mod key;
mod look;
mod timeline;

pub use key::{
    AmbientKey, BloomKey, CelestialKey, CharacterBloomKey, CharacterKey, CharacterShadeKey, CloudsKey,
    DiffusionKey, DiscKey, DiscsKey, FogKey, FogLayerKey, GradeKey, GradientBlendKey, GradientKey,
    GradientShapeKey, HeightFogKey, LensFlareKey, NebulaKey, Rgb, SkyKey, TodEase, TodGroup, TodGroups,
    TodKey, VignetteKey,
};
pub use look::{GradeMix, Mix, TodLook, celestial_sphere, mix};
pub use timeline::{DAY_HOURS, KeyTimes, Segment, TodTimeline, check_keys, segment};

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
    #[error(
        "the base timeline must set every group but the optional ones (the sky's and post's layers and \
         the emission scale), and its keys have no `{0}`"
    )]
    Incomplete(TodGroup),
    #[error("cutscene: `{field}` {problem}")]
    Cutscene { field: &'static str, problem: &'static str },
    #[error("cutscene: `light_rig.{field}` {problem}")]
    CutsceneRig { field: &'static str, problem: &'static str },
}
