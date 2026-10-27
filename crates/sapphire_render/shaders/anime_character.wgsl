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
// mask and highlight, eyes are always lit, only materials with a layer (and eyes) carry the
// layers' code (M1 C7), and two-sided ones (glTF's `doubleSided`, M1 C8) shade a back face with
// its normal flipped, as the side the camera sees: a collar's or a sleeve's inside.
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
//
// The layers (M1 C7) go on top of the base, each a branch on the block's flags
// (`CHARACTER_FLAG_SPECULAR` and the four after it) that every pixel of a draw takes alike, set
// only where a region turns the layer on, so materials without them shade exactly as before.
// Their code is compiled in only for materials with one (`PERMUTATION_LAYERS`) and for eyes, so
// the rest don't pay for its registers either: the branches alone, never taken, cost the main
// pass about 4% on an Apple GPU. A
// body's layer mask (`character_maps::layer_mask`, in the surface mask slot; 1 where unbound)
// paints them: R the matcap, G metallic, B emission, A the stockings' density. Faces, hair and
// eyes, whose masks that slot holds, take their regions' values alone.
// - **Stockings** (`stocking_opacity`), first, since they change the albedo everything after
//   reads: the fabric's color over the skin by its opacity, denser at the silhouette than facing
//   the camera (N·V through the material's range), times the region's weight and the mask's A,
//   varied by a fine weave faded to its average before its cells reach a pixel. A sheen of the
//   key light runs along the fabric (`stocking_sheen`: sin(T, H) to the sharpness, T the UV axis
//   the legs are laid out along), on the lit side.
// - **Matcap** (`matcap_sample`): the material's matcap at the view-space normal, the region's
//   channel (the color matcap or one of four grayscale ones) × its tint, × the mask's R and the
//   strength on the lit or the shadow side of the split. Added in the key light's color, or
//   multiplying the albedo.
// - **Stylized specular** (`character_specular`): the region's highlight, Blinn or minimal GGX on
//   N·H toward the key light (`specular_shape`), cut at 1 − the control map's B (0.5 without
//   one) with the material's softness or a pixel, × the control map's R (1 without one), on the
//   lit side, in the region's color tinted by the albedo as the material says.
// - **The reflection add** (`reflection_add`): where metallic (the region's × the mask's G) is
//   above `layer_mask::REFLECTS`, the sky's light along the reflected view ray × the albedo:
//   W5's reflection cube (ambient.wgsl), the sky at the ambient's scale, so it dims with the
//   ambient at night, at the mip of the region's roughness. The ambient pass prefilters it while
//   a reflective material draws, with a probe grid or without (`sapphire_render::ambient`).
// - **Emission** (`breath`): the region's glow × the mask's B, unlit, breathing over the
//   material's period at a phase per instance slot. Its share of the pixel's light, × the
//   material's bloom, is written as the world's share of the pixel (post's character mask, M1
//   P2), so the glow blooms as the world does while the character around it keeps its own.
//
// The proximity fade (M1 C7; `CharacterFade` and the dither in frame.wgsl): a fading character's
// opaque draws go through the alpha-tested variants, which, with the transparent ones, discard
// the pixels its dither leaves out (`faded_out`); under MSAA the alpha-tested variants' covered
// entry points write the dither's sample mask instead. Its outlines and eyes through hair keep
// the same pixels, and it takes no rim, whose depth step would read the character's own holes.

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
// The layers' code (M1 C7): compiled in for materials with a layer, and for every eye.
override PERMUTATION_LAYERS: bool = false;
// Both faces drawn (M1 C8): the pipeline culls nothing, and back faces shade as the side the
// camera sees, with the normal flipped (`facing`).
override PERMUTATION_TWO_SIDED: bool = false;
// The main pass's samples a pixel, for the proximity fade's dither (M1 C7; `faded_out` in
// frame.wgsl): set under MSAA on the alpha-tested variants' covered entry points, which cover a
// share of them, and on the outline hulls' masked prepass (outline.wgsl), which keeps the pixels
// where that share isn't empty; 1 for every other pipeline, which keeps or discards whole pixels.
override FADE_SAMPLES: u32 = 1u;

