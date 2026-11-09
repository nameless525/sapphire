//! Encounters through the public API (M3 B2): a zone's triggers, waves by each rule, and who a director's
//! tokens let swing. The two real ones are `content/scenarios/`' `arena-waves` and `cart-yard`, and the rest
//! build a small zone of their own on the arena's floor, to change one thing at a time.

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_gameplay::replay::{self, ActionSource, Player, Replay, SCENARIOS, find_scenario};
use sapphire_gameplay::{
    ActorActions, ActorId, Buttons, EncounterPhase, Faction, SimConfig, Simulation, Stick, TickActions,
    ZoneCast, ZoneStartError,
};
use sapphire_serialize::AssetId;
use sapphire_serialize::cooked::AlignedVec;
use sapphire_zone::{EncounterDocument, MAX_FACTION, Zone, ZoneDocument, ZonePresentation};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const ENCOUNTER: AssetId = AssetId::from_bytes([0x40; 16]);

/// A zone with four spawners round the origin, a trigger round `trigger_at`, which is where the player stands
/// unless it's moved, and an encounter that lists `waves`.
struct Small {
    tokens: u8,
    rest: u32,
    waves: &'static str,
    trigger_at: [f32; 3],
    spawns: &'static str,
    faction: u8,
}

impl Default for Small {
    fn default() -> Self {
        Self {
            tokens: 2,
            rest: 0,
            waves: r#"(when: Start, spawners: ["a", "b", "c", "d"])"#,
            trigger_at: [0.0, 1.0, 0.0],
            spawns: "warrior",
            faction: 2,
        }
    }
}

impl Small {
    fn cooked(&self) -> (AlignedVec, AlignedVec) {
        let [x, y, z] = self.trigger_at;
        let spawns = self.spawns;
        let zone = format!(
            r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (
                grid: (cell_size: 16.0),
                spawners: [
                    (name: "a", position: (2.0, 0.0, 2.0), yaw_degrees: 0.0, spawns: "{spawns}"),
                    (name: "b", position: (-2.0, 0.0, 2.0), yaw_degrees: 0.0, spawns: "{spawns}"),
                    (name: "c", position: (2.0, 0.0, -2.0), yaw_degrees: 0.0, spawns: "{spawns}"),
                    (name: "d", position: (-2.0, 0.0, -2.0), yaw_degrees: 0.0, spawns: "{spawns}"),
                ],
                triggers: [
                    (name: "t", position: ({x:?}, {y:?}, {z:?}), volume: Sphere(radius: 2.0), event: "go"),
                ],
                encounters: ["{ENCOUNTER}"],
            ))"#
        );
        let document = ZoneDocument::from_ron(&zone).expect("a good zone");
        let zone = Zone {
            simulation: document.simulation().expect("it reads"),
            presentation: ZonePresentation {
                cell_size: 16.0,
                backdrop: None,
                cells: Vec::new(),
                instances: Vec::new(),
                lights: Vec::new(),
                probes: None,
                day: None,
                rigs: None,
                splines: Vec::new(),
            },
        };
        let encounter = format!(
            r#"(type: "sapphire::zone::EncounterDocument", version: 1, data: (
                starts_on: "go", faction: {}, tokens: {}, token_rest: {}, waves: [{}],
            ))"#,
            self.faction, self.tokens, self.rest, self.waves,
        );
        let encounter = EncounterDocument::from_ron(&encounter).expect("a good encounter");
        (zone.to_cooked().expect("archives"), encounter.cook().expect("cooks").to_cooked().expect("archives"))
    }

    /// A simulation with this zone, the arena's colliders and moves, and a player at the origin.
    fn start(&self, max_actors: u32) -> Result<(Simulation, ActorId), ZoneStartError> {
        let (zone, encounter) = self.cooked();
        let config =
            SimConfig { max_actors, ..find_scenario(&SCENARIOS, "arena-fight").expect("listed").config };
        let mut sim = Simulation::new(config);
        let colliders = sapphire_gameplay::arena::cooked_colliders().expect("cooks");
        let set = sapphire_colliders::ColliderSet::read(&colliders).expect("reads");
        let physics = sapphire_physics::PhysicsWorld::from_cooked(set, sim.body_capacity()).expect("builds");
        sim.set_physics_world(physics).expect("room");
        let controller =
            Some(sim.add_controller_profile(&sapphire_gameplay::arena::profile()).expect("good"));
        let player_moves = sim.add_move_set(replay::player_moves().expect("reads"));
        let warrior_moves = replay::add_warrior(&mut sim).expect("compiles");
        let moves = [("player", player_moves), ("warrior", warrior_moves)];
        let zone = Zone::read(&zone).expect("reads");
        sim.set_zone(zone, ZoneCast { moves: &moves, controller }, |id| {
            (id == ENCOUNTER).then_some(encounter.as_slice())
        })?;
        let player = sim
            .spawn(sapphire_gameplay::ActorSpawn {
                moves: Some(player_moves),
                controller,
                faction: Faction::new(1).expect("a side"),
                ..sapphire_gameplay::ActorSpawn::default()
            })
            .expect("room");
        sim.give_soft_lock(player);
        Ok((sim, player))
    }
}

