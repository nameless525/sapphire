//! The viewport with the real renderer, on a real GPU and with no window: an editor in a harness asks for a pane,
//! the stage draws into it, and [`Painter`] does what the editor's window does, against a texture.
//!
//! They need a GPU and a cook: `cargo xtask content`, then
//! `cargo test -p sapphire_editor --test viewport_stage -- --ignored --test-threads=1`. Costs are worth reading
//! optimized, as `cargo xtask editor` builds: `--profile tools`, with `--nocapture`.

#![allow(clippy::print_stdout, reason = "with --nocapture, its numbers are what it is run for")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::mutex::RwLock;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, TestRenderer};
use egui_wgpu::RendererOptions;
use game_combat::combat::{CombatOption, no_bindings};
use game_combat::documents::FightDocuments;
use game_combat::fight::Fight;
use sapphire_assets::ui::UiDocument;
use sapphire_document::PropertyPath;
use sapphire_editor::edits::{EntryRef, Track, set_window};
use sapphire_editor::gizmo::{Axis, Gizmo};
use sapphire_editor::recovery::Recovery;
use sapphire_editor::screen::{Aspect, StandIn};
use sapphire_editor::shell::Pane;
use sapphire_editor::viewport::{
    OutsideView, SETTLING_FRAMES, StageView, TimelineShown, ViewShow, on_pixels, play_frames, play_timeline,
    stand, stand_zone_stage,
};
use sapphire_editor::{Editor, Options};
use sapphire_gameplay::replay::{LiveMoves, SCENARIOS, find_scenario};
use sapphire_gameplay::targeting::TargetingTuning;
use sapphire_gameplay::{ActorId, SimConfig, Simulation};
use sapphire_platform::{Event, Host, KeyEvent, Keycode, Modifiers, Scancode, Timestamp, WindowId};
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_stage::{Content, Readback, Stage, headless_gpu, lookdev};
use sapphire_time::Stopwatch;

/// Moves that name no clip.
const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const TARGETING: &str = include_str!("../../../crates/sapphire_gameplay/default_targeting.ron");

const PLAIN: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SRGB: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).expect("the workspace").to_owned()
}

fn cook() -> PathBuf {
    root().join("target/content/desktop")
}

fn player_path() -> PathBuf {
    root().join("content/combat/player/player.moves.ron")
}

fn host() -> Host {
    Host::with_content_at(&cook())
}

fn device() -> Arc<GpuContext> {
    Arc::new(headless_gpu().expect("a GPU"))
}

/// egui's renderer on the engine's device.
#[derive(Clone)]
struct Painter {
    renderer: Arc<RwLock<egui_wgpu::Renderer>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
}

impl Painter {
    fn new(gpu: &GpuContext, format: wgpu::TextureFormat) -> Self {
        let renderer = egui_wgpu::Renderer::new(gpu.device(), format, RendererOptions::PREDICTABLE);
        Self {
            renderer: Arc::new(RwLock::new(renderer)),
            device: gpu.device().clone(),
            queue: gpu.queue().clone(),
            format,
        }
    }

    /// Paints the harness's last pass, as the window does, and reads it back.
    fn paint(&self, gpu: &GpuContext, harness: &Harness<'_, Editor>) -> Image {
        read_texture(gpu, &self.paint_only(harness))
    }

    /// Paints the harness's last pass.
    fn paint_only(&self, harness: &Harness<'_, Editor>) -> wgpu::Texture {
        let output = harness.output();
        let pixels_per_point = output.pixels_per_point;
        let primitives = harness.ctx.tessellate(output.shapes.clone(), pixels_per_point);
        let size = harness.ctx.content_rect().size() * pixels_per_point;
        let (width, height) = (size.x.round() as u32, size.y.round() as u32);
        let framebuffer = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shell"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = framebuffer.create_view(&wgpu::TextureViewDescriptor::default());
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [width, height], pixels_per_point };
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut renderer = self.renderer.write();
        renderer.update_buffers(&self.device, &self.queue, &mut encoder, &primitives, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shell"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            renderer.render(&mut pass, &primitives, &screen);
        }
        drop(renderer);
        self.queue.submit([encoder.finish()]);
        framebuffer
    }
}

impl TestRenderer for Painter {
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        let mut renderer = self.renderer.write();
        for (id, deltas) in &delta.set {
            for image in deltas {
                renderer.update_texture(&self.device, &self.queue, *id, image);
            }
        }
        delta.set.clear();
        for id in delta.free.drain() {
            renderer.free_texture(&id);
        }
    }
}

struct Image {
    width: u32,
    pixels: Vec<u8>,
}

impl Image {
    /// The pixels of `rect`, at a scale of 1.
    fn cropped(&self, rect: egui::Rect) -> Self {
        let (left, top) = (rect.min.x as u32, rect.min.y as u32);
        let (width, height) = (rect.width() as u32, rect.height() as u32);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in top..top + height {
            for x in left..left + width {
                pixels.extend_from_slice(self.at(x, y));
            }
        }
        Self { width, pixels }
    }

    fn at(&self, x: u32, y: u32) -> &[u8] {
        let at = ((y * self.width + x) * 4) as usize;
        &self.pixels[at..at + 4]
    }
}

fn read_texture(gpu: &GpuContext, texture: &wgpu::Texture) -> Image {
    let (width, height) = (texture.width(), texture.height());
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("maps"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("polls");
    let mapped = buffer.slice(..).get_mapped_range().expect("mapped");
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for line in mapped.chunks(row as usize) {
        pixels.extend_from_slice(&line[..(width * 4) as usize]);
    }
    Image { width, pixels }
}

/// An editor on the player's moves, with the viewport's tab in front.
fn editor_harness(painter: &Painter, size: (f32, f32), scale: f32) -> Harness<'static, Editor> {
    let options = Options { file: Some(player_path()), content: Some(cook()), link: None };
    let mut editor = Editor::new(options);
    editor.shell_mut().open(Pane::Viewport);
    harness_on(painter, editor, size, scale)
}

/// A new egui over `editor`, as the window makes after a lost device.
fn harness_on(
    painter: &Painter,
    editor: Editor,
    (width, height): (f32, f32),
    scale: f32,
) -> Harness<'static, Editor> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(width, height))
        .with_pixels_per_point(scale)
        .with_accessibility_check(false)
        .with_step_dt(1.0 / 60.0)
        .renderer(painter.clone())
        .build_ui_state(|ui, editor: &mut Editor| editor.ui(ui), editor);
    harness.run_steps(2);
    harness
}

