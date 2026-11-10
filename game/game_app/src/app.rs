//! The window: its surface, its clocks and the frame loop, first around the boot screen while the game builds
//! and stands under it (`game_flow::boot`, M4 H2), then around the stood game's stage.
//!
//! **The OS moving the app (spec §6.5, M4 S6)** is [`duties`]' table, in order: going down, a run someone's
//! playing pauses and what's on its way to disk gets there before the audio falls silent; coming back, the
//! pause's screen is what's drawn, and the player's tap on it is what goes on. A boot in the background draws
//! and uploads nothing, as no frame does there, and holds where it was; the warm-up pool goes on building, and
//! the boot screen is what comes back.
//!
//! **A lost device** takes the stage with it, or the boot. The show ends as a quit ends it, writing the run's
//! file and the save; the device, its surface and a boot are made again, and once that boot has stood the game
//! it goes back where the save has it, waiting at the pause if someone was playing. A boot whose device is lost
//! starts again on a new one, from where the last lost stage left the game, or from the title. A device lost
//! again before the next stage presents a frame counts toward [`Losses`]' limit; the audio, the settings and
//! the window go on through it.
//!
//! **Patches (M4 P4)** are the first boot's to fetch and install (`game_flow::update`): it checks the build's
//! host before anything else, and stands the game on what it installs. Every later boot, after a lost device,
//! stands on the release that's current then. The launch is confirmed, and earlier releases' files cleared,
//! once the stood game has presented a frame.

use std::sync::Arc;
use std::time::Duration;

use game_combat::cues::PadRumble;
use game_flow::boot::{Boot, BootUpdate};
use game_flow::show::{GameShow, Reload};
use game_flow::update::Patches;
use game_ui::menus::PlayerSettings;
use sapphire_platform::raw_window_handle::HandleError;
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, DeviceClass, Event, Host, LifecycleEvent, Os, Platform, Size,
    SurfaceHandle, SystemInfo, Window, WindowDesc, WindowEvent, durable_replace,
};
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuDebug, GpuInstance, GpuInstanceDesc, PipelineCacheDesc, PresentPolicy,
    RenderSurface, SkipReason, SurfaceAcquire, SurfaceTarget,
};
use sapphire_stage::Stage;
use sapphire_stage::audio::{DeviceAudio, Volumes};
use sapphire_time::{FixedStepConfig, FixedStepDriver, RealClock};

use crate::options::Options;
use crate::settings::KeptSettings;
use crate::tiers::{Pacing, TierFollower};
use crate::{LOG_TARGET, build_info};

/// Loop period while nothing can be presented (occluded, minimized, backgrounded).
const IDLE_FRAME: Duration = Duration::from_millis(33);
/// Longest real-time step presentation takes in one frame, so a hitch can't fling the camera.
const MAX_PRESENTATION_DT: Duration = Duration::from_millis(100);
/// How long a window with no surface, or a game with no device, waits before trying to make one again.
const RETRY: Duration = Duration::from_secs(1);

/// See this module.
pub struct GameApp {
    /// Before the window, whose native handle its surface holds. None from a lost device until a boot starts on
    /// the next.
    on_device: Option<OnDevice>,
    /// Where the game goes back to once a boot on the next device has stood it.
    reload: Option<Reload>,
    losses: Losses,
    /// Made once the first boot has stood the game. Outlives a lost device: what it plays was read from the
    /// content, and nothing of it is on the GPU.
    audio: Option<DeviceAudio>,
    /// Whether the first boot has stood the game, so the audio's made and the title's been passed or shown.
    heard: bool,
    /// The release the game stands on, and the means to look for the next. None where the options say a run
    /// patches nothing.
    patches: Option<Patches>,
    /// Whether a stood game has presented a frame on its release, so the launch is confirmed.
    launched: bool,
    /// What the player had chosen as the app started: what a boot stands the first show with.
    chosen: PlayerSettings,
    settings: KeptSettings,
    /// The quality tier, the heat and the frame cap, frame by frame (M4 T3).
    tiers: TierFollower,
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
    /// When a missing surface or device is next tried for.
    retry_due: Duration,
    /// Frames presented, for `--lose-device`.
    #[cfg(feature = "dev")]
    presented: u64,
    /// A playtest build's session (M4 F1): what's kept of this play.
    #[cfg(feature = "playtest")]
    playtest: Option<crate::playtest::Playtest>,
}

