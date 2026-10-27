//! Shader builds off the frame thread, rendered and read back: a request draws with the shaders
//! in use until its build is swapped in, then with the new ones, both owners' at once; a broken
//! one keeps them; a newer request supersedes the build in flight; pipelines built lazily during a
//! build are built again from the new sources; dropping the renderer stops its build. They need a
//! GPU: run them with `cargo test -p sapphire_render --lib -- --ignored shader_builds::`.
//!
//! Each test holds the worker at a gate before it builds, so frames are known to draw while the
//! build is in flight: a frame that waited on the build would wait for the gate, which gives up
//! after a while and lets the build land in that frame, which the test sees.

use std::sync::Arc;
use std::time::Duration;

use glam::Vec3;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::{Gate, ShaderOutcome, ShaderRequest};
use crate::graph::{FrameGraph, GraphExecutor};
use crate::{
    BloomParams, CameraData, ColorGrade, FramePacketRing, Inversion, PostEffects, PostParams, PostVariants,
    RenderSettings, Renderer, ShaderError, ShaderFile, ShaderSources, ShaderUpdate, SkyGradient, ToneCurve,
    ViewData, Vignette,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: u32 = 32;

/// Post that shows linear values below about 0.98 as they are.
fn passthrough() -> PostParams {
    PostParams {
        exposure: 0.0,
        bloom: BloomParams { intensity: 0.0, ..BloomParams::default() },
        tone_curve: ToneCurve {
            linear_start: 0.001,
            linear_length: 0.99,
            black_tightness: 1.0,
            ..ToneCurve::default()
        },
        grade_a: ColorGrade::NEUTRAL,
        grade_b: ColorGrade::NEUTRAL,
        grade_blend: 0.0,
        filter_strength: 1.0,
        vignette: Vignette { intensity: 0.0, ..Vignette::default() },
        ..PostParams::default()
    }
}

/// `sources` with `from` replaced by `to` in `file`, which holds it once.
fn edited(sources: &ShaderSources, file: ShaderFile, from: &str, to: &str) -> ShaderSources {
    let text = sources.get(file);
    assert_eq!(text.matches(from).count(), 1, "{file}: {from}");
    let mut edited = sources.clone();
    edited.set(file, text.replace(from, to));
    edited
}

/// Post with red and blue swapped.
fn swapped_post(sources: &ShaderSources) -> ShaderSources {
    let from = "return vec4<f32>(srgb_to_linear(display), 1.0);";
    edited(sources, ShaderFile::Post, from, "return vec4<f32>(srgb_to_linear(display.bgr), 1.0);")
}

/// A sky of one color, through post with red and blue swapped: two owners' programs.
fn flat_sky_swapped() -> ShaderSources {
    let from = "return vec4<f32>(fogged_sky(color, dir), 1.0);";
    let to = "return vec4<f32>(vec3<f32>(0.25, 0.5, 0.75) + 0.0 * fogged_sky(color, dir), 1.0);";
    swapped_post(&edited(&ShaderSources::compiled_in(), ShaderFile::Sky, from, to))
}

/// A renderer drawing sky-only frames into a readable target.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
}

impl Harness {
    fn new() -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { gpu, renderer, graph: GraphExecutor::new(), packets: FramePacketRing::new(), target }
    }

    /// Holds the renderer's builds before they build, until the gate returned is opened.
    fn hold(&mut self) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        self.renderer.hold_shader_builds(gate.clone());
        gate
    }

    /// Draws one frame of a three-colored sky, through `post`, and reads it back.
    fn render_with(&mut self, post: PostParams) -> Vec<u8> {
        let packet = self.packets.begin();
        let camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1.2, 1.0, 0.05);
        packet.views.push(ViewData::new(camera));
        packet.sky = SkyGradient {
            zenith: Vec3::new(0.05, 0.2, 0.7),
            horizon: Vec3::new(0.6, 0.3, 0.1),
            ground: Vec3::new(0.1, 0.4, 0.1),
        };
        packet.post = post;
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "no validation errors");
        read_texture(&self.gpu, &self.target)
    }

    fn render(&mut self) -> Vec<u8> {
        self.render_with(passthrough())
    }

    /// The answers given since the last call.
    fn outcomes(&mut self) -> Vec<ShaderOutcome> {
        self.renderer.shader_outcomes().collect()
    }

    /// Draws frames until a request is answered, each either `old` or, from the frame that
    /// answers it on, `new`; returns the answer.
    fn until_answered(&mut self, old: &[u8], new: &[u8]) -> ShaderOutcome {
        for _ in 0..20_000 {
            let frame = self.render();
            let mut outcomes = self.outcomes();
            if let Some(outcome) = outcomes.pop() {
                assert!(outcomes.is_empty(), "one answer");
                assert!(frame == new, "the frame that swaps the build in draws with it");
                return outcome;
            }
            assert!(frame == old, "a frame before the swap draws with the shaders in use");
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("no answer");
    }
}

