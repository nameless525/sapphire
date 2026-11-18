//! Forces and ground collision on an emitter's particles (M6 N2): wind, a curl of noise, a vortex, an attractor
//! and a speed limit ([`Force`]), and a plane at the owner's ground they bounce on, slide along or end at
//! ([`Ground`]). An emitter lists them in its document; a step resolves each playing emitter's into world space
//! once ([`Resolved`]), and then each particle runs through what its emitter resolved to.
//!
//! None of it takes a random number from an effect's stream: a noise's offset comes from a stream of the
//! emitter's own, forked off the effect's without advancing it, so an emitter gaining a force moves no
//! particle's birth, and a library without forces steps exactly as it did before them.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::document::{Problem, require, share_ok};
use crate::rng::Rng;
use crate::wind::WindSample;

/// The most forces an emitter has: each playing emitter resolves this many inline, so a step never allocates.
pub const MAX_FORCES: usize = 6;

/// The most octaves a noise sums: each is a lattice's eight corners per particle per step.
pub const MAX_NOISE_OCTAVES: u8 = 2;

/// How far apart, in noise cells, two emitters' noises may start: far enough that no two effects swirl alike.
const NOISE_SPAN: f32 = 4096.0;

/// What a sum of curls is scaled by, so that one octave's acceleration is about 1 at its root mean square
/// (`a_noises_strength_is_its_typical_acceleration` measures it).
const CURL_SCALE: f32 = 0.62;

/// A force on an emitter's particles, each step of their owner's clock, in the order the emitter lists them.
/// Points and axes are in the effect's space, so they follow it where it's [placed](crate::VfxWorld::set_placement)
/// from step to step, while the particles stay in the world.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Force {
    /// Carried by the [wind](crate::VfxWorld::set_wind): the gap between a particle's velocity and the air's
    /// closes as `e^(-response t)`, so a mote of 8 drifts with a breeze within a fraction of a second and an
    /// ember of 1 leans into it.
    Wind { response: f32 },
    /// A swirl that changes over space and time: the curl of a lattice of hashed values, so it neither gathers
    /// particles nor spreads them. `strength` m/s² is its typical acceleration; `frequency` its cells a meter
    /// (2 swirls about every half meter); `octaves` 1 or 2, the second twice as fine at half the strength;
    /// `drift` the cells a second its pattern moves through, 0 for one that holds still.
    Noise { strength: f32, frequency: f32, octaves: u8, drift: f32 },
    /// Round `axis` through the effect's position: `strength` m/s² along the way round (the right-hand turn
    /// about the axis; negative turns the other way), and `pull` m/s² in toward the axis, which keeps a column
    /// from flinging itself apart.
    Vortex { axis: [f32; 3], strength: f32, pull: f32 },
    /// Toward `point`: `strength` m/s² whatever the distance (negative pushes away). A particle that comes
    /// within `kill_radius` meters ends there, gathered, as a charge-up's motes do; 0 lets them pass through.
    Attract { point: [f32; 3], strength: f32, kill_radius: f32 },
    /// No faster than `max` m/s, after the forces before it in the list.
    LimitSpeed { max: f32 },
}

/// A level plane at the owner's ground ([`VfxWorld::set_ground`](crate::VfxWorld::set_ground)), which an
/// emitter's particles meet as they fall through it.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ground {
    /// The share of its speed into the ground a particle keeps, coming back out: 0 lands dead, 1 bounces
    /// as high as it fell.
    #[serde(default)]
    pub bounce: f32,
    /// The share of its speed along the ground a particle loses each sixtieth of a second on its owner's clock
    /// that it touches the ground, as a rate whatever the step: 0 skates, 1 stops.
    #[serde(default)]
    pub friction: f32,
    /// Whether a particle ends where it meets the ground, as a raindrop does, rather than bouncing.
    #[serde(default)]
    pub dies: bool,
}

