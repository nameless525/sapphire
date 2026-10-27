//! The look-dev sky's own maps (M1 X1 part 2e), toward Wuthering Waves' sky: towering cumulus for
//! the card band, which the key light shades (`CloudShading::Lit`), and the night's nebula. Both
//! are painted here in code, from shapes and noise, never traced from the reference frames.
//!
//! - **`sky/cumulus.png`** ([`cumulus_png`], 4096 × 576, linear RGBA): the card band once around
//!   the horizon at the sandbox band's 11.4 texels a degree, and as tall as the band keeps its
//!   texels square at the horizon, which reaches 58°. Each cloud ([`CLOUDS`]) is a union of
//!   masses, each mass a dome of round lobes stacked in rows over a flat base, smaller lobes
//!   budding from their tops, the upper ones in front, merged smoothly and roughened by noise so
//!   the outline reads as painted. R and G hold the normal of the front lobe (a sphere facing the
//!   viewer) across and up, B the distance to the cloud's edge (the sandbox band's field), A how
//!   open the lobe is to the sky: closed in the crevices around the lobes in front of it and
//!   under the base. Shapes are laid out in degrees of arc on the sky, so they keep their
//!   proportions up the band, where its rows grow taller.
//! - **`sky/nebula.png`** ([`nebula_png`], 2048 × 1024, linear RGB): nebulae on the celestial
//!   sphere, equirectangular as the star and Milky Way maps, along the Milky Way's band (the
//!   sandbox sky's galactic pole) and wandering off it: R broad billowing clouds of warped
//!   noise, cut by wide dark rifts of dust; G the glow where the clouds meet the rifts and in a
//!   few bright hearts; B sparse knots deep in the clouds. Time of day colors each channel.
//!
//! Every field is a continuous function of the texel's position (soft lobe fronts, smooth minima,
//! smoothsteps), so the libm rounding that differs between platforms moves a texel by a code
//! value at most; nothing a shape is built from passes through a transcendental function.

use std::f32::consts::{PI, TAU};
use std::io;

use glam::{Vec2, Vec3};

use crate::color::unorm8;
use crate::samples::noise::{NO_PERIOD, Rng, fbm, smoothstep};
use crate::samples::sky::{CLOUD_DISTANCE_RANGE, GALACTIC_POLE, dissolve_field};

/// The card band's map: once around the horizon, 11.4 texels a degree as the sandbox's band.
pub const CUMULUS_SIZE: (u32, u32) = (4096, 576);
/// The nebula's map, equirectangular on the celestial sphere: 5.7 texels a degree.
pub const NEBULA_SIZE: (u32, u32) = (2048, 1024);

/// The band's bottom edge as the sine of the elevation (the sky shader's `CLOUD_BAND_BASE`).
const BAND_BASE: f32 = -0.035;
/// Texels of the band a degree of azimuth.
const TEXELS_PER_DEGREE: f32 = CUMULUS_SIZE.0 as f32 / 360.0;
/// How far around a cloud its fields reach, in degrees: its distance field fades out half a
/// distance range outside the edge, and noise moves the edge by up to a degree.
const CLOUD_MARGIN: f32 = 0.5 * CLOUD_DISTANCE_RANGE / TEXELS_PER_DEGREE + 1.5;
/// How far outside a cloud's edge its lobes' normals reach, in texels, so filtering across the
/// silhouette blends the rim's normals; further out the map holds the flat normal.
const NORMAL_REACH: f32 = 3.0;
/// The soft front's blend between overlapping lobes, in degrees of lobe height: a fraction of a
/// texel wide, so each lobe's edge stays crisp and the map continuous.
const FRONT_SOFTNESS: f32 = 0.02;
/// Unit directions on a lobe's upper arc where smaller lobes bud: up, and 30° and 55° either side.
const BUDS: [Vec2; 5] = [
    Vec2::new(0.0, 1.0),
    Vec2::new(0.5, 0.866_025_4),
    Vec2::new(-0.5, 0.866_025_4),
    Vec2::new(0.819_152, 0.573_576_4),
    Vec2::new(-0.819_152, 0.573_576_4),
];

