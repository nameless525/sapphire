// Declarations shared by every Sapphire M0 pass. `shaders.rs` prepends this file to each
// pass's source; WESL imports replace the concatenation in M1 (spec §5.12).
//
// Bind groups (the budget behind them is in the crate docs):
//   0  frame   per-frame constants: sun, sky, time, light counts, the day/night factor,
//              ambient light, the shadow tint, the weather and the fog; the local lights (binding 1) and
//              their lists (binding 2) are declared in lights.wgsl and the passes that read
//              them, the characters (binding 3), the shadows (bindings 4–7) and the
//              character prepass's targets (bindings 8–10) below, the ambient probe grid,
//              reflection cube and occlusion (bindings 11–16) in ambient.wgsl
//   1  view    one camera and its target, bound with a dynamic offset so the main view,
//              shadow cascades and other views (M1) share one buffer
//   2  scene   persistent scene buffers: instances, and the parameter buffer of the material
//              family the pass draws (materials.wgsl): the anime character family's is
//              `materials` below, the environment family's `environment_materials`
//              (environment.wgsl), the foliage family's `foliage_materials` with the frame's
//              wind at binding 2 (foliage.wgsl)
//   3  material one material's textures and samplers, per family (anime_character.wgsl,
//              environment.wgsl and foliage.wgsl declare their families')
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
    // The scale on materials' outline LOD cutoffs (M1 C4, outline.wgsl).
    outline_distance: f32,
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
    // The fog over the world and the sky (M1 W3, `FramePacket::fog`; see "Fog" below).
    fog: FogUniforms,
}

