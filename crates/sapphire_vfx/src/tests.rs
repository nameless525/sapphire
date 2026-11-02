//! The simulation's contracts: owners' clocks, caps, seeds, and what a trail resolves to.

use glam::{Quat, Vec3};
use sapphire_look::{VfxFrame, VfxShape, VfxSprite};

use crate::curve::Curve;
use crate::{
    Effect, EffectId, EffectLibrary, EmitterDesc, Facing, Light, Motion, OwnerId, OwnerSteps, Placement, Shape,
    Source, Spawn, TrailCore, TrailDesc, VfxBudget, VfxWorld,
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
    Effect { name: "burst".to_owned(), emitters: vec![dots("dots", Spawn::Burst { count })], trail: None }
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
            additive: 0.8,
            soft: 0.02,
        }),
    }
}

fn world_of(effects: Vec<Effect>, budget: VfxBudget) -> VfxWorld {
    VfxWorld::new(EffectLibrary::new(effects).expect("valid"), budget, 7)
}

fn frame_of(world: &mut VfxWorld) -> VfxFrame {
    let mut frame = VfxFrame::default();
    world.write_frame(&mut frame);
    frame
}

/// Steps every owner by a tick.
const ALL_RUN: OwnerSteps<'static> = OwnerSteps { world: TICK, owners: &[] };

/// The frozen-owner test: hitstop freezes one owner's effects while another's, and the world's,
/// keep running.
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
    let climb = |x: f32| {
        frame.sprites.iter().find(|sprite| sprite.position.x == x).expect("its particle").position.y
    };
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
    effect.emitters[0].motion = Motion { speed: (10.0, 10.0), direction: [1.0, 0.0, 0.0], ..Motion::default() };
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
    let placement = Placement {
        position: Vec3::ZERO,
        rotation: Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),
    };
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
    for budget in [
        VfxBudget::DESKTOP_HIGH,
        VfxBudget::DESKTOP_LOW,
        VfxBudget::MOBILE_HIGH,
        VfxBudget::MOBILE_LOW,
    ] {
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