/// One mass of a cloud, in degrees from the cloud's middle along its base: a dome of lobes
/// `width` wide and `height` tall, a tower when taller than wide.
#[derive(Clone, Copy, Debug)]
pub struct Mass {
    pub x: f32,
    pub width: f32,
    pub height: f32,
}

/// A cloud on the band: where it stands, in degrees (azimuth from −Z toward +X, and the
/// elevation of its flat base), its masses and its noise seed.
#[derive(Clone, Copy, Debug)]
pub struct Cloud {
    pub azimuth: f32,
    pub base: f32,
    pub masses: &'static [Mass],
    pub seed: u32,
}

const fn mass(x: f32, width: f32, height: f32) -> Mass {
    Mass { x, width, height }
}

/// The look-dev band's clouds, around the horizon. The stage's cameras look toward −Z (azimuth
/// 0): the wide shot sees about −54° to 26°, the sky shot −16° to 72° and up to 47°. Over the
/// square stand two towering masses with clear sky between them, rising from behind the
/// mountains to about 40° and 31°, so they tower over the roofs; heaped cumulus and smaller clouds
/// floating higher fill the rest of the horizon, clear of the sun where the look-dev day puts it
/// at 9:00 (118°, 25° up), 13:00 (113°, 38°), 16:00 (222°, 37°) and golden hour (322°, 27° at
/// 18:00), so it shines between them.
pub const CLOUDS: [Cloud; 14] = [
    // Over the square, left of the middle: a broad base under two towers, the golden-hour sun
    // low beside its left flank.
    Cloud {
        azimuth: -8.0,
        base: -3.0,
        masses: &[
            mass(0.0, 42.0, 16.0),
            mass(-9.0, 26.0, 28.0),
            mass(5.0, 26.0, 37.0),
            mass(15.0, 16.0, 22.0),
        ],
        seed: 1,
    },
    // Right of the square: a narrower tower.
    Cloud {
        azimuth: 40.0,
        base: -3.0,
        masses: &[mass(0.0, 26.0, 14.0), mass(-3.0, 20.0, 30.0), mass(7.0, 14.0, 20.0)],
        seed: 2,
    },
    // Floating over the gap between them, and left of the golden-hour sun.
    Cloud { azimuth: 17.0, base: 30.0, masses: &[mass(0.0, 9.0, 4.0), mass(2.0, 5.0, 5.5)], seed: 3 },
    Cloud { azimuth: -58.0, base: 18.0, masses: &[mass(0.0, 12.0, 5.0), mass(-2.0, 7.0, 7.0)], seed: 4 },
    // Low heaps behind the left of the wide shot.
    Cloud { azimuth: -46.0, base: -3.0, masses: &[mass(0.0, 18.0, 9.0), mass(4.0, 9.0, 13.0)], seed: 5 },
    // The sky shot's right, and on round the horizon.
    Cloud { azimuth: 66.0, base: -3.0, masses: &[mass(0.0, 20.0, 10.0), mass(3.0, 10.0, 18.0)], seed: 6 },
    Cloud { azimuth: 62.0, base: 26.0, masses: &[mass(0.0, 10.0, 4.5)], seed: 7 },
    Cloud { azimuth: 95.0, base: -3.0, masses: &[mass(0.0, 26.0, 11.0), mass(-5.0, 12.0, 20.0)], seed: 8 },
    Cloud { azimuth: 140.0, base: -3.0, masses: &[mass(0.0, 30.0, 12.0), mass(6.0, 14.0, 26.0)], seed: 9 },
    Cloud { azimuth: 165.0, base: 22.0, masses: &[mass(0.0, 12.0, 5.0)], seed: 10 },
    Cloud {
        azimuth: 188.0,
        base: -3.0,
        masses: &[mass(0.0, 28.0, 13.0), mass(-4.0, 14.0, 30.0), mass(7.0, 10.0, 18.0)],
        seed: 11,
    },
    Cloud { azimuth: 250.0, base: -3.0, masses: &[mass(0.0, 34.0, 12.0), mass(8.0, 14.0, 24.0)], seed: 12 },
    Cloud { azimuth: 276.0, base: 20.0, masses: &[mass(0.0, 11.0, 5.0)], seed: 13 },
    Cloud { azimuth: 296.0, base: -3.0, masses: &[mass(0.0, 22.0, 10.0), mass(-3.0, 10.0, 17.0)], seed: 14 },
];

