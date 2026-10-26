//! Ambient light and ambient occlusion (M1 W5; spec §5.12 "Lighting", look pillars 1 and 4): an
//! ambient probe grid baked by the cooker and relit as the sky and time of day change, the sky
//! cube prefiltered for reflections, capsule AO from the characters' bodies, and GTAO at half
//! resolution.
//!
//! # The probe grid
//!
//! A zone's grid ([`sapphire_assets::probes`], baked by the cooker's `probes` module) holds, per
//! probe of a sparse brick grid, the sky's visibility as SH and the bounce light of every
//! time-of-day key the bake lit, as L1 per channel. [`Renderer::set_probe_grid`]
//! (crate::Renderer::set_probe_grid) uploads it whole (a hot reload replaces it the same way):
//!
//! - **Its data** in one storage buffer of words: the kept bricks' probe indices, then per probe
//!   its data bits, its visibility and its bounce, the coefficients left as the half floats they
//!   were cooked as and decoded in the shader. One buffer keeps the relight within the
//!   4-storage-buffers-per-stage floor (see the crate docs, "Binding budget").
//! - **The brick table** as a 3D `R32Uint` texture, one texel per brick position.
//! - **The relit atlas**, a 3D `Rgba16Float` texture: each kept brick as 5³ texels (its 4³ cells
//!   with the probes it shares with its neighbors, so trilinear filtering never crosses into
//!   another brick), rows of up to 64 bricks, and five sub-volumes stacked along z: red's,
//!   green's and blue's L1 irradiance, the sky visibility's L1, and the probe data.
//!
//! Each frame with a grid or GTAO, one compute pass runs before the main pass ([`PASSES`]: the
//! frame graph opens one compute pass for all of it, after the sky cube's face and the scene
//! prepass), each of its dispatches only when due (`probes.wgsl`, `occlusion.wgsl`):
//!
//! 1. **The sky's projection:** the sky cube projected to L2 SH, scaled per channel so a surface
//!    facing straight up under the open sky receives time of day's authored ambient sky (in the
//!    L1 irradiance the grid holds), as it does from the ambient hemisphere: the grid changes
//!    where ambient light comes from and what blocks it, never how strong time of day authored
//!    it. Then the transfer from a probe's visibility to its irradiance, exact for L2 visibility
//!    on the icosahedron's vertices ([`sapphire_math::sh::SkyTransfer`]). The cube holds the
//!    glows and clouds, so the grid's sky term follows them; the cooker's bake, which has no
//!    cube, lights with [`sky_radiance`], the same projection of the gradient alone. A face at a
//!    time: the sky light keeps each face's sums, and a frame projects only the faces the sky
//!    cube drew (one a frame on desktop, one every other frame on Mobile-Low; all six when a grid
//!    arrives) and sums the six. One workgroup does it, so its cost is latency, which a sixth of
//!    the samples cuts by about as much.
//! 2. **The prefilter** of the same faces (below, "Reflections").
//! 3. **The relight:** atlas texels relit as sky visibility × the sky plus the bounce of the keys
//!    the frame blends ([`FramePacket::probe_keys`]: time of day resolves keys and weights from
//!    the hour and the grid's key hours, rule R4; the renderer never sees an hour), one thread per
//!    texel, about 10,000 for the courtyard. Amortized (`RelightSchedule`): every brick the frame
//!    a grid arrives, or the keys' weights or the authored sky jump (5% from one frame to the
//!    next, when the projection also runs at once for the sky's); after a smaller change (the
//!    projection of a face, the keys drifting with the hour) a quarter of the bricks a frame in
//!    turn, until every quarter has relit since the last change; nothing while nothing changes.
//!    A brick's light is then at most three frames older than its neighbors', while the sky cube
//!    itself takes six (or twelve) frames to catch up with the sky.
//! 4. **The characters** (below).
//! 5. **GTAO** (below, "Ambient occlusion").
//!
//! The pass's uniforms are one buffer, staged with one write in the frames they change
//! (`AmbientBlock`: the ambient uniforms, the relight's, GTAO's and the prefilter's per mip, each
//! at a 256-byte boundary), and its bind groups are made once: when a grid arrives, and for
//! GTAO's per view of the frame graph's pooled prepass textures, the last two seen (post's
//! caching). Its pipelines are built only for work that can run (below, "Cost").
//!
//! The environment shader then reads the grid (`ambient.wgsl`'s `grid_lookup`) behind a pipeline
//! constant set while a grid is uploaded, so content without a bake draws exactly as before: the
//! frame's hemisphere, the real path and the pin for the scenes from before W5. A lookup moves
//! off the surface along its normal and toward the camera, then weighs the cell's eight probes
//! by trilinear distance times three leak tests: whether each probe sees the point along each
//! axis (a probe whose free distance toward the point is shorter than the way to it has a wall
//! between them, and drops out), whether the surface faces it (DDGI's soft backface weight), and
//! whether it lies in open space. The weights' centroid within the cell is sampled through the
//! hardware filter: exact when a wall drops a whole face of the cell, and three fetches per
//! channel instead of eight. The relight flags each open cell (all eight corners in open space,
//! each seeing the others a full spacing away), whose lookup passes every test and so reads one
//! corner's data instead of all eight: most of the air above open ground. The grid's irradiance
//! replaces the hemisphere's by the trilinear share of the cell's corners inside the kept bricks
//! (the renderer marks the probes on the edge of an empty brick or of the grid at upload,
//! `PROBE_EDGE_BIT`), so it fades into the hemisphere over the last cell before an empty brick
//! or the grid's edge, and outside them the hemisphere stays.
//!
//! Where it reads the grid is a tier setting ([`ProbeLookup`](crate::ProbeLookup), `tiers.ron`):
//! per pixel on desktop and Mobile-High; per vertex on Mobile-Low, whose per-pixel lookup cost
//! 38% of its main pass (spec §5.12 "Lighting"): the vertex shader reads the grid along the
//! vertex's normal and the triangle interpolates its irradiance and weight. The mobile cook's
//! grid is 3 m between probes, no finer than the world's vertices, so little is lost; the ambient
//! specular keeps the hemisphere there, which Mobile-Low doesn't draw.
//!
//! # Characters
//!
//! C2's stylized GI stays stylized: characters keep shading from a hemisphere (sky above, ground
//! below), flattened toward the camera and clamped in HSV, and the grid only decides what that
//! hemisphere is. Each character's entry ([`GpuCharacterAmbient`](crate::characters), WGSL
//! `CharacterAmbient`) holds its sky and ground colors and the axis between them; the CPU fills
//! them with the frame's own (exactly what `ambient_light` reads), and where a grid exists the
//! `character ambient` pass replaces them with the grid's L1 at the body's center and its head,
//! as a hemisphere along the luminance gradient. The fragment stage has no storage buffer to
//! spare, and per-pixel grid lookups would bring the grid's cell structure onto faces, so the
//! character entry, already bound, carries it: one lookup pair per character per frame, and the
//! low frequency the anime look wants. Characters take capsule AO from the other characters'
//! bodies on the CPU, as one factor on their ambient (`axis.w`), and no GTAO: screen-space
//! occlusion would mark faces and hair with the noise and creases the look keeps off them.
//!
//! # Reflections
//!
//! With a grid and environment specular on, the sky cube's faces drawn each frame are prefiltered
//! into a reflection cube's roughness mips (`cs_prefilter`; [`ReflectionSettings`] per tier): mip
//! 0 is the sky at the ambient's scale, and each rougher mip filters the one before it with GGX's
//! lobe for the difference of their roughnesses, 32 importance samples a texel. The environment's
//! ambient specular reads the mip for its roughness where the grid's sky visibility along the
//! reflection vector sees the sky, and the grid's irradiance from that direction where geometry
//! blocks it. The cube and its prefilter are made with the first grid, since nothing reads them
//! without one. C7's reflection add builds on the same cube; reflection probes come later (see
//! "Later milestones").
//!
//! # Ambient occlusion
//!
//! Per tier ([`AmbientOcclusion`], `tiers.ron`; the sandbox's `--ao off|capsule|full`), on the
//! environment's ambient light alone (its diffuse through Jimenez's multi-bounce fit, its
//! specular through Lagarde's specular occlusion), behind pipeline constants too:
//!
//! - **Capsule AO**, every tier: the nearest [`MAX_CAPSULES`] character bodies, each a sphere at
//!   the point of its axis nearest the shaded point taking its cosine-weighted solid angle
//!   (Quílez's sphere occlusion), faded out between three and five radii. It costs a short loop
//!   per pixel and no pass.
//! - **GTAO** ([`GtaoSettings`]), desktop and Mobile-High (`occlusion.wgsl`): half resolution from
//!   the scene prepass's depth and normals, which it makes run. The depth is first reduced to
//!   view distance and [`GTAO_DEPTH_MIPS`] − 1 coarser mips (XeGTAO's prefilter), which samples
//!   read by how far they reach, so a wide search stays in the texture cache (it cost the M5 about
//!   three times as much at 1080p without them); then the search, a 5 × 5 depth-aware blur, and
//!   an upsample by depth in the environment shader. Its radius (1 m on desktop) stays under the
//!   grid's spacing: GTAO is contact occlusion, and the grid's sky visibility carries occlusion at
//!   larger scales, so the two layer rather than count the same creases twice. Its 4 × 4
//!   pattern moves every frame under temporal AA, whose resolve averages it, and holds still
//!   without.
//! - **Characters are capsule AO's alone:** GTAO leaves out every pixel where a character (the
//!   character prepass's depth, M1 C5) or an outline hull (the scene prepass's outline mask, M1
//!   C4) is the nearest surface, as it leaves out the sky, so neither occludes the world around
//!   it; their bodies' capsules darken it instead, on every tier alike, and a character's
//!   contact with the ground isn't counted twice. Characters don't read GTAO themselves.
//! - **What the prepass leaves to GTAO:** transparent surfaces write no prepass depth, so GTAO
//!   measures what lies behind them and they take capsule AO alone; alpha-tested surfaces
//!   occlude as the prepass draws them, so GTAO relies on it cutting their holes out. Foliage's
//!   cards are cut out there as the main pass cuts them (M1 W6), so a card occludes with its
//!   leaves, never its quad; other families' alpha-tested surfaces are still drawn whole there
//!   (the crate docs' later milestones).
//! - **Foliage** (M1 W6, `foliage.wgsl`) takes all of it as the environment does: the grid's
//!   irradiance along its volume normals (per vertex on Mobile-Low), then capsule AO and GTAO
//!   through the multi-bounce fit, under its own canopy occlusion, which stays as it was. Its
//!   surface pipelines take the same constants, so a grid arriving builds them anew too.
//!
//! # Cost
//!
//! Spec §3.3's "Light clustering, AO" row (1.5 ms on Desktop-Low; no figure for Mobile-Low) holds
//! the light-cluster pass and [`PASSES`]. Measured on the M5 at 1080p in the courtyard with its
//! grid (spec §5.12 "Lighting" has the numbers).
//!
//! **Nothing for work nothing reads.** A renderer without a grid or GTAO records no ambient pass,
//! builds none of its pipelines (nor their module), makes no reflection cube, and stages its
//! uniforms only when they change, which is once: content and tiers without them pay nothing they
//! can measure. Pipelines are built when their work can first run ([`AmbientPipelines`]): GTAO's
//! three with a renderer whose tier has it, the grid's light (the projection, the relight and the
//! characters', and the prefilter with the reflection cube) with the first grid, a load-time cost,
//! and kept after it. Built with every renderer, the seven had cost each process about 0.75 s on
//! the M5 with Metal's shader cache cold (10 ms warm), and far more on CI's software backends,
//! which compile them again for every golden scene: WARP's scenes took 60–75 s instead of 9.
//!
//! # Later milestones
//!
//! - **Reflection probes** (early M2): baked local cubes blended by volume, which the sky cube's
//!   prefiltered mips stand in for today.
//! - **Probe visualization** (F5 part 2): spheres at each probe showing its irradiance, beside the
//!   overlay's grid readout.
//! - **Several grids and streaming**: one grid per zone today, uploaded whole; open worlds stream
//!   bricks by distance into a fixed atlas, which the brick table already indirects.
//! - **Dynamic objects and local lights in the bake**, and GTAO's bent normals for the grid's
//!   lookup direction.
//! - **Cheaper lookups:** Mobile-Low reads the grid per vertex; on the other tiers the per-pixel
//!   lookup is still most of the grid's cost (about a quarter of Desktop-High's main pass at
//!   1080p in the arcade): distant geometry could read it per vertex there too, and a brick's
//!   light could be stored compressed, in fewer sub-volumes to filter.

use std::num::NonZeroU64;
use std::ops::Range;

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use sapphire_assets::probes::{
    ArchivedProbeGrid, BOUNCE_COEFFICIENTS, BRICK_CELLS, BRICK_PROBES, BRICK_SIDE, EMPTY_BRICK,
};
use sapphire_math::sh;
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

