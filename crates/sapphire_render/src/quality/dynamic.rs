//! Dynamic resolution (spec §5.12 "Quality tiers", §6.3's first response to heat): a controller
//! that lowers the render scale below the tier's base while the GPU frame runs over its budget,
//! and raises it back once there is room.
//!
//! # Model
//!
//! GPU time is taken to grow with the scene's pixel count, so each sample gives a *load*: time
//! per scene pixel, smoothed over samples. The scale moves in steps of [`STEP`] of the base (5%),
//! never below the tier's floor, and the controller holds the predicted time in a band:
//!
//! - **Down** when the smoothed time is over the budget: straight to the largest step whose
//!   predicted time is under the low-water mark (85% of the budget), at most three steps at
//!   once, so one bad estimate can't throw the picture to the floor.
//! - **Up** one step at a time, only when the step up is predicted to stay under the low-water
//!   mark, and only after the scale has held for half a second. A step up that fits the band
//!   can't trigger the next step down, which is why the controller doesn't oscillate: time also
//!   has a part that doesn't scale with pixels (fixed per-pass costs), which makes every
//!   prediction of a step up pessimistic and of a step down optimistic, so it settles from above
//!   instead of overshooting.
//! - **After a change** it decides nothing until a sample of a frame drawn at the new scale has
//!   come back: GPU timings arrive one to three frames late, and judging the new scale by old
//!   frames is what makes naive controllers ring.
//!
//! Steps keep the frame graph's transient pool from making new textures for every frame: each
//! step is one new set of scene-sized targets, and the pool releases sizes left unused for 60
//! frames (`sapphire_rhi::EVICT_AFTER_FRAMES`).
//!
//! # Signals
//!
//! - **GPU time** ([`ResolutionSignal::GpuTime`]): the frame graph's pass span
//!   ([`GpuTimings::span`](crate::graph::GpuTimings::span)), against the tier's GPU frame
//!   budget. It needs timestamp queries and [`GraphExecutor::set_gpu_timing`] on. While the first
//!   timings of a new executor are still on their way (at startup, after device loss) the
//!   controller waits rather than falling back: those frames' lengths include loading.
//! - **Frame time** ([`ResolutionSignal::FrameTime`]), only on devices without timestamp
//!   queries: the CPU's frame time against the tier's frame. A vsync-paced frame sits at the
//!   budget whatever its headroom, so this signal counts only a missed vsync as over (115%), and
//!   raises the scale only when frames come in well under the budget (without vsync); on a paced
//!   display a lowered scale holds. It can't tell a CPU-bound frame from a GPU-bound one either,
//!   and the app passes no length for frames that weren't steady (nothing presented, or a GPU
//!   stack or renderer rebuilt).
//!
//! [`GraphExecutor::set_gpu_timing`]: crate::graph::GraphExecutor::set_gpu_timing

use std::time::Duration;

use crate::graph::GraphExecutor;

/// The scale moves in steps of this fraction of the base.
pub const STEP: f32 = 0.05;
/// How far each sample moves the smoothed load toward itself.
const SMOOTHING: f32 = 0.2;
/// Predicted times must stay under this fraction of the budget for the scale to rise, and a
/// drop aims under it.
const LOW_WATER: f32 = 0.85;
/// Where the frame-time signal counts as over: a vsync-paced frame lands right on its budget.
const FRAME_TIME_HIGH_WATER: f32 = 1.15;
/// The most steps one decision drops.
const MAX_DROP_STEPS: u32 = 3;
/// Frames the scale holds after a change before it may rise.
const RAISE_HOLD_FRAMES: u64 = 30;
/// Frames whose scales are remembered for timings that come back late.
const HISTORY: usize = 8;

/// What a controller follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionSignal {
    /// The frame graph's GPU pass span, against the GPU frame budget.
    GpuTime,
    /// The CPU frame time, against the frame budget, where the device has no GPU timings.
    FrameTime,
}

/// What a controller holds the frame to: the tier's budgets and how far it may lower the scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolutionBudget {
    /// The GPU frame (spec §3.1's GPU frame column).
    pub gpu_frame: Duration,
    /// The whole frame at the tier's frame rate.
    pub frame: Duration,
    /// The lowest fraction of the base scale it renders at, in (0, 1].
    pub floor: f32,
}

