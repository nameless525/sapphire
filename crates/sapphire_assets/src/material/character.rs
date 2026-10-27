//! The anime character family: characters with authored light response (spec §5.12, M1 plan
//! pillar 2). The anime character shader (M1 C1's uber-shader) reads its block: ramp and band
//! diffuse, the control map's regions and bias, each region's share of the character light
//! rig's stylized GI (M1 C2), the depth-offset rim and its region mask, hair highlights, the
//! bangs' shadow on a face and eyes through the bangs (M1 C5), the layered eyes (M1 C6), the
//! layers on top of the base (M1 C7: stylized specular, the matcap, the reflection add,
//! stockings and breathing emission), two-sided surfaces (M1 C8), and the static permutations of
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
    AlphaMode, GpuBlock, bound_bits, color, finite, invalid, non_negative, pack_swizzles, rgb_w, shader_time,
    unit, whole_cycles,
};
use super::{MaterialDocument, MaterialError, MaterialFamily, MaterialFlags, TextureSlot, TextureUsage};
use crate::AssetId;
use crate::character_maps::ramp::{self, Half};
use crate::character_maps::{MATERIAL_IDS, MaterialId, eye_mask, layer_mask};
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
/// - **Layers** (M1 C7), each off until a region turns it on: a stylized highlight
///   ([`Specular`], the region's specular color), the matcap ([`Matcap`], the region's
///   [`RegionMatcap`]), the reflection add (the region's `metallic` above
///   [`layer_mask::REFLECTS`]), stockings ([`Stockings`], the region's `stocking`) and breathing
///   emission ([`Emission`], the region's `emission`). A body's `layer_mask` paints where each
///   shows.
/// - **Sides** (M1 C8): `double_sided` draws back faces too, lit as the side the camera sees.
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
    /// The stylized highlight's model (M1 C7); each region sets its color and size.
    pub specular: Specular,
    /// A spheremap for metals and gloss (M1 C7): each region picks a channel of it, a blend and
    /// a tint ([`RegionMatcap`]), and the layer mask's R paints where it shows.
    pub matcap: Option<Matcap>,
    pub outline: Outline,
    /// The layer mask (M1 C7; `character_maps::layer_mask`), linear: R matcap, G metallic, B
    /// emission, A stocking density, each scaling its region's value. Bodies alone: faces, hair
    /// and eyes bind their own masks in its slot.
    pub layer_mask: Option<AssetId>,
    /// The stockings the regions with a `stocking` weight wear (M1 C7).
    pub stockings: Stockings,
    /// How the regions' emission breathes and blooms (M1 C7).
    pub emission: Emission,
    /// Draws the surface's back faces too (M1 C8; glTF's `doubleSided`), shading them with the
    /// flipped normal: garments made of one layer, such as a collar or a skirt, show their
    /// inside rather than what lies behind them. Blended materials (`alpha: Blend`) keep culling
    /// back faces until transparent draws are sorted (spec §5.12, "Sorting"). The outline hull
    /// stays the surface's outer shell either way. Written only when set, so documents from
    /// before it read and write as they did.
    #[serde(skip_serializing_if = "is_false")]
    pub double_sided: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Versioned for AnimeCharacterMaterial {
    /// 2: the depth-offset rim (M1 C5). [`Rim::width`] became the depth offset in pixels, and
    /// what it was, the Fresnel extent, is [`Rim::fresnel`]; version 1 documents load with their
    /// width moved there and the new settings at their defaults.
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
            layer_mask: None,
            stockings: Stockings::default(),
            emission: Emission::default(),
            double_sided: false,
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
    /// A layer of an eye, or lashes or brows modeled as eye parts (C5, C6).
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
    /// How soft its edge is (M1 C8), in meters across the face like [`Self::distance`]: the
    /// width over which it fades in along the key light's direction, centered on the hard
    /// edge's place, as a penumbra. The renderer looks toward the light at reaches spread over
    /// it and blends the face's tone from lit to the shadow's by the share of them that find
    /// hair, so the edge fades smoothly instead of snapping to the face's light split. 0 (the
    /// default) keeps the hard edge exactly, with one look. Written only when set, so documents
    /// from before it read and write as they did.
    #[serde(skip_serializing_if = "is_zero")]
    pub softness: f32,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

impl Default for HairShadow {
    fn default() -> Self {
        Self { distance: 0.03, strength: 1.0, softness: 0.0 }
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
    /// Linear RGBA: R highlight intensity, G highlight shift (0.5 none, in the highlight's own
    /// units), B occlusion (WuWa's `_HM`), A thinness for the transmission (M1 C8; 1 without
    /// alpha); `character_maps::hair_mask`.
    pub mask_map: Option<AssetId>,
    pub highlight: HairHighlight,
    /// Light through thin hair against a back light (M1 C8): off by default. Written only when
    /// it differs from the default, so documents from before it read and write as they did.
    #[serde(skip_serializing_if = "HairTransmission::is_default")]
    pub transmission: HairTransmission,
}

/// Light through thin hair (M1 C8; spec §5.12 "Anime character shading"): where the key light
/// comes from behind the hair as the camera sees it, thin hair glows in its own color, as
/// back-lit hair does in the reference games' key art. The glow is the hair's albedo × the key
/// light's radiance × the strength × the mask's thinness (A) × `max(−L·V, 0)^sharpness`, and an
/// environment shadow over the character blocks it with the key light. It adds to whatever the
/// hair's light split gives, so it shows most on the shadow side the camera sees against a back
/// light.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct HairTransmission {
    /// How much of the key light passes through thin hair: 0 (the default) turns it off.
    pub strength: f32,
    /// How tightly it gathers around a light straight behind the hair: the power `−L·V` is
    /// raised to, above 0. 4 glows within about 40° of straight behind.
    pub sharpness: f32,
}

impl HairTransmission {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The hair's surface vector's w and color's w: the strength and the sharpness, zero both
    /// when off, so hair without it packs as it did before.
    fn gpu(&self) -> (f32, f32) {
        if self.strength > 0.0 { (self.strength, self.sharpness) } else { (0.0, 0.0) }
    }
}

impl Default for HairTransmission {
    fn default() -> Self {
        Self { strength: 0.0, sharpness: 4.0 }
    }
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

/// Eye settings: eyes, lashes and brows modeled apart from the face. They are always lit, take
/// no rim, and draw through the bangs when `through_hair` says so (M1 C5).
///
/// Anime eyes are layered meshes (M1 C6; spec §5.12 "Anime character shading"), each with an
/// eye material of its own, which `layer` names: the eye itself (the eyeball, and lashes and
/// brows modeled as eye parts), then over it the lid's shadow, the iris and the highlights.
/// The eye writes the character prepass's eye part, the **eye depth mask**; the layers over it
/// blend only where that mask shows the character's own eye as its nearest surface but for hair,
/// so a lid closing over the eye takes its iris and highlights with it, and they never spill
/// onto the skin around it.
///
/// The eye and iris layers can take a **parallax refraction fake**: the iris lies `parallax`
/// meters below the cornea at its deepest, and each texel shows the iris toward where the view
/// ray through it would meet that depth (offset-limited: the view's along-surface part times the
/// depth, so it never runs off at grazing angles), so the iris shifts against the eye's outline
/// as the view turns, its deepest point (the pupil) most.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct EyeShading {
    /// Which layer of the eye the material draws (M1 C6).
    pub layer: EyeLayer,
    /// The eye mask (`character_maps::eye_mask`), linear: R the iris's depth below the cornea,
    /// from 0 to 1 of `parallax`. Without it the depth is a dome over `iris`, or with no iris
    /// the full depth, so the layer's whole texture shifts as one.
    pub mask_map: Option<AssetId>,
    /// How far below the cornea the iris lies at its deepest, in meters: the parallax
    /// refraction fake's depth on the eye and iris layers (M1 C6). 0 turns it off. A few
    /// millimeters suit an anime eye a few centimeters wide. An alpha-tested eye still cuts out at
    /// its own texture coordinates, the cornea's edge: the depth moves only what shows inside it.
    pub parallax: f32,
    /// Where the iris lies in the layer's texture coordinates, for the parallax's dome.
    pub iris: Option<IrisShape>,
    /// Draws the eye through the character's hair.
    pub through_hair: Option<ThroughHair>,
}

/// Which layer of an anime eye a material draws (M1 C6; see [`EyeShading`]). Layers draw in this
/// order, each over the ones before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum EyeLayer {
    /// The eye itself: the eyeball (the sclera, with an iris where one is painted on it), and
    /// lashes and brows modeled as eye parts. Opaque or alpha-tested: it writes depth and the
    /// character prepass's eye part, the eye depth mask the other layers draw within.
    #[default]
    Eye,
    /// The upper lid's shadow on the eye white, blended over the eye by its coverage in its
    /// shadow tone (the region's shade over the albedo, as on a shadow side): the eye's own
    /// authored shadow color, never a darkened white.
    Shadow,
    /// The iris and pupil, blended over the eye and its shadow by their coverage: a mesh of its
    /// own, so gaze (eye joints or look expressions) moves it across the eye white, which clips
    /// it.
    Iris,
    /// Highlights, blended over everything below, lit as the eye is and animated.
    Highlight(HighlightMotion),
}

