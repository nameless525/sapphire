//! `--bench <frames>`: measures the sandbox's per-frame CPU path without a window (see
//! [`crate::offscreen`]).
//!
//! Each frame runs one fixed step, presentation and extraction into the frame's packet,
//! `Renderer::prepare`, and a frame graph with the renderer's passes, executed against an
//! offscreen target (compile, record, submit), then waits for the GPU so frames don't queue up.
//! After a warm-up it logs CPU time per frame (that span, without the GPU wait) and
//! allocations per frame (the whole frame, wait included, like the windowed stats). It leaves
//! out what only a window has: acquiring and presenting the swapchain, and vsync. A fast,
//! repeatable check for changes to the frame path against the budgets of spec §3. It also logs
//! time of day's own resolve time per frame (part of extraction), against its 0.05 ms budget;
//! `--day-length` makes the hour move every frame.
//!
//! While the frame graph times passes on the GPU (with `--dynamic-resolution`, or the overlay
//! shown) it also logs the mean GPU pass span of the frames whose timings came back: the GPU
//! wait alone is mostly submission latency on a light scene.
//!
//! Then, where the device has timestamp queries, it runs [`TIMED_FRAMES`] more frames with the
//! frame graph's GPU timing on and logs the mean GPU time of the main pass (the sky draws in it),
//! of the skinning pass (`--skinned`), of the sky cube's face pass and of the whole frame. Timing
//! comes last so it doesn't touch the CPU and allocation numbers, which stay comparable with
//! builds that have none.

use std::time::Duration;

use sapphire_core::alloc;
use sapphire_platform::{AppInitError, Platform};
use sapphire_rhi::wgpu;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::offscreen::Stage;
use crate::options::Options;

/// Frames run before measuring: first-use pipeline work and buffer growth settle here.
const WARMUP_FRAMES: u32 = 60;

/// Frames run with GPU timing on, after the CPU measurement.
const TIMED_FRAMES: u32 = 240;

/// With `--debug-overlay` the bench measures the overlay shown; otherwise, in `dev` builds,
/// its hooks with it hidden.
pub(crate) fn run(
    platform: &Platform,
    options: &Options,
    frames: u32,
    width: u32,
    height: u32,
) -> Result<(), AppInitError> {
    if frames == 0 {
        return Err(AppInitError::msg("--bench needs at least one frame"));
    }
    let startup = Stopwatch::start();
    let mut stage = Stage::new(platform, options)?;
    let startup = startup.elapsed();

    let target = stage.target(width, height, wgpu::TextureUsages::empty());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut cpu = Vec::with_capacity(frames.min(1 << 20) as usize);
    let mut gpu_wait = Duration::ZERO;
    let (mut allocations, mut bytes) = (0, 0);
    // Time of day's resolve, per measured frame: its total and its slowest.
    let (mut tod_total, mut tod_max) = (Duration::ZERO, Duration::ZERO);
    // GPU pass spans of the measured frames that came back: (newest frame, total, count).
    let (mut timed_frame, mut gpu_span, mut timed) = (0, Duration::ZERO, 0_u32);
    for frame in 0..WARMUP_FRAMES + frames {
        let before = alloc::stats();
        let mut stopwatch = Stopwatch::start();
        stage.step();
        stage.render(&view, 1.0, |_, _| {})?;
        stage.scene.end_frame();
        let frame_cpu = stopwatch.lap();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        let wait = stopwatch.lap();
        let after = alloc::stats();
        if frame >= WARMUP_FRAMES {
            cpu.push(frame_cpu);
            gpu_wait += wait;
            allocations += after.total_allocations - before.total_allocations;
            bytes += after.total_bytes - before.total_bytes;
            let tod = stage.scene.time_of_day().stats().resolve_time;
            tod_total += tod;
            tod_max = tod_max.max(tod);
            if let Some(timings) = stage.graph.gpu_timings()
                && timings.frame > timed_frame.max(u64::from(WARMUP_FRAMES))
            {
                timed_frame = timings.frame;
                gpu_span += timings.span();
                timed += 1;
            }
        }
    }

    cpu.sort_unstable();
    let percentile = |p: usize| cpu[(cpu.len() - 1) * p / 100];
    let micros = |duration: Duration| duration.as_secs_f64() * 1e6;
    let quality = stage.quality.stats();
    let (scene_width, scene_height) = stage.renderer.stats().scene_size;
    let gpu_span = match timed {
        0 => "untimed".to_owned(),
        timed => format!("{:.1}", micros(gpu_span / timed)),
    };
    tracing::info!(
        target: LOG_TARGET,
        frames,
        width,
        height,
        tier = quality.tier.id.name(),
        render_scale = format_args!("{:.3}", quality.render_scale),
        scene = format_args!("{scene_width}x{scene_height}"),
        dynamic_resolution = quality.dynamic.enabled,
        msaa = options.render_settings(stage.quality.render_settings()).msaa.sample_count(),
        lights = options.lights,
        skinned = options.skinned,
        faces = options.faces,
        cpu_us_mean = format_args!("{:.1}", micros(cpu.iter().sum::<Duration>() / frames)),
        cpu_us_p50 = format_args!("{:.1}", micros(percentile(50))),
        cpu_us_p99 = format_args!("{:.1}", micros(percentile(99))),
        cpu_us_max = format_args!("{:.1}", micros(percentile(100))),
        gpu_wait_us_mean = format_args!("{:.1}", micros(gpu_wait / frames)),
        gpu_span_us_mean = %gpu_span,
        timed_frames = timed,
        allocs_per_frame = format_args!("{:.1}", allocations as f64 / f64::from(frames)),
        tod_us_mean = format_args!("{:.2}", micros(tod_total / frames)),
        tod_us_max = format_args!("{:.2}", micros(tod_max)),
        alloc_bytes_per_frame = bytes / u64::from(frames),
        startup_ms = format_args!("{:.1}", startup.as_secs_f64() * 1e3),
        graph = ?stage.graph.stats(),
        render = ?stage.renderer.stats(),
        "bench",
    );
    if !alloc::is_installed() {
        tracing::warn!(target: LOG_TARGET, "no counting allocator: allocation counts are zero");
    }
    gpu_times(&mut stage, &view)
}

