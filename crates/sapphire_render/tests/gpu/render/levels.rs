//! Levels of detail for static draws (M5 D4, the renderer's `levels` module): a level draws its own
//! index ranges from a prefix of its mesh's vertices; the main view's family (the scene prepass and
//! the main pass) draws one level of an instance, and each cascade its own; and with levels off the
//! renderer draws what it drew before levels were chosen, byte for byte. Run alone with
//! `cargo test -p sapphire_render --test gpu render::levels:: -- --ignored --test-threads=1`.

use sapphire_assets::material::{EnvironmentMaterial, MaterialDocument};
use sapphire_assets::mesh::Lod;
use sapphire_render::quality::TierTable;
use sapphire_render::{LevelSettings, LevelsMode, ShadowSettings};

use super::*;

const SIZE: (u32, u32) = (96, 96);
/// The cameras' vertical field of view, radians: at 96 rows, 99 output pixels a meter covers at a
/// meter, so [`ERROR`] covers a pixel from 5 m.
const FOV: f32 = 0.9;
/// Level 1's error, meters.
const ERROR: f32 = 0.05;

/// A square 1 m across facing +z around its origin. Level 0 is its two triangles over its four
/// vertices; level 1, at [`ERROR`], the lower-right triangle alone, over the first three vertices (a
/// prefix of the streams) and an index range of its own after level 0's.
fn square_with_levels() -> Mesh {
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [x + 0.5, 0.5 - y],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [corner(-0.5, -0.5), corner(0.5, -0.5), corner(0.5, 0.5), corner(-0.5, 0.5)];
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("square", cooked(&AnimeCharacterMaterial::default()))];
    let indices = vec![0, 1, 2, 0, 2, 3, 0, 1, 2];
    let mut mesh = Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16);
    let coarse = vec![Submesh { first_index: 6, index_count: 3, material: 0 }];
    mesh.lods.push(Lod { vertex_count: 3, error: ERROR, submeshes: coarse });
    mesh
}

/// A renderer of `settings` with `mesh` uploaded under an opaque red environment material.
fn frames_with(gpu: &GpuContext, settings: RenderSettings, mesh: &Mesh) -> (Frames, MeshHandle) {
    let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    let environment =
        EnvironmentMaterial { base_color: [0.9, 0.1, 0.1, 1.0], ..EnvironmentMaterial::default() };
    let swizzles = vec![Swizzle::IDENTITY; environment.textures().len()];
    let material = Material::from_document(&environment, &swizzles).expect("a valid material");
    let bytes = material.to_cooked().expect("cook");
    let material =
        frames.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload");
    let bytes = mesh.to_cooked().expect("cook");
    let handle = frames.renderer.upload_mesh(Mesh::read(&bytes).expect("read"), &[material]).expect("upload");
    (frames, handle)
}

fn with_levels(mode: LevelsMode, settings: RenderSettings) -> RenderSettings {
    RenderSettings { levels: LevelSettings { mode, ..settings.levels }, ..settings }
}

fn camera_at(eye: Vec3, target: Vec3) -> CameraData {
    CameraData::look_at(eye, target, Vec3::Y, FOV, SIZE.0 as f32 / SIZE.1 as f32, 0.05)
}

#[test]
#[ignore = "needs a GPU"]
fn a_level_draws_its_own_triangles_from_a_prefix_of_the_vertices() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    // The square fills most of the view: its upper-left half is level 0's alone.
    let camera = camera_at(Vec3::new(0.0, 0.0, 1.2), Vec3::ZERO);
    let mesh = square_with_levels();
    let picture = |mode| {
        let (mut frames, square) = frames_with(&gpu, with_levels(mode, RenderSettings::default()), &mesh);
        assert!(frames.render(&gpu, &view, camera, &[instance(0, square, Mat4::IDENTITY)], &[]).is_none());
        (read_back(&gpu, &texture), frames.renderer.stats().passes.main)
    };
    let ((whole, at_0), (coarse, at_1)) = (picture(LevelsMode::Fixed(0)), picture(LevelsMode::Fixed(1)));
    let at = |pixels: &[[u8; 4]], x: u32, y: u32| pixels[(y * SIZE.0 + x) as usize];
    let (sky, upper_left, lower_right) = ((1, 1), (30, 30), (66, 66));
    assert_eq!(at(&whole, sky.0, sky.1), at(&coarse, sky.0, sky.1), "the sky");
    assert_ne!(at(&whole, upper_left.0, upper_left.1), at(&whole, sky.0, sky.1), "level 0 covers it");
    assert_eq!(at(&coarse, upper_left.0, upper_left.1), at(&coarse, sky.0, sky.1), "level 1 doesn't");
    assert_eq!(at(&whole, lower_right.0, lower_right.1), at(&coarse, lower_right.0, lower_right.1));
    assert_ne!(at(&coarse, lower_right.0, lower_right.1), at(&coarse, sky.0, sky.1), "level 1 covers it");
    // The main pass counts the square at its level, and the sky's triangle.
    assert_eq!((at_0.levels, at_0.triangles), ([1, 0, 0, 0], 3), "{at_0:?}");
    assert_eq!((at_1.levels, at_1.triangles), ([0, 1, 0, 0], 2), "{at_1:?}");
}

