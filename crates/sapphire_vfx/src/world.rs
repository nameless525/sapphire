//! [`VfxWorld`]: the effects playing, each on its owner's clock, and the frame they resolve to.

use std::f32::consts::TAU;

use glam::{Quat, Vec3};
use sapphire_look::{VfxFrame, VfxRibbonPoint, VfxSprite, VfxStats};

use crate::budget::VfxBudget;
use crate::document::{
    Effect, EffectId, EffectLibrary, EmitterDesc, Facing, MAX_EMITTERS, MAX_TRAIL_SAMPLES, Source, Spawn,
    TrailDesc,
};
use crate::rng::Rng;
use crate::trail::{Segment, TrailSample};

/// Whose clock an effect runs on: a plain key, an actor's stable ID in a game. Gameplay never reads
/// back through it (R3: effects read cues, never affect them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OwnerId(pub u64);

impl OwnerId {
    /// The world's own effects, which keep running through any character's hitstop.
    pub const WORLD: Self = Self(0);
}

/// This step's time per owner, in seconds of each owner's clock, as the caller read them (rule R2,
/// spec §4.5): 0 for an owner in hitstop, scaled for one in slow motion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OwnerSteps<'a> {
    /// The step of every owner not listed in [`Self::owners`], [`OwnerId::WORLD`] among them.
    pub world: f32,
    pub owners: &'a [(OwnerId, f32)],
}

impl OwnerSteps<'_> {
    /// `owner`'s step: never negative, 0 where it isn't finite.
    fn of(&self, owner: OwnerId) -> f32 {
        let step = self.owners.iter().find(|(id, _)| *id == owner).map_or(self.world, |(_, step)| *step);
        if step.is_finite() { step.max(0.0) } else { 0.0 }
    }
}

/// Where an effect plays, in the world: its emitters' sources and directions are relative to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub position: Vec3,
    pub rotation: Quat,
}

impl Placement {
    pub fn at(position: Vec3) -> Self {
        Self { position, rotation: Quat::IDENTITY }
    }
}

impl Default for Placement {
    fn default() -> Self {
        Self::at(Vec3::ZERO)
    }
}

/// An effect playing. Handles to an effect that ended do nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectHandle {
    slot: u16,
    generation: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct EmitterState {
    /// A burst has fired.
    fired: bool,
    /// Fractions of a particle a rate has owed since its last one.
    owed: f32,
}

/// An effect's place in the world.
#[derive(Clone, Debug)]
struct Slot {
    live: bool,
    generation: u32,
    effect: u16,
    owner: OwnerId,
    placement: Placement,
    rng: Rng,
    /// Seconds on the owner's clock since it spawned.
    age: f32,
    /// This step's time on the owner's clock.
    step: f32,
    /// No more emission or samples; it ends when what it holds has.
    stopped: bool,
    /// Live particles it spawned.
    particles: u32,
    emitters: [EmitterState; MAX_EMITTERS],
    /// Its place in the trails, if its effect has one and there was room.
    trail: Option<u16>,
}

impl Slot {
    fn new() -> Self {
        Self {
            live: false,
            generation: 0,
            effect: 0,
            owner: OwnerId::WORLD,
            placement: Placement::default(),
            rng: Rng::new(0, 0),
            age: 0.0,
            step: 0.0,
            stopped: false,
            particles: 0,
            emitters: [EmitterState::default(); MAX_EMITTERS],
            trail: None,
        }
    }
}

/// A trail's samples: a ring in the world's pool, [`MAX_TRAIL_SAMPLES`] long per trail, of which
/// its document uses `capacity`.
#[derive(Clone, Copy, Debug, Default)]
struct TrailRing {
    live: bool,
    /// The oldest sample's place in the ring.
    oldest: u16,
    len: u16,
    capacity: u16,
}

/// A particle, in the world from birth: its effect moves on without it.
#[derive(Clone, Copy, Debug)]
struct Particle {
    position: Vec3,
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    rotation: f32,
    spin: f32,
    size: f32,
    /// Cached from its emitter: the update reads these for every particle.
    gravity: f32,
    drag: f32,
    slot: u16,
    emitter: u8,
}

