// The frame's ground decals (M5 L7; `sapphire_render::vfx::decal`, whose docs have the design): a quad lying in
// the ground's plane per decal, drawn instanced in the main pass after the opaque world, unlit, its shape a
// signed distance evaluated per pixel. The program is frame.wgsl, materials.wgsl, then this file.
//
// `sapphire_look::vfx::decal` holds the reference: `VfxDecalShape::distance` and `progress`, `edge_noise`
// and `VfxDecal::opacity` are this file's functions, and their tests are its.

// `vfx::decal::DARKEN_CONSTANT`: the ground darkened toward the color (blend zero, source), rather than light
// added over it (one, one).
override VFX_DECAL_DARKEN: bool = false;

// `vfx::decal::PULL`: the share of its own distance a decal is drawn nearer the eye. The view's projection is
// reverse Z with an infinite far plane, clip z the near plane's distance and w the point's, so scaling clip z by
// 1 + this moves each point's depth to a distance 1 / (1 + this) of its own, whatever the quad's size.
override VFX_DECAL_PULL: f32 = 0.0;

// `GpuVfxDecal::kind`.
const DECAL_DISC: u32 = 0u;
const DECAL_RING: u32 = 1u;
const DECAL_SECTOR: u32 = 2u;
const DECAL_LANE: u32 = 3u;

// `edge_noise`'s cells and its fine octave's share.
const DECAL_NOISE_CELL: f32 = 0.45;
const DECAL_NOISE_FINE: f32 = 0.15;
const DECAL_NOISE_FINE_WEIGHT: f32 = 0.35;

// Mirrors `vfx::decal::GpuVfxDecal`.
struct VfxDecal {
    // Rotation and translation: local +Y the ground's normal, +Z the way it faces.
    world_from_local: mat4x4<f32>,
    // xy: the quad's least local (x, z); zw: its greatest, the edge's noise and a pixel's margin past the shape.
    bounds: vec4<f32>,
    color: vec3<f32>,
    alpha: f32,
    // Disc: radius. Ring: inner, outer. Sector: radius, sin and cos of its half angle, inner. Lane: width, length.
    shape: vec4<f32>,
    kind: u32,
    fill: f32,
    outline: f32,
    body: f32,
    noise: f32,
    padding_a: f32,
    padding_b: f32,
    padding_c: f32,
}

// Mirrors `vfx::decal::GpuVfxDecalUniforms`.
struct VfxDecalUniforms {
    // What a light's color is multiplied by: the reciprocal of the frame's exposure.
    emissive: f32,
    padding_a: f32,
    padding_b: f32,
    padding_c: f32,
}

// The vertex stage alone reads the decals: the fragment stage has no storage buffer to spare (frame.wgsl).
@group(2) @binding(0) var<storage, read> vfx_decals: array<VfxDecal>;
@group(2) @binding(1) var<uniform> vfx_decal: VfxDecalUniforms;

struct VfxDecalVaryings {
    @builtin(position) clip_position: vec4<f32>,
    // The pixel's local (x, z).
    @location(0) local: vec2<f32>,
    // rgb: the light added (× 1/exposure) or the color darkened toward; a: its opacity.
    @location(1) tint: vec4<f32>,
    @location(2) @interpolate(flat) shape: vec4<f32>,
    // x: fill; y: outline; z: body; w: noise.
    @location(3) @interpolate(flat) params: vec4<f32>,
    @location(4) @interpolate(flat) kind: u32,
    // Where the pixel lies, for its fog: a decal is broad, and the fog changes across it.
    @location(5) world: vec3<f32>,
}

