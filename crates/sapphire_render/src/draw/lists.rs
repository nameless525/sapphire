//! Each view's own draw list (M5 D1, spec §5.12 "Scene and draws"): the draws one view's passes
//! draw, in the order they draw them, so a pass records by walking its list and tests nothing.
//!
//! - **The order is one seam,** [`ViewKind::key`], worked out when the draw list is rebuilt
//!   ([`Orders`]). Draws already come grouped by pipeline: per position format (or deformed),
//!   surface variant and set ([`DrawSet`]), each group drawn through one pipeline in a fixed order
//!   a pass's code walks. Within a group, the views that write depth alone (each cascade, the
//!   character maps) draw the static depth-writing surfaces by [`DrawKey`]: mesh, then the draw's
//!   record, since there an exact tie in depth writes the same depth whichever draws first. **The
//!   views that write anything else keep the draw list's order (record):** the main view and each
//!   inset, whose pass clears its own depth and keeps the first-drawn surface at an exact tie, and
//!   both prepasses, which write normals, the outline mask and the characters' IDs where the same
//!   ties fall. Reordering one moves those pixels (spec §5.12, 124 of 400 Metal captures under a
//!   material-first key); m5-plan D2 decides the reorder. Groups whose order shows in any view
//!   keep it everywhere: transparent surfaces and eyes' layers blend in it, eyes through hair and
//!   outline hulls draw at depths that tie, and skinned or morphed instances bind by instance.
//! - **A view's list is compacted each frame** ([`ViewList`]): walking its groups in key order, it
//!   keeps the draws that stand in the view and pass its tests (the main view's frustum, an outline
//!   hull's cutoff, a cascade's casters, a character map's character, the listed characters) and
//!   counts the rest by the test that left them out, as the passes counted them before. The lists
//!   keep their capacity, so a warm frame allocates nothing; the shadow views are compacted in one
//!   walk ([`ViewLists::compact_casters`]).
//! - **An entry is a position in its group** of the draw list (`DrawList::commands` or
//!   `DrawList::deformed`), whose command carries what later work reads: its instance's slot (whose
//!   bounds `InstanceSlots::world_bounds` gives), its mesh, its material and its record. Every draw
//!   draws its mesh's level 0 until levels are chosen (m5-plan D4), which the key will then hold
//!   after the mesh.
//!
//! The filtered walk the passes made before stays reachable behind
//! [`Renderer::walk_filtered`](crate::Renderer::walk_filtered), for the test that draws scenes both
//! ways and compares every byte; m5-plan D2 deletes it.

use sapphire_assets::material::MaterialFlags;
use sapphire_assets::mesh::PositionFormat;

use super::DrawSet;
use crate::counts::PassCounts;
use crate::inset::MAX_INSETS;
use crate::materials::{MaterialHandle, MaterialRecord};
use crate::pipelines::{SurfaceVariant, format_index};
use crate::scene::{DeformedDraw, DrawCommand, DrawList};
use crate::shadows::{MAX_CASCADES, MAX_CHARACTER_MAPS};

/// The variants whose static surfaces can sort: the depth-writing ones, which come first in
/// [`SurfaceVariant::ALL`].
const SORTED: usize = SurfaceVariant::DEPTH_WRITING.len();
const FORMATS: usize = PositionFormat::ALL.len();
const KINDS: usize = ViewKind::ALL.len();
/// Static draws per format, then deformed ones.
const SOURCES: usize = PositionFormat::ALL.len() + 1;
const SETS: usize = 3;
const VARIANTS: usize = SurfaceVariant::ALL.len();
/// Every group a list can hold: per source, variant and set.
const GROUPS: usize = SOURCES * VARIANTS * SETS;

/// A sorted group's order: the key of one of its draws ([`ViewKind::key`]). Fields compare in
/// order (the derived `Ord`), as plain integers:
///
/// | field | what | why |
/// |---|---|---|
/// | (group) | the pipeline: format or deformed, surface variant, set | held apart, as the list's groups, which a pass walks in its own fixed order |
/// | `mesh` | the mesh's handle index | each run binds the mesh's streams once; later, a run of one mesh and level is an instanced draw (m5-plan D2) |
/// | `draw` | the draw's record (`DrawCommand::draw_id`) | today's order: unique in its group, so the order is total and the same whatever sorts it |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DrawKey {
    pub mesh: u32,
    pub draw: u32,
}

