//! The wind as the frame draws it (M1 W6, M6 Y1): one field every reader samples, foliage, and later
//! precipitation, effects, water, mist, springs and cloth (m6-plan §3, "One wind and one weather
//! state"). Plain data, resolved outside the renderer each frame (rule R4), and one sampler, the same
//! maths on the CPU ([`WindField::sample`]) and on the GPU (the renderer's `wind.wgsl`), operation for
//! operation.
//!
//! The field is three layers of air, each a velocity in the global wind's units (1 is the strength
//! foliage's responses are authored at):
//!
//! - **The global wind** ([`Wind`]): a direction, a strength and gusts, slow swells that travel
//!   downwind across the world ([`WindState::gust`]): the wind foliage has swayed in since M1.
//! - **Turbulence** ([`WindField::turbulence`]): a share of the global strength, eddies about 24 m
//!   and 9 m across that turn and drift in time ([`WindState::turbulence`]), four plane waves of a
//!   stream function, so the air swirls without a source or a sink. 0, the default, is the global
//!   wind exactly.
//! - **Volumes** ([`WindVolume`]): at most [`MAX_WIND_VOLUMES`] spheres of local air, a draught
//!   (directional), a vortex about an axis or a burst outward from a centre, each fading to nothing at
//!   its radius. A tier reads fewer: the renderer's `WindSettings` say how many its GPU readers loop
//!   over and how many its CPU readers take, and [`WindField::choose`] keeps the strongest near the
//!   camera.
//!
//! Time is the packet's presentation time, wrapped to [`WIND_PERIOD`] in `f64` before it reaches
//! `f32`; every oscillator runs a whole number of cycles in the period ([`cycles`]), so the wrap never
//! jumps and long sessions keep their precision. Nothing here enters the simulation: no replay, no
//! checksum and no zone digest reads the wind.
//!
//! The resolved form ([`ResolvedWind`], [`WindState`], [`WindVolumeState`]) is what the GPU reads, byte
//! for byte, and what the CPU sampler reads too, so the two can only differ in the last bits of their
//! `sin`, `cos` and `sqrt`.

use std::f32::consts::TAU;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// The wind's period in seconds: time reaches the samplers wrapped to it, and every oscillator runs a
/// whole number of cycles in it (`wind.wgsl`'s `WIND_PERIOD`).
pub const WIND_PERIOD: f64 = 1024.0;

/// How often gust fronts pass, in cycles per second before rounding to whole cycles per period
/// (`wind.wgsl`'s `WIND_GUST_HZ`).
pub const GUST_HZ: f32 = 0.09;

/// Meters between gust fronts traveling downwind (`wind.wgsl`'s `WIND_GUST_WAVELENGTH`).
pub const GUST_WAVELENGTH: f32 = 36.0;

/// Turbulence's four waves (`wind.wgsl`'s `WIND_TURBULENCE`): xy the wave's number in cycles per meter
/// of x and z, two about 24 m long and two about 9 m, at four headings; zw the air's flow along its
/// crests, at right angles to the wave, its length the wave's share (0.35, 0.35, 0.15 and 0.15, which
/// add to 1, so turbulence's air is at most the global strength times [`WindField::turbulence`]).
pub const TURBULENCE_WAVES: [[f32; 4]; 4] = [
    [0.0336, 0.0252, 0.21, -0.28],
    [-0.0117, 0.04, 0.336, 0.098],
    [-0.0667, -0.0889, -0.12, 0.09],
    [0.1067, -0.0311, -0.042, -0.144],
];

/// How fast each of [`TURBULENCE_WAVES`] turns over, in cycles per second before rounding to whole
/// cycles per period (`wind.wgsl`'s `WIND_TURBULENCE_HZ`).
pub const TURBULENCE_HZ: [f32; 4] = [0.05, 0.07, 0.13, 0.17];

/// The most volumes a field holds, and the most any tier's readers take.
pub const MAX_WIND_VOLUMES: usize = 8;

/// The least radius a volume resolves with, in meters: a smaller one is this.
pub const MIN_VOLUME_RADIUS: f32 = 0.01;

