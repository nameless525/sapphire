//! Offscreen modes: `--capture`, `--bench`, `--contact-sheet` and, in `dev` builds, `--offscreen`.
//!
//! They render the sandbox scene into an offscreen target instead of a window. `main` runs them
//! through `sapphire_platform::run_with` with `RunOptions { background: true }`, so the process
//! never takes focus either. That makes the real app checkable on a desktop someone is using:
//! [`capture`](crate::capture) writes a PNG (the hook for golden-image tests, spec §9) and
//! [`bench`](crate::bench) measures the per-frame CPU path, then each exits; `--offscreen`
//! (`serve.rs`) keeps drawing for hot reload and live link until stopped.

use sapphire_platform::{App, AppControl, AppInitError, AppMetadata, Event, Platform};
use sapphire_render::RenderSettings;
use sapphire_render::debug_view::DebugView;
#[cfg(feature = "dev")]
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_rhi::wgpu;

use crate::LOG_TARGET;
use crate::app::SandboxApp;
use crate::options::{OffscreenMode, Options};
#[cfg(feature = "dev")]
use crate::stats::Stages;
use game_combat::combat::Combat;
use game_combat::fight::{Arena, Fight, FightShow};
use game_combat::party_looks::PartyLooks;
use game_ui::hud::ShownFight;
use sapphire_stage::scene::LookDev;
#[cfg(feature = "dev")]
use sapphire_stage::scene::Scene;
#[cfg(feature = "dev")]
use sapphire_stage::{FRAME, FrameHooks, LoadingFrame, ToolFrame};
use sapphire_stage::{StageDesc, headless_gpu};

/// Offscreen targets are sRGB like the swapchain, so they match what the window shows.
pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The GPU, renderer and scene an offscreen mode draws with, under the debug overlay in `dev` builds.
#[cfg(feature = "dev")]
pub(crate) type Stage = sapphire_stage::Stage<ShownFight, StageTools>;
/// The GPU, renderer and scene an offscreen mode draws with.
#[cfg(not(feature = "dev"))]
pub(crate) type Stage = sapphire_stage::Stage<ShownFight>;

/// The offscreen "app". A capture or bench does all its work in `init`, and the first frame
/// exits; `--offscreen` draws a frame each iteration until the process is told to quit.
pub struct OffscreenApp {
    #[cfg(feature = "dev")]
    serving: Option<crate::serve::Serving>,
}

