use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::pack::{Codec, PackAsset, PackHeader, write_pack};
use crate::{AssetId, AssetKind};

/// A key from a fixed seed: no key is ever made from entropy or kept in a file.
fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public(seed: u8) -> PublicKey {
    key(seed).verifying_key().to_bytes()
}

/// A small real pack, its bytes depending on `seed`.
fn pack_bytes(seed: u8) -> Vec<u8> {
    let bytes: Vec<u8> = (0..3000u32).map(|index| (index / 5) as u8 ^ seed).collect();
    let asset =
        PackAsset::encode(AssetId::from_bytes([seed; 16]), AssetKind::Mesh, Codec::Zstd, &bytes, vec![], 3)
            .expect("encodes");
    write_pack(vec![asset]).expect("writes")
}

fn released(name: &str, bytes: &[u8]) -> ReleasedPack {
    let header = PackHeader::read(bytes).expect("a pack");
    ReleasedPack { name: name.to_owned(), size: bytes.len() as u64, header_hash: header.hash() }
}

fn file(path: &str, text: &str, patchable: bool) -> ReleasedFile {
    ReleasedFile {
        path: path.to_owned(),
        size: text.len() as u64,
        hash: *blake3::hash(text.as_bytes()).as_bytes(),
        patchable,
    }
}

/// A release shaped like the game's: its packs' names, real pack headers, and the documents beside them.
fn release(version: u64) -> Manifest {
    let names = ["audio", "fonts", "loc", "shared", "ui", "zone_lantern_ward", "zone_town"];
    Manifest {
        version,
        expires: 2_000_000_000,
        min_app_build: 1200,
        cook: "desktop".to_owned(),
        successor_keys: vec![public(1)],
        packs: names
            .iter()
            .enumerate()
            .map(|(index, name)| released(name, &pack_bytes(index as u8 + 1)))
            .collect(),
        files: vec![
            file("NOTICES.txt", "the fonts' licenses", true),
            file("documents/flags.ron", "flags", false),
            file("documents/game.documents.ron", "the list", false),
            file("documents/game.flow.ron", "the flow", false),
            file("documents/ui/menus.ron", "menus", true),
        ],
    }
}

fn signed(manifest: &Manifest, keys: &[&SigningKey]) -> Vec<u8> {
    let body = manifest.body().expect("a valid manifest");
    let signatures: Vec<ManifestSignature> = keys
        .iter()
        .map(|key| ManifestSignature {
            key: key.verifying_key().to_bytes(),
            signature: key.sign(&body).to_bytes(),
        })
        .collect();
    manifest.encode(&signatures).expect("encodes")
}

#[test]
fn manifests_round_trip_and_encode_one_way() {
    let manifest = release(7);
    let unsigned = manifest.encode(&[]).unwrap();
    assert_eq!(Manifest::read(&unsigned).unwrap(), manifest);
    assert_eq!(unsigned, [manifest.body().unwrap(), vec![0]].concat(), "unsigned: the body and a zero");
    let bytes = signed(&manifest, &[&key(1)]);
    assert_eq!(Manifest::read(&bytes).unwrap(), manifest);
    assert_eq!(Manifest::verify(&bytes, &[public(1)]).unwrap(), manifest);
    assert_eq!(bytes[..unsigned.len() - 1], unsigned[..unsigned.len() - 1], "one body, signed or not");
    assert_eq!(manifest.pack("shared").map(|pack| pack.name.as_str()), Some("shared"));
    assert!(manifest.pack("zone_forest").is_none());
    assert!(!manifest.file("documents/game.flow.ron").unwrap().patchable);
    assert!(manifest.file("documents/ui/menus.ron").unwrap().patchable);
}

/// What P0 estimated (about 128 bytes and 72 a pack) against what the layout costs, held so a change to it is
/// a change someone meant.
#[test]
fn a_manifest_costs_what_its_docs_say() {
    let manifest = release(1);
    let bytes = signed(&manifest, &[&key(1)]);
    let names: usize = manifest.packs.iter().map(|pack| 41 + pack.name.len()).sum();
    let paths: usize = manifest.files.iter().map(|file| 42 + file.path.len()).sum();
    assert_eq!(bytes.len(), 165 + manifest.cook.len() + names + paths);
}

#[test]
fn only_a_trusted_key_makes_a_manifest_verify() {
    let manifest = release(3);
    let unsigned = manifest.encode(&[]).unwrap();
    assert_eq!(Manifest::verify(&unsigned, &[public(1)]), Err(ManifestError::Untrusted));
    let bytes = signed(&manifest, &[&key(2)]);
    assert_eq!(Manifest::verify(&bytes, &[public(1)]), Err(ManifestError::Untrusted));
    assert_eq!(Manifest::verify(&bytes, &[]), Err(ManifestError::Untrusted));
    assert!(Manifest::verify(&bytes, &[public(1), public(2)]).is_ok());

    // A signature by a trusted key over another body is refused, whoever else signed.
    let other = signed(&release(4), &[&key(2)]);
    let forged =
        [manifest.body().unwrap(), vec![1], public(2).to_vec(), other[other.len() - 64..].to_vec()].concat();
    assert_eq!(
        Manifest::verify(&forged, &[public(2)]),
        Err(ManifestError::BadSignature { key: KeyId::of(&public(2)) })
    );
}

