//! Frame pacing (spec §6.2, ADR 0003): which display refreshes become frames, and how evenly they
//! come.
//!
//! [`FramePacer`] is pure: it reads no clock and is fed each display tick's vsync time, so its
//! decisions and counters are deterministic. The platform's frame clock feeds it: the engine's
//! display link on iOS and the choreographer's vsyncs on Android, where it also holds back ticks
//! that come before a frame is due (Android's frame limiter), and the time each frame begins on
//! desktop, where presentation paces frames and the pacer only measures.
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
//!
//! # Vsyncs faster than the refresh says
//!
//! On Android the vsyncs can speed up before the platform hears of it: the choreographer reports a
//! lifted frame-rate override (the app's vsyncs going from 30 back to 60 Hz) only after the vsyncs
//! change. A vsync taken for two refreshes when it is one would start frames early, so under
//! choreographer pacing a tick less than three quarters of the refresh set after the tick before
//! divides that refresh by the whole number nearest the ratio (2 to 8) until the next one is set.
//! A refresh shorter than the panel's is safe, whole vsyncs being whole multiples of it. A tick
//! more than eight times sooner is taken for one off the vsyncs (Android's loop iterates on the
//! clock when vsyncs stop) and changes nothing.

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
    /// The engine's frame limiter: its loop waits for each vsync from Android's choreographer and
    /// starts a frame on the vsyncs the target rate is due on, holding back the rest, while the
    /// window asks the display for the target rate (Android, ADR 0004).
    Choreographer,
    /// SDL's loop runs frames back to back and presentation paces them, FIFO waiting for vsync
    /// (desktop). The target rate is measured against, not enforced.
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
    /// a 60 fps game on a display link running at 120 Hz. A display link and the choreographer's
    /// limiter hold such ticks back, so only presentation pacing counts them.
    pub doubled: u64,
    /// Display ticks held back so frames keep the target rate (display-link and choreographer
    /// pacing).
    pub held: u64,
    /// The latest frame's interval since the frame before.
    pub last_interval: Duration,
}

/// Turns display ticks into frames at the target rate and measures their intervals (see the
/// module docs).
#[derive(Clone, Debug)]
pub(crate) struct FramePacer {
    /// Hold back ticks that come before a frame is due (display-link and choreographer pacing), or
    /// let every tick through and only measure (presentation pacing).
    gate: bool,
    /// Shorten the refresh when ticks come faster than it (choreographer pacing; module docs).
    follow_faster_ticks: bool,
    target: Option<NonZeroU32>,
    /// The display's shortest refresh period, if known.
    refresh: Option<Duration>,
    /// The refresh as last set, which ticks faster than it may divide (choreographer pacing).
    refresh_set: Option<Duration>,
    /// The vsync time of the last tick, held or let through, in nanoseconds (choreographer pacing).
    last_tick: Option<u64>,
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
    /// Before it was due: a display link or the limiter holds the tick back; presentation counts it
    /// doubled.
    Early,
    /// On the refresh it was due, overpaying `carry` of a refresh.
    OnTime { carry: f64 },
    /// After it was due, by `missed` target intervals (at least one).
    Late { missed: u64 },
}

