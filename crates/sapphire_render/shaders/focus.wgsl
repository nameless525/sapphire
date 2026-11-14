// Depth of field (M5 L8, `post::focus`): appended to post.wgsl in its program, so it shares post's group 0
// (`PostUniforms`, whose `focus` and `focus_blur` it reads, and the linear clamp sampler), its group 1
// textures and its fullscreen vertex stage.
//
// Bind groups 1, by the textures a pass reads:
//
//      pass                texture0                      texture1                 focus_depth (binding 6)
//      focus prepare       post's input (full size)      —                        the scene prepass's depth
//      focus gather        the prepared half size        —                        —
//      focus composite     post's input                  the gathered half size   the scene prepass's depth
//      soft backdrop       an inset's scene              —                        —
//
// The main view, on the desktop tiers: the prepare pass halves post's input (temporal AA's resolve, at the
// output's size) and gives each half-size texel its signed blur in half-size pixels from the depth under it
// (negative in front of the sharp range, positive behind); the gather blurs each texel over three rings of
// taps (8, 16 and 24) out to its own blur, each tap weighted by how far its own blur reaches, so a sharp
// subject never spreads into what is blurred and what is blurred behind never spreads over it; the
// composite mixes the gathered texel back over each full-size pixel by the pixel's own blur, upsampling with
// weights that favour texels of a like blur, so neither side of a sharp edge leaks into the other. A pixel
// whose blur is within half a pixel is post's input, bit for bit.
//
// An inset's backdrop (M3 U6's 3D insets): its scene has no depth, so what blurs is what isn't a character
// (scene colour's alpha, the character mask: 0 on characters and their outlines), over the same rings, each
// tap by its world share, and every character pixel is its scene's, bit for bit.

// The scene prepass's reverse-Z depth: 0 is infinitely far, at its own size (the scene's, under the output's
// below render scale 1).
@group(1) @binding(6) var focus_depth: texture_depth_2d;

// How much of the largest blur a surface at reverse-Z `depth` takes: `sapphire_look::Focus::share_at`.
// The camera's projection is reverse-Z with an infinite far plane, so view depth is the near plane's
// distance over the stored depth.
fn focus_share(depth: f32) -> f32 {
    let view_depth = post.focus_blur.y / max(depth, 1e-30);
    let offset = view_depth - post.focus.x;
    let far = saturate((offset - post.focus.y) * post.focus.w);
    let near = saturate((-offset - post.focus.y) * post.focus.z);
    return far - near;
}

// The depth under input pixel `pixel` of an input of `size`: the depth's own texel at the same place.
fn focus_depth_at(pixel: vec2<i32>, size: vec2<f32>) -> f32 {
    let depth_size = vec2<i32>(textureDimensions(focus_depth));
    let uv = (vec2<f32>(pixel) + 0.5) / size;
    let texel = min(vec2<i32>(uv * vec2<f32>(depth_size)), depth_size - 1);
    return textureLoad(focus_depth, max(texel, vec2<i32>(0)), 0);
}

// The largest blur's radius in half-size pixels, for an input of `height` rows.
fn focus_radius(height: f32) -> f32 {
    return post.focus_blur.x * height * 0.5;
}

