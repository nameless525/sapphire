// Pass 3: the sky (M1 W2; spec §5.12 "Sky"), on a fullscreen triangle at the far plane (depth 0
// in reverse-Z). It is drawn after the opaque passes with depth test `Equal`, so only pixels no
// geometry covered run the fragment shader, which saves fill on mobile. The same shader draws the
// sky cube's faces, without the discs (`SKY_DISCS` off). The `sky` module's docs describe the
// layers and their data; the comments here say how each is drawn.
//
// Back to front, over the gradient (`sky_color()` in frame.wgsl, unchanged, which the anime
// character shader also reads for ambient light): stars and the Milky Way on the celestial
// sphere, the sun's and moon's glows, their discs, the cirrus plane, and the band of card clouds
// around the horizon. Stars and clouds are pipeline variants (below); within one, every layer is
// a branch every pixel takes alike, so a layer at zero is skipped and leaves the gradient exactly
// as it was. The sky shades most of the screen, so every instruction counts: per-draw constants
// are worked out on the CPU (`SkyUniforms`), and edges are linear ramps. Pixels also skip what
// can't reach them, leaving their color exactly as drawing it would: under the horizon nothing
// but the glows' fringe adds anything, so there the layers are skipped (the clouds per 2 × 2
// quad, whose derivatives need it), and a cloud layer paints only where its texel covers the
// pixel. Last, the frame's fog covers it all as at infinite distance (`fogged_sky`, M1 W3): a
// band along the horizon, all fog at and below it, so the sky meets fogged geometry without a
// seam.

struct SkyUniforms {
    // xyz: unit vector toward the sun; w: its disc's angular radius (0: a dark disc, skipped).
    sun: vec4<f32>,
    // rgb: the disc's radiance; w: the width of its edge, in radians.
    sun_disc: vec4<f32>,
    // rgb: the glow at its center; w: the glow's angular radius (0: none).
    sun_glow: vec4<f32>,
    moon: vec4<f32>,
    moon_disc: vec4<f32>,
    moon_glow: vec4<f32>,
    // The moon face's axes on the sky, as the viewer sees it: right, and up toward the zenith.
    moon_right: vec4<f32>,
    moon_up: vec4<f32>,
    // World directions into the celestial sphere's frame, where the stars stay put.
    celestial: mat3x3<f32>,
    // x: star brightness; y: twinkle, 0 to 1.
    stars: vec4<f32>,
    // rgb: the clouds' lit tone; w: card cloud coverage (0 skips the layer).
    cloud_lit: vec4<f32>,
    // rgb: their shade tone; w: cirrus coverage (0 skips the layer).
    cloud_shade: vec4<f32>,
    // rgb: their rim.
    cloud_rim: vec4<f32>,
    // x: card cloud repeats around the horizon; y: dissolve softness; z: drift, in repeats.
    clouds: vec4<f32>,
    // x: cirrus scale; y: dissolve softness; zw: drift, in repeats.
    cirrus: vec4<f32>,
    // x: the sky material's bound texture slots; y: the slots whose swizzle is the identity.
    modes: vec4<u32>,
    swizzles: vec4<u32>,
    // The glows' shapes, sun then moon: x, z the inverse of the radius; y, w how much a vertical
    // offset counts, squared, less one (spreading a low body's glow along the horizon).
    glow_shape: vec4<f32>,
}

// Binding 2: frame.wgsl declares the scene group's bindings 0 and 1, which the sky doesn't read.
@group(2) @binding(2) var<uniform> sky: SkyUniforms;

// The sky family's group 3 (`SkyMaterial::TEXTURE_SLOTS`): slot `i` at binding `i`, each unbound
// one a white 1×1 fallback, and the material's trilinear repeating sampler.
@group(3) @binding(0) var moon_map: texture_2d<f32>;
@group(3) @binding(1) var stars_map: texture_2d<f32>;
@group(3) @binding(2) var milky_way_map: texture_2d<f32>;
@group(3) @binding(3) var clouds_map: texture_2d<f32>;
@group(3) @binding(4) var cirrus_map: texture_2d<f32>;
@group(3) @binding(8) var sky_sampler: sampler;

