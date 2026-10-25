use super::*;

const ZSTD_LEVEL: i32 = 19;

fn id(byte: u8) -> AssetId {
    AssetId::from_bytes([byte; 16])
}

/// Deterministic bytes: runs of a repeating pattern (compressible) or xorshift noise.
fn data(len: usize, seed: u32, compressible: bool) -> Vec<u8> {
    let mut state = seed.max(1);
    (0..len)
        .map(|index| {
            if compressible {
                ((index / 7) as u32 ^ seed) as u8
            } else {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            }
        })
        .collect()
}

struct Sample {
    id: AssetId,
    codec: Codec,
    bytes: Vec<u8>,
    dependencies: Vec<AssetId>,
}

fn samples() -> Vec<Sample> {
    let sample = |byte, codec, bytes, dependencies| Sample { id: id(byte), codec, bytes, dependencies };
    vec![
        sample(9, Codec::Zstd, data(3 * BLOCK_SIZE + 1234, 1, true), vec![id(2), id(5)]),
        sample(2, Codec::Lz4, data(BLOCK_SIZE, 2, true), vec![]),
        sample(5, Codec::Zstd, data(1, 3, true), vec![id(2)]),
        sample(7, Codec::None, data(BLOCK_SIZE + 1, 4, true), vec![]),
        // Noise doesn't compress, so its blocks are stored raw.
        sample(3, Codec::Zstd, data(2 * BLOCK_SIZE + 17, 5, false), vec![]),
        sample(4, Codec::Lz4, data(100, 6, false), vec![id(9)]),
    ]
}

fn encode(sample: &Sample) -> PackAsset {
    PackAsset::encode(
        sample.id,
        AssetKind::Mesh,
        sample.codec,
        &sample.bytes,
        sample.dependencies.clone(),
        ZSTD_LEVEL,
    )
    .expect("encode")
}

fn pack() -> Vec<u8> {
    write_pack(samples().iter().map(encode).collect()).expect("write")
}

#[test]
fn packs_round_trip() {
    let bytes = pack();
    let reader = PackReader::new(&bytes).expect("open");
    let toc = reader.toc();
    let ids: Vec<AssetId> = toc.entries().iter().map(|entry| entry.id).collect();
    assert_eq!(ids, [2, 3, 4, 5, 7, 9].map(id), "sorted by ID");
    for sample in samples() {
        let entry = toc.find(sample.id).expect("listed");
        assert_eq!(
            (entry.kind, entry.codec, entry.size),
            (AssetKind::Mesh, sample.codec, sample.bytes.len() as u64)
        );
        assert_eq!(entry.block_count() as usize, sample.bytes.len().div_ceil(BLOCK_SIZE));
        assert_eq!(toc.dependencies(entry), sample.dependencies.as_slice());
        let read = reader.read(sample.id).expect("read");
        assert_eq!(read.as_slice(), sample.bytes.as_slice());
        assert!((read.as_ptr() as usize).is_multiple_of(16), "decoded assets are aligned for rkyv");
        for block in toc.blocks(entry) {
            assert!(block.offset.is_multiple_of(16));
        }
    }
    assert_eq!(reader.read(id(1)).unwrap_err(), PackError::Missing(id(1)));

    // Compressible data shrinks; noise is stored raw rather than "compressed" larger.
    let block_codecs = |byte| -> Vec<Codec> {
        toc.blocks(toc.find(id(byte)).unwrap()).iter().map(|block| block.codec).collect()
    };
    assert_eq!(block_codecs(9), [Codec::Zstd; 4]);
    assert_eq!(block_codecs(2), [Codec::Lz4]);
    assert_eq!(block_codecs(7), [Codec::None; 2]);
    assert_eq!(block_codecs(3), [Codec::None; 3]);
    assert_eq!(block_codecs(4), [Codec::None]);
    let stored: u64 =
        toc.entries().iter().map(|entry| toc.stored_range(entry).end - toc.stored_range(entry).start).sum();
    let size: u64 = toc.entries().iter().map(|entry| entry.size).sum();
    assert!(stored < size, "{stored} stored for {size}");
}

