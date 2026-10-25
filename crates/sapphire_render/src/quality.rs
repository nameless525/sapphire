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
//!   height (the spec's "~1280×590 → native" on any phone), and the floor dynamic resolution may
//!   lower it to. [`Quality::render_scale`] turns it into [`Renderer::set_render_scale`](crate::Renderer::set_render_scale) every
//!   frame.
//! - **MSAA, bloom levels, the light path and the light budget**: [`QualityTier::render_settings`],
//!   which a renderer is created with.
//! - **Frame rate and GPU frame budget** (§3.1): dynamic resolution's targets.
//!
//! Plain data until their packages read them: temporal AA (R7), shadow cascades, their resolution
//! and the per-character shadow (R4, C2), capsule AO and GTAO (W5), volumetric fog (after W3's
//! distance and height fog, which every tier has), screen-space environment outlines (C4's
//! desktop option), the particle cap (`sapphire_vfx`, M2) and the LOD bias (LOD selection, with
//! culling). The frame rate is also what frame pacing (§6.2, F7) will hold.
//!
//! # Choosing a tier
//!
//! A [`DeviceProfile`] (the device class, the GPU's kind and API from [`GpuCaps`], and the RAM)
//! is matched against each tier's `select_when` rules: a device gets the highest tier of its class
//! whose rules it meets, and each class's lowest tier has none, so every device gets one. Never by
//! OS (rule R7). Three sources override it, in this order:
//!
//! 1. the `r.tier` cvar, unless `auto` (development builds, from the overlay);
//! 2. the player's setting ([`QualitySettings::tier`]; the sandbox's `--tier`);
//! 3. the device's tier.
//!
//! Any tier runs on any device, so mobile tiers can be previewed on a desktop. A renderer is
//! created with its tier's settings; a tier change makes a new one (the app follows
//! [`Quality::update_tier`]), as device loss does.
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
//! - **Thermal input** (§6.3): the platform's thermal state (iOS `thermalState`, Android thermal
//!   headroom) lowers dynamic resolution's budget first, then drops the tier, the ladder's third
//!   step.
//! - **Selection by measurement:** the rules are a first heuristic (spec §3's numbers are all
//!   Proposals). They can't see VRAM, so a GTX 1060 machine with 16 GB of RAM qualifies for
//!   Desktop-High; a device database keyed by adapter, and a startup benchmark, refine them on
//!   the reference devices (M2).
//! - **Applying a tier in place:** a tier change makes a new renderer, which uploads its meshes
//!   again. Once packs and residency (A4) make that costly, settings that don't rebuild pipelines
//!   (bloom levels, the light budget) can change in place.
//! - **The player's settings** come from a settings screen (M3) and saves (`sapphire_save`)
//!   instead of the command line, and can change while running.
//! - **Cheaper timing:** GPU timing costs about 50 µs of CPU and 100 allocations per frame on
//!   Metal (spec §5.12, M1 R6's cost), while dynamic resolution needs a few samples a second.
//!   Timing every few frames, a frame graph option, would cut that to a fraction.
//! - **A tuned controller:** per-tier bands and steps once devices are measured, and an
//!   upscaler's input range (R7) in place of a fixed floor.

pub mod dynamic;

use std::time::Duration;

use bevy_reflect::{Reflect, TypePath};
use sapphire_core::cvar::{AnyCvar, Cvar, CvarKind, CvarType, CvarValue};
use sapphire_core::target::FormFactor;
use sapphire_reflect::Versioned;
use sapphire_rhi::{GpuCaps, wgpu};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::{
    LOG_TARGET, LightPath, MAX_BLOOM_LEVELS, MAX_LIGHTS, MIN_RENDER_SCALE, Msaa, RenderSettings, scaled_size,
};
pub use dynamic::{
    DynamicResolution, DynamicResolutionStats, FrameTiming, ResolutionBudget, ResolutionSignal,
};

/// The shipped table (see the module docs).
const SHIPPED: &str = include_str!("../tiers.ron");

/// The most shadow cascades a tier may ask for (spec §5.12: 4 on desktop, 2 on mobile).
pub const MAX_SHADOW_CASCADES: u32 = 4;

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
    /// Frames per second the tier is built for (§3.1): dynamic resolution's frame-time target
    /// now, and what frame pacing will hold (§6.2).
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
    /// Read by cascaded shadows (R4) and the character shadow (C2).
    pub shadows: Shadows,
    /// Read by ambient occlusion (W5).
    pub ambient_occlusion: AmbientOcclusion,
    /// Volumetric fog, on top of the distance and height fog every tier has (W3; volumetric fog
    /// is desktop-only and later).
    pub volumetric_fog: bool,
    /// The screen-space edge pass for the environment, from the prepass's normals and IDs: a
    /// desktop option beside the characters' inverted-hull outlines (C4).
    pub screen_space_outlines: bool,
    /// Live GPU particles at most (§3.4), for `sapphire_vfx` (M2).
    pub max_particles: u32,
    /// Added to each draw's LOD choice, in LOD levels: positive picks coarser LODs sooner. Read
    /// by LOD selection, which arrives with culling.
    pub lod_bias: f32,
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

