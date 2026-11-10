//! The warm-up off the frame, drawn through (M4 H1): frames draw while it builds and build nothing
//! themselves; the frame after it finishes takes it in; requests made meanwhile wait for it and are
//! answered in order; one built from shaders replaced meanwhile starts again; a blocking prewarm after
//! the first frame counts its wait as a hitch; dropping the renderer stops it. They need a GPU: run
//! them with `cargo test -p sapphire_render --lib -- --ignored warm_up::`.
//!
//! Each test that needs frames to draw while it builds holds the warm-up's task at a gate before it
//! builds: a frame that waited on it would wait for the gate, which gives up after a while.

use std::sync::Arc;
use std::time::Duration;

use glam::Vec3;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use crate::graph::{FrameGraph, GraphExecutor};
use crate::pipeline_builds::PipelineInventory;
use crate::shader_builds::Gate;
use crate::{
    CameraData, FramePacketRing, PostVariants, RenderSettings, Renderer, ShaderFile, ShaderSources,
    SkyGradient, ViewData, WarmUpRequest,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: u32 = 32;

/// A renderer drawing sky-only frames into a target.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
}

impl Harness {
    /// A renderer past its first frame: from here on a frame's build is a hitch.
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut harness =
            Self { gpu, renderer, graph: GraphExecutor::new(), packets: FramePacketRing::new(), target };
        harness.render();
        harness.render();
        assert_eq!(
            harness.renderer.stats().pipelines.hitches(),
            0,
            "{}",
            harness.renderer.first_use_report()
        );
        harness
    }

    /// Holds the renderer's warm-ups before they build, until the gate returned is opened.
    fn hold(&mut self) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        self.renderer.hold_warm_ups(gate.clone());
        gate
    }

    /// Draws one frame of sky.
    fn render(&mut self) {
        let packet = self.packets.begin();
        let camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1.2, 1.0, 0.05);
        packet.views.push(ViewData::new(camera));
        packet.sky = SkyGradient {
            zenith: Vec3::new(0.05, 0.2, 0.7),
            horizon: Vec3::new(0.6, 0.3, 0.1),
            ground: Vec3::new(0.1, 0.4, 0.1),
        };
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "no validation errors");
    }

    /// Draws frames until `request` is warm; says how many.
    fn until_warm(&mut self, request: WarmUpRequest) -> u32 {
        for frames in 0..20_000 {
            if self.renderer.is_warm(request) {
                return frames;
            }
            self.render();
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("never warm: {:?}", self.renderer.stats().warm_up);
    }
}

/// Everything but meshes and the sky's materials: every owner's step but the sky's.
fn inventory() -> PipelineInventory {
    PipelineInventory {
        post: PostVariants { layers: true, lens_effects: true, ..PostVariants::NONE },
        effects: true,
        skinning: true,
        ui: true,
        insets: true,
        probe_grid: true,
        fades: true,
        debug_draw: true,
        ..PipelineInventory::default()
    }
}

#[test]
#[ignore = "needs a GPU"]
fn frames_draw_while_the_warm_up_builds_and_build_nothing_themselves() {
    let mut harness = Harness::new();
    let at_start = harness.renderer.stats().pipelines;

    // While its task is held, frames go on and nothing is taken in.
    let gate = harness.hold();
    let request = harness.renderer.warm(&inventory());
    for _ in 0..3 {
        harness.render();
        assert!(!harness.renderer.is_warm(request));
        let progress = harness.renderer.warm_up();
        assert!(progress.warming && progress.steps > 0 && progress.steps_done == 0, "{progress:?}");
    }
    assert_eq!(harness.renderer.stats().pipelines, at_start, "nothing built while it's held");

    gate.open();
    let frames = harness.until_warm(request);
    assert!(frames > 0, "taken in by a frame, not by the request");
    let stats = harness.renderer.stats();
    let built = stats.pipelines.prewarmed - at_start.prewarmed;
    assert!(built > 10, "{built} built");
    assert_eq!(
        stats.pipelines.warmed_off_frame - at_start.warmed_off_frame,
        built,
        "each on the warm-up's threads"
    );
    assert_eq!(stats.pipelines.hitches(), 0, "{}", harness.renderer.first_use_report());
    assert_eq!(stats.pipelines.first_use_builds, at_start.first_use_builds);
    assert_eq!((stats.warm_up.started, stats.warm_up.restarted, stats.warm_up.failed), (1, 0, 0));
    let progress = harness.renderer.warm_up();
    assert!(!progress.warming && progress.fraction() == 1.0, "{progress:?}");

    // What it warmed is in use: frames and asking again build nothing.
    for _ in 0..3 {
        harness.render();
    }
    assert_eq!(harness.renderer.prewarm(&inventory()), 0);
    assert_eq!(harness.renderer.stats().pipelines, stats.pipelines);
    assert_eq!(harness.renderer.stats().warm_up.started, 1, "nothing to start");
}

