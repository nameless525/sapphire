// Pass 1: environment surfaces (M1 W4, spec §5.12 "Environment"). One uber-shader for the
// world: a stylized GGX, painted rather than physical, and softer and less contrasty than the
// anime characters drawn over it, so the characters read on top (look pillar 4: soft ambient
// light and tinted shadows; world_lighting_post.md §2).
//
// Materials are the environment family (`EnvironmentParams` in materials.wgsl, bound as
// `environment_materials`). Per pixel the shader reads:
// - the base color map (albedo with painted gradients, and coverage for the alpha-tested and
//   transparent permutations);
// - the normal map, turned into world space by a frame from screen derivatives (meshes carry no
//   tangents yet), its tilt scaled by the material's normal strength;
// - the mask map: R ambient occlusion, which darkens the ambient light (its diffuse and its
//   reflection), G cavity, which darkens the ambient light and every highlight (never the key
//   light's diffuse, which would read dirty), B the specular mask, A a roughness multiplier;
// - the terminator band, a ramp across the light/shadow transition;
// - the emission map.
// The vertex color's R is ambient occlusion too, as on characters.
//
// The key light is the world's (`frame.sun_*`: the sun by day, the moon at night, as time of
// day blends them), never a character's light rig. Per pixel:
// - **The terminator.** N·L, pushed toward the deepest shadow in half-Lambert space by the cast
//   shadow (`cast_shadow`, `occluded`), passes through `smoothstep(-w, w, ·)`: 0 on the shadow
//   side, 1 on the lit side. The half-width `w` is the material's up close plus its growth per
//   meter of view distance, so far shading softens into painted masses (Blue Protocol's
//   distance-dependent shadow edges), and never less than half a pixel's change of N·L, so a
//   near-zero width is a clean two-tone step for props that should read like characters. The
//   terminator sits at N·L = 0, where cast shadows begin, so the two always meet.
// - **Wrap lighting.** The lit side falls off as wrapped Lambert, `(N·L + wrap) / (1 + wrap)`:
//   0 is Lambert's falloff, 0.3–0.5 keeps a third of the light as the surface turns away, a soft
//   painted gradient that meets the shadow side's tone at the terminator in a visible, colored
//   step instead of fading to nothing.
// - **The shadow tint.** Inside the terminator and in cast shadow the key light keeps the
//   frame's shadow tint (time of day's, `frame.shadow_tint`) times the material's, a dim
//   sky-ward color: shadows are colored, never black. The key light's diffuse is
//   `key × mix(keep, 1, lit × falloff)`, so shade and light meet without a dark seam.
// - **The terminator band.** A ramp across the transition, sampled at the terminator's value
//   (0 shadow side, 1 lit side) and blended in by the band's strength, multiplies the key
//   light's diffuse: a saturated, slightly warm band along the boundary, as anime backgrounds
//   paint it. Authored ramps end in white, so only the transition takes it.
// - **Ambient light** is the frame's sky-and-ground hemisphere (`ambient_light`, time of day's,
//   shared with the characters' GI), occluded by vertex AO, the mask's AO and its cavity.
// - **Specular** is GGX with the roughness remapped into the material's range (WuWa keeps
//   reflective surfaces under 0.4) and each light's highlight held under the material's
//   specular clamp, a multiple of the light's radiance. Reflectance is 4% for dielectrics and
//   the albedo for metals, times the mask's specular mask, which takes the grazing reflection
//   down with it, so a masked texel is matte on every tier. The ambient hemisphere also reflects,
//   seen along the reflection vector through Karis's analytic environment BRDF, so metals and
//   glossy surfaces keep a sky-colored sheen until reflection probes and the sky cube's
//   prefiltered mips arrive (W5). Per tier (`ENVIRONMENT_SPECULAR`, `EnvironmentSpecular`):
//   - full (desktop): GGX distribution, height-correlated Smith visibility, Schlick Fresnel;
//   - approximate (Mobile-High): the GGX distribution with visibility and Fresnel folded into
//     `1 / (4 · max(L·H², 0.1) · (α + 0.5))` at the reflectance (Zioma, "Optimizing PBR for
//     Mobile", SIGGRAPH 2015, as Unity's mobile pipeline ships it);
//   - off (Mobile-Low): no specular at all; metals shade as diffuse in their own color.
// - **Distance desaturation.** The albedo loses saturation (toward its own luminance) from the
//   material's start distance to its end, by its desaturation amount, eased: distant surfaces
//   read as painted color masses while the frame's light keeps its time of day hue. Emission
//   keeps its color.
// - **Emission** (color × map) adds HDR light unaffected by lighting, so it blooms.
//
// Local lights (M1 R5) come from the same lists the anime character shader walks (the pixel's
// cluster on desktop, its instance's list on mobile; `light_list`). Each lights the diffuse
// through the same terminator (its edge at least a pixel's change of its own N·L) and wrapped
// falloff, without a shadow tint (local lights keep nothing in shadow), and adds its own
// highlight. Their sum isn't held under a ceiling as characters' is: the world takes light as
// it comes, and a lamp's pool may outshine its surroundings.
//
// Coordinates follow the frame's conventions (frame.wgsl): lighting is linear HDR, and Lambert
// carries no 1/π (a unit key light on a surface facing it gives its albedo), so the specular
// lobes carry the matching π.
//
// Static permutations are pipeline-overridable constants (`pipelines::SurfaceVariant`):
// alpha-tested materials discard below their cutoff, transparent ones write their coverage for
// blending, and the tier picks the specular model. The environment draws no outline hulls in
// M1: its edges are the spec's screen-space edge pass (a desktop option, later); props that
// should read like characters use the anime character family.
//
// Wetness (later): the frame carries the weather's wetness and puddle coverage
// (`frame.weather`, `FramePacket::weather`) for the wet surface model that arrives with weather
// (Lagarde's: porous albedo darkened, roughness lowered toward 0.05–0.1, puddles filled to a flat
// smooth normal by a mask and height threshold, rain ripples as an animated normal atlas). They
// will apply to the surface below, before any light is computed; nothing reads them yet.