@fragment
fn fs_focus_prepare(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let dimensions = vec2<i32>(textureDimensions(texture0));
    let size = vec2<f32>(dimensions);
    let base = vec2<i32>(in.position.xy) * 2;
    let radius = focus_radius(size.y);
    // The shares of the 4 × 4 pixels around the texel's four, so each tap can take the least of its 3 × 3:
    // a pixel on a sharp subject's antialiased edge holds some of the subject's light under the depth of
    // what's behind it, and counts as sharp, so that light isn't spread at the blur behind.
    var shares: array<f32, 16>;
    for (var cell = 0u; cell < 16u; cell++) {
        let offset = vec2<i32>(i32(cell & 3u), i32(cell >> 2u)) - 1;
        let pixel = clamp(base + offset, vec2<i32>(0), dimensions - 1);
        shares[cell] = focus_share(focus_depth_at(pixel, size));
    }
    var color = vec3<f32>(0.0);
    var blur = 0.0;
    var total = 0.0;
    for (var tap = 0u; tap < 4u; tap++) {
        let corner = vec2<u32>(tap & 1u, tap >> 1u);
        let pixel = min(base + vec2<i32>(corner), dimensions - 1);
        var share = shares[(corner.y + 1u) * 4u + corner.x + 1u];
        for (var cell = 0u; cell < 9u; cell++) {
            let around = shares[(corner.y + cell / 3u) * 4u + corner.x + cell % 3u];
            share = select(share, around, abs(around) < abs(share));
        }
        let light = min(max(textureLoad(texture0, pixel, 0).rgb, vec3<f32>(0.0)), vec3<f32>(COLOR_MAX));
        // A blurred tap far outweighs a sharper one, so a texel across a sharp edge takes the blurred side's
        // light and blur: the subject's light never rides a blurred texel into the blur around it.
        let weight = 0.001 + share * share;
        color += light * weight;
        blur += share * weight;
        total += weight;
    }
    return vec4<f32>(color / total, blur / total * radius);
}

// The gather and the soft backdrop tap three rings, of 8, 16 and 24 taps, out to the reach a pixel blurs to,
// each ring's directions turned by a fixed step from the last.
const FOCUS_RINGS: u32 = 3u;
const FOCUS_RING_TAPS: u32 = 8u;

// The area a blur of `reach` half-size pixels spreads a texel's light over, from a texel's.
fn focus_area(reach: f32) -> f32 {
    let radius = max(reach, 1.0);
    return radius * radius;
}

@fragment
fn fs_focus_gather(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let dimensions = vec2<i32>(textureDimensions(texture0));
    let middle = textureLoad(texture0, pixel, 0);
    let reach = abs(middle.a);
    // What blurred behind may take from a tap: no more than its own blur, so a farther, blurrier surface
    // never spreads over a nearer one. What blurs in front spreads by its own.
    let behind = max(middle.a, 0.0);
    // Each tap's light is spread over its blur's disc, so it counts over that disc's area: where a smaller
    // blur meets a larger one, the smaller's light keeps its share of the larger's taps, as a scatter would.
    var total = vec4<f32>(middle.rgb, 1.0) / focus_area(reach);
    if reach > 0.25 {
        for (var ring = 1u; ring <= 3u; ring++) {
            let count = ring * FOCUS_RING_TAPS;
            let distance = reach * f32(ring) / f32(FOCUS_RINGS);
            let step = TAU / f32(count);
            let turn = vec2<f32>(cos(step), sin(step));
            // Each ring starts half a step round from the last, so no two rings' taps line up.
            let start = step * 0.5 * f32(ring - 1u);
            var direction = vec2<f32>(cos(start), sin(start));
            for (var tap = 0u; tap < count; tap++) {
                // Whole texels: a bilinear tap would mix a sharp texel's light into a blurred one's.
                let offset = vec2<i32>(round(direction * distance));
                let taken = textureLoad(texture0, clamp(pixel + offset, vec2<i32>(0), dimensions - 1), 0);
                let cover = select(-taken.a, min(taken.a, behind), taken.a > 0.0);
                // A tap counts as far as its blur reaches, by the texel's own distance, softly over a texel
                // about its edge; a sharp tap (a subject in focus) spreads nowhere.
                let reaches = cover - length(vec2<f32>(offset)) + 0.5;
                let weight = saturate(reaches) * saturate(cover * 4.0) / focus_area(cover);
                total += vec4<f32>(taken.rgb, 1.0) * weight;
                direction = vec2<f32>(
                    direction.x * turn.x - direction.y * turn.y,
                    direction.x * turn.y + direction.y * turn.x,
                );
            }
        }
    }
    return vec4<f32>(total.rgb / total.w, middle.a);
}

