//! The frame clock: what starts each frame, the target rate, and the display's refresh (spec
//! §6.2, ADR 0003).
//!
//! [`FrameClock`] belongs to the [`Platform`](crate::Platform) and runs before every
//! [`App::iterate`](crate::App::iterate). It takes the frame's tick from its source (the engine's
//! display link on iOS, the time the frame begins elsewhere), asks the [`FramePacer`] whether the
//! tick becomes a frame, and keeps the display's refresh current from SDL's display events.

use std::num::NonZeroU32;
use std::time::Duration;

use sdl3_sys::events::{SDL_Event, SDL_EventType};
use sdl3_sys::init::{SDL_INIT_VIDEO, SDL_WasInit};
use sdl3_sys::video::{SDL_DisplayID, SDL_GetCurrentDisplayMode, SDL_GetPrimaryDisplay};

use crate::pacing::{FramePacer, PacingSource, PacingStats};

/// How frames are paced, and how evenly they come: a snapshot for stats panels and logs (rule R9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePacing {
    pub source: PacingSource,
    /// The frame rate frames are paced to (a display link) or measured against (presentation);
    /// `None`: the display's own rate, unmeasured.
    pub target: Option<NonZeroU32>,
    /// The display's refresh rate in Hz (its highest, on a variable-rate display), if known.
    pub refresh_hz: Option<f32>,
    /// This frame's interval since the frame before: whole refreshes on a display link.
    pub interval: Duration,
    pub stats: PacingStats,
}

#[derive(Debug)]
pub(crate) struct FrameClock {
    source: PacingSource,
    pacer: FramePacer,
    /// The SDL display frames are shown on, once a window has reported one; else the primary one.
    display: Option<u32>,
    /// The refresh must be read again before the next frame (a display or its mode changed).
    refresh_stale: bool,
    refresh_hz: Option<f32>,
    /// The current frame's interval.
    interval: Duration,
}

impl FrameClock {
    pub fn new() -> Self {
        let source =
            if cfg!(target_os = "ios") { PacingSource::DisplayLink } else { PacingSource::Presentation };
        Self {
            source,
            pacer: FramePacer::new(source),
            display: None,
            refresh_stale: true,
            refresh_hz: None,
            interval: Duration::ZERO,
        }
    }

    pub fn set_frame_rate(&mut self, rate: Option<NonZeroU32>) {
        self.pacer.set_target(rate);
        #[cfg(target_os = "ios")]
        crate::ios_loop::set_frame_rate(rate);
    }

    pub fn pacing(&self) -> FramePacing {
        FramePacing {
            source: self.source,
            target: self.pacer.target(),
            refresh_hz: self.refresh_hz,
            interval: self.interval,
            stats: self.pacer.stats(),
        }
    }

    /// The frame's interval where a display link paces frames: the time between the vsyncs the
    /// frames are shown at, free of the scheduling jitter a clock read as the frame begins would
    /// carry.
    pub fn display_interval(&self) -> Option<Duration> {
        (self.source == PacingSource::DisplayLink).then_some(self.interval)
    }

    /// Runs before each [`App::iterate`](crate::App::iterate): whether this tick becomes a frame.
    pub fn begin_frame(&mut self) -> bool {
        if self.refresh_stale {
            self.refresh_stale = false;
            let refresh = display_refresh(self.display);
            self.refresh_hz = refresh.map(|(hz, _)| hz);
            self.pacer.set_refresh(refresh.map(|(_, period)| period));
        }
        let Some(vsync) = frame_tick() else {
            // iOS, outside the display link's tick: only the link starts frames there, and SDL
            // 3.4.18 iterates from nowhere else. Should a later SDL do so, no frame beats one
            // run unpaced with a stale interval.
            return false;
        };
        match self.pacer.tick(vsync) {
            Some(interval) => {
                self.interval = interval;
                true
            }
            None => false,
        }
    }

    /// The app leaves the foreground: frames may stall or sleep there, so they aren't measured.
    pub fn pause(&mut self) {
        self.pacer.pause();
    }

    /// The app is back: the next frame starts the cadence afresh.
    pub fn resume(&mut self) {
        self.pacer.resume();
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

/// The frame's tick in nanoseconds: now, on the clock event timestamps use.
#[cfg(not(target_os = "ios"))]
fn frame_tick() -> Option<u64> {
    // SAFETY: SDL_GetTicksNS has no preconditions.
    Some(unsafe { sdl3_sys::timer::SDL_GetTicksNS() })
}

/// The refresh rate of `display`, or of the primary display, in Hz and as a period; `None`
/// without video (headless runs) or if SDL doesn't know it. SDL reports a phone's highest rate
/// (`UIScreen.maximumFramesPerSecond` on iOS).
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
