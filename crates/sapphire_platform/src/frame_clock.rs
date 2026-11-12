//! The frame clock: what starts each frame, the target rate, and the display's refresh (spec
//! §6.2, ADR 0003).
//!
//! [`FrameClock`] belongs to the [`Platform`](crate::Platform) and runs before every
//! [`App::iterate`](crate::App::iterate). It takes the frame's tick from its source (the engine's
//! display link on iOS, the engine's vsync loop on Android, the time the frame begins on desktop),
//! asks the [`FramePacer`] whether the tick becomes a frame, and keeps the display's refresh
//! current: from the choreographer's reports on Android 11 and later, else from SDL's display
//! events. After the app's iteration it reports the frame's work to Android's ADPF hint session
//! (`hint.rs`), against the frame's budget: the frame thread's wall time from the tick to the end of
//! the iteration, less what it waited on the display meanwhile (the swapchain's acquire and present,
//! which `sapphire_rhi` adds to `sapphire_core::frame_waits`), so a frame bound by the GPU doesn't
//! read long and raise the CPU's clocks for nothing (M4 T7). [`FramePacing::work`] sums both.

use std::num::NonZeroU32;
use std::time::Duration;

use sapphire_core::frame_waits;
use sdl3_sys::events::{SDL_Event, SDL_EventType};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_WasInit};
use sdl3_sys::video::{SDL_DisplayID, SDL_GetCurrentDisplayMode, SDL_GetPrimaryDisplay};

use crate::LOG_TARGET;
use crate::hint::HintSession;
use crate::pacing::{CapGate, FramePacer, PacingSource, PacingStats};

/// How frames are paced, and how evenly they come: a snapshot for stats panels and logs (rule R9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePacing {
    pub source: PacingSource,
    /// The frame rate frames are paced to (a display link or the choreographer's limiter) or
    /// measured against (presentation); `None`: the display's own rate, unmeasured.
    pub target: Option<NonZeroU32>,
    /// The display's refresh rate in Hz, if known: an iPhone's highest; on Android the rate the
    /// choreographer reports, which a frame-rate override can make the app's own (30 Hz vsyncs on
    /// a 60 Hz panel).
    pub refresh_hz: Option<f32>,
    /// This frame's interval since the frame before: whole refreshes where vsyncs start frames.
    pub interval: Duration,
    pub stats: PacingStats,
    /// The frames' work and their waits on the display, summed since the start (M4 T7).
    pub work: FrameWork,
}

/// Frames' work as the hint session hears it, and what was left out of it, summed (rule R9): a stats
/// line shows the difference across its window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameWork {
    /// Frames measured: every frame the pacer let through and the app iterated.
    pub frames: u64,
    /// Their work: the frame thread's wall time from the tick to the end of the app's iteration, less
    /// `waited`.
    pub work: Duration,
    /// What they waited on the display: the swapchain's acquire (every image in flight, as when the GPU
    /// is behind) and its present.
    pub waited: Duration,
}

/// Measures each frame's work: its wall time on the frame thread less its waits on the display.
#[derive(Debug, Default)]
struct WorkMeter {
    /// When the frame being iterated started its work, in nanoseconds on SDL's clock.
    started: Option<u64>,
    totals: FrameWork,
}

impl WorkMeter {
    /// A frame starts its work at `now`: waits from before it (a skipped frame's, a loading screen's)
    /// aren't its.
    fn start(&mut self, now: u64) {
        frame_waits::take();
        self.started = Some(now);
    }

    /// The frame started last ends at `now`: its work, less what the thread waited on the display since
    /// it started. `None` if no frame started.
    fn end(&mut self, now: u64) -> Option<Duration> {
        let started = self.started.take()?;
        let waited = frame_waits::take();
        let work = Duration::from_nanos(now.saturating_sub(started)).saturating_sub(waited);
        self.totals.frames += 1;
        self.totals.work = self.totals.work.saturating_add(work);
        self.totals.waited = self.totals.waited.saturating_add(waited);
        Some(work)
    }
}