// The frame's fog (`sapphire_render::fog::GpuFog`).
struct FogUniforms {
    // rgb: the near layer's color; a: its extinction per meter.
    near: vec4<f32>,
    // rgb: the far layer's tint on the sky's horizon; a: its extinction per meter.
    far: vec4<f32>,
    // rgb: the height layer's color; a: its extinction per meter at its base.
    height: vec4<f32>,
    // x: where the near layer begins, meters from the camera; y: where the far one does; z: the
    // height layer's falloff per meter; w: its base height.
    shape: vec4<f32>,
    // rgb: the light scattered toward the camera around the key light; a: characters' share of
    // the fog.
    light: vec4<f32>,
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
    // x: the proximity fade's turn this frame (M1 C7; `temporal::fade_turn`): a sixteenth under
    // temporal AA, 0 without.
    dither: vec4<f32>,
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
    // How far its character has faded (M1 C7; `CharacterFade`): 0 drawn whole. The prepasses'
    // faded pipelines read it in the vertex stage, which their fragments dither by.
    fade: f32,
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

// A character's shadow (M1 R4): where the environment's shadow over its face is sampled, and
// what was found there.
struct CharacterShadow {
    // xyz: the head's center, world space; w: the radius of the sphere around it the face pass
    // samples (0: none, so the face is lit).
    probe: vec4<f32>,
    // x: the world key light's visibility over that sphere past environment casters, which the
    // face pass (shadows.wgsl) writes every frame shadows are on and the face permutation takes
    // as occlusion; 1 otherwise. yzw: 0.
    visibility: vec4<f32>,
}

// A character's ambient light (M1 W5): the hemisphere its stylized GI reads
// (`character_ambient_light`). Without a probe grid it is the frame's own (`ambient_light`):
// time of day's sky and ground along +Y. Where a grid lights the character, the ambient pass
// (probes.wgsl's `cs_character_ambient`) writes the grid's light around its body, kept low
// frequency: a hemisphere along the axis its light comes from.
struct CharacterAmbient {
    // rgb: the light from the hemisphere's pole, color × intensity, times `axis.w`.
    sky: vec4<f32>,
    // rgb: the light from the opposite pole, likewise.
    ground: vec4<f32>,
    // xyz: the unit axis from ground to sky (+Y for the frame's hemisphere); w: what capsule AO
    // from the other characters' bodies leaves of the ambient light (1 without them).
    axis: vec4<f32>,
    // xyz: the body's center, world space, where the ambient pass reads the grid; w: 1 with a
    // body, 0 without.
    probe: vec4<f32>,
}

// A character's proximity fade (M1 C7, `sapphire_render::CharacterInstance::opacity`).
struct CharacterFade {
    // x: how far it has faded, 1 − its opacity: 0 drawn whole (entry 0's, which instances that
    // aren't characters read), 1 gone. yzw: 0.
    amount: vec4<f32>,
}

// One character's data for the frame. Each package's per-character data is a struct of its
// own here: the light rig (C2), the face (C3), the shadow (R4), the ambient light (W5) and the
// fade (C7).
struct CharacterData {
    rig: CharacterRig,
    face: CharacterFace,
    shadow: CharacterShadow,
    ambient: CharacterAmbient,
    fade: CharacterFade,
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var<uniform> view: ViewUniforms;
@group(2) @binding(0) var<storage, read> instances: array<InstanceData>;
// The anime character family's blocks, by the draw record's material slot.
@group(2) @binding(1) var<storage, read> materials: array<AnimeCharacterParams>;
// The frame's characters, by `InstanceData::character`: entry 0 is the world's key light with
// unstyled GI, for instances that aren't characters.
@group(0) @binding(3) var<storage, read> characters: array<CharacterData>;

// The character prepass (M1 C5, prepass.wgsl; the `character_prepass` module): the listed
// characters' own depth (reverse-Z, 0 where none is) and IDs (`character_id`), single-sampled at
// the scene's size, which the anime character shader reads for its rim, the bangs' shadow on
// faces, eyes through the bangs and the eyes' layers. The IDs' R is the nearest surface's, G the
// nearest surface's but for hair (M1 C6): where G is a character's eye, its eye would show but
// for its bangs, the eye depth mask. The pass that writes them binds no group 0.
@group(0) @binding(8) var character_depth: texture_depth_2d;
@group(0) @binding(9) var character_ids: texture_2d<u32>;
// Nearest, clamped to the edge: gathers of both.
@group(0) @binding(10) var character_sampler: sampler;

// A character ID's layout (`character_prepass::ID_ENTRY_BITS`): the character's entry in the low
// bits, 0 for none, and its part (`CHARACTER_SURFACE_*`) above them.
const CHARACTER_ID_ENTRY_BITS: u32 = 14u;
const CHARACTER_ID_ENTRY_MASK: u32 = 16383u;

// The character prepass's ID for part `part` of the character in entry `entry`
// (`character_prepass::encode_id`).
fn character_id(entry: u32, part: u32) -> u32 {
    return (entry & CHARACTER_ID_ENTRY_MASK) | (part << CHARACTER_ID_ENTRY_BITS);
}

// The reverse-Z depth of a point `distance` meters in front of the main view's camera, through
// its projection (any jitter moves x and y alone).
fn depth_at_distance(distance: f32) -> f32 {
    let p = view.clip_from_view;
    return (p[2][2] * -distance + p[3][2]) / (p[2][3] * -distance + p[3][3]);
}

// The distance in front of the camera of a point at reverse-Z depth `depth`: the inverse of
// `depth_at_distance`, and very large at 0, the cleared depth.
fn distance_at_depth(depth: f32) -> f32 {
    let p = view.clip_from_view;
    let denominator = depth * p[2][3] - p[2][2];
    return (depth * p[3][3] - p[3][2]) / select(-1e-30, denominator, abs(denominator) > 1e-30);
}

// How far in front of the main view's camera `position` (world space) is.
fn view_distance(position: vec3<f32>) -> f32 {
    return -(view.view_from_world * vec4<f32>(position, 1.0)).z;
}

// ---------------------------------------------------------------------------------------------
// The proximity fade (M1 C7; the `characters` module has the design): a fading character covers
// a share of its pixels, 1 − its fade, picked by a screen-space dither that every pass drawing it
// shares, so its surfaces, outlines, eyes through hair and its depth, IDs and velocity in the
// prepasses all keep the same pixels. The pattern is a 4 × 4 ordered dither (`fade_order`), its
// thresholds turned per character (so two fading characters keep different pixels) and per frame
// under temporal AA by sixteenths over a 16-frame cycle (`view.dither`), so every pixel takes
// each of the sixteen thresholds once a cycle and the history averages the pattern into an even
// blend; without temporal AA it holds still.
// Under MSAA a pixel covers a run of its samples instead: as many as its share, the dither
// rounding between levels.

// The thresholds' turn per character entry: the golden ratio's fraction.
const FADE_ENTRY_STEP: f32 = 0.618034;

// The fade's dither order: 0 to 15, each once in every 4 × 4 block, a row per component, 4 bits
// a cell with x = 0 in the lowest. Any three consecutive values (cyclically, as the turns take
// them) lie in three different rows and columns, so every 3 × 3 block of pixels holds one of
// them, which a Bayer matrix's don't: wherever at least 3 of the 16 thresholds keep the character
// and 3 show what lies behind it, temporal AA's 3 × 3 neighborhood sees both, and its clip keeps
// the history's blend rather than pulling it to this frame's color. Of such orders, it is the
// most even a search found (a Gaussian energy over every run of consecutive values, on the
// tiled plane: within 2% of Bayer's).
const FADE_ORDER = vec4<u32>(0xE4A1u, 0xB8F5u, 0x307Du, 0x6C29u);

// The fade's dither order at `pixel` (`FADE_ORDER`).
fn fade_order(pixel: vec2<u32>) -> u32 {
    let row = FADE_ORDER[pixel.y & 3u];
    return (row >> ((pixel.x & 3u) * 4u)) & 15u;
}

// The fade's threshold at framebuffer position `pixel` for character entry `character`, in
// [0, 1): sixteen evenly spaced values in every 4 × 4 block, turned by the entry and this frame's
// turn (zero without temporal AA).
fn fade_threshold(pixel: vec2<f32>, character: u32) -> f32 {
    let order = f32(fade_order(vec2<u32>(floor(pixel))));
    let turn = view.dither.x + fract(f32(character) * FADE_ENTRY_STEP);
    return fract((order + 0.5) / 16.0 + turn);
}

// How many of a pixel's `samples` a character faded by `fade` covers there, at `threshold`: its
// share of them, rounded by the threshold, so all of them at no fade and none fully faded. With
// one sample, the pixel is covered where the threshold is at least the fade.
fn fade_coverage(fade: f32, threshold: f32, samples: u32) -> u32 {
    let count = f32(samples);
    return u32(clamp(floor((1.0 - fade) * count + threshold), 0.0, count));
}

// The sample mask of `covered` of `samples` (at most 8): a run of them, wrapping, starting where
// the threshold says, so neighboring pixels cover different samples.
fn fade_sample_mask(covered: u32, threshold: f32, samples: u32) -> u32 {
    let all = (1u << samples) - 1u;
    let run = (1u << covered) - 1u;
    let start = min(u32(fract(threshold * 4.0) * f32(samples)), samples - 1u);
    return ((run << start) | (run >> (samples - start))) & all;
}

// Whether the proximity fade's dither leaves the pixel at `pixel` out of character `character`
// faded by `fade`: none of the main pass's `samples` there covered. Every pass that cuts a fading
// character out asks this with the main pass's sample count (each program's `FADE_SAMPLES`), so
// a single-sampled prepass keeps the pixels where the main pass covers any sample and the passes
// agree on the character's pixels.
fn faded_out(fade: f32, pixel: vec2<f32>, character: u32, samples: u32) -> bool {
    return fade_coverage(fade, fade_threshold(pixel, character), samples) == 0u;
}

// What the scene prepass's vertex stages pass on (prepass.wgsl, and the outline hull's in
// outline.wgsl).
struct ScenePrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    // Unjittered clip positions now and when last drawn, divided per pixel so velocity is
    // perspective-correct.
    @location(1) current: vec4<f32>,
    @location(2) previous: vec4<f32>,
}

// The scene prepass's color targets.
// The scene prepass's outline mask (`pipelines::outline_mask`): 1 where an outline hull is the
// nearest surface (M1 C4). A fading character's surfaces write 0.25 and its hulls 0.75 (M1 C7),
// which temporal AA's responsive mask reads to let the proximity fade's dither accumulate into a
// blend; GTAO leaves every hull out (above 0.5) and the character's surfaces through the
// character prepass.
const OUTLINE_MASK_LINE: f32 = 1.0;
const OUTLINE_MASK_FADING: f32 = 0.25;
const OUTLINE_MASK_FADING_LINE: f32 = 0.75;

struct ScenePrepassTargets {
    // World-space unit normal, octahedral (`oct_encode`).
    @location(0) normal: vec2<f32>,
    // Motion since the last drawn frame in texture coordinates (x right, y down): the pixel was
    // at `uv - velocity`.
    @location(1) velocity: vec2<f32>,
}

// The scene prepass's targets for a pixel of a surface or hull.
fn scene_prepass_targets(in: ScenePrepassVaryings) -> ScenePrepassTargets {
    let current = in.current.xy / in.current.w;
    let previous = in.previous.xy / in.previous.w;
    var out: ScenePrepassTargets;
    out.normal = oct_encode(safe_normalize(in.world_normal));
    // NDC is y up, texture coordinates y down, and both span 2 NDC units per texture.
    out.velocity = (current - previous) * vec2<f32>(0.5, -0.5);
    return out;
}

// A mesh vertex as stored: the packed streams of `sapphire_assets::mesh::VertexStreams`, in
// vertex buffer slots 1–3 (`pipelines::{POSITION_*, SHADING, SURFACE}_ATTRIBUTES`), each bound
// at the mesh's first vertex because indices are mesh-relative. `decode_vertex` unpacks it.
struct PackedVertex {
    // Slot 1. Quantized meshes: xyz unorm16 across the mesh's bounds, w zero. f32 meshes and
    // skinned or morphed instances (their skinning outputs, `skinning.wgsl`): xyz as they are,
    // w = 1 from the vertex fetch.
    @location(0) position: vec4<f32>,
    // Slot 2: octahedral snorm16 × 2 each. The normal is in object space. The outline normal,
    // averaged over every vertex sharing a position (outlines extrude along it), is in the
    // normal's tangent frame (`outline_from_tangent_space`, M1 C4).
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
    vertex.outline_normal = outline_from_tangent_space(vertex.normal, oct_decode(packed.outline_normal));
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

// The octahedral encoding of a unit vector (Cigolle et al., JCGT 2014), the inverse of
// `oct_decode`; a zero vector encodes +Z.
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let l1 = abs(n.x) + abs(n.y) + abs(n.z);
    var e = select(vec2<f32>(0.0), n.xy / l1, l1 > 0.0);
    if n.z < 0.0 {
        e = (1.0 - abs(e.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), e >= vec2<f32>(0.0));
    }
    return e;
}

// The tangent frame outline normals are stored in (M1 C4; `sapphire_assets::mesh::outline_frame`,
// whose constants these mirror): the normal, and a tangent derived from it alone by crossing
// with a fixed axis no modeled surface is likely to face, or a fallback axis within about 0.6°
// of it. Meshes carry no tangents; deformations carry outline normals between frames themselves
// (skinning.wgsl).
const OUTLINE_FRAME_AXIS: vec3<f32> = vec3<f32>(0.4, 0.55, 0.73);
const OUTLINE_FRAME_FALLBACK: vec3<f32> = vec3<f32>(0.83, -0.31, 0.46);
const OUTLINE_FRAME_MIN_CROSS: f32 = 1e-4;

// The outline frame's unit tangent for the unit normal `n`; the bitangent is `cross(n, tangent)`.
fn outline_frame_tangent(n: vec3<f32>) -> vec3<f32> {
    let tangent = cross(OUTLINE_FRAME_AXIS, n);
    let fallback = cross(OUTLINE_FRAME_FALLBACK, n);
    return normalize(select(fallback, tangent, dot(tangent, tangent) > OUTLINE_FRAME_MIN_CROSS));
}

// The unit outline normal stored as `tangent_space` in the outline frame of the unit normal `n`,
// in `n`'s space.
fn outline_from_tangent_space(n: vec3<f32>, tangent_space: vec3<f32>) -> vec3<f32> {
    let tangent = outline_frame_tangent(n);
    let bitangent = cross(n, tangent);
    return normalize(tangent * tangent_space.x + bitangent * tangent_space.y + n * tangent_space.z);
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

// A character's ambient light (`CharacterAmbient`) reaching a surface whose unit normal is `n`:
// exactly `ambient_light(n)` for the frame's own hemisphere.
fn character_ambient_light(ambient: CharacterAmbient, n: vec3<f32>) -> vec3<f32> {
    return mix(ambient.ground.rgb, ambient.sky.rgb, 0.5 + 0.5 * dot(n, ambient.axis.xyz));
}

// ---------------------------------------------------------------------------------------------
// Fog (M1 W3, `sapphire_render::fog`, whose docs have the design): two distance layers and an
// exponential height layer, mixed by their optical depths along the view ray, the far one
// colored by the sky's horizon, with the key light scattered forward through them. Every world
// pass applies it at its final color (`fogged`), the sky as at infinite distance (`fogged_sky`),
// so distant geometry melts into the sky without a seam. `Fog::ray` is the same function on the
// CPU; keep the two in step.

// The distance layers fill the air up to this many meters above the camera (`fog::CEILING`).
const FOG_CEILING: f32 = 30.0;
// The sky's distance (`fog::SKY_DISTANCE`).
const FOG_SKY_DISTANCE: f32 = 1.0e6;
// The largest exponent the height layer takes (`fog::MAX_EXPONENT`), so its depth stays finite.
const FOG_MAX_EXPONENT: f32 = 30.0;
// The sky's horizon in the layers' mix, as an optical depth, where they are thin.
const FOG_HORIZON_WEIGHT: f32 = 1.0e-3;
// Below this, the height layer's (1 − e^(−x)) / x is its series.
const FOG_SERIES_BELOW: f32 = 1.0e-3;
// The optical depth, at the densities around the camera, of the stretch of a ray the layers'
// colors mix over (`fog::MIX_DEPTH`), and the least density it is measured by.
const FOG_MIX_DEPTH: f32 = 4.0;
const FOG_MIN_DENSITY: f32 = 1.0e-12;

// The fog over a view ray.
struct FogRay {
    // The share of the light behind that reaches the camera.
    transmittance: f32,
    // The fog's light: its layers' colors mixed by their optical depths (the sky's horizon where
    // they are thin).
    light: vec3<f32>,
    // The key light scattered toward the camera along the ray.
    sun: vec3<f32>,
}

// Whether the frame has fog: some layer has a density. A uniform branch on it keeps frames
// without fog exactly as they were.
fn fog_on() -> bool {
    return frame.fog.near.a + frame.fog.far.a + frame.fog.height.a > 0.0;
}

// The share of the fog an instance's surfaces take: characters' (`characters` entries other than
// 0, as post's character mask has it), or the world's whole.
fn fog_share(character: u32) -> f32 {
    return select(1.0, frame.fog.light.a, character != 0u);
}

// The layers' optical depths (near, far, height) over the first `length` meters of a ray rising
// `rise` per meter, `start` the height layer's density at the eye over its density at its base.
fn fog_depths(rise: f32, length: f32, start: f32) -> vec3<f32> {
    // The distance layers end where the ray has risen FOG_CEILING above the eye.
    let reach = select(length, min(length, FOG_CEILING / rise), rise > 0.0);
    let near = frame.fog.near.a * max(reach - frame.fog.shape.x, 0.0);
    let far = frame.fog.far.a * max(reach - frame.fog.shape.y, 0.0);
    // The height layer: density × ∫ exp(−k (h0 + rise·t)) dt over the ray, which is
    // density × exp(−k h0) × length × (1 − exp(−x)) / x with x = k · rise · length.
    let x = frame.fog.shape.z * rise * length;
    let ratio = select((1.0 - exp(min(-x, FOG_MAX_EXPONENT))) / x, 1.0 - 0.5 * x, abs(x) < FOG_SERIES_BELOW);
    return vec3<f32>(near, far, frame.fog.height.a * start * length * ratio);
}

// The fog over the ray from `eye` along the unit vector `dir` to what lies `distance` meters
// along it, `share` of it.
fn fog_ray(eye: vec3<f32>, dir: vec3<f32>, distance: f32, share: f32) -> FogRay {
    let rise = dir.y;
    let start = exp(min(-frame.fog.shape.z * (eye.y - frame.fog.shape.w), FOG_MAX_EXPONENT));
    let depths = fog_depths(rise, distance, start);
    let total = depths.x + depths.y + depths.z;
    // The colors mix over the stretch of the ray the camera sees: FOG_MIX_DEPTH at the densities
    // around it, so the sky and distant geometry agree along the horizon.
    let around = frame.fog.near.a + frame.fog.far.a + frame.fog.height.a * start;
    let stretch = FOG_MIX_DEPTH / max(around, FOG_MIN_DENSITY);
    // Shorter rays (all of them in light fog) mix over their whole length: no second walk.
    var mixed = depths;
    if distance > stretch {
        mixed = fog_depths(rise, stretch, start);
    }
    let horizon = frame.sky_horizon.rgb;
    var ray: FogRay;
    ray.transmittance = exp(-share * total);
    ray.light = (frame.fog.near.rgb * mixed.x + frame.fog.far.rgb * horizon * mixed.y
        + frame.fog.height.rgb * mixed.z + horizon * FOG_HORIZON_WEIGHT)
        / (mixed.x + mixed.y + mixed.z + FOG_HORIZON_WEIGHT);
    // A forward lobe toward the key light, cos⁸.
    let c = saturate(dot(dir, frame.sun_direction.xyz));
    let c2 = c * c;
    let c4 = c2 * c2;
    ray.sun = frame.fog.light.rgb * (c4 * c4);
    return ray;
}

// `color` seen through `ray`'s fog.
fn apply_fog(color: vec3<f32>, ray: FogRay) -> vec3<f32> {
    return color * ray.transmittance + (ray.light + ray.sun) * (1.0 - ray.transmittance);
}

// A world surface's final color at `position` with the frame's fog over it, `share` of it (1 for
// the world, `fog_share` for an instance's). A uniform branch: without fog, `color` exactly.
// One exit (WARP's compiler, see the outline shader).
fn fogged(color: vec3<f32>, position: vec3<f32>, share: f32) -> vec3<f32> {
    var result = color;
    if fog_on() {
        let offset = position - view.position.xyz;
        let distance = length(offset);
        let dir = offset / max(distance, 1e-6);
        result = apply_fog(color, fog_ray(view.position.xyz, dir, distance, share));
    }
    return result;
}

// The sky's color in the unit direction `dir` with the frame's fog over it, as at infinite
// distance.
fn fogged_sky(color: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    var result = color;
    if fog_on() {
        result = apply_fog(color, fog_ray(view.position.xyz, dir, FOG_SKY_DISTANCE, 1.0));
    }
    return result;
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

// The world's light model: the environment shader's (M1 W4), which foliage shares (M1 W6).

// The lit side's falloff toward a light at `n_dot_l`: Lambert's wrapped by `wrap`, 0 at
// N·L = -wrap and 1 facing the light.
fn wrapped(n_dot_l: f32, wrap: f32) -> f32 {
    return saturate((n_dot_l + wrap) / (1.0 + wrap));
}

// How far distance stylization has gone at `distance` meters: 0 up to the start (`x`), easing to
// 1 at the end (`y`).
fn distance_amount(distance: f32, stylization: vec4<f32>) -> f32 {
    let t = saturate((distance - stylization.x) / max(stylization.y - stylization.x, 1e-3));
    return t * t * (3.0 - 2.0 * t);
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

// Whether `passes` holds for any pixel of this pixel's 2 × 2 quad: the same answer in all four,
// from the pass/fail value's fine derivatives, which give its neighbors' in the quad. Taken in
// uniform control flow; a branch on it is uniform per quad, so derivatives inside it stay well
// defined (the sky skips its cloud layers that way). A fragment stage that cuts pixels out discards
// the quads it cuts whole early on it and every other pixel it cuts at its one exit
// (docs/conventions.md): Apple's Metal compiler has miscompiled a per-pixel `discard` taken
// earlier.
fn quad_any(passes: bool) -> bool {
    let value = select(0.0, 1.0, passes);
    let across = dpdxFine(value);
    let down = dpdyFine(value);
    return value + abs(across) + abs(down) + abs(dpdyFine(across)) > 0.0;
}

// ---------------------------------------------------------------------------------------------
// Shadows (M1 R4, `sapphire_render::shadows`): the world key light's cascades, which hold
// environment casters alone, and each character's own shadow, a tile of the character atlas
// (desktop) or its capsule. Receivers call one of two functions:
// - `shadow_visibility()`: environment surfaces, which take the cascades and every character's
//   own shadow;
// - `cascade_visibility()`: character surfaces, which take the cascades alone, so a character
//   never shadows itself.
// Both return the key light's visibility, 1 lit and 0 shadowed, a smooth field (a tent filter)
// for the receiver's light split or terminator to threshold into a clean edge. Shadows are
// colored, never black: the receivers tint them (environment.wgsl keeps `frame.shadow_tint`
// there; characters go into their authored shade tones).
// Everything is a branch on `SHADOWS`, a pipeline constant, so pipelines without shadows compile
// to what they were before.

struct ShadowCascade {
    // xy: the square's center in light space; z: texture coordinates per meter; w: one texel in
    // meters.
    area: vec4<f32>,
    // xy: reverse-Z depth of light-space z, z × x + y; z: texels across; w: 0.
    depth: vec4<f32>,
}

struct CharacterShadowMap {
    // As ShadowCascade's.
    area: vec4<f32>,
    // xy: as ShadowCascade's; z: the light-space z where its reach ends; w: 1 / the length it
    // fades out over before that.
    depth: vec4<f32>,
    // xy: the tile's corner in the atlas's texture coordinates; zw: its size in them.
    tile: vec4<f32>,
}

struct ShadowCapsule {
    // xyz: the body's base; w: its radius.
    base: vec4<f32>,
    // xyz: its top.
    top: vec4<f32>,
}

const MAX_SHADOW_CASCADES: u32 = 4u;
const MAX_CHARACTER_SHADOW_MAPS: u32 = 8u;
const MAX_SHADOW_CAPSULES: u32 = 16u;

struct ShadowUniforms {
    // Light space's axes in the world: x, y, and z toward the key light as shadows are cast.
    light_x: vec4<f32>,
    light_y: vec4<f32>,
    light_z: vec4<f32>,
    // x: cascades; y: character maps; z: capsules; w: character entries (the face pass's).
    counts: vec4<u32>,
    // xyz: the main camera's position; w: how far the cascades reach from it.
    camera: vec4<f32>,
    // xy: one texel of the character atlas in its texture coordinates; z: a tile's texels across.
    atlas: vec4<f32>,
    cascades: array<ShadowCascade, MAX_SHADOW_CASCADES>,
    characters: array<CharacterShadowMap, MAX_CHARACTER_SHADOW_MAPS>,
    capsules: array<ShadowCapsule, MAX_SHADOW_CAPSULES>,
}

@group(0) @binding(4) var<uniform> shadows: ShadowUniforms;
// The cascades, layers of one reverse-Z depth array (1 toward the light, 0 cleared).
@group(0) @binding(5) var shadow_cascades: texture_depth_2d_array;
// The characters' maps, tiles of one atlas.
@group(0) @binding(6) var character_shadows: texture_depth_2d;
// Linear, comparing with greater-equal: a receiver at or nearer the light than the nearest
// caster is lit.
@group(0) @binding(7) var shadow_sampler: sampler_comparison;

// Shadows are on (`shadows::override_constant`).
override SHADOWS: bool = false;
// The tent filter's width in texels: 5 on desktop, 3 on phones (`ShadowFilter`).
override SHADOW_TENT: u32 = 5u;

// Receivers' bias (`shadows` module docs): the point moves off the surface along its normal by
// SHADOW_NORMAL_OFFSET texels, plus half the tent's width × sin θ where the light grazes it, then
// toward the light by SHADOW_DEPTH_BIAS texels.
const SHADOW_NORMAL_OFFSET: f32 = 0.5;
const SHADOW_DEPTH_BIAS: f32 = 0.25;
// Texels a lookup's center keeps from its map's edge, so the filter stays inside: half the tent
// and the bilinear footprint, with room to spare.
const SHADOW_EDGE_TEXELS: f32 = 3.5;
// The outer share of a cascade, by its half-width, that blends into the next.
const SHADOW_CASCADE_BLEND: f32 = 0.1;
// Shadows fade out from this share of their distance to all of it.
const SHADOW_FADE_START: f32 = 0.9;
// A capsule's shadow edge: soft by this share of its radius at contact, widening by this much
// per meter away from it (a 1.7° light), and gone this far from it.
const CAPSULE_SOFTNESS: f32 = 0.35;
const CAPSULE_SPREAD: f32 = 0.03;
const CAPSULE_REACH: f32 = 8.0;

// `p` in light space.
fn light_space(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(p, shadows.light_x.xyz), dot(p, shadows.light_y.xyz), dot(p, shadows.light_z.xyz));
}

// The tent filter's taps (Castaño 2013's optimized PCF) around texel coordinate `texel` (texel
// centers at .5): up to three per axis, each a bilinear compare tap at `base + offset`, weighted.
// Lookups divide by the weights' sum as they add them, so a point every tap lights is exactly 1
// and one every tap shadows exactly 0.
struct ShadowTent {
    base: vec2<f32>,
    u: vec3<f32>,
    v: vec3<f32>,
    weight_u: vec3<f32>,
    weight_v: vec3<f32>,
    taps: u32,
}

fn shadow_tent(texel: vec2<f32>) -> ShadowTent {
    let center = floor(texel + 0.5);
    let s = texel + 0.5 - center;
    var tent: ShadowTent;
    tent.base = center - 0.5;
    if SHADOW_TENT == 3u {
        tent.taps = 2u;
        tent.weight_u = vec3<f32>(3.0 - 2.0 * s.x, 1.0 + 2.0 * s.x, 0.0);
        tent.weight_v = vec3<f32>(3.0 - 2.0 * s.y, 1.0 + 2.0 * s.y, 0.0);
        tent.u = vec3<f32>((2.0 - s.x) / tent.weight_u.x - 1.0, s.x / tent.weight_u.y + 1.0, 0.0);
        tent.v = vec3<f32>((2.0 - s.y) / tent.weight_v.x - 1.0, s.y / tent.weight_v.y + 1.0, 0.0);
    } else {
        tent.taps = 3u;
        tent.weight_u = vec3<f32>(4.0 - 3.0 * s.x, 7.0, 1.0 + 3.0 * s.x);
        tent.weight_v = vec3<f32>(4.0 - 3.0 * s.y, 7.0, 1.0 + 3.0 * s.y);
        tent.u = vec3<f32>((3.0 - 2.0 * s.x) / tent.weight_u.x - 2.0, (3.0 + s.x) / 7.0, s.x / tent.weight_u.z + 2.0);
        tent.v = vec3<f32>((3.0 - 2.0 * s.y) / tent.weight_v.x - 2.0, (3.0 + s.y) / 7.0, s.y / tent.weight_v.z + 2.0);
    }
    return tent;
}

// How far a lookup moves off its surface, in texels: SHADOW_NORMAL_OFFSET plus half the tent where
// the light grazes the surface. `slope` is sin θ of the light's angle to the normal.
fn shadow_offset_texels(slope: f32) -> f32 {
    return SHADOW_NORMAL_OFFSET + 0.5 * f32(SHADOW_TENT) * slope;
}

// Cascade `index`'s filtered visibility at light-space point `p` on a surface with light-space
// normal `n` (`slope`: sin θ to the light), and where the lookup lies: x the visibility, y its
// distance from the square's center as a share of the reach the filter allows (above 1: outside).
fn cascade_lookup(index: u32, p: vec3<f32>, n: vec3<f32>, slope: f32) -> vec2<f32> {
    let cascade = shadows.cascades[index];
    let texel = cascade.area.w;
    let at = p + n * (texel * shadow_offset_texels(slope));
    // Texture v runs down; light space's y up.
    let uv = vec2<f32>(0.5, 0.5) + (at.xy - cascade.area.xy) * cascade.area.z * vec2<f32>(1.0, -1.0);
    let size = cascade.depth.z;
    let reach = 0.5 - SHADOW_EDGE_TEXELS / size;
    let edge = max(abs(uv.x - 0.5), abs(uv.y - 0.5)) / reach;
    if edge > 1.0 {
        return vec2<f32>(1.0, edge);
    }
    let depth = saturate((at.z + texel * SHADOW_DEPTH_BIAS) * cascade.depth.x + cascade.depth.y);
    let tent = shadow_tent(uv * size);
    var sum = 0.0;
    var weights = 0.0;
    for (var j = 0u; j < tent.taps; j++) {
        for (var i = 0u; i < tent.taps; i++) {
            let coords = (tent.base + vec2<f32>(tent.u[i], tent.v[j])) / size;
            let lit = textureSampleCompareLevel(shadow_cascades, shadow_sampler, coords, index, depth);
            let weight = tent.weight_u[i] * tent.weight_v[j];
            sum += weight * lit;
            weights += weight;
        }
    }
    return vec2<f32>(sum / weights, edge);
}

// The key light's visibility at `position` past the cascades' casters, which are environment
// casters alone: what character surfaces receive (anime_character.wgsl's `cast_shadow`), and part
// of what environment surfaces do. `normal` is the surface's geometric normal, unit length.
fn cascade_visibility(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if !SHADOWS {
        return 1.0;
    }
    let count = min(shadows.counts.x, MAX_SHADOW_CASCADES);
    let reach = shadows.camera.w;
    let fade = smoothstep(reach * SHADOW_FADE_START, reach, distance(position, shadows.camera.xyz));
    if count == 0u || fade >= 1.0 {
        return 1.0;
    }
    let n_dot_l = dot(normal, shadows.light_z.xyz);
    let slope = sqrt(saturate(1.0 - n_dot_l * n_dot_l));
    let p = light_space(position);
    let n = light_space(normal);
    // The first cascade that holds the point, whose outer tenth blends into the next.
    for (var index = 0u; index < count; index++) {
        let lookup = cascade_lookup(index, p, n, slope);
        if lookup.y > 1.0 {
            continue;
        }
        var visibility = lookup.x;
        let blend = smoothstep(1.0 - SHADOW_CASCADE_BLEND, 1.0, lookup.y);
        if blend > 0.0 && index + 1u < count {
            let next = cascade_lookup(index + 1u, p, n, slope);
            if next.y <= 1.0 {
                visibility = mix(visibility, next.x, blend);
            }
        }
        return mix(visibility, 1.0, fade);
    }
    return 1.0;
}

// The key light's visibility at light-space `p` past the characters' own maps (desktop).
fn character_map_visibility(p: vec3<f32>, n: vec3<f32>, slope: f32) -> f32 {
    var visibility = 1.0;
    let count = min(shadows.counts.y, MAX_CHARACTER_SHADOW_MAPS);
    let size = shadows.atlas.z;
    for (var index = 0u; index < count; index++) {
        let map = shadows.characters[index];
        let texel = map.area.w;
        let at = p + n * (texel * shadow_offset_texels(slope));
        let uv = vec2<f32>(0.5, 0.5) + (at.xy - map.area.xy) * map.area.z * vec2<f32>(1.0, -1.0);
        // Its reach ends `depth.z` away from the light, fading out before.
        let fade = saturate((at.z - map.depth.z) * map.depth.w);
        if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || fade <= 0.0 {
            continue;
        }
        // Kept inside the tile, whose padding the character never reaches: the filter never reads
        // another character's tile.
        let inside = clamp(uv, vec2<f32>(SHADOW_EDGE_TEXELS / size), vec2<f32>(1.0 - SHADOW_EDGE_TEXELS / size));
        let atlas_uv = map.tile.xy + inside * map.tile.zw;
        let depth = saturate((at.z + texel * SHADOW_DEPTH_BIAS) * map.depth.x + map.depth.y);
        let tent = shadow_tent(atlas_uv / shadows.atlas.xy);
        var sum = 0.0;
        var weights = 0.0;
        for (var j = 0u; j < tent.taps; j++) {
            for (var i = 0u; i < tent.taps; i++) {
                let coords = (tent.base + vec2<f32>(tent.u[i], tent.v[j])) * shadows.atlas.xy;
                let lit = textureSampleCompareLevel(character_shadows, shadow_sampler, coords, depth);
                let weight = tent.weight_u[i] * tent.weight_v[j];
                sum += weight * lit;
                weights += weight;
            }
        }
        visibility *= mix(1.0, sum / weights, fade);
    }
    return visibility;
}

// The key light's visibility at `position` past the characters' capsules: where the light's ray
// from the point passes closest to a capsule's axis, its shadow, soft by the radius and the
// distance.
fn capsule_visibility(position: vec3<f32>) -> f32 {
    var visibility = 1.0;
    let l = shadows.light_z.xyz;
    let count = min(shadows.counts.z, MAX_SHADOW_CAPSULES);
    for (var index = 0u; index < count; index++) {
        let capsule = shadows.capsules[index];
        let a = capsule.base.xyz;
        let radius = capsule.base.w;
        let axis = capsule.top.xyz - a;
        let w = position - a;
        // The closest approach of the ray position + t·l (t ≥ 0) and the segment a + u·axis.
        let e = dot(axis, axis);
        let along = dot(l, axis);
        let c = dot(l, w);
        let f = dot(axis, w);
        let denominator = e - along * along;
        var u = select(0.0, (f - c * along) / denominator, denominator > 1e-8);
        u = saturate(u);
        let t = max(u * along - c, 0.0);
        u = select(0.0, saturate((f + t * along) / e), e > 1e-8);
        let d = length(w + t * l - u * axis);
        if t <= 0.0 {
            continue;
        }
        let soft = radius * CAPSULE_SOFTNESS + t * CAPSULE_SPREAD;
        let occlusion = 1.0 - smoothstep(radius - soft, radius + soft, d);
        let fade = 1.0 - smoothstep(0.75 * CAPSULE_REACH, CAPSULE_REACH, t);
        visibility *= 1.0 - occlusion * fade;
    }
    return visibility;
}

// The key light's visibility at `position` on an environment surface whose geometric normal is
// `normal` (unit): past the cascades' casters and every character's own shadow.
fn shadow_visibility(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if !SHADOWS {
        return 1.0;
    }
    var visibility = cascade_visibility(position, normal);
    let n_dot_l = dot(normal, shadows.light_z.xyz);
    let slope = sqrt(saturate(1.0 - n_dot_l * n_dot_l));
    if shadows.counts.y > 0u {
        visibility *= character_map_visibility(light_space(position), light_space(normal), slope);
    }
    if shadows.counts.z > 0u {
        visibility *= capsule_visibility(position);
    }
    return visibility;
}