// The specular models, by tier (`EnvironmentSpecular`).
const ENVIRONMENT_SPECULAR_OFF: u32 = 0u;
const ENVIRONMENT_SPECULAR_APPROXIMATE: u32 = 1u;
const ENVIRONMENT_SPECULAR_FULL: u32 = 2u;

const PI: f32 = 3.14159265358979;
// Dielectrics' reflectance at normal incidence (glTF's 4%).
const DIELECTRIC_F0: f32 = 0.04;
// The smoothest a surface gets, so GGX's peak stays finite (Filament's floor at full precision).
const MIN_ROUGHNESS: f32 = 0.045;

// Static permutation bits and the tier's specular model (`pipelines::SurfaceVariant`,
// `environment::EnvironmentSpecular`), set per pipeline.
override PERMUTATION_ALPHA_TEST: bool = false;
override PERMUTATION_TRANSPARENT: bool = false;
override ENVIRONMENT_SPECULAR: u32 = 2u;

// The frame's light lists: a list per cluster or per instance slot (see lights.wgsl).
@group(0) @binding(2) var<storage, read> light_lists: array<u32>;
// The environment family's blocks, by the draw record's material slot (the scene group of the
// environment pipelines; frame.wgsl's `materials` is the anime character family's).
@group(2) @binding(1) var<storage, read> environment_materials: array<EnvironmentParams>;

// The environment family's group 3 (`EnvironmentMaterial::TEXTURE_SLOTS`): slot `i` at binding
// `i`, each unbound one a white 1×1 fallback.
@group(3) @binding(0) var base_color_map: texture_2d<f32>;
@group(3) @binding(1) var normal_map: texture_2d<f32>;
@group(3) @binding(2) var mask_map: texture_2d<f32>;
@group(3) @binding(3) var terminator_band_map: texture_2d<f32>;
@group(3) @binding(4) var emission_map: texture_2d<f32>;
@group(3) @binding(8) var material_sampler: sampler;
@group(3) @binding(9) var ramp_sampler: sampler;

struct EnvironmentVaryings {
    // Invariant, so the scene prepass (prepass.wgsl) computes the same depth.
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
}

@vertex
fn vs_environment(packed: PackedVertex, draw: DrawRecord) -> EnvironmentVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(vertex.position, 1.0);
    var out: EnvironmentVaryings;
    out.clip_position = view.clip_from_world * world_position;
    out.world_position = world_position.xyz;
    out.world_normal = instance.normal_from_object * vertex.normal;
    out.uv = vertex.uv;
    out.occlusion = vertex.color.r;
    out.material = draw.material;
    out.instance = draw.instance;
    return out;
}

