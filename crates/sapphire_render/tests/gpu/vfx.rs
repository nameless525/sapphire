//! The frame's effects (M2 E2): where a particle's pixels land, what the fog and the exposure do to
//! them, depth on both paths, the ribbon's cross-section, that nothing reaches temporal AA's history,
//! and that a frame without effects pays nothing. Run with
//! `cargo test -p sapphire_render --test gpu vfx:: -- --ignored --test-threads=1`.
//!
//! Scenes show linear values as they are (`passthrough`), over a flat grey sky, so each pixel can be
//! predicted.

use glam::{Mat4, Vec3};
use sapphire_assets::material::{
    AnimeCharacterMaterial, CharacterRegion, EnvironmentMaterial, LightSplit, Material, MaterialDocument,
    Outline, Rim, Terminator,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::Swizzle;
use sapphire_core::debug_draw::{Color, DebugLine, Depth};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::{
    BloomParams, CameraData, CharacterBloom, CharacterBody, CharacterFace, CharacterInstance,
    CharacterLighting, ColorGrade, DirectionalLight, Fog, FogLayer, FramePacket, FramePacketRing, HeightFog,
    InstanceId, MaterialHandle, MeshHandle, MeshInstance, Msaa, PostParams, RenderSettings, Renderer,
    ShaderFile, ShaderSources, ShaderUpdate, SkyGradient, ToneCurve, VfxLight, VfxRibbonPoint, VfxShape,
    VfxSprite, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIDE: u32 = 128;
/// The sky every scene draws over, linear.
const GREY: f32 = 0.25;
/// How far a pixel's linear color may be from the model: 8-bit output.
const TOLERANCE: f32 = 0.02;

/// Post that shows linear values below about 0.98 as they are.
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

/// A flat emissive dot of half-width `half`, whose middle is `color` wholly, as light (additive).
fn dot(position: Vec3, half: f32, color: Vec3) -> VfxSprite {
    VfxSprite {
        position,
        axis: Vec3::ZERO,
        rotation: 0.0,
        half_width: half,
        half_length: half,
        color,
        alpha: 1.0,
        additive: 1.0,
        soft: 1e-3,
        shape: VfxShape::SoftDot,
        light: VfxLight::Emissive,
    }
}

/// A ribbon of two points, a strip from x -1 to 1 at depth `z`, its base rail at y -0.5 and its tip
/// at 0.5, with a thin core at 3/4 of the way across.
fn strip(z: f32, color: Vec3, core: Vec3) -> [VfxRibbonPoint; 2] {
    let point = |x: f32, ends| VfxRibbonPoint {
        base: Vec3::new(x, -0.5, z),
        tip: Vec3::new(x, 0.5, z),
        color,
        alpha: 1.0,
        core,
        core_position: 0.75,
        core_half_width: 0.05,
        glow_reach: 0.5,
        additive: 1.0,
        soft: 1e-3,
        ends,
    };
    [point(-1.0, false), point(1.0, true)]
}

fn flat_settings() -> RenderSettings {
    RenderSettings { msaa: Msaa::Off, bloom_levels: 5, ..RenderSettings::default() }
}

fn temporal_settings() -> RenderSettings {
    RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() }
}

/// Both of the paths effects draw through.
fn paths() -> [(&'static str, RenderSettings); 2] {
    [("the main pass", flat_settings()), ("after the resolve", temporal_settings())]
}

/// A black environment material, glowing with `emission`.
fn glowing(emission: Vec3) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: [0.0, 0.0, 0.0, 1.0],
        roughness: 0.5,
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        emission: emission.to_array(),
        ..EnvironmentMaterial::default()
    }
}

/// A rectangle across the view at depth `z`, wide enough to fill it at this camera's 90°.
fn wall(z: f32, half: f32) -> Mesh {
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, z],
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv: [0.5, 0.5],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let vertices = [corner(-half, -half), corner(half, -half), corner(half, half), corner(-half, half)];
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let placeholder = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).expect("valid");
    Mesh::from_vertices(
        &vertices,
        vec![0, 1, 2, 0, 2, 3],
        vec![Submesh { first_index: 0, index_count: 6, material: 0 }],
        vec![MaterialSlot::embedded("wall".to_owned(), placeholder)],
        PositionFormat::Float32,
    )
}

struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
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
            added: Vec::new(),
            next_id: 0,
        }
    }

    fn material<D: MaterialDocument>(&mut self, document: &D) -> MaterialHandle {
        let swizzles = vec![Swizzle::IDENTITY; document.textures().len()];
        let bytes = Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook");
        self.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
    }

    fn mesh(&mut self, mesh: &Mesh, material: MaterialHandle) -> MeshHandle {
        let bytes = mesh.to_cooked().expect("cook");
        self.renderer.upload_mesh(Mesh::read(&bytes).expect("a valid mesh"), &[material]).expect("upload")
    }

    fn add(&mut self, mesh: MeshHandle) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.added.push(MeshInstance { id, mesh, transform: Mat4::IDENTITY });
        id
    }

    /// A wall at depth `z` glowing `emission`.
    fn add_wall(&mut self, z: f32, emission: Vec3) -> InstanceId {
        let material = self.material(&glowing(emission));
        let mesh = self.mesh(&wall(z, 20.0), material);
        self.add(mesh)
    }

    /// Draws one frame, the camera at the origin looking down -Z over 90°, and reads it back as
    /// linear colors. `fill` adds the frame's effects, or changes its look.
    fn render(&mut self, fill: impl FnOnce(&mut FramePacket)) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(camera()));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 0.0 };
        packet.sky =
            SkyGradient { zenith: Vec3::splat(GREY), horizon: Vec3::splat(GREY), ground: Vec3::splat(GREY) };
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        fill(packet);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        Frame { pixels: self.read_back() }
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

    /// Pipelines the device holds.
    fn render_pipelines(&self) -> usize {
        let report = self.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        report.hub.render_pipelines.num_kept_from_user
    }

    /// Buffers and textures the device holds.
    fn resources(&self) -> (usize, usize) {
        let report = self.gpu.instance().raw().generate_report().expect("a wgpu-core backend");
        (report.hub.buffers.num_kept_from_user, report.hub.textures.num_kept_from_user)
    }
}

/// The camera at the origin, looking down -Z over 90°.
fn camera() -> CameraData {
    CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, std::f32::consts::FRAC_PI_2, 1.0, 0.05)
}

/// The pixel `point` projects into.
fn pixel_of(point: Vec3) -> (u32, u32) {
    let camera = camera();
    let ndc = (camera.proj * camera.view).project_point3(point);
    (((ndc.x + 1.0) * 0.5 * SIDE as f32) as u32, ((1.0 - ndc.y) * 0.5 * SIDE as f32) as u32)
}

struct Frame {
    pixels: Vec<[u8; 4]>,
}

impl Frame {
    fn linear(&self, (x, y): (u32, u32)) -> Vec3 {
        let pixel = self.pixels[(y * SIDE + x) as usize];
        Vec3::new(srgb_to_linear(pixel[0]), srgb_to_linear(pixel[1]), srgb_to_linear(pixel[2]))
    }

    /// The linear color at the middle of the frame.
    fn middle(&self) -> Vec3 {
        self.linear((SIDE / 2, SIDE / 2))
    }
}

fn assert_near(actual: Vec3, expected: Vec3, what: &str) {
    assert!((actual - expected).abs().max_element() < TOLERANCE, "{what}: {actual} against {expected}");
}

/// The readback test: a particle's pixels are where it stands, its color added over the sky, and
/// everywhere else is the sky.
#[test]
#[ignore = "needs a GPU"]
fn a_particles_pixels_are_where_it_stands() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let at = Vec3::new(1.0, 0.5, -4.0);
        let color = Vec3::new(0.6, 0.2, 0.1);
        let frame = harness.render(|packet| packet.vfx.sprites.push(dot(at, 0.5, color)));
        let (x, y) = pixel_of(at);
        assert_near(frame.linear((x, y)), Vec3::splat(GREY) + color, path);
        // 0.5 m at 4 m through this camera is 8 pixels: a dozen pixels away nothing shows.
        for away in [(x + 12, y), (x - 12, y), (x, y + 12), (x, y - 12), (8, 8)] {
            assert_near(frame.linear(away), Vec3::splat(GREY), path);
        }
        let stats = harness.renderer.stats().vfx;
        assert_eq!((stats.sprites, stats.ribbon_points, stats.rejected), (1, 0, 0), "{path}");
        assert_eq!(stats.after_resolve, settings.temporal, "{path}");
        assert_eq!(stats.bytes, 64, "{path}");
    }
}

/// Debug draw's pass follows post, so its lines stay on top of effects on both paths, even a tested
/// line behind one, since effects write no depth.
#[test]
#[ignore = "needs a GPU"]
fn debug_lines_stay_on_top_of_effects() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let at = Vec3::new(0.0, 0.0, -4.0);
        let frame = harness.render(|packet| {
            packet.vfx.sprites.push(dot(at, 3.0, Vec3::splat(0.5)));
            let behind = Vec3::new(0.0, 0.0, -6.0);
            packet.debug.lines.push(DebugLine {
                from: behind - Vec3::X * 2.0,
                to: behind + Vec3::X * 2.0,
                color: Color::rgb(255, 40, 200),
                depth: Depth::Tested,
            });
            packet.debug.stats.lines = 1;
        });
        // The effect's pixels around it, and the line across their middle: at this small target its
        // antialiased width leaves some of what's behind, far less than an effect drawn over it would.
        let (x, y) = pixel_of(at);
        assert!(frame.linear((x, y + 12)).y > 0.5, "{path}: {}", frame.linear((x, y + 12)));
        let line = (y - 2..=y + 2).map(|row| frame.linear((x, row))).min_by(|a, b| a.y.total_cmp(&b.y));
        let line = line.expect("rows");
        assert!(line.x > 0.8 && line.y < 0.4, "{path}: {line}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn a_streak_lies_along_its_axis_and_its_length() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let streak = VfxSprite {
            axis: Vec3::X,
            half_width: 0.1,
            half_length: 1.0,
            shape: VfxShape::Streak,
            color: Vec3::ONE * 0.6,
            ..dot(Vec3::new(0.0, 0.0, -2.0), 0.0, Vec3::ZERO)
        };
        let frame = harness.render(|packet| packet.vfx.sprites.push(streak));
        // Toward its head, +X, it's bright, and toward its tail fainter; across its axis, the sky.
        let at = |x: f32, y: f32| frame.linear(pixel_of(Vec3::new(x, y, -2.0)));
        assert!(at(0.5, 0.0).x > GREY + 0.4, "{path}: {}", at(0.5, 0.0));
        assert!(at(-0.6, 0.0).x < at(0.5, 0.0).x - 0.1, "{path}: {}", at(-0.6, 0.0));
        assert_near(at(0.5, 0.3), Vec3::splat(GREY), path);
    }
}

