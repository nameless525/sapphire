//! Levels of detail for static draws (M5 D4, spec §5.12 "Scene and draws") and characters (M5 D5):
//! which of its mesh's cooked levels each view draws an instance at.
//!
//! The cook writes up to four levels a mesh (spec §5.27), each a prefix of the vertex streams with
//! its own index ranges and its error: how far it may stray from LOD0, in object-space meters,
//! folding the shading normals' error in with the geometry's (`sapphire_assets::mesh`, "LODs"). The
//! mesh pool holds every level's ranges, and a view draws one level of an instance:
//!
//! - **The main view's family** (the scene prepass, the main pass, the outline hulls, the debug
//!   views' replays) draws one and the same level of an instance in a frame, chosen with its
//!   culling verdict (the `view_cull` module), so the prepass's depth and the main pass's agree. A
//!   level is good enough when its error, times the instance's largest stretch, projected at the
//!   nearest point of its bounds, covers at most the frame's allowance in **output** pixels: one
//!   pixel × 2^bias ([`allowance`]). The coarsest good enough draws. Output pixels, not the scene's,
//!   so dynamic resolution and a phone's upscale never move a switch (`CullView` measures by the
//!   target's rows).
//! - **Hysteresis** ([`choose`]): an instance the main view drew last frame goes coarser only when
//!   the coarser level is good enough within the allowance over [`COARSER_MARGIN`], and comes back
//!   finer once its level stops being good enough within the allowance itself: a band of
//!   [`COARSER_MARGIN`] in distance, as culling's size test holds its own (`view_cull::SIZE_RETURN`).
//!   One the main view didn't draw last frame, or any after a camera cut (a new shot, a jump, a
//!   zone trip's arrival, a tier change), chooses afresh: a level held across a cut is wrong.
//! - **Each cascade** chooses by its own texel ([`cascade_level`]), with no memory: the coarsest
//!   level whose error, times the stretch, is at most [`CASCADE_TEXELS`] of a texel × 2^bias. That
//!   depends only on the cascade's fit, the mesh and the instance's transform, so a cascade left
//!   cached holds the levels it was drawn with, which are the ones a redraw at the same fit would
//!   choose. A quarter of a texel is the receivers' own depth bias (`SHADOW_DEPTH_BIAS` in
//!   frame.wgsl, which the normal offset's half texel adds to), so a coarser caster moves its
//!   surface toward its receiver by less than the bias already allows for: no acne, and no
//!   shadow peeling off a foot.
//! - **The bias** is the tier's ([`LevelSettings::bias`], `tiers.ron`'s `lod_bias`), plus the
//!   thermal ladder's [`Rung::LodBias`](crate::quality::Rung::LodBias), which builds nothing. The
//!   frames apply a rung's bias at most [`RUNG_BIAS_STEP`] (a 32nd of a doubling) at a time, either
//!   way ([`RungBias`]): taken at once, a doubled allowance would switch every instance whose next
//!   switch lies within twice its distance on one frame, about half of those with levels, and redraw
//!   every held cascade whose casters' levels moved. Ramped, each frame switches what a 2% step of
//!   the camera would, through the same hysteresis. A tier change drops it at once, as a cut.
//! - **Characters** (M5 D5) choose only where the stage says so
//!   ([`CharacterDetail::ByError`](crate::CharacterDetail::ByError) on the character's body: a
//!   fight's enemies while nothing pins them, a zone's ambient characters, the sandbox's
//!   `--character-lods hero`). Such a character chooses one level a frame for all its parts, by the
//!   sphere around them, as a static draw chooses (the same allowance and hysteresis): the
//!   coarsest every part with levels is good enough at, a part at its own coarsest holding it back
//!   no further; each part then draws the coarsest of its own levels within that
//!   (`view_cull::part_level`). Its character map chooses by the map's texel as a cascade does.
//!   Skinning skins the prefix of the finest level any view drew (`DrawnBy::finest`), and its
//!   outline hull draws at its body's level, out to the material's unchanged cutoff.
//! - **Level 0 always:** a character the stage doesn't mark (the hero, its on-field party, what
//!   they hold, everyone in a cutscene's or a cut-in's shot), anything skinned or morphed that isn't
//!   a character, everything in an inset, fog cards, foliage (its cooked error measures the cards'
//!   geometry, not the canopy's coverage, so a coarser canopy would thin), and what isn't an
//!   instance (the sky, effects, decals, the interface).
//!
//! A switch pops: there is no crossfade. [`LevelsMode::Off`] draws every instance at level 0
//! through the same path, the pictures of the renderer before levels, and [`LevelsMode::Fixed`]
//! holds every instance that may change level at one level, for comparing a switch's two sides.

