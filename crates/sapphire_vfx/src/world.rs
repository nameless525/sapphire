//! [`VfxWorld`]: the effects playing, each on its owner's clock, and the frame they resolve to.

use std::f32::consts::TAU;

use glam::{Quat, Vec3};
use sapphire_look::{VfxFrame, VfxRibbonPoint, VfxSprite, VfxStats};

use crate::budget::VfxBudget;
use crate::decal::DecalLayerDesc;
use crate::document::{
    Effect, EffectId, EffectLibrary, EmitterDesc, Facing, MAX_EMITTERS, MAX_TRAIL_SAMPLES, Source, Spawn,
    TrailDesc,
};
use crate::forces::{Force, Here, Resolved, noise_offset};
use crate::mesh::MeshLayerDesc;
use crate::rng::Rng;
use crate::trail::{Segment, TrailSample};
use crate::wind::WindSample;

/// What sets the marks' random streams apart from every other effect's, from the same seed.
const MARK_SEED: u64 = 0x6D61_726B_7321;

/// The farthest an effect moves in one step and still fires a rate a meter along the way, in metres (M6 N2):
/// 180 m/s at 60 steps a second, past any dash or launch. A longer move is a jump, a teleport, a respawn or a
/// hop between zones, and it lays no line of particles across the gap.
const MAX_STEP_MOVE: f32 = 3.0;

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
    /// Fractions of a particle a rate, or a rate a meter, has owed since its last one.
    owed: f32,
    /// Seconds to go before it starts, or before a repeat's next burst.
    delay_left: f32,
    time_left: f32,
    /// The bursts a repeat has fired.
    bursts: u32,
    /// Where its noise starts in the noise's lattice (M6 N2), from a stream of its own.
    noise_offset: Vec3,
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
    /// This step's time on the owner's clock, which a frame reads back along ([`VfxWorld::write_frame`]).
    step: f32,
    /// No more emission or samples; it ends when what it holds has.
    stopped: bool,
    /// Live particles it spawned.
    particles: u32,
    emitters: [EmitterState; MAX_EMITTERS],
    /// Its place in the trails, if its effect has one and there was room.
    trail: Option<u16>,
    /// Seconds on the owner's clock from its start to its actor's next strike, where its cue knew one
    /// ([`VfxWorld::set_strike`]): what a telegraph's decal lives until.
    strike: Option<f32>,
    /// The height of its owner's ground, which its particles' [`Ground`](crate::Ground) lies at: where it was
    /// placed, until [`VfxWorld::set_ground`] says.
    ground: f32,
    /// Where it emitted from on the last step that emitted, which a rate a meter measures from.
    emitted_from: Vec3,
    /// Whether this step is a noise's (one of every two, under `noise_on_alternate_steps`), and the time the
    /// steps it skipped owe it.
    noise_turn: bool,
    noise_owed: f32,
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
            strike: None,
            ground: 0.0,
            emitted_from: Vec3::ZERO,
            noise_turn: false,
            noise_owed: 0.0,
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
    /// Whether its emitter has forces or a ground (M6 N2), which a step has resolved for it.
    forced: bool,
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
    /// Effects spawned so far, but marks: each one's random stream.
    spawned: u64,
    /// Marks spawned so far (M4 A5, [`Effect::mark`]): their streams, apart from the others'.
    marks: u64,
    slots: Vec<Slot>,
    /// Slots not in use, the next last.
    free_slots: Vec<u16>,
    /// Slots in use, in the order their effects started: what a cap keeps of equal priority, whichever slot an
    /// effect took ([`write_capped`]).
    by_age: Vec<u16>,
    particles: Vec<Particle>,
    /// Each playing emitter's forces as this step applies them (M6 N2), [`MAX_EMITTERS`] a slot: written for
    /// the emitters that have them, read by their particles.
    resolved: Vec<Resolved>,
    /// The wind this step's `Wind` forces carry particles by.
    wind: WindSample,
    rings: Vec<TrailRing>,
    samples: Vec<TrailSample>,
    effects: u32,
    dropped_particles: u64,
    dropped_ribbon_points: u64,
    dropped_effects: u64,
    dropped_trails: u64,
    dropped_meshes: u64,
    dropped_decals: u64,
    dropped_distortions: u64,
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
            marks: 0,
            slots: vec![Slot::new(); usize::from(effects)],
            free_slots: (0..effects).rev().collect(),
            by_age: Vec::with_capacity(usize::from(effects)),
            particles: Vec::with_capacity(budget.max_particles as usize),
            resolved: vec![Resolved::NONE; usize::from(effects) * MAX_EMITTERS],
            wind: WindSample::CALM,
            rings: vec![TrailRing::default(); trails],
            samples: vec![TrailSample::default(); trails * MAX_TRAIL_SAMPLES],
            effects: 0,
            dropped_particles: 0,
            dropped_ribbon_points: 0,
            dropped_effects: 0,
            dropped_trails: 0,
            dropped_meshes: 0,
            dropped_decals: 0,
            dropped_distortions: 0,
        }
    }

    pub fn library(&self) -> &EffectLibrary {
        &self.library
    }

    pub fn budget(&self) -> VfxBudget {
        self.budget
    }

    /// Plays within `budget` from now on (a quality tier's change, M4 T3), resizing what it's sized by: every
    /// effect playing ends, as with [`Self::clear`], and the seeded sequence goes on. Allocates, so a game calls
    /// it at a pause or a wipe, never in a step. Nothing changes for the budget it has.
    pub fn set_budget(&mut self, budget: VfxBudget) {
        if budget == self.budget {
            return;
        }
        let fresh = Self::new(self.library.clone(), budget, self.seed);
        let (spawned, marks, wind) = (self.spawned, self.marks, self.wind);
        *self = Self { spawned, marks, wind, ..fresh };
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
        self.by_age.clear();
        self.effects = 0;
    }

    /// Starts the seeded sequence over, as a fresh world with this library and budget would.
    /// Ends every effect and clears drop counters, preserving all pool capacities. Old handles
    /// stay invalid, including when their slots are reused. Allocates nothing.
    pub fn start_over(&mut self) {
        self.clear();
        self.free_slots.clear();
        self.free_slots.extend((0..self.slots.len() as u16).rev());
        self.spawned = 0;
        self.marks = 0;
        self.dropped_particles = 0;
        self.dropped_ribbon_points = 0;
        self.dropped_effects = 0;
        self.dropped_trails = 0;
        self.dropped_meshes = 0;
        self.dropped_decals = 0;
        self.dropped_distortions = 0;
    }

    /// Starts `effect` at `placement` on `owner`'s clock. `None` if the library has no such effect
    /// or the budget's cap on effects is full, which is counted. An effect whose trail didn't fit
    /// spawns without it, also counted. A [mark](Effect::mark) takes its numbers from the marks'
    /// stream, so it moves no other effect's. An effect of ground decals alone (M5 L7), or of them and mesh layers
    /// that only bend the picture (M5 L6), or of stateless emitters (M6 N8a), takes no stream at all, as it uses no
    /// random numbers here: playing one moves no spark of any effect spawned after it.
    ///
    /// Allocates nothing.
    pub fn spawn(&mut self, effect: EffectId, owner: OwnerId, placement: Placement) -> Option<EffectHandle> {
        let desc = self.library.effect(effect)?;
        let mark = desc.mark;
        if desc.emitters.iter().all(EmitterDesc::is_stateless)
            && desc.trail.is_none()
            && desc.meshes.iter().all(MeshLayerDesc::bends_only)
        {
            return self.start(effect, owner, placement);
        }
        let handle = self.start(effect, owner, placement)?;
        let stream = if mark {
            self.marks += 1;
            Rng::new(self.seed ^ MARK_SEED, self.marks)
        } else {
            self.spawned += 1;
            Rng::new(self.seed, self.spawned)
        };
        // A noise's offset comes from its emitter's own stream, forked off this one, which it doesn't advance.
        let mut offsets = [Vec3::ZERO; MAX_EMITTERS];
        let emitters = self.library.effect(effect).map_or(&[][..], |desc| &desc.emitters[..]);
        for (place, emitter) in emitters.iter().enumerate().take(MAX_EMITTERS) {
            if !emitter.is_stateless() && emitter.forces.iter().any(Force::is_noise) {
                offsets[place] = noise_offset(&stream, place);
            }
        }
        if let Some(slot) = self.slot_mut(handle) {
            slot.rng = stream;
            for (state, offset) in slot.emitters.iter_mut().zip(offsets) {
                state.noise_offset = offset;
            }
        }
        Some(handle)
    }

    /// As [`Self::spawn`], for an effect that's a trail and nothing else, which takes no random numbers: so
    /// it leaves those of the effects spawned after it as they'd be without it. One with emitters needs
    /// numbers of its own, and is spawned as ever.
    pub fn spawn_trail(
        &mut self,
        effect: EffectId,
        owner: OwnerId,
        placement: Placement,
    ) -> Option<EffectHandle> {
        if self.library.effect(effect)?.emitters.is_empty() {
            self.start(effect, owner, placement)
        } else {
            self.spawn(effect, owner, placement)
        }
    }

    /// Takes a slot, and a ring for a trail, for `effect`.
    fn start(&mut self, effect: EffectId, owner: OwnerId, placement: Placement) -> Option<EffectHandle> {
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
        let slot = &mut self.slots[usize::from(index)];
        *slot = Slot {
            live: true,
            generation: slot.generation,
            effect: effect.0,
            owner,
            placement,
            trail,
            ground: placement.position.y,
            emitted_from: placement.position,
            ..Slot::new()
        };
        for (state, emitter) in slot.emitters.iter_mut().zip(&desc.emitters) {
            state.delay_left = emitter.delay;
            if let Spawn::Rate { duration, .. } = emitter.spawn {
                state.time_left = duration;
            }
        }
        self.by_age.push(index);
        self.effects += 1;
        Some(EffectHandle { slot: index, generation: slot.generation })
    }

    /// Moves where `handle`'s effect emits from now on. Particles already born stay where they are.
    pub fn set_placement(&mut self, handle: EffectHandle, placement: Placement) {
        if let Some(slot) = self.slot_mut(handle) {
            slot.placement = placement;
        }
    }

    /// Says when `handle`'s actor's next strike lands: `seconds` on its owner's clock from the effect's start,
    /// which a decal that lives `until_strike` lives until (M5 L7). A cue's dispatcher sets it as it plays a
    /// telegraph; nothing else reads it. Ignored unless it's finite and above 0.
    pub fn set_strike(&mut self, handle: EffectHandle, seconds: f32) {
        if let Some(slot) = self.slot_mut(handle) {
            slot.strike = (seconds.is_finite() && seconds > 0.0).then_some(seconds);
        }
    }

    /// Says where `handle`'s owner stands: the height of the level ground its particles' [`Ground`](crate::Ground)
    /// lies at (M6 N2), from the next step on. Until it's said, the ground is where the effect was placed, which
    /// suits dust at the feet and not sparks at a chest. Ignored unless it's finite.
    pub fn set_ground(&mut self, handle: EffectHandle, height: f32) {
        if let Some(slot) = self.slot_mut(handle)
            && height.is_finite()
        {
            slot.ground = height;
        }
    }

    /// The wind for the steps from now on, until it's set again: what particles with a `Wind` force are carried
    /// by (M6 N2). The caller fills it before each step, from the wind field once there's one (M6 Y1); calm until
    /// then. A sample that isn't finite is calm.
    pub fn set_wind(&mut self, wind: WindSample) {
        self.wind = wind.finite();
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
            let unmoved =
                newest.base.distance_squared(base) < MIN_MOVE && newest.tip.distance_squared(tip) < MIN_MOVE;
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
            slot.age = (slot.age + slot.step).min(f32::MAX);
        }
        self.resolve_forces();
        self.update_particles();
        self.retire_samples();
        self.emit();
        self.retire_effects();
    }

    /// Each moving emitter's forces and ground, in the world, for this step (M6 N2): once an emitter, so a
    /// particle only runs through them. Only emitters with forces or a ground are written, and only their
    /// particles read them.
    fn resolve_forces(&mut self) {
        let Self { library, budget, slots, resolved, wind, .. } = self;
        for (index, slot) in slots.iter_mut().enumerate() {
            if !slot.live || slot.step <= 0.0 {
                continue;
            }
            let Some(effect) = library.effect(EffectId(slot.effect)) else { continue };
            if !effect.emitters.iter().any(EmitterDesc::is_moved) {
                continue;
            }
            let noise_step = if budget.noise_on_alternate_steps {
                slot.noise_turn = !slot.noise_turn;
                if slot.noise_turn {
                    slot.noise_owed += slot.step;
                    0.0
                } else {
                    std::mem::take(&mut slot.noise_owed) + slot.step
                }
            } else {
                slot.step
            };
            let here = Here {
                position: slot.placement.position,
                rotation: slot.placement.rotation,
                ground: slot.ground,
                age: slot.age,
                step: slot.step,
                noise_step,
                wind: *wind,
            };
            for (place, emitter) in effect.emitters.iter().enumerate().take(MAX_EMITTERS) {
                if emitter.is_moved() {
                    let offset = slot.emitters[place].noise_offset;
                    resolved[index * MAX_EMITTERS + place] =
                        Resolved::new(&emitter.forces, emitter.collide.as_ref(), offset, &here);
                }
            }
        }
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
            if particle.forced {
                let resolved =
                    &self.resolved[usize::from(particle.slot) * MAX_EMITTERS + usize::from(particle.emitter)];
                let mut alive = resolved.pull(particle.position, &mut particle.velocity);
                if alive {
                    particle.velocity *= (-particle.drag * step).exp();
                    particle.position += particle.velocity * step;
                    alive = resolved
                        .ground
                        .is_none_or(|ground| ground.meet(&mut particle.position, &mut particle.velocity));
                }
                if !alive {
                    slot.particles -= 1;
                    self.particles.swap_remove(index);
                    continue;
                }
            } else {
                particle.velocity *= (-particle.drag * step).exp();
                particle.position += particle.velocity * step;
            }
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
            if ring.len == 1
                && slot.age - self.samples[start + usize::from(ring.oldest)].time >= desc.lifetime
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
            // The way it was moved since it last emitted, which a rate a meter spreads its births along. A
            // move too far for a step is a jump, a teleport or a respawn: it owes nothing across the gap.
            let (mut from, to) = (slot.emitted_from, slot.placement.position);
            let moved = from.distance(to);
            let moved = if moved.is_finite() && moved <= MAX_STEP_MOVE {
                moved
            } else {
                from = to;
                0.0
            };
            // A stateless emitter is the GPU's (M6 N8a): it takes no state, budget or random number here.
            for (place, emitter) in
                effect.emitters.iter().enumerate().filter(|(_, emitter)| !emitter.is_stateless())
            {
                let count = owed_particles(emitter, &mut slot.emitters[place], slot.age, slot.step, moved);
                // Only what fits is walked, so a step costs the budget at most, however much a long
                // one's rate owes. A drop takes no random numbers, so what's kept doesn't move.
                let room = (budget.max_particles as usize).saturating_sub(particles.len());
                let born = room.min(count as usize);
                let along = matches!(emitter.spawn, Spawn::PerMeter { .. });
                for birth_index in 0..born {
                    let origin =
                        if along { from.lerp(to, (birth_index + 1) as f32 / born as f32) } else { to };
                    let particle = birth(emitter, slot, origin, index as u16, place as u8);
                    particles.push(particle);
                    slot.particles += 1;
                }
                *dropped_particles = dropped_particles.saturating_add(u64::from(count) - born as u64);
            }
            slot.emitted_from = to;
        }
    }

    fn retire_effects(&mut self) {
        let Self { library, budget, slots, free_slots, by_age, rings, effects, .. } = self;
        let mut retired = false;
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
            } && meshes_done(effect, slot)
                && decals_done(effect, slot, budget);
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
            retired = true;
        }
        if retired {
            by_age.retain(|&index| slots[usize::from(index)].live);
        }
    }

    /// Writes the frame `alpha` of the way from the step before the latest to the latest into `frame`,
    /// replacing what it held, keeping its vectors' capacity: where transforms and poses draw (spec
    /// §4.5, review WA-3), each effect by its own owner's last step, so a frozen owner's hold exactly and
    /// nothing leads the pose it accents. A particle born on the latest step, or a trail that only just
    /// began, reads from where it begins, whatever the `alpha`. Allocates nothing once `frame` has held as
    /// much.
    pub fn write_frame(&mut self, frame: &mut VfxFrame, alpha: f32) {
        profiling::scope!("VfxWorld::write_frame");
        // Written so that NaN draws the previous state, as `presented_ticks` does.
        let unplayed = 1.0 - if alpha >= 0.0 { alpha.min(1.0) } else { 0.0 };
        frame.clear();
        frame.sprites.reserve(self.particles.len());
        for particle in &self.particles {
            let slot = &self.slots[usize::from(particle.slot)];
            let Some(effect) = self.library.effect(EffectId(slot.effect)) else { continue };
            let Some(emitter) = effect.emitters.get(usize::from(particle.emitter)) else { continue };
            let floor = emitter.collide.map(|_| slot.ground);
            frame.sprites.push(sprite(particle, emitter, unplayed * slot.step, floor));
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
                slot.age - unplayed * slot.step,
                self.budget,
                &mut frame.ribbon_points,
                &mut self.dropped_ribbon_points,
            );
            ribbons += u32::from(drawn);
        }
        self.write_meshes(frame, unplayed);
        self.write_decals(frame, unplayed);
        self.write_distortions(frame, unplayed);
        frame.stats = VfxStats {
            ribbons,
            ribbon_points: frame.ribbon_points.len() as u32,
            meshes: frame.meshes.len() as u32,
            decals: frame.decals.len() as u32,
            distortions: frame.distortions.len() as u32,
            ..self.stats()
        };
    }

    /// The live effects' mesh layers, by the budget: past [`VfxBudget::max_meshes`] the highest priority
    /// draw, and of equal priority those of the effects that started first, so what's kept doesn't flicker; the
    /// rest are counted in `dropped_meshes`, once a frame each. Two passes, no allocation.
    fn write_meshes(&mut self, frame: &mut VfxFrame, unplayed: f32) {
        let Self { library, slots, by_age, budget, dropped_meshes, .. } = self;
        let (library, slots): (&EffectLibrary, &[Slot]) = (library, slots);
        let dropped = write_capped(
            ByAge { slots, order: by_age },
            unplayed,
            budget.max_meshes,
            move |slot| library.effect(EffectId(slot.effect)).map_or(&[][..], |effect| &effect.meshes[..]),
            |layer: &MeshLayerDesc, _, age| (layer.has_color() && layer.shows(age)).then_some(layer.priority),
            |layer, slot, age| layer.resolve(&slot.placement, age),
            &mut frame.meshes,
        );
        *dropped_meshes = dropped_meshes.saturating_add(dropped);
    }

    /// The live effects' ground decals, by the budget, as [`Self::write_meshes`] does meshes:
    /// past [`VfxBudget::max_decals`], counted in `dropped_decals`.
    fn write_decals(&mut self, frame: &mut VfxFrame, unplayed: f32) {
        let Self { library, slots, by_age, budget, dropped_decals, .. } = self;
        let (library, slots, budget): (&EffectLibrary, &[Slot], VfxBudget) = (library, slots, *budget);
        let dropped = write_capped(
            ByAge { slots, order: by_age },
            unplayed,
            budget.max_decals,
            move |slot| library.effect(EffectId(slot.effect)).map_or(&[][..], |effect| &effect.decals[..]),
            |layer: &DecalLayerDesc, slot, age| {
                layer.shows(age, slot.strike, &budget).then_some(layer.priority)
            },
            |layer, slot, age| layer.resolve(&slot.placement, age, slot.strike, &budget),
            &mut frame.decals,
        );
        *dropped_decals = dropped_decals.saturating_add(dropped);
    }

    /// The live effects' mesh layers that bend the picture (M5 L6), by the budget, as [`Self::write_meshes`]
    /// does the meshes that draw light, and apart from them: past [`VfxBudget::max_distortions`], counted in
    /// `dropped_distortions`. A cap of 0 writes none.
    fn write_distortions(&mut self, frame: &mut VfxFrame, unplayed: f32) {
        // A tier without them (Mobile-Low) has nothing past a cap to count.
        if self.budget.max_distortions == 0 {
            return;
        }
        let Self { library, slots, by_age, budget, dropped_distortions, .. } = self;
        let (library, slots): (&EffectLibrary, &[Slot]) = (library, slots);
        let dropped = write_capped(
            ByAge { slots, order: by_age },
            unplayed,
            budget.max_distortions,
            move |slot| library.effect(EffectId(slot.effect)).map_or(&[][..], |effect| &effect.meshes[..]),
            |layer: &MeshLayerDesc, _, age| {
                (layer.distortion.is_some() && layer.shows(age)).then_some(layer.priority)
            },
            |layer, slot, age| layer.resolve_distortion(&slot.placement, age),
            &mut frame.distortions,
        );
        *dropped_distortions = dropped_distortions.saturating_add(dropped);
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
            dropped_meshes: self.dropped_meshes,
            dropped_decals: self.dropped_decals,
            dropped_distortions: self.dropped_distortions,
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

/// How many particles `emitter` owes in this step, its effect having been `moved` meters since it last emitted.
/// Rates use remaining intervals rather than subtracting large absolute ages; bursts retain their existing
/// absolute-clock admission boundary.
fn owed_particles(emitter: &EmitterDesc, state: &mut EmitterState, age: f32, step: f32, moved: f32) -> u32 {
    match emitter.spawn {
        Spawn::Burst { count } => {
            let since_start = age - emitter.delay;
            if since_start < 0.0 {
                return 0;
            }
            if state.fired {
                return 0;
            }
            state.fired = true;
            count
        }
        Spawn::Rate { per_second, duration } => {
            let active = (step - state.delay_left).max(0.0);
            state.delay_left = (state.delay_left - step).max(0.0);
            if state.delay_left > 0.0 {
                return 0;
            }
            let elapsed = if duration > 0.0 { active.min(state.time_left) } else { active };
            state.time_left = (state.time_left - elapsed).max(0.0);
            let owed = state.owed + per_second * elapsed;
            // Keep a finite fractional remainder even when the count saturates. Every finite
            // f32 rate/step product fits in f64; ordinary counts stay on the f32 path.
            if owed.is_finite() {
                let whole = owed.floor();
                state.owed = owed - whole;
                whole as u32
            } else {
                let owed = f64::from(state.owed) + f64::from(per_second) * f64::from(elapsed);
                state.owed = (owed - owed.floor()) as f32;
                u32::MAX
            }
        }
        Spawn::Repeat { count, interval, times } => {
            if times > 0 && state.bursts >= times {
                return 0;
            }
            if state.delay_left > step {
                state.delay_left -= step;
                return 0;
            }
            // One burst is due as the wait runs out, and one more each interval of the step left after it.
            let rest = f64::from(step - state.delay_left);
            let due = 1.0 + (rest / f64::from(interval)).floor();
            let left = if times > 0 { f64::from(times - state.bursts) } else { f64::from(u32::MAX) };
            let bursts = due.min(left);
            state.delay_left =
                if due > left { interval } else { ((f64::from(interval) * bursts - rest) as f32).max(0.0) };
            let bursts = bursts as u32;
            state.bursts = state.bursts.saturating_add(bursts);
            count.saturating_mul(bursts)
        }
        Spawn::PerMeter { per_meter } => {
            state.delay_left = (state.delay_left - step).max(0.0);
            if state.delay_left > 0.0 {
                return 0;
            }
            let owed = state.owed + per_meter * moved;
            // As a rate's: a finite remainder, even when a leap owes more than a count holds.
            if owed.is_finite() && owed < u32::MAX as f32 {
                let whole = owed.floor();
                state.owed = owed - whole;
                whole as u32
            } else {
                state.owed = 0.0;
                u32::MAX
            }
        }
    }
}

/// The live slots in the order their effects started, the oldest first: [`VfxWorld`]'s `by_age` over its slots.
#[derive(Clone, Copy)]
struct ByAge<'a> {
    slots: &'a [Slot],
    order: &'a [u16],
}

