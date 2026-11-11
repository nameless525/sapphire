//! A game drawn with no window (M3 G1): what the game's `--capture` and the sandbox's `--game` draw. The show
//! stands on a headless GPU, the title chooses what it's told to, and each tick is a frame: stepped, its view
//! driven and drawn, as the window's loop would, small until [`GameCapture::draw_from`]. The last frame is read
//! back. With [`GameCapture::boot`] it boots as the game's window does first (`crate::boot`), its boot screen
//! drawn at the tier's pace until the title's warm, and logs what that took.

use std::sync::Arc;
use std::time::Duration;

use game_combat::combat::CombatOption;
use game_combat::fight::fight_stage;
use game_ui::hud::OffscreenDisplay;
use sapphire_platform::{
    AppInitError, Event, KeyEvent, Keycode, Modifiers, Platform, Scancode, Timestamp, WindowId,
};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Content, Readback, Stage, StageDesc, headless_gpu};
use sapphire_time::TICK_HZ;

use crate::LOG_TARGET;
use crate::boot::{Boot, BootReport};
use crate::flow::{FlowCommand, FlowState};
use crate::show::GameShow;

/// The capture's target, sRGB like a window's swapchain.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// How many times smaller each side of the frames before [`GameCapture::draw_from`] is.
const SMALL: u32 = 10;

/// What a capture draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameCapture {
    /// What the title's chosen at once, as its row would: a new game or a continue. `None` stays on it.
    pub choose: Option<FlowCommand>,
    /// The frame's size in pixels.
    pub size: (u32, u32),
    /// Frames drawn, a tick each.
    pub ticks: u32,
    /// The player's quality settings: the defaults, so the device's tier, for the game's own; a golden pins
    /// its tier, since a device chooses its own (CI's Metal runner a lower one than a Mac's).
    pub quality: QualitySettings,
    /// Whether the keyboard holds the stick forward from the first frame until a timeline plays: a continue
    /// facing a gate walks through it into its cutscene, as a player would (M3 Q4's subtitles).
    pub forward: bool,
    /// The first frame drawn at [`size`](Self::size); 0 or 1 draws them all so. A frame before it is drawn
    /// whole, but into a target a tenth as wide and high, so a golden on a software rasterizer, which pays by
    /// the pixel, pays for few. Skipping those frames (`Stage::skip_frame`) wouldn't keep the picture: what
    /// the renderer carries from one drawn frame to the next would then have run fewer times, the ambient
    /// relight's quarters and the sky cube's faces a frame, the shadows, and the jitter's and the proximity
    /// fade's turns. Drawn small, all of that runs every frame as it would, and the size changing drops only
    /// TAA's history, as a camera cut does. So a capture draws full size from its last cut or a few frames
    /// before, and its last frame is byte for byte the one drawing them all would give.
    pub draw_from: u32,
    /// Whether it boots as the game's window does (M4 H2), its boot screen's frames paced at the tier's rate
    /// while the pipelines build off them, and says how it went ([`BootReport`]); else it stands the game at
    /// once and prewarms, as a golden needs nothing more.
    pub boot: bool,
    /// The screen the frames stand for: a phone's pixels per point and touch controls, or a desktop's
    /// (M4 S5's store screenshots). The default draws as captures always have.
    pub display: OffscreenDisplay,
}

/// A capture's last frame.
#[derive(Clone, Debug)]
pub struct Captured {
    /// Tightly packed RGBA8 rows.
    pub pixels: Vec<u8>,
    /// The GPU that drew it, for a golden to compare against its own backend's.
    pub adapter: wgpu::AdapterInfo,
    /// What its boot took, if it booted.
    pub boot: Option<BootReport>,
}

