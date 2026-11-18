//! The nature zone's documents, generated but not cooked: its simulation half is the code's, its ground flat
//! where the fight stands, its walks on land, and what each preset scatters.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;

/// The repository's `bench/nature/`.
fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/nature")
}

fn document() -> BenchNature {
    read_document(&dir().join("nature.ron")).unwrap()
}

/// What a preset generates, by path, and its zone read back.
fn generated(preset: Preset) -> (BTreeMap<String, Source>, ZoneDocument) {
    let sources: BTreeMap<String, Source> = generate(&dir(), preset, Cook::Desktop)
        .unwrap()
        .into_iter()
        .map(|source| (source.path.clone(), source))
        .collect();
    let zone = sapphire_serialize::from_ron(std::str::from_utf8(&sources[ZONE].bytes).unwrap()).unwrap();
    (sources, zone)
}

/// The generated document's simulation half is `sapphire_synthetic_zone::nature`'s at every preset, so one
/// replay of the clearing's fight plays on any, and the IDs the code gives the files the generator writes are
/// the generator's.
#[test]
fn the_zones_simulation_is_the_codes() {
    for preset in [Preset::Ci, Preset::Budget] {
        let (sources, zone) = generated(preset);
        assert_eq!(zone.simulation().unwrap(), simulation::simulation(), "{preset}");
        assert_eq!(sources[ENCOUNTER].bytes, document_bytes(&simulation::encounter()), "{preset}");
        assert_eq!(sources[COLLIDERS].bytes, document_bytes(&simulation::colliders()), "{preset}");
        assert_eq!(asset_id(PART, ENCOUNTER), simulation::encounter_id());
        assert_eq!(zone.colliders, Some(asset_id(PART, COLLIDERS)));
        assert_eq!(asset_id(PART, COLLIDERS), simulation::colliders_id());
    }
}

/// The ground is flat at 0 over the clearing's collided square, where the fight stands, and every water body's
/// surface lies over its bed: the terrain sinks under the lake, the river and the frozen pond.
#[test]
fn the_ground_is_flat_under_the_fight_and_sinks_under_the_water() {
    let nature = document();
    let land = Land::new(&nature);
    let [x, z] = simulation::CLEARING;
    let half = simulation::FLAT_HALF;
    for step in 0..=16 {
        for across in 0..=16 {
            let at = [x - half + step as f32 * half / 8.0, z - half + across as f32 * half / 8.0];
            assert_eq!(land.height(at), 0.0, "the clearing at {at:?}");
        }
    }
    let (lake, _) = simulation::LAKE;
    let (pond, _) = simulation::POND;
    for middle in [lake, pond, [300.0, -220.0], [380.0, -60.0]] {
        assert!(land.water_reach(middle) < 0.0, "{middle:?} is in water");
        assert!(land.height(middle) < simulation::WATER_HEIGHT - 1.0, "the bed at {middle:?}");
    }
    // The ground meets each lake's surface at its outline, the polygon its volume is (along its edges too, not
    // only at its corners), and rises past it: no trench under the drawn surface. Where the river runs into the
    // lake, the edge is the river's water, and the ground is under it.
    let mut shores = 0;
    for body in simulation::water() {
        let WaterShape::Lake { outline, height } = &body.shape else { continue };
        for (k, &corner) in outline.iter().enumerate() {
            let next = outline[(k + 1) % outline.len()];
            for at in [corner, [(corner[0] + next[0]) * 0.5, (corner[1] + next[1]) * 0.5]] {
                assert!(from_outline(at, outline).abs() < 1e-3, "{}: {at:?} is its edge", body.name);
                if land.water_reach(at) < -1e-3 {
                    continue;
                }
                assert!((land.height(at) - height).abs() < 1e-3, "{}: the shore at {at:?}", body.name);
                shores += 1;
            }
        }
    }
    assert!(shores > 50, "{shores} shore points");
}

