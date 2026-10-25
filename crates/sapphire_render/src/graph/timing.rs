//! GPU pass timings from timestamp queries (spec §5.11, §5.12 debug views).
//!
//! Each running pass gets a pair of timestamps. How they are written depends on the device:
//!
//! - **Between passes** where the device has `TIMESTAMP_QUERY_INSIDE_ENCODERS` (Vulkan, DX12):
//!   the executor writes them into the encoder around each pass, so every pass is timed,
//!   copies and uploads included.
//! - **At render-pass boundaries** elsewhere (Apple GPUs, whose Metal counters sample only at
//!   stage boundaries): the pass asks [`PassContext::timestamp_writes`](super::PassContext)
//!   for its render pass's `timestamp_writes` (or `compute_timestamp_writes` for a compute
//!   pass), and passes that never do go untimed.
//!
//! Timestamps are resolved and copied into one of a few mapped-on-demand readback buffers, read
//! a frame or two later when the GPU is done, without ever waiting on it. If every readback is
//! still in flight, a frame simply isn't timed. Steady frames allocate only wgpu's own map
//! callback; nothing here runs unless timing is turned on.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use sapphire_rhi::{GpuContext, wgpu};

/// Readbacks in flight at most: a frame being recorded plus the frame-latency the swapchain
/// allows, with one to spare.
const READBACKS: usize = 3;
/// Pairs the first query set holds; it grows to fit the graph.
const INITIAL_PAIRS: u32 = 32;
/// Each pair resolves to its own 256-byte slot: `resolve_query_set` writes at aligned offsets,
/// and only the pairs that were written may be resolved.
const PAIR_STRIDE: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;

/// Readback states, in an `AtomicU8` the map callback writes.
const FREE: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;

/// A pass's GPU time in the latest timed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassTime {
    pub name: &'static str,
    /// When it started, after the frame's first timed pass started.
    pub start: Duration,
    /// From its start timestamp to its end one. Tile-based GPUs (Apple's) start a pass's
    /// vertex work while earlier passes still run, so there this includes waiting on them.
    pub duration: Duration,
    /// The part of `duration` after every earlier pass had ended: what the pass added to the
    /// frame. Where passes don't overlap it is `duration`; the passes' exclusive times add up
    /// to the span less any time no pass ran.
    pub exclusive: Duration,
}

/// GPU time per pass for the most recent frame whose timestamps have come back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuTimings {
    /// Which [`GraphExecutor::execute`](super::GraphExecutor::execute) call it was, counting
    /// from 1, so readers can tell how old it is.
    pub frame: u64,
    /// Timed passes in the order they ran. A pass that took no timestamps is absent.
    pub passes: Vec<PassTime>,
    /// Whether every pass is timed (timestamps between passes); otherwise only passes that
    /// begin a render pass with [`PassContext::timestamp_writes`](super::PassContext).
    pub every_pass: bool,
}

impl GpuTimings {
    /// From the first timed pass's start to the last one's end. On tile-based GPUs passes
    /// overlap, so this is less than the sum of their durations.
    pub fn span(&self) -> Duration {
        self.passes.iter().map(|pass| pass.start + pass.duration).max().unwrap_or_default()
    }

    /// The times of the first pass named `name`.
    pub fn get(&self, name: &str) -> Option<&PassTime> {
        self.passes.iter().find(|pass| pass.name == name)
    }
}

/// What a frame's recording needs: the query set and how to write into it.
#[derive(Clone, Copy, Debug)]
pub(super) struct FrameQueries<'a> {
    pub query_set: &'a wgpu::QuerySet,
    /// Write timestamps into the encoder around each pass, rather than through render passes.
    pub encoder_writes: bool,
}

/// One running pass's timestamps, handed to its recording.
#[derive(Debug)]
pub(super) struct PassTimestamps<'a> {
    pub query_set: &'a wgpu::QuerySet,
    /// The pass's order among the running passes; it owns queries `2 * pair` and `2 * pair + 1`.
    pub pair: u32,
    /// Set once a render pass took them.
    pub taken: std::cell::Cell<bool>,
}

impl<'a> PassTimestamps<'a> {
    pub fn render_pass_writes(&self) -> wgpu::RenderPassTimestampWrites<'a> {
        let (begin, end) = self.take();
        wgpu::RenderPassTimestampWrites {
            query_set: self.query_set,
            beginning_of_pass_write_index: Some(begin),
            end_of_pass_write_index: Some(end),
        }
    }

    pub fn compute_pass_writes(&self) -> wgpu::ComputePassTimestampWrites<'a> {
        let (begin, end) = self.take();
        wgpu::ComputePassTimestampWrites {
            query_set: self.query_set,
            beginning_of_pass_write_index: Some(begin),
            end_of_pass_write_index: Some(end),
        }
    }

    /// The pair's query indices, marking them written.
    fn take(&self) -> (u32, u32) {
        debug_assert!(!self.taken.get(), "a graph pass's timestamps go to one render or compute pass");
        self.taken.set(true);
        (2 * self.pair, 2 * self.pair + 1)
    }
}

