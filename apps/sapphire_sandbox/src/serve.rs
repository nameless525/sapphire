//! `--offscreen` (`dev` builds): the scene drawn into an offscreen target until the process is
//! told to quit (or for `--frames`), applying hot reload's changes and live link's pushes and
//! answering live link's captures (see [`crate::offscreen`]).
//!
//! It is how the desktop runs what a device runs without opening a window, which would take
//! focus from whoever uses the machine: `cargo xtask link` pushes to it at 127.0.0.1:7811 as to a
//! phone, `cargo xtask link capture <png>` writes what it draws, and A8's watcher reloads what
//! changes on disk. The M1 exit run's desktop leg runs on it (`docs/m1-exit-run.md`).
//!
//! Its stage is the capture and bench modes' ([`Stage`]), which prewarms the post variants the
//! scene may draw at startup (`Scene::effect_variants`, `Renderer::prewarm_post`), as the
//! windowed app does; replaced screen effect presets prewarm theirs as hot reload or live link
//! applies them. It has no tier changes or device loss to rebuild after.
//!
//! It draws one frame per iteration of the platform's loop, so SDL still delivers its events:
//! Ctrl-C and `kill` (SIGINT, SIGTERM) come as a quit, which ends the run. Each frame is one fixed
//! step, so the scene moves at its own speed while frames keep up with 60 a second, and slower
//! when they don't; the rest of each 1/60 s is slept out, so an idle scene costs a fraction of a
//! core rather than all of one. Every two seconds it logs `frame stats` as the windowed app does:
//! the frames drawn and the longest, which shows whether anything held a frame (a shader push
//! builds off the frame thread, M1 A8 follow-up 2).

use std::time::Duration;

use sapphire_platform::{AppControl, AppInitError};
use sapphire_rhi::wgpu;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::link::FrameCapture;
use crate::offscreen::{FORMAT, Stage};

/// The shortest a frame takes: 60 frames a second, as the fixed step.
const FRAME: Duration = Duration::from_micros(16_667);
/// How often the frame stats line is logged, as the windowed app's.
const REPORT_INTERVAL: Duration = Duration::from_secs(2);

/// The offscreen stage, drawn a frame at a time.
pub(crate) struct Serving {
    // Drop order: the target before the stage's device.
    view: wgpu::TextureView,
    _target: wgpu::Texture,
    stage: Stage,
    size: (u32, u32),
    /// Frames still to draw, with `--frames`.
    remaining: Option<u64>,
    /// The frames since the last frame stats line.
    lengths: FrameLengths,
}

/// Frames drawn over a window of about [`REPORT_INTERVAL`], for the frame stats line.
#[derive(Default)]
struct FrameLengths {
    /// Their lengths, slept out to 1/60 s, summed.
    window: Duration,
    frames: u32,
    /// The longest time a frame took to draw.
    longest: Duration,
}

impl FrameLengths {
    /// Counts a frame that took `drawn` to draw, and logs the window's once it is long enough.
    fn record(&mut self, drawn: Duration) {
        self.window += drawn.max(FRAME);
        self.frames += 1;
        self.longest = self.longest.max(drawn);
        if self.window < REPORT_INTERVAL {
            return;
        }
        tracing::info!(
            target: LOG_TARGET,
            frames = self.frames,
            fps = format_args!("{:.1}", f64::from(self.frames) / self.window.as_secs_f64()),
            longest_ms = format_args!("{:.2}", self.longest.as_secs_f64() * 1e3),
            "frame stats"
        );
        *self = Self::default();
    }
}

impl Serving {
    /// Draws `stage`'s scene at `width` by `height`, `frame_limit` frames or until the process
    /// quits.
    pub fn new(stage: Stage, width: u32, height: u32, frame_limit: Option<u64>) -> Self {
        let target = stage.target(width, height, wgpu::TextureUsages::empty());
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        tracing::info!(target: LOG_TARGET, width, height, ?frame_limit, "drawing offscreen until told to quit");
        let lengths = FrameLengths::default();
        Self { view, _target: target, stage, size: (width, height), remaining: frame_limit, lengths }
    }

    /// Draws the next frame, then sleeps out the rest of its 1/60 s.
    pub fn frame(&mut self) -> AppControl {
        if self.remaining == Some(0) {
            return AppControl::Exit;
        }
        let stopwatch = Stopwatch::start();
        if let Err(error) = self.draw() {
            tracing::error!(target: LOG_TARGET, %error, "offscreen frame failed");
            return AppControl::Fail;
        }
        self.remaining = self.remaining.map(|remaining| remaining - 1);
        let drawn = stopwatch.elapsed();
        self.lengths.record(drawn);
        if let Some(rest) = FRAME.checked_sub(drawn) {
            std::thread::sleep(rest);
        }
        AppControl::Continue
    }

    fn draw(&mut self) -> Result<(), AppInitError> {
        let stage = &mut self.stage;
        stage.step();
        // Captures asked for while the last frame was drawn: this one answers them, drawn into a
        // target that can be read back.
        match stage.dev.take_capture() {
            Some(request) => {
                let capture = FrameCapture::new(&stage.gpu, self.size, FORMAT, request);
                stage.render(capture.view(), 1.0, |graph, target| capture.record(graph, target))?;
                capture.finish(&stage.gpu, stage.dev.frames());
            }
            None => {
                stage.render(&self.view, 1.0, |_, _| {})?;
                // One frame in flight at most, as on a display.
                stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
            }
        }
        stage.scene.end_frame();
        Ok(())
    }
}
