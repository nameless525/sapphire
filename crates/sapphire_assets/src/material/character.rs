//! The anime character family: characters with authored light response (spec §5.12, M1 plan
//! pillar 2). The anime character shader (M1 C1's uber-shader) reads its block: ramp and band
//! diffuse, the control map's regions and bias, each region's share of the character light
//! rig's stylized GI (M1 C2), the depth-offset rim and its region mask, hair highlights, the
//! bangs' shadow on a face and eyes through the bangs (M1 C5), and the static permutations of
//! its flags.
//!
//! # Versions
//!
//! 2 (M1 C5): the rim became the depth-offset rim. Its `width` is now pixels at a 1080-row
//! target, the depth offset; what was the width, the share of N·V in from the silhouette, is
//! `fresnel`. Version 1 documents load through [`v1::AnimeCharacterMaterial`], their rim width
//! becoming the Fresnel extent and the new settings taking their defaults.

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_reflect::{Decoder, VersionError, Versioned, decode_versioned};
use sapphire_serialize::ReflectDocument;
use serde::{Deserialize, Serialize};

use super::common::{
    AlphaMode, GpuBlock, bound_bits, color, finite, invalid, non_negative, pack_swizzles, rgb_w, unit,
};
use super::{MaterialDocument, MaterialError, MaterialFamily, MaterialFlags, TextureSlot, TextureUsage};
use crate::AssetId;
use crate::character_maps::ramp::{self, Half};
use crate::character_maps::{MATERIAL_IDS, MaterialId};
use crate::texture::Swizzle;

/// An anime character material, as a document:
///
/// ```ron
/// (
///     type: "sapphire::render::AnimeCharacterMaterial",
///     version: 2,
///     data: (
///         base_color: (1.0, 0.75, 0.62, 1.0),
///         base_color_map: "7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d",
///         control_map: "0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b",
///         light_split: (threshold: 0.1, softness: 0.03),
///         diffuse: Bands((deep_threshold: -0.4)),
///         regions: [
///             (shade_color: (0.84, 0.4, 0.39), outline_color: (0.15, 0.03, 0.03)),
///             (shade_color: (0.6, 0.55, 0.85), outline_color: (0.02, 0.02, 0.05)),
///         ],
///         rim: (width: 3.5, strength: 0.25),
///         outline: (width: 4.1),
///     ),
/// )
/// ```
///
/// Everything left out takes its default, so new fields join without a schema version.
///
/// - **Light split:** N·L, offset by the control map's shadow bias, is thresholded into a lit
///   and a shadow side. The shadow side takes an authored color, never a darkened albedo.
/// - **Diffuse:** [`DiffuseMode::Bands`] (ZZZ: two or three analytic bands with per-region
///   colors, no ramp, cheapest on mobile) or [`DiffuseMode::Ramp`] (Genshin, Star Rail, WuWa: a
///   ramp texture row per region, a day and a night row, mixed with a subsurface tint).
/// - **Regions:** up to [`AnimeCharacterParams::MAX_REGIONS`] sets of per-region parameters,
///   picked per pixel by the control map's alpha (its material ID), or without a control map
///   by the vertex's material ID (`sapphire_assets::mesh::VertexFlags`).
/// - **Surface:** body, face, hair or eye. Faces, hair and eyes are static permutation bits of
///   the anime character shader, each with settings of its own.
/// - **Rim:** the depth-offset rim ([`Rim`]), which each region scales, widens and masks.
///
/// Colors are linear RGB(A), as glTF factors are. Textures are referenced by [`AssetId`]; see
/// [`AnimeCharacterMaterial::TEXTURE_SLOTS`] for what each must hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[reflect(Document)]
#[serde(default)]
pub struct AnimeCharacterMaterial {
    pub surface: CharacterSurface,
    /// Lit albedo, linear RGBA. Multiplies `base_color_map`.
    pub base_color: [f32; 4],
    /// RGB albedo with painted gradients and AO; A is coverage for `alpha`.
    pub base_color_map: Option<AssetId>,
    pub alpha: AlphaMode,
    /// The control (ILM) map, in A7's channel layout (`character_maps`): R specular mask,
    /// G shadow bias (0.5 neutral, at least 0.95 always lit, at most 0.05 always shadowed),
    /// B specular threshold, A material ID, which picks the region.
    pub control_map: Option<AssetId>,
    /// Tangent-space normal map (X, Y).
    pub normal_map: Option<AssetId>,
    pub light_split: LightSplit,
    pub diffuse: DiffuseMode,
    /// Per-region parameters; at least one, at most [`AnimeCharacterParams::MAX_REGIONS`].
    pub regions: Vec<CharacterRegion>,
    pub rim: Rim,
    pub specular: Specular,
    /// A spheremap for metals and gloss. Which control map channel gates it is C7's to settle:
    /// A7's layout has no channel of its own for it (R is the specular mask, B its threshold).
    pub matcap: Option<Matcap>,
    pub outline: Outline,
}

impl Versioned for AnimeCharacterMaterial {
    /// 2: the depth-offset rim (M1 C5; see the [module docs](self)).
    const VERSION: u32 = 2;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        decode_versioned::<v1::AnimeCharacterMaterial, D>(version, data).map(Into::into)
    }
}

impl Default for AnimeCharacterMaterial {
    fn default() -> Self {
        Self {
            surface: CharacterSurface::default(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            base_color_map: None,
            alpha: AlphaMode::default(),
            control_map: None,
            normal_map: None,
            light_split: LightSplit::default(),
            diffuse: DiffuseMode::default(),
            regions: vec![CharacterRegion::default()],
            rim: Rim::default(),
            specular: Specular::default(),
            matcap: None,
            outline: Outline::default(),
        }
    }
}

/// Which part of a character a material shades.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum CharacterSurface {
    #[default]
    Body,
    /// Face shadows from an SDF threshold map rather than N·L (C3).
    Face(FaceShading),
    /// Hair highlights (C5).
    Hair(HairShading),
    /// Layered eyes drawn through the bangs (C5, C6).
    Eye(EyeShading),
}

