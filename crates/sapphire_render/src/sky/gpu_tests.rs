//! Sky tests that render and read back. They need a GPU: run them with
//! `cargo test -p sapphire_render --tests -- --ignored`.
//!
//! Frames go through post with a linear tone curve and no bloom, grade or vignette, so the
//! target holds the sky's own colors, sRGB-encoded, to within the LUT's precision.

use std::f32::consts::{PI, TAU};

use glam::{Quat, Vec3, Vec4};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::material::{CloudLayer, Material, MaterialDocument, SkyMaterial};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::{CUBE_FACES, SkySettings, cube_face_axes};
use crate::graph::{FrameGraph, GraphExecutor};
use crate::texture::GpuTexture;
use crate::{
    BloomParams, CameraData, CloudLight, ColorGrade, DirectionalLight, FramePacket, FramePacketRing,
    MaterialHandle, PostParams, RenderSettings, Renderer, SkyDisc, SkyGradient, ToneCurve, ViewData,
    Vignette,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The CPU twin of `sky_color()` in frame.wgsl.
fn sky_color(sky: &SkyGradient, dir: Vec3) -> Vec3 {
    if dir.y >= 0.0 {
        sky.horizon.lerp(sky.zenith, dir.y.clamp(0.0, 1.0).sqrt())
    } else {
        sky.horizon.lerp(sky.ground, (-dir.y * 5.0).clamp(0.0, 1.0))
    }
}

fn srgb_to_linear(value: u8) -> f32 {
    let c = f32::from(value) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

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
    }
}

const BLACK: SkyGradient = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };

/// A renderer drawing sky-only frames into a readable sRGB target.
struct Harness {
    gpu: GpuContext,
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    size: (u32, u32),
    camera: CameraData,
    /// Copied into every packet.
    sky: SkyGradient,
}

impl Harness {
    fn new(settings: RenderSettings, size: (u32, u32)) -> Self {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let renderer = Renderer::new(&gpu, FORMAT, settings).expect("renderer");
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let camera = Self::looking(Vec3::NEG_Z, Vec3::Y, 50.0, size);
        Self {
            gpu,
            renderer,
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            size,
            camera,
            sky: BLACK,
        }
    }

    /// A camera at the origin looking along `forward`.
    fn looking(forward: Vec3, up: Vec3, fov_degrees: f32, (width, height): (u32, u32)) -> CameraData {
        CameraData::look_at(
            Vec3::ZERO,
            forward,
            up,
            fov_degrees.to_radians(),
            width as f32 / height as f32,
            0.05,
        )
    }

    fn look(&mut self, forward: Vec3, fov_degrees: f32) {
        let up = if forward.normalize().y.abs() > 0.99 { Vec3::NEG_Z } else { Vec3::Y };
        self.camera = Self::looking(forward, up, fov_degrees, self.size);
    }