/// Passes and engine frames as a window runs them, until the viewport has settled. The pass after is what shows.
fn settle(harness: &mut Harness<'_, Editor>, view: &mut StageView, gpu: &Arc<GpuContext>, painter: &Painter) {
    for _ in 0..200 {
        harness.step();
        view.draw(gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
        if !harness.state_mut().viewport().is_settling() {
            harness.step();
            view.draw(gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
            return;
        }
    }
    panic!("it never settled");
}

/// What a capture of the same scene draws, in the same frames: a stage of its own at `sizes` in turn, with the
/// player's moves as the file has them, and the last read back.
fn capture(
    stage: &mut Stage<ViewShow>,
    sizes: &[(u32, u32)],
    aim: impl Fn(&mut Stage<ViewShow>, usize),
) -> Image {
    let mut last = None;
    for (round, &(width, height)) in sizes.iter().enumerate() {
        aim(stage, round);
        let target = stage.target(width, height, wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        for _ in 1..SETTLING_FRAMES {
            stage.render(&view, 1.0, |_, _| {}).expect("draws");
        }
        let readback = Readback::new(&stage.gpu, width, height);
        stage.render(&view, 1.0, |graph, target| readback.record(graph, target)).expect("draws");
        stage.check_gpu().expect("no errors");
        last = Some(Image { width, pixels: readback.read(&stage.gpu).expect("reads") });
    }
    last.expect("a size")
}

/// A stage as the viewport makes it, on the document's moves.
fn capture_stage(gpu: &Arc<GpuContext>, harness: &mut Harness<'_, Editor>) -> Stage<ViewShow> {
    let mut stage = stand(&host(), gpu).expect("stands");
    let frame = harness.state_mut().viewport_frame();
    let (name, set) = frame.moves.expect("the player's moves compile");
    let mut live = stage.show.fight.documents.live();
    live.remember(name, set.clone());
    let Stage { show, scene, content, .. } = &mut stage;
    show.fight.start_on(&sapphire_editor::viewport::the_fight(), live, scene, content).expect("starts");
    stage
}

/// Two runs of the renderer on one scene agree to within what [`how_far_two_captures_agree`] sees of them.
fn assert_runs_agree(most: u8, count: usize, name: &str) {
    assert!(most <= 1 && count <= 4, "{name}: {count} pixels differ, by up to {most}");
}

/// Keeps a frame to look at, and holds it to being a picture rather than two blank frames that agree.
fn is_a_picture(frame: &Image, name: &str) {
    let mut colours: Vec<&[u8]> = frame.pixels.chunks(4).collect();
    colours.sort_unstable();
    colours.dedup();
    assert!(colours.len() > 2000, "{name}: {} colours", colours.len());
    keep(frame, name);
}

/// Keeps a frame to look at, as `viewport_<name>.png`.
fn keep(frame: &Image, name: &str) {
    let folder = std::env::var_os("SAPPHIRE_EDITOR_CAPTURES")
        .map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")), PathBuf::from);
    std::fs::create_dir_all(&folder).expect("a folder for captures");
    let file = std::fs::File::create(folder.join(format!("viewport_{name}.png"))).expect("a file");
    let height = frame.pixels.len() as u32 / 4 / frame.width;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("a header");
    writer.write_image_data(&frame.pixels).expect("written");
}

/// The largest difference of a channel between a capture and where the frame shows in the shell, how many pixels
/// differ at all, and how many were compared. Leaves out `rim` pixels each way.
fn differences(
    frame: &Image,
    shell: &Image,
    rect: egui::Rect,
    pixels_per_point: f32,
    rim: u32,
) -> (u8, usize, usize) {
    let (left, top) =
        ((rect.min.x * pixels_per_point).round() as u32, (rect.min.y * pixels_per_point).round() as u32);
    let height = frame.pixels.len() as u32 / 4 / frame.width;
    let (mut most, mut count, mut all) = (0, 0, 0);
    for y in rim..height - rim {
        for x in rim..frame.width - rim {
            let (at, from) = (shell.at(x + left, y + top), frame.at(x, y));
            let difference = (0..3).map(|c| from[c].abs_diff(at[c])).max().unwrap_or(0);
            most = most.max(difference);
            count += usize::from(difference > 0);
            all += 1;
        }
    }
    (most, count, all)
}

/// The renderer in a pane shows what a capture of the same scene does, with its code values: on an sRGB framebuffer
/// and a plain one, at more than one size, at more than one scale.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_pane_shows_what_a_capture_does() {
    let gpu = device();
    for (name, format) in [("sRGB framebuffer", SRGB), ("plain framebuffer", PLAIN)] {
        for (size, scale) in [((1200.0, 700.0), 1.0), ((1000.0, 640.0), 2.0), ((1101.0, 677.0), 1.5)] {
            let painter = Painter::new(&gpu, format);
            let mut harness = editor_harness(&painter, size, scale);
            let mut view = StageView::default();
            settle(&mut harness, &mut view, &gpu, &painter);
            assert_eq!(view.frames(), u64::from(SETTLING_FRAMES), "only what settling asks for");
            assert_eq!(harness.state_mut().viewport().note(), None);
            let shell = painter.paint(&gpu, &harness);
            let rect = harness.state_mut().viewport().rect().expect("shown");
            let (_, pixels) = on_pixels(rect, scale);
            let mut stage = capture_stage(&gpu, &mut harness);
            let frame = capture(&mut stage, &[pixels], |_, _| ());
            is_a_picture(&frame, &format!("{}x{}", pixels.0, pixels.1));
            let (most, count, all) = differences(&frame, &shell, rect, scale, 0);
            println!("{name}, scale {scale}, {pixels:?}: {count} of {all} pixels differ, by at most {most}");
            assert_runs_agree(most, count, name);
            // The pane against what the stage drew, which no other run of the renderer comes into.
            let drawn = read_texture(&gpu, view.frame().expect("drawn"));
            let (most, count, all) = differences(&drawn, &shell, rect, scale, 0);
            println!("  against its own frame: {count} of {all} differ, by at most {most}");
            assert_eq!((most, count), (0, 0), "{name}, scale {scale}, {pixels:?}");
        }
    }
}

/// A resize shows in its own frame, and what it shows is what a capture of those frames does. It builds no
/// pipeline, and the document that came in built none either.
#[test]
#[ignore = "needs a GPU and a cook"]
fn opening_and_resizing_build_nothing() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let stage = view.stage().expect("stood");
    let after_open = stage.renderer.stats().pipelines;
    println!("after opening a document: {after_open:?}");
    let report = stage.renderer.first_use_report();
    assert_eq!(after_open.first_use_builds, 0, "{report}");

    let mut sizes = vec![on_pixels(harness.state_mut().viewport().rect().expect("shown"), 1.0).1];
    for size in [(1400.0, 800.0), (1000.0, 600.0), (1001.0, 601.0)] {
        harness.set_size(egui::vec2(size.0, size.1));
        settle(&mut harness, &mut view, &gpu, &painter);
        sizes.push(on_pixels(harness.state_mut().viewport().rect().expect("shown"), 1.0).1);
        let stage = view.stage().expect("stood");
        let pipelines = stage.renderer.stats().pipelines;
        assert_eq!(pipelines, after_open, "a resize to {size:?} built a pipeline");
    }
    let shell = painter.paint(&gpu, &harness);
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let mut stage = capture_stage(&gpu, &mut harness);
    let frame = capture(&mut stage, &sizes, |_, _| ());
    let (most, count, all) = differences(&frame, &shell, rect, 1.0, 0);
    println!("after {sizes:?}: {count} of {all} pixels differ, by at most {most}");
    assert_runs_agree(most, count, "after resizing");
    let drawn = read_texture(&gpu, view.frame().expect("drawn"));
    assert_eq!(differences(&drawn, &shell, rect, 1.0, 0).1, 0, "a resize shows in its own frame");
}

/// A drag and a scroll over the pane aim a camera from outside the game. The stage draws what a capture does from
/// the same view, and with nothing changing it stops drawing.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_camera_from_outside_aims_it() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let before = painter.paint(&gpu, &harness).cropped(rect);
    let drawn = view.frames();

    let from = rect.center();
    harness.drag_at(from);
    harness.step();
    harness.hover_at(from + egui::vec2(60.0, -20.0));
    harness.step();
    harness.drop_at(from + egui::vec2(60.0, -20.0));
    settle(&mut harness, &mut view, &gpu, &painter);
    let stage = view.stage().expect("stood");
    let aimed: OutsideView = stage.show.outside.expect("a drag takes the view over");
    let eye = stage.scene.camera().eye();
    assert!(eye.distance(aimed.eye()) < 1e-4, "{eye} against {:?}", aimed.eye());
    assert_eq!(view.frames(), drawn + u64::from(SETTLING_FRAMES), "one change, one settling");

    let after = painter.paint(&gpu, &harness);
    let (most, count, all) = differences(&before, &after, rect, 1.0, 0);
    println!("the drag moved {count} of {all} pixels, by at most {most}");
    assert!(count > all / 10, "{count} of {all}");

    let (_, pixels) = on_pixels(rect, 1.0);
    let mut stage = capture_stage(&gpu, &mut harness);
    let aim = |stage: &mut Stage<ViewShow>, round: usize| {
        if round == 1 {
            stage.show.outside = Some(aimed);
        }
    };
    let frame = capture(&mut stage, &[pixels, pixels], aim);
    let (most, count, all) = differences(&frame, &after, rect, 1.0, 0);
    println!("from the view it was dragged to: {count} of {all} pixels differ, by at most {most}");
    assert_runs_agree(most, count, "from the same view");
    drop(stage);

    let at_rest = view.frames();
    harness.run_steps(5);
    for _ in 0..5 {
        view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    }
    assert_eq!(view.frames(), at_rest, "nothing changed");
}