use crate::buffer;
use crate::characters::GpuCharacter;
use crate::environment::ProbeLookup;
use crate::gpu_types::FrameUniforms;
use crate::graph::{FrameGraph, TextureId};
use crate::pipelines::Layouts;
use crate::shaders::{self, Program, ShaderSources};
use crate::{AmbientLight, CharacterBody, FramePacket, LOG_TARGET, RenderError, SkyGradient};

/// Ambient occlusion on a quality tier (`tiers.ron`, [`RenderSettings::ambient_occlusion`]
/// (crate::RenderSettings::ambient_occlusion)): capsule AO from characters' bodies, and GTAO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(deny_unknown_fields)]
pub struct AmbientOcclusion {
    /// Capsule AO: every character's body darkens the ambient light of the environment and of
    /// the other characters around it. Every tier.
    pub capsules: bool,
    /// GTAO at half resolution, on the environment's ambient light: desktop and Mobile-High.
    pub gtao: Option<GtaoSettings>,
}

impl AmbientOcclusion {
    /// No ambient occlusion: what the golden scenes from before M1 W5 pin (`--ao off`).
    pub const OFF: Self = Self { capsules: false, gtao: None };

    /// Checks GTAO's settings, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        match &self.gtao {
            Some(gtao) => gtao.validate(),
            None => Ok(()),
        }
    }
}

impl Default for AmbientOcclusion {
    fn default() -> Self {
        Self::OFF
    }
}

/// GTAO's cost on a tier: how many directions each pixel searches and how far.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(deny_unknown_fields)]
pub struct GtaoSettings {
    /// Slices (directions in screen space) per pixel, 1..=4. A 4 × 4 pattern turns them from
    /// pixel to pixel, and the blur averages the turns.
    pub slices: u32,
    /// Depth samples along each side of a slice, 1..=8.
    pub steps: u32,
    /// How far, in meters, a surface occludes, above 0 and at most 8: contact scale, below the
    /// probe grid's spacing, which carries larger-scale occlusion as sky visibility.
    pub radius: f32,
}

/// Settings are compared as a whole, and a valid radius is finite, so equality is total.
impl Eq for GtaoSettings {}

impl GtaoSettings {
    /// The desktop tiers': two slices of four steps, out to 1 m.
    pub const DESKTOP: Self = Self { slices: 2, steps: 4, radius: 1.0 };

    /// Checks the counts and the radius, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if !(1..=4).contains(&self.slices) {
            return Err(("ambient_occlusion.gtao.slices", "must be 1 to 4"));
        }
        if !(1..=8).contains(&self.steps) {
            return Err(("ambient_occlusion.gtao.steps", "must be 1 to 8"));
        }
        if !(self.radius.is_finite() && self.radius > 0.0 && self.radius <= 8.0) {
            return Err(("ambient_occlusion.gtao.radius", "must be above 0 and at most 8 meters"));
        }
        Ok(())
    }
}

/// The reflection source on a tier (`tiers.ron`, [`RenderSettings::reflections`]
/// (crate::RenderSettings::reflections)): the sky cube prefiltered into roughness mips, which
/// the environment's ambient specular reads where a probe grid lights it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(deny_unknown_fields)]
pub struct ReflectionSettings {
    /// Texels along each edge of the prefiltered cube's sharpest mip: a power of two in
    /// 16..=512. Mips halve down to 8 texels, the roughest at roughness 1.
    pub cube_size: u32,
}

impl Default for ReflectionSettings {
    /// The desktop tiers': half the sky cube's size, which mip 0's bilinear lookups then box
    /// filter.
    fn default() -> Self {
        Self { cube_size: 128 }
    }
}

impl ReflectionSettings {
    /// Checks the size, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if !(self.cube_size.is_power_of_two() && (16..=512).contains(&self.cube_size)) {
            return Err(("reflections.cube_size", "must be a power of two from 16 to 512"));
        }
        Ok(())
    }

    /// The cube's mips: 128 texels has 5, the last 8 texels a side.
    pub fn mips(&self) -> u32 {
        (self.cube_size.max(REFLECTION_MIN_SIZE) / REFLECTION_MIN_SIZE).ilog2() + 1
    }
}

/// Most time-of-day keys a frame blends the probe grid's bounce from: the two its hour lies
/// between, and two more while a cutscene crossfades to its own hour.
pub const MAX_PROBE_KEYS: usize = 4;

/// Which of the probe grid's baked keys this frame's bounce light blends, and by how much
/// ([`FramePacket::probe_keys`]): resolved by time of day from the frame's hour and the hours
/// the grid was baked at (rule R4). Weights of 0 are unused entries; the default has none, so a
/// grid lends sky light alone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeKeys {
    /// Indices into the grid's keys.
    pub keys: [u32; MAX_PROBE_KEYS],
    pub weights: [f32; MAX_PROBE_KEYS],
}

impl ProbeKeys {
    /// Key `key` alone.
    pub fn single(key: u32) -> Self {
        let mut keys = Self::default();
        keys.keys[0] = key;
        keys.weights[0] = 1.0;
        keys
    }

    /// Whether every weight is finite and at least 0.
    pub fn is_valid(&self) -> bool {
        self.weights.iter().all(|weight| weight.is_finite() && *weight >= 0.0)
    }
}

/// The sky gradient's radiance in direction `d` (unit): `sky_color()` in `frame.wgsl`.
pub fn sky_gradient(sky: &SkyGradient, d: Vec3) -> Vec3 {
    let y = d.y;
    if y >= 0.0 {
        sky.horizon.lerp(sky.zenith, y.clamp(0.0, 1.0).sqrt())
    } else {
        sky.horizon.lerp(sky.ground, (-y * 5.0).clamp(0.0, 1.0))
    }
}

/// The sky light the ambient probe grid relights with, as L2 SH per color channel: the sky
/// gradient's shape, scaled per channel so a surface facing straight up under the open sky
/// receives exactly time of day's authored ambient sky (`ambient.sky × ambient.intensity`), as
/// it does from the ambient hemisphere, in the L1 irradiance the grid holds and the bake lights
/// with (`sh::SkyTransfer`, which leaves out the sky's L2 band). The grid then changes where
/// ambient light comes from and what blocks it, never how strong time of day authored it.
///
/// The renderer projects the sky cube the same way each frame on the GPU (its clouds and glows
/// included); the cooker's bake, which has no sky cube, lights surfaces with this.
pub fn sky_radiance(sky: &SkyGradient, ambient: &AmbientLight) -> [[f32; sh::L2]; 3] {
    // The gradient depends on height alone, so its projection reduces to an integral over
    // y = cos θ (the azimuth integrates out): only Y₀₀, Y₁₋₁ (∝ y), Y₂₀ and Y₂₂ are nonzero, and
    // over a full turn of azimuth both of the latter integrate to π(1 − 3y²) times their constant.
    const STEPS: u32 = 512;
    // The constants of Y₀₀, Y₁₋₁ = c₁y, Y₂₀ = c₂₀(3z² − 1) and Y₂₂ = c₂₂(x² − y²), read off the
    // basis straight up.
    let up = sh::basis(Vec3::Y);
    let (c0, c1, c20, c22) = (up[0], up[1], -up[6], -up[8]);
    let mut channels = [[0.0f32; sh::L2]; 3];
    let dy = 2.0 / STEPS as f32;
    for step in 0..STEPS {
        let y = -1.0 + (step as f32 + 0.5) * dy;
        let radiance = sky_gradient(sky, Vec3::new((1.0 - y * y).sqrt(), y, 0.0));
        let around = std::f32::consts::TAU * dy;
        let zonal2 = std::f32::consts::PI * (1.0 - 3.0 * y * y) * dy;
        for (channel, value) in channels.iter_mut().zip(radiance.to_array()) {
            channel[0] += value * around * c0;
            channel[1] += value * around * c1 * y;
            channel[6] += value * zonal2 * c20;
            channel[8] += value * zonal2 * c22;
        }
    }
    let target = ambient.sky * ambient.intensity;
    for (channel, target) in channels.iter_mut().zip(target.to_array()) {
        let open = sh::evaluate(&sh::irradiance(*channel)[..sh::L1], Vec3::Y);
        let scale = if open > 1e-6 { target.max(0.0) / open } else { 0.0 };
        for value in channel.iter_mut() {
            *value *= scale;
        }
    }
    channels
}

/// The most character bodies the environment's capsule AO reads in a frame, nearest the camera
/// first (WGSL `MAX_AMBIENT_CAPSULES`).
pub const MAX_CAPSULES: usize = 16;

/// The ambient pass's name in the frame graph, for GPU timing: one compute pass for every
/// ambient dispatch of a frame (with a grid, the sky's projection, the reflection prefilter, the
/// relight and the characters' ambient; with GTAO, its depth prefilter, search and blur).
pub const PASSES: [&str; 1] = [AMBIENT_PASS];
const AMBIENT_PASS: &str = "ambient";

/// Threads per workgroup of the projection, the relight and the character pass (WGSL
/// `AMBIENT_WORKGROUP`).
pub(crate) const WORKGROUP: u32 = 64;
/// The prefilter's square workgroups (WGSL `PREFILTER_WORKGROUP`).
pub(crate) const PREFILTER_WORKGROUP: u32 = 8;
/// GTAO's square workgroups (WGSL `GTAO_WORKGROUP`). The depth prefilter's workgroup also
/// reduces its 8 × 8 texels to one texel of the last mip: `8 = 2^(GTAO_DEPTH_MIPS − 1)`.
pub(crate) const GTAO_WORKGROUP: u32 = 8;
/// Mips of GTAO's half-resolution view distance (WGSL `GTAO_DEPTH_MIPS`).
pub(crate) const GTAO_DEPTH_MIPS: u32 = 4;
/// GTAO's view distance: one float, writable as a storage texture on every WebGPU device.
const GTAO_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// GGX samples per texel of each rougher reflection mip: each filters the mip before it, already
/// blurred, so few are needed.
const PREFILTER_SAMPLES: u32 = 32;
/// The reflection cube's smallest mip, texels a side: roughness 1.
const REFLECTION_MIN_SIZE: u32 = 8;
/// Most reflection mips (512 texels down to 8).
const MAX_REFLECTION_MIPS: usize = 7;
/// Bytes between the regions of the ambient uniform buffer ([`AmbientBlock`]): the largest
/// uniform offset alignment WebGPU allows a device to need.
const REGION_ALIGNMENT: u64 = 256;
/// Where each struct of [`AmbientBlock`] starts in the uniform buffer.
const AMBIENT_REGION: u64 = std::mem::offset_of!(AmbientBlock, ambient) as u64;
const RELIGHT_REGION: u64 = std::mem::offset_of!(AmbientBlock, relight) as u64;
const GTAO_REGION: u64 = std::mem::offset_of!(AmbientBlock, gtao) as u64;
const PREFILTER_REGION: u64 = std::mem::offset_of!(AmbientBlock, prefilter) as u64;
/// The slices the relight spreads its bricks over after a small change: each brick relights
/// within this many frames of the last change.
const RELIGHT_SLICES: u32 = 4;
/// The change from one frame to the next past which the relight treats the light as jumping and
/// relights every brick at once: the summed change of the keys' weights, and the authored ambient
/// sky's largest change in a channel, relative to its larger value.
const KEY_JUMP: f32 = 0.05;
const SKY_JUMP: f32 = 0.05;
/// Bricks along a row and a column of the atlas.
const ATLAS_ROW: u32 = 64;
/// The atlas's sub-volumes along z (WGSL `ATLAS_SUB_VOLUMES`).
pub(crate) const ATLAS_SUB_VOLUMES: u32 = 5;
/// The relit atlas's, the reflection cube's and GTAO's format: filterable and writable as a
/// storage texture on every WebGPU device.
const STORAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// WGSL `SkyLight`: the transfer's 27 columns, 9 radiance coefficients, the scale, and each cube
/// face's 9 coefficients.
pub(crate) const SKY_LIGHT_BYTES: u64 = (27 + 9 + 1 + 6 * 9) * 16;
/// The probe data bit the renderer sets at upload on a probe some brick around it lacks: an
/// empty brick, or one past the grid's extent (WGSL `PROBE_EDGE_BIT`). The lookup fades the grid
/// out toward such probes. Cooked grids leave it clear (`ProbeData::UNUSED_BITS`).
pub(crate) const PROBE_EDGE_BIT: u32 = 1 << 31;

/// The frame group's ambient bindings (`ambient.wgsl`), after the character prepass's 8–10
/// (M1 C5).
pub(crate) mod frame_binding {
    pub const UNIFORMS: u32 = 11;
    pub const ATLAS: u32 = 12;
    pub const BRICKS: u32 = 13;
    pub const REFLECTION: u32 = 14;
    pub const OCCLUSION: u32 = 15;
    pub const SAMPLER: u32 = 16;
}