/// The simulation (spec §5.16): effects spawned on owners, stepped by owners' clocks, resolved into
/// a [`VfxFrame`] for the renderer. See the crate docs.
///
/// Everything is sized from the [`VfxBudget`] when it's made: past a cap it counts what it dropped
/// and keeps drawing, and nothing allocates per frame once a frame's vectors have grown.
#[derive(Clone, Debug)]
pub struct VfxWorld {
    library: EffectLibrary,
    budget: VfxBudget,
    seed: u64,
    /// Effects spawned so far: each one's random stream.
    spawned: u64,
    slots: Vec<Slot>,
    /// Slots not in use, the next last.
    free_slots: Vec<u16>,
    particles: Vec<Particle>,
    rings: Vec<TrailRing>,
    samples: Vec<TrailSample>,
    effects: u32,
    dropped_particles: u64,
    dropped_ribbon_points: u64,
    dropped_effects: u64,
    dropped_trails: u64,
}

impl VfxWorld {
    /// A world playing `library`'s effects within `budget`, its random numbers from `seed`: two
    /// worlds given the same seed and the same calls resolve the same frames.
    pub fn new(library: EffectLibrary, budget: VfxBudget, seed: u64) -> Self {
        let effects = budget.max_effects.min(u32::from(u16::MAX)) as u16;
        let trails = budget.max_trails.min(u32::from(u16::MAX)) as usize;
        Self {
            library,
            budget,
            seed,
            spawned: 0,
            slots: vec![Slot::new(); usize::from(effects)],
            free_slots: (0..effects).rev().collect(),
            particles: Vec::with_capacity(budget.max_particles as usize),
            rings: vec![TrailRing::default(); trails],
            samples: vec![TrailSample::default(); trails * MAX_TRAIL_SAMPLES],
            effects: 0,
            dropped_particles: 0,
            dropped_ribbon_points: 0,
            dropped_effects: 0,
            dropped_trails: 0,
        }
    }

    pub fn library(&self) -> &EffectLibrary {
        &self.library
    }

    pub fn budget(&self) -> VfxBudget {
        self.budget
    }

    /// Replaces the effects (a reload), ending every effect playing: their places in the library
    /// may have moved.
    pub fn set_library(&mut self, library: EffectLibrary) {
        self.clear();
        self.library = library;
    }

    /// Ends every effect playing at once.
    pub fn clear(&mut self) {
        self.particles.clear();
        for ring in &mut self.rings {
            ring.live = false;
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.live {
                slot.live = false;
                slot.generation = slot.generation.wrapping_add(1);
                self.free_slots.push(index as u16);
            }
        }
        self.effects = 0;
    }

    /// Starts `effect` at `placement` on `owner`'s clock. `None` if the library has no such effect
    /// or the budget's cap on effects is full, which is counted. An effect whose trail didn't fit
    /// spawns without it, also counted.
    ///
    /// Allocates nothing.
    pub fn spawn(&mut self, effect: EffectId, owner: OwnerId, placement: Placement) -> Option<EffectHandle> {
        let desc = self.library.effect(effect)?;
        let Some(index) = self.free_slots.pop() else {
            self.dropped_effects += 1;
            return None;
        };
        let trail = desc.trail.as_ref().and_then(|desc| {
            let ring = self.rings.iter().position(|ring| !ring.live);
            if ring.is_none() {
                self.dropped_trails += 1;
            }
            ring.map(|ring| {
                let capacity = desc.max_samples.min(MAX_TRAIL_SAMPLES as u32) as u16;
                self.rings[ring] = TrailRing { live: true, oldest: 0, len: 0, capacity };
                ring as u16
            })
        });
        self.spawned += 1;
        let slot = &mut self.slots[usize::from(index)];
        *slot = Slot {
            live: true,
            generation: slot.generation,
            effect: effect.0,
            owner,
            placement,
            rng: Rng::new(self.seed, self.spawned),
            trail,
            ..Slot::new()
        };
        self.effects += 1;
        Some(EffectHandle { slot: index, generation: slot.generation })
    }

    /// Moves where `handle`'s effect emits from now on. Particles already born stay where they are.
    pub fn set_placement(&mut self, handle: EffectHandle, placement: Placement) {
        if let Some(slot) = self.slot_mut(handle) {
            slot.placement = placement;
        }
    }