/// One lobe: a sphere's front, `depth` toward the viewer, in a cloud's degrees.
#[derive(Clone, Copy, Debug)]
struct Lobe {
    center: Vec2,
    radius: f32,
    depth: f32,
}

/// A cloud's lobes, in degrees across from its middle and up from its base, and how far they
/// reach.
struct Shape {
    cloud: Cloud,
    lobes: Vec<Lobe>,
    /// Half its width and its top, in degrees, lobes and margin included.
    half_width: f32,
    top: f32,
}

impl Shape {
    fn new(cloud: Cloud) -> Self {
        let mut rng = Rng(cloud.seed.wrapping_mul(0x2545_f491));
        let mut lobes = Vec::new();
        for mass in cloud.masses {
            // Lobes about a third of the mass across: big rounded masses, not a heap of stones.
            let size = (0.3 * mass.width.min(1.2 * mass.height)).clamp(1.2, 7.0);
            let rows = ((mass.height - 1.2 * size) / (0.8 * size)).ceil().max(0.0) as u32 + 1;
            for row in 0..rows {
                let y = 0.6 * size + row as f32 * 0.8 * size;
                // The dome's half width where the row's lobes top out: an ellipse's.
                let t = ((y + 0.6 * size) / mass.height).min(1.0);
                let half = 0.5 * mass.width * (1.0 - t * t).max(0.0).sqrt();
                let radius = size * rng.range(0.85, 1.15) * (1.0 - 0.2 * t);
                let span = (half - 0.75 * radius).max(0.0);
                let count = ((2.0 * span) / (1.3 * radius)).ceil().max(1.0) as u32;
                for i in 0..count {
                    let along = if count == 1 { 0.5 } else { i as f32 / (count - 1) as f32 };
                    // Jittered off the row, so the lobes' crescents don't line up into stripes.
                    let x = mass.x - span + 2.0 * span * along + rng.range(-0.25, 0.25) * radius;
                    let center = Vec2::new(x, y + rng.range(-0.2, 0.2) * radius);
                    let lobe = Lobe {
                        center,
                        radius: radius * rng.range(0.85, 1.15),
                        depth: 0.6 * y + rng.range(0.0, 0.5) * radius,
                    };
                    lobes.push(lobe);
                    // The outline's lobes, along the sides and the top rows, bud smaller ones
                    // from their upper arcs: the cauliflower edge.
                    let outline = i == 0 || i + 1 == count || row + 2 >= rows;
                    for _ in 0..if outline { 2 } else { 0 } {
                        let bud = BUDS[(rng.next() * BUDS.len() as f32) as usize % BUDS.len()];
                        lobes.push(Lobe {
                            center: lobe.center + bud * lobe.radius * rng.range(0.75, 0.95),
                            radius: lobe.radius * rng.range(0.38, 0.52),
                            depth: lobe.depth + lobe.radius * 0.3,
                        });
                    }
                }
            }
        }
        let half_width = lobes.iter().map(|l| l.center.x.abs() + l.radius).fold(0.0, f32::max) + CLOUD_MARGIN;
        let top = lobes.iter().map(|l| l.center.y + l.radius).fold(0.0, f32::max) + CLOUD_MARGIN;
        Self { cloud, lobes, half_width, top }
    }

