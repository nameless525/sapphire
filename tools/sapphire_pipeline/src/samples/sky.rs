//! The sandbox sky's content (M1 W2): the maps the sky shader draws its layers from, generated
//! here, with no downloaded or third-party art, and the sky material that names them by asset ID.
//! They are written as ordinary sources (PNG and a material document) and cook as any texture
//! and material do.
//!
//! | File | Size | Class (slot) | What it holds |
//! |---|---|---|---|
//! | `sky_clouds.png` | 4096 × 384 | mask, four channels (`clouds`) | a band of cumulus cards once around the horizon |
//! | `sky_cirrus.png` | 1024 × 1024 | mask, four channels (`cirrus`) | tiling streaks of cirrus |
//! | `sky_moon.png` | 256 × 256 | color (`moon`) | the moon's face, its disc in alpha |
//! | `sky_stars.png` | 512 × 256 | color (`stars`) | where stars are dense, and their tint |
//! | `sky_milky_way.png` | 1024 × 512 | color (`milky_way`) | the Milky Way's glow |
//!
//! Both cloud maps are in the Genshin scheme the shader reads: R the lit amount, whose 0.5 line is
//! the painted terminator; G a painted rim along the edge; B a signed distance to the cloud's edge,
//! 0.5 on the silhouette and rising inward, which time of day's coverage dissolves along; A
//! opacity. They hold smooth fields rather than finished pixels, so the shader keeps edges sharp
//! at any magnification.
//!
//! - **Card clouds:** each card is a cumulus built from puffs, round lobes stacked in a dome over
//!   a flat base, merged smoothly and roughened by noise so the outline reads as painted. Lighting
//!   treats each lobe as the front of a sphere, upper lobes in front of lower ones, lit from above
//!   and a little in front, so every lobe has a lit cap and casts a shaded crescent on the lobe
//!   below; the base is shaded. Towers stand on the horizon (the band's bottom 23 rows sit under
//!   it), smaller cards float higher, and the band runs once around the horizon (no direction
//!   shows another's clouds), joining seamlessly where it wraps.
//! - **Cirrus:** long streaks along u from periodic noise, warped to curl, mostly lit with shaded
//!   patches, translucent at their thin ends.
//! - **The moon:** a flat cream disc with soft blue-grey maria and a few faint craters, the anime
//!   way: a clean shape, little texture.
//! - **Stars and the Milky Way** are equirectangular maps on the celestial sphere (u around its
//!   pole, v from it), as the shader's `dome_texel` reads them: a band of light along a great
//!   circle tilted from the celestial equator, clumped and cut by dark dust lanes, warmer toward
//!   its core. The star map's luminance is 0.5 away from the band (the shader's even density),
//!   up to twice that along it, so stars crowd into the Milky Way.

use std::f32::consts::{PI, TAU};
use std::io;

