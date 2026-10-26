//! Foliage (M1 W6; spec §5.12 "Foliage", look pillar 7): trees, bushes and grass in the
//! world's painted look. Anime foliage reads as soft clumps rather than noisy leaves: a canopy
//! lights as one rounded volume with a clean terminator and a soft colored shade, its leaf cards
//! breaking the silhouette, and everything sways in layered wind.
//!
//! The shader is `shaders/foliage.wgsl`, whose header has the model. Its materials are cooked
//! [`FoliageMaterial`](sapphire_assets::material::FoliageMaterial)s, drawn on meshes beside the
//! other families (see the crate docs, "Material families on meshes"); the frame's
//! [`Wind`](crate::Wind) moves them.
//!
//! # Meshes
//!
//! Foliage meshes are cooked with the pipeline's foliage bake (`sapphire_pipeline::bake::foliage`,
//! chosen per mesh in its import settings), which writes what the shader needs:
//!
//! - **Volume normals:** leaf cards' normals replaced by those of the canopy's volume (a fitted
//!   sphere, clumps of spheres, or a smooth hull over the clumps), so a canopy lights as one
//!   shape; grass blades take the ground's normal (straight up), so a field lights like the
//!   ground it covers. Blob canopies (a closed low-poly shell, as stylized kits model them) work
//!   the same way: the sphere fit rounds their facets off, and the bake's `canopy_from` keeps a
//!   trunk modeled in the canopy's material slot as wood.
//! - **The foliage stream** ([`VertexFoliage`](sapphire_assets::mesh::VertexFoliage), vertex
//!   slot 5): each vertex's weights for the trunk's sway, its branch's bend and its leaf's
//!   flutter, its branch's phase, and the color gradient's coordinate (root to tip, the canopy's
//!   inside to its outside), from each vertex's height, its distance from the trunk's axis and
//!   its depth in the canopy (the bake's docs have the table).
//!
//! [`Renderer::upload_mesh`](crate::Renderer::upload_mesh) refuses foliage materials on a mesh
//! without the stream ([`RenderError::NoFoliageStream`](crate::RenderError::NoFoliageStream)) and
//! on skinned or morphed meshes ([`RenderError::DeformedFoliage`](crate::RenderError::DeformedFoliage)):
//! wind displaces foliage in its vertex stage, from the static mesh. The stream lives in a buffer
//! of the mesh pool's own, with its own numbering, so other meshes spend nothing on it.
//!
//! # Shading
//!
//! The environment's light model (W4), so foliage sits in the world's picture: the world key
//! light through the terminator remap whose width grows with distance, wrapped Lambert, time of
//! day's shadow tint × the material's, the ambient hemisphere and distance desaturation, sharing
//! `frame.wgsl`'s functions with the environment shader. On top: canopy occlusion and the color
//! gradient along the gradient coordinate, and translucency, light through thin leaves seen
//! against the key light. Shadows (R4) enter the terminator as for the environment: foliage
//! receives the cascades and the characters' own shadows, and casts into the cascades.
//!
//! # Wind
//!
//! Three levels, each the material's response (meters at unit strength) × the vertex's cooked
//! weight × the frame's wind: the trunk leans downwind and sways slowly, branches swing faster in
//! their own phases, leaves flutter. Gusts swell and travel downwind. [`wind_offset`] is the
//! shader's function on the CPU, for tests and for gameplay that must follow a swaying branch.
//!
//! Time is the packet's `time_seconds`, wrapped to [`WIND_PERIOD`] in `f64` before it reaches
//! the GPU, and every oscillator runs a whole number of cycles in the period, so the wrap never
//! jumps and long sessions keep `f32` precision. The prepass's velocity (temporal AA) takes the
//! last drawn frame's wind as well as its transform, so swaying leaves reproject without ghosts;
//! after a camera cut the wind counts as unmoved, as the camera does. Every pass that draws
//! foliage displaces it alike (the main pass, the scene prepass and the shadow casters), so its
//! depths match and its shadows sway with it.
//!
//! # Coverage
//!
//! Leaf and blade cards are alpha-tested: a material with an alpha cutoff of 0 draws opaque (for
//! trunks and solid canopies, which cull their back faces: their shapes are closed, so cards
//! drawn opaque must be modeled two-sided). Cards are two-sided, lit alike from both sides by
//! their volume normals. On desktop, temporal AA's jittered history resolves the alpha test's edges, whose
//! alpha grows with the map's mip level so distant canopies stay full; the scene prepass and the
//! shadow casters test alike, so shadows are the leaves' shapes, not their cards'. Under MSAA
//! (the phones) the tier can write alpha-to-coverage instead: the main pass covers the share of
//! the pixel's samples that the card's alpha covers, through the sample mask, so card edges
//! resolve in tile memory. The run of samples starts where a hash of the pixel and the card
//! picks, a dither: 4× has five levels, which an ordered mask would band, and two half-covering
//! edges stacked in a pixel would otherwise cover the same half and leave the canopy
//! see-through there. The mask is written by the shader rather than by the hardware's
//! alpha-to-coverage, which would also store the alpha in scene color, where post's character
//! mask reads 1 for the world (M1 P2).
//!
//! # Tiers
//!
//! [`FoliageSettings`] (`tiers.ron`): how many of the wind's levels run, alpha-to-coverage under
//! MSAA, and translucency. Desktop tiers run every level with translucency; phones take
//! alpha-to-coverage, and Mobile-Low drops the leaves' flutter and translucency.
//!
//! # Passes and bindings
//!
//! Foliage draws in the passes every surface draws in, through pipelines of its own per coverage
//! (`pipelines`): in the main pass after the environment in each coverage, in the scene prepass
//! after the shared prepass pipeline's draws, and in the shadow cascades after the shared
//! casters. Its scene group (group 2) holds the instances, the family's material blocks and the
//! wind ([`WindUniforms`], binding 2); its prepass and casters also bind its group 3, for the
//! alpha test. Its GPU time is the main pass's, the scene prepass's and the shadow passes' (the
//! stats overlay's forward, prepass and shadow rows); [`RenderStats`](crate::RenderStats) counts
//! its draws, instances and pipelines.
//!
//! # Later milestones
//!
//! - **Impostors and billboards** (M2): distant trees as octahedral impostors and grass as
//!   camera-facing cards past the mesh LODs. Their vertex shaders will orient cards by the view's
//!   axes, so in the shadow passes they face the light without code of their own (the `shadows`
//!   module docs).
//! - **The GPU-driven path:** the foliage stream's own numbering needs a second base vertex (or a
//!   per-draw offset in the draw record) when draws move to multi-draw indirect.
//! - **Wind zones and interaction:** wind per zone and a time-of-day curve in the world, and
//!   characters pushing grass aside (a trail buffer the vertex stage reads).
//! - **Skinned foliage:** wind on top of the skinning pass's outputs, for vines and banners on
//!   animated props.
//! - **Grass density and placement** from terrain masks (with world streaming), and LODs that
//!   thin cards with distance.
//! - **Cheaper shadow casters:** the casters are the 14-plant test grove's largest cost on
//!   Desktop-High (an M5 at 1080p: the shadow passes 698 µs against 242 without the plants, 42
//!   caster draws over the three cascades drawn a frame), most of it the cards' alpha-tested
//!   overdraw. Casters could leave out the leaves' flutter (centimeters, under a cascade texel past
//!   the nearest cascade), and a caster LOD could draw fewer, larger cards, or the canopy's volume
//!   opaque, into the farther cascades.

