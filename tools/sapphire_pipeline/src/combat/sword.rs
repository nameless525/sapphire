//! The player's sword (M2 O3): a slim straight blade of Jinzhou's kind, written whole as
//! `player/sword.glb` with the maps its material reads.
//!
//! Its origin is the middle of the `blade` path in `player.moves.ron`, so a hand that holds the path's
//! socket holds the mesh: +Y runs from the middle of the fist to the tip, 0.45 m each way, and the edge
//! lies along Z. Flat faces carry hard normals, so a bevel catches the light as a clean band, and every
//! triangle's UVs lie in one cell of a row of [`CELLS`] swatches, as the warrior's do, so a part's cell
//! says what it is and V runs down its gradient. V follows the blade's length, which lets its gradient
//! run from hilt to tip.

use glam::Vec3;
use sapphire_assets::character_maps::{MaterialId, ilm};
use sapphire_math::det;
use serde_json::json;

use super::{gradient, png};
use crate::color::linear_to_srgb;
use crate::color::unorm8;
use crate::gltf_writer::{Geometry, GltfWriter};

/// The material slot its sidecar binds.
pub const SLOT: &str = "sword";

/// A part of the sword: a region of its material, picked per pixel by the control map's material ID,
/// in the order `sword.material.ron` and `sword.ramp.ron` list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The blade's bevels and tip: bright steel.
    Steel,
    /// The flat down the blade's middle, a dark inlay with a glowing line.
    Inlay,
    /// The guard and the pommel.
    Gold,
    /// The grip's wrapping.
    Wrap,
}

/// How many swatches across the maps are.
pub const CELLS: u32 = Part::ALL.len() as u32;

impl Part {
    /// Every part, by its material ID.
    pub const ALL: [Self; 4] = [Self::Steel, Self::Inlay, Self::Gold, Self::Wrap];

    /// The material ID its pixels carry, and its region in the material.
    pub fn id(self) -> MaterialId {
        MaterialId::new(self as u8).expect("fewer parts than IDs")
    }
}

/// The part whose cell a U lies in.
pub fn part_at(u: f32) -> Part {
    Part::ALL[((u * CELLS as f32) as usize).min(Part::ALL.len() - 1)]
}

/// Half the blade's length: from the middle of the fist to the mesh's origin, and on to the tip.
pub const HALF_LENGTH: f32 = 0.45;

// Heights along the sword, from the mesh's origin. The fist's middle is at -HALF_LENGTH.
const POMMEL_END: f32 = -0.565;
const POMMEL_NECK: f32 = -0.525;
const GUARD_LOW: f32 = -0.392;
const GUARD_HIGH: f32 = -0.368;
const BLADE_ROOT: f32 = -0.364;
const INLAY_END: f32 = 0.27;
const TIP_SHOULDER: f32 = 0.36;
const TIP: f32 = HALF_LENGTH;

/// The blade's half width along Z at a height, tapering to the shoulder of its tip.
fn half_width(y: f32) -> f32 {
    if y >= TIP_SHOULDER {
        return 0.021 * (TIP - y) / (TIP - TIP_SHOULDER);
    }
    let along = (y - BLADE_ROOT) / (TIP_SHOULDER - BLADE_ROOT);
    0.028 + (0.021 - 0.028) * along
}

/// How thick the blade is either side of its flat, along X.
const HALF_THICKNESS: f32 = 0.0055;
/// How much of the half width the middle flat takes.
const FLAT_SHARE: f32 = 0.42;

#[derive(Default)]
struct Builder {
    geometry: Geometry,
}

impl Builder {
    /// A flat polygon of hard normals, wound counter-clockwise from outside, its V by each corner's height.
    fn face(&mut self, part: Part, corners: &[Vec3]) {
        assert!(corners.len() >= 3);
        let normal = (corners[1] - corners[0]).cross(corners[corners.len() - 1] - corners[0]).normalize();
        self.fan(part, corners, &vec![normal; corners.len()]);
    }

