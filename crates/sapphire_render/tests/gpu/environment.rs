//! Environment shader tests that need a GPU (M1 W4): the key light through wrap lighting and a
//! terminator that widens with distance, the shadow tint and band, the cast-shadow hook's lit
//! default, distance desaturation, the tiers' specular, emission and time of day's scale on it,
//! local lights, both families on
//! one mesh, the prepasses and temporal AA over the world, and hot reload. Run with
//! `cargo test -p sapphire_render --test gpu environment:: -- --ignored`.
//!
//! Scenes have a black sky, no MSAA and post that shows linear values as they are, so each pixel
//! can be predicted: a CPU model of the shader ([`Model`]) shades the exact point a pixel's ray
//! hits on an analytic sphere or plane, and the frame must match it within 8-bit precision and
//! the tessellation's faceting.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use half::f16;
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, CharacterRegion, DistanceStylization, EnvironmentMaterial, LightSplit,
    Material, MaterialDocument, Outline, Rim, SkyMaterial, Terminator, TerminatorBand,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Submesh, VERTEX_COLOR_NEUTRAL, Vertex,
    VertexFlags,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor, TextureId};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    AmbientLight, BloomParams, CameraData, ColorGrade, DirectionalLight, EnvironmentSettings,
    EnvironmentSpecular, FramePacketRing, InstanceId, LightKind, LightPath, LocalLight, MaterialHandle,
    MeshHandle, MeshInstance, Msaa, PostParams, PrepassTargets, RenderError, RenderSettings, Renderer,
    ShaderFile, ShaderSources, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Image side in pixels.
const SIDE: u32 = 128;
/// How far a pixel's linear color may be from the model: 8-bit output, and the tessellated
/// sphere's normals a fraction of a degree from the analytic one's.
const TOLERANCE: f32 = 0.025;
/// Rec. 709 luminance, as the shader weighs it.
const LUMINANCE: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// The texture IDs materials name; each test binds its own textures to them.
const BAND: &str = "3d1e7f2a-9b4c-4d5e-8f6a-7b8c9d0e1f2a";
const EMISSION: &str = "6a7b8c9d-0e1f-4a2b-9c3d-4e5f6a7b8c9d";
const MASK: &str = "8e9f0a1b-2c3d-4e4f-8a5b-6c7d8e9f0a1b";

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

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A plain environment material: grey, Lambert, a hard terminator, no highlights (the clamp at
/// 0; the hemisphere's reflection stays wherever there is ambient light) and nothing kept in
/// shadow beyond the frame's tint.
fn plain(albedo: Vec3) -> EnvironmentMaterial {
    EnvironmentMaterial {
        base_color: albedo.extend(1.0).to_array(),
        roughness: 0.5,
        specular_clamp: 0.0,
        wrap: 0.0,
        terminator: Terminator { width: 0.0, growth: 0.0, band: None },
        distance: DistanceStylization { start: 0.0, end: 0.0, desaturation: 0.0 },
        ..EnvironmentMaterial::default()
    }
}

/// The environment shader on the CPU: one point's color, as `environment.wgsl` shades it, for a
/// material and a frame without local lights or textures beyond the band.
#[derive(Clone, Copy)]
struct Model<'a> {
    material: &'a EnvironmentMaterial,
    specular: EnvironmentSpecular,
    /// The frame's key light: unit vector toward it, and color × intensity.
    toward_key: Vec3,
    key: Vec3,
    ambient: AmbientLight,
    shadow_tint: Vec3,
    /// Time of day's emission scale (`FramePacket::emission_scale`).
    emission_scale: f32,
    /// The band's ramp, sampled as the GPU does (one row, linear, clamped).
    band: Option<&'a [[f32; 4]]>,
    /// The mask map's texel, the same everywhere (all 1 without a map): AO, cavity, specular
    /// mask, roughness multiplier.
    mask: [f32; 4],
}

impl Model<'_> {
    /// The color at a point with unit normal `n`, seen from `eye`.
    fn shade(&self, point: Vec3, n: Vec3, eye: Vec3) -> Vec3 {
        let m = self.material;
        let distance = point.distance(eye);
        let v = (eye - point) / distance;
        let base = Vec3::from_slice(&m.base_color[..3]);
        let stylization = m.distance;
        let t = ((distance - stylization.start) / (stylization.end - stylization.start).max(1e-3))
            .clamp(0.0, 1.0);
        let desaturation = stylization.desaturation * t * t * (3.0 - 2.0 * t);
        let albedo = base.lerp(Vec3::splat(base.dot(LUMINANCE)), desaturation);
        let [occlusion, cavity, specular_mask, rough] = self.mask;
        let (diffuse, f0) = match self.specular {
            EnvironmentSpecular::Off => (albedo, Vec3::ZERO),
            _ => (albedo * (1.0 - m.metallic), Vec3::splat(0.04).lerp(albedo, m.metallic) * specular_mask),
        };
        let [low, high] = m.roughness_range;
        let roughness = (low + (high - low) * (m.roughness * rough).clamp(0.0, 1.0)).max(0.045);
        let surface = Surface {
            n,
            v,
            f0,
            roughness,
            occlusion,
            cavity,
            specular: self.specular,
            clamp: m.specular_clamp,
        };

        let l = self.toward_key;
        let n_dot_l = n.dot(l);
        let width = m.terminator.width + m.terminator.growth * distance;
        let lit = smoothstep(-width.max(1e-5), width.max(1e-5), n_dot_l);
        let band = match (self.band, m.terminator.band) {
            (Some(ramp), Some(band)) => Vec3::ONE.lerp(sample_row(ramp, lit), band.strength),
            _ => Vec3::ONE,
        };
        let keep = self.shadow_tint * Vec3::from(m.shadow_tint);
        let key = self.key * keep.lerp(Vec3::ONE, lit * wrapped(n_dot_l, m.wrap)) * band;
        let ambient = ambient_light(&self.ambient, n) * occlusion * cavity;
        diffuse * (key + ambient)
            + self.key * surface.specular(l, n_dot_l) * lit
            + surface.ambient_specular(&self.ambient)
            + Vec3::from(m.emission) * (self.emission_scale + (1.0 - self.emission_scale) * m.emission_steady)
    }
}

fn wrapped(n_dot_l: f32, wrap: f32) -> f32 {
    ((n_dot_l + wrap) / (1.0 + wrap)).clamp(0.0, 1.0)
}

fn ambient_light(ambient: &AmbientLight, n: Vec3) -> Vec3 {
    (ambient.ground * ambient.intensity).lerp(ambient.sky * ambient.intensity, 0.5 + 0.5 * n.y)
}

/// One row of linear texels sampled at `u` through a linear, clamped sampler.
fn sample_row(row: &[[f32; 4]], u: f32) -> Vec3 {
    let x = (u * row.len() as f32 - 0.5).clamp(0.0, (row.len() - 1) as f32);
    let (i, f) = (x.floor() as usize, x.fract());
    let at = |i: usize| Vec3::from_slice(&row[i.min(row.len() - 1)][..3]);
    at(i).lerp(at(i + 1), f)
}

