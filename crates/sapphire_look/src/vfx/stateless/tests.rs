use super::*;

const SIZE: [(f32, f32); 3] = [(0.0, 0.05), (0.3, 0.2), (1.0, 0.0)];
const ALPHA: [(f32, f32); 2] = [(0.0, 1.0), (1.0, 0.0)];
const COLOR: [(f32, [f32; 3]); 2] = [(0.0, [1.0, 0.8, 0.4]), (1.0, [0.2, 0.1, 0.0])];

/// A burst of 64 from a point, straight up, with nothing pulling on it.
fn params() -> StatelessParams<'static> {
    StatelessParams {
        spawn: StatelessSpawn::Burst { count: 64 },
        delay: 0.0,
        lifetime: (1.0, 2.0),
        source: StatelessSource::Point,
        speed: (1.0, 3.0),
        direction: Vec3::Y,
        spread: 30.0,
        gravity: 0.0,
        drag: 0.0,
        wind: 0.0,
        noise: None,
        facing: StatelessFacing::Camera { spin: (-2.0, 2.0) },
        size_scale: (0.5, 1.5),
        size: &SIZE,
        alpha: &ALPHA,
        color: &COLOR,
        wrap: None,
        shape: VfxShape::SoftDot,
        light: VfxLight::Emissive,
        additive: 1.0,
        soft: 0.02,
    }
}

fn instance(age: f32) -> StatelessInstance {
    StatelessInstance::new(Vec3::new(3.0, 1.0, -2.0), Quat::IDENTITY, age, 7, 0)
}

fn alive(emitter: &StatelessEmitter, instance: &StatelessInstance) -> usize {
    (0..emitter.slots()).filter(|&index| emitter.particle(instance, index).alive).count()
}

/// Gravity, drag and the wind's pull, solved, are where `sapphire_vfx`'s integrator goes as its step shrinks:
/// its order (gravity, the wind's pull, drag, then the move) stepped 48,000 times a second in `f64` lands
/// within half a millimetre of the closed form after a second and a half, for every particle.
#[test]
fn the_closed_form_is_where_the_integrator_goes() {
    let cases = [(9.8, 0.0, 0.0), (9.8, 1.5, 0.0), (-1.0, 0.4, 2.0), (0.0, 0.0, 6.0), (4.0, 3.0, 0.5)];
    let wind = Vec3::new(2.0, 0.5, -1.0);
    let turned = Quat::from_rotation_z(0.7) * Quat::from_rotation_y(-1.1);
    for (gravity, drag, response) in cases {
        let emitter = StatelessEmitter::new(&StatelessParams {
            lifetime: (2.0, 2.0),
            gravity,
            drag,
            wind: response,
            source: StatelessSource::Sphere { radius: 0.5 },
            ..params()
        });
        let born = StatelessInstance::new(Vec3::new(3.0, 1.0, -2.0), turned, 0.0, 11, 0).with_wind(wind);
        let age = 1.5;
        let dt = 1.0 / 48_000.0;
        for index in 0..emitter.slots() {
            let start = emitter.particle(&born, index);
            let (mut position, mut velocity) = (start.position.as_dvec3(), start.velocity.as_dvec3());
            let (gravity, drag, response, wind) =
                (f64::from(gravity), f64::from(drag), f64::from(response), wind.as_dvec3());
            let (pull, keep) = (1.0 - (-response * dt).exp(), (-drag * dt).exp());
            for _ in 0..(f64::from(age) / dt).round() as u32 {
                velocity.y -= gravity * dt;
                velocity += (wind - velocity) * pull;
                velocity *= keep;
                position += velocity * dt;
            }
            let closed = emitter.particle(&born.at_age(age), index);
            assert!(closed.alive);
            let error = (closed.position.as_dvec3() - position).length();
            assert!(error < 5e-4, "{gravity}, {drag}, {response}: particle {index} off by {error} m");
            let error = (closed.velocity.as_dvec3() - velocity).length();
            assert!(
                error < 1e-3,
                "{gravity}, {drag}, {response}: particle {index}'s velocity off by {error}"
            );
        }
    }
}