/// A viewport a tab away makes nothing, and closing its pane lets it all go.
#[test]
#[ignore = "needs a GPU and a cook"]
fn it_costs_nothing_until_it_shows() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let options = Options { file: Some(player_path()), content: Some(cook()), link: None };
    let mut harness = harness_on(&painter, Editor::new(options), (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    assert!(view.stage().is_none(), "the preview's in front");

    harness.state_mut().shell_mut().open(Pane::Viewport);
    settle(&mut harness, &mut view, &gpu, &painter);
    assert!(view.stage().is_some());

    harness.state_mut().shell_mut().open(Pane::Preview);
    harness.step();
    let frames = view.frames();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    assert_eq!(view.frames(), frames, "a tab away, it draws nothing");
    assert!(view.stage().is_some(), "and keeps what it stood");

    harness.state_mut().shell_mut().close(Pane::Viewport);
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    assert!(view.stage().is_none(), "closed, it goes");
}

/// With nothing cooked it says so, and a click tries again rather than every frame.
#[test]
#[ignore = "needs a GPU"]
fn no_cook_says_so_in_the_pane() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let nowhere = root().join("target/no-cook-here");
    let options = Options { file: None, content: Some(nowhere), link: None };
    let mut editor = Editor::new(options);
    editor.shell_mut().open(Pane::Viewport);
    let mut harness = harness_on(&painter, editor, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    for _ in 0..3 {
        harness.step();
        view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    }
    assert!(view.stage().is_none());
    let note = harness.state_mut().viewport().note().expect("a reason").to_owned();
    assert!(note.contains("cargo xtask content"), "{note}");
    assert!(note.contains("Click to try again"), "{note}");
    assert!(!harness.state_mut().viewport().is_settling(), "and no more frames for it");
}

/// What the stack in a window holds of the GPU, which all goes when its device does.
struct Stack {
    view: StageView,
    painter: Painter,
    gpu: Arc<GpuContext>,
}

impl Stack {
    fn new() -> Result<Self, String> {
        let gpu = device();
        Ok(Self { view: StageView::default(), painter: Painter::new(&gpu, PLAIN), gpu })
    }
}

/// A device lost under the pane: the frame in hand does nothing and doesn't panic, everything on the GPU is made
/// again, and what's being edited is all still there, unsaved, and on the new frame. It's as far as a test
/// can go with no surface to report the loss: `Device::destroy` for the driver, and [`Recovery`] for the window.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_lost_device_takes_the_stage_and_leaves_the_documents() {
    let (size, scale) = ((800.0, 520.0), 1.5);
    let mut recovery = Recovery::new(Stack::new().expect("a stack"));
    let mut harness = {
        let stack = recovery.ready().expect("made");
        editor_harness(&stack.painter, size, scale)
    };
    {
        let Stack { view, painter, gpu } = recovery.ready().expect("made");
        settle(&mut harness, view, gpu, painter);
    }
    // An edit nobody's saved.
    let entry = EntryRef { mv: 0, track: Track::Hitboxes, index: 0 };
    harness
        .state_mut()
        .moves_document_mut()
        .expect("open")
        .together("an edit", |text| set_window(text, entry, 7, 12))
        .expect("an edit");
    harness.step();
    let edited = harness.state().current().expect("open").text().to_owned();
    assert!(harness.state().is_dirty());

    let stack = recovery.ready().expect("made");
    stack.gpu.device().destroy();
    let _ = stack.gpu.device().poll(wgpu::PollType::wait_indefinitely());
    assert!(stack.gpu.is_lost());
    // The frame in hand, which a window's only told of by its next acquire.
    harness.step();
    let Stack { view, painter, gpu } = stack;
    view.draw(gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());

    recovery.lost(0.0);
    harness.state_mut().viewport().forget_gpu();
    assert!(recovery.ready().is_none());
    assert!(harness.state_mut().viewport().note().is_some(), "the pane says why");
    let text_now = |editor: &Editor| editor.current().expect("open").text().to_owned();
    assert_eq!(text_now(harness.state()), edited, "still there");
    assert!(harness.state().is_dirty(), "and still unsaved");

    // A new context and a new egui, as the window makes on its next try.
    let stack = recovery.device(10.0, Stack::new).expect("another");
    let Stack { view, painter, gpu } = stack;
    let editor = std::mem::replace(harness.state_mut(), Editor::new(Options::default()));
    let mut harness = harness_on(painter, editor, size, scale);
    settle(&mut harness, view, gpu, painter);
    assert_eq!(view.frames(), u64::from(SETTLING_FRAMES));
    assert_eq!(harness.state_mut().viewport().note(), None);
    assert_eq!(text_now(harness.state()), edited);
    assert!(harness.state().is_dirty());

    let shell = painter.paint(gpu, &harness);
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let (_, pixels) = on_pixels(rect, scale);
    let drawn = read_texture(gpu, view.frame().expect("drawn"));
    assert_eq!(differences(&drawn, &shell, rect, scale, 0).1, 0, "the pane is its frame");
    let mut stage = capture_stage(gpu, &mut harness);
    let frame = capture(&mut stage, &[pixels], |_, _| ());
    let (most, count, all) = differences(&frame, &shell, rect, scale, 0);
    println!("after the loss: {count} of {all} pixels differ, by at most {most}");
    assert_runs_agree(most, count, "after the loss");
    let builds = stage.renderer.stats().pipelines;
    assert_eq!(builds.first_use_builds, 0);
}

/// What the editor's frame costs with the viewport on the arena: a pass of egui, the stage's frame and egui's paint.
#[test]
#[ignore = "needs a GPU and a cook, and prints costs"]
fn what_the_viewport_costs() {
    let stood = {
        let gpu = device();
        let started = Stopwatch::start();
        let stage = stand(&host(), &gpu).expect("stands");
        let elapsed = started.elapsed();
        let builds = stage.renderer.stats().pipelines;
        println!("a stage on the arena: {elapsed:?}, {builds:?}");
        elapsed
    };
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1500.0, 960.0), 2.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let rect = harness.state_mut().viewport().rect().expect("shown");
    println!("the pane: {:?} pixels (a stage took {stood:?})", on_pixels(rect, 2.0).1);

    let frames = 300;
    let time = |harness: &mut Harness<'_, Editor>, turn: bool, painter: &Painter, view: &mut StageView| {
        let mut all = Vec::with_capacity(frames);
        let from = rect.center();
        if turn {
            harness.drag_at(from);
            harness.step();
            harness.hover_at(from + egui::vec2(30.0, 0.0));
            harness.step();
        }
        for at in 0..frames {
            if turn {
                harness.hover_at(from + egui::vec2(if at % 2 == 0 { 34.0 } else { 30.0 }, 0.0));
            }
            let started = Stopwatch::start();
            harness.step();
            view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
            drop(painter.paint_only(harness));
            // All of it, the GPU's share too, as a window's next frame would wait for.
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("polls");
            all.push(started.elapsed());
        }
        all.sort();
        (all[all.len() / 2], all[all.len() * 99 / 100], all[all.len() - 1])
    };
    let idle = time(&mut harness, false, &painter, &mut view);
    println!("nothing changing, the stage drawing nothing: median, p99, worst {idle:?}");
    let drawing = time(&mut harness, true, &painter, &mut view);
    println!("a view turning, the stage drawing every frame: median, p99, worst {drawing:?}");

    harness.state_mut().play_in_viewport(Some(SCRIPT.to_owned()));
    harness.step();
    let started = Stopwatch::start();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    let hitch = started.elapsed();
    assert!(harness.state_mut().viewport().is_playing());
    let running = time(&mut harness, false, &painter, &mut view);
    println!("Play's frame: {hitch:?}; a run: median, p99, worst {running:?}");
    println!("reached tick {}", simulated(&mut view).tick());
}

/// How far two captures of one scene agree, with no pane in it: what a pane's comparison can't beat. About one pair
/// in 60 differs, by one code in one pixel, in a different place each time. Nothing about the renderer's frame says
/// why: it's left to whoever cares for bytes across runs, and goldens compare by FLIP.
#[test]
#[ignore = "needs a GPU and a cook, and prints how they agree"]
fn how_far_two_captures_agree() {
    let gpu = device();
    let (mut differing, rounds) = (0, 60);
    for _ in 0..rounds {
        let mut one = stand(&host(), &gpu).expect("stands");
        let first = capture(&mut one, &[(725, 335)], |_, _| ());
        let mut two = stand(&host(), &gpu).expect("stands");
        let second = capture(&mut two, &[(725, 335)], |_, _| ());
        let (most, count, _) = differences(&first, &second, egui::Rect::ZERO, 1.0, 0);
        assert_runs_agree(most, count, "two captures");
        differing += usize::from(count > 0);
    }
    println!("{differing} of {rounds} pairs of captures differ");
}

// Play-in-editor (M3 E4, exit test 6).

const SCRIPT: &str = "attempts";