use glam::{Vec2, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::material::{CloudLayer, Disc, Moon, SkyMaterial, Stars};
use sapphire_serialize::to_ron;

use super::noise::{NO_PERIOD, Rng, fbm, smoothstep};
use crate::color::{linear_to_srgb, unorm8};

/// File names, in the samples directory.
pub const CLOUDS_FILE_NAME: &str = "sky_clouds.png";
pub const CIRRUS_FILE_NAME: &str = "sky_cirrus.png";
pub const MOON_FILE_NAME: &str = "sky_moon.png";
pub const STARS_FILE_NAME: &str = "sky_stars.png";
pub const MILKY_WAY_FILE_NAME: &str = "sky_milky_way.png";
pub const MATERIAL_FILE_NAME: &str = "sky.material.ron";

/// The maps' and the material's asset IDs, which their committed sidecars give them: the material
/// names the maps by ID, and the sandbox loads the material by its.
pub const CLOUDS_ID: AssetId = AssetId::from_text("096e0ddb-5a8a-4fe2-9328-55c315e45ccd");
pub const CIRRUS_ID: AssetId = AssetId::from_text("81b5e21b-ec48-4b86-a352-9d24e5fe3491");
pub const MOON_ID: AssetId = AssetId::from_text("c5d7e99b-310c-4cec-8026-9fa89a7f84d1");
pub const STARS_ID: AssetId = AssetId::from_text("3e53d7a8-5f8d-4889-a649-bc2e818cc17b");
pub const MILKY_WAY_ID: AssetId = AssetId::from_text("119e13cb-6e21-4efa-afa2-d8fb22f177ec");
pub const MATERIAL_ID: AssetId = AssetId::from_text("22d0ccf1-d03c-4387-a00f-42891c86866f");

/// The card cloud band's size in texels: once around the horizon (the material's scale 1), about
/// 11 texels a degree, and 34° tall (the shader keeps texels square).
const BAND_SIZE: (u32, u32) = (4096, 384);
/// Rows of the band under the horizon: the shader's band starts 2° below it.
const BAND_ROWS_BELOW_HORIZON: f32 = 2.0 * BAND_SIZE.0 as f32 / 360.0;
/// How deep inside a cloud, in texels, B runs from the edge (0.5) to 1; outside it falls to 0
/// in half that.
const CLOUD_DISTANCE_RANGE: f32 = 64.0;
/// The painted rim's width inside the edge, in texels.
const CLOUD_RIM_TEXELS: f32 = 4.0;
/// Where the cards' lobes are lit from: above and a little in front.
const CLOUD_LIGHT: Vec3 = Vec3::new(0.2, 0.8, 0.55);
/// The lit amount (lobe normal · light) that falls on the painted terminator.
const CLOUD_TERMINATOR: f32 = 0.3;

const CIRRUS_SIZE: u32 = 1024;
const MOON_SIZE: u32 = 256;
const STARS_SIZE: (u32, u32) = (512, 256);
const MILKY_WAY_SIZE: (u32, u32) = (1024, 512);

/// The Milky Way's plane in the celestial sphere's frame (its pole is +Y): nearly through the
/// celestial poles, so with the default day's turn of the sphere it arcs high across the sky
/// through the evening and is overhead near midnight.
const GALACTIC_POLE: Vec3 = Vec3::new(0.974, 0.099, 0.204);

/// The sky material: the sandbox's zone sky, drawing every map above.
pub fn material() -> SkyMaterial {
    SkyMaterial {
        sun: Disc { size: 2.2, softness: 0.08 },
        moon: Moon { disc: Disc { size: 3.4, softness: 0.04 }, map: Some(MOON_ID) },
        stars: Stars { map: Some(STARS_ID), milky_way: Some(MILKY_WAY_ID), twinkle: 0.35 },
        // Once around the horizon, drifting a turn in about an hour.
        clouds: CloudLayer {
            map: Some(CLOUDS_ID),
            scale: 1.0,
            scroll: [0.00025, 0.0],
            dissolve_softness: 0.006,
        },
        cirrus: CloudLayer {
            map: Some(CIRRUS_ID),
            scale: 1.2,
            scroll: [0.004, 0.0015],
            dissolve_softness: 0.06,
        },
    }
}

/// The sky material document.
pub fn material_document() -> String {
    use sapphire_assets::material::MaterialDocument;
    let material = material();
    material.validate().expect("the sky material is valid");
    to_ron(&material).expect("a material document serializes")
}

/// One lobe of a cumulus card: a sphere's front seen from the camera, `depth` toward it.
#[derive(Clone, Copy, Debug)]
struct Puff {
    center: Vec2,
    radius: f32,
    depth: f32,
}

/// A cumulus card: its puffs, its flat base (texels above the band's bottom), its noise seed and
/// the texels it can reach.
struct Card {
    puffs: Vec<Puff>,
    base: f32,
    seed: u32,
    /// Horizontal center and half-width of the box beyond which it adds nothing, and its top.
    center_x: f32,
    reach: f32,
    top: f32,
}

impl Card {
    /// A cumulus `width` × `height` texels standing on `base`, centered on `center_x`: rows of
    /// puffs filling a dome that is widest at the base and rounded at the top, smaller and further
    /// in front as they rise.
    fn cumulus(center_x: f32, base: f32, width: f32, height: f32, seed: u32) -> Self {
        let mut rng = Rng(seed);
        let mut puffs = Vec::new();
        // Small cards get fewer, larger lobes, so they read as one cloud rather than a cluster.
        let rows = (height / 60.0).clamp(1.0, 5.0) as u32;
        for row in 0..=rows {
            let t = row as f32 / rows as f32;
            let radius = height * (0.34 - 0.16 * t) * rng.range(0.85, 1.15);
            let half_width = 0.5 * width * (1.0 - t.powf(1.6)).max(0.0).powf(0.6);
            let y = base + t * (height - radius) + radius * 0.25;
            let span = (half_width - radius * 0.6).max(0.0);
            let count = ((2.0 * span) / (radius * 1.25)).ceil().max(1.0) as u32;
            for i in 0..count {
                let along = if count == 1 { 0.5 } else { i as f32 / (count - 1) as f32 };
                // Jittered off the row, so the lobes' shaded crescents don't line up into stripes.
                let x = center_x - span + 2.0 * span * along + rng.range(-0.2, 0.2) * radius;
                let lift = rng.range(-0.3, 0.3) * radius;
                puffs.push(Puff {
                    center: Vec2::new(x, y + lift),
                    radius: radius * rng.range(0.8, 1.2),
                    depth: t * height * 0.5 + rng.range(0.0, 0.45) * radius,
                });
            }
        }
        let reach = 0.5 * width + 0.4 * height + CLOUD_DISTANCE_RANGE + 16.0;
        let top = puffs.iter().map(|p| p.center.y + p.radius).fold(base, f32::max);
        Self { puffs, base, seed, center_x, reach, top }
    }

    /// The signed distance (texels, negative inside) from `p` (x across, y up, with `dx` the
    /// wrapped horizontal offset from the card's center) to the card's roughened outline, and the
    /// lit amount there.
    fn sample(&self, p: Vec2) -> (f32, f32) {
        // Warp the lookup so lobes aren't perfect circles, then ripple the edge a little: the
        // irregular, brush-like outline of a painted cloud. The noise repeats with the band.
        let period = (BAND_SIZE.0 as i32 / 32, NO_PERIOD);
        let warp = Vec2::new(
            fbm(p / 32.0, period, 3, self.seed ^ 0x51),
            fbm(p / 32.0 + Vec2::splat(17.3), period, 3, self.seed ^ 0x52),
        ) - 0.5;
        let q = p + warp * 14.0;
        // Smooth union of the puffs' circles, merging lobes into one body.
        let mut distance = f32::INFINITY;
        let mut front = None::<(Puff, f32)>;
        let mut nearest = (f32::INFINITY, self.puffs[0]);
        for puff in &self.puffs {
            let offset = q - puff.center;
            let d = offset.length() - puff.radius;
            distance = smooth_min(distance, d, 0.3 * puff.radius);
            if d < nearest.0 {
                nearest = (d, *puff);
            }
            let inside = puff.radius * puff.radius - offset.length_squared();
            if inside > 0.0 {
                let height = puff.depth + inside.sqrt();
                if front.is_none_or(|(_, best)| height > best) {
                    front = Some((*puff, height));
                }
            }
        }
        let ripple = fbm(p / 16.0, (BAND_SIZE.0 as i32 / 16, NO_PERIOD), 2, self.seed ^ 0x53) - 0.5;
        // The flat base: nothing below it.
        let distance = (distance + ripple * 2.5).max(self.base - q.y);
        // Lit amount from the front lobe's sphere (the nearest lobe's rim outside them all).
        let lobe = front.map_or(nearest.1, |(puff, _)| puff);
        let offset = ((q - lobe.center) / lobe.radius).clamp_length_max(1.0);
        let normal = Vec3::new(offset.x, offset.y, (1.0 - offset.length_squared()).max(0.0).sqrt());
        let lit = normal.dot(CLOUD_LIGHT.normalize());
        // The underside, near the flat base, is in shade.
        let above_base = (q.y - self.base) / (self.top - self.base).max(1.0);
        let lit = lit - 0.5 * (1.0 - smoothstep(0.0, 0.18, above_base));
        (distance, lit)
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

/// The cards of the band, left to right (u runs with the azimuth from −Z toward +X): towers on
/// the horizon in three groups with gaps of clear sky between them, and smaller clouds floating
/// higher. The gaps around 1300 and 2790 texels are where the default day's sun sets (114°) and
/// rises (245°), so the clouds frame it rather than hide it.
fn band_cards() -> Vec<Card> {
    let horizon = BAND_ROWS_BELOW_HORIZON;
    let card = |x: f32, base: f32, width: f32, height: f32, seed: u32| {
        Card::cumulus(x, horizon + base, width, height, seed)
    };
    vec![
        card(180.0, -10.0, 400.0, 240.0, 1),
        card(520.0, -6.0, 220.0, 120.0, 2),
        card(900.0, -14.0, 420.0, 280.0, 3),
        card(1560.0, -8.0, 300.0, 170.0, 4),
        card(1880.0, -12.0, 380.0, 230.0, 5),
        card(2200.0, -6.0, 240.0, 110.0, 6),
        card(2480.0, -12.0, 360.0, 250.0, 7),
        card(3080.0, -10.0, 340.0, 200.0, 8),
        card(3420.0, -14.0, 440.0, 270.0, 9),
        card(3800.0, -8.0, 300.0, 150.0, 10),
        card(4060.0, -6.0, 200.0, 100.0, 11),
        card(430.0, 200.0, 170.0, 72.0, 12),
        card(1300.0, 240.0, 210.0, 84.0, 13),
        card(1740.0, 270.0, 150.0, 62.0, 14),
        card(2350.0, 225.0, 190.0, 70.0, 15),
        card(3000.0, 265.0, 160.0, 60.0, 16),
        card(3650.0, 285.0, 140.0, 50.0, 17),
        card(760.0, 290.0, 120.0, 48.0, 18),
    ]
}

/// The card cloud band (see the module docs).
pub fn clouds_png() -> io::Result<Vec<u8>> {
    let (width, height) = BAND_SIZE;
    let cards = band_cards();
    let band = width as f32;
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        let (px, up) = (x as f32 + 0.5, height as f32 - (y as f32 + 0.5));
        let (mut distance, mut lit) = (CLOUD_DISTANCE_RANGE, 0.0);
        for card in &cards {
            // The band repeats around the horizon: measure across the seam.
            let dx = (px - card.center_x + 0.5 * band).rem_euclid(band) - 0.5 * band;
            if dx.abs() > card.reach || up > card.top + CLOUD_DISTANCE_RANGE {
                continue;
            }
            let (d, l) = card.sample(Vec2::new(card.center_x + dx, up));
            if d < distance {
                (distance, lit) = (d, l);
            }
        }
        let b = dissolve_field(distance);
        let r = 0.5 + (lit - CLOUD_TERMINATOR) * 1.25;
        let g = 1.0 - smoothstep(0.0, CLOUD_RIM_TEXELS, -distance);
        image::Rgba([unorm8(r), unorm8(g), unorm8(b), 255])
    });
    png(image)
}

/// B for a texel `distance` texels from a cloud's edge (negative inside): 0.5 on the edge, rising
/// inward as `1 − (1 − t)²` of the depth `t` (in [`CLOUD_DISTANCE_RANGE`]s), so a cloud keeps most
/// of its shape while coverage is high and loses its cores last, and falling outward at the slope
/// it leaves the edge with, so the silhouette's antialiasing is even on both sides. The slope at
/// the edge, two 8-bit steps a texel, keeps the 8-bit (and block-compressed) contour within about
/// a quarter of a texel.
fn dissolve_field(distance: f32) -> f32 {
    let depth = -distance / CLOUD_DISTANCE_RANGE;
    if depth > 0.0 {
        let rest = 1.0 - depth.min(1.0);
        0.5 + 0.5 * (1.0 - rest * rest)
    } else {
        0.5 + depth
    }
}

/// The cirrus map (see the module docs): streaks along u, tiling both ways.
pub fn cirrus_png() -> io::Result<Vec<u8>> {
    let size = CIRRUS_SIZE;
    let image = image::RgbaImage::from_fn(size, size, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / size as f32;
        // Long thin features: 3 cells across and 20 down the map, warped so they curl.
        let curl = fbm(p * 2.0, (2, 2), 3, 0xc1) - 0.5;
        let streak = Vec2::new(p.x * 3.0, (p.y + curl * 0.1) * 16.0);
        let density = fbm(streak, (3, 16), 3, 0xc2);
        let patches = fbm(p * 4.0, (4, 4), 2, 0xc3);
        let b = 0.5 + (density - 0.56) * 2.0;
        let a = 0.25 + 0.75 * smoothstep(0.56, 0.78, density);
        let r = 0.85 - 0.3 * smoothstep(0.6, 0.85, patches);
        let g = 1.0 - smoothstep(0.5, 0.55, b);
        image::Rgba([unorm8(r), unorm8(g), unorm8(b), unorm8(a)])
    });
    png(image)
}

