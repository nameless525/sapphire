//! Stateless particles (M6 N8a, `docs/m6-plan.md` §3 "Particles: two worlds and a stateless path"): an
//! emitter whose every particle is a closed-form function of its instance's seed, its own index and its
//! instance's age on its owner's clock, so nothing is kept between frames, a frame at any rate shows the
//! same particle at the same age, and hitstop holds an instance by holding its age. This is the CPU
//! reference; the renderer's `procedural_particles.wgsl` evaluates the same function on the GPU, operation
//! for operation, and the renderer's GPU test holds the two together.
//!
//! - **The emitter** ([`StatelessEmitter`], 448 bytes, WGSL `StatelessEmitter`) is a library's emitter
//!   resolved once from its document (`sapphire_vfx`) through [`StatelessParams`]: its births, its source,
//!   its motion, its wind and noise terms and its curves inline, at most [`STATELESS_KEYS`] keys each.
//! - **An instance** ([`StatelessInstance`], 80 bytes) is one playing copy: where it is, how it's turned,
//!   its age, the wind where it plays, when it was stopped and its seed.
//! - **Births.** An emitter keeps a fixed number of slots ([`StatelessEmitter::slots`], at most
//!   [`MAX_STATELESS_SLOTS`]): a burst one a particle; a rate one a particle its longest life can keep alive,
//!   reborn every period; a repeat a pulse's worth for each pulse alive at once. Slot `k` first fills at its
//!   group's birth and again every period, each fill (a *cycle*) with numbers of its own, from a hash of
//!   the seed, `k` and the cycle; the cycle is taken modulo [`STATELESS_CYCLES`], and an endless emitter's
//!   noise drifts a whole number of cells over as many periods, so its age can wrap
//!   ([`StatelessEmitter::wrapped_age`], with its stop) with nothing jumping.
//! - **Motion** is the limit of `sapphire_vfx`'s integrator: speed and direction at birth, gravity, drag as
//!   `e^(-drag t)`, and the wind's pull (`velocity += (wind - velocity) * response * dt`) together are one
//!   linear equation, whose solution is exact for an instance's wind held constant. Noise is the push of a
//!   curl field (an ABC flow, the curl of sines, in one or two octaves) where the particle is, as if it had
//!   felt it all its life: no state, and a swirl that drifts with the instance's age.
//! - **What it can't do:** a particle's path is fixed at birth but for the instance, so moving an instance
//!   moves its living particles whole, and a change of wind bends every living particle's path at once.
//!   There is no collision, attractor, speed limit or spawn along a move: those need state (the CPU path's).
//!
//! Everything here is pure, deterministic and allocation-free; floating-point that the GPU can't match to
//! the bit (`exp`, `sin`, `cos`, `pow`) only ever moves a result within the renderer's test's tolerance.

use bytemuck::{Pod, Zeroable};
use glam::{Quat, Vec3};

use super::{VfxLight, VfxShape};

/// The most particles one stateless emitter keeps alive at once.
pub const MAX_STATELESS_SLOTS: u32 = 4096;

/// How many cycles of a slot have numbers of their own: the cycle is taken modulo this, so an endless
/// emitter repeats every this many periods, and its age can wrap by as much with nothing jumping.
pub const STATELESS_CYCLES: u32 = 256;

/// The most keys a stateless emitter's curve holds inline: `sapphire_vfx`'s curves' most.
pub const STATELESS_KEYS: usize = 8;

/// An instance's stop time while it plays: never (WGSL `STATELESS_NEVER`). Finite, so the GPU's arithmetic
/// on it stays finite.
pub const STATELESS_NEVER: f32 = 1e30;

const TAU: f32 = std::f32::consts::TAU;

/// Where a source kind sits in [`StatelessEmitter`]'s packed kinds (WGSL `STATELESS_SOURCE_*`).
const SOURCE_POINT: u32 = 0;
const SOURCE_SPHERE: u32 = 1;
const SOURCE_DISC: u32 = 2;
const SOURCE_RING: u32 = 3;
const SOURCE_CONE: u32 = 4;
const SOURCE_BOX: u32 = 5;
const SOURCE_LINE: u32 = 6;

/// A facing's code (WGSL `STATELESS_FACING_*`).
const FACING_CAMERA: u32 = 0;
const FACING_VELOCITY: u32 = 1;
const FACING_UPRIGHT: u32 = 2;

