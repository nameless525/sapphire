//! The foliage test content (M1 W6): a stylized tree, a bush and a patch of grass, for the
//! sandbox's `--foliage`, with the foliage materials they draw with and the cards' maps, written
//! as sources so they travel the import → bake → cook path an artist's plants do. No third-party
//! art: every shape and texel is generated here.
//!
//! - **`tree.glb`**: a trunk and five branches (`Bark`), and six clumps of leaf cards around the
//!   branches' tips and the crown (`Leaves`). Its sidecar bakes foliage with `Hull(count: 6)`, so
//!   the canopy lights as one soft volume over its clumps, the wood's normals left as modeled.
//! - **`bush.glb`**: a dome of leaf cards (`Leaves`), baked with `Sphere`.
//! - **`grass.glb`**: a round patch of grass cards (`Blades`) 4 m across, thinning and shortening
//!   toward its edge so patches blend into the ground and each other, each card two rows of quads
//!   so blades bend; baked with `Up` and a full blend: the field lights like the ground under it.
//! - **Materials** (`*.material.ron`, the foliage family): bark opaque (an alpha cutoff of 0) and
//!   swaying with the leaves it carries; leaves, bush and grass alpha-tested, with canopy
//!   occlusion, a root-to-tip color gradient, translucency and wind responses of their own.
//! - **`leaves.png`** (color, alpha coverage): a cluster of pointed leaves around the card's
//!   middle, light values the materials' gradients tint. **`grass.png`**: blades rising from the
//!   card's foot, tapering to their tips.
//!
//! Every plant's origin is its root, as the foliage bake and the wind expect. Their sidecars turn
//! LODs off: simplification would merge cards (card-thinning LODs and impostors come later). The
//! golden scenes `foliage-*` show them in the courtyard.

use std::f32::consts::{PI, TAU};
use std::io;

use glam::{Vec2, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    DistanceStylization, FoliageGradient, FoliageMaterial, MaterialDocument, Translucency, Wind,
};
use serde_json::json;

use super::noise::{Rng, fbm, smoothstep};
use crate::color::{srgb8, unorm8};
use crate::gltf_writer::{Geometry, GltfWriter};

/// The directory of the foliage content, under the samples directory.
pub const DIR: &str = "foliage";
/// File names, in [`DIR`].
pub const TREE_FILE_NAME: &str = "tree.glb";
pub const BUSH_FILE_NAME: &str = "bush.glb";
pub const GRASS_FILE_NAME: &str = "grass.glb";
pub const BARK_MATERIAL_FILE_NAME: &str = "bark.material.ron";
pub const LEAVES_MATERIAL_FILE_NAME: &str = "leaves.material.ron";
pub const BUSH_MATERIAL_FILE_NAME: &str = "bush.material.ron";
pub const GRASS_MATERIAL_FILE_NAME: &str = "grass.material.ron";
pub const LEAVES_COLOR_FILE_NAME: &str = "leaves.png";
pub const GRASS_COLOR_FILE_NAME: &str = "grass.png";

/// The assets' IDs, which their committed sidecars (`content/samples/foliage/*.meta`) give them:
/// materials name their maps by ID, and the plants' sidecars bind their slots to the materials.
pub const TREE_ID: AssetId = AssetId::from_text("2feef8a7-6b16-4c41-ae6d-44b73cdabb44");
pub const BUSH_ID: AssetId = AssetId::from_text("a24406d9-3d40-4468-b165-d8eaa06d10bf");
pub const GRASS_ID: AssetId = AssetId::from_text("14d8adcb-6683-4cdd-babd-a3433d3dac33");
pub const BARK_MATERIAL_ID: AssetId = AssetId::from_text("9404bc18-2769-461b-8289-5e7dad9448ae");
pub const LEAVES_MATERIAL_ID: AssetId = AssetId::from_text("5fe64bfd-c3bd-4e5a-917c-5c552c7fa63b");
pub const BUSH_MATERIAL_ID: AssetId = AssetId::from_text("76b958ec-5757-479a-8dce-4d83f07b3fe5");
pub const GRASS_MATERIAL_ID: AssetId = AssetId::from_text("8206c2ad-a44b-4224-a39c-a7636cff957e");
pub const LEAVES_COLOR_ID: AssetId = AssetId::from_text("3b93f25e-0f01-488c-b5ba-691136d40542");
pub const GRASS_COLOR_ID: AssetId = AssetId::from_text("e23a7846-ca32-4369-9832-661a15dd4efd");

