// Post-processing (spec §5.12 "Post-processing"): the bloom chain and the fused composite.
// Unlike the scene passes this file stands alone; none of frame.wgsl's groups are bound.
//
// Bind groups:
//   0  post   constants (`PostUniforms`, written in `prepare`) and a linear clamp sampler
//   1  pass   the textures one pass reads:
//
//      pass        texture0                     texture1               lut_a, lut_b
//      prefilter   scene color (full size)      —                      —
//      down        the previous, larger level   —                      —
//      up          the next, smaller level      this size's down level —
//      composite   scene color                  bloom (half size)      the two grading LUTs
//
// Every pass draws one fullscreen triangle and writes every pixel. Bloom levels hold
// pre-exposed light (exposure × scene), so thresholds don't move when exposure does. Sizes are
// the scene's, which is the target's at render scale 1; only the composite writes the target.

// Mirrors `post::PostUniforms`.
struct PostUniforms {
    // x: exposure multiplier; y: bloom intensity; z: LUT blend (0: A alone, 1: B alone);
    // w: 1 when scene color is smaller than the target (render scale below 1).
    composite: vec4<f32>,
    // x: threshold; y: knee width (threshold × knee); z: 1 / (4 × knee width); w: scatter.
    bloom: vec4<f32>,
    // The LUT shaper: x: 1 / ε; y: 1 / log2(1 + max / ε); z: (size - 1) / size; w: 0.5 / size.
    lut: vec4<f32>,
    // x: vignette intensity; y: start radius; z: 1 / (1 - start).
    vignette: vec4<f32>,
}

@group(0) @binding(0) var<uniform> post: PostUniforms;
@group(0) @binding(1) var linear_clamp: sampler;
@group(1) @binding(0) var texture0: texture_2d<f32>;
@group(1) @binding(1) var texture1: texture_2d<f32>;
@group(1) @binding(2) var lut_a: texture_3d<f32>;
@group(1) @binding(3) var lut_b: texture_3d<f32>;

