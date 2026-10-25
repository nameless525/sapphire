//! Skinning tests that read GPU buffers and depth back. They need a GPU: run them with
//! `cargo test -p sapphire_render --tests -- --ignored`.

use glam::{Affine3A, Mat4, Quat, Vec3};
use sapphire_assets::material::{AnimeCharacterMaterial, Material, Outline, Rim};
use sapphire_assets::mesh::{
    Influences, Joint, JointTransform, MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Skin,
    SkinBind, Submesh, Vertex, VertexInfluence, VertexShading, quantize_weights,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::reference;
use crate::graph::{FrameGraph, GraphExecutor, TextureId};
use crate::{
    CameraData, FramePacketRing, InstanceId, MeshHandle, MeshInstance, Msaa, RenderSettings, Renderer,
    ViewData,
};

/// `material` embedded in a mesh's material slot, as the glTF importer embeds one.
fn slot(material: &AnimeCharacterMaterial) -> MaterialSlot {
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    MaterialSlot::embedded(
        "material",
        Material::from_document(material, &swizzles).expect("a valid material"),
    )
}

/// Cooks `mesh` and uploads it the way the runtime loads one, with the materials it embeds.
fn upload(renderer: &mut Renderer, mesh: &Mesh) -> MeshHandle {
    let bytes = mesh.to_cooked().expect("cook");
    let mesh = Mesh::read(&bytes).expect("valid");
    let materials = renderer.upload_embedded_materials(mesh).expect("materials");
    renderer.upload_mesh(mesh, &materials).expect("upload")
}

/// A deterministic generator (PCG-style LCG), so failures reproduce.
struct Rng(u64);

impl Rng {
    /// Uniform in [0, 1).
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.next()
    }

    fn index(&mut self, len: usize) -> usize {
        ((self.next() * len as f32) as usize).min(len - 1)
    }

    fn direction(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0));
            if (0.01..=1.0).contains(&v.length_squared()) {
                return v.normalize();
            }
        }
    }

    /// A rotation, a uniform scale in [0.5, 2] and a translation within a meter.
    fn affine(&mut self) -> Affine3A {
        let rotation = Quat::from_axis_angle(self.direction(), self.range(0.0, std::f32::consts::TAU));
        let translation = Vec3::new(self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0));
        Affine3A::from_scale_rotation_translation(Vec3::splat(self.range(0.5, 2.0)), rotation, translation)
    }
}