// The layers (M1 C7). The reflection add's gate: from `layer_mask::REFLECTS` up, over this much
// more metallic, so a mask's filtered edge doesn't alias.
const REFLECTS: f32 = 0.9;
const REFLECTS_EDGE: f32 = 0.02;
// The matcap's coordinates: the view-space normal's xy over this radius around the map's center,
// a little inside its edge, so filtering never wraps across it.
const MATCAP_RADIUS: f32 = 0.49;
// The weave shows while its cells span more than 1 / x pixels, and is gone by 1 / y.
const WEAVE_FADE: vec2<f32> = vec2<f32>(0.25, 0.5);
// Instance slots step the breath's phase by the golden ratio's fraction of a turn.
const BREATH_PHASE_STEP: f32 = 0.618034;
// `SpecularModel::MIN_GGX_ROUGHNESS`.
const MIN_GGX_ROUGHNESS: f32 = 0.02;

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
    // The control map's channels: R specular mask, G shadow bias, B specular threshold (the
    // stylized specular reads the specular ones, M1 C7).
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

// --- The layers (M1 C7; see the top of this file).

// The direction the fabric runs at a point: the way UV v grows across the surface (`along_v`), or
// u, from the screen derivatives as `strand_direction` finds v's, made perpendicular to `normal`.
fn fabric_direction(
    normal: vec3<f32>,
    position_dx: vec3<f32>,
    position_dy: vec3<f32>,
    uv: MaterialUv,
    along_v: bool,
) -> vec3<f32> {
    let determinant = uv.dx.x * uv.dy.y - uv.dy.x * uv.dx.y;
    let v = position_dy * uv.dx.x - position_dx * uv.dy.x;
    let u = position_dx * uv.dy.y - position_dy * uv.dx.y;
    let along = select(u, v, along_v) * select(-1.0, 1.0, determinant >= 0.0);
    return safe_normalize(along - normal * dot(normal, along));
}

// The stockings' opacity over the skin (`Stockings::opacity`, then the weave) at a point whose
// N·V is `n_dot_v`, for a region and mask wearing `weight` of them.
fn stocking_opacity(material: u32, weight: f32, n_dot_v: f32, uv: MaterialUv) -> f32 {
    let edge = materials[material].stocking_color.a;
    let shape = materials[material].stocking_shape;
    let strength = materials[material].stocking_weave.x;
    let facing = smoothstep(shape.x, max(shape.y, shape.x + 1e-4), n_dot_v);
    var opacity = mix(edge, shape.z, facing);
    // The weave: knit cells shifting the opacity around its average, faded to it before a cell
    // shrinks to two pixels.
    let cells = uv.uv * shape.w;
    let per_pixel = max(length(uv.dx), length(uv.dy)) * shape.w;
    let knit = sin(2.0 * PI * cells.x) * sin(2.0 * PI * cells.y);
    let shows = 1.0 - smoothstep(WEAVE_FADE.x, WEAVE_FADE.y, per_pixel);
    opacity *= 1.0 + strength * knit * shows;
    return saturate(opacity * weight);
}

// The stockings' sheen: the key light along the fabric's run, sin(T, H) to the material's
// sharpness, in its color, where the fabric covers `opacity` of the point and the split is `lit`.
fn stocking_sheen(
    material: u32,
    surface: CharacterSurface,
    direction: vec3<f32>,
    key_direction: vec3<f32>,
    key_radiance: vec3<f32>,
    lit: f32,
    opacity: f32,
) -> vec3<f32> {
    let sheen = materials[material].stocking_sheen;
    let h = safe_normalize(key_direction + surface.view_dir);
    let t_dot_h = dot(direction, h);
    let across = sqrt(saturate(1.0 - t_dot_h * t_dot_h));
    return sheen.rgb * key_radiance * (pow(across, max(sheen.a, 1.0)) * lit * opacity);
}

// The region's matcap (`RegionMatcap`): the matcap's channel it picks (the color matcap, or one
// of four grayscale ones) at `uv`, × its tint; black for none.
fn matcap_sample(material: u32, region: u32, swizzles: vec4<u32>, uv: MaterialUv) -> vec3<f32> {
    let pick = materials[material].layer_regions[region].matcap;
    let sampled = sample_material(matcap_map, swizzles, CHARACTER_MATCAP, uv);
    let channel = i32(round(pick.a));
    let gray = vec3<f32>(sampled[clamp(channel - 1, 0, 3)]);
    let value = select(gray, sampled.rgb, channel == 0);
    return select(value * pick.rgb, vec3<f32>(0.0), channel < 0);
}

