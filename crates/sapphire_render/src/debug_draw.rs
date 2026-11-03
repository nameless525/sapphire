//! Drawing debug draws (spec §5.20, M2 T1): the lines of the packet's
//! [`DebugDrawList`], which `sapphire_core::debug_draw` merges from any thread (rule R4: the
//! renderer sees plain lines alone). Labels aren't drawn here: the debug overlay paints them
//! (`sapphire_debug`, dev builds), and this pass only counts them in its stats.
//!
//! # A frame with nothing costs nothing
//!
//! A renderer holds no pipeline, buffer or bind group for this until the first frame with lines:
//! every frame before it is one comparison, adds no pass and reads no prepass. That first frame
//! builds the one pipeline on the calling thread (a cold Metal compile's hitch, for a tool), and the
//! scene prepass's, on a tier that doesn't otherwise read that pass. After it, a frame with no lines
//! adds no pass again.
//!
//! # How lines draw
//!
//! **Decision: thick lines in screen space, not the line topology.** Each line is an instanced
//! quad, a rectangle on screen of [`LINE_WIDTH`] pixels at 720 rows and wider in step with the
//! target's height, around a capsule with a one pixel analytic edge. wgpu's line topology has no
//! width, so its hairlines thin out as a display's density rises and on a phone can't be read; it
//! rejects depth bias, and backends rasterize it differently (F5's wireframe differs 47% between
//! lavapipe and Metal), which golden images then have to take on trust. The quads are computed
//! the same everywhere and move by fractions of a pixel. A line crossing the near plane is cut
//! there first, in the vertex stage, since the screen direction needs both ends in front of the
//! camera.
//!
//! **Decision: after post, over the finished frame** and after the debug views, like them. Colors
//! are what the caller named, at any hour: no exposure, grade or bloom reaches them. Temporal AA
//! neither smears nor jitters them, so they hold still (they draw through the main view without
//! its jitter), and the render scale doesn't soften them. The overlay draws over them.
//!
//! **Depth-tested lines read the scene prepass's depth.** The main pass's depth is transient, and
//! under TAA the prepass's depth is the one that lives on, so each line's fragment compares its
//! own distance with that depth (`debug_line_visibility`). It's compared in the shader rather than
//! through a depth attachment because it has the scene's size, which at a render scale under 1
//! isn't the target's: there the test is filtered over four texels, so an edge doesn't step by
//! whole texels. Reading it makes the prepass run on a frame whose antialiasing wouldn't otherwise
//! need it; a frame with no lines doesn't.
//!
//! **What depth tests honestly and what it doesn't.** That depth holds opaque surfaces of every
//! family, characters (posed, with their cut-outs and fades' dither), foliage (with its wind and
//! cut-outs) and outline hulls. So:
//! - Transparent surfaces and fog cards write no depth: a line behind one draws over it, whole.
//! - The environment's alpha-tested surfaces aren't cut out of that prepass (crate docs, "Later
//!   milestones"): their holes hide a line behind them.
//! - A character's outline hull hides what is behind it, so a tested line stops a line's width
//!   short of its silhouette.
//! - Under TAA that depth jitters, so where a tested line goes behind a silhouette its end moves
//!   by up to a pixel from frame to frame. A wider filter only spread the same flicker over more
//!   pixels (3 × 3 texels: three times the pixels at a quarter of the step), so it stays this
//!   simple. Everywhere else lines hold still.
//! - Lines aren't fogged: a far line through fog reads at full strength.
//! - A line within a centimeter, and 0.2% of its distance, behind a surface still shows, so what
//!   rests on a surface doesn't fight it.
//!
//! [`Depth::OnTop`](sapphire_core::debug_draw::Depth) skips all of it, and reads through anything.

use sapphire_core::debug_draw::{DebugDrawList, DebugDrawStats, DebugLine, Depth};
use sapphire_rhi::wgpu;

