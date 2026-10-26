//! The environment test content (M1 W4): a small courtyard on rolling ground, for the sandbox's
//! `--environment`, with the environment materials it draws with and the maps they read, written
//! as sources so they travel the import → cook path an artist's world does. No third-party art:
//! every shape and texel is generated here.
//!
//! - **`courtyard.glb`**, one mesh in world space around the sandbox's character at the origin
//!   (which faces +Z), each part a material slot its sidecar binds to a material document:
//!   - `Ground`: 220 m of ground, flat for 7 m around the character, rolling beyond, and rising
//!     into hills from 45 m out, so terminators cross slopes at every distance;
//!   - `Rock`: boulders from 3 m away to 60 m, near ones small, far ones large, darker toward
//!     their feet (vertex AO);
//!   - `Stone`: a colonnade of round pillars on plinths, receding from the character's left into
//!     the distance, the wall's coping and the lantern posts;
//!   - `Wall`: a low plaster wall behind the character, with a gap;
//!   - `Lantern`: paper lanterns on posts and on the wall, emissive.
//! - **Materials** (`*.material.ron`), each softer than the characters it frames: wrap lighting,
//!   a terminator that widens with distance, a band along it on stone, and distance
//!   desaturation, under time of day's shadow tint.
//! - **`terminator_band.png`** (ramp): white at both ends, a warm peach across the middle of the
//!   light/shadow transition.
//! - **`ground.png`** (color): painted grass, olive and gold patches over green, tiling every
//!   [`GROUND_TILE`] meters.
//!
//! The golden scenes `environment-day`, `environment-sunset` and `environment-night` show it
//! with the character.

use std::f32::consts::{PI, TAU};
use std::io;

use glam::{Vec2, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    DistanceStylization, EnvironmentMaterial, MaterialDocument, Terminator, TerminatorBand,
};
use serde_json::json;

use super::noise::{Rng, fbm, smoothstep};
use super::shapes;
use crate::color::{srgb8, unorm8};
use crate::gltf_writer::{Geometry, GltfWriter};

/// The directory of the environment content, under the samples directory.
pub const DIR: &str = "environment";
/// File names, in [`DIR`].
pub const COURTYARD_FILE_NAME: &str = "courtyard.glb";
pub const GROUND_MATERIAL_FILE_NAME: &str = "ground.material.ron";
pub const ROCK_MATERIAL_FILE_NAME: &str = "rock.material.ron";
pub const STONE_MATERIAL_FILE_NAME: &str = "stone.material.ron";
pub const WALL_MATERIAL_FILE_NAME: &str = "wall.material.ron";
pub const LANTERN_MATERIAL_FILE_NAME: &str = "lantern.material.ron";
pub const BAND_FILE_NAME: &str = "terminator_band.png";
pub const GROUND_COLOR_FILE_NAME: &str = "ground.png";

/// The assets' IDs, which their committed sidecars (`content/samples/environment/*.meta`) give
/// them: materials name their maps by ID, and the courtyard's sidecar binds its slots to the
/// materials.
pub const COURTYARD_ID: AssetId = AssetId::from_text("14ad5875-ec27-4866-bb2e-501408f4e1ad");
pub const GROUND_MATERIAL_ID: AssetId = AssetId::from_text("fdb90952-4488-42d6-a474-55115aa2db08");
pub const ROCK_MATERIAL_ID: AssetId = AssetId::from_text("26b87ba8-6b44-4aad-a05e-93315d6e4ab1");
pub const STONE_MATERIAL_ID: AssetId = AssetId::from_text("74541928-2897-450e-9528-157d19d9e9a4");
pub const WALL_MATERIAL_ID: AssetId = AssetId::from_text("4e68ee72-a635-4ca3-8fb9-87c813226709");
pub const LANTERN_MATERIAL_ID: AssetId = AssetId::from_text("75c36792-1225-42ae-aa70-95f4c3f84f2c");
pub const BAND_ID: AssetId = AssetId::from_text("14d1e642-3b75-456f-b100-0395230326bb");
pub const GROUND_COLOR_ID: AssetId = AssetId::from_text("9792c615-2cbd-494c-9681-750dfbc10e31");

/// The courtyard's material slots, in file order, with the material each is bound to.
pub const SLOTS: [(&str, AssetId); 5] = [
    ("Ground", GROUND_MATERIAL_ID),
    ("Rock", ROCK_MATERIAL_ID),
    ("Stone", STONE_MATERIAL_ID),
    ("Wall", WALL_MATERIAL_ID),
    ("Lantern", LANTERN_MATERIAL_ID),
];

