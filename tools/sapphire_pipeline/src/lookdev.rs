//! The look-dev scene's generated sources (M1 X1): the painted maps the town's environment
//! materials read, written under `content/lookdev/` by `sapphire_pipeline gen-lookdev`.
//!
//! The town itself is authored, not generated: `town/town.assembly.ron` ([`crate::assembly`])
//! builds Jinzhou's halls, shops, walls, roofs and lanterns from the assembly's parametric
//! builtins, and places the Chinese pavilion (`town/pieces/pavilion.glb`) and a few of Kenney's
//! Fantasy Town Kit's pieces (market stalls, a cart, the far rocks); `town/materials/` holds its
//! materials. Its maps are painted here instead, in code, so they regenerate byte for byte and owe
//! nothing to a kit's palette:
//!
//! | File (in `town/textures/`) | Repeats every | What |
//! |---|---|---|
//! | `stone.png` | 2 m | Ashlar: courses of cut blocks, each its own warm grey, with painted bevels and mortar |
//! | `timber.png` | 1 m | Planks and beams: soft streaks along V, a seam every quarter meter; near white, tinted |
//! | `grass.png` | 6 m | A vivid lawn: yellow-green patches and darker clumps |
//! | `foliage.png` | 2 m | Leaf clusters, light on their upper sides, for tree crowns and hedges |
//! | `rock.png` | 16 m | Banded rock for the far cliffs, which fog and distance mostly paint over |
//! | `band.png` | — | The terminator band: white ends, a warm apricot across the transition |
//! | `whitewash.png` | 3.2 m across, once down a wall | Jinzhou's weathered plaster: grey-white with cool and warm mottling, a stain and streaks under the eaves, grime rising from the plinth |
//! | `granite.png` | 2 m | Cool grey blocks in four courses, bevelled, in dark joints: plinths and stone trim |
//! | `tiles.png` | 4 tile periods, 1.2 m | The tile rows on their modeled phase (channels dark, cover tiles rounded to their crests, the seams between) and four courses, each lip lit and shaded under; near white |
//! | `tiles_mask.png` | as `tiles.png` | Its mask, linear: occlusion in the channels, seams and under each lip; cavity at the seams; specular on the crests |
//! | `rafters.png` | 1.2 m | A soffit's rafters, four to the map, along V over darker boards between; near white |
//! | `flagstones.png` | 4 m | Dark grey stone slabs in staggered rows, five rows of three or four, each its own cool grey, in thin darker joints |
//! | `lantern.png` | one rib | A lantern's paper between two ribs, brightest between them, with bands at top and bottom; its emission map too |
//!
//! Colors are picked in 8-bit sRGB, as an artist would, and maps tile seamlessly: every pattern
//! repeats on the map's own period (whitewash across only: it runs once down a wall).
//!
//! X1 part 2 adds two more sets of generated sources:
//!
//! | File | What |
//! |---|---|
//! | `foliage/{tree_green,tree_blossom,tree_autumn,bush_green,bush_lavender,grass}.glb` | W6's foliage samples' geometry ([`PLANTS`]), one file per plant so each sidecar binds the plant's own look-dev foliage material (`foliage/materials/`) |
//! | `sky/milky_way.png` | The night's Milky Way in teal, blue-violet and cyan-white ([`MILKY_WAY`]), which the look-dev sky (`sky/sky.material.ron`) reads in place of the sandbox's |
//!
//! The kit's own trees become foliage through single-piece assemblies
//! (`foliage/kit_tree*.assembly.ron`), authored, not generated.
//!
//! The character (pixiv's VRM sample, A6) brings its own textures, extracted byte for byte under
//! `content/third_party/`; its look-dev materials, face map and masks are in `character/`. One
//! of its maps is derived here: **`character/hair_mask.png`** ([`hair_mask_png`]), the hair
//! mask C5's hair shading reads, from the model's painted highlight.

use std::io;

use glam::{Vec2, Vec3};
use sapphire_assets::character_maps::hair_mask;

use crate::color::{linear_to_srgb, srgb_to_linear, unorm8};
use crate::samples::noise::{Rng, fbm, hash, smoothstep};
use crate::samples::{foliage, sky};

/// The town's directory, under the look-dev directory.
pub const TOWN_DIR: &str = "town";

/// The look-dev plants' directory, under the look-dev directory.
pub const FOLIAGE_DIR: &str = "foliage";

/// The look-dev sky's directory, under the look-dev directory.
pub const SKY_DIR: &str = "sky";

/// The look-dev plants made from W6's foliage samples: each file the sample's geometry
/// (`samples::foliage`), so its own sidecar can bind its slots to a look-dev material of its own
/// (a sidecar binds materials per source file). `(file, sample)`.
pub const PLANTS: [(&str, Plant); 6] = [
    ("tree_green.glb", Plant::Tree),
    ("tree_blossom.glb", Plant::Tree),
    ("tree_autumn.glb", Plant::Tree),
    ("bush_green.glb", Plant::Bush),
    ("bush_lavender.glb", Plant::Bush),
    ("grass.glb", Plant::Grass),
];

/// A foliage sample's geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plant {
    /// A trunk and branches (`Bark`) carrying six clumps of leaf cards (`Leaves`).
    Tree,
    /// A dome of leaf cards (`Leaves`).
    Bush,
    /// A patch of grass cards 4 m across (`Blades`).
    Grass,
}

impl Plant {
    fn glb(self) -> Vec<u8> {
        match self {
            Self::Tree => foliage::tree_glb(),
            Self::Bush => foliage::bush_glb(),
            Self::Grass => foliage::grass_glb(),
        }
    }
}

