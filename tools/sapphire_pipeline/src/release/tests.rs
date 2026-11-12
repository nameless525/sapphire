use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::{AssetId, AssetKind};

use super::*;
use crate::documents::ship_documents;
use crate::test_util::TempDir;

const TARGETING: &str = include_str!("../../../../crates/sapphire_gameplay/default_targeting.ron");
const HUD: &str = include_str!("../../../../crates/sapphire_ui/default_hud.ron");
/// The shipped list of documents, which says which the simulation reads.
const MANIFEST_FILE_PATH: &str = "documents/game.documents.ron";

/// A key from a fixed seed: tests never make one from entropy, and none is kept in a file.
fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public(seed: u8) -> PublicKey {
    key(seed).verifying_key().to_bytes()
}

fn pack(seed: u8) -> Vec<u8> {
    let bytes: Vec<u8> = (0..70_000u32).map(|index| (index / 3) as u8 ^ seed).collect();
    let asset = PackAsset::encode(
        AssetId::from_bytes([seed; 16]),
        AssetKind::Texture,
        Codec::Zstd,
        &bytes,
        vec![],
        3,
    )
    .unwrap();
    write_pack(vec![asset]).unwrap()
}

/// A cook's directory as `cook --packs` leaves it: two packs, the notices, and the game's documents, the
/// targeting (the simulation's) and the HUD (presentation).
struct Cooked {
    dir: TempDir,
}

impl Cooked {
    fn new(name: &str) -> Self {
        let cooked = Self { dir: TempDir::new(name) };
        let source = cooked.dir.path().join("content");
        fs::create_dir_all(source.join("tuning")).unwrap();
        fs::write(source.join("tuning/targeting.ron"), TARGETING).unwrap();
        fs::write(source.join("hud.ron"), HUD).unwrap();
        fs::write(
            source.join(MANIFEST_FILE),
            "(type: \"sapphire::assets::GameDocuments\", version: 1, \
             data: (targeting: Some(\"tuning/targeting.ron\"), hud: Some(\"hud.ron\")))",
        )
        .unwrap();
        fs::create_dir_all(cooked.packs()).unwrap();
        fs::write(cooked.packs().join("shared.spack"), pack(1)).unwrap();
        fs::write(cooked.packs().join("zone_town.spack"), pack(2)).unwrap();
        fs::write(cooked.packs().join(NOTICES), "the fonts' licenses").unwrap();
        ship_documents(&source, &cooked.packs()).unwrap();
        write_manifest(&cooked.packs(), Cook::Desktop).unwrap();
        cooked
    }

    fn packs(&self) -> PathBuf {
        self.dir.path().join("packs")
    }

    fn out(&self) -> PathBuf {
        self.dir.path().join("host")
    }
}

fn options(version: u64) -> ReleaseOptions {
    ReleaseOptions {
        version,
        expires: date("2027-06-01").unwrap(),
        min_app_build: 1200,
        successor_keys: None,
        patch_from: None,
    }
}

#[test]
fn a_cook_names_everything_it_ships_and_what_a_patch_may_change() {
    let cooked = Cooked::new("release_names");
    let written = fs::read(cooked.packs().join(manifest::FILE_NAME)).unwrap();
    let manifest = Manifest::read(&written).unwrap();
    assert_eq!(manifest, manifest_of(&cooked.packs(), Cook::Desktop).unwrap());
    let packs: Vec<&str> = manifest.packs.iter().map(|pack| pack.name.as_str()).collect();
    assert_eq!(packs, ["shared", "zone_town"]);
    assert_eq!(manifest.pack("shared").unwrap().size, pack(1).len() as u64);
    let files: Vec<(&str, bool)> =
        manifest.files.iter().map(|file| (file.path.as_str(), file.patchable)).collect();
    assert_eq!(
        files,
        [
            ("NOTICES.txt", true),
            ("documents/game.documents.ron", false),
            ("documents/hud.ron", true),
            ("documents/tuning/targeting.ron", false),
        ]
    );
    assert_eq!(manifest.file("documents/hud.ron").unwrap().hash, *blake3::hash(HUD.as_bytes()).as_bytes());
    assert!(manifest.successor_keys.is_empty() && manifest.version == 0, "a cook's manifest is no release");
    assert_eq!(manifest.cook, "desktop");
    assert_eq!(write_manifest(&cooked.packs(), Cook::Desktop).unwrap(), None, "written only when it changes");

    // Nothing ships unnamed: a stray file beside the packs, or under documents/, stops the cook.
    fs::write(cooked.packs().join("notes.txt"), "mine").unwrap();
    assert!(matches!(write_manifest(&cooked.packs(), Cook::Desktop), Err(ReleaseError::Unknown { .. })));
    fs::remove_file(cooked.packs().join("notes.txt")).unwrap();
    fs::write(cooked.packs().join("documents/extra.ron"), "()").unwrap();
    assert!(matches!(write_manifest(&cooked.packs(), Cook::Desktop), Err(ReleaseError::Unknown { .. })));
    fs::remove_file(cooked.packs().join("documents/extra.ron")).unwrap();
    // Nor does a pack that isn't one.
    fs::write(cooked.packs().join("broken.spack"), "not a pack").unwrap();
    assert!(matches!(write_manifest(&cooked.packs(), Cook::Desktop), Err(ReleaseError::Pack { .. })));
}

