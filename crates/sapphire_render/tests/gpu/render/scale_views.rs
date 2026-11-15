//! The scale debug views (M5 X1, the renderer's `debug_view` module): each instance's pixels in the
//! colour of what the frame did with it, the level it drew at, its textures' debt, the views that
//! drew it and the runs a cascade drew it in; a cull pose that culls in the camera's place while the
//! passes draw from the camera; the overdraw replay's depth rule; and with no view shown, every tier
//! drawing what it draws without them, byte for byte, with the same pipelines. Run alone with
//! `cargo test -p sapphire_render --test gpu render::scale_views:: -- --ignored --test-threads=1`.

use sapphire_assets::material::EnvironmentMaterial;
use sapphire_assets::texture::{Texture, TextureDesc, TextureFormat};
use sapphire_render::debug_view::{
    CULLING_LEGEND, LEVELS_LEGEND, RUNS_LEGEND, SCALE_CULLED, SCALE_NEUTRAL, ScaleCell, TEXTURES_LEGEND,
};
use sapphire_render::quality::TierTable;
use sapphire_render::texture::GpuTexture;
use sapphire_render::{CullPose, DebugView, LevelsMode, MainDepth, StreamedTops};

use super::levels::{camera_at, frames_with, shadowed, square_with_levels, street, with_levels};
use super::*;

const SIZE: (u32, u32) = (96, 96);

/// A legend's cell's display color as the target's bytes.
fn cell_bytes(legend: &[ScaleCell], name: &str) -> [u8; 3] {
    let cell = legend.iter().find(|cell| cell.name == name).expect("a cell of the legend");
    bytes(cell.color)
}

fn bytes(color: [f32; 3]) -> [u8; 3] {
    color.map(|channel| (channel * 255.0).round() as u8)
}

fn close(pixel: [u8; 4], color: [u8; 3]) -> bool {
    (0..3).all(|c| pixel[c].abs_diff(color[c]) <= 3)
}

fn at(pixels: &[[u8; 4]], (x, y): (u32, u32)) -> [u8; 4] {
    pixels[(y * SIZE.0 + x) as usize]
}

/// How many of `pixels` above the legend bar (which shows every cell, along the bottom 6% of the
/// frame) are `color`.
fn count(pixels: &[[u8; 4]], color: [u8; 3]) -> usize {
    let above = (SIZE.1 as f32 * 0.93) as usize * SIZE.0 as usize;
    pixels[..above].iter().filter(|&&pixel| close(pixel, color)).count()
}

/// One frame (after the first, which adds `instances`) showing `view`, read back.
fn shown(
    gpu: &GpuContext,
    texture: &wgpu::Texture,
    frames: &mut Frames,
    (camera, instances): (CameraData, &[MeshInstance]),
    view: DebugView,
    frame: u32,
) -> Vec<[u8; 4]> {
    let target = texture.create_view(&Default::default());
    frames.renderer.set_debug_view(view);
    let changed = if frame == 0 { instances } else { &[] };
    assert!(frames.render(gpu, &target, camera, changed, &[]).is_none(), "{view:?}");
    read_back(gpu, texture)
}

#[test]
#[ignore = "needs a GPU"]
fn the_levels_view_shows_each_draws_level_and_follows_a_forced_one() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    // The square fills most of the view; its lower-right half is every level's.
    let camera = camera_at(Vec3::new(0.0, 0.0, 1.2), Vec3::ZERO);
    let mesh = square_with_levels();
    let lower_right = (66, 66);
    for (mode, cell) in [(LevelsMode::Fixed(0), "level 0"), (LevelsMode::Fixed(1), "level 1")] {
        let (mut frames, square) = frames_with(&gpu, with_levels(mode, RenderSettings::default()), &mesh);
        let instances = [instance(0, square, Mat4::IDENTITY)];
        let pixels = shown(&gpu, &texture, &mut frames, (camera, &instances), DebugView::Levels, 0);
        let pixel = at(&pixels, lower_right);
        assert!(close(pixel, cell_bytes(&LEVELS_LEGEND, cell)), "{mode:?}: {pixel:?} isn't {cell}'s");
        assert_eq!(frames.renderer.stats().debug_view.view, DebugView::Levels);
    }
    // Chosen by error, the near squares of a street draw level 0 and the far ones level 1.
    let (mut frames, square) = frames_with(&gpu, shadowed(), &mesh);
    let (instances, camera) = street(&mut frames, square);
    let pixels = (0..3)
        .map(|frame| shown(&gpu, &texture, &mut frames, (camera, &instances), DebugView::Levels, frame))
        .last()
        .expect("frames");
    let near = count(&pixels, cell_bytes(&LEVELS_LEGEND, "level 0"));
    let far = count(&pixels, cell_bytes(&LEVELS_LEGEND, "level 1"));
    assert!(near > 0 && far > 0, "level 0 on {near} pixels, level 1 on {far}");
}

