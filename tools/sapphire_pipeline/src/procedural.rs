//! Procedural textures (M5 B1): a surface described by a few lines of RON, rasterized by the cook and
//! cooked as one of its maps, so a texture set costs a document in git and megabytes only in a cook.
//!
//! A `<name>.ptex.ron` document ([`ProceduralTexture`]) describes a tiling surface: a pattern of blocks
//! (bricks, tiles, boards) or stones, a fractal grain over it, its colours and its relief. Its sidecar
//! says which of the surface's maps the source cooks to ([`ProceduralSettings`], `importer:
//! ProceduralTexture((map: Normal))`), as a texture's sidecar names its class, and the map cooks as that
//! class's texture does ([`crate::texture`]). The three maps the environment family reads are three
//! sources with one surface, so they agree texel for texel:
//!
//! | `map` | Texture class | What it holds |
//! |---|---|---|
//! | `Color` | Color: BC7 sRGB, ASTC sRGB | The colour stops by each block's tone and the grain, the joints' colour between, darker down a bevel |
//! | `Normal` | Normal: BC5, ASTC | The height's slope times `relief`, in glTF's convention (+Y up the image) |
//! | `Mask` | Mask, four channels: BC7, ASTC | The environment family's mask: R occlusion, G cavity, B specular, A roughness |
//!
//! ```ron
//! (
//!     type: "sapphire::pipeline::ProceduralTexture",
//!     version: 1,
//!     data: (
//!         size: 2048,
//!         seed: 7,
//!         pattern: Blocks((columns: 4, rows: 8, joint: 0.006, stagger: 0.5, bevel: 0.01)),
//!         grain: (scale: 16, octaves: 5, depth: 0.25),
//!         colors: [(0.0, "#7a5a48"), (1.0, "#b08870")],
//!         joint_color: "#4a4440",
//!         relief: 0.02,
//!         roughness: (0.6, 0.95),
//!         specular: 0.2,
//!     ),
//! )
//! ```
//!
//! Lengths (`joint`, `bevel`, `relief`) are fractions of the texture's side, so a surface reads the same
//! at any `size`. Every surface tiles: the grain repeats every `scale` cells across, blocks and stones
//! repeat with the texture, and a running bond's `stagger` times its `rows` must be whole.
//!
//! # Determinism
//!
//! The rasterizer is integer hashes, `floor`, `sqrt` and the four operations, in a fixed order, and colours
//! decode through `sapphire_math::det` (`crate::color`): every machine rasterizes the same texels, which
//! `rasterizes_the_same_texels_on_every_machine` pins, and only the block encoders differ by architecture
//! as any texture's do. Nothing else is read: a document is its own key's only input.

use std::path::{Path, PathBuf};

use bevy_reflect::TypePath;
use sapphire_assets::texture::MAX_TEXTURE_DIMENSION;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::cache::Stage;
use crate::character_maps::ramp::SrgbColor;
use crate::color;
use crate::cook::Cook;
use crate::import::image::{ChannelLayout, SourceImage};
use crate::samples::noise;
use crate::texture::{
    self, ColorSettings, CookedTexture, MaskChannels, MaskSettings, NormalSettings, TextureClass,
    TextureError, TextureSettings,
};

/// The rasterizer, as cache keys know it. Bump the version whenever it changes the texels of a document.
pub const IMPORTER: Stage = Stage { name: "procedural texture", version: 1 };

/// The end of a procedural texture's file name, lowercase.
pub const SOURCE_SUFFIX: &str = ".ptex.ron";

/// The most octaves a grain sums: past this they're finer than any texel of the largest texture.
pub const MAX_OCTAVES: u32 = 12;

/// The field the maps are taken from, per texel: height, tone, joint and grain, four `f32`s.
const FIELD_BYTES_PER_TEXEL: u64 = 16;

/// Whether `path` is a procedural texture, by the end of its file name (in any case).
pub fn is_source(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|name| name.len() > SOURCE_SUFFIX.len() && name.ends_with(SOURCE_SUFFIX))
}

