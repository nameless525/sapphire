// Pass 1: anime character surfaces (M1 C1, spec §5.12). One uber-shader: a thresholded light
// split with authored shadow colors under the character light rig's key light, a depth-offset
// rim, the rig's stylized GI, and local point and spot lights split the same way.
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
// character's ambient hemisphere (its entry's `CharacterAmbient`: time of day's, or since M1 W5
// the probe grid's light around its body, low frequency) at a normal turned toward a sphere
// around the head
// by the region's sphere weight (faces, hair) and bent toward the camera, so it stays low
// frequency; then an HSV clamp of its saturation and value, the rig's skin tint by the region's
// skin weight, and vertex AO.
//
// The depth-offset rim (M1 C5; `Rim`), on characters' bodies, faces and hair: a thin line of
// light where the character ends against what lies behind it. Each pixel looks along its normal
// across the screen, as many pixels as the material's width (at 1080 rows, by the region's width
// scale, and like the outline's by its distance curve and model scale, so rim and line art keep
// their proportions at any distance and field of view; not by the outlines' LOD cutoff, so the
// rim stays past it and with outlines off), into the character prepass's depth
// (`character_depth`, frame.wgsl). Where what lies there is the material's threshold farther
// away, or no character is, a filtered comparison gives 1: the rim's depth step. Where the pixel
// there is the same character's (an arm over the torso), the region's overlap scales it, so a
// region can keep its rim to the outer silhouette. The Fresnel gate (the share of N·V in from the
// silhouette) and the region's strength scale it. That much is the same for every light
// (`rim_mask`); per light (`rim_light`) the light gate keeps it to the lit side by the
// material's amount (WuWa), and ZZZ's back-light weighting scales it by 1 − V·L. It takes the
// light's radiance, the rig's rim scale and a tint: half the albedo, half white, × the region's
// rim color. Unlisted instances, eyes and transparent surfaces take none.
//
// Local lights (M1 R5) are lit like the key light: each light splits its own N·L at the same
// threshold (bias, locks and vertex AO included) into a lit and a shadow tone, the shadow tone
// being the surface's shadow color, in the light's color × its smooth range window × its spot
// cone, with a one-pixel edge, and adds its rim through the same depth step (M1 C5). They keep
// their own directions: the rig's direction constraints are for the key light alone, and a
// lantern below a face lights it from below. Their sum is held under the rig's ceiling, so
// stacked lights tint rather than blow out. Lights come from the frame's light lists
// (`lights.wgsl`): the pixel's cluster on desktop, its instance's own list on mobile.
//
// Static permutations (`pipelines::Permutation`) are pipeline-overridable constants, so each
// variant compiles only its own code: alpha-tested materials discard below their cutoff,
// transparent ones write their coverage for blending, faces shade by their SDF map, hair takes its
// mask and highlight and eyes are always lit.
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
//   so a face's shadow is the SDF's, the environment's whole (R4), and the bangs'.
// - The bangs' shadow (M1 C5; `hair_visibility`): the face looks toward the key light across the
//   screen, the material's distance at the face's depth (the light's view-space direction
//   unnormalized, so light along the view casts a short shadow), and gathers the character
//   prepass there: the share of its own hair in front of the face, bilinearly weighted, × the
//   material's strength occludes the split as a cast shadow would, so the face darkens into its
//   own authored shadow tone. Desktop and mobile alike: the prepass is on every tier.
// - Masks (`character_maps::face_mask`, `face_expression`): always-lit eyes and brows; a face
//   highlight that brightens the lit tone away from the shadow's edge; a nose line in the
//   region's outline color that appears as the head turns from the camera; blush tinting the
//   albedo and gloom pushing toward the deepest shadow as occlusion does (its GI too, except
//   where always lit), each scaled by the character's expression weight.
//
// Hair (M1 C5, the hair permutation). The hair mask (`character_maps::hair_mask`) occludes by its
// B and reveals the highlight by its R; its G shifts the band. The highlight, in the material's
// color and strength and the key light's radiance, dimmed on the shadow side, is a band edged by
// the material's specular softness and a pixel over one of three coordinates, all computed and
// one picked so the edge's derivative is taken once:
// - painted band: N·H on the region's GI normals (rounded toward the head's sphere), plus the
//   shift, above 1 − width (WuWa, ZZZ);
// - angel ring: how near the view-space height of those normals is to the shift, the ring's
//   height, within the width: a ring that stays put on screen as the camera circles the head;
// - Kajiya-Kay: sin(T, H) above 1 − width, for strands along the direction in which UV v grows
//   (root to tip; from screen derivatives, since meshes carry no tangents), tilted along the
//   normal by the shift.
//
// Eyes (M1 C5, the eye permutation): always lit, going into an environment shadow with the head
// (`face_shadow`), with no rim and no cast shadow of their own. Anime eyes are layered meshes
// (M1 C6), each material naming its layer (`character_eye_layer`, materials.wgsl), drawn in
// order:
// - The eye itself (opaque or alpha-tested): the eyeball, and lashes or brows modeled as eye
//   parts. It writes the character prepass's eye part, whose second ID (the nearest surface but
//   for hair) is the **eye depth mask**.
// - Over it, transparent and sorted by layer: the upper lid's shadow on the eye white, in the
//   eye's shadow tone (its region's shade, as on a shadow side); the iris; the highlights. Each
//   blends only where the mask holds the character's own eye (`eye_mask`), so a lid closing over
//   the eye takes the layers with it and a gaze carrying the iris past the eye white is clipped
//   there, whatever their depth.
// - The parallax refraction fake (`iris_parallax`), on the eye and iris layers: the base color
//   is read where the view ray through the point would lie the material's depth below the cornea
//   (× the eye mask's R, or a dome over the iris ellipse, or whole), offset-limited (Welsh: the
//   view's along-surface part, not divided by N·V, so it never runs off at grazing angles), along
//   the surface by the gradients of u and v from screen derivatives, as `mapped_normal` finds
//   them. The pupil shifts against the limbal ring as the view turns.
// - Highlights live (`highlight_drift`, `highlight_sparkle`): each drifts along a Lissajous curve
//   across the model's front in the vertex stage and glints in a short pulse, brighter and its
//   coverage with it, so its soft edge swells; both on the frame's clock (R4: the packet's
//   time), at a phase per instance slot.
//
// Eyes through hair (M1 C5; `fs_through_hair`): faces and eyes whose material says so draw again
// after the outlines, where the character prepass's ID is the character's own hair, its second ID
// the drawing part's own (M1 C6: an eye under a closed lid stays hidden), and the eye lies at most
// the material's distance behind it: the face's always-lit mask (B: eyes, brows) or the whole eye,
// its layers included by their coverage, at the material's opacity, faded as the camera turns
// from head-forward past its yaw and pitch, blended over the hair. The fragment writes the hair's
// depth, a little nearer, so the main pass's depth test (greater-or-equal) keeps whatever hides
// the hair. Writing depth turns early depth testing off, so the prepass test comes first and only
// 2 × 2 quads with a pixel that passes it shade.
//
// Fog (M1 W3) covers the final color last (`fogged` in frame.wgsl), characters taking time of
// day's share of it (`fog_share`) and anything else drawn in this family the world's. The rim and
// the hair's highlight are lit inside it, and eyes through hair end in it too, at the eye's own
// position with its character's share (M1 C5), as the surface under them does.

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