/// What the specular model needs of a point.
struct Surface {
    n: Vec3,
    v: Vec3,
    f0: Vec3,
    roughness: f32,
    /// The ambient light's occlusion, and the cavity every reflection keeps.
    occlusion: f32,
    cavity: f32,
    specular: EnvironmentSpecular,
    clamp: f32,
}

impl Surface {
    fn specular(&self, l: Vec3, n_dot_l: f32) -> Vec3 {
        if self.specular == EnvironmentSpecular::Off {
            return Vec3::ZERO;
        }
        let h = (l + self.v).normalize_or_zero();
        let (n_dot_h, l_dot_h) = (self.n.dot(h).clamp(0.0, 1.0), l.dot(h).clamp(0.0, 1.0));
        let alpha = self.roughness * self.roughness;
        let alpha2 = alpha * alpha;
        let d = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
        let distribution = alpha2 / (PI * d * d);
        let n_dot_l = n_dot_l.clamp(0.0, 1.0);
        let reflected = if self.specular == EnvironmentSpecular::Full {
            let n_dot_v = self.n.dot(self.v).max(1e-4);
            let view = n_dot_l * (n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2).sqrt();
            let light = n_dot_v * (n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2).sqrt();
            let visibility = 0.5 / (view + light).max(1e-6);
            let fresnel = self.f0 + (Vec3::splat(self.grazing()) - self.f0) * (1.0 - l_dot_h).powi(5);
            PI * distribution * visibility * fresnel
        } else {
            self.f0 * (PI * distribution / (4.0 * (l_dot_h * l_dot_h).max(0.1) * (alpha + 0.5)))
        };
        (reflected * n_dot_l).min(Vec3::splat(self.clamp)) * self.cavity
    }

    /// The grazing reflectance: 1, but taken down with a reflectance under about 2%.
    fn grazing(&self) -> f32 {
        self.f0.dot(Vec3::splat(50.0 * 0.33)).clamp(0.0, 1.0)
    }

    fn ambient_specular(&self, ambient: &AmbientLight) -> Vec3 {
        if self.specular == EnvironmentSpecular::Off {
            return Vec3::ZERO;
        }
        let n_dot_v = self.n.dot(self.v).clamp(0.0, 1.0);
        let r = [
            1.0 - self.roughness,
            self.roughness * -0.0275 + 0.0425,
            self.roughness * -0.572 + 1.04,
            self.roughness * 0.022 - 0.04,
        ];
        let a004 = (r[0] * r[0]).min((-9.28 * n_dot_v).exp2()) * r[0] + r[1];
        let (scale, bias) = (-1.04 * a004 + r[2], 1.04 * a004 + r[3]);
        let reflected = -self.v - 2.0 * (-self.v).dot(self.n) * self.n;
        let brdf = (self.f0 * scale + Vec3::splat(bias * self.grazing())).max(Vec3::ZERO);
        ambient_light(ambient, reflected) * brdf * self.occlusion * self.cavity
    }
}

/// One rendered frame, as linear RGB per pixel.
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

    /// Compares every pixel whose ray meets `surface` (a point and its unit normal) squarely enough
    /// to be clear of silhouettes against `expected`, and returns the largest difference and where.
    fn compare(
        &self,
        surface: impl Fn(Vec3, Vec3) -> Option<(Vec3, Vec3)>,
        expected: impl Fn(Vec3, Vec3) -> Vec3,
    ) -> (f32, (u32, u32), usize) {
        let mut worst = (0.0f32, (0, 0));
        let mut compared = 0;
        for y in 0..SIDE {
            for x in 0..SIDE {
                let (eye, direction) = self.ray(x, y);
                let Some((point, normal)) = surface(eye, direction) else { continue };
                if normal.dot(-direction) < 0.3 {
                    continue;
                }
                compared += 1;
                let error = (self.linear(x, y) - expected(point, normal)).abs().max_element();
                if error > worst.0 {
                    worst = (error, (x, y));
                }
            }
        }
        (worst.0, worst.1, compared)
    }
}

/// Where a ray from `eye` along unit `direction` first meets the sphere of `radius` at `center`,
/// and the outward normal there.
fn hit_sphere(center: Vec3, radius: f32) -> impl Fn(Vec3, Vec3) -> Option<(Vec3, Vec3)> {
    move |eye, direction| {
        let to_eye = eye - center;
        let b = to_eye.dot(direction);
        let c = to_eye.length_squared() - radius * radius;
        let discriminant = b * b - c;
        (discriminant > 0.0).then(|| {
            let point = eye + direction * (-b - discriminant.sqrt());
            (point, (point - center) / radius)
        })
    }
}

/// Where a ray meets the rectangle centered at `center` facing `normal`, `half` wide along
/// `across` and `half` tall along `normal × across`.
fn hit_rectangle(
    center: Vec3,
    normal: Vec3,
    across: Vec3,
    half: f32,
) -> impl Fn(Vec3, Vec3) -> Option<(Vec3, Vec3)> {
    move |eye, direction| {
        let denominator = direction.dot(normal);
        if denominator.abs() < 1e-6 {
            return None;
        }
        let point = eye + direction * ((center - eye).dot(normal) / denominator);
        let offset = point - center;
        let up = normal.cross(across);
        (offset.dot(across).abs() < half && offset.dot(up).abs() < half).then_some((point, normal))
    }
}

fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2]) -> Vertex {
    Vertex {
        position: position.to_array(),
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv,
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    }
}

/// A mesh of `parts`, each its own submesh drawing its own material slot (so families can mix),
/// with f32 positions so shading sees the exact geometry.
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

/// A UV sphere of `radius` around `center`, finely tessellated.
fn sphere(center: Vec3, radius: f32) -> (Vec<Vertex>, Vec<u32>) {
    const RINGS: u32 = 96;
    const SEGMENTS: u32 = 128;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
            let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            let uv = [segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32];
            vertices.push(vertex(center + n * radius, n, uv));
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

/// A square centered at `center` facing `normal`, `half` from its center to each edge, `u`
/// along `across` and `v` down `normal × across`.
fn square(center: Vec3, normal: Vec3, across: Vec3, half: f32) -> (Vec<Vertex>, Vec<u32>) {
    let up = normal.cross(across);
    let corner = |s: f32, t: f32| {
        let uv = [(s + 1.0) * 0.5, (1.0 - t) * 0.5];
        vertex(center + (across * s + up * t) * half, normal, uv)
    };
    (vec![corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)], vec![0, 1, 2, 0, 2, 3])
}

/// An RGBA16 float texture of linear data, `width` × `height`, per `texel(u, v)` at each texel's
/// center.
fn half_texture(
    gpu: &GpuContext,
    width: u32,
    height: u32,
    texel: impl Fn(f32, f32) -> [f32; 4],
) -> GpuTexture {
    let texels: Vec<[u16; 4]> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (u, v) = ((x as f32 + 0.5) / width as f32, (y as f32 + 0.5) / height as f32);
            texel(u, v).map(|c| f16::from_f32(c).to_bits())
        })
        .collect();
    let desc = TextureDesc {
        format: TextureFormat::Rgba16Float,
        width,
        height,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let bytes: &[u8] = bytemuck::cast_slice(&texels);
    GpuTexture::upload(gpu, &Texture { desc, levels: vec![bytes] }, "half").expect("texture")
}