#[derive(Debug)]
pub(crate) struct FrameClock {
    source: PacingSource,
    pacer: FramePacer,
    /// The desktop's frame cap (M4 T3): how long each frame sleeps before it starts. Unused where vsyncs start
    /// frames, whose pacer holds ticks back instead.
    cap: CapGate,
    /// The SDL display frames are shown on, once a window has reported one; else the primary one.
    display: Option<u32>,
    /// The refresh must be read again before the next frame (a display or its mode changed).
    refresh_stale: bool,
    /// The refresh period the vsync source last reported (Android's choreographer), which takes
    /// precedence over SDL's display mode.
    reported_refresh: Option<Duration>,
    refresh_hz: Option<f32>,
    /// The current frame's interval.
    interval: Duration,
    /// The frame thread's hint session, where the OS has them.
    hint: HintSession,
    /// Each frame's work, for the hint session and the stats.
    work: WorkMeter,
}

impl FrameClock {
    pub fn new() -> Self {
        let source = if cfg!(target_os = "ios") {
            PacingSource::DisplayLink
        } else if cfg!(target_os = "android") {
            PacingSource::Choreographer
        } else {
            PacingSource::Presentation
        };
        Self {
            source,
            pacer: FramePacer::new(source),
            cap: CapGate::default(),
            display: None,
            refresh_stale: true,
            reported_refresh: None,
            refresh_hz: None,
            interval: Duration::ZERO,
            hint: HintSession::new(),
            work: WorkMeter::default(),
        }
    }

    pub fn set_frame_rate(&mut self, rate: Option<NonZeroU32>) {
        self.pacer.set_target(rate);
        #[cfg(target_os = "ios")]
        crate::ios_loop::set_frame_rate(rate);
        #[cfg(target_os = "android")]
        crate::android_loop::set_frame_rate(rate);
    }

    /// The desktop's frame cap (spec §6.2): frames start at most `cap` times a second, sleeping until each is
    /// due, and presentation keeps them on vsyncs; `None` lifts it. Ignored where vsyncs start frames (iOS,
    /// Android): there the frame rate is the cap.
    pub fn set_frame_cap(&mut self, cap: Option<NonZeroU32>) {
        if self.source == PacingSource::Presentation {
            self.cap.set_cap(cap);
        }
    }

    /// The desktop's frame cap, if one is set.
    pub fn frame_cap(&self) -> Option<NonZeroU32> {
        self.cap.cap()
    }

    pub fn pacing(&self) -> FramePacing {
        FramePacing {
            source: self.source,
            target: self.pacer.target(),
            refresh_hz: self.refresh_hz,
            interval: self.interval,
            stats: self.pacer.stats(),
            work: self.work.totals,
        }
    }

    /// The frame's interval where vsyncs start frames (a display link, the choreographer's
    /// limiter): the time between the frames' vsyncs, free of the scheduling jitter a clock read
    /// as the frame begins would carry.
    pub fn display_interval(&self) -> Option<Duration> {
        (self.source != PacingSource::Presentation).then_some(self.interval)
    }

    /// Runs before each [`App::iterate`](crate::App::iterate): whether this tick becomes a frame.
    pub fn begin_frame(&mut self) -> bool {
        let reported = reported_refresh();
        if reported != self.reported_refresh {
            self.reported_refresh = reported;
            self.refresh_stale = true;
            if let Some(period) = reported {
                tracing::info!(target: LOG_TARGET, hz = format_args!("{:.2}", hz(period)), "display refresh reported");
            }
        }
        if self.refresh_stale {
            self.refresh_stale = false;
            let refresh = self
                .reported_refresh
                .map(|period| (hz(period), period))
                .or_else(|| display_refresh(self.display));
            self.refresh_hz = refresh.map(|(hz, _)| hz);
            self.pacer.set_refresh(refresh.map(|(_, period)| period));
        }
        if self.cap.cap().is_some() {
            sleep_for_cap(&mut self.cap);
        }
        let Some(vsync) = frame_tick() else {
            // Outside a vsync's tick: on iOS only the display link starts frames, and SDL 3.4.18
            // iterates from nowhere else (should a later SDL do so, no frame beats one run unpaced
            // with a stale interval); on Android the tick's vsync was dropped because the app came
            // back during its event pump, and the loop's next vsync starts the frame.
            return false;
        };
        match self.pacer.tick(vsync) {
            Some(interval) => {
                self.interval = interval;
                // SAFETY: SDL_GetTicksNS has no preconditions.
                self.work.start(unsafe { sdl3_sys::timer::SDL_GetTicksNS() });
                true
            }
            None => false,
        }
    }

