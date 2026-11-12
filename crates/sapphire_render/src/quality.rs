//! Quality tiers (spec §3.1, §5.12 "Quality tiers", M1 R6): the tier table, choosing a tier for
//! the device, and the render scale every frame, which dynamic resolution can lower.
//!
//! # The table
//!
//! `tiers.ron`, next to this crate's manifest, is a [`TierTable`] document (schema 1) holding one
//! [`QualityTier`] for each tier of spec §3.1: Desktop-High, Desktop-Low, Mobile-High and
//! Mobile-Low. It is compiled into the renderer ([`TierTable::shipped`]) rather than cooked into
//! content: the app can't draw a frame without a tier, and a table in the binary can't be
//! missing, stale or unreadable on a device, while a unit test proves it parses and validates.
//! Hot reload and live link (A8, A9) can later hand an edited table to [`TierTable::from_ron`],
//! which validates it the same way; a [`TierTable`] is valid however it was made.
//!
//! Every knob names the package that reads it. Drawn from today:
//!
//! - **Render scale** ([`Resolution`]): the scene's size as a fraction of the output, capped in
//!   height (§3.1's internal resolutions: 1440 rows on Desktop-High, 1080 on Desktop-Low, "~1280×590
//!   → native" on any phone), and the floor dynamic resolution may lower it to.
//!   [`Quality::render_scale`] turns it into [`Renderer::set_render_scale`](crate::Renderer::set_render_scale)
//!   every frame.
//! - **Antialiasing** ([`Antialiasing`], M1 R7): temporal AA with MSAA off on the desktop tiers,
//!   whose resolve also upsamples to the output; 4× MSAA kept in tile memory on phones.
//! - **MSAA and temporal AA, bloom levels, the light path, the light budget, the sky** (its
//!   cube's size and update rate, and the cirrus layer, M1 W2), **the environment shader's
//!   specular model** (M1 W4) and where it reads a probe grid (per pixel or per vertex, M1 W5),
//!   **the shadows** (cascades, their resolution, distance and
//!   caching, the characters' maps or capsules and the filter, M1 R4), **the screen effects'
//!   cost** (the radial blur's taps, [`PostEffectSettings`], M1 P3), **foliage** (its wind
//!   levels, alpha-to-coverage and translucency, M1 W6), **ambient occlusion** (capsule AO, and
//!   GTAO's slices, steps and radius) **and the reflection cube's size** (M1 W5):
//!   [`QualityTier::render_settings`], which a renderer is created with. The player's settings
//!   can turn the shadows off ([`QualitySettings::shadows`]) and choose the ambient occlusion
//!   ([`QualitySettings::ambient_occlusion`]).
//! - **Frame rate and GPU frame budget** (§3.1): dynamic resolution's targets.
//!
//! The player's settings can also shed [`OptionalPass`]es, each a step of what the tier draws that
//! the look can spare (GTAO, the cirrus, a wind level, half the sky's face rate, a bloom level):
//! [`QualitySettings::shed`], with the renderer, and the sandbox's `--shed`.
//!
//! # The thermal ladder
//!
//! Each tier lists the rungs it gives up when the device runs hot or the GPU can't hold its budget
//! ([`QualityTier::ladder`], M4 T2): a lower render scale, optional passes off in place, a frame-rate
//! cap, and last a lower tier. [`Quality::follow_heat`] runs the tier's [`Ladder`] every frame from the
//! platform's [`ThermalReading`] and dynamic resolution's pressure; [`ladder`] has the controller.
//!
//! One more knob is a cvar rather than a tier's: `r.outline_distance`, a scale on every
//! material's outline LOD cutoff (M1 C4), for tuning and profiling ([`Quality::outline_distance`];
//! the sandbox's `--outline-distance`). Every tier draws the same lines today, within its budget.
//!
//! Plain data until their packages read them: volumetric fog (after W3's distance and height fog,
//! which every tier has), screen-space environment outlines (the desktop option beside C4's
//! character hulls, which no M1 package builds), the particle cap (`sapphire_vfx`, M2) and the
//! LOD bias (LOD selection, with culling). The frame rate is also what frame pacing (§6.2, F7)
//! will hold.
//!
//! # Choosing a tier
//!
//! A [`DeviceProfile`] (the device class, the GPU's kind, API, vendor and name from [`GpuCaps`], and
//! the RAM) is matched against each tier's `select_when` rules: a device gets the highest tier of its
//! class whose rules it meets, and each class's lowest tier has none, so every device gets one. A
//! desktop adapter the [`GpuTable`] knows (`gpus.ron`, M4 T4; [`gpus`]) takes no higher than its
//! family's ceiling, so an old or low-end GPU with plenty of RAM isn't Desktop-High; a handheld PC
//! takes its class's lowest. [`TierTable::select`] says which of these chose ([`SelectedBy`]), and
//! [`Quality::new`] logs it. Never by OS (rule R7). Three sources override it, in this order:
//!
//! 1. the `r.tier` cvar, unless `auto` (development builds, from the overlay);
//! 2. the player's setting ([`QualitySettings::tier`]; the sandbox's `--tier`);
//! 3. the device's tier.
//!
//! Any tier runs on any device, so mobile tiers can be previewed on a desktop. A renderer is
//! created with its tier's settings. The game changes tier in place (M4 T3): the new tier's
//! renderer builds on the warm-up pool ([`Renderer::build_tier`](crate::Renderer::build_tier)), and
//! [`Renderer::take_tier`](crate::Renderer::take_tier) swaps it in at a pause or a wipe, keeping every
//! mesh, material and instance, so no frame builds. The sandbox's `r.tier` still makes a new
//! renderer ([`Quality::update_tier`]), as device loss does.
//!
//! # Render scale and dynamic resolution
//!
//! Each frame [`Quality::render_scale`] takes the base scale (`r.render_scale` if set, else the
//! player's, else the tier's [`Resolution::base_scale`] for the output) and lets
//! [`DynamicResolution`] lower it, in 5% steps down to the tier's floor, to hold the GPU frame
//! under the tier's budget. Dynamic resolution is off by default (`r.dynamic_resolution`,
//! [`QualitySettings::dynamic_resolution`]); while it is on, the app turns on the frame graph's GPU
//! timing ([`Quality::wants_gpu_timing`]). See [`dynamic`] for the controller.
//!
//! # Later milestones
//!
//! - **Selection by measurement:** the rules and the GPU table are a first heuristic from specs
//!   (spec §3's numbers are all Proposals); a startup benchmark, and measurements on the reference
//!   devices, would refine them.
//! - **A tier without a pause:** a tier change waits for a pause or a wipe, since the swap is a
//!   camera cut that starts temporal history and the shadows' caches again. Settings that build no
//!   pipeline, as the ladder's rungs already shed, could change mid-fight.
//! - **Shadows and ambient occlusion for the player:** the game's settings come from its settings
//!   screen and its save, not the command line, and change while it runs (M4 T3): its tier and
//!   dynamic resolution reach [`Quality::set_settings`]. Turning shadows off and choosing the
//!   ambient occlusion are still the sandbox's flags, and take effect only with a new tier's
//!   renderer.
//! - **Cheaper timing:** GPU timing costs about 50 µs of CPU and 100 allocations per frame on
//!   Metal (spec §5.12, M1 R6's cost), while dynamic resolution needs a few samples a second.
//!   Timing every few frames, a frame graph option, would cut that to a fraction.
//! - **A tuned controller:** per-tier bands and steps once devices are measured, and an
//!   upscaler's input range (R7) in place of a fixed floor.

pub mod dynamic;
pub mod gpus;
pub mod ladder;

use std::num::NonZeroU32;
use std::time::Duration;

use bevy_reflect::{Reflect, TypePath};
use sapphire_core::cvar::{AnyCvar, Cvar, CvarKind, CvarType, CvarValue};
use sapphire_core::target::FormFactor;
use sapphire_core::thermal::ThermalReading;
use sapphire_reflect::Versioned;
use sapphire_rhi::{GpuCaps, wgpu};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::{
    AmbientOcclusion, EnvironmentSettings, FoliageSettings, GtaoSettings, InsetSettings, LOG_TARGET,
    LightPath, MAX_BLOOM_LEVELS, MAX_LIGHTS, MIN_RENDER_SCALE, Msaa, PostEffectSettings, PostVariants,
    ReflectionSettings, RenderSettings, ShadowSettings, SkySettings, scaled_size,
};
pub use dynamic::{
    DynamicResolution, DynamicResolutionStats, FrameTiming, Pressure, ResolutionBudget, ResolutionSignal,
};
pub use gpus::{GpuFamily, GpuTable, GpuTableError, GpuVendor};
pub use ladder::{Cause, Ladder, LadderState, LadderStats, LadderStep, Rung};

/// The shipped table (see the module docs).
const SHIPPED: &str = include_str!("../tiers.ron");

/// The quality tiers of spec §3.1. They order lowest first within each class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum TierId {
    MobileLow,
    MobileHigh,
    DesktopLow,
    DesktopHigh,
}

impl TierId {
    pub const ALL: [Self; 4] = [Self::DesktopHigh, Self::DesktopLow, Self::MobileHigh, Self::MobileLow];

    /// The class of device the tier is made for.
    pub fn class(self) -> FormFactor {
        match self {
            Self::DesktopHigh | Self::DesktopLow => FormFactor::Desktop,
            Self::MobileHigh | Self::MobileLow => FormFactor::Mobile,
        }
    }

    /// The lowest tier of `class`, which every device of the class qualifies for.
    pub fn lowest(class: FormFactor) -> Self {
        match class {
            FormFactor::Desktop => Self::DesktopLow,
            FormFactor::Mobile => Self::MobileLow,
        }
    }

    /// The tier a boot falls back to when a renderer can't be built at this one (M4 H9): the next lower of its
    /// class, Desktop-High to Desktop-Low and Mobile-High to Mobile-Low. None below a class's lowest.
    pub fn lower(self) -> Option<Self> {
        match self {
            Self::DesktopHigh => Some(Self::DesktopLow),
            Self::MobileHigh => Some(Self::MobileLow),
            Self::DesktopLow | Self::MobileLow => None,
        }
    }

    /// This tier, or `ceiling` where that's a lower tier of its class: what a device that couldn't build a
    /// tier gets in its place ([`QualitySettings::ceiling`]).
    pub fn under(self, ceiling: Option<Self>) -> Self {
        match ceiling {
            Some(ceiling) if ceiling.class() == self.class() && ceiling < self => ceiling,
            _ => self,
        }
    }

    /// The spec's name: `Desktop-High`.
    pub fn name(self) -> &'static str {
        match self {
            Self::DesktopHigh => "Desktop-High",
            Self::DesktopLow => "Desktop-Low",
            Self::MobileHigh => "Mobile-High",
            Self::MobileLow => "Mobile-Low",
        }
    }

    /// How cvars and the command line spell it: `desktop-high`.
    pub fn key(self) -> &'static str {
        match self {
            Self::DesktopHigh => "desktop-high",
            Self::DesktopLow => "desktop-low",
            Self::MobileHigh => "mobile-high",
            Self::MobileLow => "mobile-low",
        }
    }

    /// The tier [`Self::key`] spells, ignoring case.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tier| tier.key().eq_ignore_ascii_case(key))
    }
}