#[test]
fn any_block_decodes_on_its_own() {
    let bytes = pack();
    let reader = PackReader::new(&bytes).expect("open");
    let sample = &samples()[0];
    for index in [3, 0, 2, 1] {
        let block = reader.read_block(sample.id, index).expect("read block");
        let start = index as usize * BLOCK_SIZE;
        assert_eq!(block, sample.bytes[start..(start + BLOCK_SIZE).min(sample.bytes.len())]);
    }
    assert!(matches!(reader.read_block(sample.id, 4), Err(PackError::Invalid { .. })));
}

#[test]
fn packing_is_deterministic() {
    let mut reversed: Vec<PackAsset> = samples().iter().map(encode).collect();
    reversed.reverse();
    assert_eq!(write_pack(reversed).expect("write"), pack(), "input order doesn't matter");
    assert_eq!(encode(&samples()[0]), encode(&samples()[0]));
}

#[test]
fn bad_inputs_are_refused_when_packing() {
    let mut twice: Vec<PackAsset> = samples().iter().map(encode).collect();
    twice.push(encode(&samples()[1]));
    assert_eq!(write_pack(twice).unwrap_err(), PackError::Duplicate(id(2)));
    let encode_bytes = |bytes: &[u8], dependencies| {
        PackAsset::encode(id(1), AssetKind::Texture, Codec::Lz4, bytes, dependencies, 3)
    };
    assert!(matches!(encode_bytes(&[], vec![]), Err(PackError::Invalid { .. })));
    assert!(matches!(encode_bytes(&[1], vec![id(1)]), Err(PackError::Invalid { .. })));
    let mut asset = encode_bytes(&[1, 2, 3], vec![id(3), id(2), id(3)]).expect("encode");
    assert_eq!(asset.dependencies, [id(2), id(3)], "sorted and distinct");
    asset.size = 4;
    assert!(matches!(write_pack(vec![asset]), Err(PackError::Invalid { .. })));
}

/// Recomputes the table of contents' and the header's hashes after an edit, so tests reach
/// the structural checks behind them.
fn rehash(bytes: &mut [u8]) {
    let header = PackHeader::read(bytes).or_else(|_| {
        // The edit may have broken the header hash; fix it first.
        let hash = blake3::hash(&bytes[..HASHED_HEADER_LEN]);
        bytes[HASHED_HEADER_LEN..HASHED_HEADER_LEN + 32].copy_from_slice(hash.as_bytes());
        PackHeader::read(bytes)
    });
    let toc_end = header.map_or(HEADER_LEN, |header| header.data_offset as usize);
    let toc_hash = blake3::hash(&bytes[HEADER_LEN..toc_end]);
    bytes[64..96].copy_from_slice(toc_hash.as_bytes());
    let hash = blake3::hash(&bytes[..HASHED_HEADER_LEN]);
    bytes[HASHED_HEADER_LEN..HASHED_HEADER_LEN + 32].copy_from_slice(hash.as_bytes());
}

/// Opens the pack and reads every asset in it.
fn read_all(bytes: &[u8]) -> Result<(), PackError> {
    let reader = PackReader::new(bytes)?;
    for entry in reader.toc().entries() {
        reader.read(entry.id)?;
    }
    Ok(())
}

