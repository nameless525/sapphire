//! `--bench <frames>`: measures the sandbox's per-frame CPU path without a window (see
//! [`crate::offscreen`]).
//!
//! Each frame runs one fixed step, `Renderer::prepare`, encoding with `Renderer::record` and
//! the submit against an offscreen target, then waits for the GPU so frames don't queue up.
//! After a warm-up it logs CPU time per frame (that span, without the GPU wait) and
//! allocations per frame (the whole frame, wait included, like the windowed stats). It leaves
//! out what only a window has: acquiring and presenting the swapchain, and vsync. A fast,
//! repeatable check for changes to the frame path against the budgets of spec §3.

use std::time::Duration;

use sapphire_core::alloc;
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::Msaa;
use sapphire_rhi::wgpu;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::offscreen::Stage;

/// Frames run before measuring: first-use pipeline work and buffer growth settle here.
const WARMUP_FRAMES: u32 = 60;

pub(crate) fn run(
    platform: &Platform,
    msaa: Msaa,
    frames: u32,
    width: u32,
    height: u32,
) -> Result<(), AppInitError> {
    if frames == 0 {
        return Err(AppInitError::msg("--bench needs at least one frame"));
    }
    let startup = Stopwatch::start();
    let mut stage = Stage::new(platform, msaa)?;
    let startup = startup.elapsed();

    let target = stage.target(width, height, wgpu::TextureUsages::empty());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let aspect = width as f32 / height as f32;
    let mut cpu = Vec::with_capacity(frames.min(1 << 20) as usize);
    let mut gpu_wait = Duration::ZERO;
    let (mut allocations, mut bytes) = (0, 0);
    for frame in 0..WARMUP_FRAMES + frames {
        let before = alloc::stats();
        let mut stopwatch = Stopwatch::start();
        stage.scene.step();
        let encoder = stage.encode(&view, aspect, 1.0);
        stage.gpu.queue().submit([encoder.finish()]);
        let frame_cpu = stopwatch.lap();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        let wait = stopwatch.lap();
        let after = alloc::stats();
        if frame >= WARMUP_FRAMES {
            cpu.push(frame_cpu);
            gpu_wait += wait;
            allocations += after.total_allocations - before.total_allocations;
            bytes += after.total_bytes - before.total_bytes;
        }
    }

    cpu.sort_unstable();
    let percentile = |p: usize| cpu[(cpu.len() - 1) * p / 100];
    let micros = |duration: Duration| duration.as_secs_f64() * 1e6;
    tracing::info!(
        target: LOG_TARGET,
        frames,
        width,
        height,
        msaa = msaa.sample_count(),
        cpu_us_mean = format_args!("{:.1}", micros(cpu.iter().sum::<Duration>() / frames)),
        cpu_us_p50 = format_args!("{:.1}", micros(percentile(50))),
        cpu_us_p99 = format_args!("{:.1}", micros(percentile(99))),
        cpu_us_max = format_args!("{:.1}", micros(percentile(100))),
        gpu_wait_us_mean = format_args!("{:.1}", micros(gpu_wait / frames)),
        allocs_per_frame = format_args!("{:.1}", allocations as f64 / f64::from(frames)),
        alloc_bytes_per_frame = bytes / u64::from(frames),
        startup_ms = format_args!("{:.1}", startup.as_secs_f64() * 1e3),
        "bench",
    );
    if !alloc::is_installed() {
        tracing::warn!(target: LOG_TARGET, "no counting allocator: allocation counts are zero");
    }
    Ok(())
}
