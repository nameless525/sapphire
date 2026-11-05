//! Admission for header and TOC work before issuing reads from a file source.

use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Block, Entry, HEADER_LEN, PackError, PackHeader};
use crate::{AssetId, PipelineKey};

#[derive(Debug)]
struct Budget {
    limit: u64,
    reserved: AtomicU64,
}

/// Shared capacity for queued and in-flight pack metadata reads and parsing. It charges the
/// header, serialized TOC, and requested native table vector capacities. After a reader accepts the table,
/// its lifetime follows the reader's mounted pack or snapshot ownership.
#[derive(Clone, Debug)]
pub struct PackMetadataBudget(Arc<Budget>);

impl PackMetadataBudget {
    /// Default transient capacity. This leaves space for a 4 MiB live-link metadata frame
    /// and its decoded table, along with the ordinary sub-megabyte TOCs in shipped packs.
    pub const DEFAULT_BYTES: u64 = 12 << 20;

    /// Creates an independent pool. Clones of the result share its capacity.
    pub fn new(bytes: u64) -> Self {
        Self(Arc::new(Budget { limit: bytes, reserved: AtomicU64::new(0) }))
    }

    /// Reserves the fixed header before queuing the read. A caller retains the reservation until
    /// it accepts or discards the result, including any time spent waiting for completion delivery.
    pub fn begin(&self) -> Result<PackMetadataRead, PackError> {
        let mut read = PackMetadataRead { budget: self.clone(), reserved: 0 };
        read.reserve(HEADER_LEN as u64)?;
        Ok(read)
    }
}

impl Default for PackMetadataBudget {
    fn default() -> Self {
        Self::new(Self::DEFAULT_BYTES)
    }
}

/// A header and TOC admission reservation. Dropping it returns its capacity to the shared pool.
#[derive(Debug)]
pub struct PackMetadataRead {
    budget: PackMetadataBudget,
    reserved: u64,
}

impl PackMetadataRead {
    /// Checks the extent of a parsed header, then reserves the TOC work before the source reads it.
    pub fn toc_read_len(&mut self, header: &PackHeader, file_len: u64) -> Result<usize, PackError> {
        header.check_file_len(file_len)?;
        let wire = header.toc_len();
        let bytes = wire
            + u64::from(header.entry_count) * size_of::<Entry>() as u64
            + u64::from(header.block_count) * size_of::<Block>() as u64
            + u64::from(header.dependency_count) * size_of::<AssetId>() as u64
            + u64::from(header.pipeline_key_count) * size_of::<PipelineKey>() as u64;
        if bytes > isize::MAX as u64 {
            return Err(PackError::Header("pack metadata is too large for this platform"));
        }
        self.reserve(bytes)?;
        Ok(wire as usize)
    }

    fn reserve(&mut self, additional: u64) -> Result<(), PackError> {
        let budget = &self.budget.0;
        budget
            .reserved
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |reserved| {
                reserved.checked_add(additional).filter(|&total| total <= budget.limit)
            })
            .map_err(|reserved| PackError::MetadataBudget { additional, reserved, limit: budget.limit })?;
        self.reserved += additional;
        Ok(())
    }
}

impl Drop for PackMetadataRead {
    fn drop(&mut self) {
        self.budget.0.reserved.fetch_sub(self.reserved, Ordering::Relaxed);
    }
}