/// Every walk stays on land, clear of every water body's edge, inside the zone; the nature walk ends in the
/// clearing, where the fight is; each rail is laid at most [`WALK_STEP`] apart at the terrain's height.
#[test]
fn the_walks_keep_to_land_and_the_nature_walk_ends_in_the_clearing() {
    let nature = document();
    let land = Land::new(&nature);
    let (_, zone) = generated(Preset::Ci);
    for walk in &nature.walks {
        let rail = &zone.splines.iter().find(|spline| spline.name == walk.name).unwrap().points;
        for pair in rail.windows(2) {
            let (a, b) = ([pair[0][0], pair[0][2]], [pair[1][0], pair[1][2]]);
            assert!(distance(a, b) <= WALK_STEP + 1e-3, "{}", walk.name);
            assert_eq!(pair[0][1], land.height(a) + walk.lift);
            for t in 0..=4 {
                let at = [a[0] + (b[0] - a[0]) * t as f32 / 4.0, a[1] + (b[1] - a[1]) * t as f32 / 4.0];
                assert!(land.water_reach(at) > WATER_CLEAR, "{} is in the water at {at:?}", walk.name);
            }
        }
    }
    let end = *zone.splines.iter().find(|spline| spline.name == WALK).unwrap().points.last().unwrap();
    let [x, z] = simulation::CLEARING;
    assert!((end[0] - x).abs() < simulation::FLAT_HALF && (end[2] - z).abs() < simulation::FLAT_HALF);
    let length: f32 = nature.walks[0].points.windows(2).map(|leg| distance(leg[0], leg[1])).sum();
    assert!((700.0..900.0).contains(&length), "the nature walk is {length} m");
}

/// The named areas are the row's: a forest block of 1 to 2 km², the coast, the cave, the valley and the frozen
/// pond, which holds the frozen body; each has its marker, and the forest's trees stand only in the forest.
#[test]
fn the_named_areas_are_reserved() {
    let nature = document();
    let names: Vec<&str> = nature.areas.iter().map(|area| area.name.as_str()).collect();
    assert_eq!(names, ["forest", "coast", "cave", "valley", "frozen_pond"]);
    let forest = &nature.areas[0];
    let square_km = (forest.max[0] - forest.min[0]) * (forest.max[1] - forest.min[1]) / 1e6;
    assert!((1.0..=2.0).contains(&square_km), "the forest block is {square_km} km²");
    assert!(nature.areas.iter().filter(|area| area.forest).count() == 1 && forest.forest);
    let (pond, _) = simulation::POND;
    assert!(nature.areas[4].contains(pond));
    let (_, zone) = generated(Preset::Ci);
    assert_eq!(zone.placements.len(), nature.areas.len(), "a marker an area");
}

/// What each preset scatters: the budget's denser than `ci`'s, every tree one of the trees part's models, and
/// a tile a cell; no tree or tuft stands in the water or on the clearing's flat ground.
#[test]
fn a_preset_scatters_trees_and_grass() {
    let nature = document();
    let land = Land::new(&nature);
    let count = |zone: &ZoneDocument, mesh: &dyn Fn(AssetId) -> bool| {
        zone.instances.iter().filter(|i| mesh(i.mesh)).count()
    };
    let trees: Vec<AssetId> = nature
        .trees
        .iter()
        .flat_map(|kind| {
            kind.ages.iter().map(move |age| asset_id("trees", &format!("{0}/{0}_{age}.glb", kind.species)))
        })
        .collect();
    let grass = asset_id(PART, GRASS);
    let mut counted = Vec::new();
    for preset in [Preset::Ci, Preset::Budget] {
        let (sources, zone) = generated(preset);
        let tiles =
            sources.keys().filter(|path| path.starts_with("terrain/") && path.ends_with(".glb")).count();
        assert_eq!(tiles, 32 * 20, "{preset}");
        let tree_count = count(&zone, &|mesh| trees.contains(&mesh));
        let tuft_count = count(&zone, &|mesh| mesh == grass);
        assert_eq!(zone.instances.len(), tiles + tree_count + tuft_count, "{preset}: nothing else");
        for instance in zone.instances.iter().filter(|i| i.mesh == grass || trees.contains(&i.mesh)) {
            let at = [instance.at[0], instance.at[2]];
            assert!(land.water_reach(at) > WATER_CLEAR, "{preset}: {at:?}");
            let [x, z] = simulation::CLEARING;
            assert!((at[0] - x).abs().max((at[1] - z).abs()) > simulation::FLAT_HALF, "{preset}: {at:?}");
        }
        counted.push((tree_count, tuft_count));
    }
    let [(ci_trees, ci_grass), (budget_trees, budget_grass)] = counted[..] else { unreachable!() };
    assert!(ci_trees > 1000 && budget_trees > 3 * ci_trees, "{ci_trees} against {budget_trees}");
    assert!(ci_grass > 500 && budget_grass > 3 * ci_grass, "{ci_grass} against {budget_grass}");
}

