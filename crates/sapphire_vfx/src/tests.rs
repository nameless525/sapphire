//! The simulation's contracts: owners' clocks, caps, seeds, and what a trail resolves to.

use std::time::Duration;

use glam::{Quat, Vec3};
use sapphire_look::{VfxFrame, VfxShape, VfxSprite};
use sapphire_time::Stopwatch;

use crate::curve::Curve;
use crate::document::MAX_RATE;
use crate::{
    Effect, EffectId, EffectLibrary, EmitterDesc, Facing, Light, MAX_EMITTERS, Motion, OwnerId, OwnerSteps,
    Placement, Shape, Source, Spawn, TrailCore, TrailDesc, VfxBudget, VfxWorld,
};

const TICK: f32 = 1.0 / 60.0;

fn dots(name: &str, spawn: Spawn) -> EmitterDesc {
    EmitterDesc {
        name: name.to_owned(),
        spawn,
        delay: 0.0,
        source: Source::Point,
        lifetime: (1.0, 1.0),
        motion: Motion { speed: (2.0, 2.0), ..Motion::default() },
        shape: Shape::SoftDot,
        facing: Facing::default(),
        light: Light::Emissive,
        additive: 1.0,
        soft: 0.02,
        size_scale: (1.0, 1.0),
        size: Curve(vec![(0.0, 0.1)]),
        color: Curve(vec![(0.0, [1.0, 1.0, 1.0])]),
        alpha: Curve(vec![(0.0, 1.0), (1.0, 0.0)]),
    }
}

fn burst(count: u32) -> Effect {
    Effect {
        name: "burst".to_owned(),
        emitters: vec![dots("dots", Spawn::Burst { count })],
        trail: None,
        mark: false,
    }
}

fn trail(max_samples: u32) -> Effect {
    Effect {
        name: "trail".to_owned(),
        emitters: Vec::new(),
        trail: Some(TrailDesc {
            lifetime: 0.5,
            max_samples,
            width: Curve(vec![(0.0, 1.0)]),
            anchor: 1.0,
            color: Curve(vec![(0.0, [0.2, 0.5, 1.0])]),
            alpha: Curve(vec![(0.0, 1.0), (1.0, 0.0)]),
            core: TrailCore { position: 0.9, half_width: 0.05, color: Curve(vec![(0.0, [4.0; 3])]) },
            glow_reach: 0.8,
            start_fade: 0.0,
            additive: 0.8,
            soft: 0.02,
        }),
        mark: false,
    }
}

fn world_of(effects: Vec<Effect>, budget: VfxBudget) -> VfxWorld {
    VfxWorld::new(EffectLibrary::new(effects).expect("valid"), budget, 7)
}

fn frame_of(world: &mut VfxWorld) -> VfxFrame {
    let mut frame = VfxFrame::default();
    world.write_frame(&mut frame, 1.0);
    frame
}

/// Steps every owner by a tick.
const ALL_RUN: OwnerSteps<'static> = OwnerSteps { world: TICK, owners: &[] };

/// The frozen-owner test: hitstop freezes one owner's effects while another's, and the world's,
/// keep running.
#[test]
fn a_trails_start_grows_from_nothing() {
    let mut effect = trail(48);
    effect.trail.as_mut().unwrap().start_fade = 0.1;
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_HIGH);
    let id = world.library().find("trail").unwrap();
    let handle = world.spawn(id, OwnerId::WORLD, Placement::default()).unwrap();
    for tick in 0..12 {
        world.advance(&OwnerSteps { world: 0.02, owners: &[] });
        let x = tick as f32 * 0.1;
        world.sample_trail(handle, Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 0.0), 0.0);
    }
    let frame = frame_of(&mut world);
    let width = |point: &sapphire_look::VfxRibbonPoint| point.tip.distance(point.base);
    let first = frame.ribbon_points.first().unwrap();
    let last = frame.ribbon_points.last().unwrap();
    assert!(width(first) < 0.15, "{}", width(first));
    assert!(first.alpha < 0.15, "{}", first.alpha);
    // Further than the fade's reach, the curves alone say what it is.
    assert!(width(last) > 0.9, "{}", width(last));
    assert!(last.alpha > 0.9, "{}", last.alpha);
}

#[test]
fn a_frozen_owner_holds_its_effects_while_others_run() {
    let library = vec![burst(4), trail(32)];
    let mut world = world_of(library, VfxBudget::DESKTOP_HIGH);
    let (frozen, running) = (OwnerId(1), OwnerId(2));
    let spark = EffectId(0);
    world.spawn(spark, frozen, Placement::at(Vec3::X)).expect("spawned");
    world.spawn(spark, running, Placement::at(Vec3::NEG_X)).expect("spawned");
    world.spawn(spark, OwnerId::WORLD, Placement::at(Vec3::Z)).expect("spawned");
    let blade = world.spawn(EffectId(1), frozen, Placement::default()).expect("spawned");
    // A few ticks with everything running, so all of it has particles and samples.
    for tick in 0..6 {
        world.advance(&ALL_RUN);
        let swing = tick as f32 * 0.1;
        world.sample_trail(blade, Vec3::new(swing, 0.0, 0.0), Vec3::new(swing, 1.0, 0.0), 0.0);
    }
    let by_owner = |frame: &VfxFrame, near: Vec3| -> Vec<VfxSprite> {
        // Each owner's particles rise from where its effect stands.
        frame.sprites.iter().filter(|sprite| sprite.position.distance(near) < 0.5).copied().collect()
    };
    let before = frame_of(&mut world);
    let held = by_owner(&before, Vec3::X);
    assert_eq!(held.len(), 4);

    // Hitstop: `frozen`'s clock stops, and the rest go on.
    let steps = OwnerSteps { world: TICK, owners: &[(frozen, 0.0)] };
    for _ in 0..5 {
        world.advance(&steps);
        // A frozen owner's pose repeats, adding nothing.
        world.sample_trail(blade, Vec3::new(0.5, 0.0, 0.0), Vec3::new(0.5, 1.0, 0.0), 0.0);
    }
    let after = frame_of(&mut world);
    assert_eq!(by_owner(&after, Vec3::X), held, "not one frozen particle moved or aged");
    assert_eq!(after.ribbon_points, before.ribbon_points, "nor its trail");
    let moved = |near: Vec3| {
        let (was, now) = (by_owner(&before, near), by_owner(&after, near));
        assert_eq!(was.len(), now.len());
        was.iter().zip(&now).all(|(a, b)| a.position != b.position && a.alpha > b.alpha)
    };
    assert!(moved(Vec3::NEG_X), "another owner's moved and aged");
    assert!(moved(Vec3::Z), "the world's moved and aged");

    // Thawed, it carries on from where it stood, and comes to the same place as a world never
    // frozen, its clock having lost those ticks.
    world.advance(&ALL_RUN);
    let thawed = frame_of(&mut world);
    let was = by_owner(&after, Vec3::X);
    assert!(by_owner(&thawed, Vec3::X).iter().zip(&was).all(|(a, b)| a.position != b.position));
    assert_ne!(thawed.ribbon_points, after.ribbon_points, "its trail ages again");
}

