//! A painted effect sheet, cooked (`.ssheet`, M6 N4a, spec §5.16): a flipbook of frames an effects library's
//! emitters name, laid out as the effects atlas reads it.
//!
//! A sheet's source is a document beside a painted image, a grid of frames in reading order
//! (`<name>.sheet.ron` naming `<name>.sheet.png`, the pipeline's `effect_sheet`). The cook cuts each frame
//! into a **layer** of [`LAYER_SIZE`]² texels with its whole mip chain ([`LAYER_LEVELS`] levels), encoded in
//! the cook's one sheet format: BC5 on the desktop cook, ASTC 4×4 on the mobile one, two channels each, the
//! sheet's value (grey, which a gradient colours) and its coverage. Every layer of every sheet has the same
//! shape and format on a cook, so the effects atlas (N4b) is one texture array a library: its sheets' layers
//! one after another, each sheet's at the first layer of its own, with no repacking at load. The cooked
//! bytes are already that array's data, layer by layer and each layer's levels from the top down.
//!
//! # The atlas's budget, and each tier's levels
//!
//! A layer holds [`LAYER_BYTES`] (85.4 KiB) at its whole chain, the same in both formats. A library's
//! atlas holds at most [`ATLAS_BUDGET_DESKTOP`] (32 MiB, 383 layers) on the desktop cook and
//! [`ATLAS_BUDGET_MOBILE`] (8 MiB, 95 layers) on the mobile cook, counted at the whole chain
//! ([`atlas_bytes`]); the content check holds every library to it (`sapphire_pipeline check`'s
//! `atlas-budget`), since a sheet can't know what else its library names. Which levels a tier's atlas
//! takes, from the top ([`AtlasTop`]): Desktop-High, Desktop-Low and Mobile-High take every level, from
//! 256²; Mobile-Low, whose frame is at most 590 rows, takes them from 128² down, a quarter of the bytes
//! ([`AtlasTop::bytes`]), so a library at the phones' budget is 2 MiB there.
//!
//! Block-encoded, a sheet's bytes differ between architectures as any texture's do (x86-64 and AArch64
//! encode with kernels of their own), and are the same on every machine of one: CI's cooks-agree check
//! counts sheets with textures and their levels.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use crate::texture::{AstcBlock, TextureFormat, full_mip_count};

/// Conventional file extension for cooked effect sheets.
pub const SHEET_EXTENSION: &str = "ssheet";

/// A layer's side in texels: every frame of every sheet is one.
pub const LAYER_SIZE: u32 = 256;

/// A layer's levels, from [`LAYER_SIZE`] down to 1.
pub const LAYER_LEVELS: u32 = full_mip_count(LAYER_SIZE, LAYER_SIZE);

/// The most columns or rows a sheet's grid has: an 8 × 8 sheet is 2048² of painting.
pub const MAX_GRID: u32 = 8;

/// The bytes of one layer at its whole chain, in either cook's format: 87,408.
pub const LAYER_BYTES: u64 = layer_bytes(AtlasTop::Whole);

/// The most a library's atlas holds on the desktop cook, at the whole chain: the desktop tiers' row.
pub const ATLAS_BUDGET_DESKTOP: u64 = 32 << 20;

/// The most a library's atlas holds on the mobile cook, at the whole chain: Mobile-High's row, which
/// Mobile-Low's quarter meets too.
pub const ATLAS_BUDGET_MOBILE: u64 = 8 << 20;

/// The format a cook's sheets are encoded in: two channels, value and coverage. BC5 holds them in R and G;
/// ASTC as luminance and alpha, which a two-channel mask's swizzle (`ra01`) reads back as R and G: the
/// atlas's view (N4b) applies it on the mobile cook.
pub const fn sheet_format(mobile: bool) -> TextureFormat {
    if mobile {
        TextureFormat::Astc { block: AstcBlock::B4x4, srgb: false }
    } else {
        TextureFormat::Bc5RgUnorm
    }
}

/// Which of a layer's levels a tier's atlas takes, from the top (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasTop {
    /// Every level, from [`LAYER_SIZE`]²: Desktop-High, Desktop-Low and Mobile-High.
    Whole,
    /// From half of [`LAYER_SIZE`] down: Mobile-Low.
    Half,
}

impl AtlasTop {
    /// The first level it takes.
    pub const fn first_level(self) -> u32 {
        match self {
            Self::Whole => 0,
            Self::Half => 1,
        }
    }

    /// What `layers` layers cost at this top.
    pub const fn bytes(self, layers: u64) -> u64 {
        layers * layer_bytes(self)
    }
}