// The hair mask's G that leaves the highlight where it is (`character_maps::hair_mask::NO_SHIFT`).
const HAIR_MASK_NO_SHIFT: f32 = 0.5;
// The hair highlight on the shadow side, as a share of the lit side's.
const HAIR_HIGHLIGHT_SHADOW: f32 = 0.3;
// How far in front of a face, in meters, its own hair must be to shadow it: bangs that rest on the
// forehead cast nothing where they touch it.
const HAIR_SHADOW_BIAS: f32 = 0.005;
// Eyes through hair write the hair's depth × this: a little nearer, so the main pass's depth test
// passes where the hair is the nearest surface across the pixel's samples.
const THROUGH_HAIR_DEPTH_NUDGE: f32 = 1.002;

// The eye layers' motion (M1 C6; `character_maps::eye_mask`): the drift's second axis turns this
// many times as fast as its first, and instance slots step the phase by the golden ratio's
// fraction of a turn. The sparkle's wave is raised to the sixth power (`highlight_sparkle`).
const EYE_DRIFT_RATIO: f32 = 1.5;
const EYE_PHASE_STEP: f32 = 0.618034;

// Widths are pixels on a 1080-row target, through a 45° lens (outlines, and the rim).
const OUTLINE_REFERENCE_HEIGHT: f32 = 1080.0;
// tan(22.5°): widths are normalized to a 45° vertical field of view.
const OUTLINE_REFERENCE_TAN_HALF_FOV: f32 = 0.41421356;
// The share of the cutoff distance, at its end, over which the width fades out
// (`OutlineCurve::FADE`).
const OUTLINE_CUTOFF_FADE: f32 = 0.25;

// Static permutation bits (`pipelines::Permutation`), set per pipeline.
override PERMUTATION_ALPHA_TEST: bool = false;
override PERMUTATION_TRANSPARENT: bool = false;
override PERMUTATION_FACE: bool = false;
override PERMUTATION_HAIR: bool = false;
override PERMUTATION_EYE: bool = false;

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

// A surface vertex, for the surface and the eyes-through-hair entry points alike. An eye's
// highlight layer drifts (M1 C6).
fn character_varyings(packed: PackedVertex, draw: DrawRecord) -> CharacterVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    var position = vertex.position;
    if PERMUTATION_EYE && PERMUTATION_TRANSPARENT {
        position += vec3<f32>(highlight_drift(draw.material, draw.instance), 0.0);
    }
    let world_position = instance.world_from_object * vec4<f32>(position, 1.0);
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

