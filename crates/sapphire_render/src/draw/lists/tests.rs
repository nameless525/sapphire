use glam::{Mat4, Vec3};
use sapphire_assets::material::{MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::PositionFormat;

use super::*;
use crate::bounds::Sphere;
use crate::gpu_types::Dequantize;
use crate::mesh_pool::{DeformSource, MeshRecord, MeshStreams, Placement, SubmeshDraw, SubmeshMaterial};
use crate::pipelines::{Coverage, Part};
use crate::scene::{DrawLimits, InstanceSlots};
use crate::view_cull::SlotCull;
use crate::{InstanceDeltas, InstanceId, MeshHandle, MeshInstance};

const UNORM16: PositionFormat = PositionFormat::Unorm16;

/// Material handle `handle`, in material slot 0.
fn submesh(first_index: u32, handle: u32) -> SubmeshDraw {
    SubmeshDraw {
        first_index,
        index_count: 3,
        material: SubmeshMaterial { handle: MaterialHandle::from_raw(handle), slot: 0 },
    }
}

fn mesh(streams: MeshStreams, submeshes: Vec<SubmeshDraw>) -> MeshRecord {
    MeshRecord {
        streams,
        dequantize: Dequantize { position_scale: [1.0; 3], ..Dequantize::default() },
        lods: vec![submeshes],
        texel_density: Default::default(),
        lod_vertices: vec![4],
        lod_errors: vec![0.0],
        placement: Placement::default(),
        slots: Vec::new(),
        bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
        palette_reach: Vec::new(),
        morph_reach: Vec::new(),
        foliage: None,
    }
}

fn slab(first_vertex: u32) -> MeshStreams {
    MeshStreams::Slab { format: UNORM16, first_vertex }
}

/// Handles 10 to 19 are opaque environment materials, 20 an anime character body with a hull, 30
/// a transparent environment material and 40 an eye's layer.
fn kind(handle: MaterialHandle) -> (MaterialFamily, MaterialFlags) {
    match handle.index() {
        20 => (MaterialFamily::AnimeCharacter, MaterialFlags::OUTLINE),
        30 => (MaterialFamily::Environment, MaterialFlags::TRANSPARENT),
        40 => (MaterialFamily::AnimeCharacter, MaterialFlags::EYE | MaterialFlags::TRANSPARENT),
        _ => (MaterialFamily::Environment, MaterialFlags::empty()),
    }
}

/// Instances of `meshes`, in slots 0 onward, one a metre apart.
fn slots(meshes: &[u32]) -> InstanceSlots {
    let changed = meshes
        .iter()
        .enumerate()
        .map(|(id, &mesh)| MeshInstance {
            id: InstanceId(id as u64),
            mesh: MeshHandle::from_raw(mesh),
            transform: Mat4::from_translation(Vec3::new(id as f32, 0.0, 0.0)),
        })
        .collect();
    let mut slots = InstanceSlots::default();
    slots.apply(&InstanceDeltas { changed, removed: Vec::new() });
    slots
}

fn built(meshes: &[MeshRecord], slots: &InstanceSlots) -> DrawList {
    let mut list = DrawList::default();
    list.build(meshes, kind, |handle| handle.index() as u32, slots, DrawLimits::NONE);
    list
}

fn environment() -> SurfaceVariant {
    SurfaceVariant::environment(Coverage::Opaque)
}

/// What a group of a view's list draws: (record, material handle, mesh handle) in its order.
fn drawn(
    list: &DrawList,
    view: &ViewList,
    variant: SurfaceVariant,
    set: DrawSet,
) -> Vec<(u32, usize, usize)> {
    let commands = &list.commands[format_index(UNORM16)][variant.index()];
    view.group(Source::Static(UNORM16), variant, set)
        .iter()
        .map(|entry| {
            let command = &commands[entry.at as usize];
            (command.draw_id, command.material.index(), command.mesh.index())
        })
        .collect()
}

/// What a group of a view's list draws: (record, level) in its order.
fn levels(view: &ViewList, list: &DrawList, set: DrawSet) -> Vec<(u32, u8)> {
    let commands = &list.commands[format_index(UNORM16)][environment().index()];
    let entries = view.group(Source::Static(UNORM16), environment(), set);
    entries.iter().map(|entry| (commands[entry.at as usize].draw_id, entry.level)).collect()
}

fn world<'a>(
    stands: &'a [u16],
    hulls: &'a [bool],
    view: Option<Culled<'a>>,
) -> ViewTests<'a, fn(u32) -> bool> {
    ViewTests { stands, stand: 0, hulls, materials: &[], view, levels: None, instance: |_| true }
}

/// A caster drawn into the cascades of `cascades` and the character map `map`, held by those
/// cascades alone, at level 0.
fn caster(cascades: u8, map: Option<u8>) -> SlotCaster {
    SlotCaster { cascades, map, held: cascades, ..SlotCaster::default() }
}