/// The environment shader's pipeline constants for the ambient terms (`environment.wgsl`).
pub(crate) mod override_constant {
    pub const PROBES: &str = "AMBIENT_PROBES";
    pub const PROBES_PER_VERTEX: &str = "AMBIENT_PROBES_PER_VERTEX";
    pub const CAPSULE_AO: &str = "CAPSULE_AO";
    pub const GTAO: &str = "GTAO";
}

/// The environment pipelines' ambient constants with `occlusion` and with a grid read the way
/// `grid` says, or without one (`None`): only those that are on, so a renderer without any builds
/// the shader as it was before W5.
pub(crate) fn constants(occlusion: &AmbientOcclusion, grid: Option<ProbeLookup>) -> Vec<(&'static str, f64)> {
    [
        (override_constant::PROBES, grid.is_some()),
        (override_constant::PROBES_PER_VERTEX, grid == Some(ProbeLookup::PerVertex)),
        (override_constant::CAPSULE_AO, occlusion.capsules),
        (override_constant::GTAO, occlusion.gtao.is_some()),
    ]
    .into_iter()
    .filter(|&(_, on)| on)
    .map(|(name, _)| (name, 1.0))
    .collect()
}

/// One character body for capsule AO (WGSL `AmbientCapsule`). 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuAmbientCapsule {
    /// xyz: the body's base, world space; w: its radius.
    pub base: [f32; 4],
    /// xyz: its top; w: 0.
    pub top: [f32; 4],
}

/// Group 0 binding 8 (WGSL `AmbientUniforms`): where the grid is, the atlas, GTAO's size and the
/// capsules. 624 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct AmbientUniforms {
    /// xyz: the grid's first probe, world space; w: meters between probes.
    pub grid_origin: [f32; 4],
    /// xyz: cells along each axis (bricks × 4); w: 0.
    pub grid_cells: [f32; 4],
    /// xyz: bricks along each axis; w: 1 with a grid, 0 without.
    pub grid_bricks: [u32; 4],
    /// xyz: the atlas's bricks along each axis; w: texels of one sub-volume along z.
    pub atlas: [u32; 4],
    /// xyz: one atlas texel in texture coordinates; w: the reflection cube's roughest mip.
    pub atlas_texel: [f32; 4],
    /// xy: GTAO's texture size; zw: one texel in texture coordinates.
    pub occlusion_size: [f32; 4],
    /// x: capsules; y: character entries; z: the first sky cube face the projection projects;
    /// w: the faces it projects.
    pub counts: [u32; 4],
    pub capsules: [GpuAmbientCapsule; MAX_CAPSULES],
}

/// The relight's uniforms (WGSL `RelightUniforms`). 96 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct RelightUniforms {
    pub keys: [u32; 4],
    pub weights: [f32; 4],
    /// x: atlas texels; y: visibility coefficients; z: words of visibility per probe; w: keys.
    pub counts: [u32; 4],
    /// Where the data, the visibility and the bounce start in the words buffer.
    pub offsets: [u32; 4],
    pub atlas: [u32; 4],
    /// x: the first atlas texel this frame relights; y: the texel past its last; zw: 0.
    pub slice: [u32; 4],
}

/// One reflection mip's prefilter uniforms (WGSL `PrefilterUniforms`). 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct PrefilterUniforms {
    /// x: texels a side; y: the added lobe's GGX α; z: samples; w: 1 to scale by the sky's.
    pub mip: [f32; 4],
    /// x: the first face filtered this frame.
    pub faces: [u32; 4],
}

/// GTAO's uniforms (WGSL `GtaoUniforms`). 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GtaoUniforms {
    /// xy: the half-resolution textures' size; zw: the scene's.
    pub size: [f32; 4],
    /// x: radius; y: slices; z: steps; w: the frame's noise offset.
    pub params: [f32; 4],
}

/// Every ambient uniform of a frame, in one buffer staged with one write: each struct at a
/// multiple of [`REGION_ALIGNMENT`], where its bindings point. 3,072 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct AmbientBlock {
    ambient: AmbientUniforms,
    _ambient_padding: [[u32; 4]; 9],
    relight: RelightUniforms,
    _relight_padding: [[u32; 4]; 10],
    gtao: GtaoUniforms,
    _gtao_padding: [[u32; 4]; 14],
    /// Per reflection mip.
    prefilter: [PrefilterRegion; MAX_REFLECTION_MIPS],
}

/// One reflection mip's prefilter uniforms in [`AmbientBlock`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct PrefilterRegion {
    uniforms: PrefilterUniforms,
    _padding: [[u32; 4]; 14],
}

const _: () = {
    assert!(
        AMBIENT_REGION.is_multiple_of(REGION_ALIGNMENT) && RELIGHT_REGION.is_multiple_of(REGION_ALIGNMENT)
    );
    assert!(
        GTAO_REGION.is_multiple_of(REGION_ALIGNMENT) && PREFILTER_REGION.is_multiple_of(REGION_ALIGNMENT)
    );
    assert!(size_of::<PrefilterRegion>() as u64 == REGION_ALIGNMENT);
    assert!(size_of::<AmbientBlock>() == 3072);
};

/// `size` bytes of `buffer` from `offset`, as a binding.
fn region(buffer: &wgpu::Buffer, offset: u64, size: usize) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer, offset, size: NonZeroU64::new(size as u64) })
}

/// Ambient counters for the stats overlay (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AmbientStats {
    /// A probe grid is uploaded.
    pub grid: bool,
    /// The grid's probes, its kept bricks and the brick positions of its extent.
    pub probes: u32,
    pub bricks: u32,
    pub brick_positions: u32,
    /// Time-of-day keys the grid was baked at, and how many the last frame blended.
    pub keys: u32,
    pub blended_keys: u32,
    /// GPU memory of the grid: its data, the relit atlas and the brick table.
    pub grid_bytes: u64,
    /// Capsule AO and GTAO, as the renderer was created.
    pub capsule_ao: bool,
    pub gtao: bool,
    /// Character bodies the last frame's environment took capsule AO from.
    pub capsules: u32,
    /// GTAO's textures: half the scene's size, zero without.
    pub gtao_size: (u32, u32),
    /// The reflection cube's sharpest mip, texels a side, and its mips; zero without one.
    pub reflection_size: u32,
    pub reflection_mips: u32,
    /// Reflection cube faces prefiltered since the renderer was created.
    pub prefiltered_faces: u64,
    /// Bricks the last frame relit: all of them when a grid arrives or the light jumps, a slice
    /// after a small change, none while nothing changes.
    pub relit_bricks: u32,
    /// The ambient pass's pipelines built: none without a grid or GTAO; GTAO's three with the
    /// renderer, and the grid's light's three (four with the prefilter) when a grid first arrives.
    pub pipelines: u32,
    /// Compute dispatches the last recorded frame's ambient pass made: none without a grid or
    /// GTAO, when the pass doesn't run.
    pub dispatches: u32,
}

/// The renderer's ambient light and occlusion: the grid, the reflection cube, GTAO, their
/// pass and the frame group's bindings 11–16 ([`frame_binding`]).
#[derive(Debug)]
pub(crate) struct Ambient {
    device: wgpu::Device,
    occlusion: AmbientOcclusion,
    /// Where the environment reads a grid.
    lookup: ProbeLookup,
    /// Temporal AA resolves the frames, so GTAO's pattern moves every frame.
    temporal: bool,
    /// The frame's uniforms, which [`Self::split_for_record`] completes.
    block: AmbientBlock,
    /// [`AmbientBlock`]'s buffer.
    uniform_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    placeholders: Placeholders,
    grid: Option<GpuGrid>,
    /// The tier's reflection cube, with environment specular on.
    reflections: Option<ReflectionSettings>,
    /// The cube, made with the grid's light pipelines when a grid first arrives: nothing reads it
    /// without one.
    reflection: Option<Reflection>,
    gtao: Option<Gtao>,
    /// WGSL `SkyLight`, which the projection writes and the relight and prefilter read.
    sky_light: wgpu::Buffer,
    /// The sky's cube, which the projection and the prefilter read.
    sky_cube: wgpu::TextureView,
    layouts: AmbientLayouts,
    pipelines: AmbientPipelines,
    cache: Option<wgpu::PipelineCache>,
    /// The frame's uniforms, which the projection and the character pass read.
    frame_uniforms: wgpu::Buffer,
    /// Group 0 of the compute passes that read the grid: the frame's and the ambient uniforms,
    /// the atlas, the brick table and the sampler.
    compute_frame: wgpu::BindGroup,
    /// The projection's group 3: the sky cube and the sky light, read-write. Made with the
    /// grid's light pipelines.
    projection_group: Option<wgpu::BindGroup>,
    /// [`Self::block`] as last staged: a frame stages it only when it changed.
    staged: Option<AmbientBlock>,
    /// The character pass's group 1: the character buffer, read-write.
    character_group: Option<wgpu::BindGroup>,
    /// The keys the prepared frame blends, and its authored ambient sky.
    keys: ProbeKeys,
    sky: Vec3,
    schedule: RelightSchedule,
    /// Scratch for the capsules' choice: distance² to the camera, and the body.
    candidates: Vec<(f32, CharacterBody)>,
    frames: u64,
    stats: AmbientStats,
}

/// 1-texel stand-ins for the frame group's ambient bindings while nothing real is bound: they
/// are never read then, since the shaders' constants are off.
#[derive(Debug)]
struct Placeholders {
    atlas: wgpu::TextureView,
    bricks: wgpu::TextureView,
    reflection: wgpu::TextureView,
    /// Open: visibility 1.
    occlusion: wgpu::TextureView,
}

/// An uploaded grid.
#[derive(Debug)]
struct GpuGrid {
    atlas: wgpu::TextureView,
    bricks: wgpu::TextureView,
    /// Its kept bricks.
    kept: u32,
    /// The relight's group 3: its uniforms, the words, the sky light and the atlas to write.
    relight_group: wgpu::BindGroup,
    /// Kept alive by the bind group too; held to say whose it is.
    _words: wgpu::Buffer,
    /// The relight's uniforms but the keys and the slice: its counts, offsets and atlas.
    relight: RelightUniforms,
}

/// When the sky's projection runs, over which cube faces, and which bricks the relight relights:
/// every face and brick the frame a grid arrives; the faces the sky cube drew, and the authored
/// sky's scale alone when it jumps; every brick when the light jumps; after a small change of
/// what the relight reads (the sky's projection, or the keys' weights), a [`RELIGHT_SLICES`]th
/// of the bricks a frame in turn until every slice has relit since the last change; and nothing
/// while nothing changes.
#[derive(Debug, Default)]
struct RelightSchedule {
    /// Every brick is due: a grid arrived.
    all: bool,
    /// The last frame's keys, and the authored ambient sky the projection last ran with.
    keys: ProbeKeys,
    sky: Vec3,
    /// Slices still due since the last change, and the slice the next one is.
    remaining: u32,
    next: u32,
}

/// A frame's share of the grid's light: whether the sky's projection runs, the cube faces it
/// projects again (and the prefilter filters), and the kept bricks the relight relights.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RelightWork {
    project: bool,
    faces: Range<u32>,
    bricks: Range<u32>,
}

impl RelightSchedule {
    /// A schedule that relights every brick next.
    fn all() -> Self {
        Self { all: true, ..Self::default() }
    }

    /// The work of a frame of a grid of `kept` bricks whose sky cube drew `faces`, which blends
    /// `keys` under the authored ambient sky `sky`. A jump in the sky also projects it again, so
    /// the relight reads the authored light the frame it changes.
    fn frame(&mut self, kept: u32, faces: Range<u32>, keys: ProbeKeys, sky: Vec3) -> RelightWork {
        let arrived = std::mem::take(&mut self.all);
        let faces = if arrived { 0..6 } else { faces };
        let sky_jump =
            arrived || (sky - self.sky).abs().max_element() > SKY_JUMP * sky.max(self.sky).max_element();
        let jump = sky_jump || keys_change(&self.keys, &keys) > KEY_JUMP;
        let project = sky_jump || !faces.is_empty();
        if project {
            self.sky = sky;
        }
        let changed = project || keys != self.keys;
        self.keys = keys;
        if jump {
            self.remaining = 0;
            return RelightWork { project, faces, bricks: 0..kept };
        }
        if changed {
            self.remaining = RELIGHT_SLICES;
        }
        if self.remaining == 0 {
            return RelightWork { project, faces, bricks: 0..0 };
        }
        self.remaining -= 1;
        let slice = self.next;
        self.next = (slice + 1) % RELIGHT_SLICES;
        let per_slice = kept.div_ceil(RELIGHT_SLICES);
        let bricks = (slice * per_slice).min(kept)..((slice + 1) * per_slice).min(kept);
        RelightWork { project, faces, bricks }
    }
}