/// A burst is born whole at its start, after its delay, and gone by its longest life; each particle dies at
/// its own lifetime, never before.
#[test]
fn a_burst_is_born_at_once_and_lives_its_lifetimes() {
    let emitter = StatelessEmitter::new(&StatelessParams { delay: 0.5, ..params() });
    assert_eq!(emitter.slots(), 64);
    assert!(!emitter.is_endless());
    assert_eq!(alive(&emitter, &instance(0.49)), 0, "before its delay");
    assert_eq!(alive(&emitter, &instance(0.5)), 64);
    assert_eq!(alive(&emitter, &instance(1.49)), 64, "the shortest life is a second");
    let middle = alive(&emitter, &instance(2.0));
    assert!((8..56).contains(&middle), "{middle} of 64 halfway through their lives");
    assert_eq!(alive(&emitter, &instance(2.5)), 0, "the longest life is two seconds");
    assert_eq!(alive(&emitter, &instance(60.0)), 0, "a burst fires once");
    // Past its slots, nothing.
    assert!(!emitter.particle(&instance(1.0), 64).alive);
}

/// A rate keeps as many slots as its longest life holds, and settles to its rate times the mean life; with a
/// duration it stops being born then, and a stop does the same for an endless one.
#[test]
fn a_rate_keeps_its_rate_and_stops_where_it_is_told() {
    let rate = |duration| StatelessParams {
        spawn: StatelessSpawn::Rate { per_second: 100.0, duration },
        ..params()
    };
    let endless = StatelessEmitter::new(&rate(0.0));
    assert_eq!(endless.slots(), 200);
    assert!(endless.is_endless());
    for age in [10.0, 37.3, 500.0] {
        let count = alive(&endless, &instance(age));
        assert!((135..165).contains(&count), "{count} alive at {age} s: about 150");
    }
    let early = alive(&endless, &instance(0.5));
    assert!((50..=51).contains(&early), "{early}: one at once, then one every hundredth");
    let stopped = instance(30.0).stopped_at(29.0);
    let count = alive(&endless, &stopped);
    assert!((20..80).contains(&count), "{count} still alive a second after a stop");
    assert_eq!(alive(&endless, &instance(31.01).stopped_at(29.0)), 0, "none past the longest life");

    let timed = StatelessEmitter::new(&rate(3.0));
    assert!(alive(&timed, &instance(2.9)) > 100);
    assert_eq!(alive(&timed, &instance(5.01)), 0);
    // No slot is born after the duration: what lives a moment after it was born before it.
    let after = instance(3.5);
    for index in 0..timed.slots() {
        let particle = timed.particle(&after, index);
        assert!(!particle.alive || particle.life * 2.0 >= 0.5 - 1e-4, "slot {index}: {particle:?}");
    }
}

/// A repeat fires its count each interval, `times` times, keeping a pulse's slots for each pulse alive at once.
#[test]
fn a_repeat_fires_its_pulses() {
    let emitter = StatelessEmitter::new(&StatelessParams {
        spawn: StatelessSpawn::Repeat { count: 10, interval: 0.75, times: 4 },
        lifetime: (1.0, 1.0),
        ..params()
    });
    assert_eq!(emitter.slots(), 20);
    assert_eq!(alive(&emitter, &instance(0.1)), 10);
    assert_eq!(alive(&emitter, &instance(0.8)), 20, "two pulses overlap");
    assert_eq!(alive(&emitter, &instance(1.1)), 10);
    assert_eq!(alive(&emitter, &instance(2.3)), 20, "the third and fourth pulses");
    assert_eq!(alive(&emitter, &instance(3.3)), 0, "no fifth");
}