/// A 4 × 4 white texture, standing in for a streamed one.
fn white(gpu: &GpuContext) -> GpuTexture {
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 4,
        height: 4,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let texels = [255u8; 64];
    GpuTexture::upload(gpu, &Texture { desc, levels: vec![&texels] }, "white").expect("a texture")
}

#[test]
#[ignore = "needs a GPU"]
fn the_texture_view_shows_a_materials_worst_streamed_texture() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let mut frames = Frames::with_settings(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
    let map: sapphire_assets::AssetId = "3d1f6a2b-8c4e-4f5a-9b7d-2e6c8a0f1b3d".parse().expect("an asset ID");
    let environment = EnvironmentMaterial { base_color_map: Some(map), ..EnvironmentMaterial::default() };
    let swizzles = vec![Swizzle::IDENTITY; EnvironmentMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&environment, &swizzles).expect("a valid material");
    let bytes = material.to_cooked().expect("cook");
    let white = white(&gpu);
    let handle = frames
        .renderer
        .upload_material(Material::read(&bytes).expect("read"), |id| (id == map).then_some(&white))
        .expect("upload");
    let bytes = square_with_levels().to_cooked().expect("cook");
    let square = frames.renderer.upload_mesh(Mesh::read(&bytes).expect("read"), &[handle]).expect("upload");
    let instances = [instance(0, square, Mat4::IDENTITY)];
    let camera = camera_at(Vec3::new(0.0, 0.0, 1.2), Vec3::ZERO);
    let middle = (66, 66);
    // Nothing sent (streaming off, or the texture held whole): not streamed. Then the tops the app
    // sends, each a frame.
    let pixels = shown(&gpu, &texture, &mut frames, (camera, &instances), DebugView::Textures, 0);
    assert!(
        close(at(&pixels, middle), cell_bytes(&TEXTURES_LEGEND, "not streamed")),
        "{:?}",
        at(&pixels, middle)
    );
    for (frame, (wanted, resident, cell)) in (1..).zip([
        (1, 1, "as wanted"),
        (2, 1, "finer than wanted"),
        (0, 1, "a level coarse"),
        (0, 2, "two levels coarse"),
        (1, 5, "three levels coarse or more"),
    ]) {
        frames.renderer.set_texture_tops([(map, StreamedTops { wanted, resident })]);
        let pixels = shown(&gpu, &texture, &mut frames, (camera, &instances), DebugView::Textures, frame);
        let pixel = at(&pixels, middle);
        assert!(
            close(pixel, cell_bytes(&TEXTURES_LEGEND, cell)),
            "{wanted} wanted, {resident} resident: {pixel:?}"
        );
    }
}

