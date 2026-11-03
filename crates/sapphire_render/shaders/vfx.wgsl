// The frame's effects (M2 E2; `sapphire_render::vfx`, whose docs have the design): instanced quads,
// sprites then the stretches of ribbons, drawn in ranges of four vertices an instance, pulling everything
// from storage buffers. The program is frame.wgsl, materials.wgsl, then this file.
//
// Blending is premultiplied, so one pipeline draws light and surfaces alike: its alpha is how much
// of what's behind it covers, coverage × (1 − additive). The fog, the light and the exposure are
// the vertex stage's: a particle is small, and a frame can stack many.

// `vfx::AFTER_RESOLVE_CONSTANT`: this pipeline draws over temporal AA's resolved frame, at the
// output's size, unjittered, testing depth by hand against the scene prepass's (and fading into it);
// otherwise in the main pass, against its depth, which nothing here can read.
override VFX_AFTER_RESOLVE: bool = false;

// `GpuVfxSprite::flags`: the shape's layer in the low byte, then whether it's lit.
const VFX_LAYER_MASK: u32 = 255u;
const VFX_LIT: u32 = 256u;
// A ribbon's stretch in the varyings' `layer`, which reads no map.
const VFX_RIBBON: u32 = 65535u;
// How much of the key light a lit quad takes: a puff of dust scatters it every way, so no N·L.
const VFX_LIT_KEY: f32 = 0.45;
// Past this share of the strip's width the glow has faded at its base rail.
const VFX_BASE_FADE: f32 = 0.2;

// Mirrors `vfx::GpuVfxSprite`.
struct VfxSprite {
    position: vec3<f32>,
    half_width: f32,
    // World direction its length lies along; zero faces the camera whole, turned by `rotation`.
    axis: vec3<f32>,
    rotation: f32,
    color: vec3<f32>,
    alpha: f32,
    half_length: f32,
    additive: f32,
    soft: f32,
    flags: u32,
}

// Mirrors `vfx::GpuVfxRibbonPoint`.
struct VfxRibbonPoint {
    base: vec3<f32>,
    // Not 0: its ribbon's last point, which joins nothing to the next.
    ends: u32,
    tip: vec3<f32>,
    core_position: f32,
    color: vec3<f32>,
    alpha: f32,
    core: vec3<f32>,
    core_half_width: f32,
    glow_reach: f32,
    additive: f32,
    soft: f32,
    padding: f32,
}

// Mirrors `vfx::GpuVfxUniforms`.
struct VfxUniforms {
    // xy: the output's size in pixels; zw: reciprocals.
    target_size: vec4<f32>,
    // Instances below this are sprites; the rest are ribbons' stretches.
    sprite_count: u32,
    // What an emissive color is multiplied by: the reciprocal of the frame's exposure.
    emissive: f32,
    padding: vec2<f32>,
}

// The vertex stage alone reads these: the fragment stage has no storage buffer to spare (frame.wgsl).
@group(2) @binding(0) var<storage, read> vfx_sprites: array<VfxSprite>;
@group(2) @binding(1) var<storage, read> vfx_ribbon: array<VfxRibbonPoint>;
@group(2) @binding(2) var<uniform> vfx: VfxUniforms;
@group(2) @binding(3) var vfx_sampler: sampler;

// The generated shapes, a layer each (`VfxShape`).
@group(3) @binding(0) var vfx_map: texture_2d_array<f32>;
// The scene prepass's depth over temporal AA's frame, else a 1×1 stand-in nothing reads.
@group(3) @binding(1) var vfx_scene_depth: texture_depth_2d;

