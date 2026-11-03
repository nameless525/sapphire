//! Frames that never run (own review RC-2, RC-4, RC-5, RC-6): a device lost after `prepare`, a
//! graph dropped or rejected, a target of no size, a mesh replaced between `prepare` and `record`.
//! Nothing panics, and the next frame that runs draws as if the lost one was never recorded.

use sapphire_assets::material::{AlphaMode, FoliageMaterial};
use sapphire_render::quality::TierTable;
use sapphire_render::{
    CharacterBody, CharacterFace, CharacterInstance, CharacterLighting, HistoryReset, Wind,
};
use sapphire_rhi::TransientDesc;

use super::sequences::{card, environment_sphere};
use super::*;

const SIZE: (u32, u32) = (64, 64);

/// How a frame is lost before its passes run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Loss {
    /// Prepared and never recorded, as for a frame without a surface.
    Unrecorded,
    /// Prepared and never recorded, then a frame without a view.
    UnrecordedThenNoView,
    /// Recorded for a target of no size.
    EmptyTarget,
    /// Recorded into a graph that is dropped unexecuted.
    Dropped,
    /// Recorded into a graph the executor rejects.
    Rejected,
}

const LOSSES: [Loss; 5] =
    [Loss::Unrecorded, Loss::UnrecordedThenNoView, Loss::EmptyTarget, Loss::Dropped, Loss::Rejected];

/// A texture nothing writes, which the executor rejects a read of.
fn never_written() -> TransientDesc {
    TransientDesc {
        width: 4,
        height: 4,
        format: FORMAT,
        sample_count: 1,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
    }
}

/// Takes the frame `frames` just prepared and loses it `how`.
fn lose(frames: &mut Frames, gpu: &GpuContext, view: &wgpu::TextureView, how: Loss) {
    match how {
        Loss::Unrecorded => {}
        Loss::UnrecordedThenNoView => {
            frames.packets.begin();
            frames.renderer.prepare(gpu, frames.packets.current());
        }
        Loss::EmptyTarget => {
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
            let desc = TransientDesc { width: 0, height: 0, format: FORMAT, sample_count: 1, usage };
            let mut graph = FrameGraph::new();
            let empty = graph.create_texture("empty", desc);
            assert!(frames.renderer.record(&mut graph, empty).is_none(), "nothing to draw into");
        }
        Loss::Dropped => {
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", view);
            frames.renderer.record(&mut graph, target).expect("a view and a target");
        }
        Loss::Rejected => {
            let mut graph = FrameGraph::new();
            let target = graph.import_texture("target", view);
            frames.renderer.record(&mut graph, target).expect("a view and a target");
            let unwritten = graph.create_texture("never written", never_written());
            graph.add_pass("reads it").read(unwritten).side_effect().record(|_| {});
            assert!(frames.graph.execute(gpu, graph).is_err(), "the executor rejects the graph");
        }
    }
}

/// Prepares a frame with `frames`' light and post, as [`Frames::render`] does.
fn prepare(frames: &mut Frames, gpu: &GpuContext, camera: CameraData, cut: bool, changed: &[MeshInstance]) {
    let packet = frames.packets.begin();
    let mut view = ViewData::new(camera);
    view.cut = cut;
    packet.views.push(view);
    packet.sun = frames.sun;
    packet.sky = frames.sky;
    packet.ambient = frames.ambient;
    packet.post = frames.post;
    packet.instances.changed.extend_from_slice(changed);
    for (id, palette, weights) in frames.poses.drain(..) {
        packet.push_skinned(InstanceId(id), palette, weights);
    }
    frames.renderer.prepare(gpu, frames.packets.current());
}

fn taa() -> RenderSettings {
    RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() }
}

