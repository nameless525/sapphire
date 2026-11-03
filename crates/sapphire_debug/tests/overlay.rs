//! The overlay as an app drives it, on a GPU. Run with
//! `cargo test -p sapphire_debug --test overlay -- --ignored`.

use sapphire_core::cvar::CvarRegistry;
use sapphire_core::debug_draw::{Color, DebugLabel, LabelText};
use sapphire_debug::{DebugOverlay, OverlayConfig, OverlayFrame, WindowMetrics};
use sapphire_math::Vec3;
use sapphire_platform::{Event, FingerId, Timestamp, TouchDeviceId, TouchEvent, TouchPhase, WindowId};
use sapphire_render::CameraData;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};
use sapphire_time::ScaledClock;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// What the overlay's frames read, besides the window.
struct App {
    gpu: GpuContext,
    graph: GraphExecutor,
    cvars: CvarRegistry,
    clock: ScaledClock,
    /// Touches the overlay passed to the game, and the cancels among them.
    game_touches: usize,
    game_cancels: usize,
    /// Debug draw's labels, and the camera they are seen through.
    camera: Option<CameraData>,
    labels: Vec<DebugLabel>,
}

impl App {
    fn new() -> Self {
        Self {
            gpu: gpu(),
            graph: GraphExecutor::new(),
            cvars: CvarRegistry::new(),
            clock: ScaledClock::new(),
            game_touches: 0,
            game_cancels: 0,
            camera: None,
            labels: Vec::new(),
        }
    }

    /// One frame at `ms`: `events` routed, the gesture polled, then the overlay updated, as the
    /// sandbox orders them.
    fn frame(&mut self, overlay: &mut DebugOverlay, ms: u64, window: WindowMetrics, events: &[Event]) {
        let (touches, cancels) = (&mut self.game_touches, &mut self.game_cancels);
        let mut game = |event: &Event| {
            if let Event::Touch(touch) = event {
                *touches += 1;
                *cancels += usize::from(touch.phase == TouchPhase::Canceled);
            }
        };
        for event in events {
            overlay.handle_event(event, &mut game);
        }
        let now = Timestamp::from_nanos(ms * 1_000_000);
        overlay.poll_gestures(now, &mut game);
        overlay.update(&OverlayFrame {
            camera: self.camera,
            debug_labels: &self.labels,
            now,
            window,
            allocations: Default::default(),
            render: Default::default(),
            quality: None,
            pacing: None,
            graph: &self.graph,
            gpu: &self.gpu,
            input_device: None,
            gamepads: 0,
            cvars: &self.cvars,
            world_clock: &self.clock,
            time_of_day: None,
            characters: &[],
            screen_effects: None,
            combat: None,
            assets: None,
            audio: None,
            streaming: None,
            hot_reload: None,
            live_link: None,
        });
    }
}

fn touch(ms: u64, finger: u64, phase: TouchPhase, x: f32) -> Event {
    Event::Touch(TouchEvent {
        timestamp: Timestamp::from_nanos(ms * 1_000_000),
        window: WindowId::from_raw(1),
        finger: FingerId::new(TouchDeviceId::from_raw(1), finger),
        phase,
        x,
        y: 0.5,
        pressure: 1.0,
    })
}

/// Two fingers down at `start` ms, held for 1.1 s in frames of 100 ms, moving apart by `spread`
/// of the window's width a frame each, then lifted.
fn two_fingers(app: &mut App, overlay: &mut DebugOverlay, start: u64, window: WindowMetrics, spread: f32) {
    let down = [touch(start, 1, TouchPhase::Down, 0.4), touch(start, 2, TouchPhase::Down, 0.6)];
    app.frame(overlay, start, window, &down);
    for step in 1..=11u8 {
        let (at, apart) = (start + 100 * u64::from(step), spread * f32::from(step));
        let moved =
            [touch(at, 1, TouchPhase::Moved, 0.4 - apart), touch(at, 2, TouchPhase::Moved, 0.6 + apart)];
        app.frame(overlay, at, window, &moved);
    }
    let end = start + 1200;
    let lifted = [touch(end, 1, TouchPhase::Up, 0.4), touch(end, 2, TouchPhase::Up, 0.6)];
    app.frame(overlay, end, window, &lifted);
}

