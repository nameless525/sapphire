// The light-cluster pass (spec §5.12 "Lighting", clustered forward+): bins the frame's local
// lights into the 16 × 9 × 24 cluster grid of the main view, one thread per cluster. Mirrors the
// CPU reference in `src/lights/clusters.rs`, which the GPU tests compare it with.
//
// Bind groups: 0 holds the frame constants, the lights and the cluster buffer (written here,
// read by the anime character pass); 1 is the main view.
//
// Each workgroup walks the lights in batches of one per thread: every thread moves one light
// into view space and workgroup memory, then every thread tests the whole batch against its
// cluster. A cluster lists the lights it touches in index order and keeps the first
// `frame.lights.w`; the CPU puts lights in packet order, so the packet's first lights win in a
// cluster that overflows.

@group(0) @binding(2) var<storage, read_write> cluster_lists: array<u32>;

const CLUSTER_WORKGROUP: u32 = 128u;

// A light as the culling tests read it, in view space.
struct ClusterLight {
    // xyz: center; w: radius, the range with a little padding (see CLUSTER_CULL_PADDING).
    sphere: vec4<f32>,
    // xyz: the spot's axis; w: cos(outer angle). The cone test runs only for w > 0: point
    // lights (w = −1) and spots wider than 90° are culled as spheres.
    cone: vec4<f32>,
}

// Grows each light's culling radius, so a pixel whose depth rounds into a neighboring slice
// still finds every light that reaches it.
const CLUSTER_CULL_PADDING: f32 = 1.001;

var<workgroup> batch: array<ClusterLight, CLUSTER_WORKGROUP>;

// A cluster's view-space box (camera looking down −Z).
struct ClusterBox {
    lower: vec3<f32>,
    upper: vec3<f32>,
}

// The view depth range of `slice`: slice 0 starts at the near plane, the last one ends where
// the farthest light does.
fn slice_depths(slice: u32) -> vec2<f32> {
    let start = frame.clusters.y;
    let per_octave = frame.clusters.z;
    var near = frame.clusters.x;
    if slice > 0u {
        near = start * exp2(f32(slice - 1u) / per_octave);
    }
    var far = frame.clusters.w;
    if slice < CLUSTER_SLICES - 1u {
        far = start * exp2(f32(slice) / per_octave);
    }
    return vec2<f32>(near, far);
}

// The box around the frustum piece of `cluster`: its tile's four corners unprojected at the
// slice's two depths. Unprojecting through the inverse projection handles any perspective
// projection, off-center (jittered) ones included.
fn cluster_box(cluster: u32) -> ClusterBox {
    let x = cluster % CLUSTER_TILES_X;
    let y = (cluster / CLUSTER_TILES_X) % CLUSTER_TILES_Y;
    let slice = cluster / (CLUSTER_TILES_X * CLUSTER_TILES_Y);
    let tiles = vec2<f32>(f32(CLUSTER_TILES_X), f32(CLUSTER_TILES_Y));
    // Tile rows count down from the top, NDC y up.
    let ndc_lower = vec2<f32>(f32(x) / tiles.x * 2.0 - 1.0, 1.0 - f32(y + 1u) / tiles.y * 2.0);
    let ndc_upper = vec2<f32>(f32(x + 1u) / tiles.x * 2.0 - 1.0, 1.0 - f32(y) / tiles.y * 2.0);
    let depths = slice_depths(slice);
    let view_from_clip = view.view_from_world * view.world_from_clip;
    var bounds: ClusterBox;
    bounds.lower = vec3<f32>(3.0e38);
    bounds.upper = vec3<f32>(-3.0e38);
    for (var corner = 0u; corner < 8u; corner++) {
        let ndc = select(ndc_lower, ndc_upper, vec2<bool>((corner & 1u) != 0u, (corner & 2u) != 0u));
        // Reverse-Z with an infinite far plane: depth d is at NDC z = near / d.
        let depth = select(depths.x, depths.y, corner >= 4u);
        let point = view_from_clip * vec4<f32>(ndc, frame.clusters.x / depth, 1.0);
        let p = point.xyz / point.w;
        bounds.lower = min(bounds.lower, p);
        bounds.upper = max(bounds.upper, p);
    }
    return bounds;
}

fn cluster_light(light: LocalLight) -> ClusterLight {
    var out: ClusterLight;
    let center = view.view_from_world * vec4<f32>(light.position.xyz, 1.0);
    out.sphere = vec4<f32>(center.xyz, light.position.w * CLUSTER_CULL_PADDING);
    let axis = safe_normalize((view.view_from_world * vec4<f32>(light.spot.xyz, 0.0)).xyz);
    out.cone = vec4<f32>(axis, light.spot.w);
    return out;
}

fn sphere_touches_box(sphere: vec4<f32>, bounds: ClusterBox) -> bool {
    let offset = sphere.xyz - clamp(sphere.xyz, bounds.lower, bounds.upper);
    return dot(offset, offset) <= sphere.w * sphere.w;
}

// Whether a spot's cone can reach a sphere around the cluster (Bart Wronski, "Cull that cone",
// 2017): culled if the sphere lies wholly outside the cone's angle, beyond its range, or behind
// its apex.
fn cone_touches_sphere(light: ClusterLight, center: vec3<f32>, radius: f32) -> bool {
    let cos_outer = light.cone.w;
    if cos_outer <= 0.0 {
        return true;
    }
    let sin_outer = sqrt(max(1.0 - cos_outer * cos_outer, 0.0));
    let v = center - light.sphere.xyz;
    let along = dot(v, light.cone.xyz);
    let across = sqrt(max(dot(v, v) - along * along, 0.0));
    let outside_angle = cos_outer * across - along * sin_outer > radius;
    let beyond = along > radius + light.sphere.w;
    let behind = along < -radius;
    return !(outside_angle || beyond || behind);
}

@compute @workgroup_size(128)
fn cs_clusters(
    @builtin(global_invocation_id) global: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    let cluster = global.x;
    let in_grid = cluster < CLUSTER_COUNT;
    let bounds = cluster_box(min(cluster, CLUSTER_COUNT - 1u));
    let center = (bounds.lower + bounds.upper) * 0.5;
    let radius = length(bounds.upper - bounds.lower) * 0.5;

    let light_count = frame.lights.x;
    let capacity = frame.lights.w;
    let base = cluster * frame.lights.z;
    var count = 0u;
    // Indices waiting for the high half of their word.
    var pair = 0u;
    for (var first = 0u; first < light_count; first += CLUSTER_WORKGROUP) {
        if first + local < light_count {
            batch[local] = cluster_light(lights[first + local]);
        }
        workgroupBarrier();
        let batch_len = min(CLUSTER_WORKGROUP, light_count - first);
        for (var i = 0u; i < batch_len && in_grid && count < capacity; i++) {
            let light = batch[i];
            if sphere_touches_box(light.sphere, bounds) && cone_touches_sphere(light, center, radius) {
                let index = first + i;
                if (count & 1u) == 0u {
                    pair = index;
                } else {
                    cluster_lists[base + 1u + count / 2u] = pair | (index << 16u);
                }
                count++;
            }
        }
        workgroupBarrier();
    }
    if in_grid {
        if (count & 1u) == 1u {
            cluster_lists[base + 1u + count / 2u] = pair;
        }
        cluster_lists[base] = count;
    }
}
