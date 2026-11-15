//! Whether an adapter runs a depth-killed fragment whole for its quad's derivatives: a probe draw, by
//! behaviour, never by an adapter's name. A 2 × 2 target (one quad) with a depth buffer: a near occluder
//! over exactly one of its pixels, then a far surface over all four, testing depth, whose fragment stage
//! computes `x` in a loop that runs longest in the occluded pixel and writes `x`'s derivatives. Drawn with
//! and without the occluder, the three pixels it leaves must hold the same values: their derivatives read
//! the killed pixel's `x`, and a GPU that runs it whole for them computes the same `x` there whether or not
//! it is drawn. The killed pixel takes each of the quad's four corners in turn (one corner alone can miss
//! it, as (0, 0) does below).
//!
//! As measured (M5 D10): on Metal (Apple M5) every shape is whole at every corner. On CI's lavapipe
//! (llvmpipe, LLVM 18) both looped shapes are not whole with the killed pixel at (1, 0), (0, 1) and (1, 1),
//! and whole at (0, 0); straight code is whole at all four. Our reading, not something the probe shows: its
//! derivatives read the killed pixel's `x` as its loops left it, not run to their end.

use super::*;

/// The probe's target: one quad.
const SIZE: (u32, u32) = (2, 2);

/// The looped shapes of `x`'s computation the probe draws, by fragment entry point: a plain counted loop,
/// and a loop that continues and returns from inside a called function around a depth compare (the shape
/// of the cascades' lookup, frame.wgsl's `cascade_visibility`). Its verdict is theirs.
pub(super) const LOOPED: [&str; 2] = ["counted", "shadowed"];

/// Straight code: a control, with nothing for a killed fragment to leave unfinished, whole on any adapter.
pub(super) const STRAIGHT: &str = "straight";

