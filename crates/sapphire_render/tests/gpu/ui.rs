//! The on-screen interface (M2 I2, the `ui` module): a frame with no quads builds and draws nothing,
//! the first with quads builds one pipeline, a widget's pixels land where its anchors put them at
//! every aspect ratio and inside any insets, colors land on their codes, edits to `ui.wgsl` rebuild
//! it, a new renderer brings its atlas along and gets its glyph pages filled again, and debug draw's
//! lines stay visible over it. They need
//! a GPU: run with `cargo test -p
//! sapphire_render --test gpu ui:: -- --ignored`.

use std::f32::consts::PI;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use sapphire_core::debug_draw::{Color, DebugLine, Depth};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, PipelineInventory, PostParams,
    RenderSettings, Renderer, ShaderFile, ShaderSources, ShaderUpdate, SkyGradient, ToneCurve, UI_PASS,
    UiAtlas, UiDrawList, UiFieldFormat, UiPageUpload, UiQuad, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};
use sapphire_ui::{
    Atlas, Button, ButtonState, CombatHud, FighterValues, HudLayout, HudValues, Insets, Paint, Painter, Rect,
    SkillValues, TouchActions, UiMetrics,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// The pipelines alive on `gpu`'s device, of both kinds.
fn pipelines(gpu: &GpuContext) -> usize {
    let report = gpu.instance().raw().generate_report().expect("a wgpu-core backend");
    report.hub.render_pipelines.num_kept_from_user + report.hub.compute_pipelines.num_kept_from_user
}

/// A dark sky and post that keeps it dark, so a widget's pixels stand out and are exact.
fn dark_packet(ring: &mut FramePacketRing, size: (u32, u32), ui: Option<&UiDrawList>) {
    let packet = ring.begin();
    let aspect = size.0 as f32 / size.1 as f32;
    let eye = Vec3::new(0.0, 0.0, 5.0);
    packet.views.push(ViewData::new(CameraData::look_at(
        eye,
        Vec3::ZERO,
        Vec3::Y,
        40.0 * PI / 180.0,
        aspect,
        0.1,
    )));
    packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
    packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
    packet.post = PostParams {
        exposure: 0.0,
        bloom: BloomParams { intensity: 0.0, ..BloomParams::default() },
        tone_curve: ToneCurve {
            linear_start: 0.001,
            linear_length: 0.99,
            black_tightness: 1.0,
            ..ToneCurve::default()
        },
        grade_a: ColorGrade::NEUTRAL,
        grade_b: ColorGrade::NEUTRAL,
        grade_blend: 0.0,
        filter_strength: 1.0,
        vignette: Vignette { intensity: 0.0, ..Vignette::default() },
        ..PostParams::default()
    };
    if let Some(ui) = ui {
        packet.ui.clone_from(ui);
    }
}

/// Frames drawn with their own renderer and executor.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    ring: FramePacketRing,
    size: (u32, u32),
    target: wgpu::Texture,
}

impl Harness {
    fn new(size: (u32, u32)) -> Self {
        let gpu = gpu();
        let renderer = Renderer::new(&gpu, FORMAT, RenderSettings::default()).expect("renderer");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { gpu, renderer, graph: GraphExecutor::new(), ring: FramePacketRing::new(), size, target }
    }

    /// Draws a frame of `ui` and returns its RGBA8 pixels, rows from the top.
    fn draw(&mut self, ui: Option<&UiDrawList>) -> Pixels {
        self.draw_with_lines(ui, &[])
    }

    /// [`Self::draw`], with debug draw's `lines` in the packet.
    fn draw_with_lines(&mut self, ui: Option<&UiDrawList>, lines: &[DebugLine]) -> Pixels {
        dark_packet(&mut self.ring, self.size, ui);
        let packet = self.ring.current_mut();
        packet.debug.lines.extend_from_slice(lines);
        packet.debug.stats.lines = lines.len() as u32;
        self.renderer.prepare(&self.gpu, self.ring.current());
        let view = self.target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a frame with a view records");
        self.graph.execute(&self.gpu, graph).expect("a valid frame");
        read_back(&self.gpu, &self.target, self.size)
    }

    /// Whether the last frame's graph ran the interface's pass.
    fn drew_ui_pass(&self) -> bool {
        let ran = |pass: &sapphire_render::graph::PassReport| pass.name == UI_PASS && pass.order.is_some();
        self.graph.report().is_some_and(|report| report.passes.iter().any(ran))
    }
}

/// A frame read back, with what the tests ask of it.
struct Pixels {
    size: (u32, u32),
    rgba: Vec<u8>,
}

impl Pixels {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * self.size.0 + x) * 4) as usize;
        self.rgba[start..start + 4].try_into().expect("four bytes")
    }

    /// The smallest rectangle of pixels near `color`, each channel within `tolerance`, as
    /// `(x0, y0, x1, y1)`, inclusive, if any are.
    fn bounds_of(
        &self,
        color: [u8; 3],
        tolerance: i32,
        area: (u32, u32, u32, u32),
    ) -> Option<(u32, u32, u32, u32)> {
        let mut bounds: Option<(u32, u32, u32, u32)> = None;
        for y in area.1..area.3.min(self.size.1) {
            for x in area.0..area.2.min(self.size.0) {
                let pixel = self.at(x, y);
                let near = (0..3).all(|i| (i32::from(pixel[i]) - i32::from(color[i])).abs() <= tolerance);
                if near {
                    bounds = Some(match bounds {
                        None => (x, y, x, y),
                        Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                    });
                }
            }
        }
        bounds
    }
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture, size: (u32, u32)) -> Pixels {
    let row = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row) * u64::from(size.1),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size.1),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    let rgba =
        mapped.chunks_exact(row as usize).flat_map(|line| &line[..size.0 as usize * 4]).copied().collect();
    Pixels { size, rgba }
}

/// A list of `paint` over the atlas, at a viewport the size of the frame.
fn list(size: (u32, u32), paint: impl FnOnce(&mut Painter<'_>)) -> UiDrawList {
    let atlas = Atlas::paint();
    let mut quads = Vec::new();
    paint(&mut Painter::new(&mut quads, &atlas.sprites));
    UiDrawList {
        viewport: [size.0 as f32, size.1 as f32],
        atlas: Some(atlas.image),
        quads,
        ..UiDrawList::default()
    }
}

/// Its pipeline's on the inventory: declared, the first quads and the first glyph upload build nothing; not,
/// they're a named first use. Glyph pages draw through the same pipeline, so they declare nothing more.
#[test]
#[ignore = "needs a GPU"]
fn the_first_quads_build_nothing_only_when_the_inventory_declared_them() {
    let quads = list((64, 64), |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::splat(32.0), Vec2::splat(20.0), 0.0, Paint::fill([255; 4]));
    });
    let mut cold = Harness::new((64, 64));
    cold.draw(None);
    let before = cold.renderer.stats().pipelines.first_use_builds;
    cold.draw(Some(&quads));
    assert_eq!(cold.renderer.stats().pipelines.first_use_builds, before + 1);
    assert!(cold.renderer.first_use_report().to_string().contains("ui: "));

    let mut warmed = Harness::new((64, 64));
    let inventory = PipelineInventory { ui: true, ..PipelineInventory::default() };
    assert_eq!(warmed.renderer.prewarm(&inventory), 1);
    assert_eq!(warmed.renderer.prewarm(&inventory), 0);
    warmed.draw(None);
    let before = warmed.renderer.stats().pipelines.first_use_builds;
    warmed.draw(Some(&quads));
    assert_eq!(warmed.renderer.stats().pipelines.first_use_builds, before);
    assert_eq!(warmed.renderer.stats().ui.pipelines, 1);

    let mut glyphs = UiDrawList { viewport: [64.0, 64.0], page_size: 64, ..UiDrawList::default() };
    upload_block(&mut glyphs, 0, (0, 0), (4, 4), 255);
    let mut cold = Harness::new((64, 64));
    cold.draw(None);
    let before = cold.renderer.stats().pipelines.first_use_builds;
    cold.draw(Some(&glyphs));
    assert_eq!(
        cold.renderer.stats().pipelines.first_use_builds,
        before + 1,
        "a page's first upload builds it"
    );
    let mut warmed = Harness::new((64, 64));
    warmed.renderer.prewarm(&inventory);
    glyphs.page_state = warmed.renderer.ui_page_state();
    warmed.draw(None);
    let before = warmed.renderer.stats().pipelines.first_use_builds;
    warmed.draw(Some(&glyphs));
    assert_eq!(warmed.renderer.stats().pipelines.first_use_builds, before);
    assert_eq!(warmed.renderer.stats().ui.page_uploads, 1);
}

