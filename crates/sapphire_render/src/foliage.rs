//! Foliage (M1 W6; spec §5.12 "Foliage", look pillar 7): trees, bushes and grass in the
//! world's painted look. Anime foliage reads as soft clumps rather than noisy leaves: a canopy
//! lights as one rounded volume with a clean terminator and a soft colored shade, its leaf cards
//! breaking the silhouette, and everything sways in layered wind.
//!
//! The shader is `shaders/foliage.wgsl`, whose header has the model. Its materials are cooked
//! [`FoliageMaterial`](sapphire_assets::material::FoliageMaterial)s, drawn on meshes beside the
//! other families (see the crate docs, "Material families on meshes"); the frame's wind field
//! ([`WindField`], the [`wind`](crate::wind) module) moves them.
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
//! their own phases, leaves flutter. Gusts swell and travel downwind. With
//! [`FoliageSettings::local_air`] on, the field's local air (turbulence and volumes, M6 Y1),
//! sampled at each vertex or at the plant's root, moves the same levels again along its own
//! horizontal direction at its own speed, without gusts, beside the global wind's offset; a frame
//! without local air skips it. Off, the shader compiles it out and draws exactly as foliage drew
//! before the field: Metal's compiler moved a few pixels of every swaying plant by a code value
//! when the local air's code was merely present, even untaken. [`wind_offset`] is the shader's
//! function on the CPU, for tests and for gameplay that must follow a swaying branch.
//!
//! Time is the packet's `time_seconds`, wrapped to the field's period
//! ([`WIND_PERIOD`](sapphire_look::wind::WIND_PERIOD)) in `f64` before it reaches
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
//! wind (the [`wind`](crate::wind) module's buffer, binding 2); its prepass and casters also bind its group 3, for the
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
//! - **Interaction:** characters pushing grass aside (a trail buffer the vertex stage reads).
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
use glam::Vec3;
use sapphire_assets::material::Wind as WindResponse;
use sapphire_assets::mesh::FoliageVertex;
use sapphire_look::wind::{WindState, cycles, fract};
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

use crate::pipelines::Coverage;
use crate::wind::WindSettings;
use crate::{RenderSettings, WindField};

/// The trunk's swing, the branches' and their bob's frequencies, in cycles per second before
/// rounding to whole cycles per period (`foliage.wgsl`'s `WIND_*_HZ`; the gusts' are the field's,
/// `sapphire_look::wind`).
pub(crate) const SWAY_HZ: f32 = 0.31;
pub(crate) const BEND_HZ: f32 = 0.83;
pub(crate) const BOB_HZ: f32 = 1.27;
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
    pub const LOCAL_AIR: &str = "FOLIAGE_LOCAL_AIR";
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
    /// Whether and where plants sway in the wind field's local air (turbulence and volumes, M6
    /// Y1): at each vertex on every tier whose GPU takes volumes, off on Mobile-Low.
    pub local_air: LocalAir,
}

/// Where foliage samples the wind field's local air ([`FoliageSettings::local_air`], the pipeline
/// constant `FOLIAGE_LOCAL_AIR`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum LocalAir {
    /// Not at all: the shader compiles it out, and foliage draws exactly as before the field
    /// (Mobile-Low, and the golden scenes made before it).
    Off,
    /// At each vertex: a draught's edge crosses a plant (every tier whose GPU takes volumes).
    #[default]
    Vertex,
    /// Once a plant, at its root, so a whole plant leans alike: the Y1 row's fallback if its
    /// volumes cost too much. Each vertex still runs the sampler, so this alone saves no GPU time:
    /// it is the picture a sample taken once a plant, outside the vertex stage, would draw.
    Root,
}

impl LocalAir {
    /// The pipeline constant's value: 0 off, 1 at each vertex, 2 at the root.
    pub(crate) fn constant(self) -> f64 {
        match self {
            Self::Off => 0.0,
            Self::Vertex => 1.0,
            Self::Root => 2.0,
        }
    }
}