/// A burst whose one particle flies up at 6 m/s and turns, with no gravity, drag or spin of chance.
fn flying() -> Effect {
    let mut effect = burst(1);
    let emitter = &mut effect.emitters[0];
    emitter.lifetime = (10.0, 10.0);
    emitter.motion = Motion { speed: (6.0, 6.0), spread: 0.0, ..emitter.motion };
    emitter.facing = Facing::Camera { spin: (3.0, 3.0) };
    effect
}

fn drawn(world: &mut VfxWorld, alpha: f32) -> Vec<VfxSprite> {
    let mut frame = VfxFrame::default();
    world.write_frame(&mut frame, alpha);
    frame.sprites
}

/// Review WA-3: a frame reads a particle `alpha` of the way from the step before the latest to the
/// latest, as it reads a transform, so a particle that keeps its own last step never leads a pose.
#[test]
fn a_frame_draws_a_particle_between_its_last_two_steps() {
    let mut world = world_of(vec![flying()], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    // Its first step: it's only just born, so it reads from there at every alpha.
    let born = drawn(&mut world, 1.0)[0];
    assert_eq!(born.position, Vec3::ZERO);
    for alpha in [0.0, 0.25, 0.5] {
        assert_eq!(drawn(&mut world, alpha)[0], born, "alpha {alpha}");
    }
    world.advance(&ALL_RUN);
    let latest = drawn(&mut world, 1.0)[0];
    assert!((latest.position.y - 6.0 * TICK).abs() < 1e-5, "{}", latest.position);
    assert_eq!(drawn(&mut world, 0.0)[0].position, born.position, "where the last frame's step left it");
    assert!((drawn(&mut world, 0.0)[0].rotation - born.rotation).abs() < 1e-5);
    let halfway = drawn(&mut world, 0.5)[0];
    assert!((halfway.position.y - 3.0 * TICK).abs() < 1e-5, "{}", halfway.position);
    assert!((halfway.rotation - (born.rotation + latest.rotation) * 0.5).abs() < 1e-5);
    assert!(latest.alpha < halfway.alpha && halfway.alpha < born.alpha, "it ages along the step too");
    assert_eq!(drawn(&mut world, f32::NAN)[0], drawn(&mut world, 0.0)[0], "not a number: the step before");
    assert_eq!(drawn(&mut world, 2.0)[0], latest, "and never past the latest");
}

/// Each effect reads back along its own owner's step: hitstop holds it to the bit at every alpha, and a
/// slower owner's frame covers less ground. On the step it freezes it's still on its way there.
#[test]
fn a_frame_reads_each_effect_by_its_owners_last_step() {
    let owner = OwnerId(1);
    let mut world = world_of(vec![flying()], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), owner, Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    world.advance(&ALL_RUN);
    let before = drawn(&mut world, 0.75)[0];
    world.advance(&OwnerSteps { world: TICK, owners: &[(owner, 0.0)] });
    let held = drawn(&mut world, 1.0)[0];
    assert!(before.position.y < held.position.y, "the frame before was still on its way");
    for alpha in [0.0, 0.25, 0.5] {
        assert_eq!(drawn(&mut world, alpha)[0], held, "frozen, at alpha {alpha}");
    }
    let slow = TICK * 0.2;
    world.advance(&OwnerSteps { world: TICK, owners: &[(owner, slow)] });
    let (from, to) = (drawn(&mut world, 0.0)[0], drawn(&mut world, 1.0)[0]);
    assert_eq!(from.position, held.position, "it thaws from where it held");
    assert!((to.position.y - held.position.y - 6.0 * slow).abs() < 1e-5, "{}", to.position);
}

/// A frame between two steps ends the strip between two samples, on the spline the next frame draws
/// through them, so the head stays with the blade and doesn't jump when the next step arrives.
#[test]
fn a_frame_ends_a_trail_where_the_frame_is() {
    let budget = VfxBudget { trail_subdivisions: 4, ..VfxBudget::DESKTOP_HIGH };
    let mut world = world_of(vec![trail(48)], budget);
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    let arc = |angle: f32, reach: f32| Vec3::new(angle.cos() * reach, angle.sin() * reach, 0.0);
    for tick in 0..6 {
        world.advance(&ALL_RUN);
        for sub in 0..4 {
            let angle = (tick * 4 + sub) as f32 * 0.05;
            let ago = TICK * (3 - sub) as f32 / 4.0;
            world.sample_trail(blade, arc(angle, 0.3), arc(angle, 1.2), ago);
        }
    }
    let points = |world: &mut VfxWorld, alpha: f32| {
        let mut frame = VfxFrame::default();
        world.write_frame(&mut frame, alpha);
        frame.ribbon_points
    };
    let whole = points(&mut world, 1.0);
    let newest = (5 * 4 + 3) as f32 * 0.05;
    let last = whole[whole.len() - 1];
    assert!(last.ends && last.tip.abs_diff_eq(arc(newest, 1.2), 1e-5), "{}", last.tip);
    // A tick's four samples ago is the step before's last: an exact sample again.
    let before = points(&mut world, 0.0);
    let head = before[before.len() - 1];
    assert!(head.ends && head.tip.abs_diff_eq(arc(newest - 0.2, 1.2), 1e-3), "{}", head.tip);
    assert_eq!(before.len(), whole.len() - 4 * 4, "four stretches of four points fewer");
    assert!(before[..before.len() - 1].iter().all(|point| !point.ends));
    // Halfway: halfway along that step's arc, between two samples, and still on it.
    let halfway = points(&mut world, 0.5);
    let head = halfway[halfway.len() - 1];
    assert!(head.ends && head.tip.abs_diff_eq(arc(newest - 0.1, 1.2), 2e-3), "{}", head.tip);
    assert!(before.len() < halfway.len() && halfway.len() < whole.len());
    // What's behind the head is the same strip it will be at the latest step: shifted in age, not place.
    for (a, b) in before.iter().zip(&whole).take(before.len() - 1) {
        assert!(a.tip.abs_diff_eq(b.tip, 1e-2), "{} {}", a.tip, b.tip);
        assert!(a.alpha >= b.alpha, "younger at the frame before");
    }
    // A trail frozen holds its head at every alpha.
    world.advance(&OwnerSteps { world: 0.0, owners: &[] });
    let held = points(&mut world, 0.25);
    assert_eq!(held, points(&mut world, 1.0));
    assert_eq!(held.len(), whole.len());
}

#[test]
fn a_smaller_step_is_slow_motion() {
    let mut world = world_of(vec![burst(1)], VfxBudget::DESKTOP_LOW);
    let (fast, slow) = (OwnerId(1), OwnerId(2));
    world.spawn(EffectId(0), fast, Placement::at(Vec3::X)).expect("spawned");
    world.spawn(EffectId(0), slow, Placement::at(Vec3::NEG_X)).expect("spawned");
    let steps = OwnerSteps { world: 0.0, owners: &[(fast, TICK), (slow, TICK * 0.25)] };
    world.advance(&steps);
    world.advance(&steps);
    let frame = frame_of(&mut world);
    let climb =
        |x: f32| frame.sprites.iter().find(|sprite| sprite.position.x == x).expect("its particle").position.y;
    assert!((climb(1.0) - 2.0 * TICK).abs() < 1e-5, "{}", climb(1.0));
    assert!((climb(-1.0) - 0.5 * TICK).abs() < 1e-5, "{}", climb(-1.0));
}

#[test]
fn bad_steps_count_as_none() {
    let mut world = world_of(vec![burst(1)], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&OwnerSteps { world: f32::NAN, owners: &[] });
    world.advance(&OwnerSteps { world: -1.0, owners: &[] });
    assert_eq!(world.stats().particles, 0, "it hasn't fired: no time has passed");
    world.advance(&ALL_RUN);
    world.advance(&OwnerSteps { world: f32::INFINITY, owners: &[] });
    let frame = frame_of(&mut world);
    assert_eq!(frame.sprites.len(), 1);
    assert!(frame.sprites[0].position.is_finite());
}

#[test]
fn past_the_particle_cap_it_counts_whats_dropped_and_keeps_drawing() {
    let budget = VfxBudget { max_particles: 10, ..VfxBudget::DESKTOP_LOW };
    let mut world = world_of(vec![burst(8)], budget);
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    let frame = frame_of(&mut world);
    assert_eq!(frame.sprites.len(), 10, "as many as fit still draw");
    assert_eq!(frame.stats.particles, 10);
    assert_eq!(frame.stats.dropped_particles, 6);
}

/// A cap has to bound the work as well as the storage: a long step at the highest rate a document may
/// hold asks each emitter for more than a count can hold, and walked one at a time that's minutes of a
/// frame. It costs what fits.
#[test]
fn what_a_step_cant_fit_is_counted_without_being_walked() {
    let stream = |name: &str| dots(name, Spawn::Rate { per_second: MAX_RATE, duration: 0.0 });
    let flood = Effect {
        name: "flood".to_owned(),
        emitters: vec![stream("a"), stream("b"), stream("c"), stream("d")],
        trail: None,
        mark: false,
    };
    for max_particles in [0, 512] {
        let budget = VfxBudget { max_particles, ..VfxBudget::MOBILE_LOW };
        let mut world = world_of(vec![flood.clone()], budget);
        for _ in 0..2 {
            world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
        }
        let stopwatch = Stopwatch::start();
        world.advance(&OwnerSteps { world: 1e6, owners: &[] });
        let took = stopwatch.elapsed();
        assert!(took < Duration::from_secs(5), "{max_particles} places took {took:?} to fill");
        let stats = world.stats();
        assert_eq!(stats.particles, max_particles);
        let asked = 8 * u64::from(u32::MAX);
        assert_eq!(stats.dropped_particles, asked - u64::from(max_particles), "{max_particles} places");
    }
}

/// Every effect the budget holds, each with as many of the largest burst as an effect may have.
#[test]
fn overlapping_bursts_past_the_cap_count_what_didnt_fit() {
    let full = Effect {
        name: "full".to_owned(),
        emitters: (0..MAX_EMITTERS).map(|_| dots("dots", Spawn::Burst { count: 4096 })).collect(),
        trail: None,
        mark: false,
    };
    let budget = VfxBudget::DESKTOP_HIGH;
    let mut world = world_of(vec![full], budget);
    for _ in 0..budget.max_effects {
        world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    }
    world.advance(&ALL_RUN);
    let dropped =
        u64::from(budget.max_effects) * MAX_EMITTERS as u64 * 4096 - u64::from(budget.max_particles);
    assert_eq!(world.stats().particles, budget.max_particles);
    assert_eq!(world.stats().dropped_particles, dropped);
    // A burst fires once, so what didn't fit isn't asked for again.
    world.advance(&ALL_RUN);
    assert_eq!(world.stats().dropped_particles, dropped);
}

/// A world that runs out of room holds exactly what one with room made first: a drop takes no random
/// numbers, so what's kept doesn't move.
#[test]
fn what_a_full_world_keeps_is_what_it_would_have_made_first() {
    let effect = Effect {
        name: "mixed".to_owned(),
        emitters: vec![
            dots("burst", Spawn::Burst { count: 40 }),
            dots("rate", Spawn::Rate { per_second: 6_000.0, duration: 0.25 }),
            dots("late", Spawn::Burst { count: 7 }),
        ],
        trail: None,
        mark: false,
    };
    let sprites = |max_particles| {
        let budget = VfxBudget { max_particles, ..VfxBudget::DESKTOP_LOW };
        let mut world = world_of(vec![effect.clone()], budget);
        world.spawn(EffectId(0), OwnerId(1), Placement::at(Vec3::Y)).expect("spawned");
        world.advance(&ALL_RUN);
        let dropped = world.stats().dropped_particles;
        (frame_of(&mut world).sprites, dropped)
    };
    let (roomy, none) = sprites(1024);
    assert_eq!(none, 0);
    let (full, dropped) = sprites(64);
    assert_eq!(dropped, roomy.len() as u64 - 64);
    assert_eq!(full[..], roomy[..64]);
}

/// A rate's fractions add up to what it owes, however its particles are kept or dropped.
#[test]
fn a_rate_owes_the_same_with_room_or_without() {
    let stream = Effect {
        name: "stream".to_owned(),
        emitters: vec![dots("dots", Spawn::Rate { per_second: 37.5, duration: 0.5 })],
        trail: None,
        mark: false,
    };
    let owed = |max_particles| {
        let budget = VfxBudget { max_particles, ..VfxBudget::DESKTOP_LOW };
        let mut world = world_of(vec![stream.clone()], budget);
        world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
        for _ in 0..30 {
            world.advance(&ALL_RUN);
        }
        u64::from(world.stats().particles) + world.stats().dropped_particles
    };
    let with_room = owed(1024);
    assert!((18..=19).contains(&with_room), "half a second at 37.5: {with_room}");
    assert_eq!(owed(4), with_room);
    assert_eq!(owed(0), with_room);
}

/// A step too long to count owes as much as a count holds.
#[test]
fn a_step_too_long_to_count_owes_what_a_count_holds() {
    let stream = Effect {
        name: "stream".to_owned(),
        emitters: vec![dots("dots", Spawn::Rate { per_second: 60.0, duration: 0.0 })],
        trail: None,
        mark: false,
    };
    let mut world = world_of(vec![stream], VfxBudget { max_particles: 8, ..VfxBudget::DESKTOP_LOW });
    let handle = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&OwnerSteps { world: f32::MAX, owners: &[] });
    assert_eq!(world.stats().dropped_particles, u64::from(u32::MAX) - 8);
    assert!(world.is_playing(handle));
}

