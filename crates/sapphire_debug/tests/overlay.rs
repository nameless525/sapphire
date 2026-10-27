//! The overlay as an app drives it, on a GPU. Run with
//! `cargo test -p sapphire_debug --test overlay -- --ignored`.

use sapphire_core::cvar::CvarRegistry;
use sapphire_debug::{DebugOverlay, OverlayConfig, OverlayFrame, WindowMetrics};
use sapphire_platform::{Event, FingerId, Timestamp, TouchDeviceId, TouchEvent, TouchPhase, WindowId};
use sapphire_render::graph::GraphExecutor;
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
            assets: None,
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