/// Meters from the character to the ground's edge.
const GROUND_HALF: f32 = 110.0;
/// Ground vertices along each side.
const GROUND_STEPS: u32 = 96;
/// Meters the ground's color map spans before it repeats: 2 cm a texel, so the ground at the
/// character's feet stays sharp at 1080 rows.
pub const GROUND_TILE: f32 = 10.0;
/// The flat courtyard's radius around the character, and where the hills begin.
const FLAT_RADIUS: f32 = 7.0;
const HILLS_START: f32 = 45.0;

/// Every environment file, relative to the samples directory, with its bytes.
pub fn files() -> io::Result<Vec<(String, Vec<u8>)>> {
    let path = |name: &str| format!("{DIR}/{name}");
    Ok(vec![
        (path(COURTYARD_FILE_NAME), courtyard_glb()),
        (path(GROUND_MATERIAL_FILE_NAME), document(&ground_material())),
        (path(ROCK_MATERIAL_FILE_NAME), document(&rock_material())),
        (path(STONE_MATERIAL_FILE_NAME), document(&stone_material())),
        (path(WALL_MATERIAL_FILE_NAME), document(&wall_material())),
        (path(LANTERN_MATERIAL_FILE_NAME), document(&lantern_material())),
        (path(BAND_FILE_NAME), band_png()?),
        (path(GROUND_COLOR_FILE_NAME), ground_png()?),
    ])
}

fn document(material: &EnvironmentMaterial) -> Vec<u8> {
    material.validate().expect("the environment materials are valid");
    sapphire_serialize::to_ron(material).expect("a material document serializes").into_bytes()
}

/// Linear RGB from 8-bit sRGB, the way an artist picks it.
fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    let [r, g, b, _] = srgb8(r, g, b);
    [r, g, b]
}

/// What every courtyard material shares: a soft painted falloff, a terminator that softens far
/// away, and distant color fading toward grey.
fn painted(base: [f32; 3]) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [base[0], base[1], base[2], 1.0],
        wrap: 0.4,
        terminator: Terminator { width: 0.06, growth: 0.008, band: None },
        distance: DistanceStylization { start: 15.0, end: 140.0, desaturation: 0.5 },
        roughness: 0.85,
        roughness_range: [0.4, 1.0],
        specular_clamp: 0.25,
        ..EnvironmentMaterial::default()
    }
}

/// Grass over the painted ground map: the widest wrap and terminator, the most distance fade.
pub fn ground_material() -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(GROUND_COLOR_ID),
        wrap: 0.45,
        terminator: Terminator { width: 0.08, growth: 0.008, band: None },
        distance: DistanceStylization { start: 18.0, end: 150.0, desaturation: 0.55 },
        roughness: 0.95,
        specular_clamp: 0.12,
        ..painted([1.0; 3])
    }
}

/// Warm grey boulders, with a soft band along their terminators.
pub fn rock_material() -> EnvironmentMaterial {
    EnvironmentMaterial {
        terminator: Terminator {
            width: 0.06,
            growth: 0.008,
            band: Some(TerminatorBand { map: BAND_ID, strength: 0.5 }),
        },
        shadow_tint: [1.0, 0.95, 1.05],
        roughness: 0.8,
        ..painted(rgb(176, 164, 150))
    }
}

/// Pale cream stone, smoother: a crisper terminator with a warm band, and a gloss kept under
/// the reflective range.
pub fn stone_material() -> EnvironmentMaterial {
    EnvironmentMaterial {
        wrap: 0.3,
        terminator: Terminator {
            width: 0.04,
            growth: 0.007,
            band: Some(TerminatorBand { map: BAND_ID, strength: 0.7 }),
        },
        roughness: 0.55,
        roughness_range: [0.25, 0.6],
        specular_clamp: 0.4,
        ..painted(rgb(226, 214, 192))
    }
}

/// Terracotta plaster.
pub fn wall_material() -> EnvironmentMaterial {
    EnvironmentMaterial { roughness: 0.9, ..painted(rgb(214, 162, 138)) }
}

/// Paper lanterns: their own warm light, by day a glow, at night the brightest thing about.
pub fn lantern_material() -> EnvironmentMaterial {
    EnvironmentMaterial {
        wrap: 0.5,
        // Under the tone curve's shoulder at night's exposure, so the glow stays orange.
        emission: [0.75, 0.3, 0.08],
        distance: DistanceStylization { desaturation: 0.0, ..DistanceStylization::default() },
        ..painted(rgb(245, 214, 160))
    }
}

