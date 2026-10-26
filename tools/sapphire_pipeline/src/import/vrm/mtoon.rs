//! MToon → anime character materials.
//!
//! MToon (`VRMC_materials_mtoon` 1.0; VRM 0.x's `VRM/MToon` shader migrates to it, see
//! [`Mtoon::from_v0`]) shades in two tones:
//!
//! ```text
//! shading = linearstep(−1 + toony, 1 − toony, N·L + shadingShift + shiftTexture.r × scale)
//! color   = mix(shadeColor × shadeTexture, baseColor × baseTexture, shading) × light
//! rim     = (matcap × matcapFactor + (1 − N·V + lift)^power × rimColor) × rimTexture
//!           × mix(1, light, rimLightingMix)
//! ```
//!
//! with an inverted-hull outline whose width is in meters or a fraction of the screen's
//! height, scaled by a texture's G channel. That is Sapphire's band mode with two tones: the
//! shadow side takes an authored color and the transition is a threshold on biased N·L.
//!
//! # The mapping
//!
//! Field by field, onto [`AnimeCharacterMaterial`] (A3) and the cooked mesh:
//!
//! | MToon (1.0 name) | Sapphire | How |
//! |---|---|---|
//! | `baseColorFactor`, `baseColorTexture` (glTF) | `base_color`, `base_color_map` | As they are: linear RGBA, an sRGB color texture |
//! | `alphaMode`, `alphaCutoff` (glTF) | `alpha` | Opaque, `Mask { cutoff }` (clamped to 1), `Blend` |
//! | `shadingShiftFactor`, `shadingToonyFactor` | `light_split` | Exact: threshold `−shift`, softness `1 − toony`, the half-width of MToon's `linearstep` (Sapphire's step is a smoothstep between the same ends) |
//! | `shadeColorFactor` | `diffuse: Bands` with two tones, `regions[0].shade_color` | `shadeColor / baseColor` per channel, since the shade side is `albedo × shade_color`; a black base channel keeps the shade value |
//! | `shadeMultiplyTexture` | (the ratio above) | Exact when it is the base color's image, as exporters write it; otherwise approximated by the base texture. Extracted as a color texture either way |
//! | `shadingShiftTexture` (R) and its `scale` | `control_map`, G (shadow bias) | Extraction writes an ILM document whose G is the image's R mapped onto `[0.5, 0.5 + scale / 2]`: A7's encoding of an N·L offset of `r × scale` (`character_maps::ilm::encode_shadow_bias`). R, B and A stay neutral. Before extraction (the material the import generates), the texture's mean offset, measured on import, joins the threshold instead |
//! | `parametricRimColorFactor` | `rim.strength`, `regions[0].rim_color` | Strength is the brightest channel, the tint the color over it |
//! | `parametricRimFresnelPowerFactor`, `parametricRimLiftFactor` | `rim.fresnel` | The N·V where MToon's falloff is at half: `1 + lift − 0.5^(1 / power)`, clamped to `[0, 1]`, the depth-offset rim's Fresnel gate (M1 C5): Sapphire's rim is a band of its default width in pixels at the silhouette, cut where N·V passes it |
//! | `matcapTexture`, `matcapFactor` | `matcap` | The texture as a color texture; strength the factor's brightest channel |
//! | `rimLightingMixFactor` | `rim.light_gate`, `matcap.shadow_strength` | As it is: MToon multiplies the rim by the light by this much, Sapphire keeps it to the lit side by this much (M1 C5); and `strength × (1 − mix)` for the matcap, how much of it the shadow side keeps |
//! | `outlineWidthMode`, `outlineWidthFactor` | `outline.width` (pixels at 1080p) and `outline.curve` | None → 0. Screen: `factor × 1080`, the factor being a fraction of the full screen height, kept from the camera out to the curve's cutoff (`near` at 0 m, `far` at the cutoff). World: the width in pixels from [`WORLD_OUTLINE_DISTANCE`] through a [`WORLD_OUTLINE_FOV_DEGREES`] field of view, with the curve's three knots at that distance, so the line keeps its width in meters at every distance and field of view (outlines v2, M1 C4) |
//! | `outlineWidthMultiplyTexture` (G) | vertex color G, the outline-width scale | Sampled at each vertex's UV and baked into the outline-width scale the mesh carries (the research note's mobile-friendly choice). Extracted as a mask too |
//! | `outlineColorFactor` | `regions[0].outline_color` | As it is |
//! | `outlineLightingMixFactor` | `outline.lighting` | As it is: MToon multiplies the outline by the light by this much, Sapphire lights it as its surface by this much (key light through the split, plus flattened GI) |
//! | `normalTexture` (glTF) | `normal_map` | As it is, as a normal map; its `scale` is dropped |
//!
//! Dropped, because the anime character family (A3, C1's contract) has nowhere to put them yet;
//! their textures are still extracted, so nothing authored is lost:
//!
//! - `rimMultiplyTexture`: no rim mask slot; the rim is masked per region (`rim_strength`, M1
//!   C5), which a control map's material IDs pick. Extracted as a mask.
//! - `emissiveFactor`, `emissiveTexture`, `KHR_materials_emissive_strength` and
//!   `VRMC_materials_hdr_emissiveMultiplier`: no emission (C7). The texture is extracted as color.
//! - `uvAnimation*` and its mask: no UV animation. The mask is extracted.
//! - `giEqualizationFactor`: GI stylization is the character light rig's (C2).
//! - `transparentWithZWrite`, `renderQueueOffsetNumber`: no sort control; `Blend` materials draw
//!   in the transparent pass after the sky (C1), back to front once draws sort.
//! - `doubleSided` (glTF): materials carry no cull mode; characters draw back-face culled.
//! - `KHR_texture_transform` and `texCoord`: meshes carry one UV set and materials no UV
//!   transform.
//! - `surface` stays `Body`: MToon doesn't say which material is a face, hair or eyes, and
//!   those need maps (SDF face maps, hair masks) that an artist authors on the extracted
//!   document.
//!
//! A material with `KHR_materials_unlit` (or a VRM 0.x unlit shader) maps as an MToon whose
//! shade side is its lit side, without rim or outline ([`Mtoon::unlit`]).

