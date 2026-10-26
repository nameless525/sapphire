use std::fs;

use glam::{Vec2, Vec3};
use sapphire_assets::material::{EnvironmentMaterial, FogCardMaterial, FoliageMaterial};
use sapphire_assets::probes::{BRICK_PROBES, EMPTY_BRICK, ProbeData};
use sapphire_math::sh;
use serde_json::json;

use super::bake::{BakeKey, BakeMaterial, BakeScene, BakeSettings, BakeTriangle, Cutout, bake};
use super::*;
use crate::cook::{CookOptions, CookStatus, cook_dir};
use crate::gltf_writer::{Geometry, GltfWriter};
use crate::meta::{AssetMeta, sidecar_path};
use crate::test_util::TempDir;
use crate::texture::{ColorSettings, TextureSettings};

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
        let meta = AssetMeta { id: id(text), importer };
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
        let options = CookOptions { cook: target, cache: Some(cache.as_path()), locked: true };
        (cook_dir(&source, &out, &options).unwrap(), out)
    };
    let (report, out) = cook(Cook::Desktop);
    let grid_asset =
        report.assets.iter().find(|asset| asset.output.ends_with("court.sprobe")).expect("baked");
    assert_eq!(grid_asset.status, CookStatus::Cooked);
    let bytes = crate::test_util::read_aligned(&out.join("court.sprobe"));
    let grid = ProbeGrid::read(&bytes).expect("a valid grid");
    let default_day = TodTimeline::default_day();
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
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true };
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
    let day = keys(BakeTimeline::DefaultDay);
    assert_eq!(keys_hash(&day), keys_hash(&keys(BakeTimeline::DefaultDay)));
    let mut moved = day.clone();
    moved[2].sky[1][4] = f32::from_bits(moved[2].sky[1][4].to_bits() + 1);
    assert_ne!(keys_hash(&day), keys_hash(&moved));
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
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false };
        cook_dir(&source, &out, &options).unwrap();
        fs::read(out.join("court.sprobe")).unwrap()
    };
    let open = bake(Wall::Open, "open");
    assert!(bake(Wall::Leaves(0), "clear") == open, "cut out whole, the wall bakes as none");
    let leaves = bake(Wall::Leaves(255), "leaves");
    assert!(leaves != open, "leaves shade the grid");
    assert!(leaves != bake(Wall::Stone, "stone"), "as leaves, not stone");
}
