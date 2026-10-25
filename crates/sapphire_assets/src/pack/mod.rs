//! Packs (`.spack`): the files cooked content ships in (spec §5.10, M1 A4).
//!
//! A pack holds cooked assets (meshes, textures, materials) by [`AssetId`], each exactly the bytes
//! the cook wrote, compressed in 64 KB blocks so any block can be read and decoded on its own. Its
//! table of contents is the runtime's ID index: nothing at runtime names an asset by path
//! (rule R5).
//!
//! # Layout
//!
//! Every integer is little-endian.
//!
//! | Offset | Size | Part |
//! |---|---|---|
//! | 0 | 256 | header ([`PackHeader`]) |
//! | 256 | 80 × entries | one entry per asset, sorted by ID ([`Entry`]) |
//! | … | 48 × blocks | one per block, asset by asset in entry order ([`Block`]) |
//! | … | 16 × dependencies | asset IDs, each entry's in a run of its own, sorted |
//! | `data_offset` | `data_len` | block data, each block starting 16-byte aligned |
//!
//! The header:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `SPAK` |
//! | 4 | 4 | format version, [`PACK_VERSION`] |
//! | 8 | 4 | header length, 256 |
//! | 12 | 4 | flags, 0 |
//! | 16 | 4 | block size, [`BLOCK_SIZE`] |
//! | 20 | 12 | entry, block and dependency counts (u32 each) |
//! | 32 | 24 | table of contents length, data offset and data length (u64 each) |
//! | 56 | 8 | reserved, 0 |
//! | 64 | 32 | blake3 of the table of contents |
//! | 96 | 32 | reserved, 0 |
//! | 128 | 32 | blake3 of bytes 0..128: the header hash |
//! | 160 | 96 | signature: scheme (u32), 28 bytes for the signing key's ID, 64 signature bytes; all 0 while unsigned |
//!
//! An entry is the asset's ID (16 bytes), its [`AssetKind`] (u8) and [`Codec`] (u8), 2 reserved
//! bytes, its first block and block count, its first dependency and dependency count (u32
//! each), 4 reserved bytes, its size (u64) and the blake3 hash of its bytes (32). A block is its
//! offset in the data (u64), its stored length (u32), its [`Codec`] (u8), 3 reserved bytes and the
//! blake3 hash of its stored bytes (32). Reserved bytes are 0.
//!
//! # Integrity
//!
//! Every byte a reader uses is covered by a hash (the alignment padding between blocks is never
//! read): the header hash covers the header, which holds the table of contents' hash, which holds
//! every block's hash. [`Toc::read`] checks the header, the table of
//! contents and their consistency (counts, sizes, ranges, order, the data's extent against the
//! file's length) when a pack is mounted; each block's hash is checked before the block is
//! decoded, and each asset's hash after its blocks are. Corrupt, truncated or hostile data is an
//! error, never a panic, and a decoder only ever sees bytes whose hash matched.
//!
//! **Signing (later, with `sapphire_patch`, M4):** the signature fields are reserved now so the
//! layout doesn't change then. A signature will cover the header hash, and through it every byte
//! of the pack; a reader rejects any scheme it doesn't know, and today every nonzero one. Spec
//! §5.9's plan to skip bytecheck for verified packs rests on that signature, so until it exists
//! every cooked asset is validated when it is decoded.
//!
//! # Blocks and codecs
//!
//! An asset's bytes are cut into [`BLOCK_SIZE`] blocks (its last one shorter), each compressed on
//! its own with the asset's [`Codec`]: zstd for most data, lz4 for data that must decode fast
//! (the packer's policy, in `sapphire_pipeline::pack`). A block that compression doesn't shrink
//! by at least 1/64 is stored raw ([`Codec::None`]), so decoders skip data that doesn't compress,
//! such as already-compressed sections. Blocks start 16-byte aligned and lie in entry order, so
//! an asset stored raw is contiguous and aligned in the file: memory-mapping one in place is
//! possible later without a format change.
//!
//! [`PackReader`] reads packs held in memory (tools and tests). The runtime reads ranges through
//! the asset server's IO ([`crate::PackSource`]) and decodes them with [`Toc::decode_asset`].