/// Cooks `document` against the textures it names, bound by ID, as the pipeline would.
fn cook<D: MaterialDocument>(document: &D, textures: &[(&str, &GpuTexture)]) -> Vec<u8> {
    let swizzles: Vec<Swizzle> = document
        .textures()
        .iter()
        .map(|slot| {
            slot.and_then(|asked| textures.iter().find(|(name, _)| id(name) == asked))
                .map_or(Swizzle::IDENTITY, |(_, texture)| texture.swizzle())
        })
        .collect();
    Material::from_document(document, &swizzles).expect("valid").to_cooked().expect("cook").to_vec()
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
    ambient: AmbientLight,
    shadow_tint: Vec3,
    emission_scale: f32,
    lights: Vec<LocalLight>,
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
            camera: CameraData::look_at(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y, 0.6, 1.0, 0.05),
            sun: DirectionalLight { direction: Vec3::NEG_X, color: Vec3::ONE, intensity: 1.0 },
            ambient: AmbientLight { sky: Vec3::ZERO, ground: Vec3::ZERO, intensity: 0.0 },
            shadow_tint: Vec3::ZERO,
            emission_scale: 1.0,
            lights: Vec::new(),
            added: Vec::new(),
            next_id: 0,
        }
    }

    /// Without MSAA, so every interior pixel is shaded once at its center, as the model is.
    fn exact() -> Self {
        Self::new(RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() })
    }

    /// With the tier's `specular` model.
    fn with_specular(specular: EnvironmentSpecular) -> Self {
        let environment = EnvironmentSettings { specular, ..EnvironmentSettings::default() };
        Self::new(RenderSettings { msaa: Msaa::Off, environment, ..RenderSettings::default() })
    }

    /// Frames a sphere of `radius` at the origin from `distance` on +Z, nearly edge to edge.
    fn frame_sphere(&mut self, radius: f32, distance: f32) {
        let fov = 2.0 * (radius / distance).asin() * 1.08;
        self.camera = CameraData::look_at(Vec3::new(0.0, 0.0, distance), Vec3::ZERO, Vec3::Y, fov, 1.0, 0.05);
    }

    fn model<'a>(&self, material: &'a EnvironmentMaterial, specular: EnvironmentSpecular) -> Model<'a> {
        Model {
            material,
            specular,
            toward_key: (-self.sun.direction).normalize(),
            key: self.sun.color * self.sun.intensity,
            ambient: self.ambient,
            shadow_tint: self.shadow_tint,
            emission_scale: self.emission_scale,
            band: None,
            mask: [1.0; 4],
        }
    }

    /// Cooks `document` against the textures it names and uploads it.
    fn environment(
        &mut self,
        document: &EnvironmentMaterial,
        textures: &[(&str, &GpuTexture)],
    ) -> MaterialHandle {
        let bytes = cook(document, textures);
        let lookup = |asked: AssetId| textures.iter().find(|(name, _)| id(name) == asked).map(|(_, t)| *t);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), lookup).expect("upload")
    }

    fn character(&mut self, document: &AnimeCharacterMaterial) -> MaterialHandle {
        let bytes = cook(document, &[]);
        self.renderer.upload_material(Material::read(&bytes).expect("read"), |_| None).expect("upload")
    }

    /// Replaces `handle` with `document`.
    fn update(&mut self, handle: MaterialHandle, document: &EnvironmentMaterial) {
        let bytes = cook(document, &[]);
        self.renderer
            .update_material(handle, Material::read(&bytes).expect("read"), |_| None)
            .expect("update");
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

    /// Draws one frame and reads the target back. Fails on any validation error.
    fn render(&mut self) -> Frame {
        self.render_reading(&[])
    }

    /// Draws one frame, also copying the prepass targets `reads` picks into their readbacks.
    fn render_reading(&mut self, reads: &[Read<'_>]) -> Frame {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(self.camera));
        packet.sun = self.sun;
        packet.sky = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        packet.ambient = self.ambient;
        packet.shadow_tint = self.shadow_tint;
        packet.emission_scale = self.emission_scale;
        packet.post = passthrough();
        packet.lights.extend_from_slice(&self.lights);
        packet.instances.changed.append(&mut self.added);
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(&self.gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        let targets = self.renderer.record(&mut graph, target).expect("a view and a target");
        let reads: Vec<(TextureId, &Readback)> =
            reads.iter().map(|(pick, readback)| (pick(&targets), *readback)).collect();
        if !reads.is_empty() {
            let mut pass = graph.add_pass("read back").side_effect();
            for (texture, _) in &reads {
                pass = pass.read(*texture);
            }
            pass.record(move |ctx| {
                for (texture, readback) in reads {
                    readback.copy(ctx.encoder, ctx.texture(texture));
                }
            });
        }
        self.graph.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        let target = Readback::new(&self.gpu, (SIDE, SIDE), 4);
        let mut encoder = self.gpu.device().create_command_encoder(&Default::default());
        target.copy(&mut encoder, &self.target);
        self.gpu.queue().submit([encoder.finish()]);
        let pixels = target
            .texels(&self.gpu)
            .into_iter()
            .map(|texel| [texel[0], texel[1], texel[2], texel[3]])
            .collect();
        Frame { pixels, camera: self.camera }
    }
}

/// A prepass target to read back after a frame, and where to.
type Read<'a> = (fn(&PrepassTargets) -> TextureId, &'a Readback);

/// A buffer a texture of up to 8 bytes per texel is copied into, row by row.
struct Readback {
    buffer: wgpu::Buffer,
    size: (u32, u32),
    texel: u32,
    padded: u32,
}

impl Readback {
    fn new(gpu: &GpuContext, size: (u32, u32), texel: u32) -> Self {
        let padded = (size.0 * texel).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * size.1),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, size, texel, padded }
    }

    fn copy(&self, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        let aspect = if texture.format().is_depth_stencil_format() {
            wgpu::TextureAspect::DepthOnly
        } else {
            wgpu::TextureAspect::All
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { aspect, ..texture.as_image_copy() },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded),
                    rows_per_image: Some(self.size.1),
                },
            },
            texture.size(),
        );
    }

    /// Each texel's bytes, rows top to bottom.
    fn texels(&self, gpu: &GpuContext) -> Vec<Vec<u8>> {
        self.buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
        gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let texels = {
            let mapped = self.buffer.get_mapped_range(..).expect("mapped");
            mapped
                .chunks_exact(self.padded as usize)
                .flat_map(|row| row[..(self.size.0 * self.texel) as usize].chunks_exact(self.texel as usize))
                .map(<[u8]>::to_vec)
                .collect()
        };
        self.buffer.unmap();
        texels
    }
}