use crate::CameraData;
use crate::buffer::{GrowableBuffer, stage};
use crate::graph::{FrameGraph, TextureId};
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::shader_builds::Step;
use crate::shaders::{self, Program, ShaderSources};

/// The pass in the frame graph, for GPU timings.
pub const PASS: &str = "debug draw";

/// A line's width in pixels on a target of [`LINE_WIDTH_ROWS`] rows or fewer; taller targets widen
/// it in step, so it reads alike at any resolution and density.
pub const LINE_WIDTH: f32 = 1.5;

/// The rows [`LINE_WIDTH`] is for.
pub const LINE_WIDTH_ROWS: f32 = 720.0;

/// A line's width, in pixels, on a target `rows` tall.
pub fn line_width(rows: u32) -> f32 {
    LINE_WIDTH * (rows as f32 / LINE_WIDTH_ROWS).max(1.0)
}

/// The `flags` bit of a line drawn over everything (`DEBUG_DRAW_ON_TOP` in `debug_draw.wgsl`).
const ON_TOP: u32 = 1;

/// What the vertex buffer starts with: about 2,000 lines.
const INITIAL_BYTES: u64 = 64 * 1024;

/// The frame's debug draws (rule R9), in [`RenderStats::debug_draw`](crate::RenderStats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DebugDrawPassStats {
    /// What the packet's list held and what the collector lost.
    pub list: DebugDrawStats,
    /// Lines the frame draws: the list's, but those past the vertex buffer's limit.
    pub drawn: u32,
    /// Lines past the vertex buffer's limit (the device's largest buffer), which don't draw.
    pub over_limit: u32,
    /// Pipelines built for them: 0 until a frame has lines, then 1.
    pub pipelines: u32,
    /// Bytes of its vertex buffer: none until a frame has lines.
    pub buffer_bytes: u64,
}

/// One line as the vertex stage reads it, an instance (WGSL `DebugLineInput`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuLine {
    start: [f32; 3],
    /// Display (sRGB-encoded) RGBA, as `Unorm8x4`.
    color: [u8; 4],
    end: [f32; 3],
    flags: u32,
}

impl From<&DebugLine> for GpuLine {
    fn from(line: &DebugLine) -> Self {
        let color = [line.color.r, line.color.g, line.color.b, line.color.a];
        let flags = if line.depth == Depth::OnTop { ON_TOP } else { 0 };
        Self { start: line.from.to_array(), color, end: line.to.to_array(), flags }
    }
}

const LINE_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Unorm8x4,
    2 => Float32x3,
    3 => Uint32,
];

/// The frame's constants (WGSL `DebugDrawUniforms`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct DebugDrawUniforms {
    clip_from_world: [[f32; 4]; 4],
    target_size: [f32; 2],
    scene_scale: [f32; 2],
    half_width: f32,
    near: f32,
    padding: [f32; 2],
}

impl DebugDrawUniforms {
    /// For `camera`'s unjittered view, a scene of `scene` pixels and a target of `target` pixels.
    pub(crate) fn new(camera: &CameraData, scene: (u32, u32), target: (u32, u32)) -> Self {
        let size = |pixels: (u32, u32)| [pixels.0 as f32, pixels.1 as f32];
        let (scene, target) = (size(scene), size(target));
        Self {
            clip_from_world: (camera.proj * camera.view).to_cols_array_2d(),
            target_size: target,
            scene_scale: [scene[0] / target[0], scene[1] / target[1]],
            half_width: line_width(target[1] as u32) * 0.5,
            // A reverse-Z projection with its far plane at infinity gives clip z = near.
            near: camera.proj.w_axis.z,
            padding: [0.0; 2],
        }
    }
}

/// The pass's pipeline and buffers, made by the first frame with lines.
#[derive(Debug)]
pub(crate) struct DebugDraw {
    device: wgpu::Device,
    color_format: wgpu::TextureFormat,
    builds: PipelineBuilds,
    bind_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    buffer: GrowableBuffer,
    /// The frame's lines, packed, kept to reuse their room.
    lines: Vec<GpuLine>,
    over_limit: u32,
}

