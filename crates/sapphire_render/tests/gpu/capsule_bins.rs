//! The characters' capsules by cluster (M5 L18): with 16 capsules or fewer a frame draws the same
//! bytes with bins on as with the plain loops, on every shipped tier, and past 16 every body in a
//! crowd casts its shadow and its occlusion. Run with
//! `cargo test -p sapphire_render --test gpu capsule_bins:: -- --ignored --test-threads=1`.
//!
//! The scenes are environment surfaces (a long ground and a few walls), which take both lists, and
//! characters whose bodies are the capsules: anime character cubes, ambient characters, so every
//! body casts its capsule on every tier rather than a map.
//!
//! Set `SAPPHIRE_CAPSULE_DUMP=<dir>` to write the frames compared there as PNGs.

use glam::{Mat4, Quat, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, DistanceStylization, EnvironmentMaterial, Material, MaterialDocument, Outline,
    Rim, Terminator,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::quality::{TierId, TierTable};
use sapphire_render::{
    AmbientLight, AmbientOcclusion, BloomParams, CameraData, CharacterBody, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, FramePacketRing, InstanceId, MaterialDrive, MeshHandle,
    MeshInstance, Msaa, PostParams, RenderSettings, RenderStats, Renderer, ShadowRole, SkyGradient,
    ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

fn vertex(position: Vec3, normal: Vec3) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv: [0.0, 0.0],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    }
}

fn mesh_of<D: MaterialDocument>(vertices: &[Vertex], indices: Vec<u32>, document: &D) -> Mesh {
    let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
    let material = Material::from_document(document, &swizzles).expect("a valid material");
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    Mesh::from_vertices(
        vertices,
        indices,
        submeshes,
        vec![MaterialSlot::embedded("surface", material)],
        PositionFormat::Unorm16,
    )
}

/// The square from −1 to 1 in X and Z, facing up.
fn ground<D: MaterialDocument>(document: &D) -> Mesh {
    let vertices: Vec<Vertex> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .map(|(x, z)| vertex(Vec3::new(x, 0.0, z), Vec3::Y))
        .collect();
    mesh_of(&vertices, vec![0, 2, 1, 0, 3, 2], document)
}

/// The unit box standing on the origin, hard-edged.
fn cuboid<D: MaterialDocument>(document: &D) -> Mesh {
    let faces = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for (normal, u, v) in faces {
        let first = vertices.len() as u32;
        for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            vertices.push(vertex(normal * 0.5 + u * a + v * b + Vec3::Y * 0.5, normal));
        }
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    mesh_of(&vertices, indices, document)
}

/// Lambert's environment surface with a one-pixel terminator and nothing stylized.
fn environment() -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [0.7, 0.7, 0.7, 1.0],
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    }
}

fn character_surface() -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [0.8, 0.5, 0.4, 1.0],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 1.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    }
}

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

/// A capsule body: `(base, top, radius)`, and whether a cube stands for it in view.
#[derive(Clone, Copy, Debug)]
struct Body {
    base: Vec3,
    top: Vec3,
    radius: f32,
    shown: bool,
}

/// What a scene holds besides its ground: walls and bodies, a camera and a light.
#[derive(Clone, Debug)]
struct Scene {
    camera: CameraData,
    toward_light: Vec3,
    walls: Vec<Mat4>,
    bodies: Vec<Body>,
    sun: f32,
    ambient: f32,
}

/// One renderer drawing a scene into a target.
struct Shot {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    texture: wgpu::Texture,
    scene: Scene,
    added: Vec<MeshInstance>,
    characters: Vec<CharacterInstance>,
}