impl std::fmt::Display for TierId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// One tier's knobs (spec §5.12 "Quality tiers"). All numbers are Proposals until M2 measures
/// the reference devices (spec §3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct QualityTier {
    pub id: TierId,
    /// Devices that qualify, any rule matching; empty for each class's lowest tier, which every
    /// device of the class gets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub select_when: Vec<TierRule>,
    /// Frames per second the tier is built for (§3.1): dynamic resolution's frame-time target,
    /// and the rate frame pacing holds (§6.2, M1 F7).
    pub frame_rate: u32,
    /// The GPU frame budget in milliseconds (§3.1), which dynamic resolution holds frames under.
    pub gpu_frame_ms: f32,
    pub resolution: Resolution,
    pub antialiasing: Antialiasing,
    /// Bloom's levels below the scene's size ([`RenderSettings::bloom_levels`]).
    pub bloom_levels: u32,
    /// How local lights are listed ([`RenderSettings::light_path`]).
    pub light_path: LightPath,
    /// The most local lights a frame draws ([`RenderSettings::max_lights`]; §3.4).
    pub max_lights: u32,
    /// The key light's shadows: cascades and characters' own shadows (M1 R4,
    /// [`RenderSettings::shadows`]).
    pub shadows: ShadowSettings,
    /// Capsule AO and GTAO (M1 W5, [`RenderSettings::ambient_occlusion`]).
    pub ambient_occlusion: AmbientOcclusion,
    /// Volumetric fog, on top of the distance and height fog every tier has (W3; volumetric fog
    /// is desktop-only and later).
    pub volumetric_fog: bool,
    /// The screen-space edge pass for the environment, from the prepass's normals and IDs: a
    /// desktop option beside the characters' inverted-hull outlines (C4) that no M1 package builds.
    pub screen_space_outlines: bool,
    /// Live GPU particles at most (§3.4), for `sapphire_vfx` (M2).
    pub max_particles: u32,
    /// Added to each draw's LOD choice, in LOD levels: positive picks coarser LODs sooner. Read
    /// by LOD selection, which arrives with culling.
    pub lod_bias: f32,
    /// The sky's cube size and update rate and its cirrus layer ([`RenderSettings::sky`]). A
    /// table without it gets the desktop tiers' sky.
    #[serde(default)]
    pub sky: SkySettings,
    /// The environment shader's specular model and where it reads a probe grid
    /// ([`RenderSettings::environment`]). A table without it gets the desktop tiers' full
    /// specular and per-pixel grid.
    #[serde(default)]
    pub environment: EnvironmentSettings,
    /// What the screen effects spend ([`RenderSettings::post_effects`], M1 P3). A table without
    /// it gets Desktop-Low's.
    #[serde(default)]
    pub post_effects: PostEffectSettings,
    /// Foliage's wind levels, alpha-to-coverage and translucency ([`RenderSettings::foliage`]).
    /// A table without it gets the desktop tiers' foliage.
    #[serde(default)]
    pub foliage: FoliageSettings,
    /// The reflection cube a probe grid's environment specular reads (M1 W5,
    /// [`RenderSettings::reflections`]). A table without it gets the desktop tiers'.
    #[serde(default)]
    pub reflections: ReflectionSettings,
    /// How large 3D insets' pictures and scenes are (M3 U6, [`RenderSettings::inset`]). A table
    /// without it gets the desktop tiers'.
    #[serde(default)]
    pub inset: InsetSettings,
    /// The thermal ladder's rungs, from the top (M4 T2, [`ladder`]). A tier without them answers
    /// neither heat nor dynamic resolution's pressure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ladder: Vec<Rung>,
}

/// The scene's size relative to the output.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct Resolution {
    /// The scene's width and height as a fraction of the output's, in 0.25..=1.
    pub scale: f32,
    /// The most rows the scene renders: below `scale` on taller outputs, so the spec's internal
    /// resolutions hold on any display ("~1280×590 → native"; "1440p native, or 4K upscaled").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,
    /// Dynamic resolution's floor, as a fraction of the base scale, in 0.25..=1.
    pub dynamic_floor: f32,
}

impl Resolution {
    /// The base scale for an output `height` pixels tall: `scale`, lowered to fit `max_height`,
    /// and at least [`MIN_RENDER_SCALE`].
    pub fn base_scale(&self, height: u32) -> f32 {
        let fit = match self.max_height {
            Some(max) if height > max => max as f32 / height as f32,
            _ => 1.0,
        };
        self.scale.min(fit).clamp(MIN_RENDER_SCALE, 1.0)
    }
}

/// Edge antialiasing (spec §5.12; plan pillar 6). `tiers.ron` says why each class uses what.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct Antialiasing {
    /// Multisampling of the main pass: off on the desktop tiers, whose temporal AA resolves the
    /// edges; 4× on phones, kept in tile memory (transient attachments).
    pub msaa: Msaa,
    /// Temporal AA and upsampling with velocity and a responsive mask (M1 R7,
    /// [`RenderSettings::temporal`]): the desktop tiers.
    pub temporal: bool,
}

/// One way a device qualifies for a tier: it must meet every condition the rule sets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct TierRule {
    /// The GPU is one of these kinds; empty: any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpus: Vec<GpuKind>,
    /// The device runs one of these graphics APIs; empty: any. With the GPU's kind it names Apple
    /// silicon (an integrated GPU on Metal), whose adapters report no vendor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<GpuBackend>,
    /// Physical RAM of at least this many MiB, where the spec's memory budgets (§3.1) apply.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub min_ram_mib: u32,
}

impl TierRule {
    pub fn matches(&self, device: &DeviceProfile) -> bool {
        (self.gpus.is_empty() || self.gpus.contains(&device.gpu))
            && (self.backends.is_empty() || self.backends.contains(&device.backend))
            && device.ram_mib >= self.min_ram_mib
    }
}

/// What kind of GPU a device has (wgpu's `DeviceType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum GpuKind {
    Discrete,
    /// Sharing memory with the CPU: Apple silicon, phones, most laptops.
    Integrated,
    /// A virtual machine's or a hosted runner's paravirtual GPU.
    Virtual,
    /// A software rasterizer: lavapipe, WARP, SwiftShader.
    Cpu,
    Other,
}

impl From<wgpu::DeviceType> for GpuKind {
    fn from(kind: wgpu::DeviceType) -> Self {
        match kind {
            wgpu::DeviceType::DiscreteGpu => Self::Discrete,
            wgpu::DeviceType::IntegratedGpu => Self::Integrated,
            wgpu::DeviceType::VirtualGpu => Self::Virtual,
            wgpu::DeviceType::Cpu => Self::Cpu,
            wgpu::DeviceType::Other => Self::Other,
        }
    }
}

/// The graphics API a device runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum GpuBackend {
    Vulkan,
    Metal,
    Dx12,
    Other,
}

impl From<wgpu::Backend> for GpuBackend {
    fn from(backend: wgpu::Backend) -> Self {
        match backend {
            wgpu::Backend::Vulkan => Self::Vulkan,
            wgpu::Backend::Metal => Self::Metal,
            wgpu::Backend::Dx12 => Self::Dx12,
            _ => Self::Other,
        }
    }
}

/// What tier selection knows about a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProfile {
    pub class: FormFactor,
    pub gpu: GpuKind,
    pub backend: GpuBackend,
    /// The adapter's PCI vendor id (`wgpu::AdapterInfo::vendor`): 0 where the API reports none, as
    /// Metal does.
    pub vendor: u32,
    /// The adapter's name as its driver reports it (`wgpu::AdapterInfo::name`), by which the
    /// [`GpuTable`] knows its family.
    pub adapter: String,
    /// Physical RAM in MiB (`sapphire_platform::SystemInfo::ram_mib`): shared with the GPU on
    /// Apple silicon and phones.
    pub ram_mib: u32,
    /// The GPU can time passes (`GpuCaps::timestamp_query`), so dynamic resolution follows GPU
    /// time; without it, the frame time.
    pub timestamps: bool,
    /// A handheld PC (`sapphire_platform::SystemInfo::handheld`, a Steam Deck): it starts on its class's lowest
    /// tier, whatever its GPU and RAM would qualify for, since its panel is 1280 × 800 and its battery small
    /// (M4 T3).
    pub handheld: bool,
}

impl DeviceProfile {
    /// A device of `class` with `caps`' GPU and `ram_mib` of RAM.
    pub fn new(class: FormFactor, caps: &GpuCaps, ram_mib: u32) -> Self {
        Self {
            class,
            gpu: caps.adapter.device_type.into(),
            backend: caps.backend().into(),
            vendor: caps.adapter.vendor,
            adapter: caps.adapter.name.clone(),
            ram_mib,
            timestamps: caps.timestamp_query,
            handheld: false,
        }
    }

    /// The same device, a handheld PC or not.
    #[must_use]
    pub fn handheld(self, handheld: bool) -> Self {
        Self { handheld, ..self }
    }
}

impl QualityTier {
    /// What a renderer for this tier is created with.
    pub fn render_settings(&self) -> RenderSettings {
        RenderSettings {
            msaa: self.antialiasing.msaa,
            scene_velocity: false,
            temporal: self.antialiasing.temporal,
            bloom_levels: self.bloom_levels,
            light_path: self.light_path,
            max_lights: self.max_lights,
            sky: self.sky,
            environment: self.environment,
            shadows: Some(self.shadows),
            post_effects: self.post_effects,
            post_variants: PostVariants::COMPOSITING,
            foliage: self.foliage,
            ambient_occlusion: self.ambient_occlusion,
            reflections: self.reflections,
            inset: self.inset,
            view_culling: true,
        }
    }

    /// The budgets dynamic resolution holds this tier's frames to.
    pub fn resolution_budget(&self) -> ResolutionBudget {
        self.resolution_budget_at(self.frame_rate)
    }

    /// The budgets at `frame_rate` frames a second, a ladder's cap: the GPU frame stretched as the
    /// frame is.
    pub fn resolution_budget_at(&self, frame_rate: u32) -> ResolutionBudget {
        let frame_rate = frame_rate.max(1);
        let gpu_frame = Duration::from_secs_f32(self.gpu_frame_ms / 1000.0);
        ResolutionBudget {
            gpu_frame: if frame_rate == self.frame_rate {
                gpu_frame
            } else {
                gpu_frame.mul_f64(f64::from(self.frame_rate) / f64::from(frame_rate))
            },
            frame: Duration::from_secs_f64(1.0 / f64::from(frame_rate)),
            floor: self.resolution.dynamic_floor,
        }
    }

    /// Whether `device` qualifies for the tier by its rules (its class aside).
    pub fn qualifies(&self, device: &DeviceProfile) -> bool {
        self.select_when.is_empty() || self.select_when.iter().any(|rule| rule.matches(device))
    }

    /// Checks every knob is in its range.
    pub fn validate(&self) -> Result<(), TierError> {
        let invalid = |field: &'static str, problem: &str| {
            Err(TierError::Invalid { tier: self.id, field, problem: problem.into() })
        };
        let fraction = MIN_RENDER_SCALE..=1.0;
        let resolution = &self.resolution;
        if !fraction.contains(&resolution.scale) {
            return invalid("resolution.scale", "must be within 0.25..=1");
        }
        if resolution.max_height == Some(0) {
            return invalid("resolution.max_height", "must be at least 1");
        }
        if !fraction.contains(&resolution.dynamic_floor) {
            return invalid("resolution.dynamic_floor", "must be within 0.25..=1");
        }
        if !(1..=240).contains(&self.frame_rate) {
            return invalid("frame_rate", "must be within 1..=240");
        }
        let frame_ms = 1000.0 / self.frame_rate as f32;
        if !(self.gpu_frame_ms > 0.0 && self.gpu_frame_ms <= frame_ms) {
            return invalid("gpu_frame_ms", "must be positive and fit in the frame");
        }
        if self.bloom_levels > MAX_BLOOM_LEVELS {
            return invalid("bloom_levels", "must be at most 6");
        }
        if self.max_lights > MAX_LIGHTS {
            return invalid("max_lights", "must be at most 1024");
        }
        if let Err((field, problem)) = self.shadows.validate() {
            return invalid(field, problem);
        }
        if !(-4.0..=4.0).contains(&self.lod_bias) {
            return invalid("lod_bias", "must be within -4..=4");
        }
        if let Err((field, problem)) = self.sky.validate() {
            return invalid(field, problem);
        }
        if let Err((field, problem)) = self.post_effects.validate() {
            return invalid(field, problem);
        }
        if let Err((field, problem)) = self.foliage.validate() {
            return invalid(field, problem);
        }
        if let Err((field, problem)) = self.ambient_occlusion.validate() {
            return invalid(field, problem);
        }
        if let Err((field, problem)) = self.reflections.validate() {
            return invalid(field, problem);
        }
        if let Err((field, problem)) = self.inset.validate() {
            return invalid(field, problem);
        }
        if let Err(problem) = ladder::validate(self) {
            return invalid("ladder", problem);
        }
        Ok(())
    }
}