#[test]
fn a_continuous_emitter_recovers_after_large_finite_steps() {
    for step in [1e8, f32::MAX] {
        let mut emitter = dots("dots", Spawn::Rate { per_second: 60.0, duration: 0.0 });
        emitter.lifetime = (0.25, 0.25);
        let stream = Effect { name: "stream".to_owned(), emitters: vec![emitter], trail: None, mark: false };
        let mut world = world_of(vec![stream], VfxBudget { max_particles: 8, ..VfxBudget::DESKTOP_LOW });
        let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
        world.advance(&OwnerSteps { world: step, owners: &[] });
        assert_eq!(world.stats().particles, 8);
        for _ in 0..3 {
            let dropped = world.stats().dropped_particles;
            world.advance(&OwnerSteps { world: 1.0, owners: &[] });
            assert_eq!(world.stats().particles, 8, "after {step}");
            assert_eq!(world.stats().dropped_particles - dropped, 52);
            assert!(world.is_playing(handle));
        }
    }
}

#[test]
fn a_large_step_respects_delayed_finite_emission_and_pauses() {
    for step in [1e8, f32::MAX] {
        let mut emitter = dots("dots", Spawn::Rate { per_second: 60.0, duration: 0.5 });
        emitter.delay = 1.0;
        emitter.lifetime = (0.25, 0.25);
        let stream = Effect { name: "stream".to_owned(), emitters: vec![emitter], trail: None, mark: false };
        let mut world = world_of(vec![stream], VfxBudget { max_particles: 8, ..VfxBudget::DESKTOP_LOW });
        let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
        world.advance(&OwnerSteps { world: 0.0, owners: &[] });
        assert_eq!(world.stats().particles, 0);
        world.advance(&OwnerSteps { world: step, owners: &[] });
        assert_eq!(world.stats().particles, 8);
        assert_eq!(world.stats().dropped_particles, 22);
        let paused = frame_of(&mut world);
        world.advance(&OwnerSteps { world: 0.0, owners: &[] });
        assert_eq!(frame_of(&mut world), paused);
        world.advance(&OwnerSteps { world: 1.0, owners: &[] });
        assert_eq!(world.stats().particles, 0);
        assert_eq!(world.stats().dropped_particles, 22);
        assert!(!world.is_playing(handle));
    }
}