#[test]
fn a_release_is_signed_and_laid_out_by_hash() {
    let cooked = Cooked::new("release_layout");
    let out = cooked.out();
    let published = publish(&cooked.packs(), &out, &options(5), &[key(7)]).unwrap();
    assert_eq!((published.written, published.kept), (6, 0), "two packs and four files");
    let signed = fs::read(&published.current).unwrap();
    let release = Manifest::verify(&signed, &[public(7)]).expect("signed by the key it was given");
    assert_eq!(release, published.manifest);
    assert_eq!(
        (release.version, release.min_app_build, release.successor_keys.clone()),
        (5, 1200, vec![public(7)])
    );
    assert_eq!(fs::read(out.join("manifests/5.smanifest")).unwrap(), signed);
    let cooked_manifest =
        Manifest::read(&fs::read(cooked.packs().join(manifest::FILE_NAME)).unwrap()).unwrap();
    assert_eq!((&release.packs, &release.files), (&cooked_manifest.packs, &cooked_manifest.files));
    for pack in &release.packs {
        let hosted = out.join("packs").join(format!("{}.spack", hex(&pack.header_hash)));
        assert_eq!(
            fs::read(hosted).unwrap(),
            fs::read(cooked.packs().join(format!("{}.spack", pack.name))).unwrap()
        );
    }
    for file in &release.files {
        assert_eq!(
            fs::read(out.join("files").join(hex(&file.hash))).unwrap(),
            fs::read(cooked.packs().join(&file.path)).unwrap()
        );
    }

    // A host's release only moves forward, and its hashed files are never rewritten.
    assert!(matches!(
        publish(&cooked.packs(), &out, &options(5), &[key(7)]),
        Err(ReleaseError::NotNewer { existing: 5, version: 5, .. })
    ));
    let again = publish(&cooked.packs(), &out, &options(6), &[key(7)]).unwrap();
    assert_eq!((again.written, again.kept), (0, 6), "the same content, a new version");
    assert!(out.join("manifests/5.smanifest").is_file() && out.join("manifests/6.smanifest").is_file());
    let hosted = out.join("packs").join(format!("{}.spack", hex(&release.packs[0].header_hash)));
    fs::write(&hosted, "not what its name says").unwrap();
    assert!(matches!(
        publish(&cooked.packs(), &out, &options(7), &[key(7)]),
        Err(ReleaseError::Collision { .. })
    ));

    // A pack changed after its cook's manifest isn't what that cook released.
    fs::write(cooked.packs().join("zone_town.spack"), pack(3)).unwrap();
    assert!(matches!(
        publish(&cooked.packs(), &cooked.dir.path().join("other"), &options(1), &[key(7)]),
        Err(ReleaseError::Stale { .. })
    ));
}

/// The exit test's hostile manifest, on a published one: every byte flipped, the file cut at every length,
/// and each refused by a client that trusts its key.
#[test]
fn every_flipped_byte_of_a_published_manifest_is_refused() {
    let cooked = Cooked::new("release_hostile");
    let published = publish(&cooked.packs(), &cooked.out(), &options(1), &[key(7)]).unwrap();
    let bytes = fs::read(&published.current).unwrap();
    for at in 0..bytes.len() {
        let mut hostile = bytes.clone();
        hostile[at] ^= 0x5a;
        assert!(Manifest::verify(&hostile, &[public(7)]).is_err(), "byte {at} passed");
    }
    for len in 0..bytes.len() {
        assert!(Manifest::verify(&bytes[..len], &[public(7)]).is_err(), "cut at {len} passed");
    }
}

