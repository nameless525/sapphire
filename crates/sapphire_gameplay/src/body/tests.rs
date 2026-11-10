use std::alloc::System;
use std::path::Path;

use sapphire_assets::clip::{Clip, DEFAULT_RATE};
use sapphire_assets::graph::AnimGraph;
use sapphire_assets::rig::{CharacterRig, HumanBone};
use sapphire_assets::skeleton::{Joint, JointTransform, Skeleton};
use sapphire_assets::{AssetId, CpuResident};
use sapphire_core::Name;
use sapphire_core::alloc::{self, AllocScope, CountingAllocator};

use super::*;
use crate::checksum::legacy_state;
use crate::replay::{LiveMoves, Replay, SCENARIOS, find_scenario};
use crate::{
    ActorSpawn, ActorTransform, CombatSet, OverheadShadowResult, OverheadShadowShape, ShadowCapsule,
    ShadowEndpoint, SimConfig, Simulation, TickActions,
};

#[global_allocator]
static TEST_ALLOCATOR: CountingAllocator<System> = CountingAllocator::new(System);

fn assert_no_fixed_allocations(sim: &Simulation, case: &str) {
    if alloc::PER_THREAD_COUNTS {
        assert_eq!(sim.stats().allocations, 0, "{case} allocated by step {}", sim.tick());
    }
}

fn id(value: u8) -> AssetId {
    AssetId::from_bytes([value; 16])
}

fn test_assets() -> BodyAssets {
    test_assets_with_joint("root")
}

fn test_assets_with_joint(name: &str) -> BodyAssets {
    let skeleton =
        Skeleton::new(&[Joint { name: name.to_owned(), parent: None, rest: JointTransform::IDENTITY }]);
    let rig = CharacterRig { joint_count: 1, ..CharacterRig::default() };
    let clip = Clip {
        name: "idle".to_owned(),
        skeleton: Some(id(2)),
        joint_count: 1,
        rate: DEFAULT_RATE,
        ticks: 30,
        ..Clip::default()
    };
    let mut graph = AnimGraph::from_document(
        r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
            clips: [(name: "idle", source: "01010101-0101-0101-0101-010101010101")],
            locomotion: (start: "ground", states: [
                (name: "ground", plays: Clip(clip: "idle", looping: true)),
            ]),
        ))"#,
    )
    .unwrap();
    graph.resolve(|_| id(1));
    BodyAssets {
        graph: BodyGraphAssets {
            graph: CpuResident::from_cooked(graph.to_cooked().unwrap()).unwrap(),
            skeleton_id: id(2),
            skeleton: CpuResident::from_cooked(skeleton.to_cooked().unwrap()).unwrap(),
            clips: vec![(id(1), CpuResident::from_cooked(clip.to_cooked().unwrap()).unwrap())],
        },
        rig: CpuResident::from_cooked(rig.to_cooked().unwrap()).unwrap(),
    }
}

fn test_options(scale: f32) -> BodyProfileOptions<'static> {
    BodyProfileOptions {
        tuning: *BodyTuning::shipped(),
        floor: 0.0,
        scale,
        morphs: &[],
        contact_joints: &["root"],
        seed: 1,
        ground_turn: false,
        grounded_gather: true,
        look_height: 0.0,
        look_weight: 0.0,
        head_authority: None,
    }
}

fn with_body(scale: f32) -> (Simulation, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let actor = sim.spawn(ActorSpawn::default()).unwrap();
    sim.attach_body(actor, None, BodyProfile::bind(test_assets(), test_options(scale)).unwrap()).unwrap();
    (sim, actor)
}

#[test]
fn fixed_body_profile_binds_exact_cooked_ids_and_policy() {
    let profile = BodyProfile::bind(test_assets(), test_options(1.0)).unwrap();
    let same = BodyProfile::bind(test_assets(), test_options(1.0)).unwrap();
    let changed = BodyProfile::bind(test_assets(), test_options(1.1)).unwrap();
    assert_eq!(profile.fingerprint(), same.fingerprint());
    assert_ne!(profile.fingerprint(), changed.fingerprint());
    let mut wrong = test_assets();
    wrong.graph.clips[0].0 = id(4);
    assert!(matches!(BodyProfile::bind(wrong, test_options(1.0)), Err(BodyBindError::Clips)));
    let mut wrong = test_assets();
    wrong.graph.skeleton_id = id(4);
    assert!(matches!(BodyProfile::bind(wrong, test_options(1.0)), Err(BodyBindError::ClipSkeleton(_))));
}

/// A body with legs and a head, playing the same idle as [`test_assets`]: what the feet and the look act on.
fn humanoid_assets() -> BodyGraphAssets {
    const JOINTS: [(&str, Option<u16>, [f32; 3]); 9] = [
        ("hips", None, [0.0, 0.95, 0.0]),
        ("left_upper_leg", Some(0), [0.1, -0.05, 0.0]),
        ("left_lower_leg", Some(1), [0.0, -0.42, 0.0]),
        ("left_foot", Some(2), [0.0, -0.42, 0.0]),
        ("right_upper_leg", Some(0), [-0.1, -0.05, 0.0]),
        ("right_lower_leg", Some(4), [0.0, -0.42, 0.0]),
        ("right_foot", Some(5), [0.0, -0.42, 0.0]),
        ("neck", Some(0), [0.0, 0.5, 0.0]),
        ("head", Some(7), [0.0, 0.1, 0.0]),
    ];
    let joints: Vec<Joint> = JOINTS
        .iter()
        .map(|&(name, parent, translation)| Joint {
            name: name.to_owned(),
            parent,
            rest: JointTransform { translation, ..JointTransform::IDENTITY },
        })
        .collect();
    let mut assets = test_assets().graph;
    let clip = Clip {
        name: "idle".to_owned(),
        skeleton: Some(id(2)),
        joint_count: JOINTS.len() as u32,
        rate: DEFAULT_RATE,
        ticks: 30,
        ..Clip::default()
    };
    assets.skeleton = CpuResident::from_cooked(Skeleton::new(&joints).to_cooked().unwrap()).unwrap();
    assets.clips = vec![(id(1), CpuResident::from_cooked(clip.to_cooked().unwrap()).unwrap())];
    assets
}