/// Edge antialiasing (spec §5.12; plan pillar 6).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct Antialiasing {
    /// Multisampling of the main pass: 4× on every tier, kept in tile memory on phones.
    pub msaa: Msaa,
    /// Temporal AA with velocity and a responsive mask (R7), which decides with it what MSAA
    /// the desktop tiers keep. Off until R7 measures it.
    pub temporal: bool,
}

/// Sun shadows (R4) and the desktop character shadow (C2).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct Shadows {
    /// Cascades, 1..=[`MAX_SHADOW_CASCADES`]: 4 on desktop, 2 on mobile (§3.3).
    pub cascades: u32,
    /// Each cascade's map size in texels, a power of two in 256..=8192.
    pub resolution: u32,
    /// A separate high-resolution shadow frustum per character (desktop).
    pub character_shadow: bool,
}

/// Ambient occlusion (W5).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub struct AmbientOcclusion {
    /// Capsule AO under characters: every tier.
    pub capsules: bool,
    /// GTAO at half resolution: desktop and Mobile-High.
    pub gtao: bool,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceProfile {
    pub class: FormFactor,
    pub gpu: GpuKind,
    pub backend: GpuBackend,
    /// Physical RAM in MiB (`sapphire_platform::SystemInfo::ram_mib`): shared with the GPU on
    /// Apple silicon and phones.
    pub ram_mib: u32,
    /// The GPU can time passes (`GpuCaps::timestamp_query`), so dynamic resolution follows GPU
    /// time; without it, the frame time.
    pub timestamps: bool,
}

impl DeviceProfile {
    /// A device of `class` with `caps`' GPU and `ram_mib` of RAM.
    pub fn new(class: FormFactor, caps: &GpuCaps, ram_mib: u32) -> Self {
        Self {
            class,
            gpu: caps.adapter.device_type.into(),
            backend: caps.backend().into(),
            ram_mib,
            timestamps: caps.timestamp_query,
        }
    }
}

impl QualityTier {
    /// What a renderer for this tier is created with.
    pub fn render_settings(&self) -> RenderSettings {
        RenderSettings {
            msaa: self.antialiasing.msaa,
            bloom_levels: self.bloom_levels,
            light_path: self.light_path,
            max_lights: self.max_lights,
        }
    }