impl Shot {
    fn new(gpu: &GpuContext, settings: RenderSettings, size: (u32, u32), scene: Scene) -> Self {
        let mut renderer = Renderer::new(gpu, FORMAT, settings).expect("renderer");
        let mut upload = |mesh: Mesh| -> MeshHandle {
            let bytes = mesh.to_cooked().expect("cook");
            let mesh = Mesh::read(&bytes).expect("a valid mesh");
            let materials = renderer.upload_embedded_materials(mesh).expect("materials");
            renderer.upload_mesh(mesh, &materials).expect("upload")
        };
        let (floor, wall, figure) = (
            upload(ground(&environment())),
            upload(cuboid(&environment())),
            upload(cuboid(&character_surface())),
        );
        let mut added = vec![MeshInstance {
            id: InstanceId(0),
            mesh: floor,
            transform: Mat4::from_scale(Vec3::new(400.0, 1.0, 400.0)),
        }];
        for (index, &transform) in scene.walls.iter().enumerate() {
            added.push(MeshInstance { id: InstanceId(1 + index as u64), mesh: wall, transform });
        }
        let mut characters = Vec::new();
        let light =
            DirectionalLight { direction: -scene.toward_light, color: Vec3::ONE, intensity: scene.sun };
        for (index, body) in scene.bodies.iter().enumerate() {
            let id = InstanceId(1000 + index as u64);
            // A cube about the body where it shows; a speck inside its base where it doesn't.
            let transform = if body.shown {
                let height = body.base.distance(body.top).max(0.2);
                Mat4::from_scale_rotation_translation(
                    Vec3::new(body.radius * 1.2, height, body.radius * 1.2),
                    Quat::IDENTITY,
                    body.base,
                )
            } else {
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(1e-3),
                    Quat::IDENTITY,
                    body.base - Vec3::Y * 0.01,
                )
            };
            added.push(MeshInstance { id, mesh: figure, transform });
            characters.push(CharacterInstance {
                id,
                lighting: CharacterLighting::unstyled(light),
                face: Default::default(),
                body: CharacterBody::capsule(body.base, body.top, body.radius).with_role(ShadowRole::Ambient),
                opacity: 1.0,
                drive: MaterialDrive::NONE,
            });
        }
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            texture,
            scene,
            added,
            characters,
        }
    }

    /// Draws a frame at render `scale` and reads it back.
    fn frame(&mut self, gpu: &GpuContext, scale: f32) -> Vec<[u8; 4]> {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.scene.camera));
        packet.sun = DirectionalLight {
            direction: -self.scene.toward_light,
            color: Vec3::ONE,
            intensity: self.scene.sun,
        };
        packet.sky =
            SkyGradient { zenith: Vec3::splat(0.3), horizon: Vec3::splat(0.4), ground: Vec3::splat(0.2) };
        packet.ambient =
            AmbientLight { sky: Vec3::splat(1.0), ground: Vec3::splat(0.6), intensity: self.scene.ambient };
        packet.shadow_tint = Vec3::new(0.3, 0.35, 0.5);
        packet.post = passthrough();
        packet.characters.extend_from_slice(&self.characters);
        packet.instances.changed.append(&mut self.added);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.set_render_scale(scale);
        self.renderer.prepare(gpu, self.packets.current());
        let view = self.texture.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        read_back(gpu, &self.texture)
    }

    fn stats(&self) -> RenderStats {
        self.renderer.stats()
    }
}

fn read_back(gpu: &GpuContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped
        .chunks_exact(padded as usize)
        .flat_map(|row| row[..width as usize * 4].as_chunks::<4>().0.iter().copied())
        .collect()
}

fn dump(name: &str, pixels: &[[u8; 4]], (width, height): (u32, u32)) {
    let Some(dir) = std::env::var_os("SAPPHIRE_CAPSULE_DUMP") else { return };
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    let file = std::fs::File::create(&path).expect("create the dump");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("header");
    writer.write_image_data(bytemuck::cast_slice(pixels)).expect("pixels");
}

/// A vector toward the light at `elevation` degrees, its horizontal part `azimuth` degrees from +X
/// toward +Z.
fn toward(elevation: f32, azimuth: f32) -> Vec3 {
    let (e, a) = (elevation.to_radians(), azimuth.to_radians());
    Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin())
}

