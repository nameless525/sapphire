// Post's distorted composites (M5 L6; `sapphire_render::post`, and the `vfx` module's `distortion` for the
// offsets): the fused pass of each variant, reading the frame through the effects' bends. Appended to post.wgsl,
// whose code is unchanged: a frame without a bend draws post.wgsl's own entry points, so these never run there.
//
// `distortion_offsets` holds where each pixel reads the frame from, in frame heights, at a share of post's
// input, read filtered at the screen's pixel. A pixel that bends reads the frame filtered at the moved place, and
// its bloom and diffusion glow from there too, so a bright edge bends with its glow; the vignette, the lens flare,
// gradient layers, the colour effects and the CRT stay on the screen, so a flat frame bends to itself and no bend
// can brighten it. A character never bends: a pixel whose own mask isn't wholly world, or whose moved place
// isn't, reads as it would without the bend, so nothing of a character is smeared over the world and no
// character pixel moves. Each mask is read over two pixels round the place, so a silhouette's
// antialiasing, temporal AA's jitter of the mask and its resolve's spread past it never bend either. Near the frame's edge the bend fades out over `BEND_EDGE`, and a read is held inside.

@group(1) @binding(6) var distortion_offsets: texture_2d<f32>;
// Scene colour, whose alpha is the character mask the bends keep characters out of. `mask_source` stays what the
// variant's own composite reads, which may be the frame itself, so a pixel that doesn't bend composites exactly
// as it would without bends.
@group(1) @binding(7) var bend_mask: texture_2d<f32>;

// Over how much of the frame's height, from its edge, a bend fades to nothing.
const BEND_EDGE: f32 = 0.05;
// A world pixel's mask: scene colour's alpha is 1 there and 0 on characters and their outlines.
const BEND_WORLD: f32 = 0.999;
// The least a bend moves a pixel, in output pixels, to read anywhere but its own place: below it, the pixel reads
// as it would without the bend, bit for bit.
const BEND_LEAST: f32 = 1.0 / 64.0;

struct Bend {
    // Where the pixel reads the frame, its bloom and its glow.
    uv: vec2<f32>,
    // How far that is from its own place, in output pixels.
    pixels: vec2<f32>,
    bent: bool,
}

// The least of the character mask over two of its texels each way round `uv`: four filtered reads a texel and a
// quarter out on the diagonals, each weighing the two texels it falls between. Two, as temporal AA's resolve
// spreads a character's colour that far past the mask.
fn bend_world(uv: vec2<f32>) -> f32 {
    let step = 1.25 / vec2<f32>(textureDimensions(bend_mask));
    let a = textureSampleLevel(bend_mask, linear_clamp, uv + vec2<f32>(-step.x, -step.y), 0.0).a;
    let b = textureSampleLevel(bend_mask, linear_clamp, uv + vec2<f32>(step.x, -step.y), 0.0).a;
    let c = textureSampleLevel(bend_mask, linear_clamp, uv + vec2<f32>(-step.x, step.y), 0.0).a;
    let d = textureSampleLevel(bend_mask, linear_clamp, uv + vec2<f32>(step.x, step.y), 0.0).a;
    return min(min(a, b), min(c, d));
}

// The bend at the screen's `here` (texture coordinates) for a pixel showing the frame at `uv`: moved by the
// offsets there, faded near the edge, held inside the frame, and none where either end isn't wholly world.
fn bend_at(here: vec2<f32>, uv: vec2<f32>) -> Bend {
    let aspect = post.frame.x;
    let offset = textureSampleLevel(distortion_offsets, linear_clamp, here, 0.0).xy;
    let edge = min(min(here.x, 1.0 - here.x) * aspect, min(here.y, 1.0 - here.y));
    let moved = offset * saturate(edge / BEND_EDGE) * vec2<f32>(1.0 / aspect, 1.0);
    let half_pixel = 0.5 * post.frame.yz;
    let there = clamp(uv + moved, half_pixel, 1.0 - half_pixel);
    let own_world = bend_world(uv);
    let there_world = bend_world(there);
    let pixels = (there - uv) / post.frame.yz;
    var bend: Bend;
    bend.bent = own_world >= BEND_WORLD && there_world >= BEND_WORLD && length(pixels) >= BEND_LEAST;
    bend.uv = select(uv, there, bend.bent);
    bend.pixels = select(vec2<f32>(0.0), pixels, bend.bent);
    return bend;
}

