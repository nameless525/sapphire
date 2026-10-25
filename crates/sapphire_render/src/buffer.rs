//! GPU buffer helpers: a buffer that grows geometrically, and writes staged through the
//! frame's staging belt.

use sapphire_rhi::wgpu;

/// A buffer that is replaced by a larger one when it runs out of room. Whoever binds it must
/// rebuild bind groups when [`GrowableBuffer::reserve`] reports a replacement.
#[derive(Debug)]
pub(crate) struct GrowableBuffer {
    buffer: wgpu::Buffer,
    label: &'static str,
    usage: wgpu::BufferUsages,
    max_size: u64,
    /// Times the buffer was replaced by a larger one (rule R9 stats).
    growths: u32,
}

impl GrowableBuffer {
    /// `max_size` is the largest buffer the device allows for this use (buffer size, or
    /// binding size for storage buffers).
    pub(crate) fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        capacity: u64,
        max_size: u64,
    ) -> Self {
        // COPY_DST for queue writes, COPY_SRC to carry contents over when growing.
        let usage = usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let buffer = create(device, label, usage, capacity.min(max_size));
        Self { buffer, label, usage, max_size, growths: 0 }
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub(crate) fn max_size(&self) -> u64 {
        self.max_size
    }

    pub(crate) fn growths(&self) -> u32 {
        self.growths
    }

    /// Ensures room for `needed` bytes (at most [`Self::max_size`]), copying the first `keep`
    /// bytes into the new buffer if it has to grow. Returns `true` when the buffer was replaced.
    pub(crate) fn reserve(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        needed: u64,
        keep: u64,
    ) -> bool {
        debug_assert!(
            needed <= self.max_size,
            "{} needs {needed} bytes, limit {}",
            self.label,
            self.max_size
        );
        if needed <= self.buffer.size() {
            return false;
        }
        let capacity = needed.max(self.buffer.size() * 2).next_power_of_two().min(self.max_size);
        let grown = create(device, self.label, self.usage, capacity);
        if keep > 0 {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(self.label) });
            encoder.copy_buffer_to_buffer(&self.buffer, 0, &grown, 0, keep);
            queue.submit([encoder.finish()]);
        }
        tracing::debug!(target: crate::LOG_TARGET, label = self.label, capacity, "grew GPU buffer");
        self.buffer = grown;
        self.growths += 1;
        true
    }
}

/// Copies `bytes` to `target` at `offset` through the staging belt, as part of `encoder`.
/// Empty writes are skipped. `offset` and the length must be multiples of 4.
pub(crate) fn stage(
    belt: &mut wgpu::util::StagingBelt,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::Buffer,
    offset: u64,
    bytes: &[u8],
) {
    if let Some(size) = wgpu::BufferSize::new(bytes.len() as u64) {
        belt.write_buffer(encoder, target, offset, size).copy_from_slice(bytes);
    }
}

fn create(device: &wgpu::Device, label: &str, usage: wgpu::BufferUsages, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        // Bindings can't be empty, and copies need 4-byte multiples.
        size: capacity.max(16).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
        usage,
        mapped_at_creation: false,
    })
}