// Everything the lighting stages need about one shaded point.
struct EnvironmentSurface {
    // What diffuse light multiplies: the albedo, less what metals reflect instead.
    diffuse: vec3<f32>,
    // Reflectance at normal incidence, times the specular mask.
    f0: vec3<f32>,
    normal: vec3<f32>,
    // Unit vector toward the camera.
    view_dir: vec3<f32>,
    // Perceptual roughness, remapped into the material's range.
    roughness: f32,
    // What the ambient light keeps: vertex AO × the mask's AO.
    occlusion: f32,
    // The mask's cavity, which the ambient light and every highlight keep.
    cavity: f32,
    // The lit side's wrapped falloff.
    wrap: f32,
    // The terminator's half-width at the point's distance, in N·L.
    terminator: f32,
    // The most specular light per light, as a multiple of its radiance.
    specular_clamp: f32,
}

// Texture slot `slot` at `uv`, through the slot's swizzle (`swizzles` from the block).
fn sample_material(map: texture_2d<f32>, swizzles: vec4<u32>, slot: u32, uv: MaterialUv) -> vec4<f32> {
    let sampled = textureSampleGrad(map, material_sampler, uv.uv, uv.dx, uv.dy);
    return material_swizzle(sampled, swizzles, slot);
}

// The world key light (the sun or the moon) reaching the point past cast shadows: 1 lit, 0
// shadowed, in between in a penumbra. It scales the terminator's input as ambient occlusion
// scales the characters' light split (`occluded`), so a cast shadow pushes the point onto the
// shadow side of the same terminator: the same remap, its width growing with distance, the same
// tinted keep, and no specular highlight. Shadows (R4, frame.wgsl's `shadow_visibility`): the
// environment receives every caster in the key light's cascades, which hold the environment's,
// and every character's own shadow (its map on desktop, its capsule on phones), sampled at the
// varyings' world position, offset along `world_normal` against acne. 1 with shadows off.
fn cast_shadow(in: EnvironmentVaryings) -> f32 {
    return shadow_visibility(in.world_position, safe_normalize(in.world_normal));
}

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

// GGX's normal distribution for `alpha2` (roughness⁴).
fn ggx_distribution(n_dot_h: f32, alpha2: f32) -> f32 {
    let d = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    return alpha2 / (PI * d * d);
}

// Height-correlated Smith-GGX visibility, the masking term over 4 N·L N·V (Heitz 2014, as
// Filament writes it).
fn smith_ggx_visibility(n_dot_v: f32, n_dot_l: f32, alpha2: f32) -> f32 {
    let view = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2);
    let light = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2);
    return 0.5 / max(view + light, 1e-6);
}

// The reflectance at grazing angles: 1, except where the specular mask takes the reflectance
// below about 2% (no real material reflects less), which it then takes down with it, so a masked
// texel reflects nothing at any angle on every tier (Filament's specular occlusion from f0).
fn grazing_reflectance(f0: vec3<f32>) -> f32 {
    return saturate(dot(f0, vec3<f32>(50.0 * 0.33)));
}

fn fresnel_schlick(f0: vec3<f32>, l_dot_h: f32) -> vec3<f32> {
    let f = pow(1.0 - l_dot_h, 5.0);
    return f0 + (vec3<f32>(grazing_reflectance(f0)) - f0) * f;
}

// The specular light a light toward the unit vector `l` reflects toward the camera, per unit of
// its radiance, held under the material's clamp: the tier's GGX (see the top of this file) × π
// × N·L. Zero with specular off.
fn specular(surface: EnvironmentSurface, l: vec3<f32>, n_dot_l: f32) -> vec3<f32> {
    if ENVIRONMENT_SPECULAR == ENVIRONMENT_SPECULAR_OFF {
        return vec3<f32>(0.0);
    }
    let h = safe_normalize(l + surface.view_dir);
    let n_dot_h = saturate(dot(surface.normal, h));
    let l_dot_h = saturate(dot(l, h));
    let alpha = surface.roughness * surface.roughness;
    let alpha2 = alpha * alpha;
    var reflected: vec3<f32>;
    if ENVIRONMENT_SPECULAR == ENVIRONMENT_SPECULAR_FULL {
        let n_dot_v = max(dot(surface.normal, surface.view_dir), 1e-4);
        let visibility = smith_ggx_visibility(n_dot_v, saturate(n_dot_l), alpha2);
        let fresnel = fresnel_schlick(surface.f0, l_dot_h);
        reflected = PI * ggx_distribution(n_dot_h, alpha2) * visibility * fresnel;
    } else {
        let folded = 1.0 / (4.0 * max(l_dot_h * l_dot_h, 0.1) * (alpha + 0.5));
        reflected = surface.f0 * (PI * ggx_distribution(n_dot_h, alpha2) * folded);
    }
    return min(reflected * saturate(n_dot_l), vec3<f32>(surface.specular_clamp)) * surface.cavity;
}