// The highlight's shape (`SpecularModel::shape`) at `n_dot_h` for a region's `size`, and its
// slope: x the shape, 0 to 1 and 1 facing the half vector; y its derivative by N·H.
fn specular_shape(model: u32, n_dot_h: f32, size: f32) -> vec2<f32> {
    let x = saturate(n_dot_h);
    let r2 = clamp(size, MIN_GGX_ROUGHNESS, 1.0) * clamp(size, MIN_GGX_ROUGHNESS, 1.0);
    let d = x * x * (r2 - 1.0) + 1.0;
    let ggx = (r2 / d) * (r2 / d);
    let exponent = max(size, 1.0);
    let blinn = pow(x, exponent);
    let shape = select(blinn, ggx, model == CHARACTER_SPECULAR_GGX);
    let slope = select(exponent * blinn / max(x, 1e-4), ggx * 4.0 * x * (1.0 - r2) / d, model == CHARACTER_SPECULAR_GGX);
    return vec2<f32>(shape, slope);
}

// The stylized highlight (`Specular`): the region's shape of N·H cut at 1 − the specular
// threshold, its edge the material's softness or a pixel (the footprint's N·H change across
// it), × the specular mask, on the lit side, in the region's color tinted by the albedo.
fn character_specular(
    material: u32,
    surface: CharacterSurface,
    footprint: LayerFootprint,
    lit: f32,
    key_radiance: vec3<f32>,
) -> vec3<f32> {
    let region = surface.region.specular;
    let shape = specular_shape(materials[material].modes.w & 0xffu, footprint.n_dot_h, region.a);
    let width = max(materials[material].terms.y, 0.5 * abs(shape.y) * footprint.n_dot_h_width);
    let highlight = light_step(1.0 - surface.specular_threshold, width, shape.x) * surface.specular_mask * lit;
    let tint = mix(vec3<f32>(1.0), surface.albedo, materials[material].layers.x);
    return region.rgb * tint * key_radiance * highlight;
}

// The reflection add at a point `metallic` reflects, `roughness` blurred: the sky's light along
// the reflected view ray from W5's reflection cube, at the ambient's scale (mip 0 is the sky cube
// scaled as the ambient's sky, each rougher mip filtered from it), × the albedo and vertex AO;
// nothing at or below `layer_mask::REFLECTS`.
fn reflection_add(surface: CharacterSurface, metallic: f32, roughness: f32) -> vec3<f32> {
    let gate = smoothstep(REFLECTS, REFLECTS + REFLECTS_EDGE, metallic);
    let direction = reflect(-surface.view_dir, surface.normal);
    let mip = roughness * ambient.atlas_texel.w;
    let sky = textureSampleLevel(reflection_cube, ambient_sampler, direction, mip).rgb;
    return sky * surface.albedo * (surface.occlusion * gate);
}

// How bright a material's emission is now (`Emission::level`): 1 at the top of a breath, 1 − the
// depth at its bottom, at a phase per instance slot; 1 for a steady glow.
fn breath(material: u32, instance: u32) -> f32 {
    let layers = materials[material].layers;
    let phase = 2.0 * PI * fract(f32(instance) * BREATH_PHASE_STEP);
    let wave = 0.5 - 0.5 * cos(2.0 * PI * layers.y * frame.time.x + phase);
    return select(1.0, 1.0 - layers.z * wave, layers.y > 0.0);
}

// How far a character has faded (`CharacterFade`): 0 for whole, and for instances that aren't
// characters.
fn character_fade(character: u32) -> f32 {
    return characters[character].fade.amount.x;
}

// What the highlight and the matcap read (M1 C7): N·H toward the key light and its change
// across a pixel, for the highlight's edge, and the matcap's coordinates, from the view-space
// normal, with their footprint.
struct LayerFootprint {
    n_dot_h: f32,
    n_dot_h_width: f32,
    matcap: MaterialUv,
}

