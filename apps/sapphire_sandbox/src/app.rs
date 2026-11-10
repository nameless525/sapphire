//! The sandbox [`App`]: window, GPU, renderer, clocks, sound and the frame loop.

use std::fmt;
use std::time::Duration;

use sapphire_assets::AssetStats;
use sapphire_core::thermal::ThermalReading;
use sapphire_gameplay::ActorId;
use sapphire_gameplay::replay::ActionSource;
use sapphire_platform::raw_window_handle::HandleError;
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, LifecycleEvent, Os, Platform,
    SurfaceHandle, Window, WindowDesc, WindowEvent,
};
use sapphire_render::graph::{FrameGraph, GraphError, GraphExecutor};
use sapphire_render::quality::{DeviceProfile, FrameTiming, Quality, TierTable};
use sapphire_render::{AmbientOcclusion, MaterialHandle, MeshHandle, RenderSettings, Renderer};
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuDebug, GpuInstance, GpuInstanceDesc, PipelineCacheDesc, PresentPolicy,
    RenderSurface, SkipReason, SurfaceAcquire, SurfaceTarget, wgpu,
};
use sapphire_time::{
    FixedStepConfig, FixedStepDriver, FixedSteps, RealClock, ScaleHandle, ScaleSource, Stopwatch,
};

use crate::LOG_TARGET;
#[cfg(feature = "dev")]
use crate::dev::{DevTools, FrameState};
use crate::input::{PLAYER, SandboxInput};
use crate::options::Options;
use crate::stats::{FrameStats, Stages};
use game_combat::combat::Combat;
use game_combat::combat::CombatOption;
use game_combat::cues::PadRumble;
use game_combat::enemy::EnemyMeshes;
use game_combat::fight::{Arena, FightShow};
use game_combat::party_looks::PartyLooks;
use game_ui::hud::{FightHud, ShownFight};
use game_ui::menus::MenuDocument;
use sapphire_stage::audio::DeviceAudio;
use sapphire_stage::content::Content;
use sapphire_stage::environment::FOG_CARDS;
use sapphire_stage::foliage::FoliageMeshes;
use sapphire_stage::lookdev::{Shot, TownMeshes};
use sapphire_stage::rims::RimRigMeshes;
use sapphire_stage::scene::{LookDev, Scene};
use sapphire_stage::studio::StudioMeshes;
use sapphire_stage::swatches::SWATCHES;
use sapphire_stage::{SceneShow, garments, layers, lookdev, skinned, studio, swatches};
use sapphire_ui::Atlas;

/// Loop period while nothing can be presented (occluded, minimized, backgrounded).
const IDLE_FRAME: Duration = Duration::from_millis(33);
/// Longest real-time step presentation (camera springs, held input) takes in one frame, so a
/// hitch can't fling the camera.
const MAX_PRESENTATION_DT: Duration = Duration::from_millis(100);
/// `--frames N` runs fail unless N frames were presented within this long (or 50 ms per frame,
/// whichever is longer), so an unattended run that never drew anything can't pass.
const FRAME_LIMIT_MIN_DEADLINE: Duration = Duration::from_secs(10);

pub struct SandboxApp {
    // Fields drop in declaration order: GPU objects go before the window they present to.
    // Crash capture is process-wide (see `crate::startup`) and outlives the app.
    /// The debug overlay and its cvars.
    #[cfg(feature = "dev")]
    dev: DevTools,
    renderer: Renderer,
    /// The asset server and what it uploaded to the renderer.
    content: Content,
    graph: GraphExecutor,
    surface: Option<RenderSurface>,
    gpu: GpuContext,
    window: Window,
    recovery: SurfaceRecovery,
    /// Devices lost while renderers were made, and what the next one is made with.
    losses: CreationLosses,
    /// How the GPU stack is built, kept to rebuild it after device loss.
    gpu_debug: GpuDebug,
    /// The surface's format, which the renderer and the overlay draw into.
    format: wgpu::TextureFormat,
    /// The quality tier, whose settings the renderer is made with, and the render scale.
    quality: Quality,
    /// The command line's overrides of the tier's renderer settings.
    options: Options,

    scene: Scene,
    input: SandboxInput,
    /// The audio system, with `--cue`: nothing audio exists without it.
    audio: Option<DeviceAudio>,
    /// What plays on the scene: with `--record` a simulation that takes a tick with each fixed step,
    /// played by the bindings, with `--arena` what stands it there, and with `--hud` the HUD.
    show: ShownFight,
    /// Whether the window last held the pointer for a fight.
    pointer_held: bool,

    real: RealClock,
    driver: FixedStepDriver,
    background_pause: Option<ScaleHandle>,
    /// Between `WillEnterBackground` and `DidEnterForeground`: no GPU work allowed (spec §6.5).
    suspended: bool,

    stats: FrameStats,
    /// The latest frame's CPU time per stage.
    stages: Stages,
    /// The last iteration's real length if it was steady, else zero: dynamic resolution's
    /// signal where the GPU has no timings.
    frame_time: Duration,
    /// This iteration has presented and rebuilt nothing so far, so its length measures
    /// rendering alone.
    steady: bool,
    presented_frames: u64,
    frame_limit: Option<u64>,
}