/// The scenes compared: 16 bodies or fewer, placed where binning could go wrong.
fn equality_scenes(aspect: f32) -> Vec<(&'static str, Scene)> {
    let eye = Vec3::new(0.0, 1.6, 6.0);
    let camera = CameraData::look_at(eye, Vec3::new(0.0, 0.8, -6.0), Vec3::Y, 0.95, aspect, 0.1);
    let forward = (Vec3::new(0.0, 0.8, -6.0) - eye).normalize();
    let walls = vec![
        Mat4::from_scale_rotation_translation(
            Vec3::new(8.0, 3.0, 0.3),
            Quat::IDENTITY,
            Vec3::new(0.0, 0.0, -12.0),
        ),
        Mat4::from_scale_rotation_translation(
            Vec3::new(0.3, 2.5, 10.0),
            Quat::IDENTITY,
            Vec3::new(-4.5, 0.0, -3.0),
        ),
    ];
    let stand = |x: f32, z: f32, shown: bool| Body {
        base: Vec3::new(x, 0.15, z),
        top: Vec3::new(x, 1.5, z),
        radius: 0.3,
        shown,
    };
    let mut bodies = vec![
        // Across the near plane: its capsule round the camera's eye.
        Body {
            base: eye - Vec3::Y * 1.4 + forward * 0.05,
            top: eye + Vec3::Y * 0.2,
            radius: 0.3,
            shown: false,
        },
        // Behind the camera, its shadow and occlusion reaching forward.
        stand(0.8, 7.5, false),
        // Lying on the ground across many clusters.
        Body { base: Vec3::new(-3.0, 0.3, -2.0), top: Vec3::new(3.0, 0.3, -9.0), radius: 0.35, shown: true },
        // At the frame's edges and corners.
        stand(-3.6, 1.0, true),
        stand(3.4, 1.5, false),
        stand(-6.0, -9.0, true),
        stand(6.5, -10.0, true),
        // A tall one, and a few ordinary ones near and far.
        Body { base: Vec3::new(1.5, 0.0, -4.0), top: Vec3::new(1.5, 4.0, -4.0), radius: 0.5, shown: true },
    ];
    for i in 0..8 {
        let z = -1.0 - 3.0 * i as f32;
        bodies.push(stand(-1.5 + 0.4 * i as f32, z, i % 2 == 0));
    }
    assert_eq!(bodies.len(), 16);
    let scene = |toward_light: Vec3| Scene {
        camera,
        toward_light,
        walls: walls.clone(),
        bodies: bodies.clone(),
        sun: 1.0,
        ambient: 0.5,
    };
    vec![
        ("noon", scene(toward(60.0, 30.0))),
        // Long shadows across many clusters and slices.
        ("grazing", scene(toward(6.0, -100.0))),
        ("grazing toward the camera", scene(toward(8.0, 95.0))),
    ]
}

/// Frames drawn per scene: enough for temporal AA's jitter to move between them.
const FRAMES: u32 = 3;

/// Draws each scene with bins on and off from separate renderers and compares every byte of every
/// frame.
fn assert_equal(gpu: &GpuContext, name: &str, settings: RenderSettings, size: (u32, u32), scale: f32) -> u32 {
    let mut compared = 0;
    for (scene_name, scene) in equality_scenes(size.0 as f32 / size.1 as f32) {
        let mut on = Shot::new(gpu, RenderSettings { capsule_bins: true, ..settings }, size, scene.clone());
        let mut off = Shot::new(gpu, RenderSettings { capsule_bins: false, ..settings }, size, scene);
        for frame in 0..FRAMES {
            let (binned, plain) = (on.frame(gpu, scale), off.frame(gpu, scale));
            let stats = on.stats().capsule_bins;
            assert!(stats.enabled && stats.marked_clusters > 0, "{name}, {scene_name}: binned {stats:?}");
            assert!(stats.shadow_capsules + stats.occlusion_capsules > 0, "{name}, {scene_name}: {stats:?}");
            assert!(!off.stats().capsule_bins.enabled);
            if binned != plain {
                dump(&format!("{name}-{scene_name}-on"), &binned, size);
                dump(&format!("{name}-{scene_name}-off"), &plain, size);
                let differing = binned.iter().zip(&plain).filter(|(a, b)| a != b).count();
                panic!("{name}, {scene_name}, frame {frame}: {differing} pixels differ");
            }
            compared += 1;
        }
    }
    compared
}

#[test]
#[ignore = "needs a GPU"]
fn sixteen_capsules_or_fewer_draw_the_same_bytes_binned_on_every_tier() {
    let gpu = gpu();
    let table = TierTable::shipped();
    let mut compared = 0;
    for tier in table.tiers() {
        let settings = tier.render_settings();
        assert!(settings.ambient_occlusion.capsules && settings.shadows.is_some(), "{:?}", tier.id);
        let name = format!("{:?}", tier.id);
        compared += assert_equal(&gpu, &name, settings, (480, 270), 1.0);
        // Below render scale 1.
        compared += assert_equal(&gpu, &format!("{name} at 0.7"), settings, (480, 270), 0.7);
    }
    // With and without MSAA on the desktop's settings.
    let desktop = table.get(TierId::DesktopHigh).render_settings();
    let msaa = RenderSettings { msaa: Msaa::X4, temporal: false, ..desktop };
    compared += assert_equal(&gpu, "desktop with 4x MSAA", msaa, (480, 270), 1.0);
    let single = RenderSettings { msaa: Msaa::Off, temporal: false, ..desktop };
    compared += assert_equal(&gpu, "desktop single-sampled", single, (480, 270), 1.0);
    assert!(compared > 0);
}