/// The game on one device: what a lost device takes.
struct OnDevice {
    /// Before the game: it's on the game's device, and holds the window's native handle.
    surface: Option<RenderSurface>,
    game: Game,
}

/// The game on its device: booting, then stood.
enum Game {
    /// The boot screen up, the rest building where no frame waits.
    Booting(Box<Boot>),
    Stood(Box<Stage<GameShow>>),
}

impl App for GameApp {
    const METADATA: AppMetadata = AppMetadata {
        name: "Sapphire",
        organization: "Sapphire",
        identifier: build_info::BUNDLE_ID,
        version: env!("CARGO_PKG_VERSION"),
    };

    /// The window, the device and the boot screen with its one pipeline: everything else builds while it draws.
    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let options = Options::parse(platform.args()).map_err(AppInitError::msg)?;
        let info = *platform.system_info();
        tracing::info!(target: LOG_TARGET, build = build_info::BUILD_ID, ?info, ?options, "starting");
        let window = platform.create_window(&window_desc(Self::METADATA.name, &info))?;
        let (settings, chosen) = KeptSettings::load(platform.user_dir(), durable_replace);
        let tiers = TierFollower::new(&chosen, info.device_class);
        #[cfg(feature = "playtest")]
        let playtest = {
            let dir = options.save_dir.clone().unwrap_or_else(|| platform.user_dir().to_owned());
            crate::playtest::Playtest::begin(platform, &Self::METADATA, &dir, &chosen)
        };
        let rumble = chosen.rumble;
        let patches = options.patch_setup(platform).map(|setup| Patches::open(platform.host(), setup));
        let host = host_of(platform, patches.as_ref());
        let update = patches
            .as_ref()
            .and_then(Patches::check)
            .map(|update| BootUpdate { update, remake: options.make_show(platform, chosen.clone()) });
        let on_device = boot_on_device(platform, &host, &window, &options, chosen.clone(), update)
            .map_err(|failed| failed.error)?;
        Ok(Self {
            on_device: Some(on_device),
            reload: None,
            losses: Losses::default(),
            audio: None,
            heard: false,
            patches,
            launched: false,
            chosen,
            settings,
            tiers,
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
            #[cfg(feature = "playtest")]
            playtest,
        })
    }

    fn event(&mut self, platform: &mut Platform, event: &Event) -> AppControl {
        // Nothing's there to play while booting, so input goes nowhere.
        if let Some(show) = self.show_mut() {
            show.handle_event(event);
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
        if let Some(OnDevice { game: Game::Stood(stage), .. }) = &mut self.on_device
            && stage.show.read_input(&self.window, self.desktop, platform.now())
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
        let Some(on_device) = self.on_device.as_mut().filter(|on_device| !on_device.gpu().is_lost()) else {
            if let Err(error) = self.recover(platform) {
                tracing::error!(target: LOG_TARGET, %error, "the game can't stand on a GPU device");
                return AppControl::Fail;
            }
            std::thread::sleep(IDLE_FRAME);
            return AppControl::Continue;
        };
        if on_device.surface.is_none() && self.real.elapsed() >= self.retry_due {
            self.retry_due = self.real.elapsed() + RETRY;
            on_device.surface = rebuild_surface(&self.window, on_device.gpu());
        }

        let OnDevice { surface, game } = on_device;
        let stage = match game {
            Game::Booting(boot) => {
                let dt = frame_time.min(MAX_PRESENTATION_DT).as_secs_f32();
                match boot_frame(surface, boot, dt) {
                    Ok(Some(stage)) => {
                        // A boot that installed a release stood on it: so does every boot after.
                        if boot.report().updated
                            && let Some(patches) = &mut self.patches
                        {
                            patches.reopen();
                        }
                        *game = Game::Stood(Box::new(stage));
                    }
                    Ok(None) => {}
                    Err(error) if boot.gpu().is_lost() => {
                        // The next frame boots again on a new device.
                        tracing::warn!(target: LOG_TARGET, %error, "the device was lost booting");
                    }
                    Err(error) => {
                        tracing::error!(target: LOG_TARGET, %error, "the game didn't boot");
                        return AppControl::Fail;
                    }
                }
                if matches!(game, Game::Stood(_))
                    && let Err(error) = self.stood(platform)
                {
                    tracing::error!(target: LOG_TARGET, %error, "the game didn't boot");
                    return AppControl::Fail;
                }
                profiling::finish_frame!();
                return AppControl::Continue;
            }
            Game::Stood(stage) => stage,
        };

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

        // Between frames: the ladder's heat, a tier change while the show holds still, and the pace.
        let heat = platform.thermal();
        if let Some(pacing) = self.tiers.frame(stage, heat) {
            pace(platform, pacing);
        }
        let presented = match render(surface, stage, steps.alpha) {
            Ok(presented) => presented,
            Err(error) if stage.gpu.is_lost() => {
                // The next frame boots the game again on a new device.
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
        if presented && !stage.gpu.is_lost() {
            // Rendering works with this pipeline cache, so the next boot may trust it.
            stage.gpu.confirm_pipeline_cache();
            self.losses.presented();
            // And with this release, so what earlier ones left can go.
            if !self.launched {
                self.launched = true;
                if let Some(patches) = &mut self.patches {
                    patches.confirm_launch();
                }
            }
            #[cfg(feature = "dev")]
            {
                self.presented += 1;
            }
        } else {
            // Nothing blocked on vsync this frame, so pace the loop here instead of spinning a core.
            std::thread::sleep(IDLE_FRAME);
        }
        stage.scene.end_frame();
        #[cfg(feature = "playtest")]
        if let Some(playtest) = &mut self.playtest {
            playtest.frame(stage, frame_time, heat);
        }
        profiling::finish_frame!();
        AppControl::Continue
    }

    fn quit(&mut self, _platform: &mut Platform) {
        self.save_pipeline_cache();
        self.settings.finish();
        // The run's file, if it keeps one, and the save's last write. A stage lost and not yet stood again
        // wrote both as it went, and a boot has neither yet.
        if let Some(show) = self.show_mut()
            && let Err(error) = show.finish()
        {
            tracing::error!(target: LOG_TARGET, %error, "the run didn't end well");
        }
        #[cfg(feature = "playtest")]
        if let Some(playtest) = self.playtest.take() {
            playtest.end(self.show_mut());
        }
        tracing::info!(target: LOG_TARGET, "quit");
    }
}

impl GameApp {
    /// The stood game's show: none while booting.
    fn show_mut(&mut self) -> Option<&mut GameShow> {
        match &mut self.on_device {
            Some(OnDevice { game: Game::Stood(stage), .. }) => Some(&mut stage.show),
            _ => None,
        }
    }

    fn show(&self) -> Option<&GameShow> {
        match &self.on_device {
            Some(OnDevice { game: Game::Stood(stage), .. }) => Some(&stage.show),
            _ => None,
        }
    }

    /// The game a boot has just stood, in the window that booted it: sized to the window and named in its
    /// language, then, the first time, heard and on its title or past it as the arguments say; after a lost
    /// device, back where the save has it.
    fn stood(&mut self, platform: &mut Platform) -> Result<(), AppInitError> {
        let Some(OnDevice { game: Game::Stood(stage), .. }) = &mut self.on_device else {
            unreachable!("a boot has just stood the game");
        };
        let window_size = self.window.size();
        stage.show.controls.input_mut().set_window_size(window_size.width as f32, window_size.height as f32);
        name_window(&self.window, &stage.show);
        // The boot paced its own frames: the game's first tells the platform again.
        self.tiers.stood();
        let Stage { show, content, renderer, gpu, .. } = &mut **stage;
        if !self.heard {
            self.heard = true;
            self.audio = match show.shown.fight.show.cue_show() {
                Some(cues) => {
                    DeviceAudio::for_show(&cues.sounds_to_load(), platform, content, renderer, gpu)?
                }
                None => None,
            };
            if let Some(audio) = &mut self.audio {
                audio.set_volumes(volumes_of(&self.chosen));
                audio.set_mono(self.chosen.mono);
                speak_in(audio, &self.chosen.locale().voice);
            }
        }
        if let Some(reload) = self.reload.take() {
            show.reload_from(&reload);
            tracing::info!(target: LOG_TARGET, "the game stands on a new GPU device");
        } else if let Some(command) = self.options.choose.take() {
            // Only the first show starts past its title: a reload goes back where the game was.
            show.choose(command);
        } else {
            tracing::info!(target: LOG_TARGET, "the title is up");
        }
        Ok(())
    }

    /// A fight someone's playing at a desktop holds the pointer: the mouse turns its camera, which a cursor
    /// reaching the screen's edge would stop. A pause gives it back, once for each change.
    fn follow_pointer(&mut self) {
        let playing = self.show().is_some_and(|show| show.is_played() && !show.controls.is_paused());
        let held = self.desktop && playing && self.window.has_keyboard_focus();
        if held != self.pointer_held {
            self.pointer_held = held;
            self.window.set_relative_mouse(held);
        }
    }

    /// What the player has just chosen: heard now, the window named in its language, and kept for the next
    /// run. The language and size have reached the screens already.
    fn follow_settings(&mut self, chosen: &PlayerSettings, platform: &mut Platform) {
        if let Some(show) = self.show() {
            name_window(&self.window, show);
        }
        if let Some(audio) = &mut self.audio {
            audio.set_volumes(volumes_of(chosen));
            audio.set_mono(chosen.mono);
            speak_in(audio, &chosen.locale().voice);
        }
        if self.rumble && !chosen.rumble {
            self.stop_rumble(platform);
        }
        self.rumble = chosen.rumble;
        if let Some(OnDevice { game: Game::Stood(stage), .. }) = &mut self.on_device {
            self.tiers.follow_settings(chosen, stage);
        }
        self.settings.keep(chosen);
        #[cfg(feature = "playtest")]
        if let Some(playtest) = &mut self.playtest {
            playtest.settings(chosen);
        }
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

    /// Spec §6.5, as [`duties`] orders it. A boot has no show to pause or keep, and no audio yet unless a lost
    /// device's boot follows a stood game.
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
                    #[cfg(feature = "playtest")]
                    if let Some(playtest) = &mut self.playtest {
                        let show = match &mut self.on_device {
                            Some(OnDevice { game: Game::Stood(stage), .. }) => Some(&mut stage.show),
                            _ => None,
                        };
                        if event == LifecycleEvent::WillEnterBackground {
                            playtest.left_foreground(show);
                        } else {
                            playtest.flush(show);
                        }
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
                    if let Some(OnDevice { game: Game::Booting(boot), .. }) = &mut self.on_device {
                        boot.restart_clock();
                    }
                }
                Duty::Draw => self.suspended = false,
                Duty::RebuildSurface => {
                    if let Some(on_device) =
                        self.on_device.as_mut().filter(|on_device| on_device.surface.is_none())
                    {
                        self.retry_due = self.real.elapsed() + RETRY;
                        on_device.surface = rebuild_surface(&self.window, on_device.gpu());
                    }
                }
            }
        }
    }

    /// Boots the game again on a new device after the last was lost. The first call ends the lost stage's
    /// show, which writes the run's file and the save (a lost boot has neither); then each try, a second apart,
    /// makes a device and starts a boot on it, which takes the game back where the save has it once it's stood
    /// ([`Self::stood`]). A failed try leaves the app running with no device. An error when [`Losses`] says to
    /// stop.
    fn recover(&mut self, platform: &mut Platform) -> Result<(), String> {
        if let Some(mut lost) = self.on_device.take() {
            match &mut lost.game {
                Game::Stood(stage) => {
                    tracing::warn!(target: LOG_TARGET, "the GPU device was lost; the game boots again on a new one");
                    self.reload = Some(stage.show.end_for_reload());
                    #[cfg(feature = "playtest")]
                    if let Some(playtest) = &mut self.playtest {
                        playtest.take_runs(&mut stage.show);
                    }
                }
                // Where an earlier loss left the game is still where the next boot takes it.
                Game::Booting(_) => {
                    tracing::warn!(target: LOG_TARGET, "the GPU device was lost booting; the game boots again on a new one");
                }
            }
            // Everything on the old device, the surface first, before a new surface is made for the window.
            drop(lost);
            self.losses.lost()?;
            self.retry_due = Duration::ZERO;
        }
        if self.real.elapsed() < self.retry_due {
            return Ok(());
        }
        self.retry_due = self.real.elapsed() + RETRY;
        let settings = self.reload.as_ref().map_or(&self.chosen, |reload| &reload.settings).clone();
        let host = host_of(platform, self.patches.as_ref());
        match boot_on_device(platform, &host, &self.window, &self.options, settings, None) {
            Ok(on_device) => {
                self.on_device = Some(on_device);
                tracing::info!(target: LOG_TARGET, "the game boots on a new GPU device");
            }
            Err(Failed { error, lost: true }) => {
                tracing::warn!(target: LOG_TARGET, %error, "the new GPU device was lost as the game booted");
                self.losses.lost()?;
            }
            Err(Failed { error, lost: false }) => {
                tracing::warn!(target: LOG_TARGET, %error, retry_in = ?RETRY, "the game didn't boot on a new GPU device");
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
            let gpu = on_device.gpu();
            gpu.device().destroy();
            // wgpu reports a destroyed device once its queue drains, during a poll: this frame's checks see it.
            let _ = gpu.device().poll(sapphire_rhi::wgpu::PollType::wait_indefinitely());
        }
    }

    fn save_pipeline_cache(&self) {
        let Some(on_device) = &self.on_device else { return };
        if let Err(error) = on_device.gpu().save_pipeline_cache() {
            tracing::warn!(target: LOG_TARGET, %error, "pipeline cache not saved");
        }
    }
}

impl OnDevice {
    fn gpu(&self) -> &GpuContext {
        match &self.game {
            Game::Booting(boot) => boot.gpu(),
            Game::Stood(stage) => &stage.gpu,
        }
    }
}

/// A boot frame: moved on, and its screen drawn and presented if the surface gave an image. Returns the stage
/// once it's stood and warm.
fn boot_frame(
    surface: &mut Option<RenderSurface>,
    boot: &mut Boot,
    dt: f32,
) -> Result<Option<Stage<GameShow>>, AppInitError> {
    // The image, if any, goes with this statement, before a lost surface is let go of.
    let (stage, presented, lost) = match surface.as_mut().map(RenderSurface::acquire) {
        Some(SurfaceAcquire::Frame(frame)) => {
            let stage = boot.frame(Some(frame.view()), dt)?;
            frame.present();
            (stage, true, false)
        }
        Some(SurfaceAcquire::Skip(reason)) => (boot.frame(None, dt)?, false, reason == SkipReason::Lost),
        None => (boot.frame(None, dt)?, false, false),
    };
    // `Lost` is sticky until the surface is rebuilt from a fresh native handle.
    if lost {
        *surface = None;
    }
    if !presented && stage.is_none() {
        // Nothing blocked on vsync this frame, so pace the loop here instead of spinning a core.
        std::thread::sleep(IDLE_FRAME);
    }
    Ok(stage)
}

/// Draws and presents the stage's frame if the surface gave an image. Every frame is prepared, drawn or not, so
/// the renderer's persistent scene sees every packet's deltas. Returns whether one was presented.
fn render(
    surface: &mut Option<RenderSurface>,
    stage: &mut Stage<GameShow>,
    alpha: f32,
) -> Result<bool, AppInitError> {
    profiling::scope!("render");
    let skipped = match surface.as_mut().map(RenderSurface::acquire) {
        Some(SurfaceAcquire::Frame(frame)) => {
            stage.render(frame.view(), alpha, |_, _| {})?;
            frame.present();
            stage.show.say_first_uses(&stage.renderer);
            return Ok(true);
        }
        Some(SurfaceAcquire::Skip(reason)) => Some(reason),
        None => None,
    };
    stage.skip_frame(alpha);
    // `Lost` is sticky until the surface is rebuilt from a fresh native handle. `DeviceLost` is the next
    // frame's to boot the game again on.
    if skipped == Some(SkipReason::Lost) {
        *surface = None;
    }
    Ok(false)
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

/// Devices lost in a row with no frame presented between: a device lost while the game boots, or before it
/// draws, would be lost again on every try (the sandbox's `CreationLosses` has the emulator's case), so the
/// [`Self::LIMIT`]th stops the app. A presented frame, which only a stood stage on a live device draws (the
/// boot screen's don't count), ends the run of losses.
#[derive(Debug, Default)]
struct Losses {
    in_a_row: u32,
}

impl Losses {
    const LIMIT: u32 = 3;

    /// A device was lost: the one in use, or one the game was booting on.
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

/// Why the game didn't start booting on a device, and whether the device was lost.
struct Failed {
    error: AppInitError,
    lost: bool,
}

/// The device with the release the game stands on: `patches`', else what the app shipped.
fn host_of(platform: &Platform, patches: Option<&Patches>) -> Host {
    patches.map_or(platform.host(), Patches::host).clone()
}

/// The game on a new device for `window`: the device, the window's surface sized to it, and the show `options`
/// make with `settings` on `host`'s content booting on it, its screen's pipeline built and the rest building
/// (`game_flow::boot`), the update first if given. At startup and after a lost one.
fn boot_on_device(
    platform: &mut Platform,
    host: &Host,
    window: &Window,
    options: &Options,
    settings: PlayerSettings,
    update: Option<BootUpdate>,
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
        // A new build may compile its pipelines differently, so it starts the driver's cache afresh.
        build_id: build_info::BUILD_ID.to_owned(),
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
    let (show, first) = options.show(platform, host.content(), settings).map_err(failed)?;
    // The player's tier and dynamic resolution from the start, so a held tier's renderer is the first made.
    let quality = crate::tiers::starting_quality(&show, info.device_class);
    let mut boot = Boot::start(host, Arc::new(gpu), format, quality, show, &first).map_err(&lost)?;
    if let Some(update) = update {
        boot = boot.with_update(update);
    }
    // Frames are paced to the tier's rate (spec §6.2), the boot screen's from its first, until the stood game's
    // first frame paces them as the player's cap and the ladder say (`tiers`).
    platform.set_frame_rate(boot.frame_rate());
    Ok(OnDevice { surface: Some(surface), game: Game::Booting(Box::new(boot)) })
}

/// Tells the platform how to pace frames (`tiers`).
fn pace(platform: &mut Platform, pacing: Pacing) {
    platform.set_frame_rate(pacing.rate);
    platform.set_frame_cap(pacing.cap);
}

/// The game's window on a device `info` describes: a Steam Deck's fills its 1280 × 800 panel (M4 T3).
fn window_desc(title: &str, info: &SystemInfo) -> WindowDesc {
    let desc = WindowDesc { title: title.to_owned(), ..WindowDesc::default() };
    if info.handheld {
        return WindowDesc { size: Size { width: 1280, height: 800 }, fullscreen: true, ..desc };
    }
    desc
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

/// The player's volumes (M4 A2): every sound's, and the music's, the effects' and the voices' under it.
fn volumes_of(chosen: &PlayerSettings) -> Volumes {
    let share = PlayerSettings::share;
    Volumes {
        all: chosen.volume(),
        music: share(chosen.music),
        effects: share(chosen.effects),
        voice: share(chosen.voice),
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
    fn metadata_matches_the_identity_file() {
        // xtask bundles and packages the app from platform/identity.toml; the identity SDL sees at runtime
        // (user directories, iOS bundle checks, Linux's window class) must be the same one.
        let identities = include_str!("../../../platform/identity.toml");
        let table = identities.split("\n[").find(|table| table.starts_with("game_app]")).expect("[game_app]");
        let field = |key: &str| {
            table
                .lines()
                .find_map(|line| {
                    let value = line.split_once(key)?.1.trim_start().strip_prefix('=')?.trim_start();
                    value.strip_prefix('"')?.split('"').next()
                })
                .unwrap_or_else(|| panic!("`{key}` missing from [game_app]"))
        };
        assert_eq!(GameApp::METADATA.identifier, field("bundle-id"));
        assert_eq!(GameApp::METADATA.name, field("{ en"), "the English display name");
    }

    /// M4 T3: a Steam Deck starts fullscreen at its panel's size, on Desktop-Low (`sapphire_render`'s
    /// selection test holds the tier); any other desktop in the window it had.
    #[test]
    fn a_steam_deck_starts_at_1280_by_800() {
        use sapphire_platform::Os;
        let info = |handheld| SystemInfo {
            os: Os::Linux,
            device_class: DeviceClass::Desktop,
            ram_mib: 16 * 1024,
            logical_cores: 8,
            is_emulator: false,
            handheld,
        };
        let deck = window_desc("Sapphire", &info(true));
        assert_eq!((deck.size.width, deck.size.height, deck.fullscreen), (1280, 800, true));
        let desk = window_desc("Sapphire", &info(false));
        assert_eq!(desk, WindowDesc { title: "Sapphire".to_owned(), ..WindowDesc::default() });
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