use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// The allowance's base: a level is good enough when its error covers at most this many output
/// pixels × 2^bias.
pub(crate) const ALLOWANCE_PIXELS: f32 = 1.0;

/// An instance goes coarser only when the coarser level is good enough within the allowance over
/// this: the hysteresis band, the same ratio as culling's size test (`view_cull::SIZE_RETURN`).
pub(crate) const COARSER_MARGIN: f32 = 1.25;

/// The share of a cascade's texel a caster's level may stray by: the receivers' depth bias.
pub(crate) const CASCADE_TEXELS: f32 = 0.25;

/// The most levels a mesh's draws choose among: a draw command says which levels its submesh has
/// triangles at in a byte (`DrawCommand::levels`). The cook writes four.
pub(crate) const MAX_LEVELS: usize = 8;

/// Whether and how instances choose their levels (M5 D4,
/// [`RenderSettings::levels`](crate::RenderSettings::levels)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum LevelsMode {
    /// Each view chooses (see the module docs).
    #[default]
    On,
    /// Every instance at level 0, as before levels were chosen: what older scenes pin.
    Off,
    /// Every instance that may change level at this level, or its mesh's coarsest if it has fewer:
    /// the two sides of a switch, for comparing them.
    Fixed(u8),
}

impl LevelsMode {
    /// The coarsest level a draw command can say it has triangles at, which
    /// [`LevelsMode::Fixed`] may name.
    pub const MAX_LEVEL: u8 = (MAX_LEVELS - 1) as u8;
}

/// A renderer's levels of detail: the mode and the tier's bias (`tiers.ron`'s `lod_bias`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct LevelSettings {
    pub mode: LevelsMode,
    /// The allowance's exponent: a level is good enough within 2^bias output pixels, and a
    /// cascade's within a quarter of its texel × 2^bias.
    pub bias: f32,
}

impl LevelSettings {
    /// Levels chosen, with no bias: [`RenderSettings`](crate::RenderSettings)'s default.
    pub const ON: Self = Self { mode: LevelsMode::On, bias: 0.0 };

    /// The most a bias may be, either way: 16 pixels' allowance at most.
    pub const MAX_BIAS: f32 = 4.0;

    /// Checks the bias is finite and within ±[`Self::MAX_BIAS`], returning the field and what is
    /// wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if !(self.bias.is_finite() && self.bias.abs() <= Self::MAX_BIAS) {
            return Err(("levels.bias", "must be within -4..=4"));
        }
        Ok(())
    }

    /// The level every instance that may change level draws at whatever its distance, if the mode
    /// forces one: for a mesh of `levels` levels.
    pub(crate) fn forced(&self, levels: usize) -> Option<u8> {
        match self.mode {
            LevelsMode::On => None,
            LevelsMode::Off => Some(0),
            LevelsMode::Fixed(level) => Some(level.min(last_level(levels))),
        }
    }
}

impl Default for LevelSettings {
    fn default() -> Self {
        Self::ON
    }
}

/// Settings compare whole (`RenderSettings` is `Eq`): the bias is finite where it's used
/// ([`LevelSettings::validate`]), where `==` is an equivalence.
impl Eq for LevelSettings {}