    /// The budgets dynamic resolution holds this tier's frames to.
    pub fn resolution_budget(&self) -> ResolutionBudget {
        ResolutionBudget {
            gpu_frame: Duration::from_secs_f32(self.gpu_frame_ms / 1000.0),
            frame: Duration::from_secs_f64(1.0 / f64::from(self.frame_rate)),
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
        if !(1..=MAX_SHADOW_CASCADES).contains(&self.shadows.cascades) {
            return invalid("shadows.cascades", "must be within 1..=4");
        }
        let texels = self.shadows.resolution;
        if !(texels.is_power_of_two() && (256..=8192).contains(&texels)) {
            return invalid("shadows.resolution", "must be a power of two in 256..=8192");
        }
        if !(-4.0..=4.0).contains(&self.lod_bias) {
            return invalid("lod_bias", "must be within -4..=4");
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

    /// The highest tier of `device`'s class that it qualifies for.
    pub fn select(&self, device: &DeviceProfile) -> TierId {
        self.tiers
            .iter()
            .filter(|tier| tier.id.class() == device.class && tier.qualifies(device))
            .map(|tier| tier.id)
            .max()
            .unwrap_or(TierId::lowest(device.class))
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

/// This module's cvars, for the app's registry.
pub static CVARS: [&dyn AnyCvar; 3] = [&TIER, &RENDER_SCALE, &DYNAMIC_RESOLUTION];

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
/// active tier and its dynamic resolution. Per frame it reads three cvars and does a few dozen
/// floating-point operations, and allocates nothing.
#[derive(Debug)]
pub struct Quality {
    table: TierTable,
    device: TierId,
    settings: QualitySettings,
    active: ActiveTier,
    /// `r.tier` when last read, so each change applies once.
    seen: TierChoice,
    /// The device can time GPU passes, for each tier's dynamic resolution.
    timestamps: bool,
    dynamic: DynamicResolution,
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
}

impl Quality {
    /// Selects `device`'s tier in `table` and applies the overrides.
    pub fn new(table: TierTable, device: &DeviceProfile, settings: QualitySettings) -> Self {
        let device_tier = table.select(device);
        let seen = TIER.get();
        let active = choose_tier(seen, settings.tier, device_tier);
        let dynamic = DynamicResolution::new(table.get(active.id).resolution_budget(), device.timestamps);
        tracing::info!(
            target: LOG_TARGET,
            tier = active.id.name(),
            source = ?active.source,
            device_tier = device_tier.name(),
            ?device,
            "quality tier"
        );
        Self {
            table,
            device: device_tier,
            settings,
            active,
            seen,
            timestamps: device.timestamps,
            dynamic,
            base_scale: 1.0,
            render_scale: 1.0,
        }
    }

    /// The active tier's knobs.
    pub fn tier(&self) -> &QualityTier {
        self.table.get(self.active.id)
    }

    pub fn active(&self) -> ActiveTier {
        self.active
    }

    pub fn table(&self) -> &TierTable {
        &self.table
    }

    /// What a renderer for the active tier is created with.
    pub fn render_settings(&self) -> RenderSettings {
        self.tier().render_settings()
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
        let previous = std::mem::replace(&mut self.active, next);
        if previous.id == next.id {
            return None;
        }
        self.dynamic = DynamicResolution::new(self.tier().resolution_budget(), self.timestamps);
        tracing::info!(target: LOG_TARGET, tier = next.id.name(), from = previous.id.name(), "quality tier changed");
        Some(previous)
    }

    /// Makes `previous` (from [`Self::update_tier`]) active again.
    pub fn restore(&mut self, previous: ActiveTier) {
        self.active = previous;
        self.dynamic = DynamicResolution::new(self.tier().resolution_budget(), self.timestamps);
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
        let (width, height) = scaled_size(size, base);
        let factor = self.dynamic.update(timing, width as f32 * height as f32);
        self.base_scale = base;
        self.render_scale = (base * factor).clamp(MIN_RENDER_SCALE, 1.0);
        self.render_scale
    }

    pub fn stats(&self) -> QualityStats {
        QualityStats {
            tier: self.active,
            device_tier: self.device,
            base_scale: self.base_scale,
            render_scale: self.render_scale,
            dynamic: self.dynamic.stats(),
        }
    }
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(class: FormFactor, gpu: GpuKind, backend: GpuBackend, ram_gib: u32) -> DeviceProfile {
        DeviceProfile { class, gpu, backend, ram_mib: ram_gib * 1024, timestamps: true }
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
    fn desktop_tiers_draw_as_the_goldens_do() {
        // Golden images render with the default settings at 640×360 on whichever desktop tier
        // their machine selects, so both must give exactly those, at render scale 1.
        let table = TierTable::shipped();
        for id in [TierId::DesktopHigh, TierId::DesktopLow] {
            let tier = table.get(id);
            assert_eq!(tier.render_settings(), RenderSettings::default(), "{id}");
            assert_eq!(tier.resolution.base_scale(360), 1.0, "{id}");
            assert_eq!(tier.resolution.base_scale(1080), 1.0, "{id}: the bench's size");
        }
        let mobile = table.get(TierId::MobileLow).render_settings();
        assert_eq!(
            (mobile.light_path, mobile.max_lights),
            (LightPath::PerObject, 8),
            "spec §3.4's mobile budget"
        );
    }

    #[test]
    fn base_scales_hold_the_internal_height() {
        let table = TierTable::shipped();
        // Mobile-Low's ~590 rows on an iPhone 12 in landscape (2532×1170).
        let mobile_low = table.get(TierId::MobileLow).resolution.base_scale(1170);
        assert!((mobile_low * 1170.0 - 590.0).abs() < 1.0, "{mobile_low}");
        // Desktop tiers render at native height until R7's upscaler (tiers.ron explains why).
        let desktop_high = &table.get(TierId::DesktopHigh).resolution;
        assert_eq!(desktop_high.base_scale(2160), 1.0);
        assert_eq!(table.get(TierId::DesktopLow).resolution.base_scale(2160), 1.0);
        let tiny = Resolution { scale: 1.0, max_height: Some(10), dynamic_floor: 1.0 };
        assert_eq!(tiny.base_scale(1000), MIN_RENDER_SCALE, "never below the minimum");
    }

    /// A change to one tier, and the field it makes invalid.
    type BadValue = (TierId, &'static str, fn(&mut QualityTier));

    #[test]
    fn bad_values_are_rejected() {
        use TierId::*;
        let cases: [BadValue; 13] = [
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
            (DesktopLow, "lod_bias", |tier| tier.lod_bias = f32::INFINITY),
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
        let pick = |profile: DeviceProfile| table.select(&profile);
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
        Quality::new(TierTable::shipped(), &profile, settings)
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
        });
        assert_eq!(player.active(), ActiveTier { id: TierId::MobileLow, source: TierSource::Setting });
        assert_eq!(player.render_scale((1920, 1080), &timing), 0.7);
        assert!(!player.wants_gpu_timing());

        let mut quality = quality(QualitySettings::default());
        assert_eq!(quality.active().id, TierId::DesktopHigh);
        assert_eq!(quality.render_scale((1920, 1080), &timing), 1.0);
        assert_eq!(quality.render_scale((3840, 2160), &timing), 1.0, "native until R7's upscaler");
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
    }
}