/// The terminator band: white at both ends, and a warm peach across the middle of the
/// transition. Rows are alike; the shader reads one.
pub fn band_png() -> io::Result<Vec<u8>> {
    const WIDTH: u32 = 64;
    let peach = [255.0, 196.0, 158.0];
    let image = image::RgbaImage::from_fn(WIDTH, 4, |x, _| {
        let u = (x as f32 + 0.5) / WIDTH as f32;
        let warmth = smoothstep(0.0, 1.0, 1.0 - (2.0 * u - 1.0).abs());
        let channel = |c: f32| (255.0 + (c - 255.0) * warmth).round() as u8;
        image::Rgba([channel(peach[0]), channel(peach[1]), channel(peach[2]), 255])
    });
    png(image)
}

/// Painted grass: green, with olive and gold patches and small darker clumps, tiling.
pub fn ground_png() -> io::Result<Vec<u8>> {
    const SIZE: u32 = 512;
    let green = Vec3::from(rgb(124, 152, 92));
    let olive = Vec3::from(rgb(150, 160, 94));
    let gold = Vec3::from(rgb(186, 170, 110));
    let clump = Vec3::from(rgb(102, 134, 80));
    let image = image::RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / SIZE as f32;
        let patches = fbm(p * 4.0, (4, 4), 4, 0xe101);
        let warm = fbm(p * 3.0 + Vec2::splat(7.1), (3, 3), 3, 0xe102);
        let clumps = fbm(p * 32.0, (32, 32), 2, 0xe103);
        let mut color = green.lerp(olive, smoothstep(0.4, 0.65, patches));
        color = color.lerp(gold, 0.6 * smoothstep(0.58, 0.75, warm));
        color = color.lerp(clump, 0.35 * smoothstep(0.55, 0.8, clumps));
        let srgb = color.to_array().map(|c| unorm8(crate::color::linear_to_srgb(c)));
        image::Rgb(srgb)
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

/// The ground's height in meters at `(x, z)`: flat around the character, rolling beyond, and
/// rising into hills far out.
pub fn ground_height(x: f32, z: f32) -> f32 {
    let r = x.hypot(z);
    let rolling = 1.3 * (x / 7.0).sin() * (z / 9.0).cos() + 0.8 * ((x + 2.0 * z) / 13.0).sin();
    let hills = 9.0 + 7.0 * (3.0 * z.atan2(x) + 0.6).sin();
    smoothstep(FLAT_RADIUS, 26.0, r) * (rolling + 0.9) + smoothstep(HILLS_START, 95.0, r) * hills
}

/// The courtyard as a binary glTF: one mesh, a primitive per slot, in world space.
pub fn courtyard_glb() -> Vec<u8> {
    let mut gltf = GltfWriter::default();
    let parts = [ground(), rocks(), stone(), wall(), lanterns()];
    let primitives: Vec<(Geometry, usize)> = SLOTS
        .iter()
        .zip(parts)
        .map(|(&(name, _), geometry)| {
            let material = gltf.add_material(json!({
                "name": name,
                "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0] },
            }));
            (geometry, material)
        })
        .collect();
    let primitives: Vec<(&Geometry, Option<usize>)> =
        primitives.iter().map(|(geometry, material)| (geometry, Some(*material))).collect();
    let mesh = gltf.add_mesh("Courtyard", &primitives);
    let root = gltf.add_node(json!({ "name": "Courtyard", "mesh": mesh }));
    gltf.add_root(root);
    gltf.to_glb()
}

/// The ground: a grid over the height field, normals from its slopes, UVs tiling the color map.
fn ground() -> Geometry {
    let mut geometry = Geometry::default();
    let step = 2.0 * GROUND_HALF / GROUND_STEPS as f32;
    for row in 0..=GROUND_STEPS {
        for column in 0..=GROUND_STEPS {
            let (x, z) = (-GROUND_HALF + column as f32 * step, -GROUND_HALF + row as f32 * step);
            let e = 0.05;
            let dx = (ground_height(x + e, z) - ground_height(x - e, z)) / (2.0 * e);
            let dz = (ground_height(x, z + e) - ground_height(x, z - e)) / (2.0 * e);
            geometry.positions.push([x, ground_height(x, z), z]);
            geometry.normals.push(Vec3::new(-dx, 1.0, -dz).normalize().to_array());
            geometry.uvs.push([x / GROUND_TILE, z / GROUND_TILE]);
            geometry.colors.push([255; 4]);
        }
    }
    let stride = GROUND_STEPS + 1;
    for row in 0..GROUND_STEPS {
        for column in 0..GROUND_STEPS {
            let a = row * stride + column;
            let (b, c, d) = (a + 1, a + stride, a + stride + 1);
            // Counter-clockwise seen from above.
            geometry.indices.extend([a, c, b, b, c, d]);
        }
    }
    geometry
}

/// The boulders: `(x, z, radius)`, near and small to far and large.
const ROCKS: [(f32, f32, f32); 12] = [
    (3.3, -2.2, 0.7),
    (-2.6, -5.8, 1.1),
    (4.4, -6.5, 0.45),
    (6.5, -13.0, 1.8),
    (-8.5, -15.0, 1.4),
    (-10.5, -23.0, 2.4),
    (12.0, -22.0, 1.2),
    (16.0, -36.0, 3.2),
    (-4.0, -38.0, 2.0),
    (-22.0, -48.0, 4.0),
    (26.0, -58.0, 5.0),
    (2.0, -64.0, 3.5),
];

/// Boulders: spheres pushed out and in by a few broad lobes, flattened and sunk into the ground,
/// darker toward their feet.
fn rocks() -> Geometry {
    const RINGS: u32 = 16;
    const SEGMENTS: u32 = 22;
    let mut geometry = Geometry::default();
    for (index, &(x, z, radius)) in ROCKS.iter().enumerate() {
        let mut rng = Rng(0x0c0c + index as u32);
        let lobes: Vec<(Vec3, f32)> = (0..5)
            .map(|_| {
                let direction =
                    Vec3::new(rng.range(-1.0, 1.0), rng.range(-0.4, 1.0), rng.range(-1.0, 1.0)).normalize();
                (direction, rng.range(0.08, 0.2))
            })
            .collect();
        let shape = |n: Vec3| {
            let bulge: f32 = lobes.iter().map(|&(direction, amount)| amount * n.dot(direction)).sum();
            n * (1.0 + bulge) * Vec3::new(1.0, 0.68, 1.0) * radius
        };
        let base = Vec3::new(x, ground_height(x, z) - 0.25 * radius, z);
        let mut rock = sphere_surface(RINGS, SEGMENTS, shape);
        for (position, color) in rock.positions.iter_mut().zip(&mut rock.colors) {
            let height = position[1];
            let ao = 0.55 + 0.45 * smoothstep(-0.4 * radius, 0.5 * radius, height);
            *color = [unorm8(ao), 255, 255, 255];
            *position = (Vec3::from(*position) + base).to_array();
        }
        append(&mut geometry, rock);
    }
    geometry
}

/// A closed surface over the unit sphere's directions, moved by `shape`, with normals from the
/// moved surface's tangents and spherical UVs. Counter-clockwise from outside.
fn sphere_surface(rings: u32, segments: u32, shape: impl Fn(Vec3) -> Vec3) -> Geometry {
    let mut geometry = Geometry::default();
    let direction =
        |theta: f32, phi: f32| Vec3::new(theta.sin() * phi.sin(), theta.cos(), theta.sin() * phi.cos());
    for ring in 0..=rings {
        let theta = PI * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = TAU * segment as f32 / segments as f32;
            let e = 1e-3;
            let d_theta = shape(direction(theta + e, phi)) - shape(direction(theta - e, phi));
            let d_phi = shape(direction(theta, phi + e)) - shape(direction(theta, phi - e));
            let normal = d_theta.cross(d_phi).try_normalize().unwrap_or(Vec3::Y * theta.cos().signum());
            // At the poles the tangent along phi vanishes: the axis is the normal.
            let normal = if ring == 0 || ring == rings { Vec3::Y * theta.cos().signum() } else { normal };
            geometry.positions.push(shape(direction(theta, phi)).to_array());
            geometry.normals.push(normal.to_array());
            geometry.uvs.push([segment as f32 / segments as f32, ring as f32 / rings as f32]);
            geometry.colors.push([255; 4]);
        }
    }
    let stride = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            // The rings at the poles are single points: each quad there is one triangle.
            if ring > 0 {
                geometry.indices.extend([a, b, c]);
            }
            if ring + 1 < rings {
                geometry.indices.extend([c, b, d]);
            }
        }
    }
    geometry
}

