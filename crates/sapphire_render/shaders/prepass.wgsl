// The prepasses (M1 R2, spec §5.12): passes over the frame's opaque meshes that draw nothing
// visible, for later passes to read.
//
// - Character prepass (`vs_character_prepass`, no fragment stage): skinned and morphed instances
//   of the anime character family only, into a depth target of their own. That character-only depth is what rim light, hair
//   shadow on the face and eyes over the bangs read (C2, C5), on every tier.
// - Scene prepass (`vs_scene_prepass`, `fs_scene_prepass`): every opaque instance, into depth,
//   world normals and velocity, for TAA (R7), outlines under TAA (P1), GTAO and screen-space
//   edges. Characters move by their skinned positions, this frame's and the last drawn one's
//   (vertex slot 4); static meshes by their instance's transform since it was last drawn; both
//   by the camera's.
//
// Both compute the clip position exactly as the main pass's vertex stages do (`object_position`,
// then the instance transform, then the view), and mark it invariant, so a prepass's depth is
// the main pass's for the same geometry.
//
// Under temporal AA (M1 R7) the view's projection is jittered; velocity compares unjittered clip
// positions, so geometry that holds still has exactly zero velocity whatever the jitter.

struct ScenePrepassVaryings {
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    // Unjittered clip positions now and when last drawn, divided per pixel so velocity is
    // perspective-correct.
    @location(1) current: vec4<f32>,
    @location(2) previous: vec4<f32>,
}

struct ScenePrepassTargets {
    // World-space unit normal, octahedral (`oct_encode`).
    @location(0) normal: vec2<f32>,
    // Motion since the last drawn frame in texture coordinates (x right, y down): the pixel was
    // at `uv - velocity`.
    @location(1) velocity: vec2<f32>,
}

@vertex
fn vs_character_prepass(@location(0) position: vec4<f32>, draw: DrawRecord) -> @invariant @builtin(position) vec4<f32> {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(object_position(position, draw), 1.0);
    return view.clip_from_world * world_position;
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
    let current = in.current.xy / in.current.w;
    let previous = in.previous.xy / in.previous.w;
    var out: ScenePrepassTargets;
    out.normal = oct_encode(safe_normalize(in.world_normal));
    // NDC is y up, texture coordinates y down, and both span 2 NDC units per texture.
    out.velocity = (current - previous) * vec2<f32>(0.5, -0.5);
    return out;
}

// The octahedral encoding of a unit vector (Cigolle et al., JCGT 2014), the inverse of
// `oct_decode`; a zero vector encodes +Z.
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let l1 = abs(n.x) + abs(n.y) + abs(n.z);
    var e = select(vec2<f32>(0.0), n.xy / l1, l1 > 0.0);
    if n.z < 0.0 {
        e = (1.0 - abs(e.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), e >= vec2<f32>(0.0));
    }
    return e;
}