// The scene through `bend`, before exposure: filtered at its place where it bends, else as `scene_at` reads it.
fn bent_scene(position: vec2<f32>, uv: vec2<f32>, bend: Bend) -> vec3<f32> {
    var scene = scene_at(position, uv);
    if bend.bent {
        scene = textureSampleLevel(texture0, linear_clamp, bend.uv, 0.0).rgb;
    }
    return scene;
}

// The plain variant's distorted form: `fs_post`, with the scene and the bloom read through the bend.
@fragment
fn fs_post_distorted(in: FullscreenVaryings) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(texture0));
    let bend = bend_at(in.uv, in.uv);
    var color = bent_scene(in.position.xy, in.uv, bend) * post.composite.x;
    color += textureSampleLevel(texture1, linear_clamp, bend.uv, 0.0).rgb * post.composite.y;
    color *= vignette(in.uv, size);
    return vec4<f32>(srgb_to_linear(graded(color)), 1.0);
}

// `composited`, with the bloom and the diffusion glow read at `bent`, where the scene was: the rest stays on
// `in`'s place.
fn composited_bent(scene: vec3<f32>, in: CompositeVaryings, bent: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    var color = scene;
    color += textureSampleLevel(texture1, linear_clamp, bent, 0.0).rgb * post.composite.y;
    if POST_LAYERS {
        if in.flare > 0.0 {
            color += lens_flare(in.uv, in.flare);
        }
        color *= tinted_vignette(in.uv, size);
    } else {
        color *= vignette(in.uv, size);
    }
    var display = graded(color);
    if POST_LAYERS {
        if post.diffusion.w > 0.0 {
            display = screen(display, diffusion_glow(bent));
        }
        display = gradient_layers(display, in.uv);
    }
    return display;
}

// The exposed scene without the lens effects, through `bend`: as `plain_scene` where it doesn't bend; where it
// does, filtered at its place, unsharpened.
fn bent_plain_scene(position: vec2<f32>, uv: vec2<f32>, bend: Bend) -> vec3<f32> {
    if bend.bent {
        return textureSampleLevel(texture0, linear_clamp, bend.uv, 0.0).rgb * post.composite.x;
    }
    return plain_scene(position, uv);
}

// The compositing variants' distorted form: `fs_composite`, in its order. The glitch moves where the frame is
// read first, by whole pixels; the bend, read at the screen's pixel, moves it on from there; then the composite,
// the colour effects at the glitched place, and the CRT on the screen. The held swing compares with the bent
// frame without the screen effects.
@fragment
fn fs_composite_distorted(in: CompositeVaryings) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(texture0));
    var at = in;
    var split = 0.0;
    let glitched = POST_COLOR_EFFECTS && post.effects.glitch.x > 0.0;
    if glitched {
        let offsets = glitch_offsets(in.uv);
        let moved = clamp(in.position.x + offsets.x, 0.5, 1.0 / post.frame.y - 0.5);
        at.position.x = moved;
        at.uv.x += (moved - in.position.x) * post.frame.y;
        split = offsets.y;
    }
    let bend = bend_at(in.uv, at.uv);
    let bent_position = at.position.xy + bend.pixels;
    var scene: vec3<f32>;
    if POST_LENS_EFFECTS {
        scene = lens_scene(bent_position, bend.uv) * post.composite.x;
    } else if glitched {
        scene = glitch_scene(bent_position, bend.uv, split);
    } else {
        scene = bent_plain_scene(in.position.xy, in.uv, bend);
    }
    var display = composited_bent(scene, at, bend.uv, size);
    if POST_COLOR_EFFECTS {
        display = effect_colors(display, at.uv);
        if post.effects.crt.x > 0.0 {
            display = crt(display, in.position.xy, in.uv);
        }
    }
    let light = srgb_to_linear(display);
    let swing = post.effects.chromatic.z;
    if (POST_LENS_EFFECTS || POST_COLOR_EFFECTS) && swing < 1.0 {
        let screen_bend = bend_at(in.uv, in.uv);
        let unaffected = bent_plain_scene(in.position.xy, in.uv, screen_bend);
        let plain = srgb_to_linear(composited_bent(unaffected, in, screen_bend.uv, size));
        return vec4<f32>(held_swing(light, plain, swing), 1.0);
    }
    return vec4<f32>(light, 1.0);
}