#[test]
#[ignore = "needs a GPU"]
fn a_pinch_never_toggles_a_hidden_overlay() {
    // DT-002: hidden, the overlay never told the router the window's size, so a pinch's drift
    // measured at most √2 units and passed for two fingers holding still.
    let mut app = App::new();
    let mut overlay =
        DebugOverlay::new(&app.gpu, wgpu::TextureFormat::Rgba8UnormSrgb, OverlayConfig::default());
    let landscape = WindowMetrics::offscreen(1000, 500, 1.0);
    app.frame(&mut overlay, 0, landscape, &[]);
    // 0.03 of 1000 units a frame: 330 units each by the end.
    two_fingers(&mut app, &mut overlay, 100, landscape, 0.03);
    assert!(!overlay.is_visible(), "a pinch toggled the hidden overlay");
    assert_eq!((app.game_touches, app.game_cancels), (26, 0), "the game kept its fingers");

    // Two fingers holding still still toggle, and the game's fingers are cancelled.
    two_fingers(&mut app, &mut overlay, 2000, landscape, 0.0);
    assert!(overlay.is_visible(), "a still hold toggles");
    assert_eq!(app.game_cancels, 2);
    overlay.set_visible(false);

    // Turned while hidden: the router takes the new size from the hidden frames.
    let portrait = WindowMetrics::offscreen(500, 1000, 1.0);
    app.frame(&mut overlay, 4000, portrait, &[]);
    two_fingers(&mut app, &mut overlay, 4100, portrait, 0.03);
    assert!(!overlay.is_visible(), "a pinch toggled after turning");
    two_fingers(&mut app, &mut overlay, 6000, portrait, 0.0);
    assert!(overlay.is_visible());

    // Fingers down before an overlay's first frame, when it knows no size, time no hold at all.
    let mut fresh =
        DebugOverlay::new(&app.gpu, wgpu::TextureFormat::Rgba8UnormSrgb, OverlayConfig::default());
    two_fingers(&mut app, &mut fresh, 0, landscape, 0.0);
    assert!(!fresh.is_visible(), "a hold from before the first frame toggled");
}

/// RC-2: egui's renderer panics when it can't get a staging buffer, and that's all a lost device
/// gives: the overlay's frame is lost with the device, as the renderer's is.
#[test]
#[ignore = "needs a GPU"]
fn a_lost_device_loses_the_overlay_frame_alone() {
    let mut app = App::new();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut overlay = DebugOverlay::new(&app.gpu, format, OverlayConfig::default());
    overlay.set_visible(true);
    let window = WindowMetrics::offscreen(800, 600, 1.0);
    app.frame(&mut overlay, 0, window, &[]);
    app.frame(&mut overlay, 100, window, &[]);
    let texture = app.gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 800, height: 600, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    app.gpu.device().destroy();
    let _ = app.gpu.device().poll(wgpu::PollType::wait_indefinitely());
    assert!(app.gpu.is_lost());
    let mut graph = FrameGraph::new();
    let target = graph.import_texture("target", &view);
    overlay.record(&mut graph, target);
    app.graph.execute(&app.gpu, graph).expect("a valid graph, though nothing runs");
}

/// Debug draw's labels show with the panels hidden, as long as there are any: a hidden overlay with
/// none adds no pass.
#[test]
#[ignore = "needs a GPU"]
fn labels_show_with_the_panels_hidden() {
    let mut app = App::new();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut overlay = DebugOverlay::new(&app.gpu, format, OverlayConfig::default());
    app.graph.set_report(true);
    let window = WindowMetrics::offscreen(320, 240, 1.0);
    let texture = app.gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 320, height: 240, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    // Does the frame's graph hold the overlay's pass?
    let draws = |app: &mut App, overlay: &mut DebugOverlay, ms| {
        app.frame(overlay, ms, window, &[]);
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        overlay.record(&mut graph, target);
        app.graph.execute(&app.gpu, graph).expect("a valid graph");
        app.graph.report().expect("the report is on").passes.iter().any(|pass| pass.order.is_some())
    };
    assert!(!draws(&mut app, &mut overlay, 0), "hidden, with nothing to label");
    app.camera = Some(CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 4.0 / 3.0, 0.05));
    assert!(!draws(&mut app, &mut overlay, 100), "a camera, but no labels");
    let text = LabelText::format(format_args!("hit {}", 12));
    app.labels.push(DebugLabel { position: Vec3::ZERO, color: Color::WHITE, text });
    assert!(draws(&mut app, &mut overlay, 200), "labels draw with the panels hidden");
    assert!(!overlay.is_visible());
    app.labels.clear();
    assert!(!draws(&mut app, &mut overlay, 300), "and no pass stays behind once they are gone");
}

/// RC-2: a device lost while the graph records, as a shader build's worker can: the executor
/// checked before the overlay's pass, which checks for itself.
#[test]
#[ignore = "needs a GPU"]
fn a_device_lost_while_recording_loses_the_overlay_frame_alone() {
    let mut app = App::new();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut overlay = DebugOverlay::new(&app.gpu, format, OverlayConfig::default());
    overlay.set_visible(true);
    let window = WindowMetrics::offscreen(800, 600, 1.0);
    app.frame(&mut overlay, 0, window, &[]);
    app.frame(&mut overlay, 100, window, &[]);
    let texture = app.gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 800, height: 600, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut graph = FrameGraph::new();
    let target = graph.import_texture("target", &view);
    let device = app.gpu.device().clone();
    // A graph of under four passes records in order, on this thread.
    graph.add_pass("loses the device").write(target).record(move |_| {
        device.destroy();
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    });
    overlay.record(&mut graph, target);
    app.graph.execute(&app.gpu, graph).expect("a valid graph");
    assert!(app.gpu.is_lost());
}
