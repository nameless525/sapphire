//! What the texels of the anime character maps mean (M1 A7): ILM maps, ramps, SDF face maps,
//! the face's masks (M1 C3), the hair mask (M1 C5), the eye mask (M1 C6), with the eyes'
//! layered shading that the shader and its tests share, and the layer mask (M1 C7).
//!
//! The maps are ordinary cooked textures ([`texture`](crate::texture)) of the ILM, ramp, SDF
//! and mask classes. This module fixes their channel and row layouts and the encodings inside
//! them, both directions, so the pipeline's map tools (which write them) and the anime
//! character shader (C1 and C3, which read them) can't drift apart, as the mesh element
//! encodings do for vertex data.
//!
//! # ILM maps
//!
//! Linear RGBA, the channel assignment of the reference games' lightmaps (spec §5.12; the
//! research notes' Genshin and Star Rail layouts):
//!
//! | Channel | Holds | Neutral value |
//! |---|---|---|
//! | R | Specular mask: where highlights may appear, and how strongly | 0, no highlight |
//! | G | Shadow bias: an offset to N·L before the two-tone light split, `2·G − 1` ([`ilm::encode_shadow_bias`]); ≥ 0.95 stays lit, ≤ 0.05 stays in shadow ([`ilm::SHADOW_BIAS_LIT`], [`ilm::SHADOW_BIAS_SHADOWED`]) | 0.5, no shift |
//! | B | Specular threshold: the highlight's size | 0.5 |
//! | A | [`MaterialId`]: picks the ramp rows and the per-ID parameters | ID 0 |
//!
//! The shadow bias spans N·L's own range: 0 and 1 stand for offsets of −1 and +1, so a
//! texel at 1 is lit down to N·L = −1 + the split's threshold, and MToon's shading shift
//! texture (`N·L + shift × scale`, M1 A6) is the same quantity. Positive offsets light a texel.
//!
//! Material ID `k` is stored as `k / 7`, evenly spaced with IDs 0 and 7 at exactly 0 and 1.
//! Decoding rounds to the nearest of the eight levels, so it tolerates errors under half a
//! level (1/14 ≈ 0.071); BC7 and ASTC 4×4 stay well inside that. Averaging does not: two IDs
//! blend where they meet under linear filtering, and the ILM class's mips average the ID
//! channel like the others, so a border between IDs 0 and 7 reads as ID 3 or 4 at a
//! distance. The shader therefore reads IDs from level 0 without filtering (`textureLoad`,
//! or a nearest sampler clamped to level 0), or the map is cooked without mips.
//!
//! # Ramps
//!
//! Uncompressed sRGB RGBA, [`ramp::ROWS`] rows of equal height: the day ramps of material IDs
//! 0 to 7 in rows 0 to 7, then their night ramps in rows 8 to 15 (the spec's day/night pair;
//! the shader blends the two by time of day). `u` runs from the deepest shadow (0) to the lit
//! edge of the terminator (1). Rows are sampled at their centers ([`ramp::row_center`]). A
//! row's height is a cook setting (4 texels by default; from 2 up, bilinear filtering at a
//! row's center stays inside the row even with some error in `v`), and the shader needs only
//! the row count.
//!
//! # SDF face maps
//!
//! One or two linear channels. Each texel holds the light angle at which it turns from lit
//! to shadow, as `angle / 180°` ([`face_sdf::encode_angle`]):
//!
//! - **R:** a light turning in the head's horizontal plane, from head-forward (0°) toward
//!   head-right (90°) and on to behind the head (180°).
//! - **G** (two-channel maps): a light rising in the head's forward–up plane, from
//!   head-forward (0°) toward overhead (90°): WuWa's vertical channel for top light.
//!
//! A texel is lit while the light's angle is below its value ([`face_sdf::is_lit`]): 0 is
//! never lit, 1 is lit until the light is directly behind. Equal steps of angle are equal steps
//! of value, so masks can be authored at any angles. Maps are authored for light on the head's
//! right; light on its left samples them mirrored (`u → 1 − u`), which needs face UVs that are
//! mirror-symmetric about `u = 0.5`. A one-channel map reads G as 1 (its texture's swizzle is
//! `r101`, A2's SDF class): nothing on it turns to shadow as the light rises.
//!
//! **Head space** (M1 C3). The shader expresses the light in the head's basis: head-right
//! `R = forward × up` (the character's own right; models face +Z with +Y up, so it is −X at
//! rest), head-up `U` and head-forward `F`. For the unit vector toward the light, with
//! `x = L·R`, `y = L·U`, `z = L·F` ([`face_sdf::HeadLight::new`]):
//!
//! - **The horizontal angle** is the yaw of the light's projection on the head's horizontal
//!   plane, `atan2(|x|, z)`, scaled by `cos(elevation) = |(x, z)|`. In the horizontal plane that
//!   is the yaw itself, so a light turning around the head reproduces the R masks exactly; as
//!   the light rises toward overhead the angle fades to 0, so a light passing over the top
//!   never flips the face from one side to the other.
//! - **The vertical angle** is the light's elevation above the horizontal plane,
//!   `asin(y)`, 0 for a light below it.
//! - **The two channels intersect:** a texel is lit while it is lit by both, with the margin
//!   `min(R − horizontal, G − vertical)` ([`face_sdf::lit_margin`]). A light in the horizontal
//!   plane has a vertical angle of 0, which lights the whole face in G; a light rising straight
//!   in front has a horizontal angle of 0, which lights it in R. So each channel reproduces its
//!   own masks exactly, and a light from above and to the side shadows what either would.
//! - **The mirror** is chosen by the sign of `x`. Within `|x| < MIRROR_BLEND × |y|`
//!   ([`face_sdf::MIRROR_BLEND`]) the R channel blends from the light's side toward the darker
//!   of both sides, `min(R(u), R(1 − u))`, which is symmetric: a light behind and above the
//!   head, whose faded horizontal angle lies between 0 and 180°, darkens the face evenly instead
//!   of lighting one side and flipping to the other as it crosses the mirror. The blend vanishes
//!   in the horizontal plane, where crossing the mirror is seamless anyway (in front both sides
//!   are lit, behind neither), so the R masks hold exactly there.
//! - **G is never mirrored:** the vertical masks are authored for lights in the head's
//!   symmetric forward–up plane, so a painted asymmetry (a parting's shadow) stays in place.
//!
//! # Face masks
//!
//! A face material (C3) binds two masks besides its SDF map, both linear:
//!
//! - **The face mask** (the material's surface mask slot), [`face_mask`]: R the nose line,
//!   G the face highlight, B always lit (eyes, brows, lashes; also what draws through the
//!   bangs, M1 C5); A unused. These are the face's fixed features.
//! - **The expression map** ([`face_expression`]): R blush, G gloom; B and A unused. Each
//!   channel is scaled by a weight the frame packet animates per character, so one material
//!   serves every expression of every character wearing it.
//!
//! Neutral values are 0 everywhere: an unpainted mask changes nothing.
//!
//! # Hair masks
//!
//! A hair material (M1 C5) binds a linear mask in its surface mask slot, [`hair_mask`] (WuWa's
//! `_HM`): R where the highlight may show and how strongly, G a shift of the highlight's band
//! (0.5 none), B occlusion that darkens the hair's light between strands (1 none), A thinness
//! for the light through the hair (M1 C8; 1 thin, which a mask without alpha reads). An unbound
//! mask reads as a full highlight, no shift, no occlusion and thin hair.
//!
//! # Eye masks
//!
//! An eye or iris layer (M1 C6) may bind a linear mask in its surface mask slot, [`eye_mask`]:
//! R the iris's depth below the cornea, from 0 (level with it: the sclera, the limbal ring's
//! edge) to 1 (the material's full parallax depth: the pupil), as WuWa's eye mask paints it. An
//! unbound mask reads as a dome over the material's iris ellipse ([`eye_mask::iris_dome`]), or
//! with no ellipse as the full depth. [`eye_mask`] also holds the highlights' motion, which the
//! shader and its tests compute alike.
//!
//! # Layer masks
//!
//! A body material (M1 C7) may bind a linear mask in its surface mask slot, [`layer_mask`] (the
//! second data map of the reference games: ZZZ's `_OtherDataTex`, Star Rail's stocking range
//! map): R where the region's matcap shows, G the region's metallic (which reflects above
//! [`layer_mask::REFLECTS`]), B where the region glows, A where its stockings cover and how
//! densely. Each channel scales what the material's region sets, so the mask paints the shapes
//! and the regions their looks. An unbound mask reads 1 in every channel: the regions' values
//! alone. Faces, hair and eyes bind their own masks in the slot; their layers take the regions'
//! values alone.

