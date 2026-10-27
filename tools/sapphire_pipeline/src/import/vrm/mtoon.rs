//! MToon → anime character materials.
//!
//! MToon (`VRMC_materials_mtoon` 1.0; VRM 0.x's `VRM/MToon` shader migrates to it, see
//! `Mtoon::from_v0`) shades in two tones:
//!
//! ```text
//! shading = linearstep(−1 + toony, 1 − toony, N·L + shadingShift + shiftTexture.r × scale)
//! color   = mix(shadeColor × shadeTexture, baseColor × baseTexture, shading) × light
//!           + GI, flattened toward the mean of the up and down GI by giEqualization
//! rim     = (matcap × matcapFactor + (1 − N·V + lift)^power × rimColor) × rimTexture
//!           × mix(1, light, rimLightingMix)
//! glow    = emissiveFactor × emissiveStrength × emissiveTexture
//! outline = outlineColor × mix(1, the surface's lit color, outlineLightingMix)
//! ```
//!
//! with an inverted-hull outline whose width is in meters or a fraction of the screen's
//! height, scaled by a texture's G channel, and texture coordinates that may scroll and turn.
//! That is Sapphire's band mode with two tones (the shadow side takes an authored color, and
//! the transition is a threshold on biased N·L) with M1 C7's layers on top.
//!
//! # The mapping
//!
//! Every MToon 1.0 parameter, and the glTF material properties MToon reads, onto
//! [`AnimeCharacterMaterial`] (A3, with C1–C7's fields) and the cooked mesh. VRM 0.x's
//! properties take the same paths through their 1.0 counterparts (`Mtoon::from_v0`). Spec
//! §5.27 has the same table.
//!
//! | MToon (1.0 name) | Sapphire | How |
//! |---|---|---|
//! | `baseColorFactor`, `baseColorTexture` (glTF) | `base_color`, `base_color_map` | As they are: linear RGBA, an sRGB color texture |
//! | `alphaMode`, `alphaCutoff` (glTF) | `alpha` | Opaque, `Mask { cutoff }` (clamped to 1), `Blend` |
//! | `normalTexture` (glTF) | `normal_map` | As it is, as a normal map; its `scale` is dropped |
//! | `shadeColorFactor` | `diffuse: Bands` with two tones, `regions[0].shade_color` | `shadeColor / baseColor` per channel, since the shade side is `albedo × shade_color`; a black base channel keeps the shade value |
//! | `shadeMultiplyTexture` | (the ratio above) | Exact when it is the base color's image, as exporters write it; otherwise approximated by the base texture. Extracted as a color texture either way |
//! | `shadingShiftFactor`, `shadingToonyFactor` | `light_split` | Exact: threshold `−shift`, softness `1 − toony`, the half-width of MToon's `linearstep` (Sapphire's step is a smoothstep between the same ends) |
//! | `shadingShiftTexture` (R) and its `scale` | `control_map`, G (shadow bias) | Extraction writes an ILM document whose G is the image's R mapped onto `[0.5, 0.5 + scale / 2]`: A7's encoding of an N·L offset of `r × scale` (`character_maps::ilm::encode_shadow_bias`). R, B and A stay neutral. Before extraction (the material the import generates), the texture's mean offset, measured on import, joins the threshold instead |
//! | `giEqualizationFactor` | none | Not mapped: Sapphire's stylized GI flattens per character, not per material (C2's rig leans every normal toward the camera; a region's sphere weight rounds faces and hair, from VRoid's names), so a material's factor has no field to go to |
//! | `matcapTexture` | `matcap.map`, `regions[0].matcap` | MToon's matcap is the region's default, the color matcap added in the key light's color (C7). Extracted as a color texture; a generated material has none |
//! | `matcapFactor` | `matcap.strength`, `regions[0].matcap.color` | The strength its brightest channel, the region's tint the color over it, × the rim multiply texture's hue or mean (below) |
//! | `rimLightingMixFactor` | `rim.light_gate`, `matcap.shadow_strength` | As it is: MToon multiplies its rim and matcap by the light by this much; Sapphire keeps the rim to the lit side by this much (M1 C5), and the matcap's shadow side takes `strength × (1 − mix)` |
//! | `parametricRimColorFactor` | `rim.strength`, `regions[0].rim_color` | The color × the rim multiply texture's mean color: its brightest channel the strength, the tint the color over it |
//! | `parametricRimFresnelPowerFactor`, `parametricRimLiftFactor` | `rim.fresnel` | The N·V where MToon's falloff is at half: `1 + lift − 0.5^(1 / power)`, clamped to `[0, 1]`, the depth-offset rim's Fresnel gate (M1 C5). Sapphire's rim is a band of its default width in pixels at the silhouette, cut where N·V passes it (the decision below) |
//! | `rimMultiplyTexture` | the layer mask's R; its mean color on the rim | Masks the matcap where the material's layer mask holds it (below). The depth-offset rim takes no texture, so the mean tints it. Read as sRGB, as MToon reads it |
//! | `outlineWidthMode`, `outlineWidthFactor` | `outline.width` (pixels at 1080p) and `outline.curve` | None → 0. Screen: `factor × 1080`, the factor being a fraction of the full screen height, kept from the camera out to the curve's cutoff (`near` at 0 m, `far` at the cutoff). World: the width in pixels from [`WORLD_OUTLINE_DISTANCE`] through a [`WORLD_OUTLINE_FOV_DEGREES`] field of view, with the curve's three knots at that distance, so the line keeps its width in meters at every distance and field of view (outlines v2, M1 C4) |
//! | `outlineWidthMultiplyTexture` (G) | vertex color G, the outline-width scale | Sampled at each vertex's UV and baked into the outline-width scale the mesh carries (the research note's mobile-friendly choice). Extracted as a mask too |
//! | `outlineColorFactor` | `regions[0].outline_color` | As it is |
//! | `outlineLightingMixFactor` | `outline.lighting`, `outline.albedo` | Both as it is: MToon multiplies the line's color by its surface's lit color (albedo × light) by this much; Sapphire lights the line as its surface (key light through the split, plus flattened GI) and takes its albedo, each by this much. Exact at 0 and 1 |
//! | `emissiveFactor` (glTF), `KHR_materials_emissive_strength` | `regions[0].emission`, `emission` | The factor × the strength (or VRM's older `VRMC_materials_hdr_emissiveMultiplier` where a file has only that), × the emissive texture's hue or mean (below): C7's glow, unlit, steady (`period` 0: MToon's doesn't breathe), blooming as the world's light |
//! | `emissiveTexture` (glTF) | the layer mask's B | Paints the glow where the material's layer mask holds it (below). Read as sRGB, as glTF reads it |
//! | `uvAnimationMaskTexture`, `uvAnimationScrollXSpeedFactor`, `uvAnimationScrollYSpeedFactor`, `uvAnimationRotationSpeedFactor` | none | Not mapped: the family's texture coordinates are the mesh's and never move (spec §5.12's later milestones). The mask is extracted |
//! | `transparentWithZWrite`, `renderQueueOffsetNumber` | none | Not mapped: no sort control; `Blend` materials draw in the transparent pass after the sky (C1), back to front once draws sort |
//! | `doubleSided` (glTF) | `double_sided` | As it is (M1 C8): back faces draw too, shaded with the flipped normal. VRM 0.x's `_CullMode` 0 (off) is its counterpart. Blended materials keep culling until transparent draws sort |
//! | `KHR_texture_transform`, `texCoord` (glTF) | none | Not mapped: meshes carry one UV set and materials no UV transform |
//!
//! Every texture the family can't bind is still extracted, so nothing authored is lost.
//!
//! # The layer mask
//!
//! MToon's rim multiply texture masks its matcap, and its emissive texture paints its glow: the
//! R and B of C7's layer mask (`character_maps::layer_mask`), each one channel where MToon's are
//! colors. [`Mtoon::layer_mask`] says what a material's holds: a body's (eyes bind their own
//! mask in that slot), the rim multiply texture where the material has a matcap and the emissive
//! texture where it glows; where the two differ in size, the emission alone. The import measures
//! each such texture ([`LayerTexture`]): the mask takes its brightest channel by mean, decoded
//! from sRGB, and the layer's color the rest of its hue (`mean / mean[channel]`), so a texture
//! of one hue, gray (as VRoid Studio writes its hair's emission) or tinted, maps exactly, and a
//! many-colored one keeps its brightest channel's shape. Extraction writes the mask as a layer
//! mask document over the textures (`character_maps::layer_mask`). Where no layer mask is bound
//! (the generated material, an eye), a texture's mean color multiplies its layer's color
//! instead, as the shading shift texture's mean offset joins a generated material's threshold.
//!
//! # The parametric rim
//!
//! **Decision: MToon's rim stays on the depth-offset rim, approximately** (M1 C5's mapping).
//! MToon's is a Fresnel glow over the whole surface, `(1 − N·V + lift)^power`, whose width on
//! screen follows the surface's curvature: it thins on a close-up and swamps a distant
//! character. C5 replaced exactly that with a rim of a fixed width in pixels at the silhouette,
//! as the reference games draw theirs, so the import keeps MToon's color, its lighting mix and
//! where its falloff is at half, and lets the rim's own width and depth step place it. C7's
//! layers don't carry it faithfully either: a matcap is sampled by the view-space normal and so
//! could hold the Fresnel falloff (MToon adds its matcap and rim together), but that brings back
//! the N·V rim C5 removed, needs a texture a generated material can't bind, and takes the one
//! matcap slot, which MToon's own matcap fills.
//!
//! # Surfaces and GI
//!
//! `surface` stays `Body`, but for VRoid Studio's eye materials: MToon doesn't say which
//! material is a face, hair or eyes, and faces and hair need maps (SDF face maps, hair masks)
//! that an artist authors on the extracted document.
//!
//! Eyes need no map, and VRoid Studio names their materials, so the import maps those onto anime
//! eyes' layers (M1 C6, `EyeShading`; [`Mtoon::eye`], from the names by
//! [`super::vroid_eye_layer`]): `EyeWhite`, `FaceEyeline`, `FaceEyelash` and `FaceBrow` are the
//! eye itself (blending ones alpha-tested instead); `EyeIris` the iris and `EyeHighlight` the
//! highlights, both blended over the eye within its depth mask, the iris behind the cornea over
//! the ellipse its texture coordinates span, the highlights drifting and glinting at the
//! defaults. All draw through the bangs at half, and without an outline.
//!
//! The character rig's stylized GI (M1 C2) takes its region weights (`regions[0].gi`,
//! [`RegionGi`]) from VRoid Studio's names too ([`Mtoon::gi`], by [`super::vroid_region_gi`],
//! from the group a name ends in): hair (`HAIR`) turns its GI toward the head's sphere
//! (`sphere 1, skin 0`), the face's skin (`Face_00_SKIN`) takes the sphere and the skin tint
//! (`1, 1`), the body's skin (other `SKIN` parts) the skin tint (`0, 1`). Everything else keeps
//! `(0, 0)`, the GI its normals give it: clothes, eyes, the brows, lash line and the mouth's
//! inside (VRoid's `FACE` group), and every material a VRoid name doesn't place. VRM 1.0 has no
//! material-role field, so the name is the only signal; the weights are the extracted
//! document's to change. The face and hair still draw as bodies (`surface` above), but their GI
//! is already a face's and hair's.
//!
//! A material with `KHR_materials_unlit` (or a VRM 0.x unlit shader) maps as an MToon whose
//! shade side is its lit side, without rim, outline or glow (`Mtoon::unlit`).

