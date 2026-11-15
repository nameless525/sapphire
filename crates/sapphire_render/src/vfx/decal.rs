//! The frame's ground decals (M5 L7): [`VfxFrame::decals`](sapphire_look::VfxFrame), each a quad lying in the
//! ground's plane where `sapphire_vfx` placed it, its signed distance shape evaluated per pixel by
//! `shaders/vfx_decal.wgsl` (`sapphire_look::vfx::decal` has the shapes and the reference). A family of their
//! own ([`PipelineFamily::Decals`]): two pipelines, one buffer and the world's frame group. **Nothing of it
//! exists until a frame has a decal**, or a load declares them
//! ([`PipelineInventory::decals`](crate::PipelineInventory)).
//!
//! - **Where they draw: in the main pass, on every tier,** after the opaque and alpha-tested surfaces
//!   (characters' and the world's both, so every depth a decal could be hidden by is written) and before the
//!   characters' outlines, the eyes through hair, the sky, transparent surfaces and fog cards. So a character
//!   or a prop standing on a decal covers it by depth, its outline draws over it, and mist in front of it
//!   draws over it. Unlike sprites they aren't held back till after temporal AA's resolve: a decal lies still
//!   on still ground, whose velocity of zero is its own, so the history holds it as it holds the ground, and
//!   gains it the jitter's smoothing.
//! - **Never over a character's feet:** depth-tested against what the main pass wrote, writing none, pulled
//!   toward the eye by far less than a shoe's sole: a foot on the ground covers it as the ground would be
//!   covered.
//! - **The pull** ([`PULL`], [`SLOPE_BIAS`]): every point of a decal drawn nearer the eye by a share of its own
//!   distance, in the vertex stage, and the rasterizer's slope term for grazing views, so a decal lying where the
//!   ground is never fights it, at near and far ends of the depth range and at grazing angles alike, and never
//!   visibly floats. `tests/gpu/vfx_decal.rs` reads the margins back. The rasterizer's constant bias isn't used:
//!   on a float depth buffer Direct3D and Vulkan size it by the largest depth anywhere in the primitive, which
//!   under reverse Z is the quad's nearest point, so a decal the camera stands over (its quad crossing the near
//!   plane) took a bias sized for 5 cm away at its far side, and on WARP drew over the ground sunk 4.6e-3 of
//!   the distance 150 m away at 60°, where Metal, sizing it by each fragment's own depth, read 9.8e-4.
//! - **Two blends:** light added (`One`, `One`) × 1/exposure, bright enough for bloom; and darkening (`Zero`,
//!   `Src`), the ground multiplied toward a color. Darkening draws first, then light, so a telegraph over a
//!   scorch glows over it. Both leave scene color's alpha alone: a decal never marks or unmarks a character.
//! - **Fog:** a decal takes the fog in front of it, as the ground under it does: light added takes one ray's
//!   transmittance, a darkening darkens only the share of the pixel that's the ground's.
//! - **Not in debug views,** which replay the main pass's meshes; decals aren't meshes, as effects aren't.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2};
use sapphire_look::{VfxDecal, VfxDecalBlend, VfxDecalShape};
use sapphire_rhi::wgpu;

use super::{COLOR_MAX, VfxMaker, sane, sane_color};
use crate::buffer::{self, GrowableBuffer};
use crate::counts::PassCounts;
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::pipelines::{DEPTH_FORMAT, SCENE_COLOR_FORMAT};
use crate::shader_builds::Step;
use crate::shaders::{self, BuildOn, Program, ShaderSources};

/// The name of the shader's constant: the pipeline darkens rather than adds light.
const DARKEN_CONSTANT: &str = "VFX_DECAL_DARKEN";
/// The name of the shader's constant [`PULL`] arrives by.
const PULL_CONSTANT: &str = "VFX_DECAL_PULL";