/// Three meshes of environment draws, instanced in an order that alternates them.
fn mixed() -> (Vec<MeshRecord>, InstanceSlots) {
    let meshes = vec![
        // Two submeshes: materials 12 then 11.
        mesh(slab(0), vec![submesh(0, 12), submesh(3, 11)]),
        mesh(slab(4), vec![submesh(6, 11)]),
        mesh(slab(8), vec![submesh(9, 12)]),
    ];
    (meshes, slots(&[2, 0, 1, 2, 0, 1]))
}

/// Each kind of view holds its order: the views that write anything but depth (the main view, an
/// inset, both prepasses) draw a group in the record's order, where an exact tie in depth keeps
/// the first-drawn surface; the shadow views (each cascade, each character map) by mesh, then the
/// submesh's place, then record, so each mesh's submesh is one run (m5-plan D2).
#[test]
fn each_kind_of_view_holds_its_order() {
    let (meshes, slots) = mixed();
    let list = built(&meshes, &slots);
    // Records in slot order: slot 0 (mesh 2) is record 0, slot 1 (mesh 0) records 1 and 2, ...
    let records =
        [(0, 12, 2), (1, 12, 0), (2, 11, 0), (3, 11, 1), (4, 12, 2), (5, 12, 0), (6, 11, 0), (7, 11, 1)];
    let listed: Vec<(u32, usize, usize)> = list.commands[0][environment().index()]
        .iter()
        .map(|command| (command.draw_id, command.material.index(), command.mesh.index()))
        .collect();
    assert_eq!(listed, records);

    let mut lists = ViewLists::default();
    let stands = [];
    let tests = world(&stands, &[], None);
    for walk in [Walk::Main, Walk::ScenePrepass, Walk::InsetMain(0)] {
        lists.compact_view(walk, &list, &tests);
        let view = lists.list(walk).expect("listed");
        assert_eq!(
            drawn(&list, view, environment(), DrawSet::Surfaces),
            records,
            "{walk:?}: the record's order"
        );
    }
    // The character prepass draws the characters' surfaces alone, in the record's order too.
    let body = SurfaceVariant::character(Coverage::Opaque, Part::Body);
    let characters = vec![mesh(slab(0), vec![submesh(0, 20)]), mesh(slab(4), vec![submesh(3, 20)])];
    let party = built(&characters, &self::slots(&[1, 0, 1]));
    for walk in [Walk::CharacterPrepass, Walk::InsetCharacters(0)] {
        lists.compact_view(walk, &party, &tests);
        let meshes: Vec<usize> = drawn(&party, lists.list(walk).expect("listed"), body, DrawSet::Surfaces)
            .iter()
            .map(|d| d.2)
            .collect();
        assert_eq!(meshes, [1, 0, 1], "{walk:?}: the record's order");
    }

    let plan = |slot: u32| caster(1, Some((slot % 2) as u8));
    let casters = CasterTests {
        stands: &stands,
        cascades: [true, false, false, false],
        maps: 2,
        spots: [false; 4],
        plan,
    };
    lists.compact_casters(&list, &casters);
    let cascade = lists.list(Walk::Cascade(0)).expect("listed");
    assert_eq!(
        drawn(&list, cascade, environment(), DrawSet::Surfaces),
        [(1, 12, 0), (5, 12, 0), (2, 11, 0), (6, 11, 0), (3, 11, 1), (7, 11, 1), (0, 12, 2), (4, 12, 2)],
        "depth alone: mesh, then the submesh's place, then record"
    );
    // Map 0 draws slots 0, 2 and 4: records 0 (mesh 2), 3 (mesh 1), 5 and 6 (mesh 0).
    let map = lists.list(Walk::CharacterMap(0)).expect("listed");
    let records: Vec<u32> = drawn(&list, map, environment(), DrawSet::Surfaces).iter().map(|d| d.0).collect();
    assert_eq!(records, [5, 6, 3, 0], "a character map: mesh, then record");
}

/// Keys are unique and total: a rebuild gives the same order, and draws of one mesh keep their
/// records' order. The views that keep the record list no order of their own.
#[test]
fn the_order_is_stable() {
    let (meshes, slots) = mixed();
    let list = built(&meshes, &slots);
    let again = built(&meshes, &slots);
    for kind in ViewKind::ALL {
        assert_eq!(
            list.orders.sorted(kind, UNORM16, environment()),
            again.orders.sorted(kind, UNORM16, environment())
        );
        assert_eq!(list.orders.sorted(kind, UNORM16, environment()).is_some(), kind == ViewKind::Casters);
    }
    let mut keys: Vec<DrawKey> = list.commands[0][environment().index()]
        .iter()
        .filter_map(|command| ViewKind::Casters.key(command))
        .collect();
    keys.sort();
    keys.dedup_by_key(|key| key.mesh);
    assert_eq!(keys.len(), 3, "three runs: meshes 0, 1 and 2");
}

