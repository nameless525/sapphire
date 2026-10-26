// Temporal anti-aliasing and upsampling (M1 R7; spec §5.12 "Anti-aliasing and upscaling"): the
// responsive mask and the resolve. Like post.wgsl this file stands alone; none of frame.wgsl's
// groups are bound.
//
// Bind groups:
//   0  temporal  constants (`TemporalUniforms`, written each frame), a linear clamp sampler and a
//                nearest clamp one
//   1  pass      the textures one pass reads and writes:
//
//      pass        bindings
//      mask        0 character depth, 1 scene depth (both the scene's size, from the prepasses)
//      resolve     0 scene color, 1 scene depth, 2 velocity, 3 responsive mask, 4 this frame's
//                  drawn depth (all the scene's size), 5 history (the output's size), 6 last
//                  frame's drawn depth (last frame's scene size), 7 the resolved frame (written,
//                  the output's size)
//
// The resolve runs at the output's size and reads the scene at its own (temporal upsampling when
// the scene is smaller), on HDR scene color before bloom and the tone curve: WuWa found that
// resolving after them amplified edge noise. It blends in a tonemap-weighted space, so a bright
// sample can't dominate a pixel and flicker (Karis, SIGGRAPH 2014). The history's alpha holds
// how many frames' worth of samples it carries, which is what makes upsampling work: a frame
// counts in proportion to how close its samples landed to the pixel.
//
// The resolve is a compute pass, written for its cost (half the first, fragment version's on an
// M5): each workgroup gathers the scene pixels its output pixels' 3 × 3 neighborhoods cover into
// workgroup memory once, already in the resolve's color space; the hot path has no loops, since
// wgpu bounds every loop with a counter that keeps Metal from unrolling it, and it reads
// textures through samplers, which need none of the bounds checks `textureLoad` gets.

// Mirrors `temporal::TemporalUniforms`.
struct TemporalUniforms {
    // xy: the scene's size in pixels (the resolve's input); zw: reciprocals.
    input_size: vec4<f32>,
    // xy: the output's size in pixels (the history's); zw: reciprocals.
    output_size: vec4<f32>,
    // xy: this frame's jitter in scene pixels, x right and y down: scene pixel p sampled what
    // lies at p + 0.5 - jitter without it. z: the reconstruction filter's sharpness, per square
    // output pixel. w: how much nearer, as a fraction of depth, a surface must be to count as
    // another one: to lend its motion to a neighbor, or to make a pixel it covered disoccluded.
    jitter: vec4<f32>,
    // This frame's unjittered clip space to the last frame's, for reprojecting from depth.
    previous_clip_from_clip: mat4x4<f32>,
    // x: exposure multiplier; y: 1 drops the history (a reset); z: the history's weight cap,
    // in frames, for pixels that hold still; w: for pixels moving at `response.x` or faster.
    history: vec4<f32>,
    // x: the speed, in output pixels per frame, where the moving cap applies; y: what the cap
    // is scaled by on responsive pixels moving that fast; z: the clip box's half-width in
    // standard deviations; w: the same on responsive pixels moving that fast.
    response: vec4<f32>,
    // x: how far, in scene pixels, a pixel drawn without velocity looks for the surface it moves
    // with: an outline's width at the scene's height.
    motion: vec4<f32>,
}

@group(0) @binding(0) var<uniform> taa: TemporalUniforms;
@group(0) @binding(1) var linear_clamp: sampler;
// For point reads and gathers: clamping at the edges is what the 3 × 3 neighborhoods want.
@group(0) @binding(2) var nearest_clamp: sampler;

// The mask pass's textures.
@group(1) @binding(0) var character_depth: texture_depth_2d;
@group(1) @binding(1) var mask_scene_depth: texture_depth_2d;

// The resolve's textures (the same group, another layout).
@group(1) @binding(0) var scene_color: texture_2d<f32>;
@group(1) @binding(1) var scene_depth: texture_depth_2d;
@group(1) @binding(2) var velocity_texture: texture_2d<f32>;
@group(1) @binding(3) var responsive_mask: texture_2d<f32>;
// What the main pass drew: its depth, the outline hull's included; the scene prepass's under
// MSAA, whose multisampled depth this can't read. This frame's, and last frame's.
@group(1) @binding(4) var drawn_depth: texture_depth_2d;
@group(1) @binding(5) var history: texture_2d<f32>;
@group(1) @binding(6) var previous_depth: texture_depth_2d;
@group(1) @binding(7) var resolved: texture_storage_2d<rgba16float, write>;

