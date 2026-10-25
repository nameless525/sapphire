//! Running a compiled frame graph: pooled textures, recording and the frame's one submit.

use sapphire_rhi::{GpuContext, TransientPool, TransientTexture, wgpu};

use super::plan::{Access, PassNode, Plan};
use super::{FrameGraph, GraphError, GraphStats, PassContext, RecordFn, Resolved};

/// Below this many passes, everything is recorded into one encoder on the calling thread:
/// a job scope costs more than recording a few passes.
const PARALLEL_MIN_PASSES: usize = 4;

/// The fewest passes recorded into each encoder when recording in parallel.
const MIN_PASSES_PER_ENCODER: usize = 2;

/// Compiles, records and submits [`FrameGraph`]s. Keep one per device for as long as it is
/// rendering: it owns the transient texture pool.
#[derive(Debug, Default)]
pub struct GraphExecutor {
    plan: Plan,
    pool: TransientPool,
    /// The pooled texture behind each of the plan's slots.
    slot_textures: Vec<TransientTexture>,
    stats: GraphStats,
}

/// A running pass, handed to whichever thread records it.
struct Job<'g, 'f> {
    pass: &'g PassNode,
    record: Option<RecordFn<'f>>,
}

impl GraphExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compiles `graph`, records its running passes and submits them. On an error nothing
    /// is recorded or submitted.
    pub fn execute(&mut self, gpu: &GpuContext, graph: FrameGraph<'_>) -> Result<(), GraphError> {
        profiling::scope!("FrameGraph::execute");
        let FrameGraph { shape, imports, mut records } = graph;
        self.plan.compile(&shape)?;

        self.pool.begin_frame();
        self.slot_textures.clear();
        for desc in &self.plan.slot_descs {
            self.slot_textures.push(self.pool.acquire(gpu, desc));
        }
        let pool = &self.pool;
        let textures: Vec<Option<Resolved<'_>>> = imports
            .iter()
            .zip(&self.plan.slots)
            .map(|(import, slot)| match (import, slot) {
                (Some(view), _) => Some((view.texture(), *view)),
                (None, Some(slot)) => {
                    let texture = self.slot_textures[*slot];
                    Some((pool.texture(texture), pool.view(texture)))
                }
                (None, None) => None,
            })
            .collect();
        let mut jobs: Vec<Job<'_, '_>> = self
            .plan
            .passes
            .iter()
            .map(|&index| Job { pass: &shape.passes[index], record: records[index].take() })
            .collect();

        let encoders = if jobs.len() < PARALLEL_MIN_PASSES {
            1
        } else {
            let threads = sapphire_jobs::system().compute_pool().thread_num();
            // The waiting thread records too.
            (jobs.len() / MIN_PASSES_PER_ENCODER).min(threads + 1)
        };
        let recorder = Recorder {
            device: gpu.device(),
            labels: gpu.label("").is_some(),
            textures: &textures,
            accesses: &shape.accesses,
        };
        let command_buffers = if encoders <= 1 {
            vec![recorder.record(&mut jobs)]
        } else {
            let chunk_len = jobs.len().div_ceil(encoders);
            sapphire_jobs::scope(|scope| {
                for chunk in jobs.chunks_mut(chunk_len) {
                    let recorder = &recorder;
                    scope.spawn(move || recorder.record(chunk));
                }
            })
        };
        self.stats = GraphStats {
            passes: self.plan.passes.len() as u32,
            culled: (shape.passes.len() - self.plan.passes.len()) as u32,
            transients: self.plan.slots.iter().flatten().count() as u32,
            transient_slots: self.plan.slot_descs.len() as u32,
            command_buffers: command_buffers.len() as u32,
            pool: self.pool.stats(),
        };
        gpu.queue().submit(command_buffers);
        Ok(())
    }

    /// Counters from the last [`Self::execute`].
    pub fn stats(&self) -> GraphStats {
        self.stats
    }
}

/// What every recording thread shares.
struct Recorder<'a> {
    device: &'a wgpu::Device,
    /// Whether to label encoders and wrap passes in debug groups.
    labels: bool,
    textures: &'a [Option<Resolved<'a>>],
    accesses: &'a [Access],
}

impl Recorder<'_> {
    /// Records `jobs` in order into a new encoder.
    fn record(&self, jobs: &mut [Job<'_, '_>]) -> wgpu::CommandBuffer {
        let label = self.labels.then_some("frame graph");
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label });
        for job in jobs {
            let Some(record) = job.record.take() else { continue };
            profiling::scope!("graph pass", job.pass.name);
            if self.labels {
                encoder.push_debug_group(job.pass.name);
            }
            record(&mut PassContext {
                encoder: &mut encoder,
                textures: self.textures,
                accesses: &self.accesses[job.pass.accesses.clone()],
            });
            if self.labels {
                encoder.pop_debug_group();
            }
        }
        encoder.finish()
    }
}
