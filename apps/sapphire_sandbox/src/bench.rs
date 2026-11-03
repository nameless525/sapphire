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
//! `--day-length` makes the hour move every frame; animation's time per frame with `--clip`
//! (M2 N2), against §3.2's "Animation + IK", 1.5 ms on Desktop-Low and 3.0 ms on Mobile-Low; and within
//! it spring bones' (M2 N5), against §3.2's "Secondary motion", 0.5 ms and 1.0 ms.
//!
//! While the frame graph times passes on the GPU (with `--dynamic-resolution`, or the overlay
//! shown) it also logs the mean GPU pass span of the frames whose timings came back: the GPU
//! wait alone is mostly submission latency on a light scene.
//!
//! Then, where the device has timestamp queries, it runs [`TIMED_FRAMES`] more frames with the
//! frame graph's GPU timing on and logs the mean GPU time of the main pass (the sky draws in it),
//! the prepasses (against §3.3's "Culling + prepass", 1.5 ms) and the character prepass among them
//! (M1 C5, with its draws), temporal AA (its mask and resolve, against spec §3.3's 0.8 ms), post (its bloom
//! chain, its fused composite with the compositing layers, and the two together against §3.3's
//! "Bloom + tonemap + LUT", 0.7 ms on Desktop-Low and 2.5 ms on Mobile-Low), the skinning pass
//! (`--skinned`), the sky cube's face pass, the shadow passes (the cascades drawn, the character
//! maps and the face pass, against spec §3.3's 2 ms on desktop and 3 ms on phones), the light
//! clusters, the ambient pass (the probe grid's sky, prefilter, relight and characters, and GTAO;
//! with the clusters against §3.3's "Light clustering, AO", 1.5 ms on Desktop-Low), a debug
//! view's passes (`--debug-view`, M1 F5), debug draw's pass (`--debug-draw`, M2 T1), the effects'
//! pass under temporal AA (`--vfx`, M2 E2, against §3.3's 1.2 ms on Desktop-Low) and the whole
//! frame.
//! Timing comes last so it doesn't touch the CPU and allocation numbers, which stay comparable
//! with builds that have none.
//!
//! A fight is measured whole, by phases: `crate::fight_bench`.

use std::time::Duration;

use sapphire_core::alloc;
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::{
    CHARACTER_PREPASS, MAIN_PASS, PREPASSES, UI_PASS, ambient, debug_draw, debug_view, post, shadows,
    temporal, vfx,
};
use sapphire_rhi::wgpu;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::offscreen::Stage;
use crate::options::Options;
use sapphire_stage::combat::Combat;
use sapphire_stage::combat::CombatOption;

/// Frames run before measuring: first-use pipeline work and buffer growth settle here.
const WARMUP_FRAMES: u32 = 60;

/// Frames run with GPU timing on, after the CPU measurement.
const TIMED_FRAMES: u32 = 240;

