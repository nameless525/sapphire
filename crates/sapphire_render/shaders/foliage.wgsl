// Foliage (M1 W6, spec §5.12 "Foliage", look pillar 7): NPR leaves, canopies, bushes and grass.
// Anime foliage reads as soft painted clumps, not noisy individual leaves: a canopy lights as one
// rounded volume with a clean terminator and a soft colored shade, its leaf cards breaking the
// silhouette, and grass and branches sway in layered wind.
//
// Meshes carry what makes that work, cooked by the pipeline (`sapphire_pipeline::bake::foliage`):
// - **volume normals:** leaf cards' normals replaced by a sphere's, clumps', a smooth hull's, or
//   straight up for grass, so the light follows the canopy's shape and blades light like the
//   ground they stand on;
// - **the foliage stream** (vertex slot 5, `sapphire_assets::mesh::VertexFoliage`): x trunk sway,
//   y branch bend and z leaf flutter weights, w the branch's phase; then the gradient coordinate
//   (0 at the root or deep in the canopy, 1 at the tip or on its surface).
//
// Materials are the foliage family (`FoliageParams` in materials.wgsl, bound as
// `foliage_materials`). The key light is the world's (`frame.sun_*`), shaded as the environment
// shader shades it (environment.wgsl), so foliage sits in the world's picture:
// - **The terminator:** N·L on the volume normal, pushed toward the deepest shadow in half-Lambert
//   space by the cast shadow and the canopy's occlusion (`occluded`), through
//   `smoothstep(-w, w, ·)`, `w` the material's width plus its growth per meter of view distance
//   (at least half a pixel's change), so far canopies soften into painted masses.
// - **Wrap and shadow tint:** the lit side falls off as wrapped Lambert; the shadow side and cast
//   shadows keep time of day's shadow tint × the material's, a soft colored shade, never black.
// - **Canopy occlusion and the color gradient:** along the gradient coordinate the albedo moves
//   from the material's root color to its tip color, and the canopy's inside darkens by its
//   canopy occlusion: inner leaves deeper and pushed into the shade, the outer ones bright.
//   Vertex AO (vertex color R) and the mask map's G occlude too.
// - **Translucency:** light through thin leaves seen against the key light,
//   `pow(saturate(dot(-L, V)), power)`, strongest on the side facing away from it and at the
//   canopy's edges (the gradient's tip), scaled by the mask map's thickness (R) and kept out of
//   cast shadow: a canopy against a low sun glows. A tier can turn it off (`FOLIAGE_TRANSLUCENCY`).
// - **Ambient light,** the frame's hemisphere, occluded as above; **distance desaturation** of the
//   albedo, as the environment's.
// - **Local lights** (R5's lists) light the diffuse through the same terminator and wrap.
// - **Fog** (M1 W3) covers the final color, as it does every world pass's (`fogged` in
//   frame.wgsl): distance and height fog colored by the sky's horizon.
// No specular: leaves are matte at the picture's scale.
//
// **Wind** (`wind_offset`): three levels, each a material amplitude (`FoliageParams::wind`, meters
// at unit strength) × the vertex's cooked weight × the frame's wind (`WindUniforms`, the
// foliage scene group's): the trunk leans downwind and sways slowly, each plant in its own
// phase; branches swing faster in their own phases; leaves flutter fast in a direction that
// wanders across the canopy. Gusts swell and travel downwind across the world. Every term is a
// continuous function of the vertex's position and cooked data; the bake gives a card's vertices
// one phase, and its branch's sway and bend where they meet (sapphire_pipeline's
// `bake::foliage`), so a card moves as one piece, swaying and bending with its branch. The tier
// picks how many levels run (`FOLIAGE_WIND_LEVELS`). Every pass displaces alike (the main pass,
// the scene prepass and the shadow casters), and the prepass's velocity takes the last frame's
// wind as well as its transform, so temporal AA reprojects swaying leaves without ghosts.
//
// **Coverage:** alpha-tested cards discard below their cutoff, their alpha scaled up with the
// map's mip level so distant canopies keep their fullness (Golus, "Anti-aliased Alpha Test",
// 2017). That is the desktop path, where temporal AA's jittered history resolves the edges, and
// the prepass and the shadow casters test alike, so every depth matches and shadows cut out.
// Under MSAA (the phones) the main pass writes alpha-to-coverage instead (the tier's
// `alpha_to_coverage`, through `fs_foliage_coverage`): the coverage sharpened to a pixel,
// `(alpha - cutoff) / fwidth(alpha) + 0.5`, covers that share of the pixel's samples through the
// sample mask, a run of samples starting where a hash of the pixel and the card picks (a dither,
// so 4×'s five levels don't band and stacked edges cover different samples), so card edges
// resolve smoothly in tile memory. The mask is written rather than hardware alpha-to-coverage,
// which would also store the coverage in scene color's alpha, where 1 marks the world for
// post's character mask.

