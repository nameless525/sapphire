// GTAO at half resolution (M1 W5, `sapphire_render::ambient`; Jimenez et al., "Practical
// Real-Time Strategies for Accurate Indirect Occlusion", 2016, as Intel's XeGTAO implements it):
// how much of the hemisphere above each point its surroundings leave open, for the
// environment's ambient light (ambient.wgsl's `gtao_visibility` reads it back).
//
// 0. `cs_gtao_depths`: per half-resolution texel, the view distance of the scene prepass's
//    depth at the top-left pixel of its 2 × 2 block, and `GTAO_DEPTH_MIPS` − 1 coarser mips,
//    each texel a depth-weighted mean of four (XeGTAO's depth prefilter), so samples far from
//    their texel read a coarser mip and stay in the texture cache. Where a character or an
//    outline hull is the nearest surface, the distance is the sky's: characters occlude the
//    world through their capsules alone (capsule AO), so GTAO leaves them out.
// 1. `cs_gtao`: per half-resolution texel, that distance and the prepass's normal at the same
//    pixel, then `slices` screen-space directions, each searched `steps` times to either side
//    for the highest horizon within `radius` meters, integrated against the cosine around the
//    normal projected into the slice. A 4 × 4 pattern turns the slices and jitters the steps;
//    under temporal AA it also moves every frame, and the resolve averages the frames.
// 2. `cs_gtao_denoise`: a 5 × 5 depth-aware blur whose weights (1, 2, 2, 2, 1) per axis give
//    each of the pattern's 16 phases the same total weight, so one frame's pattern averages out
//    exactly on flat ground.
//
// Both write visibility in x and the texel's view depth in y, which the blur and the upsample
// weigh by. Positions are reconstructed in XeGTAO's left-handed view space (x right, y up,
// z forward): the renderer's view space with z negated.

struct GtaoUniforms {
    // xy: the half-resolution textures' size; zw: the scene's.
    size: vec4<f32>,
    // x: the radius in meters; y: slices; z: steps to each side; w: the frame's noise offset in
    // [0, 1), 0 without temporal AA.
    params: vec4<f32>,
}

// The scene prepass's depth and normals (`PrepassTargets`).
@group(2) @binding(2) var gtao_depth: texture_depth_2d;
@group(2) @binding(3) var gtao_normals: texture_2d<f32>;
@group(2) @binding(4) var gtao_raw_out: texture_storage_2d<rgba16float, write>;
@group(2) @binding(5) var gtao_raw: texture_2d<f32>;
@group(2) @binding(6) var gtao_out: texture_storage_2d<rgba16float, write>;
@group(2) @binding(7) var<uniform> gtao: GtaoUniforms;
// The half-resolution view distance's mips, written by the prefilter and read by the search.
@group(2) @binding(8) var gtao_depth_mip0: texture_storage_2d<r32float, write>;
@group(2) @binding(9) var gtao_depth_mip1: texture_storage_2d<r32float, write>;
@group(2) @binding(10) var gtao_depth_mip2: texture_storage_2d<r32float, write>;
@group(2) @binding(11) var gtao_depth_mip3: texture_storage_2d<r32float, write>;
@group(2) @binding(12) var gtao_depths: texture_2d<f32>;
// The character prepass's depth (M1 C5) and the scene prepass's outline mask (M1 C4): where
// characters and their hulls are nearest, which the prefilter leaves out.
@group(2) @binding(13) var gtao_character_depth: texture_depth_2d;
@group(2) @binding(14) var gtao_outlines: texture_2d<f32>;

// Threads along each side of a workgroup (`ambient::GTAO_WORKGROUP`): the prefilter reduces a
// workgroup's 8 × 8 texels to one texel of its last mip.
const GTAO_WORKGROUP: u32 = 8u;
// Mips of the view distance (`ambient::GTAO_DEPTH_MIPS`).
const GTAO_DEPTH_MIPS: u32 = 4u;
// A sample `n` half-resolution texels from its texel reads mip log2(n) − this (XeGTAO's 3.3).
const GTAO_MIP_OFFSET: f32 = 3.3;
const GTAO_PI: f32 = 3.14159265358979;
const GTAO_HALF_PI: f32 = 1.5707963;
// The view depth stored where nothing was drawn (the sky): f16's largest.
const GTAO_FAR: f32 = 65504.0;
// The search starts this many half-resolution texels out, past the center's own texel.
const GTAO_MIN_TEXELS: f32 = 1.3;
// Steps cluster toward the center: the step's fraction of the radius, squared.
const GTAO_STEP_POWER: f32 = 2.0;
// A horizon's weight falls to 0 over this last share of the radius (from 1 at 60% of it).
const GTAO_FALLOFF_SHARE: f32 = 0.4;
// Visibility to this power (XeGTAO's final power, softened for the anime look): open surfaces
// stay open and creases deepen.
const GTAO_POWER: f32 = 1.5;
// The blur's depth tolerance, as a share of the center's view depth.
const GTAO_DEPTH_TOLERANCE: f32 = 0.03;