/// The sorted groups' orders: per [`ViewKind`] that sorts, format and depth-writing variant, the
/// positions of the variant's static commands in key order. Built with the draw list, so a frame
/// that only moves instances sorts nothing.
#[derive(Debug, Default)]
pub(crate) struct Orders {
    sorted: [[[Vec<u32>; SORTED]; FORMATS]; KINDS],
    /// Scratch for the sort, kept so a rebuild allocates nothing new.
    keys: Vec<(DrawKey, u32)>,
}

impl Orders {
    /// Sorts every sortable group of `commands`, the draw list's static commands, for each view
    /// kind that keys them.
    pub(crate) fn build(&mut self, commands: &[[Vec<DrawCommand>; VARIANTS]; FORMATS]) {
        profiling::scope!("Orders::build");
        let Self { sorted, keys } = self;
        for (kind, sorted) in ViewKind::ALL.into_iter().zip(sorted) {
            for (commands, sorted) in commands.iter().zip(sorted) {
                // The depth-writing variants alone: zip stops at the sorted ones.
                for (commands, sorted) in commands.iter().zip(sorted) {
                    keys.clear();
                    sorted.clear();
                    let mut keyed = true;
                    for (command, index) in commands.iter().zip(0..) {
                        let Some(key) = kind.key(command) else {
                            // A kind that keeps the list's order keys nothing, and lists no order.
                            keyed = false;
                            break;
                        };
                        keys.push((key, index));
                    }
                    if !keyed {
                        continue;
                    }
                    // Keys are unique (their records are), so unstable is stable.
                    keys.sort_unstable();
                    sorted.extend(keys.iter().map(|&(_, index)| index));
                }
            }
        }
    }

    /// `variant`'s static commands in `format` in `kind`'s key order, if `kind` sorts them.
    pub(crate) fn sorted(
        &self,
        kind: ViewKind,
        format: PositionFormat,
        variant: SurfaceVariant,
    ) -> Option<&[u32]> {
        let sorted = self.sorted[kind_index(kind)][format_index(format)].get(variant.index())?;
        (!sorted.is_empty()).then_some(sorted.as_slice())
    }
}

/// Where a group's draws come from: a format's static meshes, or skinned and morphed instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Static(PositionFormat),
    Deformed,
}

impl Source {
    fn index(self) -> usize {
        match self {
            Self::Static(format) => format_index(format),
            Self::Deformed => PositionFormat::ALL.len(),
        }
    }

    fn of(index: usize) -> Self {
        PositionFormat::ALL.get(index).map_or(Self::Deformed, |&format| Self::Static(format))
    }
}

fn set_index(set: DrawSet) -> usize {
    match set {
        DrawSet::Surfaces => 0,
        DrawSet::Outlines => 1,
        DrawSet::ThroughHair => 2,
    }
}

const SETS_BY_INDEX: [DrawSet; SETS] = [DrawSet::Surfaces, DrawSet::Outlines, DrawSet::ThroughHair];

/// A group's index among [`GROUPS`].
fn group(source: Source, variant: SurfaceVariant, set: DrawSet) -> usize {
    (source.index() * VARIANTS + variant.index()) * SETS + set_index(set)
}

/// The group at `index`.
fn group_of(index: usize) -> (Source, SurfaceVariant, DrawSet) {
    let (set, variant) = (index % SETS, index / SETS % VARIANTS);
    (Source::of(index / SETS / VARIANTS), SurfaceVariant::ALL[variant], SETS_BY_INDEX[set])
}

/// The kinds of view, by the groups their passes walk and the order they draw them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewKind {
    /// The main pass (`draw_main_pass`), an inset's and the debug views' replays: every surface
    /// but fog cards (which draw in an order of the frame's own), the outline hulls, eyes through
    /// hair.
    Main,
    /// The scene prepass: the shared depth variants, foliage's (static alone), the outline hulls.
    ScenePrepass,
    /// The character prepass, the world's and an inset's: the anime character surfaces that
    /// write depth.
    CharacterPrepass,
    /// The shadow casters, every cascade and character map: as the scene prepass's surfaces.
    Casters,
}

impl ViewKind {
    const ALL: [Self; 4] = [Self::Main, Self::ScenePrepass, Self::CharacterPrepass, Self::Casters];