// Texture slots (`FoliageMaterial::TEXTURE_SLOTS`).
const FOLIAGE_BASE_COLOR: u32 = 0u;
const FOLIAGE_MASK: u32 = 1u;

// The wrapped time's period in seconds (`foliage::WIND_PERIOD`): every oscillator runs a whole
// number of cycles in it, so the time wraps without a jump.
const WIND_PERIOD: f32 = 1024.0;
// The trunk's swing, the branches' and the vertical bob's frequencies, and the gusts' swell.
const WIND_SWAY_HZ: f32 = 0.31;
const WIND_BEND_HZ: f32 = 0.83;
const WIND_BOB_HZ: f32 = 1.27;
const WIND_GUST_HZ: f32 = 0.09;
// Meters between gust fronts traveling downwind.
const WIND_GUST_WAVELENGTH: f32 = 36.0;
// The trunk's lean downwind and its swing around it, as shares of its sway.
const WIND_LEAN: f32 = 0.6;
const WIND_SWING: f32 = 0.4;
// How fast the flutter's direction wanders across space, in cycles per meter.
const WIND_FLUTTER_SPREAD: vec3<f32> = vec3<f32>(1.37, 0.91, 1.13);
// Each plant's own timing from where it stands, in cycles per meter of x and z.
const WIND_PLANT_PHASE: vec2<f32> = vec2<f32>(0.0713, 0.0917);
// Alpha gained per mip level, so cards keep their coverage as the map's mips average it away.
const ALPHA_MIP_SCALE: f32 = 0.25;
const TAU: f32 = 6.28318530717959;

// Static permutations and the tier's settings (`pipelines::SurfaceVariant`,
// `foliage::FoliageSettings`), set per pipeline. The sample count is the main pass's, which
// `fs_foliage_coverage` covers.
override PERMUTATION_ALPHA_TEST: bool = false;
override FOLIAGE_SAMPLES: u32 = 4u;
override FOLIAGE_WIND_LEVELS: u32 = 3u;
override FOLIAGE_TRANSLUCENCY: bool = true;

// The wind at one frame (`foliage::WindState`).
struct WindState {
    // xyz: unit horizontal direction the wind blows toward; w: strength, 1 the materials' unit
    // wind, 0 calm.
    direction: vec4<f32>,
    // x: time in seconds, wrapped to WIND_PERIOD; y: gustiness, 0 to 1.
    time: vec4<f32>,
}

// This frame's wind, and the last drawn frame's, for velocity (`foliage::WindUniforms`).
struct WindUniforms {
    current: WindState,
    previous: WindState,
}

// The frame's light lists: a list per cluster or per instance slot (see lights.wgsl).
@group(0) @binding(2) var<storage, read> light_lists: array<u32>;
// The foliage family's blocks, by the draw record's material slot, and the wind (the foliage
// pipelines' scene group).
@group(2) @binding(1) var<storage, read> foliage_materials: array<FoliageParams>;
@group(2) @binding(2) var<uniform> wind: WindUniforms;

// The foliage family's group 3 (`FoliageMaterial::TEXTURE_SLOTS`), each unbound slot a white
// 1×1 fallback.
@group(3) @binding(0) var base_color_map: texture_2d<f32>;
@group(3) @binding(1) var mask_map: texture_2d<f32>;
@group(3) @binding(8) var material_sampler: sampler;

// Vertex slot 5: the foliage stream.
struct FoliageVertex {
    // x sway, y bend, z flutter weights; w the branch's phase in cycles.
    @location(11) wind: vec4<f32>,
    // x: the gradient coordinate; yzw reserved.
    @location(12) shape: vec4<f32>,
}