/// [`humanoid_assets`]' bones.
const HUMANOID: [(HumanBone, usize); 9] = [
    (HumanBone::Hips, 0),
    (HumanBone::LeftUpperLeg, 1),
    (HumanBone::LeftLowerLeg, 2),
    (HumanBone::LeftFoot, 3),
    (HumanBone::RightUpperLeg, 4),
    (HumanBone::RightLowerLeg, 5),
    (HumanBone::RightFoot, 6),
    (HumanBone::Neck, 7),
    (HumanBone::Head, 8),
];

/// `melee`'s first two fighters, on its floor, as humanoid bodies looking at each other, on a fight started
/// on `tuning` as a game's document starts one: its `tables_hash`, and both bodies' receipts after each
/// of `ticks` steps.
fn humanoid_fight(tuning: BodyTuning, ticks: u32) -> (u64, Vec<[u64; 2]>) {
    let scenario = find_scenario(&SCENARIOS, "melee").unwrap();
    let mut live = LiveMoves::default();
    live.set_bodies(tuning);
    let (mut sim, cast) = live.start(scenario, scenario.config).unwrap();
    let pair = [cast[0], cast[1]];
    for (index, actor) in pair.into_iter().enumerate() {
        let options = BodyProfileOptions {
            tuning: sim.body_tuning(),
            // Its soles 4 cm over its root, as a capsule's round foot leaves them on a slope (sim.md), so the
            // feet have ground to reach for and the hips somewhere to go.
            floor: 0.04,
            contact_joints: &["hips"],
            look_height: 1.5,
            look_weight: 1.0,
            ..test_options(1.0)
        };
        let profile = BodyProfile::bind_named(humanoid_assets(), &HUMANOID, Vec3::Y * 0.05, options).unwrap();
        sim.attach_body(actor, Some(pair[1 - index]), profile).unwrap();
    }
    let receipts = (0..ticks)
        .map(|_| {
            sim.step(&TickActions::NEUTRAL);
            pair.map(|actor| sim.body_receipt(actor).expect("published").2)
        })
        .collect();
    (sim.tables_hash(), receipts)
}

/// The fixture numbers [`FootSettings::default`] and [`LookSettings::default`], which the bodies ran on
/// until M3 S3b gave the simulation a table.
fn fixture_tuning() -> BodyTuning {
    BodyTuning { feet: FootSettings::default(), look: sapphire_body_pose::LookSettings::default() }
}

/// Taking the table from a document moved no body: the shipped one poses every tick of a fight as the
/// numbers the bodies ran on before did. This holds the document to those numbers, so a deliberate retune of
/// `default_body.ron` ends this test, and moves every replay's hash as well.
#[test]
fn the_shipped_table_poses_the_bodies_as_the_numbers_they_ran_on_before() {
    let (shipped_hash, shipped) = humanoid_fight(*BodyTuning::shipped(), 90);
    let (fixture_hash, fixture) = humanoid_fight(fixture_tuning(), 90);
    assert_eq!(shipped, fixture);
    assert_eq!(shipped_hash, fixture_hash);
}

/// A game's own feet or look reach its bodies, and its replays' hash, so a file recorded on other numbers
/// refuses rather than plays on.
#[test]
fn a_foot_or_look_number_moves_the_tables_hash_and_the_bodies() {
    let (hash, receipts) = humanoid_fight(*BodyTuning::shipped(), 90);
    type Edit = fn(&mut BodyTuning);
    let edits: [(&str, Edit); 2] = [
        // Less than the 4 cm its feet reach down for.
        ("feet.max_drop", |tuning| tuning.feet.max_drop = 0.02),
        // Each turns more than this to the other.
        ("look.yaw_limit", |tuning| tuning.look.yaw_limit = 0.2),
    ];
    for (name, edit) in edits {
        let mut tuning = *BodyTuning::shipped();
        edit(&mut tuning);
        let (edited_hash, edited) = humanoid_fight(tuning, 90);
        assert_ne!(edited_hash, hash, "{name}");
        for body in 0..2 {
            assert!(receipts.iter().zip(&edited).any(|(a, b)| a[body] != b[body]), "{name}: body {body}");
        }
    }
}

