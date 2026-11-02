//! The editor's window: `sapphire_platform`'s SDL window, `sapphire_rhi`'s surface and `egui-wgpu`, as
//! the sandbox's overlay draws. No test opens it, and agents never launch it: everything worth testing is
//! in [`sapphire_editor::Editor`], which this only hands a `Ui` and frames.

use std::time::Duration;

use sapphire_editor::cli::Command;
use sapphire_editor::input::WindowInput;
use sapphire_editor::{Editor, Options};
use sapphire_platform::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, Platform, TextInputKind, Window, WindowDesc,
    WindowEvent,
};
use sapphire_rhi::{
    GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, PresentPolicy, RenderSurface, SkipReason,
    SurfaceAcquire, SurfaceTarget, wgpu,
};
use sapphire_time::Stopwatch;

/// How long a frame with nothing to do sleeps, so an open editor costs next to nothing.
const IDLE: Duration = Duration::from_millis(4);

/// What draws into the window. All of it is made again if the device is lost.
struct Device {
    // Field order is drop order: the surface goes before the context it came from.
    surface: RenderSurface,
    renderer: egui_wgpu::Renderer,
    gpu: GpuContext,
}

impl Device {
    fn new(window: &Window) -> Result<Self, AppInitError> {
        let instance = GpuInstance::new(&GpuInstanceDesc::default())?;
        let handle = window.surface_handle()?;
        let target = if let Some(layer) = handle.metal_layer() {
            SurfaceTarget::CoreAnimationLayer(layer)
        } else {
            SurfaceTarget::Raw {
                window: handle.window_handle()?.as_raw(),
                display: handle.display_handle()?.as_raw(),
            }
        };
        // SAFETY: the surface owns `handle`, which keeps the native window alive, and drops it right after
        // itself.
        let mut surface = unsafe { RenderSurface::new(&instance, target, handle)? };
        let gpu = GpuContext::new(&instance, &GpuContextDesc { pipeline_cache: None }, Some(&surface))?;
        let size = window.pixel_size();
        surface.configure(&gpu, size.width, size.height, PresentPolicy::Fifo)?;
        let format = surface.format().ok_or_else(|| AppInitError::msg("a surface with no format"))?;
        let options = egui_wgpu::RendererOptions {
            msaa_samples: 1,
            depth_stencil_format: None,
            ..egui_wgpu::RendererOptions::default()
        };
        let renderer = egui_wgpu::Renderer::new(gpu.device(), format, options);
        Ok(Self { surface, renderer, gpu })
    }
}

/// The app `sapphire_platform` runs.
pub struct EditorApp {
    editor: Editor,
    ctx: egui::Context,
    input: WindowInput,
    window: Window,
    device: Option<Device>,
    clock: Stopwatch,
    /// When egui next asked for a pass, in seconds.
    next_pass: f64,
    /// Texture changes for the next frame that draws.
    textures: egui::TexturesDelta,
    to_free: Vec<egui::TextureId>,
}

impl App for EditorApp {
    const METADATA: AppMetadata = AppMetadata {
        name: "Sapphire Timeline",
        organization: "Sapphire",
        identifier: "dev.sapphire.editor",
        version: env!("CARGO_PKG_VERSION"),
    };

    fn init(platform: &mut Platform) -> Result<Self, AppInitError> {
        let Command::Window(options) =
            sapphire_editor::cli::parse_command_line(platform.args()).map_err(AppInitError::msg)?
        else {
            return Err(AppInitError::msg("a capture opens no window"));
        };
        let window = platform.create_window(&WindowDesc {
            title: Self::METADATA.name.to_owned(),
            size: sapphire_platform::Size { width: 1500, height: 960 },
            ..WindowDesc::default()
        })?;
        let device = Device::new(&window)?;
        Ok(Self::new(options, window, device))
    }

    fn event(&mut self, _platform: &mut Platform, event: &Event) -> AppControl {
        match event {
            Event::Quit { .. } | Event::Window { event: WindowEvent::CloseRequested, .. } => {
                self.editor.request_quit();
            }
            Event::Window { event: WindowEvent::PixelSizeChanged { width, height }, .. } => {
                if let Some(device) = &mut self.device {
                    device.surface.resize(*width, *height);
                }
                self.next_pass = 0.0;
            }
            _ => {}
        }
        self.input.push(event);
        if self.editor.may_quit() { AppControl::Exit } else { AppControl::Continue }
    }