use sapphire_assets::AssetId;
use sapphire_assets::character_maps::ilm;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, BandDiffuse, CharacterRegion, CharacterSurface, DiffuseMode, Emission,
    EyeShading, LightSplit, Matcap, Outline, OutlineCurve, RegionGi, RegionMatcap, Rim,
};
use serde::Deserialize;
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

/// The glTF extension scaling `emissiveFactor` past 1.
const EMISSIVE_STRENGTH: &str = "KHR_materials_emissive_strength";

/// VRM's earlier extension for the same, which some VRM 1.0 exporters wrote.
const HDR_EMISSIVE_MULTIPLIER: &str = "VRMC_materials_hdr_emissiveMultiplier";

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

/// A color texture a channel of the layer mask can take (M1 C7; see the
/// [module docs](self#the-layer-mask)): MToon's rim multiply texture, which masks its matcap, or
/// its emissive texture, which paints its glow. The import measures it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerTexture {
    /// The texture, as color.
    pub texture: TextureRef,
    /// Its brightest channel by mean, which the layer mask takes: R, G or B.
    pub channel: Channel,
    /// Its mean color, linear: decoded from sRGB.
    pub mean: [f32; 3],
    /// Its size in texels: a layer mask's inputs share one.
    pub size: [u32; 2],
}

