// Pass 3: inverted-hull outline. The pipeline culls front faces, so only the back of a hull
// pushed out along the smoothed outline normal survives, peeking out around the silhouette.

// The distance factor is the view depth, clamped. Inside this range the on-screen width stays
// roughly constant; beyond the far end outlines thin out so distant characters don't turn
// into outlines, and close-ups keep a minimum thickness.
const OUTLINE_MIN_DEPTH: f32 = 0.5;
const OUTLINE_MAX_DEPTH: f32 = 25.0;

struct OutlineVaryings {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(flat) material: u32,
}

@vertex
fn vs_outline(vertex: MeshVertex, @location(5) draw_id: u32) -> OutlineVaryings {
    let draw = draws[draw_id];
    let instance = instances[draw.instance];
    let material = materials[draw.material];

    let view_position = (frame.view * (instance.world_from_object * vec4<f32>(vertex.position, 1.0))).xyz;
    let world_normal = instance.normal_from_object * vertex.outline_normal;
    let view_normal = safe_normalize((frame.view * vec4<f32>(world_normal, 0.0)).xyz);

    // params.w is the width in view-space units at 1 m; vertex alpha scales it per vertex
    // (painted thinner at fingertips, zero where no outline is wanted). M1 adds the
    // resolution scale and an artist distance curve (spec §5.12 "Outlines").
    let distance_factor = clamp(-view_position.z, OUTLINE_MIN_DEPTH, OUTLINE_MAX_DEPTH);
    let width = material.params.w * vertex.color.a * distance_factor;

    var out: OutlineVaryings;
    out.clip_position = frame.proj * vec4<f32>(view_position + view_normal * width, 1.0);
    out.material = draw.material;
    return out;
}

@fragment
fn fs_outline(in: OutlineVaryings) -> @location(0) vec4<f32> {
    return vec4<f32>(materials[in.material].outline_color.rgb, 1.0);
}