#[test]
fn past_the_effect_cap_a_spawn_is_counted_and_refused() {
    let budget = VfxBudget { max_effects: 2, ..VfxBudget::DESKTOP_LOW };
    let mut world = world_of(vec![burst(1)], budget);
    assert!(world.spawn(EffectId(0), OwnerId(1), Placement::default()).is_some());
    assert!(world.spawn(EffectId(0), OwnerId(1), Placement::default()).is_some());
    assert!(world.spawn(EffectId(0), OwnerId(1), Placement::default()).is_none());
    assert_eq!(world.stats().dropped_effects, 1);
    assert_eq!(world.stats().effects, 2);
    // An unknown effect isn't a dropped one.
    assert!(world.spawn(EffectId(9), OwnerId(1), Placement::default()).is_none());
    assert_eq!(world.stats().dropped_effects, 1);
}

#[test]
fn past_the_trail_cap_an_effect_plays_without_its_trail() {
    let budget = VfxBudget { max_trails: 1, ..VfxBudget::DESKTOP_LOW };
    let mut effect = trail(8);
    effect.emitters.push(dots("dots", Spawn::Burst { count: 1 }));
    let mut world = world_of(vec![effect], budget);
    let first = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    let second = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    assert_eq!(world.stats().dropped_trails, 1);
    world.advance(&ALL_RUN);
    world.sample_trail(first, Vec3::ZERO, Vec3::Y, 0.0);
    world.sample_trail(second, Vec3::ZERO, Vec3::Y, 0.0);
    world.advance(&ALL_RUN);
    world.sample_trail(first, Vec3::X, Vec3::X + Vec3::Y, 0.0);
    world.sample_trail(second, Vec3::X, Vec3::X + Vec3::Y, 0.0);
    let frame = frame_of(&mut world);
    assert_eq!(frame.sprites.len(), 2, "both particles");
    assert_eq!(frame.stats.ribbons, 1, "one trail");
    // Without its trail the second plays out like any effect without one.
    world.stop(first);
    for _ in 0..90 {
        world.advance(&ALL_RUN);
    }
    assert!(!world.is_playing(second));
    assert!(!world.is_playing(first));
}