mod write;

use std::fmt;
use std::ops::Range;

use sapphire_serialize::cooked::AlignedVec;
use serde::{Deserialize, Serialize};

use crate::{AssetId, AssetKind};

pub use write::{PackAsset, StoredBlock, write_pack};

/// Conventional file extension for packs.
pub const PACK_EXTENSION: &str = "spack";
/// The pack layout this build reads and writes. Readers reject other versions.
pub const PACK_VERSION: u32 = 1;
/// Bytes per block of an asset (its last block may be shorter).
pub const BLOCK_SIZE: usize = 64 * 1024;
/// Bytes in a pack's header.
pub const HEADER_LEN: usize = 256;
/// The largest asset a pack may hold, decoded: 1 GiB. The largest texture the engine cooks, 16384² BC7
/// with its mips, is a third of that. A pack can't make a reader allocate more for one asset,
/// however it is crafted, until signatures (M4) vouch for packs.
pub const MAX_ASSET_SIZE: u64 = 1 << 30;

const MAGIC: [u8; 4] = *b"SPAK";
/// The header bytes the header hash covers: everything before it.
const HASHED_HEADER_LEN: usize = 128;
const ENTRY_LEN: usize = 80;
const BLOCK_LEN: usize = 48;
const DEPENDENCY_LEN: usize = 16;
/// Block data starts at multiples of this, as rkyv archives and KTX2 levels need.
const DATA_ALIGN: u64 = 16;

/// The file a pack named `name` is stored in: `<name>.spack`.
pub fn file_name(name: &str) -> String {
    format!("{name}.{PACK_EXTENSION}")
}

/// How a block's bytes are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Codec {
    /// As they are.
    None = 0,
    /// LZ4 block format (`lz4_flex`): for data that must decode fast.
    Lz4 = 1,
    /// A zstd frame without checksum (blocks have blake3 hashes): for most data.
    Zstd = 2,
}

impl Codec {
    /// The codec a pack stores as `value`, if any.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::Lz4),
            2 => Some(Self::Zstd),
            _ => None,
        }
    }

    /// The name used in messages.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A pack that can't be read, or an asset in it that can't be decoded.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PackError {
    #[error("not a pack (no `SPAK` magic)")]
    NotAPack,
    #[error("pack format {0} is not supported (this build reads format {PACK_VERSION})")]
    UnsupportedVersion(u32),
    #[error("the pack's header doesn't match its hash: the file is corrupt")]
    HeaderHash,
    #[error("the pack's header is inconsistent: {0}")]
    Header(&'static str),
    #[error("signature scheme {0} is not supported (packs are unsigned until M4)")]
    UnsupportedSignature(u32),
    #[error("the pack is truncated: {found} bytes, but its header describes {expected}")]
    Truncated { expected: u64, found: u64 },
    #[error("the pack's table of contents doesn't match its hash: the file is corrupt")]
    TocHash,
    #[error("the pack's table of contents is inconsistent at {part} {index}: {reason}")]
    Toc { part: &'static str, index: usize, reason: &'static str },
    #[error("block {block} of asset {id} doesn't match its hash: the file is corrupt")]
    BlockHash { id: AssetId, block: u32 },
    #[error("block {block} of asset {id} doesn't decode with {codec}: {reason}")]
    Decode { id: AssetId, block: u32, codec: Codec, reason: String },
    #[error("asset {id} decoded to bytes that don't match its hash")]
    AssetHash { id: AssetId },
    #[error("the pack has no asset {0}")]
    Missing(AssetId),
    #[error("asset {0} is in the pack twice")]
    Duplicate(AssetId),
    #[error("asset {id} can't be packed: {reason}")]
    Invalid { id: AssetId, reason: &'static str },
    #[error("asset {id} failed to compress with {codec}: {reason}")]
    Encode { id: AssetId, codec: Codec, reason: String },
}

/// A pack's header, checked ([`Self::read`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackHeader {
    pub entry_count: u32,
    pub block_count: u32,
    pub dependency_count: u32,
    /// Where block data starts in the file: right after the table of contents.
    pub data_offset: u64,
    pub data_len: u64,
    pub toc_hash: [u8; 32],
}

