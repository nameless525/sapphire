//! A cut-in's panel (M5 L10): an inset picture cut along two straight edges by the interface. Inside the edges
//! the frame shows the inset's picture exactly as an uncut quad of it would; outside them the game's frame,
//! byte for byte, as if no cut-in played; along them a one-pixel antialiased seam, and a border where one is
//! asked for. Two shapes are read back, a slanted band and a full-frame panel opening from its centre line, and
//! the band partway through sliding in from the left.

use glam::Vec2;
use sapphire_render::{EffectCoverage, InsetCutIn, InsetView, SpeedLines, UiDrawList, UiQuad};

use super::*;

const SIZE: (u32, u32) = (96, 64);
const STAND: u64 = 1;
const WORLD: u64 = 2;
const INSET: u16 = u16::MAX;

/// Pixels this close to an edge or a cut are the antialiased seam, which neither side claims.
const SEAM: f32 = 1.5;

/// The stand's feet: far below the world, where the game's camera never looks.
fn stand() -> Vec3 {
    Vec3::new(0.0, -2000.0, 0.0)
}

/// A renderer with a sphere in the world at the origin and another on a cut-in's stand far below it.
struct Studio {
    frames: Frames,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    camera: CameraData,
    stand_camera: CameraData,
    mesh: MeshHandle,
    first: bool,
}

impl Studio {
    fn new(gpu: &GpuContext) -> Self {
        let mut frames = Frames::new(gpu, FORMAT, Msaa::default()).expect("renderer");
        let cooked = cooked_with(&colored([0.9, 0.2, 0.1]), &swizzles());
        let red = Material::read(&cooked).expect("valid");
        let material = frames.renderer.upload_material(red, |_| None).expect("upload");
        let mesh = upload_with(&mut frames.renderer, material);
        let texture = target(gpu, FORMAT, SIZE);
        let view = texture.create_view(&Default::default());
        let stand_camera = CameraData::look_at(
            stand() + Vec3::new(0.4, 0.2, 2.6),
            stand(),
            Vec3::Y,
            34f32.to_radians(),
            SIZE.0 as f32 / SIZE.1 as f32,
            0.05,
        );
        let camera = framing_camera(38.0, SIZE, 0.5);
        Self { frames, texture, view, camera, stand_camera, mesh, first: true }
    }

    /// One frame's pixels: the game's view, with the cut-in's inset and `ui` when there are any.
    fn frame(&mut self, gpu: &GpuContext, ui: Option<&UiDrawList>) -> Vec<[u8; 4]> {
        let frames = &mut self.frames;
        let packet = frames.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = frames.sun;
        packet.sky = frames.sky;
        packet.ambient = frames.ambient;
        packet.post = frames.post;
        if std::mem::take(&mut self.first) {
            packet.instances.changed.push(instance(WORLD, self.mesh, Mat4::IDENTITY));
            packet.instances.changed.push(instance(STAND, self.mesh, Mat4::from_translation(stand())));
        }
        if let Some(ui) = ui {
            let lines = SpeedLines {
                opacity: 0.6,
                center: Vec2::new(0.55, 0.45),
                count: 64,
                width: 0.35,
                inner_radius: 0.3,
                spread: 0.3,
                color: Vec3::new(1.0, 0.95, 0.85),
                seed: 7,
                coverage: EffectCoverage::WORLD,
            };
            let cutin =
                InsetCutIn { backdrop_far: Vec3::new(0.4, 0.1, 0.02), toward: Vec2::new(0.8, 0.2), lines };
            let inset = InsetView {
                backdrop: Vec3::new(0.02, 0.02, 0.1),
                cutin: Some(cutin),
                ..InsetView::new(INSET, self.stand_camera, SIZE, frames.ambient)
            };
            packet.push_inset(inset, [InstanceId(STAND)]);
            packet.ui.clone_from(ui);
        }
        frames.renderer.prepare(gpu, frames.packets.current());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &self.view);
        frames.renderer.record(&mut graph, target).expect("a view and a target");
        frames.graph.execute(gpu, graph).expect("a valid graph");
        read_back(gpu, &self.texture)
    }
}

