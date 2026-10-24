//! Combat-feel scenarios for the world and entity clocks (spec §4.5).

use sapphire_time::{ClockId, LocalClock, ScaleSource, ScaledClock, SimDuration, SimTime, TimeScale};

const ONE_TICK: SimDuration = SimDuration::from_ticks(1);

/// Runs one fixed step's clock updates in the documented order.
fn step(world: &mut ScaledClock, locals: &mut [&mut LocalClock]) {
    world.tick();
    for local in locals {
        local.tick(world);
    }
}

#[test]
fn hitstop_freezes_local_time_for_exactly_n_ticks() {
    for n in [1u16, 2, 3, 8, 30, u16::MAX / 64] {
        let mut world = ScaledClock::new();
        let mut attacker = world.new_local_clock();
        let mut bystander = world.new_local_clock();
        for _ in 0..5 {
            step(&mut world, &mut [&mut attacker, &mut bystander]);
        }
        let frozen_at = attacker.now();
        attacker.apply_hitstop(n);

        for remaining in (0..n).rev() {
            world.tick();
            assert_eq!(attacker.tick(&world), SimDuration::ZERO, "n = {n}");
            assert_eq!(attacker.now(), frozen_at, "n = {n}");
            assert_eq!(attacker.hitstop_ticks(), remaining, "n = {n}");
            assert_eq!(bystander.tick(&world), ONE_TICK, "hitstop is per entity");
        }
        world.tick();
        assert_eq!(attacker.tick(&world), ONE_TICK, "n = {n}: resumes on tick n + 1");
        assert_eq!(world.now().since(frozen_at), SimDuration::from_ticks(u64::from(n) + 1));
    }
}

#[test]
fn zero_hitstop_does_nothing() {
    let mut world = ScaledClock::new();
    let mut local = world.new_local_clock();
    local.apply_hitstop(0);
    step(&mut world, &mut [&mut local]);
    assert_eq!(local.last_delta(), ONE_TICK);
}

#[test]
fn overlapping_hitstop_keeps_the_longer_freeze() {
    let mut local = ScaledClock::new().new_local_clock();
    local.apply_hitstop(5);
    local.apply_hitstop(3);
    assert_eq!(local.hitstop_ticks(), 5);
    local.apply_hitstop(8);
    assert_eq!(local.hitstop_ticks(), 8);
}

#[test]
fn hitstop_counts_real_ticks_during_slow_motion() {
    let mut world = ScaledClock::new();
    world.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    let mut victim = world.new_local_clock();
    victim.apply_hitstop(4);
    for _ in 0..4 {
        step(&mut world, &mut [&mut victim]);
        assert_eq!(victim.last_delta(), SimDuration::ZERO);
    }
    step(&mut world, &mut [&mut victim]);
    assert_eq!(victim.last_delta(), SimDuration::from_subticks(200));
}

#[test]
fn hitstop_survives_a_pause() {
    let mut world = ScaledClock::new();
    let mut local = world.new_local_clock();
    local.apply_hitstop(3);
    step(&mut world, &mut [&mut local]);
    assert_eq!(local.hitstop_ticks(), 2);

    let pause = world.push_scale(ScaleSource::Pause, 0).expect("room");
    for _ in 0..100 {
        step(&mut world, &mut [&mut local]);
        assert_eq!(local.hitstop_ticks(), 2, "paused worlds do not consume hitstop");
        assert_eq!(local.last_delta(), SimDuration::ZERO);
    }
    world.pop_scale(pause);

    step(&mut world, &mut [&mut local]);
    step(&mut world, &mut [&mut local]);
    assert_eq!(local.hitstop_ticks(), 0);
    assert_eq!(local.now(), SimTime::ZERO);
    step(&mut world, &mut [&mut local]);
    assert_eq!(local.now(), SimTime::ZERO + ONE_TICK);
}