struct VfxVaryings {
    @builtin(position) clip_position: vec4<f32>,
    // A sprite's map coordinates, its length along y; a ribbon's along the strip and across it, y
    // from 0 at the base rail to 1 at the tip.
    @location(0) uv: vec2<f32>,
    // rgb: the color, its light and the fog over it, without the fog's own light. a: opacity.
    @location(1) color: vec4<f32>,
    // rgb: a ribbon's core likewise. a: what's left of what's behind where it covers, 1 − additive.
    @location(2) core: vec4<f32>,
    // rgb: the fog's own light in front of it, where it covers what's behind. a: its soft distance.
    @location(3) air: vec4<f32>,
    // A ribbon's cross-section: x the core's middle, y half its width, z the glow's reach.
    @location(4) cross_section: vec3<f32>,
    @location(5) @interpolate(flat) layer: u32,
}

fn vfx_clip(world: vec3<f32>) -> vec4<f32> {
    if VFX_AFTER_RESOLVE {
        return view.unjittered_clip_from_world * vec4<f32>(world, 1.0);
    }
    return view.clip_from_world * vec4<f32>(world, 1.0);
}

// What the varyings need of `fogged` over the way from `world` to the camera, its two parts apart:
// what comes through it, and its own light. A quad's light adds to what's behind, which has its
// fog already, so only the share of it that covers what's behind brings the fog's own light.
fn vfx_fogged(out: ptr<function, VfxVaryings>, world: vec3<f32>, color: vec3<f32>, core: vec3<f32>, additive: f32) {
    let air = fogged(vec3<f32>(0.0), world, 1.0);
    let through = fogged(vec3<f32>(1.0), world, 1.0) - air;
    (*out).color = vec4<f32>(color * through, (*out).color.a);
    (*out).core = vec4<f32>(core * through, 1.0 - additive);
    (*out).air = vec4<f32>(air * (1.0 - additive), (*out).air.a);
}

fn vfx_sprite_vertex(index: u32, corner: vec2<f32>) -> VfxVaryings {
    let sprite = vfx_sprites[index];
    let local = (corner * 2.0 - 1.0) * vec2<f32>(sprite.half_width, sprite.half_length);
    // The camera's axes in the world: the view matrix's rows.
    let v = view.view_from_world;
    let right = vec3<f32>(v[0].x, v[1].x, v[2].x);
    let up = vec3<f32>(v[0].y, v[1].y, v[2].y);
    var world: vec3<f32>;
    if all(sprite.axis == vec3<f32>(0.0)) {
        let c = cos(sprite.rotation);
        let s = sin(sprite.rotation);
        world = sprite.position + right * (c * local.x - s * local.y) + up * (s * local.x + c * local.y);
    } else {
        // Its length along the axis, its width turned as far toward the camera as that leaves.
        let along = safe_normalize(sprite.axis);
        let side = safe_normalize(cross(along, view.position.xyz - sprite.position));
        world = sprite.position + side * local.x + along * local.y;
    }
    var color = sprite.color * vfx.emissive;
    if (sprite.flags & VFX_LIT) != 0u {
        color = sprite.color * (ambient_light(vec3<f32>(0.0, 0.5, 0.0)) + frame.sun_radiance.rgb * VFX_LIT_KEY);
    }
    var out: VfxVaryings;
    out.clip_position = vfx_clip(world);
    out.uv = corner;
    out.color.a = sprite.alpha;
    out.air.a = sprite.soft;
    vfx_fogged(&out, world, color, vec3<f32>(0.0), sprite.additive);
    out.cross_section = vec3<f32>(0.0);
    out.layer = sprite.flags & VFX_LAYER_MASK;
    return out;
}

fn vfx_ribbon_vertex(index: u32, corner: vec2<f32>) -> VfxVaryings {
    var out: VfxVaryings;
    // Its ribbon's last point joins nothing: a stretch of no area, off the screen.
    out.clip_position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    out.layer = VFX_RIBBON;
    if vfx_ribbon[index].ends == 0u {
        let point = vfx_ribbon[index + u32(corner.x)];
        let world = mix(point.base, point.tip, corner.y);
        out.clip_position = vfx_clip(world);
        out.uv = corner;
        out.color.a = point.alpha;
        out.air.a = point.soft;
        out.cross_section = vec3<f32>(point.core_position, point.core_half_width, point.glow_reach);
        vfx_fogged(&out, world, point.color * vfx.emissive, point.core * vfx.emissive, point.additive);
    }
    return out;
}

