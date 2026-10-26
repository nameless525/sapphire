// Ambient light and ambient occlusion (M1 W5, `sapphire_render::ambient`), shared by the
// environment shader and the ambient pass (probes.wgsl, occlusion.wgsl):
//
// - **The probe grid**, relit as the light changes (probes.wgsl's `cs_relight`) into one 3D
//   atlas of bricks, 5³ texels each (a brick's 4³ cells with the probes it shares with its
//   neighbors), its sub-volumes stacked along z: red's, green's and blue's L1 irradiance (renderer
//   normalization: a uniform radiance of 1 is an irradiance of 1), the sky visibility's L1, and
//   the probe data the leak test reads (how far each probe sees along each axis, and whether it
//   lies in open space). A brick table (`probe_bricks`) maps each brick of the grid's extent to
//   its brick in the atlas.
// - **Capsule AO** from the characters' bodies (`ambient.capsules`).
// - **GTAO** at half resolution (occlusion.wgsl), read back here through a depth-aware upsample.
// - **The reflection cube**: the sky cube prefiltered into roughness mips, at the authored
//   ambient's strength (probes.wgsl's `cs_prefilter`).
//
// Everything a surface reads is behind the environment pipelines' constants (environment.wgsl),
// so pipelines without a grid or occlusion compile to what they were without them.

struct AmbientCapsule {
    // xyz: the body's base; w: its radius.
    base: vec4<f32>,
    // xyz: its top.
    top: vec4<f32>,
}

const MAX_AMBIENT_CAPSULES: u32 = 16u;

struct AmbientUniforms {
    // xyz: the grid's first probe, world space; w: meters between probes.
    grid_origin: vec4<f32>,
    // xyz: cells along each axis (bricks × 4).
    grid_cells: vec4<f32>,
    // xyz: bricks along each axis; w: 1 with a grid, 0 without.
    grid_bricks: vec4<u32>,
    // xyz: the atlas's bricks along each axis; w: texels of one sub-volume along z.
    atlas: vec4<u32>,
    // xyz: one atlas texel in texture coordinates; w: the reflection cube's roughest mip.
    atlas_texel: vec4<f32>,
    // xy: the GTAO texture's size in texels; zw: one texel in texture coordinates.
    occlusion_size: vec4<f32>,
    // x: capsules; y: character entries; z: the first sky cube face the projection projects
    // (`probes.wgsl`); w: the faces it projects.
    counts: vec4<u32>,
    capsules: array<AmbientCapsule, MAX_AMBIENT_CAPSULES>,
}

@group(0) @binding(11) var<uniform> ambient: AmbientUniforms;
@group(0) @binding(12) var probe_atlas: texture_3d<f32>;
@group(0) @binding(13) var probe_bricks: texture_3d<u32>;
@group(0) @binding(14) var reflection_cube: texture_cube<f32>;
// x: GTAO's visibility; y: the view depth it was computed at, for the upsample.
@group(0) @binding(15) var occlusion_map: texture_2d<f32>;
// Linear, clamped: the atlas's trilinear lookups and the reflection cube's mips.
@group(0) @binding(16) var ambient_sampler: sampler;

// The atlas's sub-volumes, along z.
const ATLAS_RED: u32 = 0u;
const ATLAS_GREEN: u32 = 1u;
const ATLAS_BLUE: u32 = 2u;
const ATLAS_VISIBILITY: u32 = 3u;
const ATLAS_DATA: u32 = 4u;
const ATLAS_SUB_VOLUMES: u32 = 5u;
const BRICK_CELLS: u32 = 4u;
const BRICK_SIDE: u32 = 5u;
const EMPTY_BRICK: u32 = 0xffffffffu;

