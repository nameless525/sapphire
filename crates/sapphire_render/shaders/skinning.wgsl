// Compute skinning (M1 R2, spec §5.12 "Skinning and morph targets"): for every skinned or
// morphed instance that needs a new pose this frame, morph targets and then linear blend
// skinning, written to the instance's own output streams, which every later pass draws from.
//
// One dispatch covers all of a frame's instances (see `skinning.rs` for why). Each instance
// owns a run of whole workgroups, listed in its job; a workgroup finds its job by binary search,
// which is uniform across the workgroup.
//
// Per vertex, in this order:
// 1. Decode the bind-space position (unorm16 across the mesh's bounds, or f32) and shading
//    normal to f32, as the vertex shaders would.
// 2. Add each morph target's delta times its weight, walking the vertex's deltas (regrouped by
//    vertex at upload, targets in order), then renormalize the normal. The outline normal turns
//    with it, by the rotation that takes the bind pose's normal to the morphed one, so it follows
//    the morphed surface (M1 C4).
// 3. Blend the palette matrices by the vertex's four unorm8 weights and transform the position,
//    normal and outline normal (the normals by the blended linear part, renormalized: exact for
//    rotation and uniform scale). The outline normal is transformed itself, not rebuilt from the
//    deformed normal's frame: where linear blending isn't a rotation (a bending joint), the split
//    vertices of a hard edge share one outline normal and must get one result, which only the
//    same matrix applied to the same vector guarantees, so the hull stays closed.
// 4. Write the position as f32 and both normals octahedral snorm16 (`VertexShading`), the
//    layouts the f32 mesh pipelines read: the outline normal in the tangent frame of the normal
//    as written (frame.wgsl's `outline_from_tangent_space` undoes it).
//
// Standalone, not prefixed with frame.wgsl: it binds its own group 0. `oct_decode`, `oct_encode`
// and the outline frame mirror frame.wgsl's until WESL imports share them (§5.12).

// Mirrors `skinning::WORKGROUP_SIZE`.
const SKINNING_WORKGROUP: u32 = 64u;
// 16-byte rows per job (`skinning::Job`).
const JOB_ROWS: u32 = 5u;

// `Job::flags` (`skinning::flags`).
const FLAG_FLOAT_POSITIONS: u32 = 1u;
const FLAG_SKIN: u32 = 2u;
const FLAG_WIDE_INFLUENCES: u32 = 4u;
const FLAG_MORPHS: u32 = 8u;

// Every skinned or morphed mesh's streams, skin and regrouped morph deltas, as words
// (`mesh_pool.rs`, "Skinned and morphed meshes").
@group(0) @binding(0) var<storage, read> sources: array<u32>;
// This frame: a header (job count, workgroups), the jobs, the palettes as three rows of a 3×4
// matrix each, then the morph weights, as 16-byte rows of words.
@group(0) @binding(1) var<storage, read> frame_data: array<vec4<u32>>;
// Every instance's outputs: two halves of f32 positions (this frame's and the last drawn one),
// then the shading words.
@group(0) @binding(2) var<storage, read_write> outputs: array<u32>;

struct Job {
    first_workgroup: u32,
    vertex_count: u32,
    flags: u32,
    // Row of the first palette matrix in `frame_data`.
    palette: u32,
    // Words in `sources`.
    positions: u32,
    shading: u32,
    influences: u32,
    morph_offsets: u32,
    morph_deltas: u32,
    // Word of the first morph weight in `frame_data`.
    weights: u32,
    // Words in `outputs`.
    out_positions: u32,
    out_shading: u32,
    position_offset: vec3<f32>,
    position_scale: vec3<f32>,
}

fn load_job(index: u32) -> Job {
    let row = 1u + index * JOB_ROWS;
    let a = frame_data[row];
    let b = frame_data[row + 1u];
    let c = frame_data[row + 2u];
    var job: Job;
    job.first_workgroup = a.x;
    job.vertex_count = a.y;
    job.flags = a.z;
    job.palette = a.w;
    job.positions = b.x;
    job.shading = b.y;
    job.influences = b.z;
    job.morph_offsets = b.w;
    job.morph_deltas = c.x;
    job.weights = c.y;
    job.out_positions = c.z;
    job.out_shading = c.w;
    job.position_offset = bitcast<vec4<f32>>(frame_data[row + 3u]).xyz;
    job.position_scale = bitcast<vec4<f32>>(frame_data[row + 4u]).xyz;
    return job;
}

fn morph_weight(word: u32) -> f32 {
    return bitcast<f32>(frame_data[word >> 2u][word & 3u]);
}

