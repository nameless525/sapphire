//! Packs cut into parts (M6 K2): where the cut falls, and a cook's packs in parts read back, mounted and packed
//! again.

use std::collections::BTreeSet;

use sapphire_assets::pack::{PACK_OVERHEAD, write_part};

use super::*;

/// Bytes that don't compress, from `seed`.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn stored(id: AssetId, kind: AssetKind, len: usize, seed: u64) -> PackAsset {
    PackAsset::encode(id, kind, Codec::None, &noise(len, seed), Vec::new(), ZSTD_LEVEL).unwrap()
}

/// A small zone: 24 textures of 3 levels and a tail each, and 40 meshes, about 13 MiB in all. With `edit`, the
/// tenth texture's top level has other bytes of the same length.
fn district(edit: bool) -> Vec<PackAsset> {
    let mut assets = Vec::new();
    for texture in 0..24u64 {
        let id = AssetId::from_bytes(*blake3::hash(&texture.to_le_bytes()).as_bytes().first_chunk().unwrap());
        assets.push(stored(id, AssetKind::Texture, 20_000, texture));
        for level in 0..3 {
            let seed = 1000 + texture * 10 + u64::from(level) + u64::from(edit && texture == 9 && level == 0);
            assets.push(stored(
                TextureLevel::id(id, level),
                AssetKind::TextureLevel,
                65_536 << (2 - level),
                seed,
            ));
        }
    }
    for mesh in 0..40u64 {
        let id =
            AssetId::from_bytes(*blake3::hash(&(mesh + 100).to_le_bytes()).as_bytes().first_chunk().unwrap());
        assets.push(stored(id, AssetKind::Mesh, 30_000 + mesh as usize * 997, 5000 + mesh));
    }
    assets.sort_unstable_by_key(|asset| asset.id);
    assets
}

/// Each part of `assets` cut at `cap`, laid out.
fn written(assets: &[PackAsset], cap: u64) -> Vec<Vec<u8>> {
    let Cut { parts, stamp } = cut(assets.iter(), cap).unwrap();
    let count = parts.len() as u32;
    (0..count)
        .zip(parts)
        .map(|(part, members)| {
            write_part(members.iter().map(|&index| assets[index].clone()).collect(), part, count, stamp)
                .unwrap()
        })
        .collect()
}

#[test]
fn a_pack_that_fits_is_one_part() {
    let assets = district(false);
    let total: u64 = assets.iter().map(PackAsset::packed_len).sum::<u64>() + PACK_OVERHEAD;
    let whole = cut(assets.iter(), total).unwrap();
    assert_eq!((whole.parts, whole.stamp), (vec![(0..assets.len()).collect::<Vec<_>>()], [0; 8]));
    let cut = cut(assets.iter(), total - 1).unwrap();
    assert!(cut.parts.len() > 1 && cut.stamp != [0; 8]);
}

/// Every asset is in one part, a texture's levels with it, and every part's file fits the cap.
#[test]
fn a_cut_never_splits_a_texture_from_its_levels_and_each_part_fits() {
    let assets = district(false);
    let cap = 1 << 20;
    let parts = cut(assets.iter(), cap).unwrap().parts;
    assert!(parts.len() >= 13, "about 13 MiB in parts of at most 1 MiB: {}", parts.len());
    let mut part_of = HashMap::new();
    for (part, members) in parts.iter().enumerate() {
        for &index in members {
            assert!(part_of.insert(assets[index].id, part).is_none(), "asset {index} is in two parts");
        }
    }
    assert_eq!(part_of.len(), assets.len(), "every asset is in a part");
    for texture in assets.iter().filter(|asset| asset.kind == AssetKind::Texture) {
        for level in 0..3 {
            assert_eq!(
                part_of[&TextureLevel::id(texture.id, level)],
                part_of[&texture.id],
                "a level with its texture"
            );
        }
    }
    for (part, bytes) in written(&assets, cap).iter().enumerate() {
        assert!(bytes.len() as u64 <= cap, "part {part} is {} bytes", bytes.len());
        let header = *PackReader::new(bytes).unwrap().toc().header();
        assert_eq!((header.part, header.parts), (part as u32, parts.len() as u32));
    }
}

/// What a patch to one texture touches: one part's bytes, every other part's the same.
#[test]
fn a_change_to_one_texture_changes_one_part() {
    let cap = 1 << 20;
    let (before, after) = (written(&district(false), cap), written(&district(true), cap));
    assert_eq!(before.len(), after.len());
    let changed: Vec<usize> = (0..before.len()).filter(|&part| before[part] != after[part]).collect();
    assert_eq!(changed.len(), 1, "parts {changed:?} changed");
    // The same cut again makes the same bytes.
    assert_eq!(written(&district(true), cap), after);
}

/// A unit too large to share a part is a part alone, after the others.
#[test]
fn a_unit_over_the_cap_is_a_part_alone() {
    let mut assets = district(false);
    let large = AssetId::from_bytes([0; 16]);
    assets.push(stored(large, AssetKind::Texture, 2 << 20, 7));
    assets.push(stored(TextureLevel::id(large, 0), AssetKind::TextureLevel, 1 << 20, 8));
    assets.sort_unstable_by_key(|asset| asset.id);
    let with_it = cut(assets.iter(), 1 << 20).unwrap();
    let last: BTreeSet<AssetId> =
        with_it.parts.last().unwrap().iter().map(|&index| assets[index].id).collect();
    assert_eq!(last, BTreeSet::from([large, TextureLevel::id(large, 0)]));
    // A unit alone added changes the count and the stamp, so every part's header: a patch installs them all.
    let without = cut(district(false).iter(), 1 << 20).unwrap();
    assert_eq!(with_it.parts.len(), without.parts.len() + 1);
    assert_ne!(with_it.stamp, without.stamp);
}

