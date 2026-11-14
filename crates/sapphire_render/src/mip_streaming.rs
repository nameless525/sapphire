//! Raising and lowering textures a level at a time (M5 M4, m5-plan §3 "Mip streaming"): the renderer's
//! half of what [`crate::Renderer::raise_texture`] and [`crate::Renderer::lower_texture`] do beyond making
//! the new texture ([`GpuTexture::raised`], [`GpuTexture::lowered`]).
//!
//! - **Off the frame's passes.** A raise or a lower runs between frames, where the app hands it the levels
//!   it loaded. Its copies go into an encoder of their own, submitted once, ahead of the next frame's
//!   ([`MipStreaming::submit`], at the top of [`crate::Renderer::prepare`]), and the new levels go with the
//!   queue's writes, which wgpu runs before any submission's commands: no pass records any of it, and a
//!   frame lost before it ran still finds the new texture whole.
//! - **A budget a frame.** A frame may write so many bytes by raising ([`crate::Renderer::set_mip_budget`]):
//!   the app raises while [`crate::Renderer::may_raise`] says they fit, and the first raise always does,
//!   however large, as the asset server's upload budget lets one asset through.
//! - **The old texture goes when no frame reads it.** Materials bind the new one at once
//!   ([`crate::materials::Materials::rebind`]), so no frame prepared after a raise draws with the old, and
//!   it waits as freed meshes and materials do (the `retire` module) for the last frame that may read it:
//!   the one the copies are submitted with, which comes after every frame that drew it.
//! - **Counters** (rule R9, [`MipStreamingStats`]): raises, lowers and bytes since the last frame was
//!   prepared, and what waits to be let go of.

use sapphire_rhi::wgpu;

use crate::retire::Retiring;
use crate::texture::GpuTexture;

/// Bytes a frame may write by raising textures until [`crate::Renderer::set_mip_budget`] sets another: the
/// asset server's desktop upload budget, so a level it hands over in a frame is written in that frame.
pub const DEFAULT_MIP_BUDGET: u64 = 16 << 20;

/// Raises and lowers: what they made and wrote between the last two prepared frames, what waits for its
/// frames, and the budget (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MipStreamingStats {
    /// Textures raised: each a new texture, of one level more or several.
    pub raises: u32,
    /// Textures lowered.
    pub lowers: u32,
    /// Bytes of levels written by the raises: what [`Self::budget`] holds.
    pub bytes_written: u64,
    /// Bytes of kept levels the raises and lowers copied, on the GPU.
    pub bytes_copied: u64,
    /// Materials' group 3 made again to bind the new textures.
    pub rebinds: u32,
    /// Textures replaced and let go of, now that no frame can read them.
    pub retired: u32,
    /// Textures replaced and still waiting for the last frame that may read them, and their bytes: the
    /// moment a raise holds the old texture beside the new one.
    pub retiring: u32,
    pub retiring_bytes: u64,
    /// The bytes a frame may write by raising.
    pub budget: u64,
}

/// What raises and lowers leave for the renderer: their copies to submit, the textures they replaced, and
/// their counters.
#[derive(Debug)]
pub(crate) struct MipStreaming {
    /// Copies recorded since the last submission.
    encoder: Option<wgpu::CommandEncoder>,
    /// Textures replaced, until no frame can be reading them.
    retired: Retiring<GpuTexture>,
    retired_bytes: u64,
    budget: u64,
    /// What the counters read since the last prepared frame.
    frame: MipStreamingStats,
}

impl MipStreaming {
    pub(crate) fn new() -> Self {
        Self {
            encoder: None,
            retired: Retiring::default(),
            retired_bytes: 0,
            budget: DEFAULT_MIP_BUDGET,
            frame: MipStreamingStats::default(),
        }
    }

    /// The encoder the next copies go into.
    pub(crate) fn encoder(&mut self, device: &wgpu::Device) -> &mut wgpu::CommandEncoder {
        self.encoder.get_or_insert_with(|| {
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mip streaming") })
        })
    }

    /// Submits the copies recorded since the last submission, if any.
    pub(crate) fn submit(&mut self, queue: &wgpu::Queue) {
        if let Some(encoder) = self.encoder.take() {
            queue.submit([encoder.finish()]);
        }
    }

    /// Counts a raise that wrote `written` bytes of levels and copied `copied`, or a lower that copied
    /// `copied`, and rebound `rebinds` materials; and keeps `old`, which it replaced, until frame
    /// `last_read` is finished.
    pub(crate) fn replaced(
        &mut self,
        old: GpuTexture,
        last_read: u64,
        written: Option<u64>,
        copied: u64,
        rebinds: u32,
    ) {
        match written {
            Some(written) => {
                self.frame.raises += 1;
                self.frame.bytes_written += written;
            }
            None => self.frame.lowers += 1,
        }
        self.frame.bytes_copied += copied;
        self.frame.rebinds += rebinds;
        self.retired_bytes += old.bytes();
        self.retired.push(last_read, old);
    }

    /// Lets go of the textures no frame up to `finished` was the last to read.
    pub(crate) fn release_finished(&mut self, finished: u64) {
        while let Some(old) = self.retired.pop_finished(finished) {
            self.retired_bytes -= old.bytes();
            self.frame.retired += 1;
        }
    }

    /// Replaced textures still waiting for frames.
    pub(crate) fn retiring(&self) -> usize {
        self.retired.len()
    }

    /// Whether a raise writing `bytes` fits what's left of this frame's budget: the first always does.
    pub(crate) fn may_write(&self, bytes: u64) -> bool {
        self.frame.bytes_written == 0 || self.frame.bytes_written + bytes <= self.budget
    }

    pub(crate) fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    /// The counters since the last prepared frame, which start again from here.
    pub(crate) fn take_stats(&mut self) -> MipStreamingStats {
        let stats = MipStreamingStats {
            retiring: self.retired.len() as u32,
            retiring_bytes: self.retired_bytes,
            budget: self.budget,
            ..self.frame
        };
        self.frame = MipStreamingStats::default();
        stats
    }
}
