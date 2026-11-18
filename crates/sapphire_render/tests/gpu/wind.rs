//! The wind field's one sampler, on the GPU and the CPU (M6 Y1): `wind.wgsl`'s `wind_air`, read
//! from the frame's `WindBuffer` as foliage reads it, gives what `ResolvedWind::air` gives, within
//! 1e-5 m/s, at fixed points and times over layouts of every kind of volume, at each tier's GPU cap.
//! Run with `cargo test -p sapphire_render --test gpu wind:: -- --ignored --test-threads=1`.

use glam::Vec3;
use sapphire_look::wind::{MAX_WIND_VOLUMES, WIND_PERIOD};
use sapphire_look::{ResolvedWind, Wind, WindField, WindVolume};
use sapphire_render::ShaderFile;
use sapphire_render::wind::{VOLUMES_CONSTANT, WindBuffer};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

/// The largest difference allowed between the two samplers, in meters per second.
const TOLERANCE: f32 = 1e-5;

/// A compute pass that samples the air at each point for this frame's half of the buffer and the
/// last drawn frame's, at `WIND_VOLUMES` = `cap`.
struct Sampler {
    gpu: GpuContext,
    pipeline: wgpu::ComputePipeline,
    uniforms: wgpu::Buffer,
    points: wgpu::Buffer,
    results: wgpu::Buffer,
    read: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl Sampler {
    fn new(cap: u32, count: usize) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let device = gpu.device();
        // Points arrive through storage, so the compiler folds none of the maths under test.
        let source = format!(
            "{}
@group(0) @binding(0) var<uniform> wind: WindUniforms;
@group(0) @binding(1) var<storage, read> points: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> results: array<vec4<f32>>;
@compute @workgroup_size(1)
fn cs_sample(@builtin(global_invocation_id) id: vec3<u32>) {{
    let position = points[id.x].xyz;
    results[2u * id.x] = vec4<f32>(wind_air(wind.current, 0u, position), 0.0);
    results[2u * id.x + 1u] = vec4<f32>(wind_air(wind.previous, MAX_WIND_VOLUMES, position), 0.0);
}}
",
            ShaderFile::Wind.compiled_in()
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wind sampler"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let constants = [(VOLUMES_CONSTANT, f64::from(cap))];
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("wind sampler"),
            layout: None,
            module: &module,
            entry_point: Some("cs_sample"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            cache: None,
        });
        let buffer = |size: usize, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: size as u64,
                usage,
                mapped_at_creation: false,
            })
        };
        let point = size_of::<[f32; 4]>();
        let uniforms =
            buffer(size_of::<WindBuffer>(), wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let points = buffer(count * point, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let results = buffer(2 * count * point, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let read = buffer(2 * count * point, wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: points.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: results.as_entire_binding() },
            ],
        });
        Self { gpu, pipeline, uniforms, points, results, read, group }
    }

    /// The GPU's air at each of `points`: this frame's and the last drawn frame's, in pairs.
    fn sample(&self, buffer: &WindBuffer, points: &[Vec3]) -> Vec<[Vec3; 2]> {
        let (device, queue) = (self.gpu.device(), self.gpu.queue());
        let inputs: Vec<[f32; 4]> = points.iter().map(|point| point.extend(1.0).to_array()).collect();
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(buffer));
        queue.write_buffer(&self.points, 0, bytemuck::cast_slice(&inputs));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(points.len() as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.results, 0, &self.read, 0, self.read.size());
        queue.submit([encoder.finish()]);
        self.read.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let results: Vec<[f32; 4]> = {
            let mapped = self.read.get_mapped_range(..).expect("mapped");
            bytemuck::pod_collect_to_vec(&mapped[..])
        };
        self.read.unmap();
        let vector = |value: &[f32; 4]| Vec3::from_slice(&value[..3]);
        results.as_chunks::<2>().0.iter().map(|[now, before]| [vector(now), vector(before)]).collect()
    }
}

