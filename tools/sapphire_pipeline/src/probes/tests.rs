use std::fs;

use glam::{Vec2, Vec3};
use sapphire_assets::material::{EnvironmentMaterial, FogCardMaterial, FoliageMaterial};
use sapphire_assets::probes::{
    BRICK_CELLS, BRICK_PROBES, BRICK_SIDE, EMPTY_BRICK, MAX_BRICKS_PER_AXIS, ProbeData,
};
use sapphire_math::sh;
use serde_json::json;

use super::bake::{BakeKey, BakeMaterial, BakeScene, BakeSettings, BakeTriangle, Cutout, bake, probes_of};
use super::*;
use crate::cook::{CookOptions, CookStatus, cook_dir};
use crate::gltf_writer::{Geometry, GltfWriter};
use crate::meta::{AssetMeta, sidecar_path};
use crate::test_util::TempDir;
use crate::texture::{ColorSettings, HdrSettings, TextureClass, TextureSettings};

/// Two triangles of the rectangle from `corner` along `u` and `v`, wound to face `u × v`, its
/// texture coordinates running 0 to 1 along each.
fn quad(corner: Vec3, u: Vec3, v: Vec3, material: u32) -> [BakeTriangle; 2] {
    let normal = u.cross(v).normalize();
    let (a, b, c, d) = (corner, corner + u, corner + v, corner + u + v);
    let (ta, tb, tc, td) = (Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE);
    let triangle = |positions, uvs| BakeTriangle { positions, normals: [normal; 3], uvs, material };
    [triangle([a, b, d], [ta, tb, td]), triangle([a, d, c], [ta, td, tc])]
}

/// A 16 m floor at y = 0 (material 0), and a wall 4 m tall and 10 cm thick across x = 0 from
/// z = −4 to 4 (material 1): a thin wall between a lit and a shaded side.
fn floor_and_wall() -> BakeScene {
    let mut triangles = Vec::new();
    triangles.extend(quad(
        Vec3::new(-8.0, 0.0, -8.0),
        Vec3::new(0.0, 0.0, 16.0),
        Vec3::new(16.0, 0.0, 0.0),
        0,
    ));
    // Facing +x, then facing −x.
    triangles.extend(quad(Vec3::new(0.05, 0.0, -4.0), Vec3::new(0.0, 4.0, 0.0), Vec3::new(0.0, 0.0, 8.0), 1));
    triangles.extend(quad(
        Vec3::new(-0.05, 0.0, -4.0),
        Vec3::new(0.0, 0.0, 8.0),
        Vec3::new(0.0, 4.0, 0.0),
        1,
    ));
    BakeScene {
        triangles,
        materials: vec![
            BakeMaterial { albedo: Vec3::new(0.8, 0.2, 0.1), ..BakeMaterial::default() },
            BakeMaterial { albedo: Vec3::splat(0.5), ..BakeMaterial::default() },
        ],
        cutouts: Vec::new(),
    }
}

/// A sun from +x and above, so the wall's +x side is lit and its −x side shaded, under a grey sky.
fn sunny_key(hours: f32) -> BakeKey {
    let sky = [0.5, 0.6, 0.8].map(|value| {
        let mut channel = [0.0; sh::L2];
        channel[0] = sh::project_constant(value)[0];
        channel
    });
    BakeKey {
        hours,
        to_light: Vec3::new(1.0, 1.0, 0.2).normalize(),
        radiance: Vec3::ONE,
        shadow_tint: Vec3::splat(0.2),
        sky,
        emission: 1.0,
    }
}

fn settings() -> BakeSettings {
    BakeSettings {
        min: Vec3::new(-6.0, -0.5, -6.0),
        max: Vec3::new(6.0, 5.5, 6.0),
        spacing: 1.0,
        visibility_coefficients: 9,
        probe_rays: 128,
        surfel_rays: 64,
        surfel_spacing: 1.0,
        bounces: 2,
        bounce_strength: 1.0,
    }
}

/// The probe at grid coordinates `at`, and its bounce for key 0, as RGB L1.
fn probe(grid: &ProbeGrid, at: [u32; 3]) -> Option<usize> {
    let brick = at.map(|c| c / 4);
    let local = [0, 1, 2].map(|axis| at[axis] - brick[axis] * 4);
    let bricks = grid.bricks;
    let entry = grid.brick_table[(brick[0] + bricks[0] * (brick[1] + bricks[1] * brick[2])) as usize];
    (entry != EMPTY_BRICK).then(|| {
        grid.brick_probes[(entry * BRICK_PROBES + local[0] + 5 * (local[1] + 5 * local[2])) as usize] as usize
    })
}

fn visibility(grid: &ProbeGrid, probe: usize) -> Vec<f32> {
    let n = grid.visibility_coefficients as usize;
    grid.visibility[probe * n..(probe + 1) * n]
        .iter()
        .map(|&bits| half::f16::from_bits(bits).to_f32())
        .collect()
}

/// Key `key`'s bounce at `probe`, as irradiance from direction `d` per channel.
fn bounce(grid: &ProbeGrid, probe: usize, key: usize, d: Vec3) -> Vec3 {
    let keys = grid.key_hours.len();
    let start = (probe * keys + key) * 12;
    let coefficients: Vec<f32> =
        grid.bounce[start..start + 12].iter().map(|&bits| half::f16::from_bits(bits).to_f32()).collect();
    Vec3::from_array([0, 1, 2].map(|channel| sh::evaluate(&coefficients[channel * 4..channel * 4 + 4], d)))
}

