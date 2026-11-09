//! The combat slice's generated sources (M2 O4), written under `content/combat/` by
//! `sapphire_pipeline gen-combat`.
//!
//! The enemy is KayKit's skeleton warrior, byte for byte under `content/third_party/`, which
//! [`warrior`] re-proportions into `enemy/warrior.glb`. Its model shades from a palette: every face's
//! UVs lie in one cell of a grid of [`COLUMNS`] × [`ROWS`] colour swatches, and V within a cell runs
//! down a painted gradient, so a part's cell says what it is and its V where it is on the part. That
//! makes the palette's layout the one place maps can paint on, so the enemy's maps are painted here,
//! in code, on that layout. No texel of the pack's own palette is read:
//!
//! | File (in `enemy/`) | What |
//! |---|---|
//! | `warrior.glb` | The model, re-proportioned at the bind pose ([`warrior::warrior_glb`]), from the pack's |
//! | `warrior_palette.png` | The albedo ([`palette_png`]): the anime palette in place of the pack's flat one, each part's cell a gradient of its own |
//! | `warrior_control.png` | What `warrior.ilm.ron` packs ([`control_png`]): each part's material ID, shadow bias and highlight |
//! | `warrior_layers.png` | The body's layer mask ([`layers_png`]): where the matcap shows and where the cloak's hem burns |
//! | `warrior_matcap.png` | Painted gunmetal ([`matcap_png`]), a spheremap for the armour and the blade |
//!
//! The player's sword is all this generator's own ([`sword`]): `player/sword.glb` and the maps
//! `sword_palette.png`, `sword_control.png` and `sword_layers.png`.
//!
//! Colours are picked in 8-bit sRGB, as an artist would.

pub mod sword;
pub mod warrior;

use std::io;
use std::path::Path;

use glam::{Vec2, Vec3};
use sapphire_assets::character_maps::{MaterialId, ilm};

use crate::color::exact::{linear_to_srgb, srgb_to_linear};
use crate::color::unorm8;
use crate::generators::Output;
use crate::samples::noise::smoothstep;
use crate::samples::{Existing, Generated, write_files};

/// The enemy's directory, under the combat directory.
pub const ENEMY_DIR: &str = "enemy";

/// The player's directory, under the combat directory.
pub const PLAYER_DIR: &str = "player";

/// The pack's warrior, relative to the combat directory: [`warrior::warrior_glb`]'s input.
pub const WARRIOR_SOURCE: &str = "../third_party/kaykit_skeletons/characters/Skeleton_Warrior.glb";

/// The enemy's model, relative to the combat directory.
pub const WARRIOR: &str = "enemy/warrior.glb";

/// The palette's columns, in KayKit's layout.
pub const COLUMNS: u32 = 8;
/// The palette's rows, in KayKit's layout.
pub const ROWS: u32 = 4;

/// A part of the warrior: a region of its material, picked per pixel by the control map's material
/// ID, whose ramp row, outline and rim the material sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Skull, jaw, ribs and limbs.
    Bone,
    /// Breastplate, pauldrons, bracers, greaves and the helmet's shell.
    Armour,
    /// The blade's edge.
    Steel,
    /// The cloak, and the helmet's crest in the same cloth.
    Cloak,
    /// Straps, belts, and the blade's grip and guard wrappings.
    Leather,
    /// Dark inlays: the helmet's visor bars and the belt's buckle.
    Trim,
}

impl Part {
    /// Every part, by its material ID.
    pub const ALL: [Self; 6] =
        [Self::Bone, Self::Armour, Self::Steel, Self::Cloak, Self::Leather, Self::Trim];

    /// The material ID its pixels carry, and its region in the material.
    pub fn id(self) -> MaterialId {
        MaterialId::new(self as u8).expect("fewer parts than IDs")
    }
}

/// The cells the warrior and its blade use, as `(column, row, part)`, in the pack's layout. A test
/// holds every triangle of both models to one of these.
pub const CELLS: [(u32, u32, Part); 10] = [
    (1, 1, Part::Bone),
    (3, 0, Part::Armour),
    (3, 1, Part::Steel),
    (2, 2, Part::Cloak),
    (6, 0, Part::Leather),
    (6, 2, Part::Leather),
    (7, 0, Part::Leather),
    (4, 0, Part::Trim),
    (2, 0, Part::Trim),
    // The eyes' cell: their own material reads no map, but their UVs lie here.
    (7, 3, Part::Trim),
];

/// The part a palette cell belongs to.
pub fn part_of(column: u32, row: u32) -> Option<Part> {
    CELLS.iter().find(|cell| (cell.0, cell.1) == (column, row)).map(|cell| cell.2)
}