/// Runs [`TIMED_FRAMES`] frames with GPU timing on and logs the mean GPU times: each pass's
/// `exclusive` time (what it added to the frame; see `sapphire_render::graph::PassTime`) and its
/// start-to-end `duration`, and the frame's span.
fn gpu_times(stage: &mut Stage, view: &wgpu::TextureView) -> Result<(), AppInitError> {
    if !stage.gpu.caps().timestamp_query {
        tracing::info!(target: LOG_TARGET, "no timestamp queries on this device: GPU times not measured");
        return Ok(());
    }
    stage.measure_gpu = true;
    // Sums of (exclusive, duration) per pass, and frames timed.
    let mut skinning = (Duration::ZERO, Duration::ZERO, 0u32);
    let mut sky_cube = (Duration::ZERO, Duration::ZERO, 0u32);
    let mut main = (Duration::ZERO, Duration::ZERO, 0u32);
    let (mut span, mut timed, mut last) = (Duration::ZERO, 0u32, 0);
    for _ in 0..TIMED_FRAMES {
        stage.step();
        stage.render(view, 1.0, |_, _| {})?;
        stage.scene.end_frame();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        let Some(timings) = stage.graph.gpu_timings().filter(|timings| timings.frame > last) else {
            continue;
        };
        last = timings.frame;
        timed += 1;
        span += timings.span();
        for (name, sums) in [("skinning", &mut skinning), ("main", &mut main), ("sky cube", &mut sky_cube)] {
            if let Some(pass) = timings.get(name) {
                *sums = (sums.0 + pass.exclusive, sums.1 + pass.duration, sums.2 + 1);
            }
        }
    }
    let micros = |total: Duration, count: u32| total.as_secs_f64() * 1e6 / f64::from(count.max(1));
    tracing::info!(
        target: LOG_TARGET,
        frames = timed,
        gpu_frame_us = format_args!("{:.1}", micros(span, timed)),
        gpu_main_us = format_args!("{:.1}", micros(main.0, main.2)),
        gpu_main_duration_us = format_args!("{:.1}", micros(main.1, main.2)),
        gpu_skinning_us = format_args!("{:.1}", micros(skinning.0, skinning.2)),
        gpu_skinning_duration_us = format_args!("{:.1}", micros(skinning.1, skinning.2)),
        skinning_frames = skinning.2,
        gpu_sky_cube_us = format_args!("{:.1}", micros(sky_cube.0, sky_cube.2)),
        gpu_sky_cube_duration_us = format_args!("{:.1}", micros(sky_cube.1, sky_cube.2)),
        sky_cube_frames = sky_cube.2,
        "bench gpu",
    );
    Ok(())
}