/// Stands `show` with `first`, its first run, draws [`GameCapture::ticks`] frames and returns the last one,
/// unless the GPU reported an error. The run's file, if it keeps one, is written and the save's writes waited
/// for.
pub fn capture(
    platform: &Platform,
    mut show: GameShow,
    first: &CombatOption,
    capture: &GameCapture,
) -> Result<Captured, AppInitError> {
    show.stand_for(capture.display);
    let gpu = headless_gpu()?;
    let (mut stage, boot) = if capture.boot {
        let boot = Boot::start(platform.host(), Arc::new(gpu), FORMAT, capture.quality, show, first)?;
        let (stage, report) = boot.run_offscreen(capture.size, true)?;
        (stage, Some(report))
    } else {
        let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, capture.quality));
        let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
        let combat = show.shown.fight.prepare_stock_combat(first, &mut content)?;
        let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
        let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
        show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
        (stage, None)
    };
    if let Some(command) = capture.choose {
        stage.show.choose(command);
    }

    let (width, height) = capture.size;
    let target = stage.target(width, height, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let small = stage.target(
        width.div_ceil(SMALL).max(1),
        height.div_ceil(SMALL).max(1),
        wgpu::TextureUsages::empty(),
    );
    let small = small.create_view(&wgpu::TextureViewDescriptor::default());
    let frame = Duration::from_secs(1) / TICK_HZ;
    let mut now = Duration::ZERO;
    // Whether it still walks, until a timeline plays, and whether forward's pressed yet.
    let (mut forward, mut held) = (capture.forward, false);
    for frame_at in 1..capture.ticks {
        now += frame;
        let at = Timestamp::from_nanos(now.as_nanos() as u64);
        stage.show.update_menus(at);
        let playing = stage.show.game().is_some_and(|game| game.flow().state() == FlowState::Playing);
        if forward && !held && playing {
            // Once the title's gone, so its rows don't take the press.
            stage.show.handle_event(&forward_key(now, true));
            held = true;
        }
        // What a window's `read_input` does after the menus and the press: the HUD follows the fight.
        stage.show.follow_input(at);
        if forward && held && stage.show.shown.fight.show.timeline_player().is_some() {
            stage.show.handle_event(&forward_key(now, false));
            forward = false;
            // Where a golden pins its frame from: the timeline's first.
            tracing::info!(target: LOG_TARGET, frame = frame_at, "walked into a timeline");
        }
        stage.step();
        stage.show.drive_view(frame);
        let into = if frame_at < capture.draw_from { &small } else { &view };
        stage.render(into, 1.0, |_, _| {})?;
        stage.show.say_first_uses(&stage.renderer);
    }
    now += frame;
    let at = Timestamp::from_nanos(now.as_nanos() as u64);
    stage.show.update_menus(at);
    stage.show.follow_input(at);
    if capture.ticks > 0 {
        stage.step();
        stage.show.drive_view(frame);
    }
    let readback = Readback::new(&stage.gpu, width, height);
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    stage.check_gpu()?;
    stage.show.say_first_uses(&stage.renderer);
    let pixels = readback.read(&stage.gpu).map_err(AppInitError::msg)?;
    if let Some(boot) = &boot {
        let pipelines = stage.renderer.stats().pipelines;
        tracing::info!(
            target: LOG_TARGET,
            ticks = capture.ticks,
            frame_thread = pipelines.frame_thread_builds - boot.pipelines.frame_thread_builds,
            frame_waits = pipelines.frame_waits - boot.pipelines.frame_waits,
            "the title and the game's frames after the boot"
        );
    }
    stage.show.finish().map_err(AppInitError::msg)?;
    tracing::info!(target: LOG_TARGET, ticks = capture.ticks, "a game drawn with no window");
    Ok(Captured { pixels, adapter: stage.gpu.caps().adapter.clone(), boot })
}

/// The keyboard's forward, down or up, at `now`, as a window would send it.
fn forward_key(now: Duration, down: bool) -> Event {
    Event::Key(KeyEvent {
        timestamp: Timestamp::from_nanos(now.as_nanos() as u64),
        window: WindowId::from_raw(1),
        scancode: Scancode::W,
        keycode: Keycode(0),
        modifiers: Modifiers::default(),
        down,
        repeat: false,
    })
}