    /// Renders one frame whose packet `fill` sets up, and reads the target back as linear RGB.
    fn render(&mut self, fill: impl FnOnce(&mut FramePacket)) -> Vec<Vec3> {
        let packet = self.packets.begin();
        packet.views.push(ViewData { camera: self.camera });
        packet.sky = self.sky;
        packet.post = passthrough();
        fill(packet);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none(), "no validation errors");
        let texels = read_texture(&self.gpu, &self.target, 0, 4);
        texels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|t| Vec3::new(srgb_to_linear(t[0]), srgb_to_linear(t[1]), srgb_to_linear(t[2])))
            .collect()
    }

    /// The direction through pixel `(x, y)`'s center, as the sky shader unprojects it.
    fn direction(&self, x: u32, y: u32) -> Vec3 {
        let ndc = Vec4::new(
            (x as f32 + 0.5) / self.size.0 as f32 * 2.0 - 1.0,
            1.0 - (y as f32 + 0.5) / self.size.1 as f32 * 2.0,
            1.0,
            1.0,
        );
        let near = (self.camera.proj * self.camera.view).inverse() * ndc;
        (near.truncate() / near.w - self.camera.position).normalize()
    }

    /// Where direction `dir` lands on the target, in pixels from the top left.
    fn project(&self, dir: Vec3) -> (f32, f32) {
        let clip = self.camera.proj * self.camera.view * dir.extend(0.0);
        let ndc = clip.truncate() / clip.w;
        ((ndc.x + 1.0) * 0.5 * self.size.0 as f32, (1.0 - ndc.y) * 0.5 * self.size.1 as f32)
    }

    /// Uploads a linear `Rgba16Float` texture `width` × `height`, each texel `texel(u, v)` at its
    /// center.
    fn texture(&self, (width, height): (u32, u32), texel: impl Fn(f32, f32) -> [f32; 4]) -> GpuTexture {
        let mut bytes = Vec::with_capacity((width * height * 8) as usize);
        for y in 0..height {
            for x in 0..width {
                let value = texel((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
                for channel in value {
                    bytes.extend_from_slice(&f16::from_f32(channel).to_le_bytes());
                }
            }
        }
        let desc = TextureDesc {
            format: TextureFormat::Rgba16Float,
            width,
            height,
            mip_level_count: 1,
            swizzle: Swizzle::IDENTITY,
        };
        GpuTexture::upload(&self.gpu, &Texture { desc, levels: vec![&bytes] }, "sky test").expect("texture")
    }

    /// Cooks and uploads `material`, its cloud and cirrus slots bound to `clouds` and `cirrus`.
    fn sky_material(
        &mut self,
        material: &SkyMaterial,
        clouds: Option<&GpuTexture>,
        cirrus: Option<&GpuTexture>,
    ) -> MaterialHandle {
        let swizzles = [Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()];
        let bytes = Material::from_document(material, &swizzles).expect("valid").to_cooked().expect("cook");
        let lookup = |asked: AssetId| {
            if Some(asked) == material.clouds.map {
                clouds
            } else if Some(asked) == material.cirrus.map {
                cirrus
            } else {
                None
            }
        };
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }
}

/// Copies layer `layer` of `texture` (`bytes` per texel) back.
fn read_texture(gpu: &GpuContext, texture: &wgpu::Texture, layer: u32, bytes: u32) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * bytes).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue().submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let mapped = buffer.get_mapped_range(..).expect("mapped");
    mapped
        .chunks_exact(padded as usize)
        .flat_map(|row| row[..(width * bytes) as usize].iter().copied())
        .collect()
}

/// A gradient with a different color at each stop, so any change to the mix shows.
fn tricolor() -> SkyGradient {
    SkyGradient {
        zenith: Vec3::new(0.05, 0.2, 0.7),
        horizon: Vec3::new(0.7, 0.6, 0.4),
        ground: Vec3::new(0.1, 0.3, 0.1),
    }
}

fn disc(color: Vec3, intensity: f32, glow: Vec3, glow_degrees: f32) -> SkyDisc {
    SkyDisc { color, intensity, glow, glow_radius: glow_degrees.to_radians() }
}

/// A body at `elevation` and `azimuth` degrees (azimuth from −Z toward +X), as the direction its
/// light travels.
fn toward(elevation: f32, azimuth: f32) -> Vec3 {
    let (elevation, azimuth) = (elevation.to_radians(), azimuth.to_radians());
    -Vec3::new(azimuth.sin() * elevation.cos(), elevation.sin(), -azimuth.cos() * elevation.cos())
}

#[test]
#[ignore = "needs a GPU"]
fn the_gradient_is_unchanged_by_layers_that_draw_nothing_here() {
    let mut harness = Harness::new(RenderSettings::default(), (160, 120));
    harness.sky = tricolor();
    harness.look(Vec3::new(0.0, 0.1, -1.0), 70.0);
    let plain = harness.render(|_| {});
    // Every layer set but none reaching the view: the sun and its glow behind the camera, the
    // moon below the horizon, no stars, and clouds with no maps to draw.
    let idle = harness.render(|packet| {
        packet.celestial.sun.direction = toward(20.0, 180.0);
        packet.celestial.moon.direction = toward(-20.0, 0.0);
        packet.sky_layers.sun = disc(Vec3::ONE, 30.0, Vec3::splat(0.5), 40.0);
        packet.sky_layers.moon = disc(Vec3::ONE, 2.0, Vec3::ZERO, 0.0);
        packet.sky_layers.clouds =
            CloudLight { lit: Vec3::ONE, coverage: 0.8, cirrus: 0.8, ..CloudLight::default() };
    });
    assert_eq!(plain, idle, "exactly the gradient");
    // And the gradient is `sky_color`'s, to within the grading LUT's precision.
    let sky = tricolor();
    for (index, color) in plain.iter().enumerate() {
        let (x, y) = (index as u32 % harness.size.0, index as u32 / harness.size.0);
        let expected = sky_color(&sky, harness.direction(x, y));
        let error = (color.to_array().map(linear_to_srgb), expected.to_array().map(linear_to_srgb));
        let worst = (0..3).map(|c| (error.0[c] - error.1[c]).abs()).fold(0.0, f32::max);
        assert!(worst < 2.5 / 255.0, "pixel ({x}, {y}): {color} vs {expected}");
    }
}

