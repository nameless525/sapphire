// Post-processing (spec §5.12 "Post-processing"): the bloom chain and the fused composite.
// Unlike the scene passes this file stands alone; none of frame.wgsl's groups are bound.
//
// Bind groups:
//   0  post   constants (`PostUniforms`, written in `prepare`) and a linear clamp sampler
//   1  pass   the textures one pass reads:
//
//      pass        texture0                     texture1               lut_a, lut_b   diffusion_level
//      prefilter   post's input (full size)     scene color (its alpha) —              —
//      down        the previous, larger level   —                      —              —
//      up          the next, smaller level      this size's down level —              —
//      composite   post's input                 bloom (half size)      the two LUTs   bloom's widest
//
// Post's input is the main pass's scene color, or temporal AA's resolve of it. Every pass draws
// one fullscreen triangle and writes every pixel. Bloom levels hold pre-exposed light (exposure
// × scene), so thresholds don't move when exposure does. Sizes are the scene's, which is the
// target's at render scale 1; only the composite writes the target.
//
// The compositing layers (M1 P2) are pipeline variants: `POST_LAYERS` (the diffusion glow, the
// gradient layers, the lens flare and the vignette's tint) and `POST_SHARPEN` (RCAS) in the
// composite, `POST_CHARACTERS` (the character mask) in the prefilter. With all three off, the
// passes run the code they ran before the layers.

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
    // The compositing layers' (M1 P2), appended.
    // rgb: the color the vignette's corners go toward, linear.
    vignette_color: vec4<f32>,
    // Characters' bloom: xyz as `bloom`'s, with their own threshold; w: their strength relative
    // to the world's.
    character_bloom: vec4<f32>,
    // rgb: the diffusion glow's color, linear; w: its intensity (0: none).
    diffusion: vec4<f32>,
    // The diffusion's soft threshold, as `bloom`'s xyz.
    diffusion_threshold: vec4<f32>,
    // x: the frame's width / height; yz: one output pixel in texture coordinates; w: sharpening,
    // the RCAS lobe's scale (0: none).
    frame: vec4<f32>,
    // xy: the sun's disc's center in texture coordinates; z: its radius in frame heights (0: no
    // flare); w: the exposed luminance at which a disc pixel counts as fully visible.
    flare_sun: vec4<f32>,
    // rgb: the lens flare's color × intensity, linear.
    flare: vec4<f32>,
    // x: gradient layers in use.
    counts: vec4<u32>,
    gradients: array<GradientLayer, MAX_GRADIENT_LAYERS>,
    flare_elements: array<FlareElement, FLARE_ELEMENTS>,
}

// Mirrors `post::layers::GpuGradient`.
struct GradientLayer {
    // rgb: color, sRGB-encoded; w: opacity (times the player's filter strength).
    color: vec4<f32>,
    // xy: a linear layer's direction into the frame (over the frame's extent along it), or a
    // radial one's center; z: where it has faded out; w: 1 / (start - end).
    geometry: vec4<f32>,
    // x: the shape (GRADIENT_*); y: how it blends (BLEND_*).
    mode: vec4<u32>,
    // The affine modes' blend, `a × scale + offset`, worked out on the CPU from the color.
    scale: vec4<f32>,
    offset: vec4<f32>,
}

// Mirrors `post::layers::GpuFlareElement`.
struct FlareElement {
    // x: position along the axis from the sun (0) through the middle (0.5) to its mirror (1);
    // y: radius in frame heights; z: softness; w: the kind (FLARE_*).
    shape: vec4<f32>,
    // rgb: linear color.
    color: vec4<f32>,
    // Where it draws anything, in frame heights from its center: xy the half-extents of a box
    // around it; z the radius of a hole in its middle (a ring's).
    reach: vec4<f32>,
}

const MAX_GRADIENT_LAYERS: u32 = 4u;
const FLARE_ELEMENTS: u32 = 8u;

// `GradientShape`.
const GRADIENT_LINEAR: u32 = 0u;
const GRADIENT_RADIAL: u32 = 1u;
// How a gradient layer blends (`layers::BlendKind`): normal, multiply, screen and add are affine in
// the frame's value, `a × scale + offset` clamped to white; overlay and soft light are curves.
const BLEND_AFFINE: u32 = 0u;
const BLEND_OVERLAY: u32 = 1u;
const BLEND_SOFT_LIGHT: u32 = 2u;
// `layers::FlareKind`.
const FLARE_DISC: u32 = 0u;
const FLARE_RING: u32 = 1u;
const FLARE_HEXAGON: u32 = 2u;
const FLARE_STREAK: u32 = 3u;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// FSR 1's RCAS limit, 1/4 - 1/16: the strongest negative lobe sharpening uses.
const RCAS_LIMIT: f32 = 0.1875;