    /// Runs after the app's iteration of a frame [`Self::begin_frame`] let through: reports the
    /// frame's work, from then to now less its waits on the display, to the hint session against the
    /// frame's budget, once there is one (a target rate or a known refresh).
    pub fn end_frame(&mut self) {
        // SAFETY: SDL_GetTicksNS has no preconditions.
        let Some(work) = self.work.end(unsafe { sdl3_sys::timer::SDL_GetTicksNS() }) else { return };
        let Some(budget) = self.pacer.frame_budget() else { return };
        self.hint.report(budget, work);
    }

    /// Whether the frame thread's hint session is live: `None` where the OS has none.
    pub fn hint_session(&self) -> Option<bool> {
        self.hint.live()
    }

    /// The app leaves the foreground: frames may stall or sleep there, so they aren't measured.
    pub fn pause(&mut self) {
        self.pacer.pause();
        self.cap.reset();
    }

    /// The app is back: the next frame starts the cadence afresh. On Android the tick being
    /// iterated came before the trip (SDL's event pump blocks while the app is paused), so it is
    /// dropped: the next vsync starts that frame.
    pub fn resume(&mut self) {
        self.pacer.resume();
        self.cap.reset();
        #[cfg(target_os = "android")]
        crate::android_loop::discard_vsync();
    }

