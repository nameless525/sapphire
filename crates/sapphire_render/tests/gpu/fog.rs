//! Fog tests that need a GPU (M1 W3): the world and the sky against the CPU model (`Fog::ray`),
//! frames without density drawing as without fog, characters' share, and fog cards' fades,
//! facings and order. Run with `cargo test -p sapphire_render --test gpu fog:: -- --ignored`.
//!
//! Scenes have no MSAA (temporal AA where a test is about it) and post that shows linear values
//! as they are, so each pixel can be predicted: surfaces are emissive or black, and each pixel's
//! expected color comes from the model along the exact ray it shades.

use glam::{Mat4, Quat, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CardFacing, CharacterRegion, DistanceStylization, EnvironmentMaterial,
    FogCardMaterial, LightSplit, Material, MaterialDocument, Outline, Rim, Terminator,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_render::fog::SKY_DISTANCE;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, Fog, FogLayer, FramePacketRing, HeightFog, InstanceId,
    MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, Renderer, SkyGradient,
    ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 128;
/// How far a pixel's linear color may be from the model: 8-bit output.
const TOLERANCE: f32 = 0.02;

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

/// `card_fade` in `fog_card.wgsl`: 0 at `start`, 1 from `end` on, eased.
fn smoothstep(start: f32, end: f32, x: f32) -> f32 {
    let t = ((x - start) / (end - start).max(1e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The frame's fog: every layer, the sun's inscatter, and characters taking a quarter of it.
fn thick_fog() -> Fog {
    Fog {
        near: FogLayer { color: Vec3::new(0.6, 0.5, 0.4), density: 0.01, start: 3.0 },
        far: FogLayer { color: Vec3::new(0.9, 0.95, 1.0), density: 0.005, start: 20.0 },
        height: HeightFog { color: Vec3::new(0.4, 0.5, 0.6), density: 0.04, falloff: 0.25, base: 0.0 },
        sun_inscatter: Vec3::new(0.15, 0.1, 0.05),
        characters: 0.25,
    }
}

/// A sky the fog's horizon colors, distinct at the zenith and below the horizon.
const SKY: SkyGradient = SkyGradient {
    zenith: Vec3::new(0.2, 0.3, 0.6),
    horizon: Vec3::new(0.5, 0.55, 0.6),
    ground: Vec3::new(0.3, 0.3, 0.3),
};

/// `sky_color` in `frame.wgsl`.
fn sky_color(sky: &SkyGradient, dir: Vec3) -> Vec3 {
    if dir.y >= 0.0 {
        sky.horizon.lerp(sky.zenith, dir.y.clamp(0.0, 1.0).sqrt())
    } else {
        sky.horizon.lerp(sky.ground, (-dir.y * 5.0).clamp(0.0, 1.0))
    }
}

/// A black environment material, glowing with `emission`: its color is the emission alone.
fn glowing(emission: Vec3) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        roughness: 0.5,
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        emission: emission.to_array(),
        ..EnvironmentMaterial::default()
    }
}

/// A flat anime character material, without rim or outline.
fn flat_character(rgb: Vec3) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: rgb.extend(1.0).to_array(),
        light_split: LightSplit { threshold: 0.0, softness: 0.02 },
        regions: vec![CharacterRegion { shade_color: [0.4; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

/// A fog card without a map: evenly dense, `opacity` of it, tinted `tint`.
fn card(tint: Vec3, opacity: f32, facing: CardFacing) -> FogCardMaterial {
    FogCardMaterial {
        color: tint.extend(opacity).to_array(),
        map: None,
        scroll: [0.0; 2],
        depth_fade: 0.0,
        camera_fade: [0.0, 0.0],
        sun_scatter: 0.0,
        facing,
    }
}

/// One rendered frame.
struct Frame {
    pixels: Vec<[u8; 4]>,
    camera: CameraData,
}

impl Frame {
    fn linear(&self, x: u32, y: u32) -> Vec3 {
        let pixel = self.pixels[(y * SIDE + x) as usize];
        Vec3::new(srgb_to_linear(pixel[0]), srgb_to_linear(pixel[1]), srgb_to_linear(pixel[2]))
    }

    /// The world-space ray through pixel `(x, y)`'s center: the eye and a unit direction.
    fn ray(&self, x: u32, y: u32) -> (Vec3, Vec3) {
        let inverse = (self.camera.proj * self.camera.view).inverse();
        let ndc_x = (x as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0;
        let ndc_y = 1.0 - (y as f32 + 0.5) / SIDE as f32 * 2.0;
        let far = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 0.5));
        (self.camera.position, (far - self.camera.position).normalize())
    }

    /// The pixel `point` projects into.
    fn pixel_of(&self, point: Vec3) -> (u32, u32) {
        let ndc = (self.camera.proj * self.camera.view).project_point3(point);
        (((ndc.x + 1.0) * 0.5 * SIDE as f32) as u32, ((1.0 - ndc.y) * 0.5 * SIDE as f32) as u32)
    }
}

/// A rectangle facing `normal`: `width` along `across`, `height` along `normal × across`.
#[derive(Clone, Copy)]
struct Rect {
    center: Vec3,
    normal: Vec3,
    across: Vec3,
    width: f32,
    height: f32,
}

impl Rect {
    /// The distance along the ray from `eye` along unit `direction` to where it crosses the
    /// rectangle, if it does, from its front, past `margin` from its edges (negative: beyond).
    fn hit(&self, eye: Vec3, direction: Vec3, margin: f32) -> Option<f32> {
        let denominator = direction.dot(self.normal);
        if denominator > -1e-4 {
            return None;
        }
        let distance = (self.center - eye).dot(self.normal) / denominator;
        let offset = eye + direction * distance - self.center;
        let up = self.normal.cross(self.across);
        let inside = offset.dot(self.across).abs() < self.width * 0.5 - margin
            && offset.dot(up).abs() < self.height * 0.5 - margin;
        (distance > 0.0 && inside).then_some(distance)
    }

    /// Its two triangles, counter-clockwise from the front, UVs across it with V down.
    fn part(&self) -> (Vec<Vertex>, Vec<u32>) {
        let up = self.normal.cross(self.across);
        let corner = |s: f32, t: f32| Vertex {
            position: (self.center + self.across * (s * self.width * 0.5) + up * (t * self.height * 0.5))
                .to_array(),
            normal: self.normal.to_array(),
            outline_normal: self.normal.to_array(),
            uv: [(s + 1.0) * 0.5, (1.0 - t) * 0.5],
            color: VERTEX_COLOR_NEUTRAL,
            flags: VertexFlags::NONE,
        };
        (
            vec![corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)],
            vec![0, 1, 2, 0, 2, 3],
        )
    }
}

/// A UV sphere of `radius` around the origin.
fn sphere(radius: f32) -> (Vec<Vertex>, Vec<u32>) {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = std::f32::consts::PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            vertices.push(Vertex {
                position: (n * radius).to_array(),
                normal: n.to_array(),
                outline_normal: n.to_array(),
                uv: [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
        }
    }
    let stride = SEGMENTS + 1;
    let mut indices = Vec::new();
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let (b, c, d) = (a + stride, a + 1, a + stride + 1);
            indices.extend([a, c, b, c, d, b]);
        }
    }
    (vertices, indices)
}

/// A mesh of `parts`, each its own submesh drawing its own material slot, with f32 positions.
fn mesh(parts: Vec<(Vec<Vertex>, Vec<u32>)>) -> Mesh {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    let (mut vertices, mut indices, mut submeshes, mut slots) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (part, (part_vertices, part_indices)) in parts.into_iter().enumerate() {
        let first = vertices.len() as u32;
        submeshes.push(Submesh {
            first_index: indices.len() as u32,
            index_count: part_indices.len() as u32,
            material: part as u32,
        });
        indices.extend(part_indices.into_iter().map(|index| first + index));
        vertices.extend(part_vertices);
        slots.push(MaterialSlot::embedded(format!("part {part}"), placeholder.clone()));
    }
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Float32)
}

/// A renderer and everything a test frame needs.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    camera: CameraData,
    sun: DirectionalLight,
    sky: SkyGradient,
    ambient: AmbientLight,
    fog: Fog,
    characters: Vec<CharacterInstance>,
    added: Vec<MeshInstance>,
    next_id: u64,
}

