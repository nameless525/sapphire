//! Texture upload tests that need a GPU. Run with `cargo test -p sapphire_render -- --ignored`.
//!
//! Each case cooks a procedural image with the pipeline, uploads the KTX2 file it writes,
//! samples every mip level back on the GPU (linear filtering at texel centers, so the format
//! must be filterable) and compares the result, after the texture's swizzle, with the levels
//! the cook prepared before encoding.

use std::f32::consts::TAU;

use sapphire_assets::texture::{AstcBlock, Texture, TextureFormat};
use sapphire_pipeline::cook::Cook;
use sapphire_pipeline::import::image::{ChannelLayout, SourceImage};
use sapphire_pipeline::texture::{
    self, ColorSettings, HdrMobileFormat, HdrSettings, IlmSettings, MaskChannels, MaskSettings,
    NormalSettings, RampSettings, SdfChannels, SdfSettings, TextureClass, TextureSettings,
};
use sapphire_render::texture::{GpuTexture, TextureError, check_support, wgpu_format};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// Samples one level per dispatch into a storage buffer.
const SAMPLE_SHADER: &str = r"
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var bilinear: sampler;
@group(0) @binding(2) var<storage, read_write> texels: array<vec4<f32>>;
// x: level, y: width, z: height.
@group(0) @binding(3) var<uniform> level: vec4<u32>;

@compute @workgroup_size(8, 8)
fn sample(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = level.yz;
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    texels[id.y * size.x + id.x] = textureSampleLevel(source, bilinear, uv, f32(level.x));
}
";

/// Every level of `texture`, as the GPU samples it.
fn sample_levels(gpu: &GpuContext, texture: &GpuTexture) -> Vec<Vec<[f32; 4]>> {
    let device = gpu.device();
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SAMPLE_SHADER.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        })),
        module: &module,
        entry_point: Some("sample"),
        compilation_options: Default::default(),
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let raw = texture.texture();
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut readbacks = Vec::new();
    for level in 0..raw.mip_level_count() {
        let (width, height) = ((raw.width() >> level).max(1), (raw.height() >> level).max(1));
        let size = u64::from(width * height) * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue().write_buffer(&params, 0, bytemuck::cast_slice(&[level, width, height, 0]));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture.view()),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: params.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        readbacks.push(readback);
    }
    gpu.queue().submit([encoder.finish()]);
    for readback in &readbacks {
        readback.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    }
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    readbacks
        .iter()
        .map(|readback| bytemuck::pod_collect_to_vec(&readback.get_mapped_range(..).expect("mapped")))
        .collect()
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// A smooth `size × size` image in `layout`, with every channel varying.
fn pattern(size: u32, layout: ChannelLayout, texel: impl Fn(f32, f32) -> [f32; 4]) -> SourceImage {
    let pixels = (0..size * size)
        .map(|i| texel((i % size) as f32 / size as f32, (i / size) as f32 / size as f32))
        .map(|[r, g, b, a]| match layout {
            ChannelLayout::Gray => [r, r, r, 1.0],
            ChannelLayout::GrayAlpha => [r, r, r, a],
            ChannelLayout::Rgb => [r, g, b, 1.0],
            ChannelLayout::Rgba => [r, g, b, a],
        })
        .collect();
    SourceImage::new(size, size, layout, pixels).expect("a valid image")
}

/// One smooth field over the image, in `[0, 1]`. Every channel of the patterns below is an
/// affine function of it, so each block's texels lie near a line in color space, which is
/// what one BC7 or ASTC block encodes. The tests then measure the cook and the upload, not
/// how an encoder copes with four channels varying independently inside a block.
fn field(u: f32, v: f32) -> f32 {
    0.5 + 0.3 * (u * TAU).sin() * (v * TAU * 0.5).cos() + 0.2 * (u - v)
}

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [0, 1, 2, 3].map(|c| a[c] + (b[c] - a[c]) * t)
}

/// sRGB-authored color with alpha: what a PNG decodes to.
fn color(u: f32, v: f32) -> [f32; 4] {
    let [r, g, b, a] = mix([0.1, 0.3, 0.85, 1.0], [0.95, 0.6, 0.2, 0.4], field(u, v));
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
}

/// Linear data in `[0, 1]`.
fn data(u: f32, v: f32) -> [f32; 4] {
    mix([0.1, 0.9, 0.3, 0.2], [0.9, 0.2, 0.7, 0.8], field(u, v))
}

/// A bumpy unit normal, stored in `[0, 1]`.
fn normal(u: f32, v: f32) -> [f32; 4] {
    let (x, y) = (0.35 * (u * TAU).sin(), 0.35 * (v * TAU).cos());
    let z = (1.0 - x * x - y * y).sqrt();
    [x * 0.5 + 0.5, y * 0.5 + 0.5, z * 0.5 + 0.5, 1.0]
}

