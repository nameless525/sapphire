//! The memory ledger (M3 Z1, M6 X2): what the engine holds, by its own accounting, beside what the OS counts
//! the process as holding, one row a pool, read the same way by the stats overlay and a bench's memory line.
//!
//! - **Its rows** ([`LEDGER_ROWS`]) are bytes after a frame: the asset server's resident bytes (all, meshes,
//!   textures), the renderer's mesh pool and uploaded textures, the frame graph's transient pool, and, since
//!   M6 X2, the bytes on their way in or out that no pool counts: textures a raise replaced still waiting for
//!   the last frame that may read them, the raises' staging the GPU may not have copied yet, and what the
//!   asset server decoded and holds for the GPU. Since the M6 backfill, the decoded sound the mixer holds
//!   (M6 U1's counter, given to the ledger as a number: the renderer knows nothing of audio). The process's
//!   footprint comes last.
//! - **Memoryless attachments count as nothing** where the adapter says they save memory (the transient
//!   pool's `TransientStats::bytes`), and their estimate stands in a row of its own.
//! - **Estimates, not the GPU's figures:** textures and transients are their payloads (alignment, padding and
//!   compression aside), and the asset server's bytes are cooked bytes. The rows overlap in places (an
//!   uploaded texture's levels are the asset server's resident bytes and the renderer's texture bytes both),
//!   so their sum is no figure: compare each with its budget, and the process with spec §3.1.

use sapphire_assets::{AssetKind, AssetStats};
use sapphire_rhi::TransientStats;

use crate::RenderStats;

/// What the engine holds after a frame, in bytes (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryLedger {
    /// All the asset server holds, cooked, and of it meshes, and textures (tails and the levels above
    /// them alike, M5 M4).
    pub resident: u64,
    pub resident_meshes: u64,
    pub resident_textures: u64,
    /// The renderer's mesh pool in use: vertices and indices.
    pub mesh_pool: u64,
    /// The textures uploaded to the GPU, as the app that uploads them counts them (`Content::texture_bytes`).
    pub textures: u64,
    /// The frame graph's transient textures, memoryless attachments counted as nothing, and those
    /// attachments' own estimate (M6 X2).
    pub transient: u64,
    pub memoryless: u64,
    /// Textures raised or lowered away from and still waiting for the last frame that may read them (M5 M4),
    /// and the raises' new levels staged for copies the GPU may not have made yet (M6 X2).
    pub retiring: u64,
    pub staging: u64,
    /// Bytes the asset server read and decoded and holds for the GPU, waiting for their dependencies or in
    /// the upload queue (M5 B4).
    pub decoded_waiting: u64,
    /// The decoded samples the mixer holds (M6 U1, `sapphire_audio::AudioStats::decoded_bytes`): 0 where nothing
    /// plays sound. The asset server's resident bytes hold the same sounds again, cooked.
    pub audio_decoded: u64,
    /// The process, as its OS counts it (`sapphire_platform::process_memory`): 0 where it isn't read.
    pub process: u64,
}

impl MemoryLedger {
    /// The ledger from the counters that keep it: the renderer's, the transient pool's, the asset
    /// server's, the textures the app uploaded, the decoded sound its mixer holds and the process's footprint.
    pub fn of(
        render: &RenderStats,
        transients: &TransientStats,
        assets: &AssetStats,
        textures: u64,
        audio_decoded: u64,
        process: u64,
    ) -> Self {
        Self {
            resident: assets.resident.iter().sum(),
            resident_meshes: assets.resident[AssetKind::Mesh.index()],
            resident_textures: assets.resident[AssetKind::Texture.index()]
                + assets.resident[AssetKind::TextureLevel.index()],
            mesh_pool: render.mesh_pool_bytes,
            textures,
            transient: transients.bytes,
            memoryless: transients.memoryless_bytes,
            retiring: render.mip_streaming.retiring_bytes,
            staging: render.mip_streaming.staging_bytes,
            decoded_waiting: assets.decoded_waiting,
            audio_decoded,
            process,
        }
    }
}