fn attackers(sim: &Simulation) -> usize {
    sim.actors().filter(|actor| actor.brain.is_attacking()).count()
}

/// Runs `ticks` of nobody pressing anything, and says the most ever attacking at once.
fn most_attacking(sim: &mut Simulation, ticks: u32) -> usize {
    let mut most = 0;
    for _ in 0..ticks {
        sim.step(&TickActions::NEUTRAL);
        most = most.max(attackers(sim));
    }
    most
}

fn encounter(sim: &Simulation) -> &sapphire_gameplay::SimEncounter {
    &sim.zone().expect("it has one").encounters()[0]
}

/// The director's rule, by what the fighters do rather than by its own count: fewer tokens than fighters,
/// and the rest wait, and they're why.
#[test]
fn an_encounter_with_more_enemies_than_tokens_has_enemies_waiting() {
    let held = |tokens: u8| {
        let (mut sim, _) = Small { tokens, ..Small::default() }.start(8).expect("starts");
        let most = most_attacking(&mut sim, 1500);
        (most, sim.zone().expect("it has one").stats())
    };
    let (one, one_stats) = held(1);
    assert_eq!(one, 1, "one token, one swinging at a time");
    assert_eq!(one_stats.most_tokens_held, 1);
    assert!(one_stats.tokens_waited > 500, "the rest waited: {one_stats:?}");

    let (two, two_stats) = held(2);
    assert_eq!(two, 2, "two tokens, two at a time");
    assert_eq!(two_stats.most_tokens_held, 2);
    assert!(two_stats.tokens_taken > one_stats.tokens_taken, "{two_stats:?} against {one_stats:?}");
    assert!(two_stats.tokens_waited < one_stats.tokens_waited);

    let (four, four_stats) = held(4);
    assert!(four > 2, "with one each, more than two swing at once: {four}");
    assert_eq!(four_stats.tokens_waited, 0, "and nobody waits");
}

/// A token rests when it comes back, so the next swing can't start sooner.
#[test]
fn a_token_rests_before_the_next_can_take_it() {
    let (mut sim, _) = Small { tokens: 1, rest: 240, ..Small::default() }.start(8).expect("starts");
    let (mut ended_at, mut was) = (None, 0);
    let mut starts = 0;
    for tick in 0..2000_u32 {
        sim.step(&TickActions::NEUTRAL);
        let now = attackers(&sim);
        if now > was {
            if let Some(ended_at) = ended_at {
                assert!(tick - ended_at >= 240, "{ended_at} to {tick}");
            }
            starts += 1;
        } else if now < was {
            ended_at = Some(tick);
        }
        was = now;
    }
    assert!(starts >= 3, "{starts}");
}

