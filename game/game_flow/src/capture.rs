//! A game drawn with no window (M3 G1): what the game's `--capture` and the sandbox's `--game` draw. The show
//! stands on a headless GPU, the title chooses what it's told to, and each tick is a frame: stepped, its view
//! driven and drawn, as the window's loop would. The last frame is read back.

use std::time::Duration;

use game_combat::combat::CombatOption;
use game_combat::fight::fight_stage;
use sapphire_platform::{AppInitError, Platform, Timestamp};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::wgpu;
use sapphire_stage::{Content, Readback, Stage, StageDesc, headless_gpu};
use sapphire_time::TICK_HZ;

use crate::LOG_TARGET;
use crate::flow::FlowCommand;
use crate::show::GameShow;

/// The capture's target, sRGB like a window's swapchain.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// What a capture draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameCapture {
    /// What the title's chosen at once, as its row would: a new game or a continue. `None` stays on it.
    pub choose: Option<FlowCommand>,
    /// The frame's size in pixels.
    pub size: (u32, u32),
    /// Frames drawn, a tick each.
    pub ticks: u32,
}

/// A capture's last frame.
#[derive(Clone, Debug)]
pub struct Captured {
    /// Tightly packed RGBA8 rows.
    pub pixels: Vec<u8>,
    /// The GPU that drew it, for a golden to compare against its own backend's.
    pub adapter: wgpu::AdapterInfo,
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
    let gpu = headless_gpu()?;
    let desc = fight_stage(StageDesc::new(platform.host(), &gpu, FORMAT, QualitySettings::default()));
    let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
    let combat = show.shown.fight.prepare_stock_combat(first, &mut content)?;
    let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
    let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
    show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
    if let Some(command) = capture.choose {
        show.choose(command);
    }

    let (width, height) = capture.size;
    let target = stage.target(width, height, wgpu::TextureUsages::COPY_SRC);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let frame = Duration::from_secs(1) / TICK_HZ;
    let mut now = Duration::ZERO;
    for _ in 1..capture.ticks {
        now += frame;
        stage.show.update_menus(Timestamp::from_nanos(now.as_nanos() as u64));
        stage.step();
        stage.show.drive_view(frame);
        stage.render(&view, 1.0, |_, _| {})?;
        stage.show.say_first_uses(&stage.renderer);
    }
    now += frame;
    stage.show.update_menus(Timestamp::from_nanos(now.as_nanos() as u64));
    if capture.ticks > 0 {
        stage.step();
        stage.show.drive_view(frame);
    }
    let readback = Readback::new(&stage.gpu, width, height);
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    stage.check_gpu()?;
    stage.show.say_first_uses(&stage.renderer);
    let pixels = readback.read(&stage.gpu).map_err(AppInitError::msg)?;
    stage.show.finish().map_err(AppInitError::msg)?;
    tracing::info!(target: LOG_TARGET, ticks = capture.ticks, "a game drawn with no window");
    Ok(Captured { pixels, adapter: stage.gpu.caps().adapter.clone() })
}
