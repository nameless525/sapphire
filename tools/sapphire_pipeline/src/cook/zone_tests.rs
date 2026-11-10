//! Zones through a whole cook: sidecars, the cache, cells' keys, and the runtime's reads of what they
//! write. The document and the cooked format's own checks are `sapphire_zone`'s tests.

use std::collections::BTreeSet;
use std::error::Error;
use std::io::Write as _;
use std::time::Instant;

use glam::Vec3;
use sapphire_assets::AssetKind;
use sapphire_assets::material::AnimeCharacterMaterial;
use sapphire_assets::pack::PackReader;
use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Group, Rotation};
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

/// The ID of the mesh of placement `index`'s shape in `data`.
fn shape_id(data: &str, index: usize) -> AssetId {
    let document = ZoneDocument::from_ron(&zone_text(data)).unwrap();
    ZONE.sub_asset(&document.placements[index].shape_name())
}

#[test]
fn an_editors_cook_has_a_mesh_of_each_shape_in_its_prefabs_space_shipped_alone() {
    let tree = tree("zone-shapes");
    let data = format!(
        r##"{}
        instances: [],"##,
        placements().replace("(-4.0, 0.0, -4.0),", "(-4.0, 0.0, -4.0), scale: (-2.0, 1.0, 1.0),")
    );
    write_zone(&tree, &data);
    let plain = tree.cook();
    assert!(
        plain.assets.iter().all(|asset| !asset.output.to_string_lossy().contains(".prefab.")),
        "a cook that ships has none"
    );
    let report = tree.cook_for_editor();
    assert_eq!(report.assets.len(), plain.assets.len() + 3, "an editor's cook is the same, and the shapes");

    // A plain crate, three blocks however placed, and a mirrored crate in stone: three shapes.
    let shapes: BTreeSet<AssetId> = (0..5).map(|index| shape_id(&data, index)).collect();
    assert_eq!(shapes.len(), 3);
    assert_eq!(shape_id(&data, 1), shape_id(&data, 4), "a block wherever it stands");
    let crate_mesh = by_id(&report, shape_id(&data, 0));
    let name = crate_mesh.output.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("town.prefab.") && name.ends_with(".smesh"), "{name}");
    let read = |id| {
        let bytes = read_aligned(&by_id(&report, id).output);
        let mesh = Mesh::read(&bytes).unwrap();
        let bounds = mesh.streams.bounds.to_native();
        let materials: Vec<_> = mesh.materials.iter().map(|slot| format!("{:?}", slot.material)).collect();
        let corner = |index: usize| {
            Vec3::from(mesh.streams.position(mesh.indices[index].to_native() as usize).expect("in range"))
        };
        let facing = (corner(1) - corner(0)).cross(corner(2) - corner(0)).normalize();
        ((bounds.min, bounds.max), materials, facing)
    };
    let (bounds, materials, facing) = read(shape_id(&data, 0));
    assert_eq!(bounds, ([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]), "the prefab's own space, wherever it's placed");
    assert_eq!(materials, [format!("Asset({WOOD:?})")]);
    let (mirrored, materials, mirrored_facing) = read(shape_id(&data, 3));
    assert_eq!(mirrored, ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), "mirrored, at its size of one");
    assert_eq!(materials, [format!("Asset({STONE:?})")], "with the placement's swap");
    assert_eq!(facing, mirrored_facing, "its faces outward, as a mirrored cell's are");

    // Moving a placement moves no shape.
    write_zone(&tree, &data.replace("(20.0, 0.0, 4.0)", "(21.0, 0.0, 4.0)"));
    let moved = tree.cook_for_editor();
    for id in &shapes {
        assert_eq!(by_id(&moved, *id).status, CookStatus::UpToDate);
    }

    // Nothing names them, and they ship with their zone, and only there.
    let zone = read_zone(&moved);
    assert!(!AssetKind::Zone.reach(&zone).unwrap().contains(&shape_id(&data, 0)));
    let packed = pack_cooked(&moved, &tree.dir.path().join("packs"), None).unwrap();
    for id in &shapes {
        let holding: Vec<&str> = packed
            .packs
            .iter()
            .filter(|pack| {
                let bytes = fs::read(&pack.path).unwrap();
                PackReader::new(&bytes).unwrap().toc().entries().iter().any(|entry| entry.id == *id)
            })
            .map(|pack| pack.name.as_str())
            .collect();
        assert_eq!(holding, ["zone_town"]);
    }
}

