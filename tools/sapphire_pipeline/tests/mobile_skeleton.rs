//! The mobile skeleton cut (M6 C8, `sapphire_pipeline::skeleton`) through a whole cook, on the two shipped
//! characters whose sidecars cap their joints: the look-dev character (155 joints) and Aster (200). Each
//! cooks alone in a scratch tree, with its materials unbound so no texture cooks: the cut touches only
//! the mesh, the skeleton and the rig. The desktop cook writes the same bytes with the cap as without
//! it, and the mobile cook keeps the phones' 140, as the runtime reads them.

use std::collections::BTreeMap;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::{env, fs, process};

use sapphire_assets::mesh::{Mesh, VertexInfluence};
use sapphire_assets::rig::{CharacterRig, SpringTip};
use sapphire_assets::skeleton::Skeleton;
use sapphire_pipeline::cook::{self, Cook, CookOptions};
use sapphire_pipeline::meta::{self, ImportSettings, sidecar_path};
use sapphire_pipeline::skeleton::JointCap;
use sapphire_serialize::cooked::AlignedVec;

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("sapphire_mobile_skeleton_{name}_{}_{unique}", process::id()));
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

/// The shipped character at `relative` in a tree of its own, its sidecar's settings with `cap`, cooked
/// locked on `cook`: every file the cook wrote.
fn cook_alone(relative: &str, cap: JointCap, cook: Cook) -> BTreeMap<PathBuf, Vec<u8>> {
    let source = repository("content").join(relative);
    let mut sidecar = meta::read(&sidecar_path(&source)).unwrap();
    let ImportSettings::Vrm(settings) = &mut sidecar.importer else { panic!("{relative} is a VRM") };
    settings.mesh.materials.clear();
    settings.mesh.merge.clear();
    settings.joints = cap;
    sidecar.budget = None;
    let tree = TestDir::new("tree");
    let model = tree.0.join("character.vrm");
    fs::copy(&source, &model).unwrap();
    fs::write(sidecar_path(&model), sapphire_serialize::to_ron(&sidecar).unwrap()).unwrap();
    let out = TestDir::new("out");
    let options = CookOptions { cook, cache: None, locked: true, editor: false };
    cook::cook_dir(&tree.0, &out.0, &options).unwrap_or_else(|error| panic!("{relative}, {cook}: {error:?}"));
    files(&out.0)
}

fn aligned(bytes: &[u8]) -> AlignedVec {
    let mut aligned = AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

/// The one cooked file with `extension`.
fn cooked<'a>(files: &'a BTreeMap<PathBuf, Vec<u8>>, extension: &str) -> &'a [u8] {
    let mut found = files.iter().filter(|(path, _)| path.extension().is_some_and(|e| e == extension));
    let (_, bytes) = found.next().unwrap_or_else(|| panic!("a .{extension}"));
    assert!(found.next().is_none(), "one .{extension}");
    bytes
}

#[test]
fn the_capped_characters_cook_the_desktop_as_before_and_the_phones_at_their_row() {
    let cap = JointCap { desktop: None, mobile: NonZeroU16::new(140) };
    for (relative, joints) in [("lookdev/character/character.vrm", 155), ("combat/hero/hero.vrm", 200)] {
        // What the cut could touch: the reports beside them name keys, which the cap is in.
        let drawn = |files: BTreeMap<PathBuf, Vec<u8>>| -> BTreeMap<PathBuf, Vec<u8>> {
            let cut = |path: &Path| {
                matches!(path.extension().and_then(|e| e.to_str()), Some("smesh" | "sskel" | "srig"))
            };
            files.into_iter().filter(|(path, _)| cut(path)).collect()
        };
        let desktop = drawn(cook_alone(relative, cap, Cook::Desktop));
        assert_eq!(desktop.len(), 3, "{relative}: {:?}", desktop.keys());
        assert_eq!(
            desktop,
            drawn(cook_alone(relative, JointCap::NONE, Cook::Desktop)),
            "{relative}: desktop bytes"
        );
        let skeleton = Skeleton::from_cooked(&aligned(cooked(&desktop, "sskel"))).unwrap();
        assert_eq!(skeleton.names.len(), joints, "{relative}: the desktop keeps every joint");

        let mobile = cook_alone(relative, cap, Cook::Mobile);
        let skeleton = Skeleton::from_cooked(&aligned(cooked(&mobile, "sskel"))).unwrap();
        let mesh = Mesh::from_cooked(&aligned(cooked(&mobile, "smesh"))).unwrap();
        let rig = CharacterRig::from_cooked(&aligned(cooked(&mobile, "srig"))).unwrap();
        let skin = mesh.skin.as_ref().expect("skinned");
        assert_eq!((skeleton.names.len(), skin.joint_count, rig.joint_count), (140, 140, 140), "{relative}");
        // Every kept vertex's cooked weights are whole (255 is 1), on palette entries the mesh has.
        for vertex in 0..skin.influences.len() {
            let VertexInfluence { binds, weights } = skin.influences.get(vertex).unwrap();
            assert_eq!(
                weights.iter().map(|&w| u32::from(w)).sum::<u32>(),
                255,
                "{relative}: vertex {vertex}"
            );
            assert!(binds.iter().all(|&bind| usize::from(bind) < skin.binds.len()));
        }
        assert!(rig.springs.iter().any(|spring| matches!(spring.tip, SpringTip::Point { .. })), "{relative}");
    }
}
