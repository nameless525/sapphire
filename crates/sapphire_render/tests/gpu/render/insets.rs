//! A held inset picture draws again once what drew it is replaced (M3 Z5c): a shader reload's
//! pipelines, a material or mesh reloaded in place. Before, nothing moved a held picture but a
//! change in the packet, so a showcase kept showing the old shader or content until it turned. And an
//! inset's soft backdrop (M5 L8) blurs only what isn't a character.

use sapphire_assets::material::Material;
use sapphire_assets::texture::Swizzle;
use sapphire_render::{
    CharacterBody, CharacterFace, CharacterInstance, CharacterLighting, DepthOfFieldSettings, InsetView,
    MaterialDrive, ShaderFile, ShaderSources, UiDrawList, UiQuad,
};

use super::*;

const SIZE: (u32, u32) = (64, 64);
const SPHERE: u64 = 1;

/// A renderer showing one inset of a unit sphere across its whole target.
struct Showcase {
    frames: Frames,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    camera: CameraData,
    /// The interface's one quad, showing the inset's picture over the frame.
    ui: UiDrawList,
    material: MaterialHandle,
    mesh: MeshHandle,
    /// The sphere's delta, sent with the first frame only.
    first: bool,
}

impl Showcase {
    fn new(gpu: &GpuContext) -> Self {
        let mut frames = Frames::new(gpu, FORMAT, Msaa::default()).expect("renderer");
        let red = cooked_material([1.0, 0.0, 0.0]);
        let material =
            frames.renderer.upload_material(Material::read(&red).expect("valid"), |_| None).expect("upload");
        let mesh = upload_with(&mut frames.renderer, material);
        let texture = target(gpu, FORMAT, SIZE);
        let view = texture.create_view(&Default::default());
        let mut ui = UiDrawList { viewport: [SIZE.0 as f32, SIZE.1 as f32], ..UiDrawList::default() };
        ui.push_inset_quad(1, UiQuad::picture([0.0, 0.0], [SIZE.0 as f32, 0.0], [0.0, SIZE.1 as f32], 255));
        let camera = framing_camera(38.0, SIZE, 0.6);
        Self { frames, texture, view, camera, ui, material, mesh, first: true }
    }

    /// Draws a frame, returning how many insets it drew and the linear color at the sphere's center.
    fn frame(&mut self, gpu: &GpuContext) -> (u32, [f32; 3]) {
        let frames = &mut self.frames;
        let packet = frames.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = frames.sun;
        packet.sky = frames.sky;
        packet.ambient = frames.ambient;
        packet.post = frames.post;
        if std::mem::take(&mut self.first) {
            packet.instances.changed.push(instance(SPHERE, self.mesh, Mat4::IDENTITY));
        }
        let inset = InsetView::new(1, self.camera, SIZE, frames.ambient);
        packet.push_inset(inset, [InstanceId(SPHERE)]);
        packet.ui.clone_from(&self.ui);
        frames.renderer.prepare(gpu, frames.packets.current());
        let drawn = frames.renderer.stats().insets.drawn;
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &self.view);
        frames.renderer.record(&mut graph, target).expect("a view and a target");
        frames.graph.execute(gpu, graph).expect("a valid graph");
        let pixels = read_back(gpu, &self.texture);
        (drawn, linear_at(&pixels, SIZE, SIZE.0 as usize / 2))
    }
}

fn cooked_material(rgb: [f32; 3]) -> sapphire_assets::material::AlignedVec {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    cooked_with(&colored(rgb), &swizzles)
}

/// Mostly red, mostly blue: the sphere's center, lit by the sun along the view.
fn reddest(rgb: [f32; 3]) -> bool {
    rgb[0] > 0.5 && rgb[1] < 0.1 && rgb[2] < 0.1
}

fn bluest(rgb: [f32; 3]) -> bool {
    rgb[2] > 0.5 && rgb[0] < 0.1 && rgb[1] < 0.1
}