/// The colonnade's pillars: meters along its line from the first, which stands left of the
/// character and a little behind.
const PILLARS: [f32; 9] = [0.0, 5.0, 10.5, 16.5, 23.0, 30.0, 38.0, 47.0, 57.0];
const COLONNADE_START: Vec2 = Vec2::new(-3.4, -3.2);
const COLONNADE_DIRECTION: Vec2 = Vec2::new(-0.33, -0.94);
const PILLAR_RADIUS: f32 = 0.32;
const PILLAR_HEIGHT: f32 = 3.0;

/// Where the wall stands: its segments' `(x from, x to)` at [`WALL_Z`], its height and half
/// thickness.
const WALL_SEGMENTS: [(f32, f32); 2] = [(-0.5, 5.0), (7.5, 16.0)];
const WALL_Z: f32 = -9.5;
const WALL_HEIGHT: f32 = 1.7;
const WALL_HALF_THICKNESS: f32 = 0.25;

/// The lantern posts: `(x, z)`.
const POSTS: [(f32, f32); 2] = [(-1.9, -1.4), (2.4, -4.4)];
const POST_HEIGHT: f32 = 1.55;

/// Everything of stone: the pillars on their plinths under their capitals, the wall's coping and
/// the lantern posts.
fn stone() -> Geometry {
    let mut geometry = Geometry::default();
    for &along in &PILLARS {
        let at = COLONNADE_START + COLONNADE_DIRECTION * along;
        let floor = ground_height(at.x, at.y) - 0.1;
        let base = Vec3::new(at.x, floor, at.y);
        let plinth = shapes::cuboid([0.48, 0.18, 0.48]);
        append(&mut geometry, placed(plinth, base + Vec3::Y * 0.18));
        // Darker toward its foot.
        let shaft = occluded(shapes::cylinder(PILLAR_RADIUS, PILLAR_HEIGHT, 28), |y| {
            0.6 + 0.4 * smoothstep(0.0, 1.2, y)
        });
        append(&mut geometry, placed(shaft, base + Vec3::Y * 0.36));
        let capital = shapes::cuboid([0.44, 0.13, 0.44]);
        append(&mut geometry, placed(capital, base + Vec3::Y * (0.36 + PILLAR_HEIGHT + 0.13)));
    }
    for &(from, to) in &WALL_SEGMENTS {
        let coping = shapes::cuboid([(to - from) * 0.5 + 0.06, 0.07, WALL_HALF_THICKNESS + 0.06]);
        let center = Vec3::new((from + to) * 0.5, WALL_HEIGHT + 0.07, WALL_Z);
        append(&mut geometry, placed(coping, center));
    }
    for &(x, z) in &POSTS {
        let post = shapes::cuboid([0.05, POST_HEIGHT * 0.5, 0.05]);
        append(&mut geometry, placed(post, Vec3::new(x, POST_HEIGHT * 0.5, z)));
    }
    geometry
}