// Texture slots (`SkyMaterial::TEXTURE_SLOTS`).
const SKY_SLOT_MOON: u32 = 0u;
const SKY_SLOT_STARS: u32 = 1u;
const SKY_SLOT_MILKY_WAY: u32 = 2u;
const SKY_SLOT_CLOUDS: u32 = 3u;
const SKY_SLOT_CIRRUS: u32 = 4u;

// Pipeline variants, so a frame compiles only the layers it draws (and the plainest sky keeps
// the registers of the plainest shader). The discs are off for the sky cube: they are the key
// light, which shading adds itself. Stars and clouds are on while time of day gives them a
// brightness or a coverage; within the clouds' variant, each layer still skips itself at zero.
override SKY_DISCS: bool = true;
override SKY_STARS: bool = true;
override SKY_CLOUDS: bool = true;

const SKY_PI: f32 = 3.14159265;
const SKY_TAU: f32 = 6.28318531;
const SKY_LUMINANCE: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
// The glows fade out this far below the horizon, as a direction's height.
const SKY_GLOW_FRINGE: f32 = 0.08;

// Star grids: cells per cube-face edge, the chance a cell holds a star, the brightest star's
// light (summed over its pixels), and its Gaussian's radius in pixels. A fine grid of faint stars
// and a coarse one of bright ones.
const STAR_FINE_CELLS: f32 = 160.0;
const STAR_FINE_CHANCE: f32 = 0.3;
const STAR_FINE_FLUX: f32 = 1.2;
const STAR_FINE_SIZE: f32 = 0.6;
const STAR_COARSE_CELLS: f32 = 44.0;
const STAR_COARSE_CHANCE: f32 = 0.35;
const STAR_COARSE_FLUX: f32 = 7.0;
const STAR_COARSE_SIZE: f32 = 0.85;
// Stars sit this far inside their cell's edges, so few reach past it.
const STAR_MARGIN: f32 = 0.15;
// Grids fade out while their cells shrink from this many pixels to the next, before they crowd.
const STAR_FADE_PIXELS: vec2<f32> = vec2<f32>(3.0, 6.0);
const STAR_COOL: vec3<f32> = vec3<f32>(0.72, 0.84, 1.0);
const STAR_WARM: vec3<f32> = vec3<f32>(1.0, 0.86, 0.68);

// The card cloud band's bottom edge, as a direction's height (about radians of elevation): a
// little under the horizon, so the clouds' bases sit behind it.
const CLOUD_BAND_BASE: f32 = -0.035;
// The painted terminator's softness, in R's units (at least a pixel's change).
const CLOUD_SPLIT_SOFTNESS: f32 = 0.05;
// The rim band inside the dissolving edge, in B's units (at least two pixels' change).
const CLOUD_RIM_WIDTH: f32 = 0.012;
// The rim's strength away from the key light, as a fraction of its strength toward it.
const CLOUD_RIM_AMBIENT: f32 = 0.12;
// Low clouds melt this far into the horizon color.
const CLOUD_HAZE: f32 = 0.35;
// The cirrus plane's directions are lifted this much, so it recedes smoothly to the horizon.
const CIRRUS_LIFT: f32 = 0.15;
const CIRRUS_OPACITY: f32 = 0.6;