/// The moon's face (see the module docs): sRGB color, the disc in alpha.
pub fn moon_png() -> io::Result<Vec<u8>> {
    let size = MOON_SIZE as f32;
    // Maria as soft blobs, (center, radius, strength), in the face's unit square.
    let maria = [
        (Vec2::new(0.38, 0.36), 0.17, 0.3),
        (Vec2::new(0.58, 0.3), 0.12, 0.25),
        (Vec2::new(0.64, 0.54), 0.14, 0.28),
        (Vec2::new(0.42, 0.63), 0.1, 0.2),
        (Vec2::new(0.3, 0.5), 0.08, 0.16),
    ];
    let craters =
        [(Vec2::new(0.55, 0.78), 0.035), (Vec2::new(0.74, 0.36), 0.025), (Vec2::new(0.26, 0.3), 0.02)];
    let face = Vec3::new(0.94, 0.9, 0.79);
    let sea = Vec3::new(0.66, 0.7, 0.8);
    let image = image::RgbaImage::from_fn(MOON_SIZE, MOON_SIZE, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / size;
        let from_center = (p - 0.5).length();
        let alpha = 1.0 - smoothstep(0.47 - 1.0 / size, 0.47 + 1.0 / size, from_center);
        let mottle = fbm(p * 8.0, (8, 8), 3, 0x3001) - 0.5;
        let mut sea_weight = 0.0f32;
        for (center, radius, strength) in maria {
            let d = (p - center).length() / radius;
            sea_weight = sea_weight.max(strength * (1.0 - smoothstep(0.5, 1.0 + mottle * 0.6, d)));
        }
        let mut color = face.lerp(sea, sea_weight.clamp(0.0, 1.0));
        for (center, radius) in craters {
            let d = (p - center).length() / radius;
            let ring = smoothstep(0.7, 0.9, d) * (1.0 - smoothstep(0.9, 1.2, d));
            color *= 1.0 - 0.06 * ring;
        }
        let [r, g, b] = color.to_array().map(|c| unorm8(linear_to_srgb(c)));
        image::Rgba([r, g, b, unorm8(alpha)])
    });
    png(image)
}

