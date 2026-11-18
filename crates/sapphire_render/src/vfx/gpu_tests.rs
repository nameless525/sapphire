//! Binding lifetimes when effects stop or the view disappears.

use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

use super::*;

#[test]
#[ignore = "needs a GPU"]
fn dormant_effects_release_depth_bindings_and_active_effects_reuse_them() {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
    let layouts = crate::pipelines::Layouts::new(gpu.device(), shaders::ProbeRead::Faces);
    let builds = PipelineBuilds::new(None);
    let sources = ShaderSources::compiled_in();
    let frame = layouts.frame(crate::pipelines::FrameGroup::World);
    let off = DistortionResolution::Off;
    let mut vfx = Vfx::new(&gpu, frame, &layouts.view, 1, true, off, &builds);
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

/// The effects' pixels (M6 B5) are the prepared quads' share of the frame at the size they draw at: the output's
/// after the resolve, the scene's in the main pass. A 1 × 2 m sprite facing a 90° camera from 5 m covers a fiftieth
/// of the frame; a frame prepared with none counts none.
#[test]
#[ignore = "needs a GPU"]
fn the_effects_pixels_are_counted_at_the_size_they_draw_at() {
    let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
    let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
    let layouts = crate::pipelines::Layouts::new(gpu.device(), shaders::ProbeRead::Faces);
    let builds = PipelineBuilds::new(None);
    let sources = ShaderSources::compiled_in();
    let frame = layouts.frame(crate::pipelines::FrameGroup::World);
    let camera =
        CameraData::look_at(Vec3::Z * 5.0, Vec3::ZERO, Vec3::Y, std::f32::consts::FRAC_PI_2, 1.0, 0.1);
    let sprite = VfxSprite {
        position: Vec3::ZERO,
        axis: Vec3::ZERO,
        rotation: 0.0,
        half_width: 0.5,
        half_length: 1.0,
        color: Vec3::ONE,
        alpha: 1.0,
        additive: 1.0,
        soft: 0.0,
        shape: VfxShape::SoftDot,
        light: VfxLight::Emissive,
    };
    let active = VfxFrame { sprites: vec![sprite], ..VfxFrame::default() };
    for (after_resolve, pixels) in [(true, 200), (false, 50)] {
        let mut vfx =
            Vfx::new(&gpu, frame, &layouts.view, 1, after_resolve, DistortionResolution::Off, &builds);
        vfx.prepare(&active, Some(&camera), 1.0, false, &sources, BuildOn::ThisThread);
        assert!(vfx.split_for_record((100, 100), (50, 50)).is_some());
        assert_eq!(vfx.stats().pixels, pixels, "after the resolve: {after_resolve}");
        vfx.prepare(&VfxFrame::default(), Some(&camera), 1.0, false, &sources, BuildOn::ThisThread);
        assert!(vfx.split_for_record((100, 100), (50, 50)).is_none());
        assert_eq!(vfx.stats().pixels, 0);
    }
}
