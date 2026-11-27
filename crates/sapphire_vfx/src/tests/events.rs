//! M6 N3's contracts: events fire where and when their documents say, at most a step's cap of them, counted, and
//! adding one moves no other particle.

use super::*;
use crate::world::ParticleState;
use crate::{EventTrigger, Force, Ground, ParticleEvent};

fn event(on: EventTrigger, emitter: &str, inherit: f32) -> ParticleEvent {
    ParticleEvent { on, emitter: emitter.to_owned(), inherit }
}

/// A sub-emitter called `name`: a burst of `count`, still at birth, living `life` seconds.
fn sub(name: &str, count: u32, life: f32) -> EmitterDesc {
    let mut emitter = dots(name, Spawn::Burst { count });
    emitter.lifetime = (life, life);
    emitter.motion = Motion { speed: (0.0, 0.0), ..Motion::default() };
    emitter
}

fn effect_of(name: &str, emitters: Vec<EmitterDesc>) -> Effect {
    Effect { name: name.to_owned(), emitters, ..burst(1) }
}

fn steps(world: &mut VfxWorld, ticks: u32) {
    for _ in 0..ticks {
        world.advance(&ALL_RUN);
    }
}

/// The live particles of the emitter at `emitter` of every effect.
fn of_emitter(world: &VfxWorld, emitter: u8) -> Vec<ParticleState> {
    world.particle_states().into_iter().filter(|state| state.emitter == emitter).collect()
}

/// The firing effect of the bar's tests: an emitter that bursts, lives and lands, a noise-swirled one beside it
/// bouncing on the ground, and, `with_events`, the first's events and the sub-emitters they name appended, two deep.
fn fire(with_events: bool) -> Effect {
    let mut sparks = dots("sparks", Spawn::Repeat { count: 6, interval: 0.25, times: 0 });
    sparks.lifetime = (0.3, 0.6);
    sparks.motion = Motion { speed: (1.0, 3.0), spread: 50.0, gravity: 9.8, drag: 0.5, ..Motion::default() };
    sparks.collide = Some(Ground { bounce: 0.4, friction: 0.2, dies: false });
    let mut beside = dots("beside", Spawn::Rate { per_second: 40.0, duration: 0.0 });
    beside.lifetime = (0.4, 0.9);
    beside.motion = Motion { speed: (0.5, 2.0), spread: 90.0, gravity: 4.0, ..Motion::default() };
    beside.forces = vec![Force::Noise { strength: 3.0, frequency: 1.5, octaves: 2, drift: 0.4 }];
    beside.collide = Some(Ground { bounce: 0.2, friction: 0.5, dies: false });
    let mut emitters = vec![sparks, beside];
    if with_events {
        emitters[0].events = vec![
            event(EventTrigger::Birth, "flash", 0.0),
            event(EventTrigger::Death, "embers", 0.3),
            event(EventTrigger::Collide, "dust", 0.5),
        ];
        let mut embers = sub("embers", 5, 0.3);
        embers.motion = Motion { speed: (0.5, 1.5), spread: 180.0, gravity: 2.0, ..Motion::default() };
        embers.events = vec![event(EventTrigger::Death, "glint", 1.0)];
        emitters.extend([sub("flash", 1, 0.1), embers, sub("dust", 3, 0.2), sub("glint", 1, 0.1)]);
    }
    effect_of("fire", emitters)
}

