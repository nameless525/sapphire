// Pass 2: inverted-hull outline. The pipeline culls front faces, so only the back of a hull
// pushed outward along the smoothed outline normal survives, peeking out around the
// silhouette.
//
// The hull is pushed in clip space by a width in pixels, so the outline is equally thick at
// any field of view and any resolution:
//
//   width_px = edges.z × region scale × vertex color g × (target height / 1080) × distance curve
//
// The material's outline width (`AnimeCharacterParams::edges.z`) is authored in pixels at a
// 1080-pixel-high target; the region's (the vertex's material ID) scales it and gives the color.
// The distance curve keeps the width constant between OUTLINE_NEAR_DEPTH and OUTLINE_FAR_DEPTH,
// thins it beyond (so distant characters don't turn into outlines), and lets it grow in extreme
// close-ups. The hull is also pushed away from the camera along the view ray by the material's
// Z offset (`edges.w`, meters) × the vertex color's b, which hides it behind nearby surfaces
// without moving it on screen. Outlines v2 (C4) replaces the curve with an authored one and
// adds the view-space flattening.
//
// The alpha-tested permutation discards the hull where the base color map's coverage is below
// the material's cutoff, so cutouts get no outline across their holes. Transparent materials
// draw no hull.

const OUTLINE_REFERENCE_HEIGHT: f32 = 1080.0;
const OUTLINE_NEAR_DEPTH: f32 = 0.5;
const OUTLINE_FAR_DEPTH: f32 = 25.0;

// Static permutation bits (`pipelines::Permutation`), set per pipeline.
override PERMUTATION_ALPHA_TEST: bool = false;

// The anime character family's group 3 (see anime_character.wgsl): the hull reads the base color
// map's coverage alone.
@group(3) @binding(0) var base_color_map: texture_2d<f32>;
@group(3) @binding(8) var material_sampler: sampler;

struct OutlineVaryings {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(flat) material: u32,
    // The region the vertex's material ID picks, clamped to the material's.
    @location(1) @interpolate(flat) region: u32,
    // The base color map's coordinates, for the alpha-tested permutation.
    @location(2) uv: vec2<f32>,
}

@vertex
fn vs_outline(packed: PackedVertex, draw: DrawRecord) -> OutlineVaryings {
    let vertex = decode_vertex(packed, draw);
    let instance = instances[draw.instance];
    // Read field by field rather than copying the whole block.
    let regions = max(materials[draw.material].modes.y, 1u);
    let region = min(vertex.flags & VERTEX_MATERIAL_ID_MASK, regions - 1u);
    let width = materials[draw.material].edges.z * materials[draw.material].regions[region].outline.w;
    let z_offset = materials[draw.material].edges.w * vertex.color.b;

    let world_position = instance.world_from_object * vec4<f32>(vertex.position, 1.0);
    let away = safe_normalize(world_position.xyz - view.position.xyz) * z_offset;
    var clip = view.clip_from_world * vec4<f32>(world_position.xyz + away, 1.0);

    // The outline normal's direction on screen, in pixels: project it to view space, scale by
    // the projection's x/y focal lengths (aspect) and by the target size, then normalize.
    let world_normal = instance.normal_from_object * vertex.outline_normal;
    let view_normal = (view.view_from_world * vec4<f32>(world_normal, 0.0)).xy;
    let focal = vec2<f32>(view.clip_from_view[0][0], view.clip_from_view[1][1]);
    let screen_direction = safe_normalize2(view_normal * focal * view.viewport.xy);

    // Perspective projection: clip.w is the view depth.
    let depth = max(clip.w, 1e-4);
    let distance_curve = clamp(depth, OUTLINE_NEAR_DEPTH, OUTLINE_FAR_DEPTH) / depth;
    let width_px = width * vertex.color.g * (view.viewport.y / OUTLINE_REFERENCE_HEIGHT)
        * distance_curve;

    // Pixels to NDC is 2 / size per axis; multiplying by w undoes the perspective divide.
    clip = vec4<f32>(clip.xy + screen_direction * width_px * 2.0 * view.viewport.zw * clip.w, clip.zw);

    var out: OutlineVaryings;
    out.clip_position = clip;
    out.material = draw.material;
    out.region = region;
    out.uv = vertex.uv;
    return out;
}

@fragment
fn fs_outline(in: OutlineVaryings) -> @location(0) vec4<f32> {
    // Taken in uniform control flow; the sample sits on a branch the block decides.
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    if PERMUTATION_ALPHA_TEST {
        let modes = materials[in.material].modes;
        var alpha = materials[in.material].base_color.a;
        if material_has_texture(modes, CHARACTER_BASE_COLOR) {
            let sampled = textureSampleGrad(base_color_map, material_sampler, uv.uv, uv.dx, uv.dy);
            alpha *= material_swizzle(sampled, materials[in.material].swizzles, CHARACTER_BASE_COLOR).a;
        }
        if alpha < materials[in.material].split.w {
            discard;
        }
    }
    return vec4<f32>(materials[in.material].regions[in.region].outline.rgb, 1.0);
}