/// How far `b`'s blend of keys lies from `a`'s: the summed change of each key's weight.
fn keys_change(a: &ProbeKeys, b: &ProbeKeys) -> f32 {
    let weight = |keys: &ProbeKeys, key: u32| -> f32 {
        keys.keys.iter().zip(keys.weights).filter(|&(&k, _)| k == key).map(|(_, weight)| weight).sum()
    };
    let mut change = 0.0;
    let keys = || a.keys.iter().chain(&b.keys);
    for (index, &key) in keys().enumerate() {
        if !keys().take(index).any(|&seen| seen == key) {
            change += (weight(a, key) - weight(b, key)).abs();
        }
    }
    change
}

/// The reflection cube and its prefilter's per-mip groups.
#[derive(Debug)]
struct Reflection {
    size: u32,
    mips: u32,
    view: wgpu::TextureView,
    /// Per mip: the prefilter's group 3 (the sky light, its source, its uniforms, the mip).
    groups: Vec<wgpu::BindGroup>,
}

/// GTAO's settings and its textures at the scene's size.
#[derive(Debug)]
struct Gtao {
    settings: GtaoSettings,
    textures: Option<GtaoTextures>,
}

#[derive(Debug)]
struct GtaoTextures {
    /// The scene's size they were made for.
    scene: (u32, u32),
    size: (u32, u32),
    /// The view distance at half resolution and its [`GTAO_DEPTH_MIPS`] mips, which the search
    /// reads by how far each sample lies, so far samples stay in the texture cache (XeGTAO's
    /// depth prefilter).
    depths: wgpu::TextureView,
    depth_mips: [wgpu::TextureView; GTAO_DEPTH_MIPS as usize],
    raw: wgpu::TextureView,
    output: wgpu::TextureView,
    /// The blur's group 2: the raw term, the output and the uniforms.
    denoise_group: wgpu::BindGroup,
    /// The depth prefilter's group 2, per depth, character depth and outline mask views it was
    /// made for.
    depths_groups: ViewGroups<3>,
    /// The search's group 2, per normals view it was made for.
    search_groups: ViewGroups<1>,
}

/// Bind groups made for the frame graph's transient views, `N` to a group, for the last two sets
/// of views seen: the graph hands a pass the same pooled textures frame after frame, or two in
/// turn (temporal AA keeps the last frame's depth), so groups are made only after a resize.
#[derive(Debug, Default)]
struct ViewGroups<const N: usize> {
    entries: [Option<([wgpu::TextureView; N], wgpu::BindGroup)>; 2],
    /// The entry the next new set of views replaces.
    next: usize,
}

impl<const N: usize> ViewGroups<N> {
    /// The group for `views`, made by `make` if neither entry holds it.
    fn get(
        &mut self,
        views: [&wgpu::TextureView; N],
        make: impl FnOnce() -> wgpu::BindGroup,
    ) -> &wgpu::BindGroup {
        let held = self.entries.iter().position(|entry| {
            entry.as_ref().is_some_and(|(seen, _)| seen.iter().zip(views).all(|(seen, view)| seen == view))
        });
        let index = held.unwrap_or_else(|| {
            let index = self.next;
            self.next = 1 - index;
            self.entries[index] = Some((views.map(Clone::clone), make()));
            index
        });
        &self.entries[index].as_ref().expect("held or made").1
    }
}

#[derive(Debug)]
struct AmbientLayouts {
    compute_frame: wgpu::BindGroupLayout,
    projection: wgpu::BindGroupLayout,
    prefilter: wgpu::BindGroupLayout,
    relight: wgpu::BindGroupLayout,
    character: wgpu::BindGroupLayout,
    depths: wgpu::BindGroupLayout,
    search: wgpu::BindGroupLayout,
    denoise: wgpu::BindGroupLayout,
    /// Per pipeline, in [`AmbientPipelines`]' order.
    pipelines: [wgpu::PipelineLayout; 7],
}

/// The ambient pass's pipelines, each group built only once the renderer can run its work (see
/// the module docs, "Cost"): the grid's light when a grid first arrives, the prefilter with it
/// where there is a reflection cube, and GTAO's with the renderer on tiers that have it. Without
/// a grid or GTAO there are none, and no module is made. [`Ambient::rebuilt_pipelines`]' result.
#[derive(Debug, Default)]
pub(crate) struct AmbientPipelines {
    light: Option<LightPipelines>,
    occlusion: Option<OcclusionPipelines>,
}

/// The grid's light work's pipelines: the sky's projection, the relight and the characters', and
/// the reflection cube's prefilter where there is one.
#[derive(Debug)]
struct LightPipelines {
    projection: wgpu::ComputePipeline,
    relight: wgpu::ComputePipeline,
    character: wgpu::ComputePipeline,
    prefilter: Option<wgpu::ComputePipeline>,
}

/// GTAO's pipelines: the depth prefilter, the search and the blur.
#[derive(Debug)]
struct OcclusionPipelines {
    depths: wgpu::ComputePipeline,
    search: wgpu::ComputePipeline,
    denoise: wgpu::ComputePipeline,
}

/// Which of the ambient pass's pipelines to build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PipelineSet {
    /// The grid's light.
    light: bool,
    /// The prefilter, with the light.
    prefilter: bool,
    occlusion: bool,
}

impl AmbientPipelines {
    /// Builds `set` from `sources`, from one module, made only if `set` holds a pipeline.
    fn new(
        device: &wgpu::Device,
        layouts: &AmbientLayouts,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
        set: PipelineSet,
    ) -> Self {
        if !set.light && !set.occlusion {
            return Self::default();
        }
        let module = shaders::module(device, Program::Ambient, sources);
        let build = |index: usize, entry_point: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&layouts.pipelines[index]),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache,
            })
        };
        Self {
            light: set.light.then(|| LightPipelines {
                projection: build(0, "cs_sky_projection"),
                prefilter: set.prefilter.then(|| build(1, "cs_prefilter")),
                relight: build(2, "cs_relight"),
                character: build(3, "cs_character_ambient"),
            }),
            occlusion: set.occlusion.then(|| OcclusionPipelines {
                depths: build(4, "cs_gtao_depths"),
                search: build(5, "cs_gtao"),
                denoise: build(6, "cs_gtao_denoise"),
            }),
        }
    }

    /// The groups built.
    fn set(&self) -> PipelineSet {
        PipelineSet {
            light: self.light.is_some(),
            prefilter: self.light.as_ref().is_some_and(|light| light.prefilter.is_some()),
            occlusion: self.occlusion.is_some(),
        }
    }

    /// Pipelines built.
    pub(crate) fn count(&self) -> u32 {
        let set = self.set();
        3 * u32::from(set.light) + u32::from(set.prefilter) + 3 * u32::from(set.occlusion)
    }
}

/// Group 0's ambient entries in the frame layout ([`frame_binding`], `ambient.wgsl`): the grid's (its
/// uniforms, atlas, brick table and sampler) vertex- and fragment-visible, for the per-vertex
/// lookup ([`ProbeLookup::PerVertex`]); the reflection cube and GTAO fragment-visible.
pub(crate) fn frame_layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
    let fragment = wgpu::ShaderStages::FRAGMENT;
    let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
    let texture = |binding, visibility, view_dimension, sample_type| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
        count: None,
    };
    let float = wgpu::TextureSampleType::Float { filterable: true };
    [
        uniform_entry(frame_binding::UNIFORMS, both, size_of::<AmbientUniforms>()),
        texture(frame_binding::ATLAS, both, wgpu::TextureViewDimension::D3, float),
        texture(frame_binding::BRICKS, both, wgpu::TextureViewDimension::D3, wgpu::TextureSampleType::Uint),
        texture(frame_binding::REFLECTION, fragment, wgpu::TextureViewDimension::Cube, float),
        texture(frame_binding::OCCLUSION, fragment, wgpu::TextureViewDimension::D2, float),
        wgpu::BindGroupLayoutEntry {
            binding: frame_binding::SAMPLER,
            visibility: both,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn storage_entry(binding: u32, read_only: bool, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size),
        },
        count: None,
    }
}

fn texture_entry(
    binding: u32,
    view_dimension: wgpu::TextureViewDimension,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
        count: None,
    }
}

fn storage_texture_entry(
    binding: u32,
    view_dimension: wgpu::TextureViewDimension,
    format: wgpu::TextureFormat,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension,
        },
        count: None,
    }
}

impl AmbientLayouts {
    fn new(device: &wgpu::Device, layouts: &Layouts) -> Self {
        use wgpu::TextureSampleType as Sample;
        use wgpu::TextureViewDimension as Dimension;
        let compute = wgpu::ShaderStages::COMPUTE;
        let float = Sample::Float { filterable: true };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let compute_frame = layout(
            "ambient frame",
            &[
                uniform_entry(0, compute, size_of::<FrameUniforms>()),
                uniform_entry(frame_binding::UNIFORMS, compute, size_of::<AmbientUniforms>()),
                texture_entry(frame_binding::ATLAS, Dimension::D3, float),
                texture_entry(frame_binding::BRICKS, Dimension::D3, Sample::Uint),
                wgpu::BindGroupLayoutEntry {
                    binding: frame_binding::SAMPLER,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        );
        let projection = layout(
            "sky projection",
            &[texture_entry(4, Dimension::Cube, float), storage_entry(5, false, SKY_LIGHT_BYTES)],
        );
        let prefilter = layout(
            "reflection prefilter",
            &[
                storage_entry(2, true, SKY_LIGHT_BYTES),
                texture_entry(4, Dimension::Cube, float),
                uniform_entry(6, compute, size_of::<PrefilterUniforms>()),
                storage_texture_entry(7, Dimension::D2Array, STORAGE_FORMAT),
            ],
        );
        let relight = layout(
            "probe relight",
            &[
                uniform_entry(0, compute, size_of::<RelightUniforms>()),
                storage_entry(1, true, 4),
                storage_entry(2, true, SKY_LIGHT_BYTES),
                storage_texture_entry(3, Dimension::D3, STORAGE_FORMAT),
            ],
        );
        let character =
            layout("character ambient", &[storage_entry(2, false, size_of::<GpuCharacter>() as u64)]);
        let unfiltered = Sample::Float { filterable: false };
        let depths = layout(
            "gtao depths",
            &[
                texture_entry(2, Dimension::D2, Sample::Depth),
                uniform_entry(7, compute, size_of::<GtaoUniforms>()),
                storage_texture_entry(8, Dimension::D2, GTAO_DEPTH_FORMAT),
                storage_texture_entry(9, Dimension::D2, GTAO_DEPTH_FORMAT),
                storage_texture_entry(10, Dimension::D2, GTAO_DEPTH_FORMAT),
                storage_texture_entry(11, Dimension::D2, GTAO_DEPTH_FORMAT),
                texture_entry(13, Dimension::D2, Sample::Depth),
                texture_entry(14, Dimension::D2, unfiltered),
            ],
        );
        let search = layout(
            "gtao",
            &[
                texture_entry(3, Dimension::D2, unfiltered),
                storage_texture_entry(4, Dimension::D2, STORAGE_FORMAT),
                uniform_entry(7, compute, size_of::<GtaoUniforms>()),
                texture_entry(12, Dimension::D2, unfiltered),
            ],
        );
        let denoise = layout(
            "gtao denoise",
            &[
                texture_entry(5, Dimension::D2, unfiltered),
                storage_texture_entry(6, Dimension::D2, STORAGE_FORMAT),
                uniform_entry(7, compute, size_of::<GtaoUniforms>()),
            ],
        );
        let pipeline = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let pipelines = [
            pipeline("sky projection", &[Some(&compute_frame), None, None, Some(&projection)]),
            pipeline("reflection prefilter", &[Some(&compute_frame), None, None, Some(&prefilter)]),
            pipeline("probe relight", &[None, None, None, Some(&relight)]),
            pipeline("character ambient", &[Some(&compute_frame), Some(&character)]),
            pipeline("gtao depths", &[None, Some(&layouts.view), Some(&depths)]),
            pipeline("gtao", &[None, Some(&layouts.view), Some(&search)]),
            pipeline("gtao denoise", &[None, None, Some(&denoise)]),
        ];
        Self { compute_frame, projection, prefilter, relight, character, depths, search, denoise, pipelines }
    }
}

impl Placeholders {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let texture = |label, dimension, layers, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let view = |texture: &wgpu::Texture, dimension| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(dimension),
                ..Default::default()
            })
        };
        let atlas = texture("no probe atlas", wgpu::TextureDimension::D3, 1, STORAGE_FORMAT);
        let bricks = texture("no brick table", wgpu::TextureDimension::D3, 1, wgpu::TextureFormat::R32Uint);
        let reflection = texture("no reflection cube", wgpu::TextureDimension::D2, 6, STORAGE_FORMAT);
        let occlusion = texture("no gtao", wgpu::TextureDimension::D2, 1, STORAGE_FORMAT);
        // Open: visibility 1 at the farthest depth, as half floats (1, 65504, 0, 1).
        let open: [u16; 4] = [0x3c00, 0x7bff, 0, 0x3c00];
        queue.write_texture(
            occlusion.as_image_copy(),
            bytemuck::cast_slice(&open),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8), rows_per_image: None },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        Self {
            atlas: view(&atlas, wgpu::TextureViewDimension::D3),
            bricks: view(&bricks, wgpu::TextureViewDimension::D3),
            reflection: view(&reflection, wgpu::TextureViewDimension::Cube),
            occlusion: view(&occlusion, wgpu::TextureViewDimension::D2),
        }
    }
}