/// Fog over a particle at distance: it fades into the fog as the model says, as light keeping none
/// of the fog's own, and as a surface taking all of it.
#[test]
#[ignore = "needs a GPU"]
fn the_fog_covers_a_particle_at_distance() {
    let fog = Fog {
        near: FogLayer { color: Vec3::new(0.6, 0.5, 0.4), density: 0.04, start: 2.0 },
        far: FogLayer::default(),
        height: HeightFog::default(),
        sun_inscatter: Vec3::ZERO,
        characters: 1.0,
    };
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let distance = 30.0;
        let at = Vec3::new(0.0, 0.0, -distance);
        let horizon = Vec3::splat(GREY);
        let ray = fog.ray(Vec3::ZERO, Vec3::NEG_Z, distance, 1.0, horizon, Vec3::NEG_Z);
        assert!((0.2..0.8).contains(&ray.transmittance), "the test's fog: {}", ray.transmittance);
        let color = Vec3::new(0.7, 0.4, 0.2);
        let sky = |harness: &mut Harness| harness.render(|packet| packet.fog = fog).middle();
        let behind = sky(&mut harness);

        // Light: what's behind, plus the color through the fog.
        let light = dot(at, 6.0, color);
        let frame = harness.render(|packet| {
            packet.fog = fog;
            packet.vfx.sprites.push(light);
        });
        assert_near(frame.middle(), behind + color * ray.transmittance, path);

        // A surface: it covers what's behind, and shows as any surface at that distance does.
        let surface = VfxSprite { additive: 0.0, ..light };
        let frame = harness.render(|packet| {
            packet.fog = fog;
            packet.vfx.sprites.push(surface);
        });
        let fogged = color * ray.transmittance + (ray.light + ray.sun) * (1.0 - ray.transmittance);
        assert_near(frame.middle(), fogged, path);

        // Without fog, the color as it is, at any distance.
        let frame = harness.render(|packet| packet.vfx.sprites.push(surface));
        assert_near(frame.middle(), color, path);
    }
}

/// An emissive color is relative to the display: the same at any exposure. A lit one follows the
/// hour's light.
#[test]
#[ignore = "needs a GPU"]
fn emissive_color_ignores_the_exposure_and_lit_color_takes_the_light() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let at = Vec3::new(0.0, 0.0, -4.0);
        let surface = VfxSprite { additive: 0.0, ..dot(at, 3.0, Vec3::new(0.6, 0.3, 0.1)) };
        let mut shown = Vec::new();
        for exposure in [-1.0, 0.0, 1.5] {
            let frame = harness.render(|packet| {
                packet.post.exposure = exposure;
                packet.vfx.sprites.push(surface);
            });
            shown.push(frame.middle());
        }
        assert_near(shown[0], surface.color, path);
        assert_near(shown[1], surface.color, path);
        assert_near(shown[2], surface.color, path);

        let dust = VfxSprite { light: VfxLight::Lit, ..surface };
        let lit = |harness: &mut Harness, ambient: f32| {
            harness
                .render(|packet| {
                    packet.ambient.sky = Vec3::ONE;
                    packet.ambient.ground = Vec3::ONE;
                    packet.ambient.intensity = ambient;
                    packet.vfx.sprites.push(dust);
                })
                .middle()
        };
        assert_near(lit(&mut harness, 0.0), Vec3::ZERO, path);
        assert_near(lit(&mut harness, 0.5), dust.color * 0.5, path);
    }
}

#[test]
#[ignore = "needs a GPU"]
fn additive_adds_to_whats_behind_and_zero_covers_it() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let color = Vec3::new(0.3, 0.1, 0.05);
        let sprite = dot(Vec3::new(0.0, 0.0, -4.0), 3.0, color);
        let shown = |harness: &mut Harness, additive: f32| {
            let sprite = VfxSprite { additive, ..sprite };
            harness.render(|packet| packet.vfx.sprites.push(sprite)).middle()
        };
        assert_near(shown(&mut harness, 1.0), Vec3::splat(GREY) + color, path);
        assert_near(shown(&mut harness, 0.0), color, path);
        // Half of what's behind stays, and all of the light adds.
        assert_near(shown(&mut harness, 0.5), Vec3::splat(GREY) * 0.5 + color, path);
    }
}

