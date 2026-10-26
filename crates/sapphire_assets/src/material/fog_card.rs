//! The fog card family: camera-faded, depth-soft translucent sheets artists place for valley
//! mist, beams in rooms and street haze (world_lighting_post.md §6), drawn on meshes by the
//! renderer's fog card shader (M1 W3, `sapphire_render::fog`).

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_reflect::Versioned;
use sapphire_serialize::ReflectDocument;
use serde::{Deserialize, Serialize};

use super::common::{bound_bits, color, finite, gpu_block, invalid, non_negative, pack_swizzles, unit};
use super::{MaterialDocument, MaterialError, MaterialFamily, MaterialFlags, TextureSlot, TextureUsage};
use crate::AssetId;
use crate::texture::Swizzle;

/// A fog card material, as a document (`sapphire::render::FogCardMaterial`, version 1).
///
/// A card is lit by the frame's fog: its color multiplies the light the fog has at the card's
/// distance (its layers' colors, the sky's horizon where there is none), which time of day
/// drives, so a card placed at noon still reads right at dusk. The card is then fogged like any
/// surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[reflect(Document)]
#[serde(default)]
pub struct FogCardMaterial {
    /// Linear RGB tint and A opacity.
    pub color: [f32; 4],
    /// The card's shape and noise, linear: R its density in the card's UVs (its shape, painted
    /// soft at the edges so the card never shows them), G a tiling octave read where it drifts
    /// by `scroll`, multiplying it (white for none). Unbound, the card is evenly dense.
    pub map: Option<AssetId>,
    /// The G octave's drift, UV per second, so mist moves through the card's shape. The renderer
    /// rounds it to a whole number of repeats an hour (under 1/7200 UV per second), so it never
    /// jumps when the shaders' clock wraps.
    pub scroll: [f32; 2],
    /// Meters over which the card fades where it meets geometry (a soft particle).
    pub depth_fade: f32,
    /// Distances from the camera, in meters, over which the card fades in: invisible at
    /// `[0]`, full at `[1]`, so walking through it never shows its edge.
    pub camera_fade: [f32; 2],
    /// Brightening toward the sun, as forward scattering through mist, 0 to 1: the share of the
    /// fog's sun inscatter the card adds.
    pub sun_scatter: f32,
    /// Which way the card faces.
    pub facing: CardFacing,
}

/// Which way a fog card faces ([`FogCardMaterial::facing`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum CardFacing {
    /// As placed: the mesh in world space (sheets across a valley, layers over water).
    #[default]
    World,
    /// Turned whole toward the camera around its instance's origin, its object-space XY plane
    /// the screen's (puffs, haze): one card per instance, centered on its origin.
    Camera,
    /// Turned about the world's up axis toward the camera, staying upright (mist columns, haze
    /// along a street): likewise one card per instance.
    Upright,
}

impl CardFacing {
    /// Its code in the GPU block (`FogCardParams::modes[1]`).
    pub const fn code(self) -> u32 {
        match self {
            Self::World => 0,
            Self::Camera => 1,
            Self::Upright => 2,
        }
    }
}

impl Versioned for FogCardMaterial {
    const VERSION: u32 = 1;
}

impl Default for FogCardMaterial {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 0.5],
            map: None,
            scroll: [0.0; 2],
            depth_fade: 1.0,
            camera_fade: [0.5, 3.0],
            sun_scatter: 0.3,
            facing: CardFacing::World,
        }
    }
}

/// The fog card family's GPU block (WGSL `FogCardParams`). 80 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct FogCardParams {
    /// Linear RGB tint, A opacity.
    pub color: [f32; 4],
    /// x: depth fade; y, z: camera fade start and end; w: sun scatter.
    pub fade: [f32; 4],
    /// xy: scroll; zw: 0.
    pub scroll: [f32; 4],
    /// x: flags (bit `i` for texture slot `i`); y: the facing ([`CardFacing::code`]).
    pub modes: [u32; 4],
    /// Each texture slot's swizzle code, two to a word.
    pub swizzles: [u32; 4],
}

gpu_block!(FogCardParams { color, fade, scroll });

impl FogCardMaterial {
    pub const MAP: usize = 0;

    /// The family's texture slots, in binding order.
    pub const TEXTURE_SLOTS: &[TextureSlot] = &[TextureSlot { name: "map", usage: TextureUsage::Mask }];

    /// The GPU block, given the swizzle of each slot's texture.
    pub fn gpu_params(&self, swizzles: &[Swizzle]) -> FogCardParams {
        FogCardParams {
            color: self.color,
            fade: [self.depth_fade, self.camera_fade[0], self.camera_fade[1], self.sun_scatter],
            scroll: [self.scroll[0], self.scroll[1], 0.0, 0.0],
            modes: [bound_bits(&self.textures()), self.facing.code(), 0, 0],
            swizzles: pack_swizzles(swizzles),
        }
    }
}

impl MaterialDocument for FogCardMaterial {
    const FAMILY: MaterialFamily = MaterialFamily::FogCard;

    fn validate(&self) -> Result<(), MaterialError> {
        color("color", &self.color)?;
        unit("color[3]", self.color[3])?;
        finite("scroll[0]", self.scroll[0])?;
        finite("scroll[1]", self.scroll[1])?;
        non_negative("depth_fade", self.depth_fade)?;
        let [start, end] = self.camera_fade;
        non_negative("camera_fade[0]", start)?;
        finite("camera_fade[1]", end)?;
        if end < start {
            return Err(invalid("camera_fade", "must be ordered near to far"));
        }
        unit("sun_scatter", self.sun_scatter)
    }

    fn textures(&self) -> Vec<Option<AssetId>> {
        vec![self.map]
    }

    fn flags(&self) -> MaterialFlags {
        MaterialFlags::TRANSPARENT
    }

    fn params(&self, swizzles: &[Swizzle]) -> Vec<u8> {
        bytemuck::bytes_of(&self.gpu_params(swizzles)).to_vec()
    }
}