// How far a lookup moves off its surface before it reads the grid, in probe spacings: along the
// surface's normal, away from what lies behind it, and toward the camera, so the probes it reads
// are on the side it is seen from (Unity's and DDGI's bias).
const PROBE_NORMAL_BIAS: f32 = 0.3;
const PROBE_VIEW_BIAS: f32 = 0.1;
// A probe inside geometry keeps this share of its weight.
const PROBE_INVALID_WEIGHT: f32 = 0.05;
// The probe data's flags (its alpha, `probes.wgsl`'s relight): the probe is in open space; the
// cell it is the lowest corner of is open; every brick the probe belongs to is kept (it isn't on
// the edge of the grid's bricks).
const PROBE_VALID: u32 = 1u;
const PROBE_CELL_OPEN: u32 = 2u;
const PROBE_INSIDE: u32 = 4u;
// The leak test's softness: a corner fades out over this share of a spacing past where it sees
// geometry between it and the point.
const PROBE_LEAK_SOFTNESS: f32 = 0.125;

// The L1 SH basis' constants (sapphire_math::sh).
const SH_C0: f32 = 0.2820948;
const SH_C1: f32 = 0.48860252;

// The function L1 coefficients `c` (Y₀₀, Y₁₋₁ ∝ y, Y₁₀ ∝ z, Y₁₁ ∝ x) hold, in direction `d`.
fn sh_l1(c: vec4<f32>, d: vec3<f32>) -> f32 {
    return SH_C0 * c.x + SH_C1 * (c.y * d.y + c.z * d.z + c.w * d.x);
}

// What the grid holds at a point: L1 irradiance per channel, the sky visibility's L1, and how
// much the grid speaks for the point: 0 outside it, in an empty brick or where the leak test
// drops every probe around it, and fading to 0 over the last cell before its edge or an empty
// brick.
struct GridSample {
    red: vec4<f32>,
    green: vec4<f32>,
    blue: vec4<f32>,
    visibility: vec4<f32>,
    weight: f32,
}

// One of the probe data's axis pairs: (toward +axis, toward −axis), in 31sts of a spacing.
fn free_pair(packed: f32) -> vec2<f32> {
    let plus = floor(packed / 32.0 + 1e-3);
    return vec2<f32>(plus, packed - plus * 32.0) / 31.0;
}

