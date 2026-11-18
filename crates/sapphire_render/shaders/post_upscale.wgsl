// The phone tiers' upscaler (M5 L14; `sapphire_render::post`'s `upscale` module): an edge-adaptive upscale and a
// sharpen, the two passes after the fused pass where the scene is smaller than the output and the tier upscales
// along edges. Appended to post.wgsl, whose code is unchanged: the fused pass writes the scene's size, these two
// write the output's, and a frame at render scale 1, or on a tier without them, never runs them.
//
// Written for this engine from the published description of AMD FidelityFX Super Resolution 1.0's two passes,
// EASU (edge-adaptive spatial upsampling) and RCAS (robust contrast-adaptive sharpening); no AMD code is in it.
// What follows FSR 1 and what is this engine's own is said at each pass. FSR 1 is under the MIT license, whose
// notice the method's users keep (content/third_party/amd_fsr1/LICENSE):
//
//   Copyright (c) 2021 Advanced Micro Devices, Inc. All rights reserved.
//
//   Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
//   associated documentation files (the "Software"), to deal in the Software without restriction, including
//   without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//   copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the
//   following conditions:
//
//   The above copyright notice and this permission notice shall be included in all copies or substantial
//   portions of the Software.
//
//   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT
//   LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO
//   EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN
//   AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE
//   OR OTHER DEALINGS IN THE SOFTWARE.
//
// Bind groups: 0 holds `UpscaleUniforms` and a nearest sampler clamped to the edge, at bindings 2 and 3, past what
// post.wgsl's group 0 binds, so the two never meet; 1 is post's one-texture group, `texture0`: the fused pass's
// output (display light, read through the target format's sRGB view as linear) for the upscale, the upscale's
// output (perceptual) for the sharpen. Texels are read through the sampler at their centers, which gives each
// texel's value exactly, as a load would, without a load's bounds checks (which cost the first TAA resolve half
// its time, spec §5.12) and with the frame's edge held for free.
//
// Both passes work on perceptual values, the square root of linear light (gamma 2), as FSR 1 asks: its edge
// weights and its sharpen's limits assume a roughly even response. The upscale writes them at 10 bits a channel,
// and the sharpen squares them back into the target.

// Mirrors `post::upscale::UpscaleUniforms`.
struct UpscaleUniforms {
    // xy: the input's size over the output's; zw: -0.5, so a pixel's position (its center) times xy plus zw is
    // its place among the input's texel centers.
    easu: vec4<f32>,
    // x: the sharpen's strength, 0 to 1 of RCAS's strongest lobe: the tier's, faded to nothing as the input's
    // size nears the output's.
    sharpen: vec4<f32>,
    // xy: one input texel in texture coordinates, the upscale's step between taps; zw: one output pixel, the
    // sharpen's.
    texel: vec4<f32>,
}

@group(0) @binding(2) var<uniform> upscale: UpscaleUniforms;
@group(0) @binding(3) var upscale_nearest: sampler;

// Below this squared length, the edge's direction is noise: a flat area reads as no edge, along x.
const EASU_FLAT: f32 = 1.0 / 32768.0;
// RCAS's lobe stops at 1/4 - 1/16, short of the 1/4 where its kernel would divide by nothing.
const RCAS_STRONGEST: f32 = 0.1875;

// The input texel `offset` texels from the one whose center is at `center`, held inside the input, as perceptual
// light.
fn easu_texel(center: vec2<f32>, offset: vec2<f32>) -> vec3<f32> {
    let linear = textureSampleLevel(texture0, upscale_nearest, center + offset * upscale.texel.xy, 0.0).rgb;
    return sqrt(max(linear, vec3<f32>(0.0)));
}

// The pixel `offset` pixels from the one whose center is at `center`, held inside the frame.
fn rcas_pixel(center: vec2<f32>, offset: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(texture0, upscale_nearest, center + offset * upscale.texel.zw, 0.0).rgb;
}

// FSR 1's cheap luma, twice the true one's scale: green, half red, half blue.
fn easu_luma(color: vec3<f32>) -> f32 {
    return color.b * 0.5 + (color.r * 0.5 + color.g);
}

// One of the four texels round the position votes for the edge through it (FSR 1's direction and length
// analysis): `w` is its bilinear weight, `up`, `left`, `center`, `right` and `down` the lumas of the cross
// round it. xy: its weighted gradient, the edge's normal; z: its weighted length, how much like a single edge
// the cross looks on each axis (1 where the step is all on one side of the center, 0 where it is noise or flat).
fn easu_vote(w: f32, up: f32, left: f32, center: f32, right: f32, down: f32) -> vec3<f32> {
    let across_x = right - left;
    let across_y = down - up;
    // The steeper half of each axis's step over the whole step; a flat axis has neither, and reads 0.
    let steeper_x = max(abs(right - center), abs(center - left));
    let steeper_y = max(abs(down - center), abs(center - up));
    let length_x = saturate(abs(across_x) / max(steeper_x, 1e-6));
    let length_y = saturate(abs(across_y) / max(steeper_y, 1e-6));
    return vec3<f32>(across_x, across_y, length_x * length_x + length_y * length_y) * w;
}