use sapphire_assets::AssetId;
use sapphire_assets::character_maps::ilm;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, BandDiffuse, CharacterRegion, DiffuseMode, LightSplit, Matcap,
    Outline, OutlineCurve, Rim,
};
use vrm_spec::vrm_0_0::VRMMaterial;
use vrm_spec::vrmc_materials_mtoon_1_0::{OutlineWidthMode, VrmcMaterialsMtoonSchema};

use super::{Images, VrmImportError};
use crate::color::srgb_to_linear;
use crate::texture::{ColorSettings, MaskSettings, NormalSettings, TextureClass};

/// The target height Sapphire's outline widths are measured at, in pixels
/// ([`Outline::width`]).
const OUTLINE_REFERENCE_HEIGHT: f32 = 1080.0;

/// World-space outline widths become pixels as seen from this far, in meters: a full-length
/// shot of a 1.6 m character fills a 45° frame from about 2 m.
pub const WORLD_OUTLINE_DISTANCE: f32 = 2.0;

/// The vertical field of view world-space outline widths are converted through, in degrees:
/// the one C4 normalizes outline widths to.
pub const WORLD_OUTLINE_FOV_DEGREES: f32 = 45.0;

/// A base color channel at most this is black: shade colors are ratios to the base color.
const BLACK: f32 = 1e-4;

/// The channels of a texture a use reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    R,
    G,
    B,
    /// Color: all of them.
    Rgba,
}

/// A material's reference to a glTF texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureRef {
    /// An index into the document's textures.
    pub texture: usize,
    pub channel: Channel,
}

/// The shading shift texture: an offset to N·L of `r × scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadingShift {
    pub texture: TextureRef,
    pub scale: f32,
    /// The texture's mean `r`, which the import measures: what a material without the texture
    /// (one the import generates) takes for every texel.
    pub mean: f32,
}

impl ShadingShift {
    /// The ILM shadow-bias range the texture's `[0, 1]` maps onto: texel `r` offsets N·L by
    /// `r × scale` (A7's encoding). Exact for scales up to ±1; beyond, the range ends at the
    /// channel's end, so texels short of `r = 1` offset less than `r × scale` (the channel
    /// spans offsets of ±1, which already move the split past every N·L).
    pub fn shadow_bias_range(&self) -> [f32; 2] {
        [ilm::encode_shadow_bias(0.0), ilm::encode_shadow_bias(self.scale)]
    }
}