#[test]
fn bakes_are_deterministic() {
    let scene = floor_and_wall();
    let keys = [sunny_key(6.0), sunny_key(18.0)];
    let first = bake(&scene, &keys, &settings()).unwrap().to_cooked().unwrap().into_vec();
    let second = bake(&scene, &keys, &settings()).unwrap().to_cooked().unwrap().into_vec();
    assert_eq!(first, second, "same input, same bytes");
}

#[test]
fn bricks_are_kept_around_geometry_only() {
    let grid = bake(&floor_and_wall(), &[sunny_key(10.0)], &settings()).unwrap();
    // 12 m by 6 m by 12 m at 4 m bricks: 3 × 2 × 3 positions.
    assert_eq!(grid.bricks, [3, 2, 3]);
    let kept = |x: u32, y: u32, z: u32| grid.brick_table[(x + 3 * (y + 2 * z)) as usize] != EMPTY_BRICK;
    // The floor's layer is kept everywhere; above it, only around the wall (x = 0 lies in the
    // middle column), whose top at 4 m reaches into the upper layer grown by a cell.
    for x in 0..3 {
        for z in 0..3 {
            assert!(kept(x, 0, z), "the floor's bricks");
            assert_eq!(kept(x, 1, z), x == 1, "upper brick ({x}, 1, {z})");
        }
    }
    let kept_count = grid.brick_table.iter().filter(|&&entry| entry != EMPTY_BRICK).count();
    assert_eq!(grid.brick_probes.len(), kept_count * BRICK_PROBES as usize);
    // Shared probes are stored once.
    let mut unique = grid.brick_probes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), grid.probe_data.len());
}

/// [`probes_of`] by an entry for every coordinate of the grid, as the bake's layout once was.
fn probes_by_every_coordinate(bricks: [u32; 3], kept: &[[u32; 3]]) -> (Vec<[u32; 3]>, Vec<u32>) {
    let side = bricks.map(|n| n * BRICK_CELLS + 1);
    let linear = |[x, y, z]: [u32; 3]| (x + side[0] * (y + side[1] * z)) as usize;
    let in_brick = |[bx, by, bz]: [u32; 3]| {
        (0..BRICK_PROBES).map(move |i| {
            let local = [i % BRICK_SIDE, (i / BRICK_SIDE) % BRICK_SIDE, i / (BRICK_SIDE * BRICK_SIDE)];
            [bx * BRICK_CELLS + local[0], by * BRICK_CELLS + local[1], bz * BRICK_CELLS + local[2]]
        })
    };
    let mut index = vec![u32::MAX; side.iter().map(|&n| n as usize).product()];
    for &brick in kept {
        for probe in in_brick(brick) {
            index[linear(probe)] = 0;
        }
    }
    let mut probes = Vec::new();
    for z in 0..side[2] {
        for y in 0..side[1] {
            for x in 0..side[0] {
                if index[linear([x, y, z])] == 0 {
                    index[linear([x, y, z])] = probes.len() as u32;
                    probes.push([x, y, z]);
                }
            }
        }
    }
    let brick_probes = kept.iter().flat_map(|&brick| in_brick(brick)).map(|g| index[linear(g)]).collect();
    (probes, brick_probes)
}

/// A grid's probes keep their order and indices whichever bricks it keeps, so its bytes do.
#[test]
fn probes_are_listed_in_grid_order_whichever_bricks_are_kept() {
    let mut noise = 0x2545_f491_u32;
    let mut next = move || {
        noise ^= noise << 13;
        noise ^= noise >> 17;
        noise ^= noise << 5;
        noise
    };
    for bricks in [[1, 1, 1], [2, 1, 1], [1, 3, 2], [8, 2, 8], [5, 7, 3], [16, 4, 9]] {
        // One in 16, one in 2, then all of them, kept in the bake's order: x fastest.
        for one_in in [16, 2, 1] {
            let mut kept = Vec::new();
            for z in 0..bricks[2] {
                for y in 0..bricks[1] {
                    for x in 0..bricks[0] {
                        if next().is_multiple_of(one_in) {
                            kept.push([x, y, z]);
                        }
                    }
                }
            }
            if kept.is_empty() {
                kept.push([0, 0, 0]);
            }
            assert_eq!(
                probes_of(bricks, &kept),
                probes_by_every_coordinate(bricks, &kept),
                "{bricks:?}, {one_in}"
            );
        }
    }
}

/// What the layout holds follows the bricks kept, not the box: an entry for every coordinate of this
/// one is 4 GiB, and it holds three bricks.
#[test]
fn the_largest_box_costs_what_it_keeps() {
    let (bricks, last) = ([MAX_BRICKS_PER_AXIS; 3], MAX_BRICKS_PER_AXIS - 1);
    let (probes, brick_probes) = probes_of(bricks, &[[0, 0, 0], [128, 0, 77], [last, last, last]]);
    let count = 3 * BRICK_PROBES as usize;
    assert_eq!((probes.len(), brick_probes.len()), (count, count));
    assert_eq!(probes[0], [0, 0, 0]);
    assert_eq!(probes[count - 1], [MAX_BRICKS_PER_AXIS * BRICK_CELLS; 3]);
    assert!(
        probes
            .windows(2)
            .all(|pair| (pair[0][2], pair[0][1], pair[0][0]) < (pair[1][2], pair[1][1], pair[1][0]))
    );
    assert_eq!(probes[brick_probes[0] as usize], [0, 0, 0]);
}