#[test]
#[ignore = "needs a GPU"]
fn a_frame_with_no_quads_builds_nothing_and_adds_no_pass() {
    let mut harness = Harness::new((64, 64));
    let before = pipelines(&harness.gpu);
    assert_eq!(harness.renderer.stats().ui.pipelines, 0);
    harness.graph.set_report(true);

    // No list, and a list with nothing in it.
    harness.draw(None);
    assert!(!harness.drew_ui_pass());
    let empty = UiDrawList {
        viewport: [64.0, 64.0],
        atlas: Some(Atlas::paint().image),
        quads: Vec::new(),
        ..UiDrawList::default()
    };
    harness.draw(Some(&empty));
    assert!(!harness.drew_ui_pass());
    assert_eq!(pipelines(&harness.gpu), before, "nothing built for a frame without quads");
    assert_eq!(harness.renderer.stats().ui.atlas_uploads, 0, "and nothing uploaded");

    // Quads with no atlas to read draw nothing either: nothing to build for.
    let mut no_atlas = list((64, 64), |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::splat(32.0), Vec2::splat(20.0), 0.0, Paint::fill([255; 4]));
    });
    no_atlas.atlas = None;
    harness.draw(Some(&no_atlas));
    assert!(!harness.drew_ui_pass());
    assert_eq!(pipelines(&harness.gpu), before);

    // The first quads build its one pipeline, and upload its atlas once.
    let quads = list((64, 64), |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::splat(32.0), Vec2::splat(20.0), 0.0, Paint::fill([255; 4]));
    });
    harness.draw(Some(&quads));
    assert!(harness.drew_ui_pass());
    assert_eq!(pipelines(&harness.gpu), before + 1, "one pipeline, on the first draw");
    let stats = harness.renderer.stats().ui;
    assert_eq!((stats.pipelines, stats.quads, stats.atlas_uploads), (1, 1, 1));
    assert_eq!(stats.atlas_bytes, quads.atlas.as_ref().unwrap().texels.len() as u64);

    // Later frames build and upload nothing, and an empty frame goes back to no pass.
    harness.draw(Some(&quads));
    assert_eq!(pipelines(&harness.gpu), before + 1);
    assert_eq!(harness.renderer.stats().ui.atlas_uploads, 1);
    harness.draw(None);
    assert!(!harness.drew_ui_pass());
    assert_eq!(harness.renderer.stats().ui.quads, 0);
    assert_eq!(pipelines(&harness.gpu), before + 1);
}

#[test]
#[ignore = "needs a GPU"]
fn a_widget_s_pixels_land_where_it_was_placed_on_its_exact_code() {
    const COLOR: [u8; 3] = [64, 200, 150];
    let size = (320, 180);
    let center = Vec2::new(100.0, 60.0);
    let ui = list(size, |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(
            disc,
            center,
            Vec2::splat(40.0),
            0.0,
            Paint::fill([COLOR[0], COLOR[1], COLOR[2], 255]),
        );
    });
    let mut harness = Harness::new(size);
    let pixels = harness.draw(Some(&ui));
    // Display-encoded in, the same code out: the shader decodes, the target encodes.
    let at_center = pixels.at(100, 60);
    assert_eq!(&at_center[..3], &COLOR, "an opaque color lands on its code");
    assert_eq!(at_center[3], 255);
    let (x0, y0, x1, y1) = pixels.bounds_of(COLOR, 1, (0, 0, 320, 180)).expect("the disc's pixels");
    // A disc 40 points across at a point a pixel, on (100, 60).
    assert!((x0 as i32 - 80).abs() <= 1 && (x1 as i32 - 119).abs() <= 1, "x {x0}..{x1}");
    assert!((y0 as i32 - 40).abs() <= 1 && (y1 as i32 - 79).abs() <= 1, "y {y0}..{y1}");
    // Its edge is antialiased across about a pixel: between the fill and the dark frame, somewhere.
    let edge: Vec<[u8; 4]> = (70..90).map(|x| pixels.at(x, 60)).collect();
    assert!(edge.iter().any(|p| p[1] > 4 && p[1] < COLOR[1] - 4), "no blended pixel along {edge:?}");
    // A pixel well away from it is untouched.
    assert_eq!(pixels.at(10, 170), harness.draw(None).at(10, 170));
}

#[test]
#[ignore = "needs a GPU"]
fn translucent_colors_blend_in_linear_light_as_the_module_docs_say() {
    let size = (64, 64);
    // Half-transparent white over black: linear 0.5, which encodes to code 188 (187 on lavapipe).
    let half = list(size, |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::splat(32.0), Vec2::splat(48.0), 0.0, Paint::fill([255, 255, 255, 128]));
    });
    let mut harness = Harness::new(size);
    let pixels = harness.draw(Some(&half));
    let code = pixels.at(32, 32)[0];
    assert!((185..=190).contains(&code), "{code}");
}

#[test]
#[ignore = "needs a GPU"]
fn a_bar_s_cut_follows_its_slant_and_its_corners_keep_their_shape() {
    const COLOR: [u8; 3] = [240, 120, 40];
    let size = (320, 60);
    let rect = Rect::new(Vec2::new(40.0, 20.0), Vec2::new(280.0, 40.0));
    let paint = Paint::fill([COLOR[0], COLOR[1], COLOR[2], 255]);
    let mut harness = Harness::new(size);

    // Half a bar, upright: it ends in the middle of its length, on a straight cut.
    let upright = list(size, |painter| painter.bar(rect, 0.0, (0.0, 0.5), paint));
    let pixels = harness.draw(Some(&upright));
    let (x0, y0, x1, y1) = pixels.bounds_of(COLOR, 2, (0, 0, 320, 60)).expect("the bar's pixels");
    assert!((x0 as i32 - 40).abs() <= 1, "{x0}");
    assert!((x1 as i32 - 159).abs() <= 1, "{x1}");
    assert!((y0 as i32 - 20).abs() <= 1 && (y1 as i32 - 39).abs() <= 1, "{y0}..{y1}");

    // Slanted, the cut leans with it: the top row ends further right than the bottom row.
    let slanted = list(size, |painter| painter.bar(rect, 0.3, (0.0, 0.5), paint));
    let pixels = harness.draw(Some(&slanted));
    let row_end =
        |y: u32| (0..320).rev().find(|&x| pixels.at(x, y)[0] > COLOR[0] - 4).expect("a row of the bar");
    let (top, bottom) = (row_end(22), row_end(37));
    let lean = top as f32 - bottom as f32;
    assert!((lean - 0.3 * 15.0).abs() <= 1.5, "the cut leans {lean} over 15 rows");

    // The whole bar reaches both ends, its corners rounded, not cut: the corner pixel stays dark.
    let whole = list(size, |painter| painter.bar(rect, 0.0, (0.0, 1.0), paint));
    let pixels = harness.draw(Some(&whole));
    let (x0, _, x1, _) = pixels.bounds_of(COLOR, 2, (0, 0, 320, 60)).expect("the bar's pixels");
    assert!((x0 as i32 - 40).abs() <= 1 && (x1 as i32 - 279).abs() <= 1, "{x0}..{x1}");
    assert!(pixels.at(40, 20)[0] < 200, "a rounded corner, not a square one: {:?}", pixels.at(40, 20));
}