/// Groups whose order can show keep the draw list's: transparent surfaces, eyes' layers (in their
/// layers' order), outline hulls; and skinned instances bind by instance.
#[test]
fn order_dependent_groups_keep_the_list_order() {
    let meshes = vec![
        // A transparent card over an opaque one, and an eye's layers 2 then 1 (`layer` is the
        // handle), each listed after the other mesh's.
        mesh(slab(0), vec![submesh(0, 30), submesh(3, 10)]),
        mesh(slab(4), vec![submesh(6, 30), submesh(9, 20)]),
        mesh(MeshStreams::Deformable(DeformSource::default()), vec![submesh(12, 20), submesh(15, 10)]),
    ];
    let slots = slots(&[1, 0, 2, 1, 0, 2]);
    let list = built(&meshes, &slots);
    let mut lists = ViewLists::default();
    let hulls = [true; 16];
    lists.compact_view(Walk::Main, &list, &world(&[], &hulls, None));
    let main = lists.list(Walk::Main).expect("listed");
    let transparent = SurfaceVariant::environment(Coverage::Transparent);
    let records = |variant: SurfaceVariant| -> Vec<u32> {
        list.commands[0][variant.index()].iter().map(|command| command.draw_id).collect()
    };
    let in_list = |variant: SurfaceVariant, set| -> Vec<u32> {
        drawn(&list, main, variant, set).into_iter().map(|(record, ..)| record).collect()
    };
    assert_eq!(in_list(transparent, DrawSet::Surfaces), records(transparent), "blended in today's order");
    let body = SurfaceVariant::character(Coverage::Opaque, Part::Body);
    assert_eq!(in_list(body, DrawSet::Outlines), records(body), "hulls in today's order");
    let deformed: Vec<u32> = main
        .group(Source::Deformed, body, DrawSet::Surfaces)
        .iter()
        .map(|entry| list.deformed[body.index()][entry.at as usize].draw_id)
        .collect();
    let skinned: Vec<u32> = list.deformed[body.index()].iter().map(|draw| draw.draw_id).collect();
    assert_eq!(deformed, skinned, "skinned instances in slot order");
    assert!(!skinned.is_empty() && !records(transparent).is_empty());
}

/// Eyes' layers blend in their layers' order, which the draw list sorted them into: the list keeps it.
#[test]
fn eye_layers_keep_their_layers_order() {
    // Handle 41 is a later layer than 40, listed first.
    let kind = |handle: MaterialHandle| match handle.index() {
        40 | 41 => (MaterialFamily::AnimeCharacter, MaterialFlags::EYE | MaterialFlags::TRANSPARENT),
        _ => (MaterialFamily::Environment, MaterialFlags::empty()),
    };
    let meshes = vec![mesh(slab(0), vec![submesh(0, 41), submesh(3, 40)])];
    let slots = slots(&[0, 0]);
    let mut list = DrawList::default();
    list.build(&meshes, kind, |handle| handle.index() as u32, &slots, DrawLimits::NONE);
    let mut lists = ViewLists::default();
    lists.compact_view(Walk::Main, &list, &world(&[], &[], None));
    let main = lists.list(Walk::Main).expect("listed");
    let layers: Vec<usize> = drawn(&list, main, SurfaceVariant::EYE_LAYERS, DrawSet::Surfaces)
        .into_iter()
        .map(|(_, material, _)| material)
        .collect();
    assert_eq!(layers, [40, 40, 41, 41], "layer 40 under 41, whatever the mesh lists first");
}