impl Force {
    pub(crate) fn problem(&self) -> Result<(), Problem> {
        let finite = |value: f32| value.is_finite();
        match *self {
            Self::Wind { response } => require(
                finite(response) && response >= 0.0,
                "forces",
                "a wind's `response` can't be negative",
            ),
            Self::Noise { strength, frequency, octaves, drift } => {
                require(finite(strength), "forces", "a noise's `strength` must be finite")?;
                require(
                    finite(frequency) && frequency > 0.0,
                    "forces",
                    "a noise's `frequency` must be above 0",
                )?;
                require((1..=MAX_NOISE_OCTAVES).contains(&octaves), "forces", "a noise has 1 or 2 octaves")?;
                require(finite(drift) && drift >= 0.0, "forces", "a noise's `drift` can't be negative")
            }
            Self::Vortex { axis, strength, pull } => {
                let axis = Vec3::from(axis);
                require(
                    axis.is_finite() && axis.length_squared() > 1e-8,
                    "forces",
                    "a vortex's `axis` must be finite and not zero",
                )?;
                require(
                    finite(strength) && finite(pull),
                    "forces",
                    "a vortex's `strength` and `pull` must be finite",
                )
            }
            Self::Attract { point, strength, kill_radius } => {
                require(Vec3::from(point).is_finite(), "forces", "an attractor's `point` must be finite")?;
                require(finite(strength), "forces", "an attractor's `strength` must be finite")?;
                require(
                    finite(kill_radius) && kill_radius >= 0.0,
                    "forces",
                    "an attractor's `kill_radius` can't be negative",
                )
            }
            Self::LimitSpeed { max } => {
                require(finite(max) && max > 0.0, "forces", "a speed limit's `max` must be above 0")
            }
        }
    }

    /// Whether it draws the emitter's own random numbers when its effect starts: a noise's offset.
    pub(crate) fn is_noise(&self) -> bool {
        matches!(self, Self::Noise { .. })
    }
}

impl Ground {
    pub(crate) fn problem(&self) -> Result<(), Problem> {
        require(share_ok(self.bounce), "collide.bounce", "must be 0 to 1")?;
        require(share_ok(self.friction), "collide.friction", "must be 0 to 1")
    }
}

/// Where an emitter's noise starts in its lattice, from a stream of its own: `stream` forked by the emitter's
/// place, never advanced, so the effect's own numbers are as they'd be without it.
pub(crate) fn noise_offset(stream: &Rng, place: usize) -> Vec3 {
    let mut own = stream.fork(place as u64);
    Vec3::new(own.unit(), own.unit(), own.unit()) * NOISE_SPAN
}

/// A force as one step applies it, in the world, its step's length folded in.
#[derive(Clone, Copy, Debug, Default)]
enum Pulled {
    #[default]
    Nothing,
    /// Toward `target` by `share` of the gap.
    Wind {
        target: Vec3,
        share: f32,
    },
    /// `strength` × the step, at `scale` cells a meter from `offset`.
    Noise {
        strength: f32,
        scale: f32,
        octaves: u8,
        offset: Vec3,
    },
    Vortex {
        center: Vec3,
        axis: Vec3,
        around: f32,
        pull: f32,
    },
    Attract {
        center: Vec3,
        strength: f32,
        kill_squared: f32,
    },
    Limit {
        max: f32,
    },
}

/// The ground as one step meets it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Landing {
    pub(crate) height: f32,
    bounce: f32,
    /// The share of its speed along the ground a particle keeps at a touch.
    keep: f32,
    dies: bool,
}

/// One playing emitter's forces and ground, resolved for this step: what each of its particles runs through.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Resolved {
    count: u8,
    forces: [Pulled; MAX_FORCES],
    pub(crate) ground: Option<Landing>,
}

/// The steps a second over which [`Ground::friction`] takes its share once each: friction is a rate, so a
/// slowed owner's particles slide as far in its time as a full-speed owner's do in theirs.
const FRICTION_STEPS_PER_SECOND: f32 = 60.0;