/// An endless emitter's age wraps with nothing moving, two octaves of drifting noise and all: its cycles are
/// counted modulo `STATELESS_CYCLES`, and its drift is packed to repeat over as many periods. A wrapped age
/// lands within a millimetre of where it was before the laps (an `f32` age past a wrap, about 520 s here, is
/// good to 6.1e-5 s).
#[test]
fn an_endless_emitters_age_wraps_with_nothing_jumping() {
    let emitter = StatelessEmitter::new(&StatelessParams {
        spawn: StatelessSpawn::Rate { per_second: 40.0, duration: 0.0 },
        delay: 0.25,
        noise: Some(StatelessNoise { strength: 2.0, frequency: 0.7, octaves: 2, drift: 0.3 }),
        ..params()
    });
    let wrap = f64::from(emitter.period()) * f64::from(STATELESS_CYCLES);
    let near = 0.25 + wrap + 7.123;
    assert_eq!(emitter.wrapped_age(near, None), (near as f32, None), "unwrapped below twice the wrap");
    let close = |a: StatelessParticle, b: StatelessParticle, what: &str| {
        assert_eq!(a.alive, b.alive, "{what}");
        assert!((a.position - b.position).length() < 1e-3, "{what}: {a:?} against {b:?}");
    };
    for laps in [1.0, 3.0, 1000.0] {
        let (far, _) = emitter.wrapped_age(near + laps * wrap, None);
        assert!((far - near as f32).abs() < 1e-3, "{laps} laps: {far} against {near}");
        for index in 0..emitter.slots() {
            let (a, b) =
                (emitter.particle(&instance(near as f32), index), emitter.particle(&instance(far), index));
            close(a, b, &format!("{laps} laps, slot {index}"));
        }
    }
    // And the function itself repeats every wrap, noise and all, within 5 mm: an age a wrap on (1,031 s) is
    // good to a step of 1.2e-4 s, and the noise, pushing each particle by where it already is, carries that to
    // 2.3 mm at the worst here. A drift that didn't repeat would move them by a metre and more.
    let mut worst = 0.0_f32;
    for index in 0..emitter.slots() {
        let (a, b) = (
            emitter.particle(&instance(near as f32), index),
            emitter.particle(&instance((near + wrap) as f32), index),
        );
        assert_eq!(a.alive, b.alive, "a wrap on, slot {index}");
        worst = worst.max((a.position - b.position).length());
    }
    assert!(worst < 5e-3, "{worst} m a wrap on");
    let burst = StatelessEmitter::new(&params());
    assert_eq!(burst.wrapped_age(1e6, Some(2.0)), (1e6, Some(2.0)), "an emitter that ends doesn't wrap");
}

/// A drift an endless emitter can't repeat over its wrap would move every noisy particle at each wrap, a
/// metre and more; the packed one is the nearest that does, and one that ends keeps its own.
#[test]
fn an_endless_emitters_drift_repeats_over_its_wrap() {
    let noise = Some(StatelessNoise { strength: 2.0, frequency: 0.7, octaves: 2, drift: 0.3 });
    let endless = StatelessEmitter::new(&StatelessParams {
        spawn: StatelessSpawn::Rate { per_second: 40.0, duration: 0.0 },
        noise,
        ..params()
    });
    let wrap = f64::from(endless.period()) * f64::from(STATELESS_CYCLES);
    let cells = f64::from(endless.noise[1]) * wrap;
    assert!((cells / 2.0 - (cells / 2.0).round()).abs() < 1e-4, "{cells} cells over a wrap");
    assert!((endless.noise[1] - 0.3).abs() <= (1.0 / wrap) as f32, "{} against 0.3", endless.noise[1]);
    let timed = StatelessEmitter::new(&StatelessParams {
        spawn: StatelessSpawn::Rate { per_second: 40.0, duration: 3.0 },
        noise,
        ..params()
    });
    assert_eq!(timed.noise[1], 0.3);
}

/// A stop and its age wrap together: an instance stopped just under twice its emitter's wrap, whose age then
/// wraps below the stop, still bears nothing after it, and its last particles die out within their lives.
#[test]
fn a_stopped_instance_wraps_with_its_stop() {
    let emitter = StatelessEmitter::new(&StatelessParams {
        spawn: StatelessSpawn::Rate { per_second: 40.0, duration: 0.0 },
        delay: 0.25,
        ..params()
    });
    let wrap = f64::from(emitter.period()) * f64::from(STATELESS_CYCLES);
    let stop = 0.25 + 2.0 * wrap - 0.4;
    let at = |age: f64| {
        let (age, stopped) = emitter.wrapped_age(age, Some(stop));
        instance(age).stopped_at(stopped.expect("a stop"))
    };
    let playing = alive(&emitter, &at(stop));
    assert!(playing > 40, "{playing} alive at the stop");
    let mut last = playing;
    for step in 1..=300 {
        let age = stop + f64::from(step) * 0.01;
        let now = alive(&emitter, &at(age));
        assert!(now <= last, "{now} alive {age} s on, after {last}: born after the stop");
        last = now;
    }
    assert!(emitter.wrapped_age(stop + 1.0, Some(stop)).0 < stop as f32, "its age wrapped below the stop");
    assert_eq!(last, 0, "all dead a longest life past the stop");
}