/// How many material IDs the character maps tell apart: ILM alpha levels, ramp rows per half,
/// and the per-ID parameter arrays of the anime character shader (C1).
pub const MATERIAL_IDS: u8 = 8;

/// A material ID of the character maps, below [`MATERIAL_IDS`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaterialId(u8);

impl MaterialId {
    /// Every ID, in order.
    pub const ALL: [Self; MATERIAL_IDS as usize] = {
        let mut all = [Self(0); MATERIAL_IDS as usize];
        let mut id = 0;
        while id < MATERIAL_IDS {
            all[id as usize] = Self(id);
            id += 1;
        }
        all
    };

    /// The ID `id`, or `None` if it isn't below [`MATERIAL_IDS`].
    pub const fn new(id: u8) -> Option<Self> {
        if id < MATERIAL_IDS { Some(Self(id)) } else { None }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// ILM channels: see the [module docs](self).
pub mod ilm {
    use super::{MATERIAL_IDS, MaterialId};

    /// Index of the specular mask channel (R).
    pub const SPECULAR_MASK: usize = 0;
    /// Index of the shadow bias channel (G).
    pub const SHADOW_BIAS: usize = 1;
    /// Index of the specular threshold channel (B).
    pub const SPECULAR_THRESHOLD: usize = 2;
    /// Index of the material ID channel (A).
    pub const MATERIAL_ID: usize = 3;

    /// The texel that changes nothing: no highlight, no shadow shift, a middling highlight size,
    /// material ID 0.
    pub const NEUTRAL: [f32; 4] = [0.0, 0.5, 0.5, 0.0];

    /// The stored shadow bias of an offset to N·L: `0.5 + offset / 2`, clamped to `[0, 1]`
    /// (offsets beyond ±1 move the split past every N·L anyway).
    pub fn encode_shadow_bias(offset: f32) -> f32 {
        (0.5 + 0.5 * offset).clamp(0.0, 1.0)
    }

    /// The offset to N·L a stored (or sampled) shadow bias stands for: `2·value − 1`. A
    /// positive offset lights a texel: the split compares `N·L + offset` with its threshold,
    /// so a shader that moves the threshold instead moves it by `−offset`.
    pub fn decode_shadow_bias(value: f32) -> f32 {
        2.0 * value - 1.0
    }

    /// The shadow bias at and above which a texel stays lit whatever the light.
    pub const SHADOW_BIAS_LIT: f32 = 0.95;
    /// The shadow bias at and below which a texel stays in shadow whatever the light.
    pub const SHADOW_BIAS_SHADOWED: f32 = 0.05;

    /// The step between two material IDs' stored values.
    const STEP: f32 = 1.0 / (MATERIAL_IDS - 1) as f32;

    /// The stored value of `id`.
    pub fn encode_material_id(id: MaterialId) -> f32 {
        f32::from(id.get()) * STEP
    }

    /// The material ID nearest a stored (or sampled) value. Values outside `[0, 1]` clamp to
    /// the first or last ID.
    pub fn decode_material_id(value: f32) -> MaterialId {
        // The clamp keeps the cast in range; NaN saturates to 0.
        let level = (value / STEP).round().clamp(0.0, f32::from(MATERIAL_IDS - 1)) as u8;
        MaterialId(level)
    }
}

/// Ramp rows: see the [module docs](self).
pub mod ramp {
    use super::{MATERIAL_IDS, MaterialId};

    /// Rows in a ramp texture: a day and a night row per material ID.
    pub const ROWS: u32 = 2 * MATERIAL_IDS as u32;

    /// Which ramp of a material's day/night pair.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Half {
        Day,
        Night,
    }

    /// The row holding `id`'s ramp for `half`.
    pub fn row(id: MaterialId, half: Half) -> u32 {
        let base = match half {
            Half::Day => 0,
            Half::Night => u32::from(MATERIAL_IDS),
        };
        base + u32::from(id.get())
    }

    /// The `v` texture coordinate of `row`'s center, where the shader samples it.
    pub fn row_center(row: u32) -> f32 {
        (row as f32 + 0.5) / ROWS as f32
    }
}

/// SDF face-map values, and the light in head space they are compared with: see the
/// [module docs](self).
pub mod face_sdf {
    use std::f32::consts::PI;