    /// Stops `handle`'s effect: no more emission or samples, and it ends when its particles and its
    /// trail have.
    pub fn stop(&mut self, handle: EffectHandle) {
        if let Some(slot) = self.slot_mut(handle) {
            slot.stopped = true;
        }
    }

    /// Whether `handle`'s effect still plays.
    pub fn is_playing(&self, handle: EffectHandle) -> bool {
        self.slot(handle).is_some()
    }

    /// Adds a sample of the two sockets to `handle`'s trail: `base` the blade's base, say, and `tip`
    /// its tip, `ago` seconds before now on the owner's clock. Call it at sub-tick rate, oldest
    /// first, for a smooth arc: after [`Self::advance`], with the sub-tick's sockets (from
    /// interpolated poses, once animation feeds them). Samples are smoothed by a centripetal
    /// Catmull-Rom spline through each socket's path (the `trail` module).
    ///
    /// Does nothing for an effect without a trail, a stopped one, sockets that aren't finite, a
    /// sample older than the newest it has, or sockets that haven't moved since it (a frozen owner
    /// repeating its pose): a trail that holds still adds no points.
    pub fn sample_trail(&mut self, handle: EffectHandle, base: Vec3, tip: Vec3, ago: f32) {
        /// Sockets closer than this to the last sample haven't moved: 1 mm squared.
        const MIN_MOVE: f32 = 1e-6;
        let Some(slot) = self.slot(handle) else { return };
        let Some(trail) = slot.trail.filter(|_| !slot.stopped) else { return };
        if !base.is_finite() || !tip.is_finite() || !ago.is_finite() {
            return;
        }
        let time = slot.age - ago.max(0.0);
        let index = usize::from(trail);
        let ring = self.rings[index];
        let start = index * MAX_TRAIL_SAMPLES;
        let at = |place: u16| start + usize::from((ring.oldest + place) % ring.capacity);
        if ring.len > 0 {
            let newest = self.samples[at(ring.len - 1)];
            let unmoved = newest.base.distance_squared(base) < MIN_MOVE
                && newest.tip.distance_squared(tip) < MIN_MOVE;
            if time < newest.time || unmoved {
                return;
            }
        }
        let sample = TrailSample { base, tip, time };
        let ring = &mut self.rings[index];
        if ring.len == ring.capacity {
            self.samples[start + usize::from(ring.oldest)] = sample;
            ring.oldest = (ring.oldest + 1) % ring.capacity;
        } else {
            let place = (ring.oldest + ring.len) % ring.capacity;
            self.samples[start + usize::from(place)] = sample;
            ring.len += 1;
        }
    }

    /// One step: `steps` says how far each owner's clock moved. Particles age and move, emitters
    /// fire and effects that have finished end, each by its owner's time alone, so an owner in
    /// hitstop holds its effects exactly where they are while others go on. Allocates nothing.
    pub fn advance(&mut self, steps: &OwnerSteps<'_>) {
        profiling::scope!("VfxWorld::advance");
        for slot in self.slots.iter_mut().filter(|slot| slot.live) {
            slot.step = steps.of(slot.owner);
            slot.age += slot.step;
        }
        self.update_particles();
        self.retire_samples();
        self.emit();
        self.retire_effects();
    }

    fn update_particles(&mut self) {
        let mut index = 0;
        while index < self.particles.len() {
            let particle = &mut self.particles[index];
            let slot = &mut self.slots[usize::from(particle.slot)];
            let step = slot.step;
            if step <= 0.0 {
                index += 1;
                continue;
            }
            particle.age += step;
            if particle.age >= particle.lifetime {
                slot.particles -= 1;
                self.particles.swap_remove(index);
                continue;
            }
            particle.velocity.y -= particle.gravity * step;
            particle.velocity *= (-particle.drag * step).exp();
            particle.position += particle.velocity * step;
            particle.rotation += particle.spin * step;
            index += 1;
        }
    }