impl EyeLayer {
    /// The layer's index in the GPU block (`AnimeCharacterParams::EYE_LAYER_*`), which is also
    /// its drawing order.
    pub fn index(self) -> u32 {
        match self {
            Self::Eye => AnimeCharacterParams::EYE_LAYER_EYE,
            Self::Shadow => AnimeCharacterParams::EYE_LAYER_SHADOW,
            Self::Iris => AnimeCharacterParams::EYE_LAYER_IRIS,
            Self::Highlight(_) => AnimeCharacterParams::EYE_LAYER_HIGHLIGHT,
        }
    }

    /// Whether it blends over the eye (the shadow, iris and highlight layers), rather than being
    /// the eye.
    pub fn is_overlay(self) -> bool {
        self != Self::Eye
    }
}

/// An ellipse in texture coordinates: where an eye layer's iris lies (M1 C6).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct IrisShape {
    pub center: [f32; 2],
    /// The half-extents along U and V: above 0.
    pub radius: [f32; 2],
}

impl Default for IrisShape {
    /// An iris filling its texture.
    fn default() -> Self {
        Self { center: [0.5, 0.5], radius: [0.5, 0.5] }
    }
}

/// How a highlight layer moves (M1 C6): a slow drift across the eye and a sparkle, both on the
/// frame's clock, each instance at a phase of its own. A highlight lies on the cornea, so it is
/// the part of an anime eye that lives: the reference games tremble and pulse theirs.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct HighlightMotion {
    /// How far the highlight drifts from where it is modeled, in meters across the model's front
    /// (its X and Y): a Lissajous curve (`character_maps::eye_mask::highlight_drift`). 0 holds
    /// it still.
    pub drift: f32,
    /// The drift's rate, in cycles a second. It runs as whole pairs of cycles an hour on the
    /// shaders' clock (`character_maps::eye_mask::drift_rate`), at most 1/3600 Hz off.
    pub drift_rate: f32,
    /// How much brighter the highlight glints at the peak of each sparkle: 0 none, 1 twice as
    /// bright (`character_maps::eye_mask::highlight_sparkle`). Its coverage scales with it, so
    /// a soft edge swells: a highlight tone mapping already holds at white still glints.
    pub sparkle: f32,
    /// Sparkles a second. It runs as whole sparkles an hour on the shaders' clock
    /// (`character_maps::eye_mask::sparkle_rate`), at most 1/7200 Hz off.
    pub sparkle_rate: f32,
}

