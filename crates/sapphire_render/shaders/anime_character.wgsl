// Pass 1: anime character surfaces (M1 C1, spec §5.12). One uber-shader: a thresholded light
// split with authored shadow colors, an N·V rim, a small sky ambient term, and local point and
// spot lights split the same way.
//
// Materials are the anime character family (`AnimeCharacterParams` in materials.wgsl). Per
// pixel the shader reads:
// - the base color map (albedo, and coverage for the alpha-tested and transparent permutations);
// - the normal map, turned into world space by a frame from screen derivatives (meshes carry no
//   tangents yet);
// - the control (ILM) map in A7's layout (`sapphire_assets::character_maps`): R specular mask,
//   G shadow bias, B specular threshold, filtered; A the material ID, read unfiltered from level
//   0 through the nearest sampler, which picks the pixel's region of the per-ID parameters.
//   Without a control map the region is the vertex's material ID (its flags) and the bias
//   neutral.
//
// The light split. N·L is scaled in half-Lambert space by the occlusion (vertex AO × the cast
// shadow), so occluded and shadowed points move toward the shadow side rather than darkening;
// the control map's shadow bias G adds the offset 2·G − 1 to it (0.5 is neutral, a higher G
// lights more: `character_maps::ilm`, which MToon's shading shift maps onto); and the sum is
// compared with the material's threshold. At G ≥ 0.95 a point stays lit and at G ≤ 0.05 it
// stays in shadow, whatever the light. The transition is about a pixel wide, or the material's
// softness if wider.
//
// The shadow side's color is authored, never a darkened albedo: albedo × a tint from one of two
// diffuse modes, chosen per material on a branch every pixel of a draw takes alike:
// - **Band mode** (ZZZ): the region's shade color, and with a deep threshold a third band below
//   it in the region's deep shade color. No ramp texture: cheapest on mobile.
// - **Ramp mode** (Genshin, Star Rail, WuWa): the region's ramp rows, sampled at how far the
//   point is into shadow (u = 0 where the split's input is -1, 1 at the threshold), its day and
//   night rows
//   blended by the frame's day/night factor, then mixed with the region's subsurface tint by the
//   material's ramp mix (WuWa's `_RampInt`; 1 is the ramp alone).
//
// Local lights (M1 R5) are lit like the sun's key light: each light splits its own N·L at the
// same threshold (bias, locks and vertex AO included) into a lit and a shadow tone, the shadow
// tone being the surface's shadow color, in the light's color × its smooth range window × its
// spot cone, with a one-pixel edge. Their sum is held under LOCAL_LIGHT_CEILING, so stacked
// lights tint rather than blow out. Lights come from the frame's light lists (`lights.wgsl`): the
// pixel's cluster on desktop, its instance's own list on mobile. Local lights add no rim; the
// rim stays the key light's (C5 revisits it).
//
// Static permutations (`pipelines::Permutation`) are pipeline-overridable constants, so each
// variant compiles only its own code: alpha-tested materials discard below their cutoff, and
// transparent ones write their coverage for blending. Faces (C3), hair (C5) and eyes (C6) add
// theirs.

// Strength of the hemispherical sky ambient. Kept small so the two tones stay distinct.
const AMBIENT_STRENGTH: f32 = 0.12;
// The most luminance local lights add together, in units of a unit sun. The character light
// rig (C2) makes it a rig parameter.
const LOCAL_LIGHT_CEILING: f32 = 2.0;
const LUMINANCE: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// The narrowest light split, in N·L: far below a pixel's change on any curved surface.
const MIN_SPLIT_WIDTH: f32 = 1e-5;

// The control map's shadow bias (`character_maps::ilm`): neutral, and the values at and beyond
// which a point stays lit or in shadow whatever the light.
const SHADOW_BIAS_NEUTRAL: f32 = 0.5;
const SHADOW_BIAS_LIT: f32 = 0.95;
const SHADOW_BIAS_SHADOWED: f32 = 0.05;
// Ramp rows (`character_maps::ramp::ROWS`): each region's day row, then its night row.
const RAMP_ROWS: f32 = 16.0;
// The control map's material IDs, k stored as k / (MATERIAL_ID_LEVELS - 1).
const MATERIAL_ID_LEVELS: f32 = 8.0;

