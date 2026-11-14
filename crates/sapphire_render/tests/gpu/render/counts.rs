//! What a frame submits, pass by pass (M5 B4, `RenderStats::passes`): a scene of known meshes reads back
//! exactly, in every pass that ran, and zero in every pass that didn't. Run alone with
//! `cargo test -p sapphire_render --test gpu render::counts:: -- --ignored`.

use sapphire_assets::material::{EnvironmentMaterial, MaterialDocument};
use sapphire_render::{FrameCounts, MESH_LEVELS, PassCounts, RenderStats, ShadowSettings};

use super::*;

const SIZE: (u32, u32) = (96, 64);
/// The UV sphere's triangles: 48 rings of 64 segments, two triangles a quad.
const SPHERE_TRIANGLES: u64 = 48 * 64 * 2;
/// The environment squares in front of the camera, and those behind it.
const IN_VIEW: u32 = 3;
const BEHIND: u32 = 2;

/// A square of 0.2 m facing +z, standing on the origin: two triangles.
fn square() -> Mesh {
    let corner = |s: f32, t: f32| Vertex {
        position: [s * 0.1, (t + 1.0) * 0.1, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [(s + 1.0) * 0.5, (1.0 - t) * 0.5],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("square", cooked(&AnimeCharacterMaterial::default()))];
    Mesh::from_vertices(&vertices, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Unorm16)
}

/// Three opaque environment squares in front of a camera on +z, two behind it, and a skinned sphere, an
/// anime character surface with an outline hull, posed on the first frame. The renderer's frames.
fn scene(gpu: &GpuContext, settings: RenderSettings) -> (Frames, Vec<MeshInstance>, CameraData) {
    let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    let environment = EnvironmentMaterial::default();
    let swizzles = vec![Swizzle::IDENTITY; environment.textures().len()];
    let material = Material::from_document(&environment, &swizzles).expect("a valid material");
    let bytes = material.to_cooked().expect("cook");
    let material =
        frames.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload");
    let bytes = square().to_cooked().expect("cook");
    let square = frames.renderer.upload_mesh(Mesh::read(&bytes).expect("read"), &[material]).expect("upload");
    let character = upload(&mut frames.renderer, &skinned_sphere(flat([0.9, 0.3, 0.2], 2.0)));
    let mut instances: Vec<MeshInstance> = (0..IN_VIEW)
        .map(|n| Mat4::from_translation(Vec3::new(-0.6 + 0.3 * n as f32, 0.0, 0.0)))
        .chain((0..BEHIND).map(|n| Mat4::from_translation(Vec3::new(0.3 * n as f32, 0.0, 6.0))))
        .enumerate()
        .map(|(id, transform)| instance(id as u64, square, transform))
        .collect();
    let sphere =
        Mat4::from_scale_rotation_translation(Vec3::splat(0.3), Quat::IDENTITY, Vec3::new(0.7, 0.3, 0.0));
    instances.push(instance(100, character, sphere));
    frames.poses.push((100, bend(0.3, 0.0), vec![]));
    let camera =
        CameraData::look_at(Vec3::new(0.0, 0.2, 3.0), Vec3::new(0.0, 0.2, 0.0), Vec3::Y, 0.9, 1.5, 0.05);
    (frames, instances, camera)
}

/// What a pass of the scene's draws counts: the five squares' surfaces looked at, the three in view drawn
/// and two culled by the frustum where the pass culls by it, and the sphere's surface and hull.
fn scene_pass(frustum: bool) -> PassCounts {
    let squares = if frustum { IN_VIEW } else { IN_VIEW + BEHIND };
    PassCounts {
        draws: squares + 2,
        instances: squares + 2,
        triangles: 2 * u64::from(squares) + 2 * SPHERE_TRIANGLES,
        outline_draws: 1,
        considered: IN_VIEW + BEHIND + 2,
        culled_by_frustum: if frustum { BEHIND } else { 0 },
        levels: [squares + 2, 0, 0, 0],
        ..PassCounts::default()
    }
}

/// Every pass's mesh draws are in the levels, LOD0 alone until levels are selected, and the frame's are the
/// passes' summed.
fn assert_levels_add_up(passes: &FrameCounts) {
    for (name, pass) in passes.passes() {
        assert_eq!(pass.levels[1..], [0; MESH_LEVELS - 1], "{name}: nothing selects a coarser level yet");
        assert!(pass.mesh_draws() <= pass.draws, "{name}: {pass:?}");
        assert_eq!(pass.considered, pass.mesh_draws() + pass.culled(), "{name}: drawn or culled: {pass:?}");
    }
    let total = passes.total();
    let mesh_draws: u32 = passes.passes().iter().map(|(_, pass)| pass.mesh_draws()).sum();
    assert_eq!(total.mesh_draws(), mesh_draws);
    assert_eq!(total.levels.iter().sum::<u32>(), mesh_draws);
}

/// The passes that draw a fullscreen triangle or a few each draw one triangle an instance.
fn assert_fullscreen(name: &str, pass: &PassCounts) {
    assert!(pass.draws > 0, "{name} ran");
    assert_eq!((pass.instances, pass.triangles), (pass.draws, u64::from(pass.draws)), "{name}: {pass:?}");
    assert_eq!((pass.considered, pass.mesh_draws()), (0, 0), "{name}: no mesh draws");
}

#[test]
#[ignore = "needs a GPU"]
fn a_known_scene_counts_exactly_in_each_pass() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let (mut frames, instances, camera) = scene(&gpu, RenderSettings::default());
    assert!(frames.render(&gpu, &view, camera, &instances, &[]).is_none());
    let stats: RenderStats = frames.renderer.stats();
    let passes = stats.passes;

    // The main pass: the scene's draws, culled by the frustum, and the sky's triangle.
    let sky = PassCounts { draws: 1, instances: 1, triangles: 1, ..PassCounts::default() };
    assert_eq!(passes.main, scene_pass(true).plus(&sky), "{passes:#?}");
    assert_eq!(passes.main.outline_draws, stats.outline_draws);
    assert_eq!(passes.main.culled_by_frustum, stats.view_culling.culled);
    // The character prepass looks at the sphere, which the frame doesn't list as a character.
    let unlisted = PassCounts { considered: 1, culled_by_pass: 1, ..PassCounts::default() };
    assert_eq!(passes.character_prepass, unlisted);
    // Post draws its passes; nothing reads the scene prepass, and no shadows, insets, temporal AA,
    // interface or debug views drew.
    assert_fullscreen("post", &passes.post);
    let quiet = FrameCounts {
        main: passes.main,
        character_prepass: passes.character_prepass,
        post: passes.post,
        ..FrameCounts::default()
    };
    assert_eq!(passes, quiet, "every other pass is zero");
    assert_levels_add_up(&passes);
    assert_eq!(passes.total().draws, passes.main.draws + passes.post.draws);
    // The pose moved the sphere's two joints.
    let skinning = stats.skinning;
    assert_eq!((skinning.skinned, skinning.joints, skinning.palette_bytes), (1, 2, 96));

    // A frame recorded and never executed counts nothing: recording starts the counts over, and only a
    // pass that runs adds to them.
    frames.packets.begin().views.push(ViewData::new(camera));
    frames.renderer.prepare(&gpu, frames.packets.current());
    let mut graph = FrameGraph::new();
    let output = graph.import_texture("target", &view);
    frames.renderer.record(&mut graph, output).expect("a view and a target");
    drop(graph);
    assert_eq!(frames.renderer.stats().passes, FrameCounts::default());

    // A frame whose depth is read runs the scene prepass: the same draws as the main pass's, but the sky.
    frames.renderer.keep_scene_prepass();
    let readback = Readback::new(&gpu, SIZE);
    let reads: [PrepassRead<'_>; 1] = [(|targets| targets.normals, &readback)];
    assert!(frames.render_reading(&gpu, &view, camera, &[], &[], &reads).is_none());
    let stats = frames.renderer.stats();
    assert_eq!(stats.passes.scene_prepass, scene_pass(true), "{:#?}", stats.passes);
    assert_eq!(stats.passes.main, scene_pass(true).plus(&sky));
    assert_levels_add_up(&stats.passes);
    // Unposed, the sphere moves no joints.
    assert_eq!((stats.skinning.skinned, stats.skinning.joints), (0, 0));
}

#[test]
#[ignore = "needs a GPU"]
fn each_cascade_counts_its_casters_apart() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let settings = RenderSettings { shadows: Some(ShadowSettings::default()), ..RenderSettings::default() };
    let (mut frames, instances, camera) = scene(&gpu, settings);
    frames.sun = DirectionalLight {
        direction: Vec3::new(-0.3, -0.8, -0.5).normalize(),
        color: Vec3::ONE,
        intensity: 1.0,
    };
    assert!(frames.render(&gpu, &view, camera, &instances, &[]).is_none());
    let stats = frames.renderer.stats();
    let passes = stats.passes;
    // A cascade drawn looks at every depth-writing surface, the squares behind the camera too, and draws
    // those its caster test keeps; one not drawn this frame counts nothing.
    let mut drawn = 0;
    for (index, cascade) in passes.cascades.iter().enumerate() {
        if *cascade == PassCounts::default() {
            continue;
        }
        drawn += 1;
        assert_eq!(cascade.considered, IN_VIEW + BEHIND + 1, "cascade {index}: {cascade:?}");
        assert_eq!((cascade.culled_by_frustum, cascade.culled_by_cutoff, cascade.outline_draws), (0, 0, 0));
        assert!(cascade.draws > 0, "cascade {index} draws the sphere at least: {cascade:?}");
    }
    assert_eq!(drawn, stats.shadows.cascades_drawn, "{passes:#?}");
    // They agree with the shadows' own counts: every caster draw, and every square culled from a cascade.
    let shadows = passes.shadows();
    assert_eq!(shadows.draws, stats.shadows.caster_draws);
    assert_eq!(shadows.culled_by_pass, stats.shadows.culled_draws);
    // No character listed, so no character maps.
    assert_eq!(passes.character_maps, PassCounts::default());
    assert_levels_add_up(&passes);
}
