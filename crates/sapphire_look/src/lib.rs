//! The frame's look as plain data (rule R4, spec §4.6): the scene constants and post-processing
//! values time of day resolves and the renderer draws, and the CPU models of how the renderer
//! draws them, for tests and tools.
//!
//! This crate sits beneath both. `sapphire_render` carries these values in its frame packet and
//! re-exports every one of them, so renderer code and its users name them as
//! `sapphire_render::SkyGradient` and so on; time of day (`sapphire_tod`) resolves its keys into
//! them. Tools that light or preview a look without drawing it, such as the pipeline's probe
//! bake, which lights its time-of-day keys as the renderer relights the sky, depend on this crate
//! and `sapphire_tod` rather than on the renderer and the world, so an edit to a shader or to the
//! default day rebuilds neither them nor the tools (M1 A8's follow-up). Nothing here embeds a
//! file.
//!
//! # Owns
//!
//! - **Light** ([`light`]): the key light and the bodies that give it ([`DirectionalLight`],
//!   [`Celestial`]), the ambient hemisphere ([`AmbientLight`]), time of day's curve for the
//!   characters' key ([`CharacterLight`]) and a character's light rig as the world resolves it
//!   for a frame ([`CharacterLighting`], [`CharacterGi`], [`CharacterShade`]).
//! - **The sky** ([`sky`]): its gradient and the layers drawn over it ([`SkyGradient`],
//!   [`SkyLayers`], [`SkyDisc`], [`Nebula`], [`CloudLight`]), and its light as the probe grid
//!   takes it ([`sky::sky_radiance`]).
//! - **Fog** ([`fog`]): its layers ([`Fog`]) and the CPU reference of the shaders' fog over a view
//!   ray ([`Fog::ray`]), with the constants the shaders share.
//! - **Post-processing's input** ([`post`]): exposure, bloom, the tone curve and grades, the
//!   vignette, the compositing layers and combat's screen effects ([`PostParams`]).
//!
//! # Does not own
//!
//! - The frame packet itself and the rest of what it carries (views, instance deltas, poses,
//!   local lights, characters' faces and bodies, weather, wind, the probe keys): the renderer's,
//!   beside the code that reads them.
//! - Drawing: the renderer's passes and WGSL. The CPU models here mirror its shaders, and the
//!   renderer's tests hold the two together.
//! - Where the values come from: time of day's keys (`sapphire_tod`), the world's light rigs and
//!   screen effects (`sapphire_world`).
//!
//! # Later milestones
//!
//! - **Weather and wind** join the look here once the world resolves them per zone and hour (M2);
//!   until then they are the renderer's plain packet data, like the rest of the packet.

pub mod fog;
pub mod light;
pub mod post;
pub mod sky;

pub use fog::{Fog, FogLayer, FogRay, HeightFog};
pub use light::{
    AmbientLight, Celestial, CharacterGi, CharacterLight, CharacterLighting, CharacterShade, DirectionalLight,
};
pub use post::{
    BloomParams, CharacterBloom, ChromaticAberration, ColorGrade, Diffusion, Duotone, EffectCoverage,
    GradientBlend, GradientLayer, GradientShape, Inversion, LensFlare, MAX_GRADIENT_LAYERS, PostEffects,
    PostParams, RadialBlur, SpeedLines, ToneCurve, Vignette,
};
pub use sky::{CloudLight, Nebula, SkyDisc, SkyGradient, SkyLayers};
