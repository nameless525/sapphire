//! Frame graph tests that need a GPU. Run with `cargo test -p sapphire_render -- --ignored`.

use sapphire_render::graph::{FrameGraph, GpuTimings, GraphExecutor, TextureId};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, TransientDesc, wgpu};

const SIZE: u32 = 4;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

#[test]
#[ignore = "needs a GPU"]
fn large_pass_weights_record_every_pass_and_serial_reports_replace_the_previous_frame() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let gpu = gpu();
    let mut executor = GraphExecutor::new();
    executor.set_report(true);
    let recorded = AtomicU32::new(0);
    let mut graph = FrameGraph::new();
    for (index, name) in ["first", "second", "third", "fourth"].into_iter().enumerate() {
        let recorded = &recorded;
        graph.add_pass(name).side_effect().weight(1 << 31).record(move |_| {
            recorded.fetch_or(1 << index, Ordering::Relaxed);
        });
    }
    executor.execute(&gpu, graph).expect("large weights are valid");
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("complete");
    assert_eq!(recorded.load(Ordering::Relaxed), 0b1111);
    assert_eq!(executor.report().expect("on").running().count(), 4);

    executor.set_serial(true);
    let mut graph = FrameGraph::new();
    graph.add_pass("serial first").side_effect().record(|_| {});
    graph.add_pass("serial second").side_effect().record(|_| {});
    executor.execute(&gpu, graph).expect("a serial graph");
    let report: Vec<_> = executor
        .report()
        .expect("on")
        .passes
        .iter()
        .map(|pass| (pass.name, pass.order, pass.command_buffer))
        .collect();
    assert_eq!(report, [("serial first", Some(0), Some(0)), ("serial second", Some(1), Some(1))]);
    assert_eq!(executor.stats().command_buffers, 2);
}

fn desc() -> TransientDesc {
    TransientDesc { width: SIZE, height: SIZE, format: FORMAT, sample_count: 1, usage: USAGE }
}

fn clear(graph: &mut FrameGraph<'_>, name: &'static str, texture: TextureId, color: wgpu::Color) {
    graph.add_pass(name).write(texture).record(move |ctx| {
        ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(name),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.view(texture),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(color), store: wgpu::StoreOp::Store },
            })],
            timestamp_writes: ctx.timestamp_writes(),
            ..Default::default()
        });
    });
}

fn copy(graph: &mut FrameGraph<'_>, name: &'static str, from: TextureId, to: TextureId) {
    graph.add_pass(name).read(from).write(to).record(move |ctx| {
        let (from, to) = (ctx.texture(from), ctx.texture(to));
        ctx.encoder.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), from.size());
    });
}

#[test]
#[ignore = "needs a GPU"]
fn graphs_cull_share_pooled_textures_and_record_in_parallel() {
    let gpu = gpu();
    let target_texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: USAGE,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&Default::default());
    let padded = (SIZE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * SIZE),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut executor = GraphExecutor::new();
    for _ in 0..2 {
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &target_view);
        let mut previous = graph.create_texture("source", desc());
        clear(&mut graph, "clear source", previous, wgpu::Color::GREEN);
        let unused = graph.create_texture("unused", desc());
        clear(&mut graph, "clear unused", unused, wgpu::Color::RED);
        for name in ["copy 1", "copy 2", "copy 3", "copy 4", "copy 5", "copy 6"] {
            let next = graph.create_texture(name, desc());
            copy(&mut graph, name, previous, next);
            previous = next;
        }
        copy(&mut graph, "to target", previous, target);
        let readback = &readback;
        graph.add_pass("readback").read(target).side_effect().record(move |ctx| {
            let texture = ctx.texture(target);
            ctx.encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(SIZE),
                    },
                },
                texture.size(),
            );
        });
        executor.execute(&gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none());
    }

    readback.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let pixels = readback.get_mapped_range(..).expect("mapped");
    assert_eq!(pixels[..4], [0, 255, 0, 255], "the source's green reached the target");

    let stats = executor.stats();
    assert_eq!((stats.passes, stats.culled), (9, 1), "only `clear unused` is culled");
    // Each copy overlaps only its neighbours, so two textures carry the whole chain.
    assert_eq!((stats.transients, stats.transient_slots), (7, 2));
    assert_eq!(stats.pool.created, 2, "the second frame reused the first frame's textures");
    // Two 4 × 4 RGBA8 textures, estimated (review RA-O06).
    let bytes = 2 * u64::from(SIZE * SIZE * 4);
    assert_eq!((stats.pool.bytes, stats.pool.peak_bytes), (bytes, bytes));
    assert!(stats.command_buffers > 1, "9 passes record in parallel: {stats:?}");
}

