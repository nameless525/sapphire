//! M6 N2's contracts: each force does what its document says, the ground holds particles up, the new spawns
//! fire what they owe, and none of it moves a particle of an effect without them.

use super::*;
use crate::{Force, Ground, WindSample};

/// One particle a burst, standing still at birth, living long, with no gravity or drag but what's asked.
fn mote(forces: Vec<Force>, collide: Option<Ground>) -> Effect {
    let mut emitter = dots("mote", Spawn::Burst { count: 1 });
    emitter.lifetime = (10.0, 10.0);
    emitter.motion = Motion { speed: (0.0, 0.0), ..Motion::default() };
    emitter.forces = forces;
    emitter.collide = collide;
    Effect { emitters: vec![emitter], ..burst(1) }
}

/// Where the particles are drawn after `ticks` more steps of everyone's clock.
fn after(world: &mut VfxWorld, ticks: u32) -> Vec<Vec3> {
    for _ in 0..ticks {
        world.advance(&ALL_RUN);
    }
    drawn(world, 1.0).iter().map(|sprite| sprite.position).collect()
}

fn only(positions: &[Vec3]) -> Vec3 {
    assert_eq!(positions.len(), 1, "{positions:?}");
    positions[0]
}

#[test]
fn wind_carries_a_particle_toward_its_speed_and_calm_carries_none() {
    let mut world = world_of(vec![mote(vec![Force::Wind { response: 4.0 }], None)], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).expect("spawned");
    assert_eq!(only(&after(&mut world, 30)), Vec3::ZERO, "calm until it's said");
    world.set_wind(WindSample { velocity: Vec3::new(2.0, 0.0, 0.0) });
    let first = only(&after(&mut world, 60));
    let second = only(&after(&mut world, 60));
    // Within a second it moves nearly as fast as the air: about 2 m in the next second.
    assert!(first.x > 1.0 && first.y == 0.0, "{first}");
    assert!((second.x - first.x - 2.0).abs() < 0.05, "{first} then {second}");
    world.set_wind(WindSample { velocity: Vec3::new(f32::NAN, 0.0, 0.0) });
    assert!(only(&after(&mut world, 1)).is_finite(), "a wind that isn't finite is calm");
}

#[test]
fn a_vortex_turns_particles_round_its_axis() {
    let mut effect = mote(vec![Force::Vortex { axis: [0.0, 1.0, 0.0], strength: 6.0, pull: 2.0 }], None);
    effect.emitters[0].source = Source::Disc(1.0);
    effect.emitters[0].spawn = Spawn::Burst { count: 32 };
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).expect("spawned");
    let born = after(&mut world, 1);
    let later = after(&mut world, 10);
    let mut turned = 0;
    for (from, to) in born.iter().zip(&later) {
        assert_eq!(to.y, 0.0, "it stays level");
        if from.length() > 0.3 {
            // The right-hand turn about +Y: from +X toward -Z.
            assert!(from.cross(*to).y > 0.0, "{from} to {to}");
            turned += 1;
        }
    }
    assert!(turned > 16, "{turned}");
}

#[test]
fn an_attractor_gathers_particles_and_ends_them_at_its_radius() {
    let attract = Force::Attract { point: [0.0, 1.0, 0.0], strength: 20.0, kill_radius: 0.25 };
    let mut effect = mote(vec![attract], None);
    effect.emitters[0].source = Source::Sphere(1.0);
    effect.emitters[0].spawn = Spawn::Burst { count: 64 };
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    // Placed away from the world's middle: the point it attracts to is in the effect's space.
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::at(Vec3::new(3.0, 0.0, 0.0))).unwrap();
    world.advance(&ALL_RUN);
    assert_eq!(world.stats().particles, 64);
    let nearer = after(&mut world, 10);
    assert!(nearer.iter().all(|at| at.distance(Vec3::new(3.0, 1.0, 0.0)) < 2.0), "{nearer:?}");
    for _ in 0..120 {
        world.advance(&ALL_RUN);
    }
    // Every one has been drawn in and ended long before its lifetime, and with them the effect.
    assert_eq!(world.stats().particles, 0);
    assert!(!world.is_playing(handle));
}