#[test]
#[ignore = "needs a GPU"]
fn requests_made_while_one_builds_are_answered_in_order() {
    let mut harness = Harness::new();
    let gate = harness.hold();
    let effects = PipelineInventory { effects: true, ..PipelineInventory::default() };
    let first = harness.renderer.warm(&effects);
    let second = harness.renderer.warm(&PipelineInventory { ui: true, ..PipelineInventory::default() });
    let third = harness.renderer.warm(&PipelineInventory { skinning: true, ..effects });
    harness.render();
    assert!(!harness.renderer.is_warm(first));
    gate.open();
    harness.until_warm(third);
    assert!(harness.renderer.is_warm(first) && harness.renderer.is_warm(second));
    let stats = harness.renderer.stats();
    // The first alone, then the two that waited for it, together.
    assert_eq!((stats.warm_up.requests, stats.warm_up.started), (3, 2));
    assert_eq!(stats.pipelines.hitches(), 0, "{}", harness.renderer.first_use_report());
}

#[test]
#[ignore = "needs a GPU"]
fn a_warm_up_built_from_shaders_replaced_meanwhile_starts_again() {
    let mut harness = Harness::new();
    let gate = harness.hold();
    let request = harness.renderer.warm(&inventory());
    harness.render();

    // Sources set while it builds: what it builds from the old ones is stale.
    let mut edited = ShaderSources::compiled_in();
    let post = format!("{}\n// edited while a warm-up builds\n", edited.get(ShaderFile::Post));
    edited.set(ShaderFile::Post, post);
    harness.renderer.set_shader_sources(&edited).expect("a valid edit");
    let rebuilt = harness.renderer.stats().pipelines.rebuilt;
    gate.open();
    harness.until_warm(request);
    let stats = harness.renderer.stats();
    assert_eq!((stats.warm_up.started, stats.warm_up.restarted), (2, 1));
    assert_eq!(stats.pipelines.rebuilt, rebuilt, "the warm-up's builds aren't rebuilds");
    assert_eq!(stats.pipelines.hitches(), 0, "{}", harness.renderer.first_use_report());
    assert_eq!(harness.renderer.prewarm(&inventory()), 0, "built from the edited sources, and in use");
}

#[test]
#[ignore = "needs a GPU"]
fn a_blocking_prewarm_after_the_first_frame_is_a_hitch() {
    let mut harness = Harness::new();
    let built = harness.renderer.prewarm(&PipelineInventory { ui: true, ..PipelineInventory::default() });
    assert_eq!(built, 1);
    let pipelines = harness.renderer.stats().pipelines;
    assert_eq!((pipelines.frame_waits, pipelines.frame_thread_builds, pipelines.hitches()), (1, 0, 1));
    assert_eq!(pipelines.warmed_off_frame, 1, "built off the frame, though the frame waited");
}

#[test]
#[ignore = "needs a GPU"]
fn dropping_the_renderer_stops_its_warm_up() {
    let mut harness = Harness::new();
    let _gate = harness.hold();
    let request = harness.renderer.warm(&inventory());
    harness.render();
    assert!(!harness.renderer.is_warm(request));
    drop(harness);
}
