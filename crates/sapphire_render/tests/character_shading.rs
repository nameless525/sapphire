//! Tests of the character shading follow-ups that need a GPU (M1 C8), each against what the
//! feature should draw: two-sided surfaces shade their back faces as the side the camera sees,
//! and the character prepass holds them; the shadow side takes its hue by its share. Run with
//! `cargo test -p sapphire_render --test character_shading -- --ignored`.
//!
//! Scenes have no ambient light, a black sky and no outline unless a test says otherwise.
//! Characters are listed with white key lights; flat materials light to their albedo and shade
//! to a quarter of it.

use glam::{Mat4, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, CharacterRegion, CharacterSurface, LightSplit, Material,
    MaterialDocument, Outline, OutlineCurve, Rim,
};
use sapphire_assets::mesh::{MaterialSlot, Mesh, PositionFormat, Submesh, Vertex, VertexFlags};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, CharacterShade, ColorGrade, DirectionalLight, FramePacketRing, HeadBasis, InstanceId,
    MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, RenderStats, Renderer,
    SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 128;
/// How far a pixel's linear color may be from the prediction: 8-bit output.
const TOLERANCE: f32 = 0.025;

#[expect(dead_code, reason = "the later features' tests bind maps by ID")]
fn id(text: &str) -> AssetId {
    text.parse().expect("a valid asset ID")
}

/// Post-processing that shows linear values below about 0.98 as they are.
fn passthrough() -> PostParams {
    PostParams {
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
    }
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn assert_near(actual: [f32; 3], expected: [f32; 3], what: &str) {
    let close = actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < TOLERANCE);
    assert!(close, "{what}: {actual:?}, expected {expected:?}");
}

fn scale(color: [f32; 3], by: f32) -> [f32; 3] {
    color.map(|c| c * by)
}

/// An outline distance curve that keeps widths at their full size from 10 cm to 60 m.
const FLAT: OutlineCurve = OutlineCurve { near: (0.1, 1.0), mid: 50.0, far: (60.0, 1.0), cutoff: 100.0 };

/// A flat-lit body material in `color`: lit to its albedo, shaded to a quarter of it across a
/// one-pixel split, with no rim or outline.
fn flat_material(color: [f32; 3]) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        surface: CharacterSurface::Body,
        base_color: [color[0], color[1], color[2], 1.0],
        light_split: LightSplit { threshold: 0.0, softness: 0.0 },
        regions: vec![CharacterRegion { shade_color: [0.25; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, curve: FLAT, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// A shape's vertices and triangles.
struct Part {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2]) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv,
        color: [255; 3],
        flags: VertexFlags::with_material_id(MaterialId::new(0).expect("a material ID")),
    }
}

/// A rectangle at depth `z` from `min` to `max` in x and y, its UVs running left to right and
/// top to bottom, facing +Z, or -Z (its normal and winding turned around) when `back`.
fn rect(min: (f32, f32), max: (f32, f32), z: f32, back: bool) -> Part {
    let normal = if back { Vec3::NEG_Z } else { Vec3::Z };
    let vertices = [(min.0, min.1), (max.0, min.1), (max.0, max.1), (min.0, max.1)]
        .into_iter()
        .map(|(x, y)| {
            let uv = [(x - min.0) / (max.0 - min.0), (max.1 - y) / (max.1 - min.1)];
            vertex(Vec3::new(x, y, z), normal, uv)
        })
        .collect();
    let indices = if back { vec![0, 2, 1, 0, 3, 2] } else { vec![0, 1, 2, 0, 2, 3] };
    Part { vertices, indices }
}

/// `parts` as one mesh, each part a submesh drawn with its own material slot, in order.
fn mesh(parts: &[Part]) -> Mesh {
    let (mut vertices, mut indices, mut submeshes) = (Vec::new(), Vec::new(), Vec::new());
    for (slot, part) in parts.iter().enumerate() {
        let base = vertices.len() as u32;
        let first_index = indices.len() as u32;
        vertices.extend_from_slice(&part.vertices);
        indices.extend(part.indices.iter().map(|index| base + index));
        submeshes.push(Submesh {
            first_index,
            index_count: part.indices.len() as u32,
            material: slot as u32,
        });
    }
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let slots = (0..parts.len())
        .map(|slot| MaterialSlot::embedded(format!("part {slot}"), placeholder.clone()))
        .collect();
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32)
}