impl Default for HighlightMotion {
    fn default() -> Self {
        Self { drift: 0.0003, drift_rate: 0.3, sparkle: 0.6, sparkle_rate: 0.4 }
    }
}

impl HighlightMotion {
    fn validate(&self) -> Result<(), MaterialError> {
        non_negative("surface.layer.drift", self.drift)?;
        non_negative("surface.layer.drift_rate", self.drift_rate)?;
        non_negative("surface.layer.sparkle", self.sparkle)?;
        non_negative("surface.layer.sparkle_rate", self.sparkle_rate)
    }
}

impl EyeShading {
    fn validate(&self, alpha: AlphaMode) -> Result<(), MaterialError> {
        non_negative("surface.parallax", self.parallax)?;
        if let Some(iris) = &self.iris {
            finite("surface.iris.center[0]", iris.center[0])?;
            finite("surface.iris.center[1]", iris.center[1])?;
            if !iris.radius.iter().all(|r| r.is_finite() && *r > 0.0) {
                return Err(invalid("surface.iris.radius", "must be above 0"));
            }
        }
        match (self.layer.is_overlay(), alpha) {
            (true, AlphaMode::Blend) | (false, AlphaMode::Opaque | AlphaMode::Mask { .. }) => {}
            (true, _) => {
                return Err(invalid(
                    "alpha",
                    "must be Blend: an eye's shadow, iris and highlight blend over it",
                ));
            }
            (false, _) => {
                return Err(invalid("alpha", "can't be Blend for an eye: it writes the eye depth mask"));
            }
        }
        if let EyeLayer::Highlight(motion) = &self.layer {
            motion.validate()?;
        }
        self.through_hair.map_or(Ok(()), |t| t.validate("surface.through_hair"))
    }

    /// The GPU block's `surface` and `surface_color`: x the parallax's depth, then by layer the
    /// iris (center, radius) of the eye and iris layers, zero without one, or a highlight's
    /// motion (drift, its rate, sparkle, its rate), the rates as whole cycles an hour.
    fn gpu(&self) -> ([f32; 4], [f32; 4]) {
        let surface = [self.parallax, 0.0, 0.0, 0.0];
        let color = match self.layer {
            EyeLayer::Eye | EyeLayer::Iris => self
                .iris
                .map_or([0.0; 4], |iris| [iris.center[0], iris.center[1], iris.radius[0], iris.radius[1]]),
            EyeLayer::Shadow => [0.0; 4],
            EyeLayer::Highlight(m) => [
                m.drift,
                eye_mask::drift_rate(m.drift_rate),
                m.sparkle,
                eye_mask::sparkle_rate(m.sparkle_rate),
            ],
        };
        (surface, color)
    }
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
    /// The stylized highlight's color × intensity (M1 C7), gated by the control map's specular
    /// mask (R) and sized by its threshold (B); black turns it off.
    pub specular_color: [f32; 3],
    /// Highlight size: the Blinn exponent (at least 1), or the GGX roughness (0 to 1).
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
    /// The matcap's channel, blend and tint here, where the material has a matcap (M1 C7).
    pub matcap: RegionMatcap,
    /// How strongly the region reflects (M1 C7; WuWa's metallic, a reflection strength rather
    /// than a physical blend toward a conductor): the layer mask's G scales it, and only texels
    /// above [`layer_mask::REFLECTS`] add the reflection, on top of the matcap, which stays the
    /// metal's base look. 0 to 1.
    pub metallic: f32,
    /// How blurred the reflection is: 0 a mirror, 1 the roughest the renderer keeps (WuWa clamps
    /// characters' roughness); 0 to 1.
    pub roughness: f32,
    /// How much of the material's stockings the region wears, 0 to 1, × the layer mask's A (M1
    /// C7).
    pub stocking: f32,
    /// The region's glow, linear RGB × intensity, × the layer mask's B (M1 C7): unlit, breathing
    /// as the material's [`Emission`] says. Black for none.
    pub emission: [f32; 3],
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
            matcap: RegionMatcap::default(),
            metallic: 0.0,
            roughness: 0.3,
            stocking: 0.0,
            emission: [0.0; 3],
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

/// The stylized highlight (M1 C7; spec §5.12 "Anime character shading"): a painted-looking
/// shape of the key light, not physically based shine. Each region sets its color and size; the
/// control map's specular mask (R) gates it and its specular threshold (B) sizes it (0.5
/// neutral, higher larger), and it shows on the lit side alone.
///
/// The model shapes N·H (on the shading normal, toward the character's key light), and the shape
/// is thresholded at `1 − B` into a crisp edge, `softness` wide or a pixel, whichever is wider
/// (Genshin's `step(1 − B, blinn)`, ZZZ's hard-clamped GGX). Its color is the region's, taking
/// the albedo by `albedo`: white highlights look plastic, so the reference games tint them by
/// the surface (WuWa raises the albedo to a power by the highlight).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Specular {
    pub model: SpecularModel,
    /// Half-width of the highlight's edge, in the shape's units (0 to 1).
    pub softness: f32,
    /// How much of the surface's albedo the highlight takes: 0 the region's color alone, 1 that
    /// color × the albedo.
    pub albedo: f32,
}