// The 4 × 4 ordered-dither matrix, 0 to 15, at a pixel.
fn gtao_dither(pixel: vec2<u32>) -> f32 {
    let x = pixel.x & 3u;
    let y = pixel.y & 3u;
    let bayer = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    return f32(bayer[y * 4u + x]);
}

// The distance in front of the camera of reverse-Z depth `depth` through the view's
// perspective projection (clip.w = −z and clip.z = p[2][2]·z + p[3][2]); `GTAO_FAR` where
// nothing was drawn (the sky).
fn gtao_distance(depth: f32) -> f32 {
    let p = view.clip_from_view;
    return select(GTAO_FAR, p[3][2] / max(depth + p[2][2], 1e-12), depth > 0.0);
}

// The left-handed view-space position of the scene pixel at `pixel` (framebuffer coordinates)
// at `distance` in front of the camera, through the view's (jittered) projection.
fn gtao_view_position(pixel: vec2<f32>, distance: f32) -> vec3<f32> {
    let p = view.clip_from_view;
    let uv = pixel / gtao.size.zw;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    return vec3<f32>(
        distance * (ndc.x + p[2][0]) / p[0][0],
        distance * (ndc.y + p[2][1]) / p[1][1],
        distance,
    );
}

// The view distance a sample at `pixel` (framebuffer coordinates) `texels` half-resolution
// texels from its center reads: the half-resolution texel holding it, in the mip for that
// reach, clamped to the mip.
fn gtao_sample_distance(pixel: vec2<f32>, texels: f32) -> f32 {
    let mip = u32(clamp(log2(max(texels, 1.0)) - GTAO_MIP_OFFSET, 0.0, f32(GTAO_DEPTH_MIPS - 1u)));
    let last = vec2<i32>(textureDimensions(gtao_depths, mip)) - 1;
    let texel = clamp(vec2<i32>(floor(pixel * 0.5)) >> vec2<u32>(mip), vec2<i32>(0), last);
    return textureLoad(gtao_depths, texel, i32(mip)).x;
}

// XeGTAO's depth mip filter: the mean of four distances, each weighed down as it lies nearer
// than the farthest by more than part of the radius, so a coarse texel keeps the far surface
// at a silhouette rather than a distance between.
fn gtao_depth_filter(d: vec4<f32>) -> f32 {
    let farthest = max(max(d.x, d.y), max(d.z, d.w));
    let radius = 0.75 * gtao.params.x;
    let range = GTAO_FALLOFF_SHARE * radius;
    let start = (1.0 - GTAO_FALLOFF_SHARE) * radius;
    let weights = saturate((farthest - d - start) * (-1.0 / range) + 1.0);
    return dot(weights, d) / dot(weights, vec4<f32>(1.0));
}

// XeGTAO's arc cosine approximation (Eberly), within 1e-4 of acos over [−1, 1].
fn gtao_acos(x: f32) -> f32 {
    let a = abs(x);
    let root = sqrt(1.0 - a) * (1.5707288 + a * (-0.2121144 + a * (0.0742610 - 0.0187293 * a)));
    return select(GTAO_PI - root, root, x >= 0.0);
}

// A workgroup's distances, reduced in place mip by mip.
var<workgroup> gtao_tile: array<f32, 64>;