    /// The cloud's texel at `p`, degrees across from its middle (arc along the parallel) and up
    /// from its base: its distance field (degrees, negative inside), its front lobe's normal
    /// across and up, and how open it is to the sky.
    fn sample(&self, p: Vec2) -> (f32, Vec2, f32) {
        let seed = self.cloud.seed;
        // Warp the lookup so lobes aren't perfect circles, and ripple the edge: the irregular,
        // brush-like outline of a painted cloud.
        let warp = Vec2::new(
            fbm(p / 2.8, (NO_PERIOD, NO_PERIOD), 3, seed ^ 0x51),
            fbm(p / 2.8 + Vec2::splat(17.3), (NO_PERIOD, NO_PERIOD), 3, seed ^ 0x52),
        ) - 0.5;
        let q = p + warp * 1.4;
        let mut distance = f32::INFINITY;
        let mut front = f32::NEG_INFINITY;
        // The lobes' fronts: a sphere's height inside it, falling off steeply outside, so the
        // nearest lobe's rim carries on past the edge.
        let height = |lobe: &Lobe| {
            let offset = (q - lobe.center) / lobe.radius;
            let squared = offset.length_squared();
            if squared < 1.0 {
                lobe.depth + lobe.radius * (1.0 - squared).sqrt()
            } else {
                lobe.depth - 3.0 * lobe.radius * (squared.sqrt() - 1.0)
            }
        };
        for lobe in &self.lobes {
            let d = (q - lobe.center).length() - lobe.radius;
            distance = smooth_min(distance, d, 0.3 * lobe.radius);
            front = front.max(height(lobe));
        }
        let ripple = fbm(p / 1.4, (NO_PERIOD, NO_PERIOD), 2, seed ^ 0x53) - 0.5;
        let base = 0.4 * (fbm(Vec2::new(p.x / 3.0, 0.0), (NO_PERIOD, NO_PERIOD), 2, seed ^ 0x54) - 0.5);
        // The flat base: nothing below it.
        let distance = (distance + ripple * 0.25).max(base - q.y);
        // The front lobe's normal and depth, blended softly where two fronts meet.
        let (mut normal, mut front_depth, mut weight) = (Vec2::ZERO, 0.0, 0.0);
        for lobe in &self.lobes {
            let behind = front - height(lobe);
            if behind < 12.0 * FRONT_SOFTNESS {
                let w = (-behind / FRONT_SOFTNESS).exp();
                normal += ((q - lobe.center) / lobe.radius).clamp_length_max(1.0) * w;
                front_depth += lobe.depth * w;
                weight += w;
            }
        }
        let (normal, front_depth) = ((normal / weight).clamp_length_max(1.0), front_depth / weight);
        // Lobes standing in front of the front lobe here close it in around them: contact shading
        // in the crevices, deepest under them.
        let mut occlusion = 0.0f32;
        for lobe in &self.lobes {
            let ahead = smoothstep(0.0, 0.25 * lobe.radius, lobe.depth - front_depth);
            let outside = ((q - lobe.center).length() - lobe.radius).max(0.0);
            let near = 1.0 - smoothstep(0.0, 0.6 * lobe.radius, outside);
            let under = smoothstep(0.0, 1.0, (lobe.center.y - q.y) / lobe.radius);
            occlusion = occlusion.max(ahead * near * (0.35 + 0.35 * under));
        }
        // Under the base, in shade.
        let open = smoothstep(-0.5, 3.0, q.y - base) * (1.0 - occlusion);
        (distance, normal, open)
    }
}