/// The probe's shader, with the killed pixel at `killed`.
fn source(killed: (u32, u32)) -> String {
    format!(
        "const KILLED: vec2<u32> = vec2<u32>({}u, {}u);
@group(0) @binding(0) var probe_map: texture_depth_2d;
@group(0) @binding(1) var probe_sampler: sampler_comparison;

fn corner(i: u32, depth: f32) -> vec4<f32> {{
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, depth, 1.0);
}}
@vertex fn vs_near(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{ return corner(i, 0.25); }}
@vertex fn vs_far(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{ return corner(i, 0.75); }}
@fragment fn fs_near() -> @location(0) vec4<f32> {{ return vec4<f32>(-1.0); }}

// How many times the pixel at `pixel` loops: the killed one longest, by far.
fn trips(pixel: vec2<u32>) -> u32 {{
    if all(pixel == KILLED) {{
        return 9u;
    }}
    return 1u + (pixel.x + pixel.y) % 2u;
}}
fn term(i: u32, p: vec2<f32>) -> f32 {{
    return 0.37 + 0.11 * f32(i) + 0.013 * p.x + 0.029 * p.y;
}}

fn counted(n: u32, p: vec2<f32>) -> f32 {{
    var x = 0.0;
    for (var i = 0u; i < n; i++) {{
        x += term(i, p);
    }}
    return x;
}}
// One lookup's visibility and how far it reaches, as `cascade_lookup` returns them.
fn lookup(index: u32, p: vec2<f32>) -> vec2<f32> {{
    let lit = textureSampleCompareLevel(probe_map, probe_sampler, vec2<f32>(0.5), 0.3 + 0.05 * f32(index));
    return vec2<f32>(0.25 + 0.5 * lit + term(index, p), f32(index));
}}
// The first lookup that holds the point, as `cascade_visibility` takes it.
fn shadowed(n: u32, p: vec2<f32>) -> f32 {{
    for (var index = 0u; index < 12u; index++) {{
        let found = lookup(index, p);
        if found.y + 1.0 < f32(n) {{
            continue;
        }}
        return found.x;
    }}
    return 1.0;
}}
fn straight(n: u32, p: vec2<f32>) -> f32 {{
    return term(n, p);
}}

fn derivatives(x: f32) -> vec4<f32> {{
    return vec4<f32>(dpdx(x), dpdy(x), fwidth(x), x);
}}
@fragment fn fs_counted(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {{
    return derivatives(counted(trips(vec2<u32>(at.xy)), at.xy));
}}
@fragment fn fs_shadowed(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {{
    return derivatives(shadowed(trips(vec2<u32>(at.xy)), at.xy));
}}
@fragment fn fs_straight(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {{
    return derivatives(straight(trips(vec2<u32>(at.xy)), at.xy));
}}
",
        killed.0, killed.1
    )
}

/// One shape's verdict with the killed pixel at one corner: whether the three pixels left held the same
/// values with and without the occluder, and both draws' values (`x`'s dpdx, dpdy, fwidth and `x`).
pub(super) struct Verdict {
    pub(super) shape: &'static str,
    pub(super) killed: (u32, u32),
    pub(super) whole: bool,
    pub(super) occluded: Vec<[f32; 4]>,
    pub(super) open: Vec<[f32; 4]>,
}

/// Every shape's verdict with the killed pixel at each of the quad's four corners.
pub(super) fn verdicts(gpu: &GpuContext) -> Vec<Verdict> {
    let device = gpu.device();
    let color_format = wgpu::TextureFormat::Rgba32Float;
    let depth_format = wgpu::TextureFormat::Depth32Float;
    let extent = wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 };
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    // The map the shadow-like shape compares against: cleared to 0.5 once.
    let map =
        texture(depth_format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
    let map_view = map.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &map_view,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.5), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        ..Default::default()
    });
    gpu.queue().submit([encoder.finish()]);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&map_view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = |module: &wgpu::ShaderModule, vertex: &str, fragment: &str| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(vertex),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(color_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let color = texture(color_format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth = texture(depth_format, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, depth_view) =
        (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    let padded = (SIZE.0 * 16).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * SIZE.1),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut verdicts = Vec::new();
    for killed in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("killed fragments probe"),
            source: wgpu::ShaderSource::Wgsl(source(killed).into()),
        });
        let near = pipeline(&module, "vs_near", "fs_near");
        for shape in LOOPED.into_iter().chain([STRAIGHT]) {
            let far = pipeline(&module, "vs_far", &format!("fs_{shape}"));
            let draw = |occluded: bool| -> Vec<[f32; 4]> {
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &color_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..Default::default()
                    });
                    pass.set_bind_group(0, &group, &[]);
                    if occluded {
                        pass.set_pipeline(&near);
                        pass.set_scissor_rect(killed.0, killed.1, 1, 1);
                        pass.draw(0..3, 0..1);
                        pass.set_scissor_rect(0, 0, SIZE.0, SIZE.1);
                    }
                    pass.set_pipeline(&far);
                    pass.draw(0..3, 0..1);
                }
                encoder.copy_texture_to_buffer(
                    color.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(padded),
                            rows_per_image: Some(SIZE.1),
                        },
                    },
                    extent,
                );
                gpu.queue().submit([encoder.finish()]);
                buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
                device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
                let texels = {
                    let mapped = buffer.get_mapped_range(..).expect("mapped");
                    mapped
                        .chunks_exact(padded as usize)
                        .flat_map(|row| {
                            row[..SIZE.0 as usize * 16].as_chunks::<16>().0.iter().map(|texel| {
                                let channels = texel.as_chunks::<4>().0;
                                [channels[0], channels[1], channels[2], channels[3]].map(f32::from_le_bytes)
                            })
                        })
                        .collect::<Vec<_>>()
                };
                buffer.unmap();
                texels
            };
            let (occluded, open) = (draw(true), draw(false));
            let killed_index = (killed.1 * SIZE.0 + killed.0) as usize;
            let left = |texels: &[[f32; 4]]| -> Vec<[u32; 4]> {
                let bits = |texel: &[f32; 4]| texel.map(f32::to_bits);
                texels
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != killed_index)
                    .map(|(_, t)| bits(t))
                    .collect()
            };
            let whole = left(&occluded) == left(&open);
            verdicts.push(Verdict { shape, killed, whole, occluded, open });
        }
    }
    verdicts
}

/// Whether the adapter runs a depth-killed fragment whole for its quad's derivatives: both looped shapes,
/// the killed pixel at every corner, leave the other three pixels as they are without the occluder.
pub(super) fn derivatives_read_a_killed_fragment_whole(gpu: &GpuContext) -> bool {
    verdicts(gpu).iter().filter(|verdict| LOOPED.contains(&verdict.shape)).all(|verdict| verdict.whole)
}