#[derive(Debug)]
struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// The execute call it holds.
    frame: u64,
    /// Per pair: the pass and whether its timestamps were written.
    passes: Vec<(&'static str, bool)>,
}

/// What the executor asks of a GPU timer, once per frame. The executor holds its timer as a
/// trait object that only [`timer`] creates, so an app that never turns timing on doesn't link
/// the timer, or wgpu's query-set code, at all.
pub(super) trait FrameTimer: std::fmt::Debug + Send + Sync {
    /// The latest timings read back, if any frame has come back yet.
    fn latest(&self) -> Option<&GpuTimings>;

    /// Reads back finished frames, then picks a readback for frame `frame` of `passes` running
    /// passes. `None` if every readback is still in flight: this frame goes untimed.
    fn begin(&mut self, device: &wgpu::Device, frame: u64, passes: usize) -> Option<FrameQueries<'_>>;

    /// Resolves the written timestamps of this frame's `passes` (in running order, with
    /// whether each was timed) into the frame's readback. Submit the returned command buffer
    /// after the frame's others, then call [`Self::submitted`].
    fn finish(
        &mut self,
        device: &wgpu::Device,
        passes: &mut dyn Iterator<Item = (&'static str, bool)>,
    ) -> Option<wgpu::CommandBuffer>;

    /// Starts reading back the frame [`Self::finish`] resolved, once it has been submitted.
    fn submitted(&mut self);
}

/// A timer for `gpu`, or `None` if the device can't take timestamps.
pub(super) fn timer(gpu: &GpuContext) -> Option<Box<dyn FrameTimer>> {
    PassTimer::new(gpu).map(|timer| Box::new(timer) as Box<dyn FrameTimer>)
}

/// Timestamp queries and their readbacks; see the module docs.
#[derive(Debug)]
struct PassTimer {
    query_set: wgpu::QuerySet,
    /// Pairs `query_set`, `resolve` and the readbacks hold.
    capacity: u32,
    resolve: wgpu::Buffer,
    readbacks: Vec<Readback>,
    encoder_writes: bool,
    /// Nanoseconds per timestamp tick.
    period: f32,
    /// The readback this frame writes, between `begin` and `submitted`.
    current: Option<usize>,
    latest: GpuTimings,
}

impl PassTimer {
    fn new(gpu: &GpuContext) -> Option<Self> {
        if !gpu.caps().timestamp_query {
            return None;
        }
        let encoder_writes = gpu.caps().features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let (query_set, resolve, readbacks) = create(gpu.device(), INITIAL_PAIRS);
        Some(Self {
            query_set,
            capacity: INITIAL_PAIRS,
            resolve,
            readbacks,
            encoder_writes,
            period: gpu.queue().get_timestamp_period(),
            current: None,
            latest: GpuTimings { every_pass: encoder_writes, ..GpuTimings::default() },
        })
    }

    /// Turns every mapped readback into timings, keeping the newest frame's.
    fn collect(&mut self) {
        for readback in &mut self.readbacks {
            if readback.state.load(Ordering::Acquire) != MAPPED {
                continue;
            }
            if readback.frame > self.latest.frame {
                let used = readback.passes.len() as u64 * PAIR_STRIDE;
                // The buffer mapped successfully, so the range is available.
                if let Ok(bytes) = readback.buffer.get_mapped_range(..used) {
                    self.latest.frame = readback.frame;
                    decode(&bytes, &readback.passes, self.period, &mut self.latest.passes);
                }
            }
            readback.buffer.unmap();
            readback.state.store(FREE, Ordering::Release);
        }
    }
}

impl FrameTimer for PassTimer {
    fn latest(&self) -> Option<&GpuTimings> {
        (self.latest.frame > 0).then_some(&self.latest)
    }

    fn begin(&mut self, device: &wgpu::Device, frame: u64, passes: usize) -> Option<FrameQueries<'_>> {
        self.collect();
        let passes = u32::try_from(passes).unwrap_or(u32::MAX);
        if passes > self.capacity {
            // Frames still in flight lose their timings; their buffers are dropped with them.
            self.capacity = passes.next_power_of_two().min(wgpu::QUERY_SET_MAX_QUERIES / 2);
            (self.query_set, self.resolve, self.readbacks) = create(device, self.capacity);
        }
        if passes > self.capacity {
            return None;
        }
        let index =
            self.readbacks.iter().position(|readback| readback.state.load(Ordering::Acquire) == FREE)?;
        self.readbacks[index].frame = frame;
        self.readbacks[index].passes.clear();
        self.current = Some(index);
        Some(FrameQueries { query_set: &self.query_set, encoder_writes: self.encoder_writes })
    }