#[test]
fn a_speed_limit_holds_after_the_forces_before_it() {
    let forces = vec![
        Force::Attract { point: [0.0, -100.0, 0.0], strength: 50.0, kill_radius: 0.0 },
        Force::LimitSpeed { max: 1.5 },
    ];
    let mut world = world_of(vec![mote(forces, None)], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).expect("spawned");
    let before = only(&after(&mut world, 60));
    let then = only(&after(&mut world, 60));
    assert!((before.distance(then) - 1.5).abs() < 1e-3, "{before} then {then}");
}

#[test]
fn noise_swirls_particles_and_holds_still_in_hitstop() {
    let noise = Force::Noise { strength: 8.0, frequency: 1.0, octaves: 2, drift: 0.5 };
    let mut effect = mote(vec![noise], None);
    effect.emitters[0].source = Source::Sphere(0.5);
    effect.emitters[0].spawn = Spawn::Burst { count: 8 };
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let (frozen, free) = (OwnerId(1), OwnerId(2));
    world.spawn(EffectId(0), frozen, Placement::default()).expect("spawned");
    world.spawn(EffectId(0), free, Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    let start = drawn(&mut world, 1.0);
    let hold = [(frozen, 0.0)];
    for _ in 0..30 {
        world.advance(&OwnerSteps { world: TICK, owners: &hold });
    }
    let held = drawn(&mut world, 1.0);
    assert_eq!(start.len(), 16);
    // The first effect's eight, its owner frozen, haven't moved; the second's, born still, have swirled away.
    assert_eq!(start[..8], held[..8], "the frozen owner's particles held");
    for (from, to) in start[8..].iter().zip(&held[8..]) {
        assert!(from.position.distance(to.position) > 0.01, "{} to {}", from.position, to.position);
    }
}

/// Under `noise_on_alternate_steps` a noise skips a step and takes it the next, over both steps' time.
#[test]
fn the_alternate_step_switch_turns_noise_on_twos() {
    let noise = Force::Noise { strength: 8.0, frequency: 1.0, octaves: 1, drift: 0.0 };
    // Where the particle is at its birth and the two steps after it.
    let place = |budget| {
        let mut world = world_of(vec![mote(vec![noise], None)], budget);
        world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).expect("spawned");
        [after(&mut world, 1), after(&mut world, 1), after(&mut world, 1)].map(|positions| only(&positions))
    };
    let [born, first, second] = place(VfxBudget { noise_on_alternate_steps: true, ..VfxBudget::DESKTOP_LOW });
    assert_eq!(born, Vec3::ZERO);
    assert_ne!(first, Vec3::ZERO, "the step after its birth swirls, by both steps' time");
    assert!(((second - first) - (first - born)).length() < 1e-6, "and the next coasts at what it took");
    let [born, first, second] = place(VfxBudget::DESKTOP_LOW);
    assert!(((second - first) - (first - born)).length() > 1e-5, "off, every step swirls");
}

#[test]
fn the_ground_bounces_particles_and_never_draws_them_under_it() {
    let mut effect = mote(Vec::new(), Some(Ground { bounce: 0.6, friction: 0.0, dies: false }));
    effect.emitters[0].motion = Motion { gravity: 9.8, ..Motion::default() };
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::at(Vec3::new(0.0, 2.0, 0.0))).unwrap();
    // Its owner stands on the floor, not where its effect was placed; a height that isn't finite is ignored.
    world.set_ground(handle, 0.0);
    world.set_ground(handle, f32::NAN);
    let (mut lowest, mut landed, mut rose_after_landing) = (f32::MAX, false, false);
    let mut last = 2.0_f32;
    for _ in 0..240 {
        world.advance(&ALL_RUN);
        for alpha in [0.0, 0.5] {
            lowest = lowest.min(drawn(&mut world, alpha)[0].position.y);
        }
        let y = drawn(&mut world, 1.0)[0].position.y;
        lowest = lowest.min(y);
        landed |= y < 0.1;
        rose_after_landing |= landed && y > last + 1e-4;
        last = y;
    }
    assert!(lowest >= 0.0, "drawn under the ground at {lowest}");
    assert!(landed && rose_after_landing, "it bounced");
}

