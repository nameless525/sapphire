//! Culling for every view (spec §5.12 "Scene and draws": review RA-O01, then M5 D3).
//!
//! Each recorded frame decides, once per instance, whether the view it stands in draws it (its
//! [`Verdict`]): the main view for the world's instances, an inset's camera for an inset's (M3 U6).
//! The main pass, both prepasses, the debug views' replays of the main pass and each inset's
//! passes read the one verdict, so every view of a camera agrees and the prepass culls exactly as
//! the main pass does. The shadow views cull their casters against their own volumes in the
//! shadow plan (`shadows::plan`), toward the light too, by the same bounds.
//!
//! # The frustum test is exact
//!
//! What it leaves out reaches no sample, so the frame's pixels are the same bit for bit with it on
//! or off ([`CullingMode::FrustumOnly`] against [`CullingMode::Off`]). It is conservative:
//!
//! - **Bounds:** an instance's mesh's sphere under its transform ([`Sphere::transformed`], which
//!   holds the mesh under any affine transform, shear included), or a skinned or morphed
//!   instance's posed bounds (`InstanceSlots::world_bounds`, the `bounds` module's, which hold the
//!   pending pose and the last drawn one).
//! - **Padding:** the frustum's sides move out by [`PAD_PIXELS`] and by the frame's temporal AA
//!   jitter, and the spheres grow by a share of their size for rounding (the cooked positions'
//!   quantization, the GPU's transforms), so a triangle that could cover a sample at the edge, or
//!   a line of the wireframe view, is never left out.
//! - **Foliage** grows by the frame's wind margin (`foliage::wind_margin`), as shadow casters do.
//! - **Fog cards** that face the camera turn about their instance's origin keeping their scale
//!   (`fog_card.wgsl`): their sphere is the origin's, of the mesh's farthest reach from it.
//! - **Outline hulls** reach past their surface by a width in screen pixels, or close up in
//!   meters (`outline.wgsl`, the `outlines` module's [`CharacterReach`]): an instance whose hull the
//!   frame draws grows by the meters and is tested against sides moved out by the pixels, without
//!   the near plane (a hull's Z offset moves it along the view ray). An eye's highlight drifts by
//!   its material's meters too. So an outline never pops at the frame's edge.
//! - **Not tested:** instances with a projective transform, and bounds that aren't finite.
//!
//! # Distance and size, by tier (M5 D3)
//!
//! Past the tier's distance, or smaller than its least size in output pixels, an instance is left
//! out ([`CullingThresholds`], resolved from the tier before the renderer, rule R4). These change
//! pixels, so their thresholds are chosen from the district walk by what they save and what they
//! cost in a FLIP comparison (m5-plan D3's as-built note). Each holds with hysteresis: an instance
//! culled by distance comes back within [`DISTANCE_RETURN`] of it, one culled by size at
//! [`SIZE_RETURN`] times it, so a camera held on a threshold doesn't flicker. Size is in output
//! pixels, not the render scale's, so dynamic resolution never changes what draws. Never culled by
//! either:
//!
//! - the named characters (the party, the encounter's enemies, anyone a timeline casts: every
//!   character whose body casts as [`ShadowRole::Named`]), and characters without a body;
//! - whatever overlaps a named character's bounds (what it holds: a weapon, a prop in hand);
//! - everything in an inset, which frames it;
//! - fog cards, whose size is the atmosphere's, not an object's;
//! - the sky, decals, effects and the interface, which aren't instances.
//!
//! A crowd's ambient characters ([`ShadowRole::Ambient`]) cull by distance and size as one, by the
//! sphere around every part of the character, so a character's parts never part.
//!
//! # Parallel, and allocation-free
//!
//! The verdicts are worked out over chunks of instance slots on the job system
//! ([`sapphire_jobs::par_for_each_chunk_mut`]): each chunk writes its own slots' verdicts and adds
//! its counts once, so the result doesn't depend on the threads or the order chunks run in. A frame
//! of one chunk runs inline. The verdicts' vectors keep their capacity, so a warm frame allocates
//! nothing here; the job system allocates a few times per parallel call (its crate docs).
//!
//! [`CullingMode`] turns the tests off for comparisons: `FrustumOnly` keeps the exact test, `Off`
//! draws everything.
//!
//! # Levels of detail (M5 D4)
//!
//! The main view's verdict comes with the level of detail the main view's family draws the
//! instance at (the `levels` module: its error projected at its bounds' nearest point, in output
//! pixels, with hysteresis from the level drawn last frame), worked out in the same chunks.
//! [`Culled::level`] gives it to the lists; [`DrawnBy`] says what level each view drew, a frame
//! late.
//!
//! # Characters' levels (M5 D5)
//!
//! A character the stage lets draw coarser ([`CharacterDetail::ByError`] on its body) gives its
//! parts its body's group, as an ambient crowd's culling does. Before the chunks, each such
//! character chooses one level from the sphere around its parts, the coarsest every part with
//! levels is good enough at, with the hysteresis held from the coarsest level the main view drew
//! a part at last frame; each part then draws the coarsest of its own levels within its
//! character's ([`part_level`]). [`ViewCull::drawn`] gives skinning the finest level any view drew
//! an instance at, whose prefix it skins. Every other character draws level 0.

use std::sync::atomic::{AtomicU32, Ordering};

use bevy_reflect::Reflect;
use glam::{Mat4, Vec3, Vec4, Vec4Swizzles};
use serde::{Deserialize, Serialize};