impl Ambient {
    /// The ambient light and occlusion of a renderer with `occlusion`, a grid read per `lookup`,
    /// temporal AA or not, and `reflections` (none with environment specular off, which never
    /// reads them). `sky_cube` is the sky's cube, which the projection and the prefilter read.
    /// Builds GTAO's pipelines if `occlusion` has it; the grid's light, its pipelines and the
    /// reflection cube wait for a grid ([`Self::set_grid`]).
    #[expect(
        clippy::too_many_arguments,
        reason = "the renderer's shared buffers and layouts, and the tier's settings, each read once"
    )]
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: &Layouts,
        frame_uniforms: &wgpu::Buffer,
        sky_cube: &wgpu::TextureView,
        occlusion: AmbientOcclusion,
        lookup: ProbeLookup,
        temporal: bool,
        reflections: Option<ReflectionSettings>,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
    ) -> Self {
        let ambient_layouts = AmbientLayouts::new(device, layouts);
        // GTAO runs every frame on the tiers that have it; the grid's light waits for a grid.
        let set = PipelineSet { occlusion: occlusion.gtao.is_some(), ..PipelineSet::default() };
        let pipelines = AmbientPipelines::new(device, &ambient_layouts, cache, sources, set);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ambient uniforms"),
            size: size_of::<AmbientBlock>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sky_light = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky light"),
            size: SKY_LIGHT_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ambient"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let placeholders = Placeholders::new(device, queue);
        let gtao = occlusion.gtao.map(|settings| Gtao { settings, textures: None });
        let compute_frame = compute_frame_group(
            device,
            &ambient_layouts,
            frame_uniforms,
            &uniform_buffer,
            (&placeholders.atlas, &placeholders.bricks),
            &sampler,
        );
        let mut block = AmbientBlock::zeroed();
        block.ambient.occlusion_size = [1.0; 4];
        let stats = AmbientStats {
            capsule_ao: occlusion.capsules,
            gtao: occlusion.gtao.is_some(),
            pipelines: pipelines.count(),
            ..AmbientStats::default()
        };
        Self {
            device: device.clone(),
            occlusion,
            lookup,
            temporal,
            block,
            uniform_buffer,
            sampler,
            placeholders,
            grid: None,
            reflections,
            reflection: None,
            gtao,
            sky_light,
            sky_cube: sky_cube.clone(),
            layouts: ambient_layouts,
            pipelines,
            cache: cache.cloned(),
            frame_uniforms: frame_uniforms.clone(),
            compute_frame,
            projection_group: None,
            staged: None,
            character_group: None,
            keys: ProbeKeys::default(),
            sky: Vec3::ZERO,
            schedule: RelightSchedule::default(),
            candidates: Vec::new(),
            frames: 0,
            stats,
        }
    }

    /// The environment pipelines' ambient constants as things stand: the tier's occlusion, and
    /// the grid while one is uploaded.
    pub(crate) fn constants(&self) -> Vec<(&'static str, f64)> {
        constants(&self.occlusion, self.grid.as_ref().map(|_| self.lookup))
    }

    /// The frame group's bindings 11–16: the uniforms, the atlas and brick table, the reflection
    /// cube, GTAO's output and the sampler, or stand-ins for those there are none of.
    pub(crate) fn frame_entries(&self) -> [wgpu::BindGroupEntry<'_>; 6] {
        fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
        }
        let placeholders = &self.placeholders;
        let (atlas, bricks) =
            self.grid.as_ref().map_or((&placeholders.atlas, &placeholders.bricks), |g| (&g.atlas, &g.bricks));
        let reflection = self.reflection.as_ref().map_or(&placeholders.reflection, |r| &r.view);
        let occlusion = self
            .gtao
            .as_ref()
            .and_then(|gtao| gtao.textures.as_ref())
            .map_or(&placeholders.occlusion, |textures| &textures.output);
        [
            wgpu::BindGroupEntry {
                binding: frame_binding::UNIFORMS,
                resource: region(&self.uniform_buffer, AMBIENT_REGION, size_of::<AmbientUniforms>()),
            },
            view(frame_binding::ATLAS, atlas),
            view(frame_binding::BRICKS, bricks),
            view(frame_binding::REFLECTION, reflection),
            view(frame_binding::OCCLUSION, occlusion),
            wgpu::BindGroupEntry {
                binding: frame_binding::SAMPLER,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ]
    }

    /// Uploads `grid`, replacing any grid there was, or removes it (`None`). The first grid also
    /// builds the light work's pipelines from `sources` and makes the reflection cube, both kept
    /// for the grids after it. The frame group and the environment pipelines must then be rebuilt
    /// ([`Self::frame_entries`], [`Self::constants`]).
    pub(crate) fn set_grid(
        &mut self,
        queue: &wgpu::Queue,
        grid: Option<&ArchivedProbeGrid>,
        sources: &ShaderSources,
    ) -> Result<(), RenderError> {
        self.grid = grid
            .map(|grid| {
                GpuGrid::new(&self.device, queue, &self.layouts, &self.sky_light, &self.uniform_buffer, grid)
            })
            .transpose()?;
        if self.grid.is_some() && self.pipelines.light.is_none() {
            self.prepare_light(sources);
        }
        let (atlas, bricks) = self
            .grid
            .as_ref()
            .map_or((&self.placeholders.atlas, &self.placeholders.bricks), |g| (&g.atlas, &g.bricks));
        self.compute_frame = compute_frame_group(
            &self.device,
            &self.layouts,
            &self.frame_uniforms,
            &self.uniform_buffer,
            (atlas, bricks),
            &self.sampler,
        );
        let uniforms = &mut self.block.ambient;
        let stats = &mut self.stats;
        match grid {
            Some(grid) => {
                let origin = grid.origin();
                let spacing = grid.spacing.to_native();
                let bricks = grid.bricks();
                let atlas = atlas_layout(grid.brick_count() as u32);
                let texels = atlas.map(|bricks| bricks * BRICK_SIDE);
                uniforms.grid_origin = [origin[0], origin[1], origin[2], spacing];
                let cells = bricks.map(|bricks| (bricks * BRICK_CELLS) as f32);
                uniforms.grid_cells = [cells[0], cells[1], cells[2], 0.0];
                uniforms.grid_bricks = [bricks[0], bricks[1], bricks[2], 1];
                uniforms.atlas = [atlas[0], atlas[1], atlas[2], texels[2]];
                let depth = texels[2] * ATLAS_SUB_VOLUMES;
                uniforms.atlas_texel[..3]
                    .copy_from_slice(&[texels[0], texels[1], depth].map(|size| (size as f32).recip()));
                *stats = AmbientStats {
                    grid: true,
                    probes: grid.probe_count() as u32,
                    bricks: grid.brick_count() as u32,
                    brick_positions: grid.brick_table.len() as u32,
                    keys: grid.key_hours.len() as u32,
                    grid_bytes: grid_bytes(grid),
                    ..*stats
                };
                tracing::info!(
                    target: LOG_TARGET,
                    probes = stats.probes,
                    bricks = stats.bricks,
                    brick_positions = stats.brick_positions,
                    keys = stats.keys,
                    bytes = stats.grid_bytes,
                    "probe grid uploaded"
                );
            }
            None => {
                uniforms.grid_bricks = [0; 4];
                *stats = AmbientStats {
                    grid: false,
                    probes: 0,
                    bricks: 0,
                    brick_positions: 0,
                    keys: 0,
                    blended_keys: 0,
                    grid_bytes: 0,
                    relit_bricks: 0,
                    ..*stats
                };
            }
        }
        self.schedule = RelightSchedule::all();
        Ok(())
    }

    /// Builds the grid's light work, when the first grid arrives: its pipelines from `sources`,
    /// the projection's group and, with environment specular on, the reflection cube and its
    /// prefilter.
    fn prepare_light(&mut self, sources: &ShaderSources) {
        let reflection = self.reflections.map(|settings| {
            Reflection::new(
                &self.device,
                &self.layouts,
                settings,
                &self.sky_cube,
                &self.sky_light,
                &self.uniform_buffer,
            )
        });
        let set = PipelineSet { light: true, prefilter: reflection.is_some(), occlusion: false };
        let built = AmbientPipelines::new(&self.device, &self.layouts, self.cache.as_ref(), sources, set);
        self.pipelines.light = built.light;
        self.projection_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky projection"),
            layout: &self.layouts.projection,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.sky_cube),
                },
                wgpu::BindGroupEntry { binding: 5, resource: self.sky_light.as_entire_binding() },
            ],
        }));
        let mips = reflection.as_ref().map_or(0, |reflection| reflection.mips);
        self.block.ambient.atlas_texel[3] = mips.saturating_sub(1) as f32;
        self.stats.reflection_size = reflection.as_ref().map_or(0, |reflection| reflection.size);
        self.stats.reflection_mips = mips;
        self.stats.pipelines = self.pipelines.count();
        self.reflection = reflection;
    }

    /// Points the character pass at the characters' buffer, after it was replaced.
    pub(crate) fn set_character_buffer(&mut self, buffer: &wgpu::Buffer) {
        self.character_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("character ambient"),
            layout: &self.layouts.character,
            entries: &[wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() }],
        }));
    }

    /// The passes' pipelines built so far, built again from `sources` (hot reload), on the
    /// layouts they had: none if none were built.
    pub(crate) fn rebuilt_pipelines(&self, sources: &ShaderSources) -> AmbientPipelines {
        AmbientPipelines::new(&self.device, &self.layouts, self.cache.as_ref(), sources, self.pipelines.set())
    }

    pub(crate) fn set_pipelines(&mut self, pipelines: AmbientPipelines) {
        self.pipelines = pipelines;
    }

    /// The CPU half of a frame: the capsules the environment reads (the valid bodies of
    /// `packet`'s characters, one per character, nearest its main view first), the keys the
    /// relight blends, and `entries`, the character entries the character pass reads. Allocates
    /// nothing once its scratch has grown.
    pub(crate) fn prepare(&mut self, packet: &FramePacket, entries: u32) {
        profiling::scope!("Ambient::prepare");
        let mut capsules = 0;
        if self.occlusion.capsules
            && let Some(view) = packet.main_view()
        {
            let eye = view.camera.position;
            self.candidates.clear();
            for character in packet.characters.iter().filter(|character| character.is_valid()) {
                let body = character.body;
                if body.casts() && !self.candidates.iter().any(|(_, other)| *other == body) {
                    let center = 0.5 * (body.base + body.top);
                    self.candidates.push((center.distance_squared(eye), body));
                }
            }
            self.candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            for (gpu, (_, body)) in self.block.ambient.capsules.iter_mut().zip(&self.candidates) {
                *gpu = GpuAmbientCapsule {
                    base: body.base.extend(body.radius).to_array(),
                    top: body.top.extend(0.0).to_array(),
                };
                capsules += 1;
            }
        }
        self.block.ambient.counts = [capsules, entries, 0, 0];
        self.stats.capsules = capsules;
        if let Some(grid) = &self.grid {
            let keys = if packet.probe_keys.is_valid() { packet.probe_keys } else { ProbeKeys::default() };
            self.keys = keys;
            self.sky = packet.ambient.sky * packet.ambient.intensity;
            self.stats.blended_keys = keys
                .keys
                .iter()
                .zip(keys.weights)
                .filter(|&(&key, weight)| weight > 0.0 && key < grid.relight.counts[3])
                .count() as u32;
        }
    }

    /// Makes GTAO's textures fit a scene of `size` pixels, before a frame records. Returns
    /// whether they were made, so the frame group, which binds the output, must be rebuilt.
    pub(crate) fn fit(&mut self, size: (u32, u32)) -> bool {
        let Some(gtao) = &mut self.gtao else { return false };
        if gtao.textures.as_ref().is_some_and(|textures| textures.scene == size) {
            return false;
        }
        gtao.textures = Some(GtaoTextures::new(&self.device, &self.layouts, &self.uniform_buffer, size));
        let half = (size.0.div_ceil(2), size.1.div_ceil(2));
        self.block.ambient.occlusion_size =
            [half.0 as f32, half.1 as f32, (half.0 as f32).recip(), (half.1 as f32).recip()];
        self.stats.gtao_size = half;
        true
    }

    /// Splits the ambient work of a frame whose sky cube draws `faces` into its uploads and its
    /// pass, for [`Renderer::record`](crate::Renderer::record); [`Self::fit`] first. The uniforms
    /// are staged only when they changed since they last were.
    pub(crate) fn split_for_record(&mut self, faces: Range<u32>) -> AmbientRecord<'_> {
        self.frames += 1;
        let Self {
            ref device,
            temporal,
            ref mut block,
            ref uniform_buffer,
            ref grid,
            ref reflection,
            ref mut gtao,
            ref layouts,
            ref pipelines,
            ref compute_frame,
            ref projection_group,
            ref character_group,
            keys,
            sky,
            ref mut schedule,
            frames,
            ref mut staged,
            ref mut stats,
            ..
        } = *self;
        let mut light = None;
        // The light pipelines are built with the first grid.
        if let (Some(grid), Some(light_pipelines)) = (grid, &pipelines.light) {
            let work = schedule.frame(grid.kept, faces, keys, sky);
            let texels = [work.bricks.start * BRICK_PROBES, work.bricks.end * BRICK_PROBES];
            block.relight = RelightUniforms {
                keys: keys.keys,
                weights: keys.weights,
                slice: [texels[0], texels[1], 0, 0],
                ..grid.relight
            };
            stats.relit_bricks = work.bricks.len() as u32;
            block.ambient.counts[2..].copy_from_slice(&[work.faces.start, work.faces.len() as u32]);
            // The prefilter, for the faces the projection projects (whose scale mip 0 reads).
            let faces = work.faces;
            let mut prefilter = None;
            if let (Some(reflection), Some(pipeline)) =
                (reflection.as_ref().filter(|_| !faces.is_empty()), &light_pipelines.prefilter)
            {
                stats.prefiltered_faces += u64::from(faces.len() as u32);
                for (region, uniforms) in
                    block.prefilter.iter_mut().zip(reflection.uniform_values(faces.start))
                {
                    region.uniforms = uniforms;
                }
                prefilter = Some(PrefilterWork { pipeline, reflection, faces });
            }
            let entries = block.ambient.counts[1];
            light = Some(LightWork {
                pipelines: light_pipelines,
                compute_frame,
                projection: projection_group.as_ref().filter(|_| work.project),
                prefilter,
                relight: (texels[1] > texels[0]).then_some((&grid.relight_group, texels[1] - texels[0])),
                characters: character_group
                    .as_ref()
                    .filter(|_| entries > 1)
                    .map(|group| (group, entries - 1)),
            });
        }
        let occlusion = gtao.as_mut().and_then(|gtao| {
            let settings = gtao.settings;
            let textures = gtao.textures.as_mut()?;
            // Built with the renderer when it has GTAO.
            let pipelines = pipelines.occlusion.as_ref()?;
            // Under temporal AA the pattern moves every frame by the golden ratio's fraction.
            let noise = if temporal { (frames as f64 * 0.618_033_988_75).fract() as f32 } else { 0.0 };
            block.gtao = GtaoUniforms {
                size: [
                    textures.size.0 as f32,
                    textures.size.1 as f32,
                    textures.scene.0 as f32,
                    textures.scene.1 as f32,
                ],
                params: [settings.radius, settings.slices as f32, settings.steps as f32, noise],
            };
            Some(OcclusionWork { pipelines, device, layouts, uniform_buffer, textures })
        });
        stats.dispatches = light.as_ref().map_or(0, LightWork::dispatches)
            + if occlusion.is_some() { OcclusionWork::DISPATCHES } else { 0 };
        let block: &AmbientBlock = block;
        let changed =
            staged.as_ref().is_none_or(|staged| bytemuck::bytes_of(staged) != bytemuck::bytes_of(block));
        if changed {
            *staged = Some(*block);
        }
        AmbientRecord {
            upload: changed.then_some(AmbientUpload { block, buffer: uniform_buffer }),
            pass: (light.is_some() || occlusion.is_some()).then_some(AmbientPass { light, occlusion }),
        }
    }

    pub(crate) fn stats(&self) -> AmbientStats {
        self.stats
    }
}

