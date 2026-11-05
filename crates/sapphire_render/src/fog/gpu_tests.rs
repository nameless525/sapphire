//! Fog-card binding lifetime regressions; run explicitly with a GPU.

use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

use super::*;

#[test]
#[ignore = "needs a GPU"]
fn depth_bindings_keep_only_reusable_sources_and_release_when_dormant() {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
    let device = gpu.device();
    let storage = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            storage(0),
            storage(1),
            wgpu::BindGroupLayoutEntry {
                binding: FOG_CARD_DEPTH_BINDING,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let buffer = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        })
    };
    let view = |side| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let (instances, materials) = (buffer(), buffer());
    let mut cards = FogCards::new(device, true);
    let bind = |cards: &mut FogCards, instances: &wgpu::Buffer, depth: &wgpu::TextureView| {
        cards.split_for_record().1.get(device, &layout, instances, &materials, depth).clone()
    };
    let (a, b) = (view(8), view(8));
    let first = bind(&mut cards, &instances, &a);
    let second = bind(&mut cards, &instances, &b);
    for _ in 0..4 {
        assert_eq!(bind(&mut cards, &instances, &a), first);
        assert_eq!(bind(&mut cards, &instances, &b), second);
    }
    let large = view(16);
    bind(&mut cards, &instances, &large);
    assert_eq!(cards.depth_groups.iter().flatten().count(), 1, "old size released on first miss");
    let moved = buffer();
    bind(&mut cards, &moved, &large);
    assert!(cards.depth_groups.iter().flatten().all(|group| group.instances == moved));
    cards.no_view();
    assert!(cards.depth_groups.iter().all(Option::is_none));

    bind(&mut cards, &instances, &a);
    let camera = CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.1);
    let fog =
        FogAdmission::new(&Fog::default(), Vec3::ZERO, camera.position.y, wgpu::TextureFormat::Rgba16Float);
    cards.sort(
        &camera,
        &DrawList::default(),
        &InstanceSlots::default(),
        &[],
        &Materials::new(device, gpu.queue()),
        &fog,
    );
    assert!(cards.depth_groups.iter().all(Option::is_none), "an empty card list releases the depth");
}
