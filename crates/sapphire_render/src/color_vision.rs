//! The colour-vision debug view (M4 A5, spec §5.12 "Debug views"): the finished frame as a dichromat
//! sees it, for checking that a fight's cues stay apart by shape where colour fails them (m4-plan §3,
//! "Colour"). The app picks one with [`Renderer::set_color_vision`](crate::Renderer::set_color_vision)
//! every frame, from the `r.color_vision` cvar ([`COLOR_VISION`]); the sandbox sets it from
//! `--color-vision`.
//!
//! # How it draws
//!
//! Last of all: post, any other debug view, the interface and debug lines draw into a texture of the
//! target's size and format instead of the target, and one fullscreen pass (`post.wgsl`'s
//! `fs_color_vision`) reads it texel by texel and writes the target through Machado, Oliveira and
//! Fernandes's matrix for the dichromacy ([`sapphire_look::color_vision`]), on linear light: the target's
//! sRGB format decodes as it's read and encodes on write. The HUD's colours are simulated with the
//! world's. The debug overlay draws over it, as it would.
//!
//! **Decision: a filter of the whole frame, apart from `r.debug_view`.** The other views replay the
//! main pass and draw before the interface; this one changes nothing that draws, so it shows over any
//! of them, and a cue's colour is judged as the player gets it, the HUD included.
//!
//! # Off costs nothing
//!
//! Off (the default, and every shipping frame), nothing is built and the frame's graph is what it was.
//! Each dichromacy is a post pipeline of its own, of family `debug`: the matrix is the pipeline's
//! constants. A load with the view on declares it (`PipelineInventory::color_vision`, part of the
//! `debug` entry, M4 H3), so a capture or a bench with it on builds it with the rest; one turned on
//! later builds it on the frame that first shows it, kept for the next time.

use sapphire_core::cvar::{AnyCvar, Cvar, CvarKind, CvarType, CvarValue};
use sapphire_look::Dichromacy;
use sapphire_rhi::{TransientDesc, wgpu};

use crate::counts::{Counter, PassCounts};
use crate::graph::{FrameGraph, TextureId};

/// The view's pass in the frame graph, for GPU timings.
pub const COLOR_VISION_PASS: &str = "color vision";

/// `post.wgsl`'s constants for the matrix, by rows.
pub(crate) const COLOR_VISION_CONSTANTS: [&str; 9] = [
    "COLOR_VISION_00",
    "COLOR_VISION_01",
    "COLOR_VISION_02",
    "COLOR_VISION_10",
    "COLOR_VISION_11",
    "COLOR_VISION_12",
    "COLOR_VISION_20",
    "COLOR_VISION_21",
    "COLOR_VISION_22",
];

/// How the frame is seen (see the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Hash)]
pub enum ColorVision {
    /// As it is.
    #[default]
    Off,
    /// As a dichromat sees it.
    Simulated(Dichromacy),
}

/// The choices' names, as the cvar and the sandbox's `--color-vision` spell them.
const NAMES: [&str; 4] = ["off", "protanopia", "deuteranopia", "tritanopia"];

impl ColorVision {
    pub const ALL: [Self; 4] = [
        Self::Off,
        Self::Simulated(Dichromacy::Protanopia),
        Self::Simulated(Dichromacy::Deuteranopia),
        Self::Simulated(Dichromacy::Tritanopia),
    ];

    /// How the cvar and the command line spell it: `deuteranopia`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => NAMES[0],
            Self::Simulated(dichromacy) => dichromacy.name(),
        }
    }

    /// The choice called `name`, ignoring case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        <Self as CvarType>::parse(name)
    }

    /// The dichromacy simulated, if any.
    #[must_use]
    pub const fn dichromacy(self) -> Option<Dichromacy> {
        match self {
            Self::Off => None,
            Self::Simulated(dichromacy) => Some(dichromacy),
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Off => 0,
            Self::Simulated(dichromacy) => 1 + dichromacy as usize,
        }
    }
}

impl CvarType for ColorVision {
    const KIND: CvarKind = CvarKind::Choice(&NAMES);

    fn to_bits(self) -> u32 {
        self.index() as u32
    }

    fn from_bits(bits: u32) -> Self {
        Self::ALL.get(bits as usize).copied().unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Self> {
        let index = NAMES.iter().position(|name| name.eq_ignore_ascii_case(text))?;
        Some(Self::ALL[index])
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Choice { index: self.index() as u32, names: &NAMES }
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Choice { index, .. } => Self::ALL.get(index as usize).copied(),
            _ => None,
        }
    }
}

/// How the app's frames are seen ([`Renderer::set_color_vision`](crate::Renderer::set_color_vision)).
pub static COLOR_VISION: Cvar<ColorVision> = Cvar::new(
    "r.color_vision",
    ColorVision::Off,
    "Colour-vision debug view (M4 A5): the finished frame, the HUD included, as a protanope, deuteranope or \
     tritanope sees it, by Machado et al. (2009). Off builds and draws nothing.",
);

/// This module's cvars, for the app's registry.
pub static CVARS: [&dyn AnyCvar; 1] = [&COLOR_VISION];

/// What a frame showing the view draws its pass with: post's pipeline for the dichromacy and post's
/// group 0, taken from post when the frame is prepared, so a shader edit's rebuild reaches it.
#[derive(Clone, Debug)]
pub(crate) struct ColorVisionDraw {
    pub device: wgpu::Device,
    pub pipeline: wgpu::RenderPipeline,
    pub shared: wgpu::BindGroup,
    /// Post's group 1 for one input.
    pub layout: wgpu::BindGroupLayout,
}

impl ColorVisionDraw {
    /// A texture for the frame to draw into in `target`'s place, which [`Self::record`] then reads.
    pub(crate) fn input(
        graph: &mut FrameGraph<'_>,
        target: TextureId,
        format: wgpu::TextureFormat,
    ) -> TextureId {
        let (width, height) = graph.size(target);
        graph.create_texture(
            "color vision input",
            TransientDesc {
                width,
                height,
                format,
                sample_count: 1,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            },
        )
    }

    /// Adds the pass that writes `input` into `target` as the dichromacy sees it: the frame's last.
    /// Its draw counts in `counter`.
    pub(crate) fn record<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        input: TextureId,
        target: TextureId,
        labels: bool,
        counter: &'f Counter,
    ) {
        graph.add_pass(COLOR_VISION_PASS).read(input).write(target).record(move |ctx| {
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: labels.then_some(COLOR_VISION_PASS),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(ctx.view(input)),
                }],
            });
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some(COLOR_VISION_PASS),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.view(target),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes: ctx.timestamp_writes(),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.shared, &[]);
            pass.set_bind_group(1, &group, &[]);
            pass.draw(0..3, 0..1);
            counter.add(&PassCounts::fullscreen());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_reads_back_by_its_name() {
        for choice in ColorVision::ALL {
            assert_eq!(ColorVision::from_name(choice.name()), Some(choice));
            assert_eq!(ColorVision::from_bits(choice.to_bits()), choice);
        }
        assert_eq!(
            ColorVision::from_name("Deuteranopia"),
            Some(ColorVision::Simulated(Dichromacy::Deuteranopia))
        );
        assert_eq!(ColorVision::from_name("achromatopsia"), None);
        assert_eq!(ColorVision::default().dichromacy(), None);
    }
}
