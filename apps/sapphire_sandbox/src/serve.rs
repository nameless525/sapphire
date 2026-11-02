//! `--offscreen` (`dev` builds): the scene drawn into an offscreen target until the process is
//! told to quit (or for `--frames`), applying hot reload's changes and live link's pushes and
//! answering live link's captures (see [`crate::offscreen`]).
//!
//! It is how the desktop runs what a device runs without opening a window, which would take
//! focus from whoever uses the machine: `cargo xtask link` pushes to it at 127.0.0.1:7811 as to a
//! phone, `cargo xtask link capture <png>` writes what it draws, and A8's watcher reloads what
//! changes on disk. The M1 exit run's desktop leg runs on it (`docs/m1-exit-run.md`).
//!
//! With `--record <file> --scenario <name> --script <name>` it runs a fight beside the scene, one tick a
//! frame, which live tuning edits and whose recording is written when it quits (M2 T2).
//!
//! With `--cues` a fight's sound goes through the real mixer with no device (`OfflineAudio`), so a long
//! run holds what a fight in a window holds.
//!
//! Its stage is the capture and bench modes' ([`Stage`]), which prewarms the post variants the
//! scene may draw at startup (`Scene::effect_variants`, `Renderer::prewarm_post`), as the
//! windowed app does; replaced screen effect presets prewarm theirs as hot reload or live link
//! applies them. It follows `r.tier` as the windowed app does, with a new renderer on the same
//! device, and draws at that tier's rate (M2 Z1): Mobile-Low's frames are 1/30 s, each with two fixed
//! steps, as a phone's are. It has no device loss to rebuild after.
//!
//! It draws one frame per iteration of the platform's loop, so SDL still delivers its events:
//! Ctrl-C and `kill` (SIGINT, SIGTERM) come as a quit, which ends the run. The scene moves at its own
//! speed while frames keep up with that rate, and slower when they don't; the rest of each frame is
//! slept out, so an idle scene costs a fraction of a core rather than all of one. Every two seconds it
//! logs `frame stats` as the windowed app does: the frames drawn, their mean and the longest, which shows
//! whether anything held a frame (a shader push builds off the frame thread, M1 A8 follow-up 2), with what
//! they allocated and what the heap holds, so a long run can say whether anything grows.

use std::num::NonZero;
use std::time::Duration;

use sapphire_platform::{AppControl, AppInitError};
use sapphire_rhi::wgpu;
use sapphire_time::{Stopwatch, TICK_HZ};

use crate::LOG_TARGET;
use crate::app::create_renderer;
use crate::audio::OfflineAudio;
use crate::fight_bench::{frame_length, steps_a_frame};
use crate::link::FrameCapture;
use crate::offscreen::{FORMAT, Stage};
use crate::options::Options;

/// How often the frame stats line is logged, as the windowed app's.
const REPORT_INTERVAL: Duration = Duration::from_secs(2);

/// The offscreen stage, drawn a frame at a time.
pub(crate) struct Serving {
    // Drop order: the target before the stage's device.
    view: wgpu::TextureView,
    _target: wgpu::Texture,
    stage: Stage,
    /// What a new renderer holds and is made with.
    options: Options,
    size: (u32, u32),
    /// Frames still to draw, with `--frames`.
    remaining: Option<u64>,
    /// The frames since the last frame stats line.
    lengths: FrameLengths,
    /// A fight's sound, with `--cues`.
    audio: Option<OfflineAudio>,
    /// Since the first frame, and when on it the next is due. A sleep overshoots by a millisecond or
    /// more, which at 30 frames a second came to 25 when each frame slept for what was left of its own.
    clock: Stopwatch,
    due: Duration,
}

/// Frames drawn over a window of about [`REPORT_INTERVAL`], for the frame stats line.
struct FrameLengths {
    /// Since the window started.
    window: Stopwatch,
    /// The time they took to draw, summed.
    drawn: Duration,
    frames: u32,
    /// The longest time a frame took to draw.
    longest: Duration,
    /// The process's allocations when the window started.
    allocations: u64,
}

impl FrameLengths {
    fn new() -> Self {
        Self {
            window: Stopwatch::start(),
            drawn: Duration::ZERO,
            frames: 0,
            longest: Duration::ZERO,
            allocations: sapphire_core::alloc::stats().total_allocations,
        }
    }