    use crate::texture::{Component, Swizzle};

    /// The light angle a value of 1 stands for, in degrees: directly behind the head.
    pub const MAX_ANGLE_DEGREES: f32 = 180.0;

    /// Index of the horizontal channel (R).
    pub const HORIZONTAL: usize = 0;
    /// Index of the vertical channel (G).
    pub const VERTICAL: usize = 1;

    /// The swizzle of a one-channel map: G reads 1, so the vertical channel never shadows.
    pub const ONE_CHANNEL_SWIZZLE: Swizzle =
        Swizzle([Component::R, Component::One, Component::Zero, Component::One]);

    /// How far from the head's forward–up plane the horizontal channel blends from the light's
    /// side toward the darker of both sides (see the [module docs](super)): within
    /// `MIRROR_BLEND × |L·U|` of it, in `|L·R|`. The blend widens as the light leaves the
    /// horizontal plane, where the faded horizontal angle could light one side alone, and
    /// vanishes in it, where the R masks hold exactly.
    pub const MIRROR_BLEND: f32 = 0.25;

    /// The stored value of a light angle in degrees, clamped to `[0, 1]`.
    pub fn encode_angle(degrees: f32) -> f32 {
        (degrees / MAX_ANGLE_DEGREES).clamp(0.0, 1.0)
    }