/// The share of its own distance every point of a decal is drawn nearer the eye, in the vertex stage (clip z
/// scaled by 1 + this, under the view's reverse Z with an infinite far plane): the same at every depth and for a
/// quad of any size, on every backend. It's what holds ground seen from straight above, where the slope term is
/// nothing. Chosen from the z-fight test's readbacks (`tests/gpu/vfx_decal.rs`): with nothing at all, a decal
/// lying on the ground lost 12% to 69% of its pixels to it seen from above, and needed lifting by up to 2.4e-7 of
/// the distance to show whole; this is 25 times that, as near as Metal's sizing of the old constant bias came
/// (5.4e-6 to 6.4e-6).
pub(crate) const PULL: f32 = 6e-6;

/// The rasterizer's slope term: by how much of the depth's change across a pixel it pulls toward the eye, for
/// ground seen at an angle, where neighbouring pixels' depths differ the most and the rasterizer's rounding of
/// them with it. With nothing a decal needed lifting by up to 6.1e-5 of the distance at 60° and 4.3e-5 at 85°
/// under temporal AA's jitter; with this it reaches 1.0e-3 at 60° and 2.4e-4 at 85°, 16 and 5.6 times those.
/// That's under a tenth of a pixel's footprint on the ground, at any resolution, since the term is a share of a
/// pixel's change: a decal never visibly floats over a foot. Every backend sizes it the same way.
pub(crate) const SLOPE_BIAS: f32 = 0.2;

/// Decals the buffer holds when first made: past every tier's budget, so it never grows in a fight.
const INITIAL_DECALS: u64 = 32;

/// `GpuVfxDecal::kind`.
const DISC: u32 = 0;
const RING: u32 = 1;
const SECTOR: u32 = 2;
const LANE: u32 = 3;

/// The least margin past a shape's edge its quad reaches, metres, and its share of the shape's size: room for
/// the edge's smoothing over a pixel's footprint.
const MARGIN: f32 = 0.03;
const MARGIN_SHARE: f32 = 0.02;

/// A decal (`VfxDecal` in `vfx_decal.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuVfxDecal {
    world_from_local: [[f32; 4]; 4],
    bounds: [f32; 4],
    color: [f32; 3],
    alpha: f32,
    shape: [f32; 4],
    kind: u32,
    fill: f32,
    outline: f32,
    body: f32,
    noise: f32,
    padding: [f32; 3],
}

/// `VfxDecalUniforms` in `vfx_decal.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct GpuVfxDecalUniforms {
    emissive: f32,
    padding: [f32; 3],
}

/// A size of a shape: finite, above 0 and at most an arena's, else `None`.
fn size(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value.min(1e3))
}

impl GpuVfxDecal {
    /// `decal`, unless where it lies, how it's turned or its shape isn't sound: its colors and shares are made
    /// safe.
    pub(crate) fn new(decal: &VfxDecal) -> Option<Self> {
        if !(decal.position.is_finite() && decal.rotation.is_finite()) {
            return None;
        }
        let rotation = decal.rotation.normalize();
        if !rotation.is_finite() {
            return None;
        }
        let (kind, shape, checked) = match decal.shape {
            VfxDecalShape::Disc { radius } => {
                let radius = size(radius)?;
                (DISC, [radius, 0.0, 0.0, 0.0], VfxDecalShape::Disc { radius })
            }
            VfxDecalShape::Ring { inner, outer } => {
                let outer = size(outer)?;
                let inner = if inner.is_finite() { inner.clamp(0.0, outer) } else { 0.0 };
                (RING, [inner, outer, 0.0, 0.0], VfxDecalShape::Ring { inner, outer })
            }
            VfxDecalShape::Sector { radius, angle, inner } => {
                let radius = size(radius)?;
                let angle = if angle.is_finite() { angle.clamp(0.0, 360.0) } else { 360.0 };
                let inner = if inner.is_finite() { inner.clamp(0.0, radius) } else { 0.0 };
                let (sin, cos) = (angle * 0.5).to_radians().sin_cos();
                (SECTOR, [radius, sin, cos, inner], VfxDecalShape::Sector { radius, angle, inner })
            }
            VfxDecalShape::Lane { width, length } => {
                let (width, length) = (size(width)?, size(length)?);
                (LANE, [width, length, 0.0, 0.0], VfxDecalShape::Lane { width, length })
            }
        };
        let extent = checked.size();
        let noise = if decal.noise.is_finite() { decal.noise.clamp(0.0, extent * 0.25) } else { 0.0 };
        let outline = if decal.outline.is_finite() { decal.outline.clamp(0.0, extent * 0.5) } else { 0.0 };
        let margin = Vec2::splat(noise + MARGIN.max(MARGIN_SHARE * extent));
        let (min, max) = checked.bounds();
        let (min, max) = (min - margin, max + margin);
        let world_from_local = Mat4::from_rotation_translation(rotation, decal.position);
        Some(Self {
            world_from_local: world_from_local.to_cols_array_2d(),
            bounds: [min.x, min.y, max.x, max.y],
            color: sane_color(decal.color),
            alpha: sane(decal.alpha, 1.0),
            shape,
            kind,
            fill: sane(decal.fill, 1.0),
            outline,
            body: sane(decal.body, 1.0),
            noise,
            padding: [0.0; 3],
        })
    }
}

