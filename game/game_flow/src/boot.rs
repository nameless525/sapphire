//! The game's boot (M4 H2): from its first frame to the title, drawing the boot screen
//! ([`game_ui::boot::BootScreen`]) every frame while what the game draws through is built where no frame waits.
//!
//! A renderer builds its passes' pipelines as it's made, about a second of them on Desktop-High's cold Metal
//! cache, and the packs' and the first zone's take seconds more (M4 H0, H1). Until H2 the game made its renderer
//! and prewarmed everything in its `init`, before any frame. Now `init` draws nothing heavier than the
//! interface's one pipeline ([`InterfaceScreen`]), and the rest happens here, a frame at a time, in [`Step`]'s
//! order:
//!
//! 0. **An update** ([`Step::Update`], M4 P4), if the boot has one ([`Boot::with_update`]): the build's host
//!    asked for a newer release, what it adds downloaded and then installed on threads of their own
//!    (`crate::update`), the screen saying which and the bar how far. A release installed here is what the rest
//!    of the boot stands: the show is made again on it ([`BootUpdate::remake`]), then the content, a part a
//!    frame, and its first run is prepared and what it reads of its scenario read on the warm-up pool (M4 Z3e).
//!    The
//!    renderer builds beside it all along. Nothing that goes wrong stops the boot: it goes on with what it has.
//! 1. **The renderer** ([`Step::Renderer`]): `Renderer::new` on the warm-up pool (`sapphire_jobs::spawn_warm_up`),
//!    whose scopes stay on its threads, so its builds never land on a frame. It starts in [`Boot::start`]. If it
//!    can't be built at the boot's tier, here or as the packs' pipelines warm (the renderer refuses, or the
//!    device reports an error: a pipeline its driver wouldn't link, memory it couldn't give), the boot falls back
//!    to the next lower tier of its class and builds it again (M4 H9, [`Quality::fall_back`]); only the lowest's
//!    failure ends the boot. A device lost meanwhile can't be built on again: the window boots on a new one, under
//!    the ceiling [`Boot::ceiling_after_loss`] gives it, since on Vulkan wgpu loses the device when a driver
//!    refuses a pipeline.
//! 2. **The packs' pipelines** ([`Step::Packs`]): what the mounted packs' meshes and the first zone's draw
//!    through, with its probe grid's constants (`Content::zone_pipelines`), skinning, effects and the interface,
//!    asked of the new renderer ([`Renderer::warm`]) and taken in between frames
//!    ([`Renderer::take_in_warm_up`]). From the renderer's arrival, anything built or waited for on the frame's
//!    thread counts as a hitch ([`Renderer::count_hitches_from_now`]): exit test 1 counts from the boot screen's
//!    first frame, which this renderer never drew.
//! 3. **Loading** ([`Step::Stage`], [`Step::Stand`], [`Step::Screens`]): the stage made on that renderer, the
//!    character and the sky uploaded (`Stage::new_with_renderer`); on the frames after, the fight stood on it, in
//!    the zone around where the run starts, a part a frame ([`GameShow::stand_fight_part`], `FightStand`, M4
//!    H4), what it reads of its scenario read on the warm-up pool beside the renderer (`StandPlan`); and on the
//!    next its screens ([`GameShow::stand_screens`]). Their uploads and prewarms find everything built. They're
//!    the frames of reading and uploading, each within twice the tier's interval on an M5, with the screen saying
//!    so from the frame before.
//! 4. **What the show declares** ([`Step::Declared`]): its screen effects' post variants, its sky, its fades and
//!    insets (`Scene::declared_pipelines`), warmed the same way.
//!
//! Then [`Boot::frame`] hands the stage back, and the title is the next frame. [`BootReport`] says what it took.
//!
//! # Adding a step
//!
//! A step is a variant of [`Step`] with a key under `boot-step-` in both languages' `game_ui.ftl`, its share of
//! the bar ([`Step::share`]), and what moves it on in `Boot::advance`. The update goes first, before
//! [`Step::Renderer`], which keeps building beside it, and ends before [`Step::Packs`] reads what the mounted
//! packs list, so packs a patch placed over the shipped ones are what warms and stands. The boot screen's own
//! strings and fonts are the ones the game started on, read in [`Boot::start`]: an update installed under it
//! shows from the title on.

use std::num::NonZeroU32;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use game_combat::combat::{Combat, CombatOption};
use game_combat::fight::{FightStand, PreparedRun, StandPlan, fight_stage};
use game_ui::boot::{BOOT_SCREEN, BootScreen};
use sapphire_platform::{AppInitError, ContentFs, Host};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::quality::{Quality, QualitySettings, TierId};
use sapphire_render::{
    InterfaceScreen, PipelineInventory, PipelineStats, RenderError, RenderSettings, Renderer, UiDrawList,
    WarmUpRequest,
};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_stage::{Content, Stage, StageDesc};
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::show::GameShow;
use crate::update::{Update, UpdateState};

/// The update's share of the bar once it downloads: a check that finds nothing takes none, and the steps after
/// share the rest as they would without it.
const UPDATE_SHARE: f32 = 0.3;
/// Each step's share of the boot screen's bar ([`Step::share`]), roughly what each took of a cold boot on Metal.
const RENDERER_SHARE: f32 = 0.15;
const PACKS_SHARE: f32 = 0.65;
const STAND_SHARE: f32 = 0.05;