/// A token's holder who goes gives it back, even though no brain of theirs thinks again.
#[test]
fn a_holder_who_goes_gives_the_token_back() {
    let (mut sim, _) = Small { tokens: 1, rest: 100, ..Small::default() }.start(8).expect("starts");
    let mut holder = None;
    for _ in 0..1000 {
        sim.step(&TickActions::NEUTRAL);
        holder = sim.actors().find(|actor| actor.brain.is_attacking()).map(|actor| actor.id);
        if holder.is_some() {
            break;
        }
    }
    let holder = holder.expect("someone took it");
    assert_eq!(encounter(&sim).tokens_held(), 1);
    assert!(sim.despawn(holder));
    sim.step(&TickActions::NEUTRAL);
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.actor(holder).is_none());
    assert_eq!(encounter(&sim).tokens_held(), 0, "it isn't lost with them");
    let most = most_attacking(&mut sim, 1000);
    assert_eq!(most, 1, "and someone else takes it");
}

/// What presentation warms ahead of (M3 W5): an encounter's start is its trigger's volume, and what's still
/// to come of it is what its waves haven't spawned.
#[test]
fn an_encounter_says_where_it_starts_and_how_many_are_still_to_come() {
    let (mut sim, cast) = {
        let scenario = find_scenario(&SCENARIOS, "cart-yard").expect("listed");
        scenario.start(scenario.config).expect("sets up")
    };
    let zone = sim.zone().expect("it has one");
    let start = zone.start_volume(0).expect("the cart yard's trigger starts it");
    assert_eq!(start.position, [9.0, 1.0, 10.0]);
    assert_eq!(start.volume, sapphire_zone::CookedVolume::Box { half_extents: [4.0, 2.0, 4.0] });
    assert_eq!(zone.start_volume(1), None, "one encounter");
    assert_eq!(encounter(&sim).spawned(), 0);
    let mut script = replay::into_the_yard;
    for _ in 0..300 {
        let actions = script.actions(sim.tick(), &cast);
        sim.step(&actions);
    }
    assert_eq!(encounter(&sim).spawned(), 2, "the first wave");
}

/// The trigger fires as someone goes in, and not before, once, with its encounter starting once.
#[test]
fn a_zone_trigger_starts_its_encounter_when_someone_walks_in() {
    let (mut sim, cast) = {
        let scenario = find_scenario(&SCENARIOS, "cart-yard").expect("listed");
        scenario.start(scenario.config).expect("sets up")
    };
    assert_eq!(cast.len(), 1, "only the player, to start with");
    for _ in 0..60 {
        sim.step(&TickActions::NEUTRAL);
    }
    let zone = sim.zone().expect("it has one");
    assert_eq!(zone.stats().triggers_fired, 0, "standing at the start starts nothing");
    assert_eq!(zone.planned(), 4);
    assert_eq!(encounter(&sim).phase(), EncounterPhase::Waiting);
    assert_eq!(sim.actors().count(), 1);

    let (mut sim, cast) = {
        let scenario = find_scenario(&SCENARIOS, "cart-yard").expect("listed");
        scenario.start(scenario.config).expect("sets up")
    };
    let mut script = replay::into_the_yard;
    let mut fired_at = None;
    for _ in 0..300 {
        let actions = script.actions(sim.tick(), &cast);
        sim.step(&actions);
        if fired_at.is_none() && sim.zone().expect("it has one").stats().triggers_fired > 0 {
            fired_at = Some(sim.tick());
            let player = sim.actor(cast[0]).expect("there");
            assert!(player.position.x >= 4.9, "it's at the box's edge: {:?}", player.position);
        }
    }
    let fired_at = fired_at.expect("the walk gets there");
    assert!(fired_at > 90, "not before it turned for the yard: {fired_at}");
    let zone = sim.zone().expect("it has one");
    assert_eq!(zone.stats().triggers_fired, 1);
    assert_eq!(zone.stats().encounters_started, 1);
    assert_eq!(encounter(&sim).phase(), EncounterPhase::Running);
    assert_eq!(encounter(&sim).fighters().count(), 2, "the first wave's");

    // They stand on the proving ground, where the document put them, and they fight.
    for fighter in encounter(&sim).fighters() {
        let view = sim.actor(fighter).expect("standing");
        assert!(view.position.y.abs() < 0.1, "{:?}", view.position);
        assert!(view.brain.is_on());
    }
}