/// What the decals' pipelines are built with: fixed for a renderer's life.
#[derive(Clone, Debug)]
pub(super) struct DecalBuilder {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    samples: u32,
    builds: PipelineBuilds,
}

/// Light added over what's there.
const LIGHT_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::REPLACE,
};

/// What's there multiplied by the output.
const DARKEN_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::Src,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent::REPLACE,
};

impl DecalBuilder {
    /// A decal pipeline of the main pass: its samples and depth, testing and never writing it, pulled toward
    /// the eye ([`PULL`] in the vertex stage, [`SLOPE_BIAS`] in the rasterizer, no constant bias); a strip of
    /// four corners per instance; the color alone written.
    fn pipeline(&self, module: &wgpu::ShaderModule, darken: bool) -> wgpu::RenderPipeline {
        let constants = [(DARKEN_CONSTANT, if darken { 1.0 } else { 0.0 }), (PULL_CONSTANT, f64::from(PULL))];
        let compilation_options =
            wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() };
        self.builds.render(
            &self.device,
            PipelineFamily::Decals,
            &wgpu::RenderPipelineDescriptor {
                label: Some(if darken { "vfx decal darken" } else { "vfx decal light" }),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_vfx_decal"),
                    compilation_options: compilation_options.clone(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: 0, slope_scale: SLOPE_BIAS, clamp: 0.0 },
                }),
                multisample: wgpu::MultisampleState { count: self.samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_vfx_decal"),
                    compilation_options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: SCENE_COLOR_FORMAT,
                        blend: Some(if darken { DARKEN_BLEND } else { LIGHT_BLEND }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: self.builds.cache(),
            },
        )
    }
}

/// The decals' pipelines, light's and darkening's, built together: a frame with one decal builds both, so the
/// next one's blend never stalls a frame.
#[derive(Clone, Debug)]
pub(crate) struct DecalPipelines {
    pub(super) light: wgpu::RenderPipeline,
    pub(super) darken: wgpu::RenderPipeline,
}

impl DecalPipelines {
    pub(super) const COUNT: u32 = 2;

    fn build(builder: &DecalBuilder, sources: &ShaderSources, on: BuildOn) -> Self {
        let module = shaders::module(&builder.device, Program::VfxDecal, sources);
        let module = &module;
        let builds = [false, true].map(|darken| move || builder.pipeline(module, darken));
        let mut built = on.run(builds.into());
        let darken = built.pop().expect("two built");
        let light = built.pop().expect("two built");
        Self { light, darken }
    }
}

fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    use wgpu::ShaderStages as S;
    let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("vfx decal buffers"),
        entries: &[
            // The fragment stage's storage buffers are all taken (`frame.wgsl`).
            entry(
                0,
                S::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<GpuVfxDecal>() as u64),
                },
            ),
            entry(
                1,
                S::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<GpuVfxDecalUniforms>() as u64),
                },
            ),
        ],
    })
}

