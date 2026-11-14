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
        ("cast/ambient_a_00", "403a0cd7ff30c8e7d2d84eb7298d00dd96d84715801b6df9704e02d111a1ad4c"),
        ("cast/ambient_b_00", "a8fc041b56341744077c5846701d8b134ade32a74b08e2cb78072bbbb41294e7"),
        ("cast/enemy_00", "a4d4bf649b5df5e3e778fac27194fa8062a9e8905919a6bb630453de7c7f47d4"),
        ("cast/hero_00", "b627dd928a11da43905f0bd5dffeb539eb5f5165be37e9b8329c9f2d879df764"),
        ("district", "3ad95515a63dc940ace97b511b8b2d5285f5b67e1d61fa41f48ffe95f8dbe482"),
        ("district/buildings", "4ba81538425fc6d90cb7a002164f7b9b40b0a76b3323f104ace974afb131d6e4"),
        ("district/materials", "57aaa0fc00432b725fe541e79abfa9e0b89db350aad58b9d6045af8e407685b0"),
        ("district/prefabs", "fd7f488cd15839048f1ecd7c2dc0f05d588fed511ddaedc1270d2e4b4923476c"),
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
    assert_eq!((count(Preset::Ci, ".vrm"), count(Preset::Ci, ".glb")), (1, 3));
    assert_eq!((count(Preset::Budget, ".vrm"), count(Preset::Budget, ".glb")), (3, 3 + 6));

    // Every source but the control maps, the face's threshold masks and the district's day has a
    // sidecar, every sidecar's ID is its own, and the zone reaches every material.
    let day = format!("district/{}", district::DAY);
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
        } else if path != MARKER
            && !path.ends_with("_control.png")
            && !path.contains("_threshold_")
            && *path != day
        {
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
    assert!(!out.join("cast/hero_01").exists(), "an emptied folder goes too");
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
    assert_eq!("double".parse::<Preset>().unwrap(), Preset::Double);
    assert!(matches!("huge".parse::<Preset>(), Err(BenchError::Preset(_))));
}