    /// Whether a texel holding `value` is lit by a light `degrees` from head-forward.
    pub fn is_lit(value: f32, degrees: f32) -> bool {
        value > encode_angle(degrees)
    }

    /// A light in head space, as the face shader compares the map with it.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct HeadLight {
        /// The horizontal channel's angle, as a stored value: the yaw, faded toward 0 as the
        /// light rises or sinks out of the horizontal plane.
        pub horizontal: f32,
        /// The vertical channel's angle, as a stored value: the elevation, 0 below the horizon.
        pub vertical: f32,
        /// The light is on the head's left, so the horizontal channel reads the map mirrored.
        pub mirrored: bool,
        /// How much the horizontal channel reads the light's side alone (1) rather than the
        /// darker of both sides (0): `|L·R| / (MIRROR_BLEND × |L·U|)`, at most 1.
        pub side: f32,
    }

    impl HeadLight {
        /// The light whose unit direction (toward the light) has `toward_light = [L·R, L·U,
        /// L·F]` in the head's basis.
        pub fn new(toward_light: [f32; 3]) -> Self {
            let [x, y, z] = toward_light;
            let level = x.hypot(z);
            // Straight above or below, the yaw is undefined and fades out anyway.
            let yaw = if level > 1e-6 { x.abs().atan2(z) } else { 0.0 };
            Self {
                horizontal: yaw * level / PI,
                vertical: y.clamp(0.0, 1.0).asin() / PI,
                mirrored: x < 0.0,
                // In the horizontal plane only `x = 0` reads both sides, where they agree: in
                // front both are lit, and behind neither.
                side: (x.abs() / (MIRROR_BLEND * y.abs()).max(1e-6)).min(1.0),
            }
        }
    }

