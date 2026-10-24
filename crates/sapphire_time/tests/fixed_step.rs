//! Frame pacing behaviour of `FixedStepDriver` (spec §4.2, §5.7, §6.2).

use std::time::Duration;

use sapphire_time::{FixedStepConfig, FixedStepDriver, FixedSteps};

/// One display refresh at `hz`, truncated to whole nanoseconds as real timestamps are.
fn frame(hz: u64) -> Duration {
    Duration::from_nanos(1_000_000_000 / hz)
}

fn no_snapping() -> FixedStepConfig {
    FixedStepConfig { snap_tolerance: Duration::ZERO, ..FixedStepConfig::default() }
}

/// A small deterministic generator, so the property tests need no RNG dependency.
struct Lcg(u64);

impl Lcg {
    fn next_below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

#[test]
fn sixty_fps_runs_one_step_per_frame() {
    let mut driver = FixedStepDriver::default();
    for _ in 0..600 {
        let steps = driver.advance(frame(60));
        assert_eq!(steps.count, 1);
        assert_eq!(steps.dropped, Duration::ZERO);
    }
    assert_eq!(driver.ticks(), 600);
}

#[test]
fn thirty_fps_runs_two_steps_every_frame() {
    let mut driver = FixedStepDriver::default();
    for index in 0..300 {
        let steps = driver.advance(frame(30));
        assert_eq!(steps.count, 2, "frame {index}");
        assert_eq!(steps.alpha, 0.0, "frame {index}");
    }
}

#[test]
fn thirty_fps_with_vsync_jitter_stays_at_two_steps() {
    // Real jitter is in the timestamps: each frame starts up to ±90 µs off its vsync, so a delta
    // is off by the difference of two such errors and the errors never accumulate.
    let mut driver = FixedStepDriver::default();
    let mut rng = Lcg(7);
    let period = frame(30).as_nanos() as i64;
    let mut previous_stamp = 0i64;
    for index in 1..=10_000i64 {
        let stamp = index * period + rng.next_below(180_001) as i64 - 90_000;
        let delta = Duration::from_nanos((stamp - previous_stamp) as u64);
        previous_stamp = stamp;
        assert_eq!(driver.advance(delta).count, 2, "frame {index}");
    }
}

#[test]
fn alternating_long_and_short_frames_are_repaid() {
    // A 150 µs early frame followed by a 150 µs late one: the early step is repaid exactly.
    let mut driver = FixedStepDriver::default();
    for index in 0..1000 {
        let delta = if index % 2 == 0 {
            frame(30) - Duration::from_micros(150)
        } else {
            frame(30) + Duration::from_micros(150)
        };
        assert_eq!(driver.advance(delta).count, 2, "frame {index}");
    }
}

#[test]
fn without_snapping_thirty_fps_still_averages_two_steps() {
    let mut driver = FixedStepDriver::new(no_snapping());
    let counts: Vec<u32> = (0..300).map(|_| driver.advance(frame(30)).count).collect();
    // 33,333,333 ns is a hair under two ticks, so the very first frame runs one step.
    assert_eq!(counts[0], 1);
    assert!(counts[1..].iter().all(|&count| count == 2));
}

/// Presented time (ticks run plus `alpha`) must advance by the same amount every frame,
/// whatever the step pattern, or interpolated motion judders.
fn assert_smooth_presentation(hz: u64, frames: u32) -> u32 {
    let mut driver = FixedStepDriver::default();
    let expected_advance = 60.0 / hz as f64;
    let mut previous = 0.0;
    let mut total = 0;
    for index in 0..frames {
        let steps = driver.advance(frame(hz));
        total += steps.count;
        let presented = driver.ticks() as f64 + f64::from(steps.alpha);
        assert!((presented - previous - expected_advance).abs() < 1e-4, "{hz} Hz frame {index}: {steps:?}");
        previous = presented;
    }
    total
}

#[test]
fn one_twenty_hz_interpolates_evenly() {
    // 120 truncated frames are 40 ns short of one second, well inside the snap tolerance, so the
    // 60th step runs on time rather than one frame late.
    assert_eq!(assert_smooth_presentation(120, 120), 60);
}

#[test]
fn one_forty_four_hz_keeps_real_time() {
    // 1440 frames of 6,944,444 ns are 640 ns short of 10 s; again the last step runs.
    assert_eq!(assert_smooth_presentation(144, 1440), 600);
}

#[test]
fn without_snapping_steps_never_run_early() {
    let mut driver = FixedStepDriver::new(no_snapping());
    let total: u32 = (0..120).map(|_| driver.advance(frame(120)).count).sum();
    assert_eq!(total, 59);
    assert_eq!(driver.advance(Duration::from_nanos(40)).count, 1);
}

/// Runs one hour of frames at `millihertz` (59,940 for 59.94 Hz) and checks, every frame, that
/// simulated time stays within one tick behind and the snap tolerance ahead of wall time.
/// Returns how many frames ran each step count.
fn run_an_hour(millihertz: u64) -> [u64; 5] {
    let mut driver = FixedStepDriver::default();
    let tolerance = i128::from(FixedStepConfig::default().snap_tolerance.as_nanos() as u64) * 60;
    let tick: i128 = 1_000_000_000;
    let frame = Duration::from_nanos(1_000_000_000_000 / millihertz);
    let frames = millihertz * 3600 / 1000;
    let mut wall: i128 = 0;
    let mut histogram = [0u64; 5];
    for index in 0..frames {
        let steps = driver.advance(frame);
        histogram[steps.count as usize] += 1;
        wall += frame.as_nanos() as i128 * 60;
        let simulated = i128::from(driver.ticks()) * tick;
        assert!(
            simulated <= wall + tolerance,
            "{millihertz} mHz frame {index}: ahead by more than the tolerance"
        );
        assert!(simulated > wall - tick, "{millihertz} mHz frame {index}: behind by a tick or more");
    }
    histogram
}

#[test]
fn an_hour_at_59_94_hz_has_no_drift() {
    let histogram = run_an_hour(59_940);
    // Frames are 0.1 % longer than a tick: one step each, plus a second about every 17 s.
    assert_eq!(histogram[0], 0);
    assert_eq!(histogram[3] + histogram[4], 0);
    assert!((200..=230).contains(&histogram[2]), "{histogram:?}");
}

#[test]
fn an_hour_at_29_97_hz_has_no_drift() {
    let histogram = run_an_hour(29_970);
    assert_eq!(histogram[0] + histogram[1] + histogram[4], 0, "{histogram:?}");
    assert!((200..=230).contains(&histogram[3]), "{histogram:?}");
}

#[test]
fn an_hour_at_90_hz_has_no_drift() {
    let histogram = run_an_hour(90_000);
    // Two steps every three frames.
    assert_eq!(histogram[2] + histogram[3] + histogram[4], 0, "{histogram:?}");
    assert_eq!(histogram[1], 2 * histogram[0], "{histogram:?}");
}

#[test]
fn an_hour_at_120_hz_has_no_drift() {
    let histogram = run_an_hour(120_000);
    assert_eq!(histogram[0], histogram[1], "{histogram:?}");
    assert_eq!(histogram[2] + histogram[3] + histogram[4], 0, "{histogram:?}");
}

#[test]
fn long_run_has_no_drift() {
    // 7 ms frames (not a divisor of a tick) for an hour of game time.
    let mut driver = FixedStepDriver::new(no_snapping());
    let frames = 3600 * 1000 / 7;
    let total: u64 = (0..frames).map(|_| u64::from(driver.advance(Duration::from_millis(7)).count)).sum();
    let elapsed_ns = frames as u128 * 7_000_000;
    assert_eq!(u128::from(total), elapsed_ns * 60 / 1_000_000_000);
}

#[test]
fn step_cap_drops_backlog() {
    let mut driver = FixedStepDriver::default();
    let steps = driver.advance(Duration::from_millis(200));
    assert_eq!(steps.count, 4);
    // 200 ms is 12 ticks; 8 whole ticks are dropped and nothing carries into the next frame.
    assert_eq!(steps.dropped, Duration::from_nanos(133_333_333));
    assert_eq!(steps.alpha, 0.0);
    assert_eq!(driver.advance(frame(60)).count, 1);
}

#[test]
fn huge_deltas_are_clamped() {
    for real_delta in
        [Duration::from_secs(1), Duration::from_secs(30), Duration::from_secs(86_400), Duration::MAX]
    {
        let mut driver = FixedStepDriver::default();
        let steps = driver.advance(real_delta);
        assert_eq!(steps.count, 4);
        assert!(steps.alpha >= 0.0 && steps.alpha < 1.0);
        // Everything except the four ticks that ran is reported as dropped.
        let ran = Duration::from_nanos(66_666_666);
        assert!(steps.dropped >= real_delta - ran - Duration::from_nanos(1), "{real_delta:?}: {steps:?}");
        assert!(steps.dropped <= real_delta - ran, "{real_delta:?}: {steps:?}");
        // The next normal frame is unaffected.
        assert_eq!(driver.advance(frame(60)).count, 1);
    }
}

#[test]
fn max_steps_is_configurable() {
    let mut driver =
        FixedStepDriver::new(FixedStepConfig { max_steps_per_frame: 2, ..FixedStepConfig::default() });
    assert_eq!(driver.advance(Duration::from_millis(100)).count, 2);
    let mut driver =
        FixedStepDriver::new(FixedStepConfig { max_steps_per_frame: 8, ..FixedStepConfig::default() });
    assert_eq!(driver.advance(Duration::from_millis(100)).count, 6);
}

#[test]
fn alpha_is_always_in_unit_interval() {
    for config in [FixedStepConfig::default(), no_snapping()] {
        let mut driver = FixedStepDriver::new(config);
        let mut rng = Lcg(42);
        for _ in 0..100_000 {
            let steps: FixedSteps = driver.advance(Duration::from_nanos(rng.next_below(100_000_000)));
            assert!((0.0..1.0).contains(&steps.alpha), "{steps:?}");
            assert!(steps.count <= 4);
        }
    }
    let mut driver = FixedStepDriver::new(no_snapping());
    // Just short of a tick: the accumulator holds 10⁹ − 40 units, so alpha rounds toward 1.
    let steps = driver.advance(Duration::from_nanos(16_666_666));
    assert_eq!(steps.count, 0);
    assert!(steps.alpha < 1.0 && steps.alpha > 0.9999);
}

#[test]
fn zero_delta_runs_nothing() {
    let mut driver = FixedStepDriver::default();
    let steps = driver.advance(Duration::ZERO);
    assert_eq!(steps, FixedSteps { count: 0, alpha: 0.0, dropped: Duration::ZERO });
}