/// The layouts: the global wind alone, as every scene before the field drew; turbulence alone;
/// one of each kind of volume, overlapping; and every slot full of mixed kinds. Falloffs run from
/// 0.25 to 0.81: a sharper edge divides the shape's differences by its falloff, so near
/// `MIN_VOLUME_FALLOFF` the GPU and the CPU part by up to a thousand times more.
fn layouts() -> Vec<(&'static str, WindField)> {
    let breeze = Wind { direction: Vec3::new(1.0, 0.0, 0.4), strength: 1.5, gustiness: 0.5 };
    let mut layouts = vec![("global", WindField::new(breeze))];
    let mut turbulent = WindField::new(breeze);
    turbulent.turbulence = 0.6;
    layouts.push(("turbulence", turbulent));

    let mut kinds = turbulent;
    kinds.turbulence = 0.3;
    kinds.push(WindVolume::directional(Vec3::new(4.0, 1.0, -3.0), 9.0, Vec3::new(-2.0, 0.5, 1.0)));
    kinds.push(WindVolume::vortex(Vec3::new(-6.0, 0.0, 5.0), 7.0, Vec3::Y, 2.5, 0.4));
    kinds.push(WindVolume::burst(Vec3::new(1.0, 0.5, 2.0), 5.0, 3.0).with_falloff(0.6));
    layouts.push(("kinds", kinds));

    let mut full =
        WindField::new(Wind { direction: Vec3::new(-0.3, 0.0, 1.0), strength: 2.0, gustiness: 0.8 });
    full.turbulence = 1.0;
    for index in 0..MAX_WIND_VOLUMES {
        let angle = index as f32 * 0.9;
        let center = Vec3::new(12.0 * angle.cos(), 0.5 * index as f32, 12.0 * angle.sin());
        let radius = 4.0 + index as f32;
        let volume = match index % 3 {
            0 => WindVolume::directional(center, radius, Vec3::new(angle.sin(), 0.2, angle.cos()) * 2.0),
            1 => WindVolume::vortex(center, radius, Vec3::new(0.2, 1.0, -0.1), 1.0 + angle, 0.3),
            _ => WindVolume::burst(center, radius, 1.5 + 0.2 * angle),
        };
        assert!(full.push(volume.with_falloff(0.25 + 0.08 * index as f32)), "room for {index}");
    }
    layouts.push(("full", full));
    layouts
}

/// Points over the layouts' volumes and beyond them: a lattice from -40 m to 40 m, and a ring
/// about each volume, through its middle and over its edge.
fn points(layouts: &[(&str, WindField)]) -> Vec<Vec3> {
    let mut points = Vec::new();
    for x in -4..=4 {
        for z in -4..=4 {
            points.push(Vec3::new(10.0 * x as f32 + 0.37, 0.25 * (x + z) as f32, 10.0 * z as f32 - 0.61));
        }
    }
    for (_, field) in layouts {
        for volume in field.volumes() {
            for step in 0..12 {
                let angle = step as f32 * 0.55;
                let reach = volume.radius * (0.1 + 0.08 * step as f32);
                points.push(volume.center + Vec3::new(angle.cos(), 0.3 * angle.sin(), angle.sin()) * reach);
            }
            points.push(volume.center);
        }
    }
    points
}

#[test]
#[ignore = "needs a GPU"]
fn the_gpu_samples_the_air_as_the_cpu_does() {
    let layouts = layouts();
    let points = points(&layouts);
    let times = [0.0, 7.25, WIND_PERIOD - 0.05, WIND_PERIOD * 3.0 + 41.5];
    let mut compared = 0;
    let mut worst = 0.0f32;
    // Each tier's GPU cap: the desktops' 8, Mobile-High's 4 and Mobile-Low's 0.
    for cap in [8u32, 4, 0] {
        let sampler = Sampler::new(cap, points.len());
        for (name, field) in &layouts {
            for (index, &time) in times.iter().enumerate() {
                // The last drawn frame's half holds another layout's air, so a reader that took
                // the wrong half, or the wrong offset into the volumes, reads the wrong wind.
                let other = &layouts[(index + 1) % layouts.len()].1;
                let (current, previous) = (field.resolve(time), other.resolve(time - 0.016));
                let gpu = sampler.sample(&WindBuffer::new(&current, &previous), &points);
                let expected = |resolved: &ResolvedWind, point| resolved.air(point, cap as usize);
                for (&point, [now, before]) in points.iter().zip(gpu) {
                    for (half, (got, want)) in [
                        ("current", (now, expected(&current, point))),
                        ("previous", (before, expected(&previous, point))),
                    ] {
                        let error = (got - want).abs().max_element();
                        worst = worst.max(error);
                        assert!(
                            error <= TOLERANCE,
                            "{name} at {time} s, cap {cap}, {half} at {point}: GPU {got}, CPU {want}"
                        );
                        compared += 1;
                    }
                }
            }
        }
    }
    eprintln!("wind sampler: {compared} samples, worst difference {worst:e} m/s");
    assert!(compared > 1000, "{compared}");

    // Mobile-Low's GPU takes no volume: its air is the global wind's and turbulence's alone.
    let full = layouts.last().unwrap().1;
    let mut global = full;
    global.truncate(0);
    let sampler = Sampler::new(0, points.len());
    let (with, without) = (full.resolve(7.25), global.resolve(7.25));
    let gpu = sampler.sample(&WindBuffer::new(&with, &with), &points);
    for (&point, [now, _]) in points.iter().zip(gpu) {
        let want = without.air(point, MAX_WIND_VOLUMES);
        assert!((now - want).abs().max_element() <= TOLERANCE, "{point}: {now} against {want}");
    }
}
