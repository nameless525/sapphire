//! The engine's half of a viewport pane: a texture a frame draws into, under the two views that frame and egui
//! each want. E3 draws the real renderer into it, on the window's device. `tests/viewport.rs` holds what makes
//! it right, on a real GPU and with no window.

use sapphire_rhi::wgpu;

/// The format a viewport draws in: an sRGB target, which encodes what post returns (`post.wgsl`).
pub const VIEWPORT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The format egui samples it in. `egui_wgpu` takes a native texture's values as sRGB-encoded already, and
/// documents `Rgba8Unorm` only, so it needs a plain view of the same bytes. An sRGB view decodes on
/// sampling, and egui then treats the linear values as encoded: the frame comes out too dark.
pub const VIEWPORT_SAMPLED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Where in a pane a viewport shows, and the size of texture that fills it: on whole pixels, since a pane's edges
/// fall between them when its shares do, and a frame sampled off the grid moves every pixel in it.
#[must_use]
pub fn on_pixels(pane: egui::Rect, pixels_per_point: f32) -> (egui::Rect, (u32, u32)) {
    use egui::emath::GuiRounding as _;
    let rect = pane.round_to_pixels(pixels_per_point);
    let size = rect.size() * pixels_per_point;
    (rect, (size.x.round() as u32, size.y.round() as u32))
}

/// The texture a viewport's frame draws into.
#[derive(Debug)]
pub struct ViewportTarget {
    pub texture: wgpu::Texture,
    /// What the engine renders to.
    pub render_view: wgpu::TextureView,
    /// What egui samples.
    pub sampled_view: wgpu::TextureView,
}

impl ViewportTarget {
    #[must_use]
    pub fn new(device: &wgpu::Device, (width, height): (u32, u32)) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: VIEWPORT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[VIEWPORT_SAMPLED_FORMAT],
        });
        let render_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampled_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(VIEWPORT_SAMPLED_FORMAT),
            ..wgpu::TextureViewDescriptor::default()
        });
        Self { texture, render_view, sampled_view }
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.texture.width(), self.texture.height())
    }
}
