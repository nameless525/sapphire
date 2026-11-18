//! Cook on save (M6 C4): `cargo xtask content --watch` runs this cook again on each saved source, so a save costs
//! what the change reaches. Here a scratch tree of committed sources, the Blender add-on's test character among
//! them, is cooked and packed, the character exported again with one material changed, and the tree cooked
//! again: the report names the character's assets alone as cooked (its mesh and skeleton), and its pack alone
//! is written again, with only the mesh compressed anew.

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_pipeline::cook::{Cook, CookOptions, CookReport, CookStatus, cook_dir};
use sapphire_pipeline::pack::{self, PackReport, PackStatus};

/// The test character, as the add-on's export writes it (`tools/blender/test_character.py`), and its sidecar.
const CHARACTER: &str = "fixtures/character/test_character.glb";

/// Committed sources beside it in the scratch tree: a texture, and a material document that binds it.
const NEIGHBOURS: [&str; 3] = [
    "samples/environment/ground.png",
    "samples/environment/ground.material.ron",
    "samples/environment/mist.png",
];

fn crate_file(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn content(path: &str) -> PathBuf {
    crate_file("../../content").join(path)
}

/// Copies `from` and its sidecar into `dir`.
fn copy_with_sidecar(from: &Path, dir: &Path) {
    let name = from.file_name().expect("a file");
    fs::copy(from, dir.join(name)).expect("copies the source");
    let sidecar = format!("{}.meta", name.to_str().expect("a name"));
    fs::copy(from.with_file_name(&sidecar), dir.join(&sidecar)).expect("copies its sidecar");
}

/// A cook of the tree, then its packs, through the scratch cache, as `cargo xtask content` runs it.
struct Scratch {
    source: PathBuf,
    out: PathBuf,
    packs: PathBuf,
    cache: PathBuf,
}

impl Scratch {
    fn cook(&self) -> (CookReport, PackReport) {
        let options =
            CookOptions { cook: Cook::Desktop, cache: Some(&self.cache), locked: true, editor: false };
        let report = cook_dir(&self.source, &self.out, &options).expect("cooks");
        let packed = pack::pack_cooked(&report, &self.packs, Some(&self.cache)).expect("packs");
        (report, packed)
    }
}

#[test]
fn a_saved_character_recooks_alone_and_rewrites_only_its_pack() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cook_on_save");
    let _ = fs::remove_dir_all(&root);
    let scratch = Scratch {
        source: root.join("content"),
        out: root.join("loose"),
        packs: root.join("packs"),
        cache: root.join("cache"),
    };
    let characters = scratch.source.join("characters");
    fs::create_dir_all(&characters).expect("a scratch tree");
    copy_with_sidecar(&crate_file(CHARACTER), &characters);
    let environment = scratch.source.join("environment");
    fs::create_dir_all(&environment).expect("a scratch tree");
    for neighbour in NEIGHBOURS {
        copy_with_sidecar(&content(neighbour), &environment);
    }

    let (first, packed) = scratch.cook();
    assert_eq!(first.count(CookStatus::Cooked), first.assets.len(), "a cold cook cooks everything");
    assert!(first.assets.len() >= 4, "the character and its neighbours: {:?}", first.assets);
    let before: Vec<(String, Vec<u8>)> =
        packed.packs.iter().map(|pack| (pack.name.clone(), fs::read(&pack.path).expect("a pack"))).collect();

    // Saved again, unchanged: nothing cooks, nothing is written.
    let (unchanged, packed) = scratch.cook();
    assert_eq!(unchanged.count(CookStatus::UpToDate), unchanged.assets.len());
    assert!(packed.packs.iter().all(|pack| pack.status == PackStatus::UpToDate), "{packed:?}");

    // Exported again with its skin's shade a little darker: the same file, one value changed.
    let glb = characters.join("test_character.glb");
    let mut bytes = fs::read(&glb).expect("the character");
    let (was, now) = (&b"\"shade_color\":[0.85,"[..], &b"\"shade_color\":[0.75,"[..]);
    let at: Vec<usize> =
        bytes.windows(was.len()).enumerate().filter(|(_, w)| *w == was).map(|(at, _)| at).collect();
    assert_eq!(at.len(), 1, "the skin's shade color, once in the file's JSON");
    bytes[at[0]..at[0] + now.len()].copy_from_slice(now);
    fs::write(&glb, &bytes).expect("saves the character");

    let (saved, packed) = scratch.cook();
    let cooked: Vec<(&Path, CookStatus)> = saved
        .assets
        .iter()
        .filter(|asset| asset.status != CookStatus::UpToDate)
        .map(|asset| (asset.output.strip_prefix(&scratch.out).expect("in the cook"), asset.status))
        .collect();
    assert!(
        saved.assets.iter().all(|asset| asset.status == CookStatus::UpToDate || asset.source == glb),
        "the report names the character alone: {cooked:?}"
    );
    // Its mesh, and its skeleton, which the same source's bytes key.
    let character = [
        (Path::new("characters/test_character.smesh"), CookStatus::Cooked),
        (Path::new("characters/test_character.sskel"), CookStatus::Cooked),
    ];
    assert_eq!(cooked, character, "the character's assets cooked anew, not taken from the cache");

    let rewritten: Vec<&str> = packed
        .packs
        .iter()
        .filter(|pack| {
            let old = before.iter().find(|(name, _)| *name == pack.name).map(|(_, bytes)| bytes);
            old != Some(&fs::read(&pack.path).expect("a pack"))
        })
        .map(|pack| pack.name.as_str())
        .collect();
    let home = packed.packs.iter().find(|pack| rewritten.contains(&pack.name.as_str()));
    assert_eq!(rewritten.len(), 1, "one pack written again: {packed:?}");
    let home = home.expect("the character's pack");
    // The skeleton cooked to the bytes it had, so the packer takes its compression from the cache.
    assert_eq!((home.status, home.encoded), (PackStatus::Packed, 1), "the mesh alone compressed anew");
    for pack in packed.packs.iter().filter(|pack| pack.name != home.name) {
        assert_eq!(pack.status, PackStatus::UpToDate, "{} is as it was", pack.name);
    }
    let _ = fs::remove_dir_all(&root);
}
