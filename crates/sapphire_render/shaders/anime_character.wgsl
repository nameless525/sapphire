// Pass 1: anime character surfaces (M1 C1, spec §5.12). One uber-shader: a thresholded light
// split with authored shadow colors under the character light rig's key light, an N·V rim, the
// rig's stylized GI, and local point and spot lights split the same way.
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
// The character light rig (M1 C2). Each instance reads its entry of the frame's characters
// (`frame.wgsl`), resolved on the CPU per character: its own key light (an art-directed
// direction, and time of day's character key for color and brightness), the rim's scale, the
// local lights' ceiling and the stylized GI's settings. Instances that aren't characters read
// entry 0, the world's key light with unstyled GI. The fragment shader reads the key light once,
// as `key_direction` and `key_radiance`, and every key-light term takes it from there.
//
// Stylized GI (`character_gi`, WuWa's character GI) stands in for plain ambient light: the
// frame's ambient hemisphere (time of day's) at a normal turned toward a sphere around the head
// by the region's sphere weight (faces, hair) and bent toward the camera, so it stays low
// frequency; then an HSV clamp of its saturation and value, the rig's skin tint by the region's
// skin weight, and vertex AO.
//
// Local lights (M1 R5) are lit like the key light: each light splits its own N·L at the same
// threshold (bias, locks and vertex AO included) into a lit and a shadow tone, the shadow tone
// being the surface's shadow color, in the light's color × its smooth range window × its spot
// cone, with a one-pixel edge. They keep their own directions: the rig's direction constraints
// are for the key light alone, and a lantern below a face lights it from below. Their sum is
// held under the rig's ceiling, so stacked lights tint rather than blow out. Lights come from the
// frame's light lists (`lights.wgsl`): the pixel's cluster on desktop, its instance's own list on
// mobile. Local lights add no rim; the rim stays the key light's (C5 revisits it).
//
// Static permutations (`pipelines::Permutation`) are pipeline-overridable constants, so each
// variant compiles only its own code: alpha-tested materials discard below their cutoff,
// transparent ones write their coverage for blending, and faces shade by their SDF map. Hair
// (C5) and eyes (C6) add theirs.
//
// Faces (M1 C3, the face permutation). The light split comes from the face's SDF map and the
// character's key light in head space instead of N·L, as `sapphire_assets::character_maps`
// ("Head space") defines it:
// - The head is the character's: its entry's face (`CharacterData::face` in frame.wgsl), the
//   head's forward and up in world space and its expression weights, read through the same
//   index as the rig. Instances that aren't characters read entry 0's, a head at rest.
// - The key light is the rig's, read once with the body's (`key_direction`, `key_radiance`):
//   face code reads no light of its own.
// - In head space (right = forward × up) the light has a horizontal angle (its yaw, faded as it
//   rises) and a vertical one (its elevation). The map is sampled at u and mirrored at 1 - u:
//   R on the light's side (blending toward the darker side near the head's forward–up plane),
//   G always unmirrored; the texel's margin is min(R - horizontal, G - vertical), lit while
//   positive. A one-channel map's G reads 1.
// - The margin becomes the split's input as the N·L of a sphere whose terminator follows the
//   face map (`face_n_dot_l`), so the body's occlusion, shadow bias and locks, threshold and
//   diffuse mode (`shade_tint`) apply as they do to N·L, and the face's shadow tones are the
//   body's. So is the rig's stylized GI, which spherizes the face by its regions' sphere weight.
//   Faces never take the cast shadow (`cast_shadow`): shadow maps are too coarse at face scale,
//   so a face's shadow is the SDF's alone.
// - Masks (`character_maps::face_mask`, `face_expression`): always-lit eyes and brows; a face
//   highlight that brightens the lit tone away from the shadow's edge; a nose line in the
//   region's outline color that appears as the head turns from the camera; blush tinting the
//   albedo and gloom pushing toward the deepest shadow as occlusion does (its GI too, except
//   where always lit), each scaled by the character's expression weight.

// The control map's shadow bias (`character_maps::ilm`): neutral, and the values at and beyond
// which a point stays lit or in shadow whatever the light.
const SHADOW_BIAS_NEUTRAL: f32 = 0.5;
const SHADOW_BIAS_LIT: f32 = 0.95;
const SHADOW_BIAS_SHADOWED: f32 = 0.05;
// Ramp rows (`character_maps::ramp::ROWS`): each region's day row, then its night row.
const RAMP_ROWS: f32 = 16.0;
// The control map's material IDs, k stored as k / (MATERIAL_ID_LEVELS - 1).
const MATERIAL_ID_LEVELS: f32 = 8.0;