/// Face settings (M1 C3). A face's light split comes from its SDF map and the light in head
/// space rather than from N·L, then takes the material's diffuse mode as the body does, so a
/// face's shadow tones are the body's. What the maps hold is `character_maps`' (face SDF, face
/// mask and expression layouts).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct FaceShading {
    /// The SDF face-shadow map (A7's generator): R the threshold for horizontal light angles,
    /// G for vertical ones (WuWa's top light). Authored for light from one side and mirrored
    /// for the other.
    pub shadow_map: AssetId,
    /// The face mask, linear: R nose line, G face highlight, B always lit (eyes, brows).
    #[serde(default)]
    pub mask_map: Option<AssetId>,
    /// The expression map, linear: R blush, G gloom, each scaled by the character's expression
    /// weights.
    #[serde(default)]
    pub expression_map: Option<AssetId>,
    /// Half-width of the shadow edge, in threshold units (light angle / 180°), where it is
    /// wider than the one-pixel edge every split gets.
    #[serde(default = "FaceShading::default_softness")]
    pub softness: f32,
    /// The nose line shows where its mask × the sine of the camera's turn from head-forward
    /// exceeds this: a fully painted line appears once the head turns `asin` of it from the
    /// camera (24° at the default). Above 1 hides it.
    #[serde(default = "FaceShading::default_nose_line")]
    pub nose_line: f32,
    /// How much the face highlight brightens the lit tone at full mask.
    #[serde(default = "FaceShading::default_highlight")]
    pub highlight: f32,
    /// How far into the lit side, in threshold units, the face highlight takes to fade in from
    /// the shadow's edge.
    #[serde(default = "FaceShading::default_highlight_reveal")]
    pub highlight_reveal: f32,
    /// Linear RGB the albedo is multiplied toward at full blush.
    #[serde(default = "FaceShading::default_blush_color")]
    pub blush_color: [f32; 3],
    /// The bangs' shadow on the face (M1 C5).
    #[serde(default)]
    pub hair_shadow: HairShadow,
    /// Draws the mask's always-lit parts (B: eyes, brows) through the character's hair (M1 C5),
    /// which needs `mask_map`.
    #[serde(default)]
    pub through_hair: Option<ThroughHair>,
}

impl FaceShading {
    /// A face shaded by `shadow_map`, with every other setting at its default.
    pub fn new(shadow_map: AssetId) -> Self {
        Self {
            shadow_map,
            mask_map: None,
            expression_map: None,
            softness: Self::default_softness(),
            nose_line: Self::default_nose_line(),
            highlight: Self::default_highlight(),
            highlight_reveal: Self::default_highlight_reveal(),
            blush_color: Self::default_blush_color(),
            hair_shadow: HairShadow::default(),
            through_hair: None,
        }
    }

    fn default_softness() -> f32 {
        0.02
    }

    fn default_nose_line() -> f32 {
        0.4
    }

    fn default_highlight() -> f32 {
        0.3
    }

    fn default_highlight_reveal() -> f32 {
        0.15
    }

    fn default_blush_color() -> [f32; 3] {
        [1.0, 0.55, 0.58]
    }
}

/// The bangs' shadow on a face (M1 C5; spec §5.12 "Anime character shading"): the face darkens
/// into its own shadow tone (its split and diffuse colors, as an SDF shadow does) where the
/// character's hair lies a short way toward the key light from it across the screen, in front
/// of it. The renderer finds that hair in the character prepass, so it costs a face two texture
/// gathers and no extra draw.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct HairShadow {
    /// How far the shadow reaches below the hair, in meters across the face, with the key light
    /// across the view. Light from in front of the face or behind it shortens it, as a real
    /// shadow looks shorter from the light's side. 0 turns it off.
    pub distance: f32,
    /// How dark it is: 1 the face's full shadow tone.
    pub strength: f32,
}

impl Default for HairShadow {
    fn default() -> Self {
        Self { distance: 0.03, strength: 1.0 }
    }
}

/// Eyes and brows drawn through the bangs (M1 C5; spec §5.12 "Anime character shading"): where
/// the character's own hair covers them, they show through it, at an opacity that fades as the
/// head turns away from the camera, so a character's eyes stay readable under the bangs
/// (StarRailNPR's eyes-through-hair pass). A face draws its mask's always-lit parts through, an
/// eye material all of itself.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct ThroughHair {
    /// How much of the eye shows through the hair with the head facing the camera: 0 to 1.
    pub opacity: f32,
    /// The camera's angle from head-forward around the head's up axis, in degrees, by which the
    /// eyes have faded behind the hair: above 0, at most 180.
    pub fade_yaw: f32,
    /// The same above or below head-forward.
    pub fade_pitch: f32,
    /// How far behind the hair, in meters, an eye may lie and still show through it, so hair
    /// well in front of the face covers it.
    pub distance: f32,
}

impl Default for ThroughHair {
    fn default() -> Self {
        Self { opacity: 0.5, fade_yaw: 70.0, fade_pitch: 45.0, distance: 0.15 }
    }
}

impl ThroughHair {
    fn validate(&self, field: &str) -> Result<(), MaterialError> {
        unit(&format!("{field}.opacity"), self.opacity)?;
        for (name, angle) in [("fade_yaw", self.fade_yaw), ("fade_pitch", self.fade_pitch)] {
            if !(angle > 0.0 && angle <= 180.0) {
                return Err(invalid(&format!("{field}.{name}"), "must be above 0° and at most 180°"));
            }
        }
        non_negative(&format!("{field}.distance"), self.distance)
    }

    /// The GPU block's `through_hair` vector: opacity, the fade angles' cosines, distance; all
    /// zero for none.
    fn gpu(through: Option<&Self>) -> [f32; 4] {
        through.map_or([0.0; 4], |t| {
            [t.opacity, t.fade_yaw.to_radians().cos(), t.fade_pitch.to_radians().cos(), t.distance]
        })
    }
}

/// Hair settings (M1 C5).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct HairShading {
    /// Linear RGB: R highlight intensity, G highlight shift (0.5 none, in the highlight's own
    /// units), B occlusion (WuWa's `_HM`; `character_maps::hair_mask`).
    pub mask_map: Option<AssetId>,
    pub highlight: HairHighlight,
}

/// The hair highlight: a band of the key light's color over the hair, revealed by the mask's R
/// and the style's geometry, edged by the material's specular softness, and dimmed on the
/// shadow side.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct HairHighlight {
    pub style: HighlightStyle,
    /// Linear RGB.
    pub color: [f32; 3],
    /// 0 turns the highlight off.
    pub strength: f32,
    /// Width of the band: in N·H for a painted band, in the view-space normal's height for an
    /// angel ring, in sin(T, H) for Kajiya-Kay.
    pub width: f32,
    /// Moves the band, before the mask's shift: lowers a painted band's N·H threshold, sets an
    /// angel ring's height on the head (the view-space normal's Y: 0.5 rings it 30° above its
    /// middle), tilts Kajiya-Kay's strands along the normal.
    pub shift: f32,
}

