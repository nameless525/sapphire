//! What draws over the finished frame (M6 F1): the interface ([`crate::ui`], M2 I2), then debug draw's lines
//! ([`crate::debug_draw`], M2 T1), in that order, at the output's size, after post and any debug view. (The app's
//! debug overlay is something else: it draws after the frame graph, over all of these.)
//!
//! **Decision: on a frame that upscales along edges, they draw in the sharpen's render pass,** after its
//! fullscreen triangle, rather than in passes of their own that load the target the sharpen has just stored and
//! store it again (m6-plan F1's second step). On the phones' frames that is an output-size load and store fewer:
//! 2532 × 1170 at four bytes a pixel each way. Draws within a render pass run in order and blend over what the
//! pass drew before them exactly as over what a load brought back, so the frame is the same to the bit.
//!
//! - **The colour-vision view** (M4 A5) is a filter of the whole frame, the interface included, so it stays the
//!   frame's last pass of its own: where it is on, the sharpen writes the texture it reads, and these draw there.
//! - **A debug view** (M1 F5) replays the main pass over the finished frame before these draw, so with one on they
//!   keep passes of their own after it, as every frame drew them before.
//! - **Every other frame** keeps them in passes of their own: the fused pass is a frame's last elsewhere, and the
//!   desktop tiers' §3.3 rows time the interface as a pass of its own. Folding them there too is the same hook,
//!   left for when it pays (the interface pass reads 17 to 24 µs on the M5 at either size).
//!
//! In the sharpen's pass their time is the sharpen's: the bench's `gpu_ui_us` and `gpu_debug_draw_us` read
//! nothing on a frame that upscales, and `gpu_upscale_sharpen_us` holds them. Their counts stay in their own rows.
//! **To time them apart,** a dev sandbox's `--measure-unfolded-overlays`
//! ([`RenderSettings::measure_unfolded_overlays`](crate::RenderSettings::measure_unfolded_overlays), M6 B6's style:
//! never a tier's) keeps their passes on every frame: the phones' `gpu ui` row reads again and the upscaler's time
//! leaves them out, for §3.3's rows, while a run without it reads what a phone pays. Chosen over judging the
//! sharpen and the interface against the sum of their two rows on a folded frame: that sum would budget the
//! upscaler's row with the interface's time in it, and F2's and the platform upscalers' comparisons price the
//! upscaler alone.

use sapphire_rhi::wgpu;

use crate::debug_draw::LinesOverlay;
use crate::graph::{FrameGraph, PassContext, TextureId};
use crate::ui::UiOverlay;

/// The draws over one finished frame, in the order they draw.
pub(crate) struct Overlays<'f> {
    pub ui: Option<UiOverlay<'f>>,
    pub lines: Option<LinesOverlay<'f>>,
}

impl<'f> Overlays<'f> {
    /// Them, or `None` where neither draws.
    pub(crate) fn new(ui: Option<UiOverlay<'f>>, lines: Option<LinesOverlay<'f>>) -> Option<Self> {
        (ui.is_some() || lines.is_some()).then_some(Self { ui, lines })
    }

    /// The textures they read besides the target, which the pass they draw in declares: the insets' pictures and
    /// the scene prepass's depth.
    pub(crate) fn reads(&self) -> impl Iterator<Item = TextureId> + '_ {
        let pictures = self.ui.iter().flat_map(UiOverlay::reads);
        pictures.chain(self.lines.map(|lines| lines.depth))
    }

    /// Adds each in a pass of its own over `target`, which keeps what's there and blends over it.
    pub(crate) fn record(self, graph: &mut FrameGraph<'f>, target: TextureId) {
        if let Some(ui) = self.ui {
            ui.record(graph, target);
        }
        if let Some(lines) = self.lines {
            lines.record(graph, target);
        }
    }

    /// What they need from the running pass `ctx`, made before its render pass begins.
    pub(crate) fn bind(self, ctx: &PassContext<'_>) -> BoundOverlays<'f> {
        let lines = self.lines.map(|lines| (lines, lines.bind(ctx.view(lines.depth))));
        BoundOverlays { ui: self.ui, lines }
    }
}

/// [`Overlays`] with what they bind ([`Overlays::bind`]).
pub(crate) struct BoundOverlays<'f> {
    ui: Option<UiOverlay<'f>>,
    lines: Option<(LinesOverlay<'f>, wgpu::BindGroup)>,
}

impl BoundOverlays<'_> {
    /// Draws them into `pass`, over the whole of what it drew.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(ui) = &self.ui {
            ui.draw(pass);
        }
        if let Some((lines, inputs)) = &self.lines {
            lines.draw(pass, inputs);
        }
    }
}