/// A view's list leaves out what its passes left out, counting it by the test, as they counted.
#[test]
fn a_view_counts_what_it_leaves_out() {
    let meshes = vec![mesh(slab(0), vec![submesh(0, 10)]), mesh(slab(4), vec![submesh(3, 20)])];
    // Slots 0 to 3: environment, character, environment, character; slot 2 culled by the frustum,
    // slot 3 stands in an inset.
    let slots = slots(&[0, 1, 0, 1]);
    let list = built(&meshes, &slots);
    let stands = [0, 0, 0, 1];
    let view = SlotCull::of(&[Verdict::Drawn, Verdict::Drawn, Verdict::Frustum, Verdict::Drawn]);
    // The character's hull (record 1) is past its cutoff.
    let hulls = [false; 4];
    let mut lists = ViewLists::default();
    lists.compact_view(Walk::Main, &list, &world(&stands, &hulls, Some(Culled::new(&view))));
    let main = lists.list(Walk::Main).expect("listed");
    assert_eq!(main.len(), 2, "slot 0's surface and slot 1's");
    let left_out = lists.left_out(Walk::Main);
    assert_eq!((left_out.considered, left_out.culled_by_frustum, left_out.culled_by_cutoff), (2, 1, 1));
    // The character prepass takes the listed characters alone: none.
    let tests = ViewTests { view: None, ..world(&stands, &hulls, None).with_instance(|_| false) };
    lists.compact_view(Walk::CharacterPrepass, &list, &tests);
    assert_eq!(lists.list(Walk::CharacterPrepass).expect("listed").len(), 0);
    assert_eq!(
        lists.left_out(Walk::CharacterPrepass).culled_by_pass,
        1,
        "slot 1's body; slot 3's is the inset's"
    );
    // The inset's view: slot 3's surface and its hull, cut off.
    let inset = ViewTests { stand: 1, ..world(&stands, &hulls, None) };
    lists.compact_view(Walk::InsetMain(0), &list, &inset);
    assert_eq!(lists.list(Walk::InsetMain(0)).expect("listed").len(), 1);
    assert_eq!(lists.left_out(Walk::InsetMain(0)).culled_by_cutoff, 1);

    // Shadows: cascade 0 takes slots 0 and 1, cascade 1 slot 2; slot 1 draws into map 0 of two.
    let plan = |slot: u32| match slot {
        0 => caster(0b01, None),
        1 => caster(0b01, Some(0)),
        2 => caster(0b10, None),
        _ => caster(0b11, Some(1)),
    };
    let tests = CasterTests {
        stands: &stands,
        cascades: [true, true, false, false],
        maps: 2,
        spots: [false; 4],
        plan,
    };
    lists.compact_casters(&list, &tests);
    let len = |walk| lists.list(walk).expect("listed").len();
    assert_eq!((len(Walk::Cascade(0)), len(Walk::Cascade(1))), (2, 1), "slot 3 stands in the inset");
    assert_eq!((len(Walk::CharacterMap(0)), len(Walk::CharacterMap(1))), (1, 0));
    assert_eq!(lists.left_out(Walk::Cascade(0)).culled_by_pass, 1);
    assert_eq!(lists.left_out(Walk::Cascade(1)).culled_by_pass, 2);
    // Each map looks at all three casters and draws its own: six looked at, one drawn.
    let maps = lists.left_out(Walk::CharacterMaps);
    assert_eq!((maps.considered, maps.culled_by_pass), (5, 5));
    assert_eq!(lists.left_out(Walk::CharacterMap(0)), PassCounts::default(), "counted by the maps' pass");
}

/// The mesh pool moves meshes when it compacts (M5 D9): the rebuilt list draws the same draws in
/// the same order, from their new places, since the key holds handles and no pool range.
#[test]
fn a_list_rebuilt_after_the_pool_compacts_draws_the_same() {
    let (mut meshes, slots) = mixed();
    let list = built(&meshes, &slots);
    // Compaction slides meshes 1 and 2 down over a freed range.
    meshes[1].streams = slab(2);
    meshes[2].streams = slab(5);
    let moved = built(&meshes, &slots);
    let mut lists = ViewLists::default();
    let casters = CasterTests {
        stands: &[],
        cascades: [true, false, false, false],
        maps: 0,
        spots: [false; 4],
        plan: |_| caster(1, None),
    };
    let compact = |lists: &mut ViewLists, list: &DrawList| {
        lists.compact_view(Walk::Main, list, &world(&[], &[], None));
        lists.compact_casters(list, &casters);
    };
    compact(&mut lists, &list);
    let before = [Walk::Main, Walk::Cascade(0)]
        .map(|walk| drawn(&list, lists.list(walk).expect("listed"), environment(), DrawSet::Surfaces));
    compact(&mut lists, &moved);
    for (walk, before) in [Walk::Main, Walk::Cascade(0)].into_iter().zip(before) {
        let view = lists.list(walk).expect("listed");
        assert_eq!(drawn(&moved, view, environment(), DrawSet::Surfaces), before, "{walk:?}");
    }
    let cascade = lists.list(Walk::Cascade(0)).expect("listed");
    let first_vertices: Vec<u32> = cascade
        .group(Source::Static(UNORM16), environment(), DrawSet::Surfaces)
        .iter()
        .map(|entry| moved.commands[0][environment().index()][entry.at as usize].first_vertex)
        .collect();
    assert_eq!(first_vertices, [0, 0, 0, 0, 2, 2, 5, 5], "by mesh, from the meshes' new places");
}

/// Each run's records in the frame's run records, or a run of one's own record.
fn run_records(lists: &ViewLists, runs: &[Run]) -> Vec<Vec<u32>> {
    runs.iter()
        .map(|run| match run.count {
            1 => vec![run.record],
            count => lists.records()[run.record as usize..][..count as usize].to_vec(),
        })
        .collect()
}

/// A shadow view runs each mesh's submesh (M5 D2): its key puts them side by side, and each run's
/// records lie side by side in the run records, in the list's order.
#[test]
fn a_cascade_runs_each_meshs_submesh() {
    let (meshes, slots) = mixed();
    let list = built(&meshes, &slots);
    let mut lists = ViewLists::default();
    lists.begin_frame(DrawRuns::Shadows);
    let casters = CasterTests {
        stands: &[],
        cascades: [true, false, false, false],
        maps: 0,
        spots: [false; 4],
        plan: |_| caster(1, None),
    };
    lists.compact_casters(&list, &casters);
    let cascade = lists.list(Walk::Cascade(0)).expect("listed");
    let runs = cascade.runs(UNORM16, environment());
    let places: Vec<(u32, u32)> = runs.iter().map(|run| (run.first, run.count)).collect();
    assert_eq!(places, [(0, 2), (2, 2), (4, 2), (6, 2)], "mesh 0's two submeshes, mesh 1, mesh 2");
    assert_eq!(run_records(&lists, runs), [[1, 5], [2, 6], [3, 7], [0, 4]]);
    assert_eq!(lists.records().len(), 8);
}