/// A simulation's bodies all run on its one table: a profile bound on another is refused, and the table
/// can't change under a body or a step.
#[test]
fn a_body_binds_on_its_simulations_table_and_the_table_holds_from_then() {
    let mut other = *BodyTuning::shipped();
    other.feet.soften = 0.9;
    let mut sim = Simulation::new(SimConfig::default());
    let actor = sim.spawn(ActorSpawn::default()).unwrap();
    let elsewhere =
        BodyProfile::bind(test_assets(), BodyProfileOptions { tuning: other, ..test_options(1.0) });
    assert_eq!(sim.attach_body(actor, None, elsewhere.unwrap()), Err(BodyAttachError::Tuning));
    assert_eq!(sim.tune_bodies(other), Ok(true));
    assert_eq!(sim.tune_bodies(other), Ok(false), "the same table again changes nothing");
    let here = BodyProfile::bind(test_assets(), BodyProfileOptions { tuning: other, ..test_options(1.0) });
    sim.attach_body(actor, None, here.unwrap()).unwrap();
    assert_eq!(sim.tune_bodies(*BodyTuning::shipped()), Err(BodyAttachError::TableInUse));
    let mut stepped = Simulation::new(SimConfig::default());
    stepped.step(&TickActions::NEUTRAL);
    assert_eq!(stepped.tune_bodies(other), Err(BodyAttachError::TableInUse));
}

#[test]
fn fixed_body_is_ready_before_hits_and_reads_do_not_advance_it() {
    let (mut sim, actor) = with_body(1.0);
    let mut sample = sim.body_sample_buffer(actor).unwrap();
    assert!(!sim.read_body(actor, 1.0, &mut sample));
    sim.add_combat_system(CombatSet::Hits, move |world: &World| {
        let body = world.resource::<BodyAuthority>();
        let stamp = body.receipt(actor).expect("ready before hit resolution").0;
        assert_eq!(stamp.step, world.resource::<WorldClock>().0.ticks());
    });
    for _ in 0..4 {
        let allocations = AllocScope::begin();
        sim.step(&TickActions::NEUTRAL);
        let checksum = sim.checksum();
        for fraction in [0.0, 0.25, 0.6, 1.0, 0.25] {
            assert!(sim.read_body(actor, fraction, &mut sample));
            assert_eq!(sample.span().unwrap().after.step, sim.tick());
        }
        assert_eq!(sim.checksum(), checksum);
        assert!(!alloc::PER_THREAD_COUNTS || allocations.counts().allocations == 0);
    }
}

#[test]
fn fixed_body_cut_and_restart_do_not_publish_old_world_endpoints() {
    let (mut sim, actor) = with_body(1.0);
    let mut sample = sim.body_sample_buffer(actor).unwrap();
    for _ in 0..5 {
        sim.step(&TickActions::NEUTRAL);
    }
    let entity = sim.world().resource::<ActorTable>().entity(actor).unwrap();
    sim.cut_body(actor);
    sim.ecs_mut().world_mut().get_mut::<ActorTransform>(entity).unwrap().position = Vec3::X * 2.0;
    assert!(!sim.read_body(actor, 1.0, &mut sample));
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.read_body(actor, 0.25, &mut sample));
    let span = sample.span().unwrap();
    assert_eq!(span.before, span.after);
    assert_eq!(span.after.generation, 1);
    assert_eq!(sample.root().translation, Vec3::X * 2.0);

    sim.restart_builtin_fight(SimConfig::default());
    assert_eq!(sim.spawn(ActorSpawn::default()).unwrap(), actor);
    let (mut fresh, _) = with_body(1.0);
    assert_eq!(sim.checksum(), fresh.checksum());
    sim.step(&TickActions::NEUTRAL);
    fresh.step(&TickActions::NEUTRAL);
    assert_eq!(sim.checksum(), fresh.checksum());
    assert!(sim.read_body(actor, 0.25, &mut sample));
    let span = sample.span().unwrap();
    assert_eq!(span.before, span.after);
    assert_eq!(span.after.generation, 0);
    assert_eq!(sample.root().translation, Vec3::ZERO);

    assert!(sim.despawn(actor));
    assert!(!sim.read_body(actor, 1.0, &mut sample));
}

#[test]
fn fixed_body_uses_the_actors_frozen_and_fractional_clock() {
    let (mut sim, actor) = with_body(1.0);
    let entity = sim.world().resource::<ActorTable>().entity(actor).unwrap();
    let mut sample = sim.body_sample_buffer(actor).unwrap();
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.read_body(actor, 1.0, &mut sample));
    let initial = sample.span().unwrap().after;
    {
        let world = sim.ecs_mut().world_mut();
        let mut clock = world.get_mut::<EntityClock>(entity).unwrap();
        clock.0.push_scale(sapphire_time::ScaleSource::Debug, 250).unwrap();
        clock.0.apply_hitstop(2);
    }
    for expected_delta in [0, 0, 250, 250] {
        let previous = sample.span().unwrap().after;
        sim.step(&TickActions::NEUTRAL);
        assert!(sim.read_body(actor, 0.25, &mut sample));
        let now = sample.span().unwrap().after;
        assert_eq!(now.step, previous.step + 1);
        assert_eq!(now.owner_now.since(previous.owner_now).as_subticks(), expected_delta);
        assert_eq!(now.owner_now, sim.actor(actor).unwrap().clock_now);
    }
    assert_eq!(sample.span().unwrap().after.owner_now.since(initial.owner_now).as_subticks(), 500);
}