/// Frames a second where a tier has no cap: a window's usual vsync.
const DEFAULT_RATE: u32 = 60;

/// What the boot is doing: see the [module docs](self).
#[derive(Debug)]
pub enum Step {
    /// An update from the build's host, while the renderer builds.
    Update(UpdateStage),
    /// The renderer's own pipelines, built with it on the warm-up pool.
    Renderer,
    /// The packs' and the first zone's pipelines, warming.
    Packs(WarmUpRequest),
    /// The stage made on the renderer, on the next frame.
    Stage,
    /// The fight standing on the stage, on the next frame.
    Stand,
    /// The show's screens loading, on the next frame.
    Screens,
    /// What the stood show declares, warming.
    Declared(WarmUpRequest),
    /// The stage's ready, and handed back.
    Done,
}

/// Where the boot's update is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStage {
    /// Asking the host for its release.
    Checking,
    /// Fetching what a newer release adds.
    Downloading,
    /// Assembling it beside the current release.
    Installing,
}

impl Step {
    /// Its key, under `boot-step-`, for the screen.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            Self::Update(UpdateStage::Checking) => "boot-step-update-check",
            Self::Update(UpdateStage::Downloading) => "boot-step-update",
            Self::Update(UpdateStage::Installing) => "boot-step-update-install",
            Self::Renderer | Self::Packs(_) | Self::Declared(_) | Self::Done => "boot-step-shaders",
            Self::Stage | Self::Stand | Self::Screens => "boot-step-loading",
        }
    }

    /// Its share of the bar, in order: the pipelines are most of a cold boot, and loading is a frame. An update
    /// that downloads takes 30 % of it first, and the rest share what's left.
    #[must_use]
    pub const fn share(&self) -> f32 {
        match self {
            Self::Update(_) => UPDATE_SHARE,
            Self::Renderer => RENDERER_SHARE,
            Self::Packs(_) => PACKS_SHARE,
            Self::Stage | Self::Stand | Self::Screens => STAND_SHARE / 3.0,
            Self::Declared(_) => 1.0 - RENDERER_SHARE - PACKS_SHARE - STAND_SHARE,
            Self::Done => 0.0,
        }
    }

    /// Its name, for the log.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Update(_) => "update",
            Self::Renderer => "renderer",
            Self::Packs(_) => "packs",
            Self::Stage => "stage",
            Self::Stand => "stand",
            Self::Screens => "screens",
            Self::Declared(_) => "declared",
            Self::Done => "done",
        }
    }
}

/// What a boot took, logged as it ends ([`Boot::report`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BootReport {
    /// Boot screen frames drawn, or prepared with nowhere to draw.
    pub frames: u32,
    /// From [`Boot::start`] to its update's end, and the bytes it received from the host: zero with none.
    pub update: Duration,
    pub update_received: u64,
    /// Whether the update installed a release, and the boot stood it.
    pub updated: bool,
    /// From [`Boot::start`] to each step's end: the renderer made, the packs warm, the stage stood, all warm.
    pub renderer: Duration,
    pub packs: Duration,
    pub stood: Duration,
    pub warm: Duration,
    /// The longest a [`Boot::frame`] took, and in which step: its own work, short of what presenting waits.
    pub longest_frame: Duration,
    pub longest_in: &'static str,
    /// How much of that frame drew the screen (its uploads, recording and submit) rather than moved the boot on.
    /// A frame whose driver holds its calls behind the warm-up's compiles is nearly all drawing: the Android
    /// emulator's, whose GPU calls queue behind its host's (M4 H6).
    pub longest_drawing: Duration,
    /// The longest from one frame's start to the next's, from the second frame: what the player sees, with
    /// whatever presenting waited for.
    pub longest_interval: Duration,
    /// Frames whose interval passed twice the tier's (exit test 1's limit for a load screen's frame), and that
    /// limit.
    pub intervals_over: u32,
    pub interval_limit: Duration,
    /// The stage's renderer's builds as the boot hands it on: `frame_thread_builds` and `frame_waits` 0 when no
    /// frame built or waited for a pipeline.
    pub pipelines: PipelineStats,
    /// The boot screen's own: its one pipeline, built before its first frame.
    pub screen_pipelines: u32,
    /// The tiers whose renderer couldn't be built, each fallen back from to the next lower (M4 H9): none, on a
    /// device that builds the tier it chose.
    pub fell_back_from: Vec<TierId>,
}

/// What makes a boot's renderer on the warm-up pool, for a tier: `Renderer::new`, or a test's that refuses one
/// (M4 H9, [`Boot::start_making`]).
pub type MakeRenderer = Arc<
    dyn Fn(&GpuContext, wgpu::TextureFormat, TierId, RenderSettings) -> Result<Renderer, RenderError>
        + Send
        + Sync,
>;

/// What a boot makes its show from: the content it stands on, read again when an update installs a release.
pub type MakeShow = Box<dyn FnMut(&ContentFs) -> Result<(GameShow, CombatOption), AppInitError>>;

