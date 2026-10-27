//! Skinning tests that read GPU buffers and depth back. They need a GPU: run them with
//! `cargo test -p sapphire_render --tests -- --ignored`.

use glam::{Affine3A, Mat4, Quat, Vec3};
use sapphire_assets::material::{AnimeCharacterMaterial, Material, Outline, Rim};
use sapphire_assets::mesh::{
    Influences, Joint, JointTransform, MaterialSlot, Mesh, MorphDelta, MorphTarget, PositionFormat, Skin,
    SkinBind, Submesh, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags, VertexInfluence, VertexShading,
    quantize_weights,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc, wgpu};

use super::reference;
use crate::graph::{FrameGraph, GraphExecutor, TextureId};
use crate::{
    CameraData, CharacterBody, CharacterFace, CharacterInstance, CharacterLighting, FramePacketRing,
    InstanceId, MeshHandle, MeshInstance, Msaa, RenderSettings, Renderer, SkinningStats, ViewData,
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
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
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
        packet.views.push(ViewData::new(camera(1.0)));
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

/// Skins and morphs `mesh` once with `palette` and `weights`, and returns its output vertices'
/// shading as the passes after decode it.
fn deformed_shading(mesh: &Mesh, palette: Vec<Affine3A>, weights: Vec<f32>) -> Vec<VertexShading> {
    let gpu = gpu();
    let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, RenderSettings::default())
        .expect("renderer");
    let handle = upload(&mut renderer, mesh);
    let texture = target(&gpu, (64, 64));
    let view = texture.create_view(&Default::default());
    let mut packets = FramePacketRing::new();
    let packet = packets.begin();
    packet.views.push(ViewData::new(camera(1.0)));
    packet.instances.changed.push(MeshInstance {
        id: InstanceId(1),
        mesh: handle,
        transform: Mat4::IDENTITY,
    });
    packet.push_skinned(InstanceId(1), palette, weights);
    renderer.prepare(&gpu, packets.current());
    let outputs = renderer.skinning().outputs().buffer().clone();
    let copy = readback(&gpu, outputs.size());
    let mut graph = FrameGraph::new();
    let target_id = graph.import_texture("target", &view);
    renderer.record(&mut graph, target_id);
    let (outputs_ref, copy_ref) = (&outputs, &copy);
    graph.add_pass("read outputs").side_effect().record(move |ctx| {
        ctx.encoder.copy_buffer_to_buffer(outputs_ref, 0, copy_ref, 0, outputs_ref.size());
    });
    GraphExecutor::new().execute(&gpu, graph).expect("a valid graph");
    let words = read_words(&gpu, &copy);
    let shading = (renderer.skinning().outputs().get(0).expect("deformed").shading / 4) as usize;
    (0..mesh.streams.len())
        .map(|v| {
            VertexShading::from_bytes(bytemuck::cast([words[shading + 2 * v], words[shading + 2 * v + 1]]))
        })
        .collect()
}

