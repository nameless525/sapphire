//! The window: its surface, its clocks and the frame loop around a stage.

use std::time::Duration;

use game_combat::cues::PadRumble;
use game_combat::fight::fight_stage;
use game_flow::show::GameShow;
use game_ui::menus::PlayerSettings;
use sapphire_platform::raw_window_handle::HandleError;
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, LifecycleEvent, Os, Platform,
    SurfaceHandle, Window, WindowDesc, WindowEvent, durable_replace,
};
use sapphire_render::quality::QualitySettings;
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuDebug, GpuInstance, GpuInstanceDesc, PipelineCacheDesc, PresentPolicy,
    RenderSurface, SkipReason, SurfaceAcquire, SurfaceTarget,
};
use sapphire_stage::audio::DeviceAudio;
use sapphire_stage::{Content, Stage, StageDesc};
use sapphire_time::{FixedStepConfig, FixedStepDriver, RealClock, ScaleHandle, ScaleSource};

use crate::LOG_TARGET;
use crate::options::Options;
use crate::settings::KeptSettings;

/// Loop period while nothing can be presented (occluded, minimized, backgrounded).
const IDLE_FRAME: Duration = Duration::from_millis(33);
/// Longest real-time step presentation takes in one frame, so a hitch can't fling the camera.
const MAX_PRESENTATION_DT: Duration = Duration::from_millis(100);
/// How long a window with no surface waits before trying to make one again.
const SURFACE_RETRY: Duration = Duration::from_secs(1);

/// See this module.
pub struct GameApp {
    /// Before the stage and the window: it's on the stage's device, and holds the window's native handle.
    surface: Option<RenderSurface>,
    stage: Stage<GameShow>,
    audio: Option<DeviceAudio>,
    settings: KeptSettings,
    /// Whether a pad may shake.
    rumble: bool,
    window: Window,
    /// A device whose player has more than a screen to touch.
    desktop: bool,
    real: RealClock,
    driver: FixedStepDriver,
    /// Holds the world's clock at zero while backgrounded.
    background_pause: Option<ScaleHandle>,
    suspended: bool,
    /// Whether the fight has the pointer.
    pointer_held: bool,
    /// When a window with no surface next tries for one.
    surface_due: Duration,
}