#[test]
fn probes_see_the_sky_bounce_and_walls() {
    let grid = bake(&floor_and_wall(), &[sunny_key(10.0)], &settings()).unwrap();
    // Probes stand at -6 + i m along x and z, and -0.5 + j m along y: (4, 2, 6) is at (-2, 1.5, 0),
    // 2 m from the wall's shaded side; (8, 2, 6) at (2, 1.5, 0) on its lit side.
    let shaded = probe(&grid, [4, 2, 6]).unwrap();
    let lit = probe(&grid, [8, 2, 6]).unwrap();
    for at in [shaded, lit] {
        let open = visibility(&grid, at);
        // Over a floor: the sky above is open, the ground below isn't.
        assert!(sh::evaluate(&open, Vec3::Y) > 0.8, "{open:?}");
        assert!(sh::evaluate(&open, Vec3::NEG_Y) < 0.2, "{open:?}");
        assert!(ProbeData::from_bits(grid.probe_data[at]).valid);
        // The red floor's bounce from below is red.
        let below = bounce(&grid, at, 0, Vec3::NEG_Y);
        assert!(below.x > 2.0 * below.y && below.x > 2.0 * below.z, "red bounce: {below}");
    }
    // The lit side's floor is in the sun, the shaded side's isn't: its bounce is far brighter.
    let (lit_below, shaded_below) =
        (bounce(&grid, lit, 0, Vec3::NEG_Y), bounce(&grid, shaded, 0, Vec3::NEG_Y));
    assert!(lit_below.x > 1.5 * shaded_below.x, "{lit_below} vs {shaded_below}");
    // The wall blocks the sky toward it: less of the sky is open toward +x from the shaded side.
    let toward_wall = sh::evaluate(&visibility(&grid, shaded), Vec3::new(1.0, 0.3, 0.0).normalize());
    let away = sh::evaluate(&visibility(&grid, shaded), Vec3::new(-1.0, 0.3, 0.0).normalize());
    assert!(toward_wall < away - 0.2, "{toward_wall} vs {away}");
    // A probe 0.5 m below the floor is inside: invalid, and filled from above.
    let under = probe(&grid, [4, 0, 2]).unwrap();
    let data = ProbeData::from_bits(grid.probe_data[under]);
    assert!(!data.valid);
    assert!(sh::evaluate(&visibility(&grid, under), Vec3::Y) > 0.5, "dilated from its open neighbors");
}

#[test]
fn free_distances_find_thin_walls() {
    let mut settings = settings();
    // Probes at x = -0.5 and 0.5: half a spacing either side of the wall.
    settings.min.x = -6.5;
    let grid = bake(&floor_and_wall(), &[sunny_key(10.0)], &settings).unwrap();
    let left = ProbeData::from_bits(grid.probe_data[probe(&grid, [6, 2, 6]).unwrap()]);
    let right = ProbeData::from_bits(grid.probe_data[probe(&grid, [7, 2, 6]).unwrap()]);
    // +x, −x: the wall's faces are 0.45 m from each, toward the other.
    assert!((13..=14).contains(&left.free[0]), "{left:?}");
    assert_eq!(left.free[1], ProbeData::FULL);
    assert!((13..=14).contains(&right.free[1]), "{right:?}");
    assert_eq!(right.free[0], ProbeData::FULL);
    // Down: the floor 1.5 m below is past a spacing; up: open.
    assert_eq!((left.free[2], left.free[3]), (ProbeData::FULL, ProbeData::FULL));
}

#[test]
fn bounce_follows_the_keys_and_its_strength() {
    let mut dark = sunny_key(0.0);
    dark.radiance = Vec3::ZERO;
    dark.sky = [[0.0; sh::L2]; 3];
    let keys = [dark, sunny_key(10.0)];
    let grid = bake(&floor_and_wall(), &keys, &settings()).unwrap();
    assert_eq!(grid.key_hours, [0.0, 10.0]);
    let at = probe(&grid, [8, 2, 6]).unwrap();
    assert_eq!(bounce(&grid, at, 0, Vec3::NEG_Y), Vec3::ZERO, "no light, no bounce");
    let day = bounce(&grid, at, 1, Vec3::NEG_Y);
    let half_strength =
        bake(&floor_and_wall(), &keys, &BakeSettings { bounce_strength: 0.5, ..settings() }).unwrap();
    let halved = bounce(&half_strength, at, 1, Vec3::NEG_Y);
    assert!((halved - day * 0.5).abs().max_element() < 0.01 * day.max_element(), "{halved} vs {day}");
    // L1 visibility stores 4 coefficients.
    let l1 =
        bake(&floor_and_wall(), &keys, &BakeSettings { visibility_coefficients: 4, ..settings() }).unwrap();
    assert_eq!(l1.visibility.len(), l1.probe_data.len() * 4);
}