    /// A polygon with a normal at each corner.
    fn fan(&mut self, part: Part, corners: &[Vec3], normals: &[Vec3]) {
        let first = self.geometry.positions.len() as u32;
        for (corner, normal) in corners.iter().zip(normals) {
            self.geometry.positions.push(corner.to_array());
            self.geometry.normals.push(normal.to_array());
            self.geometry.uvs.push(uv(part, corner.y));
        }
        for index in 1..corners.len() as u32 - 1 {
            self.geometry.indices.extend([first, first + index, first + index + 1]);
        }
    }

    /// A ring of smooth quads from `low` to `high`, each ring `(height, half width along X, along Z)`.
    fn band(&mut self, part: Part, a: (f32, f32, f32), b: (f32, f32, f32), sides: u32) {
        let (low, high) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        let point = |ring: (f32, f32, f32), side: u32| {
            let angle = std::f32::consts::TAU * side as f32 / sides as f32;
            let (sin, cos) = det::sin_cos(angle);
            Vec3::new(ring.1 * cos, ring.0, ring.2 * sin)
        };
        let normal = |ring: (f32, f32, f32), side: u32| {
            let angle = std::f32::consts::TAU * side as f32 / sides as f32;
            // Sideways by the ellipse, tilted by how the band narrows.
            let slope = (low.1 - high.1) / (high.0 - low.0).abs().max(1e-4);
            let (sin, cos) = det::sin_cos(angle);
            Vec3::new(cos / ring.1.max(1e-3), slope / ring.1.max(1e-3), sin / ring.2.max(1e-3)).normalize()
        };
        for side in 0..sides {
            // Clockwise round +Y seen from above, which faces outward.
            let (a, b) = (side, side + 1);
            let corners = [point(low, a), point(high, a), point(high, b), point(low, b)];
            let normals = [normal(low, a), normal(high, a), normal(high, b), normal(low, b)];
            self.fan(part, &corners, &normals);
        }
    }

    /// A flat cap on a ring, facing down the sword: closes the pommel's end.
    fn cap(&mut self, part: Part, ring: (f32, f32, f32), sides: u32) {
        let point = |side: u32| {
            let angle = std::f32::consts::TAU * side as f32 / sides as f32;
            let (sin, cos) = det::sin_cos(angle);
            Vec3::new(ring.1 * cos, ring.0, ring.2 * sin)
        };
        let corners: Vec<Vec3> = (0..sides).map(point).collect();
        self.face(part, &corners);
    }
}

/// A corner's UV: the middle of its part's cell across, its height down the cell.
fn uv(part: Part, y: f32) -> [f32; 2] {
    let v = ((y - POMMEL_END) / (TIP - POMMEL_END)).clamp(0.0, 1.0);
    [(part as u32 as f32 + 0.5) / CELLS as f32, 1.0 - v]
}