impl App for GameApp {
    const METADATA: AppMetadata = AppMetadata {
        name: "Sapphire",
        organization: "Sapphire",
        identifier: "dev.sapphire.game",
        version: env!("CARGO_PKG_VERSION"),
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let options = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        let info = *platform.system_info();
        tracing::info!(target: LOG_TARGET, version = Self::METADATA.version, ?info, ?options, "starting");
        let window = platform
            .create_window(&WindowDesc { title: Self::METADATA.name.to_owned(), ..WindowDesc::default() })?;

        // Narrow the build's defaults, never widen them: the Android emulator's Vulkan driver (gfxstream)
        // crashes naming objects, so labels go off there and only there.
        let base = GpuDebug::default();
        let debug = GpuDebug {
            object_labels: base.object_labels && !(info.is_emulator && info.os == Os::Android),
            ..base
        };
        let instance = GpuInstance::new(&GpuInstanceDesc { debug, ..GpuInstanceDesc::default() })?;
        let handle = window.surface_handle()?;
        let target = surface_target(&handle)?;
        // SAFETY: this app owns the surface and drops it before SDL ends the run. The surface
        // owns `handle`, which keeps the native window alive within that run, and drops it right after
        // itself.
        let mut surface = unsafe { RenderSurface::new(&instance, target, handle)? };
        let pipeline_cache = PipelineCacheDesc {
            dir: platform.cache_dir().join("pipeline_cache"),
            build_id: Self::METADATA.version.to_owned(),
        };
        let desc = GpuContextDesc { pipeline_cache: Some(pipeline_cache), ..GpuContextDesc::default() };
        let gpu = GpuContext::new(&instance, &desc, Some(&surface))?;
        tracing::info!(target: LOG_TARGET, caps = ?gpu.caps(), "GPU ready");
        let size = window.pixel_size();
        surface.configure(&gpu, size.width, size.height, PresentPolicy::Fifo)?;
        let format =
            surface.format().ok_or_else(|| AppInitError::msg("configured surface reports no format"))?;

        let desc = fight_stage(StageDesc::new(platform.host(), &gpu, format, QualitySettings::default()));
        let (settings, chosen) = KeptSettings::load(platform.user_dir(), durable_replace);
        let rumble = chosen.rumble;
        let volume = chosen.volume();
        let voice = chosen.locale().voice;
        let mut content = Content::new(platform.host(), desc.character, desc.sky)?;
        let (mut show, first) = options.show(platform, chosen)?;
        let combat = show.shown.fight.prepare_stock_combat(&first, &mut content)?;
        let mut stage = Stage::new_with_content(gpu, desc, content)?.with_show(show);
        // Frames are paced to the tier's rate (spec §6.2).
        platform.set_frame_rate(stage.quality.frame_rate());
        let window_size = window.size();
        stage.show.controls.input_mut().set_window_size(window_size.width as f32, window_size.height as f32);
        let Stage { show, scene, content, renderer, gpu, quality, .. } = &mut stage;
        show.stand(combat, scene, content, renderer, gpu, quality.active().id)?;
        let mut audio = match show.shown.fight.show.cue_show() {
            Some(cues) => DeviceAudio::for_show(&cues.sounds_to_load(), platform, content, renderer, gpu)?,
            None => None,
        };
        if let Some(audio) = &mut audio {
            audio.set_volume(volume);
            speak_in(audio, &voice);
        }
        if let Some(command) = options.choose {
            show.choose(command);
        } else {
            tracing::info!(target: LOG_TARGET, "the title is up");
        }
        Ok(Self {
            surface: Some(surface),
            stage,
            audio,
            settings,
            rumble,
            window,
            desktop: info.device_class == DeviceClass::Desktop,
            real: RealClock::new(),
            driver: FixedStepDriver::new(FixedStepConfig::default()),
            background_pause: None,
            suspended: false,
            pointer_held: false,
            surface_due: Duration::ZERO,
        })
    }

    fn event(&mut self, platform: &mut Platform, event: &Event) -> AppControl {
        self.stage.show.handle_event(event);
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
                // Whoever's holding the pad isn't watching any more, and a desktop's fight waits for them.
                WindowEvent::FocusLost => {
                    self.stop_rumble(platform);
                    if self.desktop && self.played() {
                        self.stage.show.controls.set_paused(true);
                    }
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
        // Where vsyncs start frames, the frame's time is its interval in whole vsyncs, so steady frames run
        // whole fixed steps whatever the scheduler's jitter.
        let frame_time = platform.frame_delta().unwrap_or(measured);
        if self.stage.show.read_input(&self.window, self.desktop, platform.now()) {
            return AppControl::Exit;
        }
        if let Some(chosen) = self.stage.show.take_changed_settings() {
            self.follow_settings(&chosen, platform);
        }
        self.settings.poll();
        self.follow_pointer();
        if self.suspended {
            std::thread::sleep(IDLE_FRAME);
            return AppControl::Continue;
        }
        if self.stage.gpu.is_lost() {
            tracing::error!(target: LOG_TARGET, "the GPU device was lost, and the game can't rebuild on a new one yet");
            return AppControl::Fail;
        }
        if self.surface.is_none() && self.real.elapsed() >= self.surface_due {
            self.rebuild_surface();
        }

        let stage = &mut self.stage;
        let steps = stage.show.advance(&stage.scene, &mut self.driver, frame_time);
        for _ in 0..steps.count {
            stage.fixed_step();
        }

        // Presentation runs on the real clock, so the camera stays responsive in slow motion.
        let dt = frame_time.min(MAX_PRESENTATION_DT);
        let view_dt = stage.show.drive_view(dt);
        let dt = dt.as_secs_f32();
        if let Some(audio) = &mut self.audio {
            audio.update(dt, platform, stage.scene.sound(), stage.show.shown.fight.show.cue_sounds());
        }
        let pad = stage.show.controls.input().active_gamepad().filter(|_| self.rumble);
        stage.show.shown.fight.show.end_cue_frame(
            &mut stage.scene,
            dt,
            view_dt,
            steps.alpha,
            &mut PadRumble { platform, pad },
        );

        let presented = match self.render(steps.alpha) {
            Ok(presented) => presented,
            Err(error) => {
                // The graph is built in code, so it would fail the same way every frame.
                tracing::error!(target: LOG_TARGET, %error, "the frame can't be drawn");
                return AppControl::Fail;
            }
        };
        if presented {
            // Rendering works with this pipeline cache, so the next boot may trust it.
            self.stage.gpu.confirm_pipeline_cache();
        } else {
            // Nothing blocked on vsync this frame, so pace the loop here instead of spinning a core.
            std::thread::sleep(IDLE_FRAME);
        }
        self.stage.scene.end_frame();
        profiling::finish_frame!();
        AppControl::Continue
    }

    fn quit(&mut self, _platform: &mut Platform) {
        self.save_pipeline_cache();
        self.settings.finish();
        // The run's file, if it keeps one, and the save's last write.
        if let Err(error) = self.stage.show.finish() {
            tracing::error!(target: LOG_TARGET, %error, "the run didn't end well");
        }
        tracing::info!(target: LOG_TARGET, "quit");
    }
}

impl GameApp {
    /// Someone's playing the fight, not a script.
    fn played(&self) -> bool {
        let show = &self.stage.show;
        !show.on_title() && show.shown.fight.combat.as_ref().is_some_and(|combat| combat.is_played())
    }