/// One layer's bytes from `top` down, in 16-byte blocks of 4 × 4 texels (BC5's and ASTC 4×4's alike).
const fn layer_bytes(top: AtlasTop) -> u64 {
    let mut level = top.first_level();
    let mut bytes = 0;
    while level < LAYER_LEVELS {
        let side = LAYER_SIZE >> level;
        let blocks = side.div_ceil(4) as u64;
        bytes += blocks * blocks * 16;
        level += 1;
    }
    bytes
}

/// What `layers` layers of a library cost in its atlas at the whole chain, against its cook's budget.
pub const fn atlas_bytes(layers: u64) -> u64 {
    AtlasTop::Whole.bytes(layers)
}

/// How a sheet's frames turn over a particle's life.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug))]
pub enum SheetTiming {
    /// Every frame once over the particle's life, evenly.
    Life,
    /// `rate` frames a second on the owner's clock, from the first again if `looped`, or holding the last:
    /// 12 is a drawing on twos, 8 on threes.
    PerSecond { rate: f32, looped: bool },
}

/// A painted effect sheet, cooked. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct EffectSheet {
    /// The painting's grid, frames in reading order: what a tool shows it as.
    pub columns: u32,
    pub rows: u32,
    /// How many of the grid's cells are frames, from the first: its layers.
    pub frames: u32,
    pub timing: SheetTiming,
    /// The layers' format, as its `VkFormat` value (KTX2's numbering, [`TextureFormat::vk_format`]): the
    /// cook's [`sheet_format`].
    pub format: u32,
    /// Every layer, in frame order, each its [`LAYER_LEVELS`] levels from the top down, tightly packed.
    pub data: Vec<u8>,
}

impl Versioned for EffectSheet {
    /// 1: M6 N4a.
    const VERSION: u32 = 1;
}

/// A cooked sheet that can't be read, or breaks an invariant of the format.
#[derive(Debug, thiserror::Error)]
pub enum EffectSheetError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("VkFormat {0} is no cook's sheet format (BC5 or ASTC 4x4)")]
    Format(u32),
    #[error("a {columns}x{rows} grid: columns and rows must be 1 to {MAX_GRID}")]
    Grid { columns: u32, rows: u32 },
    #[error("{frames} frames in a {columns}x{rows} grid: it holds 1 to {}", columns * rows)]
    Frames { frames: u32, columns: u32, rows: u32 },
    #[error("a rate of {0} frames a second: it must be above 0")]
    Rate(f32),
    #[error("{found} bytes of layers, expected {expected}")]
    DataSize { expected: u64, found: u64 },
}

impl EffectSheet {
    /// The sheet as cooked bytes: the envelope header, then the archive.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked sheet bytes in place and checks them ([`ArchivedEffectSheet::validate`]). The bytes must
    /// start 16-byte aligned.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedEffectSheet, EffectSheetError> {
        let sheet = sapphire_serialize::access::<Self>(bytes)?;
        sheet.validate()?;
        Ok(sheet)
    }
}

impl ArchivedEffectSheet {
    /// Checks every invariant in [`EffectSheetError`]. [`EffectSheet::read`] calls it.
    pub fn validate(&self) -> Result<(), EffectSheetError> {
        let format = self.format.to_native();
        if ![sheet_format(false), sheet_format(true)].iter().any(|sheet| sheet.vk_format() == format) {
            return Err(EffectSheetError::Format(format));
        }
        let (columns, rows, frames) = (self.columns.to_native(), self.rows.to_native(), self.frames());
        if !(1..=MAX_GRID).contains(&columns) || !(1..=MAX_GRID).contains(&rows) {
            return Err(EffectSheetError::Grid { columns, rows });
        }
        if !(1..=columns * rows).contains(&frames) {
            return Err(EffectSheetError::Frames { frames, columns, rows });
        }
        if let ArchivedSheetTiming::PerSecond { rate, .. } = &self.timing {
            let rate = rate.to_native();
            if !(rate.is_finite() && rate > 0.0) {
                return Err(EffectSheetError::Rate(rate));
            }
        }
        let (expected, found) = (u64::from(frames) * LAYER_BYTES, self.data.len() as u64);
        if expected != found {
            return Err(EffectSheetError::DataSize { expected, found });
        }
        Ok(())
    }

    /// Its frames: the layers it adds to its library's atlas.
    pub fn frames(&self) -> u32 {
        self.frames.to_native()
    }

    /// Its layers' format.
    pub fn format(&self) -> TextureFormat {
        TextureFormat::from_vk_format(self.format.to_native()).expect("validated when read")
    }