/// The least falloff a volume resolves with: its edge fades over at least this share of its radius,
/// so its air stays continuous.
pub const MIN_VOLUME_FALLOFF: f32 = 1e-3;

/// What a vortex's axis is held above when the samplers divide by its length squared: a burst's axis
/// is zero (`wind.wgsl`'s `WIND_AXIS_FLOOR`).
pub const AXIS_FLOOR: f32 = 1e-30;

/// The global wind (M1 W6): what a zone, a day's keys or a tool set, and what foliage has swayed in
/// since. Only its horizontal direction counts; with none the air is calm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wind {
    /// The direction the wind blows toward. Only its horizontal part counts; with none the air
    /// is calm.
    pub direction: Vec3,
    /// 0 is calm and 1 the strength materials' responses are authored at; a gale is a few.
    pub strength: f32,
    /// How much gusts swell and ease the strength as they travel downwind, 0 (steady) to 1
    /// (from calm to double).
    pub gustiness: f32,
}

impl Default for Wind {
    /// Calm: foliage stands still.
    fn default() -> Self {
        Self::CALM
    }
}

impl Wind {
    /// No wind at all.
    pub const CALM: Self = Self { direction: Vec3::X, strength: 0.0, gustiness: 0.0 };
}

/// A sphere of local air (M6 Y1): a draught through a doorway or a canyon, a vortex, a burst from a
/// blast. Its air is
///
/// `shape(d) × (velocity + (swirl × across + across × radial) / radius)`,
///
/// `across` the offset from its centre less its part along the swirl's axis (all of it for a burst,
/// whose axis is zero), and `shape` 1 inside `radius × (1 - falloff)`, easing (a smoothstep) to 0 at
/// the radius. So a draught blows `velocity` everywhere inside it, a vortex turns about its axis faster
/// away from it, as a solid body does, and a burst blows outward faster away from its centre, each
/// fading out at its edge. One formula for every kind, so the samplers never branch on one.
///
/// Who owns a volume's life is the volume's source's: a burst's owner fades its strength over its
/// life, a zone's draught streams with its cell. The field holds what's alive this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindVolume {
    pub center: Vec3,
    /// Meters; at least [`MIN_VOLUME_RADIUS`] when resolved.
    pub radius: f32,
    /// The share of the radius its edge fades over, 0 (hard) to 1 (from the centre); at least
    /// [`MIN_VOLUME_FALLOFF`] when resolved.
    pub falloff: f32,
    /// The air it blows everywhere inside it.
    pub velocity: Vec3,
    /// Its axis times its turning speed at its radius: the right-hand rule's turn. Zero for none.
    pub swirl: Vec3,
    /// How fast the air leaves its axis (or its centre, with no swirl) at its radius: negative flows
    /// in.
    pub radial: f32,
}

impl WindVolume {
    /// A draught blowing `velocity` through the sphere.
    #[must_use]
    pub const fn directional(center: Vec3, radius: f32, velocity: Vec3) -> Self {
        Self { center, radius, falloff: 0.5, velocity, swirl: Vec3::ZERO, radial: 0.0 }
    }

    /// A vortex about `axis` through `center`, turning at `speed` at its radius (by the right-hand
    /// rule about the axis; negative turns the other way) and drawing air in toward its axis at
    /// `inflow`. A zero axis makes it a sink or a source about its centre.
    #[must_use]
    pub fn vortex(center: Vec3, radius: f32, axis: Vec3, speed: f32, inflow: f32) -> Self {
        let swirl = axis.normalize_or_zero() * speed;
        Self { center, radius, falloff: 0.5, velocity: Vec3::ZERO, swirl, radial: -inflow }
    }

    /// A burst blowing outward from `center`, at `speed` at its radius.
    #[must_use]
    pub const fn burst(center: Vec3, radius: f32, speed: f32) -> Self {
        Self { center, radius, falloff: 0.5, velocity: Vec3::ZERO, swirl: Vec3::ZERO, radial: speed }
    }

    /// The same volume fading over `falloff` of its radius.
    #[must_use]
    pub const fn with_falloff(self, falloff: f32) -> Self {
        Self { falloff, ..self }
    }

