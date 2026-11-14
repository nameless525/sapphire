//! A fight's crowd posed in jobs and by levels (M5 C1), with no GPU: the same poses in jobs as one after
//! another, no allocation of its own, and nothing the simulation does moved by a level.

use sapphire_anim::{AnimStats, Clip};
use sapphire_gameplay::replay::{SCENARIOS, find_scenario};
use sapphire_gameplay::{ActorId, TickActions};
use sapphire_math::Vec3;
use sapphire_stage::anim_levels::{AnimBand, AnimLevels};

use super::scene_tests::{GRAPH_CLIP, Staged, clip, fighting};
use super::*;

/// `scenario`'s fight on a stage of its own, stood after its first step: the stage, its simulation and its
/// cast.
fn crowd(table: &ClipTable<'_>, scenario: &str) -> (Staged, Simulation, Vec<ActorId>) {
    let (mut scene, _, _) = fighting(table);
    let scenario = find_scenario(&SCENARIOS, scenario).expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("it starts");
    sim.step(&TickActions::NEUTRAL);
    scene.follow_simulation(&sim, &cast);
    (scene, sim, cast)
}

/// What each fighter draws, in the roster's order: its palette and morph weights, to the bit.
fn poses(scene: &Staged) -> Vec<(Vec<[u32; 12]>, Vec<u32>)> {
    let arena = scene.show.arena.as_ref().expect("built");
    (0..arena.fighter_count())
        .map(|index| {
            let pose = scene.world().get::<RenderPose>(arena.entity(index)).expect("posed");
            let palette = pose.palette.iter().map(|joint| joint.to_cols_array().map(f32::to_bits)).collect();
            (palette, pose.morph_weights.iter().map(|weight| weight.to_bits()).collect())
        })
        .collect()
}

/// Bands so tight that everyone but the one on field is past the nearest: posed every fourth tick from 2 m,
/// with no IK or springs, and every other from 1 m.
const TIGHT: AnimLevels = AnimLevels {
    bands: [
        AnimBand::NEAR,
        AnimBand { from: 1.0, interval: 2, ik: true, springs: true },
        AnimBand { from: 2.0, interval: 4, ik: false, springs: false },
    ],
    margin: 0.2,
};

/// AnimStats less its times and its jobs, which differ between a frame in jobs and one without.
fn counted(stats: AnimStats) -> AnimStats {
    AnimStats { time: Duration::ZERO, spring_time: Duration::ZERO, jobs: 0, ..stats }
}

/// Fifteen fighters posed in jobs and one after another come to the same bits every frame, levels on (the
/// tier's and tight ones) and off, between ticks and on them.
#[test]
fn a_crowd_poses_the_same_in_jobs_as_one_after_another() {
    let bytes = clip().to_cooked().expect("cooked");
    let cooked = Clip::read(&bytes).expect("a valid clip");
    let table = ClipTable::new(0, [(GRAPH_CLIP, Some(cooked))].into_iter());
    for levels in [None, Some(AnimLevels::DESKTOP_LOW), Some(TIGHT)] {
        let (mut jobs, mut sim, cast) = crowd(&table, "crowd-15");
        let (mut serial, _, _) = crowd(&table, "crowd-15");
        for scene in [&mut jobs, &mut serial] {
            scene.scene.set_anim_levels_on(levels.is_some());
            scene.scene.set_anim_levels(levels.unwrap_or(AnimLevels::DESKTOP_HIGH));
        }
        serial.scene.set_anim_jobs(false);
        assert_eq!(poses(&jobs).len(), 15);
        let mut in_jobs = 0;
        for tick in 0..240 {
            sim.step(&TickActions::NEUTRAL);
            jobs.follow_simulation(&sim, &cast);
            serial.follow_simulation(&sim, &cast);
            for alpha in [0.25, 0.75, 1.0] {
                jobs.animate_fighters(&table, &table, alpha);
                serial.animate_fighters(&table, &table, alpha);
                let (stats, alone) = (jobs.anim_stats(), serial.anim_stats());
                assert_eq!(counted(stats), counted(alone), "{levels:?} at tick {tick}, {alpha}");
                assert_eq!(alone.jobs, 0);
                in_jobs += u32::from(stats.jobs > 1);
                assert!(
                    poses(&jobs) == poses(&serial),
                    "{levels:?}: the poses differ at tick {tick}, {alpha}"
                );
            }
        }
        assert!(in_jobs > 0, "{levels:?}: no frame ran in jobs");
    }
}

