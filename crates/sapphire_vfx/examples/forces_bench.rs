//! What a step of forces costs at Desktop-Low's cap: M6 N2's measure first, against its kill line of 0.3 ms at
//! p95 for 2,048 particles under noise.
//!
//! ```text
//! cargo run --release -p sapphire_vfx --example forces_bench [steps]
//! ```
//!
//! Fills a world at Desktop-Low's budget with 2,048 particles that live longer than the run, and times each
//! [`VfxWorld::advance`] at 60 Hz for four arms in alternating rounds: `plain` (gravity and drag only, the step
//! as it was before forces), `noise` (two octaves, every particle, the row's measure), `noise-alternate` (the
//! same under `noise_on_alternate_steps`, the kill line's first fallback) and `every-force` (wind, noise, a
//! vortex, an attractor, a speed limit and the ground). For each arm it prints the steps' median, p95 and
//! slowest over every round, the rounds' p95s, the particle count every step held, and the allocations the
//! timed steps made, which must be none. It reads the clock here, as the app does, never in the crate (R2).

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::hint::black_box;
use std::time::Duration;

use glam::Vec3;
use sapphire_core::alloc::{CountingAllocator, count_in_scope};
use sapphire_time::Stopwatch;
use sapphire_vfx::{
    Curve, Effect, EffectId, EffectLibrary, EmitterDesc, Facing, Force, Ground, Light, Motion, OwnerId,
    OwnerSteps, Placement, Shape, Sim, Source, Spawn, VfxBudget, VfxWorld, WindSample,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

const TICK: f32 = 1.0 / 60.0;
/// Desktop-Low's cap, which the row measures at.
const PARTICLES: u32 = 2048;
const ROUNDS: u32 = 5;
/// Steps a round runs before it times: the burst is born and the forces are under way.
const WARM_UP: u32 = 30;
/// The kill line, microseconds at p95 (`docs/m6-plan.md`, N2).
const KILL_LINE_US: f64 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Arm {
    Plain,
    Noise,
    NoiseAlternate,
    EveryForce,
}

impl Arm {
    const ALL: [Self; 4] = [Self::Plain, Self::Noise, Self::NoiseAlternate, Self::EveryForce];

    fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Noise => "noise",
            Self::NoiseAlternate => "noise-alternate",
            Self::EveryForce => "every-force",
        }
    }

    fn forces(self) -> (Vec<Force>, Option<Ground>) {
        let noise = Force::Noise { strength: 3.0, frequency: 1.5, octaves: 2, drift: 0.5 };
        match self {
            Self::Plain => (Vec::new(), None),
            Self::Noise | Self::NoiseAlternate => (vec![noise], None),
            Self::EveryForce => (
                vec![
                    Force::Wind { response: 1.5 },
                    noise,
                    Force::Vortex { axis: [0.0, 1.0, 0.0], strength: 2.0, pull: 1.0 },
                    Force::Attract { point: [0.0, 1.0, 0.0], strength: 1.0, kill_radius: 0.0 },
                    Force::LimitSpeed { max: 3.0 },
                ],
                Some(Ground { bounce: 0.4, friction: 0.2, dies: false }),
            ),
        }
    }

    /// A world of this arm's 2,048 particles, none of which ends during a round.
    fn world(self) -> VfxWorld {
        let (forces, collide) = self.forces();
        let emitter = EmitterDesc {
            name: "field".to_owned(),
            spawn: Spawn::Burst { count: PARTICLES },
            delay: 0.0,
            source: Source::Sphere(2.0),
            lifetime: (1000.0, 1000.0),
            motion: Motion { speed: (0.5, 2.0), spread: 180.0, gravity: 0.3, drag: 0.5, ..Motion::default() },
            shape: Shape::SoftDot,
            facing: Facing::Camera { spin: (-1.0, 1.0) },
            light: Light::Emissive,
            additive: 1.0,
            soft: 0.02,
            size_scale: (0.7, 1.3),
            size: Curve(vec![(0.0, 0.02)]),
            color: Curve(vec![(0.0, [2.0, 1.0, 0.4])]),
            alpha: Curve(vec![(0.0, 1.0)]),
            forces,
            collide,
            sim: Sim::Cpu,
            wrap: None,
            sheet: None,
        };
        let effect = Effect {
            name: "field".to_owned(),
            emitters: vec![emitter],
            trail: None,
            mark: false,
            meshes: Vec::new(),
            decals: Vec::new(),
        };
        let library = EffectLibrary::new(vec![effect]).expect("the bench's effect is valid");
        let budget =
            VfxBudget { noise_on_alternate_steps: self == Self::NoiseAlternate, ..VfxBudget::DESKTOP_LOW };
        let mut world = VfxWorld::new(library, budget, 7);
        world.set_wind(WindSample { velocity: Vec3::new(1.0, 0.0, 0.3) });
        let handle = world.spawn(EffectId(0), OwnerId::WORLD, Placement::at(Vec3::new(0.0, 1.0, 0.0)));
        if let Some(handle) = handle {
            world.set_ground(handle, 0.0);
        }
        world
    }
}