    /// Follows the display frames are shown on and its mode, from SDL's raw events.
    pub fn observe(&mut self, raw: &SDL_Event) {
        // SAFETY: every SDL event variant begins with the common header, so the type is initialized.
        let kind = SDL_EventType(unsafe { raw.r#type });
        match kind {
            SDL_EventType::WINDOW_DISPLAY_CHANGED => {
                // SAFETY: the tag says this is a window event, whose `data1` is the new display.
                let display = unsafe { raw.window.data1 };
                self.display = u32::try_from(display).ok();
                self.refresh_stale = true;
            }
            SDL_EventType::DISPLAY_CURRENT_MODE_CHANGED
            | SDL_EventType::DISPLAY_ADDED
            | SDL_EventType::DISPLAY_REMOVED => {
                // SAFETY: the tag says this is a display event.
                let changed = unsafe { raw.display.displayID }.0;
                if kind == SDL_EventType::DISPLAY_REMOVED && self.display == Some(changed) {
                    self.display = None;
                }
                self.refresh_stale = true;
            }
            _ => {}
        }
    }
}

/// The frame's tick in nanoseconds: the vsync the engine's display link's frame will be shown at,
/// if the link is what started it.
#[cfg(target_os = "ios")]
fn frame_tick() -> Option<u64> {
    crate::ios_loop::take_vsync()
}

/// The frame's tick in nanoseconds: the vsync the engine's loop waited for, unless it was dropped.
#[cfg(target_os = "android")]
fn frame_tick() -> Option<u64> {
    crate::android_loop::take_vsync()
}

/// The frame's tick in nanoseconds: now, on the clock event timestamps use.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn frame_tick() -> Option<u64> {
    // SAFETY: SDL_GetTicksNS has no preconditions.
    Some(unsafe { sdl3_sys::timer::SDL_GetTicksNS() })
}

/// Sleeps until the next frame is due under the desktop's cap, on the clock SDL's ticks use.
fn sleep_for_cap(cap: &mut CapGate) {
    // SAFETY: SDL_GetTicksNS has no preconditions.
    let now = unsafe { sdl3_sys::timer::SDL_GetTicksNS() };
    let wait = cap.wait(now);
    if !wait.is_zero() {
        // SAFETY: SDL_DelayPrecise has no preconditions.
        unsafe { sdl3_sys::timer::SDL_DelayPrecise(u64::try_from(wait.as_nanos()).unwrap_or(u64::MAX)) };
    }
}

/// The display's refresh period as the vsync source reports it: Android's choreographer, from
/// Android 11 (API 30).
#[cfg(target_os = "android")]
fn reported_refresh() -> Option<Duration> {
    crate::android_loop::refresh_period()
}

/// The display's refresh period as the vsync source reports it: no source reports one here.
#[cfg(not(target_os = "android"))]
fn reported_refresh() -> Option<Duration> {
    None
}

/// A refresh period as a rate.
fn hz(period: Duration) -> f32 {
    (1.0 / period.as_secs_f64()) as f32
}

/// The refresh rate of `display`, or of the primary display, in Hz and as a period; `None`
/// without video (headless runs) or if SDL doesn't know it. SDL reports an iPhone's highest rate
/// (`UIScreen.maximumFramesPerSecond`), and an Android phone's rate when its surface last changed
/// size (`Display.getRefreshRate()`), which the choreographer's reports supersede.
fn display_refresh(display: Option<u32>) -> Option<(f32, Duration)> {
    // SAFETY: SDL_WasInit has no preconditions.
    if unsafe { SDL_WasInit(SDL_INIT_VIDEO) } != SDL_INIT_VIDEO {
        return None;
    }
    // SAFETY: video is initialized and this runs on the main thread (the frame clock is the
    // platform's). SDL returns null for an unknown display, and the mode is copied out before any
    // other SDL call.
    let mode = unsafe {
        let display = display.map_or_else(|| SDL_GetPrimaryDisplay(), SDL_DisplayID);
        SDL_GetCurrentDisplayMode(display)
            .as_ref()
            .map(|mode| (mode.refresh_rate, mode.refresh_rate_numerator, mode.refresh_rate_denominator))
    };
    let (hz, numerator, denominator) = mode?;
    if !(hz.is_finite() && hz > 0.0) {
        return None;
    }
    // The exact rational rate where SDL has one (59.94 Hz is 60000/1001).
    let period = match (u64::try_from(numerator), u64::try_from(denominator)) {
        (Ok(numerator @ 1..), Ok(denominator @ 1..)) => {
            Duration::from_nanos(denominator * 1_000_000_000 / numerator)
        }
        _ => Duration::from_secs_f64(1.0 / f64::from(hz)),
    };
    Some((hz, period))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// A frame bound by the GPU: 16 ms on the frame thread, of which 9 ms waited for a swapchain image and
    /// 1 ms in present. ADPF hears the 6 ms of work.
    #[test]
    fn the_work_reported_leaves_out_the_swapchains_waits() {
        let mut meter = WorkMeter::default();
        frame_waits::add(Duration::from_millis(40));
        meter.start(100 * MS);
        frame_waits::add(Duration::from_millis(9));
        frame_waits::add(Duration::from_millis(1));
        assert_eq!(
            meter.end(116 * MS),
            Some(Duration::from_millis(6)),
            "the wait before the frame isn't its"
        );
        assert_eq!(
            meter.totals,
            FrameWork { frames: 1, work: Duration::from_millis(6), waited: Duration::from_millis(10) }
        );

        // A frame that waited on nothing reports its whole wall time.
        meter.start(116 * MS);
        assert_eq!(meter.end(120 * MS), Some(Duration::from_millis(4)));

        // Waits can't take a frame's work below nothing (two clocks, read apart); the session then hears
        // its least, 1 ns.
        meter.start(120 * MS);
        frame_waits::add(Duration::from_millis(5));
        assert_eq!(meter.end(124 * MS), Some(Duration::ZERO));
        assert_eq!(
            meter.totals,
            FrameWork { frames: 3, work: Duration::from_millis(10), waited: Duration::from_millis(15) }
        );

        assert_eq!(meter.end(130 * MS), None, "no frame started");
        assert_eq!(meter.totals.frames, 3);
    }
}
