// Light shafts from a low sun (M5 L13; `sapphire_render::post`'s `shafts` module). Appended to post.wgsl, whose
// code is unchanged: a frame without shafts draws post.wgsl's own entry points, so these never run there.
//
// Three passes: the sky the frame shows, at an eighth of post's input (`fs_shaft_mask`, from the scene prepass's
// depth, where only open sky stays at the far plane); the march at a quarter (`fs_shaft_march`), each pixel's
// share of open sky along its line to the sun, fading with its distance from the sun; and bloom's top up level
// drawn with the shafts added once, after bloom's own blur and threshold (`fs_bloom_up_shafts`), so the fused
// pass's add of bloom puts them into the exposed frame before the grade.

// Mirrors `post::shafts::ShaftUniforms`.
struct ShaftUniforms {
    // xy: the sun's place in texture coordinates; z: how far the shafts reach, in frame heights; w: the frame's
    // width over its height.
    sun: vec4<f32>,
    // rgb: the shafts' light at the sun, over bloom's intensity, faded as the sun leaves the frame or sets; w: the
    // march's taps.
    light: vec4<f32>,
    // xy: one mask texel, in texture coordinates.
    mask: vec4<f32>,
}

@group(0) @binding(4) var<uniform> shafts: ShaftUniforms;
// The scene prepass's depth (reverse-Z, cleared to 0: open sky).
@group(1) @binding(8) var shaft_depth: texture_depth_2d;
// The march's result, a quarter of post's input: the share of the shafts' light each place takes.
@group(1) @binding(9) var shaft_light: texture_2d<f32>;

// Points a mask texel tests, along each axis of its footprint: sixteen in all.
const SHAFT_MASK_POINTS: u32 = 4u;
// How much weaker the far end of a pixel's line (the sun's) counts than its near end: the falloff along it.
const SHAFT_FAR_WEIGHT: f32 = 0.5;
// `post::shafts::MIN_TAPS` and `MAX_TAPS`.
const SHAFT_MIN_TAPS: u32 = 8u;
const SHAFT_MAX_TAPS: u32 = 160u;

// The share of the mask texel's footprint that is open sky: the prepass's depth at a grid of points across it.
@fragment
fn fs_shaft_mask(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let depth_size = vec2<f32>(textureDimensions(shaft_depth));
    let mask_texel = shafts.mask.xy;
    let last = vec2<i32>(depth_size) - 1;
    var open = 0.0;
    for (var row = 0u; row < SHAFT_MASK_POINTS; row++) {
        for (var column = 0u; column < SHAFT_MASK_POINTS; column++) {
            let cell = (vec2<f32>(f32(column), f32(row)) + 0.5) / f32(SHAFT_MASK_POINTS) - 0.5;
            let uv = in.uv + cell * mask_texel;
            let texel = clamp(vec2<i32>(uv * depth_size), vec2<i32>(0), last);
            open += select(0.0, 1.0, textureLoad(shaft_depth, texel, 0) <= 0.0);
        }
    }
    return vec4<f32>(open / f32(SHAFT_MASK_POINTS * SHAFT_MASK_POINTS), 0.0, 0.0, 1.0);
}

// The share of the shafts' light at a pixel: open sky's share along the line from it to the sun, its taps
// weighted from 1 at the pixel to `SHAFT_FAR_WEIGHT` at the sun and normalised, so a line all open sky takes
// exactly 1; times a fade with the pixel's distance from the sun, to none at the shafts' length. Each pixel's
// taps start at a fixed per-pixel offset, the same every frame, so the few taps read as grain at an occluder's
// edge rather than as bands, and nothing crawls on a still camera.
@fragment
fn fs_shaft_march(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let aspect = shafts.sun.w;
    let toward = shafts.sun.xy - in.uv;
    let distance = length(toward * vec2<f32>(aspect, 1.0));
    let near = saturate(1.0 - distance / shafts.sun.z);
    // Past the shafts' length nothing reaches: no march.
    if near <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    // `post::shafts::taps`'s, held to its range so no constant can make the loop long or empty.
    let taps = clamp(u32(shafts.light.w), SHAFT_MIN_TAPS, SHAFT_MAX_TAPS);
    let step = toward / f32(taps);
    let jitter = interleaved_noise(in.position.xy);
    var open = 0.0;
    var weights = 0.0;
    for (var tap = 0u; tap < taps; tap++) {
        let along = (f32(tap) + jitter) / f32(taps);
        let weight = 1.0 - (1.0 - SHAFT_FAR_WEIGHT) * along;
        let sky = textureSampleLevel(texture0, linear_clamp, in.uv + step * (f32(tap) + jitter), 0.0).r;
        open += sky * weight;
        weights += weight;
    }
    return vec4<f32>(near * near * (open / weights), 0.0, 0.0, 1.0);
}

// `fs_bloom_up` of bloom's top level, with the shafts added after its blur: the march's share, filtered at the
// pixel, times their light over bloom's intensity, so the fused pass's `bloom × intensity` adds their light, held
// to what a bloom level holds.
@fragment
fn fs_bloom_up_shafts(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(texture0));
    var wide = sample0(in.uv + vec2<f32>(-1.0, 0.0) * texel) + sample0(in.uv + vec2<f32>(1.0, 0.0) * texel)
        + sample0(in.uv + vec2<f32>(0.0, -1.0) * texel) + sample0(in.uv + vec2<f32>(0.0, 1.0) * texel);
    wide += 2.0 * (sample0(in.uv + vec2<f32>(-0.5, -0.5) * texel) + sample0(in.uv + vec2<f32>(0.5, -0.5) * texel)
        + sample0(in.uv + vec2<f32>(-0.5, 0.5) * texel) + sample0(in.uv + vec2<f32>(0.5, 0.5) * texel));
    let own = textureLoad(texture1, vec2<i32>(in.position.xy), 0).rgb;
    let share = textureSampleLevel(shaft_light, linear_clamp, in.uv, 0.0).r;
    let bloom = mix(own, wide / 12.0, post.bloom.w);
    return vec4<f32>(min(bloom + share * shafts.light.rgb, vec3<f32>(BLOOM_MAX)), 1.0);
}
