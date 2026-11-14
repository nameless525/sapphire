// Reflection probes' composed cubes (M5 L3), read as a cube array: the read of devices that have
// them. `probe_faces.wgsl` is the same read from a 2D array of faces, for devices that don't; a
// program is assembled with one or the other (`shaders::ProbeRead`). Both follow `ambient.wgsl`,
// whose sampler they read with.
//
// Cube p is probe p, each level a step of the roughness chain the composition prefilters.

@group(0) @binding(17) var probe_cubes: texture_cube_array<f32>;

// Probe `probe`'s composed cube toward `direction` (any length but zero), at level `level`.
fn probe_cube_sample(direction: vec3<f32>, probe: u32, level: f32) -> vec3<f32> {
    return textureSampleLevel(probe_cubes, ambient_sampler, direction, i32(probe), level).rgb;
}

// The levels of the composed cubes' roughness chain.
fn probe_cube_levels() -> f32 {
    return f32(textureNumLevels(probe_cubes));
}
