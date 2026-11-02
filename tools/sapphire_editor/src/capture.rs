//! The editor drawn to a PNG with no window and no GPU: what tests read, what `--capture` writes, and how
//! an agent looks at this interface at all.
//!
//! egui tessellates to plain triangles with vertex colours and one font atlas, so a small software
//! rasterizer draws them the way `egui-wgpu` does: premultiplied colours, blended in gamma space, nearest
//! sampling at one pixel per point being all a capture needs. It isn't a golden: its job is to be read.

use std::collections::HashMap;
use std::path::Path;

use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Color32, Context, FullOutput, ImageData, Pos2, RawInput, Rect, TextureId, TexturesDelta, Vec2};

/// An image of a frame: RGBA, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Capture {
    /// Writes it as a PNG.
    pub fn write_png(&self, path: &Path) -> anyhow::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut encoder = png::Encoder::new(file, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&self.pixels)?;
        Ok(())
    }

    /// A pixel's colour.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * self.width as usize + x as usize) * 4;
        [self.pixels[at], self.pixels[at + 1], self.pixels[at + 2], self.pixels[at + 3]]
    }
}

/// The textures egui has made, kept from pass to pass.
#[derive(Debug, Default)]
pub struct Textures {
    images: HashMap<TextureId, Atlas>,
}

#[derive(Debug)]
struct Atlas {
    size: [usize; 2],
    pixels: Vec<Color32>,
}

impl Textures {
    /// Takes a pass's changes.
    pub fn apply(&mut self, delta: &TexturesDelta) {
        for (id, changes) in &delta.set {
            for change in changes {
                let ImageData::Color(image) = &change.image;
                match change.pos {
                    None => {
                        self.images.insert(*id, Atlas { size: image.size, pixels: image.pixels.clone() });
                    }
                    Some([x, y]) => {
                        if let Some(atlas) = self.images.get_mut(id) {
                            for row in 0..image.size[1] {
                                let from = row * image.size[0];
                                let to = (y + row) * atlas.size[0] + x;
                                atlas.pixels[to..to + image.size[0]]
                                    .copy_from_slice(&image.pixels[from..from + image.size[0]]);
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.images.remove(id);
        }
    }

    fn sample(&self, id: TextureId, uv: Pos2) -> Color32 {
        let Some(atlas) = self.images.get(&id) else { return Color32::WHITE };
        let x = ((uv.x * atlas.size[0] as f32) as usize).min(atlas.size[0] - 1);
        let y = ((uv.y * atlas.size[1] as f32) as usize).min(atlas.size[1] - 1);
        atlas.pixels[y * atlas.size[0] + x]
    }
}

/// Input for a frame of `width` × `height` points, `dt` seconds after the last.
#[must_use]
pub fn raw_input(width: u32, height: u32, time: f64) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width as f32, height as f32))),
        time: Some(time),
        ..RawInput::default()
    }
}

/// Runs one pass of `ctx` and rasterizes it.
pub fn draw(
    ctx: &Context,
    textures: &mut Textures,
    input: RawInput,
    width: u32,
    height: u32,
    ui: impl FnMut(&mut egui::Ui),
) -> Capture {
    let output = ctx.run_ui(input, ui);
    rasterize(ctx, textures, output, width, height)
}

/// Rasterizes a finished pass, at one pixel per point.
#[must_use]
pub fn rasterize(ctx: &Context, textures: &mut Textures, mut output: FullOutput, width: u32, height: u32) -> Capture {
    textures.apply(&output.textures_delta);
    // Taken: epaint asserts that no texture change is dropped by accident.
    output.textures_delta.clear();
    let primitives = ctx.tessellate(output.shapes, 1.0);
    let mut canvas = Canvas { width, height, pixels: vec![[0u8, 0, 0, 255]; (width * height) as usize] };
    for primitive in &primitives {
        canvas.primitive(primitive, textures);
    }
    Capture {
        width,
        height,
        pixels: canvas.pixels.iter().flatten().copied().collect(),
    }
}

struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}

impl Canvas {
    fn primitive(&mut self, primitive: &ClippedPrimitive, textures: &Textures) {
        let Primitive::Mesh(mesh) = &primitive.primitive else { return };
        let clip = primitive.clip_rect;
        for triangle in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|index| &mesh.vertices[index as usize]);
            self.triangle(a, b, c, clip, mesh.texture_id, textures);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn triangle(
        &mut self,
        a: &egui::epaint::Vertex,
        b: &egui::epaint::Vertex,
        c: &egui::epaint::Vertex,
        clip: Rect,
        texture: TextureId,
        textures: &Textures,
    ) {
        let area = edge(a.pos, b.pos, c.pos);
        if area == 0.0 {
            return;
        }
        let left = a.pos.x.min(b.pos.x).min(c.pos.x).max(clip.left()).max(0.0).floor() as u32;
        let right = a.pos.x.max(b.pos.x).max(c.pos.x).min(clip.right()).min(self.width as f32).ceil() as u32;
        let top = a.pos.y.min(b.pos.y).min(c.pos.y).max(clip.top()).max(0.0).floor() as u32;
        let bottom = a.pos.y.max(b.pos.y).max(c.pos.y).min(clip.bottom()).min(self.height as f32).ceil() as u32;
        for y in top..bottom {
            for x in left..right {
                let at = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let weights = [edge(b.pos, c.pos, at) / area, edge(c.pos, a.pos, at) / area, edge(a.pos, b.pos, at) / area];
                if weights.iter().any(|weight| *weight < 0.0) {
                    continue;
                }
                let blend = |values: [f32; 3]| values[0] * weights[0] + values[1] * weights[1] + values[2] * weights[2];
                let uv = Pos2::new(blend([a.uv.x, b.uv.x, c.uv.x]), blend([a.uv.y, b.uv.y, c.uv.y]));
                let sampled = textures.sample(texture, uv);
                let channel = |index: usize| {
                    blend([
                        f32::from(a.color.to_array()[index]),
                        f32::from(b.color.to_array()[index]),
                        f32::from(c.color.to_array()[index]),
                    ]) * f32::from(sampled.to_array()[index])
                        / 255.0
                };
                let source = [channel(0), channel(1), channel(2), channel(3)];
                let pixel = &mut self.pixels[(y * self.width + x) as usize];
                let keep = 1.0 - source[3] / 255.0;
                for index in 0..3 {
                    pixel[index] = (source[index] + f32::from(pixel[index]) * keep).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

fn edge(a: Pos2, b: Pos2, point: Pos2) -> f32 {
    (b.x - a.x) * (point.y - a.y) - (b.y - a.y) * (point.x - a.x)
}

#[cfg(test)]
mod tests {
    use egui::{CentralPanel, Frame};

    use super::*;

    /// A filled panel comes out its colour, and text marks the pixels it covers, so a capture says something.
    #[test]
    fn a_frame_rasterizes_with_its_fill_and_its_text() {
        let ctx = Context::default();
        let mut textures = Textures::default();
        let run = |ctx: &Context, textures: &mut Textures, time| {
            draw(ctx, textures, raw_input(200, 60, time), 200, 60, |ui| {
                CentralPanel::no_frame().frame(Frame::NONE.fill(Color32::from_rgb(10, 20, 30))).show(ui, |ui| {
                    ui.label("Sapphire");
                });
            })
        };
        // egui's first pass measures; the second draws what it measured.
        run(&ctx, &mut textures, 0.0);
        let capture = run(&ctx, &mut textures, 0.1);
        assert_eq!(capture.pixel(199, 59), [10, 20, 30, 255]);
        let marked = capture.pixels.chunks_exact(4).filter(|pixel| pixel[0] > 100).count();
        assert!(marked > 20, "{marked} pixels of text");
    }
}
