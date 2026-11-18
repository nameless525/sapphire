//! Stateless particles on the GPU and the CPU (M6 N8a): `procedural_particles.wgsl`'s `stateless_particle`
//! gives what `sapphire_look`'s `StatelessEmitter::particle` gives, for every source, facing and spawn, with
//! a delay, a stop, wind, one and two octaves of noise and a wrap, over many seeds, turns and ages, early and
//! past an endless emitter's wrap: whether a particle lives exactly, and its state within the tolerances below.
//! Run with `cargo test -p sapphire_render --test gpu procedural_particles:: -- --ignored --test-threads=1`.

use glam::{Quat, Vec3};
use sapphire_look::vfx::stateless::{
    StatelessEmitter, StatelessFacing, StatelessInstance, StatelessNoise, StatelessParams, StatelessParticle,
    StatelessSource, StatelessSpawn,
};
use sapphire_look::{VfxLight, VfxShape};
use sapphire_render::ShaderFile;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

/// The most a position may differ, in meters, but for the noise and its age's rounding ([`AGE_STEPS`]).
/// Positions here reach about 45 m, where an `f32`'s step is 3.8e-6 m, and each is a sum of a few terms, some
/// through `exp`, `pow`, `sin` and `cos`, which the GPU computes to within a few of its own steps rather than
/// to the bit, and through fused multiply-adds the CPU doesn't take. A tenth of a millimetre is far under a
/// pixel at any distance a particle is drawn from.
const POSITION: f32 = 1e-4;
/// The most a velocity may differ, in meters a second, by the same reasoning at speeds under 20 m/s.
const VELOCITY: f32 = 1e-4;
/// How many of its instance's age's `f32` steps a particle's own age may differ by, beyond the limits above:
/// it's the instance's age less its birth, `first + cycle * period`, whose product and sum the GPU may fuse
/// or round otherwise, so a few steps of the instance's age. A step is 1.9e-6 s at 31 s but 1.2e-4 s past
/// 1,024 s, where an endless emitter's age lives once it wraps (640 to 1,280 s here), and there a few steps
/// at 4 m/s are a few tenths of a millimetre. So each state's limit grows by this many steps of the age
/// times how fast that state moves, measured on the CPU ([`rates`]).
const AGE_STEPS: f32 = 4.0;
/// Either side of a sample, the span the CPU's rates are measured over, seconds: some sixteen steps of the
/// latest age's.
const RATE_SPAN: f32 = 2e-3;
/// What a position may differ by beyond [`POSITION`], as a share of the noise's reach (its strength times
/// its fall, measured as a level field's push): the push is the reach times sines of where the particle is,
/// in cells, so the few hundredths of a millimetre the particle's place may differ by, times 2pi times the
/// frequency (0.7 cells a metre), move the push by up to about 2e-4 of its reach.
const NOISE: f32 = 5e-4;
/// The most a size, color, opacity, life share or turn may differ: a curve is linear in the life share,
/// itself an age over a lifetime, both exact to an `f32`'s step, but for a wrap's fade, which divides a
/// position's difference by a tenth of the box's half (here 0.3 m at the least).
const SHADE: f32 = 5e-4;
/// The wrapped motes' box's half extents.
const WRAP: Vec3 = Vec3::new(8.0, 3.0, 8.0);
/// The age either side of a sample at which a particle that changes whether it lives, or is born again, is
/// on a boundary: there the two may round to either side, so only its life is compared there, not its state.
/// At least [`AGE_STEPS`] steps of the age, which at the late ages are more.
const BOUNDARY: f32 = 1e-4;

const SIZE: [(f32, f32); 4] = [(0.0, 0.02), (0.15, 0.3), (0.6, 0.2), (1.0, 0.0)];
const ALPHA: [(f32, f32); 3] = [(0.0, 0.0), (0.1, 1.0), (1.0, 0.0)];
const COLOR: [(f32, [f32; 3]); 5] = [
    (0.0, [4.0, 3.0, 2.0]),
    (0.2, [1.0, 0.6, 0.2]),
    (0.2, [0.9, 0.5, 0.1]),
    (0.7, [0.3, 0.1, 0.05]),
    (1.0, [0.0, 0.0, 0.0]),
];