/// The tier table (`tiers.ron`): exactly one [`QualityTier`] per [`TierId`], each valid, and
/// each class's lowest tier without rules. Every way of making one checks this, deserializing
/// included. It has a stable type path but no `Reflect`, which could build or edit one without
/// the checks; an editor edits a [`QualityTier`] and makes a new table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::render"]
#[serde(try_from = "UncheckedTable")]
pub struct TierTable {
    tiers: Vec<QualityTier>,
}

impl Versioned for TierTable {
    const VERSION: u32 = 1;
}

/// A table as written, before [`TierTable::new`] checks it.
#[derive(Deserialize)]
struct UncheckedTable {
    tiers: Vec<QualityTier>,
}

impl TryFrom<UncheckedTable> for TierTable {
    type Error = TierError;

    fn try_from(table: UncheckedTable) -> Result<Self, TierError> {
        Self::new(table.tiers)
    }
}

/// Why a tier table is invalid.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TierError {
    #[error("{tier}: `{field}` {problem}")]
    Invalid { tier: TierId, field: &'static str, problem: String },
    #[error("the table has no {0} tier")]
    Missing(TierId),
    #[error("the table has {0} more than once")]
    Duplicate(TierId),
    #[error("{0} is its class's lowest tier, which every device must qualify for: it can't have rules")]
    ConditionalLowest(TierId),
}

impl TierTable {
    /// A table of `tiers`, checked.
    pub fn new(tiers: Vec<QualityTier>) -> Result<Self, TierError> {
        for id in TierId::ALL {
            match tiers.iter().filter(|tier| tier.id == id).count() {
                0 => return Err(TierError::Missing(id)),
                1 => {}
                _ => return Err(TierError::Duplicate(id)),
            }
        }
        for tier in &tiers {
            tier.validate()?;
            if tier.id == TierId::lowest(tier.id.class()) && !tier.select_when.is_empty() {
                return Err(TierError::ConditionalLowest(tier.id));
            }
        }
        Ok(Self { tiers })
    }

    /// The table compiled into the renderer (`tiers.ron`).
    pub fn shipped() -> Self {
        Self::from_ron(SHIPPED).expect("the shipped tier table is checked by this module's tests")
    }

    /// Reads a table document, upgrading older schema versions and checking the table.
    pub fn from_ron(text: &str) -> Result<Self, DocumentError> {
        sapphire_serialize::from_ron(text)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// Every tier, in the table's order.
    pub fn tiers(&self) -> &[QualityTier] {
        &self.tiers
    }

    pub fn get(&self, id: TierId) -> &QualityTier {
        self.tiers.iter().find(|tier| tier.id == id).expect("a checked table holds every tier")
    }

    /// The highest tier of `device`'s class that it qualifies for, at most the ceiling `gpus` gives a
    /// desktop adapter it knows; a handheld's lowest. Says which chose.
    pub fn select(&self, gpus: &GpuTable, device: &DeviceProfile) -> Selection {
        let lowest = TierId::lowest(device.class);
        if device.handheld {
            return Selection { tier: lowest, by: SelectedBy::Handheld };
        }
        let rules = self
            .tiers
            .iter()
            .filter(|tier| tier.id.class() == device.class && tier.qualifies(device))
            .map(|tier| tier.id)
            .max()
            .unwrap_or(lowest);
        let known = match device.class {
            FormFactor::Desktop => gpus.find(device.vendor, &device.adapter),
            FormFactor::Mobile => None,
        };
        match known {
            Some(family) if family.ceiling < rules => Selection {
                tier: family.ceiling,
                by: SelectedBy::Gpu { family: family.family.clone(), rules },
            },
            _ => Selection {
                tier: rules,
                by: SelectedBy::Rules { gpu: known.map(|family| family.family.clone()) },
            },
        }
    }
}

/// The tier [`TierTable::select`] gives a device, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub tier: TierId,
    pub by: SelectedBy,
}

/// Which rule chose a device's tier, for the log and a bug report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectedBy {
    /// A handheld PC, which starts on its class's lowest tier.
    Handheld,
    /// The tiers' `select_when` rules (the GPU's kind and API, and the RAM). `gpu` names the
    /// [`GpuTable`]'s family where it knows the adapter, its ceiling no lower than the rules' tier.
    Rules { gpu: Option<String> },
    /// The [`GpuTable`]'s `family`, whose ceiling is below the tier the `rules` give.
    Gpu { family: String, rules: TierId },
}

impl std::fmt::Display for SelectedBy {
    /// `the GPU table's GeForce GTX row, under the rules' Desktop-High`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Handheld => f.write_str("a handheld PC's lowest tier"),
            Self::Rules { gpu: None } => {
                f.write_str("the rules (GPU kind and RAM), an adapter the GPU table doesn't know")
            }
            Self::Rules { gpu: Some(family) } => {
                write!(f, "the rules (GPU kind and RAM), within the GPU table's {family} row")
            }
            Self::Gpu { family, rules } => {
                write!(f, "the GPU table's {family} row, under the rules' {rules}")
            }
        }
    }
}

/// A tier chosen by name, or `Auto`: `r.tier`'s values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd)]
pub enum TierChoice {
    /// The player's setting, else the device's tier.
    #[default]
    Auto,
    Tier(TierId),
}

const TIER_CHOICES: [&str; 5] = ["auto", "desktop-high", "desktop-low", "mobile-high", "mobile-low"];

impl CvarType for TierChoice {
    const KIND: CvarKind = CvarKind::Choice(&TIER_CHOICES);

    fn to_bits(self) -> u32 {
        match self {
            Self::Auto => 0,
            Self::Tier(id) => {
                1 + TierId::ALL.iter().position(|&tier| tier == id).expect("ALL lists every tier") as u32
            }
        }
    }

    fn from_bits(bits: u32) -> Self {
        match bits.checked_sub(1) {
            Some(index) => TierId::ALL.get(index as usize).map_or(Self::Auto, |&id| Self::Tier(id)),
            None => Self::Auto,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let index = TIER_CHOICES.iter().position(|name| name.eq_ignore_ascii_case(text))?;
        Some(Self::from_bits(index as u32))
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Choice { index: self.to_bits(), names: &TIER_CHOICES }
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Choice { index, .. } if (index as usize) < TIER_CHOICES.len() => {
                Some(Self::from_bits(index))
            }
            _ => None,
        }
    }
}

/// An on/off override that can also defer: `r.dynamic_resolution`'s values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd)]
pub enum Switch {
    /// Whatever the setting it overrides says.
    #[default]
    Auto,
    On,
    Off,
}

const SWITCH_CHOICES: [&str; 3] = ["auto", "on", "off"];

impl Switch {
    /// On or off, deferring to `setting` when `Auto`.
    pub fn resolve(self, setting: bool) -> bool {
        match self {
            Self::Auto => setting,
            Self::On => true,
            Self::Off => false,
        }
    }
}

impl CvarType for Switch {
    const KIND: CvarKind = CvarKind::Choice(&SWITCH_CHOICES);

    fn to_bits(self) -> u32 {
        self as u32
    }

    fn from_bits(bits: u32) -> Self {
        match bits {
            1 => Self::On,
            2 => Self::Off,
            _ => Self::Auto,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let index = SWITCH_CHOICES.iter().position(|name| name.eq_ignore_ascii_case(text))?;
        Some(Self::from_bits(index as u32))
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Choice { index: self as u32, names: &SWITCH_CHOICES }
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Choice { index, .. } if (index as usize) < SWITCH_CHOICES.len() => {
                Some(Self::from_bits(index))
            }
            _ => None,
        }
    }
}

/// The active tier, over the player's setting and the device's (see the module docs).
pub static TIER: Cvar<TierChoice> = Cvar::new(
    "r.tier",
    TierChoice::Auto,
    "Quality tier (spec §3.1): auto is the player's setting, else the device's. A change makes a new renderer.",
);
/// The base render scale, over the player's and the tier's.
pub static RENDER_SCALE: Cvar<f32> = Cvar::with_range(
    "r.render_scale",
    0.0,
    0.0,
    1.0,
    "Base render scale, a fraction of the output's size (at least 0.25): 0 is the player's, else the tier's.",
);
/// Dynamic resolution on or off, over the player's setting.
pub static DYNAMIC_RESOLUTION: Cvar<Switch> = Cvar::new(
    "r.dynamic_resolution",
    Switch::Auto,
    "Dynamic resolution, lowering the render scale to hold the tier's GPU budget: auto is the player's setting (off).",
);

/// The reach of outline hulls (M1 C4): a scale on every material's outline LOD cutoff.
pub static OUTLINE_DISTANCE: Cvar<f32> = Cvar::with_range(
    "r.outline_distance",
    1.0,
    0.0,
    4.0,
    "Scales every material's outline LOD cutoff: below 1 lines stop nearer, fading out as always; 0 draws none (profiling).",
);

/// Holds the thermal ladder at a depth (M4 T2): goldens, the overlay and tests walk it with this.
pub static RUNG: Cvar<i32> = Cvar::with_range(
    "r.rung",
    -1,
    -1,
    ladder::MAX_RUNGS as i32,
    "Holds the thermal ladder this many rungs down (spec §6.3): -1 follows heat and dynamic resolution's pressure.",
);

/// This module's cvars, for the app's registry.
pub static CVARS: [&dyn AnyCvar; 5] = [&TIER, &RENDER_SCALE, &DYNAMIC_RESOLUTION, &OUTLINE_DISTANCE, &RUNG];

/// The player's quality settings: a settings screen's, saved with the player's settings later
/// (M3); the sandbox takes them from its command line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QualitySettings {
    /// A tier instead of the device's.
    pub tier: Option<TierId>,
    /// A base render scale instead of the tier's.
    pub render_scale: Option<f32>,
    /// Let dynamic resolution lower the scale. Off by default.
    pub dynamic_resolution: bool,
    /// Shadows: the tier's (`Auto`, the default, or `On`), or none (`Off`), which draws no shadow
    /// maps at all.
    pub shadows: Switch,
    /// Ambient occlusion: the tier's, or a level of the player's (M1 W5).
    pub ambient_occlusion: OcclusionSetting,
    /// Optional passes to go without, after the settings above, with the renderer: the sandbox's
    /// `--shed` for captures and benches (M4 T0). The ladder sheds in place on top of them.
    pub shed: OptionalPasses,
    /// The highest tier of its class this device is given, whoever chose: below one whose renderer couldn't be
    /// built (M4 H9, [`Quality::fall_back`]), so it's never built again in this run. `None`, no limit. It only
    /// lowers: [`Quality::set_settings`] keeps the lower of the two.
    pub ceiling: Option<TierId>,
}

/// A pass a tier can go without and keep the look (M4 plan §3, "The ladder"): shedding one never
/// touches the character family, the outlines, the ramps or the grade. Each sheds one step of what
/// its tier draws, and nothing where the tier draws none. M4 T0 measured what each saves. Three shed
/// in place, on a renderer that has them ([`Self::in_place`]); the other two only with a new one.
///
/// The ladder's list names two more that nothing here sheds: volumetric fog, which no package
/// builds yet (`QualityTier::volumetric_fog` is plain data), and the particle cap, which is
/// `sapphire_vfx`'s budget row and not the renderer's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum OptionalPass {
    /// GTAO, leaving capsule AO, which grounds the characters.
    Gtao,
    /// The sky's cirrus layer; the card clouds stay.
    Cirrus,
    /// Foliage's finest wind level: leaf flutter on a tier with three, branch bend with two.
    WindLevel,
    /// Half the sky cube's face rate: a face every other frame where it drew one a frame.
    SkyFaceRate,
    /// Bloom's lowest level, its widest glow.
    BloomLevel,
}

impl OptionalPass {
    pub const ALL: [Self; 5] =
        [Self::Gtao, Self::Cirrus, Self::WindLevel, Self::SkyFaceRate, Self::BloomLevel];

