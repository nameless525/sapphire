//! `--bench-overlap` (M6 B7): a bench whose CPU and GPU overlap as a shipped frame's do, and the reading of
//! which of them the frames waited on.
//!
//! By default a bench waits for everything it submitted at the end of each frame, so the GPU is idle while the CPU
//! records the next one and nothing says which of the two a shipped frame would wait on. Overlapping, the bench
//! waits after frame N's submit for the frame graph's submission of frame N - 2 alone
//! (`wgpu::PollType::Wait` with its index): the CPU records frame N + 1 while the GPU may still run N - 1 and N,
//! two frames queued, as behind a swapchain that holds two. Each phase starts from an empty queue ([`GpuWait::drain`]).
//!
//! **The reading** ([`read`]) is taken over frames whose GPU times came back, so from a bench's GPU phase, on the
//! GPU's own clock. The CPU's clock and the GPU's share no zero, so the reading lines them up (`lined_up_by`):
//!
//! - **`waits`**, with [`LINED_UP_FEWEST`] or more frames whose CPU waited [`WAITED`] or longer: a wait for frame
//!   N - 2 returns once that frame has ended on the GPU, never before, so each return puts the GPU's clock at most
//!   its frame's end less the return ahead of the CPU's. The reading takes the most of them: a return read late, as a
//!   busy machine can delay a thread, only ever lowers its own. A submit so placed is where the CPU made it, and a
//!   frame's age counts the driver's latency.
//! - **`idle`**, a GPU that rarely made the CPU wait: the median lag from a submit to its frame's first timed pass
//!   over the frames the GPU idled [`IDLE_FRAME`] or longer before ([`LINED_UP_FEWEST`] of them at least), the
//!   driver's latency and any untimed work before that pass (an Apple GPU's copies take no timestamps); a median,
//!   since one submit read late would throw a least lag off by its delay. A submit so placed is where its frame could
//!   start at the soonest.
//! - **`least`**, with too few of either: the least such lag in the phase, which the frame after its drain holds.
//!
//! - **Behind:** how much GPU work was still queued ahead of a frame at its submit, from its submit to the previous
//!   frame's end: how far the GPU runs behind the CPU.
//! - **The queue's age:** from a frame's submit to its GPU work's end.
//! - **Limited:** `gpu` where more than half the frames read waited [`WAITED`] or longer for the frame two back, so
//!   the GPU set the pace, `cpu` where the CPU mostly found it done, so the GPU waited for the CPU's submits. This one
//!   is read on the CPU's clock alone, and needs no lining up: a balanced pair, the GPU's frame about the CPU's, can
//!   keep a frame queued behind the last while neither waits, so how much is queued does not decide it. With overlap,
//!   a CPU-limited tier is what reopens a render thread (m6-plan §3, "What not to do").
//!
//! The submit is read just after the frame's CPU span, with nothing on it but the scene's end of frame; a backend
//! whose GPU clock drifts against the CPU's moves every figure with it, a few microseconds over a phase of seconds at
//! a few parts in a million. Whether a wait by submission index behaves alike everywhere is held by the renderer's GPU
//! test `graph::waiting_on_a_submission_two_frames_back_reads_its_timings` on each backend it runs on.

use std::time::Duration;

use sapphire_rhi::wgpu;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;

/// A frame the GPU idled this long or longer before met an idle GPU: back to back, an Apple GPU's frames are a few
/// hundred microseconds apart on their timed passes, its untimed copies and the turnaround between command buffers.
pub(crate) const IDLE_FRAME: Duration = Duration::from_millis(1);

/// The fewest frames whose waits, or whose lags after an idle GPU, line the clocks up.
pub(crate) const LINED_UP_FEWEST: usize = 8;

/// A frame whose CPU waited this long or longer for the frame two back waited on the GPU: a wait on a submission
/// already done takes a few microseconds.
pub(crate) const WAITED: Duration = Duration::from_micros(100);

/// How a bench waits for the GPU after each frame's submit: for everything, or overlapping, for the frame graph's
/// submission two frames back.
pub(crate) struct GpuWait {
    overlap: Option<Overlap>,
}

struct Overlap {
    /// The bench's clock, which each submit is read on.
    clock: Stopwatch,
    /// The frame graph's submissions of the last two frames, the older first.
    in_flight: [Option<wgpu::SubmissionIndex>; 2],
}

impl GpuWait {
    /// `--bench-overlap`'s, or the default's.
    pub(crate) fn new(overlap: bool) -> Self {
        Self { overlap: overlap.then(|| Overlap { clock: Stopwatch::start(), in_flight: [None, None] }) }
    }

    /// Whether it overlaps (`--bench-overlap`).
    pub(crate) fn overlaps(&self) -> bool {
        self.overlap.is_some()
    }