/// Each replacement draws the held picture again on the next frame, showing the new result, and the
/// frame after it holds again. Without the redraw, each `drawn` after a replacement reads 0 and the
/// picture keeps the old color.
#[test]
#[ignore = "needs a GPU"]
fn a_held_picture_draws_again_after_a_reload() {
    let gpu = gpu();
    let mut showcase = Showcase::new(&gpu);
    let (drawn, center) = showcase.frame(&gpu);
    assert!(drawn == 1 && reddest(center), "the first frame draws the red sphere: {drawn}, {center:?}");
    assert_eq!(showcase.frame(&gpu).0, 0, "an unchanged inset holds its picture");

    // A material reloaded in place, as hot reload applies one.
    let blue = cooked_material([0.0, 0.0, 1.0]);
    let renderer = &mut showcase.frames.renderer;
    renderer.update_material(showcase.material, Material::read(&blue).expect("valid"), |_| None).unwrap();
    let (drawn, center) = showcase.frame(&gpu);
    assert!(drawn == 1 && bluest(center), "a reloaded material draws: {drawn}, {center:?}");
    assert_eq!(showcase.frame(&gpu).0, 0, "and then holds");

    // A shader reload: the inset's resolve swaps red and blue.
    let sources = ShaderSources::compiled_in();
    let line = "let color = textureSampleLevel(texture0, linear_clamp, in.uv, 0.0).rgb * post.composite.x;";
    let text = sources.get(ShaderFile::Post);
    assert_eq!(text.matches(line).count(), 1, "fs_inset's first line");
    let mut swapped = sources.clone();
    swapped.set(ShaderFile::Post, text.replace(line, &line.replace(".rgb", ".bgr")));
    showcase.frames.renderer.set_shader_sources(&swapped).expect("a valid edit");
    let (drawn, center) = showcase.frame(&gpu);
    assert!(drawn == 1 && reddest(center), "the new resolve draws: {drawn}, {center:?}");
    assert_eq!(showcase.frame(&gpu).0, 0, "and then holds");

    // A mesh reloaded in place: the sphere moved off the picture's center, so the backdrop shows.
    let away = sphere_at(Vec3::new(4.0, 0.0, 0.0), 1.0, colored([0.0, 0.0, 1.0]), PositionFormat::Unorm16);
    let bytes = away.to_cooked().expect("cook");
    let renderer = &mut showcase.frames.renderer;
    renderer.update_mesh(showcase.mesh, Mesh::read(&bytes).expect("a valid mesh"), |_| None).unwrap();
    let (drawn, center) = showcase.frame(&gpu);
    assert!(drawn == 1 && center.iter().all(|&c| c < 0.02), "a reloaded mesh draws: {drawn}, {center:?}");
    assert_eq!(showcase.frame(&gpu).0, 0, "and then holds");
}