#[test]
fn emission_bounces_as_each_key_shows_it() {
    // Keys without sun or sky, so what reaches a probe is the floor's glow alone: none of the
    // authored emission, half and all of it (M1 W1's keyed emission).
    let mut dark = sunny_key(0.0);
    dark.radiance = Vec3::ZERO;
    dark.sky = [[0.0; sh::L2]; 3];
    let keys = [0.0, 0.5, 1.0].map(|emission| BakeKey { hours: 8.0 * emission, emission, ..dark });
    let glowing = |steady: f32| {
        let mut scene = floor_and_wall();
        scene.materials[0].emission = Vec3::new(0.6, 0.3, 0.1);
        scene.materials[0].emission_steady = steady;
        bake(&scene, &keys, &settings()).unwrap()
    };
    let (keyed, steady) = (glowing(0.0), glowing(1.0));
    let at = probe(&keyed, [8, 2, 6]).unwrap();
    let glow = |grid: &ProbeGrid, key: usize| bounce(grid, at, key, Vec3::NEG_Y);
    assert_eq!(glow(&keyed, 0), Vec3::ZERO, "dark: no glow, no bounce");
    let (half, full) = (glow(&keyed, 1), glow(&keyed, 2));
    assert!(full.x > 0.1, "the floor's glow: {full}");
    assert!((half - full * 0.5).abs().max_element() < 0.01 * full.max_element(), "{half} vs {full}");
    for key in 0..keys.len() {
        assert_eq!(glow(&steady, key), full, "key {key}: a steady glow lights every key alike");
    }
}

#[test]
fn bad_settings_are_refused() {
    let scene = floor_and_wall();
    let keys = [sunny_key(10.0)];
    let refuse = |edit: &dyn Fn(&mut BakeSettings)| {
        let mut settings = settings();
        edit(&mut settings);
        bake(&scene, &keys, &settings).is_err()
    };
    assert!(refuse(&|s| s.spacing = 0.0));
    assert!(refuse(&|s| s.max = s.min));
    assert!(refuse(&|s| s.visibility_coefficients = 3));
    assert!(refuse(&|s| s.bounces = 0));
    assert!(refuse(&|s| s.spacing = 0.01), "too many bricks");
    // Nothing in the box: no bricks.
    assert!(refuse(&|s| {
        s.min = Vec3::splat(100.0);
        s.max = Vec3::splat(110.0);
    }));
}

/// What stands across the floor of [`write_tree`]'s court.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wall {
    /// [`floor_and_wall`]'s wall, in a grey environment material.
    Stone,
    /// The same wall bound to a fog card material (M1 W3).
    FogCard,
    /// The same wall bound to an alpha-tested foliage material (M1 W6) whose card map is green at
    /// this alpha everywhere.
    Leaves(u8),
    /// None: the floor alone.
    Open,
}

/// The floor and `wall` as a glTF, its slots bound to material documents by the sidecar.
fn write_tree(dir: &Path, floor_color: [f32; 4], wall: Wall) {
    let geometry = |triangles: &[BakeTriangle]| {
        let mut geometry = Geometry::default();
        for triangle in triangles {
            for (position, normal) in triangle.positions.iter().zip(triangle.normals) {
                geometry.indices.push(geometry.positions.len() as u32);
                geometry.positions.push(position.to_array());
                geometry.normals.push(normal.to_array());
                geometry.uvs.push([0.0, 0.0]);
                geometry.colors.push([255; 4]);
            }
        }
        geometry
    };
    let scene = floor_and_wall();
    let mut gltf = GltfWriter::default();
    let floor = geometry(&scene.triangles[..2]);
    let wall_geometry = geometry(&scene.triangles[2..]);
    let floor_material = gltf.add_material(json!({ "name": "Floor" }));
    let wall_material = gltf.add_material(json!({ "name": "Wall" }));
    let mesh = if wall == Wall::Open {
        gltf.add_mesh("Court", &[(&floor, Some(floor_material))])
    } else {
        gltf.add_mesh("Court", &[(&floor, Some(floor_material)), (&wall_geometry, Some(wall_material))])
    };
    let node = gltf.add_node(json!({ "name": "Court", "mesh": mesh }));
    gltf.add_root(node);
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("court.glb"), gltf.to_glb()).unwrap();
    let ids = [
        ("court.glb", "00000000-0000-4000-8000-000000000001"),
        ("floor.material.ron", "00000000-0000-4000-8000-000000000002"),
        ("wall.material.ron", "00000000-0000-4000-8000-000000000003"),
        ("court.probes.ron", "00000000-0000-4000-8000-000000000004"),
        ("leaves.png", "00000000-0000-4000-8000-000000000005"),
    ];
    let id = |text: &str| AssetId::from_text(text);
    let mut mesh_settings = crate::meta::MeshSettings::default();
    mesh_settings.materials.insert("Floor".into(), id(ids[1].1));
    if wall != Wall::Open {
        mesh_settings.materials.insert("Wall".into(), id(ids[2].1));
    }
    let importers = [
        ImportSettings::Gltf(mesh_settings),
        ImportSettings::Material(Default::default()),
        ImportSettings::Material(Default::default()),
        ImportSettings::ProbeGrid(Default::default()),
        ImportSettings::Texture(TextureSettings { class: TextureClass::Color(ColorSettings::default()) }),
    ];
    for ((file, text), importer) in ids.iter().zip(importers) {
        let meta = AssetMeta::new(id(text), importer);
        fs::write(sidecar_path(&dir.join(file)), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
    }
    let material = |color: [f32; 4]| {
        let material = EnvironmentMaterial { base_color: color, ..EnvironmentMaterial::default() };
        sapphire_serialize::to_ron(&material).unwrap()
    };
    fs::write(dir.join("floor.material.ron"), material(floor_color)).unwrap();
    let alpha = if let Wall::Leaves(alpha) = wall { alpha } else { 255 };
    let leaves = ::image::RgbaImage::from_pixel(16, 16, ::image::Rgba([40, 200, 40, alpha]));
    leaves.save_with_format(dir.join("leaves.png"), ::image::ImageFormat::Png).unwrap();
    let wall_material = match wall {
        Wall::FogCard => sapphire_serialize::to_ron(&FogCardMaterial::default()).unwrap(),
        Wall::Leaves(_) => {
            let leaves = FoliageMaterial {
                base_color_map: Some(id(ids[4].1)),
                alpha_cutoff: 0.5,
                ..FoliageMaterial::default()
            };
            sapphire_serialize::to_ron(&leaves).unwrap()
        }
        Wall::Stone | Wall::Open => material([0.5, 0.5, 0.5, 1.0]),
    };
    fs::write(dir.join("wall.material.ron"), wall_material).unwrap();
    let document = ProbeGridDocument {
        bounds: BakeBounds { min: [-6.0, -0.5, -6.0], max: [6.0, 5.5, 6.0] },
        spacing: PerCook { desktop: 2.0, mobile: 4.0 },
        visibility: PerCook { desktop: ShOrder::L2, mobile: ShOrder::L1 },
        rays: PerCook { desktop: 64, mobile: 32 },
        surfel_rays: 32,
        surfel_spacing: 2.0,
        bounces: 1,
        bounce_strength: 1.0,
        timeline: BakeTimeline::DefaultDay,
        meshes: vec![BakeMesh { mesh: id(ids[0].1), translation: [0.0; 3], yaw: 0.0 }],
    };
    fs::write(dir.join("court.probes.ron"), sapphire_serialize::to_ron(&document).unwrap()).unwrap();
}

