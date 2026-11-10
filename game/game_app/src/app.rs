//! The window: its surface, its clocks and the frame loop around a stage.
//!
//! **The OS moving the app (spec §6.5, M4 S6)** is [`duties`]' table, in order: going down, a run someone's
//! playing pauses and what's on its way to disk gets there before the audio falls silent; coming back, the
//! pause's screen is what's drawn, and the player's tap on it is what goes on.
//!
//! **A lost device** takes the stage with it. The show ends as a quit ends it, writing the run's file and the
//! save; the device, its surface and a stage are made again, and the game stands on them where the save has it,
//! waiting at the pause if someone was playing. A device lost again before the next stage presents a frame
//! counts toward [`Losses`]' limit; the audio, the settings and the window go on through it.

use std::time::Duration;

use game_combat::cues::PadRumble;
use game_flow::show::{GameShow, Reload};
use game_ui::menus::PlayerSettings;
use sapphire_platform::raw_window_handle::HandleError;
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, LifecycleEvent, Os, Platform,
    SurfaceHandle, Window, WindowDesc, WindowEvent, durable_replace,
};
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuDebug, GpuInstance, GpuInstanceDesc, PipelineCacheDesc, PresentPolicy,
    RenderSurface, SkipReason, SurfaceAcquire, SurfaceTarget,
};
use sapphire_stage::Stage;
use sapphire_stage::audio::DeviceAudio;
use sapphire_time::{FixedStepConfig, FixedStepDriver, RealClock};

use crate::LOG_TARGET;
use crate::options::Options;
use crate::settings::KeptSettings;

/// Loop period while nothing can be presented (occluded, minimized, backgrounded).
const IDLE_FRAME: Duration = Duration::from_millis(33);
/// Longest real-time step presentation takes in one frame, so a hitch can't fling the camera.
const MAX_PRESENTATION_DT: Duration = Duration::from_millis(100);
/// How long a window with no surface, or a game with no stage, waits before trying to make one again.
const RETRY: Duration = Duration::from_secs(1);

/// See this module.
pub struct GameApp {
    /// Before the window, whose native handle its surface holds. None from a lost device until the game stands
    /// on the next.
    on_device: Option<OnDevice>,
    /// Where the game goes back to once it stands on the next device.
    reload: Option<Reload>,
    losses: Losses,
    /// Outlives a lost device: what it plays was read from the content, and nothing of it is on the GPU.
    audio: Option<DeviceAudio>,
    settings: KeptSettings,
    /// Whether a pad may shake.
    rumble: bool,
    window: Window,
    /// A device whose player has more than a screen to touch.
    desktop: bool,
    options: Options,
    real: RealClock,
    driver: FixedStepDriver,
    suspended: bool,
    /// Whether the fight has the pointer.
    pointer_held: bool,
    /// When a missing surface or stage is next tried for.
    retry_due: Duration,
    /// Frames presented, for `--lose-device`.
    #[cfg(feature = "dev")]
    presented: u64,
}

/// The game on one device: what a lost device takes.
struct OnDevice {
    /// Before the stage: it's on the stage's device, and holds the window's native handle.
    surface: Option<RenderSurface>,
    stage: Stage<GameShow>,
}

impl App for GameApp {
    const METADATA: AppMetadata = AppMetadata {
        name: "Sapphire",
        organization: "Sapphire",
        identifier: "dev.sapphire.game",
        version: env!("CARGO_PKG_VERSION"),
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let mut options = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        let info = *platform.system_info();
        tracing::info!(target: LOG_TARGET, version = Self::METADATA.version, ?info, ?options, "starting");
        let window = platform
            .create_window(&WindowDesc { title: Self::METADATA.name.to_owned(), ..WindowDesc::default() })?;
        let (settings, chosen) = KeptSettings::load(platform.user_dir(), durable_replace);
        let rumble = chosen.rumble;
        let volume = chosen.volume();
        let voice = chosen.locale().voice;
        let mut on_device =
            stand_on_device(platform, &window, &options, chosen).map_err(|failed| failed.error)?;
        let Stage { show, content, renderer, gpu, .. } = &mut on_device.stage;
        let mut audio = match show.shown.fight.show.cue_show() {
            Some(cues) => DeviceAudio::for_show(&cues.sounds_to_load(), platform, content, renderer, gpu)?,
            None => None,
        };
        if let Some(audio) = &mut audio {
            audio.set_volume(volume);
            speak_in(audio, &voice);
        }
        // Only the first show starts past its title: a reload goes back where the game was.
        if let Some(command) = options.choose.take() {
            show.choose(command);
        } else {
            tracing::info!(target: LOG_TARGET, "the title is up");
        }
        Ok(Self {
            on_device: Some(on_device),
            reload: None,
            losses: Losses::default(),
            audio,
            settings,
            rumble,
            window,
            desktop: info.device_class == DeviceClass::Desktop,
            options,
            real: RealClock::new(),
            driver: FixedStepDriver::new(FixedStepConfig::default()),
            suspended: false,
            pointer_held: false,
            retry_due: Duration::ZERO,
            #[cfg(feature = "dev")]
            presented: 0,
        })
    }