// Static permutation bits (`pipelines::Permutation`), set per pipeline.
override PERMUTATION_ALPHA_TEST: bool = false;
override PERMUTATION_TRANSPARENT: bool = false;

// The frame's light lists: a list per cluster or per instance slot (see lights.wgsl).
@group(0) @binding(2) var<storage, read> light_lists: array<u32>;

// The anime character family's group 3 (`AnimeCharacterMaterial::TEXTURE_SLOTS`): slot `i` at
// binding `i`, each unbound one a white 1×1 fallback. Face, surface mask and matcap reads join
// with C3, C5 and C7.
@group(3) @binding(0) var base_color_map: texture_2d<f32>;
@group(3) @binding(1) var control_map: texture_2d<f32>;
@group(3) @binding(2) var normal_map: texture_2d<f32>;
@group(3) @binding(3) var ramp_map: texture_2d<f32>;
@group(3) @binding(4) var face_shadow_map: texture_2d<f32>;
@group(3) @binding(5) var surface_mask_map: texture_2d<f32>;
@group(3) @binding(6) var matcap_map: texture_2d<f32>;
@group(3) @binding(8) var material_sampler: sampler;
@group(3) @binding(9) var ramp_sampler: sampler;
@group(3) @binding(10) var material_id_sampler: sampler;

struct CharacterVaryings {
    // Invariant, so the prepasses (prepass.wgsl) compute the same depth.
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    // The material textures' coordinates.
    @location(2) uv: vec2<f32>,
    // The vertex color's ambient occlusion.
    @location(3) occlusion: f32,
    @location(4) @interpolate(flat) material: u32,
    // The instance's slot: its light list on the per-object path.
    @location(5) @interpolate(flat) instance: u32,
    // The vertex's material ID, the region of a material without a control map.
    @location(6) @interpolate(flat) vertex_material_id: u32,
}

@vertex
fn vs_anime_character(packed: PackedVertex, draw: DrawRecord) -> CharacterVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(vertex.position, 1.0);
    var out: CharacterVaryings;
    out.clip_position = view.clip_from_world * world_position;
    out.world_position = world_position.xyz;
    out.world_normal = instance.normal_from_object * vertex.normal;
    out.uv = vertex.uv;
    out.occlusion = vertex.color.r;
    out.material = draw.material;
    out.instance = draw.instance;
    out.vertex_material_id = vertex.flags & VERTEX_MATERIAL_ID_MASK;
    return out;
}

// Everything the lighting stages need about one shaded point.
struct CharacterSurface {
    // Material base color × base color map.
    albedo: vec3<f32>,
    // The shadow side's color: albedo × the diffuse mode's tint for this point.
    shade: vec3<f32>,
    normal: vec3<f32>,
    // Unit vector toward the camera.
    view_dir: vec3<f32>,
    // The vertex color's ambient occlusion.
    occlusion: f32,
    // The control map's channels: R specular mask, G shadow bias, B specular threshold (C7
    // reads the specular ones).
    specular_mask: f32,
    shadow_bias: f32,
    specular_threshold: f32,
    // The material's light split: x threshold on N·L, y softness.
    split: vec2<f32>,
    // The material's rim: x width, y strength.
    rim: vec2<f32>,
    // The region this point shades with: the control map's material ID, or the vertex's. Fields
    // are read from the material buffer one by one rather than copying the whole block, which
    // a per-pixel region index would otherwise force into scratch memory on mobile GPUs.
    region: AnimeCharacterRegion,
}

// Texture slot `slot` at `uv`, through the slot's swizzle (`swizzles` from the block).
fn sample_material(map: texture_2d<f32>, swizzles: vec4<u32>, slot: u32, uv: MaterialUv) -> vec4<f32> {
    let sampled = textureSampleGrad(map, material_sampler, uv.uv, uv.dx, uv.dy);
    return material_swizzle(sampled, swizzles, slot);
}

