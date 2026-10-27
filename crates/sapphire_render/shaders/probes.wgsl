// The ambient pass's grid work (M1 W5, `sapphire_render::ambient`), compute, in the order a frame
// runs it after the sky cube's face, each only when due:
//
// 1. The sky's projection: the sky cube's radiance projected to L2 SH, a face at a time (the
//    faces the sky cube drew this frame, each face's sums kept), scaled per channel so a surface
//    facing straight up under the open sky receives time of day's authored ambient sky
//    (`frame.ambient_sky`), and the transfer that turns a probe's sky visibility into the
//    irradiance it receives (exact on the icosahedron's twelve vertices,
//    `sapphire_math::sh::SkyTransfer`, whose L2 × L2 → L1 products they integrate exactly).
//    Three dispatches: `cs_sky_projection` sums each face's texels per thread,
//    `cs_sky_projection_faces` adds up each face's threads, and `cs_sky_light` totals the six
//    faces into the sky light. They share no workgroup memory: WARP, the Windows CI's CPU
//    rasterizer, took 26–48 s to create the single-workgroup projection, whose threads added up
//    their sums in workgroup memory behind barriers, and 3–7 minutes more to run it the first
//    time (`sapphire_render::ambient`, "Cost"). The threads' sums go through a buffer of the
//    pass's own instead, and each dispatch adds in the order the workgroup did, so the sky light
//    is the same to the bit.
// 2. `cs_prefilter`, once per mip of the reflection cube: the cube faces the sky drew this frame,
//    filtered into roughness mips (GGX, importance sampled). Mip 0 is the sky cube at the same
//    scale as the ambient; each rougher mip filters the one before it by the difference of their
//    lobes, so a few samples cover a wide lobe without aliasing.
// 3. `cs_relight`: the texels of the probe atlas the frame relights (every one, or a slice of its
//    bricks), one per thread: sky visibility × the sky, plus the bounce of the time-of-day keys
//    the frame blends, and the probe data the lookup's leak test reads.
// 4. `cs_character_ambient`: each character's ambient hemisphere from the grid at its body's
//    center and its head (`CharacterAmbient` in frame.wgsl).
//
// Group 3 holds each pass's own resources; the grid's lookup is ambient.wgsl's, through group 0.

// The sky as the relight multiplies it by visibility (see the top of this file).
struct SkyLight {
    // Per channel c and visibility coefficient k, at c * 9 + k: column k of the 4 × 9 transfer
    // to L1 irradiance (renderer normalization), rows in xyzw.
    transfer: array<vec4<f32>, 27>,
    // Per L2 coefficient: the scaled sky radiance, rgb.
    radiance: array<vec4<f32>, 9>,
    // rgb: the scale from the sky cube's radiance to the authored ambient's; w: 0.
    scale: vec4<f32>,
    // Per cube face f and L2 coefficient k, at f * 9 + k: the face's share of the sky's
    // projection, unscaled, rgb. The projection updates the faces the sky cube drew and sums all
    // six.
    faces: array<vec4<f32>, 54>,
}

struct RelightUniforms {
    // The keys of the grid's bake the frame blends, and their weights (0: unused).
    keys: vec4<u32>,
    weights: vec4<f32>,
    // x: atlas texels to write (kept bricks × 125); y: visibility coefficients (4 or 9); z:
    // words of visibility per probe; w: keys the grid was baked at.
    counts: vec4<u32>,
    // Where the sections of `probe_words` start, in words: x the probe data, y the visibility,
    // z the bounce (the brick probes start at 0).
    offsets: vec4<u32>,
    // xyz: the atlas's bricks along each axis; w: texels of one sub-volume along z.
    atlas: vec4<u32>,
    // x: the first texel this frame relights; y: the texel past its last.
    slice: vec4<u32>,
}