// The layers' footprint where the surface's normal is `normal`, seen along `view_dir` and lit
// along `key_direction`. Only materials with a highlight or a matcap take it, so the rest pay
// nothing for it. That branch is on the material, which a draw shares, so it is uniform in every
// 2 × 2 quad and the derivatives stay well defined in it: the diagnostic filter lets the
// validator accept control flow it can't prove uniform (as `through_hair`'s does).
@diagnostic(off, derivative_uniformity)
fn layer_footprint(normal: vec3<f32>, view_dir: vec3<f32>, key_direction: vec3<f32>) -> LayerFootprint {
    let n_dot_h = dot(normal, safe_normalize(key_direction + view_dir));
    let view_normal = (view.view_from_world * vec4<f32>(normal, 0.0)).xy;
    let coords = vec2<f32>(0.5) + MATCAP_RADIUS * vec2<f32>(view_normal.x, -view_normal.y);
    let dx = dpdx(vec3<f32>(n_dot_h, coords));
    let dy = dpdy(vec3<f32>(n_dot_h, coords));
    return LayerFootprint(n_dot_h, abs(dx.x) + abs(dy.x), MaterialUv(coords, dx.yz, dy.yz));
}

// The sample mask the proximity fade's dither covers at `pixel` (FADE_SAMPLES of them).
fn fade_mask(fade: f32, pixel: vec2<f32>, character: u32) -> u32 {
    let threshold = fade_threshold(pixel, character);
    return fade_sample_mask(fade_coverage(fade, threshold, FADE_SAMPLES), threshold, FADE_SAMPLES);
}

// A shaded surface pixel, for the surface and the eyes-through-hair entry points.
struct ShadedCharacter {
    color: vec3<f32>,
    // The base color's alpha: transparent surfaces' coverage.
    alpha: f32,
    // What of it draws through the character's hair: a face's always-lit mask (B: eyes, brows),
    // a whole eye.
    through_hair: f32,
    // The emission's share of the color's luminance × the material's bloom (M1 C7): what of the
    // pixel blooms as the world's light. 0 without emission.
    glow: f32,
    // Whether the pixel stays (M1 C7): false where the alpha test or the proximity fade cuts it
    // out, which the entry points discard at their one exit. Always true in opaque variants.
    kept: bool,
    // The region it shades with, which the material debug views show (M1 F5).
    region: u32,
}