fn f16_to_f32(bits: u16) -> f32 {
    f16::from_bits(bits).to_f32()
}

/// An `Rg16Float` texel's two values.
fn rg16f(texel: &[u8]) -> [f32; 2] {
    [u16::from_le_bytes([texel[0], texel[1]]), u16::from_le_bytes([texel[2], texel[3]])].map(f16_to_f32)
}

fn depth(texel: &[u8]) -> f32 {
    f32::from_le_bytes([texel[0], texel[1], texel[2], texel[3]])
}

/// Renders a unit sphere at the origin from `distance` with `material` and returns the frame.
fn sphere_frame(harness: &mut Harness, material: &EnvironmentMaterial, distance: f32) -> Frame {
    let handle = harness.environment(material, &[]);
    let mesh = harness.mesh(&mesh(vec![sphere(Vec3::ZERO, 1.0)]), &[handle]);
    harness.frame_sphere(1.0, distance);
    harness.add(mesh, Mat4::IDENTITY);
    harness.render()
}

#[test]
#[ignore = "needs a GPU"]
fn the_key_light_wraps_and_its_terminator_widens_with_distance() {
    // A grey sphere lit from its right: the terminator is the vertical line through its middle.
    let material = EnvironmentMaterial {
        wrap: 0.5,
        terminator: Terminator { width: 0.06, growth: 0.035, band: None },
        ..plain(Vec3::splat(0.7))
    };
    // Light reaching N·L = -0.1, just inside the shadow side, as a share of the lit side's.
    let mut reach = Vec::new();
    for distance in [3.0, 12.0] {
        let mut harness = Harness::exact();
        harness.shadow_tint = Vec3::new(0.1, 0.15, 0.3);
        let frame = sphere_frame(&mut harness, &material, distance);
        let model = harness.model(&material, EnvironmentSpecular::Full);
        let (error, at, compared) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
            model.shade(point, normal, frame.camera.position)
        });
        assert!(compared > 8000, "the sphere fills the frame: {compared} pixels");
        assert!(error < TOLERANCE, "at {distance} m: {error} at {at:?}");

        // The terminator's value there: the key light's step above what shadows keep, over the
        // wrapped falloff's.
        let (x, y) = frame.pixel_of(Vec3::new(-0.1, 0.0, 0.99f32.sqrt()));
        reach.push((frame.linear(x, y).x - 0.7 * 0.1) / (0.7 * 0.9 * wrapped(-0.1, 0.5)));
    }
    // Up close the width is 0.13 in N·L, so the point is nearly dark (smoothstep's 0.04); 11 m
    // away it is 0.45 and the transition reaches it (0.33).
    assert!(reach[0] < 0.12 && reach[1] > 0.25, "the far terminator is softer: {reach:?}");

    // Without wrap the lit side falls off as Lambert's: half the light at N·L = 0.5.
    let lambert = EnvironmentMaterial { wrap: 0.0, ..material };
    let mut harness = Harness::exact();
    let frame = sphere_frame(&mut harness, &lambert, 3.0);
    let model = harness.model(&lambert, EnvironmentSpecular::Full);
    let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
        model.shade(point, normal, frame.camera.position)
    });
    assert!(error < TOLERANCE, "Lambert: {error} at {at:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn shadows_keep_a_tinted_key_light_and_the_band_colors_the_terminator() {
    let mut harness = Harness::exact();
    harness.shadow_tint = Vec3::new(0.3, 0.35, 0.6);
    // White at both ends, a warm orange across the middle of the transition.
    let ramp: Vec<[f32; 4]> = (0..64)
        .map(|i| {
            let u = (i as f32 + 0.5) / 64.0;
            let warmth = 1.0 - (2.0 * u - 1.0).abs();
            Vec3::ONE.lerp(Vec3::new(1.0, 0.45, 0.25), warmth).extend(1.0).to_array()
        })
        .collect();
    let band = half_texture(&harness.gpu, 64, 1, |u, _| ramp[(u * 64.0) as usize]);
    let material = EnvironmentMaterial {
        shadow_tint: [1.0, 0.8, 1.0],
        wrap: 0.3,
        terminator: Terminator {
            width: 0.25,
            growth: 0.0,
            band: Some(TerminatorBand { map: id(BAND), strength: 0.8 }),
        },
        ..plain(Vec3::splat(0.75))
    };
    let handle = harness.environment(&material, &[(BAND, &band)]);
    let mesh = harness.mesh(&mesh(vec![sphere(Vec3::ZERO, 1.0)]), &[handle]);
    harness.frame_sphere(1.0, 3.0);
    harness.add(mesh, Mat4::IDENTITY);
    let frame = harness.render();
    let model = Model { band: Some(&ramp), ..harness.model(&material, EnvironmentSpecular::Full) };
    let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
        model.shade(point, normal, frame.camera.position)
    });
    assert!(error < TOLERANCE, "{error} at {at:?}");

    // The shadow side keeps the key light in the frame's tint times the material's: colored,
    // never black. The middle of the transition is warmer than the lit side.
    let row = SIDE / 2;
    let shadow = frame.linear(SIDE / 8, row);
    let keep = Vec3::new(0.3, 0.35 * 0.8, 0.6) * 0.75;
    assert!((shadow - keep).abs().max_element() < TOLERANCE, "{shadow} against {keep}");
    let terminator = frame.pixel_of(Vec3::new(0.0, 0.0, 1.0));
    let middle = frame.linear(terminator.0, row);
    let lit = frame.linear(SIDE * 7 / 8, row);
    assert!(middle.x / middle.z > 1.3 * lit.x / lit.z, "a warm band: {middle} against {lit}");
}

#[test]
#[ignore = "needs a GPU"]
fn the_cast_shadow_hook_leaves_everything_lit_until_shadows_fill_it() {
    // A square under a strong shadow tint, lit from the front and then from behind: facing the
    // key it takes the whole key light, facing away only what shadows keep. A hook returning
    // shadow would turn the first into the second.
    let mut harness = Harness::exact();
    harness.shadow_tint = Vec3::new(0.2, 0.25, 0.5);
    harness.sun.direction = Vec3::NEG_Z;
    let albedo = Vec3::new(0.7, 0.6, 0.5);
    let material = EnvironmentMaterial { wrap: 0.4, ..plain(albedo) };
    let handle = harness.environment(&material, &[]);
    let facing = square(Vec3::new(-0.5, 0.0, 0.0), Vec3::Z, Vec3::X, 0.45);
    let mesh = harness.mesh(&mesh(vec![facing]), &[handle]);
    harness.camera = CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.6, 1.0, 0.05);
    harness.add(mesh, Mat4::IDENTITY);
    harness.sun.direction = Vec3::new(0.0, 0.0, -1.0);
    let lit = harness.render();
    let center = lit.pixel_of(Vec3::new(-0.5, 0.0, 0.0));
    assert!((lit.linear(center.0, center.1) - albedo).abs().max_element() < TOLERANCE);

    // Lit from behind, the same square keeps the tint alone.
    harness.sun.direction = Vec3::Z;
    let shaded = harness.render();
    let keep = albedo * harness.shadow_tint;
    assert!((shaded.linear(center.0, center.1) - keep).abs().max_element() < TOLERANCE);
}