#[test]
fn perfect_dodge_slows_the_world_but_not_the_player() {
    let mut world = ScaledClock::new();
    let mut player = world.new_local_clock();
    let mut enemy = world.new_local_clock();

    let world_slow = world.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    let counter = player.push_scale(ScaleSource::PerfectDodge, 5000).expect("room");
    assert_eq!(world.effective_scale(), 0.2);
    assert_eq!(player.effective_scale(&world), 1.0);
    assert_eq!(enemy.effective_scale(&world), 0.2);

    for _ in 0..60 {
        step(&mut world, &mut [&mut player, &mut enemy]);
    }
    assert_eq!(world.now(), SimTime::from_subticks(12_000));
    assert_eq!(enemy.now(), SimTime::from_subticks(12_000));
    assert_eq!(player.now(), SimTime::from_subticks(60_000));

    world.pop_scale(world_slow);
    player.pop_scale(counter);
    step(&mut world, &mut [&mut player, &mut enemy]);
    assert_eq!(player.last_delta(), ONE_TICK);
    assert_eq!(enemy.last_delta(), ONE_TICK);
}

#[test]
fn partial_counter_scale_keeps_the_player_slightly_slowed() {
    let mut world = ScaledClock::new();
    world.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    let mut player = world.new_local_clock();
    player.push_scale(ScaleSource::PerfectDodge, 4500).expect("room");
    for _ in 0..60 {
        step(&mut world, &mut [&mut player]);
        assert_eq!(player.last_delta(), SimDuration::from_subticks(900));
    }
}

#[test]
fn nested_scales_pop_in_any_order() {
    let mut world = ScaledClock::new();
    let dodge = world.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    let cinematic = world.push_scale(ScaleSource::Cinematic, 500).expect("room");
    let debug = world.push_scale(ScaleSource::Debug, 3000).expect("room");
    assert_eq!(world.tick(), SimDuration::from_subticks(300));

    // The dodge ends first, from the bottom of the stack.
    assert_eq!(world.pop_scale(dodge), Some(TimeScale { source: ScaleSource::PerfectDodge, per_mille: 200 }));
    assert_eq!(world.tick(), SimDuration::from_subticks(1500));
    let order: Vec<_> = world.scales().map(|scale| scale.source).collect();
    assert_eq!(order, [ScaleSource::Cinematic, ScaleSource::Debug]);

    // Popping twice is harmless.
    assert_eq!(world.pop_scale(dodge), None);
    assert_eq!(world.pop_scale(debug).map(|scale| scale.per_mille), Some(3000));
    assert_eq!(world.tick(), SimDuration::from_subticks(500));
    assert!(world.pop_scale(cinematic).is_some());
    assert_eq!(world.tick(), ONE_TICK);
    assert_eq!(world.scales().len(), 0);

    // Handles issued after the pops stay distinct from the earlier ones.
    let again = world.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    assert_ne!(again, dodge);
    assert_eq!(world.pop_scale(dodge), None);
    assert!(world.pop_scale(again).is_some());
}

#[test]
fn remove_source_clears_only_that_source() {
    let mut world = ScaledClock::new();
    world.push_scale(ScaleSource::Cinematic, 500).expect("room");
    world.push_scale(ScaleSource::Debug, 2000).expect("room");
    world.push_scale(ScaleSource::Cinematic, 500).expect("room");
    assert_eq!(world.remove_source(ScaleSource::Cinematic), 2);
    assert_eq!(world.tick(), SimDuration::from_ticks(2));
}