/// A smooth minimum of `a` and `b` over a blend of `k` (Quílez's polynomial smin).
fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    if !a.is_finite() {
        return b;
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

/// `degrees` around the horizon into −180..180.
fn wrap_degrees(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

/// The band's row `y`'s elevation in degrees: its rows follow the sine of the elevation from
/// [`BAND_BASE`] at the bottom edge.
fn row_elevation(y: f32) -> f32 {
    let (width, height) = (CUMULUS_SIZE.0 as f32, CUMULUS_SIZE.1 as f32);
    let band = TAU * height / width;
    let sine = BAND_BASE + (height - y) / height * band;
    sine.clamp(-1.0, 1.0).asin().to_degrees()
}

/// The card band of towering cumulus (see the module docs).
pub fn cumulus_png() -> io::Result<Vec<u8>> {
    let shapes: Vec<Shape> = CLOUDS.iter().map(|&cloud| Shape::new(cloud)).collect();
    let image = fill(CUMULUS_SIZE, |x, y| {
        let azimuth = (x as f32 + 0.5) / TEXELS_PER_DEGREE;
        let elevation = row_elevation(y as f32 + 0.5);
        // Degrees of arc along the parallel at this elevation, and up.
        let arc = elevation.to_radians().cos();
        // Where no cloud is, the map holds the flat normal and an open sky.
        let (mut normal, mut closed, mut field) = (Vec2::ZERO, 0.0, 0.0f32);
        for shape in &shapes {
            let across = wrap_degrees(azimuth - shape.cloud.azimuth) * arc;
            let up = elevation - shape.cloud.base;
            if across.abs() > shape.half_width || up < -CLOUD_MARGIN || up > shape.top {
                continue;
            }
            let (distance, lobe, open) = shape.sample(Vec2::new(across, up));
            let distance = distance * TEXELS_PER_DEGREE;
            // A cloud's normals and closure fade into those past its rim's reach, well inside its
            // margin, so clouds apart add up to each one's own.
            let reach = smoothstep(2.0 * NORMAL_REACH, NORMAL_REACH, distance);
            normal += lobe * reach;
            closed += (1.0 - open) * reach;
            field = field.max(dissolve_field(distance));
        }
        let normal = normal.clamp_length_max(1.0);
        [unorm8(0.5 + 0.5 * normal.x), unorm8(0.5 + 0.5 * normal.y), unorm8(field), unorm8(1.0 - closed)]
    });
    png(image)
}

/// The nebula (see the module docs).
pub fn nebula_png() -> io::Result<Vec<u8>> {
    let pole = GALACTIC_POLE.normalize();
    let first = pole.cross(Vec3::Y).normalize();
    let second = pole.cross(first);
    let (width, height) = NEBULA_SIZE;
    // Noise cells: about 5.6° around the band, and 4.8° across it near its middle.
    const AROUND: i32 = 64;
    // Every noise's frequency around the band is a whole multiple of the band's, so each
    // repeats once around it: no seam where the longitude wraps.
    let image = fill(NEBULA_SIZE, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
        let (theta, phi) = (v * PI, (u - 0.5) * TAU);
        let c = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
        // Galactic latitude's sine, and longitude around the band in turns.
        let across = c.dot(pole);
        let along = c.dot(second).atan2(c.dot(first)) / TAU + 0.5;
        let period = (AROUND, NO_PERIOD);
        let p = Vec2::new(along * AROUND as f32, across * 12.0);
        // The band's course and width wander along it.
        let course = 0.12 * (fbm(Vec2::new(p.x / 8.0, 0.0), (AROUND / 8, NO_PERIOD), 2, 0x7a1) - 0.5);
        let width = 0.22 + 0.22 * fbm(Vec2::new(p.x / 4.0, 3.0), (AROUND / 4, NO_PERIOD), 2, 0x7a2);
        let offset = (across - course) / width;
        let band = (-offset * offset).exp();
        // A broad, gentle warp, so the billows lean and stretch along the band without marbling.
        let warp = Vec2::new(
            fbm(p * 0.25, (AROUND / 4, NO_PERIOD), 3, 0x7b1),
            fbm(p * 0.25 + Vec2::splat(9.1), (AROUND / 4, NO_PERIOD), 3, 0x7b2),
        ) - 0.5;
        let q = p + warp * Vec2::new(5.0, 4.0);
        // Billows: broad soft masses, a finer grain over them.
        let billow = fbm(q * 0.5, (AROUND / 2, NO_PERIOD), 5, 0x7c1);
        let grain = fbm(q * 2.0 + Vec2::splat(2.7), (2 * AROUND, NO_PERIOD), 3, 0x7c3);
        let body = smoothstep(0.36, 0.66, billow) * band * (0.7 + 0.3 * grain);
        // Dust: wide dark rifts across the clouds, ragged at their edges.
        let dust = smoothstep(
            0.5,
            0.64,
            fbm(q * Vec2::new(0.375, 0.45) + Vec2::splat(4.2), (24, NO_PERIOD), 5, 0x7c2),
        );
        let clouds = body * (1.0 - 0.9 * dust);
        // Wisps: the clouds glowing where they meet the rifts, and in a few bright hearts.
        let rim = 4.0 * dust * (1.0 - dust);
        let hearts = smoothstep(0.6, 0.78, fbm(q, period, 4, 0x7d1)) * (1.0 - dust);
        let wisps = ((0.85 * rim + 0.7 * hearts) * smoothstep(0.05, 0.4, body)).min(1.0);
        // Accents: a few broad knots deep in the clouds.
        let knots = smoothstep(0.64, 0.8, fbm(q * 0.5 + Vec2::splat(7.7), (AROUND / 2, NO_PERIOD), 3, 0x7e1));
        let accents = knots * smoothstep(0.3, 0.7, clouds);
        [unorm8(clouds), unorm8(wisps), unorm8(accents), 255]
    });
    let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
    png(rgb)
}

/// A `size` RGBA image from `texel(x, y)`, its rows shared among the cores: each texel depends
/// on its position alone, so the image is the same however they are shared.
fn fill(size: (u32, u32), texel: impl Fn(u32, u32) -> [u8; 4] + Sync) -> image::RgbaImage {
    let (width, height) = (size.0 as usize, size.1 as usize);
    let mut pixels = vec![0u8; width * height * 4];
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rows = height.div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk, band) in pixels.chunks_mut(rows * width * 4).enumerate() {
            let texel = &texel;
            scope.spawn(move || {
                for (index, pixel) in band.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let at = chunk * rows * width + index;
                    *pixel = texel((at % width) as u32, (at / width) as u32);
                }
            });
        }
    });
    image::RgbaImage::from_raw(size.0, size.1, pixels).expect("sized to the image")
}