    /// Overlapping, starts a phase from an empty queue: waits for everything in flight, so its first frame meets an
    /// idle GPU, which lines the reading's clocks up. Waits for nothing by default, where each frame drained it.
    pub(crate) fn drain(&mut self, device: &wgpu::Device) -> Result<(), wgpu::PollError> {
        let Some(overlap) = &mut self.overlap else { return Ok(()) };
        overlap.in_flight = [None, None];
        device.poll(wgpu::PollType::wait_indefinitely()).map(drop)
    }

    /// After a frame's submit, `submitted` its frame graph's ([`GraphExecutor::submitted`]): waits for everything
    /// by default; overlapping, for the graph's submission two frames before this one, if there was one. Returns,
    /// overlapping, when the frame's CPU span ended on the bench's clock.
    ///
    /// [`GraphExecutor::submitted`]: sapphire_render::graph::GraphExecutor::submitted
    pub(crate) fn wait(
        &mut self,
        device: &wgpu::Device,
        submitted: Option<&wgpu::SubmissionIndex>,
    ) -> Result<Option<Duration>, wgpu::PollError> {
        let Some(overlap) = &mut self.overlap else {
            return device.poll(wgpu::PollType::wait_indefinitely()).map(|_| None);
        };
        let at = overlap.clock.elapsed();
        let [two_back, one_back] = std::mem::take(&mut overlap.in_flight);
        overlap.in_flight = [one_back, submitted.cloned()];
        if let Some(index) = two_back {
            device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None })?;
        }
        Ok(Some(at))
    }
}

/// One frame of an overlapped phase, as [`read`] takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sample {
    /// When its CPU span ended, at its submit, on the bench's clock ([`GpuWait::wait`]).
    pub submitted: Duration,
    /// Its CPU span, and the wait after it.
    pub cpu: Duration,
    pub wait: Duration,
    /// Its first timed pass's start and its last one's end on the GPU's clock (`GpuTimings::start`, `end`), where its
    /// timings came back with every timed pass read; `None` leaves it out, and the frame after it unread.
    pub gpu: Option<(Duration, Duration)>,
}

/// What [`read`] reads over an overlapped phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reading {
    /// Frames read: those with GPU times following a frame with GPU times.
    pub frames: usize,
    /// The phase's frames, and those whose GPU times came back: a frame overlapped without them leaves out its own
    /// reading and the next one's.
    pub phase_frames: usize,
    pub timed: usize,
    /// Of them, those whose CPU waited [`WAITED`] or longer for the frame two back.
    pub waited: usize,
    /// What lined the clocks up (`waits`, `idle` or `least`), and how many frames did.
    pub lined_up_by: &'static str,
    pub lined_up: usize,
    /// The GPU work ahead of a frame at its submit.
    pub behind_p50: Duration,
    pub behind_p95: Duration,
    /// The queue's age, a submit to its GPU end.
    pub age_p50: Duration,
    pub age_p95: Duration,
    pub age_worst: Duration,
    /// From one frame's GPU end to the next's: the GPU's frame interval.
    pub interval_p50: Duration,
    pub cpu_p50: Duration,
    pub wait_p50: Duration,
}

impl Reading {
    /// `gpu` where more than half the frames read waited on the GPU, `cpu` otherwise.
    pub(crate) fn limited(&self) -> &'static str {
        if self.waited * 2 > self.frames { "gpu" } else { "cpu" }
    }
}