/// What a texture does in an MToon material, which decides how it is extracted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TextureUse {
    BaseColor,
    Shade,
    Normal,
    /// An input of an ILM document rather than a texture of its own.
    ShadingShift,
    Matcap,
    RimMultiply,
    OutlineWidth,
    UvAnimationMask,
    Emissive,
}

impl TextureUse {
    /// The texture class an image extracted for this use gets, or `None` for an ILM input.
    pub fn class(self) -> Option<TextureClass> {
        Some(match self {
            Self::BaseColor | Self::Shade | Self::Matcap | Self::Emissive => {
                TextureClass::Color(ColorSettings::default())
            }
            Self::Normal => TextureClass::Normal(NormalSettings::default()),
            Self::RimMultiply | Self::OutlineWidth | Self::UvAnimationMask => {
                TextureClass::Mask(MaskSettings::default())
            }
            Self::ShadingShift => return None,
        })
    }
}

/// How wide the outline is, before the multiply texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutlineWidth {
    None,
    /// Meters.
    World(f32),
    /// A fraction of the screen's height.
    Screen(f32),
}

/// An MToon material in 1.0 semantics: what the mapping reads and what extraction writes out.
/// Colors are linear.
#[derive(Clone, Debug, PartialEq)]
pub struct Mtoon {
    pub base_color: [f32; 4],
    pub base_color_texture: Option<TextureRef>,
    pub alpha: AlphaMode,
    pub normal_texture: Option<TextureRef>,
    pub shade_color: [f32; 3],
    pub shade_multiply_texture: Option<TextureRef>,
    pub shading_shift: f32,
    pub shading_shift_texture: Option<ShadingShift>,
    pub shading_toony: f32,
    pub matcap_factor: [f32; 3],
    pub matcap_texture: Option<TextureRef>,
    pub rim_color: [f32; 3],
    pub rim_fresnel_power: f32,
    pub rim_lift: f32,
    pub rim_lighting_mix: f32,
    pub rim_multiply_texture: Option<TextureRef>,
    pub outline_width: OutlineWidth,
    pub outline_width_multiply_texture: Option<TextureRef>,
    pub outline_color: [f32; 3],
    pub outline_lighting_mix: f32,
    pub emissive_texture: Option<TextureRef>,
    pub uv_animation_mask_texture: Option<TextureRef>,
}

/// Where a material's textures are checked against the document, and its shading shift
/// texture measured.
pub(super) struct TextureCheck<'a> {
    /// The material, in messages.
    pub material: &'a str,
    pub images: Images<'a>,
}

impl TextureCheck<'_> {
    fn reference(
        &self,
        property: &str,
        texture: usize,
        channel: Channel,
    ) -> Result<TextureRef, VrmImportError> {
        let textures = self.images.textures();
        if texture < textures {
            Ok(TextureRef { texture, channel })
        } else {
            Err(VrmImportError::reference(
                format!("material {} {property}", self.material),
                format!("names texture {texture}, but the document has {textures}"),
            ))
        }
    }

    /// The shading shift texture `texture` (read in R) with `scale`, its mean measured.
    fn shading_shift(
        &self,
        property: &str,
        texture: usize,
        scale: f32,
    ) -> Result<ShadingShift, VrmImportError> {
        let texture = self.reference(property, texture, Channel::R)?;
        let mask = self.images.mask(self.images.image_of(texture.texture), texture.channel)?;
        let sum: f64 = mask.values.iter().map(|&value| f64::from(value)).sum();
        let mean = if mask.values.is_empty() { 0.0 } else { (sum / mask.values.len() as f64) as f32 };
        Ok(ShadingShift { texture, scale, mean })
    }
}

/// The MToon defaults with a glTF material's core properties, which VRM 1.0's MToon keeps
/// there: base color, alpha, normal and emissive textures.
fn core(material: &gltf::Material<'_>) -> Mtoon {
    let pbr = material.pbr_metallic_roughness();
    let color = |info: Option<gltf::texture::Info<'_>>| {
        info.map(|info| TextureRef { texture: info.texture().index(), channel: Channel::Rgba })
    };
    Mtoon {
        base_color: pbr.base_color_factor(),
        base_color_texture: color(pbr.base_color_texture()),
        alpha: crate::import::gltf::alpha_mode(material),
        normal_texture: material
            .normal_texture()
            .map(|normal| TextureRef { texture: normal.texture().index(), channel: Channel::Rgba }),
        emissive_texture: color(material.emissive_texture()),
        ..Mtoon::UNSHADED
    }
}

