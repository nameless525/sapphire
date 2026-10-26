//! Hot reload in the renderer (M1 A8): new shader sources rebuild pipelines, broken ones keep
//! the pipelines the renderer has, and meshes, materials and textures are replaced in place.
//! They need a GPU: run with `cargo test -p sapphire_render --tests -- --ignored`.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};
use sapphire_assets::AssetId;
use sapphire_assets::material::{
    AlphaMode, AnimeCharacterMaterial, CharacterRegion, CharacterSurface, FaceShading, LightSplit, Material,
    Rim,
};
use sapphire_assets::mesh::{
    MaterialSlot, Mesh, PositionFormat, SlotMaterial, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};
use sapphire_assets::texture::{Swizzle, Texture, TextureDesc, TextureFormat};
use sapphire_render::graph::{FrameGraph, GraphExecutor};
use sapphire_render::texture::GpuTexture;
use sapphire_render::{
    BloomParams, CameraData, ColorGrade, DirectionalLight, FramePacketRing, InstanceId, MeshHandle,
    MeshInstance, PostParams, RenderError, RenderSettings, Renderer, ShaderError, ShaderFile, ShaderSources,
    ShaderUpdate, SkyGradient, ToneCurve, ViewData, Vignette,
};
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: (u32, u32) = (64, 64);

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