fn builder(maker: &VfxMaker) -> (DecalBuilder, wgpu::BindGroupLayout) {
    let buffers = layout(&maker.device);
    let layout = maker.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vfx decal"),
        bind_group_layouts: &[Some(&maker.frame_layout), Some(&maker.view_layout), Some(&buffers)],
        immediate_size: 0,
    });
    let builder = DecalBuilder {
        device: maker.device.clone(),
        layout,
        samples: maker.samples,
        builds: maker.builds.clone(),
    };
    (builder, buffers)
}

/// Everything the decals make when the first is drawn.
#[derive(Debug)]
pub(crate) struct DecalGpu {
    builder: DecalBuilder,
    pipelines: DecalPipelines,
    decals: GrowableBuffer,
    uniforms: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
}

fn group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    decals: &wgpu::Buffer,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vfx decal buffers"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: decals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: uniforms.as_entire_binding() },
        ],
    })
}

impl DecalGpu {
    fn new(builder: DecalBuilder, pipelines: DecalPipelines, layout: wgpu::BindGroupLayout) -> Self {
        let device = &builder.device;
        let limit = device.limits().max_storage_buffer_binding_size;
        let capacity = INITIAL_DECALS * size_of::<GpuVfxDecal>() as u64;
        let decals = GrowableBuffer::new(device, "vfx decals", wgpu::BufferUsages::STORAGE, capacity, limit);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vfx decal uniforms"),
            size: size_of::<GpuVfxDecalUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = group(device, &layout, decals.buffer(), &uniforms);
        Self { builder, pipelines, decals, uniforms, layout, group }
    }

    fn make(maker: &VfxMaker, sources: &ShaderSources, on: BuildOn) -> Self {
        let (builder, layout) = builder(maker);
        let pipelines = DecalPipelines::build(&builder, sources, on);
        Self::new(builder, pipelines, layout)
    }
}

/// The frame's decals' share of the effects ([`super::Vfx`] holds it).
#[derive(Debug, Default)]
pub(crate) struct VfxDecals {
    gpu: Option<Box<DecalGpu>>,
    /// This frame's decals, those that darken first.
    packed: Vec<GpuVfxDecal>,
    /// How many of them darken.
    darken: u32,
    /// Decals left out because they weren't sound or didn't fit the buffer's limit.
    pub(super) rejected: u32,
}

impl VfxDecals {
    /// Makes what the first decal needs and builds both pipelines, unless they're built.
    pub(super) fn build(&mut self, maker: &VfxMaker, sources: &ShaderSources, on: BuildOn) {
        if self.gpu.is_none() {
            self.gpu = Some(Box::new(DecalGpu::make(maker, sources, on)));
        }
    }

    /// Whether they're built.
    pub(super) fn held(&self) -> bool {
        self.gpu.is_some()
    }

    /// A warm-up's step: what [`Self::build`] would make, off the frame. `None` when it's made.
    pub(super) fn warm(&self, maker: VfxMaker, on: BuildOn) -> Option<Step<Box<DecalGpu>>> {
        if self.gpu.is_some() {
            return None;
        }
        Some(Box::new(move |sources, _| Box::new(DecalGpu::make(&maker, sources, on))))
    }

    /// Takes what a warm-up made, unless the first decal made its own meanwhile.
    pub(super) fn adopt(&mut self, made: Box<DecalGpu>) {
        if self.gpu.is_none() {
            self.gpu = Some(made);
        }
    }

    pub(super) fn pipelines(&self) -> u32 {
        if self.gpu.is_some() { DecalPipelines::COUNT } else { 0 }
    }

    /// Both pipelines anew from new sources, if they're built.
    pub(super) fn rebuild(&self) -> Option<Step<DecalPipelines>> {
        let builder = self.gpu.as_ref()?.builder.clone();
        Some(Box::new(move |sources, _cancel| DecalPipelines::build(&builder, sources, BuildOn::ThisThread)))
    }

