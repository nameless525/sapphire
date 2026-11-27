//! Painted effect sheets (M6 N4a): a `<name>.sheet.ron` document beside its painting, `<name>.sheet.png`,
//! cooked to `.ssheet` as the effects atlas reads it ([`sapphire_assets::effect_sheet`]).
//!
//! ```ron
//! (
//!     type: "sapphire::pipeline::EffectSheetDocument",
//!     version: 1,
//!     data: (
//!         image: "smoke_puff.sheet.png",
//!         columns: 4,
//!         rows: 4,
//!         frames: 16,
//!         timing: PerSecond(rate: 12.0, looped: false),
//!     ),
//! )
//! ```
//!
//! The painting is a grid of `columns` × `rows` cells of [`LAYER_SIZE`]² texels, frames in reading order, of
//! which the first `frames` play, as grey and alpha: the value a gradient will colour (N4b), and coverage. Its
//! name ends in `.sheet.png`, which no cook takes as a texture of its own: it's read through its document only
//! ([`is_image`]), and keyed by its bytes. Each frame's value is spread into its empty texels (`dilate`), so
//! its lower levels keep a shape's value to its edge, and it cooks as a mask of two channels with its whole mip chain
//! (BC5 on the desktop cook, ASTC 4×4 on the mobile one, [`texture`]'s mask row), frames encoded on up to a
//! cook's threads, each frame's levels on one: the same bytes on any number ([`cook_on`]).
//!
//! [`EffectSheetDocument::problem`] names each wrong field, for the cook and for the content check alike
//! (`sapphire_pipeline check`'s `sheet`); the painting's size and channels are the cook's to hold.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::thread;

use bevy_reflect::TypePath;
use sapphire_assets::effect_sheet::{
    EffectSheet, EffectSheetError, LAYER_LEVELS, LAYER_SIZE, MAX_GRID, SheetTiming,
};
use sapphire_assets::texture::AstcBlock;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;
use crate::cook::Cook;
use crate::import::image::{ChannelLayout, SourceImage};
use crate::texture::{self, MaskChannels, MaskSettings, TextureClass, TextureError, TextureSettings};

/// Reads and checks the document. Bump the version whenever that changes output for the same document.
pub const IMPORTER: Stage = Stage { name: "effect sheet document", version: 1 };

/// Cuts the painting into layers and encodes them. Bump the version whenever that changes output for the same
/// document and painting; the texture cooker's version is in its name, since its encoders write the layers.
pub const COOKER: Stage = Stage { name: "ssheet (on the ktx2 cooker's encoders, version 6)", version: 1 };

/// What a sheet's document's file name ends with.
pub const SOURCE_SUFFIX: &str = ".sheet.ron";

/// What a sheet's painting's file name ends with.
pub const IMAGE_SUFFIX: &str = ".sheet.png";

/// Import settings for effect sheets: none, since the document says everything. A struct, so settings can join
/// without changing existing sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectSheetSettings {}

/// A sheet's document (`sapphire::pipeline::EffectSheetDocument`, version 1): see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct EffectSheetDocument {
    /// The painting, beside the document: a file name ending in [`IMAGE_SUFFIX`].
    pub image: String,
    /// The grid's cells across and down, 1 to [`MAX_GRID`].
    pub columns: u32,
    pub rows: u32,
    /// How many cells, from the first in reading order, are frames.
    pub frames: u32,
    /// How its frames turn over a particle's life.
    pub timing: SheetTiming,
}

impl Versioned for EffectSheetDocument {
    /// 1: M6 N4a.
    const VERSION: u32 = 1;
}

/// A field of a sheet's document that's wrong, and why.
pub type Problem = (&'static str, &'static str);