/// RC-2: a device lost after `prepare` read the flag panicked in the frame's uploads: `record`
/// still recorded them, and the staging belt can't map on a lost device. A lazily built pipeline
/// in `prepare` itself, or a shader build's worker, loses it at that moment.
#[test]
#[ignore = "needs a GPU"]
fn a_device_lost_after_prepare_loses_the_frame_alone() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.6);
    let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
    let plain = upload(&mut frames.renderer, &sphere(white(1.0)));
    // A normal frame first, so the belt has chunks, as any running app's has.
    assert!(draw_frame(&mut frames, &gpu, &view, camera, &[instance(0, plain, Mat4::IDENTITY)]));
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    prepare(&mut frames, &gpu, camera, false, &[]);
    gpu.device().destroy();
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    assert!(gpu.is_lost());
    let executed = frames.graph.frames();
    let mut graph = FrameGraph::new();
    let target = graph.import_texture("target", &view);
    assert!(frames.renderer.record(&mut graph, target).is_some(), "prepare hadn't seen the loss");
    frames.graph.execute(&gpu, graph).expect("a valid graph, though nothing runs");
    assert_eq!(frames.graph.frames(), executed, "nothing is recorded on a lost device");
    // The frames after see the loss, and record nothing.
    assert!(!draw_frame(&mut frames, &gpu, &view, camera, &[]));
}

/// RC-2: an invalid graph is still an error on a lost device, and none of its passes run.
#[test]
#[ignore = "needs a GPU"]
fn a_lost_device_still_rejects_an_invalid_graph() {
    let gpu = lost_gpu();
    let mut executor = GraphExecutor::new();
    let mut graph = FrameGraph::new();
    let unwritten = graph.create_texture("never written", never_written());
    graph.add_pass("reads it").read(unwritten).side_effect().record(|_| panic!("never runs"));
    assert!(executor.execute(&gpu, graph).is_err());
}

/// RC-4: a graph lost with a deformed instance's first skin left it with outputs nobody wrote and
/// nothing pending, so it drew nothing until its next pose: forever, for a held pose.
#[test]
#[ignore = "needs a GPU"]
fn a_deformed_instance_survives_a_lost_first_skin() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.6);
    let run = |loss: Option<Loss>| {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
        let skinned = upload(&mut frames.renderer, &skinned_sphere(colored([0.9, 0.1, 0.1])));
        let mut added = vec![instance(0, skinned, Mat4::IDENTITY)];
        if let Some(loss) = loss {
            prepare(&mut frames, &gpu, camera, false, &added);
            lose(&mut frames, &gpu, &view, loss);
            added.clear();
        }
        assert!(frames.render(&gpu, &view, camera, &added, &[]).is_none());
        for _ in 0..3 {
            assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        }
        read_back(&gpu, &texture)
    };
    let whole = run(None);
    let center = |pixels: &[[u8; 4]]| pixels[(SIZE.1 / 2 * SIZE.0 + SIZE.0 / 2) as usize];
    let reddish = |[r, g, _, _]: [u8; 4]| r > g + 32;
    assert!(reddish(center(&whole)), "the instance draws: {:?}", center(&whole));
    for loss in LOSSES {
        let lost = run(Some(loss));
        assert_eq!(difference(&whole, &lost, 0).1, 0, "after {loss:?}, center {:?}", center(&lost));
    }
}