@fragment
fn fs_focus_composite(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let dimensions = vec2<i32>(textureDimensions(texture0));
    let size = vec2<f32>(dimensions);
    let sharp = textureLoad(texture0, pixel, 0);
    let radius = focus_radius(size.y);
    let share = focus_share(focus_depth_at(pixel, size));
    // The least blur of the pixel and its four neighbours: under temporal AA the depth jitters by under a
    // pixel while the resolved frame holds still, so a pixel on a sharp edge never blurs on alternate frames.
    // It keeps a pixel-wide band of the blurred side sharp along the edge, under the subject's own line.
    var least = abs(share);
    for (var side = 0u; side < 4u; side++) {
        let axis = select(vec2<i32>(0, 1), vec2<i32>(1, 0), side < 2u);
        let neighbour = clamp(pixel + axis * select(-1, 1, (side & 1u) == 1u), vec2<i32>(0), dimensions - 1);
        least = min(least, abs(focus_share(focus_depth_at(neighbour, size))));
    }
    // The blur's radius in full-size pixels: within half a pixel stays sharp, from one and a half all blurred.
    let amount = saturate(least * radius * 2.0 - 0.5);
    // The gathered half size, up by the four texels around the pixel, each by its bilinear share and how near
    // its blur is to the pixel's own, so a sharp texel doesn't leak into a blurred pixel or the other way.
    let half_size = vec2<i32>(textureDimensions(texture1));
    let position = in.uv * vec2<f32>(half_size) - 0.5;
    let corner = floor(position);
    let fraction = position - corner;
    let blur = share * radius;
    var gathered = vec3<f32>(0.0);
    var total = 0.0;
    for (var tap = 0u; tap < 4u; tap++) {
        let offset = vec2<i32>(i32(tap & 1u), i32(tap >> 1u));
        let texel = clamp(vec2<i32>(corner) + offset, vec2<i32>(0), half_size - 1);
        let taken = textureLoad(texture1, texel, 0);
        let along = select(1.0 - fraction, fraction, vec2<bool>(offset.x == 1, offset.y == 1));
        // Halved for each quarter of a half-size pixel its blur differs by.
        let weight = along.x * along.y * exp2(-4.0 * abs(taken.a - blur)) + 1e-6;
        gathered += taken.rgb * weight;
        total += weight;
    }
    let mixed = mix(sharp.rgb, gathered / total, amount);
    return vec4<f32>(select(sharp.rgb, mixed, amount > 0.0), sharp.a);
}

@fragment
fn fs_soft_backdrop(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let dimensions = vec2<i32>(textureDimensions(texture0));
    let middle = textureLoad(texture0, pixel, 0);
    let reach = post.focus_blur.z * f32(dimensions.y);
    var total = vec4<f32>(middle.rgb * middle.a, middle.a);
    for (var ring = 1u; ring <= 3u; ring++) {
        let count = ring * FOCUS_RING_TAPS;
        let distance = reach * f32(ring) / f32(FOCUS_RINGS);
        let step = TAU / f32(count);
        let turn = vec2<f32>(cos(step), sin(step));
        let start = step * 0.5 * f32(ring - 1u);
        var direction = vec2<f32>(cos(start), sin(start));
        for (var tap = 0u; tap < count; tap++) {
            // Whole texels, so a tap never mixes a character's light into the backdrop's.
            let at = clamp(pixel + vec2<i32>(round(direction * distance)), vec2<i32>(0), dimensions - 1);
            let taken = textureLoad(texture0, at, 0);
            total += vec4<f32>(taken.rgb, 1.0) * taken.a;
            direction = vec2<f32>(
                direction.x * turn.x - direction.y * turn.y,
                direction.x * turn.y + direction.y * turn.x,
            );
        }
    }
    let blurred = select(middle.rgb, total.rgb / max(total.w, 1e-6), total.w > 0.0);
    let mixed = mix(middle.rgb, blurred, middle.a);
    return vec4<f32>(select(middle.rgb, mixed, middle.a > 0.0), middle.a);
}
