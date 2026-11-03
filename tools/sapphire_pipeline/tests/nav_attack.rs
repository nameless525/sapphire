//! Attacks the bake with scenes nobody drew: turned boxes, posts, walls and platforms on a floor, some
//! crowded and some not. `rerecast` 0.5's bugs are on paths real content reaches rarely, so an
//! ordinary test can pass over them. What it found stays as an ordinary test here.
//!
//! ```text
//! SAPPHIRE_NAV_SCENES=2000 cargo test --profile tools -p sapphire_pipeline --test nav_attack -- --ignored --nocapture
//! ```
//!
//! A hang shows as the run not finishing: the last scene printed is the one before it. `NAV-ATTACK`
//! lines are written past the harness's capture for that reason.

use std::io::Write as _;

use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Group, Rotation};
use sapphire_navmesh::NavMesh;
use sapphire_pipeline::nav::{BakeSettings, bake, colliders_to_triangles};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn within(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next()
    }
}

fn scene(seed: u64) -> ColliderDocument {
    let mut random = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut colliders = vec![ColliderEntry {
        name: "floor".to_owned(),
        shape: ColliderShape::Box { half_extents: [34.0, 0.5, 34.0] },
        position: [0.0, -0.5, 0.0],
        rotation: Rotation::None,
        groups: vec![Group::World],
    }];
    let count = 1 + (random.next() * 120.0) as u32;
    for index in 0..count {
        let kind = random.next();
        let half_extents = if kind < 0.3 {
            // A post.
            [random.within(0.1, 0.5), random.within(0.5, 3.0), random.within(0.1, 0.5)]
        } else if kind < 0.6 {
            // A wall.
            [random.within(0.1, 0.4), random.within(0.8, 3.0), random.within(1.0, 8.0)]
        } else if kind < 0.8 {
            // Something low enough to walk over, or to stand on.
            [random.within(0.3, 3.0), random.within(0.05, 0.5), random.within(0.3, 3.0)]
        } else {
            [random.within(0.5, 5.0), random.within(0.5, 2.5), random.within(0.5, 5.0)]
        };
        let lifted = if random.next() < 0.1 { random.within(1.8, 5.0) } else { 0.0 };
        colliders.push(ColliderEntry {
            name: format!("box{index:03}"),
            shape: ColliderShape::Box { half_extents },
            position: [random.within(-32.0, 32.0), half_extents[1] + lifted, random.within(-32.0, 32.0)],
            rotation: if random.next() < 0.5 {
                Rotation::None
            } else {
                Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: random.within(0.0, 180.0) }
            },
            groups: vec![Group::World],
        });
    }
    ColliderDocument { colliders }
}

/// Each polygon with a corner that turns the wrong way: its corners, and how far each turns.
fn reflex_polygons(mesh: &NavMesh) -> Vec<(usize, Vec<[i32; 3]>, Vec<i64>)> {
    let mut bad = Vec::new();
    for polygon in 0..mesh.polygon_count() {
        let (start, end) = (mesh.polygon_starts[polygon] as usize, mesh.polygon_starts[polygon + 1] as usize);
        let corners: Vec<[i32; 3]> =
            mesh.corners[start..end].iter().map(|&v| mesh.vertices[v as usize]).collect();
        let n = corners.len();
        let turns: Vec<i64> = (0..n)
            .map(|i| {
                let (a, b, c) = (corners[i], corners[(i + 1) % n], corners[(i + 2) % n]);
                i64::from(b[2] - a[2]) * i64::from(c[0] - a[0])
                    - i64::from(b[0] - a[0]) * i64::from(c[2] - a[2])
            })
            .collect();
        if turns.iter().any(|&t| t < 0) {
            bad.push((polygon, corners, turns));
        }
    }
    bad
}

/// Two scenes the attack found, kept: one whose tile `rerecast` refused, one it wound a polygon backward
/// in. Both have an edge between regions crossing its own contour.
#[test]
fn a_crossed_contour_follows_its_cells_and_the_mesh_reads() {
    for seed in [16, 54] {
        let set = scene(seed).cook().expect("it cooks");
        let triangles = colliders_to_triangles(&set, Group::World.mask());
        let baked = bake(&triangles, ([-3, -3], [2, 2]), &BakeSettings::CHARACTER).expect("it bakes");
        assert_eq!(baked.stats.tiles_cell_exact, 1, "scene {seed}");
        let bytes = baked.mesh.to_cooked().expect("it writes");
        NavMesh::read(&bytes).unwrap_or_else(|error| panic!("scene {seed}: {error}"));
    }
}

#[test]
#[ignore = "takes minutes: an attack, not a check"]
fn no_scene_stops_the_bake() {
    let scenes: u64 =
        std::env::var("SAPPHIRE_NAV_SCENES").ok().and_then(|text| text.parse().ok()).unwrap_or(200);
    let first: u64 = std::env::var("SAPPHIRE_NAV_SEED").ok().and_then(|text| text.parse().ok()).unwrap_or(1);
    let settings = BakeSettings::CHARACTER;
    let mut failed = Vec::new();
    let (mut polygons, mut tiles, mut cell_exact, mut holes_lost) = (0, 0, 0, 0);
    let mut stdout = std::io::stdout();
    for seed in first..first + scenes {
        let set = scene(seed).cook().expect("it cooks");
        let triangles = colliders_to_triangles(&set, Group::World.mask());
        match bake(&triangles, ([-3, -3], [2, 2]), &settings) {
            Ok(baked) => {
                tiles += baked.stats.tiles_kept;
                cell_exact += baked.stats.tiles_cell_exact;
                holes_lost += baked.stats.holes_lost;
                let bytes = baked.mesh.to_cooked().expect("it writes");
                match NavMesh::read(&bytes) {
                    Ok(_) => polygons += baked.mesh.polygon_count(),
                    Err(error) => {
                        let bad = reflex_polygons(&baked.mesh);
                        failed.push((seed, format!("unreadable: {error}; reflex {bad:?}")));
                    }
                }
            }
            Err(error) => failed.push((seed, error.to_string())),
        }
        if seed % 100 == 0 {
            writeln!(stdout, "NAV-ATTACK scene {seed}: {} failed so far", failed.len()).expect("it prints");
        }
    }
    writeln!(
        stdout,
        "NAV-ATTACK {scenes} scenes, {polygons} polygons, {tiles} tiles, {cell_exact} following cells, {holes_lost} holes lost, {} failed",
        failed.len()
    )
    .expect("it prints");
    assert!(failed.is_empty(), "{failed:?}");
}
