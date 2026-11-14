//! How a mesh's bytes reach the pool's buffers (M5 D9): through a ring of staging buffers the pool
//! keeps, copied on the GPU, or by `Queue::write_buffer`, which makes a staging buffer for each write.
//!
//! The ring is wgpu's `StagingBelt`: buffers of [`CHUNK`] bytes, mapped, written in place and copied
//! from by an encoder of the pool's own, given back to be mapped again once the GPU has copied from
//! them. A frame stages at most [`BYTES_PER_FRAME`] through it, which bounds what the ring holds to
//! that over the frames in flight; a write larger than a chunk, or past the frame's budget, goes by
//! `write_buffer` as before.
//!
//! The ring's copies are submitted ([`Uploads::flush`]) before anything else that touches the pool's
//! buffers on the GPU: a buffer's growth (which copies the old buffer's bytes into the new), a
//! compaction, the frame ([`Renderer::record`](crate::Renderer::record)) and a write by
//! `write_buffer`, whose bytes land at the start of the next submission: every write lands in the
//! order it was made, before anything reads or copies it. On a lost device the ring's buffers can't
//! be mapped (wgpu panics on that), so it's left alone: nothing draws again, and writes go by
//! `write_buffer`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sapphire_rhi::wgpu;

/// Bytes a staging buffer of the ring holds; a larger write goes by `write_buffer`.
const CHUNK: u64 = 1 << 20;
/// Bytes a frame stages through the ring at most.
const BYTES_PER_FRAME: u64 = 8 << 20;

#[derive(Debug)]
pub(crate) struct Uploads {
    belt: wgpu::util::StagingBelt,
    /// Copies from the ring since the last [`Self::flush`].
    encoder: Option<wgpu::CommandEncoder>,
    /// Bytes staged through the ring since the last [`Self::end_frame`].
    staged: u64,
    /// Whether writes go through the ring at all.
    ring: bool,
    /// The device's lost flag (`GpuContext::lost_flag`).
    lost: Arc<AtomicBool>,
}

impl Uploads {
    pub(crate) fn new(device: &wgpu::Device, lost: Arc<AtomicBool>) -> Self {
        let belt = wgpu::util::StagingBelt::new(device.clone(), CHUNK);
        Self { belt, encoder: None, staged: 0, ring: true, lost }
    }

    /// Sends writes through the ring (`true`) or by `write_buffer` alone, after submitting what the
    /// ring holds.
    pub(crate) fn set_ring(&mut self, queue: &wgpu::Queue, ring: bool) {
        self.flush(queue);
        self.ring = ring;
    }

    /// Writes `bytes` to `buffer` at `offset`; both are multiples of 4. Empty writes do nothing.
    pub(crate) fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        offset: u64,
        bytes: &[u8],
    ) {
        let Some(size) = wgpu::BufferSize::new(bytes.len() as u64) else {
            return;
        };
        let staged = self.staged + size.get();
        if !self.ring || size.get() > CHUNK || staged > BYTES_PER_FRAME || self.lost.load(Ordering::Acquire) {
            // After the ring's copies so far, which the write would otherwise land before: a range freed
            // and taken again is written in the order its meshes were.
            self.flush(queue);
            queue.write_buffer(buffer, offset, bytes);
            return;
        }
        let encoder = self.encoder.get_or_insert_with(|| {
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mesh uploads") })
        });
        self.belt.write_buffer(encoder, buffer, offset, size).copy_from_slice(bytes);
        self.staged = staged;
    }

    /// Submits the ring's copies so far, and gives its buffers back to be mapped once the GPU is
    /// done with them. On a lost device it drops them instead.
    pub(crate) fn flush(&mut self, queue: &wgpu::Queue) {
        let Some(encoder) = self.encoder.take() else {
            return;
        };
        if self.lost.load(Ordering::Acquire) {
            return;
        }
        self.belt.finish();
        queue.submit([encoder.finish()]);
        self.belt.recall();
    }

    /// [`Self::flush`], and a new frame's budget.
    pub(crate) fn end_frame(&mut self, queue: &wgpu::Queue) {
        self.flush(queue);
        self.staged = 0;
    }
}