/// Passes and frames, as a window runs them, until the run's simulation has reached `tick`. Returns its checksum and
/// the tick it reached, since a frame can take more than one step.
fn play_to(
    harness: &mut Harness<'_, Editor>,
    view: &mut StageView,
    gpu: &Arc<GpuContext>,
    painter: &Painter,
    tick: u64,
) -> (u64, u64) {
    for _ in 0..tick * 4 + 100 {
        harness.step();
        view.draw(gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
        assert_eq!(harness.state_mut().viewport().note(), None);
        let simulation = simulated(view);
        if simulation.tick() >= tick {
            return (simulation.checksum(), simulation.tick());
        }
    }
    panic!("it never reached tick {tick}");
}

fn simulated(view: &mut StageView) -> &Simulation {
    let stage = view.stage().expect("stood");
    stage.show.fight.combat.as_ref().expect("a fight").simulation()
}

/// The fight with no renderer, editor or anything else: what `moves` and the script come to by themselves, set up
/// from the cook's CPU content as the game's is.
fn headless(moves: &LiveMoves, ticks: u64) -> u64 {
    let mut content = Content::new(&host(), lookdev::CHARACTER, lookdev::SKY).expect("the cook");
    let option = CombatOption::Fight { script: Some(SCRIPT.to_owned()), party: false };
    let mut fight = Fight::default();
    fight.documents = FightDocuments::read(host().content()).expect("the cook's documents");
    let mut combat = fight.prepare_stock_fight_on(&option, moves.clone(), &mut content).expect("sets up");
    for _ in 0..ticks {
        combat.step(&mut no_bindings);
    }
    combat.simulation().checksum()
}

/// Asks for a run by the script, and starts it with a frame.
fn play(
    harness: &mut Harness<'_, Editor>,
    view: &mut StageView,
    gpu: &Arc<GpuContext>,
    painter: &Painter,
) -> LiveMoves {
    harness.state_mut().play_in_viewport(Some(SCRIPT.to_owned()));
    let moves = {
        let note = harness.state_mut().viewport().note().map(str::to_owned);
        let asked = harness.state_mut().viewport().asked_for();
        let asked = asked.unwrap_or_else(|| panic!("refused: {note:?}")).snapshot.moves().clone();
        harness.step();
        view.draw(gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
        asked
    };
    assert!(harness.state_mut().viewport().is_playing(), "{:?}", harness.state_mut().viewport().note());
    moves
}

fn stop(harness: &mut Harness<'_, Editor>, view: &mut StageView, gpu: &Arc<GpuContext>, painter: &Painter) {
    harness.state_mut().viewport().stop_playing();
    settle(harness, view, gpu, painter);
    assert!(!harness.state_mut().viewport().is_playing());
    assert!(view.snapshot().is_none());
}

/// All of a document that could change, and the file behind it.
fn documents_as_they_are(editor: &Editor) -> (String, bool, Option<String>, Option<String>, Vec<u8>) {
    let current = editor.current().expect("open");
    (
        current.text().to_owned(),
        current.is_dirty(),
        current.undo_label().map(str::to_owned),
        current.redo_label().map(str::to_owned),
        std::fs::read(player_path()).expect("the file"),
    )
}

/// The runtime borrows nothing, so no borrow of a document can be in it. A shared handle would pass this: what keeps
/// one out is that the runtime's crates don't have `sapphire_document` in their dependency graph.
#[test]
fn a_stage_holds_no_borrow_of_anything() {
    fn owns_all_it_holds<T: 'static>() {}
    owns_all_it_holds::<Stage<ViewShow>>();
    owns_all_it_holds::<StageView>();
}

/// A snapshot is copies, with no GPU in it: an edit after it doesn't reach it, and it changes nothing itself. Its
/// moves name no clip, so they compile with no cook, as the test job has none.
#[test]
fn a_snapshot_is_copies() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("snapshot_copies");
    std::fs::create_dir_all(&dir).expect("a folder");
    let path = dir.join("slice.moves.ron");
    std::fs::write(&path, SLICE).expect("written");
    let mut editor = Editor::new(Options { file: Some(path), content: None, link: None });
    let before = documents_as_they_are(&editor);
    editor.play_in_viewport(None);
    assert_eq!(documents_as_they_are(&editor), before, "taking it touched nothing");

    let entry = EntryRef { mv: 0, track: Track::Hitboxes, index: 0 };
    let document = editor.moves_document_mut().expect("open");
    document.together("an edit", |text| set_window(text, entry, 7, 12)).expect("an edit");
    let edited = editor.current().expect("open").text().to_owned();
    assert_ne!(edited, before.0);
    let request = editor.viewport().asked_for().expect("asked");
    assert_eq!(request.snapshot.text_of("slice.moves.ron"), Some(before.0.as_str()), "it stayed");
    assert!(request.snapshot.moves().latest("slice.moves.ron").is_some());
}

/// A cook's folder of `documents/` alone, as a cook ships a game's: its manifest naming `targeting`, if it's given.
fn a_games_documents(name: &str, targeting: Option<&str>) -> PathBuf {
    let cook = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name).join("cook");
    let _ = std::fs::remove_dir_all(&cook);
    std::fs::create_dir_all(cook.join("documents/tuning")).expect("a cook");
    if let Some(text) = targeting {
        std::fs::write(cook.join("documents/tuning/my_targeting.ron"), text).expect("written");
        let manifest = "(type: \"sapphire::assets::GameDocuments\", version: 1, \
                        data: (targeting: Some(\"tuning/my_targeting.ron\")))";
        std::fs::write(cook.join("documents/game.documents.ron"), manifest).expect("written");
    }
    cook
}

/// Play starts where the game's fight does: on the targeting weights its cook ships, not Sapphire's, with the moves
/// open here over them. The run's simulation holds the game's weights.
#[test]
fn a_run_starts_on_the_games_own_numbers() {
    let own = TARGETING.replacen("range: 12.0", "range: 20.0", 1).replacen(
        "keep_range: 16.0",
        "keep_range: 24.0",
        1,
    );
    let cook = a_games_documents("play_game_documents", Some(&own));
    let path = cook.parent().expect("its folder").join("slice.moves.ron");
    std::fs::write(&path, SLICE).expect("written");
    let mut editor = Editor::new(Options { file: Some(path), content: Some(cook), link: None });
    editor.play_in_viewport(None);
    let note = editor.viewport().note().map(str::to_owned);
    let request = editor.viewport().asked_for().unwrap_or_else(|| panic!("refused: {note:?}"));
    let own = TargetingTuning::from_ron(&own).expect("reads");
    assert_ne!(own, TargetingTuning::shipped(), "the game's own weights");
    assert_eq!(request.snapshot.fight().targeting, own);
    let duel = find_scenario(&SCENARIOS, "duel").expect("listed");
    let (simulation, _) = request.snapshot.moves().start(duel, SimConfig::default()).expect("starts");
    assert_eq!(simulation.targeting(), own, "the run's simulation holds them");
    assert!(
        request.snapshot.moves().latest("slice.moves.ron").is_some(),
        "with the moves open here over them"
    );
}

/// A cook from before a game's documents shipped can't say what its fight starts on, so Play says so, as the game
/// would, rather than playing Sapphire's numbers as if they were the game's.
#[test]
fn a_cook_with_no_manifest_refuses_a_run() {
    let cook = a_games_documents("play_no_manifest", None);
    let path = cook.parent().expect("its folder").join("slice.moves.ron");
    std::fs::write(&path, SLICE).expect("written");
    let mut editor = Editor::new(Options { file: Some(path), content: Some(cook), link: None });
    editor.play_in_viewport(None);
    assert!(editor.viewport().asked_for().is_none());
    let note = editor.viewport().note().expect("why").to_owned();
    assert!(note.contains("cook again"), "{note}");
}

/// What doesn't compile can't be played, and the pane says which and why.
#[test]
fn moves_that_do_not_compile_refuse_a_run() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("play_refused");
    std::fs::create_dir_all(&dir).expect("a folder");
    let path = dir.join("broken.moves.ron");
    std::fs::write(&path, "(version: 1, moves: not this)").expect("written");
    let mut editor = Editor::new(Options { file: Some(path), content: None, link: None });
    editor.play_in_viewport(None);
    assert!(editor.viewport().asked_for().is_none());
    let note = editor.viewport().note().expect("why").to_owned();
    assert!(note.contains("broken.moves.ron"), "{note}");
}

/// Exit test 6: a run reads a snapshot and leaves the documents alone. With nothing edited, 600 ticks of a fight
/// change no byte of a document, of its history or of its file, and it's the run a headless fight has.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_run_leaves_the_documents_alone() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let before = documents_as_they_are(harness.state());

    let moves = play(&mut harness, &mut view, &gpu, &painter);
    let (checksum, tick) = play_to(&mut harness, &mut view, &gpu, &painter, 600);
    println!("a run reached tick {tick}, checksum {checksum:016x}");
    assert_eq!(checksum, headless(&moves, tick), "what a fight with no editor reaches");
    assert_eq!(documents_as_they_are(harness.state()), before, "during the run");

    stop(&mut harness, &mut view, &gpu, &painter);
    assert_eq!(documents_as_they_are(harness.state()), before, "after it");
    assert!(!harness.state().is_dirty());
}