impl Mtoon {
    /// MToon 1.0's defaults, without textures, for a white opaque base: everything the
    /// specification's schema leaves out takes these.
    const UNSHADED: Self = Self {
        base_color: [1.0; 4],
        base_color_texture: None,
        alpha: AlphaMode::Opaque,
        normal_texture: None,
        shade_color: [0.0; 3],
        shade_multiply_texture: None,
        shading_shift: 0.0,
        shading_shift_texture: None,
        shading_toony: 0.9,
        matcap_factor: [1.0; 3],
        matcap_texture: None,
        rim_color: [0.0; 3],
        rim_fresnel_power: 5.0,
        rim_lift: 0.0,
        rim_lighting_mix: 1.0,
        rim_multiply_texture: None,
        outline_width: OutlineWidth::None,
        outline_width_multiply_texture: None,
        outline_color: [0.0; 3],
        outline_lighting_mix: 1.0,
        emissive_texture: None,
        uv_animation_mask_texture: None,
    };

    /// A VRM 1.0 MToon material: the glTF material's core properties and its
    /// `VRMC_materials_mtoon` extension.
    pub(super) fn from_v1(
        material: &gltf::Material<'_>,
        schema: &VrmcMaterialsMtoonSchema,
        check: &TextureCheck<'_>,
    ) -> Result<Self, VrmImportError> {
        if schema.spec_version != "1.0" {
            return Err(VrmImportError::SpecVersion {
                extension: super::MTOON,
                found: schema.spec_version.clone(),
            });
        }
        // The core material's textures were checked when gltf parsed the document; the
        // extension's are checked here.
        let mut mtoon = core(material);
        let texture = |property: &str, info: &Option<gltf::json::texture::Info>, channel| {
            info.as_ref().map(|info| check.reference(property, info.index.value(), channel)).transpose()
        };
        let f = |value: Option<f64>, default: f32| value.map_or(default, |value| value as f32);
        let rgb = |value: Option<[f64; 3]>, default: [f32; 3]| value.map_or(default, |v| v.map(|c| c as f32));
        let defaults = Self::UNSHADED;
        mtoon.shade_color = rgb(schema.shade_color_factor, defaults.shade_color);
        mtoon.shade_multiply_texture =
            texture("shadeMultiplyTexture", &schema.shade_multiply_texture, Channel::Rgba)?;
        mtoon.shading_shift = f(schema.shading_shift_factor, defaults.shading_shift);
        mtoon.shading_shift_texture = schema
            .shading_shift_texture
            .as_ref()
            .map(|info| check.shading_shift("shadingShiftTexture", info.index.value(), f(info.scale, 1.0)))
            .transpose()?;
        mtoon.shading_toony = f(schema.shading_toony_factor, defaults.shading_toony);
        mtoon.matcap_factor = rgb(schema.matcap_factor, defaults.matcap_factor);
        mtoon.matcap_texture = texture("matcapTexture", &schema.matcap_texture, Channel::Rgba)?;
        mtoon.rim_color = rgb(schema.parametric_rim_color_factor, defaults.rim_color);
        mtoon.rim_fresnel_power = f(schema.parametric_rim_fresnel_power_factor, defaults.rim_fresnel_power);
        mtoon.rim_lift = f(schema.parametric_rim_lift_factor, defaults.rim_lift);
        mtoon.rim_lighting_mix = f(schema.rim_lighting_mix_factor, defaults.rim_lighting_mix);
        mtoon.rim_multiply_texture =
            texture("rimMultiplyTexture", &schema.rim_multiply_texture, Channel::Rgba)?;
        let width = f(schema.outline_width_factor, 0.0);
        mtoon.outline_width = match schema.outline_width_mode {
            None | Some(OutlineWidthMode::None) => OutlineWidth::None,
            Some(OutlineWidthMode::WorldCoordinates) => OutlineWidth::World(width),
            Some(OutlineWidthMode::ScreenCoordinates) => OutlineWidth::Screen(width),
        };
        mtoon.outline_width_multiply_texture =
            texture("outlineWidthMultiplyTexture", &schema.outline_width_multiply_texture, Channel::G)?;
        mtoon.outline_color = rgb(schema.outline_color_factor, defaults.outline_color);
        mtoon.outline_lighting_mix = f(schema.outline_lighting_mix_factor, defaults.outline_lighting_mix);
        mtoon.uv_animation_mask_texture =
            texture("uvAnimationMaskTexture", &schema.uv_animation_mask_texture, Channel::B)?;
        Ok(mtoon)
    }