    /// How far a texel is lit by `light`, in stored-value units: positive while lit. `r` and
    /// `vertical` are the map's channels at the texel's `u`, `r_mirrored` its R at `1 − u`.
    pub fn lit_margin(light: HeadLight, r: f32, r_mirrored: f32, vertical: f32) -> f32 {
        let (own, other) = if light.mirrored { (r_mirrored, r) } else { (r, r_mirrored) };
        let both = own.min(other);
        let horizontal = both + (own - both) * light.side - light.horizontal;
        horizontal.min(vertical - light.vertical)
    }
}

/// The face mask's channels (M1 C3): the fixed features a face material paints, in its
/// surface mask slot. Linear; 0 changes nothing.
pub mod face_mask {
    /// R, the nose line, painted where it is drawn for a camera on the head's right (on the
    /// nose's far side from it) and mirrored for one on its left. It shows in the region's
    /// outline color where `mask × |sin(turn)|` exceeds the material's nose line threshold,
    /// `turn` being the camera's angle from head-forward around the head: a fully painted line
    /// appears as the head turns from the camera, a fainter one later.
    pub const NOSE_LINE: usize = 0;
    /// G, the face highlight: painted shapes (the nose tip, the cheeks) that brighten the lit
    /// tone and fade out near the shadow's edge, over the material's highlight reveal.
    pub const HIGHLIGHT: usize = 1;
    /// B, always lit: eyes, brows and lashes, which the light split never shadows.
    pub const ALWAYS_LIT: usize = 2;
}

/// The hair mask's channels (M1 C5). Linear.
pub mod hair_mask {
    /// R, the highlight: where the hair's highlight band may show, and how strongly.
    pub const HIGHLIGHT: usize = 0;
    /// G, the highlight's shift: `G − 0.5` moves the band by that much in the highlight style's
    /// own units (N·H, the view-space normal's height, or along the normal), so a painted
    /// gradient bends a straight band to follow a hairstyle. 0.5 is no shift.
    pub const SHIFT: usize = 1;
    /// B, occlusion: multiplies the hair's ambient occlusion, darkening it between strands and
    /// under layers. 1 is none.
    pub const OCCLUSION: usize = 2;
    /// A, thinness (M1 C8): how much of a back light passes through
    /// (`HairShading::transmission`), 1 where the hair is thin (tips, loose strands), 0 where
    /// it's thick. A mask without alpha reads 1.
    pub const THINNESS: usize = 3;

    /// The value of G that leaves the band where it is.
    pub const NO_SHIFT: f32 = 0.5;
}

/// The eye mask's channel (M1 C6), and the shading of the eye's layers that the anime character
/// shader and its tests share: the iris's dome and the highlights' drift and sparkle.
pub mod eye_mask {
    use std::f32::consts::TAU;

    /// R, the iris's depth below the cornea: 0 level with it, 1 the material's full parallax
    /// depth.
    pub const DEPTH: usize = 0;

    /// The drift's second axis turns this many times as fast as its first: a Lissajous figure
    /// of 2 : 3, which wanders rather than circles.
    pub const DRIFT_RATIO: f32 = 1.5;
    /// The sparkle's wave is raised to this power: a short glint in each period rather than a
    /// slow throb.
    pub const SPARKLE_SHARPNESS: i32 = 6;
    /// Instances' phases step by this fraction of a turn per instance slot (the golden ratio's),
    /// so neighbors never glint together.
    pub const PHASE_STEP: f32 = 0.618_034;