/// The same in few frames with one stage, for where a frame costs a second or more: CI's lavapipe lane.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_short_run_leaves_the_documents_alone() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (800.0, 520.0), 1.0);
    let mut view = StageView::default();
    harness.step();
    let before = documents_as_they_are(harness.state());

    let moves = play(&mut harness, &mut view, &gpu, &painter);
    let (checksum, tick) = play_to(&mut harness, &mut view, &gpu, &painter, 40);
    assert_eq!(checksum, headless(&moves, tick), "what a fight with no editor reaches");
    let stage = view.stage().expect("stood");
    assert_eq!(stage.renderer.stats().pipelines.first_use_builds, 0, "{}", stage.renderer.first_use_report());
    assert_eq!(documents_as_they_are(harness.state()), before, "during the run");

    harness.state_mut().shell_mut().close(Pane::Viewport);
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    assert!(!harness.state_mut().viewport().is_playing());
    assert!(view.stage().is_none());
    assert_eq!(documents_as_they_are(harness.state()), before, "after it");
}

/// A run plays what stood when it started. An edit during it reaches only the next, which starts fresh, and undo
/// takes the document back to its bytes with runs in between.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_run_plays_its_snapshot_and_the_next_plays_the_edit() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let original = harness.state().current().expect("open").text().to_owned();
    // The script presses every 30 ticks, so whether the string's next hit is open at 30 sends the fight elsewhere,
    // where a hit's window can't with nobody in reach of it.
    let next_hit = EntryRef { mv: 0, track: Track::Cancels, index: 1 };
    let edit = |harness: &mut Harness<'_, Editor>, to: u32| {
        let document = harness.state_mut().moves_document_mut().expect("open");
        set_window(document, next_hit, 14, to).expect("an edit");
        harness.step();
    };

    edit(&mut harness, 31);
    let first = play(&mut harness, &mut view, &gpu, &painter);
    let (.., early) = play_to(&mut harness, &mut view, &gpu, &painter, 120);
    edit(&mut harness, 29);
    let edited = harness.state().current().expect("open").text().to_owned();
    assert_eq!(
        view.snapshot().expect("a run").text_of("player.moves.ron").map(|text| text != edited),
        Some(true)
    );
    let (checksum, tick) = play_to(&mut harness, &mut view, &gpu, &painter, 600);
    println!("from tick {early}, an edit in the run: at {tick}, {checksum:016x}");
    assert_eq!(checksum, headless(&first, tick), "the snapshot's");
    stop(&mut harness, &mut view, &gpu, &painter);

    let second = play(&mut harness, &mut view, &gpu, &painter);
    assert_eq!(simulated(&mut view).tick(), 0, "it starts fresh");
    let (again, tick) = play_to(&mut harness, &mut view, &gpu, &painter, 600);
    assert_eq!(again, headless(&second, tick), "the edit's");
    println!("the next run: at {tick}, {again:016x}");
    assert_ne!(checksum, again, "the edit shows");
    stop(&mut harness, &mut view, &gpu, &painter);

    // A run's cost nothing of history.
    harness.state_mut().undo();
    harness.state_mut().undo();
    harness.step();
    assert_eq!(harness.state().current().expect("open").text(), original, "the same bytes");
    assert!(!harness.state().is_dirty());
}

/// Two runs from one snapshot are one run, whatever came before them, since nothing's carried from it. And a stage
/// after a run shows what it showed before.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_run_starts_from_nothing_of_the_last() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let (_, pixels) = on_pixels(rect, 1.0);

    let moves = play(&mut harness, &mut view, &gpu, &painter);
    let tick = 300;
    let (one, reached) = play_to(&mut harness, &mut view, &gpu, &painter, tick);
    stop(&mut harness, &mut view, &gpu, &painter);
    let shell = painter.paint(&gpu, &harness);
    let rect_after = harness.state_mut().viewport().rect().expect("shown");
    assert_eq!(rect_after, rect);
    let mut stage = capture_stage(&gpu, &mut harness);
    let frame = capture(&mut stage, &[pixels], |_, _| ());
    let (most, count, all) = differences(&frame, &shell, rect, 1.0, 0);
    println!("back at rest: {count} of {all} pixels differ, by at most {most}");
    assert_runs_agree(most, count, "back at rest");
    drop(stage);

    let _ = play(&mut harness, &mut view, &gpu, &painter);
    let (two, reached_again) = play_to(&mut harness, &mut view, &gpu, &painter, tick);
    assert_eq!(one, headless(&moves, reached));
    assert_eq!(two, headless(&moves, reached_again));
    stop(&mut harness, &mut view, &gpu, &painter);
}

/// What a fight plays builds nothing a load didn't: its hits and effects, over a long run.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_run_builds_no_pipeline() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    harness.state_mut().play_in_viewport(Some("fight-back".to_owned()));
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    play_to(&mut harness, &mut view, &gpu, &painter, 1200);
    let stage = view.stage().expect("stood");
    let (values, _) = stage.show.fight.show.hud_values().expect("the fight says how it's going");
    let enemy = values.enemy.expect("an enemy stands there");
    assert!(values.player.hp < 1.0 || enemy.hp < 1.0, "someone's been hit");
    let report = stage.renderer.first_use_report();
    let builds = stage.renderer.stats().pipelines;
    println!("after 1200 ticks of a run: {builds:?}");
    assert_eq!(builds.first_use_builds, 0, "{report}");
    stage.check_gpu().expect("no errors");
}

/// The keys reach the fight through the game's own bindings, and the editor's shortcuts wait.
#[test]
#[ignore = "needs a GPU and a cook"]
fn the_keys_play_a_run() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    harness.state_mut().play_in_viewport(None);
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    assert!(harness.state_mut().viewport().is_playing());
    let player = ActorId::from_raw(1).expect("not zero");

    let mut nanos = 1_000_000_u64;
    let mut frame = |harness: &mut Harness<'_, Editor>, view: &mut StageView, down: Option<bool>| {
        nanos += 16_666_667;
        let timestamp = Timestamp::from_nanos(nanos);
        if let Some(down) = down {
            view.handle_event(&Event::Key(KeyEvent {
                timestamp,
                window: WindowId::from_raw(1),
                scancode: Scancode::J,
                keycode: Keycode(0),
                modifiers: Modifiers::default(),
                down,
                repeat: false,
            }));
        }
        view.read_input(timestamp);
        harness.step();
        view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    };
    for _ in 0..10 {
        frame(&mut harness, &mut view, None);
    }
    let simulation = simulated(&mut view);
    assert!(simulation.actor(player).expect("there").moves.current.is_none(), "nobody's pressed anything");

    frame(&mut harness, &mut view, Some(true));
    frame(&mut harness, &mut view, Some(false));
    for _ in 0..6 {
        frame(&mut harness, &mut view, None);
    }
    let simulation = simulated(&mut view);
    assert!(simulation.actor(player).expect("there").moves.current.is_some(), "a press starts a move");

    // The timeline's Space is the fight's now: it doesn't start the timeline's playback.
    harness.key_press(egui::Key::Space);
    harness.step();
    assert!(!harness.state().is_playing());
}

/// The buttons start and stop a run, and the keys a fight takes don't press them.
#[test]
#[ignore = "needs a GPU and a cook"]
fn the_buttons_start_and_stop_a_run() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let frame = |harness: &mut Harness<'_, Editor>, view: &mut StageView| {
        harness.run_steps(2);
        view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    };

    harness.get_by_label("Play in viewport").click();
    frame(&mut harness, &mut view);
    frame(&mut harness, &mut view);
    assert!(harness.state_mut().viewport().is_playing(), "{:?}", harness.state_mut().viewport().note());

    for key in [egui::Key::Space, egui::Key::Enter] {
        harness.key_press(key);
        frame(&mut harness, &mut view);
        assert!(harness.state_mut().viewport().is_playing(), "{key:?} is the fight's");
    }

    harness.get_by_label("Stop playing").click();
    frame(&mut harness, &mut view);
    frame(&mut harness, &mut view);
    assert!(!harness.state_mut().viewport().is_playing());
    settle(&mut harness, &mut view, &gpu, &painter);
    assert!(view.stage().is_some(), "the viewport stands again");
}

/// Nobody can see or stop a run behind another tab, and it would hold the keys, so a tab away ends it: the keys
/// are the editor's again and what comes back is the viewport at rest.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_tab_away_ends_a_run() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let before = documents_as_they_are(harness.state());
    let _ = play(&mut harness, &mut view, &gpu, &painter);
    play_to(&mut harness, &mut view, &gpu, &painter, 30);

    harness.key_press(egui::Key::Space);
    harness.step();
    assert!(!harness.state().is_playing(), "the fight's");
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());

    harness.state_mut().shell_mut().open(Pane::Preview);
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    assert!(!harness.state_mut().viewport().is_playing());
    assert!(view.snapshot().is_none());
    assert_eq!(documents_as_they_are(harness.state()), before);

    harness.key_press(egui::Key::Space);
    harness.step();
    assert!(harness.state().is_playing(), "the editor's again");
    harness.key_press(egui::Key::Space);
    harness.step();

    harness.state_mut().shell_mut().open(Pane::Viewport);
    settle(&mut harness, &mut view, &gpu, &painter);
    assert!(view.stage().expect("stood").show.controls.is_none(), "at rest");
}