/// A rotation's release: signed by the old key and the new, naming the new one for what comes after.
#[test]
fn a_release_can_hand_its_successor_to_a_new_key() {
    let cooked = Cooked::new("release_rotation");
    let rotating = ReleaseOptions { successor_keys: Some(vec![public(8)]), ..options(2) };
    let published = publish(&cooked.packs(), &cooked.out(), &rotating, &[key(7), key(8)]).unwrap();
    let signed = fs::read(&published.current).unwrap();
    assert!(
        Manifest::verify(&signed, &[public(7)]).is_ok() && Manifest::verify(&signed, &[public(8)]).is_ok()
    );
    assert_eq!(published.manifest.successor_keys, [public(8)]);
    assert!(matches!(publish(&cooked.packs(), &cooked.out(), &options(3), &[]), Err(ReleaseError::NoKey)));
}

/// `--patch-from`: a patch carries presentation only, and must be what an install of the release it patches
/// takes. Each refusal names what's wrong and writes nothing.
#[test]
fn a_patch_carries_only_what_a_patch_may_and_names_what_it_may_not() {
    let cooked = Cooked::new("release_patch");
    let out = cooked.out();
    publish(&cooked.packs(), &out, &options(1), &[key(7)]).unwrap();
    let patch = |version: u64| ReleaseOptions {
        patch_from: Some(out.join("manifests/1.smanifest")),
        ..options(version)
    };
    let recook = |path: &str, text: &str, cook: Cook| {
        fs::write(cooked.packs().join(path), text).unwrap();
        write_manifest(&cooked.packs(), cook).unwrap();
    };

    // The simulation's tuning, changed: refused, naming it, with nothing published.
    recook("documents/tuning/targeting.ron", &format!("{TARGETING}\n// retuned\n"), Cook::Desktop);
    let refused = publish(&cooked.packs(), &out, &patch(2), &[key(7)]).unwrap_err();
    assert!(
        matches!(&refused, ReleaseError::NotPatchable { path, from: 1 } if path == "documents/tuning/targeting.ron"),
        "{refused}"
    );
    assert!(refused.to_string().starts_with("documents/tuning/targeting.ron isn't release 1's"), "{refused}");
    assert!(!out.join("manifests/2.smanifest").exists(), "a refused patch publishes nothing");
    // The list of what the simulation reads is the simulation's too.
    recook("documents/tuning/targeting.ron", TARGETING, Cook::Desktop);
    let list = fs::read_to_string(cooked.packs().join(MANIFEST_FILE_PATH)).unwrap();
    recook(MANIFEST_FILE_PATH, &format!("{list}\n"), Cook::Desktop);
    assert!(matches!(
        publish(&cooked.packs(), &out, &patch(2), &[key(7)]),
        Err(ReleaseError::NotPatchable { path, .. }) if path == MANIFEST_FILE_PATH
    ));
    recook(MANIFEST_FILE_PATH, &list, Cook::Desktop);

    // Not something an install of release 1 would take.
    recook("documents/hud.ron", &format!("{HUD}\n// a new HUD\n"), Cook::Desktop);
    assert!(matches!(
        publish(&cooked.packs(), &out, &patch(2), &[key(8)]),
        Err(ReleaseError::PatchNotTrusted { from: 1 })
    ));
    assert!(matches!(
        publish(&cooked.packs(), &out, &patch(1), &[key(7)]),
        Err(ReleaseError::PatchNotNewer { from: 1, version: 1 })
    ));
    write_manifest(&cooked.packs(), Cook::Mobile).unwrap();
    assert!(matches!(
        publish(&cooked.packs(), &cooked.dir.path().join("mobile"), &patch(2), &[key(7)]),
        Err(ReleaseError::PatchOtherCook { from: 1, .. })
    ));
    write_manifest(&cooked.packs(), Cook::Desktop).unwrap();

    // Presentation changed, by a key release 1 trusts: a patch.
    let published = publish(&cooked.packs(), &out, &patch(2), &[key(7)]).unwrap();
    assert_eq!(published.manifest.version, 2);
    let first = Manifest::read(&fs::read(out.join("manifests/1.smanifest")).unwrap()).unwrap();
    let signed = fs::read(&published.current).unwrap();
    assert_eq!(first.accept_successor(&signed, 1_800_000_000, 1200), Ok(published.manifest));
}