/// Each source keeps its particles within its shape, and across it.
#[test]
fn sources_keep_their_shapes() {
    let at_birth = |source| {
        let emitter = StatelessEmitter::new(&StatelessParams {
            spawn: StatelessSpawn::Burst { count: 2048 },
            source,
            speed: (0.0, 0.0),
            ..params()
        });
        let born = StatelessInstance::new(Vec3::ZERO, Quat::IDENTITY, 0.0, 3, 0);
        (0..emitter.slots()).map(move |index| emitter.particle(&born, index).position)
    };
    let sphere = at_birth(StatelessSource::Sphere { radius: 2.0 }).map(Vec3::length).fold(0.0, f32::max);
    assert!(sphere <= 2.0 + 1e-5 && sphere > 1.95, "{sphere}");
    for point in at_birth(StatelessSource::Disc { radius: 1.5 }) {
        assert!(point.y == 0.0 && point.length() <= 1.5 + 1e-5, "{point}");
    }
    for point in at_birth(StatelessSource::Ring { radius: 3.0, width: 0.4 }) {
        let reach = point.length();
        assert!(point.y == 0.0 && (2.8 - 1e-5..=3.2 + 1e-5).contains(&reach), "{point}");
    }
    for point in at_birth(StatelessSource::Cone { angle: 30.0, length: 2.0 }) {
        let across = Vec3::new(point.x, 0.0, point.z).length();
        assert!((0.0..=2.0 + 1e-5).contains(&point.y), "{point}");
        assert!(across <= point.y * 30f32.to_radians().tan() + 1e-5, "{point}");
    }
    let half = Vec3::new(1.0, 0.5, 2.0);
    let corner = at_birth(StatelessSource::Box { half }).fold(Vec3::ZERO, |most, point| {
        assert!(point.abs().cmple(half).all(), "{point}");
        most.max(point.abs())
    });
    assert!(corner.cmpgt(half * 0.99).all(), "{corner}");
    for point in at_birth(StatelessSource::Line { length: 4.0 }) {
        assert!(point.y == 0.0 && point.z == 0.0 && point.x.abs() <= 2.0, "{point}");
    }
    assert!(at_birth(StatelessSource::Point).all(|point| point == Vec3::ZERO));
}

/// They leave within the spread of their direction, turned by the instance.
#[test]
fn they_leave_within_their_spread_turned_by_the_instance() {
    let emitter = StatelessEmitter::new(&StatelessParams { direction: Vec3::new(1.0, 1.0, 0.0), ..params() });
    let turned = Quat::from_rotation_y(1.2);
    let born = StatelessInstance::new(Vec3::ZERO, turned, 0.0, 5, 0);
    let axis = turned * Vec3::new(1.0, 1.0, 0.0).normalize();
    let mut widest = 0.0_f32;
    for index in 0..emitter.slots() {
        let velocity = emitter.particle(&born, index).velocity;
        let angle = velocity.angle_between(axis).to_degrees();
        assert!(angle <= 30.0 + 1e-3, "{angle}");
        assert!((1.0 - 1e-5..=3.0 + 1e-5).contains(&velocity.length()));
        widest = widest.max(angle);
    }
    assert!(widest > 20.0, "{widest}");
}

/// The curves, sizes and spins at an age: the size factor scales the curve, opacity follows its curve, and a
/// camera facing turns at its spin.
#[test]
fn curves_and_spin_follow_the_age() {
    let emitter = StatelessEmitter::new(&StatelessParams { lifetime: (2.0, 2.0), ..params() });
    let (young, old) = (instance(0.3), instance(1.3));
    for index in 0..8 {
        let (a, b) = (emitter.particle(&young, index), emitter.particle(&old, index));
        assert_eq!((a.life, b.life), (0.15, 0.65));
        let scale = a.half_width / (0.05 + (0.2 - 0.05) * (0.15 / 0.3));
        assert!((0.5..=1.5).contains(&scale), "{scale}");
        assert!((b.alpha - 0.35).abs() < 1e-6 && (a.alpha - 0.85).abs() < 1e-6);
        assert!(
            (a.color - Vec3::new(1.0 - 0.8 * 0.15, 0.8 - 0.7 * 0.15, 0.4 - 0.4 * 0.15)).abs().max_element()
                < 1e-6
        );
        let turned = (b.rotation - a.rotation).rem_euclid(TAU);
        assert!((0.0..TAU).contains(&a.rotation));
        assert!(turned <= 2.0 + 1e-4 || turned >= TAU - 2.0 - 1e-4, "{turned} in a second at most 2 rad/s");
    }
    let upright = StatelessEmitter::new(&StatelessParams { facing: StatelessFacing::Upright, ..params() });
    assert_eq!(upright.particle(&young, 0).rotation, 0.0);
}