#[test]
fn past_the_ribbon_cap_a_trail_loses_its_oldest() {
    let budget = VfxBudget { max_ribbon_points: 9, trail_subdivisions: 2, ..VfxBudget::DESKTOP_LOW };
    let mut world = world_of(vec![trail(32)], budget);
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    for step in 0..20 {
        world.advance(&ALL_RUN);
        let x = step as f32 * 0.1;
        world.sample_trail(blade, Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 0.0), 0.0);
    }
    let frame = frame_of(&mut world);
    // 20 samples at 2 subdivisions want 39 points; 9 fit, whole stretches at a time.
    assert_eq!(frame.ribbon_points.len(), 9);
    assert_eq!(frame.stats.dropped_ribbon_points, 30);
    let newest = frame.ribbon_points.last().expect("points");
    assert!(newest.ends);
    assert!((newest.tip.x - 1.9).abs() < 1e-5, "the newest stays: {}", newest.tip.x);
    assert!((frame.ribbon_points[0].tip.x - 1.5).abs() < 1e-3, "{}", frame.ribbon_points[0].tip.x);
}

#[test]
fn a_ribbon_with_no_room_is_counted_and_others_still_draw() {
    let budget = VfxBudget { max_ribbon_points: 3, trail_subdivisions: 1, ..VfxBudget::DESKTOP_LOW };
    let mut world = world_of(vec![trail(32)], budget);
    let first = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    let second = world.spawn(EffectId(0), OwnerId(2), Placement::default()).expect("spawned");
    for step in 0..3 {
        world.advance(&ALL_RUN);
        let x = step as f32 * 0.1;
        world.sample_trail(first, Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 0.0), 0.0);
        world.sample_trail(second, Vec3::new(x, 0.0, 1.0), Vec3::new(x, 1.0, 1.0), 0.0);
    }
    let frame = frame_of(&mut world);
    assert_eq!(frame.ribbon_points.len(), 3);
    assert_eq!(frame.stats.ribbons, 1);
    assert_eq!(frame.stats.dropped_ribbon_points, 3);
}

#[test]
fn one_seed_gives_one_show_and_another_seed_another() {
    let library = EffectLibrary::default_effects();
    let spark = library.find("hit_spark").expect("an effect");
    let play = |seed| {
        let mut world = VfxWorld::new(library.clone(), VfxBudget::DESKTOP_HIGH, seed);
        world.spawn(spark, OwnerId(1), Placement::at(Vec3::Y)).expect("spawned");
        for _ in 0..8 {
            world.advance(&ALL_RUN);
        }
        frame_of(&mut world).sprites
    };
    assert_eq!(play(7), play(7));
    assert_ne!(play(7), play(8));
}

#[test]
fn two_spawns_from_one_seed_differ() {
    let mut world = world_of(vec![burst(1)], VfxBudget::DESKTOP_LOW);
    let mut effect = burst(1);
    effect.emitters[0].motion = Motion { speed: (1.0, 2.0), spread: 180.0, ..Motion::default() };
    world.set_library(EffectLibrary::new(vec![effect]).expect("valid"));
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    world.advance(&ALL_RUN);
    let frame = frame_of(&mut world);
    assert_ne!(frame.sprites[0].position, frame.sprites[1].position);
}

/// A cue mark (M4 A5) draws its numbers from a stream of its own, so the sparks spawned after it land as
/// they would without it, while a plain effect in its place moves them.
#[test]
fn a_mark_moves_no_other_effects_numbers() {
    let mut spark = burst(4);
    spark.emitters[0].motion = Motion { speed: (1.0, 2.0), spread: 180.0, ..Motion::default() };
    let mark = Effect { name: "mark".to_owned(), mark: true, ..spark.clone() };
    let plain = Effect { name: "plain".to_owned(), ..spark.clone() };
    let play = |first: Option<EffectId>| {
        let library = EffectLibrary::new(vec![spark.clone(), mark.clone(), plain.clone()]).expect("valid");
        let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_LOW, 7);
        if let Some(first) = first {
            world.spawn(first, OwnerId(2), Placement::at(Vec3::X * 9.0)).expect("spawned");
        }
        world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
        world.advance(&ALL_RUN);
        let frame = frame_of(&mut world);
        frame.sprites.into_iter().filter(|sprite| sprite.position.x < 5.0).collect::<Vec<_>>()
    };
    let alone = play(None);
    assert_eq!(alone.len(), 4);
    assert_eq!(play(Some(EffectId(1))), alone);
    assert_ne!(play(Some(EffectId(2))), alone);
}

#[test]
fn an_upright_quad_never_turns() {
    let mut effect = burst(3);
    effect.emitters[0].facing = Facing::Upright;
    effect.emitters[0].shape = Shape::Chevron;
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    for _ in 0..10 {
        world.advance(&ALL_RUN);
    }
    let frame = frame_of(&mut world);
    assert_eq!(frame.sprites.len(), 3);
    for sprite in frame.sprites {
        assert_eq!((sprite.axis, sprite.rotation, sprite.shape), (Vec3::ZERO, 0.0, VfxShape::Chevron));
    }
}

#[test]
fn particles_follow_their_curves_and_die_at_their_lifetime() {
    let mut effect = burst(1);
    effect.emitters[0].size = Curve(vec![(0.0, 0.1), (1.0, 0.3)]);
    effect.emitters[0].color = Curve(vec![(0.0, [1.0, 0.0, 0.0]), (1.0, [0.0, 0.0, 1.0])]);
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    for _ in 0..30 {
        world.advance(&ALL_RUN);
    }
    let frame = frame_of(&mut world);
    let sprite = frame.sprites[0];
    assert!((sprite.half_width - 0.2).abs() < 1e-4, "{}", sprite.half_width);
    assert!((sprite.alpha - 0.5).abs() < 1e-4, "{}", sprite.alpha);
    assert!((sprite.color.x - 0.5).abs() < 1e-4, "{}", sprite.color);
    assert_eq!(sprite.shape, VfxShape::SoftDot);
    // One more than a second's worth: 60 steps of 1/60 add up to a hair under 1 in f32.
    for _ in 0..31 {
        world.advance(&ALL_RUN);
    }
    assert_eq!(world.stats().particles, 0);
    assert!(!world.is_playing(handle), "a burst ends with its last particle");
}