    /// **The order's one seam:** the key this kind of view draws `command` by within its group (a
    /// static depth-writing surface's), or `None` where it keeps the draw list's order, the
    /// record. A kind answers alike for every command: all keyed, or none.
    ///
    /// - The shadow views write depth alone, which an exact tie writes the same whichever draws
    ///   first: mesh, then record ([`DrawKey`]), so each run binds a mesh's streams once.
    /// - The main view and each inset keep the record: their pass clears its own depth and keeps
    ///   the first-drawn surface at an exact tie (reverse-Z, `Greater`), so a reorder moves those
    ///   pixels. The scene prepass writes normals and the outline mask, and the character prepass
    ///   the characters' IDs, where the same ties fall: they keep it too. Which reorder they
    ///   take, and the tests and goldens it moves, is m5-plan D2's to decide, here.
    pub(crate) fn key(self, command: &DrawCommand) -> Option<DrawKey> {
        match self {
            Self::Casters => Some(DrawKey { mesh: to_u32(command.mesh.index()), draw: command.draw_id }),
            Self::Main | Self::ScenePrepass | Self::CharacterPrepass => None,
        }
    }

    /// Whether this view's passes walk `set` of `variant`'s draws from `source`: what each pass's
    /// code draws, which a list must hold exactly, so it counts what the pass looked at.
    fn walks(self, source: Source, variant: SurfaceVariant, set: DrawSet) -> bool {
        let foliage = source != Source::Deformed && SurfaceVariant::FOLIAGE.contains(&variant);
        match (self, set) {
            (Self::Main, DrawSet::Surfaces) => variant != SurfaceVariant::FOG_CARD,
            (Self::Main | Self::ScenePrepass, DrawSet::Outlines) => {
                SurfaceVariant::OUTLINED.contains(&variant)
            }
            (Self::Main, DrawSet::ThroughHair) => SurfaceVariant::DRAWS_THROUGH_HAIR.contains(&variant),
            (Self::ScenePrepass | Self::Casters, DrawSet::Surfaces) => {
                SurfaceVariant::SHARED_DEPTH.contains(&variant) || foliage
            }
            (Self::CharacterPrepass, DrawSet::Surfaces) => {
                SurfaceVariant::CHARACTER_DEPTH_WRITING.contains(&variant)
            }
            _ => false,
        }
    }

    /// The groups this view's passes walk, by index.
    fn groups(self) -> Vec<u16> {
        (0..GROUPS)
            .filter(|&index| {
                let (source, variant, set) = group_of(index);
                self.walks(source, variant, set)
            })
            .map(|index| u16::try_from(index).expect("a few hundred groups"))
            .collect()
    }
}

/// A draw the list compacts: a static mesh's command or a deformed instance's.
trait Listed {
    /// Static: the main view culls it by its bounds.
    const STATIC: bool;
    fn slot(&self) -> u32;
    fn draw_id(&self) -> u32;
    fn material(&self) -> MaterialHandle;
    fn outline(&self) -> bool;
}

impl Listed for DrawCommand {
    const STATIC: bool = true;
    fn slot(&self) -> u32 {
        self.slot
    }
    fn draw_id(&self) -> u32 {
        self.draw_id
    }
    fn material(&self) -> MaterialHandle {
        self.material
    }
    fn outline(&self) -> bool {
        self.outline
    }
}

impl Listed for DeformedDraw {
    const STATIC: bool = false;
    fn slot(&self) -> u32 {
        self.slot
    }
    fn draw_id(&self) -> u32 {
        self.draw_id
    }
    fn material(&self) -> MaterialHandle {
        self.material
    }
    fn outline(&self) -> bool {
        self.outline
    }
}

/// What a view's draws are tested by as its list is compacted: the tests its passes made as they
/// drew, in the same order.
pub(crate) struct ViewTests<'a, I> {
    /// Per instance slot, the view it stands in (M3 U6); slots past its end stand in the world's.
    pub stands: &'a [u16],
    /// The view: only the draws of instances that stand in it are listed, or looked at.
    pub stand: u16,
    /// Per draw record, whether the frame draws its outline hull (`outlines::OutlineCull`).
    pub hulls: &'a [bool],
    /// Every uploaded material, by handle: which draw eyes through hair.
    pub materials: &'a [MaterialRecord],
    /// Per instance slot, whether the main view draws its static draws (`view_cull`); `None`
    /// draws every one.
    pub view: Option<&'a [bool]>,
    /// The pass's own test of an instance slot (the listed characters).
    pub instance: I,
}

/// Whether a set holds a draw, as `MeshDraws::includes` says.
enum Pick {
    In,
    Out,
    Cutoff,
}