impl<'a> ByAge<'a> {
    fn iter(self) -> impl Iterator<Item = &'a Slot> {
        self.order.iter().map(move |&index| &self.slots[usize::from(index)])
    }
}

/// Appends the live slots' layers (`layers` of each) that show at the frame's age to `out`, by priority up to
/// `cap`: the highest priority first, and of equal priority those of the effects that started first, whichever
/// slot each took ([`ByAge`]), so what's kept doesn't flicker and an effect started in a slot an older one left
/// never evicts a layer already drawing. `shows` gives a layer's priority where it shows then, and `resolve`
/// what it draws; `out` is in the order the effects started. Returns how many didn't fit. Two passes over the
/// live slots, no sort and no allocation once `out` has held as much.
fn write_capped<'a, L: 'a, T>(
    live_slots: ByAge<'a>,
    unplayed: f32,
    cap: u32,
    layers: impl Fn(&Slot) -> &'a [L],
    shows: impl Fn(&L, &Slot, f32) -> Option<u8>,
    resolve: impl Fn(&L, &Slot, f32) -> Option<T>,
    out: &mut Vec<T>,
) -> u64 {
    let mut showing = [0_u32; 256];
    let mut total = 0_u32;
    let live = || {
        live_slots.iter().flat_map(|slot| {
            let age = slot.age - unplayed * slot.step;
            layers(slot).iter().map(move |layer| (layer, slot, age))
        })
    };
    for (layer, slot, age) in live() {
        if let Some(priority) = shows(layer, slot, age) {
            showing[usize::from(priority)] += 1;
            total += 1;
        }
    }
    if total == 0 {
        return 0;
    }
    // The lowest priority that draws, and how many of it fit.
    let (mut floor, mut room) = (0_usize, cap);
    let mut above = 0_u32;
    for priority in (0..256).rev() {
        if above + showing[priority] >= cap {
            floor = priority;
            room = cap - above;
            break;
        }
        above += showing[priority];
    }
    let before = out.len();
    out.reserve(total.min(cap) as usize);
    for (layer, slot, age) in live() {
        let Some(priority) = shows(layer, slot, age).map(usize::from) else { continue };
        if priority < floor || (priority == floor && room == 0) {
            continue;
        }
        if let Some(item) = resolve(layer, slot, age) {
            if priority == floor {
                room -= 1;
            }
            out.push(item);
        }
    }
    u64::from(total) - (out.len() - before) as u64
}