    /// Lets go of each trail's samples past their lifetime, but for the youngest of them, which the
    /// tail fades into: the strip's last stretch reaches an age of 1 rather than stopping short.
    fn retire_samples(&mut self) {
        for slot in self.slots.iter().filter(|slot| slot.live && slot.step > 0.0) {
            let Some(trail) = slot.trail else { continue };
            let Some(desc) = self.library.effect(EffectId(slot.effect)).and_then(|e| e.trail.as_ref()) else {
                continue;
            };
            let index = usize::from(trail);
            let start = index * MAX_TRAIL_SAMPLES;
            let ring = &mut self.rings[index];
            while ring.len > 1 {
                let next = (ring.oldest + 1) % ring.capacity;
                if slot.age - self.samples[start + usize::from(next)].time < desc.lifetime {
                    break;
                }
                ring.oldest = next;
                ring.len -= 1;
            }
            if ring.len == 1 && slot.age - self.samples[start + usize::from(ring.oldest)].time >= desc.lifetime
            {
                ring.len = 0;
            }
        }
    }

    fn emit(&mut self) {
        let Self { library, budget, slots, particles, dropped_particles, .. } = self;
        for (index, slot) in slots.iter_mut().enumerate() {
            if !slot.live || slot.stopped || slot.step <= 0.0 {
                continue;
            }
            let Some(effect) = library.effect(EffectId(slot.effect)) else { continue };
            for (place, emitter) in effect.emitters.iter().enumerate() {
                let count = owed_particles(emitter, &mut slot.emitters[place], slot.age, slot.step);
                for _ in 0..count {
                    if particles.len() >= budget.max_particles as usize {
                        *dropped_particles += 1;
                        continue;
                    }
                    let particle = birth(emitter, slot, index as u16, place as u8);
                    particles.push(particle);
                    slot.particles += 1;
                }
            }
        }
    }

    fn retire_effects(&mut self) {
        let Self { library, slots, free_slots, rings, effects, .. } = self;
        for (index, slot) in slots.iter_mut().enumerate() {
            if !slot.live || slot.particles > 0 {
                continue;
            }
            let Some(effect) = library.effect(EffectId(slot.effect)) else { continue };
            let done = match slot.trail {
                // A trail records until it's stopped, then until its samples have gone.
                Some(trail) => slot.stopped && rings[usize::from(trail)].len == 0,
                // Without one: an effect whose trail didn't fit plays out as if it had none.
                None => slot.stopped || emitters_done(effect, slot),
            };
            if !done {
                continue;
            }
            if let Some(trail) = slot.trail {
                rings[usize::from(trail)].live = false;
            }
            slot.live = false;
            slot.generation = slot.generation.wrapping_add(1);
            free_slots.push(index as u16);
            *effects -= 1;
        }
    }

    /// Writes this moment's sprites and ribbon points into `frame`, replacing what it held, keeping
    /// its vectors' capacity. Allocates nothing once `frame` has held as much.
    pub fn write_frame(&mut self, frame: &mut VfxFrame) {
        profiling::scope!("VfxWorld::write_frame");
        frame.clear();
        frame.sprites.reserve(self.particles.len());
        for particle in &self.particles {
            let slot = &self.slots[usize::from(particle.slot)];
            let Some(effect) = self.library.effect(EffectId(slot.effect)) else { continue };
            let Some(emitter) = effect.emitters.get(usize::from(particle.emitter)) else { continue };
            frame.sprites.push(sprite(particle, emitter));
        }
        let mut ribbons = 0;
        for slot in self.slots.iter().filter(|slot| slot.live) {
            let Some(trail) = slot.trail else { continue };
            let Some(desc) = self.library.effect(EffectId(slot.effect)).and_then(|e| e.trail.as_ref()) else {
                continue;
            };
            let index = usize::from(trail);
            let start = index * MAX_TRAIL_SAMPLES;
            let drawn = write_ribbon(
                desc,
                self.rings[index],
                &self.samples[start..start + MAX_TRAIL_SAMPLES],
                slot.age,
                self.budget,
                &mut frame.ribbon_points,
                &mut self.dropped_ribbon_points,
            );
            ribbons += u32::from(drawn);
        }
        frame.stats = VfxStats { ribbons, ribbon_points: frame.ribbon_points.len() as u32, ..self.stats() };
    }

