//! The foliage family: NPR leaves, grass and the wood that carries them (world_lighting_post.md
//! §8, M1 plan pillar 7), lit by the environment's light model (wrap, a terminator that softens
//! with distance, time of day's shadow tint) on cook-time volume normals, so a canopy reads as
//! one painted mass in the world it stands in. Its shader is the renderer's `foliage.wgsl`
//! (M1 W6), whose header says how each field shades.

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_reflect::Versioned;
use sapphire_serialize::ReflectDocument;
use serde::{Deserialize, Serialize};

use super::common::{
    DistanceStylization, bound_bits, color, gpu_block, non_negative, pack_swizzles, rgb_w, unit,
};
use super::{MaterialDocument, MaterialError, MaterialFamily, MaterialFlags, TextureSlot, TextureUsage};
use crate::AssetId;
use crate::texture::Swizzle;

/// A foliage material, as a document (`sapphire::render::FoliageMaterial`, version 1).
///
/// Wrapped N·L on the cook-time sphere or hull normals a foliage mesh carries, through the
/// environment's terminator remap (a width that grows with distance) and time of day's shadow
/// tint; canopy occlusion and a color gradient along the mesh's cooked gradient coordinate
/// (root to tip, the canopy's inside to its outside); light through thin leaves seen against
/// the key light; three levels of wind. Cards are alpha-tested (alpha-to-coverage under MSAA);
/// an `alpha_cutoff` of 0 draws opaque, for trunks, branches and solid canopies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[reflect(Document)]
#[serde(default)]
pub struct FoliageMaterial {
    /// Linear RGBA; multiplies `base_color_map`.
    pub base_color: [f32; 4],
    /// Leaf or blade cards; alpha is coverage.
    pub base_color_map: Option<AssetId>,
    /// Coverage below this is discarded; 0 draws the material opaque, without a test.
    pub alpha_cutoff: f32,
    /// Linear RGB: R thickness (scales the translucency), G occlusion (darkens the ambient light
    /// and pushes the key light toward the shadow side, as vertex AO does); B unused.
    pub mask_map: Option<AssetId>,
    /// Diffuse wrap, 0 to 1; foliage wants it soft.
    pub wrap: f32,
    /// Half-width of the light/shadow transition up close, in N·L.
    pub terminator_width: f32,
    /// Added to the width per meter of view distance, so far canopies soften into painted masses
    /// (the environment's terminator remap).
    pub terminator_growth: f32,
    /// Multiplies the time of day's shadow tint.
    pub shadow_tint: [f32; 3],
    /// How much leaves darken toward the canopy's inside (the gradient coordinate's 0), 0 to 1.
    pub canopy_occlusion: f32,
    pub translucency: Translucency,
    pub gradient: FoliageGradient,
    pub wind: Wind,
    pub distance: DistanceStylization,
}

impl Versioned for FoliageMaterial {
    const VERSION: u32 = 1;
}

impl Default for FoliageMaterial {
    fn default() -> Self {
        Self {
            base_color: [0.35, 0.6, 0.25, 1.0],
            base_color_map: None,
            alpha_cutoff: 0.5,
            mask_map: None,
            wrap: 0.5,
            terminator_width: 0.15,
            terminator_growth: 0.004,
            shadow_tint: [1.0; 3],
            canopy_occlusion: 0.5,
            translucency: Translucency::default(),
            gradient: FoliageGradient::default(),
            wind: Wind::default(),
            distance: DistanceStylization::default(),
        }
    }
}

/// Light through leaves seen against the key light: `pow(saturate(dot(-L, V)), power) ×
/// thickness`, so a canopy against a low sun glows at its edges.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Translucency {
    /// Linear RGB.
    pub color: [f32; 3],
    pub strength: f32,
    pub power: f32,
}

impl Default for Translucency {
    fn default() -> Self {
        Self { color: [0.8, 1.0, 0.4], strength: 0.5, power: 4.0 }
    }
}

/// A color gradient multiplied into the albedo along the mesh's cooked gradient coordinate:
/// from `root` (0: a blade's or trunk's root, or deep inside a canopy) to `tip` (1: the tip, or
/// the canopy's outer surface).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct FoliageGradient {
    pub root: [f32; 3],
    pub tip: [f32; 3],
}

impl Default for FoliageGradient {
    fn default() -> Self {
        Self { root: [1.0; 3], tip: [1.0; 3] }
    }
}

/// Response to the frame's wind, in meters at unit wind strength, each level scaled by the
/// mesh's cooked per-vertex weight: trunk sway, branch bend and leaf flutter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Wind {
    /// How far the top leans and sways.
    pub sway: f32,
    /// How far a branch's tip swings.
    pub bend: f32,
    /// How far a leaf flutters.
    pub flutter: f32,
    /// Flutter cycles per second.
    pub flutter_frequency: f32,
}