impl App for SandboxApp {
    const METADATA: AppMetadata = AppMetadata {
        name: "Sapphire Sandbox",
        organization: "Sapphire",
        identifier: "dev.sapphire.sandbox",
        version: env!("CARGO_PKG_VERSION"),
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let (mut options, unknown) = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        options.refuse_offscreen().map_err(AppInitError::msg)?;
        if !unknown.is_empty() {
            tracing::warn!(target: LOG_TARGET, ?unknown, "ignoring unknown arguments");
        }
        let info = *platform.system_info();
        // A phone's app has no working directory to speak of, and its cache is what `run-as` and
        // `devicectl` can fetch from.
        if info.device_class != DeviceClass::Desktop {
            options.files_under(platform.cache_dir());
        }
        tracing::info!(target: LOG_TARGET, version = Self::METADATA.version, ?info, ?options, "starting");

        let window = platform
            .create_window(&WindowDesc { title: Self::METADATA.name.to_owned(), ..WindowDesc::default() })?;

        // Narrow the build's defaults, never widen them: the Android emulator's Vulkan driver
        // (gfxstream) crashes naming objects, so labels go off there and only there.
        let base = GpuDebug::default();
        let debug = GpuDebug {
            object_labels: base.object_labels && !(info.is_emulator && info.os == Os::Android),
            ..base
        };
        let mut device = create_device(platform, &window, debug)?;
        // The tier comes from the device the GPU stack runs on; the renderer is made for it.
        let profile = DeviceProfile::new(info.device_class, device.gpu.caps(), info.ram_mib);
        let quality = Quality::new(TierTable::shipped(), &profile, options.quality_settings());
        // Frames are paced to the tier's rate (spec §6.2): on iOS the platform's display link asks
        // the display for it, and on Android its frame limiter holds it and the window asks for it.
        platform.set_frame_rate(quality.frame_rate());
        let mut content = Content::new(platform.host(), options.character(), options.sky())?;
        if options.keeps_character_mesh() {
            content.keep_character_mesh();
        }
        if options.party {
            content.watch_uploads(Box::<PartyLooks>::default());
        }
        content.override_materials(options.material_overrides());
        let mut show = ShownFight::default();
        let combat = options
            .combat
            .as_ref()
            .map(|option| {
                if options.arena.is_some()
                    && options.character() == lookdev::CHARACTER
                    && options.character_rig.is_none()
                {
                    show.fight.prepare_stock_combat(option, &mut content)
                } else {
                    Combat::new(option).map_err(AppInitError::msg)
                }
            })
            .transpose()?;
        if let (Some(_), Some(combat)) = (options.arena, &combat) {
            game_combat::arena::check_scenario(combat, options.party).map_err(AppInitError::msg)?;
            // The window's stage loads the town before it knows the fight; a fight in a zone plays offscreen,
            // or in the game, until it doesn't.
            if game_combat::arena::FightPlace::of(combat.scenario())
                .map_err(AppInitError::msg)?
                .zone
                .is_some()
            {
                return Err(AppInitError::msg(format!(
                    "`{}` stands in a zone, which the window's arena can't yet: play it with --capture or --bench",
                    combat.scenario()
                )));
            }
        }
        // A device lost while the renderer is made gets a new device and another try, under
        // `CreationLosses`' policy, rather than a renderer of dead objects.
        let mut losses = CreationLosses::default();
        let mut renderer = loop {
            let settings = losses.settings(options.render_settings(quality.render_settings()));
            match create_renderer(
                &mut content,
                &mut show.fight.show,
                &device.gpu,
                device.format,
                settings,
                &options,
            ) {
                Ok(renderer) => {
                    losses.built();
                    break renderer;
                }
                Err(error) if device.gpu.is_lost() => {
                    losses.lost(&settings, quality.active().id, &error).map_err(AppInitError::msg)?;
                    // The old surface goes before a new one is made for the same window.
                    drop(device);
                    device = create_device(platform, &window, debug)?;
                }
                Err(error) => return Err(error),
            }
        };
        // Desktop-only bindings, such as `Esc` quitting, are a context of their own.
        let desktop = info.device_class == DeviceClass::Desktop;
        let mut input = SandboxInput::new(desktop, options.combat_input, window.size())?;
        input.follow_window(&window);
        if matches!(options.combat, Some(CombatOption::Fight { .. })) {
            input.listen_for_fight_restart()?;
        }
        let mut scene = Scene::with_character(renderer.character, renderer.character_bounds, options.skinned);
        scene.set_sky_material(renderer.sky);
        if let Some(rig) = &options.character_rig {
            scene.set_character_rig(rig).map_err(AppInitError::msg)?;
        }
        if options.lights {
            scene.spawn_demo_lights();
        }
        if let Some(stalk) = renderer.stalk {
            scene.set_stalk_mesh(stalk);
        }
        if let Some(meshes) = renderer.swatches {
            scene.spawn_swatches(meshes);
        }
        if let Some(meshes) = renderer.layer_samples {
            scene.spawn_layer_samples(meshes);
        }
        if let Some(meshes) = renderer.garments {
            scene.spawn_garments(meshes);
        }
        if let Some(head) = renderer.face_head {
            scene.spawn_face_rig(head, options.faces.unwrap_or_default());
        }
        if let (Some(layout), Some(meshes)) = (options.rim_rig, renderer.rim_rig) {
            scene.spawn_rim_rig(layout, meshes);
        }
        if let Some(courtyard) = renderer.courtyard {
            scene.spawn_environment(courtyard);
            if let Some(meshes) = renderer.fog_cards {
                scene.spawn_fog_cards(meshes);
            }
        }
        if let (Some(view), Some(meshes)) = (options.foliage, renderer.foliage) {
            scene.spawn_foliage(view, meshes);
        }
        if let Some(arcade) = renderer.arcade {
            scene.spawn_arcade(arcade, content.probe_hours());
        }
        if options.outline_row {
            scene.spawn_outline_row(renderer.character);
        }
        if let Some(mesh) = renderer.hard_edges {
            scene.spawn_hard_edges(mesh);
        }
        if let (Some(shot), Some(town)) = (options.look_dev, renderer.town) {
            let (pose, figure) = lookdev::hold(&mut content, &mut renderer.renderer, &device.gpu)?;
            scene.spawn_look_dev(shot, LookDev { town, fog_cards: renderer.fog_cards, pose, figure });
            if let Some(settings) = options.stream.settings(quality.active().id) {
                scene.open_town_zone(&mut content, settings, &mut renderer.renderer, &device.gpu)?;
            }
            if let Some(clip) = &options.clip {
                content.load_clip(
                    clip.resolve(&content).map_err(AppInitError::msg)?,
                    &mut renderer.renderer,
                    &device.gpu,
                )?;
                let mut player = lookdev::player(&content)
                    .map_err(AppInitError::msg)?
                    .with_root_motion(!options.clip_in_place)
                    .with_springs_on(options.springs != Some(false));
                if let (Some(ik), Some(block)) = (&options.ik, renderer.ik_block) {
                    let solver =
                        scene.start_ik(ik, block, &content, player.pose()).map_err(AppInitError::msg)?;
                    player = player.with_ik(solver);
                }
                scene.play_clip(player);
                scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
            }
            if let Some(script) = options.graph {
                content.load_graph(lookdev::GRAPH, &mut renderer.renderer, &device.gpu)?;
                let mut driver = lookdev::graph_driver(&content).map_err(AppInitError::msg)?;
                if let (Some(ik), Some(block)) = (&options.ik, renderer.ik_block) {
                    let solver = scene
                        .start_ik(ik, block, &content, driver.player.pose())
                        .map_err(AppInitError::msg)?;
                    driver = driver.with_ik(solver);
                }
                scene.play_graph(
                    driver.with_springs_on(options.springs != Some(false)),
                    sapphire_stage::anim_graph::GraphShow::new(script),
                );
                scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
            }
            if let Some(meshes) = renderer.enemy {
                let held = show.fight.show.held_enemy(&content)?;
                show.fight.show.spawn_enemy(&mut scene, meshes, held);
            }
            if let Some(model) = options.model {
                let held = lookdev::model::hold(&mut content, &mut renderer.renderer, &device.gpu, model)?;
                lookdev::model::spawn(&mut scene, held);
            }
        }
        if let (Some(shot), Some(town), Some(enemy)) = (options.arena, renderer.town, renderer.enemy) {
            let (pose, figure) = lookdev::hold(&mut content, &mut renderer.renderer, &device.gpu)?;
            let ground =
                show.fight.show.load_arena_ground(&mut content, &mut renderer.renderer, &device.gpu)?;
            let world = ground.share();
            let look = LookDev { town, fog_cards: renderer.fog_cards, pose, figure };
            let held = show.fight.show.held_enemy(&content)?;
            show.fight.show.spawn_arena(&mut scene, Arena { shot, look, ground, enemy, held });
            if let Some(settings) = options.stream.settings(quality.active().id) {
                scene.open_town_zone(&mut content, settings, &mut renderer.renderer, &device.gpu)?;
            }
            if let Some(timeline) = options.timeline {
                let timeline = content.load_timeline(timeline, &mut renderer.renderer, &device.gpu)?;
                show.fight.show.play_timeline(timeline, sapphire_sequencer::Origin::default());
            }
            if let Some(meshes) = renderer.party {
                show.fight.show.set_party_meshes(&mut scene, meshes);
            }
            if let Some(clip) = &options.clip {
                content.load_clip(
                    clip.resolve(&content).map_err(AppInitError::msg)?,
                    &mut renderer.renderer,
                    &device.gpu,
                )?;
                let mut player = lookdev::player(&content)
                    .map_err(AppInitError::msg)?
                    .with_root_motion(false)
                    .with_springs_on(options.springs != Some(false));
                if let Some(ik) = &options.ik {
                    let look = ik.look.map(|look| (look, 0.0));
                    let solver = scene
                        .ik_on(world.clone(), look, &content, player.pose())
                        .map_err(AppInitError::msg)?;
                    player = player.with_ik(solver);
                }
                scene.play_clip(player);
                scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
            }
            if options.combat.is_some() {
                content.load_graph(lookdev::GRAPH, &mut renderer.renderer, &device.gpu)?;
                show.fight.show.load_enemy_graph(&mut content, &mut renderer.renderer, &device.gpu)?;
                let player = lookdev::graph_driver(&content).map_err(AppInitError::msg)?;
                let solver = scene
                    .ik_on(world.clone(), None, &content, player.player.pose())
                    .map_err(AppInitError::msg)?;
                let enemy = show.fight.show.enemy_driver(&content, world)?;
                show.fight.show.play_fighters(
                    &mut scene,
                    player.with_springs_on(options.springs != Some(false)).with_ik(solver),
                    enemy,
                );
                scene.set_spring_quality(sapphire_stage::animate::spring_quality(quality.active().id));
            }
        }
        if let Some(sword) = renderer.sword {
            let hand = lookdev::held_hand(&content).map_err(AppInitError::msg)?;
            show.fight.show.spawn_blade(&mut scene, sword, hand);
        }
        // The day, the sky options, the starting view and the turntable's speed, last so `--view`
        // holds whatever framing the scene chose.
        options.apply_to(&mut scene).map_err(AppInitError::msg)?;
        if let Some(scale) = options.effect_scale {
            show.fight.show.set_effect_scale(scale);
        }
        if let Some(show) = options.vfx {
            scene.play_vfx(show, sapphire_stage::vfx::budget(quality.active().id));
        }
        if options.cues {
            show.fight
                .show
                .play_cues(&mut scene, sapphire_stage::vfx::budget(quality.active().id))
                .map_err(AppInitError::msg)?;
        }
        if let Some(combat) = combat {
            show.fight.attach_prepared_combat(combat, &mut scene, &content)?;
        }
        // Each of the fight's kinds of enemy, drawn as itself (M3 B3b), before the prewarm sees them.
        show.fight.load_kinds(&mut content, &mut renderer.renderer, &device.gpu)?;
        // The stage after the hour, which places its pillars.
        if let (Some(layout), Some(meshes)) = (options.stage, renderer.stage) {
            scene.spawn_studio(layout, meshes);
        }
        let audio = match (&options.cue, show.fight.show.cue_show()) {
            (Some(cue), _) => {
                DeviceAudio::new(cue, platform, &mut content, &mut renderer.renderer, &device.gpu)?
            }
            (None, Some(show)) => {
                let sounds = show.sounds_to_load();
                DeviceAudio::for_show(&sounds, platform, &mut content, &mut renderer.renderer, &device.gpu)?
            }
            (None, None) => None,
        };
        if let Some(hud) = options.hud {
            show.hud = Some(match &show.fight.combat {
                Some(_) => FightHud::for_fight(hud),
                None => FightHud::new(hud),
            });
        }
        if let Some(shown) = &options.ui_screen {
            let screens = MenuDocument::read(platform.content()).map_err(AppInitError::msg)?;
            let atlas = show.hud.as_ref().map_or_else(Atlas::paint, |hud| hud.atlas().clone());
            let (stack, inset) =
                shown.load(&screens, &mut content, &mut renderer.renderer, &device.gpu, atlas)?;
            show.screen = Some(stack);
            if inset {
                scene.spawn_showcase(scene.character(), 0);
            }
        }
        if let Some(sample) = options.display_text {
            let text = crate::display_text::DisplayText::load(
                sample,
                &mut content,
                &mut renderer.renderer,
                &device.gpu,
            )?;
            show.over = Some(Box::new(text));
        }
        let atlas = show.hud.as_ref().map_or_else(Atlas::paint, |hud| hud.atlas().clone());
        game_ui::wipe::cover_trips(
            &mut show,
            atlas,
            &mut content,
            &mut renderer.renderer,
            &device.gpu,
            quality.active().id,
        )?;
        // The screen effects it may push, `--post-effect`'s among them, the effects it plays, its sky
        // and its HUD show on their first frame.
        scene.prewarm(&show, &mut renderer.renderer);
        // Its bars read the fighters from the first frame, with no step yet.
        if let Some(combat) = &show.fight.combat {
            let followed = show.fight.show.follow_simulation(&mut scene, combat.simulation(), combat.cast());
            show.hear(followed);
        }
        scene.warm_fixed_step();
        #[cfg(feature = "dev")]
        let dev = DevTools::new(&device.gpu, device.format, &options, platform, &mut content)?;
        #[cfg(feature = "dev")]
        if let Some(combat) = &mut show.fight.combat {
            dev.start_fight_from_cooks(combat, &content);
        }
        #[cfg(not(feature = "dev"))]
        if options.debug_overlay.is_some() {
            tracing::warn!(target: LOG_TARGET, "--debug-overlay needs a `dev` build (`--features dev`)");
        }
        Ok(Self {
            #[cfg(feature = "dev")]
            dev,
            renderer: renderer.renderer,
            content,
            graph: GraphExecutor::new(),
            surface: Some(device.surface),
            gpu: device.gpu,
            window,
            recovery: SurfaceRecovery::default(),
            losses,
            gpu_debug: debug,
            format: device.format,
            quality,
            frame_limit: options.frame_limit,
            options,
            scene,
            input,
            audio,
            show,
            pointer_held: false,
            real: RealClock::new(),
            driver: FixedStepDriver::new(FixedStepConfig::default()),
            background_pause: None,
            suspended: false,
            stats: FrameStats::new(),
            stages: Stages::default(),
            frame_time: Duration::ZERO,
            steady: false,
            presented_frames: 0,
        })
    }