/// A UV sphere of `radius` at the origin, its one slot `material`.
fn sphere(radius: f32, material: SlotMaterial) -> Mesh {
    const RINGS: u32 = 24;
    const SEGMENTS: u32 = 32;
    let mut vertices = Vec::new();
    for ring in 0..=RINGS {
        let theta = PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = TAU * segment as f32 / SEGMENTS as f32;
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
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![MaterialSlot { name: "sphere".into(), material }];
    Mesh::from_vertices(&vertices, indices, submeshes, slots, PositionFormat::Unorm16)
}

/// Fully lit in `rgb`, through `base_color_map` if any, with no rim or outline.
fn flat(rgb: [f32; 3], base_color_map: Option<AssetId>) -> AnimeCharacterMaterial {
    AnimeCharacterMaterial {
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        base_color_map,
        light_split: LightSplit { threshold: -1.0, softness: 0.02 },
        regions: vec![CharacterRegion { shade_color: [1.0; 3], ..CharacterRegion::default() }],
        rim: Rim { strength: 0.0, ..Rim::default() },
        ..AnimeCharacterMaterial::default()
    }
}

fn cooked(material: &AnimeCharacterMaterial) -> Material {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    Material::from_document(material, &swizzles).expect("a valid material")
}

/// A 4×4 RGBA8 sRGB texture of one color.
fn texture(gpu: &GpuContext, rgba: [u8; 4]) -> GpuTexture {
    let texels: Vec<u8> = rgba.repeat(16);
    let desc = TextureDesc {
        format: TextureFormat::Rgba8UnormSrgb,
        width: 4,
        height: 4,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    GpuTexture::upload(gpu, &Texture { desc, levels: vec![&texels] }, "test").expect("upload")
}

/// A tone curve linear from black, a neutral grade, no bloom or vignette: pixels show linear
/// values below about 0.98 as they are.
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

/// A renderer drawing into a 64×64 target, and what its frames need.
struct Frames {
    renderer: Renderer,
    graph: GraphExecutor,
    packets: FramePacketRing,
    target: wgpu::Texture,
    added: Vec<MeshInstance>,
}

impl Frames {
    fn new(gpu: &GpuContext) -> Self {
        let target = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self {
            renderer: Renderer::new(gpu, FORMAT, RenderSettings::default()).expect("renderer"),
            graph: GraphExecutor::new(),
            packets: FramePacketRing::new(),
            target,
            added: Vec::new(),
        }
    }

    /// Draws `mesh` at the origin from the next frame.
    fn show(&mut self, mesh: MeshHandle) {
        self.added.push(MeshInstance { id: InstanceId(1), mesh, transform: Mat4::IDENTITY });
    }

    /// Renders a frame, the unit sphere filling half the height, and reads it back; fails on any
    /// validation error.
    fn render(&mut self, gpu: &GpuContext) -> Vec<[u8; 4]> {
        let packet = self.packets.begin();
        let fov = 40f32.to_radians();
        let distance = 1.0 / (0.5 * (fov * 0.5).tan());
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, distance), Vec3::ZERO, Vec3::Y, fov, 1.0, 0.05);
        packet.views.push(ViewData::new(camera));
        packet.sun = DirectionalLight { direction: Vec3::NEG_Z, color: Vec3::ONE, intensity: 1.0 };
        packet.sky =
            SkyGradient { zenith: Vec3::splat(0.5), horizon: Vec3::splat(0.5), ground: Vec3::splat(0.5) };
        packet.post = passthrough();
        packet.instances.changed.append(&mut self.added);
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        self.renderer.prepare(gpu, self.packets.current());
        let view = self.target.create_view(&Default::default());
        let mut graph = FrameGraph::new();
        let target = graph.import_texture("target", &view);
        self.renderer.record(&mut graph, target).expect("a view and a target");
        self.graph.execute(gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        read_back(gpu, &self.target)
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

/// The pixel at `(x, y)`.
fn at(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE.0 + x) as usize]
}

/// The middle of the image: the sphere.
fn center(pixels: &[[u8; 4]]) -> [u8; 4] {
    at(pixels, SIZE.0 / 2, SIZE.1 / 2)
}

/// A corner: the sky.
fn corner(pixels: &[[u8; 4]]) -> [u8; 4] {
    at(pixels, 1, 1)
}

/// `sources` with `old` replaced by `new` in `file`, which must hold it once.
fn edited(sources: &ShaderSources, file: ShaderFile, old: &str, new: &str) -> ShaderSources {
    let text = sources.get(file);
    assert_eq!(text.matches(old).count(), 1, "{old:?} in {file}");
    let mut edited = sources.clone();
    edited.set(file, text.replace(old, new));
    edited
}

/// The 1-based line of `needle` in `text`.
fn line_of(text: &str, needle: &str) -> u32 {
    text[..text.find(needle).expect("present")].matches('\n').count() as u32 + 1
}

/// Uploads `mesh` with the materials it embeds, as the runtime does.
fn upload(renderer: &mut Renderer, mesh: &Mesh) -> MeshHandle {
    let bytes = mesh.to_cooked().expect("cook");
    let mesh = Mesh::read(&bytes).expect("a valid mesh");
    let materials = renderer.upload_embedded_materials(mesh).expect("materials");
    renderer.upload_mesh(mesh, &materials).expect("upload")
}

const SKY_RETURN: &str = "return vec4<f32>(color, 1.0);";
const CHARACTER_RETURN: &str = "return vec4<f32>(color, select(1.0, base.a, PERMUTATION_TRANSPARENT));";

#[test]
#[ignore = "needs a GPU"]
fn a_shader_edit_rebuilds_the_pipelines_built_from_it() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let red = SlotMaterial::Embedded(cooked(&flat([0.8, 0.0, 0.0], None)));
    let mesh = upload(&mut frames.renderer, &sphere(1.0, red));
    frames.show(mesh);
    let before = frames.render(&gpu);
    let (sphere_red, sky) = (center(&before), corner(&before));
    assert!(sphere_red[0] > 200 && sphere_red[2] < 10, "{sphere_red:?}");
    let sources = frames.renderer.shader_sources().clone();
    assert!(sources.is_compiled_in());

    // The sky's program alone: its six pipelines (four layer variants into the scene, two into
    // the sky cube).
    let blue_sky = edited(&sources, ShaderFile::Sky, SKY_RETURN, "return vec4<f32>(0.0, 0.0, 0.5, 1.0);");
    let update = frames.renderer.set_shader_sources(&blue_sky).expect("a valid edit");
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 6 });
    let pixels = frames.render(&gpu);
    assert_eq!(center(&pixels), sphere_red, "the sphere's pipelines are untouched");
    let new_sky = corner(&pixels);
    assert!(new_sky[0] == 0 && new_sky[1] == 0 && new_sky[2] > 150, "{new_sky:?} (was {sky:?})");

    // The anime character shader: its opaque pipeline for quantized meshes, the one built.
    let swapped = edited(
        &blue_sky,
        ShaderFile::AnimeCharacter,
        CHARACTER_RETURN,
        "return vec4<f32>(color.bgr, select(1.0, base.a, PERMUTATION_TRANSPARENT));",
    );
    let update = frames.renderer.set_shader_sources(&swapped).expect("a valid edit");
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 1 });
    let pixels = frames.render(&gpu);
    assert_eq!(center(&pixels), [sphere_red[2], sphere_red[1], sphere_red[0], 255], "red and blue swapped");
    assert_eq!(corner(&pixels), new_sky);

    // A shared file: every program that includes it, and every pipeline built from them (the
    // sky's six, surface, outline, scene prepass, light clusters).
    let mut commented = swapped.clone();
    commented.set(ShaderFile::Frame, format!("{}\n// edited\n", swapped.get(ShaderFile::Frame)));
    let update = frames.renderer.set_shader_sources(&commented).expect("a valid edit");
    assert_eq!(update, ShaderUpdate { programs: 5, pipelines: 10 });
    assert_eq!(frames.render(&gpu), pixels, "a comment changes nothing drawn");
    assert_eq!(frames.renderer.set_shader_sources(&commented).unwrap(), ShaderUpdate::default());

    // Back to the compiled-in shaders.
    frames.renderer.set_shader_sources(&ShaderSources::compiled_in()).expect("compiled in");
    assert_eq!(frames.render(&gpu), before);
    assert_eq!(frames.renderer.stats().shader_rebuilds, 4);
}

