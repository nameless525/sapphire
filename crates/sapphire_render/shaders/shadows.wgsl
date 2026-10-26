// Shadow passes (M1 R4, `sapphire_render::shadows`): the casters into the cascades and the
// character atlas, and the face pass. The receivers' functions are frame.wgsl's.
//
// - Casters (`vs_shadow_caster`, no fragment stage): positions only, through the shadow view
//   bound at the map's dynamic offset (group 1) and the instances (group 2). Group 0 is left out:
//   it holds the maps being drawn. Every pass decodes positions through `object_position`, so a
//   caster's shape is the one the main pass draws.
// - The face pass (`cs_face_shadow`): one thread per character entry samples the cascades over a
//   sphere around the head (the entry's `shadow.probe`) and writes the key light's visibility
//   there into the entry (`shadow.visibility`), which the face permutation takes as occlusion
//   (anime_character.wgsl's `face_shadow`): an environment shadow reaches a face whole. Its group
//   0 holds the shadow uniforms, the cascades and the comparison sampler at frame.wgsl's bindings;
//   group 1 the character buffer, read-write, at binding 1 (binding 0 is the view's, which the
//   face pass doesn't bind).

// Threads per workgroup of the face pass (`shadows::FACE_WORKGROUP`).
const FACE_SHADOW_WORKGROUP: u32 = 64u;

@group(1) @binding(1) var<storage, read_write> character_entries: array<CharacterData>;

@vertex
fn vs_shadow_caster(@location(0) position: vec4<f32>, draw: DrawRecord) -> @builtin(position) vec4<f32> {
    let instance = instances[draw.instance];
    let world_position = instance.world_from_object * vec4<f32>(object_position(position, draw), 1.0);
    return view.clip_from_world * world_position;
}

@compute @workgroup_size(FACE_SHADOW_WORKGROUP)
fn cs_face_shadow(@builtin(global_invocation_id) id: vec3<u32>) {
    let entry = id.x;
    if entry >= min(shadows.counts.w, arrayLength(&character_entries)) {
        return;
    }
    let probe = character_entries[entry].shadow.probe;
    var visibility = 1.0;
    if probe.w > 0.0 {
        // The head's center and six points around it, across and along the light: a head half in
        // a shadow's edge takes part of it, and a face moves into shadow smoothly as it walks in.
        // The normal points at the light, so the lookups move toward it, off any surface.
        let l = shadows.light_z.xyz;
        let x = shadows.light_x.xyz * probe.w;
        let y = shadows.light_y.xyz * probe.w;
        let z = l * probe.w;
        let center = probe.xyz;
        var sum = cascade_visibility(center, l);
        sum += cascade_visibility(center + x, l) + cascade_visibility(center - x, l);
        sum += cascade_visibility(center + y, l) + cascade_visibility(center - y, l);
        sum += cascade_visibility(center + z, l) + cascade_visibility(center - z, l);
        visibility = sum / 7.0;
    }
    character_entries[entry].shadow.visibility = vec4<f32>(visibility, 0.0, 0.0, 0.0);
}