    fn event(&mut self, platform: &mut Platform, event: &Event) -> AppControl {
        // Input queues for the next frame. Losing focus or the foreground releases everything
        // held, since the releases may never arrive. With the debug overlay, it sees each event
        // first and passes on what it doesn't use.
        let (show, input) = (&mut self.show, &mut self.input);
        #[cfg(feature = "dev")]
        self.dev.handle_event(event, &mut |event| game_event(show.hud.as_mut(), input, event));
        #[cfg(not(feature = "dev"))]
        game_event(show.hud.as_mut(), input, event);
        // A thumb on the stick and another on a button are playing, not holding two fingers still.
        #[cfg(feature = "dev")]
        if let Event::Touch(touch) = event
            && touch.phase == sapphire_platform::TouchPhase::Down
            && show.hud.as_ref().is_some_and(|hud| hud.holds_control(touch.finger))
        {
            self.dev.claim_touch(touch.finger, touch.timestamp);
        }
        match event {
            Event::Quit { .. } => return AppControl::Exit,
            Event::Window { event, .. } => match *event {
                WindowEvent::CloseRequested => return AppControl::Exit,
                WindowEvent::PixelSizeChanged { width, height } => self.resize(width, height),
                // SDL reports no 0×0 size on minimize; park the surface so nothing is acquired.
                WindowEvent::Minimized => self.resize(0, 0),
                WindowEvent::Restored => {
                    let size = self.window.pixel_size();
                    self.resize(size.width, size.height);
                }
                // Whoever's holding the pad isn't watching any more.
                WindowEvent::FocusLost => {
                    let pad = self.input.active_gamepad();
                    self.show.fight.show.stop_rumble(&mut PadRumble { platform, pad });
                    focus_lost(&mut self.input, self.show.fight.combat.as_ref());
                }
                _ => {}
            },
            Event::Lifecycle { event, .. } => self.lifecycle(*event, platform),
            _ => {}
        }
        AppControl::Continue
    }

    fn iterate(&mut self, platform: &mut Platform) -> AppControl {
        let measured = self.real.update();
        // Where vsyncs start frames (iOS's display link, Android's frame limiter), the frame's time
        // is its interval in whole vsyncs, so steady frames run whole fixed steps whatever the
        // scheduler's jitter.
        let frame_time = platform.frame_delta().unwrap_or(measured);
        let pacing = platform.frame_pacing();
        self.frame_time = if self.steady { frame_time } else { Duration::ZERO };
        self.steady = true;
        // The previous frame's stages go with its length.
        #[cfg(feature = "dev")]
        self.dev.begin_frame(frame_time, Some(&self.stages), Some(pacing));
        let mut stopwatch = Stopwatch::start();
        // On the clock event timestamps use, so every trigger is judged on one timeline.
        #[cfg(feature = "dev")]
        {
            let (show, input) = (&mut self.show, &mut self.input);
            self.dev.poll(platform.now(), &mut |event| game_event(show.hud.as_mut(), input, event));
        }
        #[cfg(feature = "dev")]
        if sapphire_stage::cvars::take_device_loss() {
            tracing::warn!(target: LOG_TARGET, "losing the GPU device, as debug.lose_device asks");
            self.gpu.device().destroy();
        }
        let now = platform.now();
        // The HUD's touch zones and animation are on the real clock too. What they read reaches this
        // frame's steps and camera through the bindings, as a pad's does, so they go first.
        self.input.follow_window(&self.window);
        update_hud(self.show.hud.as_mut(), now, &self.window, &mut self.input);
        if self.input.update(now) {
            return AppControl::Exit;
        }
        self.follow_pointer();
        self.stages.input = stopwatch.lap();
        if self.suspended {
            self.steady = false;
            std::thread::sleep(IDLE_FRAME);
            return self.frame_limit_control().unwrap_or(AppControl::Continue);
        }
        if self.gpu.is_lost() {
            if self.recovery.due(self.real.elapsed()) && self.recover_from_device_loss(platform).is_err() {
                return AppControl::Fail;
            }
        } else if self.surface.is_none() && self.recovery.due(self.real.elapsed()) {
            self.rebuild_surface();
        }
        if !self.gpu.is_lost() {
            if self.follow_tier(platform).is_err() {
                return AppControl::Fail;
            }
            // Packs, shaders and documents changed on disk, swapped in before loading.
            #[cfg(feature = "dev")]
            self.dev.reload(
                &mut self.content,
                &mut self.renderer,
                &self.gpu,
                &mut self.scene,
                &mut self.show.fight,
                &mut crate::input::DocumentsHere {
                    input: Some(&mut self.input),
                    hud: self.show.hud.as_mut(),
                    screen: self.show.screen.as_mut(),
                },
            );
            // Finished reads, unloads and this frame's uploads, within the upload budget.
            self.content.update(&mut self.renderer, &self.gpu);
            self.scene.follow_content(&mut self.content, &mut self.show);
        }
        // Recovery, tier changes and loading are no stage of the frame.
        stopwatch.lap();

        let steps = self.simulate(frame_time);
        self.stages.simulation = stopwatch.lap();
        self.stages.fixed_steps = steps.count;

        // Presentation runs on the real clock, so the camera stays responsive in slow motion.
        let dt = frame_time.min(MAX_PRESENTATION_DT).as_secs_f32();
        let view_dt = drive_view(&mut self.scene, &mut self.show, &self.input, dt);
        let presentation = stopwatch.lap();
        if let Some(audio) = &mut self.audio {
            audio.update(dt, platform, self.scene.sound(), self.show.fight.show.cue_sounds());
        }
        let pad = self.input.active_gamepad();
        self.show.fight.show.end_cue_frame(
            &mut self.scene,
            dt,
            view_dt,
            steps.alpha,
            &mut PadRumble { platform, pad },
        );
        self.stages.audio = stopwatch.lap();
        // Before the render scale, which reads the ladder's rung.
        let thermal = platform.thermal();
        self.follow_heat(platform, &thermal);
        let presented = match self.render(steps.alpha, &mut stopwatch, presentation) {
            Ok(presented) => presented,
            Err(error) => {
                // The graph is built in code, so it would fail the same way every frame.
                tracing::error!(target: LOG_TARGET, %error, "invalid frame graph");
                return AppControl::Fail;
            }
        };
        if presented {
            self.presented_frames += 1;
            self.recovery.reset();
            self.losses.presented();
            // Rendering works with this pipeline cache, so the next boot may trust it.
            self.gpu.confirm_pipeline_cache();
        } else {
            // Nothing blocked on vsync this frame (occluded, minimized, no surface), so pace
            // the loop here instead of spinning a core.
            self.steady = false;
            std::thread::sleep(IDLE_FRAME);
        }

        self.scene.end_frame();
        self.stats.record(frame_time, steps.dropped, presented, &self.stages, &pacing, thermal);
        profiling::finish_frame!();
        self.frame_limit_control().unwrap_or(AppControl::Continue)
    }

    fn quit(&mut self, _platform: &mut Platform) {
        self.save_pipeline_cache();
        if let Some(Err(error)) = self.show.fight.combat.take().map(Combat::finish) {
            tracing::error!(target: LOG_TARGET, %error, "the recording didn't reach its file");
        }
        tracing::info!(target: LOG_TARGET, presented = self.presented_frames, "quit");
    }
}

/// An event for the game: the HUD's touch zones take what lands on them, and the game's input gets
/// the rest. Losing focus or the foreground reaches both. While paused a landing is the pause's, since
/// a tap is how a phone gets out of it, and the zones keep only the fingers they already hold: a thumb
/// that was on the stick when the pause came still has to lift.
fn game_event(hud: Option<&mut FightHud>, input: &mut SandboxInput, event: &Event) {
    let pauses_landing = input.pause_takes_landings()
        && matches!(event, Event::Touch(touch) if touch.phase == sapphire_platform::TouchPhase::Down);
    if pauses_landing || !hud.is_some_and(|hud| hud.handle_event(event)) {
        input.handle_event(event);
    }
}

/// The HUD's frame on the real clock, where a window has its metrics: what its touch zones read goes to
/// `input`, before that judges the frame.
fn update_hud(
    hud: Option<&mut FightHud>,
    now: sapphire_platform::Timestamp,
    window: &Window,
    input: &mut SandboxInput,
) {
    let Some(hud) = hud else { return };
    let metrics = hud.metrics_of(window);
    hud.follow_devices(input.desktop(), input.active_gamepad().is_some());
    read_touch(hud, now, metrics, input);
}