#[test]
#[ignore = "needs a GPU"]
fn a_broken_shader_keeps_the_pipelines_it_has() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let red = SlotMaterial::Embedded(cooked(&flat([0.8, 0.0, 0.0], None)));
    let mesh = upload(&mut frames.renderer, &sphere(1.0, red));
    frames.show(mesh);
    let before = frames.render(&gpu);
    let sources = ShaderSources::compiled_in();

    // A syntax error, found by naga and placed in its own file and line.
    let anchor = "fn fs_anime_character(in: CharacterVaryings) -> @location(0) vec4<f32> {";
    let broken =
        edited(&sources, ShaderFile::AnimeCharacter, anchor, &format!("{anchor}\n    let broken = ;"));
    let line = line_of(broken.get(ShaderFile::AnimeCharacter), "let broken");
    match frames.renderer.set_shader_sources(&broken) {
        Err(ShaderError::Invalid { file: ShaderFile::AnimeCharacter, line: found, .. }) => {
            assert_eq!(found, line)
        }
        other => panic!("{other:?}"),
    }
    // A type error, found by validation.
    let mistyped = edited(&sources, ShaderFile::Sky, SKY_RETURN, "return sky_color(dir);");
    let line = line_of(mistyped.get(ShaderFile::Sky), "return sky_color(dir);");
    match frames.renderer.set_shader_sources(&mistyped) {
        Err(error @ ShaderError::Invalid { file: ShaderFile::Sky, .. }) => {
            let ShaderError::Invalid { line: found, .. } = error else { unreachable!() };
            assert!(found.abs_diff(line) <= 1, "line {found}, expected about {line}");
        }
        other => panic!("{other:?}"),
    }
    // Valid WGSL that wgpu refuses to build the renderer's pipelines from: no `fs_sky`.
    let renamed = edited(&sources, ShaderFile::Sky, "fn fs_sky(", "fn fs_sky_renamed(");
    let error = frames.renderer.set_shader_sources(&renamed).unwrap_err();
    assert!(matches!(error, ShaderError::Pipeline { .. }), "{error}");

    assert!(frames.renderer.shader_sources().is_compiled_in(), "nothing was applied");
    assert_eq!(frames.render(&gpu), before, "the old pipelines draw on");
    assert_eq!(frames.renderer.stats().shader_rebuilds, 0);
    // A fixed edit applies.
    let fixed = edited(&sources, ShaderFile::Sky, SKY_RETURN, "return vec4<f32>(0.0, 0.0, 0.5, 1.0);");
    assert_eq!(frames.renderer.set_shader_sources(&fixed).unwrap().programs, 1);
}