/// Squares from 1 m to 40 m down the view, with shadows on and the sun from the side.
fn street(frames: &mut Frames, square: MeshHandle) -> (Vec<MeshInstance>, CameraData) {
    frames.sun = DirectionalLight {
        direction: Vec3::new(-0.4, -0.7, -0.6).normalize(),
        color: Vec3::ONE,
        intensity: 1.0,
    };
    let instances = (0..14)
        .map(|n| {
            let distance = 1.0 + 3.0 * n as f32;
            let at = Vec3::new(if n % 2 == 0 { -0.6 } else { 0.6 }, 0.5, -distance);
            instance(n, square, Mat4::from_translation(at))
        })
        .collect();
    (instances, camera_at(Vec3::new(0.0, 0.6, 0.0), Vec3::new(0.0, 0.5, -10.0)))
}

/// Shadows with coarse maps, 256 texels a cascade, none cached so each draws every frame: the
/// farthest cascade's quarter texel passes [`ERROR`], and the nearest's doesn't.
fn shadowed() -> RenderSettings {
    let shadows = ShadowSettings { resolution: 256, cached_cascades: 0, ..ShadowSettings::default() };
    RenderSettings { shadows: Some(shadows), ..RenderSettings::default() }
}

#[test]
#[ignore = "needs a GPU"]
fn levels_off_draw_what_a_mesh_without_levels_draws() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let leveled = square_with_levels();
    let mut plain = leveled.clone();
    plain.lods.truncate(1);
    // Levels off with a mesh that has them; levels on with the same mesh without: what the renderer
    // drew before levels were chosen. Then levels on with them, which draws coarser. Plain settings
    // with shadows first, then every shipped tier's, its bias included.
    let table = TierTable::shipped();
    let tiers = table.tiers().iter().map(|tier| (format!("{:?}", tier.id), tier.render_settings()));
    let mut on = None;
    for (name, settings) in std::iter::once(("shadows".to_owned(), shadowed())).chain(tiers) {
        let runs = [(LevelsMode::Off, &leveled), (LevelsMode::On, &plain), (LevelsMode::On, &leveled)];
        let [off, base, leveled_on] = runs.map(|(mode, mesh)| {
            let (mut frames, square) = frames_with(&gpu, with_levels(mode, settings), mesh);
            let (instances, camera) = street(&mut frames, square);
            for frame in 0..3 {
                let changed: &[MeshInstance] = if frame == 0 { &instances } else { &[] };
                assert!(frames.render(&gpu, &view, camera, changed, &[]).is_none(), "{name}");
            }
            (read_back(&gpu, &texture), frames.renderer.stats())
        });
        assert!(off.0 == base.0, "{name}: levels off draw the base's picture, byte for byte");
        assert_eq!(off.1.passes.main, base.1.passes.main, "{name}");
        assert_eq!(off.1.passes.shadows(), base.1.passes.shadows(), "{name}");
        assert_eq!(off.1.passes.total().levels[1..], [0; 3], "{name}");
        on.get_or_insert((leveled_on, base));
    }
    let (on, base) = on.expect("the plain settings ran");
    // On, the far squares draw level 1 in the view and in the far cascades.
    let main = on.1.passes.main;
    assert!(main.levels[0] > 0 && main.levels[1] > 0, "{main:?}");
    assert!(on.1.passes.shadows().levels[1] > 0, "{:?}", on.1.passes.shadows());
    assert!(main.triangles < base.1.passes.main.triangles);
    assert!(on.0 != base.0, "the coarse squares show");
}

#[test]
#[ignore = "needs a GPU"]
fn the_prepass_and_the_main_pass_draw_one_level_and_each_cascade_its_own() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let (mut frames, square) = frames_with(&gpu, shadowed(), &square_with_levels());
    let (instances, camera) = street(&mut frames, square);
    frames.renderer.keep_scene_prepass();
    let readback = Readback::new(&gpu, SIZE);
    let reads: [PrepassRead<'_>; 1] = [(|targets| targets.normals, &readback)];
    for frame in 0..3 {
        let changed: &[MeshInstance] = if frame == 0 { &instances } else { &[] };
        assert!(frames.render_reading(&gpu, &view, camera, changed, &[], &reads).is_none());
        let stats = frames.renderer.stats();
        let (main, prepass) = (stats.passes.main, stats.passes.scene_prepass);
        // The same draws at the same levels, the sky's triangle aside.
        assert_eq!(main.levels, prepass.levels, "frame {frame}");
        assert_eq!(main.triangles, prepass.triangles + 1, "frame {frame}");
        assert!(main.levels[0] > 0 && main.levels[1] > 0, "frame {frame}: {main:?}");
        // The camera held still: no switch after the first frame.
        assert_eq!(stats.levels.switches(), 0, "frame {frame}");
        // Cascade 0, the nearest and finest, draws level 0 where a later one draws level 1.
        let cascades = stats.passes.cascades;
        assert!(cascades[0].levels[0] > 0, "frame {frame}: {cascades:?}");
        assert!(cascades.iter().any(|cascade| cascade.levels[1] > 0), "frame {frame}: {cascades:?}");
    }
}