/// The views that write colour keep the record's order (M5 D2's first change), so runs form there only
/// where equal draws are neighbours already; the scene prepass takes the main view's runs and lists no
/// records of its own; runs off, or dropped, form none; with the prepasses' runs the main view forms
/// none and the scene prepass forms its own; by default neither forms any.
#[test]
fn the_main_view_runs_equal_neighbours_alone() {
    let (meshes, _) = mixed();
    // Meshes 1 and 2 have a submesh each, in materials 11 and 12: records 0 to 6 in slot order.
    let slots = slots(&[1, 1, 2, 1, 2, 2, 2]);
    let list = built(&meshes, &slots);
    let mut lists = ViewLists::default();
    let tests = world(&[], &[], None);
    lists.begin_frame(DrawRuns::EveryView);
    lists.compact_view(Walk::Main, &list, &tests);
    let main = lists.list(Walk::Main).expect("listed").runs(UNORM16, environment()).to_vec();
    let counts: Vec<u32> = main.iter().map(|run| run.count).collect();
    assert_eq!(counts, [2, 1, 1, 3]);
    assert_eq!(run_records(&lists, &main), [vec![0, 1], vec![2], vec![3], vec![4, 5, 6]]);
    assert_eq!(lists.records(), [0, 1, 4, 5, 6], "runs of one bind their own record");
    lists.compact_view(Walk::ScenePrepass, &list, &tests);
    let scene = lists.list(Walk::ScenePrepass).expect("listed").runs(UNORM16, environment());
    assert_eq!(scene, main, "the main view's runs");
    assert_eq!(lists.records().len(), 5, "and its records");
    let stats = lists.run_stats();
    assert_eq!((stats.runs, stats.instances, stats.longest, stats.median), (4, 10, 3, 2), "{stats:?}");
    assert_eq!(stats.record_bytes, 5 * 48);

    lists.drop_runs();
    assert!(lists.list(Walk::Main).expect("listed").runs(UNORM16, environment()).is_empty());
    assert!(lists.records().is_empty());
    lists.begin_frame(DrawRuns::Off);
    lists.compact_view(Walk::Main, &list, &tests);
    assert!(lists.list(Walk::Main).expect("listed").runs(UNORM16, environment()).is_empty(), "off");
    assert_eq!(lists.list(Walk::Main).expect("listed").len(), 7, "every draw still listed");

    lists.begin_frame(DrawRuns::ShadowsAndPrepasses);
    lists.compact_view(Walk::Main, &list, &tests);
    lists.compact_view(Walk::InsetMain(0), &list, &tests);
    lists.compact_view(Walk::ScenePrepass, &list, &tests);
    for walk in [Walk::Main, Walk::InsetMain(0)] {
        assert!(lists.list(walk).expect("listed").runs(UNORM16, environment()).is_empty(), "{walk:?}");
    }
    let scene = lists.list(Walk::ScenePrepass).expect("listed").runs(UNORM16, environment());
    assert_eq!(scene, main, "the scene prepass's own runs");
    assert_eq!(lists.records(), [0, 1, 4, 5, 6], "and records");

    lists.begin_frame(DrawRuns::default());
    for walk in [Walk::Main, Walk::ScenePrepass, Walk::CharacterPrepass] {
        lists.compact_view(walk, &list, &tests);
        assert!(lists.list(walk).expect("listed").runs(UNORM16, environment()).is_empty(), "{walk:?}");
    }
    assert!(lists.records().is_empty(), "by default, the shadow views alone");
}

/// A run is of one level of detail (M5 D4): two draws of one mesh's submesh at different levels draw
/// different index ranges, so they never join, in a view that keeps the record's order or in a shadow
/// view, whose key puts each level's draws side by side.
#[test]
fn a_run_is_of_one_level() {
    let mut meshes = vec![mesh(slab(0), vec![submesh(0, 10)])];
    meshes[0].lods.push(vec![submesh(3, 10)]);
    meshes[0].lod_vertices.push(4);
    // Four instances of the mesh: records 0 to 3, in slot order.
    let list = built(&meshes, &slots(&[0, 0, 0, 0]));
    let mut lists = ViewLists::default();
    lists.begin_frame(DrawRuns::EveryView);
    // The main view draws slots 1 and 3 at level 1: no two neighbours of one level.
    let culled = SlotCull::at_levels(&[0, 1, 0, 1]);
    let tests = ViewTests { levels: Some(Culled::new(&culled)), ..world(&[], &[], None) };
    lists.compact_view(Walk::Main, &list, &tests);
    let main = lists.list(Walk::Main).expect("listed");
    assert_eq!(levels(main, &list, DrawSet::Surfaces), [(0, 0), (1, 1), (2, 0), (3, 1)]);
    let counts: Vec<u32> = main.runs(UNORM16, environment()).iter().map(|run| run.count).collect();
    assert_eq!(counts, [1, 1, 1, 1], "equal neighbours but for their level");
    // Cascade 0 draws slots 0 and 1 at level 1 and the others at 0: a run of each level.
    let plan = |slot: u32| SlotCaster { levels: [u8::from(slot < 2), 0, 0, 0], ..caster(1, None) };
    lists.compact_casters(
        &list,
        &CasterTests { stands: &[], cascades: [true, false, false, false], maps: 0, spots: [false; 4], plan },
    );
    let cascade = lists.list(Walk::Cascade(0)).expect("listed");
    assert_eq!(levels(cascade, &list, DrawSet::Surfaces), [(2, 0), (3, 0), (0, 1), (1, 1)], "mesh, level");
    let runs = cascade.runs(UNORM16, environment());
    let places: Vec<(u32, u32)> = runs.iter().map(|run| (run.first, run.count)).collect();
    assert_eq!(places, [(0, 2), (2, 2)], "one run a level");
    assert_eq!(run_records(&lists, runs), [[2, 3], [0, 1]]);
}

