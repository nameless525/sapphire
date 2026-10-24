//! How a vertex shader finds its draw record.
//!
//! Every draw needs a draw ID to index the draw-record buffer (instance and material). It
//! arrives through vertex attribute `@location(5)`, fed by an instance-step
//! vertex buffer holding the identity table `[0, 1, 2, …]`. The shaders never read
//! `instance_index`, which leaves two ways to submit the same pipelines:
//!
//! - **CPU-driven (this path, portable everywhere):** bind the table at byte offset
//!   `draw_id * 4` and draw instances `0..1`. It needs neither a non-zero `first_instance`
//!   nor a non-zero `base_vertex`, so it runs on the iOS Simulator, which supports
//!   neither.
//! - **GPU-driven (later, desktop first):** bind the table at offset 0 and let each
//!   multi-draw-indirect record set `first_instance = draw_id`. That requires
//!   `GpuCaps::multi_draw_indirect`.

use sapphire_rhi::wgpu;

use crate::buffer::GrowableBuffer;

const ID_BYTES: u64 = size_of::<u32>() as u64;

/// The identity table. It is rewritten only when it grows, so steady state allocates and
/// uploads nothing.
#[derive(Debug)]
pub(crate) struct DrawIdTable {
    buffer: GrowableBuffer,
    /// Number of IDs written, which is the buffer's whole capacity.
    len: u32,
}

impl DrawIdTable {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let buffer = GrowableBuffer::new(
            device,
            "draw ids",
            wgpu::BufferUsages::VERTEX,
            1024 * ID_BYTES,
            device.limits().max_buffer_size,
        );
        let mut table = Self { buffer, len: 0 };
        table.fill(queue);
        table
    }

    /// Ensures IDs `0..count` exist.
    pub(crate) fn reserve(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, count: usize) {
        let count = count as u64;
        if count > u64::from(self.len) {
            self.buffer.reserve(device, queue, count * ID_BYTES, 0);
            self.fill(queue);
        }
    }

    /// The vertex buffer slice that makes instance 0 read `draw_id`.
    pub(crate) fn slice(&self, draw_id: u32) -> wgpu::BufferSlice<'_> {
        debug_assert!(draw_id < self.len, "draw id {draw_id} beyond the table ({})", self.len);
        self.buffer.buffer().slice(u64::from(draw_id) * ID_BYTES..)
    }

    fn fill(&mut self, queue: &wgpu::Queue) {
        let len = u32::try_from(self.buffer.buffer().size() / ID_BYTES).expect("draw id table under 16 GiB");
        let ids: Vec<u32> = (0..len).collect();
        queue.write_buffer(self.buffer.buffer(), 0, bytemuck::cast_slice(&ids));
        self.len = len;
    }
}
