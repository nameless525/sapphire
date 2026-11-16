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
//! pass under temporal AA (`--vfx`, M2 E2, against §3.3's 1.2 ms on Desktop-Low), the effects' bends' offsets
//! (`--vfx distort-*`, M5 L6, with the effects' pass §3.3's "Particles, distortion"), a 3D inset's
//! passes (`--ui-game character`, M3 U6: with `--ui-spin` it turns, and so draws, every frame), a phone tier's
//! upscale and sharpen below scale 1 (M5 L14: `gpu_upscale_us`, each pass apart, against §3.3's post row with
//! post's own) and the whole frame.
//! Timing comes last so it doesn't touch the CPU and allocation numbers, which stay comparable
//! with builds that have none.
//!
//! A fight is measured whole, by phases: `crate::fight_bench`, and so is a zone stage's walk (`--zone
//! --zone-walk`, M5 B4) but with `--draw-every`. A zone stage (`--zone`, M3 C1) adds what walking a zone costs,
//! from its first frame on ([`ZoneWalk`]), and fails if a frame had to build a pipeline, as a fight's bench does;
//! a walk fails too if a frame waited on the pipelines' warm-up ([`check_no_waits`], M5 Z3).
//! A walk under `--draw-every` logs the count rows of the frames it drew too, as the fight's bench logs them
//! (`fight_bench::DrawnCounts`, M5 X4): what CI's scale lane holds. So does a zone stage standing still under
//! `--draw-every` (M5 C11): a view held, such as a crowd's densest.
//!
//! From its first frame to its last a bench's thread is the frame's, in play (`sapphire_core::frame_loads`, exit
//! test 2): what it reads, decodes or checks of an asset meanwhile is counted, and a zone stage's walk logs it
//! (`frame_thread_loads=`, by kind) and as the count row `frame thread loads`, whose budget is 0. A fight's bench
//! does the same from its first frame, the audio and stage ready.
//!
//! Every bench first logs the class its OS schedules its threads in, the main thread's and the compute pool's,
//! then asks for the main thread to be scheduled as user-interactive, as a game the user started is
//! ([`schedule`]): a run started from a background shell would otherwise measure what the shell's class allows.

use std::num::NonZeroU32;
use std::time::Duration;

use sapphire_core::alloc;
use sapphire_core::frame_loads::{self, FrameLoads, InPlay};
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::{
    CHARACTER_PREPASS, DISTORTION_PASS, MAIN_PASS, PREPASSES, UI_PASS, ambient, debug_draw, debug_view,
    inset, post, shadows, temporal, vfx,
};
use sapphire_rhi::wgpu;
use sapphire_time::Stopwatch;

use crate::LOG_TARGET;
use crate::fight_bench::DrawnCounts;
use crate::offscreen::Stage;
use crate::options::Options;
use game_combat::combat::CombatOption;

/// Frames run before measuring: first-use pipeline work and buffer growth settle here.
const WARMUP_FRAMES: u32 = 60;

/// Frames run with GPU timing on, after the CPU measurement.
const TIMED_FRAMES: u32 = 240;

/// What a walk through a zone costs, each frame from the first, warm-up and all: the walk's first steps
/// are what a warm-up would hide.
#[derive(Default)]
struct ZoneWalk {
    /// [`Streamer::update`](sapphire_world::streaming::Streamer::update)'s time.
    streaming: Vec<Duration>,
    /// `Content::update`, and so the uploads, with streaming's own time taken out.
    uploads: Vec<Duration>,
    /// What each frame uploaded, and the frames that uploaded anything.
    upload_bytes: u64,
    upload_most: u64,
    upload_frames: u32,
    /// The most the pools have held.
    texture_peak: u64,
    mesh_peak: u64,
    /// The frame's thread's loads in play as the walk began (exit test 2, `sapphire_core::frame_loads`).
    loads: FrameLoads,
}

