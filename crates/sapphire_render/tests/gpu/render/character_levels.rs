//! Levels of detail for characters (M5 D5, the renderer's `levels` module): a character the stage
//! lets draw coarser draws its level's triangles and skins only that level's prefix, morph and all,
//! the same picture as a mesh that is only that level; one it keeps whole draws level 0 whatever
//! the mode; with levels off a character draws what it drew before characters chose levels, byte
//! for byte, on every shipped tier; and its depth and motion across a switch either way are those
//! of a character that never switched. Run alone with
//! `cargo test -p sapphire_render --test gpu render::character_levels:: -- --ignored --test-threads=1`.

use sapphire_assets::mesh::Lod;
use sapphire_render::quality::TierTable;
use sapphire_render::{
    CharacterBody, CharacterDetail, CharacterFace, CharacterInstance, CharacterLighting, LevelSettings,
    LevelsMode, MaterialDrive, RenderStats, ShadowSettings,
};

use super::*;

const SIZE: (u32, u32) = (160, 96);
/// The sphere's rings above its equator and the vertices they hold: level 1's prefix.
const CAP_RINGS: u32 = 24;
const CAP_VERTICES: u32 = (CAP_RINGS + 1) * 65;
const CAP_INDICES: u32 = CAP_RINGS * 64 * 6;

/// A skinned sphere as [`skinned_sphere`]'s with a second level: the upper half's triangles, over the prefix of the
/// vertices its rings hold, at a 2 mm error.
fn sphere_with_levels() -> Mesh {
    // f32 positions, so the cap's own bounds quantize nothing differently.
    let mut mesh = with_skin(sphere_at(Vec3::ZERO, 1.0, white(3.0), PositionFormat::Float32));
    let first_index = mesh.indices.len() as u32;
    let cap: Vec<u32> = mesh.indices[..CAP_INDICES as usize].to_vec();
    mesh.indices.extend(cap);
    let submeshes = vec![Submesh { first_index, index_count: CAP_INDICES, material: 0 }];
    mesh.lods.push(Lod { vertex_count: CAP_VERTICES, error: 0.002, submeshes });
    mesh
}

/// The upper half alone, as a mesh of one level: what [`sphere_with_levels`]'s level 1 draws.
fn cap() -> Mesh {
    let (vertices, indices) = sphere_geometry(Vec3::ZERO, 1.0);
    let indices = indices[..CAP_INDICES as usize].to_vec();
    let submeshes = vec![Submesh { first_index: 0, index_count: CAP_INDICES, material: 0 }];
    let slots = vec![MaterialSlot::embedded("sphere", cooked(&white(3.0)))];
    let vertices = &vertices[..CAP_VERTICES as usize];
    let mesh = Mesh::from_vertices(vertices, indices, submeshes, slots, PositionFormat::Float32);
    with_skin(mesh)
}

fn with_levels(mode: LevelsMode, settings: RenderSettings) -> RenderSettings {
    RenderSettings { levels: LevelSettings { mode, ..settings.levels }, ..settings }
}

/// Temporal AA with its motion, no MSAA.
fn moving() -> RenderSettings {
    RenderSettings { msaa: Msaa::Off, temporal: true, scene_velocity: true, ..RenderSettings::default() }
}

/// One frame of [`run`]: its target, depth and motion (where temporal AA draws it), and its stats.
type Frame = (Vec<Vec<[u8; 4]>>, RenderStats);

/// Draws `mesh` as a character with the detail `detail(frame)` 5 m in front of the camera, which
/// holds still, through `frames` frames with `settings`, posed (a new pose each time) on the
/// frames `posed` says.
fn run(
    gpu: &GpuContext,
    settings: RenderSettings,
    mesh: &Mesh,
    frames_count: u32,
    detail: impl Fn(u32) -> CharacterDetail,
    posed: impl Fn(u32) -> bool,
) -> Vec<Frame> {
    run_lit(gpu, settings, mesh, frames_count, detail, posed, &[])
}