    /// A fight someone's playing at a desktop holds the pointer: the mouse turns its camera, which a cursor
    /// reaching the screen's edge would stop. A pause gives it back, once for each change.
    fn follow_pointer(&mut self) {
        let paused = self.stage.show.controls.is_paused();
        let held = self.desktop && self.played() && self.window.has_keyboard_focus() && !paused;
        if held != self.pointer_held {
            self.pointer_held = held;
            self.window.set_relative_mouse(held);
        }
    }

    /// What the player has just chosen: heard now, and kept for the next run. The language and size have
    /// reached the screens already.
    fn follow_settings(&mut self, chosen: &PlayerSettings, platform: &mut Platform) {
        if let Some(audio) = &mut self.audio {
            audio.set_volume(chosen.volume());
            speak_in(audio, &chosen.locale().voice);
        }
        if self.rumble && !chosen.rumble {
            self.stop_rumble(platform);
        }
        self.rumble = chosen.rumble;
        self.settings.keep(chosen);
    }

    fn stop_rumble(&mut self, platform: &mut Platform) {
        let pad = self.stage.show.controls.input().active_gamepad();
        self.stage.show.shown.fight.show.stop_rumble(&mut PadRumble { platform, pad });
    }

    /// Draws and presents the frame if the surface gave an image. Every frame is prepared, drawn or not, so
    /// the renderer's persistent scene sees every packet's deltas. Returns whether one was presented.
    fn render(&mut self, alpha: f32) -> Result<bool, AppInitError> {
        profiling::scope!("render");
        let skipped = match self.surface.as_mut().map(RenderSurface::acquire) {
            Some(SurfaceAcquire::Frame(frame)) => {
                self.stage.render(frame.view(), alpha, |_, _| {})?;
                frame.present();
                self.stage.show.say_first_uses(&self.stage.renderer);
                return Ok(true);
            }
            Some(SurfaceAcquire::Skip(reason)) => Some(reason),
            None => None,
        };
        self.stage.skip_frame(alpha);
        // `Lost` is sticky until the surface is rebuilt from a fresh native handle. `DeviceLost` is the next
        // frame's to end the run on.
        if skipped == Some(SkipReason::Lost) {
            self.surface = None;
        }
        Ok(false)
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(surface) = &mut self.surface {
            surface.resize(width, height);
        }
    }