/// What the HUD's touch zones read this frame, on a frame of `metrics`, to `input`.
fn read_touch(
    hud: &mut FightHud,
    now: sapphire_platform::Timestamp,
    metrics: sapphire_ui::UiMetrics,
    input: &mut SandboxInput,
) {
    if let Some(touch) = hud.update(now, metrics) {
        input.read_touch(touch, now);
    }
}

/// The view's share of a windowed frame, `dt` real seconds after the last: `input`'s orbit and zoom, on the
/// real clock so the view stays responsive in slow motion. A pause holds a fight's view, as the director's
/// frame asks, shakes and all: only the sandbox's own orbit still moves, for looking around at what's held,
/// and the HUD says it's waiting. Returns the seconds the view moved by, for `FightShow::end_cue_frame`.
fn drive_view(scene: &mut Scene, show: &mut ShownFight, input: &SandboxInput, dt: f32) -> f32 {
    let view_dt = if input.is_paused() { 0.0 } else { dt };
    if let Some(hud) = &mut show.hud {
        hud.show_pause(input.is_paused());
    }
    let mut camera = scene.camera_mut();
    input.drive_camera(&mut camera);
    camera.update(dt);
    let (look, zoom) = input.camera_motion();
    show.fight.show.drive_camera(view_dt, look, zoom);
    view_dt
}

/// A fight someone's playing in a desktop's window waits for them when they go elsewhere, and they end
/// the pause as any other, so a click that brings the window back doesn't also swing. A phone has its own
/// road back from the background, and a script or a file has nobody to come back, so its steps go on and
/// its checksums hold.
fn focus_lost(input: &mut SandboxInput, combat: Option<&Combat>) {
    if input.desktop() && combat.is_some_and(Combat::is_played) {
        input.pause();
    }
}

/// What decides who has the pointer this frame.
#[derive(Clone, Copy, Debug)]
struct PointerState {
    /// A desktop's window: a phone has no pointer to hold.
    desktop: bool,
    /// Someone's playing a fight here. A script needs no mouse, and whoever's watching it may want theirs.
    played: bool,
    /// In a pause the player may want the rest of their screen, so that's how they get the pointer back.
    paused: bool,
    /// The keyboard's focus is here.
    focused: bool,
    /// The `dev` overlay is up, which has nothing to click with otherwise.
    overlay: bool,
}

impl PointerState {
    fn of(input: &SandboxInput, combat: Option<&Combat>, focused: bool, overlay: bool) -> Self {
        Self {
            desktop: input.desktop(),
            played: combat.is_some_and(Combat::is_played),
            paused: input.is_paused(),
            focused,
            overlay,
        }
    }

    /// Whether the fight holds the pointer: the mouse turns its camera, which a cursor reaching the
    /// screen's edge would stop, and a click off the window would leave the fight.
    fn holds(self) -> bool {
        self.desktop && self.played && self.focused && !self.paused && !self.overlay
    }
}

impl SandboxApp {
    /// Gives a fight the pointer or takes it back, as [`PointerState`] says, once for each change.
    fn follow_pointer(&mut self) {
        #[cfg(feature = "dev")]
        let overlay = self.dev.overlay_visible();
        #[cfg(not(feature = "dev"))]
        let overlay = false;
        let focused = self.window.has_keyboard_focus();
        let held = PointerState::of(&self.input, self.show.fight.combat.as_ref(), focused, overlay).holds();
        if held != self.pointer_held {
            self.pointer_held = held;
            self.window.set_relative_mouse(held);
        }
    }

    /// Runs this frame's fixed steps, none while paused. Queued input lands on the first of them
    /// (rule R3), and the steps themselves must not allocate (rule R6).
    fn simulate(&mut self, frame_time: Duration) -> FixedSteps {
        profiling::scope!("simulate");
        // The overlay's recorder, between frames: a recording starts or ends with no step under way.
        #[cfg(feature = "dev")]
        self.dev.apply_recorder(&mut self.show.fight, &mut self.scene, &self.content);
        let steps = self.input.advance(&mut self.driver, frame_time);
        if steps.count == 0 {
            return steps;
        }
        let actions = self.input.take_actions();
        // Between steps, as the overlay's recorder is: a new fight's simulation allocates.
        if actions.restart_fight
            && let Some(combat) = &mut self.show.fight.combat
        {
            combat.restart();
        }
        self.scene.apply(actions.of_scene());
        // The camera the player was looking at when they pushed. A tick carries the direction they
        // meant, so nothing past here reads a camera.
        let camera_yaw = self.show.fight.show.camera_heading(&self.scene);

        for _ in 0..steps.count {
            let scene = &mut self.scene;
            #[cfg(feature = "dev")]
            let dev = &mut self.dev;
            let ((), allocations) = sapphire_core::alloc::count_in_scope(|| {
                // The overlay's pause, step and speed, applied to the step about to run.
                #[cfg(feature = "dev")]
                dev.before_fixed_step(scene);
                scene.step();
            });
            // Only exact where counts are per thread (spec R6; `PER_THREAD_COUNTS`).
            debug_assert!(
                !sapphire_core::alloc::PER_THREAD_COUNTS || allocations.allocations == 0,
                "rule R6: the fixed step allocated {} times",
                allocations.allocations
            );

            // A cutscene holds the fight, and the scene's own step goes on under it.
            if self.show.fight.holds_step() {
                continue;
            }
            // After the scene's own step, as `Stage::step` does: what a cue pushes on the scene's effect
            // stack begins here, so the step just taken doesn't age it before its first frame.
            self.read_combat_step(camera_yaw);
            if let Some(combat) = &self.show.fight.combat {
                let followed = self.show.fight.show.follow_simulation(
                    &mut self.scene,
                    combat.simulation(),
                    combat.cast(),
                );
                self.show.fight.show.show_cues(&mut self.scene, combat.simulation());
                self.show.hear(followed);
                self.show.show_damage(&self.scene);
            }
        }
        steps
    }

    /// The tick's input, for `--record`'s simulation if there is one, which then takes its tick, and
    /// for dev builds' log either way. Outside the steps' allocation count, because a recording's
    /// list of actions grows: the simulation's own step is held to rule R6 by `SimStats::allocations`.
    fn read_combat_step(&mut self, camera_yaw: f32) {
        let mut bindings = self.input.source(camera_yaw);
        #[cfg(feature = "dev")]
        let dev = &mut self.dev;
        let mut read = |tick: u64, cast: &[ActorId]| {
            let actions = bindings.actions(tick, cast);
            #[cfg(feature = "dev")]
            dev.log_tick_actions(&actions);
            actions
        };
        game_combat::combat::read_bindings(self.show.fight.combat.as_mut(), &mut read, PLAYER);
    }

    /// Extracts the frame, then draws and presents it if the surface gave an image. Returns
    /// whether a frame was presented. `stopwatch` times the extract and render stages, which
    /// `presentation` (already measured) joins; acquiring and presenting wait for vsync and
    /// count for neither.
    fn render(
        &mut self,
        alpha: f32,
        stopwatch: &mut Stopwatch,
        presentation: Duration,
    ) -> Result<bool, GraphError> {
        profiling::scope!("render");
        // A capture live link asked for draws this frame into a texture it reads back, in place
        // of the surface's image: the display shows the frame before for one more refresh.
        #[cfg(feature = "dev")]
        let capture = match self.surface.as_ref().and_then(RenderSurface::size) {
            Some(size) => {
                self.dev.take_capture().and_then(|request| request.start(&self.gpu, size, self.format))
            }
            None => {
                if let Some(request) = self.dev.take_capture() {
                    request.refuse("there is no drawable surface to capture".to_owned());
                }
                None
            }
        };
        #[cfg(feature = "dev")]
        let acquire = capture.is_none();
        #[cfg(not(feature = "dev"))]
        let acquire = true;
        // The frame borrows the surface until the end of this block.
        let skipped = {
            let (frame, skipped) = match self.surface.as_mut().filter(|_| acquire).map(RenderSurface::acquire)
            {
                Some(SurfaceAcquire::Frame(frame)) => (Some(frame), None),
                Some(SurfaceAcquire::Skip(reason)) => (None, Some(reason)),
                None => (None, None),
            };
            stopwatch.lap();
            // What the frame draws into, and its size.
            let target = frame.as_ref().map(|frame| (frame.view(), frame.size()));
            #[cfg(feature = "dev")]
            let target = target.or_else(|| capture.as_ref().map(|capture| (capture.view(), capture.size())));
            // Every frame is extracted and prepared, drawn or not, so the renderer's persistent
            // scene sees every packet's deltas. On a lost device the GPU calls do nothing (wgpu
            // drops device-lost errors), and the rebuilt renderer is sent everything.
            #[cfg(feature = "dev")]
            self.dev.after_steps(self.show.fight.combat.as_ref(), &self.scene);
            self.scene.animate(&self.content, alpha, &mut self.show);
            self.stages.secondary_motion = self.scene.anim_stats().spring_time;
            if target.is_some() {
                self.show.begin_interface(self.renderer.ui_page_state());
            }
            let packet = self.scene.frame_packet(alpha, target.map(|(_, size)| size), &mut self.show);
            self.stages.extract = presentation + stopwatch.lap();
            // Before `prepare`, which culls hulls by it, and readies the debug view.
            self.renderer.set_outline_distance(self.quality.outline_distance());
            self.renderer.set_debug_view(sapphire_render::debug_view::DEBUG_VIEW.get());
            self.renderer.set_color_vision(sapphire_render::color_vision::COLOR_VISION.get());
            self.renderer.prepare(&self.gpu, packet);
            if let Some((view, size)) = target {
                let timing = FrameTiming::new(&self.graph, self.frame_time);
                self.renderer.set_render_scale(self.quality.render_scale(size, &timing));
                // The overlay's own frame is no stage of the game's; its panel shows its cost.
                #[cfg(feature = "dev")]
                let overlay = {
                    let started = Stopwatch::start();
                    let (width, height) = size;
                    let window = sapphire_debug::WindowMetrics {
                        pixel_size: sapphire_platform::Size { width, height },
                        ..sapphire_debug::WindowMetrics::of(&self.window, self.dev.focused())
                    };
                    let state = FrameState {
                        window,
                        render: self.renderer.stats(),
                        quality: &self.quality,
                        gpu_timing: self.quality.wants_gpu_timing(),
                        gpu: &self.gpu,
                        input: Some(&self.input),
                        scene: &self.scene,
                        assets: self.content.stats(),
                        audio: self.audio.as_ref().map(DeviceAudio::stats),
                        combat: self.show.fight.combat.as_ref(),
                    };
                    // It also keeps GPU timing on while dynamic resolution reads it.
                    let output = self.dev.update(state, &mut self.graph);
                    DevTools::sync_text_input(&self.window, &output);
                    started.elapsed()
                };
                #[cfg(not(feature = "dev"))]
                let overlay = {
                    self.graph.set_gpu_timing(&self.gpu, self.quality.wants_gpu_timing());
                    Duration::ZERO
                };
                let mut graph = FrameGraph::new();
                // The renderer sizes its attachments from the frame itself.
                let target = graph.import_texture("swapchain", view);
                self.renderer.record(&mut graph, target);
                #[cfg(feature = "dev")]
                self.dev.record(&mut graph, target);
                #[cfg(feature = "dev")]
                if let Some(capture) = &capture {
                    capture.record(&mut graph, target);
                }
                self.graph.execute(&self.gpu, graph)?;
                self.stages.render = stopwatch.lap().saturating_sub(overlay);
                #[cfg(feature = "dev")]
                if let Some(capture) = capture {
                    capture.finish(&self.gpu, self.dev.frames());
                }
                // A captured frame isn't presented.
                let Some(frame) = frame else { return Ok(false) };
                frame.present();
                return Ok(true);
            }
            self.stages.render = stopwatch.lap();
            skipped
        };
        let Some(reason) = skipped else { return Ok(false) };
        self.stats.skipped(reason);
        if reason == SkipReason::Lost {
            // `Lost` is sticky until the surface is rebuilt from a fresh native handle;
            // `recovery` paces the attempts.
            self.surface = None;
        }
        // `DeviceLost` is handled in `iterate`, which rebuilds the whole GPU stack.
        Ok(false)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(surface) = &mut self.surface {
            surface.resize(width, height);
        }
    }

