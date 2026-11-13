// The prepasses (M1 R2, spec §5.12): passes over the frame's opaque meshes that draw nothing
// visible, for later passes to read.
//
// - Character prepass (`vs_character_prepass`, `fs_character_prepass`; M1 C5): the opaque and
//   alpha-tested anime character surfaces of the instances the frame lists as characters, static
//   or skinned, into a depth target and an ID target of their own (the `character_prepass`
//   module): each pixel's character entry and part (`character_id`, frame.wgsl), twice. Hair
//   draws last through a pipeline that writes the first alone, so the second keeps the nearest
//   surface that isn't hair: the eye depth mask (M1 C6). The anime character shader's
//   depth-offset rim, the bangs' shadow on faces, eyes through the bangs and the eyes' layers
//   read them, on every tier, and temporal AA's responsive mask reads the depth. Its layout has
//   no group 0, which binds both targets; the ID is the vertex stage's, from the instance and the
//   material block.
// - Scene prepass (`vs_scene_prepass`, `fs_scene_prepass`): every opaque instance, into depth,
//   world normals and velocity, for TAA (R7, outlines M1 P1), GTAO and screen-space
//   edges. Characters move by their skinned positions, this frame's and the last drawn one's
//   (vertex slot 4); static meshes by their instance's transform since it was last drawn; both
//   by the camera's. The outline hulls follow the surfaces into the same targets
//   (`vs_outline_prepass` in outline.wgsl, M1 C4), with their line's velocity (M1 P1).
//
// Both compute the clip position exactly as the main pass's vertex stages do (`object_position`,
// then the instance transform, then the view), and mark it invariant, so a prepass's depth is
// the main pass's for the same geometry.
//
// Under temporal AA (M1 R7) the view's projection is jittered; velocity compares unjittered clip
// positions, so geometry that holds still has exactly zero velocity whatever the jitter.
//
// Masked prepasses (`*_masked`, M1 C7): the alpha-tested anime character variants draw into both
// through pipelines of their own, whose fragments keep the pixels the main pass keeps: the
// alpha test (the base color's alpha × its map's against the material's cutoff, for materials
// that alpha-test) and the proximity fade's dither (frame.wgsl's `faded_out`; under MSAA the
// pixels where the main pass covers any sample), whose opaque draws the draw list moves to those
// variants while a character fades. Alpha-tested hair has a masked twin of hair's pipeline,
// which writes the nearest surface's ID alone and draws last with it. So the rim and the bangs'
// shadow, eyes
// through hair, the eye layers, temporal AA's responsive mask and reprojection, and GTAO see a
// character where it is drawn, and what lies behind it through its cutouts and holes, as foliage
// is cut out of the scene prepass (M1 W6). Opaque surfaces keep the plain pipelines, which read
// no texture and never discard. The masked ones bind the anime character family's group 3 (its
// base color map) and read the surface stream's UVs; the fade and the character's entry come
// from the instance.
//
// They keep a dissolving character's pixels and a see-through region's as the main pass does
// too (M5 L11; `see_through_fade` and `dissolve_noise` in materials.wgsl): the dissolve from the
// instance, as the fade, and the region from the control map or the vertex's material ID.
//
// Two-sided surfaces (M1 C8): two-sided anime character variants draw into both through copies
// of the plain or masked pipelines that cull nothing, built with the first such variant. The
// scene prepass's write a back face's normal flipped (`PERMUTATION_TWO_SIDED`), the side the
// main pass shades. Their outline hulls are every variant's.

// `ScenePrepassVaryings` and `ScenePrepassTargets` are frame.wgsl's: the outline hull's prepass
// (outline.wgsl, M1 C4) writes the same targets.

struct CharacterPrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
}

@vertex
fn vs_character_prepass(@location(0) position: vec4<f32>, draw: DrawRecord) -> CharacterPrepassVaryings {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(object_position(position, draw), 1.0);
    var out: CharacterPrepassVaryings;
    out.clip_position = view.clip_from_world * world_position;
    out.id = character_id(instance.character, materials[draw.material].modes.z);
    return out;
}

// R the nearest surface's ID, G the nearest non-hair surface's: hair's pipeline writes R alone.
@fragment
fn fs_character_prepass(in: CharacterPrepassVaryings) -> @location(0) vec2<u32> {
    return vec2<u32>(in.id);
}

@vertex
fn vs_scene_prepass(
    @location(0) position: vec4<f32>,
    @location(1) normal: vec2<f32>,
    // Slot 4: where the vertex was when last drawn, in slot 1's format. Static meshes bind
    // their own positions again.
    @location(10) previous_position: vec4<f32>,
    draw: DrawRecord,
) -> ScenePrepassVaryings {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(object_position(position, draw), 1.0);
    var out: ScenePrepassVaryings;
    out.clip_position = view.clip_from_world * world_position;
    if SCENE_PREPASS_VELOCITY {
        let previous_world =
            instance.previous_world_from_object * vec4<f32>(object_position(previous_position, draw), 1.0);
        out.current = view.unjittered_clip_from_world * world_position;
        out.previous = view.previous_clip_from_world * previous_world;
    }
    out.world_normal = instance.normal_from_object * oct_decode(normal);
    return out;
}