#[test]
fn a_zone_with_a_split_placement_has_no_shapes() {
    let tree = tree("zone-split-shapes");
    add_town(&tree);
    let data = format!(
        r##"placements: [
            (prefab: "{CRATE}", at: (2.0, 0.0, 2.0)),
            (prefab: "{TOWN}", split: Some((backdrop: ["#quad"]))),
        ],"##
    );
    write_zone(&tree, &data);
    let report = tree.cook_for_editor();
    assert!(report.assets.iter().all(|asset| asset.id != shape_id(&data, 0)));
    assert!(report.assets.iter().all(|asset| !asset.output.to_string_lossy().contains(".prefab.")));
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
        (format!(r##"encounters: ["{WOOD}"],"##), "`encounters[0]`"),
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
        let options = CookOptions { cook: Cook::Desktop, cache: Some(&cache), locked: false, editor: false };
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

fn navigation_id() -> AssetId {
    ZONE.sub_asset(zone::NAVIGATION_PART)
}

/// Ground 64 m a side, a table in the tile at (0, 0), and a slab up high that nothing leads to.
const NAV_COLLIDERS: &str = r##"(type: "sapphire::colliders::ColliderDocument", version: 1, data: (colliders: [
    (name: "ground", shape: Box(half_extents: (32.0, 0.5, 32.0)), position: (0.0, -0.5, 0.0)),
    (name: "table", shape: Box(half_extents: (1.0, 0.5, 1.0)), position: (8.0, 0.5, 8.0)),
    (name: "slab", shape: Box(half_extents: (3.0, 0.5, 3.0)), position: (-20.0, 6.0, -20.0)),
]))"##;

/// A document's text for that ground, with this added.
fn nav_zone(data: &str) -> String {
    format!(
        r##"colliders: Some("{COLLIDERS}"),
        navigation: Some((min: (-32.0, -32.0), max: (32.0, 32.0))),
        {data}"##
    )
}

const NAV_SPAWNERS: &str = r##"spawners: [(name: "start", position: (1.0, 0.0, 1.0), spawns: "player")],"##;

/// A tree with colliders, and `data` as its zone.
fn nav_tree(name: &str, data: &str) -> (Tree, PathBuf) {
    let tree = tree(name);
    let colliders = tree.write("arena.colliders.ron", NAV_COLLIDERS.as_bytes());
    tree.sidecar(&colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
    write_zone(&tree, data);
    (tree, colliders)
}

fn file_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() { file_count(&path) } else { 1 }
        })
        .sum()
}

const YARD: AssetId = AssetId::from_bytes([0x28; 16]);