struct PrefilterUniforms {
    // x: the mip's texels along an edge; y: GGX's α for the lobe this mip adds to the one it
    // filters (0: a plain resample); z: samples per texel; w: 1 to scale by `sky_light.scale`
    // (mip 0, from the sky cube), else 0.
    mip: vec4<f32>,
    // x: the first face this frame filters.
    faces: vec4<u32>,
}

// The frame's characters, read-write (frame.wgsl's `characters` is the same buffer, read-only).
@group(1) @binding(2) var<storage, read_write> ambient_entries: array<CharacterData>;

@group(3) @binding(0) var<uniform> relight: RelightUniforms;
// The grid's data, all u32 words: per atlas texel its probe (`ProbeGrid::brick_probes`), then per
// probe its data bits, then its visibility (half floats, two to a word, padded to whole words),
// then per probe and key its bounce (half floats: red's L1, green's, blue's).
@group(3) @binding(1) var<storage, read> probe_words: array<u32>;
@group(3) @binding(2) var<storage, read> sky_light: SkyLight;
@group(3) @binding(3) var relit_atlas: texture_storage_3d<rgba16float, write>;
// What the projection and the prefilter read: the sky cube, or the reflection cube's previous
// mip.
@group(3) @binding(4) var source_cube: texture_cube<f32>;
@group(3) @binding(5) var<storage, read_write> sky_light_out: SkyLight;
@group(3) @binding(6) var<uniform> prefilter: PrefilterUniforms;
@group(3) @binding(7) var prefiltered: texture_storage_2d_array<rgba16float, write>;
// Per projected face (the frame's first is 0), projection thread and L2 coefficient, at
// (face * AMBIENT_WORKGROUP + thread) * 9 + k: the thread's share of the face's sum, rgb
// (`ambient::PROJECTION_PARTIALS_BYTES`).
@group(3) @binding(8) var<storage, read_write> projection_partials: array<vec4<f32>>;

// Threads per workgroup of the projection, the relight and the character pass
// (`ambient::WORKGROUP`).
const AMBIENT_WORKGROUP: u32 = 64u;
// The prefilter's workgroups, square (`ambient::PREFILTER_WORKGROUP`).
const PREFILTER_WORKGROUP: u32 = 8u;

const PI: f32 = 3.14159265358979;
// The icosahedron's vertices, unit length (sapphire_math::sh::ICOSAHEDRON).
const ICOSAHEDRON_A: f32 = 0.5257311;
const ICOSAHEDRON_B: f32 = 0.8506508;
const ICOSAHEDRON_WEIGHT: f32 = 4.0 * PI / 12.0;

fn icosahedron(index: u32) -> vec3<f32> {
    let a = ICOSAHEDRON_A;
    let b = ICOSAHEDRON_B;
    var vertex: vec3<f32>;
    switch index {
        case 0u: { vertex = vec3<f32>(0.0, a, b); }
        case 1u: { vertex = vec3<f32>(0.0, a, -b); }
        case 2u: { vertex = vec3<f32>(0.0, -a, b); }
        case 3u: { vertex = vec3<f32>(0.0, -a, -b); }
        case 4u: { vertex = vec3<f32>(a, b, 0.0); }
        case 5u: { vertex = vec3<f32>(a, -b, 0.0); }
        case 6u: { vertex = vec3<f32>(-a, b, 0.0); }
        case 7u: { vertex = vec3<f32>(-a, -b, 0.0); }
        case 8u: { vertex = vec3<f32>(b, 0.0, a); }
        case 9u: { vertex = vec3<f32>(-b, 0.0, a); }
        case 10u: { vertex = vec3<f32>(b, 0.0, -a); }
        default: { vertex = vec3<f32>(-b, 0.0, -a); }
    }
    return vertex;
}