use std::f32::consts::TAU;

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use sapphire_assets::material::Wind as WindResponse;
use sapphire_assets::mesh::FoliageVertex;
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

use crate::pipelines::Coverage;
use crate::{FramePacket, Wind};

/// The wind's period in seconds: time reaches the shaders wrapped to it, and every oscillator runs
/// a whole number of cycles in it (`foliage.wgsl`'s `WIND_PERIOD`).
pub const WIND_PERIOD: f64 = 1024.0;

/// The trunk's swing, the branches' and their bob's frequencies, and the gusts' swell, in cycles
/// per second before rounding to whole cycles per period (`foliage.wgsl`'s `WIND_*_HZ`).
pub(crate) const SWAY_HZ: f32 = 0.31;
pub(crate) const BEND_HZ: f32 = 0.83;
pub(crate) const BOB_HZ: f32 = 1.27;
pub(crate) const GUST_HZ: f32 = 0.09;
/// Meters between gust fronts.
pub(crate) const GUST_WAVELENGTH: f32 = 36.0;
/// The trunk's lean downwind and its swing around it, as shares of its sway.
pub(crate) const LEAN: f32 = 0.6;
pub(crate) const SWING: f32 = 0.4;
/// How fast the flutter's direction wanders across space, in cycles per meter.
pub(crate) const FLUTTER_SPREAD: Vec3 = Vec3::new(1.37, 0.91, 1.13);
/// Each plant's own timing from where it stands, in cycles per meter of x and z.
pub(crate) const PLANT_PHASE: [f32; 2] = [0.0713, 0.0917];