impl PackHeader {
    /// Checks and reads the header at the start of `bytes` (which may hold more of the file).
    pub fn read(bytes: &[u8]) -> Result<Self, PackError> {
        let Some(header) = bytes.first_chunk::<HEADER_LEN>() else {
            return Err(PackError::Truncated { expected: HEADER_LEN as u64, found: bytes.len() as u64 });
        };
        if header[0..4] != MAGIC {
            return Err(PackError::NotAPack);
        }
        let version = u32_at(header, 4);
        if version != PACK_VERSION {
            return Err(PackError::UnsupportedVersion(version));
        }
        let hash = blake3::hash(&header[..HASHED_HEADER_LEN]);
        if hash.as_bytes()[..] != header[HASHED_HEADER_LEN..HASHED_HEADER_LEN + 32] {
            return Err(PackError::HeaderHash);
        }
        let scheme = u32_at(header, 160);
        if scheme != 0 {
            return Err(PackError::UnsupportedSignature(scheme));
        }
        if header[164..].iter().any(|&byte| byte != 0) {
            return Err(PackError::Header("an unsigned pack has signature bytes"));
        }
        if u32_at(header, 8) as usize != HEADER_LEN {
            return Err(PackError::Header("the header length isn't 256"));
        }
        if u32_at(header, 12) != 0 {
            return Err(PackError::Header("unknown flags"));
        }
        if u32_at(header, 16) as usize != BLOCK_SIZE {
            return Err(PackError::Header("the block size isn't 64 KiB"));
        }
        if header[56..64].iter().chain(&header[96..128]).any(|&byte| byte != 0) {
            return Err(PackError::Header("reserved bytes aren't zero"));
        }
        let parsed = Self {
            entry_count: u32_at(header, 20),
            block_count: u32_at(header, 24),
            dependency_count: u32_at(header, 28),
            data_offset: u64_at(header, 40),
            data_len: u64_at(header, 48),
            toc_hash: *header[64..96].first_chunk().expect("32 bytes"),
        };
        if u64_at(header, 32) != parsed.toc_len() {
            return Err(PackError::Header("the table of contents' length doesn't match its counts"));
        }
        if parsed.data_offset != HEADER_LEN as u64 + parsed.toc_len() {
            return Err(PackError::Header("the data doesn't follow the table of contents"));
        }
        if parsed.data_offset.checked_add(parsed.data_len).is_none() {
            return Err(PackError::Header("the data's extent overflows"));
        }
        Ok(parsed)
    }

    /// Bytes in the table of contents, from its counts.
    pub fn toc_len(&self) -> u64 {
        u64::from(self.entry_count) * ENTRY_LEN as u64
            + u64::from(self.block_count) * BLOCK_LEN as u64
            + u64::from(self.dependency_count) * DEPENDENCY_LEN as u64
    }

    /// Where the table of contents lies in the file.
    pub fn toc_range(&self) -> Range<u64> {
        HEADER_LEN as u64..self.data_offset
    }

    /// The length of the whole file.
    pub fn file_len(&self) -> u64 {
        self.data_offset + self.data_len
    }

