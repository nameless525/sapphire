//! Writing packs. The packer (`sapphire_pipeline::pack`) encodes each asset once
//! ([`PackAsset::encode`]), caches the result, and lays packs out from encoded assets
//! ([`write_pack`]). The writer lives here, next to the reader, so one module owns the format.

use serde::{Deserialize, Serialize};

use super::{
    BLOCK_LEN, BLOCK_SIZE, Codec, DATA_ALIGN, DEPENDENCY_LEN, ENTRY_LEN, HEADER_LEN, MAX_ASSET_SIZE,
    PackError, PackHeader, block_len,
};
use crate::{AssetId, AssetKind};

/// An asset encoded for a pack: its blocks as stored, and what its table of contents entry
/// records. Pack layouts are built from these, so an asset is compressed once however many
/// packs and pack builds it goes into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackAsset {
    pub id: AssetId,
    pub kind: AssetKind,
    pub codec: Codec,
    /// The assets it references: sorted, distinct, and never itself.
    pub dependencies: Vec<AssetId>,
    /// Bytes in the asset, decoded.
    pub size: u64,
    /// blake3 of the decoded asset.
    pub hash: [u8; 32],
    /// One per [`BLOCK_SIZE`] of the asset, in order.
    pub blocks: Vec<StoredBlock>,
}

/// One block of a [`PackAsset`], as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBlock {
    /// The asset's codec, or [`Codec::None`] where compressing didn't pay.
    pub codec: Codec,
    /// blake3 of [`Self::bytes`].
    pub hash: [u8; 32],
    pub bytes: Vec<u8>,
}

impl PackAsset {
    /// Encodes the cooked `bytes` of asset `id`: cuts them into blocks and compresses each with
    /// `codec`, keeping a block raw where compression saves less than 1/64 of it. `zstd_level`
    /// is zstd's compression level (1–22): higher levels compress better and encode slower,
    /// while decoding speed barely changes. Encoding is deterministic for a given zstd and
    /// lz4_flex version.
    pub fn encode(
        id: AssetId,
        kind: AssetKind,
        codec: Codec,
        bytes: &[u8],
        mut dependencies: Vec<AssetId>,
        zstd_level: i32,
    ) -> Result<Self, PackError> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_ASSET_SIZE {
            return Err(PackError::Invalid { id, reason: "it is empty or larger than 1 GiB" });
        }
        dependencies.sort_unstable();
        dependencies.dedup();
        if dependencies.contains(&id) {
            return Err(PackError::Invalid { id, reason: "it depends on itself" });
        }
        let encode_error = |reason: String| PackError::Encode { id, codec, reason };
        let mut zstd = match codec {
            Codec::Zstd => {
                Some(new_zstd_encoder(zstd_level).map_err(|error| encode_error(error.to_string()))?)
            }
            Codec::None | Codec::Lz4 => None,
        };
        let mut blocks = Vec::with_capacity(bytes.len().div_ceil(BLOCK_SIZE));
        for chunk in bytes.chunks(BLOCK_SIZE) {
            let compressed = match (&mut zstd, codec) {
                (_, Codec::None) => None,
                (_, Codec::Lz4) => Some(lz4_flex::block::compress(chunk)),
                (Some(zstd), Codec::Zstd) => {
                    Some(zstd.compress(chunk).map_err(|error| encode_error(error.to_string()))?)
                }
                (None, Codec::Zstd) => unreachable!("made above for zstd"),
            };
            let (codec, bytes) = match compressed {
                Some(compressed) if saves_enough(compressed.len(), chunk.len()) => (codec, compressed),
                _ => (Codec::None, chunk.to_vec()),
            };
            blocks.push(StoredBlock { codec, hash: *blake3::hash(&bytes).as_bytes(), bytes });
        }
        Ok(Self {
            id,
            kind,
            codec,
            dependencies,
            size: bytes.len() as u64,
            hash: *blake3::hash(bytes).as_bytes(),
            blocks,
        })
    }

    /// Bytes its blocks take in a pack, without alignment padding.
    pub fn stored_len(&self) -> u64 {
        self.blocks.iter().map(|block| block.bytes.len() as u64).sum()
    }

    /// Checks what [`super::Toc::read`] will check of this asset's entry and blocks.
    fn validate(&self) -> Result<(), PackError> {
        let invalid = |reason| PackError::Invalid { id: self.id, reason };
        if self.size == 0 || self.size > MAX_ASSET_SIZE {
            return Err(invalid("it is empty or larger than 1 GiB"));
        }
        if self.blocks.len() as u64 != self.size.div_ceil(BLOCK_SIZE as u64) {
            return Err(invalid("its block count doesn't match its size"));
        }
        if self.dependencies.windows(2).any(|pair| pair[0] >= pair[1]) || self.dependencies.contains(&self.id)
        {
            return Err(invalid("its dependencies aren't sorted and distinct, or include itself"));
        }
        for (index, block) in (0..).zip(&self.blocks) {
            let len = block_len(self.size, index) as usize;
            let fits = match block.codec {
                Codec::None => block.bytes.len() == len,
                codec if codec == self.codec => !block.bytes.is_empty() && block.bytes.len() < len,
                _ => return Err(invalid("a block's codec is neither the asset's nor none")),
            };
            if !fits {
                return Err(invalid("a block's stored length doesn't fit its codec"));
            }
            debug_assert_eq!(
                blake3::hash(&block.bytes).as_bytes(),
                &block.hash,
                "block hashes are of their bytes"
            );
        }
        Ok(())
    }
}