#[test]
fn grids_cook_through_the_cache_and_follow_their_materials() {
    let scratch = TempDir::new("probe_cook");
    let source = scratch.path().join("content");
    write_tree(&source, [0.8, 0.2, 0.1, 1.0], Wall::Stone);
    let cache = scratch.path().join("cache");
    let cook = |target: Cook| {
        let out = scratch.path().join("out").join(target.name());
        let options = CookOptions { cook: target, cache: Some(cache.as_path()), locked: true, editor: false };
        (cook_dir(&source, &out, &options).unwrap(), out)
    };
    let (report, out) = cook(Cook::Desktop);
    let grid_asset =
        report.assets.iter().find(|asset| asset.output.ends_with("court.sprobe")).expect("baked");
    assert_eq!(grid_asset.status, CookStatus::Cooked);
    let bytes = crate::test_util::read_aligned(&out.join("court.sprobe"));
    let grid = ProbeGrid::read(&bytes).expect("a valid grid");
    let default_day = timeline(&BakeTimeline::DefaultDay, Path::new("")).expect("the default day");
    assert_eq!(grid.key_hours().count(), default_day.keys().len(), "a bake per key of the day");
    assert_eq!(grid.visibility_coefficients.to_native(), 9, "L2 on desktop");
    // The red channel's mean bounce over every probe and key.
    let red_bounce = |grid: &sapphire_assets::probes::ArchivedProbeGrid| {
        grid.bounce
            .iter()
            .step_by(12)
            .map(|bits| half::f16::from_bits(bits.to_native()).to_f32())
            .sum::<f32>()
    };
    let red = red_bounce(grid);

    // Unchanged: from the cache, not rewritten.
    let (again, _) = cook(Cook::Desktop);
    assert!(again.assets.iter().all(|asset| asset.status == CookStatus::UpToDate));

    // The mobile cook: coarser, L1.
    let (_, mobile_out) = cook(Cook::Mobile);
    let mobile = crate::test_util::read_aligned(&mobile_out.join("court.sprobe"));
    let mobile = ProbeGrid::read(&mobile).unwrap();
    assert_eq!(mobile.visibility_coefficients.to_native(), 4);
    assert!(mobile.probe_count() < grid.probe_count());

    // A material the grid sees changed: the grid bakes again, with the new color's bounce.
    write_tree(&source, [0.1, 0.1, 0.1, 1.0], Wall::Stone);
    let (changed, _) = cook(Cook::Desktop);
    let rebaked = changed.assets.iter().find(|asset| asset.output.ends_with("court.sprobe")).unwrap();
    assert_eq!(rebaked.status, CookStatus::Cooked);
    let bytes = crate::test_util::read_aligned(&out.join("court.sprobe"));
    let dark = ProbeGrid::read(&bytes).unwrap();
    let dark = red_bounce(dark);
    assert!(dark < 0.5 * red, "a darker floor bounces less red: {dark} vs {red}");
}

#[test]
fn fog_cards_are_left_out_of_the_bake() {
    // Fog cards (M1 W3) are transparent fog, neither occluding the sky nor bouncing light: a wall
    // bound to one bakes as no wall at all, where a stone one shades the grid.
    let scratch = TempDir::new("probe_fog_cards");
    let bake = |wall: Wall, name: &str| {
        let source = scratch.path().join(name).join("content");
        write_tree(&source, [0.8, 0.2, 0.1, 1.0], wall);
        let out = scratch.path().join(name).join("out");
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
        cook_dir(&source, &out, &options).unwrap();
        fs::read(out.join("court.sprobe")).unwrap()
    };
    let card = bake(Wall::FogCard, "card");
    assert!(card == bake(Wall::Open, "open"), "a fog card's wall bakes as none");
    assert!(card != bake(Wall::Stone, "stone"), "a stone wall shades the grid");
}