use crate::bounds::{Sphere, max_stretch};
use crate::characters::CharacterTable;
use crate::inset::MAX_INSETS;
use crate::levels::{self, LevelSettings, LevelStats, LevelsMode};
use crate::mesh_pool::MeshRecord;
use crate::scene::{DrawList, InstanceSlots};
use crate::{CameraData, CharacterDetail, ShadowRole};

/// Pixels the frustum's sides move out by, beyond temporal AA's jitter: whole pixels of margin
/// for the samples a triangle at the edge could cover under MSAA, and for the debug views' lines.
pub(crate) const PAD_PIXELS: f32 = 2.0;

/// The share of a sphere's radius it grows by for rounding: the cooked positions' quantization
/// (half a unorm16 step across the mesh's extent, under 2e-5 of the radius) and the GPU's
/// transforms.
const RADIUS_SLACK: f32 = 1e-4;

/// The share of a plane distance's terms it may be off by in f32 and still be rounding.
const DISTANCE_SLACK: f32 = 1e-5;

/// An instance culled by distance comes back within this share of the distance.
pub(crate) const DISTANCE_RETURN: f32 = 0.9;

/// An instance culled by size comes back at this many times the least size.
pub(crate) const SIZE_RETURN: f32 = 1.25;

/// Instance slots per chunk of the parallel verdicts: a district's few thousand instances make a
/// few chunks, a test scene's one.
const CHUNK: usize = 2048;

/// Views a frame culls for: the main view, then each inset's.
pub(crate) const CULL_VIEWS: usize = 1 + MAX_INSETS;

/// What view culling does: [`RenderSettings::view_culling`](crate::RenderSettings::view_culling).
/// Only tests and the bench set anything but `On`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum CullingMode {
    /// Every test: each view's frustum, and the tier's distance and size.
    #[default]
    On,
    /// The frustum tests alone, which change no pixel.
    FrustumOnly,
    /// Nothing culled: every instance drawn by every view it stands in.
    Off,
}

impl CullingMode {
    /// Whether the views' frusta cull.
    pub fn frustum(self) -> bool {
        self != Self::Off
    }

    /// Whether the tier's distance and size cull.
    pub fn thresholds(self) -> bool {
        self == Self::On
    }
}

/// A tier's distance and size culling (M5 D3; `tiers.ron`,
/// [`RenderSettings::culling`](crate::RenderSettings::culling)): resolved before the renderer,
/// which reads only these numbers.
/// Zero turns a test off.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default, deny_unknown_fields)]
pub struct CullingThresholds {
    /// Meters from the camera to an instance's nearest point past which it isn't drawn, by any
    /// view: 0 for none.
    pub distance: f32,
    /// The least size an instance's sphere shows across, in output pixels, below which the main
    /// view doesn't draw it: 0 for none.
    pub least_pixels: f32,
    /// The least size a caster's sphere covers in a cascade's own texels, below which that cascade
    /// doesn't draw it: 0 for none.
    pub least_texels: f32,
}

impl CullingThresholds {
    /// No distance or size culling: [`RenderSettings`](crate::RenderSettings)'s default, which the
    /// goldens and most GPU tests draw with.
    pub const NONE: Self = Self { distance: 0.0, least_pixels: 0.0, least_texels: 0.0 };

    /// Checks each threshold is finite and not negative, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        for (field, value) in [
            ("culling.distance", self.distance),
            ("culling.least_pixels", self.least_pixels),
            ("culling.least_texels", self.least_texels),
        ] {
            if !(value.is_finite() && value >= 0.0) {
                return Err((field, "must be finite and at least 0"));
            }
        }
        Ok(())
    }
}

impl Default for CullingThresholds {
    fn default() -> Self {
        Self::NONE
    }
}

/// Settings compare whole (`RenderSettings` is `Eq`): the thresholds are finite where they're used
/// ([`CullingThresholds::validate`]), where `==` is an equivalence.
impl Eq for CullingThresholds {}

/// Why a view doesn't draw an instance, or that it does: the first test that left it out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Verdict {
    #[default]
    Drawn,
    Frustum,
    Distance,
    Size,
}

/// View culling counters from the last recorded frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewCullingStats {
    /// Instances tested, every view's: those with draws and a transform that isn't projective. 0
    /// with culling off.
    pub tested: u32,
    /// Of those, the ones wholly outside their view's frustum, whose draws its passes skipped.
    pub culled: u32,
    /// Of those, the ones past the tier's distance ([`CullingThresholds::distance`]).
    pub culled_by_distance: u32,
    /// Of those, the ones below the tier's least size ([`CullingThresholds::least_pixels`]).
    pub culled_by_size: u32,
    /// Surface draws the views drew: [`RenderStats::draws`](crate::RenderStats::draws) less the
    /// culled instances'.
    pub drawn: u32,
}

/// Which views drew an instance in the last recorded frame (M5 D3), and at what level of detail
/// (M5 D4), for what animates it to read in the next: [`Renderer::drawn_by`](crate::Renderer::drawn_by).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawnBy {
    /// The main view: the main pass and the prepasses.
    pub view: bool,
    /// A shadow cascade or a character map drawn that frame.
    pub shadow: bool,
    /// The inset it stands in.
    pub inset: bool,
    /// The level the main view drew it at: 0 if it didn't.
    pub level: u8,
    /// The finest level any view drew it at, the main view's, an inset's or a cascade's: what a
    /// prefix of its vertices must hold for every view (m5-plan D5's skinning). 0 if none drew it.
    pub finest: u8,
}

impl DrawnBy {
    /// Whether any view drew it.
    pub fn any(&self) -> bool {
        self.view || self.shadow || self.inset
    }

