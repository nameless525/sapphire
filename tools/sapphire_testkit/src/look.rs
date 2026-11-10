//! Measures of the anime look in output pixels (M4 T0; M4 plan pillar 1, exit test 4): how wide
//! and how crisp lines are, and how wide the two-tone edge's transition is. Pure functions over
//! 8-bit sRGB pixels, so the renderer's GPU tests (a dev-dependency) and a capture's analysis
//! (`sapphire_testkit measure`) read the same numbers.
//!
//! - [`Lines`]: a line's darkness against what it covers, summed. Its strength is the line's
//!   width times its darkness; its contrast, the darkness-weighted mean darkness, falls when a line
//!   blurs; and their ratio, [`Lines::width`], is how many pixels' worth the darkness spreads over.
//! - [`Edge`]: one crossing of a two-tone edge, from a profile of luminances across it: the
//!   distance its middle 80% takes, and the largest share of the step one pixel takes.
//! - [`Spread`]: the mean width of every edge in a region, for scenes too busy to profile one by
//!   one. Comparing two captures of the same shot, the ratio of their spreads is the ratio of
//!   their edges' widths.

use crate::golden::Image;

/// A rectangle of pixels: `x` and `y` its top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// The whole of `image`.
    pub fn of(image: &Image) -> Self {
        Self { x: 0, y: 0, width: image.width, height: image.height }
    }

    /// Whether it lies inside `image`.
    pub fn fits(&self, image: &Image) -> bool {
        self.x.checked_add(self.width).is_some_and(|right| right <= image.width)
            && self.y.checked_add(self.height).is_some_and(|bottom| bottom <= image.height)
    }

    fn pixels(&self) -> impl Iterator<Item = (u32, u32)> {
        let Self { x, y, width, height } = *self;
        (y..y + height).flat_map(move |row| (x..x + width).map(move |column| (column, row)))
    }
}

impl std::str::FromStr for Region {
    type Err = String;

    /// `x,y,width,height`.
    fn from_str(text: &str) -> Result<Self, String> {
        let numbers: Vec<u32> = text
            .split(',')
            .map(|part| part.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("invalid region {text:?}: expected x,y,width,height in whole pixels"))?;
        match numbers[..] {
            [x, y, width, height] if width > 0 && height > 0 => Ok(Self { x, y, width, height }),
            _ => Err(format!("invalid region {text:?}: expected x,y,width,height with a size")),
        }
    }
}

/// An sRGB code's linear value.
pub fn linear(code: u8) -> f32 {
    let c = f32::from(code) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// A pixel's linear luminance (Rec. 709).
pub fn luminance([r, g, b, _]: [u8; 4]) -> f32 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// How much light a line takes from what lies under it, from 0 (none) to 1 (black): `1 −
/// line / under`, in linear light, where `under` is the light without the line.
pub fn darkness(line: f32, under: f32) -> f32 {
    if under > 0.0 { (1.0 - line / under).clamp(0.0, 1.0) } else { 0.0 }
}

/// Lines, summed from their pixels' [`darkness`] (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lines {
    /// Every pixel's darkness, summed: a line's width times its darkness, in pixels.
    pub strength: f32,
    /// Every pixel's darkness squared, summed.
    squares: f32,
}

impl Lines {
    /// Lines of these pixels' darkness.
    pub fn from_darkness(darkness: impl IntoIterator<Item = f32>) -> Self {
        darkness.into_iter().fold(Self::default(), |lines, d| Self {
            strength: lines.strength + d,
            squares: lines.squares + d * d,
        })
    }

    /// The lines `with` has and `without` lacks over `region`: two captures of one shot, with
    /// the lines drawn and without them.
    pub fn between(with: &Image, without: &Image, region: Region) -> Result<Self, String> {
        check(with, without, region)?;
        Ok(Self::from_darkness(
            region
                .pixels()
                .map(|(x, y)| darkness(luminance(pixel(with, x, y)), luminance(pixel(without, x, y)))),
        ))
    }

    /// The darkness-weighted mean darkness: 1 for solid black lines, less as they blur or fade.
    pub fn contrast(&self) -> f32 {
        if self.strength > 0.0 { self.squares / self.strength } else { 0.0 }
    }

    /// How many pixels' worth the darkness spreads over: `strength / contrast`.
    pub fn width(&self) -> f32 {
        let contrast = self.contrast();
        if contrast > 0.0 { self.strength / contrast } else { 0.0 }
    }
}

/// One crossing of a two-tone edge (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Pixels between where the profile crosses 10% and 90% of the way between its tones, read
    /// linearly between pixel centers: 0.8 for a step from one pixel to the next, 1.6 with one
    /// pixel halfway between, and wider as the edge blurs.
    pub width: f32,
    /// The largest share of the step one pixel takes: near 1 for a hard edge.
    pub steepest: f32,
}