/// A sheet that can't cook.
#[derive(Debug, thiserror::Error)]
pub enum SheetCookError {
    #[error("the document isn't UTF-8")]
    Encoding(#[from] std::str::Utf8Error),
    /// The document doesn't read. Boxed, since a document's error is large.
    #[error(transparent)]
    Document(Box<DocumentError>),
    #[error("`{field}` {problem}")]
    Invalid { field: &'static str, problem: &'static str },
    #[error("its painting")]
    Image(#[source] Box<TextureError>),
    #[error(
        "its painting is {width}x{height}, and a grid of {columns}x{rows} frames of {LAYER_SIZE} needs {}x{}",
        columns * LAYER_SIZE,
        rows * LAYER_SIZE
    )]
    ImageSize { width: u32, height: u32, columns: u32, rows: u32 },
    #[error("its painting is {0:?}: a sheet is painted in grey and alpha")]
    Channels(ChannelLayout),
    #[error(transparent)]
    Cooked(#[from] EffectSheetError),
}

impl From<DocumentError> for SheetCookError {
    fn from(error: DocumentError) -> Self {
        Self::Document(Box::new(error))
    }
}

impl From<TextureError> for SheetCookError {
    fn from(error: TextureError) -> Self {
        Self::Image(Box::new(error))
    }
}

impl EffectSheetDocument {
    /// Reads a document and checks it ([`Self::problem`]).
    pub fn read(bytes: &[u8]) -> Result<Self, SheetCookError> {
        let document: Self = sapphire_serialize::from_ron(std::str::from_utf8(bytes)?)?;
        document.problem().map_err(|(field, problem)| SheetCookError::Invalid { field, problem })?;
        Ok(document)
    }

    /// The first field that's wrong, named, and why: what the cook refuses and the content check reports.
    pub fn problem(&self) -> Result<(), Problem> {
        let name = Path::new(&self.image).file_name().and_then(|name| name.to_str());
        if name != Some(self.image.as_str()) || !is_image(Path::new(&self.image)) {
            return Err(("image", "must be a file beside the document whose name ends in `.sheet.png`"));
        }
        if !(1..=MAX_GRID).contains(&self.columns) {
            return Err(("columns", "must be 1 to 8"));
        }
        if !(1..=MAX_GRID).contains(&self.rows) {
            return Err(("rows", "must be 1 to 8"));
        }
        if !(1..=self.columns * self.rows).contains(&self.frames) {
            return Err(("frames", "must be 1 to the grid's cells, `columns` times `rows`"));
        }
        if let SheetTiming::PerSecond { rate, .. } = self.timing
            && !(rate.is_finite() && rate > 0.0)
        {
            return Err(("timing", "a rate must be above 0"));
        }
        Ok(())
    }

    /// Its painting's path, for a document at `path`.
    pub fn image_path(&self, path: &Path) -> PathBuf {
        path.with_file_name(&self.image)
    }
}

/// Whether `path` names a sheet's document.
pub fn is_source(path: &Path) -> bool {
    ends_with(path, SOURCE_SUFFIX)
}

/// Whether `path` names a sheet's painting, which only its document reads.
pub fn is_image(path: &Path) -> bool {
    ends_with(path, IMAGE_SUFFIX)
}

fn ends_with(path: &Path, suffix: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|name| name.len() > suffix.len() && name.ends_with(suffix))
}

/// The cooked file a document at `relative` writes: `smoke_puff.sheet.ron` → `smoke_puff.ssheet`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// How a frame cooks: a mask of two channels, value and coverage, with its whole chain.
fn frame_settings() -> TextureSettings {
    TextureSettings::from(TextureClass::Mask(MaskSettings {
        mips: true,
        channels: MaskChannels::Two,
        astc_block: AstcBlock::B4x4,
    }))
}

/// About what cooking a sheet holds at its peak: its painting at a texture cook's rate, which covers the
/// frames in flight.
pub fn cook_memory(document: &EffectSheetDocument) -> u64 {
    let texels = u64::from(document.columns * LAYER_SIZE) * u64::from(document.rows * LAYER_SIZE);
    texels.saturating_mul(texture::COOK_BYTES_PER_TEXEL)
}

/// Cooks a checked document and its painting's bytes for `cook`, its frames on as many threads as the machine
/// has.
pub fn cook(document: &EffectSheetDocument, image: &[u8], cook: Cook) -> Result<Vec<u8>, SheetCookError> {
    cook_on(document, image, cook, thread::available_parallelism().map_or(1, usize::from))
}