/// The compute passes' group 0 over `atlas` and `bricks`.
fn compute_frame_group(
    device: &wgpu::Device,
    layouts: &AmbientLayouts,
    frame_uniforms: &wgpu::Buffer,
    uniforms: &wgpu::Buffer,
    (atlas, bricks): (&wgpu::TextureView, &wgpu::TextureView),
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ambient frame"),
        layout: &layouts.compute_frame,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: frame_uniforms.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: frame_binding::UNIFORMS,
                resource: region(uniforms, AMBIENT_REGION, size_of::<AmbientUniforms>()),
            },
            wgpu::BindGroupEntry {
                binding: frame_binding::ATLAS,
                resource: wgpu::BindingResource::TextureView(atlas),
            },
            wgpu::BindGroupEntry {
                binding: frame_binding::BRICKS,
                resource: wgpu::BindingResource::TextureView(bricks),
            },
            wgpu::BindGroupEntry {
                binding: frame_binding::SAMPLER,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// The atlas's bricks along each axis for `kept` bricks: rows of up to [`ATLAS_ROW`], up to
/// [`ATLAS_ROW`] rows a layer.
fn atlas_layout(kept: u32) -> [u32; 3] {
    let kept = kept.max(1);
    let x = kept.min(ATLAS_ROW);
    let y = kept.div_ceil(ATLAS_ROW).min(ATLAS_ROW);
    let z = kept.div_ceil(ATLAS_ROW * ATLAS_ROW);
    [x, y, z]
}

/// The GPU memory a grid takes: its words, the atlas and the brick table.
fn grid_bytes(grid: &ArchivedProbeGrid) -> u64 {
    let atlas = atlas_layout(grid.brick_count() as u32);
    let texels = atlas.iter().map(|&bricks| u64::from(bricks * BRICK_SIDE)).product::<u64>();
    let atlas_bytes = texels * u64::from(ATLAS_SUB_VOLUMES) * 8;
    let table_bytes = grid.brick_table.len() as u64 * 4;
    atlas_bytes + table_bytes + probe_word_count(grid) as u64 * 4
}

/// The words of [`probe_layout`]'s buffer.
fn probe_word_count(grid: &ArchivedProbeGrid) -> usize {
    let visibility_words = (grid.visibility_coefficients.to_native() as usize).div_ceil(2);
    let bounce_words = BOUNCE_COEFFICIENTS as usize / 2 * grid.key_hours.len();
    grid.brick_probes.len() + grid.probe_count() * (1 + visibility_words + bounce_words)
}

/// The grid's data as the relight reads it (`probe_words` in `probes.wgsl`): the kept bricks'
/// probe indices, then per probe its data bits, its visibility (half floats, two to a word, the
/// lower first, padded to whole words) and per key its bounce. Returns the words, the offsets of
/// the data, the visibility and the bounce, and the visibility's words per probe.
fn probe_layout(grid: &ArchivedProbeGrid) -> (Vec<u32>, [u32; 4], u32) {
    let coefficients = grid.visibility_coefficients.to_native() as usize;
    let visibility_words = coefficients.div_ceil(2);
    let mut words = Vec::with_capacity(probe_word_count(grid));
    words.extend(grid.brick_probes.iter().map(|index| index.to_native()));
    let data = words.len() as u32;
    let edges = edge_probes(grid);
    words.extend(
        grid.probe_data
            .iter()
            .zip(edges)
            .map(|(bits, edge)| bits.to_native() | if edge { PROBE_EDGE_BIT } else { 0 }),
    );
    let visibility = words.len() as u32;
    // Two half floats to a word, the first in the low 16 bits; an odd last one with a zero.
    let pair = |low: u16, high: u16| u32::from(low) | u32::from(high) << 16;
    for probe in grid.visibility.chunks_exact(coefficients) {
        for two in probe.chunks(2) {
            words.push(pair(two[0].to_native(), two.get(1).map_or(0, |half| half.to_native())));
        }
    }
    let bounce = words.len() as u32;
    words.extend(
        grid.bounce.as_chunks::<2>().0.iter().map(|[low, high]| pair(low.to_native(), high.to_native())),
    );
    (words, [data, visibility, bounce, 0], visibility_words as u32)
}

/// Per probe, whether some brick it belongs to (up to eight: it may be a corner, on an edge or a
/// face, or inside) is empty or past the grid's extent: the probes the grid fades out toward.
fn edge_probes(grid: &ArchivedProbeGrid) -> Vec<bool> {
    let bricks = grid.bricks().map(i64::from);
    let kept = |brick: [i64; 3]| {
        brick.iter().zip(bricks).all(|(&b, n)| (0..n).contains(&b))
            && grid.brick_table[(brick[0] + bricks[0] * (brick[1] + bricks[1] * brick[2])) as usize]
                .to_native()
                != EMPTY_BRICK
    };
    let side = i64::from(BRICK_CELLS);
    let mut edges = vec![false; grid.probe_count()];
    for (position, entry) in grid.brick_table.iter().enumerate() {
        let entry = entry.to_native();
        if entry == EMPTY_BRICK {
            continue;
        }
        let position = position as i64;
        let brick =
            [position % bricks[0], position / bricks[0] % bricks[1], position / (bricks[0] * bricks[1])];
        for local in 0..BRICK_PROBES {
            let probe = grid.brick_probes[(entry * BRICK_PROBES + local) as usize].to_native() as usize;
            if edges[probe] {
                continue;
            }
            let offset =
                [local % BRICK_SIDE, local / BRICK_SIDE % BRICK_SIDE, local / (BRICK_SIDE * BRICK_SIDE)];
            // Along each axis, the brick holding the probe, or the two either side of the face
            // it lies on.
            let around = |axis: usize| {
                let at = brick[axis] * side + i64::from(offset[axis]);
                let low = at.div_euclid(side);
                if at.rem_euclid(side) == 0 { [low - 1, low] } else { [low, low] }
            };
            let (xs, ys, zs) = (around(0), around(1), around(2));
            edges[probe] = xs.iter().any(|&x| ys.iter().any(|&y| zs.iter().any(|&z| !kept([x, y, z]))));
        }
    }
    edges
}

impl GpuGrid {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: &AmbientLayouts,
        sky_light: &wgpu::Buffer,
        uniforms: &wgpu::Buffer,
        grid: &ArchivedProbeGrid,
    ) -> Result<Self, RenderError> {
        let kept = grid.brick_count() as u32;
        let limits = device.limits();
        let atlas = atlas_layout(kept);
        let texels = kept * BRICK_PROBES;
        let max_side = limits.max_texture_dimension_3d;
        let fits = atlas[2] * BRICK_SIDE * ATLAS_SUB_VOLUMES <= max_side
            && atlas[0] * BRICK_SIDE <= max_side
            && grid.bricks().iter().all(|&bricks| bricks <= max_side)
            && texels.div_ceil(WORKGROUP) <= limits.max_compute_workgroups_per_dimension;
        if !fits {
            return Err(RenderError::ProbeGridTooLarge { bricks: kept });
        }
        let (words, offsets, visibility_words) = probe_layout(grid);
        let word_bytes = (words.len() * 4) as u64;
        if word_bytes > limits.max_storage_buffer_binding_size {
            return Err(RenderError::ProbeGridTooLarge { bricks: kept });
        }
        let words_buffer = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("probe grid"),
                contents: bytemuck::cast_slice(&words),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe atlas"),
            size: wgpu::Extent3d {
                width: atlas[0] * BRICK_SIDE,
                height: atlas[1] * BRICK_SIDE,
                depth_or_array_layers: atlas[2] * BRICK_SIDE * ATLAS_SUB_VOLUMES,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: STORAGE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let bricks = grid.bricks();
        let table = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brick table"),
            size: wgpu::Extent3d { width: bricks[0], height: bricks[1], depth_or_array_layers: bricks[2] },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let entries: Vec<u32> = grid.brick_table.iter().map(|entry| entry.to_native()).collect();
        queue.write_texture(
            table.as_image_copy(),
            bytemuck::cast_slice(&entries),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bricks[0] * 4),
                rows_per_image: Some(bricks[1]),
            },
            table.size(),
        );
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let relight_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("probe relight"),
            layout: &layouts.relight,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: region(uniforms, RELIGHT_REGION, size_of::<RelightUniforms>()),
                },
                wgpu::BindGroupEntry { binding: 1, resource: words_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: sky_light.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
            ],
        });
        let keys = grid.key_hours.len() as u32;
        let coefficients = grid.visibility_coefficients.to_native();
        Ok(Self {
            atlas: atlas_view,
            bricks: table.create_view(&wgpu::TextureViewDescriptor::default()),
            kept,
            relight_group,
            _words: words_buffer,
            relight: RelightUniforms {
                keys: [0; 4],
                weights: [0.0; 4],
                counts: [texels, coefficients, visibility_words, keys],
                offsets,
                atlas: [atlas[0], atlas[1], atlas[2], atlas[2] * BRICK_SIDE],
                slice: [0; 4],
            },
        })
    }
}