/// [`run`], with `lights` in every frame.
fn run_lit(
    gpu: &GpuContext,
    settings: RenderSettings,
    mesh: &Mesh,
    frames_count: u32,
    detail: impl Fn(u32) -> CharacterDetail,
    posed: impl Fn(u32) -> bool,
    lights: &[LocalLight],
) -> Vec<Frame> {
    let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    frames.renderer.set_render_scale(1.0);
    // The depth read below reads the scene prepass, which a tier may otherwise leave unbuilt.
    frames.renderer.keep_scene_prepass();
    frames.sun = DirectionalLight {
        direction: Vec3::new(-0.3, -0.8, -0.5).normalize(),
        color: Vec3::ONE,
        intensity: 1.0,
    };
    let mesh = upload(&mut frames.renderer, mesh);
    let texture = target(gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let reads = [Readback::new(gpu, SIZE), Readback::new(gpu, SIZE)];
    let camera = CameraData::look_at(
        Vec3::new(0.0, 0.3, 5.0),
        Vec3::ZERO,
        Vec3::Y,
        0.8,
        SIZE.0 as f32 / SIZE.1 as f32,
        0.05,
    );
    let key = DirectionalLight { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 1.0 };
    let mut out = Vec::new();
    for frame in 0..frames_count {
        let packet = frames.packets.begin();
        packet.views.push(ViewData::new(camera));
        packet.sun = frames.sun;
        packet.sky = frames.sky;
        packet.ambient = frames.ambient;
        packet.post = frames.post;
        packet.lights.extend_from_slice(lights);
        if frame == 0 {
            packet.instances.changed.push(instance(1, mesh, Mat4::IDENTITY));
        }
        if posed(frame) {
            let pose = bend(0.15 * frame as f32, 0.03 * frame as f32);
            packet.push_skinned(InstanceId(1), pose.iter().copied(), [0.1 * frame as f32]);
        }
        let base = Vec3::new(0.0, -1.0, 0.0);
        packet.characters.push(CharacterInstance {
            id: InstanceId(1),
            lighting: CharacterLighting::unstyled(key),
            face: CharacterFace::default(),
            body: CharacterBody::capsule(base, base + Vec3::Y * 2.0, 0.6).with_detail(detail(frame)),
            opacity: 1.0,
            drive: MaterialDrive::NONE,
        });
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        frames.renderer.prepare(gpu, frames.packets.current());
        let mut graph = FrameGraph::new();
        let target_id = graph.import_texture("target", &view);
        let targets = frames.renderer.record(&mut graph, target_id).expect("a view and a target");
        let picks: Vec<TextureId> = std::iter::once(targets.depth).chain(targets.velocity).collect();
        let count = picks.len();
        let reads_ref = &reads;
        let mut pass = graph.add_pass("read prepasses").side_effect();
        for &texture in &picks {
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
        images.extend(reads[..count].iter().map(|readback| readback.texels(gpu)));
        out.push((images, frames.renderer.stats()));
    }
    out
}

#[test]
#[ignore = "needs a GPU"]
fn a_character_held_at_each_level_draws_its_triangles_and_skins_its_prefix() {
    let gpu = gpu();
    let leveled = sphere_with_levels();
    let full = leveled.streams.len() as u32;
    let by_error = |_| CharacterDetail::ByError;
    let posed = |_| true;
    let at = |mode, mesh: &Mesh, detail: fn(u32) -> CharacterDetail| {
        run(&gpu, with_levels(mode, moving()), mesh, 3, detail, posed).pop().expect("frames")
    };
    let (coarse, coarse_stats) = at(LevelsMode::Fixed(1), &leveled, by_error);
    let (whole, whole_stats) = at(LevelsMode::Fixed(0), &leveled, by_error);
    let (kept, kept_stats) = at(LevelsMode::Fixed(1), &leveled, |_| CharacterDetail::Whole);
    let (alone, _) = at(LevelsMode::Fixed(1), &cap(), by_error);
    // Level 1: the cap's triangles, from the cap's vertices alone, morphed and skinned: the
    // picture, depth and motion of a mesh that is only the cap, byte for byte.
    let names = ["target", "depth", "motion"];
    for ((name, coarse), alone) in names.iter().zip(&coarse).zip(&alone) {
        assert!(coarse == alone, "{name}: level 1 draws what the cap alone draws");
    }
    assert!(coarse[0] != whole[0], "level 1 shows");
    let main = coarse_stats.passes.main;
    assert!(main.levels[1] > 0, "the sphere and its hull at level 1: {main:?}");
    assert_eq!(coarse_stats.skinning.vertices, CAP_VERTICES, "the cap's prefix alone is skinned");
    assert_eq!(whole_stats.skinning.vertices, full);
    assert_eq!(whole_stats.passes.main.levels[1], 0, "{:?}", whole_stats.passes.main);
    // Its hull draws at its body's level: the cap's own hull, in the pictures above.
    assert!(main.outline_draws > 0, "{main:?}");
    // A character the stage keeps whole draws level 0 whatever the mode.
    assert!(kept == whole, "whole, as level 0");
    assert_eq!(kept_stats.passes.main.levels[1], 0);
    assert_eq!(kept_stats.skinning.vertices, full);
}

#[test]
#[ignore = "needs a GPU"]
fn levels_off_draw_a_character_as_before_on_every_tier() {
    let gpu = gpu();
    let leveled = sphere_with_levels();
    let mut plain = leveled.clone();
    plain.lods.truncate(1);
    plain.indices.truncate(plain.lods[0].submeshes[0].index_count as usize);
    let by_error = |_| CharacterDetail::ByError;
    let table = TierTable::shipped();
    let tiers = table.tiers().iter().map(|tier| (format!("{:?}", tier.id), tier.render_settings()));
    for (name, settings) in std::iter::once(("plain".to_owned(), moving())).chain(tiers) {
        let at = |mode, mesh: &Mesh| run(&gpu, with_levels(mode, settings), mesh, 3, by_error, |_| true);
        let off = at(LevelsMode::Off, &leveled);
        let base = at(LevelsMode::On, &plain);
        let on = at(LevelsMode::On, &leveled);
        for (frame, ((off, base), on)) in off.iter().zip(&base).zip(&on).enumerate() {
            assert!(off.0 == base.0, "{name} frame {frame}: levels off draw the base's pictures");
            assert_eq!(off.1.passes.main, base.1.passes.main, "{name} frame {frame}");
            assert_eq!(off.1.skinning, base.1.skinning, "{name} frame {frame}");
            // On, 5 m away, 2 mm is well inside a pixel: level 1 in the main view. Skinning skins
            // the finest level any view drew: the cap's prefix, unless another view (a shadow
            // view's texel, the character map's close around it) wanted level 0.
            let passes = &on.1.passes;
            assert!(passes.main.levels[1] > 0, "{name} frame {frame}: {:?}", passes.main);
            let others = passes.cascades.iter().chain(&passes.insets);
            let whole_elsewhere = others
                .chain([&passes.character_maps, &passes.character_prepass])
                .any(|pass| pass.levels[0] > 0);
            let skinned = if whole_elsewhere { leveled.streams.len() as u32 } else { CAP_VERTICES };
            assert_eq!(on.1.skinning.vertices, skinned, "{name} frame {frame}: {passes:?}");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn depth_and_motion_across_a_switch_are_those_of_no_switch() {
    let gpu = gpu();
    let leveled = sphere_with_levels();
    let settings = with_levels(LevelsMode::Fixed(1), moving());
    use CharacterDetail::{ByError, Whole};
    // Coarser for three frames, whole for two (the prefix grows), coarser for two (it shrinks),
    // whole again for two; against whole and coarser throughout.
    let switching = |frame: u32| if matches!(frame, 3 | 4 | 7 | 8) { Whole } else { ByError };
    // Posed every frame; and every other frame, so a prefix grows on a frame with no new pose too.
    type Posed = fn(u32) -> bool;
    let runs: [(&str, Posed); 2] =
        [("posed every frame", |_| true), ("posed every other frame", |frame| frame.is_multiple_of(2))];
    for (label, posed) in runs {
        let switched = run(&gpu, settings, &leveled, 9, switching, posed);
        let whole = run(&gpu, settings, &leveled, 9, |_| Whole, posed);
        let coarse = run(&gpu, settings, &leveled, 9, |_| ByError, posed);
        for (frame, (images, stats)) in switched.iter().enumerate() {
            let frame = frame as u32;
            let (same, level) = if switching(frame) == Whole { (&whole, 0) } else { (&coarse, 1) };
            let (other, _) = &same[frame as usize];
            for (name, index) in [("depth", 1), ("motion", 2)] {
                let (_, differing) = difference(&images[index], &other[index], 0);
                assert_eq!(differing, 0, "{label}, frame {frame}: {name} differs in {differing} texels");
            }
            let coarser = stats.passes.main.levels[1] > 0;
            assert_eq!(coarser, level == 1, "{label}, frame {frame}: {:?}", stats.passes.main);
        }
        // It moves: the frames posed after the first draw motion.
        let motion =
            |frame: usize| switched[frame].0[2].iter().filter(|texel| rg16f(**texel) != [0.0, 0.0]).count();
        assert!(motion(4) > 0, "{label}: the frame the prefix grows on has motion");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_spot_tile_draws_a_coarser_character_at_its_texel() {
    let gpu = gpu();
    let leveled = sphere_with_levels();
    let full = leveled.streams.len() as u32;
    // A flagged spot above and beside the camera, its cone round the character: a tile it casts into.
    let lamp_at = Vec3::new(2.0, 4.0, 4.0);
    let lamp = LocalLight {
        position: lamp_at,
        color: Vec3::ONE,
        intensity: 1.0,
        range: 20.0,
        kind: LightKind::Spot {
            direction: -lamp_at.normalize(),
            inner_angle: 25f32.to_radians(),
            outer_angle: 30f32.to_radians(),
            shadow: true,
        },
    };
    let shadows = ShadowSettings { spots: 2, ..ShadowSettings::default() };
    let settings = with_levels(LevelsMode::Fixed(1), RenderSettings { shadows: Some(shadows), ..moving() });
    let last = |detail: fn(u32) -> CharacterDetail| {
        run_lit(&gpu, settings, &leveled, 3, detail, |_| true, &[lamp]).pop().expect("frames").1
    };
    // Held at level 1, it casts into the tile at level 1 too, so only level 1's prefix is skinned.
    let coarse = last(|_| CharacterDetail::ByError);
    let spots = coarse.passes.spots;
    assert!(spots.levels[1] > 0, "the tile draws it at level 1: {spots:?}");
    assert_eq!(spots.levels[0], 0, "and not at level 0: {spots:?}");
    assert_eq!(coarse.skinning.coarser, 1, "{:?}", coarse.skinning);
    assert_eq!(coarse.skinning.vertices, CAP_VERTICES, "the cap's prefix alone is skinned");
    // One the stage keeps whole casts whole.
    let whole = last(|_| CharacterDetail::Whole);
    let spots = whole.passes.spots;
    assert!(spots.levels[0] > 0 && spots.levels[1] == 0, "the tile draws it whole: {spots:?}");
    assert_eq!(whole.skinning.vertices, full);
}