fn swizzles() -> [Swizzle; AnimeCharacterMaterial::TEXTURE_SLOTS.len()] {
    [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()]
}

/// A straight edge in the frame's points (here its pixels): a point on it and its unit normal into the panel.
#[derive(Clone, Copy)]
struct Edge {
    point: [f32; 2],
    normal: [f32; 2],
}

impl Edge {
    /// Through `(x, y)`, rising `degrees` to the right, the panel below it (`below`) or above it.
    fn through(x: f32, y: f32, degrees: f32, below: bool) -> Self {
        let rise = degrees.to_radians().tan();
        let length = (1.0 + rise * rise).sqrt();
        let sign = if below { 1.0 } else { -1.0 };
        Self { point: [x, y], normal: [sign * rise / length, sign / length] }
    }

    fn moved(self, dx: f32) -> Self {
        Self { point: [self.point[0] + dx, self.point[1]], ..self }
    }

    fn distance(&self, [x, y]: [f32; 2]) -> f32 {
        self.normal[0] * (x - self.point[0]) + self.normal[1] * (y - self.point[1])
    }
}

/// A panel as the interface draws one: a picture quad over `rect` (`x0, y0, x1, y1`), slid `slide` widths along
/// the frame with its picture, cut along `edges` when asked.
#[derive(Clone, Copy)]
struct Panel {
    rect: [f32; 4],
    edges: [Edge; 2],
    slide: f32,
}

impl Panel {
    fn quad(&self, cut: bool, border: f32) -> UiQuad {
        let [x0, y0, x1, y1] = self.rect;
        let (width, height) = (x1 - x0, y1 - y0);
        let mut quad = UiQuad::picture([x0, y0], [width, 0.0], [0.0, height], 255);
        if self.slide != 0.0 {
            quad.uv = [-self.slide, 0.0, 1.0 - self.slide, 1.0];
            quad.clip = [self.slide.max(0.0), (1.0 + self.slide).min(1.0)];
        }
        if cut {
            let edge = |edge: Edge| {
                let edge = edge.moved(self.slide * width);
                let [nx, ny] = edge.normal;
                [nx * width, ny * height, nx * (x0 - edge.point[0]) + ny * (y0 - edge.point[1])]
            };
            quad.edges = [edge(self.edges[0]), edge(self.edges[1])];
            quad.outline = border;
            quad.outline_color = [0, 255, 0, 255];
        }
        quad
    }

    fn list(&self, cut: bool, border: f32) -> UiDrawList {
        let mut list = UiDrawList { viewport: [SIZE.0 as f32, SIZE.1 as f32], ..UiDrawList::default() };
        list.push_inset_quad(INSET, self.quad(cut, border));
        list
    }

    /// Where the pixel centred at `at` lies: inside the cut panel, outside it, or on its seam.
    fn side(&self, at: [f32; 2]) -> Option<bool> {
        let [x0, y0, x1, y1] = self.rect;
        let width = x1 - x0;
        let (from, to) = (x0 + self.slide.max(0.0) * width, x0 + (1.0 + self.slide).min(1.0) * width);
        let mut least = f32::INFINITY;
        for edge in self.edges {
            least = least.min(edge.moved(self.slide * width).distance(at));
        }
        // How far inside the quad's own sides and cuts, which its triangles and clip draw.
        let within = (at[0] - from).min(to - at[0]).min(at[1] - y0).min(y1 - at[1]);
        let inside = least.min(within);
        if inside > SEAM {
            Some(true)
        } else if inside < -SEAM {
            Some(false)
        } else {
            None
        }
    }
}