/// The target's sRGB bytes.
fn read_texture(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped.chunks_exact(padded as usize).flat_map(|row| row[..(width * 4) as usize].iter().copied()).collect()
}

/// The frames `sources` draw, set on this thread, and the renderer back on the compiled-in
/// shaders, drawing `before` again.
fn reference(harness: &mut Harness, sources: &ShaderSources, before: &[u8]) -> Vec<u8> {
    harness.renderer.set_shader_sources(sources).expect("a valid edit");
    let frame = harness.render();
    harness.renderer.set_shader_sources(&ShaderSources::compiled_in()).expect("compiled in");
    assert!(harness.render() == before, "back to the compiled-in shaders");
    frame
}

#[test]
#[ignore = "needs a GPU"]
fn a_request_draws_the_old_shaders_until_its_build_swaps_in() {
    let mut harness = Harness::new();
    let before = harness.render();
    let sources = flat_sky_swapped();
    let after = reference(&mut harness, &sources, &before);
    assert!(after != before);
    let rebuilds = harness.renderer.stats().shader_rebuilds;

    // While the worker is held, frames draw with the shaders in use: nothing waits on the build.
    let gate = harness.hold();
    let request = harness.renderer.request_shader_sources(sources.clone());
    assert_eq!(harness.renderer.requested_shader_sources(), &sources);
    for _ in 0..3 {
        assert!(harness.render() == before, "drawn with the shaders in use");
        let stats = harness.renderer.stats();
        assert!(stats.shader_builds.building);
        assert_eq!(stats.shader_rebuilds, rebuilds);
        assert!(harness.outcomes().is_empty());
    }
    assert!(harness.renderer.shader_sources().is_compiled_in());

    // Once built, the frame that prepares it draws with the new sky and post at once.
    gate.open();
    let outcome = harness.until_answered(&before, &after);
    assert_eq!(outcome.request, request);
    // The sky's six (four layer variants and the cube's two) and post's eight.
    assert_eq!(outcome.result.expect("built"), ShaderUpdate { programs: 2, pipelines: 14 });
    assert!(outcome.frames > 3, "{} frames drew while it built", outcome.frames);
    assert!(harness.render() == after);
    assert_eq!(harness.renderer.shader_sources(), &sources);
    let stats = harness.renderer.stats();
    assert_eq!(stats.shader_rebuilds, rebuilds + 1);
    assert!(!stats.shader_builds.building);
    assert_eq!((stats.shader_builds.started, stats.shader_builds.restarted), (1, 0));

    // Sources equal to those in use build nothing, and are answered at once.
    let request = harness.renderer.request_shader_sources(sources);
    assert!(harness.render() == after);
    let outcome = harness.outcomes().pop().expect("answered");
    assert_eq!((outcome.request, outcome.result.expect("nothing")), (request, ShaderUpdate::default()));
    assert_eq!(harness.renderer.stats().shader_builds.started, 1);
}

#[test]
#[ignore = "needs a GPU"]
fn a_broken_request_keeps_the_shaders_in_use() {
    let mut harness = Harness::new();
    let before = harness.render();
    let compiled_in = ShaderSources::compiled_in();

    // A syntax error, found by naga on the worker and placed in its own file and line.
    let anchor = "fn fs_sky(in: SkyVaryings) -> @location(0) vec4<f32> {";
    let broken = edited(&compiled_in, ShaderFile::Sky, anchor, &format!("{anchor}\n    let broken = ;"));
    let request = harness.renderer.request_shader_sources(broken);
    let outcome = harness.until_answered(&before, &before);
    assert_eq!(outcome.request, request);
    let line = compiled_in.get(ShaderFile::Sky).lines().position(|line| line == anchor).expect("anchor") + 2;
    match outcome.result {
        Err(ShaderError::Invalid { file: ShaderFile::Sky, line: at, .. }) => assert_eq!(at as usize, line),
        other => panic!("{other:?}"),
    }

    // A renamed entry point, which naga accepts and wgpu refuses on the worker.
    let renamed = edited(&compiled_in, ShaderFile::Sky, "fn fs_sky(", "fn fs_sky_renamed(");
    harness.renderer.request_shader_sources(renamed);
    let outcome = harness.until_answered(&before, &before);
    assert!(matches!(outcome.result, Err(ShaderError::Pipeline { .. })), "{:?}", outcome.result);
    assert!(harness.render() == before, "the shaders in use draw on");
    assert!(harness.renderer.shader_sources().is_compiled_in());
    assert_eq!(harness.renderer.stats().shader_builds.failed, 2);

    // A good request after them lands.
    let sources = flat_sky_swapped();
    let after = reference(&mut harness, &sources, &before);
    harness.renderer.request_shader_sources(sources);
    assert!(harness.until_answered(&before, &after).result.is_ok());
}