const PI: f32 = 3.14159265358979;
// `character_maps::face_sdf::MIRROR_BLEND`: within this × |L·U| of the head's forward–up plane,
// in |L·R|, the horizontal channel blends toward the darker of both sides.
const FACE_MIRROR_BLEND: f32 = 0.25;
// The narrowest face highlight reveal and nose line edge, in their own units.
const MIN_FACE_EDGE: f32 = 1e-4;

// Static permutation bits (`pipelines::Permutation`), set per pipeline.
override PERMUTATION_ALPHA_TEST: bool = false;
override PERMUTATION_TRANSPARENT: bool = false;
override PERMUTATION_FACE: bool = false;

// The frame's light lists: a list per cluster or per instance slot (see lights.wgsl).
@group(0) @binding(2) var<storage, read> light_lists: array<u32>;

// The anime character family's group 3 (`AnimeCharacterMaterial::TEXTURE_SLOTS`): slot `i` at
// binding `i`, each unbound one a white 1×1 fallback. Matcap reads join with C7.
@group(3) @binding(0) var base_color_map: texture_2d<f32>;
@group(3) @binding(1) var control_map: texture_2d<f32>;
@group(3) @binding(2) var normal_map: texture_2d<f32>;
@group(3) @binding(3) var ramp_map: texture_2d<f32>;
@group(3) @binding(4) var face_shadow_map: texture_2d<f32>;
@group(3) @binding(5) var surface_mask_map: texture_2d<f32>;
@group(3) @binding(6) var matcap_map: texture_2d<f32>;
@group(3) @binding(7) var expression_map: texture_2d<f32>;
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
    // The instance's entry in the frame's characters: its light rig.
    @location(7) @interpolate(flat) character: u32,
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
    out.character = instance.character;
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

// The world key light (the sun or the moon) reaching the point past cast shadows: 1 lit, 0
// shadowed. It scales the light split's input as vertex occlusion does, so a cast shadow pushes
// the point into the authored shadow tones instead of darkening them. Shadows (R4) fill it from
// the cascades, which hold environment casters only: characters never self-shadow, and the
// rig's key direction, which may differ from the sun's, shades but never casts. Until then
// nothing casts. The face permutation never calls it: a face's shadow is its SDF map's alone,
// so R4 adds no shadow-map reads to faces; an environment shadow reaches a face whole, per
// character (a value in its entry that the face takes as occlusion).
fn cast_shadow(in: CharacterVaryings) -> f32 {
    return 1.0;
}

// How far the face map lights the point for a light toward `toward_light`, in threshold units
// (light angle / 180°), positive while lit: `character_maps::face_sdf::lit_margin` for the light
// in head space (`HeadLight::new`). Two taps of the face map: at uv, and mirrored.
fn face_margin(face: CharacterFace, toward_light: vec3<f32>, swizzles: vec4<u32>, uv: MaterialUv) -> f32 {
    let forward = face.forward.xyz;
    let up = face.up.xyz;
    // The character's own right: models face +Z with +Y up, so -X at rest.
    let right = cross(forward, up);
    let l = vec3<f32>(dot(toward_light, right), dot(toward_light, up), dot(toward_light, forward));
    let level = length(l.xz);
    // Straight above or below, the yaw is undefined and fades out anyway.
    let yaw = select(0.0, atan2(abs(l.x), l.z), level > 1e-6);
    let horizontal = yaw * level / PI;
    let vertical = asin(saturate(l.y)) / PI;
    let side = min(abs(l.x) / max(FACE_MIRROR_BLEND * abs(l.y), 1e-6), 1.0);

    let flip = vec2<f32>(-1.0, 1.0);
    let mirrored_uv = MaterialUv(vec2<f32>(1.0 - uv.uv.x, uv.uv.y), uv.dx * flip, uv.dy * flip);
    let at_uv = sample_material(face_shadow_map, swizzles, CHARACTER_FACE_SHADOW, uv);
    let at_mirror = sample_material(face_shadow_map, swizzles, CHARACTER_FACE_SHADOW, mirrored_uv).r;
    // Maps are authored for light on the head's right; light on its left reads them mirrored.
    let own = select(at_uv.r, at_mirror, l.x < 0.0);
    let both = min(at_uv.r, at_mirror);
    return min(mix(both, own, side) - horizontal, at_uv.g - vertical);
}