#[test]
fn the_cache_key_follows_the_resolved_keys() {
    // The keys as resolved, by their bits: the same day gives the same hash, and any change to
    // a key's light (here one sky coefficient by a hair) another.
    let day = keys(&BakeTimeline::DefaultDay, Path::new("")).expect("the default day");
    assert_eq!(keys_hash(&day), keys_hash(&keys(&BakeTimeline::DefaultDay, Path::new("")).unwrap()));
    let mut moved = day.clone();
    moved[2].sky[1][4] = f32::from_bits(moved[2].sky[1][4].to_bits() + 1);
    assert_ne!(keys_hash(&day), keys_hash(&moved));
    // The default day shows emission as authored at every key; a keyed scale bakes again.
    assert!(day.iter().all(|key| key.emission == 1.0));
    let mut dimmed = day.clone();
    dimmed[3].emission = 0.1;
    assert_ne!(keys_hash(&day), keys_hash(&dimmed));
}

#[test]
fn the_default_day_is_the_runtime_s_document() {
    // The file the runtime compiles in, read where it lies (`sapphire_hot_reload`'s tests hold
    // the file to what `sapphire_world` compiles in): the base timeline, a bake key per key.
    assert!(default_day_path().ends_with("crates/sapphire_world/default_day.ron"));
    let day = timeline(&BakeTimeline::DefaultDay, Path::new("")).expect("the default day");
    assert!(day.is_complete(), "a base timeline");
    assert_eq!(keys(&BakeTimeline::DefaultDay, Path::new("")).unwrap().len(), day.keys().len());
}

/// [`floor_and_wall`]'s floor without the wall, and a 2 m square card 0.5 m over the probe at
/// grid (6, 2, 6) (at (0, 1.5, 0)), facing up, green, cut out by `coverage`: two texels across
/// its texture's u, which runs along z (the first over z < 0), against a cutoff of 0.5. Without
/// `coverage`, no card.
fn card_scene(coverage: Option<[f32; 2]>, two_sided: bool) -> BakeScene {
    let mut scene = floor_and_wall();
    scene.triangles.truncate(2);
    scene.materials.truncate(1);
    if let Some(coverage) = coverage {
        let (u, v) = (Vec3::new(0.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 0.0));
        scene.triangles.extend(quad(Vec3::new(-1.0, 2.0, -1.0), u, v, 1));
        scene.materials.push(BakeMaterial {
            albedo: Vec3::new(0.1, 0.8, 0.1),
            two_sided,
            cutout: Some(0),
            ..BakeMaterial::default()
        });
        scene.cutouts.push(Cutout { width: 2, height: 1, coverage: coverage.to_vec(), cutoff: 0.5 });
    }
    scene
}

#[test]
fn cards_hide_the_sky_and_bounce_as_their_leaves() {
    // Foliage's alpha-tested cards (M1 W6) bake as the renderer draws them: a ray passes where a
    // card's coverage is under its cutoff, so a card cut out whole bakes as none and half a card
    // shades half the sky; the leaves it keeps bounce their color; and a card is two-sided, so a
    // probe beneath one isn't inside geometry, and no wall to the leak test.
    let bake = |scene: &BakeScene| bake(scene, &[sunny_key(10.0)], &settings()).unwrap();
    let at = [6, 2, 6];
    let sky =
        |grid: &ProbeGrid, d: Vec3| sh::evaluate(&visibility(grid, probe(grid, at).unwrap()), d.normalize());
    let data = |grid: &ProbeGrid| ProbeData::from_bits(grid.probe_data[probe(grid, at).unwrap()]);
    let open = bake(&card_scene(None, true));
    let clear = bake(&card_scene(Some([0.0, 0.0]), true));
    let solid = bake(&card_scene(Some([1.0, 1.0]), true));
    let half = bake(&card_scene(Some([1.0, 0.0]), true));
    assert!((sky(&clear, Vec3::Y) - sky(&open, Vec3::Y)).abs() < 1e-3, "a clear card hides nothing");
    assert!(sky(&solid, Vec3::Y) < sky(&open, Vec3::Y) - 0.3, "a solid card hides the sky above");
    let (covered, cut) = (sky(&half, Vec3::new(0.0, 1.0, -0.7)), sky(&half, Vec3::new(0.0, 1.0, 0.7)));
    assert!(covered < cut - 0.1, "half a card shades its half: {covered} against {cut}");
    // The leaves bounce green from above; the clear card bounces nothing.
    let above = |grid: &ProbeGrid| bounce(grid, probe(grid, at).unwrap(), 0, Vec3::Y);
    assert!(above(&solid).y > 2.0 * above(&solid).x, "green leaves: {}", above(&solid));
    assert!((above(&clear) - above(&open)).abs().max_element() < 1e-3);
    // Two-sided: valid beneath the card, which the leak test sees through; one-sided, the same
    // card's back is the inside of geometry to the probe, and a wall half a meter up.
    assert!(data(&solid).valid);
    assert_eq!(data(&solid).free[2], ProbeData::FULL, "up, past the leaves");
    let one_sided = bake(&card_scene(Some([1.0, 1.0]), false));
    assert!(!data(&one_sided).valid, "a one-sided card's back");
    assert!(data(&one_sided).free[2] < ProbeData::FULL / 2 + 1, "{:?}", data(&one_sided));
}

