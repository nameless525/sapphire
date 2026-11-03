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
use egui_kittest::{Harness, TestRenderer};
use egui_wgpu::RendererOptions;
use sapphire_editor::edits::{EntryRef, Track, set_window};
use sapphire_editor::recovery::Recovery;
use sapphire_editor::shell::Pane;
use sapphire_editor::viewport::{OutsideView, SETTLING_FRAMES, StageView, ViewShow, on_pixels, stand};
use sapphire_editor::{Editor, Options};
use sapphire_platform::Host;
use sapphire_rhi::{GpuContext, wgpu};
use sapphire_stage::{Readback, Stage, headless_gpu};
use sapphire_time::Stopwatch;

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
    let (_, gpu) = headless_gpu().expect("a GPU");
    Arc::new(gpu)
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
    let mut live = sapphire_gameplay::replay::LiveMoves::default();
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
    let mut recovery = Recovery::new(Stack::new().expect("a stack"));
    let mut harness = {
        let stack = recovery.ready().expect("made");
        editor_harness(&stack.painter, (1200.0, 700.0), 1.0)
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
    let mut harness = harness_on(painter, editor, (1200.0, 700.0), 1.0);
    settle(&mut harness, view, gpu, painter);
    assert_eq!(view.frames(), u64::from(SETTLING_FRAMES));
    assert_eq!(harness.state_mut().viewport().note(), None);
    assert_eq!(text_now(harness.state()), edited);
    assert!(harness.state().is_dirty());

    let shell = painter.paint(gpu, &harness);
    let rect = harness.state_mut().viewport().rect().expect("shown");
    let (_, pixels) = on_pixels(rect, 1.0);
    let mut stage = capture_stage(gpu, &mut harness);
    let frame = capture(&mut stage, &[pixels], |_, _| ());
    let (most, count, all) = differences(&frame, &shell, rect, 1.0, 0);
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
