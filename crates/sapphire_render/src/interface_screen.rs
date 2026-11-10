//! A screen drawn by the interface's one pipeline alone (M4 H2): what a game shows before it has a renderer.
//!
//! A [`Renderer`](crate::Renderer) builds its passes' pipelines as it's made: on Desktop-High's cold Metal cache
//! about a second of them, more on a phone, and then the packs' and the first zone's, seconds more, on the
//! warm-up pool (M4 H0, H1). A boot screen has to draw from the first frame, so it can't wait for any of that.
//! This draws a frame's [`UiDrawList`] over a cleared target and nothing else, through the same pipeline and
//! shader the renderer's interface uses ([`crate::ui`]), built on the calling thread as this is made: one
//! pipeline, before the screen's first frame, which a load counts as prewarmed. The renderer's own interface
//! builds its copy on the warm-up pool, against the cache this one has just warmed.
//!
//! It keeps its own glyph pages ([`InterfaceScreen::page_state`]): a screen's glyph atlas starts over when it
//! moves to the renderer, as it does for any new renderer.

use sapphire_look::{UiDrawList, UiPageState};
use sapphire_rhi::{GpuContext, wgpu};

use crate::graph::{FrameGraph, TextureId};
use crate::inset::MAX_INSETS;
use crate::pipeline_builds::{PipelineBuilds, PipelineStats};
use crate::shaders::ShaderSources;
use crate::ui::{Ui, UiStats};

/// What the staging belt maps at a time: a screen's quads are a few kilobytes.
const UPLOAD_CHUNK_SIZE: u64 = 64 * 1024;

/// Draws an interface over a cleared target, with nothing under it: what a game shows before it has a renderer.
#[derive(Debug)]
pub struct InterfaceScreen {
    ui: Ui,
    builds: PipelineBuilds,
    uploads: wgpu::util::StagingBelt,
    /// What the target's cleared to under the interface, in linear light.
    clear: wgpu::Color,
    labels: bool,
}

impl InterfaceScreen {
    /// Builds the interface's pipeline for targets of `color_format` (one that encodes on write, as a
    /// renderer's), now, on this thread, with the device's pipeline cache: before the screen's first frame.
    #[must_use]
    pub fn new(gpu: &GpuContext, color_format: wgpu::TextureFormat) -> Self {
        let device = gpu.device();
        let builds = PipelineBuilds::new(gpu.pipeline_cache());
        let labels = gpu.label("").is_some();
        let mut ui = Ui::new(device, color_format, builds.clone(), labels);
        {
            // The screen's load, not a first use.
            let _warming = builds.warming();
            ui.build(&ShaderSources::compiled_in());
        }
        Self {
            ui,
            builds,
            uploads: wgpu::util::StagingBelt::new(device.clone(), UPLOAD_CHUNK_SIZE),
            clear: wgpu::Color::BLACK,
            labels,
        }
    }

    /// What the target's cleared to under the interface, in linear light: black unless said.
    pub fn set_clear(&mut self, clear: wgpu::Color) {
        self.clear = clear;
    }

    /// What this screen's glyph pages hold, for the list's atlas to start its frame from.
    #[must_use]
    pub fn page_state(&self) -> UiPageState {
        self.ui.page_state()
    }

    /// Takes the frame's list: its glyph pages, its atlas and fields, and its quads for [`Self::record`].
    pub fn prepare(&mut self, gpu: &GpuContext, list: &UiDrawList) {
        self.ui.prepare(gpu.queue(), list);
    }

    /// Adds the frame's passes over `target`, `size` pixels: its clear, the quads' upload and the interface.
    pub fn record<'f>(&'f mut self, graph: &mut FrameGraph<'f>, target: TextureId, size: (u32, u32)) {
        let (clear, labels) = (self.clear, self.labels);
        graph.add_pass("interface screen clear").write(target).record(move |ctx| {
            ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some("interface screen clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.view(target),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
        });
        let (ui, uploads) = (&self.ui, &mut self.uploads);
        if let Some(upload) = ui.uploads() {
            graph.add_pass("interface screen uploads").side_effect().record(move |ctx| {
                upload.write(uploads, ctx.encoder);
                uploads.finish_and_recall_on_submit(ctx.encoder);
            });
        }
        ui.record(graph, target, size, std::array::from_fn::<_, MAX_INSETS, _>(|_| None));
    }

    /// The interface's counters, as a renderer's.
    #[must_use]
    pub fn stats(&self) -> UiStats {
        self.ui.stats()
    }

    /// What it built: its one pipeline.
    #[must_use]
    pub fn pipelines(&self) -> PipelineStats {
        self.builds.stats()
    }
}