// Two binary16 values, low half first. `unpack2x16float` would do, but naga accepts it only with
// the f16-in-f32 shader capability, which not every backend has.
fn unpack_halves(word: u32) -> vec2<f32> {
    return vec2<f32>(half_to_f32(word & 0xffffu), half_to_f32(word >> 16u));
}

// Exact: a normal binary16 rebiases its exponent (15 to 127), and a subnormal is its mantissa ×
// 2^-24. Deltas are finite (checked when the mesh is read), so the all-ones exponent never
// occurs.
fn half_to_f32(bits: u32) -> f32 {
    let exponent = (bits >> 10u) & 0x1fu;
    let mantissa = bits & 0x3ffu;
    let normal = bitcast<f32>(((exponent + 112u) << 23u) | (mantissa << 13u));
    let magnitude = select(f32(mantissa) * 0x1p-24f, normal, exponent != 0u);
    return select(magnitude, -magnitude, (bits & 0x8000u) != 0u);
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let z = 1.0 - abs(e.x) - abs(e.y);
    let t = max(-z, 0.0);
    let xy = e + select(vec2<f32>(t), vec2<f32>(-t), e >= vec2<f32>(0.0));
    return normalize(vec3<f32>(xy, z));
}

// The octahedral encoding of a direction (Cigolle et al., JCGT 2014), rounded to the nearest
// snorm16 by `pack2x16snorm`; a zero vector encodes +Z, as `sapphire_assets::mesh::oct_encode`.
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let l1 = abs(n.x) + abs(n.y) + abs(n.z);
    var e = select(vec2<f32>(0.0), n.xy / l1, l1 > 0.0);
    if n.z < 0.0 {
        e = (1.0 - abs(e.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), e >= vec2<f32>(0.0));
    }
    return e;
}

// The outline frame (frame.wgsl, `sapphire_assets::mesh::outline_frame`).
const OUTLINE_FRAME_AXIS: vec3<f32> = vec3<f32>(0.4, 0.55, 0.73);
const OUTLINE_FRAME_FALLBACK: vec3<f32> = vec3<f32>(0.83, -0.31, 0.46);
const OUTLINE_FRAME_MIN_CROSS: f32 = 1e-4;

fn outline_frame_tangent(n: vec3<f32>) -> vec3<f32> {
    let tangent = cross(OUTLINE_FRAME_AXIS, n);
    let fallback = cross(OUTLINE_FRAME_FALLBACK, n);
    return normalize(select(fallback, tangent, dot(tangent, tangent) > OUTLINE_FRAME_MIN_CROSS));
}

// The outline normal stored as `tangent_space` around the unit normal `n`, in `n`'s space.
fn outline_from_tangent_space(n: vec3<f32>, tangent_space: vec3<f32>) -> vec3<f32> {
    let tangent = outline_frame_tangent(n);
    return normalize(tangent * tangent_space.x + cross(n, tangent) * tangent_space.y + n * tangent_space.z);
}

// The unit `outline` in the outline frame of the unit normal `n`.
fn outline_to_tangent_space(n: vec3<f32>, outline: vec3<f32>) -> vec3<f32> {
    let tangent = outline_frame_tangent(n);
    return vec3<f32>(dot(outline, tangent), dot(outline, cross(n, tangent)), dot(outline, n));
}

// `v` turned by the rotation that takes the unit vector `start` to the unit vector `end` along
// their great circle (Rodrigues' formula), or left as it is where they point opposite ways.
fn rotate_between(v: vec3<f32>, start: vec3<f32>, end: vec3<f32>) -> vec3<f32> {
    let axis = cross(start, end);
    let c = dot(start, end);
    let turned = v * c + cross(axis, v) + axis * (dot(axis, v) / max(1.0 + c, 1e-6));
    return select(v, turned, c > -0.9999);
}

fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(v, v);
    return select(vec3<f32>(0.0, 0.0, 1.0), v * inverseSqrt(length_squared), length_squared > 1e-24);
}

fn palette_row(row: u32) -> vec4<f32> {
    return bitcast<vec4<f32>>(frame_data[row]);
}