// The pipeline variants (see the header).
override POST_LAYERS: bool = false;
override POST_SHARPEN: bool = false;
override POST_CHARACTERS: bool = false;

@group(0) @binding(0) var<uniform> post: PostUniforms;
@group(0) @binding(1) var linear_clamp: sampler;
@group(1) @binding(0) var texture0: texture_2d<f32>;
@group(1) @binding(1) var texture1: texture_2d<f32>;
@group(1) @binding(2) var lut_a: texture_3d<f32>;
@group(1) @binding(3) var lut_b: texture_3d<f32>;
@group(1) @binding(4) var diffusion_level: texture_2d<f32>;

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

// Soft-knee threshold: a quadratic fade-in over threshold ± knee width, linear above, with
// `shape`'s x the threshold, y the knee width and z 1 / (4 × knee width). Scaling the whole
// color keeps its hue.
fn soft_threshold(color: vec3<f32>, shape: vec4<f32>) -> vec3<f32> {
    let brightness = max3(color);
    let knee = shape.y;
    let soft = clamp(brightness - shape.x + knee, 0.0, 2.0 * knee);
    let contribution = max(soft * soft * shape.z, brightness - shape.x);
    return color * (contribution / max(brightness, 1e-4));
}

// The world's bloom threshold.
fn bloom_threshold(color: vec3<f32>) -> vec3<f32> {
    return soft_threshold(color, post.bloom);
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

// The world's share of the pixel at `uv`: scene color's alpha, 0 on characters (the character
// mask; see the post module's docs).
fn world_at(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(texture1, linear_clamp, uv, 0.0).a;
}

// `prefilter_tap` where `world` of the pixel is the world's: the rest takes the characters'
// threshold and strength.
fn character_tap(color: vec3<f32>, world: f32, weight: f32) -> vec4<f32> {
    let exposed = color * post.composite.x;
    let character = soft_threshold(exposed, post.character_bloom) * post.character_bloom.w;
    let bright = mix(character, bloom_threshold(exposed), saturate(world));
    let karis = weight / (1.0 + max3(bright));
    return vec4<f32>(bright * karis, karis);
}

// The first level: exposure and the threshold per tap, then a Karis average, so a lone very
// bright pixel can't flicker into a big blob of bloom as it moves.
@fragment
fn fs_bloom_prefilter(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let taps = down_taps(in.uv);
    if POST_CHARACTERS {
        // The mask is read at the taps' own coordinates, whatever its size.
        let texel = 1.0 / vec2<f32>(textureDimensions(texture0));
        let sum = character_tap(taps.center, world_at(in.uv), 4.0)
            + character_tap(taps.corners[0], world_at(in.uv + vec2<f32>(-1.0, -1.0) * texel), 1.0)
            + character_tap(taps.corners[1], world_at(in.uv + vec2<f32>(1.0, -1.0) * texel), 1.0)
            + character_tap(taps.corners[2], world_at(in.uv + vec2<f32>(-1.0, 1.0) * texel), 1.0)
            + character_tap(taps.corners[3], world_at(in.uv + vec2<f32>(1.0, 1.0) * texel), 1.0);
        return vec4<f32>(sum.rgb / sum.a, 1.0);
    }
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

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let clamped = saturate(c);
    let low = clamped * 12.92;
    let high = 1.055 * pow(clamped, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, clamped <= vec3<f32>(0.0031308));
}

// How far the vignette reaches at `uv`, 0 in the middle to 1 at the corners, round at any
// aspect.
fn vignette_reach(uv: vec2<f32>, size: vec2<f32>) -> f32 {
    let radius = length((uv - 0.5) * size) / length(0.5 * size);
    let t = saturate((radius - post.vignette.y) * post.vignette.z);
    return t * t * (3.0 - 2.0 * t);
}

// 1 in the middle, falling smoothly toward 1 - intensity at the corners, round at any aspect.
fn vignette(uv: vec2<f32>, size: vec2<f32>) -> f32 {
    return 1.0 - post.vignette.x * vignette_reach(uv, size);
}

// The scene at the pixel, before exposure. A uniform branch: at render scale 1 scene color is
// loaded texel for texel, as it always was; a smaller scene is filtered up bilinearly at the
// pixel's position.
fn scene_at(position: vec2<f32>, uv: vec2<f32>) -> vec3<f32> {
    if post.composite.w > 0.0 {
        return textureSampleLevel(texture0, linear_clamp, uv, 0.0).rgb;
    }
    return textureLoad(texture0, vec2<i32>(position), 0).rgb;
}

// Exposed scene-linear light to display values (sRGB-encoded): the shaper into the two grading
// LUTs, which hold the tone curve and grade.
fn graded(color: vec3<f32>) -> vec3<f32> {
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
    return display;
}

// The fused pass (spec §3.3: bloom, tonemap and LUT in one pass on mobile): exposure, bloom,
// vignette, then the shaper into the two grading LUTs, which hold the tone curve and grade.
// Below render scale 1 it is also the upsample to the target. This is the plain variant; the
// compositing layers' variants draw `fs_composite`.
@fragment
fn fs_post(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(texture0));
    var color = scene_at(in.position.xy, in.uv) * post.composite.x;
    color += textureSampleLevel(texture1, linear_clamp, in.uv, 0.0).rgb * post.composite.y;
    color *= vignette(in.uv, size);
    // The LUT holds sRGB-encoded values; the sRGB target encodes what this returns.
    return vec4<f32>(srgb_to_linear(graded(color)), 1.0);
}