#[test]
#[ignore = "needs a GPU"]
fn outline_normals_stay_shared_by_split_vertices_and_turn_with_morphs() {
    // Groups of three split vertices, as at a hard-edged corner: one position, one set of
    // influences, one welded outline normal, three shading normals. Linear blend skinning of
    // random palettes is no rotation, yet each group keeps one outline normal (M1 C4): the pass
    // transforms the outline normal itself, not the deformed normals' frames.
    let mut rng = Rng(23);
    let mut vertices = Vec::new();
    let mut influences = Vec::new();
    for _ in 0..400 {
        let position = [rng.range(-1.0, 1.0), rng.range(0.0, 2.0), rng.range(-0.5, 0.5)];
        let outline = rng.direction();
        let count = 1 + rng.index(4);
        let mut binds = [0u16; 4];
        let mut weights = [0.0f32; 4];
        for slot in 0..count {
            binds[slot] = rng.index(8) as u16;
            weights[slot] = rng.range(0.05, 1.0);
        }
        for _ in 0..3 {
            vertices.push(Vertex {
                position,
                normal: (outline + 0.8 * rng.direction()).normalize().to_array(),
                outline_normal: outline.to_array(),
                uv: [0.0; 2],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
            influences.push(VertexInfluence { binds, weights: quantize_weights(weights) });
        }
    }
    let indices: Vec<u32> = (0..vertices.len() as u32).collect();
    let submeshes = vec![Submesh { first_index: 0, index_count: indices.len() as u32, material: 0 }];
    let mut mesh = Mesh::from_vertices(
        &vertices,
        indices,
        submeshes,
        vec![slot(&AnimeCharacterMaterial::default())],
        PositionFormat::Unorm16,
    );
    mesh.skin = Some(Skin {
        joints: vec![Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY }],
        binds: (0..8)
            .map(|_| SkinBind { joint: 0, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
            .collect(),
        influences: Influences::pack(&influences, 8),
    });
    let palette = (0..8).map(|_| rng.affine()).collect();
    let shading = deformed_shading(&mesh, palette, Vec::new());
    let mut worst = 0.0f32;
    for group in shading.as_chunks::<3>().0 {
        let first = Vec3::from(group[0].outline_normal());
        for other in &group[1..] {
            worst = worst.max(first.angle_between(Vec3::from(other.outline_normal())).to_degrees());
        }
    }
    eprintln!("split vertices' outline normals after skinning: {worst:.4}° apart at worst");
    assert!(worst < 0.05, "{worst}°");

    // A morph that turns a smooth vertex's normal turns its outline normal with it.
    let smooth = |normal: Vec3| Vertex {
        position: [0.0, 1.0, 0.0],
        normal: normal.to_array(),
        outline_normal: normal.to_array(),
        uv: [0.0; 2],
        color: VERTEX_COLOR_NEUTRAL,
        flags: VertexFlags::NONE,
    };
    let normals = [Vec3::Z, Vec3::new(0.3, 0.5, 0.8).normalize(), Vec3::NEG_X];
    let vertices: Vec<Vertex> = normals.iter().map(|&n| smooth(n)).collect();
    let submeshes = vec![Submesh { first_index: 0, index_count: 3, material: 0 }];
    let mut morphed = Mesh::from_vertices(
        &vertices,
        vec![0, 1, 2],
        submeshes,
        vec![slot(&AnimeCharacterMaterial::default())],
        PositionFormat::Unorm16,
    );
    // Each normal turns 35° about a different axis at full weight.
    let turned: Vec<Vec3> = normals
        .iter()
        .zip([Vec3::X, Vec3::Y, Vec3::Z])
        .map(|(&n, axis)| Quat::from_axis_angle(axis, 35f32.to_radians()) * n)
        .collect();
    morphed.morph_deltas =
        (0..3).map(|v| MorphDelta::new(v as u32, [0.0; 3], (turned[v] - normals[v]).to_array())).collect();
    morphed.morph_targets =
        vec![MorphTarget { name: "turn".into(), default_weight: 0.0, first_delta: 0, delta_count: 3 }];
    for (v, shading) in deformed_shading(&morphed, Vec::new(), vec![1.0]).iter().enumerate() {
        let (normal, outline) = (Vec3::from(shading.normal()), Vec3::from(shading.outline_normal()));
        let off = normal.angle_between(outline).to_degrees();
        eprintln!("morphed vertex {v}: outline normal {off:.4}° from the morphed normal");
        assert!(normal.angle_between(turned[v]).to_degrees() < 0.05 && off < 0.05, "vertex {v}: {off}°");
    }
}

#[test]
#[ignore = "needs a GPU"]
fn prepass_depth_equals_the_main_pass_depth() {
    let gpu = gpu();
    let size = (161, 97);
    // Outlined: the hulls draw into the scene prepass too, at the main pass's depths (M1 C4).
    let material = slot(&AnimeCharacterMaterial {
        rim: Rim { strength: 0.3, ..Rim::default() },
        outline: Outline { width: 8.0, ..Outline::default() },
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
                    color: VERTEX_COLOR_NEUTRAL,
                    flags: VertexFlags::NONE,
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
    packet.views.push(ViewData::new(camera(size.0 as f32 / size.1 as f32)));
    let tilt = Mat4::from_rotation_z(0.3);
    packet.instances.changed.push(MeshInstance { id: InstanceId(1), mesh: character, transform: tilt });
    packet.instances.changed.push(MeshInstance { id: InstanceId(2), mesh: prop, transform: Mat4::IDENTITY });
    // Leans the character into the prop's way, so the two overlap on screen.
    let lean = Affine3A::from_rotation_translation(Quat::from_rotation_y(0.4), Vec3::new(0.5, 0.1, 0.2));
    packet.push_skinned(InstanceId(1), [Affine3A::IDENTITY, lean], []);
    // Listed as a character, so the character prepass draws it (M1 C5); the prop isn't.
    packet.characters.push(CharacterInstance {
        id: InstanceId(1),
        lighting: CharacterLighting::unstyled(packet.sun),
        face: CharacterFace::default(),
        body: CharacterBody::default(),
        opacity: 1.0,
    });
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

    assert_eq!(
        scene, main,
        "the scene prepass's depth is the main pass's, bit for bit, outline hulls included"
    );
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

/// A palette and morph weights.
type Pose = (Vec<Affine3A>, Vec<f32>);

/// A random pose of `mesh`.
fn random_pose(rng: &mut Rng, mesh: &Mesh) -> Pose {
    let joints = mesh.skin.as_ref().expect("skinned").binds.len();
    ((0..joints).map(|_| rng.affine()).collect(), vec![rng.next(), 0.0, rng.range(-0.5, 1.5)])
}

/// A renderer whose frame data for skinning holds `limit` bytes, skinning frames of poses and
/// reading its outputs back.
struct Skinner {
    gpu: GpuContext,
    renderer: Renderer,
    packets: FramePacketRing,
    executor: GraphExecutor,
    view: wgpu::TextureView,
}

impl Skinner {
    fn new(limit: u64) -> Self {
        let gpu = gpu();
        let mut renderer =
            Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, RenderSettings::default())
                .expect("renderer");
        renderer.limit_buffers(1024, 1024, None, Some(limit));
        let view = target(&gpu, (64, 64)).create_view(&Default::default());
        Self { gpu, renderer, packets: FramePacketRing::new(), executor: GraphExecutor::new(), view }
    }

    /// Prepares and records a frame that adds `added` instances (ID, mesh) and poses `poses`
    /// (ID, pose): the skinning stats its `prepare` gave, and the outputs' words once it ran.
    fn frame(&mut self, added: &[(u64, MeshHandle)], poses: &[(u64, &Pose)]) -> (SkinningStats, Vec<u32>) {
        let packet = self.packets.begin();
        packet.views.push(ViewData::new(camera(1.0)));
        for &(id, mesh) in added {
            packet.instances.changed.push(MeshInstance {
                id: InstanceId(id),
                mesh,
                transform: Mat4::IDENTITY,
            });
        }
        for &(id, (palette, weights)) in poses {
            packet.push_skinned(InstanceId(id), palette.iter().copied(), weights.iter().copied());
        }
        self.renderer.prepare(&self.gpu, self.packets.current());
        let stats = self.renderer.stats().skinning;
        let outputs = self.renderer.skinning().outputs().buffer().clone();
        let copy = readback(&self.gpu, outputs.size());
        let scope = self.gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let mut graph = FrameGraph::new();
        let target_id = graph.import_texture("target", &self.view);
        self.renderer.record(&mut graph, target_id);
        let (outputs, copy_ref) = (&outputs, &copy);
        graph.add_pass("read outputs").side_effect().record(move |ctx| {
            ctx.encoder.copy_buffer_to_buffer(outputs, 0, copy_ref, 0, outputs.size());
        });
        self.executor.execute(&self.gpu, graph).expect("a valid graph");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        (stats, read_words(&self.gpu, &copy))
    }

    /// The largest relative error of the positions the instance in `slot` holds in `words`
    /// against `mesh` in `pose`, skinned and morphed on the CPU; `None` if it has nothing to draw.
    fn error(&self, words: &[u32], slot: u32, mesh: &Mesh, pose: &Pose) -> Option<f32> {
        let positions = (self.renderer.skinning().outputs().get(slot)?.positions / 4) as usize;
        let expected = reference::deform(mesh, &pose.0, &pose.1);
        Some(expected.iter().enumerate().fold(0.0f32, |worst, (v, vertex)| {
            let p =
                Vec3::from_array(std::array::from_fn(|axis| f32::from_bits(words[positions + 3 * v + axis])));
            let expected = Vec3::from(vertex.position);
            worst.max((p - expected).abs().max_element() / (1.0 + expected.length()))
        }))
    }
}

#[test]
#[ignore = "needs a GPU"]
fn poses_past_the_frame_limit_wait_for_the_next_frame() {
    // A frame's jobs, palettes and weights go up as one storage buffer (review RA-004). Two
    // instances posed in one packet need 16 bytes of header, two 80-byte jobs, 340 palette
    // matrices of 48 bytes and six weights, rounded up to 16 bytes: 16 528 bytes. With room for
    // that or more both are skinned at once; with less, the second waits for the next recorded
    // frame, its pose kept, and has nothing to draw until then.
    const WHOLE: u64 = 16_528;
    let mut rng = Rng(31);
    let meshes = [
        random_mesh(&mut rng, 600, 40, PositionFormat::Unorm16),
        random_mesh(&mut rng, 300, 300, PositionFormat::Float32),
    ];
    for (limit, deferred) in [(WHOLE + 16, 0), (WHOLE, 0), (WHOLE - 1, 1), (WHOLE - 16, 1)] {
        let mut skinner = Skinner::new(limit);
        let added: Vec<_> = meshes
            .iter()
            .enumerate()
            .map(|(id, mesh)| (id as u64, upload(&mut skinner.renderer, mesh)))
            .collect();
        let poses: Vec<Pose> = meshes.iter().map(|mesh| random_pose(&mut rng, mesh)).collect();
        let posed: Vec<_> = poses.iter().enumerate().map(|(id, pose)| (id as u64, pose)).collect();
        // The first frame poses both.
        let (stats, words) = skinner.frame(&added, &posed);
        assert_eq!((stats.skinned, stats.deferred_poses), (2, 0));
        let first = skinner.error(&words, 0, &meshes[0], &poses[0]).expect("skinned");
        assert!(first < 2e-6, "limit {limit}: {first}");
        let second = skinner.error(&words, 1, &meshes[1], &poses[1]);
        if deferred == 1 {
            assert_eq!(second, None, "limit {limit}: nothing to draw before its first skin");
        } else {
            assert!(second.is_some_and(|error| error < 2e-6), "limit {limit}: {second:?}");
        }
        // The second poses nothing: a waiting pose is skinned now.
        let (stats, words) = skinner.frame(&[], &[]);
        assert_eq!((stats.skinned, stats.deferred_poses), (deferred, deferred));
        for (slot, (mesh, pose)) in meshes.iter().zip(&poses).enumerate() {
            let error = skinner.error(&words, slot as u32, mesh, pose).expect("skinned");
            assert!(error < 2e-6, "limit {limit}, instance {slot}: {error}");
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn waiting_poses_take_turns_and_keep_their_latest_pose() {
    // Room for one pose a frame: a 16-byte header, an 80-byte job, 40 palette matrices and three
    // weights, rounded up to 16 bytes. Three instances wait in turn, in the order they were
    // queued, and a waiting instance posed again is skinned with its latest pose.
    const ONE: u64 = 2032;
    let mut rng = Rng(37);
    let mesh = random_mesh(&mut rng, 200, 40, PositionFormat::Unorm16);
    let mut skinner = Skinner::new(ONE);
    let handle = upload(&mut skinner.renderer, &mesh);
    let added = [(0, handle), (1, handle), (2, handle)];
    let first: Vec<Pose> = (0..3).map(|_| random_pose(&mut rng, &mesh)).collect();
    let second: Vec<Pose> = (0..2).map(|_| random_pose(&mut rng, &mesh)).collect();
    let error =
        |skinner: &Skinner, words: &[u32], slot: u32, pose: &Pose| skinner.error(words, slot, &mesh, pose);
    let close = |error: Option<f32>| error.is_some_and(|error| error < 2e-6);

    // All three posed: the first is skinned, the others wait.
    let (stats, words) = skinner.frame(&added, &[(0, &first[0]), (1, &first[1]), (2, &first[2])]);
    assert_eq!((stats.skinned, stats.deferred_poses), (3, 0));
    assert!(close(error(&skinner, &words, 0, &first[0])));
    assert_eq!((error(&skinner, &words, 1, &first[1]), error(&skinner, &words, 2, &first[2])), (None, None));
    // The first and second posed again: the second, first in line, takes its new pose.
    let (stats, words) = skinner.frame(&[], &[(0, &second[0]), (1, &second[1])]);
    assert_eq!((stats.skinned, stats.deferred_poses), (3, 2));
    assert!(close(error(&skinner, &words, 1, &second[1])), "the latest pose");
    assert!(close(error(&skinner, &words, 0, &first[0])), "still its first pose");
    assert_eq!(error(&skinner, &words, 2, &first[2]), None);
    // Nothing posed: the third, then the first with its second pose.
    let (stats, words) = skinner.frame(&[], &[]);
    assert_eq!((stats.skinned, stats.deferred_poses), (2, 2));
    assert!(close(error(&skinner, &words, 2, &first[2])));
    assert!(close(error(&skinner, &words, 0, &first[0])));
    let (stats, words) = skinner.frame(&[], &[]);
    assert_eq!((stats.skinned, stats.deferred_poses), (1, 1));
    assert!(close(error(&skinner, &words, 0, &second[0])));
    assert!(close(error(&skinner, &words, 1, &second[1])) && close(error(&skinner, &words, 2, &first[2])));
    let (stats, _) = skinner.frame(&[], &[]);
    assert_eq!((stats.skinned, stats.deferred_poses), (0, 0));
}