struct FoliageVaryings {
    // Invariant, so the scene prepass computes the same depth.
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // x: vertex AO; y: the gradient coordinate.
    @location(3) shape: vec2<f32>,
    @location(4) @interpolate(flat) material: u32,
    // The instance's slot: its light list on the per-object path.
    @location(5) @interpolate(flat) instance: u32,
    // The triangle's first vertex (flat interpolation takes the first): a number for its card,
    // which alpha-to-coverage's dither hashes so stacked cards cover different samples. A card's
    // two triangles may number differently; where an edge crosses its diagonal the two dither
    // apart, which covers the same share of the pixel on average.
    @location(6) @interpolate(flat) card: u32,
}

// Cycles of an oscillator at `hz` at wrapped time `t`, in [0, 1). The frequency is rounded to a
// whole number of cycles per WIND_PERIOD (steps of 1/1024 Hz), so `t` wrapping changes nothing.
fn wind_cycles(hz: f32, t: f32) -> f32 {
    return fract(round(hz * WIND_PERIOD) / WIND_PERIOD * t);
}

// How far the wind `state` moves a vertex at world position `position` (before wind), of an
// instance whose origin (the plant's root) is at `origin`, with cooked `weights` (sway, bend,
// flutter, phase) under a material whose response is `response` (sway, bend and flutter in
// meters at unit strength, flutter frequency). `foliage::wind_offset` mirrors it for tests.
fn wind_offset(
    state: WindState,
    position: vec3<f32>,
    origin: vec3<f32>,
    weights: vec4<f32>,
    response: vec4<f32>,
) -> vec3<f32> {
    let strength = state.direction.w;
    let direction = state.direction.xyz;
    let t = state.time.x;
    // Phases are taken in cycles and wrapped before they become angles, so `sin` stays precise
    // however far from the origin the plant stands.
    // Gusts: a slow swell traveling downwind, so a field ripples as fronts pass.
    let downwind = dot(position.xz, direction.xz) / WIND_GUST_WAVELENGTH;
    let gust = 1.0 + state.time.y * sin(TAU * fract(wind_cycles(WIND_GUST_HZ, t) - downwind));
    // Each plant's own timing, from where it stands.
    let plant = dot(origin.xz, WIND_PLANT_PHASE);
    let phase = weights.w;
    var offset = vec3<f32>(0.0);
    if FOLIAGE_WIND_LEVELS >= 1u {
        // The trunk: a lean downwind with a slow swing around it.
        let swing = sin(TAU * fract(wind_cycles(WIND_SWAY_HZ, t) + plant));
        offset += direction * (response.x * weights.x * (WIND_LEAN + WIND_SWING * swing));
    }
    if FOLIAGE_WIND_LEVELS >= 2u {
        // Branches: faster, each in its own phase, bobbing up and down a little.
        let branch = TAU * fract(wind_cycles(WIND_BEND_HZ, t) + phase + plant);
        let bob = TAU * fract(wind_cycles(WIND_BOB_HZ, t) + phase);
        offset += (direction * sin(branch) + vec3<f32>(0.0, 0.4 * cos(bob), 0.0)) * (response.y * weights.y);
    }
    if FOLIAGE_WIND_LEVELS >= 3u {
        // Leaves: fast and small, in a direction that wanders across the canopy and in time.
        let a = TAU * fract(wind_cycles(response.w, t) + phase + fract(dot(position, WIND_FLUTTER_SPREAD)));
        let wander = vec3<f32>(sin(a), 0.6 * sin(2.0 * a + 1.3), cos(3.0 * a + 0.4));
        offset += wander * (response.z * weights.z);
    }
    // Bending, not shearing: what moves sideways drops a little, as a point on a bending stem.
    offset *= strength * gust;
    let height = max(position.y - origin.y, 0.0);
    let sideways = dot(offset.xz, offset.xz);
    offset.y -= min(sideways / (2.0 * max(height, 0.1)), sqrt(sideways));
    return offset;
}

// A vertex's world position under the wind `state`, through `world_from_object`.
fn foliage_position(
    state: WindState,
    world_from_object: mat4x4<f32>,
    object: vec3<f32>,
    weights: vec4<f32>,
    response: vec4<f32>,
) -> vec3<f32> {
    let world = (world_from_object * vec4<f32>(object, 1.0)).xyz;
    let origin = world_from_object[3].xyz;
    return world + wind_offset(state, world, origin, weights, response);
}

