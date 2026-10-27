//! Pieces every family shares: alpha modes, distance stylization, validation and the packing of
//! texture swizzles into GPU words.

use bevy_reflect::Reflect;
use bytemuck::Pod;
use serde::{Deserialize, Serialize};

use super::MaterialError;
use crate::texture::{Component, Swizzle};

/// How a material's alpha (base color alpha × its map's) is used. The renderer's alpha-test
/// and transparent permutations follow it (spec §5.12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum AlphaMode {
    /// Alpha is ignored.
    #[default]
    Opaque,
    /// Pixels with alpha below `cutoff` are discarded: hair cards, lashes, leaf cards.
    Mask { cutoff: f32 },
    /// Alpha blends over what is behind: glass, visors, fog.
    Blend,
}

impl AlphaMode {
    /// The alpha-test cutoff the GPU block carries: 0 when nothing is discarded.
    pub(crate) fn cutoff(self) -> f32 {
        match self {
            Self::Mask { cutoff } => cutoff,
            Self::Opaque | Self::Blend => 0.0,
        }
    }

    pub(crate) fn validate(self, field: &str) -> Result<(), MaterialError> {
        match self {
            Self::Mask { cutoff } => unit(&format!("{field}.cutoff"), cutoff),
            Self::Opaque | Self::Blend => Ok(()),
        }
    }
}

/// Stylization that grows with view distance, so far surfaces read painted rather than detailed:
/// colors desaturate and drift toward the fog and sky (world_lighting_post.md §2).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct DistanceStylization {
    /// Meters from the camera where it starts.
    pub start: f32,
    /// Meters where it reaches full strength; at least `start`.
    pub end: f32,
    /// How much color is lost at full strength, 0 to 1.
    pub desaturation: f32,
}

impl Default for DistanceStylization {
    fn default() -> Self {
        Self { start: 60.0, end: 400.0, desaturation: 0.0 }
    }
}

impl DistanceStylization {
    pub(crate) fn validate(&self, field: &str) -> Result<(), MaterialError> {
        non_negative(&format!("{field}.start"), self.start)?;
        finite(&format!("{field}.end"), self.end)?;
        if self.end < self.start {
            return Err(invalid(&format!("{field}.end"), "must be at least `start`"));
        }
        unit(&format!("{field}.desaturation"), self.desaturation)
    }

    pub(crate) fn gpu(&self) -> [f32; 4] {
        [self.start, self.end, self.desaturation, 0.0]
    }
}

pub(crate) fn invalid(field: &str, reason: &'static str) -> MaterialError {
    MaterialError::Invalid { field: field.to_owned(), reason }
}

pub(crate) fn finite(field: &str, value: f32) -> Result<(), MaterialError> {
    if value.is_finite() { Ok(()) } else { Err(invalid(field, "must be finite")) }
}

pub(crate) fn non_negative(field: &str, value: f32) -> Result<(), MaterialError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(invalid(field, "must be finite and at least 0"))
    }
}

pub(crate) fn unit(field: &str, value: f32) -> Result<(), MaterialError> {
    if (0.0..=1.0).contains(&value) { Ok(()) } else { Err(invalid(field, "must be between 0 and 1")) }
}

/// Colors are linear light, so any finite non-negative value is one (above 1 is HDR).
pub(crate) fn color<const N: usize>(field: &str, value: &[f32; N]) -> Result<(), MaterialError> {
    if value.iter().all(|c| c.is_finite() && *c >= 0.0) {
        Ok(())
    } else {
        Err(invalid(field, "must be finite and at least 0 in every channel"))
    }
}

/// `rgb` with `w` as a fourth component.
pub(crate) fn rgb_w(rgb: [f32; 3], w: f32) -> [f32; 4] {
    [rgb[0], rgb[1], rgb[2], w]
}

/// The period, in seconds, of the clock shaders read: the renderer wraps the frame's
/// presentation time to it ([`shader_time`]), since an hour keeps `f32` precision to a fraction of
/// a millisecond. Anything periodic on that clock must complete whole cycles in it, or it jumps at
/// every wrap, so materials pack their rates as whole cycles an hour ([`whole_cycles`]).
pub const SHADER_TIME_PERIOD: f64 = 3600.0;

