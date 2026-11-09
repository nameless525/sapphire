//! The editor's sidecars and the cooker's are one thing: `sapphire_document` writes the bytes a cook would, a
//! cook keeps the ID it wrote, and both refuse the same files. Neither crate may depend on the other's
//! insides, so this is what holds them together.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, fs, process};

use sapphire_document::identity::{self, sidecar_path};
use sapphire_document::{DocumentSet, IdentityError, SetError, kinds, registry_of};
use sapphire_moves::MoveSetDocument;
use sapphire_pipeline::meta::{self, AssetMeta, ImportSettings, MetaError, SourceTree};
use sapphire_serialize::to_ron;

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("sapphire_sidecars_{name}_{}_{unique}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("tools/<crate>").join(relative)
}

/// The cooker's view of one file.
fn resolve(root: &Path, locked: bool) -> Result<meta::Resolved, MetaError> {
    meta::resolve(&SourceTree::scan(root)?, locked)
}

#[test]
fn a_sidecar_from_either_side_is_the_same_bytes() {
    let id = meta::new_id().expect("an ID");
    for (name, importer) in [
        ("a.moves.ron", "MoveSet(())"),
        ("a.animgraph.ron", "AnimGraph(())"),
        ("a.cue.ron", "Cue(())"),
        ("a.ui.ron", "Ui(())"),
    ] {
        let settings = ImportSettings::for_new_sidecar(Path::new(name)).expect("a source");
        let cooker = to_ron(&AssetMeta::new(id, settings)).expect("writes");
        assert_eq!(identity::sidecar_text(id, Some(importer)), cooker, "{name}");
        assert_eq!(identity::parse_id(&cooker).expect("reads"), id);
    }
}

#[test]
fn the_kinds_say_the_importer_a_cook_would_give_them() {
    for (name, importer) in [
        ("a.moves.ron", kinds::moves().importer),
        ("a.animgraph.ron", kinds::graph().importer),
        ("a.cue.ron", kinds::cue().importer),
        ("a.ui.ron", kinds::ui().importer),
        ("a.timeline.ron", kinds::timeline().importer),
        ("a.zone.ron", kinds::zone().importer),
    ] {
        let id = meta::new_id().expect("an ID");
        let settings = ImportSettings::for_new_sidecar(Path::new(name)).expect("a source");
        let cooker = to_ron(&AssetMeta::new(id, settings)).expect("writes");
        assert_eq!(identity::sidecar_text(id, importer), cooker, "{name}");
    }
}

#[test]
fn a_cook_keeps_the_id_the_editor_gave_locked_or_not() {
    let dir = TestDir::new("keeps");
    let source = fs::read_to_string(repository("content/combat/player/player.moves.ron")).expect("reads");
    let path = dir.0.join("a.moves.ron");
    fs::write(&path, &source).expect("written");
    let mut set = DocumentSet::new(vec![dir.0.clone()]);
    let id = set.open(&path, kinds::moves(), Arc::new(registry_of::<MoveSetDocument>())).expect("opens");
    let sidecar = fs::read(sidecar_path(&path)).expect("reads");

    for locked in [true, false] {
        let resolved = resolve(&dir.0, locked).expect("resolves");
        assert_eq!(resolved.assets[0].meta.id, id, "locked: {locked}");
        assert_eq!(resolved.created, Vec::<PathBuf>::new());
    }
    assert_eq!(fs::read(sidecar_path(&path)).expect("reads"), sidecar, "and leaves it as it is");
}

#[test]
fn the_editor_reads_the_id_a_cook_gave() {
    let dir = TestDir::new("reads");
    let source = fs::read_to_string(repository("content/combat/player/player.moves.ron")).expect("reads");
    let path = dir.0.join("a.moves.ron");
    fs::write(&path, &source).expect("written");
    let cooked = resolve(&dir.0, false).expect("resolves").assets[0].meta.id;
    let mut set = DocumentSet::new(vec![dir.0.clone()]);
    let id = set.open(&path, kinds::moves(), Arc::new(registry_of::<MoveSetDocument>())).expect("opens");
    assert_eq!(id, cooked);
}

#[test]
fn both_refuse_two_claims_on_one_id_and_a_move_without_its_sidecar() {
    let dir = TestDir::new("refuses");
    let source = fs::read_to_string(repository("content/combat/player/player.moves.ron")).expect("reads");
    let (a, b) = (dir.0.join("a.moves.ron"), dir.0.join("b.moves.ron"));
    fs::write(&a, &source).expect("written");
    resolve(&dir.0, false).expect("resolves");

    fs::write(&b, &source).expect("written");
    fs::copy(sidecar_path(&a), sidecar_path(&b)).expect("copied");
    assert!(matches!(resolve(&dir.0, false), Err(MetaError::DuplicateId { .. })));
    let mut set = DocumentSet::new(vec![dir.0.clone()]);
    let registry = Arc::new(registry_of::<MoveSetDocument>());
    let refused = set.open(&a, kinds::moves(), registry.clone()).expect_err("two claims");
    assert!(matches!(refused, SetError::Identity(IdentityError::Duplicate { .. })), "{refused}");
    fs::remove_file(sidecar_path(&b)).expect("deleted");
    fs::remove_file(&b).expect("deleted");

    fs::rename(&a, &b).expect("moved");
    assert!(matches!(resolve(&dir.0, false), Err(MetaError::Moved { .. })));
    let refused = set.open(&b, kinds::moves(), registry).expect_err("a move");
    assert!(matches!(refused, SetError::Identity(IdentityError::Moved { .. })), "{refused}");
}

#[test]
fn a_cook_leaves_a_document_with_no_importer_alone() {
    let dir = TestDir::new("alone");
    let path = dir.0.join("effects.ron");
    fs::copy(repository("crates/sapphire_vfx/default_effects.ron"), &path).expect("copied");
    let sidecar = sidecar_path(&path);
    fs::copy(repository("crates/sapphire_vfx/default_effects.ron.meta"), &sidecar).expect("copied");
    let before = fs::read(&sidecar).expect("reads");
    for locked in [true, false] {
        let resolved = resolve(&dir.0, locked).expect("resolves");
        assert_eq!(resolved.assets, Vec::new());
        assert_eq!(resolved.orphaned, Vec::<PathBuf>::new());
    }
    assert_eq!(fs::read(&sidecar).expect("reads"), before);
}