    /// Spec §6.5: no GPU work in the background. Android also destroys the native window, so
    /// the surface is dropped going down and rebuilt coming back; elsewhere it is kept.
    fn lifecycle(&mut self, event: LifecycleEvent, platform: &mut Platform) {
        tracing::info!(target: LOG_TARGET, ?event, "lifecycle");
        match event {
            LifecycleEvent::WillEnterBackground => {
                self.suspended = true;
                if let Some(combat) = &mut self.show.fight.combat {
                    combat.keep_so_far();
                }
                let pad = self.input.active_gamepad();
                self.show.fight.show.stop_rumble(&mut PadRumble { platform, pad });
                if let Some(audio) = &mut self.audio {
                    audio.set_background(true, platform);
                }
                if !Os::CURRENT.keeps_surface_in_background() {
                    self.surface = None;
                }
                if self.background_pause.is_none() {
                    self.background_pause =
                        self.scene.world_clock_mut().0.push_scale(ScaleSource::Pause, 0).ok();
                }
                self.save_pipeline_cache();
            }
            LifecycleEvent::DidEnterForeground => {
                // `iterate` doesn't run while suspended on Android (or after a real iOS
                // background), so the next frame would otherwise see the whole pause as one
                // frame: drop that gap before anything steps.
                self.real.update();
                self.suspended = false;
                if let Some(audio) = &mut self.audio {
                    audio.set_background(false, platform);
                }
                if let Some(handle) = self.background_pause.take() {
                    self.scene.world_clock_mut().0.pop_scale(handle);
                }
                if self.surface.is_none() {
                    self.recovery.reset();
                    self.rebuild_surface();
                }
            }
            LifecycleEvent::Terminating => {
                self.save_pipeline_cache();
                if let Some(combat) = &mut self.show.fight.combat {
                    combat.keep_so_far();
                }
            }
            LifecycleEvent::LowMemory if trims_for_memory_warnings(Os::CURRENT) => self.trim_memory(),
            LifecycleEvent::DidEnterBackground
            | LifecycleEvent::WillEnterForeground
            | LifecycleEvent::LowMemory => {}
        }
    }

    /// A system memory warning (iOS): drops the assets nothing uses, which hot reloads, live link
    /// pushes and renderer rebuilds leave behind, and logs what that freed (review PA-004).
    fn trim_memory(&mut self) {
        let (before, after) = self.content.trim();
        let resident = |stats: &AssetStats| stats.resident.iter().sum::<u64>();
        tracing::info!(
            target: LOG_TARGET,
            unloaded = after.unloads - before.unloads,
            assets = after.assets,
            resident_before = resident(&before),
            resident_after = resident(&after),
            "memory warning: unused assets unloaded"
        );
    }

    /// Recreates the surface from a freshly fetched native handle. On failure the app keeps
    /// running without one, and [`SurfaceRecovery`] schedules the next attempt.
    fn rebuild_surface(&mut self) {
        self.surface = None;
        self.steady = false;
        self.recovery.attempted(self.real.elapsed());
        let size = self.window.pixel_size();
        let result = self.window.surface_handle().map_err(|error| error.to_string()).and_then(|handle| {
            let target = surface_target(&handle).map_err(|error| error.to_string())?;
            // SAFETY: the surface owns `handle`, retaining the native window. App state drops
            // the surface before the SDL run ends; the handle cannot extend that run.
            let mut surface = unsafe { RenderSurface::new(self.gpu.instance(), target, handle) }
                .map_err(|error| error.to_string())?;
            surface
                .configure(&self.gpu, size.width, size.height, PresentPolicy::Fifo)
                .map_err(|error| error.to_string())?;
            Ok(surface)
        });
        match result {
            Ok(surface) => {
                self.surface = Some(surface);
                tracing::info!(target: LOG_TARGET, width = size.width, height = size.height, "surface rebuilt");
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, retry_in = ?self.recovery.backoff, "surface rebuild failed");
            }
        }
    }

    /// Rebuilds everything that lived on the lost device (instance, context, surface,
    /// renderer) and has the asset server upload every asset to it again, per the recovery contract on
    /// `GpuContext`. On failure the app keeps running without a GPU and [`SurfaceRecovery`]
    /// paces the next attempt, made as [`CreationLosses`] says if the new device was lost too, or
    /// the lost one before its stack presented anything; an error when it says to stop.
    fn recover_from_device_loss(&mut self, platform: &Platform) -> Result<(), GaveUp> {
        tracing::warn!(target: LOG_TARGET, "GPU device lost; rebuilding the GPU stack");
        // A stack that lost its device before presenting anything counts as one that lost it
        // coming up, and may change what the rebuild is made with.
        let used = self.render_settings();
        self.losses.stack_lost(&used, self.quality.active().id).map_err(GaveUp::log)?;
        // The old surface has to go before a new one is created for the same native window.
        self.surface = None;
        self.steady = false;
        self.recovery.attempted(self.real.elapsed());
        let settings = self.render_settings();
        let (content, options) = (&mut self.content, &self.options);
        let mut lost = false;
        let stack = create_device(platform, &self.window, self.gpu_debug).and_then(|device| {
            let renderer = create_renderer(
                content,
                &mut self.show.fight.show,
                &device.gpu,
                device.format,
                settings,
                options,
            )
            .inspect_err(|_| lost = device.gpu.is_lost())?;
            Ok((device, renderer))
        });
        match stack {
            Ok((device, mut renderer)) => {
                self.losses.built();
                // Field order matters even here: the renderer goes before the old context.
                #[cfg(feature = "dev")]
                self.dev.recreate_gpu(&device.gpu, device.format);
                set_meshes(&mut self.scene, &mut self.show, &mut renderer, content, &device.gpu);
                self.renderer = renderer.renderer;
                #[cfg(feature = "dev")]
                self.dev.renderer_created(&mut self.renderer);
                self.scene.prewarm(&self.show, &mut self.renderer);
                self.graph = GraphExecutor::new();
                self.gpu = device.gpu;
                self.format = device.format;
                self.surface = Some(device.surface);
                // The new renderer's scene starts empty.
                self.scene.resend_instances();
                tracing::info!(target: LOG_TARGET, "GPU stack rebuilt");
            }
            Err(error) if lost => {
                self.losses.lost(&settings, self.quality.active().id, &error).map_err(GaveUp::log)?;
            }
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, retry_in = ?self.recovery.backoff, "GPU stack rebuild failed");
            }
        }
        Ok(())
    }

    /// The renderer settings: the quality tier's, with the command line's overrides, and GTAO off
    /// if [`CreationLosses`] turned it off.
    fn render_settings(&self) -> RenderSettings {
        self.losses.settings(self.options.render_settings(self.quality.render_settings()))
    }

    /// Follows `r.tier`: a new tier gets a new renderer with its settings, on the same device,
    /// as after device loss, and its frame rate. If that fails the old tier and renderer stay; if
    /// it lost the device, it counts with [`CreationLosses`], and the recovery that follows
    /// rebuilds at the old tier. An error when that policy says to stop.
    fn follow_tier(&mut self, platform: &mut Platform) -> Result<(), GaveUp> {
        let Some(previous) = self.quality.update_tier() else { return Ok(()) };
        self.steady = false;
        let settings = self.render_settings();
        match create_renderer(
            &mut self.content,
            &mut self.show.fight.show,
            &self.gpu,
            self.format,
            settings,
            &self.options,
        ) {
            Ok(mut renderer) => {
                self.losses.built();
                set_meshes(&mut self.scene, &mut self.show, &mut renderer, &mut self.content, &self.gpu);
                self.renderer = renderer.renderer;
                #[cfg(feature = "dev")]
                self.dev.renderer_created(&mut self.renderer);
                self.scene.prewarm(&self.show, &mut self.renderer);
                // The new renderer's scene starts empty.
                self.scene.resend_instances();
                tracing::info!(target: LOG_TARGET, tier = %self.quality.active().id, "renderer rebuilt for the quality tier");
            }
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, tier = %self.quality.active().id, "no renderer for the quality tier; keeping the last one");
                if self.gpu.is_lost() {
                    self.losses.lost(&settings, self.quality.active().id, &error).map_err(GaveUp::log)?;
                }
                self.quality.restore(previous);
            }
        }
        self.scene.set_spring_quality(sapphire_stage::animate::spring_quality(self.quality.active().id));
        platform.set_frame_rate(self.quality.frame_rate());
        Ok(())
    }

    /// Follows heat and dynamic resolution's pressure along the tier's thermal ladder (M4 T2) on the
    /// real clock: sheds the rung's passes in place on the renderer, so nothing builds, and paces to
    /// its frame-rate cap. A tier the ladder asks for is logged and left, as a fight would leave it:
    /// taking one belongs to the game, at a pause or a wipe (`r.tier` changes it here).
    fn follow_heat(&mut self, platform: &mut Platform, thermal: &ThermalReading) {
        let step = self.quality.follow_heat(self.real.elapsed(), thermal);
        if step.is_some_and(|step| step.from.frame_rate != step.to.frame_rate) {
            platform.set_frame_rate(self.quality.frame_rate());
        }
        // Every frame rather than on a step, so a renderer made since (a new tier, device loss) sheds too.
        if let Err(error) = self.renderer.set_shed(self.quality.ladder().state().shed) {
            tracing::error!(target: LOG_TARGET, %error, "the thermal ladder's rung");
        }
    }

    /// `--frames N`: exit successfully once N frames were presented, or fail once the run's
    /// deadline passes without getting there.
    fn frame_limit_control(&self) -> Option<AppControl> {
        let limit = self.frame_limit?;
        if self.presented_frames >= limit {
            return Some(AppControl::Exit);
        }
        let deadline = FRAME_LIMIT_MIN_DEADLINE.max(Duration::from_millis(limit.saturating_mul(50)));
        if self.real.elapsed() > deadline {
            tracing::error!(
                target: LOG_TARGET,
                presented = self.presented_frames,
                limit,
                ?deadline,
                "frame limit not reached in time; is the window visible?"
            );
            return Some(AppControl::Fail);
        }
        None
    }

    fn save_pipeline_cache(&self) {
        if let Err(error) = self.gpu.save_pipeline_cache() {
            tracing::warn!(target: LOG_TARGET, %error, "pipeline cache not saved");
        }
    }
}