/// Which of a particle's numbers each draw takes (WGSL `STATELESS_DRAW_*`): one hash each, so adding a
/// draw moves none of the others.
mod draw {
    pub(super) const LIFETIME: u32 = 0;
    pub(super) const SPEED: u32 = 1;
    pub(super) const CONE_Z: u32 = 2;
    pub(super) const CONE_TURN: u32 = 3;
    pub(super) const SOURCE: u32 = 4;
    pub(super) const SIZE: u32 = 7;
    pub(super) const SPIN: u32 = 8;
    pub(super) const TURN: u32 = 9;
}

/// The second noise octave's offset in cells, so it isn't the first's pattern at half the scale.
const OCTAVE_OFFSET: Vec3 = Vec3::new(0.31, 0.57, 0.13);

/// Over what share of a wrap box's half a particle fades as it nears the box's side.
const WRAP_FADE: f32 = 0.1;

/// `φ₁(z) = (eᶻ − 1) / z` and `φ₂(z) = (eᶻ − 1 − z) / z²` by their series inside `|z| < 1`, where the
/// direct forms lose their digits to cancellation: the terms `zⁿ / (n + 1)!` and `zⁿ / (n + 2)!`, n = 0..=8,
/// highest first for Horner's rule. Written as decimals both sides read to the same `f32`s.
const PHI1: [f32; 9] = [
    2.755_731_9e-6,
    2.480_158_7e-5,
    1.984_127e-4,
    1.388_888_9e-3,
    8.333_333e-3,
    4.166_666_8e-2,
    0.166_666_67,
    0.5,
    1.0,
];
const PHI2: [f32; 9] = [
    2.755_732e-7,
    2.755_731_9e-6,
    2.480_158_7e-5,
    1.984_127e-4,
    1.388_888_9e-3,
    8.333_333e-3,
    4.166_666_8e-2,
    0.166_666_67,
    0.5,
];

/// When a stateless emitter's particles are born.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatelessSpawn {
    /// `count` at once.
    Burst { count: u32 },
    /// `per_second` for `duration` seconds, or until the instance is stopped where `duration` is 0.
    Rate { per_second: f32, duration: f32 },
    /// `count` at once every `interval` seconds, `times` times, or until stopped where `times` is 0.
    Repeat { count: u32, interval: f32, times: u32 },
}

impl StatelessSpawn {
    /// How many slots it keeps for particles that live at most `longest` seconds: every particle that can be
    /// alive at once. What a document's check holds under [`MAX_STATELESS_SLOTS`].
    #[must_use]
    pub fn slots(self, longest: f32) -> u64 {
        let (group, alive) = self.groups(longest);
        u64::from(group) * alive
    }

    /// Particles born together, and how many such groups can be alive at once.
    fn groups(self, longest: f32) -> (u32, u64) {
        let alive = |interval: f64| (f64::from(longest) / interval).ceil().max(1.0) as u64;
        match self {
            Self::Burst { count } => (count, 1),
            Self::Rate { per_second, .. } => (1, alive(1.0 / f64::from(per_second))),
            Self::Repeat { count, interval, .. } => (count, alive(f64::from(interval))),
        }
    }
}

/// Where in the effect's space a stateless emitter's particles are born, before the instance turns them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum StatelessSource {
    /// At the instance's position.
    #[default]
    Point,
    /// Anywhere within a ball of this radius.
    Sphere { radius: f32 },
    /// Anywhere on a level disc of this radius.
    Disc { radius: f32 },
    /// On a level band `width` across, centred on a circle of `radius`.
    Ring { radius: f32, width: f32 },
    /// Anywhere within a cone `length` long up +Y from its apex at the instance, opening `angle` degrees from
    /// its axis (under 90).
    Cone { angle: f32, length: f32 },
    /// Anywhere within a box of these half extents.
    Box { half: Vec3 },
    /// On a line `length` long along X, centred on the instance.
    Line { length: f32 },
}

/// How a stateless emitter's quads turn (`sapphire_vfx`'s facings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatelessFacing {
    /// Toward the camera, turning at `spin` radians a second, each from a random angle.
    Camera { spin: (f32, f32) },
    /// Lengthwise along their velocity, half their length their size plus `stretch` seconds of their speed.
    Velocity { stretch: f32 },
    /// Toward the camera and upright on screen, never turning.
    Upright,
}