impl LayerTexture {
    /// What multiplies the layer's color where the layer mask holds the texture's channel: its
    /// mean color over that channel's mean, the rest of its hue. White for a black texture.
    pub fn hue(&self) -> [f32; 3] {
        let value = match self.channel {
            Channel::G => self.mean[1],
            Channel::B => self.mean[2],
            Channel::R | Channel::Rgba => self.mean[0],
        };
        if value > 0.0 { self.mean.map(|c| c / value) } else { [1.0; 3] }
    }
}

/// What a material's layer mask holds (M1 C7; [`Mtoon::layer_mask`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerMask {
    /// R: the rim multiply texture, masking the matcap.
    pub matcap: Option<LayerTexture>,
    /// B: the emissive texture, painting the glow.
    pub emission: Option<LayerTexture>,
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
    /// Kept as a texture where the layer mask doesn't hold it.
    RimMultiply,
    OutlineWidth,
    UvAnimationMask,
    /// Kept as a texture where the layer mask doesn't hold it.
    Emissive,
}

impl TextureUse {
    /// The texture class an image extracted for this use gets, or `None` for an ILM input.
    pub fn class(self) -> Option<TextureClass> {
        Some(match self {
            // The rim multiply texture multiplies color, so MToon reads it as sRGB.
            Self::BaseColor | Self::Shade | Self::Matcap | Self::RimMultiply | Self::Emissive => {
                TextureClass::Color(ColorSettings::default())
            }
            Self::Normal => TextureClass::Normal(NormalSettings::default()),
            Self::OutlineWidth | Self::UvAnimationMask => TextureClass::Mask(MaskSettings::default()),
            Self::ShadingShift => return None,
        })
    }
}