/// Linear light up to 6.
fn hdr(u: f32, v: f32) -> [f32; 4] {
    mix([0.05, 0.2, 1.5, 0.5], [6.0, 3.0, 0.3, 0.5], field(u, v))
}

/// How a format's error is measured.
#[derive(Clone, Copy, Debug)]
enum Metric {
    /// Absolute, in `[0, 1]` units; RGB sRGB-encoded first for sRGB formats.
    Absolute,
    /// Relative to the texel's largest expected RGB channel (at least 1/16), as shared
    /// exponents and BC6H endpoints quantize.
    Relative,
}

struct Case {
    name: &'static str,
    class: TextureClass,
    cook: Cook,
    image: SourceImage,
    metric: Metric,
    /// Largest error allowed at any texel, and allowed on average.
    max: f32,
    mean: f32,
}

fn errors(case: &Case, format: TextureFormat, expected: &[[f32; 4]], sampled: &[[f32; 4]]) -> (f32, f32) {
    let srgb = format.is_srgb();
    let mut worst = 0.0f32;
    let mut sum = 0.0f32;
    for (e, s) in expected.iter().zip(sampled) {
        let scale = e[..3].iter().fold(1.0f32 / 16.0, |m, &v| m.max(v));
        for c in 0..4 {
            let (e, s) =
                if srgb && c < 3 { (linear_to_srgb(e[c]), linear_to_srgb(s[c])) } else { (e[c], s[c]) };
            let error = match case.metric {
                Metric::Absolute => (e - s).abs(),
                Metric::Relative => (e - s).abs() / scale,
            };
            worst = worst.max(error);
            sum += error;
        }
    }
    (worst, sum / (expected.len() * 4) as f32)
}

fn cases() -> Vec<Case> {
    let case =
        |name, class, cook, image, metric, max, mean| Case { name, class, cook, image, metric, max, mean };
    let rgba = || pattern(32, ChannelLayout::Rgba, color);
    let rgba_data = || pattern(32, ChannelLayout::Rgba, data);
    let normals = || pattern(32, ChannelLayout::Rgb, normal);
    let gray = || pattern(32, ChannelLayout::Gray, data);
    let gray_alpha = || pattern(32, ChannelLayout::GrayAlpha, data);
    let light = || pattern(32, ChannelLayout::Rgba, hdr);
    let color_class = TextureClass::Color(ColorSettings::default());
    let normal_class = TextureClass::Normal(NormalSettings::default());
    let mask = TextureClass::Mask(MaskSettings::default());
    let mask_two =
        TextureClass::Mask(MaskSettings { channels: MaskChannels::Two, ..MaskSettings::default() });
    let ilm = TextureClass::Ilm(IlmSettings::default());
    let ramp = TextureClass::Ramp(RampSettings { mips: true });
    let sdf = TextureClass::Sdf(SdfSettings::default());
    let sdf_two = TextureClass::Sdf(SdfSettings { channels: SdfChannels::Two, ..SdfSettings::default() });
    let hdr_class = TextureClass::Hdr(HdrSettings::default());
    let hdr_half = TextureClass::Hdr(HdrSettings { mobile_format: HdrMobileFormat::Rgba16Float, mips: true });
    use Cook::{Desktop, Mobile};
    use Metric::{Absolute, Relative};
    vec![
        case("color BC7 sRGB", color_class, Desktop, rgba(), Absolute, 0.03, 0.006),
        case("color ASTC 4x4 sRGB", color_class, Mobile, rgba(), Absolute, 0.04, 0.008),
        case("normal BC5", normal_class, Desktop, normals(), Absolute, 0.02, 0.004),
        case("normal ASTC (ra01)", normal_class, Mobile, normals(), Absolute, 0.03, 0.006),
        case("mask BC4", mask, Desktop, gray(), Absolute, 0.035, 0.006),
        case("mask ASTC 1 channel (r001)", mask, Mobile, gray(), Absolute, 0.03, 0.006),
        case("mask BC5", mask, Desktop, gray_alpha(), Absolute, 0.035, 0.008),
        case("mask ASTC 2 channels (ra01)", mask_two, Mobile, rgba_data(), Absolute, 0.03, 0.006),
        case("mask BC7", mask, Desktop, rgba_data(), Absolute, 0.03, 0.006),
        case("ILM BC7", ilm, Desktop, rgba_data(), Absolute, 0.03, 0.006),
        case("ILM ASTC 4x4", ilm, Mobile, rgba_data(), Absolute, 0.04, 0.008),
        case("ramp RGBA8 sRGB", ramp, Desktop, rgba(), Absolute, 1.0 / 255.0, 0.5 / 255.0),
        case("SDF R16 (r101)", sdf, Desktop, gray(), Absolute, 0.6 / 65535.0, 0.3 / 65535.0),
        case("SDF RG16", sdf_two, Desktop, gray_alpha(), Absolute, 0.6 / 65535.0, 0.3 / 65535.0),
        case("SDF R8 (r101)", sdf, Mobile, gray(), Absolute, 1.0 / 255.0, 0.5 / 255.0),
        case("HDR BC6H", hdr_class, Desktop, light(), Relative, 0.06, 0.015),
        case("HDR RGB9E5", hdr_class, Mobile, light(), Relative, 1.0 / 256.0, 1.0 / 1024.0),
        case("HDR RGBA16F", hdr_half, Mobile, light(), Relative, 1.0 / 1024.0, 1.0 / 4096.0),
    ]
}

