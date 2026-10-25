// Pass 2: inverted-hull outline. The pipeline culls front faces, so only the back of a hull
// pushed outward along the smoothed outline normal survives, peeking out around the
// silhouette.
//
// The hull is pushed in clip space by a width in pixels, so the outline is equally thick at
// any field of view and any resolution:
//
//   width_px = edges.z × region scale × vertex alpha × (target height / 1080) × distance curve
//
// The material's outline width (`AnimeCharacterParams::edges.z`, scaled by region 0's) is
// authored in pixels at a 1080-pixel-high target, and its color is region 0's. The distance curve keeps the
// width constant between OUTLINE_NEAR_DEPTH and OUTLINE_FAR_DEPTH, thins it beyond (so distant
// characters don't turn into outlines), and lets it grow in extreme close-ups. M1 replaces the
// curve with an artist-authored one (spec §5.12 "Outlines").

const OUTLINE_REFERENCE_HEIGHT: f32 = 1080.0;
const OUTLINE_NEAR_DEPTH: f32 = 0.5;
const OUTLINE_FAR_DEPTH: f32 = 25.0;

struct OutlineVaryings {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(flat) material: u32,
}

@vertex
fn vs_outline(packed: PackedVertex, draw: DrawRecord) -> OutlineVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    // The material's width × region 0's scale (C4 picks the region per vertex), read field by
    // field rather than copying the whole block.
    let width = materials[draw.material].edges.z * materials[draw.material].regions[0].outline.w;

    let world_position = instance.world_from_object * vec4<f32>(vertex.position, 1.0);
    var clip = view.clip_from_world * world_position;

    // The outline normal's direction on screen, in pixels: project it to view space, scale by
    // the projection's x/y focal lengths (aspect) and by the target size, then normalize.
    let world_normal = instance.normal_from_object * vertex.outline_normal;
    let view_normal = (view.view_from_world * vec4<f32>(world_normal, 0.0)).xy;
    let focal = vec2<f32>(view.clip_from_view[0][0], view.clip_from_view[1][1]);
    let screen_direction = safe_normalize2(view_normal * focal * view.viewport.xy);

    // Perspective projection: clip.w is the view depth.
    let depth = max(clip.w, 1e-4);
    let distance_curve = clamp(depth, OUTLINE_NEAR_DEPTH, OUTLINE_FAR_DEPTH) / depth;
    let width_px = width * vertex.color.a * (view.viewport.y / OUTLINE_REFERENCE_HEIGHT)
        * distance_curve;

    // Pixels to NDC is 2 / size per axis; multiplying by w undoes the perspective divide.
    clip = vec4<f32>(clip.xy + screen_direction * width_px * 2.0 * view.viewport.zw * clip.w, clip.zw);

    var out: OutlineVaryings;
    out.clip_position = clip;
    out.material = draw.material;
    return out;
}

@fragment
fn fs_outline(in: OutlineVaryings) -> @location(0) vec4<f32> {
    return vec4<f32>(materials[in.material].regions[0].outline.rgb, 1.0);
}