impl<'a, I> ViewTests<'a, I> {
    /// These tests, with `instance` for the pass's own.
    pub(crate) fn with_instance<J>(&self, instance: J) -> ViewTests<'a, J> {
        let Self { stands, stand, hulls, materials, view, .. } = *self;
        ViewTests { stands, stand, hulls, materials, view, instance }
    }

    fn stands_here(&self, slot: u32) -> bool {
        self.stands.get(slot as usize).copied().unwrap_or(0) == self.stand
    }

    fn pick(&self, set: DrawSet, draw: &impl Listed) -> Pick {
        match set {
            DrawSet::Surfaces => Pick::In,
            DrawSet::Outlines if !draw.outline() => Pick::Out,
            DrawSet::Outlines if self.hulls.get(draw.draw_id() as usize).copied().unwrap_or(false) => {
                Pick::In
            }
            DrawSet::Outlines => Pick::Cutoff,
            DrawSet::ThroughHair => {
                let material = self.materials.get(draw.material().index());
                if material.is_some_and(|record| record.flags.contains(MaterialFlags::THROUGH_HAIR)) {
                    Pick::In
                } else {
                    Pick::Out
                }
            }
        }
    }

    fn in_view(&self, slot: u32) -> bool {
        self.view.is_none_or(|view| view.get(slot as usize).is_none_or(|&drawn| drawn))
    }
}

/// One view's draws this frame, group by group, each in its key's order.
#[derive(Debug)]
pub(crate) struct ViewList {
    /// Positions in their groups of the draw list, group after group.
    draws: Vec<u32>,
    /// Per group ([`GROUPS`]), its draws' range in `draws`; empty for one the view doesn't walk.
    ranges: Box<[[u32; 2]; GROUPS]>,
    /// The draws the view's passes look at and don't draw, by the test that left them out: what
    /// each pass adds to its counts once (`MeshDraws::count_left_out`). Their draws count what
    /// they walk.
    left_out: PassCounts,
}

impl Default for ViewList {
    fn default() -> Self {
        Self { draws: Vec::new(), ranges: Box::new([[0; 2]; GROUPS]), left_out: PassCounts::default() }
    }
}

impl ViewList {
    /// Starts the frame's list over, keeping its capacity.
    fn clear(&mut self) {
        self.draws.clear();
        self.ranges.fill([0; 2]);
        self.left_out = PassCounts::default();
    }

    /// The listed draws of `set` of `variant`'s from `source`: positions in the draw list's group.
    pub(crate) fn group(&self, source: Source, variant: SurfaceVariant, set: DrawSet) -> &[u32] {
        let [start, end] = self.ranges[group(source, variant, set)];
        &self.draws[start as usize..end as usize]
    }

    /// Whether the view draws any of `variant`'s draws from `source`, of any set.
    pub(crate) fn has(&self, source: Source, variant: SurfaceVariant) -> bool {
        let first = group(source, variant, DrawSet::Surfaces);
        self.ranges[first..first + SETS].iter().any(|[start, end]| start != end)
    }

    /// What the view's passes look at and don't draw.
    pub(crate) fn left_out(&self) -> &PassCounts {
        &self.left_out
    }

    /// The draws listed.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.draws.len()
    }

    fn end(&self) -> u32 {
        to_u32(self.draws.len())
    }

    /// Lists the draws of `groups` (of `kind`'s) of `list` that pass `tests`.
    fn compact<I: Fn(u32) -> bool>(
        &mut self,
        kind: ViewKind,
        groups: &[u16],
        list: &DrawList,
        tests: &ViewTests<'_, I>,
    ) {
        self.clear();
        for &index in groups {
            let index = usize::from(index);
            let (source, variant, set) = group_of(index);
            let start = self.end();
            match source {
                Source::Static(format) => {
                    let commands = &list.commands[format_index(format)][variant.index()];
                    let sorted = (set == DrawSet::Surfaces)
                        .then(|| list.orders.sorted(kind, format, variant))
                        .flatten();
                    self.compact_group(commands, sorted, set, tests);
                }
                Source::Deformed => self.compact_group(&list.deformed[variant.index()], None, set, tests),
            }
            self.ranges[index] = [start, self.end()];
        }
    }

    /// Lists the draws of `set` of `draws` that pass `tests`, in `sorted`'s order if the group
    /// sorts and in theirs if not.
    fn compact_group<D: Listed, I: Fn(u32) -> bool>(
        &mut self,
        draws: &[D],
        sorted: Option<&[u32]>,
        set: DrawSet,
        tests: &ViewTests<'_, I>,
    ) {
        let Self { draws: listed, left_out, .. } = self;
        let mut take = |index: u32| {
            let draw = &draws[index as usize];
            if !tests.stands_here(draw.slot()) {
                return;
            }
            match tests.pick(set, draw) {
                Pick::Out => return,
                Pick::Cutoff => {
                    left_out.considered += 1;
                    left_out.culled_by_cutoff += 1;
                    return;
                }
                Pick::In => {}
            }
            if !(tests.instance)(draw.slot()) {
                left_out.considered += 1;
                left_out.culled_by_pass += 1;
            } else if D::STATIC && !tests.in_view(draw.slot()) {
                left_out.considered += 1;
                left_out.culled_by_frustum += 1;
            } else {
                listed.push(index);
            }
        };
        // A list built by hand (a unit test's) has no orders: its own order, then.
        match sorted.filter(|sorted| sorted.len() == draws.len()) {
            Some(sorted) => sorted.iter().for_each(|&index| take(index)),
            None => (0..to_u32(draws.len())).for_each(take),
        }
    }
}

