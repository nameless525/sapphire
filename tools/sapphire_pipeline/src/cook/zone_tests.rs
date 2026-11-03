//! Zones through a whole cook: sidecars, the cache, cells' keys, and the runtime's reads of what they
//! write. The document and the cooked format's own checks are `sapphire_zone`'s tests.

use std::error::Error;
use std::time::Instant;

use glam::Vec3;
use sapphire_assets::material::AnimeCharacterMaterial;
use sapphire_core::Name;
use sapphire_zone::{CellCoord, ZoneDocument};
use serde_json::json;

use super::clip_tests::{Tree, by_id};
use super::*;
use crate::gltf_writer::{Geometry, GltfWriter};
use crate::material::MaterialSettings;
use crate::meta::{AssetMeta, sidecar_path};
use crate::pack::pack_cooked;
use crate::test_util::read_aligned;

const ZONE: AssetId = AssetId::from_bytes([0x20; 16]);
const CRATE: AssetId = AssetId::from_bytes([0x21; 16]);
const BLOCK: AssetId = AssetId::from_bytes([0x22; 16]);
const WOOD: AssetId = AssetId::from_bytes([0x23; 16]);
const STONE: AssetId = AssetId::from_bytes([0x24; 16]);
const COLLIDERS: AssetId = AssetId::from_bytes([0x25; 16]);
const LAMP: AssetId = AssetId::from_bytes([0x26; 16]);

/// A square of two triangles, a unit wide.
fn piece_glb(height: f32) -> Vec<u8> {
    let mut writer = GltfWriter::default();
    let square = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, height, 0.0], [0.0, height, 0.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Geometry::default()
    };
    let mesh = writer.add_mesh("Square", &[(&square, None)]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    writer.to_glb()
}

/// A prefab's document, with these slots, placing `place`.
fn assembly_text(slots: &[&str], place: &str) -> String {
    let slots: Vec<String> = slots.iter().map(|slot| format!(r##"(name: "{slot}")"##)).collect();
    format!(
        r##"(type: "sapphire::pipeline::Assembly", version: 1, data: (
            kits: {{"kit": "../kit"}},
            slots: [{}],
            surfaces: [],
            place: [{place}],
        ))"##,
        slots.join(", ")
    )
}

/// A zone document with 16 m cells.
fn zone_text(data: &str) -> String {
    format!(
        r##"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 16.0), {data}))"##
    )
}

/// The zone most tests cook: a crate and a block in cell (0, 0), a block in (1, 0), a crate in another
/// material in (-1, -1), and a block in the backdrop.
fn placements() -> String {
    format!(
        r##"placements: [
            (prefab: "{CRATE}", at: (2.0, 0.0, 2.0)),
            (prefab: "{BLOCK}", at: (5.0, 0.0, 5.0)),
            (prefab: "{BLOCK}", at: (20.0, 0.0, 4.0)),
            (prefab: "{CRATE}", at: (-4.0, 0.0, -4.0), materials: {{"wood": "{STONE}"}}),
            (prefab: "{BLOCK}", at: (100.0, 0.0, 100.0), backdrop: true),
        ],"##
    )
}

/// A content tree holding the zone's materials and prefabs, ready for a zone document.
fn tree(name: &str) -> Tree {
    let tree = Tree::new(name);
    for (name, id) in [("wood", WOOD), ("stone", STONE)] {
        let text = sapphire_serialize::to_ron(&AnimeCharacterMaterial::default()).unwrap();
        let path = tree.write(&format!("materials/{name}.material.ron"), text.as_bytes());
        tree.sidecar(&path, id, ImportSettings::Material(MaterialSettings::default()));
    }
    let piece = tree.write("kit/crate.glb", &piece_glb(1.0));
    uncooked(&piece, AssetId::from_bytes([0x27; 16]), ImportSettings::Gltf(MeshSettings::default()));
    prefab(
        &tree,
        "crate",
        CRATE,
        &["wood"],
        r##"(piece: "kit/crate", slot: Some("wood"))"##,
        &[("wood", WOOD)],
    );
    prefab(
        &tree,
        "block",
        BLOCK,
        &["stone"],
        r##"(piece: "#box", scale: 2.0, slot: Some("stone"))"##,
        &[("stone", STONE)],
    );
    tree
}

fn uncooked(source: &Path, id: AssetId, importer: ImportSettings) {
    let meta = AssetMeta { cook: false, ..AssetMeta::new(id, importer) };
    fs::write(sidecar_path(source), sapphire_serialize::to_ron::<AssetMeta>(&meta).unwrap()).unwrap();
}

