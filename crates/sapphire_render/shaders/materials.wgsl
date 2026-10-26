// Material blocks (M1 A3): one struct per material family, each the exact mirror of its
// `sapphire_assets::material::*Params` (the layout test in `shaders.rs` checks sizes against
// naga). Each family's parameter buffer is an array of its own block, bound as group 2 binding
// 1 by that family's pipelines; the anime character family's is `materials` in frame.wgsl.
// `shaders.rs` puts this file after frame.wgsl in every scene pass.
//
// Every block has `modes` (x: bit `i` set when texture slot `i` binds a texture, then the
// family's flags) and `swizzles` (each slot's swizzle code, two to a word). A family's textures
// are group 3: slot `i` at binding `i`, the trilinear repeating sampler at 8, the clamped ramp
// sampler at 9 and the nearest level-0 sampler for material IDs at 10
// (`pipelines::material_binding`).

// --- Anime character (`AnimeCharacterParams`): the anime character shader and its outlines.

struct AnimeCharacterRegion {
    // rgb: first shade (band mode), a multiplier on albedo; a: daylight ramp row.
    shade: vec4<f32>,
    // rgb: deep shade (three bands); a: night ramp row.
    deep_shade: vec4<f32>,
    // rgb: the subsurface tint ramp rows mix with (ramp mode); a: the character light rig's GI
    // sphere weight (`RegionGi::sphere`).
    subsurface: vec4<f32>,
    // rgb: specular color × intensity; a: size.
    specular: vec4<f32>,
    // rgb: rim tint; a: the GI's skin weight (`RegionGi::skin`).
    rim: vec4<f32>,
    // rgb: outline color; a: outline width scale.
    outline: vec4<f32>,
}

struct AnimeCharacterParams {
    // Linear RGBA; multiplies the base color map.
    base_color: vec4<f32>,
    // x: light-split threshold on N·L; y: split softness; z: deep shade threshold; w: alpha cutoff.
    split: vec4<f32>,
    // x: rim width in pixels at a 1080-pixel-high target at the outline curve's mid distance
    // (M1 C5); y: rim strength; z: outline width, likewise; w: outline Z offset in meters.
    edges: vec4<f32>,
    // x: ramp mix; y: specular softness; z, w: matcap strength lit and in shadow.
    terms: vec4<f32>,
    // Face: x shadow softness, y nose line threshold, z highlight strength, w highlight reveal.
    // Hair: x highlight strength, y width, z shift. Eye: x parallax.
    surface: vec4<f32>,
    // Face: rgb blush color. Hair: rgb highlight color.
    surface_color: vec4<f32>,
    // x: flags; y: region count; z: surface (CHARACTER_SURFACE_*); w: specular model | highlight
    // style << 8.
    modes: vec4<u32>,
    swizzles: vec4<u32>,
    // Picked per pixel by the control map's alpha, read unfiltered so IDs never blend across
    // region borders, or by the vertex's material ID without a control map. Regions from
    // `modes.y` on are zero, so the shaders clamp the ID below `modes.y`.
    regions: array<AnimeCharacterRegion, 8>,
    // Outlines v2 (M1 C4; `OutlineCurve`): the width's distance curve in effective meters, x near,
    // y mid, z far, w the LOD cutoff.
    outline_curve: vec4<f32>,
    // x: the curve's near scale; y: its far scale; z: model scale; w: view-space flattening.
    outline_shape: vec4<f32>,
    // x: how much of the albedo the line takes; y: how much it is lit.
    outline_color: vec4<f32>,
    // The depth-offset rim (M1 C5; `Rim`): x the depth step in meters; y the Fresnel extent; z
    // the light gate; w the back-light weighting.
    rim: vec4<f32>,
    // A face's hair shadow (`HairShadow`): x distance in meters; y strength.
    hair_shadow: vec4<f32>,
    // Eyes through hair (`ThroughHair`): x opacity (0 for none); y, z the cosines of the yaw and
    // pitch at which they have faded; w how far behind the hair, in meters.
    through_hair: vec4<f32>,
    // Per region: x rim strength scale; y rim width scale; z rim where the character overlaps
    // itself.
    rim_regions: array<vec4<f32>, 8>,
}

// Texture slots (`AnimeCharacterMaterial::TEXTURE_SLOTS`).
const CHARACTER_BASE_COLOR: u32 = 0u;
const CHARACTER_CONTROL: u32 = 1u;
const CHARACTER_NORMAL: u32 = 2u;
const CHARACTER_RAMP: u32 = 3u;
const CHARACTER_FACE_SHADOW: u32 = 4u;
const CHARACTER_SURFACE_MASK: u32 = 5u;
const CHARACTER_MATCAP: u32 = 6u;
const CHARACTER_EXPRESSION: u32 = 7u;

// Flags in `modes.x` above the slot bits.
const CHARACTER_FLAG_RAMP: u32 = 256u;
const CHARACTER_FLAG_DEEP_SHADE: u32 = 512u;
const CHARACTER_FLAG_ALPHA_TEST: u32 = 1024u;
const CHARACTER_FLAG_TRANSPARENT: u32 = 2048u;

const CHARACTER_SURFACE_BODY: u32 = 0u;
const CHARACTER_SURFACE_FACE: u32 = 1u;
const CHARACTER_SURFACE_HAIR: u32 = 2u;
const CHARACTER_SURFACE_EYE: u32 = 3u;

