// Pass 1: sky gradient on a fullscreen triangle, drawn at the far plane (depth 0 in reverse-Z)
// without depth writes. M1+ adds animated 2D cloud layers and sun and moon discs (§5.12).

struct SkyVaryings {
    @builtin(position) clip_position: vec4<f32>,
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

@fragment
fn fs_sky(in: SkyVaryings) -> @location(0) vec4<f32> {
    // Unproject the pixel on the near plane (depth 1); the far plane is at infinity.
    let near = frame.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(near.xyz / near.w - frame.camera_position.xyz);
    return vec4<f32>(sky_color(dir), 1.0);
}