impl Default for HairHighlight {
    fn default() -> Self {
        Self { style: HighlightStyle::PaintedBand, color: [1.0; 3], strength: 0.0, width: 0.1, shift: 0.0 }
    }
}

/// How the hair highlight is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum HighlightStyle {
    /// A painted mask revealed by thresholded N·H on the region's GI normals, which its sphere
    /// weight rounds (WuWa, ZZZ). Cheapest.
    #[default]
    PaintedBand,
    /// A ring fixed in view space: a band where the view-space normal's height is `shift`, so it
    /// stays put on screen as the camera moves around the head (the angel ring of Unity's anime
    /// shaders, without their second UV set).
    AngelRing,
    /// Kajiya-Kay's strand highlight, thresholded, along strands that run the way the hair's UV
    /// V grows, root to tip (meshes carry no tangents).
    KajiyaKay,
}

/// Eye settings: eyes and brows modeled apart from the face. They are always lit, take no rim,
/// and draw through the bangs when `through_hair` says so (M1 C5); C6 layers them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct EyeShading {
    /// Linear RGBA: R highlight, G eye shadow.
    pub mask_map: Option<AssetId>,
    /// Depth of the iris's parallax refraction fake, in UV units.
    pub parallax: f32,
    /// Draws the eye through the character's hair.
    pub through_hair: Option<ThroughHair>,
}

/// The light split: where N·L turns from lit to shadow.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct LightSplit {
    /// N·L at the split, before the control map's bias moves it.
    pub threshold: f32,
    /// Half-width of the transition, in N·L, where it is wider than the one-pixel edge every
    /// split gets.
    pub softness: f32,
}

impl Default for LightSplit {
    fn default() -> Self {
        Self { threshold: 0.0, softness: 0.02 }
    }
}

/// Where the shadow side's color comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum DiffuseMode {
    /// Analytic cel bands with per-region colors (ZZZ).
    Bands(BandDiffuse),
    /// Ramp texture rows per region (Genshin, Star Rail, WuWa).
    Ramp(RampDiffuse),
}

impl Default for DiffuseMode {
    fn default() -> Self {
        Self::Bands(BandDiffuse::default())
    }
}

/// Band mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct BandDiffuse {
    /// Two tones by default. With a threshold, a third band below it (on the same biased
    /// N·L as the split) takes each region's `deep_shade_color`.
    pub deep_threshold: Option<f32>,
}

/// Ramp mode.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct RampDiffuse {
    /// The ramp: uncompressed RGBA, a row per region and time of day.
    pub map: AssetId,
    /// How much of the shadow color is the ramp row rather than the region's subsurface color:
    /// 1 is the row alone, and WuWa's skin uses about 0.3 (`_RampInt`).
    #[serde(default = "RampDiffuse::default_mix")]
    pub mix: f32,
}

impl RampDiffuse {
    fn default_mix() -> f32 {
        1.0
    }
}

/// Parameters of one region (material ID).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct CharacterRegion {
    /// Band mode: the shadow side's color, as a multiplier on albedo.
    pub shade_color: [f32; 3],
    /// Band mode with a deep threshold: the third band's color, a multiplier on albedo.
    pub deep_shade_color: [f32; 3],
    /// Ramp mode: the daylight row, one of the ramp's [`ramp::ROWS`] (A7's layout); by default
    /// this region's own, `ramp::row(id, Day)` for its material ID.
    pub ramp_row: Option<u16>,
    /// Ramp mode: the night row; by default `ramp::row(id, Night)`. Time of day blends the
    /// two.
    pub night_ramp_row: Option<u16>,
    /// Ramp mode: the tint the row mixes with (WuWa skin: a warm peach).
    pub subsurface_color: [f32; 3],
    /// Specular color × intensity, gated by the control map's specular mask (R); black turns it
    /// off.
    pub specular_color: [f32; 3],
    /// Highlight size: the Blinn exponent, or the GGX roughness.
    pub specular_size: f32,
    /// Tints the rim light.
    pub rim_color: [f32; 3],
    /// Scales the rim's strength: 0 masks the rim off the region (M1 C5).
    pub rim_strength: f32,
    /// Scales the rim's width.
    pub rim_width: f32,
    /// How much rim the region takes where the character overlaps itself (an arm over the
    /// torso) rather than where it ends against something else: 0 none, 1 full.
    pub rim_overlap: f32,
    /// The inverted-hull outline's color.
    pub outline_color: [f32; 3],
    /// Scales the material's outline width.
    pub outline_width: f32,
    /// How the character rig's stylized GI treats this region.
    pub gi: RegionGi,
}

impl Default for CharacterRegion {
    fn default() -> Self {
        Self {
            shade_color: [0.55, 0.5, 0.65],
            deep_shade_color: [0.4, 0.35, 0.55],
            ramp_row: None,
            night_ramp_row: None,
            subsurface_color: [1.0; 3],
            specular_color: [0.0; 3],
            specular_size: 32.0,
            rim_color: [1.0; 3],
            rim_strength: 1.0,
            rim_width: 1.0,
            rim_overlap: 1.0,
            outline_color: [0.08, 0.06, 0.1],
            outline_width: 1.0,
            gi: RegionGi::default(),
        }
    }
}

/// A region's share of the character light rig's stylized GI (spec §5.12 "Stylized character
/// GI", M1 C2): the ambient light characters take is flattened and tinted rather than shaded
/// like a physical surface's, and these weights say how much of that each region takes. Both
/// are 0 to 1, and 0 (the default) leaves the region's GI as its normals give it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct RegionGi {
    /// How far the region's GI normals turn toward those of a sphere around the character's
    /// head: 1 for faces and hair, whose ambient light then shades like one smooth ball instead
    /// of following every fold of a nose or strand (WuWa's spherical GI normals).
    pub sphere: f32,
    /// How much of the rig's skin tint the region's GI takes: 1 for skin, so its ambient light
    /// never reads grey or cold on the shadow side.
    pub skin: f32,
}