impl Reflection {
    fn new(
        device: &wgpu::Device,
        layouts: &AmbientLayouts,
        settings: ReflectionSettings,
        sky_cube: &wgpu::TextureView,
        sky_light: &wgpu::Buffer,
        uniforms: &wgpu::Buffer,
    ) -> Self {
        let mips = settings.mips();
        let size = settings.cube_size;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reflection cube"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: STORAGE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("reflection cube"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        let mip_view = |mip: u32, dimension| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("reflection mip"),
                dimension: Some(dimension),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        // Mip 0 reads the sky cube; each rougher mip the one before it.
        let groups = (0..mips)
            .map(|mip| {
                let source = (mip > 0).then(|| mip_view(mip - 1, wgpu::TextureViewDimension::Cube));
                let target = mip_view(mip, wgpu::TextureViewDimension::D2Array);
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("reflection prefilter"),
                    layout: &layouts.prefilter,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 2, resource: sky_light.as_entire_binding() },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(source.as_ref().unwrap_or(sky_cube)),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: region(
                                uniforms,
                                PREFILTER_REGION + REGION_ALIGNMENT * u64::from(mip),
                                size_of::<PrefilterUniforms>(),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(&target),
                        },
                    ],
                })
            })
            .collect();
        Self { size, mips, view, groups }
    }

    /// Each mip's prefilter uniforms for a frame whose filtered faces start at `first_face`.
    /// Mip `i` is roughness `i / (mips − 1)`; GGX lobes of roughness α and α' convolve to about
    /// one of √(α² + α'²), so each mip adds the lobe that takes the one before it there.
    fn uniform_values(&self, first_face: u32) -> [PrefilterUniforms; MAX_REFLECTION_MIPS] {
        let mut values = [PrefilterUniforms::default(); MAX_REFLECTION_MIPS];
        let last = (self.mips - 1).max(1) as f32;
        let alpha = |mip: u32| (mip as f32 / last).powi(2);
        for (mip, value) in (0..self.mips).zip(values.iter_mut()) {
            let added =
                if mip == 0 { 0.0 } else { (alpha(mip).powi(2) - alpha(mip - 1).powi(2)).max(0.0).sqrt() };
            let samples = if mip == 0 { 1 } else { PREFILTER_SAMPLES };
            *value = PrefilterUniforms {
                mip: [(self.size >> mip) as f32, added, samples as f32, if mip == 0 { 1.0 } else { 0.0 }],
                faces: [first_face, 0, 0, 0],
            };
        }
        values
    }
}

impl GtaoTextures {
    fn new(
        device: &wgpu::Device,
        layouts: &AmbientLayouts,
        uniforms: &wgpu::Buffer,
        scene: (u32, u32),
    ) -> Self {
        let size = (scene.0.div_ceil(2).max(1), scene.1.div_ceil(2).max(1));
        let texture = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: STORAGE_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let raw = texture("gtao raw");
        let output = texture("gtao");
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gtao depths"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: GTAO_DEPTH_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GTAO_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let depths = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_mips = std::array::from_fn(|mip| {
            depth_texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: mip as u32,
                mip_level_count: Some(1),
                ..wgpu::TextureViewDescriptor::default()
            })
        });
        let denoise_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gtao denoise"),
            layout: &layouts.denoise,
            entries: &[
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&raw) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&output) },
                wgpu::BindGroupEntry { binding: 7, resource: gtao_uniforms(uniforms) },
            ],
        });
        Self {
            scene,
            size,
            depths,
            depth_mips,
            raw,
            output,
            denoise_group,
            depths_groups: ViewGroups::default(),
            search_groups: ViewGroups::default(),
        }
    }

    /// The depth prefilter's, the search's and the blur's groups 2 for this frame's `views`.
    fn groups(
        &mut self,
        device: &wgpu::Device,
        layouts: &AmbientLayouts,
        uniforms: &wgpu::Buffer,
        views: GtaoViews<'_>,
    ) -> [&wgpu::BindGroup; 3] {
        let Self { depth_mips, depths, raw, depths_groups, search_groups, denoise_group, .. } = self;
        let GtaoViews { depth, normals, outlines, characters } = views;
        let view = wgpu::BindingResource::TextureView;
        let depths_group = depths_groups.get([depth, characters, outlines], || {
            let mip = |binding: u32| wgpu::BindGroupEntry {
                binding,
                resource: view(&depth_mips[(binding - 8) as usize]),
            };
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gtao depths"),
                layout: &layouts.depths,
                entries: &[
                    wgpu::BindGroupEntry { binding: 2, resource: view(depth) },
                    wgpu::BindGroupEntry { binding: 7, resource: gtao_uniforms(uniforms) },
                    mip(8),
                    mip(9),
                    mip(10),
                    mip(11),
                    wgpu::BindGroupEntry { binding: 13, resource: view(characters) },
                    wgpu::BindGroupEntry { binding: 14, resource: view(outlines) },
                ],
            })
        });
        let search_group = search_groups.get([normals], || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gtao"),
                layout: &layouts.search,
                entries: &[
                    wgpu::BindGroupEntry { binding: 3, resource: view(normals) },
                    wgpu::BindGroupEntry { binding: 4, resource: view(raw) },
                    wgpu::BindGroupEntry { binding: 7, resource: gtao_uniforms(uniforms) },
                    wgpu::BindGroupEntry { binding: 12, resource: view(depths) },
                ],
            })
        });
        [depths_group, search_group, denoise_group]
    }
}

/// GTAO's uniforms in the ambient uniform buffer `uniforms`, as a binding.
fn gtao_uniforms(uniforms: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    region(uniforms, GTAO_REGION, size_of::<GtaoUniforms>())
}

/// A frame's ambient work, split for recording: its uploads, and its pass.
pub(crate) struct AmbientRecord<'a> {
    /// When the uniforms changed.
    pub upload: Option<AmbientUpload<'a>>,
    /// With a grid or GTAO.
    pub pass: Option<AmbientPass<'a>>,
}

/// The ambient share of a frame's uploads pass: every ambient uniform, in one write.
pub(crate) struct AmbientUpload<'a> {
    block: &'a AmbientBlock,
    buffer: &'a wgpu::Buffer,
}

impl AmbientUpload<'_> {
    pub(crate) fn write(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        buffer::stage(belt, encoder, self.buffer, 0, bytemuck::bytes_of(self.block));
    }
}

/// The reflection mips a frame filters, for the faces it filters.
struct PrefilterWork<'a> {
    pipeline: &'a wgpu::ComputePipeline,
    reflection: &'a Reflection,
    faces: Range<u32>,
}

/// A frame's grid work: the sky's projection (its group), the prefilter, the relight (its group
/// and the texels it relights) and the characters (their group and the entries past the
/// fallback), each only when due.
struct LightWork<'a> {
    pipelines: &'a LightPipelines,
    compute_frame: &'a wgpu::BindGroup,
    projection: Option<&'a wgpu::BindGroup>,
    prefilter: Option<PrefilterWork<'a>>,
    relight: Option<(&'a wgpu::BindGroup, u32)>,
    characters: Option<(&'a wgpu::BindGroup, u32)>,
}

impl LightWork<'_> {
    /// The dispatches it makes: one per mip for the prefilter, one for each other part due.
    fn dispatches(&self) -> u32 {
        u32::from(self.projection.is_some())
            + self.prefilter.as_ref().map_or(0, |prefilter| prefilter.reflection.mips)
            + u32::from(self.relight.is_some())
            + u32::from(self.characters.is_some())
    }

    fn dispatch(self, pass: &mut wgpu::ComputePass<'_>) {
        let Self { pipelines, compute_frame, projection, prefilter, relight, characters } = self;
        if let Some(group) = projection {
            pass.set_pipeline(&pipelines.projection);
            pass.set_bind_group(0, compute_frame, &[]);
            pass.set_bind_group(3, group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        if let Some(PrefilterWork { pipeline, reflection, faces }) = prefilter {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, compute_frame, &[]);
            for (mip, group) in (0..reflection.mips).zip(&reflection.groups) {
                let size = (reflection.size >> mip).div_ceil(PREFILTER_WORKGROUP);
                pass.set_bind_group(3, group, &[]);
                pass.dispatch_workgroups(size, size, faces.len() as u32);
            }
        }
        if let Some((group, texels)) = relight {
            pass.set_pipeline(&pipelines.relight);
            pass.set_bind_group(3, group, &[]);
            pass.dispatch_workgroups(texels.div_ceil(WORKGROUP), 1, 1);
        }
        if let Some((group, count)) = characters {
            pass.set_pipeline(&pipelines.character);
            pass.set_bind_group(0, compute_frame, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.dispatch_workgroups(count.div_ceil(WORKGROUP), 1, 1);
        }
    }
}

/// What GTAO reads, in a frame's graph: the scene prepass's depth, normals and outline mask, and
/// the character prepass's depth, which leave characters and their hulls out.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GtaoInputs {
    pub depth: TextureId,
    pub normals: TextureId,
    pub outlines: TextureId,
    pub characters: TextureId,
}

/// [`GtaoInputs`]' views in a frame.
#[derive(Clone, Copy)]
struct GtaoViews<'v> {
    depth: &'v wgpu::TextureView,
    normals: &'v wgpu::TextureView,
    outlines: &'v wgpu::TextureView,
    characters: &'v wgpu::TextureView,
}

/// A frame's GTAO: its pipelines, its textures, and what making their groups for the frame's
/// views needs.
struct OcclusionWork<'a> {
    pipelines: &'a OcclusionPipelines,
    device: &'a wgpu::Device,
    layouts: &'a AmbientLayouts,
    uniform_buffer: &'a wgpu::Buffer,
    textures: &'a mut GtaoTextures,
}

impl OcclusionWork<'_> {
    /// The depth prefilter, the search and the blur.
    const DISPATCHES: u32 = 3;
}

/// A frame's ambient pass: the grid's work, then GTAO's.
pub(crate) struct AmbientPass<'a> {
    light: Option<LightWork<'a>>,
    occlusion: Option<OcclusionWork<'a>>,
}

