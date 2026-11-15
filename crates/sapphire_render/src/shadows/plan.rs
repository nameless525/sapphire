//! Each frame's shadows, planned on the CPU: which cascades are drawn and where, who casts into
//! what, which characters draw maps and which cast capsules, which other characters' shadows each
//! character receives, which spot lights cast and which of their tiles are drawn again (M5 L16, the
//! `spots` module), and what receivers read. Its vectors are reused, so a warm frame allocates
//! nothing.

use glam::Vec3;

use super::fit::{self, CascadeFit, Frustum, LightSpace};
use super::spots::{self, ChosenSpot, Signature, SpotTile};
use super::{
    ATLAS_COLUMNS, CascadeCache, GpuCapsule, GpuCascade, GpuCharacterMap, GpuSpotShadow, HOLD_SHARE,
    MAX_CAPSULES, MAX_CASCADES, MAX_CHARACTER_MAPS, MAX_OTHERS, MAX_SPOT_SHADOWS, SPOT_COLUMNS,
    ShadowSettings, ShadowStats, ShadowUniforms,
};
use crate::bounds::{Sphere, max_stretch};
use crate::characters::{CharacterTable, GpuReceived, RECEIVED_CAPSULE};
use crate::levels::{self, LevelSettings};
use crate::mesh_pool::{MeshRecord, MeshStreams};
use crate::pipelines::SurfaceVariant;
use crate::scene::{DrawList, InstanceSlots};
use crate::view_cull::{Culled, SIZE_RETURN, Verdict};
use crate::{CameraData, CharacterBody, FramePacket, ShadowRole};

/// How far a capsule's shadow reaches from it along the light, in meters (`CAPSULE_REACH` in
/// frame.wgsl).
const CAPSULE_REACH: f32 = 8.0;
/// Over how many meters a caster's weight on a receiver grows from 0 as its sphere comes to
/// overlap the receiver's in the light's view, comes toward the light past the receiver's back,
/// or comes within its shadow's reach: where its shadow can't yet touch the receiver, so the
/// weight's fade never shows.
const FALLS_ON_BAND: f32 = 0.25;
/// Over how many meters of rank a receiver's last chosen caster yields its slot to the first left
/// out: it fades out as their ranks meet, so they trade places at weight 0 (M5 L12).
const YIELD_BAND: f32 = 0.5;
/// What a caster's rank grows by as its weight falls from 1 to 0: past any distance along the
/// light, so a caster only beginning to fall on a receiver ranks behind every whole one.
const UNSURE_RANK: f32 = 1.0e4;

/// How far the frame's wind can carry a mesh's vertices past its cooked bounds, in meters:
/// foliage's margin (`foliage::wind_margin`), 0 for every other mesh.
pub(crate) type WindMargin<'a> = dyn Fn(&MeshRecord) -> f32 + 'a;

/// What each instance slot casts into this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SlotCaster {
    /// Bit `i`: the instance is drawn into cascade `i`.
    pub cascades: u8,
    /// The character map it is drawn into, if any.
    pub map: Option<u8>,
    /// Bit `i`: cascade `i` is drawn and its volume holds the instance (M5 D3): those of these not
    /// in [`Self::cascades`] left it out by distance or by size.
    pub held: u8,
    /// Past the tier's distance (`view_cull`): no cascade draws it.
    pub far: bool,
    /// Per cascade: the level of detail it draws the instance at (M5 D4, `levels::cascade_level`),
    /// by the cascade's texel alone; 0 where it doesn't draw it, for a character and for whatever
    /// draws level 0 alone.
    pub levels: [u8; MAX_CASCADES],
    /// The level its character map draws it at (M5 D5), by the map's texel as a cascade chooses:
    /// 0 for a character that stays whole, and where no map draws it.
    pub map_level: u8,
    /// Bit `i`: the instance is drawn into spot tile `i` (M5 L16), which is drawn this frame.
    pub spots: u8,
    /// Per spot tile: the level it draws the instance at, by the tile's texel where it stands, as a
    /// cascade's; 0 where it doesn't draw it.
    pub spot_levels: [u8; MAX_SPOT_SHADOWS],
}

/// What the cascades' maps hold of one instance slot, as they were last drawn (M5 D8): what a
/// cascade that keeps compares the frame's casters with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeptCaster {
    /// Bit `i`: cascade `i`'s map holds it, at `levels[i]`.
    pub maps: u8,
    pub levels: [u8; MAX_CASCADES],
}

impl SlotCaster {
    /// Why cascade `cascade` doesn't draw the instance, by the first test that left it out: its
    /// volume ([`Verdict::Frustum`], the cascade's own pick), the tier's distance, then its size in
    /// the cascade's texels; `None` if it draws it.
    pub(crate) fn left_out(&self, cascade: usize) -> Option<Verdict> {
        let bit = 1 << cascade;
        if self.cascades & bit != 0 {
            None
        } else if self.held & bit == 0 {
            Some(Verdict::Frustum)
        } else if self.far {
            Some(Verdict::Distance)
        } else {
            Some(Verdict::Size)
        }
    }
}

/// What the shadow plan culls casters by beyond each cascade's volume (M5 D3, `view_cull`): the
/// frame's far and kept instances, and the least size in a cascade's texels (0 for none); and what
/// it chooses their levels of detail by (M5 D4): the frame's settings, and what draws level 0
/// alone ([`Culled::whole`]); and a scale debug view's cull pose (M5 X1), which the cascades fit
/// and cull by in the main view's camera's place.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CasterCull<'a> {
    pub culled: Culled<'a>,
    pub least_texels: f32,
    pub levels: LevelSettings,
    pub pose: Option<&'a CameraData>,
}

impl CasterCull<'_> {
    /// No culling beyond the cascades' volumes, and every caster at level 0.
    #[cfg(test)]
    pub(crate) const NONE: CasterCull<'static> =
        CasterCull { culled: Culled::NONE, least_texels: 0.0, levels: LevelSettings::ON, pose: None };
}

/// Whether an instance slot is a character this frame (it has an entry in the character
/// buffer), and the character group it belongs to if that casts a shadow of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SlotCharacter {
    #[default]
    Environment,
    Character(Option<u32>),
}

/// A character casting a shadow of its own this frame: its instances share `body`.
#[derive(Clone, Copy, Debug)]
struct CharacterGroup {
    body: CharacterBody,
    /// Around its body and its instances' bounds, world space.
    sphere: Sphere,
    /// From the camera to the sphere's nearest point, for choosing who gets maps.
    distance: f32,
    cast: Cast,
}

/// What a character casts its own shadow with this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cast {
    /// A tile of the character atlas (desktop).
    Map(u8),
    /// Its body's capsule, the receivers' capsule of this index.
    Capsule(u8),
    /// Nothing: past the shadow distance, or past the budgets.
    Nothing,
}

/// A frame's shadows (see the module docs) and the cascades as last drawn, which later frames
/// reuse.
#[derive(Debug, Default)]
pub(crate) struct ShadowPlan {
    /// Per cascade: how it was last drawn, while still valid.
    cached: [Option<CascadeFit>; MAX_CASCADES],
    /// The quantized key light the cached cascades were drawn along.
    toward: Option<Vec3>,
    /// Per cascade: its map holds nothing, since nothing cast into it when it was last drawn.
    empty: [bool; MAX_CASCADES],
    /// Frames planned since the renderer was created: whose turn it is.
    turn: u64,
    /// Per cascade: drawn this frame.
    pub drawn: [Option<CascadeFit>; MAX_CASCADES],
    /// Per cascade: the fit its casters are tested in this frame, drawn or not. Drawing in turns,
    /// only those drawn have one; a cache that keeps tests every cascade's, to see what changed.
    fits: [Option<CascadeFit>; MAX_CASCADES],
    /// Per cascade (M5 D8, a cache that keeps): what its map holds is no longer what its casters
    /// would draw, since something in it moved, came, left or changed its level after it drew.
    stale: [bool; MAX_CASCADES],
    /// The draw list's build the maps were last checked against: a new one leaves every map stale.
    list_build: Option<u32>,
    /// Per instance slot: what the maps hold of it.
    kept: Vec<KeptCaster>,
    /// Per instance slot, this frame: it changes its depth without the plan seeing it (skinned or
    /// morphed, moved this frame, or reading a texture on a frame a texture changed).
    moving: Vec<bool>,
    /// Per instance slot, this frame: foliage the wind carries, which the fresh cascades draw again
    /// every frame and the cached ones leave at rest (the module docs).
    windy: Vec<bool>,
    /// The cache the last frame planned with.
    cache: CascadeCache,
    /// A texture was raised, lowered or rewritten since the last frame planned: what reads one as it
    /// casts may cast other depth.
    textures_changed: bool,
    /// Bit `i`: cached cascade `i` is drawn this frame under the fit it already had.
    redrawn: u8,
    /// Per instance slot, for the draw list's build in `list_build`: it casts through a pipeline that
    /// reads a texture (alpha-tested foliage, masked anime character variants), whose levels stream.
    cutout: Vec<bool>,
    /// This frame's character maps, by index.
    pub maps: Vec<CascadeFit>,
    /// Per instance slot: what it casts into.
    pub casters: Vec<SlotCaster>,
    /// Per instance slot, bit `i`: below the least size in cascade `i`'s texels when last tested
    /// (with hysteresis).
    small: Vec<u8>,
    /// Per instance slot: a character or not.
    characters: Vec<SlotCharacter>,
    groups: Vec<CharacterGroup>,
    /// The groups' indices, nearest the camera first.
    order: Vec<u32>,
    /// Per group: the other characters' shadows it receives (M5 L12).
    pub received: Vec<GpuReceived>,
    /// A receiver's casters as [`Self::receive`] ranks them.
    candidates: Vec<Candidate>,
    /// The spot layer's tiles as last drawn (M5 L16): what each holds, kept across frames.
    tiles: [SpotTile; MAX_SPOT_SHADOWS],
    /// This frame's spots, nearest the camera first, each with its tile.
    pub spots: Vec<ChosenSpot>,
    /// Scratch for choosing them.
    spot_candidates: Vec<spots::Candidate>,
    /// The spot pass clears the whole layer this frame: every tile a spot holds is drawn again.
    pub spot_clear: bool,
    /// Flagged spots too wide to cast were logged: once.
    warned_wide: bool,
    /// What receivers read.
    pub uniforms: ShadowUniforms,
    /// The frame has a main view: there is something to draw.
    pub active: bool,
    pub stats: ShadowStats,
}

impl ShadowPlan {
    /// Plans the frame of `packet` with `settings`: the quantized key light, the cascades, the
    /// casters and the receivers' uniforms. `slots` has the packet's instance deltas applied,
    /// `meshes` are the pool's records, `wind_margin` how far the frame's wind can carry each
    /// mesh's vertices past its bounds (foliage's; 0 for the rest), `list` the frame's draw list
    /// (for the stats' counts), `characters` the frame's characters as the character buffer
    /// resolved them, `atlas` the character atlas's size in texels, `cull` what view culling
    /// leaves out of every cascade (M5 D3), and `capsules` the most characters that cast capsules
    /// (M5 L18: [`MAX_CAPSULES`] with capsule bins on, 16 with them off).
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs, each read once")]
    pub(crate) fn update(
        &mut self,
        settings: &ShadowSettings,
        packet: &FramePacket,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
        wind_margin: &WindMargin<'_>,
        list: &DrawList,
        characters: CharacterTable<'_>,
        atlas: (u32, u32),
        cull: CasterCull<'_>,
        capsules: usize,
    ) {
        profiling::scope!("ShadowPlan::update");
        self.drawn = [None; MAX_CASCADES];
        self.maps.clear();
        let Some(view) = packet.main_view() else {
            // Nothing draws, and the frames before and after may be far apart: the next frame
            // with a view draws every cascade.
            self.active = false;
            self.cached = [None; MAX_CASCADES];
            self.received.clear();
            self.spots.clear();
            return;
        };
        self.active = true;
        let camera = cull.pose.unwrap_or(&view.camera);
        let toward = fit::quantize(-packet.sun.direction);
        let light = LightSpace::new(toward);
        if self.toward != Some(toward) || view.cut {
            if self.toward.is_some_and(|before| before != toward) {
                self.stats.light_steps += 1;
            }
            self.cached = [None; MAX_CASCADES];
            self.toward = Some(toward);
        }
        if self.cache != settings.cache {
            // What the maps hold was never kept drawing in turns (M5 D8).
            self.forget_kept();
            self.cache = settings.cache;
        }
        let keeps = settings.cache.keeps();
        if keeps {
            self.fit_kept_cascades(settings, camera, light);
        } else {
            self.plan_cascades(settings, camera, light);
        }
        self.classify(characters, slots, (meshes, wind_margin), settings, camera, light, cull, capsules);
        if keeps {
            self.choose_kept_cascades(settings, slots);
        }
        self.receive(settings, light);
        self.skip_empty(list);
        if keeps {
            self.settle_kept_cascades(slots);
        }
        self.textures_changed = false;
        self.uniforms = self.pack(settings, camera, light, characters.count, atlas);
        self.count_draws(list, settings);
    }