// One tap of FSR 1's kernel: a windowed lobe, stretched along the edge and squeezed across it. `offset` is
// the texel's center from the position, in input texels; `direction` the edge's normal, unit length; `scale`
// how far the kernel reaches along and across; `lobe` the negative lobe's depth; `reach` the squared distance
// past which the window holds at its last value.
fn easu_weight(offset: vec2<f32>, direction: vec2<f32>, scale: vec2<f32>, lobe: f32, reach: f32) -> f32 {
    let rotated = vec2<f32>(
        offset.x * direction.x + offset.y * direction.y,
        offset.y * direction.x - offset.x * direction.y,
    ) * scale;
    let d2 = min(dot(rotated, rotated), reach);
    // The window (2/5 x² - 1)², scaled to meet 0 where the base does, times the base (lobe x² - 1)²: an
    // approximation of lanczos 2 that needs no trigonometry.
    let window = (0.4 * d2 - 1.0) * (0.4 * d2 - 1.0);
    let base = (lobe * d2 - 1.0) * (lobe * d2 - 1.0);
    return (1.5625 * window - 0.5625) * base;
}

// The upscale (FSR 1's EASU): each output pixel reads the twelve input texels round where it falls, the four
// nearest and two beyond each of their sides. The four nearest each vote for the edge through the place; the
// kernel then turns to the voted direction, stretches along it as far as the votes agree that it is one edge, and
// sharpens across it, and its result is held within the four nearest texels' range, so it never rings. Written
// from the method: the taps, votes, kernel and clamp are FSR 1's; the input read texel by texel, held inside the
// frame, the guards against dividing by nothing and the flat path are ours.
//
// The flat path (M6 F1): where the four nearest texels are equal, their range is that one value, so the clamp
// would return it whatever the kernel made; it is returned at once, the votes and the kernel skipped, and the
// result is the same to the bit. The branch is per pixel but a GPU runs a fragment shader in groups (32 pixels on
// Apple's GPUs and most phones'), and a group pays for the whole path if any one of its pixels does: the time is
// saved only by a group that is wholly flat, which B6's `flat upscale groups` counter counts (8 × 4 tiles
// standing in for the groups). The twelve taps are read in this order before the branch, as they were before it:
// reading the four nearest first changed how Metal's compiler rounds the kernel's sum, and moved a pixel by a
// code value in two `upscale-*` scenes. The compiler may still move the other taps past the branch.
@fragment
fn fs_upscale_edge(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let place = in.position.xy * upscale.easu.xy + upscale.easu.zw;
    let floor_place = floor(place);
    let f = place - floor_place;
    // Texel f's center, in texture coordinates.
    let origin = (floor_place + 0.5) * upscale.texel.xy;
    //     b c
    //   e f g h
    //   i j k l
    //     n o
    let b = easu_texel(origin, vec2<f32>(0.0, -1.0));
    let c = easu_texel(origin, vec2<f32>(1.0, -1.0));
    let e = easu_texel(origin, vec2<f32>(-1.0, 0.0));
    let ff = easu_texel(origin, vec2<f32>(0.0, 0.0));
    let g = easu_texel(origin, vec2<f32>(1.0, 0.0));
    let h = easu_texel(origin, vec2<f32>(2.0, 0.0));
    let i = easu_texel(origin, vec2<f32>(-1.0, 1.0));
    let j = easu_texel(origin, vec2<f32>(0.0, 1.0));
    let k = easu_texel(origin, vec2<f32>(1.0, 1.0));
    let l = easu_texel(origin, vec2<f32>(2.0, 1.0));
    let n = easu_texel(origin, vec2<f32>(0.0, 2.0));
    let o = easu_texel(origin, vec2<f32>(1.0, 2.0));
    if all(ff == g) && all(ff == j) && all(ff == k) {
        return vec4<f32>(ff, 1.0);
    }
    let lb = easu_luma(b);
    let lc = easu_luma(c);
    let le = easu_luma(e);
    let lf = easu_luma(ff);
    let lg = easu_luma(g);
    let lh = easu_luma(h);
    let li = easu_luma(i);
    let lj = easu_luma(j);
    let lk = easu_luma(k);
    let ll = easu_luma(l);
    let ln = easu_luma(n);
    let lo = easu_luma(o);
    // The four nearest vote, each by its bilinear weight.
    let votes = easu_vote((1.0 - f.x) * (1.0 - f.y), lb, le, lf, lg, lj)
        + easu_vote(f.x * (1.0 - f.y), lc, lf, lg, lh, lk)
        + easu_vote((1.0 - f.x) * f.y, lf, li, lj, lk, ln)
        + easu_vote(f.x * f.y, lg, lj, lk, ll, lo);
    let gradient2 = dot(votes.xy, votes.xy);
    let flat = gradient2 < EASU_FLAT;
    let direction = select(votes.xy * inverseSqrt(max(gradient2, EASU_FLAT)), vec2<f32>(1.0, 0.0), flat);
    // Agreement, 0 to 1: each vote's two axes sum to at most 2.
    let agreement = (votes.z * 0.5) * (votes.z * 0.5);
    // A diagonal edge reaches √2 further along itself before it leaves the square of taps.
    let stretch = 1.0 / max(abs(direction.x), abs(direction.y));
    let scale = vec2<f32>(1.0 + (stretch - 1.0) * agreement, 1.0 - 0.5 * agreement);
    // From a soft kernel on flat ground to a sharper one across an edge.
    let lobe = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * agreement;
    let reach = 1.0 / lobe;
    let wb = easu_weight(vec2<f32>(0.0, -1.0) - f, direction, scale, lobe, reach);
    let wc = easu_weight(vec2<f32>(1.0, -1.0) - f, direction, scale, lobe, reach);
    let we = easu_weight(vec2<f32>(-1.0, 0.0) - f, direction, scale, lobe, reach);
    let wff = easu_weight(vec2<f32>(0.0, 0.0) - f, direction, scale, lobe, reach);
    let wg = easu_weight(vec2<f32>(1.0, 0.0) - f, direction, scale, lobe, reach);
    let wh = easu_weight(vec2<f32>(2.0, 0.0) - f, direction, scale, lobe, reach);
    let wi = easu_weight(vec2<f32>(-1.0, 1.0) - f, direction, scale, lobe, reach);
    let wj = easu_weight(vec2<f32>(0.0, 1.0) - f, direction, scale, lobe, reach);
    let wk = easu_weight(vec2<f32>(1.0, 1.0) - f, direction, scale, lobe, reach);
    let wl = easu_weight(vec2<f32>(2.0, 1.0) - f, direction, scale, lobe, reach);
    let wn = easu_weight(vec2<f32>(0.0, 2.0) - f, direction, scale, lobe, reach);
    let wo = easu_weight(vec2<f32>(1.0, 2.0) - f, direction, scale, lobe, reach);
    let sum = b * wb + c * wc + e * we + ff * wff + g * wg + h * wh + i * wi + j * wj + k * wk + l * wl + n * wn
        + o * wo;
    let weights = wb + wc + we + wff + wg + wh + wi + wj + wk + wl + wn + wo;
    let lowest = min(min(ff, g), min(j, k));
    let highest = max(max(ff, g), max(j, k));
    // The kernel's weights sum well above zero wherever it is used; the guard keeps a pathological sum from
    // dividing by nothing, which the clamp then holds in range.
    let color = clamp(sum / max(weights, 1e-4), lowest, highest);
    return vec4<f32>(color, 1.0);
}