fn png<P: image::PixelWithColorType>(image: image::ImageBuffer<P, Vec<P::Subpixel>>) -> io::Result<Vec<u8>>
where
    [P::Subpixel]: image::EncodableLayout,
{
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cloud, its margin included, stays under the band's top row, so none is cut flat
    /// there; clouds stand on the horizon, their bases under the band's bottom, or float clear
    /// of it.
    #[test]
    fn every_cloud_fits_the_band() {
        let (top, bottom) = (row_elevation(0.0), row_elevation(CUMULUS_SIZE.1 as f32));
        for cloud in CLOUDS {
            let shape = Shape::new(cloud);
            let (low, high) = (cloud.base - CLOUD_MARGIN, cloud.base + shape.top);
            let at = cloud.azimuth;
            assert!(high <= top, "the cloud at {at}° reaches {high}°, over the band's top at {top}°");
            assert!(
                cloud.base < bottom || low >= bottom,
                "the cloud at {at}° floats at {}°, its margin cut by the band's bottom at {bottom}°",
                cloud.base
            );
        }
    }

    /// Both maps wrap around the sky: across their left and right edges each channel steps no
    /// more than it does between neighbors inside.
    #[test]
    fn the_maps_wrap_without_a_seam() {
        for (name, bytes) in [("cumulus", cumulus_png().unwrap()), ("nebula", nebula_png().unwrap())] {
            let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
            let (width, height) = image.dimensions();
            let step = |a: u32, b: u32, y: u32| {
                let (a, b) = (image.get_pixel(a, y).0, image.get_pixel(b, y).0);
                a.iter().zip(b).map(|(&a, b)| a.abs_diff(b)).max().unwrap()
            };
            let inside = (0..height).flat_map(|y| (0..width - 1).map(move |x| (x, y)));
            let inside = inside.map(|(x, y)| step(x, x + 1, y)).max().unwrap();
            let across = (0..height).map(|y| step(width - 1, 0, y)).max().unwrap();
            assert!(across <= inside, "{name}: {across} across the wrap, {inside} inside");
        }
    }
}