#[test]
fn velocity_facing_lies_along_its_motion_and_stretches_with_speed() {
    let mut effect = burst(1);
    effect.emitters[0].motion =
        Motion { speed: (10.0, 10.0), direction: [1.0, 0.0, 0.0], ..Motion::default() };
    effect.emitters[0].facing = Facing::Velocity { stretch: 0.01 };
    effect.emitters[0].size = Curve(vec![(0.0, 0.1)]);
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    let sprite = frame_of(&mut world).sprites[0];
    assert_eq!(sprite.axis.normalize(), Vec3::X);
    assert!((sprite.half_length - 0.2).abs() < 1e-4, "{}", sprite.half_length);
    assert_eq!(sprite.half_width, 0.1);
}

#[test]
fn a_placements_rotation_turns_its_emitters() {
    let mut effect = burst(1);
    effect.emitters[0].motion = Motion { speed: (1.0, 1.0), direction: [0.0, 1.0, 0.0], ..Motion::default() };
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let placement =
        Placement { position: Vec3::ZERO, rotation: Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2) };
    world.spawn(EffectId(0), OwnerId(1), placement).expect("spawned");
    world.advance(&ALL_RUN);
    world.advance(&ALL_RUN);
    let position = frame_of(&mut world).sprites[0].position;
    assert!(position.x > 0.0 && position.y.abs() < 1e-5, "+Y turned to +X: {position}");
}

#[test]
fn a_rate_emits_at_its_rate_and_stops_when_stopped() {
    let effect = Effect {
        name: "stream".to_owned(),
        emitters: vec![dots("dots", Spawn::Rate { per_second: 60.0, duration: 0.0 })],
        trail: None,
        mark: false,
    };
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    for _ in 0..30 {
        world.advance(&ALL_RUN);
    }
    let particles = world.stats().particles;
    assert!((29..=31).contains(&particles), "{particles}");
    world.stop(handle);
    world.advance(&ALL_RUN);
    assert_eq!(world.stats().particles, particles, "no new ones");
    for _ in 0..70 {
        world.advance(&ALL_RUN);
    }
    assert!(!world.is_playing(handle), "gone with its last particle");
}

/// What an effect says of itself is what the world does with it, left alone.
#[test]
fn an_effect_is_endless_exactly_when_it_never_ends_by_itself() {
    let of = |spawns: &[Spawn]| Effect {
        name: "effect".to_owned(),
        emitters: spawns.iter().map(|&spawn| dots("dots", spawn)).collect(),
        trail: None,
        mark: false,
    };
    let (once, brief) = (Spawn::Burst { count: 2 }, Spawn::Rate { per_second: 60.0, duration: 0.5 });
    let endless = Spawn::Rate { per_second: 60.0, duration: 0.0 };
    let mut with_trail = burst(2);
    with_trail.trail = trail(8).trail;
    let effects = [
        (of(&[once]), false),
        (of(&[brief]), false),
        (of(&[once, brief]), false),
        (of(&[endless]), true),
        (of(&[once, endless]), true),
        (of(&[endless, brief]), true),
        (trail(8), true),
        (with_trail, true),
    ];
    for (effect, endless) in effects {
        assert_eq!(effect.is_endless(), endless, "{effect:?}");
        let mut world = world_of(vec![effect.clone()], VfxBudget::DESKTOP_LOW);
        let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).expect("spawned");
        for _ in 0..600 {
            world.advance(&ALL_RUN);
        }
        assert_eq!(world.is_playing(handle), endless, "{effect:?}");
        world.stop(handle);
        for _ in 0..600 {
            world.advance(&ALL_RUN);
        }
        assert!(!world.is_playing(handle), "stopping ends any of them: {effect:?}");
    }
}

#[test]
fn a_delayed_burst_keeps_its_absolute_clock_boundary_through_pauses() {
    for pause in [false, true] {
        let mut effect = burst(1);
        effect.emitters[0].delay = 0.1;
        let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
        world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
        world.advance(&OwnerSteps { world: 0.0, owners: &[] });
        assert_eq!(world.stats().particles, 0, "a zero step cannot fire it early");
        for step in 1..=6 {
            if pause {
                let held = frame_of(&mut world);
                world.advance(&OwnerSteps { world: 0.0, owners: &[] });
                assert_eq!(frame_of(&mut world), held);
            }
            world.advance(&ALL_RUN);
            assert_eq!(world.stats().particles, u32::from(step == 6), "positive step {step}, pause={pause}");
        }
        world.advance(&OwnerSteps { world: 0.0, owners: &[] });
        assert_eq!(world.stats().particles, 1, "holding the boundary never fires twice");
    }
}

#[test]
fn a_delay_waits_on_the_owners_clock() {
    let mut effect = burst(1);
    effect.emitters[0].delay = 0.5;
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let owner = OwnerId(1);
    world.spawn(EffectId(0), owner, Placement::default()).expect("spawned");
    // The world runs a second, but the owner is frozen.
    for _ in 0..60 {
        world.advance(&OwnerSteps { world: TICK, owners: &[(owner, 0.0)] });
    }
    assert_eq!(world.stats().particles, 0);
    for _ in 0..31 {
        world.advance(&ALL_RUN);
    }
    assert_eq!(world.stats().particles, 1);
}

#[test]
fn a_rates_duration_bounds_how_many_it_emits() {
    let effect = Effect {
        name: "stream".to_owned(),
        emitters: vec![EmitterDesc {
            lifetime: (10.0, 10.0),
            ..dots("dots", Spawn::Rate { per_second: 60.0, duration: 0.5 })
        }],
        trail: None,
        mark: false,
    };
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    for _ in 0..120 {
        world.advance(&ALL_RUN);
    }
    let particles = world.stats().particles;
    assert!((29..=31).contains(&particles), "half a second at 60: {particles}");
}