impl App for OffscreenApp {
    const METADATA: AppMetadata = SandboxApp::METADATA;

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let (options, unknown) = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        options.refuse_unknown(&unknown).map_err(AppInitError::msg)?;
        if !unknown.is_empty() {
            tracing::warn!(target: LOG_TARGET, ?unknown, "ignoring unknown arguments");
        }
        let offscreen = options.offscreen.as_ref().ok_or_else(|| {
            AppInitError::msg("OffscreenApp needs --capture <png>, --bench <frames> or --offscreen")
        })?;
        let (width, height) = (offscreen.width, offscreen.height);
        // Before any GPU work, so a bad replay file costs nothing and says why.
        let combat = options.combat.as_ref().map(Combat::new).transpose().map_err(AppInitError::msg)?;
        if let (Some(_), Some(combat)) = (options.arena, &combat) {
            game_combat::arena::check_scenario(combat, options.party).map_err(AppInitError::msg)?;
        }
        match &offscreen.mode {
            OffscreenMode::Capture { path, ticks, frames } => {
                let ticks = match (&combat, &options.combat) {
                    (Some(combat), Some(option)) => {
                        combat.capture_ticks(option, *ticks, *frames).map_err(AppInitError::msg)?
                    }
                    _ => *ticks,
                };
                let mut stage = stage(platform, &options)?;
                stage.show.fight.combat = combat;
                crate::capture::run(&mut stage, path, ticks, *frames, width, height)?;
            }
            OffscreenMode::Bench { frames } => {
                crate::bench::run(platform, &options, combat, *frames, width, height)?;
            }
            OffscreenMode::ContactSheet { path, cells, ticks, yaws } => {
                let mut stage = stage(platform, &options)?;
                let framing = crate::contact_sheet::Framing::new(width, height, options.zoom);
                let layout = crate::contact_sheet::Layout { cells: *cells, ticks, yaws };
                crate::contact_sheet::run(&mut stage, path, &layout, framing)?;
            }
            #[cfg(feature = "dev")]
            OffscreenMode::Serve => {
                let mut stage = stage(platform, &options)?;
                stage.show.fight.combat = combat;
                if let Some(combat) = &mut stage.show.fight.combat {
                    stage.hooks.dev.start_fight_from_cooks(combat, &stage.content);
                }
                let serving = crate::serve::Serving::new(stage, options.clone(), width, height)?;
                return Ok(Self { serving: Some(serving) });
            }
            #[cfg(not(feature = "dev"))]
            OffscreenMode::Serve => {
                return Err(AppInitError::msg(
                    "--offscreen serves hot reload and live link, which only `dev` builds have (`--features dev`)",
                ));
            }
        }
        // Every golden scene passes through here: CI's runners have no sound, and a run that draws
        // has no business opening a device.
        sapphire_stage::audio::stayed_shut(sapphire_audio::device_opens()).map_err(AppInitError::msg)?;
        Ok(Self {
            #[cfg(feature = "dev")]
            serving: None,
        })
    }

    fn event(&mut self, _platform: &mut Platform, event: &Event) -> AppControl {
        match event {
            // Ctrl-C or `kill`, which SDL reports as a quit. A recording gets written then.
            #[cfg(feature = "dev")]
            Event::Quit { .. } if self.serving.is_some() => {
                self.serving.as_mut().map_or(AppControl::Exit, crate::serve::Serving::finish)
            }
            Event::Quit { .. } => AppControl::Exit,
            _ => AppControl::Continue,
        }
    }

    fn iterate(&mut self, _platform: &mut Platform) -> AppControl {
        #[cfg(feature = "dev")]
        if let Some(serving) = &mut self.serving {
            return serving.frame();
        }
        AppControl::Exit
    }
}