/// A stateless emitter's noise: the push of a curl field in `octaves` (1 or 2), `strength` meters per
/// second², `frequency` cells a meter, its pattern moving `drift` cells a second (as `sapphire_vfx`'s noise);
/// an endless emitter packs the drift to the nearest that repeats over its age's wrap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatelessNoise {
    pub strength: f32,
    pub frequency: f32,
    pub octaves: u32,
    pub drift: f32,
}

/// What a stateless emitter is, typed: what [`StatelessEmitter::new`] packs. Ranges draw uniformly between
/// their ends, as a document's do. Curves are `(t, value)` keys over a particle's life, linear between them
/// and level beyond their ends, at least one and at most [`STATELESS_KEYS`] each (past it, the rest are
/// left out).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatelessParams<'a> {
    pub spawn: StatelessSpawn,
    /// Seconds of the instance's age before it starts.
    pub delay: f32,
    /// How long each particle lives, seconds, above 0.
    pub lifetime: (f32, f32),
    pub source: StatelessSource,
    /// Meters per second at birth.
    pub speed: (f32, f32),
    /// The direction they leave in, in the effect's space; normalized here.
    pub direction: Vec3,
    /// How far from `direction` they may leave, degrees.
    pub spread: f32,
    /// Meters per second² down the world's -Y.
    pub gravity: f32,
    /// The share of their speed they lose a second, as `e^(-drag t)`.
    pub drag: f32,
    /// How fast they take the instance's wind, as `sapphire_vfx`'s `Wind { response }`; 0 for none.
    pub wind: f32,
    pub noise: Option<StatelessNoise>,
    pub facing: StatelessFacing,
    /// A random factor on each particle's size.
    pub size_scale: (f32, f32),
    /// Half its width in meters, over its life.
    pub size: &'a [(f32, f32)],
    /// Its opacity over its life.
    pub alpha: &'a [(f32, f32)],
    /// Its linear color over its life.
    pub color: &'a [(f32, [f32; 3])],
    /// Half extents of a box around the instance's wrap centre (the camera, for ambient motes) its particles
    /// wrap into, fading at its sides: a field without end. `None` leaves them where they are.
    pub wrap: Option<Vec3>,
    pub shape: VfxShape,
    pub light: VfxLight,
    /// As a sprite's: 1 is pure light, 0 a surface.
    pub additive: f32,
    /// Over how many meters it fades into the surface behind it.
    pub soft: f32,
}

/// A stateless emitter as the GPU reads it (WGSL `StatelessEmitter`, 448 bytes; see the module docs).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct StatelessEmitter {
    /// x: slots; y: particles born together; z: kinds (source, facing << 4, noise octaves << 8, shape << 12,
    /// light << 16); w: keys (size, alpha << 8, color << 16).
    counts: [u32; 4],
    /// x: seconds between groups' first births; y: a slot's period; z: when births end, 0 never; w: delay.
    timing: [f32; 4],
    /// The lifetime's ends, then the speed's.
    lifetime_speed: [f32; 4],
    /// xyz: the unit direction they leave in; w: the cosine of the spread.
    direction: [f32; 4],
    /// Two unit vectors across `direction`, a cone's turn's axes.
    across_x: [f32; 4],
    across_y: [f32; 4],
    /// The source's sizes: a radius; a ring's radius and width; a cone's tangent and length; a box's half
    /// extents; a line's length.
    source: [f32; 4],
    /// Gravity, drag, the wind's response, the noise's strength.
    motion: [f32; 4],
    /// The noise's frequency and drift.
    noise: [f32; 4],
    /// The size factor's ends, then the spin's.
    scale_spin: [f32; 4],
    /// xyz: the wrap box's half extents; w: 1 if it wraps.
    wrap: [f32; 4],
    /// Additive, soft, a velocity facing's stretch.
    look: [f32; 4],
    /// Size keys, two `(t, value)` a vector.
    size: [[f32; 4]; 4],
    /// Opacity keys, two a vector.
    alpha: [[f32; 4]; 4],
    /// Color keys: t, then the color.
    color: [[f32; 4]; STATELESS_KEYS],
}

const _: () = assert!(size_of::<StatelessEmitter>() == 448, "WGSL StatelessEmitter");

