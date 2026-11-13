//! GPU tests of what [`GpuCaps`](super::GpuCaps) reports. Run with `cargo test -p sapphire_rhi -- --ignored`.

use wgpu::{TextureFormat, TextureUsages};

use crate::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

/// The reflection probes' storage (M5 L0): one 2D texture of `6 × n` `Rgba16Float` layers,
/// written by a compute shader through a 2D array storage view as the prefilter writes, holds
/// cube `i`'s faces at layers `6i..6i + 6` in wgpu's face order. It reads the same as a 2D
/// array of faces, the face picked by hand, on every device, and as a cube array where the
/// device has them. CPU adapters (lavapipe, WARP) must have them. Without them (the iOS
/// Simulator) wgpu must refuse a module that declares a cube array, which is what keeps the
/// cube array lookup off that device. Run with `cargo test -p sapphire_rhi -- --ignored`.
#[test]
#[ignore = "needs a GPU"]
fn probe_layers_read_as_faces_everywhere_and_as_cubes_where_the_device_has_them() {
    // A read's group: the view it reads, the sampler and what it saw.
    fn read_back<'a>(
        view: &'a wgpu::TextureView,
        sampler: &'a wgpu::Sampler,
        seen: &'a wgpu::Buffer,
    ) -> [wgpu::BindGroupEntry<'a>; 3] {
        [
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 3, resource: seen.as_entire_binding() },
        ]
    }

    const CUBES: u32 = 3;
    const SIDE: u32 = 4;
    // The directions each cube is read at: every face's centre, and a point off it.
    const DIRECTIONS: &str = r"
fn direction(face: u32, off_centre: bool) -> vec3<f32> {
    let axes = array<vec3<f32>, 6>(
        vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0),
    );
    let axis = axes[face];
    let off = 0.3 * vec3<f32>(axis.y + axis.z, axis.z + axis.x, axis.x + axis.y);
    return axis + select(vec3<f32>(0.0), off, off_centre);
}
";
    const FACES: &str = r"
@group(0) @binding(0) var written: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(1) var faces: texture_2d_array<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var<storage, read_write> seen: array<vec4<f32>>;

// Every texel of layer `l` holds (l, 1 + l, 0.5, 1): exact in half floats.
@compute @workgroup_size(4, 4, 1)
fn cs_write(@builtin(global_invocation_id) id: vec3<u32>) {
    let layer = f32(id.z);
    textureStore(written, vec2<i32>(id.xy), i32(id.z), vec4<f32>(layer, 1.0 + layer, 0.5, 1.0));
}

// One thread per cube, face and point: the face picked from the direction by hand.
@compute @workgroup_size(6, 2, 1)
fn cs_faces(@builtin(global_invocation_id) id: vec3<u32>) {
    let d = direction(id.x, id.y == 1u);
    let a = abs(d);
    var face: i32;
    var st: vec2<f32>;
    if a.x >= a.y && a.x >= a.z {
        face = select(1, 0, d.x > 0.0);
        st = select(vec2<f32>(d.z, -d.y), vec2<f32>(-d.z, -d.y), d.x > 0.0) / a.x;
    } else if a.y >= a.z {
        face = select(3, 2, d.y > 0.0);
        st = select(vec2<f32>(d.x, -d.z), vec2<f32>(d.x, d.z), d.y > 0.0) / a.y;
    } else {
        face = select(5, 4, d.z > 0.0);
        st = select(vec2<f32>(-d.x, -d.y), vec2<f32>(d.x, -d.y), d.z > 0.0) / a.z;
    }
    let layer = i32(id.z) * 6 + face;
    seen[(id.z * 6u + id.x) * 2u + id.y] = textureSampleLevel(faces, linear, st * 0.5 + 0.5, layer, 0.0);
}
";
    const CUBE_ARRAY: &str = r"
@group(0) @binding(1) var cubes: texture_cube_array<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var<storage, read_write> seen: array<vec4<f32>>;

@compute @workgroup_size(6, 2, 1)
fn cs_cubes(@builtin(global_invocation_id) id: vec3<u32>) {
    let d = direction(id.x, id.y == 1u);
    seen[(id.z * 6u + id.x) * 2u + id.y] = textureSampleLevel(cubes, linear, d, i32(id.z), 0.0);
}
";

    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    let desc = GpuContextDesc { hold_errors: true, ..GpuContextDesc::default() };
    let gpu = GpuContext::new(&instance, &desc, None).expect("device");
    let adapter = &gpu.caps().adapter;
    let on = format!("{} ({})", adapter.name, adapter.backend);
    let device = gpu.device();
    let module = |source: &str| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(format!("{DIRECTIONS}{source}").into()),
        })
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 6 * CUBES },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::Rgba16Float,
        usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = |dimension| {
        texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(dimension), ..Default::default() })
    };
    let faces = view(wgpu::TextureViewDimension::D2Array);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bytes = u64::from(12 * CUBES) * 16;
    let seen = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    // Runs `entry_point` of `module` over every cube, face and point (or every layer's texels
    // for `cs_write`) with `entries` as group 0.
    let dispatch = |module: &wgpu::ShaderModule, entry_point, entries: &[wgpu::BindGroupEntry<'_>]| {
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module,
            entry_point: Some(entry_point),
            compilation_options: Default::default(),
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            let layers = if entry_point == "cs_write" { 6 * CUBES } else { CUBES };
            pass.dispatch_workgroups(1, 1, layers);
        }
        gpu.queue().submit([encoder.finish()]);
    };
    // What the last read saw, checked against what was written.
    let check = |how: &str| {
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(&seen, 0, &read, 0, bytes);
        gpu.queue().submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |mapped| mapped.expect("mapped"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let mapped = read.slice(..).get_mapped_range().expect("mapped");
        let values: Vec<f32> =
            mapped.as_chunks::<4>().0.iter().map(|word| f32::from_le_bytes(*word)).collect();
        assert_eq!(gpu.first_error(), None, "{how} on {on}");
        for (index, value) in values.as_chunks::<4>().0.iter().enumerate() {
            let (layer, point) = (index / 2, ["centre", "off centre"][index % 2]);
            let (cube, face) = (layer / 6, layer % 6);
            let layer = layer as f32;
            assert_eq!(
                *value,
                [layer, 1.0 + layer, 0.5, 1.0],
                "{how}: cube {cube} face {face}, {point}, on {on}"
            );
        }
    };

    let faces_module = module(FACES);
    dispatch(
        &faces_module,
        "cs_write",
        &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&faces) }],
    );
    dispatch(&faces_module, "cs_faces", &read_back(&faces, &sampler, &seen));
    check("read as faces");

    if gpu.caps().cube_array_textures {
        let cubes = view(wgpu::TextureViewDimension::CubeArray);
        dispatch(&module(CUBE_ARRAY), "cs_cubes", &read_back(&cubes, &sampler, &seen));
        check("read as cubes");
    } else {
        assert_ne!(
            adapter.device_type,
            wgpu::DeviceType::Cpu,
            "{on} is a CPU adapter, which must have cube arrays"
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _ = module(CUBE_ARRAY);
        assert!(
            pollster::block_on(scope.pop()).is_some(),
            "{on} took a cube array shader without cube arrays"
        );
    }
}