#[test]
#[ignore = "needs a GPU"]
fn meshes_materials_and_textures_are_replaced_in_place() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let texture_id = AssetId::from_bytes([1; 16]);
    let material_id = AssetId::from_bytes([2; 16]);
    // A white material through a red texture, bound to the mesh as an asset.
    let red = texture(&gpu, [255, 0, 0, 255]);
    let material_bytes = cooked(&flat([0.8; 3], Some(texture_id))).to_cooked().expect("cook");
    let material = Material::read(&material_bytes).expect("a valid material");
    let handle = frames.renderer.upload_material(material, |id| (id == texture_id).then_some(&red)).unwrap();
    let mesh_bytes = sphere(1.0, SlotMaterial::Asset(material_id)).to_cooked().expect("cook");
    let mesh = Mesh::read(&mesh_bytes).expect("a valid mesh");
    let resolve = |id: AssetId| (id == material_id).then_some(handle);
    let materials = frames.renderer.upload_slot_materials(mesh, resolve).unwrap();
    let mesh_handle = frames.renderer.upload_mesh(mesh, &materials).unwrap();
    frames.show(mesh_handle);
    let pixels = frames.render(&gpu);
    let lit = center(&pixels);
    assert!(lit[0] > 150 && lit[1] < 10, "{lit:?}");

    // A new texture: the material that binds it is rebound, uploading nothing else.
    let blue = texture(&gpu, [0, 0, 255, 255]);
    frames.renderer.update_material(handle, material, |id| (id == texture_id).then_some(&blue)).unwrap();
    let pixels = frames.render(&gpu);
    // Blue where red was. Not exactly red's channels swapped: the character light (its ambient
    // from the sky, C2) isn't neutral.
    let rebound = center(&pixels);
    assert!(rebound[2] > 150 && rebound[0] < 10 && rebound[2].abs_diff(lit[0]) <= 8, "{rebound:?} {lit:?}");
    assert_eq!(frames.renderer.stats().material_uploads, 1);

    // A new mesh under the same handle: half the radius, so a point at 3/4 of it is sky now.
    let stats = frames.renderer.stats();
    let edge = (SIZE.0 / 2 + SIZE.1 * 3 / 16, SIZE.1 / 2);
    assert_ne!(at(&pixels, edge.0, edge.1), corner(&pixels));
    let small = sphere(0.5, SlotMaterial::Asset(material_id)).to_cooked().expect("cook");
    frames.renderer.update_mesh(mesh_handle, Mesh::read(&small).unwrap(), resolve).unwrap();
    let pixels = frames.render(&gpu);
    assert_eq!(at(&pixels, edge.0, edge.1), corner(&pixels), "the instance draws the new mesh");
    assert_eq!(center(&pixels), rebound);
    // Replacing a mesh again and again reuses the pool's room: nothing grows.
    for radius in [1.0, 0.5, 1.0, 0.5] {
        let bytes = sphere(radius, SlotMaterial::Asset(material_id)).to_cooked().expect("cook");
        frames.renderer.update_mesh(mesh_handle, Mesh::read(&bytes).unwrap(), resolve).unwrap();
        frames.render(&gpu);
    }
    let after = frames.renderer.stats();
    assert_eq!((after.mesh_pool_bytes, after.buffer_growths), (stats.mesh_pool_bytes, stats.buffer_growths));

    // A mesh that embeds its material rewrites the old one's slot rather than taking a new one.
    let green = sphere(1.0, SlotMaterial::Embedded(cooked(&flat([0.0, 0.8, 0.0], None))));
    let embedded = upload(&mut frames.renderer, &green);
    let materials = frames.renderer.stats().materials;
    let yellow = sphere(1.0, SlotMaterial::Embedded(cooked(&flat([0.8, 0.8, 0.0], None))));
    let bytes = yellow.to_cooked().expect("cook");
    frames.renderer.update_mesh(embedded, Mesh::read(&bytes).unwrap(), |_| None).unwrap();
    assert_eq!(frames.renderer.stats().materials, materials);
    frames.added.push(MeshInstance { id: InstanceId(1), mesh: embedded, transform: Mat4::IDENTITY });
    let pixels = frames.render(&gpu);
    let yellow = center(&pixels);
    assert!(yellow[0] > 150 && yellow[1] > 150 && yellow[2] < 10, "{yellow:?}");

    // Failures change nothing: a slot nobody resolves, a handle of no mesh.
    let error = frames.renderer.update_mesh(mesh_handle, Mesh::read(&small).unwrap(), |_| None).unwrap_err();
    assert!(matches!(error, RenderError::UnresolvedMaterial { .. }), "{error}");
    let unknown = MeshHandle::from_raw(99);
    let error = frames.renderer.update_mesh(unknown, Mesh::read(&small).unwrap(), resolve).unwrap_err();
    assert!(matches!(error, RenderError::UnknownMesh(_)), "{error}");
    assert_eq!(frames.render(&gpu), pixels);
}