impl ZoneWalk {
    fn frame(&mut self, stage: &mut Stage) {
        // The laps add up until someone takes them, here as in a fight's bench.
        let laps = std::mem::take(&mut stage.laps);
        let Some(stream) = stage.scene.stream_stats() else { return };
        self.streaming.push(stream.time);
        self.uploads.push(laps.content.saturating_sub(stream.time));
        let uploaded = stage.content.stats().last_upload.bytes;
        self.upload_bytes += uploaded;
        self.upload_most = self.upload_most.max(uploaded);
        self.upload_frames += u32::from(uploaded > 0);
        self.texture_peak = self.texture_peak.max(stage.content.texture_bytes());
        self.mesh_peak = self.mesh_peak.max(stage.renderer.stats().mesh_pool_bytes);
    }

    /// Says what it measured, against the budgets of m3-plan §3.2 and the asset server's.
    fn log(mut self, stage: &Stage, options: &Options) {
        let (Some(stream), false) = (stage.scene.stream_stats(), self.streaming.is_empty()) else { return };
        self.streaming.sort_unstable();
        self.uploads.sort_unstable();
        let at = |frames: &[Duration], p: usize| frames[(frames.len() - 1) * p / 100].as_secs_f64() * 1e6;
        let settings = options.stream.settings(stage.quality.active().id);
        let builds = stage.renderer.stats().pipelines;
        let (now, then) = (frame_loads::loads(), self.loads);
        tracing::info!(
            target: LOG_TARGET,
            frames = self.streaming.len(),
            streaming_us_p50 = format_args!("{:.1}", at(&self.streaming, 50)),
            streaming_us_p99 = format_args!("{:.1}", at(&self.streaming, 99)),
            streaming_us_worst = format_args!("{:.1}", at(&self.streaming, 100)),
            streaming_us_budget = settings.map(|settings| settings.budget.micros),
            over_budget_frames = stream.over_budget_frames,
            uploads_us_p50 = format_args!("{:.1}", at(&self.uploads, 50)),
            uploads_us_p99 = format_args!("{:.1}", at(&self.uploads, 99)),
            uploads_us_worst = format_args!("{:.1}", at(&self.uploads, 100)),
            upload_bytes = self.upload_bytes,
            upload_bytes_most = self.upload_most,
            upload_frames = self.upload_frames,
            placed = stream.total_placed,
            removed = stream.total_removed,
            texture_bytes_peak = self.texture_peak,
            mesh_bytes_peak = self.mesh_peak,
            prewarmed = builds.prewarmed,
            first_use = builds.first_use_builds,
            rebuilt = builds.rebuilt,
            warmed_off_frame = builds.warmed_off_frame,
            frame_thread = builds.frame_thread_builds,
            frame_waits = builds.frame_waits,
            frame_thread_loads = now.total() - then.total(),
            frame_thread_reads = now.reads - then.reads,
            frame_thread_decodes = now.decodes - then.decodes,
            frame_thread_builds = now.builds - then.builds,
            "zone walk"
        );
    }
}

/// Exit test 7: a load leaves no frame anything to build, wherever it would come from; and exit test 1: after
/// the first frame no frame builds a pipeline itself, the warm-up building them off the frame (M4 H1).
pub(crate) fn check_first_uses(stage: &Stage) -> Result<(), AppInitError> {
    let builds = stage.renderer.stats().pipelines;
    if builds.first_use_builds == 0 && builds.frame_thread_builds == 0 {
        return Ok(());
    }
    let report = stage.renderer.first_use_report();
    if builds.first_use_builds == 0 {
        return Err(AppInitError::msg(format!("a frame built pipelines the warm-up should have: {report}")));
    }
    Err(AppInitError::msg(format!("the load's pipeline inventory missed: {report}")))
}

/// Exit test 1 for a zone walk (M5 Z3): a frame that waited on the pipelines' warm-up stalled as surely as one
/// that built, so a walk fails on any wait, as a fight's bench report does.
pub(crate) fn check_no_waits(builds: sapphire_render::PipelineStats) -> Result<(), String> {
    match builds.frame_waits {
        0 => Ok(()),
        waits => {
            Err(format!("the walk's frames waited on the pipelines' warm-up {waits} time(s) after its first"))
        }
    }
}

/// The frames a counting walk's end gives the mesh pool to come back to its floor once the zone is closed: a
/// compaction moves a few parts a frame, and the frees land at the update after the close.
const GIVE_BACK_FRAMES: u32 = 120;