impl Default for Wind {
    fn default() -> Self {
        Self { sway: 0.05, bend: 0.1, flutter: 0.02, flutter_frequency: 2.0 }
    }
}

/// The foliage family's GPU block (WGSL `FoliageParams`). 160 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct FoliageParams {
    /// Linear RGBA.
    pub base_color: [f32; 4],
    /// x: alpha cutoff; y: wrap; z: terminator width; w: its growth per meter.
    pub shading: [f32; 4],
    /// rgb: shadow tint multiplier; a: 0.
    pub shadow_tint: [f32; 4],
    /// rgb: translucency color × strength; a: power.
    pub translucency: [f32; 4],
    /// rgb: root color; a: canopy occlusion.
    pub gradient_root: [f32; 4],
    /// rgb: tip color; a: 0.
    pub gradient_tip: [f32; 4],
    /// x: sway; y: bend; z: flutter; w: flutter frequency.
    pub wind: [f32; 4],
    /// x: start; y: end; z: desaturation; w: 0.
    pub distance: [f32; 4],
    /// x: flags (bit `i` for texture slot `i`).
    pub modes: [u32; 4],
    /// Each texture slot's swizzle code, two to a word.
    pub swizzles: [u32; 4],
}

gpu_block!(FoliageParams {
    base_color,
    shading,
    shadow_tint,
    translucency,
    gradient_root,
    gradient_tip,
    wind,
    distance,
});

impl FoliageMaterial {
    pub const BASE_COLOR: usize = 0;
    pub const MASK: usize = 1;

    /// The family's texture slots, in binding order.
    pub const TEXTURE_SLOTS: &[TextureSlot] = &[
        TextureSlot { name: "base_color", usage: TextureUsage::Color },
        TextureSlot { name: "mask", usage: TextureUsage::Mask },
    ];

    /// The GPU block, given the swizzle of each slot's texture.
    pub fn gpu_params(&self, swizzles: &[Swizzle]) -> FoliageParams {
        let t = &self.translucency;
        FoliageParams {
            base_color: self.base_color,
            shading: [self.alpha_cutoff, self.wrap, self.terminator_width, self.terminator_growth],
            shadow_tint: rgb_w(self.shadow_tint, 0.0),
            translucency: rgb_w(t.color.map(|c| c * t.strength), t.power),
            gradient_root: rgb_w(self.gradient.root, self.canopy_occlusion),
            gradient_tip: rgb_w(self.gradient.tip, 0.0),
            wind: [self.wind.sway, self.wind.bend, self.wind.flutter, self.wind.flutter_frequency],
            distance: self.distance.gpu(),
            modes: [bound_bits(&self.textures()), 0, 0, 0],
            swizzles: pack_swizzles(swizzles),
        }
    }
}

impl MaterialDocument for FoliageMaterial {
    const FAMILY: MaterialFamily = MaterialFamily::Foliage;

    fn validate(&self) -> Result<(), MaterialError> {
        color("base_color", &self.base_color)?;
        unit("base_color[3]", self.base_color[3])?;
        unit("alpha_cutoff", self.alpha_cutoff)?;
        unit("wrap", self.wrap)?;
        non_negative("terminator_width", self.terminator_width)?;
        non_negative("terminator_growth", self.terminator_growth)?;
        color("shadow_tint", &self.shadow_tint)?;
        unit("canopy_occlusion", self.canopy_occlusion)?;
        color("translucency.color", &self.translucency.color)?;
        non_negative("translucency.strength", self.translucency.strength)?;
        non_negative("translucency.power", self.translucency.power)?;
        color("gradient.root", &self.gradient.root)?;
        color("gradient.tip", &self.gradient.tip)?;
        non_negative("wind.sway", self.wind.sway)?;
        non_negative("wind.bend", self.wind.bend)?;
        non_negative("wind.flutter", self.wind.flutter)?;
        non_negative("wind.flutter_frequency", self.wind.flutter_frequency)?;
        self.distance.validate("distance")
    }

    fn textures(&self) -> Vec<Option<AssetId>> {
        vec![self.base_color_map, self.mask_map]
    }

    fn flags(&self) -> MaterialFlags {
        if self.alpha_cutoff > 0.0 { MaterialFlags::ALPHA_TEST } else { MaterialFlags::empty() }
    }

    fn params(&self, swizzles: &[Swizzle]) -> Vec<u8> {
        bytemuck::bytes_of(&self.gpu_params(swizzles)).to_vec()
    }
}
