//! Ground cover's geometry: a fern's fronds, a flower's stalks and cards, a grass clump's blades and a rock's
//! faces, each from its form, its size and its variant's seed. See the [cover module's docs](super).
//!
//! As the trees' are, every number is the samples' `Rng`, the four operations, `sqrt` and
//! `sapphire_math::det`, in a fixed order, so a model is the same on every machine.

use std::f32::consts::TAU;

use glam::Vec3;
use sapphire_math::det;

use super::super::mesh::{append, card, frame, quad, tube, unit};
use super::paint::{FLOWER_CELLS, FROND_CELLS, LEAF_CELL, swatch};
use super::{Fern, Flower, Grass, Rock};
use crate::color::unorm8;
use crate::gltf_writer::Geometry;
use crate::samples::noise::{Rng, hash, smoothstep};
use crate::samples::shapes;

/// The golden turn: each frond, stalk or blade a golden share of a turn round from the last, so none lines up.
const GOLDEN: f32 = 0.618_034;

/// A flower's stalk's radius, as a share of its size.
const STALK: f32 = 0.012;

/// A grass clump's blades take one of these flat tones (vertex occlusion), the darker toward its middle:
/// its painted value steps.
const BLADE_TONES: [f32; 3] = [0.74, 0.87, 1.0];

/// A rock's sphere, before its planes cut it: segments round and rings from pole to pole.
const ROCK_SEGMENTS: u32 = 24;
const ROCK_RINGS: u32 = 16;

/// A rock's faces turned down further than this (their normal's upward part below it) take the shade.
const FACING_DOWN: f32 = -0.25;

/// A vertex's occlusion, `[r, 255, 255, 255]`.
fn occlusion(value: f32) -> [u8; 4] {
    [unorm8(value), 255, 255, 255]
}

/// A unit vector in the ground's plane, `turns` of a turn round from +X.
fn level(turns: f32) -> Vec3 {
    let (sin, cos) = det::sin_cos(turns * TAU);
    Vec3::new(cos, 0.0, sin)
}

/// A fern `size` long at its longest frond: its fronds, each a strip of `segments` quads along its curve,
/// showing one of the atlas's fronds, its root at the fern's heart and its tip at the frond's.
pub fn fern(fern: &Fern, size: f32, seed: u32) -> Geometry {
    let mut rng = Rng(seed ^ 0xf3_2a);
    let (columns, rows) = FROND_CELLS;
    let mut geometry = Geometry::default();
    for frond in 0..fern.fronds {
        // Every frond draws the same numbers.
        let turns = frond as f32 * GOLDEN + rng.range(-0.04, 0.04);
        let rise = rng.range(fern.rise.0, fern.rise.1).to_radians();
        let length = size * rng.range(fern.length.0, fern.length.1);
        let roll = rng.range(-0.35, 0.35);
        let cell = (rng.next() * (columns * rows) as f32) as u32 % (columns * rows);
        let out = level(turns);
        let (sin, cos) = det::sin_cos(rise);
        let direction = out * cos + Vec3::Y * sin;
        // Level and square to the frond, then rolled about it.
        let flat = out.cross(Vec3::Y);
        let half = 0.5 * fern.width * length;
        let (u0, u1) = (cell as f32 / columns as f32, (cell + 1) as f32 / columns as f32);
        let mut strip = Geometry::default();
        for ring in 0..=fern.segments {
            let t = ring as f32 / fern.segments as f32;
            let at = out * (0.04 * size) + direction * (length * t) - Vec3::Y * (fern.arch * length * t * t);
            let tangent = (direction - Vec3::Y * (2.0 * fern.arch * t)).normalize();
            let up = flat.cross(tangent).normalize();
            let (sin, cos) = det::sin_cos(roll);
            let across = (flat * cos + up * sin).normalize();
            let normal = across.cross(tangent).normalize();
            for (side, u) in [(-1.0, u0), (1.0, u1)] {
                strip.positions.push((at + across * (side * half)).to_array());
                strip.normals.push(normal.to_array());
                // The cell's bottom at the heart, its top at the tip.
                strip.uvs.push([u, 1.0 - t]);
                strip.colors.push(occlusion(0.7 + 0.3 * t));
            }
        }
        for ring in 0..fern.segments {
            let (left, right) = (2 * ring, 2 * ring + 1);
            // Counter-clockwise seen from the normal: across, then along.
            strip.indices.extend([left, right, right + 2, left, right + 2, left + 2]);
        }
        append(&mut geometry, strip);
    }
    geometry
}