/// Whether the sandbox trims its caches when `os` sends a memory warning
/// ([`LifecycleEvent::LowMemory`]): only where that means real pressure, iOS (review PA-004).
/// Android's SDL sends one on every trip to the background, which mustn't cost a reload each.
const fn trims_for_memory_warnings(os: Os) -> bool {
    os.memory_warnings_are_pressure()
}

/// When to try rebuilding a missing surface: at once after it was lost or the app returned to
/// the foreground, then with exponential backoff while attempts keep failing.
#[derive(Debug)]
struct SurfaceRecovery {
    next_attempt: Duration,
    backoff: Duration,
}

impl SurfaceRecovery {
    const INITIAL_BACKOFF: Duration = Duration::from_millis(250);
    const MAX_BACKOFF: Duration = Duration::from_secs(4);

    /// Whether an attempt may run at real time `now`.
    fn due(&self, now: Duration) -> bool {
        now >= self.next_attempt
    }

    /// Records an attempt at `now` and backs off the next one.
    fn attempted(&mut self, now: Duration) {
        self.next_attempt = now + self.backoff;
        self.backoff = (self.backoff * 2).min(Self::MAX_BACKOFF);
    }

    /// A frame was presented, or the app returned to the foreground: the next loss retries at once.
    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl Default for SurfaceRecovery {
    fn default() -> Self {
        Self { next_attempt: Duration::ZERO, backoff: Self::INITIAL_BACKOFF }
    }
}

/// What the app does when the GPU device is lost while its GPU stack comes up: while a renderer
/// is made (at startup, while the stack is rebuilt after a loss, or for a new quality tier), or
/// before the stack it was made for presents its first frame (QA-001). A device that is lost
/// while the stack comes up would otherwise be lost again on every attempt, rebuilding forever
/// (the Android emulator lost its device building GTAO's pipelines on Mobile-High, every time).
///
/// - The first loss tries again with the same settings: a loss can be a one-off.
/// - A second in a row tries again with GTAO off, capsule AO only, logged as a warning: the
///   renderer's costliest pipelines to build, and what lost the emulator's device. GTAO stays off
///   for the rest of the run, tier changes included.
/// - A third in a row stops the app with an error.
///
/// Only a presented frame ends the run of losses: a renderer made without a loss doesn't, so a
/// stack that comes up and then loses its device before it shows anything counts too. A loss
/// after the stack has presented starts nothing: the rebuild after it is the first try. Dev and
/// release builds follow the same policy.
#[derive(Debug, Default)]
struct CreationLosses {
    /// Devices lost in a row while stacks came up.
    in_a_row: u32,
    /// GTAO was turned off: two stacks in a row lost the device with it on.
    gtao_off: bool,
    /// The stack in use was made, and has presented nothing yet: a loss of its device counts. Set
    /// back once it presents, or once its loss is counted.
    unproven: bool,
}

impl CreationLosses {
    /// Losses in a row after which the app stops.
    const LIMIT: u32 = 3;

    /// `settings`, with GTAO off if it was turned off.
    fn settings(&self, mut settings: RenderSettings) -> RenderSettings {
        if self.gtao_off && settings.ambient_occlusion.gtao.is_some() {
            settings.ambient_occlusion = AmbientOcclusion { capsules: true, gtao: None };
        }
        settings
    }

    /// Counts a device lost while a renderer with `settings` was made for `tier`, failing with
    /// `error`, and logs what follows: another try (with GTAO off from the second loss in a row),
    /// or none, an error, from the [`Self::LIMIT`]th.
    fn lost(
        &mut self,
        settings: &RenderSettings,
        tier: impl fmt::Display,
        error: &impl fmt::Display,
    ) -> Result<(), GaveUp> {
        self.in_a_row += 1;
        // The device that was lost is the one in use, whose stack this loss covers too.
        self.unproven = false;
        let (losses, ambient_occlusion) = (self.in_a_row, settings.ambient_occlusion);
        if losses >= Self::LIMIT {
            return Err(GaveUp(format!(
                "the GPU device was lost {losses} times in a row before the GPU stack presented a \
                 frame (tier {tier}, ambient occlusion {ambient_occlusion:?}); giving up: {error}"
            )));
        }
        if losses == 2 && ambient_occlusion.gtao.is_some() {
            self.gtao_off = true;
            tracing::warn!(
                target: LOG_TARGET,
                %error,
                %tier,
                "GPU device lost twice in a row before the GPU stack presented a frame; ambient \
                 occlusion downgraded: GTAO off, capsule AO only, for the rest of the run"
            );
        } else {
            tracing::warn!(
                target: LOG_TARGET,
                %error,
                %tier,
                ?ambient_occlusion,
                losses,
                "GPU device lost before the GPU stack presented a frame; trying again"
            );
        }
        Ok(())
    }

    /// The device of the stack in use, made with `settings` for `tier`, was lost: counted as
    /// [`Self::lost`] counts it if the stack hadn't presented a frame yet, once per stack.
    fn stack_lost(&mut self, settings: &RenderSettings, tier: impl fmt::Display) -> Result<(), GaveUp> {
        if !std::mem::take(&mut self.unproven) {
            return Ok(());
        }
        self.lost(settings, tier, &"the GPU device was lost before the stack's first frame")
    }

    /// A renderer was made without losing the device: the stack in use until it presents.
    fn built(&mut self) {
        self.unproven = true;
    }

