// Local lights (spec §5.12 "Lighting", M1 R5), shared by the anime character and environment
// passes and the light-cluster pass. `shaders.rs` puts it after frame.wgsl and before the pass's
// own file. Mirrors `src/lights.rs` and `src/lights/clusters.rs`.
//
// `frame.lights` says what group 0 binding 2 holds this frame:
//   x  local lights in `lights`
//   y  LIGHT_LISTS_*: none, a list per cluster (clustered forward+, desktop) or a list per
//      instance slot (per-object lists, mobile)
//   z  words per list in the list buffer
//   w  most lights one list holds
// `frame.clusters` places the cluster grid's depth slices:
//   x  the projection's near plane: view depth is x / fragment depth (reverse-Z, infinite far)
//   y  where the exponential slices start; slice 0 is everything nearer
//   z  slices per doubling of depth beyond y
//   w  how deep the last slice reaches for culling (it runs to infinity for pixels)
//
// A list is a count followed by 16-bit light indices, two per word, low half first.

struct LocalLight {
    // xyz: world position; w: range in meters.
    position: vec4<f32>,
    // rgb: color × intensity, the light at its own position; w: 1 / range².
    radiance: vec4<f32>,
    // xyz: unit vector the spot points along (zero for point lights); w: cos(outer angle), −1
    // for point lights.
    spot: vec4<f32>,
    // xy: the cone as saturate(cos θ × x + y) before the smoothstep (0 and 1 for point lights);
    // zw: unused.
    cone: vec4<f32>,
}

@group(0) @binding(1) var<storage, read> lights: array<LocalLight>;

const LIGHT_LISTS_NONE: u32 = 0u;
const LIGHT_LISTS_CLUSTERS: u32 = 1u;
const LIGHT_LISTS_OBJECTS: u32 = 2u;

// The cluster grid: 16 × 9 screen tiles by 24 depth slices; cluster index
// (slice × 9 + tile y) × 16 + tile x, with tile y counted from the top.
const CLUSTER_TILES_X: u32 = 16u;
const CLUSTER_TILES_Y: u32 = 9u;
const CLUSTER_SLICES: u32 = 24u;
const CLUSTER_COUNT: u32 = 3456u;

// The depth slice holding view-space depth `depth` (meters in front of the camera).
fn cluster_slice(depth: f32) -> u32 {
    let start = frame.clusters.y;
    if depth < start {
        return 0u;
    }
    let slice = 1.0 + floor(log2(depth / start) * frame.clusters.z);
    return u32(min(slice, f32(CLUSTER_SLICES - 1u)));
}

// The light list a fragment walks: on the clustered path its cluster's, from the fragment's
// position (`@builtin(position)`: pixel coordinates and reverse-Z depth), on the per-object path
// its instance slot's (`instance`).
fn light_list(clip_position: vec4<f32>, instance: u32) -> u32 {
    if frame.lights.y == LIGHT_LISTS_OBJECTS {
        return instance;
    }
    let tiles = vec2<f32>(f32(CLUSTER_TILES_X), f32(CLUSTER_TILES_Y));
    let tile = min(vec2<u32>(clip_position.xy * view.viewport.zw * tiles), vec2<u32>(tiles) - 1u);
    // Reverse-Z with an infinite far plane: view depth is near / fragment depth.
    let slice = cluster_slice(frame.clusters.x / max(clip_position.z, 1e-30));
    return (slice * CLUSTER_TILES_Y + tile.y) * CLUSTER_TILES_X + tile.x;
}

// The light's smooth range window: 1 at the light, falling to 0 at its range with zero slope,
// so a light's reach ends without an edge. There is no inverse-square hot spot: intensity is
// the light's brightness wherever it is near, as two-tone lighting wants.
fn light_window(distance_squared: f32, inverse_range_squared: f32) -> f32 {
    let fade = saturate(1.0 - distance_squared * inverse_range_squared);
    return fade * fade;
}

// The spot cone toward a point the unit vector `to_light` leads away from: smoothstep from the
// outer angle (0) to the inner angle (1). Always 1 for point lights.
fn light_cone(light: LocalLight, to_light: vec3<f32>) -> f32 {
    let t = saturate(dot(-to_light, light.spot.xyz) * light.cone.x + light.cone.y);
    return t * t * (3.0 - 2.0 * t);
}