impl Harness {
    fn new(settings: RenderSettings) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
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
            camera: CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 1.0, 1.0, 0.05),
            // Toward the camera's back: no inscatter lobe unless a test turns it.
            sun: DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 0.0 },
            sky: SKY,
            // None: black surfaces stay black, their ambient reflection too.
            ambient: AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 },
            fog: Fog::default(),
            characters: Vec::new(),
            added: Vec::new(),
            next_id: 0,
        }
    }

    /// Without MSAA, so every interior pixel is shaded once at its center, as the model is.
    fn exact() -> Self {
        Self::new(RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() })
    }

    /// Cooks `document` (which names no textures) and uploads it.
    fn material<D: MaterialDocument>(&mut self, document: &D) -> MaterialHandle {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        self.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
    }

    /// Uploads `mesh`, its slots drawn with `materials`.
    fn mesh(&mut self, mesh: &Mesh, materials: &[MaterialHandle]) -> MeshHandle {
        let bytes = mesh.to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), materials).expect("upload")
    }

    /// Adds an instance of `mesh` with the next frame.
    fn add(&mut self, mesh: MeshHandle, transform: Mat4) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform });
        id
    }

    /// Lists instance `id` as a character, lit by the world's key light.
    fn character(&mut self, id: InstanceId) {
        self.characters.push(CharacterInstance {
            id,
            lighting: CharacterLighting::unstyled(self.sun),
            face: CharacterFace::default(),
            body: CharacterBody::default(),
            opacity: 1.0,
        });
    }

    /// The unit vector toward the key light, as the shaders take it.
    fn toward_key(&self) -> Vec3 {
        (-self.sun.direction).normalize()
    }

    /// Draws one frame and reads the target back. Fails on any validation error.
    fn render(&mut self) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = self.sun;
        packet.sky = self.sky;
        packet.ambient = self.ambient;
        packet.fog = self.fog;
        packet.post = passthrough();
        packet.characters.extend_from_slice(&self.characters);
        packet.instances.changed.append(&mut self.added);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        Frame { pixels: self.read_back(), camera: self.camera }
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
        let pixels = {
            let mapped = buffer.get_mapped_range(..).expect("mapped");
            mapped
                .chunks_exact(padded as usize)
                .flat_map(|row| row[..(SIDE * 4) as usize].as_chunks::<4>().0.iter().copied())
                .collect()
        };
        buffer.unmap();
        pixels
    }
}