    fn event(&mut self, platform: &mut Platform, event: &Event) -> AppControl {
        if let Some(on_device) = &mut self.on_device {
            on_device.stage.show.handle_event(event);
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
                // Whoever's holding the pad isn't watching any more, and a desktop's fight waits for them.
                WindowEvent::FocusLost => {
                    self.stop_rumble(platform);
                    let desktop = self.desktop;
                    if let Some(show) = self.show_mut().filter(|show| desktop && show.is_played()) {
                        show.controls.set_paused(true);
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
        if let Some(on_device) = &mut self.on_device
            && on_device.stage.show.read_input(&self.window, self.desktop, platform.now())
        {
            return AppControl::Exit;
        }
        if let Some(chosen) = self.show_mut().and_then(GameShow::take_changed_settings) {
            self.follow_settings(&chosen, platform);
        }
        self.settings.poll();
        self.follow_pointer();
        if self.suspended {
            std::thread::sleep(IDLE_FRAME);
            return AppControl::Continue;
        }
        #[cfg(feature = "dev")]
        self.lose_device_when_asked();
        let Some(on_device) = self.on_device.as_mut().filter(|on_device| !on_device.stage.gpu.is_lost())
        else {
            if let Err(error) = self.recover(platform) {
                tracing::error!(target: LOG_TARGET, %error, "the game can't stand on a GPU device");
                return AppControl::Fail;
            }
            std::thread::sleep(IDLE_FRAME);
            return AppControl::Continue;
        };
        if on_device.surface.is_none() && self.real.elapsed() >= self.retry_due {
            self.retry_due = self.real.elapsed() + RETRY;
            on_device.surface = rebuild_surface(&self.window, &on_device.stage.gpu);
        }

        let stage = &mut on_device.stage;
        let steps = stage.show.advance(&stage.scene, &mut self.driver, frame_time);
        for _ in 0..steps.count {
            stage.fixed_step();
        }

        // Presentation runs on the real clock, so the camera stays responsive in slow motion.
        let dt = frame_time.min(MAX_PRESENTATION_DT);
        let view_dt = stage.show.drive_view(dt);
        let dt = dt.as_secs_f32();
        if let Some(audio) = &mut self.audio {
            audio.update(dt, platform, stage.scene.sound(), stage.show.cue_sounds());
        }
        let pad = stage.show.controls.input().active_gamepad().filter(|_| self.rumble);
        stage.show.shown.fight.show.end_cue_frame(
            &mut stage.scene,
            dt,
            view_dt,
            steps.alpha,
            &mut PadRumble { platform, pad },
        );

        let presented = match on_device.render(steps.alpha) {
            Ok(presented) => presented,
            Err(error) if on_device.stage.gpu.is_lost() => {
                // The next frame stands the game on a new device.
                tracing::warn!(target: LOG_TARGET, %error, "the device was lost drawing the frame");
                false
            }
            Err(error) => {
                // The graph is built in code, so it would fail the same way every frame.
                tracing::error!(target: LOG_TARGET, %error, "the frame can't be drawn");
                return AppControl::Fail;
            }
        };
        // A lost device's frame presents nothing it drew, and proves nothing.
        if presented && !on_device.stage.gpu.is_lost() {
            // Rendering works with this pipeline cache, so the next boot may trust it.
            on_device.stage.gpu.confirm_pipeline_cache();
            self.losses.presented();
            #[cfg(feature = "dev")]
            {
                self.presented += 1;
            }
        } else {
            // Nothing blocked on vsync this frame, so pace the loop here instead of spinning a core.
            std::thread::sleep(IDLE_FRAME);
        }
        on_device.stage.scene.end_frame();
        profiling::finish_frame!();
        AppControl::Continue
    }

    fn quit(&mut self, _platform: &mut Platform) {
        self.save_pipeline_cache();
        self.settings.finish();
        // The run's file, if it keeps one, and the save's last write. A stage lost and not yet stood again
        // wrote both as it went.
        if let Some(show) = self.show_mut()
            && let Err(error) = show.finish()
        {
            tracing::error!(target: LOG_TARGET, %error, "the run didn't end well");
        }
        tracing::info!(target: LOG_TARGET, "quit");
    }
}

impl GameApp {
    fn show_mut(&mut self) -> Option<&mut GameShow> {
        self.on_device.as_mut().map(|on_device| &mut on_device.stage.show)
    }

    /// A fight someone's playing at a desktop holds the pointer: the mouse turns its camera, which a cursor
    /// reaching the screen's edge would stop. A pause gives it back, once for each change.
    fn follow_pointer(&mut self) {
        let show = self.on_device.as_ref().map(|on_device| &on_device.stage.show);
        let playing = show.is_some_and(|show| show.is_played() && !show.controls.is_paused());
        let held = self.desktop && playing && self.window.has_keyboard_focus();
        if held != self.pointer_held {
            self.pointer_held = held;
            self.window.set_relative_mouse(held);
        }
    }

    /// What the player has just chosen: heard now, the window named in its language, and kept for the next
    /// run. The language and size have reached the screens already.
    fn follow_settings(&mut self, chosen: &PlayerSettings, platform: &mut Platform) {
        if let Some(on_device) = &self.on_device {
            name_window(&self.window, &on_device.stage.show);
        }
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
        let Some(show) = self.show_mut() else { return };
        let pad = show.controls.input().active_gamepad();
        show.shown.fight.show.stop_rumble(&mut PadRumble { platform, pad });
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(surface) = self.on_device.as_mut().and_then(|on_device| on_device.surface.as_mut()) {
            surface.resize(width, height);
        }
    }

    /// Spec §6.5, as [`duties`] orders it.
    fn lifecycle(&mut self, event: LifecycleEvent, platform: &mut Platform) {
        tracing::info!(target: LOG_TARGET, ?event, "lifecycle");
        for &duty in duties(event) {
            match duty {
                Duty::StopDrawing => self.suspended = true,
                Duty::StopRumble => self.stop_rumble(platform),
                Duty::PauseAndKeep => {
                    if let Some(show) = self.show_mut() {
                        show.leave_foreground();
                    }
                }
                Duty::FlushSettings => self.settings.finish(),
                Duty::SuspendAudio | Duty::ResumeAudio => {
                    if let Some(audio) = &mut self.audio {
                        audio.set_background(duty == Duty::SuspendAudio, platform);
                    }
                }
                Duty::DropSurface => {
                    if !Os::CURRENT.keeps_surface_in_background()
                        && let Some(on_device) = &mut self.on_device
                    {
                        on_device.surface = None;
                    }
                }
                Duty::SavePipelineCache => self.save_pipeline_cache(),
                // `iterate` doesn't run while suspended on Android (or after a real iOS background), so the
                // next frame would otherwise see the whole pause as one frame.
                Duty::RestartClock => {
                    self.real.update();
                }
                Duty::Draw => self.suspended = false,
                Duty::RebuildSurface => {
                    if let Some(on_device) =
                        self.on_device.as_mut().filter(|on_device| on_device.surface.is_none())
                    {
                        self.retry_due = self.real.elapsed() + RETRY;
                        on_device.surface = rebuild_surface(&self.window, &on_device.stage.gpu);
                    }
                }
            }
        }
    }

    /// Stands the game on a new device after the last was lost. The first call ends the lost stage's show,
    /// which writes the run's file and the save; then each try, a second apart, makes a device and a stage and
    /// takes the game back where the save has it. A failed try leaves the app running with no stage. An error
    /// when [`Losses`] says to stop.
    fn recover(&mut self, platform: &mut Platform) -> Result<(), String> {
        if let Some(mut lost) = self.on_device.take() {
            tracing::warn!(target: LOG_TARGET, "the GPU device was lost; the game stands again on a new one");
            self.reload = Some(lost.stage.show.end_for_reload());
            // Everything on the old device, the surface first, before a new surface is made for the window.
            drop(lost);
            self.losses.lost()?;
            self.retry_due = Duration::ZERO;
        }
        if self.real.elapsed() < self.retry_due {
            return Ok(());
        }
        self.retry_due = self.real.elapsed() + RETRY;
        let Some(reload) = &self.reload else { return Ok(()) };
        match stand_on_device(platform, &self.window, &self.options, reload.settings.clone()) {
            Ok(mut on_device) => {
                on_device.stage.show.reload_from(reload);
                self.reload = None;
                self.on_device = Some(on_device);
                tracing::info!(target: LOG_TARGET, "the game stands on a new GPU device");
            }
            Err(Failed { error, lost: true }) => {
                tracing::warn!(target: LOG_TARGET, %error, "the new GPU device was lost as the game stood");
                self.losses.lost()?;
            }
            Err(Failed { error, lost: false }) => {
                tracing::warn!(target: LOG_TARGET, %error, retry_in = ?RETRY, "the game didn't stand on a new GPU device");
            }
        }
        Ok(())
    }

    /// `--lose-device`: destroys the device once, after the frames it says.
    #[cfg(feature = "dev")]
    fn lose_device_when_asked(&mut self) {
        if self.options.lose_device.is_some_and(|after| self.presented >= after)
            && let Some(on_device) = &self.on_device
        {
            self.options.lose_device = None;
            tracing::warn!(target: LOG_TARGET, presented = self.presented, "losing the GPU device, as --lose-device asks");
            let gpu = &on_device.stage.gpu;
            gpu.device().destroy();
            // wgpu reports a destroyed device once its queue drains, during a poll: this frame's checks see it.
            let _ = gpu.device().poll(sapphire_rhi::wgpu::PollType::wait_indefinitely());
        }
    }

    fn save_pipeline_cache(&self) {
        let Some(on_device) = &self.on_device else { return };
        if let Err(error) = on_device.stage.gpu.save_pipeline_cache() {
            tracing::warn!(target: LOG_TARGET, %error, "pipeline cache not saved");
        }
    }
}

impl OnDevice {
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
        // frame's to stand the game again on.
        if skipped == Some(SkipReason::Lost) {
            self.surface = None;
        }
        Ok(false)
    }
}

/// What the app does as the OS moves it (spec §6.5), each in [`duties`]' order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Duty {
    /// Submit nothing more to the GPU: iOS ends an app that does in the background.
    StopDrawing,
    /// Whoever holds the pad isn't watching.
    StopRumble,
    /// A run someone's playing pauses, and its file and the save are written ([`GameShow::leave_foreground`]).
    PauseAndKeep,
    /// The settings' writes finish.
    FlushSettings,
    SuspendAudio,
    /// Where the OS takes the window behind it (Android): made again coming back.
    DropSurface,
    /// What the driver built this run, for the next boot.
    SavePipelineCache,
    /// The time away counts as no frame's.
    RestartClock,
    Draw,
    ResumeAudio,
    /// A surface dropped going down, made again from the window's new handle.
    RebuildSurface,
}

/// What `event` asks of the app, in order. Going down, the pause comes before the writes, so what's kept is
/// what the player comes back to, and the writes before the audio falls silent and the window goes, so a
/// process the OS kills there has lost nothing kept. Coming back, the pause's screen is what's drawn first:
/// the player's tap on it is what goes on, so nothing here resumes the run.
fn duties(event: LifecycleEvent) -> &'static [Duty] {
    use Duty::*;
    match event {
        LifecycleEvent::WillEnterBackground => &[
            StopDrawing,
            StopRumble,
            PauseAndKeep,
            FlushSettings,
            SuspendAudio,
            DropSurface,
            SavePipelineCache,
        ],
        LifecycleEvent::DidEnterForeground => &[RestartClock, Draw, ResumeAudio, RebuildSurface],
        // The app may never quit: what a quit writes, written now.
        LifecycleEvent::Terminating => &[PauseAndKeep, FlushSettings, SavePipelineCache],
        LifecycleEvent::LowMemory
        | LifecycleEvent::DidEnterBackground
        | LifecycleEvent::WillEnterForeground => &[],
    }
}

