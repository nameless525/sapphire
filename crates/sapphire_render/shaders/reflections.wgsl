// Reflection probes as surfaces read them (M5 L3; the `ambient::reflection_probes` module): a
// zone's local cubes, each its baked keys blended by the day's key with the live sky added through
// their baked sky share, prefiltered into a roughness chain (`probes.wgsl`'s composition), read
// through `probe_cube_sample` (`probe_cubes.wgsl` or `probe_faces.wgsl`).
//
// The environment finds the probe of the cell around a point in a table built at load from the
// probes' reaches (the two probes reaching into a cell with the largest weight anywhere in it, and
// of those the one whose weight at the point is larger), corrects the
// reflected ray by the box (box parallax, `REFLECTION_PROBES` 2) or reads along it as it is (1),
// and takes the probe whole inside its box and on its faces, where a room's walls lie, fading to
// the light it read before probes (the grid's or the hemisphere's) over a short reach past them;
// beyond every reach and with specular off, that light alone. A character reads
// the probe its record names (`CharacterReflection`, chosen on the CPU by where it stands) along
// the plain direction. Either replaces only the source of the reflected light, never how a
// surface responds to it.

// One probe as the environment reads it (`ambient::reflection_probes::GpuProbe`). 48 bytes.
struct ReflectionProbe {
    // xyz: the least corner of its box, world space; w: 0.
    box_min: vec4<f32>,
    // xyz: the greatest corner; w: how far past the box its cube reaches, fading out, metres.
    box_max: vec4<f32>,
    // xyz: where its cubes were baked from; w: 0.
    position: vec4<f32>,
}

// The environment's probes (`ambient::reflection_probes::ProbeUniforms`). 2,048 bytes.
struct ReflectionUniforms {
    // xyz: the cell table's least corner, world space; w: a cell's side, metres.
    cells_origin: vec4<f32>,
    // xyz: cells along each axis (0 without probes); w: probes.
    cells: vec4<u32>,
    probes: array<ReflectionProbe, 42>,
}

// Per cell, the two probes the environment reads there (`CellTable`), or `NO_CELL_PROBE`.
@group(0) @binding(18) var probe_cells: texture_3d<u32>;
@group(0) @binding(19) var<uniform> reflection: ReflectionUniforms;

// Whether the environment reads probes: 0 not (no probes expected, or Mobile-Low, which has no
// environment specular), 1 along the reflected ray as it is (Mobile-High), 2 with box parallax
// (the desktop tiers).
override REFLECTION_PROBES: u32 = 0u;

// A cell with no probe (`ambient::reflection_probes::NO_CELL_PROBE`).
const NO_CELL_PROBE: u32 = 255u;
// A character with no probe (`ambient::reflection_probes::NO_PROBE`).
const NO_CHARACTER_PROBE: u32 = 0xffffffffu;

// The two probes the cell around `position` names, or `NO_CELL_PROBE` outside the table.
fn cell_probes(position: vec3<f32>) -> vec2<u32> {
    let dims = vec3<f32>(reflection.cells.xyz);
    let cell = floor((position - reflection.cells_origin.xyz) / reflection.cells_origin.w);
    let inside = all(cell >= vec3<f32>(0.0)) && all(cell < dims);
    let clamped = vec3<i32>(clamp(cell, vec3<f32>(0.0), max(dims - 1.0, vec3<f32>(0.0))));
    let named = textureLoad(probe_cells, clamped, 0).rg;
    return select(vec2<u32>(NO_CELL_PROBE), named, inside);
}

// How much of probe `index`'s cube a point at `position` takes: 1 inside its box and on its faces,
// falling to 0 over the box's fade past them, by the distance to the box (`ProbeBox::weight`).
fn probe_weight(index: u32, position: vec3<f32>) -> f32 {
    let probe = reflection.probes[index];
    let outside = max(max(probe.box_min.xyz - position, position - probe.box_max.xyz), vec3<f32>(0.0));
    return saturate(1.0 - length(outside) / probe.box_max.w);
}

// The light the environment reflects at `position` along `reflected` at `roughness`: `fallback`
// (what it read before probes) where no probe reaches, else the cube of the one of its cell's two
// probes whose weight at `position` is larger (the first where they tie), fading into it past the
// box's faces (`CellTable::probe_at`).
fn environment_reflection(position: vec3<f32>, reflected: vec3<f32>, roughness: f32, fallback: vec3<f32>) -> vec3<f32> {
    let named = cell_probes(position);
    var index = named.x;
    var weight = 0.0;
    if index != NO_CELL_PROBE {
        weight = probe_weight(index, position);
    }
    if named.y != NO_CELL_PROBE {
        let other = probe_weight(named.y, position);
        if other > weight {
            index = named.y;
            weight = other;
        }
    }
    var light = fallback;
    if index != NO_CELL_PROBE {
        let probe = reflection.probes[index];
        var direction = reflected;
        if REFLECTION_PROBES == 2u {
            // Where the ray leaves the box, seen from where the probe was baked (Lagarde and
            // Zanuttini 2012). A component near 0 never leaves through its pair of faces; a point
            // in the reach past the box, heading away, reads toward itself.
            let toward = select(reflected, vec3<f32>(1e-6), abs(reflected) < vec3<f32>(1e-6));
            let far = max((probe.box_max.xyz - position) / toward, (probe.box_min.xyz - position) / toward);
            let distance = max(min(min(far.x, far.y), far.z), 0.0);
            direction = position + reflected * distance - probe.position.xyz;
        }
        let level = roughness * (probe_cube_levels() - 1.0);
        let probed = probe_cube_sample(direction, index, level);
        light = mix(fallback, probed, weight);
    }
    return light;
}

// The light a character reflects along `direction` at `roughness`: `fallback` (the sky's
// reflection) without a probe, else its probe's cube by its weight.
fn character_reflection(probe: CharacterReflection, direction: vec3<f32>, roughness: f32, fallback: vec3<f32>) -> vec3<f32> {
    var light = fallback;
    if probe.probe != NO_CHARACTER_PROBE {
        let level = roughness * (probe_cube_levels() - 1.0);
        light = mix(fallback, probe_cube_sample(direction, probe.probe, level), probe.weight);
    }
    return light;
}