// The light split: 0 below `edge - width`, 1 above `edge + width`. The width never reaches 0,
// where smoothstep's edges would meet and its result is undefined (a flat surface with no
// softness, whose N·L doesn't change across a pixel).
fn light_step(edge: f32, width: f32, x: f32) -> f32 {
    let w = max(width, MIN_SPLIT_WIDTH);
    return smoothstep(edge - w, edge + w, x);
}

// N·L scaled by `occlusion` in half-Lambert space: unchanged at 1, pushed toward -1 (the
// deepest shadow) as it falls. Exactly N·L when nothing occludes.
fn occluded(n_dot_l: f32, occlusion: f32) -> f32 {
    return n_dot_l + (occlusion - 1.0) * (n_dot_l + 1.0);
}

// The light split's input: N·L, occluded, plus the shadow bias's offset `2·G − 1`, which a
// higher bias makes lighter. Exactly N·L with nothing occluding and a neutral bias.
fn split_input(n_dot_l: f32, occlusion: f32, bias: f32) -> f32 {
    return occluded(n_dot_l, occlusion) + (2.0 * bias - 1.0);
}

// `lit` with the shadow bias's locks: always lit at or above SHADOW_BIAS_LIT, always in shadow
// at or below SHADOW_BIAS_SHADOWED.
fn bias_locks(lit: f32, bias: f32) -> f32 {
    let shadowed = select(lit, 0.0, bias <= SHADOW_BIAS_SHADOWED);
    return select(shadowed, 1.0, bias >= SHADOW_BIAS_LIT);
}

