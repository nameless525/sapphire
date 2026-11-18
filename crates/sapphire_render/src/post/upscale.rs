//! The phone tiers' upscaler (M5 L14, spec §5.12 "Upscaling"): where the scene is smaller than the output and no
//! temporal AA upsamples it, the fused pass writes the scene's size, and two passes take it to the output's,
//! [`EDGE_PASS`] and [`SHARPEN_PASS`] (`post_upscale.wgsl`). The tier's [`Upscaler`] says which: Mobile-High and
//! Mobile-Low upscale along edges and sharpen; any other tier, or [`Upscaler::Bilinear`], keeps the fused pass's
//! own bilinear read of a smaller scene, at the output's size.
//!
//! - **The method** is AMD FidelityFX Super Resolution 1.0's two passes (MIT; the notice is at the shader's head
//!   and in `content/third_party/amd_fsr1/`), written here from its published description: EASU's twelve taps,
//!   the four nearest texels' votes for an edge's direction and length, its windowed kernel stretched along the
//!   edge and its clamp to the four nearest; then RCAS's contrast-limited lobe over four neighbours. Ours, not
//!   FSR 1's: the sharpen's result is held within its five pixels' own range, so it can't overshoot an edge (no
//!   halo beside a dark line, no ring on a two-tone edge); FSR 1's noise detection is left out; the sharpen fades
//!   to nothing as the scale nears 1 ([`FADE`]), where there is less to restore.
//! - **Encoding:** the fused pass writes its usual display light into a transient of the target's own format, so
//!   no pipeline of it changes; the upscale reads it as linear, works on its square root (gamma 2, which FSR 1's
//!   weights assume) and writes that at 10 bits a channel ([`FORMAT`]); the sharpen squares it back into the
//!   target. Two passes, as FSR 1 has: the sharpen reads the upscale's neighbours, which one pass can't.
//! - **Nothing at scale 1, nothing on the desktop tiers:** a frame whose scene is the output's size draws neither
//!   pass and creates neither target, and a tier without the upscaler builds neither pipeline nor buffer.
//! - **Built with the renderer** on a tier that upscales along edges, as the compositing variants are: a phone's
//!   frame is below scale 1 from its first frame at any phone's height (the tiers' height caps put it there), so
//!   no frame and no step of the thermal ladder builds them; a tier change in place brings them with the new
//!   renderer.
//! - **The flat path (M6 F1):** where a pixel's four nearest texels are equal, the upscale returns that texel at
//!   once, which is what its clamp to their range would have made of any kernel: the same to the bit, skipping
//!   the votes and the kernel where a 32-pixel group of the GPU's is wholly flat (the shader says how it's taken,
//!   and why it still reads all twelve taps first).
//! - **What draws after:** the interface, debug draw, the debug views and the colour-vision view draw over the
//!   target at the output's size, after the sharpen, untouched. The screen effects (the CRT's scanlines and
//!   grille, the glitch's whole-pixel shifts, the halftone) are the fused pass's, so on these tiers they're drawn
//!   at the scene's pixels and upscaled with it, as everything else in the frame is.

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

/// The edge-adaptive upscale's pass, from the fused pass's output at the scene's size into `Rgb10a2Unorm` at the
/// output's.
pub const EDGE_PASS: &str = "upscale";
/// The sharpen's pass, from the upscale's output into the target.
pub const SHARPEN_PASS: &str = "upscale sharpen";
/// Both, in the order a frame draws them: what the bench and the overlay sum for the upscaler's GPU time.
pub const PASSES: [&str; 2] = [EDGE_PASS, SHARPEN_PASS];

/// The upscale's output: perceptual values (the square root of linear light), 10 bits a channel, renderable and
/// filterable everywhere.
pub(crate) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgb10a2Unorm;

/// Over how much of the scale below 1 the sharpen fades in: none at 1, the tier's whole strength at 0.75 and
/// below, where every phone tier's own scale and its ladder's rungs fall.
pub const FADE: f32 = 0.25;

/// How a frame smaller than its output is taken to it without temporal AA (M5 L14,
/// [`RenderSettings::upscaler`](crate::RenderSettings::upscaler)). Temporal AA's resolve upsamples on its own, so
/// a tier with it never draws this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum Upscaler {
    /// The fused pass reads the scene bilinearly at the output's size: no pass, target or pipeline of the
    /// upscaler's. The default, and what a desktop tier has.
    #[default]
    Bilinear,
    /// The edge-adaptive upscale and the sharpen after the fused pass: the phone tiers'.
    Edge {
        /// The sharpen's strength, as a percentage of its strongest lobe: 0 to 100.
        sharpen: u8,
    },
}

impl Upscaler {
    /// Whether it upscales along edges.
    pub const fn edge(self) -> bool {
        matches!(self, Self::Edge { .. })
    }