/// The emitters: every source, facing and spawn, and the terms that pull on them; each with its twins without
/// noise and with a level noise (frequency and drift 0), whose difference is the noise's reach.
fn emitters() -> Vec<(&'static str, StatelessEmitter, [StatelessEmitter; 2])> {
    let base = StatelessParams {
        spawn: StatelessSpawn::Burst { count: 48 },
        delay: 0.0,
        lifetime: (0.8, 2.5),
        source: StatelessSource::Point,
        speed: (0.5, 4.0),
        direction: Vec3::new(0.2, 1.0, -0.1),
        spread: 40.0,
        gravity: 9.8,
        drag: 0.0,
        wind: 0.0,
        noise: None,
        facing: StatelessFacing::Camera { spin: (-3.0, 5.0) },
        size_scale: (0.6, 1.4),
        size: &SIZE,
        alpha: &ALPHA,
        color: &COLOR,
        wrap: None,
        shape: VfxShape::Star,
        light: VfxLight::Emissive,
        additive: 0.8,
        soft: 0.05,
    };
    let noise = |octaves| Some(StatelessNoise { strength: 3.0, frequency: 0.7, octaves, drift: 0.4 });
    let rate = StatelessSpawn::Rate { per_second: 30.0, duration: 0.0 };
    let all = [
        ("point burst", base),
        (
            "sphere with drag",
            StatelessParams { source: StatelessSource::Sphere { radius: 1.5 }, drag: 2.5, ..base },
        ),
        (
            "disc rate in the wind",
            StatelessParams { source: StatelessSource::Disc { radius: 2.0 }, spawn: rate, wind: 1.5, ..base },
        ),
        (
            "ring repeat, delayed",
            StatelessParams {
                source: StatelessSource::Ring { radius: 3.0, width: 0.5 },
                spawn: StatelessSpawn::Repeat { count: 12, interval: 0.6, times: 5 },
                delay: 0.35,
                facing: StatelessFacing::Velocity { stretch: 0.05 },
                ..base
            },
        ),
        (
            "cone, one octave",
            StatelessParams {
                source: StatelessSource::Cone { angle: 25.0, length: 1.2 },
                spawn: rate,
                noise: noise(1),
                drag: 0.3,
                ..base
            },
        ),
        (
            "box, two octaves, rising",
            StatelessParams {
                source: StatelessSource::Box { half: Vec3::new(2.0, 0.5, 1.0) },
                spawn: StatelessSpawn::Rate { per_second: 25.0, duration: 4.0 },
                noise: noise(2),
                gravity: -0.6,
                wind: 0.8,
                facing: StatelessFacing::Upright,
                ..base
            },
        ),
        (
            "line, timed rate",
            StatelessParams {
                source: StatelessSource::Line { length: 5.0 },
                spawn: StatelessSpawn::Rate { per_second: 18.0, duration: 2.5 },
                spread: 180.0,
                drag: 0.05,
                ..base
            },
        ),
        (
            "wrapped motes",
            StatelessParams {
                source: StatelessSource::Box { half: Vec3::new(8.0, 3.0, 8.0) },
                spawn: StatelessSpawn::Rate { per_second: 40.0, duration: 0.0 },
                wrap: Some(WRAP),
                speed: (0.0, 0.3),
                gravity: 0.0,
                wind: 0.5,
                noise: noise(2),
                ..base
            },
        ),
    ];
    all.iter()
        .map(|(name, params)| {
            let level = params.noise.map(|noise| StatelessNoise { frequency: 0.0, drift: 0.0, ..noise });
            let twin = |noise| StatelessEmitter::new(&StatelessParams { noise, ..*params });
            (*name, StatelessEmitter::new(params), [twin(None), twin(level)])
        })
        .collect()
}

/// Instances: turned and not, in wind and calm air, about the origin and away from it, some stopped.
fn instances() -> Vec<StatelessInstance> {
    let mut instances = Vec::new();
    for seed in 0..6_u32 {
        let angle = seed as f32 * 1.3;
        let position = Vec3::new(12.0 * angle.cos(), 0.5 * seed as f32, -9.0 * angle.sin());
        let rotation = if seed % 2 == 0 {
            Quat::IDENTITY
        } else {
            Quat::from_rotation_y(angle) * Quat::from_rotation_x(0.3 * angle)
        };
        let mut instance =
            StatelessInstance::new(position, rotation, 0.0, seed.wrapping_mul(2_654_435_761), 0)
                .with_wind(Vec3::new(3.0 * angle.sin(), 0.2, 2.0 * angle.cos()))
                .with_wrap_center(Vec3::new(-20.0 + 7.0 * seed as f32, 1.0, 15.0 - 4.0 * seed as f32));
        if seed == 3 {
            instance = instance.stopped_at(2.2);
        }
        instances.push(instance);
    }
    instances
}

/// The ages: at the start, through the first lives, and well into an endless emitter's cycles.
const AGES: [f32; 9] = [0.0, 0.05, 0.37, 0.9, 1.61, 2.48, 3.3, 7.77, 31.4];

