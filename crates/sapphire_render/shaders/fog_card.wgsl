// Pass 1: fog cards (M1 W3; `sapphire_render::fog`, whose docs have the design): translucent
// sheets of mist on meshes, after every other transparent surface, back to front, without writing
// depth. The program is frame.wgsl, materials.wgsl, then this file.
//
// - **Facing** (the block's `modes.y`, `CardFacing`): world-placed cards draw as their mesh is;
//   camera-facing ones turn their object-space XY plane to the screen's around their instance's
//   origin, keeping the instance's scale (one card per instance); upright ones turn about the
//   world's up axis only.
// - **Density:** the map's R (the card's shape, soft at its edges) times its G (a tiling octave
//   drifting by the block's scroll), × the material's opacity; an unbound map is evenly dense.
// - **Fades:** toward the lens over the material's camera fade (invisible at its start, whole at
//   its end), and where the card meets geometry over its depth fade, a soft particle, by the
//   view depth between the card and what the scene prepass holds behind it. The prepass's depth
//   is bound where the frame has it (`FOG_CARD_DEPTH_FADE`: the desktop tiers, whose temporal
//   AA runs it); elsewhere (phones' MSAA in tile memory) the depth fade is off, a 1×1 stand-in
//   bound, and the camera fade and the map's soft edges carry it.
// - **Light:** the frame's fog lights the card: its tint × the fog's light at the card's distance
//   (`fog_ray`: its layers' colors by their depths, the sky's horizon where there are none) plus
//   its sun scatter × the fog's sun inscatter along the ray; then the fog in front covers it as
//   it does any surface. Alpha is its coverage, blended over what is behind, keeping scene
//   color's alpha (post's character mask).

// `CardFacing::code`.
const CARD_FACING_WORLD: u32 = 0u;
const CARD_FACING_UPRIGHT: u32 = 2u;
// The map's texture slot (`FogCardMaterial::MAP`).
const FOG_CARD_MAP: u32 = 0u;
// The shaders' clock wraps to this many seconds (`material::SHADER_TIME_PERIOD`): the drift is
// rounded to a whole number of repeats in it, so it never jumps at the wrap.
const FOG_CARD_TIME_PERIOD: f32 = 3600.0;

// Cards fade against the scene prepass's depth (`fog::FogCards`).
override FOG_CARD_DEPTH_FADE: bool = false;

// A private word view of the unchanged FogCardParams storage bytes. Keeping tint words
// intact here lets the multiplication below retain subnormal tint on flushing backends.
// Offsets and size match FogCardParams; the shared material source and archive stay unchanged.
struct FogCardWords {
    color: vec4<u32>,
    fade: vec4<f32>,
    scroll: vec4<f32>,
    modes: vec4<u32>,
    swizzles: vec4<u32>,
}

// The fog card family's blocks, by the draw record's material slot.
@group(2) @binding(1) var<storage, read> fog_card_materials: array<FogCardWords>;
// The scene prepass's depth where cards fade against it, else a 1×1 stand-in nothing reads.
@group(2) @binding(2) var scene_depth: texture_depth_2d;

// The fog card family's group 3 (`FogCardMaterial::TEXTURE_SLOTS`).
@group(3) @binding(0) var card_map: texture_2d<f32>;
@group(3) @binding(8) var material_sampler: sampler;

struct FogCardVaryings {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    // The material textures' coordinates.
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) material: u32,
}

@vertex
fn vs_fog_card(packed: PackedVertex, draw: DrawRecord) -> FogCardVaryings {
    let vertex = decode_vertex(packed, draw);
    let m = instances[draw.instance].world_from_object;
    var world = (m * vec4<f32>(vertex.position, 1.0)).xyz;
    let facing = fog_card_materials[draw.material].modes.y;
    if facing != CARD_FACING_WORLD {
        // The camera's axes in the world: the view matrix's rows.
        let v = view.view_from_world;
        let camera_right = vec3<f32>(v[0].x, v[1].x, v[2].x);
        var right = camera_right;
        var up = vec3<f32>(v[0].y, v[1].y, v[2].y);
        if facing == CARD_FACING_UPRIGHT {
            // Upright: the camera's right, level; the world's up.
            right = safe_normalize(vec3<f32>(camera_right.x, 0.0, camera_right.z));
            up = vec3<f32>(0.0, 1.0, 0.0);
        }
        let toward_camera = cross(right, up);
        let scale = vec3<f32>(length(m[0].xyz), length(m[1].xyz), length(m[2].xyz));
        let p = vertex.position * scale;
        world = m[3].xyz + right * p.x + up * p.y + toward_camera * p.z;
    }
    var out: FogCardVaryings;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    out.world_position = world;
    out.uv = vertex.uv;
    out.material = draw.material;
    return out;
}