#[test]
fn a_malleable_signature_is_refused() {
    let manifest = release(3);
    let mut bytes = signed(&manifest, &[&key(1)]);
    // s + ℓ, where ℓ is the group's order: the same signature to a lax verifier.
    const L: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let s = bytes.len() - 32;
    let mut carry = 0u16;
    for (byte, add) in bytes[s..].iter_mut().zip(L) {
        let sum = u16::from(*byte) + u16::from(add) + carry;
        *byte = sum as u8;
        carry = sum >> 8;
    }
    assert!(Manifest::verify(&bytes, &[public(1)]).is_err());
}

/// The exit test's hostile manifest: every byte of a signed release flipped, three ways, and the file cut at
/// every length. Each is refused cleanly, by the signature if not before it, and an unchecked read never
/// panics either.
#[test]
fn every_flipped_byte_and_every_truncation_is_refused() {
    let manifest = release(9);
    let bytes = signed(&manifest, &[&key(1)]);
    let trusted = [public(1)];
    for at in 0..bytes.len() {
        for flip in [0x01, 0x80, 0xff] {
            let mut hostile = bytes.clone();
            hostile[at] ^= flip;
            assert!(Manifest::verify(&hostile, &trusted).is_err(), "byte {at} ^ {flip:#x} passed");
            let _ = Manifest::read(&hostile);
        }
    }
    for len in 0..bytes.len() {
        assert!(Manifest::verify(&bytes[..len], &trusted).is_err(), "cut at {len} passed");
        assert!(Manifest::read(&bytes[..len]).is_err(), "cut at {len} read");
    }
    let longer = [bytes.clone(), vec![0]].concat();
    assert_eq!(Manifest::read(&longer), Err(ManifestError::TrailingBytes(1)));
    assert_eq!(Manifest::read(&vec![0; MAX_LEN + 1]), Err(ManifestError::TooLong(MAX_LEN + 1)));
}

/// Fields no signature would catch on their own: each one's rule holds on read, and on write.
#[test]
fn what_a_manifest_names_is_held_to_its_rules() {
    let invalid = |change: &dyn Fn(&mut Manifest)| {
        let mut manifest = release(1);
        change(&mut manifest);
        let written = manifest.body();
        assert!(matches!(written, Err(ManifestError::Invalid { .. })), "{manifest:?} wrote: {written:?}");
    };
    for path in [
        "../escape",
        "documents/../../x",
        "/abs",
        "documents//x",
        ".hidden",
        "a\\b",
        "doc s",
        "x.spack",
        FILE_NAME,
    ] {
        invalid(&|manifest| manifest.files[0].path = path.to_owned());
    }
    invalid(&|manifest| manifest.files.push(file("notices.txt", "", true)));
    invalid(&|manifest| manifest.files.swap(1, 2));
    invalid(&|manifest| manifest.files[0].size = MAX_FILE_SIZE + 1);
    for name in ["", "Shared", "zone/town", "..", "a.b", &"x".repeat(MAX_NAME_LEN + 1)] {
        invalid(&|manifest| manifest.packs[0].name = name.to_owned());
    }
    invalid(&|manifest| manifest.packs.swap(0, 1));
    invalid(&|manifest| manifest.packs[1].name = manifest.packs[0].name.clone());
    invalid(&|manifest| manifest.packs[0].size = 255);
    invalid(&|manifest| manifest.packs[0].size = MAX_PACK_SIZE + 1);
    invalid(&|manifest| manifest.cook = "Desktop".to_owned());
    invalid(&|manifest| manifest.cook = String::new());
    invalid(&|manifest| manifest.successor_keys = vec![public(1), public(1)]);
    invalid(&|manifest| manifest.successor_keys = (1..=5).map(public).collect());
    // Not a point on the curve.
    let off_curve =
        (0..=255).map(|byte| [byte; 32]).find(|key| VerifyingKey::from_bytes(key).is_err()).unwrap();
    invalid(&|manifest| manifest.successor_keys = vec![off_curve]);

    // Bytes no field reads: the reserved pair and a file's other flags.
    let bytes = release(1).encode(&[]).unwrap();
    let mut reserved = bytes.clone();
    reserved[34] = 1;
    assert!(matches!(Manifest::read(&reserved), Err(ManifestError::Invalid { part: "header", .. })));
    let notices = bytes.windows(11).position(|window| window == b"NOTICES.txt").unwrap();
    let mut flags = bytes;
    flags[notices - 2] |= 2;
    assert!(matches!(
        Manifest::read(&flags),
        Err(ManifestError::Invalid { part: "file", reason: "unknown flags", .. })
    ));

    let twice = ManifestSignature { key: public(1), signature: [0; 64] };
    assert!(matches!(
        release(1).encode(&[twice, twice]),
        Err(ManifestError::Invalid { part: "signature", .. })
    ));
}