/// The worst difference over `pixels` between the frame and `expected` (None skips a pixel),
/// where it was, and how many pixels were compared.
fn compare(frame: &Frame, mut expected: impl FnMut(u32, u32) -> Option<Vec3>) -> (f32, (u32, u32), usize) {
    let mut worst = (0.0f32, (0, 0));
    let mut compared = 0;
    for y in 0..SIDE {
        for x in 0..SIDE {
            let Some(expected) = expected(x, y) else { continue };
            compared += 1;
            let error = (frame.linear(x, y) - expected).abs().max_element();
            if error > worst.0 {
                worst = (error, (x, y));
            }
        }
    }
    (worst.0, worst.1, compared)
}

/// A glowing floor under the camera and a glowing wall far off, under the sky.
const FLOOR: Rect = Rect {
    center: Vec3::new(0.0, 0.0, -60.0),
    normal: Vec3::Y,
    across: Vec3::X,
    width: 160.0,
    height: 120.0,
};
const WALL: Rect = Rect {
    center: Vec3::new(30.0, 8.0, -110.0),
    normal: Vec3::Z,
    across: Vec3::X,
    width: 60.0,
    height: 16.0,
};

#[test]
#[ignore = "needs a GPU"]
fn the_world_and_the_sky_take_the_model_s_fog() {
    let mut harness = Harness::exact();
    let (floor_glow, wall_glow) = (Vec3::new(0.1, 0.25, 0.05), Vec3::new(0.3, 0.15, 0.1));
    let floor = harness.material(&glowing(floor_glow));
    let wall = harness.material(&glowing(wall_glow));
    let scene = harness.mesh(&mesh(vec![FLOOR.part(), WALL.part()]), &[floor, wall]);
    harness.add(scene, Mat4::IDENTITY);
    // Inside the height layer, 3 m up, looking a little down across the floor to the wall, the
    // horizon and the sky above it; the key light ahead and low, so its lobe is in view.
    harness.camera =
        CameraData::look_at(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, 2.0, -20.0), Vec3::Y, 1.2, 1.0, 0.05);
    harness.sun.direction = -Vec3::new(0.2, 0.15, -1.0).normalize();
    harness.fog = thick_fog();
    let frame = harness.render();
    let (fog, toward_key) = (harness.fog, harness.toward_key());
    let (mut floors, mut walls, mut skies) = (0, 0, 0);
    let (worst, at, compared) = compare(&frame, |x, y| {
        let (eye, dir) = frame.ray(x, y);
        // Clear of the edges by a pixel's footprint, where a pixel center may fall either side.
        let margin = 0.6;
        let floor = FLOOR.hit(eye, dir, margin).map(|distance| (distance, floor_glow));
        let wall = WALL.hit(eye, dir, margin).map(|distance| (distance, wall_glow));
        let near_an_edge = |rect: &Rect| rect.hit(eye, dir, -margin).is_some();
        let nearest = match (wall, floor) {
            (Some(wall), Some(floor)) => Some(if wall.0 < floor.0 { wall } else { floor }),
            (wall, floor) => wall.or(floor),
        };
        match nearest {
            Some((distance, glow)) => {
                if glow == floor_glow {
                    floors += 1
                } else {
                    walls += 1
                }
                Some(fog.ray(eye, dir, distance, 1.0, SKY.horizon, toward_key).apply(glow))
            }
            None if near_an_edge(&FLOOR) || near_an_edge(&WALL) => None,
            None => {
                skies += 1;
                Some(
                    fog.ray(eye, dir, SKY_DISTANCE, 1.0, SKY.horizon, toward_key).apply(sky_color(&SKY, dir)),
                )
            }
        }
    });
    assert!(floors > 2000 && walls > 100 && skies > 2000, "{floors} floor, {walls} wall, {skies} sky pixels");
    assert!(worst < TOLERANCE, "{worst} off the model at {at:?} ({compared} pixels)");
}