/// Each plant's material slots, in file order, with the material each is bound to.
pub const TREE_SLOTS: [(&str, AssetId); 2] = [("Bark", BARK_MATERIAL_ID), ("Leaves", LEAVES_MATERIAL_ID)];
pub const BUSH_SLOTS: [(&str, AssetId); 1] = [("Leaves", BUSH_MATERIAL_ID)];
pub const GRASS_SLOTS: [(&str, AssetId); 1] = [("Blades", GRASS_MATERIAL_ID)];

/// The grass patch's radius, in meters.
pub const GRASS_RADIUS: f32 = 2.0;

/// Every foliage file, relative to the samples directory, with its bytes.
pub fn files() -> io::Result<Vec<(String, Vec<u8>)>> {
    let path = |name: &str| format!("{DIR}/{name}");
    Ok(vec![
        (path(TREE_FILE_NAME), tree_glb()),
        (path(BUSH_FILE_NAME), bush_glb()),
        (path(GRASS_FILE_NAME), grass_glb()),
        (path(BARK_MATERIAL_FILE_NAME), document(&bark_material())),
        (path(LEAVES_MATERIAL_FILE_NAME), document(&leaves_material())),
        (path(BUSH_MATERIAL_FILE_NAME), document(&bush_material())),
        (path(GRASS_MATERIAL_FILE_NAME), document(&grass_material())),
        (path(LEAVES_COLOR_FILE_NAME), leaves_png()?),
        (path(GRASS_COLOR_FILE_NAME), grass_png()?),
    ])
}

fn document(material: &FoliageMaterial) -> Vec<u8> {
    material.validate().expect("the foliage materials are valid");
    sapphire_serialize::to_ron(material).expect("a material document serializes").into_bytes()
}

/// Linear RGB from 8-bit sRGB, the way an artist picks it.
fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    let [r, g, b, _] = srgb8(r, g, b);
    [r, g, b]
}

/// The tree's wind response, which its bark and leaves share so the canopy stays on its
/// branches: the crown leans a few centimeters, branch tips swing a little more.
const TREE_WIND: Wind = Wind { sway: 0.09, bend: 0.12, flutter: 0.035, flutter_frequency: 2.2 };

/// What every foliage sample material shares: a soft painted falloff, a terminator that softens
/// far away under a lavender shade, and distant color fading toward grey, as the courtyard's.
fn painted() -> FoliageMaterial {
    FoliageMaterial {
        wrap: 0.5,
        terminator_width: 0.16,
        terminator_growth: 0.006,
        shadow_tint: [0.95, 0.95, 1.08],
        distance: DistanceStylization { start: 15.0, end: 140.0, desaturation: 0.45 },
        ..FoliageMaterial::default()
    }
}

/// Grey-brown bark, opaque, darker at the root; no translucency, and the tree's sway and bend.
pub fn bark_material() -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        alpha_cutoff: 0.0,
        wrap: 0.35,
        terminator_width: 0.08,
        canopy_occlusion: 0.0,
        translucency: Translucency { strength: 0.0, ..Translucency::default() },
        gradient: FoliageGradient { root: rgb(86, 72, 66), tip: rgb(138, 118, 102) },
        wind: TREE_WIND,
        ..painted()
    }
}

/// The tree's leaves: deep teal-green inside the canopy to a warm yellow-green on its surface,
/// glowing yellow-green against a low sun.
pub fn leaves_material() -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(LEAVES_COLOR_ID),
        alpha_cutoff: 0.45,
        canopy_occlusion: 0.55,
        translucency: Translucency { color: rgb(214, 236, 120), strength: 0.7, power: 4.0 },
        gradient: FoliageGradient { root: rgb(54, 110, 92), tip: rgb(176, 214, 102) },
        wind: TREE_WIND,
        ..painted()
    }
}

/// The bush: a cooler, bluer green, stiffer in the wind, fluttering faster.
pub fn bush_material() -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(LEAVES_COLOR_ID),
        alpha_cutoff: 0.45,
        canopy_occlusion: 0.6,
        translucency: Translucency { color: rgb(190, 228, 140), strength: 0.5, power: 4.0 },
        gradient: FoliageGradient { root: rgb(40, 92, 86), tip: rgb(128, 188, 116) },
        wind: Wind { sway: 0.03, bend: 0.04, flutter: 0.025, flutter_frequency: 2.6 },
        ..painted()
    }
}

