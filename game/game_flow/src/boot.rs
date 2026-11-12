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
//!    of the boot stands: the content is made again on it, and the show with it ([`BootUpdate::remake`]). The
//!    renderer builds beside it all along. Nothing that goes wrong stops the boot: it goes on with what it has.
//! 1. **The renderer** ([`Step::Renderer`]): `Renderer::new` on the warm-up pool (`sapphire_jobs::spawn_warm_up`),
//!    whose scopes stay on its threads, so its builds never land on a frame. It starts in [`Boot::start`].
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
use game_combat::fight::{FightStand, StandPlan, fight_stage};
use game_ui::boot::{BOOT_SCREEN, BootScreen};
use sapphire_platform::{AppInitError, ContentFs, Host};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::quality::QualitySettings;
use sapphire_render::{
    InterfaceScreen, PipelineInventory, PipelineStats, RenderError, Renderer, UiDrawList, WarmUpRequest,
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
}

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
    update: Option<BootUpdate>,
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
        mut show: GameShow,
        first: &CombatOption,
    ) -> Result<Self, AppInitError> {
        let started = Stopwatch::start();
        let desc = fight_stage(StageDesc::new(host, &gpu, format, quality));
        let (sender, built) = mpsc::channel();
        let (renderer_gpu, settings) = (gpu.clone(), desc.settings);
        sapphire_jobs::spawn_warm_up(move || {
            // Nobody's waiting if the boot ended first.
            let _ = sender.send(Renderer::new(&renderer_gpu, format, settings));
        });
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
            update: None,
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
        if let Some(view) = view {
            self.draw(view, dt)?;
        }
        self.report.frames += 1;
        let took = stopwatch.elapsed();
        if took > self.report.longest_frame {
            self.report.longest_frame = took;
            self.report.longest_in = step;
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
                            self.stand_on(content, remake);
                        }
                        self.report.update = self.started.elapsed();
                        self.step = Step::Renderer;
                    }
                }
            }
            Step::Renderer => match self.take_plan()?.then(|| self.built.try_recv()) {
                None => {}
                Some(Ok(renderer)) => {
                    let mut renderer = renderer?;
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

    /// Makes the content again on `content`, a release an update installed, and the show and its first run on
    /// it with `remake`, in place of what [`Self::start`] made on the release before. If that fails, the boot goes
    /// on with what it had, whose files stay until the next launch confirms.
    fn stand_on(&mut self, content: ContentFs, mut remake: MakeShow) {
        let stopwatch = Stopwatch::start();
        let host = self.host.with_content(content);
        let desc = self.desc.as_ref().expect("a desc until the stage is made");
        let (character, sky) = (desc.character, desc.sky);
        let made = (|| -> Result<_, AppInitError> {
            let (show, first) = remake(host.content())?;
            let mut show = Box::new(show);
            let mut content = Content::new(&host, character, sky)?;
            let combat = show.shown.fight.prepare_stock_combat(&first, &mut content)?;
            let plan = StandPlan::of_combat(&combat).map_err(AppInitError::msg)?;
            Ok((show, content, combat, plan))
        })();
        match made {
            Ok((show, content, combat, plan)) => {
                let zone = plan.zone_stem();
                (self.show, self.content, self.combat) = (Some(show), Some(content), Some(combat));
                (self.planned, self.plan) = (None, Some(plan));
                self.host = host;
                self.report.updated = true;
                tracing::info!(
                    target: LOG_TARGET,
                    elapsed_ms = format_args!("{:.1}", stopwatch.elapsed().as_secs_f64() * 1e3),
                    zone,
                    "the game's content is the update's"
                );
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "the update's content didn't stand; the game goes on with what it had");
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
        let stood = show.stand_fight_part(stand, scene, content, renderer, gpu)?;
        tracing::info!(
            target: LOG_TARGET,
            elapsed_ms = format_args!("{:.1}", stopwatch.elapsed().as_secs_f64() * 1e3),
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