/// Where a procedural texture at `relative` cooks to: `walls/brick.color.ptex.ron` → `walls/brick.color.ktx2`.
pub fn output_path(relative: &Path, extension: &str) -> PathBuf {
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let stem = &name[..name.len().saturating_sub(SOURCE_SUFFIX.len())];
    relative.with_file_name(format!("{stem}.{extension}"))
}

/// A procedural texture document (`sapphire::pipeline::ProceduralTexture`, version 1): see the
/// [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::pipeline"]
#[serde(deny_unknown_fields)]
pub struct ProceduralTexture {
    /// Texels on a side: a multiple of 4, as block-compressed formats need.
    pub size: u32,
    /// Picks the blocks' tones, the stones' places and the grain.
    pub seed: u32,
    pub pattern: Pattern,
    #[serde(default)]
    pub grain: Grain,
    /// The surface's colours by tone, `(tone, "#rrggbb")` from 0 to 1, ascending, blended in linear light.
    pub colors: Vec<(f32, SrgbColor)>,
    /// The joints' colour. A pattern with joints needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint_color: Option<SrgbColor>,
    /// The height from a joint's floor to a block's face, as a fraction of the texture's side: how steep
    /// the normal map's slopes are.
    pub relief: f32,
    /// Roughness from the grain's low to its high: the mask's A.
    pub roughness: (f32, f32),
    /// The specular mask on a block's face, from none to full: the mask's B.
    pub specular: f32,
}

impl Versioned for ProceduralTexture {
    /// 1: M5 B1.
    const VERSION: u32 = 1;
}

/// What a surface is laid out as.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    /// The grain alone: plaster, earth, cloth.
    Plain,
    /// Rows of `columns` blocks, each row shifted by `stagger` of a block from the one above: bricks with
    /// 0.5, tiles with 0, boards with many columns and few rows.
    Blocks(Blocks),
    /// Irregular flagstones, one around each of `cells` × `cells` scattered points.
    Stones(Stones),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocks {
    pub columns: u32,
    pub rows: u32,
    /// The joint's width, as a fraction of the texture's side.
    pub joint: f32,
    #[serde(default)]
    pub stagger: f32,
    /// How far in from a joint a block's face rises to its full height, as a fraction of the side.
    #[serde(default)]
    pub bevel: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stones {
    pub cells: u32,
    /// The joint's width, as a fraction of the texture's side.
    pub joint: f32,
    /// How far in from a joint a stone's face rises to its full height, as a fraction of the side.
    #[serde(default)]
    pub bevel: f32,
}

/// Fractal value noise over the whole surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grain {
    /// Its coarsest octave's cells across the texture.
    pub scale: u32,
    pub octaves: u32,
    /// How much of a face's height it takes away, from none to all.
    pub depth: f32,
}

impl Default for Grain {
    fn default() -> Self {
        Self { scale: 8, octaves: 4, depth: 0.2 }
    }
}

/// Import settings for procedural textures: which of the surface's maps the source cooks to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProceduralSettings {
    pub map: SurfaceMap,
}

impl ProceduralSettings {
    /// The texture settings its map cooks with: its class's defaults.
    pub fn texture(self) -> TextureSettings {
        TextureSettings { class: self.map.class() }
    }
}

/// One of a surface's maps (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SurfaceMap {
    #[default]
    Color,
    Normal,
    Mask,
}

impl SurfaceMap {
    /// Every map, in a stable order.
    pub const ALL: [Self; 3] = [Self::Color, Self::Normal, Self::Mask];

    /// The texture class it cooks as.
    pub fn class(self) -> TextureClass {
        match self {
            Self::Color => TextureClass::Color(ColorSettings::default()),
            Self::Normal => TextureClass::Normal(NormalSettings::default()),
            Self::Mask => {
                TextureClass::Mask(MaskSettings { channels: MaskChannels::Four, ..MaskSettings::default() })
            }
        }
    }
}