#[test]
fn bad_headers_are_rejected() {
    let good = pack();
    let edit = |offset: usize, value: &[u8], fix_hashes: bool| {
        let mut bytes = good.clone();
        bytes[offset..offset + value.len()].copy_from_slice(value);
        if fix_hashes {
            rehash(&mut bytes);
        }
        PackReader::new(&bytes).map(|_| ())
    };
    assert_eq!(edit(0, b"SPAX", false), Err(PackError::NotAPack));
    assert_eq!(edit(4, &2u32.to_le_bytes(), false), Err(PackError::UnsupportedVersion(2)));
    assert_eq!(edit(20, &7u32.to_le_bytes(), false), Err(PackError::HeaderHash));
    assert_eq!(edit(160, &1u32.to_le_bytes(), false), Err(PackError::UnsupportedSignature(1)));
    assert!(matches!(edit(200, &[1], false), Err(PackError::Header(_))), "signature bytes while unsigned");
    // With matching hashes, the fields themselves are checked.
    assert!(matches!(edit(12, &1u32.to_le_bytes(), true), Err(PackError::Header(_))), "flags");
    assert!(matches!(edit(16, &4096u32.to_le_bytes(), true), Err(PackError::Header(_))), "block size");
    assert!(matches!(edit(100, &[1], true), Err(PackError::Header(_))), "reserved");
    assert!(
        matches!(edit(20, &7u32.to_le_bytes(), true), Err(PackError::Header(_))),
        "counts against length"
    );
    assert!(matches!(edit(40, &999u64.to_le_bytes(), true), Err(PackError::Header(_))), "data offset");
    assert!(matches!(edit(48, &1u64.to_le_bytes(), true), Err(PackError::Header(_))), "data length");
    assert!(edit(0, b"SPAK", true).is_ok(), "an edit that changes nothing");
}

#[test]
fn bad_tables_of_contents_are_rejected() {
    let good = pack();
    let entries = HEADER_LEN;
    let header = PackHeader::read(&good).unwrap();
    let blocks = entries + header.entry_count as usize * ENTRY_LEN;
    // Any edit without fixing the hash: the hash catches it.
    for offset in [entries, entries + 17, blocks + 9, good.len() - 1 - header.data_len as usize] {
        let mut bytes = good.clone();
        bytes[offset] ^= 0x40;
        assert_eq!(PackReader::new(&bytes).unwrap_err(), PackError::TocHash, "byte {offset}");
    }
    // With matching hashes, each record is checked.
    let edit = |offset: usize, value: &[u8]| {
        let mut bytes = good.clone();
        bytes[offset..offset + value.len()].copy_from_slice(value);
        rehash(&mut bytes);
        match PackReader::new(&bytes) {
            Err(PackError::Toc { reason, .. }) => reason,
            other => panic!("expected an inconsistent table of contents, got {other:?}"),
        }
    };
    let second = entries + ENTRY_LEN;
    assert!(edit(second, &[0; 16]).contains("sorted"), "unsorted IDs");
    assert!(edit(second, &good[entries..entries + 16]).contains("sorted"), "a repeated ID");
    assert!(edit(entries + 16, &[9]).contains("kind"));
    assert!(edit(entries + 17, &[7]).contains("codec"));
    assert!(edit(entries + 18, &[1]).contains("reserved"));
    assert!(edit(entries + 40, &(5 * BLOCK_SIZE as u64).to_le_bytes()).contains("block count"));
    assert!(edit(entries + 40, &0u64.to_le_bytes()).contains("empty"));
    assert!(edit(entries + 20, &1u32.to_le_bytes()).contains("order"), "first block");
    assert!(edit(blocks, &3u64.to_le_bytes()).contains("aligned"), "block offset");
    assert!(edit(blocks + 12, &[2]).contains("codec"), "an lz4 asset's block claims zstd");
    // Asset 3's first block (the table's second) is noise, stored raw.
    let raw_block = blocks + BLOCK_LEN;
    assert!(
        edit(raw_block + 8, &1u32.to_le_bytes()).contains("stored length"),
        "raw blocks hold their whole length"
    );
    // The entry for asset 9 lists two dependencies; make them unsorted.
    let deps = blocks + header.block_count as usize * BLOCK_LEN;
    let last_two = deps + (header.dependency_count as usize - 2) * DEPENDENCY_LEN;
    let mut swapped = good[last_two + 16..last_two + 32].to_vec();
    swapped.extend_from_slice(&good[last_two..last_two + 16]);
    assert!(edit(last_two, &swapped).contains("dependencies"));
}