/// The names of the foliage shader's tier and sample constants.
pub(crate) mod override_constant {
    pub const WIND_LEVELS: &str = "FOLIAGE_WIND_LEVELS";
    pub const TRANSLUCENCY: &str = "FOLIAGE_TRANSLUCENCY";
    pub const SAMPLES: &str = "FOLIAGE_SAMPLES";
}

/// Foliage's settings for a renderer, from its quality tier (`tiers.ron`; see the
/// [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct FoliageSettings {
    /// How many of the wind's levels run, 0..=[`Self::MAX_WIND_LEVELS`]: 0 holds foliage still,
    /// 1 sways trunks, 2 bends branches too, 3 flutters leaves too.
    pub wind_levels: u32,
    /// Under MSAA, alpha-tested cards cover the share of each pixel's samples their alpha covers
    /// (phones); without MSAA, or with this off, they are alpha-tested.
    pub alpha_to_coverage: bool,
    /// Light through leaves seen against the key light.
    pub translucency: bool,
}

impl Default for FoliageSettings {
    /// The desktop tiers': every wind level, translucency, and alpha-to-coverage whenever MSAA
    /// is on.
    fn default() -> Self {
        Self { wind_levels: Self::MAX_WIND_LEVELS, alpha_to_coverage: true, translucency: true }
    }
}

impl FoliageSettings {
    /// Trunk sway, branch bend and leaf flutter.
    pub const MAX_WIND_LEVELS: u32 = 3;

    /// Checks every knob is in its range, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if self.wind_levels > Self::MAX_WIND_LEVELS {
            return Err(("foliage.wind_levels", "must be within 0..=3"));
        }
        Ok(())
    }

    /// Whether alpha-tested surfaces write alpha-to-coverage in a main pass of `samples`.
    pub(crate) fn covers(&self, samples: u32) -> bool {
        self.alpha_to_coverage && samples > 1
    }

    /// The wind levels' pipeline constant, which every foliage pass takes.
    pub(crate) fn wind_constant(&self) -> (&'static str, f64) {
        (override_constant::WIND_LEVELS, f64::from(self.wind_levels.min(Self::MAX_WIND_LEVELS)))
    }

    /// The surfaces' tier constants: the wind levels and translucency.
    pub(crate) fn constants(&self) -> [(&'static str, f64); 2] {
        [self.wind_constant(), (override_constant::TRANSLUCENCY, f64::from(u8::from(self.translucency)))]
    }
}

/// One frame's wind as the shaders read it (WGSL `WindState`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct WindState {
    /// xyz: the unit horizontal direction the wind blows toward; w: its strength (0 calm).
    pub direction: [f32; 4],
    /// x: time in seconds, wrapped to [`WIND_PERIOD`]; y: gustiness, 0 to 1.
    pub time: [f32; 4],
}