// The ambient hemisphere's light reflected toward the camera: the hemisphere seen along the
// reflection vector, through Karis's analytic fit of the split-sum environment BRDF
// ("Physically Based Shading on Mobile", 2014). Zero with specular off.
fn ambient_specular(surface: EnvironmentSurface) -> vec3<f32> {
    if ENVIRONMENT_SPECULAR == ENVIRONMENT_SPECULAR_OFF {
        return vec3<f32>(0.0);
    }
    let n_dot_v = saturate(dot(surface.normal, surface.view_dir));
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = surface.roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let scale_bias = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    let reflected = reflect(-surface.view_dir, surface.normal);
    // The fit's bias dips just under 0 for rough surfaces that reflect nothing.
    let brdf = max(surface.f0 * scale_bias.x + scale_bias.y * grazing_reflectance(surface.f0), vec3<f32>(0.0));
    return ambient_light(reflected) * brdf * surface.occlusion * surface.cavity;
}

// Screen-space derivatives of the shading point, taken once per pixel in uniform control flow:
// the local-light loop runs a different number of times per pixel, where `fwidth` is undefined.
struct EnvironmentDerivatives {
    normal_dx: vec3<f32>,
    normal_dy: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
}

// Every listed local light: the diffuse through the terminator and the wrapped falloff, and the
// highlight (see the top of this file).
fn local_lighting(
    surface: EnvironmentSurface,
    list: u32,
    position: vec3<f32>,
    derivatives: EnvironmentDerivatives,
) -> vec3<f32> {
    let base = list * frame.lights.z;
    // Guards keep a list from reading past what was written this frame.
    let count = min(light_lists[base], frame.lights.w);
    var diffuse = vec3<f32>(0.0);
    var specular_light = vec3<f32>(0.0);
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
        let n_dot_l = dot(surface.normal, l);
        // The exact first-order change of N·L across the pixel, d(N·L) = dN·L + N·dL, where
        // dL = ((L·dP) L − dP) / distance, so the edge is at least a pixel wide.
        let dx = dot(derivatives.normal_dx, l)
            + (n_dot_l * dot(l, derivatives.position_dx) - dot(surface.normal, derivatives.position_dx)) / distance;
        let dy = dot(derivatives.normal_dy, l)
            + (n_dot_l * dot(l, derivatives.position_dy) - dot(surface.normal, derivatives.position_dy)) / distance;
        let lit = light_step(0.0, max(surface.terminator, 0.5 * (abs(dx) + abs(dy))), n_dot_l);
        diffuse += radiance * (lit * wrapped(n_dot_l, surface.wrap));
        specular_light += radiance * specular(surface, l, n_dot_l) * lit;
    }
    return surface.diffuse * diffuse + specular_light;
}

