//! The district's documents, generated but not cooked: what each preset makes, against the spec's budgets.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use sapphire_assets::{AssetBudgets, AssetKind};

use super::*;

/// The repository's `bench/district/`.
fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/district")
}

/// What a preset generates, read back.
struct Generated {
    district: BenchDistrict,
    plan: Plan,
    zone: ZoneDocument,
    sources: BTreeMap<String, Source>,
}

fn generated(preset: Preset) -> Generated {
    let district: BenchDistrict = read_document(&dir().join("district.ron")).unwrap();
    let sources: BTreeMap<String, Source> =
        generate(&dir(), preset).unwrap().into_iter().map(|source| (source.path.clone(), source)).collect();
    let zone = sapphire_serialize::from_ron(std::str::from_utf8(&sources[ZONE].bytes).unwrap()).unwrap();
    let plan = Plan::new(&district, district.presets.get(preset).cells);
    Generated { district, plan, zone, sources }
}

/// The materials a source's sidecar binds.
fn bound(source: &Source) -> Vec<AssetId> {
    match &source.sidecar {
        Some((ImportSettings::Assembly(settings), _)) => settings.materials.values().copied().collect(),
        _ => Vec::new(),
    }
}

/// A lot's ground, as its turn lays its footprint.
fn lot_rect(district: &BenchDistrict, lot: &Lot) -> Rect {
    let [width, depth] = district.buildings.footprints[lot.footprint];
    let across = lot.turn == 90.0 || lot.turn == 270.0;
    Rect::about(lot.middle, if across { [depth * 0.5, width * 0.5] } else { [width * 0.5, depth * 0.5] })
}

/// Lights whose reach meets the view from `eye` along `forward` (unit), out to `far` metres: a
/// camera's frustum seen from above, 55° high at 16:9 (the zone walk's), so about 42.8° either side.
fn lights_in_view(lights: &[Light], eye: [f32; 2], forward: [f32; 2], far: f32) -> usize {
    const SIN: f32 = 0.6794;
    const COS: f32 = 0.7337;
    lights
        .iter()
        .filter(|light| {
            let to = [light.position[0] - eye[0], light.position[2] - eye[1]];
            let ahead = to[0] * forward[0] + to[1] * forward[1];
            let side = (to[0] * forward[1] - to[1] * forward[0]).abs();
            ahead > -light.range && ahead < far + light.range && side * COS - ahead * SIN <= light.range
        })
        .count()
}

/// Bytes a set's three maps take at `size` texels with every level: BC7 and BC5 are a byte a texel, in
/// 4 × 4 blocks, so a level under 4 texels still takes a block.
fn set_bytes(size: u32) -> u64 {
    let mut bytes = 0;
    let mut side = size;
    loop {
        let blocks = u64::from(side.div_ceil(4));
        bytes += blocks * blocks * 16;
        if side == 1 {
            break;
        }
        side /= 2;
    }
    3 * bytes
}

#[test]
fn the_budget_district_holds_the_specs_counts() {
    let Generated { district, plan, zone, sources } = generated(Preset::Budget);
    let grounds =
        zone.placements.iter().filter(|p| p.prefab == asset_id(PART, &prefab_path("ground"))).count();
    let cells: BTreeSet<_> = zone.placements.iter().map(|p| plan.cell_of([p.at[0], p.at[2]])).collect();
    assert_eq!((grounds, cells.len()), (64, 64), "8 × 8 cells, each with its ground");
    assert_eq!(zone.grid.cell_size, 32.0);

    let meshes: BTreeSet<_> = zone.instances.iter().map(|instance| instance.mesh).collect();
    let variants = sources.keys().filter(|path| path.starts_with("buildings/")).count();
    assert_eq!((meshes.len(), variants), (40, 40), "forty building variants, every one standing");
    assert_eq!(zone.instances.len(), plan.lots.len());

    let props = zone.placements.len() - grounds;
    eprintln!(
        "budget: {} cells, {} buildings of {} variants, {props} props {:?}, {} lights, {} closures",
        cells.len(),
        zone.instances.len(),
        meshes.len(),
        plan.prop_counts(),
        zone.lights.len(),
        plan.closures.len(),
    );
    assert!((6000..=10_000).contains(&props), "{props} props");
    assert!(zone.lights.iter().all(|light| light.at_night && light.shape == LightShape::Point));
    assert_eq!(zone.lights.len(), plan.lights.len());
    assert_eq!(zone.splines.iter().map(|spline| spline.name.as_str()).collect::<Vec<_>>(), [TOUR]);
    // Every variant's slots and every cell's palette within a cell's mesh's sixteen materials.
    assert!(zone.placements.iter().all(|placement| placement.materials.len() <= 3));
    assert_eq!(district.palette, 16);
}

