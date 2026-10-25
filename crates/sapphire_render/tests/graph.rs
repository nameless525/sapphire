//! Frame graph tests that need a GPU. Run with `cargo test -p sapphire_render -- --ignored`.

use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
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
    assert!(stats.command_buffers > 1, "9 passes record in parallel: {stats:?}");
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
    assert!(timings.frame > 0 && timings.frame <= executor.frames());
    assert!(timings.passes.iter().any(|pass| pass.name == "clear source"), "{timings:?}");
    assert!(timings.passes.iter().all(|pass| pass.name != "clear unused"), "culled passes aren't timed");
    // The copy takes no render pass, so only devices that time between passes see it.
    assert_eq!(timings.get("to target").is_some(), timings.every_pass, "{timings:?}");
    let exclusive: std::time::Duration = timings.passes.iter().map(|pass| pass.exclusive).sum();
    assert!(timings.passes.iter().all(|pass| pass.exclusive <= pass.duration), "{timings:?}");
    // Up to a nanosecond per pass of rounding from ticks.
    let rounding = std::time::Duration::from_nanos(timings.passes.len() as u64);
    assert!(
        exclusive <= timings.span() + rounding,
        "exclusive times add up to the span at most: {timings:?}"
    );
    executor.set_gpu_timing(&gpu, false);
    assert!(executor.gpu_timings().is_none(), "turning timing off drops it");
}