/// Reads `frames`, an overlapped phase's in order. `None` with no two frames in a row timed on the GPU.
pub(crate) fn read(frames: &[Sample]) -> Option<Reading> {
    let nanos = |duration: Duration| duration.as_nanos() as i128;
    let span = |nanos: i128| Duration::from_nanos(u64::try_from(nanos).unwrap_or(0));
    let lag = |frame: &Sample| Some(nanos(frame.gpu?.0) - nanos(frame.submitted));
    // What puts submits on the GPU's clock: the most a wait's return trails its frame's end by, ...
    let waits: Vec<i128> = frames
        .windows(3)
        .filter(|three| three[2].wait >= WAITED)
        .filter_map(|three| Some(nanos(three[0].gpu?.1) - nanos(three[2].submitted + three[2].wait)))
        .collect();
    // ... or the median lag of the frames that met an idle GPU, or the least lag.
    let mut idle: Vec<i128> = frames
        .windows(2)
        .filter(|pair| {
            pair[0]
                .gpu
                .zip(pair[1].gpu)
                .is_some_and(|((_, last_end), (start, _))| start >= last_end + IDLE_FRAME)
        })
        .filter_map(|pair| lag(&pair[1]))
        .collect();
    idle.sort_unstable();
    let (lined_up_by, lined_up, lag) = match (waits.len(), idle.len()) {
        (count, _) if count >= LINED_UP_FEWEST => ("waits", count, waits.into_iter().max()?),
        (_, count) if count >= LINED_UP_FEWEST => ("idle", count, idle[count / 2]),
        _ => ("least", 1, frames.iter().filter_map(lag).min()?),
    };
    let (mut behind, mut age, mut interval) = (Vec::new(), Vec::new(), Vec::new());
    let (mut cpu, mut wait) = (Vec::new(), Vec::new());
    let mut waited = 0;
    for pair in frames.windows(2) {
        let (Some((_, last_end)), Some((_, end))) = (pair[0].gpu, pair[1].gpu) else { continue };
        let submitted = nanos(pair[1].submitted) + lag;
        waited += usize::from(pair[1].wait >= WAITED);
        behind.push(span(nanos(last_end) - submitted));
        age.push(span(nanos(end) - submitted));
        interval.push(end.saturating_sub(last_end));
        cpu.push(pair[1].cpu);
        wait.push(pair[1].wait);
    }
    let at = |values: &mut Vec<Duration>, percent: usize| {
        values.sort_unstable();
        values[(values.len() - 1) * percent / 100]
    };
    (!behind.is_empty()).then(|| Reading {
        frames: behind.len(),
        phase_frames: frames.len(),
        timed: frames.iter().filter(|frame| frame.gpu.is_some()).count(),
        waited,
        lined_up_by,
        lined_up,
        behind_p50: at(&mut behind, 50),
        behind_p95: at(&mut behind, 95),
        age_p50: at(&mut age, 50),
        age_p95: at(&mut age, 95),
        age_worst: at(&mut age, 100),
        interval_p50: at(&mut interval, 50),
        cpu_p50: at(&mut cpu, 50),
        wait_p50: at(&mut wait, 50),
    })
}