/// One playing copy of a stateless emitter as the GPU reads it (WGSL `StatelessInstance`, 80 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct StatelessInstance {
    /// xyz: where it plays; w: its age, seconds on its owner's clock.
    position: [f32; 4],
    /// How it's turned, a unit quaternion.
    rotation: [f32; 4],
    /// xyz: the wind where it plays, meters a second; w: the age it was stopped at, [`STATELESS_NEVER`] while
    /// it plays.
    wind: [f32; 4],
    /// xyz: the wrap box's centre, for an emitter that wraps.
    wrap_center: [f32; 4],
    /// x: its seed; y: its emitter's place in the renderer's table.
    ids: [u32; 4],
}

const _: () = assert!(size_of::<StatelessInstance>() == 80, "WGSL StatelessInstance");

impl StatelessInstance {
    /// An instance at `position`, turned by `rotation`, `age` seconds into its owner's clock, seeded by
    /// `seed`, of the emitter at `emitter` in the renderer's table: playing, in calm air, wrapping about the
    /// origin.
    #[must_use]
    pub fn new(position: Vec3, rotation: Quat, age: f32, seed: u32, emitter: u32) -> Self {
        Self {
            position: position.extend(age).to_array(),
            rotation: rotation.to_array(),
            wind: [0.0, 0.0, 0.0, STATELESS_NEVER],
            wrap_center: [0.0; 4],
            ids: [seed, emitter, 0, 0],
        }
    }

    /// With the wind where it plays, meters a second.
    #[must_use]
    pub fn with_wind(mut self, wind: Vec3) -> Self {
        self.wind = wind.extend(self.wind[3]).to_array();
        self
    }

    /// With the centre its emitter's wrap box follows: the camera, for ambient motes.
    #[must_use]
    pub fn with_wrap_center(mut self, center: Vec3) -> Self {
        self.wrap_center = center.extend(0.0).to_array();
        self
    }

    /// Stopped at `age` on its owner's clock: no particle is born after it, and those alive live out their lives.
    #[must_use]
    pub fn stopped_at(mut self, age: f32) -> Self {
        self.wind[3] = age;
        self
    }

    /// With its age set: where its owner's clock has it this frame.
    #[must_use]
    pub fn at_age(mut self, age: f32) -> Self {
        self.position[3] = age;
        self
    }

    /// Its age, seconds on its owner's clock.
    #[must_use]
    pub fn age(&self) -> f32 {
        self.position[3]
    }
}

/// A stateless particle at one age: what the vertex stage draws.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatelessParticle {
    /// Whether it lives: born and not yet past its lifetime. A dead one collapses to nothing.
    pub alive: bool,
    /// Where it is in the world.
    pub position: Vec3,
    /// Meters a second, without the noise's push: what a velocity facing lies along.
    pub velocity: Vec3,
    /// Half its width, meters.
    pub half_width: f32,
    /// Radians about the view axis, 0 to τ, for a camera facing; 0 otherwise.
    pub rotation: f32,
    pub color: Vec3,
    /// Its opacity, faded at a wrap box's sides.
    pub alpha: f32,
    /// Its age over its lifetime, 0 to 1.
    pub life: f32,
}

impl StatelessParticle {
    const DEAD: Self = Self {
        alive: false,
        position: Vec3::ZERO,
        velocity: Vec3::ZERO,
        half_width: 0.0,
        rotation: 0.0,
        color: Vec3::ZERO,
        alpha: 0.0,
        life: 0.0,
    };
}