/// Devices lost in a row with no frame presented between: a device lost while the game stands, or before it
/// draws, would be lost again on every try (the sandbox's `CreationLosses` has the emulator's case), so the
/// [`Self::LIMIT`]th stops the app. A presented frame, which only a stage standing on a live device draws, ends
/// the run of losses.
#[derive(Debug, Default)]
struct Losses {
    in_a_row: u32,
}

impl Losses {
    const LIMIT: u32 = 3;

    /// A device was lost: the one in use, or one the game was standing on.
    fn lost(&mut self) -> Result<(), String> {
        self.in_a_row += 1;
        if self.in_a_row >= Self::LIMIT {
            return Err(format!(
                "the GPU device was lost {} times in a row with no frame presented between",
                self.in_a_row
            ));
        }
        Ok(())
    }

    /// A frame was presented: the stage in use works.
    fn presented(&mut self) {
        self.in_a_row = 0;
    }
}

/// Why the game didn't stand on a device, and whether the device was lost.
struct Failed {
    error: AppInitError,
    lost: bool,
}

/// The game on a new device for `window`: the device, the window's surface sized to it, and the show `options`
/// make with `settings` stood on a stage, at its title ([`crate::stand_game`]). At startup and after a lost one.
fn stand_on_device(
    platform: &mut Platform,
    window: &Window,
    options: &Options,
    settings: PlayerSettings,
) -> Result<OnDevice, Failed> {
    let info = *platform.system_info();
    let failed = |error: AppInitError| Failed { error, lost: false };
    // Narrow the build's defaults, never widen them: the Android emulator's Vulkan driver (gfxstream) crashes
    // naming objects, so labels go off there and only there.
    let base = GpuDebug::default();
    let debug = GpuDebug {
        object_labels: base.object_labels && !(info.is_emulator && info.os == Os::Android),
        ..base
    };
    let instance = GpuInstance::new(&GpuInstanceDesc { debug, ..GpuInstanceDesc::default() })
        .map_err(|error| failed(error.into()))?;
    let handle = window.surface_handle().map_err(|error| failed(error.into()))?;
    let target = surface_target(&handle).map_err(|error| failed(error.into()))?;
    // SAFETY: this app owns the surface and drops it before SDL ends the run. The surface owns `handle`, which
    // keeps the native window alive within that run, and drops it right after itself.
    let mut surface =
        unsafe { RenderSurface::new(&instance, target, handle) }.map_err(|error| failed(error.into()))?;
    let pipeline_cache = PipelineCacheDesc {
        dir: platform.cache_dir().join("pipeline_cache"),
        build_id: GameApp::METADATA.version.to_owned(),
    };
    let desc = GpuContextDesc { pipeline_cache: Some(pipeline_cache), ..GpuContextDesc::default() };
    let gpu = GpuContext::new(&instance, &desc, Some(&surface)).map_err(|error| failed(error.into()))?;
    tracing::info!(target: LOG_TARGET, caps = ?gpu.caps(), "GPU ready");
    let lost = gpu.lost_flag();
    let lost =
        move |error: AppInitError| Failed { error, lost: lost.load(std::sync::atomic::Ordering::Acquire) };
    let size = window.pixel_size();
    surface
        .configure(&gpu, size.width, size.height, PresentPolicy::Fifo)
        .map_err(|error| lost(error.into()))?;
    let format = surface
        .format()
        .ok_or_else(|| AppInitError::msg("configured surface reports no format"))
        .map_err(&lost)?;
    let (show, first) = options.show(platform, settings).map_err(failed)?;
    let mut stage = crate::stand_game(platform.host(), gpu, format, show, &first).map_err(&lost)?;
    // Frames are paced to the tier's rate (spec §6.2).
    platform.set_frame_rate(stage.quality.frame_rate());
    let window_size = window.size();
    stage.show.controls.input_mut().set_window_size(window_size.width as f32, window_size.height as f32);
    name_window(window, &stage.show);
    Ok(OnDevice { surface: Some(surface), stage })
}