/// The depth-offset rim light (M1 C5; spec §5.12 "Anime character shading"): a thin line of the
/// key light on the silhouette, where the character ends against what lies behind it.
///
/// Each pixel looks `width` pixels across the screen, along its normal, into the character
/// prepass's depth. Where what lies there is at least `threshold` farther away, or no character
/// is there at all, the pixel is on the rim, which then takes the key light's color and the
/// rig's rim scale, gated by Fresnel, by the light and by the region. The offset is in pixels,
/// so the rim holds its width at any distance and field of view as the reference games' rims
/// do, rather than thinning on a close-up and swamping a distant character as an N·V rim does.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Rim {
    /// The rim's width in pixels at a 1080-row target, scaled with the target's height and, as
    /// the outline's is, by its distance curve and model scale ([`Outline`]), so rim and line
    /// art keep their proportions. Each region scales it.
    pub width: f32,
    /// 0 turns it off.
    pub strength: f32,
    /// The depth step behind the pixel, in meters, that makes a rim.
    pub threshold: f32,
    /// The Fresnel gate: the share of the N·V range, in from the silhouette, where the rim may
    /// show. 1 leaves it to the depth step alone.
    pub fresnel: f32,
    /// How much the rim needs the light: 0 it shows on the shadow side as on the lit, 1 only
    /// where the key light reaches (WuWa).
    pub light_gate: f32,
    /// ZZZ's back-light weighting: 0 none. At 1 the rim scales with how far behind the character
    /// the light is: nothing with the light behind the camera, 1 with it to the side, 2 with it
    /// straight behind.
    pub back_light: f32,
}

impl Default for Rim {
    fn default() -> Self {
        Self { width: 4.0, strength: 0.35, threshold: 0.1, fresnel: 0.5, light_gate: 0.75, back_light: 0.0 }
    }
}

impl Rim {
    fn validate(&self) -> Result<(), MaterialError> {
        non_negative("rim.width", self.width)?;
        non_negative("rim.strength", self.strength)?;
        non_negative("rim.threshold", self.threshold)?;
        unit("rim.fresnel", self.fresnel)?;
        unit("rim.light_gate", self.light_gate)?;
        unit("rim.back_light", self.back_light)
    }
}

/// The specular highlight's model; each region sets its color and size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Specular {
    pub model: SpecularModel,
    /// Half-width of the highlight's edge.
    pub softness: f32,
}

impl Default for Specular {
    fn default() -> Self {
        Self { model: SpecularModel::Blinn, softness: 0.05 }
    }
}

/// How the highlight is shaped before it is thresholded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum SpecularModel {
    /// Thresholded Blinn-Phong (Genshin, WuWa).
    #[default]
    Blinn,
    /// Thresholded minimal GGX (ZZZ).
    Ggx,
}

/// The matcap.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct Matcap {
    /// Color or HDR, sampled by the view-space normal.
    pub map: AssetId,
    /// On the lit side.
    #[serde(default = "Matcap::default_strength")]
    pub strength: f32,
    /// On the shadow side.
    #[serde(default = "Matcap::default_shadow_strength")]
    pub shadow_strength: f32,
}

impl Matcap {
    fn default_strength() -> f32 {
        1.0
    }

    fn default_shadow_strength() -> f32 {
        0.5
    }
}

/// The inverted-hull outline (outlines v2, M1 C4; spec §5.12 "Outlines"): the anime line art
/// around the silhouette and over overlapping parts, at an art-directed width that holds at any
/// distance and field of view, in a color the character's light reaches.
///
/// The width in pixels at a 1080-row target is
/// `width × region scale × vertex color G × model_scale × curve(distance)`, scaled with the
/// target's height. The curve reads the *effective distance*: the view depth times
/// `tan(fov_y / 2) / tan(22.5°)`, the distance at which the model would look as large through a
/// 45° lens, so zooming in is moving closer and a line keeps its width wherever a character looks
/// the same size (the reference games' FOV normalization).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Outline {
    /// Pixels at a 1080-pixel-high target at the curve's `mid` distance, scaled with resolution;
    /// the region's scale, the vertex color's G, the model scale and the curve also apply. 0
    /// turns the outline off.
    pub width: f32,
    /// Pushes the hull away from the camera along the view ray, in meters, times the vertex
    /// color's B, so it doesn't poke through nearby surfaces.
    pub z_offset: f32,
    /// How the width follows the effective distance.
    pub curve: OutlineCurve,
    /// Scales the width: the model's size relative to the character the line style was tuned
    /// for, so a chibi (0.6) and an adult (1) keep the same weight of line for their size
    /// (StarRailNPR's `_ModelScale`).
    pub model_scale: f32,
    /// The depth the hull's extrusion direction takes, away from the camera, in place of the
    /// outline normal's own (view-space Z flattening): the direction is the outline normal's
    /// part across the screen with this behind it, normalized. 0 extrudes across the screen
    /// alone, the full width wherever the normal points; larger values narrow and push back the
    /// extrusion where the normal faces the camera, where a hull could otherwise poke through
    /// the surface at grazing angles. Genshin uses about 0.01, StarRailNPR 0.1.
    pub flatten: f32,
    /// How much of the surface's albedo the line takes: 0 the region's outline color as it is,
    /// 1 that color times the albedo, a darker shade of the color it outlines, as anime color
    /// lines are. In between blends the two.
    pub albedo: f32,
    /// How much the line is lit: 1 as the surface it outlines is (the character's key light
    /// through the material's light split and shadow color, plus its stylized GI flattened
    /// toward the camera), so lines darken at night and take the light's tint; 0 the color as
    /// it is, unlit (MToon's `outlineLightingMixFactor`).
    pub lighting: f32,
}

impl Default for Outline {
    fn default() -> Self {
        Self {
            width: 5.5,
            z_offset: 0.0,
            curve: OutlineCurve::default(),
            model_scale: 1.0,
            flatten: 0.1,
            albedo: 0.0,
            lighting: 1.0,
        }
    }
}

impl Outline {
    fn validate(&self) -> Result<(), MaterialError> {
        non_negative("outline.width", self.width)?;
        finite("outline.z_offset", self.z_offset)?;
        self.curve.validate()?;
        non_negative("outline.model_scale", self.model_scale)?;
        non_negative("outline.flatten", self.flatten)?;
        unit("outline.albedo", self.albedo)?;
        unit("outline.lighting", self.lighting)
    }
}

