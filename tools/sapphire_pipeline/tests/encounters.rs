//! A scenario's zone and a cook's are one zone (M3 B2): a scenario document archives what it reads of its zone
//! with no pipeline, so a replay needs no assets, and this holds that to what a real cook writes. Through the
//! same door, `Simulation::set_zone`, they give one `tables_hash`, and a run of one has the other's checksums.

use std::path::{Path, PathBuf};
use std::{env, fs};

use sapphire_gameplay::replay::{
    SCENARIOS, ULTIMATE_CINEMATIC, add_warrior, find_scenario, player_moves, ultimate_cinematic,
};
use sapphire_gameplay::{ActorSpawn, Faction, Simulation, TickActions, ZoneCast};
use sapphire_pipeline::cook::{Cook, CookOptions, cook_dir};
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_zone::Zone;

fn content(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content").join(path)
}

fn aligned(path: &Path) -> AlignedVec {
    let mut bytes = AlignedVec::new();
    bytes.extend_from_slice(&fs::read(path).expect("a cooked file"));
    bytes
}

#[test]
fn a_cooked_zone_is_what_a_scenario_reads() {
    // The real documents, by themselves, through a real cook.
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("encounters");
    let (source, out) = (scratch.join("source"), scratch.join("out"));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&source).expect("a scratch directory");
    for entry in fs::read_dir(content("zones/square")).expect("the square's zone") {
        let path = entry.expect("an entry").path();
        fs::copy(&path, source.join(path.file_name().expect("a name"))).expect("copies");
    }
    let colliders = content("arena/square.colliders.ron");
    fs::copy(&colliders, source.join("square.colliders.ron")).expect("copies");
    fs::copy(content("arena/square.colliders.ron.meta"), source.join("square.colliders.ron.meta"))
        .expect("copies");
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
    let report = cook_dir(&source, &out, &options).expect("cooks");

    let output_named = |suffix: &str| {
        let found = report.assets.iter().find(|asset| asset.output.to_string_lossy().ends_with(suffix));
        found.unwrap_or_else(|| panic!("no {suffix} in {:?}", report.assets))
    };
    let zone = aligned(&output_named(".szone").output);
    let encounter = output_named(".sencounter");
    let encounter_id: AssetId = encounter.id;
    let encounter = aligned(&encounter.output);
    let colliders = aligned(&output_named(".scol").output);

    let scenario = find_scenario(&SCENARIOS, "arena-waves").expect("listed");
    let (mut read, read_cast) = scenario.start(scenario.config).expect("sets up");

    let mut cooked = Simulation::new(scenario.config);
    let set = sapphire_colliders::ColliderSet::read(&colliders).expect("reads");
    let physics = sapphire_physics::PhysicsWorld::from_cooked(set, cooked.body_capacity()).expect("builds");
    cooked.set_physics_world(physics).expect("room");
    // The player's moves name their ultimate's cinematic, which every fight on them loads (M3 Q3b).
    cooked.add_cinematic(ULTIMATE_CINEMATIC, ultimate_cinematic().expect("reads")).expect("room");
    let controller = Some(cooked.add_controller_profile(&sapphire_gameplay::arena::profile()).expect("good"));
    let player = cooked.add_move_set(player_moves().expect("reads"));
    let warrior = add_warrior(&mut cooked).expect("compiles");
    let moves = [("player", player), ("warrior", warrior)];
    let cast = ZoneCast { moves: &moves, controller };
    cooked
        .set_zone(Zone::read(&zone).expect("reads"), cast, |id| {
            (id == encounter_id).then_some(encounter.as_slice())
        })
        .expect("takes it");
    let spot = sapphire_gameplay::arena::spawns().player;
    let actor = cooked
        .spawn(ActorSpawn {
            position: spot.position(),
            yaw: spot.yaw(),
            moves: Some(player),
            controller,
            faction: Faction::new(1).expect("a side"),
        })
        .expect("room");
    // A scenario's fighters start with full meters (M3 Q3c).
    cooked.fill_energy(actor);
    cooked.give_soft_lock(actor);

    assert_eq!(cooked.tables_hash(), read.tables_hash(), "one zone, one set of tables");
    assert_eq!(actor, read_cast[0]);
    for tick in 0..600 {
        read.step(&TickActions::NEUTRAL);
        cooked.step(&TickActions::NEUTRAL);
        assert_eq!(cooked.checksum(), read.checksum(), "at tick {tick}");
    }
    assert!(read.zone().expect("it has one").stats().spawned > 0, "and something came of it");
}
