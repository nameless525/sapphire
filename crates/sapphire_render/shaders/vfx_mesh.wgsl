// The frame's effect meshes (M5 L5; `sapphire_render::vfx::mesh`, whose docs have the design): generated
// shapes, each an instance drawn indexed, unlit, with a generated pattern scrolled over them, an erosion and a
// flipbook. The program is frame.wgsl, materials.wgsl, then this file.
//
// Blending is the sprites' (vfx.wgsl): premultiplied, its alpha the share of what's behind it that it covers,
// coverage × (1 − additive). The fog and the exposure are the vertex stage's, as a sprite's are.

// `vfx::mesh::AFTER_RESOLVE_CONSTANT`: over temporal AA's resolved frame, unjittered, testing depth by hand and
// fading into the scene's surfaces; otherwise in the main pass, against its depth, with no soft fade.
override VFX_MESH_AFTER_RESOLVE: bool = false;

// `GpuVfxMesh::flags`.
const VFX_MESH_ERODES: u32 = 1u;
const VFX_MESH_FLIPBOOK: u32 = 2u;
// Bit 2 (`vfx::mesh::DISTORTS`) is kept for M5 L6: a mesh that offsets what's behind it rather than adding
// light writes its offset to a second target there, and post's fused pass applies it. Nothing sets it yet.
// The widest an erosion's edge is smoothed over, in the mask's values, however far its texels spread.
const VFX_MESH_EDGE_AA: f32 = 0.05;

// Mirrors `vfx::mesh::GpuVfxMesh`.
struct VfxMesh {
    world_from_local: mat4x4<f32>,
    color: vec3<f32>,
    alpha: f32,
    edge_color: vec3<f32>,
    edge: f32,
    scroll: vec2<f32>,
    tiling: vec2<f32>,
    mask_tiling: vec2<f32>,
    threshold: f32,
    additive: f32,
    // xy: this frame's cell's corner in the sheet; zw: the next frame's.
    cells: vec4<f32>,
    cell_size: vec2<f32>,
    blend: f32,
    soft: f32,
    pattern: u32,
    mask: u32,
    flags: u32,
    padding: u32,
}

// Mirrors `vfx::mesh::GpuVfxMeshUniforms`.
struct VfxMeshUniforms {
    // xy: the output's size in pixels; zw: reciprocals.
    target_size: vec4<f32>,
    // What a color is multiplied by: the reciprocal of the frame's exposure.
    emissive: f32,
    padding_a: f32,
    padding_b: f32,
    padding_c: f32,
}

// The vertex stage alone reads the meshes: the fragment stage has no storage buffer to spare (frame.wgsl).
@group(2) @binding(0) var<storage, read> vfx_meshes: array<VfxMesh>;
@group(2) @binding(1) var<uniform> vfx_mesh: VfxMeshUniforms;
// Repeats, so a pattern scrolls with no seam.
@group(2) @binding(2) var vfx_mesh_sampler: sampler;

// The generated patterns, a layer each (`VfxPattern`).
@group(3) @binding(0) var vfx_mesh_patterns: texture_2d_array<f32>;
// The scene prepass's depth over temporal AA's frame, else a 1×1 stand-in nothing reads.
@group(3) @binding(1) var vfx_mesh_scene_depth: texture_depth_2d;

struct VfxMeshVertex {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // How much of the layer shows here: 0 on the shape's open borders.
    @location(2) fade: f32,
}

struct VfxMeshVaryings {
    @builtin(position) clip_position: vec4<f32>,
    // xy: where the pattern is read, this frame's cell of a flipbook; zw: the next frame's, or xy again.
    @location(0) uv: vec4<f32>,
    // Where the erosion's mask is read.
    @location(1) mask_uv: vec2<f32>,
    // rgb: the color with the fog over it, without the fog's own light. a: opacity, with the shape's fade.
    @location(2) color: vec4<f32>,
    // rgb: the erosion edge's color likewise. a: the edge's width.
    @location(3) edge: vec4<f32>,
    // rgb: the fog's own light in front of it, where it covers. a: its soft distance.
    @location(4) air: vec4<f32>,
    // x: the erosion's threshold; y: how far toward the next frame; z: 1 − additive.
    @location(5) params: vec3<f32>,
    // x: the pattern's layer; y: the mask's; z: the flags.
    @location(6) @interpolate(flat) layers: vec3<u32>,
}

fn vfx_mesh_clip(world: vec3<f32>) -> vec4<f32> {
    if VFX_MESH_AFTER_RESOLVE {
        return view.unjittered_clip_from_world * vec4<f32>(world, 1.0);
    }
    return view.clip_from_world * vec4<f32>(world, 1.0);
}

