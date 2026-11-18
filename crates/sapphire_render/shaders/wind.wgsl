// The wind field (M1 W6, M6 Y1; `sapphire_look::wind`, whose docs have the model): the one sampler
// every reader of the air shares, operation for operation the CPU's (`WindState::gust`,
// `WindState::turbulence`, `WindVolumeState::air`, `ResolvedWind::local` and `ResolvedWind::air`):
// the global wind and its gusts, turbulence as a share of its strength, and the volumes alive this
// frame.
//
// The program that includes this file binds the frame's wind as `wind: WindUniforms` in a group of
// its own (foliage's scene group, binding 2), and sets `WIND_VOLUMES`, the most volumes its tier's
// GPU readers loop over (`WindSettings::volumes`): with 0 the loop compiles out. Turbulence and the
// volumes add nothing, exactly, at turbulence 0 with no volume reaching a point, so a field of the
// global wind alone draws as the wind drew before them.

// The wrapped time's period in seconds (`wind::WIND_PERIOD`): every oscillator runs a whole number of
// cycles in it, so the time wraps without a jump.
const WIND_PERIOD: f32 = 1024.0;
// The gusts' swell, in cycles per second.
const WIND_GUST_HZ: f32 = 0.09;
// Meters between gust fronts traveling downwind.
const WIND_GUST_WAVELENGTH: f32 = 36.0;
// Turbulence's waves (`wind::TURBULENCE_WAVES`): xy the wave's number in cycles per meter of x and z;
// zw the air's flow along its crests, its length the wave's share.
const WIND_TURBULENCE = array<vec4<f32>, 4>(
    vec4<f32>(0.0336, 0.0252, 0.21, -0.28),
    vec4<f32>(-0.0117, 0.04, 0.336, 0.098),
    vec4<f32>(-0.0667, -0.0889, -0.12, 0.09),
    vec4<f32>(0.1067, -0.0311, -0.042, -0.144),
);
// How fast each wave turns over, in cycles per second (`wind::TURBULENCE_HZ`).
const WIND_TURBULENCE_HZ: vec4<f32> = vec4<f32>(0.05, 0.07, 0.13, 0.17);
// The most volumes the buffer holds a frame (`wind::MAX_WIND_VOLUMES`).
const MAX_WIND_VOLUMES: u32 = 8u;
// What a vortex's axis is held above where the sampler divides by its length squared: a burst's is
// zero (`wind::AXIS_FLOOR`).
const WIND_AXIS_FLOOR: f32 = 1e-30;
const TAU: f32 = 6.28318530717959;

// The most volumes this tier's GPU readers loop over (`WindSettings::volumes`), at most
// MAX_WIND_VOLUMES.
override WIND_VOLUMES: u32 = 8u;

// The global wind at one frame (`wind::WindState`).
struct WindState {
    // xyz: unit horizontal direction the wind blows toward; w: strength, 1 the materials' unit
    // wind, 0 calm.
    direction: vec4<f32>,
    // x: time in seconds, wrapped to WIND_PERIOD; y: gustiness, 0 to 1; z: turbulence, 0 to 1;
    // w: how many of the frame's volumes are live, as a float.
    time: vec4<f32>,
}

// A sphere of local air (`wind::WindVolumeState`).
struct WindVolume {
    // xyz: its centre; w: its radius.
    center: vec4<f32>,
    // xyz: the air it blows everywhere inside it; w: its falloff.
    velocity: vec4<f32>,
    // xyz: its axis times its turning speed; w: its radial speed.
    swirl: vec4<f32>,
}

// This frame's wind and the last drawn frame's, for velocity, then each one's volumes: this frame's
// from 0, the last drawn frame's from MAX_WIND_VOLUMES (`wind::WindBuffer`, 832 bytes).
struct WindUniforms {
    current: WindState,
    previous: WindState,
    volumes: array<WindVolume, 16>,
}

// Cycles of an oscillator at `hz` at wrapped time `t`, in [0, 1). The frequency is rounded to a
// whole number of cycles per WIND_PERIOD (steps of 1/1024 Hz), so `t` wrapping changes nothing.
fn wind_cycles(hz: f32, t: f32) -> f32 {
    return fract(round(hz * WIND_PERIOD) / WIND_PERIOD * t);
}