/// Flowers `size` tall at the tallest: their stalks, thin tubes curving out from their foot, and their cards,
/// a head on each stalk facing up and out and the leaves standing round the foot, leaning out.
pub fn flower(flower: &Flower, size: f32, seed: u32) -> (Geometry, Geometry) {
    let mut rng = Rng(seed ^ 0xf1_0e);
    let (columns, rows) = FLOWER_CELLS;
    let (mut stalks, mut cards) = (Geometry::default(), Geometry::default());
    for stem in 0..flower.stems {
        let turns = stem as f32 * GOLDEN + rng.range(-0.05, 0.05);
        let reach = if stem == 0 { 0.0 } else { flower.spread * size * rng.next().sqrt() };
        let foot = level(turns) * reach;
        let height = size * rng.range(flower.shortest, 1.0);
        let lean = level(turns + rng.range(-0.2, 0.2)) * (flower.lean * height * rng.range(0.5, 1.0));
        let points: Vec<Vec3> = (0..=4)
            .map(|step| {
                let t = step as f32 / 4.0;
                foot + lean * (t * t) + Vec3::Y * (height * t)
            })
            .collect();
        let radius = STALK * size;
        let radii: Vec<f32> = (0..=4).map(|step| radius * (1.0 - 0.3 * step as f32 / 4.0)).collect();
        append(&mut stalks, tube(&points, &radii, 3, size));
        let top = points[4];
        let along = (lean * 2.0 + Vec3::Y * height).normalize();
        let facing = (along + unit(&mut rng) * 0.25).normalize();
        let (u, v) = frame(facing, rng.range(0.0, TAU));
        let head = flower.head * size * rng.range(0.85, 1.15);
        let cell = (rng.next() * LEAF_CELL as f32) as u32 % LEAF_CELL;
        append(
            &mut cards,
            card(quad(top + facing * (0.2 * radius), u * head, v * head), cell, columns, rows),
        );
    }
    for leaf in 0..flower.leaves {
        let out = level(leaf as f32 * GOLDEN + rng.range(-0.05, 0.05));
        let tall = 0.4 * size * rng.range(0.8, 1.2);
        let (sin, cos) = det::sin_cos(rng.range(0.25, 0.5));
        let up = Vec3::Y * cos + out * sin;
        let across = out.cross(Vec3::Y);
        let center = out * (0.03 * size) + up * (0.5 * tall);
        append(
            &mut cards,
            card(quad(center, across * (0.5 * tall), up * (0.5 * tall)), LEAF_CELL, columns, rows),
        );
    }
    (stalks, cards)
}

/// A grass clump `size` tall at its tallest blade: blades in a disc, each a strip tapering from its root on the
/// ground to a pointed tip, arching out, both faces wound outward (opaque foliage culls back faces), each
/// in one of [`BLADE_TONES`].
pub fn grass(grass: &Grass, size: f32, seed: u32) -> Geometry {
    let mut rng = Rng(seed ^ 0x9a_55);
    let mut geometry = Geometry::default();
    let disc = grass.radius * size;
    for blade in 0..grass.blades {
        let turns = blade as f32 * GOLDEN + rng.range(-0.05, 0.05);
        let out = level(turns);
        let spread = rng.next().sqrt();
        let root = out * (disc * spread);
        // Leaning away from the clump's middle, give or take.
        let lean_to = (out + level(rng.next()) * 0.6).normalize_or_zero();
        let lean_to = if lean_to == Vec3::ZERO { out } else { lean_to };
        let height = size * rng.range(grass.shortest, 1.0);
        let lean = lean_to * (grass.lean * height * rng.range(0.5, 1.0));
        let width = grass.width * size * rng.range(0.8, 1.2);
        let step = ((spread * 2.0 + rng.next()) as usize).min(BLADE_TONES.len() - 1);
        let tone = occlusion(BLADE_TONES[step]);
        let across = lean_to.cross(Vec3::Y);
        let base = geometry.positions.len() as u32;
        let segments = grass.segments;
        for ring in 0..segments {
            let t = ring as f32 / segments as f32;
            let at = root + lean * (t * t) + Vec3::Y * (height * t);
            let half = 0.5 * width * (1.0 - t);
            for side in [-1.0, 1.0] {
                geometry.positions.push((at + across * (side * half)).to_array());
                geometry.uvs.push([0.5 + 0.5 * side, 1.0 - t]);
            }
        }
        geometry.positions.push((root + lean + Vec3::Y * height).to_array());
        geometry.uvs.push([0.5, 0.0]);
        let count = 2 * segments + 1;
        geometry.normals.extend(std::iter::repeat_n([0.0, 1.0, 0.0], count as usize));
        geometry.colors.extend(std::iter::repeat_n(tone, count as usize));
        let tip = base + 2 * segments;
        for ring in 0..segments {
            let (left, right) = (base + 2 * ring, base + 2 * ring + 1);
            let faces: &[[u32; 3]] = if ring + 1 == segments {
                &[[left, right, tip]]
            } else {
                &[[left, right, right + 2], [left, right + 2, left + 2]]
            };
            for &[a, b, c] in faces {
                geometry.indices.extend([a, b, c, a, c, b]);
            }
        }
    }
    geometry
}