    pub(super) fn set_pipelines(&mut self, pipelines: DecalPipelines) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.pipelines = pipelines;
        }
    }

    /// Clears the last frame's decals.
    pub(super) fn clear(&mut self) {
        self.packed.clear();
        self.darken = 0;
        self.rejected = 0;
    }

    /// Packs `decals`, those that darken first, and sizes their buffer. [`Self::build`] first.
    pub(super) fn prepare(&mut self, decals: &[VfxDecal], device: &wgpu::Device, queue: &wgpu::Queue) {
        self.clear();
        let Some(gpu) = self.gpu.as_mut() else { return };
        let size = size_of::<GpuVfxDecal>() as u64;
        let fits = &decals[..decals.len().min(gpu.decals.max_items(size) as usize)];
        for blend in [VfxDecalBlend::Darken, VfxDecalBlend::Light] {
            let of_blend = fits.iter().filter(|decal| decal.blend == blend);
            self.packed.extend(of_blend.filter_map(GpuVfxDecal::new));
            if blend == VfxDecalBlend::Darken {
                self.darken = self.packed.len() as u32;
            }
        }
        self.rejected = (decals.len() - self.packed.len()) as u32;
        if gpu.decals.reserve(device, queue, self.packed.len() as u64 * size, 0) {
            gpu.group = group(device, &gpu.layout, gpu.decals.buffer(), &gpu.uniforms);
        }
    }

    pub(super) fn count(&self) -> u32 {
        self.packed.len() as u32
    }

    pub(super) fn bytes(&self) -> u64 {
        (self.packed.len() * size_of::<GpuVfxDecal>()) as u64
    }

    /// This frame's upload and draw, `None` with no decals. `emissive` is 1 / exposure.
    pub(super) fn split_for_record(&self, emissive: f32) -> Option<(DecalUpload<'_>, DecalDraw<'_>)> {
        if self.packed.is_empty() {
            return None;
        }
        let gpu = self.gpu.as_ref()?;
        let upload = DecalUpload {
            uniforms: GpuVfxDecalUniforms { emissive: emissive.min(COLOR_MAX), padding: [0.0; 3] },
            uniforms_buffer: &gpu.uniforms,
            decals: &self.packed,
            buffer: gpu.decals.buffer(),
        };
        let draw = DecalDraw {
            light: &gpu.pipelines.light,
            darken: &gpu.pipelines.darken,
            group: &gpu.group,
            darkening: self.darken,
            count: self.packed.len() as u32,
        };
        Some((upload, draw))
    }
}

/// The decals' uploads.
#[derive(Debug)]
pub(crate) struct DecalUpload<'a> {
    uniforms: GpuVfxDecalUniforms,
    uniforms_buffer: &'a wgpu::Buffer,
    decals: &'a [GpuVfxDecal],
    buffer: &'a wgpu::Buffer,
}

impl DecalUpload<'_> {
    pub(super) fn write(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        buffer::stage(belt, encoder, self.uniforms_buffer, 0, bytemuck::bytes_of(&self.uniforms));
        buffer::stage(belt, encoder, self.buffer, 0, bytemuck::cast_slice(self.decals));
    }
}

/// The decals' draw in the main pass (see the module docs for where).
#[derive(Clone, Copy, Debug)]
pub(crate) struct DecalDraw<'a> {
    light: &'a wgpu::RenderPipeline,
    darken: &'a wgpu::RenderPipeline,
    group: &'a wgpu::BindGroup,
    /// The first this many darken.
    darkening: u32,
    count: u32,
}