/// An update for a boot to run first ([`Boot::with_update`]).
pub struct BootUpdate {
    /// The download, from `Patches::check`.
    pub update: Update,
    /// The show and its first run, made again on the content a release installed under the boot screen: what
    /// made the one [`Boot::start`] took, on the new content.
    pub remake: MakeShow,
}

/// The game booting: see the [module docs](self).
pub struct Boot {
    gpu: Arc<GpuContext>,
    /// The device, and the content the game stands on.
    host: Host,
    step: Step,
    /// The renderer, building on the warm-up pool until [`Step::Renderer`] takes it.
    built: mpsc::Receiver<Result<Renderer, RenderError>>,
    make: MakeRenderer,
    /// The device's errors when the renderer's build started ([`GpuContext::errors`]): one since, and what it
    /// built can't be trusted.
    errors_at_build: u32,
    update: Option<BootUpdate>,
    /// A release the update installed, made again a part a frame until it's stood on.
    remaking: Option<Box<Remaking>>,
    /// The bar's share the update took: [`UPDATE_SHARE`] once it downloads, else none.
    update_share: f32,
    /// How far its download has got, 0 to 1.
    downloaded: f32,
    /// What stands, until it does.
    desc: Option<StageDesc>,
    content: Option<Content>,
    show: Option<Box<GameShow>>,
    combat: Option<Combat>,
    /// What the first run's stand reads of its scenario: on the warm-up pool, then here (M4 H4).
    planned: Option<mpsc::Receiver<Result<StandPlan, String>>>,
    plan: Option<StandPlan>,
    /// The first run, standing a part a frame.
    stand: Option<FightStand>,
    /// What frames draw into.
    format: wgpu::TextureFormat,
    frame_rate: Option<NonZeroU32>,
    renderer: Option<Box<Renderer>>,
    stage: Option<Box<Stage<GameShow>>>,
    screen: BootScreen,
    interface: InterfaceScreen,
    executor: GraphExecutor,
    list: UiDrawList,
    /// The bar as shown, which only goes forward.
    shown: f32,
    started: Stopwatch,
    /// Since the last frame started, from the first.
    since_frame: Option<Stopwatch>,
    report: BootReport,
}

impl Boot {
    /// Starts booting `show` with `first`, its first run, on `gpu`, for targets of `format`, with the player's
    /// `quality`: the content's packs mounted and the character and sky read, the first run's simulation
    /// prepared, the boot screen loaded with its pipeline built, and the renderer building on the warm-up pool.
    /// What the first frame needs and nothing more.
    pub fn start(
        host: &Host,
        gpu: Arc<GpuContext>,
        format: wgpu::TextureFormat,
        quality: QualitySettings,
        show: GameShow,
        first: &CombatOption,
    ) -> Result<Self, AppInitError> {
        let make: MakeRenderer = Arc::new(|gpu, format, _, settings| Renderer::new(gpu, format, settings));
        Self::start_making(host, gpu, format, quality, show, first, make)
    }

    /// [`Self::start`], with its renderer made by `make` for each tier it tries: a test's, which refuses one to
    /// see the boot fall back (M4 H9).
    pub fn start_making(
        host: &Host,
        gpu: Arc<GpuContext>,
        format: wgpu::TextureFormat,
        quality: QualitySettings,
        mut show: GameShow,
        first: &CombatOption,
        make: MakeRenderer,
    ) -> Result<Self, AppInitError> {
        let started = Stopwatch::start();
        let desc = fight_stage(StageDesc::new(host, &gpu, format, quality));
        let errors_at_build = gpu.errors();
        let built = build_renderer(&gpu, format, &desc.quality, desc.settings, &make);
        let mut content = Content::new(host, desc.character, desc.sky)?;
        let combat = show.shown.fight.prepare_stock_combat(first, &mut content)?;
        // Read beside the renderer's build: its documents are tens of milliseconds of a frame (M4 H4).
        let (scenario, party) = (combat.scenario().to_owned(), combat.simulation().party().is_formed());
        let (plan_sender, planned) = mpsc::channel();
        sapphire_jobs::spawn_warm_up(move || {
            let _ = plan_sender.send(StandPlan::of(&scenario, party));
        });
        let assets = content.load_screens_alone(&[BOOT_SCREEN])?;
        let screen = BootScreen::new(assets, show.language()).map_err(AppInitError::msg)?;
        let mut interface = InterfaceScreen::new(&gpu, format);
        let [r, g, b] = game_ui::boot::CLEAR.map(|code| f64::from(srgb_to_linear(code)));
        interface.set_clear(wgpu::Color { r, g, b, a: 1.0 });
        let frame_rate = desc.quality.frame_rate();
        let rate = frame_rate.map_or(DEFAULT_RATE, NonZeroU32::get);
        let report = BootReport {
            screen_pipelines: interface.pipelines().built(),
            interval_limit: Duration::from_secs(2) / rate,
            ..BootReport::default()
        };
        tracing::info!(
            target: LOG_TARGET,
            elapsed_ms = format_args!("{:.1}", started.elapsed().as_secs_f64() * 1e3),
            "the boot screen is up; the renderer builds off the frame"
        );
        Ok(Self {
            gpu,
            host: host.clone(),
            step: Step::Renderer,
            built,
            make,
            errors_at_build,
            update: None,
            remaking: None,
            update_share: 0.0,
            downloaded: 0.0,
            desc: Some(desc),
            content: Some(content),
            show: Some(Box::new(show)),
            combat: Some(combat),
            planned: Some(planned),
            plan: None,
            stand: None,
            format,
            frame_rate,
            renderer: None,
            stage: None,
            screen,
            interface,
            executor: GraphExecutor::new(),
            list: UiDrawList::default(),
            shown: 0.0,
            started,
            since_frame: None,
            report,
        })
    }