@vertex
fn vs_vfx(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VfxVaryings {
    // A triangle strip: (0, 0), (1, 0), (0, 1), (1, 1).
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    if instance < vfx.sprite_count {
        return vfx_sprite_vertex(instance, corner);
    }
    return vfx_ribbon_vertex(instance - vfx.sprite_count, corner);
}

// How the quad covers a pixel: light by the color and light by the core, and all it covers.
struct VfxShape {
    glow: f32,
    band: f32,
    coverage: f32,
}

// A ribbon's cross-section at `across`, 0 at the base rail and 1 at the tip: a crisp band, whose
// edges stay one pixel wide (`aa`, the across coordinate's change per pixel) however near the
// strip is, and a glow falling away from it to nothing at each rail.
fn vfx_ribbon_shape(across: f32, aa: f32, section: vec3<f32>) -> VfxShape {
    let from_core = abs(across - section.x);
    let band = 1.0 - smoothstep(section.y - aa, section.y + aa, from_core);
    // Its reach on the tip's side is what's left of the strip.
    let reach = select(section.z, max(1.0 - section.x, 1e-4), across > section.x);
    let falloff = saturate(1.0 - from_core / reach);
    let glow = falloff * falloff * smoothstep(0.0, VFX_BASE_FADE, across);
    return VfxShape(glow, band, max(glow, band));
}

// One exit, and no early return under the varyings' `layer`: WARP's compiler crashes on one.
fn vfx_shape(in: VfxVaryings, dx: vec2<f32>, dy: vec2<f32>) -> VfxShape {
    var shape: VfxShape;
    if in.layer == VFX_RIBBON {
        shape = vfx_ribbon_shape(in.uv.y, max(abs(dx.y) + abs(dy.y), 1e-4), in.cross_section);
    } else {
        let coverage = textureSampleGrad(vfx_map, vfx_sampler, in.uv, in.layer, dx, dy).r;
        shape = VfxShape(coverage, 0.0, coverage);
    }
    return shape;
}

// How much of it shows past the scene's surfaces, over temporal AA's frame: by the view depth to
// the surfaces around the pixel, each of the four fading it over its soft distance, so it cuts no
// hard line where it meets one, and a depth jittered under the resolved frame doesn't crawl. Reverse-Z
// with an infinite far plane: view depth is near / depth, and a pixel nothing covered (depth 0) is
// as far as the sky.
fn vfx_scene_fade(in: VfxVaryings) -> f32 {
    var fade = 1.0;
    if VFX_AFTER_RESOLVE {
        let depths = textureGather(vfx_scene_depth, character_sampler, in.clip_position.xy * vfx.target_size.zw);
        let near = view.clip_from_view[3][2];
        let own = near / in.clip_position.z;
        let scene = near / max(depths, vec4<f32>(1e-9));
        fade = dot(saturate((scene - vec4<f32>(own)) / max(in.air.a, 1e-4)), vec4<f32>(0.25));
    }
    return fade;
}

@fragment
fn fs_vfx(in: VfxVaryings) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow.
    let shape = vfx_shape(in, dpdx(in.uv), dpdy(in.uv));
    let opacity = in.color.a * vfx_scene_fade(in);
    let light = shape.glow * in.color.rgb + shape.band * in.core.rgb + shape.coverage * in.air.rgb;
    return vec4<f32>(light * opacity, shape.coverage * opacity * in.core.a);
}

// Writes scene color's alpha alone, in the main pass: the world's share of the pixel (post's
// character mask, M1 P2), which rises by the quad's coverage, so a spark over a character blooms
// as a spark does anywhere, not as skin does.
@fragment
fn fs_vfx_mask(in: VfxVaryings) -> @location(0) vec4<f32> {
    let shape = vfx_shape(in, dpdx(in.uv), dpdy(in.uv));
    return vec4<f32>(0.0, 0.0, 0.0, shape.coverage * in.color.a);
}
