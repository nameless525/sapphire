//! The sky family: a zone's painted sky (spec §5.12 "Sky", world_lighting_post.md §5). Its
//! shader is W2's.
//!
//! Time of day owns the sky's colors (zenith, horizon, ground, the sun halo, cloud tints, star
//! intensity: `TodKey`, W1); the material owns what stays fixed through the day: the textures
//! and the shapes and motion of discs and clouds.

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_reflect::Versioned;
use sapphire_serialize::ReflectDocument;
use serde::{Deserialize, Serialize};

use super::common::{bound_bits, finite, gpu_block, non_negative, pack_swizzles, unit};
use super::{MaterialDocument, MaterialError, MaterialFamily, MaterialFlags, TextureSlot, TextureUsage};
use crate::AssetId;
use crate::texture::Swizzle;

/// A sky material, as a document (`sapphire::render::SkyMaterial`, version 1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[reflect(Document)]
#[serde(default)]
pub struct SkyMaterial {
    pub sun: Disc,
    pub moon: Moon,
    pub stars: Stars,
    /// Card clouds: linear RGBA, R lit amount, G rim, B dissolve threshold, A alpha (the Genshin
    /// scheme).
    pub clouds: CloudLayer,
    /// A high scrolling layer (cirrus): linear RGBA as `clouds`.
    pub cirrus: CloudLayer,
}

impl Versioned for SkyMaterial {
    const VERSION: u32 = 1;
}

/// A disc in the sky, drawn in HDR so it blooms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Disc {
    /// Angular diameter in degrees.
    pub size: f32,
    /// Width of the edge, as a fraction of the size.
    pub softness: f32,
}

impl Default for Disc {
    fn default() -> Self {
        Self { size: 1.5, softness: 0.1 }
    }
}

/// The moon.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Moon {
    pub disc: Disc,
    /// The moon's face, sRGB color with alpha; without one the disc is flat.
    pub map: Option<AssetId>,
}

impl Default for Moon {
    fn default() -> Self {
        Self { disc: Disc { size: 2.0, softness: 0.05 }, map: None }
    }
}

/// The night sky, rotating with sidereal time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Stars {
    /// A star field, color or HDR, mapped onto the sky dome.
    pub map: Option<AssetId>,
    /// The Milky Way band, color or HDR.
    pub milky_way: Option<AssetId>,
    /// How much stars twinkle, 0 to 1.
    pub twinkle: f32,
}

impl Default for Stars {
    fn default() -> Self {
        Self { map: None, milky_way: None, twinkle: 0.3 }
    }
}

/// A cloud layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct CloudLayer {
    pub map: Option<AssetId>,
    /// Texture repeats across the dome.
    pub scale: f32,
    /// UV scroll per second, along the wind.
    pub scroll: [f32; 2],
    /// Width of the dissolve edge as time of day moves coverage.
    pub dissolve_softness: f32,
}

impl Default for CloudLayer {
    fn default() -> Self {
        Self { map: None, scale: 1.0, scroll: [0.002, 0.0], dissolve_softness: 0.1 }
    }
}

/// The sky family's GPU block (WGSL `SkyParams`). 96 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct SkyParams {
    /// x: sun size (radians); y: sun softness; z: moon size (radians); w: moon softness.
    pub discs: [f32; 4],
    /// x: star twinkle; yzw: 0.
    pub stars: [f32; 4],
    /// x: scale; y: dissolve softness; zw: scroll.
    pub clouds: [f32; 4],
    /// x: scale; y: dissolve softness; zw: scroll.
    pub cirrus: [f32; 4],
    /// x: flags (bit `i` for texture slot `i`).
    pub modes: [u32; 4],
    /// Each texture slot's swizzle code, two to a word.
    pub swizzles: [u32; 4],
}

gpu_block!(SkyParams { discs, stars, clouds, cirrus });

impl SkyMaterial {
    pub const MOON: usize = 0;
    pub const STARS: usize = 1;
    pub const MILKY_WAY: usize = 2;
    pub const CLOUDS: usize = 3;
    pub const CIRRUS: usize = 4;

    /// The family's texture slots, in binding order.
    pub const TEXTURE_SLOTS: &[TextureSlot] = &[
        TextureSlot { name: "moon", usage: TextureUsage::Color },
        TextureSlot { name: "stars", usage: TextureUsage::Light },
        TextureSlot { name: "milky_way", usage: TextureUsage::Light },
        TextureSlot { name: "clouds", usage: TextureUsage::Mask },
        TextureSlot { name: "cirrus", usage: TextureUsage::Mask },
    ];

    /// The GPU block, given the swizzle of each slot's texture.
    pub fn gpu_params(&self, swizzles: &[Swizzle]) -> SkyParams {
        let layer =
            |layer: &CloudLayer| [layer.scale, layer.dissolve_softness, layer.scroll[0], layer.scroll[1]];
        SkyParams {
            discs: [
                self.sun.size.to_radians(),
                self.sun.softness,
                self.moon.disc.size.to_radians(),
                self.moon.disc.softness,
            ],
            stars: [self.stars.twinkle, 0.0, 0.0, 0.0],
            clouds: layer(&self.clouds),
            cirrus: layer(&self.cirrus),
            modes: [bound_bits(&self.textures()), 0, 0, 0],
            swizzles: pack_swizzles(swizzles),
        }
    }
}

impl MaterialDocument for SkyMaterial {
    const FAMILY: MaterialFamily = MaterialFamily::Sky;

    fn validate(&self) -> Result<(), MaterialError> {
        for (name, disc) in [("sun", &self.sun), ("moon.disc", &self.moon.disc)] {
            non_negative(&format!("{name}.size"), disc.size)?;
            unit(&format!("{name}.softness"), disc.softness)?;
        }
        unit("stars.twinkle", self.stars.twinkle)?;
        for (name, layer) in [("clouds", &self.clouds), ("cirrus", &self.cirrus)] {
            non_negative(&format!("{name}.scale"), layer.scale)?;
            finite(&format!("{name}.scroll[0]"), layer.scroll[0])?;
            finite(&format!("{name}.scroll[1]"), layer.scroll[1])?;
            non_negative(&format!("{name}.dissolve_softness"), layer.dissolve_softness)?;
        }
        Ok(())
    }

    fn textures(&self) -> Vec<Option<AssetId>> {
        vec![self.moon.map, self.stars.map, self.stars.milky_way, self.clouds.map, self.cirrus.map]
    }

    fn flags(&self) -> MaterialFlags {
        MaterialFlags::empty()
    }

    fn params(&self, swizzles: &[Swizzle]) -> Vec<u8> {
        bytemuck::bytes_of(&self.gpu_params(swizzles)).to_vec()
    }
}
