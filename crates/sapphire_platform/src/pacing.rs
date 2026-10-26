//! Frame pacing (spec §6.2, ADR 0003): which display refreshes become frames, and how evenly they
//! come.
//!
//! [`FramePacer`] is pure: it reads no clock and is fed each display tick's vsync time, so its
//! decisions and counters are deterministic. The platform's frame clock feeds it: the engine's
//! display link on iOS, where it also holds back ticks that come before a frame is due, and the
//! time each frame begins elsewhere, where presentation paces frames and the pacer only measures.
//!
//! # Frames due on whole refreshes
//!
//! Where the display's refresh is known, the pacer counts in whole refreshes. Each frame owes the
//! target interval in refreshes (2 for 60 fps at 120 Hz, 2.4 at 144 Hz, 1.5 at 90 Hz) and is due
//! on the first refresh that pays it, and what a frame overpays carries to the next. So frames
//! keep the target rate on any display and never run faster: 60 fps at 144 Hz takes 3, 2, 3, 2, 2
//! refreshes, and at 90 Hz 2, 1, 2, 1. A frame later than its refresh starts the count afresh,
//! taking no time back with a short interval. Each interval is rounded to whole refreshes, so
//! timestamp noise can't move a decision and a reported refresh a little off the panel's can't
//! drift. Without a known refresh, a frame is due once the target interval has passed, within a
//! quarter of it.

use std::cmp::Ordering;
use std::num::NonZeroU32;
use std::time::Duration;

const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// How far over a whole number of refreshes a frame may owe and still be due on it. Periods are
/// whole nanoseconds, so a frame's owe is a little off (30 fps at 120 Hz owes 33_333_333 /
/// 8_333_333 refreshes, a sliver over 4), and the carry adds those slivers up over the frames of a
/// cadence (five at 144 Hz): a ten-thousandth of a refresh absorbs a thousand frames' worth.
const OWED_SLACK: f64 = 1e-4;

/// What starts each frame, and so what the target frame rate does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PacingSource {
    /// The engine's display link starts each frame on a vsync at the target rate, holding back any
    /// vsync that comes before the frame is due (iOS, ADR 0003).
    DisplayLink,
    /// SDL's loop runs frames back to back and presentation paces them, FIFO waiting for vsync
    /// (desktop and Android). The target rate is measured against, not enforced.
    Presentation,
}

/// How evenly frames have come against the target rate (rule R9). Counters run from startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PacingStats {
    /// Frames begun: the [`App::iterate`](crate::App::iterate) calls the pacer let through.
    pub frames: u64,
    /// Frames measured against the target: all but the first, those in the background and the
    /// first back, the one that ends a change of target, and those without a target.
    pub measured: u64,
    /// Measured frames that came on the refresh they were due (without a known refresh, within a
    /// quarter of the target interval of it).
    pub on_time: u64,
    /// Target intervals that passed without a new frame, so the display showed the frame before
    /// again: for each frame later than its refresh, the intervals it was late by, at least one.
    pub missed: u64,
    /// Measured frames that came before they were due, faster than the target rate: the judder of
    /// a 60 fps game on a display link running at 120 Hz. A display link holds such ticks back, so
    /// only presentation pacing counts them.
    pub doubled: u64,
    /// Display ticks held back so frames keep the target rate (display-link pacing only).
    pub held: u64,
    /// The latest frame's interval since the frame before.
    pub last_interval: Duration,
}

/// Turns display ticks into frames at the target rate and measures their intervals (see the
/// module docs).
#[derive(Clone, Debug)]
pub(crate) struct FramePacer {
    /// Hold back ticks that come before a frame is due (display-link pacing), or let every tick
    /// through and only measure (presentation pacing).
    gate: bool,
    target: Option<NonZeroU32>,
    /// The display's shortest refresh period, if known.
    refresh: Option<Duration>,
    /// The vsync time of the last frame let through, in nanoseconds.
    last: Option<u64>,
    /// What the frames so far have overpaid, in refreshes (0 up to 1), which the next one owes
    /// less.
    carry: f64,
    /// Between a trip to the background and the return: frames are counted, not measured.
    paused: bool,
    /// The next frame's interval spans a change of target, so it is held to the new target but
    /// not measured against it.
    retargeted: bool,
    stats: PacingStats,
}