    /// How the command line spells it: `sky-face-rate`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Gtao => "gtao",
            Self::Cirrus => "cirrus",
            Self::WindLevel => "wind-level",
            Self::SkyFaceRate => "sky-face-rate",
            Self::BloomLevel => "bloom-level",
        }
    }

    /// The pass [`Self::key`] spells, ignoring case.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pass| pass.key().eq_ignore_ascii_case(key))
    }

    /// Whether [`Renderer::set_shed`](crate::Renderer::set_shed) sheds it in place, building nothing:
    /// GTAO, the sky's face rate and a bloom level, the passes the thermal ladder takes. The cirrus
    /// and the wind levels aren't rungs (M4 plan §3), and only a new renderer sheds them: the wind
    /// levels are pipeline constants.
    pub fn in_place(self) -> bool {
        matches!(self, Self::Gtao | Self::SkyFaceRate | Self::BloomLevel)
    }

    /// `settings` without this pass: as they are where they draw none of it.
    pub fn shed(self, settings: RenderSettings) -> RenderSettings {
        let RenderSettings { ambient_occlusion, sky, foliage, bloom_levels, .. } = settings;
        match self {
            Self::Gtao => RenderSettings {
                ambient_occlusion: AmbientOcclusion { gtao: None, ..ambient_occlusion },
                ..settings
            },
            Self::Cirrus => RenderSettings { sky: SkySettings { cirrus: false, ..sky }, ..settings },
            Self::WindLevel => RenderSettings {
                foliage: FoliageSettings { wind_levels: foliage.wind_levels.saturating_sub(1), ..foliage },
                ..settings
            },
            Self::SkyFaceRate => RenderSettings {
                sky: SkySettings {
                    cube_interval: (sky.cube_interval * 2).min(SkySettings::MAX_CUBE_INTERVAL),
                    ..sky
                },
                ..settings
            },
            Self::BloomLevel => RenderSettings { bloom_levels: bloom_levels.saturating_sub(1), ..settings },
        }
    }
}

/// A set of [`OptionalPass`]es.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OptionalPasses(u8);

impl OptionalPasses {
    pub const NONE: Self = Self(0);

    /// The set with `pass` added.
    pub fn with(self, pass: OptionalPass) -> Self {
        Self(self.0 | 1 << pass as u8)
    }

    pub fn contains(self, pass: OptionalPass) -> bool {
        self.0 & 1 << pass as u8 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Its passes, in [`OptionalPass::ALL`]'s order.
    pub fn iter(self) -> impl Iterator<Item = OptionalPass> {
        OptionalPass::ALL.into_iter().filter(move |&pass| self.contains(pass))
    }

    /// `settings` without any of its passes.
    pub fn shed(self, settings: RenderSettings) -> RenderSettings {
        self.iter().fold(settings, |settings, pass| pass.shed(settings))
    }
}

impl std::fmt::Display for OptionalPasses {
    /// `gtao+bloom-level`, or `none`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        for (index, pass) in self.iter().enumerate() {
            if index > 0 {
                f.write_str("+")?;
            }
            f.write_str(pass.key())?;
        }
        Ok(())
    }
}

impl FromIterator<OptionalPass> for OptionalPasses {
    fn from_iter<I: IntoIterator<Item = OptionalPass>>(passes: I) -> Self {
        passes.into_iter().fold(Self::NONE, Self::with)
    }
}

/// The player's ambient occlusion setting (M1 W5; the sandbox's `--ao`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OcclusionSetting {
    /// The tier's.
    #[default]
    Auto,
    /// None: no passes, and the environment shader as it was without it.
    Off,
    /// Capsule AO alone.
    Capsules,
    /// Capsule AO and GTAO: the tier's GTAO, or the desktop tiers' on a tier without it.
    Full,
}

impl OcclusionSetting {
    /// The ambient occlusion it gives on a tier with `tier`'s.
    pub fn resolve(self, tier: AmbientOcclusion) -> AmbientOcclusion {
        match self {
            Self::Auto => tier,
            Self::Off => AmbientOcclusion::OFF,
            Self::Capsules => AmbientOcclusion { capsules: true, gtao: None },
            Self::Full => {
                AmbientOcclusion { capsules: true, gtao: Some(tier.gtao.unwrap_or(GtaoSettings::DESKTOP)) }
            }
        }
    }
}

/// Where the active tier came from, in precedence order (lowest first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierSource {
    /// Selected for the device by the table's rules.
    Device,
    /// The player's setting.
    Setting,
    /// The `r.tier` cvar.
    Cvar,
    /// The thermal ladder's, at a pause or a wipe ([`Quality::take_tier_request`]).
    Ladder,
}

/// The tier in use and where it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveTier {
    pub id: TierId,
    pub source: TierSource,
}

/// The tier `r.tier` (`cvar`), the player's `setting` and the `device`'s tier give, in that
/// order of precedence.
pub fn choose_tier(cvar: TierChoice, setting: Option<TierId>, device: TierId) -> ActiveTier {
    match (cvar, setting) {
        (TierChoice::Tier(id), _) => ActiveTier { id, source: TierSource::Cvar },
        (TierChoice::Auto, Some(id)) => ActiveTier { id, source: TierSource::Setting },
        (TierChoice::Auto, None) => ActiveTier { id: device, source: TierSource::Device },
    }
}

/// Quality tiers as an app runs them: the table, the device's tier, the player's settings, the
/// active tier, its thermal ladder and its dynamic resolution. Per frame it reads four cvars and
/// does a few dozen floating-point operations, and allocates nothing.
#[derive(Debug)]
pub struct Quality {
    table: TierTable,
    device: TierId,
    /// Which rule chose the device's tier, for the log.
    selected_by: SelectedBy,
    settings: QualitySettings,
    active: ActiveTier,
    /// `r.tier` when last read, so each change applies once.
    seen: TierChoice,
    /// The device can time GPU passes, for each tier's dynamic resolution.
    timestamps: bool,
    dynamic: DynamicResolution,
    ladder: Ladder,
    /// The ladder of the tier a change replaced, for [`Self::restore`].
    previous_ladder: Option<Ladder>,
    /// What the last [`Self::render_scale`] chose.
    base_scale: f32,
    render_scale: f32,
}

/// Quality counters for the stats overlay (rule R9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityStats {
    pub tier: ActiveTier,
    /// The tier the device qualified for.
    pub device_tier: TierId,
    /// The last frame's base scale, and the scale it rendered at: the base lowered by dynamic
    /// resolution.
    pub base_scale: f32,
    pub render_scale: f32,
    pub dynamic: DynamicResolutionStats,
    pub ladder: LadderStats,
}

impl Quality {
    /// Selects `device`'s tier in `table`, under `gpus`' ceilings, and applies the overrides.
    pub fn new(table: TierTable, gpus: &GpuTable, device: &DeviceProfile, settings: QualitySettings) -> Self {
        let Selection { tier: device_tier, by: selected_by } = table.select(gpus, device);
        let seen = TIER.get();
        let active = choose_tier(seen, settings.tier, device_tier);
        let active = ActiveTier { id: active.id.under(settings.ceiling), ..active };
        let dynamic = DynamicResolution::new(table.get(active.id).resolution_budget(), device.timestamps);
        let ladder = Ladder::new(table.get(active.id));
        tracing::info!(
            target: LOG_TARGET,
            tier = active.id.name(),
            source = ?active.source,
            device_tier = device_tier.name(),
            by = %selected_by,
            ?device,
            "quality tier"
        );
        Self {
            table,
            device: device_tier,
            selected_by,
            settings,
            active,
            seen,
            timestamps: device.timestamps,
            dynamic,
            ladder,
            previous_ladder: None,
            base_scale: 1.0,
            render_scale: 1.0,
        }
    }

    /// The active tier's knobs.
    pub fn tier(&self) -> &QualityTier {
        self.table.get(self.active.id)
    }

    /// The frame rate the active tier is built for, under the ladder's cap, which the platform
    /// paces frames to (spec §6.2, M1 F7). Tables are validated, so it is never zero.
    pub fn frame_rate(&self) -> Option<NonZeroU32> {
        let tier = self.tier().frame_rate;
        NonZeroU32::new(self.ladder.state().frame_rate.map_or(tier, |cap| cap.min(tier)))
    }

    /// The active tier's thermal ladder.
    pub fn ladder(&self) -> &Ladder {
        &self.ladder
    }

    /// Follows heat and dynamic resolution's pressure along the active tier's ladder ([`ladder`]):
    /// call it once a frame, before [`Self::render_scale`], with the app's clock (it only moves
    /// forward) and the platform's reading. A move is logged and returned for the app to apply:
    /// [`Renderer::set_shed`](crate::Renderer::set_shed) with `to.shed` when the shed changed, and
    /// [`Self::frame_rate`] to the platform when the cap did; the scale follows in
    /// [`Self::render_scale`]. A tier the ladder asks for waits for [`Self::take_tier_request`].
    pub fn follow_heat(&mut self, now: Duration, reading: &ThermalReading) -> Option<LadderStep> {
        let forced = usize::try_from(RUNG.get()).ok();
        let asked = self.ladder.tier_request();
        let step = self.ladder.update(now, reading, self.dynamic.pressure(), forced);
        let asking = self.ladder.tier_request();
        if asking != asked {
            tracing::info!(
                target: LOG_TARGET,
                tier = asking.map(TierId::name),
                active = self.active.id.name(),
                "thermal ladder asks for a tier at the next pause"
            );
        }
        let step = step?;
        if step.from.frame_rate != step.to.frame_rate {
            self.dynamic.set_budget(self.tier().resolution_budget_at(self.capped_rate()));
        }
        tracing::info!(
            target: LOG_TARGET,
            depth = step.to.depth,
            from = step.from.depth,
            cause = ?step.cause,
            scale = format_args!("{:.2}", step.to.scale),
            shed = %step.to.shed,
            frame_rate = step.to.frame_rate,
            heat = %reading,
            "thermal ladder"
        );
        Some(step)
    }

    /// The tier the ladder asks for: lower, at its last rung, or back to the one it lowered from once
    /// calm. Never in a fight: the game takes it with [`Self::take_tier_request`] at a pause or a wipe.
    pub fn tier_request(&self) -> Option<TierId> {
        self.ladder.tier_request()
    }

    /// Makes the tier the ladder asks for active, at `now`: the game calls it at a pause or a wipe,
    /// then makes a new renderer from [`Self::render_settings`] (its pipelines warmed first, H1),
    /// sheds [`Ladder::state`]'s passes on it and paces to [`Self::frame_rate`], as for
    /// [`Self::update_tier`], or calls [`Self::restore`] with what this returns if that fails. The new
    /// tier's ladder starts at its deepest rung short of a tier and climbs back as its holds allow.
    pub fn take_tier_request(&mut self, now: Duration) -> Option<ActiveTier> {
        let ceiling = self.settings.ceiling;
        let id = self.ladder.tier_request().map(|id| id.under(ceiling)).filter(|&id| id != self.active.id)?;
        let previous = self.active;
        let lowered_from = (id < previous.id).then_some(previous.id);
        let ladder = Ladder::entered(self.table.get(id), &self.ladder, lowered_from, now);
        self.previous_ladder = Some(std::mem::replace(&mut self.ladder, ladder));
        self.active = ActiveTier { id, source: TierSource::Ladder };
        self.reset_dynamic();
        tracing::info!(
            target: LOG_TARGET,
            tier = id.name(),
            from = previous.id.name(),
            "quality tier changed by the thermal ladder"
        );
        Some(previous)
    }

    /// The player's settings.
    pub fn settings(&self) -> QualitySettings {
        self.settings
    }

    /// Takes the player's new settings (M4 T3). Dynamic resolution and the render scale follow from the next
    /// frame. A tier they choose, or automatic in place of one, waits for [`Self::wanted_tier`] and
    /// [`Self::change_tier`]; so do shadows and ambient occlusion, which only a new renderer changes.
    pub fn set_settings(&mut self, settings: QualitySettings) {
        let ceiling = match (self.settings.ceiling, settings.ceiling) {
            (Some(kept), Some(new)) if kept.class() == new.class() => Some(kept.min(new)),
            (kept, new) => kept.or(new),
        };
        let settings = QualitySettings { ceiling, ..settings };
        if settings != self.settings {
            tracing::info!(target: LOG_TARGET, ?settings, "quality settings");
        }
        self.settings = settings;
    }