/// Makes the surface again from a freshly fetched native handle, or none, logged, for another try later.
fn rebuild_surface(window: &Window, gpu: &GpuContext) -> Option<RenderSurface> {
    let size = window.pixel_size();
    let result = window.surface_handle().map_err(|error| error.to_string()).and_then(|handle| {
        let target = surface_target(&handle).map_err(|error| error.to_string())?;
        // SAFETY: this app owns the surface and drops it before SDL ends the run. The surface owns `handle`,
        // which keeps the native window alive within that run, and drops it right after itself.
        let mut surface = unsafe { RenderSurface::new(gpu.instance(), target, handle) }
            .map_err(|error| error.to_string())?;
        surface
            .configure(gpu, size.width, size.height, PresentPolicy::Fifo)
            .map_err(|error| error.to_string())?;
        Ok(surface)
    });
    match result {
        Ok(surface) => {
            tracing::info!(target: LOG_TARGET, width = size.width, height = size.height, "surface rebuilt");
            Some(surface)
        }
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "surface rebuild failed");
            None
        }
    }
}

/// Names the window in the player's language, from the game's strings; until they load it has the app's name.
fn name_window(window: &Window, show: &GameShow) {
    if let Some(title) = show.window_title() {
        window.set_title(title);
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

    fn at(duties: &[Duty], duty: Duty) -> usize {
        duties.iter().position(|&held| held == duty).unwrap_or_else(|| panic!("{duty:?} isn't in {duties:?}"))
    }

    /// Going down: nothing more is drawn, then the run pauses, then what's on its way to disk gets there, and
    /// only then does the audio fall silent and the window go, so a process killed in the background has lost
    /// nothing kept and comes back, if it does, to the pause.
    #[test]
    fn the_background_pauses_then_writes_then_suspends() {
        let down = duties(LifecycleEvent::WillEnterBackground);
        assert_eq!(at(down, Duty::StopDrawing), 0, "no GPU work from the first moment");
        assert!(at(down, Duty::PauseAndKeep) < at(down, Duty::FlushSettings));
        assert!(at(down, Duty::FlushSettings) < at(down, Duty::SuspendAudio));
        assert!(at(down, Duty::SuspendAudio) < at(down, Duty::DropSurface));
        assert!(!down.contains(&Duty::Draw) && !down.contains(&Duty::ResumeAudio));

        // Where the OS may end the app without a quit, what a quit writes is written.
        let end = duties(LifecycleEvent::Terminating);
        assert!(at(end, Duty::PauseAndKeep) < at(end, Duty::FlushSettings));
        assert!(end.contains(&Duty::SavePipelineCache));
    }

    /// Coming back: the time away is no frame's, the audio and the surface return, and the run stays at the
    /// pause, for the player's tap: nothing here resumes it.
    #[test]
    fn the_return_draws_the_pause_and_waits_for_a_tap() {
        let up = duties(LifecycleEvent::DidEnterForeground);
        assert!(at(up, Duty::RestartClock) < at(up, Duty::Draw), "the time away isn't a frame's");
        assert!(up.contains(&Duty::ResumeAudio) && up.contains(&Duty::RebuildSurface));
        assert!(!up.contains(&Duty::PauseAndKeep), "the pause it comes back to is the one it left in");
        for between in [
            LifecycleEvent::DidEnterBackground,
            LifecycleEvent::WillEnterForeground,
            LifecycleEvent::LowMemory,
        ] {
            assert_eq!(duties(between), [], "{between:?}");
        }
    }

    /// The third device lost with no frame presented between stops the app: one lost in play, then one lost as
    /// the game stood again, then the stage it stood lost before it drew. A frame presented starts the count again.
    #[test]
    fn three_devices_lost_with_nothing_drawn_stop_the_app() {
        let mut losses = Losses::default();
        assert!(losses.lost().is_ok());
        assert!(losses.lost().is_ok());
        assert!(losses.lost().is_err(), "a third in a row");

        let mut losses = Losses::default();
        for _ in 0..10 {
            assert!(losses.lost().is_ok());
            assert!(losses.lost().is_ok());
            losses.presented();
        }
    }
}