/// Lays out a pack holding `assets`, in any order: they are sorted by ID, which fixes the
/// table of contents and the data order, so the same assets always make the same bytes.
pub fn write_pack(mut assets: Vec<PackAsset>) -> Result<Vec<u8>, PackError> {
    assets.sort_unstable_by_key(|asset| asset.id);
    if let Some(pair) = assets.windows(2).find(|pair| pair[0].id == pair[1].id) {
        return Err(PackError::Duplicate(pair[0].id));
    }
    for asset in &assets {
        asset.validate()?;
    }
    let count = |n: usize| u32::try_from(n).map_err(|_| PackError::Header("more than 2^32 records"));
    let block_count = count(assets.iter().map(|asset| asset.blocks.len()).sum())?;
    let dependency_count = count(assets.iter().map(|asset| asset.dependencies.len()).sum())?;
    let entry_count = count(assets.len())?;

    let mut entries = Vec::with_capacity(assets.len() * ENTRY_LEN);
    let mut blocks = Vec::with_capacity(block_count as usize * BLOCK_LEN);
    let mut dependencies = Vec::with_capacity(dependency_count as usize * DEPENDENCY_LEN);
    let (mut first_block, mut first_dependency, mut data_len) = (0u32, 0u32, 0u64);
    for asset in &assets {
        let (asset_blocks, asset_dependencies) = (asset.blocks.len() as u32, asset.dependencies.len() as u32);
        entries.extend_from_slice(&asset.id.to_bytes());
        entries.extend_from_slice(&[asset.kind as u8, asset.codec as u8, 0, 0]);
        for value in [first_block, asset_blocks, first_dependency, asset_dependencies, 0] {
            entries.extend_from_slice(&value.to_le_bytes());
        }
        entries.extend_from_slice(&asset.size.to_le_bytes());
        entries.extend_from_slice(&asset.hash);
        for block in &asset.blocks {
            let offset = data_len.next_multiple_of(DATA_ALIGN);
            blocks.extend_from_slice(&offset.to_le_bytes());
            blocks.extend_from_slice(&(block.bytes.len() as u32).to_le_bytes());
            blocks.extend_from_slice(&[block.codec as u8, 0, 0, 0]);
            blocks.extend_from_slice(&block.hash);
            data_len = offset + block.bytes.len() as u64;
        }
        for dependency in &asset.dependencies {
            dependencies.extend_from_slice(&dependency.to_bytes());
        }
        first_block += asset_blocks;
        first_dependency += asset_dependencies;
    }

    let mut toc = entries;
    toc.extend_from_slice(&blocks);
    toc.extend_from_slice(&dependencies);
    let header = PackHeader {
        entry_count,
        block_count,
        dependency_count,
        data_offset: (HEADER_LEN + toc.len()) as u64,
        data_len,
        toc_hash: *blake3::hash(&toc).as_bytes(),
    };
    debug_assert_eq!(header.toc_len(), toc.len() as u64);
    // Records are multiples of 16 bytes, so the data starts aligned.
    debug_assert!(header.data_offset.is_multiple_of(DATA_ALIGN));

    let mut out = Vec::with_capacity(usize::try_from(header.file_len()).unwrap_or(0));
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(&toc);
    for block in assets.iter().flat_map(|asset| &asset.blocks) {
        // The data starts aligned, so aligning in the file aligns in the data.
        out.resize(out.len().next_multiple_of(DATA_ALIGN as usize), 0);
        out.extend_from_slice(&block.bytes);
    }
    debug_assert_eq!(out.len() as u64, header.file_len());
    Ok(out)
}

/// Whether a block compressed from `len` to `stored` bytes is worth decoding: smaller by at
/// least 1/64 (1 KiB of a whole block).
fn saves_enough(stored: usize, len: usize) -> bool {
    stored < len && len - stored >= len / 64
}

/// A zstd encoder for pack blocks: a frame with its content size (so a decoder knows the size
/// up front) but no checksum or dictionary ID, which blake3 hashes and the format make
/// redundant.
fn new_zstd_encoder(level: i32) -> std::io::Result<zstd::bulk::Compressor<'static>> {
    use zstd::zstd_safe::CParameter;
    let mut encoder = zstd::bulk::Compressor::new(level)?;
    encoder.set_parameter(CParameter::ContentSizeFlag(true))?;
    encoder.set_parameter(CParameter::ChecksumFlag(false))?;
    encoder.set_parameter(CParameter::DictIdFlag(false))?;
    Ok(encoder)
}