    /// What a renderer with temporal AA (`temporal`) draws: bilinear, whose read its resolve never needs.
    pub const fn without_temporal(self, temporal: bool) -> Self {
        if temporal { Self::Bilinear } else { self }
    }

    /// Checks the sharpen's strength.
    pub(crate) fn validate(self) -> Result<(), (&'static str, &'static str)> {
        match self {
            Self::Edge { sharpen } if sharpen > 100 => Err(("upscaler.sharpen", "must be 0 to 100")),
            _ => Ok(()),
        }
    }

    /// The sharpen's lobe scale for an input of `input` texels drawn into `output`'s: the strength faded in over
    /// the last quarter of scale below 1. 0 for bilinear.
    pub fn sharpen_at(self, input: (u32, u32), output: (u32, u32)) -> f32 {
        let Self::Edge { sharpen } = self else { return 0.0 };
        let ratio = |input: u32, output: u32| input as f32 / output.max(1) as f32;
        let scale = ratio(input.0, output.0).max(ratio(input.1, output.1));
        f32::from(sharpen.min(100)) / 100.0 * ((1.0 - scale) / FADE).clamp(0.0, 1.0)
    }
}

/// The upscaler's constants (WGSL `UpscaleUniforms` in `post_upscale.wgsl`, group 0 binding 2).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct UpscaleUniforms {
    /// xy: the input's size over the output's; zw: -0.5, so a pixel's center (its position, which holds the
    /// half) times xy plus zw is its place among the input's texel centers.
    pub easu: [f32; 4],
    /// x: the sharpen's lobe scale ([`Upscaler::sharpen_at`]).
    pub sharpen: [f32; 4],
    /// xy: one input texel in texture coordinates, the upscale's step between taps; zw: one output pixel, the
    /// sharpen's.
    pub texel: [f32; 4],
}

impl UpscaleUniforms {
    pub(crate) fn new(upscaler: Upscaler, input: (u32, u32), output: (u32, u32)) -> Self {
        let (input_x, input_y) = (input.0.max(1) as f32, input.1.max(1) as f32);
        let (output_x, output_y) = (output.0.max(1) as f32, output.1.max(1) as f32);
        Self {
            easu: [input_x / output_x, input_y / output_y, -0.5, -0.5],
            sharpen: [upscaler.sharpen_at(input, output), 0.0, 0.0, 0.0],
            texel: [1.0 / input_x, 1.0 / input_y, 1.0 / output_x, 1.0 / output_y],
        }
    }
}

/// One of the upscaler's two pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpscalePass {
    Edge,
    Sharpen,
}

impl UpscalePass {
    pub(super) const ALL: [Self; 2] = [Self::Edge, Self::Sharpen];

    /// Its pass's name and its fragment shader's entry point.
    pub(super) const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Edge => (EDGE_PASS, "fs_upscale_edge"),
            Self::Sharpen => (SHARPEN_PASS, "fs_upscale_sharpen"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sharpen_fades_in_below_scale_one() {
        let edge = Upscaler::Edge { sharpen: 80 };
        let output = (1600, 1170);
        assert_eq!(edge.sharpen_at(output, output), 0.0);
        assert_eq!(Upscaler::Bilinear.sharpen_at((800, 585), output), 0.0);
        // Every phone tier's own scale and its rungs are below 0.75: the whole strength.
        assert!((edge.sharpen_at((800, 585), output) - 0.8).abs() < 1e-6);
        let near = edge.sharpen_at((1520, 1112), output);
        assert!(near > 0.0 && near < 0.2, "{near}");
        // The larger ratio of the two sides decides, so a rounded side can't sharpen a frame at scale 1.
        assert_eq!(edge.sharpen_at((1600, 1100), output), 0.0);
        assert!(Upscaler::Edge { sharpen: 101 }.validate().is_err());
        assert!(Upscaler::Edge { sharpen: 100 }.validate().is_ok());
        assert_eq!(edge.without_temporal(true), Upscaler::Bilinear);
        assert_eq!(edge.without_temporal(false), edge);
    }

    #[test]
    fn the_kernel_starts_at_each_pixels_place_among_the_texels() {
        let uniforms = UpscaleUniforms::new(Upscaler::Edge { sharpen: 50 }, (800, 585), (1600, 1170));
        let [x, y, ox, oy] = uniforms.easu;
        // Output pixel 0's center (0.5) falls a quarter of a texel before texel 0's center, and pixel 1's a
        // quarter after it: two pixels a texel, either side of its center.
        assert_eq!((x, y), (0.5, 0.5));
        assert_eq!((0.5 * x + ox, 0.5 * y + oy), (-0.25, -0.25));
        assert_eq!((1.5 * x + ox, 1.5 * y + oy), (0.25, 0.25));
        assert_eq!(uniforms.sharpen[0], 0.5);
        assert_eq!(uniforms.texel, [1.0 / 800.0, 1.0 / 585.0, 1.0 / 1600.0, 1.0 / 1170.0]);
    }
}