#[test]
fn fractional_slow_motion_does_not_drift() {
    let mut world = ScaledClock::new();
    world.push_scale(ScaleSource::Cinematic, 300).expect("room");
    let mut local = world.new_local_clock();
    local.push_scale(ScaleSource::Gameplay, 333).expect("room");
    for _ in 0..6000 {
        step(&mut world, &mut [&mut local]);
    }
    // 0.3 × 0.333 = 0.0999× of 6000 ticks is exactly 599.4 ticks.
    assert_eq!(world.now(), SimTime::from_subticks(1_800_000));
    assert_eq!(local.now(), SimTime::from_subticks(599_400));

    // 0.333 × 1.001 = 0.333333× is 333.333 sub-ticks per tick: carried fractions make 3000 ticks
    // exactly 999,999 sub-ticks, where truncating each tick would give 999,000.
    let mut world = ScaledClock::new();
    world.push_scale(ScaleSource::Debug, 333).expect("room");
    let mut third = world.new_local_clock();
    third.push_scale(ScaleSource::Debug, 1001).expect("room");
    for _ in 0..3000 {
        step(&mut world, &mut [&mut third]);
    }
    assert_eq!(third.now(), SimTime::from_subticks(999_999));
}

/// Per tick: world time, player time, enemy time, enemy hitstop.
type Trace = Vec<(SimTime, SimTime, SimTime, u16)>;

/// Plays a fixed script of clock events and records every observable value.
fn scripted_run() -> (Trace, ScaledClock, LocalClock, LocalClock) {
    let mut world = ScaledClock::new();
    let mut player = world.new_local_clock();
    let mut enemy = world.new_local_clock();
    let mut trace = Vec::new();
    let mut dodge = None;
    let mut counter = None;
    let mut haste = None;
    for tick in 0u32..600 {
        match tick {
            30 => enemy.apply_hitstop(6),
            45 => {
                dodge = world.push_scale(ScaleSource::PerfectDodge, 200).ok();
                counter = player.push_scale(ScaleSource::PerfectDodge, 4500).ok();
            }
            60 => haste = enemy.push_scale(ScaleSource::Gameplay, 1250).ok(),
            90 => player.apply_hitstop(9),
            120 => {
                world.pop_scale(dodge.take().expect("pushed at 45"));
                player.pop_scale(counter.take().expect("pushed at 45"));
            }
            200 => {
                world.push_scale(ScaleSource::Pause, 0).expect("room");
                enemy.apply_hitstop(4);
            }
            260 => {
                world.remove_source(ScaleSource::Pause);
            }
            300 => {
                enemy.pop_scale(haste.take().expect("pushed at 60"));
                world.push_scale(ScaleSource::Cinematic, 333).expect("room");
            }
            _ => {}
        }
        step(&mut world, &mut [&mut player, &mut enemy]);
        trace.push((world.now(), player.now(), enemy.now(), enemy.hitstop_ticks()));
    }
    (trace, world, player, enemy)
}

#[test]
fn identical_inputs_give_identical_subtick_totals() {
    let first = scripted_run();
    let second = scripted_run();
    assert_eq!(first, second);

    // Pin the totals so a change to the arithmetic shows up as a test failure, not as replay
    // desyncs later. Each is derivable by hand from the script above.
    let (_, world, player, enemy) = first;
    // World: 45 normal + 75 at 0.2× + 80 normal + 60 paused + 40 normal + 300 at 0.333×.
    assert_eq!(world.now(), SimTime::from_subticks(45_000 + 15_000 + 80_000 + 40_000 + 99_900));
    // Player: 45 normal + 66 at 0.9× (75 minus 9 hitstop) + 80 + 40 + 300 at 0.333×.
    assert_eq!(player.now(), SimTime::from_subticks(45_000 + 59_400 + 80_000 + 40_000 + 99_900));
    // Enemy: 30 normal, 6 hitstop, 9 normal, 15 at 0.2×, 60 at 0.25×, 80 at 1.25×, paused 60
    // (hitstop 4 frozen), 4 hitstop, 36 at 1.25×, 300 at 0.333×.
    assert_eq!(
        enemy.now(),
        SimTime::from_subticks(30_000 + 9_000 + 3_000 + 15_000 + 100_000 + 45_000 + 99_900)
    );
}

#[test]
fn clock_ids_are_issued_in_creation_order() {
    let mut world = ScaledClock::new();
    assert_eq!(world.id(), ClockId::WORLD);
    let ids: Vec<ClockId> = (0..3).map(|_| world.new_local_clock().id()).collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(!ids.contains(&ClockId::WORLD));

    // The same creation order replays to the same ids.
    let mut again = ScaledClock::new();
    let replayed: Vec<ClockId> = (0..3).map(|_| again.new_local_clock().id()).collect();
    assert_eq!(ids, replayed);
}