/// The blade: a flat down each side between two bevels, the flat's inlay stopping short of the tip.
fn blade(builder: &mut Builder) {
    let heights = [BLADE_ROOT, INLAY_END, TIP_SHOULDER];
    for pair in heights.windows(2) {
        let (low, high) = (pair[0], pair[1]);
        let flat = if high <= INLAY_END { Part::Inlay } else { Part::Steel };
        for sign in [1.0, -1.0] {
            let section = |y: f32| {
                let width = half_width(y);
                let side = |z: f32, x: f32| Vec3::new(sign * x, y, sign * z);
                (
                    side(width, 0.0),
                    side(width * FLAT_SHARE, HALF_THICKNESS),
                    side(-width * FLAT_SHARE, HALF_THICKNESS),
                    side(-width, 0.0),
                )
            };
            let (l_edge, l_flat, l_back, l_far) = section(low);
            let (h_edge, h_flat, h_back, h_far) = section(high);
            builder.face(Part::Steel, &[l_edge, l_flat, h_flat, h_edge]);
            builder.face(flat, &[l_flat, l_back, h_back, h_flat]);
            builder.face(Part::Steel, &[l_back, l_far, h_far, h_back]);
        }
    }
    // The point: each side's three faces closing on the tip.
    let tip = Vec3::new(0.0, TIP, 0.0);
    for sign in [1.0, -1.0] {
        let width = half_width(TIP_SHOULDER);
        let side = |z: f32, x: f32| Vec3::new(sign * x, TIP_SHOULDER, sign * z);
        let (edge, flat, back, far) = (
            side(width, 0.0),
            side(width * FLAT_SHARE, HALF_THICKNESS),
            side(-width * FLAT_SHARE, HALF_THICKNESS),
            side(-width, 0.0),
        );
        builder.face(Part::Steel, &[edge, flat, tip]);
        builder.face(Part::Steel, &[flat, back, tip]);
        builder.face(Part::Steel, &[back, far, tip]);
    }
    // The root's end, where a blade meets the guard.
    let width = half_width(BLADE_ROOT);
    let root = |z: f32, x: f32| Vec3::new(x, BLADE_ROOT, z);
    builder.face(
        Part::Steel,
        &[
            root(width, 0.0),
            root(width * FLAT_SHARE, -HALF_THICKNESS),
            root(-width * FLAT_SHARE, -HALF_THICKNESS),
            root(-width, 0.0),
            root(-width * FLAT_SHARE, HALF_THICKNESS),
            root(width * FLAT_SHARE, HALF_THICKNESS),
        ],
    );
}

/// The guard's outline across the blade, a lens swept up a little at its tips: along Z, half its
/// thickness along X, and how far its top rises over [`GUARD_HIGH`].
const GUARD_OUTLINE: [(f32, f32, f32); 5] = [
    (0.0, 0.0165, 0.0),
    (0.024, 0.0145, 0.0),
    (0.044, 0.0095, 0.004),
    (0.056, 0.0045, 0.012),
    (0.06, 0.0015, 0.02),
];

/// A guard of faceted gold: a lens across the blade, its ends swept toward the tip.
fn guard(builder: &mut Builder) {
    // Round the lens: out to its +Z tip on the +X side, back on the -X side, and the same toward -Z.
    let mut outline: Vec<(f32, f32, f32)> = Vec::new();
    for &(z, half, rise) in GUARD_OUTLINE.iter() {
        outline.push((z, half, rise));
    }
    for &(z, half, rise) in GUARD_OUTLINE.iter().rev().skip(1) {
        outline.push((z, -half, rise));
    }
    for &(z, half, rise) in GUARD_OUTLINE.iter().skip(1) {
        outline.push((-z, -half, rise));
    }
    for &(z, half, rise) in GUARD_OUTLINE.iter().rev().skip(1).take(GUARD_OUTLINE.len() - 2) {
        outline.push((-z, half, rise));
    }
    let top = |p: (f32, f32, f32)| Vec3::new(p.1, GUARD_HIGH + p.2, p.0);
    let bottom = |p: (f32, f32, f32)| Vec3::new(p.1 * 0.8, GUARD_LOW + p.2 * 0.8, p.0 * 0.92);
    let count = outline.len();
    for index in 0..count {
        let (a, b) = (outline[index], outline[(index + 1) % count]);
        builder.face(Part::Gold, &[bottom(a), top(a), top(b), bottom(b)]);
        // The top and the underside, as a fan of facets, since the top's tips rise.
        builder.face(Part::Gold, &[Vec3::new(0.0, GUARD_HIGH, 0.0), top(b), top(a)]);
        builder.face(Part::Gold, &[Vec3::new(0.0, GUARD_LOW, 0.0), bottom(a), bottom(b)]);
    }
}