/// The late ages, where the include runs once an endless emitter's age has wrapped: the wrapped motes' (whose
/// wrap is 640 s, as the cone's and the disc's are) just under twice its wrap, and an hour and a half wrapped.
fn late_ages(motes: &StatelessEmitter) -> [f32; 2] {
    let wrap = f64::from(motes.period()) * 256.0;
    [motes.wrapped_age(2.0 * wrap - 0.63, None).0, motes.wrapped_age(5_400.37, None).0]
}

/// A particle's age's step at `age`: one `f32` step of the instance's age.
fn age_step(age: f32) -> f32 {
    f32::from_bits(age.abs().to_bits() + 1) - age.abs()
}

/// How fast a living particle's position, velocity and look move at its instance's age, per second, measured
/// on the CPU over [`RATE_SPAN`] to either side in the same life, the faster side.
fn rates(emitter: &StatelessEmitter, instance: &StatelessInstance, index: u32, wraps: bool) -> [f32; 3] {
    let (age, now) = (instance.age(), emitter.particle(instance, index));
    let mut fastest = [0.0_f32; 3];
    for side in [(age - RATE_SPAN).max(0.0), age + RATE_SPAN] {
        let other = emitter.particle(&instance.at_age(side), index);
        // Dead there, or born again between: not this particle's life.
        if !other.alive || (other.life - now.life) * (side - age) < 0.0 || side == age {
            continue;
        }
        let span = (side - age).abs();
        let mut moved = other.position - now.position;
        if wraps {
            moved -= WRAP * 2.0 * (moved / (WRAP * 2.0)).round();
        }
        let shade = [
            (other.half_width - now.half_width).abs(),
            (other.color - now.color).abs().max_element(),
            (other.alpha - now.alpha).abs(),
            (other.life - now.life).abs(),
            turn_difference(other.rotation, now.rotation),
        ]
        .into_iter()
        .fold(0.0, f32::max);
        let rate = [moved.abs().max_element(), (other.velocity - now.velocity).abs().max_element(), shade];
        for (fastest, rate) in fastest.iter_mut().zip(rate) {
            *fastest = fastest.max(rate / span);
        }
    }
    fastest
}

/// A compute pass that evaluates particles: each query names an emitter, an instance and a particle.
struct Evaluator {
    gpu: GpuContext,
    pipeline: wgpu::ComputePipeline,
}

impl Evaluator {
    fn new() -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        // Everything arrives through storage, so the compiler folds none of the maths under test.
        let source = format!(
            "{}
@group(0) @binding(0) var<storage, read> emitters: array<StatelessEmitter>;
@group(0) @binding(1) var<storage, read> instances: array<StatelessInstance>;
@group(0) @binding(2) var<storage, read> queries: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> results: array<vec4<f32>>;
@compute @workgroup_size(64)
fn cs_particles(@builtin(global_invocation_id) id: vec3<u32>) {{
    if id.x >= arrayLength(&queries) {{
        return;
    }}
    let query = queries[id.x];
    let particle = stateless_particle(emitters[query.x], instances[query.y], query.z);
    results[4u * id.x] = vec4<f32>(particle.position, select(0.0, 1.0, particle.alive));
    results[4u * id.x + 1u] = vec4<f32>(particle.velocity, particle.half_width);
    results[4u * id.x + 2u] = vec4<f32>(particle.color, particle.alpha);
    results[4u * id.x + 3u] = vec4<f32>(particle.rotation, particle.life, 0.0, 0.0);
}}
",
            ShaderFile::ProceduralParticles.compiled_in()
        );
        let module = gpu.device().create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stateless particles"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = gpu.device().create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("stateless particles"),
            layout: None,
            module: &module,
            entry_point: Some("cs_particles"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { gpu, pipeline }
    }

    /// The GPU's particle for each query.
    fn evaluate(
        &self,
        emitters: &[StatelessEmitter],
        instances: &[StatelessInstance],
        queries: &[[u32; 4]],
    ) -> Vec<StatelessParticle> {
        let (device, queue) = (self.gpu.device(), self.gpu.queue());
        let storage = |label, bytes: &[u8]| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, bytes);
            buffer
        };
        let emitters = storage("emitters", bytemuck::cast_slice(emitters));
        let instances = storage("instances", bytemuck::cast_slice(instances));
        let query_buffer = storage("queries", bytemuck::cast_slice(queries));
        let size = (queries.len() * 4 * size_of::<[f32; 4]>()) as u64;
        let results = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("results"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: emitters.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: instances.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: query_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: results.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(queries.len().div_ceil(64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&results, 0, &read, 0, size);
        queue.submit([encoder.finish()]);
        read.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let values: Vec<[f32; 4]> = {
            let mapped = read.get_mapped_range(..).expect("mapped");
            bytemuck::pod_collect_to_vec(&mapped[..])
        };
        read.unmap();
        let vector = |value: [f32; 4]| Vec3::new(value[0], value[1], value[2]);
        values
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&[place, motion, shade, turn]| StatelessParticle {
                alive: place[3] > 0.5,
                position: vector(place),
                velocity: vector(motion),
                half_width: motion[3],
                rotation: turn[0],
                color: vector(shade),
                alpha: shade[3],
                life: turn[1],
            })
            .collect()
    }
}