#[test]
#[ignore = "needs a GPU"]
fn a_cooldown_s_sweep_covers_its_share_clockwise_from_the_top() {
    const COLOR: [u8; 3] = [255, 255, 255];
    let size = (128, 128);
    let ui = list(size, |painter| {
        let disc = painter.sprites().disc;
        let paint = Paint::fill([COLOR[0], COLOR[1], COLOR[2], 255]).swept(0.25);
        painter.sprite(disc, Vec2::splat(64.0), Vec2::splat(100.0), 0.0, paint);
    });
    let mut harness = Harness::new(size);
    let pixels = harness.draw(Some(&ui));
    let bright = |x: u32, y: u32| pixels.at(x, y)[0] > 200;
    assert!(bright(90, 40), "a quarter turn clockwise from the top: top right");
    assert!(!bright(90, 90), "not the bottom right");
    assert!(!bright(40, 90), "not the bottom left");
    assert!(!bright(40, 40), "not the top left");
}

/// The combat HUD, its fixed values, at `metrics`, with a thumb on each of the buttons `held`.
fn hud_list(metrics: UiMetrics, held: &[Button]) -> (HudLayout, UiDrawList) {
    let layout = HudLayout::new(metrics);
    let timestamp = sapphire_platform::Timestamp::from_nanos(1);
    let mut actions = TouchActions::default();
    for &button in held {
        actions.buttons[button.index()] = ButtonState {
            down: true,
            pressed_at: timestamp,
            over: true,
            pressed: true,
            ..ButtonState::default()
        };
    }
    let values = HudValues {
        player: FighterValues { hp: 0.5, poise: 0.5, hp_points: Some(1234) },
        enemy: Some(FighterValues { hp: 0.5, poise: 0.5, hp_points: None }),
        skill: SkillValues { cooldown: 0.0, seconds: None },
        ..HudValues::default()
    };
    let mut hud = CombatHud::new();
    hud.update(timestamp, &values, &actions);
    let mut list = UiDrawList::default();
    hud.draw(&layout, &actions, &mut list);
    (layout, list)
}

/// Pixel bounds, as `Pixels::bounds_of` gives them.
type Bounds = (u32, u32, u32, u32);

/// Where the mint health fill's pixels are, and the attack button's white sword's, as pixel bounds.
fn find_widgets(pixels: &Pixels, layout: &HudLayout, pixels_per_point: f32) -> (Bounds, Bounds) {
    let (width, height) = pixels.size;
    // The player's health fill is mint and only the player's is: within the top-left of the frame.
    let health =
        pixels.bounds_of([96, 243, 212], 6, (0, 0, width, height)).expect("the health fill's pixels");
    // The button's glyph is the frame's whitest thing near where the layout says.
    let attack = layout.attack.center * pixels_per_point;
    let half = layout.attack.half * pixels_per_point;
    let area = (
        (attack.x - half).max(0.0) as u32,
        (attack.y - half).max(0.0) as u32,
        (attack.x + half) as u32,
        (attack.y + half) as u32,
    );
    let sword = pixels.bounds_of([255, 255, 255], 3, area).expect("the attack's glyph");
    (health, sword)
}