/// The wall: a box per segment, sunk a little into the ground.
fn wall() -> Geometry {
    let mut geometry = Geometry::default();
    for &(from, to) in &WALL_SEGMENTS {
        let segment = shapes::cuboid([(to - from) * 0.5, WALL_HEIGHT * 0.5 + 0.1, WALL_HALF_THICKNESS]);
        // Its lower corners darker, where it meets the ground.
        let segment = occluded(segment, |y| if y < 0.0 { 0.7 } else { 1.0 });
        let center = Vec3::new((from + to) * 0.5, WALL_HEIGHT * 0.5 - 0.1, WALL_Z);
        append(&mut geometry, placed(segment, center));
    }
    geometry
}

/// Paper lanterns: one on each post, and two on the wall.
fn lanterns() -> Geometry {
    let mut geometry = Geometry::default();
    let on_posts = POSTS.iter().map(|&(x, z)| Vec3::new(x, POST_HEIGHT + 0.2, z));
    let on_wall = [1.2, 12.5].map(|x| Vec3::new(x, WALL_HEIGHT + 0.14 + 0.2, WALL_Z));
    for center in on_posts.chain(on_wall) {
        append(&mut geometry, placed(shapes::capsule(0.15, 0.06, 16, 6), center));
    }
    geometry
}

/// `geometry` with its vertex color's ambient occlusion (R) at `ao(y)` of each vertex's height.
fn occluded(mut geometry: Geometry, ao: impl Fn(f32) -> f32) -> Geometry {
    geometry.colors =
        geometry.positions.iter().map(|position| [unorm8(ao(position[1])), 255, 255, 255]).collect();
    geometry
}

