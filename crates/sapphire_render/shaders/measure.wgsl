// The measurement counters (M6 B6; `sapphire_render::measure`, m6-plan §3's "Measurement switches"): what the
// character pixel's probes count on the GPU, in a dev sandbox's counted runs alone. Two compute passes, each
// adding into four counters a frame, which the renderer reads back a few frames late:
//
//   0: character pixels: the main view's pixels a listed character's surface or outline hull is nearest at,
//      where the main pass's scene colour carries the character mask (alpha 0; under MSAA, under a half);
//   1: hull pixels: of those, the ones no listed character's surface covers in the character prepass's IDs: a
//      hull line outside every character's silhouette (one over another character behind it goes uncounted);
//   2: upscale groups: the phones' edge upscaler's output in groups of 32 pixels, 8 wide and 4 high, counted
//      where the frame upscales;
//   3: flat upscale groups: of those, the groups each of whose pixels has its four nearest input texels equal,
//      where the upscale's result is that texel exactly (m6-plan F1's exact flat path). A GPU runs a fragment
//      shader in groups of its own choosing, 32 pixels wide on Apple's and most phones'; 8 × 4 tiles stand in.
//
// It stands alone: no frame group, nothing shared with the scene's programs.

// Mirrors `measure::MeasureUniforms`.
struct MeasureUniforms {
    // The upscaler's (`UpscaleUniforms::easu`): xy, the input's size over the output's; zw, -0.5. A pixel's
    // center times xy plus zw is its place among the input's texel centers, as the edge pass finds it.
    easu: vec4<f32>,
    // xy: the upscaler's output size in pixels.
    output: vec4<u32>,
}

@group(0) @binding(0) var<storage, read_write> counts: array<atomic<u32>, 4>;
@group(0) @binding(1) var<uniform> measure: MeasureUniforms;

// The pixels' pass: the main pass's resolved scene colour and the character prepass's IDs, both at the scene's
// size.
@group(1) @binding(0) var scene_color: texture_2d<f32>;
@group(1) @binding(1) var character_ids: texture_2d<u32>;

// The upscale's pass: the fused pass's output at the scene's size, which the edge pass upscales.
@group(1) @binding(2) var upscale_input: texture_2d<f32>;

// A character ID's entry bits (`CHARACTER_ID_ENTRY_MASK` in frame.wgsl): 0 where no character's surface is.
const ID_ENTRY_MASK: u32 = 16383u;

var<workgroup> characters: atomic<u32>;
var<workgroup> hulls: atomic<u32>;
var<workgroup> not_flat: atomic<u32>;

@compute @workgroup_size(8, 8)
fn cs_measure_pixels(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    let size = textureDimensions(scene_color);
    if all(id.xy < size) {
        if textureLoad(scene_color, id.xy, 0).a < 0.5 {
            atomicAdd(&characters, 1u);
            if (textureLoad(character_ids, id.xy, 0).r & ID_ENTRY_MASK) == 0u {
                atomicAdd(&hulls, 1u);
            }
        }
    }
    workgroupBarrier();
    if index == 0u {
        atomicAdd(&counts[0], atomicLoad(&characters));
        atomicAdd(&counts[1], atomicLoad(&hulls));
    }
}

// The input texel `offset` from `corner`, held inside the input, as the edge pass's nearest sampler holds it.
fn upscale_texel(corner: vec2<i32>, offset: vec2<i32>, last: vec2<i32>) -> vec4<f32> {
    return textureLoad(upscale_input, clamp(corner + offset, vec2<i32>(0), last), 0);
}

@compute @workgroup_size(8, 4)
fn cs_measure_upscale(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    let output = measure.output.xy;
    if all(id.xy < output) {
        let place = (vec2<f32>(id.xy) + 0.5) * measure.easu.xy + measure.easu.zw;
        let corner = vec2<i32>(floor(place));
        let last = vec2<i32>(textureDimensions(upscale_input)) - 1;
        let f = upscale_texel(corner, vec2<i32>(0, 0), last);
        let g = upscale_texel(corner, vec2<i32>(1, 0), last);
        let j = upscale_texel(corner, vec2<i32>(0, 1), last);
        let k = upscale_texel(corner, vec2<i32>(1, 1), last);
        let flat = all(f.rgb == g.rgb) && all(f.rgb == j.rgb) && all(f.rgb == k.rgb);
        if !flat {
            atomicStore(&not_flat, 1u);
        }
    }
    workgroupBarrier();
    // A group wholly past the output's edge isn't one; one partly past it counts its pixels inside.
    if index == 0u && all(group.xy * vec2<u32>(8u, 4u) < output) {
        atomicAdd(&counts[2], 1u);
        if atomicLoad(&not_flat) == 0u {
            atomicAdd(&counts[3], 1u);
        }
    }
}