/// An encounter's document with this for its `data`, and its sidecar.
fn write_encounter(tree: &Tree, data: &str) {
    let text = format!(r##"(type: "sapphire::zone::EncounterDocument", version: 1, data: ({data}))"##);
    let path = tree.write("yard.encounter.ron", text.as_bytes());
    tree.sidecar(&path, YARD, ImportSettings::Encounter(encounter::EncounterSettings::default()));
}

/// An encounter starting on `event`, whose one wave is `spawner`.
fn encounter_data(event: &str, spawner: &str) -> String {
    format!(
        r##"starts_on: "{event}", faction: 2, tokens: 1, waves: [(when: Start, spawners: ["{spawner}"])],"##
    )
}

/// What a zone needs for [`encounter_data`] of `in_the_yard` and `gate` to stand in it.
fn zone_with_a_yard() -> String {
    format!(
        r##"spawners: [(name: "gate", position: (1.0, 0.0, 1.0), yaw_degrees: 0.0, spawns: "warrior")],
            triggers: [(name: "yard", position: (0.0, 0.0, 0.0), volume: Sphere(radius: 2.0), event: "in_the_yard")],
            encounters: ["{YARD}"],"##
    )
}

/// An encounter a zone lists cooks, packs with that zone and no other, and reads back from there.
#[test]
fn a_zones_encounter_ships_with_it() {
    let tree = tree("zone-encounter");
    write_encounter(&tree, &encounter_data("in_the_yard", "gate"));
    write_zone(&tree, &zone_with_a_yard());
    let report = tree.cook();
    let cooked = by_id(&report, YARD);
    assert_eq!(cooked.output.extension().and_then(|extension| extension.to_str()), Some("sencounter"));
    let bytes = read_aligned(&cooked.output);
    let encounter = Encounter::read(&bytes).expect("reads");
    assert_eq!(encounter.starts_on.as_str(), "in_the_yard");

    let packs = tree.out().join("packs");
    let packed = pack_cooked(&report, &packs, None).unwrap();
    let holding: Vec<&str> = packed
        .packs
        .iter()
        .filter(|pack| {
            let bytes = fs::read(&pack.path).unwrap();
            let reader = PackReader::new(&bytes).unwrap();
            reader.toc().entries().iter().any(|entry| entry.id == YARD)
        })
        .map(|pack| pack.name.as_str())
        .collect();
    assert_eq!(holding, ["zone_town"]);
}

/// What an encounter names has to be in the zone that lists it, and a cook says which field of which.
#[test]
fn an_encounter_that_doesnt_fit_its_zone_fails_the_cook_naming_the_field() {
    let tree = tree("zone-encounter-bad");
    write_zone(&tree, &zone_with_a_yard());
    for (event, spawner, expected) in
        [("in_the_yard", "nowhere", "nowhere"), ("no_such_event", "gate", "no_such_event")]
    {
        write_encounter(&tree, &encounter_data(event, spawner));
        let text = chain(&tree.try_cook(false).expect_err(expected));
        assert!(text.contains("town.zone.ron"), "`{text}`");
        assert!(text.contains("`encounters[0]`"), "`{text}`");
        assert!(text.contains(expected), "`{text}` must say `{expected}`");
    }

    // Wrong on its own, it says so whatever lists it.
    for (data, expected) in [
        (r##"starts_on: "e", faction: 2, tokens: 99, waves: [(when: Start, spawners: ["a"])],"##, "tokens"),
        (r##"starts_on: "e", faction: 2, tokens: 1, waves: [], attackers: 2,"##, "attackers"),
    ] {
        write_encounter(&tree, data);
        let text = chain(&tree.try_cook(false).expect_err(expected));
        assert!(text.contains("yard.encounter.ron"), "`{text}`");
        assert!(text.contains(expected), "`{text}` must say `{expected}`");
    }
}

/// The ground, and a box where the zone draws a block at `x` (a 2 m cube about its `at`).
fn block_colliders(x: f32) -> String {
    format!(
        r##"(type: "sapphire::colliders::ColliderDocument", version: 1, data: (colliders: [
            (name: "ground", shape: Box(half_extents: (32.0, 0.5, 32.0)), position: (0.0, -0.5, 0.0)),
            (name: "block", shape: Box(half_extents: (1.0, 1.0, 1.0)), position: ({x:?}, 0.0, 5.0)),
        ]))"##
    )
}