/// One inset picture of a red character sphere before a blue sphere that isn't a character, on a black
/// backdrop, from a fresh renderer with `settings`, its backdrop as soft as `soft` says (M5 L8).
fn soft_backdrop_picture(gpu: &GpuContext, settings: RenderSettings, soft: f32) -> Vec<[u8; 4]> {
    let mut frames = Frames::with_settings(gpu, FORMAT, settings).expect("renderer");
    let red = sphere_at(Vec3::ZERO, 0.6, colored([1.0, 0.0, 0.0]), PositionFormat::Float32);
    let character = upload(&mut frames.renderer, &red);
    let blue = sphere_at(Vec3::new(0.7, 0.5, -2.0), 0.8, colored([0.0, 0.0, 1.0]), PositionFormat::Float32);
    let world = upload(&mut frames.renderer, &blue);
    let texture = target(gpu, FORMAT, SIZE);
    let view = texture.create_view(&Default::default());
    let mut ui = UiDrawList { viewport: [SIZE.0 as f32, SIZE.1 as f32], ..UiDrawList::default() };
    ui.push_inset_quad(1, UiQuad::picture([0.0, 0.0], [SIZE.0 as f32, 0.0], [0.0, SIZE.1 as f32], 255));
    let camera = framing_camera(38.0, SIZE, 0.4);
    let packet = frames.packets.begin();
    packet.views.push(ViewData::new(camera));
    packet.sun = frames.sun;
    packet.sky = frames.sky;
    packet.ambient = frames.ambient;
    packet.post = frames.post;
    packet.instances.changed.push(instance(1, character, Mat4::IDENTITY));
    packet.instances.changed.push(instance(2, world, Mat4::IDENTITY));
    packet.characters.push(CharacterInstance {
        id: InstanceId(1),
        lighting: CharacterLighting::unstyled(frames.sun),
        face: CharacterFace::default(),
        body: CharacterBody::default(),
        opacity: 1.0,
        drive: MaterialDrive::NONE,
    });
    let inset = InsetView { soft_backdrop: soft, ..InsetView::new(1, camera, SIZE, frames.ambient) };
    packet.push_inset(inset, [InstanceId(1), InstanceId(2)]);
    packet.ui.clone_from(&ui);
    frames.renderer.prepare(gpu, frames.packets.current());
    assert_eq!(frames.renderer.stats().insets.drawn, 1);
    let mut graph = FrameGraph::new();
    let target = graph.import_texture("target", &view);
    frames.renderer.record(&mut graph, target).expect("a view and a target");
    frames.graph.execute(gpu, graph).expect("a valid graph");
    read_back(gpu, &texture)
}

/// A soft backdrop (M5 L8) blurs what isn't a character behind an inset's characters, so the blue sphere's
/// edge spreads into the black, and leaves every pixel of the character as it was, with none of its red
/// spread into the backdrop. A tier without soft backdrops ignores it.
#[test]
#[ignore = "needs a GPU"]
fn a_soft_backdrop_blurs_what_isnt_a_character_and_leaves_the_characters_exact() {
    let gpu = gpu();
    let soft_tier = RenderSettings {
        depth_of_field: DepthOfFieldSettings { view: false, insets: true },
        ..RenderSettings::default()
    };
    let plain = soft_backdrop_picture(&gpu, soft_tier, 0.0);
    let soft = soft_backdrop_picture(&gpu, soft_tier, 0.02);
    assert_eq!(
        soft_backdrop_picture(&gpu, RenderSettings::default(), 0.02),
        plain,
        "a tier without soft backdrops ignores one"
    );
    let (width, height) = (SIZE.0 as i32, SIZE.1 as i32);
    let at = |x: i32, y: i32| (y.clamp(0, height - 1) * width + x.clamp(0, width - 1)) as usize;
    let linear = |texel: [u8; 4]| [0, 1, 2].map(|c| srgb_to_linear(texel[c]));
    let red = |x: i32, y: i32| reddest(linear(plain[at(x, y)]));
    let (mut exact, mut spread, mut leak) = (0, 0, 0.0f32);
    for y in 0..height {
        for x in 0..width {
            let around = |reach: i32| {
                (-reach..=reach).flat_map(move |dy| (-reach..=reach).map(move |dx| (x + dx, y + dy)))
            };
            let (before, after) = (linear(plain[at(x, y)]), linear(soft[at(x, y)]));
            if around(1).all(|(x, y)| red(x, y)) {
                assert_eq!(soft[at(x, y)], plain[at(x, y)], "the character's pixel ({x}, {y}) is exact");
                exact += 1;
            } else if !around(2).any(|(x, y)| red(x, y)) {
                leak = leak.max(after[0] - before[0]);
                // The blue sphere's light spread into what was black around it.
                spread += usize::from(before[2] < 0.01 && after[2] > 0.02);
            }
        }
    }
    assert!(exact > 100, "{exact} character pixels");
    assert!(spread > 20, "the blue sphere's edge blurs into the backdrop: {spread} pixels");
    assert!(leak < 0.01, "none of the character's red reaches the backdrop: {leak}");
    eprintln!(
        "{exact} character pixels exact; blue spread over {spread} black pixels; red added at most {leak:.4}"
    );
}