/// Why a procedural texture can't cook.
#[derive(Debug, thiserror::Error)]
pub enum ProceduralError {
    #[error("the document isn't valid UTF-8")]
    NotUtf8,
    #[error("not a valid procedural texture")]
    Document(#[source] Box<DocumentError>),
    #[error("`{field}`: {why}")]
    Invalid { field: &'static str, why: String },
    #[error("failed to cook the rasterized map")]
    Texture(#[source] Box<TextureError>),
}

impl ProceduralTexture {
    /// Reads a document.
    pub fn read(bytes: &[u8]) -> Result<Self, ProceduralError> {
        let text = std::str::from_utf8(bytes).map_err(|_| ProceduralError::NotUtf8)?;
        let document: Self =
            sapphire_serialize::from_ron(text).map_err(|error| ProceduralError::Document(Box::new(error)))?;
        document.check()?;
        Ok(document)
    }

    /// Fails, naming the field, on anything the rasterizer can't make a tiling map of.
    pub fn check(&self) -> Result<(), ProceduralError> {
        let invalid = |field, why: &str| Err(ProceduralError::Invalid { field, why: why.to_owned() });
        let unit = |value: f32| (0.0..=1.0).contains(&value);
        let joint = |width: f32| (0.0..0.5).contains(&width);
        if self.size == 0 || !self.size.is_multiple_of(4) || self.size > MAX_TEXTURE_DIMENSION {
            return invalid("size", &format!("must be a multiple of 4 up to {MAX_TEXTURE_DIMENSION}"));
        }
        match self.pattern {
            Pattern::Plain => {}
            Pattern::Blocks(blocks) => {
                if blocks.columns == 0 || blocks.rows == 0 || blocks.columns.max(blocks.rows) > self.size {
                    return invalid("pattern", "blocks need 1 to `size` columns and rows");
                }
                if !joint(blocks.joint) || !joint(blocks.bevel) {
                    return invalid("pattern", "a joint and a bevel are from 0 to under 0.5 of the side");
                }
                if !unit(blocks.stagger) || (blocks.stagger * blocks.rows as f32).fract() > 0.0 {
                    return invalid(
                        "pattern",
                        "a stagger is from 0 to 1, and times the rows must be whole to tile",
                    );
                }
            }
            Pattern::Stones(stones) => {
                if !(1..=256).contains(&stones.cells) {
                    return invalid("pattern", "stones need 1 to 256 cells a side");
                }
                if !joint(stones.joint) || !joint(stones.bevel) {
                    return invalid("pattern", "a joint and a bevel are from 0 to under 0.5 of the side");
                }
            }
        }
        if self.pattern != Pattern::Plain && self.joint_color.is_none() {
            return invalid("joint_color", "a pattern with joints needs their colour");
        }
        let grain = self.grain;
        if grain.scale == 0 || grain.scale > self.size || !(1..=MAX_OCTAVES).contains(&grain.octaves) {
            return invalid("grain", &format!("needs 1 to `size` cells and 1 to {MAX_OCTAVES} octaves"));
        }
        if !unit(grain.depth) {
            return invalid("grain", "its depth is from 0 to 1");
        }
        if self.colors.is_empty()
            || !self.colors.iter().all(|(tone, _)| unit(*tone))
            || !self.colors.windows(2).all(|pair| pair[0].0 <= pair[1].0)
        {
            return invalid("colors", "needs a colour, by ascending tones from 0 to 1");
        }
        if !(0.0..=1.0).contains(&self.relief) {
            return invalid("relief", "is from 0 to 1 of the side");
        }
        if !unit(self.roughness.0) || !unit(self.roughness.1) {
            return invalid("roughness", "is from 0 to 1");
        }
        if !unit(self.specular) {
            return invalid("specular", "is from 0 to 1");
        }
        Ok(())
    }

    /// The map `map` of this surface, before cooking: linear values in its class's logical channels.
    pub fn rasterize(&self, map: SurfaceMap) -> SourceImage {
        let field = Field::new(self);
        let size = self.size as usize;
        let pixels: Vec<[f32; 4]> = match map {
            SurfaceMap::Color => {
                let colors: Vec<(f32, [f32; 4])> =
                    self.colors.iter().map(|&(tone, color)| (tone, color.to_linear())).collect();
                let joint = self.joint_color.map_or([0.0; 4], SrgbColor::to_linear);
                (0..size * size)
                    .map(|i| {
                        let face = stops(&colors, field.tone[i]);
                        // Darker down a bevel, as light in a joint is.
                        let shade = 0.8 + 0.2 * field.height[i];
                        let j = field.joint[i];
                        let mix = |c: usize| face[c] * shade * (1.0 - j) + joint[c] * j;
                        [mix(0), mix(1), mix(2), 1.0]
                    })
                    .collect()
            }
            SurfaceMap::Normal => {
                // Central differences across the wrap, as the surface tiles: a slope per texture side.
                let scale = self.relief * 0.5 * self.size as f32;
                let at = |x: usize, y: usize| field.height[y * size + x];
                (0..size * size)
                    .map(|i| {
                        let (x, y) = (i % size, i / size);
                        let (left, right) = ((x + size - 1) % size, (x + 1) % size);
                        let (up, down) = ((y + size - 1) % size, (y + 1) % size);
                        // +Y runs up the image, against its rows.
                        let dx = -(at(right, y) - at(left, y)) * scale;
                        let dy = (at(x, down) - at(x, up)) * scale;
                        let length = (dx * dx + dy * dy + 1.0).sqrt();
                        [dx / length * 0.5 + 0.5, dy / length * 0.5 + 0.5, 1.0 / length * 0.5 + 0.5, 1.0]
                    })
                    .collect()
            }
            SurfaceMap::Mask => {
                let (low, high) = self.roughness;
                (0..size * size)
                    .map(|i| {
                        let (height, grain, joint) = (field.height[i], field.grain[i], field.joint[i]);
                        let occlusion = 0.45 + 0.55 * height;
                        let cavity = 0.7 + 0.3 * height;
                        let specular = self.specular * (0.6 + 0.4 * grain) * (1.0 - joint);
                        let roughness = low + (high - low) * grain;
                        [occlusion, cavity, specular, roughness + (1.0 - roughness) * joint]
                    })
                    .collect()
            }
        };
        let layout = if map == SurfaceMap::Mask { ChannelLayout::Rgba } else { ChannelLayout::Rgb };
        SourceImage::new(self.size, self.size, layout, pixels)
            .expect("one pixel a texel, and a size checked above 0")
    }
}

/// What a surface is at each texel, from which every map is taken.
struct Field {
    /// 0 on a joint's floor, 1 on a face's top.
    height: Vec<f32>,
    /// Picks the colour: the block's own tone and the grain.
    tone: Vec<f32>,
    /// How much of the texel is joint, 0 to 1.
    joint: Vec<f32>,
    /// The grain, 0 to 1.
    grain: Vec<f32>,
}

/// Where a texel is in its pattern: how far it is from the nearest joint's middle, as a fraction of the
/// side, and a value of its block's own, 0 to 1.
struct Place {
    edge: f32,
    tone: f32,
}

impl Field {
    fn new(surface: &ProceduralTexture) -> Self {
        let size = surface.size as usize;
        let side = surface.size as f32;
        let texel = 1.0 / side;
        let grain = surface.grain;
        let period = grain.scale as i32;
        let seed = noise::hash(surface.seed);
        let (joint, bevel) = match surface.pattern {
            Pattern::Plain => (0.0, 0.0),
            Pattern::Blocks(blocks) => (blocks.joint, blocks.bevel),
            Pattern::Stones(stones) => (stones.joint, stones.bevel),
        };
        let mut field = Self {
            height: vec![0.0; size * size],
            tone: vec![0.0; size * size],
            joint: vec![0.0; size * size],
            grain: vec![0.0; size * size],
        };
        for y in 0..size {
            for x in 0..size {
                let (u, v) = ((x as f32 + 0.5) * texel, (y as f32 + 0.5) * texel);
                let place = match surface.pattern {
                    Pattern::Plain => Place { edge: 1.0, tone: 0.5 },
                    Pattern::Blocks(blocks) => blocks_place(blocks, u, v, seed),
                    Pattern::Stones(stones) => stones_place(stones, u, v, seed),
                };
                let at = glam::Vec2::new(u * grain.scale as f32, v * grain.scale as f32);
                let g = noise::fbm(at, (period, period), grain.octaves, seed.wrapping_add(1));
                // Half a joint each side of its middle, softened over a texel so lines don't stair.
                let half = joint * 0.5;
                let in_joint = if joint > 0.0 {
                    1.0 - noise::smoothstep(half - texel * 0.5, half + texel * 0.5, place.edge)
                } else {
                    0.0
                };
                let rise = if bevel > 0.0 { noise::smoothstep(half, half + bevel, place.edge) } else { 1.0 };
                let i = y * size + x;
                field.height[i] = (1.0 - in_joint) * rise * (1.0 - grain.depth * (1.0 - g));
                field.tone[i] = if surface.pattern == Pattern::Plain { g } else { (place.tone + g) * 0.5 };
                field.joint[i] = in_joint;
                field.grain[i] = g;
            }
        }
        field
    }
}

/// A value from 0 to 1 for a lattice point, as the noise's are.
fn lattice(x: u32, y: u32, seed: u32) -> f32 {
    noise::hash(x ^ noise::hash(y ^ noise::hash(seed))) as f32 / 4_294_967_296.0
}

fn blocks_place(blocks: Blocks, u: f32, v: f32, seed: u32) -> Place {
    let (columns, rows) = (blocks.columns as f32, blocks.rows as f32);
    let along = v * rows;
    let row = along.floor();
    let across = u * columns + blocks.stagger * row;
    let column = across.floor();
    let (fu, fv) = (across - column, along - row);
    let edge = (fu.min(1.0 - fu) / columns).min(fv.min(1.0 - fv) / rows);
    // A block's own tone, by where it lies on the tile, so one across the wrap is the same block.
    let column = (column as i64).rem_euclid(i64::from(blocks.columns)) as u32;
    let row = (row as i64).rem_euclid(i64::from(blocks.rows)) as u32;
    Place { edge, tone: lattice(column, row, seed) }
}

fn stones_place(stones: Stones, u: f32, v: f32, seed: u32) -> Place {
    let cells = stones.cells as i64;
    let (px, py) = (u * stones.cells as f32, v * stones.cells as f32);
    let (cx, cy) = (px.floor(), py.floor());
    // The nearest point and the next nearest, over the 3 × 3 cells around, wrapping.
    let (mut first, mut second, mut nearest) = (f32::MAX, f32::MAX, (0, 0));
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (nx, ny) = (cx + dx as f32, cy + dy as f32);
            let wrapped = ((nx as i64).rem_euclid(cells) as u32, (ny as i64).rem_euclid(cells) as u32);
            let jitter = |salt: u32| 0.15 + 0.7 * lattice(wrapped.0, wrapped.1, seed.wrapping_add(salt));
            let (fx, fy) = (nx + jitter(11) - px, ny + jitter(23) - py);
            let distance = (fx * fx + fy * fy).sqrt();
            if distance < first {
                (second, first, nearest) = (first, distance, wrapped);
            } else if distance < second {
                second = distance;
            }
        }
    }
    // Half the gap between the two nearest is about the distance to the edge between their stones.
    Place { edge: (second - first) * 0.5 / stones.cells as f32, tone: lattice(nearest.0, nearest.1, seed) }
}

/// Piecewise-linear colour stops, ascending by tone, held past the ends.
fn stops(points: &[(f32, [f32; 4])], at: f32) -> [f32; 4] {
    let (first, last) = (points[0], points[points.len() - 1]);
    if at <= first.0 {
        return first.1;
    }
    if at >= last.0 {
        return last.1;
    }
    let pair = points.windows(2).find(|pair| at <= pair[1].0).expect("between the first and the last");
    let (a, b) = (pair[0], pair[1]);
    let t = if b.0 > a.0 { (at - a.0) / (b.0 - a.0) } else { 1.0 };
    std::array::from_fn(|c| a.1[c] + (b.1[c] - a.1[c]) * t)
}

/// A rasterized map as an 8-bit PNG to look at (`sapphire_pipeline bake-map`): sRGB for colour, linear
/// values for the normal and the mask, the mask's roughness in its alpha.
pub fn to_png(image: &SourceImage, map: SurfaceMap) -> image::ImageResult<Vec<u8>> {
    let encode = |v: f32| {
        if map == SurfaceMap::Color { color::unorm8(color::linear_to_srgb(v)) } else { color::unorm8(v) }
    };
    let texels =
        image.pixels.iter().flat_map(|&[r, g, b, a]| [encode(r), encode(g), encode(b), color::unorm8(a)]);
    let png =
        image::RgbaImage::from_raw(image.width, image.height, texels.collect()).expect("one texel per pixel");
    let mut buffer = std::io::Cursor::new(Vec::new());
    png.write_to(&mut buffer, image::ImageFormat::Png)?;
    Ok(buffer.into_inner())
}

/// About what cooking a procedural texture holds at its peak: the field, and what a texture of its size
/// holds ([`texture::cook_memory`]). Nothing if the document doesn't read, since it won't cook.
pub fn cook_memory(bytes: &[u8]) -> u64 {
    ProceduralTexture::read(bytes).map_or(0, |document| {
        let texels = u64::from(document.size) * u64::from(document.size);
        texels.saturating_mul(texture::COOK_BYTES_PER_TEXEL + FIELD_BYTES_PER_TEXEL)
    })
}

/// Rasterizes and cooks one procedural texture: the bytes of its cooked KTX2 file, whole.
pub fn cook_source(
    bytes: &[u8],
    settings: &ProceduralSettings,
    cook: Cook,
) -> Result<Vec<u8>, ProceduralError> {
    Ok(cook_chain(bytes, settings, cook)?.to_ktx2())
}

/// Rasterizes and cooks one procedural texture: its whole mip chain, which the cook splits into a
/// tail and levels as it does a texture of its map's class (M5 M3, [`CookedTexture::tail`]).
pub fn cook_chain(
    bytes: &[u8],
    settings: &ProceduralSettings,
    cook: Cook,
) -> Result<CookedTexture, ProceduralError> {
    let document = ProceduralTexture::read(bytes)?;
    let image = document.rasterize(settings.map);
    let texture = settings.texture();
    let cooked =
        texture::cook(&image, &texture, cook).map_err(|error| ProceduralError::Texture(Box::new(error)))?;
    // The plan split it by the document's size, before anything cooked.
    let (planned, found) = (planned_levels(bytes, settings, cook), cooked.levels_above(&texture));
    if planned != found {
        return Err(ProceduralError::Texture(Box::new(TextureError::PlannedLevels { planned, found })));
    }
    Ok(cooked)
}

/// How many levels above its tail the document `bytes` splits off for `cook`, by its size alone, as a
/// texture of its map's class of that size does ([`texture::levels_above`]): 0 if it doesn't read,
/// since it won't cook.
pub fn planned_levels(bytes: &[u8], settings: &ProceduralSettings, cook: Cook) -> u32 {
    ProceduralTexture::read(bytes)
        .map_or(0, |document| texture::levels_above(&settings.texture(), cook, document.size, document.size))
}

#[cfg(test)]
mod tests;