impl Default for Specular {
    fn default() -> Self {
        Self { model: SpecularModel::Blinn, softness: 0.05, albedo: 0.5 }
    }
}

/// How the highlight is shaped before it is thresholded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum SpecularModel {
    /// Thresholded Blinn-Phong (Genshin, WuWa): `N·H^size`, the region's size an exponent.
    #[default]
    Blinn,
    /// Thresholded minimal GGX (ZZZ): the GGX distribution at roughness `size`, normalized to 1
    /// facing the half vector, so its long tail sets a softer, wider shape than Blinn's.
    Ggx,
}

impl SpecularModel {
    /// The smallest GGX roughness a highlight takes: below it the shape is a single point.
    pub const MIN_GGX_ROUGHNESS: f32 = 0.02;

    /// The highlight's shape at `n_dot_h` for a region's `size`, 0 to 1 and 1 at `n_dot_h` = 1:
    /// what the specular threshold cuts (`specular_shape` in the anime character shader).
    pub fn shape(self, n_dot_h: f32, size: f32) -> f32 {
        let n_dot_h = n_dot_h.clamp(0.0, 1.0);
        match self {
            Self::Blinn => n_dot_h.powf(size.max(1.0)),
            Self::Ggx => {
                let r2 = size.clamp(Self::MIN_GGX_ROUGHNESS, 1.0).powi(2);
                let d = n_dot_h * n_dot_h * (r2 - 1.0) + 1.0;
                (r2 / d).powi(2)
            }
        }
    }
}

/// The matcap (M1 C7; spec §5.12 "Anime character shading"): a spheremap of painted light,
/// sampled by the view-space normal, for metals, leather and gloss. It reads as a painted
/// highlight that turns with the camera, not as a reflection of the scene (the reflection add
/// is the region's `metallic`, on top of it).
///
/// The map holds one color matcap (RGB) or up to four grayscale ones (R, G, B, A: WuWa's
/// four-channel `_MatCapTex`), and each region picks one and its blend and tint
/// ([`RegionMatcap`]). In a color texture R, G and B decode from sRGB and A is linear, as any
/// color texture's alpha; an HDR texture is linear throughout. The layer mask's R paints where it
/// shows. `strength` and `shadow_strength` scale it on the lit and the shadow side of the key
/// light's split (MToon's rim lighting mix, WuWa's lit and back matcap intensities).
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

/// How one region takes the material's matcap (M1 C7).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct RegionMatcap {
    pub channel: MatcapChannel,
    pub blend: MatcapBlend,
    /// Multiplies the matcap: linear RGB, above 1 to brighten.
    pub color: [f32; 3],
}

impl Default for RegionMatcap {
    /// The color matcap, added as light, untinted: MToon's matcap on every region.
    fn default() -> Self {
        Self { channel: MatcapChannel::Rgb, blend: MatcapBlend::Add, color: [1.0; 3] }
    }
}

/// Which of the matcap's channels a region reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum MatcapChannel {
    /// None: the region takes no matcap.
    Off,
    /// The color matcap.
    #[default]
    Rgb,
    /// One of four grayscale matcaps, tinted by the region's color.
    R,
    G,
    B,
    A,
}

impl MatcapChannel {
    /// The GPU block's code (`CharacterLayerRegion::matcap`'s fourth component): −1 none, 0 the
    /// color matcap, 1 to 4 the R, G, B or A channel.
    fn code(self) -> f32 {
        match self {
            Self::Off => -1.0,
            Self::Rgb => 0.0,
            Self::R => 1.0,
            Self::G => 2.0,
            Self::B => 3.0,
            Self::A => 4.0,
        }
    }
}

/// How a region's matcap meets its surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum MatcapBlend {
    /// Added as light, in the key light's color and intensity: gloss on leather and hair, a
    /// metal's painted sheen (WuWa, MToon).
    #[default]
    Add,
    /// Multiplies the albedo, before the light: the metal's shading painted into its color
    /// (Genshin's metal map), which the light split and GI then light. The lit and shadow
    /// strengths blend toward it from the plain albedo.
    Multiply,
}