#[test]
#[ignore = "needs a GPU"]
fn a_pipeline_built_later_that_fails_goes_back_to_the_compiled_in_shaders() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    let red = SlotMaterial::Embedded(cooked(&flat([0.8, 0.0, 0.0], None)));
    let mesh = upload(&mut frames.renderer, &sphere(1.0, red));
    frames.show(mesh);
    frames.render(&gpu);
    // Good for the pipelines built so far (opaque surfaces), but an alpha-tested surface sets
    // the permutation constant this edit renames.
    let compiled_in = ShaderSources::compiled_in();
    let mut renamed = compiled_in.clone();
    let text =
        compiled_in.get(ShaderFile::AnimeCharacter).replace("PERMUTATION_ALPHA_TEST", "PERMUTATION_CUTOUT");
    renamed.set(ShaderFile::AnimeCharacter, text);
    assert_eq!(frames.renderer.set_shader_sources(&renamed).unwrap().programs, 1);
    let cutout =
        AnimeCharacterMaterial { alpha: AlphaMode::Mask { cutoff: 0.5 }, ..flat([0.0, 0.8, 0.0], None) };
    let cutout = upload(&mut frames.renderer, &sphere(1.0, SlotMaterial::Embedded(cooked(&cutout))));
    // The renderer went back to the compiled-in shaders rather than keep an invalid pipeline,
    // and draws the alpha-tested sphere without a validation error.
    assert!(frames.renderer.shader_sources().is_compiled_in());
    assert_eq!(frames.renderer.stats().shader_rebuilds, 2);
    frames.added.push(MeshInstance { id: InstanceId(1), mesh: cutout, transform: Mat4::IDENTITY });
    let green = center(&frames.render(&gpu));
    assert!(green[1] > 150 && green[0] < 10, "{green:?}");
}