/// Staying in isn't entering, walking out and back is, and an encounter starts once for all of it.
#[test]
fn a_trigger_fires_on_each_entry_and_an_encounter_starts_once() {
    // A trigger nobody stands in never fires, whatever comes of the rest.
    let away = Small { trigger_at: [30.0, 1.0, 30.0], ..Small::default() };
    let (mut sim, _) = away.start(8).expect("starts");
    most_attacking(&mut sim, 30);
    assert_eq!(sim.zone().expect("it has one").stats().triggers_fired, 0);
    assert_eq!(encounter(&sim).phase(), EncounterPhase::Waiting);

    let (mut sim, player) = Small::default().start(8).expect("starts");
    sim.step(&TickActions::NEUTRAL);
    let stats = sim.zone().expect("it has one").stats();
    assert_eq!((stats.triggers_fired, stats.encounters_started), (1, 1));
    most_attacking(&mut sim, 30);
    assert_eq!(sim.zone().expect("it has one").stats().triggers_fired, 1, "staying in isn't entering");

    let run = |sim: &mut Simulation, x: i8, done: fn(f32) -> bool| {
        for _ in 0..600 {
            let mut actions = TickActions::NEUTRAL;
            let stick = Stick::new(x, 0);
            actions.set(player, ActorActions { buttons: Buttons::NONE, stick }).expect("one fits");
            sim.step(&actions);
            if done(sim.actor(player).expect("there").position.x) {
                return;
            }
        }
        panic!("it never got there");
    };
    run(&mut sim, 127, |x| x > 3.0);
    assert_eq!(sim.zone().expect("it has one").stats().triggers_fired, 1, "leaving isn't either");
    run(&mut sim, -127, |x| x < 1.0);
    let stats = sim.zone().expect("it has one").stats();
    assert_eq!((stats.triggers_fired, stats.encounters_started), (2, 1));
    assert_eq!(stats.spawned, 4, "and nothing more came of it");
}

/// Each of a wave's rules, by when its fighters come.
#[test]
fn waves_come_by_their_rules() {
    let waves = r#"(when: Start, spawners: ["a", "b"]),
                   (when: After(200), gap: 10, spawners: ["c", "d"]),
                   (when: Remaining(1), spawners: ["a"])"#;
    let (mut sim, _) = Small { waves, ..Small::default() }.start(8).expect("starts");
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(encounter(&sim).waves_begun(), 1);
    assert_eq!(encounter(&sim).fighters().count(), 2, "both at once, with no gap");
    let began = sim.tick();

    while sim.tick() < began + 199 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(encounter(&sim).waves_begun(), 1, "the clock's not out");
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(encounter(&sim).waves_begun(), 2, "and now it is");
    assert_eq!(encounter(&sim).fighters().count(), 3, "the first of them");
    for _ in 0..9 {
        sim.step(&TickActions::NEUTRAL);
        assert_eq!(encounter(&sim).fighters().count(), 3, "the gap");
    }
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(encounter(&sim).fighters().count(), 4, "the second of them");

    // Four standing: more than the last wave waits for, however long.
    most_attacking(&mut sim, 60);
    assert_eq!(encounter(&sim).waves_begun(), 2);
    let fighters: Vec<ActorId> = encounter(&sim).fighters().collect();
    assert!(sim.despawn(fighters[0]) && sim.despawn(fighters[1]));
    most_attacking(&mut sim, 5);
    assert_eq!(encounter(&sim).waves_begun(), 2, "two still stand");
    assert!(sim.despawn(fighters[2]));
    sim.step(&TickActions::NEUTRAL);
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(encounter(&sim).waves_begun(), 3, "one, and it comes");

    // All come and all gone: that's the end of it, and it stays there.
    let everyone: Vec<ActorId> = encounter(&sim).fighters().collect();
    most_attacking(&mut sim, 2);
    for actor in everyone {
        sim.despawn(actor);
    }
    most_attacking(&mut sim, 3);
    assert_eq!(encounter(&sim).phase(), EncounterPhase::Cleared);
    assert_eq!(sim.zone().expect("it has one").stats().encounters_cleared, 1);
}