#[test]
#[ignore = "needs a GPU"]
fn every_class_samples_back_within_tolerance() {
    let gpu = gpu();
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    for case in cases() {
        let prepared =
            texture::prepare(&case.image, &TextureSettings { class: case.class }, case.cook).expect("plan");
        let format = prepared.desc.format;
        let file = prepared.encode().expect("encode").to_ktx2();
        let cooked = Texture::from_ktx2(&file).expect("the cooked file reads back");
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let uploaded = match GpuTexture::upload(&gpu, &cooked, case.name) {
            Ok(uploaded) => uploaded,
            Err(error @ TextureError::Unsupported { .. }) => {
                skipped.push(format!("{}: {error}", case.name));
                continue;
            }
            Err(error) => panic!("{}: {error}", case.name),
        };
        let sampled = sample_levels(&gpu, &uploaded);
        assert!(pollster::block_on(scope.pop()).is_none(), "{}: validation error", case.name);
        assert_eq!(sampled.len(), prepared.levels.len(), "{}", case.name);
        // Every level, down to 1×1, which ctt's own BC7 chains got wrong (its issue #114).
        for (level, (expected, sampled)) in prepared.levels.iter().zip(&sampled).enumerate() {
            let logical: Vec<[f32; 4]> =
                sampled.iter().map(|&texel| uploaded.swizzle().apply(texel)).collect();
            let (max, mean) = errors(&case, format, &expected.pixels, &logical);
            eprintln!(
                "{:<28} {format:?} level {level} ({}x{}): max {max:.5}, mean {mean:.5}",
                case.name, expected.width, expected.height
            );
            if max > case.max || mean > case.mean {
                failures.push(format!("{} level {level}: max {max}, mean {mean}", case.name));
            }
        }
    }
    for skip in &skipped {
        eprintln!("skipped {skip}");
    }
    assert!(failures.is_empty(), "{failures:#?}");
    // Apple GPUs support BC, ASTC and 16-bit normalized formats, so nothing may be skipped there.
    if gpu.caps().backend() == wgpu::Backend::Metal {
        assert!(skipped.is_empty(), "{skipped:?}");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn unsupported_formats_fail_with_the_cook_they_need() {
    let gpu = gpu();
    // A device like this one without BC: what a phone reports for the desktop cook's formats.
    let mut caps = gpu.caps().clone();
    caps.features.remove(wgpu::Features::TEXTURE_COMPRESSION_BC);
    for format in [TextureFormat::Bc7RgbaUnormSrgb, TextureFormat::Bc5RgUnorm, TextureFormat::Bc6hRgbUfloat] {
        let raw = wgpu_format(format);
        let support = caps.format_support(raw, gpu.adapter().get_texture_format_features(raw));
        let error = check_support(format, support, &caps.adapter.name).unwrap_err();
        assert!(matches!(error, TextureError::Unsupported { .. }), "{error:?}");
        let message = error.to_string();
        assert!(
            message.contains("needs the mobile cook") && message.contains(&caps.adapter.name),
            "{message}"
        );
    }
    // And one without ASTC, for the mobile cook's.
    let mut caps = gpu.caps().clone();
    caps.features.remove(wgpu::Features::TEXTURE_COMPRESSION_ASTC);
    let format = TextureFormat::Astc { block: AstcBlock::B4x4, srgb: true };
    let raw = wgpu_format(format);
    let support = caps.format_support(raw, gpu.adapter().get_texture_format_features(raw));
    assert!(
        check_support(format, support, &caps.adapter.name).unwrap_err().to_string().contains("desktop cook")
    );

    // An invalid texture never reaches wgpu.
    let cooked = texture::cook(
        &pattern(8, ChannelLayout::Rgba, color),
        &TextureSettings { class: TextureClass::Color(ColorSettings::default()) },
        Cook::Desktop,
    )
    .expect("cook");
    let mut broken = cooked.texture();
    broken.levels[1] = &broken.levels[1][..8];
    assert!(matches!(GpuTexture::upload(&gpu, &broken, "broken"), Err(TextureError::Invalid(_))));
}