/// 60 bodies standing in a grid on a flat floor, seen from above, unseen themselves: what their
/// capsules leave on the ground.
fn crowd(sun: f32, ambient: f32) -> Scene {
    let mut bodies = Vec::new();
    for row in 0..6 {
        for column in 0..10 {
            // Bodies stand above the ground as a character's do, from its knees.
            let at = Vec3::new(-9.0 + 2.0 * column as f32, 0.35, -5.0 + 2.0 * row as f32);
            bodies.push(Body { base: at, top: at + Vec3::Y * 1.25, radius: 0.3, shown: false });
        }
    }
    Scene {
        camera: CameraData::look_at(
            Vec3::new(0.0, 30.0, 0.01),
            Vec3::ZERO,
            Vec3::NEG_Z,
            0.9,
            16.0 / 9.0,
            0.5,
        ),
        toward_light: toward(75.0, 20.0),
        walls: Vec::new(),
        bodies,
        sun,
        ambient,
    }
}

fn pixel_of(camera: &CameraData, (width, height): (u32, u32), point: Vec3) -> usize {
    let clip = camera.proj * camera.view * point.extend(1.0);
    let (x, y) = (clip.x / clip.w, clip.y / clip.w);
    let (px, py) = ((x + 1.0) * 0.5 * width as f32, (1.0 - y) * 0.5 * height as f32);
    assert!((0.0..width as f32).contains(&px) && (0.0..height as f32).contains(&py), "{point} out of view");
    py as usize * width as usize + px as usize
}

fn red(pixels: &[[u8; 4]], at: usize) -> u8 {
    pixels[at][0]
}

#[test]
#[ignore = "needs a GPU"]
fn every_body_of_a_crowd_of_sixty_shadows_and_occludes_the_ground() {
    let gpu = gpu();
    let size = (640, 360);
    let desktop = TierTable::shipped().get(TierId::DesktopHigh).render_settings();
    let settings = RenderSettings { msaa: Msaa::X4, temporal: false, ..desktop };
    // The key light's shadows alone, no ambient: under each body, its shadow's tint.
    let shadows = RenderSettings { ambient_occlusion: AmbientOcclusion::OFF, ..settings };
    // Capsule AO alone, no key light: at each body's foot, the ambient light it takes.
    let occlusion = RenderSettings {
        shadows: None,
        ambient_occlusion: AmbientOcclusion { capsules: true, gtao: None },
        ..settings
    };
    for (name, settings, scene, offset) in
        [("shadows", shadows, crowd(1.0, 0.0), 0.0), ("occlusion", occlusion, crowd(0.0, 1.0), 0.1)]
    {
        let camera = scene.camera;
        let open = {
            let mut empty = Shot::new(&gpu, settings, size, Scene { bodies: Vec::new(), ..scene.clone() });
            empty.frame(&gpu, 1.0)
        };
        for binned in [true, false] {
            let mut shot =
                Shot::new(&gpu, RenderSettings { capsule_bins: binned, ..settings }, size, scene.clone());
            let pixels = shot.frame(&gpu, 1.0);
            dump(&format!("crowd-{name}-{}", if binned { "on" } else { "off" }), &pixels, size);
            let stats = shot.stats();
            let (mut marked, mut reds) = (0, Vec::new());
            for body in &scene.bodies {
                // Where its shadow or occlusion falls on the ground: below its middle along the
                // light, or at its foot.
                let middle = (body.base + body.top) * 0.5;
                let ground = if offset > 0.0 {
                    Vec3::new(body.base.x + offset, 0.0, body.base.z)
                } else {
                    middle - scene.toward_light * (middle.y / scene.toward_light.y)
                };
                let at = pixel_of(&camera, size, ground);
                reds.push((red(&pixels, at), red(&open, at)));
                if red(&pixels, at) + 8 < red(&open, at) {
                    marked += 1;
                }
            }
            if binned {
                assert_eq!(
                    marked,
                    scene.bodies.len(),
                    "{name}: every body marks the ground, {reds:?} {stats:?}"
                );
                let bins = stats.capsule_bins;
                let listed = if name == "shadows" { bins.shadow_capsules } else { bins.occlusion_capsules };
                assert_eq!(listed, 60, "{bins:?}");
            } else {
                assert_eq!(marked, 16, "{name}: the plain loops' 16, {reds:?}");
            }
        }
    }
}
