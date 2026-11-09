// The on-screen interface over the finished frame (M2 I2, `ui.rs`): quads over a signed distance
// field atlas, after post, into the target, which encodes on write. A program of its own: it reads
// no scene data.
//
// A texel holds 0.5 + distance / (2 × spread), so 0.5 is a shape's edge at any scale or shear, and
// its screen-space slope gives a one-pixel antialiased edge wherever the quad lands. A glyph page's
// texel is coverage instead, already antialiased, which a negative softness says, and an inset's
// (M3 U6) a picture, linear color the quad's color tints, which a softness of -2 or less says
// (`UI_PICTURE`). Colors arrive
// display-encoded and are decoded here, so an opaque color comes out on exactly its code through
// the target's encode, and everything blends in linear light.

struct UiUniforms {
    // Points to clip space: 2 / the viewport in points.
    points_to_clip: vec2<f32>,
    padding: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: UiUniforms;
@group(0) @binding(1) var ui_atlas: texture_2d<f32>;
@group(0) @binding(2) var ui_sampler: sampler;

// `sapphire_look::UiQuad`, one instance per quad.
struct UiQuadInput {
    @location(0) origin: vec2<f32>,
    @location(1) axis_x: vec2<f32>,
    @location(2) axis_y: vec2<f32>,
    @location(3) uv: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) outline_color: vec4<f32>,
    // Its edge, softness, outline and sweep.
    @location(6) shape: vec4<f32>,
    @location(7) clip: vec2<f32>,
    @location(8) slice: f32,
    @location(9) color_end: vec4<f32>,
    @location(10) gradient: vec3<f32>,
};

struct UiVertex {
    @builtin(position) position: vec4<f32>,
    // Across and down the quad, 0 to 1 along its own axes, so a slant carries everything with it.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) uv: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) outline_color: vec4<f32>,
    @location(4) @interpolate(flat) shape: vec4<f32>,
    @location(5) @interpolate(flat) clip_slice: vec3<f32>,
    @location(6) @interpolate(flat) color_end: vec4<f32>,
    @location(7) @interpolate(flat) gradient: vec3<f32>,
};

fn ui_linear(display: vec3<f32>) -> vec3<f32> {
    let low = display / 12.92;
    let high = pow((display + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, display <= vec3<f32>(0.04045));
}

@vertex
fn vs_ui(@builtin(vertex_index) index: u32, quad: UiQuadInput) -> UiVertex {
    // A triangle strip of the quad's four corners.
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let position = quad.origin + quad.axis_x * corner.x + quad.axis_y * corner.y;
    let clip = position * ui.points_to_clip - vec2<f32>(1.0);
    var out: UiVertex;
    out.position = vec4<f32>(clip.x, -clip.y, 0.0, 1.0);
    out.local = corner;
    out.uv = quad.uv;
    out.color = vec4<f32>(ui_linear(quad.color.rgb), quad.color.a);
    out.outline_color = vec4<f32>(ui_linear(quad.outline_color.rgb), quad.outline_color.a);
    out.shape = quad.shape;
    out.clip_slice = vec3<f32>(quad.clip, quad.slice);
    out.color_end = vec4<f32>(ui_linear(quad.color_end.rgb), quad.color_end.a);
    out.gradient = quad.gradient;
    return out;
}

// Where `x` across the quad reads across its cell: a three-slice keeps each end's half of the cell
// at its own size and stretches the middle column between them.
fn ui_slice(x: f32, cap: f32) -> f32 {
    let safe_cap = max(cap, 1e-6);
    let left = x / safe_cap * 0.5;
    let right = 1.0 - (1.0 - x) / safe_cap * 0.5;
    let sliced = select(select(0.5, right, x > 1.0 - safe_cap), left, x < safe_cap);
    return select(x, sliced, cap > 0.0);
}

// Coverage of `value` past `edge` over `width` of field, centered on the edge.
fn ui_ramp(value: f32, edge: f32, width: f32) -> f32 {
    return clamp((value - edge) / width + 0.5, 0.0, 1.0);
}

// One exit, and everything a derivative reads worked out first, on every path (CLAUDE.md's rules
// for WARP and Metal).
@fragment
fn fs_ui(in: UiVertex) -> @location(0) vec4<f32> {
    let edge = in.shape.x;
    let softness = in.shape.y;
    let outline = in.shape.z;
    let sweep = in.shape.w;

    let across = ui_slice(in.local.x, in.clip_slice.z);
    let uv = mix(in.uv.xy, in.uv.zw, vec2<f32>(across, in.local.y));
    let texel = textureSampleLevel(ui_atlas, ui_sampler, uv, 0.0);
    let field = texel.r;

    // A pixel's worth of field, so edges stay one pixel wide at any scale; softness widens them.
    let slope = length(vec2<f32>(dpdx(field), dpdy(field)));
    let width = max(max(slope, softness * 2.0), 1e-4);
    let is_coverage = softness < 0.0;
    let inner = select(ui_ramp(field, edge, width), field, is_coverage);
    let outer = select(ui_ramp(field, edge - outline, width), field, is_coverage);

    // The cut along the quad's width, along its slant. A range that reaches an end isn't cut there,
    // so a whole quad is untouched.
    let pixel_x = max(length(vec2<f32>(dpdx(in.local.x), dpdy(in.local.x))), 1e-6);
    let from_start = clamp((in.local.x - in.clip_slice.x) / pixel_x + 0.5, 0.0, 1.0);
    let from_end = clamp((in.clip_slice.y - in.local.x) / pixel_x + 0.5, 0.0, 1.0);
    let cut = select(from_start, 1.0, in.clip_slice.x <= 0.0) * select(from_end, 1.0, in.clip_slice.y >= 1.0);

    // The sweep: a share of a turn clockwise from the quad's top, around its middle.
    let from_middle = in.local - vec2<f32>(0.5);
    let turn = fract(atan2(from_middle.x, -from_middle.y) / 6.2831853);
    let reach = length(vec2<f32>(dpdx(from_middle.x), dpdy(from_middle.x)))
        + length(vec2<f32>(dpdx(from_middle.y), dpdy(from_middle.y)));
    let pixel_turn = max(reach / (6.2831853 * max(length(from_middle), 1e-3)), 1e-5);
    let inside_sweep = clamp((sweep - turn) / pixel_turn + 0.5, 0.0, 1.0)
        + clamp((turn - 1.0) / pixel_turn + 0.5, 0.0, 1.0);
    let swept = select(1.0, min(inside_sweep, 1.0), sweep > 0.0);

    // The fill's gradient, in linear light. A share of 0 is the near color exactly.
    let share = clamp(dot(in.gradient.xy, in.local) + in.gradient.z, 0.0, 1.0);
    let color = mix(in.color, in.color_end, share);

    let mask = cut * swept;
    let fill = color.a * inner;
    let line = in.outline_color.a * (outer - inner);
    let rgb = color.rgb * fill + in.outline_color.rgb * line;
    let shape = vec4<f32>(rgb, fill + line);
    // A picture is opaque light, premultiplied by its alpha, which the color tints and fades.
    let picture = vec4<f32>(texel.rgb * color.rgb, texel.a) * color.a;
    return select(shape, picture, softness < -1.5) * mask;
}
