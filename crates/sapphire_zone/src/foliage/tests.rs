use std::collections::BTreeSet;

use super::*;
use crate::grid::Grid;

const GROVE: &str = include_str!("../../testdata/grove.foliage.ron");

fn grove() -> FoliageDocument {
    FoliageDocument::from_ron(GROVE).expect("the grove reads")
}

/// Gentle ground rising east, a steep bank past x = 24 and none at all south of z = −36.
fn ground(x: f32, z: f32) -> Option<GroundPoint> {
    if z < -36.0 {
        return None;
    }
    let (height, normal) = if x > 24.0 {
        (0.05 * x + 0.02 * z + (x - 24.0), [-1.05, 1.0, -0.02])
    } else {
        (0.05 * x + 0.02 * z, [-0.05, 1.0, -0.02])
    };
    Some(GroundPoint { height, normal })
}

/// Every plant in `cells` of `grid`, as comparable values.
fn plants(foliage: &FoliageDocument, grid: Grid, cells: &[CellCoord]) -> BTreeSet<(u32, u64, [u32; 3], u32)> {
    let mut all = BTreeSet::new();
    for &cell in cells {
        let set = scatter_set(foliage, &ground, grid, cell).expect("places");
        for run in &set.runs {
            for chunk in &set.chunks[run.first_chunk as usize..(run.first_chunk + run.chunk_count) as usize] {
                for at in chunk.first as usize..(chunk.first + chunk.count) as usize {
                    let (instance, id) = (set.instances[at], set.ids[at]);
                    let plant = (run.kind, id, instance.position.map(f32::to_bits), instance.packed);
                    assert!(all.insert(plant), "placed twice");
                }
            }
        }
    }
    all
}

fn cells_of(foliage: &FoliageDocument, grid: Grid) -> Vec<CellCoord> {
    foliage.cells(grid).expect("cells")
}

#[test]
fn the_grove_reads_and_plans_the_cells_its_layers_reach() {
    let grove = grove();
    assert_eq!(grove.kinds.len(), 3);
    let cells = cells_of(&grove, Grid { cell_size: 32.0 });
    let expected: Vec<CellCoord> = (-2..=1).flat_map(|x| (-2..=1).map(move |z| CellCoord { x, z })).collect();
    assert_eq!(cells, expected, "−40 to 40 m on a 32 m grid");
}

/// The instance is what the GPU reads: 16 bytes, its scale and yaw from its bytes.
#[test]
fn an_instance_is_sixteen_bytes_and_unpacks() {
    let instance = ScatterInstance::new([1.0, 2.0, 3.0], 64, 255, 7, 0);
    assert_eq!(
        (instance.yaw_byte(), instance.scale_byte(), instance.tint(), instance.flags()),
        (64, 255, 7, 0)
    );
    assert!((instance.yaw() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert!((instance.scale() - MAX_SCALE).abs() < 1e-5);
    assert!((ScatterInstance::new([0.0; 3], 0, 0, 0, 0).scale() - MIN_SCALE).abs() < 1e-6);
    let set = ScatterSet::from_placed([0, 0], vec![Placed { kind: 0, id: 9, instance }]);
    let bytes = set.to_cooked().expect("writes");
    let read = ScatterSet::read(&bytes).expect("reads");
    assert_eq!(read.instance(0), instance);
    let instances = read.instances.as_ptr() as usize;
    assert_eq!(instances % 4, 0, "aligned as the GPU reads them");
}

/// The determinism the cook leans on: each cell's set the same bytes whichever thread places it, and the same
/// plants whichever grid cuts them, since every choice is keyed by world space.
#[test]
fn placement_is_the_same_at_one_and_eight_threads_and_under_any_cut() {
    let grove = grove();
    let grid = Grid { cell_size: 32.0 };
    let cells = cells_of(&grove, grid);
    let one: Vec<AlignedVec> = cells
        .iter()
        .map(|&cell| scatter_set(&grove, &ground, grid, cell).expect("places").to_cooked().expect("writes"))
        .collect();
    let eight: Vec<AlignedVec> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let (grove, cells) = (&grove, &cells);
                scope.spawn(move || {
                    let mine = cells.iter().enumerate().filter(|(index, _)| index % 8 == worker);
                    let made = mine.map(|(index, &cell)| {
                        (
                            index,
                            scatter_set(grove, &ground, grid, cell)
                                .expect("places")
                                .to_cooked()
                                .expect("writes"),
                        )
                    });
                    made.collect::<Vec<_>>()
                })
            })
            .collect();
        let mut made: Vec<(usize, AlignedVec)> =
            workers.into_iter().flat_map(|worker| worker.join().expect("a worker")).collect();
        made.sort_by_key(|(index, _)| *index);
        made.into_iter().map(|(_, bytes)| bytes).collect()
    });
    assert_eq!(one.len(), 16);
    for (cell, (one, eight)) in cells.iter().zip(one.iter().zip(&eight)) {
        assert_eq!(one.as_slice(), eight.as_slice(), "cell {cell:?}");
    }

    let at_32 = plants(&grove, grid, &cells);
    let wide = Grid { cell_size: 64.0 };
    let at_64 = plants(&grove, wide, &cells_of(&grove, wide));
    let fine = Grid { cell_size: 12.5 };
    let at_12 = plants(&grove, fine, &cells_of(&grove, fine));
    assert!(at_32.len() > 1000, "{} plants", at_32.len());
    assert_eq!(at_32, at_64, "a 64 m cut places what a 32 m one does");
    assert_eq!(at_32, at_12, "and so does a 12.5 m one");
}