// The 9 real L2 basis functions at unit `d` (sapphire_math::sh::basis).
fn sh_basis(d: vec3<f32>) -> array<f32, 9> {
    return array<f32, 9>(
        0.2820948,
        0.48860252 * d.y,
        0.48860252 * d.z,
        0.48860252 * d.x,
        1.0925484 * d.x * d.y,
        1.0925484 * d.y * d.z,
        0.31539157 * (3.0 * d.z * d.z - 1.0),
        1.0925484 * d.x * d.z,
        0.5462742 * (d.x * d.x - d.y * d.y),
    );
}

// The unit direction of cube face `face`'s point `(s, t)` (each −1 to 1, t down the face), as a
// cube-map lookup reads it (sky.rs's `cube_face_axes`).
fn cube_texel_direction(face: u32, s: f32, t: f32) -> vec3<f32> {
    var right: vec3<f32>;
    var up = vec3<f32>(0.0, 1.0, 0.0);
    var forward: vec3<f32>;
    switch face {
        case 0u: {
            right = vec3<f32>(0.0, 0.0, -1.0);
            forward = vec3<f32>(1.0, 0.0, 0.0);
        }
        case 1u: {
            right = vec3<f32>(0.0, 0.0, 1.0);
            forward = vec3<f32>(-1.0, 0.0, 0.0);
        }
        case 2u: {
            right = vec3<f32>(1.0, 0.0, 0.0);
            up = vec3<f32>(0.0, 0.0, -1.0);
            forward = vec3<f32>(0.0, 1.0, 0.0);
        }
        case 3u: {
            right = vec3<f32>(1.0, 0.0, 0.0);
            up = vec3<f32>(0.0, 0.0, 1.0);
            forward = vec3<f32>(0.0, -1.0, 0.0);
        }
        case 4u: {
            right = vec3<f32>(1.0, 0.0, 0.0);
            forward = vec3<f32>(0.0, 0.0, 1.0);
        }
        default: {
            right = vec3<f32>(-1.0, 0.0, 0.0);
            forward = vec3<f32>(0.0, 0.0, -1.0);
        }
    }
    return normalize(forward + s * right - t * up);
}

// ---------------------------------------------------------------------------------------------
// 1. The sky's projection.

// Samples along each edge of each face: 6144 in all, far finer than L2 needs.
const PROJECTION_SIDE: u32 = 32u;

// One workgroup per face `ambient.counts.zw` names (the first and how many: those the sky cube
// drew this frame, all six when a grid arrives, none when only the authored ambient jumped): each
// thread sums every AMBIENT_WORKGROUP-th texel of the face, from its index on.
@compute @workgroup_size(AMBIENT_WORKGROUP)
fn cs_sky_projection(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_index) thread: u32,
) {
    let face = ambient.counts.z + group.x;
    let per_face = PROJECTION_SIDE * PROJECTION_SIDE;
    let step = 2.0 / f32(PROJECTION_SIDE);
    var sum: array<vec3<f32>, 9>;
    for (var texel = thread; texel < per_face; texel += AMBIENT_WORKGROUP) {
        let s = (f32(texel % PROJECTION_SIDE) + 0.5) * step - 1.0;
        let t = (f32(texel / PROJECTION_SIDE) + 0.5) * step - 1.0;
        // A cube texel's solid angle: its area over its distance cubed.
        let length_squared = 1.0 + s * s + t * t;
        let solid_angle = step * step / (length_squared * sqrt(length_squared));
        let d = cube_texel_direction(face, s, t);
        let radiance = textureSampleLevel(source_cube, ambient_sampler, d, 0.0).rgb;
        let y = sh_basis(d);
        for (var k = 0u; k < 9u; k++) {
            sum[k] += radiance * (y[k] * solid_angle);
        }
    }
    let first = (group.x * AMBIENT_WORKGROUP + thread) * 9u;
    for (var k = 0u; k < 9u; k++) {
        projection_partials[first + k] = vec4<f32>(sum[k], 0.0);
    }
}

