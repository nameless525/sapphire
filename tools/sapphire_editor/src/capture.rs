//! The editor drawn to a PNG with no window and no GPU: what tests read, what `--capture` writes, and how
//! an agent looks at this interface at all.
//!
//! egui tessellates to plain triangles with vertex colours and one font atlas, so a small software
//! rasterizer draws them the way `egui-wgpu` does: premultiplied colours, blended in gamma space, nearest
//! sampling at one pixel per point being all a capture needs. It isn't a golden: its job is to be read, and
//! a GPU window's edges and text can differ from it by a pixel. Each pixel belongs to one triangle of a
//! shape, as it does on a GPU, so a seam in a capture is a seam in what was drawn.

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
pub fn rasterize(
    ctx: &Context,
    textures: &mut Textures,
    mut output: FullOutput,
    width: u32,
    height: u32,
) -> Capture {
    textures.apply(&output.textures_delta);
    // Taken: epaint asserts that no texture change is dropped by accident.
    output.textures_delta.clear();
    let primitives = ctx.tessellate(output.shapes, 1.0);
    let mut canvas = Canvas { width, height, pixels: vec![[0u8, 0, 0, 255]; (width * height) as usize] };
    for primitive in &primitives {
        canvas.primitive(primitive, textures);
    }
    Capture { width, height, pixels: canvas.pixels.iter().flatten().copied().collect() }
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
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|index| &mesh.vertices[index as usize]);
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
        // One winding from here on, so two triangles sharing an edge walk it in opposite directions.
        let (b, c, area) = if area > 0.0 { (b, c, area) } else { (c, b, -area) };
        let left = a.pos.x.min(b.pos.x).min(c.pos.x).max(clip.left()).max(0.0).floor() as u32;
        let right = a.pos.x.max(b.pos.x).max(c.pos.x).min(clip.right()).min(self.width as f32).ceil() as u32;
        let top = a.pos.y.min(b.pos.y).min(c.pos.y).max(clip.top()).max(0.0).floor() as u32;
        let bottom =
            a.pos.y.max(b.pos.y).max(c.pos.y).min(clip.bottom()).min(self.height as f32).ceil() as u32;
        for y in top..bottom {
            for x in left..right {
                let at = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let sides = [edge(b.pos, c.pos, at), edge(c.pos, a.pos, at), edge(a.pos, b.pos, at)];
                let inside = |side: f32, from: Pos2, to: Pos2| {
                    side > 0.0 || (side == 0.0 && owns_samples_on_it(from, to))
                };
                if !(inside(sides[0], b.pos, c.pos)
                    && inside(sides[1], c.pos, a.pos)
                    && inside(sides[2], a.pos, b.pos))
                {
                    continue;
                }
                let weights = sides.map(|side| side / area);
                let blend = |values: [f32; 3]| {
                    values[0] * weights[0] + values[1] * weights[1] + values[2] * weights[2]
                };
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
                    pixel[index] =
                        (source[index] + f32::from(pixel[index]) * keep).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Which side of the line from `a` to `b` a point is on. It's measured from the same end whichever way
/// round the edge is given, so two triangles sharing an edge get exactly opposite values, rounding
/// included, and no pixel falls between them or belongs to both.
fn edge(a: Pos2, b: Pos2, point: Pos2) -> f32 {
    let side = |a: Pos2, b: Pos2| (b.x - a.x) * (point.y - a.y) - (b.y - a.y) * (point.x - a.x);
    if (a.x, a.y) <= (b.x, b.y) { side(a, b) } else { -side(b, a) }
}

/// Whether a triangle walking an edge this way takes the pixels exactly on it, which the triangle walking
/// it the other way then doesn't: the top-left rule GPUs fill by, so a translucent shape split into
/// triangles isn't blended twice along its seams.
fn owns_samples_on_it(from: Pos2, to: Pos2) -> bool {
    to.y < from.y || (to.y == from.y && to.x > from.x)
}

#[cfg(test)]
mod tests {
    use egui::epaint::{ClippedShape, Mesh, Vertex};
    use egui::{CentralPanel, Frame, Shape};

    use super::*;

    /// Half-covering white over black, premultiplied: one blend makes 128 and two make 192.
    const HALF: Color32 = Color32::from_rgba_premultiplied(128, 128, 128, 128);

    fn rasterize_mesh(mesh: Mesh, clip: Rect, width: u32, height: u32) -> Capture {
        let output =
            FullOutput { shapes: vec![ClippedShape::new(clip, Shape::mesh(mesh))], ..FullOutput::default() };
        let ctx = Context::default();
        // The tessellator wants a pass's fonts.
        let mut warmed = ctx.run_ui(RawInput::default(), |_| {});
        warmed.textures_delta.clear();
        rasterize(&ctx, &mut Textures::default(), output, width, height)
    }

    /// A mesh of `points`, half-covering, with triangles by index.
    fn half_mesh(points: &[Pos2], indices: &[u32]) -> Mesh {
        let mut mesh = Mesh::default();
        for pos in points {
            mesh.vertices.push(Vertex { pos: *pos, uv: Pos2::ZERO, color: HALF });
        }
        mesh.indices.extend_from_slice(indices);
        mesh
    }

    /// Whether each pixel has been covered once, by the test `inside` says it should be, and otherwise not
    /// at all: no seam of two blends, and no hole.
    #[track_caller]
    fn assert_covered_once(image: &Capture, inside: impl Fn(f32, f32) -> bool) {
        for y in 0..image.height {
            for x in 0..image.width {
                let want = if inside(x as f32 + 0.5, y as f32 + 0.5) { 128 } else { 0 };
                assert_eq!(image.pixel(x, y)[0], want, "pixel {x}, {y}");
            }
        }
    }

    /// A quad split along a diagonal that crosses pixel centres, in either winding and along either
    /// diagonal: each pixel on it is one triangle's.
    #[test]
    fn a_shared_edge_is_covered_once() {
        let corners = [Pos2::new(0.0, 0.0), Pos2::new(8.0, 0.0), Pos2::new(8.0, 8.0), Pos2::new(0.0, 8.0)];
        for indices in [
            [0, 1, 2, 0, 2, 3],
            [2, 1, 0, 3, 2, 0],
            [0, 1, 2, 2, 3, 0],
            [0, 2, 1, 0, 3, 2],
            [1, 2, 3, 1, 3, 0],
            [3, 2, 1, 0, 3, 1],
        ] {
            let image = rasterize_mesh(half_mesh(&corners, &indices), Rect::EVERYTHING, 8, 8);
            assert_covered_once(&image, |_, _| true);
        }
    }

    /// Quads side by side and one over the other share upright and flat edges through pixel centres, and
    /// edges at thirds, where rounding differs by which end is measured from.
    #[test]
    fn tiled_quads_show_no_seams() {
        for (step, tiles) in [(2.5_f32, 4_u32), (1.0 / 3.0, 30), (0.7, 14), (1.5, 6)] {
            let mut points = Vec::new();
            for y in 0..=tiles {
                for x in 0..=tiles {
                    points.push(Pos2::new(x as f32 * step, y as f32 * step));
                }
            }
            let at = |x: u32, y: u32| y * (tiles + 1) + x;
            let mut indices = Vec::new();
            for y in 0..tiles {
                for x in 0..tiles {
                    let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
                    indices.extend_from_slice(&[a, b, c, a, c, d]);
                }
            }
            let side = tiles as f32 * step;
            let image = rasterize_mesh(half_mesh(&points, &indices), Rect::EVERYTHING, 12, 12);
            assert_covered_once(&image, |x, y| x < side && y < side);
        }
    }

    /// Ends that round, on a line through pixels' centres: how far a centre is from the line then depends
    /// on which end it's measured from, unless it's always the same one. A seam or a hole would show here.
    #[test]
    fn rounding_leaves_no_seam_along_a_shared_edge() {
        let vertex = |pos| Vertex { pos, uv: Pos2::ZERO, color: HALF };
        for before in 1..=60 {
            for after in 1..=60 {
                let (before, after) = (before as f32 * 0.00737, after as f32 * 0.00913);
                let a = vertex(Pos2::new(1.5 - 3.0 * before, 1.5 - before));
                let b = vertex(Pos2::new(7.5 + 3.0 * after, 3.5 + after));
                let (above, below) = (vertex(Pos2::new(11.0, -3.0)), vertex(Pos2::new(-2.0, 9.0)));
                let (width, height) = (12, 6);
                let pixels = vec![[0, 0, 0, 255]; (width * height) as usize];
                let mut canvas = Canvas { width, height, pixels };
                let textures = Textures::default();
                let texture = TextureId::Managed(0);
                canvas.triangle(&a, &above, &b, Rect::EVERYTHING, texture, &textures);
                canvas.triangle(&b, &below, &a, Rect::EVERYTHING, texture, &textures);
                for (x, y) in [(1, 1), (4, 2), (7, 3)] {
                    let pixel = canvas.pixels[(y * width + x) as usize][0];
                    assert_eq!(pixel, 128, "pixel {x}, {y} with ends {:?} and {:?}", a.pos, b.pos);
                }
            }
        }
    }

    /// Triangles around a point that's exactly a pixel's centre: that pixel is one triangle's too.
    #[test]
    fn a_fan_around_a_pixels_centre_covers_it_once() {
        let centre = Pos2::new(3.5, 3.5);
        let rim = [
            Pos2::new(0.0, 0.0),
            Pos2::new(3.5, 0.0),
            Pos2::new(7.0, 0.0),
            Pos2::new(7.0, 3.5),
            Pos2::new(7.0, 7.0),
            Pos2::new(3.5, 7.0),
            Pos2::new(0.0, 7.0),
            Pos2::new(0.0, 3.5),
        ];
        let mut points = vec![centre];
        points.extend_from_slice(&rim);
        let mut indices = Vec::new();
        for spoke in 0..8 {
            indices.extend_from_slice(&[0, spoke + 1, (spoke + 1) % 8 + 1]);
        }
        let image = rasterize_mesh(half_mesh(&points, &indices), Rect::EVERYTHING, 7, 7);
        assert_covered_once(&image, |_, _| true);
    }

    /// A clip keeps what's inside it, covered once, and colours still come out of a triangle's own blend.
    #[test]
    fn a_clipped_quad_is_covered_once_inside_its_clip() {
        let corners = [Pos2::new(0.0, 0.0), Pos2::new(8.0, 0.0), Pos2::new(8.0, 8.0), Pos2::new(0.0, 8.0)];
        let clip = Rect::from_min_max(Pos2::new(2.0, 1.0), Pos2::new(6.0, 7.0));
        let image = rasterize_mesh(half_mesh(&corners, &[0, 1, 2, 0, 2, 3]), clip, 8, 8);
        assert_covered_once(&image, |x, y| (2.0..6.0).contains(&x) && (1.0..7.0).contains(&y));

        // Opaque red over half-covered white: an edge's owner is no business of the colour.
        let mut red = half_mesh(&corners, &[0, 1, 2, 0, 2, 3]);
        for vertex in &mut red.vertices {
            vertex.color = Color32::from_rgb(200, 10, 20);
        }
        let image = rasterize_mesh(red, Rect::EVERYTHING, 8, 8);
        for (x, y) in [(2, 2), (2, 1), (1, 2), (7, 7)] {
            assert_eq!(image.pixel(x, y), [200, 10, 20, 255], "pixel {x}, {y}");
        }
    }

    /// A filled panel comes out its colour, and text marks the pixels it covers, so a capture says something.
    #[test]
    fn a_frame_rasterizes_with_its_fill_and_its_text() {
        let ctx = Context::default();
        let mut textures = Textures::default();
        let run = |ctx: &Context, textures: &mut Textures, time| {
            draw(ctx, textures, raw_input(200, 60, time), 200, 60, |ui| {
                CentralPanel::no_frame().frame(Frame::NONE.fill(Color32::from_rgb(10, 20, 30))).show(
                    ui,
                    |ui| {
                        ui.label("Sapphire");
                    },
                );
            })
        };
        // egui's first pass measures; the second draws what it measured.
        run(&ctx, &mut textures, 0.0);
        let capture = run(&ctx, &mut textures, 0.1);
        assert_eq!(capture.pixel(199, 59), [10, 20, 30, 255]);
        let marked = capture.pixels.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 100).count();
        assert!(marked > 20, "{marked} pixels of text");
    }
}