#[test]
#[ignore = "needs a GPU"]
fn distant_geometry_meets_the_sky_without_a_seam() {
    // The floor's far edge, 1 km out, against the sky just above the horizon: the fog over both
    // has saturated, so the two sides of the edge agree.
    let mut harness = Harness::exact();
    let floor = harness.material(&glowing(Vec3::new(0.05, 0.3, 0.05)));
    let far_floor = Rect { center: Vec3::new(0.0, 0.0, -500.0), width: 400.0, height: 1000.0, ..FLOOR };
    let scene = harness.mesh(&mesh(vec![far_floor.part()]), &[floor]);
    harness.add(scene, Mat4::IDENTITY);
    harness.camera =
        CameraData::look_at(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 2.0, -10.0), Vec3::Y, 0.6, 1.0, 0.05);
    harness.fog = thick_fog();
    let frame = harness.render();
    let edge = frame.pixel_of(Vec3::new(0.0, 0.0, -1000.0));
    let (below, above) = (frame.linear(edge.0, edge.1 + 2), frame.linear(edge.0, edge.1.saturating_sub(2)));
    assert!((below - above).abs().max_element() < 0.02, "a seam: {below} below the edge, {above} above");
    // Without fog the edge shows.
    harness.fog = Fog::default();
    let clear = harness.render();
    let (below, above) = (clear.linear(edge.0, edge.1 + 2), clear.linear(edge.0, edge.1.saturating_sub(2)));
    assert!((below - above).abs().max_element() > 0.1, "{below} against {above}");
}