/// RC-4: a lost frame's pose is the one the next frame that runs skins, and velocity compares it
/// with the pose last drawn, not the half the lost frame would have written.
#[test]
#[ignore = "needs a GPU"]
fn a_lost_pose_still_lands_and_velocity_follows_the_pose_last_drawn() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.6);
    let velocity = Readback::new(&gpu, SIZE);
    let reads: [PrepassRead<'_>; 1] = [(|targets| targets.velocity.expect("asked for"), &velocity)];
    let run = |loss: Option<Loss>| {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::Off).expect("renderer");
        let skinned = upload(&mut frames.renderer, &skinned_sphere(colored([0.9, 0.1, 0.1])));
        assert!(frames.render(&gpu, &view, camera, &[instance(0, skinned, Mat4::IDENTITY)], &[]).is_none());
        frames.poses.push((0, bend(0.2, 0.0), vec![0.0]));
        assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        frames.poses.push((0, bend(0.8, 0.2), vec![0.5]));
        if let Some(loss) = loss {
            prepare(&mut frames, &gpu, camera, false, &[]);
            lose(&mut frames, &gpu, &view, loss);
        }
        assert!(frames.render_reading(&gpu, &view, camera, &[], &[], &reads).is_none());
        (read_back(&gpu, &texture), velocity.texels(&gpu))
    };
    let whole = run(None);
    assert!(whole.1.iter().any(|&texel| rg16f(texel) != [0.0, 0.0]), "the pose moved it");
    for loss in LOSSES {
        let lost = run(Some(loss));
        assert_eq!(difference(&whole.0, &lost.0, 0).1, 0, "the color after {loss:?}");
        assert_eq!(difference(&whole.1, &lost.1, 0).1, 0, "the velocity after {loss:?}");
    }
}

/// RC-4: a lost first frame claimed a history nothing wrote, and the first frame that ran read it
/// as last frame's.
#[test]
#[ignore = "needs a GPU"]
fn a_lost_first_frame_claims_no_history() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.6);
    for loss in LOSSES {
        let mut frames = Frames::with_settings(&gpu, FORMAT, taa()).expect("renderer");
        prepare(&mut frames, &gpu, camera, false, &[]);
        lose(&mut frames, &gpu, &view, loss);
        assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        let stats = frames.renderer.stats().temporal;
        assert_eq!(stats.reset, Some(HistoryReset::Start), "after {loss:?}");
        assert_eq!((stats.resets, stats.jitter_index), (1, 0), "after {loss:?}");
    }
}

/// RC-4: a frame lost at a new size made new, empty histories, which the next frame at that size
/// can't take for last frame's.
#[test]
#[ignore = "needs a GPU"]
fn a_lost_frame_at_a_new_size_claims_no_history() {
    let gpu = gpu();
    let small = (32, 32);
    let (small_texture, large_texture) = (target(&gpu, FORMAT, small), target(&gpu, FORMAT, SIZE));
    let small_view = small_texture.create_view(&Default::default());
    let large_view = large_texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.6);
    let mut frames = Frames::with_settings(&gpu, FORMAT, taa()).expect("renderer");
    for _ in 0..3 {
        assert!(frames.render(&gpu, &small_view, camera, &[], &[]).is_none());
    }
    prepare(&mut frames, &gpu, camera, false, &[]);
    lose(&mut frames, &gpu, &large_view, Loss::Dropped);
    assert!(frames.render(&gpu, &large_view, camera, &[], &[]).is_none());
    assert!(frames.renderer.stats().temporal.reset.is_some(), "the new histories hold nothing");
}

/// RC-4: the sky cube's first fill, lost with its frame, is drawn by the next frame that runs.
#[test]
#[ignore = "needs a GPU"]
fn a_lost_first_frame_still_fills_the_sky_cube() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.6);
    for loss in LOSSES {
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
        prepare(&mut frames, &gpu, camera, false, &[]);
        lose(&mut frames, &gpu, &view, loss);
        assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        let sky = frames.renderer.stats().sky;
        assert_eq!((sky.cube_faces, sky.last_face), (6, Some(5)), "after {loss:?}");
    }
}

/// RC-5: a camera cut applies to the next frame that runs, however its own frame was lost.
#[test]
#[ignore = "needs a GPU"]
fn a_camera_cut_survives_a_lost_frame() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.6);
    for loss in LOSSES {
        let mut frames = Frames::with_settings(&gpu, FORMAT, taa()).expect("renderer");
        for _ in 0..3 {
            assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        }
        prepare(&mut frames, &gpu, camera, true, &[]);
        lose(&mut frames, &gpu, &view, loss);
        assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        assert_eq!(frames.renderer.stats().temporal.reset, Some(HistoryReset::Cut), "after {loss:?}");
        // A cut that ran is done with.
        assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        assert_eq!(frames.renderer.stats().temporal.reset, None, "the frame after {loss:?}");
    }
}

