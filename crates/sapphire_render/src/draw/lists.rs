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
//!   keeps the draws that stand in the view and pass its tests (its instance's culling verdict, M5
//!   D3's `view_cull`; an outline hull's cutoff, a cascade's casters, a character map's character,
//!   the listed characters) and counts the rest by the test that left them out, as the passes
//!   counted them before. The lists
//!   keep their capacity, so a warm frame allocates nothing; the shadow views are compacted in one
//!   walk, beside the world's other views ([`ViewLists::compact_world`]).
//! - **An entry is a position in its group** of the draw list (`DrawList::commands` or
//!   `DrawList::deformed`), whose command carries what later work reads: its instance's slot (whose
//!   bounds `InstanceSlots::world_bounds` gives), its mesh, its material and its record, **and the
//!   level of detail the view draws it at** ([`Entry`], M5 D4, the `levels` module): the main
//!   view's family's from its culling ([`ViewTests::levels`]), each cascade's from its plan
//!   (`SlotCaster::levels`), 0 for a character map, an inset and anything deformed. A submesh its
//!   level simplified away is left out unlisted and uncounted, as a draw outside the group's set
//!   is. The shadow views draw by (mesh, level, record): their order built at level 0, and a
//!   group whose instances of one mesh draw at more than one level sorted again as it's compacted.
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
use crate::shadows::plan::SlotCaster;
use crate::shadows::{MAX_CASCADES, MAX_CHARACTER_MAPS};
use crate::view_cull::{Culled, Verdict};

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
/// | `level` | the level of detail the view draws it at (M5 D4) | a run of one mesh draws one level's index ranges; built at 0, and sorted again where a frame's levels differ ([`Entry`]) |
/// | `draw` | the draw's record (`DrawCommand::draw_id`) | today's order: unique in its group, so the order is total and the same whatever sorts it |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DrawKey {
    pub mesh: u32,
    pub level: u8,
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
            Self::Casters => Some(caster_key(command, 0)),
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

/// The shadow views' key of `command` drawn at `level`.
fn caster_key(command: &DrawCommand, level: u8) -> DrawKey {
    DrawKey { mesh: to_u32(command.mesh.index()), level, draw: command.draw_id }
}

/// A draw the list compacts: a static mesh's command or a deformed instance's.
trait Listed {
    fn slot(&self) -> u32;
    fn draw_id(&self) -> u32;
    fn material(&self) -> MaterialHandle;
    fn outline(&self) -> bool;
    /// The level it draws at where its instance's is `level`, or `None` if its submesh has no
    /// triangles there (M5 D4).
    fn at_level(&self, level: u8) -> Option<u8>;
}

impl Listed for DrawCommand {
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
    fn at_level(&self, level: u8) -> Option<u8> {
        has_level(self.levels, level).then_some(level)
    }
}

impl Listed for DeformedDraw {
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
    /// Skinned and morphed instances draw level 0 alone (m5-plan D5's to change).
    fn at_level(&self, _: u8) -> Option<u8> {
        Some(0)
    }
}

/// Whether a command whose submesh has triangles at the levels `levels` holds (bit `l`,
/// `DrawCommand::levels`) has them at `level`.
pub(crate) fn has_level(levels: u8, level: u8) -> bool {
    1u8.checked_shl(u32::from(level)).is_some_and(|bit| levels & bit != 0)
}

/// One draw a view lists (M5 D4): its position in its group of the draw list, and the level of
/// detail the view draws it at, whose index ranges it draws (`MeshRecord::lods`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Entry {
    pub at: u32,
    pub level: u8,
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
    /// Per instance slot, whether the view it stands in draws it, or by what test not
    /// (`view_cull`, M5 D3); `None` draws every one.
    pub view: Option<Culled<'a>>,
    /// Per instance slot, the level of detail the main view's family draws it at (M5 D4,
    /// [`Culled::level`]), whatever the culling mode: the main view's and the scene prepass's
    /// lists; `None` draws every one at level 0.
    pub levels: Option<Culled<'a>>,
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
        let Self { stands, stand, hulls, materials, view, levels, .. } = *self;
        ViewTests { stands, stand, hulls, materials, view, levels, instance }
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

    fn verdict(&self, slot: u32) -> Verdict {
        self.view.map_or(Verdict::Drawn, |view| view.verdict(slot))
    }

    fn level(&self, slot: u32) -> u8 {
        self.levels.map_or(0, |levels| levels.level(slot))
    }
}

