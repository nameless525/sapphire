// Declarations shared by every Sapphire M0 pass. `shaders.rs` prepends this file to each
// pass's source; WESL imports replace the concatenation in M1 (spec §5.12).
//
// Bind groups (the budget behind them is in the crate docs):
//   0  frame   per-frame constants: sun, sky, time, light counts, the day/night factor,
//              ambient light, the shadow tint and the weather; the local lights (binding 1) and
//              their lists (binding 2) are declared in lights.wgsl and the passes that read
//              them, the characters (binding 3) below
//   1  view    one camera and its target, bound with a dynamic offset so the main view,
//              shadow cascades and other views (M1) share one buffer
//   2  scene   persistent scene buffers: instances, and the parameter buffer of the material
//              family the pass draws (materials.wgsl): the anime character family's is
//              `materials` below, the environment family's `environment_materials`
//              (environment.wgsl)
//   3  material one material's textures and samplers, per family (anime_character.wgsl and
//              environment.wgsl declare their families')
//
// Conventions: right-handed world, Y up; reverse-Z depth (near = 1, infinity = 0); all
// colors linear HDR light, unclamped: passes write `Rgba16Float` scene color, which post.wgsl
// tone maps into the target.

struct FrameUniforms {
    // The world's key light (`FramePacket::sun`: the sun or the moon), for what the world shades
    // and casts by; characters take their light rigs' (`characters`). xyz: unit vector from the
    // surface toward it.
    sun_direction: vec4<f32>,
    // rgb: its color × intensity.
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
    // 0 by day, 1 at night, from time of day: the day/night ramp blend.
    day_night: f32,
    // Ambient light (time of day's `FramePacket::ambient`), color × intensity: rgb from the
    // sky above and from the ground below (see `ambient_light`).
    ambient_sky: vec4<f32>,
    ambient_ground: vec4<f32>,
    // rgb: the share of the world's key light kept inside its shadows (time of day's
    // `FramePacket::shadow_tint`), so the world's shadows are colored, never black. The
    // environment shader reads it; characters take their authored shade colors instead.
    shadow_tint: vec4<f32>,
    // The weather's wetness inputs (`FramePacket::weather`): x how wet surfaces are, y how far
    // puddles have filled, both 0 to 1. Plumbed for the wet surface model that arrives with
    // weather; no shader reads them yet (environment.wgsl's header says where they will apply).
    weather: vec4<f32>,
}

// Under temporal AA (M1 R7) every matrix into clip space carries the frame's sub-pixel jitter,
// so all passes rasterize the same jittered samples; velocity compares the unjittered ones.
struct ViewUniforms {
    view_from_world: mat4x4<f32>,
    // Jittered under temporal AA.
    clip_from_view: mat4x4<f32>,
    // Jittered under temporal AA.
    clip_from_world: mat4x4<f32>,
    // The inverse of the jittered `clip_from_world`.
    world_from_clip: mat4x4<f32>,
    // The unjittered `clip_from_world` in the last frame this view drew, for velocity.
    previous_clip_from_world: mat4x4<f32>,
    // xyz: world-space camera position.
    position: vec4<f32>,
    // xy: target size in pixels; zw: reciprocals.
    viewport: vec4<f32>,
    // `clip_from_world` without the jitter, for velocity.
    unjittered_clip_from_world: mat4x4<f32>,
    // xy: this frame's jitter in pixels (x right, y down: the image moves by it); zw: in NDC.
    // Zero without temporal AA.
    jitter: vec4<f32>,
}

struct InstanceData {
    world_from_object: mat4x4<f32>,
    // Inverse-transpose of the upper 3×3 of `world_from_object`.
    normal_from_object: mat3x3<f32>,
    // `world_from_object` in the last frame drawn, for velocity.
    previous_world_from_object: mat4x4<f32>,
    // The instance's entry in `characters`: 0, the world's key light, unless the frame lists
    // it as a character.
    character: u32,
}