/// What a step resolves an emitter's forces from, beside the emitter itself.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Here {
    pub(crate) position: Vec3,
    pub(crate) rotation: Quat,
    /// The owner's ground.
    pub(crate) ground: f32,
    /// The effect's age, which moves a drifting noise.
    pub(crate) age: f32,
    /// This step's length on the owner's clock.
    pub(crate) step: f32,
    /// The step a noise integrates over: the step, 0 on a step it skips, or two steps on the one after
    /// ([`VfxBudget::noise_on_alternate_steps`](crate::VfxBudget::noise_on_alternate_steps)).
    pub(crate) noise_step: f32,
    pub(crate) wind: WindSample,
}

impl Resolved {
    /// Nothing: an emitter without forces or a ground, whose particles move as they always have.
    pub(crate) const NONE: Self = Self { count: 0, forces: [Pulled::Nothing; MAX_FORCES], ground: None };

    /// `forces` and `ground` as this step applies them, `here`; `noise_offset` the emitter's.
    pub(crate) fn new(forces: &[Force], ground: Option<&Ground>, noise_offset: Vec3, here: &Here) -> Self {
        let mut resolved = Self::NONE;
        let step = here.step;
        for force in forces.iter().take(MAX_FORCES) {
            let pulled = match *force {
                Force::Wind { response } => {
                    Pulled::Wind { target: here.wind.velocity, share: 1.0 - (-response * step).exp() }
                }
                Force::Noise { strength, frequency, octaves, drift } => {
                    if here.noise_step <= 0.0 {
                        continue;
                    }
                    let drifted = noise_offset + Vec3::new(0.0, -drift, drift * 0.5) * here.age;
                    Pulled::Noise {
                        strength: strength * here.noise_step,
                        scale: frequency,
                        octaves,
                        offset: drifted,
                    }
                }
                Force::Vortex { axis, strength, pull } => Pulled::Vortex {
                    center: here.position,
                    axis: (here.rotation * Vec3::from(axis)).normalize_or(Vec3::Y),
                    around: strength * step,
                    pull: pull * step,
                },
                Force::Attract { point, strength, kill_radius } => Pulled::Attract {
                    center: here.position + here.rotation * Vec3::from(point),
                    strength: strength * step,
                    kill_squared: kill_radius * kill_radius,
                },
                Force::LimitSpeed { max } => Pulled::Limit { max },
            };
            resolved.forces[usize::from(resolved.count)] = pulled;
            resolved.count += 1;
        }
        resolved.ground = ground.map(|ground| Landing {
            height: here.ground,
            bounce: ground.bounce,
            keep: (1.0 - ground.friction).powf(here.step * FRICTION_STEPS_PER_SECOND),
            dies: ground.dies,
        });
        resolved
    }

    /// Applies its forces to a particle at `position` moving at `velocity`. False where one ended it.
    pub(crate) fn pull(&self, position: Vec3, velocity: &mut Vec3) -> bool {
        for pulled in &self.forces[..usize::from(self.count)] {
            match *pulled {
                Pulled::Nothing => {}
                Pulled::Wind { target, share } => *velocity += (target - *velocity) * share,
                Pulled::Noise { strength, scale, octaves, offset } => {
                    *velocity += curl(position * scale + offset, octaves) * strength;
                }
                Pulled::Vortex { center, axis, around, pull } => {
                    let reach = position - center;
                    let across = reach - axis * axis.dot(reach);
                    let distance = across.length();
                    if distance > 1e-6 {
                        let outward = across / distance;
                        *velocity += axis.cross(outward) * around - outward * pull;
                    }
                }
                Pulled::Attract { center, strength, kill_squared } => {
                    let toward = center - position;
                    let distance_squared = toward.length_squared();
                    if distance_squared <= kill_squared {
                        return false;
                    }
                    if distance_squared > 1e-12 {
                        *velocity += toward * (strength / distance_squared.sqrt());
                    }
                }
                Pulled::Limit { max } => {
                    let speed_squared = velocity.length_squared();
                    if speed_squared > max * max {
                        *velocity *= max / speed_squared.sqrt();
                    }
                }
            }
        }
        true
    }
}