/// One rendered frame.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
    stats: RenderStats,
}

impl Frame {
    fn linear(&self, x: usize, y: usize) -> [f32; 3] {
        let pixel = self.pixels[y * SIDE as usize + x];
        [0, 1, 2].map(|c| srgb_to_linear(pixel[c]))
    }

    /// The pixel `point` projects into.
    fn pixel_of(&self, point: Vec3) -> (usize, usize) {
        let clip = self.camera.proj * self.camera.view * point.extend(1.0);
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        let side = SIDE as f32;
        (((x + 1.0) * 0.5 * side) as usize, ((1.0 - y) * 0.5 * side) as usize)
    }

    /// The linear color where `point` projects.
    fn at(&self, point: Vec3) -> [f32; 3] {
        let (x, y) = self.pixel_of(point);
        self.linear(x, y)
    }
}

/// A renderer and everything a test frame needs.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    added: Vec<MeshInstance>,
    removed: Vec<InstanceId>,
    characters: Vec<CharacterInstance>,
    next_id: u64,
}

impl Harness {
    fn new() -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            camera: CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.66, 1.0, 0.05),
            added: Vec::new(),
            removed: Vec::new(),
            characters: Vec::new(),
            next_id: 0,
        }
    }

    /// Looks at the origin from `distance` along `direction` (from the origin toward the eye)
    /// through a vertical field of view of `fov` degrees.
    fn view(&mut self, direction: Vec3, distance: f32, fov: f32) {
        let eye = direction.normalize() * distance;
        self.camera = CameraData::look_at(eye, Vec3::ZERO, Vec3::Y, fov.to_radians(), 1.0, 0.05);
    }

    fn cook(document: &AnimeCharacterMaterial) -> Vec<u8> {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook").to_vec()
    }

    /// Uploads `document`, which names no textures.
    fn material(&mut self, document: &AnimeCharacterMaterial) -> MaterialHandle {
        let bytes = Self::cook(document);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
    }

    /// Replaces `handle` with `document`, which names no textures.
    fn update(&mut self, handle: MaterialHandle, document: &AnimeCharacterMaterial) {
        let bytes = Self::cook(document);
        self.renderer
            .update_material(handle, Material::read(&bytes).expect("read"), |_| None)
            .expect("update");
    }

    /// Uploads `parts` as one mesh drawn with `materials`, a material per part.
    fn mesh(&mut self, parts: &[Part], materials: &[MaterialHandle]) -> MeshHandle {
        let bytes = mesh(parts).to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), materials).expect("upload")
    }

    /// Adds an instance of `mesh` at `transform` with the next frame and lists it as a character
    /// lit by a white key light from `toward_key`, its head at rest.
    fn character(&mut self, mesh: MeshHandle, transform: Mat4, toward_key: Vec3) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform });
        self.light(id, CharacterLighting::unstyled(white_key(toward_key)));
        id
    }

    /// Lights character `id` with `lighting` from the next frame on.
    fn light(&mut self, id: InstanceId, lighting: CharacterLighting) {
        self.characters.retain(|character| character.id != id);
        self.characters.push(CharacterInstance {
            id,
            lighting,
            face: CharacterFace { head: HeadBasis::REST, ..CharacterFace::default() },
            body: CharacterBody::default(),
            opacity: 1.0,
        });
    }

    /// Removes instance `id` with the next frame.
    fn remove(&mut self, id: InstanceId) {
        self.characters.retain(|character| character.id != id);
        self.removed.push(id);
    }

    /// Prepares, records and submits a frame and reads the target back. Fails on any
    /// validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        packet.ambient = AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 };
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        packet.instances.removed.append(&mut self.removed);
        packet.characters.extend_from_slice(&self.characters);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        Frame { pixels: self.read_back(), camera: self.camera, stats: self.renderer.stats() }
    }

    fn read_back(&self) -> Vec<[u8; 4]> {
        let padded = (SIDE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * SIDE),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.gpu.device().create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(SIDE),
                },
            },
            self.target.size(),
        );
        self.gpu.queue().submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        self.gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let mapped = buffer.get_mapped_range(..).expect("mapped");
        mapped
            .chunks_exact(padded as usize)
            .flat_map(|row| row[..SIDE as usize * 4].as_chunks::<4>().0.iter().copied())
            .collect()
    }
}