impl WindState {
    /// `wind` at `time_seconds`: its direction made horizontal and unit (calm without one), its
    /// strength at least 0, its gustiness within 0..=1 and the time wrapped, each finite.
    pub(crate) fn new(wind: &Wind, time_seconds: f64) -> Self {
        let horizontal = Vec3::new(wind.direction.x, 0.0, wind.direction.z);
        let direction = horizontal.try_normalize().unwrap_or(Vec3::ZERO);
        let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
        let strength = if direction == Vec3::ZERO { 0.0 } else { finite(wind.strength).max(0.0) };
        let time = if time_seconds.is_finite() { time_seconds.rem_euclid(WIND_PERIOD) as f32 } else { 0.0 };
        Self {
            direction: direction.extend(strength).to_array(),
            time: [time, finite(wind.gustiness).clamp(0.0, 1.0), 0.0, 0.0],
        }
    }

    /// How far this wind moves a vertex at world `position` (before the wind) of a plant rooted
    /// at `origin`, with cooked `weights` (sway, bend, flutter, phase), under a material's
    /// `response` (sway, bend, flutter, flutter frequency), with `levels` of the wind running:
    /// `foliage.wgsl`'s `wind_offset`, operation for operation.
    pub(crate) fn offset(
        &self,
        levels: u32,
        response: [f32; 4],
        weights: [f32; 4],
        position: Vec3,
        origin: Vec3,
    ) -> Vec3 {
        let [x, _, z, strength] = self.direction;
        let direction = Vec3::new(x, 0.0, z);
        let [t, gustiness, ..] = self.time;
        let downwind = (position.x * direction.x + position.z * direction.z) / GUST_WAVELENGTH;
        let gust = 1.0 + gustiness * (TAU * fract(cycles(GUST_HZ, t) - downwind)).sin();
        let plant = origin.x * PLANT_PHASE[0] + origin.z * PLANT_PHASE[1];
        let phase = weights[3];
        let mut offset = Vec3::ZERO;
        if levels >= 1 {
            let swing = (TAU * fract(cycles(SWAY_HZ, t) + plant)).sin();
            offset += direction * (response[0] * weights[0] * (LEAN + SWING * swing));
        }
        if levels >= 2 {
            let branch = TAU * fract(cycles(BEND_HZ, t) + phase + plant);
            let bob = TAU * fract(cycles(BOB_HZ, t) + phase);
            offset += (direction * branch.sin() + Vec3::new(0.0, 0.4 * bob.cos(), 0.0))
                * (response[1] * weights[1]);
        }
        if levels >= 3 {
            let a = TAU * fract(cycles(response[3], t) + phase + fract(position.dot(FLUTTER_SPREAD)));
            let wander = Vec3::new(a.sin(), 0.6 * (2.0 * a + 1.3).sin(), (3.0 * a + 0.4).cos());
            offset += wander * (response[2] * weights[2]);
        }
        offset *= strength * gust;
        let height = (position.y - origin.y).max(0.0);
        let sideways = offset.x * offset.x + offset.z * offset.z;
        offset.y -= (sideways / (2.0 * height.max(0.1))).min(sideways.sqrt());
        offset
    }
}

/// `x - floor(x)`, as WGSL's `fract`.
fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Cycles of an oscillator at `hz` at wrapped time `t`, in [0, 1): the frequency rounded to a
/// whole number of cycles per [`WIND_PERIOD`], so the wrap changes nothing. Ties round to even,
/// as WGSL's `round` does.
fn cycles(hz: f32, t: f32) -> f32 {
    let period = WIND_PERIOD as f32;
    fract((hz * period).round_ties_even() / period * t)
}

/// The faces foliage of `coverage` culls in the main pass and the scene prepass: none for
/// alpha-tested cards and blades, seen from both sides; back faces for opaque foliage, wood and
/// solid canopies, whose shapes are closed. Cards drawn opaque must be modeled two-sided. The
/// shadow casters are two-sided whatever the coverage, as every family's are.
pub(crate) fn cull(coverage: Coverage) -> Option<wgpu::Face> {
    (coverage != Coverage::AlphaTest).then_some(wgpu::Face::Back)
}