/// The Milky Way's band at celestial direction `c`: its glow (0 to 1), how close to its warm core,
/// and its dust lanes (0 to 1).
fn milky_way(c: Vec3) -> (f32, f32, f32) {
    let pole = GALACTIC_POLE.normalize();
    let across = c.dot(pole);
    // Along the band: an angle around its pole, 0 at its core.
    let first = pole.cross(Vec3::Y).normalize();
    let second = pole.cross(first);
    let along = c.dot(second).atan2(c.dot(first));
    let band = Vec2::new(along / TAU * 48.0, across * 24.0);
    let clumps = smoothstep(0.3, 0.75, fbm(band, (48, NO_PERIOD), 4, 0x6a1));
    let glow = (-(across * across) / (0.15 * 0.15)).exp() * (0.25 + 1.1 * clumps)
        + 0.15 * (-(across * across) / (0.32 * 0.32)).exp();
    let core = (-(along * along) / (0.7 * 0.7)).exp();
    let lane_center = 0.03 * (3.0 * along).sin();
    let lane_noise = fbm(band * Vec2::new(1.0, 2.0) + Vec2::splat(5.0), (48, NO_PERIOD), 3, 0x6a2);
    let lanes = (-((across - lane_center).powi(2)) / (0.05 * 0.05)).exp() * smoothstep(0.3, 0.65, lane_noise);
    (glow.clamp(0.0, 1.5), core, lanes)
}