    /// An unlit material (`KHR_materials_unlit`, or a VRM 0.x unlit shader): its shade side is
    /// its lit side, with no rim and no outline.
    pub(super) fn unlit(material: &gltf::Material<'_>) -> Self {
        let mtoon = core(material);
        let [r, g, b, _] = mtoon.base_color;
        Self {
            shade_color: [r, g, b],
            shade_multiply_texture: mtoon.base_color_texture,
            shading_toony: 1.0,
            ..mtoon
        }
    }

    /// A VRM 0.x `VRM/MToon` material, migrated to 1.0 semantics from its Unity material
    /// properties (VRM 0.x's `materialProperties`), as UniVRM and three-vrm (both MIT) migrate
    /// them:
    ///
    /// - **Colors** (`_Color`, `_ShadeColor`, `_RimColor`, `_OutlineColor`) are Unity's sRGB
    ///   values: decoded to linear, alpha kept.
    /// - **Shading:** 0.x maps N·L linearly from `_ShadeShift` up to
    ///   `lerp(1, _ShadeShift, _ShadeToony)`. The same ramp in 1.0's terms is
    ///   `toony = lerp(_ShadeToony, 1, ½ + ½ _ShadeShift)` and `shift = −_ShadeShift − (1 − toony)`.
    /// - **Shading grade** (`_ShadingGradeTexture`, R): 0.x scales half-Lambert by
    ///   `1 − _ShadingGradeRate × (1 − r)`, which moves the split by `−rate × (1 − r)` near it.
    ///   That becomes a shading shift texture with scale `rate`, and `rate` off the shift.
    /// - **Rim:** `_RimFresnelPower`, `_RimLift`, `_RimLightingMix`, `_RimTexture`; the matcap is
    ///   `_SphereAdd` at full strength.
    /// - **Outline:** `_OutlineWidthMode` 1 (world) and 2 (screen). `_OutlineWidth` is
    ///   centimeters in world mode, and in screen mode a percentage of half the screen's height
    ///   (MToon 0.x offsets clip space by `0.01 × width`): ×0.01 and ×0.005. The width texture
    ///   is read from its R channel, as 0.x reads it. `_OutlineLightingMix` is 1.0's lighting
    ///   mix.
    /// - **Alpha:** `_BlendMode` 0 opaque, 1 cutout at `_Cutoff`, 2 and 3 (with Z write) blend.
    /// - **Textures:** `_MainTex`, `_ShadeTexture`, `_BumpMap`, `_EmissionMap` and
    ///   `_UvAnimMaskTexture` (R). `_MainTex`'s scale and offset, `_ReceiveShadow*`,
    ///   `_LightColorAttenuation`, `_IndirectLightIntensity`, `_OutlineScaledMaxDistance`,
    ///   `_OutlineColorMode`, `_CullMode` and the UV animation speeds are dropped, as their 1.0
    ///   counterparts are.
    ///
    /// Missing properties take MToon 0.x's shader defaults.
    pub(super) fn from_v0(
        properties: &VRMMaterial,
        check: &TextureCheck<'_>,
    ) -> Result<Self, VrmImportError> {
        let float = |name: &str, default: f32| {
            properties.float_properties.as_ref().and_then(|map| map.get(name)).map_or(default, |&v| v as f32)
        };
        // Unity colors: sRGB RGB, linear alpha.
        let color = |name: &str, default: [f32; 4]| {
            let value = properties
                .vector_properties
                .as_ref()
                .and_then(|map| map.get(name))
                .filter(|value| value.len() >= 4)
                .map_or(default, |value| [0, 1, 2, 3].map(|i| value[i] as f32));
            let [r, g, b, a] = value;
            [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
        };
        let rgb = |name: &str, default: [f32; 4]| {
            let [r, g, b, _] = color(name, default);
            [r, g, b]
        };
        let texture = |name: &str, channel: Channel| {
            properties
                .texture_properties
                .as_ref()
                .and_then(|map| map.get(name))
                .map(|index| check.reference(name, index.value(), channel))
                .transpose()
        };

        let toony0 = float("_ShadeToony", 0.9);
        let shift0 = float("_ShadeShift", 0.0);
        let toony = toony0 + (1.0 - toony0) * (0.5 + 0.5 * shift0);
        let mut shift = -shift0 - (1.0 - toony);
        let grade = properties.texture_properties.as_ref().and_then(|map| map.get("_ShadingGradeTexture"));
        let shading_shift_texture = match grade {
            Some(index) => {
                let rate = float("_ShadingGradeRate", 1.0);
                shift -= rate;
                Some(check.shading_shift("_ShadingGradeTexture", index.value(), rate)?)
            }
            None => None,
        };
        let alpha = match float("_BlendMode", 0.0) as i32 {
            1 => AlphaMode::Mask { cutoff: float("_Cutoff", 0.5).clamp(0.0, 1.0) },
            2 | 3 => AlphaMode::Blend,
            _ => AlphaMode::Opaque,
        };
        let width = float("_OutlineWidth", 0.5);
        let outline_width = match float("_OutlineWidthMode", 0.0) as i32 {
            1 => OutlineWidth::World(width * 0.01),
            2 => OutlineWidth::Screen(width * 0.005),
            _ => OutlineWidth::None,
        };
        let matcap_texture = texture("_SphereAdd", Channel::Rgba)?;
        Ok(Self {
            base_color: color("_Color", [1.0; 4]),
            base_color_texture: texture("_MainTex", Channel::Rgba)?,
            alpha,
            normal_texture: texture("_BumpMap", Channel::Rgba)?,
            shade_color: rgb("_ShadeColor", [0.97, 0.81, 0.86, 1.0]),
            shade_multiply_texture: texture("_ShadeTexture", Channel::Rgba)?,
            shading_shift: shift,
            shading_shift_texture,
            shading_toony: toony,
            matcap_factor: [1.0; 3],
            matcap_texture,
            rim_color: rgb("_RimColor", [0.0, 0.0, 0.0, 1.0]),
            rim_fresnel_power: float("_RimFresnelPower", 1.0),
            rim_lift: float("_RimLift", 0.0),
            rim_lighting_mix: float("_RimLightingMix", 0.0),
            rim_multiply_texture: texture("_RimTexture", Channel::Rgba)?,
            outline_width,
            outline_width_multiply_texture: texture("_OutlineWidthTexture", Channel::R)?,
            outline_color: rgb("_OutlineColor", [0.0, 0.0, 0.0, 1.0]),
            outline_lighting_mix: float("_OutlineLightingMix", 1.0),
            emissive_texture: texture("_EmissionMap", Channel::Rgba)?,
            uv_animation_mask_texture: texture("_UvAnimMaskTexture", Channel::R)?,
        })
    }

    /// Every texture the material uses, with what for, in [`TextureUse`] order.
    pub fn textures(&self) -> Vec<(TextureUse, TextureRef)> {
        [
            (TextureUse::BaseColor, self.base_color_texture),
            (TextureUse::Shade, self.shade_multiply_texture),
            (TextureUse::Normal, self.normal_texture),
            (TextureUse::ShadingShift, self.shading_shift_texture.map(|shift| shift.texture)),
            (TextureUse::Matcap, self.matcap_texture),
            (TextureUse::RimMultiply, self.rim_multiply_texture),
            (TextureUse::OutlineWidth, self.outline_width_multiply_texture),
            (TextureUse::UvAnimationMask, self.uv_animation_mask_texture),
            (TextureUse::Emissive, self.emissive_texture),
        ]
        .into_iter()
        .filter_map(|(usage, texture)| Some((usage, texture?)))
        .collect()
    }

    /// Whether the material's document binds any texture: a base color, normal or matcap
    /// texture, or a control map from the shading shift texture. Materials that don't draw
    /// the same from the material the importer generates.
    pub fn is_textured(&self) -> bool {
        self.base_color_texture.is_some()
            || self.normal_texture.is_some()
            || self.shading_shift_texture.is_some()
            || self.matcap_texture.is_some()
    }

    /// The anime character material this MToon material maps to (see the [module docs](self)).
    /// `bind` gives the texture asset standing for a use, if any: the import binds none, and
    /// extraction binds the textures (and, for [`TextureUse::ShadingShift`], the ILM document)
    /// it writes.
    pub fn to_material(&self, bind: impl Fn(TextureUse) -> Option<AssetId>) -> AnimeCharacterMaterial {
        let [r, g, b, _] = self.base_color;
        let shade_color = std::array::from_fn(|channel| {
            let base = [r, g, b][channel];
            let shade = self.shade_color[channel];
            if base > BLACK { shade / base } else { shade }
        });
        let brightest = |color: [f32; 3]| color.into_iter().fold(0.0f32, f32::max);
        let rim_strength = brightest(self.rim_color);
        let rim_color = if rim_strength > 0.0 { self.rim_color.map(|c| c / rim_strength) } else { [1.0; 3] };
        // MToon raises the falloff to max(power, ε); 0.5^(1/ε) underflows to 0, a full-width rim.
        let half = 0.5f32.powf(1.0 / self.rim_fresnel_power.max(1e-4));
        let rim_fresnel = (1.0 + self.rim_lift - half).clamp(0.0, 1.0);
        let matcap_strength = brightest(self.matcap_factor);
        let default_curve = OutlineCurve::default();
        let (outline_width, curve) = match self.outline_width {
            OutlineWidth::None => (0.0, default_curve),
            // Screen widths hold from the camera to the cutoff.
            OutlineWidth::Screen(fraction) => {
                let cutoff = default_curve.cutoff;
                let curve = OutlineCurve { near: (0.0, 1.0), far: (cutoff, 1.0), ..default_curve };
                (fraction * OUTLINE_REFERENCE_HEIGHT, curve)
            }
            // World widths hold everywhere: every knot at the distance the pixel width is
            // measured from, beyond which the curve keeps the width in meters.
            OutlineWidth::World(meters) => {
                let view_height =
                    2.0 * WORLD_OUTLINE_DISTANCE * (0.5 * WORLD_OUTLINE_FOV_DEGREES).to_radians().tan();
                let knot = (WORLD_OUTLINE_DISTANCE, 1.0);
                let curve =
                    OutlineCurve { near: knot, mid: WORLD_OUTLINE_DISTANCE, far: knot, ..default_curve };
                (meters / view_height * OUTLINE_REFERENCE_HEIGHT, curve)
            }
        };
        let outline_width = outline_width.max(0.0);
        let control_map = self.shading_shift_texture.and_then(|_| bind(TextureUse::ShadingShift));
        // Without its control map, the shading shift texture's mean offset applies everywhere:
        // 0.x's grade folds `−rate` into the shift for the texture to give back.
        let unbound_shift = match (self.shading_shift_texture, control_map) {
            (Some(shift), None) => shift.scale * shift.mean,
            _ => 0.0,
        };
        AnimeCharacterMaterial {
            base_color: self.base_color,
            base_color_map: self.base_color_texture.and_then(|_| bind(TextureUse::BaseColor)),
            alpha: self.alpha,
            control_map,
            normal_map: self.normal_texture.and_then(|_| bind(TextureUse::Normal)),
            light_split: LightSplit {
                threshold: -(self.shading_shift + unbound_shift),
                softness: 1.0 - self.shading_toony.clamp(0.0, 1.0),
            },
            diffuse: DiffuseMode::Bands(BandDiffuse { deep_threshold: None }),
            regions: vec![CharacterRegion {
                shade_color,
                rim_color,
                outline_color: self.outline_color,
                outline_width: 1.0,
                ..CharacterRegion::default()
            }],
            rim: Rim {
                strength: rim_strength,
                fresnel: rim_fresnel,
                light_gate: self.rim_lighting_mix.clamp(0.0, 1.0),
                ..Rim::default()
            },
            matcap: self.matcap_texture.and_then(|_| bind(TextureUse::Matcap)).map(|map| Matcap {
                map,
                strength: matcap_strength,
                shadow_strength: matcap_strength * (1.0 - self.rim_lighting_mix.clamp(0.0, 1.0)),
            }),
            outline: Outline {
                width: outline_width,
                curve,
                lighting: self.outline_lighting_mix.clamp(0.0, 1.0),
                ..Outline::default()
            },
            ..AnimeCharacterMaterial::default()
        }
    }
}