/// The retained lists cost the heap nothing: one after another, a crowd of 15 or 60 allocates nothing at
/// all, and in jobs no more than the job system's few blocks a call and one a job, which the pool's threads
/// bound, not the crowd.
#[test]
fn a_crowd_allocates_nothing_but_the_job_systems() {
    let bytes = clip().to_cooked().expect("cooked");
    let cooked = Clip::read(&bytes).expect("a valid clip");
    let table = ClipTable::new(0, [(GRAPH_CLIP, Some(cooked))].into_iter());
    const FRAMES: u64 = 120;
    let mut per_frame = Vec::new();
    for scenario in ["crowd-15", "crowd-60"] {
        for jobs in [false, true] {
            let (mut scene, mut sim, cast) = crowd(&table, scenario);
            scene.scene.set_anim_jobs(jobs);
            // Levels off, so every frame poses every fighter: the most work a frame's lists hold.
            scene.scene.set_anim_levels_on(false);
            scene.animate_fighters(&table, &table, 1.0);
            let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
                for _ in 0..FRAMES {
                    sim.step(&TickActions::NEUTRAL);
                    scene.follow_simulation(&sim, &cast);
                    scene.animate_fighters(&table, &table, 0.5);
                }
            });
            let stats = scene.anim_stats();
            assert_eq!(stats.characters as usize, scenario[6..].parse::<usize>().expect("its count"));
            if !sapphire_core::alloc::PER_THREAD_COUNTS {
                continue;
            }
            if jobs {
                assert!(stats.jobs > 1, "{scenario}: {stats:?}");
                // The job system's: a few a call and one a job, which its threads bound.
                let bound = FRAMES * (4 + u64::from(stats.jobs));
                assert!(counts.allocations <= bound, "{scenario}: {counts:?} over {FRAMES} frames");
                per_frame.push(counts.allocations / FRAMES);
            } else {
                assert_eq!(counts.allocations, 0, "{scenario} one after another: {counts:?}");
            }
        }
    }
    // However many there are, no more jobs than a few for each of the pool's threads and the caller.
    let most = 4 + 2 * (sapphire_jobs::system().compute_pool().thread_num() as u64 + 1);
    for allocations in per_frame {
        assert!(allocations <= most, "{allocations} a frame, past the pool's {most}");
    }
}

/// What a step left of each of the cast: where it stands and faces, its health, and the move it's in.
fn cast_state(sim: &Simulation, cast: &[ActorId]) -> Vec<(Vec3, f32, u32, Option<u32>)> {
    cast.iter()
        .map(|&id| {
            let actor = sim.actor(id).expect("alive");
            (
                actor.position,
                actor.yaw,
                actor.fighter.health(),
                actor.moves.current.map(|current| current.serial),
            )
        })
        .collect()
}

/// Levels never move the simulation: a skirmish posed with tight levels, its farthest fighter behind the
/// camera and held off screen, steps exactly as one posed with levels off, checksum for checksum, and that
/// fighter moves, hits and is hit as it does there.
#[test]
fn a_far_fighter_out_of_sight_hits_is_hit_and_moves_as_with_levels_off() {
    let bytes = clip().to_cooked().expect("cooked");
    let cooked = Clip::read(&bytes).expect("a valid clip");
    let table = ClipTable::new(0, [(GRAPH_CLIP, Some(cooked))].into_iter());
    let (mut thinned, mut sim, cast) = crowd(&table, crate::arena::SKIRMISH_SCENARIO);
    let (mut whole, mut plain, plain_cast) = crowd(&table, crate::arena::SKIRMISH_SCENARIO);
    assert_eq!(cast, plain_cast);
    thinned.scene.set_anim_levels(TIGHT);
    whole.scene.set_anim_levels_on(false);
    // The camera stands by the player, looking away from whoever's farthest from them.
    let at = |id: ActorId| sim.actor(id).expect("alive").position;
    let far = *cast[1..]
        .iter()
        .max_by(|a, b| at(**a).distance(at(cast[0])).total_cmp(&at(**b).distance(at(cast[0]))))
        .expect("a skirmish has others");
    let away = (at(cast[0]) - at(far)).with_y(0.0).normalize();
    let eye = at(cast[0]) + Vec3::Y * 1.6 - away * 2.0;
    thinned.scene.camera_mut().set_view(eye, away, 0.8);

    let far_health = sim.actor(far).expect("alive").fighter.health();
    let far_from = at(far);
    let (mut held, mut off_screen, mut far_hidden) = (0, 0, 0);
    let mut hurt = false;
    for tick in 0..900 {
        let place = sim.actor(far).map(|actor| actor.position);
        let hidden = place.is_some_and(|place| !thinned.scene.level_view().shows(place + Vec3::Y * 0.8, 1.6));
        far_hidden += u32::from(hidden);
        sim.step(&TickActions::NEUTRAL);
        plain.step(&TickActions::NEUTRAL);
        assert_eq!(sim.checksum(), plain.checksum(), "tick {tick}");
        assert_eq!(cast_state(&sim, &cast), cast_state(&plain, &cast), "tick {tick}");
        thinned.follow_simulation(&sim, &cast);
        whole.follow_simulation(&plain, &cast);
        for alpha in [0.5, 1.0] {
            thinned.animate_fighters(&table, &table, alpha);
            whole.animate_fighters(&table, &table, alpha);
            let stats = thinned.anim_stats();
            held += stats.held;
            off_screen += stats.off_screen;
            assert_eq!(whole.anim_stats().held, 0);
        }
        hurt |= cast.iter().any(|&id| {
            sim.actor(id).is_some_and(|actor| actor.fighter.health() < actor.fighter.max_health())
        });
    }
    assert!(far_hidden > 100, "the far fighter was out of sight for {far_hidden} ticks");
    assert!(
        off_screen > 0 && held > off_screen,
        "some were held off screen, and some between ticks: {held}, {off_screen}"
    );
    assert!(hurt, "the skirmish landed hits");
    let now = sim.actor(far).expect("still there");
    assert!(
        now.position.distance(far_from) > 0.5 || now.fighter.health() != far_health,
        "the far fighter took part"
    );
}