/// A rock `size` wide: a sphere's every point pushed out onto the nearest of `planes` cutting planes in its
/// direction (each cut at its own depth, by `rough`), so the planes' faces come out flat and broad and the
/// triangles where two meet a narrow bevel; scaled to its height, sunk by `sink`, the triangles wholly under
/// the ground dropped. Every triangle has its own corners and its face's normal; it samples one swatch: its
/// plane's tone (the shade for a face turned down), the light one for a bevel, the moss row for a face turned
/// up by `mossy`; darker at the foot.
pub fn rock(rock: &Rock, size: f32, seed: u32) -> Geometry {
    let mut rng = Rng(seed ^ 0x0c_4a);
    // The planes, spread over the sphere on a golden spiral and jittered.
    let planes: Vec<(Vec3, f32)> = (0..rock.planes)
        .map(|index| {
            let y = 1.0 - 2.0 * (index as f32 + 0.5) / rock.planes as f32;
            let around = level(index as f32 * GOLDEN) * (1.0 - y * y).max(0.0).sqrt();
            let normal = (around + Vec3::Y * y + unit(&mut rng) * 0.3).normalize();
            (normal, 1.0 - rock.rough * rng.next())
        })
        .collect();
    let stretch = Vec3::new(0.5 * size, 0.5 * rock.height * size, 0.5 * size * rng.range(0.7, 1.0));
    let lift = 0.5 * rock.height * size * (1.0 - 2.0 * rock.sink);
    let above = rock.height * size * (1.0 - rock.sink);
    let sphere = shapes::sphere(1.0, ROCK_SEGMENTS, ROCK_RINGS);
    // Each corner of the sphere on its nearest plane, and which plane that is.
    let cut: Vec<(Vec3, usize)> = sphere
        .positions
        .iter()
        .map(|&p| {
            let direction = Vec3::from(p).normalize();
            let mut best = (1.0f32, usize::MAX);
            for (index, &(normal, depth)) in planes.iter().enumerate() {
                let facing = direction.dot(normal);
                if facing > 1e-4 && (best.1 == usize::MAX || depth / facing < best.0) {
                    best = (depth / facing, index);
                }
            }
            let at = direction * best.0 * stretch + Vec3::Y * lift;
            (at, best.1)
        })
        .collect();
    let mut geometry = Geometry::default();
    for triangle in sphere.indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|index| cut[index as usize]);
        let corners = [a.0, b.0, c.0];
        if corners.iter().all(|corner| corner.y < -0.02 * size) {
            continue;
        }
        let normal = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
        if normal.length_squared() < 1e-12 {
            continue;
        }
        let normal = normal.normalize();
        let flat = a.1 == b.1 && b.1 == c.1;
        let column = if normal.y < FACING_DOWN {
            0
        } else if flat {
            1 + hash(seed ^ a.1 as u32) % 2
        } else {
            3
        };
        let row = u32::from(normal.y > 1.0 - rock.mossy);
        let uv = swatch(column, row);
        let first = geometry.positions.len() as u32;
        for corner in corners {
            geometry.positions.push(corner.to_array());
            geometry.normals.push(normal.to_array());
            geometry.uvs.push(uv);
            geometry.colors.push(occlusion(0.62 + 0.38 * smoothstep(0.0, 0.35 * above, corner.y)));
        }
        geometry.indices.extend([first, first + 1, first + 2]);
    }
    geometry
}