/// Grass in the ground map's palette: green at the root to a sunlit yellow-green at the tips,
/// the roots occluded, swaying lightly.
pub fn grass_material() -> FoliageMaterial {
    FoliageMaterial {
        base_color: [1.0; 4],
        base_color_map: Some(GRASS_COLOR_ID),
        alpha_cutoff: 0.4,
        wrap: 0.55,
        terminator_width: 0.2,
        canopy_occlusion: 0.45,
        translucency: Translucency { color: rgb(220, 232, 130), strength: 0.45, power: 3.0 },
        gradient: FoliageGradient { root: rgb(96, 132, 72), tip: rgb(196, 214, 120) },
        wind: Wind { sway: 0.05, bend: 0.06, flutter: 0.03, flutter_frequency: 1.7 },
        distance: DistanceStylization { start: 18.0, end: 150.0, desaturation: 0.5 },
        ..painted()
    }
}

/// A plant as a binary glTF: one mesh, a primitive per slot, its origin at the root.
fn plant_glb(name: &str, slots: &[(&str, AssetId)], parts: Vec<Geometry>) -> Vec<u8> {
    let mut gltf = GltfWriter::default();
    let primitives: Vec<(Geometry, usize)> = slots
        .iter()
        .zip(parts)
        .map(|(&(slot, _), geometry)| {
            let material = gltf.add_material(json!({
                "name": slot,
                "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0] },
            }));
            (geometry, material)
        })
        .collect();
    let primitives: Vec<(&Geometry, Option<usize>)> =
        primitives.iter().map(|(geometry, material)| (geometry, Some(*material))).collect();
    let mesh = gltf.add_mesh(name, &primitives);
    let root = gltf.add_node(json!({ "name": name, "mesh": mesh }));
    gltf.add_root(root);
    gltf.to_glb()
}

/// The tree's branches: `(height on the trunk, azimuth in turns, length, rise per meter out)`.
const BRANCHES: [(f32, f32, f32, f32); 5] = [
    (2.2, 0.05, 1.5, 0.55),
    (2.5, 0.27, 1.35, 0.6),
    (2.35, 0.5, 1.45, 0.5),
    (2.7, 0.7, 1.3, 0.65),
    (2.9, 0.88, 1.2, 0.7),
];
/// Where the trunk ends, under the crown's clump.
const TRUNK_TOP: f32 = 3.6;

/// The trunk's axis at height `y`: it leans a little, more toward the top.
fn trunk(y: f32) -> Vec3 {
    let t = y / TRUNK_TOP;
    Vec3::new(0.12 * t * t, y, -0.05 * t)
}

/// A point `s` meters out along a branch: rising, then arching over toward its tip.
fn branch((height, turns, _, rise): (f32, f32, f32, f32), s: f32) -> Vec3 {
    let out = Vec3::new((turns * TAU).cos(), 0.0, (turns * TAU).sin());
    trunk(height) + out * s + Vec3::Y * (rise * s - 0.12 * s * s)
}

/// The tree: a trunk and branches of bark, and clumps of leaf cards.
pub fn tree_glb() -> Vec<u8> {
    plant_glb("Tree", &TREE_SLOTS, vec![tree_wood(), tree_leaves()])
}

/// The trunk, leaning a little, and its branches, each a tapered tube.
fn tree_wood() -> Geometry {
    let mut wood = Geometry::default();
    let trunk_points: Vec<Vec3> = (0..=8).map(|i| trunk(TRUNK_TOP * i as f32 / 8.0)).collect();
    // Flared at the foot, thin under the crown.
    let trunk_radii: Vec<f32> =
        (0..=8).map(|i| i as f32 / 8.0).map(|t| 0.1 + 0.08 * (1.0 - t) + 0.08 * (1.0 - t).powi(6)).collect();
    append(&mut wood, tube(&trunk_points, &trunk_radii, 12));
    for shape in BRANCHES {
        let points: Vec<Vec3> = (0..=5).map(|i| branch(shape, shape.2 * i as f32 / 5.0)).collect();
        let radii: Vec<f32> = (0..=5).map(|i| 0.06 - 0.035 * i as f32 / 5.0).collect();
        append(&mut wood, tube(&points, &radii, 7));
    }
    wood
}