#[test]
fn a_ground_that_kills_ends_particles_as_they_meet_it_and_friction_stops_a_slide() {
    let mut deadly = mote(Vec::new(), Some(Ground { dies: true, ..Ground::default() }));
    deadly.emitters[0].motion =
        Motion { speed: (1.0, 1.0), direction: [0.0, -1.0, 0.0], ..Motion::default() };
    deadly.emitters[0].spawn = Spawn::Burst { count: 5 };
    let mut sliding = mote(Vec::new(), Some(Ground { bounce: 0.0, friction: 0.5, dies: false }));
    sliding.name = "slide".to_owned();
    sliding.emitters[0].motion =
        Motion { speed: (3.0, 3.0), direction: [1.0, -1.0, 0.0], gravity: 9.8, ..Motion::default() };
    let mut world = world_of(vec![deadly, sliding], VfxBudget::DESKTOP_LOW);
    let rain = world.spawn(EffectId(0), OwnerId::WORLD, Placement::at(Vec3::new(0.0, 0.1, 0.0))).unwrap();
    world.set_ground(rain, 0.0);
    world.spawn(EffectId(1), OwnerId::WORLD, Placement::default()).expect("spawned");
    world.advance(&ALL_RUN);
    assert_eq!(world.stats().particles, 6);
    for _ in 0..10 {
        world.advance(&ALL_RUN);
    }
    assert_eq!(world.stats().particles, 1, "the five met the ground and ended there");
    assert!(!world.is_playing(rain));
    let resting = only(&after(&mut world, 120));
    assert_eq!(resting, only(&after(&mut world, 60)), "friction stopped it");
    assert_eq!(resting.y, 0.0);
}

/// Ground friction is a rate over its owner's time: a slide on a clock at half speed, twice the steps of half
/// the length, stops as far along as one at full speed, where a share taken each step would stop it in half
/// the distance.
#[test]
fn friction_slows_a_slide_by_its_owners_time_not_its_steps() {
    let slid = |step: f32, steps: u32| {
        let mut sliding = mote(Vec::new(), Some(Ground { bounce: 0.0, friction: 0.05, dies: false }));
        sliding.emitters[0].motion =
            Motion { speed: (3.0, 3.0), direction: [1.0, 0.0, 0.0], gravity: 9.8, ..Motion::default() };
        let mut world = world_of(vec![sliding], VfxBudget::DESKTOP_LOW);
        world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).expect("spawned");
        for _ in 0..steps {
            world.advance(&OwnerSteps { world: step, owners: &[] });
        }
        only(&drawn(&mut world, 1.0).iter().map(|sprite| sprite.position).collect::<Vec<_>>())
    };
    let (full, half) = (slid(TICK, 180), slid(TICK * 0.5, 360));
    assert!(full.x > 0.5 && full.y == 0.0 && half.y == 0.0, "{full} and {half}");
    assert!((half.x - full.x).abs() < 0.03 * full.x, "{full} and {half}");
}

