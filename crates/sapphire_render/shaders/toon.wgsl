// Pass 1: toon opaque. A two-tone light split with an authored shadow color, an N·V rim, a
// small sky ambient term, and local point and spot lights in the same toon model.
//
// The fragment shader is split into the stages M1 extends:
// - `ToonSurface` gains the ILM mask sample (R: specular mask, G: shadow bias,
//   B: specular threshold, A: ramp row) and, for faces, head-space SDF inputs.
// - `toon_light_split` takes the ILM shadow bias, and the face permutation replaces N·L with
//   the SDF face-shadow test (mirrored for light from the other side). `toon_local_split` gives
//   local lights the same threshold, so the bias moves their terminators too.
// - `toon_shadow_color` becomes a ramp-texture lookup instead of a flat multiplier.
//
// Local lights (M1 R5) are lit exactly like the sun's key light: each light splits N·L at the
// material's threshold into a lit and a shadow tone, with the same authored shadow color and a
// one-pixel edge, in the light's color × its smooth range window × its spot cone. Where a light
// reaches, it pushes surfaces in the sun's shadow into its own lit tone with a crisp edge, and
// tints both tones, rather than adding a soft Lambert gradient. Their sum is held under
// LOCAL_LIGHT_CEILING, so stacked lights tint rather than blow out. Lights come from the
// frame's light lists (`lights.wgsl`): the pixel's cluster on desktop, its instance's own list
// on mobile. Local lights add no rim; the rim stays the key light's (C5 revisits it).

// Strength of the hemispherical sky ambient. Kept small so the two tones stay distinct.
const AMBIENT_STRENGTH: f32 = 0.12;
// Fraction of the N·V range, measured in from the silhouette, that receives rim light.
const RIM_WIDTH: f32 = 0.22;
// The most luminance local lights add together, in units of a unit sun. The character light
// rig (C2) makes it a rig parameter.
const LOCAL_LIGHT_CEILING: f32 = 2.0;
const LUMINANCE: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// The frame's light lists: a list per cluster or per instance slot (see lights.wgsl).
@group(0) @binding(2) var<storage, read> light_lists: array<u32>;

struct ToonVaryings {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    // Unused until M1 samples ramp, ILM and base textures.
    @location(2) uv: vec2<f32>,
    // Linear vertex tint.
    @location(3) tint: vec3<f32>,
    @location(4) @interpolate(flat) material: u32,
    // The instance's slot: its light list on the per-object path.
    @location(5) @interpolate(flat) instance: u32,
}

@vertex
fn vs_toon(vertex: MeshVertex, draw: DrawRecord) -> ToonVaryings {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(vertex.position, 1.0);
    var out: ToonVaryings;
    out.clip_position = view.clip_from_world * world_position;
    out.world_position = world_position.xyz;
    out.world_normal = instance.normal_from_object * vertex.normal;
    out.uv = vertex.uv;
    out.tint = srgb_to_linear(vertex.color.rgb);
    out.material = draw.material;
    out.instance = draw.instance;
    return out;
}

// Everything the lighting stages need about one shaded point.
struct ToonSurface {
    // Material base color × vertex tint.
    albedo: vec3<f32>,
    normal: vec3<f32>,
    // Unit vector toward the camera.
    view_dir: vec3<f32>,
    // Shifts the light-split threshold. 0 in M0; the ILM G channel in M1.
    shadow_bias: f32,
}

// A threshold step whose transition is about one pixel wide: as crisp as possible without
// aliasing, on curved and flat surfaces alike. MSAA doesn't help here; it only resolves edges.
fn crisp_step(edge: f32, softness: f32, x: f32) -> f32 {
    return toon_step(edge, max(softness, 0.5 * fwidth(x)), x);
}

// The split itself: 0 below `edge - width`, 1 above `edge + width`.
fn toon_step(edge: f32, width: f32, x: f32) -> f32 {
    return smoothstep(edge - width, edge + width, x);
}

// 1 on the lit side of the two-tone split, 0 on the shadow side. `params.y` widens the
// transition beyond one pixel when the material wants a softer terminator.
fn toon_light_split(surface: ToonSurface, material: ToonMaterial) -> f32 {
    let n_dot_l = dot(surface.normal, frame.sun_direction.xyz);
    let threshold = material.params.x + surface.shadow_bias;
    return crisp_step(threshold, material.params.y, n_dot_l);
}