/// The look: the day and the weather plan's states ride beside the zone, the plan rains from 19:00, the probe
/// grid sees the terrain and trees inside its box, and every water body's probe is placed.
#[test]
fn the_zone_brings_its_day_weather_and_probes() {
    let (sources, zone) = generated(Preset::Ci);
    assert_eq!(zone.day.as_deref(), Some(DAY));
    let plan = zone.weather_plan.as_ref().unwrap();
    assert_eq!(plan.states, WEATHER);
    assert!(sources.contains_key(DAY) && sources.contains_key(WEATHER));
    let rain = plan.schedule.iter().find(|slot| slot.from == 19.0).unwrap();
    assert_eq!(rain.states[0].state, "rain");
    let grid: ProbeGridDocument =
        sapphire_serialize::from_ron(std::str::from_utf8(&sources[PROBES].bytes).unwrap()).unwrap();
    assert!(grid.meshes.iter().any(|mesh| mesh.mesh == asset_id(PART, "terrain/tile_21_14.glb")));
    assert!(grid.meshes.len() > 30, "{} meshes", grid.meshes.len());
    for body in &zone.water {
        assert!(
            zone.reflections.iter().any(|probe| Some(&probe.name) == body.probe.as_ref()),
            "{}",
            body.name
        );
    }
}

#[test]
fn a_wrong_document_is_an_error() {
    let nature = document();
    let scale = nature.presets.ci;
    assert!(check(&nature, scale).is_ok());
    assert!(
        check(&BenchNature { cell_size: 32.0, ..nature.clone() }, scale).is_err(),
        "not the code's cells"
    );
    let mut over = nature.clone();
    over.areas[3].min = [256.0, 256.0];
    over.areas[3].max = [400.0, 400.0];
    assert!(check(&over, scale).is_err(), "an area over the clearing");
    let mut outside = nature.clone();
    outside.areas[0].min[0] = -2000.0;
    assert!(check(&outside, scale).is_err(), "an area outside the zone");
    let mut no_walk = nature.clone();
    no_walk.walks.retain(|walk| walk.name != WALK);
    assert!(check(&no_walk, scale).is_err(), "no nature walk");
    let mut no_probe = nature.clone();
    no_probe.reflections.retain(|probe| probe.name != "lake");
    assert!(check(&no_probe, scale).is_err(), "the lake reflects no probe");
    assert!(check(&nature, NatureScale { texture_size: 300, ..scale }).is_err(), "not a power of two");
    assert!(check(&nature, NatureScale { grass_spacing: 0.0, ..scale }).is_err(), "no spacing");
}

/// The zone's day and weather states are copies, so a part reads only its folder: each holds its source's
/// document word for word below its own header, `bench/district/district_day.ron` and the shipped presets,
/// `content/weather/presets.weather.ron`. A change there fails here until it's copied (and the zone re-pinned).
#[test]
fn the_day_and_the_weather_states_are_their_sources() {
    /// A document's text below its leading comment block.
    fn body(path: &Path) -> String {
        let text = fs::read_to_string(path).unwrap();
        text.lines().skip_while(|line| line.starts_with("//")).collect::<Vec<_>>().join("\n")
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (copy, source) in [
        (DAY, repo.join("bench/district/district_day.ron")),
        (WEATHER, repo.join("content/weather/presets.weather.ron")),
    ] {
        assert_eq!(body(&dir().join(copy)), body(&source), "{copy} isn't {}", source.display());
    }
}
