//! [`EffectStack`]: the effects playing, on their clocks, resolved once per frame.

use bevy_ecs::prelude::{Res, ResMut, Resource};
use sapphire_ecs::WorldClock;
use sapphire_math::Vec2;
use sapphire_render::PostEffects;
use sapphire_time::{LocalClock, SUBTICKS_PER_TICK, ScaledClock, SimDuration, TICK_HZ, TICK_SECONDS};

use super::layer::{EffectClock, EffectKind, EffectLayer};
use super::library::EffectPreset;

/// The most layers playing at once (see the module docs, "The stack"): four impact frames.
pub const STACK_CAPACITY: usize = 16;

const SUBTICKS_PER_SECOND: f64 = (SUBTICKS_PER_TICK as u64 * TICK_HZ as u64) as f64;

/// The layers one push started: release, stop or hold them together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectHandle(u32);

/// One layer playing.
#[derive(Clone, Copy, Debug)]
struct Instance {
    layer: EffectLayer,
    push: EffectHandle,
    /// Picks its speed lines: the push and the layer's place in it.
    seed: u32,
    /// Seconds into its envelope on its clock: negative during its delay.
    time: f32,
    /// When a sustained layer was released, in its envelope's seconds.
    released: Option<f32>,
    /// Held at `time`: its clock doesn't move it.
    held: bool,
}

impl Instance {
    /// Seconds left until it ends: infinite for a held or unreleased sustained one.
    fn remaining(&self) -> f32 {
        if self.held { f32::INFINITY } else { self.layer.end(self.released) - self.time }
    }

    fn ended(&self) -> bool {
        !self.held && self.time >= self.layer.end(self.released)
    }
}

/// A playing layer, as the overlay lists it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayingEffect {
    pub push: EffectHandle,
    pub kind: EffectKind,
    pub clock: EffectClock,
    /// Seconds into its envelope (negative during its delay).
    pub time: f32,
    /// When it ends, in its envelope's seconds: infinite for a sustained one until released.
    pub end: f32,
    /// Its weight at the last resolve.
    pub weight: f32,
    pub held: bool,
}

/// What the stack did (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EffectStackStats {
    /// Layers playing: the stack's depth.
    pub depth: u32,
    /// Of those, the ones with a weight at the last resolve (not in their delay or past their
    /// end).
    pub active: u32,
    /// Layers pushed since the stack was made.
    pub pushed: u64,
    /// Layers dropped because the stack was full (see [`EffectStack::push`]).
    pub dropped: u64,
}

/// The screen effects playing (an ECS resource; see the module docs): layers in push order,
/// each on its own timeline, at most [`STACK_CAPACITY`]. Ticked in the fixed step
/// ([`tick_screen_effects`]) and resolved once per frame into the frame packet's
/// [`PostParams::effects`](sapphire_render::PostParams::effects). Nothing here allocates.
#[derive(Resource, Debug)]
pub struct EffectStack {
    /// The world's time for [`EffectClock::World`] layers, issued by the world clock.
    clock: LocalClock,
    /// Bottom first; `None` past `len`.
    instances: [Option<Instance>; STACK_CAPACITY],
    len: usize,
    next_push: u32,
    /// Seconds the last fixed step moved each clock's layers.
    world_step: f32,
    real_step: f32,
    /// Each playing layer's weight at the last resolve, by its place.
    weights: [f32; STACK_CAPACITY],
    stats: EffectStackStats,
}

impl EffectStack {
    /// An empty stack on `clock`, a clock the world clock issued
    /// ([`ScaledClock::new_local_clock`]).
    pub fn new(clock: LocalClock) -> Self {
        Self {
            clock,
            instances: [None; STACK_CAPACITY],
            len: 0,
            next_push: 0,
            world_step: 0.0,
            real_step: 0.0,
            weights: [0.0; STACK_CAPACITY],
            stats: EffectStackStats::default(),
        }
    }

    /// Starts `preset`'s layers on top of the stack, each at the start of its delay. When the
    /// stack is full, each new layer takes the place of the layer with the least time left (the
    /// one closest to fading out by itself), or is itself dropped if it has less left than every
    /// layer playing: either way the drop is counted ([`EffectStackStats::dropped`]). Held and
    /// sustained layers that weren't released have all the time in the world.
    pub fn push(&mut self, preset: &EffectPreset) -> EffectHandle {
        self.start(preset, None)
    }

    /// [`Self::push`], with the preset's radial blur, chromatic aberration and speed lines
    /// centered on `center` (texture coordinates): where a hit landed on screen.
    pub fn push_at(&mut self, preset: &EffectPreset, center: Vec2) -> EffectHandle {
        self.start(preset, Some(center))
    }

    fn start(&mut self, preset: &EffectPreset, center: Option<Vec2>) -> EffectHandle {
        let push = EffectHandle(self.next_push);
        self.next_push = self.next_push.wrapping_add(1);
        for (index, layer) in preset.layers.iter().enumerate() {
            let mut layer = *layer;
            if let Some(center) = center {
                layer.effect = layer.effect.centered(center);
            }
            let seed = push.0.wrapping_mul(0x9e37_79b9) ^ index as u32;
            let instance = Instance { layer, push, seed, time: -layer.delay, released: None, held: false };
            self.stats.pushed += 1;
            if self.len == STACK_CAPACITY {
                self.stats.dropped += 1;
                let (shortest, left) =
                    self.instances[..self.len].iter().flatten().map(Instance::remaining).enumerate().fold(
                        (0, f32::INFINITY),
                        |best, (at, left)| if left < best.1 { (at, left) } else { best },
                    );
                if instance.remaining() < left || left == f32::INFINITY {
                    continue;
                }
                self.remove(shortest);
            }
            self.instances[self.len] = Some(instance);
            self.len += 1;
        }
        self.stats.depth = self.len as u32;
        push
    }