/// Covering quads draw back to front, whatever order the frame lists them.
#[test]
#[ignore = "needs a GPU"]
fn covering_sprites_draw_back_to_front() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let near =
            VfxSprite { additive: 0.0, ..dot(Vec3::new(0.0, 0.0, -2.0), 1.5, Vec3::new(0.7, 0.1, 0.1)) };
        let far =
            VfxSprite { additive: 0.0, ..dot(Vec3::new(0.0, 0.0, -4.0), 3.0, Vec3::new(0.1, 0.1, 0.7)) };
        for list in [[near, far], [far, near]] {
            let frame = harness.render(|packet| packet.vfx.sprites.extend(list));
            assert_near(frame.middle(), near.color, path);
        }
    }
}

/// The frame's sprites over a black sky, seen from `camera`.
fn over_black(harness: &mut Harness, camera: CameraData, sprites: &[VfxSprite]) -> Frame {
    harness.render(|packet| {
        packet.views[0] = ViewData::new(camera);
        packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        packet.vfx.sprites.extend_from_slice(sprites);
    })
}

/// A half-covering quad of `color`: where it covers, what's behind it counts for less.
fn covering(position: Vec3, half: f32, color: Vec3) -> VfxSprite {
    VfxSprite { additive: 0.0, alpha: 0.65, ..dot(position, half, color) }
}

/// Quads facing the camera lie in parallel planes, so which covers which is their depth along the
/// view, whichever is farther from the eye: off to the side, a near quad's middle is farther away
/// than a far one's straight ahead (review REN-001). Red in front of blue, on black: what stands in
/// front keeps all its light, whatever order the frame lists them and however the camera has turned.
#[test]
#[ignore = "needs a GPU"]
fn a_near_sprite_off_to_the_side_covers_a_far_one_straight_ahead() {
    let (red, blue) = (Vec3::new(0.8, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.8));
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        for (yaw, height) in [(0.0, 0.0), (0.9, 1.5), (2.4, -2.0), (-1.3, 3.0)] {
            let eye = Mat4::from_rotation_y(yaw).transform_point3(Vec3::new(0.0, height, 6.0));
            let camera =
                CameraData::look_at(eye, Vec3::ZERO, Vec3::Y, std::f32::consts::FRAC_PI_2, 1.0, 0.05);
            let view = camera.view;
            let right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
            let forward = -Vec3::new(view.x_axis.z, view.y_axis.z, view.z_axis.z);
            let near = covering(eye + forward * 2.0 + right * 1.4, 2.0, red);
            let far = covering(eye + forward * 2.3, 1.8, blue);
            let what = format!("{path}, turned {yaw} from {height} up");
            assert!(near.position.distance(eye) > far.position.distance(eye), "{what}: as the test means");
            let at = (84, 64);
            let near_alone = over_black(&mut harness, camera, &[near]).linear(at).x;
            let far_alone = over_black(&mut harness, camera, &[far]).linear(at).z;
            assert!(near_alone > 0.2 && far_alone > 0.2, "{what}: {near_alone} and {far_alone} reach it");
            for list in [[near, far], [far, near]] {
                let both = over_black(&mut harness, camera, &list).linear(at);
                assert!((both.x - near_alone).abs() < TOLERANCE, "{what}: red {} of {near_alone}", both.x);
                assert!(both.z < far_alone - 0.05, "{what}: blue {} of {far_alone}, covered", both.z);
            }
        }
    }
}

/// Quads at one depth keep the frame's order, the later over the earlier; and quads of light add,
/// whichever draws first.
#[test]
#[ignore = "needs a GPU"]
fn sprites_at_one_depth_keep_the_frames_order_and_light_adds_either_way() {
    let view = camera();
    let (red, blue) = (Vec3::new(0.8, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.8));
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let left = covering(Vec3::new(-0.2, 0.0, -2.0), 1.5, red);
        let right = covering(Vec3::new(0.2, 0.0, -2.0), 1.5, blue);
        let middle = (SIDE / 2, SIDE / 2);
        let left_alone = over_black(&mut harness, view, &[left]).linear(middle).x;
        let right_alone = over_black(&mut harness, view, &[right]).linear(middle).z;
        let right_last = over_black(&mut harness, view, &[left, right]).linear(middle);
        assert!((right_last.z - right_alone).abs() < TOLERANCE, "{path}: {right_last} of {right_alone}");
        assert!(right_last.x < left_alone - 0.05, "{path}: {right_last} covers {left_alone}");
        let left_last = over_black(&mut harness, view, &[right, left]).linear(middle);
        assert!((left_last.x - left_alone).abs() < TOLERANCE, "{path}: {left_last} of {left_alone}");
        assert!(left_last.z < right_alone - 0.05, "{path}: {left_last} covers {right_alone}");

        let near = dot(Vec3::new(1.4, 0.0, -2.0), 2.0, red * 0.6);
        let far = dot(Vec3::new(0.0, 0.0, -2.3), 1.8, blue * 0.6);
        let at = (84, 64);
        let summed = over_black(&mut harness, view, &[near]).linear(at)
            + over_black(&mut harness, view, &[far]).linear(at);
        for list in [[near, far], [far, near]] {
            assert_near(over_black(&mut harness, view, &list).linear(at), summed, path);
        }
    }
}

