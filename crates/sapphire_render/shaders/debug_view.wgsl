// The renderer's debug views (spec §5.12 "Debug views", M1 F5 part 2; the `debug_view` module):
// the passes that draw what a view gathered over the frame, the stand-in fragment stage of the
// views' replays of the main pass, and the wireframe's line indices. `shaders.rs` puts
// frame.wgsl, materials.wgsl and lights.wgsl before it.
//
// A view draws over the finished frame, after post, so no exposure, grade, bloom or temporal AA
// touches its colors: a pixel's color is its value's on the view's scale, at any hour. What it
// reads is its replay of the main pass's stencil and depth, at the scene's size and
// single-sampled, read texel by texel (a smaller scene shows as blocks); the replay drew through
// the main view without temporal AA's jitter (group 1), so the view holds still under TAA. Every
// view with a scale draws its legend: a bar of one cell per value from the scale's bottom to its
// top along the target's bottom-left corner, framed in black.
//
// - Overdraw (`fs_debug_overdraw`): the main pass replayed into a stencil incremented per
//   fragment shaded, the sky counting one where no surface wrote depth. The heat ramp runs over 1
//   to DEBUG_OVERDRAW_SCALE fragments a pixel, log scaled; beyond is white.
// - Light lists (`fs_debug_cluster_lights`, `fs_debug_object_lights`): the local lights the
//   nearest surface walks there, `min(count, frame.lights.w)` of its list, as the anime
//   character, environment and foliage shaders read it. On the clustered path that is the
//   cluster of the pixel's tile and the depth slice of the surfaces' replayed depth there
//   (`light_list`), on the per-object path its instance's list: the surfaces replayed with each
//   instance's count + 1 as the stencil. The ramp runs from 0 to `frame.lights.w`, the most
//   lights a list holds (`LightStats::list_capacity`), log scaled, so one light reads apart from
//   none and red is a full list; a surface without lights is the legend's first grey. Colors are
//   flat but for silhouettes and creases (`debug_edge_shade`); where there is no surface (the
//   sky) the frame shows, dimmed.
// - Wireframe (`fs_debug_wireframe`): every surface's edges replayed as lines over the depth of
//   those that hide them, each line's pixels tagged in the stencil with its shading family, whose
//   tone it takes, over the frame at a fifth of its light. Its line indices come from
//   `cs_debug_lines`.
//
// Fragment stages here have one exit each (WARP's shader compiler crashes on an early return
// under a per-pixel condition).

// The light lists, as the surface shaders declare them (lights.wgsl's header has the layout).
@group(0) @binding(2) var<storage, read> light_lists: array<u32>;

// Group 2: what the view's replay gathered, at the scene's size: its stencil (counts, or tags)
// as unsigned integers, and its depth.
@group(2) @binding(0) var view_stencil: texture_2d<u32>;
@group(2) @binding(1) var view_depth: texture_depth_2d;

// Fragments a pixel at the top of the overdraw view's scale (`debug_view::OVERDRAW_SCALE`).
const DEBUG_OVERDRAW_SCALE: u32 = 16u;
// The light lists' grey for a surface walking no light (`debug_view::NO_LIGHTS`).
const DEBUG_NO_LIGHTS: f32 = 0.15;

struct DebugVaryings {
    @builtin(position) position: vec4<f32>,
    // 0 to 1 across the target, y down.
    @location(0) uv: vec2<f32>,
}