// The grid at `position` on a surface with unit normal `normal`, seen along unit `view_dir`
// (toward the camera); zero vectors for a lookup that isn't on a surface (a character's body).
//
// The cell's eight probes are weighed by trilinear interpolation, times, for each, whether it
// sees the point along each axis (the leak test: a probe whose free distance toward the point is
// shorter than the way to it has geometry between them, a thin wall, and drops out), whether it
// lies on the side the surface faces (DDGI's soft backface weight), and whether it is in open
// space. The weights' centroid within the cell is then sampled with the hardware's trilinear
// filter: exact where the test drops a whole face of the cell (a wall along an axis), and three
// filtered fetches per channel instead of eight; where it drops corners that aren't a face, the
// dropped ones keep a little of their light (under a tenth). An open cell (the relight's flag on
// its lowest corner: every corner in open space, inside the grid's bricks and seeing the others)
// passes every test, so its lookup reads that one corner's data instead of all eight.
//
// How much the grid speaks for the point is the trilinear share of its corners inside the
// grid's bricks: probes on the edge of an empty brick or of the grid take none, so the grid
// fades into the hemisphere over the last cell before them, and a character rising past the
// geometry the bake kept bricks around never pops between them. Surfaces lie at least a cell
// inside the kept bricks (the bake keeps a brick within a cell of geometry), so they never
// fade.
fn grid_lookup(position: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>) -> GridSample {
    var result: GridSample;
    let spacing = ambient.grid_origin.w;
    let biased = position + (normal * PROBE_NORMAL_BIAS + view_dir * PROBE_VIEW_BIAS) * spacing;
    let g = (biased - ambient.grid_origin.xyz) / spacing;
    let cells = ambient.grid_cells.xyz;
    let cell = clamp(floor(g), vec3<f32>(0.0), max(cells - 1.0, vec3<f32>(0.0)));
    let f = clamp(g - cell, vec3<f32>(0.0), vec3<f32>(1.0));
    let cell_index = vec3<u32>(cell);
    let entry = textureLoad(probe_bricks, vec3<i32>(cell_index / BRICK_CELLS), 0).r;
    let kept = entry != EMPTY_BRICK && ambient.grid_bricks.w != 0u;
    let brick = select(0u, entry, kept);
    let atlas = ambient.atlas;
    let atlas_brick = vec3<u32>(brick % atlas.x, (brick / atlas.x) % atlas.y, brick / (atlas.x * atlas.y));
    let base = atlas_brick * BRICK_SIDE + cell_index % BRICK_CELLS;
    let data_base = base + vec3<u32>(0u, 0u, ATLAS_DATA * atlas.w);

    let lowest = textureLoad(probe_atlas, vec3<i32>(data_base), 0);
    let open_cell = (u32(lowest.w + 0.5) & PROBE_CELL_OPEN) != 0u;
    var weights = 0.0;
    var centroid = vec3<f32>(0.0);
    // An open cell's corners are all inside.
    var inside = select(0.0, 1.0, open_cell);
    for (var corner = 0u; corner < 8u; corner++) {
        let offset = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, corner >> 2u);
        let at = vec3<f32>(offset);
        let trilinear = select(1.0 - f, f, offset == vec3<u32>(1u));
        let share = trilinear.x * trilinear.y * trilinear.z;
        let facing = (dot(safe_normalize(at - f), normal) + 1.0) * 0.5;
        var weight = share * (facing * facing + 0.2);
        if !open_cell {
            var data = lowest;
            if corner != 0u {
                data = textureLoad(probe_atlas, vec3<i32>(data_base + offset), 0);
            }
            // From the probe to the point, in cells along each axis.
            let toward = f - at;
            let free = vec3<f32>(
                select(free_pair(data.x).y, free_pair(data.x).x, toward.x > 0.0),
                select(free_pair(data.y).y, free_pair(data.y).x, toward.y > 0.0),
                select(free_pair(data.z).y, free_pair(data.z).x, toward.z > 0.0),
            );
            let sees = saturate((free - abs(toward)) / PROBE_LEAK_SOFTNESS + 1.0);
            let flags = u32(data.w + 0.5);
            let valid = (flags & PROBE_VALID) != 0u;
            weight *= sees.x * sees.y * sees.z * select(PROBE_INVALID_WEIGHT, 1.0, valid);
            inside += select(0.0, share, (flags & PROBE_INSIDE) != 0u);
        }
        weights += weight;
        centroid += weight * at;
    }
    // Every probe dropped (a point the probes around it all see walls toward): no grid light.
    let seen = weights > 1e-6;
    let filtered = select(f, centroid / max(weights, 1e-12), seen);
    let texel = ambient.atlas_texel.xyz;
    let coords = (vec3<f32>(base) + 0.5 + filtered) * texel;
    let sub_volume = vec3<f32>(0.0, 0.0, f32(atlas.w) * texel.z);
    result.red = textureSampleLevel(probe_atlas, ambient_sampler, coords + sub_volume * f32(ATLAS_RED), 0.0);
    result.green = textureSampleLevel(probe_atlas, ambient_sampler, coords + sub_volume * f32(ATLAS_GREEN), 0.0);
    result.blue = textureSampleLevel(probe_atlas, ambient_sampler, coords + sub_volume * f32(ATLAS_BLUE), 0.0);
    result.visibility =
        textureSampleLevel(probe_atlas, ambient_sampler, coords + sub_volume * f32(ATLAS_VISIBILITY), 0.0);
    result.weight = select(0.0, inside, kept && seen);
    return result;
}

// The grid's irradiance toward unit `n`.
fn grid_irradiance(grid: GridSample, n: vec3<f32>) -> vec3<f32> {
    return max(vec3<f32>(sh_l1(grid.red, n), sh_l1(grid.green, n), sh_l1(grid.blue, n)), vec3<f32>(0.0));
}

// The share of the sky a point sees along unit `d`, from the grid's sky visibility.
fn grid_sky_visibility(grid: GridSample, d: vec3<f32>) -> f32 {
    return saturate(sh_l1(grid.visibility, d));
}