/// Stockings and tights (M1 C7; spec §5.12 "Anime character shading"): sheer fabric over the
/// skin, painted as the reference games paint it (WuWa's and Star Rail's stocking layers) rather
/// than simulated, and opaque, so it needs no sorting. Regions wear it by their `stocking`
/// weight, and the layer mask's A paints where it covers.
///
/// - **Denier:** fabric hides more of the skin where the view grazes through it, so a leg reads
///   darker at its silhouette and lighter facing the camera, the skin showing through. The
///   albedo goes toward `color` by `edge_opacity` at and below the first N·V of `range`, and by
///   `facing_opacity` at and above the second, smoothly between ([`Stockings::opacity`]).
/// - **Sheen:** a soft highlight of the key light along the fabric's run (`direction`, the UV
///   axis the legs are laid out along; meshes carry no tangents), `sin(T, H)^sharpness`, in
///   `sheen` × the key light, on the lit side and where the fabric covers.
/// - **Weave:** a fine knit, `weave_scale` cells per UV unit, varying the fabric's opacity by
///   `weave_strength`, faded to its average where its cells shrink toward a pixel, so it never
///   shimmers.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Stockings {
    /// The fabric's color, linear RGB: what the skin goes toward where it hides it.
    pub color: [f32; 3],
    /// How much of the skin the fabric hides at the silhouette, 0 to 1.
    pub edge_opacity: f32,
    /// How much it hides facing the camera, 0 to 1: the denier.
    pub facing_opacity: f32,
    /// The N·V range from the silhouette's opacity to facing's: ascending, within 0 to 1.
    pub range: [f32; 2],
    /// The sheen's color × strength, linear RGB; black for none.
    pub sheen: [f32; 3],
    /// The sheen's sharpness: the exponent on `sin(T, H)`, at least 1; higher is narrower.
    pub sheen_sharpness: f32,
    pub direction: FabricDirection,
    /// Weave cells per UV unit; 0 for none.
    pub weave_scale: f32,
    /// How much the weave varies the fabric's opacity, 0 to 1.
    pub weave_strength: f32,
}

impl Default for Stockings {
    /// Sheer black tights: mostly opaque at the silhouette, the skin showing through facing the
    /// camera, a faint cool sheen, no weave.
    fn default() -> Self {
        Self {
            color: [0.012, 0.01, 0.016],
            edge_opacity: 0.92,
            facing_opacity: 0.45,
            range: [0.15, 0.85],
            sheen: [0.16, 0.16, 0.2],
            sheen_sharpness: 12.0,
            direction: FabricDirection::V,
            weave_scale: 0.0,
            weave_strength: 0.25,
        }
    }
}

impl Stockings {
    /// How much of the skin the fabric hides at `n_dot_v`, before the weight, the mask and the
    /// weave: the denier gradient, a smoothstep across `range` (`stocking_opacity` in the anime
    /// character shader).
    pub fn opacity(&self, n_dot_v: f32) -> f32 {
        let [from, to] = self.range;
        let t = ((n_dot_v - from) / (to - from).max(1e-4)).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        self.edge_opacity + (self.facing_opacity - self.edge_opacity) * smooth
    }

    fn validate(&self) -> Result<(), MaterialError> {
        color("stockings.color", &self.color)?;
        unit("stockings.edge_opacity", self.edge_opacity)?;
        unit("stockings.facing_opacity", self.facing_opacity)?;
        let [from, to] = self.range;
        if !((0.0..=1.0).contains(&from) && (0.0..=1.0).contains(&to) && from <= to) {
            return Err(invalid("stockings.range", "must ascend within 0 to 1"));
        }
        color("stockings.sheen", &self.sheen)?;
        if !(self.sheen_sharpness.is_finite() && self.sheen_sharpness >= 1.0) {
            return Err(invalid("stockings.sheen_sharpness", "must be finite and at least 1"));
        }
        non_negative("stockings.weave_scale", self.weave_scale)?;
        unit("stockings.weave_strength", self.weave_strength)
    }
}

/// The UV axis a fabric runs along, which its sheen streaks across.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum FabricDirection {
    U,
    /// Along V: legs laid out top to bottom.
    #[default]
    V,
}

/// Emission (M1 C7; spec §5.12 "Anime character shading"): regions glow in their `emission`
/// color, painted by the layer mask's B, unlit, so accents read the same by day and by night,
/// and breathe: the glow dims and swells over `period` seconds by `depth`, each instance at a
/// phase of its own (WuWa's breath light). It feeds the glow: post's bloom takes an emissive
/// pixel's share of light as the world's rather than as the character's, whose bloom is
/// restrained (M1 P2's character mask), by `bloom`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct Emission {
    /// Seconds a breath takes; 0 holds the glow steady. It runs as whole breaths an hour on the
    /// shaders' clock ([`Emission::breaths`]): a 7 s breath takes 3600 / 514 ≈ 7.004 s.
    pub period: f32,
    /// How far the glow dims at the bottom of a breath: 0 steady, 1 out.
    pub depth: f32,
    /// How much of the glow blooms as the world's light rather than the character's, 0 to 1.
    pub bloom: f32,
}

impl Default for Emission {
    /// A slow breath, dimming by a third, blooming as the world's light.
    fn default() -> Self {
        Self { period: 4.0, depth: 0.35, bloom: 1.0 }
    }
}

