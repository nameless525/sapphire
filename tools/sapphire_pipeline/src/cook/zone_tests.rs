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
use sapphire_tod::TodTimeline;
use sapphire_zone::{CellCoord, WaterSite, WaterSiteKind, ZoneDocument};
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
const GROUND: AssetId = AssetId::from_bytes([0x2f; 16]);

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

/// A weather document of the states `names`, each naming nothing but itself.
fn weather_states(names: &[&str]) -> String {
    let states: Vec<String> = names.iter().map(|name| format!(r#"(name: "{name}")"#)).collect();
    format!(
        r#"(type: "sapphire::world::WeatherDocument", version: 1, data: (states: [{}]))"#,
        states.join(", ")
    )
}

/// A zone's weather plan (M6 W8a) cooks its states' document's text, held to the names the plan gives, and
/// saving the document recooks the zone alone.
#[test]
fn a_weather_plan_cooks_with_its_states_and_recooks_on_a_save() {
    let tree = tree("zone-weather");
    tree.write("weather.ron", weather_states(&["clear", "rain"]).as_bytes());
    let plan = r##"weather_plan: Some((states: "weather.ron", seed: 3, schedule: [(from: 6.0, states: [(state: "clear"), (state: "rain", weight: 2)])])),"##;
    write_zone(&tree, &format!("{} {plan}", placements()));
    let report = tree.cook();
    let bytes = read_zone(&report);
    let read = Zone::read(&bytes).unwrap();
    let cooked = read.presentation.weather_plan.as_ref().expect("the plan cooks");
    assert_eq!(cooked.states.as_str(), weather_states(&["clear", "rain"]));
    assert_eq!(cooked.seed.to_native(), 3);

    tree.write("weather.ron", weather_states(&["clear", "rain", "snow"]).as_bytes());
    let report = tree.cook();
    assert_eq!(cooked_places(&report), [None], "only the zone recooks");
    let bytes = read_zone(&report);
    assert!(
        Zone::read(&bytes)
            .unwrap()
            .presentation
            .weather_plan
            .as_ref()
            .unwrap()
            .states
            .as_str()
            .contains("snow")
    );
}

/// A partial day keying only exposure, at `stops`.
fn lift(stops: f32) -> String {
    format!(
        r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [(time: 0.0, exposure: {stops:?})]))"#
    )
}

/// A zone's atmosphere volumes (M6 V1) cook each look they name once, its text in the zone, by ascending ID, and
/// saving a look recooks the zone alone.
#[test]
fn atmosphere_volumes_cook_with_their_looks_and_recook_on_a_save() {
    let tree = tree("zone-atmosphere");
    tree.write("looks/cave_day.ron", lift(1.0).as_bytes());
    tree.write("inn_day.ron", lift(0.5).as_bytes());
    let volumes = r##"volumes: [
        (name: "grotto", position: (4.0, 1.0, -6.0), shape: Ellipsoid(radii: (6.0, 3.0, 4.0), yaw: 30.0), look: "looks/cave_day.ron", blend: 4.0),
        (name: "inn", position: (-4.0, 1.5, 2.0), shape: Box(half_extents: (3.0, 1.5, 2.0)), look: "inn_day.ron", blend: 1.0, priority: 1),
        (name: "cellar", position: (0.0, -2.0, 0.0), shape: Sphere(radius: 2.0), look: "looks/cave_day.ron", blend: 0.5, strength: 0.6),
    ],"##;
    write_zone(&tree, &format!("{} {volumes}", placements()));
    let report = tree.cook();
    let bytes = read_zone(&report);
    let read = Zone::read(&bytes).unwrap();
    let atmosphere = read.presentation.atmosphere.to_native();
    assert_eq!(atmosphere.looks, [lift(1.0), lift(0.5)], "each look once, in the order first named");
    assert_eq!(atmosphere.volumes.len(), 3);
    assert!(atmosphere.volumes.windows(2).all(|pair| pair[0].id < pair[1].id), "by ascending ID");
    let cellar = atmosphere.volumes.iter().find(|volume| volume.name == "cellar").unwrap();
    assert_eq!((cellar.look, cellar.strength, cellar.blend), (0, 0.6, 0.5));

    tree.write("looks/cave_day.ron", lift(2.0).as_bytes());
    let report = tree.cook();
    assert_eq!(cooked_places(&report), [None], "only the zone recooks");
    let bytes = read_zone(&report);
    assert_eq!(Zone::read(&bytes).unwrap().presentation.atmosphere.looks[0].as_str(), lift(2.0));
}