/// One frame's measurements, as the app has them before recording the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTiming {
    /// The frame about to be recorded, counted the way [`GraphExecutor::frames`] counts
    /// executed graphs (the next one is `frames() + 1`).
    pub frame: u64,
    /// The newest GPU timings: the frame they measured and its GPU time. `None` while the
    /// device has no timestamp queries or timing is off, and until a new executor's first
    /// timings come back.
    pub gpu: Option<(u64, Duration)>,
    /// The CPU time of the last frame, start to start. Zero for a frame that wasn't steady
    /// (nothing presented, or something rebuilt), which then counts for nothing.
    pub frame_time: Duration,
}

impl FrameTiming {
    /// The measurements `executor`, which ran the frames so far, has for the next one.
    pub fn new(executor: &GraphExecutor, frame_time: Duration) -> Self {
        Self {
            frame: executor.frames() + 1,
            gpu: executor.gpu_timings().map(|timings| (timings.frame, timings.span())),
            frame_time,
        }
    }
}

/// A dynamic-resolution controller (see the [module docs](self)). It is the hook later tuning
/// builds on: thermal input, per-pass budgets and an upscaler's own ranges replace the inputs and
/// constants, not the shape.
#[derive(Clone, Debug)]
pub struct DynamicResolution {
    budget: ResolutionBudget,
    /// The device times GPU passes, so the controller follows GPU time alone.
    gpu_timings: bool,
    enabled: bool,
    /// Steps below the base: the factor is `1 − level × STEP`, at least the floor.
    level: u32,
    max_level: u32,
    signal: Option<ResolutionSignal>,
    /// Smoothed seconds per scene pixel for `signal`.
    load: Option<f32>,
    /// Recent frames: number, factor and scene pixels.
    history: [(u64, f32, f32); HISTORY],
    /// The newest frame a sample was taken from.
    sampled: u64,
    /// The frame the factor last changed at.
    changed_at: u64,
    changes: u64,
}

/// A controller's state, for the stats overlay (rule R9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicResolutionStats {
    pub enabled: bool,
    /// The fraction of the base scale it renders at.
    pub factor: f32,
    pub floor: f32,
    /// What it follows, once it has a sample.
    pub signal: Option<ResolutionSignal>,
    /// The time it predicts for the current scale.
    pub estimate: Option<Duration>,
    /// Its band for the signal (GPU time until it has one): it drops the scale over `high` and
    /// raises it only while the step up stays under `low`.
    pub low: Duration,
    pub high: Duration,
    /// Scale changes since it was made.
    pub changes: u64,
}

impl DynamicResolution {
    /// A controller holding frames to `budget`, off until [`Self::set_enabled`]. `gpu_timings`
    /// says the device has timestamp queries ([`DeviceProfile::timestamps`]), so it follows GPU
    /// time; without them it follows the frame time.
    ///
    /// [`DeviceProfile::timestamps`]: super::DeviceProfile::timestamps
    pub fn new(budget: ResolutionBudget, gpu_timings: bool) -> Self {
        let floor = if budget.floor.is_finite() { budget.floor.clamp(f32::EPSILON, 1.0) } else { 1.0 };
        // Enough steps to reach the floor; the last one stops at it.
        let max_level = ((1.0 - floor) / STEP - 1e-4).ceil().max(0.0) as u32;
        Self {
            budget: ResolutionBudget { floor, ..budget },
            gpu_timings,
            enabled: false,
            level: 0,
            max_level,
            signal: None,
            load: None,
            history: [(0, 1.0, 0.0); HISTORY],
            sampled: 0,
            changed_at: 0,
            changes: 0,
        }
    }