    /// Whether it was drawn only as a shadow.
    pub fn only_shadow(&self) -> bool {
        self.shadow && !self.view && !self.inset
    }
}

/// How far an instance's outline hulls and drifting eye highlights reach past its bounds this
/// frame (the `outlines` module's, from its materials): what the frustum test grows it by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CharacterReach {
    /// The frame draws a hull of it: test without the near plane.
    pub hulled: bool,
    /// A hull's widest line, in pixels of a 1080-row target.
    pub pixels: f32,
    /// A hull's widest line close up, where it holds its width in the world, in meters.
    pub meters: f32,
    /// An eye highlight's widest drift, in object-space meters.
    pub drift: f32,
}

/// A static or deformed instance with draws: what the views test, in slot order (the draw list's,
/// built with it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CullCandidate {
    pub slot: u32,
    pub mesh: crate::MeshHandle,
    /// Its surface draws.
    pub draws: u32,
    /// Some of its draws are foliage's: it grows by the wind's margin.
    pub foliage: bool,
    /// Some of its draws are fog cards': camera-facing ones turn about its origin.
    pub fog_card: bool,
    /// It draws at level 0 in every view (M5 D4, the `levels` module): it is skinned or morphed,
    /// shades as a character, is foliage or a fog card.
    pub whole: bool,
}

/// One view's frustum, as planes facing in: left, right, bottom, top and near. The far plane is at
/// infinity (`CameraData::proj`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frustum {
    /// xyz the unit normal, w the distance along it from the origin, negated: a point is inside
    /// a plane where `dot(xyz, p) + w >= 0`.
    planes: [Vec4; 5],
}

impl Frustum {
    /// The frustum of `clip_from_world` (unjittered) for a `size` image, its sides moved out by
    /// `pad` pixels.
    pub(crate) fn new(clip_from_world: Mat4, (width, height): (u32, u32), pad: f32) -> Self {
        let [x, y, z, w] = [0, 1, 2, 3].map(|row| clip_from_world.row(row));
        // A pixel is 2 / size of normalized device coordinates: the sides at ±(1 + 2 pad / size).
        let sx = 1.0 + 2.0 * pad / width.max(1) as f32;
        let sy = 1.0 + 2.0 * pad / height.max(1) as f32;
        let planes = [w * sx + x, w * sx - x, w * sy + y, w * sy - y, w - z].map(|plane| {
            let length = plane.xyz().length();
            // A degenerate camera's plane excludes nothing.
            if length > 0.0 && length.is_finite() { plane / length } else { Vec4::W }
        });
        Self { planes }
    }

    /// Whether `sphere` lies wholly outside: never when it isn't finite.
    pub(crate) fn excludes(&self, sphere: &Sphere) -> bool {
        Self::outside(&self.planes, sphere)
    }

    /// Whether `sphere` lies wholly outside the four sides, whatever its depth: what a hull, which
    /// its Z offset moves along the view ray, is tested by. The sides alone exclude what is behind
    /// the camera.
    pub(crate) fn excludes_beside(&self, sphere: &Sphere) -> bool {
        Self::outside(&self.planes[..4], sphere)
    }

    fn outside(planes: &[Vec4], sphere: &Sphere) -> bool {
        let (center, radius) = (sphere.center, sphere.radius);
        let reach = radius * (1.0 + RADIUS_SLACK);
        planes.iter().any(|plane| {
            let along = plane.xyz().dot(center);
            let slack = DISTANCE_SLACK * (along.abs() + plane.w.abs());
            // False for NaN: a sphere that isn't finite stays.
            along + plane.w + reach + slack < 0.0
        })
    }
}

/// A view a frame culls for: its frustum, padded, and what its size test measures by.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CullView {
    frustum: Frustum,
    /// The frustum padded for the frame's widest hull too.
    hulled: Frustum,
    position: Vec3,
    /// Output pixels a sphere one meter across shows at one meter: the projection's vertical focal
    /// length × half the output's height. An orthographic view's at any distance.
    pixels_per_meter: f32,
    near: f32,
    /// The projection is orthographic: sizes don't shrink with distance.
    orthographic: bool,
}

impl CullView {
    /// The view of `camera` drawn at `size` (the scene's pixels, which pad and lines are in) onto
    /// `output_height` rows, its sides moved out by `pad` pixels, and by `hull_pixels` (of a
    /// 1080-row target) more for hulled instances.
    pub(crate) fn new(
        camera: &CameraData,
        size: (u32, u32),
        output_height: u32,
        pad: f32,
        hull_pixels: f32,
    ) -> Self {
        let clip_from_world = camera.proj * camera.view;
        // A hull's line is `pixels × rows / 1080` of the scene's pixels wide, and at least the
        // narrowest line its samples draw (`whole_line`, under a pixel), within the padding.
        let hull = hull_pixels * size.1 as f32 / 1080.0;
        let focal = camera.proj.y_axis.y.abs();
        Self {
            frustum: Frustum::new(clip_from_world, size, pad),
            hulled: Frustum::new(clip_from_world, size, pad + hull),
            position: camera.position,
            pixels_per_meter: focal * output_height as f32 * 0.5,
            near: camera.proj.w_axis.z.abs().max(1e-4),
            orthographic: camera.proj.z_axis.w == 0.0,
        }
    }

    /// The pixels `sphere` shows across at most, as far as it is.
    fn pixels(&self, sphere: &Sphere) -> f32 {
        if self.orthographic {
            return 2.0 * sphere.radius * self.pixels_per_meter;
        }
        let distance = (sphere.center.distance(self.position) - sphere.radius).max(self.near);
        2.0 * sphere.radius * self.pixels_per_meter / distance
    }