#[test]
#[ignore = "needs a GPU"]
fn a_frame_without_density_draws_exactly_as_without_fog() {
    let mut harness = Harness::exact();
    let floor = harness.material(&glowing(Vec3::new(0.1, 0.25, 0.05)));
    let character = harness.material(&flat_character(Vec3::new(0.8, 0.5, 0.4)));
    let haze = harness
        .material(&FogCardMaterial { camera_fade: [1.0, 4.0], ..card(Vec3::ONE, 0.5, CardFacing::World) });
    let card = Rect {
        center: Vec3::new(-2.0, 1.0, -12.0),
        normal: Vec3::Z,
        across: Vec3::X,
        width: 4.0,
        height: 3.0,
    };
    let scene = harness.mesh(&mesh(vec![FLOOR.part(), card.part()]), &[floor, haze]);
    harness.add(scene, Mat4::IDENTITY);
    let figure = harness.mesh(&mesh(vec![sphere(1.0)]), &[character]);
    let id = harness.add(figure, Mat4::from_translation(Vec3::new(2.0, 1.0, -8.0)));
    harness.character(id);
    harness.camera =
        CameraData::look_at(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 1.0, -10.0), Vec3::Y, 1.0, 1.0, 0.05);
    let without = harness.render();
    // Colors, inscatter and the characters' share, but no density: nothing to draw.
    harness.fog = Fog {
        near: FogLayer { density: 0.0, ..thick_fog().near },
        far: FogLayer { density: 0.0, ..thick_fog().far },
        height: HeightFog { density: 0.0, ..thick_fog().height },
        ..thick_fog()
    };
    let none = harness.render();
    assert!(without.pixels == none.pixels, "every pixel as without fog");
    harness.fog = thick_fog();
    let fogged = harness.render();
    assert!(without.pixels != fogged.pixels, "and fog does draw");
}

#[test]
#[ignore = "needs a GPU"]
fn characters_take_their_share_of_the_fog() {
    // Two spheres in the anime character material at the same distance: the left one listed as
    // a character, the right one not (a prop, which fogs as the world). Each pixel's fogged color
    // is its clear color under the model's fog, with the characters' share for the left.
    let mut harness = Harness::exact();
    // Both lit alike, from behind the camera (the inscatter lobe out of view).
    harness.sun = DirectionalLight {
        direction: Vec3::new(-0.3, -0.5, -0.8).normalize(),
        color: Vec3::ONE,
        intensity: 1.0,
    };
    let material = harness.material(&flat_character(Vec3::new(0.8, 0.5, 0.4)));
    let figure = harness.mesh(&mesh(vec![sphere(1.0)]), &[material]);
    let (left, right) = (Vec3::new(-1.3, 2.0, -24.0), Vec3::new(1.3, 2.0, -24.0));
    let character = harness.add(figure, Mat4::from_translation(left));
    harness.add(figure, Mat4::from_translation(right));
    harness.character(character);
    harness.camera =
        CameraData::look_at(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 2.0, -24.0), Vec3::Y, 0.2, 1.0, 0.05);
    let clear = harness.render();
    harness.fog = thick_fog();
    let fogged = harness.render();
    let (fog, toward_key) = (harness.fog, harness.toward_key());
    for (center, share) in [(left, fog.characters), (right, 1.0)] {
        let (worst, at, compared) = compare(&fogged, |x, y| {
            let (eye, dir) = fogged.ray(x, y);
            let to_center = center - eye;
            let along = to_center.dot(dir);
            let miss = (to_center - dir * along).length();
            // Well inside the silhouette, where every pixel is the sphere.
            (miss < 0.8).then(|| {
                let distance = along - (1.0 - miss * miss).sqrt();
                fog.ray(eye, dir, distance, share, SKY.horizon, toward_key).apply(clear.linear(x, y))
            })
        });
        assert!(compared > 100, "{compared} pixels");
        assert!(worst < TOLERANCE, "share {share}: {worst} off at {at:?}");
    }
    // The character reads: less of the fog than the prop beside it.
    let pixel = |frame: &Frame, at: Vec3| {
        let (x, y) = frame.pixel_of(at);
        frame.linear(x, y)
    };
    let fogged_by = |at: Vec3| (pixel(&fogged, at) - pixel(&clear, at)).abs().max_element();
    assert!(fogged_by(left + Vec3::Z) < 0.5 * fogged_by(right + Vec3::Z));
}