/// By default (`DrawRuns::Shadows`), the runs and calls of every view don't depend on the order the
/// instances were placed in, which for a streamed world is the order its cells arrived in: the shadow
/// views key their draws by mesh, and the views that keep the record's order form no runs. The same
/// instances in two slot orders make the same run stats, and each view the same calls.
#[test]
fn default_runs_and_calls_dont_follow_the_slot_order() {
    let (meshes, _) = mixed();
    let calls = |order: &[u32]| {
        let list = built(&meshes, &slots(order));
        let mut lists = ViewLists::default();
        lists.begin_frame(DrawRuns::default());
        let world = world(&[], &[], None);
        let casters = CasterTests {
            stands: &[],
            cascades: [true; 4],
            maps: 0,
            spots: [false; 4],
            plan: |_| caster(0b1111, None),
        };
        lists.compact_world_as(&list, &world, &world, &casters, false);
        let walks = [Walk::Main, Walk::ScenePrepass, Walk::Cascade(0), Walk::Cascade(3)];
        let calls = walks.map(|walk| {
            let view = lists.list(walk).expect("listed");
            match view.runs(UNORM16, environment()) {
                [] => view.group(Source::Static(UNORM16), environment(), DrawSet::Surfaces).len(),
                runs => runs.len(),
            }
        });
        let stats = lists.run_stats();
        (calls, (stats.runs, stats.instances, stats.longest, stats.median), stats.record_bytes)
    };
    let alternating = calls(&[2, 0, 1, 2, 0, 1, 0, 0]);
    let grouped = calls(&[0, 0, 0, 0, 1, 1, 2, 2]);
    assert_eq!(alternating, grouped);
    // Eight instances, twelve draws a view: a cascade's runs are each mesh's submesh, of four, four,
    // two and two instances.
    assert_eq!(alternating.0, [12, 12, 4, 4], "{alternating:?}");
    assert_eq!(alternating.1, (16, 48, 4, 2), "four runs a cascade");
}

/// Groups whose order shows form no runs, whatever their neighbours: transparent surfaces, outline
/// hulls (a set apart from the surfaces) and skinned instances draw a call a draw.
#[test]
fn only_static_depth_writing_surfaces_run() {
    let meshes = vec![
        mesh(slab(0), vec![submesh(0, 30)]),
        mesh(slab(4), vec![submesh(3, 20)]),
        mesh(MeshStreams::Deformable(DeformSource::default()), vec![submesh(6, 20)]),
    ];
    let list = built(&meshes, &slots(&[0, 0, 1, 1, 2, 2]));
    let mut lists = ViewLists::default();
    lists.begin_frame(DrawRuns::EveryView);
    lists.compact_view(Walk::Main, &list, &world(&[], &[true; 6], None));
    let main = lists.list(Walk::Main).expect("listed");
    let transparent = SurfaceVariant::environment(Coverage::Transparent);
    assert_eq!(main.group(Source::Static(UNORM16), transparent, DrawSet::Surfaces).len(), 2);
    assert!(main.runs(UNORM16, transparent).is_empty(), "blended draws a call a draw");
    let body = SurfaceVariant::character(Coverage::Opaque, Part::Body);
    assert_eq!(main.group(Source::Static(UNORM16), body, DrawSet::Outlines).len(), 2, "two hulls");
    let bodies: Vec<u32> = main.runs(UNORM16, body).iter().map(|run| run.count).collect();
    assert_eq!(bodies, [2], "the bodies' surfaces run; their hulls are drawn one by one");
    assert_eq!(main.group(Source::Deformed, body, DrawSet::Surfaces).len(), 2, "skinned, one by one");
    assert_eq!(lists.records(), [2, 3], "the bodies' alone");
}

