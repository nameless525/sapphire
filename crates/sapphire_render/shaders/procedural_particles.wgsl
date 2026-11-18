// Stateless particles (M6 N8a; `sapphire_look::vfx::stateless`, whose docs have the model): every
// particle a closed-form function of its instance's seed, its own index and its instance's age on its
// owner's clock, with nothing kept between frames. Operation for operation the CPU's
// (`StatelessEmitter::particle`), so the renderer's GPU test holds the two within its tolerance. It is
// the one stateless include: the effects' stateless draw (N8b) evaluates `stateless_particle` in its
// vertex stage, and falling weather (W6) hashes, wraps and falls by the same functions.
//
// It declares no binding: a program that includes it binds its emitters and instances as it likes.

// The most particles one emitter keeps alive at once (`MAX_STATELESS_SLOTS`).
const MAX_STATELESS_SLOTS: u32 = 4096u;
// How many cycles of a slot have numbers of their own (`STATELESS_CYCLES`).
const STATELESS_CYCLES: f32 = 256.0;
// An instance's stop time while it plays (`STATELESS_NEVER`).
const STATELESS_NEVER: f32 = 1e30;
const PARTICLES_TAU: f32 = 6.28318530717959;

// Source kinds, facings and draws, as `stateless.rs` numbers them.
const STATELESS_SOURCE_SPHERE: u32 = 1u;
const STATELESS_SOURCE_DISC: u32 = 2u;
const STATELESS_SOURCE_RING: u32 = 3u;
const STATELESS_SOURCE_CONE: u32 = 4u;
const STATELESS_SOURCE_BOX: u32 = 5u;
const STATELESS_SOURCE_LINE: u32 = 6u;
const STATELESS_FACING_CAMERA: u32 = 0u;
const STATELESS_DRAW_LIFETIME: u32 = 0u;
const STATELESS_DRAW_SPEED: u32 = 1u;
const STATELESS_DRAW_CONE_Z: u32 = 2u;
const STATELESS_DRAW_CONE_TURN: u32 = 3u;
const STATELESS_DRAW_SOURCE: u32 = 4u;
const STATELESS_DRAW_SIZE: u32 = 7u;
const STATELESS_DRAW_SPIN: u32 = 8u;
const STATELESS_DRAW_TURN: u32 = 9u;
// The second noise octave's offset in cells.
const STATELESS_OCTAVE_OFFSET: vec3<f32> = vec3<f32>(0.31, 0.57, 0.13);
// Over what share of a wrap box's half a particle fades.
const STATELESS_WRAP_FADE: f32 = 0.1;

// A stateless emitter (`StatelessEmitter`, 448 bytes).
struct StatelessEmitter {
    // x: slots; y: particles born together; z: kinds (source, facing << 4, noise octaves << 8,
    // shape << 12, light << 16); w: keys (size, alpha << 8, color << 16).
    counts: vec4<u32>,
    // x: seconds between groups' first births; y: a slot's period; z: when births end, 0 never;
    // w: delay.
    timing: vec4<f32>,
    // The lifetime's ends, then the speed's.
    lifetime_speed: vec4<f32>,
    // xyz: the unit direction they leave in; w: the cosine of the spread.
    direction: vec4<f32>,
    across_x: vec4<f32>,
    across_y: vec4<f32>,
    // The source's sizes.
    source: vec4<f32>,
    // Gravity, drag, the wind's response, the noise's strength.
    motion: vec4<f32>,
    // The noise's frequency and drift.
    noise: vec4<f32>,
    // The size factor's ends, then the spin's.
    scale_spin: vec4<f32>,
    // xyz: the wrap box's half extents; w: 1 if it wraps.
    wrap: vec4<f32>,
    // Additive, soft, a velocity facing's stretch.
    look: vec4<f32>,
    // Size and opacity keys, two (t, value) a vector; color keys, t then the color.
    size: array<vec4<f32>, 4>,
    alpha: array<vec4<f32>, 4>,
    color: array<vec4<f32>, 8>,
}

// One playing copy of an emitter (`StatelessInstance`, 80 bytes).
struct StatelessInstance {
    // xyz: where it plays; w: its age, seconds on its owner's clock.
    position: vec4<f32>,
    // How it's turned, a unit quaternion.
    rotation: vec4<f32>,
    // xyz: the wind where it plays; w: the age it was stopped at, STATELESS_NEVER while it plays.
    wind: vec4<f32>,
    // xyz: the wrap box's centre.
    wrap_center: vec4<f32>,
    // x: its seed; y: its emitter's place in the table.
    ids: vec4<u32>,
}