#[test]
fn keys_come_from_the_environment_and_are_never_echoed() {
    assert!(matches!(keys_from_env(None), Err(ReleaseError::NoKey)));
    assert!(matches!(keys_from_env(Some("  ")), Err(ReleaseError::NoKey)));
    let seed = "07".repeat(32);
    let keys = keys_from_env(Some(&format!("{seed}, {}", "08".repeat(32)))).unwrap();
    assert_eq!(
        keys.iter().map(|key| key.verifying_key().to_bytes()).collect::<Vec<_>>(),
        [public(7), public(8)]
    );
    let secret = format!("{}zz", "ab".repeat(31));
    let refused = keys_from_env(Some(&format!("{seed},{secret}"))).unwrap_err();
    assert!(matches!(refused, ReleaseError::BadKey { index: 1 }));
    assert!(!refused.to_string().contains("abab"), "{refused}");
    assert!(keys_from_env(Some(&format!("+{}", "0".repeat(63)))).is_err());

    assert_eq!(public_keys(&hex(&public(8))).unwrap(), [public(8)]);
    assert!(matches!(public_keys("00"), Err(ReleaseError::BadSuccessorKey { index: 0 })));
}

/// M4 X4: keys from fixed seeds only, in a directory of the test's own.
#[test]
fn a_key_file_is_written_once_and_read_back_as_its_public_half() {
    let dir = TempDir::new("release_key_file");
    let path = dir.path().join("release.key");
    assert_eq!(write_key_file(&path, &key(9)).unwrap(), public(9));
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text, format!("{}\n", "09".repeat(32)), "the seed as SAPPHIRE_RELEASE_KEY takes it");
    assert_eq!(keys_from_env(Some(&text)).unwrap()[0].verifying_key().to_bytes(), public(9));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "its owner's alone");
    }
    assert_eq!(public_key_of_file(&path).unwrap(), public(9));
    assert_eq!(public_key_hex(&public(9)), hex(&public(9)));
    assert_eq!(public_keys(&public_key_hex(&public(9))).unwrap(), [public(9)], "what --next-keys takes");

    // Never over a key that's there, a new one's or another; a new one refuses before it makes a seed.
    assert!(matches!(write_key_file(&path, &key(10)), Err(ReleaseError::KeyFileExists { .. })));
    assert!(matches!(new_key_file(&path), Err(ReleaseError::KeyFileExists { .. })));
    assert_eq!(public_key_of_file(&path).unwrap(), public(9), "the file is as it was");

    // A file that isn't a key says nothing of what it holds.
    let wrong = dir.path().join("wrong.key");
    fs::write(&wrong, format!("{}zz\n", "cd".repeat(31))).unwrap();
    let refused = public_key_of_file(&wrong).unwrap_err();
    assert!(matches!(refused, ReleaseError::BadKeyFile { .. }));
    assert!(!refused.to_string().contains("cdcd"), "{refused}");
    fs::write(&wrong, [0xff, 0xfe, 0x00]).unwrap();
    assert!(matches!(public_key_of_file(&wrong), Err(ReleaseError::BadKeyFile { .. })));
    assert!(matches!(public_key_of_file(&dir.path().join("none.key")), Err(ReleaseError::Io { .. })));
}

#[test]
fn dates_are_utc_midnights() {
    assert_eq!(date("1970-01-01").unwrap(), 0);
    assert_eq!(date("2000-03-01").unwrap(), 951_868_800);
    assert_eq!(date("2024-02-29").unwrap(), 1_709_164_800);
    assert_eq!(date("2027-06-01").unwrap(), 1_811_808_000);
    for bad in [
        "2023-02-29",
        "2027-13-01",
        "2027-00-10",
        "2027-5-01",
        "27-05-01",
        "2027-05-01T00",
        "1969-12-31",
        "2027+05-01",
    ] {
        assert!(matches!(date(bad), Err(ReleaseError::BadDate(_))), "{bad}");
    }
}