    /// The meters an output pixel covers at `sphere`'s nearest point, at least the near plane's
    /// distance away: the least it covers anywhere on the sphere.
    pub(crate) fn meters_per_pixel(&self, sphere: &Sphere) -> f32 {
        if self.orthographic {
            return 1.0 / self.pixels_per_meter;
        }
        let distance = (sphere.center.distance(self.position) - sphere.radius).max(self.near);
        distance / self.pixels_per_meter
    }
}

/// One instance slot's culling this frame, and what carries to the next.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SlotCull {
    /// Whether the view it stands in draws it.
    pub verdict: Verdict,
    /// Past the tier's distance (with hysteresis): no view draws it.
    pub far: bool,
    /// Below the least size in the main view (with hysteresis).
    small: bool,
    /// Never culled by distance or size.
    pub kept: bool,
    /// Its instance is new in this frame's slot: no hysteresis carries.
    pub fresh: bool,
    /// Its ambient character's group, if it is one's part ([`NO_GROUP`] if not).
    group: u32,
    /// The views that drew it ([`DrawnBy`] as bits).
    drawn_by: u8,
    /// The level of detail the main view's family draws it at this frame (M5 D4): what it drew
    /// last frame until the frame's verdicts are worked out, which its hysteresis starts from.
    level: u8,
    /// It draws at level 0 in every view (the `levels` module): a character's part unless its
    /// character chooses by error (M5 D5), foliage, a fog card, anything else skinned or morphed.
    whole: bool,
    /// The finest level any view drew it at last frame ([`DrawnBy::finest`]).
    finest: u8,
    /// The character it is a part of, if that character chooses its level by error (M5 D5,
    /// [`CharacterDetail::ByError`]): its body's group, whose level every part takes ([`NO_GROUP`]
    /// if not).
    character: u32,
}

#[cfg(test)]
impl SlotCull {
    /// Slots of these verdicts, not far, not kept.
    pub(crate) fn of(verdicts: &[Verdict]) -> Vec<SlotCull> {
        let cull = |&verdict| SlotCull {
            verdict,
            far: verdict == Verdict::Distance,
            group: NO_GROUP,
            character: NO_GROUP,
            ..SlotCull::default()
        };
        verdicts.iter().map(cull).collect()
    }

    /// A drawn slot that draws level 0 alone (M5 D4).
    pub(crate) fn whole() -> SlotCull {
        SlotCull { whole: true, group: NO_GROUP, character: NO_GROUP, ..SlotCull::default() }
    }

    /// Drawn slots at these levels of detail (M5 D4).
    pub(crate) fn at_levels(levels: &[u8]) -> Vec<SlotCull> {
        let cull = |&level| SlotCull { level, group: NO_GROUP, character: NO_GROUP, ..SlotCull::default() };
        levels.iter().map(cull).collect()
    }
}

/// No group: [`SlotCull::group`] of an instance that isn't an ambient character's part.
const NO_GROUP: u32 = u32::MAX;

const BY_VIEW: u8 = 1;
const BY_SHADOW: u8 = 2;
const BY_INSET: u8 = 4;

/// A character with a body that casts, by its body's group ([`CharacterTable::bodies`]): an
/// ambient one is culled by distance and size as one, and one that chooses its level by error
/// (M5 D5) draws all its parts at one level.
#[derive(Clone, Copy, Debug)]
struct Group {
    /// Around every part it lists.
    sphere: Option<Sphere>,
    far: bool,
    small: bool,
    /// Its state came from a part it had last frame.
    known: bool,
    /// The level of detail the main view's family draws its parts at this frame, before each
    /// part takes the coarsest its mesh has (M5 D5): 0 unless it chooses by error.
    level: u8,
}

/// Per instance slot, the frame's culling, for the passes that read it: copied into each, as the
/// mesh draws are.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Culled<'a> {
    slots: &'a [SlotCull],
}

impl Culled<'_> {
    /// No culling: every slot drawn and kept.
    #[cfg(test)]
    pub(crate) const NONE: Culled<'static> = Culled { slots: &[] };

    /// The culling of `slots`, for tests that set verdicts by hand ([`SlotCull::of`]).
    #[cfg(test)]
    pub(crate) fn new(slots: &[SlotCull]) -> Culled<'_> {
        Culled { slots }
    }

    /// The verdict of the view `slot` stands in: drawn past the slots it has.
    pub(crate) fn verdict(&self, slot: u32) -> Verdict {
        self.slots.get(slot as usize).map_or(Verdict::Drawn, |cull| cull.verdict)
    }

    /// Whether `slot` is past the tier's distance, so no view draws it.
    pub(crate) fn far(&self, slot: u32) -> bool {
        self.slots.get(slot as usize).is_some_and(|cull| cull.far)
    }

    /// Whether `slot` is never culled by distance or size.
    pub(crate) fn kept(&self, slot: u32) -> bool {
        self.slots.get(slot as usize).is_none_or(|cull| cull.kept)
    }

    /// Whether `slot`'s instance is new this frame: no hysteresis carries to it.
    pub(crate) fn fresh(&self, slot: u32) -> bool {
        self.slots.get(slot as usize).is_none_or(|cull| cull.fresh)
    }

    /// The level of detail the main view's family draws `slot` at (M5 D4): 0 past the slots.
    pub(crate) fn level(&self, slot: u32) -> u8 {
        self.slots.get(slot as usize).map_or(0, |cull| cull.level)
    }

    /// Whether `slot` draws at level 0 in every view (the `levels` module): true past the slots.
    pub(crate) fn whole(&self, slot: u32) -> bool {
        self.slots.get(slot as usize).is_none_or(|cull| cull.whole)
    }
}

