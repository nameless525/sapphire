//! The environment family: a stylized GGX for the world, kept on the characters' side of the
//! picture (spec §5.12 "Environment", world_lighting_post.md §2). Its shader is the renderer's
//! `environment.wgsl` (M1 W4), whose header says how each field shades.

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_reflect::Versioned;
use sapphire_serialize::ReflectDocument;
use serde::{Deserialize, Serialize};

use super::common::{
    AlphaMode, DistanceStylization, bound_bits, color, gpu_block, invalid, non_negative, pack_swizzles,
    rgb_w, unit,
};
use super::{MaterialDocument, MaterialError, MaterialFamily, MaterialFlags, TextureSlot, TextureUsage};
use crate::AssetId;
use crate::texture::Swizzle;

/// An environment material, as a document (`sapphire::render::EnvironmentMaterial`, version 1).
///
/// PBR inputs bent toward a painted read: diffuse wrap, a terminator whose width grows with
/// distance, an authored shadow tint, a clamped specular in a narrow roughness range, and
/// distance desaturation. Time of day supplies the shadow tint and ambient these modulate, and
/// decides how much of the emission shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[reflect(Document)]
#[serde(default)]
pub struct EnvironmentMaterial {
    /// Linear RGBA; multiplies `base_color_map`.
    pub base_color: [f32; 4],
    /// Albedo with painted gradients and AO baked in.
    pub base_color_map: Option<AssetId>,
    pub alpha: AlphaMode,
    pub normal_map: Option<AssetId>,
    pub normal_strength: f32,
    /// Linear RGBA: R ambient occlusion (the ambient light's), G cavity (the ambient light's and
    /// every highlight's), B specular mask, A roughness (× `roughness`).
    pub mask_map: Option<AssetId>,
    pub roughness: f32,
    pub metallic: f32,
    /// Roughness is remapped into this range: WuWa keeps reflective surfaces under 0.4.
    pub roughness_range: [f32; 2],
    /// The most specular light a pixel takes, as a multiple of the light's radiance.
    pub specular_clamp: f32,
    /// Diffuse wrap: 0 is Lambert, 0.3–0.5 a soft painted falloff.
    pub wrap: f32,
    pub terminator: Terminator,
    /// Multiplies the time of day's shadow tint, the share of the key light kept on the shadow
    /// side and in cast shadow: shadows take a color, never grey.
    pub shadow_tint: [f32; 3],
    pub distance: DistanceStylization,
    /// Linear RGB × intensity; multiplies `emission_map`. Time of day scales it by its emission
    /// key (M1 W1's keyed emission), but for the share kept steady.
    pub emission: [f32; 3],
    pub emission_map: Option<AssetId>,
    /// The share of the emission that shines the same at every hour, 0 to 1 (M1 W1's keyed
    /// emission). 0, the default, leaves all of it to time of day's emission key, as paper
    /// lanterns and lit windows want: pale by day, glowing at night. 1 keeps it as authored
    /// whatever the hour, for a sign or a crystal that glows by day too. Written only when set,
    /// so documents from before it read and write as they did.
    #[serde(skip_serializing_if = "is_zero")]
    pub emission_steady: f32,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

impl Versioned for EnvironmentMaterial {
    const VERSION: u32 = 1;
}

impl Default for EnvironmentMaterial {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            base_color_map: None,
            alpha: AlphaMode::default(),
            normal_map: None,
            normal_strength: 1.0,
            mask_map: None,
            roughness: 0.8,
            metallic: 0.0,
            roughness_range: [0.0, 1.0],
            specular_clamp: 1.0,
            wrap: 0.0,
            terminator: Terminator::default(),
            shadow_tint: [1.0; 3],
            distance: DistanceStylization::default(),
            emission: [0.0; 3],
            emission_map: None,
            emission_steady: 0.0,
        }
    }
}

/// The light/shadow boundary: `smoothstep(-w, w, x)` of N·L pushed toward the deepest shadow by
/// the cast shadow, `w` the width at the point's view distance, at least half a pixel's change.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Terminator {
    /// Half-width of the transition up close; near 0 is a hard step for props that should
    /// read like characters.
    pub width: f32,
    /// Added to the width per meter of view distance, so far shadow edges soften.
    pub growth: f32,
    /// A saturated band along the boundary: a small ramp sampled by the transition.
    pub band: Option<TerminatorBand>,
}

impl Default for Terminator {
    fn default() -> Self {
        Self { width: 0.05, growth: 0.0, band: None }
    }
}

/// The terminator band: one row, read at the transition's value (0 on the shadow side, 1 on the
/// lit side) and multiplied into the key light's diffuse by `strength`. Its ends should be white,
/// so only the transition takes its color.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct TerminatorBand {
    /// A ramp across the transition, one row.
    pub map: AssetId,
    #[serde(default = "TerminatorBand::default_strength")]
    pub strength: f32,
}

impl TerminatorBand {
    fn default_strength() -> f32 {
        1.0
    }
}