/// The centroid and pixel count of pixels brighter than `threshold` in red, among those
/// `filter` keeps.
fn bright_spot(
    harness: &Harness,
    image: &[Vec3],
    threshold: f32,
    filter: impl Fn(u32, u32) -> bool,
) -> (f32, f32, f32) {
    let (mut sum, mut weight) = ((0.0, 0.0), 0.0);
    for (index, color) in image.iter().enumerate() {
        let (x, y) = (index as u32 % harness.size.0, index as u32 / harness.size.0);
        if color.x > threshold && filter(x, y) {
            sum.0 += (x as f32 + 0.5) * color.x;
            sum.1 += (y as f32 + 0.5) * color.x;
            weight += color.x;
        }
    }
    (sum.0 / weight, sum.1 / weight, weight)
}

#[test]
#[ignore = "needs a GPU"]
fn sun_and_moon_discs_sit_where_celestial_puts_them() {
    let mut harness = Harness::new(RenderSettings::default(), (320, 320));
    harness.look(Vec3::new(0.0, 0.25, -1.0), 40.0);
    let (sun, moon) = (toward(18.0, 8.0), toward(12.0, -9.0));
    let image = harness.render(|packet| {
        packet.celestial.sun.direction = sun;
        packet.celestial.moon.direction = moon;
        packet.sky_layers.sun = disc(Vec3::new(1.0, 0.0, 0.0), 0.6, Vec3::ZERO, 0.0);
        packet.sky_layers.moon = disc(Vec3::new(1.0, 0.0, 0.0), 0.4, Vec3::ZERO, 0.0);
    });
    let middle = harness.size.0 / 2;
    let defaults = SkyMaterial::default();
    for (body, size, side, name) in
        [(sun, defaults.sun.size, true, "sun"), (moon, defaults.moon.disc.size, false, "moon")]
    {
        let (x, y, light) = bright_spot(&harness, &image, 0.02, |x, _| (x >= middle) == side);
        let (expected_x, expected_y) = harness.project(-body);
        assert!(
            (x - expected_x).abs() < 0.25 && (y - expected_y).abs() < 0.25,
            "{name} at ({x}, {y}), expected ({expected_x}, {expected_y})"
        );
        // Its area: π r², with r the disc's angular radius in pixels.
        let pixels_per_radian = harness.size.1 as f32 / (2.0 * (20f32.to_radians()).tan());
        let radius = (size * 0.5).to_radians() * pixels_per_radian;
        let brightness = if name == "sun" { 0.6 } else { 0.4 };
        let area = light / brightness;
        let expected = PI * radius * radius;
        assert!((area / expected - 1.0).abs() < 0.12, "{name}: {area} pixels, expected {expected}");
    }
    // Set below the horizon, a disc is gone.
    let set = harness.render(|packet| {
        packet.celestial.sun.direction = toward(-3.0, 8.0);
        packet.sky_layers.sun = disc(Vec3::ONE, 0.6, Vec3::ZERO, 0.0);
    });
    assert!(set.iter().all(|color| *color == Vec3::ZERO), "nothing under the horizon");
}

/// The CPU twin of `glow()` in sky.wgsl, for a body toward `body`: `light` at its center over
/// `radius` radians.
fn glow(dir: Vec3, body: Vec3, light: f32, radius: f32) -> f32 {
    let mut offset = dir - body;
    offset.y *= 2.0 + (1.0 - 2.0) * (body.y * 4.0).clamp(0.0, 1.0);
    let fall = 1.0 - (offset.length() * radius.recip()).clamp(0.0, 1.0);
    let halo = fall * fall * fall;
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    light * (halo + halo.powi(4)) * smooth(-0.08, 0.0, dir.y)
}

/// The sum of every pixel's light.
fn total(image: &[Vec3]) -> f32 {
    image.iter().map(|color| color.x + color.y + color.z).sum()
}

