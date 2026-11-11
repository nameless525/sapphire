//! A figure's maps, painted in code on its cells: one column of each map a region, in the order the
//! spec lists them, and V running down a gradient within it. Every triangle's U sits in its cell's
//! middle, so a mip has to shrink a cell to a texel before neighbours bleed into it.
//!
//! Colours are picked in 8-bit sRGB, as an artist would, and blended in linear light.

use sapphire_assets::character_maps::{MaterialId, ilm};

use super::{Figure, FigureError, Region};
use crate::color::unorm8;
use crate::color::{linear_to_srgb, srgb_to_linear};
use crate::combat::png;

/// The palette's texels across a cell, and down it.
const PALETTE_CELL: (u32, u32) = (64, 256);
/// The control map's and the layer mask's texels across a cell, and down it.
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

fn check(figure: &Figure) -> Result<Vec<Stops>, FigureError> {
    figure
        .regions
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

/// The albedo.
pub fn palette_png(figure: &Figure) -> Result<Vec<u8>, FigureError> {
    let albedo = check(figure)?;
    let (width, height) = (PALETTE_CELL.0 * albedo.len() as u32, PALETTE_CELL.1);
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        let v = (y as f32 + 0.5) / height as f32;
        let color = stops(&albedo[(x / PALETTE_CELL.0) as usize], v, lerp3);
        image::Rgb(color.map(|c| unorm8(linear_to_srgb(c))))
    });
    Ok(png(image)?)
}

/// What an `.ilm.ron` packs: R highlight, G shadow bias, B highlight size, and A the region's
/// material ID, as gray levels the document snaps to.
pub fn control_png(figure: &Figure) -> Result<Vec<u8>, FigureError> {
    check(figure)?;
    let (width, height) = (MASK_CELL.0 * figure.regions.len() as u32, MASK_CELL.1);
    let image = image::RgbaImage::from_fn(width, height, |x, _| {
        let index = (x / MASK_CELL.0) as usize;
        let region = &figure.regions[index];
        let id = MaterialId::new(index as u8).expect("fewer regions than IDs");
        let id = ilm::encode_material_id(id);
        image::Rgba([region.specular, region.shadow_bias, region.specular_size, id].map(unorm8))
    });
    Ok(png(image)?)
}

/// The layer mask: R the matcap, G nothing (nothing reflects), B emission down the cell, A nothing.
pub fn layers_png(figure: &Figure) -> Result<Vec<u8>, FigureError> {
    check(figure)?;
    let (width, height) = (MASK_CELL.0 * figure.regions.len() as u32, MASK_CELL.1);
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        let region = &figure.regions[(x / MASK_CELL.0) as usize];
        let v = (y as f32 + 0.5) / height as f32;
        let emission = if region.emission.is_empty() {
            0.0
        } else {
            stops(&region.emission, v, |a, b, t| a + (b - a) * t)
        };
        image::Rgba([region.matcap, 0.0, emission, 0.0].map(unorm8))
    });
    Ok(png(image)?)
}