/// A bucket emptied is an empty part: the count, the stamp and every other part's bytes stay, so a patch that
/// removes a bucket's assets installs that part alone; with too many units for the cap there's no cut.
#[test]
fn an_emptied_bucket_is_an_empty_part_and_moves_no_other() {
    let cap = 1 << 20;
    let assets = district(false);
    let before = cut(assets.iter(), cap).unwrap();
    let emptied = before.parts.iter().position(|members| !members.is_empty()).unwrap();
    let gone: BTreeSet<AssetId> = before.parts[emptied].iter().map(|&index| assets[index].id).collect();
    let fewer: Vec<PackAsset> = assets.iter().filter(|asset| !gone.contains(&asset.id)).cloned().collect();
    let after = cut(fewer.iter(), cap).unwrap();
    assert_eq!((after.parts.len(), after.stamp), (before.parts.len(), before.stamp));
    assert!(after.parts[emptied].is_empty());
    let (old, new) = (written(&assets, cap), written(&fewer, cap));
    let changed: Vec<usize> = (0..old.len()).filter(|&part| old[part] != new[part]).collect();
    assert_eq!(changed, [emptied], "only the emptied part changes");
    let empty = PackReader::new(&new[emptied]).unwrap();
    assert!(empty.toc().entries().is_empty() && empty.toc().header().parts == old.len() as u32);

    // Every unit over the cap is a part alone: more of them than a pack has parts is no cut.
    let tiny = PACK_OVERHEAD + 1;
    let many: Vec<PackAsset> = (0..=format::MAX_PARTS as u64)
        .map(|n| {
            stored(
                AssetId::from_bytes(*blake3::hash(&n.to_le_bytes()).as_bytes().first_chunk().unwrap()),
                AssetKind::Mesh,
                1,
                n,
            )
        })
        .collect();
    assert!(matches!(cut(many.iter(), tiny), Err(format::PackError::Uncuttable { cap }) if cap == tiny));
}

/// A cook's pack over the cap: parts named `shared`, `shared.1`, …, mounted as one pack by its name, every
/// asset read back as cooked, and packing again, cached or not, writes the same bytes; an uncapped pack after
/// it is one part again, and the other parts' files go.
#[test]
fn a_cooks_pack_in_parts_reads_back_mounts_and_packs_again_to_the_same_bytes() {
    let scratch = Scratch::new("pack_parts");
    let report = scratch.cook(Cook::Desktop);
    let out = scratch.packs(Cook::Desktop);
    let cache = scratch.cache();
    let whole = pack_cooked(&report, &out, Some(&cache)).unwrap();
    let cap = whole.packs[0].bytes - 1;
    let packed = pack_cooked_in_parts(&report, &out, Some(&cache), cap).unwrap();
    let names: Vec<&str> = packed.packs.iter().map(|pack| pack.name.as_str()).collect();
    let expected: Vec<String> = (0..names.len() as u32).map(|part| format::part_name(SHARED, part)).collect();
    assert!(names.len() > 1 && names == expected, "{names:?}");

    let source = Arc::new(MemoryPackSource::new());
    let mut found = 0;
    for pack in &packed.packs {
        let bytes = fs::read(&pack.path).unwrap();
        assert_eq!(pack.bytes, bytes.len() as u64);
        let reader = PackReader::new(&bytes).unwrap();
        assert!(
            pack.bytes <= cap || reader.toc().entries().len() == 1,
            "{} holds {} bytes",
            pack.name,
            pack.bytes
        );
        for entry in reader.toc().entries() {
            let cooked = report.assets.iter().find(|asset| asset.id == entry.id).unwrap();
            assert_eq!(
                reader.read(entry.id).unwrap().as_slice(),
                fs::read(&cooked.output).unwrap(),
                "as cooked"
            );
            found += 1;
        }
        source.insert(&pack.name, bytes);
    }
    assert_eq!(found, report.assets.len(), "every asset, once");
    let mut server = AssetServer::new(source, AssetBudgets::DESKTOP);
    server.mount_now(SHARED).unwrap();
    let mesh: Handle<Mesh> = server.load(id(MANNEQUIN)).unwrap();
    server.finish(&mesh, |_| Ok(())).unwrap();

    let files = |report: &PackReport| -> Vec<Vec<u8>> {
        report.packs.iter().map(|pack| fs::read(&pack.path).unwrap()).collect()
    };
    let first = files(&packed);
    let again = pack_cooked_in_parts(&report, &out, Some(&cache), cap).unwrap();
    assert!(again.packs.iter().all(|pack| pack.status == PackStatus::UpToDate), "{again:?}");
    let uncached = pack_cooked_in_parts(&report, &out, None, cap).unwrap();
    assert_eq!(files(&uncached), first, "the same bytes, cached or not");

    let whole_again = pack_cooked(&report, &out, Some(&cache)).unwrap();
    assert_eq!(whole_again.packs.len(), 1);
    assert_eq!(whole_again.removed.len(), first.len() - 1, "the other parts' files go");
}