#[test]
#[ignore = "needs a GPU"]
fn stars_fade_with_the_hour_and_hold_still_under_sub_pixel_motion() {
    let size = (480, 360);
    let mut harness = Harness::new(RenderSettings::default(), size);
    let fov = 60.0;
    let forward = Vec3::new(0.3, 1.0, -0.6).normalize();
    harness.look(forward, fov);
    let stars = |brightness: f32| move |packet: &mut FramePacket| packet.sky_layers.stars = brightness;
    let full = harness.render(stars(1.0));
    let lit = full.iter().filter(|color| color.length() > 0.01).count();
    assert!(lit > 500, "a field of stars: {lit} pixels");
    // Dim enough that no star reaches the tone curve's shoulder, so light adds up linearly.
    let light = total(&harness.render(stars(0.5)));
    let quarter = total(&harness.render(stars(0.25)));
    assert!((quarter / light - 0.5).abs() < 0.03, "half as bright: {quarter} of {light}");
    assert!(harness.render(stars(0.0)).iter().all(|color| *color == Vec3::ZERO), "none by day");

    // Turning the camera by fractions of a pixel moves every star between pixels; each keeps its
    // light, so the frame's total holds, where point stars would flicker in and out.
    let pixel = (fov.to_radians() * 0.5).tan() * 2.0 / size.1 as f32;
    for step in 1..4 {
        let turn = Quat::from_rotation_y(step as f32 * 0.25 * pixel);
        harness.look(turn * forward, fov);
        let moved = total(&harness.render(stars(0.5)));
        assert!((moved / light - 1.0).abs() < 0.01, "{step} quarter pixels: {moved} vs {light}");
    }
}

/// A sky material with a card cloud map bound, repeated once around the horizon, and a cirrus
/// map, scrolled by `cirrus_scroll`.
fn cloud_material(cirrus_scroll: [f32; 2]) -> SkyMaterial {
    let id = |text| Some(AssetId::from_text(text));
    SkyMaterial {
        clouds: CloudLayer {
            map: id("00000000-0000-4000-8000-0000000000c1"),
            scale: 1.0,
            scroll: [0.0; 2],
            dissolve_softness: 0.0,
        },
        cirrus: CloudLayer {
            map: id("00000000-0000-4000-8000-0000000000c2"),
            scale: 1.0,
            scroll: cirrus_scroll,
            dissolve_softness: 0.0,
        },
        ..SkyMaterial::default()
    }
}

#[test]
#[ignore = "needs a GPU"]
fn card_clouds_dissolve_from_their_edges_with_coverage() {
    let mut harness = Harness::new(RenderSettings::default(), (200, 400));
    // The band's map, 256 × 32 once around: 45° of elevation from 2° under the horizon. Its
    // distance channel rises from 0 at the top to 1 at the bottom, so a cloud fills the band from
    // its bottom to where B meets the dissolve threshold.
    let clouds = harness.texture((256, 32), |_, v| [1.0, 0.0, v, 1.0]);
    let cirrus = harness.texture((4, 4), |_, _| [0.0; 4]);
    let material = cloud_material([0.0; 2]);
    let handle = harness.sky_material(&material, Some(&clouds), Some(&cirrus));
    harness.look(Vec3::new(0.0, 0.35, -1.0), 50.0);
    let band = TAU * 32.0 / 256.0;
    let mut top = |coverage: f32| {
        let image = harness.render(|packet| {
            packet.sky_material = Some(handle);
            packet.sky_layers.clouds = CloudLight { lit: Vec3::X, coverage, ..CloudLight::default() };
        });
        // The highest red pixel in the middle column, as an elevation.
        let column = harness.size.0 / 2;
        let row = (0..harness.size.1).find(|&y| image[(y * harness.size.0 + column) as usize].x > 0.3)?;
        Some(harness.direction(column, row).y.asin().to_degrees())
    };
    // Up the band, the map follows the direction's height, the sine of its elevation.
    let expected = |threshold: f32| (-0.035 + (1.0 - threshold) * band).asin().to_degrees();
    // Coverage 1 keeps the painted edge (B = 0.5); half dissolves to 0.75; none clears the sky.
    let full = top(1.0).expect("clouds at full coverage");
    assert!((full - expected(0.5)).abs() < 0.3, "full: top at {full}°, expected {}°", expected(0.5));
    let half = top(0.5).expect("clouds at half coverage");
    assert!((half - expected(0.75)).abs() < 0.3, "half: top at {half}°, expected {}°", expected(0.75));
    assert_eq!(top(0.0), None, "a clear sky");
    assert!(!harness.renderer.stats().sky.clouds, "no clouds drawn at zero coverage");
}