/// `image` in `format`, as a file's bytes.
fn encoded(image: ::image::DynamicImage, format: ::image::ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

/// An image of one float color, as an EXR's bytes.
fn exr(value: f32) -> Vec<u8> {
    let image = ::image::Rgba32FImage::from_pixel(4, 4, ::image::Rgba([value, value, value, 1.0]));
    encoded(::image::DynamicImage::ImageRgba32F(image), ::image::ImageFormat::OpenExr)
}

fn hdr() -> TextureClass {
    TextureClass::Hdr(HdrSettings::default())
}

fn color() -> TextureClass {
    TextureClass::Color(ColorSettings::default())
}

/// A map reads as its texture's cook reads it: float sources are linear light whatever their class,
/// integer ones sRGB-encoded in both classes that can color a surface, nothing past what the cooked
/// format holds, and every sample at its own depth.
#[test]
fn a_color_map_reads_as_its_texture_does() {
    let scratch = TempDir::new("probe_color_maps");
    let srgb = |code: u16, max: f32| crate::color::srgb_to_linear(f32::from(code) / max);
    let gray8 = |code: u8, format| {
        let image = ::image::RgbaImage::from_pixel(4, 4, ::image::Rgba([code, code, code, 255]));
        encoded(::image::DynamicImage::ImageRgba8(image), format)
    };
    let gray16 = |code: u16| {
        let image = ::image::ImageBuffer::<::image::Rgba<u16>, Vec<u16>>::from_pixel(
            4,
            4,
            ::image::Rgba([code, code, code, 65535]),
        );
        encoded(::image::DynamicImage::ImageRgba16(image), ::image::ImageFormat::Png)
    };
    use ::image::ImageFormat::{Png, Tga};
    let rows: Vec<(&str, &str, Vec<u8>, TextureClass, f32)> = vec![
        ("HDR past one", "a.exr", exr(4.0), hdr(), 4.0),
        ("HDR below one stays linear", "b.exr", exr(0.5), hdr(), 0.5),
        ("HDR below zero", "c.exr", exr(-1.0), hdr(), 0.0),
        ("HDR past a half float", "d.exr", exr(70000.0), hdr(), 65504.0),
        ("an HDR map in 8 bits is sRGB", "e.png", gray8(128, Png), hdr(), srgb(128, 255.0)),
        ("a color map in 8 bits is too", "f.png", gray8(128, Png), color(), srgb(128, 255.0)),
        ("a TGA reads as a PNG does", "g.tga", gray8(128, Tga), color(), srgb(128, 255.0)),
        ("16 bits keep their depth", "h.png", gray16(0x8040), hdr(), srgb(0x8040, 65535.0)),
        ("a float color map is linear too", "i.exr", exr(0.5), color(), 0.5),
        ("no brighter than BC7 and ASTC hold", "j.exr", exr(4.0), color(), 1.0),
    ];
    let mut wrong = Vec::new();
    for (name, file, bytes, class, expected) in rows {
        let path = scratch.path().join(file);
        fs::write(&path, bytes).unwrap();
        let settings = ImportSettings::Texture(TextureSettings { class });
        let input = |_| Some(ProbeInput { source: &path, settings: &settings, mesh: false, cooked: None });
        let read = color_map(Some(AssetId::from_bytes([1; 16])), &input);
        match read {
            Ok(Some(map)) if map.mean(|_| true) == Vec3::splat(expected) => {}
            Ok(Some(map)) => wrong.push(format!("{name}: {} for {expected}", map.mean(|_| true))),
            other => wrong.push(format!("{name}: {:?}", other.map(|map| map.is_some()))),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");

    // Alpha is a share at any depth, and cuts a card as it did.
    let path = scratch.path().join("leaves.png");
    let leaves = ::image::RgbaImage::from_pixel(4, 4, ::image::Rgba([40, 200, 40, 128]));
    leaves.save_with_format(&path, Png).unwrap();
    let settings = ImportSettings::Texture(TextureSettings { class: color() });
    let input = |_| Some(ProbeInput { source: &path, settings: &settings, mesh: false, cooked: None });
    let map = color_map(Some(AssetId::from_bytes([1; 16])), &input).unwrap().unwrap();
    assert!(map.alpha.iter().all(|&alpha| alpha == 128.0 / 255.0));
    assert_eq!((map.width, map.height), (4, 4));
}

/// An emissive floor's bounce follows its map past one: at 1 and 4 a grid baked from a map read in 8
/// bits came out the same to the byte, while the renderer shows the map's own light.
#[test]
fn an_hdr_emission_map_bounces_at_its_own_strength() {
    let scratch = TempDir::new("probe_hdr_emission");
    let glow = AssetId::from_text("00000000-0000-4000-8000-000000000006");
    // The red channel's bounce, over every probe and key.
    let bake = |name: &str, map: &[u8], file: &str, class: TextureClass| {
        let source = scratch.path().join(name).join("content");
        write_tree(&source, [0.0, 0.0, 0.0, 1.0], Wall::Open);
        let material = EnvironmentMaterial {
            base_color: [0.0, 0.0, 0.0, 1.0],
            emission: [1.0; 3],
            emission_map: Some(glow),
            emission_steady: 1.0,
            ..EnvironmentMaterial::default()
        };
        fs::write(source.join("floor.material.ron"), sapphire_serialize::to_ron(&material).unwrap()).unwrap();
        fs::write(source.join(file), map).unwrap();
        let settings = ImportSettings::Texture(TextureSettings { class });
        let meta = AssetMeta::new(glow, settings);
        fs::write(sidecar_path(&source.join(file)), sapphire_serialize::to_ron(&meta).unwrap()).unwrap();
        let out = scratch.path().join(name).join("out");
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
        cook_dir(&source, &out, &options).unwrap();
        let bytes = crate::test_util::read_aligned(&out.join("court.sprobe"));
        ProbeGrid::read(&bytes)
            .unwrap()
            .bounce
            .iter()
            .step_by(12)
            .map(|bits| half::f16::from_bits(bits.to_native()).to_f32())
            .sum::<f32>()
    };
    let (dark, one, four) = (
        bake("dark", &exr(0.0), "glow.exr", hdr()),
        bake("one", &exr(1.0), "glow.exr", hdr()),
        bake("four", &exr(4.0), "glow.exr", hdr()),
    );
    assert_eq!(dark, 0.0, "a black floor under a black map lights nothing");
    assert!(one > 0.0, "the map at 1 lights the grid");
    assert!((four / one - 4.0).abs() < 0.02, "the map at 4 lights it 4 times as much: {four} and {one}");

    // Below one, the same light in a float file and an integer one bakes the same, to rounding.
    let gray = ::image::RgbaImage::from_pixel(4, 4, ::image::Rgba([128, 128, 128, 255]));
    let png = encoded(::image::DynamicImage::ImageRgba8(gray), ::image::ImageFormat::Png);
    let linear = crate::color::srgb_to_linear(128.0 / 255.0);
    let as_float = bake("float", &exr(linear), "glow.exr", hdr());
    let as_integer = bake("integer", &png, "glow.png", hdr());
    assert!((as_integer / as_float - 1.0).abs() < 1e-3, "{as_integer} and {as_float}");
}

#[test]
fn foliage_cards_bake_through_their_maps() {
    // Through the cook: a wall bound to an alpha-tested foliage material (M1 W6) reads its card
    // map's alpha. Clear everywhere, nothing of it is drawn and it bakes as no wall at all; opaque,
    // it shades the grid, as two-sided green leaves rather than the stone wall's grey.
    let scratch = TempDir::new("probe_foliage_cards");
    let bake = |wall: Wall, name: &str| {
        let source = scratch.path().join(name).join("content");
        write_tree(&source, [0.8, 0.2, 0.1, 1.0], wall);
        let out = scratch.path().join(name).join("out");
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false, editor: false };
        cook_dir(&source, &out, &options).unwrap();
        fs::read(out.join("court.sprobe")).unwrap()
    };
    let open = bake(Wall::Open, "open");
    assert!(bake(Wall::Leaves(0), "clear") == open, "cut out whole, the wall bakes as none");
    let leaves = bake(Wall::Leaves(255), "leaves");
    assert!(leaves != open, "leaves shade the grid");
    assert!(leaves != bake(Wall::Stone, "stone"), "as leaves, not stone");
}

#[test]
fn earlier_probe_bakes_miss_the_repaired_tie_semantics() {
    use crate::cache::{Cache, CacheKey, KeyInputs, Stage};
    use crate::cook::{Cook, Recipe};
    use crate::meta::ImportSettings;
    let dir = TempDir::new("probe_tie_version");
    let cache = Cache::new(dir.path().join("cache"));
    let recipe = Recipe { importer: IMPORTER, cooker: COOKER, format: 1, extension: "sprobe" };
    let old = Recipe { cooker: Stage { version: 3, ..COOKER }, ..recipe };
    let settings = ImportSettings::ProbeGrid(Default::default());
    let inputs = KeyInputs {
        source: blake3::hash(b"same probes"),
        recipe: &recipe,
        settings: &settings,
        cook: Cook::Desktop,
        dependencies: &[],
    };
    let key = CacheKey::new(&inputs);
    cache.put(&CacheKey::new(&KeyInputs { recipe: &old, ..inputs }), b"old tie winner").unwrap();
    assert!(cache.get(&key).unwrap().is_none());
    cache.put(&key, b"new tie winner").unwrap();
    assert_eq!(cache.get(&key).unwrap().unwrap(), b"new tie winner");
}

/// A grid baked under its zone's day reads that document, from where the grid's own document is, and a bake
/// keeps the keys it says (M3 C1). A wrong path says which.
#[test]
fn a_grid_bakes_under_the_day_its_document_names() {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let grid = content.join("zones/town/no_grid.probes.ron");
    let named = BakeTimeline::Document("../../lookdev/lookdev_day.ron".to_owned());
    assert!(!keys(&named, &grid).unwrap().is_empty());
    let wrong = BakeTimeline::Document("no_such_day.ron".to_owned());
    let message = timeline(&wrong, &grid).unwrap_err().to_string();
    assert!(message.contains("no_such_day.ron"), "{message}");
}