// The shadow side is its own authored color, never a darkened copy of the lit side
// (spec §5.12 "Ramps").
fn toon_shadow_color(surface: ToonSurface, material: ToonMaterial) -> vec3<f32> {
    return surface.albedo * material.shade_color.rgb;
}

// N·V rim light: the mobile rim. Desktop gets a depth-difference rim once the prepass exists.
fn toon_rim(surface: ToonSurface, material: ToonMaterial, lit: f32) -> vec3<f32> {
    let fresnel = 1.0 - saturate(dot(surface.normal, surface.view_dir));
    let mask = crisp_step(1.0 - RIM_WIDTH, 0.0, fresnel);
    // Mostly on the lit side, so it reads as the key light wrapping around the silhouette.
    let strength = material.params.z * mask * mix(0.25, 1.0, lit);
    return frame.sun_radiance.rgb * mix(surface.albedo, vec3<f32>(1.0), 0.5) * strength;
}

// Screen-space derivatives of the shading point, taken once per pixel in uniform control flow:
// the local-light loop runs a different number of times per pixel, where `fwidth` is undefined.
struct ToonDerivatives {
    normal_dx: vec3<f32>,
    normal_dy: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
}

// `toon_light_split` for a local light toward the unit vector `l`, `distance` away. Its edge is
// the one-pixel width `crisp_step` gives the sun, from the exact first-order change of N·L
// across the pixel: d(N·L) = dN·L + N·dL, where dL = ((L·dP) L − dP) / distance.
fn toon_local_split(
    surface: ToonSurface,
    material: ToonMaterial,
    derivatives: ToonDerivatives,
    l: vec3<f32>,
    distance: f32,
    n_dot_l: f32,
) -> f32 {
    let dx = dot(derivatives.normal_dx, l)
        + (n_dot_l * dot(l, derivatives.position_dx) - dot(surface.normal, derivatives.position_dx)) / distance;
    let dy = dot(derivatives.normal_dy, l)
        + (n_dot_l * dot(l, derivatives.position_dy) - dot(surface.normal, derivatives.position_dy)) / distance;
    let width = max(material.params.y, 0.5 * (abs(dx) + abs(dy)));
    return toon_step(material.params.x + surface.shadow_bias, width, n_dot_l);
}

// The light list this pixel walks: its cluster's, or its instance's.
fn toon_light_list(in: ToonVaryings) -> u32 {
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
fn toon_local_lights(
    surface: ToonSurface,
    material: ToonMaterial,
    list: u32,
    position: vec3<f32>,
    derivatives: ToonDerivatives,
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
        let split = toon_local_split(surface, material, derivatives, l, distance, dot(surface.normal, l));
        lit += radiance * split;
        shaded += radiance * (1.0 - split);
    }
    let luminance = dot(lit + shaded, LUMINANCE);
    let ceiling = min(1.0, LOCAL_LIGHT_CEILING / max(luminance, 1e-6));
    return (surface.albedo * lit + toon_shadow_color(surface, material) * shaded) * ceiling;
}

@fragment
fn fs_toon(in: ToonVaryings) -> @location(0) vec4<f32> {
    let material = materials[in.material];
    var surface: ToonSurface;
    surface.albedo = material.base_color.rgb * in.tint;
    surface.normal = safe_normalize(in.world_normal);
    surface.view_dir = safe_normalize(view.position.xyz - in.world_position);
    surface.shadow_bias = 0.0;

    let lit = toon_light_split(surface, material);
    let diffuse = mix(toon_shadow_color(surface, material), surface.albedo, lit) * frame.sun_radiance.rgb;
    let ambient = surface.albedo * sky_color(surface.normal) * AMBIENT_STRENGTH;
    let rim = toon_rim(surface, material, lit);
    var color = diffuse + ambient + rim;
    // A uniform branch: frames without local lights skip them, derivatives included.
    if frame.lights.y != LIGHT_LISTS_NONE {
        var derivatives: ToonDerivatives;
        derivatives.normal_dx = dpdx(surface.normal);
        derivatives.normal_dy = dpdy(surface.normal);
        derivatives.position_dx = dpdx(in.world_position);
        derivatives.position_dy = dpdy(in.world_position);
        color += toon_local_lights(surface, material, toon_light_list(in), in.world_position, derivatives);
    }
    return vec4<f32>(color, 1.0);
}