// The face's N·L: that of a sphere whose terminator follows the face map, turned so the map's
// edge (margin 0) falls on the material's `threshold`. A quarter turn of light past the edge,
// plus the threshold's own angle, is the deepest shadow (-1), as on the sphere. The body's
// occlusion, bias, locks and diffuse mode then apply to it as they do to N·L.
fn face_n_dot_l(margin: f32, threshold: f32) -> f32 {
    let offset = asin(clamp(threshold, -1.0, 1.0));
    return sin(clamp(PI * margin + offset, -0.5 * PI, 0.5 * PI));
}

// The face's light split edge in N·L units: the material's face softness (threshold units) at
// the edge, where face_n_dot_l changes by π·cos(asin(threshold)) per threshold unit.
fn face_softness(softness: f32, threshold: f32) -> f32 {
    let t = clamp(threshold, -1.0, 1.0);
    return PI * sqrt(1.0 - t * t) * softness;
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

// N·V rim light in the key light's color, scaled by the rig: the mobile rim. Desktop gets a
// depth-difference rim once the prepass exists.
fn rim_light(surface: CharacterSurface, key_radiance: vec3<f32>, rig_scale: f32, lit: f32) -> vec3<f32> {
    let fresnel = 1.0 - saturate(dot(surface.normal, surface.view_dir));
    let mask = light_step(1.0 - surface.rim.x, 0.5 * fwidth(fresnel), fresnel);
    // Mostly on the lit side, so it reads as the key light wrapping around the silhouette.
    let strength = surface.rim.y * rig_scale * mask * mix(0.25, 1.0, lit);
    let tint = mix(surface.albedo, vec3<f32>(1.0), 0.5) * surface.region.rim.rgb;
    return key_radiance * tint * strength;
}

// Stylized character GI (see the top of this file): the light the GI gives the point, which
// its albedo then takes.
fn character_gi(surface: CharacterSurface, rig: CharacterRig, position: vec3<f32>) -> vec3<f32> {
    // Faces and hair shade like one smooth ball around the head, by the region's weight
    // (`RegionGi::sphere`, in the subsurface vector's spare channel), where the rig knows the head.
    let sphere = safe_normalize(position - rig.head.xyz);
    let spherized = safe_normalize(mix(surface.normal, sphere, surface.region.subsurface.a * rig.gi.w));
    // The whole character leans toward the camera, which flattens it.
    let normal = safe_normalize(spherized + rig.gi.x * surface.view_dir);
    let gi = hsv_clamp(ambient_light(normal) * rig.head.w, rig.gi.y, rig.gi.z);
    // Skin takes the rig's tint by the region's weight (`RegionGi::skin`, in the rim vector's
    // spare channel), so its shadow side never reads grey.
    let skin = mix(vec3<f32>(1.0), rig.skin_tint.rgb, surface.region.rim.a);
    return gi * skin * surface.occlusion;
}

// `color` with its HSV saturation held to `max_saturation` and its value (the brightest channel)
// to `max_value`, its hue kept: the channels move toward the value, then all scale down. A color
// within both comes back exactly.
fn hsv_clamp(color: vec3<f32>, max_saturation: f32, max_value: f32) -> vec3<f32> {
    let value = max(max(max(color.r, color.g), color.b), 1e-12);
    let saturation = 1.0 - min(min(color.r, color.g), color.b) / value;
    let kept = select(1.0, max_saturation / max(saturation, 1e-6), saturation > max_saturation);
    let held = mix(vec3<f32>(value), color, kept) * (min(value, max_value) / value);
    return select(color, held, saturation > max_saturation || value > max_value);
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

// Every listed local light, each split and toned like the key light, their sum held under
// `ceiling` (the rig's) in luminance (see the top of this file).
fn local_lighting(
    surface: CharacterSurface,
    ceiling: f32,
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
    let held = min(1.0, ceiling / max(luminance, 1e-6));
    return (surface.albedo * lit + surface.shade * shaded) * held;
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

    // The key light, from the character's rig: every key-light term reads it here, a face's
    // included.
    let rig = characters[in.character].rig;
    let key_direction = rig.key_direction.xyz;
    let key_radiance = rig.key_radiance.rgb;

    // A face's head, masks and expression (`character_maps::face_mask`, `face_expression`),
    // before anything reads the albedo or occlusion. Zero for the body.
    var face: CharacterFace;
    var nose_line = 0.0;
    var face_highlight = 0.0;
    var always_lit = 0.0;
    if PERMUTATION_FACE {
        face = characters[in.character].face;
        // The camera's turn from head-forward: its side picks the nose line's mirror (masks are
        // authored for a camera on the head's right), and its sine reveals the line.
        let turn = dot(surface.view_dir, cross(face.forward.xyz, face.up.xyz));
        if material_has_texture(modes, CHARACTER_SURFACE_MASK) {
            let mask = sample_material(surface_mask_map, swizzles, CHARACTER_SURFACE_MASK, uv);
            let flip = vec2<f32>(-1.0, 1.0);
            let mirrored_uv = MaterialUv(vec2<f32>(1.0 - uv.uv.x, uv.uv.y), uv.dx * flip, uv.dy * flip);
            let mirrored = sample_material(surface_mask_map, swizzles, CHARACTER_SURFACE_MASK, mirrored_uv).r;
            nose_line = select(mask.r, mirrored, turn < 0.0) * abs(turn);
            face_highlight = mask.g;
            always_lit = mask.b;
        }
        if material_has_texture(modes, CHARACTER_EXPRESSION) {
            let painted = sample_material(expression_map, swizzles, CHARACTER_EXPRESSION, uv).rg;
            let expression = painted * saturate(face.expression.xy);
            surface.albedo *= mix(vec3<f32>(1.0), materials[material].surface_color.rgb, expression.x);
            // Gloom darkens as occlusion does: toward the deepest of the authored shadow tones,
            // and out of the GI. Always-lit texels (eyes, brows) keep both.
            surface.occlusion *= 1.0 - expression.y * (1.0 - always_lit);
        }
    }
    // The nose line's edge, a pixel wide, in uniform control flow.
    let nose_line_width = max(0.5 * fwidth(nose_line), MIN_FACE_EDGE);

    var x: f32;
    var softness = surface.split.y;
    var face_lit_margin = 0.0;
    if PERMUTATION_FACE {
        // The face map alone shadows a face: no cast shadow.
        face_lit_margin = face_margin(face, key_direction, swizzles, uv);
        x = split_input(face_n_dot_l(face_lit_margin, surface.split.x), surface.occlusion, surface.shadow_bias);
        softness = face_softness(materials[material].surface.x, surface.split.x);
    } else {
        let n_dot_l = dot(surface.normal, key_direction);
        x = split_input(n_dot_l, surface.occlusion * cast_shadow(in), surface.shadow_bias);
    }
    let width = max(softness, 0.5 * fwidth(x));
    var lit = bias_locks(light_step(surface.split.x, width, x), surface.shadow_bias);
    if PERMUTATION_FACE {
        lit = max(lit, always_lit);
    }
    let rim = rim_light(surface, key_radiance, rig.key_direction.w, lit);
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
    let diffuse = mix(surface.shade, surface.albedo, lit) * key_radiance;
    let ambient = surface.albedo * character_gi(surface, rig, in.world_position);
    var color = diffuse + ambient + rim;
    // A uniform branch: frames without local lights skip them.
    if frame.lights.y != LIGHT_LISTS_NONE {
        let list = light_list(in.clip_position, in.instance);
        color += local_lighting(surface, rig.key_radiance.w, list, in.world_position, derivatives);
    }
    if PERMUTATION_FACE {
        // The face highlight brightens the lit tone, fading in from the shadow's edge over the
        // material's reveal (surface.w), by its strength (surface.z).
        let reveal = smoothstep(0.0, max(materials[material].surface.w, MIN_FACE_EDGE), face_lit_margin);
        let highlight = face_highlight * reveal * lit * materials[material].surface.z;
        color += surface.albedo * key_radiance * highlight;
        // The nose line is drawn, not lit: the region's outline color over everything, where
        // the mask × the turn passes the material's threshold (surface.y).
        let edge = max(materials[material].surface.y, nose_line_width);
        let line = smoothstep(edge - nose_line_width, edge + nose_line_width, nose_line);
        color = mix(color, surface.region.outline.rgb, line);
    }
    // Alpha is the world's share of the pixel, post's character mask (M1 P2): opaque passes write
    // 0 for characters (instances the frame lists, which read a light rig of their own) and 1
    // for everything else, as every other pass does; transparent ones blend by their coverage,
    // keeping the mask under them.
    let world = select(1.0, 0.0, in.character != 0u);
    return vec4<f32>(color, select(world, base.a, PERMUTATION_TRANSPARENT));
}