#[test]
#[ignore = "needs a GPU"]
fn the_hud_s_pixels_follow_their_anchors_at_three_aspect_ratios_and_inside_insets() {
    const NOTCH: Insets = Insets { left: 62.0, top: 0.0, right: 62.0, bottom: 21.0 };
    let frames: [(&str, (u32, u32), Insets); 5] = [
        ("4:3", (480, 360), Insets { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 }),
        ("16:9", (640, 360), Insets { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 }),
        ("21:9", (840, 360), Insets { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 }),
        ("16:9 with a notch", (640, 360), NOTCH),
        ("21:9 with a notch", (840, 360), NOTCH),
    ];
    let mut failures = String::new();
    for (name, size, insets) in frames {
        let metrics = UiMetrics::of_pixels(size, 1.0, insets);
        let (layout, ui) = hud_list(metrics, &[]);
        let mut harness = Harness::new(size);
        let bare = harness.draw(None);
        let pixels = harness.draw(Some(&ui));
        let (health, sword) = find_widgets(&pixels, &layout, 1.0);

        // The health bar's fill is on the top left of the safe area, half its length.
        let bar = layout.player.hp;
        let lean = bar.height() * 0.5 * sapphire_ui::hud::SLANT + 2.0;
        if (health.0 as f32) < bar.min.x - lean || (health.0 as f32) > bar.min.x + lean {
            failures += &format!("{name}: the health fill starts at {} where its bar is {bar:?}\n", health.0);
        }
        let half_end = bar.min.x + bar.width() * 0.5;
        if (health.2 as f32 - half_end).abs() > lean + 2.0 {
            failures += &format!("{name}: the health fill ends at {} not about {half_end}\n", health.2);
        }
        if !insets_clear(insets, size, health) {
            failures += &format!("{name}: the health fill is under the insets\n");
        }

        // The attack's glyph sits on its button, on the safe area's bottom right.
        let middle = Vec2::new((sword.0 + sword.2) as f32, (sword.1 + sword.3) as f32) * 0.5;
        if middle.distance(layout.attack.center) > 6.0 {
            failures += &format!("{name}: the glyph's middle {middle} is off {}\n", layout.attack.center);
        }
        if !insets_clear(insets, size, sword) {
            failures += &format!("{name}: the glyph is under the insets\n");
        }

        // Nothing the HUD draws reaches the notch or the home indicator: those bands stay exactly
        // as the scene left them, along the whole height and width.
        let bands = [
            (0, 0, insets.left as u32, size.1),
            (size.0 - insets.right as u32, 0, size.0, size.1),
            (0, size.1 - insets.bottom as u32, size.0, size.1),
        ];
        for (x0, y0, x1, y1) in bands {
            for y in y0..y1 {
                for x in x0..x1 {
                    if pixels.at(x, y) != bare.at(x, y) {
                        failures += &format!("{name}: pixel ({x}, {y}) is under the insets\n");
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{failures}");
}

/// Whether `bounds` keeps out of the frame's `insets`.
fn insets_clear(insets: Insets, size: (u32, u32), bounds: (u32, u32, u32, u32)) -> bool {
    (bounds.0 as f32) >= insets.left
        && (bounds.1 as f32) >= insets.top
        && (bounds.2 as f32) < size.0 as f32 - insets.right
        && (bounds.3 as f32) < size.1 as f32 - insets.bottom
}

#[test]
#[ignore = "needs a GPU"]
fn a_list_in_points_draws_the_same_at_any_pixel_density() {
    // The same frame at one pixel a point and at three: the larger, sampled every third pixel,
    // agrees with the smaller away from edges.
    let metrics = UiMetrics::new(Vec2::new(320.0, 180.0), Insets::default());
    let (_, ui) = hud_list(metrics, &[Button::Attack]);
    let small = Harness::new((320, 180)).draw(Some(&ui));
    let large = Harness::new((960, 540)).draw(Some(&ui));
    let mut compared = 0;
    let mut different = 0;
    for y in 2..178 {
        for x in 2..318 {
            let flat = (0..3).all(|i| {
                let center = i32::from(small.at(x, y)[i]);
                [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .iter()
                    .all(|&(nx, ny)| (i32::from(small.at(nx, ny)[i]) - center).abs() <= 2)
            });
            if !flat {
                continue;
            }
            compared += 1;
            let there = large.at(x * 3 + 1, y * 3 + 1);
            let here = small.at(x, y);
            if (0..3).any(|i| (i32::from(there[i]) - i32::from(here[i])).abs() > 10) {
                different += 1;
            }
        }
    }
    assert!(compared > 40_000, "{compared} flat pixels");
    assert!(different * 500 < compared, "{different} of {compared} differ");
}

#[test]
#[ignore = "needs a GPU"]
fn a_new_renderer_uploads_the_atlas_it_is_handed_and_another_atlas_replaces_it() {
    let size = (64, 64);
    let ui = list(size, |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::splat(32.0), Vec2::splat(40.0), 0.0, Paint::fill([255; 4]));
    });
    let mut harness = Harness::new(size);
    assert_eq!(harness.draw(Some(&ui)).at(32, 32)[0], 255);
    assert_eq!(harness.renderer.stats().ui.atlas_uploads, 1);

    // A second list naming the same atlas uploads nothing. Another atlas does, whose whole cell is
    // outside its shape: the same quads now draw nothing.
    let mut same = ui.clone();
    same.atlas = ui.atlas.clone();
    harness.draw(Some(&same));
    assert_eq!(harness.renderer.stats().ui.atlas_uploads, 1);
    let source = ui.atlas.as_ref().unwrap();
    let blank = UiAtlas { texels: vec![0; source.texels.len()], ..(**source).clone() };
    let mut replaced = ui.clone();
    replaced.atlas = Some(Arc::new(blank));
    assert_eq!(harness.draw(Some(&replaced)).at(32, 32)[0], 0);
    assert_eq!(harness.renderer.stats().ui.atlas_uploads, 2);

    // An atlas whose size lies about its bytes draws nothing, with no validation error.
    let mut broken = ui.clone();
    broken.atlas =
        Some(Arc::new(UiAtlas { width: 8, height: 8, texels: vec![255; 3], ..(**source).clone() }));
    harness.draw(Some(&broken));
    assert_eq!(harness.renderer.stats().ui.quads, 0);

    // A renderer of its own, as after device loss or a new tier, brings it along from the list.
    let mut again = Harness::new(size);
    assert_eq!(again.draw(Some(&ui)).at(32, 32)[0], 255);
}

#[test]
#[ignore = "needs a GPU"]
fn an_edit_to_its_shader_rebuilds_its_one_pipeline_once_it_is_built() {
    let size = (64, 64);
    let ui = list(size, |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::splat(32.0), Vec2::splat(40.0), 0.0, Paint::fill([255; 4]));
    });
    let mut harness = Harness::new(size);
    let mut sources = ShaderSources::compiled_in();
    let original = sources.get(ShaderFile::Ui).to_owned();
    // A shader that draws everything half as bright.
    let dimmed = original.replace(
        "let shape = vec4<f32>(rgb, fill + line);",
        "let shape = vec4<f32>(rgb * 0.25, fill + line);",
    );
    assert_ne!(dimmed, original);
    sources.set(ShaderFile::Ui, dimmed.clone());

    // Nothing built yet: the program changes and nothing rebuilds.
    let update = harness.renderer.set_shader_sources(&sources).expect("it validates");
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 0 });
    assert_eq!(harness.renderer.stats().ui.pipelines, 0);

    // Built on first draw from the sources in use.
    let dim = harness.draw(Some(&ui)).at(32, 32)[0];
    assert!((130..=142).contains(&dim), "linear 0.25 encodes to about 137: {dim}");

    // Once built, an edit rebuilds it alone.
    let update = harness.renderer.set_shader_sources(&ShaderSources::compiled_in()).expect("it validates");
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 1 });
    assert_eq!(harness.draw(Some(&ui)).at(32, 32)[0], 255);
}

#[test]
#[ignore = "needs a GPU"]
fn a_quad_list_grows_past_the_buffer_s_first_size() {
    let size = (64, 64);
    let atlas = Atlas::paint();
    let mut quads = Vec::new();
    let mut painter = Painter::new(&mut quads, &atlas.sprites);
    let disc = painter.sprites().disc;
    painter.sprite(disc, Vec2::splat(20.0), Vec2::splat(20.0), 0.0, Paint::fill([0, 0, 0, 255]));
    painter.sprite(disc, Vec2::splat(44.0), Vec2::splat(20.0), 0.0, Paint::fill([255; 4]));
    let (first, last) = (quads[0], quads[1]);
    // Past the 256 the buffer starts with: the last quad in the list is the one that shows.
    let mut quads: Vec<UiQuad> = vec![first; 1999];
    quads.push(last);
    let ui = UiDrawList { viewport: [64.0, 64.0], atlas: Some(atlas.image), quads, ..UiDrawList::default() };
    let mut harness = Harness::new(size);
    assert_eq!(harness.draw(Some(&ui)).at(44, 44)[0], 255);
    assert_eq!(harness.renderer.stats().ui.quads, 2000);
}

/// The pass order: a developer's lines mark things the HUD may cover, so debug draw stays over it, as
/// it stays over everything but the app's overlay.
#[test]
#[ignore = "needs a GPU"]
fn debug_draw_s_lines_stay_visible_over_the_interface() {
    const HUD: [u8; 4] = [20, 200, 120, 255];
    const LINE: [u8; 3] = [255, 40, 200];
    // Odd, so a row's middle falls on the line, whose 1.5 pixels and soft edge would leave two rows blended.
    let size = (129, 129);
    let atlas = Atlas::paint();
    let mut quads = Vec::new();
    let mut painter = Painter::new(&mut quads, &atlas.sprites);
    let disc = painter.sprites().disc;
    painter.sprite(disc, Vec2::splat(64.5), Vec2::splat(100.0), 0.0, Paint::fill(HUD));
    let ui =
        UiDrawList { viewport: [129.0, 129.0], atlas: Some(atlas.image), quads, ..UiDrawList::default() };
    // Across the frame's middle row, in front of nothing, tested against nothing.
    let line = DebugLine {
        from: Vec3::new(-2.0, 0.0, 0.0),
        to: Vec3::new(2.0, 0.0, 0.0),
        color: Color::rgb(LINE[0], LINE[1], LINE[2]),
        depth: Depth::OnTop,
    };
    let mut harness = Harness::new(size);
    let pixels = harness.draw_with_lines(Some(&ui), &[line]);
    assert_eq!(harness.renderer.stats().debug_draw.drawn, 1);
    assert_eq!(pixels.at(64, 30), HUD, "the disc away from the line");
    let (x0, y0, x1, y1) = pixels.bounds_of(LINE, 8, (40, 40, 88, 88)).expect("the line over the disc");
    assert!(x0 <= 41 && x1 >= 86, "across the disc: {x0}..{x1}");
    assert!(y0 >= 60 && y1 <= 67, "on its row: {y0}..{y1}");
}