/// The outline width's two-segment curve over effective distance ([`Outline`]), in meters, and
/// its LOD cutoff.
///
/// Between `near` and `mid` the width scale runs linearly from `near`'s scale to 1, and between
/// `mid` and `far` from 1 to `far`'s. Outside them the line keeps its width in the world: closer
/// than `near` it widens in pixels as the camera closes in (readable in close-ups), and past
/// `far` it thins with distance, so a crowd doesn't turn into outlines, without vanishing early.
/// Past `cutoff` there is no hull at all, and over its last quarter the width fades to nothing.
/// With `near` at 0 a line keeps its pixel width close up; with `near`, `mid` and `far` equal, it
/// keeps its world width everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct OutlineCurve {
    /// The effective distance closer than which the width holds in the world, and the scale on
    /// the width there.
    pub near: (f32, f32),
    /// Where the width is the material's own.
    pub mid: f32,
    /// The effective distance past which the width holds in the world, and the scale there.
    pub far: (f32, f32),
    /// The LOD cutoff: the effective distance past which instances draw no hull.
    pub cutoff: f32,
}

impl Default for OutlineCurve {
    /// Lines at their full width from half a meter to 3 m (a full-length character about 740
    /// px tall at 1080 rows), thinning to 35% by 20 m (about 110 px tall), then with the
    /// character; gone past 80 m, fading from 60.
    fn default() -> Self {
        Self { near: (0.5, 1.0), mid: 3.0, far: (20.0, 0.35), cutoff: 80.0 }
    }
}

impl OutlineCurve {
    /// The share of the cutoff distance, at its end, over which the width fades out.
    pub const FADE: f32 = 0.25;

    /// The width scale at `distance` (effective meters), before the cutoff's fade: what the
    /// outline shader computes (`outline_curve` in `outline.wgsl`).
    pub fn scale(&self, distance: f32) -> f32 {
        let ((near, near_scale), mid, (far, far_scale)) = (self.near, self.mid, self.far);
        if distance < near {
            near_scale * near / distance.max(1e-6)
        } else if distance <= mid {
            lerp(near_scale, 1.0, ramp(near, mid, distance))
        } else if distance <= far {
            lerp(1.0, far_scale, ramp(mid, far, distance))
        } else {
            far_scale * far / distance
        }
    }

    /// The cutoff's fade at `distance`: 1 up to three quarters of the cutoff, 0 at and past it.
    pub fn fade(&self, distance: f32) -> f32 {
        ((self.cutoff - distance) / (self.cutoff * Self::FADE)).clamp(0.0, 1.0)
    }

    fn validate(&self) -> Result<(), MaterialError> {
        let ((near, near_scale), mid, (far, far_scale)) = (self.near, self.mid, self.far);
        non_negative("outline.curve.near", near)?;
        non_negative("outline.curve.near scale", near_scale)?;
        non_negative("outline.curve.far scale", far_scale)?;
        finite("outline.curve.cutoff", self.cutoff)?;
        if !(near <= mid && mid <= far && far.is_finite()) {
            return Err(invalid("outline.curve", "needs near ≤ mid ≤ far, all finite"));
        }
        if self.cutoff <= 0.0 {
            return Err(invalid("outline.curve.cutoff", "must be positive"));
        }
        Ok(())
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Where `x` lies from `from` to `to`, clamped to 0..=1; 1 for an empty range.
fn ramp(from: f32, to: f32, x: f32) -> f32 {
    if to > from { ((x - from) / (to - from)).clamp(0.0, 1.0) } else { 1.0 }
}

/// The anime character family's GPU block (WGSL `AnimeCharacterParams`,
/// `shaders/materials.wgsl` in `sapphire_render`). 1120 bytes: a 128-byte header, the regions,
/// then outlines v2's settings (M1 C4), then the rim's, the hair shadow's and the
/// eyes-through-hair settings and the regions' rim settings (M1 C5), each after the last so
/// every earlier offset holds.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct AnimeCharacterParams {
    /// Linear RGBA; multiplies the base color map.
    pub base_color: [f32; 4],
    /// x: light-split threshold on N·L; y: split softness; z: the deep shade's threshold (with
    /// [`Self::FLAG_DEEP_SHADE`]); w: alpha-test cutoff.
    pub split: [f32; 4],
    /// x: rim width in pixels at 1080p; y: rim strength; z: outline width in pixels at 1080p;
    /// w: outline Z offset in meters.
    pub edges: [f32; 4],
    /// x: ramp mix; y: specular softness; z: matcap strength lit; w: matcap strength in shadow.
    pub terms: [f32; 4],
    /// By surface. Face: x shadow softness, y nose line threshold, z highlight strength, w
    /// highlight reveal. Hair: x highlight strength, y width, z shift. Eye: x parallax depth.
    pub surface: [f32; 4],
    /// Face: rgb blush color. Hair: rgb highlight color.
    pub surface_color: [f32; 4],
    /// x: flags (bit `i` for texture slot `i`, then `FLAG_*`); y: region count; z: surface
    /// (`SURFACE_*`); w: specular model (`SPECULAR_*`) | highlight style (`HIGHLIGHT_*`) << 8.
    pub modes: [u32; 4],
    /// Each texture slot's swizzle code, two to a word ([`super::swizzle_code`]).
    pub swizzles: [u32; 4],
    /// Per region; the first `modes.y` are set.
    pub regions: [AnimeCharacterRegion; AnimeCharacterParams::MAX_REGIONS],
    /// The outline's distance curve ([`OutlineCurve`]): x near distance, y mid distance, z far
    /// distance, w cutoff, in effective meters.
    pub outline_curve: [f32; 4],
    /// x: the curve's near scale; y: its far scale; z: model scale; w: flattening.
    pub outline_shape: [f32; 4],
    /// x: how much of the albedo the line takes; y: how much it is lit; z, w: zero.
    pub outline_color: [f32; 4],
    /// The rim ([`Rim`]): x depth threshold in meters; y Fresnel extent; z light gate; w
    /// back-light weighting.
    pub rim: [f32; 4],
    /// A face's hair shadow ([`HairShadow`]): x distance in meters; y strength; z, w: zero.
    pub hair_shadow: [f32; 4],
    /// Eyes through hair ([`ThroughHair`]): x opacity, 0 for none; y, z the cosines of the yaw
    /// and pitch fades; w the distance in meters.
    pub through_hair: [f32; 4],
    /// Per region: x rim strength scale; y rim width scale; z rim on self-overlap; w zero.
    pub rim_regions: [[f32; 4]; AnimeCharacterParams::MAX_REGIONS],
}

/// One region of [`AnimeCharacterParams`] (WGSL `AnimeCharacterRegion`). 96 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct AnimeCharacterRegion {
    /// rgb: first shade, a multiplier on albedo; a: daylight ramp row (of [`ramp::ROWS`]; the
    /// shader samples its center, `(row + 0.5) / ROWS`).
    pub shade: [f32; 4],
    /// rgb: deep shade; a: night ramp row.
    pub deep_shade: [f32; 4],
    /// rgb: the subsurface tint ramp rows mix with; a: the GI's sphere weight
    /// ([`RegionGi::sphere`]).
    pub subsurface: [f32; 4],
    /// rgb: specular color × intensity; a: size.
    pub specular: [f32; 4],
    /// rgb: rim tint; a: the GI's skin weight ([`RegionGi::skin`]).
    pub rim: [f32; 4],
    /// rgb: outline color; a: outline width scale.
    pub outline: [f32; 4],
}