// One triangle over the target at the far plane (reverse-Z depth 0), where the sky's stand-in
// passes the depth test as the sky does: only where no surface wrote depth.
@vertex
fn vs_debug_view(@builtin(vertex_index) index: u32) -> DebugVaryings {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: DebugVaryings;
    out.position = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

// The replays' stand-in fragment stage, whose pipelines write no color: the sky's in the overdraw
// view's replay, and the one the copies that count every fragment a shader runs for draw with in
// place of the real one, which may discard (`Replay::Invocations`). It reads none of the
// vertex stage's outputs, so it fits any of them, and keeps every fragment.
@fragment
fn fs_debug_stand_in() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

// The views' heat ramp at `t` (0 to 1) as a display (sRGB-encoded) color: deep blue, blue,
// cyan, green, yellow, orange, red. Mirrors `debug_view::heat`.
fn debug_heat(t: f32) -> vec3<f32> {
    var stops = array<vec3<f32>, 7>(
        vec3<f32>(0.06, 0.06, 0.30),
        vec3<f32>(0.10, 0.30, 0.95),
        vec3<f32>(0.10, 0.75, 0.90),
        vec3<f32>(0.20, 0.85, 0.30),
        vec3<f32>(0.95, 0.90, 0.15),
        vec3<f32>(1.00, 0.50, 0.05),
        vec3<f32>(0.85, 0.05, 0.05),
    );
    let x = saturate(t) * 6.0;
    let i = min(u32(x), 5u);
    return mix(stops[i], stops[i + 1u], x - f32(i));
}

// Where `count` lights in a list of at most `capacity` sit on the ramp. Mirrors
// `debug_view::light_position`.
fn debug_light_position(count: u32, capacity: u32) -> f32 {
    return log2(1.0 + f32(count)) / log2(1.0 + f32(max(capacity, 1u)));
}

// Where `fragments` a pixel sit on the overdraw ramp. Mirrors `debug_view::overdraw_position`.
fn debug_overdraw_position(fragments: u32) -> f32 {
    return log2(f32(max(fragments, 1u))) / log2(f32(DEBUG_OVERDRAW_SCALE));
}

// A display color as the linear light the target's sRGB encoding turns back into it.
fn debug_linear(display: vec3<f32>) -> vec3<f32> {
    let low = display / 12.92;
    let high = pow((display + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, display <= vec3<f32>(0.04045));
}

// The scene texel under `uv`, in a texture of `size`.
fn debug_texel(uv: vec2<f32>, size: vec2<u32>) -> vec2<i32> {
    return vec2<i32>(min(vec2<u32>(uv * vec2<f32>(size)), size - 1u));
}

// The legend: a bar of `cells` cells along the bottom-left corner, a third of the target's width
// and a fortieth of its height, framed by a pixel of black, cells parted by one where they are
// at least four pixels wide. The cell under `uv` (0 is the scale's bottom), -1 on the frame or a
// parting, -2 elsewhere. `pixel` is `fwidth(uv)`.
fn debug_legend_cell(uv: vec2<f32>, pixel: vec2<f32>, cells: u32) -> i32 {
    let lower = vec2<f32>(0.02, 0.94);
    let upper = vec2<f32>(0.35, 0.965);
    let x = (uv.x - lower.x) / (upper.x - lower.x) * f32(cells);
    let cell_pixels = (upper.x - lower.x) / (f32(cells) * pixel.x);
    let parting = cell_pixels >= 4.0 && fract(x) * cell_pixels < 1.0 && x >= 1.0;
    let outside = any(uv < lower - pixel) || any(uv > upper + pixel);
    let frame = any(uv < lower) || any(uv > upper) || parting;
    let cell = min(i32(x), i32(cells) - 1);
    return select(select(cell, -1, frame), -2, outside);
}

// How much of a flat view color shows at `texel`, whose replayed depth is `depth`: all of it
// inside a surface, 55% along its silhouettes and creases, where the depth bends between the
// texel and its neighbours. Reverse-Z depth (1/z) is affine across a plane's pixels, so its
// second difference is zero on any plane however steep, and large where surfaces meet.
fn debug_edge_shade(texel: vec2<i32>, depth: f32) -> f32 {
    let last = vec2<i32>(textureDimensions(view_depth)) - 1;
    let left = textureLoad(view_depth, clamp(texel - vec2<i32>(1, 0), vec2<i32>(0), last), 0);
    let right = textureLoad(view_depth, clamp(texel + vec2<i32>(1, 0), vec2<i32>(0), last), 0);
    let up = textureLoad(view_depth, clamp(texel - vec2<i32>(0, 1), vec2<i32>(0), last), 0);
    let down = textureLoad(view_depth, clamp(texel + vec2<i32>(0, 1), vec2<i32>(0), last), 0);
    let bend = max(abs(left + right - 2.0 * depth), abs(up + down - 2.0 * depth));
    return select(1.0, 0.55, bend > 0.02 * depth);
}

// A light-list view's color: the legend's cell or its frame, else `count` lights on a
// `surface` shaded `shade` (`debug_edge_shade`), or the frame dimmed where there is no surface.
fn debug_lights(legend: i32, count: u32, capacity: u32, surface: bool, shade: f32) -> vec4<f32> {
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.75);
    if surface {
        let heat = debug_heat(debug_light_position(count, capacity));
        let flat = select(vec3<f32>(DEBUG_NO_LIGHTS), heat, count > 0u);
        color = vec4<f32>(debug_linear(flat * shade), 1.0);
    }
    if legend >= 0 {
        let cell = u32(legend);
        let heat = debug_heat(debug_light_position(cell, capacity));
        color = vec4<f32>(debug_linear(select(vec3<f32>(DEBUG_NO_LIGHTS), heat, cell > 0u)), 1.0);
    }
    if legend == -1 {
        color = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return color;
}

@fragment
fn fs_debug_overdraw(in: DebugVaryings) -> @location(0) vec4<f32> {
    // One cell per count from 1 to the scale's top, then one for more.
    let legend = debug_legend_cell(in.uv, fwidth(in.uv), DEBUG_OVERDRAW_SCALE + 1u);
    let counted = textureLoad(view_stencil, debug_texel(in.uv, textureDimensions(view_stencil)), 0).r;
    let fragments = select(counted, u32(max(legend, 0)) + 1u, legend >= 0);
    var color = debug_heat(debug_overdraw_position(fragments));
    if fragments > DEBUG_OVERDRAW_SCALE {
        color = vec3<f32>(1.0);
    }
    if legend == -1 {
        color = vec3<f32>(0.0);
    }
    return vec4<f32>(debug_linear(color), 1.0);
}

@fragment
fn fs_debug_cluster_lights(in: DebugVaryings) -> @location(0) vec4<f32> {
    let capacity = frame.lights.w;
    let legend = debug_legend_cell(in.uv, fwidth(in.uv), capacity + 1u);
    let texel = debug_texel(in.uv, textureDimensions(view_depth));
    let depth = textureLoad(view_depth, texel, 0);
    // The list a surface's pixel there walks: its tile and depth slice, from the pixel's center
    // and depth as its `@builtin(position)` holds them.
    let list = light_list(vec4<f32>(vec2<f32>(texel) + 0.5, depth, 1.0), 0u);
    let surface = depth > 0.0;
    var count = 0u;
    if surface && frame.lights.y == LIGHT_LISTS_CLUSTERS {
        count = min(light_lists[list * frame.lights.z], capacity);
    }
    return debug_lights(legend, count, capacity, surface, debug_edge_shade(texel, depth));
}

@fragment
fn fs_debug_object_lights(in: DebugVaryings) -> @location(0) vec4<f32> {
    let capacity = frame.lights.w;
    let legend = debug_legend_cell(in.uv, fwidth(in.uv), capacity + 1u);
    let texel = debug_texel(in.uv, textureDimensions(view_stencil));
    // 0 where no surface was drawn, else its instance's list's count + 1.
    let tag = textureLoad(view_stencil, texel, 0).r;
    let shade = debug_edge_shade(texel, textureLoad(view_depth, texel, 0));
    return debug_lights(legend, max(tag, 1u) - 1u, capacity, tag > 0u, shade);
}

@fragment
fn fs_debug_wireframe(in: DebugVaryings) -> @location(0) vec4<f32> {
    // Each shading family's tone, by its tag - 1 (`debug_view::WIREFRAME_TONES`): the
    // environment's, foliage's, fog cards' and the anime characters'.
    var tones = array<vec3<f32>, 4>(
        vec3<f32>(0.45, 0.85, 1.0),
        vec3<f32>(0.55, 1.0, 0.4),
        vec3<f32>(0.8, 0.6, 1.0),
        vec3<f32>(1.0, 0.72, 0.3),
    );
    // 0 where no line was drawn, else its family's tag.
    let tag = textureLoad(view_stencil, debug_texel(in.uv, textureDimensions(view_stencil)), 0).r;
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.8);
    if tag > 0u {
        color = vec4<f32>(debug_linear(tones[min(tag, 4u) - 1u]), 1.0);
    }
    return color;
}

// The wireframe's line indices (`debug_view::LineIndices`): the mesh pool's indices in use,
// copied, and each triangle's three edges, two indices each at twice the triangle's position.
// Bindings past the frame group's, which this file's other entry points use.
@group(0) @binding(20) var<storage, read> debug_triangles: array<u32>;
@group(0) @binding(21) var<storage, read_write> debug_lines: array<u32>;

// One triangle per invocation; those past the copied indices write past every draw's range.
@compute @workgroup_size(128)
fn cs_debug_lines(@builtin(global_invocation_id) id: vec3<u32>) {
    let first = id.x * 3u;
    if first + 2u < arrayLength(&debug_triangles) {
        let a = debug_triangles[first];
        let b = debug_triangles[first + 1u];
        let c = debug_triangles[first + 2u];
        let line = id.x * 6u;
        debug_lines[line] = a;
        debug_lines[line + 1u] = b;
        debug_lines[line + 2u] = b;
        debug_lines[line + 3u] = c;
        debug_lines[line + 4u] = c;
        debug_lines[line + 5u] = a;
    }
}