impl DecalDraw<'_> {
    /// Binds `frame`, the world's frame group, as group 0, and its own group 2; group 1 stays as the main pass
    /// bound it. Darkening first, then light. What it drew: a call a blend, a quad of two triangles a decal.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, frame: &wgpu::BindGroup) -> PassCounts {
        let mut counts = PassCounts::default();
        pass.set_bind_group(0, frame, &[]);
        pass.set_bind_group(2, self.group, &[]);
        if self.darkening > 0 {
            pass.set_pipeline(self.darken);
            pass.draw(0..4, 0..self.darkening);
            counts = counts.plus(&PassCounts::call(self.darkening, 2));
        }
        if self.count > self.darkening {
            pass.set_pipeline(self.light);
            pass.draw(0..4, self.darkening..self.count);
            counts = counts.plus(&PassCounts::call(self.count - self.darkening, 2));
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec3};

    use super::*;

    fn decal(shape: VfxDecalShape) -> VfxDecal {
        VfxDecal {
            shape,
            position: Vec3::new(1.0, 0.0, 2.0),
            rotation: Quat::IDENTITY,
            color: Vec3::new(2.0, 0.5, 0.1),
            alpha: 1.0,
            blend: VfxDecalBlend::Light,
            fill: 0.5,
            outline: 0.1,
            body: 0.3,
            noise: 0.0,
        }
    }

    /// The quad reaches past the shape by its margin and its noise, a sector's sine and cosine are its half
    /// angle's, and what isn't sound is made safe or left out.
    #[test]
    fn a_decal_packs_its_shape_and_bounds_and_bad_values_are_made_safe() {
        let disc = GpuVfxDecal::new(&decal(VfxDecalShape::Disc { radius: 2.0 })).expect("packed");
        let margin = MARGIN.max(MARGIN_SHARE * 4.0);
        assert_eq!(disc.bounds, [-2.0 - margin, -2.0 - margin, 2.0 + margin, 2.0 + margin]);
        assert_eq!((disc.kind, disc.shape[0]), (DISC, 2.0));
        assert_eq!(disc.world_from_local[3], [1.0, 0.0, 2.0, 1.0]);
        let cone = decal(VfxDecalShape::Sector { radius: 3.0, angle: 90.0, inner: 0.5 });
        let packed = GpuVfxDecal::new(&VfxDecal { noise: 0.2, ..cone }).expect("packed");
        let half = std::f32::consts::FRAC_PI_4;
        assert!((packed.shape[1] - half.sin()).abs() < 1e-6 && (packed.shape[2] - half.cos()).abs() < 1e-6);
        assert_eq!((packed.kind, packed.shape[3], packed.noise), (SECTOR, 0.5, 0.2));
        // Its widest is across, 6 sin 45° = 4.24 m, so the margin is that share of it past its noise.
        let across = 6.0 * half.sin();
        assert!((packed.bounds[1] - (-0.2 - MARGIN_SHARE * across)).abs() < 1e-5, "{:?}", packed.bounds);
        let lane = GpuVfxDecal::new(&decal(VfxDecalShape::Lane { width: 1.0, length: 4.0 })).expect("packed");
        assert_eq!((lane.kind, lane.shape[0], lane.shape[1]), (LANE, 1.0, 4.0));
        let wild = VfxDecal {
            alpha: 7.0,
            fill: f32::NAN,
            color: Vec3::splat(f32::INFINITY),
            ..decal(VfxDecalShape::Disc { radius: 1.0 })
        };
        let safe = GpuVfxDecal::new(&wild).expect("packed");
        assert!(safe.alpha == 1.0 && safe.fill.is_finite() && safe.color.iter().all(|c| c.is_finite()));
        assert!(GpuVfxDecal::new(&decal(VfxDecalShape::Disc { radius: 0.0 })).is_none());
        assert!(GpuVfxDecal::new(&decal(VfxDecalShape::Lane { width: f32::NAN, length: 1.0 })).is_none());
        let lost = VfxDecal { position: Vec3::splat(f32::NAN), ..decal(VfxDecalShape::Disc { radius: 1.0 }) };
        assert!(GpuVfxDecal::new(&lost).is_none());
    }

    #[test]
    fn the_gpu_decal_is_laid_out_as_the_shader_reads_it() {
        assert_eq!(size_of::<GpuVfxDecal>(), 144);
        assert_eq!(size_of::<GpuVfxDecal>() % 16, 0);
        assert_eq!(size_of::<GpuVfxDecalUniforms>(), 16);
    }
}