/// What a material document binds, which [`Mtoon::to_material`] asks for: a texture as it is,
/// or a map generated from textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    /// A texture for a use with a class ([`TextureUse::class`]).
    Texture(TextureUse),
    /// The control map: an ILM document over the shading shift texture.
    ControlMap,
    /// The layer mask (M1 C7): a layer mask document over the textures [`Mtoon::layer_mask`]
    /// names.
    LayerMask,
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
    pub rim_multiply_texture: Option<LayerTexture>,
    pub outline_width: OutlineWidth,
    pub outline_width_multiply_texture: Option<TextureRef>,
    pub outline_color: [f32; 3],
    pub outline_lighting_mix: f32,
    /// The glow: `emissiveFactor` × its strength, linear.
    pub emissive: [f32; 3],
    pub emissive_texture: Option<LayerTexture>,
    pub uv_animation_mask_texture: Option<TextureRef>,
    /// The eye layer the material draws (M1 C6): set on import from VRoid Studio's name for it
    /// ([`super::vroid_eye_layer`]), none for anything else.
    pub eye: Option<EyeShading>,
    /// The region's share of the character rig's stylized GI (M1 C2): set on import from VRoid
    /// Studio's name for the material ([`super::vroid_region_gi`]), none (0, 0) for anything
    /// else.
    pub gi: RegionGi,
    /// Whether both faces draw (glTF's `doubleSided`; VRM 0.x's `_CullMode` off; M1 C8).
    pub double_sided: bool,
}

/// Where a material's textures are checked against the document, and its shading shift and
/// layer textures measured.
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

    /// The color texture `texture` as a layer mask's input, measured: its mean color and its
    /// brightest channel by it (R on ties).
    fn layer_texture(&self, property: &str, texture: usize) -> Result<LayerTexture, VrmImportError> {
        let texture = self.reference(property, texture, Channel::Rgba)?;
        let (mean, size) = self.images.mean_color(self.images.image_of(texture.texture))?;
        let brightest = (1..3).fold(0, |best, c| if mean[c] > mean[best] { c } else { best });
        let channel = [Channel::R, Channel::G, Channel::B][brightest];
        Ok(LayerTexture { texture, channel, mean, size })
    }
}