impl Emission {
    /// Breaths a second as the GPU block packs them and shaders run them: `1 / period` rounded
    /// to whole breaths per [`SHADER_TIME_PERIOD`](super::SHADER_TIME_PERIOD) ([`whole_cycles`],
    /// review RS-003), so a breath never jumps where the shaders' clock wraps. 0 holds the glow
    /// steady: no period, or one over two hours.
    pub fn breaths(&self) -> f32 {
        if self.period > 0.0 { whole_cycles(1.0 / f64::from(self.period), 1) } else { 0.0 }
    }

    /// The glow's level, 0 to 1, `seconds` into the frame clock for an instance at `phase`
    /// radians, as the anime character shader's `breath` has it: full at the top of each breath,
    /// on the clock wrapped as shaders read it ([`shader_time`]) at [`Emission::breaths`].
    pub fn level(&self, seconds: f32, phase: f32) -> f32 {
        let breaths = self.breaths();
        if breaths <= 0.0 {
            return 1.0;
        }
        let angle = std::f32::consts::TAU * breaths * shader_time(f64::from(seconds)) + phase;
        1.0 - self.depth * (0.5 - 0.5 * angle.cos())
    }

    fn validate(&self) -> Result<(), MaterialError> {
        non_negative("emission.period", self.period)?;
        unit("emission.depth", self.depth)?;
        unit("emission.bloom", self.bloom)
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
/// `shaders/materials.wgsl` in `sapphire_render`). 1584 bytes: a 128-byte header, the regions,
/// then outlines v2's settings (M1 C4), then the rim's, the hair shadow's and the
/// eyes-through-hair settings and the regions' rim settings (M1 C5), then the layers' settings
/// and the regions' layers (M1 C7), each after the last so every earlier offset holds.
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
    /// highlight reveal. Hair: x highlight strength, y width, z shift, w transmission strength
    /// (M1 C8). Eye: x the parallax's depth in meters (M1 C6).
    pub surface: [f32; 4],
    /// Face: rgb blush color. Hair: rgb highlight color, w transmission sharpness (M1 C8, 0 with
    /// no transmission). Eye (M1 C6), by layer: the eye and iris
    /// layers' iris, xy its center and zw its radii in texture coordinates (zero for none); a
    /// highlight's motion, x drift in meters, y its rate, z sparkle, w its rate (the rates as
    /// whole cycles an hour, `character_maps::eye_mask::{drift_rate, sparkle_rate}`).
    pub surface_color: [f32; 4],
    /// x: flags (bit `i` for texture slot `i`, then `FLAG_*`); y: region count; z: surface
    /// (`SURFACE_*`); w: specular model (`SPECULAR_*`), | a hair's highlight style
    /// (`HIGHLIGHT_*`) << [`Self::HIGHLIGHT_STYLE_SHIFT`] | an eye's layer (`EYE_LAYER_*`) <<
    /// [`Self::EYE_LAYER_SHIFT`].
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
    /// A face's hair shadow ([`HairShadow`]): x distance in meters; y strength; z softness in
    /// meters (M1 C8, zero before it, so earlier blocks pack as they did); w: zero.
    pub hair_shadow: [f32; 4],
    /// Eyes through hair ([`ThroughHair`]): x opacity, 0 for none; y, z the cosines of the yaw
    /// and pitch fades; w the distance in meters.
    pub through_hair: [f32; 4],
    /// Per region: x rim strength scale; y rim width scale; z rim on self-overlap; w zero.
    pub rim_regions: [[f32; 4]; AnimeCharacterParams::MAX_REGIONS],
    /// The layers (M1 C7): x the highlight's albedo share ([`Specular::albedo`]); y emission's
    /// breaths a second ([`Emission::breaths`], 0 steady); z the breath's depth; w how much of the
    /// glow blooms as the world's light.
    pub layers: [f32; 4],
    /// Stockings ([`Stockings`]): rgb the fabric's color; a its opacity at the silhouette.
    pub stocking_color: [f32; 4],
    /// x, y: the N·V range from the silhouette to facing; z: the opacity facing the camera; w:
    /// weave cells per UV unit.
    pub stocking_shape: [f32; 4],
    /// rgb: the sheen's color × strength; a: its sharpness.
    pub stocking_sheen: [f32; 4],
    /// x: the weave's strength; y: 1 for fabric along UV v, 0 along u; z, w: zero.
    pub stocking_weave: [f32; 4],
    /// Per region: its layers (M1 C7).
    pub layer_regions: [CharacterLayerRegion; AnimeCharacterParams::MAX_REGIONS],
}

/// One region's layers in [`AnimeCharacterParams`] (M1 C7; WGSL `CharacterLayerRegion`). 48
/// bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct CharacterLayerRegion {
    /// rgb: the matcap's tint ([`RegionMatcap::color`]); a: its channel, −1 none, 0 the color
    /// matcap, 1 to 4 R, G, B or A.
    pub matcap: [f32; 4],
    /// x: metallic; y: the reflection's roughness; z: the stockings' weight; w: the matcap's
    /// blend, 0 added, 1 multiplying the albedo.
    pub surface: [f32; 4],
    /// rgb: emission × intensity; a: zero.
    pub emission: [f32; 4],
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
        let c7 =
            [self.layers, self.stocking_color, self.stocking_shape, self.stocking_sheen, self.stocking_weave];
        let layer_regions = self.layer_regions.iter().flat_map(|r| [r.matcap, r.surface, r.emission]);
        header
            .into_iter()
            .chain(regions)
            .chain(outline)
            .chain(c5)
            .chain(self.rim_regions)
            .chain(c7)
            .chain(layer_regions)
            .collect()
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
    /// The layers (M1 C7), each set when a region turns it on, so the shader skips the rest on
    /// branches every pixel of a draw takes alike: a region has a highlight color;
    pub const FLAG_SPECULAR: u32 = 1 << 12;
    /// the material has a matcap and a region takes it;
    pub const FLAG_MATCAP: u32 = 1 << 13;
    /// a region's metallic is above the reflection's threshold;
    pub const FLAG_REFLECTION: u32 = 1 << 14;
    /// a region wears the stockings;
    pub const FLAG_STOCKINGS: u32 = 1 << 15;
    /// a region glows.
    pub const FLAG_EMISSION: u32 = 1 << 16;
    /// Every layer's flag.
    pub const FLAG_LAYERS: u32 = Self::FLAG_SPECULAR
        | Self::FLAG_MATCAP
        | Self::FLAG_REFLECTION
        | Self::FLAG_STOCKINGS
        | Self::FLAG_EMISSION;