#[test]
#[ignore = "needs a GPU"]
fn distant_surfaces_lose_their_color_but_keep_the_light_s() {
    // Three squares facing a warm sun behind the camera at 4, 13 and 40 m; desaturation runs from
    // 6 m to 24 m.
    let mut harness = Harness::exact();
    harness.sun =
        DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::new(1.0, 0.85, 0.7), intensity: 1.0 };
    harness.camera = CameraData::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 0.8, 1.0, 0.05);
    let albedo = Vec3::new(0.8, 0.25, 0.1);
    let material = EnvironmentMaterial {
        distance: DistanceStylization { start: 6.0, end: 24.0, desaturation: 1.0 },
        ..plain(albedo)
    };
    let handle = harness.environment(&material, &[]);
    let placed = [(-0.25, 4.0), (0.0, 13.0), (0.25, 40.0)]
        .map(|(x, distance)| (Vec3::new(x * distance, 0.0, -distance), 0.09 * distance));
    let parts = placed.iter().map(|&(center, half)| square(center, Vec3::Z, Vec3::X, half)).collect();
    let mesh = harness.mesh(&mesh(parts), &[handle; 3]);
    harness.add(mesh, Mat4::IDENTITY);
    let frame = harness.render();
    let model = harness.model(&material, EnvironmentSpecular::Full);
    for (center, half) in placed {
        let (error, at, compared) = frame
            .compare(hit_rectangle(center, Vec3::Z, Vec3::X, half * 0.9), |point, normal| {
                model.shade(point, normal, frame.camera.position)
            });
        assert!(compared > 50 && error < TOLERANCE, "at {center}: {error} at {at:?} over {compared}");
    }
    let color = |index: usize| {
        let (x, y) = frame.pixel_of(placed[index].0);
        frame.linear(x, y)
    };
    let (near, far) = (color(0), color(2));
    assert!((near - albedo * harness.sun.color).abs().max_element() < TOLERANCE, "near keeps its color");
    // Far, the albedo is its own luminance: the pixel is the key light's color.
    let grey = albedo.dot(LUMINANCE) * harness.sun.color;
    assert!((far - grey).abs().max_element() < TOLERANCE, "{far} against {grey}");
    assert!((far.x / far.y - 1.0 / 0.85).abs() < 0.05, "the light keeps its hue: {far}");
}

#[test]
#[ignore = "needs a GPU"]
fn highlights_follow_each_tiers_ggx_under_the_clamp() {
    // Lit from above and to the right, toward the camera's side, so the highlight sits on the
    // sphere's front; a dim sky and ground add their reflection.
    let material =
        EnvironmentMaterial { roughness: 0.5, specular_clamp: 4.0, ..plain(Vec3::new(0.25, 0.3, 0.35)) };
    let setup = |harness: &mut Harness| {
        harness.sun.direction = -Vec3::new(0.6, 0.5, 1.0).normalize();
        harness.ambient =
            AmbientLight { sky: Vec3::new(0.2, 0.25, 0.35), ground: Vec3::splat(0.05), intensity: 1.0 };
    };
    let mut peaks = Vec::new();
    for specular in [EnvironmentSpecular::Full, EnvironmentSpecular::Approximate, EnvironmentSpecular::Off] {
        let mut harness = Harness::with_specular(specular);
        setup(&mut harness);
        let frame = sphere_frame(&mut harness, &material, 3.0);
        let model = harness.model(&material, specular);
        let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
            model.shade(point, normal, frame.camera.position)
        });
        assert!(error < TOLERANCE, "{specular:?}: {error} at {at:?}");
        let half = (-harness.sun.direction.normalize() + Vec3::Z).normalize();
        let (x, y) = frame.pixel_of(half);
        peaks.push(frame.linear(x, y).x);
    }
    assert!(peaks[0] > peaks[2] + 0.1 && peaks[1] > peaks[2] + 0.1, "highlights on two tiers: {peaks:?}");

    // The clamp holds a highlight under a share of the light's radiance.
    let clamped = EnvironmentMaterial { specular_clamp: 0.05, ..material };
    let mut harness = Harness::exact();
    setup(&mut harness);
    let frame = sphere_frame(&mut harness, &clamped, 3.0);
    let model = harness.model(&clamped, EnvironmentSpecular::Full);
    let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
        model.shade(point, normal, frame.camera.position)
    });
    assert!(error < TOLERANCE, "clamped: {error} at {at:?}");

    // The mask map: AO and cavity darken the ambient light, cavity the highlights, and a zero
    // specular mask leaves the surface matte at every angle, on both tiers that reflect. Lit from
    // behind and to the side under a bright sky, where grazing reflection is strongest.
    let mask = [0.9, 0.85, 0.0, 0.7];
    let masked = EnvironmentMaterial { mask_map: Some(id(MASK)), ..material };
    for specular in [EnvironmentSpecular::Full, EnvironmentSpecular::Approximate] {
        let mut harness = Harness::with_specular(specular);
        harness.sun.direction = -Vec3::new(-0.8, 0.4, -0.45).normalize();
        harness.ambient =
            AmbientLight { sky: Vec3::new(0.4, 0.5, 0.7), ground: Vec3::splat(0.1), intensity: 1.0 };
        let map = half_texture(&harness.gpu, 4, 4, |_, _| mask);
        let handle = harness.environment(&masked, &[(MASK, &map)]);
        let sphere_mesh = harness.mesh(&mesh(vec![sphere(Vec3::ZERO, 1.0)]), &[handle]);
        harness.frame_sphere(1.0, 3.0);
        harness.add(sphere_mesh, Mat4::IDENTITY);
        let frame = harness.render();
        let model = Model { mask, ..harness.model(&masked, specular) };
        let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
            model.shade(point, normal, frame.camera.position)
        });
        assert!(error < TOLERANCE, "masked {specular:?}: {error} at {at:?}");
        let matte = Model { mask, specular: EnvironmentSpecular::Off, ..model };
        let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
            matte.shade(point, normal, frame.camera.position)
        });
        assert!(error < TOLERANCE, "matte under a zero mask, {specular:?}: {error} at {at:?}");
    }

    // Off, a metal shades as diffuse in its own color.
    let metal = EnvironmentMaterial { metallic: 1.0, ..material };
    let mut harness = Harness::with_specular(EnvironmentSpecular::Off);
    setup(&mut harness);
    let frame = sphere_frame(&mut harness, &metal, 3.0);
    let model = harness.model(&metal, EnvironmentSpecular::Off);
    let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), |point, normal| {
        model.shade(point, normal, frame.camera.position)
    });
    assert!(error < TOLERANCE, "a metal without specular: {error} at {at:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn emission_adds_its_light_on_either_side() {
    let mut harness = Harness::exact();
    harness.sun.direction = Vec3::Z;
    let emission = half_texture(&harness.gpu, 4, 4, |_, _| [1.0, 0.5, 1.0, 1.0]);
    let material = EnvironmentMaterial {
        emission: [0.3, 0.6, 0.2],
        emission_map: Some(id(EMISSION)),
        // Distance stylization greys the albedo, never the emission.
        distance: DistanceStylization { start: 0.0, end: 1.0, desaturation: 1.0 },
        ..plain(Vec3::new(0.5, 0.2, 0.2))
    };
    let handle = harness.environment(&material, &[(EMISSION, &emission)]);
    let mesh = harness.mesh(&mesh(vec![square(Vec3::ZERO, Vec3::Z, Vec3::X, 0.5)]), &[handle]);
    harness.camera = CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.6, 1.0, 0.05);
    harness.add(mesh, Mat4::IDENTITY);
    // Lit from behind with nothing kept in shadow and no ambient light: emission alone.
    let dark = harness.render();
    let expected = Vec3::new(0.3, 0.3, 0.2);
    let (x, y) = dark.pixel_of(Vec3::ZERO);
    assert!((dark.linear(x, y) - expected).abs().max_element() < TOLERANCE, "{}", dark.linear(x, y));
    // Lit from the front, on top of the (greyed) albedo's light.
    harness.sun.direction = Vec3::NEG_Z;
    let lit = harness.render();
    let grey = Vec3::splat(Vec3::new(0.5, 0.2, 0.2).dot(LUMINANCE));
    assert!((lit.linear(x, y) - (grey + expected)).abs().max_element() < TOLERANCE, "{}", lit.linear(x, y));
}