/// The stage `options` ask for. Offscreen modes run on desktops only.
pub(crate) fn stage(platform: &Platform, options: &Options) -> Result<Stage, AppInitError> {
    let (instance, gpu) = headless_gpu()?;
    if let Some(offscreen) = &options.offscreen {
        let limits = gpu.device().limits();
        target_fits(offscreen.width, offscreen.height, limits.max_texture_dimension_2d)
            .map_err(AppInitError::msg)?;
        if matches!(offscreen.mode, OffscreenMode::Capture { .. } | OffscreenMode::ContactSheet { .. }) {
            crate::capture::readback_fits(offscreen.width, offscreen.height, limits.max_buffer_size)
                .map_err(AppInitError::msg)?;
        }
    }
    let mut desc = StageDesc::new(platform, &gpu, FORMAT, options.quality_settings());
    desc.settings = options.render_settings(desc.quality.render_settings());
    desc.character = options.character();
    desc.sky = options.sky();
    desc.keep_character_mesh = options.keeps_character_mesh();
    if options.party {
        desc.uploads = Some(Box::<PartyLooks>::default());
    }
    desc.material_overrides = options.material_overrides();
    desc.skinned = options.skinned;
    desc.prewarm_packs =
        options.offscreen.as_ref().is_some_and(|run| matches!(run.mode, OffscreenMode::Bench { .. }));
    desc.inventory_without = options.inventory_without.clone();
    let history_only_capture = history_only_capture_allowed(options, desc.settings);
    let mut stage =
        sapphire_stage::Stage::new(platform, instance, gpu, desc)?.with_show(ShownFight::default());
    stage.history_only_capture = history_only_capture;
    stage.graph.set_serial(options.serial_passes);
    let character = stage.character;
    let sapphire_stage::Stage { scene, content, renderer, gpu, quality, show, .. } = &mut stage;
    let ShownFight { fight: Fight { show: fight, .. }, hud: shown_hud } = &mut *show;
    if let Some(rig) = &options.character_rig {
        scene.set_character_rig(rig).map_err(AppInitError::msg)?;
    }
    if options.lights {
        scene.spawn_demo_lights();
    }
    if options.skinned {
        scene.set_stalk_mesh(sapphire_stage::skinned::upload_stalk(renderer)?);
    }
    if options.swatches {
        scene.spawn_swatches(sapphire_stage::swatches::upload_swatches(content, gpu, renderer)?);
    }
    if options.materials {
        scene.spawn_layer_samples(sapphire_stage::layers::upload_samples(content, gpu, renderer)?);
    }
    if options.two_sided {
        scene.spawn_garments(sapphire_stage::garments::upload_samples(renderer)?);
    }
    if let Some(layout) = options.faces {
        scene.spawn_face_rig(content.face_head(renderer, gpu)?, layout);
    }
    if let Some(layout) = options.rim_rig {
        scene.spawn_rim_rig(layout, content.rim_rig_meshes(renderer)?);
    }
    if options.environment {
        scene.spawn_environment(content.courtyard(renderer, gpu)?);
        if !options.fog_off {
            scene.spawn_fog_cards(content.fog_cards(renderer, gpu)?);
        }
    }
    if let Some(view) = options.foliage {
        scene.spawn_foliage(view, content.foliage(renderer, gpu)?);
    }
    if options.probes {
        let arcade = content.probes(renderer, gpu)?;
        scene.spawn_arcade(arcade, content.probe_hours());
    }
    if options.outline_row {
        scene.spawn_outline_row(character);
    }
    if options.hard_edges {
        scene.spawn_hard_edges(content.hard_edges(renderer, gpu)?);
    }
    if let Some(shot) = options.look_dev {
        let town = content.town(renderer, gpu)?;
        let fog_cards = (!options.fog_off).then(|| content.fog_cards(renderer, gpu)).transpose()?;
        let (pose, figure) = sapphire_stage::lookdev::hold(content, renderer, gpu)?;
        scene.spawn_look_dev(shot, LookDev { town, fog_cards, pose, figure });
        if let Some(clip) = options.clip {
            content.load_clip(clip.id(), renderer, gpu)?;
            let mut player = sapphire_stage::lookdev::player(content)
                .map_err(AppInitError::msg)?
                .with_root_motion(!options.clip_in_place)
                .with_springs_on(options.springs != Some(false));
            if let Some(ik) = &options.ik {
                let block = sapphire_stage::studio::upload_block(renderer)?;
                let solver = scene.start_ik(ik, block, content, player.pose()).map_err(AppInitError::msg)?;
                player = player.with_ik(solver);
            }
            scene.play_clip(player);
            scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
        }
        if let Some(script) = options.graph {
            content.load_graph(sapphire_stage::lookdev::GRAPH, renderer, gpu)?;
            let mut driver = sapphire_stage::lookdev::graph_driver(content).map_err(AppInitError::msg)?;
            if let Some(ik) = &options.ik {
                let block = sapphire_stage::studio::upload_block(renderer)?;
                let solver =
                    scene.start_ik(ik, block, content, driver.player.pose()).map_err(AppInitError::msg)?;
                driver = driver.with_ik(solver);
            }
            scene.play_graph(
                driver.with_springs_on(options.springs != Some(false)),
                sapphire_stage::anim_graph::GraphShow::new(script),
            );
            scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
        }
        if shot == sapphire_stage::lookdev::Shot::Enemy {
            let meshes = fight.load_enemy(content, renderer, gpu)?;
            let held = fight.held_enemy(content)?;
            fight.spawn_enemy(scene, meshes, held);
        }
    }
    if let Some(shot) = options.arena {
        let town = content.town(renderer, gpu)?;
        let fog_cards = (!options.fog_off).then(|| content.fog_cards(renderer, gpu)).transpose()?;
        let (pose, figure) = sapphire_stage::lookdev::hold(content, renderer, gpu)?;
        let ground = fight.load_arena_ground(content, renderer, gpu)?;
        let world = ground.share();
        let look = LookDev { town, fog_cards, pose, figure };
        let enemy = fight.load_enemy(content, renderer, gpu)?;
        let held = fight.held_enemy(content)?;
        fight.spawn_arena(scene, Arena { shot, look, ground, enemy, held });
        if let Some(meshes) = FightShow::party_meshes(content, character) {
            fight.set_party_meshes(scene, meshes);
        }
        if let Some(clip) = options.clip {
            content.load_clip(clip.id(), renderer, gpu)?;
            let mut player = sapphire_stage::lookdev::player(content)
                .map_err(AppInitError::msg)?
                .with_root_motion(false)
                .with_springs_on(options.springs != Some(false));
            if let Some(ik) = &options.ik {
                let look = ik.look.map(|look| (look, 0.0));
                let solver =
                    scene.ik_on(world.clone(), look, content, player.pose()).map_err(AppInitError::msg)?;
                player = player.with_ik(solver);
            }
            scene.play_clip(player);
            scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
        }
        if options.combat.is_some() {
            content.load_graph(sapphire_stage::lookdev::GRAPH, renderer, gpu)?;
            fight.load_enemy_graph(content, renderer, gpu)?;
            let player = sapphire_stage::lookdev::graph_driver(content).map_err(AppInitError::msg)?;
            let solver =
                scene.ik_on(world.clone(), None, content, player.player.pose()).map_err(AppInitError::msg)?;
            let enemy = fight.enemy_driver(content, world)?;
            fight.play_fighters(
                scene,
                player.with_springs_on(options.springs != Some(false)).with_ik(solver),
                enemy,
            );
            scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
        }
    }
    if options.blade() {
        let sword = fight.load_sword(content, renderer, gpu)?;
        let hand = sapphire_stage::lookdev::held_hand(content).map_err(AppInitError::msg)?;
        fight.spawn_blade(scene, sword, hand);
    }
    // The day, the sky options, the starting view and the turntable's speed, last so `--view`
    // holds whatever framing the scene chose.
    options.apply_to(scene).map_err(AppInitError::msg)?;
    if let Some(show) = options.vfx {
        scene.play_vfx(show, sapphire_stage::vfx::budget(quality.active().id));
    }
    if options.cues {
        fight
            .play_cues(scene, sapphire_stage::vfx::budget(quality.active().id))
            .map_err(AppInitError::msg)?;
    }
    // The stage after the hour, which places its pillars.
    if let Some(layout) = options.stage {
        scene.spawn_studio(layout, content.studio_meshes(renderer)?);
    }
    // Offscreen there are no touches: the HUD is its fixed frame, in either mode, unless it's
    // live and a fight's bars stand behind it.
    if let Some(hud) = options.hud {
        let fighting = options.arena.is_some() && options.combat.is_some();
        let mode = if fighting { hud.mode } else { game_ui::hud::HudMode::Fixed };
        let hud = game_ui::hud::HudOptions { mode, ..hud };
        *shown_hud = Some(game_ui::hud::FightHud::new(hud));
    }
    // The screen effects it may push, `--post-effect`'s among them, the effects it plays, its sky and
    // its HUD show on their first frame.
    let mut declared = scene.declared_pipelines(&*show);
    if let Some(entry) = &options.inventory_without {
        declared.remove(entry);
    }
    renderer.prewarm(&declared);
    #[cfg(feature = "dev")]
    let tools = StageTools { dev: crate::dev::DevTools::new(gpu, FORMAT, options, platform, content)? };
    #[cfg(not(feature = "dev"))]
    let tools = ();
    #[cfg(not(feature = "dev"))]
    if options.debug_overlay.is_some() {
        tracing::warn!(target: LOG_TARGET, "--debug-overlay needs a `dev` build (`--features dev`)");
    }
    Ok(stage.with_hooks(tools))
}