/// Which list a pass walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Walk {
    Main,
    ScenePrepass,
    CharacterPrepass,
    /// A shadow cascade, below [`MAX_CASCADES`].
    Cascade(usize),
    /// The character maps' pass as a whole: what it left out, which it counts once.
    CharacterMaps,
    /// One character map, below [`MAX_CHARACTER_MAPS`].
    CharacterMap(usize),
    /// An inset's main pass, below [`MAX_INSETS`].
    InsetMain(usize),
    /// An inset's character prepass.
    InsetCharacters(usize),
}

/// What a frame's shadow casters are tested by, in one walk for every shadow view.
pub(crate) struct CasterTests<'a, P> {
    pub stands: &'a [u16],
    /// The cascades drawn this frame.
    pub cascades: [bool; MAX_CASCADES],
    /// The character maps drawn this frame.
    pub maps: usize,
    /// Per instance slot: the cascades it casts into, as bits, and the character map it draws
    /// into, if any (`shadows::plan::SlotCaster`).
    pub plan: P,
}

/// Every view's list, kept across frames (boxed by the renderer).
#[derive(Debug)]
pub(crate) struct ViewLists {
    /// Per [`ViewKind`], the groups its passes walk.
    groups: [Vec<u16>; ViewKind::ALL.len()],
    main: ViewList,
    scene: ViewList,
    characters: ViewList,
    cascades: [ViewList; MAX_CASCADES],
    maps: [ViewList; MAX_CHARACTER_MAPS],
    /// What the character maps' pass looks at and doesn't draw: each map looks at every caster.
    maps_left_out: PassCounts,
    /// Per inset: its main pass's list and its character prepass's.
    insets: [[ViewList; 2]; MAX_INSETS],
}

impl Default for ViewLists {
    fn default() -> Self {
        Self {
            groups: ViewKind::ALL.map(ViewKind::groups),
            main: ViewList::default(),
            scene: ViewList::default(),
            characters: ViewList::default(),
            cascades: Default::default(),
            maps: Default::default(),
            maps_left_out: PassCounts::default(),
            insets: Default::default(),
        }
    }
}

impl ViewLists {
    /// The list `walk` draws from; none for the character maps' pass as a whole.
    pub(crate) fn list(&self, walk: Walk) -> Option<&ViewList> {
        match walk {
            Walk::Main => Some(&self.main),
            Walk::ScenePrepass => Some(&self.scene),
            Walk::CharacterPrepass => Some(&self.characters),
            Walk::Cascade(index) => self.cascades.get(index),
            Walk::CharacterMaps => None,
            Walk::CharacterMap(map) => self.maps.get(map),
            Walk::InsetMain(index) => self.insets.get(index).map(|[main, _]| main),
            Walk::InsetCharacters(index) => self.insets.get(index).map(|[_, characters]| characters),
        }
    }

    /// What the pass walking `walk` looked at and didn't draw, which it counts once: a single
    /// character map's is its pass's ([`Walk::CharacterMaps`]).
    pub(crate) fn left_out(&self, walk: Walk) -> PassCounts {
        match walk {
            Walk::CharacterMaps => self.maps_left_out,
            Walk::CharacterMap(_) => PassCounts::default(),
            walk => self.list(walk).map(|list| *list.left_out()).unwrap_or_default(),
        }
    }

