//! The look-dev stage, stood (M6 T15a): the one path that makes a look-dev [`Stage`]. [`LookDevDesc`] says what it
//! is, a shot, a hour and what its town runs on, over the stage a plain `--look-dev` capture stands on;
//! [`stand_with`] stands it, and [`stand`] stands it with nothing added. The sandbox's windowed app (`--look-dev`
//! with no `--capture`) still sets up its own look-dev scene, not a `Stage`.
//!
//! - **Who stands it:** the sandbox's `--look-dev` builds a description from its flags and stands it through
//!   [`stand_with`], its other flags applied at the points [`LookDevSteps`] leaves, where they applied before the
//!   stand was one; the editor's look pane stands it through [`stand`], and hands a live edit to [`show`].
//! - **What it holds:** the town and its far mountains, plants, lamps and fog cards around the character, held as its
//!   playable kind stands, the town's zone open around it (its cells, streamed as the stage's tier streams them),
//!   running on the description's look ([`Scene::set_town_look`]) at its hour, standing still there, with every
//!   texture whole, as a capture's are. What the look draws is declared before anything is built
//!   ([`Scene::declared_pipelines`]): a day that goes wet builds its wet surfaces with the rest, never on a frame.
//! - **Its capture** ([`capture`]) draws what `sapphire_sandbox --capture <png> --capture-ticks 0 --capture-frames
//!   <n>` draws of it: a test holds the sandbox's to it, and the editor's pane to the same.

use std::sync::Arc;

use sapphire_assets::AssetId;
use sapphire_platform::{AppInitError, Host};
use sapphire_render::PipelineInventory;
use sapphire_render::quality::{QualitySettings, TierId};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_world::streaming::StreamSettings;

use super::{SKY, Shot, hold};
use crate::scene::{LookDev, Scene, TownLook, stream_settings};
use crate::{
    CaptureTextures, Content, FrameHooks, PlayableLook, Readback, SceneShow, Stage, StageDesc, gpu_failure,
};

/// What a look-dev stage draws into: an sRGB target, as a window's surface and the sandbox's offscreen targets are.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// What a look-dev stage is: the description [`stand_with`] stands it by.
pub struct LookDevDesc {
    pub shot: Shot,
    /// The hour it stands still at, hours since midnight.
    pub hour: f32,
    /// What its town runs on in place of its own day and light rigs, or `None` for its own (`content/lookdev/`'s).
    pub look: Option<TownLook>,
    /// Whether the town's fog cards stand in it: frames that draw no fog (the sandbox's `--fog off`) have none.
    pub fog_cards: bool,
    /// The stage it stands on: the tier's quality and render settings, the character as the playable kind it
    /// stands as, the town's sky ([`SKY`]), a copy of the character's mesh kept, which its held pose is derived
    /// from, and what its load builds counted as prewarmed.
    pub stage: StageDesc,
}

impl LookDevDesc {
    /// `shot` on `tier` at `hour`, the town running on `look`, as a plain `sapphire_sandbox --look-dev <shot> --tier
    /// <tier> --time-of-day <hour> --capture` stands it on `gpu`: the tier's own quality and render settings, with
    /// nothing turned down, `character` standing as its playable kind under the town's sky, and its fog cards.
    pub fn new(
        host: &Host,
        gpu: &GpuContext,
        (shot, tier, hour): (Shot, TierId, f32),
        look: TownLook,
        character: AssetId,
    ) -> Self {
        let quality = QualitySettings { tier: Some(tier), ..QualitySettings::default() };
        let mut stage = StageDesc::new(host, gpu, FORMAT, quality);
        stage.character = character;
        stage.sky = SKY;
        stage.keep_character_mesh = true;
        stage.warm_load = true;
        Self { shot, hour, look: Some(look), fog_cards: true, stage }
    }
}

/// What a caller adds to a look-dev stage as [`stand_with`] stands it, at the points the stand leaves, in this
/// order: how the sandbox's other flags apply over the stand, each where it applied before the stand was one. Each
/// adds nothing unless a caller says otherwise; `()` adds nothing at all.
pub trait LookDevSteps<S: SceneShow> {
    /// The stage is made, its textures whole: before the town is placed.
    fn made(&mut self, stage: &mut Stage<S>) -> Result<(), AppInitError> {
        let _ = stage;
        Ok(())
    }

    /// The town stands around the character, held as its playable kind stands, running on the description's look:
    /// before the town's zone opens.
    fn held(&mut self, stage: &mut Stage<S>) -> Result<(), AppInitError> {
        let _ = stage;
        Ok(())
    }

    /// How the town's zone streams on `tier`, the stage's: as the tier's class of device does
    /// ([`stream_settings`]), as a plain capture's does, or `None` for no zone, so the town has no cells.
    fn stream(&mut self, tier: TierId) -> Option<StreamSettings> {
        Some(stream_settings(tier))
    }

    /// The town's zone is open around the character, held as `playable` stands it: before the hour.
    fn in_town(&mut self, stage: &mut Stage<S>, playable: &PlayableLook) -> Result<(), AppInitError> {
        let _ = (stage, playable);
        Ok(())
    }