// Two-sided anime character surfaces (M1 C8): their pipelines cull nothing, and back faces
// write the flipped normal, the side the main pass shades (`facing` in anime_character.wgsl).
override PERMUTATION_TWO_SIDED: bool = false;

// The normal of the side the camera sees: one-sided pipelines cull back faces.
fn facing_normal(normal: vec3<f32>, front: bool) -> vec3<f32> {
    return select(normal, -normal, PERMUTATION_TWO_SIDED && !front);
}

@fragment
fn fs_scene_prepass(in: ScenePrepassVaryings, @builtin(front_facing) front: bool) -> ScenePrepassTargets {
    var facing = in;
    facing.world_normal = facing_normal(in.world_normal, front);
    return scene_prepass_targets(facing);
}

// The vertex's material ID from the surface stream's vertex color, whose alpha holds the vertex
// flags as unorm8 (`decode_vertex` in frame.wgsl).
fn vertex_material_id(color: vec4<f32>) -> u32 {
    return u32(round(color.a * 255.0)) & VERTEX_MATERIAL_ID_MASK;
}

// The main pass's samples a pixel, for the masked prepasses' dither (M1 C7; `faded_out` in
// frame.wgsl): set under MSAA, so they keep the pixels where the main pass covers any sample of
// a fading character; 1 otherwise.
override FADE_SAMPLES: u32 = 1u;

// The anime character family's group 3, as anime_character.wgsl binds it
// (`AnimeCharacterMaterial::TEXTURE_SLOTS`): the masked prepasses read the base color map, and
// the control map's material IDs for see-through regions (M5 L11).
@group(3) @binding(0) var base_color_map: texture_2d<f32>;
@group(3) @binding(1) var control_map: texture_2d<f32>;
@group(3) @binding(8) var material_sampler: sampler;
@group(3) @binding(10) var material_id_sampler: sampler;

// The control map's material IDs, k stored as k / (levels - 1) (anime_character.wgsl's
// `MATERIAL_ID_LEVELS`).
const PREPASS_MATERIAL_ID_LEVELS: f32 = 8.0;

// The region a pixel of `material` at `uv` shades with, as the main pass picks it: the control
// map's material ID, unfiltered (`control_material_id` in anime_character.wgsl), or the vertex's
// `vertex_region` without one, clamped to the material's regions.
fn masked_region(material: u32, uv: vec2<f32>, vertex_region: u32) -> u32 {
    let modes = materials[material].modes;
    var region = vertex_region;
    if material_has_texture(modes, CHARACTER_CONTROL) {
        let sampled = textureSampleLevel(control_map, material_id_sampler, uv, 0.0);
        let level = material_swizzle(sampled, materials[material].swizzles, CHARACTER_CONTROL).a;
        region = u32(round(saturate(level) * (PREPASS_MATERIAL_ID_LEVELS - 1.0)));
    }
    return min(region, max(modes.y, 1u) - 1u);
}

// What a masked prepass needs to cut a pixel as the main pass does, besides its position.
struct MaskedCut {
    material: u32,
    // The surface's coordinates, with their screen derivatives.
    uv: MaterialUv,
    // The character's entry, how far it has faded and dissolved, and the vertex's material ID.
    character: u32,
    fade: f32,
    dissolve: f32,
    vertex_region: u32,
}

// How far the dither leaves out the pixel of `cut` (`see_through_fade`).
fn masked_fade(cut: MaskedCut) -> f32 {
    var fade = cut.fade;
    if (materials[cut.material].modes.x & CHARACTER_FLAG_SEE_THROUGH) != 0u {
        fade = see_through_fade(cut.material, masked_region(cut.material, cut.uv.uv, cut.vertex_region), fade);
    }
    return fade;
}

// Whether a masked prepass keeps the pixel at `pixel` of an anime character surface, `cut`:
// not cut by its alpha test, nor by the dissolve, nor left out by its dither at `fade`
// (`masked_fade`). The alpha test reads the surface's own coordinates, as the main pass's does:
// an eye's parallax moves its color alone (`iris_parallax` in anime_character.wgsl, review
// RS-004).
fn masked_keeps(cut: MaskedCut, pixel: vec2<f32>, fade: f32) -> bool {
    let material = cut.material;
    let uv = cut.uv;
    let modes = materials[material].modes;
    var alpha = materials[material].base_color.a;
    if material_has_texture(modes, CHARACTER_BASE_COLOR) {
        let sampled = textureSampleGrad(base_color_map, material_sampler, uv.uv, uv.dx, uv.dy);
        alpha *= material_swizzle(sampled, materials[material].swizzles, CHARACTER_BASE_COLOR).a;
    }
    let tested = (modes.x & CHARACTER_FLAG_ALPHA_TEST) != 0u;
    let alpha_cut = tested && alpha < materials[material].split.w;
    let dissolved = cut.dissolve > 0.0 && !dissolve_keeps(dissolve_noise(material, uv.uv), cut.dissolve);
    return !alpha_cut && !dissolved && !faded_out(fade, pixel, cut.character, FADE_SAMPLES);
}