/// The cell a UV lies in and how far down it, 0 to 1.
pub fn cell_of(uv: Vec2) -> (u32, u32, f32) {
    let (x, y) = (uv.x * COLUMNS as f32, uv.y * ROWS as f32);
    let (column, row) = (x.floor(), y.floor());
    (column as u32, row as u32, y - row)
}

/// Every generated map, relative to the combat directory, and how to make each.
pub fn files() -> Vec<Output> {
    let path = |name: &str| format!("{ENEMY_DIR}/{name}");
    vec![
        Output::new(path("warrior_palette.png"), palette_png),
        Output::new(path("warrior_control.png"), control_png),
        Output::new(path("warrior_layers.png"), layers_png),
        Output::new(path("warrior_matcap.png"), matcap_png),
        Output::plain(format!("{PLAYER_DIR}/sword.glb"), sword::sword_glb),
        Output::new(format!("{PLAYER_DIR}/sword_palette.png"), sword::palette_png),
        Output::new(format!("{PLAYER_DIR}/sword_control.png"), sword::control_png),
        Output::new(format!("{PLAYER_DIR}/sword_layers.png"), sword::layers_png),
    ]
}

/// Everything generated in the combat directory `dir`: [`files`], and the model, from [`WARRIOR_SOURCE`]
/// under `dir` when it's made.
pub fn outputs(dir: &Path) -> Vec<Output> {
    let source = dir.join(WARRIOR_SOURCE);
    let mut generated = files();
    generated.push(Output::new(WARRIOR, move || warrior::warrior_glb(&std::fs::read(source)?)));
    generated
}

/// Writes every generated file into the combat directory `dir` (creating the directories they sit
/// in). Only what's missing is written, as [`write_files`] says, unless `existing` replaces what differs.
/// Returns what it did with each file.
pub fn write_all(dir: &Path, existing: Existing) -> io::Result<Generated> {
    write_files(dir, outputs(dir), existing)
}

/// Linear RGB from 8-bit sRGB.
fn rgb(hex: u32) -> Vec3 {
    let channel = |shift: u32| srgb_to_linear(((hex >> shift) & 0xff) as f32 / 255.0);
    Vec3::new(channel(16), channel(8), channel(0))
}

/// A vertical gradient: `(at, colour)` stops, ascending, blended in linear light and held past the
/// ends.
fn gradient(stops: &[(f32, u32)], at: f32) -> Vec3 {
    let (mut before, mut after) = (stops[0], stops[stops.len() - 1]);
    for pair in stops.windows(2) {
        if (pair[0].0..=pair[1].0).contains(&at) {
            (before, after) = (pair[0], pair[1]);
        }
    }
    let span = (after.0 - before.0).max(1e-5);
    rgb(before.1).lerp(rgb(after.1), ((at - before.0) / span).clamp(0.0, 1.0))
}

/// What a part looks like at `v`, 0 to 1 down its cell. Bone runs from a pale ivory to a warm grey
/// and drops into near-black at the bottom of the cell, where the pack keeps its sockets and the
/// gaps between ribs; armour is a dark blue-black that lightens at the top, cloak a deep wine
/// that catches fire toward its hem.
fn albedo(cell: (u32, u32), part: Part, v: f32) -> Vec3 {
    match part {
        Part::Bone => gradient(
            &[
                (0.0, 0xf8f4ea),
                (0.5, 0xeee6d6),
                (0.76, 0xd4c8b4),
                (0.88, 0x8c7c78),
                (0.96, 0x2a2030),
                (1.0, 0x120e1a),
            ],
            v,
        ),
        Part::Armour => gradient(&[(0.0, 0x5a6480), (0.4, 0x363d58), (0.8, 0x1e2238), (1.0, 0x12142a)], v),
        Part::Steel => gradient(&[(0.0, 0xc4ccdc), (0.5, 0x8892a8), (1.0, 0x3e485e)], v),
        Part::Cloak => gradient(&[(0.25, 0x1e0a16), (0.45, 0x3a0e20), (0.62, 0x5c1226), (0.8, 0x7c1428)], v),
        Part::Leather if cell == (6, 2) => gradient(&[(0.0, 0x32283a), (1.0, 0x1a1420)], v),
        Part::Leather => gradient(&[(0.0, 0x3a2c32), (1.0, 0x1c141c)], v),
        Part::Trim => gradient(&[(0.0, 0x20222e), (1.0, 0x0c0c14)], v),
    }
}

