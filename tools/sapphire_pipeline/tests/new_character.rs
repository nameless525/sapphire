//! `new-character` (M6 C5) on the two characters an artist's would be like: C3's test character, a glTF the
//! Blender add-on exported, and the look-dev VRM, pixiv's VRoid Studio sample. Each scaffolds into a scratch
//! folder that a locked cook takes on both cooks with no hand edit (every sidecar there, every reference
//! whole), `check` judges at the role its preset casts, and a second run changes no byte.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, fs, process};

use sapphire_pipeline::check::{self, Rule};
use sapphire_pipeline::cook::{self, Cook, CookOptions};
use sapphire_pipeline::meta::IdSource;
use sapphire_pipeline::scaffold::{self, NewCharacter, Part};

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("sapphire_new_character_{name}_{}_{unique}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
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

/// Every file under `dir` with its bytes, by path relative to it.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(next) = dirs.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                found.insert(path.strip_prefix(dir).unwrap().to_owned(), fs::read(&path).unwrap());
            }
        }
    }
    found
}

/// Scaffolds `model` into `tree/character/`, runs it again, and cooks the tree, locked, on both cooks.
fn scaffold_and_cook(model: &Path, tree: &TestDir, out: &TestDir) -> scaffold::Scaffold {
    let request = NewCharacter {
        model: model.to_owned(),
        out: Some(tree.path().join("character")),
        ..NewCharacter::default()
    };
    let scaffold = scaffold::new_character(&request, &mut IdSource::seeded(3)).unwrap();
    let written = files(tree.path());
    let again = scaffold::new_character(&request, &mut IdSource::seeded(4)).unwrap();
    assert!(again.written.is_empty(), "a second run wrote {:?}", again.written);
    assert_eq!(files(tree.path()), written, "a second run changes no byte");

    for cook in [Cook::Desktop, Cook::Mobile] {
        let options = CookOptions { cook, cache: None, locked: true, editor: false };
        let report = cook::cook_dir(tree.path(), &out.path().join(cook.name()), &options)
            .unwrap_or_else(|error| panic!("the {} cook: {error:?}", cook.name()));
        assert!(!report.assets.is_empty());
    }
    assert_eq!(files(tree.path()), written, "a locked cook made no sidecar: the scaffold made them all");
    scaffold
}

#[test]
fn the_test_character_scaffolds_cooks_on_both_cooks_and_passes_its_check() {
    let (tree, out) = (TestDir::new("test_character"), TestDir::new("test_character_out"));
    let model = repository("tools/sapphire_pipeline/fixtures/character/test_character.glb");
    let scaffold = scaffold_and_cook(&model, &tree, &out);
    assert_eq!(scaffold.parts.len(), 4);

    let report = check::check(tree.path()).unwrap();
    assert!(report.passes(), "{:#?}", report.failures().collect::<Vec<_>>());
    let source = "character/test_character.glb";
    assert_eq!(report.of(source, Rule::UnboundSlot).count(), 0, "every slot bound");
}

#[test]
fn the_lookdev_vrm_scaffolds_cooks_on_both_cooks_and_checks_as_its_waivers_say() {
    let (tree, out) = (TestDir::new("lookdev"), TestDir::new("lookdev_out"));
    let scaffold = scaffold_and_cook(&repository("content/lookdev/character/character.vrm"), &tree, &out);
    let parts: BTreeMap<&str, Part> =
        scaffold.parts.iter().map(|(slot, part)| (slot.as_str(), *part)).collect();
    assert_eq!(parts["Face_00_SKIN"], Part::Face);
    assert_eq!((parts["Hair_00_HAIR"], parts["HairBack_00_HAIR"]), (Part::Hair, Part::Hair));
    assert_eq!((parts["Body_00_SKIN"], parts["Tops_01_CLOTH"]), (Part::Skin, Part::Cloth));
    for eye in [
        "EyeIris_00_EYE",
        "EyeWhite_00_EYE",
        "EyeHighlight_00_EYE",
        "FaceBrow_00_FACE",
        "FaceEyeline_00_FACE",
    ] {
        assert_eq!(parts[eye], Part::Eye, "{eye}");
    }
    assert_eq!(parts.values().filter(|&&part| part == Part::Face).count(), 1, "one face");

    // The shipped sidecar waives the hero row's slots (13 of 6) and mobile joints (155 of 140), with its
    // reasons: a new one has no waiver, so those, and only those, fail.
    let report = check::check(tree.path()).unwrap();
    let failed: BTreeSet<Rule> = report.failures().map(|finding| finding.rule).collect();
    assert_eq!(
        failed,
        BTreeSet::from([Rule::Joints, Rule::Slots]),
        "{:#?}",
        report.failures().collect::<Vec<_>>()
    );
}