/// The environment family's GPU block (WGSL `EnvironmentParams`). 144 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct EnvironmentParams {
    /// Linear RGBA.
    pub base_color: [f32; 4],
    /// x: roughness; y: metallic; z: normal strength; w: specular clamp.
    pub surface: [f32; 4],
    /// x, y: roughness range; z: alpha-test cutoff; w: diffuse wrap.
    pub shaping: [f32; 4],
    /// x: terminator width; y: its growth per meter; z: band strength (0 without a band); w: 0.
    pub terminator: [f32; 4],
    /// rgb: shadow tint multiplier; a: 0.
    pub shadow_tint: [f32; 4],
    /// x: start; y: end (meters); z: desaturation; w: 0.
    pub distance: [f32; 4],
    /// rgb: emission × intensity; a: the share of it kept steady at every hour
    /// ([`EnvironmentMaterial::emission_steady`]), the rest following time of day's emission
    /// scale. Blocks cooked before it hold 0 there, which is the default, so they need no recook.
    pub emission: [f32; 4],
    /// x: flags (bit `i` for texture slot `i`, then `FLAG_*`).
    pub modes: [u32; 4],
    /// Each texture slot's swizzle code, two to a word.
    pub swizzles: [u32; 4],
}

gpu_block!(EnvironmentParams { base_color, surface, shaping, terminator, shadow_tint, distance, emission });

impl EnvironmentParams {
    pub const FLAG_ALPHA_TEST: u32 = 1 << 8;
    pub const FLAG_TRANSPARENT: u32 = 1 << 9;
}

impl EnvironmentMaterial {
    pub const BASE_COLOR: usize = 0;
    pub const NORMAL: usize = 1;
    pub const MASK: usize = 2;
    pub const TERMINATOR_BAND: usize = 3;
    pub const EMISSION: usize = 4;

    /// The family's texture slots, in binding order.
    pub const TEXTURE_SLOTS: &[TextureSlot] = &[
        TextureSlot { name: "base_color", usage: TextureUsage::Color },
        TextureSlot { name: "normal", usage: TextureUsage::Normal },
        TextureSlot { name: "mask", usage: TextureUsage::Mask },
        TextureSlot { name: "terminator_band", usage: TextureUsage::Ramp },
        TextureSlot { name: "emission", usage: TextureUsage::Light },
    ];

    /// The GPU block, given the swizzle of each slot's texture.
    pub fn gpu_params(&self, swizzles: &[Swizzle]) -> EnvironmentParams {
        let mut flags = bound_bits(&self.textures());
        match self.alpha {
            AlphaMode::Opaque => {}
            AlphaMode::Mask { .. } => flags |= EnvironmentParams::FLAG_ALPHA_TEST,
            AlphaMode::Blend => flags |= EnvironmentParams::FLAG_TRANSPARENT,
        }
        let band = self.terminator.band.map_or(0.0, |band| band.strength);
        EnvironmentParams {
            base_color: self.base_color,
            surface: [self.roughness, self.metallic, self.normal_strength, self.specular_clamp],
            shaping: [self.roughness_range[0], self.roughness_range[1], self.alpha.cutoff(), self.wrap],
            terminator: [self.terminator.width, self.terminator.growth, band, 0.0],
            shadow_tint: rgb_w(self.shadow_tint, 0.0),
            distance: self.distance.gpu(),
            emission: rgb_w(self.emission, self.emission_steady),
            modes: [flags, 0, 0, 0],
            swizzles: pack_swizzles(swizzles),
        }
    }
}

impl MaterialDocument for EnvironmentMaterial {
    const FAMILY: MaterialFamily = MaterialFamily::Environment;

    fn validate(&self) -> Result<(), MaterialError> {
        color("base_color", &self.base_color)?;
        unit("base_color[3]", self.base_color[3])?;
        self.alpha.validate("alpha")?;
        non_negative("normal_strength", self.normal_strength)?;
        unit("roughness", self.roughness)?;
        unit("metallic", self.metallic)?;
        let [low, high] = self.roughness_range;
        unit("roughness_range[0]", low)?;
        unit("roughness_range[1]", high)?;
        if high < low {
            return Err(invalid("roughness_range", "must be ordered low to high"));
        }
        non_negative("specular_clamp", self.specular_clamp)?;
        unit("wrap", self.wrap)?;
        non_negative("terminator.width", self.terminator.width)?;
        non_negative("terminator.growth", self.terminator.growth)?;
        if let Some(band) = &self.terminator.band {
            non_negative("terminator.band.strength", band.strength)?;
        }
        color("shadow_tint", &self.shadow_tint)?;
        self.distance.validate("distance")?;
        color("emission", &self.emission)?;
        unit("emission_steady", self.emission_steady)
    }

    fn textures(&self) -> Vec<Option<AssetId>> {
        vec![
            self.base_color_map,
            self.normal_map,
            self.mask_map,
            self.terminator.band.map(|band| band.map),
            self.emission_map,
        ]
    }

    fn flags(&self) -> MaterialFlags {
        match self.alpha {
            AlphaMode::Opaque => MaterialFlags::empty(),
            AlphaMode::Mask { .. } => MaterialFlags::ALPHA_TEST,
            AlphaMode::Blend => MaterialFlags::TRANSPARENT,
        }
    }

    fn params(&self, swizzles: &[Swizzle]) -> Vec<u8> {
        bytemuck::bytes_of(&self.gpu_params(swizzles)).to_vec()
    }
}