// How much the gusts of `state` swell its strength at `position`: a slow swell traveling downwind,
// so a field ripples as fronts pass.
fn wind_gust(state: WindState, position: vec3<f32>) -> f32 {
    let direction = state.direction.xyz;
    let t = state.time.x;
    // Phases are taken in cycles and wrapped before they become angles, so `sin` stays precise
    // however far from the origin the point is.
    let downwind = dot(position.xz, direction.xz) / WIND_GUST_WAVELENGTH;
    return 1.0 + state.time.y * sin(TAU * fract(wind_cycles(WIND_GUST_HZ, t) - downwind));
}

// One of turbulence's waves at `position`: its flow times the cosine of its phase.
fn wind_wave(wave: vec4<f32>, hz: f32, position: vec3<f32>, t: f32) -> vec2<f32> {
    return wave.zw * cos(TAU * fract(dot(position.xz, wave.xy) + wind_cycles(hz, t)));
}

// Turbulence's air at `position`, horizontal: the strength times the share times the four waves'
// flows. Zero, by a multiply, with no turbulence.
fn wind_turbulence(state: WindState, position: vec3<f32>) -> vec3<f32> {
    let t = state.time.x;
    var flow = wind_wave(WIND_TURBULENCE[0], WIND_TURBULENCE_HZ.x, position, t);
    flow += wind_wave(WIND_TURBULENCE[1], WIND_TURBULENCE_HZ.y, position, t);
    flow += wind_wave(WIND_TURBULENCE[2], WIND_TURBULENCE_HZ.z, position, t);
    flow += wind_wave(WIND_TURBULENCE[3], WIND_TURBULENCE_HZ.w, position, t);
    let scale = state.direction.w * state.time.z;
    return vec3<f32>(flow.x, 0.0, flow.y) * scale;
}

// A volume's air at `position`: its velocity, its turn about its axis and its flow from it, by the
// offset across the axis over the radius, times a smoothstep from its radius in by its falloff.
fn wind_volume_air(volume: WindVolume, position: vec3<f32>) -> vec3<f32> {
    let radius = volume.center.w;
    let r = position - volume.center.xyz;
    let axis = volume.swirl.xyz;
    let across = r - axis * (dot(r, axis) / max(dot(axis, axis), WIND_AXIS_FLOOR));
    let x = sqrt(dot(r, r)) / radius;
    let edge = saturate((1.0 - x) / volume.velocity.w);
    let shape = edge * edge * (3.0 - 2.0 * edge);
    return (volume.velocity.xyz + (cross(axis, across) + across * volume.swirl.w) / radius) * shape;
}

// Whether `state` has air beyond the global wind: turbulence, or a live volume this tier's readers
// take. Uniform across a draw, so a reader may branch on it and a frame without local air skips it.
fn wind_has_local(state: WindState) -> bool {
    return state.time.z > 0.0 || (WIND_VOLUMES > 0u && state.time.w > 0.0);
}

// The air beyond the global wind of `state` at `position`: turbulence, then its live volumes, at
// most WIND_VOLUMES, read from `wind.volumes` at `first` (0 for this frame's, MAX_WIND_VOLUMES for
// the last drawn frame's).
fn wind_local(state: WindState, first: u32, position: vec3<f32>) -> vec3<f32> {
    var air = wind_turbulence(state, position);
    let count = min(u32(state.time.w), min(WIND_VOLUMES, MAX_WIND_VOLUMES));
    for (var i = 0u; i < count; i++) {
        air += wind_volume_air(wind.volumes[first + i], position);
    }
    return air;
}

// The air's velocity at `position`: the global wind's direction times its gusted strength, and the
// air beyond it.
fn wind_air(state: WindState, first: u32, position: vec3<f32>) -> vec3<f32> {
    let direction = vec3<f32>(state.direction.x, 0.0, state.direction.z);
    return direction * (state.direction.w * wind_gust(state, position)) + wind_local(state, first, position);
}