/// The MToon defaults with a glTF material's core properties, which VRM 1.0's MToon keeps
/// there: base color, alpha and the normal texture. Emission is MToon's alone
/// ([`Mtoon::from_v1`]).
fn core(material: &gltf::Material<'_>) -> Mtoon {
    let pbr = material.pbr_metallic_roughness();
    Mtoon {
        base_color: pbr.base_color_factor(),
        base_color_texture: pbr
            .base_color_texture()
            .map(|info| TextureRef { texture: info.texture().index(), channel: Channel::Rgba }),
        alpha: crate::import::gltf::alpha_mode(material),
        normal_texture: material
            .normal_texture()
            .map(|normal| TextureRef { texture: normal.texture().index(), channel: Channel::Rgba }),
        double_sided: material.double_sided(),
        ..Mtoon::UNSHADED
    }
}

/// `KHR_materials_emissive_strength`'s extension object.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmissiveStrength {
    emissive_strength: Option<f32>,
}

/// `VRMC_materials_hdr_emissiveMultiplier`'s extension object.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HdrEmissiveMultiplier {
    emissive_multiplier: Option<f32>,
}

/// What a material's `emissiveFactor` is multiplied by: `KHR_materials_emissive_strength`'s
/// strength, else `VRMC_materials_hdr_emissiveMultiplier`'s multiplier, else 1. Negative values
/// are 0.
fn emissive_strength(material: &gltf::Material<'_>) -> Result<f32, VrmImportError> {
    fn read<T: serde::de::DeserializeOwned>(
        material: &gltf::Material<'_>,
        extension: &'static str,
    ) -> Result<Option<T>, VrmImportError> {
        material
            .extension_value(extension)
            .map(|value| {
                T::deserialize(value).map_err(|source| VrmImportError::Extension { extension, source })
            })
            .transpose()
    }
    let strength = match read::<EmissiveStrength>(material, EMISSIVE_STRENGTH)? {
        Some(strength) => strength.emissive_strength,
        None => read::<HdrEmissiveMultiplier>(material, HDR_EMISSIVE_MULTIPLIER)?
            .and_then(|multiplier| multiplier.emissive_multiplier),
    };
    Ok(strength.unwrap_or(1.0).max(0.0))
}

/// `a × b`, per channel.
fn times(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| a[c] * b[c])
}