impl Edge {
    /// The edge in `profile`, luminances in order across it. Its tones are the medians of its
    /// first and last quarters, which must be flat sides at least two pixels long; `None` if the
    /// sides differ by less than `min_step` or the profile is shorter than eight pixels.
    pub fn of(profile: &[f32], min_step: f32) -> Option<Self> {
        if profile.len() < 8 {
            return None;
        }
        let side = profile.len() / 4;
        let (from, to) = (median(&profile[..side]), median(&profile[profile.len() - side..]));
        if (to - from).abs() < min_step {
            return None;
        }
        let share: Vec<f32> =
            profile.iter().map(|&value| ((value - from) / (to - from)).clamp(0.0, 1.0)).collect();
        let steepest = share.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f32::max);
        Some(Self { width: crossing(&share, 0.9)? - crossing(&share, 0.1)?, steepest })
    }
}

/// Where `share`, rising from 0 to 1, first reaches `level`, between pixel centers.
fn crossing(share: &[f32], level: f32) -> Option<f32> {
    share.windows(2).enumerate().find_map(|(i, pair)| {
        (pair[0] < level && pair[1] >= level).then(|| i as f32 + (level - pair[0]) / (pair[1] - pair[0]))
    })
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted[sorted.len() / 2]
}

/// The edges of a region, summed from their gradients (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spread {
    /// Every pixel's gradient magnitude in linear luminance, summed.
    pub total: f32,
    /// Every pixel's gradient magnitude squared, summed.
    pub energy: f32,
}

impl Spread {
    /// The gradients of `image`'s luminance over `region`, by central differences, skipping
    /// pixels on its border and those `skip` marks.
    pub fn of(image: &Image, region: Region, skip: impl Fn(u32, u32) -> bool) -> Result<Self, String> {
        if !region.fits(image) {
            return Err(format!("region {region:?} doesn't fit a {}×{} image", image.width, image.height));
        }
        let at = |x: u32, y: u32| luminance(pixel(image, x, y));
        let inner = |x: u32, y: u32| x > 0 && y > 0 && x + 1 < image.width && y + 1 < image.height;
        Ok(region.pixels().filter(|&(x, y)| inner(x, y) && !skip(x, y)).fold(
            Self::default(),
            |spread, (x, y)| {
                let dx = 0.5 * (at(x + 1, y) - at(x - 1, y));
                let dy = 0.5 * (at(x, y + 1) - at(x, y - 1));
                let squared = dx * dx + dy * dy;
                Self { total: spread.total + squared.sqrt(), energy: spread.energy + squared }
            },
        ))
    }

    /// `total² / energy`: for one step blurred over `k` pixels, about `k` per pixel of its length
    /// (central differences read an unblurred step as two), so two captures of one shot compare
    /// by the ratio of their edges' widths.
    pub fn width(&self) -> f32 {
        if self.energy > 0.0 { self.total * self.total / self.energy } else { 0.0 }
    }
}

fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * image.width as usize + x as usize) * 4;
    let mut rgba = [0; 4];
    rgba.copy_from_slice(&image.rgba[at..at + 4]);
    rgba
}

