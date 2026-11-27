//! Steps of several documents at once (M6 T2): each document keeps its own history, which still says what's
//! unsaved of its own file, and a set step over them is undone and redone whole or not at all. A zone's
//! placement moves with the colliders it names.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, fs, process};

use sapphire_assets::AssetId;
use sapphire_colliders::ColliderDocument;
use sapphire_document::{
    DocumentSet, HistoryError, Journal, Kind, Outcome, PropertyPath, Refused, Took, ZoneEditError,
    edit_with_colliders,
};
use sapphire_zone::ZoneDocument;
use support::read;

const PLAYER: &str = "content/combat/player/player.moves.ron";
const CUE: &str = "content/audio/hit_heavy.cue.ron";

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("sapphire_set_steps_{name}_{}_{unique}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("a test file");
        path
    }

    fn set(&self) -> DocumentSet {
        DocumentSet::new(vec![self.0.clone()])
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open(set: &mut DocumentSet, path: &Path) -> AssetId {
    Kind::of(path).expect("a kind").open(set, path).expect("opens")
}

fn path(text: &str) -> PropertyPath {
    text.parse().expect("a path")
}

fn text(set: &DocumentSet, id: AssetId) -> String {
    set.any(id).expect("open").text().to_owned()
}

/// A moves file and a cue, each with one edit made as one set step.
fn two_with_a_set_step(dir: &TestDir) -> (DocumentSet, AssetId, AssetId) {
    let mut set = dir.set();
    let moves = open(&mut set, &dir.write("a.moves.ron", &read(PLAYER)));
    let cue = open(&mut set, &dir.write("b.cue.ron", &read(CUE)));
    set.together("both", |set| {
        set.edit(moves).expect("open").set_ron("both", &path("moves[0].startup"), "9")?;
        set.edit(cue).expect("open").set_ron("both", &path("volume_db[0]"), "-2.0")
    })
    .expect("lands");
    (set, moves, cue)
}

/// Undo of either document takes back both, redo brings both back, and each file saved alone keeps its own
/// undo, which is what the per-file save and the journal count.
#[test]
fn a_set_step_undoes_whole_from_either_document_and_each_saves_alone() {
    let dir = TestDir::new("whole");
    let (mut set, moves, cue) = two_with_a_set_step(&dir);
    let step = set.set_step(cue, false).expect("a set step").clone();
    assert_eq!(step.label, "both");
    assert_eq!(step.parts.len(), 2);
    assert_eq!(set.set_step(moves, false), Some(&step), "the same step from either side");
    assert_eq!(set.stats().set_steps, 1);
    let edited = (text(&set, moves), text(&set, cue));

    set.save(cue).expect("saves");
    assert!(!set.is_dirty(cue) && set.is_dirty(moves), "a save is of its own file");
    assert_eq!(set.undo(moves), Ok(Took::Step));
    assert_eq!(text(&set, moves), read(PLAYER));
    assert_eq!(text(&set, cue), read(CUE), "the cue's part went too");
    assert!(set.is_dirty(cue), "the cue saved its edit, so taking it back is unsaved");
    assert!(!set.is_dirty(moves));
    assert_eq!(set.redo(cue), Ok(Took::Step));
    assert_eq!((text(&set, moves), text(&set, cue)), edited);
    assert!(!set.is_dirty(cue), "back to what it saved");
}

/// A document's own later edit goes first: the set step waits for it, saying which, and nothing moves.
#[test]
fn a_set_step_waits_for_a_documents_own_later_edits() {
    let dir = TestDir::new("waits");
    let (mut set, moves, cue) = two_with_a_set_step(&dir);
    set.edit(cue).expect("open").set_ron("own", &path("volume_db[0]"), "-2.5").expect("lands");
    let (moves_text, cue_text) = (text(&set, moves), text(&set, cue));
    match set.undo(moves) {
        Err(HistoryError::Later { label, path, redo: false }) => {
            assert_eq!(label, "both");
            assert!(path.ends_with("b.cue.ron"), "{}", path.display());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!((text(&set, moves), text(&set, cue)), (moves_text, cue_text));
    assert_eq!(set.undo(cue), Ok(Took::Step), "the cue's own goes first");
    assert_eq!(set.undo(moves), Ok(Took::Step), "then both");
    assert_eq!((text(&set, moves), text(&set, cue)), (read(PLAYER), read(CUE)));
}

/// A set step that isn't bound, whose part in another document is gone from where undo or redo would find it,
/// goes in this document alone and names what it left: whether a new edit cleared the other's redo, the other
/// closed, or its part was taken on its own. It never sticks.
#[test]
fn a_set_step_that_cant_be_whole_falls_apart_and_says_what_it_left() {
    let dir = TestDir::new("apart");
    let (mut set, moves, cue) = two_with_a_set_step(&dir);
    assert_eq!(set.undo(cue), Ok(Took::Step));
    set.edit(cue).expect("open").set_ron("branch", &path("volume_db[0]"), "-1.5").expect("lands");
    match set.redo(moves) {
        Ok(Took::Apart { label, left }) => {
            assert_eq!(label, "both");
            assert!(left.len() == 1 && left[0].ends_with("b.cue.ron"), "{left:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(text(&set, moves).contains("startup: 9"), "the moves' part went alone");
    assert_eq!(set.stats().set_steps, 0, "and the step with it");

    let (mut set, moves, cue) = two_with_a_set_step(&TestDir::new("closed"));
    assert!(set.close(cue));
    assert!(matches!(set.undo(moves), Ok(Took::Apart { .. })));
    assert_eq!(text(&set, moves), read(PLAYER));
    assert_eq!(set.stats().set_steps, 0);

    // One part undone on its own, past the set: the other no longer waits on it either way.
    let (mut set, moves, cue) = two_with_a_set_step(&TestDir::new("alone"));
    assert!(set.any_mut(cue).expect("open").undo());
    assert!(matches!(set.undo(moves), Ok(Took::Apart { .. })), "not stuck behind a part already gone");
    assert_eq!((text(&set, moves), text(&set, cue)), (read(PLAYER), read(CUE)));
    assert_eq!(set.redo(moves), Ok(Took::Step), "each as its own from here");
}

/// A gesture of the set's over several frames is one step in each document it edited, and one set step; one
/// that edited only one document is that document's own; a refused edit inside `together` takes back what it
/// did and leaves the gesture's earlier frames.
#[test]
fn a_set_gesture_is_one_step_and_a_refused_edit_inside_it_takes_back_only_itself() {
    let dir = TestDir::new("gesture");
    let mut set = dir.set();
    let moves = open(&mut set, &dir.write("a.moves.ron", &read(PLAYER)));
    let cue = open(&mut set, &dir.write("b.cue.ron", &read(CUE)));
    set.begin_gesture("drag");
    for frame in 1..=5 {
        set.edit(moves)
            .expect("open")
            .set_ron("drag", &path("moves[0].startup"), &frame.to_string())
            .unwrap();
        set.edit(cue).expect("open").set_ron("drag", &path("volume_db[0]"), &format!("-0.{frame}")).unwrap();
    }
    let (moves_text, cue_text) = (text(&set, moves), text(&set, cue));
    let refused: Result<Outcome, Refused> = set.together("frame", |set| {
        set.edit(moves).expect("open").set_ron("drag", &path("moves[0].startup"), "7")?;
        set.edit(cue).expect("open").set_ron("drag", &path("volume_db[0]"), "\"loud\"")
    });
    assert!(refused.is_err());
    assert_eq!((text(&set, moves), text(&set, cue)), (moves_text, cue_text), "the frame went, whole");
    assert!(set.in_gesture(), "and the gesture goes on");
    set.end_gesture();
    assert_eq!(set.any(moves).expect("open").history().1, 1, "five frames are one step");
    assert_eq!(set.stats().set_steps, 1);
    assert_eq!(set.undo(cue), Ok(Took::Step));
    assert_eq!((text(&set, moves), text(&set, cue)), (read(PLAYER), read(CUE)));

    set.begin_gesture("alone");
    set.edit(cue).expect("open").set_ron("alone", &path("volume_db[0]"), "-2.0").unwrap();
    set.end_gesture();
    assert_eq!(set.set_step(cue, false), None, "one document's is its own");
}

/// A zone of two posts, each naming its collider, and a crate naming none.
fn zone_and_colliders(dir: &TestDir) -> (DocumentSet, AssetId, PathBuf) {
    const COLLIDERS: &str = "0aec06e8-ef8a-4f60-8dce-1e159cc277fa";
    const PREFAB: &str = "1ff3290e-5fa9-4f61-b891-2ca6d76b285f";
    let colliders = dir.write(
        "yard.colliders.ron",
        r#"(
    type: "sapphire::colliders::ColliderDocument",
    version: 1,
    data: (
        colliders: [
            (name: "ground", shape: Box(half_extents: (24.0, 0.5, 24.0)), position: (0.0, -0.5, 0.0)),
            // The posts.
            (name: "post_a", shape: Box(half_extents: (0.5, 2.0, 0.5)), position: (-6.0, 2.0, -6.0)),
            (name: "post_b", shape: Box(half_extents: (0.5, 2.0, 0.5)), position: (6.35, 2.0, -6.0)),
        ],
    ),
)
"#,
    );
    fs::write(
        sapphire_document::identity::sidecar_path(&colliders),
        sapphire_document::identity::sidecar_text(COLLIDERS.parse().expect("an ID"), Some("Colliders(())")),
    )
    .expect("a sidecar");
    let zone = dir.write(
        "yard.zone.ron",
        &format!(
            r#"(
    type: "sapphire::zone::ZoneDocument",
    version: 2,
    data: (
        grid: (cell_size: 16.0),
        placements: [
            (id: "3fa91c07", prefab: "{PREFAB}", at: (-6.0, 0.0, -6.0), colliders: ["post_a"]),
            (id: "8b2e5d40", prefab: "{PREFAB}", at: (6.35, 0.0, -6.0), colliders: ["post_b"]),
            (id: "c41d0a9f", prefab: "{PREFAB}", at: (0.0, 0.0, 3.0)),
        ],
        colliders: Some("{COLLIDERS}"),
    ),
)
"#
        ),
    );
    let mut set = dir.set();
    let id = open(&mut set, &zone);
    (set, id, colliders)
}

fn collider_at(set: &DocumentSet, colliders: &Path, name: &str) -> [f32; 3] {
    let id = set.ids().find(|id| set.path(*id) == Some(colliders)).expect("open");
    let document = set.get::<ColliderDocument>(id).and_then(|document| document.value()).expect("reads");
    document.colliders.iter().find(|entry| entry.name == name).expect("named").position
}

/// A placement moved takes the colliders it names with it, by as far and to the bit, in one step of the two
/// documents that undo takes back whole; one that names none moves alone; a zone the edit's refused by leaves
/// both as they were.
#[test]
fn a_placement_moves_with_the_colliders_it_names() {
    let dir = TestDir::new("placement");
    let (mut set, zone, colliders) = zone_and_colliders(&dir);
    let original_colliders = fs::read_to_string(&colliders).expect("reads");
    let original_zone = text(&set, zone);
    let moved = edit_with_colliders(&mut set, zone, "move a placement", |zone| {
        zone.set_spelled("move a placement", &path("placements[1].at[0]"), "6.4")
    });
    assert!(matches!(moved, Ok(Outcome::Changed)), "{moved:?}");
    assert_eq!(collider_at(&set, &colliders, "post_b"), [6.4, 2.0, -6.0], "6.35 + 0.05 is 6.4, to the bit");
    assert_eq!(collider_at(&set, &colliders, "post_a"), [-6.0, 2.0, -6.0]);
    assert_eq!(set.stats().set_steps, 1);
    let colliders_id = set.ids().find(|id| *id != zone).expect("opened by the move");
    assert!(text(&set, colliders_id).contains("// The posts."), "its comments stay");

    // Three placements at once: one step of the zone, and of the colliders for the two that name some.
    let three = edit_with_colliders(&mut set, zone, "raise three", |zone| {
        for index in 0..3 {
            zone.set_spelled("raise three", &path(&format!("placements[{index}].at[1]")), "1.0")?;
        }
        Ok(Outcome::Changed)
    });
    assert!(three.is_ok(), "{three:?}");
    assert_eq!(collider_at(&set, &colliders, "post_a"), [-6.0, 3.0, -6.0]);
    assert_eq!(set.zone_value(zone).placements[2].at, [0.0, 1.0, 3.0]);
    assert_eq!(set.undo(zone), Ok(Took::Step), "three placements are one undo");
    assert_eq!(collider_at(&set, &colliders, "post_a"), [-6.0, 2.0, -6.0]);
    assert_eq!(set.zone_value(zone).placements[2].at, [0.0, 0.0, 3.0]);

    // A placement with no colliders moves alone, as its document's own step.
    edit_with_colliders(&mut set, zone, "move the crate", |zone| {
        zone.set_spelled("move the crate", &path("placements[2].at[2]"), "4.0")
    })
    .expect("lands");
    assert_eq!(set.set_step(zone, false), None);

    // A zone that refuses the edit: nothing moves.
    let before = (text(&set, zone), text(&set, colliders_id));
    let refused = edit_with_colliders(&mut set, zone, "move it far", |zone| {
        zone.set_spelled("move it far", &path("placements[1].at[0]"), "1e9")
    });
    assert!(matches!(refused, Err(ZoneEditError::Refused(_))), "{refused:?}");
    assert_eq!((text(&set, zone), text(&set, colliders_id)), before);

    while set.undo(zone) == Ok(Took::Step) {}
    assert_eq!(text(&set, zone), original_zone);
    assert_eq!(text(&set, colliders_id), original_colliders);
}

/// A placement's step with its colliders is bound: with the colliders' part gone (their document closed, or that
/// part undone on its own), undo of the zone refuses, naming the colliders, rather than leave the placement where
/// its colliders aren't. A save of the zone knows the colliders go with it.
#[test]
fn a_placement_and_its_colliders_never_go_apart() {
    for how in ["closed", "undone alone"] {
        let dir = TestDir::new("bound");
        let (mut set, zone, colliders) = zone_and_colliders(&dir);
        edit_with_colliders(&mut set, zone, "move a placement", |zone| {
            zone.set_spelled("move a placement", &path("placements[1].at[0]"), "6.4")
        })
        .expect("lands");
        let colliders_id = set.ids().find(|id| *id != zone).expect("opened by the move");
        assert!(set.set_step(zone, false).expect("a set step").bound);
        assert_eq!(set.joined(zone), [colliders_id], "what a save of the zone takes with it");
        let moved = text(&set, zone);
        if how == "closed" {
            assert!(set.close(colliders_id));
        } else {
            assert!(set.any_mut(colliders_id).expect("open").undo());
        }
        match set.undo(zone) {
            Err(HistoryError::Gone { label, path, redo: false }) => {
                assert_eq!(label, "move a placement");
                assert_eq!(path, colliders, "{how}");
            }
            other => panic!("{how}: {other:?}"),
        }
        assert_eq!(text(&set, zone), moved, "{how}: the placement stays where its collider was put");
    }
}

/// A placement taken out shifts those after it, so a place before and after needn't be one placement: nothing
/// moves, even where the one now at a place names the same collider as the one that was.
#[test]
fn a_placement_added_or_taken_out_moves_no_collider() {
    let dir = TestDir::new("shifted");
    let (mut set, zone, colliders) = zone_and_colliders(&dir);
    let before = fs::read_to_string(&colliders).expect("reads");
    set.edit(zone)
        .expect("open")
        .set_ron("both name a", &path("placements[1].colliders[0]"), "\"post_a\"")
        .expect("lands");
    let taken = edit_with_colliders(&mut set, zone, "take one out", |zone| {
        zone.remove_entry("take one out", &path("placements[0]"))
    });
    assert!(taken.is_ok(), "{taken:?}");
    assert_eq!(set.zone_value(zone).placements.len(), 2);
    assert!(set.ids().all(|id| id == zone), "the colliders weren't opened, so nothing moved");
    assert!(set.save_all().is_empty());
    assert_eq!(fs::read_to_string(&colliders).expect("reads"), before);
}

trait ZoneValue {
    fn zone_value(&self, id: AssetId) -> &ZoneDocument;
}

impl ZoneValue for DocumentSet {
    fn zone_value(&self, id: AssetId) -> &ZoneDocument {
        self.get::<ZoneDocument>(id).and_then(|document| document.value()).expect("a zone that reads")
    }
}

/// The journal keeps what a set step left unsaved of each document it touched, and each comes back to the byte
/// with its undo walking back to its file. The set step itself isn't kept: each comes back with its own steps.
#[test]
fn the_journal_brings_back_each_document_a_set_step_touched() {
    let dir = TestDir::new("journal");
    let (set, moves, cue) = two_with_a_set_step(&dir);
    let journal_at = dir.0.join("journal");
    let mut journal = Journal::open(&journal_at).expect("a session");
    assert!(journal.write(&set).is_empty());
    // Stopped as a kill would stop it.
    drop(journal);

    let next = Journal::open(&journal_at).expect("the next session");
    assert_eq!(next.offers().len(), 2, "{:?}", next.unreadable());
    let mut again = dir.set();
    for (id, original) in [(moves, read(PLAYER)), (cue, read(CUE))] {
        let path = set.path(id).expect("open").to_owned();
        let entry = next.offer(&path).expect("offered by its path").entry.clone();
        let noticed = Kind::of(&path).expect("a kind").recover(&mut again, &entry).expect("taken");
        assert!(noticed.is_empty(), "{noticed:?}");
        assert_eq!(text(&again, id), text(&set, id), "byte for byte");
        let document = again.any_mut(id).expect("open");
        assert_eq!(document.undo_label(), Some("both"));
        while document.undo() {}
        assert_eq!(document.text(), original, "undo walks back to the file");
    }
    next.close().expect("closed");
}
