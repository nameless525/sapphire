//! Culling for every view (M5 D3): culling by the frustum alone draws what no culling draws, in
//! every byte of colour and of the prepasses' depth, normals, outlines and character IDs, on every
//! shipped tier's settings, over the scene D2's runs draw ([`super::runs`]); the tiers'
//! distance and size tests leave that scene as it is; and a skinned character panned out of view
//! and back, posed all the while, draws and moves exactly as if it had never left (skinning poses
//! only what some view draws). Run alone with
//! `cargo test -p sapphire_render --test gpu render::culling:: -- --ignored`.

use sapphire_render::quality::TierTable;
use sapphire_render::{CullingMode, DrawnBy, RenderStats};

use super::runs::{Side, scene_meshes};
use super::sequences::character;
use super::*;

const SIZE: (u32, u32) = (160, 96);

#[test]
#[ignore = "needs a GPU"]
fn culling_by_the_frustum_alone_draws_what_no_culling_draws_on_every_tier() {
    let meshes = scene_meshes();
    let names = ["target", "character depth", "character IDs", "depth", "normals", "outline mask"];
    for tier in TierTable::shipped().tiers() {
        let gpu = gpu();
        let texture = target(&gpu, FORMAT, SIZE);
        let view = texture.create_view(&Default::default());
        let modes = [CullingMode::On, CullingMode::FrustumOnly, CullingMode::Off];
        let mut sides = modes.map(|view_culling| {
            let settings = RenderSettings { view_culling, ..tier.render_settings() };
            Side::new(&gpu, settings, settings.draw_runs, &meshes)
        });
        for frame in 0..5 {
            let [on, frustum, off] = &mut sides;
            let (on_images, on_stats) = on.frame(&gpu, (&texture, &view), frame);
            let (images, stats) = frustum.frame(&gpu, (&texture, &view), frame);
            let (off_images, off_stats) = off.frame(&gpu, (&texture, &view), frame);
            for (((name, frustum), off), on) in names.iter().zip(&images).zip(&off_images).zip(&on_images) {
                let (_, differing) = difference(frustum, off, 0);
                assert_eq!(differing, 0, "{} frame {frame}: {name} differs in {differing} texels", tier.id);
                // The scene is near and large enough that no tier's distance or size leaves
                // anything out: culling on draws it the same.
                let (_, differing) = difference(on, off, 0);
                assert_eq!(
                    differing, 0,
                    "{} frame {frame}: {name} differs on, in {differing} texels",
                    tier.id
                );
            }
            let culled = |stats: &RenderStats| {
                (stats.view_culling.culled_by_distance, stats.view_culling.culled_by_size)
            };
            assert_eq!(culled(&on_stats), (0, 0), "{} frame {frame}", tier.id);
            assert!(stats.view_culling.culled > 0, "{} frame {frame}: the frustum culls", tier.id);
            assert_eq!(off_stats.view_culling.tested, 0, "{} frame {frame}: off tests nothing", tier.id);
            // The same pixels from fewer draws: off draws what the frustum leaves out too.
            let draws = |stats: &RenderStats| stats.passes.total().mesh_instances();
            assert!(draws(&stats) < draws(&off_stats), "{} frame {frame}", tier.id);
            assert_eq!(draws(&on_stats), draws(&stats), "{} frame {frame}", tier.id);
        }
    }
}

/// The skinned character's instance.
const PANNED: u64 = 7;

/// One frame of [`pan`]: its target, depth and motion, its stats, and who drew the character.
type Panned = (Vec<Vec<[u8; 4]>>, RenderStats, Option<DrawnBy>);

