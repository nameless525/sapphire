//! The frame graph (spec §5.12).
//!
//! Each frame builds a [`FrameGraph`] in code. It holds textures, either imported (the
//! swapchain image, a capture target) or transient (made by the graph from a pool), and
//! passes, which declare the textures they read and write and record their GPU work in a
//! closure. A [`GraphExecutor`] then compiles the graph:
//!
//! 1. **Cull:** a pass runs only if it has a side effect or writes something that a later
//!    pass reads or that outlives the frame (an imported texture).
//! 2. **Order:** passes run in the order they were added, and every texture must be written
//!    before it is read.
//! 3. **Lifetimes:** a transient lives from the first running pass that uses it to the last.
//! 4. **Pooling:** transients with equal descriptors and disjoint lifetimes share one texture
//!    from a [`TransientPool`](sapphire_rhi::TransientPool), which also carries textures over
//!    between frames.
//!
//! It then records the passes, on several workers when there are enough of them, into
//! command buffers that it submits in order with one `queue.submit`. wgpu tracks barriers,
//! so ordering, culling, pooling and parallel recording are the graph's whole job.
//!
//! # Writes
//!
//! A write replaces a texture's contents. A pass that keeps any of them (`LoadOp::Load`, or
//! drawing over part of the texture) must also read the texture, or culling may drop the
//! passes that wrote them.
//!
//! # Cost
//!
//! A graph costs about 10 heap allocations per frame (its node lists, plus one boxed closure
//! per pass) and well under a microsecond to build and compile at M0's two passes; recording
//! the passes is the real work. Parallel recording adds a job scope's allocations.
//!
//! # Inspection (M1 F5)
//!
//! Both are off by default and cost nothing until turned on, for the debug overlay and,
//! later, dynamic resolution (spec §5.12, quality tiers):
//!
//! - [`GraphExecutor::set_report`] keeps a [`GraphReport`] of the last graph: every pass with
//!   whether it ran and its command buffer, every texture with its pooled slot and lifetime.
//!   It reuses its lists, so a steady graph fills it without allocating.
//! - [`GraphExecutor::set_gpu_timing`] times each running pass with timestamp queries and reads
//!   them back a frame or two later without waiting ([`GpuTimings`]). Devices that can write
//!   timestamps between passes time every pass; on Apple GPUs, which sample only at
//!   render-pass boundaries, a pass is timed when it hands [`PassContext::timestamp_writes`]
//!   to its render pass (or [`PassContext::compute_timestamp_writes`] to its compute pass). Devices without timestamp queries report it unavailable.
//!
//! # Later milestones
//!
//! - Buffers as graph resources, with the first transient buffers (GPU culling's per-frame draw
//!   lists). Every buffer so far is persistent, compute skinning's outputs included (M1 R2):
//!   declaring one would never cull a pass, since it outlives the frame as an imported texture
//!   does, and wgpu already orders accesses to it. Passes that write buffers other passes read
//!   (uploads, skinning, light clusters) are side effects added before their readers.
//! - Textures kept into the next frame (TAA history).
//! - A per-pass cost hint, so parallel recording splits by work instead of pass count. GPU
//!   timings could seed it.

mod executor;
mod plan;
mod report;
mod timing;

use sapphire_rhi::{TransientDesc, TransientStats, wgpu};

pub use executor::GraphExecutor;
use plan::{Access, Shape};
pub use report::{AccessReport, GraphReport, PassReport, TextureReport};
use timing::PassTimestamps;
pub use timing::{GpuTimings, PassTime};

/// A texture in a [`FrameGraph`]. Only meaningful in the graph that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(u32);

impl TextureId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// Records a pass's GPU work.
type RecordFn<'f> = Box<dyn FnOnce(&mut PassContext<'_>) + Send + 'f>;

/// One frame's passes and textures. `'f` is how long what the passes borrow lives: the
/// imported views and whatever the record closures capture.
#[derive(Default)]
pub struct FrameGraph<'f> {
    shape: Shape,
    /// Per texture: the view of an imported one.
    imports: Vec<Option<&'f wgpu::TextureView>>,
    /// Per pass.
    records: Vec<Option<RecordFn<'f>>>,
}

impl std::fmt::Debug for FrameGraph<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameGraph").field("shape", &self.shape).finish_non_exhaustive()
    }
}