/// [`cook`], its frames on up to `threads` threads: the same bytes on any number.
pub fn cook_on(
    document: &EffectSheetDocument,
    image: &[u8],
    cook: Cook,
    threads: usize,
) -> Result<Vec<u8>, SheetCookError> {
    let settings = frame_settings();
    let painting = texture::import(Path::new(&document.image), image, &settings)?;
    if painting.layout != ChannelLayout::GrayAlpha {
        return Err(SheetCookError::Channels(painting.layout));
    }
    let (columns, rows) = (document.columns, document.rows);
    if (painting.width, painting.height) != (columns * LAYER_SIZE, rows * LAYER_SIZE) {
        let (width, height) = (painting.width, painting.height);
        return Err(SheetCookError::ImageSize { width, height, columns, rows });
    }
    let frames: Vec<u32> = (0..document.frames).collect();
    let chunk = frames.len().div_ceil(threads.clamp(1, frames.len()));
    let encode = |frames: &[u32]| -> Result<Vec<u8>, SheetCookError> {
        let mut bytes = Vec::new();
        for &frame in frames {
            let mut cell = cut(&painting, columns, frame);
            dilate(&mut cell);
            let layer = texture::prepare(&cell, &settings, cook)?.encode_on(1)?;
            debug_assert_eq!(layer.levels.len(), LAYER_LEVELS as usize, "a mask's whole chain");
            layer.levels.iter().for_each(|level| bytes.extend_from_slice(level));
        }
        Ok(bytes)
    };
    let data = if chunk >= frames.len() {
        encode(&frames)?
    } else {
        thread::scope(|scope| {
            let encode = &encode;
            let chunks: Vec<_> =
                frames.chunks(chunk).map(|frames| scope.spawn(move || encode(frames))).collect();
            let mut data = Vec::new();
            for chunk in chunks {
                data.extend(chunk.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))?);
            }
            Ok::<_, SheetCookError>(data)
        })?
    };
    let format = texture::plan(&settings.class, cook, ChannelLayout::GrayAlpha)?.format;
    let sheet = EffectSheet {
        columns,
        rows,
        frames: document.frames,
        timing: document.timing,
        format: format.vk_format(),
        data,
    };
    let cooked = sheet.to_cooked().map_err(EffectSheetError::from)?;
    // The runtime's checks, so a cook never writes a sheet the atlas would refuse.
    EffectSheet::read(&cooked)?;
    Ok(cooked.into_vec())
}

/// Spreads each covered texel's value into the uncovered texels about it, nearest first (a breadth-first
/// fill, four neighbours, in texel order), and leaves coverage as it is. A data mask's levels average value
/// and coverage apart, so a rim's empty texels would pull its value toward whatever they hold, 0 in a
/// painting, darkening every shape's edge at the lower levels (Mobile-Low's atlas starts at 128²). Values
/// are copied, never mixed, so it's the same on every machine. A frame with no coverage is left as it is.
fn dilate(frame: &mut SourceImage) {
    let width = frame.width as usize;
    let mut filled: Vec<bool> = frame.pixels.iter().map(|pixel| pixel[3] > 0.0).collect();
    let mut queue: VecDeque<usize> = (0..filled.len()).filter(|&i| filled[i]).collect();
    while let Some(i) = queue.pop_front() {
        let value = frame.pixels[i][0];
        let (x, y) = (i % width, i / width);
        let around = [
            (x > 0).then(|| i - 1),
            (x + 1 < width).then(|| i + 1),
            (y > 0).then(|| i - width),
            (i + width < filled.len()).then(|| i + width),
        ];
        for next in around.into_iter().flatten() {
            if !filled[next] {
                filled[next] = true;
                let alpha = frame.pixels[next][3];
                frame.pixels[next] = [value, value, value, alpha];
                queue.push_back(next);
            }
        }
    }
}

/// Frame `frame` of a painting `columns` cells across, as an image of its own.
fn cut(painting: &SourceImage, columns: u32, frame: u32) -> SourceImage {
    let (left, top) = ((frame % columns) * LAYER_SIZE, (frame / columns) * LAYER_SIZE);
    let pixels = (top..top + LAYER_SIZE)
        .flat_map(|y| {
            let row = (y * painting.width + left) as usize;
            painting.pixels[row..row + LAYER_SIZE as usize].iter().copied()
        })
        .collect();
    SourceImage::new(LAYER_SIZE, LAYER_SIZE, painting.layout, pixels).expect("a frame is a whole layer")
}

pub mod paint;

#[cfg(test)]
mod tests;