    /// What it holds and what it has dropped. Ribbons and their points are those of the last frame
    /// written, in [`VfxFrame::stats`].
    pub fn stats(&self) -> VfxStats {
        VfxStats {
            effects: self.effects,
            particles: self.particles.len() as u32,
            dropped_particles: self.dropped_particles,
            dropped_ribbon_points: self.dropped_ribbon_points,
            dropped_effects: self.dropped_effects,
            dropped_trails: self.dropped_trails,
            ..VfxStats::default()
        }
    }

    fn slot(&self, handle: EffectHandle) -> Option<&Slot> {
        self.slots
            .get(usize::from(handle.slot))
            .filter(|slot| slot.live && slot.generation == handle.generation)
    }

    fn slot_mut(&mut self, handle: EffectHandle) -> Option<&mut Slot> {
        self.slots
            .get_mut(usize::from(handle.slot))
            .filter(|slot| slot.live && slot.generation == handle.generation)
    }
}

/// How many particles `emitter` owes in the step of `step` seconds that ends at the effect's `age`.
fn owed_particles(emitter: &EmitterDesc, state: &mut EmitterState, age: f32, step: f32) -> u32 {
    let since_start = age - emitter.delay;
    if since_start < 0.0 {
        return 0;
    }
    match emitter.spawn {
        Spawn::Burst { count } => {
            if state.fired {
                return 0;
            }
            state.fired = true;
            count
        }
        Spawn::Rate { per_second, duration } => {
            // The share of the step inside the emitter's time.
            let began = (since_start - step).max(0.0);
            let ended = if duration > 0.0 { since_start.min(duration) } else { since_start };
            state.owed += per_second * (ended - began).max(0.0);
            let whole = state.owed.floor();
            state.owed -= whole;
            whole as u32
        }
    }
}

/// Whether every emitter of `effect` has nothing left to emit.
fn emitters_done(effect: &Effect, slot: &Slot) -> bool {
    effect.emitters.iter().zip(&slot.emitters).all(|(emitter, state)| match emitter.spawn {
        Spawn::Burst { .. } => state.fired,
        Spawn::Rate { duration, .. } => duration > 0.0 && slot.age - emitter.delay >= duration,
    })
}

/// A new particle of `emitter`, in `slot`'s effect.
fn birth(emitter: &EmitterDesc, slot: &mut Slot, slot_index: u16, place: u8) -> Particle {
    let rng = &mut slot.rng;
    let placement = slot.placement;
    let source = match emitter.source {
        Source::Point => Vec3::ZERO,
        Source::Sphere(radius) => radius * rng.unit().cbrt() * unit_vector(rng),
        Source::Disc(radius) => {
            let (angle, reach) = (TAU * rng.unit(), radius * rng.unit().sqrt());
            Vec3::new(angle.cos() * reach, 0.0, angle.sin() * reach)
        }
    };
    let motion = &emitter.motion;
    let direction = cone(Vec3::from(motion.direction).normalize_or(Vec3::Y), motion.spread, rng);
    let velocity = placement.rotation * direction * rng.between(motion.speed);
    let (rotation, spin) = match emitter.facing {
        Facing::Camera { spin } => (TAU * rng.unit(), rng.between(spin)),
        Facing::Velocity { .. } => (0.0, 0.0),
    };
    Particle {
        position: placement.position + placement.rotation * source,
        velocity,
        age: 0.0,
        lifetime: rng.between(emitter.lifetime),
        rotation,
        spin,
        size: rng.between(emitter.size_scale),
        gravity: motion.gravity,
        drag: motion.drag,
        slot: slot_index,
        emitter: place,
    }
}

/// A random unit vector.
fn unit_vector(rng: &mut Rng) -> Vec3 {
    let z = 2.0 * rng.unit() - 1.0;
    let angle = TAU * rng.unit();
    let across = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(across * angle.cos(), across * angle.sin(), z)
}

/// A random unit vector within `spread` degrees of the unit vector `axis`, uniform over that cap.
fn cone(axis: Vec3, spread: f32, rng: &mut Rng) -> Vec3 {
    let cos_spread = spread.to_radians().cos();
    let z = 1.0 - rng.unit() * (1.0 - cos_spread);
    let angle = TAU * rng.unit();
    let across = (1.0 - z * z).max(0.0).sqrt();
    let (x_axis, y_axis) = axis.any_orthonormal_pair();
    axis * z + (x_axis * angle.cos() + y_axis * angle.sin()) * across
}