/// The frame's run stats: of the runs of more than one draw, their count, instances, longest and
/// median (the lower middle).
#[test]
fn run_stats_read_the_runs_of_several() {
    let run = |count| Run { first: 0, count, record: 0 };
    let runs = [run(1), run(2), run(3), run(5), run(1), run(4)];
    let mut lengths = Vec::new();
    let stats = RunStats::of(runs.iter(), 14, &mut lengths);
    assert_eq!(stats, RunStats { runs: 4, instances: 14, longest: 5, median: 3, record_bytes: 14 * 48 });
    assert_eq!(RunStats::of([run(1)].iter(), 0, &mut lengths), RunStats::default());
}

/// A warm frame's compaction allocates nothing: every list keeps its capacity, its runs' and the run
/// records' too.
#[test]
fn compacting_a_warm_frame_allocates_nothing() {
    let (meshes, slots) = mixed();
    let list = built(&meshes, &slots);
    let mut lists = ViewLists::default();
    let compact = |lists: &mut ViewLists| {
        lists.begin_frame(DrawRuns::EveryView);
        let tests = world(&[], &[], None);
        lists.compact_view(Walk::Main, &list, &tests);
        lists.compact_view(Walk::ScenePrepass, &list, &tests);
        lists.compact_view(Walk::CharacterPrepass, &list, &tests);
        lists.compact_view(Walk::InsetMain(1), &list, &tests);
        let casters = CasterTests {
            stands: &[],
            cascades: [true; 4],
            maps: 8,
            spots: [false; 4],
            plan: |slot| caster(0b1011, Some((slot % 8) as u8)),
        };
        lists.compact_casters(&list, &casters);
        lists.run_stats()
    };
    compact(&mut lists);
    let (stats, allocated) = sapphire_core::alloc::count_in_scope(|| compact(&mut lists));
    assert_eq!(allocated.allocations, 0, "{allocated:?}");
    assert_eq!(lists.list(Walk::Main).expect("listed").len(), 8);
    assert!(stats.runs > 0 && !lists.records().is_empty(), "runs formed: {stats:?}");
}

/// A district's worth of draws: `instances` instances of three meshes of four, one and two
/// submeshes, a third culled by the frustum, a tenth by distance and a tenth by size, every
/// cascade and eight character maps drawn.
fn district(instances: u32) -> (DrawList, Vec<SlotCull>, Vec<SlotCaster>) {
    let meshes = vec![
        mesh(slab(0), vec![submesh(0, 10), submesh(3, 11), submesh(6, 12), submesh(9, 13)]),
        mesh(slab(4), vec![submesh(12, 14)]),
        mesh(slab(8), vec![submesh(15, 20), submesh(18, 15)]),
    ];
    let order: Vec<u32> = (0..instances).map(|slot| (slot * 7 + slot / 5) % 3).collect();
    let list = built(&meshes, &slots(&order));
    let verdict = |slot: u32| match slot % 30 {
        0..10 => Verdict::Frustum,
        10..13 => Verdict::Distance,
        13..16 => Verdict::Size,
        _ => Verdict::Drawn,
    };
    let culled = SlotCull::of(&(0..instances).map(verdict).collect::<Vec<_>>());
    let casters = (0..instances)
        .map(|slot| SlotCaster {
            cascades: (slot % 16) as u8,
            map: (slot % 5 == 0).then_some((slot % 8) as u8),
            held: 0b1111,
            far: verdict(slot) == Verdict::Distance,
            levels: [0, 1, 2, 3].map(|level| ((slot + level) % 3) as u8),
            map_level: 0,
            ..SlotCaster::default()
        })
        .collect();
    (list, culled, casters)
}