/// `seconds` of presentation time as shaders read it: wrapped into `0..SHADER_TIME_PERIOD` in
/// `f64` before it becomes an `f32`, and 0 for a time that isn't finite.
pub fn shader_time(seconds: f64) -> f32 {
    if seconds.is_finite() { seconds.rem_euclid(SHADER_TIME_PERIOD) as f32 } else { 0.0 }
}

/// `rate`, in cycles a second, as shaders run it (review RS-003): rounded to the nearest whole
/// multiple of `step` cycles per [`SHADER_TIME_PERIOD`], so an oscillator at it, and at the rate
/// times any ratio whose denominator divides `step`, completes whole cycles every time the
/// shaders' clock wraps and never jumps there. That moves a rate by at most `step / 7200` Hz, and a
/// rate under half a step an hour runs at 0.
pub fn whole_cycles(rate: f64, step: u32) -> f32 {
    let step = f64::from(step);
    ((rate * SHADER_TIME_PERIOD / step).round() * step / SHADER_TIME_PERIOD) as f32
}

/// Bits per swizzled component in a GPU swizzle code.
const COMPONENT_BITS: u32 = 3;
/// Bits per texture slot: four components.
pub const SWIZZLE_BITS: u32 = 4 * COMPONENT_BITS;

/// The GPU form of `swizzle`: per logical component, in RGBA order, 3 bits naming its source
/// (0–3 the sampled R, G, B or A, 4 zero, 5 one). The identity is `0x688`. Shaders decode it
/// with `material_swizzle` (`shaders/materials.wgsl`).
pub fn swizzle_code(swizzle: Swizzle) -> u32 {
    swizzle.0.iter().enumerate().fold(0, |code, (index, component)| {
        let source = match component {
            Component::R => 0,
            Component::G => 1,
            Component::B => 2,
            Component::A => 3,
            Component::Zero => 4,
            Component::One => 5,
        };
        code | source << (COMPONENT_BITS * index as u32)
    })
}

/// Every slot's [`swizzle_code`], two slots to a word (slot `i` in word `i / 2`, from bit
/// `12 × (i % 2)`), as the family blocks' `swizzles` field holds them. Up to 8 slots.
pub(crate) fn pack_swizzles(swizzles: &[Swizzle]) -> [u32; 4] {
    debug_assert!(swizzles.len() <= 8, "families have at most 8 texture slots");
    let mut words = [0; 4];
    for (slot, &swizzle) in swizzles.iter().enumerate() {
        words[slot / 2] |= swizzle_code(swizzle) << (SWIZZLE_BITS * (slot % 2) as u32);
    }
    words
}

/// The low bits of a block's `modes.x`: bit `i` set when texture slot `i` binds a texture.
pub(crate) fn bound_bits(textures: &[Option<crate::AssetId>]) -> u32 {
    textures
        .iter()
        .enumerate()
        .filter(|(_, texture)| texture.is_some())
        .fold(0, |bits, (slot, _)| bits | 1 << slot)
}

/// What validating a cooked material reads from its family's GPU block.
pub(crate) trait GpuBlock: Pod {
    /// Every float vector of the block.
    fn vectors(&self) -> Vec<[f32; 4]>;
    /// x: the bound-slot bits, then the family's flags.
    fn modes(&self) -> [u32; 4];
    /// Each slot's swizzle code, two to a word.
    fn swizzles(&self) -> [u32; 4];
}

/// Implements [`GpuBlock`] for a block with `modes` and `swizzles` fields and the listed float
/// vectors.
macro_rules! gpu_block {
    ($block:ty { $($vector:ident),+ $(,)? }) => {
        impl $crate::material::common::GpuBlock for $block {
            fn vectors(&self) -> Vec<[f32; 4]> {
                vec![$(self.$vector),+]
            }

            fn modes(&self) -> [u32; 4] {
                self.modes
            }

            fn swizzles(&self) -> [u32; 4] {
                self.swizzles
            }
        }
    };
}

pub(crate) use gpu_block;