/// The look-dev night's Milky Way: teal at the band's edges, a blue-violet core and cyan-white
/// clumps, brighter than the sample's, toward the painted nebulae of Wuthering Waves' night skies
/// within what the sky's Milky Way map can show (a glow under the stars, no shapes of its own).
pub const MILKY_WAY: sky::MilkyWayPalette = sky::MilkyWayPalette {
    cool: Vec3::new(0.08, 0.46, 0.62),
    warm: Vec3::new(0.42, 0.46, 0.95),
    clumps: Vec3::new(0.55, 0.95, 1.0),
    clump_tint: 0.6,
    brightness: 0.2,
};

/// The look-dev character's painted hair highlight (VRoid's `Hair_00_spe` layer, extracted byte
/// for byte from the model), relative to the look-dev directory: [`hair_mask_png`]'s input.
pub const HAIR_HIGHLIGHT: &str = "../third_party/vrm1_constraint_twist_sample/extracted/Hair_00_spe.png";

/// The character's hair mask, relative to the look-dev directory.
pub const HAIR_MASK: &str = "character/hair_mask.png";

/// Every generated file, relative to the look-dev directory, with its bytes.
pub fn files() -> io::Result<Vec<(String, Vec<u8>)>> {
    let path = |name: &str| format!("{TOWN_DIR}/textures/{name}");
    Ok(vec![
        (path("stone.png"), stone_png()?),
        (path("timber.png"), timber_png()?),
        (path("grass.png"), grass_png()?),
        (path("foliage.png"), foliage_png()?),
        (path("rock.png"), rock_png()?),
        (path("band.png"), band_png()?),
        (path("whitewash.png"), whitewash_png()?),
        (path("granite.png"), granite_png()?),
        (path("tiles.png"), tiles_png()?),
        (path("tiles_mask.png"), tiles_mask_png()?),
        (path("rafters.png"), rafters_png()?),
        (path("lantern.png"), lantern_png()?),
        (path("flagstones.png"), flagstones_png()?),
    ])
}

/// The generated plants and the sky's Milky Way, relative to the look-dev directory, with their
/// bytes: the sources [`files`]'s maps don't cover.
pub fn content_files() -> io::Result<Vec<(String, Vec<u8>)>> {
    let mut generated: Vec<(String, Vec<u8>)> =
        PLANTS.iter().map(|&(name, plant)| (format!("{FOLIAGE_DIR}/{name}"), plant.glb())).collect();
    generated.push((format!("{SKY_DIR}/milky_way.png"), sky::milky_way_png_in(&MILKY_WAY)?));
    Ok(generated)
}

/// Writes every generated file into the look-dev directory `dir` (creating the directories they
/// sit in): the town's maps and the character's hair mask, which [`HAIR_HIGHLIGHT`] under `dir`
/// is read for. Returns their paths.
pub fn write_all(dir: &std::path::Path) -> io::Result<Vec<std::path::PathBuf>> {
    let highlight = std::fs::read(dir.join(HAIR_HIGHLIGHT))?;
    let mut generated = files()?;
    generated.extend(content_files()?);
    generated.push((HAIR_MASK.to_owned(), hair_mask_png(&highlight)?));
    let mut paths = Vec::new();
    for (name, bytes) in generated {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap_or(dir))?;
        std::fs::write(&path, bytes)?;
        paths.push(path);
    }
    Ok(paths)
}

/// Linear RGB from 8-bit sRGB.
fn rgb(r: u8, g: u8, b: u8) -> Vec3 {
    Vec3::new(f32::from(r), f32::from(g), f32::from(b)).map(|c| srgb_to_linear(c / 255.0))
}

/// A square map of `size` texels from a function of the texel's center in `0..1`, returning
/// linear RGB.
fn paint(size: u32, texel: impl Fn(Vec2) -> Vec3) -> io::Result<Vec<u8>> {
    let image = image::RgbImage::from_fn(size, size, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / size as f32;
        image::Rgb(texel(p).to_array().map(|c| unorm8(linear_to_srgb(c.max(0.0)))))
    });
    png(image)
}