    /// The iris's depth, 0 to 1, at `uv` without a mask: a dome over the ellipse at `center` with
    /// half-extents `radius`, 1 at its center and 0 at and past its rim, so the limbal ring
    /// holds still and the pupil shifts most, as through a cornea.
    pub fn iris_dome(uv: [f32; 2], center: [f32; 2], radius: [f32; 2]) -> f32 {
        let q = [(uv[0] - center[0]) / radius[0], (uv[1] - center[1]) / radius[1]];
        (1.0 - (q[0] * q[0] + q[1] * q[1])).clamp(0.0, 1.0)
    }

    /// A highlight instance's phase, in radians, from its instance slot.
    pub fn phase(instance: u32) -> f32 {
        TAU * (instance as f32 * PHASE_STEP).fract()
    }

    /// Where a highlight has drifted, at `seconds` on the frame's clock with `phase`: an offset
    /// across the model's front (X, Y), in meters, `drift` at most on each axis.
    pub fn highlight_drift(drift: f32, rate: f32, seconds: f32, phase: f32) -> [f32; 2] {
        let angle = TAU * rate * seconds + phase;
        [drift * angle.sin(), drift * (DRIFT_RATIO * angle + 1.0).sin()]
    }

    /// How much brighter a highlight is, at `seconds` with `phase`: 1, rising to `1 + sparkle`
    /// in a short glint `rate` times a second.
    pub fn highlight_sparkle(sparkle: f32, rate: f32, seconds: f32, phase: f32) -> f32 {
        let wave = 0.5 + 0.5 * (TAU * rate * seconds + phase).sin();
        1.0 + sparkle * wave.powi(SPARKLE_SHARPNESS)
    }
}

/// The layer mask's channels (M1 C7): what a body material's layers cover, in its surface mask
/// slot. Linear; each scales its region's value, and an unbound mask reads 1 in every channel.
pub mod layer_mask {
    /// R, the matcap: where the region's matcap shows, and how strongly.
    pub const MATCAP: usize = 0;
    /// G, metallic: scales the region's metallic, the reflection's strength. A texel reflects
    /// only above [`REFLECTS`] (WuWa's rule: metallic is how strongly a metal reflects, not a
    /// physical blend toward a conductor), so a painted gradient below it leaves the matcap the
    /// whole metal look.
    pub const METALLIC: usize = 1;
    /// B, emission: where the region glows, and how strongly.
    pub const EMISSION: usize = 2;
    /// A, stocking density: where the region's stockings cover the skin, 0 bare and 1 the full
    /// fabric (a sheer band fading out at a stocking's top, a lace border painted in).
    pub const STOCKING: usize = 3;

    /// The metallic above which a texel adds the reflection (the reflection add, M1 C7).
    pub const REFLECTS: f32 = 0.9;
}

/// The expression map's channels (M1 C3), each scaled by a weight the frame packet animates
/// per character (`sapphire_render::FaceExpression`). Linear; 0 changes nothing.
pub mod face_expression {
    /// R, blush: tints the albedo toward the material's blush color, lit and shadow tones
    /// alike.
    pub const BLUSH: usize = 0;
    /// G, gloom: pushes the face into its shadow tones as ambient occlusion does, toward the
    /// deepest at full weight: the anime "shadowed brow". Always-lit texels stay lit and keep
    /// their ambient light.
    pub const GLOOM: usize = 1;
}

#[cfg(test)]
mod tests {
    use super::ramp::Half;
    use super::*;

    #[test]
    fn material_ids_round_trip_with_room_for_compression_error() {
        assert_eq!(MaterialId::ALL.len(), usize::from(MATERIAL_IDS));
        assert_eq!(MaterialId::new(MATERIAL_IDS), None);
        for id in MaterialId::ALL {
            assert_eq!(MaterialId::new(id.get()), Some(id));
            let value = ilm::encode_material_id(id);
            assert_eq!(ilm::decode_material_id(value), id);
            // Within 0.07 either way, the most BC7 or ASTC should ever move a texel.
            assert_eq!(ilm::decode_material_id(value + 0.07), id, "{id:?}");
            assert_eq!(ilm::decode_material_id(value - 0.07), id, "{id:?}");
            // And as 8-bit storage holds it.
            let stored = (value * 255.0).round() / 255.0;
            assert_eq!(ilm::decode_material_id(stored), id);
        }
        assert_eq!(ilm::encode_material_id(MaterialId::ALL[0]), 0.0);
        assert_eq!(ilm::encode_material_id(MaterialId::ALL[7]), 1.0);
        assert_eq!(ilm::decode_material_id(-1.0).get(), 0);
        assert_eq!(ilm::decode_material_id(2.0).get(), 7);
        assert_eq!(ilm::decode_material_id(f32::NAN).get(), 0);
    }