/// Two squares a meter either side of the view's axis, 4 m ahead, and the camera; a pose 1 m to the
/// left of the camera turned 20 degrees further left, which sees the left square alone.
fn pair(square: MeshHandle) -> ([MeshInstance; 2], CameraData, CullPose, [(u32, u32); 2]) {
    let (left, right) = (Vec3::new(-1.0, 0.0, -2.0), Vec3::new(1.0, 0.0, -2.0));
    let camera = camera_at(Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, -2.0));
    let instances = [
        instance(0, square, Mat4::from_translation(left)),
        instance(1, square, Mat4::from_translation(right)),
    ];
    let pose = CullPose { position: Vec3::new(-1.0, 0.0, 2.0), yaw: (-20f32).to_radians(), pitch: 0.0 };
    // The squares' lower-right quarters, which every level covers.
    let pixel = |center: Vec3| {
        let clip = camera.proj * camera.view * (center + Vec3::new(0.2, -0.2, 0.0)).extend(1.0);
        let ndc = clip.truncate() / clip.w;
        (((ndc.x * 0.5 + 0.5) * SIZE.0 as f32) as u32, ((0.5 - ndc.y * 0.5) * SIZE.1 as f32) as u32)
    };
    (instances, camera, pose, [pixel(left), pixel(right)])
}

/// Whether `pixel` is `color` or, where its stripes are, black.
fn striped(pixel: [u8; 4], color: [f32; 3]) -> bool {
    close(pixel, bytes(color)) || close(pixel, [0; 3])
}