@vertex
fn vs_foliage(
    @builtin(vertex_index) index: u32,
    packed: PackedVertex,
    foliage: FoliageVertex,
    draw: DrawRecord,
) -> FoliageVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    let response = foliage_materials[draw.material].wind;
    let world = foliage_position(wind.current, instance.world_from_object, vertex.position, foliage.wind, response);
    var out: FoliageVaryings;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.world_position = world;
    out.world_normal = instance.normal_from_object * vertex.normal;
    out.uv = vertex.uv;
    out.shape = vec2<f32>(vertex.color.r, foliage.shape.x);
    out.material = draw.material;
    out.instance = draw.instance;
    out.card = index;
    return out;
}

// Texture slot `slot` at `uv`, through the slot's swizzle.
fn sample_foliage(map: texture_2d<f32>, swizzles: vec4<u32>, slot: u32, uv: MaterialUv) -> vec4<f32> {
    let sampled = textureSampleGrad(map, material_sampler, uv.uv, uv.dx, uv.dy);
    return material_swizzle(sampled, swizzles, slot);
}

// The material's color and coverage at `uv`: base color × its map, alpha scaled up with the
// map's mip level (see the top of this file).
fn foliage_base(material: u32, uv: MaterialUv) -> vec4<f32> {
    let modes = foliage_materials[material].modes;
    var base = foliage_materials[material].base_color;
    if material_has_texture(modes, FOLIAGE_BASE_COLOR) {
        base *= sample_foliage(base_color_map, foliage_materials[material].swizzles, FOLIAGE_BASE_COLOR, uv);
        let size = vec2<f32>(textureDimensions(base_color_map));
        let texels = max(length(uv.dx * size), length(uv.dy * size));
        base.a *= 1.0 + max(log2(max(texels, 1e-8)), 0.0) * ALPHA_MIP_SCALE;
    }
    return base;
}

// Every listed local light's diffuse: through the terminator, at least a pixel's change of the
// light's N·L wide, and wrapped, as the environment shader's.
fn foliage_local_lights(
    n: vec3<f32>,
    position: vec3<f32>,
    list: u32,
    wrap: f32,
    width: f32,
    normal_dx: vec3<f32>,
    normal_dy: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
) -> vec3<f32> {
    let base = list * frame.lights.z;
    let count = min(light_lists[base], frame.lights.w);
    var diffuse = vec3<f32>(0.0);
    for (var i = 0u; i < count; i++) {
        let index = (light_lists[base + 1u + i / 2u] >> ((i & 1u) * 16u)) & 0xffffu;
        if index >= frame.lights.x {
            continue;
        }
        let light = lights[index];
        let to_light = light.position.xyz - position;
        let distance_squared = dot(to_light, to_light);
        let window = light_window(distance_squared, light.radiance.w);
        if window <= 0.0 {
            continue;
        }
        let distance = max(sqrt(distance_squared), 1e-4);
        let l = to_light / distance;
        let n_dot_l = dot(n, l);
        let dx = dot(normal_dx, l) + (n_dot_l * dot(l, position_dx) - dot(n, position_dx)) / distance;
        let dy = dot(normal_dy, l) + (n_dot_l * dot(l, position_dy) - dot(n, position_dy)) / distance;
        let lit = light_step(0.0, max(width, 0.5 * (abs(dx) + abs(dy))), n_dot_l);
        diffuse += light.radiance.rgb * (window * light_cone(light, l) * lit * wrapped(n_dot_l, wrap));
    }
    return diffuse;
}

// A foliage pixel: its lit color and its coverage, before the alpha test.
struct FoliageShade {
    color: vec3<f32>,
    // Alpha, mip-scaled; the test compares it with the cutoff.
    alpha: f32,
    // The coverage sharpened to a pixel: 0.5 at the cutoff, ramping over one pixel's change.
    coverage: f32,
}