    /// At the hour: before what the stage draws is declared and built.
    fn at_hour(&mut self, stage: &mut Stage<S>) -> Result<(), AppInitError> {
        let _ = stage;
        Ok(())
    }

    /// What the stage draws, declared, before it's built: a caller may take an entry out, to prove its miss fails.
    fn declared(&mut self, inventory: &mut PipelineInventory) {
        let _ = inventory;
    }
}

impl<S: SceneShow> LookDevSteps<S> for () {}

/// The look-dev stage `desc` describes on `gpu`, with `host`'s content and nothing playing on it: [`stand_with`]
/// with nothing added.
pub fn stand(host: &Host, gpu: Arc<GpuContext>, desc: LookDevDesc) -> Result<Stage, AppInitError> {
    let content = Content::new(host, desc.stage.character, desc.stage.sky)?;
    stand_with(gpu, desc, content, (), &mut ())
}

/// The look-dev stage `desc` describes on `gpu` with `content`, mounted for its character and sky, and `show`
/// playing on it, `steps` added where they come: the stage made, its textures whole; the town placed round the
/// character held in its pose, running on the description's look; the town's zone opened; the hour; then what it
/// draws, declared and built, so no frame builds any of it. It stands still at its hour, with no tick run: every
/// frame drawn after it, with nothing changed, draws what a capture's frame as far into its run does.
pub fn stand_with<S: SceneShow>(
    gpu: Arc<GpuContext>,
    desc: LookDevDesc,
    content: Content,
    show: S,
    steps: &mut impl LookDevSteps<S>,
) -> Result<Stage<S>, AppInitError> {
    let LookDevDesc { shot, hour, look, fog_cards, stage: desc } = desc;
    let kind = desc.character;
    let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
    stage.capture_textures = CaptureTextures::Whole;
    stage.content.set_texture_streaming(false);
    let tier = stage.quality.active().id;
    stage.scene.set_ambient_tier(tier);
    steps.made(&mut stage)?;
    let Stage { scene, content, renderer, gpu, .. } = &mut stage;
    let playable = content.playable_look(kind).map_err(AppInitError::msg)?;
    let town = content.town(renderer, gpu)?;
    let fog_cards = fog_cards.then(|| content.fog_cards(renderer, gpu)).transpose()?;
    let (pose, figure) = hold(content, &playable, renderer, gpu)?;
    scene.spawn_look_dev(shot, LookDev { town, fog_cards, pose, figure });
    if let Some(look) = look {
        scene.set_town_look(look).map_err(AppInitError::msg)?;
    }
    steps.held(&mut stage)?;
    if let Some(settings) = steps.stream(tier) {
        let Stage { scene, content, renderer, gpu, .. } = &mut stage;
        scene.open_town_zone(content, settings, renderer, gpu)?;
    }
    steps.in_town(&mut stage, &playable)?;
    stage.scene.set_day(hour, None);
    steps.at_hour(&mut stage)?;
    let Stage { scene, renderer, show, .. } = &mut stage;
    let mut declared = scene.declared_pipelines(&**show);
    steps.declared(&mut declared);
    renderer.prewarm(&declared);
    scene.prepare_clock_query();
    Ok(stage)
}

/// Runs a stood look-dev stage's town on `look`, standing still at `hour`, as [`stand_with`] stands it: a tool's
/// live edit. Nothing is built or loaded, so what the new look draws that the old one didn't (a day gone wet) is the
/// caller's to declare ([`Scene::declared_pipelines`]) before a frame draws it. Fails, keeping the look it had, on a
/// day that isn't complete.
pub fn show(scene: &mut Scene, look: TownLook, hour: f32) -> Result<(), String> {
    scene.set_town_look(look)?;
    scene.set_day(hour, None);
    Ok(())
}

/// What a still capture of `stage` draws at `size`, as `sapphire_sandbox --capture <png> --capture-ticks 0
/// --capture-frames <frames>` draws the stage it stands: `frames - 1` frames while the history settles, then one
/// more, read back as tightly packed sRGB RGBA8 rows. Fails if the GPU reported an error, whatever the pixels are.
/// The sandbox's own loop (its `capture.rs`) is the same with no ticks; it also steps a fight between frames,
/// writes sequences and loses frames, so it isn't this one, and its `look_dev` test holds the two to agree.
pub fn capture<S: SceneShow, H: FrameHooks<S>>(
    stage: &mut Stage<S, H>,
    (width, height): (u32, u32),
    frames: u32,
) -> Result<Vec<u8>, AppInitError> {
    let settling = stage.target(width, height, wgpu::TextureUsages::empty());
    let view = settling.create_view(&wgpu::TextureViewDescriptor::default());
    for _ in 1..frames {
        stage.render_capture_warmup(&view, true)?;
        stage.scene.end_frame();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
    }
    let target = stage.target(width, height, wgpu::TextureUsages::COPY_SRC);
    let readback = Readback::new(&stage.gpu, width, height);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    stage.render(&view, 1.0, |graph, target| readback.record(graph, target))?;
    let pixels = readback.read(&stage.gpu).map_err(AppInitError::msg)?;
    gpu_failure(&stage.gpu).map_err(AppInitError::msg)?;
    Ok(pixels)
}