/// The row's bar: an emitter gaining events, and the sub-emitters they name appended to its effect, moves no other
/// particle while both caps have room. Every particle of the effect's own emitters (the one firing, a noise-swirled
/// one beside it bouncing on the ground) and of an effect spawned after it holds its whole state to the bit, step
/// after step, while births, deaths and landings fire bursts two deep, one owner frozen now and then.
#[test]
fn adding_an_event_moves_no_other_particle() {
    let run = |with_events: bool| {
        let later = effect_of("later", vec![dots("later", Spawn::Rate { per_second: 30.0, duration: 0.0 })]);
        let mut world = world_of(vec![fire(with_events), later], VfxBudget::DESKTOP_LOW);
        let owner = OwnerId(3);
        world.spawn(EffectId(0), owner, Placement::at(Vec3::new(0.0, 0.5, 0.0))).expect("spawned");
        let mut seen = Vec::new();
        for tick in 0..150_u32 {
            if tick == 20 {
                world.spawn(EffectId(1), OwnerId::WORLD, Placement::at(Vec3::X)).expect("spawned");
            }
            let frozen = [(owner, 0.0)];
            let owners: &[(OwnerId, f32)] = if tick % 40 < 4 { &frozen } else { &[] };
            world.advance(&OwnerSteps { world: TICK, owners });
            // The effect's own two emitters, and the later effect's.
            seen.push(
                world.particle_states().into_iter().filter(|state| state.emitter < 2).collect::<Vec<_>>(),
            );
        }
        (seen, world.stats())
    };
    let ((plain, plain_stats), (evented, stats)) = (run(false), run(true));
    assert_eq!(plain_stats.events, 0);
    assert!(stats.events > 100, "{stats:?}");
    assert_eq!((stats.dropped_events, stats.dropped_particles), (0, 0), "{stats:?}");
    assert!(plain.iter().map(Vec::len).sum::<usize>() > 2000);
    for (tick, (plain, evented)) in plain.iter().zip(&evented).enumerate() {
        assert_eq!(plain, evented, "tick {tick}");
    }
}

/// Whatever order a step meets particles in, each firing draws the same numbers: an unrelated effect spawned after
/// the firing one, its particles dying among that one's and so moving them about the list a step walks, leaves
/// every particle of the firing effect, its sub-emitters' too, to the bit.
#[test]
fn the_order_a_step_meets_particles_in_moves_no_firing() {
    let run = |churned: bool| {
        let mut churn = dots("churn", Spawn::Rate { per_second: 90.0, duration: 0.0 });
        churn.lifetime = (0.05, 0.5);
        let mut world = world_of(vec![fire(true), effect_of("churn", vec![churn])], VfxBudget::DESKTOP_LOW);
        world.spawn(EffectId(0), OwnerId(3), Placement::at(Vec3::new(0.0, 0.5, 0.0))).expect("spawned");
        if churned {
            world.spawn(EffectId(1), OwnerId::WORLD, Placement::at(Vec3::X)).expect("spawned");
        }
        let (mut seen, mut orders) = (Vec::new(), Vec::new());
        for _ in 0..150 {
            world.advance(&ALL_RUN);
            seen.push(
                world.particle_states().into_iter().filter(|state| state.slot == 0).collect::<Vec<_>>(),
            );
            orders.push(world.met_order(0));
        }
        (seen, orders, world.stats())
    };
    let ((alone, alone_order, _), (churned, churned_order, stats)) = (run(false), run(true));
    assert_eq!((stats.dropped_events, stats.dropped_particles), (0, 0), "{stats:?}");
    assert!(stats.events > 100, "{stats:?}");
    let moved = alone_order.iter().zip(&churned_order).filter(|(alone, churned)| alone != churned).count();
    assert!(moved > 50, "the churn reorders the firing effect's particles in {moved} steps only");
    assert!(alone.iter().flatten().filter(|state| state.emitter >= 2).count() > 1000);
    for (tick, (alone, churned)) in alone.iter().zip(&churned).enumerate() {
        assert_eq!(alone, churned, "tick {tick}");
    }
}