// An alpha-tested or fading character in the character prepass: its ID, and what the cut needs.
struct MaskedCharacterPrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
    @location(1) @interpolate(flat) material: u32,
    @location(2) @interpolate(flat) fade: f32,
    @location(3) uv: vec2<f32>,
    // M5 L11: how far the character has dissolved, and the vertex's material ID.
    @location(4) @interpolate(flat) dissolve: f32,
    @location(5) @interpolate(flat) vertex_region: u32,
}

@vertex
fn vs_character_prepass_masked(
    @location(0) position: vec4<f32>,
    // Slot 3: the surface stream's UVs, and the vertex color, whose alpha holds the vertex flags.
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
    draw: DrawRecord,
) -> MaskedCharacterPrepassVaryings {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(object_position(position, draw), 1.0);
    var out: MaskedCharacterPrepassVaryings;
    out.clip_position = view.clip_from_world * world_position;
    out.id = character_id(instance.character, materials[draw.material].modes.z);
    out.material = draw.material;
    out.fade = instance.fade;
    out.uv = draw.uv_transform.xy + draw.uv_transform.zw * uv;
    out.dissolve = instance.dissolve;
    out.vertex_region = vertex_material_id(color);
    return out;
}

// The IDs where the main pass keeps the pixel, as `fs_character_prepass` writes them: alpha-tested
// hair's pipeline writes R alone too (M1 C6).
@fragment
fn fs_character_prepass_masked(in: MaskedCharacterPrepassVaryings) -> @location(0) vec2<u32> {
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let entry = in.id & CHARACTER_ID_ENTRY_MASK;
    let cut = MaskedCut(in.material, uv, entry, in.fade, in.dissolve, in.vertex_region);
    if !masked_keeps(cut, in.clip_position.xy, masked_fade(cut)) {
        discard;
    }
    return vec2<u32>(in.id);
}

// An alpha-tested or fading character in the scene prepass: `ScenePrepassVaryings`, and what the
// cut needs.
struct MaskedScenePrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) current: vec4<f32>,
    @location(2) previous: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) material: u32,
    @location(5) @interpolate(flat) fade: f32,
    @location(6) @interpolate(flat) character: u32,
    // M5 L11: how far the character has dissolved, and the vertex's material ID.
    @location(7) @interpolate(flat) dissolve: f32,
    @location(8) @interpolate(flat) vertex_region: u32,
}

@vertex
fn vs_scene_prepass_masked(
    @location(0) position: vec4<f32>,
    @location(1) normal: vec2<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
    @location(10) previous_position: vec4<f32>,
    draw: DrawRecord,
) -> MaskedScenePrepassVaryings {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(object_position(position, draw), 1.0);
    var out: MaskedScenePrepassVaryings;
    out.clip_position = view.clip_from_world * world_position;
    if SCENE_PREPASS_VELOCITY {
        let previous_world =
            instance.previous_world_from_object * vec4<f32>(object_position(previous_position, draw), 1.0);
        out.current = view.unjittered_clip_from_world * world_position;
        out.previous = view.previous_clip_from_world * previous_world;
    }
    out.world_normal = instance.normal_from_object * oct_decode(normal);
    out.uv = draw.uv_transform.xy + draw.uv_transform.zw * uv;
    out.material = draw.material;
    out.fade = instance.fade;
    out.character = instance.character;
    out.dissolve = instance.dissolve;
    out.vertex_region = vertex_material_id(color);
    return out;
}

// `ScenePrepassTargets` and the outline mask, which a fading character's surfaces mark (M1 C7):
// the masked pipeline writes it, its alpha-tested surfaces 0, as the target is cleared.
struct MaskedScenePrepassTargets {
    @location(0) normal: vec2<f32>,
    @location(1) velocity: vec2<f32>,
    @location(2) outline: f32,
}

// The targets where the main pass keeps the pixel.
@fragment
fn fs_scene_prepass_masked(
    in: MaskedScenePrepassVaryings,
    @builtin(front_facing) front: bool,
) -> MaskedScenePrepassTargets {
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let normal = facing_normal(in.world_normal, front);
    let targets = scene_prepass_targets(ScenePrepassVaryings(in.clip_position, normal, in.current, in.previous));
    let cut = MaskedCut(in.material, uv, in.character, in.fade, in.dissolve, in.vertex_region);
    let fade = masked_fade(cut);
    // A see-through region marks the mask as a fading character does (M5 L11).
    let outline = select(0.0, OUTLINE_MASK_FADING, fade > 0.0);
    if !masked_keeps(cut, in.clip_position.xy, fade) {
        discard;
    }
    return MaskedScenePrepassTargets(targets.normal, targets.velocity, outline);
}