/// Whether every decal of `effect` has played out (M5 L7): as a mesh layer, a stop doesn't cut one short.
fn decals_done(effect: &Effect, slot: &Slot, budget: &VfxBudget) -> bool {
    effect.decals.iter().all(|layer| slot.age >= layer.end(slot.strike, budget))
}

/// Whether every mesh layer of `effect` has played out: a stop doesn't cut one short, as it doesn't a
/// particle already born.
fn meshes_done(effect: &Effect, slot: &Slot) -> bool {
    effect.meshes.iter().all(|layer| slot.age >= layer.end())
}

/// Whether every emitter of `effect` has nothing left to emit.
fn emitters_done(effect: &Effect, slot: &Slot) -> bool {
    effect.emitters.iter().zip(&slot.emitters).all(|(emitter, state)| match emitter.spawn {
        // The GPU's (M6 N8a): nothing here waits on it.
        _ if emitter.is_stateless() => true,
        Spawn::Burst { .. } => state.fired,
        Spawn::Rate { duration, .. } => duration > 0.0 && state.delay_left == 0.0 && state.time_left == 0.0,
        Spawn::Repeat { times, .. } => times > 0 && state.bursts >= times,
        Spawn::PerMeter { .. } => false,
    })
}

/// A new particle of `emitter`, in `slot`'s effect, from `origin`: where the effect is, or for a rate a meter
/// somewhere along the way it moved.
fn birth(emitter: &EmitterDesc, slot: &mut Slot, origin: Vec3, slot_index: u16, place: u8) -> Particle {
    let rng = &mut slot.rng;
    let placement = slot.placement;
    let source = match emitter.source {
        Source::Point => Vec3::ZERO,
        Source::Sphere(radius) => radius * rng.unit().cbrt() * unit_vector(rng),
        Source::Disc(radius) => {
            let (angle, reach) = (TAU * rng.unit(), radius * rng.unit().sqrt());
            Vec3::new(angle.cos() * reach, 0.0, angle.sin() * reach)
        }
        // M6 N8a's shapes, as the stateless form's (`sapphire_look::vfx::stateless`) are.
        Source::Ring { radius, width } => {
            let (angle, reach) = (TAU * rng.unit(), radius + width * (rng.unit() - 0.5));
            Vec3::new(angle.cos() * reach, 0.0, angle.sin() * reach)
        }
        Source::Cone { angle, length } => {
            let height = length * rng.unit().cbrt();
            let reach = height * angle.to_radians().tan() * rng.unit().sqrt();
            let turn = TAU * rng.unit();
            Vec3::new(turn.cos() * reach, height, turn.sin() * reach)
        }
        Source::Box(half) => {
            Vec3::from(half)
                * Vec3::new(2.0 * rng.unit() - 1.0, 2.0 * rng.unit() - 1.0, 2.0 * rng.unit() - 1.0)
        }
        Source::Line(length) => Vec3::new((rng.unit() - 0.5) * length, 0.0, 0.0),
    };
    let motion = &emitter.motion;
    let direction = cone(Vec3::from(motion.direction).normalize_or(Vec3::Y), motion.spread, rng);
    let velocity = placement.rotation * direction * rng.between(motion.speed);
    let (rotation, spin) = match emitter.facing {
        Facing::Camera { spin } => (TAU * rng.unit(), rng.between(spin)),
        Facing::Velocity { .. } | Facing::Upright => (0.0, 0.0),
    };
    Particle {
        position: origin + placement.rotation * source,
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
        forced: emitter.is_moved(),
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

/// `particle` as its emitter's curves show it at its age, `behind` seconds of its owner's clock before
/// its latest step. It moved in a straight line over that step, so this is where it was, and no further
/// back than its birth; and never under the `floor` it bounced on, if it collides (M6 N2).
fn sprite(particle: &Particle, emitter: &EmitterDesc, behind: f32, floor: Option<f32>) -> VfxSprite {
    let behind = behind.min(particle.age);
    let t = (particle.age - behind) / particle.lifetime;
    let half_width = emitter.size.sample(t) * particle.size;
    let (axis, half_length) = match emitter.facing {
        Facing::Camera { .. } | Facing::Upright => (Vec3::ZERO, half_width),
        Facing::Velocity { stretch } => {
            (particle.velocity, half_width + particle.velocity.length() * stretch)
        }
    };
    let mut position = particle.position - particle.velocity * behind;
    if let Some(floor) = floor {
        position.y = position.y.max(floor);
    }
    VfxSprite {
        position,
        axis,
        rotation: particle.rotation - particle.spin * behind,
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

/// Appends the strip of a trail's samples up to the effect's `age` to `points`, smoothed between them,
/// its look by each point's age then, as far as the budget's cap leaves room: a ribbon over it loses its
/// oldest stretches. A frame that falls between two samples ends the strip there, on the spline the next
/// frame will draw, so the head stays with the blade. Returns whether it drew.
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
    // The samples this frame has reached, and how far into the stretch after them it has got.
    let reached = (0..len).rev().find(|&place| sample(place).time <= age).map_or(0, |place| place + 1);
    if reached == 0 {
        return false;
    }
    let head = (reached < len).then(|| {
        let (from, to) = (sample(reached - 1), sample(reached));
        ((age - from.time) / (to.time - from.time).max(f32::MIN_POSITIVE)).clamp(0.0, 1.0)
    });
    let head = head.filter(|&share| share > 0.0);
    let stretches = reached - 1 + usize::from(head.is_some());
    if stretches == 0 {
        return false;
    }
    let subdivisions = budget.trail_subdivisions.max(1) as usize;
    let wanted = stretches * subdivisions + 1;
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
        let start = if desc.start_fade > 0.0 {
            let share = (time / desc.start_fade).clamp(0.0, 1.0);
            share * share * (3.0 - 2.0 * share)
        } else {
            1.0
        };
        let width = desc.width.sample(t) * start;
        let anchor = base.lerp(tip, desc.anchor);
        VfxRibbonPoint {
            base: anchor + (base - anchor) * width,
            tip: anchor + (tip - anchor) * width,
            color: desc.color.sample_vec3(t),
            alpha: desc.alpha.sample(t) * start,
            core: desc.core.color.sample_vec3(t),
            core_position: desc.core.position,
            core_half_width,
            glow_reach: desc.glow_reach,
            additive: desc.additive,
            soft: desc.soft,
            ends,
        }
    };
    // Where the strip ends: the last sample it reached, or partway along the stretch after it.
    let mut end = sample(reached - 1);
    for place in lost..stretches {
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
        let reach = if place + 1 == reached { head.unwrap_or(1.0) } else { 1.0 };
        for step in 0..subdivisions {
            let u = step as f32 / subdivisions as f32 * reach;
            let time = a.time + (b.time - a.time) * u;
            points.push(point(bases.at(u), tips.at(u), time, false));
        }
        if place + 1 == reached {
            end = TrailSample { base: bases.at(reach), tip: tips.at(reach), time: age };
        }
    }
    points.push(point(end.base, end.tip, end.time, true));
    true
}