#[test]
fn named_body_without_morphs_binds_the_matching_stage_pose() {
    let assets = test_assets().graph;
    let mut stage_pose = sapphire_body_pose::CharacterPose::new(assets.skeleton.view(), &[], &[], Vec::new());
    let profile = BodyProfile::bind_named(
        assets,
        &[(HumanBone::Hips, 0)],
        Vec3::Y,
        BodyProfileOptions { grounded_gather: false, ..test_options(0.8) },
    )
    .unwrap();
    assert!(profile.owner.sample_buffer().bind_presentation(&mut stage_pose));
}

#[test]
fn two_fixed_bodies_publish_once_in_actor_order_on_separate_clocks() {
    fn bind(reverse: bool) -> (Simulation, [ActorId; 2]) {
        let mut sim = Simulation::new(SimConfig::default());
        let hero = sim.spawn(ActorSpawn::default()).unwrap();
        let warrior = sim.spawn(ActorSpawn { position: 2.0 * Vec3::X, ..ActorSpawn::default() }).unwrap();
        let hero_profile = BodyProfile::bind(test_assets(), test_options(1.0)).unwrap();
        let warrior_profile = BodyProfile::bind_named(
            test_assets().graph,
            &[(HumanBone::Hips, 0)],
            Vec3::Y,
            BodyProfileOptions { grounded_gather: false, ..test_options(0.8) },
        )
        .unwrap();
        let mut bindings = [(hero, warrior, hero_profile), (warrior, hero, warrior_profile)];
        if reverse {
            bindings.reverse();
        }
        for (actor, other, profile) in bindings {
            sim.attach_body(actor, Some(other), profile).unwrap();
        }
        assert_eq!(
            sim.attach_body(
                hero,
                Some(warrior),
                BodyProfile::bind(test_assets(), test_options(1.0)).unwrap()
            ),
            Err(BodyAttachError::Bound),
        );
        let mut clock = sim.clock_mut(warrior).unwrap();
        clock.0.push_scale(sapphire_time::ScaleSource::Debug, 250).unwrap();
        clock.0.apply_hitstop(2);
        (sim, [hero, warrior])
    }

    let (mut a, [hero, warrior]) = bind(false);
    let (mut b, _) = bind(true);
    assert_eq!(a.tables_hash(), b.tables_hash());
    assert_eq!(a.checksum(), b.checksum());
    let mut hero_body = a.body_sample_buffer(hero).unwrap();
    let mut warrior_body = a.body_sample_buffer(warrior).unwrap();
    a.add_combat_system(CombatSet::Hits, move |world: &World| {
        let tick = world.resource::<WorldClock>().0.ticks();
        let authority = world.resource::<BodyAuthority>();
        assert_eq!(authority.receipt(hero).unwrap().0.step, tick);
        assert_eq!(authority.receipt(warrior).unwrap().0.step, tick);
    });
    for warrior_now in [0, 0, 250, 500] {
        let allocations = AllocScope::begin();
        a.step(&TickActions::NEUTRAL);
        b.step(&TickActions::NEUTRAL);
        assert!(a.read_body(hero, 0.25, &mut hero_body));
        assert!(a.read_body(warrior, 0.25, &mut warrior_body));
        assert_eq!(a.checksum(), b.checksum());
        assert_eq!(hero_body.span().unwrap().after.owner_now.as_subticks(), a.tick() * 1000);
        assert_eq!(warrior_body.span().unwrap().after.owner_now.as_subticks(), warrior_now);
        assert_eq!(warrior_body.root().scale, 0.8);
        assert!(!alloc::PER_THREAD_COUNTS || allocations.counts().allocations == 0);
    }
    a.cut_body(hero);
    assert!(!a.read_body(hero, 0.25, &mut hero_body));
    assert!(a.read_body(warrior, 0.25, &mut warrior_body));
    a.step(&TickActions::NEUTRAL);
    assert!(a.read_body(hero, 0.25, &mut hero_body));
    assert!(a.read_body(warrior, 0.25, &mut warrior_body));
    assert_eq!(hero_body.span().unwrap().before, hero_body.span().unwrap().after);
    assert_eq!(hero_body.span().unwrap().after.generation, 1);
    assert_eq!(warrior_body.span().unwrap().fraction, 0.25);
    assert_eq!(warrior_body.span().unwrap().after.generation, 0);
}

#[test]
fn enabled_read_only_body_pair_preserves_melee_sweep_replay() {
    use crate::replay::{ARCHITECTURE, ChecksumFile, Player, checksums_file_name};

    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays");
    let bytes = std::fs::read(directory.join("melee-sweep.sreplay")).unwrap();
    let text = std::fs::read_to_string(directory.join(checksums_file_name(ARCHITECTURE))).unwrap();
    let recorded = ChecksumFile::parse(&text).unwrap();
    let replay = Replay::decode(&bytes).unwrap();
    let mut legacy = Player::new(replay.clone(), &SCENARIOS).unwrap();
    let mut attached = Player::new(replay, &SCENARIOS).unwrap();
    assert!(alloc::is_installed());
    let actors: [ActorId; 2] = attached.cast()[..2].try_into().unwrap();
    for (index, actor) in actors.into_iter().enumerate() {
        let profile = BodyProfile::bind(test_assets(), test_options(1.0)).unwrap();
        attached.simulation_mut().attach_body(actor, Some(actors[1 - index]), profile).unwrap();
    }
    assert_eq!(legacy.simulation().checksum(), legacy_state(attached.simulation().world()).finish());
    while legacy.step() {
        assert!(attached.step());
        let tick = legacy.simulation().tick();
        assert_eq!(
            legacy.simulation().checksum(),
            legacy_state(attached.simulation().world()).finish(),
            "ordinary gameplay differs at melee-sweep step {tick}",
        );
        if tick == 1 {
            assert!(actors.into_iter().all(|actor| attached.simulation().body_receipt(actor).is_some()));
        }
        assert_no_fixed_allocations(attached.simulation(), "body");
    }
    assert!(!attached.step());
    assert_eq!(legacy.simulation().tick(), 1200);
    assert_eq!(recorded.get("melee-sweep").unwrap().first_difference(&legacy.finish().trail), None);
}