/// What a frame's verdicts are worked out from.
pub(crate) struct CullInputs<'a, M> {
    pub list: &'a DrawList,
    pub slots: &'a InstanceSlots,
    pub meshes: &'a [MeshRecord],
    /// How far the wind carries a foliage mesh's vertices past its bounds.
    pub margin: M,
    /// Per instance slot, its hulls' and eyes' reach ([`CharacterReach`]).
    pub reach: &'a [CharacterReach],
    /// Per instance slot, the view it stands in: 0 the main view's, `n` inset `n − 1`'s.
    pub stands: &'a [u16],
    /// The main view, then each inset by index; `None` for one the frame doesn't show.
    pub views: [Option<CullView>; CULL_VIEWS],
    /// The frame's levels of detail (M5 D4): their mode, and the bias with the rung's.
    pub levels: LevelSettings,
    /// The camera cut to a new shot this frame (a jump, a zone trip's arrival, a tier change): every
    /// instance chooses its level afresh.
    pub cut: bool,
}

/// Every instance's culling, kept across frames (see the module docs).
#[derive(Debug, Default)]
pub(crate) struct ViewCull {
    slots: Vec<SlotCull>,
    groups: Vec<Group>,
    /// The named characters' bounds: what overlaps them is kept.
    named: Vec<Sphere>,
    /// The slots of the parts of characters that choose their level by error (M5 D5), in the
    /// order the frame lists them.
    parts: Vec<u32>,
    /// Per group, the level its parts draw at this frame (M5 D5): [`Group::level`], for the chunks.
    group_levels: Vec<u8>,
    /// Per group, below the least size this frame.
    small_groups: Vec<bool>,
    mode: CullingMode,
    thresholds: CullingThresholds,
    stats: ViewCullingStats,
    level_stats: LevelStats,
}

impl ViewCull {
    /// Starts the frame: follows the instances `fresh` names (new in their slots), finds what is
    /// never culled by distance or size and the ambient characters' groups, and decides which
    /// instances are past the tier's distance from `camera` (the main view's, if the frame has
    /// one), which the shadow plan reads too. `characters` is the frame's.
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs, each read once")]
    pub(crate) fn prepare(
        &mut self,
        mode: CullingMode,
        thresholds: CullingThresholds,
        camera: Option<&CameraData>,
        list: &DrawList,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
        characters: CharacterTable<'_>,
        fresh: impl Iterator<Item = u32>,
    ) {
        profiling::scope!("ViewCull::prepare");
        self.mode = mode;
        self.thresholds = thresholds;
        let blank = SlotCull { fresh: true, group: NO_GROUP, character: NO_GROUP, ..SlotCull::default() };
        self.slots.resize(slots.len(), blank);
        for cull in &mut self.slots {
            cull.fresh = false;
        }
        for slot in fresh {
            if let Some(cull) = self.slots.get_mut(slot as usize) {
                *cull = blank;
            }
        }
        // What is kept: fog cards, named characters and characters without a body. Ambient
        // characters join their group. An inset's instances are kept by the record, which knows
        // this frame's insets.
        for cull in &mut self.slots {
            (cull.kept, cull.group, cull.whole, cull.character) = (false, NO_GROUP, false, NO_GROUP);
        }
        for candidate in &list.cullable {
            let cull = &mut self.slots[candidate.slot as usize];
            cull.kept |= candidate.fog_card;
            // What draws level 0 alone (the `levels` module), and the listed characters, below.
            cull.whole = candidate.whole;
        }
        self.named.clear();
        self.parts.clear();
        self.groups.clear();
        self.groups.extend(characters.bodies.iter().map(|_| Group {
            sphere: None,
            far: false,
            small: false,
            known: false,
            level: 0,
        }));
        for &slot in characters.listed {
            let Some(cull) = self.slots.get_mut(slot as usize) else { continue };
            let sphere = slots.world_bounds(slot, meshes);
            let body = characters.body(slot);
            // Level 0 in every view, unless the stage says its character chooses by error (M5 D5).
            let coarsens = body
                .is_some_and(|group| characters.bodies[group as usize].detail == CharacterDetail::ByError);
            cull.whole = !coarsens;
            if let Some(group) = body {
                let entry = &mut self.groups[group as usize];
                entry.sphere = match (entry.sphere, sphere) {
                    (Some(group), Some(part)) => Some(group.union(part)),
                    (group, part) => group.or(part),
                };
                if coarsens {
                    cull.character = group;
                    self.parts.push(slot);
                }
            }
            match body.map(|group| (group, characters.bodies[group as usize].role)) {
                Some((group, ShadowRole::Ambient)) => {
                    cull.group = group;
                    let known = !cull.fresh;
                    let entry = &mut self.groups[group as usize];
                    if known && !entry.known {
                        (entry.far, entry.small, entry.known) = (cull.far, cull.small, true);
                    }
                }
                _ => {
                    cull.kept = true;
                    self.named.extend(sphere);
                }
            }
        }
        // Distance: the main view's camera, every instance with draws.
        let distance = thresholds.distance;
        let Some(camera) = camera.filter(|_| mode.thresholds() && distance > 0.0) else {
            for cull in &mut self.slots {
                cull.far = false;
            }
            return;
        };
        let past = |sphere: &Sphere, far: bool| {
            let gap = sphere.center.distance(camera.position) - sphere.radius;
            if far { gap > distance * DISTANCE_RETURN } else { gap > distance }
        };
        for group in &mut self.groups {
            group.far = group.sphere.is_some_and(|sphere| past(&sphere, group.known && group.far));
        }
        let Self { slots: culls, groups, named, .. } = self;
        for candidate in &list.cullable {
            let cull = &mut culls[candidate.slot as usize];
            if cull.kept {
                cull.far = false;
                continue;
            }
            if let Some(group) = groups.get(cull.group as usize) {
                cull.far = group.far;
                continue;
            }
            let Some(sphere) = slots.world_bounds(candidate.slot, meshes) else {
                cull.far = false;
                continue;
            };
            cull.far = past(&sphere, cull.far && !cull.fresh);
            if cull.far && overlaps(named, &sphere) {
                (cull.far, cull.kept) = (false, true);
            }
        }
    }