@vertex
fn vs_vfx_decal(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VfxDecalVaryings {
    let decal = vfx_decals[instance];
    // A strip of two triangles over the bounds.
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let local = mix(decal.bounds.xy, decal.bounds.zw, corner);
    let world = (decal.world_from_local * vec4<f32>(local.x, 0.0, local.y, 1.0)).xyz;
    var out: VfxDecalVaryings;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.clip_position.z *= 1.0 + VFX_DECAL_PULL;
    out.local = local;
    out.world = world;
    var color = decal.color * vfx_decal.emissive;
    if VFX_DECAL_DARKEN {
        color = decal.color;
    }
    out.tint = vec4<f32>(color, decal.alpha);
    out.shape = decal.shape;
    out.params = vec4<f32>(decal.fill, decal.outline, decal.body, decal.noise);
    out.kind = decal.kind;
    return out;
}

// `VfxDecalShape::distance`: negative inside, metres.
fn decal_distance(kind: u32, shape: vec4<f32>, p: vec2<f32>) -> f32 {
    let r = length(p);
    var d = r - shape.x;
    if kind == DECAL_RING {
        d = max(r - shape.y, shape.x - r);
    } else if kind == DECAL_SECTOR {
        let folded = vec2<f32>(abs(p.x), p.y);
        let edge = shape.yz;
        let to_side = length(folded - edge * clamp(dot(folded, edge), 0.0, shape.x));
        let side = select(-to_side, to_side, edge.y * folded.x - edge.x * folded.y > 0.0);
        d = max(max(r - shape.x, side), shape.w - r);
    } else if kind == DECAL_LANE {
        let q = abs(p - vec2<f32>(0.0, shape.y * 0.5)) - vec2<f32>(shape.x * 0.5, shape.y * 0.5);
        d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
    }
    return d;
}

// `VfxDecalShape::progress`: 0 at the reach's start, 1 at its far edge.
fn decal_progress(kind: u32, shape: vec4<f32>, p: vec2<f32>) -> f32 {
    let r = length(p);
    var start = 0.0;
    var end = shape.x;
    if kind == DECAL_RING {
        start = shape.x;
        end = shape.y;
    } else if kind == DECAL_SECTOR {
        start = shape.w;
    }
    var progress = (r - start) / max(end - start, 1e-6);
    if kind == DECAL_LANE {
        progress = p.y / max(shape.y, 1e-6);
    }
    return progress;
}

// lowbias32, as `vfx::decal`'s `hash`.
fn decal_hash(value: u32) -> u32 {
    var x = value;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    return x ^ (x >> 16u);
}

fn decal_lattice(x: i32, y: i32, seed: u32) -> f32 {
    let h = decal_hash((bitcast<u32>(x) * 0x9e3779b1u) ^ decal_hash(bitcast<u32>(y) ^ seed));
    return f32(h >> 8u) / 8388608.0 - 1.0;
}

fn decal_value_noise(point: vec2<f32>, cell: f32, seed: u32) -> f32 {
    let at = point / cell;
    let base = floor(at);
    let f = at - base;
    let s = f * f * (3.0 - 2.0 * f);
    let ix = i32(base.x);
    let iy = i32(base.y);
    let a = decal_lattice(ix, iy, seed);
    let b = decal_lattice(ix + 1, iy, seed);
    let c = decal_lattice(ix, iy + 1, seed);
    let d = decal_lattice(ix + 1, iy + 1, seed);
    let top = a + (b - a) * s.x;
    let bottom = c + (d - c) * s.x;
    return top + (bottom - top) * s.y;
}

// `edge_noise`: -1 to 1.
fn decal_edge_noise(point: vec2<f32>) -> f32 {
    let coarse = decal_value_noise(point, DECAL_NOISE_CELL, 0x5c07u);
    let fine = decal_value_noise(point, DECAL_NOISE_FINE, 0x1f3du);
    return clamp((1.0 - DECAL_NOISE_FINE_WEIGHT) * coarse + DECAL_NOISE_FINE_WEIGHT * fine, -1.0, 1.0);
}

// `VfxDecal::cover`: the share of a pixel `footprint` across that `distance` from an edge covers.
fn decal_cover(distance: f32, footprint: f32) -> f32 {
    return saturate(0.5 - distance / max(footprint, 1e-6));
}

// `VfxDecal::opacity`, its footprints the pixel's own. Every derivative is taken before anything is decided.
fn decal_opacity(in: VfxDecalVaryings) -> f32 {
    let p = in.local;
    let d = decal_distance(in.kind, in.shape, p) - in.params.w * decal_edge_noise(p);
    let progress = decal_progress(in.kind, in.shape, p);
    let d_footprint = length(vec2<f32>(dpdx(d), dpdy(d)));
    let progress_footprint = length(vec2<f32>(dpdx(progress), dpdy(progress)));
    let inside = decal_cover(d, d_footprint);
    let inner = decal_cover(d + in.params.y, d_footprint);
    let band = max(inside - inner, 0.0);
    let fill = in.params.x;
    var filled = decal_cover(progress - fill, progress_footprint);
    filled = select(filled, 0.0, fill <= 0.0);
    filled = select(filled, 1.0, fill >= 1.0);
    let body = in.params.z;
    return in.tint.a * (band + inner * (body + (1.0 - body) * filled));
}

// The fog in front of the pixel, as a sprite's: light added takes one ray's transmittance. A darkening is
// one multiplying draw, so it scales the whole fogged pixel, the mist in front with the ground behind it: an
// approximation the module's docs state and a GPU test holds. Per pixel, not per corner: a decal may be
// metres across.
fn decal_transmittance(world: vec3<f32>) -> f32 {
    var transmittance = 1.0;
    if fog_on() {
        let offset = world - view.position.xyz;
        let distance = length(offset);
        transmittance = fog_ray(view.position.xyz, offset / max(distance, 1e-6), distance, 1.0).transmittance;
    }
    return transmittance;
}

@fragment
fn fs_vfx_decal(in: VfxDecalVaryings) -> @location(0) vec4<f32> {
    let opacity = decal_opacity(in) * decal_transmittance(in.world);
    var color = in.tint.rgb * opacity;
    if VFX_DECAL_DARKEN {
        color = mix(vec3<f32>(1.0), in.tint.rgb, opacity);
    }
    // The alpha is never written: scene color's keeps marking what's a character for post.
    return vec4<f32>(color, 0.0);
}