#[test]
#[ignore = "needs a GPU"]
fn fog_cards_fade_toward_the_lens_and_face_as_they_are_made_to() {
    // Black ground and a black wall behind, a sky whose horizon lights the cards (no fog, so
    // their light is the horizon's).
    let mut harness = Harness::exact();
    let horizon = Vec3::splat(0.5);
    harness.sky = SkyGradient { zenith: horizon, horizon, ground: horizon };
    let black = harness.material(&glowing(Vec3::ZERO));
    let ground = Rect { center: Vec3::new(0.0, -1.5, -20.0), ..FLOOR };
    let wall = Rect {
        center: Vec3::new(0.0, 0.0, -30.0),
        normal: Vec3::Z,
        across: Vec3::X,
        width: 100.0,
        height: 60.0,
    };
    let world = harness.mesh(&mesh(vec![ground.part(), wall.part()]), &[black, black]);
    harness.add(world, Mat4::IDENTITY);
    // A sheet lying across the ground, fading in from 2 m to 12 m.
    let fading = harness
        .material(&FogCardMaterial { camera_fade: [2.0, 12.0], ..card(Vec3::ONE, 0.8, CardFacing::World) });
    let sheet = Rect {
        center: Vec3::new(0.0, -1.0, -11.0),
        normal: Vec3::Y,
        across: Vec3::X,
        width: 4.0,
        height: 20.0,
    };
    let sheet_mesh = harness.mesh(&mesh(vec![sheet.part()]), &[fading]);
    harness.add(sheet_mesh, Mat4::IDENTITY);
    // Unit cards turned away from the camera, scaled to 2 m: one turned to face it whole, one
    // turned about the up axis alone, and one as placed.
    let unit = mesh(vec![
        Rect { center: Vec3::ZERO, normal: Vec3::Z, across: Vec3::X, width: 1.0, height: 1.0 }.part(),
    ]);
    let facings = [
        (CardFacing::Camera, Vec3::new(-4.0, 2.0, -12.0)),
        (CardFacing::Upright, Vec3::new(0.0, 2.5, -12.0)),
        (CardFacing::World, Vec3::new(4.0, 2.0, -12.0)),
    ];
    let turn = Quat::from_rotation_y(1.0) * Quat::from_rotation_x(0.5);
    for (facing, at) in facings {
        let material = harness.material(&card(Vec3::ONE, 1.0, facing));
        let handle = harness.mesh(&unit, &[material]);
        harness.add(handle, Mat4::from_scale_rotation_translation(Vec3::new(2.0, 2.0, 1.0), turn, at));
    }
    harness.camera =
        CameraData::look_at(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 0.0, -12.0), Vec3::Y, 1.1, 1.0, 0.05);
    let frame = harness.render();

    // The sheet: its opacity × the camera fade at each pixel's distance, over black.
    let (worst, at, compared) = compare(&frame, |x, y| {
        let (eye, dir) = frame.ray(x, y);
        let distance = sheet.hit(eye, dir, 0.1)?;
        Some(horizon * 0.8 * smoothstep(2.0, 12.0, distance))
    });
    assert!(compared > 500, "{compared} pixels");
    assert!(worst < TOLERANCE, "the sheet's fade: {worst} off at {at:?}");

    // Each turned card's coverage against the plane it should face in.
    let view = frame.camera.view;
    let camera_right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
    let camera_up = Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y);
    let level_right = Vec3::new(camera_right.x, 0.0, camera_right.z).normalize();
    let planes = [
        (camera_right, camera_up, facings[0].1),
        (level_right, Vec3::Y, facings[1].1),
        (turn * Vec3::X, turn * Vec3::Y, facings[2].1),
    ];
    for ((right, up, center), (facing, _)) in planes.into_iter().zip(facings) {
        let normal = right.cross(up).normalize();
        let normal = if normal.dot(frame.camera.position - center) > 0.0 { normal } else { -normal };
        let across = if normal.cross(right).dot(up) > 0.0 { right } else { -right };
        let expected = Rect { center, normal, across, width: 2.0, height: 2.0 };
        let mut mismatched = 0;
        let mut covered = 0;
        for y in 0..SIDE {
            for x in 0..SIDE {
                let (eye, dir) = frame.ray(x, y);
                let inside = expected.hit(eye, dir, 0.08).is_some();
                let outside = expected.hit(eye, dir, -0.08).is_none();
                let drawn = frame.linear(x, y).max_element() > 0.25;
                covered += usize::from(inside);
                // Around the card alone: the sheet and the other cards draw elsewhere.
                let (cx, cy) = frame.pixel_of(center);
                let near = (cx as i32 - x as i32).abs() < 16 && (cy as i32 - y as i32).abs() < 16;
                if (inside && !drawn) || (outside && drawn && near) {
                    mismatched += 1;
                }
            }
        }
        assert!(covered > 40, "{facing:?}: {covered} pixels");
        assert!(mismatched == 0, "{facing:?}: {mismatched} pixels off its plane");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn fog_cards_soften_where_they_meet_geometry_under_temporal_aa() {
    let horizon = Vec3::splat(0.5);
    // Two cards parallel to a black wall 10 m off: one 0.5 m in front of it, one 3 m in front,
    // fading over 2 m where they meet geometry.
    let draw = |temporal: bool| {
        let mut harness =
            Harness::new(RenderSettings { msaa: Msaa::Off, temporal, ..RenderSettings::default() });
        harness.sky = SkyGradient { zenith: horizon, horizon, ground: horizon };
        let black = harness.material(&glowing(Vec3::ZERO));
        let wall = Rect {
            center: Vec3::new(0.0, 0.0, -10.0),
            normal: Vec3::Z,
            across: Vec3::X,
            width: 40.0,
            height: 40.0,
        };
        let soft =
            harness.material(&FogCardMaterial { depth_fade: 2.0, ..card(Vec3::ONE, 1.0, CardFacing::World) });
        let close = Rect {
            center: Vec3::new(-1.5, 0.0, -9.5),
            normal: Vec3::Z,
            across: Vec3::X,
            width: 2.0,
            height: 3.0,
        };
        let clear = Rect { center: Vec3::new(1.5, 0.0, -7.0), ..close };
        let world = harness.mesh(&mesh(vec![wall.part(), close.part(), clear.part()]), &[black, soft, soft]);
        harness.add(world, Mat4::IDENTITY);
        harness.camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 0.8, 1.0, 0.05);
        let mut frame = harness.render();
        for _ in 0..24 {
            frame = harness.render();
        }
        let at = |point: Vec3| {
            let (x, y) = frame.pixel_of(point);
            frame.linear(x, y)
        };
        (at(Vec3::new(-1.5, 0.0, -9.5)), at(Vec3::new(1.5, 0.0, -7.0)))
    };
    // The desktop tiers' temporal AA runs the scene prepass, whose depth the cards fade against:
    // a quarter of the near card is left, all of the far one.
    let (close, clear) = draw(true);
    assert!((close - horizon * 0.25).abs().max_element() < 0.03, "{close}");
    assert!((clear - horizon).abs().max_element() < 0.03, "{clear}");
    // Without it (phones' MSAA), no depth: whole cards, the map's soft edges carrying them.
    let (close, clear) = draw(false);
    assert!((close - horizon).abs().max_element() < TOLERANCE, "{close}");
    assert!((clear - horizon).abs().max_element() < TOLERANCE, "{clear}");
}

