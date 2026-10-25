//! Running a compiled frame graph: pooled textures, recording and the frame's one submit.

use sapphire_rhi::{GpuContext, TransientPool, TransientTexture, wgpu};

use super::plan::{Access, PassNode, Plan};
use super::report::GraphReport;
use super::timing::{self, FrameQueries, FrameTimer, GpuTimings, PassTimestamps};
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
    /// Graphs executed so far.
    frames: u64,
    /// Filled each frame while on ([`Self::set_report`]).
    report: Option<GraphReport>,
    /// Present while GPU timing is on ([`Self::set_gpu_timing`]).
    timer: Option<Box<dyn FrameTimer>>,
}

/// A running pass, handed to whichever thread records it.
struct Job<'g, 'f> {
    pass: &'g PassNode,
    record: Option<RecordFn<'f>>,
    /// Its position among the running passes: its timestamp pair.
    order: u32,
    /// Whether its timestamps were written.
    timed: bool,
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
        self.frames += 1;

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
            .enumerate()
            .map(|(order, &index)| Job {
                pass: &shape.passes[index],
                record: records[index].take(),
                order: order as u32,
                timed: false,
            })
            .collect();

        let encoders = if jobs.len() < PARALLEL_MIN_PASSES {
            1
        } else {
            let threads = sapphire_jobs::system().compute_pool().thread_num();
            // The waiting thread records too.
            (jobs.len() / MIN_PASSES_PER_ENCODER).min(threads + 1)
        };
        let chunk_len = jobs.len().div_ceil(encoders.max(1)).max(1);
        let queries = match &mut self.timer {
            Some(timer) => timer.begin(gpu.device(), self.frames, jobs.len()),
            None => None,
        };
        let recorder = Recorder {
            device: gpu.device(),
            labels: gpu.label("").is_some(),
            textures: &textures,
            accesses: &shape.accesses,
            queries,
        };
        let mut command_buffers = if encoders <= 1 {
            vec![recorder.record(&mut jobs)]
        } else {
            sapphire_jobs::scope(|scope| {
                for chunk in jobs.chunks_mut(chunk_len) {
                    let recorder = &recorder;
                    scope.spawn(move || recorder.record(chunk));
                }
            })
        };
        let timed = recorder.queries.is_some();
        self.stats = GraphStats {
            passes: self.plan.passes.len() as u32,
            culled: (shape.passes.len() - self.plan.passes.len()) as u32,
            transients: self.plan.slots.iter().flatten().count() as u32,
            transient_slots: self.plan.slot_descs.len() as u32,
            command_buffers: command_buffers.len() as u32,
            pool: self.pool.stats(),
        };
        if let Some(report) = &mut self.report {
            report.fill(&shape, &self.plan, chunk_len);
        }
        if timed && let Some(timer) = &mut self.timer {
            let mut passes = jobs.iter().map(|job| (job.pass.name, job.timed));
            command_buffers.extend(timer.finish(gpu.device(), &mut passes));
        }
        gpu.queue().submit(command_buffers);
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
        Ok(())
    }

    /// Counters from the last [`Self::execute`].
    pub fn stats(&self) -> GraphStats {
        self.stats
    }

    /// Turns the [`GraphReport`] of each executed graph on or off (it is off by default).
    pub fn set_report(&mut self, enabled: bool) {
        if enabled != self.report.is_some() {
            self.report = enabled.then(GraphReport::default);
        }
    }

    /// The last executed graph, while [`Self::set_report`] is on.
    pub fn report(&self) -> Option<&GraphReport> {
        self.report.as_ref()
    }

    /// Turns per-pass GPU timing on or off (it is off by default) and returns whether it is on:
    /// it needs timestamp queries ([`GpuCaps::timestamp_query`](sapphire_rhi::GpuCaps)), which
    /// some mobile GPUs and the iOS Simulator lack. Turning it off releases its queries.
    ///
    /// Where the device can write timestamps between passes, every pass is timed. Elsewhere
    /// (Apple GPUs) a pass is timed when it hands [`PassContext::timestamp_writes`] to its
    /// render pass, as the renderer's passes do; see [`GpuTimings::every_pass`].
    pub fn set_gpu_timing(&mut self, gpu: &GpuContext, enabled: bool) -> bool {
        if enabled && self.timer.is_none() {
            self.timer = timing::timer(gpu);
        } else if !enabled {
            self.timer = None;
        }
        self.timer.is_some()
    }

    /// The latest per-pass GPU times, a frame or two behind (they are read back without
    /// waiting on the GPU). `None` while timing is off or before the first frame comes back.
    pub fn gpu_timings(&self) -> Option<&GpuTimings> {
        self.timer.as_deref().and_then(FrameTimer::latest)
    }

    /// Graphs executed so far: the frame [`GpuTimings::frame`] counts in.
    pub fn frames(&self) -> u64 {
        self.frames
    }
}

/// What every recording thread shares.
struct Recorder<'a> {
    device: &'a wgpu::Device,
    /// Whether to label encoders and wrap passes in debug groups.
    labels: bool,
    textures: &'a [Option<Resolved<'a>>],
    accesses: &'a [Access],
    /// This frame's timestamp queries, while GPU timing is on.
    queries: Option<FrameQueries<'a>>,
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
            let encoder_writes = self.queries.filter(|queries| queries.encoder_writes);
            if let Some(queries) = encoder_writes {
                encoder.write_timestamp(queries.query_set, 2 * job.order);
            }
            let timestamps = self.queries.filter(|queries| !queries.encoder_writes).map(|queries| {
                PassTimestamps { query_set: queries.query_set, pair: job.order, taken: Default::default() }
            });
            record(&mut PassContext {
                encoder: &mut encoder,
                textures: self.textures,
                accesses: &self.accesses[job.pass.accesses.clone()],
                timestamps: timestamps.as_ref(),
            });
            if let Some(queries) = encoder_writes {
                encoder.write_timestamp(queries.query_set, 2 * job.order + 1);
                job.timed = true;
            } else {
                job.timed = timestamps.is_some_and(|timestamps| timestamps.taken.get());
            }
            if self.labels {
                encoder.pop_debug_group();
            }
        }
        encoder.finish()
    }
}