    /// Works out each instance's verdict for the view it stands in (see the module docs), from
    /// `inputs`, in parallel over chunks of slots.
    pub(crate) fn record<M: Fn(&MeshRecord) -> f32 + Sync>(&mut self, inputs: &CullInputs<'_, M>) {
        profiling::scope!("ViewCull::record");
        let (mode, thresholds) = (self.mode, self.thresholds);
        let tested = AtomicU32::new(0);
        let culled = [const { AtomicU32::new(0) }; 3];
        let culled_draws = AtomicU32::new(0);
        let switched = [const { AtomicU32::new(0) }; 2];
        let candidates = &inputs.list.cullable;
        let least = thresholds.least_pixels;
        let sizing = Sizing { on: mode.thresholds() && least > 0.0, least };
        // The groups' size, once: every part takes its group's.
        let main = inputs.views[0];
        self.small_groups.clear();
        self.small_groups.extend(self.groups.iter().map(|group| {
            sizing.on
                && main.zip(group.sphere).is_some_and(|(view, sphere)| {
                    view.pixels(&sphere) < sizing.limit(group.known && group.small)
                })
        }));
        for (group, &small) in self.groups.iter_mut().zip(&self.small_groups) {
            group.small = small;
        }
        self.choose_character_levels(inputs);
        let Self { slots, named, small_groups, group_levels, .. } = self;
        let (named, small_groups, group_levels) = (&named[..], &small_groups[..], &group_levels[..]);
        let tests = (mode.frustum(), sizing);
        sapphire_jobs::par_for_each_chunk_mut(slots, CHUNK, |start, chunk| {
            let first = candidates.partition_point(|candidate| (candidate.slot as usize) < start);
            let end = start + chunk.len();
            let (mut counts, mut counted_draws, mut seen, mut switches) = ([0u32; 3], 0, 0, [0u32; 2]);
            for cull in chunk.iter_mut() {
                cull.verdict = Verdict::Drawn;
            }
            let within = candidates[first..].iter().take_while(|candidate| (candidate.slot as usize) < end);
            for candidate in within {
                let cull = &mut chunk[candidate.slot as usize - start];
                // The level the main view drew last frame, which the hysteresis holds: none after a
                // cut, for a new instance, or for one the main view didn't draw.
                let held = (!inputs.cut && !cull.fresh && cull.drawn_by & BY_VIEW != 0).then_some(cull.level);
                cull.level = 0;
                let groups = (small_groups, group_levels);
                let Some(verdict) = verdict(inputs, tests, candidate, cull, named, groups, held) else {
                    continue;
                };
                seen += 1;
                cull.verdict = verdict;
                if let Some(index) = (verdict as usize).checked_sub(1) {
                    counts[index] += 1;
                    counted_draws += candidate.draws;
                }
                if let Some(held) = held.filter(|_| verdict == Verdict::Drawn) {
                    switches[0] += u32::from(cull.level > held);
                    switches[1] += u32::from(cull.level < held);
                }
            }
            tested.fetch_add(seen, Ordering::Relaxed);
            for (total, count) in culled.iter().zip(counts) {
                total.fetch_add(count, Ordering::Relaxed);
            }
            culled_draws.fetch_add(counted_draws, Ordering::Relaxed);
            for (total, count) in switched.iter().zip(switches) {
                total.fetch_add(count, Ordering::Relaxed);
            }
        });
        let [frustum, distance, size] = culled.map(AtomicU32::into_inner);
        self.stats = ViewCullingStats {
            tested: if mode.frustum() { tested.into_inner() } else { 0 },
            culled: frustum,
            culled_by_distance: distance,
            culled_by_size: size,
            drawn: (inputs.list.draws.len() as u32).saturating_sub(culled_draws.into_inner()),
        };
        let [coarser, finer] = switched.map(AtomicU32::into_inner);
        self.level_stats = LevelStats { coarser, finer };
    }