/// Logs `phase`'s reading as `bench overlap`, or that there was none to take.
pub(crate) fn log(phase: &'static str, tier: &'static str, frames: &[Sample]) {
    let Some(reading) = read(frames) else {
        tracing::info!(target: LOG_TARGET, phase, tier, frames = 0, "bench overlap: no two frames in a row timed");
        return;
    };
    let micros = |duration: Duration| format!("{:.1}", duration.as_secs_f64() * 1e6);
    tracing::info!(
        target: LOG_TARGET,
        phase,
        tier,
        frames = reading.frames,
        phase_frames = reading.phase_frames,
        timed = reading.timed,
        limited = reading.limited(),
        waited_pct = format_args!("{:.1}", 100.0 * reading.waited as f64 / reading.frames as f64),
        lined_up_by = reading.lined_up_by,
        lined_up = reading.lined_up,
        behind_us_p50 = micros(reading.behind_p50),
        behind_us_p95 = micros(reading.behind_p95),
        queue_age_us_p50 = micros(reading.age_p50),
        queue_age_us_p95 = micros(reading.age_p95),
        queue_age_us_worst = micros(reading.age_worst),
        gpu_interval_us_p50 = micros(reading.interval_p50),
        cpu_us_p50 = micros(reading.cpu_p50),
        wait_us_p50 = micros(reading.wait_p50),
        "bench overlap",
    );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Sample, read};

    const MS: Duration = Duration::from_millis(1);

    /// An overlapped phase as a queue runs it, on a GPU clock `offset` ahead of the bench's: each frame's CPU span
    /// is `cpu` long, it waits for the frame two back to end on the GPU, and the GPU starts a frame `latency` after
    /// its submit or when the last one ended, whichever is later, and runs it `span` long.
    fn frames(
        count: usize,
        cpu: Duration,
        span: Duration,
        latency: Duration,
        offset: Duration,
    ) -> Vec<Sample> {
        let (mut begin, mut ends) = (Duration::ZERO, Vec::<Duration>::new());
        let mut frames = Vec::new();
        for frame in 0..count {
            let submitted = begin + cpu;
            let start = (submitted + latency).max(ends.last().copied().unwrap_or_default());
            ends.push(start + span);
            let next = frame.checked_sub(2).map_or(submitted, |two_back| submitted.max(ends[two_back]));
            let gpu = Some((start + offset, start + span + offset));
            frames.push(Sample { submitted, cpu, wait: next - submitted, gpu });
            begin = next;
        }
        frames
    }

    /// A GPU slower than the CPU has the frames before still queued at every submit, two frames' work less the CPU's
    /// span ahead, and the CPU waits out the difference; a faster one has finished each frame before the next is
    /// submitted, so nothing is ahead, the CPU never waits, and a frame's age is its own span.
    #[test]
    fn a_slow_gpu_reads_gpu_limited_and_a_fast_one_cpu_limited() {
        let offset = Duration::from_secs(1_000);
        let latency = Duration::from_micros(30);
        let bound = read(&frames(40, 4 * MS, 6 * MS, latency, offset)).unwrap();
        assert_eq!(
            (bound.limited(), bound.frames, bound.waited),
            ("gpu", 39, 36),
            "the queue fills by frame 4"
        );
        assert_eq!((bound.lined_up_by, bound.lined_up), ("waits", 36));
        assert_eq!(bound.behind_p50, 2 * 6 * MS - 4 * MS, "two frames' work less the CPU's span");
        assert_eq!(bound.age_p50, 3 * 6 * MS - 4 * MS, "the work ahead and its own");
        assert_eq!(bound.interval_p50, 6 * MS, "the GPU's own pace");
        assert_eq!(bound.wait_p50, 6 * MS - 4 * MS, "the CPU waits out the frame two back");

        let fed = read(&frames(40, 6 * MS, 4 * MS, latency, offset)).unwrap();
        assert_eq!((fed.limited(), fed.waited, fed.lined_up_by, fed.lined_up), ("cpu", 0, "idle", 39));
        assert_eq!(fed.behind_p50, Duration::ZERO);
        assert_eq!(fed.age_p50, 4 * MS, "its own span, the latency lined up away");
        assert_eq!(fed.interval_p50, 6 * MS, "the CPU's pace");
        assert_eq!(fed.wait_p50, Duration::ZERO, "the frame two back ended long ago");
    }

    /// The reading needs two frames in a row timed: an untimed frame leaves the next one out, not the run.
    #[test]
    fn a_frame_with_no_gpu_times_leaves_out_the_one_after_it() {
        let mut timed = frames(10, 6 * MS, 4 * MS, Duration::ZERO, Duration::ZERO);
        timed[4].gpu = None;
        let reading = read(&timed).unwrap();
        assert_eq!(reading.frames, 7, "9 pairs less the two the gap breaks");
        assert_eq!((reading.phase_frames, reading.timed), (10, 9));
        for frame in &mut timed {
            frame.gpu = None;
        }
        assert!(read(&timed).is_none());
        assert!(read(&[]).is_none());
    }

    /// A frame submitted, on the GPU's clock, before the frame before it ended was queued behind it; one submitted
    /// after met an idle GPU. With too few frames that met one, submits are placed by the least lag, here the first
    /// frame's 1 ms.
    #[test]
    fn a_frame_submitted_before_the_last_one_ended_was_queued() {
        let sample = |submitted: Duration, start: Duration, end: Duration| Sample {
            submitted,
            gpu: Some((start, end)),
            ..Sample::default()
        };
        let three = [
            sample(Duration::ZERO, MS, 10 * MS),
            sample(8 * MS, 10 * MS, 20 * MS),
            sample(21 * MS, 22 * MS, 30 * MS),
        ];
        let reading = read(&three).unwrap();
        assert_eq!((reading.frames, reading.lined_up_by), (2, "least"));
        assert_eq!(reading.behind_p50, Duration::ZERO, "the lower of 1 ms (9 against 10) and none");
        assert_eq!(reading.age_worst, 11 * MS, "placed at 9 ms, done at 20");
    }

    /// A submit read late, as a busy machine's can be, starts its frame early on the bench's clock: the least lag
    /// would place every submit that much early. The median of the frames that met an idle GPU holds.
    #[test]
    fn one_submit_read_late_does_not_move_the_others() {
        let latency = Duration::from_micros(30);
        let mut late = frames(40, 6 * MS, 4 * MS, latency, Duration::ZERO);
        late[20].submitted += 3 * MS;
        let reading = read(&late).unwrap();
        assert_eq!((reading.limited(), reading.lined_up_by), ("cpu", "idle"));
        assert_eq!(reading.behind_p95, Duration::ZERO, "nothing ahead of the frames read on time");
        assert_eq!(reading.age_p50, 4 * MS);
    }

    /// A wait's return read late only lowers its own bound on the clocks' gap, so the most of them holds.
    #[test]
    fn one_wait_read_late_does_not_move_the_others() {
        let on_time = read(&frames(40, 4 * MS, 6 * MS, Duration::from_micros(30), Duration::ZERO)).unwrap();
        let mut late = frames(40, 4 * MS, 6 * MS, Duration::from_micros(30), Duration::ZERO);
        late[20].wait += 3 * MS;
        let late = read(&late).unwrap();
        assert_eq!(late.lined_up_by, "waits");
        assert_eq!((late.behind_p50, late.age_p50), (on_time.behind_p50, on_time.age_p50));
    }
}
