//! A zone's IDs (M6 T8a): what a migration writes, what a path calls a placement, and what a version 1 zone
//! refuses so its IDs, its entries' places, can't shift under a selection.

mod support;

use std::sync::Arc;

use sapphire_document::zone::{self, add_entry, migrate, version_of};
use sapphire_document::{
    Document, DocumentSet, Kind, Outcome, PropertyPath, edit_with_colliders, kinds, registry_of, walk,
};
use sapphire_zone::{IdList, Placement, ZoneDocument};
use support::read;

/// A version 1 zone with what a migration must keep: comments, a placement of one line and one of several, one
/// written with its type's name, one that writes an ID of its own (which version 1 reads past), strings holding
/// brackets and `//`, a split, a list's last entry with no comma after it, an instance, a light, an atmosphere
/// volume, and lists it has nothing in.
const OLD: &str = r##"// A yard.
(
    type: "sapphire::zone::ZoneDocument",
    version: 1,
    data: (
        grid: (cell_size: 16.0),
        placements: [
            // A post.
            (prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (-6.0, 0.0, -6.0)),
            (
                // A turned one.
                prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f",
                at: (6.0, 0.0, -6.0),
                turn: 30.0,
            ),
            Placement(prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (0.0, 0.0, 6.0)),
            (id: "gate (old) // kept", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", split: Some((backdrop: ["#ground", "(rock) // *"]))),
            (prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", materials: {"wood (dark) // a": "f6dcae8b-5937-4299-ba5e-e2f026fc2762"}, at: (1.0, 0.0, 1.0))
        ],
        instances: [(mesh: "f6dcae8b-5937-4299-ba5e-e2f026fc2762", at: (4.0, 0.0, 6.0))],
        lights: [
            (position: (4.0, 2.5, 6.0), color: (1.0, 0.72, 0.42), intensity: 6.0, range: 8.0),
        ],
        volumes: [(name: "hall", position: (0.0, 2.0, 0.0), shape: Sphere(radius: 3.0), look: "hall_day.ron", blend: 1.0)],
        spawners: [],
    ),
)
"##;

/// [`OLD`] at version 2, as the migration writes it, byte for byte.
const NEW: &str = r##"// A yard.
(
    type: "sapphire::zone::ZoneDocument",
    version: 2,
    data: (
        grid: (cell_size: 16.0),
        placements: [
            // A post.
            (id: "placement-0", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (-6.0, 0.0, -6.0)),
            (
                id: "placement-1",
                // A turned one.
                prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f",
                at: (6.0, 0.0, -6.0),
                turn: 30.0,
            ),
            Placement(id: "placement-2", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", at: (0.0, 0.0, 6.0)),
            (id: "placement-3", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", split: Some((backdrop: ["#ground", "(rock) // *"]))),
            (id: "placement-4", prefab: "1ff3290e-5fa9-4f61-b891-2ca6d76b285f", materials: {"wood (dark) // a": "f6dcae8b-5937-4299-ba5e-e2f026fc2762"}, at: (1.0, 0.0, 1.0))
        ],
        instances: [(id: "instance-0", mesh: "f6dcae8b-5937-4299-ba5e-e2f026fc2762", at: (4.0, 0.0, 6.0))],
        lights: [
            (id: "light-0", position: (4.0, 2.5, 6.0), color: (1.0, 0.72, 0.42), intensity: 6.0, range: 8.0),
        ],
        volumes: [(id: "volume-0", name: "hall", position: (0.0, 2.0, 0.0), shape: Sphere(radius: 3.0), look: "hall_day.ron", blend: 1.0)],
        spawners: [],
    ),
)
"##;

fn zone(text: &str) -> Document<ZoneDocument> {
    Document::open(text.to_owned(), kinds::zone(), Arc::new(registry_of::<ZoneDocument>()))
}

fn path(text: &str) -> PropertyPath {
    text.parse().expect(text)
}

/// The migration gives each entry the ID it reads with, first, in the file's layout, raises the version, and
/// moves no other byte; a zone at version 2 is left alone.
#[test]
fn a_migration_writes_the_ids_a_zone_reads_with_and_nothing_else() {
    assert_eq!(version_of(OLD), Some(1));
    let migrated = migrate(OLD).expect("migrates").expect("at version 1");
    assert_eq!(migrated, NEW);
    assert_eq!(
        ZoneDocument::from_ron(&migrated).expect("reads"),
        ZoneDocument::from_ron(OLD).expect("reads")
    );
    assert!(migrate(NEW).expect("reads").is_none(), "already there");
}

/// Every shipped zone is at version 2, so the editor's command has nothing left to do to any, and their IDs are
/// what their places were when they were migrated.
#[test]
fn every_shipped_zone_is_migrated() {
    for name in ["lantern_ward", "proving_ground", "proving_hub", "square", "square_street", "town"] {
        let text = read(&format!("content/zones/{name}/{name}.zone.ron"));
        assert_eq!(version_of(&text), Some(2), "{name}");
        assert!(migrate(&text).expect(name).is_none(), "{name}");
        let document = ZoneDocument::from_ron(&text).expect(name);
        assert_eq!(document.clone().with_place_ids(), document, "{name}: migrated, its IDs are its places'");
    }
}

/// A path calls a placement by its ID, so the same path finds it after an entry goes in before it.
#[test]
fn a_path_calls_a_placement_by_its_id_wherever_it_is() {
    let mut document = zone(NEW);
    let value = document.value().expect("reads");
    let keyed = walk::keyed(value, &path("placements[1].at[0]")).expect("there");
    assert_eq!(keyed, path(r#"placements["placement-1"].at[0]"#));
    assert_eq!(document.entry_path(&path("lights"), 0), Ok(path(r#"lights["light-0"]"#)));

    let entry = Placement { id: "3fa91c07".to_owned(), at: [0.0, 0.0, 9.0], ..Placement::default() };
    assert_eq!(document.insert("place one first", &path("placements"), 0, &entry), Ok(Outcome::Changed));
    assert_eq!(document.set("move it", &keyed, &7.5_f32), Ok(Outcome::Changed));
    let value = document.value().expect("reads");
    assert_eq!((value.placements[0].id.as_str(), value.placements[2].at[0]), ("3fa91c07", 7.5));
}

/// An entry added to a list with IDs takes a fresh one no entry has, of eight hexadecimal digits.
#[test]
fn an_added_entry_takes_an_id_of_its_own() {
    let mut document = zone(NEW);
    for list in IdList::ALL {
        let before = document.value().expect("reads").ids().count();
        let added = add_entry(&mut document, "add", list, None).map(|(outcome, _)| outcome);
        let value = document.value().expect("reads");
        match list {
            // A volume's blank has no look, which a zone refuses, as it could always refuse a blank.
            IdList::Placements | IdList::Instances | IdList::Lights => {
                assert_eq!(added.expect("added"), Outcome::Changed, "{list:?}");
                let (_, _, id) = value.ids().filter(|(of, _, _)| *of == list).last().expect("the new one");
                assert!(id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
                assert_eq!(value.ids().filter(|(_, _, other)| other == &id).count(), 1);
                assert_eq!(value.ids().count(), before + 1);
            }
            IdList::Volumes => assert!(added.is_err(), "{list:?}"),
        }
    }
    let new = zone::new_id(document.value().expect("reads")).expect("an ID");
    assert!(!document.value().expect("reads").has_id(&new));
}

/// A version 1 zone's IDs are its places, so an edit that adds or takes out one of its entries is refused, the
/// text as it was, until it's migrated; an edit of one in its place isn't. (One that writes an ID other than its
/// place's, which version 1 reads past, takes no edit at all: only `MigrateZone`.)
#[test]
fn a_version_one_zone_refuses_what_would_shift_its_ids() {
    let old = OLD.replace(r#"id: "gate (old) // kept", "#, "");
    let dir = std::env::temp_dir().join(format!("sapphire_document_zone_ids_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    let mut set = DocumentSet::new(vec![dir.clone()]);
    let file = dir.join("yard.zone.ron");
    std::fs::write(&file, &old).expect("a zone");
    let id = Kind::Zone.open(&mut set, &file).expect("opens");
    let removed = edit_with_colliders(&mut set, id, "take one out", |document| {
        document.remove_entry("take one out", &path("placements[0]"))
    });
    let message = removed.expect_err("shifts the IDs").to_string();
    assert!(message.contains("`placements`") && message.contains("MigrateZone"), "{message}");
    let moved = edit_with_colliders(&mut set, id, "move one", |document| {
        document.set_ron("move one", &path(r#"placements["placement-1"].at[0]"#), "7.0")
    });
    assert_eq!(moved.expect("in its place"), Outcome::Changed);
    let text = set.any(id).expect("open").text().to_owned();
    assert_eq!(text, old.replace("at: (6.0, 0.0, -6.0)", "at: (7.0, 0.0, -6.0)"));
    let _ = std::fs::remove_dir_all(&dir);
}
