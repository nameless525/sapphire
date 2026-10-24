// Pass 1: toon opaque. A two-tone light split with an authored shadow color, an N·V rim and
// a small sky ambient term.
//
// The fragment shader is split into the stages M1 extends:
// - `ToonSurface` gains the ILM mask sample (R: specular mask, G: shadow bias,
//   B: specular threshold, A: ramp row) and, for faces, head-space SDF inputs.
// - `toon_light_split` takes the ILM shadow bias, and the face permutation replaces N·L with
//   the SDF face-shadow test (mirrored for light from the other side).
// - `toon_shadow_color` becomes a ramp-texture lookup instead of a flat multiplier.

// Strength of the hemispherical sky ambient. Kept small so the two tones stay distinct.
const AMBIENT_STRENGTH: f32 = 0.12;
// Fraction of the N·V range, measured in from the silhouette, that receives rim light.
const RIM_WIDTH: f32 = 0.22;

struct ToonVaryings {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    // Unused until M1 samples ramp, ILM and base textures.
    @location(2) uv: vec2<f32>,
    // Linear vertex tint.
    @location(3) tint: vec3<f32>,
    @location(4) @interpolate(flat) material: u32,
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
    let width = max(softness, 0.5 * fwidth(x));
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
    return vec4<f32>(diffuse + ambient + rim, 1.0);
}