// A highlight's phase in radians, from its instance slot (`character_maps::eye_mask::phase`).
fn highlight_phase(instance: u32) -> f32 {
    return 2.0 * PI * fract(f32(instance) * EYE_PHASE_STEP);
}

// Where an eye's highlight layer has drifted (M1 C6; `character_maps::eye_mask::highlight_drift`):
// an offset across the model's front (X, Y), in meters; zero for any other layer. The block's
// `surface_color` holds the motion: x drift, y its rate.
fn highlight_drift(material: u32, instance: u32) -> vec2<f32> {
    let motion = materials[material].surface_color;
    let angle = 2.0 * PI * motion.y * frame.time.x + highlight_phase(instance);
    let drift = motion.x * vec2<f32>(sin(angle), sin(EYE_DRIFT_RATIO * angle + 1.0));
    let highlight = character_eye_layer(materials[material].modes) == CHARACTER_EYE_LAYER_HIGHLIGHT;
    return select(vec2<f32>(0.0), drift, highlight);
}

// How much brighter an eye's highlight layer glints now (M1 C6;
// `character_maps::eye_mask::highlight_sparkle`): 1 for any other layer. `surface_color`'s z is
// the sparkle, w its rate.
fn highlight_sparkle(material: u32, instance: u32) -> f32 {
    let motion = materials[material].surface_color;
    let wave = 0.5 + 0.5 * sin(2.0 * PI * motion.w * frame.time.x + highlight_phase(instance));
    let squared = wave * wave;
    let sparkle = 1.0 + motion.z * squared * squared * squared;
    let highlight = character_eye_layer(materials[material].modes) == CHARACTER_EYE_LAYER_HIGHLIGHT;
    return select(1.0, sparkle, highlight);
}

@vertex
fn vs_anime_character(packed: PackedVertex, draw: DrawRecord) -> CharacterVaryings {
    return character_varyings(packed, draw);
}

