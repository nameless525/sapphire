//! A game's own documents through a real cook of a tree of its own: the manifest names them, they come out
//! beside the packs byte for byte, and whatever's wrong with one fails by name before anything ships.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_assets::documents::{GameDocuments, MANIFEST_FILE, SHIPPED_DIR};
use sapphire_gameplay::targeting::TargetingTuning;
use sapphire_pipeline::cook::{Cook, CookOptions, cook_dir};
use sapphire_pipeline::documents::{DocumentsError, ship_documents};

const TARGETING: &str = include_str!("../../../crates/sapphire_gameplay/default_targeting.ron");
const HUD: &str = include_str!("../../../crates/sapphire_ui/default_hud.ron");

/// A directory of its own, gone with the test.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire-game-documents-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("content")).expect("a tree");
        Self(dir)
    }

    fn source(&self) -> PathBuf {
        self.0.join("content")
    }

    fn packs(&self) -> PathBuf {
        self.0.join("packs")
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.source().join(path);
        fs::create_dir_all(path.parent().expect("under the tree")).expect("its directory");
        fs::write(path, text).expect("written");
    }

    fn manifest(&self, body: &str) {
        let text = format!("(type: \"sapphire::assets::GameDocuments\", version: 1, data: ({body}))");
        self.write(MANIFEST_FILE, &text);
    }

    fn shipped(&self, path: &str) -> PathBuf {
        self.packs().join(SHIPPED_DIR).join(path)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn what_a_manifest_names_ships_byte_for_byte_and_only_what_changed_is_written() {
    let tree = Tree::new("ships");
    let own = TARGETING.replacen("range: 12.0", "range: 20.0", 1).replacen(
        "keep_range: 16.0",
        "keep_range: 24.0",
        1,
    );
    tree.write("tuning/my_targeting.ron", &own);
    tree.write("my_hud.ron", HUD);
    tree.manifest("targeting: Some(\"tuning/my_targeting.ron\"), hud: Some(\"my_hud.ron\")");

    // A real cook takes the tree with these in it: they're no assets, and need no sidecars, locked or not.
    let options = CookOptions { cook: Cook::Desktop, cache: None, locked: true, editor: false };
    cook_dir(&tree.source(), &tree.0.join("loose"), &options).expect("a cook ignores them");

    let shipped = ship_documents(&tree.source(), &tree.packs()).expect("ships");
    assert_eq!(shipped.written.len(), 3, "the manifest and what it names: {:?}", shipped.written);
    assert_eq!(read(&tree.shipped("tuning/my_targeting.ron")), own);
    assert_eq!(read(&tree.shipped("my_hud.ron")), HUD);
    let manifest = GameDocuments::from_ron(&read(&tree.shipped(MANIFEST_FILE))).expect("reads");
    assert_eq!(manifest.named().count(), 2);
    let weights = TargetingTuning::from_ron(&read(&tree.shipped("tuning/my_targeting.ron"))).expect("reads");
    assert_eq!(weights.soft_lock.range, 20.0);

    let again = ship_documents(&tree.source(), &tree.packs()).expect("ships");
    assert!(again.written.is_empty() && again.removed.is_empty(), "{again:?}");
}

/// No manifest is a game on Sapphire's numbers, said out loud, so the game's start can tell that from content
/// that was never cooked this way. And what a manifest stops naming goes.
#[test]
fn with_no_manifest_an_empty_one_ships_and_what_was_named_goes() {
    let tree = Tree::new("empty");
    tree.write("my_hud.ron", HUD);
    tree.manifest("hud: Some(\"my_hud.ron\")");
    ship_documents(&tree.source(), &tree.packs()).expect("ships");
    assert!(tree.shipped("my_hud.ron").is_file());

    fs::remove_file(tree.source().join(MANIFEST_FILE)).expect("gone");
    let shipped = ship_documents(&tree.source(), &tree.packs()).expect("ships");
    assert_eq!(shipped.removed, [tree.shipped("my_hud.ron")]);
    let manifest = GameDocuments::from_ron(&read(&tree.shipped(MANIFEST_FILE))).expect("reads");
    assert_eq!(manifest, GameDocuments::default());
}

#[test]
fn whats_wrong_with_a_named_document_fails_by_name_and_ships_nothing() {
    let refused = |name: &str, setup: &dyn Fn(&Tree)| {
        let tree = Tree::new(name);
        setup(&tree);
        let error = ship_documents(&tree.source(), &tree.packs()).expect_err(name);
        assert!(!tree.shipped(MANIFEST_FILE).exists(), "{name} ships nothing");
        (error.to_string(), error)
    };

    let (text, error) = refused("missing", &|tree| tree.manifest("hud: Some(\"nope/my_hud.ron\")"));
    assert!(matches!(error, DocumentsError::Missing { .. }), "{text}");
    assert!(text.contains("`hud`") && text.contains("nope/my_hud.ron"), "{text}");

    let (text, error) = refused("kind", &|tree| {
        tree.write("my_hud.ron", TARGETING);
        tree.manifest("hud: Some(\"my_hud.ron\")");
    });
    assert!(matches!(error, DocumentsError::WrongKind { .. }), "{text}");
    assert!(text.contains("TargetingTuning") && text.contains("HudDesign"), "{text}");

    let (text, error) = refused("garbled", &|tree| {
        tree.write("my_hud.ron", "not a document");
        tree.manifest("hud: Some(\"my_hud.ron\")");
    });
    assert!(matches!(error, DocumentsError::NotADocument { .. }), "{text}");

    let (text, error) = refused("capital", &|tree| {
        tree.write("My_Hud.ron", HUD);
        tree.manifest("hud: Some(\"My_Hud.ron\")");
    });
    assert!(matches!(error, DocumentsError::BadPath { .. }) && text.contains("lowercase"), "{text}");

    let (text, error) = refused("escape", &|tree| tree.manifest("hud: Some(\"../my_hud.ron\")"));
    assert!(matches!(error, DocumentsError::BadPath { .. }), "{text}");

    let (text, error) = refused("typo", &|tree| tree.manifest("huds: Some(\"my_hud.ron\")"));
    assert!(matches!(error, DocumentsError::Manifest(_)) && text.contains("huds"), "{text}");
}
