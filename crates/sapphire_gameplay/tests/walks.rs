//! Walks through zones and back to their checkpoints (M3 B4): where `replay::walks`' scripts take the
//! player, a teleport that forgets how anyone got there, and a load at a checkpoint that walks on as the
//! teleport did. The scripts' replays (`walk-the-ground`, `from-the-platform`, `walk-the-square`) hold the
//! checksums; this says what those checksums are of.

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_gameplay::replay::walks::{self, Leg};
use sapphire_gameplay::replay::{SCENARIOS, find_scenario};
use sapphire_gameplay::{
    ActorId, CheckpointId, EncounterPhase, GroundMode, Place, Simulation, TeleportError, TickActions,
    ZoneStart, ZoneStartError,
};
use sapphire_math::Vec3;
use sapphire_save::ProgressStore;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// How high a jump from the ground takes the feet at the default profile: 7² / (2 × 22), less what a
/// fixed step leaves of the arc's top.
const JUMP_REACH: f32 = 1.114;

/// The most anyone has risen in the air over where they last stood.
#[derive(Default)]
struct Rises {
    stood: f32,
    most: f32,
}

impl Rises {
    fn see(&mut self, sim: &Simulation, actor: ActorId) {
        let view = sim.actor(actor).expect("there");
        if view.controller.state.mode == GroundMode::Airborne {
            self.most = self.most.max(view.position.y - self.stood);
        } else {
            self.stood = view.position.y;
        }
    }
}

fn start(name: &str) -> (Simulation, Vec<ActorId>) {
    let scenario = find_scenario(&SCENARIOS, name).expect("listed");
    scenario.start(scenario.config).expect("sets up")
}

/// Steps `sim` through `ticks` ticks of `script`, from its tick 0, calling `each` after every step.
fn run(
    sim: &mut Simulation,
    cast: &[ActorId],
    script: fn(u64, &[ActorId]) -> TickActions,
    ticks: u32,
    mut each: impl FnMut(&Simulation),
) {
    for tick in 0..u64::from(ticks) {
        sim.step(&script(tick, cast));
        each(sim);
    }
}

fn place_of(sim: &Simulation, actor: ActorId) -> Place {
    let view = sim.actor(actor).expect("there");
    Place { position: view.position, yaw: view.yaw }
}

fn latest_name(sim: &Simulation) -> Option<String> {
    let zone = sim.zone().expect("a zone");
    zone.latest_checkpoint().and_then(|id| zone.checkpoint_name(id)).map(str::to_owned)
}

/// [`walks::walk_the_ground`] up to its return.
fn walked_out() -> (Simulation, Vec<ActorId>) {
    let (mut sim, cast) = start("ground-walk");
    run(&mut sim, &cast, walks::walk_the_ground, walks::GROUND_RETURN_AT, |_| {});
    (sim, cast)
}

#[test]
fn the_scripts_name_the_platform_by_its_place() {
    let (sim, _) = start("ground-walk");
    assert_eq!(sim.zone().expect("a zone").checkpoint("platform"), Some(walks::PLATFORM));
    let (ward, _) = start("lantern-walk");
    assert_eq!(ward.zone().expect("a zone").checkpoint("court"), Some(walks::WARD_COURT));
}