// Shades an anime character surface pixel (see the top of this file). Quads cut out whole
// (below an alpha-tested material's cutoff, or faded out) leave here; the pixel that stays says
// whether it is kept.
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
    // Without a control map the highlight shows wherever its region's color says (M1 C7).
    surface.specular_mask = 1.0;
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
    // The pixels cut out (M1 C7): by the alpha test, of alpha-tested materials (the variant also
    // draws fading characters' opaque ones), and by the proximity fade's dither, decided after
    // every derivative above. Whole quads leave here (`quad_any`, frame.wgsl), since the layers
    // and the lighting follow; single pixels at the entry points' one exit
    // (docs/conventions.md).
    var kept = true;
    let alpha_tested = (modes.x & CHARACTER_FLAG_ALPHA_TEST) != 0u;
    if PERMUTATION_ALPHA_TEST && alpha_tested {
        kept = base.a >= materials[material].split.w;
    }
    var fade = 0.0;
    if PERMUTATION_ALPHA_TEST || PERMUTATION_TRANSPARENT {
        fade = character_fade(in.character);
        kept = kept && !(fade > 0.0 && faded_out(fade, in.clip_position.xy, in.character, FADE_SAMPLES));
        if !quad_any(kept) {
            discard;
        }
    }

    // The layers (M1 C7), where the block's flags say a region turns them on: the stockings and a
    // matcap that multiplies change the albedo, before anything reads it; the highlight, an added
    // matcap, the stockings' sheen, the reflection add and the glow add light. All of it is taken
    // here, in one branch on the flags, compiled out of the variants without the layers, and the
    // light is added below with the rest, so only the sum and the glow's luminance stay live
    // through the lighting.
    let layer_flags = select(0u, modes.x & CHARACTER_FLAG_LAYERS, PERMUTATION_LAYERS);
    var layer_light = vec3<f32>(0.0);
    var glow = 0.0;
    if layer_flags != 0u {
        var layer_mask = vec4<f32>(1.0);
        let body = !PERMUTATION_FACE && !PERMUTATION_HAIR && !PERMUTATION_EYE;
        if body && material_has_texture(modes, CHARACTER_SURFACE_MASK) {
            layer_mask = sample_material(surface_mask_map, swizzles, CHARACTER_SURFACE_MASK, uv);
        }
        if (layer_flags & CHARACTER_FLAG_STOCKINGS) != 0u {
            let weight = materials[material].layer_regions[region].surface.z * layer_mask.a;
            let n_dot_v = saturate(dot(surface.normal, surface.view_dir));
            let stockings = stocking_opacity(material, weight, n_dot_v, uv);
            surface.albedo = mix(surface.albedo, materials[material].stocking_color.rgb, stockings);
            let along_v = materials[material].stocking_weave.y > 0.5;
            let direction = fabric_direction(surface.normal, position_dx, position_dy, uv, along_v);
            layer_light += stocking_sheen(material, surface, direction, key_direction, key_radiance, lit, stockings);
        }
        var footprint: LayerFootprint;
        if (layer_flags & (CHARACTER_FLAG_SPECULAR | CHARACTER_FLAG_MATCAP)) != 0u {
            footprint = layer_footprint(surface.normal, surface.view_dir, key_direction);
        }
        if (layer_flags & CHARACTER_FLAG_MATCAP) != 0u && material_has_texture(modes, CHARACTER_MATCAP) {
            // The lit side's strength or the shadow side's, by the split, where the mask's R says;
            // an added matcap in the key light's color.
            let terms = materials[material].terms;
            let strength = mix(terms.w, terms.z, lit) * layer_mask.r;
            let sampled = matcap_sample(material, region, swizzles, footprint.matcap);
            if materials[material].layer_regions[region].surface.w > 0.5 {
                surface.albedo *= mix(vec3<f32>(1.0), sampled, saturate(strength));
            } else {
                layer_light += sampled * strength * key_radiance;
            }
        }
        // The light that reads the albedo, after both have changed it.
        if (layer_flags & CHARACTER_FLAG_SPECULAR) != 0u {
            layer_light += character_specular(material, surface, footprint, lit, key_radiance);
        }
        if (layer_flags & CHARACTER_FLAG_REFLECTION) != 0u {
            let reflects = materials[material].layer_regions[region].surface;
            layer_light += reflection_add(surface, reflects.x * layer_mask.g, reflects.y);
        }
        if (layer_flags & CHARACTER_FLAG_EMISSION) != 0u {
            let emission = materials[material].layer_regions[region].emission.rgb;
            let emitted = emission * (layer_mask.b * breath(material, in.instance));
            layer_light += emitted;
            glow = dot(emitted, LUMINANCE);
        }
    }

    // The depth-offset rim (M1 C5): characters' bodies, faces and hair, opaque or alpha-tested.
    // A fading character takes none (M1 C7).
    var rim: CharacterRim;
    rim.settings = materials[material].rim;
    rim.tint = mix(surface.albedo, vec3<f32>(1.0), 0.5) * surface.region.rim.rgb
        * (materials[material].edges.y * rig.key_direction.w);
    if !PERMUTATION_EYE && !PERMUTATION_TRANSPARENT && in.character != 0u && materials[material].edges.y > 0.0
        && fade == 0.0 {
        let pixel = in.clip_position.xy;
        rim.mask = rim_mask(surface, material, region, in.character, pixel, in.world_position, fresnel_width);
    }

    surface.shade = surface.albedo * shade_tint(surface, material, x, width);
    let diffuse = mix(surface.shade, surface.albedo, lit) * key_radiance;
    let ambient = surface.albedo * character_gi(surface, rig, characters[in.character].ambient, in.world_position);
    // The layers' light first: zero without them, which leaves the sum as it was.
    var color = layer_light + diffuse + ambient + rim_light(rim, key_direction, surface.view_dir, lit) * key_radiance;
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
    shaded.glow = 0.0;
    if (layer_flags & CHARACTER_FLAG_EMISSION) != 0u {
        let share = glow / max(dot(color, LUMINANCE), 1e-6);
        shaded.glow = saturate(materials[material].layers.w * share);
    }
    shaded.kept = kept;
    shaded.region = region;
    return shaded;
}

// A surface pixel (`fs_anime_character`): its final color and alpha, whether it is kept, and its
// region (the material debug views', M1 F5).
struct CharacterPixel {
    color: vec4<f32>,
    kept: bool,
    region: u32,
}