/// A device lost in a run ends it, and the documents go on.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_lost_device_ends_a_run() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = editor_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let before = documents_as_they_are(harness.state());
    let _ = play(&mut harness, &mut view, &gpu, &painter);
    play_to(&mut harness, &mut view, &gpu, &painter, 60);
    gpu.device().destroy();
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    harness.state_mut().viewport().forget_gpu();
    drop(view);
    assert!(!harness.state_mut().viewport().is_playing());
    assert_eq!(documents_as_they_are(harness.state()), before);
}

// The zone panel and the gizmo (M3 E5).

/// The editor's cook: the desktop one, and each zone's shapes.
fn editor_cook() -> PathBuf {
    root().join("target/content/editor")
}

/// The proving ground in a folder of its own, with its sidecar, so nothing a test does reaches the repository's. It's
/// outside the repository, whose roots would otherwise hold two files of one ID.
fn proving_ground(name: &str) -> PathBuf {
    let from = root().join("content/zones/proving_ground");
    let dir = std::env::temp_dir().join(format!("sapphire_editor_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    for file in ["proving_ground.zone.ron", "proving_ground.zone.ron.meta"] {
        std::fs::copy(from.join(file), dir.join(file)).expect("copied");
    }
    dir.join("proving_ground.zone.ron")
}

/// An editor on a copy of the proving ground, with the viewport's tab in front.
fn zone_harness(
    painter: &Painter,
    name: &str,
    size: (f32, f32),
    scale: f32,
) -> (Harness<'static, Editor>, PathBuf) {
    let path = proving_ground(name);
    let options = Options { file: Some(path.clone()), content: Some(editor_cook()), link: None };
    let mut editor = Editor::new(options);
    editor.shell_mut().open(Pane::Viewport);
    (harness_on(painter, editor, size, scale), path)
}

/// The cart the tests move, turned 30 degrees, and where the file has it.
const CART: usize = 2;
const CART_AT: &str = "at: (9.0, 0.0, 10.0)";

/// Where on the screen the gizmo's arrow along `axis` is, `along` metres past where the selection stands: a point
/// of the pane, in points.
fn on_arrow(
    harness: &mut Harness<'_, Editor>,
    origin: sapphire_math::Vec3,
    axis: Axis,
    along: f32,
) -> egui::Pos2 {
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let camera = *harness.state_mut().viewport().camera().expect("a frame's camera");
    let gizmo = Gizmo::at(origin, &camera);
    let point = origin + axis.direction() * (gizmo.length * 0.6 + along);
    camera.to_screen(point, rect).expect("in front")
}

/// A drag of the gizmo's arrow along `axis` from the selection at `origin`, moved `metres` along it, as a person's
/// hand does it: a press, ten moves, a release. No frame's drawn while the hand moves.
fn drag_arrow(harness: &mut Harness<'_, Editor>, origin: sapphire_math::Vec3, axis: Axis, metres: f32) {
    let from = on_arrow(harness, origin, axis, 0.0);
    let to = on_arrow(harness, origin, axis, metres);
    harness.hover_at(from);
    harness.step();
    harness.drag_at(from);
    harness.step();
    for step in 1..=10 {
        harness.hover_at(from.lerp(to, step as f32 / 10.0));
        harness.step();
    }
    harness.drop_at(to);
    harness.step();
}

/// A stage of its own laying out the zone the editor shows, through the layouts in `rounds` in turn, each drawn as
/// many frames as a change settles for: what a capture of the viewport's frames draws.
fn capture_zone(
    gpu: &Arc<GpuContext>,
    size: (u32, u32),
    outside: Option<OutsideView>,
    rounds: &[(Vec<sapphire_stage::scene::LaidOut>, Option<Gizmo>)],
) -> (Stage<ViewShow>, Image) {
    let mut stage = stand_zone_stage(&Host::with_content_at(&editor_cook()), gpu).expect("stands");
    stage.show.outside = outside;
    let pack = sapphire_zone::pack_name("proving_ground");
    let frame = capture(&mut stage, &vec![size; rounds.len()], |stage, round| {
        let (laid, gizmo) = &rounds[round];
        let Stage { scene, content, renderer, gpu, show, .. } = stage;
        let layout = scene.lay_out_zone(content, &pack, laid, renderer, gpu).expect("laid out");
        assert!(layout.missing.is_empty(), "{layout:?}");
        show.gizmo = *gizmo;
    });
    (stage, frame)
}

/// What the viewport has laid out and the gizmo it draws, from the editor's documents as they are.
fn zone_as_shown(harness: &mut Harness<'_, Editor>) -> (Vec<sapphire_stage::scene::LaidOut>, Option<Gizmo>) {
    let frame = harness.state_mut().viewport_frame();
    let zone = frame.zone.expect("a zone shown");
    (zone.laid_out(), zone.gizmo)
}

/// A drag of the gizmo moves the selected placement in the zone document by exactly the bytes of that one number,
/// as one step of undo, which takes it back to the file's bytes. The viewport shows it where a capture of the same
/// layouts does, and nothing's built: opening the zone built what its pack draws through, and a drag moves an
/// instance.
#[test]
#[ignore = "needs a GPU and an editor's cook"]
fn a_drag_of_the_gizmo_moves_one_number() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let (mut harness, path) = zone_harness(&painter, "gizmo_drag", (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    assert_eq!(harness.state_mut().viewport().note(), None, "every placement's shape is in the cook");
    let opened = zone_as_shown(&mut harness);
    let after_open = view.stage().expect("stood").renderer.stats().pipelines;
    println!("after opening the zone: {after_open:?}");
    assert_eq!(
        after_open.first_use_builds,
        0,
        "{}",
        view.stage().expect("stood").renderer.first_use_report()
    );
    let outside = view.stage().expect("stood").show.outside;
    assert!(outside.is_some(), "a zone's view is from outside");

    harness.state_mut().select_placement(Some(CART));
    settle(&mut harness, &mut view, &gpu, &painter);
    let selected = zone_as_shown(&mut harness);
    assert!(selected.1.is_some(), "the gizmo's on the cart");
    let before = painter.paint(&gpu, &harness);

    let file = std::fs::read_to_string(&path).expect("the file");
    let cart = sapphire_math::Vec3::new(9.0, 0.0, 10.0);
    drag_arrow(&mut harness, cart, Axis::X, 1.0);
    let text = harness.state().current().expect("open").text().to_owned();
    assert_eq!(text, file.replacen(CART_AT, "at: (10.0, 0.0, 10.0)", 1), "one number's bytes");
    let current = harness.state().current().expect("open");
    assert!(!current.in_gesture(), "the release ended it");
    assert_eq!(current.undo_label(), Some("move a placement"));

    settle(&mut harness, &mut view, &gpu, &painter);
    let moved = zone_as_shown(&mut harness);
    assert_ne!(moved.0[CART], selected.0[CART], "the cart's moved");
    assert_eq!(
        moved.0.iter().zip(&selected.0).filter(|(now, was)| now != was).count(),
        1,
        "and only the cart"
    );
    let pipelines = view.stage().expect("stood").renderer.stats().pipelines;
    assert_eq!(pipelines, after_open, "a drag built nothing");

    let shell = painter.paint(&gpu, &harness);
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let (_, pixels) = on_pixels(rect, 1.0);
    let drawn = read_texture(&gpu, view.frame().expect("drawn"));
    assert_eq!(differences(&drawn, &shell, rect, 1.0, 0).1, 0, "the pane is its frame");
    let (_, moved_from, _) = differences(&before, &shell, egui::Rect::ZERO, 1.0, 0);
    assert!(moved_from > 500, "the move shows: {moved_from} pixels differ");
    let (capture_stage, frame) = capture_zone(&gpu, pixels, outside, &[opened, selected, moved]);
    is_a_picture(&frame, "zone_moved");
    let (most, count, all) = differences(&frame, &shell, rect, 1.0, 0);
    println!("after a drag: {count} of {all} pixels differ from a capture, by at most {most}");
    assert_runs_agree(most, count, "after a drag");
    assert_eq!(capture_stage.renderer.stats().pipelines.first_use_builds, 0);

    harness.state_mut().undo();
    harness.step();
    let current = harness.state().current().expect("open");
    assert_eq!(current.text().as_bytes(), file.as_bytes(), "undo is the file's bytes");
    assert!(!current.is_dirty());
}

/// What the zone panel and a drag of the gizmo cost against the editor's frame: a pass of egui, the stage's frame
/// and egui's paint, with the GPU's share waited for.
#[test]
#[ignore = "needs a GPU and an editor's cook, and prints costs"]
fn what_the_zone_panel_costs() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let (mut harness, _) = zone_harness(&painter, "zone_costs", (1500.0, 960.0), 2.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    harness.state_mut().select_placement(Some(CART));
    settle(&mut harness, &mut view, &gpu, &painter);
    let frames = 300;
    let cart = sapphire_math::Vec3::new(9.0, 0.0, 10.0);
    let time = |harness: &mut Harness<'_, Editor>, view: &mut StageView, drag: bool| {
        let (mut passes, mut all) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        let (from, past) = (on_arrow(harness, cart, Axis::X, 0.0), on_arrow(harness, cart, Axis::X, 0.5));
        if drag {
            harness.hover_at(from);
            harness.step();
            harness.drag_at(from);
            harness.step();
        }
        for at in 0..frames {
            if drag {
                harness.hover_at(if at % 2 == 0 { past } else { from });
            }
            let started = Stopwatch::start();
            harness.step();
            passes.push(started.elapsed());
            view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
            drop(painter.paint_only(harness));
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("polls");
            all.push(started.elapsed());
        }
        if drag {
            harness.drop_at(from);
            harness.step();
        }
        passes.sort();
        all.sort();
        let pick =
            |times: &[std::time::Duration]| (times[frames / 2], times[frames * 99 / 100], times[frames - 1]);
        (pick(&passes), pick(&all))
    };
    harness.state_mut().shell_mut().open(Pane::Moves);
    let (pass, idle) = time(&mut harness, &mut view, false);
    println!(
        "a zone, nothing changing, its pane behind a tab: the pass {pass:?}, the frame {idle:?} (median, p99, worst)"
    );
    harness.state_mut().shell_mut().open(Pane::Zone);
    let (pass, listed) = time(&mut harness, &mut view, false);
    println!("with the zone pane in front: the pass {pass:?}, the frame {listed:?}");
    harness.state_mut().shell_mut().open(Pane::Viewport);
    let (pass, dragging) = time(&mut harness, &mut view, true);
    println!("dragging the gizmo, an edit and a frame each: the pass {pass:?}, the frame {dragging:?}");
    let pipelines = view.stage().expect("stood").renderer.stats().pipelines;
    assert_eq!(pipelines.first_use_builds, 0);
}

// The sequencer panel (M3 E5).

fn square_look_path() -> PathBuf {
    root().join("content/sequences/square_look.timeline.ron")
}

/// An editor on the square's look, with the viewport's tab in front.
fn timeline_harness(painter: &Painter, size: (f32, f32), scale: f32) -> Harness<'static, Editor> {
    let options = Options { file: Some(square_look_path()), content: Some(cook()), link: None };
    let mut editor = Editor::new(options);
    editor.shell_mut().open(Pane::Viewport);
    harness_on(painter, editor, size, scale)
}