#[test]
fn attaching_the_shadow_preserves_the_frozen_melee_sweep_schedule() {
    use crate::replay::{ARCHITECTURE, ChecksumFile, Player, checksums_file_name};

    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays");
    let bytes = std::fs::read(directory.join("melee-sweep.sreplay")).unwrap();
    let text = std::fs::read_to_string(directory.join(checksums_file_name(ARCHITECTURE))).unwrap();
    let recorded = ChecksumFile::parse(&text).unwrap();
    let replay = Replay::decode(&bytes).unwrap();
    let mut baseline = Player::new(replay.clone(), &SCENARIOS).unwrap();
    let mut shadowed = Player::new(replay, &SCENARIOS).unwrap();
    let actors: [ActorId; 2] = shadowed.cast()[..2].try_into().unwrap();
    assert!(alloc::is_installed());
    bind_test_shadow(shadowed.simulation_mut(), actors);
    while baseline.step() {
        assert!(shadowed.step());
        let sim = shadowed.simulation();
        assert_eq!(sim.overhead_shadow(), Some(OverheadShadowResult::Inactive));
        assert_eq!(
            baseline.simulation().checksum(),
            legacy_state(sim.world()).finish(),
            "ordinary gameplay differs at melee-sweep step {}",
            sim.tick(),
        );
        assert_no_fixed_allocations(sim, "attached shadow");
    }
    assert!(!shadowed.step());
    assert_eq!(baseline.simulation().tick(), 1200);
    assert_eq!(recorded.get("melee-sweep").unwrap().first_difference(&baseline.finish().trail), None);
}

fn bind_test_shadow(sim: &mut Simulation, actors: [ActorId; 2]) {
    for (index, name) in ["root", "handslot.r"].into_iter().enumerate() {
        let options = BodyProfileOptions { contact_joints: &[name], ..test_options(1.0) };
        let profile = BodyProfile::bind(test_assets_with_joint(name), options).unwrap();
        sim.attach_body(actors[index], Some(actors[1 - index]), profile).unwrap();
    }
    let point = |local| ShadowEndpoint { joint: 0, name: Name::new("root"), local };
    let shape = OverheadShadowShape {
        socket: 0,
        bracket: crate::OverheadShadowBracket::Strict,
        warrior_scale: 1.0,
        heroine_scale: 1.0,
        steel_vertices: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
        steel_triangles: vec![[0, 1, 2]],
        anatomy: vec![ShadowCapsule { ends: [point(Vec3::ZERO), point(Vec3::Y)], radius: 0.1 }],
    };
    assert!(sim.attach_overhead_shadow(actors[1], actors[0], shape));
}

#[test]
fn active_overhead_shadow_has_no_fixed_step_allocations() {
    let scenario = find_scenario(&SCENARIOS, "arena-fight").unwrap();
    let (mut baseline, baseline_cast) = scenario.start(scenario.config).unwrap();
    let (mut shadowed, cast) = scenario.start(scenario.config).unwrap();
    assert_eq!(baseline_cast, cast);
    let actors = cast[..2].try_into().unwrap();
    assert!(alloc::is_installed());
    bind_test_shadow(&mut shadowed, actors);
    for sim in [&mut baseline, &mut shadowed] {
        assert!(sim.force_move(actors[1], Name::new("overhead")));
    }

    let mut queried = 0;
    for tick in 1..=80 {
        baseline.step(&TickActions::NEUTRAL);
        shadowed.step(&TickActions::NEUTRAL);
        assert_eq!(
            baseline.checksum(),
            legacy_state(shadowed.world()).finish(),
            "ordinary gameplay differs at overhead step {tick}",
        );
        assert_no_fixed_allocations(&shadowed, "active overhead shadow");
        queried += usize::from(matches!(
            shadowed.overhead_shadow(),
            Some(OverheadShadowResult::Separated { probes: 1.. } | OverheadShadowResult::Witness { .. }),
        ));
    }
    assert!(queried > 0, "the fixture never reached a real overhead geometry query");
}