    /// The fastest air it adds to a sample anywhere: its velocity, swirl and radial speeds together,
    /// which its shape only lessens. 0 for a volume that resolves to nothing.
    #[must_use]
    pub fn reach(&self) -> f32 {
        self.state().map_or(0.0, |_| self.velocity.length() + self.swirl.length() + self.radial.abs())
    }

    /// How much it matters to a reader at `camera`: its reach, less with distance in radii. The field
    /// keeps the volumes with the most ([`WindField::choose`]).
    fn priority(&self, camera: Vec3) -> f32 {
        let radius = self.radius.max(MIN_VOLUME_RADIUS);
        self.reach() / (1.0 + self.center.distance(camera) / radius)
    }

    /// As the samplers read it: its radius and falloff held to their least, or nothing if any number
    /// is not finite.
    fn state(&self) -> Option<WindVolumeState> {
        let numbers = [self.radius, self.falloff, self.radial];
        let finite = numbers.iter().all(|n| n.is_finite())
            && self.center.is_finite()
            && self.velocity.is_finite()
            && self.swirl.is_finite();
        finite.then(|| WindVolumeState {
            center: self.center.extend(self.radius.max(MIN_VOLUME_RADIUS)).to_array(),
            velocity: self.velocity.extend(self.falloff.clamp(MIN_VOLUME_FALLOFF, 1.0)).to_array(),
            swirl: self.swirl.extend(self.radial).to_array(),
        })
    }
}

/// An empty slot of [`WindField`]'s: what its volumes past its count hold.
const NO_VOLUME: WindVolume = WindVolume::directional(Vec3::ZERO, 0.0, Vec3::ZERO);

/// The air of one frame (M6 Y1, see the [module docs](self)): the global wind, turbulence as a share
/// of its strength, and the volumes alive this frame, strongest first. The default is calm, which
/// draws as the world drew before the field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindField {
    pub global: Wind,
    /// Turbulence's air as a share of the global strength, 0 (none, the default) to 1.
    pub turbulence: f32,
    volumes: [WindVolume; MAX_WIND_VOLUMES],
    count: u8,
}

impl Default for WindField {
    fn default() -> Self {
        Self::CALM
    }
}

impl From<Wind> for WindField {
    fn from(global: Wind) -> Self {
        Self::new(global)
    }
}

impl WindField {
    /// No air at all.
    pub const CALM: Self = Self::new(Wind::CALM);

    /// `global` alone: no turbulence and no volumes.
    #[must_use]
    pub const fn new(global: Wind) -> Self {
        Self { global, turbulence: 0.0, volumes: [NO_VOLUME; MAX_WIND_VOLUMES], count: 0 }
    }

    /// Its volumes, strongest first as [`Self::choose`] keeps them.
    #[must_use]
    pub fn volumes(&self) -> &[WindVolume] {
        &self.volumes[..usize::from(self.count)]
    }

    /// Adds `volume` after the others, returning whether there was room ([`MAX_WIND_VOLUMES`]).
    pub fn push(&mut self, volume: WindVolume) -> bool {
        let Some(slot) = self.volumes.get_mut(usize::from(self.count)) else { return false };
        *slot = volume;
        self.count += 1;
        true
    }

    /// Keeps the first `count` volumes, and clears the rest's slots, so two fields with the same
    /// volumes compare and print alike.
    pub fn truncate(&mut self, count: usize) {
        self.count = self.count.min(count.min(MAX_WIND_VOLUMES) as u8);
        self.volumes[usize::from(self.count)..].fill(NO_VOLUME);
    }