/// The debug overlay and what it reads: drawn over the frame with `--debug-overlay`, hidden otherwise, with
/// hot reload and live link running before each frame's loading.
#[cfg(feature = "dev")]
pub(crate) struct StageTools {
    pub dev: crate::dev::DevTools,
}

#[cfg(feature = "dev")]
impl FrameHooks<ShownFight> for StageTools {
    const TOOLS: bool = true;

    fn before_fixed_step(&mut self, scene: &mut Scene) {
        self.dev.before_fixed_step(scene);
    }

    fn before_loading(&mut self, frame: LoadingFrame<'_, ShownFight>) {
        let LoadingFrame { content, renderer, gpu, scene, show } = frame;
        self.dev.reload(content, renderer, gpu, scene, &mut show.fight);
    }

    fn overlay_visible(&self) -> bool {
        self.dev.overlay_visible()
    }

    fn after_steps(&mut self, show: &ShownFight, scene: &Scene) {
        self.dev.after_steps(show.fight.combat.as_ref(), scene);
    }

    fn tools(&mut self, frame: ToolFrame<'_, ShownFight>, executor: &mut GraphExecutor) {
        let ToolFrame { width, height, last_times, renderer, quality, gpu_timing, gpu, scene, content, show } =
            frame;
        let stages = last_times.map(|times| Stages {
            extract: times.extract,
            render: times.render,
            ..Stages::default()
        });
        // Offscreen frames have no real length; the overlay is told each is a 60 Hz frame.
        // No display paces offscreen frames.
        self.dev.begin_frame(FRAME, stages.as_ref(), None);
        self.dev.advance_offscreen(FRAME);
        // Drawn as on a display 720 points high, so text stays legible at any size.
        let scale = (height as f32 / 720.0).max(1.0);
        let state = crate::dev::FrameState {
            window: sapphire_debug::WindowMetrics::offscreen(width, height, scale),
            render: renderer.stats(),
            quality,
            gpu_timing,
            gpu,
            input: None,
            scene,
            assets: content.stats(),
            audio: None,
            combat: show.fight.combat.as_ref(),
        };
        self.dev.update(state, executor);
    }

    fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId) {
        self.dev.record(graph, target);
    }
}

/// Whether a `--size` target fits this device. It has to be said here, before anything is drawn: past
/// `largest` wgpu only reports validation errors, which say less about why.
fn target_fits(width: u32, height: u32, largest: u32) -> Result<(), String> {
    if width.max(height) > largest {
        return Err(format!("--size {width}x{height}: this device's largest target is {largest} a side"));
    }
    Ok(())
}

/// TAA's persistent history gives the frame graph a root independent of the display target.
/// Dynamic resolution and debug modes observe work/timings beyond the final image, so retain
/// their complete frames. Use effective settings: `--aa` can override the tier.
fn history_only_capture_allowed(options: &Options, settings: RenderSettings) -> bool {
    settings.temporal
        && !options.dynamic_resolution
        && !options.serial_passes
        && options.debug_overlay.is_none()
        && options.debug_view.is_none_or(|view| view == DebugView::Off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Options {
        let args =
            std::iter::once("sandbox").chain(args.iter().copied()).map(str::to_owned).collect::<Vec<_>>();
        let (options, unknown) = Options::parse(&args).unwrap();
        assert!(unknown.is_empty(), "unexpected arguments: {unknown:?}");
        options
    }

    #[test]
    fn a_target_past_the_devices_largest_is_refused_by_size() {
        assert_eq!(target_fits(16384, 16384, 16384), Ok(()));
        let refused = target_fits(16385, 4, 16384).expect_err("past it");
        assert!(refused.contains("16385x4") && refused.contains("16384"), "{refused}");
        assert!(target_fits(4, 16385, 16384).is_err());
    }

    #[test]
    fn history_only_requires_effective_taa_without_instrumentation() {
        let allowed = |args: &[&str], tier| {
            let options = options(args);
            history_only_capture_allowed(&options, options.render_settings(tier))
        };
        let taa = RenderSettings { temporal: true, ..RenderSettings::default() };
        let no_taa = RenderSettings { temporal: false, ..taa };
        assert!(allowed(&[], taa));
        assert!(!allowed(&[], no_taa));
        assert!(allowed(&["--aa", "taa"], no_taa), "explicit AA overrides the tier");
        assert!(!allowed(&["--aa", "msaa"], taa));
        assert!(!allowed(&["--aa", "off"], taa));
        for args in [
            &["--dynamic-resolution"][..],
            &["--serial-passes"],
            &["--debug-overlay"],
            &["--debug-view", "wireframe"],
        ] {
            assert!(!allowed(args, taa), "{args:?} must observe full frames");
        }
        assert!(allowed(&["--debug-view", "off"], taa));
    }
}
