// The frame's bends (M5 L6; `sapphire_render::vfx::distortion`, whose docs have the design): effect meshes that
// move the picture behind them, drawn into a buffer of screen offsets at a share of post's size, which post's
// fused pass reads the frame through. It stands alone: none of frame.wgsl's groups are bound.
//
// An offset is in frame heights, x to the right and y down, and says where a pixel reads the frame from:
// meshes add theirs. A pixel no mesh covers holds 0, the target's clear value.

// `vfx::distortion::DEPTH_CONSTANT`: over temporal AA's frame, each fragment tested against the scene prepass's
// depth by hand and faded into the surfaces behind it; otherwise there's no depth to read, and nothing is tested.
override VFX_DISTORTION_DEPTH: bool = false;

// `GpuVfxDistortion::flags`.
const VFX_DISTORTION_ERODES: u32 = 1u;
const VFX_DISTORTION_SHIMMER: u32 = 2u;
const VFX_DISTORTION_FLIPBOOK: u32 = 4u;
// The widest an erosion's edge is smoothed over, as `vfx_mesh.wgsl`'s.
const VFX_DISTORTION_EDGE_AA: f32 = 0.05;
// Where a shimmer reads its pattern the second time, in repeats: far enough that the two reads are unrelated.
const VFX_DISTORTION_APART: vec2<f32> = vec2<f32>(0.37, 0.61);
// Near the camera a bend fades out by its fragment's view depth, in metres: none nearer than the first, whole past
// the second. A column or dome the camera passes through would otherwise bend the whole screen at full strength.
const VFX_DISTORTION_NEAR_NONE: f32 = 0.5;
const VFX_DISTORTION_NEAR_WHOLE: f32 = 1.5;

// Mirrors `vfx::distortion::GpuVfxDistortion`.
struct VfxDistortion {
    world_from_local: mat4x4<f32>,
    scroll: vec2<f32>,
    tiling: vec2<f32>,
    mask_tiling: vec2<f32>,
    threshold: f32,
    edge: f32,
    // xy: this frame's cell's corner in the sheet; zw: the next frame's.
    cells: vec4<f32>,
    cell_size: vec2<f32>,
    blend: f32,
    strength: f32,
    alpha: f32,
    soft: f32,
    pattern: u32,
    mask: u32,
    flags: u32,
    padding_a: u32,
    padding_b: u32,
    padding_c: u32,
}

// Mirrors `vfx::distortion::GpuVfxDistortionUniforms`.
struct VfxDistortionUniforms {
    // The main view's, without temporal AA's jitter: the frame post reads is the resolved one, or unjittered.
    clip_from_world: mat4x4<f32>,
    // xy: the offsets' size in texels; zw: reciprocals.
    target_size: vec4<f32>,
    // x: the frame's width over its height; y: the camera's near plane.
    frame: vec4<f32>,
}

// Group 0: the frame's bends and their constants.
@group(0) @binding(0) var<uniform> vfx_distortion: VfxDistortionUniforms;
@group(0) @binding(1) var<storage, read> vfx_distortions: array<VfxDistortion>;
// Repeats, so a pattern scrolls with no seam.
@group(0) @binding(2) var vfx_distortion_sampler: sampler;
// Nearest, for the depth's gather.
@group(0) @binding(3) var vfx_distortion_point: sampler;
// Group 1: the effect meshes' generated patterns (`VfxPattern`), a layer each, and the scene prepass's depth
// over temporal AA's frame, else a 1×1 stand-in nothing reads.
@group(1) @binding(0) var vfx_distortion_patterns: texture_2d_array<f32>;
@group(1) @binding(1) var vfx_distortion_depth: texture_depth_2d;

struct VfxDistortionVertex {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // How much of the layer shows here: 0 on the shape's open borders.
    @location(2) fade: f32,
}

struct VfxDistortionVaryings {
    @builtin(position) clip_position: vec4<f32>,
    // xy: where the pattern is read, this frame's cell of a flipbook; zw: the next frame's, or xy again.
    @location(0) uv: vec4<f32>,
    // Where the erosion's mask is read.
    @location(1) mask_uv: vec2<f32>,
    // x: the strength × alpha × the shape's fade; y: the erosion's threshold; z: its edge; w: toward the next frame.
    @location(2) amount: vec4<f32>,
    // xy: the mesh's middle on the offsets, in texture coordinates; z: 1 where it's in front of the camera;
    // w: its soft distance.
    @location(3) @interpolate(flat) middle: vec4<f32>,
    // x: the pattern's layer; y: the mask's; z: the flags.
    @location(4) @interpolate(flat) layers: vec3<u32>,
    // The fragment's view depth, in metres: clip space's w.
    @location(5) depth: f32,
}

