// Debug draw's lines (spec §5.20, M2 T1; the `debug_draw` module): one instanced quad a line, over
// the finished frame after post, each line a capsule of a fixed width in pixels with an analytic
// edge, so it reads and holds still at any tier, render scale and resolution. No exposure, grade,
// bloom or temporal AA touches its color or position. A depth-tested line compares itself with
// the scene prepass's depth here, in the fragment stage, since that depth has the scene's size
// and may be smaller than the target's, which no depth attachment could be. This program stands
// alone.

struct DebugDrawUniforms {
    // The main view without temporal AA's jitter.
    clip_from_world: mat4x4<f32>,
    // The target's size, in pixels.
    target_size: vec2<f32>,
    // Scene depth texels per target pixel: the render scale, per axis as it rounds.
    scene_scale: vec2<f32>,
    // Half a line's width, in target pixels.
    half_width: f32,
    // The projection's near plane: reverse-Z depth is `near / distance`.
    near: f32,
    padding: vec2<f32>,
}

@group(0) @binding(0) var<uniform> debug_draw: DebugDrawUniforms;
// The scene prepass's depth, at the scene's size: opaque and cut-out surfaces and outline hulls.
@group(0) @binding(1) var debug_scene_depth: texture_depth_2d;

// The `flags` bit of a line drawn over everything (`debug_draw::ON_TOP`).
const DEBUG_DRAW_ON_TOP: u32 = 1u;
// How far behind a surface a depth-tested line still shows, in meters and as a share of its
// distance, so a line lying on a surface doesn't fight it under depth's precision and temporal
// AA's jitter.
const DEBUG_DRAW_MARGIN: f32 = 0.01;
const DEBUG_DRAW_MARGIN_SHARE: f32 = 0.002;
// Past this, a line's length on screen counts as none, and it draws as a dot.
const DEBUG_DRAW_MIN_LENGTH: f32 = 1e-4;

struct DebugLineInput {
    // 0 to 3, a triangle strip: bit 0 picks the line's end, bit 1 its side.
    @builtin(vertex_index) corner: u32,
    @location(0) start: vec3<f32>,
    // Display (sRGB-encoded), with straight alpha.
    @location(1) color: vec4<f32>,
    @location(2) end: vec3<f32>,
    @location(3) flags: u32,
}

struct DebugLineVarying {
    @builtin(position) position: vec4<f32>,
    // Pixels along the line from its start, and from its middle sideways. The quad is a rectangle
    // on screen, so these interpolate linearly in screen space.
    @location(0) @interpolate(linear) along: f32,
    @location(1) @interpolate(linear) side: f32,
    // The line's length on screen, in pixels.
    @location(2) @interpolate(flat) span: f32,
    // Linear, with straight alpha.
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) flags: u32,
}

fn debug_linear(display: vec3<f32>) -> vec3<f32> {
    let low = display / 12.92;
    let high = pow((display + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, display <= vec3<f32>(0.04045));
}

@vertex
fn vs_debug_line(line: DebugLineInput) -> DebugLineVarying {
    var a = debug_draw.clip_from_world * vec4<f32>(line.start, 1.0);
    var b = debug_draw.clip_from_world * vec4<f32>(line.end, 1.0);
    // What is in front of the near plane (z <= w) alone has a place on screen, so the line is cut
    // there before anything divides by w: an end behind the camera would mirror through it. A line
    // wholly behind it clips away, its quad along with it.
    let front_a = a.w - a.z;
    let front_b = b.w - b.z;
    if front_a < 0.0 && front_b > 0.0 {
        a = mix(a, b, front_a / (front_a - front_b));
    } else if front_b < 0.0 && front_a > 0.0 {
        b = mix(b, a, front_b / (front_b - front_a));
    }
    let half_size = debug_draw.target_size * 0.5;
    let on_screen_a = a.xy / max(a.w, 1e-6) * half_size;
    let on_screen_b = b.xy / max(b.w, 1e-6) * half_size;
    let delta = on_screen_b - on_screen_a;
    let span = length(delta);
    let direction = select(vec2<f32>(1.0, 0.0), delta / max(span, DEBUG_DRAW_MIN_LENGTH), span > DEBUG_DRAW_MIN_LENGTH);
    let normal = vec2<f32>(-direction.y, direction.x);

    let at_end = (line.corner & 1u) != 0u;
    let side = select(-1.0, 1.0, (line.corner & 2u) != 0u);
    // The quad reaches past the line by its round cap and its one pixel of soft edge.
    let reach = debug_draw.half_width + 1.0;
    let forward = select(-reach, reach, at_end);
    let offset = direction * forward + normal * side * reach;
    let corner = select(a, b, at_end);

    var out: DebugLineVarying;
    // The offset is in pixels, so it scales with w to stay one size on screen; z/w, the depth the
    // fragment stage compares, stays the line's.
    out.position = vec4<f32>(corner.xy + offset / half_size * corner.w, corner.zw);
    out.along = select(0.0, span, at_end) + forward;
    out.side = side * reach;
    out.span = span;
    out.color = vec4<f32>(debug_linear(line.color.rgb), line.color.a);
    out.flags = line.flags;
    return out;
}

// 1 where `texel`'s surface doesn't hide what is `distance` away, else 0.
fn debug_line_shows(texel: vec2<i32>, distance: f32) -> f32 {
    let last = vec2<i32>(textureDimensions(debug_scene_depth)) - 1;
    let depth = textureLoad(debug_scene_depth, clamp(texel, vec2<i32>(0), last), 0);
    // Where nothing wrote depth (0, the sky) this is farther than anything drawn.
    let surface = debug_draw.near / max(depth, 1e-9);
    return select(0.0, 1.0, distance <= surface * (1.0 + DEBUG_DRAW_MARGIN_SHARE) + DEBUG_DRAW_MARGIN);
}

// How much of a depth-tested line shows at `position`: the depth test filtered over the four
// nearest scene texels, so where the scene is smaller than the target an edge doesn't step by whole
// texels. A pixel at render scale 1 lands on one texel's middle, and that test alone decides.
fn debug_line_visibility(position: vec4<f32>) -> f32 {
    let distance = debug_draw.near / max(position.z, 1e-9);
    let at = position.xy * debug_draw.scene_scale - 0.5;
    let base = floor(at);
    let blend = at - base;
    let texel = vec2<i32>(base);
    let upper = mix(
        debug_line_shows(texel, distance),
        debug_line_shows(texel + vec2<i32>(1, 0), distance),
        blend.x,
    );
    let lower = mix(
        debug_line_shows(texel + vec2<i32>(0, 1), distance),
        debug_line_shows(texel + vec2<i32>(1, 1), distance),
        blend.x,
    );
    return mix(upper, lower, blend.y);
}

// One exit, and no discard: what doesn't show blends as nothing.
@fragment
fn fs_debug_line(in: DebugLineVarying) -> @location(0) vec4<f32> {
    let beyond = max(max(-in.along, in.along - in.span), 0.0);
    let from_middle = length(vec2<f32>(beyond, in.side));
    var coverage = saturate(debug_draw.half_width + 0.5 - from_middle);
    if (in.flags & DEBUG_DRAW_ON_TOP) == 0u && coverage > 0.0 {
        coverage *= debug_line_visibility(in.position);
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