/// Each view lists a draw at its level (M5 D4): the main view's family at its culling's, each
/// cascade at its plan's, a character map at 0; a submesh its level simplified away is left out,
/// uncounted; and a cascade's group of one mesh at more than one level draws by (mesh, level,
/// submesh, record), as the key says.
#[test]
fn each_view_lists_its_levels() {
    // Mesh 0: two submeshes at level 0, the second simplified away at level 1. Mesh 1: one, at
    // both.
    let mut meshes =
        vec![mesh(slab(0), vec![submesh(0, 10), submesh(3, 11)]), mesh(slab(4), vec![submesh(6, 12)])];
    meshes[0].lods.push(vec![
        SubmeshDraw { index_count: 3, ..submesh(9, 10) },
        SubmeshDraw { index_count: 0, ..submesh(3, 11) },
    ]);
    meshes[1].lods.push(vec![submesh(12, 12)]);
    // Slots 0 to 3: meshes 0, 1, 0, 1. Records: slot 0's 0 and 1, slot 1's 2, slot 2's 3 and 4,
    // slot 3's 5.
    let list = built(&meshes, &slots(&[0, 1, 0, 1]));
    let commands = &list.commands[0][environment().index()];
    let bits: Vec<u8> = commands.iter().map(|command| command.levels).collect();
    assert_eq!(bits, [0b11, 0b01, 0b11, 0b11, 0b01, 0b11]);
    assert_eq!(commands[1].submesh, 1);
    // The main view draws slots 1 and 2 at level 1.
    let culled = SlotCull::at_levels(&[0, 1, 1, 0]);
    let tests = ViewTests { levels: Some(Culled::new(&culled)), ..world(&[], &[], None) };
    let mut lists = ViewLists::default();
    for walk in [Walk::Main, Walk::ScenePrepass] {
        lists.compact_view(walk, &list, &tests);
        let view = lists.list(walk).expect("listed");
        assert_eq!(
            levels(view, &list, DrawSet::Surfaces),
            [(0, 0), (1, 0), (2, 1), (3, 1), (5, 0)],
            "{walk:?}: record 4's submesh has no triangles at level 1"
        );
        assert_eq!(lists.left_out(walk), PassCounts::default(), "{walk:?}: and isn't counted");
    }
    // Cascade 0 draws every caster at level 1 but slot 3 (0); cascade 1 every one at 0.
    let plan = |slot: u32| SlotCaster {
        levels: [u8::from(slot != 3), 0, 0, 0],
        ..caster(0b11, (slot == 0).then_some(0))
    };
    let tests =
        CasterTests { stands: &[], cascades: [true, true, false, false], maps: 1, spots: [false; 4], plan };
    lists.compact_casters(&list, &tests);
    let cascade = |index| levels(lists.list(Walk::Cascade(index)).expect("listed"), &list, DrawSet::Surfaces);
    // Mesh 0's at level 1, slot 0's second and slot 2's second simplified away; mesh 1's slot 3
    // (record 5) at level 0 first, then slot 1's at level 1.
    assert_eq!(cascade(0), [(0, 1), (3, 1), (5, 0), (2, 1)], "mesh, level, record");
    // Cascade 1's at level 0: mesh 0's first submeshes, then its second (M5 D2), then mesh 1's.
    assert_eq!(cascade(1), [(0, 0), (3, 0), (1, 0), (4, 0), (2, 0), (5, 0)], "mesh, level, submesh, record");
    let map = lists.list(Walk::CharacterMap(0)).expect("listed");
    assert_eq!(levels(map, &list, DrawSet::Surfaces), [(0, 0), (1, 0)], "a map draws level 0");
    assert_eq!(lists.left_out(Walk::Cascade(0)), PassCounts::default());
}

/// The world's views compact in parallel to the very lists, and counts, they compact to one after
/// another: neither the thread count nor which finishes first shows (M5 D3).
#[test]
fn the_world_compacts_alike_in_parallel_and_in_turn() {
    let (list, culled, casters) = district(6000);
    assert!(list.draws.len() >= PARALLEL_DRAWS, "{} draws", list.draws.len());
    let world = world(&[], &[], Some(Culled::new(&culled)));
    let characters = world.with_instance(|slot: u32| slot % 3 == 2);
    let plan = |slot: u32| casters[slot as usize];
    let tests = CasterTests { stands: &[], cascades: [true; 4], maps: 8, spots: [false; 4], plan };
    let walks = [
        Walk::Main,
        Walk::ScenePrepass,
        Walk::CharacterPrepass,
        Walk::Cascade(0),
        Walk::Cascade(1),
        Walk::Cascade(2),
        Walk::Cascade(3),
        Walk::CharacterMap(0),
        Walk::CharacterMap(7),
        Walk::CharacterMaps,
    ];
    let lists = [false, true].map(|parallel| {
        let mut lists = ViewLists::default();
        lists.compact_world_as(&list, &world, &characters, &tests, parallel);
        lists
    });
    for walk in walks {
        let [serial, parallel] = &lists;
        assert_eq!(serial.list(walk), parallel.list(walk), "{walk:?}");
        assert_eq!(serial.left_out(walk), parallel.left_out(walk), "{walk:?}");
    }
    let main = lists[0].left_out(Walk::Main);
    assert!(main.culled_by_frustum > 0 && main.culled_by_distance > 0 && main.culled_by_size > 0, "{main:?}");
}

/// The cull's cost at a district's scale, serial and in parallel: run alone, in release, with
/// `cargo test --release -p sapphire_render --lib cull_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a timing report"]
#[expect(clippy::disallowed_methods, reason = "a timing report, not engine code")]
fn cull_cost_at_a_districts_scale() {
    let (list, culled, casters) = district(6000);
    let world = world(&[], &[], Some(Culled::new(&culled)));
    let characters = world.with_instance(|slot: u32| slot % 3 == 2);
    let plan = |slot: u32| casters[slot as usize];
    let tests = CasterTests { stands: &[], cascades: [true; 4], maps: 8, spots: [false; 4], plan };
    let mut lists = ViewLists::default();
    for parallel in [false, true, false, true] {
        lists.compact_world_as(&list, &world, &characters, &tests, parallel);
        let start = std::time::Instant::now();
        let rounds = 200;
        for _ in 0..rounds {
            lists.compact_world_as(&list, &world, &characters, &tests, parallel);
        }
        let micros = start.elapsed().as_secs_f64() * 1e6 / f64::from(rounds);
        eprintln!("{} draws, parallel {parallel}: {micros:.1} us a frame", list.draws.len());
    }
}