// A character's light rig (M1 C2), resolved on the CPU for this frame
// (`sapphire_render::CharacterLighting`).
struct CharacterRig {
    // xyz: unit vector toward the character's key light; w: the rim's scale.
    key_direction: vec4<f32>,
    // rgb: the key light's color × intensity; w: the local lights' luminance ceiling.
    key_radiance: vec4<f32>,
    // xyz: the head's center, world space, which face and hair GI normals turn toward a
    // sphere around; w: the GI's intensity.
    head: vec4<f32>,
    // The stylized GI: x camera bias, y saturation ceiling, z value ceiling, w 1 where the head
    // is known (so face and hair regions spherize around it) and 0 where not.
    gi: vec4<f32>,
    // rgb: the GI's skin tint.
    skin_tint: vec4<f32>,
}

// A character's face (M1 C3) for the frame (`sapphire_render::CharacterFace`): its head's pose,
// which the face permutation takes the key light into (anime_character.wgsl), and its
// expression weights.
struct CharacterFace {
    // xyz: unit head-forward, world space.
    forward: vec4<f32>,
    // xyz: unit head-up, world space, perpendicular to forward.
    up: vec4<f32>,
    // x: blush; y: gloom (unclamped).
    expression: vec4<f32>,
}

// One character's data for the frame. Each package's per-character data is a struct of its
// own here: the light rig (C2) and the face (C3).
struct CharacterData {
    rig: CharacterRig,
    face: CharacterFace,
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var<uniform> view: ViewUniforms;
@group(2) @binding(0) var<storage, read> instances: array<InstanceData>;
// The anime character family's blocks, by the draw record's material slot.
@group(2) @binding(1) var<storage, read> materials: array<AnimeCharacterParams>;
// The frame's characters, by `InstanceData::character`: entry 0 is the world's key light with
// unstyled GI, for instances that aren't characters.
@group(0) @binding(3) var<storage, read> characters: array<CharacterData>;

// A mesh vertex as stored: the packed streams of `sapphire_assets::mesh::VertexStreams`, in
// vertex buffer slots 1–3 (`pipelines::{POSITION_*, SHADING, SURFACE}_ATTRIBUTES`), each bound
// at the mesh's first vertex because indices are mesh-relative. `decode_vertex` unpacks it.
struct PackedVertex {
    // Slot 1. Quantized meshes: xyz unorm16 across the mesh's bounds, w zero. f32 meshes and
    // skinned or morphed instances (their skinning outputs, `skinning.wgsl`): xyz as they are,
    // w = 1 from the vertex fetch.
    @location(0) position: vec4<f32>,
    // Slot 2: octahedral snorm16 × 2 each. The outline normal is averaged over every vertex
    // sharing a position; outlines extrude along it.
    @location(1) normal: vec2<f32>,
    @location(2) outline_normal: vec2<f32>,
    // Slot 3: unorm16 × 2 across the mesh's UV bounds, then the vertex color's character data
    // (linear: r ambient occlusion, g outline-width scale, b outline Z-offset scale) and, in a,
    // the vertex flags byte as unorm8 (`sapphire_assets::mesh::VertexFlags`).
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

// The vertex flags' material ID field (`VertexFlags::MATERIAL_ID_MASK`).
const VERTEX_MATERIAL_ID_MASK: u32 = 7u;

// A mesh vertex, decoded to object space.
struct MeshVertex {
    position: vec3<f32>,
    normal: vec3<f32>,
    outline_normal: vec3<f32>,
    uv: vec2<f32>,
    // r: ambient occlusion; g: outline-width scale; b: outline Z-offset scale.
    color: vec3<f32>,
    // `VertexFlags` bits.
    flags: u32,
}

fn decode_vertex(packed: PackedVertex, draw: DrawRecord) -> MeshVertex {
    var vertex: MeshVertex;
    vertex.position = object_position(packed.position, draw);
    vertex.normal = oct_decode(packed.normal);
    vertex.outline_normal = oct_decode(packed.outline_normal);
    vertex.uv = draw.uv_transform.xy + draw.uv_transform.zw * packed.uv;
    vertex.color = packed.color.rgb;
    // Unorm8 n arrives as n / 255, which rounds back exactly.
    vertex.flags = u32(round(packed.color.a * 255.0));
    return vertex;
}

// A slot 1 position in object space. Every pass decodes positions through this, so passes that
// mark their clip position invariant compute the same depth for the same vertex.
fn object_position(position: vec4<f32>, draw: DrawRecord) -> vec3<f32> {
    return draw.position_offset + draw.position_scale * position.xyz;
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

// Normalizes, mapping a zero vector to zero instead of NaN.
fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(v, v);
    return select(vec3<f32>(0.0), v * inverseSqrt(length_squared), length_squared > 1e-12);
}

fn safe_normalize2(v: vec2<f32>) -> vec2<f32> {
    let length_squared = dot(v, v);
    return select(vec2<f32>(0.0), v * inverseSqrt(length_squared), length_squared > 1e-12);
}

// The sky gradient in direction `dir`.
fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let y = dir.y;
    // A wide pale band above the horizon that deepens toward the zenith.
    let above = mix(frame.sky_horizon.rgb, frame.sky_zenith.rgb, sqrt(saturate(y)));
    // Below the horizon, a quick fade to the ground color.
    let below = mix(frame.sky_horizon.rgb, frame.sky_ground.rgb, saturate(-y * 5.0));
    return select(below, above, y >= 0.0);
}

// The frame's ambient light reaching a surface whose unit normal is `n`: the irradiance of a
// hemisphere of sky light above and ground light below, a blend by how far the surface faces up.
fn ambient_light(n: vec3<f32>) -> vec3<f32> {
    return mix(frame.ambient_ground.rgb, frame.ambient_sky.rgb, 0.5 + 0.5 * n.y);
}

// Rec. 709 luminance of linear RGB.
const LUMINANCE: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// The narrowest light split, in N·L: far below a pixel's change on any curved surface.
const MIN_SPLIT_WIDTH: f32 = 1e-5;

// A light split: 0 below `edge - width`, 1 above `edge + width`. The width never reaches 0,
// where smoothstep's edges would meet and its result is undefined (a flat surface with no
// softness, whose N·L doesn't change across a pixel).
fn light_step(edge: f32, width: f32, x: f32) -> f32 {
    let w = max(width, MIN_SPLIT_WIDTH);
    return smoothstep(edge - w, edge + w, x);
}

// N·L scaled by `occlusion` in half-Lambert space: unchanged at 1, pushed toward -1 (the
// deepest shadow) as it falls. Exactly N·L when nothing occludes. Cast shadows and ambient
// occlusion enter light splits through it, so a shadowed point moves into the shadow side's
// tones instead of darkening them.
fn occluded(n_dot_l: f32, occlusion: f32) -> f32 {
    return n_dot_l + (occlusion - 1.0) * (n_dot_l + 1.0);
}

// The normal map's tangent-space normal in world space, by a tangent frame built from the
// screen derivatives of the position and UV (Schüler, "Normal Mapping Without Precomputed
// Tangents", 2013), since meshes carry no tangents yet. `sampled` is the map's X and Y in [0, 1];
// maps are OpenGL-style, as glTF's are: +Y toward decreasing v.
fn mapped_normal(
    normal: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
    uv: MaterialUv,
    sampled: vec2<f32>,
) -> vec3<f32> {
    let dy_perp = cross(position_dy, normal);
    let dx_perp = cross(normal, position_dx);
    // These solve for the gradients of u and v along the surface, times the determinant of the
    // screen-space frame, whose sign flips with the screen's y axis (down in WebGPU) and the
    // facing; `orientation` takes it back out, so they point along increasing u and v.
    let orientation = select(1.0, -1.0, dot(position_dx, dy_perp) < 0.0);
    let tangent = (dy_perp * uv.dx.x + dx_perp * uv.dy.x) * orientation;
    let bitangent = (dy_perp * uv.dx.y + dx_perp * uv.dy.y) * orientation;
    let scale = inverseSqrt(max(max(dot(tangent, tangent), dot(bitangent, bitangent)), 1e-30));
    let xy = sampled * 2.0 - 1.0;
    let z = sqrt(saturate(1.0 - dot(xy, xy)));
    return safe_normalize((tangent * xy.x - bitangent * xy.y) * scale + normal * z);
}