// Capsule AO: how much light from the hemisphere around unit `n` at `position` the characters'
// bodies leave. Each body is a sphere at the point of its axis nearest the position, whose
// cosine-weighted solid angle it takes (Quílez's sphere occlusion), faded out by four radii.
fn capsule_occlusion(position: vec3<f32>, n: vec3<f32>) -> f32 {
    var visibility = 1.0;
    let count = min(ambient.counts.x, MAX_AMBIENT_CAPSULES);
    for (var index = 0u; index < count; index++) {
        let capsule = ambient.capsules[index];
        let a = capsule.base.xyz;
        let radius = capsule.base.w;
        let axis = capsule.top.xyz - a;
        let along = select(0.0, saturate(dot(position - a, axis) / dot(axis, axis)), dot(axis, axis) > 1e-8);
        let to_center = a + along * axis - position;
        let distance = max(length(to_center), 1e-4);
        let facing = saturate(dot(n, to_center / distance));
        let ratio = radius / max(distance, radius);
        let reach = 1.0 - smoothstep(2.0 * radius, 4.0 * radius, distance - radius);
        visibility *= 1.0 - facing * ratio * ratio * reach;
    }
    return visibility;
}

// A half-resolution GTAO texel holds the pixel's own surface while their view depths differ by
// less than this share of the pixel's.
const GTAO_UPSAMPLE_TOLERANCE: f32 = 0.1;

// GTAO's visibility at the pixel `pixel` (framebuffer coordinates) whose view depth is `depth`:
// the half-resolution term upsampled by those of its four nearest texels that hold the pixel's
// surface, each weighed by bilinear distance and by how close its depth is to the pixel's, so
// occlusion never bleeds across a silhouette. Where none does (a surface GTAO's input doesn't
// hold), open.
fn gtao_visibility(pixel: vec2<f32>, depth: f32) -> f32 {
    let size = ambient.occlusion_size.xy;
    let at = (pixel - 0.5) * 0.5;
    let corner = floor(at);
    let f = at - corner;
    var sum = 0.0;
    var weights = 0.0;
    for (var i = 0u; i < 4u; i++) {
        let offset = vec2<f32>(f32(i & 1u), f32(i >> 1u));
        let texel = clamp(corner + offset, vec2<f32>(0.0), size - 1.0);
        let sample = textureLoad(occlusion_map, vec2<i32>(texel), 0);
        // A texel at no bilinear weight still counts a little, so a pixel whose only match is
        // the farthest of the four keeps it.
        let bilinear = mix(1.0 - f, f, offset);
        let difference = abs(sample.y - depth) / max(depth, 1e-3);
        let similar = select(0.0, 1.0 / (1e-3 + difference), difference <= GTAO_UPSAMPLE_TOLERANCE);
        let weight = (bilinear.x * bilinear.y + 1e-3) * similar;
        sum += sample.x * weight;
        weights += weight;
    }
    return select(1.0, sum / weights, weights > 0.0);
}

// Jimenez et al.'s fit of the light ambient occlusion keeps by bouncing between the occluding
// surfaces of `albedo` (GTAO, 2016): darker surfaces lose more.
fn multibounce(visibility: f32, albedo: vec3<f32>) -> vec3<f32> {
    let a = 2.0404 * albedo - 0.3324;
    let b = -4.7951 * albedo + 0.6417;
    let c = 2.7552 * albedo + 0.6903;
    let v = vec3<f32>(visibility);
    return max(v, ((v * a + b) * v + c) * v);
}

// Lagarde's specular occlusion from ambient occlusion `visibility`, at `n_dot_v` and perceptual
// `roughness` ("Moving Frostbite to PBR", 2014).
fn specular_occlusion(n_dot_v: f32, visibility: f32, roughness: f32) -> f32 {
    return saturate(pow(n_dot_v + visibility, exp2(-16.0 * roughness - 1.0)) - 1.0 + visibility);
}