/// Two nested skinned spheres: twice [`skinned_sphere`]'s vertices.
fn larger_skinned_sphere(material: AnimeCharacterMaterial) -> Mesh {
    let (mut vertices, mut indices) = sphere_geometry(Vec3::ZERO, 1.0);
    let (inner, inner_indices) = sphere_geometry(Vec3::ZERO, 0.9);
    let first = vertices.len() as u32;
    vertices.extend(inner);
    indices.extend(inner_indices.into_iter().map(|index| index + first));
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", cooked(&material))];
    with_skin(Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16))
}

/// RC-6: replacing a posed instance's mesh between `prepare` and `record` is against its docs, and
/// tolerated: `record` panicked on a mesh that no longer deforms, and skinned a larger mesh past
/// the instance's outputs, over its neighbour's.
#[test]
#[ignore = "needs a GPU"]
fn a_mesh_replaced_between_prepare_and_record_waits_for_the_next_frame() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let camera = framing_camera(38.0, SIZE, 0.25);
    let replacements = [
        ("static", sphere(colored([0.1, 0.9, 0.1]))),
        ("larger", larger_skinned_sphere(colored([0.1, 0.9, 0.1]))),
    ];
    for (what, replacement) in &replacements {
        let cooked = replacement.to_cooked().expect("cook");
        let mut frames = Frames::new(&gpu, FORMAT, Msaa::default()).expect("renderer");
        let first = upload(&mut frames.renderer, &skinned_sphere(colored([0.9, 0.1, 0.1])));
        let second = upload(&mut frames.renderer, &skinned_sphere(colored([0.1, 0.1, 0.9])));
        let added = [
            instance(0, first, Mat4::from_translation(Vec3::new(-1.5, 0.0, 0.0))),
            instance(1, second, Mat4::from_translation(Vec3::new(1.5, 0.0, 0.0))),
        ];
        assert!(frames.render(&gpu, &view, camera, &added, &[]).is_none());
        // The neighbour's half of its positions that the larger job's normals would run over: the
        // one a first pose after its bind pose draws from.
        frames.poses.push((1, bend(0.3, 0.0), vec![0.0]));
        assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        let right = |pixels: Vec<[u8; 4]>| {
            let half = SIZE.0 as usize / 2;
            pixels.chunks(SIZE.0 as usize).flat_map(|row| row[half..].to_vec()).collect::<Vec<_>>()
        };
        let neighbour = right(read_back(&gpu, &texture));

        frames.poses.push((0, bend(0.2, 0.0), vec![0.0]));
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        prepare(&mut frames, &gpu, camera, false, &[]);
        let mesh = Mesh::read(&cooked).expect("a valid mesh");
        frames.renderer.update_mesh(first, mesh, |_| None).expect("the mesh fits");
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        frames.renderer.record(&mut graph, target).expect("a view and a target");
        frames.graph.execute(&gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "{what}");
        assert_eq!(
            difference(&right(read_back(&gpu, &texture)), &neighbour, 0).1,
            0,
            "{what}: the neighbour keeps its pose"
        );

        // The next frame starts the instance over on the new mesh, which draws.
        assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        let green = |&[r, g, b, _]: &[u8; 4]| g > r + 16 && g > b + 16;
        assert!(read_back(&gpu, &texture).iter().any(green), "{what}: the new mesh draws");
    }
}