@vertex
fn vs_through_hair(packed: PackedVertex, draw: DrawRecord) -> CharacterVaryings {
    return character_varyings(packed, draw);
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
// the point into the authored shadow tones instead of darkening them. Characters (an entry of
// their own) take the cascades alone, which hold environment casters only (M1 R4): a character
// never self-shadows, and the rig's key direction, which may differ from the sun's, shades but
// never casts. Every other instance drawn with this shader is environment, and takes the
// characters' own shadows too (`shadow_visibility` in frame.wgsl). The face and eye permutations
// never call it: a face's shadow is its SDF map's alone, and an environment shadow reaches a face
// and its eyes whole, per character (`face_shadow`).
fn cast_shadow(in: CharacterVaryings) -> f32 {
    return cast_shadow_at(in.world_position, in.world_normal, in.character);
}

// `cast_shadow` at `position`, on a surface whose geometric normal is `normal` (any length), of an
// instance whose character entry is `character`: the outline shader's lit lines take it too
// (outline.wgsl), so a line goes into a cast shadow with its surface.
fn cast_shadow_at(position: vec3<f32>, normal: vec3<f32>, character: u32) -> f32 {
    if !SHADOWS {
        return 1.0;
    }
    let n = safe_normalize(normal);
    if character != 0u {
        return cascade_visibility(position, n);
    }
    return shadow_visibility(position, n);
}

// The world key light reaching character `character`'s face past environment casters, sampled
// once a frame over a sphere around its head (the face pass, shadows.wgsl): a face takes it whole,
// as occlusion of its light split, never shadow-map texels on it. 1 without shadows.
fn face_shadow(character: u32) -> f32 {
    if !SHADOWS {
        return 1.0;
    }
    return saturate(characters[character].shadow.visibility.x);
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

// The key light reaching a face at `pixel` (framebuffer coordinates) and `position` past the
// character's own hair (M1 C5, the bangs' shadow; see the top of this file): 1 where none of its
// hair lies the material's distance toward the light across the screen, in front of the face.
fn hair_visibility(
    material: u32,
    character: u32,
    pixel: vec2<f32>,
    position: vec3<f32>,
    toward_light: vec3<f32>,
) -> f32 {
    let shadow = materials[material].hair_shadow;
    let distance = max(view_distance(position), 1e-4);
    // The light across the screen, unnormalized (y down in pixels), and meters at the face's
    // depth in pixels: the target's height spans 2 × distance / focal length.
    let light = (view.view_from_world * vec4<f32>(toward_light, 0.0)).xy;
    let pixels_per_meter = 0.5 * view.clip_from_view[1][1] * view.viewport.y / distance;
    let at = pixel + vec2<f32>(light.x, -light.y) * (shadow.x * pixels_per_meter);
    let uv = at * view.viewport.zw;
    let ids = textureGather(0, character_ids, character_sampler, uv);
    let depths = textureGather(character_depth, character_sampler, uv);
    let hair = vec4<u32>(character_id(character, CHARACTER_SURFACE_HAIR));
    let nearer = depth_at_distance(max(distance - HAIR_SHADOW_BIAS, 1e-4));
    let covered = select(vec4<f32>(0.0), vec4<f32>(1.0), (ids == hair) & (depths > vec4<f32>(nearer)));
    // Bilinear weights of the gathered texels, in gather order: (u0, v1), (u1, v1), (u1, v0),
    // (u0, v0).
    let f = fract(at - 0.5);
    let weights = vec4<f32>((1.0 - f.x) * f.y, f.x * f.y, f.x * (1.0 - f.y), (1.0 - f.x) * (1.0 - f.y));
    return 1.0 - dot(covered, weights) * shadow.y;
}

// The eye and iris layers' parallax refraction fake (M1 C6; see the top of this file): `uv`
// moved along the surface to where the view ray through the point would lie below the cornea,
// the material's depth (`surface.x`, meters) × the eye mask's R, or a dome over the iris ellipse
// (`surface_color`: center, radii), or 1. `normal` is the interpolated one, `position_dx` and
// `position_dy` the point's screen derivatives. Unchanged for other layers and without depth.
fn iris_parallax(
    material: u32,
    uv: MaterialUv,
    normal: vec3<f32>,
    position: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
) -> MaterialUv {
    let modes = materials[material].modes;
    let parallax = materials[material].surface.x;
    let iris = materials[material].surface_color;
    var depth = 1.0;
    if material_has_texture(modes, CHARACTER_SURFACE_MASK) {
        let swizzles = materials[material].swizzles;
        depth = sample_material(surface_mask_map, swizzles, CHARACTER_SURFACE_MASK, uv).r;
    } else if iris.z > 0.0 && iris.w > 0.0 {
        let q = (uv.uv - iris.xy) / iris.zw;
        depth = saturate(1.0 - dot(q, q));
    }
    let n = safe_normalize(normal);
    let v = safe_normalize(view.position.xyz - position);
    // Below the surface, away from the camera: the view's part along the surface, reversed.
    let along = (n * dot(n, v) - v) * (parallax * depth);
    // The gradients of u and v along the surface (`mapped_normal`'s, divided by the
    // determinant): texture coordinates per meter.
    let dy_perp = cross(position_dy, n);
    let dx_perp = cross(n, position_dx);
    let determinant = dot(position_dx, dy_perp);
    let degenerate = abs(determinant) < 1e-24;
    let inverse = 1.0 / select(determinant, 1.0, degenerate);
    let gradient_u = (dy_perp * uv.dx.x + dx_perp * uv.dy.x) * inverse;
    let gradient_v = (dy_perp * uv.dx.y + dx_perp * uv.dy.y) * inverse;
    let layer = character_eye_layer(modes);
    let layered = layer == CHARACTER_EYE_LAYER_EYE || layer == CHARACTER_EYE_LAYER_IRIS;
    let shift = vec2<f32>(dot(gradient_u, along), dot(gradient_v, along));
    let shifts = layered && parallax > 0.0 && !degenerate;
    return MaterialUv(uv.uv + select(vec2<f32>(0.0), shift, shifts), uv.dx, uv.dy);
}

// The eye depth mask at `pixel` (framebuffer coordinates; M1 C6): 1 where the character
// prepass's nearest surface but for hair is character `character`'s eye, else 0. 1 for an
// instance that isn't a character, which the prepass doesn't draw.
fn eye_mask(character: u32, pixel: vec2<f32>) -> f32 {
    let nearest = textureLoad(character_ids, vec2<i32>(floor(pixel)), 0).g;
    let own = nearest == character_id(character, CHARACTER_SURFACE_EYE);
    return select(1.0, select(0.0, 1.0, own), character != 0u);
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

// How far `x` lies from `start` to `end`, clamped to 0..=1; 1 for an empty range.
fn outline_segment(start: f32, end: f32, x: f32) -> f32 {
    return select(1.0, saturate((x - start) / (end - start)), end > start);
}

// The width's scale at the effective distance `distance`: `OutlineCurve::scale`. The rim takes
// it alone, so it keeps its width past the outlines' cutoff and with outlines off.
fn outline_scale(material: u32, distance: f32) -> f32 {
    let knots = materials[material].outline_curve;
    let scales = materials[material].outline_shape.xy;
    let near = scales.x * knots.x / max(distance, 1e-6);
    let first = mix(scales.x, 1.0, outline_segment(knots.x, knots.y, distance));
    let second = mix(1.0, scales.y, outline_segment(knots.y, knots.z, distance));
    let far = scales.y * knots.z / max(distance, 1e-6);
    var scale = select(far, second, distance <= knots.z);
    scale = select(scale, first, distance <= knots.y);
    return select(scale, near, distance < knots.x);
}

// The outline width's scale at the effective distance `distance`, with the cutoff's fade:
// `OutlineCurve::scale × OutlineCurve::fade` (outline.wgsl).
fn outline_curve(material: u32, distance: f32) -> f32 {
    // The renderer's scale on the cutoff (`r.outline_distance`) applies to every material.
    let cutoff = materials[material].outline_curve.w * frame.outline_distance;
    let fade = saturate((cutoff - distance) / max(cutoff * OUTLINE_CUTOFF_FADE, 1e-6));
    return outline_scale(material, distance) * fade;
}

// The rim at a point (M1 C5): what every light's share has in common.
struct CharacterRim {
    // The depth step × the region's strength (and overlap) × the Fresnel gate: 0 for no rim.
    mask: f32,
    // Half the albedo, half white, × the region's rim color × the material's strength × the
    // rig's rim scale.
    tint: vec3<f32>,
    // The material's rim settings (`AnimeCharacterParams::rim`): z the light gate, w the
    // back-light weighting.
    settings: vec4<f32>,
}

// The depth-offset rim's mask at `pixel` (framebuffer coordinates) of a point at `position` of
// character `character`, in region `region`, whose Fresnel term's edge is `fresnel_width` wide
// (taken in uniform control flow): see the top of this file.
fn rim_mask(
    surface: CharacterSurface,
    material: u32,
    region: u32,
    character: u32,
    pixel: vec2<f32>,
    position: vec3<f32>,
    fresnel_width: f32,
) -> f32 {
    let rim = materials[material].rim;
    let scales = materials[material].rim_regions[region];
    // Along the normal across the screen: its view-space direction, y down in pixels.
    let normal = (view.view_from_world * vec4<f32>(surface.normal, 0.0)).xy;
    let direction = safe_normalize2(vec2<f32>(normal.x, -normal.y));
    // As many pixels as an outline's width would be (outline.wgsl): at 1080 rows, through a 45°
    // lens at the effective distance, by the curve and the model scale.
    let distance = max(view_distance(position), 1e-4);
    let effective = distance / (view.clip_from_view[1][1] * OUTLINE_REFERENCE_TAN_HALF_FOV);
    let pixels = materials[material].edges.x * scales.y * materials[material].outline_shape.z
        * (view.viewport.y / OUTLINE_REFERENCE_HEIGHT) * outline_scale(material, effective);
    let at = pixel + direction * pixels;
    // 1 where what lies there is the threshold farther away or no character is, filtered.
    let reference = depth_at_distance(distance + rim.x);
    let step = textureSampleCompareLevel(character_depth, shadow_sampler, at * view.viewport.zw, reference);
    // The character overlapping itself takes the region's overlap.
    let texel = clamp(vec2<i32>(floor(at)), vec2<i32>(0), vec2<i32>(view.viewport.xy) - 1);
    let id = textureLoad(character_ids, texel, 0).r;
    let own = (id & CHARACTER_ID_ENTRY_MASK) == (character & CHARACTER_ID_ENTRY_MASK);
    let overlap = select(1.0, scales.z, own);
    let fresnel = 1.0 - saturate(dot(surface.normal, surface.view_dir));
    let gate = light_step(1.0 - rim.y, fresnel_width, fresnel);
    return step * overlap * gate * scales.x;
}

// The rim a light toward the unit vector `l` gives, per unit of its radiance, at a point whose
// light split for it is `lit`: the light gate and the back-light weighting on the shared mask.
fn rim_light(rim: CharacterRim, l: vec3<f32>, view_dir: vec3<f32>, lit: f32) -> vec3<f32> {
    let gate = mix(1.0 - rim.settings.z, 1.0, lit);
    let back = mix(1.0, 1.0 - dot(view_dir, l), rim.settings.w);
    return rim.tint * (rim.mask * gate * back);
}

// The surface's normal turned toward a sphere around the head by the region's GI sphere weight
// (`RegionGi::sphere`, in the subsurface vector's spare channel), where the rig knows the head:
// faces and hair shade like one smooth ball, in the GI and the hair highlight.
fn spherized_normal(surface: CharacterSurface, rig: CharacterRig, position: vec3<f32>) -> vec3<f32> {
    let sphere = safe_normalize(position - rig.head.xyz);
    return safe_normalize(mix(surface.normal, sphere, surface.region.subsurface.a * rig.gi.w));
}

// Stylized character GI (see the top of this file): the light the GI gives the point, which
// its albedo then takes.
fn character_gi(
    surface: CharacterSurface,
    rig: CharacterRig,
    ambient: CharacterAmbient,
    position: vec3<f32>,
) -> vec3<f32> {
    // Faces and hair shade like one smooth ball around the head, by the region's weight.
    let spherized = spherized_normal(surface, rig, position);
    // The whole character leans toward the camera, which flattens it.
    let normal = safe_normalize(spherized + rig.gi.x * surface.view_dir);
    let gi = hsv_clamp(character_ambient_light(ambient, normal) * rig.head.w, rig.gi.y, rig.gi.z);
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

// The hair's strand direction (M1 C5): the way UV v grows across the surface (root to tip), from
// the screen derivatives of the position and UV, made perpendicular to `normal`. Meshes carry no
// tangents; within a triangle it is constant, so strands facet as the mesh does.
fn strand_direction(normal: vec3<f32>, position_dx: vec3<f32>, position_dy: vec3<f32>, uv: MaterialUv) -> vec3<f32> {
    // dP/dv, from dP = dP/du du + dP/dv dv along x and y, up to the UV map's handedness.
    let determinant = uv.dx.x * uv.dy.y - uv.dy.x * uv.dx.y;
    let along_v = (position_dy * uv.dx.x - position_dx * uv.dy.x) * select(-1.0, 1.0, determinant >= 0.0);
    return safe_normalize(along_v - normal * dot(normal, along_v));
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

// Every listed local light, each split and toned like the key light, with its rim through the
// shared depth step (`rim`), their sum held under `ceiling` (the rig's) in luminance (see the top
// of this file).
fn local_lighting(
    surface: CharacterSurface,
    ceiling: f32,
    list: u32,
    position: vec3<f32>,
    derivatives: CharacterDerivatives,
    rim: CharacterRim,
) -> vec3<f32> {
    let base = list * frame.lights.z;
    // Guards keep a list from reading past what was written this frame.
    let count = min(light_lists[base], frame.lights.w);
    // Light reaching the lit and the shadow side of each light's own split, and the rims.
    var lit = vec3<f32>(0.0);
    var shaded = vec3<f32>(0.0);
    var rims = vec3<f32>(0.0);
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
        rims += radiance * rim_light(rim, l, surface.view_dir, split);
    }
    let luminance = dot(lit + shaded, LUMINANCE);
    let held = min(1.0, ceiling / max(luminance, 1e-6));
    return (surface.albedo * lit + surface.shade * shaded + rims) * held;
}

// A shaded surface pixel, for the surface and the eyes-through-hair entry points.
struct ShadedCharacter {
    color: vec3<f32>,
    // The base color's alpha: transparent surfaces' coverage.
    alpha: f32,
    // What of it draws through the character's hair: a face's always-lit mask (B: eyes, brows),
    // a whole eye.
    through_hair: f32,
}

// Shades an anime character surface pixel (see the top of this file). Discards it below an
// alpha-tested material's cutoff.
fn shade_character(in: CharacterVaryings) -> ShadedCharacter {
    let material = in.material;
    let modes = materials[material].modes;
    let swizzles = materials[material].swizzles;
    // Derivatives first, in uniform control flow: material slots are sampled on branches the
    // block decides, with explicit gradients, and the normal map's frame needs the position's.
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let position_dx = dpdx(in.world_position);
    let position_dy = dpdy(in.world_position);

    // An eye's layer (M1 C6): the same for every pixel of a draw.
    let eye_layer = character_eye_layer(modes);
    var base_uv = uv;
    if PERMUTATION_EYE {
        base_uv = iris_parallax(material, uv, in.world_normal, in.world_position, position_dx, position_dy);
    }
    var base = materials[material].base_color;
    if material_has_texture(modes, CHARACTER_BASE_COLOR) {
        base *= sample_material(base_color_map, swizzles, CHARACTER_BASE_COLOR, base_uv);
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
    region = min(region, max(modes.y, 1u) - 1u);
    surface.region = materials[material].regions[region];
    surface.split = materials[material].split.xy;

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

    // Hair (M1 C5): the mask, then the highlight band's coordinate for every style, one picked,
    // so the band's edge is taken once in uniform control flow (see the top of this file).
    var hair_mask = vec3<f32>(1.0, HAIR_MASK_NO_SHIFT, 1.0);
    var band_input = 0.0;
    if PERMUTATION_HAIR {
        if material_has_texture(modes, CHARACTER_SURFACE_MASK) {
            hair_mask = sample_material(surface_mask_map, swizzles, CHARACTER_SURFACE_MASK, uv).rgb;
        }
        surface.occlusion *= hair_mask.b;
        let shift = materials[material].surface.z + hair_mask.g - HAIR_MASK_NO_SHIFT;
        let style = (modes.w >> 8u) & 0xffu;
        let rounded = spherized_normal(surface, rig, in.world_position);
        let h = safe_normalize(key_direction + surface.view_dir);
        let painted = dot(rounded, h) + shift;
        let ring = 1.0 - abs((view.view_from_world * vec4<f32>(rounded, 0.0)).y - shift);
        let strand = strand_direction(surface.normal, position_dx, position_dy, uv);
        let t_dot_h = dot(safe_normalize(strand + shift * surface.normal), h);
        let kajiya_kay = sqrt(saturate(1.0 - t_dot_h * t_dot_h));
        band_input = select(painted, ring, style == CHARACTER_HIGHLIGHT_ANGEL_RING);
        band_input = select(band_input, kajiya_kay, style == CHARACTER_HIGHLIGHT_KAJIYA_KAY);
    }
    let band_width = max(materials[material].terms.y, 0.5 * fwidth(band_input));

    // The rim's Fresnel edge, a pixel wide, in uniform control flow.
    let fresnel_width = 0.5 * fwidth(dot(surface.normal, surface.view_dir));

    var x: f32;
    var softness = surface.split.y;
    var face_lit_margin = 0.0;
    // The environment's shadow over a face or an eye, whole (1 for the body, which takes
    // `cast_shadow`).
    var head_visibility = 1.0;
    if PERMUTATION_FACE {
        // The face map shadows a face; an environment shadow reaches it whole, and the bangs'
        // from the character prepass, as occlusion of the split, never shadow-map texels on it.
        face_lit_margin = face_margin(face, key_direction, swizzles, uv);
        head_visibility = face_shadow(in.character);
        var hair = 1.0;
        let hair_shadow = materials[material].hair_shadow;
        if in.character != 0u && hair_shadow.x > 0.0 && hair_shadow.y > 0.0 {
            hair = hair_visibility(material, in.character, in.clip_position.xy, in.world_position, key_direction);
        }
        let face_occlusion = surface.occlusion * head_visibility * hair;
        x = split_input(face_n_dot_l(face_lit_margin, surface.split.x), face_occlusion, surface.shadow_bias);
        softness = face_softness(materials[material].surface.x, surface.split.x);
    } else if PERMUTATION_EYE {
        // Eyes are always lit, but go into an environment shadow with the head.
        head_visibility = face_shadow(in.character);
        x = split_input(1.0, surface.occlusion * head_visibility, surface.shadow_bias);
    } else {
        let n_dot_l = dot(surface.normal, key_direction);
        x = split_input(n_dot_l, surface.occlusion * cast_shadow(in), surface.shadow_bias);
    }
    let width = max(softness, 0.5 * fwidth(x));
    var lit = bias_locks(light_step(surface.split.x, width, x), surface.shadow_bias);
    if PERMUTATION_FACE {
        // Always-lit eyes and brows hold against the face map and the bangs' shadow, but go into
        // an environment shadow with the rest of the face.
        lit = max(lit, always_lit * head_visibility);
    }
    if PERMUTATION_EYE {
        // The lid's shadow on the eye white is the eye's shadow tone (M1 C6).
        lit = select(lit, 0.0, eye_layer == CHARACTER_EYE_LAYER_SHADOW);
    }
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

    // The depth-offset rim (M1 C5): characters' bodies, faces and hair, opaque or alpha-tested.
    var rim: CharacterRim;
    rim.settings = materials[material].rim;
    rim.tint = mix(surface.albedo, vec3<f32>(1.0), 0.5) * surface.region.rim.rgb
        * (materials[material].edges.y * rig.key_direction.w);
    if !PERMUTATION_EYE && !PERMUTATION_TRANSPARENT && in.character != 0u && materials[material].edges.y > 0.0 {
        let pixel = in.clip_position.xy;
        rim.mask = rim_mask(surface, material, region, in.character, pixel, in.world_position, fresnel_width);
    }

    surface.shade = surface.albedo * shade_tint(surface, material, x, width);
    let diffuse = mix(surface.shade, surface.albedo, lit) * key_radiance;
    let ambient = surface.albedo * character_gi(surface, rig, characters[in.character].ambient, in.world_position);
    var color = diffuse + ambient + rim_light(rim, key_direction, surface.view_dir, lit) * key_radiance;
    // A uniform branch: frames without local lights skip them.
    if frame.lights.y != LIGHT_LISTS_NONE {
        let list = light_list(in.clip_position, in.instance);
        color += local_lighting(surface, rig.key_radiance.w, list, in.world_position, derivatives, rim);
    }
    if PERMUTATION_HAIR {
        // The highlight band (surface.y wide) in its color × strength (surface.x), where the mask
        // reveals it, dimmed on the shadow side.
        let params = materials[material].surface;
        let band = light_step(1.0 - params.y, band_width, band_input) * hair_mask.r;
        let strength = params.x * band * mix(HAIR_HIGHLIGHT_SHADOW, 1.0, lit);
        color += materials[material].surface_color.rgb * key_radiance * strength;
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
    if PERMUTATION_EYE && PERMUTATION_TRANSPARENT {
        // A highlight glints (M1 C6): brighter, and its soft edge fills out, so a white one that
        // tone mapping already holds at white still swells.
        let sparkle = highlight_sparkle(material, in.instance);
        color *= sparkle;
        base.a = saturate(base.a * sparkle);
    }
    var shaded: ShadedCharacter;
    shaded.color = color;
    shaded.alpha = base.a;
    // An eye's layers draw through by their coverage (M1 C6).
    shaded.through_hair = select(always_lit, select(1.0, base.a, PERMUTATION_TRANSPARENT), PERMUTATION_EYE);
    return shaded;
}

@fragment
fn fs_anime_character(in: CharacterVaryings) -> @location(0) vec4<f32> {
    let shaded = shade_character(in);
    // Alpha is the world's share of the pixel, post's character mask (M1 P2): opaque passes write
    // 0 for characters (instances the frame lists, which read a light rig of their own) and 1
    // for everything else, as every other pass does; transparent ones blend by their coverage,
    // keeping the mask under them.
    let world = select(1.0, 0.0, in.character != 0u);
    var alpha = select(world, shaded.alpha, PERMUTATION_TRANSPARENT);
    if PERMUTATION_EYE && PERMUTATION_TRANSPARENT {
        // An eye's layers blend only over the character's own eye (M1 C6).
        alpha *= eye_mask(in.character, in.clip_position.xy);
    }
    // The final color, with the frame's fog over it (M1 W3): characters take their share of it,
    // so they stay readable where the world melts into the sky.
    let fogged_color = fogged(shaded.color, in.world_position, fog_share(in.character));
    return vec4<f32>(fogged_color, alpha);
}

struct ThroughHairOutput {
    // Blended by its alpha over the hair, keeping the scene color's alpha.
    @location(0) color: vec4<f32>,
    // The hair's depth, a little nearer.
    @builtin(frag_depth) depth: f32,
}

// Eyes and brows through the character's hair (M1 C5; see the top of this file): the face's or
// eye's pixel as the surface shades it, where the character prepass says the nearest surface is
// the character's own hair, at most the material's distance in front of it, and the nearest but
// for hair is the drawing part of the character (M1 C6), fogged as the surface is (M1 W3).
//
// Writing the depth turns early depth testing off, so every fragment of a face runs this; most
// lie under no hair. The cheap test (the prepass's ID and depth, the fades) comes first, and
// only 2 × 2 quads with a pixel that passes it shade. The quad's answer is the same in its four
// pixels (from fine derivatives of the pass/fail value, which give its neighbors' in the quad),
// so the shading's derivatives stay well defined inside the branch: the diagnostic filter lets
// the validator accept control flow it can't prove uniform, but which is uniform per quad.
@diagnostic(off, derivative_uniformity)
@fragment
fn fs_through_hair(in: CharacterVaryings) -> ThroughHairOutput {
    let through = materials[in.material].through_hair;
    let pixel = vec2<i32>(floor(in.clip_position.xy));
    let hair_depth = textureLoad(character_depth, pixel, 0);
    let ids = textureLoad(character_ids, pixel, 0).rg;
    let behind = view_distance(in.world_position) - distance_at_depth(hair_depth);
    let hair = character_id(in.character, CHARACTER_SURFACE_HAIR);
    let own = character_id(in.character, materials[in.material].modes.z);
    let under_hair = in.character != 0u && ids.x == hair && ids.y == own && behind <= through.w;
    // How far the camera has turned from head-forward around the head's up axis and across it:
    // 1 facing it, 0 at the material's fades (their cosines in y and z).
    let face = characters[in.character].face;
    let forward = face.forward.xyz;
    let up = face.up.xyz;
    let right = cross(forward, up);
    let view_dir = safe_normalize(view.position.xyz - in.world_position);
    let level = safe_normalize(view_dir - up * dot(view_dir, up));
    let upright = safe_normalize(view_dir - right * dot(view_dir, right));
    let yaw = 1.0 - saturate((1.0 - dot(level, forward)) / max(1.0 - through.y, 1e-4));
    let pitch = 1.0 - saturate((1.0 - dot(upright, forward)) / max(1.0 - through.z, 1e-4));
    let fade = through.x * min(yaw, pitch);
    let passes = select(0.0, 1.0, under_hair && fade > 0.0);
    // Its quad's other pixels from it and its fine derivatives: any of the four passes.
    let across = dpdxFine(passes);
    let down = dpdyFine(passes);
    let quad_passes = passes + abs(across) + abs(down) + abs(dpdyFine(across)) > 0.0;
    var color = vec3<f32>(0.0);
    var alpha = 0.0;
    if quad_passes {
        let shaded = shade_character(in);
        color = fogged(shaded.color, in.world_position, fog_share(in.character));
        alpha = fade * shaded.through_hair;
    }
    if passes == 0.0 || alpha <= 0.0 {
        discard;
    }
    var out: ThroughHairOutput;
    out.color = vec4<f32>(color, alpha);
    out.depth = min(hair_depth * THROUGH_HAIR_DEPTH_NUDGE, 1.0);
    return out;
}