    fn finish(
        &mut self,
        device: &wgpu::Device,
        passes: &mut dyn Iterator<Item = (&'static str, bool)>,
    ) -> Option<wgpu::CommandBuffer> {
        let readback = &mut self.readbacks[self.current?];
        readback.passes.extend(passes);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pass timings") });
        for (pair, _) in readback.passes.iter().enumerate().filter(|(_, (_, written))| *written) {
            let first = 2 * pair as u32;
            encoder.resolve_query_set(
                &self.query_set,
                first..first + 2,
                &self.resolve,
                pair as u64 * PAIR_STRIDE,
            );
        }
        let used = readback.passes.len() as u64 * PAIR_STRIDE;
        if used > 0 {
            encoder.copy_buffer_to_buffer(&self.resolve, 0, &readback.buffer, 0, used);
        }
        Some(encoder.finish())
    }

    fn submitted(&mut self) {
        let Some(index) = self.current.take() else { return };
        let readback = &self.readbacks[index];
        if readback.passes.is_empty() {
            return;
        }
        readback.state.store(MAPPING, Ordering::Release);
        let state = Arc::clone(&readback.state);
        let used = readback.passes.len() as u64 * PAIR_STRIDE;
        readback.buffer.map_async(wgpu::MapMode::Read, ..used, move |result| {
            // A failed map (device lost, buffer dropped) frees the readback with nothing read.
            state.store(if result.is_ok() { MAPPED } else { FREE }, Ordering::Release);
        });
    }
}

/// Fills `out` with the timed passes' times from resolved `bytes` (one pair per
/// [`PAIR_STRIDE`]), at `period` nanoseconds per tick. Pairs a backend left invalid (an end
/// before the start, or an error value) are dropped.
fn decode(bytes: &[u8], passes: &[(&'static str, bool)], period: f32, out: &mut Vec<PassTime>) {
    out.clear();
    let ticks = |offset: usize| {
        let word: [u8; 8] = bytes[offset..offset + 8].try_into().expect("8 bytes");
        u64::from_le_bytes(word)
    };
    let timed = || {
        passes.iter().enumerate().filter_map(move |(pair, &(name, written))| {
            let offset = pair * PAIR_STRIDE as usize;
            let (start, end) = (ticks(offset), ticks(offset + 8));
            (written && end >= start && end != u64::MAX).then_some((name, start, end))
        })
    };
    let Some(origin) = timed().map(|(_, start, _)| start).min() else { return };
    let nanos = |ticks: u64| Duration::from_nanos((ticks as f64 * f64::from(period)) as u64);
    let mut latest_end = origin;
    out.extend(timed().map(|(name, start, end)| {
        let exclusive = end.saturating_sub(start.max(latest_end));
        latest_end = latest_end.max(end);
        PassTime {
            name,
            start: nanos(start - origin),
            duration: nanos(end - start),
            exclusive: nanos(exclusive),
        }
    }));
}

fn create(device: &wgpu::Device, pairs: u32) -> (wgpu::QuerySet, wgpu::Buffer, Vec<Readback>) {
    let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("pass timestamps"),
        ty: wgpu::QueryType::Timestamp,
        count: 2 * pairs,
    });
    let size = u64::from(pairs) * PAIR_STRIDE;
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pass timestamp resolve"),
        size,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readbacks = (0..READBACKS)
        .map(|_| Readback {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pass timestamp readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(FREE)),
            frame: 0,
            passes: Vec::with_capacity(pairs as usize),
        })
        .collect();
    (query_set, resolve, readbacks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(pairs: &[(u64, u64)]) -> Vec<u8> {
        let mut bytes = vec![0; pairs.len() * PAIR_STRIDE as usize];
        for (pair, &(start, end)) in pairs.iter().enumerate() {
            let offset = pair * PAIR_STRIDE as usize;
            bytes[offset..offset + 8].copy_from_slice(&start.to_le_bytes());
            bytes[offset + 8..offset + 16].copy_from_slice(&end.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn decodes_written_pairs_relative_to_the_first_start() {
        // Post starts before main ends, as on a tile-based GPU; bloom ends while post runs.
        let bytes = resolved(&[(1000, 1500), (0, 0), (1200, 4200), (5000, 4000), (1300, 2000)]);
        let passes = [("main", true), ("uploads", false), ("post", true), ("broken", true), ("bloom", true)];
        let mut out = Vec::new();
        // 2 ns per tick.
        decode(&bytes, &passes, 2.0, &mut out);
        let ns = Duration::from_nanos;
        assert_eq!(
            out,
            [
                PassTime { name: "main", start: ns(0), duration: ns(1000), exclusive: ns(1000) },
                PassTime { name: "post", start: ns(400), duration: ns(6000), exclusive: ns(5400) },
                PassTime { name: "bloom", start: ns(600), duration: ns(1400), exclusive: ns(0) },
            ],
            "untimed and invalid pairs are dropped"
        );
        let timings = GpuTimings { frame: 3, passes: out, every_pass: false };
        assert_eq!(timings.span(), ns(6400));
        assert_eq!(timings.get("post").map(|post| post.exclusive), Some(ns(5400)));
        assert_eq!(timings.get("uploads"), None);

        decode(&resolved(&[(10, 5)]), &[("only broken", true)], 1.0, &mut Vec::new());
        let mut empty = vec![PassTime { name: "stale", start: ns(0), duration: ns(1), exclusive: ns(1) }];
        decode(&[], &[], 1.0, &mut empty);
        assert!(empty.is_empty(), "an empty frame clears the old timings");
    }
}