    pub const SURFACE_BODY: u32 = 0;
    pub const SURFACE_FACE: u32 = 1;
    pub const SURFACE_HAIR: u32 = 2;
    pub const SURFACE_EYE: u32 = 3;

    pub const SPECULAR_BLINN: u32 = 0;
    pub const SPECULAR_GGX: u32 = 1;

    pub const HIGHLIGHT_PAINTED_BAND: u32 = 0;
    pub const HIGHLIGHT_ANGEL_RING: u32 = 1;
    pub const HIGHLIGHT_KAJIYA_KAY: u32 = 2;
    /// Where a hair material's highlight style sits in `modes.w`.
    pub const HIGHLIGHT_STYLE_SHIFT: u32 = 8;

    /// An eye material's layer ([`EyeLayer`], M1 C6), in `modes.w` from
    /// [`Self::EYE_LAYER_SHIFT`]: also the order layers draw in.
    pub const EYE_LAYER_EYE: u32 = 0;
    pub const EYE_LAYER_SHADOW: u32 = 1;
    pub const EYE_LAYER_IRIS: u32 = 2;
    pub const EYE_LAYER_HIGHLIGHT: u32 = 3;
    /// Where an eye material's layer sits in `modes.w`, eight bits wide.
    pub const EYE_LAYER_SHIFT: u32 = 16;