    /// Compacts `walk`'s list from `list` (a view of the main view's or the character prepass's
    /// kind, the world's or an inset's), by `tests`.
    pub(crate) fn compact_view<I: Fn(u32) -> bool>(
        &mut self,
        walk: Walk,
        list: &DrawList,
        tests: &ViewTests<'_, I>,
    ) {
        let (kind, target) = match walk {
            Walk::Main => (ViewKind::Main, &mut self.main),
            Walk::ScenePrepass => (ViewKind::ScenePrepass, &mut self.scene),
            Walk::CharacterPrepass => (ViewKind::CharacterPrepass, &mut self.characters),
            Walk::InsetMain(index) => (ViewKind::Main, &mut self.insets[index][0]),
            Walk::InsetCharacters(index) => (ViewKind::CharacterPrepass, &mut self.insets[index][1]),
            Walk::Cascade(_) | Walk::CharacterMaps | Walk::CharacterMap(_) => {
                unreachable!("the shadow views compact together, in `compact_casters`")
            }
        };
        target.compact(kind, &self.groups[kind_index(kind)], list, tests);
    }

    /// Compacts every shadow view's list from `list` in one walk of the casters: each cascade
    /// drawn lists the casters its test keeps, each character map drawn its character's.
    pub(crate) fn compact_casters<P: Fn(u32) -> (u8, Option<u8>)>(
        &mut self,
        list: &DrawList,
        tests: &CasterTests<'_, P>,
    ) {
        let Self { groups, cascades, maps, maps_left_out, .. } = self;
        let drawn_maps = tests.maps.min(MAX_CHARACTER_MAPS);
        let maps = &mut maps[..drawn_maps];
        for (cascade, &drawn) in cascades.iter_mut().zip(&tests.cascades) {
            if drawn {
                cascade.clear();
            }
        }
        maps.iter_mut().for_each(ViewList::clear);
        *maps_left_out = PassCounts::default();
        if !tests.cascades.contains(&true) && maps.is_empty() {
            return;
        }
        let others = to_u32(drawn_maps.saturating_sub(1));
        for &index in &groups[kind_index(ViewKind::Casters)] {
            let index = usize::from(index);
            let (source, variant, _) = group_of(index);
            let starts = cascades.each_ref().map(ViewList::end);
            let map_starts: [u32; MAX_CHARACTER_MAPS] =
                std::array::from_fn(|map| maps.get(map).map_or(0, ViewList::end));
            let mut take = |slot: u32, index: u32| {
                if tests.stands.get(slot as usize).copied().unwrap_or(0) != 0 {
                    return;
                }
                let (bits, map) = (tests.plan)(slot);
                for (cascade, list) in cascades.iter_mut().enumerate() {
                    if !tests.cascades[cascade] {
                        continue;
                    }
                    if bits & (1 << cascade) != 0 {
                        list.draws.push(index);
                    } else {
                        list.left_out.considered += 1;
                        list.left_out.culled_by_pass += 1;
                    }
                }
                if drawn_maps == 0 {
                    return;
                }
                // Each map looks at every caster and draws its own character's.
                let skipped = match map.map(usize::from).and_then(|map| maps.get_mut(map)) {
                    Some(list) => {
                        list.draws.push(index);
                        others
                    }
                    None => to_u32(drawn_maps),
                };
                maps_left_out.considered += skipped;
                maps_left_out.culled_by_pass += skipped;
            };
            match source {
                Source::Static(format) => {
                    let commands = &list.commands[format_index(format)][variant.index()];
                    let sorted = list.orders.sorted(ViewKind::Casters, format, variant);
                    match sorted.filter(|sorted| sorted.len() == commands.len()) {
                        Some(sorted) => sorted.iter().for_each(|&at| take(commands[at as usize].slot, at)),
                        None => commands.iter().zip(0..).for_each(|(command, at)| take(command.slot, at)),
                    }
                }
                Source::Deformed => {
                    let draws = &list.deformed[variant.index()];
                    draws.iter().zip(0..).for_each(|(draw, at)| take(draw.slot, at));
                }
            }
            for ((cascade, start), &drawn) in cascades.iter_mut().zip(starts).zip(&tests.cascades) {
                if drawn {
                    cascade.ranges[index] = [start, cascade.end()];
                }
            }
            for (map, start) in maps.iter_mut().zip(map_starts) {
                map.ranges[index] = [start, map.end()];
            }
        }
    }
}

fn kind_index(kind: ViewKind) -> usize {
    match kind {
        ViewKind::Main => 0,
        ViewKind::ScenePrepass => 1,
        ViewKind::CharacterPrepass => 2,
        ViewKind::Casters => 3,
    }
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("draw and handle counts fit u32")
}

#[cfg(test)]
mod tests;
