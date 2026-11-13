//! Binding lifetimes when effects stop or the view disappears.

use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

use super::*;

#[test]
#[ignore = "needs a GPU"]
fn dormant_effects_release_depth_bindings_and_active_effects_reuse_them() {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
    let layouts = crate::pipelines::Layouts::new(gpu.device());
    let builds = PipelineBuilds::new(None);
    let sources = ShaderSources::compiled_in();
    let mut vfx =
        Vfx::new(&gpu, layouts.frame(crate::pipelines::FrameGroup::World), &layouts.view, 1, true, &builds);
    let camera = CameraData::look_at(Vec3::Z * 5.0, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.1);
    let active = VfxFrame {
        sprites: vec![VfxSprite {
            position: Vec3::ZERO,
            axis: Vec3::ZERO,
            rotation: 0.0,
            half_width: 0.1,
            half_length: 0.1,
            color: Vec3::ONE,
            alpha: 1.0,
            additive: 0.0,
            soft: 1.0,
            shape: VfxShape::SoftDot,
            light: VfxLight::Emissive,
        }],
        ..VfxFrame::default()
    };
    for no_view in [false, true] {
        vfx.prepare(&active, Some(&camera), 1.0, false, &sources, BuildOn::ThisThread);
        let state = vfx.gpu.as_mut().expect("active effects built");
        let depth = gpu
            .device()
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let first =
            state.depth_groups.get(gpu.device(), None, &state.textures_layout, &[&state.map, &depth]).clone();
        vfx.prepare(&active, Some(&camera), 1.0, false, &sources, BuildOn::ThisThread);
        let state = vfx.gpu.as_mut().expect("built");
        assert_eq!(
            state.depth_groups.get(gpu.device(), None, &state.textures_layout, &[&state.map, &depth]),
            &first
        );
        if no_view {
            vfx.prepare(&active, None, 1.0, false, &sources, BuildOn::ThisThread);
        } else {
            vfx.prepare(&VfxFrame::default(), Some(&camera), 1.0, false, &sources, BuildOn::ThisThread);
        }
        assert!(vfx.gpu.as_ref().expect("built").depth_groups.is_empty());
    }
}