impl FramePacer {
    /// A pacer that holds back early ticks unless presentation paces frames.
    pub fn new(source: PacingSource) -> Self {
        Self {
            gate: source != PacingSource::Presentation,
            follow_faster_ticks: source == PacingSource::Choreographer,
            target: None,
            refresh: None,
            refresh_set: None,
            last_tick: None,
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
        self.refresh_set = self.refresh;
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
        if self.follow_faster_ticks
            && let Some(previous) = self.last_tick.replace(vsync_ns)
        {
            self.follow_faster_vsyncs(vsync_ns.saturating_sub(previous));
        }
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

    /// Divides the refresh set when a tick comes `spacing` nanoseconds after the tick before, less
    /// than three quarters of it (module docs). The refresh only ever shortens, to an eighth of the
    /// one set at most, and two ticks for one vsync change nothing.
    fn follow_faster_vsyncs(&mut self, spacing: u64) {
        let Some(set) = self.refresh_set else { return };
        let set = u64::try_from(set.as_nanos()).unwrap_or(u64::MAX);
        if spacing == 0 || spacing.saturating_mul(4) >= set.saturating_mul(3) {
            return;
        }
        let divisor = set.saturating_add(spacing / 2) / spacing;
        if divisor > 8 {
            return;
        }
        let divided = Duration::from_nanos(set / divisor.max(2));
        self.refresh = Some(self.refresh.map_or(divided, |refresh| refresh.min(divided)));
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

    /// The choreographer's limiter (Android) at `target` fps on a display refreshing at
    /// `display_hz`.
    fn limiter(target: u32, display_hz: u64) -> FramePacer {
        let mut pacer = FramePacer::new(PacingSource::Choreographer);
        pacer.set_target(rate(target));
        pacer.set_refresh(Some(refresh(display_hz)));
        pacer
    }

    /// Android's vsync loop from `from` until `until` (nanoseconds), on a panel refreshing at `hz`
    /// whose vsyncs fall on `phase` plus whole periods. Each wait gets the first vsync after it
    /// posts its frame callback, so a held tick (whose events take 0.1 ms) is followed by the next
    /// vsync, and a frame by the first vsync after `work(frame)` nanoseconds of its work. Returns
    /// the intervals of the frames let through, in microseconds, and the time the loop got to.
    fn choreographer_loop(
        pacer: &mut FramePacer,
        (phase, hz): (u64, u64),
        from: u64,
        until: u64,
        mut work: impl FnMut(usize) -> u64,
    ) -> (Vec<u128>, u64) {
        let vsync = |k: u64| phase + k * NANOS_PER_SECOND / hz;
        let (mut frames, mut now, mut k) = (Vec::new(), from, 0);
        loop {
            while vsync(k) <= now {
                k += 1;
            }
            if vsync(k) > until {
                return (frames, now);
            }
            now = match pacer.tick(vsync(k)) {
                Some(interval) => {
                    frames.push(interval.as_micros());
                    vsync(k) + work(frames.len() - 1)
                }
                None => vsync(k) + 100_000,
            };
        }
    }

    /// Every interval is `expected` microseconds, give or take the nanosecond the vsyncs' whole
    /// nanoseconds round away.
    fn all_near(intervals: &[u128], expected: u128) -> bool {
        intervals.iter().all(|&us| us.abs_diff(expected) <= 1)
    }

    #[test]
    fn the_limiter_holds_30_and_60_fps_on_60_90_and_120_hz_panels() {
        // Frames working 6 ms, under a refresh on every panel, so the loop sees every vsync: those a
        // frame is due on start one, and the rest are held.
        let cases: [(u64, u32, &[u128]); 6] = [
            (60, 30, &[33_333]),
            (60, 60, &[16_666]),
            (90, 30, &[33_333]),
            (90, 60, &[22_222, 11_111]),
            (120, 30, &[33_333]),
            (120, 60, &[16_666]),
        ];
        for (hz, target, cadence) in cases {
            let mut pacer = limiter(target, hz);
            let (frames, _) =
                choreographer_loop(&mut pacer, (0, hz), 0, 10 * NANOS_PER_SECOND, |_| 6_000_000);
            let cycle = frames[1..].chunks(cadence.len());
            assert!(
                cycle
                    .clone()
                    .all(|chunk| chunk.iter().zip(cadence).all(|(&us, &want)| us.abs_diff(want) <= 1))
            );
            let stats = pacer.stats();
            assert_eq!(stats.frames + stats.held, 10 * hz, "every vsync was a tick at {hz} Hz");
            assert!(stats.frames.abs_diff(10 * u64::from(target)) <= 1, "{target} fps at {hz} Hz: {stats:?}");
            assert!(all_on_time(&pacer), "{target} fps at {hz} Hz: {stats:?}");
        }
    }

    #[test]
    fn the_limiter_sleeps_through_the_vsyncs_a_frame_works_over() {
        // 60 fps at 120 Hz with frames working 12 ms: the first vsync after each frame's work is
        // the one the next frame is due on, so none is seen early and none is held.
        let mut pacer = limiter(60, 120);
        let (frames, _) = choreographer_loop(&mut pacer, (0, 120), 0, 5 * NANOS_PER_SECOND, |_| 12_000_000);
        assert!(all_near(&frames[1..], 16_666), "{frames:?}");
        assert_eq!(pacer.stats().held, 0);
        assert!(all_on_time(&pacer), "{:?}", pacer.stats());
    }

    #[test]
    fn a_late_frame_counts_its_missed_interval_and_the_cadence_restarts() {
        // 30 fps at 60 Hz. The tenth frame works 40 ms, past its 33.3 ms interval, so the next
        // starts on the first vsync after it, three refreshes on (one interval missed), and the
        // cadence restarts from there rather than catch up with a short interval.
        let mut pacer = limiter(30, 60);
        let work = |frame| if frame == 10 { 40_000_000 } else { 6_000_000 };
        let (frames, _) = choreographer_loop(&mut pacer, (0, 60), 0, 2 * NANOS_PER_SECOND, work);
        assert!(all_near(&frames[1..11], 33_333) && all_near(&frames[12..], 33_333), "{frames:?}");
        assert_eq!(frames[11].abs_diff(50_000), 0, "{frames:?}");
        let stats = pacer.stats();
        assert_eq!((stats.missed, stats.doubled, stats.on_time), (1, 0, stats.measured - 1));

        // 60 fps at 120 Hz, the fifth frame working 20 ms: three refreshes where two were due.
        let mut pacer = limiter(60, 120);
        let work = |frame| if frame == 5 { 20_000_000 } else { 6_000_000 };
        let (frames, _) = choreographer_loop(&mut pacer, (0, 120), 0, NANOS_PER_SECOND, work);
        assert_eq!(frames[6], 25_000, "{frames:?}");
        assert!(all_near(&frames[7..], 16_666), "{frames:?}");
        assert_eq!((pacer.stats().missed, pacer.stats().doubled), (1, 0));
    }

    #[test]
    fn the_target_holds_while_the_hint_drops_the_panel_from_120_to_60_hz() {
        // 30 fps on a 120 Hz panel, until the refresh hint takes it to 60 Hz. The choreographer
        // reports the new period half a second after the vsyncs change: meanwhile the pacer counts
        // each 60 Hz vsync as two of the 120 Hz refreshes it knows, which holds the target too.
        let mut pacer = limiter(30, 120);
        let second = NANOS_PER_SECOND;
        let (mut frames, now) = choreographer_loop(&mut pacer, (0, 120), 0, second, |_| 6_000_000);
        let (more, now) = choreographer_loop(&mut pacer, (second, 60), now, second * 3 / 2, |_| 6_000_000);
        frames.extend(more);
        pacer.set_refresh(Some(refresh(60)));
        let (more, _) = choreographer_loop(&mut pacer, (second, 60), now, second * 5 / 2, |_| 6_000_000);
        frames.extend(more);
        // The frames fell on odd 120 Hz vsyncs, which the 60 Hz panel no longer has: the frame due
        // on one comes on the next 60 Hz vsync, a refresh late, and the cadence holds from there.
        let off_cadence: Vec<_> = frames[1..].iter().filter(|&&us| us.abs_diff(33_333) > 1).collect();
        assert_eq!(off_cadence, [&41_666], "{frames:?}");
        assert_eq!(frames.len(), 75);
        assert_eq!((pacer.stats().missed, pacer.stats().doubled), (1, 0));
    }

    #[test]
    fn a_refresh_reported_faster_than_the_panel_holds_the_target() {
        // The hint took a 120 Hz panel to 60 Hz before the choreographer said so: a 60 Hz vsync is
        // two of the refreshes the pacer knows, so 60 fps runs on every vsync and 30 on every second.
        for (target, every) in [(60, 16_666), (30, 33_333)] {
            let mut pacer = limiter(target, 120);
            let (frames, _) = choreographer_loop(&mut pacer, (0, 60), 0, 2 * NANOS_PER_SECOND, |_| 6_000_000);
            assert!(all_near(&frames[1..], every), "{frames:?}");
            assert!(all_on_time(&pacer), "{:?}", pacer.stats());
        }
    }

    #[test]
    fn the_limiter_starts_afresh_after_a_trip_to_the_background() {
        // SDL's event pump blocks while the app is paused, so no tick comes in the background, and
        // the tick that was waiting as the app left is dropped once it is back (`android_loop`).
        // The first vsync after the return starts a frame at once, with one target interval, and
        // the cadence holds from there, on the panel's new vsync phase.
        let mut pacer = limiter(30, 60);
        let (_, now) = choreographer_loop(&mut pacer, (0, 60), 0, 2 * NANOS_PER_SECOND, |_| 6_000_000);
        pacer.pause();
        pacer.resume();
        let back = now + 5 * NANOS_PER_SECOND + 3_000_000;
        let (frames, _) =
            choreographer_loop(&mut pacer, (back, 60), back, back + 2 * NANOS_PER_SECOND, |_| 6_000_000);
        assert_eq!(frames[0], 33_333, "the first frame back gets one target interval");
        assert!(all_near(&frames[1..], 33_333), "{frames:?}");
        // Measured: every frame but the first and the first back.
        let stats = pacer.stats();
        assert_eq!(
            (stats.measured, stats.on_time, stats.missed, stats.doubled),
            (stats.frames - 2, stats.measured, 0, 0)
        );
    }

    #[test]
    fn vsyncs_faster_than_the_reported_refresh_never_start_a_frame_early() {
        // 30 fps under a 30 Hz frame-rate override that is lifted: the vsyncs go to 60 Hz, half a
        // millisecond off the 30 Hz ones, while the pacer still counts 33.3 ms refreshes. Taken as a
        // whole refresh, the first 60 Hz vsync (17.2 ms on) would start a frame; it shows the faster
        // vsyncs instead, the refresh is halved, and frames keep 33.3 ms. The report of 60 Hz that
        // follows changes nothing.
        let second = NANOS_PER_SECOND;
        let mut pacer = limiter(30, 30);
        let (mut frames, now) = choreographer_loop(&mut pacer, (0, 30), 0, second, |_| 6_000_000);
        let lifted = (second + 500_000, 60);
        let (more, now) = choreographer_loop(&mut pacer, lifted, now, 2 * second, |_| 6_000_000);
        frames.extend(more);
        assert_eq!(pacer.refresh, Some(refresh(30) / 2));
        pacer.set_refresh(Some(refresh(60)));
        let (more, _) = choreographer_loop(&mut pacer, lifted, now, 3 * second, |_| 6_000_000);
        frames.extend(more);
        assert!(frames[1..].iter().all(|&us| us >= 33_333), "{frames:?}");
        assert_eq!(frames[30], 33_833, "the vsyncs' new phase, once: {frames:?}");
        assert!(all_near(&frames[31..], 33_333), "{frames:?}");
        let stats = pacer.stats();
        assert_eq!((stats.on_time, stats.missed, stats.doubled), (stats.measured, 0, 0));
    }

    #[test]
    fn a_tick_off_the_vsyncs_keeps_the_cadence() {
        // No vsync came for 100 ms, so Android's loop iterated on the clock, and the vsyncs resume
        // 11.7 ms after that tick: the refresh is halved, which whole vsyncs still divide, and the
        // frames keep 33.3 ms on the vsyncs once the one after the clock's has restarted them.
        let mut pacer = limiter(30, 60);
        let period = NANOS_PER_SECOND / 60;
        let mut frames = run(&mut pacer, (0..=10).map(|i| i * period));
        frames.extend(run(&mut pacer, [10 * period + 105_000_000]));
        frames.extend(run(&mut pacer, (17..=40).map(|i| i * period)));
        assert!(frames[6] == 105_000 && frames[7].abs_diff(45_000) <= 1, "{frames:?}");
        assert!(all_near(&frames[8..], 33_333), "{frames:?}");
        assert_eq!(pacer.stats().doubled, 0);
        assert_eq!(pacer.refresh, Some(refresh(60) / 2));
    }

    #[test]
    fn a_tick_just_before_a_vsync_changes_no_refresh() {
        // The clock's tick 0.5 ms before a vsync would be the vsyncs running 33 times faster, which
        // no panel does: the refresh stays, the vsync is held, and the cadence restarts from the
        // clock's frame, on the vsyncs.
        let mut pacer = limiter(30, 60);
        let period = NANOS_PER_SECOND / 60;
        let mut frames = run(&mut pacer, (0..=10).map(|i| i * period));
        frames.extend(run(&mut pacer, [17 * period - 500_000]));
        frames.extend(run(&mut pacer, (17..=40).map(|i| i * period)));
        assert_eq!(pacer.refresh, Some(refresh(60)));
        assert!(frames[7].abs_diff(33_833) <= 1 && all_near(&frames[8..], 33_333), "{frames:?}");
        assert_eq!(pacer.stats().doubled, 0);
    }
}