    /// The header's bytes, with its hash, unsigned.
    fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut header = [0; HEADER_LEN];
        header[0..4].copy_from_slice(&MAGIC);
        header[4..8].copy_from_slice(&PACK_VERSION.to_le_bytes());
        header[8..12].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        header[16..20].copy_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
        header[20..24].copy_from_slice(&self.entry_count.to_le_bytes());
        header[24..28].copy_from_slice(&self.block_count.to_le_bytes());
        header[28..32].copy_from_slice(&self.dependency_count.to_le_bytes());
        header[32..40].copy_from_slice(&self.toc_len().to_le_bytes());
        header[40..48].copy_from_slice(&self.data_offset.to_le_bytes());
        header[48..56].copy_from_slice(&self.data_len.to_le_bytes());
        header[64..96].copy_from_slice(&self.toc_hash);
        let hash = blake3::hash(&header[..HASHED_HEADER_LEN]);
        header[HASHED_HEADER_LEN..HASHED_HEADER_LEN + 32].copy_from_slice(hash.as_bytes());
        header
    }
}

/// One asset in a pack's table of contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: AssetId,
    pub kind: AssetKind,
    /// The codec its blocks are compressed with; each block may also be stored raw.
    pub codec: Codec,
    /// Bytes in the asset, decoded.
    pub size: u64,
    /// blake3 of the decoded asset.
    pub hash: [u8; 32],
    blocks: (u32, u32),
    dependencies: (u32, u32),
}

impl Entry {
    /// How many blocks the asset is cut into.
    pub fn block_count(&self) -> u32 {
        self.blocks.1
    }
}

/// One block of an asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// Where its stored bytes start in the file.
    pub offset: u64,
    /// Bytes stored.
    pub stored_len: u32,
    /// Bytes once decoded: [`BLOCK_SIZE`], or less for an asset's last block.
    pub len: u32,
    pub codec: Codec,
    /// blake3 of the stored bytes.
    pub hash: [u8; 32],
}

impl Block {
    /// Where its stored bytes lie in the file.
    pub fn range(&self) -> Range<u64> {
        self.offset..self.offset + u64::from(self.stored_len)
    }
}

/// A pack's checked table of contents: the runtime's index from [`AssetId`] to where an asset's
/// blocks lie and how to decode them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toc {
    header: PackHeader,
    entries: Vec<Entry>,
    blocks: Vec<Block>,
    dependencies: Vec<AssetId>,
}