/// Where a frame came against the refresh it was due on.
enum Landing {
    /// Before it was due: a display link holds the tick back; presentation counts it doubled.
    Early,
    /// On the refresh it was due, overpaying `carry` of a refresh.
    OnTime { carry: f64 },
    /// After it was due, by `missed` target intervals (at least one).
    Late { missed: u64 },
}

impl FramePacer {
    /// A pacer that holds back early ticks if `source` is a display link.
    pub fn new(source: PacingSource) -> Self {
        Self {
            gate: source == PacingSource::DisplayLink,
            target: None,
            refresh: None,
            last: None,
            carry: 0.0,
            paused: false,
            retargeted: false,
            stats: PacingStats::default(),
        }
    }

    /// Sets the frame rate to pace to, or `None` for every tick. Takes effect from the next tick;
    /// the frames after it are measured against the new interval.
    pub fn set_target(&mut self, target: Option<NonZeroU32>) {
        if target != self.target {
            self.target = target;
            self.retargeted = true;
            self.carry = 0.0;
        }
    }

    pub fn target(&self) -> Option<NonZeroU32> {
        self.target
    }

    /// Sets the display's refresh period (its shortest, on a variable-rate display), or `None`
    /// if unknown.
    pub fn set_refresh(&mut self, refresh: Option<Duration>) {
        self.refresh = refresh.filter(|refresh| !refresh.is_zero());
    }

    pub fn stats(&self) -> PacingStats {
        self.stats
    }

    /// Stops measuring: the app is leaving the foreground, where frames may stall or the app may
    /// sleep between them. Frames still pass and are counted, each given at most one target
    /// interval, so a tick that comes back from the background before [`Self::resume`] simulates
    /// none of the time away.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Starts afresh after [`Self::pause`]: the next tick becomes a frame at once, and the frames
    /// after it are measured again.
    pub fn resume(&mut self) {
        self.paused = false;
        self.last = None;
    }

    /// A display tick for the vsync at `vsync_ns` (nanoseconds on any monotonic clock, the same
    /// for every tick). Returns the frame's interval since the last frame, or `None` if the tick is
    /// held back. The first frame, and the first after [`Self::resume`], get one target interval
    /// (or one refresh), so time spent away isn't simulated.
    pub fn tick(&mut self, vsync_ns: u64) -> Option<Duration> {
        let nominal = self.nominal_interval();
        // A tick from before the last frame means the clock changed under the pacer: start afresh
        // rather than measure a negative interval.
        let Some(last) = self.last.filter(|&last| vsync_ns >= last) else {
            self.last = Some(vsync_ns);
            self.carry = 0.0;
            self.retargeted = false;
            self.stats.frames += 1;
            self.stats.last_interval = nominal;
            return Some(nominal);
        };
        let elapsed = Duration::from_nanos(vsync_ns - last);
        if let Some(interval) = self.interval() {
            let landing = match self.refresh {
                Some(refresh) => self.land_on_refreshes(elapsed, interval, refresh),
                None => land_in_time(elapsed, interval),
            };
            if self.gate && matches!(landing, Landing::Early) {
                self.stats.held += 1;
                return None;
            }
            if !self.paused && !self.retargeted {
                self.stats.measured += 1;
                match landing {
                    Landing::Early => self.stats.doubled += 1,
                    Landing::OnTime { .. } => self.stats.on_time += 1,
                    Landing::Late { missed } => self.stats.missed += missed,
                }
            }
            // A frame on its refresh keeps the cadence; any other starts it afresh.
            self.carry = match landing {
                Landing::OnTime { carry } => carry,
                Landing::Early | Landing::Late { .. } => 0.0,
            };
        }
        self.retargeted = false;
        self.last = Some(vsync_ns);
        let interval = if self.paused { elapsed.min(nominal) } else { elapsed };
        self.stats.frames += 1;
        self.stats.last_interval = interval;
        Some(interval)
    }

    /// The target interval, never shorter than a refresh: a display that can't reach the target
    /// rate shows every frame it can, on time. At least a nanosecond, whatever the target.
    fn interval(&self) -> Option<Duration> {
        let target = self.target?;
        let interval = Duration::from_nanos((NANOS_PER_SECOND / u64::from(target.get())).max(1));
        Some(self.refresh.map_or(interval, |refresh| interval.max(refresh)))
    }