    /// Spec §6.5: no GPU work in the background. Android also destroys the native window, so the surface is
    /// dropped going down and rebuilt coming back; elsewhere it is kept.
    fn lifecycle(&mut self, event: LifecycleEvent, platform: &mut Platform) {
        tracing::info!(target: LOG_TARGET, ?event, "lifecycle");
        match event {
            LifecycleEvent::WillEnterBackground => {
                self.suspended = true;
                self.stop_rumble(platform);
                if let Some(audio) = &mut self.audio {
                    audio.set_background(true, platform);
                }
                if !Os::CURRENT.keeps_surface_in_background() {
                    self.surface = None;
                }
                if self.background_pause.is_none() {
                    self.background_pause =
                        self.stage.scene.world_clock_mut().0.push_scale(ScaleSource::Pause, 0).ok();
                }
                self.save_pipeline_cache();
            }
            LifecycleEvent::DidEnterForeground => {
                // `iterate` doesn't run while suspended on Android (or after a real iOS background), so the
                // next frame would otherwise see the whole pause as one frame.
                self.real.update();
                self.suspended = false;
                if let Some(audio) = &mut self.audio {
                    audio.set_background(false, platform);
                }
                if let Some(handle) = self.background_pause.take() {
                    self.stage.scene.world_clock_mut().0.pop_scale(handle);
                }
                if self.surface.is_none() {
                    self.rebuild_surface();
                }
            }
            LifecycleEvent::Terminating => self.save_pipeline_cache(),
            LifecycleEvent::LowMemory
            | LifecycleEvent::DidEnterBackground
            | LifecycleEvent::WillEnterForeground => {}
        }
    }

    /// Makes the surface again from a freshly fetched native handle. On failure the app keeps running
    /// without one and tries again in a while.
    fn rebuild_surface(&mut self) {
        self.surface_due = self.real.elapsed() + SURFACE_RETRY;
        let size = self.window.pixel_size();
        let gpu = &self.stage.gpu;
        let result = self.window.surface_handle().map_err(|error| error.to_string()).and_then(|handle| {
            let target = surface_target(&handle).map_err(|error| error.to_string())?;
            // SAFETY: this app owns the surface and drops it before SDL ends the run. The surface
            // owns `handle`, which keeps the native window alive within that run, and drops it right
            // after itself.
            let mut surface = unsafe { RenderSurface::new(gpu.instance(), target, handle) }
                .map_err(|error| error.to_string())?;
            surface
                .configure(gpu, size.width, size.height, PresentPolicy::Fifo)
                .map_err(|error| error.to_string())?;
            Ok(surface)
        });
        match result {
            Ok(surface) => {
                self.surface = Some(surface);
                tracing::info!(target: LOG_TARGET, width = size.width, height = size.height, "surface rebuilt");
            }
            Err(error) => tracing::warn!(target: LOG_TARGET, %error, "surface rebuild failed"),
        }
    }

    fn save_pipeline_cache(&self) {
        if let Err(error) = self.stage.gpu.save_pipeline_cache() {
            tracing::warn!(target: LOG_TARGET, %error, "pipeline cache not saved");
        }
    }
}

/// Speaks the fight's lines in `language`, the player's: a language the fight has no lines in speaks none,
/// and says so once here.
fn speak_in(audio: &mut DeviceAudio, language: &str) {
    if !audio.set_voice_language(language) {
        tracing::info!(target: LOG_TARGET, language, "the fight has no voice lines in the chosen language");
    }
}

fn surface_target(handle: &SurfaceHandle) -> Result<SurfaceTarget, HandleError> {
    if let Some(layer) = handle.metal_layer() {
        return Ok(SurfaceTarget::CoreAnimationLayer(layer));
    }
    Ok(SurfaceTarget::Raw { window: handle.raw_window_handle()?, display: handle.raw_display_handle()? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_matches_the_package_manifest() {
        // xtask packages the app from `[package.metadata.sapphire]`; the identity SDL sees at runtime (user
        // directories, iOS bundle checks) must be the same one.
        let manifest = include_str!("../Cargo.toml");
        let field = |key: &str| {
            manifest
                .lines()
                .find_map(|line| {
                    let value = line.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
                    value.strip_prefix('"')?.strip_suffix('"')
                })
                .unwrap_or_else(|| panic!("`{key}` missing from [package.metadata.sapphire]"))
        };
        assert_eq!(GameApp::METADATA.identifier, field("bundle-id"));
        assert_eq!(GameApp::METADATA.name, field("display-name"));
    }
}