/// The grip, slightly oval and waisted, and a gold ring and pommel below it.
fn hilt(builder: &mut Builder) {
    const SIDES: u32 = 8;
    let grip = [
        (GUARD_LOW, 0.0115, 0.0145),
        (-0.43, 0.0125, 0.0155),
        (-0.48, 0.0125, 0.0155),
        (POMMEL_NECK + 0.008, 0.0115, 0.0145),
    ];
    for pair in grip.windows(2) {
        builder.band(Part::Wrap, pair[0], pair[1], SIDES);
    }
    let pommel = [
        (POMMEL_NECK + 0.008, 0.0115, 0.0145),
        (POMMEL_NECK + 0.002, 0.0145, 0.0185),
        (POMMEL_NECK - 0.012, 0.0165, 0.0215),
        (POMMEL_NECK - 0.03, 0.0105, 0.0135),
        (POMMEL_END, 0.0035, 0.0045),
    ];
    for pair in pommel.windows(2) {
        builder.band(Part::Gold, pair[0], pair[1], SIDES);
    }
    builder.cap(Part::Gold, pommel[4], SIDES);
}

/// The mesh, as a binary glTF with one node and one material slot, [`SLOT`].
pub fn sword_glb() -> Vec<u8> {
    let mut builder = Builder::default();
    blade(&mut builder);
    guard(&mut builder);
    hilt(&mut builder);
    let mut writer = GltfWriter::default();
    let material = writer.add_material(json!({ "name": SLOT }));
    let mesh = writer.add_mesh("sword", &[(&builder.geometry, Some(material))]);
    let node = writer.add_node(json!({ "name": "sword", "mesh": mesh }));
    writer.add_root(node);
    writer.to_glb()
}

/// The maps' size, each cell a swatch and V a gradient.
const WIDTH: u32 = 32;
const HEIGHT: u32 = 256;

/// The cell and how far along the sword a texel is: 0 at the tip's row, 1 at the pommel's.
fn texel(x: u32, y: u32) -> (Part, f32) {
    (part_at((x as f32 + 0.5) / WIDTH as f32), (y as f32 + 0.5) / HEIGHT as f32)
}

/// How far down its V a height falls.
fn v_of(y: f32) -> f32 {
    uv(Part::Steel, y)[1]
}

/// What a part looks like at `v`. Steel pales toward the tip, the inlay is deep blue, gold warms
/// toward its ends, and the wrap is a dark wine.
fn albedo(part: Part, v: f32) -> Vec3 {
    match part {
        Part::Steel => gradient(&[(0.0, 0xf2f6ff), (0.5, 0xd4dcec), (1.0, 0xb4bed4)], v),
        Part::Inlay => gradient(&[(0.0, 0x1c2c4c), (1.0, 0x24345a)], v),
        Part::Gold => gradient(&[(0.0, 0xe8c070), (0.5, 0xd8a850), (1.0, 0xb88434)], v),
        Part::Wrap => gradient(&[(0.0, 0x3e1a2c), (1.0, 0x2c1222)], v),
    }
}

/// The albedo.
pub fn palette_png() -> std::io::Result<Vec<u8>> {
    let image = image::RgbImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let (part, v) = texel(x, y);
        image::Rgb(albedo(part, v).to_array().map(|c| unorm8(linear_to_srgb(c))))
    });
    png(image)
}

/// A part's highlight (R) and its size (B).
fn highlight(part: Part) -> (f32, f32) {
    match part {
        Part::Steel => (1.0, 0.85),
        Part::Inlay => (0.4, 0.6),
        Part::Gold => (1.0, 0.7),
        Part::Wrap => (0.3, 0.4),
    }
}

/// What `sword.ilm.ron` packs into the control map: R highlight, G shadow bias, B highlight size, and
/// A the part's material ID, as gray levels the document snaps to. Steel lights a little sooner, so a
/// blade in shade still reads as bright against the character's dark shorts.
pub fn control_png() -> std::io::Result<Vec<u8>> {
    let image = image::RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let (part, _) = texel(x, y);
        let (specular, threshold) = highlight(part);
        let bias = if part == Part::Steel { 0.6 } else { 0.5 };
        image::Rgba([specular, bias, threshold, ilm::encode_material_id(part.id())].map(unorm8))
    });
    png(image)
}

