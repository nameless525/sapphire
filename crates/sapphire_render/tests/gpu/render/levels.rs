//! Levels of detail for static draws (M5 D4, the renderer's `levels` module): a level draws its own
//! index ranges from a prefix of its mesh's vertices; the main view's family (the scene prepass and
//! the main pass) draws one level of an instance, and each cascade its own; and with levels off the
//! renderer draws what it drew before levels were chosen, byte for byte; and a zone's cells, cooked
//! apart with their borders locked (M5 D6), meet at any two levels with no crack, in the view and in a
//! cascade. Run alone with
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
pub(super) fn square_with_levels() -> Mesh {
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
pub(super) fn frames_with(gpu: &GpuContext, settings: RenderSettings, mesh: &Mesh) -> (Frames, MeshHandle) {
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

pub(super) fn with_levels(mode: LevelsMode, settings: RenderSettings) -> RenderSettings {
    RenderSettings { levels: LevelSettings { mode, ..settings.levels }, ..settings }
}

pub(super) fn camera_at(eye: Vec3, target: Vec3) -> CameraData {
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
pub(super) fn street(frames: &mut Frames, square: MeshHandle) -> (Vec<MeshInstance>, CameraData) {
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
pub(super) fn shadowed() -> RenderSettings {
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

/// A zone's grid for the seam tests (M5 D6): cells 16 m a side.
const CELL: f32 = 16.0;
/// Ground quads along a cell's side: half a meter each, so the cells' shared vertices are the same bits.
const QUADS: u32 = 32;
/// The seam tests' target, square: a crack a pixel wide shows.
const SEAM_SIZE: (u32, u32) = (256, 256);

/// The test ground's height at `(x, z)`: one function across every cell, so neighbours agree where they
/// meet, with hills enough that a cell's border is no straight line.
fn hills(x: f32, z: f32) -> f32 {
    0.5 * (0.45 * x + 0.3).sin() * (0.35 * z - 0.2).cos() + 0.2 * (1.3 * (x + z)).sin()
}

/// Cell `(cx, cz)`'s ground cooked as a zone cooks a cell's mesh with the benchmark district's levels
/// (`sapphire_pipeline::zone::optimize_place`), its border and the grid's lines locked; or, `locked`
/// false, as any mesh cooks, with a coarser chain, its border free to simplify along itself (the control).
fn cell_ground(cx: u32, cz: u32, locked: bool) -> Mesh {
    use sapphire_pipeline::cook::Cook;
    use sapphire_pipeline::mesh::{ImportedMaterial, ImportedMesh};
    use sapphire_pipeline::optimize::{self, LodLevel, LodSettings};
    use sapphire_pipeline::{bake, zone};

    let step = CELL / QUADS as f32;
    let mut vertices = Vec::new();
    for k in cz * QUADS..=(cz + 1) * QUADS {
        for i in cx * QUADS..=(cx + 1) * QUADS {
            let (x, z) = (i as f32 * step, k as f32 * step);
            let e = 0.01;
            let dx = (hills(x + e, z) - hills(x - e, z)) / (2.0 * e);
            let dz = (hills(x, z + e) - hills(x, z - e)) / (2.0 * e);
            let normal = Vec3::new(-dx, 1.0, -dz).normalize().to_array();
            vertices.push(Vertex {
                position: [x, hills(x, z), z],
                normal,
                outline_normal: normal,
                uv: [x / 4.0, z / 4.0],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
        }
    }
    let stride = QUADS + 1;
    let mut indices = Vec::new();
    for k in 0..QUADS {
        for i in 0..QUADS {
            let a = k * stride + i;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, b, c, c, b, d]);
        }
    }
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let mut mesh = ImportedMesh {
        authored_outline: vec![false; vertices.len()],
        lods: vec![Lod { vertex_count: vertices.len() as u32, error: 0.0, submeshes }],
        materials: vec![ImportedMaterial { name: "ground".to_owned(), generated: Default::default() }],
        vertices,
        indices,
        ..ImportedMesh::default()
    };
    let level = |triangle_ratio, max_error| LodLevel { triangle_ratio, max_error };
    // The control's chain is coarser, so its free border moves as far as a pixel at this picture's size.
    let levels = if locked {
        vec![level(0.5, 0.0005), level(0.25, 0.002), level(0.125, 0.006)]
    } else {
        vec![level(0.25, 0.01), level(0.06, 0.03), level(0.02, 0.06)]
    };
    let settings = zone::ZoneSettings { lods: LodSettings::new(levels, 0).unwrap(), ..Default::default() };
    bake::outline_normals(&mut mesh, &settings.outline);
    if locked {
        let grid = sapphire_zone::grid::Grid { cell_size: CELL };
        zone::optimize_place(&mut mesh, &settings, Some(grid), Cook::Desktop);
    } else {
        optimize::optimize(&mut mesh, &settings.lods, settings.outline.weld_tolerance(), Cook::Desktop);
    }
    assert_eq!(mesh.lods.len(), 4, "every level of the chain kept");
    mesh.pack(PositionFormat::Float32, &Default::default()).expect("packs")
}

/// Level `level` of `mesh`, as a mesh of that one level: what a draw at that level draws, from level 0's
/// streams.
fn one_level(mesh: &Mesh, level: usize) -> Mesh {
    let mut one = mesh.clone();
    one.lods = vec![Lod { error: 0.0, vertex_count: mesh.lods[0].vertex_count, ..one.lods[level].clone() }];
    one
}

/// The four cells about the point (`CELL`, `CELL`), cooked apart: `[(0, 0), (1, 0), (0, 1), (1, 1)]`.
fn four_cells(locked: bool) -> [Mesh; 4] {
    [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(cx, cz)| cell_ground(cx, cz, locked))
}

/// A picture of the four cells, each drawn at its level in `levels` and raised by `lift`, with `floor`
/// under them if given, the sun from above, under `settings`.
fn cells_picture(
    gpu: &GpuContext,
    settings: RenderSettings,
    camera: CameraData,
    cells: &[Mesh; 4],
    levels: [usize; 4],
    lift: f32,
    floor: Option<&Mesh>,
) -> Vec<[u8; 4]> {
    let texture = target(gpu, FORMAT, SEAM_SIZE);
    let view = texture.create_view(&Default::default());
    let settings = with_levels(LevelsMode::Off, settings);
    let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    frames.sun = DirectionalLight {
        direction: Vec3::new(0.15, -1.0, 0.1).normalize(),
        color: Vec3::ONE,
        intensity: 1.0,
    };
    let environment =
        EnvironmentMaterial { base_color: [0.9, 0.1, 0.1, 1.0], ..EnvironmentMaterial::default() };
    let swizzles = vec![Swizzle::IDENTITY; environment.textures().len()];
    let bytes = Material::from_document(&environment, &swizzles).expect("a material").to_cooked().unwrap();
    let material = frames.renderer.upload_material(Material::read(&bytes).unwrap(), |_| None).unwrap();
    let mut upload = |mesh: &Mesh| {
        let bytes = mesh.to_cooked().expect("cook");
        frames.renderer.upload_mesh(Mesh::read(&bytes).expect("read"), &[material]).expect("upload")
    };
    let raised = Mat4::from_translation(Vec3::Y * lift);
    let mut instances: Vec<MeshInstance> = (cells.iter().zip(levels).enumerate())
        .map(|(n, (cell, level))| instance(n as u64, upload(&one_level(cell, level)), raised))
        .collect();
    if let Some(floor) = floor {
        instances.push(instance(9, upload(floor), Mat4::IDENTITY));
    }
    for frame in 0..2 {
        let changed: &[MeshInstance] = if frame == 0 { &instances } else { &[] };
        assert!(frames.render(gpu, &view, camera, changed, &[]).is_none());
    }
    read_back(gpu, &texture)
}

/// A camera standing over the seam between the cells in X, looking along it and down far enough that every
/// pixel is ground: the top rows graze the seam about 5° from flat, 15 m ahead, past the corner where all
/// four cells meet.
fn grazing() -> CameraData {
    let eye = Vec3::new(CELL, 1.4, 2.0);
    let ahead = eye + Vec3::new(0.0, -(31f32.to_radians().tan()), 1.0);
    CameraData::look_at(eye, ahead, Vec3::Y, FOV, 1.0, 0.05)
}

/// Every pair of levels a seam can show: the cells in a checkerboard, so each seam has level `i` on one
/// side and `j` on the other, and the corner both twice.
fn checkerboards() -> impl Iterator<Item = [usize; 4]> {
    (0..4).flat_map(|i| (0..4).map(move |j| [i, j, j, i]))
}

/// Two cells at any two levels meet with no crack (M5 D6): seen grazing along their seam, no pixel of the
/// picture is the empty sky's. The same cells cooked with their borders free crack somewhere, so the test
/// can see a crack.
#[test]
#[ignore = "needs a GPU"]
fn cells_at_any_two_levels_meet_with_no_crack() {
    let gpu = gpu();
    let camera = grazing();
    let settings = RenderSettings::default();
    let empty = {
        let texture = target(&gpu, FORMAT, SEAM_SIZE);
        let view = texture.create_view(&Default::default());
        let mut frames = Frames::with_settings(&gpu, FORMAT, settings).expect("renderer");
        for _ in 0..2 {
            assert!(frames.render(&gpu, &view, camera, &[], &[]).is_none());
        }
        read_back(&gpu, &texture)
    };
    let see_through = |picture: &[[u8; 4]]| picture.iter().zip(&empty).filter(|(a, b)| a == b).count();
    let cells = four_cells(true);
    for levels in checkerboards() {
        let picture = cells_picture(&gpu, settings, camera, &cells, levels, 0.0, None);
        assert_eq!(see_through(&picture), 0, "levels {levels:?}: the sky shows through");
    }
    let free = four_cells(false);
    let cracked: usize = checkerboards()
        .map(|levels| see_through(&cells_picture(&gpu, settings, camera, &free, levels, 0.0, None)))
        .sum();
    assert!(cracked > 0, "the control's free borders crack where the test can see it");
}

/// A floor 64 m across at y = 0, facing up.
fn floor() -> Mesh {
    let corner = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: [0.0, 1.0, 0.0],
        outline_normal: [0.0, 1.0, 0.0],
        uv: [x, z],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [corner(-16.0, -16.0), corner(-16.0, 48.0), corner(48.0, 48.0), corner(48.0, -16.0)];
    let submeshes = vec![Submesh { first_index: 0, index_count: 6, material: 0 }];
    let slots = vec![MaterialSlot::embedded("floor", cooked(&AnimeCharacterMaterial::default()))];
    Mesh::from_vertices(&vertices, vec![0, 1, 2, 0, 2, 3], submeshes, slots, PositionFormat::Float32)
}

/// The same in a cascade: the four cells raised into a roof 5 m over a floor, the sun from above, and a
/// camera under the roof looking down at the floor where the seams cross. At any two levels the floor is
/// the picture it is at level 0, which is not the picture with the cells sunk under the floor: no line of
/// light leaks through a seam.
#[test]
#[ignore = "needs a GPU"]
fn cells_at_any_two_levels_cast_with_no_light_leak() {
    let gpu = gpu();
    let eye = Vec3::new(CELL, 3.0, CELL - 1.0);
    let camera = CameraData::look_at(eye, Vec3::new(CELL, 0.0, CELL + 0.2), Vec3::Y, FOV, 1.0, 0.05);
    let shadows = ShadowSettings { resolution: 2048, cached_cascades: 0, ..ShadowSettings::default() };
    let settings = RenderSettings { shadows: Some(shadows), ..RenderSettings::default() };
    let cells = four_cells(true);
    let floor = floor();
    let picture = |levels, lift| cells_picture(&gpu, settings, camera, &cells, levels, lift, Some(&floor));
    let whole = picture([0; 4], 5.0);
    assert!(whole != picture([0; 4], -5.0), "the roof's shadow covers the floor in view");
    for levels in checkerboards().skip(1) {
        assert!(picture(levels, 5.0) == whole, "levels {levels:?}: the floor under the seams changed");
    }
}