/// What must hold for any frame's timings from [`reports_and_gpu_timings_describe_the_executed_graph`].
fn check_timings(timings: &GpuTimings, frames: u64, encoder_writes: bool) {
    assert!(timings.frame > 0 && timings.frame <= frames);
    let clear_timed = timings.get("clear source").is_some();
    let copy_timed = timings.get("to target").is_some();
    assert_eq!(
        timings.passes.len(),
        usize::from(clear_timed) + usize::from(copy_timed),
        "only the executed passes can be timed, once each: {timings:?}"
    );
    if !encoder_writes {
        assert!(!copy_timed, "the copy needs encoder timestamps: {timings:?}");
    }
    assert_eq!(timings.every_pass, encoder_writes && clear_timed && copy_timed, "{timings:?}");
    // Every pass that took timestamps decoded: on Apple GPUs that's the clear alone, and the copy is
    // named as untimed (M3 Z1).
    let every_timed = if encoder_writes { clear_timed && copy_timed } else { clear_timed };
    assert_eq!(timings.every_timed_pass, every_timed, "{timings:?}");
    let untimed: &[&str] = if encoder_writes { &[] } else { &["to target"] };
    assert_eq!(timings.untimed, untimed, "{timings:?}");
    let exclusive: std::time::Duration = timings.passes.iter().map(|pass| pass.exclusive).sum();
    assert!(timings.passes.iter().all(|pass| pass.exclusive <= pass.duration), "{timings:?}");
    // Up to a nanosecond per pass of rounding from ticks.
    let rounding = std::time::Duration::from_nanos(timings.passes.len() as u64);
    assert!(
        exclusive <= timings.span() + rounding,
        "exclusive times add up to the span at most: {timings:?}"
    );
}

#[test]
#[ignore = "needs a GPU"]
fn reports_and_gpu_timings_describe_the_executed_graph() {
    let gpu = gpu();
    let target_texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: USAGE,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&Default::default());

    let mut executor = GraphExecutor::new();
    assert!(executor.report().is_none() && executor.gpu_timings().is_none(), "both start off");
    executor.set_report(true);
    let timing = executor.set_gpu_timing(&gpu, true);
    assert_eq!(timing, gpu.caps().timestamp_query, "timing is on wherever timestamps are");
    let encoder_writes = gpu.caps().features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
    // Whether any frame came back with the clear pass timed. Not every frame does: Apple GPUs
    // sometimes report a pass that only clears with its end before its start, and the decoder
    // drops such pairs as invalid.
    let mut clear_timed = false;
    for _ in 0..8 {
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &target_view);
        let source = graph.create_texture("source", desc());
        clear(&mut graph, "clear source", source, wgpu::Color::GREEN);
        let unused = graph.create_texture("unused", desc());
        clear(&mut graph, "clear unused", unused, wgpu::Color::RED);
        copy(&mut graph, "to target", source, target);
        executor.execute(&gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "timestamp queries validate");
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        if let Some(timings) = executor.gpu_timings() {
            check_timings(timings, executor.frames(), encoder_writes);
            clear_timed |= timings.get("clear source").is_some();
        }
    }

    let report = executor.report().expect("on");
    let passes: Vec<_> = report.passes.iter().map(|pass| (pass.name, pass.order.is_some())).collect();
    assert_eq!(passes, [("clear source", true), ("clear unused", false), ("to target", true)]);
    let source = report.textures.iter().find(|texture| texture.name == "source").expect("source");
    assert_eq!((source.slot, source.lifetime), (Some(0), Some((0, 1))));

    if !timing {
        return;
    }
    let timings = executor.gpu_timings().expect("frames came back after waiting on the GPU");
    check_timings(timings, executor.frames(), encoder_writes);
    assert!(clear_timed, "the clear pass was timed in some frame: {timings:?}");
    executor.set_gpu_timing(&gpu, false);
    assert!(executor.gpu_timings().is_none(), "turning timing off drops it");
}

#[test]
#[ignore = "needs a GPU"]
fn serial_passes_submit_one_command_buffer_each_and_draw_the_same() {
    let gpu = gpu();
    let target_texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: USAGE,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&Default::default());
    let padded = (SIZE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * SIZE),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut executor = GraphExecutor::new();
    executor.set_serial(true);
    let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
    let mut graph = FrameGraph::new();
    let target = graph.import_texture("target", &target_view);
    let mut previous = graph.create_texture("source", desc());
    clear(&mut graph, "clear source", previous, wgpu::Color::BLUE);
    for name in ["copy 1", "copy 2", "copy 3", "copy 4", "copy 5"] {
        let next = graph.create_texture(name, desc());
        copy(&mut graph, name, previous, next);
        previous = next;
    }
    copy(&mut graph, "to target", previous, target);
    let readback_buffer = &readback;
    graph.add_pass("readback").read(target).side_effect().record(move |ctx| {
        let texture = ctx.texture(target);
        ctx.encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: readback_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(SIZE),
                },
            },
            texture.size(),
        );
    });
    executor.execute(&gpu, graph).expect("a valid graph");
    assert!(pollster::block_on(scope.pop()).is_none());

    readback.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let pixels = readback.get_mapped_range(..).expect("mapped");
    assert_eq!(pixels[..4], [0, 0, 255, 255], "the source's blue reached the target");
    let stats = executor.stats();
    assert_eq!((stats.passes, stats.command_buffers), (8, 8), "one command buffer a pass: {stats:?}");
}