    /// The eye layer a block with `modes` draws: an eye's layer, 0 (the eye itself) for any other
    /// surface.
    pub const fn eye_layer(modes: [u32; 4]) -> u32 {
        if modes[2] == Self::SURFACE_EYE { (modes[3] >> Self::EYE_LAYER_SHIFT) & 0xff } else { 0 }
    }
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
        // The surface's settings, and its bits of `modes.w` above the specular model: a hair
        // highlight's style, an eye's layer.
        let (surface_kind, surface, surface_color, style) = match &self.surface {
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
                let (transmission, sharpness) = hair.transmission.gpu();
                (
                    AnimeCharacterParams::SURFACE_HAIR,
                    [h.strength, h.width, h.shift, transmission],
                    rgb_w(h.color, sharpness),
                    style << AnimeCharacterParams::HIGHLIGHT_STYLE_SHIFT,
                )
            }
            CharacterSurface::Eye(eye) => {
                let (surface, color) = eye.gpu();
                let layer = eye.layer.index() << AnimeCharacterParams::EYE_LAYER_SHIFT;
                (AnimeCharacterParams::SURFACE_EYE, surface, color, layer)
            }
        };
        let (hair_shadow, through_hair) = match &self.surface {
            CharacterSurface::Face(face) => {
                let shadow = face.hair_shadow;
                let packed = [shadow.distance, shadow.strength, shadow.softness, 0.0];
                (packed, ThroughHair::gpu(face.through_hair.as_ref()))
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
        flags |= self.layer_flags();
        let mut layer_regions = [CharacterLayerRegion::default(); AnimeCharacterParams::MAX_REGIONS];
        for (layers, region) in layer_regions.iter_mut().zip(&self.regions) {
            let blend = match region.matcap.blend {
                MatcapBlend::Add => 0.0,
                MatcapBlend::Multiply => 1.0,
            };
            *layers = CharacterLayerRegion {
                matcap: rgb_w(region.matcap.color, region.matcap.channel.code()),
                surface: [region.metallic, region.roughness, region.stocking, blend],
                emission: rgb_w(region.emission, 0.0),
            };
        }
        let (stockings, emission) = (&self.stockings, &self.emission);
        let breaths = emission.breaths();
        let along_v = match stockings.direction {
            FabricDirection::U => 0.0,
            FabricDirection::V => 1.0,
        };
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
            modes: [flags, self.regions.len() as u32, surface_kind, specular_model | style],
            swizzles: pack_swizzles(swizzles),
            regions,
            outline_curve: [curve.near.0, curve.mid, curve.far.0, curve.cutoff],
            outline_shape: [curve.near.1, curve.far.1, outline.model_scale, outline.flatten],
            outline_color: [outline.albedo, outline.lighting, 0.0, 0.0],
            rim: [self.rim.threshold, self.rim.fresnel, self.rim.light_gate, self.rim.back_light],
            hair_shadow,
            through_hair,
            rim_regions,
            layers: [self.specular.albedo, breaths, emission.depth, emission.bloom],
            stocking_color: rgb_w(stockings.color, stockings.edge_opacity),
            stocking_shape: [
                stockings.range[0],
                stockings.range[1],
                stockings.facing_opacity,
                stockings.weave_scale,
            ],
            stocking_sheen: rgb_w(stockings.sheen, stockings.sheen_sharpness),
            stocking_weave: [stockings.weave_strength, along_v, 0.0, 0.0],
            layer_regions,
        }
    }

    /// The layers' flags (M1 C7, [`AnimeCharacterParams::FLAG_LAYERS`]): each layer a region
    /// turns on. A layer every region leaves off sets none, so materials from before the layers
    /// draw exactly as they did.
    fn layer_flags(&self) -> u32 {
        let any = |on: &dyn Fn(&CharacterRegion) -> bool| self.regions.iter().any(on);
        let lit = |rgb: [f32; 3]| rgb.iter().any(|&c| c > 0.0);
        let matcap = self.matcap.is_some_and(|m| m.strength > 0.0 || m.shadow_strength > 0.0)
            && any(&|r| r.matcap.channel != MatcapChannel::Off);
        let stockings = self.stockings.edge_opacity > 0.0
            || self.stockings.facing_opacity > 0.0
            || lit(self.stockings.sheen);
        [
            (any(&|r| lit(r.specular_color)), AnimeCharacterParams::FLAG_SPECULAR),
            (matcap, AnimeCharacterParams::FLAG_MATCAP),
            (any(&|r| r.metallic > layer_mask::REFLECTS), AnimeCharacterParams::FLAG_REFLECTION),
            (stockings && any(&|r| r.stocking > 0.0), AnimeCharacterParams::FLAG_STOCKINGS),
            (any(&|r| lit(r.emission)), AnimeCharacterParams::FLAG_EMISSION),
        ]
        .into_iter()
        .filter(|&(on, _)| on)
        .fold(0, |flags, (_, flag)| flags | flag)
    }

    /// Whether it draws anything through the character's hair: a face with a mask (whose B
    /// says which parts) that isn't transparent, or any eye layer, with `through_hair` set and
    /// some opacity.
    fn draws_through_hair(&self) -> bool {
        // A transparent face doesn't write depth or the character prepass; an eye's overlay
        // layers blend by design, over their eye drawn through first (M1 C6).
        let through = match &self.surface {
            CharacterSurface::Face(face)
                if face.mask_map.is_some() && !matches!(self.alpha, AlphaMode::Blend) =>
            {
                face.through_hair
            }
            CharacterSurface::Eye(eye) => eye.through_hair,
            _ => None,
        };
        through.is_some_and(|t| t.opacity > 0.0)
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
            color(&field("matcap.color"), &region.matcap.color)?;
            unit(&field("metallic"), region.metallic)?;
            unit(&field("roughness"), region.roughness)?;
            unit(&field("stocking"), region.stocking)?;
            color(&field("emission"), &region.emission)?;
        }
        self.rim.validate()?;
        non_negative("specular.softness", self.specular.softness)?;
        unit("specular.albedo", self.specular.albedo)?;
        if let Some(matcap) = &self.matcap {
            non_negative("matcap.strength", matcap.strength)?;
            non_negative("matcap.shadow_strength", matcap.shadow_strength)?;
        }
        self.outline.validate()?;
        self.stockings.validate()?;
        self.emission.validate()?;
        if self.layer_mask.is_some() && self.surface != CharacterSurface::Body {
            return Err(invalid(
                "layer_mask",
                "is for bodies: faces, hair and eyes bind their own masks there",
            ));
        }
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
                non_negative("surface.hair_shadow.softness", face.hair_shadow.softness)?;
                face.through_hair.map_or(Ok(()), |t| t.validate("surface.through_hair"))
            }
            CharacterSurface::Hair(hair) => {
                let h = &hair.highlight;
                color("surface.highlight.color", &h.color)?;
                non_negative("surface.highlight.strength", h.strength)?;
                non_negative("surface.highlight.width", h.width)?;
                finite("surface.highlight.shift", h.shift)?;
                let transmission = &hair.transmission;
                non_negative("surface.transmission.strength", transmission.strength)?;
                if transmission.sharpness.is_finite() && transmission.sharpness > 0.0 {
                    Ok(())
                } else {
                    Err(invalid("surface.transmission.sharpness", "must be finite and above 0"))
                }
            }
            CharacterSurface::Eye(eye) => eye.validate(self.alpha),
        }
    }

    fn textures(&self) -> Vec<Option<AssetId>> {
        let ramp = match &self.diffuse {
            DiffuseMode::Ramp(ramp) => Some(ramp.map),
            DiffuseMode::Bands(_) => None,
        };
        let (face_shadow, surface_mask, expression) = match &self.surface {
            CharacterSurface::Body => (None, self.layer_mask, None),
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
        if self.layer_flags() != 0 {
            flags |= MaterialFlags::LAYERS;
        }
        // Blended surfaces draw unsorted, so both faces of one would blend in whatever order the
        // mesh lists them: they keep culling.
        if self.double_sided && !matches!(self.alpha, AlphaMode::Blend) {
            flags |= MaterialFlags::DOUBLE_SIDED;
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
            layer_mask: None,
            stockings: Stockings::default(),
            emission: Emission::default(),
            double_sided: false,
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