/// The noise pushes each particle within its strength times its fall, and differently from its neighbours;
/// none without it.
#[test]
fn noise_pushes_within_its_strength() {
    let still =
        StatelessParams { speed: (0.0, 0.0), source: StatelessSource::Sphere { radius: 4.0 }, ..params() };
    let calm = StatelessEmitter::new(&still);
    for octaves in [1, 2] {
        let noise = StatelessNoise { strength: 2.0, frequency: 0.5, octaves, drift: 0.3 };
        let pushed = StatelessEmitter::new(&StatelessParams { noise: Some(noise), ..still });
        let at = instance(0.9);
        let mut moved = Vec::new();
        for index in 0..pushed.slots() {
            let push = pushed.particle(&at, index).position - calm.particle(&at, index).position;
            let most = 2.0 * 0.5 * 0.9 * 0.9 * if octaves == 1 { 3f32.sqrt() } else { 1.5 * 3f32.sqrt() };
            assert!(push.length() <= most + 1e-4, "{octaves}: {push}");
            moved.push(push);
        }
        assert!(moved.iter().any(|push| push.length() > 0.3), "{octaves}");
        assert!(moved.windows(2).any(|pair| (pair[0] - pair[1]).length() > 0.1), "{octaves}");
    }
}

/// A wrapping emitter's particles stay in the box about its centre wherever that goes, fading at its sides.
#[test]
fn a_wrap_keeps_particles_about_its_centre() {
    let half = Vec3::new(10.0, 4.0, 10.0);
    let emitter = StatelessEmitter::new(&StatelessParams {
        spawn: StatelessSpawn::Rate { per_second: 200.0, duration: 0.0 },
        source: StatelessSource::Box { half },
        wrap: Some(half),
        speed: (0.0, 0.5),
        ..params()
    });
    let mut faded = 0;
    for center in [Vec3::ZERO, Vec3::new(137.5, -3.0, 2048.25)] {
        let at = instance(40.0).with_wrap_center(center);
        for index in 0..emitter.slots() {
            let particle = emitter.particle(&at, index);
            if !particle.alive {
                continue;
            }
            let offset = particle.position - center;
            assert!(offset.abs().cmple(half + 1e-3).all(), "{offset}");
            if (offset.abs() - half * 0.9).max_element() > 1e-3 {
                faded += 1;
                assert!(particle.alpha < 1.0 - particle.life || particle.alpha == 0.0, "{particle:?}");
            }
        }
    }
    assert!(faded > 0);
    let wrapped = particles_wrap(Vec3::new(25.0, -9.0, 0.5), Vec3::ZERO, half);
    assert!((wrapped - Vec3::new(5.0, -1.0, 0.5)).length() < 1e-5, "{wrapped}");
}

/// The hash is a function of its input alone, its units cover [0, 1), and an instance's seed sets its
/// particles apart.
#[test]
fn the_hash_spreads_and_seeds_set_instances_apart() {
    assert_eq!(particles_hash(0), particles_hash(0));
    assert_ne!(particles_hash(1), particles_hash(2));
    let (mut low, mut high) = (1.0_f32, 0.0_f32);
    for key in 0..4096 {
        let unit = particles_unit(particles_hash(key), 3);
        assert!((0.0..1.0).contains(&unit));
        (low, high) = (low.min(unit), high.max(unit));
    }
    assert!(low < 0.01 && high > 0.99, "{low} to {high}");
    let emitter = StatelessEmitter::new(&params());
    let other = StatelessInstance::new(Vec3::new(3.0, 1.0, -2.0), Quat::IDENTITY, 0.6, 8, 0);
    assert_ne!(emitter.particle(&instance(0.6), 0).position, emitter.particle(&other, 0).position);
    assert_eq!(emitter.particle(&instance(0.6), 5), emitter.particle(&instance(0.6), 5));
}

/// A zeroed emitter (what a table holds before it's written) draws nothing, and panics at nothing.
#[test]
fn a_zeroed_emitter_draws_nothing() {
    let emitter = StatelessEmitter::zeroed();
    assert_eq!(emitter.slots(), 0);
    assert!(!emitter.particle(&instance(1.0), 0).alive);
}