    /// Runs `update` before anything else, under its screen (see the [module docs](self)).
    #[must_use]
    pub fn with_update(mut self, update: BootUpdate) -> Self {
        self.step = Step::Update(UpdateStage::Checking);
        self.update = Some(update);
        self
    }

    /// The device and the content the game stands on: a release an update installed, once it has.
    #[must_use]
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// The device it boots on, the stage's from then on.
    #[must_use]
    pub fn gpu(&self) -> &Arc<GpuContext> {
        &self.gpu
    }

    /// The tier's frame rate, which a window paces its frames to from the boot screen's first (spec §6.2).
    #[must_use]
    pub const fn frame_rate(&self) -> Option<NonZeroU32> {
        self.frame_rate
    }

    /// What it's doing.
    #[must_use]
    pub fn step(&self) -> &Step {
        &self.step
    }

    /// What it's taken so far, and all of it once it's handed the stage back.
    #[must_use]
    pub fn report(&self) -> &BootReport {
        &self.report
    }

    /// The tier it boots at: the one it chose, or the one it fell back to (M4 H9).
    #[must_use]
    pub fn tier(&self) -> TierId {
        self.desc.as_ref().map_or_else(
            || self.stage.as_ref().expect("a stage once the desc is used").quality.active().id,
            |desc| desc.quality.active().id,
        )
    }

    /// The ceiling a boot on a new device takes ([`QualitySettings::ceiling`]) once this one's device is lost
    /// (M4 H9). Lost while the renderer built, its own pipelines or the packs', it's the next lower tier: on Vulkan
    /// wgpu loses the device when the driver refuses a pipeline or has no memory for one, so building the same
    /// tier on a new device would lose that too. Lost later, it's the ceiling this boot had, if any. At its
    /// class's lowest it can go no lower, and the window's limit on losses in a row ends the game.
    #[must_use]
    pub fn ceiling_after_loss(&self) -> Option<TierId> {
        let building = matches!(self.step, Step::Update(_) | Step::Renderer | Step::Packs(_));
        let quality = self.desc.as_ref().map(|desc| &desc.quality);
        let quality = quality.or_else(|| self.stage.as_ref().map(|stage| &stage.quality))?;
        let lower = building.then(|| quality.active().id.lower()).flatten();
        lower.or(quality.settings().ceiling)
    }

    /// The renderer, once it's built and until the stage is handed back: its ledger says what a frame built or
    /// waited for (exit test 1's counts), so a lane can name the step a hitch came in.
    #[must_use]
    pub fn renderer(&self) -> Option<&Renderer> {
        self.renderer.as_deref().or(self.stage.as_ref().map(|stage| &stage.renderer))
    }

    /// A frame of the boot, `dt` real seconds after the last: moves it on, then draws its screen into `view`,
    /// if there's one to draw into. Hands the stage back on the frame it's ready, stood and warm, after which the
    /// title draws; `None` until then.
    pub fn frame(
        &mut self,
        view: Option<&wgpu::TextureView>,
        dt: f32,
    ) -> Result<Option<Box<Stage<GameShow>>>, AppInitError> {
        let stopwatch = Stopwatch::start();
        if let Some(interval) = self.since_frame.replace(stopwatch).map(|since| since.elapsed()) {
            self.report.longest_interval = self.report.longest_interval.max(interval);
            self.report.intervals_over += u32::from(interval > self.report.interval_limit);
        }
        // Nothing more goes to a lost device (M4 H4): what's made or written on it fails, and a staging write
        // panics. The app boots again on a new one.
        if self.gpu.is_lost() {
            return Err(AppInitError::msg("the GPU device was lost while the game booted"));
        }
        let step = self.step.name();
        self.advance()?;
        let drawing = Stopwatch::start();
        if let Some(view) = view {
            self.draw(view, dt)?;
        }
        let drawing = drawing.elapsed();
        self.report.frames += 1;
        let took = stopwatch.elapsed();
        if took > self.report.longest_frame {
            self.report.longest_frame = took;
            self.report.longest_in = step;
            self.report.longest_drawing = drawing;
        }
        if !matches!(self.step, Step::Done) {
            return Ok(None);
        }
        // Uploads to a lost device do nothing, so what stood on it holds dead objects (as `stand_game` says).
        if self.gpu.is_lost() {
            return Err(AppInitError::msg("the GPU device was lost while the game booted"));
        }
        let stage = self.stage.take().expect("a boot that's done has its stage");
        self.report.pipelines = stage.renderer.stats().pipelines;
        let report = &self.report;
        let ms = |duration: Duration| format_args!("{:.1}", duration.as_secs_f64() * 1e3).to_string();
        tracing::info!(
            target: LOG_TARGET,
            frames = report.frames,
            update_ms = ms(report.update),
            update_received = report.update_received,
            updated = report.updated,
            renderer_ms = ms(report.renderer),
            packs_ms = ms(report.packs),
            stood_ms = ms(report.stood),
            warm_ms = ms(report.warm),
            longest_frame_ms = ms(report.longest_frame),
            longest_in = report.longest_in,
            longest_drawing_ms = ms(report.longest_drawing),
            longest_interval_ms = ms(report.longest_interval),
            intervals_over = report.intervals_over,
            interval_limit_ms = ms(report.interval_limit),
            prewarmed = report.pipelines.prewarmed,
            warmed_off_frame = report.pipelines.warmed_off_frame,
            first_use = report.pipelines.first_use_builds,
            frame_thread = report.pipelines.frame_thread_builds,
            frame_waits = report.pipelines.frame_waits,
            "booted"
        );
        Ok(Some(stage))
    }