    /// How its frames turn.
    pub fn timing(&self) -> SheetTiming {
        match &self.timing {
            ArchivedSheetTiming::Life => SheetTiming::Life,
            ArchivedSheetTiming::PerSecond { rate, looped } => {
                SheetTiming::PerSecond { rate: rate.to_native(), looped: *looped }
            }
        }
    }

    /// Frame `frame`'s level `level`, `LAYER_SIZE >> level` a side, as its format's blocks.
    pub fn level(&self, frame: u32, level: u32) -> Option<&[u8]> {
        if frame >= self.frames() || level >= LAYER_LEVELS {
            return None;
        }
        let start = u64::from(frame) * LAYER_BYTES + (LAYER_BYTES - layer_bytes_from(level));
        let length = level_bytes(level);
        self.data.get(start as usize..(start + length) as usize)
    }
}

/// One level's bytes: 16-byte blocks of 4 × 4 texels.
fn level_bytes(level: u32) -> u64 {
    let blocks = u64::from((LAYER_SIZE >> level).div_ceil(4));
    blocks * blocks * 16
}

/// A layer's bytes from `level` down.
fn layer_bytes_from(level: u32) -> u64 {
    (level..LAYER_LEVELS).map(level_bytes).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(frames: u32) -> EffectSheet {
        EffectSheet {
            columns: 4,
            rows: 4,
            frames,
            timing: SheetTiming::PerSecond { rate: 12.0, looped: false },
            format: sheet_format(false).vk_format(),
            data: (0..u64::from(frames) * LAYER_BYTES).map(|byte| byte as u8).collect(),
        }
    }

    /// The budgets in the module docs, from the shape: a layer is 87,408 bytes either way, and a library's
    /// atlas holds 383 of them on the desktops and 95 on the phones.
    #[test]
    fn a_layer_and_the_budgets_are_what_the_docs_say() {
        assert_eq!(LAYER_LEVELS, 9);
        assert_eq!(LAYER_BYTES, 87_408);
        for mobile in [false, true] {
            let format = sheet_format(mobile);
            let chain: u64 =
                (0..LAYER_LEVELS).map(|level| format.image_byte_len(256 >> level, 256 >> level)).sum();
            assert_eq!(chain, LAYER_BYTES, "{format:?}");
        }
        assert_eq!(ATLAS_BUDGET_DESKTOP / LAYER_BYTES, 383);
        assert_eq!(ATLAS_BUDGET_MOBILE / LAYER_BYTES, 95);
        assert_eq!(AtlasTop::Half.bytes(1), 21_872, "Mobile-Low's layer, from 128 down");
    }

    #[test]
    fn a_sheet_reads_back_and_hands_out_each_frames_levels() {
        let cooked = sheet(3).to_cooked().expect("cooked");
        let read = EffectSheet::read(&cooked).expect("reads");
        assert_eq!(read.frames(), 3);
        assert_eq!(read.format(), TextureFormat::Bc5RgUnorm);
        assert_eq!(read.timing(), SheetTiming::PerSecond { rate: 12.0, looped: false });
        let data = &sheet(3).data;
        let top = read.level(1, 0).expect("frame 1's top");
        assert_eq!(top, &data[LAYER_BYTES as usize..LAYER_BYTES as usize + 65_536]);
        let last = read.level(2, LAYER_LEVELS - 1).expect("frame 2's 1x1");
        assert_eq!(last, &data[data.len() - 16..]);
        assert!(read.level(3, 0).is_none() && read.level(0, LAYER_LEVELS).is_none());
    }

    #[test]
    fn a_broken_sheet_is_refused_by_what_is_wrong() {
        let refused = |sheet: EffectSheet| {
            let cooked = sheet.to_cooked().expect("cooked");
            EffectSheet::read(&cooked).expect_err("refused").to_string()
        };
        assert!(refused(EffectSheet { format: 0, ..sheet(1) }).contains("VkFormat 0"));
        assert!(refused(EffectSheet { columns: 9, ..sheet(1) }).contains("9x4 grid"));
        assert!(refused(EffectSheet { rows: 0, ..sheet(1) }).contains("4x0 grid"));
        assert!(refused(EffectSheet { frames: 17, ..sheet(1) }).contains("17 frames"));
        assert!(refused(EffectSheet { frames: 0, data: Vec::new(), ..sheet(1) }).contains("0 frames"));
        let still = SheetTiming::PerSecond { rate: 0.0, looped: true };
        assert!(refused(EffectSheet { timing: still, ..sheet(1) }).contains("rate of 0"));
        let short = EffectSheet { frames: 2, ..sheet(1) };
        assert!(refused(short).contains("expected 174816"));
    }
}