    /// Replaces its volumes with the `cap` of `candidates` (at most [`MAX_WIND_VOLUMES`]) that matter
    /// most to a reader at `camera`, strongest first: by reach over one plus the distance in radii,
    /// ties in the candidates' order. Returns how many it left out. Allocates nothing.
    pub fn choose(&mut self, candidates: &[WindVolume], camera: Vec3, cap: usize) -> usize {
        let cap = cap.min(MAX_WIND_VOLUMES);
        let mut kept = [(0.0f32, 0usize); MAX_WIND_VOLUMES];
        let mut count = 0;
        for (index, volume) in candidates.iter().enumerate() {
            let priority = volume.priority(camera);
            if priority.is_nan() || priority <= 0.0 {
                continue;
            }
            // Past every kept one with at least this priority, so ties keep their order.
            let at = kept[..count].iter().position(|&(other, _)| priority > other).unwrap_or(count);
            if at >= cap {
                continue;
            }
            let end = (count + 1).min(cap);
            kept.copy_within(at..end - 1, at + 1);
            kept[at] = (priority, index);
            count = end;
        }
        for (slot, &(_, index)) in self.volumes.iter_mut().zip(&kept[..count]) {
            *slot = candidates[index];
        }
        self.volumes[count..].fill(NO_VOLUME);
        self.count = count as u8;
        candidates.len() - count
    }

    /// How fast the air beyond the global wind can be anywhere for a reader that takes `cap`
    /// volumes: turbulence's most and the reach of the first `cap` that resolve, as the samplers
    /// take them ([`ResolvedWind::local`]). Foliage's culling pads its bounds by it on tiers that
    /// draw local air.
    #[must_use]
    pub fn local_reach(&self, cap: usize) -> f32 {
        let state = WindState::new(&self.global, 0.0);
        let turbulence = state.direction[3] * finite_share(self.turbulence);
        self.volumes()
            .iter()
            .filter(|volume| volume.state().is_some())
            .take(cap)
            .map(WindVolume::reach)
            .fold(turbulence, |sum, reach| sum + reach)
    }

    /// The field at `time_seconds` as the samplers read it: the global wind's state, turbulence and
    /// the volumes that resolve, each finite and in its range.
    #[must_use]
    pub fn resolve(&self, time_seconds: f64) -> ResolvedWind {
        let mut resolved =
            ResolvedWind { state: WindState::new(&self.global, time_seconds), ..ResolvedWind::CALM };
        let mut count = 0;
        for state in self.volumes().iter().filter_map(WindVolume::state) {
            resolved.volumes[count] = state;
            count += 1;
        }
        resolved.state.time[2] = finite_share(self.turbulence);
        resolved.state.time[3] = count as f32;
        resolved
    }

    /// The air's velocity at `position` at `time_seconds`, every layer and every volume: what a CPU
    /// reader (springs, effects, audio) takes, and what the GPU's `wind_air` gives with every volume.
    #[must_use]
    pub fn sample(&self, position: Vec3, time_seconds: f64) -> Vec3 {
        self.resolve(time_seconds).air(position, MAX_WIND_VOLUMES)
    }
}

/// `value` held to 0 to 1, a NaN counting as 0.
fn finite_share(value: f32) -> f32 {
    if value > 0.0 { value.min(1.0) } else { 0.0 }
}

/// The global wind and turbulence as the samplers read them (WGSL `WindState`, 32 bytes): the shape
/// foliage's wind had before the field, with turbulence and the count of volumes in its spare lanes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WindState {
    /// xyz: the unit horizontal direction the wind blows toward; w: its strength (0 calm).
    pub direction: [f32; 4],
    /// x: time in seconds, wrapped to [`WIND_PERIOD`]; y: gustiness, 0 to 1; z: turbulence, 0 to 1;
    /// w: how many of the volumes beside it count, as a float.
    pub time: [f32; 4],
}

impl WindState {
    /// `wind` at `time_seconds`: its direction made horizontal and unit (calm without one), its
    /// strength at least 0, its gustiness within 0..=1 and the time wrapped, each finite; no
    /// turbulence and no volumes.
    #[must_use]
    pub fn new(wind: &Wind, time_seconds: f64) -> Self {
        let horizontal = Vec3::new(wind.direction.x, 0.0, wind.direction.z);
        let direction = horizontal.try_normalize().unwrap_or(Vec3::ZERO);
        let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
        let strength = if direction == Vec3::ZERO { 0.0 } else { finite(wind.strength).max(0.0) };
        let time = if time_seconds.is_finite() { time_seconds.rem_euclid(WIND_PERIOD) as f32 } else { 0.0 };
        Self {
            direction: direction.extend(strength).to_array(),
            time: [time, finite(wind.gustiness).clamp(0.0, 1.0), 0.0, 0.0],
        }
    }