    /// The app is back from the background (spec §6.5): the time away was no frame's interval. The warm-up pool
    /// went on building while it was away, and the next frame takes in what it built.
    pub fn restart_clock(&mut self) {
        self.since_frame = None;
    }

    /// Boots to the end with no window, for a capture or a test: each frame drawn into a target of `size` pixels,
    /// at the tier's rate if `paced` (a window's vsync, for a run that times its frames), else as fast as they
    /// come.
    pub fn run_offscreen(
        mut self,
        size: (u32, u32),
        paced: bool,
    ) -> Result<(Box<Stage<GameShow>>, BootReport), AppInitError> {
        let pace = paced.then_some(self.report.interval_limit / 2);
        let target = self.gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: self.gpu.label("boot screen target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let dt = pace.unwrap_or(Duration::from_secs(1) / DEFAULT_RATE);
        let mut frame = Stopwatch::start();
        loop {
            if let Some(stage) = self.frame(Some(&view), dt.as_secs_f32())? {
                return Ok((stage, self.report));
            }
            if let Some(pace) = pace {
                std::thread::sleep(pace.saturating_sub(frame.elapsed()));
            }
            frame.lap();
        }
    }

    /// Moves the boot on as far as this frame can without waiting.
    fn advance(&mut self) -> Result<(), AppInitError> {
        match &self.step {
            Step::Update(_) if self.remaking.is_some() => {
                if self.remake_part() {
                    self.report.update = self.started.elapsed();
                    self.step = Step::Renderer;
                }
            }
            Step::Update(_) => {
                let update = &mut self.update.as_mut().expect("an update while it's the step").update;
                let state = update.poll();
                self.report.update_received = update.received();
                match state {
                    UpdateState::Checking => {}
                    UpdateState::Downloading(fraction) => {
                        self.update_share = UPDATE_SHARE;
                        self.downloaded = fraction;
                        self.step = Step::Update(UpdateStage::Downloading);
                    }
                    UpdateState::Installing => {
                        self.downloaded = 1.0;
                        self.step = Step::Update(UpdateStage::Installing);
                    }
                    UpdateState::Done(installed) => {
                        let BootUpdate { remake, .. } =
                            self.update.take().expect("an update while it's the step");
                        if let Some(content) = installed {
                            // From the next frame, under the install's words.
                            let host = self.host.with_content(content);
                            let made = Remade::Nothing;
                            let stopwatch = Stopwatch::start();
                            self.remaking = Some(Box::new(Remaking { host, remake, made, stopwatch }));
                        } else {
                            self.report.update = self.started.elapsed();
                            self.step = Step::Renderer;
                        }
                    }
                }
            }
            Step::Renderer => match self.take_plan()?.then(|| self.built.try_recv()) {
                None => {}
                Some(Ok(built)) => {
                    let mut renderer = match (built, self.errors_since_build()) {
                        (Ok(renderer), 0) => renderer,
                        (Ok(_), errors) => {
                            return self.fall_back(&format!("the device reported {errors} errors"));
                        }
                        (Err(error), _) => return self.fall_back(&error.to_string()),
                    };
                    renderer.count_hitches_from_now();
                    self.report.renderer = self.started.elapsed();
                    let content = self.content.as_mut().expect("content until the stage stands");
                    // The first zone's pack, mounted, so the packs' keys are its too.
                    let zone = self.plan.as_ref().expect("taken above").zone_stem();
                    let mut inventory = content.zone_pipelines(zone)?;
                    inventory.add(&PipelineInventory {
                        skinning: true,
                        effects: true,
                        ui: true,
                        ..content.pipeline_inventory()
                    });
                    self.step = Step::Packs(renderer.warm(&inventory));
                    self.renderer = Some(Box::new(renderer));
                }
                Some(Err(mpsc::TryRecvError::Empty)) => {}
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    return Err(AppInitError::msg("the renderer's build on the warm-up pool panicked"));
                }
            },
            &Step::Packs(request) => {
                let renderer = self.renderer.as_mut().expect("a renderer warms");
                renderer.take_in_warm_up();
                if renderer.is_warm(request) {
                    // Its pipelines are the tier's: one the device refused, and the tier is no good.
                    let errors = self.errors_since_build();
                    if errors > 0 {
                        return self
                            .fall_back(&format!("the device reported {errors} errors warming the packs"));
                    }
                    self.report.packs = self.started.elapsed();
                    // Made on the next frame, so the screen says it's loading while it does.
                    self.step = Step::Stage;
                }
            }
            Step::Stage => {
                self.make_stage()?;
                self.step = Step::Stand;
            }
            Step::Stand => {
                if self.stand()? {
                    self.step = Step::Screens;
                }
            }
            Step::Screens => {
                self.stand_screens()?;
                self.report.stood = self.started.elapsed();
                let stage = self.stage.as_deref_mut().expect("a stood stage");
                let declared = stage.scene.declared_pipelines(&stage.show);
                self.step = Step::Declared(stage.renderer.warm(&declared));
            }
            &Step::Declared(request) => {
                let renderer = &mut self.stage.as_mut().expect("a stage warms").renderer;
                renderer.take_in_warm_up();
                if renderer.is_warm(request) {
                    self.report.warm = self.started.elapsed();
                    self.step = Step::Done;
                }
            }
            Step::Done => {}
        }
        Ok(())
    }