/// Pops `handle` on `clock`, returning `None` in release builds and `Err` when the debug
/// assertion for a foreign handle fires.
fn pop_foreign(
    clock: LocalClock,
    handle: sapphire_time::ScaleHandle,
) -> std::thread::Result<Option<TimeScale>> {
    std::panic::catch_unwind(move || {
        let mut clock = clock;
        clock.pop_scale(handle)
    })
}

#[test]
fn a_world_handle_cannot_pop_a_local_scale() {
    let mut world = ScaledClock::new();
    let mut player = world.new_local_clock();
    let world_slow = world.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    player.push_scale(ScaleSource::Gameplay, 1500).expect("room");
    let result = pop_foreign(player.clone(), world_slow);
    if cfg!(debug_assertions) {
        assert!(result.is_err(), "debug builds flag the caller bug");
    } else {
        assert_eq!(result.expect("no assertion in release"), None);
    }
    // The player's own scale is untouched either way, and the world handle still works.
    assert_eq!(player.scales().len(), 1);
    assert!(world.pop_scale(world_slow).is_some());
}

#[test]
fn a_stale_handle_cannot_pop_a_respawned_clock() {
    let mut world = ScaledClock::new();
    // The enemy despawns, but gameplay code keeps its handle.
    let slow_field = world.new_local_clock().push_scale(ScaleSource::Gameplay, 500).expect("room");

    // The respawned entity's clock issues a handle with the same serial, but on a new id.
    let mut respawned = world.new_local_clock();
    let dodge = respawned.push_scale(ScaleSource::PerfectDodge, 5000).expect("room");
    assert_ne!(dodge, slow_field);
    let result = pop_foreign(respawned.clone(), slow_field);
    if cfg!(debug_assertions) {
        assert!(result.is_err());
    } else {
        assert_eq!(result.expect("no assertion in release"), None);
    }
    assert_eq!(respawned.scales().len(), 1);
    assert!(respawned.pop_scale(dodge).is_some());
}

#[test]
fn large_scales_are_exact() {
    for (world_per_mille, local_per_mille) in [
        (1_000_000u32, None),
        (100_000, Some(4000u32)),
        (341_000, None),
        (400_000, None),
        (20_000, Some(20_000)),
        (300_000, Some(2500)),
    ] {
        let mut world = ScaledClock::new();
        world.push_scale(ScaleSource::Debug, world_per_mille).expect("room");
        let mut local = world.new_local_clock();
        if let Some(per_mille) = local_per_mille {
            local.push_scale(ScaleSource::Gameplay, per_mille).expect("room");
        }
        let expected_local = u64::from(world_per_mille) * u64::from(local_per_mille.unwrap_or(1000)) / 1000;
        world.tick();
        assert_eq!(world.last_delta().as_subticks(), u64::from(world_per_mille), "{world_per_mille}‰");
        assert_eq!(
            local.tick(&world).as_subticks(),
            expected_local,
            "{world_per_mille}‰ × {local_per_mille:?}"
        );
        let expected_scale = expected_local as f32 / 1000.0;
        assert!((local.effective_scale(&world) - expected_scale).abs() <= expected_scale * 1e-6);
    }
}

#[test]
fn absurd_scales_saturate_at_a_billion_times() {
    let mut world = ScaledClock::new();
    world.push_scale(ScaleSource::Debug, 100_000_000).expect("room");
    let mut local = world.new_local_clock();
    local.push_scale(ScaleSource::Debug, 100_000_000).expect("room");
    world.tick();
    // 10⁵× · 10⁵× would be 10¹⁰×; it stops at 10⁹× (10¹² sub-ticks per tick).
    assert_eq!(local.tick(&world).as_subticks(), 1_000_000_000_000);
}