/// The timeline as the editor cooked it.
fn timeline_shown(harness: &mut Harness<'_, Editor>) -> TimelineShown {
    harness.state_mut().viewport_frame().timeline.expect("the timeline cooks")
}

/// A fresh stage playing the timeline the editor has, through `frames` in turn, each round drawn as many frames as a
/// change settles for and the frames between played with nothing drawn: what a capture at those times draws.
fn capture_timeline(
    gpu: &Arc<GpuContext>,
    timeline: &Arc<sapphire_timeline::Timeline>,
    size: (u32, u32),
    frames: &[u32],
) -> (Stage<ViewShow>, Image) {
    let mut stage = stand(&host(), gpu).expect("stands");
    play_timeline(&mut stage, Arc::clone(timeline));
    let at = std::cell::Cell::new(0);
    let image = capture(&mut stage, &vec![size; frames.len()], |stage, round| {
        play_frames(stage, frames[round] - at.get(), size);
        at.set(frames[round]);
    });
    (stage, image)
}

/// Scrubbing shows the frame a capture at that time does, forward and back, and the timeline's camera at a playhead
/// is the camera of a stage that drew every frame to it. Nothing's built.
#[test]
#[ignore = "needs a GPU and a cook"]
fn scrubbing_shows_what_a_capture_at_that_time_does() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = timeline_harness(&painter, (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    assert_eq!(harness.state().playhead(), Some(0));
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let (_, pixels) = on_pixels(rect, 1.0);
    let timeline = timeline_shown(&mut harness).timeline;
    let at_start = painter.paint(&gpu, &harness);

    // Forward: it plays on from where it was.
    harness.state_mut().scrub_to(150);
    settle(&mut harness, &mut view, &gpu, &painter);
    let shell = painter.paint(&gpu, &harness);
    let drawn = read_texture(&gpu, view.frame().expect("drawn"));
    assert_eq!(differences(&drawn, &shell, rect, 1.0, 0).1, 0, "the pane is its frame");
    let (_, moved, _) = differences(&at_start, &shell, egui::Rect::ZERO, 1.0, 0);
    assert!(moved > 10_000, "2.5 s in is another shot: {moved} pixels differ");
    let (capture_stage, frame) = capture_timeline(&gpu, &timeline, pixels, &[0, 150]);
    is_a_picture(&frame, "timeline_150");
    let (most, count, all) = differences(&frame, &shell, rect, 1.0, 0);
    println!("at frame 150: {count} of {all} pixels differ from a capture, by at most {most}");
    assert_runs_agree(most, count, "scrubbed forward");
    assert_eq!(capture_stage.renderer.stats().pipelines.first_use_builds, 0);

    // Back: it starts over on a fresh stage.
    harness.state_mut().scrub_to(60);
    settle(&mut harness, &mut view, &gpu, &painter);
    let shell = painter.paint(&gpu, &harness);
    let (_, frame) = capture_timeline(&gpu, &timeline, pixels, &[60]);
    let (most, count, all) = differences(&frame, &shell, rect, 1.0, 0);
    println!("back at frame 60: {count} of {all} pixels differ from a capture, by at most {most}");
    assert_runs_agree(most, count, "scrubbed back");
    let stage = view.stage().expect("stood");
    assert_eq!(stage.renderer.stats().pipelines.first_use_builds, 0, "{}", stage.renderer.first_use_report());

    // The frames played with nothing drawn leave the camera where drawing every one of them does.
    let aspect = pixels.0 as f32 / pixels.1 as f32;
    let seen = view.stage().expect("stood").scene.camera().camera_data(aspect);
    let mut every = stand(&host(), &gpu).expect("stands");
    play_timeline(&mut every, Arc::clone(&timeline));
    let target = every.target(pixels.0, pixels.1, wgpu::TextureUsages::empty());
    let target = target.create_view(&wgpu::TextureViewDescriptor::default());
    for _ in 0..60 {
        every.show.fight.show.drive_camera(1.0 / sapphire_editor::app::RATE, sapphire_math::Vec2::ZERO, 0.0);
        every.render(&target, 1.0, |_, _| {}).expect("draws");
        every.scene.end_frame();
    }
    let drew = every.scene.camera().camera_data(aspect);
    assert_eq!((seen.position, seen.view, seen.proj), (drew.position, drew.view, drew.proj));
}

/// What the sequencer costs against the editor's frame: a pass with its pane in front, Play's frames, and a scrub
/// back, which stands a fresh stage and plays to the playhead.
#[test]
#[ignore = "needs a GPU and a cook, and prints costs"]
fn what_the_sequencer_costs() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = timeline_harness(&painter, (1500.0, 960.0), 2.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let frames = 120;
    let time = |harness: &mut Harness<'_, Editor>, view: &mut StageView, act: &dyn Fn(&mut Editor, usize)| {
        let (mut passes, mut all) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        for at in 0..frames {
            act(harness.state_mut(), at);
            let started = Stopwatch::start();
            harness.step();
            passes.push(started.elapsed());
            view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
            drop(painter.paint_only(harness));
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("polls");
            all.push(started.elapsed());
        }
        passes.sort();
        all.sort();
        let pick =
            |times: &[std::time::Duration]| (times[frames / 2], times[frames * 99 / 100], times[frames - 1]);
        (pick(&passes), pick(&all))
    };
    let (pass, idle) = time(&mut harness, &mut view, &|_, _| {});
    println!("a timeline, nothing changing: the pass {pass:?}, the frame {idle:?} (median, p99, worst)");
    let (pass, forward) = time(&mut harness, &mut view, &|editor, at| editor.scrub_to(at as u32 + 1));
    println!("scrubbing forward a frame a pass: the pass {pass:?}, the frame {forward:?}");
    harness.state_mut().scrub_to(239);
    harness.step();
    view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
    for back in [200, 120, 0] {
        harness.state_mut().scrub_to(back);
        harness.step();
        let started = Stopwatch::start();
        view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
        println!("a scrub back to frame {back}, a fresh stage and the frames to it: {:?}", started.elapsed());
    }
}