/// Depth on both paths: behind a wall nothing shows; in front, everything does.
#[test]
#[ignore = "needs a GPU"]
fn walls_hide_what_is_behind_them() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let wall_color = Vec3::new(0.05, 0.1, 0.05);
        harness.add_wall(-5.0, wall_color);
        let bright = Vec3::new(0.6, 0.3, 0.3);
        // Temporal AA's scene prepass is one frame's input to the next's depth test: a frame first.
        harness.render(|_| {});
        let behind = harness.render(|packet| {
            packet.vfx.sprites.push(dot(Vec3::new(0.0, 0.0, -7.0), 3.0, bright));
        });
        assert_near(behind.middle(), wall_color, path);
        let front = harness.render(|packet| {
            packet.vfx.sprites.push(dot(Vec3::new(0.0, 0.0, -3.0), 3.0, bright));
        });
        assert_near(front.middle(), wall_color + bright, path);
    }
}

/// Over temporal AA's frame depth is read by hand: at a smaller render scale too, where the scene's
/// texels aren't the output's.
#[test]
#[ignore = "needs a GPU"]
fn walls_hide_what_is_behind_them_when_the_scene_is_upsampled() {
    let mut harness = Harness::new(temporal_settings());
    harness.renderer.set_render_scale(0.5);
    let wall_color = Vec3::new(0.05, 0.1, 0.05);
    harness.add_wall(-5.0, wall_color);
    let bright = Vec3::new(0.6, 0.3, 0.3);
    for _ in 0..4 {
        harness.render(|_| {});
    }
    let behind = harness.render(|packet| {
        packet.vfx.sprites.push(dot(Vec3::new(0.0, 0.0, -7.0), 3.0, bright));
    });
    assert_near(behind.middle(), wall_color, "behind");
    let front = harness.render(|packet| {
        packet.vfx.sprites.push(dot(Vec3::new(0.0, 0.0, -3.0), 3.0, bright));
    });
    assert_near(front.middle(), wall_color + bright, "in front");
}

/// Soft particles, where the frame has depth to read: a quad just before a wall barely shows, and
/// fades in as it comes away, over its soft distance.
#[test]
#[ignore = "needs a GPU"]
fn over_temporal_aa_a_quad_fades_into_the_surface_behind_it() {
    let mut harness = Harness::new(temporal_settings());
    let wall_color = Vec3::new(0.05, 0.1, 0.05);
    harness.add_wall(-5.0, wall_color);
    harness.render(|_| {});
    let bright = Vec3::new(0.8, 0.4, 0.2);
    let shown = |harness: &mut Harness, gap: f32| {
        let sprite = VfxSprite { soft: 1.0, ..dot(Vec3::new(0.0, 0.0, -5.0 + gap), 3.0, bright) };
        harness.render(|packet| packet.vfx.sprites.push(sprite)).middle() - wall_color
    };
    assert_near(shown(&mut harness, 0.01), bright * 0.01, "at the wall");
    assert_near(shown(&mut harness, 0.5), bright * 0.5, "half way");
    assert_near(shown(&mut harness, 2.0), bright, "clear of it");
}

/// The strip's cross-section: nothing past its rails, a bright crisp band where its core is, a
/// glow falling away from it, and along it no seams.
#[test]
#[ignore = "needs a GPU"]
fn a_ribbon_has_a_crisp_core_and_a_soft_glow() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let glow = Vec3::new(0.0, 0.05, 0.25);
        let core = Vec3::new(0.5, 0.5, 0.4);
        let z = -2.0;
        let frame = harness.render(|packet| packet.vfx.ribbon_points.extend(strip(z, glow, core)));
        let stats = harness.renderer.stats().vfx;
        assert_eq!((stats.sprites, stats.ribbon_points), (0, 2), "{path}");
        let across = |t: f32| frame.linear(pixel_of(Vec3::new(0.0, -0.5 + t, z)));
        // Past the rails the sky, and at the core's middle its color with nearly all of the glow.
        assert_near(frame.linear(pixel_of(Vec3::new(0.0, 0.6, z))), Vec3::splat(GREY), path);
        assert_near(frame.linear(pixel_of(Vec3::new(0.0, -0.6, z))), Vec3::splat(GREY), path);
        assert!((across(0.75).x - (GREY + core.x)).abs() < TOLERANCE, "{path}: {}", across(0.75));
        // A glow that falls away from the core: its reach is half the strip's width, toward the
        // base rail, and whatever's left of the strip, toward the tip.
        let height = |t: f32| (across(t) - Vec3::splat(GREY)).z;
        assert!(
            height(0.6) > height(0.5) && height(0.5) > height(0.4),
            "{path}: {} {} {}",
            height(0.6),
            height(0.5),
            height(0.4)
        );
        assert!(height(0.4) > 0.0 && height(0.2) < 0.005, "{path}: {} {}", height(0.4), height(0.2));
        assert!(height(0.97) < height(0.85), "{path}");
        // Crisp: 0.05 of the 1 m strip at 2 m is about 3 pixels, from the core's full light to the
        // glow alone within a couple of pixels of its edge.
        let core_edge = 0.05 + 2.0 * 2.0 * 2.0 / SIDE as f32;
        let beside = (across(0.75 - core_edge) - Vec3::splat(GREY)).x;
        assert!(beside < 0.1, "{path}: {beside} beside the core");
        // Along the strip it's the same, where the two triangles of its quad meet.
        for x in [-0.8, -0.4, 0.0, 0.4, 0.8] {
            let along = frame.linear(pixel_of(Vec3::new(x, 0.25, z)));
            assert_near(along, across(0.75), path);
        }
    }
}