impl Toc {
    /// Checks `toc`, the bytes [`PackHeader::toc_range`] names, against `header`, and the pack's
    /// extent against `file_len`, the length of the whole file (see the module docs).
    pub fn read(header: PackHeader, toc: &[u8], file_len: u64) -> Result<Self, PackError> {
        if file_len != header.file_len() {
            return if file_len < header.file_len() {
                Err(PackError::Truncated { expected: header.file_len(), found: file_len })
            } else {
                Err(PackError::Header("the file is longer than the pack it holds"))
            };
        }
        if toc.len() as u64 != header.toc_len() {
            return Err(PackError::Truncated { expected: header.toc_len(), found: toc.len() as u64 });
        }
        if blake3::hash(toc).as_bytes() != &header.toc_hash {
            return Err(PackError::TocHash);
        }
        let entry_bytes = header.entry_count as usize * ENTRY_LEN;
        let block_bytes = header.block_count as usize * BLOCK_LEN;
        let (entry_table, rest) = toc.split_at(entry_bytes);
        let (block_table, dependency_table) = rest.split_at(block_bytes);

        let dependencies: Vec<AssetId> = dependency_table
            .as_chunks::<DEPENDENCY_LEN>()
            .0
            .iter()
            .map(|id| AssetId::from_bytes(*id))
            .collect();
        let toc_error =
            |part: &'static str, index: usize, reason: &'static str| PackError::Toc { part, index, reason };

        let mut entries = Vec::with_capacity(header.entry_count as usize);
        let (mut next_block, mut next_dependency) = (0u32, 0u32);
        for (index, raw) in entry_table.as_chunks::<ENTRY_LEN>().0.iter().enumerate() {
            let fail = |reason| toc_error("entry", index, reason);
            let id = AssetId::from_bytes(*raw.first_chunk().expect("16 bytes"));
            let kind = AssetKind::from_u8(raw[16]).ok_or_else(|| fail("unknown asset kind"))?;
            let codec = Codec::from_u8(raw[17]).ok_or_else(|| fail("unknown codec"))?;
            if raw[18..20] != [0, 0] || raw[36..40] != [0; 4] {
                return Err(fail("reserved bytes aren't zero"));
            }
            let blocks = (u32_at(raw, 20), u32_at(raw, 24));
            let dependency_run = (u32_at(raw, 28), u32_at(raw, 32));
            let size = u64_at(raw, 40);
            if entries.last().is_some_and(|previous: &Entry| previous.id >= id) {
                return Err(fail("entries aren't sorted by ID, or an ID repeats"));
            }
            if size == 0 || size > MAX_ASSET_SIZE {
                return Err(fail("an asset is empty or larger than 1 GiB"));
            }
            if u64::from(blocks.1) != size.div_ceil(BLOCK_SIZE as u64) {
                return Err(fail("the block count doesn't match the size"));
            }
            if blocks.0 != next_block || dependency_run.0 != next_dependency {
                return Err(fail("blocks or dependencies aren't in entry order"));
            }
            next_block = next_block.checked_add(blocks.1).ok_or_else(|| fail("too many blocks"))?;
            next_dependency =
                next_dependency.checked_add(dependency_run.1).ok_or_else(|| fail("too many dependencies"))?;
            if next_block > header.block_count || next_dependency > header.dependency_count {
                return Err(fail("blocks or dependencies run past the table"));
            }
            let own = &dependencies[dependency_run.0 as usize..next_dependency as usize];
            if own.windows(2).any(|pair| pair[0] >= pair[1]) || own.contains(&id) {
                return Err(fail("dependencies aren't sorted and distinct, or include the asset itself"));
            }
            let hash = *raw[48..80].first_chunk().expect("32 bytes");
            entries.push(Entry { id, kind, codec, size, hash, blocks, dependencies: dependency_run });
        }
        if next_block != header.block_count || next_dependency != header.dependency_count {
            return Err(toc_error("entry", entries.len(), "blocks or dependencies belong to no entry"));
        }

        let block_table = block_table.as_chunks::<BLOCK_LEN>().0;
        let mut blocks = Vec::with_capacity(header.block_count as usize);
        let mut data_end = 0u64;
        for entry in &entries {
            for block in 0..entry.blocks.1 {
                let index = blocks.len();
                let fail = |reason| toc_error("block", index, reason);
                // Every entry's blocks are within the table: checked with the entries.
                let raw = &block_table[index];
                let offset = u64_at(raw, 0);
                let stored_len = u32_at(raw, 8);
                let codec = Codec::from_u8(raw[12]).ok_or_else(|| fail("unknown codec"))?;
                if raw[13..16] != [0; 3] {
                    return Err(fail("reserved bytes aren't zero"));
                }
                let len = block_len(entry.size, block);
                let stored_fits = match codec {
                    Codec::None => stored_len == len,
                    _ if codec == entry.codec => stored_len > 0 && stored_len < len,
                    _ => return Err(fail("a block's codec is neither its asset's nor none")),
                };
                if !stored_fits {
                    return Err(fail("the stored length doesn't fit the codec"));
                }
                if offset != data_end.next_multiple_of(DATA_ALIGN) {
                    return Err(fail("blocks aren't laid out in order, 16-byte aligned"));
                }
                data_end = offset + u64::from(stored_len);
                if data_end > header.data_len {
                    return Err(fail("a block lies past the data"));
                }
                let hash = *raw[16..48].first_chunk().expect("32 bytes");
                blocks.push(Block { offset: header.data_offset + offset, stored_len, len, codec, hash });
            }
        }
        if data_end != header.data_len {
            return Err(PackError::Header("the data is longer than its blocks"));
        }
        Ok(Self { header, entries, blocks, dependencies })
    }

