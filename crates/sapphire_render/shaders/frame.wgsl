// Declarations shared by every Sapphire M0 pass. `shaders.rs` prepends this file to each
// pass's source; WESL imports replace the concatenation in M1 (spec §5.12).
//
// Bind groups (the budget behind them is in the crate docs):
//   0  frame   per-frame constants: sun, sky, time and light counts; the local lights (binding
//              1) and their lists (binding 2) are declared in lights.wgsl and toon.wgsl
//   1  view    one camera and its target, bound with a dynamic offset so the main view,
//              shadow cascades and other views (M1) share one buffer
//   2  scene   persistent scene buffers: instances, materials
//   3  —       reserved for material textures (ramps, ILM masks, face SDF maps) in M1
//
// Conventions: right-handed world, Y up; reverse-Z depth (near = 1, infinity = 0); all
// colors linear HDR light, unclamped: passes write `Rgba16Float` scene color, which post.wgsl
// tone maps into the target.

struct FrameUniforms {
    // xyz: unit vector from the surface toward the sun.
    sun_direction: vec4<f32>,
    // rgb: sun color × intensity.
    sun_radiance: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_ground: vec4<f32>,
    // x: time in seconds. The app wraps it (to an hour in the sandbox) to keep f32 precision,
    // so use it only for effects that are periodic or tolerate the jump.
    time: vec4<f32>,
    // Local lights and their lists, and the cluster grid's depth slices (see lights.wgsl).
    lights: vec4<u32>,
    clusters: vec4<f32>,
}

struct ViewUniforms {
    view_from_world: mat4x4<f32>,
    clip_from_view: mat4x4<f32>,
    clip_from_world: mat4x4<f32>,
    world_from_clip: mat4x4<f32>,
    // xyz: world-space camera position.
    position: vec4<f32>,
    // xy: target size in pixels; zw: reciprocals.
    viewport: vec4<f32>,
}

struct InstanceData {
    world_from_object: mat4x4<f32>,
    // Inverse-transpose of the upper 3×3 of `world_from_object`.
    normal_from_object: mat3x3<f32>,
}

// Mirrors `sapphire_assets::mesh::ToonMaterialParams`.
struct ToonMaterial {
    base_color: vec4<f32>,
    // Multiplier applied to the albedo on the shadow side of the light split.
    shade_color: vec4<f32>,
    outline_color: vec4<f32>,
    // x: shadow threshold on N·L in [-1, 1]; y: split softness; z: rim strength;
    // w: outline width in pixels at a 1080-pixel-high target.
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var<uniform> view: ViewUniforms;
@group(2) @binding(0) var<storage, read> instances: array<InstanceData>;
@group(2) @binding(1) var<storage, read> materials: array<ToonMaterial>;

// A mesh vertex as stored: the packed streams of `sapphire_assets::mesh::VertexStreams`, in
// vertex buffer slots 1–3 (`pipelines::{POSITION_*, SHADING, SURFACE}_ATTRIBUTES`), each bound
// at the mesh's first vertex because indices are mesh-relative. `decode_vertex` unpacks it.
struct PackedVertex {
    // Slot 1. Quantized meshes: xyz unorm16 across the mesh's bounds, w zero. f32 meshes
    // (and, with R2, skinned output): xyz as they are, w = 1 from the vertex fetch.
    @location(0) position: vec4<f32>,
    // Slot 2: octahedral snorm16 × 2 each. The outline normal is averaged over every vertex
    // sharing a position; outlines extrude along it.
    @location(1) normal: vec2<f32>,
    @location(2) outline_normal: vec2<f32>,
    // Slot 3: unorm16 × 2 across the mesh's UV bounds, then rgb: albedo tint (sRGB-encoded),
    // a: outline width scale.
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
}

// Vertex buffer slot 0 (`pipelines::DRAW_ATTRIBUTES`), stepped per instance: the draw record.
// CPU-driven draws bind it at their own record and draw instance 0; GPU-driven multi-draw
// indirect (later) binds it at 0 and selects the record with `first_instance`. Either way the
// shaders never read `instance_index`.
struct DrawRecord {
    @location(5) instance: u32,
    @location(6) material: u32,
    // The mesh's dequantization: object position = offset + scale × stored position, and
    // uv = uv_transform.xy + uv_transform.zw × stored uv.
    @location(7) position_offset: vec3<f32>,
    @location(8) position_scale: vec3<f32>,
    @location(9) uv_transform: vec4<f32>,
}

// A mesh vertex, decoded to object space.
struct MeshVertex {
    position: vec3<f32>,
    normal: vec3<f32>,
    outline_normal: vec3<f32>,
    uv: vec2<f32>,
    color: vec4<f32>,
}

fn decode_vertex(packed: PackedVertex, draw: DrawRecord) -> MeshVertex {
    var vertex: MeshVertex;
    vertex.position = draw.position_offset + draw.position_scale * packed.position.xyz;
    vertex.normal = oct_decode(packed.normal);
    vertex.outline_normal = oct_decode(packed.outline_normal);
    vertex.uv = draw.uv_transform.xy + draw.uv_transform.zw * packed.uv;
    vertex.color = packed.color;
    return vertex;
}

// The unit vector an octahedral encoding holds (Cigolle et al., JCGT 2014); mirrors
// `sapphire_assets::mesh::oct_decode`.
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let z = 1.0 - abs(e.x) - abs(e.y);
    // Unfolds the lower hemisphere.
    let t = max(-z, 0.0);
    let xy = e + select(vec2<f32>(t), vec2<f32>(-t), e >= vec2<f32>(0.0));
    return normalize(vec3<f32>(xy, z));
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

fn safe_normalize2(v: vec2<f32>) -> vec2<f32> {
    let length_squared = dot(v, v);
    return select(vec2<f32>(0.0), v * inverseSqrt(length_squared), length_squared > 1e-12);
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