/// The difference between two turns, the short way round.
fn turn_difference(a: f32, b: f32) -> f32 {
    let difference = (a - b).rem_euclid(std::f32::consts::TAU);
    difference.min(std::f32::consts::TAU - difference)
}

#[test]
#[ignore = "needs a GPU"]
fn the_gpu_evaluates_particles_as_the_cpu_does() {
    let named = emitters();
    let emitters: Vec<StatelessEmitter> = named.iter().map(|(_, emitter, _)| *emitter).collect();
    let mut instances = Vec::new();
    let mut queries = Vec::new();
    let motes = named.iter().find(|(name, _, _)| *name == "wrapped motes").expect("the motes").1;
    let late = late_ages(&motes);
    for base in self::instances() {
        for age in AGES.into_iter().chain(late) {
            let at = instances.len() as u32;
            instances.push(base.at_age(age));
            for (place, emitter) in emitters.iter().enumerate() {
                for index in 0..emitter.slots().min(96) {
                    queries.push([place as u32, at, index, 0]);
                }
            }
        }
    }
    let gpu = Evaluator::new().evaluate(&emitters, &instances, &queries);
    let (mut alive, mut boundary, mut edge) = (0, 0, 0);
    // Each emitter's worst difference over its limit, for position, velocity and shade, at the early ages and
    // at the late.
    let mut worst = vec![[[0.0_f32; 3]; 2]; named.len()];
    let mut failures = Vec::new();
    for (&[place, at, index, _], got) in queries.iter().zip(&gpu) {
        let (name, emitter, [calm, level]) = &named[place as usize];
        let instance = instances[at as usize];
        let want = emitter.particle(&instance, index);
        let age = instance.age();
        let steps = AGE_STEPS * age_step(age);
        let near = BOUNDARY.max(steps);
        let around =
            [age - near, age + near].map(|age| emitter.particle(&instance.at_age(age.max(0.0)), index));
        let settled =
            around.iter().all(|near| near.alive == want.alive && (near.life - want.life).abs() < 0.01);
        if !settled {
            boundary += 1;
            continue;
        }
        assert_eq!(got.alive, want.alive, "{name}, instance {at} at {age} s, particle {index}");
        if !want.alive {
            continue;
        }
        alive += 1;
        // A particle a hair from its wrap box's side may wrap to either side; its fade is nothing there.
        let wraps = *name == "wrapped motes";
        if wraps && (want.alpha < 1e-3 || got.alpha < 1e-3) {
            edge += 1;
            continue;
        }
        let mut reach = level.particle(&instance, index).position - calm.particle(&instance, index).position;
        if wraps {
            let span = WRAP * 2.0;
            reach -= span * (reach / span).round();
        }
        let [moving, turning, shading] = rates(emitter, &instance, index, wraps);
        let differences = [
            (got.position - want.position).abs().max_element()
                / (POSITION + NOISE * reach.length() + steps * moving),
            (got.velocity - want.velocity).abs().max_element() / (VELOCITY + steps * turning),
            [
                (got.half_width - want.half_width).abs(),
                (got.color - want.color).abs().max_element(),
                (got.alpha - want.alpha).abs(),
                (got.life - want.life).abs(),
                turn_difference(got.rotation, want.rotation),
            ]
            .into_iter()
            .fold(0.0, f32::max)
                / (SHADE + steps * shading),
        ];
        let when = usize::from(late.contains(&age));
        for (difference, worst) in differences.iter().zip(&mut worst[place as usize][when]) {
            *worst = worst.max(*difference);
        }
        if differences.iter().any(|&difference| difference > 1.0) && failures.len() < 8 {
            failures.push(format!(
                "{name}, instance {at} at {age} s, particle {index}: GPU {got:?}, CPU {want:?}"
            ));
        }
    }
    eprintln!(
        "stateless particles: {} queries, {alive} alive compared, {boundary} on a boundary, {edge} at a wrap's side",
        queries.len()
    );
    eprintln!("  late ages {late:?}");
    for ((name, _, _), worst) in named.iter().zip(&worst) {
        for (when, [position, velocity, shade]) in ["early", "late"].iter().zip(worst) {
            eprintln!(
                "  {name}, {when}: worst over its limit: position {position:.3}, velocity {velocity:.3}, shade {shade:.3}"
            );
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(alive > 5000, "{alive}");
    assert!(boundary * 100 < queries.len(), "{boundary} of {} on a boundary", queries.len());
}
