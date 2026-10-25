//! What the texels of the anime character maps mean (M1 A7): ILM maps, ramps and SDF face
//! maps.
//!
//! The maps are ordinary cooked textures ([`texture`](crate::texture)) of the ILM, ramp and
//! SDF classes. This module fixes their channel and row layouts and the encodings inside
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
//! | G | Shadow bias: shifts the two-tone light split; ≥ 0.95 stays lit, ≤ 0.05 stays in shadow | 0.5, no shift |
//! | B | Specular threshold: the highlight's size | 0.5 |
//! | A | [`MaterialId`]: picks the ramp rows and the per-ID parameters | ID 0 |
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
//! never lit, 1 is lit until the light is directly behind. The shader takes the light's angle
//! from its direction projected into head space; equal steps of angle are equal steps of
//! value, so masks can be authored at any angles. Maps are authored for light on the head's
//! right; light on its left samples them mirrored (`u → 1 − u`), which needs face UVs that are
//! mirror-symmetric about `u = 0.5`.

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

/// SDF face-map values: see the [module docs](self).
pub mod face_sdf {
    /// The light angle a value of 1 stands for, in degrees: directly behind the head.
    pub const MAX_ANGLE_DEGREES: f32 = 180.0;

    /// The stored value of a light angle in degrees, clamped to `[0, 1]`.
    pub fn encode_angle(degrees: f32) -> f32 {
        (degrees / MAX_ANGLE_DEGREES).clamp(0.0, 1.0)
    }

    /// Whether a texel holding `value` is lit by a light `degrees` from head-forward.
    pub fn is_lit(value: f32, degrees: f32) -> bool {
        value > encode_angle(degrees)
    }
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
}
