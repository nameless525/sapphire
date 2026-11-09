// The on-screen interface over the finished frame (M2 I2, `ui.rs`): quads over a signed distance
// field atlas, after post, into the target, which encodes on write. A program of its own: it reads
// no scene data.
//
// A texel holds 0.5 + distance / (2 × spread), so 0.5 is a shape's edge at any scale or shear, and
// its screen-space slope gives a one-pixel antialiased edge wherever the quad lands. A glyph page's
// texel is coverage instead, already antialiased. A display face's field (M3 U8) has three channels
// whose median is the distance, which keeps corners sharp, and the true distance in alpha, smooth
// across the channels' seams, whose slope sets the edge's width and which an outline follows. An
// inset's (M3 U6) is a picture, linear color the quad's color tints. A quad's flags say which. Colors
// arrive in linear light, decoded once per quad from what a designer picked, so an opaque color comes
// out on exactly its code through the target's encode, and everything blends in linear light.
//
// What a pixel pays for (M3, `ui.rs`): both stages read a quad from the frame's storage buffer, and only
// its corner and its index pass between them, since a stage pays for every input it declares on every
// pixel. The quad's flags say which of its parts it uses, and every pixel of a quad agrees, so a branch
// on them costs nothing; a part skipped is what it would have worked out. A solid quad, inside its
// shape throughout, reads no field.