/// A map on the celestial sphere, `size` texels, each `texel(direction)` in linear RGB, encoded
/// sRGB: u turns around the pole from +X toward +Z, v runs from the pole (+Y) down.
fn dome_png(size: (u32, u32), texel: impl Fn(Vec3) -> Vec3) -> io::Result<Vec<u8>> {
    let image = image::RgbImage::from_fn(size.0, size.1, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / size.0 as f32, (y as f32 + 0.5) / size.1 as f32);
        let (theta, phi) = (v * PI, (u - 0.5) * TAU);
        let c = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
        let rgb = texel(c).to_array().map(|value| unorm8(linear_to_srgb(value)));
        image::Rgb(rgb)
    });
    png(image)
}

/// Where stars are dense and their tint (see the module docs).
pub fn stars_png() -> io::Result<Vec<u8>> {
    dome_png(STARS_SIZE, |c| {
        let (glow, core, lanes) = milky_way(c);
        let density = (0.5 + 0.5 * glow - 0.3 * lanes).clamp(0.15, 1.0);
        let tint = Vec3::new(1.0, 0.97, 0.94).lerp(Vec3::new(1.0, 0.9, 0.78), core * glow.min(1.0));
        tint * density
    })
}

/// The Milky Way's glow (see the module docs), dim: the night sky's brightness scales it.
pub fn milky_way_png() -> io::Result<Vec<u8>> {
    let cool = Vec3::new(0.42, 0.52, 0.95);
    let warm = Vec3::new(0.95, 0.76, 0.58);
    dome_png(MILKY_WAY_SIZE, |c| {
        let (glow, core, lanes) = milky_way(c);
        let light = (glow * (1.0 - 0.8 * lanes)).max(0.0);
        cool.lerp(warm, core) * light * 0.12
    })
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
    use std::path::Path;

    use sapphire_assets::material::AnyMaterialDocument;

    use super::*;

    /// The sRGB-decoded value of an 8-bit sample.
    fn linear(value: u8) -> f32 {
        crate::color::srgb_to_linear(f32::from(value) / 255.0)
    }

    fn decode(bytes: &[u8]) -> image::RgbaImage {
        image::load_from_memory(bytes).expect("a PNG").to_rgba8()
    }

    #[test]
    fn the_cloud_band_has_cards_on_the_horizon_and_clear_sky_above() {
        let band = decode(&clouds_png().unwrap());
        assert_eq!(band.dimensions(), BAND_SIZE);
        let inside = |x: u32, y: u32| band.get_pixel(x, y)[2] > 128;
        // Every column of the horizon's row under the tallest card is cloud; the top rows are
        // clear everywhere, so the band's top edge never shows.
        let horizon = BAND_SIZE.1 - BAND_ROWS_BELOW_HORIZON as u32 - 4;
        assert!((800..1000).all(|x| inside(x, horizon)), "a tall card stands on the horizon");
        assert!((0..BAND_SIZE.0).all(|x| !inside(x, 0) && !inside(x, 20)), "clear at the top");
        // Gaps of clear sky between groups, where the default day's sun rises and sets, and some
        // cloud higher up.
        let covered = (0..BAND_SIZE.0).filter(|&x| inside(x, horizon)).count() as f32 / BAND_SIZE.0 as f32;
        assert!((0.5..0.9).contains(&covered), "{covered} of the horizon is cloud");
        for (gap, at) in [((2750..2830), "sunrise"), ((1260..1340), "sunset")] {
            assert!(gap.clone().all(|x| !inside(x, horizon) && !inside(x, horizon - 40)), "clear at {at}");
        }
        assert!((1240..1360).any(|x| inside(x, BAND_SIZE.1 - 290)), "a floating card");
        // The distance rises inward, the rim hugs the edge, and lobes are lit on top, shaded at
        // the base.
        let column = 900;
        let rows: Vec<u8> = (0..BAND_SIZE.1).map(|y| band.get_pixel(column, y)[2]).collect();
        assert!(rows.iter().max().copied().unwrap() > 240, "a deep core");
        let edge = (0..BAND_SIZE.1).find(|&y| inside(column, y)).expect("cloud in the column");
        assert!(band.get_pixel(column, edge)[1] > 128, "rim at the top edge");
        assert!(band.get_pixel(column, edge + 20)[0] > 128, "lit cap");
        assert!(band.get_pixel(column, horizon)[0] < 128, "shaded base");
    }

    #[test]
    fn the_band_repeats_across_its_seam() {
        let band = decode(&clouds_png().unwrap());
        let last = BAND_SIZE.0 - 1;
        for y in (0..BAND_SIZE.1).step_by(7) {
            let (left, right) = (band.get_pixel(0, y)[2], band.get_pixel(last, y)[2]);
            assert!(left.abs_diff(right) <= 6, "row {y}: {left} vs {right}");
        }
    }

    #[test]
    fn the_cirrus_tiles_and_the_moon_is_a_disc() {
        let cirrus = decode(&cirrus_png().unwrap());
        let last = CIRRUS_SIZE - 1;
        for i in (0..CIRRUS_SIZE).step_by(13) {
            assert!(cirrus.get_pixel(0, i)[2].abs_diff(cirrus.get_pixel(last, i)[2]) <= 12, "u seam at {i}");
            assert!(cirrus.get_pixel(i, 0)[2].abs_diff(cirrus.get_pixel(i, last)[2]) <= 12, "v seam at {i}");
        }
        let visible =
            cirrus.pixels().filter(|p| p[2] > 128).count() as f32 / (CIRRUS_SIZE * CIRRUS_SIZE) as f32;
        assert!((0.1..0.6).contains(&visible), "{visible} of the cirrus shows at full coverage");
        let moon = decode(&moon_png().unwrap());
        let center = MOON_SIZE / 2;
        assert_eq!(moon.get_pixel(center, center)[3], 255);
        assert_eq!(moon.get_pixel(0, 0)[3], 0, "outside the disc");
        assert!(linear(moon.get_pixel(center + 50, center - 40)[0]) > 0.5, "a pale face");
    }

    #[test]
    fn stars_crowd_into_the_milky_way() {
        let stars = decode(&stars_png().unwrap());
        let luminance =
            |p: &image::Rgba<u8>| 0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2]);
        let mean: f32 = stars.pixels().map(luminance).sum::<f32>() / stars.pixels().len() as f32;
        assert!((0.5..0.7).contains(&mean), "about the even density on average: {mean}");
        let brightest = stars.pixels().map(luminance).fold(0.0, f32::max);
        assert!(brightest > 0.85, "dense along the band: {brightest}");
        let milky_way = decode(&milky_way_png().unwrap());
        assert!(milky_way.pixels().any(|p| linear(p[2]) > 0.08), "a visible band");
        assert!(
            milky_way.pixels().filter(|p| p[2] < 4).count() > milky_way.pixels().len() / 3,
            "dark away from it"
        );
    }

    #[test]
    fn the_material_names_the_maps_by_their_sidecars_ids() {
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples");
        for (file, id) in [
            (CLOUDS_FILE_NAME, CLOUDS_ID),
            (CIRRUS_FILE_NAME, CIRRUS_ID),
            (MOON_FILE_NAME, MOON_ID),
            (STARS_FILE_NAME, STARS_ID),
            (MILKY_WAY_FILE_NAME, MILKY_WAY_ID),
            (MATERIAL_FILE_NAME, MATERIAL_ID),
        ] {
            let meta = crate::meta::read(&crate::meta::sidecar_path(&samples.join(file))).expect("a sidecar");
            assert_eq!(meta.id, id, "{file}");
        }
        let document = AnyMaterialDocument::from_ron(&material_document()).expect("a material document");
        assert_eq!(
            document.textures(),
            [Some(MOON_ID), Some(STARS_ID), Some(MILKY_WAY_ID), Some(CLOUDS_ID), Some(CIRRUS_ID)]
        );
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(moon_png().unwrap(), moon_png().unwrap());
        assert_eq!(stars_png().unwrap(), stars_png().unwrap());
        assert_eq!(material_document(), material_document());
    }
}