/// Writes a prefab that cooks no mesh of its own, its slots bound as `bindings` says.
fn prefab(tree: &Tree, name: &str, id: AssetId, slots: &[&str], place: &str, bindings: &[(&str, AssetId)]) {
    let path = tree.write(&format!("prefabs/{name}.assembly.ron"), assembly_text(slots, place).as_bytes());
    let materials = bindings.iter().map(|(slot, id)| ((*slot).to_owned(), *id)).collect();
    uncooked(&path, id, ImportSettings::Assembly(MeshSettings { materials, ..MeshSettings::default() }));
}

fn write_zone(tree: &Tree, data: &str) -> PathBuf {
    let path = tree.write("town.zone.ron", zone_text(data).as_bytes());
    tree.sidecar(&path, ZONE, ImportSettings::Zone(zone::ZoneSettings::default()));
    path
}

fn cell_id(place: Place) -> AssetId {
    ZONE.sub_asset(&zone::part_name(place))
}

fn cell(x: i32, z: i32) -> Place {
    Place::Cell(CellCoord { x, z })
}

fn read_zone(report: &CookReport) -> sapphire_serialize::cooked::AlignedVec {
    read_aligned(&by_id(report, ZONE).output)
}

/// An error and everything under it, as one line.
fn chain(error: &CookError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        text = format!("{text}: {next}");
        source = next.source();
    }
    text
}

#[test]
fn a_zone_cooks_to_a_mesh_per_cell_in_the_zones_space() {
    let tree = tree("zone-cells");
    write_zone(&tree, &placements());
    let report = tree.cook();

    // Prefabs cook nothing of their own: nothing about them reaches the runtime.
    assert!(!report.assets.iter().any(|asset| asset.id == CRATE || asset.id == BLOCK));
    assert!(report.uncooked.iter().any(|source| source.ends_with("crate.assembly.ron")));

    let bytes = read_zone(&report);
    let zone = Zone::read(&bytes).unwrap();
    let coords: Vec<CellCoord> = zone.presentation.cells.iter().map(|cell| cell.coord()).collect();
    assert_eq!(coords, [CellCoord { x: -1, z: -1 }, CellCoord { x: 0, z: 0 }, CellCoord { x: 1, z: 0 }]);
    let backdrop = zone.presentation.backdrop.as_ref().expect("holds a block");
    assert_eq!(backdrop.mesh.as_ref().copied(), Some(cell_id(Place::Backdrop)));

    // Each named mesh is there, with its pack name for the file, in the zone's space.
    let right = zone.presentation.cell_at(CellCoord { x: 1, z: 0 }).unwrap();
    assert_eq!(right.contents.mesh.as_ref().copied(), Some(cell_id(cell(1, 0))));
    let asset = by_id(&report, cell_id(cell(1, 0)));
    assert!(asset.output.ends_with("town.cell.1.0.smesh"), "{}", asset.output.display());
    let bounds = right.contents.bounds();
    assert_eq!((bounds.min, bounds.max), ([19.0, -1.0, 3.0], [21.0, 1.0, 5.0]));
    let mesh = read_aligned(&asset.output);
    let mesh = Mesh::read(&mesh).unwrap();
    assert_eq!(mesh.streams.bounds.to_native().max, [21.0, 1.0, 5.0], "it draws with no transform");
}

#[test]
fn a_cells_submeshes_are_its_final_materials() {
    let tree = tree("zone-materials");
    write_zone(&tree, &placements());
    let report = tree.cook();
    let materials = |place| {
        let bytes = read_aligned(&by_id(&report, cell_id(place)).output);
        let mesh = Mesh::read(&bytes).unwrap();
        let ids: Vec<Option<AssetId>> = mesh
            .materials
            .iter()
            .map(|slot| match &slot.material {
                sapphire_assets::mesh::ArchivedSlotMaterial::Asset(id) => Some(*id),
                sapphire_assets::mesh::ArchivedSlotMaterial::Embedded(_) => None,
            })
            .collect();
        ids
    };
    assert_eq!(materials(cell(0, 0)), [Some(WOOD), Some(STONE)], "a crate and a block");
    assert_eq!(materials(cell(-1, -1)), [Some(STONE)], "the placement's swap took the sidecar's place");
}