// Hair highlight styles, in `modes.w`'s second byte.
const CHARACTER_HIGHLIGHT_PAINTED_BAND: u32 = 0u;
const CHARACTER_HIGHLIGHT_ANGEL_RING: u32 = 1u;
const CHARACTER_HIGHLIGHT_KAJIYA_KAY: u32 = 2u;

// --- Environment (`EnvironmentParams`): stylized GGX, the environment shader (M1 W4).

struct EnvironmentParams {
    // Linear RGBA; multiplies the base color map.
    base_color: vec4<f32>,
    // x: roughness; y: metallic; z: normal strength; w: specular clamp (a multiple of the
    // light's radiance).
    surface: vec4<f32>,
    // x, y: roughness range; z: alpha cutoff; w: diffuse wrap.
    shaping: vec4<f32>,
    // x: terminator half-width up close, in N·L; y: its growth per meter; z: band strength (0
    // without a band).
    terminator: vec4<f32>,
    // rgb: multiplies the frame's shadow tint.
    shadow_tint: vec4<f32>,
    // x: start; y: end (meters); z: desaturation at the end.
    distance: vec4<f32>,
    // rgb: emission × intensity; multiplies the emission map.
    emission: vec4<f32>,
    // x: bound slots, then ENVIRONMENT_FLAG_*.
    modes: vec4<u32>,
    swizzles: vec4<u32>,
}

// Texture slots (`EnvironmentMaterial::TEXTURE_SLOTS`).
const ENVIRONMENT_BASE_COLOR: u32 = 0u;
const ENVIRONMENT_NORMAL: u32 = 1u;
const ENVIRONMENT_MASK: u32 = 2u;
const ENVIRONMENT_TERMINATOR_BAND: u32 = 3u;
const ENVIRONMENT_EMISSION: u32 = 4u;

// Flags in `modes.x` above the slot bits.
const ENVIRONMENT_FLAG_ALPHA_TEST: u32 = 256u;
const ENVIRONMENT_FLAG_TRANSPARENT: u32 = 512u;

// --- Foliage (`FoliageParams`): NPR leaves and grass, W6's shader.

struct FoliageParams {
    base_color: vec4<f32>,
    // x: alpha cutoff; y: wrap; z: terminator width; w: canopy occlusion.
    shading: vec4<f32>,
    shadow_tint: vec4<f32>,
    // rgb: translucency color × strength; a: power.
    translucency: vec4<f32>,
    gradient_root: vec4<f32>,
    gradient_tip: vec4<f32>,
    // x: sway; y: bend; z: flutter; w: flutter frequency.
    wind: vec4<f32>,
    distance: vec4<f32>,
    modes: vec4<u32>,
    swizzles: vec4<u32>,
}

// --- Sky (`SkyParams`): W2's shader. Time of day supplies the sky's colors.

struct SkyParams {
    // x: sun size (radians); y: sun softness; z: moon size; w: moon softness.
    discs: vec4<f32>,
    // x: star twinkle.
    stars: vec4<f32>,
    // x: scale; y: dissolve softness; zw: scroll.
    clouds: vec4<f32>,
    cirrus: vec4<f32>,
    modes: vec4<u32>,
    swizzles: vec4<u32>,
}

// --- Fog card (`FogCardParams`): W3's shader.

struct FogCardParams {
    // rgb: tint (× the time of day's card tint); a: opacity.
    color: vec4<f32>,
    // x: depth fade; y, z: camera fade start and end; w: sun scatter.
    fade: vec4<f32>,
    // xy: scroll.
    scroll: vec4<f32>,
    modes: vec4<u32>,
    swizzles: vec4<u32>,
}

// --- Every family.

// Whether texture slot `slot` binds a texture, from a block's `modes`. Unbound slots hold a
// white 1×1 fallback.
fn material_has_texture(modes: vec4<u32>, slot: u32) -> bool {
    return (modes.x & (1u << slot)) != 0u;
}

// The logical channels of a texel sampled from texture slot `slot`: A2's swizzle, by the code
// in the block's `swizzles` (3 bits per component naming its source: 0–3 R, G, B, A, 4 zero,
// 5 one; see `sapphire_assets::material::swizzle_code`). ASTC stores two-channel data as
// luminance plus alpha, so without this its normal maps and masks would read (x, x, x, y).
fn material_swizzle(sampled: vec4<f32>, swizzles: vec4<u32>, slot: u32) -> vec4<f32> {
    let code = (swizzles[slot / 2u] >> (12u * (slot % 2u))) & 0xfffu;
    var logical: vec4<f32>;
    for (var component = 0u; component < 4u; component++) {
        let source = (code >> (3u * component)) & 7u;
        logical[component] = select(f32(source == 5u), sampled[min(source, 3u)], source < 4u);
    }
    return logical;
}

// A UV-mapped sample's coordinates and their screen derivatives. The derivatives are taken once,
// in uniform control flow, so slots can be sampled inside branches on the block: WGSL counts
// those as non-uniform (the block is indexed by a varying), so implicit-derivative sampling and
// `fwidth` aren't allowed in them, while `textureSampleGrad` is.
struct MaterialUv {
    uv: vec2<f32>,
    dx: vec2<f32>,
    dy: vec2<f32>,
}