/// Exit test 2's floor where the zone has no exit (M5 Z2): the counting walk's end takes the zone down and
/// unmounts its pack as a trip leaves it, asks the mesh pool to give back what the zone held, as a trip's
/// arrival does, and draws frames until the pool is at its floor or [`GIVE_BACK_FRAMES`] have passed. Logs
/// `zone closed`: the pool's capacity and floor before and after, whether it came back, and in how many
/// frames. Runs after every count row is logged, so it moves none.
fn close_the_zone(stage: &mut Stage, view: &wgpu::TextureView) -> Result<(), AppInitError> {
    let before = stage.renderer.stats();
    let (capacity_before, floor_before) = (before.mesh_pool_capacity, before.mesh_pool.floor);
    let unmounted = match stage.scene.close_zone(&mut stage.content) {
        Some(Ok(_)) => "unmounted",
        Some(Err(error)) => {
            tracing::warn!(target: LOG_TARGET, %error, "the zone's pack didn't unmount");
            "failed"
        }
        None => "no zone",
    };
    stage.content.give_back_meshes();
    let mut frames = 0;
    while frames < GIVE_BACK_FRAMES && !stage.content.meshes_given_back() {
        stage.step();
        stage.render(view, 1.0, |_, _| {})?;
        stage.scene.end_frame();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        frames += 1;
    }
    let at_floor = stage.content.meshes_given_back();
    stage.content.end_giving_back();
    let after = stage.renderer.stats();
    tracing::info!(
        target: LOG_TARGET,
        unmounted,
        frames,
        at_floor,
        capacity_before,
        floor_before,
        capacity = after.mesh_pool_capacity,
        floor = after.mesh_pool.floor,
        in_use = after.mesh_pool_bytes,
        meshes_freed = after.meshes_freed,
        "zone closed"
    );
    Ok(())
}

/// Whether a counting walk (`walk`: a zone walk under `--draw-every`) is put back at its rail's start after frame
/// `frame`: through the warm-up, so its count rows start at its first step rather than [`WARMUP_FRAMES`] steps in
/// (M5 Z3). The warm-up still steps, streams and draws there, which is what it's for.
fn holds_at_start(frame: u64, walk: bool) -> bool {
    walk && frame < u64::from(WARMUP_FRAMES)
}

/// A counting walk's pace under `--bench-pace` (M5 Z2): each frame starts no sooner than `period` after the last
/// began, on [`Stopwatch`]'s clock (R2). One that ran long starts the next at once, never catching up on the
/// time it lost, as a game's frame waits for its next swap.
struct Pace {
    period: Duration,
    last: Option<Stopwatch>,
}

impl Pace {
    fn new(period: Duration) -> Self {
        Self { period, last: None }
    }

    /// Waits for the next frame's start, and starts it.
    fn wait(&mut self) {
        if let Some(last) = &self.last {
            let left = pace_left(last.elapsed(), self.period);
            if !left.is_zero() {
                std::thread::sleep(left);
            }
        }
        self.last = Some(Stopwatch::start());
    }
}

/// What's left of a paced frame `since` after it began: none once it has run its `period` or longer.
fn pace_left(since: Duration, period: Duration) -> Duration {
    period.saturating_sub(since)
}