    /// Chooses the level of each character that chooses its level by error (M5 D5), before the
    /// chunks: one level for all its parts, by the sphere around them, the coarsest every part
    /// with levels is good enough at (each by its own mesh's errors and stretch), held as one by
    /// the hysteresis from the finest level the main view drew a part at last frame. A part with
    /// one level never holds its character back; each part then draws the coarsest of its own
    /// levels within its character's ([`part_level`]).
    fn choose_character_levels<M>(&mut self, inputs: &CullInputs<'_, M>) {
        let Self { slots, groups, parts, group_levels, .. } = self;
        group_levels.clear();
        group_levels.resize(groups.len(), 0);
        let settings = inputs.levels;
        let Some(view) = inputs.views[0].filter(|_| settings.mode == LevelsMode::On) else { return };
        // Last frame's level, one above it in `group_levels` until chosen (0 for none): the
        // coarsest a part drew at, of those the main view drew (a part with fewer levels drew its
        // own coarsest, below its character's); none after a cut.
        for group in groups.iter_mut() {
            group.level = u8::MAX;
        }
        for &slot in parts.iter() {
            let cull = &slots[slot as usize];
            if inputs.cut || cull.fresh || cull.drawn_by & BY_VIEW == 0 {
                continue;
            }
            let entry = &mut group_levels[cull.character as usize];
            *entry = (*entry).max(cull.level + 1);
        }
        for &slot in parts.iter() {
            let character = slots[slot as usize].character as usize;
            let group = &mut groups[character];
            let stand = inputs.stands.get(slot as usize).copied().unwrap_or(0);
            let (Some(sphere), Some(transform), Some(mesh)) =
                (group.sphere, inputs.slots.transform(slot), inputs.slots.mesh(slot))
            else {
                continue;
            };
            let Some(record) = inputs.meshes.get(mesh.index()).filter(|record| record.lod_errors.len() > 1)
            else {
                continue;
            };
            if stand != 0 {
                continue;
            }
            // A projective part has no stretch to scale its errors by: its character stays whole.
            if transform.row(3) != Vec4::W {
                group.level = 0;
                continue;
            }
            let errors = &record.lod_errors;
            let held = group_levels[character].checked_sub(1);
            let budget =
                levels::allowance(settings.bias) * view.meters_per_pixel(&sphere) / max_stretch(transform);
            let choice = levels::choose(errors, budget, held);
            // At its own coarsest, a part holds its character back no further.
            if choice < levels::last_level(errors.len()) {
                group.level = group.level.min(choice);
            }
        }
        for (entry, group) in group_levels.iter_mut().zip(groups.iter()) {
            *entry = group.level;
        }
    }

    /// Records which views drew each instance this frame, and the finest level any drew it at: the
    /// view it stands in, if its verdict draws it and that view drew (at the main view's level, or
    /// an inset's 0), and the shadows, by `shadow(slot)`: the finest level a cascade or a character
    /// map drew it at, if any did.
    pub(crate) fn publish(
        &mut self,
        stands: &[u16],
        views: &[bool; CULL_VIEWS],
        shadow: impl Fn(u32) -> Option<u8>,
    ) {
        for (slot, cull) in (0..).zip(&mut self.slots) {
            let stand = stands.get(slot as usize).copied().unwrap_or(0);
            let drawn =
                cull.verdict == Verdict::Drawn && views.get(usize::from(stand)).copied().unwrap_or(false);
            let mut bits = 0;
            let mut finest = None;
            if drawn {
                bits |= if stand == 0 { BY_VIEW } else { BY_INSET };
                finest = Some(if stand == 0 { cull.level } else { 0 });
            }
            if let Some(level) = shadow(slot) {
                bits |= BY_SHADOW;
                finest = Some(finest.map_or(level, |finest: u8| finest.min(level)));
            }
            cull.drawn_by = bits;
            cull.finest = finest.unwrap_or(0);
        }
    }

    /// Which views drew the instance in `slot` in the last recorded frame, and at what levels.
    pub(crate) fn drawn_by(&self, slot: u32) -> DrawnBy {
        let Some(cull) = self.slots.get(slot as usize) else { return DrawnBy::default() };
        let view = cull.drawn_by & BY_VIEW != 0;
        DrawnBy {
            view,
            shadow: cull.drawn_by & BY_SHADOW != 0,
            inset: cull.drawn_by & BY_INSET != 0,
            level: if view { cull.level } else { 0 },
            finest: cull.finest,
        }
    }

    /// The finest level any view drew the instance in `slot` at this frame, if any drew it: what
    /// skinning poses, and the prefix of its vertices it skins (M5 D5).
    pub(crate) fn drawn(&self, slot: u32) -> Option<u8> {
        self.slots.get(slot as usize).filter(|cull| cull.drawn_by != 0).map(|cull| cull.finest)
    }

    /// The instance slots it holds a culling for: the scene's.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// The frame's verdicts, for the passes; `None` with culling off.
    pub(crate) fn culled(&self) -> Option<Culled<'_>> {
        self.mode.frustum().then_some(Culled { slots: &self.slots })
    }

    /// The frame's culling for the shadow plan, whatever the mode: its far instances, the kept and
    /// what draws level 0 alone.
    pub(crate) fn casters(&self) -> Culled<'_> {
        Culled { slots: &self.slots }
    }

    /// The frame's levels of detail for the main view's family, whatever the culling mode
    /// ([`Culled::level`]).
    pub(crate) fn levels(&self) -> Culled<'_> {
        Culled { slots: &self.slots }
    }

    pub(crate) fn stats(&self) -> ViewCullingStats {
        self.stats
    }

    /// The main view's switches of level in the last recorded frame.
    pub(crate) fn level_stats(&self) -> LevelStats {
        self.level_stats
    }
}

/// Whether `sphere` overlaps any of `named`.
fn overlaps(named: &[Sphere], sphere: &Sphere) -> bool {
    named.iter().any(|named| named.center.distance(sphere.center) <= named.radius + sphere.radius)
}

/// The main view's size test this frame: on, and its least size in output pixels.
#[derive(Clone, Copy, Debug)]
struct Sizing {
    on: bool,
    least: f32,
}

