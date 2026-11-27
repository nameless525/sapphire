//! The simulation allocates nothing per frame once warmed (spec §5.16, rule R6's budget): a whole
//! duel's worth of effects spawned, stepped and resolved, again and again, under the counting
//! allocator. Effects spawn from inside the fixed step and the frame's write, so neither may touch
//! the heap.

use glam::Vec3;
use sapphire_core::alloc::{CountingAllocator, count_in_scope};
use sapphire_look::VfxFrame;
use sapphire_vfx::{EffectLibrary, OwnerId, OwnerSteps, Placement, VfxBudget, VfxWorld, WindSample};

#[global_allocator]
static ALLOCATOR: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

const TICK: f32 = 1.0 / 60.0;

#[test]
fn stepping_spawning_and_resolving_allocate_nothing_once_warmed() {
    let library = EffectLibrary::default_effects();
    let spark = library.find("hit_spark").expect("an effect");
    let sword = library.find("sword_trail").expect("an effect");
    let dust = library.find("dust_puff").expect("an effect");
    let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 7);
    let mut frame = VfxFrame::default();
    let (attacker, victim) = (OwnerId(1), OwnerId(2));
    let mut blade = None;

    // One cycle of the show: a swing with its trail, sampled at four sub-ticks, a hit with a
    // hitstop that freezes both fighters, and dust at the feet.
    let mut tick = 0_u32;
    let mut play = |world: &mut VfxWorld, frame: &mut VfxFrame, ticks: u32| {
        for _ in 0..ticks {
            let moment = tick % 60;
            let frozen = (20..26).contains(&moment);
            let held = [(attacker, 0.0), (victim, 0.0)];
            let owners: &[(OwnerId, f32)] = if frozen { &held } else { &[] };
            world.advance(&OwnerSteps { world: TICK, owners });
            if moment == 0 {
                blade = world.spawn(sword, attacker, Placement::default());
                world.spawn(dust, OwnerId::WORLD, Placement::at(Vec3::new(1.0, 0.0, 0.0)));
            }
            if moment == 20 {
                for _ in 0..8 {
                    world.spawn(spark, victim, Placement::at(Vec3::Y));
                }
            }
            if let Some(blade) = blade
                && !frozen
            {
                for sub in 0..4 {
                    let angle = (tick * 4 + sub) as f32 * 0.02;
                    let ago = TICK * (3 - sub) as f32 / 4.0;
                    let (sin, cos) = angle.sin_cos();
                    world.sample_trail(
                        blade,
                        Vec3::new(cos, sin, 0.0) * 0.3,
                        Vec3::new(cos, sin, 0.0) * 1.2,
                        ago,
                    );
                }
            }
            if moment == 40
                && let Some(blade) = blade.take()
            {
                world.stop(blade);
            }
            // Between two samples, where a frame's strip ends on a stretch of its own.
            world.write_frame(frame, 0.5);
            tick += 1;
        }
    };

    play(&mut world, &mut frame, 240);
    let peak = world.stats();
    let ((), counts) = count_in_scope(|| play(&mut world, &mut frame, 600));
    assert_eq!(counts.allocations, 0, "after warming, {peak:?}");
    assert!(frame.stats.dropped_particles == 0 && frame.stats.dropped_ribbon_points == 0);
}