/// The canopy's clumps: each branch's tip and the crown, `(center, radius, cards)`.
fn tree_clumps() -> Vec<(Vec3, f32, u32)> {
    let mut clumps: Vec<(Vec3, f32, u32)> =
        BRANCHES.iter().map(|&shape| (branch(shape, shape.2) + Vec3::Y * 0.25, 0.9, 52)).collect();
    clumps.push((trunk(TRUNK_TOP) + Vec3::Y * 0.75, 1.15, 76));
    clumps
}

/// The leaf cards, scattered through each clump's ball, toward its surface, at every angle.
fn tree_leaves() -> Geometry {
    let mut leaves = Geometry::default();
    for (index, (center, radius, cards)) in tree_clumps().into_iter().enumerate() {
        let mut rng = Rng(0x7ee0 + index as u32);
        for _ in 0..cards {
            let at = center + in_ball(&mut rng, 0.35) * radius;
            let size = rng.range(0.55, 0.75);
            append(&mut leaves, card(&mut rng, at, size));
        }
    }
    leaves
}

/// The bush: a dome of leaf cards over the ground.
pub fn bush_glb() -> Vec<u8> {
    let mut leaves = Geometry::default();
    let mut rng = Rng(0xb054);
    for _ in 0..90 {
        let offset = in_ball(&mut rng, 0.4) * Vec3::new(0.85, 0.55, 0.85);
        // Its lowest cards dip into the ground, as a bush's leaves meet it.
        let at = Vec3::new(offset.x, 0.5 + offset.y.max(-0.32), offset.z);
        let size = rng.range(0.38, 0.5);
        append(&mut leaves, card(&mut rng, at, size));
    }
    plant_glb("Bush", &BUSH_SLOTS, vec![leaves])
}

/// The grass patch: upright cards at every heading, two rows of quads each, in a disc
/// [`GRASS_RADIUS`] meters across, sparser and shorter toward its edge, their feet a little below
/// the ground.
pub fn grass_glb() -> Vec<u8> {
    let mut blades = Geometry::default();
    let mut rng = Rng(0x9a55);
    for _ in 0..260 {
        let at = Vec2::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
        // Every candidate draws its numbers, kept or not, so the patch stays the same if the
        // falloff changes.
        let (keep, heading) = (rng.next(), rng.range(0.0, PI));
        let (width, height) = (rng.range(0.42, 0.6), rng.range(0.32, 0.5));
        let edge = smoothstep(0.45, 1.0, at.length());
        if keep < edge {
            continue;
        }
        let at = at * GRASS_RADIUS;
        let height = height * (1.0 - 0.45 * edge);
        let across = Vec3::new(heading.cos(), 0.0, heading.sin()) * (width * 0.5);
        let facing = Vec3::new(-heading.sin(), 0.0, heading.cos());
        let foot = Vec3::new(at.x, -0.03, at.y);
        let first = blades.positions.len() as u32;
        for row in 0..=2 {
            let v = row as f32 / 2.0;
            for side in [-1.0f32, 1.0] {
                let p = foot + across * side + Vec3::Y * (height * v);
                blades.positions.push(p.to_array());
                blades.normals.push(facing.to_array());
                // glTF's v runs down the image: the blades' tips are at its top.
                blades.uvs.push([(side + 1.0) * 0.5, 1.0 - v]);
                blades.colors.push([255; 4]);
            }
        }
        for row in 0..2 {
            let a = first + row * 2;
            // Counter-clockwise seen from the side `facing` points to.
            blades.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
        }
    }
    plant_glb("Grass", &GRASS_SLOTS, vec![blades])
}

/// A random point in the unit ball, pushed toward its surface by `bias` (0 uniform by radius,
/// 1 all on the surface).
fn in_ball(rng: &mut Rng, bias: f32) -> Vec3 {
    let direction = loop {
        let p = Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
        if (0.05..=1.0).contains(&p.length_squared()) {
            break p.normalize();
        }
    };
    let radius = rng.next().cbrt();
    direction * (radius + (1.0 - radius) * bias)
}