// The screen panel (M3 E5).

/// An editor on the game's screen `content/ui/<stem>.ui.ron`, with the viewport's tab in front, its paths reading
/// the screen's sample model, which its goldens show.
fn screen_harness(painter: &Painter, stem: &str, size: (f32, f32), scale: f32) -> Harness<'static, Editor> {
    let file = root().join(format!("content/ui/{stem}.ui.ron"));
    let options = Options { file: Some(file), content: Some(cook()), link: None };
    let mut editor = Editor::new(options);
    editor.shell_mut().open(Pane::Viewport);
    harness_on(painter, editor, size, scale)
}

/// A committed golden, as RGBA.
fn golden(name: &str) -> Image {
    let path = root().join(format!("tests/golden/{name}.metal.png"));
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).expect("the golden")));
    let mut reader = decoder.read_info().expect("a png");
    let mut buffer = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut buffer).expect("its pixels");
    let pixels = match info.color_type {
        png::ColorType::Rgba => buffer[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            buffer[..info.buffer_size()].chunks(3).flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255]).collect()
        }
        other => panic!("{name}: {other:?}"),
    };
    Image { width: info.width, pixels }
}

/// The largest difference of a channel between two frames of one size, and how many pixels differ at all.
fn against(frame: &Image, other: &Image) -> (u8, usize) {
    assert_eq!((frame.width, frame.pixels.len()), (other.width, other.pixels.len()), "one size");
    let (mut most, mut count) = (0, 0);
    for (a, b) in frame.pixels.chunks(4).zip(other.pixels.chunks(4)) {
        let difference = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        most = most.max(difference);
        count += usize::from(difference > 0);
    }
    (most, count)
}

/// What a golden of the game's screens calls an aspect.
fn golden_aspect(aspect: Aspect) -> &'static str {
    match aspect {
        Aspect::Standard => "tablet",
        Aspect::Wide => "wide",
        Aspect::Ultrawide => "ultrawide",
    }
}

/// Each of the game's screens at each of the pane's three aspects, in English and in Japanese, is what the sandbox's
/// capture of it at that size is (`--ui-plain --ui-game <screen>`): the committed Metal golden. The title draws its
/// name from the display face's field (M3 U8), and the character screen shows the character in its inset (M3 U6).
/// The pane shows the frame whole, its shape kept. Opening a screen built what it draws through, and changing
/// aspect or language builds nothing more.
#[test]
#[ignore = "needs a GPU and a cook"]
fn a_screen_shows_what_its_golden_does() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut differing = Vec::new();
    for stem in ["title", "pause", "settings", "character", "dialogue"] {
        let mut harness = screen_harness(&painter, stem, (1200.0, 700.0), 1.0);
        let mut view = StageView::default();
        let mut built = None;
        for language in ["en", "ja"] {
            for aspect in Aspect::ALL {
                let name = format!("game-{stem}-{}-{language}", golden_aspect(aspect));
                harness.state_mut().preview_at(aspect);
                harness.state_mut().preview_in(language);
                settle(&mut harness, &mut view, &gpu, &painter);
                assert_eq!(harness.state_mut().viewport().note(), None, "{name}");
                let frame = read_texture(&gpu, view.frame().expect("drawn"));
                // A screen on a flat color has few colours: the golden says what it is.
                keep(&frame, &name);
                let (most, count) = against(&frame, &golden(&name));
                let all = frame.pixels.len() / 4;
                println!("{name}: {count} of {all} pixels differ from the golden, by at most {most}");
                if count > 0 {
                    differing.push(name.clone());
                }
                let shown = harness.state_mut().viewport().frame_rect().expect("shown");
                let (width, height) = aspect.size();
                let ratio = (shown.width() / shown.height()) / (width as f32 / height as f32);
                assert!((ratio - 1.0).abs() < 0.01, "{name}: shown whole, its shape kept ({shown:?})");
                let stage = view.stage().expect("stood");
                let pipelines = stage.renderer.stats().pipelines;
                assert_eq!(pipelines.first_use_builds, 0, "{name}: {}", stage.renderer.first_use_report());
                assert_eq!(*built.get_or_insert(pipelines), pipelines, "{name}: nothing more's built");
            }
        }
    }
    assert_eq!(differing, Vec::<String>::new(), "each preview is its capture");
}

/// An edit of the screen's document shows in the preview as the document reads, with nothing loaded or built, and
/// undo shows what was there to the pixel. A value typed in for a path shows too.
#[test]
#[ignore = "needs a GPU and a cook"]
fn an_edit_shows_in_the_preview() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = screen_harness(&painter, "settings", (1200.0, 700.0), 1.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let before = read_texture(&gpu, view.frame().expect("drawn"));
    let opened = view.stage().expect("stood").renderer.stats().pipelines;

    let id = harness.state().current().expect("open").id();
    let gap: PropertyPath = "nodes[1].layout.gap".parse().expect("a path");
    let document = harness.state_mut().documents_mut().get_mut::<UiDocument>(id).expect("a screen");
    document.set_text("an edit", &gap, "24.0").expect("taken");
    settle(&mut harness, &mut view, &gpu, &painter);
    assert_eq!(harness.state_mut().viewport().note(), None);
    let edited = read_texture(&gpu, view.frame().expect("drawn"));
    let (_, moved) = against(&edited, &before);
    println!("a wider gap moves {moved} pixels");
    assert!(moved > 1000, "the edit shows: {moved} pixels differ");
    assert_eq!(view.stage().expect("stood").renderer.stats().pipelines, opened, "an edit built nothing");

    harness.state_mut().undo();
    settle(&mut harness, &mut view, &gpu, &painter);
    let undone = read_texture(&gpu, view.frame().expect("drawn"));
    assert_eq!(against(&undone, &before), (0, 0), "undo shows what was there");

    assert!(harness.state_mut().set_stand_in("sound", StandIn::Number(30.0)));
    settle(&mut harness, &mut view, &gpu, &painter);
    let typed = read_texture(&gpu, view.frame().expect("drawn"));
    let (_, changed) = against(&typed, &before);
    assert!(changed > 0, "a typed value shows");
    assert_eq!(view.stage().expect("stood").renderer.stats().pipelines, opened, "and builds nothing");
}

/// What the screen pane and its preview cost against the editor's frame: a pass of egui, the stage's frame and
/// egui's paint, with the GPU's share waited for; at rest, and with the document edited every frame.
#[test]
#[ignore = "needs a GPU and a cook, and prints costs"]
fn what_the_screen_panel_costs() {
    let gpu = device();
    let painter = Painter::new(&gpu, PLAIN);
    let mut harness = screen_harness(&painter, "settings", (1500.0, 960.0), 2.0);
    let mut view = StageView::default();
    settle(&mut harness, &mut view, &gpu, &painter);
    let id = harness.state().current().expect("open").id();
    let gap: PropertyPath = "nodes[1].layout.gap".parse().expect("a path");
    let frames = 300;
    let time = |harness: &mut Harness<'_, Editor>, view: &mut StageView, edit: bool| {
        let (mut passes, mut all) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
        for at in 0..frames {
            if edit {
                let document =
                    harness.state_mut().documents_mut().get_mut::<UiDocument>(id).expect("a screen");
                let gap_now = if at % 2 == 0 { "24.0" } else { "8.0" };
                document.set_text("an edit", &gap, gap_now).expect("taken");
            }
            let started = Stopwatch::start();
            harness.step();
            passes.push(started.elapsed());
            view.draw(&gpu, &mut painter.renderer.write(), harness.state_mut().viewport_frame());
            drop(painter.paint_only(harness));
            gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("polls");
            all.push(started.elapsed());
        }
        passes.sort();
        all.sort();
        let pick =
            |times: &[std::time::Duration]| (times[frames / 2], times[frames * 99 / 100], times[frames - 1]);
        (pick(&passes), pick(&all))
    };
    harness.state_mut().shell_mut().open(Pane::Screen);
    let (pass, idle) = time(&mut harness, &mut view, false);
    println!(
        "a screen at rest, its pane in front: the pass {pass:?}, the frame {idle:?} (median, p99, worst)"
    );
    let (pass, editing) = time(&mut harness, &mut view, true);
    println!("an edit every frame, a new tree and a frame each: the pass {pass:?}, the frame {editing:?}");
    assert_eq!(view.stage().expect("stood").renderer.stats().pipelines.first_use_builds, 0);
}