// Shades `in` (see the top of this file). Every derivative is taken here, in uniform control
// flow, before the entry points test coverage.
fn shade_foliage(in: FoliageVaryings) -> FoliageShade {
    let material = in.material;
    let modes = foliage_materials[material].modes;
    let swizzles = foliage_materials[material].swizzles;
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let position_dx = dpdx(in.world_position);
    let position_dy = dpdy(in.world_position);
    let base = foliage_base(material, uv);
    let shading = foliage_materials[material].shading;
    var mask = vec4<f32>(1.0);
    if material_has_texture(modes, FOLIAGE_MASK) {
        mask = sample_foliage(mask_map, swizzles, FOLIAGE_MASK, uv);
    }
    let n = safe_normalize(in.world_normal);
    let to_camera = view.position.xyz - in.world_position;
    let distance = length(to_camera);
    let v = to_camera / max(distance, 1e-6);

    // The gradient: the root's color to the tip's, and the canopy's inside occluded.
    let gradient = in.shape.y;
    let root = foliage_materials[material].gradient_root;
    let tip = foliage_materials[material].gradient_tip.rgb;
    let canopy = mix(1.0 - root.w, 1.0, gradient);
    let occlusion = in.shape.x * mask.g * canopy;
    let stylization = foliage_materials[material].distance;
    let tinted = base.rgb * mix(root.rgb, tip, gradient);
    let albedo = mix(tinted, vec3<f32>(dot(tinted, LUMINANCE)), stylization.z * distance_amount(distance, stylization));

    // The key light through the environment's terminator, cast shadow and occlusion entering it.
    let l = frame.sun_direction.xyz;
    let n_dot_l = dot(n, l);
    let shadow = shadow_visibility(in.world_position, n);
    let x = occluded(n_dot_l, shadow * occlusion);
    let width = shading.z + shading.w * distance;
    let lit = light_step(0.0, max(width, 0.5 * fwidth(x)), x);
    let keep = frame.shadow_tint.rgb * foliage_materials[material].shadow_tint.rgb;
    let key = frame.sun_radiance.rgb * mix(keep, vec3<f32>(1.0), lit * wrapped(n_dot_l, shading.y));
    var color = albedo * (key + ambient_light(n) * occlusion);
    if FOLIAGE_TRANSLUCENCY {
        // Light through thin leaves seen against the key light: strongest facing away from it and
        // at the canopy's edges, never through a cast shadow.
        let translucency = foliage_materials[material].translucency;
        let through = pow(saturate(dot(-l, v)), max(translucency.a, 1e-3));
        let away = saturate(0.5 - 0.5 * n_dot_l);
        let thickness = mask.r * mix(0.35, 1.0, gradient);
        color += frame.sun_radiance.rgb * translucency.rgb * (through * away * thickness * shadow);
    }
    // A uniform branch: frames without local lights skip them. The normal's derivatives are taken
    // either way, in uniform control flow.
    let normal_dx = dpdx(n);
    let normal_dy = dpdy(n);
    if frame.lights.y != LIGHT_LISTS_NONE {
        let list = light_list(in.clip_position, in.instance);
        let local = foliage_local_lights(
            n,
            in.world_position,
            list,
            shading.y,
            width,
            normal_dx,
            normal_dy,
            position_dx,
            position_dy,
        );
        color += albedo * local * occlusion;
    }
    var shaded: FoliageShade;
    // The surface's final color, with the frame's fog over it (M1 W3, `fogged` in frame.wgsl,
    // which every world pass shares): the one place it is produced.
    shaded.color = fogged(color, in.world_position, 1.0);
    shaded.alpha = base.a;
    shaded.coverage = saturate((base.a - shading.x) / max(fwidth(base.a), 1e-4) + 0.5);
    return shaded;
}

// Opaque foliage, and alpha-tested foliage without MSAA: discards below the cutoff. Scene color's
// alpha is 1, the world's.
@fragment
fn fs_foliage(in: FoliageVaryings) -> @location(0) vec4<f32> {
    let shaded = shade_foliage(in);
    if PERMUTATION_ALPHA_TEST && shaded.alpha < foliage_materials[in.material].shading.x {
        discard;
    }
    return vec4<f32>(shaded.color, 1.0);
}

struct FoliageCoverage {
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) mask: u32,
}