// A particle at one age (`StatelessParticle`).
struct StatelessParticle {
    alive: bool,
    position: vec3<f32>,
    velocity: vec3<f32>,
    half_width: f32,
    rotation: f32,
    color: vec3<f32>,
    alpha: f32,
    life: f32,
}

// The PCG hash (Jarzynski and Olano 2020; `particles_hash`): integers alone, so the CPU's to the bit.
fn particles_hash(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Number `draw` of the numbers `key`, uniform in [0, 1) (`particles_unit`).
fn particles_unit(key: u32, draw: u32) -> f32 {
    return f32(particles_hash(key + draw * 0x9E3779B9u) >> 8u) * (1.0 / 16777216.0);
}

// `position` wrapped into the box of `half` extents about `center` (`particles_wrap`).
fn particles_wrap(position: vec3<f32>, center: vec3<f32>, half: vec3<f32>) -> vec3<f32> {
    let span = half * 2.0;
    let offset = position - center;
    return center + (offset - span * floor(offset / span + 0.5));
}

// `vector` turned by the unit quaternion `q` (`rotate`).
fn particles_rotate(q: vec4<f32>, vector: vec3<f32>) -> vec3<f32> {
    let twice = cross(q.xyz, vector) * 2.0;
    return vector + twice * q.w + cross(q.xyz, twice);
}

// The sine and cosine of `turns` whole turns in [0, 1), as (sin, cos).
fn particles_turn(turns: f32) -> vec2<f32> {
    let angle = PARTICLES_TAU * turns;
    return vec2<f32>(sin(angle), cos(angle));
}

// `(e^z - 1) / z` and `(e^z - 1 - z) / z^2`: by their series inside |z| < 1 (the CPU's `PHI1` and
// `PHI2`, highest term first), else directly. `reach` is age times the first, `fall` age squared times
// the second (`particles_fall`).
fn particles_phi1(z: f32) -> f32 {
    if abs(z) < 1.0 {
        var sum = 2.7557319e-6;
        sum = sum * z + 2.4801587e-5;
        sum = sum * z + 1.984127e-4;
        sum = sum * z + 1.3888889e-3;
        sum = sum * z + 8.333333e-3;
        sum = sum * z + 4.1666668e-2;
        sum = sum * z + 0.16666667;
        sum = sum * z + 0.5;
        sum = sum * z + 1.0;
        return sum;
    }
    return (exp(z) - 1.0) / z;
}

fn particles_phi2(z: f32) -> f32 {
    if abs(z) < 1.0 {
        var sum = 2.755732e-7;
        sum = sum * z + 2.7557319e-6;
        sum = sum * z + 2.4801587e-5;
        sum = sum * z + 1.984127e-4;
        sum = sum * z + 1.3888889e-3;
        sum = sum * z + 8.333333e-3;
        sum = sum * z + 4.1666668e-2;
        sum = sum * z + 0.16666667;
        sum = sum * z + 0.5;
        return sum;
    }
    return (exp(z) - 1.0 - z) / (z * z);
}

// The ABC flow at `cells` (`abc_flow`): the curl of a field of sines, each component within +-1.
fn particles_abc_flow(cells: vec3<f32>) -> vec3<f32> {
    let x = particles_turn(cells.x - floor(cells.x));
    let y = particles_turn(cells.y - floor(cells.y));
    let z = particles_turn(cells.z - floor(cells.z));
    return vec3<f32>(z.x + y.y, x.x + z.y, y.x + x.y) * 0.5;
}

// Key `index` of a scalar curve packed two a vector.
fn stateless_scalar_key(keys: array<vec4<f32>, 4>, index: u32) -> vec2<f32> {
    var packed = keys;
    let pair = packed[index / 2u];
    return select(pair.xy, pair.zw, (index & 1u) == 1u);
}

// A scalar curve of `count` keys at `t` (`scalar_curve`, `sapphire_vfx`'s `Curve::sample`).
fn stateless_scalar_curve(keys: array<vec4<f32>, 4>, count: u32, t: f32) -> f32 {
    let keys_held = clamp(count, 1u, 8u);
    var previous = stateless_scalar_key(keys, 0u);
    if t <= previous.x {
        return previous.y;
    }
    for (var i = 1u; i < keys_held; i++) {
        let next = stateless_scalar_key(keys, i);
        if t <= next.x {
            let span = next.x - previous.x;
            var share = 1.0;
            if span > 0.0 {
                share = (t - previous.x) / span;
            }
            return previous.y + (next.y - previous.y) * share;
        }
        previous = next;
    }
    return previous.y;
}

// A color curve of `count` keys at `t` (`color_curve`).
fn stateless_color_curve(keys: array<vec4<f32>, 8>, count: u32, t: f32) -> vec3<f32> {
    var held = keys;
    let keys_held = clamp(count, 1u, 8u);
    var previous = held[0];
    if t <= previous.x {
        return previous.yzw;
    }
    for (var i = 1u; i < keys_held; i++) {
        let next = held[i];
        if t <= next.x {
            let span = next.x - previous.x;
            var share = 1.0;
            if span > 0.0 {
                share = (t - previous.x) / span;
            }
            return previous.yzw + (next.yzw - previous.yzw) * share;
        }
        previous = next;
    }
    return previous.yzw;
}

// Where a particle with numbers `key` is born in the effect's space (`source_point`).
fn stateless_source_point(emitter: StatelessEmitter, kind: u32, key: u32) -> vec3<f32> {
    let a = emitter.source.x;
    let b = emitter.source.y;
    let c = emitter.source.z;
    let u = particles_unit(key, STATELESS_DRAW_SOURCE);
    let v = particles_unit(key, STATELESS_DRAW_SOURCE + 1u);
    let w = particles_unit(key, STATELESS_DRAW_SOURCE + 2u);
    switch kind {
        case STATELESS_SOURCE_SPHERE: {
            let z = 2.0 * v - 1.0;
            let across = sqrt(max(1.0 - z * z, 0.0));
            let turn = particles_turn(w);
            return vec3<f32>(across * turn.y, across * turn.x, z) * (a * pow(u, 1.0 / 3.0));
        }
        case STATELESS_SOURCE_DISC: {
            let turn = particles_turn(u);
            return vec3<f32>(turn.y, 0.0, turn.x) * (a * sqrt(v));
        }
        case STATELESS_SOURCE_RING: {
            let turn = particles_turn(u);
            return vec3<f32>(turn.y, 0.0, turn.x) * (a + b * (v - 0.5));
        }
        case STATELESS_SOURCE_CONE: {
            let height = b * pow(u, 1.0 / 3.0);
            let turn = particles_turn(w);
            let reach = height * a * sqrt(v);
            return vec3<f32>(turn.y * reach, height, turn.x * reach);
        }
        case STATELESS_SOURCE_BOX: {
            return vec3<f32>((2.0 * u - 1.0) * a, (2.0 * v - 1.0) * b, (2.0 * w - 1.0) * c);
        }
        case STATELESS_SOURCE_LINE: {
            return vec3<f32>((u - 0.5) * a, 0.0, 0.0);
        }
        default: {
            return vec3<f32>(0.0);
        }
    }
}

// A particle that doesn't live: it collapses to nothing.
fn stateless_dead() -> StatelessParticle {
    return StatelessParticle(false, vec3<f32>(0.0), vec3<f32>(0.0), 0.0, 0.0, vec3<f32>(0.0), 0.0, 0.0);
}

// Particle `index` of `instance` at the instance's age (`StatelessEmitter::particle`).
fn stateless_particle(emitter: StatelessEmitter, instance: StatelessInstance, index: u32) -> StatelessParticle {
    let slots = emitter.counts.x;
    let group = emitter.counts.y;
    let kinds = emitter.counts.z;
    let keys = emitter.counts.w;
    let step = emitter.timing.x;
    let period = emitter.timing.y;
    let end = emitter.timing.z;
    let delay = emitter.timing.w;
    let time = instance.position.w - delay;
    let stop = instance.wind.w - delay;
    var limit = stop;
    if end > 0.0 {
        limit = min(end, stop);
    }
    // A slot past its emitter's, not yet born, or born only after births ended holds nothing.
    if index >= slots {
        return stateless_dead();
    }
    let first = f32(index / group) * step;
    if time < first || first >= limit {
        return stateless_dead();
    }
    var cycle = floor((time - first) / period);
    if limit < STATELESS_NEVER {
        cycle = min(cycle, ceil((limit - first) / period) - 1.0);
    }
    let age = max(time - (first + cycle * period), 0.0);
    let wrapped = cycle - STATELESS_CYCLES * floor(cycle / STATELESS_CYCLES);
    let key = particles_hash(particles_hash(instance.ids.x ^ particles_hash(index)) ^ u32(wrapped));
    let lifetime_low = emitter.lifetime_speed.x;
    let lifetime_high = emitter.lifetime_speed.y;
    let lifetime = lifetime_low + (lifetime_high - lifetime_low) * particles_unit(key, STATELESS_DRAW_LIFETIME);
    if age >= lifetime {
        return stateless_dead();
    }

    // Where and how it leaves, in the effect's space, then turned by the instance.
    let source = stateless_source_point(emitter, kinds & 15u, key);
    let cos_spread = emitter.direction.w;
    let cone_z = 1.0 - particles_unit(key, STATELESS_DRAW_CONE_Z) * (1.0 - cos_spread);
    let across = sqrt(max(1.0 - cone_z * cone_z, 0.0));
    let turn = particles_turn(particles_unit(key, STATELESS_DRAW_CONE_TURN));
    let direction = emitter.direction.xyz * cone_z
        + (emitter.across_x.xyz * turn.y + emitter.across_y.xyz * turn.x) * across;
    let speed_low = emitter.lifetime_speed.z;
    let speed_high = emitter.lifetime_speed.w;
    let speed = speed_low + (speed_high - speed_low) * particles_unit(key, STATELESS_DRAW_SPEED);
    let start = particles_rotate(instance.rotation, source);
    let launch = particles_rotate(instance.rotation, direction * speed);

    // Gravity, drag and the wind's pull, solved: v' = A - D v.
    let gravity = emitter.motion.x;
    let drag = emitter.motion.y;
    let response = emitter.motion.z;
    let strength = emitter.motion.w;
    let resistance = drag + response;
    let pull = vec3<f32>(0.0, -gravity, 0.0) + instance.wind.xyz * response;
    let z = -resistance * age;
    let reach = age * particles_phi1(z);
    let fall = age * age * particles_phi2(z);
    var position = start + launch * reach + pull * fall;
    let velocity = launch * exp(z) + pull * reach;
    let octaves = (kinds >> 8u) & 3u;
    if octaves > 0u {
        let frequency = emitter.noise.x;
        let drift = emitter.noise.y;
        let cells = position * frequency + vec3<f32>(0.0, -drift, drift * 0.5) * time;
        var flow = particles_abc_flow(cells);
        if octaves > 1u {
            flow += particles_abc_flow(cells * 2.0 + STATELESS_OCTAVE_OFFSET) * 0.5;
        }
        position += flow * (strength * fall);
    }
    position += instance.position.xyz;

    var fade = 1.0;
    if emitter.wrap.w > 0.0 {
        let half = emitter.wrap.xyz;
        let center = instance.wrap_center.xyz;
        let span = half * 2.0;
        let offset = position - center;
        let wrapped_offset = offset - span * floor(offset / span + 0.5);
        position = center + wrapped_offset;
        let edge = clamp((half - abs(wrapped_offset)) / (half * STATELESS_WRAP_FADE), vec3<f32>(0.0), vec3<f32>(1.0));
        fade = min(min(edge.x, edge.y), edge.z);
    }

    let life = age / lifetime;
    let scale_low = emitter.scale_spin.x;
    let scale_high = emitter.scale_spin.y;
    let half_width = stateless_scalar_curve(emitter.size, keys & 15u, life)
        * (scale_low + (scale_high - scale_low) * particles_unit(key, STATELESS_DRAW_SIZE));
    var rotation = 0.0;
    if ((kinds >> 4u) & 3u) == STATELESS_FACING_CAMERA {
        let spin_low = emitter.scale_spin.z;
        let spin_high = emitter.scale_spin.w;
        let spin = spin_low + (spin_high - spin_low) * particles_unit(key, STATELESS_DRAW_SPIN);
        let turns = particles_unit(key, STATELESS_DRAW_TURN) + spin * age * (1.0 / PARTICLES_TAU);
        rotation = PARTICLES_TAU * (turns - floor(turns));
    }
    return StatelessParticle(
        true,
        position,
        velocity,
        half_width,
        rotation,
        stateless_color_curve(emitter.color, (keys >> 16u) & 15u, life),
        stateless_scalar_curve(emitter.alpha, (keys >> 8u) & 15u, life) * fade,
        life,
    );
}