    /// Follows the draw list (M5 D8): after a build other than the last one seen (`build`, the
    /// scene's count of them), which slots cast through a pipeline that reads a texture is listed
    /// again from `list`. What came, went or changed mesh the slots say ([`InstanceSlots::freed`],
    /// [`InstanceSlots::meshed`]); a material replaced in place calls [`Self::forget_kept`]; the
    /// rest a build follows (meshes uploaded or moved in the pool, characters' fades) casts the same.
    pub(crate) fn follow_list(&mut self, build: u32, list: &DrawList) {
        if self.list_build == Some(build) {
            return;
        }
        self.list_build = Some(build);
        self.cutout.iter_mut().for_each(|cutout| *cutout = false);
        let cut = SurfaceVariant::CHARACTER_ALPHA_TEST
            .into_iter()
            .chain([SurfaceVariant::foliage(crate::pipelines::Coverage::AlphaTest)]);
        for variant in cut {
            let slots = list.commands.iter().flat_map(|commands| &commands[variant.index()]).map(|c| c.slot);
            for slot in slots.chain(list.deformed[variant.index()].iter().map(|draw| draw.slot)) {
                let slot = slot as usize;
                if self.cutout.len() <= slot {
                    self.cutout.resize(slot + 1, false);
                }
                self.cutout[slot] = true;
            }
        }
    }

    /// Forgets what every cascade's map holds (M5 D8), so each is drawn again on its next turn: a
    /// mesh or material was replaced in place, or the shaders were rebuilt, which the plan can't see
    /// in its casters.
    pub(crate) fn forget_kept(&mut self) {
        self.stale = [true; MAX_CASCADES];
    }

    /// Notes that a texture was raised, lowered or rewritten (M5 D8): the casters that read one as
    /// they cast leave their cascades stale on the next frame planned.
    pub(crate) fn textures_changed(&mut self) {
        self.textures_changed = true;
    }

    /// Chooses the frame's spot lights from `packet` (M5 L16; the `spots` module): at most
    /// `settings.spots` flagged spots whose cone reaches the main view, nearest the camera first,
    /// each given a tile, the one that holds its drawing where one does. Before the local lights
    /// are packed, which carry each one's tile ([`Self::spot_tiles`]); [`Self::update`] then says
    /// which tiles are drawn again.
    pub(crate) fn choose_spots(&mut self, settings: &ShadowSettings, packet: &FramePacket) {
        let Some(view) = packet.main_view() else {
            self.spots.clear();
            return;
        };
        let count = settings.spot_count();
        let too_wide = spots::choose(
            &packet.lights,
            &view.camera,
            (count, settings.spot_resolution()),
            &self.tiles,
            &mut self.spot_candidates,
            &mut self.spots,
        );
        if too_wide > 0 && !std::mem::replace(&mut self.warned_wide, true) {
            tracing::warn!(
                target: crate::LOG_TARGET,
                lights = too_wide,
                "a spot light asks for a shadow with a cone wider than 120°: it casts none"
            );
        }
        self.stats.spots_too_wide = too_wide;
    }

