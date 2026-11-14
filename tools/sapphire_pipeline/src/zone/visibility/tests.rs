//! Visible sets on the `ci` district: occluders that hide only what a building's mesh hides, sets that hold
//! every object a brute force sees, and the same bytes on every run.
#![allow(
    clippy::print_stdout,
    reason = "with --nocapture, the counts these tests print are the spike's evidence"
)]

use std::path::PathBuf;

use super::*;
use crate::bench::Preset;
use crate::bench::district::{self, BUILDING_SLOTS};

/// The repository's `bench/district/`.
fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/district")
}

/// A test's coarser sampling: the argument doesn't lean on the spacing, so a cheaper cook proves it as well.
const COARSE: Sampling = Sampling { spacing: 2.0, layer: 1.0, heights: [2.0, 3.0], piece: 1.0 };

/// Seeded numbers in `[low, high)`.
struct Random(u64);

impl Random {
    fn next(&mut self, low: f32, high: f32) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        low + (high - low) * ((self.0 >> 40) as f32 / (1_u64 << 24) as f32)
    }

    fn inside(&mut self, b: &Aabb) -> [f32; 3] {
        std::array::from_fn(|axis| self.next(b.min[axis], b.max[axis]))
    }
}

/// Where segment `from` → `to` first meets triangle `abc` from its front (counter-clockwise), as a share of
/// the way, in `f64`.
fn front_hit(from: [f32; 3], to: [f32; 3], [a, b, c]: [[f32; 3]; 3]) -> Option<f64> {
    let v = |p: [f32; 3]| p.map(f64::from);
    let sub = |p: [f64; 3], q: [f64; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let cross = |p: [f64; 3], q: [f64; 3]| {
        [p[1] * q[2] - p[2] * q[1], p[2] * q[0] - p[0] * q[2], p[0] * q[1] - p[1] * q[0]]
    };
    let dot = |p: [f64; 3], q: [f64; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
    let (a, b, c, origin) = (v(a), v(b), v(c), v(from));
    let direction = sub(v(to), origin);
    let (ab, ac) = (sub(b, a), sub(c, a));
    // Seen from its back, a face isn't drawn.
    if dot(cross(ab, ac), direction) >= 0.0 {
        return None;
    }
    let p = cross(direction, ac);
    let determinant = dot(ab, p);
    let inverse = 1.0 / determinant;
    let s = sub(origin, a);
    let u = dot(s, p) * inverse;
    let q = cross(s, ab);
    let w = dot(direction, q) * inverse;
    let t = dot(ac, q) * inverse;
    (u >= 0.0 && w >= 0.0 && u + w <= 1.0 && (0.0..=1.0).contains(&t)).then_some(t)
}

/// Where segment `from` → `to` enters `b`, as a share of the way.
fn enters(b: &Aabb, from: [f32; 3], to: [f32; 3]) -> Option<f64> {
    crosses(b, from, to, 1.0).map(|(enter, _)| enter)
}

/// Where the line from `from` through `to` enters and leaves `b` within `reach` of the way, as shares of it.
fn crosses(b: &Aabb, from: [f32; 3], to: [f32; 3], reach: f64) -> Option<(f64, f64)> {
    let (mut enter, mut leave) = (0.0_f64, reach);
    for axis in 0..3 {
        let (origin, step) = (f64::from(from[axis]), f64::from(to[axis]) - f64::from(from[axis]));
        let (min, max) = (f64::from(b.min[axis]), f64::from(b.max[axis]));
        if step == 0.0 {
            if origin < min || origin > max {
                return None;
            }
            continue;
        }
        let (p, q) = ((min - origin) / step, (max - origin) / step);
        enter = enter.max(p.min(q));
        leave = leave.min(p.max(q));
    }
    (enter <= leave).then_some((enter, leave))
}

#[test]
fn a_halls_box_is_closed_by_what_its_building_draws() {
    // Every building variant at both details: lines from a camera outside its bounds through its occluder
    // boxes' part above the ground each meet a face drawn toward them before they leave the box. Cameras
    // stand no lower than any sampling's least height.
    let district: district::BenchDistrict =
        sapphire_serialize::from_ron(&std::fs::read_to_string(dir().join("district.ron")).unwrap()).unwrap();
    let sources = district::generate(&dir(), Preset::Ci).unwrap();
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let (mut segments, mut misses, mut failures) = (0, 0_u32, Vec::new());
    for variant in 0..district.buildings.variants {
        let path = district::building_path(variant);
        let source = sources.iter().find(|source| source.path == path).unwrap();
        let assembly = assembly::read(&source.bytes).unwrap();
        let shaped = Shaped::of(&path, &assembly).unwrap();
        assert!(!shaped.halls.is_empty(), "variant {variant} has halls");
        for detail in [Detail::Full, Detail::Light] {
            let mesh = assembly::import_parts(Path::new(&path), &assembly, detail).unwrap().mesh;
            assert_eq!(mesh.materials.len(), BUILDING_SLOTS.len());
            let triangles: Vec<[[f32; 3]; 3]> = mesh
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|corners| corners.map(|corner| mesh.vertices[corner as usize].position))
                .collect();
            for hall in grounded(shaped.halls.clone()) {
                let above = Aabb { min: [hall.min[0], hall.min[1].max(0.01), hall.min[2]], ..hall };
                for _ in 0..400 {
                    let target = random.inside(&above);
                    // From 0.2 to 30 m outside the building's bounds.
                    let around = shaped.bounds.grown(30.0);
                    let from = loop {
                        let from = [
                            random.next(around.min[0], around.max[0]),
                            random.next(0.5, 20.0),
                            random.next(around.min[2], around.max[2]),
                        ];
                        if !shaped.bounds.grown(0.2).contains(from) {
                            break from;
                        }
                    };
                    let (_, leaves) = crosses(&above, from, target, 1e6).expect("the line passes through");
                    let hit = |triangle: &[[f32; 3]; 3]| {
                        let far =
                            [0, 1, 2].map(|axis| from[axis] + (target[axis] - from[axis]) * leaves as f32);
                        front_hit(from, far, *triangle)
                    };
                    let first = triangles.iter().filter_map(hit).fold(f64::INFINITY, f64::min);
                    if first >= 1.0 && failures.len() < 8 {
                        failures.push(format!(
                            "variant {variant} {detail:?}: {from:?} through {target:?} leaves {above:?} unmet"
                        ));
                    }
                    misses += u32::from(first >= 1.0);
                    segments += 1;
                }
            }
        }
    }
    println!("halls and roofs: {segments} lines, {misses} unmet");
    assert!(misses == 0, "{misses} of {segments} lines: {failures:#?}");
    assert!(segments > 50_000, "{segments}");
}

#[test]
fn the_district_reads_as_cells_objects_and_buildings() {
    let scene = district::visibility_scene(&dir(), Preset::Ci).unwrap();
    // 4 × 4 cells, each with its merged mesh first.
    assert_eq!(scene.cells.len(), 16);
    let buildings: usize = scene.cells.iter().map(|cell| cell.objects.len() - 1).sum();
    assert_eq!(scene.blockers.len(), buildings, "every building blocks");
    for blocker in &scene.blockers {
        // Held by what it draws, a storey's box from its plinth's top up, the next on it.
        assert!(blocker.solids.iter().all(|solid| blocker.bounds.holds(solid)), "{blocker:?}");
        assert!(blocker.solids.iter().any(|solid| solid.min[1] < 0.5), "stands on its plinth: {blocker:?}");
    }
}

#[test]
fn a_set_holds_everything_a_brute_force_sees() {
    // Small spaces a camera may stand in, each a metre on a side round one sample, as a cook's sampling at a
    // metre's spacing makes them: the sample's set, against the occluders shrunk by half a metre, holds every
    // object a ray from anywhere in the space reaches past the occluders as they are.
    let scene = district::visibility_scene(&dir(), Preset::Ci).unwrap();
    let objects: Vec<Aabb> = scene.objects().map(|object| object.bounds).collect();
    let solids: Vec<Aabb> = scene.blockers.iter().flat_map(|blocker| blocker.solids.clone()).collect();
    let keep_out: Vec<Aabb> = scene.blockers.iter().map(|blocker| blocker.bounds).collect();
    let reach = [0.5; 3];
    let occluders = Occluders::new(&scene.blockers, reach);
    let mut scratch = Scratch::new(&occluders);
    let mut random = Random(0x2545_f491_4f6c_dd1d);
    let (mut spaces, mut asked, mut hidden_sizes) = (0, 0, Vec::new());
    // How tight the sets are: of the objects a set holds, those a ray from the sample reaches past the
    // occluders as they are, and past them shrunk, in 256 tries each.
    let (mut held, mut clear, mut clear_shrunk) = (0, 0, 0);
    while spaces < 48 {
        let sample = [random.next(-63.0, 63.0), random.next(2.0, 3.0), random.next(-63.0, 63.0)];
        let space = Aabb {
            min: [0, 1, 2].map(|a| sample[a] - reach[a]),
            max: [0, 1, 2].map(|a| sample[a] + reach[a]),
        };
        if keep_out.iter().any(|bounds| bounds.overlaps(&space)) {
            continue;
        }
        spaces += 1;
        let (set, _, _) = region_set(&occluders, &objects, [sample], 1.0, &mut scratch);
        let unseen: Vec<usize> =
            (0..objects.len()).filter(|&object| set[object / 64] & (1 << (object % 64)) == 0).collect();
        hidden_sizes.push(unseen.len());
        for object in (0..objects.len()).filter(|object| !unseen.contains(object)) {
            let tries: Vec<[f32; 3]> = (0..256).map(|_| random.inside(&objects[object])).collect();
            let past = |boxes: &[Aabb]| {
                tries.iter().any(|point| !boxes.iter().any(|b| enters(b, sample, *point).is_some()))
            };
            held += 1;
            clear += usize::from(past(&solids));
            clear_shrunk += usize::from(past(&occluders.boxes));
        }
        for _ in 0..30 {
            let camera = random.inside(&space);
            for &object in &unseen {
                for _ in 0..16 {
                    let point = random.inside(&objects[object]);
                    let blocked = solids.iter().any(|solid| enters(solid, camera, point).is_some());
                    assert!(blocked, "{camera:?} by {sample:?} sees object {object} at {point:?}");
                    asked += 1;
                }
            }
        }
    }
    println!("brute force: {asked} rays; objects hidden from each space {hidden_sizes:?}");
    println!(
        "of {held} objects held, a ray reached {clear} past the occluders and {clear_shrunk} past them shrunk"
    );
    // It proves something: the spaces hide some of the district between them.
    assert!(asked > 200_000, "{asked} rays");
    assert!(hidden_sizes.iter().sum::<usize>() > 300, "{hidden_sizes:?}");
}

#[test]
fn the_sets_are_the_same_bytes_on_every_run_however_many_threads() {
    let scene = district::visibility_scene(&dir(), Preset::Ci).unwrap();
    let (one, _) = cook(&scene, COARSE, 1);
    let (four, _) = cook(&scene, COARSE, 4);
    assert_eq!(one.to_bytes(), four.to_bytes());
    let again = district::visibility_scene(&dir(), Preset::Ci).unwrap();
    assert_eq!(again, scene);
}

#[test]
fn the_ci_districts_sets_are_pinned() {
    // The combat sampling's sets, which a dev sandbox reads: CI's x86-64 run checks they're these bytes
    // there too. A change to the district, the cook's transforms or this module repins it, saying why.
    let scene = district::visibility_scene(&dir(), Preset::Ci).unwrap();
    for (sampling, pinned) in [(Sampling::WALK, PINNED_CI_WALK), (Sampling::COMBAT, PINNED_CI_COMBAT)] {
        let (sets, _) = cook(&scene, sampling, 4);
        let sizes = sets.set_sizes();
        println!(
            "ci sets at {:?} m: {} objects, sizes {sizes:?}, digest {:#018x}",
            sampling.heights,
            sets.object_count(),
            sets.digest()
        );
        assert_eq!(sets.digest(), pinned, "{:?} m: sizes {sizes:?}", sampling.heights);
    }
}

/// [`Sampling::WALK`]'s and [`Sampling::COMBAT`]'s sets for the `ci` district.
const PINNED_CI_WALK: u64 = 0xc810_47f3_5864_62a3;
const PINNED_CI_COMBAT: u64 = 0x9d73_06fb_c155_d267;

#[test]
fn a_sample_inside_an_occluder_sees_nothing_and_points_cover_a_cell() {
    let sampling = Sampling::COMBAT;
    let points = sampling.points(32.0, [-1, 2]);
    assert_eq!(points.len(), 32 * 32 * 5);
    let reach = sampling.reach(32.0);
    assert_eq!(reach, [0.5, 0.75, 0.5]);
    // Every point of the cell's space is within reach of a sample on every axis.
    let mut random = Random(7);
    for _ in 0..2_000 {
        let at = [random.next(-32.0, 0.0), random.next(0.5, 8.0), random.next(64.0, 96.0)];
        let near = points.iter().any(|p| (0..3).all(|axis| (p[axis] - at[axis]).abs() <= reach[axis] + 1e-5));
        assert!(near, "{at:?} is out of reach of every sample");
    }
    // A wall of one occluder: what's behind it is hidden, what's beside it isn't, and inside it nothing
    // is asked.
    let wall = Blocker {
        solids: vec![Aabb { min: [-10.0, BELOW, 4.0], max: [10.0, 6.0, 6.0] }],
        bounds: Aabb { min: [-10.5, 0.0, 3.5], max: [10.5, 7.0, 6.5] },
    };
    let occluders = Occluders::new(std::slice::from_ref(&wall), [0.5; 3]);
    let mut scratch = Scratch::new(&occluders);
    let behind = Aabb { min: [-2.0, 0.0, 10.0], max: [2.0, 3.0, 12.0] };
    let beside = Aabb { min: [20.0, 0.0, 10.0], max: [22.0, 3.0, 12.0] };
    let over = Aabb { min: [-2.0, 9.0, 10.0], max: [2.0, 12.0, 12.0] };
    assert!(hidden(&occluders, [0.0, 2.0, 0.0], &behind, 1.0, &mut scratch));
    assert!(!hidden(&occluders, [0.0, 2.0, 0.0], &beside, 1.0, &mut scratch));
    assert!(
        !hidden(&occluders, [0.0, 2.0, 0.0], &over, 1.0, &mut scratch),
        "seen over its top, which shrank"
    );
    assert!(!hidden(&occluders, [0.0, 9.0, 0.0], &behind, 1.0, &mut scratch), "seen over it");
    assert!(occluders.inside([0.0, 1.0, 5.0]) && !occluders.inside([0.0, 1.0, 3.0]));
}