/// One view's draws this frame, group by group, each in its key's order.
#[derive(Debug, PartialEq)]
pub(crate) struct ViewList {
    /// Positions in their groups of the draw list and their levels, group after group.
    draws: Vec<Entry>,
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

    /// The listed draws of `set` of `variant`'s from `source`: positions in the draw list's group,
    /// and their levels.
    pub(crate) fn group(&self, source: Source, variant: SurfaceVariant, set: DrawSet) -> &[Entry] {
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
                return;
            }
            match tests.verdict(draw.slot()) {
                Verdict::Drawn => {
                    // A submesh its level simplified away: not drawn, and not looked at (M5 D4).
                    if let Some(level) = draw.at_level(tests.level(draw.slot())) {
                        listed.push(Entry { at: index, level });
                    }
                }
                verdict => {
                    left_out.considered += 1;
                    left_out.count_culled(verdict);
                }
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

/// What a frame's shadow casters are tested by, in one walk for every shadow view: each cascade
/// lists a caster at its level there (`SlotCaster::levels`), a character map at level 0.
pub(crate) struct CasterTests<'a, P> {
    pub stands: &'a [u16],
    /// The cascades drawn this frame.
    pub cascades: [bool; MAX_CASCADES],
    /// The character maps drawn this frame.
    pub maps: usize,
    /// Per instance slot: the cascades it casts into, the character map it draws into, and what
    /// left it out of the others ([`SlotCaster`]).
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

    /// Compacts the world's views' lists from `list` (M5 D3): the main view's and the scene
    /// prepass's by `world`, the character prepass's by `characters`, and every shadow view's by
    /// `casters`, each its own list, in parallel on the job system when the draw list is long
    /// enough to pay for it ([`PARALLEL_DRAWS`]). The lists are the same whichever runs first.
    pub(crate) fn compact_world<I, J, P>(
        &mut self,
        list: &DrawList,
        world: &ViewTests<'_, I>,
        characters: &ViewTests<'_, J>,
        casters: &CasterTests<'_, P>,
    ) where
        I: Fn(u32) -> bool + Sync,
        J: Fn(u32) -> bool + Sync,
        P: Fn(u32) -> SlotCaster + Sync,
    {
        self.compact_world_as(list, world, characters, casters, list.draws.len() >= PARALLEL_DRAWS);
    }

    /// [`Self::compact_world`], in parallel or not as `parallel` says: the lists are the same
    /// either way.
    pub(crate) fn compact_world_as<I, J, P>(
        &mut self,
        list: &DrawList,
        world: &ViewTests<'_, I>,
        characters: &ViewTests<'_, J>,
        casters: &CasterTests<'_, P>,
        parallel: bool,
    ) where
        I: Fn(u32) -> bool + Sync,
        J: Fn(u32) -> bool + Sync,
        P: Fn(u32) -> SlotCaster + Sync,
    {
        let Self { groups, main, scene, characters: listed, cascades, maps, maps_left_out, .. } = self;
        let groups = &*groups;
        let main_groups = &groups[kind_index(ViewKind::Main)];
        let scene_groups = &groups[kind_index(ViewKind::ScenePrepass)];
        let character_groups = &groups[kind_index(ViewKind::CharacterPrepass)];
        let caster_groups = &groups[kind_index(ViewKind::Casters)];
        if !parallel {
            main.compact(ViewKind::Main, main_groups, list, world);
            scene.compact(ViewKind::ScenePrepass, scene_groups, list, world);
            listed.compact(ViewKind::CharacterPrepass, character_groups, list, characters);
            compact_casters(caster_groups, cascades, maps, maps_left_out, list, casters);
            return;
        }
        sapphire_jobs::scope(|scope| {
            scope.spawn(|| main.compact(ViewKind::Main, main_groups, list, world));
            scope.spawn(|| scene.compact(ViewKind::ScenePrepass, scene_groups, list, world));
            scope.spawn(|| listed.compact(ViewKind::CharacterPrepass, character_groups, list, characters));
            scope.spawn(|| compact_casters(caster_groups, cascades, maps, maps_left_out, list, casters));
        });
    }

    /// Compacts every shadow view's list from `list` in one walk of the casters: each cascade
    /// drawn lists the casters its test keeps, each character map drawn its character's.
    #[cfg(test)]
    pub(crate) fn compact_casters<P: Fn(u32) -> SlotCaster>(
        &mut self,
        list: &DrawList,
        tests: &CasterTests<'_, P>,
    ) {
        let Self { groups, cascades, maps, maps_left_out, .. } = self;
        compact_casters(&groups[kind_index(ViewKind::Casters)], cascades, maps, maps_left_out, list, tests);
    }
}

/// Draw lists at least this long compact the world's views in parallel ([`ViewLists::compact_world`]);
/// shorter ones on the calling thread, which allocates nothing.
pub(crate) const PARALLEL_DRAWS: usize = 4096;

/// Compacts every shadow view's list (`cascades`, `maps`, what the maps' pass leaves out) from
/// `list`'s caster `groups` in one walk of the casters, by `tests`.
fn compact_casters<P: Fn(u32) -> SlotCaster>(
    groups: &[u16],
    cascades: &mut [ViewList; MAX_CASCADES],
    maps: &mut [ViewList; MAX_CHARACTER_MAPS],
    maps_left_out: &mut PassCounts,
    list: &DrawList,
    tests: &CasterTests<'_, P>,
) {
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
    for &index in groups {
        let index = usize::from(index);
        let (source, variant, _) = group_of(index);
        let starts = cascades.each_ref().map(ViewList::end);
        let map_starts: [u32; MAX_CHARACTER_MAPS] =
            std::array::from_fn(|map| maps.get(map).map_or(0, ViewList::end));
        // `levels`: the levels its submesh has triangles at (`DrawCommand::levels`).
        let mut take = |slot: u32, index: u32, levels: u8| {
            if tests.stands.get(slot as usize).copied().unwrap_or(0) != 0 {
                return;
            }
            let caster = (tests.plan)(slot);
            let map = caster.map;
            for (cascade, list) in cascades.iter_mut().enumerate() {
                if !tests.cascades[cascade] {
                    continue;
                }
                match caster.left_out(cascade) {
                    // A submesh simplified away at the cascade's level: not drawn, not looked at.
                    None => {
                        let level = caster.levels[cascade];
                        if has_level(levels, level) {
                            list.draws.push(Entry { at: index, level });
                        }
                    }
                    // Its volume is the cascade's own pick (B4's meaning); distance and size
                    // count as such.
                    Some(Verdict::Drawn | Verdict::Frustum) => {
                        list.left_out.considered += 1;
                        list.left_out.culled_by_pass += 1;
                    }
                    Some(verdict) => {
                        list.left_out.considered += 1;
                        list.left_out.count_culled(verdict);
                    }
                }
            }
            if drawn_maps == 0 {
                return;
            }
            // Each map looks at every caster and draws its own character's.
            let skipped = match map.map(usize::from).and_then(|map| maps.get_mut(map)) {
                Some(list) => {
                    list.draws.push(Entry { at: index, level: 0 });
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
                    Some(sorted) => {
                        for &at in sorted {
                            let command = &commands[at as usize];
                            take(command.slot, at, command.levels);
                        }
                    }
                    None => commands
                        .iter()
                        .zip(0..)
                        .for_each(|(command, at)| take(command.slot, at, command.levels)),
                }
            }
            Source::Deformed => {
                let draws = &list.deformed[variant.index()];
                draws.iter().zip(0..).for_each(|(draw, at)| take(draw.slot, at, 1));
            }
        }
        for ((cascade, start), &drawn) in cascades.iter_mut().zip(starts).zip(&tests.cascades) {
            if drawn {
                let end = cascade.end();
                cascade.ranges[index] = [start, end];
                if let Source::Static(format) = source {
                    let commands = &list.commands[format_index(format)][variant.index()];
                    sort_by_level(&mut cascade.draws[start as usize..end as usize], commands);
                }
            }
        }
        for (map, start) in maps.iter_mut().zip(map_starts) {
            map.ranges[index] = [start, map.end()];
        }
    }
}

/// Puts a cascade's group of static casters, listed in their key's order at level 0, in the key's
/// order at their levels (M5 D4): only where one mesh's instances draw at more than one level, as
/// they rarely do (a cascade chooses by its texel alone, so only their stretch tells them apart).
/// A list built by hand keeps its order.
fn sort_by_level(entries: &mut [Entry], commands: &[DrawCommand]) {
    let key = |entry: &Entry| commands.get(entry.at as usize).map(|command| caster_key(command, entry.level));
    if entries.is_sorted_by_key(key) {
        return;
    }
    // Keys are unique (their records are), so unstable is stable.
    entries.sort_unstable_by_key(key);
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