/// The brightest channel of `color`, and the color over it (white for black).
fn strength_and_tint(color: [f32; 3]) -> (f32, [f32; 3]) {
    let strength = color.into_iter().fold(0.0f32, f32::max);
    let tint = if strength > 0.0 { color.map(|c| c / strength) } else { [1.0; 3] };
    (strength, tint)
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
        emissive: [0.0; 3],
        emissive_texture: None,
        uv_animation_mask_texture: None,
        eye: None,
        gi: RegionGi { sphere: 0.0, skin: 0.0 },
        double_sided: false,
    };

    /// A VRM 1.0 MToon material: the glTF material's core properties and emission, and its
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
        let layer = |property: &str, info: &Option<gltf::json::texture::Info>| {
            info.as_ref().map(|info| check.layer_texture(property, info.index.value())).transpose()
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
        mtoon.rim_multiply_texture = layer("rimMultiplyTexture", &schema.rim_multiply_texture)?;
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
        let strength = emissive_strength(material)?;
        mtoon.emissive = material.emissive_factor().map(|c| c.max(0.0) * strength);
        mtoon.emissive_texture = material
            .emissive_texture()
            .map(|info| check.layer_texture("emissiveTexture", info.texture().index()))
            .transpose()?;
        mtoon.uv_animation_mask_texture =
            texture("uvAnimationMaskTexture", &schema.uv_animation_mask_texture, Channel::B)?;
        Ok(mtoon)
    }

    /// An unlit material (`KHR_materials_unlit`, or a VRM 0.x unlit shader): its shade side is
    /// its lit side, with no rim, no outline and no glow (unlit shades its base color alone).
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
    /// - **Colors** (`_Color`, `_ShadeColor`, `_RimColor`, `_OutlineColor`, `_EmissionColor`)
    ///   are Unity's sRGB values: decoded to linear, alpha kept.
    /// - **Shading:** 0.x maps N·L linearly from `_ShadeShift` up to
    ///   `lerp(1, _ShadeShift, _ShadeToony)`. The same ramp in 1.0's terms is
    ///   `toony = lerp(_ShadeToony, 1, ½ + ½ _ShadeShift)` and `shift = −_ShadeShift − (1 − toony)`.
    /// - **Shading grade** (`_ShadingGradeTexture`, R): 0.x scales half-Lambert by
    ///   `1 − _ShadingGradeRate × (1 − r)`, which moves the split by `−rate × (1 − r)` near it.
    ///   That becomes a shading shift texture with scale `rate`, and `rate` off the shift.
    /// - **Rim:** `_RimFresnelPower`, `_RimLift`, `_RimLightingMix`, `_RimTexture` (sRGB, as
    ///   1.0's rim multiply texture); the matcap is `_SphereAdd` at full strength.
    /// - **Outline:** `_OutlineWidthMode` 1 (world) and 2 (screen). `_OutlineWidth` is
    ///   centimeters in world mode, and in screen mode a percentage of half the screen's height
    ///   (MToon 0.x offsets clip space by `0.01 × width`): ×0.01 and ×0.005. The width texture
    ///   is read from its R channel, as 0.x reads it. `_OutlineColorMode` 1 (mixed lighting)
    ///   takes `_OutlineLightingMix` as 1.0's lighting mix; 0 (a fixed color, the default) draws
    ///   the line unlit, a lighting mix of 0.
    /// - **Emission:** `_EmissionColor` and `_EmissionMap`.
    /// - **Alpha:** `_BlendMode` 0 opaque, 1 cutout at `_Cutoff`, 2 and 3 (with Z write) blend.
    /// - **Textures:** `_MainTex`, `_ShadeTexture`, `_BumpMap` and `_UvAnimMaskTexture` (R).
    ///   `_MainTex`'s scale and offset, `_ReceiveShadow*`, `_LightColorAttenuation`,
    ///   `_IndirectLightIntensity` (1.0's GI equalization is `1 −` it, which isn't mapped either),
    ///   `_OutlineScaledMaxDistance` and the UV animation speeds (`_UvAnimScrollX`,
    ///   `_UvAnimScrollY`, `_UvAnimRotation`) are dropped, as their 1.0 counterparts are.
    /// - **Sides:** `_CullMode` 0 (off) draws both faces, as glTF's `doubleSided` (M1 C8); 1
    ///   (front) has no counterpart and culls back faces, as 2 (back) does.
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
        let index = |name: &str| properties.texture_properties.as_ref().and_then(|map| map.get(name));
        let texture = |name: &str, channel: Channel| {
            index(name).map(|index| check.reference(name, index.value(), channel)).transpose()
        };
        let layer =
            |name: &str| index(name).map(|index| check.layer_texture(name, index.value())).transpose();

        let toony0 = float("_ShadeToony", 0.9);
        let shift0 = float("_ShadeShift", 0.0);
        let toony = toony0 + (1.0 - toony0) * (0.5 + 0.5 * shift0);
        let mut shift = -shift0 - (1.0 - toony);
        let shading_shift_texture = match index("_ShadingGradeTexture") {
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
        let outline_lighting_mix = match float("_OutlineColorMode", 0.0) as i32 {
            1 => float("_OutlineLightingMix", 1.0),
            _ => 0.0,
        };
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
            matcap_texture: texture("_SphereAdd", Channel::Rgba)?,
            rim_color: rgb("_RimColor", [0.0, 0.0, 0.0, 1.0]),
            rim_fresnel_power: float("_RimFresnelPower", 1.0),
            rim_lift: float("_RimLift", 0.0),
            rim_lighting_mix: float("_RimLightingMix", 0.0),
            rim_multiply_texture: layer("_RimTexture")?,
            outline_width,
            outline_width_multiply_texture: texture("_OutlineWidthTexture", Channel::R)?,
            outline_color: rgb("_OutlineColor", [0.0, 0.0, 0.0, 1.0]),
            outline_lighting_mix,
            emissive: rgb("_EmissionColor", [0.0, 0.0, 0.0, 1.0]).map(|c| c.max(0.0)),
            emissive_texture: layer("_EmissionMap")?,
            uv_animation_mask_texture: texture("_UvAnimMaskTexture", Channel::R)?,
            eye: None,
            gi: Self::UNSHADED.gi,
            double_sided: float("_CullMode", 2.0) as i32 == 0,
        })
    }

    /// Whether it glows: an emissive factor (× its strength) that isn't black.
    pub fn glows(&self) -> bool {
        self.emissive.iter().any(|&c| c > 0.0)
    }

    /// What its layer mask holds (M1 C7; see the [module docs](self#the-layer-mask)), if it has
    /// one: a body's, from the rim multiply texture where it has a matcap (R) and the emissive
    /// texture where it glows (B); where the two differ in size, the emission alone, the mask's
    /// inputs sharing one size. Eyes bind their own mask in its slot, so they have none.
    pub fn layer_mask(&self) -> Option<LayerMask> {
        if self.eye.is_some() {
            return None;
        }
        let emission = self.emissive_texture.filter(|_| self.glows());
        let matcap = self
            .rim_multiply_texture
            .filter(|rim| self.matcap_texture.is_some() && emission.is_none_or(|glow| glow.size == rim.size));
        (matcap.is_some() || emission.is_some()).then_some(LayerMask { matcap, emission })
    }

    /// Every texture the material uses, with what for, in [`TextureUse`] order, but those its
    /// layer mask holds ([`Self::layer_mask`]), which are that map's inputs.
    pub fn textures(&self) -> Vec<(TextureUse, TextureRef)> {
        let mask = self.layer_mask();
        let outside = |texture: Option<LayerTexture>, held: Option<LayerTexture>| {
            texture.filter(|_| held.is_none()).map(|texture| texture.texture)
        };
        [
            (TextureUse::BaseColor, self.base_color_texture),
            (TextureUse::Shade, self.shade_multiply_texture),
            (TextureUse::Normal, self.normal_texture),
            (TextureUse::ShadingShift, self.shading_shift_texture.map(|shift| shift.texture)),
            (TextureUse::Matcap, self.matcap_texture),
            (TextureUse::RimMultiply, outside(self.rim_multiply_texture, mask.and_then(|m| m.matcap))),
            (TextureUse::OutlineWidth, self.outline_width_multiply_texture),
            (TextureUse::UvAnimationMask, self.uv_animation_mask_texture),
            (TextureUse::Emissive, outside(self.emissive_texture, mask.and_then(|m| m.emission))),
        ]
        .into_iter()
        .filter_map(|(usage, texture)| Some((usage, texture?)))
        .collect()
    }

    /// Whether the material's document binds any texture: a base color, normal or matcap
    /// texture, a control map from the shading shift texture, or a layer mask (M1 C7).
    /// Materials that don't draw the same from the material the importer generates.
    pub fn is_textured(&self) -> bool {
        self.base_color_texture.is_some()
            || self.normal_texture.is_some()
            || self.shading_shift_texture.is_some()
            || self.matcap_texture.is_some()
            || self.layer_mask().is_some()
    }

    /// The anime character material this MToon material maps to (see the [module docs](self)).
    /// `bind` gives the asset standing for a binding, if any: the import binds none, and
    /// extraction binds the textures, the ILM document and the layer mask document it writes.
    pub fn to_material(&self, bind: impl Fn(Binding) -> Option<AssetId>) -> AnimeCharacterMaterial {
        let texture = |usage| bind(Binding::Texture(usage));
        let [r, g, b, _] = self.base_color;
        let shade_color = std::array::from_fn(|channel| {
            let base = [r, g, b][channel];
            let shade = self.shade_color[channel];
            if base > BLACK { shade / base } else { shade }
        });
        // The rim multiply texture's mean color: the depth-offset rim takes no texture.
        let rim_mean = self.rim_multiply_texture.map_or([1.0; 3], |rim| rim.mean);
        let (rim_strength, rim_color) = strength_and_tint(times(self.rim_color, rim_mean));
        // MToon raises the falloff to max(power, ε); 0.5^(1/ε) underflows to 0, a full-width rim.
        let half = 0.5f32.powf(1.0 / self.rim_fresnel_power.max(1e-4));
        let rim_fresnel = (1.0 + self.rim_lift - half).clamp(0.0, 1.0);
        let (matcap_strength, matcap_color) = strength_and_tint(self.matcap_factor);
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
        let control_map = self.shading_shift_texture.and_then(|_| bind(Binding::ControlMap));
        // Without its control map, the shading shift texture's mean offset applies everywhere:
        // 0.x's grade folds `−rate` into the shift for the texture to give back.
        let unbound_shift = match (self.shading_shift_texture, control_map) {
            (Some(shift), None) => shift.scale * shift.mean,
            _ => 0.0,
        };
        // A texture that paints a layer multiplies the layer's color by the rest of its hue
        // where the bound layer mask holds its channel, and by its mean color where it doesn't.
        let mask = self.layer_mask();
        let layer_mask = mask.and_then(|_| bind(Binding::LayerMask));
        let painted = |texture: Option<LayerTexture>, held: Option<LayerTexture>| match texture {
            Some(texture) if layer_mask.is_some() && held.is_some() => texture.hue(),
            Some(texture) => texture.mean,
            None => [1.0; 3],
        };
        let lighting_mix = self.outline_lighting_mix.clamp(0.0, 1.0);
        let matcap = self.matcap_texture.and_then(|_| texture(TextureUse::Matcap)).map(|map| Matcap {
            map,
            strength: matcap_strength,
            shadow_strength: matcap_strength * (1.0 - self.rim_lighting_mix.clamp(0.0, 1.0)),
        });
        let mut region = CharacterRegion {
            shade_color,
            rim_color,
            outline_color: self.outline_color,
            outline_width: 1.0,
            gi: self.gi,
            ..CharacterRegion::default()
        };
        if matcap.is_some() {
            let tint = painted(self.rim_multiply_texture, mask.and_then(|m| m.matcap));
            region.matcap = RegionMatcap { color: times(matcap_color, tint), ..RegionMatcap::default() };
        }
        if self.glows() {
            let tint = painted(self.emissive_texture, mask.and_then(|m| m.emission));
            region.emission = times(self.emissive, tint);
        }
        let mut material = AnimeCharacterMaterial {
            base_color: self.base_color,
            base_color_map: self.base_color_texture.and_then(|_| texture(TextureUse::BaseColor)),
            alpha: self.alpha,
            control_map,
            normal_map: self.normal_texture.and_then(|_| texture(TextureUse::Normal)),
            light_split: LightSplit {
                threshold: -(self.shading_shift + unbound_shift),
                softness: 1.0 - self.shading_toony.clamp(0.0, 1.0),
            },
            diffuse: DiffuseMode::Bands(BandDiffuse { deep_threshold: None }),
            regions: vec![region],
            rim: Rim {
                strength: rim_strength,
                fresnel: rim_fresnel,
                light_gate: self.rim_lighting_mix.clamp(0.0, 1.0),
                ..Rim::default()
            },
            matcap,
            outline: Outline {
                width: outline_width,
                curve,
                albedo: lighting_mix,
                lighting: lighting_mix,
                ..Outline::default()
            },
            layer_mask,
            double_sided: self.double_sided,
            ..AnimeCharacterMaterial::default()
        };
        // MToon's glow is steady.
        if self.glows() {
            material.emission = Emission { period: 0.0, ..Emission::default() };
        }
        // An eye's layer (M1 C6): those over the eye blend, the eye itself doesn't, and eyes
        // draw no outline.
        if let Some(eye) = self.eye {
            material.surface = CharacterSurface::Eye(eye);
            material.alpha = match (eye.layer.is_overlay(), self.alpha) {
                (true, _) => AlphaMode::Blend,
                (false, AlphaMode::Blend) => AlphaMode::Mask { cutoff: 0.5 },
                (false, alpha) => alpha,
            };
            material.outline.width = 0.0;
        }
        material
    }
}