    /// The tier `r.tier` or the player chose, if either did: one the ladder never changes ([`Self::wanted_tier`]).
    fn chosen(&self) -> Option<ActiveTier> {
        let chosen = choose_tier(TIER.get(), self.settings.tier, self.device);
        (chosen.source != TierSource::Device).then_some(chosen)
    }

    /// The tier the game should change to at its next pause or wipe (M4 T3), if not the active one: the one
    /// `r.tier` or the player chose; with neither, automatic, the ladder's request, or the device's tier in
    /// place of one a choice had made active. A chosen tier holds whatever the heat, its ladder stepping down
    /// only within it: the player asked for that look. The game builds its renderer from
    /// [`Self::render_settings_of`] off the frame first, then calls [`Self::change_tier`].
    pub fn wanted_tier(&self) -> Option<TierId> {
        let wanted = match self.chosen() {
            Some(chosen) => chosen.id,
            None => match self.active.source {
                TierSource::Setting | TierSource::Cvar => self.ladder.tier_request().unwrap_or(self.device),
                TierSource::Device | TierSource::Ladder => self.ladder.tier_request()?,
            },
        };
        let wanted = wanted.under(self.settings.ceiling);
        (wanted != self.active.id).then_some(wanted)
    }

    /// What a renderer for tier `id` is made with, under the player's settings: [`Self::render_settings`] for a
    /// tier not yet active.
    pub fn render_settings_of(&self, id: TierId) -> RenderSettings {
        self.settings_for(self.table.get(id))
    }

    /// Makes [`Self::wanted_tier`] active, at `now` on the ladder's clock, once the game has the new tier's
    /// renderer: at a pause or a wipe, never in a fight's frame. Returns the tier it replaced, for
    /// [`Self::restore`] if the renderer can't be taken after all. A ladder's tier starts at its deepest rung
    /// short of a tier ([`Self::take_tier_request`]); a chosen one, or the device's, at the top.
    pub fn change_tier(&mut self, now: Duration) -> Option<ActiveTier> {
        let id = self.wanted_tier()?;
        self.seen = TIER.get();
        let chosen = self.chosen();
        if chosen.is_none() && self.ladder.tier_request() == Some(id) {
            return self.take_tier_request(now);
        }
        let previous = self.active;
        let source = chosen.map_or(TierSource::Device, |chosen| chosen.source);
        let ladder = Ladder::new(self.table.get(id));
        self.previous_ladder = Some(std::mem::replace(&mut self.ladder, ladder));
        self.active = ActiveTier { id, source };
        self.reset_dynamic();
        tracing::info!(
            target: LOG_TARGET,
            tier = id.name(),
            from = previous.id.name(),
            ?source,
            device_by = %self.selected_by,
            "quality tier changed"
        );
        Some(previous)
    }

    /// The tier's frame rate under the ladder's cap.
    fn capped_rate(&self) -> u32 {
        self.frame_rate().map_or(self.tier().frame_rate, NonZeroU32::get)
    }

    /// A new dynamic resolution for the active tier at the ladder's frame rate.
    fn reset_dynamic(&mut self) {
        let budget = self.tier().resolution_budget_at(self.capped_rate());
        self.dynamic = DynamicResolution::new(budget, self.timestamps);
    }

    pub fn active(&self) -> ActiveTier {
        self.active
    }

    pub fn table(&self) -> &TierTable {
        &self.table
    }

    /// What a renderer for the active tier is created with, under the player's settings.
    pub fn render_settings(&self) -> RenderSettings {
        self.settings_for(self.tier())
    }

    /// What a renderer for `tier` is made with, under the player's settings.
    fn settings_for(&self, tier: &QualityTier) -> RenderSettings {
        let tier = tier.render_settings();
        self.settings.shed.shed(RenderSettings {
            shadows: tier.shadows.filter(|_| self.settings.shadows.resolve(true)),
            ambient_occlusion: self.settings.ambient_occlusion.resolve(tier.ambient_occlusion),
            ..tier
        })
    }

    /// Follows `r.tier`. When it changed since the last call to a choice that selects another
    /// tier, that tier becomes active and the one it replaced is returned: the app then makes a
    /// new renderer from [`Self::render_settings`], or calls [`Self::restore`] with it if that
    /// fails. The cvar isn't followed again until it changes again.
    pub fn update_tier(&mut self) -> Option<ActiveTier> {
        let choice = TIER.get();
        if choice == self.seen {
            return None;
        }
        self.seen = choice;
        let next = choose_tier(choice, self.settings.tier, self.device);
        let next = ActiveTier { id: next.id.under(self.settings.ceiling), ..next };
        let previous = std::mem::replace(&mut self.active, next);
        if previous.id == next.id {
            return None;
        }
        let ladder = Ladder::new(self.tier());
        self.previous_ladder = Some(std::mem::replace(&mut self.ladder, ladder));
        self.reset_dynamic();
        tracing::info!(
            target: LOG_TARGET,
            tier = next.id.name(),
            from = previous.id.name(),
            source = ?next.source,
            device_by = %self.selected_by,
            "quality tier changed"
        );
        Some(previous)
    }

    /// Makes the next lower tier of the active one's class active, for good: a renderer for the active tier
    /// couldn't be built (M4 H9, a boot's). It becomes [`QualitySettings::ceiling`], so neither the player's
    /// choice, `r.tier` nor the ladder's climb asks for the tier it fell from again. Its ladder starts at the top.
    /// Returns the tier it fell back to, or `None` at its class's lowest, where nothing is left to fall back to.
    pub fn fall_back(&mut self) -> Option<TierId> {
        let from = self.active.id;
        let to = from.lower()?;
        self.settings.ceiling = Some(to);
        self.active.id = to;
        self.ladder = Ladder::new(self.table.get(to));
        self.previous_ladder = None;
        self.reset_dynamic();
        tracing::warn!(
            target: LOG_TARGET,
            tier = to.name(),
            from = from.name(),
            "a renderer couldn't be built at the quality tier: it falls back to the next lower"
        );
        Some(to)
    }

    /// Makes `previous` (from [`Self::update_tier`] or [`Self::take_tier_request`]) active again,
    /// with its ladder as it was.
    pub fn restore(&mut self, previous: ActiveTier) {
        self.active = previous;
        if let Some(ladder) = self.previous_ladder.take() {
            self.ladder = ladder;
        }
        self.reset_dynamic();
    }

    /// Whether dynamic resolution is on, so the frame graph should time passes on the GPU
    /// ([`GraphExecutor::set_gpu_timing`](crate::graph::GraphExecutor::set_gpu_timing)).
    pub fn wants_gpu_timing(&self) -> bool {
        DYNAMIC_RESOLUTION.get().resolve(self.settings.dynamic_resolution)
    }

    /// The render scale for the next frame into an output of `size`, given what `timing`
    /// measured: the base scale, lowered by dynamic resolution while it is on. Pass it to
    /// [`Renderer::set_render_scale`](crate::Renderer::set_render_scale) before recording.
    pub fn render_scale(&mut self, size: (u32, u32), timing: &FrameTiming) -> f32 {
        let cvar = RENDER_SCALE.get();
        let base = if cvar > 0.0 {
            cvar
        } else {
            self.settings.render_scale.unwrap_or_else(|| self.tier().resolution.base_scale(size.1))
        };
        let base = if base.is_finite() { base.clamp(MIN_RENDER_SCALE, 1.0) } else { 1.0 };
        self.dynamic.set_enabled(self.wants_gpu_timing());
        self.dynamic.limit_floor(MIN_RENDER_SCALE / base);
        self.dynamic.set_ceiling(self.ladder.state().scale);
        let (width, height) = scaled_size(size, base);
        let factor = self.dynamic.update(timing, width as f32 * height as f32);
        self.base_scale = base;
        self.render_scale = (base * factor).clamp(MIN_RENDER_SCALE, 1.0);
        self.render_scale
    }

    /// The scale on materials' outline LOD cutoffs (`r.outline_distance`, 1 unless tuned): pass it
    /// to [`Renderer::set_outline_distance`](crate::Renderer::set_outline_distance) before
    /// recording.
    pub fn outline_distance(&self) -> f32 {
        let scale = OUTLINE_DISTANCE.get();
        if scale.is_finite() { scale.max(0.0) } else { 1.0 }
    }

    pub fn stats(&self) -> QualityStats {
        QualityStats {
            tier: self.active,
            device_tier: self.device,
            base_scale: self.base_scale,
            render_scale: self.render_scale,
            dynamic: self.dynamic.stats(),
            ladder: self.ladder.stats(),
        }
    }
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device whose adapter reports no vendor and no name, which the GPU table never knows.
    fn device(class: FormFactor, gpu: GpuKind, backend: GpuBackend, ram_gib: u32) -> DeviceProfile {
        DeviceProfile {
            class,
            gpu,
            backend,
            vendor: 0,
            adapter: String::new(),
            ram_mib: ram_gib * 1024,
            timestamps: true,
            handheld: false,
        }
    }

    /// `profile` with its adapter named `name` by `vendor`.
    fn named(profile: DeviceProfile, vendor: u32, name: &str) -> DeviceProfile {
        DeviceProfile { vendor, adapter: name.into(), ..profile }
    }

    /// The shipped table with `edit` applied to `id`'s tier, unchecked.
    fn edited(id: TierId, edit: impl FnOnce(&mut QualityTier)) -> Vec<QualityTier> {
        let mut tiers = TierTable::shipped().tiers;
        edit(tiers.iter_mut().find(|tier| tier.id == id).unwrap());
        tiers
    }