// The sharpen (FSR 1's RCAS), over the upscale's output: a negative lobe over the four neighbours, as strong as
// keeps every channel of the result inside 0 to 1 given their contrast, at most `RCAS_STRONGEST`, scaled by
// `upscale.sharpen.x`. Then, ours and not FSR 1's, the result is held within the five pixels' own range, so it
// steepens an edge without overshooting it: nothing beside a dark line goes brighter than what is round it, and
// a hard edge between two tones gains no ring. FSR 1's noise detection is left out. Squared back to linear light
// for the target's sRGB encoding.
@fragment
fn fs_upscale_sharpen(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    // The pixel's center, in texture coordinates: its position holds the half.
    let middle = in.position.xy * upscale.texel.zw;
    let center = rcas_pixel(middle, vec2<f32>(0.0, 0.0));
    let up = rcas_pixel(middle, vec2<f32>(0.0, -1.0));
    let left = rcas_pixel(middle, vec2<f32>(-1.0, 0.0));
    let right = rcas_pixel(middle, vec2<f32>(1.0, 0.0));
    let down = rcas_pixel(middle, vec2<f32>(0.0, 1.0));
    let lowest4 = min(min(up, left), min(right, down));
    let highest4 = max(max(up, left), max(right, down));
    // How far the lobe may go before the result leaves [0, 1], channel by channel.
    let hit_min = min(lowest4, center) / max(4.0 * highest4, vec3<f32>(1e-5));
    let hit_max = (1.0 - max(highest4, center)) / min(4.0 * lowest4 - 4.0, vec3<f32>(-1e-5));
    let lobe_rgb = max(-hit_min, hit_max);
    let lobe = max(-RCAS_STRONGEST, min(max(lobe_rgb.r, max(lobe_rgb.g, lobe_rgb.b)), 0.0)) * upscale.sharpen.x;
    let sharpened = (lobe * (up + left + right + down) + center) / (4.0 * lobe + 1.0);
    let held = clamp(sharpened, min(lowest4, center), max(highest4, center));
    return vec4<f32>(held * held, 1.0);
}