/// What the rules say holds of what they place: the spacing, the avoided distances, the clearing, the slope and
/// height, the layers' hole and the ground's end.
#[test]
fn every_rule_holds_of_its_plants() {
    let grove = grove();
    let grid = Grid { cell_size: 32.0 };
    let all = plants(&grove, grid, &cells_of(&grove, grid));
    let of = |kind: u32| -> Vec<[f32; 3]> {
        all.iter().filter(|plant| plant.0 == kind).map(|plant| plant.2.map(f32::from_bits)).collect()
    };
    let (oaks, ferns, clumps) = (of(0), of(1), of(2));
    assert!(
        oaks.len() > 20 && ferns.len() > 500 && clumps.len() > 500,
        "{} {} {}",
        oaks.len(),
        ferns.len(),
        clumps.len()
    );
    let apart = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]) * (a[0] - b[0]) + (a[2] - b[2]) * (a[2] - b[2]);
    for (index, a) in oaks.iter().enumerate() {
        for b in &oaks[index + 1..] {
            assert!(apart(*a, *b) >= 16.0, "two oaks within 4 m: {a:?} {b:?}");
        }
    }
    for fern in &ferns {
        assert!(oaks.iter().all(|oak| apart(*oak, *fern) > 1.5 * 1.5), "a fern by a trunk: {fern:?}");
        let clearing = apart(*fern, [-20.0, 0.0, 20.0]) + fern[1] * fern[1];
        assert!(clearing > 36.0, "a fern in the clearing: {fern:?}");
        assert!((-1.0..=3.0).contains(&fern[1]), "a fern at {} m", fern[1]);
    }
    for clump in &clumps {
        assert!(clump[0] < 0.0 && clump[2] < 0.0, "grass off its meadow: {clump:?}");
        assert!(oaks.iter().all(|oak| apart(*oak, *clump) > 0.8 * 0.8), "grass in a trunk: {clump:?}");
    }
    for plant in oaks.iter().chain(&ferns).chain(&clumps) {
        assert!(plant[2] >= -36.0, "a plant where there's no ground: {plant:?}");
        let in_pond = (8.0..16.0).contains(&plant[0]) && (8.0..16.0).contains(&plant[2]);
        assert!(!in_pond, "a plant in the pond: {plant:?}");
    }
    assert!(oaks.iter().all(|oak| oak[0] <= 24.0), "an oak on the bank, steeper than its 30°");
    // Each rule's scales stay within its own.
    for plant in all.iter().filter(|plant| plant.0 == 0) {
        let scale = ScatterInstance { position: [0.0; 3], packed: plant.3 }.scale();
        assert!((0.79..=1.31).contains(&scale), "an oak at {scale}");
    }
}