struct UiUniforms {
    // Points to clip space: 2 / the viewport in points.
    points_to_clip: vec2<f32>,
    padding: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: UiUniforms;
@group(0) @binding(1) var ui_atlas: texture_2d<f32>;
@group(0) @binding(2) var ui_sampler: sampler;

// `GpuUiQuad` in `ui.rs`, field by field.
struct UiQuad {
    origin: vec2<f32>,
    axis_x: vec2<f32>,
    axis_y: vec2<f32>,
    clip: vec2<f32>,
    uv: vec4<f32>,
    color: vec4<f32>,
    color_end: vec4<f32>,
    outline_color: vec4<f32>,
    // Its edge, softness, outline and sweep.
    shape: vec4<f32>,
    gradient: vec3<f32>,
    slice: f32,
    flags: u32,
    padding: array<u32, 3>,
};

// `ui.rs`'s flags.
const UI_SOLID: u32 = 1u;
const UI_SLICED: u32 = 2u;
const UI_OUTLINED: u32 = 4u;
const UI_GRADIENT: u32 = 8u;
const UI_CUT: u32 = 16u;
const UI_SWEPT: u32 = 32u;
const UI_COVERAGE_TEXELS: u32 = 64u;
const UI_MULTI_TEXELS: u32 = 128u;
const UI_PICTURE_TEXELS: u32 = 256u;

@group(1) @binding(0) var<storage, read> ui_quads: array<UiQuad>;

struct UiVertex {
    @builtin(position) position: vec4<f32>,
    // Across and down the quad, 0 to 1 along its own axes, so a slant carries everything with it.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) quad: u32,
};

// `index` is the quad's place in the frame's buffer: a draw's instances come in through a vertex
// stream of indices sliced at its first quad, since a draw can't start its instances past 0 on the
// iOS Simulator.
@vertex
fn vs_ui(@builtin(vertex_index) corner_index: u32, @location(0) index: u32) -> UiVertex {
    let quad = &ui_quads[index];
    // A triangle strip of the quad's four corners.
    let corner = vec2<f32>(f32(corner_index & 1u), f32(corner_index >> 1u));
    let position = (*quad).origin + (*quad).axis_x * corner.x + (*quad).axis_y * corner.y;
    let clip = position * ui.points_to_clip - vec2<f32>(1.0);
    var out: UiVertex;
    out.position = vec4<f32>(clip.x, -clip.y, 0.0, 1.0);
    out.local = corner;
    out.quad = index;
    return out;
}

fn ui_median(rgb: vec3<f32>) -> f32 {
    return max(min(rgb.r, rgb.g), min(max(rgb.r, rgb.g), rgb.b));
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

// One exit, and every derivative taken where every pixel of the quad takes it (CLAUDE.md's rules for
// WARP and Metal): the branches only choose values.
@fragment
fn fs_ui(in: UiVertex) -> @location(0) vec4<f32> {
    let quad = &ui_quads[in.quad];
    let flags = (*quad).flags;
    let solid = (flags & UI_SOLID) != 0u;

    // The texels, which a solid quad has no need of: its field would read 1, with no slope.
    var texel = vec4<f32>(1.0);
    if !solid {
        var across = in.local.x;
        if (flags & UI_SLICED) != 0u {
            across = ui_slice(in.local.x, (*quad).slice);
        }
        let uv = mix((*quad).uv.xy, (*quad).uv.zw, vec2<f32>(across, in.local.y));
        texel = textureSampleLevel(ui_atlas, ui_sampler, uv, 0.0);
    }
    // A display face's field: the median of three is the edge, and the true distance its slope.
    let multi = (flags & UI_MULTI_TEXELS) != 0u;
    var field = texel.r;
    var distance = texel.r;
    if multi {
        field = ui_median(texel.rgb);
        distance = texel.a;
    }
    let distance_slope = vec2<f32>(dpdx(distance), dpdy(distance));
    let local_x = vec2<f32>(dpdx(in.local.x), dpdy(in.local.x));
    let from_middle = in.local - vec2<f32>(0.5);
    let middle_x = vec2<f32>(dpdx(from_middle.x), dpdy(from_middle.x));
    let middle_y = vec2<f32>(dpdx(from_middle.y), dpdy(from_middle.y));

    // The fill's gradient, in linear light. A share of 0 is the near color exactly.
    var color = (*quad).color;
    if (flags & UI_GRADIENT) != 0u {
        let gradient = (*quad).gradient;
        let share = clamp(dot(gradient.xy, in.local) + gradient.z, 0.0, 1.0);
        color = mix(color, (*quad).color_end, share);
    }

    // A solid quad is filled through: both its ramps are 1, and its outline adds nothing.
    var fill = color.a;
    var rgb = color.rgb * fill;
    var alpha = fill;
    if !solid {
        let shape = (*quad).shape;
        let edge = shape.x;
        let softness = shape.y;
        // A pixel's worth of field, so edges stay one pixel wide at any scale; softness widens them.
        let width = max(max(length(distance_slope), softness * 2.0), 1e-4);
        let is_coverage = (flags & UI_COVERAGE_TEXELS) != 0u;
        let inner = select(ui_ramp(field, edge, width), field, is_coverage);
        fill = color.a * inner;
        rgb = color.rgb * fill;
        alpha = fill;
        if (flags & UI_OUTLINED) != 0u {
            let outline_color = (*quad).outline_color;
            var outer = select(ui_ramp(field, edge - shape.z, width), field, is_coverage);
            if multi {
                // A display face's outline follows the true distance, which rounds its outer corners,
                // and never falls inside the fill where the two disagree.
                outer = max(ui_ramp(distance, edge - shape.z, width), inner);
            }
            let line = outline_color.a * (outer - inner);
            rgb = color.rgb * fill + outline_color.rgb * line;
            alpha = fill + line;
        }
    }

    // The cut along the quad's width, along its slant. A range that reaches an end isn't cut there,
    // so a whole quad is untouched.
    var cut = 1.0;
    if (flags & UI_CUT) != 0u {
        let clip = (*quad).clip;
        let pixel_x = max(length(local_x), 1e-6);
        let from_start = clamp((in.local.x - clip.x) / pixel_x + 0.5, 0.0, 1.0);
        let from_end = clamp((clip.y - in.local.x) / pixel_x + 0.5, 0.0, 1.0);
        cut = select(from_start, 1.0, clip.x <= 0.0) * select(from_end, 1.0, clip.y >= 1.0);
    }

    // The sweep: a share of a turn clockwise from the quad's top, around its middle.
    var swept = 1.0;
    if (flags & UI_SWEPT) != 0u {
        let sweep = (*quad).shape.w;
        let turn = fract(atan2(from_middle.x, -from_middle.y) / 6.2831853);
        let reach = length(middle_x) + length(middle_y);
        let pixel_turn = max(reach / (6.2831853 * max(length(from_middle), 1e-3)), 1e-5);
        let inside_sweep = clamp((sweep - turn) / pixel_turn + 0.5, 0.0, 1.0)
            + clamp((turn - 1.0) / pixel_turn + 0.5, 0.0, 1.0);
        swept = min(inside_sweep, 1.0);
    }

    var shaded = vec4<f32>(rgb, alpha);
    if (flags & UI_PICTURE_TEXELS) != 0u {
        // A picture is opaque light, premultiplied by its alpha, which the color tints and fades.
        shaded = vec4<f32>(texel.rgb * color.rgb, texel.a) * color.a;
    }
    return shaded * (cut * swept);
}