impl StatelessEmitter {
    /// `params` packed. Allocates nothing; a document's check holds its values (`sapphire_vfx`).
    #[must_use]
    pub fn new(params: &StatelessParams<'_>) -> Self {
        let longest = params.lifetime.1;
        let (group, alive) = params.spawn.groups(longest);
        let slots = (u64::from(group) * alive).min(u64::from(u32::MAX)) as u32;
        let alive = alive as f32;
        let (step, end) = match params.spawn {
            // One group, born at 0, whose period is its longest life: births end with it.
            StatelessSpawn::Burst { .. } => (longest, longest),
            StatelessSpawn::Rate { per_second, duration } => (1.0 / per_second, duration),
            StatelessSpawn::Repeat { interval, times, .. } => (interval, times as f32 * interval),
        };
        let (source_kind, source) = match params.source {
            StatelessSource::Point => (SOURCE_POINT, [0.0; 4]),
            StatelessSource::Sphere { radius } => (SOURCE_SPHERE, [radius, 0.0, 0.0, 0.0]),
            StatelessSource::Disc { radius } => (SOURCE_DISC, [radius, 0.0, 0.0, 0.0]),
            StatelessSource::Ring { radius, width } => (SOURCE_RING, [radius, width, 0.0, 0.0]),
            StatelessSource::Cone { angle, length } => {
                (SOURCE_CONE, [angle.to_radians().tan(), length, 0.0, 0.0])
            }
            StatelessSource::Box { half } => (SOURCE_BOX, half.extend(0.0).to_array()),
            StatelessSource::Line { length } => (SOURCE_LINE, [length, 0.0, 0.0, 0.0]),
        };
        let (facing, stretch, spin) = match params.facing {
            StatelessFacing::Camera { spin } => (FACING_CAMERA, 0.0, spin),
            StatelessFacing::Velocity { stretch } => (FACING_VELOCITY, stretch, (0.0, 0.0)),
            StatelessFacing::Upright => (FACING_UPRIGHT, 0.0, (0.0, 0.0)),
        };
        let noise =
            params.noise.unwrap_or(StatelessNoise { strength: 0.0, frequency: 0.0, octaves: 0, drift: 0.0 });
        let octaves = if noise.strength == 0.0 { 0 } else { noise.octaves.clamp(1, 2) };
        let light = match params.light {
            VfxLight::Emissive => 0,
            VfxLight::Lit => 1,
        };
        let axis = params.direction.normalize_or(Vec3::Y);
        let (across_x, across_y) = axis.any_orthonormal_pair();
        let mut size = [[0.0; 4]; 4];
        let mut alpha = [[0.0; 4]; 4];
        let mut color = [[0.0; 4]; STATELESS_KEYS];
        let size_keys = params.size.len().min(STATELESS_KEYS);
        let alpha_keys = params.alpha.len().min(STATELESS_KEYS);
        let color_keys = params.color.len().min(STATELESS_KEYS);
        for (index, &(t, value)) in params.size.iter().take(STATELESS_KEYS).enumerate() {
            size[index / 2][2 * (index % 2)..][..2].copy_from_slice(&[t, value]);
        }
        for (index, &(t, value)) in params.alpha.iter().take(STATELESS_KEYS).enumerate() {
            alpha[index / 2][2 * (index % 2)..][..2].copy_from_slice(&[t, value]);
        }
        for (key, &(t, [r, g, b])) in color.iter_mut().zip(params.color) {
            *key = [t, r, g, b];
        }
        let wrap = params.wrap.map_or([0.0; 4], |half| half.extend(1.0).to_array());
        let period = alive * step;
        // An endless emitter's age wraps (`wrapped_age`), so its noise's drift is packed to repeat with it.
        let drift = if end > 0.0 { noise.drift } else { whole_drift(noise.drift, period) };
        Self {
            counts: [
                slots,
                group.max(1),
                source_kind | facing << 4 | octaves << 8 | params.shape.layer() << 12 | light << 16,
                size_keys as u32 | (alpha_keys as u32) << 8 | (color_keys as u32) << 16,
            ],
            timing: [step, period, end, params.delay],
            lifetime_speed: [params.lifetime.0, params.lifetime.1, params.speed.0, params.speed.1],
            direction: axis.extend(params.spread.to_radians().cos()).to_array(),
            across_x: across_x.extend(0.0).to_array(),
            across_y: across_y.extend(0.0).to_array(),
            source,
            motion: [params.gravity, params.drag, params.wind, noise.strength],
            noise: [noise.frequency, drift, 0.0, 0.0],
            scale_spin: [params.size_scale.0, params.size_scale.1, spin.0, spin.1],
            wrap,
            look: [params.additive, params.soft, stretch, 0.0],
            size,
            alpha,
            color,
        }
    }

    /// How many particles it keeps: the instanced draw's count for an instance of it.
    #[must_use]
    pub fn slots(&self) -> u32 {
        self.counts[0]
    }

    /// A slot's period: how often each slot is born again.
    #[must_use]
    pub fn period(&self) -> f32 {
        self.timing[1]
    }

    /// Whether it emits until its instance is stopped.
    #[must_use]
    pub fn is_endless(&self) -> bool {
        self.timing[2] <= 0.0
    }