/// The bench's overlap mode (M6 B7) waits after frame N's submit for frame N - 2's alone, by the submission index
/// [`GraphExecutor::submitted`] gives: on each backend this runs on, that wait returns once frame N - 2 is done with its
/// map callbacks run, so a buffer it wrote reads and its GPU timings are read when frame N + 1 begins. A wait runs
/// every map callback that is done, so one begin can read two frames' timings: every frame's comes back, through
/// [`GraphExecutor::gpu_timings_read`], none dropped for a newer one. Waiting again on an index already done returns
/// at once, and waiting for the newest submission empties the queue.
#[test]
#[ignore = "needs a GPU"]
fn waiting_on_a_submission_two_frames_back_reads_its_timings() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    const FRAMES: u64 = 8;
    let gpu = gpu();
    let target_texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: USAGE,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&Default::default());
    let padded = (SIZE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    // Written by the first frame alone.
    let readback = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * SIZE),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mapped = Arc::new(AtomicBool::new(false));

    let mut executor = GraphExecutor::new();
    let timing = executor.set_gpu_timing(&gpu, true);
    let mut submitted = Vec::new();
    // Each frame's timings as the begins read them, and the most one begin read.
    let (mut read_back, mut most) = (Vec::new(), 0);
    let mut read = |executor: &GraphExecutor| {
        let read = executor.gpu_timings_read();
        assert_eq!(read.last(), read.last().and(executor.gpu_timings()), "the newest read is the latest");
        read_back.extend(read.iter().map(|timings| timings.frame));
        most = most.max(read.len());
    };
    let blue_to = || {
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &target_view);
        let source = graph.create_texture("source", desc());
        clear(&mut graph, "clear source", source, wgpu::Color::BLUE);
        copy(&mut graph, "to target", source, target);
        (graph, target)
    };
    for frame in 1..=FRAMES {
        let (mut graph, target) = blue_to();
        let readback_buffer = &readback;
        if frame == 1 {
            graph.add_pass("readback").read(target).side_effect().record(move |ctx| {
                let texture = ctx.texture(target);
                ctx.encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: readback_buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(padded),
                            rows_per_image: Some(SIZE),
                        },
                    },
                    texture.size(),
                );
            });
        }
        executor.execute(&gpu, graph).expect("a valid graph");
        assert_eq!(executor.frames(), frame);
        read(&executor);
        submitted.push(executor.submitted().cloned().expect("a frame submits"));
        if frame == 1 {
            let mapped = Arc::clone(&mapped);
            readback.map_async(wgpu::MapMode::Read, .., move |result| {
                result.expect("map");
                mapped.store(true, Ordering::Release);
            });
        }
        // This frame's begin read what the wait after the frame before it finished: frame - 3, two back from that.
        if timing && frame > 3 {
            let read = executor.gpu_timings().map_or(0, |timings| timings.frame);
            assert!(
                read >= frame - 3,
                "frame {frame} began with frame {read}'s timings, not frame {}'s",
                frame - 3
            );
        }
        if let Some(two_back) = frame.checked_sub(3).map(|at| submitted[at as usize].clone()) {
            let status = gpu
                .device()
                .poll(wgpu::PollType::Wait { submission_index: Some(two_back), timeout: None })
                .expect("a wait by submission index");
            assert!(status.wait_finished(), "frame {frame}: {status:?}");
            assert!(mapped.load(Ordering::Acquire), "frame 1's map ran by the wait for it, or one after");
        }
    }
    let pixels = readback.get_mapped_range(..).expect("mapped");
    assert_eq!(pixels[..4], [0, 0, 255, 255], "frame 1 had finished when its wait returned");
    drop(pixels);
    let again = gpu
        .device()
        .poll(wgpu::PollType::Wait { submission_index: Some(submitted[0].clone()), timeout: None });
    assert!(again.expect("an index already done").wait_finished());
    let newest = gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("a wait for the newest");
    assert!(newest.is_queue_empty(), "{newest:?}");
    // One frame more begins with every frame before it done, and reads the ones still unread.
    executor.execute(&gpu, blue_to().0).expect("a valid graph");
    read(&executor);
    if timing {
        assert_eq!(read_back, (1..=FRAMES).collect::<Vec<_>>(), "every frame's timings, in order, once each");
        eprintln!("the most frames one begin read: {most}");
    }
}