impl GpuBlock for AnimeCharacterParams {
    fn vectors(&self) -> Vec<[f32; 4]> {
        let header = [self.base_color, self.split, self.edges, self.terms, self.surface, self.surface_color];
        let regions = self
            .regions
            .iter()
            .flat_map(|r| [r.shade, r.deep_shade, r.subsurface, r.specular, r.rim, r.outline]);
        let outline = [self.outline_curve, self.outline_shape, self.outline_color];
        let c5 = [self.rim, self.hair_shadow, self.through_hair];
        header.into_iter().chain(regions).chain(outline).chain(c5).chain(self.rim_regions).collect()
    }

    fn modes(&self) -> [u32; 4] {
        self.modes
    }

    fn swizzles(&self) -> [u32; 4] {
        self.swizzles
    }
}

impl AnimeCharacterParams {
    /// Regions a material can have: one per material ID of the control map.
    pub const MAX_REGIONS: usize = MATERIAL_IDS as usize;

    /// Shadow colors come from the ramp (texture slot [`AnimeCharacterMaterial::RAMP`]).
    pub const FLAG_RAMP: u32 = 1 << 8;
    /// Band mode has a third, deep band.
    pub const FLAG_DEEP_SHADE: u32 = 1 << 9;
    /// Alpha below `split.w` is discarded.
    pub const FLAG_ALPHA_TEST: u32 = 1 << 10;
    /// Alpha blends.
    pub const FLAG_TRANSPARENT: u32 = 1 << 11;

    pub const SURFACE_BODY: u32 = 0;
    pub const SURFACE_FACE: u32 = 1;
    pub const SURFACE_HAIR: u32 = 2;
    pub const SURFACE_EYE: u32 = 3;

    pub const SPECULAR_BLINN: u32 = 0;
    pub const SPECULAR_GGX: u32 = 1;

    pub const HIGHLIGHT_PAINTED_BAND: u32 = 0;
    pub const HIGHLIGHT_ANGEL_RING: u32 = 1;
    pub const HIGHLIGHT_KAJIYA_KAY: u32 = 2;
}

impl AnimeCharacterMaterial {
    /// Texture slot of the base color map.
    pub const BASE_COLOR: usize = 0;
    /// Texture slot of the control map.
    pub const CONTROL: usize = 1;
    pub const NORMAL: usize = 2;
    /// Texture slot of the ramp, in ramp mode.
    pub const RAMP: usize = 3;
    /// Texture slot of a face's SDF shadow map.
    pub const FACE_SHADOW: usize = 4;
    /// Texture slot of the face, hair or eye mask.
    pub const SURFACE_MASK: usize = 5;
    pub const MATCAP: usize = 6;
    /// Texture slot of a face's expression map.
    pub const EXPRESSION: usize = 7;

    /// The family's texture slots, in binding order (group 3 in the renderer).
    pub const TEXTURE_SLOTS: &[TextureSlot] = &[
        TextureSlot { name: "base_color", usage: TextureUsage::Color },
        TextureSlot { name: "control", usage: TextureUsage::Control },
        TextureSlot { name: "normal", usage: TextureUsage::Normal },
        TextureSlot { name: "ramp", usage: TextureUsage::Ramp },
        TextureSlot { name: "face_shadow", usage: TextureUsage::FaceShadow },
        TextureSlot { name: "surface_mask", usage: TextureUsage::Mask },
        TextureSlot { name: "matcap", usage: TextureUsage::Light },
        TextureSlot { name: "expression", usage: TextureUsage::Mask },
    ];