// The varyings of the side of a surface the camera sees (M1 C8): a two-sided variant's back
// faces turn the normal around, so they shade (and take the normal map, the shadow's offset and
// the rim) as a front face would. One-sided variants cull back faces, so `front` is always true.
fn facing(in: CharacterVaryings, front: bool) -> CharacterVaryings {
    var out = in;
    out.world_normal = select(in.world_normal, -in.world_normal, PERMUTATION_TWO_SIDED && !front);
    return out;
}

fn character_pixel(in: CharacterVaryings) -> CharacterPixel {
    let shaded = shade_character(in);
    // Alpha is the world's share of the pixel, post's character mask (M1 P2): opaque passes write
    // 0 for characters (instances the frame lists, which read a light rig of their own) and 1
    // for everything else, as every other pass does; transparent ones blend by their coverage,
    // keeping the mask under them. A character's glow is the world's light (M1 C7).
    let world = select(1.0, shaded.glow, in.character != 0u);
    var alpha = select(world, shaded.alpha, PERMUTATION_TRANSPARENT);
    if PERMUTATION_EYE && PERMUTATION_TRANSPARENT {
        // An eye's layers blend only over the character's own eye (M1 C6).
        alpha *= eye_mask(in.character, in.clip_position.xy);
    }
    // The final color, with the frame's fog over it (M1 W3): characters take their share of it,
    // so they stay readable where the world melts into the sky. One variable, at the one exit.
    let color = vec4<f32>(fogged(shaded.color, in.world_position, fog_share(in.character)), alpha);
    return CharacterPixel(color, shaded.kept, shaded.region);
}

@fragment
fn fs_anime_character(in: CharacterVaryings, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let pixel = character_pixel(facing(in, front));
    // The one exit, where the pixels cut out leave (docs/conventions.md). Opaque variants never
    // discard, whether or not the compiler inlines the shading: a reachable discard would cost
    // them the GPU's hidden surface removal.
    if (PERMUTATION_ALPHA_TEST || PERMUTATION_TRANSPARENT) && !pixel.kept {
        discard;
    }
    return pixel.color;
}

// A covered entry point's output (M1 C7): the color, and the proximity fade's sample mask.
struct CoveredOutput {
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) mask: u32,
}

// The alpha-tested variants' surfaces under MSAA (M1 C7): a fading character covers its share of
// each pixel's samples, as its dither says, rather than whole pixels.
@fragment
fn fs_anime_character_covered(in: CharacterVaryings, @builtin(front_facing) front: bool) -> CoveredOutput {
    let pixel = character_pixel(facing(in, front));
    var out: CoveredOutput;
    out.color = pixel.color;
    out.mask = fade_mask(character_fade(in.character), in.clip_position.xy, in.character);
    // The one exit, where the pixels cut out leave: the alpha test's, and those the dither
    // covers no sample of.
    if !pixel.kept {
        discard;
    }
    return out;
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
// only 2 × 2 quads with a pixel that passes it shade: the others leave whole (`quad_any`, whose
// answer is the same in a quad's four pixels), so the shading's derivatives stay well defined
// after it. The diagnostic filter lets the validator accept control flow it can't prove
// uniform, but which is uniform per quad. Single pixels leave at the entry points' one exit.
@diagnostic(off, derivative_uniformity)
fn through_hair(in: CharacterVaryings) -> ThroughHairPixel {
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
    let passes = under_hair && fade > 0.0;
    if !quad_any(passes) {
        discard;
    }
    let shaded = shade_character(in);
    // The final color, at the one exit.
    let alpha = fade * shaded.through_hair;
    var drawn: ThroughHairPixel;
    drawn.out.color = vec4<f32>(fogged(shaded.color, in.world_position, fog_share(in.character)), alpha);
    drawn.out.depth = min(hair_depth * THROUGH_HAIR_DEPTH_NUDGE, 1.0);
    drawn.kept = passes && shaded.kept && alpha > 0.0;
    drawn.region = shaded.region;
    return drawn;
}

// An eye-through-hair pixel, whether it is kept, and its region (M1 F5).
struct ThroughHairPixel {
    out: ThroughHairOutput,
    kept: bool,
    region: u32,
}

@fragment
fn fs_through_hair(in: CharacterVaryings) -> ThroughHairOutput {
    let pixel = through_hair(in);
    // The one exit, where the pixels cut out leave (docs/conventions.md).
    if !pixel.kept {
        discard;
    }
    return pixel.out;
}

// `ThroughHairOutput` with the proximity fade's sample mask (M1 C7).
struct CoveredThroughHairOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
    @builtin(sample_mask) mask: u32,
}