/// Whether frame `frame` runs its graph under `--draw-every <every>`: a counting walk's every `every`th from its
/// first measured frame, so the frame of its first step is drawn; anything else's every `every`th from frame 0.
fn drawn_frame(frame: u64, every: u64, walk: bool) -> bool {
    let from = if walk { u64::from(WARMUP_FRAMES) } else { 0 };
    frame.abs_diff(from) % every == 0
}

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
    let mut stage = crate::offscreen::stage(platform, options)?;
    let startup = startup.elapsed();
    // After the stage, which starts the job system with the app's settings.
    schedule();
    if let Some(fight @ (CombatOption::Replay { .. } | CombatOption::Fight { .. })) = &options.combat {
        return crate::fight_bench::run(&mut stage, Some(fight), options, (frames, width, height), startup);
    }
    if options.zone_walk.is_some() && options.draw_every.is_none() {
        return crate::fight_bench::run(&mut stage, None, options, (frames, width, height), startup);
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
    let (mut vfx_particles, mut vfx_points, mut vfx_meshes, mut vfx_decals) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut vfx_distortions = 0_u64;
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
    // In play from the first frame, the stage built: what this thread loads from now on counts (exit test 2).
    let in_play = InPlay::begin();
    let mut zone_walk = ZoneWalk { loads: frame_loads::loads(), ..ZoneWalk::default() };
    let spin = options.ui_screen.as_ref().map_or(0.0, |shown| shown.spin);
    // `--draw-every`, on a zone (M3 K1): every frame steps, streams, prepares and records, and one in `n` runs its
    // graph, so a pipeline any of them builds still counts, on the frame it's built or the next one drawn.
    let draw_every = options.draw_every.map(|every| u64::from(every.get()));
    // What a zone stage's drawn frames submitted, as count rows, walking (M5 X4) or held (M5 C11).
    let mut drawn = draw_every
        .filter(|_| options.zone.is_some())
        .map(|every| DrawnCounts::new(&stage, (u64::from(frames) / every + 1) as usize));
    // A counting walk (M5 Z3) holds at its start through the warm-up and draws its first step.
    let counting_walk = drawn.is_some() && options.zone_walk.is_some();
    // Its waits for a texture's level from here, logged at its end as a fight's bench logs a phase's (M5 Z2).
    stage.content.start_texture_waits_over();
    // The walk's first load is a zone's arrival, which a trip's wipe covers in the game (M5 Z2): until the first
    // view's textures are first at their plan, waits that open are the arrival's, counted apart.
    if counting_walk {
        stage.content.open_textures_as_arrival();
    }
    // `--bench-pace` (M5 Z2): each frame starts a frame's time after the last began, or late if that ran long.
    let mut pace = options.bench_pace.map(|rate| Pace::new(Duration::from_secs(1) / rate.get()));
    for frame in 0..u64::from(WARMUP_FRAMES) + u64::from(frames) {
        if let Some(pace) = &mut pace {
            pace.wait();
        }
        stage.lose_frames = draw_every.is_some_and(|every| !drawn_frame(frame, every, counting_walk));
        let before = alloc::stats();
        let mut stopwatch = Stopwatch::start();
        spin_insets(&mut stage, spin);
        stage.step();
        stage.render(&view, 1.0, |_, _| {})?;
        stage.scene.end_frame();
        let frame_cpu = stopwatch.lap();
        stage.gpu.device().poll(wgpu::PollType::wait_indefinitely()).map_err(AppInitError::msg)?;
        let wait = stopwatch.lap();
        let after = alloc::stats();
        if stage.content.textures_arriving() && stage.scene.zone_arrived(&stage.content) {
            stage.content.end_texture_arrival();
        }
        zone_walk.frame(&mut stage);
        if let Some(drawn) = &mut drawn {
            let measured = frame >= u64::from(WARMUP_FRAMES);
            drawn.frame(&stage, frame, measured && !stage.lose_frames);
        }
        if holds_at_start(frame, counting_walk) {
            stage.scene.restart_zone_walk();
        }
        if frame >= u64::from(WARMUP_FRAMES) {
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
            vfx_meshes += u64::from(effects.meshes);
            vfx_decals += u64::from(effects.decals);
            vfx_distortions += u64::from(effects.distortions);
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
    drop(in_play);
    stage.lose_frames = false;
    zone_walk.log(&stage, options);
    if let Some(drawn) = &drawn {
        drawn.log(&stage);
        // Exit test 2's waits over the counting walk, as its one phase, `drawn` (while streaming).
        crate::fight_bench::report_texture_waits("drawn", &stage.content.texture_streaming_stats());
    }
    if options.zone.is_some() {
        check_first_uses(&stage)?;
    }
    if options.zone_walk.is_some() {
        check_no_waits(stage.renderer.stats().pipelines).map_err(AppInitError::msg)?;
    }
    // What the loads held before the GPU took them, and the texture pool's most, against their budgets (M5 M2).
    let assets = stage.content.stats();
    let texture = sapphire_assets::AssetKind::Texture.index();
    tracing::info!(
        target: LOG_TARGET,
        admitted_peak = assets.admitted_peak,
        admission = assets.budgets.admission,
        held_back = assets.held_back,
        texture_peak = assets.resident_peak[texture],
        texture_pool = assets.budgets.resident[texture],
        "asset budgets"
    );
    if let Some(stream) = stage.scene.stream_stats() {
        tracing::info!(
            target: LOG_TARGET,
            places = stream.places,
            active = stream.active,
            failed = stream.failed,
            entities = stream.entities,
            placed = stream.total_placed,
            removed = stream.total_removed,
            peak_us = format_args!("{:.1}", micros(stream.peak_time)),
            over_budget_frames = stream.over_budget_frames,
            texture_bytes = stage.content.texture_bytes(),
            mesh_pool_bytes = stage.renderer.stats().mesh_pool_bytes,
            mesh_pool_capacity = stage.renderer.stats().mesh_pool_capacity,
            meshes_freed = stage.renderer.stats().meshes_freed,
            "streaming"
        );
    }
    if counting_walk {
        close_the_zone(&mut stage, &view)?;
    }
    if draw_every.is_some() {
        // Most frames lost: what's left checks the load's inventory and measures nothing, as a fight's does.
        let paced = options.bench_pace.map_or(0, NonZeroU32::get);
        tracing::info!(target: LOG_TARGET, frames, paced, "one frame in --draw-every drawn: nothing measured");
        stage.check_gpu()?;
        return stage.show.fight.finish().map_err(AppInitError::msg);
    }
    tracing::info!(
        target: LOG_TARGET,
        frames,
        width,
        height,
        tier = quality.tier.id.name(),
        render_scale = format_args!("{:.3}", quality.render_scale),
        scene = format_args!("{scene_width}x{scene_height}"),
        dynamic_resolution = quality.dynamic.enabled,
        rung = quality.ladder.state.depth,
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
        vfx_meshes_mean = vfx_meshes / u64::from(frames),
        vfx_decals_mean = vfx_decals / u64::from(frames),
        vfx_distortions_mean = vfx_distortions / u64::from(frames),
        hud_us_mean = format_args!("{:.2}", micros(hud_total / frames)),
        hud_us_max = format_args!("{:.2}", micros(hud_max)),
        ui_quads = stage.renderer.stats().ui.quads,
        ui_draws = stage.renderer.stats().ui.draws,
        ui_tree_us = stage.renderer.stats().ui.tree.micros,
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
    gpu_times(&mut stage, &view, settings.bloom_levels, spin)?;
    stage.check_gpu()?;
    stage.show.fight.finish().map_err(AppInitError::msg)
}

/// Logs the classes the OS schedules the bench's threads in as it was started, the main thread's and the compute
/// pool's (a sample of its workers), and the kinds of core they were on; then asks for the main thread to be
/// scheduled as user-interactive, as an app the user starts has its main thread, and logs whether it took. Where
/// the OS has no classes, it logs that they're unknown.
fn schedule() {
    let class = |class: Option<sapphire_platform::ThreadClass>| class.map_or("unknown", |class| class.name());
    let core = |core: Option<sapphire_platform::CoreKind>| core.map_or("unknown", |core| core.name());
    let main = (sapphire_platform::thread_class(), sapphire_platform::current_core());
    let workers = sapphire_jobs::scope(|scope| {
        for _ in 0..sapphire_jobs::system().compute_pool().thread_num() {
            scope.spawn(|| (sapphire_platform::thread_class(), sapphire_platform::current_core()));
        }
    });
    let mut classes = String::new();
    for (worker_class, worker_core) in &workers {
        classes.push_str(class(*worker_class));
        classes.push('/');
        classes.push_str(core(*worker_core));
        classes.push(' ');
    }
    let took = sapphire_platform::set_thread_class(sapphire_platform::ThreadClass::UserInteractive);
    tracing::info!(
        target: LOG_TARGET,
        main = class(main.0),
        main_core = core(main.1),
        workers = classes.trim_end(),
        layout = ?sapphire_platform::core_layout(),
        set_user_interactive = took,
        now = class(sapphire_platform::thread_class()),
        "bench scheduling",
    );
}

/// Turns the screen's insets by `across` (`--ui-spin`), so they draw again this frame.
fn spin_insets(stage: &mut Stage, across: f32) {
    if across != 0.0
        && let Some(screen) = &mut stage.show.screen
    {
        screen.turn_insets(across);
    }
}

/// The UI pass's median exclusive time, and the median of its share of the rest of its frame in
/// thousandths. The share holds still where other work on the machine moves the GPU's clocks, and
/// with them every time, from frame to frame by two or three times.
fn ui_medians(frames: &[(Duration, Duration)]) -> (Duration, f64) {
    let mut own: Vec<Duration> = frames.iter().map(|frame| frame.0).collect();
    let mut share: Vec<f64> = frames
        .iter()
        .map(|&(ui, span)| ui.as_secs_f64() * 1000.0 / span.saturating_sub(ui).as_secs_f64().max(1e-9))
        .collect();
    own.sort_unstable();
    share.sort_by(f64::total_cmp);
    let middle = frames.len() / 2;
    (own.get(middle).copied().unwrap_or_default(), share.get(middle).copied().unwrap_or_default())
}

/// Runs [`TIMED_FRAMES`] frames with GPU timing on and logs the mean GPU times: each pass's
/// `exclusive` time (what it added to the frame; see `sapphire_render::graph::PassTime`) and its
/// start-to-end `duration`, and the frame's span, with the UI pass's medians
/// ([`ui_medians`]). `bloom_levels` is the renderer's.
fn gpu_times(
    stage: &mut Stage,
    view: &wgpu::TextureView,
    bloom_levels: u32,
    spin: f32,
) -> Result<(), AppInitError> {
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
    let mut distortion = zero;
    // The phone tiers' upscale and sharpen (M5 L14), and each one's exclusive time, in `post::UPSCALE_PASSES`'
    // order.
    let mut upscale = zero;
    let mut upscale_each = [Duration::ZERO; 2];
    let mut ui = zero;
    let mut insets = zero;
    // Depth of field's three passes on the main view, and an inset's soft backdrop (M5 L8).
    let (mut focus, mut soft_backdrop) = (zero, zero);
    // Each of the three's exclusive time, in `post::FOCUS_PASSES`' order.
    let mut focus_each = [Duration::ZERO; 3];
    // Light shafts' mask and march (M5 L13); bloom's top up level, which adds them, counts as bloom's.
    let mut shafts = zero;
    // Each frame's UI pass and its whole span, for medians that other work's swings move less.
    let mut ui_each: Vec<(Duration, Duration)> = Vec::new();
    // Post's passes are summed only over frames whose timings hold every one of them and the
    // main pass before them: on Apple GPUs some frames come back with some passes' timestamps
    // invalid, and a missing pass leaves its time in the next one's exclusive time.
    let (mut bloom, mut composite, mut post_total) = (zero, zero, zero);
    let (mut span, mut timed, mut last) = (Duration::ZERO, 0u32, 0);
    for _ in 0..TIMED_FRAMES {
        spin_insets(stage, spin);
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
        let groups: [(&[&str], &mut Sums); 19] = [
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
            (&[DISTORTION_PASS], &mut distortion),
            (&post::UPSCALE_PASSES, &mut upscale),
            (&[UI_PASS], &mut ui),
            (&inset::PASSES, &mut insets),
            (&post::FOCUS_PASSES, &mut focus),
            (&[post::FOCUS_BACKDROP_PASS], &mut soft_backdrop),
            (&post::SHAFT_PASSES, &mut shafts),
        ];
        for (sum, name) in focus_each.iter_mut().zip(post::FOCUS_PASSES) {
            *sum += timings.get(name).map_or(Duration::ZERO, |pass| pass.exclusive);
        }
        for (sum, name) in upscale_each.iter_mut().zip(post::UPSCALE_PASSES) {
            *sum += timings.get(name).map_or(Duration::ZERO, |pass| pass.exclusive);
        }
        if let Some(pass) = timings.get(UI_PASS) {
            ui_each.push((pass.exclusive, timings.span()));
        }
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
    let (ui_p50, ui_share_p50) = ui_medians(&ui_each);
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
        surface_draws = stage.renderer.stats().draws,
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
        gpu_distortion_us = format_args!("{:.1}", micros(distortion.0, distortion.2)),
        gpu_distortion_duration_us = format_args!("{:.1}", micros(distortion.1, distortion.2)),
        distortion_frames = distortion.2,
        gpu_upscale_us = format_args!("{:.1}", micros(upscale.0, upscale.2)),
        gpu_upscale_duration_us = format_args!("{:.1}", micros(upscale.1, upscale.2)),
        gpu_upscale_edge_us = format_args!("{:.1}", micros(upscale_each[0], upscale.2)),
        gpu_upscale_sharpen_us = format_args!("{:.1}", micros(upscale_each[1], upscale.2)),
        upscale_frames = upscale.2,
        gpu_ui_us = format_args!("{:.1}", micros(ui.0, ui.2)),
        gpu_ui_duration_us = format_args!("{:.1}", micros(ui.1, ui.2)),
        gpu_ui_us_p50 = format_args!("{:.1}", micros(ui_p50, 1)),
        gpu_ui_share_p50 = format_args!("{:.1}", ui_share_p50),
        ui_frames = ui.2,
        gpu_insets_us = format_args!("{:.1}", micros(insets.0, insets.2)),
        gpu_insets_duration_us = format_args!("{:.1}", micros(insets.1, insets.2)),
        inset_frames = insets.2,
        gpu_focus_us = format_args!("{:.1}", micros(focus.0, focus.2)),
        gpu_focus_duration_us = format_args!("{:.1}", micros(focus.1, focus.2)),
        focus_frames = focus.2,
        gpu_shafts_us = format_args!("{:.1}", micros(shafts.0, shafts.2)),
        gpu_shafts_duration_us = format_args!("{:.1}", micros(shafts.1, shafts.2)),
        shafts_frames = shafts.2,
        gpu_focus_prepare_us = format_args!("{:.1}", micros(focus_each[0], focus.2)),
        gpu_focus_gather_us = format_args!("{:.1}", micros(focus_each[1], focus.2)),
        gpu_focus_composite_us = format_args!("{:.1}", micros(focus_each[2], focus.2)),
        gpu_soft_backdrop_us = format_args!("{:.1}", micros(soft_backdrop.0, soft_backdrop.2)),
        soft_backdrop_frames = soft_backdrop.2,
        insets = ?stage.renderer.stats().insets,
        // The last frame's capsules by cluster (M5 L18): what a pixel walks, summed and at most.
        capsule_bins = ?stage.renderer.stats().capsule_bins,
        "bench gpu",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use sapphire_render::PipelineStats;

    use std::time::Duration;

    use super::{WARMUP_FRAMES, check_no_waits, drawn_frame, holds_at_start, pace_left};

    /// A paced frame waits out the rest of its period, and one that ran its period or longer waits for nothing.
    #[test]
    fn a_paced_frame_waits_out_its_period_and_never_catches_up() {
        let period = Duration::from_secs(1) / 60;
        assert_eq!(pace_left(Duration::ZERO, period), period);
        assert_eq!(pace_left(period / 4, period), period - period / 4);
        assert_eq!(pace_left(period, period), Duration::ZERO);
        assert_eq!(pace_left(period * 3, period), Duration::ZERO, "a long frame owes the next nothing");
    }

    /// A counting walk holds at its rail's start through the warm-up and counts from its first step: the first
    /// measured frame is drawn, and the frames drawn are every `n`th from it. A held view keeps its old cadence.
    #[test]
    fn a_counting_walk_counts_from_its_first_step() {
        let first = u64::from(WARMUP_FRAMES);
        assert!((0..first).all(|frame| holds_at_start(frame, true)), "held through the warm-up");
        assert!(
            !holds_at_start(first, true) && !holds_at_start(0, false),
            "walks from the first measured frame"
        );
        assert!(drawn_frame(first, 7, true), "the first step is drawn");
        assert!(drawn_frame(first + 7, 7, true) && !drawn_frame(first + 1, 7, true), "then every 7th");
        assert!(drawn_frame(0, 7, false) && drawn_frame(63, 7, false), "a held view's cadence stays");
    }

    /// A frame of a zone walk that waited on the warm-up stalled as surely as one that built: the walk fails.
    #[test]
    fn a_zone_walk_whose_frame_waited_on_the_warm_up_fails() {
        let warmed = PipelineStats { prewarmed: 812, warmed_off_frame: 812, ..PipelineStats::default() };
        assert!(check_no_waits(warmed).is_ok());
        let waited = check_no_waits(PipelineStats { frame_waits: 3, ..PipelineStats::default() });
        assert!(waited.is_err_and(|error| error.contains("3 time(s)")), "a wait fails, saying how many");
    }
}