    /// An instance's age, `age` seconds of its owner's clock, and the age it was `stopped` at if it was, as
    /// `f32`s it can carry for good ([`StatelessInstance::at_age`], [`StatelessInstance::stopped_at`]).
    ///
    /// An endless emitter's are both moved back by the same whole number of wraps, each [`STATELESS_CYCLES`]
    /// periods, once its age is past twice that, so its age stays under it and its precision never runs out.
    /// That changes no particle: its cycles are counted modulo as much, its noise's drift is packed to repeat
    /// over a wrap ([`StatelessEmitter::new`]), and its age less its stop, all a stop is measured by, is kept.
    /// Another emitter's are as they are.
    #[must_use]
    pub fn wrapped_age(&self, age: f64, stopped: Option<f64>) -> (f32, Option<f32>) {
        let wrap = wrap_seconds(self.period());
        let time = age - f64::from(self.timing[3]);
        let laps = if self.is_endless() && time >= 2.0 * wrap { (time / wrap).floor() - 1.0 } else { 0.0 };
        let back = laps * wrap;
        ((age - back) as f32, stopped.map(|stop| (stop - back) as f32))
    }

    /// Particle `index` of `instance` at the instance's age: the closed form the vertex stage evaluates (see
    /// the module docs). Allocates nothing.
    #[must_use]
    pub fn particle(&self, instance: &StatelessInstance, index: u32) -> StatelessParticle {
        let [slots, group, kinds, keys] = self.counts;
        let [step, period, end, delay] = self.timing;
        let time = instance.position[3] - delay;
        let stop = instance.wind[3] - delay;
        let limit = if end > 0.0 { end.min(stop) } else { stop };
        // A slot past its emitter's, not yet born, or born only after births ended holds nothing.
        if index >= slots {
            return StatelessParticle::DEAD;
        }
        let first = (index / group) as f32 * step;
        if time < first || first >= limit {
            return StatelessParticle::DEAD;
        }
        let mut cycle = ((time - first) / period).floor();
        if limit < STATELESS_NEVER {
            cycle = cycle.min(((limit - first) / period).ceil() - 1.0);
        }
        let age = (time - (first + cycle * period)).max(0.0);
        let cycles = STATELESS_CYCLES as f32;
        let key = particle_key(instance.ids[0], index, (cycle - cycles * (cycle / cycles).floor()) as u32);
        let [lifetime_low, lifetime_high, speed_low, speed_high] = self.lifetime_speed;
        let lifetime = between(lifetime_low, lifetime_high, unit(key, draw::LIFETIME));
        if age >= lifetime {
            return StatelessParticle::DEAD;
        }

        // Where and how it leaves, in the effect's space, then turned by the instance.
        let source = self.source_point(kinds & 15, key);
        let cos_spread = self.direction[3];
        let z = 1.0 - unit(key, draw::CONE_Z) * (1.0 - cos_spread);
        let across = (1.0 - z * z).max(0.0).sqrt();
        let (sin, cos) = turn(unit(key, draw::CONE_TURN));
        let direction =
            vec(self.direction) * z + (vec(self.across_x) * cos + vec(self.across_y) * sin) * across;
        let speed = between(speed_low, speed_high, unit(key, draw::SPEED));
        let start = rotate(instance.rotation, source);
        let launch = rotate(instance.rotation, direction * speed);

        // Gravity, drag and the wind's pull, solved: `v' = A - D v`.
        let [gravity, drag, response, strength] = self.motion;
        let resistance = drag + response;
        let pull = Vec3::new(0.0, -gravity, 0.0) + vec(instance.wind) * response;
        let z = -resistance * age;
        let reach = age * phi(&PHI1, z, (z.exp() - 1.0) / z);
        let fall = age * age * phi(&PHI2, z, (z.exp() - 1.0 - z) / (z * z));
        let mut position = start + launch * reach + pull * fall;
        let velocity = launch * z.exp() + pull * reach;
        let octaves = (kinds >> 8) & 3;
        if octaves > 0 {
            let [frequency, drift, _, _] = self.noise;
            let cells = position * frequency + Vec3::new(0.0, -drift, drift * 0.5) * time;
            let mut flow = abc_flow(cells);
            if octaves > 1 {
                flow += abc_flow(cells * 2.0 + OCTAVE_OFFSET) * 0.5;
            }
            position += flow * (strength * fall);
        }
        position += vec(instance.position);

        let mut fade = 1.0;
        if self.wrap[3] > 0.0 {
            let (half, center) = (vec(self.wrap), vec(instance.wrap_center));
            let span = half * 2.0;
            let offset = position - center;
            let wrapped = offset - span * (offset / span + 0.5).floor();
            position = center + wrapped;
            let edge = ((half - wrapped.abs()) / (half * WRAP_FADE)).clamp(Vec3::ZERO, Vec3::ONE);
            fade = edge.min_element();
        }

        let life = age / lifetime;
        let [scale_low, scale_high, spin_low, spin_high] = self.scale_spin;
        let half_width =
            scalar_curve(&self.size, keys & 15, life) * between(scale_low, scale_high, unit(key, draw::SIZE));
        let rotation = if (kinds >> 4) & 3 == FACING_CAMERA {
            let spin = between(spin_low, spin_high, unit(key, draw::SPIN));
            let turns = unit(key, draw::TURN) + spin * age * (1.0 / TAU);
            TAU * (turns - turns.floor())
        } else {
            0.0
        };
        StatelessParticle {
            alive: true,
            position,
            velocity,
            half_width,
            rotation,
            color: color_curve(&self.color, (keys >> 16) & 15, life),
            alpha: scalar_curve(&self.alpha, (keys >> 8) & 15, life) * fade,
            life,
        }
    }