// One thread per projected face and coefficient: the face's threads' sums added pairwise, half
// the threads onto the other half until one remains (a tree, the order a workgroup reduction
// adds in), kept in the sky light's faces.
@compute @workgroup_size(AMBIENT_WORKGROUP)
fn cs_sky_projection_faces(@builtin(global_invocation_id) id: vec3<u32>) {
    let drawn = id.x / 9u;
    let k = id.x % 9u;
    if drawn < ambient.counts.w {
        let first = drawn * AMBIENT_WORKGROUP * 9u + k;
        // The first stride: thread t takes thread t + 32's sum.
        let split = AMBIENT_WORKGROUP / 2u;
        var sums: array<vec4<f32>, 32>;
        for (var thread = 0u; thread < split; thread++) {
            sums[thread] = projection_partials[first + thread * 9u]
                + projection_partials[first + (thread + split) * 9u];
        }
        for (var stride = split / 2u; stride > 0u; stride >>= 1u) {
            for (var thread = 0u; thread < stride; thread++) {
                sums[thread] += sums[thread + stride];
            }
        }
        sky_light_out.faces[(ambient.counts.z + drawn) * 9u + k] = sums[0];
    }
}

// The whole sky, the six faces' sums, lights the grid: threads 0–26 each write a column of the
// transfer, and thread 0 the scaled radiance and the scale.
@compute @workgroup_size(AMBIENT_WORKGROUP)
fn cs_sky_light(@builtin(local_invocation_index) thread: u32) {
    if thread < 27u {
        var total: array<vec4<f32>, 9>;
        for (var k = 0u; k < 9u; k++) {
            var sum = vec4<f32>(0.0);
            for (var face = 0u; face < 6u; face++) {
                sum += sky_light_out.faces[face * 9u + k];
            }
            total[k] = sum;
        }
        // The open sky's irradiance facing up as the grid holds it, L1 (the transfer below
        // projects to L1, dropping the sky's L2 band, which convolving with the clamped cosine
        // scales by 1/4), and the scale to the authored ambient: an open probe then gives exactly
        // that facing up.
        let up = sh_basis(vec3<f32>(0.0, 1.0, 0.0));
        var open = total[0].rgb * up[0];
        for (var k = 1u; k < 4u; k++) {
            open += total[k].rgb * (up[k] * (2.0 / 3.0));
        }
        let authored = max(frame.ambient_sky.rgb, vec3<f32>(0.0));
        let scale = select(vec3<f32>(0.0), authored / max(open, vec3<f32>(1e-12)), open > vec3<f32>(1e-6));
        // Thread `thread` writes one transfer column: the scaled sky radiance at each vertex,
        // times basis function k there, projected to L1 irradiance.
        let channel = thread / 9u;
        let k = thread % 9u;
        var column = vec4<f32>(0.0);
        for (var v = 0u; v < 12u; v++) {
            let y = sh_basis(icosahedron(v));
            var radiance = 0.0;
            for (var j = 0u; j < 9u; j++) {
                radiance += total[j][channel] * y[j];
            }
            let weight = radiance * scale[channel] * ICOSAHEDRON_WEIGHT * y[k];
            column += weight * vec4<f32>(y[0], y[1] * (2.0 / 3.0), y[2] * (2.0 / 3.0), y[3] * (2.0 / 3.0));
        }
        sky_light_out.transfer[thread] = column;
        if thread == 0u {
            for (var j = 0u; j < 9u; j++) {
                sky_light_out.radiance[j] = vec4<f32>(total[j].rgb * scale, 0.0);
            }
            sky_light_out.scale = vec4<f32>(scale, 0.0);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2. The reflection cube's prefilter.

// Point `i` of `n` of the Hammersley set.
fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>((f32(i) + 0.5) / f32(n), f32(reverseBits(i)) * 2.3283064e-10);
}

// A GGX half vector of roughness `alpha` around +Z for the point `xi` (Karis 2013).
fn ggx_half_vector(xi: vec2<f32>, alpha: f32) -> vec3<f32> {
    let phi = 2.0 * PI * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (alpha * alpha - 1.0) * xi.y));
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    return vec3<f32>(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
}

@compute @workgroup_size(PREFILTER_WORKGROUP, PREFILTER_WORKGROUP, 1)
fn cs_prefilter(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = u32(prefilter.mip.x);
    if id.x >= size || id.y >= size {
        return;
    }
    let face = prefilter.faces.x + id.z;
    let s = (f32(id.x) + 0.5) / f32(size) * 2.0 - 1.0;
    let t = (f32(id.y) + 0.5) / f32(size) * 2.0 - 1.0;
    let n = cube_texel_direction(face, s, t);
    let alpha = prefilter.mip.y;
    var color = textureSampleLevel(source_cube, ambient_sampler, n, 0.0).rgb;
    if alpha > 0.0 {
        // The split sum's view-equals-normal lobe, weighed by N·L (Karis 2013).
        let helper = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
        let x = normalize(cross(helper, n));
        let y = cross(n, x);
        let samples = u32(prefilter.mip.z);
        var sum = vec3<f32>(0.0);
        var weights = 0.0;
        for (var i = 0u; i < samples; i++) {
            let h_local = ggx_half_vector(hammersley(i, samples), alpha);
            let h = x * h_local.x + y * h_local.y + n * h_local.z;
            let l = 2.0 * dot(n, h) * h - n;
            let n_dot_l = dot(n, l);
            if n_dot_l > 0.0 {
                sum += textureSampleLevel(source_cube, ambient_sampler, l, 0.0).rgb * n_dot_l;
                weights += n_dot_l;
            }
        }
        color = sum / max(weights, 1e-6);
    }
    let scale = select(vec3<f32>(1.0), sky_light.scale.rgb, prefilter.mip.w > 0.0);
    textureStore(prefiltered, vec2<i32>(id.xy), i32(face), vec4<f32>(color * scale, 1.0));
}

// ---------------------------------------------------------------------------------------------
// 3. The relight.

// The half float in the low 16 bits of `bits`, decoded by hand: `unpack2x16float` needs a
// capability the renderer's shaders don't assume. Baked data holds finite values alone.
fn half_to_f32(bits: u32) -> f32 {
    let exponent = (bits >> 10u) & 31u;
    let mantissa = f32(bits & 1023u);
    let normal = (1.0 + mantissa / 1024.0) * exp2(f32(exponent) - 15.0);
    let magnitude = select(normal, mantissa * exp2(-24.0), exponent == 0u);
    return select(magnitude, -magnitude, (bits & 0x8000u) != 0u);
}

// Half float `index` of the words from `base` on, the low half of a word first.
fn probe_half(base: u32, index: u32) -> f32 {
    return half_to_f32(probe_words[base + index / 2u] >> ((index & 1u) * 16u));
}

@compute @workgroup_size(AMBIENT_WORKGROUP)
fn cs_relight(@builtin(global_invocation_id) id: vec3<u32>) {
    let texel = relight.slice.x + id.x;
    if texel >= relight.slice.y {
        return;
    }
    let brick = texel / 125u;
    let local = texel % 125u;
    let probe = probe_words[texel];
    let atlas = relight.atlas;
    let atlas_brick = vec3<u32>(brick % atlas.x, (brick / atlas.x) % atlas.y, brick / (atlas.x * atlas.y));
    let coords = atlas_brick * BRICK_SIDE + vec3<u32>(local % 5u, (local / 5u) % 5u, local / 25u);

    // The visibility, L2 (L1's padded with zeros).
    var visibility: array<f32, 9>;
    let visibility_base = relight.offsets.y + probe * relight.counts.z;
    for (var k = 0u; k < min(relight.counts.y, 9u); k++) {
        visibility[k] = probe_half(visibility_base, k);
    }
    // The sky through it, per channel.
    var channels: array<vec4<f32>, 3>;
    for (var c = 0u; c < 3u; c++) {
        var irradiance = vec4<f32>(0.0);
        for (var k = 0u; k < 9u; k++) {
            irradiance += sky_light.transfer[c * 9u + k] * visibility[k];
        }
        channels[c] = irradiance;
    }
    // The bounce of the frame's keys.
    let keys = relight.counts.w;
    for (var j = 0u; j < 4u; j++) {
        let weight = relight.weights[j];
        let key = relight.keys[j];
        if weight > 0.0 && key < keys {
            let base = relight.offsets.z + (probe * keys + key) * 6u;
            for (var c = 0u; c < 3u; c++) {
                let first = 4u * c;
                let bounce = vec4<f32>(
                    probe_half(base, first),
                    probe_half(base, first + 1u),
                    probe_half(base, first + 2u),
                    probe_half(base, first + 3u),
                );
                channels[c] += bounce * weight;
            }
        }
    }
    // The free distances as the lookup's leak test reads them: per axis, toward + × 32 + toward −.
    let bits = probe_words[relight.offsets.x + probe];
    let free = vec3<f32>(
        f32((bits & 31u) * 32u + ((bits >> 5u) & 31u)),
        f32(((bits >> 10u) & 31u) * 32u + ((bits >> 15u) & 31u)),
        f32(((bits >> 20u) & 31u) * 32u + ((bits >> 25u) & 31u)),
    );
    // Flags: open space (1), an open cell above it (2) and inside the grid's bricks (4;
    // `ambient.wgsl`'s lookup).
    let flags = select(0u, 1u, (bits & PROBE_VALID_BIT) != 0u) | select(0u, 2u, cell_is_open(brick, local))
        | select(4u, 0u, (bits & PROBE_EDGE_BIT) != 0u);
    let at = vec3<i32>(coords);
    let sub_volume = vec3<i32>(0, 0, i32(atlas.w));
    textureStore(relit_atlas, at + sub_volume * i32(ATLAS_RED), channels[0]);
    textureStore(relit_atlas, at + sub_volume * i32(ATLAS_GREEN), channels[1]);
    textureStore(relit_atlas, at + sub_volume * i32(ATLAS_BLUE), channels[2]);
    let sky_visibility = vec4<f32>(visibility[0], visibility[1], visibility[2], visibility[3]);
    textureStore(relit_atlas, at + sub_volume * i32(ATLAS_VISIBILITY), sky_visibility);
    textureStore(relit_atlas, at + sub_volume * i32(ATLAS_DATA), vec4<f32>(free, f32(flags)));
}

// The probe data's open-space bit (`sapphire_assets::probes::ProbeData`), and the bit the
// renderer sets at upload on a probe some brick around it lacks (empty, or past the grid's edge;
// `ambient::PROBE_EDGE_BIT`).
const PROBE_VALID_BIT: u32 = 0x40000000u;
const PROBE_EDGE_BIT: u32 = 0x80000000u;

// Whether the cell whose lowest corner is the brick's probe `local` is open: all eight corners
// in open space and inside the grid's bricks, each seeing a full spacing toward the others along
// every axis, so the lookup's leak test passes every corner and it can skip reading them. The
// brick's top faces start no cell.
fn cell_is_open(brick: u32, local: u32) -> bool {
    let lowest = vec3<u32>(local % 5u, (local / 5u) % 5u, local / 25u);
    if any(lowest >= vec3<u32>(BRICK_CELLS)) {
        return false;
    }
    var open = true;
    for (var corner = 0u; corner < 8u; corner++) {
        let offset = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, corner >> 2u);
        let at = lowest + offset;
        let probe = probe_words[brick * 125u + at.x + at.y * 5u + at.z * 25u];
        let bits = probe_words[relight.offsets.x + probe];
        // Toward the cell along each axis: + (free distance 2a) from a low corner, − (2a + 1)
        // from a high one, 5 bits each.
        let shifts = vec3<u32>(0u, 10u, 20u) + offset * 5u;
        let free = (vec3<u32>(bits) >> shifts) & vec3<u32>(31u);
        let inside = (bits & PROBE_EDGE_BIT) == 0u;
        open = open && (bits & PROBE_VALID_BIT) != 0u && inside && all(free == vec3<u32>(31u));
    }
    return open;
}