/// A death's burst starts where its particle ended, a landing's on the ground where it first met it, a birth's
/// where it began, each carrying its share of that one's velocity; and a landing fires once, however often it
/// bounces after.
#[test]
fn events_fire_where_their_particles_are_with_their_share_of_its_speed() {
    let mut drop = dots("drop", Spawn::Burst { count: 1 });
    drop.lifetime = (2.0, 2.0);
    drop.motion = Motion { speed: (3.0, 3.0), direction: [1.0, 0.0, 0.0], gravity: 9.8, ..Motion::default() };
    drop.collide = Some(Ground { bounce: 0.8, friction: 0.0, dies: false });
    drop.events = vec![
        event(EventTrigger::Birth, "start", 1.0),
        event(EventTrigger::Collide, "splash", 0.5),
        event(EventTrigger::Death, "end", 0.0),
    ];
    let emitters = vec![drop, sub("start", 1, 0.05), sub("splash", 2, 0.05), sub("end", 1, 0.05)];
    let mut world = world_of(vec![effect_of("drop", emitters)], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::at(Vec3::new(0.0, 1.0, 0.0))).unwrap();
    world.set_ground(handle, 0.0);
    world.advance(&ALL_RUN);
    let [drop] = of_emitter(&world, 0)[..] else { panic!("one drop") };
    let [start] = of_emitter(&world, 1)[..] else { panic!("one start") };
    assert_eq!(
        (start.position, start.velocity),
        (drop.position, drop.velocity),
        "where it began, at its speed"
    );
    let mut splashes = Vec::new();
    let mut last = drop;
    for _ in 0..150 {
        world.advance(&ALL_RUN);
        let splash = of_emitter(&world, 2);
        if !splash.is_empty() && splashes.is_empty() {
            splashes = splash;
            let landed = of_emitter(&world, 0)[0];
            assert!(landed.landed);
            for splash in &splashes {
                assert_eq!(splash.position.y, 0.0, "on the ground");
                assert_eq!(splash.velocity, landed.velocity * 0.5, "half of what it leaves with");
            }
        }
        if let [drop] = of_emitter(&world, 0)[..] {
            last = drop;
        }
    }
    assert_eq!(splashes.len(), 2);
    // Two seconds' life: it has bounced since, and ended, and its end's burst is where it was last.
    let stats = world.stats();
    assert_eq!(stats.events, 3, "a birth, one landing and a death: {stats:?}");
    assert!(of_emitter(&world, 0).is_empty() && of_emitter(&world, 3).is_empty(), "all played out");
    assert!(last.position.x > 4.0, "it went on bouncing: {last:?}");
}

/// A death's burst, by any way of dying: a ground that ends particles fires its landing's and its death's at once.
#[test]
fn a_ground_that_ends_a_particle_fires_its_landing_and_its_death() {
    let mut drop = dots("drop", Spawn::Burst { count: 4 });
    drop.motion = Motion { speed: (1.0, 1.0), direction: [0.0, -1.0, 0.0], ..Motion::default() };
    drop.collide = Some(Ground { bounce: 0.0, friction: 0.0, dies: true });
    drop.events = vec![event(EventTrigger::Collide, "ring", 0.0), event(EventTrigger::Death, "puff", 1.0)];
    let emitters = vec![drop, sub("ring", 1, 1.0), sub("puff", 1, 1.0)];
    let mut world = world_of(vec![effect_of("rain", emitters)], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::at(Vec3::new(0.0, 0.1, 0.0))).unwrap();
    world.set_ground(handle, 0.0);
    steps(&mut world, 12);
    let (rings, puffs) = (of_emitter(&world, 1), of_emitter(&world, 2));
    assert_eq!((rings.len(), puffs.len(), world.stats().events), (4, 4, 8));
    for puff in puffs {
        assert_eq!(puff.position.y, 0.0, "put on the ground");
        assert_eq!(puff.velocity, Vec3::ZERO, "its speed into the ground gone");
    }
}