impl Default for FoliageSettings {
    /// The desktop tiers': every wind level, translucency, alpha-to-coverage whenever MSAA is on,
    /// and local air at each vertex.
    fn default() -> Self {
        Self {
            wind_levels: Self::MAX_WIND_LEVELS,
            alpha_to_coverage: true,
            translucency: true,
            local_air: LocalAir::Vertex,
        }
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

    /// The wind's pipeline constants, which every foliage pass takes: the levels, where the local air
    /// is sampled, if at all, and how many of the field's volumes the tier's GPU readers loop over
    /// (`wind`).
    pub(crate) fn wind_constants(&self, wind: &WindSettings) -> [(&'static str, f64); 3] {
        [
            (override_constant::WIND_LEVELS, f64::from(self.wind_levels.min(Self::MAX_WIND_LEVELS))),
            (override_constant::LOCAL_AIR, self.local_air.constant()),
            wind.constant(),
        ]
    }

    /// The surfaces' tier constants: the wind's and translucency.
    pub(crate) fn constants(&self, wind: &WindSettings) -> [(&'static str, f64); 4] {
        let [levels, local_air, volumes] = self.wind_constants(wind);
        [
            levels,
            local_air,
            volumes,
            (override_constant::TRANSLUCENCY, f64::from(u8::from(self.translucency))),
        ]
    }
}

/// How far the wind `state` moves a vertex at world `position` (before the wind) of a plant rooted
/// at `origin`, with cooked `weights` (sway, bend, flutter, phase), under a material's `response`
/// (sway, bend, flutter, flutter frequency), with `levels` of the wind running: `foliage.wgsl`'s
/// `wind_offset`, operation for operation.
pub(crate) fn offset(
    state: &WindState,
    levels: u32,
    response: [f32; 4],
    weights: [f32; 4],
    position: Vec3,
    origin: Vec3,
) -> Vec3 {
    let strength = state.strength();
    let direction = state.direction();
    let t = state.time[0];
    let gust = state.gust(position);
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
        offset +=
            (direction * branch.sin() + Vec3::new(0.0, 0.4 * bob.cos(), 0.0)) * (response[1] * weights[1]);
    }
    if levels >= 3 {
        let a = TAU * fract(cycles(response[3], t) + phase + fract(position.dot(FLUTTER_SPREAD)));
        let wander = Vec3::new(a.sin(), 0.6 * (2.0 * a + 1.3).sin(), (3.0 * a + 0.4).cos());
        offset += wander * (response[2] * weights[2]);
    }
    offset *= strength * gust;
    droop(offset, position, origin)
}

/// How far the field's `local` air (turbulence and volumes, as
/// [`ResolvedWind::local`](sapphire_look::ResolvedWind::local) samples it) moves the same vertex,
/// beside [`offset`]: the same levels along the air's own horizontal direction at its own speed,
/// without gusts, drooping alike, and exactly nothing where there is none. `foliage.wgsl`'s
/// `wind_local_offset`, operation for operation; kept apart so a frame without local air draws
/// exactly as before the field.
pub(crate) fn local_offset(
    state: &WindState,
    local: Vec3,
    levels: u32,
    response: [f32; 4],
    weights: [f32; 4],
    position: Vec3,
    origin: Vec3,
) -> Vec3 {
    let t = state.time[0];
    let plant = origin.x * PLANT_PHASE[0] + origin.z * PLANT_PHASE[1];
    let phase = weights[3];
    let push = Vec3::new(local.x, 0.0, local.z);
    let speed = push.dot(push).sqrt();
    let mut offset = Vec3::ZERO;
    if levels >= 1 {
        let swing = (TAU * fract(cycles(SWAY_HZ, t) + plant)).sin();
        offset += push * (response[0] * weights[0] * (LEAN + SWING * swing));
    }
    if levels >= 2 {
        let branch = TAU * fract(cycles(BEND_HZ, t) + phase + plant);
        let bob = TAU * fract(cycles(BOB_HZ, t) + phase);
        offset +=
            (push * branch.sin() + Vec3::new(0.0, 0.4 * bob.cos() * speed, 0.0)) * (response[1] * weights[1]);
    }
    if levels >= 3 {
        let a = TAU * fract(cycles(response[3], t) + phase + fract(position.dot(FLUTTER_SPREAD)));
        let wander = Vec3::new(a.sin(), 0.6 * (2.0 * a + 1.3).sin(), (3.0 * a + 0.4).cos());
        offset += wander * (response[2] * weights[2] * speed);
    }
    droop(offset, position, origin)
}

/// Bending, not shearing: what moves sideways drops a little, as a point on a bending stem.
fn droop(mut offset: Vec3, position: Vec3, origin: Vec3) -> Vec3 {
    let height = (position.y - origin.y).max(0.0);
    let sideways = offset.x * offset.x + offset.z * offset.z;
    offset.y -= (sideways / (2.0 * height.max(0.1))).min(sideways.sqrt());
    offset
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
/// meters, at unit strength and without gusts: [`offset`]'s bound. The trunk's lean and swing reach
/// its sway, the branch's swing with its bob √(1 + 0.4²) of its bend, the leaf's wander
/// √(1 + 0.6² + 1) of its flutter, every weight at most 1; the droop toward the root can lengthen
/// their sum by up to the golden ratio (a sideways offset `h` with a vertical one `y` droops by at
/// most `h`, and `h² + (|y| + h)²` peaks at φ² (`h² + y²`)). The field's local air moves the same
/// levels at its own speed, so its reach is this times that speed.
pub(crate) fn wind_reach(response: [f32; 4]) -> f32 {
    const BOB: f32 = 1.077_033; // √(1 + 0.4²)
    const WANDER: f32 = 1.536_229_1; // √(1 + 0.6² + 1)
    const DROOP: f32 = std::f32::consts::GOLDEN_RATIO;
    let [sway, bend, flutter, _] = response.map(f32::abs);
    DROOP * (sway * (LEAN + SWING) + bend * BOB + flutter * WANDER)
}

/// How hard a frame's wind field can push foliage, for its culling margins ([`Self::margin`]): the
/// global wind's strength and strongest gust, and, on a tier that draws local air, the fastest
/// local air anywhere over the volumes its GPU takes ([`WindField::local_reach`]). Taken once a
/// frame. Every plant is padded alike, wherever it stands: padding each only by the volumes it
/// touches would need its position where culling sees only its mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WindPush {
    strength: f32,
    gusts: f32,
    local: f32,
}

impl WindPush {
    pub(crate) fn new(wind: &WindField, settings: &RenderSettings) -> Self {
        let state = WindState::new(&wind.global, 0.0);
        let local = match settings.foliage.local_air {
            LocalAir::Off => 0.0,
            LocalAir::Vertex | LocalAir::Root => wind.local_reach(settings.wind.volumes as usize),
        };
        Self { strength: state.strength(), gusts: 1.0 + state.time[1], local }
    }

    /// How much farther than its cooked bounds this wind can carry foliage whose materials reach
    /// `reach` meters ([`wind_reach`]): times the wind's strength and its strongest gust
    /// (`1 + gustiness`), and times the fastest local air the tier draws. Shadow casters' culling and the main
    /// view's pad foliage's bounds by it (the [`shadows`](crate::shadows) module).
    pub(crate) fn margin(&self, reach: f32) -> f32 {
        reach * self.strength * self.gusts + reach * self.local
    }
}

/// How far `wind` at `time_seconds` moves a foliage vertex, in meters, world space, as a renderer
/// with `settings` draws it: the foliage shader's displacement on the CPU (`foliage.wgsl`'s
/// `wind_offset`, with the tier's wind levels, its GPU readers' volumes and where the local air is
/// sampled), for tests and for gameplay that must follow what the GPU draws. `position` is the
/// vertex's world position before the wind, `origin` its instance's (the plant's root, its
/// transform's translation), `vertex` its cooked foliage data as the stream holds it
/// ([`FoliageVertex::decode`]) and `response` its material's
/// [`Wind`](sapphire_assets::material::Wind). The GPU's `sin` differs from the CPU's in the last
/// bits, so expect agreement to a few micrometers per meter of response.
pub fn wind_offset(
    wind: &WindField,
    time_seconds: f64,
    settings: &RenderSettings,
    response: &WindResponse,
    vertex: &FoliageVertex,
    position: Vec3,
    origin: Vec3,
) -> Vec3 {
    let response = [response.sway, response.bend, response.flutter, response.flutter_frequency];
    let weights = [vertex.sway, vertex.bend, vertex.flutter, vertex.phase];
    let resolved = wind.resolve(time_seconds);
    let (levels, volumes) = (settings.foliage.wind_levels, settings.wind.volumes as usize);
    let mut moved = offset(&resolved.state, levels, response, weights, position, origin);
    // As the shader branches: only a tier that takes local air, on a frame with some, adds it.
    let local_air = settings.foliage.local_air;
    if local_air != LocalAir::Off && resolved.state.has_local(volumes) {
        let at = if local_air == LocalAir::Root { origin } else { position };
        let local = resolved.local(at, volumes);
        moved += local_offset(&resolved.state, local, levels, response, weights, position, origin);
    }
    moved
}

#[cfg(test)]
mod tests {
    use sapphire_look::WindVolume;
    use sapphire_look::wind::WIND_PERIOD;

    use super::*;
    use crate::Wind;

    fn breeze() -> Wind {
        Wind { direction: Vec3::new(3.0, 5.0, 4.0), strength: 1.0, gustiness: 0.3 }
    }

    const RESPONSE: [f32; 4] = [0.2, 0.15, 0.03, 2.3];
    const TIP: [f32; 4] = [1.0, 0.8, 1.0, 0.37];

    /// The global wind alone, as foliage swayed before the field.
    fn global(wind: &Wind, seconds: f64, levels: u32, weights: [f32; 4], at: Vec3, origin: Vec3) -> Vec3 {
        offset(&WindState::new(wind, seconds), levels, RESPONSE, weights, at, origin)
    }

    #[test]
    fn calm_or_still_tiers_and_roots_never_move() {
        let at = Vec3::new(4.0, 2.0, -3.0);
        assert_eq!(global(&Wind::default(), 17.0, 3, TIP, at, Vec3::ZERO), Vec3::ZERO);
        assert_eq!(global(&breeze(), 17.0, 0, TIP, at, Vec3::ZERO), Vec3::ZERO, "no levels run");
        assert_eq!(global(&breeze(), 17.0, 3, [0.0, 0.0, 0.0, 0.5], at, Vec3::ZERO), Vec3::ZERO, "a root");
        assert_ne!(global(&breeze(), 17.0, 3, TIP, at, Vec3::ZERO), Vec3::ZERO);
        // Local air alone sways a plant in calm, and not with no levels.
        let calm = WindState::new(&Wind::default(), 17.0);
        assert_ne!(local_offset(&calm, Vec3::X, 3, RESPONSE, TIP, at, Vec3::ZERO), Vec3::ZERO);
        assert_eq!(local_offset(&calm, Vec3::X, 0, RESPONSE, TIP, at, Vec3::ZERO), Vec3::ZERO);
    }

    #[test]
    fn each_level_adds_its_own_motion_within_its_reach() {
        let at = Vec3::new(1.0, 3.0, 0.5);
        for step in 0..200 {
            let seconds = f64::from(step) * 0.137;
            let one = global(&breeze(), seconds, 1, TIP, at, Vec3::ZERO);
            let two = global(&breeze(), seconds, 2, TIP, at, Vec3::ZERO);
            let three = global(&breeze(), seconds, 3, TIP, at, Vec3::ZERO);
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
    fn local_air_adds_exactly_nothing_where_there_is_none() {
        // Turbulence 0 and no volume reaching the plant, on a tier that takes local air: the
        // field's offset is the global wind's to the bit.
        let mut field = WindField::new(breeze());
        field.push(WindVolume::vortex(Vec3::new(50.0, 0.0, 50.0), 5.0, Vec3::Y, 4.0, 1.0));
        let foliage = FoliageSettings { local_air: LocalAir::Vertex, ..FoliageSettings::default() };
        let settings = RenderSettings { foliage, ..RenderSettings::default() };
        let response = WindResponse { sway: 0.2, bend: 0.15, flutter: 0.03, flutter_frequency: 2.3 };
        let vertex = FoliageVertex { sway: 1.0, bend: 0.8, flutter: 1.0, phase: 0.37, gradient: 1.0 };
        for step in 0..300 {
            let seconds = f64::from(step) * 0.29;
            let at = Vec3::new(step as f32 * 0.1 - 9.0, 1.0 + (step % 5) as f32, -(step as f32) * 0.07);
            let origin = Vec3::new(at.x, 0.0, at.z);
            let drawn = wind_offset(&field, seconds, &settings, &response, &vertex, at, origin);
            assert_eq!(drawn, global(&breeze(), seconds, 3, TIP, at, origin), "at {at}");
        }
    }

    #[test]
    fn local_air_leans_a_plant_its_own_way_sampled_where_the_tier_says() {
        // A draught blowing +x over a plant in calm leans it +x; sampled at its root, every vertex
        // takes the root's air.
        let root = Vec3::new(2.0, 0.0, 2.0);
        let mut field = WindField::CALM;
        field.push(WindVolume::directional(root, 1.5, Vec3::X * 2.0).with_falloff(1.0));
        let response = WindResponse { sway: 0.2, bend: 0.0, flutter: 0.0, flutter_frequency: 1.0 };
        let trunk = FoliageVertex { sway: 1.0, bend: 0.0, flutter: 0.0, phase: 0.0, gradient: 1.0 };
        let tip = root + Vec3::Y * 1.2;
        let taking = |local_air| RenderSettings {
            foliage: FoliageSettings { local_air, ..FoliageSettings::default() },
            ..RenderSettings::default()
        };
        let (per_vertex, at_origin) = (taking(LocalAir::Vertex), taking(LocalAir::Root));
        let leaned = wind_offset(&field, 3.0, &at_origin, &response, &trunk, tip, root);
        // At the root the draught is whole; at the tip, 1.2 m up, it has faded.
        assert!(leaned.x > 0.07 && leaned.z.abs() < 1e-6, "{leaned}");
        let sampled = wind_offset(&field, 3.0, &per_vertex, &response, &trunk, tip, root);
        assert!(sampled.x > 0.0 && sampled.x < leaned.x, "{sampled} against {leaned}");
        // A tier whose GPU readers take no volumes draws none.
        let none =
            RenderSettings { wind: WindSettings { volumes: 0, ..WindSettings::default() }, ..per_vertex };
        assert_eq!(wind_offset(&field, 3.0, &none, &response, &trunk, tip, root), Vec3::ZERO);
        // And a tier that takes no local air (Mobile-Low, an older golden's pin) sways in the
        // global wind alone.
        let off = taking(LocalAir::Off);
        assert_eq!(wind_offset(&field, 3.0, &off, &response, &trunk, tip, root), Vec3::ZERO);
    }

    #[test]
    fn the_wind_is_continuous_across_the_period_s_wrap() {
        let at = Vec3::new(-7.0, 1.5, 2.0);
        let offset = |seconds: f64| global(&breeze(), seconds, 3, TIP, at, Vec3::new(-7.0, 0.0, 2.0));
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
        // Every level at full weight, a gale with full gusts, turbulence and a vortex, over many
        // instants, places and phases: the offset stays within the margin culling pads foliage
        // by, and comes within a fifth of it (the bound isn't loose).
        let gale = Wind { direction: Vec3::new(1.0, 0.0, -2.0), strength: 2.5, gustiness: 1.0 };
        let mut field = WindField::new(gale);
        field.turbulence = 0.5;
        field.push(WindVolume::vortex(Vec3::new(0.0, 1.0, 0.0), 6.0, Vec3::Y, 3.0, 1.0));
        let settings = RenderSettings::default();
        assert_eq!(settings.foliage.local_air, LocalAir::Vertex, "the desktops draw local air");
        let push = WindPush::new(&field, &settings);
        let margin = push.margin(wind_reach(RESPONSE));
        let mut farthest = 0.0f32;
        for step in 0..4000 {
            let seconds = f64::from(step) * 0.0173;
            let phase = (step % 97) as f32 / 97.0;
            let at =
                Vec3::new((step % 13) as f32 - 6.0, 0.05 + (step % 7) as f32 * 0.4, (step % 11) as f32 - 5.0);
            let origin = Vec3::new(at.x, 0.0, at.z);
            let resolved = field.resolve(seconds);
            let local = resolved.local(at, settings.wind.volumes as usize);
            let weights = [1.0, 1.0, 1.0, phase];
            let offset = offset(&resolved.state, 3, RESPONSE, weights, at, origin)
                + local_offset(&resolved.state, local, 3, RESPONSE, weights, at, origin);
            farthest = farthest.max(offset.length());
        }
        assert!(farthest <= margin, "{farthest} past {margin}");
        assert!(farthest > margin * 0.2, "{farthest} against {margin}");
        // A tier that draws no local air pads by the global wind alone, as does one whose GPU
        // takes no volumes by turbulence alone; a GPU that takes one volume pads by it, not by
        // the field's others.
        let global = WindPush::new(&WindField::new(gale), &settings);
        let off = RenderSettings {
            foliage: FoliageSettings { local_air: LocalAir::Off, ..settings.foliage },
            ..settings
        };
        assert_eq!(WindPush::new(&field, &off), global);
        let none = RenderSettings { wind: WindSettings { volumes: 0, ..settings.wind }, ..settings };
        assert_eq!(WindPush::new(&field, &none).local, field.local_reach(0));
        field.push(WindVolume::burst(Vec3::new(50.0, 0.0, 0.0), 3.0, 9.0));
        assert_eq!(WindPush::new(&field, &settings).local, field.local_reach(2));
        let one = RenderSettings { wind: WindSettings { volumes: 1, ..settings.wind }, ..settings };
        assert_eq!(WindPush::new(&field, &one), push, "the burst is past what the GPU takes");
        // Calm reaches nothing; the breeze reaches its reach times its gusts, and in the same
        // order of operations as before the field.
        assert_eq!(WindPush::new(&WindField::CALM, &settings).margin(1.0), 0.0);
        let breeze = WindPush::new(&WindField::new(breeze()), &settings);
        assert_eq!(breeze.margin(0.7), 0.7 * 1.0 * 1.3);
    }

    #[test]
    fn the_public_mirror_reads_materials_and_cooked_vertices() {
        let response = WindResponse { sway: 0.2, bend: 0.15, flutter: 0.03, flutter_frequency: 2.3 };
        let vertex = FoliageVertex { sway: 1.0, bend: 0.8, flutter: 1.0, phase: 0.37, gradient: 1.0 };
        let (at, origin) = (Vec3::new(1.0, 3.0, 0.5), Vec3::ZERO);
        let expected = global(&breeze(), 9.0, 3, TIP, at, origin);
        let field = WindField::new(breeze());
        assert_eq!(
            wind_offset(&field, 9.0, &RenderSettings::default(), &response, &vertex, at, origin),
            expected
        );
    }

    #[test]
    fn settings_default_to_the_desktop_s_and_validate() {
        let settings = FoliageSettings::default();
        assert_eq!(settings.wind_levels, 3);
        assert_eq!(settings.local_air, LocalAir::Vertex);
        assert!(settings.covers(4) && !settings.covers(1), "alpha-to-coverage needs MSAA");
        let phone = WindSettings { volumes: 4, cpu_volumes: 4 };
        assert_eq!(
            settings.constants(&phone),
            [
                ("FOLIAGE_WIND_LEVELS", 3.0),
                ("FOLIAGE_LOCAL_AIR", 1.0),
                ("WIND_VOLUMES", 4.0),
                ("FOLIAGE_TRANSLUCENCY", 1.0)
            ]
        );
        assert!(settings.validate().is_ok());
        let wrong = FoliageSettings { wind_levels: 4, ..settings };
        assert_eq!(wrong.validate(), Err(("foliage.wind_levels", "must be within 0..=3")));
    }
}