// --- Glyph pages (M3 U1) ---

/// `size` texels of `value` as a page upload at `corner`, appended to `list`.
fn upload_block(list: &mut UiDrawList, page: u16, corner: (u16, u16), size: (u16, u16), value: u8) {
    let offset = list.upload_texels.len() as u32;
    list.upload_texels.resize(list.upload_texels.len() + usize::from(size.0) * usize::from(size.1), value);
    list.page_uploads.push(UiPageUpload {
        page,
        x: corner.0,
        y: corner.1,
        width: size.0,
        height: size.1,
        offset,
    });
}

/// A quad over the page's rectangle at `corner`, pixel for texel.
fn page_quad(side: f32, corner: (f32, f32), at: (f32, f32), size: f32, color: [u8; 4]) -> UiQuad {
    let uv = [corner.0 / side, corner.1 / side, (corner.0 + size) / side, (corner.1 + size) / side];
    UiQuad::glyph([at.0, at.1], [size, size], uv, color)
}

/// A flat rectangle, min and max in points.
fn solid(painter: &mut Painter<'_>, min: [f32; 2], max: [f32; 2], color: [u8; 4]) {
    let rect = Rect::new(Vec2::from(min), Vec2::from(max));
    painter.bar(rect, 0.0, (0.0, 1.0), Paint::fill(color));
}

/// The shader's half of a gradient: the share of the far color follows the quad's plane, in linear light,
/// and all zeros leaves a fill as it was.
#[test]
#[ignore = "needs a GPU"]
fn a_gradient_mixes_along_its_plane_in_linear_light() {
    let mut harness = Harness::new((100, 40));
    let (red, blue) = ([255, 0, 0, 255], [0, 0, 255, 255]);
    let mut ui = list((100, 40), |painter| {
        solid(painter, [0.0, 0.0], [100.0, 20.0], red);
        solid(painter, [0.0, 20.0], [100.0, 40.0], red);
    });
    ui.quads[0].color_end = blue;
    ui.quads[0].gradient = [1.0, 0.0, 0.0];
    let pixels = harness.draw(Some(&ui));

    let (left, middle, right) = (pixels.at(10, 10), pixels.at(50, 10), pixels.at(90, 10));
    assert!(left[0] > right[0] && left[2] < right[2], "it runs toward the far color: {left:?} {right:?}");
    // Half of each in linear light is about 188 in sRGB, where a mix of the codes would be 128.
    for channel in [middle[0], middle[2]] {
        assert!((180..=196).contains(&channel), "{middle:?}");
    }
    assert_eq!(pixels.at(50, 30)[..3], [255, 0, 0], "no plane, no gradient");
}

/// Masks cut what a quad draws at their rectangle, in the list's order, and in the target's pixels when
/// those aren't points. `None` lets everything draw again.
#[test]
#[ignore = "needs a GPU"]
fn a_mask_cuts_quads_at_its_rectangle_and_ends_when_the_list_says() {
    let mut harness = Harness::new((200, 100));
    let mut ui = list((200, 100), |_| {});
    ui.viewport = [100.0, 50.0];
    let atlas = Atlas::paint();
    let add = |ui: &mut UiDrawList, min, max, color| {
        solid(&mut Painter::new(&mut ui.quads, &atlas.sprites), min, max, color);
    };
    ui.set_mask(Some([10.0, 10.0, 30.0, 30.0]));
    add(&mut ui, [0.0, 0.0], [50.0, 50.0], [255, 0, 0, 255]);
    ui.set_mask(None);
    add(&mut ui, [60.0, 0.0], [80.0, 50.0], [0, 255, 0, 255]);
    let pixels = harness.draw(Some(&ui));

    assert_eq!(pixels.at(40, 40)[..3], [255, 0, 0], "inside the mask, at two pixels a point");
    assert_eq!(pixels.at(20, 20)[..3], [255, 0, 0], "its first pixel");
    assert_eq!(pixels.at(19, 40)[..2], [0, 0], "left of it");
    assert_eq!(pixels.at(60, 40)[..2], [0, 0], "right of it, where the quad went on");
    assert_eq!(pixels.at(40, 60)[..2], [0, 0], "below it");
    assert_eq!(pixels.at(140, 50)[..3], [0, 255, 0], "the next quad has no mask");
    assert_eq!(harness.renderer.stats().ui.draws, 2);
}

#[test]
#[ignore = "needs a GPU"]
fn a_sparse_glyph_page_allocates_only_the_page_named() {
    let mut harness = Harness::new((32, 32));
    let mut ui = UiDrawList {
        viewport: [32.0, 32.0],
        page_size: 16,
        page_state: harness.renderer.ui_page_state(),
        ..UiDrawList::default()
    };
    upload_block(&mut ui, u16::MAX, (0, 0), (16, 16), 255);
    ui.push_page_quad(u16::MAX, page_quad(16.0, (0.0, 0.0), (8.0, 8.0), 16.0, [255; 4]));
    assert_eq!(harness.draw(Some(&ui)).at(16, 16), [255; 4]);
    assert_eq!(harness.renderer.stats().ui.pages, 1);
}

/// A display face's field (M3 U8): a 16 × 16 multi-channel atlas whose left half's channels agree
/// two to one on inside, and whose right half's agree two to one on outside, though every texel's red
/// says inside. Only the median of three tells the halves apart.
fn split_field() -> Arc<UiAtlas> {
    let mut texels = Vec::with_capacity(16 * 16 * 4);
    for _ in 0..16 {
        for x in 0..16 {
            texels.extend_from_slice(if x < 8 { &[255, 255, 0, 255] } else { &[255, 0, 0, 0] });
        }
    }
    Arc::new(UiAtlas { width: 16, height: 16, spread: 4.0, format: UiFieldFormat::Multi, texels })
}

/// Field quads draw through the interface's one pipeline, so a renderer that declared it builds nothing
/// for them; the field goes up once, however many frames name it, and comes down once nothing does.
#[test]
#[ignore = "needs a GPU"]
fn a_field_draws_by_its_median_through_the_one_pipeline_and_goes_up_once() {
    let mut harness = Harness::new((32, 32));
    let inventory = PipelineInventory { ui: true, ..PipelineInventory::default() };
    assert_eq!(harness.renderer.prewarm(&inventory), 1);
    harness.draw(None);
    let builds = harness.renderer.stats().pipelines.first_use_builds;
    let alive = pipelines(&harness.gpu);

    let field = split_field();
    let mut ui = UiDrawList { viewport: [32.0, 32.0], ..UiDrawList::default() };
    ui.push_field_quad(&field, UiQuad::field_glyph([8.0, 8.0], [16.0, 16.0], [0.0, 0.0, 1.0, 1.0], [255; 4]));
    let pixels = harness.draw(Some(&ui));
    assert_eq!(pixels.at(10, 16), [255; 4], "the left half's median is inside");
    assert_eq!(pixels.at(21, 16)[..3], [0; 3], "the right half's is outside, though its red isn't");
    assert_eq!(pixels.at(4, 16)[..3], [0; 3], "nothing outside the quad");
    let stats = harness.renderer.stats();
    assert_eq!(stats.pipelines.first_use_builds, builds, "no first use");
    assert_eq!(pipelines(&harness.gpu), alive, "no pipeline of its own");
    assert_eq!((stats.ui.fields, stats.ui.field_uploads, stats.ui.field_bytes), (1, 1, 16 * 16 * 4));

    harness.draw(Some(&ui));
    assert_eq!(harness.renderer.stats().ui.field_uploads, 1, "a field named again isn't uploaded again");

    drop((ui, field));
    for _ in 0..4 {
        harness.draw(None);
    }
    assert_eq!(harness.renderer.stats().ui.fields, 0, "what nothing holds comes down");
}