/// Sub-tick samples, smoothed: the strip runs through every sample, with points between.
#[test]
fn a_trail_runs_through_its_samples_smoothly() {
    let budget = VfxBudget { trail_subdivisions: 4, ..VfxBudget::DESKTOP_HIGH };
    let mut world = world_of(vec![trail(48)], budget);
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    // A quarter circle, a sample each sub-tick.
    let arc = |angle: f32, reach: f32| Vec3::new(angle.cos() * reach, angle.sin() * reach, 0.0);
    let sub_ticks = 4;
    let sample_angle = |tick: u32, sub: u32| (tick * sub_ticks + sub) as f32 * 0.05;
    for tick in 0..6 {
        world.advance(&ALL_RUN);
        for sub in 0..sub_ticks {
            let angle = sample_angle(tick, sub);
            let ago = TICK * (sub_ticks - 1 - sub) as f32 / sub_ticks as f32;
            world.sample_trail(blade, arc(angle, 0.3), arc(angle, 1.2), ago);
        }
    }
    let frame = frame_of(&mut world);
    let points = &frame.ribbon_points;
    let samples = 6 * sub_ticks as usize;
    assert_eq!(points.len(), (samples - 1) * 4 + 1);
    assert!(points[..points.len() - 1].iter().all(|point| !point.ends));
    assert!(points[points.len() - 1].ends);
    // Every fourth point is a sample, and everything between lies on the arc, close to its radius.
    for (index, point) in points.iter().enumerate() {
        let reach = point.tip.length();
        if index % 4 == 0 {
            assert!((reach - 1.2).abs() < 1e-3, "sample {index}: {reach}");
        }
        assert!((reach - 1.2).abs() < 2e-3, "smooth at {index}: {reach}");
    }
    // Younger toward the end: opacity rises along the strip, as the curve falls with age.
    assert!(points[0].alpha < points[points.len() - 1].alpha);
}

#[test]
fn a_trail_narrows_toward_its_anchor_with_age() {
    let mut effect = trail(32);
    let desc = effect.trail.as_mut().expect("a trail");
    desc.width = Curve(vec![(0.0, 1.0), (1.0, 0.0)]);
    desc.anchor = 1.0;
    let mut world = world_of(vec![effect], VfxBudget { trail_subdivisions: 1, ..VfxBudget::DESKTOP_LOW });
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    world.sample_trail(blade, Vec3::ZERO, Vec3::Y, 0.0);
    for _ in 0..15 {
        world.advance(&ALL_RUN);
    }
    world.sample_trail(blade, Vec3::X, Vec3::X + Vec3::Y, 0.0);
    let frame = frame_of(&mut world);
    let [old, new] = [frame.ribbon_points[0], frame.ribbon_points[1]];
    // A quarter second old of its half second: half its width, about the tip.
    assert!((old.tip - Vec3::Y).length() < 1e-5, "{}", old.tip);
    assert!((old.base - Vec3::Y * 0.5).length() < 1e-3, "{}", old.base);
    assert_eq!(new.base, Vec3::X);
    assert_eq!(new.tip, Vec3::X + Vec3::Y);
}

#[test]
fn a_full_trail_overwrites_its_oldest_samples() {
    let mut world = world_of(vec![trail(4)], VfxBudget { trail_subdivisions: 1, ..VfxBudget::DESKTOP_LOW });
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    for step in 0..7 {
        world.advance(&ALL_RUN);
        let x = step as f32;
        world.sample_trail(blade, Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 0.0), 0.0);
    }
    let frame = frame_of(&mut world);
    let xs: Vec<f32> = frame.ribbon_points.iter().map(|point| point.tip.x).collect();
    assert_eq!(xs, [3.0, 4.0, 5.0, 6.0]);
}

#[test]
fn samples_retire_with_their_age_and_the_trail_ends_once_stopped() {
    let mut world = world_of(vec![trail(32)], VfxBudget { trail_subdivisions: 1, ..VfxBudget::DESKTOP_LOW });
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    for step in 0..4 {
        world.advance(&ALL_RUN);
        let x = step as f32;
        world.sample_trail(blade, Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 0.0), 0.0);
    }
    assert_eq!(frame_of(&mut world).ribbon_points.len(), 4);
    world.stop(blade);
    world.sample_trail(blade, Vec3::new(9.0, 0.0, 0.0), Vec3::new(9.0, 1.0, 0.0), 0.0);
    assert_eq!(frame_of(&mut world).ribbon_points.len(), 4, "a stopped trail takes no more");
    assert!(world.is_playing(blade));
    for _ in 0..28 {
        world.advance(&ALL_RUN);
    }
    let fading = frame_of(&mut world);
    assert!(fading.ribbon_points.len() < 4, "its oldest have gone");
    assert!(fading.ribbon_points.len() >= 2 || fading.ribbon_points.is_empty());
    for _ in 0..10 {
        world.advance(&ALL_RUN);
    }
    assert!(!world.is_playing(blade));
    assert!(frame_of(&mut world).ribbon_points.is_empty());
    // Its ring is free again.
    let next = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    assert_eq!(world.stats().dropped_trails, 0);
    assert!(world.is_playing(next));
}

#[test]
fn a_trails_tail_fades_to_nothing_rather_than_cutting_off() {
    let mut world = world_of(vec![trail(32)], VfxBudget { trail_subdivisions: 1, ..VfxBudget::DESKTOP_LOW });
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    for step in 0..60 {
        world.advance(&ALL_RUN);
        let x = step as f32 * 0.05;
        world.sample_trail(blade, Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 0.0), 0.0);
    }
    let frame = frame_of(&mut world);
    assert_eq!(frame.ribbon_points[0].alpha, 0.0, "the sample it fades into is at the end of its life");
    assert!(frame.ribbon_points[1].alpha > 0.0);
}

#[test]
fn bad_sockets_are_ignored() {
    let mut world = world_of(vec![trail(8)], VfxBudget::DESKTOP_LOW);
    let blade = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    world.sample_trail(blade, Vec3::NAN, Vec3::Y, 0.0);
    world.sample_trail(blade, Vec3::ZERO, Vec3::INFINITY, 0.0);
    world.sample_trail(blade, Vec3::ZERO, Vec3::Y, f32::NAN);
    world.sample_trail(blade, Vec3::ZERO, Vec3::Y, 0.0);
    world.advance(&ALL_RUN);
    // Older than the newest it has: out of order.
    world.sample_trail(blade, Vec3::Z, Vec3::Y, 1.0);
    assert!(frame_of(&mut world).ribbon_points.is_empty(), "one sample draws no strip");
}