    /// Where a particle with numbers `key` is born in the effect's space, by source `kind`.
    fn source_point(&self, kind: u32, key: u32) -> Vec3 {
        let [a, b, c, _] = self.source;
        let (u, v, w) = (unit(key, draw::SOURCE), unit(key, draw::SOURCE + 1), unit(key, draw::SOURCE + 2));
        match kind {
            SOURCE_SPHERE => {
                let z = 2.0 * v - 1.0;
                let across = (1.0 - z * z).max(0.0).sqrt();
                let (sin, cos) = turn(w);
                Vec3::new(across * cos, across * sin, z) * (a * u.powf(1.0 / 3.0))
            }
            SOURCE_DISC => {
                let (sin, cos) = turn(u);
                Vec3::new(cos, 0.0, sin) * (a * v.sqrt())
            }
            SOURCE_RING => {
                let (sin, cos) = turn(u);
                Vec3::new(cos, 0.0, sin) * (a + b * (v - 0.5))
            }
            SOURCE_CONE => {
                let height = b * u.powf(1.0 / 3.0);
                let (sin, cos) = turn(w);
                let reach = height * a * v.sqrt();
                Vec3::new(cos * reach, height, sin * reach)
            }
            SOURCE_BOX => Vec3::new((2.0 * u - 1.0) * a, (2.0 * v - 1.0) * b, (2.0 * w - 1.0) * c),
            SOURCE_LINE => Vec3::new((u - 0.5) * a, 0.0, 0.0),
            _ => Vec3::ZERO,
        }
    }
}