/// The frame with the cut panel matches the uncut picture inside and the game's frame outside, every byte; and
/// enough of each is there to mean something.
fn check(studio: &mut Studio, gpu: &GpuContext, game: &[[u8; 4]], panel: Panel, name: &str) {
    let cut = studio.frame(gpu, Some(&panel.list(true, 0.0)));
    let picture = studio.frame(gpu, Some(&panel.list(false, 0.0)));
    let (mut inside, mut outside, mut wrong) = (0, 0, Vec::new());
    let mut differs = 0;
    for (index, ((cut, picture), game)) in cut.iter().zip(&picture).zip(game).enumerate() {
        let (x, y) = (index % SIZE.0 as usize, index / SIZE.0 as usize);
        let at = [x as f32 + 0.5, y as f32 + 0.5];
        match panel.side(at) {
            Some(true) => {
                inside += 1;
                differs += usize::from(picture != game);
                if cut != picture {
                    wrong.push(format!("({x}, {y}) inside: {cut:?}, the picture's {picture:?}"));
                }
            }
            Some(false) => {
                outside += 1;
                if cut != game {
                    wrong.push(format!("({x}, {y}) outside: {cut:?}, the game's {game:?}"));
                }
            }
            None => {}
        }
    }
    assert!(
        wrong.is_empty(),
        "{name}: {} pixels wrong, first {:#?}",
        wrong.len(),
        &wrong[..wrong.len().min(8)]
    );
    assert!(inside > 400 && outside > 400, "{name}: {inside} inside, {outside} outside");
    assert!(
        differs * 2 > inside,
        "{name}: the picture is the game's frame at {} of {inside}",
        inside - differs
    );
}

#[test]
#[ignore = "needs a GPU"]
fn a_cut_panel_shows_the_inset_inside_and_the_game_outside() {
    let gpu = gpu();
    let mut studio = Studio::new(&gpu);
    let game = studio.frame(&gpu, None);
    let (width, height) = (SIZE.0 as f32, SIZE.1 as f32);

    // A band slanting up across the frame, its sides turned unequally.
    let top = Edge::through(width * 0.5, 22.0, 8.0, true);
    let bottom = Edge::through(width * 0.5, 44.0, 4.0, false);
    let rect = [
        0.0,
        top.point[1] - 8f32.to_radians().tan() * width * 0.5,
        width,
        44.0 + 4f32.to_radians().tan() * width * 0.5,
    ];
    let band = Panel { rect, edges: [top, bottom], slide: 0.0 };
    check(&mut studio, &gpu, &game, band, "band");

    // A full-frame panel halfway open from its centre line.
    let opening = Panel {
        rect: [0.0, 0.0, width, height],
        edges: [
            Edge::through(width * 0.5, height * 0.25, 0.0, true),
            Edge::through(width * 0.5, height * 0.75, 0.0, false),
        ],
        slide: 0.0,
    };
    check(&mut studio, &gpu, &game, opening, "opening");

    // The band 60% of the way in from the left: its picture and sides slide with it, cut where the frame's
    // left edge would be.
    check(&mut studio, &gpu, &game, Panel { slide: -0.4, ..band }, "entering");

    // A border three pixels wide along the band's sides, in its color, inside the seam.
    let bordered = studio.frame(&gpu, Some(&band.list(true, 3.0)));
    let (mut lined, mut along) = (0, 0);
    for (index, pixel) in bordered.iter().enumerate() {
        let at = [(index % SIZE.0 as usize) as f32 + 0.5, (index / SIZE.0 as usize) as f32 + 0.5];
        let near = band.edges.iter().map(|edge| edge.distance(at)).fold(f32::INFINITY, f32::min);
        if (1.2..=2.2).contains(&near) && at[0] > 2.0 && at[0] < width - 2.0 {
            along += 1;
            lined += usize::from(pixel[1] > 235 && pixel[0] < 20 && pixel[2] < 20);
        }
    }
    assert!(along > 60 && lined == along, "{lined} of {along} pixels along the band's sides are its border");
}