@vertex
fn vs_vfx_mesh(vertex: VfxMeshVertex, @builtin(instance_index) instance: u32) -> VfxMeshVaryings {
    let mesh = vfx_meshes[instance];
    let world = (mesh.world_from_local * vec4<f32>(vertex.position, 1.0)).xyz;
    var out: VfxMeshVaryings;
    out.clip_position = vfx_mesh_clip(world);
    // A scroll moves the pattern toward +u and +v: along a slash's sweep, from its tail to its head.
    let pattern_uv = vertex.uv * mesh.tiling - mesh.scroll;
    out.uv = vec4<f32>(pattern_uv, pattern_uv);
    if (mesh.flags & VFX_MESH_FLIPBOOK) != 0u {
        // A frame fills the shape's UVs, inside its cell.
        let inside = clamp(vertex.uv, vec2<f32>(0.0), vec2<f32>(1.0)) * mesh.cell_size;
        out.uv = vec4<f32>(mesh.cells.xy + inside, mesh.cells.zw + inside);
    }
    out.mask_uv = vertex.uv * mesh.mask_tiling;
    // The fog as a sprite's (vfx.wgsl's `vfx_fogged`): one ray's transmittance over its light, and the fog's
    // own light only for the share of what's behind that it covers.
    var transmittance = 1.0;
    var air = vec3<f32>(0.0);
    if fog_on() {
        let offset = world - view.position.xyz;
        let distance = length(offset);
        let ray = fog_ray(view.position.xyz, offset / max(distance, 1e-6), distance, 1.0);
        transmittance = ray.transmittance;
        air = (ray.light + ray.sun) * (1.0 - ray.transmittance);
    }
    let light = vfx_mesh.emissive * transmittance;
    out.color = vec4<f32>(mesh.color * light, mesh.alpha * vertex.fade);
    out.edge = vec4<f32>(mesh.edge_color * light, mesh.edge);
    out.air = vec4<f32>(air * (1.0 - mesh.additive), mesh.soft);
    out.params = vec3<f32>(mesh.threshold, mesh.blend, 1.0 - mesh.additive);
    out.layers = vec3<u32>(mesh.pattern, mesh.mask, mesh.flags);
    return out;
}

// What the layer shows at a pixel: how much it covers, and its color there.
struct VfxMeshSurface {
    coverage: f32,
    color: vec3<f32>,
}

// Read in uniform control flow: every sample and derivative before anything is decided.
fn vfx_mesh_surface(in: VfxMeshVaryings) -> VfxMeshSurface {
    let now = textureSample(vfx_mesh_patterns, vfx_mesh_sampler, in.uv.xy, in.layers.x).r;
    let next = textureSample(vfx_mesh_patterns, vfx_mesh_sampler, in.uv.zw, in.layers.x).r;
    let mask = textureSample(vfx_mesh_patterns, vfx_mesh_sampler, in.mask_uv, in.layers.y).r;
    let half_aa = clamp(fwidth(mask), 1e-4, VFX_MESH_EDGE_AA) * 0.5;
    let pattern = mix(now, next, in.params.y);
    // The erosion: gone where the mask is under `low`, the edge's color from there over the edge's width,
    // the layer's own past it. `low` runs from −edge to 1 as the threshold does from 0 to 1, so at 0 no edge
    // shows and at 1 nothing does.
    let threshold = in.params.x;
    let edge = in.edge.a;
    let low = threshold * (1.0 + edge) - edge;
    let erodes = (in.layers.z & VFX_MESH_ERODES) != 0u;
    var shown = smoothstep(low - half_aa, low + half_aa, mask);
    var body = smoothstep(low + edge - half_aa, low + edge + half_aa, mask);
    shown = select(shown, 1.0, !erodes || threshold <= 0.0);
    shown = select(shown, 0.0, erodes && threshold >= 1.0);
    body = select(body, 1.0, !erodes || threshold <= 0.0 || edge <= 0.0);
    return VfxMeshSurface(pattern * in.color.a * shown, mix(in.edge.rgb, in.color.rgb, body));
}

// How much of it shows past the scene's surfaces over temporal AA's frame: vfx.wgsl's `vfx_scene_fade`.
fn vfx_mesh_scene_fade(in: VfxMeshVaryings) -> f32 {
    var fade = 1.0;
    if VFX_MESH_AFTER_RESOLVE {
        let depths = textureGather(vfx_mesh_scene_depth, character_sampler, in.clip_position.xy * vfx_mesh.target_size.zw);
        let near = view.clip_from_view[3][2];
        let own = near / in.clip_position.z;
        let scene = near / max(depths, vec4<f32>(1e-9));
        fade = dot(saturate((scene - vec4<f32>(own)) / max(in.air.a, 1e-4)), vec4<f32>(0.25));
    }
    return fade;
}

@fragment
fn fs_vfx_mesh(in: VfxMeshVaryings) -> @location(0) vec4<f32> {
    let surface = vfx_mesh_surface(in);
    let coverage = surface.coverage * vfx_mesh_scene_fade(in);
    return vec4<f32>((surface.color + in.air.rgb) * coverage, coverage * in.params.z);
}

// Writes scene color's alpha alone, in the main pass, as `fs_vfx_mask` does a sprite's.
@fragment
fn fs_vfx_mesh_mask(in: VfxMeshVaryings) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, vfx_mesh_surface(in).coverage);
}