#[test]
fn the_tour_passes_every_variant_and_every_set() {
    for preset in [Preset::Ci, Preset::Budget, Preset::Double] {
        let Generated { district, plan, zone, sources } = generated(preset);
        let sets = district.presets.get(preset).texture_sets;
        // What a walk along it has in view close by.
        const NEAR: f32 = SEEN;
        let mut seen = BTreeSet::new();
        let mut standing = BTreeSet::new();
        for instance in &zone.instances {
            if plan.from_tour([instance.at[0], instance.at[2]]) <= NEAR {
                standing.insert(instance.mesh);
                let path = (0..district.buildings.variants)
                    .map(building_path)
                    .find(|path| asset_id(PART, path) == instance.mesh)
                    .unwrap();
                seen.extend(bound(&sources[&path]));
            }
        }
        let tour_cells = plan.tour_cells();
        for placement in &zone.placements {
            let ground = placement.prefab == asset_id(PART, &prefab_path("ground"));
            // A cell's ground is seen from anywhere in it.
            let at = if ground { plan.cell_of([placement.at[0], placement.at[2]]) } else { [u32::MAX; 2] };
            if plan.from_tour([placement.at[0], placement.at[2]]) <= NEAR || tour_cells.contains(&at) {
                seen.extend(placement.materials.values().copied());
            }
        }
        if preset != Preset::Ci {
            assert_eq!(
                standing.len(),
                district.buildings.variants as usize,
                "{preset}: a variant stands away"
            );
        }
        for set in 0..sets {
            let id = asset_id(PART, &format!("materials/set_{set:03}.material.ron"));
            assert!(seen.contains(&id), "{preset}: the tour doesn't pass set {set}");
        }
    }
}

#[test]
fn the_lights_meet_both_tiers_along_the_tour() {
    let Generated { plan, zone, .. } = generated(Preset::Budget);
    // The long view: west along the street from the district's east end, inside desktop's 256 m.
    let (eye, to) = (plan.tour[2], plan.tour[3]);
    let length = distance(eye, to);
    let forward = [(to[0] - eye[0]) / length, (to[1] - eye[1]) / length];
    let long = lights_in_view(&zone.lights, eye, forward, 256.0);
    // Along the tour, looking along it, inside mobile's 128 m: all but where a leg ends facing the boundary
    // wall or a block's side, a few metres from it.
    let (mut least, mut under, mut points) = ((usize::MAX, [0.0; 2]), 0, 0);
    for leg in plan.tour.windows(2) {
        let length = distance(leg[0], leg[1]);
        let forward = [(leg[1][0] - leg[0][0]) / length, (leg[1][1] - leg[0][1]) / length];
        let steps = (length / 2.0).ceil() as u32;
        for step in 0..steps {
            let along = step as f32 * 2.0;
            let eye = [leg[0][0] + forward[0] * along, leg[0][1] + forward[1] * along];
            let seen = lights_in_view(&zone.lights, eye, forward, 128.0);
            if seen < least.0 {
                least = (seen, eye);
            }
            points += 1;
            under += usize::from(seen < 8);
        }
    }
    eprintln!(
        "lights in view: {long} on the long view; along the tour at least {least:?}, under 8 at {under} of {points} \
         points, {} in all",
        zone.lights.len()
    );
    assert!(long >= 64, "the long view meets {long} lights, under desktop's 64");
    assert!(under * 50 <= points, "{under} of {points} points along the tour see under mobile's 8 lights");
}

#[test]
fn the_budgets_textures_fill_the_pool_and_double_twice() {
    let district: BenchDistrict = read_document(&dir().join("district.ron")).unwrap();
    let pool = AssetBudgets::DESKTOP.resident[AssetKind::Texture.index()];
    let bytes = |preset: Preset| {
        let scale = district.presets.get(preset);
        u64::from(scale.texture_sets) * set_bytes(scale.texture_size)
    };
    let (budget, double) = (bytes(Preset::Budget), bytes(Preset::Double));
    // Within a hundredth: a set's smallest levels round up to whole blocks.
    assert!(budget.abs_diff(pool) * 100 <= pool, "{budget} bytes against the pool's {pool}");
    assert!(double.abs_diff(2 * pool) * 100 <= 2 * pool, "{double} bytes against twice the pool");
    assert!(
        generated(Preset::Budget).sources.keys().filter(|path| path.starts_with("materials/")).count() == 160
    );
    eprintln!(
        "textures: budget {budget} bytes, double {double}, desktop pool {pool}, mobile pool {}",
        AssetBudgets::MOBILE.resident[AssetKind::Texture.index()]
    );
}