impl DebugDraw {
    /// Builds its pipeline from `sources`, on this thread.
    pub(crate) fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        sources: &ShaderSources,
        builds: &PipelineBuilds,
    ) -> Self {
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(PASS),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<DebugDrawUniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(PASS),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = build_pipeline(device, &pipeline_layout, color_format, sources, builds);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(PASS),
            size: size_of::<DebugDrawUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let buffer = GrowableBuffer::new(
            device,
            PASS,
            wgpu::BufferUsages::VERTEX,
            INITIAL_BYTES,
            device.limits().max_buffer_size,
        );
        Self {
            device: device.clone(),
            color_format,
            builds: builds.clone(),
            bind_layout,
            pipeline_layout,
            pipeline,
            uniforms,
            buffer,
            lines: Vec::new(),
            over_limit: 0,
        }
    }

    /// Takes the frame's lines, making room for them, up to what one buffer holds.
    pub(crate) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, lines: &[DebugLine]) {
        profiling::scope!("DebugDraw::prepare");
        let bytes = size_of::<GpuLine>() as u64;
        self.over_limit = pack(lines, self.buffer.max_items(bytes), &mut self.lines);
        self.buffer.reserve(device, queue, self.lines.len() as u64 * bytes, 0);
    }

    /// Has the frame draw no lines.
    pub(crate) fn skip(&mut self) {
        self.lines.clear();
        self.over_limit = 0;
    }

    /// Whether the frame has lines to draw.
    pub(crate) fn has_lines(&self) -> bool {
        !self.lines.is_empty()
    }

    pub(crate) fn stats(&self, list: DebugDrawStats) -> DebugDrawPassStats {
        DebugDrawPassStats {
            list,
            drawn: self.lines.len() as u32,
            over_limit: self.over_limit,
            pipelines: 1,
            buffer_bytes: self.buffer.buffer().size(),
        }
    }

    /// Its pipeline, to build from new sources (shader hot reload, off the frame thread:
    /// `shader_builds`), for [`Self::set_pipeline`] once every changed program built.
    pub(crate) fn rebuild(&self) -> Step<wgpu::RenderPipeline> {
        let (device, layout, builds) =
            (self.device.clone(), self.pipeline_layout.clone(), self.builds.clone());
        let color_format = self.color_format;
        Box::new(move |sources, _| build_pipeline(&device, &layout, color_format, sources, &builds))
    }

    pub(crate) fn set_pipeline(&mut self, pipeline: wgpu::RenderPipeline) {
        self.pipeline = pipeline;
    }

    /// Writes the frame's constants and lines, in the frame's uploads pass.
    pub(crate) fn upload(
        &self,
        belt: &mut wgpu::util::StagingBelt,
        encoder: &mut wgpu::CommandEncoder,
        uniforms: &DebugDrawUniforms,
    ) {
        stage(belt, encoder, &self.uniforms, 0, bytemuck::bytes_of(uniforms));
        stage(belt, encoder, self.buffer.buffer(), 0, bytemuck::cast_slice(&self.lines));
    }

    /// Adds the pass, over `target`, which post (and any debug view) finished. `depth` is the scene
    /// prepass's, which this makes run.
    pub(crate) fn record<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        target: TextureId,
        depth: TextureId,
        labels: bool,
    ) {
        graph.add_pass(PASS).read(depth).read(target).write(target).record(move |ctx| {
            // A new one a frame: the depth's view changes with TAA's pair and the transient pool,
            // and this runs only while something draws.
            let inputs = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(PASS),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(ctx.view(depth)),
                    },
                ],
            });
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some(PASS),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.view(target),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                timestamp_writes: ctx.timestamp_writes(),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &inputs, &[]);
            pass.set_vertex_buffer(0, self.buffer.buffer().slice(..));
            pass.draw(0..4, 0..self.lines.len() as u32);
        });
    }
}