    /// One frame's worth of time when there is no interval to measure: the target interval, else
    /// a refresh, else nothing.
    fn nominal_interval(&self) -> Duration {
        self.interval().or(self.refresh).unwrap_or_default()
    }

    /// The frame's interval in whole refreshes against the refresh it was due on (module docs).
    fn land_on_refreshes(&self, elapsed: Duration, interval: Duration, refresh: Duration) -> Landing {
        let refresh_ns = refresh.as_nanos();
        let refreshes = (elapsed.as_nanos() + refresh_ns / 2) / refresh_ns;
        let owed = interval.as_nanos() as f64 / refresh_ns as f64 - self.carry;
        let due = (owed - OWED_SLACK).ceil().max(1.0);
        // `due` is a small whole number: the cast is exact.
        match refreshes.cmp(&(due as u128)) {
            Ordering::Less => Landing::Early,
            Ordering::Equal => Landing::OnTime { carry: (due - owed).max(0.0) },
            Ordering::Greater => {
                let late = (refreshes - due as u128) * refresh_ns;
                let missed = (late + interval.as_nanos() / 2) / interval.as_nanos();
                Landing::Late { missed: u64::try_from(missed).unwrap_or(u64::MAX).max(1) }
            }
        }
    }
}

/// The frame's interval against the target interval, without a known refresh: due once it has
/// passed, within a quarter of it.
fn land_in_time(elapsed: Duration, interval: Duration) -> Landing {
    let tolerance = interval / 4;
    if elapsed + tolerance < interval {
        Landing::Early
    } else if elapsed > interval + tolerance {
        // The frame took this many target intervals; all but its own were missed.
        let intervals = (elapsed.as_nanos() + interval.as_nanos() / 2) / interval.as_nanos();
        Landing::Late { missed: u64::try_from(intervals).unwrap_or(u64::MAX).saturating_sub(1).max(1) }
    } else {
        Landing::OnTime { carry: 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(hz: u32) -> Option<NonZeroU32> {
        NonZeroU32::new(hz)
    }

    fn refresh(hz: u64) -> Duration {
        Duration::from_nanos(NANOS_PER_SECOND / hz)
    }

    /// A display link pacer at `target` fps on a display refreshing at `display_hz`.
    fn link(target: u32, display_hz: u64) -> FramePacer {
        let mut pacer = FramePacer::new(PacingSource::DisplayLink);
        pacer.set_target(rate(target));
        pacer.set_refresh(Some(refresh(display_hz)));
        pacer
    }

    /// Vsync times of `count` ticks at `hz`, from `start` nanoseconds: exact multiples of the
    /// period, as a display link reports them.
    fn vsyncs(start: u64, hz: u64, count: u64) -> impl Iterator<Item = u64> {
        (0..count).map(move |i| start + i * NANOS_PER_SECOND / hz)
    }

    /// Feeds `ticks` and returns the intervals of the frames let through, in microseconds.
    fn run(pacer: &mut FramePacer, ticks: impl IntoIterator<Item = u64>) -> Vec<u128> {
        ticks.into_iter().filter_map(|vsync| pacer.tick(vsync)).map(|interval| interval.as_micros()).collect()
    }

    /// Every frame after the first was measured and on time.
    fn all_on_time(pacer: &FramePacer) -> bool {
        let stats = pacer.stats();
        stats.measured == stats.frames - 1
            && stats.on_time == stats.measured
            && stats.missed + stats.doubled == 0
    }

    #[test]
    fn a_120_hz_link_runs_every_second_tick_at_60_fps() {
        let mut pacer = link(60, 120);
        let frames = run(&mut pacer, vsyncs(1_000_000, 120, 240));
        assert_eq!(frames.len(), 120);
        assert!(frames.iter().all(|&us| us == 16_666), "{frames:?}");
        let stats = pacer.stats();
        assert_eq!((stats.frames, stats.held), (120, 120));
        assert_eq!((stats.measured, stats.on_time, stats.missed, stats.doubled), (119, 119, 0, 0));
        assert_eq!(stats.last_interval.as_micros(), 16_666);
    }

    #[test]
    fn a_120_hz_link_runs_every_fourth_tick_at_30_fps() {
        let mut pacer = link(30, 120);
        let frames = run(&mut pacer, vsyncs(0, 120, 240));
        assert_eq!(frames.len(), 60);
        assert!(frames.iter().all(|&us| us == 33_333), "{frames:?}");
        let stats = pacer.stats();
        assert_eq!((stats.held, stats.on_time, stats.missed, stats.doubled), (180, 59, 0, 0));
    }

    #[test]
    fn a_60_hz_link_runs_every_tick_at_60_fps_and_every_second_at_30() {
        let mut pacer = link(60, 60);
        assert_eq!(run(&mut pacer, vsyncs(0, 60, 120)).len(), 120);
        assert_eq!((pacer.stats().held, pacer.stats().on_time), (0, 119));

        let mut pacer = link(30, 60);
        let frames = run(&mut pacer, vsyncs(0, 60, 120));
        assert_eq!(frames.len(), 60);
        assert!(frames[1..].iter().all(|&us| us == 33_333), "{frames:?}");
        assert_eq!((pacer.stats().held, pacer.stats().on_time, pacer.stats().doubled), (60, 59, 0));
    }

    #[test]
    fn a_link_already_at_the_target_rate_passes_every_tick() {
        // What iOS should deliver once the link's preferred rate is the target: one tick per frame.
        let mut pacer = link(30, 120);
        let frames = run(&mut pacer, vsyncs(0, 30, 90));
        assert_eq!(frames.len(), 90);
        assert_eq!((pacer.stats().held, pacer.stats().on_time), (0, 89));
    }

    #[test]
    fn a_144_hz_link_holds_60_fps_on_three_and_two_refreshes() {
        // Ten seconds: 2.4 refreshes a frame, taken 3, 2, 3, 2, 2, so 60 fps, never faster.
        let mut pacer = link(60, 144);
        let frames = run(&mut pacer, vsyncs(0, 144, 1440));
        assert_eq!(frames.len(), 600);
        assert_eq!(frames[1..6], [20_833, 13_888, 20_833, 13_888, 13_888]);
        assert!(frames[1..].iter().all(|&us| us == 13_888 || us == 20_833), "{frames:?}");
        assert!(all_on_time(&pacer), "{:?}", pacer.stats());
    }

    #[test]
    fn a_144_hz_link_holds_30_fps_on_five_and_four_refreshes() {
        // 4.8 refreshes a frame: 5, 5, 5, 5, 4.
        let mut pacer = link(30, 144);
        let frames = run(&mut pacer, vsyncs(0, 144, 1440));
        assert_eq!(frames.len(), 300);
        assert_eq!(frames[1..6], [34_722, 34_722, 34_722, 34_722, 27_777]);
        assert!(all_on_time(&pacer), "{:?}", pacer.stats());
    }

    #[test]
    fn a_90_hz_link_holds_60_fps_whatever_the_timestamps_noise() {
        // 1.5 refreshes a frame: 2, 1, 2, 1. The ticks are a nanosecond off the vsync either way,
        // which used to straddle the boundary between holding a tick and passing it.
        for noise in [1i64, -1] {
            let mut pacer = link(60, 90);
            let ticks = vsyncs(1_000, 90, 900)
                .enumerate()
                .map(|(i, vsync)| vsync.saturating_add_signed(if i % 2 == 0 { noise } else { -noise }));
            let frames = run(&mut pacer, ticks);
            assert_eq!(frames.len(), 600, "noise {noise}");
            let alternating = frames[1..].iter().enumerate();
            assert!(alternating.clone().all(|(i, &us)| us == [22_222, 11_111][i % 2]), "{frames:?}");
            assert!(all_on_time(&pacer), "{:?}", pacer.stats());
        }
    }

    #[test]
    fn a_late_tick_counts_the_intervals_it_missed_and_the_cadence_resumes() {
        let mut pacer = link(60, 60);
        let period = NANOS_PER_SECOND / 60;
        // Vsyncs 0..=9, then the main thread stalls through vsyncs 10 and 11, then 12..=20.
        let ticks = (0..=9).chain(12..=20).map(|i| i * period);
        let frames = run(&mut pacer, ticks);
        assert_eq!(frames[10], 49_999, "the stalled frame spans three vsyncs");
        let stats = pacer.stats();
        assert_eq!((stats.on_time, stats.missed, stats.doubled), (17, 2, 0));
    }

    #[test]
    fn a_late_frame_takes_no_time_back_with_a_short_one() {
        // 30 fps on a 60 Hz link that ticks every vsync but misses vsync 2: the frame one vsync
        // late, at 3, starts the cadence afresh (5, 7, 9) rather than catch up with one at 4.
        let mut pacer = link(30, 60);
        let period = NANOS_PER_SECOND / 60;
        let frames = run(&mut pacer, (0..=9).filter(|&i| i != 2).map(|i| i * period));
        assert_eq!(frames, [33_333, 49_999, 33_333, 33_333, 33_333]);
        assert_eq!((pacer.stats().missed, pacer.stats().on_time), (1, 3));
    }

    #[test]
    fn irregular_ticks_are_held_or_measured_by_their_vsync() {
        // A 120 Hz link with a 60 fps target that the system serves unevenly: a tick one vsync
        // after a frame, a second tick for the same vsync, and a gap of four vsyncs.
        let mut pacer = link(60, 120);
        let period = NANOS_PER_SECOND / 120;
        let ticks = [0, 2, 3, 4, 6, 6, 8, 12, 13, 14].map(|i| i * period);
        let frames = run(&mut pacer, ticks);
        // Frames at vsyncs 0, 2, 4, 6, 8, 12, 14; vsync 3, the second 6 and 13 are held.
        assert_eq!(frames, [16_666, 16_666, 16_666, 16_666, 16_666, 33_333, 16_666]);
        let stats = pacer.stats();
        assert_eq!((stats.held, stats.on_time, stats.missed, stats.doubled), (3, 5, 1, 0));
    }

    #[test]
    fn scheduler_jitter_within_half_a_refresh_is_on_time() {
        // Presentation pacing on a 60 Hz display: frames begin up to ±3 ms off the vsync.
        let mut pacer = FramePacer::new(PacingSource::Presentation);
        pacer.set_target(rate(60));
        pacer.set_refresh(Some(refresh(60)));
        let jitter = [0i64, 2_900_000, -1_500_000, 400_000, -2_800_000, 1_000_000, 0];
        let ticks = jitter.iter().enumerate().map(|(i, &j)| (i as i64 * 16_666_667 + 10_000_000 + j) as u64);
        let frames = run(&mut pacer, ticks);
        assert_eq!(frames.len(), 7);
        assert_eq!((pacer.stats().on_time, pacer.stats().missed, pacer.stats().doubled), (6, 0, 0));
    }

    #[test]
    fn presentation_pacing_never_holds_and_counts_doubled_frames() {
        // A desktop on a 144 Hz monitor, unlimited: every frame comes before the 60 fps target.
        let mut pacer = FramePacer::new(PacingSource::Presentation);
        pacer.set_target(rate(60));
        pacer.set_refresh(Some(refresh(144)));
        let frames = run(&mut pacer, vsyncs(0, 144, 144));
        assert_eq!(frames.len(), 144);
        let stats = pacer.stats();
        assert_eq!((stats.held, stats.measured, stats.doubled, stats.on_time), (0, 143, 143, 0));
    }

    #[test]
    fn the_target_is_never_faster_than_the_display() {
        // 120 fps asked of a 60 Hz display: every vsync is a frame, on time.
        let mut pacer = link(120, 60);
        let frames = run(&mut pacer, vsyncs(0, 60, 60));
        assert!(frames.iter().all(|&us| us == 16_666));
        assert_eq!((pacer.stats().on_time, pacer.stats().missed), (59, 0));
    }

    #[test]
    fn without_a_target_every_tick_is_a_frame_and_nothing_is_measured() {
        let mut pacer = FramePacer::new(PacingSource::DisplayLink);
        pacer.set_refresh(Some(refresh(120)));
        let frames = run(&mut pacer, vsyncs(0, 120, 30));
        assert_eq!(frames.len(), 30);
        assert_eq!(frames[0], 8_333, "the first frame gets one refresh");
        assert_eq!((pacer.stats().held, pacer.stats().measured), (0, 0));
    }

    #[test]
    fn an_unknown_refresh_tolerates_a_quarter_interval() {
        let mut pacer = FramePacer::new(PacingSource::DisplayLink);
        pacer.set_target(rate(30));
        // SDL reports an unknown rate as zero.
        pacer.set_refresh(Some(Duration::ZERO));
        // A quarter of 33.3 ms is 8.3: a tick 7.8 ms early is a frame on time, one 9.3 ms early
        // is held.
        let frames = run(&mut pacer, [0, 25_500_000, 49_500_000, 58_833_333]);
        assert_eq!(frames, [33_333, 25_500, 33_333]);
        assert_eq!((pacer.stats().held, pacer.stats().on_time), (1, 2));
    }

    #[test]
    fn a_target_above_a_billion_fps_divides_by_no_zero() {
        let mut pacer = FramePacer::new(PacingSource::Presentation);
        pacer.set_target(rate(u32::MAX));
        assert_eq!(run(&mut pacer, [0, 1, 2, 10]).len(), 4);
        assert_eq!(pacer.stats().measured, 3);
    }

    #[test]
    fn a_target_change_takes_effect_on_the_next_tick() {
        let mut pacer = link(60, 120);
        let mut frames = run(&mut pacer, vsyncs(0, 120, 20));
        pacer.set_target(rate(30));
        frames.extend(run(&mut pacer, vsyncs(20 * NANOS_PER_SECOND / 120, 120, 40)));
        assert_eq!(frames.len(), 10 + 10);
        // Ticks are held until a whole 30 fps interval has passed since the last 60 fps frame.
        assert!(frames[10..].iter().all(|&us| us == 33_333), "{frames:?}");
        assert_eq!((pacer.stats().missed, pacer.stats().doubled), (0, 0));

        // Back to 60, with a link that ticks once more at 30 fps before it follows (vsyncs 62, then
        // every second from 64): the frame spanning the change isn't a missed 60 fps interval.
        pacer.set_target(rate(60));
        let ticks = [62].into_iter().chain((64..100).step_by(2)).map(|i| i * NANOS_PER_SECOND / 120);
        let frames = run(&mut pacer, ticks);
        assert_eq!(frames[0], 33_333);
        assert!(frames[1..].iter().all(|&us| us == 16_666), "{frames:?}");
        let stats = pacer.stats();
        assert_eq!((stats.frames, stats.measured, stats.missed, stats.doubled), (39, 36, 0, 0));
    }

    #[test]
    fn a_trip_to_the_background_is_neither_measured_nor_simulated() {
        let mut pacer = link(60, 60);
        run(&mut pacer, vsyncs(0, 60, 10));
        pacer.pause();
        // Inactive but visible: ticks keep coming, irregularly, and aren't measured; none is
        // longer than a target interval.
        let inactive = [10, 13, 17].map(|i| i * NANOS_PER_SECOND / 60);
        assert_eq!(run(&mut pacer, inactive), [16_666; 3]);
        // Five seconds in the background, then back.
        pacer.resume();
        let back = vsyncs(5 * NANOS_PER_SECOND, 60, 10);
        let frames = run(&mut pacer, back);
        assert_eq!(frames[0], 16_666, "the first frame back gets one target interval");
        let stats = pacer.stats();
        assert_eq!((stats.frames, stats.measured, stats.on_time, stats.missed), (23, 18, 18, 0));
    }

    #[test]
    fn a_tick_back_before_the_return_is_reported_simulates_no_time_away() {
        // The link ticks once more after five seconds in the background, before the platform hears
        // the app is back (`DidEnterForeground`).
        let mut pacer = link(60, 60);
        run(&mut pacer, vsyncs(0, 60, 10));
        pacer.pause();
        assert_eq!(run(&mut pacer, [5 * NANOS_PER_SECOND]), [16_666]);
        pacer.resume();
        assert_eq!(run(&mut pacer, vsyncs(5 * NANOS_PER_SECOND + 16_666_666, 60, 3)), [16_666; 3]);
        assert_eq!(pacer.stats().missed, 0);
    }

    #[test]
    fn a_clock_that_goes_backwards_starts_afresh() {
        let mut pacer = link(60, 60);
        run(&mut pacer, vsyncs(1_000_000_000, 60, 5));
        assert_eq!(pacer.tick(10), Some(Duration::from_nanos(16_666_666)));
        assert_eq!(pacer.stats().missed + pacer.stats().doubled, 0);
    }
}