    /// A frame was presented: the stack in use came up, and the run of losses ends.
    fn presented(&mut self) {
        self.in_a_row = 0;
        self.unproven = false;
    }
}

/// [`CreationLosses`] stopped the app: why.
#[derive(Debug)]
struct GaveUp(String);

impl GaveUp {
    /// Logs why, for a stop after startup.
    fn log(self) -> Self {
        tracing::error!(target: LOG_TARGET, "{}", self.0);
        self
    }
}

impl fmt::Display for GaveUp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The GPU device and the window's surface: built at startup and again after device loss.
struct GpuDevice {
    surface: RenderSurface,
    gpu: GpuContext,
    /// The surface's format, which the renderer and the overlay draw into.
    format: wgpu::TextureFormat,
}

/// A renderer and the meshes it holds: built with the device, and again for a new quality
/// tier.
pub(crate) struct SceneRenderer {
    pub(crate) renderer: Renderer,
    character: MeshHandle,
    character_bounds: [[f32; 3]; 2],
    /// Each party member's mesh, with `--party`.
    party: Option<[MeshHandle; sapphire_gameplay::PARTY_SLOTS]>,
    /// The skinned stalk, with `--skinned`.
    stalk: Option<MeshHandle>,
    /// The shader swatches, with `--swatches`.
    swatches: Option<[MeshHandle; SWATCHES]>,
    /// The material layer samples, with `--materials`.
    layer_samples: Option<[MeshHandle; layers::SAMPLES]>,
    /// The garment samples, with `--two-sided`.
    garments: Option<[MeshHandle; garments::SAMPLES]>,
    /// The face test head, with `--faces`.
    face_head: Option<MeshHandle>,
    /// The rim rig's figure and backdrop, with `--rim-rig`.
    rim_rig: Option<RimRigMeshes>,
    /// The courtyard, with `--environment`.
    courtyard: Option<MeshHandle>,
    /// The fog cards, with `--environment` or `--look-dev`, and fog.
    fog_cards: Option<[MeshHandle; FOG_CARDS.len()]>,
    /// The plants, with `--foliage`.
    foliage: Option<FoliageMeshes>,
    /// The arcade, with `--probes`, whose probe grid the renderer holds.
    arcade: Option<MeshHandle>,
    /// The hard-edged outline sample, with `--hard-edges`.
    hard_edges: Option<MeshHandle>,
    /// The look-dev town and its plants, with `--look-dev`.
    town: Option<TownMeshes>,
    /// The enemy, with `--look-dev enemy` or `--arena`.
    enemy: Option<EnemyMeshes>,
    /// The player's sword, with `--blade on`.
    sword: Option<MeshHandle>,
    /// The stage's floor and pillar, with `--stage`.
    stage: Option<StudioMeshes>,
    /// The stone block IK's ground is built of, with `--ik on`.
    ik_block: Option<MeshHandle>,
    /// The zone's sky material.
    sky: MaterialHandle,
}

/// Draws the meshes of a new `renderer`: after device loss or for a new quality tier.
pub(crate) fn set_meshes(
    scene: &mut Scene,
    show: &mut ShownFight,
    renderer: &mut SceneRenderer,
    content: &mut Content,
    gpu: &GpuContext,
) {
    scene.set_zone_meshes(content, &mut renderer.renderer, gpu);
    scene.set_character_mesh(renderer.character);
    if let Some(meshes) = renderer.party {
        show.fight.show.set_party_meshes(scene, meshes);
    }
    scene.set_sky_material(renderer.sky);
    if let Some(stalk) = renderer.stalk {
        scene.set_stalk_mesh(stalk);
    }
    if let Some(meshes) = renderer.swatches {
        scene.set_swatch_meshes(meshes);
    }
    if let Some(meshes) = renderer.layer_samples {
        scene.set_layer_sample_meshes(meshes);
    }
    if let Some(meshes) = renderer.garments {
        scene.set_garment_meshes(meshes);
    }
    if let Some(head) = renderer.face_head {
        scene.set_face_mesh(head);
    }
    if let Some(meshes) = renderer.rim_rig {
        scene.set_rim_rig_meshes(meshes);
    }
    if let Some(courtyard) = renderer.courtyard {
        scene.set_courtyard_mesh(courtyard);
    }
    if let Some(meshes) = renderer.fog_cards {
        scene.set_fog_card_meshes(meshes);
    }
    if let Some(meshes) = renderer.foliage {
        scene.set_foliage_meshes(meshes);
    }
    if let Some(arcade) = renderer.arcade {
        scene.set_arcade_mesh(arcade);
    }
    scene.set_outline_row_mesh(renderer.character);
    if let Some(mesh) = renderer.hard_edges {
        scene.set_hard_edges_mesh(mesh);
    }
    if let Some(meshes) = renderer.stage {
        scene.set_studio_meshes(meshes);
    }
    if let Some(block) = renderer.ik_block {
        scene.set_ik_block_mesh(block);
    }
    if let Some(town) = renderer.town {
        scene.set_town_mesh(town);
    }
    if let Some(meshes) = renderer.enemy {
        show.fight.show.set_enemy_meshes(scene, meshes);
    }
    if let Some(sword) = renderer.sword {
        show.fight.show.set_blade_mesh(scene, sword);
    }
}

fn create_device(platform: &Platform, window: &Window, debug: GpuDebug) -> Result<GpuDevice, AppInitError> {
    let instance = GpuInstance::new(&GpuInstanceDesc { debug, ..GpuInstanceDesc::default() })?;
    let handle = window.surface_handle()?;
    let target = surface_target(&handle)?;
    // SAFETY: the surface owns `handle`, retaining the native window. App state drops the
    // surface before the SDL run ends; the handle cannot extend that run.
    let mut surface = unsafe { RenderSurface::new(&instance, target, handle)? };
    let pipeline_cache = PipelineCacheDesc {
        dir: platform.cache_dir().join("pipeline_cache"),
        build_id: SandboxApp::METADATA.version.to_owned(),
    };
    let desc = GpuContextDesc { pipeline_cache: Some(pipeline_cache), ..GpuContextDesc::default() };
    let gpu = GpuContext::new(&instance, &desc, Some(&surface))?;
    tracing::info!(target: LOG_TARGET, caps = ?gpu.caps(), "GPU ready");

    let size = window.pixel_size();
    surface.configure(&gpu, size.width, size.height, PresentPolicy::Fifo)?;
    let format = surface.format().ok_or_else(|| AppInitError::msg("configured surface reports no format"))?;
    Ok(GpuDevice { surface, gpu, format })
}

/// What [`create_renderer`] uploads beside the character, as [`SceneRenderer`] holds it.
struct SceneMeshes {
    stalk: Option<MeshHandle>,
    swatches: Option<[MeshHandle; SWATCHES]>,
    layer_samples: Option<[MeshHandle; layers::SAMPLES]>,
    garments: Option<[MeshHandle; garments::SAMPLES]>,
    face_head: Option<MeshHandle>,
    rim_rig: Option<RimRigMeshes>,
    courtyard: Option<MeshHandle>,
    fog_cards: Option<[MeshHandle; FOG_CARDS.len()]>,
    foliage: Option<FoliageMeshes>,
    arcade: Option<MeshHandle>,
    hard_edges: Option<MeshHandle>,
    town: Option<TownMeshes>,
    enemy: Option<EnemyMeshes>,
    sword: Option<MeshHandle>,
    stage: Option<StudioMeshes>,
    ik_block: Option<MeshHandle>,
}

/// A renderer drawing into `format` targets with `settings`, holding the character, which
/// `content` uploads to it from the start, and what `options` add beside it: the skinned stalk,
/// the shader swatches and the material layer samples, whose materials `content` uploads too,
/// the garment samples, the face test head, the rim rig, the courtyard, its fog cards, arcade and probe grid, the
/// plants, the hard-edged outline sample, the stage and the look-dev town. Fails if the device is
/// lost while the renderer is made or the assets uploaded (`gpu.is_lost()` then says so), rather
/// than hand back a renderer of dead objects.
pub(crate) fn create_renderer(
    content: &mut Content,
    fight: &mut FightShow,
    gpu: &GpuContext,
    format: wgpu::TextureFormat,
    settings: RenderSettings,
    options: &Options,
) -> Result<SceneRenderer, AppInitError> {
    let mut renderer = Renderer::new(gpu, format, settings)?;
    // Before any upload, so nothing the packs list builds on a first use (M3 K1).
    renderer.prewarm(&content.pipeline_inventory());
    // A new renderer starts empty, so every asset goes to it again. If it fails, the content goes
    // back to the renderer the app keeps.
    let previous = content.renderer_rebuilt();
    let scene = content.character(&mut renderer, gpu).and_then(|(character, character_bounds)| {
        let stalk = options.skinned.then(|| skinned::upload_stalk(&mut renderer)).transpose()?;
        let swatches =
            options.swatches.then(|| swatches::upload_swatches(content, gpu, &mut renderer)).transpose()?;
        let layer_samples =
            options.materials.then(|| layers::upload_samples(content, gpu, &mut renderer)).transpose()?;
        let garments = options.two_sided.then(|| garments::upload_samples(&mut renderer)).transpose()?;
        let face_head = options.faces.map(|_| content.face_head(&mut renderer, gpu)).transpose()?;
        let rim_rig = options.rim_rig.map(|_| content.rim_rig_meshes(&mut renderer)).transpose()?;
        let courtyard = options.environment.then(|| content.courtyard(&mut renderer, gpu)).transpose()?;
        let fog_cards = ((options.environment || options.in_town()) && !options.fog_off)
            .then(|| content.fog_cards(&mut renderer, gpu))
            .transpose()?;
        let foliage = options.foliage.map(|_| content.foliage(&mut renderer, gpu)).transpose()?;
        let arcade = options.probes.then(|| content.probes(&mut renderer, gpu)).transpose()?;
        let hard_edges = options.hard_edges.then(|| content.hard_edges(&mut renderer, gpu)).transpose()?;
        let town = options.in_town().then(|| content.town(&mut renderer, gpu)).transpose()?;
        let enemy = ((options.look_dev == Some(Shot::Enemy) && options.model.is_none())
            || options.arena.is_some())
        .then(|| fight.load_enemy(content, &mut renderer, gpu))
        .transpose()?;
        let sword = options.blade().then(|| fight.load_sword(content, &mut renderer, gpu)).transpose()?;
        let stage = options.stage.map(|_| content.studio_meshes(&mut renderer)).transpose()?;
        let ik_block = options
            .ik
            .as_ref()
            .filter(|_| options.arena.is_none())
            .map(|_| studio::upload_block(&mut renderer))
            .transpose()?;
        let sky = content.sky_material(&mut renderer, gpu)?;
        let meshes = SceneMeshes {
            stalk,
            swatches,
            layer_samples,
            garments,
            face_head,
            rim_rig,
            courtyard,
            fog_cards,
            foliage,
            arcade,
            hard_edges,
            town,
            enemy,
            sword,
            stage,
            ik_block,
        };
        Ok((character, character_bounds, meshes, sky))
    });
    // Uploads to a lost device do nothing; the renderer would hold dead objects (QA-001).
    let scene = scene.and_then(|scene| {
        if gpu.is_lost() {
            return Err(AppInitError::msg("the GPU device was lost while the assets were uploaded"));
        }
        Ok(scene)
    });
    match scene {
        Ok((character, character_bounds, meshes, sky)) => {
            let SceneMeshes {
                stalk,
                swatches,
                layer_samples,
                garments,
                face_head,
                rim_rig,
                courtyard,
                fog_cards,
                foliage,
                arcade,
                hard_edges,
                town,
                enemy,
                sword,
                stage,
                ik_block,
            } = meshes;
            Ok(SceneRenderer {
                renderer,
                character,
                character_bounds,
                party: FightShow::party_meshes(content, character),
                stalk,
                swatches,
                layer_samples,
                garments,
                face_head,
                rim_rig,
                courtyard,
                fog_cards,
                foliage,
                arcade,
                hard_edges,
                town,
                enemy,
                sword,
                stage,
                ik_block,
                sky,
            })
        }
        Err(error) => {
            content.keep_renderer(previous);
            Err(error)
        }
    }
}

/// The renderer's surface target for a window's surface handle: its `CAMetalLayer` on
/// Apple platforms, raw native handles elsewhere.
fn surface_target(handle: &SurfaceHandle) -> Result<SurfaceTarget, HandleError> {
    if let Some(layer) = handle.metal_layer() {
        return Ok(SurfaceTarget::CoreAnimationLayer(layer));
    }
    Ok(SurfaceTarget::Raw { window: handle.raw_window_handle()?, display: handle.raw_display_handle()? })
}

#[cfg(test)]
mod pause_tests;
#[cfg(test)]
mod touch_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_matches_the_identity_file() {
        // xtask bundles and packages the app from platform/identity.toml; the identity SDL sees at runtime
        // (user directories, iOS bundle checks, Linux's window class) must be the same one.
        let identities = include_str!("../../../platform/identity.toml");
        let table = identities
            .split("\n[")
            .find(|table| table.starts_with("sapphire_sandbox]"))
            .expect("[sapphire_sandbox]");
        let field = |key: &str| {
            table
                .lines()
                .find_map(|line| {
                    let value = line.split_once(key)?.1.trim_start().strip_prefix('=')?.trim_start();
                    value.strip_prefix('"')?.split('"').next()
                })
                .unwrap_or_else(|| panic!("`{key}` missing from [sapphire_sandbox]"))
        };
        assert_eq!(SandboxApp::METADATA.identifier, field("bundle-id"));
        assert_eq!(SandboxApp::METADATA.name, field("{ en"), "the English display name");
    }