impl<'f> FrameGraph<'f> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a texture the graph doesn't own, such as the swapchain image. Its contents
    /// outlive the frame, so passes that write it run. `view` must cover mip 0 of a 2D texture.
    pub fn import_texture(&mut self, name: &'static str, view: &'f wgpu::TextureView) -> TextureId {
        let texture = view.texture();
        let id = self.shape.add_texture(name, (texture.width(), texture.height()), None);
        self.imports.push(Some(view));
        id
    }

    /// Adds a texture that lives only within this frame, taken from the executor's pool.
    pub fn create_texture(&mut self, name: &'static str, desc: TransientDesc) -> TextureId {
        let id = self.shape.add_texture(name, (desc.width, desc.height), Some(desc));
        self.imports.push(None);
        id
    }

    /// Width and height of `texture`.
    pub fn size(&self, texture: TextureId) -> (u32, u32) {
        self.shape.textures[texture.index()].size
    }

    /// Adds a pass. Passes run in the order they are added.
    pub fn add_pass(&mut self, name: &'static str) -> PassBuilder<'_, 'f> {
        let pass = self.shape.add_pass(name);
        self.records.push(None);
        PassBuilder { graph: self, pass }
    }
}

/// Declares what a pass reads and writes, then its recording.
#[must_use = "a pass records nothing until `record` is called"]
pub struct PassBuilder<'g, 'f> {
    graph: &'g mut FrameGraph<'f>,
    pass: usize,
}

impl<'f> PassBuilder<'_, 'f> {
    /// The pass reads `texture`'s current contents.
    pub fn read(self, texture: TextureId) -> Self {
        self.graph.shape.add_access(self.pass, texture, false);
        self
    }

    /// The pass replaces `texture`'s contents (see the module docs on keeping them).
    pub fn write(self, texture: TextureId) -> Self {
        self.graph.shape.add_access(self.pass, texture, true);
        self
    }

    /// Runs the pass even if nothing reads what it writes: uploads, readbacks, queries.
    pub fn side_effect(self) -> Self {
        self.graph.shape.passes[self.pass].side_effect = true;
        self
    }

    /// Sets the pass's recording. It runs at most once, possibly on a worker thread, and may
    /// use only the textures the pass declared.
    pub fn record(self, record: impl FnOnce(&mut PassContext<'_>) + Send + 'f) {
        self.graph.records[self.pass] = Some(Box::new(record));
    }
}

/// What a pass records with: an encoder and its declared textures.
pub struct PassContext<'a> {
    pub encoder: &'a mut wgpu::CommandEncoder,
    textures: &'a [Option<Resolved<'a>>],
    accesses: &'a [Access],
    /// This pass's timestamps, while GPU timing writes them through render passes.
    timestamps: Option<&'a PassTimestamps<'a>>,
}

/// An imported or pooled texture and its view.
type Resolved<'a> = (&'a wgpu::Texture, &'a wgpu::TextureView);

impl<'a> PassContext<'a> {
    pub fn texture(&self, texture: TextureId) -> &'a wgpu::Texture {
        self.resolve(texture).0
    }

    /// The default view: the whole texture, in its own format for transients.
    pub fn view(&self, texture: TextureId) -> &'a wgpu::TextureView {
        self.resolve(texture).1
    }

    /// The `timestamp_writes` for this pass's render pass, so GPU timing can time it on devices
    /// that take timestamps only at render-pass boundaries (Apple GPUs; see
    /// [`GraphExecutor::set_gpu_timing`]). `None` whenever the executor times passes itself or
    /// timing is off. A pass that begins several render passes passes it to one of them.
    pub fn timestamp_writes(&self) -> Option<wgpu::RenderPassTimestampWrites<'a>> {
        self.timestamps.map(PassTimestamps::render_pass_writes)
    }

    /// [`Self::timestamp_writes`] for a pass whose work is a compute pass.
    pub fn compute_timestamp_writes(&self) -> Option<wgpu::ComputePassTimestampWrites<'a>> {
        self.timestamps.map(PassTimestamps::compute_pass_writes)
    }

    fn resolve(&self, texture: TextureId) -> Resolved<'a> {
        debug_assert!(
            self.accesses.iter().any(|access| access.texture == texture),
            "the pass uses {texture:?} without declaring it"
        );
        // Every texture a running pass declares is resolved.
        self.textures[texture.index()].expect("an undeclared texture")
    }
}

/// A frame graph that can't run. These are bugs in the code that built it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("pass `{pass}` reads `{texture}` before any pass writes it")]
    ReadBeforeWrite { pass: &'static str, texture: &'static str },
}

/// Counters from the last executed frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphStats {
    /// Passes that ran.
    pub passes: u32,
    /// Passes culled because nothing used what they wrote.
    pub culled: u32,
    /// Transient textures the running passes used.
    pub transients: u32,
    /// Pooled textures those were packed into.
    pub transient_slots: u32,
    /// Command buffers recorded, more than one when recording in parallel.
    pub command_buffers: u32,
    pub pool: TransientStats,
}