    /// The horizontal unit direction the wind blows toward, zero when calm.
    #[must_use]
    pub fn direction(&self) -> Vec3 {
        let [x, _, z, _] = self.direction;
        Vec3::new(x, 0.0, z)
    }

    /// The strength, at least 0.
    #[must_use]
    pub fn strength(&self) -> f32 {
        self.direction[3]
    }

    /// How many volumes beside it count.
    #[must_use]
    pub fn volumes(&self) -> usize {
        (self.time[3] as usize).min(MAX_WIND_VOLUMES)
    }

    /// Whether there is air beyond the global wind for a reader taking at most `cap` volumes:
    /// turbulence, or a live volume (`wind.wgsl`'s `wind_has_local`). A reader may skip the local
    /// air without it, as foliage's vertex stages do, so a frame without it draws as before the
    /// field.
    #[must_use]
    pub fn has_local(&self, cap: usize) -> bool {
        self.time[2] > 0.0 || (cap > 0 && self.time[3] > 0.0)
    }

    /// How much gusts swell the strength at `position`, 1 - gustiness to 1 + gustiness: a slow swell
    /// traveling downwind, so a field ripples as fronts pass (`wind.wgsl`'s `wind_gust`).
    #[must_use]
    pub fn gust(&self, position: Vec3) -> f32 {
        let [x, _, z, _] = self.direction;
        let [t, gustiness, ..] = self.time;
        let downwind = (position.x * x + position.z * z) / GUST_WAVELENGTH;
        1.0 + gustiness * (TAU * fract(cycles(GUST_HZ, t) - downwind)).sin()
    }

    /// Turbulence's air at `position`, horizontal: the strength times the share times the four waves'
    /// flows (`wind.wgsl`'s `wind_turbulence`). Exactly zero (of either sign) with no turbulence or no
    /// wind.
    #[must_use]
    pub fn turbulence(&self, position: Vec3) -> Vec3 {
        let t = self.time[0];
        let (mut x, mut z) = (0.0f32, 0.0f32);
        for (wave, hz) in TURBULENCE_WAVES.iter().zip(TURBULENCE_HZ) {
            let along = (TAU * fract(position.x * wave[0] + position.z * wave[1] + cycles(hz, t))).cos();
            x += wave[2] * along;
            z += wave[3] * along;
        }
        let scale = self.direction[3] * self.time[2];
        Vec3::new(x, 0.0, z) * scale
    }
}

/// One volume as the samplers read it (WGSL `WindVolume`, 48 bytes; see [`WindVolume`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WindVolumeState {
    /// xyz: its centre; w: its radius.
    pub center: [f32; 4],
    /// xyz: the air it blows everywhere; w: its falloff.
    pub velocity: [f32; 4],
    /// xyz: its axis times its turning speed; w: its radial speed.
    pub swirl: [f32; 4],
}

impl WindVolumeState {
    /// Its air at `position` (`wind.wgsl`'s `wind_volume_air`).
    #[must_use]
    pub fn air(&self, position: Vec3) -> Vec3 {
        let [cx, cy, cz, radius] = self.center;
        let [vx, vy, vz, falloff] = self.velocity;
        let [sx, sy, sz, radial] = self.swirl;
        let r = position - Vec3::new(cx, cy, cz);
        let axis = Vec3::new(sx, sy, sz);
        let across = r - axis * (r.dot(axis) / axis.dot(axis).max(AXIS_FLOOR));
        let x = r.dot(r).sqrt() / radius;
        let edge = ((1.0 - x) / falloff).clamp(0.0, 1.0);
        let shape = edge * edge * (3.0 - 2.0 * edge);
        (Vec3::new(vx, vy, vz) + (axis.cross(across) + across * radial) / radius) * shape
    }
}

/// The field at one instant as the samplers read it, on the CPU and the GPU (416 bytes): the global
/// wind's [`WindState`] and the volumes, of which its count are live.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ResolvedWind {
    pub state: WindState,
    pub volumes: [WindVolumeState; MAX_WIND_VOLUMES],
}