/// Packs `lines` into `out`, up to `max` of them, and returns how many didn't fit.
fn pack(lines: &[DebugLine], max: u64, out: &mut Vec<GpuLine>) -> u32 {
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    out.clear();
    out.extend(lines.iter().take(max).map(GpuLine::from));
    (lines.len() - out.len()) as u32
}

/// Builds the pass's one pipeline: quads blended over the frame by their alpha, with no depth
/// attachment (see the module docs).
fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sources: &ShaderSources,
    builds: &PipelineBuilds,
) -> wgpu::RenderPipeline {
    let module = shaders::module(device, Program::DebugDraw, sources);
    builds.render(
        device,
        PipelineFamily::Debug,
        &wgpu::RenderPipelineDescriptor {
            label: Some(PASS),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_debug_line"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<GpuLine>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &LINE_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_debug_line"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: builds.cache(),
        },
    )
}

/// The list's stats, on a frame or a renderer that has no pass for it.
pub(crate) fn stats_without_pass(list: &DebugDrawList) -> DebugDrawPassStats {
    DebugDrawPassStats { list: list.stats, ..DebugDrawPassStats::default() }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use sapphire_core::debug_draw::Color;

    use super::*;

    fn line(x: f32, depth: Depth) -> DebugLine {
        DebugLine { from: Vec3::splat(x), to: Vec3::ONE, color: Color::rgba(1, 2, 3, 4), depth }
    }

    #[test]
    fn a_line_packs_as_its_attributes_read_it() {
        assert_eq!(size_of::<GpuLine>(), 32);
        let packed = GpuLine::from(&line(2.0, Depth::OnTop));
        assert_eq!(packed.start, [2.0; 3]);
        assert_eq!(packed.color, [1, 2, 3, 4]);
        assert_eq!(packed.end, [1.0; 3]);
        assert_eq!(packed.flags, ON_TOP);
        assert_eq!(GpuLine::from(&line(2.0, Depth::Tested)).flags, 0);
        // Each attribute starts where the last ended, in the struct's order.
        let offsets = LINE_ATTRIBUTES.map(|attribute| attribute.offset as usize);
        let expected = [
            std::mem::offset_of!(GpuLine, start),
            std::mem::offset_of!(GpuLine, color),
            std::mem::offset_of!(GpuLine, end),
            std::mem::offset_of!(GpuLine, flags),
        ];
        assert_eq!(offsets, expected);
    }

    #[test]
    fn lines_past_the_buffers_limit_are_counted_and_the_rest_draw() {
        let lines = [line(0.0, Depth::Tested), line(1.0, Depth::Tested), line(2.0, Depth::OnTop)];
        let mut out = Vec::new();
        assert_eq!(pack(&lines, 8, &mut out), 0);
        assert_eq!(out.len(), 3);
        assert_eq!(pack(&lines, 2, &mut out), 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].start, [1.0; 3]);
        assert_eq!(pack(&[], 2, &mut out), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn lines_widen_with_the_target_past_720_rows() {
        assert_eq!(line_width(360), LINE_WIDTH);
        assert_eq!(line_width(720), LINE_WIDTH);
        assert_eq!(line_width(1440), 2.0 * LINE_WIDTH);
    }

    #[test]
    fn the_constants_read_the_projections_near_plane() {
        let camera = CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.05);
        let uniforms = DebugDrawUniforms::new(&camera, (960, 540), (1920, 1080));
        assert_eq!(uniforms.near, 0.05);
        assert_eq!(uniforms.scene_scale, [0.5, 0.5]);
        assert_eq!(uniforms.target_size, [1920.0, 1080.0]);
        assert_eq!(uniforms.half_width, 0.5 * line_width(1080));
    }
}