/// Prepares frame `frame` of the no-trace scene, with what carries state between frames: a pose,
/// a moving camera, a character and wind.
fn prepare_scene(frames: &mut Frames, gpu: &GpuContext, frame: u32, changed: &[MeshInstance]) {
    let t = frame as f32;
    let packet = frames.packets.begin();
    let mut camera = framing_camera(38.0, SIZE, 0.3);
    camera.view *= Mat4::from_translation(Vec3::new(0.01 * t, 0.0, 0.0));
    packet.views.push(ViewData::new(camera));
    packet.sun = frames.sun;
    packet.sky = frames.sky;
    packet.ambient = frames.ambient;
    packet.post = frames.post;
    packet.time_seconds = f64::from(frame) / 60.0;
    packet.wind = Wind { direction: Vec3::new(1.0, 0.0, 0.5), strength: 1.0, gustiness: 0.3 };
    packet.instances.changed.extend_from_slice(changed);
    packet.push_skinned(InstanceId(0), bend(0.05 * t, 0.0), [0.1 * t]);
    let key = DirectionalLight { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 1.0 };
    packet.characters.push(CharacterInstance {
        id: InstanceId(0),
        lighting: CharacterLighting::unstyled(key),
        face: CharacterFace::default(),
        body: CharacterBody::capsule(Vec3::new(0.0, -0.5, 0.0), Vec3::new(0.0, 0.5, 0.0), 0.3),
        opacity: 1.0,
    });
    frames.renderer.prepare(gpu, frames.packets.current());
}

/// RC-4: on every shipped tier, a frame whose graph is lost in the middle of a run leaves no trace
/// in the frames after: temporal AA's history and jitter, skinning, the camera velocity compares
/// with, the sky cube's schedule and the ambient uniforms carry on from the last frame that ran.
/// (The wind's velocity has a test of its own, `foliage.rs`: the pixels here don't show it.)
#[test]
#[ignore = "needs a GPU"]
fn lost_graphs_leave_no_trace_on_any_tier() {
    let table = TierTable::shipped();
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    for tier in table.tiers() {
        let run = |loss: Option<(u32, Loss)>| {
            let mut frames = Frames::with_settings(&gpu, FORMAT, tier.render_settings()).expect("renderer");
            frames.post = PostParams::default();
            let skinned = upload(&mut frames.renderer, &skinned_sphere(colored([0.9, 0.1, 0.1])));
            let world = upload(&mut frames.renderer, &environment_sphere(AlphaMode::Opaque));
            let foliage = FoliageMaterial { alpha_cutoff: 0.0, ..FoliageMaterial::default() };
            let tree = upload(&mut frames.renderer, &card(&foliage, true));
            let added = [
                instance(0, skinned, Mat4::IDENTITY),
                instance(1, world, Mat4::from_translation(Vec3::new(1.2, 0.0, -1.0))),
                instance(2, tree, Mat4::from_translation(Vec3::new(-1.2, -1.0, -1.0))),
            ];
            for frame in 0..8 {
                let changed: &[MeshInstance] = if frame == 0 { &added } else { &[] };
                prepare_scene(&mut frames, &gpu, frame, changed);
                if let Some((_, how)) = loss.filter(|&(at, _)| at == frame) {
                    lose(&mut frames, &gpu, &view, how);
                    // Its deltas were applied: the same frame again needs none.
                    prepare_scene(&mut frames, &gpu, frame, &[]);
                }
                let mut graph = FrameGraph::new();
                let target = graph.import_texture("target", &view);
                frames.renderer.record(&mut graph, target).expect("a view and a target");
                frames.graph.execute(&gpu, graph).expect("a valid graph");
            }
            (read_back(&gpu, &texture), frames.renderer.stats())
        };
        let (whole, stats) = run(None);
        for (at, how) in [(0, Loss::Dropped), (3, Loss::Rejected)] {
            let (lost, lost_stats) = run(Some((at, how)));
            let what = format!("{} after {how:?} at frame {at}", tier.id);
            assert_eq!(difference(&whole, &lost, 0).1, 0, "{what}");
            assert_eq!(lost_stats.sky, stats.sky, "{what}");
            assert_eq!(lost_stats.temporal, stats.temporal, "{what}");
            assert_eq!(lost_stats.ambient, stats.ambient, "{what}");
        }
    }
}