#[test]
#[ignore = "needs a GPU"]
fn a_cull_pose_culls_in_the_cameras_place_while_the_passes_draw_from_the_camera() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let (mut frames, square) = frames_with(&gpu, RenderSettings::default(), &square_with_levels());
    let (instances, camera, pose, [left, right]) = pair(square);
    let scene = (camera, &instances[..]);
    let neutral = bytes(SCALE_NEUTRAL);
    // From the camera both squares draw, at level 0.
    let pixels = shown(&gpu, &texture, &mut frames, scene, DebugView::Levels, 0);
    assert!(
        close(at(&pixels, left), neutral) && close(at(&pixels, right), neutral),
        "{:?} {:?}",
        at(&pixels, left),
        at(&pixels, right)
    );
    let base = shown(&gpu, &texture, &mut frames, scene, DebugView::Off, 1);
    // From the pose the right square is outside: culled, striped; the left one its level.
    frames.renderer.set_cull_pose(Some(pose));
    let pixels = shown(&gpu, &texture, &mut frames, scene, DebugView::Levels, 2);
    assert!(close(at(&pixels, left), neutral), "inside the pose: {:?}", at(&pixels, left));
    let culled: Vec<_> = (0..12).map(|dx| at(&pixels, (right.0 - 6 + dx, right.1))).collect();
    assert!(culled.iter().all(|&pixel| striped(pixel, SCALE_CULLED)), "outside the pose: {culled:?}");
    assert!(culled.iter().any(|&pixel| close(pixel, bytes(SCALE_CULLED))), "{culled:?}");
    let pixels = shown(&gpu, &texture, &mut frames, scene, DebugView::Culling, 3);
    let frustum =
        CULLING_LEGEND.iter().find(|cell| cell.name == "culled by the frustum").expect("a cell").color;
    assert!(striped(at(&pixels, right), frustum), "{:?}", at(&pixels, right));
    assert!(
        close(at(&pixels, left), cell_bytes(&CULLING_LEGEND, "the main view alone")),
        "{:?}",
        at(&pixels, left)
    );
    assert_eq!(frames.renderer.stats().view_culling.culled, 1, "the pose's verdicts");
    // With no scale view the pose changes nothing: the frame is the camera's, byte for byte.
    let off = shown(&gpu, &texture, &mut frames, scene, DebugView::Off, 4);
    assert!(off == base, "a pose with no view shown is ignored");
    assert_eq!(frames.renderer.stats().view_culling.culled, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn the_runs_view_shows_equal_props_a_cascade_draws_in_one_call() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let (mut frames, square) = frames_with(&gpu, shadowed(), &square_with_levels());
    frames.sun = DirectionalLight {
        direction: Vec3::new(-0.4, -0.7, -0.6).normalize(),
        color: Vec3::ONE,
        intensity: 1.0,
    };
    // Four equal squares side by side, near enough to draw level 0 in the view and the cascades.
    let instances: Vec<_> = (0..4)
        .map(|n| instance(n, square, Mat4::from_translation(Vec3::new(-1.2 + 0.8 * n as f32, 0.0, -2.0))))
        .collect();
    let camera = camera_at(Vec3::new(0.0, 0.0, 1.5), Vec3::new(0.0, 0.0, -2.0));
    let pixels = (0..3)
        .map(|frame| shown(&gpu, &texture, &mut frames, (camera, &instances), DebugView::Runs, frame))
        .last()
        .expect("frames");
    let runs = frames.renderer.stats().runs;
    let in_runs = count(&pixels, cell_bytes(&RUNS_LEGEND, "a run of 3 to 8"));
    assert!(in_runs > 200, "{in_runs} pixels in a run of 3 to 8; {runs:?}");
    assert_eq!(count(&pixels, cell_bytes(&RUNS_LEGEND, "alone")), 0, "{runs:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn with_no_view_shown_every_tier_draws_what_it_draws_without_them() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let mesh = square_with_levels();
    let table = TierTable::shipped();
    for tier in table.tiers() {
        let settings = tier.render_settings();
        // A renderer that never hears of the scale views, and one sent a pose and tops with none
        // shown: the same picture, the same counts and the same pipelines.
        let [plain, told] = [false, true].map(|told| {
            let (mut frames, square) = frames_with(&gpu, settings, &mesh);
            let (instances, camera) = street(&mut frames, square);
            if told {
                frames.renderer.set_cull_pose(Some(CullPose { position: Vec3::ZERO, yaw: 3.0, pitch: 0.0 }));
                frames.renderer.set_texture_tops([]);
            }
            let pixels = (0..3)
                .map(|frame| shown(&gpu, &texture, &mut frames, (camera, &instances), DebugView::Off, frame))
                .last()
                .expect("frames");
            let stats = frames.renderer.stats();
            // As shipped, every tier's main pass writes its own depth, which the overdraw
            // view's replay counts as.
            assert_eq!(stats.main_depth, MainDepth::Own, "{:?}", tier.id);
            (pixels, stats)
        });
        assert!(plain.0 == told.0, "{:?}: the same picture", tier.id);
        assert_eq!(plain.1.passes, told.1.passes, "{:?}", tier.id);
        assert_eq!(plain.1.view_culling, told.1.view_culling, "{:?}", tier.id);
        let pipelines = |stats: &sapphire_render::RenderStats| {
            (
                stats.character_pipelines,
                stats.environment_pipelines,
                stats.foliage_pipelines,
                stats.debug_view,
            )
        };
        assert_eq!(pipelines(&plain.1), pipelines(&told.1), "{:?}", tier.id);
        assert_eq!(told.1.debug_view, Default::default(), "{:?}: nothing of the views built", tier.id);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_overdraw_view_answers_to_the_main_depth_a_frame_took() {
    let gpu = gpu();
    let texture = target(&gpu, FORMAT, SIZE);
    let mesh = square_with_levels();
    let table = TierTable::shipped();
    // A desktop tier's settings with its own depth and with the prepass's asked for (M5 D10): the
    // overdraw view shows on both, and the frame says which depth its main pass took.
    let asked = |tier: &sapphire_render::quality::QualityTier| RenderSettings {
        main_depth: MainDepth::FromPrepass,
        ..tier.render_settings()
    };
    let tier =
        table.tiers().iter().find(|tier| asked(tier).main_depth_from_prepass()).expect("a desktop tier");
    for asked in [MainDepth::Own, MainDepth::FromPrepass] {
        let settings = RenderSettings { main_depth: asked, ..tier.render_settings() };
        let (mut frames, square) = frames_with(&gpu, settings, &mesh);
        let (instances, camera) = street(&mut frames, square);
        let pixels = (0..3)
            .map(|frame| shown(&gpu, &texture, &mut frames, (camera, &instances), DebugView::Overdraw, frame))
            .last()
            .expect("frames");
        let stats = frames.renderer.stats();
        assert_eq!(stats.main_depth, asked, "the depth the main pass took");
        assert_eq!(stats.debug_view.view, DebugView::Overdraw, "{asked:?}");
        assert!(pixels.iter().any(|pixel| pixel[..3] != [0, 0, 0]), "{asked:?}: the view drew");
    }
}