/// A mesh of `vertices` random vertices (as one triangle list, which the skinning pass doesn't
/// care about), skinned to a palette of `palette` entries with one to four random influences each,
/// and with three sparse morph targets.
fn random_mesh(rng: &mut Rng, vertices: usize, palette: usize, format: PositionFormat) -> Mesh {
    let vertex_data: Vec<Vertex> = (0..vertices)
        .map(|_| {
            let normal = rng.direction();
            Vertex {
                position: [rng.range(-1.0, 1.0), rng.range(0.0, 2.0), rng.range(-0.5, 0.5)],
                normal: normal.to_array(),
                outline_normal: (normal + 0.3 * rng.direction()).normalize().to_array(),
                uv: [rng.next(), rng.next()],
                color: [255; 4],
            }
        })
        .collect();
    let indices: Vec<u32> = (0..(vertices as u32 / 3) * 3).collect();
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let slots = vec![slot(&AnimeCharacterMaterial::default())];
    let mut mesh = Mesh::from_vertices(&vertex_data, indices, submeshes, slots, format);
    let influences: Vec<VertexInfluence> = (0..vertices)
        .map(|_| {
            let count = 1 + rng.index(4);
            let mut binds = [0u16; 4];
            let mut weights = [0.0f32; 4];
            for slot in 0..count {
                binds[slot] = rng.index(palette) as u16;
                weights[slot] = rng.range(0.05, 1.0);
            }
            VertexInfluence { binds, weights: quantize_weights(weights) }
        })
        .collect();
    mesh.skin = Some(Skin {
        joints: vec![Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY }],
        binds: (0..palette)
            .map(|_| SkinBind { joint: 0, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
            .collect(),
        influences: Influences::pack(&influences, palette),
    });
    for target in 0..3 {
        let first_delta = mesh.morph_deltas.len() as u32;
        // About a third of the vertices, in increasing order.
        for vertex in 0..vertices as u32 {
            if rng.next() >= 0.35 {
                continue;
            }
            let position = (rng.direction() * rng.range(0.0, 0.2)).to_array();
            mesh.morph_deltas.push(MorphDelta::new(vertex, position, (rng.direction() * 0.3).to_array()));
        }
        let delta_count = mesh.morph_deltas.len() as u32 - first_delta;
        mesh.morph_targets.push(MorphTarget {
            name: format!("target {target}"),
            default_weight: 0.0,
            first_delta,
            delta_count,
        });
    }
    mesh
}

fn gpu() -> GpuContext {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device")
}

fn target(gpu: &GpuContext, (width, height): (u32, u32)) -> wgpu::Texture {
    gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn readback(gpu: &GpuContext, size: u64) -> wgpu::Buffer {
    gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn read_words(gpu: &GpuContext, buffer: &wgpu::Buffer) -> Vec<u32> {
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.expect("map"));
    gpu.device().poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let words = bytemuck::cast_slice(&buffer.get_mapped_range(..).expect("mapped")).to_vec();
    buffer.unmap();
    words
}

fn camera(aspect: f32) -> CameraData {
    CameraData::look_at(Vec3::new(0.0, 1.0, 6.0), Vec3::new(0.0, 1.0, 0.0), Vec3::Y, 0.8, aspect, 0.05)
}

#[test]
#[ignore = "needs a GPU"]
fn gpu_skinning_matches_the_cpu_reference() {
    let gpu = gpu();
    let mut rng = Rng(11);
    // u8 influences with quantized positions, u16 influences (a palette over 256) with f32 ones.
    let meshes = [
        random_mesh(&mut rng, 3000, 40, PositionFormat::Unorm16),
        random_mesh(&mut rng, 1500, 300, PositionFormat::Float32),
    ];
    let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, RenderSettings::default())
        .expect("renderer");
    let handles: Vec<_> = meshes.iter().map(|mesh| upload(&mut renderer, mesh)).collect();
    let texture = target(&gpu, (64, 64));
    let view = texture.create_view(&Default::default());
    let mut packets = FramePacketRing::new();
    let mut executor = GraphExecutor::new();

    // Three poses per mesh, the last with every weight zero: morphs off, skin on.
    for round in 0..3 {
        let poses: Vec<(Vec<Affine3A>, Vec<f32>)> = meshes
            .iter()
            .map(|mesh| {
                let palette =
                    (0..mesh.skin.as_ref().expect("skinned").binds.len()).map(|_| rng.affine()).collect();
                let weights =
                    if round == 2 { vec![0.0; 3] } else { vec![rng.range(-0.5, 1.5), 0.0, rng.next()] };
                (palette, weights)
            })
            .collect();
        let packet = packets.begin();
        packet.views.push(ViewData { camera: camera(1.0) });
        for (index, (handle, (palette, weights))) in handles.iter().zip(&poses).enumerate() {
            let id = InstanceId(index as u64);
            if round == 0 {
                packet.instances.changed.push(MeshInstance { id, mesh: *handle, transform: Mat4::IDENTITY });
            }
            packet.push_skinned(id, palette.iter().copied(), weights.iter().copied());
        }
        renderer.prepare(&gpu, packets.current());
        let stats = renderer.stats().skinning;
        assert_eq!((stats.instances, stats.skinned, stats.vertices, stats.ignored_poses), (2, 2, 4500, 0));
        assert_eq!(stats.palette_bytes, 48 * 340);

        let outputs = renderer.skinning().outputs().buffer().clone();
        let copy = readback(&gpu, outputs.size());
        let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let mut graph = FrameGraph::new();
        let target_id = graph.import_texture("target", &view);
        renderer.record(&mut graph, target_id);
        let (outputs, copy_ref) = (&outputs, &copy);
        graph.add_pass("read outputs").side_effect().record(move |ctx| {
            ctx.encoder.copy_buffer_to_buffer(outputs, 0, copy_ref, 0, outputs.size());
        });
        executor.execute(&gpu, graph).expect("a valid graph");
        assert!(pollster::block_on(scope.pop()).is_none());
        let words = read_words(&gpu, &copy);

        for (slot, (mesh, (palette, weights))) in meshes.iter().zip(&poses).enumerate() {
            let expected = reference::deform(mesh, palette, weights);
            let found = renderer.skinning().outputs().get(slot as u32).expect("deformed");
            let (positions, shading) = ((found.positions / 4) as usize, (found.shading / 4) as usize);
            let (mut worst_position, mut worst_normal) = (0.0f32, 1.0f32);
            for (v, vertex) in expected.iter().enumerate() {
                let p = Vec3::from_array(std::array::from_fn(|axis| {
                    f32::from_bits(words[positions + 3 * v + axis])
                }));
                let error = (p - Vec3::from(vertex.position)).abs().max_element();
                worst_position = worst_position.max(error / (1.0 + Vec3::from(vertex.position).length()));
                let shading_words = [words[shading + 2 * v], words[shading + 2 * v + 1]];
                let gpu_shading = VertexShading::from_bytes(bytemuck::cast(shading_words));
                for (gpu_normal, cpu_normal) in [
                    (gpu_shading.normal(), vertex.normal),
                    (gpu_shading.outline_normal(), vertex.outline_normal),
                ] {
                    worst_normal = worst_normal.min(Vec3::from(gpu_normal).dot(Vec3::from(cpu_normal)));
                }
            }
            eprintln!(
                "round {round}, mesh {slot}: worst position error {worst_position:.2e} (relative), worst normal {:.4}°",
                worst_normal.min(1.0).acos().to_degrees()
            );
            assert!(worst_position < 2e-6, "positions within float rounding: {worst_position}");
            // The GPU rounds octahedral encodings to the nearest snorm16 and the CPU to the closest
            // of four, a step apart (about 0.003°); and blending unrelated random rotations makes
            // some vertices' matrices nearly singular, which magnifies rounding in the normal's
            // direction. 0.06° at worst on an M5.
            assert!(worst_normal > 0.99999, "normals within 0.26°: {worst_normal}");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn prepass_depth_equals_the_main_pass_depth() {
    let gpu = gpu();
    let size = (161, 97);
    // No outlines: the hull would write depth in the main pass only.
    let material = slot(&AnimeCharacterMaterial {
        rim: Rim { strength: 0.3, ..Rim::default() },
        outline: Outline { width: 0.0, z_offset: 0.0 },
        ..AnimeCharacterMaterial::default()
    });
    let sphere = |center: Vec3, radius: f32| {
        let mut vertices = Vec::new();
        let (rings, segments) = (24u32, 32u32);
        for ring in 0..=rings {
            let theta = std::f32::consts::PI * ring as f32 / rings as f32;
            for segment in 0..=segments {
                let phi = std::f32::consts::TAU * segment as f32 / segments as f32;
                let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
                vertices.push(Vertex {
                    position: (center + n * radius).to_array(),
                    normal: n.to_array(),
                    outline_normal: n.to_array(),
                    uv: [0.0; 2],
                    color: [255; 4],
                });
            }
        }
        let mut indices = Vec::new();
        for ring in 0..rings {
            for segment in 0..segments {
                let a = ring * (segments + 1) + segment;
                let (b, c, d) = (a + segments + 1, a + 1, a + segments + 2);
                indices.extend([a, c, b, c, d, b]);
            }
        }
        (vertices, indices)
    };
    let (vertices, indices) = sphere(Vec3::new(-0.6, 1.0, 0.0), 0.7);
    let count = indices.len() as u32;
    let submeshes = vec![Submesh { first_index: 0, index_count: count, material: 0 }];
    let mut character = Mesh::from_vertices(
        &vertices,
        indices.clone(),
        submeshes.clone(),
        vec![material.clone()],
        PositionFormat::Unorm16,
    );
    // Every vertex follows the second palette entry.
    let influences = vec![VertexInfluence { binds: [1, 0, 0, 0], weights: [255, 0, 0, 0] }; vertices.len()];
    let bind = SkinBind { joint: 0, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() };
    character.skin = Some(Skin {
        joints: vec![Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY }],
        binds: vec![bind; 2],
        influences: Influences::pack(&influences, 2),
    });
    // In front of the character, hiding part of it.
    let (vertices, _) = sphere(Vec3::new(0.3, 1.0, 1.5), 0.5);
    let prop = Mesh::from_vertices(&vertices, indices, submeshes, vec![material], PositionFormat::Unorm16);

    let settings = RenderSettings { msaa: Msaa::Off, ..RenderSettings::default() };
    let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, settings).expect("renderer");
    renderer.readable_depth = true;
    let (character, prop) = (upload(&mut renderer, &character), upload(&mut renderer, &prop));
    let mut packets = FramePacketRing::new();
    let packet = packets.begin();
    packet.views.push(ViewData { camera: camera(size.0 as f32 / size.1 as f32) });
    let tilt = Mat4::from_rotation_z(0.3);
    packet.instances.changed.push(MeshInstance { id: InstanceId(1), mesh: character, transform: tilt });
    packet.instances.changed.push(MeshInstance { id: InstanceId(2), mesh: prop, transform: Mat4::IDENTITY });
    // Leans the character into the prop's way, so the two overlap on screen.
    let lean = Affine3A::from_rotation_translation(Quat::from_rotation_y(0.4), Vec3::new(0.5, 0.1, 0.2));
    packet.push_skinned(InstanceId(1), [Affine3A::IDENTITY, lean], []);
    renderer.prepare(&gpu, packets.current());

    let texture = target(&gpu, size);
    let view = texture.create_view(&Default::default());
    let padded = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffers: Vec<wgpu::Buffer> = (0..3).map(|_| readback(&gpu, u64::from(padded * size.1))).collect();
    let scope = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
    let mut graph = FrameGraph::new();
    let target_id = graph.import_texture("target", &view);
    let targets = renderer.record(&mut graph, target_id).expect("a view and a target");
    let read: [(TextureId, &wgpu::Buffer); 3] = [
        (targets.main_depth, &buffers[0]),
        (targets.character_depth, &buffers[1]),
        (targets.depth, &buffers[2]),
    ];
    let mut readback_pass = graph.add_pass("read depths").side_effect();
    for (texture, _) in read {
        readback_pass = readback_pass.read(texture);
    }
    readback_pass.record(move |ctx| {
        for (texture, buffer) in read {
            let texture = ctx.texture(texture);
            ctx.encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    aspect: wgpu::TextureAspect::DepthOnly,
                    ..texture.as_image_copy()
                },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(size.1),
                    },
                },
                texture.size(),
            );
        }
    });
    let mut executor = GraphExecutor::new();
    executor.execute(&gpu, graph).expect("a valid graph");
    assert!(pollster::block_on(scope.pop()).is_none());
    let depths: Vec<Vec<f32>> = buffers
        .iter()
        .map(|buffer| {
            read_words(&gpu, buffer)
                .chunks_exact(padded as usize / 4)
                .flat_map(|row| row[..size.0 as usize].iter().map(|&bits| f32::from_bits(bits)))
                .collect()
        })
        .collect();
    let [main, character, scene] = [&depths[0], &depths[1], &depths[2]];

    assert_eq!(scene, main, "the scene prepass's depth is the main pass's, bit for bit");
    let covered = character.iter().filter(|&&depth| depth > 0.0).count();
    let (mut equal, mut behind_prop) = (0, 0);
    for (&character, &main) in character.iter().zip(main) {
        if character > 0.0 {
            if character == main {
                equal += 1;
            } else {
                assert!(main > character, "only nearer geometry hides a character");
                behind_prop += 1;
            }
        }
    }
    eprintln!("{covered} character pixels: {equal} equal to the main depth, {behind_prop} behind the prop");
    assert!(covered > 400 && behind_prop > 50, "the character fills part of the image, partly hidden");
    assert_eq!(equal + behind_prop, covered);
}