struct SkyVaryings {
    // Invariant because the depth test is `Equal`: the position must not vary with how the
    // shader happens to be compiled.
    @builtin(position) @invariant clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> SkyVaryings {
    // Vertices (-1, -1), (3, -1), (-1, 3) cover the screen with one triangle.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = corner * 2.0 - 1.0;
    var out: SkyVaryings;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

// The cloud layers branch per 2 × 2 quad (`quad_any`), which WGSL's uniformity analysis can't
// prove uniform: the filter lets their texels' derivatives through, as they stay well defined.
@diagnostic(off, derivative_uniformity)
@fragment
fn fs_sky(in: SkyVaryings) -> @location(0) vec4<f32> {
    // Unproject the pixel on the near plane (depth 1); the far plane is at infinity.
    let near = view.world_from_clip * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(near.xyz / near.w - view.position.xyz);
    var color = sky_color(dir);
    // The direction's change across the pixel, taken here in uniform control flow: every layer
    // sizes its edges and texture reads by it. `footprint` is the pixel's angular size.
    let ddx = dpdx(dir);
    let ddy = dpdy(dir);
    let footprint = max(max(length(ddx), length(ddy)), 1e-6);
    // How much of the pixel is above the horizon, which hides what sinks below it.
    let above = saturate(0.5 + dir.y / (2.0 * footprint));
    // Whether a cloud layer can show in the pixel's quad: some pixel of it lies less than its
    // footprint under the horizon (with a margin for rounding). Lower, `above` is 0 for the card
    // clouds and the cirrus has faded out, so skipping them leaves the color exactly as it was.
    // One test serves both layers, taken here in uniform control flow, since it reads the quad's
    // other pixels through derivatives.
    let clouds = SKY_CLOUDS && quad_any(dir.y > -1.001 * footprint);

    // The bodies, the stars and the glows add nothing under the glows' fringe below the horizon.
    if dir.y > -SKY_GLOW_FRINGE {
        var moon = vec4<f32>(0.0);
        if SKY_DISCS {
            moon = moon_face(dir, ddx, ddy, footprint, above);
        }
        if SKY_STARS {
            // The moon hides the stars behind it.
            color += night_sky(dir, footprint) * (1.0 - moon.a);
        }
        if sky.sun_glow.w > 0.0 || sky.moon_glow.w > 0.0 {
            // The glows light only the sky above the horizon.
            let glows = glow(dir, sky.sun, sky.sun_glow, sky.glow_shape.xy)
                + glow(dir, sky.moon, sky.moon_glow, sky.glow_shape.zw);
            color += glows * smoothstep(-SKY_GLOW_FRINGE, 0.0, dir.y);
        }
        if SKY_DISCS {
            color += sky.sun_disc.rgb * disc_cover(dir, sky.sun, sky.sun_disc.w, footprint, above);
            color += sky.moon_disc.rgb * moon.rgb * moon.a;
        }
    }
    if clouds && sky.cloud_shade.w > 0.0 {
        color = cirrus_layer(color, dir, ddx, ddy);
    }
    if clouds && sky.cloud_lit.w > 0.0 {
        color = cloud_band(color, dir, ddx, ddy, above);
    }
    // The frame's fog over all of it (M1 W3), as at infinite distance, so the sky agrees with
    // the fog over distant geometry wherever the two meet.
    return vec4<f32>(fogged_sky(color, dir), 1.0);
}

// atan2(y, x) to within 2e-6 radians: a minimax polynomial for the arctangent on [0, 1], with the
// octant folded in. The sky's azimuths and elevations need a texel's precision, about 1e-3, and
// the built-in costs several times as much where GPUs have no native arctangent.
fn sky_atan2(y: f32, x: f32) -> f32 {
    let ax = abs(x);
    let ay = abs(y);
    let t = min(ax, ay) / max(max(ax, ay), 1e-30);
    let s = t * t;
    var angle = t * (0.99997726 + s * (-0.33262347 + s * (0.19354346 + s * (-0.11643287 + s * (0.05265332 - s * 0.0117212)))));
    angle = select(angle, 1.57079633 - angle, ay > ax);
    angle = select(angle, SKY_PI - angle, x < 0.0);
    return select(angle, -angle, y < 0.0);
}

// Whether `passes` holds for any pixel of this pixel's 2 × 2 quad: the same answer in all four,
// from the pass/fail value's fine derivatives, which give its neighbors' in the quad (the eyes
// through hair do the same, anime_character.wgsl). Taken in uniform control flow; a branch on it
// is uniform per quad, so derivatives inside it stay well defined.
fn quad_any(passes: bool) -> bool {
    let value = select(0.0, 1.0, passes);
    let across = dpdxFine(value);
    let down = dpdyFine(value);
    return value + abs(across) + abs(down) + abs(dpdyFine(across)) > 0.0;
}

// A texel sampled from texture slot `slot` in its logical channels: as sampled when the slot's
// swizzle is the identity (the CPU marks those in `modes.y`; every sky map of both cooks is),
// else through the swizzle, whose per-component selection is far dearer than the read.
fn sky_texel(sampled: vec4<f32>, slot: u32) -> vec4<f32> {
    if (sky.modes.y & (1u << slot)) != 0u {
        return sampled;
    }
    return material_swizzle(sampled, sky.swizzles, slot);
}

// --- Sun and moon

// The glow around a body at `body.xyz` (toward it): `light.rgb` at its center, fading smoothly to
// nothing at angular radius `light.w`, a wide halo with a brighter core. `shape` is the inverse
// radius and the vertical stretch (`glow_shape`): while the body is low the glow spreads along
// the horizon, as anime skies spread a setting sun's warmth along it. Compact, so a body far out
// of view adds exactly nothing.
fn glow(dir: vec3<f32>, body: vec4<f32>, light: vec4<f32>, shape: vec2<f32>) -> vec3<f32> {
    if light.w <= 0.0 {
        return vec3<f32>(0.0);
    }
    let offset = dir - body.xyz;
    let distance = sqrt(dot(offset, offset) + shape.y * offset.y * offset.y);
    let fall = 1.0 - saturate(distance * shape.x);
    let halo = fall * fall * fall;
    let core = halo * halo * halo * halo;
    return light.rgb * (halo + core);
}

// How much of a disc around `body.xyz` of angular radius `body.w` covers the pixel: its edge
// `edge` wide, or a pixel if narrower (a linear ramp, the pixel's own coverage of a straight
// edge), and cut by the horizon, `above` (a setting body sinks behind it).
fn disc_cover(dir: vec3<f32>, body: vec4<f32>, edge: f32, footprint: f32, above: f32) -> f32 {
    if body.w <= 0.0 {
        return 0.0;
    }
    // The chord to the body: its angle, to well under a pixel for discs a few degrees wide.
    let inside = saturate((body.w - length(dir - body.xyz)) / max(edge, footprint) + 0.5);
    return inside * above;
}

// The moon's face where the disc covers the pixel: rgb its color (white without a face map),
// a its coverage, the disc's times the map's alpha, which can paint a crescent.
fn moon_face(dir: vec3<f32>, ddx: vec3<f32>, ddy: vec3<f32>, footprint: f32, above: f32) -> vec4<f32> {
    let cover = disc_cover(dir, sky.moon, sky.moon_disc.w, footprint, above);
    // One exit: this runs under a per-pixel branch (WARP's compiler, see the outline shader).
    var moon = vec4<f32>(0.0);
    if cover > 0.0 {
        var face = vec4<f32>(1.0);
        if material_has_texture(sky.modes, SKY_SLOT_MOON) {
            // The map spans the disc, upright: u across, v down.
            let scale = 0.5 / sky.moon.w;
            let axes = mat2x3<f32>(sky.moon_right.xyz * scale, -sky.moon_up.xyz * scale);
            let uv = vec2<f32>(0.5) + (dir - sky.moon.xyz) * axes;
            let sampled = textureSampleGrad(moon_map, sky_sampler, uv, ddx * axes, ddy * axes);
            face = sky_texel(sampled, SKY_SLOT_MOON);
        }
        moon = vec4<f32>(face.rgb, face.a * cover);
    }
    return moon;
}

// --- Stars and the Milky Way

// The night sky's light toward `dir`, at full star brightness: the stars of both grids, tinted
// by the star map where there is one, and the Milky Way's glow. Both dim into the haze near the
// horizon and stop at it.
fn night_sky(dir: vec3<f32>, footprint: f32) -> vec3<f32> {
    var light = vec3<f32>(0.0);
    if dir.y > 0.0 {
        // A rotation: the direction stays a unit vector.
        let celestial = sky.celestial * dir;
        // Both grids share the cube face the direction points through.
        let face = cube_face(celestial);
        light = star_grid(celestial, face, footprint, STAR_FINE_CELLS, STAR_FINE_CHANCE, STAR_FINE_FLUX, STAR_FINE_SIZE, 0u)
            + star_grid(celestial, face, footprint, STAR_COARSE_CELLS, STAR_COARSE_CHANCE, STAR_COARSE_FLUX, STAR_COARSE_SIZE, 1u);
        if material_has_texture(sky.modes, SKY_SLOT_MILKY_WAY) {
            light += dome_texel(milky_way_map, SKY_SLOT_MILKY_WAY, celestial, footprint).rgb;
        }
        light *= sky.stars.x * smoothstep(0.0, 0.25, dir.y);
    }
    return light;
}

// The cube face a direction `c` points through (+X, −X, +Y, −Y, +Z, −Z): its other two
// components, and the one along the face's axis, which divides them into the point on the face.
// The grids each divide (`star_grid`): sharing the quotient would round some stars differently.
struct CubeFace {
    index: u32,
    across: vec2<f32>,
    along: f32,
}

fn cube_face(c: vec3<f32>) -> CubeFace {
    let a = abs(c);
    var face: CubeFace;
    if a.x >= a.y && a.x >= a.z {
        face = CubeFace(select(1u, 0u, c.x > 0.0), c.yz, a.x);
    } else if a.y >= a.z {
        face = CubeFace(select(3u, 2u, c.y > 0.0), c.xz, a.y);
    } else {
        face = CubeFace(select(5u, 4u, c.z > 0.0), c.xy, a.z);
    }
    return face;
}

// One grid of star cells on a cube around the celestial sphere, `cells` per face edge, each cell
// holding a star with probability `chance` at a hashed spot, brightness, color and twinkle.
// `face` is the cube face `c` points through.
//
// A star is a normalized Gaussian `size` pixels in radius, measured in pixels from the pixel's
// own angular footprint: its light summed over its pixels stays the same wherever it falls
// between them, so sub-pixel camera motion never makes it shimmer, and it is a crisp dot of the
// same size at any resolution. A star is seen only from pixels in its own cell (it sits well
// inside it); that clips a little of its rim the same way every frame, which stays stable too.
//
// One exit: this runs under per-pixel branches (WARP's compiler, see the outline shader).
fn star_grid(c: vec3<f32>, face: CubeFace, footprint: f32, cells: f32, chance: f32, flux: f32, size: f32, seed: u32) -> vec3<f32> {
    var light = vec3<f32>(0.0);
    let fade = smoothstep(STAR_FADE_PIXELS.x, STAR_FADE_PIXELS.y, 2.0 / (cells * footprint));
    // Where on the face, and in which cell.
    let uv = face.across / face.along;
    let cell = min(floor((uv * 0.5 + 0.5) * cells), vec2<f32>(cells - 1.0));
    // One hash of the cell (unique per grid, face and cell: at most 256 cells an edge) places
    // its star, 16 bits a coordinate. Most of a cell's pixels are beyond the star's reach (four
    // radii, where the Gaussian has fallen to e⁻⁸): they stop there, before the rest is hashed.
    let key = star_hash(((seed * 6u + face.index) * 256u + u32(cell.y)) * 256u + u32(cell.x));
    let place = vec2<f32>(f32(key >> 16u), f32(key & 0xffffu)) / 65536.0;
    let spot = (cell + STAR_MARGIN + (1.0 - 2.0 * STAR_MARGIN) * place) / cells * 2.0 - 1.0;
    let star = normalize(cube_face_point(face.index, spot));
    let r = length(c - star) / footprint;
    // A faded grid (cells a few pixels wide) draws nothing.
    if fade > 0.0 && r <= 4.0 * size {
        // Whether the cell has a star (16 bits), its brightness and its warmth (8 each);
        // then its twinkle's rate and phase.
        let more = star_hash(key);
        let twinkling = star_hash(more);
        let random = vec4<f32>(f32(more >> 16u) / 65536.0, f32((more >> 8u) & 255u) / 255.0,
            f32(more & 255u) / 255.0, 0.0);
        let rate = f32(twinkling >> 24u) / 255.0;
        let phase = f32((twinkling >> 16u) & 255u) / 255.0;
        var tint = vec3<f32>(1.0);
        var density = 1.0;
        if material_has_texture(sky.modes, SKY_SLOT_STARS) {
            // The star map tints the stars, and its luminance sets where they are dense
            // (0.5 is the grid's own density), read where the star is so every pixel of it
            // agrees.
            let field = dome_texel(stars_map, SKY_SLOT_STARS, star, 2.0 / cells).rgb;
            let luminance = dot(field, SKY_LUMINANCE);
            density = 2.0 * luminance;
            tint = field / max(luminance, 1e-4);
        }
        if random.x < chance * density {
            let kernel = exp(-0.5 * r * r / (size * size)) / (SKY_TAU * size * size);
            // Many faint stars and a few bright ones; cool and warm whites; each twinkling
            // at its own rate.
            let bright = random.y * random.y;
            let brightness = flux * (0.04 + 0.96 * bright * bright);
            let color = mix(STAR_COOL, STAR_WARM, random.z * random.z);
            let twinkle = 1.0 - sky.stars.y * (0.5 + 0.5 * sin(frame.time.x * (1.5 + 2.5 * rate) + SKY_TAU * phase));
            light = color * tint * (brightness * twinkle * kernel * fade);
        }
    }
    return light;
}

// The point on cube face `face` (+X, −X, +Y, −Y, +Z, −Z) at `uv`, the inverse of `cube_face`.
fn cube_face_point(face: u32, uv: vec2<f32>) -> vec3<f32> {
    let side = select(-1.0, 1.0, (face & 1u) == 0u);
    var point = vec3<f32>(uv.x, uv.y, side);
    if face < 4u {
        point = vec3<f32>(uv.x, side, uv.y);
    }
    if face < 2u {
        point = vec3<f32>(side, uv.x, uv.y);
    }
    return point;
}

// PCG (the RXS-M-XS output of O'Neill's generator), which Jarzynski and Olano ("Hash Functions
// for GPU Rendering", JCGT 2020) recommend for one 32-bit value: well mixed, cheap, and the same
// on every GPU.
fn star_hash(input: u32) -> u32 {
    let state = input * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// A texel of a map wrapped on the celestial sphere (equirectangular: u around the pole, v from
// it down to the opposite pole), at the level whose texels are about `angle` radians: sampled by
// level, so the seam where u wraps needs no derivatives.
fn dome_texel(map: texture_2d<f32>, slot: u32, c: vec3<f32>, angle: f32) -> vec4<f32> {
    let uv = vec2<f32>(sky_atan2(c.z, c.x) / SKY_TAU + 0.5, sky_atan2(length(c.xz), c.y) / SKY_PI);
    let texels = f32(textureDimensions(map).y) / SKY_PI;
    let level = max(log2(angle * texels), 0.0);
    return sky_texel(textureSampleLevel(map, sky_sampler, uv, level), slot);
}

// --- Clouds

// How much of a painted cloud texel in the Genshin scheme covers the pixel, dissolved to
// `coverage`: R the lit amount (its 0.5 line the painted terminator), G a painted rim, B a signed
// distance to the cloud's edge (0.5 on the painted silhouette, rising inward), A opacity.
// `widths` are R's and B's change across the pixel, so the terminator and the silhouette stay a
// pixel sharp at any magnification, or `softness` (B's units) if softer.
//
// Coverage 1 keeps the painted silhouette; lowering it raises the threshold along B, so a cloud
// shrinks toward its core and vanishes past it. Returns the cover (x, the dissolved body times
// A) and the threshold (y), which `paint_cloud` shades by: the layers paint only covered pixels.
fn cloud_cover(texel: vec4<f32>, widths: vec2<f32>, coverage: f32, softness: f32) -> vec2<f32> {
    let edge = max(softness, widths.y);
    let threshold = mix(1.0 + edge, 0.5, coverage);
    // Linear ramps, a pixel or the authored softness wide: edges need no smoother, and the sky
    // pays for every instruction on every pixel it covers.
    let body = saturate((texel.b - threshold) / (2.0 * edge) + 0.5);
    return vec2<f32>(body * texel.a, threshold);
}

struct PaintedCloud {
    // The lit and shade tones' mix.
    color: vec3<f32>,
    // How much rim, 0 to 1.
    rim: f32,
}

// A covered cloud texel shaded (see `cloud_cover`; `threshold` its dissolve threshold). Its
// colors are authored: the lit and shade tones, never a darkened albedo, and a rim (the painted
// one and a band just inside the dissolving edge), which the layer colors.
fn paint_cloud(texel: vec4<f32>, widths: vec2<f32>, threshold: f32) -> PaintedCloud {
    let split = max(CLOUD_SPLIT_SOFTNESS, widths.x);
    let lit = saturate((texel.r - 0.5) / (2.0 * split) + 0.5);
    let rim_band = saturate(1.0 - (texel.b - threshold) / max(CLOUD_RIM_WIDTH, 2.0 * widths.y));
    return PaintedCloud(mix(sky.cloud_shade.rgb, sky.cloud_lit.rgb, lit), max(texel.g, rim_band));
}

// The card clouds: a band of painted cumulus around the horizon, the map repeated `clouds.x`
// times a turn and drifting around it, its bottom just under the horizon, rimmed most brightly
// toward the key light. Up the band, v follows
// the direction's height (the sine of its elevation), not the elevation itself: its texels are
// square at the horizon and a few percent taller toward the band's top, for one arctangent less.
// Called in control flow uniform per quad (the layer's branch), so the texel's own screen
// derivatives size its edges; texture coordinates and their derivatives come from the
// direction's, which stay continuous where the azimuth wraps.
fn cloud_band(color: vec3<f32>, dir: vec3<f32>, ddx: vec3<f32>, ddy: vec3<f32>, above: f32) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(clouds_map));
    let repeats = sky.clouds.x;
    let height = SKY_TAU / repeats * size.y / size.x;
    let flat_squared = max(dir.x * dir.x + dir.z * dir.z, 1e-8);
    let azimuth = sky_atan2(dir.x, -dir.z);
    let uv = vec2<f32>(azimuth / SKY_TAU * repeats + sky.clouds.z, 1.0 - (dir.y - CLOUD_BAND_BASE) / height);
    // d(azimuth) = (x dz − z dx) / (x² + z²) for azimuth = atan2(x, −z).
    let u_scale = repeats / (SKY_TAU * flat_squared);
    let v_scale = -1.0 / height;
    let gx = vec2<f32>((dir.x * ddx.z - dir.z * ddx.x) * u_scale, ddx.y * v_scale);
    let gy = vec2<f32>((dir.x * ddy.z - dir.z * ddy.x) * u_scale, ddy.y * v_scale);
    let texel = sky_texel(textureSampleGrad(clouds_map, sky_sampler, uv, gx, gy), SKY_SLOT_CLOUDS);
    let widths = fwidth(texel.rb);
    let cover = cloud_cover(texel, widths, sky.cloud_lit.w, sky.clouds.y);
    let in_band = select(0.0, 1.0, uv.y >= 0.0 && uv.y <= 1.0);
    let alpha = cover.x * in_band * above;
    // Clear sky between and above the clouds keeps its color as it is: only covered pixels paint.
    var result = color;
    if alpha > 0.0 {
        let cloud = paint_cloud(texel, widths, cover.y);
        let toward_key = saturate(dot(dir, frame.sun_direction.xyz) * 0.5 + 0.5);
        let facing = CLOUD_RIM_AMBIENT + (1.0 - CLOUD_RIM_AMBIENT) * toward_key * toward_key * toward_key;
        var painted = cloud.color + sky.cloud_rim.rgb * (cloud.rim * facing);
        // Low clouds melt into the horizon, the anime way of showing distance.
        painted = mix(painted, frame.sky_horizon.rgb, CLOUD_HAZE * saturate(1.0 - dir.y / 0.15));
        result = mix(color, painted, alpha);
    }
    return result;
}

// The cirrus: a high layer on a plane over the camera, scrolled by the wind, its directions
// lifted a little so the plane recedes smoothly to the horizon, where it fades out before its
// texels shrink below a pixel. Its wisps are too thin for a rim, which would only outline them.
// Called in control flow uniform per quad, like the card clouds.
fn cirrus_layer(color: vec3<f32>, dir: vec3<f32>, ddx: vec3<f32>, ddy: vec3<f32>) -> vec3<f32> {
    let lift = max(dir.y + CIRRUS_LIFT, 0.5 * CIRRUS_LIFT);
    let scale = 0.5 * sky.cirrus.x;
    let uv = dir.xz / lift * scale + sky.cirrus.zw;
    // d(xz / lift) = (d(xz) lift − xz dy) / lift².
    let gx = (ddx.xz * lift - dir.xz * ddx.y) * (scale / (lift * lift));
    let gy = (ddy.xz * lift - dir.xz * ddy.y) * (scale / (lift * lift));
    let texel = sky_texel(textureSampleGrad(cirrus_map, sky_sampler, uv, gx, gy), SKY_SLOT_CIRRUS);
    let widths = fwidth(texel.rb);
    let cover = cloud_cover(texel, widths, sky.cloud_shade.w, sky.cirrus.y);
    let alpha = cover.x * CIRRUS_OPACITY * smoothstep(0.03, 0.3, dir.y);
    var result = color;
    if alpha > 0.0 {
        result = mix(color, paint_cloud(texel, widths, cover.y).color, alpha);
    }
    return result;
}