/// A library's forces, its ground and its new spawns (M6 N2) step and resolve with nothing from the heap: the
/// shipped embers, bouncing sparks and charge, and a dust trail fired along the way it's moved under every
/// force at once, in a breeze that changes every step, one owner frozen now and then.
#[test]
fn forces_the_ground_and_the_new_spawns_allocate_nothing_once_warmed() {
    let shipped = EffectLibrary::default_effects();
    let effects =
        ["ember_drift", "spark_bounce", "charge_gather"].map(|name| shipped.find(name).expect("shipped"));
    let mut world = VfxWorld::new(shipped, VfxBudget::DESKTOP_LOW, 7);
    let trail = EffectLibrary::from_ron(DUST_TRAIL).expect("valid");
    let mut trails = VfxWorld::new(trail, VfxBudget::DESKTOP_LOW, 7);
    let mut frame = VfxFrame::default();
    let runner = OwnerId(1);
    let (mut tick, mut dust) = (0_u32, None);
    let mut play = |world: &mut VfxWorld, trails: &mut VfxWorld, frame: &mut VfxFrame, ticks: u32| {
        for _ in 0..ticks {
            let gust = (tick as f32 * 0.05).sin();
            let wind = WindSample { velocity: Vec3::new(1.0 + gust, 0.0, 0.3 * gust) };
            world.set_wind(wind);
            trails.set_wind(wind);
            let frozen = (tick % 90) < 6;
            let held = [(runner, 0.0)];
            let owners: &[(OwnerId, f32)] = if frozen { &held } else { &[] };
            world.advance(&OwnerSteps { world: TICK, owners });
            trails.advance(&OwnerSteps { world: TICK, owners });
            if tick % 120 == 0 {
                for (index, &effect) in effects.iter().enumerate() {
                    let handle =
                        world.spawn(effect, runner, Placement::at(Vec3::new(index as f32, 1.0, 0.0)));
                    if let Some(handle) = handle {
                        world.set_ground(handle, 0.0);
                    }
                }
            }
            if tick % 120 == 100 {
                world.clear();
            }
            let dust = *dust
                .get_or_insert_with(|| trails.spawn(sapphire_vfx::EffectId(0), runner, Placement::default()));
            if let Some(handle) = dust {
                let along = tick as f32 * 0.05;
                trails.set_placement(
                    handle,
                    Placement::at(Vec3::new(along.sin() * 3.0, 0.0, along.cos() * 3.0)),
                );
            }
            world.write_frame(frame, 0.5);
            trails.write_frame(frame, 0.5);
            tick += 1;
        }
    };
    // To 90 ticks after the third round of spawns, so the peak is a full one.
    play(&mut world, &mut trails, &mut frame, 330);
    let peak = (world.stats(), trails.stats());
    let ((), counts) = count_in_scope(|| play(&mut world, &mut trails, &mut frame, 720));
    assert_eq!(counts.allocations, 0, "after warming, {peak:?}");
    assert!(peak.0.particles > 100 && peak.1.particles > 100, "{peak:?}");
    assert_eq!(frame.stats.dropped_particles, 0);
}

/// A run's dust, fired along the way it's moved and pushed by every force there is, bouncing on the ground.
const DUST_TRAIL: &str = r#"(
    type: "sapphire::vfx::EffectLibrary",
    version: 5,
    data: (
        effects: [
            (
                name: "dust_trail",
                emitters: [
                    (
                        name: "dust",
                        spawn: PerMeter(per_meter: 12.0),
                        source: Disc(0.2),
                        lifetime: (0.8, 1.4),
                        motion: (speed: (0.3, 1.2), spread: 60.0, gravity: 2.0, drag: 1.0),
                        size: [(0.0, 0.05)],
                        color: [(0.0, (0.8, 0.7, 0.6))],
                        forces: [
                            Wind(response: 2.0),
                            Noise(strength: 2.0, frequency: 1.0, octaves: 2, drift: 0.5),
                            Vortex(axis: (0.0, 1.0, 0.0), strength: 1.0, pull: 0.5),
                            Attract(point: (0.0, 1.0, 0.0), strength: 0.5, kill_radius: 0.05),
                            LimitSpeed(max: 3.0),
                        ],
                        collide: Some((bounce: 0.3, friction: 0.2)),
                    ),
                    (
                        name: "pulse",
                        spawn: Repeat(count: 4, interval: 0.2, times: 0),
                        lifetime: (0.5, 0.5),
                        motion: (speed: (1.0, 2.0), spread: 90.0, gravity: 9.8),
                        size: [(0.0, 0.02)],
                        color: [(0.0, (2.0, 1.0, 0.5))],
                        collide: Some((dies: true)),
                    ),
                ],
            ),
        ],
    ),
)"#;