/// With `--debug-overlay` the bench measures the overlay shown; otherwise, in `dev` builds,
/// its hooks with it hidden.
pub(crate) fn run(
    platform: &Platform,
    options: &Options,
    combat: Option<Combat>,
    frames: u32,
    width: u32,
    height: u32,
) -> Result<(), AppInitError> {
    if frames == 0 {
        return Err(AppInitError::msg("--bench needs at least one frame"));
    }
    let startup = Stopwatch::start();
    let mut stage = crate::offscreen::stage(platform, options)?;
    stage.show.fight.combat = combat;
    let startup = startup.elapsed();
    if let Some(fight @ (CombatOption::Replay { .. } | CombatOption::Fight { .. })) = &options.combat {
        return crate::fight_bench::run(&mut stage, fight, options, (frames, width, height), startup);
    }

    let target = stage.target(width, height, wgpu::TextureUsages::empty());
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut cpu = Vec::with_capacity(frames.min(1 << 20) as usize);
    let mut gpu_wait = Duration::ZERO;
    let (mut allocations, mut bytes) = (0, 0);
    // Time of day's resolve, per measured frame: its total and its slowest.
    let (mut tod_total, mut tod_max) = (Duration::ZERO, Duration::ZERO);
    // The effects' simulation and frame write per measured frame (`--vfx`), and what they held.
    let (mut vfx_total, mut vfx_max) = (Duration::ZERO, Duration::ZERO);
    let (mut vfx_particles, mut vfx_points) = (0_u64, 0_u64);
    // The HUD's update and draw, with `--hud`: its total and its slowest.
    let (mut hud_total, mut hud_max) = (Duration::ZERO, Duration::ZERO);
    // Animation's per-frame time (M2 N2, `--clip`), sorted below, and the poses it evaluated.
    let mut anim = Vec::with_capacity(cpu.capacity());
    let (mut anim_evaluated, mut anim_joints, mut anim_teleports) = (0_u64, 0_u64, 0_u32);
    // And spring bones' within it, and the sub-steps they ran.
    let mut springs = Vec::with_capacity(cpu.capacity());
    let mut spring_substeps = 0_u64;
    let mut anim_ik = 0_u64;
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
            let effects = stage.renderer.stats().vfx;
            vfx_total += effects.simulation.simulation;
            vfx_max = vfx_max.max(effects.simulation.simulation);
            vfx_particles += u64::from(effects.sprites);
            vfx_points += u64::from(effects.ribbon_points);
            if let Some(hud) = &stage.show.hud {
                let (_, cost) = hud.last_frame();
                hud_total += cost;
                hud_max = hud_max.max(cost);
            }
            let stats = stage.scene.anim_stats();
            anim.push(stats.time);
            anim_evaluated += u64::from(stats.evaluated);
            anim_joints += u64::from(stats.joints);
            springs.push(stats.spring_time);
            spring_substeps += u64::from(stats.spring_substeps);
            anim_teleports = stats.teleports;
            anim_ik += u64::from(stats.ik);
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
    anim.sort_unstable();
    springs.sort_unstable();
    let percentile = |p: usize| cpu[(cpu.len() - 1) * p / 100];
    let anim_percentile = |p: usize| anim[(anim.len() - 1) * p / 100];
    let spring_percentile = |p: usize| springs[(springs.len() - 1) * p / 100];
    let micros = |duration: Duration| duration.as_secs_f64() * 1e6;
    let quality = stage.quality.stats();
    let (scene_width, scene_height) = stage.renderer.stats().scene_size;
    let settings = options.render_settings(stage.quality.render_settings());
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
        msaa = settings.msaa.sample_count(),
        taa = settings.temporal,
        shadows = settings.shadows.is_some(),
        ambient_occlusion = ?settings.ambient_occlusion,
        environment = options.environment,
        probes = options.probes,
        stage = ?options.stage,
        lights = options.lights,
        skinned = options.skinned,
        faces = ?options.faces,
        rim_rig = ?options.rim_rig,
        foliage = ?options.foliage,
        arena = ?options.arena,
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
        vfx_show = ?options.vfx,
        vfx_sim_us_mean = format_args!("{:.2}", micros(vfx_total / frames)),
        vfx_sim_us_max = format_args!("{:.2}", micros(vfx_max)),
        vfx_sprites_mean = vfx_particles / u64::from(frames),
        vfx_ribbon_points_mean = vfx_points / u64::from(frames),
        hud_us_mean = format_args!("{:.2}", micros(hud_total / frames)),
        hud_us_max = format_args!("{:.2}", micros(hud_max)),
        clip = ?options.clip,
        anim_graph = ?options.graph,
        ik = ?options.ik.as_ref().map(|ik| ik.ground),
        anim_ik_per_frame = format_args!("{:.2}", anim_ik as f64 / f64::from(frames)),
        anim_us_p50 = format_args!("{:.2}", micros(anim_percentile(50))),
        anim_us_p99 = format_args!("{:.2}", micros(anim_percentile(99))),
        anim_us_max = format_args!("{:.2}", micros(anim_percentile(100))),
        anim_poses_per_frame = format_args!("{:.2}", anim_evaluated as f64 / f64::from(frames)),
        anim_joints_per_frame = format_args!("{:.1}", anim_joints as f64 / f64::from(frames)),
        springs = ?options.springs,
        spring_us_p50 = format_args!("{:.2}", micros(spring_percentile(50))),
        spring_us_p99 = format_args!("{:.2}", micros(spring_percentile(99))),
        spring_us_max = format_args!("{:.2}", micros(spring_percentile(100))),
        spring_substeps_per_frame = format_args!("{:.2}", spring_substeps as f64 / f64::from(frames)),
        anim_teleports,
        alloc_bytes_per_frame = bytes / u64::from(frames),
        startup_ms = format_args!("{:.1}", startup.as_secs_f64() * 1e3),
        graph = ?stage.graph.stats(),
        render = ?stage.renderer.stats(),
        "bench",
    );
    if !alloc::is_installed() {
        tracing::warn!(target: LOG_TARGET, "no counting allocator: allocation counts are zero");
    }
    gpu_times(&mut stage, &view, settings.bloom_levels)?;
    stage.check_gpu()?;
    stage.show.fight.finish().map_err(AppInitError::msg)
}