/// A list's stretches join only within a ribbon: two ribbons draw nothing between them.
#[test]
#[ignore = "needs a GPU"]
fn ribbons_dont_join_each_other() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let glow = Vec3::new(0.05, 0.1, 0.4);
        let core = Vec3::new(0.7, 0.7, 0.7);
        let z = -2.0;
        let shift = |mut points: [VfxRibbonPoint; 2], y: f32| {
            for point in &mut points {
                point.base.y += y;
                point.tip.y += y;
            }
            points
        };
        let frame = harness.render(|packet| {
            packet.vfx.ribbon_points.extend(shift(strip(z, glow, core), -0.9));
            packet.vfx.ribbon_points.extend(shift(strip(z, glow, core), 0.9));
        });
        assert_near(frame.linear(pixel_of(Vec3::new(0.0, 0.0, z))), Vec3::splat(GREY), path);
        assert!(frame.linear(pixel_of(Vec3::new(0.0, 1.15, z))).x > GREY + 0.3, "{path}");
        assert!(frame.linear(pixel_of(Vec3::new(0.0, -0.65, z))).x > GREY + 0.3, "{path}");
    }
}

/// A point that can't draw still ends what it ended: its ribbon's last, left out, doesn't leave the
/// ribbon open to the next one's first, so nothing draws between them, and the next ribbon does
/// (review REN-002).
#[test]
#[ignore = "needs a GPU"]
fn a_point_left_out_doesnt_join_two_ribbons() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let glow = Vec3::new(0.05, 0.1, 0.4);
        let core = Vec3::new(0.7, 0.7, 0.7);
        let z = -2.0;
        let at = |x: f32, ends| {
            let mut point = strip(z, glow, core)[0];
            point.base.x = x;
            point.tip.x = x;
            point.ends = ends;
            point
        };
        let mut bad = at(0.0, true);
        bad.tip = Vec3::NAN;
        let frame = harness.render(|packet| {
            packet.vfx.ribbon_points.extend([
                at(-1.5, false),
                at(-1.0, false),
                bad,
                at(1.0, false),
                at(1.5, true),
            ]);
        });
        let stats = harness.renderer.stats().vfx;
        assert_eq!((stats.ribbon_points, stats.rejected), (4, 1), "{path}");
        let (y, lit) = (0.25, GREY + 0.3);
        assert_near(frame.linear(pixel_of(Vec3::new(0.0, y, z))), Vec3::splat(GREY), path);
        assert!(frame.linear(pixel_of(Vec3::new(-1.25, y, z))).x > lit, "{path}: what came before");
        assert!(frame.linear(pixel_of(Vec3::new(1.25, y, z))).x > lit, "{path}: what comes after");
    }
}

/// A moving spark over a still world has that world's zero velocity, and temporal AA would hold its
/// first frame to a thirty-third of its light: over the resolved frame it's whole on every frame.
#[test]
#[ignore = "needs a GPU"]
fn a_moving_spark_keeps_its_whole_light_under_temporal_aa() {
    let mut harness = Harness::new(temporal_settings());
    let color = Vec3::new(0.5, 0.3, 0.2);
    for _ in 0..16 {
        harness.render(|_| {});
    }
    for step in 0..8 {
        let at = Vec3::new(-2.0 + step as f32 * 0.6, 0.0, -4.0);
        let frame = harness.render(|packet| packet.vfx.sprites.push(dot(at, 0.5, color)));
        assert_near(frame.linear(pixel_of(at)), Vec3::splat(GREY) + color, "its light");
    }
}