#[test]
fn the_zones_simulation_is_the_codes() {
    for preset in [Preset::Ci, Preset::Budget] {
        let Generated { district, zone, sources, .. } = generated(preset);
        let cells = district.presets.get(preset).cells;
        let code = simulation::simulation(cells, asset_id(PART, ENCOUNTER));
        assert_eq!(zone.simulation().unwrap(), code, "{preset}");
        assert_eq!(sources[ENCOUNTER].bytes, document_bytes(&simulation::encounter()), "{preset}");
        assert_eq!(zone.encounters, [asset_id(PART, ENCOUNTER)]);
    }
}

#[test]
fn the_fight_stands_clear_in_the_plaza() {
    for preset in [Preset::Ci, Preset::Budget] {
        let Generated { district, plan, zone, .. } = generated(preset);
        let mut taken: Vec<Rect> = plan.lots.iter().map(|lot| lot_rect(&district, lot)).collect();
        for prop in &plan.props {
            let [x, z] = district.props.get(&prop.prefab).map_or([0.0; 2], |prop| prop.clear);
            let across = prop.turn == 90.0 || prop.turn == 270.0;
            taken.push(Rect::about(prop.at, if across { [z, x] } else { [x, z] }));
        }
        let plaza = plan.plaza();
        for spawner in &zone.spawners {
            let at = [spawner.position[0], spawner.position[2]];
            // Half a metre about it, for a body.
            let body = Rect::about(at, [0.5, 0.5]);
            assert!(
                !taken.iter().any(|rect| rect.overlaps(&body)),
                "{preset}: {} stands in something",
                spawner.name
            );
            if spawner.name.starts_with("plaza_") {
                assert!(plaza.contains(at), "{preset}: {} stands outside the plaza", spawner.name);
            }
        }
        // The plaza's volume lies inside its paving, and holds nothing but its rim's furniture.
        let trigger = zone.triggers.iter().find(|trigger| trigger.name == "plaza").unwrap();
        let sapphire_zone::document::Volume::Box { half_extents } = trigger.volume else { panic!("a box") };
        let volume =
            Rect::about([trigger.position[0], trigger.position[2]], [half_extents[0], half_extents[2]]);
        assert!(
            plaza.contains(volume.min) && plaza.contains(volume.max),
            "{preset}: {volume:?} in {plaza:?}"
        );
        assert!(!plan.lots.iter().any(|lot| lot_rect(&district, lot).overlaps(&volume)));
    }
}

#[test]
fn the_ci_district_is_small_and_has_every_kind_of_thing() {
    let Generated { district, plan, zone, sources } = generated(Preset::Ci);
    let counts = plan.prop_counts();
    for name in district.props.keys().map(String::as_str).chain(["closure"]) {
        assert!(counts.get(name).is_some_and(|count| *count > 0), "ci places no `{name}`");
    }
    assert_eq!(
        zone.placements.iter().filter(|p| p.prefab == asset_id(PART, &prefab_path("ground"))).count(),
        16
    );
    assert!(!zone.instances.is_empty() && !zone.lights.is_empty());
    assert!(sources.contains_key(DAY) && sources[DAY].sidecar.is_none());
    eprintln!(
        "ci: {} buildings, {} props {counts:?}, {} lights, {} sources",
        zone.instances.len(),
        zone.placements.len() - 16,
        zone.lights.len(),
        sources.len()
    );
}

#[test]
fn a_wrong_document_is_an_error() {
    let district: BenchDistrict = read_document(&dir().join("district.ron")).unwrap();
    let scale = district.presets.budget;
    assert!(check(&district, scale).is_ok());
    assert!(check(&district, DistrictScale { cells: 7, ..scale }).is_err(), "odd");
    assert!(check(&district, DistrictScale { cells: 2, ..scale }).is_err(), "too few");
    assert!(
        check(&BenchDistrict { cell_size: 30.0, ..district.clone() }, scale).is_err(),
        "not the code's cells"
    );
    let mut same = district.clone();
    same.buildings.storeys = vec![1, 2, 3, 4, 5];
    assert!(check(&same, scale).is_err(), "variants that come out the same");
    let mut missing = district.clone();
    missing.dressing.frontage.push("fountain".to_owned());
    assert!(check(&missing, scale).is_err(), "a prop it doesn't hold");
    // Too many sets for the tour to pass.
    let plan = Plan::new(&district, 4);
    assert!(Assignment::new(&plan, &district.buildings, 1, 1000, &BTreeMap::new()).is_err());
}