struct CompositeVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // How much of the sun's disc the frame shows, 0 to 1: the lens flare's brightness, measured
    // once a frame.
    @location(1) @interpolate(flat) flare: f32,
}

// How visible a point of the sun's disc is: the frame's exposed brightness there over what a
// fully visible disc shows, 0 off screen.
fn disc_tap(uv: vec2<f32>) -> f32 {
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return 0.0;
    }
    let light = dot(textureSampleLevel(texture0, linear_clamp, uv, 0.0).rgb, LUMA) * post.composite.x;
    return saturate(light / post.flare_sun.w);
}

// How much of the sun's disc the frame shows: five taps within half its radius, so whatever
// stands in front of the sun (a character, the world, a cloud) dims the flare by how much of the
// disc it covers, and a sun off screen shows none.
fn flare_visibility() -> f32 {
    let sun = post.flare_sun;
    if sun.z <= 0.0 || sun.w <= 0.0 {
        return 0.0;
    }
    let reach = vec2<f32>(sun.z * 0.5 / post.frame.x, sun.z * 0.5);
    let taps = disc_tap(sun.xy) + disc_tap(sun.xy + vec2<f32>(reach.x, 0.0))
        + disc_tap(sun.xy - vec2<f32>(reach.x, 0.0)) + disc_tap(sun.xy + vec2<f32>(0.0, reach.y))
        + disc_tap(sun.xy - vec2<f32>(0.0, reach.y));
    return taps / 5.0;
}

// The compositing variants' vertex shader: the fullscreen triangle, and the flare's visibility,
// measured once for the frame rather than per pixel.
@vertex
fn vs_composite(@builtin(vertex_index) index: u32) -> CompositeVaryings {
    // The fullscreen triangle, as `vs_fullscreen` draws it.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: CompositeVaryings;
    out.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    out.flare = 0.0;
    if POST_LAYERS {
        out.flare = flare_visibility();
    }
    return out;
}

// The scene `offset` output pixels from the pixel, before exposure, clamped to the frame: a
// neighbor for sharpening.
fn scene_tap(position: vec2<f32>, uv: vec2<f32>, offset: vec2<f32>) -> vec3<f32> {
    if post.composite.w > 0.0 {
        return textureSampleLevel(texture0, linear_clamp, uv + offset * post.frame.yz, 0.0).rgb;
    }
    let last = vec2<i32>(textureDimensions(texture0)) - 1;
    let pixel = clamp(vec2<i32>(position + offset), vec2<i32>(0), last);
    return textureLoad(texture0, pixel, 0).rgb;
}

// Exposed light compressed by its luma into [0, 1), where sharpening works, and back.
fn compress(color: vec3<f32>) -> vec3<f32> {
    let exposed = max(color, vec3<f32>(0.0)) * post.composite.x;
    return exposed / (1.0 + dot(exposed, LUMA));
}

fn expand(color: vec3<f32>) -> vec3<f32> {
    return color / max(1.0 - dot(color, LUMA), 1e-4);
}