struct FullscreenVaryings {
    @builtin(position) position: vec4<f32>,
    // 0 at the top-left corner of the target, 1 at the bottom-right.
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> FullscreenVaryings {
    // Vertices (-1, -1), (3, -1), (-1, 3) cover the target with one triangle.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: FullscreenVaryings;
    out.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

fn max3(c: vec3<f32>) -> f32 {
    return max(c.r, max(c.g, c.b));
}

fn sample0(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(texture0, linear_clamp, uv, 0.0).rgb;
}

// Soft-knee threshold: a quadratic fade-in over threshold ± knee width, linear above. Scaling
// the whole color keeps its hue.
fn bloom_threshold(color: vec3<f32>) -> vec3<f32> {
    let brightness = max3(color);
    let knee = post.bloom.y;
    let soft = clamp(brightness - post.bloom.x + knee, 0.0, 2.0 * knee);
    let contribution = max(soft * soft * post.bloom.z, brightness - post.bloom.x);
    return color * (contribution / max(brightness, 1e-4));
}

// The dual-filter downsample (Bjørge, SIGGRAPH 2015): the 2×2 block under the destination
// pixel, weighted 4, plus four diagonal blocks one source texel out, over a 4×4 footprint.
// Each tap is one bilinear sample, so five taps cover sixteen texels.
struct DownTaps {
    center: vec3<f32>,
    corners: array<vec3<f32>, 4>,
}

fn down_taps(uv: vec2<f32>) -> DownTaps {
    let texel = 1.0 / vec2<f32>(textureDimensions(texture0));
    var taps: DownTaps;
    taps.center = sample0(uv);
    taps.corners[0] = sample0(uv + vec2<f32>(-1.0, -1.0) * texel);
    taps.corners[1] = sample0(uv + vec2<f32>(1.0, -1.0) * texel);
    taps.corners[2] = sample0(uv + vec2<f32>(-1.0, 1.0) * texel);
    taps.corners[3] = sample0(uv + vec2<f32>(1.0, 1.0) * texel);
    return taps;
}

// Exposure and the threshold, then the tap's Karis weight, 1 / (1 + brightness).
fn prefilter_tap(color: vec3<f32>, weight: f32) -> vec4<f32> {
    let bright = bloom_threshold(color * post.composite.x);
    let karis = weight / (1.0 + max3(bright));
    return vec4<f32>(bright * karis, karis);
}

// The first level: exposure and the threshold per tap, then a Karis average, so a lone very
// bright pixel can't flicker into a big blob of bloom as it moves.
@fragment
fn fs_bloom_prefilter(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let taps = down_taps(in.uv);
    let sum = prefilter_tap(taps.center, 4.0) + prefilter_tap(taps.corners[0], 1.0)
        + prefilter_tap(taps.corners[1], 1.0) + prefilter_tap(taps.corners[2], 1.0)
        + prefilter_tap(taps.corners[3], 1.0);
    return vec4<f32>(sum.rgb / sum.a, 1.0);
}

@fragment
fn fs_bloom_down(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let taps = down_taps(in.uv);
    let corners = taps.corners[0] + taps.corners[1] + taps.corners[2] + taps.corners[3];
    return vec4<f32>((4.0 * taps.center + corners) / 8.0, 1.0);
}

// The dual-filter upsample: a tent over the smaller level, from four taps one texel out along
// the axes (weight 1) and four half a texel out on the diagonals (weight 2). The result
// replaces `scatter` of this size's own level, so wider levels add their glow while the total
// light stays the same.
@fragment
fn fs_bloom_up(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(texture0));
    var wide = sample0(in.uv + vec2<f32>(-1.0, 0.0) * texel) + sample0(in.uv + vec2<f32>(1.0, 0.0) * texel)
        + sample0(in.uv + vec2<f32>(0.0, -1.0) * texel) + sample0(in.uv + vec2<f32>(0.0, 1.0) * texel);
    wide += 2.0 * (sample0(in.uv + vec2<f32>(-0.5, -0.5) * texel) + sample0(in.uv + vec2<f32>(0.5, -0.5) * texel)
        + sample0(in.uv + vec2<f32>(-0.5, 0.5) * texel) + sample0(in.uv + vec2<f32>(0.5, 0.5) * texel));
    let own = textureLoad(texture1, vec2<i32>(in.position.xy), 0).rgb;
    return vec4<f32>(mix(own, wide / 12.0, post.bloom.w), 1.0);
}

// Same as frame.wgsl's; post passes don't include it. WESL imports will share it (M1).
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

// 1 in the middle, falling smoothly toward 1 - intensity at the corners, round at any aspect.
fn vignette(uv: vec2<f32>, size: vec2<f32>) -> f32 {
    let radius = length((uv - 0.5) * size) / length(0.5 * size);
    let t = saturate((radius - post.vignette.y) * post.vignette.z);
    return 1.0 - post.vignette.x * t * t * (3.0 - 2.0 * t);
}

// The fused pass (spec §3.3: bloom, tonemap and LUT in one pass on mobile): exposure, bloom,
// vignette, then the shaper into the two grading LUTs, which hold the tone curve and grade.
// Below render scale 1 it is also the upsample to the target.
@fragment
fn fs_post(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(texture0));
    // A uniform branch: at render scale 1 scene color is loaded texel for texel, as it always
    // was; a smaller scene is filtered up bilinearly at the pixel's position.
    var scene: vec3<f32>;
    if post.composite.w > 0.0 {
        scene = textureSampleLevel(texture0, linear_clamp, in.uv, 0.0).rgb;
    } else {
        scene = textureLoad(texture0, vec2<i32>(in.position.xy), 0).rgb;
    }
    var color = scene * post.composite.x;
    color += textureSampleLevel(texture1, linear_clamp, in.uv, 0.0).rgb * post.composite.y;
    color *= vignette(in.uv, size);

    let shaped = log2(1.0 + max(color, vec3<f32>(0.0)) * post.lut.x) * post.lut.y;
    let coordinate = shaped * post.lut.z + post.lut.w;
    var display = textureSampleLevel(lut_a, linear_clamp, coordinate, 0.0).rgb;
    // A uniform branch: grade B costs a second LUT fetch only while it is blending in, and
    // weights 0 and 1 show each LUT exactly.
    let blend = post.composite.z;
    if blend > 0.0 {
        let display_b = textureSampleLevel(lut_b, linear_clamp, coordinate, 0.0).rgb;
        display = select(mix(display, display_b, blend), display_b, blend >= 1.0);
    }
    // The LUT holds sRGB-encoded values; the sRGB target encodes what this returns.
    return vec4<f32>(srgb_to_linear(display), 1.0);
}