    fn invalid_field(tiers: Vec<QualityTier>) -> &'static str {
        match TierTable::new(tiers) {
            Err(TierError::Invalid { field, .. }) => field,
            other => panic!("expected an invalid field, got {other:?}"),
        }
    }

    /// Every tier's shadow maps fit Vulkan 1.1's least 2D texture side, the least of any device the
    /// engine takes (`sapphire_rhi::caps`), so a shipped tier never meets `RenderError::ShadowMapTooLarge`.
    #[test]
    fn the_shipped_tiers_shadow_maps_fit_every_device() {
        for tier in TierTable::shipped().tiers() {
            assert!(tier.shadows.check_limit(4096).is_ok(), "{}", tier.id);
        }
    }

    #[test]
    fn the_shipped_table_is_valid_and_round_trips() {
        let table = TierTable::shipped();
        assert_eq!(table.tiers().len(), 4);
        let text = table.to_ron().expect("write");
        assert!(
            text.starts_with("(\n    type: \"sapphire::render::TierTable\",\n    version: 1,\n"),
            "{text}"
        );
        assert_eq!(TierTable::from_ron(&text).expect("read back"), table);
        for id in TierId::ALL {
            assert_eq!(table.get(id).id, id);
        }
    }

    #[test]
    fn desktop_tiers_resolve_with_taa_and_phones_with_msaa() {
        // Desktop tiers: temporal AA with MSAA off, otherwise the default settings, which the
        // golden scenes from before M1 R7 pin with 4× MSAA instead (`--aa msaa`). At 640×360 and
        // 1920×1080 (the goldens' and the bench's sizes) both render at native height. The
        // radial blur takes 16 taps on Desktop-High and the default 12 on Desktop-Low (M1 P3).
        let table = TierTable::shipped();
        for (id, taps) in [(TierId::DesktopHigh, 16), (TierId::DesktopLow, 12)] {
            let tier = table.get(id);
            let shadows = Some(tier.shadows);
            let post_effects = PostEffectSettings { radial_blur_taps: taps };
            let ambient_occlusion = tier.ambient_occlusion;
            let expected = RenderSettings {
                msaa: Msaa::Off,
                temporal: true,
                shadows,
                post_effects,
                ambient_occlusion,
                inset: tier.inset,
                ..RenderSettings::default()
            };
            assert_eq!(tier.render_settings(), expected, "{id}");
            assert_eq!(tier.inset.supersample, 2, "{id}: no MSAA, so an inset supersamples");
            assert_eq!(tier.resolution.base_scale(360), 1.0, "{id}");
            assert_eq!(tier.resolution.base_scale(1080), 1.0, "{id}: the bench's size");
        }
        // Phones: 4× MSAA kept on tile and no temporal AA (the renderer makes the multisampled
        // attachments transient: `renderer::tests`).
        for id in [TierId::MobileHigh, TierId::MobileLow] {
            let settings = table.get(id).render_settings();
            assert_eq!((settings.msaa, settings.temporal), (Msaa::X4, false), "{id}");
            assert_eq!(
                (settings.light_path, settings.max_lights),
                (LightPath::PerObject, 8),
                "{id}: spec §3.4's mobile budget"
            );
            assert_eq!(settings.post_effects.radial_blur_taps, 8, "{id}");
            assert_eq!(settings.inset.supersample, 1, "{id}: an inset takes the main pass's MSAA");
        }
    }

    #[test]
    fn phones_take_cheaper_environment_specular() {
        use crate::EnvironmentSpecular as Specular;
        let table = TierTable::shipped();
        let specular = |id| table.get(id).render_settings().environment.specular;
        // The desktop tiers draw the default, which the golden images render with.
        assert_eq!(specular(TierId::DesktopHigh), Specular::Full);
        assert_eq!(specular(TierId::DesktopLow), Specular::Full);
        assert_eq!(specular(TierId::MobileHigh), Specular::Approximate);
        assert_eq!(specular(TierId::MobileLow), Specular::Off);
    }

    #[test]
    fn mobile_low_reads_the_probe_grid_per_vertex() {
        let table = TierTable::shipped();
        let lookup = |id| table.get(id).render_settings().environment.probes;
        for id in [TierId::DesktopHigh, TierId::DesktopLow, TierId::MobileHigh] {
            assert_eq!(lookup(id), crate::ProbeLookup::PerPixel, "{id}");
        }
        assert_eq!(lookup(TierId::MobileLow), crate::ProbeLookup::PerVertex);
    }

    #[test]
    fn every_tier_has_capsule_ao_and_gtao_runs_on_desktop_and_mobile_high() {
        let table = TierTable::shipped();
        for id in TierId::ALL {
            let occlusion = table.get(id).render_settings().ambient_occlusion;
            assert!(occlusion.capsules, "{id}");
            assert_eq!(occlusion.gtao.is_some(), id != TierId::MobileLow, "{id}");
        }
        // The desktop tiers search alike, so golden images match on every desktop GPU.
        let desktop = |id| table.get(id).ambient_occlusion.gtao;
        assert_eq!(desktop(TierId::DesktopHigh), Some(GtaoSettings::DESKTOP));
        assert_eq!(desktop(TierId::DesktopLow), Some(GtaoSettings::DESKTOP));
        // The player's setting over the tier's.
        let tier = table.get(TierId::MobileLow).ambient_occlusion;
        assert_eq!(OcclusionSetting::Auto.resolve(tier), tier);
        assert_eq!(OcclusionSetting::Off.resolve(tier), AmbientOcclusion::OFF);
        assert_eq!(OcclusionSetting::Capsules.resolve(tier), AmbientOcclusion { capsules: true, gtao: None });
        assert_eq!(OcclusionSetting::Full.resolve(tier).gtao, Some(GtaoSettings::DESKTOP));
        let mobile_high = table.get(TierId::MobileHigh).ambient_occlusion;
        assert_eq!(OcclusionSetting::Full.resolve(mobile_high), mobile_high, "its own GTAO");
    }

    #[test]
    fn base_scales_hold_the_internal_height() {
        let table = TierTable::shipped();
        // Mobile-Low's ~590 rows on an iPhone 12 in landscape (2532×1170).
        let mobile_low = table.get(TierId::MobileLow).resolution.base_scale(1170);
        assert!((mobile_low * 1170.0 - 590.0).abs() < 1.0, "{mobile_low}");
        // Desktop-High: 1440p native, or 4K upsampled from 1440 rows by temporal AA.
        let desktop_high = &table.get(TierId::DesktopHigh).resolution;
        assert_eq!(desktop_high.base_scale(1440), 1.0);
        assert_eq!(scaled_size((3840, 2160), desktop_high.base_scale(2160)), (2560, 1440));
        // Desktop-Low: 1080 rows.
        let desktop_low = &table.get(TierId::DesktopLow).resolution;
        assert_eq!(desktop_low.base_scale(1080), 1.0);
        assert_eq!(scaled_size((2560, 1440), desktop_low.base_scale(1440)), (1920, 1080));
        assert_eq!(scaled_size((3840, 2160), desktop_low.base_scale(2160)), (1920, 1080));
        let tiny = Resolution { scale: 1.0, max_height: Some(10), dynamic_floor: 1.0 };
        assert_eq!(tiny.base_scale(1000), MIN_RENDER_SCALE, "never below the minimum");
    }

    /// A change to one tier, and the field it makes invalid.
    type BadValue = (TierId, &'static str, fn(&mut QualityTier));

    #[test]
    fn bad_values_are_rejected() {
        use TierId::*;
        let cases: [BadValue; 20] = [
            (DesktopHigh, "resolution.scale", |tier| tier.resolution.scale = 0.0),
            (DesktopHigh, "resolution.scale", |tier| tier.resolution.scale = 1.5),
            (DesktopLow, "resolution.scale", |tier| tier.resolution.scale = f32::NAN),
            (MobileHigh, "resolution.max_height", |tier| tier.resolution.max_height = Some(0)),
            (MobileLow, "resolution.dynamic_floor", |tier| tier.resolution.dynamic_floor = 0.1),
            (MobileLow, "frame_rate", |tier| tier.frame_rate = 0),
            (MobileLow, "gpu_frame_ms", |tier| tier.gpu_frame_ms = 40.0),
            (DesktopLow, "gpu_frame_ms", |tier| tier.gpu_frame_ms = -1.0),
            (DesktopHigh, "bloom_levels", |tier| tier.bloom_levels = 7),
            (DesktopHigh, "max_lights", |tier| tier.max_lights = 2000),
            (MobileHigh, "shadows.cascades", |tier| tier.shadows.cascades = 5),
            (MobileHigh, "shadows.resolution", |tier| tier.shadows.resolution = 1000),
            (DesktopLow, "shadows.distance", |tier| tier.shadows.distance = 0),
            (DesktopHigh, "shadows.cached_cascades", |tier| tier.shadows.cached_cascades = 4),
            (MobileLow, "shadows.cached_cascades", |tier| tier.shadows.cached_cascades = 2),
            (DesktopHigh, "shadows.character_resolution", |tier| tier.shadows.character_resolution = 100),
            (DesktopLow, "lod_bias", |tier| tier.lod_bias = f32::INFINITY),
            (DesktopHigh, "ambient_occlusion.gtao.slices", |tier| {
                tier.ambient_occlusion.gtao = Some(GtaoSettings { slices: 9, ..GtaoSettings::DESKTOP });
            }),
            (MobileHigh, "ambient_occlusion.gtao.radius", |tier| {
                tier.ambient_occlusion.gtao = Some(GtaoSettings { radius: -1.0, ..GtaoSettings::DESKTOP });
            }),
            (MobileLow, "reflections.cube_size", |tier| tier.reflections.cube_size = 100),
        ];
        for (id, field, edit) in cases {
            assert_eq!(invalid_field(edited(id, edit)), field, "{id}");
        }

        let mut tiers = TierTable::shipped().tiers;
        tiers.retain(|tier| tier.id != MobileHigh);
        assert_eq!(TierTable::new(tiers.clone()), Err(TierError::Missing(MobileHigh)));
        tiers.push(tiers[0].clone());
        tiers.push(TierTable::shipped().get(MobileHigh).clone());
        assert!(matches!(TierTable::new(tiers), Err(TierError::Duplicate(_))));
        let rules = edited(MobileLow, |tier| tier.select_when.push(TierRule::default()));
        assert_eq!(TierTable::new(rules), Err(TierError::ConditionalLowest(MobileLow)));
    }

    #[test]
    fn documents_are_checked_as_they_load() {
        let text = TierTable::shipped().to_ron().unwrap().replace("bloom_levels: 5,", "bloom_levels: 9,");
        let error = TierTable::from_ron(&text).unwrap_err().to_string();
        assert!(error.contains("bloom_levels"), "{error}");
        // A knob left out is an error, not a silent default.
        let text = TierTable::shipped().to_ron().unwrap().replacen("max_particles: 50000,", "", 1);
        assert!(TierTable::from_ron(&text).is_err());
        let newer = SHIPPED.replace("version: 1", "version: 2");
        assert!(matches!(TierTable::from_ron(&newer), Err(DocumentError::TooNew { found: 2, current: 1 })));
    }

    #[test]
    fn selection_follows_the_device_class_and_gpu() {
        use FormFactor::{Desktop, Mobile};
        use GpuBackend::{Dx12, Metal, Vulkan};
        use GpuKind::{Cpu, Discrete, Integrated, Virtual};
        let table = TierTable::shipped();
        let gpus = GpuTable::shipped();
        let pick = |profile: DeviceProfile| table.select(&gpus, &profile).tier;
        // Desktop-High: a discrete GPU, or Apple silicon, with 16 GB.
        assert_eq!(pick(device(Desktop, Discrete, Vulkan, 32)), TierId::DesktopHigh);
        assert_eq!(pick(device(Desktop, Discrete, Dx12, 16)), TierId::DesktopHigh);
        assert_eq!(pick(device(Desktop, Integrated, Metal, 16)), TierId::DesktopHigh, "M2 Pro");
        assert_eq!(pick(device(Desktop, Integrated, Metal, 8)), TierId::DesktopLow, "M1 8 GB");
        assert_eq!(pick(device(Desktop, Integrated, Vulkan, 16)), TierId::DesktopLow, "Steam Deck");
        assert_eq!(pick(device(Desktop, Discrete, Vulkan, 8)), TierId::DesktopLow);
        // Software and paravirtual GPUs (the golden-image runners) take the lowest tier.
        assert_eq!(pick(device(Desktop, Cpu, Vulkan, 64)), TierId::DesktopLow, "lavapipe");
        assert_eq!(pick(device(Desktop, Virtual, Metal, 16)), TierId::DesktopLow);
        // Phones: 8 GB for Mobile-High, never a desktop tier.
        assert_eq!(pick(device(Mobile, Integrated, Metal, 8)), TierId::MobileHigh, "iPhone 15 Pro");
        assert_eq!(pick(device(Mobile, Integrated, Vulkan, 12)), TierId::MobileHigh);
        assert_eq!(pick(device(Mobile, Integrated, Metal, 4)), TierId::MobileLow, "iPhone 12");
        assert_eq!(pick(device(Mobile, Integrated, Vulkan, 6)), TierId::MobileLow, "Snapdragon 865");
        assert_eq!(pick(device(Mobile, GpuKind::Other, Vulkan, 4)), TierId::MobileLow, "an emulator");
        assert_eq!(pick(device(Mobile, Discrete, Vulkan, 64)).class(), Mobile);
    }

    /// M4 T4: a desktop adapter the GPU table knows takes no higher than its family's ceiling, and the
    /// selection says which rule chose.
    #[test]
    fn a_known_gpu_takes_no_higher_than_its_family() {
        use FormFactor::{Desktop, Mobile};
        use GpuKind::{Discrete, Integrated};
        let table = TierTable::shipped();
        let gpus = GpuTable::shipped();
        let pick = |profile: DeviceProfile| table.select(&gpus, &profile);
        let discrete = |ram_gib, vendor, name| {
            named(device(Desktop, Discrete, GpuBackend::Vulkan, ram_gib), vendor, name)
        };
        let rules = |gpu: Option<&str>| SelectedBy::Rules { gpu: gpu.map(Into::into) };
        // A GTX 1060 with 16 GB, which the RAM rule alone made Desktop-High.
        let gtx_1060 = discrete(16, 0x10DE, "NVIDIA GeForce GTX 1060 6GB");
        assert_eq!(
            pick(gtx_1060.clone()),
            Selection {
                tier: TierId::DesktopLow,
                by: SelectedBy::Gpu { family: "GeForce GTX".into(), rules: TierId::DesktopHigh },
            }
        );
        // A known high-end card: Desktop-High, with the RAM rule still deciding under its ceiling.
        let rtx_3070 = |ram_gib| discrete(ram_gib, 0x10DE, "NVIDIA GeForce RTX 3070");
        assert_eq!(
            pick(rtx_3070(32)),
            Selection { tier: TierId::DesktopHigh, by: rules(Some("GeForce RTX")) }
        );
        assert_eq!(pick(rtx_3070(8)), Selection { tier: TierId::DesktopLow, by: rules(Some("GeForce RTX")) });
        let rx_6700 = discrete(16, 0x1002, "AMD Radeon RX 6700 XT");
        assert_eq!(pick(rx_6700).tier, TierId::DesktopHigh);
        // An adapter the table doesn't know falls to the rules unchanged.
        let unknown = discrete(16, 0x10DE, "NVIDIA GeForce RTX 6090");
        assert_eq!(pick(unknown), Selection { tier: TierId::DesktopHigh, by: rules(None) });
        // Apple silicon on Metal reports no vendor: the name alone. An M1 with 16 GB is Desktop-Low, an M2 Pro
        // Desktop-High.
        let apple = |ram_gib, name| named(device(Desktop, Integrated, GpuBackend::Metal, ram_gib), 0, name);
        assert_eq!(pick(apple(16, "Apple M1")).tier, TierId::DesktopLow);
        assert_eq!(pick(apple(16, "Apple M2 Pro")).tier, TierId::DesktopHigh);
        // Phones never read the table, and a handheld keeps its own rule.
        let phone = named(device(Mobile, Integrated, GpuBackend::Metal, 8), 0, "Apple M1");
        assert_eq!(pick(phone), Selection { tier: TierId::MobileHigh, by: rules(None) });
        let handheld = discrete(32, 0x10DE, "NVIDIA GeForce RTX 4090").handheld(true);
        assert_eq!(pick(handheld), Selection { tier: TierId::DesktopLow, by: SelectedBy::Handheld });
        // The player's tier, and `r.tier` over it (`overrides_take_precedence_in_order`), still win.
        let chosen = QualitySettings { tier: Some(TierId::DesktopHigh), ..QualitySettings::default() };
        let quality = Quality::new(table.clone(), &gpus, &gtx_1060, chosen);
        assert_eq!(quality.active(), ActiveTier { id: TierId::DesktopHigh, source: TierSource::Setting });
        assert_eq!(quality.stats().device_tier, TierId::DesktopLow);
    }

    /// M4 T4: the adapters goldens are drawn on select exactly what they did before the GPU table, at any RAM:
    /// CI's lavapipe and WARP, macOS runners' paravirtual GPU, and the Apple M5 the Metal goldens come from.
    #[test]
    fn golden_adapters_select_as_before_the_gpu_table() {
        use GpuBackend::{Dx12, Metal, Vulkan};
        use GpuKind::{Cpu, Integrated, Virtual};
        let table = TierTable::shipped();
        let shipped = GpuTable::shipped();
        let none = GpuTable::new(Vec::new()).unwrap();
        let adapters = [
            (Cpu, Vulkan, 0x10005, "llvmpipe (LLVM 15.0.7, 256 bits)", TierId::DesktopLow),
            (Cpu, Dx12, 0x1414, "Microsoft Basic Render Driver", TierId::DesktopLow),
            (Virtual, Metal, 0, "Apple Paravirtual device", TierId::DesktopLow),
            (Integrated, Metal, 0, "Apple M5", TierId::DesktopHigh),
        ];
        for (gpu, backend, vendor, name, at_24_gib) in adapters {
            for ram_gib in [4, 8, 16, 24, 64] {
                let profile = named(device(FormFactor::Desktop, gpu, backend, ram_gib), vendor, name);
                let selection = table.select(&shipped, &profile);
                assert_eq!(selection, table.select(&none, &profile), "{name} with {ram_gib} GiB");
                assert_eq!(selection.by, SelectedBy::Rules { gpu: None }, "{name}");
            }
            let profile = named(device(FormFactor::Desktop, gpu, backend, 24), vendor, name);
            assert_eq!(table.select(&shipped, &profile).tier, at_24_gib, "{name}: this Mac's RAM");
        }
    }

    #[test]
    fn a_handheld_starts_on_its_class_s_lowest_tier() {
        let table = TierTable::shipped();
        let gpus = GpuTable::shipped();
        let pick = |profile: &DeviceProfile| table.select(&gpus, profile).tier;
        // A Steam Deck, as `sapphire_platform` names it: Desktop-Low, whatever a later handheld's GPU and RAM.
        let deck = device(FormFactor::Desktop, GpuKind::Integrated, GpuBackend::Vulkan, 16).handheld(true);
        assert_eq!(pick(&deck), TierId::DesktopLow);
        let stronger = device(FormFactor::Desktop, GpuKind::Discrete, GpuBackend::Vulkan, 32).handheld(true);
        assert_eq!(pick(&stronger), TierId::DesktopLow);
        assert_eq!(pick(&stronger.handheld(false)), TierId::DesktopHigh);
        let quality = Quality::new(table, &gpus, &deck, QualitySettings::default());
        assert_eq!(quality.active(), ActiveTier { id: TierId::DesktopLow, source: TierSource::Device });
    }

    #[test]
    fn profiles_name_the_gpu_as_wgpu_reports_it() {
        // `DeviceProfile::new` reads these two from `GpuCaps::adapter`.
        assert_eq!(GpuKind::from(wgpu::DeviceType::Cpu), GpuKind::Cpu);
        assert_eq!(GpuKind::from(wgpu::DeviceType::IntegratedGpu), GpuKind::Integrated);
        assert_eq!(GpuKind::from(wgpu::DeviceType::DiscreteGpu), GpuKind::Discrete);
        assert_eq!(GpuKind::from(wgpu::DeviceType::VirtualGpu), GpuKind::Virtual);
        assert_eq!(GpuBackend::from(wgpu::Backend::Metal), GpuBackend::Metal);
        assert_eq!(GpuBackend::from(wgpu::Backend::Dx12), GpuBackend::Dx12);
        assert_eq!(GpuBackend::from(wgpu::Backend::Noop), GpuBackend::Other);
    }

    #[test]
    fn overrides_take_precedence_in_order() {
        use TierId::*;
        let device = DesktopHigh;
        assert_eq!(
            choose_tier(TierChoice::Auto, None, device),
            ActiveTier { id: device, source: TierSource::Device }
        );
        assert_eq!(
            choose_tier(TierChoice::Auto, Some(MobileLow), device),
            ActiveTier { id: MobileLow, source: TierSource::Setting }
        );
        assert_eq!(
            choose_tier(TierChoice::Tier(DesktopLow), Some(MobileLow), device),
            ActiveTier { id: DesktopLow, source: TierSource::Cvar },
            "the cvar wins over the player's setting"
        );
        assert!(Switch::Auto.resolve(true) && !Switch::Auto.resolve(false));
        assert!(Switch::On.resolve(false) && !Switch::Off.resolve(true));
    }

    #[test]
    fn cvar_choices_round_trip() {
        for choice in [TierChoice::Auto].into_iter().chain(TierId::ALL.map(TierChoice::Tier)) {
            assert_eq!(TierChoice::from_bits(choice.to_bits()), choice);
            assert_eq!(TierChoice::parse(&choice.to_value().to_string()), Some(choice));
            assert_eq!(TierChoice::from_value(choice.to_value()), Some(choice));
        }
        assert_eq!(TierChoice::parse("Mobile-Low"), Some(TierChoice::Tier(TierId::MobileLow)));
        assert_eq!(TierChoice::parse("ultra"), None);
        for id in TierId::ALL {
            assert_eq!(TierId::from_key(id.key()), Some(id));
        }
        for switch in [Switch::Auto, Switch::On, Switch::Off] {
            assert_eq!(Switch::parse(&switch.to_value().to_string()), Some(switch));
        }
    }

    /// A [`Quality`] for a desktop that selects Desktop-High, with `settings`.
    fn quality(settings: QualitySettings) -> Quality {
        let profile = device(FormFactor::Desktop, GpuKind::Discrete, GpuBackend::Vulkan, 32);
        Quality::new(TierTable::shipped(), &GpuTable::shipped(), &profile, settings)
    }

    /// At a low base scale the controller stops where the renderer's minimum does: its factor
    /// and pixels stay the scene's, under a load that keeps asking for less (OPT-017).
    #[test]
    fn dynamic_resolution_stops_at_what_the_renderer_can_draw() {
        let timing = |frame| FrameTiming {
            frame,
            gpu: Some((frame.saturating_sub(1), Duration::from_secs(1))),
            frame_time: Duration::from_secs(1),
        };
        let mut controller = DynamicResolution::new(
            ResolutionBudget {
                gpu_frame: Duration::from_millis(12),
                frame: Duration::from_millis(16),
                floor: 0.7,
            },
            true,
        );
        for (base, lowest) in [(0.25, 1.0), (0.3, 0.25 / 0.3), (0.5, 0.7), (1.0, 0.7)] {
            controller.set_enabled(false);
            controller.limit_floor(MIN_RENDER_SCALE / base);
            controller.set_enabled(true);
            let mut factor = 1.0;
            for frame in 1..200 {
                factor = controller.update(&timing(frame), 1_000_000.0 * base * base);
            }
            assert!((factor - lowest).abs() < 1e-5, "at {base}: {factor}, not {lowest}");
            assert!(base * factor >= MIN_RENDER_SCALE - 1e-5, "at {base}: {}", base * factor);
            assert_eq!(controller.stats().floor, lowest);
        }
        // A lower base later pulls a factor already past its floor back up to it.
        controller.limit_floor(MIN_RENDER_SCALE / 0.25);
        assert_eq!(controller.factor(), 1.0);
    }

    /// Each optional pass sheds one step of what its tier draws, nothing where it draws none,
    /// and nothing else: the look's own settings stay the tier's.
    #[test]
    fn optional_passes_shed_one_step_and_nothing_else() {
        let table = TierTable::shipped();
        let desktop_low = table.get(TierId::DesktopLow).render_settings();
        let mobile_low = table.get(TierId::MobileLow).render_settings();
        let shed = |pass: OptionalPass, settings| pass.shed(settings);
        assert_eq!(
            shed(OptionalPass::Gtao, desktop_low).ambient_occlusion,
            AmbientOcclusion { capsules: true, gtao: None }
        );
        assert_eq!(shed(OptionalPass::Gtao, mobile_low), mobile_low, "Mobile-Low has no GTAO");
        assert!(!shed(OptionalPass::Cirrus, desktop_low).sky.cirrus);
        assert_eq!(shed(OptionalPass::Cirrus, mobile_low), mobile_low, "Mobile-Low has no cirrus");
        assert_eq!(shed(OptionalPass::WindLevel, desktop_low).foliage.wind_levels, 2);
        assert_eq!(shed(OptionalPass::WindLevel, mobile_low).foliage.wind_levels, 1);
        assert_eq!(shed(OptionalPass::SkyFaceRate, desktop_low).sky.cube_interval, 2);
        assert_eq!(shed(OptionalPass::SkyFaceRate, mobile_low).sky.cube_interval, 4);
        assert_eq!(shed(OptionalPass::BloomLevel, desktop_low).bloom_levels, 4);
        assert_eq!(shed(OptionalPass::BloomLevel, mobile_low).bloom_levels, 3);
        let still = RenderSettings {
            foliage: FoliageSettings { wind_levels: 0, ..mobile_low.foliage },
            ..mobile_low
        };
        assert_eq!(shed(OptionalPass::WindLevel, still), still, "still foliage stays still");
        let slowest =
            RenderSettings { sky: SkySettings { cube_interval: 60, ..mobile_low.sky }, ..mobile_low };
        assert_eq!(shed(OptionalPass::SkyFaceRate, slowest).sky.cube_interval, 60);

        let all: OptionalPasses = OptionalPass::ALL.into_iter().collect();
        for tier in table.tiers() {
            let settings = tier.render_settings();
            let shed = all.shed(settings);
            assert!(shed.sky.validate().is_ok() && shed.foliage.validate().is_ok(), "{}", tier.id);
            // Put back what may be shed: everything else, the look's own, is the tier's.
            let restored = RenderSettings {
                ambient_occlusion: settings.ambient_occlusion,
                sky: settings.sky,
                foliage: settings.foliage,
                bloom_levels: settings.bloom_levels,
                ..shed
            };
            assert_eq!(restored, settings, "{}", tier.id);
        }
        for pass in OptionalPass::ALL {
            assert_eq!(OptionalPass::from_key(pass.key()), Some(pass));
            assert!(all.contains(pass) && !OptionalPasses::NONE.contains(pass));
        }
        assert_eq!(OptionalPass::from_key("Cirrus"), Some(OptionalPass::Cirrus));
        assert_eq!(OptionalPass::from_key("fog"), None);
        assert_eq!(all.iter().collect::<Vec<_>>(), OptionalPass::ALL);
        let one = OptionalPasses::NONE.with(OptionalPass::Cirrus);
        assert_eq!(one.iter().collect::<Vec<_>>(), [OptionalPass::Cirrus]);
        assert!(OptionalPasses::NONE.is_empty() && !one.is_empty());

        // The player's settings apply first: GTAO asked for and shed is gone.
        let full = quality(QualitySettings {
            ambient_occlusion: OcclusionSetting::Full,
            shed: OptionalPasses::NONE.with(OptionalPass::Gtao),
            ..QualitySettings::default()
        });
        assert_eq!(full.render_settings().ambient_occlusion.gtao, None);
    }

    // The tests below set the module's cvars, which are process-wide, so they run as one.
    #[test]
    fn quality_follows_cvars_and_settings() {
        let timing = FrameTiming::default();
        // The player's tier and scale apply under `auto`.
        let mut player = quality(QualitySettings {
            tier: Some(TierId::MobileLow),
            render_scale: Some(0.7),
            dynamic_resolution: false,
            shadows: Switch::Auto,
            ambient_occlusion: OcclusionSetting::Auto,
            ceiling: None,
            shed: OptionalPasses::NONE,
        });
        assert_eq!(player.active(), ActiveTier { id: TierId::MobileLow, source: TierSource::Setting });
        assert_eq!(player.render_scale((1920, 1080), &timing), 0.7);
        assert!(!player.wants_gpu_timing());
        let mobile_low = player.tier().shadows;
        assert_eq!(player.render_settings().shadows, Some(mobile_low), "the tier's shadows");
        let unshadowed = quality(QualitySettings { shadows: Switch::Off, ..QualitySettings::default() });
        assert_eq!(unshadowed.render_settings().shadows, None, "the player turned them off");
        let unoccluded = quality(QualitySettings {
            ambient_occlusion: OcclusionSetting::Off,
            ..QualitySettings::default()
        });
        assert_eq!(unoccluded.render_settings().ambient_occlusion, AmbientOcclusion::OFF);

        let mut quality = quality(QualitySettings::default());
        assert_eq!(quality.active().id, TierId::DesktopHigh);
        assert_eq!(quality.render_scale((1920, 1080), &timing), 1.0);
        assert_eq!(quality.render_scale((3840, 2160), &timing), 1440.0 / 2160.0, "4K from 1440 rows");
        assert_eq!(quality.update_tier(), None, "r.tier hasn't changed");

        // A tier set through the cvar applies once, and back.
        TIER.set(TierChoice::Tier(TierId::DesktopLow)).unwrap();
        let previous = quality.update_tier().expect("a new tier");
        assert_eq!(previous.id, TierId::DesktopHigh);
        assert_eq!(quality.active(), ActiveTier { id: TierId::DesktopLow, source: TierSource::Cvar });
        assert_eq!(quality.update_tier(), None, "applied once");
        quality.restore(previous);
        assert_eq!(quality.active().id, TierId::DesktopHigh, "a failed rebuild goes back");
        assert_eq!(quality.update_tier(), None, "and isn't retried until the cvar changes");
        TIER.reset();
        assert_eq!(quality.update_tier(), None, "auto selects the tier already active");

        // Render scale and dynamic resolution through their cvars.
        RENDER_SCALE.set(0.5).unwrap();
        assert_eq!(quality.render_scale((1920, 1080), &timing), 0.5);
        RENDER_SCALE.set(0.1).unwrap();
        assert_eq!(quality.render_scale((1920, 1080), &timing), MIN_RENDER_SCALE);
        RENDER_SCALE.reset();
        DYNAMIC_RESOLUTION.set(Switch::On).unwrap();
        assert!(quality.wants_gpu_timing());
        quality.render_scale((1920, 1080), &timing);
        assert!(quality.stats().dynamic.enabled);
        DYNAMIC_RESOLUTION.reset();
        quality.render_scale((1920, 1080), &timing);
        let stats = quality.stats();
        assert!(!stats.dynamic.enabled);
        assert_eq!(
            (stats.base_scale, stats.render_scale, stats.device_tier),
            (1.0, 1.0, TierId::DesktopHigh)
        );
        // Here rather than a test of its own: both read `r.tier`, which this one sets.
        tier_changes_wait_for_the_game();
        a_tier_that_cant_be_built_falls_back_for_good();
    }

    /// M4 H9: a tier whose renderer couldn't be built falls to the next lower of its class, and nothing asks for
    /// it again: not the player's choice, not `r.tier`, not the ladder's climb. A class's lowest has nowhere to go.
    fn a_tier_that_cant_be_built_falls_back_for_good() {
        use sapphire_core::thermal::ThermalState;
        assert_eq!(TierId::DesktopHigh.lower(), Some(TierId::DesktopLow));
        assert_eq!(TierId::MobileHigh.lower(), Some(TierId::MobileLow));
        assert_eq!((TierId::DesktopLow.lower(), TierId::MobileLow.lower()), (None, None));
        let low = Some(TierId::MobileLow);
        assert_eq!(TierId::MobileHigh.under(low), TierId::MobileLow);
        assert_eq!(TierId::DesktopHigh.under(low), TierId::DesktopHigh, "a ceiling holds its class alone");

        // A phone holding Mobile-High, as the player chose it.
        let phone = device(FormFactor::Mobile, GpuKind::Integrated, GpuBackend::Vulkan, 8);
        let held = QualitySettings { tier: Some(TierId::MobileHigh), ..QualitySettings::default() };
        let mut quality = Quality::new(TierTable::shipped(), &GpuTable::shipped(), &phone, held);
        assert_eq!(quality.active().id, TierId::MobileHigh);
        assert_eq!(quality.fall_back(), Some(TierId::MobileLow));
        assert_eq!(quality.active(), ActiveTier { id: TierId::MobileLow, source: TierSource::Setting });
        assert_eq!(quality.settings().ceiling, low);
        assert_eq!(quality.render_settings(), quality.render_settings_of(TierId::MobileLow));
        assert_eq!(quality.wanted_tier(), None, "the player's Mobile-High isn't asked for again");
        assert_eq!(quality.fall_back(), None, "Mobile-Low is the class's lowest");
        assert_eq!(quality.active().id, TierId::MobileLow);
        // The player's next settings keep the ceiling, whatever they say of it.
        quality.set_settings(QualitySettings { ceiling: None, ..held });
        assert_eq!((quality.settings().ceiling, quality.wanted_tier()), (low, None));
        TIER.set(TierChoice::Tier(TierId::MobileHigh)).unwrap();
        assert_eq!(quality.update_tier(), None, "r.tier gets the ceiling too");
        TIER.reset();
        quality.update_tier();
        // Automatic and calm: the ladder never climbs back to Mobile-High.
        quality.set_settings(QualitySettings { tier: None, ..quality.settings() });
        let calm = ThermalReading { state: ThermalState::Nominal, ..ThermalReading::default() };
        let mut now = Duration::ZERO;
        for _ in 0..3000 {
            now += Duration::from_millis(100);
            quality.follow_heat(now, &calm);
            assert_eq!(quality.wanted_tier(), None, "at {now:?}");
        }

        // A boot told of the ceiling before it starts (a device lost building Mobile-High) never makes it.
        let told = QualitySettings { ceiling: low, ..held };
        let quality = Quality::new(TierTable::shipped(), &GpuTable::shipped(), &phone, told);
        assert_eq!(quality.active(), ActiveTier { id: TierId::MobileLow, source: TierSource::Setting });
        assert_eq!(quality.wanted_tier(), None);
    }

    /// M4 T3: a tier the ladder, the player or `r.tier` asks for is only wanted until the game changes it, at a
    /// pause or a wipe; a chosen one holds whatever the heat; automatic goes back to the device's.
    fn tier_changes_wait_for_the_game() {
        use sapphire_core::thermal::ThermalState;
        let device = device(FormFactor::Desktop, GpuKind::Discrete, GpuBackend::Vulkan, 32);
        let mut quality =
            Quality::new(TierTable::shipped(), &GpuTable::shipped(), &device, QualitySettings::default());
        assert_eq!(quality.wanted_tier(), None);
        assert_eq!(quality.change_tier(Duration::ZERO), None, "nothing to change");
        // Critical heat walks the ladder down to its tier rung: wanted, not taken.
        let critical = ThermalReading { state: ThermalState::Critical, ..ThermalReading::default() };
        let mut now = Duration::ZERO;
        while quality.tier_request().is_none() {
            now += Duration::from_millis(100);
            assert!(now < Duration::from_secs(600), "the ladder never asked for a tier");
            quality.follow_heat(now, &critical);
        }
        assert_eq!(quality.wanted_tier(), Some(TierId::DesktopLow));
        assert_eq!(quality.active().id, TierId::DesktopHigh, "a fight's frames never change it");
        let low = quality.render_settings_of(TierId::DesktopLow);
        let previous = quality.change_tier(now).expect("the ladder's tier");
        assert_eq!(previous, ActiveTier { id: TierId::DesktopHigh, source: TierSource::Device });
        assert_eq!(quality.active(), ActiveTier { id: TierId::DesktopLow, source: TierSource::Ladder });
        assert_eq!(quality.render_settings(), low, "what the renderer was built with");
        assert_eq!(quality.wanted_tier(), None);
        // A renderer that can't be taken goes back, wanting it again.
        quality.restore(previous);
        assert_eq!(quality.active().id, TierId::DesktopHigh);
        assert_eq!(quality.wanted_tier(), Some(TierId::DesktopLow));
        quality.change_tier(now).expect("again");

        // The player chooses Desktop-High: wanted over the heat, and held there.
        quality.set_settings(QualitySettings { tier: Some(TierId::DesktopHigh), ..quality.settings() });
        assert_eq!(quality.wanted_tier(), Some(TierId::DesktopHigh));
        quality.change_tier(now).expect("the player's tier");
        assert_eq!(quality.active(), ActiveTier { id: TierId::DesktopHigh, source: TierSource::Setting });
        for _ in 0..6000 {
            now += Duration::from_millis(100);
            quality.follow_heat(now, &critical);
        }
        assert_eq!(quality.wanted_tier(), None, "a chosen tier's ladder steps down within it");
        assert!(quality.ladder().state().depth > 0);

        // Automatic again: the device's tier, then the ladder's as the heat asks.
        quality.set_settings(QualitySettings { tier: None, ..quality.settings() });
        assert_eq!(quality.wanted_tier(), Some(TierId::DesktopLow), "the ladder still asks");
        quality.change_tier(now).expect("the ladder's tier");
        assert_eq!(quality.active().source, TierSource::Ladder);
        quality.set_settings(QualitySettings { tier: Some(TierId::DesktopLow), ..quality.settings() });
        assert_eq!(quality.wanted_tier(), None, "chosen and active already");
        quality.set_settings(QualitySettings { dynamic_resolution: true, ..quality.settings() });
        assert!(quality.wants_gpu_timing(), "dynamic resolution follows at once");
    }
}