/// A white key light from `toward_key`.
fn white_key(toward_key: Vec3) -> DirectionalLight {
    DirectionalLight { direction: -toward_key.normalize(), color: Vec3::ONE, intensity: 1.0 }
}

/// The two-sided garment's albedo.
const CLOTH: [f32; 3] = [0.8, 0.5, 0.3];

/// Keys around a panel seen from -Z: from the camera's side, from behind, and two between.
const KEYS: [Vec3; 4] = [Vec3::NEG_Z, Vec3::Z, Vec3::new(0.6, 0.3, -0.7), Vec3::new(-0.5, 0.5, 0.7)];

#[test]
#[ignore = "needs a GPU adapter"]
fn two_sided_surfaces_shade_their_back_faces_as_the_side_the_camera_sees() {
    let mut harness = Harness::new();
    // A panel facing +Z, seen from behind (from -Z).
    harness.view(Vec3::NEG_Z, 3.0, 38.0);
    let one_sided = flat_material(CLOTH);
    let two_sided = AnimeCharacterMaterial { double_sided: true, ..one_sided.clone() };
    let material = harness.material(&one_sided);
    let panel = harness.mesh(&[rect((-1.0, -1.0), (1.0, 1.0), 0.0, false)], &[material]);
    let character = harness.character(panel, Mat4::IDENTITY, Vec3::NEG_Z);
    // One-sided, its back is culled: the black sky shows.
    let frame = harness.render();
    assert_near(frame.at(Vec3::ZERO), [0.0; 3], "culled");
    assert_eq!(frame.stats.character_prepass.draws, 1);

    // Two-sided, it shades with its normal turned toward the camera: lit by a key on the
    // camera's side, shaded by one behind it, as the panel's front would be from the front.
    harness.update(material, &two_sided);
    assert_near(harness.render().at(Vec3::ZERO), CLOTH, "lit from the camera's side");
    harness.light(character, CharacterLighting::unstyled(white_key(Vec3::Z)));
    assert_near(harness.render().at(Vec3::ZERO), scale(CLOTH, 0.25), "shaded from behind");

    // Exactly the pixels of a one-sided panel facing the camera (turned around, so its normal
    // and winding face -Z), under every key.
    let backs: Vec<Vec<[u8; 4]>> = KEYS
        .iter()
        .map(|&toward_key| {
            harness.light(character, CharacterLighting::unstyled(white_key(toward_key)));
            harness.render().pixels
        })
        .collect();
    harness.remove(character);
    let front_material = harness.material(&one_sided);
    let front = harness.mesh(&[rect((-1.0, -1.0), (1.0, 1.0), 0.0, true)], &[front_material]);
    let turned = harness.character(front, Mat4::IDENTITY, Vec3::NEG_Z);
    for (toward_key, back) in KEYS.into_iter().zip(backs) {
        harness.light(turned, CharacterLighting::unstyled(white_key(toward_key)));
        assert!(harness.render().pixels == back, "toward {toward_key}");
    }
}