/// The history never holds an effect, so none can ghost: once it's gone the frame is byte for byte
/// what a renderer that never drew one shows, and the same without temporal AA.
#[test]
#[ignore = "needs a GPU"]
fn effects_leave_nothing_behind_them() {
    for (path, settings) in paths() {
        let wall_color = Vec3::new(0.05, 0.1, 0.05);
        let mut clean = Harness::new(settings);
        let mut played = Harness::new(settings);
        clean.add_wall(-5.0, wall_color);
        played.add_wall(-5.0, wall_color);
        let sprite = dot(Vec3::new(0.0, 0.0, -3.0), 1.5, Vec3::ONE);
        for _ in 0..6 {
            clean.render(|_| {});
            played.render(|packet| packet.vfx.sprites.push(sprite));
        }
        let (a, b) = (clean.render(|_| {}), played.render(|_| {}));
        assert_eq!(a.pixels, b.pixels, "{path}: the frame after the effects ended");
        let stats = played.renderer.stats().vfx;
        assert_eq!((stats.sprites, stats.bytes), (0, 0), "{path}");
    }
}

/// A frame with no effects pays nothing: no pipelines, buffers or textures, and no pass. A frame with
/// them builds what it draws through, once.
#[test]
#[ignore = "needs a GPU"]
fn a_frame_with_no_effects_builds_nothing() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        // Whatever the first frames build (the transient textures among them).
        for _ in 0..2 {
            harness.render(|_| {});
        }
        let (pipelines, resources) = (harness.render_pipelines(), harness.resources());
        let passes = harness.graph.stats().passes;
        for _ in 0..4 {
            harness.render(|_| {});
        }
        assert_eq!(harness.render_pipelines(), pipelines, "{path}: no pipelines");
        assert_eq!(harness.resources(), resources, "{path}: no buffers or textures");
        assert_eq!(harness.graph.stats().passes, passes, "{path}: no pass");
        assert_eq!(harness.renderer.stats().vfx.pipelines, 0, "{path}");

        // The first effect builds its pipeline: post doesn't read the character mask here, so the
        // color's alone. Effects after it build nothing.
        let sprite = dot(Vec3::new(0.0, 0.0, -4.0), 0.5, Vec3::ONE);
        harness.render(|packet| packet.vfx.sprites.push(sprite));
        assert_eq!(harness.render_pipelines(), pipelines + 1, "{path}: the color's");
        assert_eq!(harness.renderer.stats().vfx.pipelines, 1, "{path}");
        let expected_passes = passes + u32::from(settings.temporal);
        assert_eq!(harness.graph.stats().passes, expected_passes, "{path}: its pass, if it has one");
        harness.render(|packet| packet.vfx.sprites.push(sprite));
        assert_eq!(harness.render_pipelines(), pipelines + 1, "{path}: none again");
        // Back to none: its pass goes with it, and it keeps what it built.
        harness.render(|_| {});
        assert_eq!(harness.graph.stats().passes, passes, "{path}: no pass");
        assert_eq!(harness.render_pipelines(), pipelines + 1, "{path}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn prewarming_builds_ahead_so_the_first_effect_builds_nothing() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        harness.render(|_| {});
        let pipelines = harness.render_pipelines();
        harness.renderer.prewarm_vfx();
        assert_eq!(harness.render_pipelines(), pipelines + 2, "{path}: the color's and the mask's");
        harness.renderer.prewarm_vfx();
        assert_eq!(harness.render_pipelines(), pipelines + 2, "{path}: only once");
        assert_eq!(harness.renderer.stats().vfx.pipelines, 2, "{path}");
        let sprite = dot(Vec3::new(0.0, 0.0, -4.0), 0.5, Vec3::ONE);
        harness.render(|packet| packet.vfx.sprites.push(sprite));
        assert_eq!(harness.render_pipelines(), pipelines + 2, "{path}: none at the first effect");
    }
}

/// What it says of values that aren't finite: where it stands drops it, and its color is made safe.
#[test]
#[ignore = "needs a GPU"]
fn bad_values_draw_nothing_and_count() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let bad = dot(Vec3::NAN, 1.0, Vec3::ONE);
        let poison = dot(Vec3::new(0.0, 0.0, -4.0), 1.0, Vec3::new(f32::NAN, 0.3, 0.0));
        let mut broken = strip(-2.0, Vec3::ONE, Vec3::ONE);
        broken[0].base = Vec3::INFINITY;
        let frame = harness.render(|packet| {
            packet.vfx.sprites.extend([bad, poison]);
            packet.vfx.ribbon_points.extend(broken);
        });
        let stats = harness.renderer.stats().vfx;
        assert_eq!(stats.rejected, 2, "{path}");
        assert_eq!((stats.sprites, stats.ribbon_points), (1, 1), "{path}");
        let middle = frame.middle();
        assert!(middle.is_finite() && (middle.x - GREY).abs() < TOLERANCE, "{path}: {middle}");
    }
}