    /// The GPU block, given the swizzle of each slot's texture (identity where none is bound).
    pub fn gpu_params(&self, swizzles: &[Swizzle]) -> AnimeCharacterParams {
        let textures = self.textures();
        let mut flags = bound_bits(&textures);
        let mut split = [self.light_split.threshold, self.light_split.softness, 0.0, self.alpha.cutoff()];
        let mut ramp_mix = 1.0;
        match &self.diffuse {
            DiffuseMode::Bands(bands) => {
                if let Some(threshold) = bands.deep_threshold {
                    flags |= AnimeCharacterParams::FLAG_DEEP_SHADE;
                    split[2] = threshold;
                }
            }
            DiffuseMode::Ramp(ramp) => {
                flags |= AnimeCharacterParams::FLAG_RAMP;
                ramp_mix = ramp.mix;
            }
        }
        match self.alpha {
            AlphaMode::Opaque => {}
            AlphaMode::Mask { .. } => flags |= AnimeCharacterParams::FLAG_ALPHA_TEST,
            AlphaMode::Blend => flags |= AnimeCharacterParams::FLAG_TRANSPARENT,
        }
        let (surface_kind, surface, surface_color, highlight) = match &self.surface {
            CharacterSurface::Body => (AnimeCharacterParams::SURFACE_BODY, [0.0; 4], [0.0; 4], 0),
            CharacterSurface::Face(face) => (
                AnimeCharacterParams::SURFACE_FACE,
                [face.softness, face.nose_line, face.highlight, face.highlight_reveal],
                rgb_w(face.blush_color, 0.0),
                0,
            ),
            CharacterSurface::Hair(hair) => {
                let h = &hair.highlight;
                let style = match h.style {
                    HighlightStyle::PaintedBand => AnimeCharacterParams::HIGHLIGHT_PAINTED_BAND,
                    HighlightStyle::AngelRing => AnimeCharacterParams::HIGHLIGHT_ANGEL_RING,
                    HighlightStyle::KajiyaKay => AnimeCharacterParams::HIGHLIGHT_KAJIYA_KAY,
                };
                (
                    AnimeCharacterParams::SURFACE_HAIR,
                    [h.strength, h.width, h.shift, 0.0],
                    rgb_w(h.color, 0.0),
                    style,
                )
            }
            CharacterSurface::Eye(eye) => {
                (AnimeCharacterParams::SURFACE_EYE, [eye.parallax, 0.0, 0.0, 0.0], [0.0; 4], 0)
            }
        };
        let (hair_shadow, through_hair) = match &self.surface {
            CharacterSurface::Face(face) => {
                let shadow = face.hair_shadow;
                ([shadow.distance, shadow.strength, 0.0, 0.0], ThroughHair::gpu(face.through_hair.as_ref()))
            }
            CharacterSurface::Eye(eye) => ([0.0; 4], ThroughHair::gpu(eye.through_hair.as_ref())),
            CharacterSurface::Body | CharacterSurface::Hair(_) => ([0.0; 4], [0.0; 4]),
        };
        let specular_model = match self.specular.model {
            SpecularModel::Blinn => AnimeCharacterParams::SPECULAR_BLINN,
            SpecularModel::Ggx => AnimeCharacterParams::SPECULAR_GGX,
        };
        let (matcap, matcap_shadow) = self.matcap.map_or((0.0, 0.0), |m| (m.strength, m.shadow_strength));
        let (outline, curve) = (&self.outline, &self.outline.curve);
        let mut regions = [AnimeCharacterRegion::default(); AnimeCharacterParams::MAX_REGIONS];
        let mut rim_regions = [[0.0; 4]; AnimeCharacterParams::MAX_REGIONS];
        for (rim, region) in rim_regions.iter_mut().zip(&self.regions) {
            *rim = [region.rim_strength, region.rim_width, region.rim_overlap, 0.0];
        }
        for ((gpu, region), id) in regions.iter_mut().zip(&self.regions).zip(MaterialId::ALL) {
            let row = |row: Option<u16>, half| row.map_or(ramp::row(id, half), u32::from) as f32;
            *gpu = AnimeCharacterRegion {
                shade: rgb_w(region.shade_color, row(region.ramp_row, Half::Day)),
                deep_shade: rgb_w(region.deep_shade_color, row(region.night_ramp_row, Half::Night)),
                subsurface: rgb_w(region.subsurface_color, region.gi.sphere),
                specular: rgb_w(region.specular_color, region.specular_size),
                rim: rgb_w(region.rim_color, region.gi.skin),
                outline: rgb_w(region.outline_color, region.outline_width),
            };
        }
        AnimeCharacterParams {
            base_color: self.base_color,
            split,
            edges: [self.rim.width, self.rim.strength, self.outline.width, self.outline.z_offset],
            terms: [ramp_mix, self.specular.softness, matcap, matcap_shadow],
            surface,
            surface_color,
            modes: [flags, self.regions.len() as u32, surface_kind, specular_model | highlight << 8],
            swizzles: pack_swizzles(swizzles),
            regions,
            outline_curve: [curve.near.0, curve.mid, curve.far.0, curve.cutoff],
            outline_shape: [curve.near.1, curve.far.1, outline.model_scale, outline.flatten],
            outline_color: [outline.albedo, outline.lighting, 0.0, 0.0],
            rim: [self.rim.threshold, self.rim.fresnel, self.rim.light_gate, self.rim.back_light],
            hair_shadow,
            through_hair,
            rim_regions,
        }
    }

    /// Whether it draws anything through the character's hair: a face with a mask (whose B
    /// says which parts) or an eye, with `through_hair` set and some opacity.
    fn draws_through_hair(&self) -> bool {
        let through = match &self.surface {
            CharacterSurface::Face(face) if face.mask_map.is_some() => face.through_hair,
            CharacterSurface::Eye(eye) => eye.through_hair,
            _ => None,
        };
        through.is_some_and(|t| t.opacity > 0.0) && !matches!(self.alpha, AlphaMode::Blend)
    }
}

impl MaterialDocument for AnimeCharacterMaterial {
    const FAMILY: MaterialFamily = MaterialFamily::AnimeCharacter;

    fn validate(&self) -> Result<(), MaterialError> {
        color("base_color", &self.base_color)?;
        unit("base_color[3]", self.base_color[3])?;
        self.alpha.validate("alpha")?;
        finite("light_split.threshold", self.light_split.threshold)?;
        non_negative("light_split.softness", self.light_split.softness)?;
        match &self.diffuse {
            DiffuseMode::Bands(bands) => {
                if let Some(threshold) = bands.deep_threshold {
                    finite("diffuse.deep_threshold", threshold)?;
                }
            }
            DiffuseMode::Ramp(ramp) => unit("diffuse.mix", ramp.mix)?,
        }
        if !(1..=AnimeCharacterParams::MAX_REGIONS).contains(&self.regions.len()) {
            return Err(invalid("regions", "must hold 1 to 8 regions"));
        }
        for (index, region) in self.regions.iter().enumerate() {
            let field = |name: &str| format!("regions[{index}].{name}");
            for (name, row) in [("ramp_row", region.ramp_row), ("night_ramp_row", region.night_ramp_row)] {
                if row.is_some_and(|row| u32::from(row) >= ramp::ROWS) {
                    return Err(invalid(&field(name), "must be one of the ramp's 16 rows"));
                }
            }
            color(&field("shade_color"), &region.shade_color)?;
            color(&field("deep_shade_color"), &region.deep_shade_color)?;
            color(&field("subsurface_color"), &region.subsurface_color)?;
            color(&field("specular_color"), &region.specular_color)?;
            non_negative(&field("specular_size"), region.specular_size)?;
            color(&field("rim_color"), &region.rim_color)?;
            non_negative(&field("rim_strength"), region.rim_strength)?;
            non_negative(&field("rim_width"), region.rim_width)?;
            unit(&field("rim_overlap"), region.rim_overlap)?;
            color(&field("outline_color"), &region.outline_color)?;
            non_negative(&field("outline_width"), region.outline_width)?;
            unit(&field("gi.sphere"), region.gi.sphere)?;
            unit(&field("gi.skin"), region.gi.skin)?;
        }
        self.rim.validate()?;
        non_negative("specular.softness", self.specular.softness)?;
        if let Some(matcap) = &self.matcap {
            non_negative("matcap.strength", matcap.strength)?;
            non_negative("matcap.shadow_strength", matcap.shadow_strength)?;
        }
        self.outline.validate()?;
        match &self.surface {
            CharacterSurface::Body => Ok(()),
            CharacterSurface::Face(face) => {
                non_negative("surface.softness", face.softness)?;
                non_negative("surface.nose_line", face.nose_line)?;
                non_negative("surface.highlight", face.highlight)?;
                non_negative("surface.highlight_reveal", face.highlight_reveal)?;
                color("surface.blush_color", &face.blush_color)?;
                non_negative("surface.hair_shadow.distance", face.hair_shadow.distance)?;
                unit("surface.hair_shadow.strength", face.hair_shadow.strength)?;
                face.through_hair.map_or(Ok(()), |t| t.validate("surface.through_hair"))
            }
            CharacterSurface::Hair(hair) => {
                let h = &hair.highlight;
                color("surface.highlight.color", &h.color)?;
                non_negative("surface.highlight.strength", h.strength)?;
                non_negative("surface.highlight.width", h.width)?;
                finite("surface.highlight.shift", h.shift)
            }
            CharacterSurface::Eye(eye) => {
                finite("surface.parallax", eye.parallax)?;
                eye.through_hair.map_or(Ok(()), |t| t.validate("surface.through_hair"))
            }
        }
    }