    #[test]
    fn shadow_bias_spans_n_dot_l() {
        assert_eq!(ilm::encode_shadow_bias(0.0), ilm::NEUTRAL[ilm::SHADOW_BIAS]);
        assert_eq!(ilm::encode_shadow_bias(1.0), 1.0);
        assert_eq!(ilm::encode_shadow_bias(-1.0), 0.0);
        assert_eq!(ilm::encode_shadow_bias(3.0), 1.0, "clamped");
        for offset in [-1.0, -0.4, 0.0, 0.25, 1.0] {
            assert!((ilm::decode_shadow_bias(ilm::encode_shadow_bias(offset)) - offset).abs() < 1e-6);
        }
    }

    #[test]
    fn ramp_rows_put_every_day_row_before_every_night_row() {
        let rows: Vec<u32> = [Half::Day, Half::Night]
            .into_iter()
            .flat_map(|half| MaterialId::ALL.map(|id| ramp::row(id, half)))
            .collect();
        assert_eq!(rows, (0..ramp::ROWS).collect::<Vec<_>>());
        assert_eq!(ramp::row_center(0), 0.5 / 16.0);
        assert_eq!(ramp::row_center(15), 15.5 / 16.0);
    }

    #[test]
    fn sdf_values_are_angles_over_a_half_turn() {
        assert_eq!(face_sdf::encode_angle(0.0), 0.0);
        assert_eq!(face_sdf::encode_angle(90.0), 0.5);
        assert_eq!(face_sdf::encode_angle(270.0), 1.0);
        assert!(face_sdf::is_lit(0.5, 89.0));
        assert!(!face_sdf::is_lit(0.5, 90.0));
        assert!(!face_sdf::is_lit(0.0, 0.0), "0 is never lit");
        assert!(face_sdf::is_lit(1.0, 179.9), "1 is lit until the light is behind");
    }

    /// The unit vector toward a light at `yaw` degrees toward head-right (negative: left) and
    /// `elevation` degrees up, as `[L·R, L·U, L·F]`.
    fn toward(yaw: f32, elevation: f32) -> [f32; 3] {
        let (yaw, elevation) = (yaw.to_radians(), elevation.to_radians());
        [yaw.sin() * elevation.cos(), elevation.sin(), yaw.cos() * elevation.cos()]
    }

    #[test]
    fn head_lights_in_the_horizontal_plane_reproduce_the_masks_exactly() {
        use face_sdf::{HeadLight, lit_margin};
        for degrees in [15.0, 30.0, 60.0, 90.0, 135.0, 170.0] {
            let right = HeadLight::new(toward(degrees, 0.0));
            assert!((right.horizontal - degrees / 180.0).abs() < 1e-6, "{degrees}°");
            assert_eq!((right.vertical, right.mirrored), (0.0, false));
            let left = HeadLight::new(toward(-degrees, 0.0));
            assert!(left.mirrored && (left.horizontal - right.horizontal).abs() < 1e-6);
            for value in [0.05, 0.2, 0.4, 0.6, 0.8, 0.95] {
                // Whatever the mirrored texel holds, and with any vertical value above 0.
                for (mirror, vertical) in [(0.0, 0.01), (1.0, 1.0), (value, 0.5)] {
                    let lit = face_sdf::is_lit(value, degrees);
                    assert_eq!(lit_margin(right, value, mirror, vertical) > 0.0, lit, "{degrees}° {value}");
                    assert_eq!(lit_margin(left, mirror, value, vertical) > 0.0, lit, "mirrored");
                }
            }
        }
    }