impl<'a> AmbientPass<'a> {
    /// Adds the pass to `graph`, a side effect, since what it writes is persistent. With GTAO it
    /// reads `inputs` (the scene prepass's, which makes it run) and the main view through `view`
    /// (group 1, at offset 0). Add it after the sky cube's face, which the projection and the
    /// prefilter read, and the prepasses, and before the main pass, which reads what it writes.
    pub(crate) fn add(
        self,
        graph: &mut FrameGraph<'a>,
        view: &'a wgpu::BindGroup,
        inputs: GtaoInputs,
        labels: bool,
    ) {
        let Self { light, occlusion } = self;
        let mut builder = graph.add_pass(AMBIENT_PASS).side_effect();
        if occlusion.is_some() {
            let GtaoInputs { depth, normals, outlines, characters } = inputs;
            builder = builder.read(depth).read(normals).read(outlines).read(characters);
        }
        builder.record(move |ctx| {
            // GTAO's groups for the frame's views: made before the pass begins, after a resize.
            let gtao =
                occlusion.map(|OcclusionWork { pipelines, device, layouts, uniform_buffer, textures }| {
                    let size = textures.size;
                    let groups = (size.0.div_ceil(GTAO_WORKGROUP), size.1.div_ceil(GTAO_WORKGROUP));
                    let views = GtaoViews {
                        depth: ctx.view(inputs.depth),
                        normals: ctx.view(inputs.normals),
                        outlines: ctx.view(inputs.outlines),
                        characters: ctx.view(inputs.characters),
                    };
                    (pipelines, groups, textures.groups(device, layouts, uniform_buffer, views))
                });
            let mut pass = ctx.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: labels.then_some(AMBIENT_PASS),
                timestamp_writes: ctx.compute_timestamp_writes(),
            });
            if let Some(light) = light {
                light.dispatch(&mut pass);
            }
            if let Some((pipelines, groups, [depths, search, denoise])) = gtao {
                pass.set_pipeline(&pipelines.depths);
                pass.set_bind_group(1, view, &[0]);
                pass.set_bind_group(2, depths, &[]);
                pass.dispatch_workgroups(groups.0, groups.1, 1);
                pass.set_pipeline(&pipelines.search);
                pass.set_bind_group(2, search, &[]);
                pass.dispatch_workgroups(groups.0, groups.1, 1);
                pass.set_pipeline(&pipelines.denoise);
                pass.set_bind_group(2, denoise, &[]);
                pass.dispatch_workgroups(groups.0, groups.1, 1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::probes::{EMPTY_BRICK, ProbeData, ProbeGrid};

    use super::*;

    #[test]
    fn constants_are_only_those_that_are_on() {
        assert!(constants(&AmbientOcclusion::OFF, None).is_empty(), "the shader as before W5");
        let all = AmbientOcclusion { capsules: true, gtao: Some(GtaoSettings::DESKTOP) };
        let names =
            |lookup| -> Vec<&str> { constants(&all, Some(lookup)).iter().map(|(name, _)| *name).collect() };
        assert_eq!(names(ProbeLookup::PerPixel), ["AMBIENT_PROBES", "CAPSULE_AO", "GTAO"]);
        assert_eq!(
            names(ProbeLookup::PerVertex),
            ["AMBIENT_PROBES", "AMBIENT_PROBES_PER_VERTEX", "CAPSULE_AO", "GTAO"]
        );
        let capsules = AmbientOcclusion { capsules: true, gtao: None };
        assert_eq!(constants(&capsules, None), [("CAPSULE_AO", 1.0)]);
    }

    #[test]
    fn settings_are_checked() {
        assert_eq!(GtaoSettings::DESKTOP.validate(), Ok(()));
        let bad = [
            GtaoSettings { slices: 0, ..GtaoSettings::DESKTOP },
            GtaoSettings { steps: 9, ..GtaoSettings::DESKTOP },
            GtaoSettings { radius: f32::NAN, ..GtaoSettings::DESKTOP },
            GtaoSettings { radius: 0.0, ..GtaoSettings::DESKTOP },
        ];
        for settings in bad {
            assert!(settings.validate().is_err(), "{settings:?}");
            assert!(AmbientOcclusion { capsules: false, gtao: Some(settings) }.validate().is_err());
        }
        assert!(ReflectionSettings { cube_size: 96 }.validate().is_err());
        assert!(ReflectionSettings { cube_size: 1024 }.validate().is_err());
        assert_eq!(ReflectionSettings::default().validate(), Ok(()));
        // 128 → 8 in five mips; 16 → 8 in two.
        assert_eq!(ReflectionSettings { cube_size: 128 }.mips(), 5);
        assert_eq!(ReflectionSettings { cube_size: 16 }.mips(), 2);
        assert!(ReflectionSettings { cube_size: 512 }.mips() as usize <= MAX_REFLECTION_MIPS);
    }

    #[test]
    fn rougher_mips_add_the_difference_of_their_lobes() {
        // One stand-in reflection's uniforms, without a device: only the size and mips matter.
        let alpha = |mip: u32, mips: u32| (mip as f32 / (mips - 1) as f32).powi(2);
        let mips = 5;
        let mut total = 0.0f32;
        for mip in 1..mips {
            let added = (alpha(mip, mips).powi(2) - alpha(mip - 1, mips).powi(2)).sqrt();
            total = (total * total + added * added).sqrt();
            assert!((total - alpha(mip, mips)).abs() < 1e-6, "mip {mip}: {total}");
        }
    }

    #[test]
    fn the_relight_runs_all_at_once_in_slices_or_not_at_all() {
        let sky = Vec3::new(0.3, 0.4, 0.6);
        let keys = ProbeKeys { keys: [2, 3, 0, 0], weights: [0.5, 0.5, 0.0, 0.0] };
        let mut schedule = RelightSchedule::all();
        let work = |project, faces, bricks| RelightWork { project, faces, bricks };
        // A grid arrives: every face projected, every brick relit.
        assert_eq!(schedule.frame(10, 2..3, keys, sky), work(true, 0..6, 0..10));
        // Nothing changes: nothing runs.
        assert_eq!(schedule.frame(10, 0..0, keys, sky), work(false, 0..0, 0..0));
        // The sky cube draws a face: its projection, then each slice of 3 bricks in turn, the
        // last one short, and nothing after.
        assert_eq!(schedule.frame(10, 4..5, keys, sky), work(true, 4..5, 0..3));
        for bricks in [3..6, 6..9, 9..10, 0..0] {
            assert_eq!(schedule.frame(10, 0..0, keys, sky), work(false, 0..0, bricks));
        }
        // The keys' weights drift a little: slices again from where they left off, every slice
        // after the last change.
        let drift = |by: f32| ProbeKeys { weights: [0.5 - by, 0.5 + by, 0.0, 0.0], ..keys };
        assert_eq!(schedule.frame(10, 0..0, drift(0.001), sky), work(false, 0..0, 0..3));
        assert_eq!(schedule.frame(10, 0..0, drift(0.002), sky), work(false, 0..0, 3..6));
        for bricks in [6..9, 9..10, 0..3, 0..0] {
            assert_eq!(schedule.frame(10, 0..0, drift(0.002), sky), work(false, 0..0, bricks));
        }
        // A jump in the keys or the authored sky: every brick at once, and for the sky's a
        // projection of no face, the scale alone. A small change of the sky alone waits for the
        // cube's next face.
        assert_eq!(schedule.frame(10, 0..0, drift(0.1), sky), work(false, 0..0, 0..10));
        assert_eq!(schedule.frame(10, 0..0, drift(0.1), sky * 1.2), work(true, 0..0, 0..10));
        assert_eq!(schedule.frame(10, 0..0, drift(0.1), sky * 1.21), work(false, 0..0, 0..0));
        // A key entering at no weight is no change of the blend's light.
        let entering = ProbeKeys { keys: [2, 3, 4, 0], ..drift(0.1) };
        assert!(keys_change(&drift(0.1), &entering) == 0.0);
        assert!((keys_change(&keys, &ProbeKeys::single(2)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_atlas_lays_bricks_in_rows_and_layers() {
        assert_eq!(atlas_layout(0), [1, 1, 1]);
        assert_eq!(atlas_layout(80), [64, 2, 1]);
        assert_eq!(atlas_layout(64 * 64 + 1), [64, 64, 2]);
        for kept in [1, 63, 64, 65, 4096, 5000] {
            let [x, y, z] = atlas_layout(kept);
            assert!(x * y * z >= kept, "{kept} bricks fit");
        }
    }

    /// A one-brick grid with three keys, L1 or L2 visibility.
    fn grid(coefficients: u32) -> ProbeGrid {
        let probes = BRICK_PROBES as usize;
        let keys = 3;
        let half = |value: f32| half::f16::from_f32(value).to_bits();
        ProbeGrid {
            origin: [0.0; 3],
            spacing: 1.0,
            bricks: [1, 1, 2],
            brick_table: vec![0, EMPTY_BRICK],
            brick_probes: (0..BRICK_PROBES).rev().collect(),
            key_hours: vec![6.0, 12.0, 18.0],
            visibility_coefficients: coefficients,
            visibility: (0..probes * coefficients as usize).map(|i| half(i as f32)).collect(),
            bounce: (0..probes * keys * 12).map(|i| half(-(i as f32))).collect(),
            probe_data: (0..probes)
                .map(|i| ProbeData { free: [i as u8 % 32; 6], valid: true }.to_bits())
                .collect(),
        }
    }

    #[test]
    fn probe_words_hold_every_section_in_order() {
        for coefficients in [4, 9] {
            let cooked = grid(coefficients).to_cooked().unwrap();
            let archived = ProbeGrid::read(&cooked).unwrap();
            let (words, offsets, visibility_words) = probe_layout(archived);
            let probes = BRICK_PROBES as usize;
            assert_eq!(visibility_words, coefficients.div_ceil(2));
            assert_eq!(words[0], BRICK_PROBES - 1, "the brick probes first");
            assert_eq!(offsets[0], BRICK_PROBES, "then the data");
            // Probe 5 is the brick's local probe 119, on its top face: an edge, the grid ending
            // past it.
            assert_eq!(words[offsets[0] as usize + 5], archived.probe_data[5].to_native() | PROBE_EDGE_BIT);
            let visibility = offsets[1] as usize;
            assert_eq!(visibility, offsets[0] as usize + probes);
            // Probe 2's visibility starts at its own word, low half first, padded for L2.
            let decode = |word: u32, high: bool| {
                half::f16::from_bits(if high { (word >> 16) as u16 } else { word as u16 }).to_f32()
            };
            let first = visibility + 2 * visibility_words as usize;
            assert_eq!(decode(words[first], false), (2 * coefficients) as f32);
            assert_eq!(decode(words[first], true), (2 * coefficients + 1) as f32);
            if coefficients == 9 {
                assert_eq!(words[first + 4] >> 16, 0, "the ninth's pad");
            }
            let bounce = offsets[2] as usize;
            assert_eq!(bounce, visibility + probes * visibility_words as usize);
            // Probe 1, key 2, green's first: coefficient (1 × 3 + 2) × 12 + 4.
            let index = (3 + 2) * 12 + 4;
            assert_eq!(decode(words[bounce + index / 2], index % 2 == 1), -(index as f32));
            assert_eq!(words.len(), bounce + probes * 3 * 6);
            assert_eq!(grid_bytes(archived), {
                let atlas = 5 * 5 * 5 * 5 * 8;
                (atlas + 2 * 4 + words.len() * 4) as u64
            });
        }
    }

    #[test]
    fn probes_on_the_kept_bricks_edge_are_marked() {
        // Two bricks side by side along x, kept, and a third above the first, empty: inside are
        // the probes off every outer face of the pair, the face between them included.
        let pair =
            ProbeGrid { bricks: [2, 2, 1], brick_table: vec![0, 1, EMPTY_BRICK, EMPTY_BRICK], ..grid(4) };
        // Probe indices in grid order (x fastest over 9 × 5 × 5), shared on the face between.
        let index = |[x, y, z]: [u32; 3]| x + 9 * (y + 5 * z);
        let brick_probes: Vec<u32> = (0..2)
            .flat_map(|brick| {
                (0..BRICK_PROBES).map(move |local| index([brick * 4 + local % 5, local / 5 % 5, local / 25]))
            })
            .collect();
        let probes = 9 * 5 * 5;
        let pair = ProbeGrid {
            brick_probes,
            visibility: vec![0; probes * 4],
            bounce: vec![0; probes * 3 * 12],
            probe_data: vec![0; probes],
            ..pair
        };
        let cooked = pair.to_cooked().unwrap();
        let edges = edge_probes(ProbeGrid::read(&cooked).unwrap());
        let inside: Vec<usize> = (0..probes).filter(|&probe| !edges[probe]).collect();
        let expected: Vec<usize> = (1..4)
            .flat_map(|z| (1..4).flat_map(move |y| (1..8).map(move |x| index([x, y, z]) as usize)))
            .collect();
        let mut expected = expected;
        expected.sort_unstable();
        assert_eq!(inside, expected, "7 × 3 × 3 probes off the outer faces");
    }

    #[test]
    fn sky_radiance_gives_the_authored_ambient_facing_up() {
        let sky = SkyGradient {
            zenith: Vec3::new(0.2, 0.4, 0.9),
            horizon: Vec3::new(0.7, 0.8, 0.9),
            ground: Vec3::new(0.1, 0.08, 0.05),
        };
        let ambient =
            AmbientLight { sky: Vec3::new(0.5, 0.6, 0.8), ground: Vec3::splat(0.2), intensity: 1.5 };
        let channels = sky_radiance(&sky, &ambient);
        for (channel, target) in channels.iter().zip((ambient.sky * ambient.intensity).to_array()) {
            let up = sh::evaluate(&sh::irradiance(*channel)[..sh::L1], Vec3::Y);
            assert!((up - target).abs() < 1e-4, "{up} vs {target}");
            // As an open probe holds it: the transfer of the sky through visibility 1.
            let open = sh::SkyTransfer::new(channel).apply(&sh::project_constant(1.0));
            let held = sh::evaluate(&open, Vec3::Y);
            assert!((held - target).abs() < 1e-4, "{held} vs {target}");
            // Lit less from below, where the gradient's ground is darker, and alike around.
            let down = sh::evaluate(&sh::irradiance(*channel), Vec3::NEG_Y);
            assert!(down < up);
            let east = sh::evaluate(&sh::irradiance(*channel), Vec3::X);
            let north = sh::evaluate(&sh::irradiance(*channel), Vec3::NEG_Z);
            assert!((east - north).abs() < 1e-5);
        }
        // A black sky lends nothing, whatever the ambient.
        let black = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        assert_eq!(sky_radiance(&black, &ambient), [[0.0; sh::L2]; 3]);
    }

    #[test]
    fn the_cpu_gradient_matches_sky_color() {
        let sky = SkyGradient { zenith: Vec3::X, horizon: Vec3::Y, ground: Vec3::Z };
        assert_eq!(sky_gradient(&sky, Vec3::Y), Vec3::X, "the zenith");
        assert_eq!(sky_gradient(&sky, Vec3::X), Vec3::Y, "the horizon");
        assert_eq!(sky_gradient(&sky, Vec3::NEG_Y), Vec3::Z, "the ground");
        let low = sky_gradient(&sky, Vec3::new(0.99, -0.1, 0.0).normalize());
        assert!(low.y > 0.0 && low.z > 0.0, "a quick fade below the horizon");
    }
}