impl Sizing {
    /// The least size for an instance culled by size last frame (`small`) or not.
    fn limit(&self, small: bool) -> f32 {
        if small { self.least * SIZE_RETURN } else { self.least }
    }
}

/// `candidate`'s verdict in the view it stands in, updating its size's hysteresis in `cull`, and its
/// level of detail if the main view draws it, from `held`, the level it drew last frame (the
/// `levels` module); `None` where it isn't tested (no view, a projective transform, no bounds),
/// which draws level 0.
fn verdict<M: Fn(&MeshRecord) -> f32>(
    inputs: &CullInputs<'_, M>,
    (frustum, sizing): (bool, Sizing),
    candidate: &CullCandidate,
    cull: &mut SlotCull,
    named: &[Sphere],
    (small_groups, group_levels): (&[bool], &[u8]),
    held: Option<u8>,
) -> Option<Verdict> {
    let slot = candidate.slot;
    let stand = usize::from(inputs.stands.get(slot as usize).copied().unwrap_or(0));
    let view = inputs.views.get(stand).copied().flatten()?;
    let transform = inputs.slots.transform(slot)?;
    // The vertex stages transform by the whole matrix: a projective one would move what the sphere
    // holds.
    if transform.row(3) != Vec4::W {
        return None;
    }
    let record = inputs.meshes.get(candidate.mesh.index())?;
    let bounds = inputs.slots.world_bounds(slot, inputs.meshes)?;
    let stretch = max_stretch(transform);
    let mut sphere = bounds;
    if candidate.foliage && record.foliage.is_some() {
        sphere.radius += (inputs.margin)(record);
    }
    if candidate.fog_card {
        // A facing card's vertices turn about the origin, each as far from it as its scaled
        // position: within the largest stretch × the mesh's farthest reach from its origin.
        let reach = record.bounds.center.length() + record.bounds.radius;
        let facing = Sphere { center: transform.w_axis.truncate(), radius: stretch * reach };
        sphere = sphere.union(facing);
    }
    let reach = inputs.reach.get(slot as usize).copied().unwrap_or_default();
    sphere.radius += reach.meters + reach.drift * stretch;
    // Size carries its hysteresis whatever the frustum says, so a turn of the camera doesn't reset
    // it; only the main view measures it, and a group's part takes its group's.
    cull.small = if cull.kept || stand != 0 || !sizing.on {
        false
    } else if let Some(&small) = small_groups.get(cull.group as usize) {
        small
    } else {
        let small = view.pixels(&sphere) < sizing.limit(cull.small && !cull.fresh);
        // What a named character holds stays, however small.
        if small && overlaps(named, &sphere) {
            cull.kept = true;
            false
        } else {
            small
        }
    };
    let outside = frustum
        && if reach.hulled { view.hulled.excludes_beside(&sphere) } else { view.frustum.excludes(&sphere) };
    let verdict = if outside {
        Verdict::Frustum
    } else if cull.far && stand == 0 {
        Verdict::Distance
    } else if cull.small {
        Verdict::Size
    } else {
        Verdict::Drawn
    };
    // The main view's family draws one level of it (M5 D4), a character's part its character's
    // (M5 D5); an inset's instances, whole characters, foliage and fog cards level 0.
    if verdict == Verdict::Drawn && stand == 0 && !cull.whole {
        cull.level = match group_levels.get(cull.character as usize) {
            Some(&character) => part_level(inputs.levels, &record.lod_errors, character),
            None => {
                let meters_per_pixel = view.meters_per_pixel(&bounds);
                main_level(inputs.levels, &record.lod_errors, stretch, meters_per_pixel, held)
            }
        };
    }
    Some(verdict)
}

/// The level a part of a character that chooses by error draws at, where its mesh's levels' errors
/// are `errors` and its character's level is `character` ([`ViewCull::choose_character_levels`]):
/// the coarsest of its own levels within its character's, or what the mode forces.
pub(crate) fn part_level(settings: LevelSettings, errors: &[f32], character: u8) -> u8 {
    if let Some(level) = settings.forced(errors.len()) {
        return level;
    }
    if errors.len() < 2 {
        return 0;
    }
    character.min(levels::last_level(errors.len()))
}

/// The main view's level of a mesh whose levels' errors are `errors`, for an instance stretched at
/// most `stretch` times whose bounds' nearest point an output pixel covers `meters_per_pixel` of,
/// drawn at `held` last frame if the main view drew it (the `levels` module).
pub(crate) fn main_level(
    settings: LevelSettings,
    errors: &[f32],
    stretch: f32,
    meters_per_pixel: f32,
    held: Option<u8>,
) -> u8 {
    if let Some(level) = settings.forced(errors.len()) {
        return level;
    }
    if errors.len() < 2 {
        return 0;
    }
    // The most an error may be in the mesh's own meters to cover the allowance's output pixels.
    let budget = levels::allowance(settings.bias) * meters_per_pixel / stretch;
    levels::choose(errors, budget, held)
}

/// Whether `point` lands on a `size` image or within `pixels` of its edge, in front of the
/// camera, as `clip_from_world` projects it: the tests' reference.
#[cfg(test)]
fn on_screen(clip_from_world: Mat4, size: (u32, u32), pixels: f32, point: Vec3) -> bool {
    let clip = clip_from_world * point.extend(1.0);
    let ndc = clip.xy() / clip.w;
    let limit = glam::Vec2::new(1.0 + 2.0 * pixels / size.0 as f32, 1.0 + 2.0 * pixels / size.1 as f32);
    clip.w > 0.0 && ndc.abs().cmple(limit).all()
}

#[cfg(test)]
mod tests;