#[test]
#[ignore = "needs a GPU"]
fn cirrus_drifts_with_time_without_jumping_late_in_a_session() {
    let mut harness = Harness::new(RenderSettings::default(), (256, 256));
    // One small block of cirrus at u and v 0.0625–0.125, near the zenith wherever it drifts.
    let cirrus = harness.texture((64, 64), |u, v| {
        let inside = (0.0625..0.125).contains(&u) && (0.0625..0.125).contains(&v);
        if inside { [1.0, 0.0, 1.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] }
    });
    let clouds = harness.texture((4, 4), |_, _| [0.0; 4]);
    let speed = 0.004;
    let handle = harness.sky_material(&cloud_material([speed, 0.0]), Some(&clouds), Some(&cirrus));
    harness.look(Vec3::Y, 60.0);
    // Where the block shows, in the cirrus plane's coordinates before drift: its centroid in u,
    // each pixel weighted by its coverage and by the plane area it spans (pixels away from the
    // zenith span more of it).
    let plane = |harness: &Harness, x: u32, y: u32| {
        let dir = harness.direction(x, y);
        glam::Vec2::new(dir.x, dir.z) * (0.5 / (dir.y + 0.15))
    };
    let mut center = |seconds: f64, cirrus: f32| {
        let image = harness.render(|packet| {
            packet.time_seconds = seconds;
            packet.sky_material = Some(handle);
            packet.sky_layers.clouds = CloudLight { lit: Vec3::X, cirrus, ..CloudLight::default() };
        });
        let (mut sum, mut weight) = (0.0, 0.0);
        for (index, color) in image.iter().enumerate() {
            let (x, y) = (index as u32 % 256, index as u32 / 256);
            if color.x > 0.01 && x < 255 && y < 255 {
                let at = plane(&harness, x, y);
                let area = (plane(&harness, x + 1, y) - at).perp_dot(plane(&harness, x, y + 1) - at).abs();
                sum += at.x * color.x * area;
                weight += color.x * area;
            }
        }
        (weight > 0.0).then(|| sum / weight)
    };
    let start = center(0.0, 1.0).expect("the block in view");
    let later = center(10.0, 1.0).expect("the block in view");
    assert!((later - start + 10.0 * speed).abs() < 0.002, "drifted {} in 10 s", start - later);
    // Five million seconds in (58 days), f32 time would have lost the hundredths of a second;
    // drift is reduced in f64, so a step moves it the same.
    let late = 5.0e6;
    let (from, to) = (center(late, 1.0).expect("in view"), center(late + 10.0, 1.0).expect("in view"));
    assert!((to - from + 10.0 * speed).abs() < 0.002, "drifted {} in 10 s late on", from - to);
    assert_eq!(center(0.0, 0.0), None, "no cirrus at zero coverage");
    // A tier without the cirrus layer draws none.
    let mut low = Harness::new(
        RenderSettings {
            sky: SkySettings { cirrus: false, ..SkySettings::default() },
            ..RenderSettings::default()
        },
        (64, 64),
    );
    let cirrus = low.texture((64, 64), |_, _| [1.0, 0.0, 1.0, 1.0]);
    let clouds = low.texture((4, 4), |_, _| [0.0; 4]);
    let handle = low.sky_material(&cloud_material([0.0; 2]), Some(&clouds), Some(&cirrus));
    low.look(Vec3::Y, 60.0);
    let image = low.render(|packet| {
        packet.sky_material = Some(handle);
        packet.sky_layers.clouds = CloudLight { lit: Vec3::X, cirrus: 1.0, ..CloudLight::default() };
    });
    assert!(image.iter().all(|color| *color == Vec3::ZERO));
}