@compute @workgroup_size(GTAO_WORKGROUP, GTAO_WORKGROUP, 1)
fn cs_gtao_depths(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
) {
    // Every thread takes part in the reduction: those past the edge repeat the edge's texel.
    let size = vec2<u32>(gtao.size.xy);
    let texel = min(id.xy, size - 1u);
    let pixel = vec2<i32>(min(vec2<f32>(texel * 2u), gtao.size.zw - 1.0));
    // The character prepass writes the scene prepass's depth bit for bit where a character is
    // nearest, less where something stands in front, and 0 where no character is (as temporal
    // AA's responsive mask reads them); the outline mask is 1 where a hull is nearest.
    let depth = textureLoad(gtao_depth, pixel, 0);
    let character = textureLoad(gtao_character_depth, pixel, 0);
    let hull = textureLoad(gtao_outlines, pixel, 0).r;
    let left_out = (character > 0.0 && character >= depth) || hull > 0.5;
    let distance = select(gtao_distance(depth), GTAO_FAR, left_out);
    textureStore(gtao_depth_mip0, texel, vec4<f32>(distance, 0.0, 0.0, 0.0));
    let slot = local.y * GTAO_WORKGROUP + local.x;
    gtao_tile[slot] = distance;
    // Mip m's texel is the filter of four of mip m − 1's, held by the threads 2^(m−1) apart;
    // the thread at their corner writes it.
    for (var mip = 1u; mip < GTAO_DEPTH_MIPS; mip++) {
        workgroupBarrier();
        let step = 1u << (mip - 1u);
        let corner = all(local.xy % (step * 2u) == vec2<u32>(0u));
        var filtered = 0.0;
        if corner {
            filtered = gtao_depth_filter(vec4<f32>(
                gtao_tile[slot],
                gtao_tile[slot + step],
                gtao_tile[slot + step * GTAO_WORKGROUP],
                gtao_tile[slot + step * (GTAO_WORKGROUP + 1u)],
            ));
        }
        workgroupBarrier();
        if corner {
            gtao_tile[slot] = filtered;
            let at = id.xy >> vec2<u32>(mip);
            let value = vec4<f32>(filtered, 0.0, 0.0, 0.0);
            switch mip {
                case 1u: {
                    if all(at < textureDimensions(gtao_depth_mip1)) {
                        textureStore(gtao_depth_mip1, at, value);
                    }
                }
                case 2u: {
                    if all(at < textureDimensions(gtao_depth_mip2)) {
                        textureStore(gtao_depth_mip2, at, value);
                    }
                }
                default: {
                    if all(at < textureDimensions(gtao_depth_mip3)) {
                        textureStore(gtao_depth_mip3, at, value);
                    }
                }
            }
        }
    }
}