/// Runs [`TIMED_FRAMES`] frames with GPU timing on and logs the mean GPU times: each pass's
/// `exclusive` time (what it added to the frame; see `sapphire_render::graph::PassTime`) and its
/// start-to-end `duration`, and the frame's span. `bloom_levels` is the renderer's.
fn gpu_times(stage: &mut Stage, view: &wgpu::TextureView, bloom_levels: u32) -> Result<(), AppInitError> {
    if !stage.gpu.caps().timestamp_query {
        tracing::info!(target: LOG_TARGET, "no timestamp queries on this device: GPU times not measured");
        return Ok(());
    }
    stage.measure_gpu = true;
    // Sums of (exclusive, duration) per group of passes, and the frames it ran in.
    type Sums = (Duration, Duration, u32);
    let zero: Sums = (Duration::ZERO, Duration::ZERO, 0);
    let (mut skinning, mut sky_cube, mut main, mut prepasses, mut taa, mut shadow) =
        (zero, zero, zero, zero, zero, zero);
    let (mut character_prepass, mut clusters, mut ambient_pass, mut views) = (zero, zero, zero, zero);
    let mut debug_draws = zero;
    let mut effects = zero;
    let mut ui = zero;
    // Post's passes are summed only over frames whose timings hold every one of them and the
    // main pass before them: on Apple GPUs some frames come back with some passes' timestamps
    // invalid, and a missing pass leaves its time in the next one's exclusive time.
    let (mut bloom, mut composite, mut post_total) = (zero, zero, zero);
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
        // Temporal AA's cost is its two passes, against spec §3.3's 0.8 ms; the prepasses it
        // runs are its inputs, timed apart; post's composite runs at the output's size either way,
        // and with its bloom chain is §3.3's "Bloom + tonemap + LUT".
        // The ambient pass (M1 W5): the grid's work and GTAO. A debug view's replay and its pass
        // over the frame (M1 F5): what showing it costs.
        let groups: [(&[&str], &mut Sums); 13] = [
            (&["skinning"], &mut skinning),
            (&["sky cube"], &mut sky_cube),
            (&[MAIN_PASS], &mut main),
            (&PREPASSES, &mut prepasses),
            (&[CHARACTER_PREPASS], &mut character_prepass),
            (&temporal::PASSES, &mut taa),
            (&shadows::PASSES, &mut shadow),
            (&["light clusters"], &mut clusters),
            (&ambient::PASSES, &mut ambient_pass),
            (&debug_view::PASSES, &mut views),
            (&[debug_draw::PASS], &mut debug_draws),
            (&[vfx::PASS], &mut effects),
            (&[UI_PASS], &mut ui),
        ];
        for (names, sums) in groups {
            let mut ran = false;
            for pass in names.iter().filter_map(|name| timings.get(name)) {
                sums.0 += pass.exclusive;
                sums.1 += pass.duration;
                ran = true;
            }
            sums.2 += u32::from(ran);
        }
        let chain = stage.renderer.stats().post.bloom_chain;
        let post_passes = || post::frame_passes(bloom_levels, chain);
        if timings.get(MAIN_PASS).is_some() && post_passes().all(|name| timings.get(name).is_some()) {
            for name in post_passes() {
                let pass = timings.get(name).expect("checked above");
                let sums = if name == post::COMPOSITE_PASS { &mut composite } else { &mut bloom };
                sums.0 += pass.exclusive;
                sums.1 += pass.duration;
                post_total.0 += pass.exclusive;
                post_total.1 += pass.duration;
            }
            for sums in [&mut bloom, &mut composite, &mut post_total] {
                sums.2 += 1;
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
        gpu_prepasses_us = format_args!("{:.1}", micros(prepasses.0, prepasses.2)),
        gpu_character_prepass_us = format_args!("{:.1}", micros(character_prepass.0, character_prepass.2)),
        gpu_character_prepass_duration_us =
            format_args!("{:.1}", micros(character_prepass.1, character_prepass.2)),
        character_prepass_draws = stage.renderer.stats().character_prepass.draws,
        eye_layer_draws = stage.renderer.stats().character_prepass.eye_layer_draws,
        gpu_taa_us = format_args!("{:.1}", micros(taa.0, taa.2)),
        gpu_taa_duration_us = format_args!("{:.1}", micros(taa.1, taa.2)),
        taa_frames = taa.2,
        gpu_post_us = format_args!("{:.1}", micros(composite.0, composite.2)),
        gpu_bloom_us = format_args!("{:.1}", micros(bloom.0, bloom.2)),
        gpu_post_total_us = format_args!("{:.1}", micros(post_total.0, post_total.2)),
        post_frames = post_total.2,
        gpu_skinning_us = format_args!("{:.1}", micros(skinning.0, skinning.2)),
        gpu_skinning_duration_us = format_args!("{:.1}", micros(skinning.1, skinning.2)),
        skinning_frames = skinning.2,
        gpu_sky_cube_us = format_args!("{:.1}", micros(sky_cube.0, sky_cube.2)),
        gpu_sky_cube_duration_us = format_args!("{:.1}", micros(sky_cube.1, sky_cube.2)),
        sky_cube_frames = sky_cube.2,
        gpu_shadows_us = format_args!("{:.1}", micros(shadow.0, shadow.2)),
        gpu_shadows_duration_us = format_args!("{:.1}", micros(shadow.1, shadow.2)),
        shadow_frames = shadow.2,
        gpu_light_clusters_us = format_args!("{:.1}", micros(clusters.0, clusters.2)),
        light_cluster_frames = clusters.2,
        gpu_ambient_us = format_args!("{:.1}", micros(ambient_pass.0, ambient_pass.2)),
        ambient_frames = ambient_pass.2,
        gpu_clustering_ao_us =
            format_args!("{:.1}", micros(clusters.0, clusters.2) + micros(ambient_pass.0, ambient_pass.2)),
        gpu_debug_view_us = format_args!("{:.1}", micros(views.0, views.2)),
        gpu_debug_view_duration_us = format_args!("{:.1}", micros(views.1, views.2)),
        debug_view_frames = views.2,
        gpu_debug_draw_us = format_args!("{:.1}", micros(debug_draws.0, debug_draws.2)),
        gpu_debug_draw_duration_us = format_args!("{:.1}", micros(debug_draws.1, debug_draws.2)),
        debug_draw_lines = stage.renderer.stats().debug_draw.drawn,
        debug_draw_frames = debug_draws.2,
        gpu_vfx_us = format_args!("{:.1}", micros(effects.0, effects.2)),
        gpu_vfx_duration_us = format_args!("{:.1}", micros(effects.1, effects.2)),
        vfx_frames = effects.2,
        gpu_ui_us = format_args!("{:.1}", micros(ui.0, ui.2)),
        gpu_ui_duration_us = format_args!("{:.1}", micros(ui.1, ui.2)),
        ui_frames = ui.2,
        "bench gpu",
    );
    Ok(())
}