struct FullscreenVaryings {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> FullscreenVaryings {
    // Vertices (-1, -1), (3, -1), (-1, 3) cover the target with one triangle.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: FullscreenVaryings;
    out.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

// The responsive mask (R8Unorm, the scene's size): 1 where a character is the nearest surface.
// The character prepass writes the same depth as the scene prepass there, bit for bit, and less
// where something stands in front; it is cleared to 0 where no character is.
@fragment
fn fs_responsive_mask(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let character = textureLoad(character_depth, pixel, 0);
    let scene = textureLoad(mask_scene_depth, pixel, 0);
    return vec4<f32>(select(0.0, 1.0, character > 0.0 && character >= scene), 0.0, 0.0, 0.0);
}

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// The largest finite f16: scene color can hold infinities, which would poison the history.
const COLOR_MAX: f32 = 65504.0;

// The space the resolve filters, clips and blends in: exposed color compressed by its luma, so
// every value stays below luma 1 and a bright sample weighs no more than a display-white one.
fn compress(color: vec3<f32>) -> vec3<f32> {
    let exposed = clamp(color, vec3<f32>(0.0), vec3<f32>(COLOR_MAX)) * taa.history.x;
    return exposed / (1.0 + dot(exposed, LUMA));
}

// The inverse of `compress`, back to linear HDR scene color.
fn expand(compressed: vec3<f32>) -> vec3<f32> {
    return compressed / (max(1.0 - dot(compressed, LUMA), 1e-4) * taa.history.x);
}

// YCoCg separates brightness from chroma, so the clip box hugs the neighborhood's colors more
// tightly than an RGB box does (Karis 2014). The neighborhood is kept and filtered in it; both
// are linear, so the filtered color is the same in either.
fn to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn from_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// Moves `point` toward `center` until it lies in the box of half-widths `extent` around it
// (Playdead, GDC 2016): clipping keeps the history's hue where clamping each axis would not.
fn clip_to_box(point: vec3<f32>, center: vec3<f32>, extent: vec3<f32>) -> vec3<f32> {
    let offset = point - center;
    let units = abs(offset) / max(extent, vec3<f32>(1e-6));
    let largest = max(units.x, max(units.y, units.z));
    return select(point, center + offset / largest, largest > 1.0);
}

// The history at `uv` through a Catmull-Rom filter, from five bilinear taps (Jimenez, Filmic
// SMAA, SIGGRAPH 2016): bilinear history fetches soften the image a little every frame something
// moves; Catmull-Rom keeps it sharp. At a texel's center it returns that texel exactly.
fn sample_history(uv: vec2<f32>) -> vec4<f32> {
    let position = uv * taa.output_size.xy;
    let center = floor(position - 0.5) + 0.5;
    let f = position - center;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = -0.5 * f3 + f2 - 0.5 * f;
    let w1 = 1.5 * f3 - 2.5 * f2 + 1.0;
    let w2 = -1.5 * f3 + 2.0 * f2 + 0.5 * f;
    let w3 = 0.5 * f3 - 0.5 * f2;
    let w12 = w1 + w2;
    let texel = taa.output_size.zw;
    let uv0 = (center - 1.0) * texel;
    let uv3 = (center + 2.0) * texel;
    let uv12 = (center + w2 / w12) * texel;
    let sum = textureSampleLevel(history, linear_clamp, vec2<f32>(uv12.x, uv0.y), 0.0) * (w12.x * w0.y)
        + textureSampleLevel(history, linear_clamp, vec2<f32>(uv0.x, uv12.y), 0.0) * (w0.x * w12.y)
        + textureSampleLevel(history, linear_clamp, uv12, 0.0) * (w12.x * w12.y)
        + textureSampleLevel(history, linear_clamp, vec2<f32>(uv3.x, uv12.y), 0.0) * (w3.x * w12.y)
        + textureSampleLevel(history, linear_clamp, vec2<f32>(uv12.x, uv3.y), 0.0) * (w12.x * w3.y);
    // The four corner taps are left out, so the weights are renormalized.
    let weight = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    // Catmull-Rom overshoots at hard edges; the clip box catches what's left.
    return max(sum / weight, vec4<f32>(0.0));
}

// NDC of a point at texture coordinates `uv`.
fn ndc(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
}

// Where the point at `uv` and reverse-Z `depth` lies in the last frame's clip space, if it held
// still: only the camera moved. Depth 0 is a direction at infinity, which homogeneous
// coordinates carry exactly.
fn reproject(uv: vec2<f32>, depth: f32) -> vec4<f32> {
    return taa.previous_clip_from_clip * vec4<f32>(ndc(uv), depth, 1.0);
}

// The camera's motion at `uv` and `depth`, in texture coordinates like the prepass's velocity.
fn camera_motion(uv: vec2<f32>, depth: f32) -> vec2<f32> {
    let previous = reproject(uv, depth);
    return (ndc(uv) - previous.xy / previous.w) * vec2<f32>(0.5, -0.5);
}

// Whether the surface at `uv` and reverse-Z `depth` was hidden last frame where it came from
// (`previous_uv`): every last-frame depth in the 3 × 3 pixels around that point is nearer, by
// the margin, than the surface would have been there were it still. A pixel's depth flips
// across an edge within it as the jitter moves its sample, so a single pixel's would reject the
// history of every edge now and then (they flicker); taking the farthest of the 3 × 3 keeps
// edges and slopes from rejecting their own history. The margin covers objects' own motion in
// depth.
fn disoccluded(uv: vec2<f32>, depth: f32, previous_uv: vec2<f32>) -> bool {
    let previous = reproject(uv, depth);
    let expected = previous.z / previous.w;
    // Gathers at the four corners of the pixel there cover the 3 × 3 around it.
    let size = vec2<f32>(textureDimensions(previous_depth));
    let texel = 1.0 / size;
    let corner = floor(previous_uv * size) * texel;
    let a = textureGather(previous_depth, nearest_clamp, corner);
    let b = textureGather(previous_depth, nearest_clamp, corner + vec2<f32>(texel.x, 0.0));
    let c = textureGather(previous_depth, nearest_clamp, corner + vec2<f32>(0.0, texel.y));
    let d = textureGather(previous_depth, nearest_clamp, corner + texel);
    let farther = min(min(a, b), min(c, d));
    let farthest = min(min(farther.x, farther.y), min(farther.z, farther.w));
    // Reverse-Z depth is proportional to 1 / distance, so the margin is a ratio of distances.
    return farthest > expected * (1.0 + taa.jitter.w) + 1e-7;
}

// The resolve's workgroups are GROUP × GROUP output pixels.
const GROUP: u32 = 8u;
// The scene pixels a workgroup's 3 × 3 neighborhoods cover, a side: the scene is never larger
// than the output, so GROUP output pixels' nearest scene pixels span at most GROUP of them (one
// more for rounding), with one more on each side for the neighborhood; even, for 2 × 2 gathers.
const TILE: u32 = 12u;
const TILE_PIXELS: u32 = 144u;

// The tile, from the scene pixel at `tile_origin` (clamped to the scene: rows and columns past its
// edges repeat the edge's). xyz: color in the resolve's space (compressed, YCoCg); w: depth.
var<workgroup> tile_color: array<vec4<f32>, TILE_PIXELS>;
// x: the responsive mask; y: the drawn depth.
var<workgroup> tile_other: array<vec2<f32>, TILE_PIXELS>;

// The output pixel's center among the scene's pixels, without the jitter.
fn scene_position(pixel: vec2<u32>) -> vec2<f32> {
    return (vec2<f32>(pixel) + 0.5) * taa.input_size.xy * taa.output_size.zw;
}

// The scene pixel whose sample landed nearest `position`: scene pixel p sampled p + 0.5 - jitter.
fn nearest_pixel(position: vec2<f32>) -> vec2<i32> {
    return vec2<i32>(floor(position + taa.jitter.xy));
}

// One scene pixel of the tile, in the resolve's color space.
fn tile_color_at(uv: vec2<f32>, depth: f32) -> vec4<f32> {
    let color = textureSampleLevel(scene_color, nearest_clamp, uv, 0.0).rgb;
    return vec4<f32>(to_ycocg(compress(color)), depth);
}

// Gathers the tile's 2 × 2 block whose top-left pixel is `block` (in the tile).
fn gather_block(tile_origin: vec2<i32>, block: vec2<u32>) {
    // The block's shared corner, in the scene's texture coordinates; its pixels' centers lie
    // half a pixel either way.
    let texel = taa.input_size.zw;
    let corner = vec2<f32>(tile_origin + vec2<i32>(block) + 1) * texel;
    let depth = textureGather(scene_depth, nearest_clamp, corner);
    let responsive = textureGather(0, responsive_mask, nearest_clamp, corner);
    let drawn = textureGather(drawn_depth, nearest_clamp, corner);
    let half_texel = 0.5 * texel;
    // A gather's components are its pixels (0, 1), (1, 1), (1, 0) and (0, 0), as x, y, z, w.
    let i = block.y * TILE + block.x;
    tile_color[i] = tile_color_at(corner - half_texel, depth.w);
    tile_color[i + 1u] = tile_color_at(corner + vec2<f32>(half_texel.x, -half_texel.y), depth.z);
    tile_color[i + TILE] = tile_color_at(corner + vec2<f32>(-half_texel.x, half_texel.y), depth.x);
    tile_color[i + TILE + 1u] = tile_color_at(corner + half_texel, depth.y);
    tile_other[i] = vec2<f32>(responsive.w, drawn.w);
    tile_other[i + 1u] = vec2<f32>(responsive.z, drawn.z);
    tile_other[i + TILE] = vec2<f32>(responsive.x, drawn.x);
    tile_other[i + TILE + 1u] = vec2<f32>(responsive.y, drawn.y);
}

// What one pass over a pixel's 3 × 3 neighborhood gathers.
struct Neighborhood {
    // The reconstruction filter's weighted sum, and the color moments, in the resolve's space.
    filtered: vec3<f32>,
    moment1: vec3<f32>,
    moment2: vec3<f32>,
    // The nearest surface's depth and tile index.
    closest_depth: f32,
    closest: u32,
    // Whether a character covers any of it.
    responsive: f32,
    // The nearest depth the main pass drew clearly in front of the scene prepass's (the outline
    // hull, see the resolve), or 0.
    hull_depth: f32,
}

// Adds the tile's pixel `index`, of filter weight `weight`.
fn add_tap(neighborhood: ptr<function, Neighborhood>, index: u32, weight: f32) {
    let texel = tile_color[index];
    let other = tile_other[index];
    let color = texel.xyz;
    (*neighborhood).filtered += color * weight;
    (*neighborhood).moment1 += color;
    (*neighborhood).moment2 += color * color;
    if texel.w > (*neighborhood).closest_depth {
        (*neighborhood).closest_depth = texel.w;
        (*neighborhood).closest = index;
    }
    (*neighborhood).responsive = max((*neighborhood).responsive, other.x);
    let hull = select(0.0, other.y, other.y > texel.w * (1.0 + taa.jitter.w));
    (*neighborhood).hull_depth = max((*neighborhood).hull_depth, hull);
}

@compute @workgroup_size(GROUP, GROUP)
fn cs_temporal_resolve(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    // The tile starts one scene pixel before the nearest of the workgroup's first output pixel
    // and ends one after its last's; upsampled, it takes fewer than TILE a side, and fewer
    // blocks are gathered.
    let tile_origin = nearest_pixel(scene_position(group.xy * GROUP)) - 1;
    let tile_end = nearest_pixel(scene_position(group.xy * GROUP + GROUP - 1u)) + 1;
    let blocks = min(vec2<u32>(tile_end - tile_origin) / 2u + 1u, vec2<u32>(TILE / 2u));
    if index < blocks.x * blocks.y {
        gather_block(tile_origin, vec2<u32>(index % blocks.x, index / blocks.x) * 2u);
    }
    workgroupBarrier();
    let output = group.xy * GROUP + local.xy;
    if any(vec2<f32>(output) >= taa.output_size.xy) {
        return;
    }

    let uv = (vec2<f32>(output) + 0.5) * taa.output_size.zw;
    let position = scene_position(output);
    let nearest = nearest_pixel(position);
    let last = vec2<i32>(taa.input_size.xy) - 1;

    // The reconstruction filter, exp(-k d²) in output pixels, is separable: the weights of the
    // three columns and rows around the nearest scene pixel, whose samples landed at
    // p + 0.5 - jitter.
    let scale = taa.output_size.xy * taa.input_size.zw;
    let offset = (vec2<f32>(nearest) + 0.5 - taa.jitter.xy - position) * scale;
    let columns = vec3<f32>(offset.x - scale.x, offset.x, offset.x + scale.x);
    let rows = vec3<f32>(offset.y - scale.y, offset.y, offset.y + scale.y);
    let wx = exp(-taa.jitter.z * columns * columns);
    let wy = exp(-taa.jitter.z * rows * rows);

    // One pass over the 3 × 3 scene pixels around it: the filter, the neighborhood's color
    // moments, its nearest surface and whether a character covers it.
    let tap = clamp(nearest - tile_origin, vec2<i32>(1), vec2<i32>(i32(TILE) - 2));
    let center_index = u32(tap.y) * TILE + u32(tap.x);
    let top = center_index - TILE;
    let bottom = center_index + TILE;
    var neighborhood = Neighborhood(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), 0.0, 0u, 0.0, 0.0);
    add_tap(&neighborhood, top - 1u, wx.x * wy.x);
    add_tap(&neighborhood, top, wx.y * wy.x);
    add_tap(&neighborhood, top + 1u, wx.z * wy.x);
    add_tap(&neighborhood, center_index - 1u, wx.x * wy.y);
    add_tap(&neighborhood, center_index, wx.y * wy.y);
    add_tap(&neighborhood, center_index + 1u, wx.z * wy.y);
    add_tap(&neighborhood, bottom - 1u, wx.x * wy.z);
    add_tap(&neighborhood, bottom, wx.y * wy.z);
    add_tap(&neighborhood, bottom + 1u, wx.z * wy.z);
    let weight = dot(wx, vec3<f32>(1.0)) * dot(wy, vec3<f32>(1.0));
    let current = from_ycocg(neighborhood.filtered / weight);
    // How much this frame knows about the pixel: 1 when a sample landed on its center, less
    // the farther they all are. Upsampled pixels between samples gather theirs over frames.
    let confidence = min(weight, 1.0);
    let mean = neighborhood.moment1 / 9.0;
    let deviation = sqrt(max(neighborhood.moment2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    var responsive = neighborhood.responsive;

    // Whose motion the pixel follows. Its own surface's, unless a surface clearly nearer covers
    // part of the neighborhood: then that one's (velocity dilation), so a silhouette's edge
    // pixels move with it rather than with what lies behind. A surface only slightly nearer is
    // the pixel's own, curving: its faster front would drag the edge ahead.
    let own_depth = tile_color[center_index].w;
    var source = center_index;
    var source_depth = own_depth;
    if neighborhood.closest_depth > own_depth * (1.0 + taa.jitter.w) {
        source = neighborhood.closest;
        source_depth = neighborhood.closest_depth;
    }
    var source_pixel = clamp(tile_origin + vec2<i32>(vec2<u32>(source % TILE, source / TILE)), vec2<i32>(0), last);
    // Where the main pass drew something clearly nearer than the scene prepass saw, that
    // something writes no velocity: the outline hull, until C4 draws it into the prepass. It
    // moves with the edge it outlines, which no surface pixel's velocity quite gives: on a body
    // turning about its own axis the surface slides a pixel inside the silhouette while the
    // silhouette, and so its outline, holds still. So outlines are responsive, as Blue Protocol
    // protects its outlines from blur: the current frame's opaque line outweighs a history
    // fetched from slightly the wrong place. Pixels a hull covers part of the neighborhood of,
    // in front of their own surface, follow it too, as a silhouette's edge pixels follow the
    // silhouette: the outline's antialiased edges, which this frame's jittered hull may miss,
    // would otherwise keep a still history of what the outline is moving over.
    let drawn = tile_other[center_index].y;
    let hull = neighborhood.hull_depth;
    if hull > own_depth * (1.0 + taa.jitter.w) {
        responsive = 1.0;
        // A hull wider than this neighborhood takes the velocity of the nearest surface whose
        // depth matches its own, looking out to an outline's width in rings of eight, the
        // innermost first: the edge's, not any surface point's of that depth. Hull pixels are
        // few, so this one loop stays.
        if hull > neighborhood.closest_depth * (1.0 + taa.jitter.w) {
            // A surface must match the hull's depth better than the sky (depth 0) does.
            var best = hull;
            for (var ring = 1; ring <= 3; ring++) {
                let distance = taa.motion.x * f32(ring) / 3.0;
                for (var direction = 0; direction < 8; direction++) {
                    let angle = f32(direction) * 0.7853982;
                    let reach = round(vec2<f32>(cos(angle), sin(angle)) * distance);
                    let pixel = clamp(nearest + vec2<i32>(reach), vec2<i32>(0), last);
                    let depth = textureSampleLevel(scene_depth, nearest_clamp, (vec2<f32>(pixel) + 0.5) * taa.input_size.zw, 0);
                    let difference = abs(depth - hull);
                    if difference < best {
                        best = difference;
                        source_depth = depth;
                        source_pixel = pixel;
                    }
                }
                if best < hull * taa.jitter.w {
                    break;
                }
            }
        }
    }

    // Where no geometry is, the camera's motion at infinity.
    var velocity = textureSampleLevel(velocity_texture, nearest_clamp, (vec2<f32>(source_pixel) + 0.5) * taa.input_size.zw, 0.0).xy;
    if source_depth == 0.0 {
        velocity = camera_motion(uv, 0.0);
    }
    let previous_uv = uv - velocity;
    let on_screen = all(previous_uv >= vec2<f32>(0.0)) && all(previous_uv <= vec2<f32>(1.0));
    // Against what was drawn, hulls included, so the pixels an outline has just moved off are
    // disoccluded too.
    let kept = taa.history.y == 0.0 && on_screen && !disoccluded(uv, drawn, previous_uv);

    // The history weighs at most `cap` frames: fewer the faster the pixel moves (the current
    // frame's weight rises with motion, as WuWa's does), and fewer again on moving responsive
    // pixels, which also clip tighter, so characters' outlines and light splits don't smear. A
    // character that holds still accumulates like everything else: its thin outlines need the
    // full history and the wide box to keep their contrast.
    //
    // Motion by a fraction of a pixel counts as fast motion does: every such frame resamples the
    // history between its texels, which a one- or two-pixel line doesn't survive many times (at
    // half a pixel Catmull-Rom halves its peak). The loss is 4 f (1 - f) for an offset of f.
    let moved = velocity * taa.output_size.xy;
    let fraction = abs(moved - round(moved));
    let resampling = 4.0 * max(fraction.x * (1.0 - fraction.x), fraction.y * (1.0 - fraction.y));
    let motion = max(saturate(length(moved) / taa.response.x), resampling);
    let responding = responsive * motion;
    let cap = mix(taa.history.z, taa.history.w, motion) * mix(1.0, taa.response.y, responding);
    let gamma = mix(taa.response.z, taa.response.w, responding);

    // Fetched whether or not it is kept, so the fetches start before the test is done.
    let sampled = sample_history(previous_uv);
    let clipped = clip_to_box(to_ycocg(compress(sampled.rgb)), mean, gamma * deviation);
    // The box is in YCoCg, so a clipped color can fall outside RGB's gamut. A history that isn't
    // kept is replaced outright, not weighted by 0, so nothing in it can reach the result.
    let history_color = select(current, max(from_ycocg(clipped), vec3<f32>(0.0)), kept);
    let history_weight = select(0.0, min(sampled.a, cap), kept);
    let blended = (history_color * history_weight + current * confidence) / (history_weight + confidence);
    textureStore(resolved, output, vec4<f32>(expand(blended), history_weight + confidence));
}