/// One of the pan's runs: its label, whether shadows draw, and the frames it's posed on.
type Run = (&'static str, bool, fn(u32) -> bool);

/// Draws a skinned character at the origin through `frames` frames of a camera at (0, 0, 5)
/// looking at it (frames 0 to 2), then far to its right, leaving it out of view (frames 3 to 6),
/// then back at it, under temporal AA with `view_culling`. The character is posed on the frames
/// `posed` says, a new pose each time. Returns each frame's target, depth and motion, its stats,
/// and who drew the character.
fn pan(
    gpu: &GpuContext,
    view_culling: CullingMode,
    shadows: bool,
    posed: impl Fn(u32) -> bool,
) -> Vec<Panned> {
    let mut settings = RenderSettings {
        msaa: Msaa::Off,
        temporal: true,
        scene_velocity: true,
        view_culling,
        ..RenderSettings::default()
    };
    if shadows {
        settings.shadows = Some(sapphire_render::ShadowSettings::default());
    }
    let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    frames.renderer.set_render_scale(1.0);
    frames.sun = DirectionalLight {
        direction: Vec3::new(-0.3, -0.8, -0.5).normalize(),
        color: Vec3::ONE,
        intensity: 1.0,
    };
    let mesh = upload(&mut frames.renderer, &skinned_sphere(white(2.0)));
    let ground = upload(&mut frames.renderer, &sphere(colored([0.2, 0.3, 0.7])));
    let texture = target(gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let reads = [Readback::new(gpu, SIZE), Readback::new(gpu, SIZE)];
    let mut out = Vec::new();
    for frame in 0..12 {
        let away = (3..7).contains(&frame);
        let looking_at =
            if away { Vec3::new(40.0, 0.0, -10.0) } else { Vec3::new(0.1 * frame as f32 * 0.1, 0.0, 0.0) };
        let camera = CameraData::look_at(
            Vec3::new(0.0, 0.0, 5.0),
            looking_at,
            Vec3::Y,
            0.8,
            SIZE.0 as f32 / SIZE.1 as f32,
            0.05,
        );
        let packet = frames.packets.begin();
        packet.views.push(ViewData::new(camera));
        packet.sun = frames.sun;
        packet.sky = frames.sky;
        packet.ambient = frames.ambient;
        packet.post = frames.post;
        if frame == 0 {
            packet.instances.changed.push(instance(PANNED, mesh, Mat4::IDENTITY));
            // Something else in each view, so neither is empty.
            packet.instances.changed.push(instance(
                1,
                ground,
                Mat4::from_translation(Vec3::new(0.0, -1.6, -1.0)),
            ));
            packet.instances.changed.push(instance(
                2,
                ground,
                Mat4::from_translation(Vec3::new(40.0, 0.0, -10.0)),
            ));
        }
        if posed(frame) {
            let pose = bend(0.15 * frame as f32, 0.03 * frame as f32);
            packet.push_skinned(InstanceId(PANNED), pose.iter().copied(), [0.1 * frame as f32]);
        }
        packet.characters.push(character(PANNED, Vec3::new(0.0, -1.0, 0.0), 1.0));

        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        frames.renderer.prepare(gpu, frames.packets.current());
        let mut graph = FrameGraph::new();
        let target_id = graph.import_texture("target", &view);
        let targets = frames.renderer.record(&mut graph, target_id).expect("a view and a target");
        let picks = [targets.depth, targets.velocity.expect("temporal AA draws motion")];
        let reads_ref = &reads;
        let mut pass = graph.add_pass("read prepasses").side_effect();
        for texture in picks {
            pass = pass.read(texture);
        }
        pass.record(move |ctx| {
            for (texture, readback) in picks.into_iter().zip(reads_ref) {
                readback.copy(ctx.encoder, ctx.texture(texture));
            }
        });
        frames.graph.execute(gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "no validation error");
        let mut images = vec![read_back(gpu, &texture)];
        images.extend(reads.iter().map(|readback| readback.texels(gpu)));
        out.push((images, frames.renderer.stats(), frames.renderer.drawn_by(InstanceId(PANNED))));
    }
    out
}

#[test]
#[ignore = "needs a GPU"]
fn a_character_panned_out_of_view_and_back_moves_as_if_never_culled() {
    let gpu = gpu();
    let names = ["target", "depth", "motion"];
    // Posed every frame; posed until it has been away a frame, so it comes back without a new
    // pose; as the stage's animation levels pose a fighter (M5 C1): every other frame, held while
    // out of view and posed again the frame it's back; and with shadows, which may draw it while
    // it's out of view.
    let runs: [Run; 4] = [
        ("posed throughout", false, |_| true),
        ("posed until away", false, |frame| frame < 4),
        ("posed by levels, held while away", false, |frame| {
            !(3..7).contains(&frame) && (frame.is_multiple_of(2) || frame == 7)
        }),
        ("shadowed", true, |_| true),
    ];
    for (label, shadows, posed) in runs {
        let culled = pan(&gpu, CullingMode::On, shadows, posed);
        let whole = pan(&gpu, CullingMode::Off, shadows, posed);
        for (frame, ((images, stats, drawn), (whole_images, whole_stats, _))) in
            culled.iter().zip(&whole).enumerate()
        {
            for ((name, culled), whole) in names.iter().zip(images).zip(whole_images) {
                let (_, differing) = difference(culled, whole, 0);
                assert_eq!(differing, 0, "{label}, frame {frame}: {name} differs in {differing} texels");
            }
            let away = (3..7).contains(&frame);
            let drawn = drawn.expect("a live instance");
            assert_eq!(drawn.view, !away, "{label}, frame {frame}: {drawn:?}");
            if !shadows {
                assert_eq!(drawn.any(), !away, "{label}, frame {frame}: {drawn:?}");
                // Out of view, it isn't skinned, though off skins it.
                if away && posed(frame as u32) {
                    assert_eq!(stats.skinning.skinned, 0, "{label}, frame {frame}");
                    assert_eq!(whole_stats.skinning.skinned, 1, "{label}, frame {frame}");
                }
            }
        }
        // It moved while away: the frame it's back in view draws it moving from where it was.
        let motion =
            |frame: usize| culled[frame].0[2].iter().filter(|texel| rg16f(**texel) != [0.0, 0.0]).count();
        assert!(motion(7) > 0, "{label}: the frame back in view has motion");
    }
}