#[test]
#[ignore = "needs a GPU"]
fn fog_cards_draw_back_to_front() {
    // Two half-opaque cards overlapping, added nearest first: blended back to front, the near
    // one's color covers half of the far one's.
    let mut harness = Harness::exact();
    let horizon = Vec3::splat(0.8);
    harness.sky = SkyGradient { zenith: horizon, horizon, ground: horizon };
    let black = harness.material(&glowing(Vec3::ZERO));
    let wall = Rect {
        center: Vec3::new(0.0, 0.0, -30.0),
        normal: Vec3::Z,
        across: Vec3::X,
        width: 60.0,
        height: 60.0,
    };
    let world = harness.mesh(&mesh(vec![wall.part()]), &[black]);
    harness.add(world, Mat4::IDENTITY);
    let unit = mesh(vec![
        Rect { center: Vec3::ZERO, normal: Vec3::Z, across: Vec3::X, width: 4.0, height: 4.0 }.part(),
    ]);
    let (red, green) = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
    for (tint, z) in [(green, -10.0), (red, -20.0)] {
        let material = harness.material(&card(tint, 0.5, CardFacing::World));
        let handle = harness.mesh(&unit, &[material]);
        harness.add(handle, Mat4::from_translation(Vec3::new(0.0, 0.0, z)));
    }
    harness.camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 0.4, 1.0, 0.05);
    for _ in 0..2 {
        let frame = harness.render();
        let (x, y) = frame.pixel_of(Vec3::new(0.0, 0.0, -10.0));
        let expected = green * horizon * 0.5 + red * horizon * 0.5 * 0.5;
        let drawn = frame.linear(x, y);
        assert!((drawn - expected).abs().max_element() < TOLERANCE, "{drawn} against {expected}");
    }
}