#[test]
fn a_mirrored_piece_keeps_its_faces_outward() {
    let tree = tree("zone-mirror");
    let normal_of = |report: &CookReport| {
        let bytes = read_aligned(&by_id(report, cell_id(cell(0, 0))).output);
        let mesh = Mesh::read(&bytes).unwrap();
        let corner = |index: usize| {
            Vec3::from(mesh.streams.position(mesh.indices[index].to_native() as usize).expect("in range"))
        };
        let corners = [corner(0), corner(1), corner(2)];
        (corners[1] - corners[0]).cross(corners[2] - corners[0]).normalize()
    };
    write_zone(&tree, &format!(r##"placements: [(prefab: "{CRATE}", at: (8.0, 0.0, 8.0))]"##));
    let plain = normal_of(&tree.cook());
    write_zone(
        &tree,
        &format!(r##"placements: [(prefab: "{CRATE}", at: (8.0, 0.0, 8.0), scale: (-1.0, 1.0, 1.0))]"##),
    );
    let mirrored = normal_of(&tree.cook());
    assert_eq!(plain, mirrored, "a square in XY still faces +Z, its corners swapped to say so");
}

#[test]
fn a_no_op_cook_writes_nothing() {
    let tree = tree("zone-no-op");
    write_zone(&tree, &placements());
    tree.cook();
    let again = tree.cook();
    assert_eq!(again.count(CookStatus::UpToDate), again.assets.len());
}

/// What each of a report's zone assets did, by place, and the `.szone` as `None`.
fn zone_statuses(report: &CookReport) -> BTreeMap<Option<Place>, CookStatus> {
    let of = |place: Place| (Some(place), by_id(report, cell_id(place)).status);
    let mut statuses: BTreeMap<_, _> =
        [of(cell(-1, -1)), of(cell(0, 0)), of(cell(1, 0)), of(Place::Backdrop)].into_iter().collect();
    statuses.insert(None, by_id(report, ZONE).status);
    statuses
}

fn cooked_places(report: &CookReport) -> Vec<Option<Place>> {
    let statuses = zone_statuses(report);
    statuses.iter().filter(|(_, status)| **status == CookStatus::Cooked).map(|(place, _)| *place).collect()
}

#[test]
fn moving_one_placement_recooks_its_cell_and_the_zone() {
    let tree = tree("zone-one-placement");
    write_zone(&tree, &placements());
    tree.cook();
    write_zone(&tree, &placements().replace("(20.0, 0.0, 4.0)", "(21.0, 0.0, 4.0)"));
    assert_eq!(cooked_places(&tree.cook()), [None, Some(cell(1, 0))]);

    // Out of one cell and into another: both.
    write_zone(&tree, &placements().replace("(20.0, 0.0, 4.0)", "(3.0, 0.0, 9.0)"));
    let report = tree.cook();
    assert!(report.assets.iter().all(|asset| asset.id != cell_id(cell(1, 0))), "that cell is empty now");
    assert_eq!(by_id(&report, cell_id(cell(0, 0))).status, CookStatus::Cooked);
    assert_eq!(by_id(&report, cell_id(cell(-1, -1))).status, CookStatus::UpToDate);
    assert!(!by_id(&tree.cook(), ZONE).output.with_file_name("town.cell.1.0.smesh").exists(), "pruned");
}

#[test]
fn what_doesnt_touch_a_cell_doesnt_recook_it() {
    let tree = tree("zone-spawner");
    let colliders = tree.write(
        "town.colliders.ron",
        br##"(type: "sapphire::colliders::ColliderDocument", version: 1, data: (colliders: [
            (name: "ground", shape: Box(half_extents: (9.0, 0.5, 9.0)), position: (0.0, -0.5, 0.0)),
        ]))"##,
    );
    tree.sidecar(&colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
    let spawner = |y: f32| {
        format!(
            r##"{} colliders: Some("{COLLIDERS}"), spawners: [(name: "start", position: (0.0, {y:?}, 0.0), spawns: "player")],"##,
            placements()
        )
    };
    write_zone(&tree, &spawner(0.0));
    tree.cook();
    write_zone(&tree, &spawner(1.0));
    let report = tree.cook();
    assert_eq!(cooked_places(&report), [None]);
    let bytes = read_zone(&report);
    let zone = Zone::read(&bytes).unwrap();
    assert_eq!(zone.simulation.spawner(Name::new("start")).unwrap().position()[1], 1.0);
    assert_eq!(zone.references(), [COLLIDERS], "colliders load with the zone");
    assert!(!zone.references().contains(&cell_id(cell(0, 0))), "a cell's mesh doesn't");
    assert!(zone.presentation_references().contains(&cell_id(cell(0, 0))));
}

#[test]
fn saving_a_prefab_or_its_piece_recooks_only_the_cells_that_place_it() {
    let tree = tree("zone-prefab");
    write_zone(&tree, &placements());
    tree.cook();

    tree.write("kit/crate.glb", &piece_glb(2.0));
    assert_eq!(cooked_places(&tree.cook()), [None, Some(cell(-1, -1)), Some(cell(0, 0))], "a kit piece");

    prefab(
        &tree,
        "block",
        BLOCK,
        &["stone"],
        r##"(piece: "#box", scale: 3.0, slot: Some("stone"))"##,
        &[("stone", STONE)],
    );
    let report = tree.cook();
    assert_eq!(
        cooked_places(&report),
        [None, Some(Place::Backdrop), Some(cell(0, 0)), Some(cell(1, 0))],
        "a document"
    );

    // The sidecar's binding is what a cell's material comes from.
    prefab(
        &tree,
        "block",
        BLOCK,
        &["stone"],
        r##"(piece: "#box", scale: 3.0, slot: Some("stone"))"##,
        &[("stone", WOOD)],
    );
    let report = tree.cook();
    assert_eq!(
        cooked_places(&report),
        [None, Some(Place::Backdrop), Some(cell(0, 0)), Some(cell(1, 0))],
        "a sidecar"
    );
}

#[test]
fn a_prefab_saved_after_the_plan_read_it_fails_the_cook() {
    let tree = tree("zone-saved");
    write_zone(&tree, &placements());
    let result = tree.try_cook_through(&|stage| {
        if stage == CookStage::Planned {
            prefab(
                &tree,
                "block",
                BLOCK,
                &["stone"],
                r##"(piece: "#box", scale: 3.0, slot: Some("stone"))"##,
                &[("stone", STONE)],
            );
        }
    });
    let path = match result {
        Err(CookError::SourceChanged { path }) => path,
        other => panic!("expected the save to be seen, not {other:?}"),
    };
    assert!(path.ends_with("block.assembly.ron"), "{}", path.display());
    tree.cook();
}

#[test]
fn saving_a_day_or_rig_library_recooks_only_the_zone() {
    let tree = tree("zone-day");
    let day = include_str!("../../../../content/lookdev/lookdev_day.ron");
    let rigs = |preset: &str| {
        format!(r##"(type: "sapphire::world::LightRigLibrary", version: 1, data: (presets: "{preset}"))"##)
    };
    tree.write("day.ron", day.as_bytes());
    tree.write("rigs.ron", rigs("one").as_bytes());
    write_zone(&tree, &format!(r##"{} day: Some("day.ron"), rigs: Some("rigs.ron"),"##, placements()));
    let report = tree.cook();
    let bytes = read_zone(&report);
    assert_eq!(Zone::read(&bytes).unwrap().presentation.day.as_ref().map(|day| day.as_str()), Some(day));

    tree.write("rigs.ron", rigs("two").as_bytes());
    let report = tree.cook();
    assert_eq!(cooked_places(&report), [None]);
    let bytes = read_zone(&report);
    let read = Zone::read(&bytes).unwrap();
    assert!(read.presentation.rigs.as_ref().unwrap().as_str().contains("two"));
}

/// A static mesh of its own, for a zone's instances.
fn add_lamp(tree: &Tree) {
    let lamp = tree.write(
        "lamp.assembly.ron",
        assembly_text(&["stone"], r##"(piece: "#box", at: (0.0, 1.0, 0.0), slot: Some("stone"))"##)
            .replace("../kit", "kit")
            .as_bytes(),
    );
    let mut settings = MeshSettings::default();
    settings.materials.insert("stone".to_owned(), STONE);
    tree.sidecar(&lamp, LAMP, ImportSettings::Assembly(settings));
}

#[test]
fn an_instance_stands_in_its_cell_with_its_meshs_bounds_placed() {
    let tree = tree("zone-instance");
    add_lamp(&tree);
    write_zone(
        &tree,
        &format!(
            r##"instances: [(mesh: "{LAMP}", at: (40.0, 2.0, 8.0), turn: 90.0, scale: (2.0, 1.0, 4.0))],
               lights: [(position: (40.0, 3.0, 8.0), color: (1.0, 1.0, 1.0), intensity: 1.0, range: 1.0)],"##
        ),
    );
    let report = tree.cook();
    let bytes = read_zone(&report);
    let zone = Zone::read(&bytes).unwrap();
    let held = zone.presentation.cell_at(CellCoord { x: 2, z: 0 }).expect("anchored there");
    assert!(held.contents.mesh.is_none(), "no placements, no merged mesh");
    assert_eq!(held.contents.instances().len(), 1);
    assert_eq!(held.contents.lights().len(), 1);
    assert_eq!(zone.presentation.instances[0].mesh, LAMP);
    let bounds = held.contents.bounds();
    // A unit box turned a quarter turn: its 4 m Z is X now, and its 2 m X is Z.
    let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
    assert!(close(bounds.min[0], 38.0) && close(bounds.max[0], 42.0), "{bounds:?}");
    assert!(close(bounds.min[2], 7.0) && close(bounds.max[2], 9.0), "{bounds:?}");

    // Saving the mesh recooks the zone, whose bounds came from it.
    add_lamp(&tree);
    let path = tree.content().join("lamp.assembly.ron");
    let bigger =
        fs::read_to_string(&path).unwrap().replace("(piece: \"#box\",", "(piece: \"#box\", scale: 2.0,");
    fs::write(&path, bigger).unwrap();
    assert_eq!(by_id(&tree.cook(), ZONE).status, CookStatus::Cooked);
}

/// One whole town document, cut by a placement's `split`: two boxes in two cells, and a quad too wide
/// for either.
fn add_town(tree: &Tree) {
    prefab(
        tree,
        "town",
        AssetId::from_bytes([0x28; 16]),
        &["stone", "wood"],
        r##"
        (piece: "#box", at: (2.0, 0.0, 2.0), slot: Some("stone")),
        (piece: "@row", at: (20.0, 0.0, 4.0)),
        (piece: "#quad", scale: 100.0, slot: Some("wood")),
        "##,
        &[("stone", STONE), ("wood", WOOD)],
    );
    let path = tree.content().join("prefabs/town.assembly.ron");
    let groups = fs::read_to_string(&path).unwrap().replace(
        "surfaces: [],",
        r##"surfaces: [], groups: {"row": [(piece: "#box", slot: Some("stone")), (piece: "#box", at: (0.0, 2.0, 0.0), slot: Some("stone"))]},"##,
    );
    fs::write(path, groups).unwrap();
}

const TOWN: AssetId = AssetId::from_bytes([0x28; 16]);

#[test]
fn a_split_prefab_goes_to_cells_by_its_own_placements_and_whole_groups_stay_whole() {
    let tree = tree("zone-split");
    add_town(&tree);
    write_zone(&tree, &format!(r##"placements: [(prefab: "{TOWN}", split: Some((backdrop: ["#quad"])))],"##));
    let report = tree.cook();
    let triangles = |place: Place| {
        let bytes = read_aligned(&by_id(&report, cell_id(place)).output);
        Mesh::read(&bytes).unwrap().indices.len() / 3
    };
    assert_eq!(triangles(cell(0, 0)), 12, "one box");
    assert_eq!(triangles(cell(1, 0)), 24, "a group's two boxes");
    assert!(triangles(Place::Backdrop) >= 2, "the quad");

    let bytes = read_zone(&report);
    let zone = Zone::read(&bytes).unwrap();
    assert!(zone.presentation.backdrop.as_ref().unwrap().bounds().max[0] > 40.0);
    assert!(zone.presentation.cell_at(CellCoord { x: 1, z: 0 }).unwrap().contents.bounds().max[1] > 2.0);

    // Editing one part of a split placement's *transform* moves them all, and so recooks them.
    write_zone(
        &tree,
        &format!(
            r##"placements: [(prefab: "{TOWN}", at: (0.0, 1.0, 0.0), split: Some((backdrop: ["#quad"])))],"##
        ),
    );
    assert_eq!(by_id(&tree.cook(), cell_id(cell(1, 0))).status, CookStatus::Cooked);
}

#[test]
fn what_a_zone_gets_wrong_is_named() {
    let tree = tree("zone-errors");
    add_town(&tree);
    add_lamp(&tree);
    prefab(
        &tree,
        "unbound",
        AssetId::from_bytes([0x29; 16]),
        &["bare"],
        r##"(piece: "#box", slot: Some("bare"))"##,
        &[],
    );
    let not_a_day = r##"(type: "sapphire::world::LightRigLibrary", version: 1, data: ())"##;
    let not_rigs = r##"(type: "sapphire::world::TodTimeline", version: 1, data: ())"##;
    tree.write("not_a_day.ron", not_a_day.as_bytes());
    tree.write("not_rigs.ron", not_rigs.as_bytes());
    let cases: Vec<(String, &str)> = vec![
        (format!(r##"placements: [(prefab: "{ZONE}")],"##), "placements[0].prefab"),
        (format!(r##"placements: [(prefab: "{WOOD}")],"##), "isn't an assembly"),
        (
            format!(r##"placements: [(prefab: "{CRATE}", materials: {{"steel": "{WOOD}"}})],"##),
            "no slot `steel`",
        ),
        (
            format!(r##"placements: [(prefab: "{CRATE}", materials: {{"wood": "{BLOCK}"}})],"##),
            "placements[0].wood",
        ),
        (format!(r##"placements: [(prefab: "{}")],"##, AssetId::from_bytes([0x29; 16])), "doesn't bind"),
        (format!(r##"placements: [(prefab: "{TOWN}")],"##), "placements[0]`: reaches"),
        (
            format!(r##"placements: [(prefab: "{TOWN}", split: Some(()))],"##),
            "placements[0].place[2]`: reaches",
        ),
        (
            format!(r##"placements: [(prefab: "{TOWN}", split: Some((backdrop: ["#gone"])))],"##),
            "`#gone` matches none",
        ),
        (format!(r##"colliders: Some("{WOOD}"),"##), "`colliders`"),
        (format!(r##"probes: Some("{WOOD}"),"##), "`probes`"),
        (format!(r##"instances: [(mesh: "{WOOD}")],"##), "`instances[0].mesh`"),
        (format!(r##"instances: [(mesh: "{LAMP}", scale: (99.0, 1.0, 1.0))],"##), "`instances[0]`"),
        (format!(r##"navigation: ["{WOOD}"],"##), "no navigation tiles"),
        (format!(r##"encounters: ["{WOOD}"],"##), "no encounters"),
        (r##"day: Some("missing.ron"),"##.to_owned(), "failed to read"),
        (r##"day: Some("not_a_day.ron"),"##.to_owned(), "`day`"),
        (r##"rigs: Some("not_rigs.ron"),"##.to_owned(), "not a `sapphire::world::LightRigLibrary`"),
    ];
    for (data, expected) in cases {
        write_zone(&tree, &data);
        let error = tree.try_cook(false).expect_err(&data);
        let text = chain(&error);
        assert!(text.contains("town.zone.ron"), "{data}: the zone's named in `{text}`");
        assert!(text.contains(expected), "{data}: `{text}` must say `{expected}`");
    }
}

#[test]
fn a_zones_cook_is_the_same_bytes_on_any_number_of_threads_and_every_time() {
    let cook_with = |name: &str, workers: usize| {
        let tree = tree(name);
        add_town(&tree);
        add_lamp(&tree);
        let data = format!(
            r##"{} instances: [(mesh: "{LAMP}", at: (-3.0, 0.0, 5.0))],"##,
            placements().replace(
                "placements: [",
                &format!(r##"placements: [(prefab: "{TOWN}", at: (0.0, 0.0, 64.0), split: Some((backdrop: ["#quad"]))),"##)
            )
        );
        write_zone(&tree, &data);
        let cache = tree.dir.path().join("cache");
        let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false };
        let report = cook_dir_on(&tree.content(), &tree.out(), &options, workers).unwrap();
        report.assets.iter().map(|asset| (asset.id, fs::read(&asset.output).unwrap())).collect::<Vec<_>>()
    };
    let serial = cook_with("zone-serial", 1);
    assert!(serial.iter().any(|(id, _)| *id == ZONE));
    assert_eq!(serial, cook_with("zone-serial-again", 1));
    assert_eq!(serial, cook_with("zone-workers", 4));
}

/// How long `run` took, in milliseconds.
fn timed<T>(run: impl FnOnce() -> T) -> (T, f64) {
    #[allow(clippy::disallowed_methods, reason = "a measurement to print, in a test a person runs")]
    let start = Instant::now();
    let result = run();
    (result, start.elapsed().as_secs_f64() * 1000.0)
}

/// What a cook did with a large zone, as this prints it.
fn cooked_counts(report: &CookReport) -> String {
    format!(
        "cooked {}, cached {}, up to date {}",
        report.count(CookStatus::Cooked),
        report.count(CookStatus::Cached),
        report.count(CookStatus::UpToDate)
    )
}

/// Not a test of anything but what a large zone costs: a generated zone of 3,600 placed kit pieces in
/// 256 cells of 32 m, cold and warm, to read off its printout. Copies the kit's files to a scratch
/// directory, from which nothing is kept. In release, as a cook is run:
/// `cargo test -p sapphire_pipeline --release --lib large_zone -- --ignored --nocapture`.
#[test]
#[ignore = "measures a large zone's cook, for a person to read"]
fn large_zone_costs() {
    const SIDE: i32 = 16;
    const PER_CELL: i32 = 14;
    let kit =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party/kenney_fantasy_town_kit/glb");
    let tree = Tree::new("zone-large");
    for (name, id) in [("wood", WOOD), ("stone", STONE)] {
        let text = sapphire_serialize::to_ron(&AnimeCharacterMaterial::default()).unwrap();
        let path = tree.write(&format!("materials/{name}.material.ron"), text.as_bytes());
        tree.sidecar(&path, id, ImportSettings::Material(MaterialSettings::default()));
    }
    let names = [
        "wall",
        "wall-door",
        "wall-window-glass",
        "roof",
        "roof-gable",
        "roof-corner",
        "pillar-stone",
        "pillar-wood",
        "fence",
        "cart",
        "stall",
        "stall-bench",
        "tree",
        "tree-high",
        "rock-large",
        "rock-small",
        "lantern",
        "fountain-round",
        "stairs-stone",
        "planks",
        "banner-red",
        "hedge",
        "chimney",
        "wheel",
    ];
    for name in names {
        let bytes = fs::read(kit.join(format!("{name}.glb"))).unwrap();
        let path = tree.write(&format!("kit/{name}.glb"), &bytes);
        uncooked(
            &path,
            AssetId::from_bytes([0u8; 16]).sub_asset(name),
            ImportSettings::Gltf(MeshSettings::default()),
        );
        let id = AssetId::from_bytes([1u8; 16]).sub_asset(name);
        let slot = if name.contains("wood") || name == "fence" { "wood" } else { "stone" };
        let place = format!(r##"(piece: "kit/{name}", slot: Some("{slot}"))"##);
        prefab(&tree, name, id, &[slot], &place, &[(slot, if slot == "wood" { WOOD } else { STONE })]);
    }
    let mut placements = String::from("placements: [");
    let mut placed = 0;
    for x in -SIDE / 2..SIDE / 2 {
        for z in -SIDE / 2..SIDE / 2 {
            for i in 0..PER_CELL {
                let name = names[(x * 7 + z * 13 + i * 5).rem_euclid(names.len() as i32) as usize];
                let id = AssetId::from_bytes([1u8; 16]).sub_asset(name);
                let at = |of: i32, step: i32| of as f32 * 32.0 + 1.0 + (i * step % 30) as f32;
                let (ax, az) = (at(x, 3), at(z, 7));
                placements.push_str(&format!(
                    r##"(prefab: "{id}", at: ({ax:?}, 0.0, {az:?}), turn: {:?}),"##,
                    (i * 40) as f32
                ));
                placed += 1;
            }
        }
    }
    placements.push(']');
    let written = zone_text(&format!("{placements},")).replace("cell_size: 16.0", "cell_size: 32.0");
    let path = tree.write("large.zone.ron", written.as_bytes());
    tree.sidecar(&path, ZONE, ImportSettings::Zone(zone::ZoneSettings::default()));
    eprintln!("{placed} placements of {} prefabs, a document of {} KiB", names.len(), written.len() / 1024);

    let packs = tree.dir.path().join("packs");
    let cache = tree.dir.path().join("cache");
    let pack = |report: &CookReport| timed(|| pack_cooked(report, &packs, Some(&cache)).unwrap());
    let (report, cold) = timed(|| tree.cook());
    let (_, cold_pack) = pack(&report);
    let zone_size = fs::metadata(&by_id(&report, ZONE).output).unwrap().len();
    let cells = report.assets.iter().filter(|asset| asset.source == path && asset.id != ZONE);
    let meshes: Vec<u64> = cells.map(|asset| fs::metadata(&asset.output).unwrap().len()).collect();
    let pack_size = fs::metadata(packs.join("zone_large.spack")).unwrap().len();
    let bytes = read_zone(&report);
    let zone = Zone::read(&bytes).unwrap();
    assert_eq!(zone.presentation.cells.len(), (SIDE * SIDE) as usize);
    eprintln!(
        "cold cook {cold:.0} ms, its pack {cold_pack:.0} ms; {} cells; .szone {zone_size} B, cell meshes {} KiB in all, \
         largest {} KiB, pack {} KiB",
        meshes.len(),
        meshes.iter().sum::<u64>() / 1024,
        meshes.iter().max().unwrap() / 1024,
        pack_size / 1024,
    );

    for round in 0..3 {
        let (report, cook) = timed(|| tree.cook());
        let (_, pack_time) = pack(&report);
        eprintln!("no-op {round}: cook {cook:.0} ms, pack {pack_time:.0} ms; {}", cooked_counts(&report));
    }

    let edited = written.replacen("turn: 0.0", "turn: 11.0", 1);
    tree.write_at(&path, edited.as_bytes());
    let (report, one_placement) = timed(|| tree.cook());
    let (_, pack_time) = pack(&report);
    eprintln!(
        "one placement moved: cook {one_placement:.0} ms, pack {pack_time:.0} ms; {}",
        cooked_counts(&report)
    );
    assert_eq!(report.count(CookStatus::Cooked), 2, "its cell and the zone");

    let piece = tree.content().join("kit/hedge.glb");
    let mut bytes = fs::read(&piece).unwrap();
    bytes.extend_from_slice(&[b' '; 4]);
    fs::write(&piece, bytes).unwrap();
    let (report, one_piece) = timed(|| tree.cook());
    let (_, pack_time) = pack(&report);
    eprintln!(
        "one kit piece saved: cook {one_piece:.0} ms, pack {pack_time:.0} ms; {}",
        cooked_counts(&report)
    );

    let document = tree.content().join("prefabs/hedge.assembly.ron");
    let saved = fs::read_to_string(&document).unwrap();
    fs::write(&document, format!("// saved\n{saved}")).unwrap();
    let (report, one_prefab) = timed(|| tree.cook());
    eprintln!("one prefab's document saved: cook {one_prefab:.0} ms; {}", cooked_counts(&report));
}

/// A no-op's plan comes from the copy of its parse, which must cook just as the text does: every bit of
/// a float, and a document with everything in it, read back whole.
#[test]
fn a_documents_kept_parse_is_the_document() {
    let mut floats = vec![0.1, -0.0, f32::MIN_POSITIVE, f32::MAX, 1e-7, 16_777_216.0];
    let mut bits = 0x9e37_79b9u32;
    for _ in 0..200_000 {
        bits ^= bits << 13;
        bits ^= bits >> 17;
        bits ^= bits << 5;
        floats.extend(Some(f32::from_bits(bits)).filter(|float| float.is_finite()));
    }
    let back: Vec<f32> = serde_json::from_slice(&serde_json::to_vec(&floats).unwrap()).unwrap();
    assert!(floats.iter().zip(&back).all(|(a, b)| a.to_bits() == b.to_bits()));

    let text = zone_text(&format!(
        r##"{}
        lights: [(position: (0.1, 2.0, 3.0), color: (1.0, 0.5, 0.25), intensity: 3.0, range: 9.0,
            shape: Spot(direction: (0.0, -1.0, 0.0), inner_degrees: 10.0, outer_degrees: 20.0), at_night: true)],
        spawners: [(name: "start", position: (0.1, 0.2, 0.3), yaw_degrees: 45.0, spawns: "player")],
        triggers: [(name: "gate", position: (1.0, 2.0, 3.0), volume: Sphere(radius: 2.5), event: "open")],
        splines: [(name: "rail", points: [(0.0, 1.0, 2.0), (3.0, 4.0, 5.0)], closed: true)],"##,
        placements()
    ));
    let parsed = ZoneDocument::from_ron(&text).unwrap();
    let back: ZoneDocument = serde_json::from_slice(&serde_json::to_vec(&parsed).unwrap()).unwrap();
    assert_eq!(back, parsed);
}

/// A kept parse is for the bytes it came from: a save parses, and a broken one's refused by the field.
#[test]
fn a_saved_document_is_parsed_again() {
    let tree = Tree::new("zone-kept-parse");
    let path = tree.write("a.zone.ron", zone_text("").as_bytes());
    tree.sidecar(&path, ZONE, ImportSettings::Zone(zone::ZoneSettings::default()));
    tree.cook();
    tree.cook();
    tree.write_at(&path, zone_text("").replace("cell_size: 16.0", "cell_size: 1.0").as_bytes());
    let error = tree.try_cook(false).unwrap_err();
    assert!(format!("{error:?}").contains("grid.cell_size"), "{error:?}");
}