#[test]
#[ignore = "needs a GPU"]
fn an_invalid_glyph_batch_is_not_acknowledged_and_can_be_retried() {
    let mut harness = Harness::new((32, 32));
    let initial = harness.renderer.ui_page_state();
    let mut ui = UiDrawList {
        viewport: [32.0, 32.0],
        page_size: 16,
        page_state: sapphire_look::UiPageState { batch: initial.batch + 1, ..initial },
        ..UiDrawList::default()
    };
    upload_block(&mut ui, 0, (0, 0), (16, 16), 255);
    upload_block(&mut ui, 1, (0, 0), (16, 16), 255);
    ui.push_page_quad(0, page_quad(16.0, (0.0, 0.0), (8.0, 8.0), 16.0, [255; 4]));
    let last = ui.upload_texels.pop().expect("two complete rectangles");
    assert_eq!(harness.draw(Some(&ui)).at(16, 16)[..3], [0; 3]);
    assert_eq!(harness.renderer.ui_page_state(), initial);
    assert_eq!(harness.renderer.stats().ui.pages, 0, "no partial page allocation");
    ui.upload_texels.push(last);
    assert_eq!(harness.draw(Some(&ui)).at(16, 16), [255; 4]);
    assert_eq!(harness.renderer.ui_page_state(), ui.page_state);
    assert_eq!(harness.renderer.stats().ui.pages, 2);
}

#[test]
#[ignore = "needs a GPU"]
fn glyph_only_lists_do_not_draw_field_quads_from_an_earlier_atlas() {
    let mut harness = Harness::new((64, 32));
    let mut ui = list((64, 32), |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::new(16.0, 16.0), Vec2::splat(16.0), 0.0, Paint::fill([255, 0, 0, 255]));
    });
    assert_eq!(harness.draw(Some(&ui)).at(16, 16)[..3], [255, 0, 0]);
    ui.atlas = None;
    ui.page_size = 16;
    ui.page_state = harness.renderer.ui_page_state();
    upload_block(&mut ui, 0, (0, 0), (16, 16), 255);
    ui.push_page_quad(0, page_quad(16.0, (0.0, 0.0), (40.0, 8.0), 16.0, [255; 4]));
    let pixels = harness.draw(Some(&ui));
    assert_eq!(pixels.at(16, 16)[..3], [0; 3]);
    assert_eq!(pixels.at(48, 16), [255; 4]);
    assert_eq!(harness.renderer.stats().ui.draws, 1);
}

#[test]
#[ignore = "needs a GPU"]
fn glyph_pages_fill_from_the_list_s_uploads_and_draw_in_order_with_the_atlas() {
    const SIDE: u32 = 64;
    let mut harness = Harness::new((96, 32));
    let before = pipelines(&harness.gpu);
    let white = [255, 255, 255, 255];

    // Coverage from two pages, full and half, with a field quad between them in the list's order.
    let mut ui = list((96, 32), |painter| {
        let disc = painter.sprites().disc;
        painter.sprite(disc, Vec2::new(48.0, 16.0), Vec2::splat(20.0), 0.0, Paint::fill([255, 0, 0, 255]));
    });
    ui.page_size = SIDE;
    ui.page_state = harness.renderer.ui_page_state();
    upload_block(&mut ui, 0, (4, 4), (16, 16), 255);
    upload_block(&mut ui, 1, (8, 8), (16, 16), 128);
    let side = SIDE as f32;
    let before_disc = page_quad(side, (4.0, 4.0), (8.0, 8.0), 16.0, white);
    let after_disc = page_quad(side, (8.0, 8.0), (72.0, 8.0), 16.0, white);
    let disc = ui.quads.pop().expect("the disc");
    ui.push_page_quad(0, before_disc);
    ui.quads.push(disc);
    ui.push_page_quad(1, after_disc);

    let pixels = harness.draw(Some(&ui));
    let stats = harness.renderer.stats().ui;
    assert_eq!(pipelines(&harness.gpu), before + 1, "pages share the interface's one pipeline");
    assert_eq!((stats.pages, stats.page_uploads, stats.draws, stats.quads), (2, 2, 3, 3));
    assert_eq!(stats.page_upload_bytes, 2 * 16 * 16);
    assert_eq!(pixels.at(16, 16), [255, 255, 255, 255], "full coverage is the color's exact code");
    assert_eq!(pixels.at(4, 16)[..3], [0, 0, 0], "outside the quad, nothing");
    assert_eq!(pixels.at(48, 16)[..3], [255, 0, 0], "the field quad between them still draws");
    // 128 of coverage in linear light, through the target's encode.
    let half = pixels.at(80, 16);
    assert!((i32::from(half[0]) - 188).abs() <= 1, "half coverage blends in linear light: {half:?}");

    // The next frame names the pages and uploads nothing: they kept their texels.
    ui.page_uploads.clear();
    ui.upload_texels.clear();
    let pixels = harness.draw(Some(&ui));
    let stats = harness.renderer.stats().ui;
    assert_eq!((stats.page_uploads, stats.page_upload_bytes), (0, 0));
    assert_eq!(pixels.at(16, 16), [255, 255, 255, 255]);

    // Writing into a page changes only that rectangle.
    let mut update = ui.clone();
    upload_block(&mut update, 0, (4, 4), (8, 16), 0);
    let pixels = harness.draw(Some(&update));
    assert_eq!(pixels.at(10, 16)[..3], [0, 0, 0], "the rectangle written");
    assert_eq!(pixels.at(20, 16), [255, 255, 255, 255], "beside it, as it was");

    // A page's uploads with no quads aren't lost.
    let mut quiet = UiDrawList {
        viewport: [96.0, 32.0],
        page_size: SIDE,
        page_state: harness.renderer.ui_page_state(),
        ..UiDrawList::default()
    };
    upload_block(&mut quiet, 0, (4, 4), (16, 16), 255);
    harness.draw(Some(&quiet));
    assert_eq!(harness.renderer.stats().ui.page_uploads, 1);
    assert!(!harness.drew_ui_pass(), "and no pass for nothing to draw");
    let pixels = harness.draw(Some(&ui));
    assert_eq!(pixels.at(10, 16), [255, 255, 255, 255], "the upload landed all the same");
}