#[test]
fn forming_a_party_cuts_a_benched_body_even_when_it_swaps_in_next_step() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sapphire_moves::MoveSetDocument::parse(
        r#"(type: "sapphire::moves::MoveSetDocument", version: 1, data: (
            intents: [(name: "swap", button: Swap, buffer: 12, priority: 4)],
            moves: [(name: "swap_in", startup: 1, active: 1, recovery: 1)],
        ))"#,
    )
    .unwrap()
    .compile()
    .unwrap();
    let moves = Some(sim.add_move_set(moves));
    let lead = sim.spawn(ActorSpawn { position: -3.0 * Vec3::X, moves, ..ActorSpawn::default() }).unwrap();
    let actor = sim.spawn(ActorSpawn { position: 2.0 * Vec3::X, moves, ..ActorSpawn::default() }).unwrap();
    sim.attach_body(actor, None, BodyProfile::bind(test_assets(), test_options(1.0)).unwrap()).unwrap();
    let mut sample = sim.body_sample_buffer(actor).unwrap();
    for _ in 0..2 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert!(sim.read_body(actor, 1.0, &mut sample));
    assert_eq!(sample.root().translation, 2.0 * Vec3::X);

    sim.form_party(&[lead, actor], crate::replay::PARTY_TUNING).unwrap();
    assert!(sim.actor(actor).unwrap().off_field);
    assert!(!sim.read_body(actor, 1.0, &mut sample));
    let mut actions = TickActions::NEUTRAL;
    actions.set(lead, crate::ActorActions { buttons: crate::Buttons::SWAP, stick: Stick::CENTER }).unwrap();
    sim.step(&actions);
    assert_eq!(sim.party().active(), Some(actor));
    assert!(sim.read_body(actor, 0.25, &mut sample));
    let span = sample.span().unwrap();
    assert_eq!(span.before, span.after);
    assert_eq!(span.after.generation, 1);
    assert_eq!(sample.root().translation, -3.0 * Vec3::X);
}

#[derive(Clone, Copy)]
enum BodyRevisionOrder {
    BeforeBinding,
    AfterBinding,
}

fn late_overhead_fixture(
    apply: bool,
    bracket: crate::OverheadShadowBracket,
    revision: Option<BodyRevisionOrder>,
) -> (Simulation, [ActorId; 2]) {
    late_overhead_fixture_with_meshes(apply, bracket, revision, [[1; 32], [2; 32]])
}

fn late_overhead_fixture_with_meshes(
    apply: bool,
    bracket: crate::OverheadShadowBracket,
    revision: Option<BodyRevisionOrder>,
    meshes: [[u8; 32]; 2],
) -> (Simulation, [ActorId; 2]) {
    let mut sim = Simulation::new(SimConfig::default());
    let document = sapphire_moves::MoveSetDocument::parse(
        r#"(type: "sapphire::moves::MoveSetDocument", version: 1, data: (
            intents: [],
            fighter: (
                health: 100, poise: 30, poise_refill: 60,
                hurtboxes: [(name: "body", offset: (0.0, 0.8, 0.0), shape: Sphere(radius: 0.25))],
            ),
            moves: [
                (
                    name: "overhead", startup: 45, active: 6, recovery: 20,
                    motion: (
                        root: [
                            (at: 52, offset: (0.0, 0.0, 0.0)),
                            (at: 54, offset: (0.0, 0.0, 1.5)),
                            (at: 57, offset: (0.0, 0.0, 1.8)),
                        ],
                        paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 0.8, 2.0))])],
                    ),
                    hitboxes: [(
                        name: "overhead_hit", from: 45, to: 51, path: "blade", shape: Sphere(radius: 0.2),
                        attack: (damage: 17, poise: 3, weight: Heavy, hitstop: 4),
                    )],
                ),
                (name: "off", startup: 0, active: 4, recovery: 0,
                    hurtboxes: [(name: "body", from: 0, to: 1, state: Off)]),
                (name: "immune", startup: 0, active: 4, recovery: 0, invulnerable: [(from: 0, to: 1)]),
                (name: "armoured", startup: 0, active: 4, recovery: 0,
                    armour: [(from: 0, to: 4, threshold: 12)]),
            ],
        ))"#,
    )
    .unwrap();
    let hero_set = sim.add_move_set(document.compile().unwrap());
    let warrior_set = sim.add_move_set(document.compile().unwrap());
    let hero = sim
        .spawn(ActorSpawn { moves: Some(hero_set), position: 2.0 * Vec3::Z, ..ActorSpawn::default() })
        .unwrap();
    let warrior = sim.spawn(ActorSpawn { moves: Some(warrior_set), ..ActorSpawn::default() }).unwrap();
    let replace_body = |sim: &mut Simulation| {
        let mut document = document.clone();
        document.fighter.as_mut().unwrap().hurtboxes[0].shape = sapphire_moves::Shape::Sphere { radius: 0.3 };
        assert!(matches!(
            sim.tune_move_set(hero_set, document.compile().unwrap()),
            Some(crate::Tuned::Replaced { .. })
        ));
    };
    if matches!(revision, Some(BodyRevisionOrder::BeforeBinding)) {
        replace_body(&mut sim);
    }
    for (actor, target, joint) in [(hero, warrior, "root"), (warrior, hero, "handslot.r")] {
        let options = BodyProfileOptions { contact_joints: &[joint], ..test_options(1.0) };
        let profile = BodyProfile::bind(test_assets_with_joint(joint), options).unwrap();
        sim.attach_body(actor, Some(target), profile).unwrap();
    }
    let point = |y| ShadowEndpoint { joint: 0, name: Name::new("root"), local: Vec3::Y * y };
    let shape = OverheadShadowShape {
        socket: 0,
        bracket,
        warrior_scale: 1.0,
        heroine_scale: 1.0,
        steel_vertices: vec![Vec3::new(-0.2, 0.5, 0.5), Vec3::new(0.2, 0.5, 0.5), Vec3::new(0.0, 1.4, 0.5)],
        steel_triangles: vec![[0, 1, 2]],
        anatomy: vec![ShadowCapsule { ends: [point(0.7), point(0.9)], radius: 0.12 }],
    };
    if apply {
        assert!(sim.attach_overhead_application(warrior, hero, shape, meshes));
    } else {
        assert!(sim.attach_overhead_shadow(warrior, hero, shape));
    }
    if matches!(revision, Some(BodyRevisionOrder::AfterBinding)) {
        replace_body(&mut sim);
    }
    assert!(sim.force_move(warrior, Name::new("overhead")));
    (sim, [hero, warrior])
}