impl Landing {
    /// Meets a particle that moved to `position` at `velocity` with the ground. False where it ends there.
    pub(crate) fn meet(&self, position: &mut Vec3, velocity: &mut Vec3) -> bool {
        let depth = self.height - position.y;
        if depth <= 0.0 {
            return true;
        }
        if self.dies {
            return false;
        }
        if velocity.y < 0.0 {
            velocity.y = -velocity.y * self.bounce;
        }
        velocity.x *= self.keep;
        velocity.z *= self.keep;
        // Out of the ground by the share of the way in that it bounces.
        position.y = self.height + depth * self.bounce;
        true
    }
}

/// The curl of a vector potential whose three parts are hashed values on one lattice, summed over `octaves`:
/// a divergence-free swirl, about 1 at its root mean square an octave. Integer hashing and plain arithmetic,
/// so it's the same on every machine.
pub(crate) fn curl(point: Vec3, octaves: u8) -> Vec3 {
    let mut sum = curl_octave(point);
    if octaves >= 2 {
        // Twice as fine, half as strong, from a part of the lattice the first octave never shares.
        sum += curl_octave(point * 2.0 + Vec3::new(31.7, 11.3, 23.9)) * 0.5;
    }
    sum * CURL_SCALE
}

/// One octave: the eight corners round `point` give each of the potential's three parts a value (three from
/// one hash), blended by smoothstep, whose derivatives make the curl.
fn curl_octave(point: Vec3) -> Vec3 {
    let cell = point.floor();
    let f = point - cell;
    let (x, y, z) = (cell.x as i32, cell.y as i32, cell.z as i32);
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let du = 6.0 * f * (Vec3::ONE - f);
    let corner = |dx: i32, dy: i32, dz: i32| {
        let hash = lattice_hash(x.wrapping_add(dx), y.wrapping_add(dy), z.wrapping_add(dz));
        let part = |shift: u32| ((hash >> shift) & 0x3FF) as f32 * (2.0 / 1023.0) - 1.0;
        Vec3::new(part(0), part(10), part(20))
    };
    let (c000, c100, c010, c110) = (corner(0, 0, 0), corner(1, 0, 0), corner(0, 1, 0), corner(1, 1, 0));
    let (c001, c101, c011, c111) = (corner(0, 0, 1), corner(1, 0, 1), corner(0, 1, 1), corner(1, 1, 1));
    // Each part's derivative along x, y and z, all three parts at once.
    let along_x = (c100 - c000).lerp(c110 - c010, u.y).lerp((c101 - c001).lerp(c111 - c011, u.y), u.z) * du.x;
    let along_y = (c010 - c000).lerp(c110 - c100, u.x).lerp((c011 - c001).lerp(c111 - c101, u.x), u.z) * du.y;
    let along_z = (c001 - c000).lerp(c101 - c100, u.x).lerp((c011 - c010).lerp(c111 - c110, u.x), u.y) * du.z;
    Vec3::new(along_y.z - along_z.y, along_z.x - along_x.z, along_x.y - along_y.x)
}

