//! A material's maps, painted in code on its cells: one column of each map a region, in the order
//! its regions are listed, and V running down a gradient within it. Without the figure's `maps`,
//! every triangle's U sits in its cell's middle, so a mip has to shrink a cell to a texel before
//! neighbours bleed into it, and each map is as small as its cells; with them, a map is square at
//! their size, a cell is its share of the width, and shapes are unwrapped across it.
//!
//! Colours are picked in 8-bit sRGB, as an artist would, and blended in linear light. A grain
//! (`maps.grain`) moves each albedo texel off its gradient by a hash of its place, as paint is, so a
//! map compresses as a painted one does rather than as flat colour.

use sapphire_assets::character_maps::{MaterialId, ilm};

use super::{FigureError, Maps, Region};
use crate::color::unorm8;
use crate::color::{linear_to_srgb, srgb_to_linear};
use crate::combat::png;
use crate::samples::noise;

/// The palette's texels across a cell, and down it, without `maps`.
const PALETTE_CELL: (u32, u32) = (64, 256);
/// The control map's and the layer mask's texels across a cell, and down it, without `maps`.
const MASK_CELL: (u32, u32) = (16, 128);

/// A `#rrggbb` colour in linear light.
fn color(region: &Region, text: &str) -> Result<[f32; 3], FigureError> {
    let bad = || FigureError::Region { region: region.name.clone(), why: format!("{text:?} isn't #rrggbb") };
    let hex = text.strip_prefix('#').filter(|hex| hex.len() == 6).ok_or_else(bad)?;
    let value = u32::from_str_radix(hex, 16).map_err(|_| bad())?;
    Ok([16, 8, 0].map(|shift| srgb_to_linear(((value >> shift) & 0xff) as f32 / 255.0)))
}

/// Piecewise-linear stops, ascending by their first value, held past the ends.
fn stops<T: Copy>(points: &[(f32, T)], at: f32, mix: impl Fn(T, T, f32) -> T) -> T {
    let (mut before, mut after) = (points[0], points[points.len() - 1]);
    if at <= before.0 {
        return before.1;
    }
    for pair in points.windows(2) {
        if (pair[0].0..=pair[1].0).contains(&at) {
            (before, after) = (pair[0], pair[1]);
        }
    }
    let span = (after.0 - before.0).max(1e-5);
    mix(before.1, after.1, ((at - before.0) / span).clamp(0.0, 1.0))
}

/// A region's colour stops: where along it, and the colour there.
type Stops = Vec<(f32, [f32; 3])>;

fn check(regions: &[&Region]) -> Result<Vec<Stops>, FigureError> {
    regions
        .iter()
        .map(|region| {
            let ascending = |values: &mut dyn Iterator<Item = f32>| {
                let values: Vec<f32> = values.collect();
                values.windows(2).all(|pair| pair[0] <= pair[1])
            };
            if region.albedo.is_empty() || !ascending(&mut region.albedo.iter().map(|stop| stop.0)) {
                return Err(FigureError::Region {
                    region: region.name.clone(),
                    why: "its albedo needs stops, by ascending v".to_owned(),
                });
            }
            if !ascending(&mut region.emission.iter().map(|stop| stop.0)) {
                return Err(FigureError::Region {
                    region: region.name.clone(),
                    why: "its emission's stops go by ascending v".to_owned(),
                });
            }
            region.albedo.iter().map(|(at, text)| Ok((*at, color(region, text)?))).collect()
        })
        .collect()
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|channel| a[channel] + (b[channel] - a[channel]) * t)
}

/// A map's size, and the cell texel `x` falls in.
struct Layout {
    width: u32,
    height: u32,
    cells: u32,
    /// Texels across a cell without `maps`; with them, cells share the width.
    cell_width: Option<u32>,
}

impl Layout {
    fn new(cells: usize, maps: Option<Maps>, cell: (u32, u32)) -> Self {
        let cells = cells as u32;
        match maps {
            None => Self { width: cell.0 * cells, height: cell.1, cells, cell_width: Some(cell.0) },
            Some(maps) => Self { width: maps.size, height: maps.size, cells, cell_width: None },
        }
    }

    fn cell(&self, x: u32) -> usize {
        match self.cell_width {
            Some(width) => (x / width) as usize,
            None => ((u64::from(x) * u64::from(self.cells)) / u64::from(self.width)) as usize,
        }
    }
}

/// A texel's grain: an offset in sRGB, within half of `grain` either way, by a hash of its place.
fn grain(x: u32, y: u32, channel: u32, grain: f32) -> f32 {
    let hash = noise::hash(x.wrapping_mul(0x9e37_79b9) ^ noise::hash(y.wrapping_add(channel << 28)));
    (hash as f32 / u32::MAX as f32 - 0.5) * grain
}

/// The albedo of `regions`, one material's.
pub fn palette_png(regions: &[&Region], maps: Option<Maps>) -> Result<Vec<u8>, FigureError> {
    let albedo = check(regions)?;
    let layout = Layout::new(regions.len(), maps, PALETTE_CELL);
    let grained = maps.map_or(0.0, |maps| maps.grain);
    let image = image::RgbImage::from_fn(layout.width, layout.height, |x, y| {
        let v = (y as f32 + 0.5) / layout.height as f32;
        let color = stops(&albedo[layout.cell(x)], v, lerp3);
        let mut channel = 0;
        image::Rgb(color.map(|c| {
            let srgb = linear_to_srgb(c);
            channel += 1;
            unorm8(if grained > 0.0 { srgb + grain(x, y, channel, grained) } else { srgb })
        }))
    });
    Ok(png(image)?)
}

/// What an `.ilm.ron` packs: R highlight, G shadow bias, B highlight size, and A the region's
/// material ID, as gray levels the document snaps to.
pub fn control_png(regions: &[&Region], maps: Option<Maps>) -> Result<Vec<u8>, FigureError> {
    check(regions)?;
    let layout = Layout::new(regions.len(), maps, MASK_CELL);
    let image = image::RgbaImage::from_fn(layout.width, layout.height, |x, _| {
        let index = layout.cell(x);
        let region = regions[index];
        let id = MaterialId::new(index as u8).expect("fewer regions than IDs");
        let id = ilm::encode_material_id(id);
        image::Rgba([region.specular, region.shadow_bias, region.specular_size, id].map(unorm8))
    });
    Ok(png(image)?)
}

/// The layer mask: R the matcap, G nothing (nothing reflects), B emission down the cell, A nothing.
pub fn layers_png(regions: &[&Region], maps: Option<Maps>) -> Result<Vec<u8>, FigureError> {
    check(regions)?;
    let layout = Layout::new(regions.len(), maps, MASK_CELL);
    let image = image::RgbaImage::from_fn(layout.width, layout.height, |x, y| {
        let region = regions[layout.cell(x)];
        let v = (y as f32 + 0.5) / layout.height as f32;
        let emission = if region.emission.is_empty() {
            0.0
        } else {
            stops(&region.emission, v, |a, b, t| a + (b - a) * t)
        };
        image::Rgba([region.matcap, 0.0, emission, 0.0].map(unorm8))
    });
    Ok(png(image)?)
}