/// IDs are a rule's name and a lattice square's: a reseed moves its plants and keeps their IDs, another rule
/// leaves them alone, and a rename makes them new.
#[test]
fn ids_are_stable() {
    let grove = grove();
    let grid = Grid { cell_size: 32.0 };
    let cells = cells_of(&grove, grid);
    let ids = |foliage: &FoliageDocument, kind: u32| -> BTreeSet<u64> {
        plants(foliage, grid, &cells).iter().filter(|plant| plant.0 == kind).map(|plant| plant.1).collect()
    };
    let oaks = ids(&grove, 0);
    let mut without_ferns = grove.clone();
    without_ferns.rules.truncate(1);
    assert_eq!(ids(&without_ferns, 0), oaks, "the ferns' rule moves no oak");
    let mut renamed = grove.clone();
    renamed.rules[0].name = "old oaks".to_owned();
    assert!(ids(&renamed, 0).is_disjoint(&oaks), "a renamed rule is a new one");
    let mut reseeded = grove.clone();
    reseeded.rules[0].seed = 4;
    let moved = ids(&reseeded, 0);
    assert!(moved.intersection(&oaks).count() > 0, "a reseed keeps the lattice's IDs where plants are kept");
}

/// The bytes a cook writes for the grove's cells, pinned: placement is integer hashing and correctly rounded
/// arithmetic, so every architecture's CI must write these (cooker rule 8).
#[test]
fn the_groves_sets_are_the_same_bytes_on_every_architecture() {
    let grove = grove();
    let grid = Grid { cell_size: 32.0 };
    let mut digest = xxhash_rust::xxh3::Xxh3::new();
    for cell in cells_of(&grove, grid) {
        digest.update(
            scatter_set(&grove, &ground, grid, cell).expect("places").to_cooked().expect("writes").as_slice(),
        );
    }
    assert_eq!(digest.digest(), 0xa6ff_1338_f9b5_8e14, "if placement changed on purpose, pin this");
}

#[test]
fn a_document_the_cook_cant_place_is_refused_by_its_field() {
    type Break = fn(&mut FoliageDocument);
    let cases: [(&str, Break); 16] = [
        ("kinds[1].name", |f| f.kinds[1].name = "oak".to_owned()),
        ("layers[0].outlines[1]", |f| f.layers[0].outlines[1].truncate(2)),
        ("layers[1].outlines[0][2]", |f| f.layers[1].outlines[0][2] = [f32::NAN, 0.0]),
        ("rules[0].kind", |f| f.rules[0].kind = "elm".to_owned()),
        ("rules[0].on[0]", |f| f.rules[0].on[0] = "moor".to_owned()),
        ("rules[0].on", |f| f.rules[0].on.clear()),
        ("rules[0].per_100m2", |f| f.rules[0].per_100m2 = 2.0),
        ("rules[0].spacing", |f| f.rules[0].spacing = 0.1),
        ("rules[0].slope_degrees", |f| f.rules[0].slope_degrees = [40.0, 30.0]),
        ("rules[0].cluster.contrast", |f| f.rules[0].cluster.as_mut().expect("there").contrast = 1.5),
        ("rules[0].scale", |f| f.rules[0].scale = [0.2, 1.0]),
        ("rules[0].avoid[0].kinds[0]", |f| {
            f.rules[0].avoid = vec![Avoid::Kinds { kinds: vec!["fern".to_owned()], distance: 1.0 }];
        }),
        ("rules[1].avoid[0].distance", |f| {
            f.rules[1].avoid[0] = Avoid::Kinds { kinds: vec!["oak".to_owned()], distance: 100.0 };
        }),
        ("rules[1].avoid[1].shape", |f| {
            f.rules[1].avoid[1] =
                Avoid::Area { position: [0.0; 3], shape: ZoneShape::Sphere { radius: 0.0 } };
        }),
        ("grass[0].per_m2", |f| f.grass[0].per_m2 = 100.0),
        ("grass[0].name", |f| f.grass[0].name = "oaks".to_owned()),
    ];
    for (field, mut_document) in cases {
        let mut broken = grove();
        mut_document(&mut broken);
        let message = broken.check().expect_err(field).to_string();
        assert!(message.contains(&format!("`{field}`")), "{field}: {message}");
    }
    let unknown = GROVE.replace("seed: 3,", "seed: 3, shade: 1.0,");
    assert!(FoliageDocument::from_ron(&unknown).is_err(), "an unknown field is an error");
}