/// A character with an anime character material, flat, in front of the sky: post's character mask
/// (alpha 0) covers it, so its bloom takes `character_bloom`.
fn character_wall(harness: &mut Harness, z: f32) -> InstanceId {
    let material = harness.material(&AnimeCharacterMaterial {
        base_color: [0.1, 0.1, 0.1, 1.0],
        light_split: LightSplit { threshold: -1.0, softness: 0.02 },
        regions: vec![CharacterRegion { shade_color: [1.0; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        outline: Outline { width: 0.0, ..Outline::default() },
        ..AnimeCharacterMaterial::default()
    });
    let mesh = harness.mesh(&wall(z, 20.0), material);
    harness.add(mesh)
}

/// A spark over a character blooms as it does over the world: scene color's alpha, the character
/// mask, rises by what it covers. Bloom's character strength of 0 shows it: with the mask left at 0
/// nothing around the spark would glow at all.
#[test]
#[ignore = "needs a GPU"]
fn a_spark_over_a_character_blooms_as_it_does_over_the_world() {
    let bloom = BloomParams { intensity: 1.0, threshold: 0.5, ..BloomParams::default() };
    let post = PostParams {
        bloom,
        character_bloom: CharacterBloom { threshold: 10.0, strength: 0.0 },
        ..passthrough()
    };
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let id = character_wall(&mut harness, -5.0);
        let character = CharacterInstance {
            id,
            lighting: CharacterLighting::unstyled(DirectionalLight {
                direction: Vec3::NEG_Y,
                color: Vec3::ONE,
                intensity: 1.0,
            }),
            face: CharacterFace::default(),
            body: CharacterBody::default(),
            opacity: 1.0,
        };
        let spark = dot(Vec3::new(0.0, 0.0, -3.0), 0.35, Vec3::splat(40.0));
        // Beside the spark, clear of its quad: bloom alone reaches it.
        let beside = pixel_of(Vec3::new(0.9, 0.0, -3.0));
        let glow = |harness: &mut Harness, with_spark: bool| {
            let mut last = Vec3::ZERO;
            for _ in 0..2 {
                let frame = harness.render(|packet| {
                    packet.post = post;
                    packet.characters.push(character);
                    if with_spark {
                        packet.vfx.sprites.push(spark);
                    }
                });
                last = frame.linear(beside);
            }
            last
        };
        let (none, with_spark) = (glow(&mut harness, false), glow(&mut harness, true));
        assert!(
            with_spark.x > none.x + 0.05,
            "{path}: the glow beside it: {with_spark} against {none} without it"
        );
        assert_eq!(harness.renderer.stats().vfx.pipelines, 2, "{path}: the color's and the mask's");
    }
}

/// Hot reload: an edit reaches what's built and builds nothing before the first effect.
#[test]
#[ignore = "needs a GPU"]
fn an_edit_rebuilds_what_it_has_built() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        harness.render(|_| {});
        let sprite = dot(Vec3::new(0.0, 0.0, -4.0), 3.0, Vec3::new(0.6, 0.3, 0.1));
        let edit = |sources: &ShaderSources, text: &str| {
            let mut sources = sources.clone();
            let edited = format!("{}\n// {text}\n", sources.get(ShaderFile::Vfx));
            sources.set(ShaderFile::Vfx, edited);
            sources
        };
        // Nothing built: the program builds, and no pipeline.
        let sources = edit(harness.renderer.shader_sources(), "first");
        let update = harness.renderer.set_shader_sources(&sources).expect("a valid edit");
        assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 0 }, "{path}");

        let before = harness.render(|packet| packet.vfx.sprites.push(sprite));
        assert_eq!(harness.renderer.stats().vfx.pipelines, 1, "{path}: the first effect built from it");

        // Now it has one to rebuild, whose pixels a comment doesn't change.
        let sources = edit(harness.renderer.shader_sources(), "second");
        let update = harness.renderer.set_shader_sources(&sources).expect("a valid edit");
        assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 1 }, "{path}");
        let after = harness.render(|packet| packet.vfx.sprites.push(sprite));
        assert_near(after.middle(), before.middle(), path);

        // A broken edit keeps what it has.
        let mut broken = harness.renderer.shader_sources().clone();
        broken.set(ShaderFile::Vfx, "not wgsl");
        assert!(harness.renderer.set_shader_sources(&broken).is_err(), "{path}");
        let kept = harness.render(|packet| packet.vfx.sprites.push(sprite));
        assert_near(kept.middle(), before.middle(), path);
    }
}

/// A list too long for a frame's first buffers grows them, and draws whole.
#[test]
#[ignore = "needs a GPU"]
fn buffers_grow_with_the_list() {
    for (path, settings) in paths() {
        let mut harness = Harness::new(settings);
        let sprite = dot(Vec3::new(0.0, 0.0, -4.0), 3.0, Vec3::splat(0.001));
        let count = 600;
        let frame = harness.render(|packet| packet.vfx.sprites.extend(std::iter::repeat_n(sprite, count)));
        assert_eq!(harness.renderer.stats().vfx.sprites, count as u32, "{path}");
        // Each a half float's few steps over what's behind, so not exactly their sum.
        let middle = frame.middle().x;
        assert!((middle - (GREY + 0.6)).abs() < 0.05, "{path}: {middle}");
    }
}