/// Levels of detail's counters from the last recorded frame (rule R9): the main view's switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LevelStats {
    /// Instances the main view drew last frame and draws at a coarser level this frame.
    pub coarser: u32,
    /// Instances the main view drew last frame and draws at a finer level this frame.
    pub finer: u32,
}

impl LevelStats {
    /// Every switch.
    pub fn switches(&self) -> u32 {
        self.coarser + self.finer
    }
}

/// The most the rung's applied bias moves in a frame: a 32nd of a doubling, so each frame's switches
/// are those of a step of the camera 2.2% away or nearer.
pub(crate) const RUNG_BIAS_STEP: f32 = 1.0 / 32.0;

/// The thermal ladder's [`Rung::LodBias`](crate::quality::Rung::LodBias) as the frames apply it (M5
/// Z3g): the rung's target, and the bias applied, which moves toward it by at most
/// [`RUNG_BIAS_STEP`] a frame, either way (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RungBias {
    target: f32,
    applied: f32,
}

impl RungBias {
    /// The rung's bias from now on, which the frames reach a step at a time.
    pub(crate) fn set(&mut self, target: f32) {
        self.target = target;
    }

    /// The bias this frame applies.
    pub(crate) fn applied(&self) -> f32 {
        self.applied
    }

    /// Starts a frame: the applied bias one step nearer the target, or on it once within a step.
    pub(crate) fn step(&mut self) {
        let left = self.target - self.applied;
        self.applied = if left.abs() <= RUNG_BIAS_STEP {
            self.target
        } else {
            self.applied + RUNG_BIAS_STEP.copysign(left)
        };
    }
}

/// The output pixels a level's error may cover at `bias`: [`ALLOWANCE_PIXELS`] × 2^bias.
pub(crate) fn allowance(bias: f32) -> f32 {
    ALLOWANCE_PIXELS * bias.exp2()
}

/// The coarsest level that `levels` may draw.
pub(crate) fn last_level(levels: usize) -> u8 {
    u8::try_from(levels.clamp(1, MAX_LEVELS) - 1).expect("at most MAX_LEVELS")
}

/// The coarsest of `errors`' levels (object-space meters, LOD0 first) whose error is at most
/// `budget` meters; 0 if none is. A budget that isn't a number takes none, an error that isn't
/// finite is never within one.
pub(crate) fn coarsest_within(errors: &[f32], budget: f32) -> u8 {
    let levels = &errors[..errors.len().min(MAX_LEVELS)];
    levels.iter().rposition(|&error| error <= budget).map_or(0, |level| level as u8)
}

/// The main view's level of a mesh whose levels' errors are `errors`, where `budget` is the most
/// an error may be in object-space meters to be good enough (the allowance in output pixels, over
/// the output pixels a meter of the instance covers at its nearest point), and `held` the level it
/// drew at last frame, if the main view drew it (see the module docs' hysteresis).
pub(crate) fn choose(errors: &[f32], budget: f32, held: Option<u8>) -> u8 {
    let fresh = coarsest_within(errors, budget);
    let Some(held) = held.map(|held| held.min(last_level(errors.len()))) else { return fresh };
    if errors.get(usize::from(held)).is_some_and(|&error| error <= budget) {
        // Still good enough: coarser only within the margin.
        coarsest_within(errors, budget / COARSER_MARGIN).max(held)
    } else {
        // No longer: the coarsest that is, which is finer.
        fresh
    }
}

/// A cascade's level of a mesh whose levels' errors are `errors` (object-space meters), for an
/// instance stretched at most `stretch` times, in a cascade whose texels are `texel` meters across,
/// at `bias`.
pub(crate) fn cascade_level(errors: &[f32], stretch: f32, texel: f32, bias: f32) -> u8 {
    coarsest_within(errors, CASCADE_TEXELS * texel * bias.exp2() / stretch)
}

#[cfg(test)]
mod tests;