#[test]
fn cooked_kinds_read_back_and_refuse_what_the_document_would() {
    let cooked = grove().cook();
    let bytes = cooked.to_cooked().expect("writes");
    let read = Foliage::read(&bytes).expect("reads");
    assert_eq!(read.meshes(), grove().kinds.iter().map(|kind| kind.model.mesh()).collect::<Vec<_>>());
    assert_eq!(read.kinds[0].model.to_native(), grove().kinds[0].model);
    let mut twin = cooked.clone();
    twin.kinds[1] = twin.kinds[0].clone();
    assert!(Foliage::read(&twin.to_cooked().expect("writes")).is_err(), "two kinds alike");
    let mut renamed = cooked;
    renamed.kinds[0].name = "elm".to_owned();
    assert!(Foliage::read(&renamed.to_cooked().expect("writes")).is_err(), "an ID that isn't its name's");
}

#[test]
fn a_set_another_cook_wouldnt_write_is_refused() {
    let grove = grove();
    let set =
        scatter_set(&grove, &ground, Grid { cell_size: 32.0 }, CellCoord { x: -1, z: -1 }).expect("places");
    assert!(
        set.runs.len() == 3 && set.chunks.len() > 3,
        "{} runs, {} chunks",
        set.runs.len(),
        set.chunks.len()
    );
    ScatterSet::read(&set.to_cooked().expect("writes")).expect("reads");
    type Tamper = fn(&mut ScatterSet);
    let cases: [(&str, Tamper); 7] = [
        ("an ID short", |set| {
            set.ids.pop();
        }),
        ("an ID twice", |set| {
            let chunk = *set.chunks.iter().find(|chunk| chunk.count > 1).expect("a chunk of two");
            set.ids[chunk.first as usize + 1] = set.ids[chunk.first as usize];
        }),
        ("runs out of order", |set| set.runs.swap(0, 1)),
        ("a root out of its chunk", |set| set.instances[0].position[0] += CHUNK_SIZE),
        ("a root past its bounds", |set| set.chunks[0].roots_max[1] = set.chunks[0].roots_min[1] - 1.0),
        ("a chunk short", |set| set.chunks[0].count -= 1),
        ("a root not finite", |set| set.instances[1].position[2] = f32::NAN),
    ];
    for (what, tamper) in cases {
        let mut tampered = set.clone();
        tamper(&mut tampered);
        assert!(ScatterSet::read(&tampered.to_cooked().expect("writes")).is_err(), "{what}");
    }
}

/// A 64 m cell of grass at 16 a square metre, with oaks and ferns on top: about 65,000 plants, every ID
/// unique within the set (32-bit IDs would expect about half a match here), and the set reads.
#[test]
fn a_dense_cells_ids_are_unique() {
    let dense = GROVE
        .replace(
            "(name: \"meadow\", outlines: [[(-40.0, -40.0), (0.0, -40.0), (0.0, 0.0), (-40.0, 0.0)]]),",
            "(name: \"meadow\", outlines: [[(0.0, 0.0), (64.0, 0.0), (64.0, 64.0), (0.0, 64.0)]]),",
        )
        .replace("per_m2: 1.5,", "per_m2: 16.0,")
        .replace("cluster: Some((scale: 6.0, contrast: 0.8)),", "");
    let dense = FoliageDocument::from_ron(&dense).expect("the dense grove reads");
    let flat = |_: f32, _: f32| Some(GroundPoint { height: 0.0, normal: [0.0, 1.0, 0.0] });
    let set = scatter_set(&dense, &flat, Grid { cell_size: 64.0 }, CellCoord { x: 0, z: 0 }).expect("places");
    assert!(set.ids.len() > 60_000, "{} plants", set.ids.len());
    let unique: BTreeSet<u64> = set.ids.iter().copied().collect();
    assert_eq!(unique.len(), set.ids.len(), "IDs repeat");
    ScatterSet::read(&set.to_cooked().expect("writes")).expect("reads");
}