// Sun light reaching the point past cast shadows: 1 lit, 0 shadowed. It scales the light
// split's input as vertex occlusion does, so a cast shadow pushes the point into the authored
// shadow tones instead of darkening them. Shadows (R4) fill it from the cascades, with
// characters receiving environment casters only (no self-shadow); until then nothing casts.
fn cast_shadow(in: CharacterVaryings) -> f32 {
    return 1.0;
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

// The control map's material ID at `uv`: level 0, unfiltered, so IDs never blend across region
// borders or in mips, and decoded by rounding, which tolerates block compression's error.
fn control_material_id(swizzles: vec4<u32>, uv: vec2<f32>) -> u32 {
    let sampled = textureSampleLevel(control_map, material_id_sampler, uv, 0.0);
    let level = material_swizzle(sampled, swizzles, CHARACTER_CONTROL).a;
    return u32(round(saturate(level) * (MATERIAL_ID_LEVELS - 1.0)));
}

// Row `row` of the ramp at `u`, through its swizzle.
fn ramp_row(swizzles: vec4<u32>, u: f32, row: f32) -> vec3<f32> {
    let sampled = textureSampleLevel(ramp_map, ramp_sampler, vec2<f32>(u, (row + 0.5) / RAMP_ROWS), 0.0);
    return material_swizzle(sampled, swizzles, CHARACTER_RAMP).rgb;
}

// The tint the shadow side's albedo is multiplied by, from the material's diffuse mode, for a
// point whose split input is `x` with a transition `width` wide.
fn shade_tint(surface: CharacterSurface, material: u32, x: f32, width: f32) -> vec3<f32> {
    let modes = materials[material].modes;
    if (modes.x & CHARACTER_FLAG_RAMP) != 0u {
        // How far into shadow: 0 facing away from the light, 1 at the terminator.
        let u = saturate((x + 1.0) / max(surface.split.x + 1.0, 1e-4));
        let swizzles = materials[material].swizzles;
        var ramp = ramp_row(swizzles, u, surface.region.shade.a);
        // The whole frame takes this branch alike; a frame at full day or night samples once.
        let night = frame.day_night;
        if night > 0.0 {
            ramp = mix(ramp, ramp_row(swizzles, u, surface.region.deep_shade.a), saturate(night));
        }
        return mix(surface.region.subsurface.rgb, ramp, materials[material].terms.x);
    }
    if (modes.x & CHARACTER_FLAG_DEEP_SHADE) != 0u {
        let above = bias_locks(light_step(materials[material].split.z, width, x), surface.shadow_bias);
        return mix(surface.region.deep_shade.rgb, surface.region.shade.rgb, above);
    }
    return surface.region.shade.rgb;
}

// N·V rim light: the mobile rim. Desktop gets a depth-difference rim once the prepass exists.
fn rim_light(surface: CharacterSurface, lit: f32) -> vec3<f32> {
    let fresnel = 1.0 - saturate(dot(surface.normal, surface.view_dir));
    let mask = light_step(1.0 - surface.rim.x, 0.5 * fwidth(fresnel), fresnel);
    // Mostly on the lit side, so it reads as the key light wrapping around the silhouette.
    let strength = surface.rim.y * mask * mix(0.25, 1.0, lit);
    let tint = mix(surface.albedo, vec3<f32>(1.0), 0.5) * surface.region.rim.rgb;
    return frame.sun_radiance.rgb * tint * strength;
}

// Screen-space derivatives of the shading point, taken once per pixel in uniform control flow:
// the local-light loop runs a different number of times per pixel, where `fwidth` is undefined.
struct CharacterDerivatives {
    normal_dx: vec3<f32>,
    normal_dy: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
}

// The light split for a local light toward the unit vector `l`, `distance` away. Its edge is
// about a pixel wide, from the exact first-order change of N·L across the pixel:
// d(N·L) = dN·L + N·dL, where dL = ((L·dP) L − dP) / distance.
fn local_light_split(
    surface: CharacterSurface,
    derivatives: CharacterDerivatives,
    l: vec3<f32>,
    distance: f32,
    n_dot_l: f32,
) -> f32 {
    let dx = dot(derivatives.normal_dx, l)
        + (n_dot_l * dot(l, derivatives.position_dx) - dot(surface.normal, derivatives.position_dx)) / distance;
    let dy = dot(derivatives.normal_dy, l)
        + (n_dot_l * dot(l, derivatives.position_dy) - dot(surface.normal, derivatives.position_dy)) / distance;
    let width = max(surface.split.y, 0.5 * (abs(dx) + abs(dy)));
    let x = split_input(n_dot_l, surface.occlusion, surface.shadow_bias);
    return bias_locks(light_step(surface.split.x, width, x), surface.shadow_bias);
}

// The light list this pixel walks: its cluster's, or its instance's.
fn pixel_light_list(in: CharacterVaryings) -> u32 {
    if frame.lights.y == LIGHT_LISTS_OBJECTS {
        return in.instance;
    }
    let tiles = vec2<f32>(f32(CLUSTER_TILES_X), f32(CLUSTER_TILES_Y));
    let tile = min(vec2<u32>(in.clip_position.xy * view.viewport.zw * tiles), vec2<u32>(tiles) - 1u);
    // Reverse-Z with an infinite far plane: view depth is near / fragment depth.
    let slice = cluster_slice(frame.clusters.x / max(in.clip_position.z, 1e-30));
    return (slice * CLUSTER_TILES_Y + tile.y) * CLUSTER_TILES_X + tile.x;
}

// Every listed local light, each split and toned like the sun (see the top of this file).
fn local_lighting(
    surface: CharacterSurface,
    list: u32,
    position: vec3<f32>,
    derivatives: CharacterDerivatives,
) -> vec3<f32> {
    let base = list * frame.lights.z;
    // Guards keep a list from reading past what was written this frame.
    let count = min(light_lists[base], frame.lights.w);
    // Light reaching the lit and the shadow side of each light's own split.
    var lit = vec3<f32>(0.0);
    var shaded = vec3<f32>(0.0);
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
        let radiance = light.radiance.rgb * (window * light_cone(light, l));
        let split = local_light_split(surface, derivatives, l, distance, dot(surface.normal, l));
        lit += radiance * split;
        shaded += radiance * (1.0 - split);
    }
    let luminance = dot(lit + shaded, LUMINANCE);
    let ceiling = min(1.0, LOCAL_LIGHT_CEILING / max(luminance, 1e-6));
    return (surface.albedo * lit + surface.shade * shaded) * ceiling;
}