/// A zone whose block at `x` names its collider, among the other placements.
fn claimed_block(x: f32, claims: &str) -> String {
    let placements = placements()
        .replace(r#"at: (5.0, 0.0, 5.0))"#, &format!(r#"at: ({x:?}, 0.0, 5.0), colliders: [{claims}])"#));
    format!(r##"colliders: Some("{COLLIDERS}"), {placements}"##)
}

/// A placement moved without the collider that stands for it, or a collider without its placement, is refused,
/// naming both (M4 X1): P0 moved a lantern post in the Lantern Ward, and the cell's mesh moved while its
/// collider stayed. Moved together, they cook, and a cook after only the colliders' document changed sees it.
#[test]
fn a_placement_and_the_collider_it_names_move_together_or_the_cook_refuses() {
    let tree = tree("zone-claims");
    let colliders = tree.write("zone.colliders.ron", block_colliders(5.0).as_bytes());
    tree.sidecar(&colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
    let zone = write_zone(&tree, &claimed_block(5.0, r#""block""#));
    tree.cook();

    // The placement moves and its collider doesn't: the box's sides have nothing drawn at them.
    tree.write_at(&zone, zone_text(&claimed_block(8.0, r#""block""#)).as_bytes());
    let text = chain(&tree.try_cook(false).expect_err("a moved placement"));
    for said in ["town.zone.ron", "`placements[1].colliders`", "collider `block`", "placements[1] draws"] {
        assert!(text.contains(said), "`{text}` must say {said}");
    }

    // Both move: it cooks.
    tree.write_at(&colliders, block_colliders(8.0).as_bytes());
    tree.cook();

    // Only the collider moves: the zone cooks again for it, and refuses.
    tree.write_at(&colliders, block_colliders(8.5).as_bytes());
    let text = chain(&tree.try_cook(false).expect_err("a moved collider"));
    assert!(text.contains("collider `block`") && text.contains("its +x side 0.500 m"), "`{text}`");
    tree.write_at(&colliders, block_colliders(8.0).as_bytes());
    tree.cook();

    // A collider the set doesn't have, by its field.
    tree.write_at(&zone, zone_text(&claimed_block(8.0, r#""block", "wall""#)).as_bytes());
    let text = chain(&tree.try_cook(false).expect_err("no such collider"));
    assert!(text.contains("`placements[1].colliders[1]`") && text.contains("`wall`"), "`{text}`");
}

#[test]
fn a_zones_navmesh_cooks_from_its_colliders_and_ships_with_it() {
    let data = format!("{} {NAV_SPAWNERS}", nav_zone(&placements()));
    let (tree, _) = nav_tree("zone-nav", &data);
    let report = tree.cook();

    let asset = by_id(&report, navigation_id());
    assert!(asset.output.ends_with("town.snav"), "{}", asset.output.display());
    let bytes = read_aligned(&asset.output);
    let mesh = NavMesh::read(&bytes).expect("the runtime reads it");
    assert!(mesh.polygon_count() > 0);
    // Rows count from -1000 m, in thirty-seconds, and a span's top is the cell over it. Only the ground
    // is left: nothing walks up to the table's top or the slab's.
    let rows: BTreeSet<i32> = (0..mesh.vertices.len()).map(|vertex| mesh.vertex(vertex)[1]).collect();
    assert_eq!(rows, [32_001].into(), "what's up there is pruned");

    let zone_bytes = read_zone(&report);
    let zone = Zone::read(&zone_bytes).unwrap();
    assert_eq!(zone.simulation.navigation.as_ref().copied(), Some(navigation_id()));
    let mut expected = vec![COLLIDERS, navigation_id()];
    expected.sort_unstable();
    assert_eq!(zone.references(), expected, "both load with the zone, and so are in its digest");
    assert_eq!(AssetKind::Zone.references(&zone_bytes).unwrap(), expected);
    assert_eq!(AssetKind::NavMesh.references(&bytes).unwrap(), []);
    let colliders = read_aligned(&by_id(&report, COLLIDERS).output);
    let digest_with = |navmesh: &[u8]| {
        let read = |id: AssetId| {
            (id == COLLIDERS).then_some(&colliders[..]).or((id == navigation_id()).then_some(navmesh))
        };
        zone.simulation_digest(read).unwrap()
    };
    assert_ne!(digest_with(&bytes), digest_with(b"another"), "a replay's digest reads the navmesh's bytes");

    let packs = tree.dir.path().join("packs");
    let packed = pack_cooked(&report, &packs, None).unwrap();
    let holding: Vec<&str> = packed
        .packs
        .iter()
        .filter(|pack| {
            let bytes = fs::read(&pack.path).unwrap();
            let reader = PackReader::new(&bytes).unwrap();
            reader.toc().entries().iter().any(|entry| entry.id == navigation_id())
        })
        .map(|pack| pack.name.as_str())
        .collect();
    assert_eq!(holding, ["zone_town"], "it ships with its zone, and only there");
}

/// The whole of what the cook's own unit is for: a collider's edit bakes what it reaches, and comes to
/// what baking everything would.
#[test]
fn moving_a_collider_rebakes_the_tiles_it_reaches_and_nothing_else() {
    let data = format!("{} {NAV_SPAWNERS}", nav_zone(&placements()));
    let (tree, colliders) = nav_tree("zone-nav-edit", &data);
    let cache = tree.dir.path().join("cache");
    tree.cook();
    let cold = file_count(&cache);
    assert_eq!(tree.cook().count(CookStatus::Cooked), 0, "a no-op bakes nothing");
    assert_eq!(file_count(&cache), cold);

    let moved = NAV_COLLIDERS.replace("(8.0, 0.5, 8.0)", "(8.0, 0.5, 9.0)");
    tree.write_at(&colliders, moved.as_bytes());
    let report = tree.cook();
    assert_eq!(by_id(&report, COLLIDERS).status, CookStatus::Cooked);
    assert_eq!(by_id(&report, navigation_id()).status, CookStatus::Cooked);
    assert_eq!(by_id(&report, ZONE).status, CookStatus::Cooked, "it holds the navmesh's key");
    assert_eq!(by_id(&report, cell_id(cell(0, 0))).status, CookStatus::UpToDate);
    // One tile, the `.scol`, the `.snav`, the `.szone` and what a cache keeps beside each.
    let added = file_count(&cache) - cold;
    assert!(added <= 8, "{added} new entries in the cache, for one tile's edit, of 25 tiles");

    let whole = nav_tree("zone-nav-whole", &data);
    whole.0.write_at(&whole.1, moved.as_bytes());
    let whole = whole.0.cook();
    assert_eq!(
        fs::read(by_id(&report, navigation_id()).output.clone()).unwrap(),
        fs::read(by_id(&whole, navigation_id()).output.clone()).unwrap(),
        "tiles from the cache make the bytes a whole bake makes",
    );
}

#[test]
fn what_doesnt_touch_a_navmesh_doesnt_bake_it() {
    let lamps = format!("{} {NAV_SPAWNERS}", nav_zone(&placements()));
    let (tree, _) = nav_tree("zone-nav-apart", &lamps);
    tree.cook();
    let unmoved = |report: &CookReport, what: &str| {
        assert_eq!(by_id(report, navigation_id()).status, CookStatus::UpToDate, "{what}");
    };

    // A saved kit piece, and the cells that place it.
    let piece = tree.content().join("kit/crate.glb");
    tree.write_at(&piece, &piece_glb(2.0));
    let report = tree.cook();
    assert_eq!(by_id(&report, cell_id(cell(0, 0))).status, CookStatus::Cooked);
    unmoved(&report, "a saved kit piece");

    // A placement, and a spawner's name and what it spawns, none of which it reads.
    let moved = lamps.replace("(20.0, 0.0, 4.0)", "(21.0, 0.0, 4.0)");
    write_zone(&tree, &moved);
    unmoved(&tree.cook(), "a moved placement");
    write_zone(&tree, &moved.replace(r#"name: "start""#, r#"name: "begin""#));
    let report = tree.cook();
    assert_eq!(by_id(&report, ZONE).status, CookStatus::Cooked);
    unmoved(&report, "a renamed spawner");

    // A spawner's place is what it seeds from, and a seed the same, though it finds the same ground.
    write_zone(&tree, &moved.replace("(1.0, 0.0, 1.0)", "(1.0, 0.0, 2.0)"));
    let report = tree.cook();
    assert_eq!(by_id(&report, navigation_id()).status, CookStatus::Cooked, "a moved spawner");
}

#[test]
fn a_seed_keeps_ground_no_spawner_reaches() {
    let polygons = |seeds: &str| {
        let data =
            nav_zone(NAV_SPAWNERS).replace("max: (32.0, 32.0))", &format!("max: (32.0, 32.0), {seeds})"));
        let (tree, _) = nav_tree("zone-nav-seeds", &data);
        let bytes = read_aligned(&by_id(&tree.cook(), navigation_id()).output);
        NavMesh::read(&bytes).unwrap().polygon_count()
    };
    assert!(polygons("seeds: [(-20.0, 6.5, -20.0)]") > polygons("seeds: []"));
}

/// A checkpoint seeds the navmesh as a spawner does, so the ground the player comes back to is ground
/// enemies can walk, and moving one rebakes it. Its name and volume don't.
#[test]
fn a_checkpoint_keeps_the_ground_it_stands_on() {
    let checkpoint = |at: &str, name: &str, radius: &str| {
        format!(r##"checkpoints: [(name: "{name}", position: {at}, volume: Sphere(radius: {radius}))],"##)
    };
    let (tree, _) = nav_tree("zone-nav-checkpoint", &nav_zone(NAV_SPAWNERS));
    let polygons = |report: &CookReport| {
        NavMesh::read(&read_aligned(&by_id(report, navigation_id()).output)).unwrap().polygon_count()
    };
    let without = polygons(&tree.cook());
    let on_the_slab =
        format!("{} {}", nav_zone(NAV_SPAWNERS), checkpoint("(-20.0, 6.5, -20.0)", "slab", "1.0"));
    write_zone(&tree, &on_the_slab);
    let report = tree.cook();
    assert_eq!(by_id(&report, navigation_id()).status, CookStatus::Cooked);
    assert!(polygons(&report) > without, "the slab it stands on is kept");
    write_zone(&tree, &on_the_slab.replace(r#""slab""#, r#""top""#).replace("radius: 1.0", "radius: 2.0"));
    let report = tree.cook();
    assert_eq!(by_id(&report, ZONE).status, CookStatus::Cooked);
    assert_eq!(by_id(&report, navigation_id()).status, CookStatus::UpToDate, "a renamed, wider one");
}

#[test]
fn what_a_zone_gets_wrong_about_navigation_is_named() {
    let (tree, _) = nav_tree("zone-nav-errors", "");
    let off = r##"spawners: [
        (name: "start", position: (1.0, 0.0, 1.0), spawns: "player"),
        (name: "stuck", position: (8.0, 0.0, 8.0), spawns: "warrior"),
    ],"##;
    let bounds = "min: (-32.0, -32.0), max: (32.0, 32.0)";
    let area = |min: &str, max: &str| {
        format!(
            r##"colliders: Some("{COLLIDERS}"), navigation: Some((min: {min}, max: {max})), {NAV_SPAWNERS}"##
        )
    };
    let cases: Vec<(String, &str)> = vec![
        (nav_zone(off), "`spawners[1]`"),
        (
            nav_zone(&format!(
                r##"{NAV_SPAWNERS} checkpoints: [(name: "stuck", position: (8.0, 0.0, 8.0), volume: Sphere(radius: 1.0))],"##
            )),
            "`checkpoints[0] (stuck)`",
        ),
        (
            nav_zone(NAV_SPAWNERS).replace(bounds, &format!("{bounds}, seeds: [(500.0, 0.0, 0.0)]")),
            "`navigation.seeds[0]`",
        ),
        (nav_zone(""), "needs a spawner"),
        (format!("navigation: Some(({bounds})), {NAV_SPAWNERS}"), "names none"),
        (area("(4.0, 4.0)", "(4.0, 8.0)"), "`navigation.min`"),
        (area("(-5000.0, -5000.0)", "(5000.0, 5000.0)"), "more than the 16384"),
        (area("(1000.0, 1000.0)", "(1032.0, 1032.0)"), "no walkable ground"),
    ];
    for (data, expected) in cases {
        write_zone(&tree, &data);
        let error = tree.try_cook(false).expect_err(&data);
        let text = chain(&error);
        assert!(text.contains("town.zone.ron"), "{data}: the zone's named in `{text}`");
        assert!(text.contains(expected), "{data}: `{text}` must say `{expected}`");
    }
}

/// Not a test of anything but what one edit costs: cold, a no-op and one collider moved, through a whole
/// cook for the arena as it is, and through the bake for a generated level a good deal larger, each to
/// read off its printout. As the cooker's built:
/// `cargo test -p sapphire_pipeline --profile tools --lib nav_edit -- --ignored --nocapture`.
#[test]
#[ignore = "measures a navmesh's cook, for a person to read"]
fn nav_edit_costs() {
    let measure = |name: &str, document: ColliderDocument, half: f32, spawner: [f32; 3], moved: usize| {
        let tree = Tree::new(&format!("nav-edit-{name}"));
        let write = |document: &ColliderDocument| sapphire_serialize::to_ron(document).unwrap();
        let colliders = tree.write("place.colliders.ron", write(&document).as_bytes());
        tree.sidecar(&colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
        let [x, y, z] = spawner;
        let data = format!(
            r##"colliders: Some("{COLLIDERS}"),
            navigation: Some((min: ({low:?}, {low:?}), max: ({half:?}, {half:?}))),
            spawners: [(name: "start", position: ({x:?}, {y:?}, {z:?}), spawns: "player")],"##,
            low = -half,
        );
        write_zone(&tree, &data);
        let (report, cold) = timed(|| tree.cook());
        let bytes = read_aligned(&by_id(&report, navigation_id()).output);
        let mesh = NavMesh::read(&bytes).unwrap();
        let mut stdout = io::stderr();
        writeln!(
            stdout,
            "NAV-EDIT {name}: {} colliders, {:.0} m a side, {} polygons, {} B; cold cook {cold:.0} ms; {}",
            document.colliders.len(),
            half * 2.0,
            mesh.polygon_count(),
            bytes.len(),
            cooked_counts(&report),
        )
        .unwrap();
        for round in 0..3 {
            let (report, time) = timed(|| tree.cook());
            writeln!(stdout, "NAV-EDIT {name}: no-op {round}: {time:.0} ms; {}", cooked_counts(&report))
                .unwrap();
        }
        let mut document = document;
        for round in 0..5 {
            document.colliders[moved].position[0] += 0.25;
            tree.write_at(&colliders, write(&document).as_bytes());
            let (report, time) = timed(|| tree.cook());
            writeln!(
                stdout,
                "NAV-EDIT {name}: one collider moved {round}: {time:.0} ms; {}",
                cooked_counts(&report)
            )
            .unwrap();
        }
    };

    let arena = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/arena/square.colliders.ron"),
    )
    .unwrap();
    let arena = ColliderDocument::from_ron(&arena).unwrap();
    let last = arena.colliders.len() - 1;
    measure("arena", arena, 64.0, [0.0, 0.0, 0.0], last);

    // A block of 32 boxes in each 32 m of a square 512 m a side: a large level of the size this bakes.
    let solid = |name: String, half_extents: [f32; 3], position: [f32; 3]| ColliderEntry {
        name,
        shape: ColliderShape::Box { half_extents },
        position,
        rotation: Rotation::None,
        groups: vec![Group::World],
    };
    let mut colliders = vec![solid("ground".to_owned(), [256.0, 0.5, 256.0], [0.0, -0.5, 0.0])];
    for block_x in -8..8 {
        for block_z in -8..8 {
            for i in 0..32 {
                let at = |block: i32, step: i32| block as f32 * 32.0 + 2.0 + ((i * step) % 28) as f32;
                let (x, z) = (at(block_x, 5), at(block_z, 11));
                let height = 0.5 + (i % 5) as f32;
                let name = format!("box {block_x} {block_z} {i}");
                colliders.push(solid(name, [0.75, height, 0.75], [x, height, z]));
            }
        }
    }
    let moved = colliders.len() / 2;
    let mut document = ColliderDocument { colliders };

    // The bake alone. Its document, cooked whole, is another matter, as this says: `ron`'s parse is
    // quadratic in its numbers (`zone::parse_document`), which a collider document doesn't work around.
    let text = sapphire_serialize::to_ron(&document).unwrap();
    let (_, parse) = timed(|| ColliderDocument::from_ron(&text).unwrap());
    let mut stderr = io::stderr();
    writeln!(stderr, "NAV-EDIT large: parsing its document of {} KiB takes {parse:.0} ms", text.len() / 1024)
        .unwrap();
    let dir = crate::test_util::TempDir::new("nav-edit-large");
    let cache = Cache::new(dir.path());
    let settings = nav::BakeSettings::CHARACTER;
    let seeds = [nav::Seed { field: "spawners[0]".to_owned(), position: [1.0, 0.0, 1.0] }];
    let bake = |document: &ColliderDocument| {
        let cooked = document.cook().unwrap().to_cooked().unwrap();
        timed(|| {
            nav::bake_colliders(&cooked, ([-256.0; 2], [256.0; 2]), &seeds, &settings, Some(&cache)).unwrap()
        })
    };
    let ((baked, islands), cold) = bake(&document);
    writeln!(
        stderr,
        "NAV-EDIT large: {} colliders, 512 m a side, {} polygons, {} B; cold bake {cold:.0} ms; {:?}, {islands:?}",
        document.colliders.len(),
        baked.mesh.polygon_count(),
        baked.mesh.to_cooked().unwrap().len(),
        baked.stats,
    )
    .unwrap();
    let ((baked, _), warm) = bake(&document);
    writeln!(
        stderr,
        "NAV-EDIT large: nothing touched: {warm:.0} ms; {} of {} tiles cached",
        baked.stats.tiles_cached, baked.stats.tiles_read
    )
    .unwrap();
    for round in 0..5 {
        document.colliders[moved].position[0] += 0.25;
        let ((baked, _), time) = bake(&document);
        writeln!(
            stderr,
            "NAV-EDIT large: one collider moved {round}: {time:.0} ms; {} of {} tiles cached",
            baked.stats.tiles_cached, baked.stats.tiles_read
        )
        .unwrap();
    }
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

const MARKET: AssetId = AssetId::from_bytes([0x2a; 16]);

/// A second zone in `tree`, called `market`, with a spawner and these exits.
fn write_market(tree: &Tree, exits: &str) -> PathBuf {
    let data = format!(
        r##"spawners: [(name: "from_town", position: (0.0, 0.0, 3.0), yaw_degrees: 0.0, spawns: "player")],
            placements: [(prefab: "{BLOCK}", at: (2.0, 0.0, 2.0))],
            exits: [{exits}],"##
    );
    let path = tree.write("market.zone.ron", zone_text(&data).as_bytes());
    tree.sidecar(&path, MARKET, ImportSettings::Zone(zone::ZoneSettings::default()));
    path
}

/// Two zones can lead to each other, and neither reads the other's output, nor ships a piece of it.
#[test]
fn two_zones_lead_to_each_other_and_stay_apart() {
    let tree = tree("zone-exits");
    let spawner = r#"spawners: [(name: "from_market", position: (0.0, 0.0, 0.0), yaw_degrees: 0.0, spawns: "player")],"#;
    let exits = format!(
        r#"{spawner} exits: [(name: "to_market", to: "{MARKET}", arrive: "from_town", prefetch: true)],"#
    );
    write_zone(&tree, &format!("{} {exits}", placements()));
    write_market(&tree, &format!(r#"(name: "to_town", to: "{ZONE}", arrive: "from_market")"#));

    let report = tree.cook();
    let town = read_zone(&report);
    let town = Zone::read(&town).unwrap();
    let exit = town.simulation.exits.first().expect("one");
    assert_eq!(exit.pack.as_str(), "zone_market");
    assert_eq!(exit.to, MARKET);
    assert!(exit.prefetch);
    assert!(!town.references().contains(&MARKET));
    assert!(!town.presentation_references().contains(&MARKET));
    let market = read_aligned(&by_id(&report, MARKET).output);
    assert_eq!(
        Zone::read(&market).unwrap().simulation.exits.first().expect("one").pack.as_str(),
        "zone_town"
    );
}

#[test]
fn an_exit_must_lead_to_a_zone_and_a_spawner_in_it() {
    let tree = tree("zone-exit-refused");
    let exit = |to: AssetId, arrive: &str| {
        format!(r#"{} exits: [(name: "out", to: "{to}", arrive: "{arrive}")],"#, placements())
    };
    write_zone(&tree, &exit(WOOD, "from_town"));
    write_market(&tree, "");
    let error = tree.try_cook(false).expect_err("not a zone");
    let text = chain(&error);
    assert!(text.contains("exits `out`.to"), "{text}");

    write_zone(&tree, &exit(MARKET, "nowhere"));
    let error = tree.try_cook(false).expect_err("no such spawner");
    let text = chain(&error);
    assert!(text.contains("exits `out`.arrive") && text.contains("nowhere"), "{text}");
}

/// The pack's name comes from the other zone's stem, so renaming it recooks this one.
#[test]
fn the_other_zones_pack_is_in_the_key() {
    let tree = tree("zone-exit-key");
    write_zone(
        &tree,
        &format!(r#"{} exits: [(name: "out", to: "{MARKET}", arrive: "from_town")],"#, placements()),
    );
    let market = write_market(&tree, "");
    assert_eq!(by_id(&tree.cook(), ZONE).status, CookStatus::Cooked);
    assert_eq!(by_id(&tree.cook(), ZONE).status, CookStatus::UpToDate);

    let renamed = market.with_file_name("bazaar.zone.ron");
    fs::rename(&market, &renamed).unwrap();
    fs::rename(sidecar_path(&market), sidecar_path(&renamed)).unwrap();
    let report = tree.cook();
    assert_eq!(by_id(&report, ZONE).status, CookStatus::Cooked);
    let town = read_zone(&report);
    let town = Zone::read(&town).unwrap();
    assert_eq!(town.simulation.exits[0].pack.as_str(), "zone_bazaar");
}