/// A bowl: the half of a sphere of `radius` on -Z, open toward +Z, its normals outward and its
/// triangles counter-clockwise seen from outside.
fn bowl(radius: f32) -> Part {
    let (rings, segments) = (24u32, 64u32);
    let mut vertices = Vec::new();
    for ring in 0..=rings {
        let theta = std::f32::consts::FRAC_PI_2 * ring as f32 / rings as f32;
        for segment in 0..=segments {
            let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), -theta.cos());
            vertices.push(vertex(
                n * radius,
                n,
                [segment as f32 / segments as f32, ring as f32 / rings as f32],
            ));
        }
    }
    let stride = segments + 1;
    let mut indices = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            for [i, j, k] in [[a, a + stride, a + 1], [a + 1, a + stride, a + stride + 1]] {
                let [p, q, r] = [i, j, k].map(|index| Vec3::from(vertices[index as usize].position));
                let outward =
                    Vec3::from(vertices[i as usize].normal) + Vec3::from(vertices[k as usize].normal);
                let facing = (q - p).cross(r - p).dot(outward);
                indices.extend(if facing >= 0.0 { [i, j, k] } else { [i, k, j] });
            }
        }
    }
    Part { vertices, indices }
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_character_prepass_holds_two_sided_back_faces() {
    let mut harness = Harness::new();
    // A black bowl rimmed by the depth-offset rim. The rim lights where the character prepass
    // finds nothing within a meter behind the surface, along its normal across the screen.
    let rimmed = AnimeCharacterMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        rim: Rim {
            width: 20.0,
            strength: 1.0,
            threshold: 1.0,
            fresnel: 1.0,
            light_gate: 0.0,
            back_light: 0.0,
        },
        double_sided: true,
        ..flat_material([0.0; 3])
    };
    let material = harness.material(&rimmed);
    let figure = harness.mesh(&[bowl(0.5)], &[material]);
    harness.character(figure, Mat4::IDENTITY, Vec3::Z);
    let lit = |frame: &Frame| {
        (0..SIDE as usize)
            .flat_map(|y| (0..SIDE as usize).map(move |x| (x, y)))
            .filter(|&(x, y)| frame.linear(x, y)[0] > 0.1)
            .count()
    };
    // From behind, its outside rims along its silhouette.
    harness.view(Vec3::NEG_Z, 3.0, 38.0);
    let outside = lit(&harness.render());
    assert!(outside > 50, "a rim around the outside: {outside} px");
    // Looking in, its inside (back faces) is in the prepass, so it finds the bowl a little
    // farther along everywhere and nothing lights: a prepass without them would find nothing
    // and rim the whole inside.
    harness.view(Vec3::Z, 3.0, 38.0);
    let frame = harness.render();
    assert_near(frame.at(Vec3::new(0.0, 0.0, -0.5)), [0.0; 3], "the bowl's bottom");
    assert_eq!(lit(&frame), 0, "no rim inside the bowl");
}