/// The warrior's albedo, on the pack's layout. A cell no model uses holds the bone's middle, so a
/// far mip that reaches across cells stays near what's around it.
pub fn palette_png() -> io::Result<Vec<u8>> {
    const SIZE: u32 = 512;
    let image = image::RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let uv = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / SIZE as f32;
        let (column, row, v) = cell_of(uv);
        let color = match part_of(column, row) {
            Some(part) => albedo((column, row), part, v),
            None => rgb(0xb0a89c),
        };
        image::Rgb(color.to_array().map(|c| unorm8(linear_to_srgb(c))))
    });
    png(image)
}

/// A part's highlight (R) and its size (B).
fn highlight(part: Part) -> (f32, f32) {
    match part {
        Part::Bone => (0.25, 0.3),
        Part::Armour => (1.0, 0.72),
        Part::Steel => (1.0, 0.85),
        Part::Cloak | Part::Trim => (0.0, 0.5),
        Part::Leather => (0.5, 0.45),
    }
}

/// The shadow bias at `v` down a part's cell: the sockets and rib gaps stay dark whatever the light,
/// as anime characters' painted cavities do, and the rest shades as its normals say. The cloak
/// lights a little sooner, so its folds keep a broad lit side.
fn shadow_bias(part: Part, v: f32) -> f32 {
    match part {
        Part::Bone => 0.5 * (1.0 - smoothstep(0.86, 0.95, v)),
        Part::Cloak => 0.56,
        _ => 0.5,
    }
}

/// What `warrior.ilm.ron` packs into the control map, on the pack's layout: R highlight, G shadow
/// bias, B highlight size, and A the part's material ID, as gray levels the document snaps to.
pub fn control_png() -> io::Result<Vec<u8>> {
    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 256;
    let image = image::RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let uv = Vec2::new((x as f32 + 0.5) / WIDTH as f32, (y as f32 + 0.5) / HEIGHT as f32);
        let (column, row, v) = cell_of(uv);
        let part = part_of(column, row).unwrap_or(Part::Bone);
        let (specular, threshold) = highlight(part);
        let id = ilm::encode_material_id(part.id());
        image::Rgba([specular, shadow_bias(part, v), threshold, id].map(unorm8))
    });
    png(image)
}

/// The cloak's hem starts to burn this far down its cell.
const HEM: (f32, f32) = (0.6, 0.76);

/// The body's layer mask, on the pack's layout: R the matcap on armour and steel and a little on
/// leather, G nothing (nothing reflects), B the cloak's burning hem, A nothing (no stockings).
pub fn layers_png() -> io::Result<Vec<u8>> {
    const WIDTH: u32 = 128;
    const HEIGHT: u32 = 256;
    let image = image::RgbaImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let uv = Vec2::new((x as f32 + 0.5) / WIDTH as f32, (y as f32 + 0.5) / HEIGHT as f32);
        let (column, row, v) = cell_of(uv);
        let part = part_of(column, row).unwrap_or(Part::Bone);
        let matcap = match part {
            Part::Armour | Part::Steel => 1.0,
            Part::Leather => 0.35,
            _ => 0.0,
        };
        let emission = if part == Part::Cloak { smoothstep(HEM.0, HEM.1, v) } else { 0.0 };
        image::Rgba([matcap, 0.0, emission, 0.0].map(unorm8))
    });
    png(image)
}

/// Painted gunmetal for the matcap, added as light: nearly black, a hard band of sky across the
/// upper half and a faint warm kick along the bottom edge, so a dark plate reads as metal from any
/// side without any reflection.
pub fn matcap_png() -> io::Result<Vec<u8>> {
    const SIZE: u32 = 128;
    let image = image::RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / SIZE as f32 * 2.0 - Vec2::ONE;
        let n = Vec2::new(p.x, -p.y);
        // The band follows a tilted line over the sphere, crisp on its lower edge.
        let line = n.y + 0.28 * n.x - 0.36;
        let band = smoothstep(-0.02, 0.02, line) * (1.0 - smoothstep(0.08, 0.34, line));
        let edge = smoothstep(0.72, 0.98, n.length());
        let kick = smoothstep(0.35, 0.95, -n.y) * edge;
        let color = rgb(0x0a0c12) + rgb(0x8ea4c8) * band + rgb(0x4a2c34) * kick;
        image::Rgb(color.to_array().map(|c| unorm8(linear_to_srgb(c))))
    });
    png(image)
}