/// Sapphire's biome presets (M6 V1) each read as a day, as a volume's look is read (so key no `grade`), key some
/// groups and not every group a stage's day must, and are tinted: every fog layer's, ambient, shadow and
/// inscattered colour they key has a hue, its channels at least an eighth of its brightest apart, so no near-white
/// or near-grey passes.
#[test]
fn the_biome_presets_are_partial_days_in_colour() {
    let biomes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/looks/biomes");
    let mut names: Vec<String> = fs::read_dir(&biomes)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        // Each preset's sidecar (`.ron.meta`, committed since T4's fix) sits beside it.
        .filter(|name| !name.ends_with(".meta"))
        .collect();
    names.sort();
    assert_eq!(names, ["cave_day.ron", "forest_haze_day.ron", "interior_warm_day.ron"]);
    let hued = |color: [f32; 3]| {
        let (low, high) =
            color.iter().fold((f32::MAX, f32::MIN), |(low, high), &v| (low.min(v), high.max(v)));
        high - low >= high / 8.0 && high - low > 0.02
    };
    for name in names {
        let bytes = fs::read(biomes.join(&name)).unwrap();
        let text = zone::look_text(&name, bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        let look = TodTimeline::from_ron(&text).unwrap();
        assert!(
            !look.is_complete(),
            "{name}: a preset is a partial day, leaving the sky and the sun to the day"
        );
        for key in look.keys() {
            let mut colors = Vec::new();
            if let Some(fog) = key.fog {
                colors.extend(
                    [fog.near, fog.far].iter().filter(|layer| layer.density > 0.0).map(|layer| layer.color),
                );
                colors.extend(Some(fog.sun_inscatter).filter(|color| color.iter().any(|&v| v > 0.0)));
            }
            if let Some(ambient) = key.ambient {
                colors.extend([ambient.sky, ambient.ground]);
            }
            colors.extend(key.shadow_tint);
            assert!(!colors.is_empty(), "{name}: it keys its air");
            for color in colors {
                assert!(hued(color), "{name} at {}: {color:?} is grey", key.time);
            }
        }
    }
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
    tree.write("weather.ron", weather_states(&["clear"]).as_bytes());
    let plan = |states: &str, state: &str| {
        format!(
            r##"weather_plan: Some((states: "{states}", schedule: [(from: 0.0, states: [(state: "{state}")])])),"##
        )
    };
    tree.write("day.ron", lift(1.0).as_bytes());
    let graded = r#"(type: "sapphire::world::TodTimeline", version: 1, data: (keys: [
        (time: 0.0, grade: (saturation: 1.2)),
    ]))"#;
    tree.write("graded_day.ron", graded.as_bytes());
    let volume = |more: &str| {
        format!(
            r##"volumes: [(name: "grotto", position: (0.0, 1.0, 0.0), shape: Sphere(radius: 2.0), {more})],"##
        )
    };
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
        (
            r##"reflections: [(name: "a", position: (0.0, 9.0, 0.0), parallax: (min: (-1.0, 0.0, -1.0), max: (1.0, 2.0, 1.0)))],"##
                .to_owned(),
            "`reflections[0].parallax`",
        ),
        (r##"day: Some("missing.ron"),"##.to_owned(), "failed to read"),
        (r##"day: Some("not_a_day.ron"),"##.to_owned(), "`day`"),
        (r##"rigs: Some("not_rigs.ron"),"##.to_owned(), "not a `sapphire::world::LightRigLibrary`"),
        (plan("not_rigs.ron", "clear"), "`weather_plan.states`: it doesn't read"),
        (volume(r#"look: "missing_day.ron", blend: 1.0"#), "`volumes[0].look`: \"missing_day.ron\" can't be read"),
        (volume(r#"look: "not_a_day.ron", blend: 1.0"#), "`volumes[0].look`: it doesn't read as a day"),
        (volume(r#"look: "graded_day.ron", blend: 1.0"#), "`volumes[0].look`: its key at 0 keys `grade`"),
        (volume(r#"look: "day.ron", blend: -1.0"#), "`volumes[0].blend`"),
        (volume(r#"look: "day.ron", blend: 1.0"#).replace("radius: 2.0", "radius: 0.0"), "`volumes[0].shape`"),
        (plan("weather.ron", "rain"), "names `rain`, which isn't one of its states"),
        // Two hours a hundredth apart that the runtime's schedule caps to one: refused at the cook, as a
        // trip would refuse them.
        (
            r##"weather_plan: Some((states: "weather.ron", schedule: [(from: 23.99, states: [(state: "clear")]), (from: 23.996, states: [(state: "clear")])])),"##
                .to_owned(),
            "`weather_plan.schedule",
        ),
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

/// The editor's move of a placement (M6 T2) takes the colliders it names with it, as one step of the zone and
/// its colliders' documents, so what it saves cooks where moving the zone alone is refused; undone, both files
/// are their bytes again, and cook again.
#[test]
fn a_placement_moved_by_the_editor_takes_its_collider_and_cooks() {
    use sapphire_document::{DocumentSet, Kind, edit_with_colliders};

    let tree = tree("zone-claims-editor");
    let colliders = tree.write("zone.colliders.ron", block_colliders(5.0).as_bytes());
    tree.sidecar(&colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
    let zone = write_zone(&tree, &claimed_block(5.0, r#""block""#));
    tree.cook();
    let (zone_bytes, colliders_bytes) = (fs::read(&zone).unwrap(), fs::read(&colliders).unwrap());

    let mut set = DocumentSet::new(vec![tree.content()]);
    let id = Kind::Zone.open(&mut set, &zone).expect("the zone opens");
    let path = "placements[1].at[0]".parse().expect("a path");
    edit_with_colliders(&mut set, id, "move a placement", |zone| zone.set_spelled("move", &path, "8.0"))
        .expect("the zone and its collider move");
    assert_eq!(set.stats().set_steps, 1, "one step of both documents");
    assert!(set.save_all().is_empty());
    assert_eq!(fs::read_to_string(&colliders).unwrap(), block_colliders(8.0), "only the block's x moved");
    tree.cook();

    assert_eq!(set.undo(id), Ok(sapphire_document::Took::Step));
    assert!(set.save_all().is_empty());
    assert_eq!((fs::read(&zone).unwrap(), fs::read(&colliders).unwrap()), (zone_bytes, colliders_bytes));
    tree.cook();
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

/// A cell's levels (M5 D6) are in its key, and in nothing the simulation reads: the `.szone`, its colliders
/// and its navmesh come out the same bytes, and the navmesh doesn't even bake again.
#[test]
fn asking_for_levels_recooks_the_cells_and_moves_nothing_the_simulation_reads() {
    // A ground under cell (0, 0), in 2 m quads, which simplifies; the crate and the block don't.
    let ground = r##"(piece: "#quad", scale: (16.0, 1.0, 16.0), slot: Some("stone"))"##;
    let placements = placements()
        .replace("placements: [", &format!(r##"placements: [(prefab: "{GROUND}", at: (8.0, 0.0, 8.0)),"##));
    let data = format!("{} {NAV_SPAWNERS}", nav_zone(&placements));
    let (tree, _) = nav_tree("zone-levels", &data);
    prefab(&tree, "ground", GROUND, &["stone"], ground, &[("stone", STONE)]);
    let before = tree.cook();
    let read = |report: &CookReport, id: AssetId| fs::read(&by_id(report, id).output).unwrap();
    let path = tree.content().join("town.zone.ron");
    let levels = |lods: optimize::LodSettings| {
        tree.sidecar(&path, ZONE, ImportSettings::Zone(zone::ZoneSettings { lods, ..Default::default() }));
    };

    levels(optimize::LodSettings::default());
    let after = tree.cook();
    for place in [cell(-1, -1), cell(0, 0), cell(1, 0), Place::Backdrop] {
        assert_eq!(by_id(&after, cell_id(place)).status, CookStatus::Cooked, "{place:?}");
    }
    let leveled = Mesh::read(&read_aligned(&by_id(&after, cell_id(cell(0, 0))).output)).unwrap().lods.len();
    assert!(leveled > 1, "the ground simplifies: {leveled} levels");
    assert_eq!(by_id(&after, navigation_id()).status, CookStatus::UpToDate, "it reads no mesh setting");
    assert_eq!(by_id(&after, COLLIDERS).status, CookStatus::UpToDate);
    for id in [ZONE, navigation_id(), COLLIDERS] {
        assert_eq!(read(&after, id), read(&before, id), "{id}: the same bytes, so the same digest");
    }
    assert_eq!(tree.cook().count(CookStatus::Cooked), 0, "the same settings again cook nothing");

    // Other levels move the cells' keys again, and none asked for is the cells' bytes before.
    levels(optimize::LodSettings::new(Vec::new(), 0).unwrap().lock_border(true));
    let none = tree.cook();
    for place in [cell(-1, -1), cell(0, 0), cell(1, 0), Place::Backdrop] {
        assert_eq!(read(&none, cell_id(place)), read(&before, cell_id(place)), "{place:?}");
    }
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

const GRID: AssetId = AssetId::from_bytes([0x2b; 16]);

/// A probe grid around the lamp, small and quick to bake, as `GRID`.
fn add_probe_grid(tree: &Tree) {
    add_lamp(tree);
    let document = probes::ProbeGridDocument {
        bounds: probes::BakeBounds { min: [-1.0, 0.0, -1.0], max: [1.0, 2.0, 1.0] },
        spacing: probes::PerCook { desktop: 1.0, mobile: 1.0 },
        visibility: probes::PerCook { desktop: probes::ShOrder::L1, mobile: probes::ShOrder::L1 },
        rays: probes::PerCook { desktop: 16, mobile: 16 },
        surfel_rays: 16,
        surfel_spacing: 1.0,
        bounces: 1,
        bounce_strength: 1.0,
        timeline: probes::BakeTimeline::DefaultDay,
        meshes: vec![probes::BakeMesh { mesh: LAMP, translation: [0.0; 3], yaw: 0.0 }],
    };
    let text = sapphire_serialize::to_ron(&document).unwrap();
    let path = tree.write("lamp.probes.ron", text.as_bytes());
    tree.sidecar(&path, GRID, ImportSettings::ProbeGrid(probes::ProbeGridSettings::default()));
}

/// A trip carries the grid of the zone it arrives in (M5 M6), so an exit may lead into or out of a zone with a
/// probe grid, as M4 Z3e's cook refused until then.
#[test]
fn an_exit_leads_into_and_out_of_a_zone_with_a_probe_grid() {
    let tree = tree("zone-exit-probes");
    add_probe_grid(&tree);
    let exit = format!(r#"exits: [(name: "out", to: "{MARKET}", arrive: "from_town")],"#);
    let grid = format!(r#"probes: Some("{GRID}"),"#);
    write_zone(&tree, &format!("{} {grid} {exit}", placements()));
    let market = write_market(&tree, "");
    let into = fs::read_to_string(&market).unwrap().replace("exits: [],", &format!("{grid} exits: [],"));
    fs::write(&market, into).unwrap();
    let report = tree.cook();
    let town = read_zone(&report);
    let town = Zone::read(&town).unwrap();
    assert_eq!(town.presentation.probes.as_ref().copied(), Some(GRID));
    assert_eq!(town.simulation.exits[0].to, MARKET);
}

const SKY: AssetId = AssetId::from_bytes([0x2c; 16]);

/// A zone's sky is a sky material's, which ships with the zone, and its air comes through as written. A
/// material of another family is refused by its field, as it would cook and then draw nothing in the sky.
#[test]
fn a_zone_names_its_sky_and_its_air() {
    let tree = tree("zone-sky");
    let sky = tree.write("materials/hollow_sky.material.ron", SKY_DOCUMENT.as_bytes());
    tree.sidecar(&sky, SKY, ImportSettings::Material(MaterialSettings::default()));
    let air = r#"wind: Some((direction: (0.0, 0.0, 1.0), strength: 2.0)), weather: Some((wetness: 0.8)),
        fog: Some((density: 0.5, height_base: Some(-2.0))),"#;
    write_zone(&tree, &format!(r#"{} sky: Some("{SKY}"), {air}"#, placements()));
    let report = tree.cook();
    let zone = read_zone(&report);
    let read = Zone::read(&zone).unwrap();
    assert_eq!(read.presentation.sky.as_ref().copied(), Some(SKY));
    assert_eq!(read.presentation.wind.as_ref().map(|wind| wind.to_native().strength), Some(2.0));
    assert_eq!(read.presentation.weather.as_ref().map(|weather| weather.to_native().puddles), Some(0.0));
    assert_eq!(read.presentation.fog.as_ref().and_then(|fog| fog.to_native().height_base), Some(-2.0));
    assert!(AssetKind::Zone.reach(&zone).unwrap().contains(&SKY));
    let packed = pack_cooked(&report, &tree.dir.path().join("packs"), None).unwrap();
    let holding: Vec<&str> = packed
        .packs
        .iter()
        .filter(|pack| {
            let bytes = fs::read(&pack.path).unwrap();
            PackReader::new(&bytes).unwrap().toc().entries().iter().any(|entry| entry.id == SKY)
        })
        .map(|pack| pack.name.as_str())
        .collect();
    assert_eq!(holding, ["zone_town"]);

    write_zone(&tree, &format!(r#"{} sky: Some("{WOOD}"),"#, placements()));
    let text = chain(&tree.try_cook(false).expect_err("a character material for a sky"));
    assert!(text.contains("town.zone.ron") && text.contains("sky") && text.contains("SkyMaterial"), "{text}");
}

/// A zone's water (M6 W2) cooks into both halves: its volumes where the simulation reads them, its surfaces'
/// material shipping with the zone. A second cook of the same tree, from nothing, writes the same bytes; a
/// material that isn't one is refused by its field.
#[test]
fn a_zones_water_cooks_into_both_halves_and_again_the_same() {
    let water = format!(
        r#"water: [
            (name: "pond", shape: Lake(outline: [(0.0, 0.0), (6.0, 0.0), (6.0, 4.0), (0.0, 4.0)], height: -0.25), material: Some("{WOOD}")),
            (name: "race", shape: River(points: [(at: (8.0, 0.5, 0.0), width: 2.0, speed: 1.5), (at: (8.0, 0.0, 12.0), width: 3.0)]), swim: false),
        ],"#
    );
    let cook = |name: &str| {
        let tree = tree(name);
        write_zone(&tree, &format!("{} {water}", placements()));
        let report = tree.cook();
        (read_zone(&report), report, tree)
    };
    let (first, _, tree) = cook("zone-water");
    let read = Zone::read(&first).unwrap();
    let names: Vec<&str> = read.simulation.water.iter().map(|body| body.name.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let volumes = sapphire_zone::WaterVolumes::new(&read.simulation, |_, _, _, _| None);
    let pond = volumes.at(3.0, -1.0, 2.0).expect("in the pond");
    assert_eq!((pond.surface_y, pond.swim), (-0.25, true));
    let race = volumes.at(8.0, 0.0, 6.0).expect("in the race");
    assert_eq!((race.surface_y, race.swim), (0.25, false));
    assert!(AssetKind::Zone.reach(&first).unwrap().contains(&WOOD), "its material ships with it");
    assert!(!AssetKind::Zone.references(&first).unwrap().contains(&WOOD), "and loads with its cells");
    let (second, ..) = cook("zone-water-again");
    assert_eq!(first.as_slice(), second.as_slice(), "a cook from nothing writes the same bytes");

    write_zone(&tree, &format!("{} {}", placements(), water.replace(&WOOD.to_string(), &CRATE.to_string())));
    let text = chain(&tree.try_cook(false).expect_err("a prefab for a material"));
    assert!(text.contains("town.zone.ron") && text.contains("water[0].material"), "{text}");
}

/// A sky material with nothing but its sun, as small as one reads.
const SKY_DOCUMENT: &str = r#"(
    type: "sapphire::render::SkyMaterial",
    version: 1,
    data: (sun: (size: 2.0, softness: 0.1)),
)"#;

/// One reflection probe in the yard by the lamp, its box the yard's.
const YARD_PROBE: &str = r#"reflections: [
    (name: "yard", position: (2.0, 1.5, 6.0), parallax: (min: (-4.0, 0.0, 0.0), max: (8.0, 4.0, 12.0))),
],"#;

fn reflections_id() -> AssetId {
    ZONE.sub_asset(zone::REFLECTIONS_PART)
}

/// A zone's reflection probes bake beside it (M5 L2): one asset of the zone's own, which its `.szone` names and
/// its pack holds, of the cook's size and its day's keys, keyed by what they see and nothing else.
#[test]
fn a_zones_reflection_probes_bake_beside_it_and_ship_with_it() {
    let tree = tree("zone-reflections");
    add_lamp(&tree);
    let lamp = |x: f32| format!(r#"instances: [(mesh: "{LAMP}", at: ({x}, 0.0, 6.0))],"#);
    write_zone(&tree, &format!("{} {} {YARD_PROBE}", placements(), lamp(4.0)));
    let report = tree.cook();
    let probes = by_id(&report, reflections_id());
    assert!(probes.output.ends_with("town.sreflect"), "{}", probes.output.display());
    let bytes = read_aligned(&probes.output);
    let read = ReflectionProbes::read(&bytes).unwrap();
    assert_eq!(read.size(), 128, "the desktop cook's");
    let default_day = probes::keys(&probes::BakeTimeline::DefaultDay, Path::new("")).unwrap();
    assert_eq!(
        read.key_hours().collect::<Vec<_>>(),
        default_day.iter().map(|key| key.hours).collect::<Vec<_>>()
    );
    assert_eq!(read.probes.len(), 1);
    assert_eq!(read.probes[0].name.as_str(), "yard");
    assert_eq!(read.probes[0].parallax_max.map(|v| v.to_native()), [8.0, 4.0, 12.0]);
    // The middle of +Y looks straight up, at the sky.
    let up = read.cube(0, 0).skip(2 * 128 * 128 * 4 + (64 * 128 + 64) * 4).nth(3).unwrap();
    assert_eq!(half::f16::from_bits(up).to_f32(), 1.0, "straight up is sky");

    let zone = read_zone(&report);
    assert_eq!(Zone::read(&zone).unwrap().presentation.reflections.as_ref().copied(), Some(reflections_id()));
    assert!(AssetKind::Zone.reach(&zone).unwrap().contains(&reflections_id()));
    let packed = pack_cooked(&report, &tree.dir.path().join("packs"), None).unwrap();
    let holding: Vec<&str> = packed
        .packs
        .iter()
        .filter(|pack| {
            let bytes = fs::read(&pack.path).unwrap();
            let reader = PackReader::new(&bytes).unwrap();
            reader.toc().entries().iter().any(|entry| entry.id == reflections_id())
        })
        .map(|pack| pack.name.as_str())
        .collect();
    assert_eq!(holding, ["zone_town"]);

    // A light is nothing the probes see; the lamp moved is.
    let status = |report: &CookReport| by_id(report, reflections_id()).status;
    let lit = r#"lights: [(position: (4.0, 3.0, 6.0), color: (1.0, 1.0, 1.0), intensity: 1.0, range: 4.0)],"#;
    write_zone(&tree, &format!("{} {} {YARD_PROBE} {lit}", placements(), lamp(4.0)));
    let report = tree.cook();
    assert_eq!((status(&report), by_id(&report, ZONE).status), (CookStatus::UpToDate, CookStatus::Cooked));
    write_zone(&tree, &format!("{} {} {YARD_PROBE} {lit}", placements(), lamp(5.0)));
    assert_eq!(status(&tree.cook()), CookStatus::Cooked);

    // The mobile cook bakes at 64, from which Mobile-Low reads the next level down.
    let options = CookOptions { cook: Cook::Mobile, cache: None, locked: false, editor: false };
    let mobile = cook_dir_on(&tree.content(), &tree.dir.path().join("mobile"), &options, 2).unwrap();
    let bytes = read_aligned(&by_id(&mobile, reflections_id()).output);
    assert_eq!(ReflectionProbes::read(&bytes).unwrap().size(), 64);

    // A zone without probes has no such asset, and names none.
    write_zone(&tree, &placements());
    let report = tree.cook();
    assert!(!report.assets.iter().any(|asset| asset.id == reflections_id()));
    assert!(Zone::read(&read_zone(&report)).unwrap().presentation.reflections.is_none());
}

fn ambient_id() -> AssetId {
    ZONE.sub_asset(sapphire_zone::AMBIENT_PART)
}

/// A graph this tree hasn't got, as the benchmark's district names the shipped content's.
const ELSEWHERE: AssetId = AssetId::from_bytes([0x2a; 16]);

/// A zone's ambient characters (M5 C10), `graph` playing on `mesh`, the one cast member.
fn ambient_section(graph: AssetId, mesh: AssetId, gesture: &str) -> String {
    format!(
        r#"ambient: Some((seed: 3, graph: "{graph}", cast: [(name: "lamplighter", mesh: "{mesh}")],
            posts: [(name: "corner", position: (2.0, 0.0, 3.0), yaw_degrees: 90.0, gestures: ["{gesture}"])],
            walks: [(name: "lap", points: [(0.0, 0.0, 0.0), (8.0, 0.0, 0.0), (8.0, 0.0, 8.0)], closed: true,
                     count: 2)])),"#
    )
}

/// A zone's ambient characters cook to an asset of the zone's own, which ships in its pack with the cast it
/// reaches, and leave the `.szone` the bytes it was without them: nothing the simulation reads moves.
#[test]
fn a_zones_ambient_characters_cook_beside_it_and_leave_it_as_it_was() {
    let tree = tree("zone-ambient");
    add_lamp(&tree);
    write_zone(&tree, &placements());
    let report = tree.cook();
    let without = fs::read(&by_id(&report, ZONE).output).unwrap();
    assert!(!report.assets.iter().any(|asset| asset.id == ambient_id()), "no section, no asset");

    write_zone(&tree, &format!("{} {}", placements(), ambient_section(ELSEWHERE, LAMP, "shift")));
    let report = tree.cook();
    assert_eq!(fs::read(&by_id(&report, ZONE).output).unwrap(), without, "the zone is the bytes it was");
    let cooked = by_id(&report, ambient_id());
    assert!(cooked.output.ends_with("town.sambient"), "{}", cooked.output.display());
    let bytes = read_aligned(&cooked.output);
    let ambient = Ambient::read(&bytes).unwrap();
    assert_eq!((ambient.seed.to_native(), ambient.posts.len(), ambient.walks.len()), (3, 1, 1));
    assert_eq!(ambient.cast[0].mesh, LAMP);
    let reach = AssetKind::Ambient.reach(&bytes).unwrap();
    assert!(reach.contains(&LAMP) && reach.contains(&ELSEWHERE), "{reach:?}");
    assert!(AssetKind::Ambient.references(&bytes).unwrap().is_empty(), "loading it loads nothing else");

    let packed = pack_cooked(&report, &tree.dir.path().join("packs"), None).unwrap();
    let holding = |id: AssetId| -> Vec<String> {
        packed
            .packs
            .iter()
            .filter(|pack| {
                let bytes = fs::read(&pack.path).unwrap();
                PackReader::new(&bytes).unwrap().toc().entries().iter().any(|entry| entry.id == id)
            })
            .map(|pack| pack.name.clone())
            .collect()
    };
    assert_eq!(holding(ambient_id()), ["zone_town"]);
    assert_eq!(holding(LAMP), ["zone_town"], "a cast only this zone has ships with it");
    assert!(holding(ELSEWHERE).is_empty(), "what another tree has stays out");
}

/// A cast member that isn't a mesh, a graph that isn't one, and a bad name fail the cook naming the zone and
/// the field.
#[test]
fn what_a_zones_ambient_characters_name_is_held_to_its_kind() {
    let tree = tree("zone-ambient-errors");
    add_lamp(&tree);
    let cases = [
        (ambient_section(ELSEWHERE, WOOD, "shift"), ["`ambient.cast[0].mesh`", "a mesh"]),
        (ambient_section(LAMP, LAMP, "shift"), ["`ambient.graph`", "an animation graph"]),
        (ambient_section(ELSEWHERE, LAMP, ""), ["ambient.posts[0].gestures[0]", ""]),
    ];
    for (data, said) in cases {
        write_zone(&tree, &data);
        let text = chain(&tree.try_cook(false).expect_err(&data));
        for said in ["town.zone.ron"].into_iter().chain(said) {
            assert!(text.contains(said), "`{text}` must say `{said}`");
        }
    }
}

const FOLIAGE: AssetId = AssetId::from_bytes([0x30; 16]);
/// The meshes the grove's kinds draw: an oak, a fern and a clump of grass.
const PLANTS: [AssetId; 3] =
    [AssetId::from_bytes([0x31; 16]), AssetId::from_bytes([0x32; 16]), AssetId::from_bytes([0x33; 16])];

/// The placement tests' grove (`sapphire_zone`'s), its kinds drawing [`PLANTS`].
fn grove_text() -> String {
    let grove = include_str!("../../../../crates/sapphire_zone/testdata/grove.foliage.ron");
    let ids = [
        "2b0f0d43-6d4e-4b59-9b1f-5f3c6f0b8e21",
        "8f5a7c1e-0c3d-4e1b-a2f6-3d9e4b7c5a10",
        "4d2c6b8a-1e3f-4a5b-9c7d-0e1f2a3b4c5d",
    ];
    ids.into_iter().zip(PLANTS).fold(grove.to_owned(), |text, (from, to)| text.replace(from, &to.to_string()))
}

/// A zone's text naming [`NAV_COLLIDERS`] and the grove, with `data`.
fn foliage_zone(data: &str) -> String {
    zone_text(&format!(r##"colliders: Some("{COLLIDERS}"), foliage: Some("{FOLIAGE}"), {data}"##))
}

/// A tree with [`NAV_COLLIDERS`]' ground, the grove and its plants' meshes, and [`foliage_zone`] of `data`: the
/// colliders' and the grove's paths.
fn foliage_tree(name: &str, data: &str) -> (Tree, PathBuf, PathBuf) {
    let tree = tree(name);
    let colliders = tree.write("arena.colliders.ron", NAV_COLLIDERS.as_bytes());
    tree.sidecar(&colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
    for (name, id) in ["oak", "fern", "clump"].into_iter().zip(PLANTS) {
        let mesh = tree.write(&format!("plants/{name}.glb"), &piece_glb(1.0));
        tree.sidecar(&mesh, id, ImportSettings::Gltf(MeshSettings::default()));
    }
    let foliage = tree.write("grove.foliage.ron", grove_text().as_bytes());
    tree.sidecar(&foliage, FOLIAGE, ImportSettings::Foliage(Default::default()));
    let path = tree.write("town.zone.ron", foliage_zone(data).as_bytes());
    tree.sidecar(&path, ZONE, ImportSettings::Zone(zone::ZoneSettings::default()));
    (tree, colliders, foliage)
}

fn scatter_id(x: i32, z: i32) -> AssetId {
    ZONE.sub_asset(&sapphire_zone::foliage::scatter_part(CellCoord { x, z }))
}

/// The grove's 16 m cells: its layers reach from -40 m to 40 m, cells -3 to 2 each way.
fn grove_cells() -> Vec<CellCoord> {
    (-3..=2).flat_map(|x| (-3..=2).map(move |z| CellCoord { x, z })).collect()
}

/// A zone's foliage cooks to its kinds and a set a cell (M6 G1a), each the bytes placement makes alone on the
/// zone's colliders; the `.szone` names them outside its digest, and they ship with the zone.
#[test]
fn a_zones_foliage_cooks_to_a_set_a_cell_that_ships_with_it() {
    let (tree, _, _) = foliage_tree("zone-foliage", &placements());
    let report = tree.cook();

    let kinds = read_aligned(&by_id(&report, FOLIAGE).output);
    assert!(by_id(&report, FOLIAGE).output.ends_with("grove.sfoliage"));
    assert_eq!(AssetKind::Foliage.references(&kinds).unwrap(), [], "loading it loads nothing else");
    let mut meshes = PLANTS.to_vec();
    meshes.sort_unstable();
    assert_eq!(AssetKind::Foliage.reach(&kinds).unwrap(), meshes, "but its meshes ship with it");

    let zone_bytes = read_zone(&report);
    let zone = Zone::read(&zone_bytes).unwrap();
    let foliage = zone.presentation.foliage.as_ref().expect("named");
    assert_eq!(foliage.foliage, FOLIAGE);
    let cells: Vec<CellCoord> = foliage
        .cells
        .iter()
        .map(|cell| CellCoord { x: cell.coord[0].to_native(), z: cell.coord[1].to_native() })
        .collect();
    assert_eq!(cells, grove_cells());
    assert_eq!(zone.references(), [COLLIDERS], "the simulation reads none of it");

    let document = sapphire_zone::FoliageDocument::from_ron(&grove_text()).unwrap();
    let colliders = read_aligned(&by_id(&report, COLLIDERS).output);
    let ground = zone::scatter::ColliderGround::from_cooked(&colliders).unwrap();
    let grid = sapphire_zone::grid::Grid { cell_size: 16.0 };
    let mut plants = 0;
    for &cell in &cells {
        assert_eq!(foliage.cells.iter().filter(|set| set.scatter == scatter_id(cell.x, cell.z)).count(), 1);
        let asset = by_id(&report, scatter_id(cell.x, cell.z));
        let name = format!("town.scatter.{}.{}.sscatter", cell.x, cell.z);
        assert!(asset.output.ends_with(&name), "{}", asset.output.display());
        let bytes = read_aligned(&asset.output);
        assert_eq!(AssetKind::Scatter.references(&bytes).unwrap(), []);
        let alone = zone::scatter::cook(&document, &ground, &Default::default(), grid, cell).unwrap();
        assert_eq!(&bytes[..], &alone[..], "the cook is placement on the zone's colliders, and nothing else");
        let set = ScatterSet::read(&bytes).unwrap();
        for index in 0..set.len() {
            let [x, y, z] = set.instance(index).position;
            assert!(x.abs() <= 32.0 && z.abs() <= 32.0, "only where there's ground: {x}, {z}");
            assert!([0.0, 1.0, 6.5].contains(&y), "on the ground, the table or the slab: {y}");
        }
        plants += set.len();
    }
    assert!(plants > 1_000, "{plants} plants");

    // The same zone with no foliage has the same digest: plants are drawn, never simulated.
    let bare = self::tree("zone-foliage-bare");
    let bare_colliders = bare.write("arena.colliders.ron", NAV_COLLIDERS.as_bytes());
    bare.sidecar(&bare_colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
    write_zone(&bare, &format!(r##"colliders: Some("{COLLIDERS}"), {}"##, placements()));
    let bare_bytes = read_zone(&bare.cook());
    let digest = |bytes: &[u8]| {
        Zone::read(bytes)
            .unwrap()
            .simulation_digest(|id| (id == COLLIDERS).then_some(&colliders[..]))
            .unwrap()
    };
    assert_eq!(digest(&zone_bytes), digest(&bare_bytes));

    let packs = tree.dir.path().join("packs");
    let packed = pack_cooked(&report, &packs, None).unwrap();
    let holding = |id: AssetId| -> Vec<String> {
        (packed.packs.iter())
            .filter(|pack| {
                let bytes = fs::read(&pack.path).unwrap();
                PackReader::new(&bytes).unwrap().toc().entries().iter().any(|entry| entry.id == id)
            })
            .map(|pack| pack.name.clone())
            .collect()
    };
    assert_eq!(holding(scatter_id(0, 0)), ["zone_town"], "a set ships with its zone, and only there");
    assert_eq!(holding(FOLIAGE), ["zone_town"], "and so do its kinds");
    assert_eq!(holding(PLANTS[0]), ["zone_town"], "and their meshes");
}

/// A set's key holds the foliage document and the colliders, not the rest of the zone.
#[test]
fn only_the_foliage_or_the_colliders_recook_a_zones_sets() {
    let (tree, colliders, foliage) = foliage_tree("zone-foliage-keys", &placements());
    tree.cook();
    let set = scatter_id(0, 0);

    let moved = foliage_zone(&placements().replace("(2.0, 0.0, 2.0)", "(3.0, 0.0, 2.0)"));
    tree.write("town.zone.ron", moved.as_bytes());
    let report = tree.cook();
    assert_eq!(by_id(&report, cell_id(cell(0, 0))).status, CookStatus::Cooked, "a moved placement");
    assert_eq!(by_id(&report, set).status, CookStatus::UpToDate, "plants stand on colliders, not on cells");
    assert_eq!(by_id(&report, FOLIAGE).status, CookStatus::UpToDate);

    tree.write_at(&foliage, grove_text().replace("seed: 3", "seed: 4").as_bytes());
    let report = tree.cook();
    assert_eq!(by_id(&report, FOLIAGE).status, CookStatus::Cooked);
    assert_eq!(by_id(&report, set).status, CookStatus::Cooked, "a saved foliage document");
    assert_eq!(by_id(&report, ZONE).status, CookStatus::UpToDate, "it names the same cells' sets");
    assert_eq!(by_id(&report, cell_id(cell(0, 0))).status, CookStatus::UpToDate);

    tree.write_at(&colliders, NAV_COLLIDERS.replace("(8.0, 0.5, 8.0)", "(8.0, 0.5, 9.0)").as_bytes());
    let report = tree.cook();
    assert_eq!(by_id(&report, set).status, CookStatus::Cooked, "a moved collider");

    // The `.szone` holds which cells have a set, not the document's bytes: a layer reaching a cell it didn't
    // cooks it again, naming that cell's set (M6 G1b).
    let wider = grove_text().replace("(40.0, -40.0), (40.0, 40.0)", "(56.0, -40.0), (56.0, 40.0)");
    assert_ne!(wider, grove_text(), "the wood's outline is written as this test expects");
    tree.write_at(&foliage, wider.as_bytes());
    let report = tree.cook();
    assert_eq!(by_id(&report, ZONE).status, CookStatus::Cooked, "a cell the foliage plans a set for");
    let zone_bytes = read_zone(&report);
    let zone = Zone::read(&zone_bytes).unwrap();
    let named = zone
        .presentation
        .foliage
        .as_ref()
        .expect("named")
        .cells
        .iter()
        .map(|cell| cell.scatter)
        .collect::<Vec<_>>();
    assert!(named.contains(&scatter_id(3, 0)), "and it names its set");
}

#[test]
fn what_a_zone_gets_wrong_about_foliage_is_named() {
    let refused = |name: &str, tree: &Tree| chain(&tree.try_cook(false).expect_err(name));

    // A foliage document's kind that draws something other than a mesh.
    let (tree, _, foliage) = foliage_tree("zone-foliage-not-a-mesh", &placements());
    tree.write_at(&foliage, grove_text().replace(&PLANTS[1].to_string(), &WOOD.to_string()).as_bytes());
    let text = refused("a material for a mesh", &tree);
    assert!(text.contains("kinds[1].model") && text.contains("static mesh"), "{text}");

    // A zone naming as its foliage something that isn't one.
    let (tree, _, _) = foliage_tree("zone-foliage-not-foliage", &placements());
    let wrong = foliage_zone(&placements()).replace(&FOLIAGE.to_string(), &WOOD.to_string());
    tree.write("town.zone.ron", wrong.as_bytes());
    let text = refused("a material for foliage", &tree);
    assert!(text.contains("`foliage`") && text.contains("isn't a foliage document"), "{text}");

    // A malformed foliage document, by its field.
    let (tree, _, foliage) = foliage_tree("zone-foliage-malformed", &placements());
    tree.write_at(&foliage, grove_text().replace("contrast: 0.6", "contrast: 1.6").as_bytes());
    let text = refused("a contrast past 1", &tree);
    assert!(text.contains("rules[0].cluster.contrast"), "{text}");
}

/// The grove with trunks on its oaks (M6 G1b).
fn trunked_grove() -> String {
    let oak = format!(r##"(name: "oak", model: Tree("{}"))"##, PLANTS[0]);
    let trunked =
        format!(r##"(name: "oak", model: Tree("{}"), trunk: Some((radius: 0.3, height: 4.0)))"##, PLANTS[0]);
    assert!(grove_text().contains(&oak), "the grove's oak is written as this test expects");
    grove_text().replace(&oak, &trunked)
}

fn trunks_id() -> AssetId {
    ZONE.sub_asset(zone::trunks::TRUNKS_PART)
}

/// A navmesh over [`NAV_COLLIDERS`]' ground, with the placements.
fn grove_navigation() -> String {
    format!(
        r##"navigation: Some((min: (-32.0, -32.0), max: (32.0, 32.0))), {NAV_SPAWNERS} {}"##,
        placements()
    )
}

/// The plants of `kind` a cooked scatter set holds, as their IDs and roots.
fn plants_of(bytes: &[u8], kind: u32) -> Vec<(u64, [f32; 3])> {
    let set = ScatterSet::read(bytes).unwrap();
    let mut plants = Vec::new();
    for run in set.runs.iter().filter(|run| run.kind.to_native() == kind) {
        let first = run.first_chunk.to_native() as usize;
        for chunk in &set.chunks[first..first + run.chunk_count.to_native() as usize] {
            let start = chunk.first.to_native() as usize;
            for index in start..start + chunk.count.to_native() as usize {
                plants.push((set.ids[index].to_native(), set.instance(index).position));
            }
        }
    }
    plants
}

/// Each of the grove's cells' plants of `kind`.
fn grove_plants(report: &CookReport, kind: u32) -> Vec<(u64, [f32; 3])> {
    (grove_cells().into_iter())
        .flat_map(|cell| plants_of(&read_aligned(&by_id(report, scatter_id(cell.x, cell.z)).output), kind))
        .collect()
}

/// The trunks a collider set holds, as their names, roots and radii.
fn trunks_of(set: &ColliderSet) -> Vec<(String, [f32; 3], f32)> {
    (set.colliders.iter())
        .filter(|collider| collider.name.starts_with("foliage "))
        .map(|collider| {
            let sapphire_colliders::CookedShape::Capsule { half_height, radius } = collider.shape else {
                panic!("a trunk is a capsule: {collider:?}");
            };
            let [x, y, z] = collider.position;
            (collider.name.clone(), [x, y - half_height, z], radius)
        })
        .collect()
}

/// A zone whose foliage has trunks names its own collider set, the document's and a capsule a tree, and moves
/// its digest; a body walking at a trunk stops there, where it walks on through the document's colliders; and
/// the navmesh is baked round it, where the same zone without trunks walks straight through.
#[test]
fn a_zones_trunks_join_its_colliders_stop_a_body_and_carve_its_navmesh() {
    let (tree, _, foliage) = foliage_tree("zone-trunks", &grove_navigation());
    let bare_report = tree.cook();
    let bare_zone = read_zone(&bare_report);
    assert_eq!(Zone::read(&bare_zone).unwrap().simulation.colliders.as_ref(), Some(&COLLIDERS), "no trunks");
    let authored = read_aligned(&by_id(&bare_report, COLLIDERS).output);
    let bare_navmesh = read_aligned(&by_id(&bare_report, navigation_id()).output);

    tree.write_at(&foliage, trunked_grove().as_bytes());
    let report = tree.cook();
    let joined_asset = by_id(&report, trunks_id());
    assert!(joined_asset.output.ends_with("town.trunks.scol"), "{}", joined_asset.output.display());
    let joined = read_aligned(&joined_asset.output);
    let zone_bytes = read_zone(&report);
    let zone = Zone::read(&zone_bytes).unwrap();
    assert_eq!(zone.simulation.colliders.as_ref(), Some(&trunks_id()), "the simulation reads the joined set");
    let references: BTreeSet<AssetId> = zone.references().into_iter().collect();
    assert_eq!(references, BTreeSet::from([trunks_id(), navigation_id()]), "and not the document's");
    let navmesh = read_aligned(&by_id(&report, navigation_id()).output);

    // The document's colliders, which its `.szone` no longer names, ship with the zone, not in the shared pack.
    let packed = pack_cooked(&report, &tree.dir.path().join("packs"), None).unwrap();
    let holding = |id: AssetId| -> Vec<String> {
        (packed.packs.iter())
            .filter(|pack| {
                let bytes = fs::read(&pack.path).unwrap();
                PackReader::new(&bytes).unwrap().toc().entries().iter().any(|entry| entry.id == id)
            })
            .map(|pack| pack.name.clone())
            .collect()
    };
    assert_eq!(holding(COLLIDERS), ["zone_town"], "the trunks are joined from them");
    assert_eq!(holding(trunks_id()), ["zone_town"]);

    // The document's colliders, each as it was, and a capsule for each oak, standing at its root.
    let set = ColliderSet::from_cooked(&joined).unwrap();
    let own = ColliderSet::from_cooked(&authored).unwrap();
    for collider in &own.colliders {
        assert!(set.colliders.contains(collider), "{} is kept", collider.name);
    }
    let trunks = trunks_of(&set);
    assert_eq!(set.colliders.len(), own.colliders.len() + trunks.len());
    let oaks = grove_plants(&report, 0);
    assert!(oaks.len() > 10, "{} oaks", oaks.len());
    assert_eq!(trunks.len(), oaks.len(), "a trunk an oak, and none for the ferns or the grass");
    for (id, root) in &oaks {
        let name = sapphire_zone::foliage::trunk_name(*id);
        let (_, at, _) = trunks.iter().find(|(held, _, _)| *held == name).expect("its trunk");
        assert!(at.iter().zip(root).all(|(a, b)| (a - b).abs() < 1e-4), "{at:?} for {root:?}");
    }

    // Only the zone with trunks moves its digest.
    let read = |id: AssetId| match id {
        id if id == COLLIDERS => Some(&authored[..]),
        id if id == trunks_id() => Some(&joined[..]),
        _ => None,
    };
    let bare_digest = Zone::read(&bare_zone).unwrap().digest_without_navigation(read).unwrap();
    assert_ne!(zone.digest_without_navigation(read).unwrap(), bare_digest);

    // A trunk on the open ground, clear of the table and of the ground's edge.
    let (name, [x, _, z], radius) = (trunks.iter())
        .find(|(_, [x, y, z], _)| {
            y.abs() < 1e-3
                && x.abs() < 24.0
                && z.abs() < 24.0
                && (x - 8.0) * (x - 8.0) + (z - 8.0) * (z - 8.0) > 25.0
        })
        .cloned()
        .expect("an oak on the ground");

    // A body walking at it along +X in fixed steps stops at it among the joined colliders, and walks on
    // through among the document's.
    use sapphire_physics::{
        BodyDesc, BodyId, CollisionGroups, HitTarget, MoveOptions, PhysicsWorld, Pose, Shape,
    };
    let (body, steps) = (0.3, 120);
    let walk = |colliders: &[u8]| {
        let set = ColliderSet::read(colliders).unwrap();
        let mut world = PhysicsWorld::from_cooked(set, 1).unwrap();
        world
            .add_body(BodyDesc {
                id: BodyId(1),
                shape: Shape::Capsule { half_height: 0.6, radius: body },
                pose: Pose::at(Vec3::new(x - 2.0, 1.2, z)),
                groups: CollisionGroups::new(Group::Character.mask(), Group::World.mask()),
            })
            .unwrap();
        let options = MoveOptions { max_sweeps: 1, ..MoveOptions::default() };
        let mut hit = None;
        for _ in 0..steps {
            let outcome =
                world.move_and_collide(BodyId(1), Vec3::new(4.0 / steps as f32, 0.0, 0.0), &options);
            if let Some(first) = outcome.unwrap().hits().first() {
                hit.get_or_insert(first.target);
            }
        }
        (world.body_pose(BodyId(1)).unwrap().position, hit, set.id_of(&name))
    };
    let (stopped, hit, trunk) = walk(&joined);
    assert_eq!(hit, Some(HitTarget::Static(sapphire_physics::ColliderId(trunk.expect("named")))));
    let against = x - radius - body;
    assert!(stopped.x < against && stopped.x > against - 0.05, "{stopped} against a trunk at {x}");
    let (walked, hit, _) = walk(&authored);
    assert_eq!(hit, None, "nothing stood there before");
    assert!((walked.x - (x + 2.0)).abs() < 1e-3, "{walked}");

    // The line through it walks straight on the bare navmesh, and not on the trunks', which routes round it.
    use sapphire_nav::{NavGraph, PathSearch, Reach, RouteEnd};
    let through = |bytes: &[u8]| {
        let graph = NavGraph::new(NavMesh::read(bytes).unwrap());
        let from = graph.locate(Vec3::new(x - 1.5, 0.0, z), Reach::CHARACTER).expect("west of it");
        let to = graph.locate(Vec3::new(x + 1.5, 0.0, z), Reach::CHARACTER).expect("east of it");
        let mut corridor = vec![0; graph.polygon_count()];
        let route = PathSearch::new(&graph, u32::MAX).find(&graph, from, to, &mut corridor);
        (graph.walks_straight(from, to), route.end)
    };
    assert_eq!(through(&bare_navmesh), (true, RouteEnd::Reached), "nothing stood there before");
    assert_eq!(through(&navmesh), (false, RouteEnd::Reached), "round the trunk, not through it");
}

/// A rule's plants keep off the zone's spline, its placements' footprints and its water, through the cook, and
/// a moved placement recooks the sets that keep off placements.
#[test]
fn a_zones_plants_keep_off_its_spline_placements_and_water() {
    let ferns = "avoid: [\n                    Kinds(kinds: [\"oak\"], distance: 1.5),";
    let keeping = "avoid: [Splines(names: [\"path\"], distance: 1.5), Placements(distance: 3.0), Water(distance: 1.0),\n                    Kinds(kinds: [\"oak\"], distance: 1.5),";
    assert!(grove_text().contains(ferns), "the grove's ferns are written as this test expects");
    let data = format!(
        r##"splines: [(name: "path", points: [(-30.0, 0.0, -6.0), (30.0, 0.0, -6.0)])],
            water: [(name: "pond", shape: Lake(outline: [(-20.0, -20.0), (-12.0, -20.0), (-12.0, -12.0), (-20.0, -12.0)], height: 0.2))],
            {}"##,
        placements()
    );
    let (tree, _, foliage) = foliage_tree("zone-foliage-keeps", &data);
    let roots = |report: &CookReport| -> Vec<[f32; 2]> {
        grove_plants(report, 1).into_iter().map(|(_, [x, _, z])| [x, z]).collect()
    };
    let free = roots(&tree.cook());
    tree.write_at(&foliage, grove_text().replace(ferns, keeping).as_bytes());
    let kept = roots(&tree.cook());
    assert!(kept.len() < free.len() && kept.len() > free.len() / 2, "{} of {}", kept.len(), free.len());

    let to_box = |[x, z]: [f32; 2], min: [f32; 2], max: [f32; 2]| {
        let [dx, dz] = [(min[0] - x).max(x - max[0]).max(0.0), (min[1] - z).max(z - max[1]).max(0.0)];
        (dx * dx + dz * dz).sqrt()
    };
    // The path is straight, so its curve is its chord; the crate at (2, 0, 2) is a unit square standing up
    // along X, so its footprint is that edge.
    let by_path = |root: [f32; 2]| to_box(root, [-30.0, -6.0], [30.0, -6.0]) < 1.5;
    let by_crate = |root: [f32; 2]| to_box(root, [2.0, 2.0], [3.0, 2.0]) < 3.0;
    let by_pond = |root: [f32; 2]| to_box(root, [-20.0, -20.0], [-12.0, -12.0]) < 1.0;
    for (what, test) in [
        ("the path", &by_path as &dyn Fn([f32; 2]) -> bool),
        ("the crate", &by_crate),
        ("the pond", &by_pond),
    ] {
        assert!(free.iter().any(|&root| test(root)), "ferns grew by {what} before");
        let near: Vec<_> = kept.iter().filter(|&&root| test(root)).collect();
        assert!(near.is_empty(), "and keep off it now: {near:?}");
    }

    // The sets keep off the placements, so moving one recooks them.
    let moved = foliage_zone(&data.replace("(2.0, 0.0, 2.0)", "(3.0, 0.0, 2.0)"));
    tree.write("town.zone.ron", moved.as_bytes());
    let report = tree.cook();
    assert_eq!(by_id(&report, scatter_id(0, 0)).status, CookStatus::Cooked, "a moved placement it keeps off");
}

/// The joined colliders, the navmesh baked from them and the zone are the same bytes cooked on one worker or
/// eight, as the scatter sets are.
#[test]
fn trunks_and_their_navmesh_cook_alike_on_one_worker_or_eight() {
    let (tree, _, foliage) = foliage_tree("zone-trunks-workers", &grove_navigation());
    tree.write_at(&foliage, trunked_grove().as_bytes());
    let cooked = |workers: usize| {
        let out = tree.dir.path().join(format!("out-{workers}"));
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false, editor: false };
        let report = cook_dir_on(&tree.content(), &out, &options, workers).unwrap();
        [trunks_id(), navigation_id(), ZONE].map(|id| fs::read(&by_id(&report, id).output).unwrap())
    };
    assert_eq!(cooked(1), cooked(8));
}

/// The trunks and the navmesh hold the sets' bytes, not their keys: a moved placement the ferns keep off recooks
/// their sets, and as it moves no oak, the trunks cook to the same bytes and the navmesh isn't baked again.
#[test]
fn a_moved_placement_that_moves_no_tree_leaves_the_navmesh_alone() {
    let ferns = "avoid: [\n                    Kinds(kinds: [\"oak\"], distance: 1.5),";
    let keeping =
        "avoid: [Placements(distance: 3.0),\n                    Kinds(kinds: [\"oak\"], distance: 1.5),";
    let (tree, _, foliage) = foliage_tree("zone-trunks-placements", &grove_navigation());
    tree.write_at(&foliage, trunked_grove().replace(ferns, keeping).as_bytes());
    let before = tree.cook();
    let moved = foliage_zone(&grove_navigation().replace("(2.0, 0.0, 2.0)", "(10.0, 0.0, 3.0)"));
    tree.write("town.zone.ron", moved.as_bytes());
    let after = tree.cook();
    assert_eq!(by_id(&after, scatter_id(0, 0)).status, CookStatus::Cooked, "the ferns keep off the crate");
    assert_ne!(by_id(&after, scatter_id(0, 0)).output_hash, by_id(&before, scatter_id(0, 0)).output_hash);
    assert_eq!(
        by_id(&after, trunks_id()).output_hash,
        by_id(&before, trunks_id()).output_hash,
        "no oak moved"
    );
    assert_eq!(
        by_id(&after, navigation_id()).status,
        CookStatus::UpToDate,
        "so the navmesh isn't baked again"
    );
}

/// [`placements`] at version 2 (M6 T8a), each with an ID of its own.
fn placements_with_ids() -> String {
    let mut text = placements();
    for index in 0..5 {
        text = text.replacen("(prefab:", &format!("(id: \"post-{index}\", prefab:"), 1);
    }
    text
}

/// Writes the zone at version 2.
fn write_zone_v2(tree: &Tree, data: &str) {
    let text = zone_text(data).replacen("version: 1", "version: 2", 1);
    tree.write("town.zone.ron", text.as_bytes());
}

/// What each asset a cook made holds, by its file's name.
fn cooked_bytes(report: &CookReport) -> BTreeMap<String, Vec<u8>> {
    (report.assets.iter())
        .map(|asset| (asset.output.display().to_string(), fs::read(&asset.output).expect("cooked")))
        .collect()
}

/// A zone's IDs reach no cooked byte, a `rotation` that's a yaw alone cooks as the same `turn` does, and an ID two
/// placements share or a `turn` beside a `rotation` fails the cook naming the field.
#[test]
fn a_zones_ids_cook_to_nothing_and_a_yaw_rotation_to_its_turn() {
    let tree = tree("zone-ids");
    let first = "at: (2.0, 0.0, 2.0)";
    write_zone(&tree, &placements().replacen(first, &format!("{first}, turn: -150.0"), 1));
    let turned = cooked_bytes(&tree.cook());
    assert!(turned.len() > 3, "{turned:?}");

    write_zone_v2(&tree, &placements_with_ids().replacen(first, &format!("{first}, turn: -150.0"), 1));
    assert_eq!(cooked_bytes(&tree.cook()), turned, "the IDs and the version change no byte");
    let rotated =
        placements_with_ids().replacen(first, &format!("{first}, rotation: Some((0.0, -150.0, 0.0))"), 1);
    write_zone_v2(&tree, &rotated);
    assert_eq!(cooked_bytes(&tree.cook()), turned, "a yaw is the turn");

    for (data, expected) in [
        (placements_with_ids().replacen("post-1", "post-0", 1), "placements[1].id"),
        (rotated.replacen("rotation:", "turn: 30.0, rotation:", 1), "placements[0].rotation"),
    ] {
        write_zone_v2(&tree, &data);
        let text = chain(&tree.try_cook(false).expect_err(expected));
        assert!(text.contains("town.zone.ron") && text.contains(expected), "`{text}` must say `{expected}`");
    }
}

/// A pond over the arena's ground and its table, a race along its west side, and the sea under them: the lake
/// and the river are drawn, the sea isn't.
const WATER_BODIES: &str = r#"water: [
    (name: "pond", shape: Lake(outline: [(-4.0, -4.0), (12.0, -4.0), (12.0, 12.0), (-4.0, 12.0)], height: 1.5)),
    (name: "race", shape: River(points: [(at: (-28.0, 1.0, -28.0), width: 4.0, speed: 2.0), (at: (-28.0, 0.8, 24.0), width: 4.0, speed: 2.0)]), swim: false),
    (name: "sea", shape: Ocean(height: -5.0)),
],"#;

/// [`NAV_COLLIDERS`]' ground with `water` and [`placements`].
fn water_tree(name: &str, water: &str) -> (Tree, PathBuf) {
    nav_tree(name, &format!(r##"colliders: Some("{COLLIDERS}"), {} {water}"##, placements()))
}

fn water_id(name: &str) -> AssetId {
    ZONE.sub_asset(&sapphire_zone::water::water_part(Name::new(name).as_u64()))
}

/// A zone's lakes and rivers cook to a drawn surface each (M6 W3a), each the bytes the water cook makes alone
/// on the zone's colliders: the `.szone` names them outside its digest, each body reflects the probe placed
/// for it, and they ship with the zone. On one thread or eight, every byte the same.
#[test]
fn a_zones_lakes_and_rivers_cook_to_drawn_surfaces_that_ship_with_it() {
    let (tree, _) = water_tree("zone-water-drawn", WATER_BODIES);
    let report = tree.cook();
    let zone_bytes = read_zone(&report);
    let zone = Zone::read(&zone_bytes).unwrap();
    let colliders = read_aligned(&by_id(&report, COLLIDERS).output);
    let ground = zone::scatter::ColliderGround::from_cooked(&colliders).unwrap();
    let text = fs::read_to_string(tree.content().join("town.zone.ron")).unwrap();
    let document = ZoneDocument::from_ron(&text).unwrap();
    let sizes = zone::water::sizes(&document.water, Cook::Desktop);

    for (index, body) in document.water.iter().enumerate() {
        let at = zone.simulation.water.iter().position(|volume| volume.name == body.name.as_str()).unwrap();
        let surface = &zone.presentation.water[at];
        if body.name == "sea" {
            assert_eq!(surface.mesh.as_ref().copied(), None, "the sea isn't drawn");
            assert_eq!(surface.probe.as_ref().copied(), None, "and reflects the sky");
            continue;
        }
        assert_eq!(surface.mesh.as_ref().copied(), Some(water_id(&body.name)), "{}", body.name);
        assert_eq!(
            surface.probe.as_ref().map(|probe| probe.to_native()),
            Some(index as u32),
            "its own probe"
        );
        let asset = by_id(&report, water_id(&body.name));
        let name = format!("town.water.{:016x}.swater", Name::new(&body.name).as_u64());
        assert!(asset.output.ends_with(&name), "{}", asset.output.display());
        let bytes = read_aligned(&asset.output);
        assert_eq!(AssetKind::WaterMesh.references(&bytes).unwrap(), [], "it loads nothing else");
        let alone =
            zone::water::cook(body, Some(&ground), sizes[index].unwrap()).unwrap().to_cooked().unwrap();
        assert_eq!(&bytes[..], &alone[..], "the cook is the body on the zone's colliders, and nothing else");

        let mesh = WaterMesh::read(&bytes).unwrap();
        assert_eq!(mesh.body.to_native(), Name::new(&body.name).as_u64());
        let (low, high) = body.shape.bounds();
        let margin = body.shore_margin;
        for vertex in mesh.vertices.iter() {
            let [x, _, z] = vertex.position.map(|value| value.to_native());
            assert!(
                low[0] - margin <= x && x <= high[0] + margin,
                "{}: {x} within its grown bounds",
                body.name
            );
            assert!(low[1] - margin - 0.01 <= z && z <= high[1] + margin + 0.01, "{}: {z}", body.name);
        }
        // The bed is the colliders' under the surface, wherever the body is.
        let info = &mesh.info;
        let mut sampled = 0;
        for j in 0..info.height.to_native() {
            for i in 0..info.width.to_native() {
                let texel = info.texel.to_native();
                let [x, z] = [i, j].map(|k| k as f32 + 0.5);
                let (x, z) = (info.origin[0].to_native() + x * texel, info.origin[1].to_native() + z * texel);
                let Some((surface, _)) = body.shape.column(x, z) else { continue };
                let (bed, _) = ground.under(x, z, surface);
                let bed = bed.expect("the arena's ground is under all of it");
                let depth = ((f64::from(surface) - bed) / 0.1).round() as u8;
                assert_eq!(info.texel(i, j)[0], depth, "{} at ({x}, {z})", body.name);
                sampled += 1;
            }
        }
        assert!(sampled > 100, "{sampled}");
    }
    let pond_bytes = read_aligned(&by_id(&report, water_id("pond")).output);
    let pond = WaterMesh::read(&pond_bytes).unwrap();
    let at = |x: f32, z: f32| {
        let info = &pond.info;
        let texel = info.texel.to_native();
        let [i, j] = [(x, 0), (z, 1)].map(|(at, axis)| ((at - info.origin[axis].to_native()) / texel) as u32);
        info.texel(i, j)[0]
    };
    assert_eq!((at(0.5, 0.5), at(8.5, 8.5)), (15, 5), "1.5 m over the ground, half a metre over the table");

    // The probes placed for the pond and the race, in the water's order.
    let probes = read_aligned(&by_id(&report, reflections_id()).output);
    let probes = ReflectionProbes::read(&probes).unwrap();
    let names: Vec<&str> = probes.probes.iter().map(|probe| probe.name.as_str()).collect();
    assert_eq!(names, ["pond", "race"]);
    assert_eq!(probes.probes[0].position.map(|value| value.to_native()), [4.0, 3.5, 4.0], "over its middle");
    let race = probes.probes[1].position.map(|value| value.to_native());
    assert!(race[0] == -28.0 && (race[1] - 2.9).abs() < 1e-5 && race[2] == -2.0, "halfway: {race:?}");

    let reach = AssetKind::Zone.reach(&zone_bytes).unwrap();
    assert!(reach.contains(&water_id("pond")) && reach.contains(&water_id("race")));
    assert!(!zone.references().contains(&water_id("pond")), "the simulation reads none of it");
    let packs = tree.dir.path().join("packs");
    let packed = pack_cooked(&report, &packs, None).unwrap();
    let holding = |id: AssetId| -> Vec<String> {
        (packed.packs.iter())
            .filter(|pack| {
                let bytes = fs::read(&pack.path).unwrap();
                PackReader::new(&bytes).unwrap().toc().entries().iter().any(|entry| entry.id == id)
            })
            .map(|pack| pack.name.clone())
            .collect()
    };
    assert_eq!(holding(water_id("race")), ["zone_town"], "a surface ships with its zone, and only there");

    // From nothing, on one thread and on eight: the same files.
    let cook_with = |name: &str, workers: usize| {
        let (tree, _) = water_tree(name, WATER_BODIES);
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false, editor: false };
        let report = cook_dir_on(&tree.content(), &tree.out(), &options, workers).unwrap();
        report.assets.iter().map(|asset| (asset.id, fs::read(&asset.output).unwrap())).collect::<Vec<_>>()
    };
    let serial = cook_with("zone-water-serial", 1);
    let drawn = serial.iter().find(|(id, _)| *id == water_id("pond")).expect("cooked");
    assert_eq!(&drawn.1[..], &read_aligned(&by_id(&report, water_id("pond")).output)[..]);
    assert_eq!(serial, cook_with("zone-water-workers", 8));
}

/// A surface's key holds its body's shape and margin, the sizes the zone's water gives it and the colliders,
/// not the rest of the zone.
#[test]
fn only_its_shape_or_the_colliders_recook_a_bodys_surface() {
    let (tree, colliders) = water_tree("zone-water-keys", WATER_BODIES);
    tree.cook();
    let status = |report: &CookReport, name: &str| by_id(report, water_id(name)).status;

    let moved = placements().replace("(2.0, 0.0, 2.0)", "(3.0, 0.0, 2.0)");
    write_zone(&tree, &format!(r##"colliders: Some("{COLLIDERS}"), {moved} {WATER_BODIES}"##));
    let report = tree.cook();
    assert_eq!(by_id(&report, cell_id(cell(0, 0))).status, CookStatus::Cooked, "a moved placement");
    assert_eq!(
        (status(&report, "pond"), status(&report, "race")),
        (CookStatus::UpToDate, CookStatus::UpToDate)
    );

    let higher = WATER_BODIES.replace("height: 1.5", "height: 1.75");
    write_zone(&tree, &format!(r##"colliders: Some("{COLLIDERS}"), {moved} {higher}"##));
    let report = tree.cook();
    assert_eq!(
        (status(&report, "pond"), status(&report, "race")),
        (CookStatus::Cooked, CookStatus::UpToDate)
    );

    tree.write_at(&colliders, NAV_COLLIDERS.replace("(8.0, 0.5, 8.0)", "(8.0, 0.5, 9.0)").as_bytes());
    let report = tree.cook();
    assert_eq!((status(&report, "pond"), status(&report, "race")), (CookStatus::Cooked, CookStatus::Cooked));
}

/// A lake whose outline grown by its margin meets itself, and a river turning too tightly for its width, fail
/// the cook naming the zone and the field.
#[test]
fn what_a_zone_cant_draw_of_its_water_is_named() {
    let cases = [
        (
            r#"water: [(name: "you", shape: Lake(outline: [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (6.5, 10.0), (6.5, 2.0), (5.5, 2.0), (5.5, 10.0), (0.0, 10.0)], height: 1.0))],"#,
            "water[0].shore_margin",
        ),
        (
            r#"water: [(name: "pond", shape: Lake(outline: [(-4.0, -4.0), (4.0, -4.0), (4.0, 4.0)], height: 1.0)), (name: "bend", shape: River(points: [(at: (0.0, 0.5, 0.0), width: 4.0), (at: (20.0, 0.5, 0.0), width: 4.0), (at: (0.0, 0.5, 3.5), width: 4.0)]))],"#,
            "water[1].shape.points[1]",
        ),
    ];
    for (index, (water, expected)) in cases.into_iter().enumerate() {
        let (tree, _) = water_tree(&format!("zone-water-refused-{index}"), water);
        let text = chain(&tree.try_cook(false).expect_err(water));
        assert!(text.contains("town.zone.ron"), "the zone's named in `{text}`");
        assert!(text.contains(expected), "`{text}` must say `{expected}`");
    }
}

/// The arena's ground, its top at 0, and a rock standing from 2 m under it to 2 m over it at (10, −20).
const RIVER_COLLIDERS: &str = r##"(type: "sapphire::colliders::ColliderDocument", version: 1, data: (colliders: [
    (name: "ground", shape: Box(half_extents: (32.0, 0.5, 32.0)), position: (0.0, -0.5, 0.0)),
    (name: "rock", shape: Box(half_extents: (1.0, 2.0, 1.0)), position: (10.0, 0.0, -20.0)),
]))"##;

/// A race over [`RIVER_COLLIDERS`]' ground: level, then a fall at 60° (a 3.46 m drop over 2 m), then fast and
/// shallow past the rock; a gentle brook; and a pond, which has no reading.
const RIVERS: &str = r#"water: [
    (name: "race", shape: River(points: [(at: (-28.0, 4.5, -20.0), width: 6.0, speed: 2.0), (at: (-10.0, 4.5, -20.0), width: 6.0, speed: 2.0), (at: (-8.0, 1.0358984, -20.0), width: 6.0, speed: 5.0), (at: (28.0, 0.4, -20.0), width: 6.0, speed: 5.0)]), swim: false),
    (name: "brook", shape: River(points: [(at: (-28.0, 1.0, 20.0), width: 3.0, speed: 0.5), (at: (28.0, 0.8, 20.0), width: 3.0, speed: 0.5)])),
    (name: "pond", shape: Lake(outline: [(-4.0, -4.0), (12.0, -4.0), (12.0, 12.0), (-4.0, 12.0)], height: 1.5)),
],"#;

/// [`RIVER_COLLIDERS`] with `water` and [`placements`].
fn river_tree(name: &str, water: &str) -> (Tree, PathBuf) {
    let tree = tree(name);
    let colliders = tree.write("arena.colliders.ron", RIVER_COLLIDERS.as_bytes());
    tree.sidecar(&colliders, COLLIDERS, ImportSettings::Colliders(Default::default()));
    write_zone(&tree, &format!(r##"colliders: Some("{COLLIDERS}"), {} {water}"##, placements()));
    (tree, colliders)
}

/// Each river's reading in a cooked zone, by its body's name: none for a lake or the sea.
fn readings(bytes: &[u8]) -> Vec<(String, Option<sapphire_zone::CookedRiver>)> {
    let zone = Zone::read(bytes).unwrap();
    (zone.simulation.water.iter().zip(zone.presentation.water.iter()))
        .map(|(volume, surface)| {
            (volume.name.to_string(), surface.river.as_ref().map(|river| river.to_native()))
        })
        .collect()
}

/// A zone's rivers are read into its presentation (M6 H3a), each the reading the river cook makes alone of its
/// points on the zone's colliders: the race's fall at its 60° drop and rapids by the rock, the brook's single
/// stretch with no site, no reading for the pond; `cook --report` has a line for each river. From nothing, on
/// one thread or eight, every byte the same.
#[test]
fn a_zones_rivers_are_read_into_its_presentation_and_the_same_on_any_threads() {
    let (tree, _) = river_tree("zone-rivers", RIVERS);
    let report = tree.cook();
    let zone_bytes = read_zone(&report);
    let colliders = read_aligned(&by_id(&report, COLLIDERS).output);
    let ground = zone::scatter::ColliderGround::from_cooked(&colliders).unwrap();
    let text = fs::read_to_string(tree.content().join("town.zone.ron")).unwrap();
    let document = ZoneDocument::from_ron(&text).unwrap();
    let read: BTreeMap<String, Option<sapphire_zone::CookedRiver>> =
        readings(&zone_bytes).into_iter().collect();
    for body in &document.water {
        let sapphire_zone::WaterShape::River { points } = &body.shape else {
            assert_eq!(read[&body.name], None, "a lake has no reading");
            continue;
        };
        let alone = zone::river::cook(points, Some(&ground)).unwrap();
        assert_eq!(
            read[&body.name],
            Some(alone),
            "{}: the river on the zone's colliders, and nothing else",
            body.name
        );
    }
    let race = read["race"].clone().unwrap();
    let flags: Vec<bool> = race.stretches.iter().map(|stretch| stretch.fall).collect();
    assert_eq!(flags, [false, true, false]);
    let [fall] = race.falls.as_slice() else { panic!("one fall: {:?}", race.falls) };
    assert_eq!((fall.lip, fall.foot), ([-10.0, 4.5, -20.0], [-8.0, 1.035_898_4, -20.0]));
    assert_eq!(race.sites[0].kind, WaterSiteKind::FallFoot, "the foot first, downstream: {:?}", race.sites);
    let rapids: Vec<&WaterSite> =
        race.sites.iter().filter(|site| site.kind == WaterSiteKind::Rapids).collect();
    assert!(
        !rapids.is_empty() && rapids.iter().all(|site| (6.0..=14.0).contains(&site.position[0])),
        "{rapids:?}"
    );
    let brook = read["brook"].clone().unwrap();
    assert_eq!(
        (brook.stretches.len(), brook.falls.len(), brook.sites.len()),
        (1, 0, 0),
        "a gentle stretch has none"
    );
    let lines: Vec<(&str, usize, usize, usize, usize)> = (report.rivers.iter())
        .map(|line| (line.name.as_str(), line.stretches, line.falls, line.rapids, line.feet))
        .collect();
    assert_eq!(
        lines,
        [("brook", 1, 0, 0, 0), ("race", 3, 1, rapids.len(), 1)],
        "in the cooked water's order, by ID"
    );

    let cook_with = |name: &str, workers: usize| {
        let (tree, _) = river_tree(name, RIVERS);
        let options = CookOptions { cook: Cook::Desktop, cache: None, locked: false, editor: false };
        let report = cook_dir_on(&tree.content(), &tree.out(), &options, workers).unwrap();
        report.assets.iter().map(|asset| (asset.id, fs::read(&asset.output).unwrap())).collect::<Vec<_>>()
    };
    let serial = cook_with("zone-rivers-serial", 1);
    let own = serial.iter().find(|(id, _)| *id == ZONE).expect("cooked");
    assert_eq!(&own.1[..], &zone_bytes[..], "the same `.szone` as a cook with a cache");
    assert_eq!(serial, cook_with("zone-rivers-workers", 8));
}

/// A zone with a river holds its colliders' key, so a moved rock reads its rapids again.
#[test]
fn a_moved_rock_reads_a_zones_rivers_again() {
    let (tree, colliders) = river_tree("zone-rivers-keys", RIVERS);
    let rapids = |report: &CookReport| -> Vec<[f32; 3]> {
        let read = readings(&read_zone(report));
        let race =
            read.into_iter().find(|(name, _)| name == "race").and_then(|(_, river)| river).expect("read");
        race.sites
            .iter()
            .filter(|site| site.kind == WaterSiteKind::Rapids)
            .map(|site| site.position)
            .collect()
    };
    let before = rapids(&tree.cook());
    let moved = RIVER_COLLIDERS.replace("(10.0, 0.0, -20.0)", "(20.0, 0.0, -20.0)");
    tree.write_at(&colliders, moved.as_bytes());
    let report = tree.cook();
    assert_eq!(by_id(&report, ZONE).status, CookStatus::Cooked, "the rock moved");
    let after = rapids(&report);
    assert_ne!(after, before);
    assert!(
        !after.is_empty() && after.iter().all(|[x, ..]| (16.0..=24.0).contains(x)),
        "by the rock: {after:?}"
    );
}

/// A river climbing more steeply than a fall falls fails the cook naming the zone and the point.
#[test]
fn a_river_that_climbs_like_a_fall_is_named() {
    let water = r#"water: [(name: "up", shape: River(points: [(at: (-28.0, 1.0, 0.0), width: 4.0), (at: (-20.0, 1.0, 0.0), width: 4.0), (at: (-19.0, 4.0, 0.0), width: 4.0)]))],"#;
    let (tree, _) = river_tree("zone-rivers-refused", water);
    let text = chain(&tree.try_cook(false).expect_err("climbs"));
    assert!(text.contains("town.zone.ron"), "the zone's named in `{text}`");
    assert!(text.contains("water[0].shape.points[2].at"), "`{text}` names the point");
}