    /// Errors the device has reported since the renderer's build started ([`GpuContext::errors`]).
    fn errors_since_build(&self) -> u32 {
        self.gpu.errors().saturating_sub(self.errors_at_build)
    }

    /// The renderer couldn't be built at the boot's tier, for `why` (M4 H9): on a live device, the boot falls back
    /// to the next lower tier of its class and builds its renderer again, from [`Step::Renderer`], letting go of
    /// what it had built; at the class's lowest it fails. A lost device fails it either way, for the window to
    /// boot again on a new one ([`Self::ceiling_after_loss`]).
    fn fall_back(&mut self, why: &str) -> Result<(), AppInitError> {
        if self.gpu.is_lost() {
            return Err(AppInitError::msg(format!("the GPU device was lost while the game booted: {why}")));
        }
        let desc = self.desc.as_mut().expect("a desc until the stage is made");
        let from = desc.quality.active().id;
        let Some(tier) = desc.quality.fall_back() else {
            return Err(AppInitError::msg(format!(
                "the renderer can't be built at {from}, the lowest tier of its class: {why}"
            )));
        };
        tracing::warn!(
            target: LOG_TARGET,
            tier = tier.name(),
            from = from.name(),
            why,
            "the renderer can't be built at the boot's tier: it builds again at the next lower"
        );
        desc.settings = desc.quality.render_settings();
        self.frame_rate = desc.quality.frame_rate();
        let rate = self.frame_rate.map_or(DEFAULT_RATE, NonZeroU32::get);
        self.report.interval_limit = Duration::from_secs(2) / rate;
        self.report.fell_back_from.push(from);
        // Its warm-up, if any, goes on to its end with nobody waiting.
        self.renderer = None;
        self.errors_at_build = self.gpu.errors();
        self.built = build_renderer(&self.gpu, self.format, &desc.quality, desc.settings, &self.make);
        self.step = Step::Renderer;
        Ok(())
    }