/// `particle` as its emitter's curves show it at its age.
fn sprite(particle: &Particle, emitter: &EmitterDesc) -> VfxSprite {
    let t = particle.age / particle.lifetime;
    let half_width = emitter.size.sample(t) * particle.size;
    let (axis, half_length) = match emitter.facing {
        Facing::Camera { .. } => (Vec3::ZERO, half_width),
        Facing::Velocity { stretch } => (particle.velocity, half_width + particle.velocity.length() * stretch),
    };
    VfxSprite {
        position: particle.position,
        axis,
        rotation: particle.rotation,
        half_width,
        half_length,
        color: emitter.color.sample_vec3(t),
        alpha: emitter.alpha.sample(t),
        additive: emitter.additive,
        soft: emitter.soft,
        shape: emitter.shape.into(),
        light: emitter.light.into(),
    }
}

/// Appends the strip of a trail's samples to `points`, smoothed between them, its look by each
/// point's age at the effect's `age`, as far as the budget's cap leaves room: a ribbon over it loses
/// its oldest stretches. Returns whether it drew.
fn write_ribbon(
    desc: &TrailDesc,
    ring: TrailRing,
    samples: &[TrailSample],
    age: f32,
    budget: VfxBudget,
    points: &mut Vec<VfxRibbonPoint>,
    dropped: &mut u64,
) -> bool {
    let len = usize::from(ring.len);
    if len < 2 {
        return false;
    }
    let sample = |place: usize| samples[(usize::from(ring.oldest) + place) % usize::from(ring.capacity)];
    let subdivisions = budget.trail_subdivisions.max(1) as usize;
    let wanted = (len - 1) * subdivisions + 1;
    let room = (budget.max_ribbon_points as usize).saturating_sub(points.len());
    if room < 2 {
        *dropped += wanted as u64;
        return false;
    }
    // Whole stretches, oldest first, until it fits.
    let lost = wanted.saturating_sub(room).div_ceil(subdivisions);
    let drawn = wanted - lost * subdivisions;
    *dropped += (wanted - drawn) as u64;
    points.reserve(drawn);
    let core_half_width = desc.core.half_width;
    let point = |base: Vec3, tip: Vec3, time: f32, ends: bool| {
        let t = ((age - time) / desc.lifetime).clamp(0.0, 1.0);
        let width = desc.width.sample(t);
        let anchor = base.lerp(tip, desc.anchor);
        VfxRibbonPoint {
            base: anchor + (base - anchor) * width,
            tip: anchor + (tip - anchor) * width,
            color: desc.color.sample_vec3(t),
            alpha: desc.alpha.sample(t),
            core: desc.core.color.sample_vec3(t),
            core_position: desc.core.position,
            core_half_width,
            glow_reach: desc.glow_reach,
            additive: desc.additive,
            soft: desc.soft,
            ends,
        }
    };
    for place in lost..len - 1 {
        let (a, b) = (sample(place), sample(place + 1));
        // Beyond either end the path goes on as it was going: the sample before its first reflected.
        let before = if place > 0 {
            sample(place - 1)
        } else {
            TrailSample { base: a.base * 2.0 - b.base, tip: a.tip * 2.0 - b.tip, time: a.time }
        };
        let after = if place + 2 < len {
            sample(place + 2)
        } else {
            TrailSample { base: b.base * 2.0 - a.base, tip: b.tip * 2.0 - a.tip, time: b.time }
        };
        let bases = Segment::new(before.base, a.base, b.base, after.base);
        let tips = Segment::new(before.tip, a.tip, b.tip, after.tip);
        for step in 0..subdivisions {
            let u = step as f32 / subdivisions as f32;
            let time = a.time + (b.time - a.time) * u;
            points.push(point(bases.at(u), tips.at(u), time, false));
        }
    }
    let newest = sample(len - 1);
    points.push(point(newest.base, newest.tip, newest.time, true));
    true
}