/// The farthest the wind can carry a vertex of a material with `response` (sway, bend, flutter,
/// flutter frequency; [`FoliageParams::wind`](sapphire_assets::material::FoliageParams)), in
/// meters, at unit strength and without gusts: [`WindState::offset`]'s bound. The trunk's lean
/// and swing reach its sway, the branch's swing with its bob √(1 + 0.4²) of its bend, the leaf's
/// wander √(1 + 0.6² + 1) of its flutter, every weight at most 1; the droop toward the root can
/// lengthen their sum by up to the golden ratio (a sideways offset `h` with a vertical one `y`
/// droops by at most `h`, and `h² + (|y| + h)²` peaks at φ² (`h² + y²`)).
pub(crate) fn wind_reach(response: [f32; 4]) -> f32 {
    const BOB: f32 = 1.077_033; // √(1 + 0.4²)
    const WANDER: f32 = 1.536_229_1; // √(1 + 0.6² + 1)
    const DROOP: f32 = std::f32::consts::GOLDEN_RATIO;
    let [sway, bend, flutter, _] = response.map(f32::abs);
    DROOP * (sway * (LEAN + SWING) + bend * BOB + flutter * WANDER)
}

/// How much farther than its cooked bounds `wind` can carry foliage whose materials reach `reach`
/// meters ([`wind_reach`]): times the wind's strength and its strongest gust (`1 + gustiness`).
/// Shadow casters' culling pads foliage's bounds by it (the [`shadows`](crate::shadows) module).
pub(crate) fn wind_margin(reach: f32, wind: &Wind) -> f32 {
    let state = WindState::new(wind, 0.0);
    reach * state.direction[3] * (1.0 + state.time[1])
}

/// This frame's wind and the last drawn frame's, for velocity (WGSL `WindUniforms`, foliage's
/// scene group, binding 2).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct WindUniforms {
    pub current: WindState,
    pub previous: WindState,
}

/// How far `wind` at `time_seconds` moves a foliage vertex, in meters, world space: the foliage
/// shader's displacement on the CPU (`foliage.wgsl`'s `wind_offset`), for tests and for gameplay
/// that must follow what the GPU draws. `position` is the vertex's world position before the
/// wind, `origin` its instance's (the plant's root, its transform's translation), `vertex` its
/// cooked foliage data as the stream holds it ([`FoliageVertex::decode`]) and `response` its
/// material's [`Wind`](sapphire_assets::material::Wind). `levels` is the tier's
/// [`FoliageSettings::wind_levels`]. The GPU's `sin` differs from the CPU's in the last bits, so
/// expect agreement to a few micrometers per meter of response.
pub fn wind_offset(
    wind: &Wind,
    time_seconds: f64,
    levels: u32,
    response: &WindResponse,
    vertex: &FoliageVertex,
    position: Vec3,
    origin: Vec3,
) -> Vec3 {
    let response = [response.sway, response.bend, response.flutter, response.flutter_frequency];
    let weights = [vertex.sway, vertex.bend, vertex.flutter, vertex.phase];
    WindState::new(wind, time_seconds).offset(levels, response, weights, position, origin)
}

/// The wind's GPU side: its uniform buffer, and the prepared and drawn frames' wind.
#[derive(Debug)]
pub(crate) struct FoliageWind {
    buffer: wgpu::Buffer,
    history: WindHistory,
}

impl FoliageWind {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wind"),
            size: size_of::<WindUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, history: WindHistory::default() }
    }

    /// Foliage's scene group's binding 2.
    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Takes the packet's wind and time.
    pub(crate) fn prepare(&mut self, packet: &FramePacket) {
        self.history.current = WindState::new(&packet.wind, packet.time_seconds);
    }

    /// The recorded frame's uniforms ([`WindHistory::record`]).
    pub(crate) fn record(&mut self, cut: bool) -> WindUniforms {
        self.history.record(cut)
    }
}

/// The prepared frame's wind and the last recorded frame's, which velocity compares with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct WindHistory {
    current: WindState,
    drawn: Option<WindState>,
}