/// One of the ledger's rows: the field a bench's memory line logs it as, in MiB (the scale report reads it by
/// that name), the overlay's name for it, and how it reads.
#[derive(Clone, Copy, Debug)]
pub struct LedgerRow {
    pub field: &'static str,
    pub name: &'static str,
    pub read: fn(&MemoryLedger) -> u64,
}

/// Every row, in the order both readers show them.
pub const LEDGER_ROWS: [LedgerRow; 12] = [
    LedgerRow { field: "resident_mib", name: "Asset server, all", read: |ledger| ledger.resident },
    LedgerRow { field: "resident_meshes_mib", name: "  meshes", read: |ledger| ledger.resident_meshes },
    LedgerRow { field: "resident_textures_mib", name: "  textures", read: |ledger| ledger.resident_textures },
    LedgerRow { field: "mesh_pool_mib", name: "Mesh pool", read: |ledger| ledger.mesh_pool },
    LedgerRow { field: "textures_mib", name: "Textures", read: |ledger| ledger.textures },
    LedgerRow { field: "transient_mib", name: "Transient pool", read: |ledger| ledger.transient },
    LedgerRow {
        field: "memoryless_mib",
        name: "  memoryless, not counted",
        read: |ledger| ledger.memoryless,
    },
    LedgerRow { field: "retiring_mib", name: "Textures retiring", read: |ledger| ledger.retiring },
    LedgerRow { field: "staging_mib", name: "Raises' staging", read: |ledger| ledger.staging },
    LedgerRow {
        field: "decoded_waiting_mib",
        name: "Decoded, waiting",
        read: |ledger| ledger.decoded_waiting,
    },
    LedgerRow { field: "audio_decoded_mib", name: "Decoded audio", read: |ledger| ledger.audio_decoded },
    LedgerRow { field: "process_mib", name: "Process", read: |ledger| ledger.process },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MipStreamingStats;

    /// Each row reads its own counter, the asset server's sums add the right kinds, the new rows (M6 X2)
    /// come from the streamer, the transient pool and the asset server, and decoded audio (the M6 backfill)
    /// from the number the app gives.
    #[test]
    fn rows_read_their_counters_and_sum_their_kinds() {
        let mut assets = AssetStats::default();
        for (index, bytes) in assets.resident.iter_mut().enumerate() {
            *bytes = 1 << index;
        }
        assets.decoded_waiting = 7_000;
        let render = RenderStats {
            mesh_pool_bytes: 11_000,
            mip_streaming: MipStreamingStats {
                retiring_bytes: 13_000,
                staging_bytes: 17_000,
                ..Default::default()
            },
            ..RenderStats::default()
        };
        let transients = TransientStats { bytes: 19_000, memoryless_bytes: 23_000, ..Default::default() };
        let ledger = MemoryLedger::of(&render, &transients, &assets, 29_000, 37_000, 31_000);

        let every_kind: u64 = (0..AssetKind::ALL.len()).map(|index| 1u64 << index).sum();
        let textures = (1 << AssetKind::Texture.index()) + (1 << AssetKind::TextureLevel.index());
        let read: Vec<(&str, u64)> = LEDGER_ROWS.iter().map(|row| (row.field, (row.read)(&ledger))).collect();
        assert_eq!(
            read,
            [
                ("resident_mib", every_kind),
                ("resident_meshes_mib", 1 << AssetKind::Mesh.index()),
                ("resident_textures_mib", textures),
                ("mesh_pool_mib", 11_000),
                ("textures_mib", 29_000),
                ("transient_mib", 19_000),
                ("memoryless_mib", 23_000),
                ("retiring_mib", 13_000),
                ("staging_mib", 17_000),
                ("decoded_waiting_mib", 7_000),
                ("audio_decoded_mib", 37_000),
                ("process_mib", 31_000),
            ]
        );
    }

    /// The bench logs each row by its field, which its report reads by name: no two alike, each in MiB,
    /// and none with a space or `=` to break the line.
    #[test]
    fn fields_are_distinct_log_names() {
        for (index, row) in LEDGER_ROWS.iter().enumerate() {
            assert!(row.field.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_'), "{}", row.field);
            assert!(row.field.ends_with("_mib"), "{}", row.field);
            assert!(LEDGER_ROWS[..index].iter().all(|before| before.field != row.field), "{}", row.field);
        }
    }
}