    pub fn header(&self) -> &PackHeader {
        &self.header
    }

    /// Every asset, sorted by ID.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The asset `id`, if the pack has it.
    pub fn find(&self, id: AssetId) -> Option<&Entry> {
        self.entries.binary_search_by(|entry| entry.id.cmp(&id)).ok().map(|index| &self.entries[index])
    }

    /// An asset's blocks, in order.
    pub fn blocks(&self, entry: &Entry) -> &[Block] {
        let (first, count) = entry.blocks;
        &self.blocks[first as usize..(first + count) as usize]
    }

    /// The assets an asset references, which load before it: sorted, distinct, and possibly in
    /// other packs.
    pub fn dependencies(&self, entry: &Entry) -> &[AssetId] {
        let (first, count) = entry.dependencies;
        &self.dependencies[first as usize..(first + count) as usize]
    }

    /// Where an asset's blocks lie in the file, padding between them included: one read.
    pub fn stored_range(&self, entry: &Entry) -> Range<u64> {
        let blocks = self.blocks(entry);
        let start = blocks.first().map_or(0, |block| block.offset);
        start..blocks.last().map_or(start, |block| block.range().end)
    }

    /// Decodes asset `entry` from `stored`, the bytes of its [`Self::stored_range`]: checks each
    /// block's hash, decodes it, then checks the asset's hash. The result is 16-byte aligned, as
    /// cooked meshes and materials are read.
    pub fn decode_asset(
        &self,
        entry: &Entry,
        stored: &[u8],
        decoder: &mut Decoder,
    ) -> Result<AlignedVec, PackError> {
        profiling::scope!("pack::decode_asset");
        let range = self.stored_range(entry);
        if stored.len() as u64 != range.end - range.start {
            return Err(PackError::Truncated {
                expected: range.end - range.start,
                found: stored.len() as u64,
            });
        }
        let size = usize::try_from(entry.size).map_err(|_| PackError::Invalid {
            id: entry.id,
            reason: "too large for this platform's address space",
        })?;
        let mut out = AlignedVec::with_capacity(size);
        out.resize(size, 0);
        for (index, (block, chunk)) in self.blocks(entry).iter().zip(out.chunks_mut(BLOCK_SIZE)).enumerate() {
            let start = (block.offset - range.start) as usize;
            let bytes = &stored[start..start + block.stored_len as usize];
            decoder.decode(entry.id, index as u32, block, bytes, chunk)?;
        }
        if blake3::hash(&out).as_bytes() != &entry.hash {
            return Err(PackError::AssetHash { id: entry.id });
        }
        Ok(out)
    }
}

/// Decodes blocks, keeping a zstd context between them: one per decode job, made on its first
/// zstd block.
#[derive(Default)]
pub struct Decoder {
    zstd: Option<zstd::bulk::Decompressor<'static>>,
}

impl fmt::Debug for Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder").field("zstd", &self.zstd.is_some()).finish()
    }
}

impl Decoder {
    /// Checks block `index` of asset `id` against its hash and decodes it into `out`, which must
    /// be [`Block::len`] bytes.
    pub fn decode(
        &mut self,
        id: AssetId,
        index: u32,
        block: &Block,
        stored: &[u8],
        out: &mut [u8],
    ) -> Result<(), PackError> {
        if stored.len() != block.stored_len as usize || blake3::hash(stored).as_bytes() != &block.hash {
            return Err(PackError::BlockHash { id, block: index });
        }
        let decode_error =
            |reason: String| PackError::Decode { id, block: index, codec: block.codec, reason };
        if out.len() != block.len as usize {
            return Err(decode_error(format!(
                "{} bytes of output for a {}-byte block",
                out.len(),
                block.len
            )));
        }
        let written = match block.codec {
            Codec::None if stored.len() == out.len() => {
                out.copy_from_slice(stored);
                out.len()
            }
            Codec::None => stored.len(),
            Codec::Lz4 => {
                lz4_flex::block::decompress_into(stored, out).map_err(|e| decode_error(e.to_string()))?
            }
            Codec::Zstd => {
                let zstd = match &mut self.zstd {
                    Some(zstd) => zstd,
                    slot => slot.insert(new_zstd_decoder().map_err(|e| decode_error(e.to_string()))?),
                };
                zstd.decompress_to_buffer(stored, out).map_err(|e| decode_error(e.to_string()))?
            }
        };
        if written != out.len() {
            return Err(decode_error(format!("decoded {written} bytes, expected {}", out.len())));
        }
        Ok(())
    }
}