    /// Turns the controller on or off. Off, it renders at the base scale and forgets its
    /// estimate.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled && self.level != 0 {
            self.changes += 1;
        }
        self.level = 0;
        self.signal = None;
        self.load = None;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Takes the frame's measurements and returns the fraction of the base scale to render
    /// `timing.frame` at, in `[floor, 1]`. `base_pixels` is the scene's pixel count at the
    /// base scale. Call it once per recorded frame; it allocates nothing.
    ///
    /// Frame numbers that go backwards mean a new frame graph executor (after device loss): the
    /// controller keeps its scale and estimate and starts counting frames again.
    pub fn update(&mut self, timing: &FrameTiming, base_pixels: f32) -> f32 {
        let newest = self.history.iter().map(|&(frame, ..)| frame).max().unwrap_or(0);
        if timing.frame < newest {
            self.history = [(0, 1.0, 0.0); HISTORY];
            self.sampled = 0;
            self.changed_at = 0;
        }
        if self.enabled
            && let Some((signal, time, pixels)) = self.sample(timing)
        {
            self.observe(signal, time, pixels);
            self.decide(timing.frame, base_pixels);
        }
        let factor = self.factor();
        let slot = (timing.frame % HISTORY as u64) as usize;
        self.history[slot] = (timing.frame, factor, base_pixels * factor * factor);
        factor
    }

    /// The fraction of the base scale the controller renders at now.
    pub fn factor(&self) -> f32 {
        self.factor_at(self.level)
    }

    pub fn stats(&self) -> DynamicResolutionStats {
        let (low, high) = self.water_marks(self.signal.unwrap_or(ResolutionSignal::GpuTime));
        let estimate = self.load.zip(self.last_pixels()).map(|(load, pixels)| load * pixels);
        DynamicResolutionStats {
            enabled: self.enabled,
            factor: self.factor(),
            floor: self.budget.floor,
            signal: self.signal,
            estimate: estimate.map(Duration::from_secs_f32),
            low: Duration::from_secs_f32(low),
            high: Duration::from_secs_f32(high),
            changes: self.changes,
        }
    }

    fn factor_at(&self, level: u32) -> f32 {
        (1.0 - level as f32 * STEP).max(self.budget.floor)
    }

    /// The pixels of the newest remembered frame.
    fn last_pixels(&self) -> Option<f32> {
        self.history
            .iter()
            .max_by_key(|(frame, ..)| *frame)
            .map(|&(_, _, pixels)| pixels)
            .filter(|&p| p > 0.0)
    }

    /// The newest measurement not used yet, with the scene pixels of the frame it measured.
    fn sample(&mut self, timing: &FrameTiming) -> Option<(ResolutionSignal, Duration, f32)> {
        let (signal, frame, time) = match timing.gpu {
            Some((frame, time)) => (ResolutionSignal::GpuTime, frame, time),
            // Timings are on their way; frame lengths now would include whatever held them up.
            None if self.gpu_timings => return None,
            // The last frame's length, which the frame before this one took.
            None => (ResolutionSignal::FrameTime, timing.frame.checked_sub(1)?, timing.frame_time),
        };
        if frame <= self.sampled || time.is_zero() {
            return None;
        }
        self.sampled = frame;
        // Frames older than the history are too old to say what they rendered at.
        let &(_, _, pixels) = self.history.iter().find(|(remembered, ..)| *remembered == frame)?;
        (pixels > 0.0).then_some((signal, time, pixels))
    }

    fn observe(&mut self, signal: ResolutionSignal, time: Duration, pixels: f32) {
        if self.signal != Some(signal) {
            self.signal = Some(signal);
            self.load = None;
        }
        let load = time.as_secs_f32() / pixels;
        self.load = Some(match self.load {
            Some(smoothed) => smoothed + (load - smoothed) * SMOOTHING,
            None => load,
        });
    }

    fn decide(&mut self, frame: u64, base_pixels: f32) {
        // Judge the current scale only by frames drawn at it.
        if self.sampled < self.changed_at {
            return;
        }
        let (Some(signal), Some(load)) = (self.signal, self.load) else { return };
        let (low, high) = self.water_marks(signal);
        let time_at = |level: u32| load * base_pixels * self.factor_at(level).powi(2);
        let next = if time_at(self.level) > high {
            // The first step down whose predicted time is under the low-water mark.
            let deepest = (self.level + MAX_DROP_STEPS).min(self.max_level);
            (self.level + 1..=deepest).find(|&level| time_at(level) <= low).unwrap_or(deepest)
        } else if self.level > 0
            && frame >= self.changed_at + RAISE_HOLD_FRAMES
            && time_at(self.level - 1) <= low
        {
            self.level - 1
        } else {
            self.level
        };
        if next != self.level {
            self.level = next;
            self.changed_at = frame;
            self.changes += 1;
        }
    }

    /// The band for `signal`, in seconds: (low, high).
    fn water_marks(&self, signal: ResolutionSignal) -> (f32, f32) {
        match signal {
            ResolutionSignal::GpuTime => {
                let budget = self.budget.gpu_frame.as_secs_f32();
                (budget * LOW_WATER, budget)
            }
            ResolutionSignal::FrameTime => {
                let budget = self.budget.frame.as_secs_f32();
                (budget * LOW_WATER, budget * FRAME_TIME_HIGH_WATER)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: ResolutionBudget = ResolutionBudget {
        gpu_frame: Duration::from_millis(12),
        frame: Duration::from_micros(16_667),
        floor: 0.7,
    };
    /// A 1080p scene at the base scale.
    const BASE_PIXELS: f32 = 1920.0 * 1080.0;

    /// A GPU whose frame takes `fixed` plus `per_pixel` for each scene pixel, and whose
    /// timings come back `delay` frames late.
    struct Gpu {
        fixed: f32,
        per_pixel: f32,
        delay: u64,
        /// Per frame: the time it took.
        times: Vec<Duration>,
    }

    impl Gpu {
        fn new(fixed_ms: f32, full_scale_ms: f32, delay: u64) -> Self {
            let per_pixel = (full_scale_ms - fixed_ms) / 1000.0 / BASE_PIXELS;
            Self { fixed: fixed_ms / 1000.0, per_pixel, delay, times: vec![Duration::ZERO] }
        }

        /// Runs frame `frame` at `factor` and returns the timing the app sees for the next.
        fn run(&mut self, frame: u64, factor: f32) -> FrameTiming {
            let pixels = BASE_PIXELS * factor * factor;
            self.times.push(Duration::from_secs_f32(self.fixed + self.per_pixel * pixels));
            let next = frame + 1;
            let measured = next.checked_sub(1 + self.delay).filter(|&f| f >= 1);
            FrameTiming {
                frame: next,
                gpu: measured.map(|f| (f, self.times[f as usize])),
                frame_time: BUDGET.frame,
            }
        }
    }

    /// Runs `frames` frames from `timing`, changing the load with `load(frame)` (a multiplier of
    /// the per-pixel cost), and returns each frame's factor.
    fn drive(
        controller: &mut DynamicResolution,
        gpu: &mut Gpu,
        frames: std::ops::Range<u64>,
        load: impl Fn(u64) -> f32,
        timing: &mut FrameTiming,
    ) -> Vec<f32> {
        let base = gpu.per_pixel;
        let mut factors = Vec::new();
        for frame in frames {
            gpu.per_pixel = base * load(frame);
            let factor = controller.update(timing, BASE_PIXELS);
            factors.push(factor);
            *timing = gpu.run(frame, factor);
        }
        gpu.per_pixel = base;
        factors
    }

    fn changes(factors: &[f32]) -> usize {
        factors.windows(2).filter(|pair| pair[0] != pair[1]).count()
    }

    #[test]
    fn off_it_renders_at_the_base_scale() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        let mut gpu = Gpu::new(1.0, 30.0, 2);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        let factors = drive(&mut controller, &mut gpu, 1..200, |_| 1.0, &mut timing);
        assert!(factors.iter().all(|&factor| factor == 1.0), "an overloaded GPU, but the controller is off");
        assert_eq!(controller.stats().changes, 0);
    }

    #[test]
    fn a_load_step_converges_inside_the_band_without_oscillating() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        controller.set_enabled(true);
        // 9 ms at the base scale (under budget), then the per-pixel cost doubles at frame 100
        // (to about 17 ms) and drops back at frame 400.
        let mut gpu = Gpu::new(1.0, 9.0, 2);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        let load = |frame: u64| if (100..400).contains(&frame) { 2.0 } else { 1.0 };
        let factors = drive(&mut controller, &mut gpu, 1..700, load, &mut timing);

        assert!(factors[..100].iter().all(|&factor| factor == 1.0), "no change while under budget");
        assert!(factors.iter().all(|&factor| (BUDGET.floor..=1.0).contains(&factor)), "in bounds");
        // It reacts within a few frames of the step and settles at one scale, inside the band.
        let settled = factors[130];
        assert!(settled < 1.0);
        assert!(factors[130..400].iter().all(|&factor| factor == settled), "steady, no oscillation");
        let time = |factor: f32| 1.0 + 16.0 * factor * factor;
        assert!(time(settled) <= 12.0, "{settled}: {} ms is over budget", time(settled));
        assert!(time(settled + STEP) > 12.0 * LOW_WATER, "the step up would leave the band");
        assert!(changes(&factors[100..130]) <= 2, "it drops in at most two decisions");
        // Monotonic both ways: down after the step, up after the load goes back.
        assert!(factors[100..400].windows(2).all(|pair| pair[1] <= pair[0]));
        assert!(factors[400..].windows(2).all(|pair| pair[1] >= pair[0]));
        assert_eq!(*factors.last().unwrap(), 1.0, "back at the base scale");
    }

    #[test]
    fn an_impossible_load_stops_at_the_floor() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        controller.set_enabled(true);
        let mut gpu = Gpu::new(2.0, 60.0, 1);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        let factors = drive(&mut controller, &mut gpu, 1..120, |_| 1.0, &mut timing);
        assert_eq!(*factors.last().unwrap(), BUDGET.floor);
        assert!(factors.iter().all(|&factor| factor >= BUDGET.floor));
        // Steps of at most three at a time: 1.0, 0.85, 0.7.
        assert_eq!(changes(&factors), 2, "{factors:?}");
    }

    #[test]
    fn the_floor_need_not_be_a_whole_step() {
        let mut controller = DynamicResolution::new(ResolutionBudget { floor: 0.72, ..BUDGET }, true);
        controller.set_enabled(true);
        let mut gpu = Gpu::new(2.0, 60.0, 1);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        let factors = drive(&mut controller, &mut gpu, 1..120, |_| 1.0, &mut timing);
        assert_eq!(*factors.last().unwrap(), 0.72);
    }

    #[test]
    fn noise_around_the_budget_does_not_make_it_hunt() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        controller.set_enabled(true);
        // 12.5 ms at the base scale, ±8% of pseudo-random noise per frame.
        let mut gpu = Gpu::new(1.0, 12.5, 2);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        let mut state = 0x2545_f491_u32;
        let noise: Vec<f32> = (0..1000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                1.0 + 0.08 * (state as f32 / u32::MAX as f32 * 2.0 - 1.0)
            })
            .collect();
        let factors = drive(&mut controller, &mut gpu, 1..1000, |frame| noise[frame as usize], &mut timing);
        assert!(changes(&factors) <= 3, "{} changes", changes(&factors));
        assert!(factors[100..].iter().all(|&factor| factor == factors[100]));
    }

    #[test]
    fn stale_and_unknown_frames_are_ignored() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        controller.set_enabled(true);
        let slow = Duration::from_millis(40);
        // A timing of a frame from before the controller's history says nothing.
        let early = FrameTiming { frame: 50, gpu: Some((3, slow)), ..FrameTiming::default() };
        assert_eq!(controller.update(&early, BASE_PIXELS), 1.0);
        assert_eq!(controller.stats().signal, None);
        // A frame with no newer timing back gives nothing to go on.
        controller.update(&FrameTiming { frame: 51, ..FrameTiming::default() }, BASE_PIXELS);
        assert_eq!(controller.stats().signal, None);

        let timing = FrameTiming { frame: 52, gpu: Some((51, slow)), ..FrameTiming::default() };
        assert!(controller.update(&timing, BASE_PIXELS) < 1.0, "40 ms is far over the budget");
        let after_one = controller.stats();
        assert_eq!((after_one.signal, after_one.changes), (Some(ResolutionSignal::GpuTime), 1));
        // The same timing again, as when no newer frame has come back, counts once.
        controller.update(&FrameTiming { frame: 53, ..timing }, BASE_PIXELS);
        assert_eq!(controller.stats(), after_one);
    }

    #[test]
    fn frame_times_count_for_nothing_while_gpu_timings_are_on_their_way() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        controller.set_enabled(true);
        // The first frames of an executor take loading time, but GPU timings will come.
        for frame in 1..4 {
            let timing = FrameTiming { frame, gpu: None, frame_time: Duration::from_millis(400) };
            assert_eq!(controller.update(&timing, BASE_PIXELS), 1.0);
        }
        assert_eq!(controller.stats().signal, None);
        // Without timestamp queries the same frames do count, though a frame the app had no
        // steady length for (zero) doesn't.
        let mut untimed = DynamicResolution::new(BUDGET, false);
        untimed.set_enabled(true);
        untimed.update(&FrameTiming { frame: 1, ..FrameTiming::default() }, BASE_PIXELS);
        untimed.update(&FrameTiming { frame: 2, ..FrameTiming::default() }, BASE_PIXELS);
        assert_eq!(untimed.stats().signal, None);
        for frame in 3..6 {
            untimed.update(
                &FrameTiming { frame, gpu: None, frame_time: Duration::from_millis(400) },
                BASE_PIXELS,
            );
        }
        assert!(untimed.factor() < 1.0);
    }

    /// Feeds a frame time for `frame` with no GPU timings.
    fn frame_time(controller: &mut DynamicResolution, frame: u64, time: Duration) -> f32 {
        controller.update(&FrameTiming { frame, gpu: None, frame_time: time }, BASE_PIXELS)
    }

    #[test]
    fn frame_time_counts_only_missed_vsyncs_as_over() {
        // A device without timestamp queries.
        let mut controller = DynamicResolution::new(BUDGET, false);
        controller.set_enabled(true);
        // Paced at exactly the frame: hold.
        for frame in 1..200 {
            assert_eq!(frame_time(&mut controller, frame, BUDGET.frame), 1.0);
        }
        // Every frame misses a vsync: drop.
        let mut last = 1.0;
        for frame in 200..260 {
            last = frame_time(&mut controller, frame, BUDGET.frame * 2);
        }
        assert!(last < 1.0);
        assert_eq!(controller.stats().signal, Some(ResolutionSignal::FrameTime));
        // Back to paced frames: a lowered scale holds, since a paced frame shows no headroom.
        let mut held = 0.0;
        for frame in 260..600 {
            held = frame_time(&mut controller, frame, BUDGET.frame);
        }
        assert_eq!(held, last);
        // Frames well under budget (no vsync) let it rise again.
        for frame in 600..1200 {
            held = frame_time(&mut controller, frame, BUDGET.frame / 4);
        }
        assert_eq!(held, 1.0);
    }

    #[test]
    fn a_new_executor_starts_the_frame_count_again() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        controller.set_enabled(true);
        let mut gpu = Gpu::new(2.0, 60.0, 1);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        // Long enough for the first drop (frame 3), not the second.
        drive(&mut controller, &mut gpu, 1..5, |_| 1.0, &mut timing);
        let before = controller.stats();
        assert!(before.factor < 1.0 && before.factor > BUDGET.floor, "{}", before.factor);
        // After device loss the frames count from 1 again; their timings still count.
        let mut gpu = Gpu::new(2.0, 60.0, 1);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        let factors = drive(&mut controller, &mut gpu, 1..60, |_| 1.0, &mut timing);
        assert_eq!(factors[0], before.factor, "the scale carries over");
        assert_eq!(*factors.last().unwrap(), BUDGET.floor, "and keeps following the timings");
        assert!(controller.stats().changes > before.changes);
    }

    #[test]
    fn turning_it_off_restores_the_base_scale() {
        let mut controller = DynamicResolution::new(BUDGET, true);
        controller.set_enabled(true);
        let mut gpu = Gpu::new(2.0, 60.0, 1);
        let mut timing = FrameTiming { frame: 1, ..FrameTiming::default() };
        drive(&mut controller, &mut gpu, 1..60, |_| 1.0, &mut timing);
        assert!(controller.factor() < 1.0);
        controller.set_enabled(false);
        assert_eq!(controller.update(&timing, BASE_PIXELS), 1.0);
        assert_eq!(controller.stats().estimate, None);
    }
}