fn check(a: &Image, b: &Image, region: Region) -> Result<(), String> {
    if (a.width, a.height) != (b.width, b.height) {
        return Err(format!(
            "the images differ in size: {}×{} and {}×{}",
            a.width, a.height, b.width, b.height
        ));
    }
    if !region.fits(a) {
        return Err(format!("region {region:?} doesn't fit a {}×{} image", a.width, a.height));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image `width` × `height` whose grey code at each pixel is `code(x, y)`.
    fn grey(width: u32, height: u32, code: impl Fn(u32, u32) -> u8) -> Image {
        let rgba = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let c = code(x, y);
                [c, c, c, 255]
            })
            .collect();
        Image { width, height, rgba }
    }

    /// The sRGB code of linear `value`.
    fn code(value: f32) -> u8 {
        (0..=255).min_by(|&a, &b| (linear(a) - value).abs().total_cmp(&(linear(b) - value).abs())).unwrap()
    }

    #[test]
    fn lines_read_width_and_crispness() {
        // A black line 3 px wide, then the same darkness spread over 6 px at half strength.
        let crisp = Lines::from_darkness([0.0, 1.0, 1.0, 1.0, 0.0]);
        assert_eq!((crisp.strength, crisp.contrast(), crisp.width()), (3.0, 1.0, 3.0));
        let blurred = Lines::from_darkness([0.5; 6]);
        assert_eq!((blurred.strength, blurred.contrast(), blurred.width()), (3.0, 0.5, 6.0));
        assert_eq!(Lines::default().width(), 0.0, "no lines");

        // Between captures: a 2 px black line over a grey field, which a lighter pixel can't undo.
        let without = grey(8, 4, |_, _| 188);
        let with = grey(8, 4, |x, y| match (x, y) {
            (3 | 4, _) => 0,
            (0, 0) => 255,
            _ => 188,
        });
        let lines = Lines::between(&with, &without, Region::of(&with)).unwrap();
        assert!((lines.strength - 8.0).abs() < 1e-4 && (lines.contrast() - 1.0).abs() < 1e-4, "{lines:?}");
        let one_row = Region { x: 0, y: 1, width: 8, height: 1 };
        assert!((Lines::between(&with, &without, one_row).unwrap().width() - 2.0).abs() < 1e-4);
        assert!(Lines::between(&with, &grey(8, 5, |_, _| 188), one_row).is_err());
        assert!(Lines::between(&with, &without, Region { x: 4, y: 0, width: 5, height: 1 }).is_err());
    }

    #[test]
    fn edges_read_their_transition() {
        // A step from one pixel to the next, one antialiased across a pixel, and one blurred
        // over four.
        let step = Edge::of(&[0.2, 0.2, 0.2, 0.2, 0.5, 0.5, 0.5, 0.5], 0.05).unwrap();
        assert!((step.width - 0.8).abs() < 1e-4 && (step.steepest - 1.0).abs() < 1e-4, "{step:?}");
        let hard = Edge::of(&[0.2, 0.2, 0.2, 0.2, 0.35, 0.5, 0.5, 0.5, 0.5], 0.05).unwrap();
        assert!((hard.width - 1.6).abs() < 1e-4 && (hard.steepest - 0.5).abs() < 1e-4, "{hard:?}");
        let ramp = [0.2, 0.2, 0.2, 0.26, 0.32, 0.38, 0.44, 0.5, 0.5, 0.5, 0.5];
        let soft = Edge::of(&ramp, 0.05).unwrap();
        assert!((soft.width - 4.0).abs() < 1e-3 && (soft.steepest - 0.2).abs() < 1e-3, "{soft:?}");
        let falling: Vec<f32> = ramp.iter().rev().copied().collect();
        let fall = Edge::of(&falling, 0.05).unwrap();
        assert!((fall.width - soft.width).abs() < 1e-4, "either way across: {fall:?}");
        assert_eq!(Edge::of(&[0.2; 12], 0.05), None, "no step");
        assert_eq!(Edge::of(&[0.2, 0.5], 0.05), None, "too short");
    }

    #[test]
    fn spread_compares_edge_widths() {
        // Vertical edges between two tones, as a step and blurred over four pixels.
        let (dark, light) = (code(0.1), code(0.5));
        let step = grey(32, 8, |x, _| if x % 16 < 8 { dark } else { light });
        let blurred = grey(32, 8, |x, _| {
            let t = ((x % 16) as f32 - 6.0) / 4.0;
            code(0.1 + 0.4 * t.clamp(0.0, 1.0))
        });
        let region = Region { x: 1, y: 1, width: 14, height: 6 };
        let (sharp, soft) = (
            Spread::of(&step, region, |_, _| false).unwrap(),
            Spread::of(&blurred, region, |_, _| false).unwrap(),
        );
        // Central differences read a step as two pixels wide and the ramp as about 4.6.
        let ratio = soft.width() / sharp.width();
        assert!((2.0..2.6).contains(&ratio), "{ratio}: {sharp:?} {soft:?}");
        assert!((sharp.total - soft.total).abs() < 0.05 * sharp.total, "the same steps either way");
        let skipped = Spread::of(&step, region, |x, _| x == 8).unwrap();
        assert!(skipped.total < sharp.total);
        assert!(Spread::of(&step, Region { x: 30, y: 0, width: 4, height: 1 }, |_, _| false).is_err());
    }

    #[test]
    fn regions_parse_and_fit() {
        let region: Region = "2, 3,10,4".parse().unwrap();
        assert_eq!(region, Region { x: 2, y: 3, width: 10, height: 4 });
        assert!("2,3,10".parse::<Region>().is_err() && "2,3,0,4".parse::<Region>().is_err());
        assert!("a,3,10,4".parse::<Region>().is_err());
        let image = grey(12, 7, |_, _| 0);
        assert!(region.fits(&image) && !Region { x: 3, ..region }.fits(&image));
        assert!(!Region { x: u32::MAX, ..region }.fits(&image), "no overflow");
    }
}