// The exposed scene at the pixel, sharpened: RCAS (FSR 1's robust contrast-adaptive
// sharpening). Its negative lobe over the four neighbors adapts to their contrast: the strongest
// that keeps every channel of the result inside the compressed range (0 to 1), so it never
// clips, scaled by the player's setting. It does ring past the neighbors at hard edges, which
// is why it is off by default (spec §5.12).
fn sharpened(position: vec2<f32>, uv: vec2<f32>) -> vec3<f32> {
    let center = compress(scene_at(position, uv));
    let up = compress(scene_tap(position, uv, vec2<f32>(0.0, -1.0)));
    let left = compress(scene_tap(position, uv, vec2<f32>(-1.0, 0.0)));
    let right = compress(scene_tap(position, uv, vec2<f32>(1.0, 0.0)));
    let down = compress(scene_tap(position, uv, vec2<f32>(0.0, 1.0)));
    let lowest = min(min(up, left), min(right, down));
    let highest = max(max(up, left), max(right, down));
    // How far the lobe may go before the result leaves [0, 1].
    let hit_min = min(lowest, center) / max(4.0 * highest, vec3<f32>(1e-5));
    let hit_max = (1.0 - max(highest, center)) / min(4.0 * lowest - 4.0, vec3<f32>(-1e-5));
    let lobe_rgb = max(-hit_min, hit_max);
    let lobe = max(-RCAS_LIMIT, min(max3(lobe_rgb), 0.0)) * post.frame.w;
    let result = (lobe * (up + left + right + down) + center) / (4.0 * lobe + 1.0);
    return expand(result);
}

// The vignette's tinted form: the corners multiply toward its color, black as the plain one.
fn tinted_vignette(uv: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let reach = post.vignette.x * vignette_reach(uv, size);
    return 1.0 - reach * (1.0 - post.vignette_color.rgb);
}

// A ghost's body: soft at its edge, brighter toward its rim, as an aperture's image is.
fn ghost(distance: f32, softness: f32) -> f32 {
    let body = 1.0 - smoothstep(1.0 - softness, 1.0, distance);
    return body * (0.55 + 0.45 * distance * distance);
}

// One lens flare element's light at `uv`, `offset` from its center in frame heights (so every
// element is round on screen).
fn flare_element(element: FlareElement, uv: vec2<f32>, sun: vec2<f32>, axis: vec2<f32>) -> vec3<f32> {
    let offset = (uv - (sun + axis * element.shape.x)) * vec2<f32>(post.frame.x, 1.0);
    // Most of the frame is outside most elements: a box test first, without a square root.
    if any(abs(offset) > element.reach.xy) {
        return vec3<f32>(0.0);
    }
    let distance = length(offset);
    if distance < element.reach.z {
        return vec3<f32>(0.0);
    }
    let radius = element.shape.y;
    let softness = element.shape.z;
    var amount = 0.0;
    switch u32(element.shape.w) {
        case FLARE_DISC: {
            amount = ghost(distance / radius, softness);
        }
        case FLARE_RING: {
            let across = (distance - radius) / (radius * softness);
            amount = exp(-across * across);
        }
        case FLARE_HEXAGON: {
            let p = abs(offset);
            amount = ghost(max(p.x * 0.8660254 + p.y * 0.5, p.y) / radius, softness);
        }
        case FLARE_STREAK, default: {
            // A horizontal streak, fading along its length.
            let along = saturate(1.0 - abs(offset.x) / radius);
            amount = along * along * exp(-abs(offset.y) / softness);
        }
    }
    return element.color.rgb * amount;
}

// The lens flare's light at `uv`: the default lens's elements along the axis from the sun
// through the middle of the frame, by the sun's `visibility`. Unrolled, as the gradient layers
// are: wgpu bounds every loop with a counter, which keeps Metal from unrolling it, and the loop
// cost about three times the unrolled calls (an M5 at 1080p).
fn lens_flare(uv: vec2<f32>, visibility: f32) -> vec3<f32> {
    let sun = post.flare_sun.xy;
    let axis = (vec2<f32>(0.5) - sun) * 2.0;
    let light = flare_element(post.flare_elements[0], uv, sun, axis)
        + flare_element(post.flare_elements[1], uv, sun, axis)
        + flare_element(post.flare_elements[2], uv, sun, axis)
        + flare_element(post.flare_elements[3], uv, sun, axis)
        + flare_element(post.flare_elements[4], uv, sun, axis)
        + flare_element(post.flare_elements[5], uv, sun, axis)
        + flare_element(post.flare_elements[6], uv, sun, axis)
        + flare_element(post.flare_elements[7], uv, sun, axis);
    return light * post.flare.rgb * visibility;
}