    /// This frame's spots' tiles: each chosen light's index in the packet and its tile.
    pub(crate) fn spot_tiles(&self) -> impl Iterator<Item = (u32, u8)> + '_ {
        self.spots.iter().map(|spot| (spot.light, spot.tile))
    }

    /// Forgets what every spot tile holds, so each spot's is drawn again: a mesh was replaced in
    /// place, which the tiles' signatures can't see.
    pub(crate) fn forget_spots(&mut self) {
        self.tiles = [SpotTile::default(); MAX_SPOT_SHADOWS];
    }

    /// Bit `i`: spot tile `i` is drawn this frame.
    pub(crate) fn spots_drawn(&self) -> u8 {
        self.spots.iter().filter(|spot| spot.redraw).fold(0, |bits, spot| bits | 1 << spot.tile)
    }

    /// Takes back what this plan kept of the cascades it drew, which were never drawn: their
    /// frame was prepared but its passes didn't run (never recorded, or recorded into a graph
    /// that was dropped, rejected or sized to nothing; M1 review RA-001). Their maps hold what
    /// they held before, so neither the fits cached for them nor the knowledge that they're
    /// empty holds: the next frame draws (and clears) each of them again. The other cascades
    /// keep what earlier frames that ran drew.
    pub(crate) fn not_drawn(&mut self) {
        for (index, drawn) in self.drawn.iter().enumerate() {
            if drawn.is_none() {
                continue;
            }
            // Kept too (M5 D8): what the plan kept of its map is of what the lost frame would have
            // drawn, so it's fitted and drawn again, as drawing in turns does.
            self.empty[index] = false;
            self.cached[index] = None;
        }
        // The spot tiles it drew hold what they held, and a cleared layer what it held before.
        if self.spot_clear {
            self.forget_spots();
        }
        for spot in self.spots.iter().filter(|spot| spot.redraw) {
            self.tiles[spot.tile as usize] = SpotTile::default();
        }
    }

    /// Fits the cascades for `camera` and picks the ones drawn this frame: the fresh ones
    /// always, a cached one on its turn, when it was never drawn, or once the view has moved an
    /// eighth of its width (a quarter of its radius) since it was ([`Self::skip_empty`] leaves some
    /// out again).
    fn plan_cascades(&mut self, settings: &ShadowSettings, camera: &CameraData, light: LightSpace) {
        let cascades = settings.cascades as usize;
        let frustum = Frustum::of(camera);
        let splits = fit::split_distances(frustum.near, settings.distance as f32, cascades);
        let fresh = cascades - settings.cached_cascades as usize;
        let period = u64::from(settings.cached_cascades).max(2);
        let turn = self.turn;
        self.turn += 1;
        self.redrawn = 0;
        let mut from = frustum.near;
        for (index, &to) in splits.iter().enumerate().take(cascades) {
            let sphere = fit::slice_sphere(camera, &frustum, from, to);
            from = to;
            let desired = CascadeFit::new(light, sphere, settings.resolution);
            if index >= fresh && self.cached[index] == Some(desired) {
                self.redrawn |= 1 << index;
            }
            let draw = match self.cached[index] {
                None => true,
                Some(_) if index < fresh => true,
                Some(cached) => {
                    let moved = (cached.center.truncate() - desired.center.truncate()).abs().max_element();
                    turn % period == (index - fresh) as u64 || moved > 0.25 * cached.radius
                }
            };
            if draw {
                self.cached[index] = Some(desired);
                self.drawn[index] = Some(desired);
            }
        }
        // Drawing in turns, a cascade's casters are tested only where it is drawn.
        self.fits = self.drawn;
    }

    /// Fits the cascades for `camera` with a cache that keeps (M5 D8): each one's fit this frame,
    /// which its casters are tested in, and those drawn whatever they hold. A fresh cascade takes
    /// the view's fit, and is drawn when that moved; a cached one takes it when it was never drawn,
    /// once the view has moved a quarter of its radius, and on its turn when the view's moved at
    /// all, or held ([`CascadeCache::Held`]) only once the view has moved [`HOLD_SHARE`] of its
    /// width. [`Self::choose_kept_cascades`] then draws the others whose casters changed, on their
    /// turns.
    fn fit_kept_cascades(&mut self, settings: &ShadowSettings, camera: &CameraData, light: LightSpace) {
        let cascades = settings.cascades as usize;
        let frustum = Frustum::of(camera);
        let splits = fit::split_distances(frustum.near, settings.distance as f32, cascades);
        let fresh = cascades - settings.cached_cascades as usize;
        let period = u64::from(settings.cached_cascades).max(2);
        let turn = self.turn;
        self.turn += 1;
        self.fits = [None; MAX_CASCADES];
        let mut from = frustum.near;
        for (index, &to) in splits.iter().enumerate().take(cascades) {
            let sphere = fit::slice_sphere(camera, &frustum, from, to);
            from = to;
            let desired = CascadeFit::new(light, sphere, settings.resolution);
            let refit = match self.cached[index] {
                None => true,
                Some(cached) if index < fresh => cached != desired,
                Some(cached) => {
                    let moved = (cached.center.truncate() - desired.center.truncate()).abs().max_element();
                    let on_turn = turn % period == (index - fresh) as u64;
                    // Held, only its place is held: a new size or resolution refits it on its
                    // turn as in turns (a new light forgets every fit).
                    let behind = if settings.cache.holds() {
                        let same = CascadeFit {
                            center: desired.center,
                            near: desired.near,
                            far: desired.far,
                            ..cached
                        };
                        same != desired || moved > HOLD_SHARE * 2.0 * cached.radius
                    } else {
                        cached != desired
                    };
                    moved > 0.25 * cached.radius || (on_turn && behind)
                }
            };
            if refit {
                self.cached[index] = Some(desired);
                self.drawn[index] = Some(desired);
            }
            self.fits[index] = self.cached[index];
        }
    }

    /// With a cache that keeps (M5 D8), once the casters are classified: marks each cascade's map
    /// stale whose casters changed since it drew (one came or left, changed level, or changes its
    /// depth by itself), and draws, beside the cascades refitted, the fresh ones and the cached one
    /// whose turn it is that are stale.
    fn choose_kept_cascades(&mut self, settings: &ShadowSettings, slots: &InstanceSlots) {
        let cascades = settings.cascades as usize;
        let fresh = cascades - settings.cached_cascades as usize;
        let period = u64::from(settings.cached_cascades).max(2);
        // `turn` moved on in `fit_kept_cascades`: this frame's is the one before.
        let turn = self.turn.wrapping_sub(1);
        let fresh_mask = (1u8 << fresh) - 1;
        self.kept.resize(slots.len(), KeptCaster::default());
        let mut changed = 0u8;
        // What the maps held of an instance that left, and of one that came or changed mesh, in
        // its slot (whatever they held of the slot's last instance included).
        for &slot in slots.freed() {
            let kept = &mut self.kept[slot as usize];
            changed |= kept.maps;
            *kept = KeptCaster::default();
        }
        for &slot in slots.meshed() {
            changed |= self.kept[slot as usize].maps | self.casters[slot as usize].cascades;
        }
        for (slot, _) in slots.live() {
            let slot = slot as usize;
            let (caster, kept) = (&self.casters[slot], &self.kept[slot]);
            let mut differs = caster.cascades ^ kept.maps;
            for index in 0..cascades {
                let bit = 1 << index;
                if caster.cascades & kept.maps & bit != 0 && caster.levels[index] != kept.levels[index] {
                    differs |= bit;
                }
            }
            let moved = if self.moving[slot] {
                caster.cascades | kept.maps
            } else if self.windy[slot] {
                (caster.cascades | kept.maps) & fresh_mask
            } else {
                0
            };
            changed |= differs | moved;
        }
        self.redrawn = 0;
        for index in 0..cascades {
            self.stale[index] |= changed & (1 << index) != 0;
            let on_turn = index < fresh || turn % period == (index - fresh) as u64;
            if self.drawn[index].is_none() && self.stale[index] && on_turn {
                self.drawn[index] = self.cached[index];
                if index >= fresh {
                    self.redrawn |= 1 << index;
                }
            }
        }
    }

    /// With a cache that keeps (M5 D8), once the cascades drawn are final: what each one drawn now
    /// holds of every slot.
    fn settle_kept_cascades(&mut self, slots: &InstanceSlots) {
        let drawn = self.drawn_mask();
        self.redrawn &= drawn;
        if drawn == 0 {
            return;
        }
        for (index, stale) in self.stale.iter_mut().enumerate() {
            if drawn & (1 << index) != 0 {
                *stale = false;
            }
        }
        for (slot, _) in slots.live() {
            let slot = slot as usize;
            let caster = &self.casters[slot];
            let kept = &mut self.kept[slot];
            kept.maps = (kept.maps & !drawn) | (caster.cascades & drawn);
            for index in 0..MAX_CASCADES {
                if drawn & (1 << index) != 0 {
                    kept.levels[index] = caster.levels[index];
                }
            }
        }
    }

    /// Classifies every instance as character or environment by `characters`, groups characters
    /// by body, gives the nearest groups maps (desktop) and the next capsules, and culls
    /// environment casters per cascade drawn, by its volume and by `cull`'s distance and size.
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs, each read once")]
    fn classify(
        &mut self,
        characters: CharacterTable<'_>,
        slots: &InstanceSlots,
        (meshes, wind_margin): (&[MeshRecord], &WindMargin<'_>),
        settings: &ShadowSettings,
        camera: &CameraData,
        light: LightSpace,
        cull: CasterCull<'_>,
        most_capsules: usize,
    ) {
        self.casters.clear();
        self.casters.resize(slots.len(), SlotCaster::default());
        self.small.resize(slots.len(), 0);
        self.characters.clear();
        self.characters.resize(slots.len(), SlotCharacter::Environment);
        self.moving.clear();
        self.moving.resize(slots.len(), false);
        self.windy.clear();
        self.windy.resize(slots.len(), false);
        self.cutout.resize(slots.len(), false);
        // Characters: the instances with an entry in the character buffer, grouped by the body
        // their entry casts with (M1 review RS-005), so each casts as the character, or the
        // world, it shades as. Its bodies come each once, in entry order.
        self.groups.clear();
        self.groups.extend(characters.bodies.iter().map(|&body| CharacterGroup {
            body,
            sphere: capsule_sphere(&body),
            distance: 0.0,
            cast: Cast::Nothing,
        }));
        for &slot in characters.listed {
            let group = characters.body(slot);
            // Its pose's bounds, where it has left its mesh's: a body lying down stays in its map.
            let instance = slots.world_bounds(slot, meshes);
            if let Some((group, instance)) = group.zip(instance) {
                let group = &mut self.groups[group as usize];
                group.sphere = group.sphere.union(instance);
            }
            self.characters[slot as usize] = SlotCharacter::Character(group);
        }
        // The nearest get maps (desktop), the next capsules; those past the shadow distance, or
        // past both budgets, cast nothing. A crowd's ambient characters cast capsules alone (M5
        // L12).
        self.order.clear();
        for (index, group) in self.groups.iter_mut().enumerate() {
            group.distance = (group.sphere.center.distance(camera.position) - group.sphere.radius).max(0.0);
            self.order.push(index as u32);
        }
        let groups = &self.groups;
        // In place (a stable sort allocates); equal distances keep the packet's order.
        self.order.sort_unstable_by(|&a, &b| {
            groups[a as usize].distance.total_cmp(&groups[b as usize].distance).then(a.cmp(&b))
        });
        let reach = settings.distance as f32;
        let most_capsules = most_capsules.min(MAX_CAPSULES);
        let (mut capsules, mut uncast) = (0u32, 0u32);
        for &index in &self.order {
            let group = &mut self.groups[index as usize];
            let named = group.body.role == ShadowRole::Named;
            group.cast = if group.distance > reach {
                Cast::Nothing
            } else if settings.character_shadow && named && self.maps.len() < MAX_CHARACTER_MAPS {
                self.maps.push(CascadeFit::character(light, group.sphere, settings.character_resolution));
                Cast::Map(self.maps.len() as u8 - 1)
            } else if (capsules as usize) < most_capsules {
                capsules += 1;
                Cast::Capsule(capsules as u8 - 1)
            } else {
                Cast::Nothing
            };
            uncast += u32::from(group.cast == Cast::Nothing);
        }
        // Characters into their maps; environment casters into the cascades drawn that they can
        // cast into, unless they're past the tier's distance or too small in its texels; both into
        // the tiles of the spots whose views they reach (M5 L16), each tile's signature folding
        // them in.
        let least = cull.least_texels;
        let mut signatures = [Signature::default(); MAX_SPOT_SHADOWS];
        let mut moving = [false; MAX_SPOT_SHADOWS];
        for (slot, mesh, transform) in slots.live_transforms() {
            let caster = &mut self.casters[slot as usize];
            let small = &mut self.small[slot as usize];
            if cull.culled.fresh(slot) {
                *small = 0;
            }
            // What the spots test it by: its sphere, whether it moves by itself (a tile it casts
            // into is drawn every frame), its levels' errors and its stretch.
            let mut spot_caster = None;
            match self.characters[slot as usize] {
                SlotCharacter::Character(group) => {
                    caster.map = group.and_then(|group| match self.groups[group as usize].cast {
                        Cast::Map(map) => Some(map),
                        Cast::Capsule(_) | Cast::Nothing => None,
                    });
                    // A character that chooses its level by error (M5 D5): its map's, by its texel.
                    let errors = meshes
                        .get(mesh.index())
                        .filter(|record| record.lod_errors.len() > 1 && !cull.culled.whole(slot))
                        .map(|record| record.lod_errors.as_slice());
                    if let (Some(map), Some(errors)) = (caster.map, errors) {
                        let texel = self.maps[usize::from(map)].texel();
                        caster.map_level = cull.levels.forced(errors.len()).unwrap_or_else(|| {
                            levels::cascade_level(errors, max_stretch(transform), texel, cull.levels.bias)
                        });
                    }
                    self.moving[slot as usize] = true;
                    if !self.spots.is_empty() {
                        spot_caster =
                            slots.world_bounds(slot, meshes).map(|sphere| (sphere, true, None, 1.0));
                    }
                }
                SlotCharacter::Environment => {
                    let (mut windy, mut deformed) = (false, false);
                    let sphere = match meshes.get(mesh.index()) {
                        Some(record) if matches!(record.streams, MeshStreams::Slab { .. }) => {
                            // Foliage reaches past its cooked bounds by as far as the wind
                            // carries it (M1 W6), in world meters whatever the transform.
                            let mut sphere = record.bounds.transformed(transform);
                            let margin = wind_margin(record);
                            sphere.radius += margin;
                            windy = margin > 0.0;
                            Some(sphere)
                        }
                        // A skinned or morphed instance that isn't a character: its posed bounds
                        // (M5 D3), which hold its pending pose and its last drawn one.
                        Some(_) => {
                            (windy, deformed) = (true, true);
                            slots.world_bounds(slot, meshes)
                        }
                        None => None,
                    };
                    caster.far = cull.culled.far(slot);
                    let sized = least > 0.0 && !cull.culled.kept(slot);
                    // Its levels' errors, if it may draw coarser than level 0 (M5 D4).
                    let errors = meshes
                        .get(mesh.index())
                        .filter(|record| record.lod_errors.len() > 1 && !cull.culled.whole(slot))
                        .map(|record| record.lod_errors.as_slice());
                    let stretch = max_stretch(transform);
                    // What changes its depth without the plan seeing it (M5 D8). Foliage in the
                    // wind only in the fresh cascades: its flutter is under the cached cascades'
                    // texels (the module docs).
                    self.windy[slot as usize] = windy;
                    self.moving[slot as usize] = deformed
                        || slots.moved(slot)
                        || (self.textures_changed && self.cutout[slot as usize]);
                    for (index, fit) in self.fits.iter().enumerate() {
                        let Some(fit) = fit else { continue };
                        if !sphere.is_none_or(|sphere| fit.holds(&sphere)) {
                            continue;
                        }
                        let bit = 1 << index;
                        caster.held |= bit;
                        // Its size in the cascade's texels, which are 2 × radius / resolution
                        // across; one culled by size comes back at SIZE_RETURN times the least.
                        let below = sized
                            && sphere.is_some_and(|sphere| {
                                let texels = sphere.radius * fit.resolution as f32 / fit.radius.max(1e-6);
                                let limit = if *small & bit != 0 { least * SIZE_RETURN } else { least };
                                texels < limit
                            });
                        *small = if below { *small | bit } else { *small & !bit };
                        if !caster.far && !below {
                            caster.cascades |= bit;
                            if let Some(errors) = errors {
                                caster.levels[index] =
                                    cull.levels.forced(errors.len()).unwrap_or_else(|| {
                                        levels::cascade_level(errors, stretch, fit.texel(), cull.levels.bias)
                                    });
                            }
                        }
                    }
                    if !self.spots.is_empty() {
                        spot_caster = sphere.map(|sphere| (sphere, windy, errors, stretch));
                    }
                }
            }
            let Some((sphere, moves, errors, stretch)) = spot_caster else { continue };
            for (index, spot) in self.spots.iter().enumerate() {
                if !spot.fit.holds(&sphere) {
                    continue;
                }
                let tile = usize::from(spot.tile);
                let level = errors.map_or(0, |errors| {
                    cull.levels.forced(errors.len()).unwrap_or_else(|| {
                        levels::cascade_level(errors, stretch, spot.fit.texel_at(&sphere), cull.levels.bias)
                    })
                });
                caster.spots |= 1 << tile;
                caster.spot_levels[tile] = level;
                signatures[index].caster(slot, mesh.index() as u32, level, &transform);
                moving[index] |= moves;
            }
        }
        self.decide_spots(&signatures, &moving);
        self.stats.character_maps = self.maps.len() as u32;
        self.stats.capsules = capsules;
        self.stats.uncast_characters = uncast;
    }

    /// Says which chosen spots' tiles are drawn this frame, by what cast into each (`signatures`,
    /// in [`Self::spots`]' order) and whether any of it moves by itself (`moving`): a tile is drawn
    /// again when its spot was just given it or changed its fit, when what casts into it changed,
    /// or when something in it moves. When every tile a spot holds is drawn, the pass clears the
    /// whole layer, and the tiles no spot holds are forgotten.
    fn decide_spots(
        &mut self,
        signatures: &[Signature; MAX_SPOT_SHADOWS],
        moving: &[bool; MAX_SPOT_SHADOWS],
    ) {
        let mut kept = false;
        for (index, spot) in self.spots.iter_mut().enumerate() {
            let tile = &mut self.tiles[usize::from(spot.tile)];
            let signature = signatures[index].0;
            spot.redraw = moving[index] || tile.drawn != Some(spot.fit) || tile.signature != signature;
            kept |= !spot.redraw;
            *tile = SpotTile { drawn: Some(spot.fit), signature };
        }
        self.spot_clear = !kept && !self.spots.is_empty();
        if self.spot_clear {
            let held = self.spots.iter().fold(0u8, |bits, spot| bits | 1 << spot.tile);
            for (index, tile) in self.tiles.iter_mut().enumerate() {
                if held & 1 << index == 0 {
                    *tile = SpotTile::default();
                }
            }
        }
    }

    /// Chooses the other characters' shadows each named character receives (M5 L12; the module
    /// docs of `shadows`, "Characters shadowing each other"), at most `settings.others`: the casters
    /// whose shadows can fall on it ([`falls_on`]), nearest along the light first, each weighted by
    /// how surely, and the last chosen yielding its slot to the first left out as their ranks meet,
    /// so no caster's shadow on a receiver ever pops as characters move.
    fn receive(&mut self, settings: &ShadowSettings, light: LightSpace) {
        self.received.clear();
        self.received.resize(self.groups.len(), GpuReceived::default());
        let slots = (settings.others as usize).min(MAX_OTHERS);
        let mut pairs = 0;
        if slots > 0 {
            let reach = settings.distance as f32;
            for (index, receiver) in self.groups.iter().enumerate() {
                if receiver.body.role != ShadowRole::Named || receiver.distance > reach {
                    continue;
                }
                rank_casters(index, &self.groups, light, &mut self.candidates);
                let received = &mut self.received[index];
                let next = self.candidates.get(slots).map(|left_out| left_out.rank);
                for (slot, candidate) in self.candidates.iter().take(slots).enumerate() {
                    let yields = next.map_or(1.0, |next| smoothstep(0.0, YIELD_BAND, next - candidate.rank));
                    received.casters[slot] = candidate.caster;
                    received.weights[slot] = candidate.weight * yields;
                    pairs += u32::from(received.weights[slot] > 0.0);
                }
            }
        }
        self.stats.received = pairs;
    }

    /// Leaves out the cascades picked this frame that nothing casts into while their maps hold
    /// nothing already: an empty map reads the same under any fit, so the new fit is kept as if
    /// it had been drawn. The others remember whether they hold anything.
    fn skip_empty(&mut self, list: &DrawList) {
        // Slot bounds remain conservative, but only slots in the depth command stream can
        // populate a map. Transparent-only and disowned instances cast no depth.
        let mut cast = 0u8;
        let mut include = |slot: u32| {
            cast |= self.casters.get(slot as usize).map_or(0, |caster| caster.cascades);
        };
        for variant in SurfaceVariant::DEPTH_WRITING {
            for commands in &list.commands {
                for command in &commands[variant.index()] {
                    include(command.slot);
                }
            }
            for command in &list.deformed[variant.index()] {
                include(command.slot);
            }
        }
        for (index, drawn) in self.drawn.iter_mut().enumerate() {
            if drawn.is_none() {
                continue;
            }
            let holds = cast & 1 << index != 0;
            if !holds && self.empty[index] {
                *drawn = None;
            }
            self.empty[index] = !holds;
        }
    }

    /// The receivers' uniforms for the frame planned.
    fn pack(
        &self,
        settings: &ShadowSettings,
        camera: &CameraData,
        light: LightSpace,
        entries: u32,
        (width, height): (u32, u32),
    ) -> ShadowUniforms {
        let mut uniforms = ShadowUniforms {
            light_x: light.x.extend(0.0).to_array(),
            light_y: light.y.extend(0.0).to_array(),
            light_z: light.z.extend(0.0).to_array(),
            camera: camera.position.extend(settings.distance as f32).to_array(),
            ..ShadowUniforms::default()
        };
        let cascades = settings.cascades as usize;
        // Every cascade has been drawn by now: the cache keeps those not drawn this frame.
        for (gpu, fit) in uniforms.cascades.iter_mut().zip(&self.cached[..cascades]) {
            if let Some(fit) = fit {
                *gpu = GpuCascade { area: area(fit), depth: depth(fit, fit.resolution as f32, 0.0) };
            }
        }
        let tile = settings.character_resolution as f32;
        for (index, (gpu, fit)) in uniforms.characters.iter_mut().zip(&self.maps).enumerate() {
            let (column, row) = (index as u32 % ATLAS_COLUMNS, index as u32 / ATLAS_COLUMNS);
            let fade = 0.25 * fit::CHARACTER_REACH;
            *gpu = GpuCharacterMap {
                area: area(fit),
                depth: depth(fit, fit.far, 1.0 / fade),
                tile: [
                    column as f32 * tile / width as f32,
                    row as f32 * tile / height as f32,
                    tile / width as f32,
                    tile / height as f32,
                ],
            };
        }
        let mut count = 0;
        for group in &self.groups {
            let Cast::Capsule(index) = group.cast else { continue };
            let body = group.body;
            uniforms.capsules[index as usize] = GpuCapsule {
                base: body.base.extend(body.radius).to_array(),
                top: body.top.extend(0.0).to_array(),
            };
            count += 1;
        }
        uniforms.counts = [settings.cascades, self.maps.len() as u32, count, entries];
        uniforms.atlas = [1.0 / width as f32, 1.0 / height as f32, tile, 0.0];
        // The spots, each in its tile's record: a light's record names its tile (M5 L16).
        let size = 1.0 / SPOT_COLUMNS as f32;
        for spot in &self.spots {
            let tile = u32::from(spot.tile);
            let (column, row) = (tile % SPOT_COLUMNS, tile / SPOT_COLUMNS);
            uniforms.spots[tile as usize] = GpuSpotShadow {
                clip_from_world: spot.fit.clip_from_world().to_cols_array_2d(),
                tile: [column as f32 * size, row as f32 * size, size, settings.spot_resolution() as f32],
                lens: [spot.fit.texel_per_meter(), settings.cascades as f32, spot.weight, 0.0],
            };
        }
        uniforms
    }

    /// Counts the frame's caster draws, and the environment draws culled from cascades drawn.
    fn count_draws(&mut self, list: &DrawList, settings: &ShadowSettings) {
        let drawn = self.drawn_mask();
        let spots = self.spots_drawn();
        let (mut draws, mut culled, mut spot_draws) = (0, 0, 0);
        let mut count = |slot: u32| {
            let caster = self.casters.get(slot as usize).copied().unwrap_or_default();
            let cascades = (caster.cascades & drawn).count_ones();
            draws += cascades + u32::from(caster.map.is_some());
            spot_draws += (caster.spots & spots).count_ones();
            if self.characters.get(slot as usize) == Some(&SlotCharacter::Environment) {
                culled += drawn.count_ones() - cascades;
            }
        };
        for variant in SurfaceVariant::DEPTH_WRITING {
            for commands in &list.commands {
                for command in &commands[variant.index()] {
                    count(command.slot);
                }
            }
            for draw in &list.deformed[variant.index()] {
                count(draw.slot);
            }
        }
        self.stats = ShadowStats {
            enabled: true,
            cascades: settings.cascades,
            resolution: settings.resolution,
            cascades_drawn: drawn.count_ones(),
            cascades_cached: settings.cascades - drawn.count_ones(),
            caster_draws: draws,
            culled_draws: culled,
            spot_tiles: self.spots.len() as u32,
            spot_tiles_drawn: spots.count_ones(),
            spot_tiles_cached: self.spots.len() as u32 - spots.count_ones(),
            spot_draws,
            spot_clear: self.spot_clear,
            // A loaded layer is read whole; a depth texel is 4 bytes.
            cached_redrawn: (self.redrawn & drawn).count_ones(),
            moving_casters: self.moving_casters(),
            spot_load_bytes: if spots != 0 && !self.spot_clear {
                u64::from(settings.resolution).pow(2) * 4
            } else {
                0
            },
            ..self.stats
        };
    }

    /// Casters with a cascade to cast into this frame that change its depth without the plan seeing
    /// it (M5 D8, [`ShadowStats::moving_casters`]).
    fn moving_casters(&self) -> u32 {
        let casters = self.casters.iter().zip(&self.moving);
        casters.filter(|(caster, moving)| **moving && caster.cascades != 0).count() as u32
    }

    /// Bit `i`: cascade `i` holds its map from an earlier frame, under the fit it has this frame,
    /// and isn't drawn this one (M5 D8, a cache that keeps; drawing in turns, a cascade not drawn
    /// has no fit this frame).
    pub(crate) fn held_mask(&self) -> u8 {
        (0..MAX_CASCADES)
            .filter(|&index| self.fits[index].is_some() && self.drawn[index].is_none())
            .fold(0, |bits, index| bits | 1 << index)
    }

    /// The fit cascade `index` is tested in this frame, drawn or held.
    pub(crate) fn fit(&self, index: usize) -> Option<CascadeFit> {
        self.fits.get(index).copied().flatten()
    }

    /// Per instance slot: what the cascades' maps hold of it, as they were last drawn (M5 D8, a
    /// cache that keeps; empty drawing in turns).
    pub(crate) fn kept(&self) -> &[KeptCaster] {
        &self.kept
    }

    /// Bit `i`: cascade `i` is drawn this frame.
    pub(crate) fn drawn_mask(&self) -> u8 {
        self.drawn
            .iter()
            .enumerate()
            .filter(|(_, fit)| fit.is_some())
            .fold(0, |bits, (index, _)| bits | 1 << index)
    }

    /// Whether the instance in `slot` is a character this frame.
    #[cfg(test)]
    pub(crate) fn is_character(&self, slot: u32) -> bool {
        matches!(self.characters.get(slot as usize), Some(SlotCharacter::Character(_)))
    }
}

