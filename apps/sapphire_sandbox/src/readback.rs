//! Reading a frame's pixels back from the GPU, for a caller that drew into a texture.

use std::sync::mpsc;

use sapphire_render::graph::{FrameGraph, TextureId};
use sapphire_rhi::{GpuContext, wgpu};

/// A frame read back from the GPU: a buffer the frame's target is copied into, after the
/// frame's passes, then mapped and stripped of its row padding.
pub(crate) struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row_bytes: u32,
}

impl Readback {
    /// A row of 4 byte pixels, as a copy into a buffer lays it out.
    pub fn padded_row_bytes(width: u32) -> u64 {
        (u64::from(width) * 4).next_multiple_of(u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT))
    }

    /// For a `width` by `height` target of 4 bytes a pixel.
    pub fn new(gpu: &GpuContext, width: u32, height: u32) -> Self {
        let row_bytes = Self::padded_row_bytes(width);
        let padded_row_bytes = u32::try_from(row_bytes).expect("a target's side is a device's at most");
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: gpu.label("capture readback"),
            size: row_bytes * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, width, height, padded_row_bytes }
    }

    /// Adds the pass that copies `target` here, after every pass before it that writes it.
    /// Passes borrow what they use until the graph has run.
    pub fn record<'f>(&'f self, graph: &mut FrameGraph<'f>, target: TextureId) {
        let (buffer, padded_row_bytes, height) = (&self.buffer, self.padded_row_bytes, self.height);
        graph.add_pass("readback").read(target).side_effect().record(move |ctx| {
            let texture = ctx.texture(target);
            ctx.encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row_bytes),
                        rows_per_image: Some(height),
                    },
                },
                texture.size(),
            );
        });
    }

    /// Waits for the GPU to finish the frame and returns its pixels, as tightly packed rows of
    /// the target's format.
    pub fn read(&self, gpu: &GpuContext) -> Result<Vec<u8>, String> {
        let (sender, receiver) = mpsc::channel();
        self.buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            // The receiver outlives the poll below, so the send can't fail.
            let _ = sender.send(result);
        });
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?.map_err(|error| error.to_string())?;
        let mapped = self.buffer.get_mapped_range(..).map_err(|error| error.to_string())?;
        let row_bytes = self.width as usize * 4;
        let mut pixels = Vec::with_capacity(row_bytes * self.height as usize);
        for row in mapped.chunks_exact(self.padded_row_bytes as usize) {
            pixels.extend_from_slice(&row[..row_bytes]);
        }
        Ok(pixels)
    }
}