/// A square leaf card `size` across centered at `at`, facing a random way, the whole leaf map
/// on it.
fn card(rng: &mut Rng, at: Vec3, size: f32) -> Geometry {
    let normal = loop {
        let n = Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), rng.range(-1.0, 1.0));
        if (0.05..=1.0).contains(&n.length_squared()) {
            break n.normalize();
        }
    };
    let spin = rng.range(0.0, TAU);
    let helper = if normal.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u0 = normal.cross(helper).normalize();
    let v0 = normal.cross(u0);
    let (sin, cos) = spin.sin_cos();
    let (u, v) = (u0 * cos + v0 * sin, v0 * cos - u0 * sin);
    let half = size * 0.5;
    let corner = |s: f32, t: f32| at + (u * s + v * t) * half;
    Geometry {
        positions: [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .map(|(s, t)| corner(s, t).to_array())
            .to_vec(),
        normals: vec![normal.to_array(); 4],
        uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        colors: vec![[255; 4]; 4],
        // u × v is the normal: counter-clockwise seen from the front.
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Geometry::default()
    }
}

/// A tube along `points` with a radius per point, `segments` around, its end capped by a point;
/// normals from the ring around each point.
fn tube(points: &[Vec3], radii: &[f32], segments: u32) -> Geometry {
    let mut geometry = Geometry::default();
    let count = points.len();
    for (index, (&p, &radius)) in points.iter().zip(radii).enumerate() {
        let along = (points[(index + 1).min(count - 1)] - points[index.saturating_sub(1)]).normalize();
        let helper = if along.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let side = along.cross(helper).normalize();
        let other = along.cross(side);
        for segment in 0..=segments {
            let angle = TAU * segment as f32 / segments as f32;
            let normal = side * angle.cos() + other * angle.sin();
            geometry.positions.push((p + normal * radius).to_array());
            geometry.normals.push(normal.to_array());
            geometry.uvs.push([segment as f32 / segments as f32, index as f32 / (count - 1) as f32]);
            // Darker toward the foot of the trunk.
            let ao = 0.7 + 0.3 * smoothstep(0.0, 0.8, p.y);
            geometry.colors.push([unorm8(ao), 255, 255, 255]);
        }
    }
    let stride = segments + 1;
    for ring in 0..count as u32 - 1 {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let (b, c, d) = (a + 1, a + stride, a + stride + 1);
            // Counter-clockwise seen from outside: around the ring, then along the tube.
            geometry.indices.extend([a, b, c, b, d, c]);
        }
    }
    // The tip, closed by a point.
    let tip = geometry.positions.len() as u32;
    let end = points[count - 1];
    let along = (points[count - 1] - points[count - 2]).normalize();
    geometry.positions.push((end + along * radii[count - 1]).to_array());
    geometry.normals.push(along.to_array());
    geometry.uvs.push([0.5, 1.0]);
    geometry.colors.push([255; 4]);
    let last = (count as u32 - 1) * stride;
    for segment in 0..segments {
        geometry.indices.extend([last + segment, last + segment + 1, tip]);
    }
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

/// The leaf cluster: a rounded sprig of small pointed leaves, each pointing out from the
/// card's middle, light values with a darker midrib and a little shade toward the middle, their
/// coverage in alpha with soft edges, clear at the border. A canopy of them reads as soft leafy
/// masses rather than single leaves.
pub fn leaves_png() -> io::Result<Vec<u8>> {
    const SIZE: u32 = 256;
    // `(stem, direction, length, width)` of each leaf, as fractions of the card, in the order
    // they are painted (later ones on top).
    let mut rng = Rng(0x1ea7);
    let leaves: Vec<(Vec2, Vec2, f32, f32)> = (0..34)
        .map(|i| {
            let turns = i as f32 * 0.618_034 + rng.range(-0.05, 0.05);
            let out = Vec2::from_angle(turns * TAU);
            let stem = out * rng.range(0.0, 0.24);
            let direction = Vec2::from_angle(turns * TAU + rng.range(-0.5, 0.5));
            (stem, direction, rng.range(0.15, 0.21), rng.range(0.07, 0.1))
        })
        .collect();
    let texel = 1.0 / SIZE as f32;
    let image = image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * texel - Vec2::splat(0.5);
        let (mut coverage, mut value) = (0.0f32, 0.0f32);
        for &(stem, direction, length, width) in &leaves {
            // Along the leaf from its stem, and across it.
            let along = (p - stem).dot(direction);
            let across = (p - stem).perp_dot(direction);
            let t = along / length;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            // Widest a third of the way out, pointed at the tip.
            let half_width = width * (PI * t.powf(0.7)).sin() * 0.5;
            let inside = smoothstep(-1.5 * texel, 1.5 * texel, half_width - across.abs());
            // The leaf on top shows.
            if inside > 0.0 {
                let midrib = 1.0 - 0.18 * smoothstep(2.5 * texel, 0.0, across.abs());
                let shade = 0.8 + 0.2 * smoothstep(0.0, 0.3, p.length());
                let leaf = (0.82 + 0.18 * t) * midrib * shade;
                value = value + (leaf - value) * inside;
                coverage = coverage.max(inside);
            }
        }
        // Clear at the border, so repeating samples never bleed in.
        let border = smoothstep(0.5, 0.47, p.abs().max_element());
        let variation = 0.92 + 0.08 * fbm((p + 0.5) * 6.0, (6, 6), 2, 0x1ea8);
        let channel = unorm8(crate::color::linear_to_srgb(value * variation));
        image::Rgba([channel, channel, channel, unorm8(coverage * border)])
    });
    png(image)
}