#[test]
fn a_repeat_fires_its_bursts_on_time_and_then_ends() {
    const STEP: f32 = 0.0625;
    let mut effect = burst(1);
    effect.emitters[0].spawn = Spawn::Repeat { count: 3, interval: 0.25, times: 4 };
    effect.emitters[0].lifetime = (0.1, 0.1);
    assert!(!effect.is_endless());
    let mut world = world_of(vec![effect.clone()], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
    let mut births = Vec::new();
    for tick in 0..20 {
        let before = world.stats().particles;
        world.advance(&OwnerSteps { world: STEP, owners: &[] });
        if world.stats().particles > before {
            births.push((tick, world.stats().particles - before));
        }
    }
    // The steps ending at 0.0625, 0.25, 0.5 and 0.75 s: a burst at 0 and at each quarter second.
    assert_eq!(births, [(0, 3), (3, 3), (7, 3), (11, 3)]);
    assert!(!world.is_playing(handle), "four bursts, then it's done");
    // A step longer than its interval owes each burst it spans; with no count of times it never ends.
    effect.emitters[0].spawn = Spawn::Repeat { count: 2, interval: 0.1, times: 0 };
    effect.emitters[0].lifetime = (5.0, 5.0);
    assert!(effect.is_endless());
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
    world.advance(&OwnerSteps { world: 0.35, owners: &[] });
    assert_eq!(world.stats().particles, 8, "at 0, 0.1, 0.2 and 0.3");
}

#[test]
fn a_rate_a_meter_fires_along_the_way_it_moves_and_none_standing() {
    let mut effect = burst(1);
    effect.emitters[0].spawn = Spawn::PerMeter { per_meter: 4.0 };
    effect.emitters[0].motion = Motion::default();
    effect.emitters[0].lifetime = (10.0, 10.0);
    assert!(effect.is_endless());
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
    assert!(after(&mut world, 30).is_empty(), "standing, it fires nothing");
    world.set_placement(handle, Placement::at(Vec3::new(1.0, 0.0, 0.0)));
    let mut along: Vec<f32> = after(&mut world, 1).iter().map(|position| position.x).collect();
    along.sort_by(f32::total_cmp);
    assert_eq!(along, [0.25, 0.5, 0.75, 1.0], "spread evenly along the meter");
    world.set_placement(handle, Placement::at(Vec3::new(1.0, 0.0, 0.3)));
    assert_eq!(after(&mut world, 1).len(), 5, "a fraction owed carries over");
    world.stop(handle);
    world.set_placement(handle, Placement::at(Vec3::new(9.0, 0.0, 0.0)));
    assert_eq!(after(&mut world, 1).len(), 5, "stopped, it fires no more");
}

/// A move too far for one step is a jump: it fires nothing across the gap, and the next move fires along
/// itself from where the jump landed.
#[test]
fn a_rate_a_meter_fires_nothing_across_a_jump() {
    let mut effect = burst(1);
    effect.emitters[0].spawn = Spawn::PerMeter { per_meter: 4.0 };
    effect.emitters[0].motion = Motion::default();
    effect.emitters[0].lifetime = (10.0, 10.0);
    let mut world = world_of(vec![effect], VfxBudget::DESKTOP_LOW);
    let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
    world.set_placement(handle, Placement::at(Vec3::new(2.5, 0.0, 0.0)));
    assert_eq!(after(&mut world, 1).len(), 10, "two and a half metres in a step is a run");
    world.set_placement(handle, Placement::at(Vec3::new(60.0, 0.0, 0.0)));
    assert_eq!(after(&mut world, 1).len(), 10, "a teleport lays nothing across the gap");
    world.set_placement(handle, Placement::at(Vec3::new(61.0, 0.0, 0.0)));
    let all = after(&mut world, 1);
    assert_eq!(all.len(), 14);
    let mut landed: Vec<f32> = all.iter().map(|position| position.x).filter(|&x| x > 3.0).collect();
    landed.sort_by(f32::total_cmp);
    assert_eq!(landed, [60.25, 60.5, 60.75, 61.0], "from where it landed");
}

/// What N2 adds draws only from an emitter's own stream: an emitter gaining a force or a ground moves no
/// particle's birth, and no particle of its effect's other emitters or of an effect spawned after it.
#[test]
fn forces_move_no_other_particle_and_no_birth() {
    let pair = |forced: bool| {
        let mut effect = burst(6);
        effect.emitters.push(dots("other", Spawn::Burst { count: 6 }));
        for emitter in &mut effect.emitters {
            emitter.motion.spread = 60.0;
        }
        if forced {
            effect.emitters[0].forces =
                vec![Force::Noise { strength: 4.0, frequency: 2.0, octaves: 2, drift: 1.0 }];
            effect.emitters[0].collide = Some(Ground { bounce: 0.5, friction: 0.1, dies: false });
        }
        let later = Effect { name: "later".to_owned(), ..burst(6) };
        let mut world = world_of(vec![effect, later], VfxBudget::DESKTOP_LOW);
        world.spawn(EffectId(0), OwnerId::WORLD, Placement::default()).unwrap();
        world.advance(&ALL_RUN);
        let born = drawn(&mut world, 1.0);
        world.spawn(EffectId(1), OwnerId::WORLD, Placement::at(Vec3::Z)).unwrap();
        world.advance(&ALL_RUN);
        world.advance(&ALL_RUN);
        (born, drawn(&mut world, 1.0))
    };
    let ((plain_born, plain), (forced_born, forced)) = (pair(false), pair(true));
    assert_eq!(plain_born.len(), 12);
    assert_eq!(plain_born, forced_born, "every birth as it was, the forced emitter's too");
    // In birth order: the forced emitter's six, its effect's other six, then the later effect's six.
    assert_eq!(plain.len(), 18);
    assert_eq!(plain[6..], forced[6..], "every other particle where it was");
    assert!(
        plain[..6].iter().zip(&forced[..6]).all(|(a, b)| a.position != b.position),
        "the forced six swirled"
    );
}