@vertex
fn vs_vfx_distortion(vertex: VfxDistortionVertex, @builtin(instance_index) instance: u32) -> VfxDistortionVaryings {
    let mesh = vfx_distortions[instance];
    let world = mesh.world_from_local * vec4<f32>(vertex.position, 1.0);
    var out: VfxDistortionVaryings;
    out.clip_position = vfx_distortion.clip_from_world * world;
    out.depth = out.clip_position.w;
    let pattern_uv = vertex.uv * mesh.tiling - mesh.scroll;
    out.uv = vec4<f32>(pattern_uv, pattern_uv);
    if (mesh.flags & VFX_DISTORTION_FLIPBOOK) != 0u {
        let inside = clamp(vertex.uv, vec2<f32>(0.0), vec2<f32>(1.0)) * mesh.cell_size;
        out.uv = vec4<f32>(mesh.cells.xy + inside, mesh.cells.zw + inside);
    }
    out.mask_uv = vertex.uv * mesh.mask_tiling;
    out.amount = vec4<f32>(mesh.strength * mesh.alpha * vertex.fade, mesh.threshold, mesh.edge, mesh.blend);
    let middle = vfx_distortion.clip_from_world * vec4<f32>(mesh.world_from_local[3].xyz, 1.0);
    let ahead = middle.w > 1e-4;
    let ndc = middle.xy / select(1.0, middle.w, ahead);
    out.middle = vec4<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, select(0.0, 1.0, ahead), mesh.soft);
    out.layers = vec3<u32>(mesh.pattern, mesh.mask, mesh.flags);
    return out;
}

// How much of it shows past the scene's surfaces over temporal AA's frame: `vfx_mesh.wgsl`'s fade, at this
// target's size.
fn vfx_distortion_scene_fade(in: VfxDistortionVaryings) -> f32 {
    var fade = 1.0;
    if VFX_DISTORTION_DEPTH {
        let uv = in.clip_position.xy * vfx_distortion.target_size.zw;
        let depths = textureGather(vfx_distortion_depth, vfx_distortion_point, uv);
        let near = vfx_distortion.frame.y;
        let own = near / in.clip_position.z;
        let scene = near / max(depths, vec4<f32>(1e-9));
        fade = dot(saturate((scene - vec4<f32>(own)) / max(in.middle.w, 1e-4)), vec4<f32>(0.25));
    }
    return fade;
}

// The offset this fragment adds, in frame heights: every sample and derivative read before anything is decided.
@fragment
fn fs_vfx_distortion(in: VfxDistortionVaryings) -> @location(0) vec4<f32> {
    let now = textureSample(vfx_distortion_patterns, vfx_distortion_sampler, in.uv.xy, in.layers.x).r;
    let next = textureSample(vfx_distortion_patterns, vfx_distortion_sampler, in.uv.zw, in.layers.x).r;
    let apart = textureSample(vfx_distortion_patterns, vfx_distortion_sampler, in.uv.xy + VFX_DISTORTION_APART, in.layers.x).r;
    let mask = textureSample(vfx_distortion_patterns, vfx_distortion_sampler, in.mask_uv, in.layers.y).r;
    let half_aa = clamp(fwidth(mask), 1e-4, VFX_DISTORTION_EDGE_AA) * 0.5;
    let pattern = mix(now, next, in.amount.w);
    // The erosion, as `vfx_mesh.wgsl`'s: its edge bends as the body does, since it has no colour here.
    let threshold = in.amount.y;
    let edge = in.amount.z;
    let low = threshold * (1.0 + edge) - edge;
    let erodes = (in.layers.z & VFX_DISTORTION_ERODES) != 0u;
    var shown = smoothstep(low - half_aa, low + half_aa, mask);
    shown = select(shown, 1.0, !erodes || threshold <= 0.0);
    shown = select(shown, 0.0, erodes && threshold >= 1.0);
    let near = smoothstep(VFX_DISTORTION_NEAR_NONE, VFX_DISTORTION_NEAR_WHOLE, in.depth);
    let amount = in.amount.x * shown * vfx_distortion_scene_fade(in) * near;
    // From the middle to the fragment, in frame heights.
    let uv = in.clip_position.xy * vfx_distortion.target_size.zw;
    let away = (uv - in.middle.xy) * vec2<f32>(vfx_distortion.frame.x, 1.0);
    let reach = length(away);
    // Outward: the fragment reads nearer the middle, so the picture moves away from it. Nothing at the middle
    // itself, where there's no way out, nor for a middle behind the camera.
    let outward = select(vec2<f32>(0.0), -away / max(reach, 1e-6) * pattern * in.middle.z, reach > 1e-6);
    // A shimmer: two reads of the pattern about its mean, each way at once.
    let shimmer = (vec2<f32>(now, apart) - 0.5) * 2.0;
    let shimmers = (in.layers.z & VFX_DISTORTION_SHIMMER) != 0u;
    return vec4<f32>(select(outward, shimmer, shimmers) * amount, 0.0, 0.0);
}