/// A lattice point's 32 well-mixed bits.
fn lattice_hash(x: i32, y: i32, z: i32) -> u32 {
    let mut hash = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD816_3841)
        ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x297A_2D39);
    hash ^ (hash >> 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The potential one octave's curl is taken of, by plain trilinear blending: what the curl's
    /// derivatives must agree with.
    fn potential(point: Vec3) -> Vec3 {
        let cell = point.floor();
        let f = point - cell;
        let (x, y, z) = (cell.x as i32, cell.y as i32, cell.z as i32);
        let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
        let corner = |dx: i32, dy: i32, dz: i32| {
            let hash = lattice_hash(x + dx, y + dy, z + dz);
            let part = |shift: u32| ((hash >> shift) & 0x3FF) as f32 * (2.0 / 1023.0) - 1.0;
            Vec3::new(part(0), part(10), part(20))
        };
        let near =
            corner(0, 0, 0).lerp(corner(1, 0, 0), u.x).lerp(corner(0, 1, 0).lerp(corner(1, 1, 0), u.x), u.y);
        let far =
            corner(0, 0, 1).lerp(corner(1, 0, 1), u.x).lerp(corner(0, 1, 1).lerp(corner(1, 1, 1), u.x), u.y);
        near.lerp(far, u.z)
    }

    fn points() -> impl Iterator<Item = Vec3> {
        let mut rng = Rng::new(11, 3);
        (0..4096).map(move |_| Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 40.0 - Vec3::splat(20.0))
    }

    /// The curl is the potential's, by its derivatives: a central difference of the potential agrees with it.
    #[test]
    fn a_curl_is_its_potentials() {
        let h = 1e-3;
        for point in points().take(512) {
            let d = |axis: Vec3| (potential(point + axis * h) - potential(point - axis * h)) / (2.0 * h);
            let (dx, dy, dz) = (d(Vec3::X), d(Vec3::Y), d(Vec3::Z));
            let expected = Vec3::new(dy.z - dz.y, dz.x - dx.z, dx.y - dy.x);
            let error = (curl_octave(point) - expected).length();
            assert!(error < 2e-2, "{point}: {error}");
        }
    }

    /// One octave's acceleration is about 1 at its root mean square, so `strength` is what an author sees, and
    /// it neither gathers nor spreads: its divergence, by central differences, is next to nothing beside it.
    #[test]
    fn a_noises_strength_is_its_typical_acceleration() {
        let (mut squares, mut count) = (0.0_f64, 0.0_f64);
        let h = 1e-3;
        let mut worst_divergence = 0.0_f32;
        for point in points() {
            squares += f64::from(curl(point, 1).length_squared());
            count += 1.0;
            let d = |axis: Vec3| (curl(point + axis * h, 1) - curl(point - axis * h, 1)) / (2.0 * h);
            worst_divergence = worst_divergence.max((d(Vec3::X).x + d(Vec3::Y).y + d(Vec3::Z).z).abs());
        }
        let rms = (squares / count).sqrt();
        assert!((0.9..1.1).contains(&rms), "{rms}");
        assert!(worst_divergence < 0.1, "{worst_divergence}");
    }

    #[test]
    fn a_noise_is_the_same_wherever_it_is_asked_and_differs_between_places() {
        let point = Vec3::new(3.25, -7.5, 12.125);
        assert_eq!(curl(point, 2), curl(point, 2));
        assert_ne!(curl(point, 1), curl(point, 2));
        assert_ne!(curl(point, 1), curl(point + Vec3::X, 1));
    }

    #[test]
    fn a_ground_bounces_by_its_share_and_slides_by_its_friction() {
        let ground = Landing { height: 1.0, bounce: 0.5, keep: 0.75, dies: false };
        let (mut position, mut velocity) = (Vec3::new(0.0, 0.8, 0.0), Vec3::new(2.0, -4.0, 0.0));
        assert!(ground.meet(&mut position, &mut velocity));
        assert!((position.y - 1.1).abs() < 1e-6, "{position}");
        assert_eq!(velocity, Vec3::new(1.5, 2.0, 0.0));
        let (mut above, mut moving) = (Vec3::new(0.0, 1.5, 0.0), Vec3::new(2.0, -4.0, 0.0));
        assert!(ground.meet(&mut above, &mut moving));
        assert_eq!(
            (above, moving),
            (Vec3::new(0.0, 1.5, 0.0), Vec3::new(2.0, -4.0, 0.0)),
            "above it, untouched"
        );
        let deadly = Landing { dies: true, ..ground };
        let (mut under, mut falling) = (Vec3::new(0.0, 0.9, 0.0), Vec3::NEG_Y);
        assert!(!deadly.meet(&mut under, &mut falling));
    }
}