/// The Lantern Ward's walk goes the length of its tour on its real ground: to the Market Court's far end
/// and the Blossom Garden's, down the temple's lane, through the court and up the terrace's steps,
/// reaching each checkpoint on the way in order, and pressing into nothing the controller turns it from.
/// Then it comes back to the court and runs up it.
#[test]
fn the_ward_walk_goes_the_whole_ward() {
    let (mut sim, cast) = start("lantern-walk");
    let player = cast[0];
    let (mut east, mut west, mut highest) = (f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    let mut reached = Vec::new();
    run(&mut sim, &cast, walks::walk_the_ward, walks::WARD_RETURN_AT, |sim| {
        let view = sim.actor(player).expect("there");
        east = east.max(view.position.x);
        west = west.min(view.position.x);
        highest = highest.max(view.position.y);
        if let Some(name) = latest_name(sim)
            && reached.last() != Some(&name)
        {
            reached.push(name);
        }
    });
    assert_eq!(reached, ["start", "market", "garden", "court"]);
    assert!(east > 72.0 && west < -63.5, "it reached both rooms' far ends: {east}, {west}");
    let end = place_of(&sim, player).position;
    assert!(Vec3::new(end.x, 0.0, end.z + 105.0).length() < 0.5, "it ended on the terrace: {end}");
    assert!((end.y - 1.2).abs() < 1e-3, "standing on its top: {end}");
    assert!(highest < 1.35, "it went up the steps, not over anything: {highest}");
    let view = sim.actor(player).expect("there");
    assert_eq!(view.controller.state.mode, GroundMode::Grounded);
    assert_eq!(sim.stats().controllers_refused, 0);

    // The return, as the replay's command makes it: to the court's checkpoint, and up the court.
    let court = sim.zone().expect("a zone").checkpoint_place(walks::WARD_COURT).expect("the court");
    let actions =
        |tick: u64, cast: &[ActorId]| walks::walk_the_ward(tick + u64::from(walks::WARD_RETURN_AT), cast);
    sim.return_to_checkpoint(walks::WARD_COURT).expect("it returns");
    assert_eq!(place_of(&sim, player), court);
    run(&mut sim, &cast, actions, walks::WARD_WALK_TICKS - walks::WARD_RETURN_AT, |_| {});
    let end = place_of(&sim, player).position;
    assert!(end.z < court.position.z - 10.0, "it ran up the court: {end}");
    assert_eq!(latest_name(&sim).as_deref(), Some("court"));
    assert_eq!(sim.actor(player).expect("there").teleports, 1);
}

/// The walk out goes up the ramp, over the platform, down its steps, onto the curb's edge, over the wall and
/// into the face too steep to stand on, reaching every checkpoint on the way and rising no more than a jump.
#[test]
fn the_walk_out_crosses_the_whole_course() {
    let (mut sim, cast) = start("ground-walk");
    let player = cast[0];
    let (mut on_platform, mut slid, mut furthest, mut rises) =
        (false, false, f32::INFINITY, Rises::default());
    let mut reached = Vec::new();
    run(&mut sim, &cast, walks::walk_the_ground, walks::GROUND_RETURN_AT, |sim| {
        let view = sim.actor(player).expect("there");
        let state = view.controller.state;
        on_platform |= state.mode == GroundMode::Grounded && (view.position.y - 1.0).abs() < 1e-3;
        slid |= state.mode == GroundMode::Sliding;
        furthest = furthest.min(view.position.x);
        rises.see(sim, player);
        if let Some(name) = latest_name(sim)
            && reached.last() != Some(&name)
        {
            reached.push(name);
        }
    });
    assert!(on_platform, "it stood on the platform");
    assert!(slid, "it slid down the steep face");
    assert!(furthest < -29.5, "it reached the face's foot: {furthest}");
    assert!(rises.most <= JUMP_REACH + 1e-3, "nothing threw it over a jump's height: {}", rises.most);
    assert!(rises.most >= JUMP_REACH - 1e-3, "it jumped: {}", rises.most);
    assert_eq!(reached, ["ramp_foot", "platform", "lane_end"]);
    assert_eq!(sim.stats().controllers_refused, 0);
}

/// Nothing met in the air lifts anyone (M3 B4): jumps at each of the course's edges, a tick apart, and at
/// the square's bench, never rise faster than they were, nor higher than a jump from where they stood.
#[test]
fn nothing_met_in_the_air_throws_anyone() {
    let out = Leg { stick: (0, -127), ticks: 208, jump: None };
    let over = Leg { stick: (-127, 0), ticks: 300, jump: Some(263) };
    let mut tried: Vec<(&str, Vec<Leg>)> = Vec::new();
    for at in 186..230 {
        tried.push(("ground-walk", vec![out, Leg { stick: (-127, 0), ticks: 270, jump: Some(at) }]));
    }
    for at in 228..280 {
        tried.push(("ground-walk", vec![out, Leg { stick: (-127, 0), ticks: 330, jump: Some(at) }]));
    }
    for at in 0..24 {
        tried.push(("ground-walk", vec![out, over, Leg { stick: (-127, 0), ticks: 80, jump: Some(at) }]));
    }
    for at in 196..240 {
        tried.push(("square-walk", vec![Leg { stick: (0, -127), ticks: 300, jump: Some(at) }]));
    }
    for (scenario, legs) in &tried {
        let (mut sim, cast) = start(scenario);
        let player = cast[0];
        let mut before: Option<(GroundMode, f32)> = None;
        let mut rises = Rises::default();
        for tick in 0..u64::from(walks::length(legs)) {
            sim.step(&walks::walk(legs, tick, &cast));
            let view = sim.actor(player).expect("there");
            let state = view.controller.state;
            let label = format!("{scenario} {legs:?}, tick {tick}");
            rises.see(&sim, player);
            assert!(rises.most <= JUMP_REACH + 1e-3, "{label}: thrown {} m up", rises.most);
            if let Some((mode, rising)) = before
                && mode == GroundMode::Airborne
                && state.mode == GroundMode::Airborne
                && !state.events.jumped
            {
                let most = rising.max(0.0) + 0.01;
                assert!(state.velocity.y <= most, "{label}: lifted from {rising} to {:?}", state.velocity);
            }
            before = Some((state.mode, state.velocity.y));
        }
    }
}

/// From the return on, two runs that got to the platform by different roads are one run: the whole
/// checksum, every tick.
#[test]
fn a_teleport_forgets_how_anyone_got_there() {
    let (mut walked, cast) = walked_out();
    let (mut stood, stood_cast) = start("ground-walk");
    run(&mut stood, &stood_cast, |_, _| TickActions::NEUTRAL, walks::GROUND_RETURN_AT, |_| {});
    assert_eq!(cast, stood_cast);
    assert_ne!(walked.checksum(), stood.checksum(), "they went different ways");

    for sim in [&mut walked, &mut stood] {
        assert_eq!(sim.return_to_checkpoint(walks::PLATFORM), Ok(cast[0]));
    }
    let ticks = walks::length(&walks::GROUND_ONWARD_LEGS);
    for tick in 0..u64::from(ticks) {
        let actions = walks::from_the_platform(tick, &cast);
        walked.step(&actions);
        stood.step(&actions);
        assert_eq!(walked.checksum(), stood.checksum(), "tick {tick} after the return");
    }
}

/// A game loaded at the platform (the `ground-platform` scenario, which starts its zone through
/// `Simulation::start_at`) walks on exactly as one that returned there: the player's place, yaw and
/// controller, to the bit, every tick. The rest of the checksum can't follow, since it counts ticks.
#[test]
fn a_load_at_a_checkpoint_walks_on_as_the_teleport_did() {
    let (mut walked, cast) = walked_out();
    walked.return_to_checkpoint(walks::PLATFORM).expect("returns");
    let (mut loaded, loaded_cast) = start("ground-platform");
    assert_eq!(place_of(&walked, cast[0]), place_of(&loaded, loaded_cast[0]), "they stand in one place");

    let ticks = walks::length(&walks::GROUND_ONWARD_LEGS);
    for tick in 0..u64::from(ticks) {
        walked.step(&walks::from_the_platform(tick, &cast));
        loaded.step(&walks::from_the_platform(tick, &loaded_cast));
        let (a, b) = (walked.actor(cast[0]).expect("there"), loaded.actor(loaded_cast[0]).expect("there"));
        assert_eq!(
            a.position.to_array().map(f32::to_bits),
            b.position.to_array().map(f32::to_bits),
            "tick {tick}"
        );
        assert_eq!(a.yaw.to_bits(), b.yaw.to_bits(), "tick {tick}");
        assert_eq!(a.controller.state, b.controller.state, "tick {tick}");
        assert_eq!(latest_name(&walked), latest_name(&loaded), "tick {tick}");
    }
}

/// What a save holds at the platform goes through a real save file and comes back as what the load there
/// starts from.
#[test]
fn a_saved_zone_start_comes_back_as_the_load_starts() {
    let (mut walked, cast) = walked_out();
    assert_eq!(latest_name(&walked).as_deref(), Some("lane_end"));
    walked.return_to_checkpoint(walks::PLATFORM).expect("returns");
    walked.step(&walks::from_the_platform(0, &cast));
    let saved = walked.zone_start().expect("a zone");
    assert_eq!(saved, ZoneStart { checkpoint: Some("platform".to_owned()), cleared: Vec::new() });

    let dir = std::env::temp_dir().join(format!("sapphire_gameplay-walks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory of its own");
    let store = ProgressStore::<ZoneStart>::new(&dir, "zone", |path, bytes| std::fs::write(path, bytes));
    store.write(&store.snapshot(&saved).expect("snapshots")).expect("writes");
    let loaded = store.load().expect("reads").expect("written").value;
    std::fs::remove_dir_all(&dir).expect("cleans up");

    assert_eq!(loaded, saved);
    let (fresh, _) = start("ground-platform");
    assert_eq!(fresh.zone_start(), Some(loaded), "the load starts from what was saved");
}

/// The square's walk starts as a save of someone who has fought its encounter does: cleared, so its
/// trigger, which the walk crosses, brings no one.
#[test]
fn a_cleared_encounter_stays_cleared() {
    let (mut sim, cast) = start("square-walk");
    let zone = sim.zone().expect("a zone");
    let id = zone.encounters()[0].id();
    assert_eq!(
        zone.zone_start(),
        ZoneStart { checkpoint: Some("south_paving".to_owned()), cleared: vec![id] }
    );
    run(&mut sim, &cast, walks::walk_the_square, walks::length(&walks::SQUARE_LEGS), |_| {});
    assert_eq!(sim.actors().len(), 1, "nobody came");
    assert!(
        sim.zone()
            .expect("a zone")
            .encounters()
            .iter()
            .all(|encounter| encounter.phase() == EncounterPhase::Cleared)
    );
}

/// While a fight is on, standing in a checkpoint doesn't reach it: a save made then would start the fight
/// over from somewhere it never was.
#[test]
fn a_running_fight_reaches_no_checkpoint() {
    let (mut sim, cast) = start("arena-waves");
    for _ in 0..5 {
        sim.step(&TickActions::NEUTRAL);
    }
    let south = sim.zone().expect("a zone").checkpoint("south_paving").expect("listed");
    assert_eq!(sim.return_to_checkpoint(south), Ok(cast[0]));
    for _ in 0..5 {
        sim.step(&TickActions::NEUTRAL);
    }
    let zone = sim.zone().expect("a zone");
    assert!(zone.encounters().iter().any(|encounter| encounter.phase() == EncounterPhase::Running));
    assert_eq!(zone.latest_checkpoint(), None);
}

/// Which checkpoint is the latest goes in the checksum, as soon as there is one.
#[test]
fn the_latest_checkpoint_is_in_the_checksum() {
    let (mut plain, _) = start("ground-walk");
    let (mut started, _) = start("ground-walk");
    assert_eq!(plain.checksum(), started.checksum());
    let start = ZoneStart { checkpoint: Some("lane_end".to_owned()), cleared: Vec::new() };
    let place = started.start_at(&start).expect("starts").expect("a place");
    assert_eq!(place.position, Vec3::new(-28.0, 0.0, -20.0));
    assert_ne!(plain.checksum(), started.checksum());
    plain.step(&TickActions::NEUTRAL);
    started.step(&TickActions::NEUTRAL);
    assert_ne!(plain.checksum(), started.checksum());
}

#[test]
fn a_start_names_only_what_the_zone_has_before_anything_runs() {
    let (mut sim, _) = start("ground-walk");
    let nowhere = ZoneStart { checkpoint: Some("nowhere".to_owned()), cleared: Vec::new() };
    assert!(
        matches!(sim.start_at(&nowhere), Err(ZoneStartError::UnknownCheckpoint { name }) if name == "nowhere")
    );
    let id = sapphire_serialize::AssetId::from_bytes([7; 16]);
    let stranger = ZoneStart { checkpoint: None, cleared: vec![id] };
    assert!(
        matches!(sim.start_at(&stranger), Err(ZoneStartError::UnknownEncounter { id: got }) if got == id)
    );
    assert!(matches!(sim.start_at(&ZoneStart::default()), Ok(None)));
    sim.step(&TickActions::NEUTRAL);
    assert!(matches!(sim.start_at(&ZoneStart::default()), Err(ZoneStartError::Started { ticks: 1 })));

    let (mut bare, _) = start("melee");
    assert!(matches!(bare.start_at(&ZoneStart::default()), Err(ZoneStartError::NoZone)));
    assert_eq!(bare.zone_start(), None);
}

#[test]
fn a_teleport_refuses_what_it_cant_do() {
    let (mut sim, cast) = start("ground-walk");
    let nowhere = Place { position: Vec3::new(f32::NAN, 0.0, 0.0), yaw: 0.0 };
    assert_eq!(sim.teleport(cast[0], nowhere), Err(TeleportError::NotFinite));
    let stranger = ActorId::from_raw(40).expect("an ID");
    assert_eq!(
        sim.teleport(stranger, Place { position: Vec3::ZERO, yaw: 0.0 }),
        Err(TeleportError::UnknownActor(stranger))
    );
    let ninth = CheckpointId::from_raw(9);
    assert_eq!(sim.return_to_checkpoint(ninth), Err(TeleportError::UnknownCheckpoint(ninth)));
    let (mut bare, _) = start("melee");
    assert_eq!(bare.return_to_checkpoint(walks::PLATFORM), Err(TeleportError::NoZone));
    assert_eq!(sim.actor(cast[0]).expect("alive").teleports, 0, "nothing refused is counted");
}

/// A teleport is counted where presentation reads it, so what's drawn snaps rather than sweeping, and the
/// count is no part of the checksum: two returns to the platform hash as one does.
#[test]
fn a_teleport_is_counted_for_presentation_alone() {
    let (mut sim, cast) = start("ground-walk");
    let (mut still, _) = start("ground-walk");
    sim.step(&TickActions::NEUTRAL);
    still.step(&TickActions::NEUTRAL);
    let here = place_of(&sim, cast[0]);
    sim.teleport(cast[0], here).expect("alive");
    assert_eq!(sim.actor(cast[0]).expect("alive").teleports, 1);
    assert_eq!(still.actor(cast[0]).expect("alive").teleports, 0);
    sim.return_to_checkpoint(walks::PLATFORM).expect("returns");
    assert_eq!(sim.actor(cast[0]).expect("alive").teleports, 2);
    let (mut once, _) = start("ground-walk");
    once.step(&TickActions::NEUTRAL);
    once.return_to_checkpoint(walks::PLATFORM).expect("returns");
    for _ in 0..30 {
        sim.step(&TickActions::NEUTRAL);
        once.step(&TickActions::NEUTRAL);
        assert_eq!(sim.checksum(), once.checksum());
    }
}

/// A return and the steps after it take nothing from the heap.
#[test]
fn a_return_takes_no_heap() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let (mut sim, cast) = walked_out();
    let ticks: Vec<TickActions> = (0..120).map(|tick| walks::from_the_platform(tick, &cast)).collect();
    let ((), returned) = alloc::count_in_scope(|| {
        sim.return_to_checkpoint(walks::PLATFORM).expect("returns");
    });
    let ((), stepped) = alloc::count_in_scope(|| {
        for actions in &ticks {
            sim.step(actions);
        }
    });
    assert!(alloc::is_installed());
    assert_eq!(returned.allocations, 0, "a return allocated {} times", returned.allocations);
    assert_eq!(stepped.allocations, 0, "120 steps after a return allocated {} times", stepped.allocations);
}