// A hash of a pixel and a card (lowbias32, Wellons): where a partly covered pixel's samples
// start.
fn coverage_hash(pixel: vec2<u32>, card: u32) -> u32 {
    var h = (pixel.x * 0x8da6b343u) ^ (pixel.y * 0xd8163841u) ^ (card * 0xcb1ab31fu);
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

// Alpha-to-coverage's sample mask: `coverage × samples` of a pixel's `samples` (rounded), a run
// that starts at a sample the pixel and the card pick (`coverage_hash`). An ordered run (always
// the first samples) would band at 4×, which has five levels, and two half-covering edges
// stacked in a pixel would cover the same half, so a canopy's edges would read see-through; a
// rotated run dithers the levels across pixels and lets stacked cards cover different samples.
fn coverage_mask(coverage: f32, samples: u32, pixel: vec2<u32>, card: u32) -> u32 {
    let covered = min(u32(round(saturate(coverage) * f32(samples))), samples);
    let all = select((1u << samples) - 1u, 0xffffffffu, samples >= 32u);
    let run = select((1u << covered) - 1u, 0xffffffffu, covered >= 32u);
    let start = coverage_hash(pixel, card) % max(samples, 1u);
    // `run` rotated by `start` within the pixel's samples; shifts stay under 32.
    let wrapped = select(run >> ((samples - start) % 32u), 0u, start == 0u);
    return ((run << start) | wrapped) & all;
}

// Alpha-tested foliage under MSAA (the tier's `alpha_to_coverage`): alpha-to-coverage through the
// sample mask (`coverage_mask`), so a card's edge covers the share of the pixel it covers and
// resolves smoothly.
@fragment
fn fs_foliage_coverage(in: FoliageVaryings) -> FoliageCoverage {
    let shaded = shade_foliage(in);
    var out: FoliageCoverage;
    out.color = vec4<f32>(shaded.color, 1.0);
    out.mask = coverage_mask(shaded.coverage, FOLIAGE_SAMPLES, vec2<u32>(in.clip_position.xy), in.card);
    return out;
}

// The scene prepass's varyings for foliage: the scene's, and what the alpha test reads.
struct FoliagePrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) current: vec4<f32>,
    @location(2) previous: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) material: u32,
}

// The scene prepass (prepass.wgsl's, for foliage): depth, normals and velocity, the wind included
// in both this frame's position and the last drawn one's.
@vertex
fn vs_foliage_prepass(
    packed: PackedVertex,
    @location(10) previous_position: vec4<f32>,
    foliage: FoliageVertex,
    draw: DrawRecord,
) -> FoliagePrepassVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    let response = foliage_materials[draw.material].wind;
    let world = foliage_position(wind.current, instance.world_from_object, vertex.position, foliage.wind, response);
    let previous_object = object_position(previous_position, draw);
    let previous = foliage_position(
        wind.previous,
        instance.previous_world_from_object,
        previous_object,
        foliage.wind,
        response,
    );
    var out: FoliagePrepassVaryings;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.current = view.unjittered_clip_from_world * vec4<f32>(world, 1.0);
    out.previous = view.previous_clip_from_world * vec4<f32>(previous, 1.0);
    out.world_normal = instance.normal_from_object * vertex.normal;
    out.uv = vertex.uv;
    out.material = draw.material;
    return out;
}

// Whether a card's pixel is covered, as the main pass tests it without MSAA. Opaque foliage
// covers every pixel and reads no map (a pipeline constant, so the branch is uniform and its
// derivatives are taken in uniform control flow).
fn foliage_covered(material: u32, uv: vec2<f32>) -> bool {
    var covered = true;
    if PERMUTATION_ALPHA_TEST {
        let base = foliage_base(material, MaterialUv(uv, dpdx(uv), dpdy(uv)));
        covered = base.a >= foliage_materials[material].shading.x;
    }
    return covered;
}

@fragment
fn fs_foliage_prepass(in: FoliagePrepassVaryings) -> ScenePrepassTargets {
    if !foliage_covered(in.material, in.uv) {
        discard;
    }
    return scene_prepass_targets(ScenePrepassVaryings(in.clip_position, in.world_normal, in.current, in.previous));
}

struct FoliageCasterVaryings {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) material: u32,
}

// The shadow casters (shadows.wgsl's, for foliage): through the shadow view bound at the map's
// offset, with the wind, so leaves' shadows sway with them.
@vertex
fn vs_foliage_caster(
    @location(0) position: vec4<f32>,
    @location(3) uv: vec2<f32>,
    foliage: FoliageVertex,
    draw: DrawRecord,
) -> FoliageCasterVaryings {
    let instance = instances[draw.instance];
    let response = foliage_materials[draw.material].wind;
    let object = object_position(position, draw);
    let world = foliage_position(wind.current, instance.world_from_object, object, foliage.wind, response);
    var out: FoliageCasterVaryings;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.uv = draw.uv_transform.xy + draw.uv_transform.zw * uv;
    out.material = draw.material;
    return out;
}

// Alpha-tested casters cut out: a card casts its leaves' shape, not its quad.
@fragment
fn fs_foliage_caster(in: FoliageCasterVaryings) {
    if !foliage_covered(in.material, in.uv) {
        discard;
    }
}