/// The real stack through the real pass: Japanese from `parley` and `swash`, at the scales a phone
/// and a desktop use, every pixel the exact coverage the atlas made, so no filter touched it. Set
/// `SAPPHIRE_TEST_CAPTURE_DIR` to write each frame as a PNG to look at.
#[test]
#[ignore = "needs a GPU"]
fn japanese_lands_pixel_for_pixel_at_every_scale() {
    use sapphire_text::{AtlasConfig, GlyphAtlas, TextStyle, TextSystem};

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party");
    let font = |name: &str| Arc::new(std::fs::read(dir.join(name)).expect("the font"));
    let mut text = TextSystem::new();
    text.register_font(font("mplus1/MPLUS1[wght].ttf"));
    text.register_font(font("oxanium/Oxanium[wght].ttf"));
    text.register_font(font("dela_gothic_one/DelaGothicOne-Regular.ttf"));
    let style =
        |families, size, weight| TextStyle { families, size, weight, line_height: 1.3, locale: Some("ja") };
    let lines = [
        ("設定と記録", style("'Dela Gothic One', 'M PLUS 1'", 32.0, 400.0)),
        ("必殺技ゲージ Level 90", style("Oxanium, 'M PLUS 1'", 20.0, 600.0)),
        ("「まだ船は戻らないのか」と老人は海を見つめたまま尋ねた。", style("'M PLUS 1'", 14.0, 400.0)),
        ("攻撃力 1,284　防御力 960　会心率 42%", style("'M PLUS 1'", 12.0, 500.0)),
    ];
    let capture = std::env::var_os("SAPPHIRE_TEST_CAPTURE_DIR").map(std::path::PathBuf::from);
    for scale in [1.0f32, 1.5, 2.0, 3.0] {
        let points = (440.0, 130.0);
        let size = ((points.0 * scale).ceil() as u32, (points.1 * scale).ceil() as u32);
        let mut harness = Harness::new(size);
        let mut atlas = GlyphAtlas::new(AtlasConfig::default());
        let mut ui =
            UiDrawList { viewport: [size.0 as f32 / scale, size.1 as f32 / scale], ..UiDrawList::default() };
        let mut y = 4.0;
        atlas.begin_frame(harness.renderer.ui_page_state());
        for (string, style) in &lines {
            let shaped = text.shape(string, style, scale, Some(points.0 - 16.0));
            atlas.draw(shaped, [8.0, y], scale, [255; 4], &mut ui);
            y += shaped.height / scale;
        }
        let stats = atlas.stats();
        assert_eq!((stats.dropped, stats.pages), (0, 1));
        let pixels = harness.draw(Some(&ui));
        let renderer = harness.renderer.stats().ui;
        assert_eq!((renderer.pages, renderer.draws, renderer.quads as usize), (1, 1, ui.quads.len()));

        // Rebuild the page on the CPU, and each quad from it: white on black, each pixel must be its
        // texel's coverage, encoded. Quads don't overlap in these lines' cells beyond a stray corner,
        // so a pixel with two quads over it is skipped.
        let side = ui.page_size as usize;
        let mut page = vec![0u8; side * side];
        for upload in &ui.page_uploads {
            let (w, h) = (usize::from(upload.width), usize::from(upload.height));
            for row in 0..h {
                let from = upload.offset as usize + row * w;
                let to = (usize::from(upload.y) + row) * side + usize::from(upload.x);
                page[to..to + w].copy_from_slice(&ui.upload_texels[from..from + w]);
            }
        }
        let mut covered = vec![0u8; (size.0 * size.1) as usize];
        let mut expected = vec![0u8; (size.0 * size.1) as usize];
        for quad in &ui.quads {
            let (x0, y0) = (quad.origin[0] * scale, quad.origin[1] * scale);
            let (w, h) = (quad.axis_x[0] * scale, quad.axis_y[1] * scale);
            assert!(
                (x0 - x0.round()).abs() < 1e-3 && (y0 - y0.round()).abs() < 1e-3,
                "on whole pixels: {x0}, {y0}"
            );
            let (u0, v0) = (quad.uv[0] * side as f32, quad.uv[1] * side as f32);
            for row in 0..h.round() as usize {
                for column in 0..w.round() as usize {
                    let at = (y0.round() as usize + row) * size.0 as usize + x0.round() as usize + column;
                    covered[at] = covered[at].saturating_add(1);
                    expected[at] = page[(v0.round() as usize + row) * side + u0.round() as usize + column];
                }
            }
        }
        let encode = |value: f32| {
            let value =
                if value <= 0.003_130_8 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
            value * 255.0
        };
        let decode = |code: f32| {
            let value = code / 255.0;
            let value = if value <= 0.040_45 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) };
            value * 255.0
        };
        let (mut checked, mut worst, mut inked, mut full) = (0u32, 0f32, 0u32, 0u32);
        for y in 0..size.1 {
            for x in 0..size.0 {
                let at = (y * size.0 + x) as usize;
                if covered[at] != 1 {
                    continue;
                }
                let want = encode(f32::from(expected[at]) / 255.0);
                let got = f32::from(pixels.at(x, y)[0]);
                // Near black one step of coverage is 12 encoded codes, which is what WARP reads
                // beside a full texel. A filter shows in both spaces, so a pixel must miss in both.
                let linear = (f32::from(expected[at]) - decode(got)).abs();
                worst = worst.max((want - got).abs().min(linear));
                checked += 1;
                inked += u32::from(expected[at] > 0);
                full += u32::from(expected[at] == 255);
            }
        }
        eprintln!(
            "{scale}x: {} × {} px, {} glyphs, {} of page area, {checked} pixels checked, {inked} with ink, {full} at full coverage, worst difference {worst:.2} codes",
            size.0, size.1, stats.rasterized, stats.used_bytes,
        );
        assert!(worst <= 1.5, "{scale}x: a filter touched the glyphs: {worst} codes");
        assert!(inked > 1000 && full > 100, "{scale}x: ink {inked}, full {full}");
        if let Some(out) = &capture {
            let file = std::fs::File::create(out.join(format!("japanese-{scale}x.png"))).expect("a file");
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size.0, size.1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().expect("a header").write_image_data(&pixels.rgba).expect("pixels");
        }
    }
}

/// What a language switch asks of the queue: `cargo test -p sapphire_render --test gpu --release
/// glyph_uploads_cost -- --ignored --nocapture`. It holds nothing.
#[test]
#[ignore = "needs a GPU"]
#[allow(clippy::disallowed_methods, reason = "timing needs the OS clock, and nothing here ships")]
#[allow(clippy::print_stdout, reason = "printing the numbers is this test's whole job")]
fn glyph_uploads_cost() {
    const SIDE: u32 = 2048;
    let mut harness = Harness::new((64, 64));
    for (glyphs, side) in [(0u16, 24u16), (115, 24), (308, 24), (308, 48), (1500, 24), (3400, 24)] {
        let mut times = Vec::new();
        for _ in 0..12 {
            let mut ui = UiDrawList {
                viewport: [64.0, 64.0],
                page_size: SIDE,
                page_state: harness.renderer.ui_page_state(),
                ..UiDrawList::default()
            };
            for index in 0..glyphs {
                let per_row = SIDE as u16 / side;
                let corner = ((index % per_row) * side, (index / per_row) * side);
                upload_block(&mut ui, 0, corner, (side, side), 200);
            }
            ui.push_page_quad(0, page_quad(SIDE as f32, (0.0, 0.0), (0.0, 0.0), 16.0, [255; 4]));
            dark_packet(&mut harness.ring, harness.size, Some(&ui));
            let start = std::time::Instant::now();
            harness.renderer.prepare(&harness.gpu, harness.ring.current());
            times.push(start.elapsed().as_secs_f64() * 1e3);
            // A whole frame, so the writes reach the GPU and the next prepare starts clean.
            harness.draw(Some(&UiDrawList::default()));
        }
        times.sort_by(f64::total_cmp);
        println!(
            "prepare with {glyphs} rectangles of {side}²: p50 {:.2} ms, worst {:.2} ms",
            times[times.len() / 2],
            times[times.len() - 1],
        );
    }
}