    fn iterate(&mut self, _platform: &mut Platform) -> AppControl {
        let now = self.clock.elapsed().as_secs_f64();
        if !self.input.woken() && now < self.next_pass {
            std::thread::sleep(IDLE);
            return AppControl::Continue;
        }
        self.frame(now);
        if self.editor.may_quit() { AppControl::Exit } else { AppControl::Continue }
    }
}

impl Drop for EditorApp {
    fn drop(&mut self) {
        // Frames that couldn't draw, a covered window's say, leave texture changes here, and in debug builds
        // egui panics on dropping any it thinks are lost.
        self.textures.clear();
    }
}

impl EditorApp {
    fn new(options: Options, window: Window, device: Device) -> Self {
        Self {
            editor: Editor::new(options),
            ctx: egui::Context::default(),
            input: WindowInput::default(),
            window,
            device: Some(device),
            clock: Stopwatch::start(),
            next_pass: 0.0,
            textures: egui::TexturesDelta::default(),
            to_free: Vec::new(),
        }
    }

    fn frame(&mut self, now: f64) {
        let pixels = self.window.pixel_size();
        let scale = self.window.display_scale().max(0.1);
        self.input.set_points_per_unit(self.window.pixel_density() / scale);
        let events = self.input.take();
        let released = self.input.take_released();
        let size = egui::vec2(pixels.width as f32, pixels.height as f32) / scale;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(now),
            events,
            ..egui::RawInput::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
        let editor = &mut self.editor;
        let output = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        if released {
            // egui has no event for this: a button down when focus went stays down, and a release would
            // click what's under it (`sapphire_debug`'s `reset_pointer` says more). A drag then ends with
            // the next pass, which sees no button down.
            self.ctx
                .input_mut_for(egui::ViewportId::ROOT, |input| input.pointer = egui::PointerState::default());
            self.ctx.stop_dragging();
        }

        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::ZERO, |viewport| viewport.repaint_delay);
        self.next_pass = now + delay.min(Duration::from_secs(1)).as_secs_f64();
        let wants_text = self.ctx.egui_wants_keyboard_input();
        if wants_text != self.window.is_text_input_active() {
            if wants_text {
                self.window.start_text_input(TextInputKind::Verbatim);
            } else {
                self.window.stop_text_input();
            }
        }
        self.textures.append(output.textures_delta);
        let pixels_per_point = output.pixels_per_point;
        let primitives = self.ctx.tessellate(output.shapes, pixels_per_point);
        self.draw(&primitives, pixels_per_point);
    }

    fn draw(&mut self, primitives: &[egui::ClippedPrimitive], pixels_per_point: f32) {
        let Some(device) = &mut self.device else { return };
        let Device { surface, renderer, gpu } = device;
        let lost = match surface.acquire() {
            SurfaceAcquire::Frame(frame) => {
                let (width, height) = frame.size();
                let screen =
                    egui_wgpu::ScreenDescriptor { size_in_pixels: [width, height], pixels_per_point };
                for id in self.to_free.drain(..) {
                    renderer.free_texture(&id);
                }
                for (id, deltas) in &self.textures.set {
                    for delta in deltas {
                        renderer.update_texture(gpu.device(), gpu.queue(), *id, delta);
                    }
                }
                self.textures.set.clear();
                self.to_free.extend(self.textures.free.drain());
                let mut encoder =
                    gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                renderer.update_buffers(gpu.device(), gpu.queue(), &mut encoder, primitives, &screen);
                {
                    let mut pass = encoder
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("editor"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: frame.view(),
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        })
                        .forget_lifetime();
                    renderer.render(&mut pass, primitives, &screen);
                }
                gpu.queue().submit([encoder.finish()]);
                frame.present();
                false
            }
            // A lost surface stays lost until it's made again from a fresh handle.
            SurfaceAcquire::Skip(reason) => matches!(reason, SkipReason::DeviceLost | SkipReason::Lost),
        };
        if lost {
            // Everything GPU goes, and egui's textures with it: a new context sends them again.
            self.device = None;
            self.to_free.clear();
            self.textures.clear();
            self.ctx = egui::Context::default();
            match Device::new(&self.window) {
                Ok(device) => self.device = Some(device),
                Err(error) => tracing::error!(%error, "the GPU device was lost, and no new one came"),
            }
        }
    }
}