impl ResolvedWind {
    /// No air at all.
    pub const CALM: Self = Self {
        state: WindState { direction: [0.0; 4], time: [0.0; 4] },
        volumes: [WindVolumeState { center: [0.0; 4], velocity: [0.0; 4], swirl: [0.0; 4] };
            MAX_WIND_VOLUMES],
    };

    /// The air beyond the global wind at `position`: turbulence, then the first live volumes, at most
    /// `cap` of them, as a GPU reader built for `cap` loops over them (`wind.wgsl`'s `wind_local`).
    /// Exactly zero (of either sign) with no turbulence and no volume reaching `position`.
    #[must_use]
    pub fn local(&self, position: Vec3, cap: usize) -> Vec3 {
        let count = self.state.volumes().min(cap);
        self.volumes[..count]
            .iter()
            .fold(self.state.turbulence(position), |air, volume| air + volume.air(position))
    }

    /// The air's velocity at `position`: the global wind's direction times its gusted strength, plus
    /// [`Self::local`] (`wind.wgsl`'s `wind_air`).
    #[must_use]
    pub fn air(&self, position: Vec3, cap: usize) -> Vec3 {
        let state = &self.state;
        state.direction() * (state.strength() * state.gust(position)) + self.local(position, cap)
    }
}

/// `x - floor(x)`, as WGSL's `fract`.
#[must_use]
pub fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Cycles of an oscillator at `hz` at wrapped time `t`, in [0, 1): the frequency rounded to a whole
/// number of cycles per [`WIND_PERIOD`], so the wrap changes nothing. Ties round to even, as WGSL's
/// `round` does (`wind.wgsl`'s `wind_cycles`).
#[must_use]
pub fn cycles(hz: f32, t: f32) -> f32 {
    let period = WIND_PERIOD as f32;
    fract((hz * period).round_ties_even() / period * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breeze() -> Wind {
        Wind { direction: Vec3::new(3.0, 5.0, 4.0), strength: 1.0, gustiness: 0.3 }
    }

    #[test]
    fn the_state_is_horizontal_unit_and_wrapped() {
        let state = WindState::new(&breeze(), WIND_PERIOD * 3.0 + 12.5);
        assert_eq!(state.direction, [0.6, 0.0, 0.8, 1.0]);
        assert_eq!(state.time, [12.5, 0.3, 0.0, 0.0]);
        // Straight up, or no direction at all, is calm; nonsense is tamed.
        let up = Wind { direction: Vec3::Y, ..breeze() };
        assert_eq!(WindState::new(&up, 1.0).direction, [0.0; 4]);
        let wild = Wind { direction: Vec3::X, strength: f32::NAN, gustiness: 7.0 };
        let state = WindState::new(&wild, f64::INFINITY);
        assert_eq!((state.direction[3], state.time[0], state.time[1]), (0.0, 0.0, 1.0));
        assert_eq!(WindState::new(&Wind::default(), 5.0).direction[3], 0.0, "the default is calm");
    }

    #[test]
    fn frequencies_round_ties_to_even_as_wgsl_does() {
        // 2.5 cycles a period rounds to 2, as WGSL's `round` does, not to 3.
        let hz = 2.5 / WIND_PERIOD as f32;
        assert_eq!(cycles(hz, 1.0), 2.0 / WIND_PERIOD as f32);
    }

    #[test]
    fn the_global_wind_alone_is_the_gusted_breeze() {
        let field = WindField::new(breeze());
        let at = Vec3::new(4.0, 1.0, -3.0);
        let state = WindState::new(&breeze(), 17.0);
        assert_eq!(field.sample(at, 17.0), state.direction() * (state.strength() * state.gust(at)));
        // Nothing beyond it, exactly: no turbulence, no volumes.
        assert_eq!(field.resolve(17.0).local(at, MAX_WIND_VOLUMES).length(), 0.0);
        let gust = state.gust(at);
        assert!((0.7..=1.3).contains(&gust), "{gust}");
        assert_eq!(WindField::CALM.sample(at, 17.0), Vec3::ZERO);
    }

    #[test]
    fn turbulence_is_a_share_of_the_strength_and_horizontal() {
        let mut field = WindField::new(Wind { strength: 2.0, ..breeze() });
        field.turbulence = 0.5;
        let mut largest = 0.0f32;
        for step in 0..500 {
            let at = Vec3::new(step as f32 * 0.73 - 180.0, 3.0, step as f32 * -0.41 + 50.0);
            let air = field.resolve(f64::from(step) * 0.37).local(at, 0);
            assert_eq!(air.y, 0.0);
            largest = largest.max(air.length());
        }
        // At most the strength times the share, and most of it somewhere.
        assert!(largest <= 1.0 + 1e-6 && largest > 0.4, "{largest}");
        assert!((field.local_reach(0) - 1.0).abs() < 1e-6);
        // Calm air has no turbulence, whatever the share.
        field.global = Wind::CALM;
        assert_eq!(field.sample(Vec3::new(5.0, 0.0, 9.0), 3.0), Vec3::ZERO);
    }

    #[test]
    fn each_kind_of_volume_blows_its_own_way_and_fades_at_its_edge() {
        let center = Vec3::new(10.0, 0.0, 0.0);
        let draught = WindVolume::directional(center, 4.0, Vec3::new(0.0, 0.0, 3.0)).state().unwrap();
        assert_eq!(draught.air(center), Vec3::new(0.0, 0.0, 3.0), "full inside");
        assert_eq!(draught.air(center + Vec3::X * 4.0), Vec3::ZERO, "nothing at the radius");
        assert_eq!(draught.air(center + Vec3::X * 9.0), Vec3::ZERO, "nor past it");
        let half = draught.air(center + Vec3::X * 3.0).z;
        assert!(half > 0.0 && half < 3.0, "{half}");

        // A vortex about +y turns by the right-hand rule, +x toward -z, and draws air in.
        let vortex = WindVolume::vortex(center, 4.0, Vec3::Y * 2.0, 2.0, 0.5).with_falloff(0.25);
        let air = vortex.state().unwrap().air(center + Vec3::new(2.0, 1.0, 0.0));
        assert!((air - Vec3::new(-0.25, 0.0, -1.0)).length() < 1e-6, "{air}");
        assert_eq!(vortex.state().unwrap().air(center + Vec3::Y), Vec3::ZERO, "still on its axis");

        // A burst blows out from its centre, in every direction.
        let burst = WindVolume::burst(center, 4.0, 6.0).with_falloff(0.25).state().unwrap();
        let air = burst.air(center + Vec3::new(0.0, 1.0, -1.0));
        assert!((air - Vec3::new(0.0, 1.5, -1.5)).length() < 1e-6, "{air}");
        assert!((vortex.reach() - 2.5).abs() < 1e-6 && WindVolume::burst(center, 4.0, -6.0).reach() == 6.0);
    }

    #[test]
    fn volumes_count_only_up_to_a_reader_s_cap() {
        let mut field = WindField::new(breeze());
        let at = Vec3::new(1.0, 0.0, 1.0);
        for _ in 0..3 {
            assert!(field.push(WindVolume::directional(at, 5.0, Vec3::X)));
        }
        let resolved = field.resolve(2.0);
        assert_eq!(resolved.state.volumes(), 3);
        assert_eq!(resolved.local(at, 8), Vec3::X * 3.0);
        assert_eq!(resolved.local(at, 2), Vec3::X * 2.0);
        assert_eq!(resolved.local(at, 0), Vec3::ZERO);
        while field.push(WindVolume::burst(at, 1.0, 1.0)) {}
        assert_eq!(field.volumes().len(), MAX_WIND_VOLUMES, "full at the most a field holds");
        field.truncate(4);
        assert_eq!(field.volumes().len(), 4);
        // What it dropped is gone from its slots too: equal volumes, equal fields.
        let mut four = WindField::new(breeze());
        for volume in field.volumes() {
            four.push(*volume);
        }
        assert_eq!(field, four);
        // The local reach counts as many volumes as a reader takes.
        let reach = |cap| field.local_reach(cap);
        assert_eq!((reach(0), reach(2), reach(8)), (0.0, 2.0, 2.0 + 2.0 * 1.0));
    }

    #[test]
    fn volumes_that_are_not_finite_never_reach_the_samplers() {
        let mut field = WindField::new(breeze());
        field.push(WindVolume::directional(Vec3::ZERO, f32::NAN, Vec3::X));
        field.push(WindVolume::directional(Vec3::ZERO, 0.0, Vec3::new(0.0, 0.0, f32::INFINITY)));
        field.push(WindVolume::directional(Vec3::ZERO, -3.0, Vec3::X).with_falloff(-1.0));
        field.turbulence = f32::NAN;
        let resolved = field.resolve(1.0);
        assert_eq!(resolved.state.volumes(), 1);
        assert_eq!(resolved.state.time[2], 0.0);
        // Nor its local reach: the one that resolves is the first a reader takes.
        assert_eq!(field.local_reach(1), 1.0);
        let kept = resolved.volumes[0];
        assert_eq!((kept.center[3], kept.velocity[3]), (MIN_VOLUME_RADIUS, MIN_VOLUME_FALLOFF));
        assert!(field.sample(Vec3::new(0.001, 0.0, 0.0), 1.0).is_finite());
    }

    #[test]
    fn choosing_keeps_the_strongest_near_the_camera_in_order() {
        let camera = Vec3::ZERO;
        let at = |x: f32, speed: f32| WindVolume::burst(Vec3::new(x, 0.0, 0.0), 10.0, speed);
        // Priorities: 1/(1+0)=1, 4/(1+3)=1, 9/(1+1)=4.5, 0 (left out), 2/(1+0.5)=1.33, 1/(1+0)=1.
        let candidates =
            [at(0.0, 1.0), at(30.0, 4.0), at(10.0, 9.0), at(0.0, 0.0), at(5.0, 2.0), at(0.0, 1.0)];
        let mut field = WindField::new(breeze());
        assert_eq!(field.choose(&candidates, camera, 3), 3);
        let radial: Vec<f32> = field.volumes().iter().map(|v| v.center.x).collect();
        assert_eq!(radial, [10.0, 5.0, 0.0], "strongest first, the first of a tie");
        assert_eq!(field.choose(&candidates, camera, 99), 1, "only the still one left out");
        let order: Vec<f32> = field.volumes().iter().map(|v| v.center.x + v.radial).collect();
        assert_eq!(order, [19.0, 7.0, 1.0, 34.0, 1.0]);
        assert_eq!(field.choose(&[], camera, 4), 0);
        assert!(field.volumes().is_empty());
        assert_eq!(field, WindField::new(breeze()), "no stale volume past the count");
    }

    #[test]
    fn the_field_is_continuous_across_the_period_s_wrap() {
        let mut field = WindField::new(breeze());
        field.turbulence = 1.0;
        field.push(WindVolume::vortex(Vec3::ZERO, 20.0, Vec3::Y, 3.0, 0.2));
        let at = Vec3::new(-7.0, 1.5, 2.0);
        let before = field.sample(at, WIND_PERIOD * 5.0 - 0.001);
        let after = field.sample(at, WIND_PERIOD * 5.0 + 0.001);
        assert!(before.distance(after) < 1e-3, "{before} to {after}");
        assert!(field.sample(at, 100.25).distance(field.sample(at, 100.25 + WIND_PERIOD)) < 1e-4);
    }

    #[test]
    fn the_resolved_layout_is_the_gpu_s() {
        assert_eq!(size_of::<WindState>(), 32);
        assert_eq!(size_of::<WindVolumeState>(), 48);
        assert_eq!(size_of::<ResolvedWind>(), 32 + 48 * MAX_WIND_VOLUMES);
        assert_eq!(ResolvedWind::CALM, ResolvedWind::default());
        // The waves' shares add to 1, so turbulence's reach is its share of the strength.
        let shares: f32 = TURBULENCE_WAVES.iter().map(|w| (w[2] * w[2] + w[3] * w[3]).sqrt()).sum();
        assert!((shares - 1.0).abs() < 1e-5, "{shares}");
    }
}