#[test]
fn a_successor_is_newer_current_for_this_app_and_this_cook() {
    let installed = release(10);
    let now = 1_900_000_000;
    let offer = |change: &dyn Fn(&mut Manifest)| {
        let mut next = release(11);
        change(&mut next);
        installed.accept_successor(&signed(&next, &[&key(1)]), now, 1200)
    };
    assert_eq!(offer(&|_| {}), Ok(release(11)));
    assert_eq!(offer(&|next| next.version = 10), Err(ManifestError::NotNewer { installed: 10, offered: 10 }));
    assert_eq!(offer(&|next| next.version = 3), Err(ManifestError::NotNewer { installed: 10, offered: 3 }));
    assert_eq!(
        offer(&|next| next.expires = now),
        Err(ManifestError::Expired { version: 11, expires: now, now })
    );
    assert_eq!(
        offer(&|next| next.min_app_build = 1201),
        Err(ManifestError::AppTooOld { version: 11, needs: 1201, build: 1200 })
    );
    assert!(matches!(offer(&|next| next.cook = "mobile".to_owned()), Err(ManifestError::OtherCook { .. })));
    let by_another = signed(&release(11), &[&key(2)]);
    assert_eq!(installed.accept_successor(&by_another, now, 1200), Err(ManifestError::Untrusted));

    // A cook's own manifest names no keys: nothing succeeds it but an app build.
    let cooked = Manifest { successor_keys: vec![], ..release(0) };
    assert_eq!(
        cooked.accept_successor(&signed(&release(11), &[&key(1)]), now, 1200),
        Err(ManifestError::Untrusted)
    );
}

/// What the simulation reads stays as the app build shipped it: a patch may change what's marked patchable,
/// and nothing else, and the first file it gets wrong is named.
#[test]
fn a_patch_may_not_add_change_drop_or_unmark_what_the_simulation_reads() {
    let base = release(10);
    let patch = |change: &dyn Fn(&mut Vec<ReleasedFile>)| {
        let mut next = release(11);
        change(&mut next.files);
        next
    };
    let changed = |next: &Manifest| base.unpatchable_change(next).map(str::to_owned);
    let replace = |path: &'static str, by: ReleasedFile| {
        move |files: &mut Vec<ReleasedFile>| {
            let at = files.iter().position(|file| file.path == path).expect("in the release");
            files[at] = by.clone();
        }
    };
    assert_eq!(changed(&patch(&|_| {})), None);
    assert_eq!(
        changed(&patch(&replace("documents/ui/menus.ron", file("documents/ui/menus.ron", "new", true)))),
        None
    );
    assert_eq!(
        changed(&patch(&replace(
            "documents/game.flow.ron",
            file("documents/game.flow.ron", "a new flow", false)
        ))),
        Some("documents/game.flow.ron".to_owned())
    );
    assert_eq!(
        changed(&patch(&replace("documents/flags.ron", file("documents/flags.ron", "flags", true)))),
        Some("documents/flags.ron".to_owned()),
        "unmarked"
    );
    assert_eq!(
        changed(&patch(&|files| files.retain(|file| file.path != "documents/game.flow.ron"))),
        Some("documents/game.flow.ron".to_owned()),
        "dropped"
    );
    let added = patch(&|files| files.insert(1, file("documents/combat/tuning.ron", "tuning", false)));
    assert_eq!(changed(&added), Some("documents/combat/tuning.ron".to_owned()), "added");
    assert_eq!(added.unpatchable_change(&base), Some("documents/combat/tuning.ron"), "either way round");
    // A presentation file made the simulation's is a change to what it reads, too.
    let marked = patch(&replace("documents/ui/menus.ron", file("documents/ui/menus.ron", "menus", false)));
    assert_eq!(changed(&marked), Some("documents/ui/menus.ron".to_owned()));
}

/// Keys rotate one hop at a time: a release signed by the old key and the new names the new one, so a client
/// on the old trust takes it, and from then on only what the new key signs.
#[test]
fn a_signed_successor_rotates_the_keys() {
    let now = 1_900_000_000;
    let installed = release(1);
    let bridge = Manifest { successor_keys: vec![public(2)], ..release(2) };
    let bridge =
        installed.accept_successor(&signed(&bridge, &[&key(1), &key(2)]), now, 1200).expect("bridged");
    let after = signed(&release(3), &[&key(2)]);
    assert_eq!(
        installed.accept_successor(&after, now, 1200),
        Err(ManifestError::Untrusted),
        "two hops at once"
    );
    assert!(bridge.accept_successor(&after, now, 1200).is_ok());
    let by_old = signed(&release(3), &[&key(1)]);
    assert_eq!(
        bridge.accept_successor(&by_old, now, 1200),
        Err(ManifestError::Untrusted),
        "the old key is out"
    );
}