/// A caster of a receiver's (M5 L12), as [`ShadowPlan::receive`] ranks them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Candidate {
    /// As a [`GpuReceived`] slot names it: its map, or its capsule with [`RECEIVED_CAPSULE`].
    caster: u32,
    /// How surely its shadow falls on the receiver ([`falls_on`]), above 0.
    weight: f32,
    /// Its distance from the receiver along the light, grown by [`UNSURE_RANK`] as its weight falls:
    /// the lowest ranks are chosen.
    rank: f32,
}

/// Ranks the casters whose shadows can fall on group `receiver` of `groups` into `candidates`, the
/// lowest rank first (ties by caster): every other group that casts a map or a capsule.
fn rank_casters(
    receiver: usize,
    groups: &[CharacterGroup],
    light: LightSpace,
    candidates: &mut Vec<Candidate>,
) {
    candidates.clear();
    let at = |group: &CharacterGroup| (light.point(group.sphere.center), group.sphere.radius);
    let (receiver_at, receiver_radius) = at(&groups[receiver]);
    for (index, group) in groups.iter().enumerate() {
        let (caster, reach) = match group.cast {
            _ if index == receiver => continue,
            Cast::Map(map) => (u32::from(map), fit::CHARACTER_REACH),
            Cast::Capsule(capsule) => (u32::from(capsule) | RECEIVED_CAPSULE, CAPSULE_REACH),
            Cast::Nothing => continue,
        };
        let (caster_at, caster_radius) = at(group);
        let weight = falls_on((receiver_at, receiver_radius), (caster_at, caster_radius), reach);
        if weight > 0.0 {
            let rank = (caster_at.z - receiver_at.z).abs() + (1.0 - weight) * UNSURE_RANK;
            candidates.push(Candidate { caster, weight, rank });
        }
    }
    candidates.sort_unstable_by(|a, b| a.rank.total_cmp(&b.rank).then(a.caster.cmp(&b.caster)));
}

/// How surely a caster's shadow can fall on a receiver, each a sphere in light space (its center,
/// z toward the light, and its radius), for a caster whose shadow reaches `reach` meters past it
/// away from the light: 0 where it can't (their discs don't overlap in the light's view, the
/// caster lies wholly behind the receiver, or the receiver lies wholly past its reach), growing to 1
/// over [`FALLS_ON_BAND`] past each.
fn falls_on(receiver: (Vec3, f32), caster: (Vec3, f32), reach: f32) -> f32 {
    let ((receiver, receiver_radius), (caster, caster_radius)) = (receiver, caster);
    let overlap = receiver_radius + caster_radius - receiver.truncate().distance(caster.truncate());
    let ahead = (caster.z + caster_radius) - (receiver.z - receiver_radius);
    let gap = (caster.z - caster_radius) - (receiver.z + receiver_radius);
    smoothstep(0.0, FALLS_ON_BAND, overlap)
        * smoothstep(0.0, FALLS_ON_BAND, ahead)
        * (1.0 - smoothstep(reach - FALLS_ON_BAND, reach, gap))
}

/// WGSL's `smoothstep`.
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The sphere around a body's capsule.
fn capsule_sphere(body: &CharacterBody) -> Sphere {
    Sphere { center: (body.base + body.top) * 0.5, radius: body.base.distance(body.top) * 0.5 + body.radius }
}

/// A cascade's or map's area as receivers read it.
fn area(fit: &CascadeFit) -> [f32; 4] {
    [fit.center.x, fit.center.y, 0.5 / fit.radius, fit.texel()]
}