#[test]
fn bad_blocks_are_rejected_and_only_their_asset_fails() {
    let good = pack();
    let reader = PackReader::new(&good).unwrap();
    let toc = reader.toc();
    for sample in samples() {
        let entry = toc.find(sample.id).unwrap();
        for (index, block) in (0..).zip(toc.blocks(entry)) {
            let mut bytes = good.clone();
            bytes[(block.offset + u64::from(block.stored_len) / 2) as usize] ^= 1;
            let corrupt = PackReader::new(&bytes).expect("the table of contents is intact");
            assert_eq!(
                corrupt.read(sample.id).unwrap_err(),
                PackError::BlockHash { id: sample.id, block: index }
            );
            assert_eq!(
                corrupt.read_block(sample.id, index).unwrap_err(),
                PackError::BlockHash { id: sample.id, block: index }
            );
            for other in samples().iter().filter(|other| other.id != sample.id) {
                assert_eq!(corrupt.read(other.id).unwrap().as_slice(), other.bytes.as_slice());
            }
        }
    }
}

#[test]
fn truncated_and_extended_files_are_rejected() {
    let good = pack();
    for len in [0, 3, HEADER_LEN - 1, HEADER_LEN, HEADER_LEN + 10, good.len() / 2, good.len() - 1] {
        assert!(
            matches!(PackReader::new(&good[..len]), Err(PackError::Truncated { .. })),
            "{len} of {} bytes",
            good.len()
        );
    }
    let mut longer = good.clone();
    longer.push(0);
    assert!(matches!(PackReader::new(&longer), Err(PackError::Header(_))));
}

#[test]
fn every_byte_that_is_read_is_covered() {
    // A small pack, so every byte can be flipped in turn.
    let assets = [
        (id(1), Codec::Zstd, data(300, 1, true)),
        (id(2), Codec::Lz4, data(200, 2, true)),
        (id(3), Codec::None, data(33, 3, false)),
    ];
    let good = write_pack(
        assets
            .iter()
            .map(|(id, codec, bytes)| {
                PackAsset::encode(*id, AssetKind::Material, *codec, bytes, vec![], 3).unwrap()
            })
            .collect(),
    )
    .unwrap();
    let reader = PackReader::new(&good).unwrap();
    let toc = reader.toc();
    let mut used = vec![false; good.len()];
    used[..toc.header().data_offset as usize].fill(true);
    for entry in toc.entries() {
        for block in toc.blocks(entry) {
            used[block.offset as usize..block.range().end as usize].fill(true);
        }
    }
    for (index, used) in used.iter().enumerate() {
        let mut bytes = good.clone();
        bytes[index] ^= 0x01;
        let result = read_all(&bytes);
        if *used {
            assert!(result.is_err(), "flipping byte {index} went unnoticed");
        } else {
            assert_eq!(result, Ok(()), "byte {index} is alignment padding, never read");
        }
    }
}

#[test]
fn malformed_compressed_blocks_fail_to_decode_without_panicking() {
    // Blocks whose hashes match but whose bytes aren't what their codec says: what a packer bug
    // would write. Decoders must reject them, not panic.
    for codec in [Codec::Lz4, Codec::Zstd] {
        let bytes = data(4000, 7, true);
        let mut asset = PackAsset::encode(id(1), AssetKind::Mesh, codec, &bytes, vec![], 3).unwrap();
        let block = &mut asset.blocks[0];
        assert_eq!(block.codec, codec);
        block.bytes.truncate(block.bytes.len() / 2);
        block.hash = *blake3::hash(&block.bytes).as_bytes();
        let pack = write_pack(vec![asset]).unwrap();
        let reader = PackReader::new(&pack).unwrap();
        assert!(
            matches!(reader.read(id(1)), Err(PackError::Decode { codec: c, .. }) if c == codec),
            "{codec}"
        );
    }
    // An asset whose blocks decode but whose recorded hash is wrong.
    let mut asset =
        PackAsset::encode(id(1), AssetKind::Mesh, Codec::Lz4, &data(500, 1, true), vec![], 3).unwrap();
    asset.hash[0] ^= 1;
    let pack = write_pack(vec![asset]).unwrap();
    assert_eq!(PackReader::new(&pack).unwrap().read(id(1)).unwrap_err(), PackError::AssetHash { id: id(1) });
}