/// Time of day's emission scale (M1 W1's keyed emission): a lantern's glow follows it, a sign's
/// stays steady, and one halfway between keeps half, each over the same lit albedo.
#[test]
#[ignore = "needs a GPU"]
fn time_of_day_scales_emission_but_for_its_steady_share() {
    let mut harness = Harness::exact();
    let steady = [0.0, 0.5, 1.0];
    let materials = steady.map(|emission_steady| EnvironmentMaterial {
        emission: [0.3, 0.25, 0.15],
        emission_steady,
        ..plain(Vec3::splat(0.2))
    });
    let handles = materials.clone().map(|material| harness.environment(&material, &[]));
    // Three squares side by side, facing the camera and the key light.
    let centers = [-0.6, 0.0, 0.6].map(|x| Vec3::new(x, 0.0, 0.0));
    let squares = centers.map(|center| square(center, Vec3::Z, Vec3::X, 0.25));
    let mesh = harness.mesh(&mesh(squares.to_vec()), &handles);
    harness.sun.direction = Vec3::NEG_Z;
    harness.camera = CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.6, 1.0, 0.05);
    harness.add(mesh, Mat4::IDENTITY);
    // As authored, a tenth of it (by day), none, and twice it (a night's boost).
    for scale in [1.0, 0.1, 0.0, 2.0] {
        harness.emission_scale = scale;
        let frame = harness.render();
        for ((center, material), steady) in centers.iter().zip(&materials).zip(steady) {
            let shown = scale + (1.0 - scale) * steady;
            let expected = Vec3::splat(0.2) + Vec3::from(material.emission) * shown;
            let (x, y) = frame.pixel_of(*center);
            let found = frame.linear(x, y);
            assert!(
                (found - expected).abs().max_element() < TOLERANCE,
                "×{scale}, {steady} of it steady: {found}, expected {expected}"
            );
            let model = harness.model(material, EnvironmentSpecular::Full);
            let modeled = model.shade(*center, Vec3::Z, frame.camera.position);
            assert!((modeled - expected).abs().max_element() < 1e-5, "the model: {modeled} vs {expected}");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn local_lights_light_the_world_through_both_list_paths() {
    // A floor seen from above, lit by two lamps and no sun.
    let material =
        EnvironmentMaterial { wrap: 0.3, roughness: 0.6, specular_clamp: 0.5, ..plain(Vec3::splat(0.6)) };
    // Under the tone curve's shoulder however they stack, highlights included.
    let lamps = [
        LocalLight {
            position: Vec3::new(0.35, 0.3, 0.1),
            color: Vec3::new(1.0, 0.6, 0.3),
            intensity: 0.8,
            range: 1.2,
            kind: LightKind::Point,
        },
        LocalLight {
            position: Vec3::new(-0.4, 0.25, -0.2),
            color: Vec3::new(0.3, 0.6, 1.0),
            intensity: 0.8,
            range: 0.9,
            kind: LightKind::Point,
        },
    ];
    let mut frames = Vec::new();
    for light_path in [LightPath::Clustered, LightPath::PerObject] {
        let mut harness =
            Harness::new(RenderSettings { msaa: Msaa::Off, light_path, ..RenderSettings::default() });
        harness.sun.intensity = 0.0;
        harness.lights = lamps.to_vec();
        let handle = harness.environment(&material, &[]);
        let floor = square(Vec3::ZERO, Vec3::Y, Vec3::X, 1.0);
        let mesh = harness.mesh(&mesh(vec![floor]), &[handle]);
        harness.camera = CameraData::look_at(Vec3::new(0.0, 3.0, 0.01), Vec3::ZERO, Vec3::Y, 0.75, 1.0, 0.05);
        harness.add(mesh, Mat4::IDENTITY);
        frames.push(harness.render());
    }
    // Each lamp's light on the floor, as the shader lights it.
    let lit = |point: Vec3, normal: Vec3, eye: Vec3| {
        let v = (eye - point).normalize();
        let surface = Surface {
            n: normal,
            v,
            f0: Vec3::splat(0.04),
            roughness: 0.6,
            occlusion: 1.0,
            cavity: 1.0,
            specular: EnvironmentSpecular::Full,
            clamp: 0.5,
        };
        let width = (material.terminator.width + material.terminator.growth * point.distance(eye)).max(1e-5);
        lamps.iter().fold(Vec3::ZERO, |sum, lamp| {
            let to_light = lamp.position - point;
            let window = (1.0 - to_light.length_squared() / (lamp.range * lamp.range)).max(0.0).powi(2);
            let l = to_light.normalize();
            let n_dot_l = normal.dot(l);
            // The lamps are above the floor, so its edge never shows: N·L stays past a pixel's
            // change of it.
            let split = smoothstep(-width, width, n_dot_l);
            let radiance = lamp.color * lamp.intensity * window;
            sum + radiance * (0.6 * split * wrapped(n_dot_l, material.wrap))
                + radiance * surface.specular(l, n_dot_l) * split
        })
    };
    for frame in &frames {
        let (error, at, compared) = frame
            .compare(hit_rectangle(Vec3::ZERO, Vec3::Y, Vec3::X, 0.95), |point, normal| {
                lit(point, normal, frame.camera.position)
            });
        assert!(compared > 8000 && error < TOLERANCE, "{error} at {at:?} over {compared}");
    }
    let differences = frames[0].pixels.iter().zip(&frames[1].pixels).filter(|(a, b)| a != b).count();
    assert!(differences < 64, "the two paths draw the same: {differences} pixels differ");

    // A lamp beside a sphere: its light splits at the same terminator as the key light's, a
    // colored step where wrap keeps the lit side bright to the edge, and nothing past it.
    let material = EnvironmentMaterial {
        wrap: 0.5,
        terminator: Terminator { width: 0.08, growth: 0.0, band: None },
        ..plain(Vec3::splat(0.7))
    };
    let lamp = LocalLight {
        position: Vec3::new(2.0, 0.0, 0.0),
        color: Vec3::new(1.0, 0.8, 0.6),
        intensity: 1.0,
        range: 4.0,
        kind: LightKind::Point,
    };
    let mut harness = Harness::exact();
    harness.sun.intensity = 0.0;
    harness.lights = vec![lamp];
    let frame = sphere_frame(&mut harness, &material, 3.0);
    let lit = |point: Vec3, normal: Vec3| {
        let to_light = lamp.position - point;
        let window = (1.0 - to_light.length_squared() / (lamp.range * lamp.range)).max(0.0).powi(2);
        let n_dot_l = normal.dot(to_light.normalize());
        let split = smoothstep(-0.08, 0.08, n_dot_l);
        lamp.color * lamp.intensity * window * 0.7 * split * wrapped(n_dot_l, 0.5)
    };
    let (error, at, _) = frame.compare(hit_sphere(Vec3::ZERO, 1.0), lit);
    assert!(error < TOLERANCE, "a lamp's terminator: {error} at {at:?}");
}

/// A flat anime character material: lit everywhere, no rim and no outline.
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

#[test]
#[ignore = "needs a GPU"]
fn families_mix_on_meshes_and_draw_through_their_own_pipelines() {
    let mut harness = Harness::exact();
    harness.sun.direction = Vec3::new(0.0, 0.0, -1.0);
    harness.camera = CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.75, 1.0, 0.05);
    let red = harness.character(&flat_character(Vec3::new(0.7, 0.1, 0.1)));
    let green = harness.environment(&plain(Vec3::new(0.1, 0.6, 0.1)), &[]);
    let blue = harness.environment(&plain(Vec3::new(0.1, 0.1, 0.7)), &[]);
    // One mesh drawing both families, and a second environment mesh: instances alternate.
    let mixed = mesh(vec![
        square(Vec3::new(-0.5, 0.4, 0.0), Vec3::Z, Vec3::X, 0.3),
        square(Vec3::new(0.5, 0.4, 0.0), Vec3::Z, Vec3::X, 0.3),
    ]);
    let mixed = harness.mesh(&mixed, &[red, green]);
    let world = harness.mesh(&mesh(vec![square(Vec3::new(0.0, -0.5, 0.0), Vec3::Z, Vec3::X, 0.3)]), &[blue]);
    harness.add(world, Mat4::IDENTITY);
    harness.add(mixed, Mat4::IDENTITY);
    let frame = harness.render();
    let at = |point: Vec3| {
        let (x, y) = frame.pixel_of(point);
        frame.linear(x, y)
    };
    // Each draws through its own shader: the anime character's lit tone and the environment's
    // full key light.
    for (point, expected) in [
        (Vec3::new(-0.5, 0.4, 0.0), Vec3::new(0.7, 0.1, 0.1)),
        (Vec3::new(0.5, 0.4, 0.0), Vec3::new(0.1, 0.6, 0.1)),
        (Vec3::new(0.0, -0.5, 0.0), Vec3::new(0.1, 0.1, 0.7)),
    ] {
        assert!(
            (at(point) - expected).abs().max_element() < TOLERANCE,
            "{point}: {} against {expected}",
            at(point)
        );
    }
    let stats = harness.renderer.stats();
    assert_eq!((stats.draws, stats.environment_draws, stats.outline_draws), (3, 2, 0));
    // The f32 format's opaque anime character surface and outline, and one environment surface.
    let built = (stats.character_pipelines, stats.environment_pipelines);
    let builds = stats.draw_list_builds;

    // An edit that alpha-tests the green square moves its draw to a pipeline of its own, built
    // once, with one draw-list rebuild; texels under the cutoff go.
    let cut = EnvironmentMaterial {
        base_color: [0.1, 0.6, 0.1, 0.3],
        alpha: AlphaMode::Mask { cutoff: 0.5 },
        ..plain(Vec3::new(0.1, 0.6, 0.1))
    };
    harness.update(green, &cut);
    let frame = harness.render();
    let stats = harness.renderer.stats();
    // Built for both formats drawn so far: the quantized one every renderer starts with, and f32.
    assert_eq!(stats.environment_pipelines, built.1 + 2);
    assert_eq!((stats.character_pipelines, stats.draw_list_builds), (built.0, builds + 1));
    assert_eq!((stats.draws, stats.alpha_tested_draws, stats.environment_draws), (3, 1, 2));
    let (x, y) = frame.pixel_of(Vec3::new(0.5, 0.4, 0.0));
    assert!(frame.linear(x, y).max_element() < 0.01, "cut out: {}", frame.linear(x, y));

    // A transparent environment material blends over the sky.
    let glass = EnvironmentMaterial {
        base_color: [0.1, 0.1, 0.7, 0.5],
        alpha: AlphaMode::Blend,
        ..plain(Vec3::new(0.1, 0.1, 0.7))
    };
    harness.update(blue, &glass);
    let frame = harness.render();
    let (x, y) = frame.pixel_of(Vec3::new(0.0, -0.5, 0.0));
    let expected = Vec3::new(0.1, 0.1, 0.7) * 0.5;
    assert!((frame.linear(x, y) - expected).abs().max_element() < TOLERANCE, "{}", frame.linear(x, y));
    assert_eq!(harness.renderer.stats().transparent_draws, 1);

    // Families without mesh shaders are refused, whole.
    let sky = Material::read(&cook(&SkyMaterial::default(), &[])).map(|material| {
        harness.renderer.upload_material(material, |_| None).expect("a sky material uploads")
    });
    let sky = sky.expect("read");
    let quad = mesh(vec![square(Vec3::ZERO, Vec3::Z, Vec3::X, 0.3)]);
    let bytes = quad.to_cooked().expect("cook");
    match harness.renderer.upload_mesh(Mesh::read(&bytes).expect("valid"), &[sky]) {
        Err(RenderError::NotDrawable(family)) => assert_eq!(family.name(), "sky"),
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore = "needs a GPU"]
fn the_prepass_holds_the_world_and_temporal_aa_converges_on_it() {
    let settings = RenderSettings { msaa: Msaa::Off, temporal: true, ..RenderSettings::default() };
    let mut harness = Harness::new(settings);
    harness.sun.direction = -Vec3::new(1.0, 0.6, 0.8).normalize();
    harness.shadow_tint = Vec3::new(0.3, 0.3, 0.5);
    let material = EnvironmentMaterial {
        wrap: 0.4,
        roughness: 0.35,
        specular_clamp: 1.0,
        terminator: Terminator { width: 0.04, growth: 0.02, band: None },
        ..plain(Vec3::new(0.6, 0.5, 0.4))
    };
    let handle = harness.environment(&material, &[]);
    let scene = mesh(vec![sphere(Vec3::new(0.0, 0.5, 0.0), 0.5), square(Vec3::ZERO, Vec3::Y, Vec3::X, 2.0)]);
    let mesh = harness.mesh(&scene, &[handle, handle]);
    harness.camera =
        CameraData::look_at(Vec3::new(0.0, 1.6, 3.0), Vec3::new(0.0, 0.3, 0.0), Vec3::Y, 0.8, 1.0, 0.05);
    let instance = harness.add(mesh, Mat4::IDENTITY);
    // A morphed prop in environment materials (a flag, a swinging sign): drawn from its skinning
    // outputs like a character, but no character.
    let mut prop = self::mesh(vec![sphere(Vec3::new(0.9, 0.3, 0.6), 0.25)]);
    prop.morph_targets =
        vec![MorphTarget { name: "still".into(), default_weight: 0.0, first_delta: 0, delta_count: 1 }];
    prop.morph_deltas = vec![MorphDelta::new(0, [0.0; 3], [0.0; 3])];
    let prop = harness.mesh(&prop, &[handle]);
    harness.add(prop, Mat4::IDENTITY);

    // The scene prepass holds the world's depth, the prop's included, and no motion while it
    // holds still; the character prepass holds nothing.
    let readback = || Readback::new(&harness.gpu, (SIDE, SIDE), 4);
    let (depths, velocities, characters) = (readback(), readback(), readback());
    let reads: [Read<'_>; 3] =
        [(|t| t.depth, &depths), (|t| t.velocity, &velocities), (|t| t.character_depth, &characters)];
    let frame = harness.render_reading(&reads);
    let read = [depths.texels(&harness.gpu), velocities.texels(&harness.gpu)];
    assert!(characters.texels(&harness.gpu).iter().all(|texel| depth(texel) == 0.0), "no characters");
    let (x, y) = frame.pixel_of(Vec3::new(0.9, 0.3, 0.85));
    assert!(depth(&read[0][(y * SIDE + x) as usize]) > 0.0, "the prop is in the scene prepass");
    let covered: Vec<bool> = read[0].iter().map(|texel| depth(texel) > 0.0).collect();
    let drawn: Vec<bool> = frame.pixels.iter().map(|pixel| pixel[..3] != [0, 0, 0]).collect();
    let count = |pixels: &[bool]| pixels.iter().filter(|&&set| set).count();
    assert!(count(&covered) > 6000, "the prepass draws the world: {} pixels", count(&covered));
    // Temporal AA's filter spreads the drawn edge by a pixel over the black sky; nothing more.
    assert!(covered.iter().zip(&drawn).all(|(&covered, &drawn)| !covered || drawn), "drawn where covered");
    assert!(count(&drawn) - count(&covered) < 400, "{} drawn, {} covered", count(&drawn), count(&covered));
    assert!(read[1].iter().all(|texel| rg16f(texel) == [0.0, 0.0]), "still: no velocity");

    // Converged, the frame holds still under the jitter.
    let mut frames = Vec::new();
    for _ in 0..48 {
        frames.push(harness.render().pixels);
    }
    let (last, before) = (&frames[47], &frames[46]);
    let differences: Vec<u8> = last
        .iter()
        .zip(before)
        .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0))
        .collect();
    let mean = differences.iter().map(|&d| f64::from(d)).sum::<f64>() / differences.len() as f64;
    let over = differences.iter().filter(|&&d| d > 2).count();
    assert!(mean < 0.05 && over < 16, "frame to frame: mean {mean:.3} steps, {over} pixels over 2");

    // Moved, the world's velocity is its motion on screen.
    harness.added.push(MeshInstance {
        id: instance,
        mesh,
        transform: Mat4::from_translation(Vec3::new(0.1, 0.0, 0.0)),
    });
    let frame = harness.render_reading(&reads[..2]);
    let read = [depths.texels(&harness.gpu), velocities.texels(&harness.gpu)];
    let center = frame.pixel_of(Vec3::new(0.1, 0.5, 0.0));
    let velocity = rg16f(&read[1][(center.1 * SIDE + center.0) as usize]);
    let camera = frame.camera;
    let projected = |point: Vec3| {
        let ndc = (camera.proj * camera.view).project_point3(point);
        Vec3::new(ndc.x * 0.5, -ndc.y * 0.5, 0.0)
    };
    let expected = projected(Vec3::new(0.1, 0.5, 0.5)) - projected(Vec3::new(0.0, 0.5, 0.5));
    assert!(
        (velocity[0] - expected.x).abs() < 0.003 && (velocity[1] - expected.y).abs() < 0.003,
        "{velocity:?} against {expected}"
    );
}

#[test]
#[ignore = "needs a GPU"]
fn an_environment_shader_edit_rebuilds_its_pipelines_alone() {
    let mut harness = Harness::exact();
    harness.sun.direction = Vec3::NEG_Z;
    harness.camera = CameraData::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y, 0.75, 1.0, 0.05);
    let red = harness.character(&flat_character(Vec3::new(0.7, 0.1, 0.1)));
    let world = harness.environment(&plain(Vec3::new(0.6, 0.3, 0.1)), &[]);
    let both = mesh(vec![
        square(Vec3::new(-0.5, 0.0, 0.0), Vec3::Z, Vec3::X, 0.3),
        square(Vec3::new(0.5, 0.0, 0.0), Vec3::Z, Vec3::X, 0.3),
    ]);
    let both = harness.mesh(&both, &[red, world]);
    harness.add(both, Mat4::IDENTITY);
    let before = harness.render();
    let environment_pipelines = harness.renderer.stats().environment_pipelines;

    let swapped = {
        let mut sources = ShaderSources::compiled_in();
        let text = ShaderFile::Environment.compiled_in();
        let old = "return vec4<f32>(final_color, select(1.0, base.a, PERMUTATION_TRANSPARENT));";
        assert!(text.contains(old));
        let new = "return vec4<f32>(final_color.bgr, select(1.0, base.a, PERMUTATION_TRANSPARENT));";
        sources.set(ShaderFile::Environment, text.replace(old, new));
        sources
    };
    let update = harness.renderer.set_shader_sources(&swapped).expect("a valid edit");
    assert_eq!((update.programs, update.pipelines), (1, environment_pipelines));
    let after = harness.render();
    let at = |frame: &Frame, x: f32| {
        let (px, py) = frame.pixel_of(Vec3::new(x, 0.0, 0.0));
        frame.pixels[(py * SIDE + px) as usize]
    };
    let [r, g, b, a] = at(&before, 0.5);
    assert_eq!(at(&after, 0.5), [b, g, r, a], "the world's red and blue swapped");
    assert_eq!(at(&after, -0.5), at(&before, -0.5), "the character as it was");

    harness.renderer.set_shader_sources(&ShaderSources::compiled_in()).expect("compiled in");
    assert_eq!(harness.render().pixels, before.pixels);
}