#[test]
fn starting_over_matches_a_fresh_world_and_invalidates_handles() {
    let budget = VfxBudget { max_effects: 3, max_particles: 8, ..VfxBudget::DESKTOP_LOW };
    let mut world = world_of(vec![burst(12)], budget);
    let mut fresh = world.clone();
    let old = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
    world.advance(&ALL_RUN);
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default());
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default());
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default());
    assert!(world.stats().dropped_particles > 0 && world.stats().dropped_effects > 0);
    world.start_over();
    assert!(!world.is_playing(old));
    assert_eq!(world.stats(), fresh.stats());
    for index in 0..3 {
        let placement = Placement::at(Vec3::X * index as f32);
        world.spawn(EffectId(0), OwnerId::WORLD, placement);
        fresh.spawn(EffectId(0), OwnerId::WORLD, placement);
        world.stop(old);
        world.advance(&ALL_RUN);
        fresh.advance(&ALL_RUN);
        assert_eq!(frame_of(&mut world).sprites, frame_of(&mut fresh).sprites);
        assert_eq!(world.stats(), fresh.stats());
    }
}

#[test]
fn a_stale_handle_does_nothing() {
    let mut world = world_of(vec![burst(1)], VfxBudget { max_effects: 1, ..VfxBudget::DESKTOP_LOW });
    let handle = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("spawned");
    world.clear();
    assert!(!world.is_playing(handle));
    let next = world.spawn(EffectId(0), OwnerId(1), Placement::default()).expect("the slot is free");
    world.stop(handle);
    world.set_placement(handle, Placement::at(Vec3::X));
    assert!(world.is_playing(next));
    assert_ne!(handle, next);
}

#[test]
fn clearing_frees_everything() {
    let mut world = world_of(vec![burst(4), trail(8)], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId(1), Placement::default());
    world.spawn(EffectId(1), OwnerId(1), Placement::default());
    world.advance(&ALL_RUN);
    world.clear();
    let stats = world.stats();
    assert_eq!((stats.effects, stats.particles), (0, 0));
    assert!(frame_of(&mut world).is_empty());
}

/// A duel's worst moment fits every tier's row with nothing dropped: two sword trails, 200 sparks
/// and 50 dust particles.
#[test]
fn a_duels_worst_moment_fits_every_tier() {
    for budget in
        [VfxBudget::DESKTOP_HIGH, VfxBudget::DESKTOP_LOW, VfxBudget::MOBILE_HIGH, VfxBudget::MOBILE_LOW]
    {
        let library = EffectLibrary::default_effects();
        let (spark, sword, dust) = (
            library.find("hit_spark").expect("an effect"),
            library.find("sword_trail").expect("an effect"),
            library.find("dust_puff").expect("an effect"),
        );
        let mut world = VfxWorld::new(library, budget, 7);
        let blades = [
            world.spawn(sword, OwnerId(1), Placement::default()).expect("spawned"),
            world.spawn(sword, OwnerId(2), Placement::default()).expect("spawned"),
        ];
        // Eight hits of 26 particles (a flash, a ring, 18 sparks and 6 motes), 208 in all, and five
        // puffs of 10 dust motes.
        for _ in 0..8 {
            world.spawn(spark, OwnerId::WORLD, Placement::default()).expect("spawned");
        }
        for _ in 0..5 {
            world.spawn(dust, OwnerId::WORLD, Placement::default()).expect("spawned");
        }
        for tick in 0..12 {
            world.advance(&ALL_RUN);
            for (index, blade) in blades.iter().enumerate() {
                let x = tick as f32 * 0.1 + index as f32;
                world.sample_trail(*blade, Vec3::new(x, 0.0, 0.0), Vec3::new(x, 1.0, 0.0), 0.0);
            }
        }
        let frame = frame_of(&mut world);
        assert_eq!(frame.stats.dropped_particles, 0, "{budget:?}");
        assert_eq!(frame.stats.dropped_ribbon_points, 0, "{budget:?}");
        assert_eq!(frame.stats.ribbons, 2);
    }
}

/// A trail alone leaves the random numbers of what's spawned after it as they'd be without it, so a sword
/// trail never moves a hit's sparks.
#[test]
fn a_trail_takes_no_random_numbers_from_what_follows_it() {
    let sparks_after = |trail_first: bool| {
        let library = EffectLibrary::default_effects();
        let (trail, spark) =
            (library.find("blade_trail").expect("shipped"), library.find("hit_spark").expect("shipped"));
        let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_HIGH, 7);
        if trail_first {
            assert!(world.spawn_trail(trail, OwnerId::WORLD, Placement::default()).is_some());
        }
        assert!(world.spawn(spark, OwnerId::WORLD, Placement::default()).is_some());
        world.advance(&OwnerSteps { world: 1.0 / 60.0, owners: &[] });
        let mut frame = VfxFrame::default();
        world.write_frame(&mut frame, 1.0);
        assert!(!frame.sprites.is_empty());
        frame.sprites
    };
    assert_eq!(sparks_after(true), sparks_after(false));
}

/// An effect with emitters takes numbers of its own, however it's spawned.
#[test]
fn spawning_a_trail_with_emitters_is_spawning_it() {
    let burst_after = |by_trail: bool| {
        let library = EffectLibrary::default_effects();
        let spark = library.find("hit_spark").expect("shipped");
        let mut world = VfxWorld::new(library, VfxBudget::DESKTOP_HIGH, 7);
        let spawned = if by_trail {
            world.spawn_trail(spark, OwnerId::WORLD, Placement::default())
        } else {
            world.spawn(spark, OwnerId::WORLD, Placement::default())
        };
        assert!(spawned.is_some());
        world.advance(&OwnerSteps { world: 1.0 / 60.0, owners: &[] });
        let mut frame = VfxFrame::default();
        world.write_frame(&mut frame, 1.0);
        frame.sprites
    };
    assert_eq!(burst_after(true), burst_after(false));
}