/// Each tier's cap on a step's events is the track's proposal; past it they're counted and dropped, and none waits
/// for a later step.
#[test]
fn past_a_steps_cap_events_are_dropped_and_counted_never_kept() {
    let caps =
        [VfxBudget::DESKTOP_HIGH, VfxBudget::DESKTOP_LOW, VfxBudget::MOBILE_HIGH, VfxBudget::MOBILE_LOW]
            .map(|budget| budget.max_events);
    assert_eq!(caps, [64, 48, 32, 16]);
    let mut many = dots("many", Spawn::Burst { count: 100 });
    many.lifetime = (0.4, 0.4);
    many.events = vec![event(EventTrigger::Death, "ash", 0.0)];
    let emitters = vec![many, sub("ash", 1, 1.0)];
    for budget in [VfxBudget::DESKTOP_HIGH, VfxBudget::MOBILE_LOW] {
        let cap = u64::from(budget.max_events);
        let mut world = world_of(vec![effect_of("many", emitters.clone())], budget);
        world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
        steps(&mut world, 31);
        let stats = world.stats();
        assert_eq!((stats.events, stats.dropped_events), (cap, 100 - cap), "{budget:?}");
        assert_eq!(of_emitter(&world, 1).len() as u64, cap);
        steps(&mut world, 5);
        assert_eq!(world.stats().events, cap, "nothing kept for a later step");
        world.start_over();
        assert_eq!((world.stats().events, world.stats().dropped_events), (0, 0));
    }
}

/// A frozen owner's particles neither die nor land, so they fire nothing until its clock moves; a stopped effect's
/// particles still fire as they end.
#[test]
fn a_frozen_owner_fires_nothing_and_a_stop_ends_no_event() {
    let mut spark = dots("spark", Spawn::Burst { count: 3 });
    spark.lifetime = (0.1, 0.1);
    spark.events = vec![event(EventTrigger::Death, "ember", 0.0)];
    let emitters = vec![spark, sub("ember", 2, 0.5)];
    let mut world = world_of(vec![effect_of("spark", emitters)], VfxBudget::DESKTOP_LOW);
    let owner = OwnerId(4);
    let handle = world.spawn(EffectId(0), owner, Placement::default()).unwrap();
    world.advance(&ALL_RUN);
    world.stop(handle);
    let held = [(owner, 0.0)];
    for _ in 0..30 {
        world.advance(&OwnerSteps { world: TICK, owners: &held });
    }
    assert_eq!(world.stats().events, 0, "held");
    steps(&mut world, 8);
    assert_eq!((world.stats().events, of_emitter(&world, 1).len()), (3, 6));
    assert!(world.is_playing(handle), "its embers keep it");
    steps(&mut world, 40);
    assert!(!world.is_playing(handle), "and it ends with them");
}

/// A sub-emitter fires only at its events: an effect whose events never fire never plays it.
#[test]
fn a_sub_emitter_fires_only_at_its_events() {
    let mut quiet = dots("quiet", Spawn::Burst { count: 2 });
    quiet.events = vec![event(EventTrigger::Collide, "never", 0.0)];
    quiet.collide = Some(Ground { bounce: 0.5, friction: 0.0, dies: false });
    quiet.motion.direction = [0.0, 1.0, 0.0];
    let emitters = vec![quiet, sub("never", 4, 1.0)];
    let mut world = world_of(vec![effect_of("quiet", emitters)], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
    steps(&mut world, 30);
    assert_eq!(world.stats().particles, 2, "the sub-emitter's burst never fired on its own");
    steps(&mut world, 40);
    assert!(!world.is_playing(handle), "it ends when its own particles have");
}

/// A burst past the particles' cap keeps what fits and counts the rest, as any emission does.
#[test]
fn a_burst_past_the_particle_cap_keeps_what_fits() {
    let mut spark = dots("spark", Spawn::Burst { count: 4 });
    spark.lifetime = (0.1, 0.1);
    spark.events = vec![event(EventTrigger::Death, "ember", 0.0)];
    let emitters = vec![spark, sub("ember", 4, 1.0)];
    let budget = VfxBudget { max_particles: 10, ..VfxBudget::DESKTOP_LOW };
    let mut world = world_of(vec![effect_of("spark", emitters)], budget);
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
    steps(&mut world, 8);
    let stats = world.stats();
    assert_eq!((stats.particles, stats.dropped_particles, stats.events), (10, 6, 4), "{stats:?}");
}