// The diffusion glow at `uv`, sRGB-encoded: the bloom chain's widest smooth level through its
// soft threshold, building toward the glow's color.
fn diffusion_glow(uv: vec2<f32>) -> vec3<f32> {
    let wide = textureSampleLevel(diffusion_level, linear_clamp, uv, 0.0).rgb;
    let light = soft_threshold(wide, post.diffusion_threshold);
    return linear_to_srgb(post.diffusion.rgb * (1.0 - exp(-post.diffusion.w * light)));
}

fn screen(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    return 1.0 - (1.0 - a) * (1.0 - b);
}

// `layer` over `a` in its blend mode, on sRGB-encoded display values. Three cases rather than one
// per mode: each case is inlined into every unrolled layer, and fewer kept the shader smaller and
// faster.
fn blended(a: vec3<f32>, layer: GradientLayer) -> vec3<f32> {
    let b = layer.color.rgb;
    switch layer.mode.y {
        case BLEND_OVERLAY: {
            return select(screen(a, 2.0 * b - 1.0), 2.0 * a * b, a < vec3<f32>(0.5));
        }
        case BLEND_SOFT_LIGHT: {
            // The W3C compositing formula.
            let lifted = select(sqrt(a), ((16.0 * a - 12.0) * a + 4.0) * a, a <= vec3<f32>(0.25));
            let darker = a - (1.0 - 2.0 * b) * a * (1.0 - a);
            let lighter = a + (2.0 * b - 1.0) * (lifted - a);
            return select(lighter, darker, b <= vec3<f32>(0.5));
        }
        case BLEND_AFFINE, default: {
            return min(a * layer.scale.rgb + layer.offset.rgb, vec3<f32>(1.0));
        }
    }
}

// One gradient layer over `color` at `uv`.
fn gradient_layer(color: vec3<f32>, uv: vec2<f32>, layer: GradientLayer) -> vec3<f32> {
    let aspect = post.frame.x;
    var position: f32;
    if layer.mode.x == GRADIENT_RADIAL {
        // In half the frame's diagonal, measured in frame heights.
        let half_diagonal = 0.5 * sqrt(aspect * aspect + 1.0);
        position = length((uv - layer.geometry.xy) * vec2<f32>(aspect, 1.0)) / half_diagonal;
    } else {
        position = dot(uv - 0.5, layer.geometry.xy) + 0.5;
    }
    let ramp = saturate((position - layer.geometry.z) * layer.geometry.w);
    let weight = ramp * ramp * (3.0 - 2.0 * ramp) * layer.color.a;
    return mix(color, blended(color, layer), weight);
}

// The gradient layers over `display`, in order: unrolled (see `lens_flare`), each slot behind a
// branch every pixel takes alike.
fn gradient_layers(display: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    var color = display;
    let count = post.counts.x;
    if count > 0u {
        color = gradient_layer(color, uv, post.gradients[0]);
    }
    if count > 1u {
        color = gradient_layer(color, uv, post.gradients[1]);
    }
    if count > 2u {
        color = gradient_layer(color, uv, post.gradients[2]);
    }
    if count > 3u {
        color = gradient_layer(color, uv, post.gradients[3]);
    }
    return color;
}

// The fused pass with the compositing layers (M1 P2), or sharpening, or both: as `fs_post`,
// with the scene sharpened, the lens flare added to the light and the vignette tinted before the
// grade, then the diffusion glow screened and the gradient layers blended over the display
// values.
@fragment
fn fs_composite(in: CompositeVaryings) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(texture0));
    var color: vec3<f32>;
    if POST_SHARPEN {
        color = sharpened(in.position.xy, in.uv);
    } else {
        color = scene_at(in.position.xy, in.uv) * post.composite.x;
    }
    color += textureSampleLevel(texture1, linear_clamp, in.uv, 0.0).rgb * post.composite.y;
    if POST_LAYERS {
        // Flat across the frame: a branch every pixel takes alike.
        if in.flare > 0.0 {
            color += lens_flare(in.uv, in.flare);
        }
        color *= tinted_vignette(in.uv, size);
    } else {
        color *= vignette(in.uv, size);
    }
    var display = graded(color);
    if POST_LAYERS {
        if post.diffusion.w > 0.0 {
            display = screen(display, diffusion_glow(in.uv));
        }
        display = gradient_layers(display, in.uv);
    }
    return vec4<f32>(srgb_to_linear(display), 1.0);
}