/// The body's layer mask: R the matcap on the metals, G nothing (nothing reflects), B the inlay's glow,
/// and A nothing (no stockings).
pub fn layers_png() -> std::io::Result<Vec<u8>> {
    let image = image::RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let (part, v) = texel(x, y);
        let matcap = match part {
            Part::Steel => 1.0,
            Part::Gold => 0.9,
            Part::Inlay => 0.25,
            Part::Wrap => 0.0,
        };
        // Brightest near the tip of the inlay, fading to nothing at the guard.
        let emission = if part == Part::Inlay {
            let (near_tip, near_guard) = (v_of(INLAY_END), v_of(BLADE_ROOT));
            (1.0 - (v - near_tip) / (near_guard - near_tip)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        image::Rgba([matcap, 0.0, emission, 0.0].map(unorm8))
    });
    png(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::gltf;

    /// The committed mesh, read through the importer a cook uses.
    fn imported() -> crate::mesh::ImportedMesh {
        let content = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
        gltf::import(&content.join("combat/player/sword.glb")).expect("it imports")
    }

    /// It stands where the `blade` path says a blade does, and every triangle reads one part.
    #[test]
    fn the_sword_runs_grip_to_tip_along_y_about_the_path() {
        let mesh = imported();
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for vertex in &mesh.vertices {
            low = low.min(vertex.position[1]);
            high = high.max(vertex.position[1]);
        }
        assert!((high - HALF_LENGTH).abs() < 1e-4, "the tip at {high}");
        assert!(low < -HALF_LENGTH, "the hilt runs past the fist: {low}");
        let names: Vec<&str> = mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
        assert_eq!(names, [SLOT]);
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let parts = triangle.map(|index| part_at(mesh.vertices[index as usize].uv[0]));
            assert!(parts.iter().all(|&part| part == parts[0]), "a triangle across {parts:?}");
        }
        assert!(mesh.indices.len() / 3 < 300, "{} triangles", mesh.indices.len() / 3);
    }

    /// Every face looks outward, as a culled pass and an outline's hull need: away from the middle of
    /// the piece it belongs to.
    #[test]
    fn every_face_looks_outward() {
        let mesh = imported();
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|index| Vec3::from(mesh.vertices[index as usize].position));
            let area = (b - a).cross(c - a);
            if area.length() < 1e-9 {
                continue;
            }
            let at = (a + b + c) / 3.0;
            let piece = if at.y >= BLADE_ROOT - 1e-4 && at.z.abs() < half_width(BLADE_ROOT) + 1e-4 {
                0.04
            } else if at.y >= GUARD_LOW - 1e-4 {
                (GUARD_LOW + GUARD_HIGH) * 0.5
            } else if at.y >= POMMEL_NECK + 0.008 {
                (GUARD_LOW + POMMEL_NECK) * 0.5
            } else {
                (POMMEL_NECK + POMMEL_END) * 0.5
            };
            assert!(area.dot(at - Vec3::new(0.0, piece, 0.0)) > 0.0, "a face at {at} looks inward");
        }
    }

    /// The control map's channels say each part's ID where a shader reads it.
    #[test]
    fn the_control_map_holds_each_cell_s_part() {
        let image = image::load_from_memory(&control_png().unwrap()).unwrap().to_rgba8();
        for part in Part::ALL {
            let u = (part as u32 as f32 + 0.5) / CELLS as f32;
            let texel = image.get_pixel((u * WIDTH as f32) as u32, HEIGHT / 2).0;
            let id = ilm::decode_material_id(f32::from(texel[3]) / 255.0);
            assert_eq!(id, part.id(), "{part:?}");
        }
    }
}