/// Two lines of text as a frame of `harness`'s draws them, from `atlas`.
fn text_frame(
    harness: &Harness,
    text: &mut sapphire_text::TextSystem,
    atlas: &mut sapphire_text::GlyphAtlas,
    string: &str,
) -> UiDrawList {
    let style = sapphire_text::TextStyle {
        families: "Ubuntu",
        size: 20.0,
        weight: 400.0,
        line_height: 1.3,
        locale: Some("en"),
    };
    let mut ui =
        UiDrawList { viewport: [harness.size.0 as f32, harness.size.1 as f32], ..UiDrawList::default() };
    atlas.begin_frame(harness.renderer.ui_page_state());
    atlas.draw(text.shape(string, &style, 1.0, None), [8.0, 8.0], 1.0, [255; 4], &mut ui);
    ui
}

/// Fonts every checkout has, and an atlas of a size that keeps a read-back small.
fn text_stack() -> (sapphire_text::TextSystem, sapphire_text::GlyphAtlas) {
    let mut text = sapphire_text::TextSystem::new();
    text.register_font(Arc::new(epaint_default_fonts::UBUNTU_LIGHT.to_vec()));
    let config = sapphire_text::AtlasConfig { page_size: 512, ..sapphire_text::AtlasConfig::default() };
    (text, sapphire_text::GlyphAtlas::new(config))
}

fn lit_pixels(pixels: &Pixels) -> usize {
    pixels.rgba.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 128).count()
}

/// The way a renderer used to draw holes: it starts with no pages, and nothing told the atlas.
#[test]
#[ignore = "needs a GPU"]
fn a_new_renderer_gets_the_glyphs_its_pages_lack() {
    let (mut text, mut atlas) = text_stack();
    let mut first = Harness::new((160, 48));
    let ui = text_frame(&first, &mut text, &mut atlas, "Attack");
    let drawn = first.draw(Some(&ui));
    assert!(lit_pixels(&drawn) > 100, "the test must draw something");
    let held = text_frame(&first, &mut text, &mut atlas, "Attack");
    assert!(held.page_uploads.is_empty());
    assert_eq!(first.draw(Some(&held)).rgba, drawn.rgba);

    // A new renderer, as a tier change or a recreated device makes.
    let mut second = Harness::new((160, 48));
    assert_ne!(second.renderer.ui_page_state().store, first.renderer.ui_page_state().store);
    let ui = text_frame(&second, &mut text, &mut atlas, "Attack");
    assert!(ui.glyphs.forgot);
    assert!(!ui.page_uploads.is_empty(), "the atlas sends it all again");
    assert_eq!(second.draw(Some(&ui)).rgba, drawn.rgba);
    assert_eq!(second.renderer.stats().ui.stale_page_quads, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn a_device_lost_on_a_frame_of_new_glyphs_costs_nothing_on_the_next_renderer() {
    let (mut text, mut atlas) = text_stack();
    let mut first = Harness::new((160, 48));
    let ui = text_frame(&first, &mut text, &mut atlas, "Attack");
    let drawn = first.draw(Some(&ui));

    first.gpu.device().destroy();
    let _ = first.gpu.device().poll(wgpu::PollType::wait_indefinitely());
    assert!(first.gpu.is_lost());
    let ui = text_frame(&first, &mut text, &mut atlas, "Defend");
    dark_packet(&mut first.ring, first.size, Some(&ui));
    first.renderer.prepare(&first.gpu, first.ring.current());
    drop(first);

    let mut second = Harness::new((160, 48));
    let ui = text_frame(&second, &mut text, &mut atlas, "Attack");
    assert_eq!(second.draw(Some(&ui)).rgba, drawn.rgba);
}

/// A frame the app built and never prepared took its uploads with it, and the pages don't have them.
#[test]
#[ignore = "needs a GPU"]
fn a_list_that_never_reached_the_renderer_is_sent_again() {
    let (mut text, mut atlas) = text_stack();
    let mut harness = Harness::new((160, 48));
    let mut other = Harness::new((160, 48));
    let ui = text_frame(&other, &mut text, &mut atlas, "Attack");
    let expected = other.draw(Some(&ui));
    let (mut text, mut atlas) = text_stack();

    let lost = text_frame(&harness, &mut text, &mut atlas, "Attack");
    assert!(!lost.page_uploads.is_empty());
    let ui = text_frame(&harness, &mut text, &mut atlas, "Attack");
    assert!(ui.glyphs.forgot);
    assert_eq!(harness.draw(Some(&ui)).rgba, expected.rgba);
}

/// A prepared frame that never runs still took its uploads, so nothing starts over.
#[test]
#[ignore = "needs a GPU"]
fn a_frame_prepared_and_never_recorded_loses_no_glyphs() {
    let (mut text, mut atlas) = text_stack();
    let mut harness = Harness::new((160, 48));
    let ui = text_frame(&harness, &mut text, &mut atlas, "Attack");
    dark_packet(&mut harness.ring, harness.size, Some(&ui));
    harness.renderer.prepare(&harness.gpu, harness.ring.current());

    let ui = text_frame(&harness, &mut text, &mut atlas, "Attack");
    assert!(!ui.glyphs.forgot);
    assert!(ui.page_uploads.is_empty());
    assert!(lit_pixels(&harness.draw(Some(&ui))) > 100);
}

#[test]
#[ignore = "needs a GPU"]
fn a_list_made_for_another_renderer_draws_no_glyphs_and_says_so() {
    let (mut text, mut atlas) = text_stack();
    let first = Harness::new((160, 48));
    let mut second = Harness::new((160, 48));
    let ui = text_frame(&first, &mut text, &mut atlas, "Attack");
    let pixels = second.draw(Some(&ui));
    let stats = second.renderer.stats().ui;
    assert_eq!(stats.stale_page_quads, ui.quads.len() as u32);
    assert_eq!((stats.page_uploads, stats.draws), (0, 0));
    assert_eq!(lit_pixels(&pixels), 0);
    assert!(!second.drew_ui_pass());

    let ui = text_frame(&second, &mut text, &mut atlas, "Attack");
    assert!(lit_pixels(&second.draw(Some(&ui))) > 100, "the next frame heals it");
    assert_eq!(second.renderer.stats().ui.stale_page_quads, 0);
}

#[test]
#[ignore = "needs a GPU"]
fn a_screen_of_glyphs_is_far_fewer_writes_than_glyphs() {
    let (mut text, mut atlas) = text_stack();
    let mut harness = Harness::new((960, 48));
    let ui = text_frame(
        &harness,
        &mut text,
        &mut atlas,
        "The quick brown fox jumps over the lazy dog, 0123456789",
    );
    let drawn = harness.draw(Some(&ui));
    let stats = harness.renderer.stats().ui;
    assert!(lit_pixels(&drawn) > 100);
    assert!(u32::try_from(ui.page_uploads.len()).unwrap() > 20);
    assert!(stats.page_uploads * 4 <= ui.page_uploads.len() as u32, "{} writes", stats.page_uploads);
    assert_eq!(stats.glyphs, ui.glyphs, "the atlas's stats reach the renderer's");
}