    /// Counts a frame that took `drawn` to draw, and logs the window's once it is long enough.
    fn record(&mut self, drawn: Duration) {
        self.drawn += drawn;
        self.frames += 1;
        self.longest = self.longest.max(drawn);
        let window = self.window.elapsed();
        if window < REPORT_INTERVAL {
            return;
        }
        let heap = sapphire_core::alloc::stats();
        tracing::info!(
            target: LOG_TARGET,
            frames = self.frames,
            fps = format_args!("{:.1}", f64::from(self.frames) / window.as_secs_f64()),
            mean_ms = format_args!("{:.2}", self.drawn.as_secs_f64() * 1e3 / f64::from(self.frames)),
            longest_ms = format_args!("{:.2}", self.longest.as_secs_f64() * 1e3),
            allocs_per_frame = (heap.total_allocations - self.allocations) / u64::from(self.frames),
            live_bytes = heap.live_bytes,
            "frame stats"
        );
        *self = Self::new();
    }
}

impl Serving {
    /// Draws `stage`'s scene, made as `options` say, at `width` by `height`, `--frames` frames or
    /// until the process quits.
    pub fn new(mut stage: Stage, options: Options, width: u32, height: u32) -> Result<Self, AppInitError> {
        let audio = match stage.scene.cue_show().map(|show| show.table().sound_cues()) {
            Some(cues) => {
                Some(OfflineAudio::for_cues(&cues, &mut stage.content, &mut stage.renderer, &stage.gpu)?)
            }
            None => None,
        };
        let target = stage.target(width, height, wgpu::TextureUsages::empty());
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let frame_limit = options.frame_limit;
        tracing::info!(target: LOG_TARGET, width, height, ?frame_limit, "drawing offscreen until told to quit");
        let lengths = FrameLengths::new();
        Ok(Self {
            view,
            _target: target,
            stage,
            options,
            size: (width, height),
            remaining: frame_limit,
            lengths,
            audio,
            clock: Stopwatch::start(),
            due: Duration::ZERO,
        })
    }

    /// Ends the fight, if there is one, and says how the app should leave.
    pub fn finish(&mut self) -> AppControl {
        match self.stage.finish_combat() {
            Ok(()) => AppControl::Exit,
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, "the fight didn't end cleanly");
                AppControl::Fail
            }
        }
    }

    /// Draws the next frame, then sleeps out the rest of its time.
    pub fn frame(&mut self) -> AppControl {
        if self.remaining == Some(0) {
            return self.finish();
        }
        let stopwatch = Stopwatch::start();
        if let Err(error) = self.draw() {
            tracing::error!(target: LOG_TARGET, %error, "offscreen frame failed");
            return AppControl::Fail;
        }
        self.remaining = self.remaining.map(|remaining| remaining - 1);
        self.lengths.record(stopwatch.elapsed());
        // A frame after the last was due, or now if this one ran late.
        let frame = frame_length(self.stage.quality.frame_rate().map_or(TICK_HZ, NonZero::get));
        self.due = (self.due + frame).max(self.clock.elapsed());
        if let Some(rest) = self.due.checked_sub(self.clock.elapsed()) {
            std::thread::sleep(rest);
        }
        AppControl::Continue
    }

    /// Follows `r.tier`: a new tier gets a new renderer with its settings, on the same device, as
    /// in the windowed app. If that fails the old tier and renderer stay.
    fn follow_tier(&mut self) {
        let stage = &mut self.stage;
        let Some(previous) = stage.quality.update_tier() else { return };
        let settings = self.options.render_settings(stage.quality.render_settings());
        match create_renderer(&mut stage.content, &stage.gpu, FORMAT, settings, &self.options) {
            Ok(renderer) => {
                stage.scene.set_meshes(&renderer);
                stage.renderer = renderer.renderer;
                stage.dev.renderer_created(&mut stage.renderer);
                stage.scene.prewarm(&mut stage.renderer);
                // The new renderer's scene starts empty.
                stage.scene.resend_instances();
                tracing::info!(target: LOG_TARGET, tier = %stage.quality.active().id, "renderer rebuilt for the quality tier");
            }
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, tier = %stage.quality.active().id, "no renderer for the quality tier; keeping the last one");
                stage.quality.restore(previous);
            }
        }
        stage.scene.set_spring_quality(crate::animate::spring_quality(stage.quality.active().id));
    }

    fn draw(&mut self) -> Result<(), AppInitError> {
        self.follow_tier();
        let stage = &mut self.stage;
        let rate = stage.quality.frame_rate().map_or(TICK_HZ, NonZero::get);
        let steps = steps_a_frame(rate);
        for _ in 0..steps {
            stage.fixed_step();
            // Each step clears its cues, so its hits read here.
            stage.show_damage();
        }
        let dt = steps as f32 / TICK_HZ as f32;
        if let Some(audio) = &mut self.audio {
            audio.update(dt, stage.scene.sound(), stage.scene.cue_sounds());
            audio.mix(dt);
        }
        stage.scene.end_cue_frame(dt, &mut ());
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