@compute @workgroup_size(GTAO_WORKGROUP, GTAO_WORKGROUP, 1)
fn cs_gtao(@builtin(global_invocation_id) id: vec3<u32>) {
    if f32(id.x) >= gtao.size.x || f32(id.y) >= gtao.size.y {
        return;
    }
    let texel = vec2<i32>(id.xy);
    let pixel = min(vec2<f32>(id.xy * 2u), gtao.size.zw - 1.0) + 0.5;
    let distance = textureLoad(gtao_depths, texel, 0).x;
    if distance >= GTAO_FAR {
        // Nothing drawn: the sky, which nothing occludes.
        textureStore(gtao_raw_out, texel, vec4<f32>(1.0, GTAO_FAR, 0.0, 0.0));
        return;
    }
    // Moved a hair toward the camera, so the surface never occludes itself (XeGTAO's 0.9992).
    let center = gtao_view_position(pixel, distance) * 0.9992;
    let encoded = textureLoad(gtao_normals, vec2<i32>(pixel), 0).xy;
    let world_normal = oct_decode(encoded);
    let rh_normal = (view.view_from_world * vec4<f32>(world_normal, 0.0)).xyz;
    let normal = safe_normalize(vec3<f32>(rh_normal.xy, -rh_normal.z));
    let view_vec = normalize(-center);

    let radius = gtao.params.x;
    let slices = u32(gtao.params.y);
    let steps = u32(gtao.params.z);
    // The radius in pixels at the center's distance.
    let radius_pixels = radius * view.clip_from_view[1][1] * 0.5 * gtao.size.w / center.z;
    let falloff_range = GTAO_FALLOFF_SHARE * radius;
    let falloff_mul = -1.0 / falloff_range;
    let falloff_add = (1.0 - GTAO_FALLOFF_SHARE) * radius / falloff_range + 1.0;
    // In pixels: two to a half-resolution texel.
    let min_s = 2.0 * GTAO_MIN_TEXELS / max(radius_pixels, 1e-3);

    // The pattern: the slices' turn, and the steps' jitter, offset per frame under temporal AA.
    let noise_slice = fract(gtao_dither(id.xy) / 16.0 + gtao.params.w);
    let noise_step = fract(gtao_dither(id.yx + vec2<u32>(2u, 1u)) / 16.0 + gtao.params.w * 1.618034);

    var visibility = 0.0;
    for (var slice = 0u; slice < slices; slice++) {
        let phi = (f32(slice) + noise_slice) / f32(slices) * GTAO_PI;
        let cos_phi = cos(phi);
        let sin_phi = sin(phi);
        // Screen space runs y down; view space y up.
        let omega = vec2<f32>(cos_phi, -sin_phi) * radius_pixels;
        let direction = vec3<f32>(cos_phi, sin_phi, 0.0);
        let ortho_direction = direction - dot(direction, view_vec) * view_vec;
        let axis = normalize(cross(ortho_direction, view_vec));
        let projected_normal = normal - axis * dot(normal, axis);
        let sign_n = select(-1.0, 1.0, dot(ortho_direction, projected_normal) >= 0.0);
        let projected_length = length(projected_normal);
        let cos_n = saturate(dot(projected_normal, view_vec) / max(projected_length, 1e-6));
        let n = sign_n * gtao_acos(cos_n);
        let low0 = cos(n + GTAO_HALF_PI);
        let low1 = cos(n - GTAO_HALF_PI);
        var horizon0 = low0;
        var horizon1 = low1;
        for (var step = 0u; step < steps; step++) {
            let jitter = fract(noise_step + f32(slice + step * slices) * 0.618034);
            var s = (f32(step) + jitter) / f32(steps);
            s = pow(s, GTAO_STEP_POWER) + min_s;
            // Whole half-resolution texels, so a sample lands on the pixel its texel's depth
            // was read at (the top-left of its 2 × 2 block, like the center's): a sample between
            // would take a neighbor's depth from one side, and occlude toward that side more than
            // its mirror.
            let texel_offset = round(s * omega * 0.5);
            let offset = texel_offset * 2.0;
            let texels = length(texel_offset);
            let at0 = pixel + offset;
            let at1 = pixel - offset;
            let delta0 = gtao_view_position(at0, gtao_sample_distance(at0, texels)) - center;
            let delta1 = gtao_view_position(at1, gtao_sample_distance(at1, texels)) - center;
            let distance0 = max(length(delta0), 1e-6);
            let distance1 = max(length(delta1), 1e-6);
            let weight0 = saturate(distance0 * falloff_mul + falloff_add);
            let weight1 = saturate(distance1 * falloff_mul + falloff_add);
            horizon0 = max(horizon0, mix(low0, dot(delta0 / distance0, view_vec), weight0));
            horizon1 = max(horizon1, mix(low1, dot(delta1 / distance1, view_vec), weight1));
        }
        // XeGTAO's fudge: a slice whose projected normal vanishes still counts a little.
        let length_fudged = mix(projected_length, 1.0, 0.05);
        let h0 = n + clamp(-gtao_acos(horizon1) - n, -GTAO_HALF_PI, GTAO_HALF_PI);
        let h1 = n + clamp(gtao_acos(horizon0) - n, -GTAO_HALF_PI, GTAO_HALF_PI);
        let arc0 = (cos_n + 2.0 * h0 * sin(n) - cos(2.0 * h0 - n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sin(n) - cos(2.0 * h1 - n)) * 0.25;
        visibility += length_fudged * (arc0 + arc1);
    }
    visibility = pow(saturate(visibility / f32(max(slices, 1u))), GTAO_POWER);
    // Too far for the radius to reach a pixel: open.
    let reach = saturate(radius_pixels - 1.0);
    visibility = mix(1.0, visibility, reach);
    textureStore(gtao_raw_out, texel, vec4<f32>(visibility, center.z, 0.0, 0.0));
}

@compute @workgroup_size(GTAO_WORKGROUP, GTAO_WORKGROUP, 1)
fn cs_gtao_denoise(@builtin(global_invocation_id) id: vec3<u32>) {
    if f32(id.x) >= gtao.size.x || f32(id.y) >= gtao.size.y {
        return;
    }
    let texel = vec2<i32>(id.xy);
    let center = textureLoad(gtao_raw, texel, 0);
    let tolerance = GTAO_DEPTH_TOLERANCE * center.y;
    let last = vec2<i32>(gtao.size.xy) - 1;
    var sum = 0.0;
    var weights = 0.0;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let sample = textureLoad(gtao_raw, clamp(texel + vec2<i32>(x, y), vec2<i32>(0), last), 0);
            let box = select(2.0, 1.0, abs(x) == 2) * select(2.0, 1.0, abs(y) == 2);
            let difference = (sample.y - center.y) / max(tolerance, 1e-4);
            let weight = box / (1.0 + difference * difference);
            sum += sample.x * weight;
            weights += weight;
        }
    }
    textureStore(gtao_out, texel, vec4<f32>(sum / weights, center.y, 0.0, 0.0));
}