    #[test]
    fn head_lights_rising_in_front_reproduce_the_vertical_masks_exactly() {
        use face_sdf::{HeadLight, lit_margin};
        for degrees in [10.0, 45.0, 75.0, 90.0] {
            let light = HeadLight::new(toward(0.0, degrees));
            assert!(light.horizontal.abs() < 1e-6 && (light.vertical - degrees / 180.0).abs() < 1e-6);
            for value in [0.05, 0.2, 0.4, 0.6] {
                // Any texel the horizontal channel lights from the front.
                let lit = face_sdf::is_lit(value, degrees);
                assert_eq!(lit_margin(light, 0.3, 0.7, value) > 0.0, lit, "{degrees}° {value}");
            }
        }
        // Below the horizon only the horizontal channel applies.
        assert_eq!(HeadLight::new(toward(0.0, -40.0)).vertical, 0.0);
    }

    #[test]
    fn head_lights_pass_over_the_top_and_behind_without_flipping_sides() {
        use face_sdf::{HeadLight, lit_margin};
        // Over the top, from the right to the left: the horizontal angle fades to nothing.
        let near_top = [HeadLight::new(toward(90.0, 89.0)), HeadLight::new(toward(-90.0, 89.0))];
        for light in near_top {
            assert!(light.horizontal < 0.01, "{light:?}");
        }
        // A light behind and above the head, on its forward–up plane: the two sides are lit
        // alike, so a texel and its mirror image get the same margin.
        let behind = HeadLight::new(toward(180.0, 50.0));
        assert!((0.3..0.7).contains(&behind.horizontal) && behind.side < 1e-6, "{behind:?}");
        for (r, mirrored) in [(0.9, 0.2), (0.6, 0.55), (0.1, 1.0)] {
            let margin = lit_margin(behind, r, mirrored, 1.0);
            assert!((margin - lit_margin(behind, mirrored, r, 1.0)).abs() < 1e-6);
        }
        // Crossing that plane changes the margin smoothly: no side lights up at once.
        let [a, b] = [0.2, -0.2].map(|yaw: f32| HeadLight::new(toward(180.0 + yaw, 50.0)));
        assert!((lit_margin(a, 0.9, 0.2, 1.0) - lit_margin(b, 0.9, 0.2, 1.0)).abs() < 0.01);
    }

    #[test]
    fn the_iris_dome_peaks_at_its_center_and_highlights_stay_in_bounds() {
        use eye_mask::{highlight_drift, highlight_sparkle, iris_dome, phase};
        let (center, radius) = ([0.5, 0.4], [0.2, 0.3]);
        assert_eq!(iris_dome(center, center, radius), 1.0);
        assert!((iris_dome([0.6, 0.4], center, radius) - 0.75).abs() < 1e-6, "half way out");
        assert!(iris_dome([0.7, 0.4], center, radius) < 1e-6, "the rim holds still");
        assert_eq!(iris_dome([0.5, 0.95], center, radius), 0.0, "past the rim");
        let mut peak: f32 = 1.0;
        for step in 0..2000 {
            let seconds = step as f32 * 0.01;
            let [x, y] = highlight_drift(0.001, 0.3, seconds, phase(7));
            assert!(x.abs() <= 0.001 && y.abs() <= 0.001);
            let sparkle = highlight_sparkle(0.6, 0.4, seconds, phase(7));
            assert!((1.0..=1.6).contains(&sparkle));
            peak = peak.max(sparkle);
        }
        assert!(peak > 1.59, "glints reach their peak: {peak}");
        assert_eq!(highlight_drift(0.0, 0.3, 4.0, 1.0), [0.0, 0.0], "no drift holds it still");
        assert_ne!(phase(1), phase(2), "neighbors glint apart");
    }
}