@fragment
fn fs_anime_character(in: CharacterVaryings) -> @location(0) vec4<f32> {
    let material = in.material;
    let modes = materials[material].modes;
    let swizzles = materials[material].swizzles;
    // Derivatives first, in uniform control flow: material slots are sampled on branches the
    // block decides, with explicit gradients, and the normal map's frame needs the position's.
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let position_dx = dpdx(in.world_position);
    let position_dy = dpdy(in.world_position);

    var base = materials[material].base_color;
    if material_has_texture(modes, CHARACTER_BASE_COLOR) {
        base *= sample_material(base_color_map, swizzles, CHARACTER_BASE_COLOR, uv);
    }
    var surface: CharacterSurface;
    surface.albedo = base.rgb;
    surface.normal = safe_normalize(in.world_normal);
    if material_has_texture(modes, CHARACTER_NORMAL) {
        let sampled = sample_material(normal_map, swizzles, CHARACTER_NORMAL, uv).xy;
        surface.normal = mapped_normal(surface.normal, position_dx, position_dy, uv, sampled);
    }
    surface.view_dir = safe_normalize(view.position.xyz - in.world_position);
    surface.occlusion = in.occlusion;
    var region = in.vertex_material_id;
    surface.specular_mask = 0.0;
    surface.shadow_bias = SHADOW_BIAS_NEUTRAL;
    surface.specular_threshold = 0.5;
    if material_has_texture(modes, CHARACTER_CONTROL) {
        let control = sample_material(control_map, swizzles, CHARACTER_CONTROL, uv);
        surface.specular_mask = control.r;
        surface.shadow_bias = control.g;
        surface.specular_threshold = control.b;
        region = control_material_id(swizzles, in.uv);
    }
    // Regions past the material's count are zero; validated materials have at least one.
    surface.region = materials[material].regions[min(region, max(modes.y, 1u) - 1u)];
    surface.split = materials[material].split.xy;
    surface.rim = materials[material].edges.xy;

    let n_dot_l = dot(surface.normal, frame.sun_direction.xyz);
    let x = split_input(n_dot_l, surface.occlusion * cast_shadow(in), surface.shadow_bias);
    let width = max(surface.split.y, 0.5 * fwidth(x));
    let lit = bias_locks(light_step(surface.split.x, width, x), surface.shadow_bias);
    let rim = rim_light(surface, lit);
    // Taken here for the local lights: after the alpha test below, a discarded pixel's
    // neighbours could no longer rely on them.
    var derivatives: CharacterDerivatives;
    if frame.lights.y != LIGHT_LISTS_NONE {
        derivatives.normal_dx = dpdx(surface.normal);
        derivatives.normal_dy = dpdy(surface.normal);
        derivatives.position_dx = position_dx;
        derivatives.position_dy = position_dy;
    }
    if PERMUTATION_ALPHA_TEST && base.a < materials[material].split.w {
        discard;
    }

    surface.shade = surface.albedo * shade_tint(surface, material, x, width);
    let diffuse = mix(surface.shade, surface.albedo, lit) * frame.sun_radiance.rgb;
    let ambient = surface.albedo * sky_color(surface.normal) * AMBIENT_STRENGTH * surface.occlusion;
    var color = diffuse + ambient + rim;
    // A uniform branch: frames without local lights skip them.
    if frame.lights.y != LIGHT_LISTS_NONE {
        color += local_lighting(surface, pixel_light_list(in), in.world_position, derivatives);
    }
    // Opaque passes keep alpha 1 for the character bloom mask (C7); transparent ones blend by
    // their coverage.
    return vec4<f32>(color, select(1.0, base.a, PERMUTATION_TRANSPARENT));
}