/// A cube face texel's color, decoded from `Rg11b10Ufloat` or `Rgba16Float`.
fn cube_texels(format: wgpu::TextureFormat, bytes: &[u8]) -> Vec<Vec3> {
    let small_float = |bits: u32, mantissa_bits: u32| {
        let mantissa = bits & ((1 << mantissa_bits) - 1);
        let exponent = (bits >> mantissa_bits) as i32;
        let fraction = mantissa as f32 / (1 << mantissa_bits) as f32;
        if exponent == 0 { fraction * 2f32.powi(-14) } else { (1.0 + fraction) * 2f32.powi(exponent - 15) }
    };
    match format {
        wgpu::TextureFormat::Rg11b10Ufloat => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&texel| {
                let bits = u32::from_le_bytes(texel);
                Vec3::new(
                    small_float(bits & 0x7ff, 6),
                    small_float((bits >> 11) & 0x7ff, 6),
                    small_float(bits >> 22, 5),
                )
            })
            .collect(),
        wgpu::TextureFormat::Rgba16Float => bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|texel| {
                let channel = |i: usize| f16::from_le_bytes([texel[2 * i], texel[2 * i + 1]]).to_f32();
                Vec3::new(channel(0), channel(1), channel(2))
            })
            .collect(),
        other => panic!("unexpected cube format {other:?}"),
    }
}

/// The direction through texel `(x, y)` of cube face `face`, as a cube-map lookup reads it.
fn cube_direction(face: usize, (x, y): (u32, u32), size: u32) -> Vec3 {
    let [right, up, forward] = cube_face_axes(face);
    let (s, t) = ((x as f32 + 0.5) / size as f32 * 2.0 - 1.0, (y as f32 + 0.5) / size as f32 * 2.0 - 1.0);
    (forward + s * right - t * up).normalize()
}

#[test]
#[ignore = "needs a GPU"]
fn the_sky_cube_draws_every_face_first_then_one_a_frame() {
    let size = 32;
    let settings = RenderSettings {
        sky: SkySettings { cube_size: size, cube_interval: 1, cirrus: true },
        ..RenderSettings::default()
    };
    let mut harness = Harness::new(settings, (64, 64));
    let first = tricolor();
    let second = SkyGradient {
        zenith: Vec3::new(0.8, 0.1, 0.1),
        horizon: Vec3::new(0.1, 0.8, 0.1),
        ground: Vec3::new(0.1, 0.1, 0.8),
    };
    let read_faces = |harness: &Harness| {
        let cube = harness.renderer.sky().cube_texture();
        let bytes = cube.format().block_copy_size(None).expect("an uncompressed format");
        (0..CUBE_FACES)
            .map(|face| cube_texels(cube.format(), &read_texture(&harness.gpu, cube, face, bytes)))
            .collect::<Vec<_>>()
    };
    let matches = |texels: &[Vec3], face: usize, sky: &SkyGradient| {
        texels.iter().enumerate().all(|(index, texel)| {
            let dir = cube_direction(face, (index as u32 % size, index as u32 / size), size);
            // The gradient's √elevation is steep just over the horizon, where a direction's last
            // bits move the color most: those texels are left out.
            if dir.y.abs() < 0.02 {
                return true;
            }
            // Rg11b10 keeps 6 mantissa bits in red and green and 5 in blue, which the GPU may
            // truncate: within one step, 3.2% of the value.
            let expected = sky_color(sky, dir);
            (*texel - expected).abs().max_element() < 0.032 * expected.max_element() + 2e-3
        })
    };
    // The first frame draws all six faces, each as a lookup would see it.
    harness.sky = first;
    harness.render(|_| {});
    let faces = read_faces(&harness);
    for (face, texels) in faces.iter().enumerate() {
        assert!(matches(texels, face, &first), "face {face} holds the sky");
    }
    assert_eq!(harness.renderer.stats().sky.cube_faces, 6);
    // Then one face a frame, in order: after frame k of the new sky, faces 0..k hold it.
    harness.sky = second;
    for frame in 1..=CUBE_FACES as usize {
        harness.render(|_| {});
        assert_eq!(harness.renderer.stats().sky.last_face, Some((frame - 1) as u8));
        let faces = read_faces(&harness);
        for (face, texels) in faces.iter().enumerate() {
            let sky = if face < frame { &second } else { &first };
            assert!(matches(texels, face, sky), "frame {frame}: face {face}");
        }
    }
}