#[test]
#[ignore = "needs a GPU"]
fn every_variant_built_is_rebuilt_on_the_layouts_it_had() {
    let gpu = gpu();
    let mut frames = Frames::new(&gpu);
    // Four spheres, one per surface variant built here: opaque, alpha-tested and transparent
    // bodies, and an opaque face (M1 C3) reading a shadow map.
    let sdf_id = AssetId::from_bytes([3; 16]);
    let desc = TextureDesc {
        format: TextureFormat::R8Unorm,
        width: 4,
        height: 4,
        mip_level_count: 1,
        swizzle: Swizzle::IDENTITY,
    };
    let sdf = GpuTexture::upload(&gpu, &Texture { desc, levels: vec![&[255; 16]] }, "sdf").expect("upload");
    let face = AnimeCharacterMaterial {
        surface: CharacterSurface::Face(FaceShading::new(sdf_id)),
        ..flat([0.8, 0.6, 0.5], None)
    };
    let bytes = cooked(&face).to_cooked().expect("cook");
    let face = Material::read(&bytes).expect("a valid material");
    let face = frames.renderer.upload_material(face, |id| (id == sdf_id).then_some(&sdf)).expect("a face");
    let body = |rgb, alpha| {
        let material = AnimeCharacterMaterial { alpha, ..flat(rgb, None) };
        sphere(0.4, SlotMaterial::Embedded(cooked(&material)))
    };
    let face_id = AssetId::from_bytes([4; 16]);
    let face_sphere = sphere(0.4, SlotMaterial::Asset(face_id)).to_cooked().expect("cook");
    let face_sphere = Mesh::read(&face_sphere).expect("a valid mesh");
    let materials =
        frames.renderer.upload_slot_materials(face_sphere, |id| (id == face_id).then_some(face)).unwrap();
    let meshes = [
        upload(&mut frames.renderer, &body([0.8, 0.1, 0.1], AlphaMode::Opaque)),
        upload(&mut frames.renderer, &body([0.1, 0.8, 0.1], AlphaMode::Mask { cutoff: 0.5 })),
        upload(&mut frames.renderer, &body([0.1, 0.1, 0.8], AlphaMode::Blend)),
        frames.renderer.upload_mesh(face_sphere, &materials).unwrap(),
    ];
    for (index, mesh) in meshes.into_iter().enumerate() {
        let x = if index % 2 == 0 { -0.5 } else { 0.5 };
        let y = if index < 2 { 0.5 } else { -0.5 };
        let transform = Mat4::from_translation(Vec3::new(x, y, 0.0));
        frames.added.push(MeshInstance { id: InstanceId(index as u64 + 1), mesh, transform });
    }
    let before = frames.render(&gpu);
    let stats = frames.renderer.stats();
    // Four surfaces, and the opaque and alpha-tested outlines (the face shares the opaque one).
    assert_eq!((stats.character_pipelines, stats.face_draws, stats.transparent_draws), (6, 1, 1));

    // Comments: each edit rebuilds every pipeline built from its program, and the frame draws
    // as before through them.
    let comment = |sources: &ShaderSources, file: ShaderFile| {
        let mut edited = sources.clone();
        edited.set(file, format!("{}\n// edited\n", sources.get(file)));
        edited
    };
    let character = comment(&ShaderSources::compiled_in(), ShaderFile::AnimeCharacter);
    let update = frames.renderer.set_shader_sources(&character).unwrap();
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 4 });
    assert_eq!(frames.render(&gpu), before);
    let outline = comment(&character, ShaderFile::Outline);
    let update = frames.renderer.set_shader_sources(&outline).unwrap();
    assert_eq!(update, ShaderUpdate { programs: 1, pipelines: 2 });
    assert_eq!(frames.render(&gpu), before);
    // The shared file: the sky's six, four surfaces, two outlines, the scene prepass and the
    // light clusters.
    let frame = comment(&outline, ShaderFile::Frame);
    let update = frames.renderer.set_shader_sources(&frame).unwrap();
    assert_eq!(update, ShaderUpdate { programs: 5, pipelines: 14 });
    assert_eq!(frames.render(&gpu), before);
    assert_eq!(frames.renderer.stats().character_pipelines, 6, "the same set, rebuilt");
}