/// The PCG hash (Jarzynski and Olano, "Hash Functions for GPU Rendering", 2020; WGSL `particles_hash`):
/// integer operations alone, so the GPU's is the CPU's to the bit. What W6's falling weather hashes by too.
#[must_use]
pub fn particles_hash(value: u32) -> u32 {
    let state = value.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// Number `draw` of a particle's numbers `key`, uniform in [0, 1) (WGSL `particles_unit`): 24 bits, so the
/// `f32` is exact on both sides.
#[must_use]
pub fn particles_unit(key: u32, draw: u32) -> f32 {
    (particles_hash(key.wrapping_add(draw.wrapping_mul(0x9E37_79B9))) >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// `position` wrapped into the box of `half` extents about `center` (WGSL `particles_wrap`): what a field
/// without end, motes about the camera or W6's drops, keeps its points in.
#[must_use]
pub fn particles_wrap(position: Vec3, center: Vec3, half: Vec3) -> Vec3 {
    let span = half * 2.0;
    let offset = position - center;
    center + (offset - span * (offset / span + 0.5).floor())
}

/// A particle's numbers: its instance's seed, its slot and the slot's cycle, hashed.
fn particle_key(seed: u32, index: u32, cycle: u32) -> u32 {
    particles_hash(particles_hash(seed ^ particles_hash(index)) ^ cycle)
}

fn unit(key: u32, draw: u32) -> f32 {
    particles_unit(key, draw)
}

fn between(low: f32, high: f32, share: f32) -> f32 {
    low + (high - low) * share
}

fn vec(value: [f32; 4]) -> Vec3 {
    Vec3::new(value[0], value[1], value[2])
}

/// The sine and cosine of `turns` whole turns, `turns` in [0, 1): an angle the GPU's `sin` keeps precise.
fn turn(turns: f32) -> (f32, f32) {
    let angle = TAU * turns;
    (angle.sin(), angle.cos())
}

/// `vector` turned by the unit quaternion `q`, written as WGSL writes it (`particles_rotate`).
fn rotate(q: [f32; 4], vector: Vec3) -> Vec3 {
    let axis = Vec3::new(q[0], q[1], q[2]);
    let twice = axis.cross(vector) * 2.0;
    vector + twice * q[3] + axis.cross(twice)
}

/// `φ(z)` by its series inside `|z| < 1`, else `direct`.
fn phi(series: &[f32; 9], z: f32, direct: f32) -> f32 {
    if z.abs() < 1.0 {
        let mut sum = series[0];
        for &term in &series[1..] {
            sum = sum * z + term;
        }
        sum
    } else {
        direct
    }
}

/// The ABC flow at `cells` (Arnold, Beltrami and Childress, with its three constants 1, halved): the curl of
/// a field of sines, so it neither gathers nor spreads, each component within ±1.
/// How far an endless emitter's age wraps by: [`STATELESS_CYCLES`] of its periods, seconds.
fn wrap_seconds(period: f32) -> f64 {
    f64::from(period) * f64::from(STATELESS_CYCLES)
}

/// `drift`, cells a second, packed so its pattern moves a whole even number of cells over an endless emitter's
/// wrap, the nearest such: the flow repeats every cell, and its third axis drifts at half the rate, so the field
/// is the same either side of a wrap. As a shader clock's rates pack (`sapphire_assets`'s `whole_cycles`); the
/// step is two cells over the wrap, under 0.004 cells a second for a period of 2 s.
fn whole_drift(drift: f32, period: f32) -> f32 {
    let wrap = wrap_seconds(period);
    if wrap <= 0.0 {
        return drift;
    }
    ((f64::from(drift) * wrap / 2.0).round() * 2.0 / wrap) as f32
}

fn abc_flow(cells: Vec3) -> Vec3 {
    let (sin_x, cos_x) = turn(cells.x - cells.x.floor());
    let (sin_y, cos_y) = turn(cells.y - cells.y.floor());
    let (sin_z, cos_z) = turn(cells.z - cells.z.floor());
    Vec3::new(sin_z + cos_y, sin_x + cos_z, sin_y + cos_x) * 0.5
}

/// A scalar curve of `count` keys packed two a vector, at `t`: `sapphire_vfx`'s `Curve::sample`, step for step.
fn scalar_curve(keys: &[[f32; 4]; 4], count: u32, t: f32) -> f32 {
    let key = |index: usize| {
        let pair = keys[index / 2];
        if index.is_multiple_of(2) { (pair[0], pair[1]) } else { (pair[2], pair[3]) }
    };
    let count = (count as usize).clamp(1, STATELESS_KEYS);
    let mut previous = key(0);
    if t <= previous.0 {
        return previous.1;
    }
    for index in 1..count {
        let next = key(index);
        if t <= next.0 {
            let span = next.0 - previous.0;
            let share = if span > 0.0 { (t - previous.0) / span } else { 1.0 };
            return previous.1 + (next.1 - previous.1) * share;
        }
        previous = next;
    }
    previous.1
}

/// A color curve of `count` keys at `t`, as [`scalar_curve`].
fn color_curve(keys: &[[f32; 4]; STATELESS_KEYS], count: u32, t: f32) -> Vec3 {
    let count = (count as usize).clamp(1, STATELESS_KEYS);
    let mut previous = keys[0];
    if t <= previous[0] {
        return vec([previous[1], previous[2], previous[3], 0.0]);
    }
    for &next in &keys[1..count] {
        if t <= next[0] {
            let span = next[0] - previous[0];
            let share = if span > 0.0 { (t - previous[0]) / span } else { 1.0 };
            let lerp = |a: f32, b: f32| a + (b - a) * share;
            return Vec3::new(
                lerp(previous[1], next[1]),
                lerp(previous[2], next[2]),
                lerp(previous[3], next[3]),
            );
        }
        previous = next;
    }
    Vec3::new(previous[1], previous[2], previous[3])
}

#[cfg(test)]
mod tests;