@compute @workgroup_size(SKINNING_WORKGROUP)
fn cs_skinning(
    @builtin(workgroup_id) workgroup: vec3<u32>,
    @builtin(num_workgroups) workgroups: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    let header = frame_data[0];
    let linear = workgroup.y * workgroups.x + workgroup.x;
    if linear >= header.y {
        return;
    }
    // The last job starting at or before this workgroup.
    var low = 0u;
    var high = header.x;
    loop {
        if high - low <= 1u {
            break;
        }
        let middle = (low + high) / 2u;
        if frame_data[1u + middle * JOB_ROWS].x <= linear {
            low = middle;
        } else {
            high = middle;
        }
    }
    let job = load_job(low);
    let vertex = (linear - job.first_workgroup) * SKINNING_WORKGROUP + local;
    if vertex >= job.vertex_count {
        return;
    }

    var position: vec3<f32>;
    if (job.flags & FLAG_FLOAT_POSITIONS) != 0u {
        let word = job.positions + 3u * vertex;
        position = vec3<f32>(
            bitcast<f32>(sources[word]),
            bitcast<f32>(sources[word + 1u]),
            bitcast<f32>(sources[word + 2u]),
        );
    } else {
        let word = job.positions + 2u * vertex;
        let quantized = vec3<f32>(unpack2x16unorm(sources[word]), unpack2x16unorm(sources[word + 1u]).x);
        position = job.position_offset + job.position_scale * quantized;
    }
    var normal_word = sources[job.shading + 2u * vertex];
    var outline_word = sources[job.shading + 2u * vertex + 1u];
    let deforms = (job.flags & (FLAG_MORPHS | FLAG_SKIN)) != 0u;
    // The outline normal in object space, while the vertex deforms.
    var outline = vec3<f32>(0.0, 0.0, 1.0);
    if deforms {
        outline = outline_from_tangent_space(
            oct_decode(unpack2x16snorm(normal_word)),
            oct_decode(unpack2x16snorm(outline_word)),
        );
    }

    if (job.flags & FLAG_MORPHS) != 0u {
        let bind_normal = oct_decode(unpack2x16snorm(normal_word));
        var normal = bind_normal;
        let first = sources[job.morph_offsets + vertex];
        let end = sources[job.morph_offsets + vertex + 1u];
        for (var delta = first; delta < end; delta++) {
            let word = job.morph_deltas + 4u * delta;
            let weight = morph_weight(job.weights + sources[word]);
            if weight != 0.0 {
                let a = unpack_halves(sources[word + 1u]);
                let b = unpack_halves(sources[word + 2u]);
                let c = unpack_halves(sources[word + 3u]);
                position += weight * vec3<f32>(a, b.x);
                normal += weight * vec3<f32>(b.y, c);
            }
        }
        let morphed = safe_normalize(normal);
        normal_word = pack2x16snorm(oct_encode(morphed));
        outline = rotate_between(outline, bind_normal, morphed);
    }

    if (job.flags & FLAG_SKIN) != 0u {
        var indices: vec4<u32>;
        var weight_word: u32;
        if (job.flags & FLAG_WIDE_INFLUENCES) != 0u {
            let word = job.influences + 3u * vertex;
            let low_pair = sources[word];
            let high_pair = sources[word + 1u];
            indices = vec4<u32>(low_pair & 0xffffu, low_pair >> 16u, high_pair & 0xffffu, high_pair >> 16u);
            weight_word = sources[word + 2u];
        } else {
            let word = job.influences + 2u * vertex;
            let packed = sources[word];
            indices = (vec4<u32>(packed) >> vec4<u32>(0u, 8u, 16u, 24u)) & vec4<u32>(0xffu);
            weight_word = sources[word + 1u];
        }
        let weights = unpack4x8unorm(weight_word);
        // The blended matrix's rows. Unused slots weigh 0 and are skipped.
        var rows = array<vec4<f32>, 3>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
        for (var i = 0u; i < 4u; i++) {
            let weight = weights[i];
            if weight > 0.0 {
                let row = job.palette + 3u * indices[i];
                rows[0] += weight * palette_row(row);
                rows[1] += weight * palette_row(row + 1u);
                rows[2] += weight * palette_row(row + 2u);
            }
        }
        let p = vec4<f32>(position, 1.0);
        position = vec3<f32>(dot(rows[0], p), dot(rows[1], p), dot(rows[2], p));
        let linear_part = transpose(mat3x3<f32>(rows[0].xyz, rows[1].xyz, rows[2].xyz));
        let normal = linear_part * oct_decode(unpack2x16snorm(normal_word));
        normal_word = pack2x16snorm(oct_encode(safe_normalize(normal)));
        outline = linear_part * outline;
    }
    if deforms {
        // In the frame of the normal as the passes after decode it.
        let written = oct_decode(unpack2x16snorm(normal_word));
        outline_word = pack2x16snorm(oct_encode(outline_to_tangent_space(written, safe_normalize(outline))));
    }

    let out = job.out_positions + 3u * vertex;
    outputs[out] = bitcast<u32>(position.x);
    outputs[out + 1u] = bitcast<u32>(position.y);
    outputs[out + 2u] = bitcast<u32>(position.z);
    outputs[job.out_shading + 2u * vertex] = normal_word;
    outputs[job.out_shading + 2u * vertex + 1u] = outline_word;
}