#[test]
#[ignore = "needs a GPU"]
fn a_newer_request_supersedes_the_build_in_flight() {
    let mut harness = Harness::new();
    let before = harness.render();
    let sources = flat_sky_swapped();
    let after = reference(&mut harness, &sources, &before);

    // Post alone, then the sky too, built on the request before it: only the newer one lands.
    let gate = harness.hold();
    let post = swapped_post(&ShaderSources::compiled_in());
    harness.renderer.request_shader_sources(post);
    assert!(harness.render() == before);
    let base = harness.renderer.requested_shader_sources().clone();
    let from = "return vec4<f32>(fogged_sky(color, dir), 1.0);";
    let to = "return vec4<f32>(vec3<f32>(0.25, 0.5, 0.75) + 0.0 * fogged_sky(color, dir), 1.0);";
    let newer = edited(&base, ShaderFile::Sky, from, to);
    assert_eq!(newer, sources);
    let superseding = harness.renderer.request_shader_sources(newer.clone());
    // The same sources again join the build rather than start one.
    let joining = harness.renderer.request_shader_sources(newer);
    assert!(joining > superseding);
    assert!(harness.render() == before);
    gate.open();
    let outcome = harness.until_answered(&before, &after);
    assert_eq!(outcome.request, joining, "the answer covers every request");
    assert!(outcome.result.is_ok());
    let stats = harness.renderer.stats().shader_builds;
    assert_eq!((stats.started, stats.superseded, stats.restarted), (2, 1, 0));
}

#[test]
#[ignore = "needs a GPU"]
fn pipelines_built_during_a_build_are_built_again_from_its_sources() {
    let mut harness = Harness::new();
    let compiled_in = ShaderSources::compiled_in();
    let sources = swapped_post(&compiled_in);
    let inversion = Inversion { amount: 1.0, ..Inversion::NONE };
    let inverted = || PostParams { effects: PostEffects { inversion, ..PostEffects::NONE }, ..passthrough() };
    // The inversion's frame, from the edited post and from the compiled-in one.
    harness.renderer.set_shader_sources(&sources).expect("a valid edit");
    let edited_inversion = harness.render_with(inverted());
    harness.renderer.set_shader_sources(&compiled_in).expect("compiled in");
    let plain_inversion = harness.render_with(inverted());
    assert!(edited_inversion != plain_inversion);
    let mut harness = Harness::new();
    let before = harness.render();
    let after = reference(&mut harness, &sources, &before);

    // Every screen effect's variant prewarmed while the build runs, from the shaders in use: the
    // build lacks them, so it starts again, and lands with all sixteen of post's.
    let gate = harness.hold();
    let request = harness.renderer.request_shader_sources(sources);
    assert!(harness.render() == before);
    assert_eq!(harness.renderer.prewarm_post(PostVariants::ALL), 8);
    gate.open();
    let outcome = harness.until_answered(&before, &after);
    assert_eq!(outcome.request, request);
    assert_eq!(outcome.result.expect("built"), ShaderUpdate { programs: 1, pipelines: 16 });
    let stats = harness.renderer.stats();
    assert_eq!((stats.shader_builds.started, stats.shader_builds.restarted), (2, 1));
    // The inversion draws through the edited post without building anything on the frame.
    assert!(harness.render_with(inverted()) == edited_inversion);
    assert_eq!(harness.renderer.stats().post.first_use_builds, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn dropping_the_renderer_stops_its_build() {
    let mut harness = Harness::new();
    harness.render();
    let _gate = harness.hold();
    let _request: ShaderRequest = harness.renderer.request_shader_sources(flat_sky_swapped());
    harness.render();
    assert!(harness.renderer.stats().shader_builds.building);
    // Cancels the build and waits for its worker, which the drop lets past the gate, to stop.
    drop(harness);
}
