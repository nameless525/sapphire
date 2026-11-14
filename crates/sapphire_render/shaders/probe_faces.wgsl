// Reflection probes' composed cubes (M5 L3), read as a 2D array of faces with the face picked by
// hand: the read of devices without cube arrays (the iOS Simulator). `probe_cubes.wgsl` is the
// same read through a cube array; a program is assembled with one or the other
// (`shaders::ProbeRead`), since wgpu refuses a module that declares a cube array on a device
// without them, used or not. Both follow `ambient.wgsl`, whose sampler they read with.
//
// Layer 6p + f is probe p's face f, in a cube map's order (+X, −X, +Y, −Y, +Z, −Z), each level a
// step of the roughness chain the composition prefilters.

@group(0) @binding(17) var probe_cubes: texture_2d_array<f32>;

// Probe `probe`'s composed cube toward `direction` (any length but zero), at level `level`. The
// sampler clamps at a face's edge, so a texel by an edge filters within its face: within a few
// code values of the cube array's read, which filters across the seam (spike L0).
fn probe_cube_sample(direction: vec3<f32>, probe: u32, level: f32) -> vec3<f32> {
    let a = abs(direction);
    var face: u32;
    var st: vec2<f32>;
    if a.x >= a.y && a.x >= a.z {
        face = select(1u, 0u, direction.x > 0.0);
        st = select(vec2<f32>(direction.z, -direction.y), vec2<f32>(-direction.z, -direction.y), direction.x > 0.0) / a.x;
    } else if a.y >= a.z {
        face = select(3u, 2u, direction.y > 0.0);
        st = select(vec2<f32>(direction.x, -direction.z), vec2<f32>(direction.x, direction.z), direction.y > 0.0) / a.y;
    } else {
        face = select(5u, 4u, direction.z > 0.0);
        st = select(vec2<f32>(-direction.x, -direction.y), vec2<f32>(direction.x, -direction.y), direction.z > 0.0) / a.z;
    }
    return textureSampleLevel(probe_cubes, ambient_sampler, st * 0.5 + 0.5, i32(probe * 6u + face), level).rgb;
}

// The levels of the composed cubes' roughness chain.
fn probe_cube_levels() -> f32 {
    return f32(textureNumLevels(probe_cubes));
}