#[test]
fn refined_overhead_replay_identity_binds_each_calibrated_mesh() {
    use crate::OverheadShadowBracket::EngineSpatial;

    let meshes = [[1; 32], [2; 32]];
    let (configured, _) = late_overhead_fixture_with_meshes(true, EngineSpatial, None, meshes);
    let (shadow, _) = late_overhead_fixture_with_meshes(false, EngineSpatial, None, meshes);
    for index in 0..2 {
        let mut changed = meshes;
        changed[index][31] ^= 1;
        let (alternate, _) = late_overhead_fixture_with_meshes(true, EngineSpatial, None, changed);
        let (shadow_alternate, _) = late_overhead_fixture_with_meshes(false, EngineSpatial, None, changed);
        assert_ne!(configured.tables_hash(), alternate.tables_hash());
        assert_ne!(configured.checksum(), alternate.checksum());
        assert_eq!(shadow.tables_hash(), shadow_alternate.tables_hash());
        assert_eq!(shadow.checksum(), shadow_alternate.checksum());
    }
}

#[test]
fn refined_overhead_resolves_the_original_attack_after_its_published_volume_ended() {
    use crate::OverheadShadowBracket::EngineSpatial;

    let (mut passive, [hero, _]) = late_overhead_fixture(false, EngineSpatial, None);
    let (mut sim, [applied_hero, warrior]) = late_overhead_fixture(true, EngineSpatial, None);
    assert_eq!(hero, applied_hero);
    assert_ne!(sim.tables_hash(), passive.tables_hash());
    assert_ne!(sim.checksum(), passive.checksum());
    assert!(alloc::is_installed());

    let mut delivered = None;
    for _ in 0..66 {
        passive.step(&TickActions::NEUTRAL);
        sim.step(&TickActions::NEUTRAL);
        assert_no_fixed_allocations(&sim, "applied overhead");
        if let Some(receipt) = sim.applied_overhead_contact() {
            assert!(delivered.replace(receipt).is_none(), "the swing hit twice");
            let attack = sim.actor(warrior).unwrap();
            assert!(attack.moves.volumes.iter().next().is_none());
            assert_eq!(attack.moves.current.unwrap().serial, receipt.event.instance);
            assert_eq!(receipt.event.group, Name::new("overhead_hit"));
            assert_eq!(receipt.event.volume, crate::HitEvent::PRIVATE_VOLUME);
            assert_eq!(receipt.pair_stop_ticks, 4);
            assert!((53_000.0..54_000.0).contains(&receipt.witness.source_subticks));
            assert_eq!(sim.hits().events(), &[receipt.event]);
            let source = sim.hits().private_attack_source(&receipt.event).unwrap();
            let active = attack.moves.current.unwrap();
            assert_eq!(source.movement, active.at);
            assert_eq!(source.started_at, active.started_at);
            let mismatched = crate::HitEvent { instance: receipt.event.instance + 1, ..receipt.event };
            assert!(sim.hits().private_attack_source(&mismatched).is_none());
            sim.cut_body(hero);
            assert!(sim.applied_overhead_contact().is_none());
            assert!(sim.hits().private_attack_source(&receipt.event).is_none());
        }
        if passive.actor(hero).unwrap().fighter.health() == 100 {
            assert_eq!(legacy_state(sim.world()).finish(), legacy_state(passive.world()).finish());
        }
        if delivered.is_none() {
            assert_eq!(sim.actor(hero).unwrap().fighter.health(), 100);
            assert_eq!(sim.hits().record_count(), 0);
        }
    }
    assert!(delivered.is_some());
    assert_eq!(sim.actor(hero).unwrap().fighter.health(), 83);
    assert_eq!(passive.actor(hero).unwrap().fighter.health(), 83);
    sim.restart_builtin_fight(SimConfig::default());
    assert!(sim.applied_overhead_contact().is_none());
    assert!(sim.hits().private_attack_source(&delivered.unwrap().event).is_none());
}

#[test]
fn refined_overhead_keeps_off_and_invulnerability_eligible_later_and_does_not_pair_armour() {
    use crate::OverheadShadowBracket::EngineSpatial;
    for action in ["off", "immune", "armoured"] {
        let (mut sim, [hero, warrior]) = late_overhead_fixture(true, EngineSpatial, None);
        for _ in 0..53 {
            sim.step(&TickActions::NEUTRAL);
        }
        assert_eq!(sim.actor(warrior).unwrap().moves.current.unwrap().tick(), 53);
        assert_eq!(sim.actor(hero).unwrap().fighter.health(), 100);
        assert!(sim.force_move(hero, Name::new(action)));
        sim.step(&TickActions::NEUTRAL);
        if action == "armoured" {
            let hit = sim.applied_overhead_contact().unwrap();
            assert_eq!(hit.pair_stop_ticks, 0);
            assert_eq!(sim.actor(hero).unwrap().hitstop_ticks, 0);
            assert_eq!(sim.actor(warrior).unwrap().hitstop_ticks, 4);
        } else {
            assert!(sim.applied_overhead_contact().is_none());
            assert_eq!(sim.hits().record_count(), 0);
            assert_eq!(sim.actor(hero).unwrap().fighter.health(), 100);
            sim.step(&TickActions::NEUTRAL);
            let hit = sim.applied_overhead_contact().unwrap();
            assert!(hit.witness.source_subticks >= 54_000.0);
            assert_eq!(hit.pair_stop_ticks, 4);
        }
        assert_eq!(sim.actor(hero).unwrap().fighter.health(), 83);
    }
}

