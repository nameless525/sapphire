// The prepasses (M1 R2, spec §5.12): passes over the frame's opaque meshes that draw nothing
// visible, for later passes to read.
//
// - Character prepass (`vs_character_prepass`, `fs_character_prepass`; M1 C5): the opaque and
//   alpha-tested anime character surfaces of the instances the frame lists as characters, static
//   or skinned, into a depth target and an ID target of their own (the `character_prepass`
//   module): each pixel's character entry and part (`character_id`, frame.wgsl). The anime
//   character shader's depth-offset rim, the bangs' shadow on faces and eyes through the bangs
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

@fragment
fn fs_character_prepass(in: CharacterPrepassVaryings) -> @location(0) u32 {
    return in.id;
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

@fragment
fn fs_scene_prepass(in: ScenePrepassVaryings) -> ScenePrepassTargets {
    return scene_prepass_targets(in);
}