/// `geometry` moved by `offset`, with a vertex color for every vertex (AO 1 where it had none).
fn placed(mut geometry: Geometry, offset: Vec3) -> Geometry {
    for position in &mut geometry.positions {
        *position = (Vec3::from(*position) + offset).to_array();
    }
    geometry.colors.resize(geometry.positions.len(), [255; 4]);
    geometry
}

/// Appends `part` to `geometry`.
fn append(geometry: &mut Geometry, part: Geometry) {
    let first = geometry.positions.len() as u32;
    geometry.positions.extend(part.positions);
    geometry.normals.extend(part.normals);
    geometry.uvs.extend(part.uvs);
    geometry.colors.extend(part.colors);
    geometry.indices.extend(part.indices.into_iter().map(|index| first + index));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use sapphire_assets::material::AnyMaterialDocument;
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::import;

    #[test]
    fn the_courtyard_imports_with_a_slot_per_material() {
        let mesh = import::gltf::import_slice(&courtyard_glb(), None).expect("import");
        let names: Vec<&str> = mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
        assert_eq!(names, SLOTS.map(|(name, _)| name));
        let cooked = mesh.pack(PositionFormat::Unorm16, &BTreeMap::new()).unwrap().to_cooked().expect("cook");
        sapphire_assets::mesh::Mesh::read(&cooked).expect("structurally valid");
        // Every triangle faces the way its normals do.
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|i| Vec3::from(mesh.vertices[i as usize].position));
            let face = (b - a).cross(c - a);
            let normals: Vec3 = triangle.iter().map(|&i| Vec3::from(mesh.vertices[i as usize].normal)).sum();
            assert!(face.dot(normals) > 0.0, "triangle {triangle:?} is wound against its normals");
        }
    }

    #[test]
    fn the_character_stands_on_flat_ground_and_hills_rise_far_out() {
        for (x, z) in [(0.0, 0.0), (FLAT_RADIUS * 0.7, 0.0), (0.0, -FLAT_RADIUS * 0.7)] {
            assert_eq!(ground_height(x, z), 0.0, "flat at ({x}, {z})");
        }
        let far: f32 = (0..36)
            .map(|i| (i as f32 * TAU / 36.0).sin_cos())
            .map(|(s, c)| ground_height(100.0 * c, 100.0 * s))
            .sum::<f32>()
            / 36.0;
        assert!(far > 6.0, "hills on the horizon: {far} m on average");
    }

    #[test]
    fn materials_name_the_maps_and_sidecars_give_the_ids() {
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples").join(DIR);
        let ids = [
            (COURTYARD_FILE_NAME, COURTYARD_ID),
            (GROUND_MATERIAL_FILE_NAME, GROUND_MATERIAL_ID),
            (ROCK_MATERIAL_FILE_NAME, ROCK_MATERIAL_ID),
            (STONE_MATERIAL_FILE_NAME, STONE_MATERIAL_ID),
            (WALL_MATERIAL_FILE_NAME, WALL_MATERIAL_ID),
            (LANTERN_MATERIAL_FILE_NAME, LANTERN_MATERIAL_ID),
            (BAND_FILE_NAME, BAND_ID),
            (GROUND_COLOR_FILE_NAME, GROUND_COLOR_ID),
        ];
        for (file, id) in ids {
            let meta = crate::meta::read(&crate::meta::sidecar_path(&samples.join(file))).expect("a sidecar");
            assert_eq!(meta.id, id, "{file}");
        }
        for (_, bytes) in files().expect("files") {
            let Ok(text) = String::from_utf8(bytes) else { continue };
            let document = AnyMaterialDocument::from_ron(&text).expect("a material document");
            let textures = document.textures();
            assert!(
                textures.iter().flatten().all(|id| [BAND_ID, GROUND_COLOR_ID].contains(id)),
                "{textures:?}"
            );
        }
        let meta = crate::meta::read(&crate::meta::sidecar_path(&samples.join(COURTYARD_FILE_NAME)))
            .expect("a sidecar");
        let crate::meta::ImportSettings::Gltf(settings) = meta.importer else { panic!("a glTF sidecar") };
        let bound: Vec<(&str, AssetId)> =
            settings.materials.iter().map(|(name, id)| (name.as_str(), *id)).collect();
        let mut slots = SLOTS.to_vec();
        slots.sort_unstable();
        assert_eq!(bound, slots, "every slot bound to its material");
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(courtyard_glb(), courtyard_glb());
        assert_eq!(band_png().unwrap(), band_png().unwrap());
        assert_eq!(ground_png().unwrap(), ground_png().unwrap());
    }
}