#[test]
fn refined_overhead_binds_exact_target_hurtbox_revision() {
    use crate::OverheadShadowBracket::EngineSpatial;
    let (mut stale, [hero, _]) =
        late_overhead_fixture(true, EngineSpatial, Some(BodyRevisionOrder::AfterBinding));
    let (mut current, _) = late_overhead_fixture(true, EngineSpatial, Some(BodyRevisionOrder::BeforeBinding));
    assert_eq!(legacy_state(stale.world()).finish(), legacy_state(current.world()).finish());
    assert_ne!(stale.tables_hash(), current.tables_hash());
    assert_ne!(stale.checksum(), current.checksum());

    let mut valid_contact = None;
    for _ in 0..60 {
        stale.step(&TickActions::NEUTRAL);
        current.step(&TickActions::NEUTRAL);
        assert!(stale.applied_overhead_contact().is_none());
        assert_eq!(stale.hits().record_count(), 0);
        if let Some(contact) = current.applied_overhead_contact() {
            assert!(valid_contact.replace(contact).is_none());
        }
    }
    assert!(valid_contact.is_some());
    assert_eq!(current.actor(hero).unwrap().fighter.health(), 83);
    assert_eq!(stale.actor(hero).unwrap().fighter.health(), 100);
}

#[test]
fn retained_contact_sample_copies_exactly_only_into_the_same_body_layout() {
    let (mut sim, actor) = with_body(1.0);
    let (other, other_actor) = with_body(1.0);
    let mut sample = sim.body_sample_buffer(actor).unwrap();
    let mut presentation = sim.body_sample_buffer(actor).unwrap();
    let mut wrong_binding = other.body_sample_buffer(other_actor).unwrap();
    assert!(!presentation.copy_from_sample(&sample));
    sim.step(&TickActions::NEUTRAL);
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.read_body(actor, 0.25, &mut sample));
    assert!(alloc::is_installed());
    let allocations = AllocScope::begin();
    for _ in 0..8 {
        assert!(presentation.copy_from_sample(&sample));
        assert_eq!(presentation.span(), sample.span());
        assert_eq!(presentation.pose_key(), sample.pose_key());
        assert_eq!(presentation.root(), sample.root());
        assert_eq!(presentation.grounded(), sample.grounded());
        assert_eq!(presentation.locals(), sample.locals());
        assert_eq!(presentation.model(), sample.model());
        assert_eq!(presentation.morphs(), sample.morphs());
        assert!(!wrong_binding.copy_from_sample(&sample));
        assert!(wrong_binding.span().is_none());
    }
    if alloc::PER_THREAD_COUNTS {
        assert_eq!(allocations.counts().allocations, 0);
    }
}

#[test]
fn presentation_release_preserves_live_motion_and_has_exact_endpoints() {
    use crate::OverheadShadowBracket::EngineSpatial;
    let (mut sim, [_, warrior]) = late_overhead_fixture(true, EngineSpatial, None);
    let mut held = sim.body_sample_buffer(warrior).unwrap();
    let mut origin = sim.body_sample_buffer(warrior).unwrap();
    let mut live = sim.body_sample_buffer(warrior).unwrap();
    let mut presented = sim.body_sample_buffer(warrior).unwrap();
    for _ in 0..54 {
        sim.step(&TickActions::NEUTRAL);
    }
    let impact = sim.applied_overhead_contact().unwrap();
    assert!(sim.read_body(warrior, impact.witness.witnessed_fraction, &mut held));
    for _ in 0..5 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert!(sim.read_body(warrior, 0.0, &mut origin));
    assert!(sim.read_body(warrior, 1.0, &mut live));
    let owner = sim.checksum();
    let allocations = AllocScope::begin();
    assert!(presented.copy_from_sample(&live));
    assert!(presented.apply_impact_residual(&held, &origin, 0.0));
    assert_eq!(presented.root(), live.root());
    assert_eq!(presented.model(), live.model());
    assert_eq!(presented.morphs(), live.morphs());

    assert!(presented.apply_impact_residual(&held, &origin, 1.0));
    assert_eq!(presented.root(), held.root());
    assert_eq!(presented.locals(), held.locals());
    assert_eq!(presented.model(), held.model());
    assert_eq!(presented.morphs(), held.morphs());
    assert_eq!(presented.span(), live.span());

    assert!(presented.copy_from_sample(&live));
    assert!(presented.apply_impact_residual(&held, &origin, 0.5));
    let source_motion = live.root().translation - origin.root().translation;
    let debt = held.root().translation - origin.root().translation;
    assert!(source_motion.length() > 0.05);
    assert!(debt.length() > 0.05);
    assert_eq!(presented.root().translation, live.root().translation + debt * 0.5);
    assert_eq!(sim.checksum(), owner);
    if alloc::PER_THREAD_COUNTS {
        assert_eq!(allocations.counts().allocations, 0);
    }
}