@fragment
fn fs_environment(in: EnvironmentVaryings) -> @location(0) vec4<f32> {
    let material = in.material;
    let modes = environment_materials[material].modes;
    let swizzles = environment_materials[material].swizzles;
    // Derivatives first, in uniform control flow: material slots are sampled on branches the
    // block decides, with explicit gradients, and the normal map's frame needs the position's.
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let position_dx = dpdx(in.world_position);
    let position_dy = dpdy(in.world_position);

    var base = environment_materials[material].base_color;
    if material_has_texture(modes, ENVIRONMENT_BASE_COLOR) {
        base *= sample_material(base_color_map, swizzles, ENVIRONMENT_BASE_COLOR, uv);
    }
    let surface_params = environment_materials[material].surface;
    let shaping = environment_materials[material].shaping;
    var surface: EnvironmentSurface;
    surface.normal = safe_normalize(in.world_normal);
    if material_has_texture(modes, ENVIRONMENT_NORMAL) {
        let sampled = sample_material(normal_map, swizzles, ENVIRONMENT_NORMAL, uv).xy;
        let tilted = 0.5 + (sampled - 0.5) * surface_params.z;
        surface.normal = mapped_normal(surface.normal, position_dx, position_dy, uv, tilted);
    }
    var mask = vec4<f32>(1.0);
    if material_has_texture(modes, ENVIRONMENT_MASK) {
        mask = sample_material(mask_map, swizzles, ENVIRONMENT_MASK, uv);
    }
    let to_camera = view.position.xyz - in.world_position;
    let distance = length(to_camera);
    surface.view_dir = to_camera / max(distance, 1e-6);
    surface.roughness =
        max(mix(shaping.x, shaping.y, saturate(surface_params.x * mask.a)), MIN_ROUGHNESS);
    surface.occlusion = in.occlusion * mask.r;
    surface.cavity = mask.g;
    surface.wrap = shaping.w;
    let terminator = environment_materials[material].terminator;
    surface.terminator = terminator.x + terminator.y * distance;
    surface.specular_clamp = surface_params.w;

    // Distance desaturation, on the albedo alone: the frame's light keeps its hue.
    let stylization = environment_materials[material].distance;
    let desaturation = stylization.z * distance_amount(distance, stylization);
    let albedo = mix(base.rgb, vec3<f32>(dot(base.rgb, LUMINANCE)), desaturation);
    if ENVIRONMENT_SPECULAR == ENVIRONMENT_SPECULAR_OFF {
        // Metals shade as diffuse in their own color.
        surface.diffuse = albedo;
        surface.f0 = vec3<f32>(0.0);
    } else {
        let metallic = surface_params.y;
        surface.diffuse = albedo * (1.0 - metallic);
        surface.f0 = mix(vec3<f32>(DIELECTRIC_F0), albedo, metallic) * mask.b;
    }

    // The key light: the world's, through the terminator, with its shadow tint.
    let key_direction = frame.sun_direction.xyz;
    let key_radiance = frame.sun_radiance.rgb;
    let n_dot_l = dot(surface.normal, key_direction);
    let x = occluded(n_dot_l, cast_shadow(in));
    let lit = light_step(0.0, max(surface.terminator, 0.5 * fwidth(x)), x);
    // Taken here for the local lights: after the alpha test below, a discarded pixel's
    // neighbours could no longer rely on them.
    var derivatives: EnvironmentDerivatives;
    if frame.lights.y != LIGHT_LISTS_NONE {
        derivatives.normal_dx = dpdx(surface.normal);
        derivatives.normal_dy = dpdy(surface.normal);
        derivatives.position_dx = position_dx;
        derivatives.position_dy = position_dy;
    }
    if PERMUTATION_ALPHA_TEST && base.a < shaping.z {
        discard;
    }

    var band = vec3<f32>(1.0);
    if material_has_texture(modes, ENVIRONMENT_TERMINATOR_BAND) {
        // One row, read at the terminator's value through the clamped ramp sampler.
        let sampled = textureSampleLevel(terminator_band_map, ramp_sampler, vec2<f32>(lit, 0.5), 0.0);
        let ramp = material_swizzle(sampled, swizzles, ENVIRONMENT_TERMINATOR_BAND).rgb;
        band = mix(vec3<f32>(1.0), ramp, terminator.z);
    }
    let keep = frame.shadow_tint.rgb * environment_materials[material].shadow_tint.rgb;
    let key_light = key_radiance * mix(keep, vec3<f32>(1.0), lit * wrapped(n_dot_l, surface.wrap)) * band;
    let ambient = ambient_light(surface.normal) * (surface.occlusion * surface.cavity);
    var color = surface.diffuse * (key_light + ambient)
        + key_radiance * specular(surface, key_direction, n_dot_l) * lit + ambient_specular(surface);
    // A uniform branch: frames without local lights skip them.
    if frame.lights.y != LIGHT_LISTS_NONE {
        let list = light_list(in.clip_position, in.instance);
        color += local_lighting(surface, list, in.world_position, derivatives);
    }
    var emission = environment_materials[material].emission.rgb;
    if material_has_texture(modes, ENVIRONMENT_EMISSION) {
        emission *= sample_material(emission_map, swizzles, ENVIRONMENT_EMISSION, uv).rgb;
    }
    // The surface's final color, lit and emitting: the one place it is produced, where fog (W3)
    // will join it through a function the world's passes share. Opaque passes keep alpha 1;
    // transparent ones blend by their coverage.
    let final_color = color + emission;
    return vec4<f32>(final_color, select(1.0, base.a, PERMUTATION_TRANSPARENT));
}