/// Rec. 709 luminance, as the shader takes it.
fn luminance(color: Vec3) -> f32 {
    color.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// The shadow side's light under a key of `key` radiance with `shade` (the
/// `sapphire_render::CharacterShade` docs).
fn shade_light(key: Vec3, shade: CharacterShade) -> Vec3 {
    let hue = shade.hue / luminance(shade.hue);
    key.lerp(luminance(key) * hue, shade.share.clamp(0.0, 1.0))
}

#[test]
#[ignore = "needs a GPU adapter"]
fn the_shadow_side_takes_its_hue_by_its_share() {
    let mut harness = Harness::new();
    // A panel facing the camera in a material whose shade tone is a light grey, so the shadow
    // side's light shows plainly, with an outline lit as the surface is.
    let albedo = Vec3::new(0.85, 0.8, 0.75);
    let tone = 0.7;
    let document = AnimeCharacterMaterial {
        regions: vec![CharacterRegion { shade_color: [tone; 3], ..CharacterRegion::default() }],
        ..flat_material(albedo.to_array())
    };
    let material = harness.material(&document);
    let panel = harness.mesh(&[rect((-1.0, -1.0), (1.0, 1.0), 0.0, false)], &[material]);
    let character = harness.character(panel, Mat4::IDENTITY, Vec3::Z);
    // A golden-hour key: warm, the lit side kept below where the tone curve bends.
    let warm = DirectionalLight { color: Vec3::new(1.0, 0.62, 0.32), intensity: 1.0, ..white_key(Vec3::Z) };
    let key = warm.color * warm.intensity;
    let lit = |shade: CharacterShade, toward: Vec3| CharacterLighting {
        shade,
        ..CharacterLighting::unstyled(DirectionalLight { direction: -toward, ..warm })
    };
    // Without a share, the shadow side is the key light's, exactly: the same pixels whatever
    // the hue.
    harness.light(character, lit(CharacterShade::default(), Vec3::NEG_Z));
    let plain = harness.render();
    let shaded = scale(albedo.to_array(), tone);
    assert_near(plain.at(Vec3::ZERO), (Vec3::from(shaded) * key).to_array(), "the key's own");
    let blue = Vec3::new(0.35, 0.45, 1.0);
    harness.light(character, lit(CharacterShade { hue: blue, share: 0.0 }, Vec3::NEG_Z));
    assert!(harness.render().pixels == plain.pixels, "no share, no change");
    for (hue, share) in [(blue, 0.5), (blue, 1.0), (Vec3::new(0.9, 0.3, 0.6), 0.75), (blue * 3.0, 0.5)] {
        let shade = CharacterShade { hue, share };
        // The shadow side (the key behind the panel) in the hue by its share, at the key's
        // luminance.
        harness.light(character, lit(shade, Vec3::NEG_Z));
        let expected = Vec3::from(shaded) * shade_light(key, shade);
        assert_near(harness.render().at(Vec3::ZERO), expected.to_array(), &format!("{shade:?} in shadow"));
        // The lit side keeps the key light.
        harness.light(character, lit(shade, Vec3::Z));
        let expected = albedo * key;
        assert_near(harness.render().at(Vec3::ZERO), expected.to_array(), &format!("{shade:?} lit"));
    }
    // A share past 1 counts as 1.
    let over = CharacterShade { hue: blue, share: 3.0 };
    harness.light(character, lit(over, Vec3::NEG_Z));
    let full = Vec3::from(shaded) * shade_light(key, CharacterShade { share: 1.0, ..over });
    assert_near(harness.render().at(Vec3::ZERO), full.to_array(), "held to 1");
}

#[test]
#[ignore = "needs a GPU adapter"]
fn two_sided_pipelines_are_built_only_for_two_sided_materials() {
    let mut harness = Harness::new();
    // Render pipelines the device holds, as wgpu counts them.
    let held = |harness: &Harness| {
        let report = harness.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        report.hub.render_pipelines.num_kept_from_user
    };
    let opaque = flat_material(CLOTH);
    let masked = AnimeCharacterMaterial { alpha: AlphaMode::Mask { cutoff: 0.5 }, ..flat_material(CLOTH) };
    let parts = || [rect((-1.0, -1.0), (0.0, 1.0), 0.0, false), rect((0.0, -1.0), (1.0, 1.0), 0.0, false)];
    let materials = [harness.material(&opaque), harness.material(&masked)];
    let panels = harness.mesh(&parts(), &materials);
    harness.character(panels, Mat4::IDENTITY, Vec3::Z);
    harness.render();
    let one_sided = held(&harness);
    // Compared against the base commit's count for the same scene (M1 C8's report).
    eprintln!("render pipelines without two-sided materials: {one_sided}");
    // A two-sided material's first draw in a format builds its surface's pipeline and the two
    // prepasses' that cull nothing for it (the alpha-tested one's masked), and nothing else: its
    // outline hull and shadow casters are the one-sided ones'. (A material edit would build them
    // for every format drawn so far: the quantized one is always there.)
    let mut built = one_sided;
    for document in [&opaque, &masked] {
        let two_sided = harness.material(&AnimeCharacterMaterial { double_sided: true, ..document.clone() });
        let panels = harness.mesh(&parts(), &[two_sided, two_sided]);
        harness.character(panels, Mat4::from_translation(Vec3::Z), Vec3::Z);
        harness.render();
        built += 3;
        assert_eq!(held(&harness), built, "{:?}", document.alpha);
    }
}