    /// Takes the release an update installed a part on ([`Remaking`]): the show and its first run made again on
    /// it with `remake`, then the content, then the run, prepared and with what it reads of its scenario read on
    /// the warm-up pool, in place of what [`Self::start`] made on the release before. Each is a frame's: no frame of the boot
    /// screen held them all until M4 Z3e (one of 100 to 250 ms). True once it's done, stood on or not: if a part
    /// fails, the boot goes on with what it had, whose files stay until the next launch confirms.
    fn remake_part(&mut self) -> bool {
        let part = Stopwatch::start();
        let remaking = self.remaking.as_deref_mut().expect("a remake while it's taken on");
        let desc = self.desc.as_ref().expect("a desc until the stage is made");
        let (character, sky) = (desc.character, desc.sky);
        let before = remaking.made.name();
        let made = match std::mem::replace(&mut remaking.made, Remade::Nothing) {
            Remade::Nothing => (remaking.remake)(remaking.host.content())
                .map(|(show, first)| Remade::Show(Box::new(show), first)),
            Remade::Show(show, first) => Content::new(&remaking.host, character, sky)
                .map(|content| Remade::Content(show, first, Box::new(content))),
            Remade::Content(show, first, content) => {
                // Its setup and its scenario's documents, 65 ms of a frame on an M5, on the warm-up pool.
                let prepared = show.shown.fight.prepare_run_off_frame(&first);
                Ok(Remade::Preparing(show, first, content, prepared))
            }
            Remade::Preparing(mut show, first, mut content, prepared) => {
                let combat = match prepared.try_recv() {
                    Ok(PreparedRun::Ready(combat)) => Ok(Some(*combat)),
                    // The stock arena's fight, whose setup reads content: a build with no flow's.
                    Ok(PreparedRun::OnTheFrame) => {
                        show.shown.fight.prepare_stock_combat(&first, &mut content).map(Some)
                    }
                    Ok(PreparedRun::Failed(error)) => Err(AppInitError::msg(error)),
                    Err(mpsc::TryRecvError::Empty) => Ok(None),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Err(AppInitError::msg("the run's preparation on the warm-up pool panicked"))
                    }
                };
                combat.map(|combat| match combat {
                    Some(combat) => {
                        // Read beside the renderer's build, as `start`'s is: tens of milliseconds of documents.
                        let (scenario, party) =
                            (combat.scenario().to_owned(), combat.simulation().party().is_formed());
                        let (sender, planned) = mpsc::channel();
                        sapphire_jobs::spawn_warm_up(move || {
                            let _ = sender.send(StandPlan::of(&scenario, party));
                        });
                        Remade::Run(show, content, combat, planned)
                    }
                    None => Remade::Preparing(show, first, content, prepared),
                })
            }
            Remade::Run(show, content, combat, planned) => match planned.try_recv() {
                Ok(Ok(plan)) => Ok(Remade::Planned(show, content, combat, plan)),
                Ok(Err(error)) => Err(AppInitError::msg(error)),
                Err(mpsc::TryRecvError::Empty) => Ok(Remade::Run(show, content, combat, planned)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(AppInitError::msg("the stand's plan on the warm-up pool panicked"))
                }
            },
            Remade::Planned(..) => unreachable!("a remake that's done isn't taken on"),
        };
        let took = format_args!("{:.1}", part.elapsed().as_secs_f64() * 1e3).to_string();
        match made {
            Ok(Remade::Planned(show, content, combat, plan)) => {
                let Remaking { host, stopwatch, .. } = *self.remaking.take().expect("taken on above");
                let zone = plan.zone_stem();
                (self.show, self.content, self.combat) = (Some(show), Some(*content), Some(combat));
                (self.planned, self.plan) = (None, Some(plan));
                self.host = host;
                self.report.updated = true;
                tracing::info!(
                    target: LOG_TARGET,
                    elapsed_ms = format_args!("{:.1}", stopwatch.elapsed().as_secs_f64() * 1e3),
                    zone,
                    "the game's content is the update's"
                );
                true
            }
            Ok(made) => {
                // Once a part, not each frame it waits for the warm-up pool.
                if made.name() != before {
                    tracing::info!(target: LOG_TARGET, elapsed_ms = took, made = made.name(), "the update's release is made again in part");
                }
                self.remaking.as_deref_mut().expect("taken on above").made = made;
                false
            }
            Err(error) => {
                self.remaking = None;
                tracing::warn!(target: LOG_TARGET, %error, "the update's content didn't stand; the game goes on with what it had");
                true
            }
        }
    }

    /// Makes the stage on the warm renderer, with the character and the sky. Never inlined: the stage it builds
    /// by value would sit in [`Self::frame`]'s frame, under every step, the update's install among them (M4 H4).
    #[inline(never)]
    fn make_stage(&mut self) -> Result<(), AppInitError> {
        let stopwatch = Stopwatch::start();
        let (Some(desc), Some(content), Some(show), Some(renderer)) =
            (self.desc.take(), self.content.take(), self.show.take(), self.renderer.take())
        else {
            unreachable!("a boot makes its stage once, with everything it was started with");
        };
        let stage = Stage::new_with_renderer(self.gpu.clone(), desc, content, *renderer)?.with_show(*show);
        self.stage = Some(Box::new(stage));
        tracing::info!(
            target: LOG_TARGET,
            elapsed_ms = format_args!("{:.1}", stopwatch.elapsed().as_secs_f64() * 1e3),
            "the game's stage is made under the boot screen"
        );
        Ok(())
    }

    /// Takes the stand's plan from the warm-up pool once it's read, without waiting. True once it's here.
    fn take_plan(&mut self) -> Result<bool, AppInitError> {
        let Some(planned) = &self.planned else { return Ok(true) };
        match planned.try_recv() {
            Ok(plan) => {
                self.plan = Some(plan.map_err(AppInitError::msg)?);
                self.planned = None;
                Ok(true)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(AppInitError::msg("the stand's plan on the warm-up pool panicked"))
            }
        }
    }

    /// Stands a part of the fight on the stage, in the zone around where it starts ([`FightStand`]): a part a
    /// frame, so no frame of the screen holds all of it (M4 H4). True once it's stood.
    fn stand(&mut self) -> Result<bool, AppInitError> {
        let stopwatch = Stopwatch::start();
        let stage = self.stage.as_deref_mut().expect("a stage to stand on");
        let Stage { show, scene, content, renderer, gpu, quality, .. } = stage;
        let stand = match &mut self.stand {
            Some(stand) => stand,
            None => {
                let combat =
                    self.combat.take().expect("a boot stands once, with the run it was started with");
                let plan = self.plan.take().expect("the plan's taken before the renderer");
                self.stand.insert(FightStand::over_frames(combat, plan, quality.active().id))
            }
        };
        let part = stand.next_part();
        let stood = show.stand_fight_part(stand, scene, content, renderer, gpu)?;
        tracing::info!(
            target: LOG_TARGET,
            elapsed_ms = format_args!("{:.1}", stopwatch.elapsed().as_secs_f64() * 1e3),
            part,
            stood,
            "a part of the game's fight is stood under the boot screen"
        );
        if stood {
            self.stand = None;
        }
        Ok(stood)
    }

    /// Loads the show's screens, on the title.
    fn stand_screens(&mut self) -> Result<(), AppInitError> {
        let stopwatch = Stopwatch::start();
        let stage = self.stage.as_deref_mut().expect("a stage to stand on");
        let Stage { show, content, renderer, gpu, quality, .. } = stage;
        show.stand_screens(content, renderer, gpu, quality.active().id)?;
        tracing::info!(
            target: LOG_TARGET,
            elapsed_ms = format_args!("{:.1}", stopwatch.elapsed().as_secs_f64() * 1e3),
            "the game's screens are loaded under the boot screen"
        );
        Ok(())
    }

    /// Draws the boot screen into `view`.
    fn draw(&mut self, view: &wgpu::TextureView, dt: f32) -> Result<(), AppInitError> {
        let progress = self.progress();
        // Forward only, and eased, since a step's warm-up says how far it's got in a few big steps.
        self.shown = (self.shown + (progress - self.shown).max(0.0) * (dt * 6.0).min(1.0)).max(self.shown);
        self.screen.show(self.step.key(), self.shown);
        self.screen.follow_pages(self.interface.page_state());
        let texture = view.texture();
        let size = (texture.width(), texture.height());
        self.list.reset();
        self.screen.draw(size, dt, &mut self.list);
        self.interface.prepare(&self.gpu, &self.list);
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("boot screen", view);
        self.interface.record(&mut graph, target, size);
        self.executor.execute(&self.gpu, graph).map_err(|error| AppInitError::msg(error.to_string()))
    }

    /// How much of the boot is done, 0 to 1: the steps before this one, and how far this one has got. A step
    /// that warms says so ([`Renderer::warm_up`]); the renderer's making and the stand say nothing until done.
    fn progress(&self) -> f32 {
        let warmed =
            |renderer: Option<&Renderer>| renderer.map_or(0.0, |renderer| renderer.warm_up().fraction());
        let (before, done) = match &self.step {
            Step::Update(_) => return self.update_share * self.downloaded,
            Step::Renderer => (0.0, 0.0),
            Step::Packs(_) => (RENDERER_SHARE, warmed(self.renderer.as_deref())),
            Step::Stage => (RENDERER_SHARE + PACKS_SHARE, 0.0),
            Step::Stand => (RENDERER_SHARE + PACKS_SHARE + STAND_SHARE / 3.0, 0.0),
            Step::Screens => (RENDERER_SHARE + PACKS_SHARE + STAND_SHARE * 2.0 / 3.0, 0.0),
            Step::Declared(_) => (
                RENDERER_SHARE + PACKS_SHARE + STAND_SHARE,
                warmed(self.stage.as_ref().map(|stage| &stage.renderer)),
            ),
            Step::Done => return 1.0,
        };
        self.update_share + (1.0 - self.update_share) * (before + self.step.share() * done)
    }
}