fn png<P: image::PixelWithColorType>(image: image::ImageBuffer<P, Vec<P::Subpixel>>) -> io::Result<Vec<u8>>
where
    [P::Subpixel]: image::EncodableLayout,
{
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

/// Fractal noise in 0..1 over the unit square, tiling, with `cells` lattice cells across.
fn tiling(p: Vec2, cells: i32, octaves: u32, seed: u32) -> f32 {
    fbm(p * cells as f32, (cells, cells), octaves, seed)
}

/// The nearest and second-nearest of a jittered grid of `cells` × `cells` points over the
/// unit square, tiling: `(f1, f2, id)` in cell units, the id unique per point.
fn voronoi(p: Vec2, cells: i32, jitter: f32, seed: u32) -> (f32, f32, u32) {
    let q = p * cells as f32;
    let base = q.floor();
    let (mut f1, mut f2, mut id) = (f32::MAX, f32::MAX, 0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cell = base + Vec2::new(dx as f32, dy as f32);
            let (wx, wy) = ((cell.x as i32).rem_euclid(cells), (cell.y as i32).rem_euclid(cells));
            let h = hash(seed ^ hash((wx * 7919 + wy) as u32));
            let offset = Vec2::new((h & 0xffff) as f32 / 65535.0, (h >> 16) as f32 / 65535.0);
            let point = cell + Vec2::splat(0.5) + (offset - Vec2::splat(0.5)) * jitter;
            let d = q.distance(point);
            if d < f1 {
                (f2, f1, id) = (f1, d, (wy * cells + wx) as u32);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

/// A random value in 0..1 for `id` under `seed`.
fn random(id: u32, seed: u32) -> f32 {
    hash(id ^ hash(seed)) as f32 / 4_294_967_296.0
}

/// Ashlar: five courses of blocks, staggered, each block tinted on its own, bevelled light on
/// its upper left and shaded on its lower right, in light mortar.
pub fn stone_png() -> io::Result<Vec<u8>> {
    const COURSES: u32 = 5;
    let mortar = rgb(170, 160, 146);
    let tints =
        [rgb(214, 204, 186), rgb(200, 190, 174), rgb(222, 212, 192), rgb(206, 192, 180), rgb(194, 192, 186)];
    // Each course's joints: block widths of 0.25 to 0.45 of the map, wrapping.
    let joints: Vec<Vec<f32>> = (0..COURSES)
        .map(|course| {
            let mut rng = Rng(0x5701 + course);
            let mut at = rng.next();
            let mut list = Vec::new();
            let start = at;
            while at < start + 1.0 - 0.2 {
                list.push(at.rem_euclid(1.0));
                at += rng.range(0.25, 0.45);
            }
            list.sort_by(f32::total_cmp);
            list
        })
        .collect();
    paint(512, |p| {
        let course = ((p.y * COURSES as f32) as u32).min(COURSES - 1);
        let within_y = p.y * COURSES as f32 - course as f32;
        let row = &joints[course as usize];
        // The block: from the last joint at or left of x to the next, wrapping.
        let index = row.iter().rposition(|&joint| joint <= p.x).unwrap_or(row.len() - 1);
        let left = if row[index] > p.x { row[index] - 1.0 } else { row[index] };
        let right = row.get(index + 1).copied().unwrap_or(row[0] + 1.0);
        // Distances to the block's edges, in map units.
        let (to_left, to_right) = (p.x - left, right - p.x);
        let (to_top, to_bottom) = (within_y / COURSES as f32, (1.0 - within_y) / COURSES as f32);
        let id = course * 64 + index as u32;
        let tint = tints[(random(id, 0x5702) * tints.len() as f32) as usize % tints.len()];
        let mut color = tint * (0.94 + 0.1 * random(id, 0x5703));
        // A painted bevel: light along the top and left edges, shade along the bottom and right.
        let lit = 1.0 - smoothstep(0.004, 0.02, to_top.min(to_left));
        let shaded = 1.0 - smoothstep(0.004, 0.02, to_bottom.min(to_right));
        color *= 1.0 + 0.07 * lit - 0.1 * shaded;
        color *= 0.96 + 0.08 * tiling(p, 16, 3, 0x5704);
        // Mortar about 1.5 cm wide on the 2 m map.
        let edge = to_left.min(to_right).min(to_top).min(to_bottom);
        mortar.lerp(color, smoothstep(0.002, 0.005, edge))
    })
}

/// Timber: planks a quarter of the map wide, their grain streaked along V, each plank its own
/// shade; near white.
pub fn timber_png() -> io::Result<Vec<u8>> {
    const PLANKS: u32 = 4;
    paint(256, |p| {
        let plank = ((p.x * PLANKS as f32) as u32).min(PLANKS - 1);
        let within = p.x * PLANKS as f32 - plank as f32;
        let mut value = 0.84 + 0.12 * random(plank, 0x7b01);
        // Grain: noise stretched along V.
        let grain = fbm(Vec2::new(p.x * 96.0, p.y * 6.0), (96, 6), 3, 0x7b02 + plank);
        value *= 0.9 + 0.16 * grain;
        let seam = within.min(1.0 - within);
        value *= 0.75 + 0.25 * smoothstep(0.0, 0.05, seam);
        Vec3::new(value, value * 0.97, value * 0.94)
    })
}

/// Lawn: vivid green with yellow-green patches and darker clumps.
pub fn grass_png() -> io::Result<Vec<u8>> {
    let green = rgb(112, 170, 74);
    let yellow = rgb(156, 190, 84);
    let dark = rgb(82, 140, 66);
    paint(512, |p| {
        let patches = tiling(p, 4, 4, 0x9a01);
        let clumps = tiling(p + Vec2::splat(0.2), 24, 3, 0x9a02);
        let blades = tiling(p, 96, 2, 0x9a03);
        let mut color = green.lerp(yellow, 0.7 * smoothstep(0.45, 0.7, patches));
        color = color.lerp(dark, 0.45 * smoothstep(0.55, 0.8, clumps));
        color * (0.93 + 0.12 * blades)
    })
}

/// Leaf clusters: overlapping round clumps, each lighter on its upper side, in three greens.
pub fn foliage_png() -> io::Result<Vec<u8>> {
    let light = rgb(150, 198, 96);
    let mid = rgb(98, 156, 74);
    let dark = rgb(58, 108, 62);
    paint(256, |p| {
        let (f1, _, id) = voronoi(p, 6, 0.9, 0xf011);
        let shade = random(id, 0xf012);
        // Within a clump: light toward its top, dark at its rim.
        let (g1, _, _) = voronoi(p + Vec2::new(0.0, 0.03), 6, 0.9, 0xf011);
        let up = smoothstep(-0.1, 0.25, g1 - f1);
        let rim = smoothstep(0.25, 0.65, f1);
        let mut color = mid.lerp(light, 0.65 * up + 0.2 * shade);
        color = color.lerp(dark, 0.7 * rim);
        color * (0.94 + 0.1 * tiling(p, 48, 2, 0xf013))
    })
}

/// Banded rock: layers of blue-grey and warm grey, broken by noise.
pub fn rock_png() -> io::Result<Vec<u8>> {
    let cool = rgb(150, 156, 170);
    let warm = rgb(176, 168, 158);
    let dark = rgb(116, 118, 132);
    paint(256, |p| {
        let warp = tiling(p, 4, 3, 0x40c1) - 0.5;
        let layers = ((p.y + 0.12 * warp) * 7.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        let mut color = cool.lerp(warm, smoothstep(0.3, 0.7, layers));
        color = color.lerp(dark, 0.5 * smoothstep(0.6, 0.85, tiling(p, 12, 3, 0x40c2)));
        color * (0.95 + 0.08 * tiling(p, 48, 2, 0x40c3))
    })
}

/// Whitewash, weathered: grey-white lime plaster with cool and warm mottling. V runs once down
/// a wall (`assembly`'s plaster coordinates: 0 under its eaves, 1 at its plinth), so the map
/// carries what weather does there: a stain under the eaves with soft streaks hanging from it, each
/// its own length, and grime rising unevenly from the plinth. U repeats every 3.2 m.
pub fn whitewash_png() -> io::Result<Vec<u8>> {
    const STREAKS: i32 = 24;
    let base = rgb(234, 233, 228);
    let cool = rgb(214, 218, 222);
    let warm = rgb(232, 225, 212);
    let stain = rgb(176, 174, 168);
    let grime = rgb(146, 142, 132);
    paint(512, |p| {
        let broad = tiling(p, 3, 4, 0x3a01);
        let patches = tiling(p + Vec2::splat(0.27), 7, 3, 0x3a02);
        let grain = tiling(p, 64, 2, 0x3a04);
        let mut color = base.lerp(cool, 0.7 * smoothstep(0.5, 0.85, broad));
        color = color.lerp(warm, 0.6 * smoothstep(0.58, 0.85, patches));
        // Streaks from the eaves: in some of 24 columns, each reaching its own way down, wavering.
        let wobble = 0.6 * (fbm(Vec2::new(p.x * 8.0, p.y * 4.0), (8, 4), 2, 0x3a05) - 0.5);
        let along = p.x * STREAKS as f32 + wobble;
        let column = along.floor().rem_euclid(STREAKS as f32) as u32;
        let within = (along.fract() - 0.5).abs();
        let present = random(column, 0x3a06) < 0.6;
        let length = 0.12 + 0.5 * random(column, 0x3a07);
        let strength = 0.2 + 0.35 * random(column, 0x3a08);
        let width = 0.3 + 0.25 * random(column, 0x3a09);
        let streak = if present {
            strength
                * (1.0 - smoothstep(0.1 * width, width, within))
                * (1.0 - smoothstep(0.1 * length, length, p.y))
        } else {
            0.0
        };
        // The stain right under the eaves.
        let under_eaves = 0.7 * (1.0 - smoothstep(0.0, 0.14, p.y)) * (0.7 + 0.3 * broad);
        color = color.lerp(stain, (streak + under_eaves).min(0.8));
        // Grime rising from the plinth to an uneven edge, splashed above it.
        let edge = 0.8 + 0.08 * (fbm(Vec2::new(p.x * 12.0, 0.5), (12, 4), 3, 0x3a0a) - 0.5);
        let splash = tiling(p, 48, 2, 0x3a0b);
        let rising = smoothstep(edge - 0.08, 1.0, p.y) * (0.75 + 0.25 * splash);
        color = color.lerp(grime, 0.75 * rising);
        color * (0.985 + 0.03 * grain)
    })
}

/// Granite: four courses of blocks in cool greys, staggered, bevelled light on their upper left
/// and shaded on their lower right, in dark joints.
pub fn granite_png() -> io::Result<Vec<u8>> {
    const COURSES: u32 = 4;
    let joint = rgb(96, 100, 108);
    let tints =
        [rgb(178, 182, 188), rgb(166, 170, 178), rgb(186, 188, 190), rgb(170, 172, 176), rgb(160, 166, 174)];
    paint(512, |p| {
        let course = ((p.y * COURSES as f32) as u32).min(COURSES - 1);
        let within_y = p.y * COURSES as f32 - course as f32;
        // Blocks a third of the map long, each course shifted.
        let shift = random(course, 0x6a01);
        let along = (p.x + shift).rem_euclid(1.0) * 3.0;
        let block = along.floor();
        let within_x = along - block;
        let id = course * 16 + block as u32;
        let tint = tints[(random(id, 0x6a02) * tints.len() as f32) as usize % tints.len()];
        let mut color = tint * (0.95 + 0.08 * random(id, 0x6a03));
        let (to_left, to_right) = (within_x / 3.0, (1.0 - within_x) / 3.0);
        let (to_top, to_bottom) = (within_y / COURSES as f32, (1.0 - within_y) / COURSES as f32);
        let lit = 1.0 - smoothstep(0.004, 0.018, to_top.min(to_left));
        let shaded = 1.0 - smoothstep(0.004, 0.018, to_bottom.min(to_right));
        color *= 1.0 + 0.08 * lit - 0.12 * shaded;
        color *= 0.95 + 0.1 * tiling(p, 24, 3, 0x6a04);
        let edge = to_left.min(to_right).min(to_top).min(to_bottom);
        joint.lerp(color, smoothstep(0.002, 0.005, edge))
    })
}

/// Tile rows across a tiled slope and courses down it, near white for the roof's charcoal to
/// tint: four tile periods across (U, `assembly`'s tile coordinates, on the modeled rows' phase:
/// a pan tile's channel over the period's first 40%, a cover tile rounded over the rest, its
/// crest at 70%) and four courses up the slope (V), each course's lower lip lit and the slope
/// just under it shaded. Painted in, the rows read where the modeled ones are too small to.
pub fn tiles_png() -> io::Result<Vec<u8>> {
    paint(256, |p| {
        let tile = tile_at(p);
        let value = tile.light * tile.tint;
        Vec3::new(value * 0.98, value * 0.99, value)
    })
}

/// The tiles' mask, linear: R occlusion (deep in the channels, at the seams and under each
/// course's lip), G cavity (the seams), B specular (the crests catch it, the channels less).
pub fn tiles_mask_png() -> io::Result<Vec<u8>> {
    let image = image::RgbImage::from_fn(256, 256, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / 256.0;
        let tile = tile_at(p);
        image::Rgb([tile.occlusion, tile.cavity, tile.specular].map(|c| unorm8(c.clamp(0.0, 1.0))))
    });
    png(image)
}

/// Tile periods and courses on the tiles' map.
const TILE_MAP_PERIODS: f32 = 4.0;
const TILE_MAP_COURSES: f32 = 4.0;

/// One texel of the tiles' map, for its color and its mask.
struct TileTexel {
    light: f32,
    tint: f32,
    occlusion: f32,
    cavity: f32,
    specular: f32,
}

fn tile_at(p: Vec2) -> TileTexel {
    let along = p.x * TILE_MAP_PERIODS;
    let phase = along.fract();
    let course = (p.y * TILE_MAP_COURSES).floor();
    let within = p.y * TILE_MAP_COURSES - course;
    let id = course as u32 * 16 + along.floor() as u32;
    // A channel's pan tile, a little lighter in its middle; a cover tile rounded to its crest.
    let (mut light, mut occlusion, mut specular) = if phase < 0.4 {
        let middle = 1.0 - (phase - 0.2).abs() / 0.2;
        (0.5 + 0.06 * middle, 0.72, 0.5)
    } else {
        let round = (std::f32::consts::PI * (phase - 0.4) / 0.6).sin().max(0.0).sqrt();
        (0.66 + 0.32 * round, 0.85 + 0.15 * round, 0.6 + 0.4 * round)
    };
    // Seams where cover tiles meet the channels.
    let seam = (phase - 0.4).abs().min(phase).min(1.0 - phase);
    let open = smoothstep(0.0, 0.035, seam);
    light *= 0.7 + 0.3 * open;
    occlusion *= 0.75 + 0.25 * open;
    // Each course's lower lip (the start of V within it) lit, the slope under the next shaded.
    let lip = 1.0 - smoothstep(0.0, 0.08, within);
    let under = smoothstep(0.82, 1.0, within);
    light *= 1.0 + 0.08 * lip - 0.22 * under;
    occlusion *= 1.0 - 0.2 * under;
    specular *= 1.0 - 0.3 * under;
    let tint = 0.93 + 0.08 * random(id, 0x7c01) + 0.04 * (tiling(p, 32, 2, 0x7c02) - 0.5);
    TileTexel { light, tint, occlusion, cavity: 0.7 + 0.3 * open, specular }
}

/// Rafters under the eaves: four to the map across U, running along V, each lit across its
/// middle, with darker boards between; near white, tinted warm by its material.
pub fn rafters_png() -> io::Result<Vec<u8>> {
    const RAFTERS: f32 = 4.0;
    paint(256, |p| {
        let within = (p.x * RAFTERS).fract();
        // A rafter across the middle 55% of each step, rounded; boards between.
        let edge = (within - 0.5).abs();
        let rafter = 1.0 - smoothstep(0.24, 0.29, edge);
        let round = 1.0 - 0.18 * (edge / 0.29).min(1.0).powi(2);
        let mut value = 0.62 + (0.92 * round - 0.62) * rafter;
        let grain = fbm(Vec2::new(p.x * 64.0, p.y * 4.0), (64, 4), 3, 0x8d01);
        value *= 0.92 + 0.12 * grain;
        Vec3::new(value, value * 0.96, value * 0.92)
    })
}

/// Flagstones: five rows of slabs, alternate rows of three and four so the joints stagger, each
/// slab its own dark cool grey with a little mottling, softly worn at its edges, in thin dark joints.
pub fn flagstones_png() -> io::Result<Vec<u8>> {
    const ROWS: f32 = 5.0;
    let joint = rgb(62, 64, 68);
    let tints =
        [rgb(128, 131, 136), rgb(118, 122, 128), rgb(136, 137, 138), rgb(122, 125, 128), rgb(112, 117, 124)];
    paint(512, |p| {
        let row = (p.y * ROWS).floor();
        let across = if (row as u32).is_multiple_of(2) { 3.0 } else { 4.0 };
        let shift = random(row as u32, 0xf1a1);
        let along = (p.x + shift).rem_euclid(1.0) * across;
        let slab = along.floor();
        let (u, v) = (along - slab, p.y * ROWS - row);
        let id = row as u32 * 16 + slab as u32;
        let tint = tints[(random(id, 0xf1a2) * tints.len() as f32) as usize % tints.len()];
        let mut color = tint * (0.95 + 0.07 * random(id, 0xf1a3));
        color *= 0.95 + 0.09 * tiling(p, 20, 3, 0xf1a4);
        // Distance to the slab's edge in map units; worn a little darker toward it.
        let edge = (u.min(1.0 - u) / across).min(v.min(1.0 - v) / ROWS);
        color *= 0.92 + 0.08 * smoothstep(0.0, 0.02, edge);
        joint.lerp(color, smoothstep(0.0015, 0.004, edge))
    })
}

/// A lantern's paper between two ribs: brightest midway, dimming toward the ribs at U's ends,
/// a band of darker paper at the top and the bottom; it is both the paper's color and the
/// mask on its glow.
pub fn lantern_png() -> io::Result<Vec<u8>> {
    paint(128, |p| {
        let across = 1.0 - (2.0 * p.x - 1.0).abs();
        let rib = smoothstep(0.0, 0.08, across);
        let glow = 0.78 + 0.22 * smoothstep(0.0, 1.0, across);
        let band = 1.0 - 0.35 * (1.0 - smoothstep(0.03, 0.09, p.y.min(1.0 - p.y)));
        let fibers = 0.97 + 0.05 * tiling(p, 16, 3, 0x9e01);
        let value = (0.25 + 0.75 * rib) * glow * band * fibers;
        Vec3::new(value, value * 0.97, value * 0.92)
    })
}

/// The terminator band: white at both ends and a warm apricot across the middle of the
/// transition. Rows are alike; the shader reads one.
pub fn band_png() -> io::Result<Vec<u8>> {
    const WIDTH: u32 = 64;
    let apricot = [255.0, 192.0, 150.0];
    let image = image::RgbaImage::from_fn(WIDTH, 4, |x, _| {
        let u = (x as f32 + 0.5) / WIDTH as f32;
        let warmth = smoothstep(0.0, 1.0, 1.0 - (2.0 * u - 1.0).abs());
        let channel = |c: f32| (255.0 + (c - 255.0) * warmth).round() as u8;
        image::Rgba([channel(apricot[0]), channel(apricot[1]), channel(apricot[2]), 255])
    });
    png(image)
}

/// The look-dev character's hair mask (`character_maps::hair_mask`, M1 C5) from the model's
/// painted highlight `highlight` (a PNG, [`HAIR_HIGHLIGHT`]), on the same strand UVs: R the
/// painted band as the model's author drew it (each texel's brightest channel), G no shift, B no
/// occlusion, since the albedo already darkens the roots.
pub fn hair_mask_png(highlight: &[u8]) -> io::Result<Vec<u8>> {
    let highlight = image::load_from_memory(highlight).map_err(io::Error::other)?.to_rgb8();
    let mask = image::RgbImage::from_fn(highlight.width(), highlight.height(), |x, y| {
        let mut texel = [0; 3];
        texel[hair_mask::HIGHLIGHT] = highlight.get_pixel(x, y).0.into_iter().max().unwrap_or(0);
        texel[hair_mask::SHIFT] = unorm8(hair_mask::NO_SHIFT);
        texel[hair_mask::OCCLUSION] = u8::MAX;
        image::Rgb(texel)
    });
    png(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The look-dev directory in the repository.
    fn lookdev_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/lookdev")
    }

    /// The hair mask keeps the model's painted band in R, and neither shifts the band nor
    /// occludes; the committed mask is what the generator makes of the committed highlight.
    #[test]
    fn the_hair_mask_keeps_the_painted_band() {
        let highlight_bytes = std::fs::read(lookdev_dir().join(HAIR_HIGHLIGHT)).unwrap();
        let bytes = hair_mask_png(&highlight_bytes).unwrap();
        assert_eq!(bytes, hair_mask_png(&highlight_bytes).unwrap(), "byte for byte");
        assert_eq!(bytes, std::fs::read(lookdev_dir().join(HAIR_MASK)).unwrap(), "the committed mask");
        let highlight = image::load_from_memory(&highlight_bytes).unwrap().to_rgb8();
        let mask = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(mask.dimensions(), highlight.dimensions());
        let mut painted = 0;
        for (texel, painted_texel) in mask.pixels().zip(highlight.pixels()) {
            assert_eq!(texel.0[hair_mask::HIGHLIGHT], *painted_texel.0.iter().max().unwrap());
            assert_eq!(texel.0[hair_mask::SHIFT], 128, "0.5 within 8 bits: no shift");
            assert_eq!(texel.0[hair_mask::OCCLUSION], 255);
            painted += usize::from(texel.0[hair_mask::HIGHLIGHT] > 128);
        }
        assert!(painted > 1000, "the band is there: {painted} bright texels");
    }

    /// The character's look-dev materials name its parts, which MToon doesn't, so the character
    /// prepass's IDs tell its face, hair and eyes from its body (M1 C5); they are written as the
    /// anime character document's current version, the strands read the generated hair mask, and
    /// the eye parts draw the layers the VRM import gives VRoid's names (M1 C6), and every part
    /// takes the stylized GI's region weights the import gives its name (M1 C2's follow-up; X1
    /// part 2 set the face's and body's).
    #[test]
    fn the_character_materials_name_its_parts() {
        use sapphire_assets::material::{AnimeCharacterMaterial, AnyMaterialDocument, CharacterSurface};
        use sapphire_reflect::Versioned;

        let hair_mask = crate::meta::read(&lookdev_dir().join(format!("{HAIR_MASK}.meta"))).unwrap().id;
        let version = format!("    version: {},\n", AnimeCharacterMaterial::VERSION);
        let mut parts = Vec::new();
        for entry in std::fs::read_dir(lookdev_dir().join("character/materials")).unwrap() {
            let path = entry.unwrap().path();
            let file = path.file_name().unwrap().to_str().unwrap().to_owned();
            let Some(name) = file.strip_suffix(".material.ron") else { continue };
            let bytes = std::fs::read(&path).unwrap();
            assert!(std::str::from_utf8(&bytes).unwrap().contains(&version), "{name}: {version}");
            let AnyMaterialDocument::AnimeCharacter(material) = crate::material::read(&bytes).unwrap() else {
                panic!("{name} isn't an anime character material");
            };
            // The stylized GI's weights a fresh import gives VRoid's name (M1 C2 follow-up).
            for region in &material.regions {
                assert_eq!(region.gi, crate::import::vrm::vroid_region_gi(name), "{name}");
            }
            let part = match &material.surface {
                CharacterSurface::Body => "body",
                CharacterSurface::Face(_) => "face",
                CharacterSurface::Hair(hair) => {
                    let strands = name == "Hair_00_HAIR";
                    assert_eq!(hair.mask_map, strands.then_some(hair_mask), "{name}");
                    "hair"
                }
                CharacterSurface::Eye(eye) => {
                    assert_eq!(eye.through_hair.is_some(), name != "EyeWhite_00_EYE", "{name}");
                    // The layer the VRM import maps VRoid's name onto (M1 C6).
                    let layer = crate::import::vrm::vroid_eye_layer(name);
                    assert_eq!(Some(eye.layer), layer, "{name}");
                    "eye"
                }
            };
            parts.push((name.to_owned(), part));
        }
        parts.sort();
        let expected = [
            ("Body_00_SKIN", "body"),
            ("Bottoms_01_CLOTH", "body"),
            ("EyeHighlight_00_EYE", "eye"),
            ("EyeIris_00_EYE", "eye"),
            ("EyeWhite_00_EYE", "eye"),
            ("FaceBrow_00_FACE", "eye"),
            ("FaceEyeline_00_FACE", "eye"),
            ("FaceMouth_00_FACE", "body"),
            ("Face_00_SKIN", "face"),
            ("HairBack_00_HAIR", "hair"),
            ("Hair_00_HAIR", "hair"),
            ("Shoes_01_CLOTH", "body"),
            ("Tops_01_CLOTH", "body"),
        ];
        assert_eq!(parts, expected.map(|(name, part)| (name.to_owned(), part)));
    }

    /// The committed town assembles: every triangle of every piece claimed by a rule, every slot
    /// drawn, no two placements fighting in one plane but those its `allow_coplanar` names, and
    /// every entry there needed. Its size is the scene's budget line (spec §3.4's draw calls are
    /// one per slot): about 217k triangles in the desktop cook's full detail and 147k in the
    /// mobile cook's light one, the pavilion's 31k in both.
    #[test]
    fn the_town_assembles_in_its_own_slots() {
        let path = lookdev_dir().join("town/town.assembly.ron");
        let bytes = std::fs::read(&path).unwrap();
        let mesh = crate::assembly::import(&path, &bytes, crate::assembly::Detail::Full).unwrap();
        let light = crate::assembly::import(&path, &bytes, crate::assembly::Detail::Light).unwrap();
        let triangles = mesh.indices.len() / 3;
        let slots = mesh.lods[0].submeshes.len();
        assert_eq!(slots, mesh.materials.len(), "every slot draws");
        assert!(
            (150_000..250_000).contains(&triangles),
            "{triangles} triangles, {} vertices",
            mesh.vertices.len()
        );
        // The mobile cook's lighter shapes.
        assert_eq!(light.lods[0].submeshes.len(), slots);
        assert!(
            light.indices.len() < mesh.indices.len() * 4 / 5,
            "{} light triangles",
            light.indices.len() / 3
        );
    }

    /// The look-dev stage loads the character's rig by this ID (`sapphire_sandbox`'s
    /// `lookdev::RIG`), derived from the VRM's sidecar ID.
    #[test]
    fn the_character_rig_id_is_the_vrm_sub_asset() {
        let vrm = sapphire_assets::AssetId::from_text("6f8a81b6-29c4-485f-b161-847b001d561f");
        let rig = crate::meta::sub_asset_id(vrm, crate::import::vrm::RIG.name);
        assert_eq!(rig, sapphire_assets::AssetId::from_text("4b4073fd-44c5-8437-a712-1cccad486da3"));
    }

    #[test]
    fn maps_regenerate_byte_for_byte() {
        assert_eq!(files().unwrap(), files().unwrap());
        assert_eq!(content_files().unwrap(), content_files().unwrap());
    }

    /// The committed plants and Milky Way are what the generator makes, and each plant is its
    /// foliage sample's geometry. Both are built and painted with `sin`, `cos`, `exp`, `atan2`
    /// and `powf`, which each platform's libm rounds its own way (glibc's and MSVC's against
    /// Apple's), so the comparison allows float rounding and nothing more: an edit to a
    /// generator that isn't regenerated still fails it.
    #[test]
    fn the_committed_content_is_generated() {
        for (name, generated) in content_files().unwrap() {
            let committed = std::fs::read(lookdev_dir().join(&name)).unwrap();
            if name.ends_with(".glb") {
                assert_glb_matches(&name, &committed, &generated);
            } else {
                assert_png_matches(&name, &committed, &generated);
            }
        }
        assert_eq!(Plant::Tree.glb(), foliage::tree_glb());
    }

    /// Whether two f32s generated on different platforms agree: within a millionth, relative to
    /// their size but at least 1 (the plants' geometry is in meters, their normals and texture
    /// coordinates within 0 to 1).
    fn floats_agree(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
    }

    /// Asserts two JSON values are the same but for float rounding ([`floats_agree`]): integers,
    /// strings and the structure exactly.
    fn assert_json_matches(path: &str, committed: &serde_json::Value, generated: &serde_json::Value) {
        use serde_json::Value;
        match (committed, generated) {
            (Value::Number(a), Value::Number(b)) if a.is_f64() || b.is_f64() => {
                let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
                assert!(floats_agree(a, b), "{path}: committed {a}, generated {b}");
            }
            (Value::Array(a), Value::Array(b)) => {
                assert_eq!(a.len(), b.len(), "{path}: array length");
                for (index, (a, b)) in a.iter().zip(b).enumerate() {
                    assert_json_matches(&format!("{path}[{index}]"), a, b);
                }
            }
            (Value::Object(a), Value::Object(b)) => {
                assert!(a.keys().eq(b.keys()), "{path}: keys {:?} against {:?}", a.keys(), b.keys());
                for (key, a) in a {
                    assert_json_matches(&format!("{path}.{key}"), a, &b[key]);
                }
            }
            (a, b) => assert_eq!(a, b, "{path}"),
        }
    }

    /// Asserts two GLBs hold the same glTF but for float rounding: the JSON chunk alike but for
    /// its floats (the accessors' bounds), and the binary chunk alike byte for byte but in the
    /// elements of `F32` accessors, whose floats may differ by [`floats_agree`]. Indices and
    /// every other integer match exactly.
    fn assert_glb_matches(name: &str, committed: &[u8], generated: &[u8]) {
        let (a, b) = (gltf::Glb::from_slice(committed).unwrap(), gltf::Glb::from_slice(generated).unwrap());
        let json = |glb: &gltf::Glb<'_>| serde_json::from_slice::<serde_json::Value>(&glb.json).unwrap();
        assert_json_matches(name, &json(&a), &json(&b));
        let (a, b) = (a.bin.expect("a binary chunk"), b.bin.expect("a binary chunk"));
        assert_eq!(a.len(), b.len(), "{name}: binary chunk length");
        // Which bytes hold floats, by the generated file's accessors (the JSON matched).
        let mut float = vec![false; b.len()];
        for accessor in gltf::Gltf::from_slice(generated).unwrap().accessors() {
            if accessor.data_type() != gltf::accessor::DataType::F32 {
                continue;
            }
            let view = accessor.view().expect("the plants' accessors are dense");
            let stride = view.stride().unwrap_or(accessor.size());
            for element in 0..accessor.count() {
                let start = view.offset() + accessor.offset() + element * stride;
                float[start..start + accessor.size()].fill(true);
            }
        }
        // F32 elements are 4-byte aligned in the chunk, so its words line up with their floats.
        for (index, (a, b)) in a.chunks(4).zip(b.chunks(4)).enumerate() {
            if a == b {
                continue;
            }
            let at = index * 4;
            assert!(
                float[at..at + a.len()].iter().all(|&float| float),
                "{name}: byte {at} differs outside the floats"
            );
            let word = |bytes: &[u8]| f64::from(f32::from_le_bytes(bytes.try_into().unwrap()));
            let (a, b) = (word(a), word(b));
            assert!(floats_agree(a, b), "{name}: the float at byte {at}: committed {a}, generated {b}");
        }
    }

    /// Asserts two PNGs hold the same image to within one code value a channel: an f32 a few
    /// ulps off can round to the next 8-bit value.
    fn assert_png_matches(name: &str, committed: &[u8], generated: &[u8]) {
        let decode = |bytes: &[u8]| image::load_from_memory(bytes).unwrap().to_rgba8();
        let (a, b) = (decode(committed), decode(generated));
        assert_eq!(a.dimensions(), b.dimensions(), "{name}: size");
        let off = a.as_raw().iter().zip(b.as_raw()).map(|(&a, &b)| a.abs_diff(b)).max().unwrap_or(0);
        assert!(off <= 1, "{name}: {off} code values off");
    }

    #[test]
    fn maps_tile_without_a_seam() {
        for (name, bytes) in files().unwrap() {
            if name.ends_with("band.png") || name.ends_with("lantern.png") {
                continue;
            }
            let image = image::load_from_memory(&bytes).unwrap().to_rgb8();
            let (width, height) = image.dimensions();
            let distance = |a: &image::Rgb<u8>, b: &image::Rgb<u8>| {
                a.0.iter().zip(b.0).map(|(&a, b)| i32::from(a).abs_diff(i32::from(b))).max().unwrap()
            };
            // Across each edge the step is no bigger than the map's own steps inside it.
            let horizontal =
                |x: u32, y: u32| distance(image.get_pixel(x, y), image.get_pixel((x + 1) % width, y));
            let vertical =
                |x: u32, y: u32| distance(image.get_pixel(x, y), image.get_pixel(x, (y + 1) % height));
            let inside = (0..height)
                .flat_map(|y| (0..width - 1).map(move |x| (x, y)))
                .map(|(x, y)| horizontal(x, y))
                .max()
                .unwrap();
            let across = (0..height).map(|y| horizontal(width - 1, y)).max().unwrap();
            assert!(across <= inside, "{name}: {across} across the left and right edges, {inside} inside");
            let inside = (0..height - 1)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .map(|(x, y)| vertical(x, y))
                .max()
                .unwrap();
            // Whitewash runs once down a wall, so only its sides meet.
            if name.ends_with("whitewash.png") {
                continue;
            }
            let across = (0..width).map(|x| vertical(x, height - 1)).max().unwrap();
            assert!(across <= inside, "{name}: {across} across the top and bottom edges, {inside} inside");
        }
    }
}