/// Events and sub-emitters (M6 N3) fire with nothing from the heap: the shipped bursting sparks and splashing
/// debris, two deep, beside a world whose deaths overflow Mobile-Low's cap of a step's events every second, so
/// the queue's drops are walked too, one owner frozen now and then.
#[test]
fn events_and_their_drops_allocate_nothing_once_warmed() {
    let shipped = EffectLibrary::default_effects();
    let effects = ["spark_burst", "debris_splash"].map(|name| shipped.find(name).expect("shipped"));
    let mut world = VfxWorld::new(shipped, VfxBudget::DESKTOP_LOW, 7);
    let flood = EffectLibrary::from_ron(DEATH_FLOOD).expect("valid");
    let mut floods = VfxWorld::new(flood, VfxBudget::MOBILE_LOW, 7);
    let mut frame = VfxFrame::default();
    let runner = OwnerId(1);
    let mut tick = 0_u32;
    let mut play = |world: &mut VfxWorld, floods: &mut VfxWorld, frame: &mut VfxFrame, ticks: u32| {
        for _ in 0..ticks {
            let frozen = (tick % 90) < 6;
            let held = [(runner, 0.0)];
            let owners: &[(OwnerId, f32)] = if frozen { &held } else { &[] };
            world.advance(&OwnerSteps { world: TICK, owners });
            floods.advance(&OwnerSteps { world: TICK, owners: &[] });
            if tick.is_multiple_of(120) {
                for (index, &effect) in effects.iter().enumerate() {
                    let at = Placement::at(Vec3::new(index as f32, 1.0, 0.0));
                    if let Some(handle) = world.spawn(effect, runner, at) {
                        world.set_ground(handle, 0.0);
                    }
                }
            }
            if tick % 120 == 100 {
                world.clear();
            }
            if tick.is_multiple_of(60) {
                floods.spawn(sapphire_vfx::EffectId(0), OwnerId::WORLD, Placement::default());
            }
            world.write_frame(frame, 0.5);
            floods.write_frame(frame, 0.5);
            tick += 1;
        }
    };
    play(&mut world, &mut floods, &mut frame, 330);
    let peak = (world.stats(), floods.stats());
    let ((), counts) = count_in_scope(|| play(&mut world, &mut floods, &mut frame, 720));
    assert_eq!(counts.allocations, 0, "after warming, {peak:?}");
    let (shown, flooded) = (world.stats(), floods.stats());
    assert!(shown.events > 500 && shown.dropped_events == 0, "{shown:?}");
    assert!(flooded.events > 100 && flooded.dropped_events > 1000, "{flooded:?}");
}

/// A burst of 120 that all die in one step, each firing an event: far past any tier's cap.
const DEATH_FLOOD: &str = r#"(
    type: "sapphire::vfx::EffectLibrary",
    version: 5,
    data: (
        effects: [
            (
                name: "death_flood",
                emitters: [
                    (
                        name: "many",
                        spawn: Burst(count: 120),
                        source: Sphere(0.5),
                        lifetime: (0.5, 0.5),
                        size: [(0.0, 0.02)],
                        color: [(0.0, (1.0, 1.0, 1.0))],
                        events: [(on: Death, emitter: "ash")],
                    ),
                    (
                        name: "ash",
                        spawn: Burst(count: 2),
                        lifetime: (0.3, 0.3),
                        motion: (speed: (0.2, 0.4), spread: 180.0),
                        size: [(0.0, 0.01)],
                        color: [(0.0, (0.5, 0.5, 0.5))],
                    ),
                ],
            ),
        ],
    ),
)"#;

/// The frame's capacity is all that warms: a frame holding a duel's capacity needs nothing else,
/// from the very first frame, in a world that's only just been made.
#[test]
fn a_new_world_needs_only_its_frames_capacity() {
    let library = EffectLibrary::default_effects();
    let spark = library.find("hit_spark").expect("an effect");
    let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 7);
    let mut frame = VfxFrame::default();
    frame.sprites.reserve(256);
    frame.ribbon_points.reserve(256);
    let ((), counts) = count_in_scope(|| {
        for _ in 0..4 {
            world.spawn(spark, OwnerId::WORLD, Placement::default());
        }
        for _ in 0..8 {
            world.advance(&OwnerSteps { world: TICK, owners: &[] });
            world.write_frame(&mut frame, 1.0);
        }
    });
    assert_eq!(counts.allocations, 0);
    assert!(!frame.sprites.is_empty());
}

#[test]
fn starting_over_reuses_the_effect_pools() {
    let library = EffectLibrary::default_effects();
    let spark = library.find("hit_spark").expect("an effect");
    let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 7);
    let old = world.spawn(spark, OwnerId::WORLD, Placement::default()).unwrap();
    world.advance(&OwnerSteps { world: TICK, owners: &[] });
    let ((), counts) = count_in_scope(|| {
        world.start_over();
        world.spawn(spark, OwnerId::WORLD, Placement::default());
        world.advance(&OwnerSteps { world: TICK, owners: &[] });
    });
    assert_eq!(counts.allocations, 0);
    assert!(!world.is_playing(old));
    assert!(world.stats().particles > 0);
}