/// A zstd decoder whose window can't exceed a block's, so a frame claiming more fails rather than
/// allocating.
fn new_zstd_decoder() -> std::io::Result<zstd::bulk::Decompressor<'static>> {
    let mut decoder = zstd::bulk::Decompressor::new()?;
    // 2^17 bytes: a 64 KiB block plus the frame format's rounding.
    decoder.set_parameter(zstd::zstd_safe::DParameter::WindowLogMax(17))?;
    Ok(decoder)
}

/// A whole pack held in memory: tools, tests and packs embedded in a binary.
#[derive(Debug)]
pub struct PackReader<'a> {
    bytes: &'a [u8],
    toc: Toc,
}

impl<'a> PackReader<'a> {
    /// Checks the header and table of contents of the pack in `bytes`.
    pub fn new(bytes: &'a [u8]) -> Result<Self, PackError> {
        let header = PackHeader::read(bytes)?;
        let toc_range = header.toc_range();
        // Within `bytes` once the header is known to describe at most `bytes.len()`.
        if header.file_len() > bytes.len() as u64 {
            return Err(PackError::Truncated { expected: header.file_len(), found: bytes.len() as u64 });
        }
        let toc =
            Toc::read(header, &bytes[toc_range.start as usize..toc_range.end as usize], bytes.len() as u64)?;
        Ok(Self { bytes, toc })
    }

    pub fn toc(&self) -> &Toc {
        &self.toc
    }

    /// Decodes asset `id` whole.
    pub fn read(&self, id: AssetId) -> Result<AlignedVec, PackError> {
        let entry = self.toc.find(id).ok_or(PackError::Missing(id))?;
        let range = self.toc.stored_range(entry);
        self.toc.decode_asset(
            entry,
            &self.bytes[range.start as usize..range.end as usize],
            &mut Decoder::default(),
        )
    }

    /// Decodes block `index` of asset `id` alone: blocks are independent, so any one can be read
    /// without the others.
    pub fn read_block(&self, id: AssetId, index: u32) -> Result<Vec<u8>, PackError> {
        let entry = self.toc.find(id).ok_or(PackError::Missing(id))?;
        let block = self
            .toc
            .blocks(entry)
            .get(index as usize)
            .ok_or(PackError::Invalid { id, reason: "no block with that index" })?;
        let range = block.range();
        let mut out = vec![0; block.len as usize];
        Decoder::default().decode(
            id,
            index,
            block,
            &self.bytes[range.start as usize..range.end as usize],
            &mut out,
        )?;
        Ok(out)
    }
}

/// Decoded bytes in block `index` of an asset of `size` bytes.
fn block_len(size: u64, index: u32) -> u32 {
    let start = u64::from(index) * BLOCK_SIZE as u64;
    (size - start).min(BLOCK_SIZE as u64) as u32
}

fn u32_at<const N: usize>(bytes: &[u8; N], offset: usize) -> u32 {
    u32::from_le_bytes(*bytes[offset..].first_chunk().expect("inside the record"))
}

fn u64_at<const N: usize>(bytes: &[u8; N], offset: usize) -> u64 {
    u64::from_le_bytes(*bytes[offset..].first_chunk().expect("inside the record"))
}

#[cfg(test)]
mod tests;
