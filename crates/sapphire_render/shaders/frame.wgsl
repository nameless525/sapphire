// Declarations shared by every Sapphire M0 pass. `shaders.rs` prepends this file to each
// pass's source; WESL imports replace the concatenation in M1 (spec §5.12).
//
// Bind group 0 holds the frame constants and the persistent scene buffers. Group 1 is
// reserved for material textures (ramps, ILM masks, face SDF maps) in M1.
//
// Conventions: right-handed world, Y up; reverse-Z depth (near = 1, infinity = 0); all
// colors linear, with the sRGB target encoding on write.

struct FrameUniforms {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // xyz: world-space camera position; w: time in seconds.
    camera_position: vec4<f32>,
    // xyz: unit vector from the surface toward the sun.
    sun_direction: vec4<f32>,
    // rgb: sun color × intensity.
    sun_radiance: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_ground: vec4<f32>,
    // xy: target size in pixels; zw: reciprocals.
    viewport: vec4<f32>,
}

struct InstanceData {
    world_from_object: mat4x4<f32>,
    // Inverse-transpose of the upper 3×3 of `world_from_object`.
    normal_from_object: mat3x3<f32>,
}

// One draw record, indexed by draw ID. Vertex shaders get the ID from the instance-step
// attribute at location 5 rather than `instance_index`, so the same shaders work for
// CPU-driven draws (ID chosen by vertex-buffer offset, first_instance 0: portable to the iOS
// Simulator) and GPU-driven multi-draw indirect (ID in first_instance, later).
struct DrawData {
    instance: u32,
    material: u32,
}

// Mirrors `sapphire_assets::mesh::ToonMaterialParams`.
struct ToonMaterial {
    base_color: vec4<f32>,
    // Multiplier applied to the albedo on the shadow side of the light split.
    shade_color: vec4<f32>,
    outline_color: vec4<f32>,
    // x: shadow threshold on N·L in [-1, 1]; y: split softness; z: rim strength;
    // w: outline width in view-space units at 1 m.
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(0) @binding(1) var<storage, read> instances: array<InstanceData>;
@group(0) @binding(2) var<storage, read> draws: array<DrawData>;
@group(0) @binding(3) var<storage, read> materials: array<ToonMaterial>;

// Vertex buffer slot 1 (`pipelines::DRAW_ID_ATTRIBUTES`): the draw ID, stepped per instance.
// Entry points declare it as `@location(5) draw_id: u32`; WGSL has no constants in
// attributes, so the shader tests check that location against the Rust layout.

// Vertex buffer slot 0 (`pipelines::MESH_VERTEX_ATTRIBUTES`). Mirrors
// `sapphire_assets::mesh::MeshVertex`.
struct MeshVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Normal averaged over every vertex sharing this position; outlines extrude along it.
    @location(2) outline_normal: vec3<f32>,
    @location(3) uv: vec2<f32>,
    // rgb: albedo tint, sRGB-encoded; a: outline width scale.
    @location(4) color: vec4<f32>,
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

// Normalizes, mapping a zero vector to zero instead of NaN.
fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(v, v);
    return select(vec3<f32>(0.0), v * inverseSqrt(length_squared), length_squared > 1e-12);
}

// The sky gradient in direction `dir`. Also the toon shader's ambient light.
fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let y = dir.y;
    // A wide pale band above the horizon that deepens toward the zenith.
    let above = mix(frame.sky_horizon.rgb, frame.sky_zenith.rgb, sqrt(saturate(y)));
    // Below the horizon, a quick fade to the ground color.
    let below = mix(frame.sky_horizon.rgb, frame.sky_ground.rgb, saturate(-y * 5.0));
    return select(below, above, y >= 0.0);
}