/// A release an update installed, made again under the boot screen a part a frame ([`Boot::remake_part`]).
struct Remaking {
    /// The device, with the release's content.
    host: Host,
    remake: MakeShow,
    made: Remade,
    stopwatch: Stopwatch,
}

/// How far a [`Remaking`] has got.
enum Remade {
    Nothing,
    /// The show and its first run.
    Show(Box<GameShow>, CombatOption),
    /// The content, its packs mounted and the character asked for.
    Content(Box<GameShow>, CombatOption, Box<Content>),
    /// The run, preparing on the warm-up pool.
    Preparing(Box<GameShow>, CombatOption, Box<Content>, mpsc::Receiver<PreparedRun>),
    /// The run, prepared, with its plan reading on the warm-up pool.
    Run(Box<GameShow>, Box<Content>, Combat, mpsc::Receiver<Result<StandPlan, String>>),
    /// All of it.
    Planned(Box<GameShow>, Box<Content>, Combat, StandPlan),
}

impl Remade {
    /// What's made, for the log.
    const fn name(&self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Show(..) => "show",
            Self::Content(..) => "content",
            Self::Preparing(..) => "preparing",
            Self::Run(..) => "run",
            Self::Planned(..) => "plan",
        }
    }
}

/// Starts making the renderer `quality`'s active tier is made with, `settings`, on the warm-up pool, with `make`.
fn build_renderer(
    gpu: &Arc<GpuContext>,
    format: wgpu::TextureFormat,
    quality: &Quality,
    settings: RenderSettings,
    make: &MakeRenderer,
) -> mpsc::Receiver<Result<Renderer, RenderError>> {
    let (sender, built) = mpsc::channel();
    let (gpu, tier, make) = (gpu.clone(), quality.active().id, make.clone());
    sapphire_jobs::spawn_warm_up(move || {
        // Nobody's waiting if the boot ended first.
        let _ = sender.send(make(&gpu, format, tier, settings));
    });
    built
}

/// A display-encoded sRGB code in linear light, for the clear under the boot screen.
fn srgb_to_linear(code: u8) -> f32 {
    let encoded = f32::from(code) / 255.0;
    if encoded <= 0.04045 { encoded / 12.92 } else { ((encoded + 0.055) / 1.055).powf(2.4) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A boot moves by value through `Result`s and the app's frames while it boots, and a release it installs
    /// stands its show from inside them, on the thread SDL runs the app on: 1 MiB on iOS and about that on
    /// Android. At 116 KiB, with the show and the stage in it, the install's launch overflowed an Android
    /// emulator's (M4 H4, exit test 2). Box the next field rather than raise this.
    #[test]
    fn a_boot_is_small_enough_to_move_on_the_main_thread() {
        let size = size_of::<Boot>();
        assert!(size <= 16 * 1024, "a boot takes {size} bytes");
    }
}