/// A round's timed steps, with the fewest particles any step held and the allocations they made.
struct Round {
    times: Vec<Duration>,
    fewest: u32,
    allocations: u64,
}

fn round(arm: Arm, steps: u32) -> Round {
    let mut world = arm.world();
    let steps_of = OwnerSteps { world: TICK, owners: &[] };
    for _ in 0..WARM_UP {
        world.advance(&steps_of);
    }
    let mut times = Vec::with_capacity(steps as usize);
    let mut fewest = u32::MAX;
    let ((), counts) = count_in_scope(|| {
        for _ in 0..steps {
            let clock = Stopwatch::start();
            world.advance(black_box(&steps_of));
            times.push(clock.elapsed());
            fewest = fewest.min(world.stats().particles);
        }
    });
    Round { times, fewest, allocations: counts.allocations }
}

fn micros(time: Duration) -> f64 {
    time.as_secs_f64() * 1e6
}

/// Sorted steps' median, p95 and slowest, microseconds.
fn spread(times: &mut [Duration]) -> (f64, f64, f64) {
    times.sort_unstable();
    let at = |share: usize| micros(times[(times.len() * share / 100).min(times.len() - 1)]);
    (at(50), at(95), micros(times[times.len() - 1]))
}

fn main() {
    let steps = std::env::args().nth(1).and_then(|steps| steps.parse().ok()).unwrap_or(600_u32).max(20);
    println!(
        "forces_bench: {PARTICLES} particles, Desktop-Low's budget, {ROUNDS} rounds of {steps} steps an arm"
    );
    let mut all = Arm::ALL.map(|_| (Vec::new(), Vec::new(), u32::MAX, 0_u64));
    for _ in 0..ROUNDS {
        for (index, arm) in Arm::ALL.into_iter().enumerate() {
            let mut round = round(arm, steps);
            let (times, p95s, fewest, allocations) = &mut all[index];
            *fewest = (*fewest).min(round.fewest);
            *allocations += round.allocations;
            p95s.push(spread(&mut round.times.clone()).1);
            times.append(&mut round.times);
        }
    }
    for (arm, (times, p95s, fewest, allocations)) in Arm::ALL.into_iter().zip(&mut all) {
        let (median, p95, worst) = spread(times);
        let rounds: Vec<String> = p95s.iter().map(|p95| format!("{p95:.1}")).collect();
        println!(
            "{:<16} median {median:7.1} us  p95 {p95:7.1} us  worst {worst:8.1} us  rounds' p95 [{}]  particles {fewest}  allocations {allocations}",
            arm.name(),
            rounds.join(", "),
        );
        if arm == Arm::Noise {
            let verdict = if p95 <= KILL_LINE_US { "within" } else { "OVER" };
            println!(
                "noise at {PARTICLES}: p95 {p95:.1} us, {verdict} the kill line of {KILL_LINE_US:.0} us"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each arm holds its 2,048 particles through a short round and its steps allocate nothing: what the
    /// timed run reads is what it says, with no load of the machine in it.
    #[test]
    fn every_arm_holds_its_particles_and_allocates_nothing() {
        for arm in Arm::ALL {
            let round = round(arm, 20);
            assert_eq!(round.fewest, PARTICLES, "{arm:?}");
            assert_eq!(round.allocations, 0, "{arm:?}");
            assert_eq!(round.times.len(), 20);
        }
    }
}