/// Encodes an image as PNG.
pub(crate) fn png<P: image::PixelWithColorType>(
    image: image::ImageBuffer<P, Vec<P::Subpixel>>,
) -> io::Result<Vec<u8>>
where
    [P::Subpixel]: image::EncodableLayout,
{
    let mut bytes = io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).map_err(io::Error::other)?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sapphire_assets::character_maps::ilm::decode_material_id;

    use super::*;
    use crate::generators::made;
    use crate::import::gltf;
    use crate::mesh::ImportedMesh;
    use crate::test_util::{assert_glb_matches, assert_png_matches};

    fn content_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content")
    }

    fn import(path: &str) -> ImportedMesh {
        gltf::import(&content_dir().join(path)).expect("it imports")
    }

    const WARRIOR_MODEL: &str = "combat/enemy/warrior.glb";
    const BLADE_MODEL: &str = "third_party/kaykit_skeletons/assets/Skeleton_Blade.gltf";

    /// Every triangle of the warrior and its blade lies within one cell the maps paint, whose V
    /// stays inside the cell, so no shape of theirs reads across a part's edge or an unpainted cell.
    #[test]
    fn every_triangle_lies_in_a_painted_cell() {
        for path in [WARRIOR_MODEL, BLADE_MODEL] {
            let mesh = import(path);
            let mut triangles = 0;
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let cells: Vec<(u32, u32)> = triangle
                    .iter()
                    .map(|&index| {
                        let (column, row, _) = cell_of(Vec2::from(mesh.vertices[index as usize].uv));
                        (column, row)
                    })
                    .collect();
                assert!(cells.iter().all(|&cell| cell == cells[0]), "{path}: a triangle across {cells:?}");
                assert!(
                    part_of(cells[0].0, cells[0].1).is_some(),
                    "{path}: cell {:?} is unpainted",
                    cells[0]
                );
                triangles += 1;
            }
            assert!(triangles > 0, "{path}");
        }
    }

    /// The warrior has what the stage and N3 find by name, and the slots its sidecar binds.
    #[test]
    fn the_warrior_has_its_hand_slot_and_two_slots() {
        let mesh = import(WARRIOR_MODEL);
        let skin = mesh.skin.as_ref().expect("skinned");
        for joint in ["hips", "head", "upperarm.l", "upperarm.r", "handslot.r"] {
            assert!(skin.joints.iter().any(|known| known.name == joint), "{joint}");
        }
        let names: Vec<&str> = mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
        assert_eq!(names, ["skeleton", "Glow"]);
        let blade = import(BLADE_MODEL);
        assert!(blade.skin.is_none(), "the blade is a mesh of its own, placed at the hand slot");
    }

    /// The control map's channels say each part's ID where a shader reads it.
    #[test]
    fn the_control_map_holds_each_cell_s_part() {
        let bytes = control_png().unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        let (width, height) = image.dimensions();
        for &(column, row, part) in &CELLS {
            for v in [0.1, 0.5, 0.9] {
                let x = ((column as f32 + 0.5) / COLUMNS as f32 * width as f32) as u32;
                let y = ((row as f32 + v) / ROWS as f32 * height as f32) as u32;
                let texel = image.get_pixel(x, y).0;
                let id = decode_material_id(f32::from(texel[3]) / 255.0);
                assert_eq!(id, part.id(), "cell ({column}, {row})");
            }
        }
    }

    #[test]
    fn generating_twice_is_byte_identical() {
        assert_eq!(made(files()).unwrap(), made(files()).unwrap());
    }

    /// The committed maps and models are what the generator makes, byte for byte, on every target and
    /// OS: angles go through `det`, the warrior's matrices through scalar `Affine3` and `Mat3` (never
    /// `Mat4`'s or `Quat`'s SIMD), and colours through `det::pow`. A failure on one OS is a
    /// determinism bug, never bytes to commit per platform: the comparison by value says how far off
    /// it is first. Regenerate with `cargo run -p sapphire_pipeline --profile tools -- gen-combat
    /// --out content/combat --force`.
    #[test]
    fn the_committed_content_is_generated() {
        let combat = content_dir().join("combat");
        let mut generated = made(files()).unwrap();
        let source = std::fs::read(combat.join(WARRIOR_SOURCE)).unwrap();
        generated.push((WARRIOR.to_owned(), warrior::warrior_glb(&source).unwrap()));
        for (name, generated) in generated {
            let committed = std::fs::read(combat.join(&name)).unwrap();
            if committed == generated {
                continue;
            }
            if name.ends_with(".glb") {
                assert_glb_matches(&name, &committed, &generated, |_, a, b| a == b);
            } else {
                assert_png_matches(&name, &committed, &generated);
            }
            panic!("{name}: the generated bytes differ from the committed ones");
        }
    }
}