/// A cascade's or map's depth transform, with `z` and `w` as its receivers read them.
fn depth(fit: &CascadeFit, z: f32, w: f32) -> [f32; 4] {
    let (scale, offset) = fit.depth_transform();
    [scale, offset, z, w]
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec3};

    use super::*;
    use crate::character_prepass::ID_ENTRY_MASK;
    use crate::characters::CharacterList;
    use crate::gpu_types::Dequantize;
    use crate::mesh_pool::{DeformSource, Placement};
    use crate::{
        CharacterFace, CharacterInstance, CharacterLighting, DirectionalLight, InstanceDeltas, InstanceId,
        MaterialDrive, MeshHandle, MeshInstance, ViewData,
    };

    const SETTINGS: ShadowSettings = ShadowSettings {
        cascades: 4,
        resolution: 2048,
        distance: 120,
        cached_cascades: 2,
        character_shadow: true,
        character_resolution: 1024,
        filter: super::super::ShadowFilter::Tent5,
        others: 4,
        spots: 4,
        cache: CascadeCache::Turns,
    };

    /// A static unit cube (mesh 0) and a deformable one (mesh 1).
    fn meshes() -> Vec<MeshRecord> {
        let record = |streams| MeshRecord {
            streams,
            dequantize: Dequantize::default(),
            lods: Vec::new(),
            texel_density: Default::default(),
            lod_vertices: Vec::new(),
            lod_errors: Vec::new(),
            placement: Placement::default(),
            slots: Vec::new(),
            bounds: Sphere::around([[-0.5; 3], [0.5; 3]]),
            palette_reach: Vec::new(),
            morph_reach: Vec::new(),
            foliage: None,
        };
        vec![
            record(MeshStreams::Slab {
                format: sapphire_assets::mesh::PositionFormat::Unorm16,
                first_vertex: 0,
            }),
            record(MeshStreams::Deformable(DeformSource::default())),
        ]
    }

    fn instance(id: u64, mesh: u32, at: Vec3) -> MeshInstance {
        MeshInstance {
            id: InstanceId(id),
            mesh: MeshHandle::from_raw(mesh),
            transform: Mat4::from_translation(at),
        }
    }

    fn character(id: u64, body: CharacterBody) -> CharacterInstance {
        CharacterInstance {
            id: InstanceId(id),
            lighting: CharacterLighting::unstyled(DirectionalLight::default()),
            face: CharacterFace::default(),
            body,
            opacity: 1.0,
            drive: MaterialDrive::NONE,
        }
    }

    fn packet(characters: Vec<CharacterInstance>) -> FramePacket {
        let camera =
            CameraData::look_at(Vec3::new(0.0, 1.5, 4.0), Vec3::new(0.0, 1.0, 0.0), Vec3::Y, 0.75, 1.5, 0.05);
        FramePacket {
            views: vec![ViewData::new(camera)],
            sun: DirectionalLight { direction: Vec3::new(0.55, -0.7, -0.45), ..DirectionalLight::default() },
            characters,
            ..FramePacket::default()
        }
    }

    fn slots(instances: &[MeshInstance]) -> InstanceSlots {
        let mut slots = InstanceSlots::default();
        slots.apply(&InstanceDeltas { changed: instances.to_vec(), removed: Vec::new() });
        slots
    }

    /// Plans `packet`'s frame with `settings`, its characters resolved into `slots` by the
    /// character buffer's own list, as the renderer does.
    fn update(
        plan: &mut ShadowPlan,
        settings: &ShadowSettings,
        packet: &FramePacket,
        slots: &mut InstanceSlots,
    ) {
        update_capped(plan, settings, packet, slots, MAX_CAPSULES, 0.0);
    }

    /// [`update`] giving out at most `capsules` capsules, with every static mesh foliage the wind
    /// carries `wind` meters.
    fn update_capped(
        plan: &mut ShadowPlan,
        settings: &ShadowSettings,
        packet: &FramePacket,
        slots: &mut InstanceSlots,
        capsules: usize,
        wind: f32,
    ) {
        let mut list = CharacterList::default();
        list.update(packet, slots, false, None);
        let meshes = meshes();
        // These planning fixtures represent one opaque depth draw per live instance.
        let mut list_of_draws = DrawList::default();
        for (slot, _) in slots.live() {
            list_of_draws.commands[0][SurfaceVariant::OPAQUE.index()].push(crate::scene::DrawCommand {
                indices: 0..3,
                first_vertex: 0,
                slot,
                draw_id: slot,
                material: crate::materials::MaterialHandle::from_raw(0),
                mesh: MeshHandle::from_raw(0),
                submesh: 0,
                levels: 1,
                outline: false,
                foliage: 0,
            });
        }
        plan.update(
            settings,
            packet,
            slots,
            &meshes,
            &|_| wind,
            &list_of_draws,
            list.table(),
            (4096, 2048),
            CasterCull::NONE,
            capsules,
        );
    }

    #[test]
    fn live_slots_without_depth_commands_clear_once_then_remain_empty() {
        let mut slots = slots(&[instance(1, 0, Vec3::ZERO)]);
        let packet = packet(Vec::new());
        let mut characters = CharacterList::default();
        characters.update(&packet, &mut slots, false, None);
        let meshes = meshes();
        let mut list = DrawList::default();
        let transparent = SurfaceVariant::ALL
            .into_iter()
            .find(|variant| variant.coverage == crate::pipelines::Coverage::Transparent)
            .expect("transparent variant");
        list.commands[0][transparent.index()].push(crate::scene::DrawCommand {
            indices: 0..3,
            first_vertex: 0,
            slot: 0,
            draw_id: 0,
            material: crate::materials::MaterialHandle::from_raw(0),
            mesh: MeshHandle::from_raw(0),
            submesh: 0,
            levels: 1,
            outline: false,
            foliage: 0,
        });
        let mut plan = ShadowPlan::default();
        plan.update(
            &SETTINGS,
            &packet,
            &slots,
            &meshes,
            &|_| 0.0,
            &list,
            characters.table(),
            (4096, 2048),
            CasterCull::NONE,
            MAX_CAPSULES,
        );
        assert_eq!(plan.drawn_mask(), 0b1111, "new maps must be cleared");
        plan.update(
            &SETTINGS,
            &packet,
            &slots,
            &meshes,
            &|_| 0.0,
            &list,
            characters.table(),
            (4096, 2048),
            CasterCull::NONE,
            MAX_CAPSULES,
        );
        assert_eq!(plan.drawn_mask(), 0, "transparent geometry writes no shadow depth");
        list.commands[0][transparent.index()].clear();
        plan.update(
            &SETTINGS,
            &packet,
            &slots,
            &meshes,
            &|_| 0.0,
            &list,
            characters.table(),
            (4096, 2048),
            CasterCull::NONE,
            MAX_CAPSULES,
        );
        assert_eq!(plan.drawn_mask(), 0, "a disowned slot also writes no depth");
    }

    #[test]
    fn characters_cast_only_into_their_maps_and_the_environment_into_cascades() {
        let body = CharacterBody::capsule(Vec3::new(0.0, 0.25, 0.0), Vec3::new(0.0, 1.6, 0.0), 0.25);
        let instances =
            [instance(1, 0, Vec3::ZERO), instance(2, 1, Vec3::Y), instance(3, 0, Vec3::new(2.0, 0.0, 0.0))];
        let mut slots = slots(&instances);
        // Instances 1 and 2 are one character (the same body); 3 is environment.
        let packet = packet(vec![character(1, body), character(2, body)]);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert!(plan.active);
        assert_eq!(plan.drawn_mask(), 0b1111, "every cascade on the first frame");
        assert_eq!(plan.maps.len(), 1, "one character, one map");
        for slot in [0, 1] {
            assert!(plan.is_character(slot));
            let caster = plan.casters[slot as usize];
            assert_eq!((caster.cascades, caster.map), (0, Some(0)), "never a cascade");
        }
        assert!(!plan.is_character(2));
        assert_eq!(plan.casters[2].map, None);
        assert_eq!(plan.casters[2].cascades & 1, 1, "the environment casts into the first cascade");
        // The entries: the fallback and the character's, which its instances share.
        assert_eq!(plan.uniforms.counts, [4, 1, 0, 2]);
        // Its map holds the whole body and the deformable instance's bounds.
        let fit = plan.maps[0];
        for point in [body.base, body.top, Vec3::new(0.5, 1.5, 0.5)] {
            assert!(fit.holds(&Sphere { center: point, radius: 0.0 }), "{point}");
        }
    }

    /// A body lying down leaves the bind pose's sphere: its map follows the pose, and goes back to
    /// the very fit it had when the pose comes back inside.
    #[test]
    fn a_characters_map_holds_the_pose_it_was_given() {
        let body = CharacterBody::capsule(Vec3::new(0.0, 0.25, 0.0), Vec3::new(0.0, 1.6, 0.0), 0.25);
        let mut slots = slots(&[instance(1, 1, Vec3::ZERO)]);
        let packet = packet(vec![character(1, body)]);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        let held = plan.maps[0];

        let head = Sphere { center: Vec3::new(0.0, 0.3, -2.4), radius: 0.0 };
        assert!(!held.holds(&head), "the bind pose's fit cuts the head off");
        slots.set_posed(0, Some(Sphere { center: Vec3::new(0.0, 0.3, -1.4), radius: 1.2 }));
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert!(plan.maps[0].holds(&head));

        slots.set_posed(0, None);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.maps[0], held);
    }

    #[test]
    fn phones_cast_capsules_and_bodiless_characters_cast_nothing() {
        let mobile = ShadowSettings { cascades: 2, cached_cascades: 1, character_shadow: false, ..SETTINGS };
        let body = CharacterBody::capsule(Vec3::ZERO, Vec3::Y, 0.3);
        let instances = [instance(1, 0, Vec3::ZERO), instance(2, 0, Vec3::X)];
        let packet = packet(vec![character(1, body), character(2, CharacterBody::default())]);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &mobile, &packet, &mut slots(&instances));
        assert!(plan.maps.is_empty());
        assert_eq!(plan.uniforms.counts[..3], [2, 0, 1]);
        assert_eq!(plan.uniforms.capsules[0].base, [0.0, 0.0, 0.0, 0.3]);
        assert_eq!(plan.uniforms.capsules[0].top, [0.0, 1.0, 0.0, 0.0]);
        assert!(plan.is_character(1), "a character without a body still never casts into the cascades");
        assert_eq!(plan.casters[1], SlotCaster::default());
        assert_eq!((plan.stats.capsules, plan.stats.character_maps), (1, 0));
    }

    /// With capsule bins off a frame gives out the 16 capsules it always did, and with them on the
    /// same 16 first, in the same order, then the rest up to 64 (M5 L18).
    #[test]
    fn a_crowd_casts_sixteen_capsules_unbinned_and_every_one_binned() {
        use crate::lights::capsule_bins::UNBINNED_CAPSULES;
        let mobile = ShadowSettings { cascades: 2, cached_cascades: 1, character_shadow: false, ..SETTINGS };
        let instances: Vec<MeshInstance> = (0..70)
            .map(|index| {
                instance(index, 0, Vec3::new((index % 10) as f32 * 2.0, 0.0, (index / 10) as f32 * -2.0))
            })
            .collect();
        let characters = instances
            .iter()
            .map(|instance| {
                let base = instance.transform.transform_point3(Vec3::ZERO);
                character(instance.id.0, CharacterBody::capsule(base, base + Vec3::Y, 0.25))
            })
            .collect();
        let packet = packet(characters);
        let mut plain = ShadowPlan::default();
        update_capped(&mut plain, &mobile, &packet, &mut slots(&instances), UNBINNED_CAPSULES, 0.0);
        let mut binned = ShadowPlan::default();
        update_capped(&mut binned, &mobile, &packet, &mut slots(&instances), MAX_CAPSULES, 0.0);
        assert_eq!(plain.uniforms.counts[2], UNBINNED_CAPSULES as u32);
        assert_eq!(binned.uniforms.counts[2], MAX_CAPSULES as u32, "70 characters fill the 64");
        assert_eq!(
            plain.uniforms.capsules[..UNBINNED_CAPSULES],
            binned.uniforms.capsules[..UNBINNED_CAPSULES]
        );
        assert_eq!(plain.stats.capsules, 16);
        assert_eq!(binned.stats.capsules, 64);
    }

    #[test]
    fn the_nearest_characters_get_maps_and_the_rest_capsules() {
        let instances: Vec<MeshInstance> =
            (0..12).map(|index| instance(index, 0, Vec3::new(0.0, 0.0, -(index as f32) * 3.0))).collect();
        let characters = instances
            .iter()
            .map(|instance| {
                let base = instance.transform.transform_point3(Vec3::ZERO);
                character(instance.id.0, CharacterBody::capsule(base, base + Vec3::Y, 0.25))
            })
            .collect();
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(characters), &mut slots(&instances));
        assert_eq!(plan.maps.len(), MAX_CHARACTER_MAPS);
        assert_eq!(plan.uniforms.counts[2], 12 - MAX_CHARACTER_MAPS as u32, "the rest cast capsules");
        for slot in 0..MAX_CHARACTER_MAPS as u32 {
            assert_eq!(plan.casters[slot as usize].map, Some(slot as u8), "nearest first");
        }
        assert_eq!(plan.casters[MAX_CHARACTER_MAPS].map, None);
        // The capsules go nearest first too.
        let first = plan.uniforms.capsules[0].base;
        assert!((first[2] + 3.0 * MAX_CHARACTER_MAPS as f32).abs() < 1e-5, "{first:?}");
    }

    /// A posed instance by the camera: it casts into every cascade.
    /// A deformable instance posed to reach into every cascade: it casts by its posed bounds
    /// (M5 D3).
    fn everywhere() -> InstanceSlots {
        let mut slots = slots(&[instance(1, 1, Vec3::ZERO)]);
        slots.set_posed(0, Some(Sphere { center: Vec3::ZERO, radius: 500.0 }));
        slots
    }

    #[test]
    fn cached_cascades_take_turns_and_everything_redraws_when_the_light_steps() {
        let mut slots = everywhere();
        let mut packet = packet(Vec::new());
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
        let mut masks = Vec::new();
        for _ in 0..4 {
            update(&mut plan, &SETTINGS, &packet, &mut slots);
            masks.push(plan.drawn_mask());
        }
        assert_eq!(masks, [0b1011, 0b0111, 0b1011, 0b0111], "the far two alternate");
        assert_eq!((plan.stats.cascades_drawn, plan.stats.cascades_cached), (3, 1));
        // A sub-step turn of the sun keeps the cache; a step draws everything again.
        packet.sun.direction = Vec3::new(0.5501, -0.7, -0.45);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_ne!(plan.drawn_mask(), 0b1111);
        packet.sun.direction = Vec3::new(0.6, -0.7, -0.45);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
        assert_eq!(plan.stats.light_steps, 1);
        // One cached cascade is drawn every other frame; a cut draws them all.
        let mobile = ShadowSettings { cascades: 2, cached_cascades: 1, ..SETTINGS };
        let mut plan = ShadowPlan::default();
        let masks: Vec<u8> = (0..5)
            .map(|_| {
                update(&mut plan, &mobile, &packet, &mut slots);
                plan.drawn_mask()
            })
            .collect();
        assert_eq!(masks, [0b11, 0b01, 0b11, 0b01, 0b11]);
        packet.views[0].cut = true;
        update(&mut plan, &mobile, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b11, "a cut");
    }

    #[test]
    fn a_cached_cascade_left_behind_by_the_view_is_drawn_early() {
        let mut slots = everywhere();
        let mut packet = packet(Vec::new());
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1011);
        // The camera turns around: cascade 3 is cached, but its square is now behind the view.
        packet.views[0].camera = CameraData::look_at(
            Vec3::new(0.0, 1.5, 4.0),
            Vec3::new(0.0, 1.0, 20.0),
            Vec3::Y,
            0.75,
            1.5,
            0.05,
        );
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
    }

    #[test]
    fn environment_casters_are_culled_per_cascade() {
        // One cube by the camera and one far away: the far one misses the first cascade.
        let instances = [
            instance(1, 0, Vec3::ZERO),
            instance(2, 0, Vec3::new(0.0, 0.0, -80.0)),
            instance(3, 1, Vec3::new(500.0, 0.0, 0.0)),
        ];
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
        assert_eq!(plan.casters[0].cascades & 1, 1);
        assert_eq!(plan.casters[1].cascades & 1, 0, "past the first cascade");
        assert_ne!(plan.casters[1].cascades & 0b1000, 0, "in the last");
        assert_eq!(plan.casters[2].cascades, 0, "deformable and no character: by its posed bounds, too far");
    }

    #[test]
    fn the_wind_keeps_swaying_foliage_in_the_cascades_it_reaches() {
        // The first place, walking away from the camera, where a cube falls out of the first
        // cascade; there, a margin of a meter (foliage's wind) keeps it in.
        let first_cascade = |at: Vec3, margin: f32| {
            let mut plan = ShadowPlan::default();
            let slots = slots(&[instance(1, 0, at)]);
            plan.update(
                &SETTINGS,
                &packet(Vec::new()),
                &slots,
                &meshes(),
                &|_| margin,
                &DrawList::default(),
                CharacterList::default().table(),
                (4096, 2048),
                CasterCull::NONE,
                MAX_CAPSULES,
            );
            plan.casters[0].cascades & 1 == 1
        };
        let outside = (0..400)
            .map(|step| Vec3::new(0.0, 0.0, -0.25 * step as f32))
            .find(|&at| !first_cascade(at, 0.0))
            .expect("past the first cascade within 100 m");
        assert!(first_cascade(outside, 1.0), "the wind's margin reaches back in at {outside}");
    }

    #[test]
    fn frames_without_a_view_plan_nothing_and_start_the_cache_over() {
        let mut slots = everywhere();
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots);
        update(&mut plan, &SETTINGS, &FramePacket::default(), &mut slots);
        assert!(!plan.active && plan.drawn_mask() == 0);
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111);
    }

    #[test]
    fn a_cascade_nothing_casts_into_is_drawn_once_and_left_empty() {
        // A cube by the camera casts into the near two cascades; the far two are drawn empty
        // once, then left as they are, cached or not.
        let mut instances = vec![instance(1, 0, Vec3::ZERO)];
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
        assert_eq!(plan.drawn_mask(), 0b1111, "the first frame clears every map");
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
        assert_eq!(plan.drawn_mask(), 0b0011);
        assert_eq!((plan.stats.cascades_drawn, plan.stats.cascades_cached), (2, 2));
        // Their fits are kept for the receivers all the same.
        assert!(plan.uniforms.cascades.iter().all(|cascade| cascade.area[2] > 0.0));
        // A caster far away: the far cascade is drawn on its turn, and again while it holds it.
        instances.push(instance(2, 0, Vec3::new(0.0, 0.0, -80.0)));
        let masks: Vec<u8> = (0..3)
            .map(|_| {
                update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&instances));
                plan.drawn_mask()
            })
            .collect();
        assert_eq!(masks.iter().filter(|&&mask| mask & 0b1000 != 0).count(), 1, "{masks:?}");
        // A step of the light draws again only the cascades that hold something.
        let mut stepped = packet(Vec::new());
        stepped.sun.direction = Vec3::new(0.6, -0.7, -0.45);
        update(&mut plan, &SETTINGS, &stepped, &mut slots(&instances[..1]));
        assert_eq!(plan.drawn_mask(), 0b1011, "cascade 3 is cleared of the cube that left");
        update(&mut plan, &SETTINGS, &stepped, &mut slots(&instances[..1]));
        assert_eq!(plan.drawn_mask() & 0b1100, 0, "and then left empty");
    }

    #[test]
    fn the_cascades_a_lost_frame_drew_are_drawn_again() {
        // A cube by the camera casts into the near two cascades. A frame that removes it plans
        // them drawn, emptied, but its passes never run (M1 review RA-001): the next frame
        // draws them again, clearing the cube's shadow, rather than take them for empty.
        let cube = [instance(1, 0, Vec3::ZERO)];
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&cube));
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&cube));
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&[]));
        assert_eq!(plan.drawn_mask(), 0b0011, "the near two, emptied");
        plan.not_drawn();
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&[]));
        assert_eq!(plan.drawn_mask(), 0b0011, "drawn again, though nothing casts into them");
        update(&mut plan, &SETTINGS, &packet(Vec::new()), &mut slots(&[]));
        assert_eq!(plan.drawn_mask(), 0, "and then left empty");
        // A step of the light fits every cascade anew: lost, they are all drawn again, where a
        // frame that ran leaves the far two to take turns.
        let mut slots = everywhere();
        let mut packet = packet(Vec::new());
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        packet.sun.direction = Vec3::new(0.6, -0.7, -0.45);
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111, "the step");
        plan.not_drawn();
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask(), 0b1111, "the step again");
        update(&mut plan, &SETTINGS, &packet, &mut slots);
        assert_eq!(plan.drawn_mask().count_ones(), 3, "then in turns");
    }

    const KEPT: ShadowSettings = ShadowSettings { cache: CascadeCache::Kept, ..SETTINGS };

    /// A cube by the camera and one far off, in the far cascades (M5 D8's fixtures).
    fn near_and_far() -> InstanceSlots {
        slots(&[instance(1, 0, Vec3::ZERO), instance(2, 0, Vec3::new(0.0, 0.0, -80.0))])
    }

    /// Plans `frames` frames, each drawn (its slots settled and the next frame's empty deltas applied
    /// after), and gives each one's mask.
    fn masks(
        plan: &mut ShadowPlan,
        settings: &ShadowSettings,
        packet: &FramePacket,
        slots: &mut InstanceSlots,
        frames: usize,
    ) -> Vec<u8> {
        (0..frames)
            .map(|_| {
                update(plan, settings, packet, slots);
                slots.settle_drawn();
                // The next frame's deltas, none, as the renderer applies them every frame.
                slots.apply(&InstanceDeltas { changed: Vec::new(), removed: Vec::new() });
                plan.drawn_mask()
            })
            .collect()
    }

    #[test]
    fn a_kept_cascade_is_drawn_again_only_once_something_in_it_changes() {
        let packet = packet(Vec::new());
        let mut slots = near_and_far();
        let mut plan = ShadowPlan::default();
        assert_eq!(masks(&mut plan, &KEPT, &packet, &mut slots, 1), [0b1111], "the first frame draws all");
        assert!(plan.casters[1].cascades & 0b1000 != 0, "the far cube is in the last cascade");
        assert_eq!(masks(&mut plan, &KEPT, &packet, &mut slots, 4), [0; 4], "nothing changed: nothing drawn");
        assert_eq!(plan.stats.cascades_cached, 4);
        // The far cube moves: the cascades it was in or is in now are drawn again, the fresh ones
        // at once and the cached ones on their turns, once each.
        let far = plan.casters[1].cascades;
        let moved = instance(2, 0, Vec3::new(1.0, 0.0, -80.0));
        slots.apply(&InstanceDeltas { changed: vec![moved], removed: Vec::new() });
        update(&mut plan, &KEPT, &packet, &mut slots);
        assert_eq!(plan.stats.moving_casters, 1);
        let far = far | plan.casters[1].cascades;
        let mut drawn = plan.drawn_mask();
        slots.settle_drawn();
        let after = masks(&mut plan, &KEPT, &packet, &mut slots, 4);
        assert_eq!(plan.stats.moving_casters, 0, "at rest");
        for mask in &after[..1] {
            assert_eq!(drawn & mask, 0, "each once: {drawn:#b} then {after:?}");
            drawn |= mask;
        }
        assert_eq!(drawn, far, "{after:?}");
        assert_eq!(after[1..], [0; 3]);
    }

    #[test]
    fn what_moves_by_itself_keeps_every_cascade_it_is_in_drawn_as_in_turns() {
        // A deformable instance reaching into every cascade changes its depth every frame without
        // the plan seeing it, so kept draws as in turns.
        let packet = packet(Vec::new());
        let (mut turns, mut kept) = (ShadowPlan::default(), ShadowPlan::default());
        let expected = masks(&mut turns, &SETTINGS, &packet, &mut everywhere(), 6);
        assert_eq!(masks(&mut kept, &KEPT, &packet, &mut everywhere(), 6), expected);
        assert_eq!(kept.stats.moving_casters, 1);
        assert_eq!(kept.stats.cached_redrawn, 1, "the cached cascade on its turn, its fit as it was");
    }

    #[test]
    fn foliage_in_the_wind_rests_in_a_kept_cascade_and_flutters_in_a_fresh_one() {
        // Its flutter is under the cached cascades' texels (the module docs): wind alone draws no
        // cached cascade again, and the fresh ones it is in every frame, as in turns.
        let mut slots = near_and_far();
        let mut plan = ShadowPlan::default();
        let mut frame = |plan: &mut ShadowPlan| {
            update_capped(plan, &KEPT, &packet(Vec::new()), &mut slots, MAX_CAPSULES, 1.0);
            slots.settle_drawn();
            slots.apply(&InstanceDeltas { changed: Vec::new(), removed: Vec::new() });
            plan.drawn_mask()
        };
        assert_eq!(frame(&mut plan), 0b1111);
        for _ in 0..3 {
            let fresh = plan.casters[0].cascades & 0b0011;
            assert_ne!(fresh, 0, "the near cube is in a fresh cascade");
            assert_eq!(frame(&mut plan), fresh, "the fresh cascades it is in, alone");
            assert_eq!(plan.stats.moving_casters, 0);
        }
    }

    #[test]
    fn the_cascades_a_lost_frame_drew_are_drawn_again_kept_as_in_turns() {
        // The view moves a little each frame. A lost frame's maps hold what they held, not what it
        // would have drawn: kept or in turns, every cascade it drew is fitted and drawn again with
        // the next frame, at the same fits.
        let walk = |step: f32| {
            let mut packet = packet(Vec::new());
            let eye = Vec3::new(step, 1.5, 4.0);
            packet.views[0].camera =
                CameraData::look_at(eye, eye - Vec3::new(0.0, 0.5, 4.0), Vec3::Y, 0.75, 1.5, 0.05);
            packet
        };
        let mut plans = [
            (ShadowPlan::default(), SETTINGS, near_and_far()),
            (ShadowPlan::default(), KEPT, near_and_far()),
        ];
        for (plan, settings, slots) in &mut plans {
            masks(plan, settings, &walk(0.0), slots, 2);
            update(plan, settings, &walk(0.5), slots);
            assert_eq!(plan.drawn_mask() & 0b0011, 0b0011, "{:?}: the fresh two refit", settings.cache);
            plan.not_drawn();
            slots.apply(&InstanceDeltas { changed: Vec::new(), removed: Vec::new() });
        }
        let [(turns, _, turns_slots), (kept, _, kept_slots)] = &mut plans;
        let lost = kept.drawn_mask();
        update(turns, &SETTINGS, &walk(0.5), turns_slots);
        update(kept, &KEPT, &walk(0.5), kept_slots);
        assert_eq!(kept.drawn_mask() & lost, lost, "every cascade the lost frame drew is drawn again");
        for index in 0..4 {
            if lost & 1 << index != 0 {
                assert_eq!(
                    kept.cached[index], turns.cached[index],
                    "cascade {index}: the fit drawing in turns takes"
                );
            }
        }
    }

    #[test]
    fn a_new_draw_list_or_a_changed_texture_draws_kept_cascades_again() {
        let packet = packet(Vec::new());
        let mut slots = near_and_far();
        let mut plan = ShadowPlan::default();
        // The far cube casts through an alpha-tested foliage pipeline.
        let mut list = DrawList::default();
        let cutout = SurfaceVariant::foliage(crate::pipelines::Coverage::AlphaTest);
        list.commands[0][cutout.index()].push(crate::scene::DrawCommand {
            indices: 0..3,
            first_vertex: 0,
            slot: 1,
            draw_id: 0,
            material: crate::materials::MaterialHandle::from_raw(0),
            mesh: MeshHandle::from_raw(0),
            submesh: 0,
            levels: 1,
            outline: false,
            foliage: 0,
        });
        plan.follow_list(0, &list);
        masks(&mut plan, &KEPT, &packet, &mut slots, 2);
        plan.follow_list(0, &list);
        assert_eq!(masks(&mut plan, &KEPT, &packet, &mut slots, 1), [0], "the same build");
        // A new build alone (meshes uploaded or moved) casts the same.
        let holding = plan.casters[0].cascades | plan.casters[1].cascades;
        plan.follow_list(1, &list);
        assert_eq!(masks(&mut plan, &KEPT, &packet, &mut slots, 2), [0, 0]);
        // The near cube leaves and another comes into its slot, the same size where it stood: the
        // cascades it is in are drawn again.
        let near = plan.casters[0].cascades;
        slots.apply(&InstanceDeltas { changed: Vec::new(), removed: vec![InstanceId(1)] });
        let left = masks(&mut plan, &KEPT, &packet, &mut slots, 2).iter().fold(0, |all, mask| all | mask);
        assert_eq!(left, near, "drawn without it");
        slots.apply(&InstanceDeltas { changed: vec![instance(3, 0, Vec3::ZERO)], removed: Vec::new() });
        assert_eq!(slots.meshed(), [0], "into its slot");
        let came = masks(&mut plan, &KEPT, &packet, &mut slots, 2).iter().fold(0, |all, mask| all | mask);
        assert_eq!(came, near, "drawn with it");
        // A texture changed: the cascades the cutout cube is in, and no others.
        let far = plan.casters[1].cascades;
        plan.textures_changed();
        let drawn = masks(&mut plan, &KEPT, &packet, &mut slots, 3).iter().fold(0, |all, mask| all | mask);
        assert_eq!(drawn, far);
        assert_eq!(masks(&mut plan, &KEPT, &packet, &mut slots, 2), [0, 0]);
        // A mesh or material replaced in place, or the shaders rebuilt: all of them.
        plan.forget_kept();
        assert_eq!(
            masks(&mut plan, &KEPT, &packet, &mut slots, 2).iter().fold(0, |all, mask| all | mask),
            holding
        );
    }

    const HELD: ShadowSettings = ShadowSettings { cache: CascadeCache::Held, ..SETTINGS };

    /// The fits `settings` gives `packet`'s view now, as drawing in turns takes them.
    fn view_fits(settings: &ShadowSettings, packet: &FramePacket) -> Vec<CascadeFit> {
        let camera = &packet.views[0].camera;
        let light = LightSpace::new(fit::quantize(-packet.sun.direction));
        let frustum = Frustum::of(camera);
        let cascades = settings.cascades as usize;
        let splits = fit::split_distances(frustum.near, settings.distance as f32, cascades);
        let mut from = frustum.near;
        splits[..cascades]
            .iter()
            .map(|&to| {
                let sphere = fit::slice_sphere(camera, &frustum, from, to);
                from = to;
                CascadeFit::new(light, sphere, settings.resolution)
            })
            .collect()
    }

    /// A view `x` meters along a walk across the light, looking down the street.
    fn walked(x: f32) -> FramePacket {
        let mut packet = packet(Vec::new());
        let eye = Vec3::new(x, 1.5, 4.0);
        packet.views[0].camera =
            CameraData::look_at(eye, eye + Vec3::new(0.3, -0.4, -4.0), Vec3::Y, 0.75, 1.5, 0.05);
        packet
    }

    #[test]
    fn a_held_cascade_keeps_its_fit_until_the_view_has_moved_a_64th_of_its_width() {
        // A walk of 30 m at 0.1 m a frame (6 m/s at 60 Hz): kept, a cached cascade takes the view's
        // fit on each of its turns; held, only once the view's has moved a 64th of its width, which
        // it never trails by more than that and the frames to its turn.
        let mut slots = near_and_far();
        let (mut kept, mut held) = (ShadowPlan::default(), ShadowPlan::default());
        let (mut kept_fits, mut held_fits) = ([0u32; 4], [0u32; 4]);
        let mut last = [None; 4];
        let step = 0.1;
        for frame in 0..300 {
            let packet = walked(step * frame as f32);
            update(&mut kept, &KEPT, &packet, &mut slots);
            update(&mut held, &HELD, &packet, &mut slots);
            let fits = view_fits(&HELD, &packet);
            for index in 2..4 {
                kept_fits[index] += u32::from(kept.drawn[index].is_some());
                let fit = held.cached[index].expect("fitted");
                held_fits[index] += u32::from(last[index] != Some(fit));
                last[index] = Some(fit);
                let behind = (fit.center.truncate() - fits[index].center.truncate()).abs().max_element();
                let most = HOLD_SHARE * 2.0 * fit.radius + 2.0 * step + fit.texel();
                assert!(behind <= most, "frame {frame}, cascade {index}: {behind} m behind, at most {most}");
            }
            slots.settle_drawn();
        }
        assert!(kept_fits[2..].iter().all(|&fits| fits >= 140), "kept: about every turn, {kept_fits:?}");
        for index in 2..4 {
            assert!(
                held_fits[index] * 3 < kept_fits[index],
                "cascade {index}: held {held_fits:?}, kept {kept_fits:?}"
            );
        }
        // The farthest cascade, about 85 m across here, takes a new fit every 2.7 m or so.
        assert!(held_fits[3] <= 30_u32.div_ceil(2) + 1, "{held_fits:?}");
    }

    #[test]
    fn a_held_cascade_takes_a_new_size_on_its_turn() {
        // The view stands still and widens its lens a little, so the far cascade grows while its
        // place barely moves: held, it takes the wider fit on its turn all the same, as in turns,
        // never keeping one too small for the view. (A step of the light forgets every fit.)
        let lens = |fov: f32| {
            let mut packet = packet(Vec::new());
            let eye = Vec3::new(0.0, 1.5, 4.0);
            packet.views[0].camera =
                CameraData::look_at(eye, eye - Vec3::new(0.0, 0.4, 4.0), Vec3::Y, fov, 1.5, 0.05);
            packet
        };
        let fov = (1..400)
            .map(|step| 0.75 + step as f32 * 0.0005)
            .find(|&fov| {
                let (a, b) = (view_fits(&HELD, &lens(0.75))[3], view_fits(&HELD, &lens(fov))[3]);
                b.radius > a.radius
                    && (a.center.truncate() - b.center.truncate()).abs().max_element() < HOLD_SHARE * a.radius
            })
            .expect("a lens where the far cascade grows in place");
        let mut slots = near_and_far();
        let mut plan = ShadowPlan::default();
        masks(&mut plan, &HELD, &lens(0.75), &mut slots, 4);
        assert_eq!(plan.cached[3], Some(view_fits(&HELD, &lens(0.75))[3]));
        masks(&mut plan, &HELD, &lens(fov), &mut slots, 2);
        assert_eq!(plan.cached[3], Some(view_fits(&HELD, &lens(fov))[3]), "the wider fit");
    }

    /// Whether `fit` holds light-space point `at` for a receiver, as `cascade_lookup` in `frame.wgsl`
    /// reads it: inside the square less its edge texels (3.5, `SHADOW_EDGE_TEXELS`), and not past
    /// its far plane. Returns how far past the square's reach it lies, in meters (0 inside).
    fn outside_by(fit: &CascadeFit, at: Vec3) -> f32 {
        let reach = fit.radius - 3.5 * fit.texel();
        let past = (at.truncate() - fit.center.truncate()).abs().max_element() - reach;
        if at.z < fit.far { f32::INFINITY } else { past.max(0.0) }
    }

    /// The receivers across `camera`'s view out to the shadow distance that its own fits cover and
    /// no cascade does with every cached one held `share` of its width behind, along one of eight
    /// directions: each one's distance from the camera and how far past the last cascade it lies.
    fn uncovered(settings: &ShadowSettings, camera: &CameraData, share: f32) -> Vec<(f32, f32)> {
        let mut packet = packet(Vec::new());
        packet.views[0].camera = *camera;
        let light = LightSpace::new(fit::quantize(-packet.sun.direction));
        let fits = view_fits(settings, &packet);
        let fresh = (settings.cascades - settings.cached_cascades) as usize;
        let (inverse, frustum, reach) =
            (camera.view.inverse(), Frustum::of(camera), settings.distance as f32);
        let mut found = Vec::new();
        for direction in 0..8 {
            // The largest offset along the direction whose larger component is `share` of the
            // width, in whole texels (as the fits snap).
            let angle = direction as f32 * std::f32::consts::FRAC_PI_4;
            let along = glam::Vec2::new(angle.cos(), angle.sin());
            let along = along / along.abs().max_element();
            let held: Vec<CascadeFit> = (fits.iter().enumerate())
                .map(|(index, fit)| {
                    let offset = (along * share * 2.0 * fit.radius / fit.texel()).round() * fit.texel();
                    let center = if index < fresh { fit.center } else { fit.center + offset.extend(0.0) };
                    CascadeFit { center, ..*fit }
                })
                .collect();
            for (u, v) in (0..=40).flat_map(|u| (0..=40).map(move |v| (u, v))) {
                let ray = Vec3::new(
                    (u as f32 / 20.0 - 1.0) * frustum.tangents.x,
                    (v as f32 / 20.0 - 1.0) * frustum.tangents.y,
                    -1.0,
                );
                for step in 0..400 {
                    let depth = frustum.near + (reach - frustum.near) * (step as f32 / 399.0).powi(2);
                    let world = inverse.transform_point3(ray * depth);
                    let distance = world.distance(camera.position);
                    if distance >= reach {
                        break;
                    }
                    let at = light.point(world);
                    let covered = |fits: &[CascadeFit]| fits.iter().any(|fit| outside_by(fit, at) == 0.0);
                    if covered(&fits) && !covered(&held) {
                        found.push((distance, outside_by(held.last().expect("a cascade"), at)));
                    }
                }
            }
        }
        found
    }

    #[test]
    fn a_held_fit_leaves_no_receiver_its_view_fit_covers_bar_a_sliver_in_the_fade() {
        // Points across the view out to the shadow distance, in light space; a receiver takes the
        // first cascade that holds it. Every cached cascade held a 64th of its width behind the
        // view's fit (the most `Held` lets it trail on its turn), along eight directions: whatever
        // the view's fits cover, the held ones still do, bar at most a sliver of the last cascade
        // in the distance's fade. Eight times as far behind, they would not.
        let views = [
            CameraData::look_at(Vec3::new(0.0, 1.5, 4.0), Vec3::new(0.0, 1.0, 0.0), Vec3::Y, 0.75, 1.5, 0.05),
            CameraData::look_at(
                Vec3::new(0.0, 1.7, 0.0),
                Vec3::new(10.0, 1.4, 3.0),
                Vec3::Y,
                0.75,
                16.0 / 9.0,
                0.1,
            ),
            CameraData::look_at(
                Vec3::new(0.0, 12.0, 0.0),
                Vec3::new(5.0, 0.0, -8.0),
                Vec3::Y,
                0.9,
                16.0 / 9.0,
                0.1,
            ),
        ];
        let phone =
            ShadowSettings { cascades: 2, resolution: 1024, distance: 50, cached_cascades: 1, ..HELD };
        for (name, settings) in [("desktop-high", HELD), ("mobile", phone)] {
            let reach = settings.distance as f32;
            let found: Vec<(f32, f32)> =
                views.iter().flat_map(|camera| uncovered(&settings, camera, HOLD_SHARE)).collect();
            for &(distance, past) in &found {
                assert!(
                    distance >= 0.9 * reach && past.is_finite(),
                    "{name}: {distance} m off, {past} m past"
                );
            }
            let worst = found.iter().map(|&(_, past)| past).fold(0.0, f32::max);
            assert!(worst <= 0.005 * reach, "{name}: a sliver of {worst} m");
            eprintln!("{name}: {} receivers uncovered, a sliver of at most {worst:.2} m", found.len());
            let further: usize =
                views.iter().map(|camera| uncovered(&settings, camera, 8.0 * HOLD_SHARE).len()).sum();
            assert!(further > 0, "{name}: an eighth of the width behind leaves receivers uncovered");
        }
    }

    /// `count` instances in a row, each listed as a character with a body of its own.
    fn crowd(count: u64) -> (Vec<MeshInstance>, Vec<CharacterInstance>) {
        let instances: Vec<MeshInstance> =
            (0..count).map(|index| instance(index, 0, Vec3::new(index as f32, 0.0, -2.0))).collect();
        let characters = instances
            .iter()
            .map(|instance| {
                let base = instance.transform.transform_point3(Vec3::ZERO);
                character(instance.id.0, CharacterBody::capsule(base, base + Vec3::Y, 0.25))
            })
            .collect();
        (instances, characters)
    }

    #[test]
    fn characters_past_the_entries_are_environment() {
        // The character buffer stops at the prepass IDs' 16,383 entries: the instances past them
        // shade as the world does, so they cast as the world does (M1 review RS-005).
        let cap = ID_ENTRY_MASK as u64;
        for count in [cap, cap + 1, cap + 117] {
            let (instances, characters) = crowd(count);
            let mut slots = slots(&instances);
            let mut plan = ShadowPlan::default();
            update(&mut plan, &SETTINGS, &packet(characters), &mut slots);
            let listed = (0..count as u32).filter(|&slot| slots.character(slot) != 0).count();
            assert_eq!(listed as u64, count.min(cap), "{count}: the entries there are");
            for slot in 0..count as u32 {
                assert_eq!(plan.is_character(slot), slots.character(slot) != 0, "{count}: slot {slot}");
            }
            assert_eq!(plan.groups.len() as u64, count.min(cap), "{count}: a group per character");
        }
    }

    #[test]
    fn an_instance_listed_again_casts_only_the_body_its_entry_has() {
        // Listed twice in two states, the instance reads its second entry: its first body is no
        // one's, and casts nothing.
        let body = CharacterBody::capsule(Vec3::ZERO, Vec3::Y, 0.25);
        let moved = CharacterBody::capsule(Vec3::X, Vec3::X + Vec3::Y, 0.25);
        let twice = packet(vec![character(1, body), character(1, moved)]);
        let mut one = slots(&[instance(1, 0, Vec3::ZERO)]);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &twice, &mut one);
        assert_eq!(plan.maps.len(), 1, "one character, one map");
        let mobile = ShadowSettings { character_shadow: false, ..SETTINGS };
        update(&mut plan, &mobile, &twice, &mut one);
        assert_eq!(plan.uniforms.counts[2], 1, "one capsule");
        assert_eq!(plan.uniforms.capsules[0].base, [1.0, 0.0, 0.0, 0.25], "the last body's");
        // At the last entry there is, the second listing finds no room: the instance keeps its
        // first entry, and casts with that body. A part listed later in the first listing's
        // state shares the entry, room or not.
        let cap = ID_ENTRY_MASK as u64;
        let (mut instances, mut characters) = crowd(cap - 1);
        instances.extend([instance(cap, 0, Vec3::ZERO), instance(cap + 1, 0, Vec3::Y)]);
        characters.extend([character(cap, body), character(cap, moved), character(cap + 1, body)]);
        let mut slots = slots(&instances);
        update(&mut plan, &mobile, &packet(characters), &mut slots);
        let [last, part] = [cap, cap + 1].map(|id| slots.slot(InstanceId(id)).expect("live"));
        assert_eq!([last, part].map(|slot| slots.character(slot)), [ID_ENTRY_MASK; 2], "the last entry");
        let SlotCharacter::Character(Some(group)) = plan.characters[last as usize] else {
            panic!("a character with a body: {:?}", plan.characters[last as usize]);
        };
        assert_eq!(plan.groups[group as usize].body, body, "the body of the entry it reads");
        assert_eq!(plan.characters[part as usize], SlotCharacter::Character(Some(group)), "one character");
        assert!(plan.groups.iter().all(|group| group.body != moved), "its second body is no one's");
    }

    #[test]
    fn more_bodies_than_a_frame_holds_plan_without_panicking() {
        // 70,000 distinct bodies, past the 2¹⁶ groups a 16-bit index held: the frame's
        // characters stop at the entries there are, and so do its groups.
        let (instances, characters) = crowd(70_000);
        let mut slots = slots(&instances);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(characters), &mut slots);
        assert_eq!(plan.groups.len(), ID_ENTRY_MASK as usize);
        assert_eq!(plan.maps.len(), MAX_CHARACTER_MAPS);
    }

    /// A frame of named characters standing with their bodies' centers at `centers` (world space),
    /// each a capsule 1.6 m tall, under a sun shining along `sun`, its instance a unit cube at the
    /// center; and the light space it plans in.
    fn party(centers: &[Vec3], sun: Vec3) -> (FramePacket, InstanceSlots, LightSpace) {
        let instances: Vec<MeshInstance> =
            centers.iter().enumerate().map(|(id, &at)| instance(id as u64, 0, at)).collect();
        let characters = centers
            .iter()
            .enumerate()
            .map(|(id, &at)| {
                let body = CharacterBody::capsule(at - 0.55 * Vec3::Y, at + 0.55 * Vec3::Y, 0.25);
                character(id as u64, body)
            })
            .collect();
        let mut packet = packet(characters);
        packet.sun.direction = sun;
        (packet, slots(&instances), LightSpace::new(fit::quantize(-sun)))
    }

    /// The weight receiver `receiver` gives each group's shadow this frame, by group: 0 for those it
    /// doesn't receive.
    fn weights(plan: &ShadowPlan, receiver: usize) -> Vec<f32> {
        let received = plan.received[receiver];
        plan.groups
            .iter()
            .map(|group| {
                let caster = match group.cast {
                    Cast::Map(map) => u32::from(map),
                    Cast::Capsule(capsule) => u32::from(capsule) | RECEIVED_CAPSULE,
                    Cast::Nothing => return 0.0,
                };
                (0..MAX_OTHERS)
                    .filter(|&slot| received.casters[slot] == caster)
                    .map(|slot| received.weights[slot])
                    .sum()
            })
            .collect()
    }

    /// A point `along` meters toward the light from `from`, and `across` meters across it.
    fn toward(light: LightSpace, from: Vec3, along: f32, across: f32) -> Vec3 {
        from + light.z * along + light.x * across
    }

    #[test]
    fn a_party_under_a_low_sun_receives_each_others_shadows_and_never_its_own() {
        // A low sun from the side: three characters in a row along it, each shading the next.
        let sun = Vec3::new(-1.0, -0.35, 0.0);
        let light = LightSpace::new(fit::quantize(-sun));
        let first = Vec3::new(0.0, 0.8, -3.0);
        let centers = [first, toward(light, first, 1.2, 0.0), toward(light, first, 2.4, 0.0)];
        for (settings, maps) in [(SETTINGS, 3), (ShadowSettings { character_shadow: false, ..SETTINGS }, 0)] {
            let (packet, mut slots, _) = party(&centers, sun);
            let mut plan = ShadowPlan::default();
            update(&mut plan, &settings, &packet, &mut slots);
            assert_eq!(plan.maps.len(), maps);
            // Those toward the light shade a receiver whole, and so does the one just behind it,
            // whose sphere still reaches past the receiver's back; the last, toward the light,
            // takes nothing from the first, wholly behind it. None takes its own.
            let expected = [[0.0, 1.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 0.0]];
            for (receiver, expected) in expected.iter().enumerate() {
                assert_eq!(weights(&plan, receiver), expected, "{receiver} receives");
                let slot = plan.received[receiver].casters[0];
                assert_eq!(slot & RECEIVED_CAPSULE != 0, maps == 0, "tiles on desktop, capsules on phones");
            }
            assert_eq!(plan.stats.received, 5);
        }
    }

    #[test]
    fn no_slots_and_ambient_characters_receive_nothing_and_ambient_ones_cast_capsules() {
        let sun = Vec3::new(-1.0, -0.35, 0.0);
        let light = LightSpace::new(fit::quantize(-sun));
        let first = Vec3::new(0.0, 0.8, -3.0);
        let centers = [first, toward(light, first, 1.2, 0.0)];
        let (packet, mut slots, _) = party(&centers, sun);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &ShadowSettings { others: 0, ..SETTINGS }, &packet, &mut slots);
        assert!(plan.received.iter().all(|received| *received == GpuReceived::default()));
        assert_eq!(plan.stats.received, 0);
        // The second is a crowd's: it draws no map and receives nothing; the first takes its capsule.
        let mut crowd = packet.clone();
        crowd.characters[1].body = crowd.characters[1].body.with_role(ShadowRole::Ambient);
        update(&mut plan, &SETTINGS, &crowd, &mut slots);
        assert_eq!(plan.maps.len(), 1, "the named one's alone");
        assert_eq!(plan.groups[1].cast, Cast::Capsule(0));
        assert_eq!(plan.received[1], GpuReceived::default(), "an ambient character receives none");
        assert_eq!(plan.received[0].casters[0], RECEIVED_CAPSULE, "its capsule");
        assert_eq!(plan.received[0].weights[0], 1.0);
    }

    #[test]
    fn a_caster_falls_on_a_receiver_only_where_its_shadow_can_reach_it() {
        let receiver = (Vec3::ZERO, 1.0);
        let at = |x: f32, z: f32| falls_on(receiver, (Vec3::new(x, 0.0, z), 1.0), 10.0);
        assert_eq!(at(0.0, 2.0), 1.0, "toward the light, over it");
        assert_eq!(at(2.0, 2.0), 0.0, "discs only touching in the light's view");
        assert_eq!(at(1.75, 2.0), 1.0, "overlapping by the band");
        assert!((0.0..1.0).contains(&at(1.9, 2.0)) && at(1.9, 2.0) > 0.0, "fading in between");
        assert_eq!(at(0.0, -2.0), 0.0, "wholly behind it");
        assert_eq!(at(0.0, -1.75), 1.0, "reaching past its back by the band");
        assert_eq!(at(0.0, 12.0), 0.0, "the receiver wholly past its reach");
        assert_eq!(at(0.0, 11.75), 1.0);
    }

    /// The pick never pops (M5 L12): walked in centimetre steps along every edge a caster can cross
    /// (into a receiver's slot past another, into the light's view of it, behind it), no caster's
    /// weight on a receiver moves by more than a smooth fade's step, and each crossing happens.
    #[test]
    fn a_receivers_casters_never_pop_as_they_move() {
        let sun = Vec3::new(-0.8, -0.5, 0.3);
        let light = LightSpace::new(fit::quantize(-sun));
        let receiver = Vec3::new(0.0, 0.8, -4.0);
        let one_slot = ShadowSettings { others: 1, ..SETTINGS };
        // How far a weight may move in a centimetre: the steepest of the fades, smoothstep's slope
        // of 1.5 over the narrower band, with room.
        let most = 1.5 / FALLS_ON_BAND * 0.01 * 1.2;
        let walk = |settings: &ShadowSettings, place: &dyn Fn(f32) -> Vec<Vec3>, steps: usize| {
            let mut plan = ShadowPlan::default();
            let mut before: Option<Vec<f32>> = None;
            let mut seen = Vec::new();
            for step in 0..=steps {
                let (packet, mut slots, _) = party(&place(step as f32 * 0.01), sun);
                update(&mut plan, settings, &packet, &mut slots);
                let now = weights(&plan, 0);
                if let Some(before) = &before {
                    for (caster, (was, is)) in before.iter().zip(&now).enumerate() {
                        assert!((was - is).abs() <= most, "step {step}: caster {caster} {was} to {is}");
                    }
                }
                seen.push(now.clone());
                before = Some(now);
            }
            seen
        };
        // Two casters toward the light and one slot: the far one walks in past the near one.
        let near = toward(light, receiver, 1.5, 0.0);
        let seen =
            walk(&one_slot, &|walked| vec![receiver, near, toward(light, receiver, 3.0 - walked, 0.1)], 250);
        let (first, last) = (&seen[0], &seen[seen.len() - 1]);
        assert!(first[1] == 1.0 && first[2] == 0.0, "the near one first: {first:?}");
        assert!(last[1] == 0.0 && last[2] == 1.0, "then the walker: {last:?}");
        // One caster crossing into the light's view of the receiver, then wholly behind it.
        let seen = walk(&SETTINGS, &|walked| vec![receiver, toward(light, receiver, 1.5, 2.5 - walked)], 150);
        assert!(seen[0][1] == 0.0 && seen[150][1] == 1.0, "{:?} to {:?}", seen[0], seen[150]);
        // (A body just where the receiver's would be one character with it: the walk starts 10 cm off.)
        let seen =
            walk(&SETTINGS, &|walked| vec![receiver, toward(light, receiver, -0.1 - walked, 0.0)], 300);
        assert!(seen[0][1] == 1.0 && seen[300][1] == 0.0, "{:?} to {:?}", seen[0], seen[300]);
    }

    #[test]
    fn the_plan_classifies_and_groups_by_the_character_buffers_entries() {
        // The cross-check: a frame lists one character in two parts (one state), one instance
        // twice in two states, one character with no body, then one first listed before them
        // all, an instance that isn't drawn and lighting that isn't valid. Each instance casts
        // as the entry it reads makes it, and the groups come in entry order, which is the order
        // the packet first lists their bodies in.
        let at = |x: f32| CharacterBody::capsule(Vec3::new(x, 0.0, -2.0), Vec3::new(x, 1.0, -2.0), 0.25);
        let instances: Vec<MeshInstance> =
            (0..6).map(|id| instance(id, 0, Vec3::new(id as f32, 0.0, -2.0))).collect();
        let mut invalid = character(5, at(5.0));
        invalid.lighting.rim = f32::NAN;
        let characters = vec![
            character(3, at(3.0)),
            character(0, at(0.0)),
            character(1, at(0.0)),
            character(2, at(2.0)),
            character(2, at(7.0)),
            character(4, CharacterBody::default()),
            character(9, at(9.0)),
            invalid,
        ];
        let mut slots = slots(&instances);
        let mut plan = ShadowPlan::default();
        update(&mut plan, &SETTINGS, &packet(characters), &mut slots);
        let bodies: Vec<CharacterBody> = plan.groups.iter().map(|group| group.body).collect();
        assert_eq!(bodies, [at(3.0), at(0.0), at(7.0)]);
        assert_eq!((0..6).map(|slot| slots.character(slot)).collect::<Vec<_>>(), [2, 2, 4, 1, 5, 0]);
        use SlotCharacter::{Character, Environment};
        let expected = [Character(Some(1)), Character(Some(1)), Character(Some(2)), Character(Some(0))];
        assert_eq!(plan.characters[..4], expected);
        assert_eq!(plan.characters[4..6], [Character(None), Environment]);
        for slot in 0..6 {
            assert_eq!(plan.is_character(slot), slots.character(slot) != 0, "slot {slot}");
        }
        assert_eq!(plan.maps.len(), 3, "a map each");
    }

    /// Each cascade draws a caster at the level its own texel says (M5 D4): a quarter of a texel ×
    /// 2^bias of error, times the instance's stretch, with no memory; level 0 for what draws level 0
    /// alone, and in a mode that forces one, that one.
    #[test]
    fn each_cascade_draws_the_level_its_texel_says() {
        let instances: Vec<MeshInstance> =
            (0..40).map(|id| instance(id, 0, Vec3::new(0.0, 0.5, 2.0 - 3.0 * id as f32))).collect();
        let mut slots = slots(&instances);
        let packet = packet(Vec::new());
        let mut characters = CharacterList::default();
        characters.update(&packet, &mut slots, false, None);
        let mut list = DrawList::default();
        for (slot, _) in slots.live() {
            list.commands[0][SurfaceVariant::OPAQUE.index()].push(crate::scene::DrawCommand {
                indices: 0..3,
                first_vertex: 0,
                slot,
                draw_id: slot,
                material: crate::materials::MaterialHandle::from_raw(0),
                mesh: MeshHandle::from_raw(0),
                submesh: 0,
                levels: 0b11,
                outline: false,
                foliage: 0,
            });
        }
        // Slot 1 draws level 0 alone, as a character's part or foliage would.
        let mut culled = crate::view_cull::SlotCull::at_levels(&[0; 40]);
        culled[1] = crate::view_cull::SlotCull::whole();
        let plan_with = |meshes: &[MeshRecord], levels: LevelSettings| {
            let mut plan = ShadowPlan::default();
            let cull = CasterCull { culled: Culled::new(&culled), least_texels: 0.0, levels, pose: None };
            let table = characters.table();
            plan.update(
                &SETTINGS,
                &packet,
                &slots,
                meshes,
                &|_| 0.0,
                &list,
                table,
                (4096, 2048),
                cull,
                MAX_CAPSULES,
            );
            plan
        };
        let mut meshes = meshes();
        let fits = plan_with(&meshes, LevelSettings::ON).drawn;
        let texel = |index: usize| fits[index].expect("the first frame draws every cascade").texel();
        assert!(texel(0) < texel(1) && texel(1) < texel(3));
        // An error between cascade 0's quarter texel and cascade 1's.
        let error = 0.25 * (texel(0) * texel(1)).sqrt();
        meshes[0].lod_errors = vec![0.0, error];
        // Cascade 0 keeps level 0 and the others take level 1; a bias of −2 (a sixteenth of a
        // texel) keeps level 0 further out; forced off, level 0 everywhere.
        let biased = LevelSettings { bias: -2.0, ..LevelSettings::ON };
        let off = LevelSettings { mode: crate::LevelsMode::Off, bias: 0.0 };
        for (levels, expect) in [(LevelSettings::ON, Some([0, 1, 1, 1])), (biased, None), (off, Some([0; 4]))]
        {
            let plan = plan_with(&meshes, levels);
            let mut seen = [false; MAX_CASCADES];
            for (slot, caster) in plan.casters.iter().enumerate() {
                for (index, fit) in plan.drawn.iter().enumerate() {
                    let Some(fit) = fit else { continue };
                    if caster.cascades & (1 << index) == 0 {
                        assert_eq!(caster.levels[index], 0, "slot {slot}: not drawn into {index}");
                        continue;
                    }
                    let said = levels::cascade_level(&meshes[0].lod_errors, 1.0, fit.texel(), levels.bias);
                    let level = match (slot, expect) {
                        (1, _) => 0,
                        (_, Some(expect)) => expect[index],
                        (_, None) => said,
                    };
                    assert_eq!(caster.levels[index], level, "slot {slot}, cascade {index}, {levels:?}");
                    if slot != 1 && levels.mode == crate::LevelsMode::On {
                        assert_eq!(caster.levels[index], said, "slot {slot}, cascade {index}: its texel's");
                    }
                    seen[index] |= slot != 1;
                }
            }
            assert!(seen.iter().all(|&seen| seen), "a caster in every cascade: {seen:?}");
        }
        // A bias of −2 needs sixteen times cascade 0's texel to take level 1.
        let biased_level =
            |index: usize| levels::cascade_level(&meshes[0].lod_errors, 1.0, texel(index), -2.0);
        assert_eq!(biased_level(0), 0);
        assert_eq!(biased_level(3), u8::from(texel(3) >= 4.0 * (texel(0) * texel(1)).sqrt()));
    }
}
