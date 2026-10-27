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
    let previous_world =
        instance.previous_world_from_object * vec4<f32>(object_position(previous_position, draw), 1.0);
    var out: ScenePrepassVaryings;
    out.clip_position = view.clip_from_world * world_position;
    out.current = view.unjittered_clip_from_world * world_position;
    out.previous = view.previous_clip_from_world * previous_world;
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

// The main pass's samples a pixel, for the masked prepasses' dither (M1 C7; `faded_out` in
// frame.wgsl): set under MSAA, so they keep the pixels where the main pass covers any sample of
// a fading character; 1 otherwise.
override FADE_SAMPLES: u32 = 1u;

// The anime character family's group 3, as anime_character.wgsl binds it
// (`AnimeCharacterMaterial::TEXTURE_SLOTS`): the masked prepasses read the base color map alone.
@group(3) @binding(0) var base_color_map: texture_2d<f32>;
@group(3) @binding(8) var material_sampler: sampler;

// Whether a masked prepass keeps the pixel at `pixel` of an anime character surface drawn with
// `material` at `uv` (with its screen derivatives), of character entry `character` faded by
// `fade`: not cut by its alpha test, nor left out by its dither. The alpha test reads the
// surface's own coordinates, as the main pass's does: an eye's parallax moves its color alone
// (`iris_parallax` in anime_character.wgsl, review RS-004).
fn masked_keeps(material: u32, uv: MaterialUv, pixel: vec2<f32>, fade: f32, character: u32) -> bool {
    let modes = materials[material].modes;
    var alpha = materials[material].base_color.a;
    if material_has_texture(modes, CHARACTER_BASE_COLOR) {
        let sampled = textureSampleGrad(base_color_map, material_sampler, uv.uv, uv.dx, uv.dy);
        alpha *= material_swizzle(sampled, materials[material].swizzles, CHARACTER_BASE_COLOR).a;
    }
    let tested = (modes.x & CHARACTER_FLAG_ALPHA_TEST) != 0u;
    let cut = tested && alpha < materials[material].split.w;
    return !cut && !faded_out(fade, pixel, character, FADE_SAMPLES);
}

// An alpha-tested or fading character in the character prepass: its ID, and what the cut needs.
struct MaskedCharacterPrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
    @location(1) @interpolate(flat) material: u32,
    @location(2) @interpolate(flat) fade: f32,
    @location(3) uv: vec2<f32>,
}

@vertex
fn vs_character_prepass_masked(
    @location(0) position: vec4<f32>,
    // Slot 3: the surface stream's UVs.
    @location(3) uv: vec2<f32>,
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
    return out;
}

// The IDs where the main pass keeps the pixel, as `fs_character_prepass` writes them: alpha-tested
// hair's pipeline writes R alone too (M1 C6).
@fragment
fn fs_character_prepass_masked(in: MaskedCharacterPrepassVaryings) -> @location(0) vec2<u32> {
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    let entry = in.id & CHARACTER_ID_ENTRY_MASK;
    if !masked_keeps(in.material, uv, in.clip_position.xy, in.fade, entry) {
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
}

@vertex
fn vs_scene_prepass_masked(
    @location(0) position: vec4<f32>,
    @location(1) normal: vec2<f32>,
    @location(3) uv: vec2<f32>,
    @location(10) previous_position: vec4<f32>,
    draw: DrawRecord,
) -> MaskedScenePrepassVaryings {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(object_position(position, draw), 1.0);
    let previous_world =
        instance.previous_world_from_object * vec4<f32>(object_position(previous_position, draw), 1.0);
    var out: MaskedScenePrepassVaryings;
    out.clip_position = view.clip_from_world * world_position;
    out.current = view.unjittered_clip_from_world * world_position;
    out.previous = view.previous_clip_from_world * previous_world;
    out.world_normal = instance.normal_from_object * oct_decode(normal);
    out.uv = draw.uv_transform.xy + draw.uv_transform.zw * uv;
    out.material = draw.material;
    out.fade = instance.fade;
    out.character = instance.character;
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
    let outline = select(0.0, OUTLINE_MASK_FADING, in.fade > 0.0);
    if !masked_keeps(in.material, uv, in.clip_position.xy, in.fade, in.character) {
        discard;
    }
    return MaskedScenePrepassTargets(targets.normal, targets.velocity, outline);
}