/// A full pool refuses a spawn and says so, and the rest goes on as if that one never was.
#[test]
fn a_pool_too_small_refuses_and_goes_on() {
    let (mut sim, _) = Small::default().start(3).expect("starts");
    most_attacking(&mut sim, 5);
    let stats = sim.zone().expect("it has one").stats();
    assert_eq!(stats.spawned, 2);
    assert_eq!(stats.spawns_refused, 2);
    assert_eq!(encounter(&sim).fighters().count(), 2);
}

/// The spawns, the brains' first draws, and the tokens: nothing a step does here takes the heap, the first
/// step of a wave included. What it costs is `Simulation::set_zone`'s, before a step.
#[test]
fn a_wave_arrives_with_no_heap_in_a_step() {
    if !PER_THREAD_COUNTS {
        return;
    }
    for name in ["arena-waves", "cart-yard"] {
        let scenario = find_scenario(&SCENARIOS, name).expect("listed");
        let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
        let mut script = replay::into_the_yard;
        let ticks: Vec<TickActions> = (0..1500).map(|tick| script.actions(tick, &cast)).collect();
        let ((), counts) = alloc::count_in_scope(|| {
            for actions in &ticks {
                sim.step(actions);
            }
        });
        assert!(alloc::is_installed());
        assert_eq!(counts.allocations, 0, "{name}: 1500 steps allocated {} times", counts.allocations);
        assert!(sim.zone().expect("it has one").stats().spawned > 0, "{name}: someone came");
    }
}