// Eyes through hair of the alpha-tested variants under MSAA (M1 C7): over the samples the
// character's surfaces cover there.
@fragment
fn fs_through_hair_covered(in: CharacterVaryings) -> CoveredThroughHairOutput {
    let pixel = through_hair(in);
    var out: CoveredThroughHairOutput;
    out.color = pixel.out.color;
    out.depth = pixel.out.depth;
    out.mask = fade_mask(character_fade(in.character), in.clip_position.xy, in.character);
    // The one exit, where the pixels cut out leave.
    if !pixel.kept {
        discard;
    }
    return out;
}

// --- The material debug views (M1 F5; spec §5.12 "Debug views", the `debug_view` module).
//
// A debug view's replay of the main pass draws its copies of the anime character pipelines
// through these entry points (`pipelines::Replay::Values`): the main pass's vertex stages and
// shading, cutting out the pixels the main pass cuts, but writing what the material views show
// in place of the color, which `debug_view.wgsl` reads. A transparent surface (an eye's layers,
// eyes through hair) stands for a pixel where it covers at least half of it. The main pass's own
// pipelines never draw through them.

// What drew a pixel (the values' x): a surface, or an outline hull (outline.wgsl).
const DEBUG_DRAWN_SURFACE: f32 = 1.0;
const DEBUG_DRAWN_LINE: f32 = 2.0;
// The values' z for the band modes, which read no ramp: two bands, and three.
const DEBUG_TWO_BANDS: f32 = -1.0;
const DEBUG_THREE_BANDS: f32 = -2.0;
// The coverage from which a transparent surface stands for its pixel.
const DEBUG_COVERS: f32 = 0.5;
// Material slots as the values' w holds them: modulo this, exact in a half float.
const DEBUG_MATERIALS: u32 = 2048u;

// A pixel's values (`debug_view::VALUES_FORMAT`, whole numbers exact in its half floats): x what
// drew it, y its region, z the ramp rows its shadow side reads in ramp mode (the day row +
// RAMP_ROWS × the night row; time of day blends the two), or the band mode's code, and w
// `extra`: a surface's material slot, or a line's width.
fn character_debug_values(drawn: f32, material: u32, region: u32, extra: f32) -> vec4<f32> {
    let modes = materials[material].modes.x;
    let day = materials[material].regions[region].shade.a;
    let night = materials[material].regions[region].deep_shade.a;
    let bands = select(DEBUG_TWO_BANDS, DEBUG_THREE_BANDS, (modes & CHARACTER_FLAG_DEEP_SHADE) != 0u);
    let rows = select(bands, day + RAMP_ROWS * night, (modes & CHARACTER_FLAG_RAMP) != 0u);
    return vec4<f32>(drawn, f32(region), rows, extra);
}

// A surface's values: `fs_anime_character`'s pixel.
@fragment
fn fs_anime_character_debug(in: CharacterVaryings, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let pixel = character_pixel(facing(in, front));
    let shows = pixel.kept && (!PERMUTATION_TRANSPARENT || pixel.color.a >= DEBUG_COVERS);
    let slot = f32(in.material % DEBUG_MATERIALS);
    let values = character_debug_values(DEBUG_DRAWN_SURFACE, in.material, pixel.region, slot);
    // The one exit, as the main pass's: opaque variants keep every pixel.
    if (PERMUTATION_ALPHA_TEST || PERMUTATION_TRANSPARENT) && !shows {
        discard;
    }
    return values;
}

struct ThroughHairDebugOutput {
    @location(0) values: vec4<f32>,
    // The hair's depth, as `fs_through_hair` writes it.
    @builtin(frag_depth) depth: f32,
}

// Eyes through hair's values: `fs_through_hair`'s pixel.
@fragment
fn fs_through_hair_debug(in: CharacterVaryings) -> ThroughHairDebugOutput {
    let pixel = through_hair(in);
    var out: ThroughHairDebugOutput;
    let slot = f32(in.material % DEBUG_MATERIALS);
    out.values = character_debug_values(DEBUG_DRAWN_SURFACE, in.material, pixel.region, slot);
    out.depth = pixel.out.depth;
    // The one exit.
    if !(pixel.kept && pixel.out.color.a >= DEBUG_COVERS) {
        discard;
    }
    return out;
}