// Texture slot `slot` at `uv`, through the slot's swizzle.
fn sample_card(swizzles: vec4<u32>, slot: u32, uv: MaterialUv) -> vec4<f32> {
    let sampled = textureSampleGrad(card_map, material_sampler, uv.uv, uv.dx, uv.dy);
    return material_swizzle(sampled, swizzles, slot);
}

// 0 at `start`, 1 from `end` on, eased; a step where they meet.
fn card_fade(start: f32, end: f32, x: f32) -> f32 {
    let t = saturate((x - start) / max(end - start, 1e-4));
    return t * t * (3.0 - 2.0 * t);
}

// Multiply a tint word without first materializing a subnormal float. A subnormal tint
// is signed_fraction * 2^-149. Construct the two normal factors by shifting their integer
// exponent fields: this cannot reassociate floating-point scale multiplies back into a tint
// that the backend flushes. Derived nonfinite lighting keeps the original operation.
fn card_tint_channel(word: u32, light: f32) -> f32 {
    let magnitude = word & 0x7fffffffu;
    let light_word = bitcast<u32>(light);
    let light_exponent = (light_word >> 23u) & 0xffu;
    var result: f32;
    if magnitude > 0u && magnitude < 0x00800000u && light_exponent > 64u && light_exponent < 255u {
        // Integer m in [1, 2^23) converts exactly to a normal float. Its exponent minus 85
        // remains normal, as does finite light's exponent minus 64 under this branch.
        let fraction_word = bitcast<u32>(f32(magnitude));
        let tint_scaled = bitcast<f32>((fraction_word - (85u << 23u)) | (word & 0x80000000u));
        let light_scaled = bitcast<f32>(light_word - (64u << 23u));
        result = light_scaled * tint_scaled;
    } else {
        // For finite light exponent <= 64, even the largest subnormal tint yields a value
        // below the smallest representable f32 subnormal. Normal tint/zero/nonfinite light
        // also retain their original multiplication here.
        result = bitcast<f32>(word) * light;
    }
    return result;
}

fn card_tinted_light(words: vec3<u32>, light: vec3<f32>) -> vec3<f32> {
    let magnitude = words & vec3<u32>(0x7fffffffu);
    var result: vec3<f32>;
    if any((magnitude > vec3<u32>(0u)) & (magnitude < vec3<u32>(0x00800000u))) {
        result = vec3<f32>(
            card_tint_channel(words.x, light.x),
            card_tint_channel(words.y, light.y),
            card_tint_channel(words.z, light.z),
        );
    } else {
        // Preserve the ordinary vector expression and order, including signed and zero words.
        result = bitcast<vec3<f32>>(words) * light;
    }
    return result;
}

@fragment
fn fs_fog_card(in: FogCardVaryings) -> @location(0) vec4<f32> {
    let card = fog_card_materials[in.material];
    // Derivatives first, in uniform control flow: the map is sampled on a branch.
    let uv = MaterialUv(in.uv, dpdx(in.uv), dpdy(in.uv));
    var density = 1.0;
    if material_has_texture(card.modes, FOG_CARD_MAP) {
        let shape = sample_card(card.swizzles, FOG_CARD_MAP, uv).r;
        // Whole repeats an hour, so the drift is periodic in the shaders' clock.
        let repeats = round(card.scroll.xy * FOG_CARD_TIME_PERIOD);
        let drift = fract(repeats * (frame.time.x / FOG_CARD_TIME_PERIOD));
        let octave = sample_card(card.swizzles, FOG_CARD_MAP, MaterialUv(uv.uv + drift, uv.dx, uv.dy)).g;
        density = shape * octave;
    }
    let offset = in.world_position - view.position.xyz;
    let distance = length(offset);
    let dir = offset / max(distance, 1e-6);
    var coverage = bitcast<f32>(card.color.a) * density * card_fade(card.fade.y, card.fade.z, distance);
    if FOG_CARD_DEPTH_FADE {
        // Reverse-Z with an infinite far plane: view depth is near / depth, and a pixel nothing
        // covered (depth 0) is as far as the sky.
        let near = view.clip_from_view[3][2];
        let behind = textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0);
        let scene = select(FOG_SKY_DISTANCE, near / behind, behind > 0.0);
        let card_depth = near / in.clip_position.z;
        coverage *= saturate((scene - card_depth) / max(card.fade.x, 1e-4));
    }
    let ray = fog_ray(view.position.xyz, dir, distance, 1.0);
    let lit = card_tinted_light(card.color.rgb, ray.light + card.fade.w * ray.sun);
    // The fog in front covers the card as any surface (`fogged`, from the ray in hand).
    var color = lit;
    if fog_on() {
        color = apply_fog(lit, ray);
    }
    return vec4<f32>(color, coverage);
}