    /// A desktop's fight, played, unpaused and focused, with no overlay up: take any of those away and
    /// the pointer goes back.
    #[test]
    fn only_a_played_fight_on_a_desktop_holds_the_pointer() {
        for bits in 0..32_u32 {
            let bit = |at: u32| bits & (1 << at) != 0;
            let (desktop, played, paused, focused, overlay) = (bit(0), bit(1), bit(2), bit(3), bit(4));
            let state = PointerState { desktop, played, paused, focused, overlay };
            assert_eq!(state.holds(), desktop && played && !paused && focused && !overlay, "{state:?}");
        }
    }

    /// The overlay is a dev tool over a fight someone's still playing: it can't wait for a pause.
    #[test]
    fn the_overlay_wins_the_pointer() {
        let playing =
            PointerState { desktop: true, played: true, paused: false, focused: true, overlay: false };
        assert!(playing.holds());
        assert!(!PointerState { overlay: true, ..playing }.holds());
    }

    /// Review PA-004: memory warnings trim the asset cache on iOS, and nowhere else: Android
    /// sends one on every trip to the background.
    #[test]
    fn memory_warnings_trim_on_ios_only() {
        assert!(trims_for_memory_warnings(Os::Ios));
        for os in [Os::Android, Os::MacOs, Os::Windows, Os::Linux] {
            assert!(!trims_for_memory_warnings(os), "{os:?}");
        }
    }

    #[test]
    fn surface_recovery_backs_off_until_a_frame_is_presented() {
        let mut recovery = SurfaceRecovery::default();
        let at = Duration::from_millis;
        assert!(recovery.due(at(0)));

        let mut now = at(1000);
        let mut gaps = Vec::new();
        for _ in 0..7 {
            assert!(recovery.due(now));
            recovery.attempted(now);
            gaps.push(recovery.next_attempt - now);
            assert!(!recovery.due(now + gaps.last().unwrap().saturating_sub(at(1))));
            now = recovery.next_attempt;
        }
        assert_eq!(gaps, [250, 500, 1000, 2000, 4000, 4000, 4000].map(at));

        recovery.reset();
        assert!(recovery.due(now));
        recovery.attempted(now);
        assert_eq!(recovery.next_attempt - now, at(250));
    }

    /// Mobile-High's settings: capsule AO and GTAO.
    fn with_gtao() -> RenderSettings {
        let ambient_occlusion =
            AmbientOcclusion { capsules: true, gtao: Some(sapphire_render::GtaoSettings::DESKTOP) };
        RenderSettings { ambient_occlusion, ..RenderSettings::default() }
    }

    #[test]
    fn creation_losses_retry_then_turn_gtao_off_then_stop() {
        let mut losses = CreationLosses::default();
        let tier = "mobile-high";
        assert_eq!(losses.settings(with_gtao()), with_gtao(), "nothing lost yet");

        // The first loss tries again as it was.
        let settings = losses.settings(with_gtao());
        assert!(losses.lost(&settings, tier, &"lost").is_ok());
        assert_eq!(losses.settings(with_gtao()), with_gtao());
        // The second in a row turns GTAO off, keeping capsule AO.
        assert!(losses.lost(&settings, tier, &"lost").is_ok());
        let downgraded = losses.settings(with_gtao());
        assert_eq!(downgraded.ambient_occlusion, AmbientOcclusion { capsules: true, gtao: None });
        assert_eq!(
            RenderSettings { ambient_occlusion: with_gtao().ambient_occlusion, ..downgraded },
            with_gtao()
        );
        // The third in a row stops, saying why.
        let error = losses.lost(&downgraded, tier, &"lost again").unwrap_err().to_string();
        assert!(error.contains("3 times in a row") && error.contains("mobile-high"), "{error}");
        assert!(error.contains("lost again"), "{error}");
    }

    #[test]
    fn a_presented_frame_ends_the_run_of_losses_but_gtao_stays_off() {
        let mut losses = CreationLosses::default();
        for _ in 0..2 {
            assert!(losses.lost(&with_gtao(), "mobile-high", &"lost").is_ok());
        }
        // A renderer made doesn't end the run; its first frame does.
        losses.built();
        losses.presented();
        // Two more losses in a row are a new run: no stop yet.
        let settings = losses.settings(with_gtao());
        for _ in 0..2 {
            assert!(losses.lost(&settings, "mobile-high", &"lost").is_ok());
        }
        assert!(losses.lost(&settings, "mobile-high", &"lost").is_err());
        // GTAO stays off from the first downgrade on, for every tier that has it.
        assert_eq!(losses.settings(with_gtao()).ambient_occlusion.gtao, None);
        // Tiers without GTAO are as they were; and with GTAO already off, the second loss in a row
        // has nothing to turn off, and tries again as it was.
        let plain = RenderSettings::default();
        assert_eq!(losses.settings(plain), plain);
        let mut losses = CreationLosses::default();
        for _ in 0..2 {
            assert!(losses.lost(&plain, "mobile-low", &"lost").is_ok());
        }
        assert!(!losses.gtao_off);
        assert!(losses.lost(&plain, "mobile-low", &"lost").is_err());
    }

    #[test]
    fn a_loss_before_the_first_frame_counts_once_per_stack() {
        let tier = "mobile-high";
        // A stack that presented and then lost its device: the rebuild is a first try.
        let mut losses = CreationLosses::default();
        losses.built();
        losses.presented();
        assert!(losses.stack_lost(&with_gtao(), tier).is_ok());
        assert_eq!(losses.in_a_row, 0);

        // Made, then lost before its first frame: counted, once however often recovery runs.
        losses.built();
        assert!(losses.stack_lost(&with_gtao(), tier).is_ok());
        assert_eq!(losses.in_a_row, 1);
        assert!(losses.stack_lost(&with_gtao(), tier).is_ok());
        assert_eq!(losses.in_a_row, 1, "one loss per stack");
        // Then the rebuild comes up and loses its device before a frame too: the second in a row
        // turns GTAO off.
        losses.built();
        assert!(losses.stack_lost(&with_gtao(), tier).is_ok());
        assert_eq!(losses.settings(with_gtao()).ambient_occlusion.gtao, None);
        // A loss while making the next counts as the third, and stops.
        let settings = losses.settings(with_gtao());
        let error = losses.lost(&settings, tier, &"lost").unwrap_err().to_string();
        assert!(error.contains("3 times in a row"), "{error}");

        // A loss counted while making a renderer (a new tier's, on the device in use) covers the
        // stack in use too: the recovery that follows doesn't count it again.
        let mut losses = CreationLosses::default();
        losses.built();
        assert!(losses.lost(&with_gtao(), tier, &"lost").is_ok());
        assert!(losses.stack_lost(&with_gtao(), tier).is_ok());
        assert_eq!(losses.in_a_row, 1);
    }
}