/// Samples `cube` the way its readers will, through a cube view: a `width` × `height`
/// equirectangular panorama (u around +Y from +X toward +Z, v down from +Y), in `Rgba16Float`.
fn panorama(gpu: &GpuContext, cube: &wgpu::TextureView, (width, height): (u32, u32)) -> Vec<Vec3> {
    let device = gpu.device();
    let source = format!(
        "@group(0) @binding(0) var cube: texture_cube<f32>;
        @group(0) @binding(1) var cube_sampler: sampler;
        @vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {{
            let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
            return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
        }}
        @fragment fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {{
            let theta = position.y / {height}.0 * 3.14159265;
            let phi = position.x / {width}.0 * 6.28318531;
            let dir = vec3<f32>(sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi));
            return vec4<f32>(textureSampleLevel(cube, cube_sampler, dir, 0.0).rgb, 1.0);
        }}"
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let format = wgpu::TextureFormat::Rgba16Float;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(cube) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    gpu.queue().submit([encoder.finish()]);
    cube_texels(format, &read_texture(gpu, &target, 0, 8))
}

#[test]
#[ignore = "needs a GPU"]
fn the_sky_cube_reads_back_as_the_sky_through_a_cube_view() {
    let settings = RenderSettings {
        sky: SkySettings { cube_size: 32, cube_interval: 1, cirrus: true },
        ..RenderSettings::default()
    };
    let mut harness = Harness::new(settings, (64, 64));
    harness.sky = tricolor();
    harness.render(|_| {});
    let size = (64, 32);
    let texels = panorama(&harness.gpu, harness.renderer.sky().cube_view(), size);
    let sky = tricolor();
    for (index, texel) in texels.iter().enumerate() {
        let (x, y) = (index as u32 % size.0, index as u32 / size.0);
        let (theta, phi) = ((y as f32 + 0.5) / size.1 as f32 * PI, (x as f32 + 0.5) / size.0 as f32 * TAU);
        let dir = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
        // Bilinear filtering of a 32² face rounds the gradient's corner at the horizon off.
        if dir.y.abs() < 0.12 {
            continue;
        }
        let expected = sky_color(&sky, dir);
        let error = (*texel - expected).abs().max_element();
        assert!(
            error < 0.04 * expected.max_element() + 0.01,
            "({x}, {y}) toward {dir}: {texel} vs {expected}"
        );
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_sky_cube_keeps_glows_but_not_discs() {
    let size = 64;
    let settings = RenderSettings {
        sky: SkySettings { cube_size: size, cube_interval: 1, cirrus: true },
        ..RenderSettings::default()
    };
    let mut harness = Harness::new(settings, (64, 64));
    // The sun straight along +X with a big disc: the +X face's center would be all disc.
    let material = SkyMaterial {
        sun: sapphire_assets::material::Disc { size: 30.0, softness: 0.0 },
        ..SkyMaterial::default()
    };
    material.validate().expect("valid");
    let handle = harness.sky_material(&material, None, None);
    harness.render(|packet| {
        packet.sky_material = Some(handle);
        packet.celestial.sun = DirectionalLight { direction: Vec3::NEG_X, ..DirectionalLight::default() };
        packet.sky_layers.sun = disc(Vec3::ONE, 50.0, Vec3::new(0.25, 0.0, 0.0), 60.0);
    });
    let cube = harness.renderer.sky().cube_texture();
    let bytes = cube.format().block_copy_size(None).expect("uncompressed");
    let face = cube_texels(cube.format(), &read_texture(&harness.gpu, cube, 0, bytes));
    // The texel just above the +X face's center: the glow's center and core (the CPU twin of the
    // shader's), and none of the disc's 50.
    let (x, y) = (size / 2, size / 2 - 1);
    let texel = face[(y * size + x) as usize];
    let expected = glow(cube_direction(0, (x, y), size), Vec3::X, 0.25, 60f32.to_radians());
    assert!((texel.x - expected).abs() < 0.035 * expected, "{texel} vs a glow of {expected}");
    assert!(texel.y < 1e-3 && texel.z < 1e-3);
}

#[test]
#[ignore = "needs a GPU"]
fn a_handle_that_is_no_sky_material_draws_the_default_sky() {
    let mut harness = Harness::new(RenderSettings::default(), (64, 64));
    harness.sky = tricolor();
    let plain = harness.render(|_| {});
    let unknown = harness.render(|packet| packet.sky_material = Some(MaterialHandle::from_raw(999)));
    assert_eq!(plain, unknown);
    assert_eq!(harness.renderer.stats().sky.ignored_materials, 1);
}