/// The grass card: blades rising from its foot, tapering to their tips and curving a little,
/// darker at the foot, their coverage in alpha.
pub fn grass_png() -> io::Result<Vec<u8>> {
    const SIZE: u32 = 128;
    let mut rng = Rng(0x6a55);
    // `(foot across the card, height, width at the foot, lean)` as fractions of the card.
    let blades: Vec<(f32, f32, f32, f32)> = (0..7)
        .map(|i| {
            let foot = 0.12 + 0.76 * i as f32 / 6.0 + rng.range(-0.04, 0.04);
            (foot, rng.range(0.65, 0.97), rng.range(0.07, 0.1), rng.range(-0.12, 0.12))
        })
        .collect();
    let texel = 1.0 / SIZE as f32;
    let image = image::RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        let u = (x as f32 + 0.5) * texel;
        // Up from the card's foot.
        let h = 1.0 - (y as f32 + 0.5) * texel;
        let mut coverage = 0.0f32;
        for &(foot, height, width, lean) in &blades {
            let t = h / height;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let center = foot + lean * t * t;
            let half_width = 0.5 * width * (1.0 - t).powf(0.8);
            coverage = coverage.max(smoothstep(-1.2 * texel, 1.2 * texel, half_width - (u - center).abs()));
        }
        // Clear in the outer texels, so repeating samples never bleed in (its foot, which stands
        // in the ground, is covered).
        let border = smoothstep(0.5 - texel, 0.44, (u - 0.5).abs()) * smoothstep(1.0 - texel, 0.97, h);
        let value = 0.8 + 0.2 * smoothstep(0.0, 0.9, h);
        let channel = unorm8(crate::color::linear_to_srgb(value));
        image::Rgba([channel, channel, channel, unorm8(coverage * border)])
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use sapphire_assets::material::AnyMaterialDocument;
    use sapphire_assets::mesh::{FoliageVertex, PositionFormat, Submesh};

    use super::*;
    use crate::bake::foliage::{self, NormalTransfer};
    use crate::import;
    use crate::meta::{self, ImportSettings};

    /// A plant's material slots, by name and bound material.
    type Slots = &'static [(&'static str, AssetId)];

    fn sidecar(file: &str) -> meta::AssetMeta {
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples").join(DIR);
        meta::read(&meta::sidecar_path(&samples.join(file))).expect("a sidecar")
    }

    #[test]
    fn plants_import_with_their_slots_and_stand_on_their_roots() {
        let plants: [(Vec<u8>, Slots); 3] =
            [(tree_glb(), &TREE_SLOTS), (bush_glb(), &BUSH_SLOTS), (grass_glb(), &GRASS_SLOTS)];
        for (bytes, slots) in plants {
            let mesh = import::gltf::import_slice(&bytes, None).expect("import");
            let names: Vec<&str> = mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
            assert_eq!(names, slots.iter().map(|(name, _)| *name).collect::<Vec<_>>());
            let lowest = mesh.vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
            assert!((-0.2..=0.05).contains(&lowest), "rooted at the origin: {lowest}");
            let cooked =
                mesh.pack(PositionFormat::Unorm16, &BTreeMap::new()).unwrap().to_cooked().expect("cook");
            sapphire_assets::mesh::Mesh::read(&cooked).expect("structurally valid");
        }
        let tree = import::gltf::import_slice(&tree_glb(), None).expect("import");
        let top = tree.vertices.iter().map(|v| v.position[1]).fold(0.0, f32::max);
        assert!((5.0..=6.5).contains(&top), "{top} m tall");
    }

    #[test]
    fn the_wood_faces_outward() {
        let tree = import::gltf::import_slice(&tree_glb(), None).expect("import");
        let wood = &tree.lods[0].submeshes[0];
        let range = wood.first_index as usize..(wood.first_index + wood.index_count) as usize;
        for triangle in tree.indices[range].as_chunks::<3>().0 {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(tree.vertices[triangle[i] as usize].position));
            let face = (b - a).cross(c - a);
            let normals: Vec3 = triangle.iter().map(|&i| Vec3::from(tree.vertices[i as usize].normal)).sum();
            assert!(face.dot(normals) >= -1e-9, "{triangle:?} is wound against its normals");
        }
    }

    #[test]
    fn sidecars_bake_foliage_and_bind_the_materials() {
        let ids = [
            (TREE_FILE_NAME, TREE_ID),
            (BUSH_FILE_NAME, BUSH_ID),
            (GRASS_FILE_NAME, GRASS_ID),
            (BARK_MATERIAL_FILE_NAME, BARK_MATERIAL_ID),
            (LEAVES_MATERIAL_FILE_NAME, LEAVES_MATERIAL_ID),
            (BUSH_MATERIAL_FILE_NAME, BUSH_MATERIAL_ID),
            (GRASS_MATERIAL_FILE_NAME, GRASS_MATERIAL_ID),
            (LEAVES_COLOR_FILE_NAME, LEAVES_COLOR_ID),
            (GRASS_COLOR_FILE_NAME, GRASS_COLOR_ID),
        ];
        for (file, id) in ids {
            assert_eq!(sidecar(file).id, id, "{file}");
        }
        let plants: [(&str, Slots, NormalTransfer, &[&str]); 3] = [
            (TREE_FILE_NAME, &TREE_SLOTS, NormalTransfer::Hull { count: 6 }, &["Leaves"]),
            (BUSH_FILE_NAME, &BUSH_SLOTS, NormalTransfer::Sphere, &["Leaves"]),
            (GRASS_FILE_NAME, &GRASS_SLOTS, NormalTransfer::Up, &["Blades"]),
        ];
        for (file, slots, normals, leaf_slots) in plants {
            let ImportSettings::Gltf(settings) = sidecar(file).importer else { panic!("a glTF sidecar") };
            let bound: Vec<(&str, AssetId)> =
                settings.materials.iter().map(|(name, id)| (name.as_str(), *id)).collect();
            let mut expected = slots.to_vec();
            expected.sort_unstable();
            assert_eq!(bound, expected, "{file}: every slot bound to its material");
            let foliage = settings.foliage.as_ref().expect("a foliage bake");
            assert_eq!(foliage.normals, normals, "{file}");
            assert_eq!(foliage.slots, leaf_slots, "{file}");
            // No LODs: simplification would merge cards.
            let cooked = crate::cook::cook_mesh(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples").join(DIR).join(file),
                &settings,
                crate::cook::Cook::Desktop,
            )
            .expect("cooks");
            assert_eq!(cooked.lods.len(), 1, "{file}");
            assert_eq!(cooked.streams.foliage.len(), cooked.streams.len(), "{file}: a foliage stream");
        }
        for (_, bytes) in files().expect("files") {
            let Ok(text) = String::from_utf8(bytes) else { continue };
            let document = AnyMaterialDocument::from_ron(&text).expect("a material document");
            let textures = document.textures();
            assert!(
                textures.iter().flatten().all(|id| [LEAVES_COLOR_ID, GRASS_COLOR_ID].contains(id)),
                "{textures:?}"
            );
        }
    }

    #[test]
    fn the_tree_s_canopy_bakes_one_soft_volume_and_moves_with_its_wood() {
        let mut tree = import::gltf::import_slice(&tree_glb(), None).expect("import");
        let settings = foliage::FoliageSettings {
            normals: NormalTransfer::Hull { count: 6 },
            slots: vec!["Leaves".into()],
            blend: 0.8,
            canopy_from: 0.0,
        };
        foliage::bake(&mut tree, &settings, crate::bake::DEFAULT_WELD_TOLERANCE).expect("bakes");
        let canopy_center: Vec3 = tree_clumps().iter().map(|(center, ..)| *center).sum::<Vec3>() / 6.0;
        let leaves = &tree.lods[0].submeshes[1];
        let range_of = |submesh: &Submesh| {
            submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize
        };
        let range = range_of(leaves);
        let mut outward = 0;
        let mut total = 0;
        for &index in &tree.indices[range] {
            let vertex = &tree.vertices[index as usize];
            let p = Vec3::from(vertex.position);
            total += 1;
            if Vec3::from(vertex.normal).dot((p - canopy_center).normalize()) > 0.0 {
                outward += 1;
            }
            let data: FoliageVertex = tree.foliage[index as usize];
            assert_eq!(data.flutter, 1.0, "leaves flutter");
        }
        assert!(outward * 10 > total * 9, "the canopy's normals face out of it: {outward} of {total}");
        // The trunk's foot holds still: no sway, bend or flutter, and its ring moves as one (one
        // phase), so it neither slides on the ground nor squashes.
        let foot: Vec<FoliageVertex> = tree
            .vertices
            .iter()
            .zip(&tree.foliage)
            .filter(|(v, _)| v.position[1] < 0.05)
            .map(|(_, data)| *data)
            .collect();
        assert!(foot.len() >= 3, "the foot ring");
        assert!(
            foot.iter().all(|data| data.sway < 0.01 && data.bend < 0.01 && data.flutter == 0.0),
            "{foot:?}"
        );
        assert!(foot.iter().all(|data| data.phase == foot[0].phase), "the foot ring is rigid");
        // The whole trunk shares that phase, and every clump's cards share one: the crown sways
        // as one instead of breathing.
        let wood = &tree.lods[0].submeshes[0];
        let wood = &tree.indices[wood.first_index as usize..(wood.first_index + wood.index_count) as usize];
        let around_axis =
            wood.iter().filter(|&&i| { let [x, _, z] = tree.vertices[i as usize].position; Vec2::new(x, z).length() } < 0.4);
        let low_trunk: Vec<f32> = around_axis
            .filter(|&&i| tree.vertices[i as usize].position[1] < 1.0)
            .map(|&i| tree.foliage[i as usize].phase)
            .collect();
        assert!(low_trunk.iter().all(|&phase| phase == foot[0].phase), "the trunk is rigid in phase");
        let phases: std::collections::BTreeSet<u32> = tree.indices[range_of(leaves)]
            .iter()
            .map(|&i| tree.foliage[i as usize].phase.to_bits())
            .collect();
        assert!(phases.len() <= 6, "one phase a clump: {} phases", phases.len());
    }

    #[test]
    fn card_maps_are_clear_at_their_borders_and_covered_inside() {
        // The leaf map is clear all round; grass blades stand on the card's bottom edge.
        for (bytes, size, bottom) in [(leaves_png().unwrap(), 256, true), (grass_png().unwrap(), 128, false)]
        {
            let image = image::load_from_memory(&bytes).expect("a PNG").to_rgba8();
            assert_eq!(image.dimensions(), (size, size));
            let last = size - 1;
            for i in 0..size {
                assert_eq!(image.get_pixel(i, 0)[3], 0, "the top edge is clear");
                assert_eq!(image.get_pixel(0, i)[3], 0, "the left edge is clear");
                assert_eq!(image.get_pixel(last, i)[3], 0, "the right edge is clear");
                if bottom {
                    assert_eq!(image.get_pixel(i, last)[3], 0, "the bottom edge is clear");
                }
            }
            let covered = image.pixels().filter(|p| p[3] > 128).count() as f32 / (size * size) as f32;
            assert!((0.15..0.6).contains(&covered), "{covered} of the card covered");
        }
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(tree_glb(), tree_glb());
        assert_eq!(bush_glb(), bush_glb());
        assert_eq!(grass_glb(), grass_glb());
        assert_eq!(leaves_png().unwrap(), leaves_png().unwrap());
        assert_eq!(grass_png().unwrap(), grass_png().unwrap());
    }
}