    fn textures(&self) -> Vec<Option<AssetId>> {
        let ramp = match &self.diffuse {
            DiffuseMode::Ramp(ramp) => Some(ramp.map),
            DiffuseMode::Bands(_) => None,
        };
        let (face_shadow, surface_mask, expression) = match &self.surface {
            CharacterSurface::Body => (None, None, None),
            CharacterSurface::Face(face) => (Some(face.shadow_map), face.mask_map, face.expression_map),
            CharacterSurface::Hair(hair) => (None, hair.mask_map, None),
            CharacterSurface::Eye(eye) => (None, eye.mask_map, None),
        };
        vec![
            self.base_color_map,
            self.control_map,
            self.normal_map,
            ramp,
            face_shadow,
            surface_mask,
            self.matcap.map(|matcap| matcap.map),
            expression,
        ]
    }

    fn flags(&self) -> MaterialFlags {
        let mut flags = MaterialFlags::empty();
        match self.alpha {
            AlphaMode::Opaque => {}
            AlphaMode::Mask { .. } => flags |= MaterialFlags::ALPHA_TEST,
            AlphaMode::Blend => flags |= MaterialFlags::TRANSPARENT,
        }
        if self.outline.width > 0.0 {
            flags |= MaterialFlags::OUTLINE;
        }
        if self.draws_through_hair() {
            flags |= MaterialFlags::THROUGH_HAIR;
        }
        flags
            | match self.surface {
                CharacterSurface::Body => MaterialFlags::empty(),
                CharacterSurface::Face(_) => MaterialFlags::FACE,
                CharacterSurface::Hair(_) => MaterialFlags::HAIR,
                CharacterSurface::Eye(_) => MaterialFlags::EYE,
            }
    }

    fn params(&self, swizzles: &[Swizzle]) -> Vec<u8> {
        bytemuck::bytes_of(&self.gpu_params(swizzles)).to_vec()
    }
}

impl From<v1::AnimeCharacterMaterial> for AnimeCharacterMaterial {
    fn from(old: v1::AnimeCharacterMaterial) -> Self {
        let rim = Rim { fresnel: old.rim.width, strength: old.rim.strength, ..Rim::default() };
        Self {
            surface: old.surface,
            base_color: old.base_color,
            base_color_map: old.base_color_map,
            alpha: old.alpha,
            control_map: old.control_map,
            normal_map: old.normal_map,
            light_split: old.light_split,
            diffuse: old.diffuse,
            regions: old.regions,
            rim,
            specular: old.specular,
            matcap: old.matcap,
            outline: old.outline,
        }
    }
}

/// Version 1 of the document (M1 A3 to C4), frozen: its rim's `width` was the share of N·V, in
/// from the silhouette, that the rim covered, which version 2 calls `fresnel`. Its other fields
/// are version 2's, whose later additions all have defaults.
mod v1 {
    use serde::Deserialize;

    use super::{
        AlphaMode, CharacterRegion, CharacterSurface, DiffuseMode, LightSplit, Matcap, Outline, Specular,
    };
    use crate::AssetId;

    #[derive(Deserialize)]
    #[serde(default)]
    pub struct AnimeCharacterMaterial {
        pub surface: CharacterSurface,
        pub base_color: [f32; 4],
        pub base_color_map: Option<AssetId>,
        pub alpha: AlphaMode,
        pub control_map: Option<AssetId>,
        pub normal_map: Option<AssetId>,
        pub light_split: LightSplit,
        pub diffuse: DiffuseMode,
        pub regions: Vec<CharacterRegion>,
        pub rim: Rim,
        pub specular: Specular,
        pub matcap: Option<Matcap>,
        pub outline: Outline,
    }

    impl Default for AnimeCharacterMaterial {
        fn default() -> Self {
            let current = super::AnimeCharacterMaterial::default();
            Self {
                surface: current.surface,
                base_color: current.base_color,
                base_color_map: None,
                alpha: current.alpha,
                control_map: None,
                normal_map: None,
                light_split: current.light_split,
                diffuse: current.diffuse,
                regions: current.regions,
                rim: Rim::default(),
                specular: current.specular,
                matcap: None,
                outline: current.outline,
            }
        }
    }

    impl sapphire_reflect::Versioned for AnimeCharacterMaterial {
        const VERSION: u32 = 1;
    }

    #[derive(Deserialize)]
    #[serde(default)]
    pub struct Rim {
        /// The share of N·V, in from the silhouette, that the rim covered.
        pub width: f32,
        pub strength: f32,
    }

    impl Default for Rim {
        fn default() -> Self {
            Self { width: 0.22, strength: 0.35 }
        }
    }
}