/// Two runs are one run, and a zone that says anything else has another `tables_hash`.
#[test]
fn a_zone_goes_in_the_hashes() {
    let run = |small: &Small| {
        let (mut sim, _) = small.start(8).expect("starts");
        let tables = sim.tables_hash();
        let mut checksums = Vec::new();
        for _ in 0..600 {
            sim.step(&TickActions::NEUTRAL);
            checksums.push(sim.checksum());
        }
        (tables, checksums)
    };
    let plain = run(&Small::default());
    assert_eq!(plain, run(&Small::default()), "a run is a run");

    let others = [
        Small { tokens: 1, ..Small::default() },
        Small { rest: 7, ..Small::default() },
        Small { trigger_at: [0.5, 1.0, 0.0], ..Small::default() },
        Small { faction: 3, ..Small::default() },
        Small { waves: r#"(when: Start, spawners: ["b", "a", "c", "d"])"#, ..Small::default() },
    ];
    for small in &others {
        let (tables, checksums) = run(small);
        assert_ne!(tables, plain.0, "{} tokens, rest {}, {}", small.tokens, small.rest, small.waves);
        assert_ne!(checksums, plain.1, "{} tokens, rest {}, {}", small.tokens, small.rest, small.waves);
    }
}

/// What a document can't say comes back as a refusal, naming what's wrong.
#[test]
fn a_zone_that_cant_stand_up_says_why() {
    let unknown = Small { spawns: "dancer", ..Small::default() };
    let error = unknown.start(8).err().expect("nothing says what it is");
    assert!(error.to_string().contains("`dancer`"), "{error}");

    let brainless = Small { spawns: "player", ..Small::default() };
    let error = brainless.start(8).err().expect("the player thinks for nobody");
    assert!(error.to_string().contains("no brain"), "{error}");
}

/// The document's range and the simulation's sides are one thing, across a crate that can't look it up.
#[test]
fn a_documents_faction_is_a_simulations() {
    assert_eq!(MAX_FACTION, Faction::COUNT);
    assert!(Faction::new(MAX_FACTION).is_some());
    assert!(Faction::new(MAX_FACTION + 1).is_none());
}

/// `arena-waves` as its document says: a fight's first step starts it, seven come, and they wait their turn.
#[test]
fn the_squares_waves_come_and_wait() {
    let scenario = find_scenario(&SCENARIOS, "arena-waves").expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
    let mut script = replay::idle;
    let mut most = 0;
    for _ in 0..900 {
        let actions = script.actions(sim.tick(), &cast);
        sim.step(&actions);
        most = most.max(attackers(&sim));
    }
    let stats = sim.zone().expect("it has one").stats();
    assert_eq!(stats.encounters_started, 1);
    assert!(stats.spawned >= 5, "the clock's wave came: {stats:?}");
    assert!(most <= 2, "{most}");
    assert!(stats.tokens_waited > 0, "{stats:?}");
    assert_eq!(stats.spawns_refused, 0, "the pool's as big as it says");
}

/// The suite's own two, as recorded: what each is there to show happens in it. The suite holds their checksums
/// and their steps' allocations.
#[test]
fn the_recorded_waves_show_what_they_were_recorded_for() {
    let played = |name: &str| {
        let path = format!("{}/../../tests/replays/{name}.sreplay", env!("CARGO_MANIFEST_DIR"));
        let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("decodes");
        let mut player = Player::new(replay, &SCENARIOS).expect("starts");
        player.run_to_end();
        let sim = player.simulation();
        (sim.zone().expect("it has one").stats(), encounter(sim).waves_begun(), encounter(sim).tokens_held())
    };

    let (stats, waves, held) = played("arena-waves");
    assert_eq!((stats.spawned, waves), (7, 3), "all three waves, on the clock: {stats:?}");
    assert_eq!(stats.most_tokens_held, 2, "{stats:?}");
    // Tokens go round, more of them than there are fighters, and the gate holds the rest back. Brains that
    // walk the navmesh and keep apart (M3 P3) close in later and pass fewer of them than straight-line ones did.
    assert!(stats.tokens_taken > stats.spawned && stats.tokens_waited > 300, "{stats:?}");
    assert!(held <= 2);

    let (stats, waves, _) = played("cart-yard");
    assert_eq!((stats.spawned, waves), (4, 2), "the second came because the first fell: {stats:?}");
    assert_eq!(stats.encounters_started, 1, "{stats:?}");
    assert_eq!(stats.most_tokens_held, 1, "{stats:?}");
    assert!(stats.tokens_waited > 500, "{stats:?}");
}

/// The square's zone says where the arena's spawn points are, since a stage's document can't reach a zone's: if
/// one moves, so does the other.
#[test]
fn the_squares_spawners_are_the_arenas_spawn_points() {
    let text = include_str!("../../../content/zones/square/square.zone.ron");
    let zone = ZoneDocument::from_ron(text).expect("reads");
    let at = |name: &str| zone.spawners.iter().find(|spawner| spawner.name == name).expect(name);
    let spawns = sapphire_gameplay::arena::spawns();
    let points = [
        ("player_start", spawns.player),
        ("north", spawns.enemy),
        ("north_west", spawns.crowd[0]),
        ("north_east", spawns.crowd[1]),
        ("west", spawns.crowd[2]),
        ("east", spawns.crowd[3]),
        ("far_east", spawns.crowd[4]),
        ("south_east", spawns.crowd[5]),
    ];
    for (name, point) in points {
        assert_eq!(at(name).position, point.position().to_array(), "{name}");
        assert!((at(name).yaw_degrees.to_radians() - point.yaw()).abs() < 1e-5, "{name}");
    }
}