// ---------------------------------------------------------------------------------------------
// 4. The characters' ambient.

// One channel's L1 irradiance as (constant, gradient): E(n) = constant + dot(gradient, n).
fn l1_gradient(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(SH_C0 * c.x, SH_C1 * c.w, SH_C1 * c.y, SH_C1 * c.z);
}

@compute @workgroup_size(AMBIENT_WORKGROUP)
fn cs_character_ambient(@builtin(global_invocation_id) id: vec3<u32>) {
    // Entry 0 is the world's fallback, which keeps the frame's hemisphere.
    let entry = id.x + 1u;
    if entry >= min(ambient.counts.y, arrayLength(&ambient_entries)) {
        return;
    }
    let character = ambient_entries[entry];
    if character.ambient.probe.w <= 0.0 {
        return;
    }
    let center = character.ambient.probe.xyz;
    let head = select(center, character.shadow.probe.xyz, character.shadow.probe.w > 0.0);
    let at_center = grid_lookup(center, vec3<f32>(0.0), vec3<f32>(0.0));
    let at_head = grid_lookup(head, vec3<f32>(0.0), vec3<f32>(0.0));
    let weights = at_center.weight + at_head.weight;
    if weights <= 0.0 {
        return;
    }
    // The grid's L1 at the two points, averaged by their weights, as (constant, gradient) per
    // channel, blended with the frame's hemisphere (constant (sky + ground) / 2, gradient
    // (sky − ground) / 2 along +Y) by how much the grid speaks for the character.
    let share = weights * 0.5;
    let sky = frame.ambient_sky.rgb;
    let ground = frame.ambient_ground.rgb;
    let center_weight = at_center.weight / weights;
    let head_weight = at_head.weight / weights;
    let grid = array<vec4<f32>, 3>(
        l1_gradient(at_center.red) * center_weight + l1_gradient(at_head.red) * head_weight,
        l1_gradient(at_center.green) * center_weight + l1_gradient(at_head.green) * head_weight,
        l1_gradient(at_center.blue) * center_weight + l1_gradient(at_head.blue) * head_weight,
    );
    var constant: vec3<f32>;
    var gradient: array<vec3<f32>, 3>;
    for (var c = 0u; c < 3u; c++) {
        let hemisphere = vec4<f32>(0.5 * (sky[c] + ground[c]), 0.0, 0.5 * (sky[c] - ground[c]), 0.0);
        let blended = mix(hemisphere, grid[c], share);
        constant[c] = blended.x;
        gradient[c] = blended.yzw;
    }
    // One axis for the three channels, the luminance's gradient; each channel's sky and ground
    // are its constant plus and minus its gradient along it.
    let luminance = gradient[0] * LUMINANCE.x + gradient[1] * LUMINANCE.y + gradient[2] * LUMINANCE.z;
    let axis = select(vec3<f32>(0.0, 1.0, 0.0), normalize(luminance), dot(luminance, luminance) > 1e-12);
    let along = vec3<f32>(dot(gradient[0], axis), dot(gradient[1], axis), dot(gradient[2], axis));
    let visibility = character.ambient.axis.w;
    ambient_entries[entry].ambient.sky = vec4<f32>(max(constant + along, vec3<f32>(0.0)) * visibility, 0.0);
    ambient_entries[entry].ambient.ground = vec4<f32>(max(constant - along, vec3<f32>(0.0)) * visibility, 0.0);
    ambient_entries[entry].ambient.axis = vec4<f32>(axis, visibility);
}