    /// Releases `handle`'s sustained layers: each falls over its release from now. Its other
    /// layers play on as they would.
    pub fn release(&mut self, handle: EffectHandle) {
        for instance in self.instances_of(handle) {
            if instance.released.is_none() {
                instance.released = Some(instance.time.max(0.0));
            }
        }
    }

    /// Stops `handle`'s layers at once.
    pub fn stop(&mut self, handle: EffectHandle) {
        let mut at = 0;
        while at < self.len {
            if self.instances[at].is_some_and(|instance| instance.push == handle) {
                self.remove(at);
            } else {
                at += 1;
            }
        }
        self.stats.depth = self.len as u32;
    }

    /// Stops every layer.
    pub fn clear(&mut self) {
        self.instances = [None; STACK_CAPACITY];
        self.len = 0;
        self.stats.depth = 0;
    }

    /// Moves `handle`'s layers to `seconds` after their push (each layer's own delay still
    /// counts) and holds them there, or lets them play on from wherever they are with `None`:
    /// for captures, benches and an editor's scrubbing.
    pub fn hold(&mut self, handle: EffectHandle, seconds: Option<f32>) {
        for instance in self.instances_of(handle) {
            match seconds {
                Some(seconds) => {
                    instance.time = seconds.max(0.0) - instance.layer.delay;
                    instance.held = true;
                }
                None => instance.held = false,
            }
        }
    }

    fn instances_of(&mut self, handle: EffectHandle) -> impl Iterator<Item = &mut Instance> {
        self.instances[..self.len].iter_mut().flatten().filter(move |instance| instance.push == handle)
    }

    /// Advances one fixed step: each layer by its clock's time, then drops those that ended.
    /// Allocates nothing (rule R6).
    pub fn tick(&mut self, world: &ScaledClock) {
        let scaled = self.clock.tick(world);
        self.world_step = seconds(scaled);
        // Real time counts fixed steps while the world runs, as hit-stop's countdown does.
        self.real_step = if world.is_stopped() { 0.0 } else { TICK_SECONDS };
        let (world_step, real_step) = (self.world_step, self.real_step);
        let mut at = 0;
        while at < self.len {
            let instance = self.instances[at].as_mut().expect("the first `len` places hold layers");
            if !instance.held {
                instance.time += match instance.layer.clock {
                    EffectClock::World => world_step,
                    EffectClock::Real => real_step,
                };
            }
            if instance.ended() {
                self.remove(at);
            } else {
                at += 1;
            }
        }
        self.stats.depth = self.len as u32;
    }

    /// The effects to draw `alpha` of the way from the last fixed step to the next: each layer's
    /// envelope at its time (interpolated like everything drawn), blended over the layers under
    /// it, bottom to top, so the latest push shows over what was playing by its weight, kind by
    /// kind. [`PostEffects::NONE`] when nothing plays. Allocates nothing, and costs well under a
    /// microsecond for a full stack.
    pub fn resolve(&mut self, alpha: f32) -> PostEffects {
        profiling::scope!("EffectStack::resolve");
        let mut effects = PostEffects::NONE;
        let mut active = 0;
        for (at, instance) in self.instances[..self.len].iter().flatten().enumerate() {
            let step = match instance.layer.clock {
                EffectClock::World => self.world_step,
                EffectClock::Real => self.real_step,
            };
            let time =
                if instance.held { instance.time } else { instance.time + alpha.clamp(0.0, 1.0) * step };
            let weight = instance.layer.envelope.weight(time, instance.released);
            self.weights[at] = weight;
            if weight > 0.0 {
                active += 1;
                let seed = instance.seed.wrapping_add(redraws(&instance.layer, time));
                instance.layer.effect.blend_over(&mut effects, weight, seed);
            }
        }
        self.stats.active = active;
        effects
    }

    /// The layers playing, bottom first, with their weights at the last resolve.
    pub fn playing(&self) -> impl ExactSizeIterator<Item = PlayingEffect> + '_ {
        self.instances[..self.len].iter().zip(&self.weights).map(|(instance, &weight)| {
            let instance = instance.expect("the first `len` places hold layers");
            PlayingEffect {
                push: instance.push,
                kind: instance.layer.effect.kind(),
                clock: instance.layer.clock,
                time: instance.time,
                end: instance.layer.end(instance.released),
                weight,
                held: instance.held,
            }
        })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn stats(&self) -> EffectStackStats {
        self.stats
    }

    /// The stack's clock: the world's time, for [`EffectClock::World`] layers.
    pub fn clock(&self) -> &LocalClock {
        &self.clock
    }

    /// Removes the layer at `at`, keeping the others in order.
    fn remove(&mut self, at: usize) {
        self.instances.copy_within(at + 1..self.len, at);
        self.weights.copy_within(at + 1..self.len, at);
        self.len -= 1;
        self.instances[self.len] = None;
    }
}

/// How many times speed lines have been redrawn `time` seconds into their layer.
fn redraws(layer: &EffectLayer, time: f32) -> u32 {
    match layer.effect {
        super::layer::ScreenEffect::SpeedLines(lines) if lines.rate > 0.0 => {
            (time.max(0.0) * lines.rate) as u32
        }
        _ => 0,
    }
}

fn seconds(duration: SimDuration) -> f32 {
    (duration.as_subticks() as f64 / SUBTICKS_PER_SECOND) as f32
}

/// Advances the screen effects by the step's time. Add it to `FixedFirst` after
/// `SimSet::Clocks`, so it sees the step's world scales.
pub fn tick_screen_effects(world: Res<'_, WorldClock>, mut stack: ResMut<'_, EffectStack>) {
    stack.tick(&world.0);
}