impl WindHistory {
    /// The recorded frame's uniforms: the prepared wind, and the last recorded frame's for
    /// velocity; the same after a `cut` (or with none before), when velocity counts the camera as
    /// unmoved too.
    fn record(&mut self, cut: bool) -> WindUniforms {
        let previous = match self.drawn.replace(self.current) {
            Some(previous) if !cut => previous,
            _ => self.current,
        };
        WindUniforms { current: self.current, previous }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breeze() -> Wind {
        Wind { direction: Vec3::new(3.0, 5.0, 4.0), strength: 1.0, gustiness: 0.3 }
    }

    const RESPONSE: [f32; 4] = [0.2, 0.15, 0.03, 2.3];
    const TIP: [f32; 4] = [1.0, 0.8, 1.0, 0.37];

    #[test]
    fn the_state_is_horizontal_unit_and_wrapped() {
        let state = WindState::new(&breeze(), WIND_PERIOD * 3.0 + 12.5);
        assert_eq!(state.direction, [0.6, 0.0, 0.8, 1.0]);
        assert_eq!(state.time, [12.5, 0.3, 0.0, 0.0]);
        // Straight up, or no direction at all, is calm; nonsense is tamed.
        let up = Wind { direction: Vec3::Y, ..breeze() };
        assert_eq!(WindState::new(&up, 1.0).direction, [0.0; 4]);
        let wild = Wind { direction: Vec3::X, strength: f32::NAN, gustiness: 7.0 };
        let state = WindState::new(&wild, f64::INFINITY);
        assert_eq!((state.direction[3], state.time[0], state.time[1]), (0.0, 0.0, 1.0));
        assert_eq!(WindState::new(&Wind::default(), 5.0).direction[3], 0.0, "the default is calm");
    }

    #[test]
    fn calm_or_still_tiers_and_roots_never_move() {
        let at = Vec3::new(4.0, 2.0, -3.0);
        let calm = WindState::new(&Wind::default(), 17.0);
        assert_eq!(calm.offset(3, RESPONSE, TIP, at, Vec3::ZERO), Vec3::ZERO);
        let state = WindState::new(&breeze(), 17.0);
        assert_eq!(state.offset(0, RESPONSE, TIP, at, Vec3::ZERO), Vec3::ZERO, "no levels run");
        assert_eq!(state.offset(3, RESPONSE, [0.0, 0.0, 0.0, 0.5], at, Vec3::ZERO), Vec3::ZERO, "a root");
        assert_ne!(state.offset(3, RESPONSE, TIP, at, Vec3::ZERO), Vec3::ZERO);
    }

    #[test]
    fn each_level_adds_its_own_motion_within_its_reach() {
        let at = Vec3::new(1.0, 3.0, 0.5);
        for step in 0..200 {
            let state = WindState::new(&breeze(), f64::from(step) * 0.137);
            let one = state.offset(1, RESPONSE, TIP, at, Vec3::ZERO);
            let two = state.offset(2, RESPONSE, TIP, at, Vec3::ZERO);
            let three = state.offset(3, RESPONSE, TIP, at, Vec3::ZERO);
            // The trunk leans downwind, never against it.
            assert!(one.dot(Vec3::new(0.6, 0.0, 0.8)) > 0.0, "{one}");
            // Gusts swell the strength by at most the gustiness; each level's reach adds up, and
            // bending drops the tip by less than it moves sideways.
            let reach = 1.3 * (RESPONSE[0] + RESPONSE[1] * 1.08 + RESPONSE[2] * 1.54) * 1.1;
            assert!(three.length() <= reach, "{three} beyond {reach}");
            assert_ne!(one, two);
            assert_ne!(two, three);
        }
    }

    #[test]
    fn the_wind_is_continuous_across_the_period_s_wrap() {
        let at = Vec3::new(-7.0, 1.5, 2.0);
        let offset = |seconds: f64| {
            WindState::new(&breeze(), seconds).offset(3, RESPONSE, TIP, at, Vec3::new(-7.0, 0.0, 2.0))
        };
        let before = offset(WIND_PERIOD * 5.0 - 0.001);
        let after = offset(WIND_PERIOD * 5.0 + 0.001);
        // Two milliseconds of motion (under 4 mm at this flutter); an oscillator out of step
        // with the period would jump by a share of its reach.
        assert!(before.distance(after) < 0.005, "{before} to {after}");
        // And the same instant a period later is the same offset.
        assert!(offset(100.25).distance(offset(100.25 + WIND_PERIOD)) < 1e-4);
    }

    #[test]
    fn the_wind_never_carries_a_vertex_past_its_margin() {
        // Every level at full weight, a gale with full gusts, over many instants, places and
        // phases: the offset stays within the margin shadow culling pads foliage by, and comes
        // within a fifth of it (the bound isn't loose).
        let gale = Wind { direction: Vec3::new(1.0, 0.0, -2.0), strength: 2.5, gustiness: 1.0 };
        let margin = wind_margin(wind_reach(RESPONSE), &gale);
        let mut farthest = 0.0f32;
        for step in 0..4000 {
            let seconds = f64::from(step) * 0.0173;
            let phase = (step % 97) as f32 / 97.0;
            let at =
                Vec3::new((step % 13) as f32 - 6.0, 0.05 + (step % 7) as f32 * 0.4, (step % 11) as f32 - 5.0);
            let origin = Vec3::new(at.x, 0.0, at.z);
            let offset =
                WindState::new(&gale, seconds).offset(3, RESPONSE, [1.0, 1.0, 1.0, phase], at, origin);
            farthest = farthest.max(offset.length());
        }
        assert!(farthest <= margin, "{farthest} past {margin}");
        assert!(farthest > margin * 0.2, "{farthest} against {margin}");
        // Calm reaches nothing; the breeze reaches its reach times its gusts.
        assert_eq!(wind_margin(1.0, &Wind::default()), 0.0);
        assert!((wind_margin(1.0, &breeze()) - 1.3).abs() < 1e-6);
    }

    #[test]
    fn frequencies_round_ties_to_even_as_wgsl_does() {
        // 2.5 cycles a period rounds to 2, as WGSL's `round` does, not to 3.
        let hz = 2.5 / WIND_PERIOD as f32;
        assert_eq!(cycles(hz, 1.0), 2.0 / WIND_PERIOD as f32);
    }

    #[test]
    fn the_public_mirror_reads_materials_and_cooked_vertices() {
        let response = WindResponse { sway: 0.2, bend: 0.15, flutter: 0.03, flutter_frequency: 2.3 };
        let vertex = FoliageVertex { sway: 1.0, bend: 0.8, flutter: 1.0, phase: 0.37, gradient: 1.0 };
        let (at, origin) = (Vec3::new(1.0, 3.0, 0.5), Vec3::ZERO);
        let expected = WindState::new(&breeze(), 9.0).offset(3, RESPONSE, TIP, at, origin);
        assert_eq!(wind_offset(&breeze(), 9.0, 3, &response, &vertex, at, origin), expected);
    }

    #[test]
    fn velocity_compares_with_the_last_recorded_frame_s_wind() {
        let state = |seconds| WindState::new(&breeze(), seconds);
        let mut history = WindHistory::default();
        let mut record = |seconds, cut| {
            history.current = state(seconds);
            let uniforms = history.record(cut);
            (uniforms.current, uniforms.previous)
        };
        assert_eq!(record(1.0, false), (state(1.0), state(1.0)), "the first frame holds still");
        assert_eq!(record(2.0, false), (state(2.0), state(1.0)));
        assert_eq!(record(3.0, true), (state(3.0), state(3.0)), "a cut holds still");
        assert_eq!(record(4.0, false), (state(4.0), state(3.0)));
    }

    #[test]
    fn settings_default_to_the_desktop_s_and_validate() {
        let settings = FoliageSettings::default();
        assert_eq!(settings.wind_levels, 3);
        assert!(settings.covers(4) && !settings.covers(1), "alpha-to-coverage needs MSAA");
        assert_eq!(settings.constants(), [("FOLIAGE_WIND_LEVELS", 3.0), ("FOLIAGE_TRANSLUCENCY", 1.0)]);
        assert!(settings.validate().is_ok());
        let wrong = FoliageSettings { wind_levels: 4, ..settings };
        assert_eq!(wrong.validate(), Err(("foliage.wind_levels", "must be within 0..=3")));
    }
}
