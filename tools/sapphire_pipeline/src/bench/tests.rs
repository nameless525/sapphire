use std::collections::BTreeMap;

use super::*;
use crate::test_util::TempDir;

/// The repository's `bench/`.
fn bench_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench")
}

/// Every file under `dir`, by its path from `dir`, with its bytes.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                files.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

#[test]
fn generating_twice_writes_the_same_bytes() {
    let dir = TempDir::new("bench_twice");
    for preset in Preset::ALL {
        let (first, second) =
            (dir.path().join(format!("{preset}_a")), dir.path().join(format!("{preset}_b")));
        generate(&bench_dir(), preset, &first).unwrap();
        generate(&bench_dir(), preset, &second).unwrap();
        let (first, second) = (tree(&first), tree(&second));
        assert_eq!(first.keys().collect::<Vec<_>>(), second.keys().collect::<Vec<_>>(), "{preset}");
        for (path, bytes) in &first {
            assert!(second[path] == *bytes, "{preset}: {path} differs");
        }
        // Again over the first: nothing changes.
        let again = generate(&bench_dir(), preset, &dir.path().join(format!("{preset}_a"))).unwrap();
        assert!(again.created.is_empty() && again.replaced.is_empty(), "{preset}: {again:?}");
    }
}

/// The `ci` preset's generated tree, pinned folder by folder: blake3 over each file's path, length
/// and bytes, in path order. Everything the generator does is integer hashes, the four operations,
/// `sqrt`, `floor` and `sapphire_math::det` (the figures' rings, rotations and colours included), so
/// these are the same on every OS and architecture: CI's x86-64 runs fail here if one disagrees, and
/// say which folder. A change to `bench/` or a generator moves them on purpose: update them then.
#[test]
fn the_ci_preset_generates_the_same_bytes_on_every_machine() {
    const PINNED: &[(&str, &str)] = &[
        (".", "cf2cdaca935a3bdde3fe9ab9de1e40c800aa7ada5c7371e8f23155de27b1dd72"),
        ("cast/stand_in_00", "f8aa3907ea259b4cd8718e762b4ebc61f9d654faa7132dee4ee3dbbf271a3c9d"),
        ("district", "32170bcd24dcb31e051544b2b153d8b4049497a2c67e2bc5e847bf62f4cc9c46"),
        ("district/materials", "57aaa0fc00432b725fe541e79abfa9e0b89db350aad58b9d6045af8e407685b0"),
        ("district/prefabs", "3c2c720d914b762aea5b3955d01f0d33e8a3a0abf1392fb0f3ef77949e606ff5"),
        ("district/textures", "3e8613b7430eec9bb2d03163a999abe8ff34be45744a6c7908f0b94d853005c0"),
    ];
    let dir = TempDir::new("bench_pinned");
    generate(&bench_dir(), Preset::Ci, dir.path()).unwrap();
    let mut folders: BTreeMap<String, blake3::Hasher> = BTreeMap::new();
    for (path, bytes) in tree(dir.path()) {
        let folder = path.rsplit_once('/').map_or(".", |(folder, _)| folder).to_owned();
        let hasher = folders.entry(folder).or_default();
        hasher.update(path.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    let found: Vec<(String, String)> = folders
        .into_iter()
        .map(|(folder, hasher)| (folder, hasher.finalize().to_hex().to_string()))
        .collect();
    let pinned: Vec<(String, String)> =
        PINNED.iter().map(|&(folder, hash)| (folder.to_owned(), hash.to_owned())).collect();
    assert!(found == pinned, "the ci preset's folders hash otherwise; found:\n{found:#?}");
}

#[test]
fn a_preset_scales_the_district_and_the_cast() {
    let dir = TempDir::new("bench_presets");
    let count = |preset: Preset, pattern: &str| {
        let out = dir.path().join(preset.name());
        generate(&bench_dir(), preset, &out).unwrap();
        tree(&out).keys().filter(|path| path.ends_with(pattern)).count()
    };
    assert_eq!(count(Preset::Ci, ".ptex.ron"), 4 * 3);
    assert_eq!(count(Preset::Budget, ".ptex.ron"), 160 * 3);
    assert_eq!(count(Preset::Ci, ".glb"), 1);
    assert_eq!(count(Preset::Budget, ".glb"), 2);

    // Every source but the control maps has a sidecar, every sidecar's ID is its own, and the zone
    // reaches every material.
    let files = tree(&dir.path().join("budget"));
    let mut ids = BTreeSet::new();
    for (path, bytes) in &files {
        if let Some(source) = path.strip_suffix(".meta") {
            assert!(files.contains_key(source), "{path} has no source");
            let meta: AssetMeta = sapphire_serialize::from_ron(std::str::from_utf8(bytes).unwrap()).unwrap();
            assert!(ids.insert(meta.id), "{path}'s ID is another's");
            assert_eq!(
                meta.id,
                asset_id(&path[..path.find('/').unwrap()], &source[source.find('/').unwrap() + 1..])
            );
        } else if path != MARKER && !path.ends_with("_control.png") {
            assert!(files.contains_key(&format!("{path}.meta")), "{path} has no sidecar");
        }
    }
    let zone = std::str::from_utf8(&files["district/district.zone.ron"]).unwrap();
    for set in 0..160 {
        let id = asset_id("district", &format!("materials/set_{set:03}.material.ron"));
        assert!(zone.contains(&id.to_string()), "the zone doesn't reach set {set}");
    }
}

#[test]
fn what_an_earlier_run_wrote_goes() {
    let dir = TempDir::new("bench_stale");
    let out = dir.path().join("out");
    generate(&bench_dir(), Preset::Budget, &out).unwrap();
    generate(&bench_dir(), Preset::Ci, &out).unwrap();
    assert!(!out.join("district/textures/set_004.color.ptex.ron").exists());
    assert!(!out.join("cast/stand_in_01").exists(), "an emptied folder goes too");
    let fresh = dir.path().join("fresh");
    generate(&bench_dir(), Preset::Ci, &fresh).unwrap();
    assert_eq!(tree(&out), tree(&fresh));
}

#[test]
fn a_tree_it_didnt_write_is_left_alone() {
    let dir = TempDir::new("bench_foreign");
    fs::write(dir.path().join("keep.txt"), "mine").unwrap();
    assert!(matches!(generate(&bench_dir(), Preset::Ci, dir.path()), Err(BenchError::NotGenerated(_))));
    assert_eq!(fs::read_to_string(dir.path().join("keep.txt")).unwrap(), "mine");
}

#[test]
fn presets_parse_by_name() {
    assert_eq!("ci".parse::<Preset>().unwrap(), Preset::Ci);
    assert_eq!("budget".parse::<Preset>().unwrap(), Preset::Budget);
    assert!(matches!("huge".parse::<Preset>(), Err(BenchError::Preset(_))));
}
