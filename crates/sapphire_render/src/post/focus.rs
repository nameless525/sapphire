//! Depth of field (M5 L8; spec §5.12, "Depth of field"): authored focus on the desktop tiers'
//! main view, and a soft backdrop behind an inset's characters where a tier has it. `shaders/focus.wgsl` has
//! the passes; this module has what a tier turns on, the constants and the passes' names.
//!
//! # The main view (Desktop-High and Desktop-Low)
//!
//! Three fullscreen passes after temporal AA's resolve and the effects drawn over it, in HDR, before the bloom
//! chain and the fused pass, which read the composite's output as their input and are otherwise unchanged:
//!
//! 1. **Prepare** (½ size): each texel takes the four input pixels under it, each with its signed blur from
//!    the scene prepass's depth: the least of its 3 × 3 neighbours', so a pixel of a sharp subject's
//!    antialiased edge, which holds some of the subject's light over the depth behind it, counts as sharp.
//!    A pixel weighs 1/1000 plus its blur squared, so a texel across a sharp edge takes the blurred side's
//!    light and blur, and the subject's light never rides a blurred texel into the blur behind it.
//! 2. **Gather** (½ size): 1 + 8 + 16 + 24 whole-texel taps on three rings out to the texel's own blur (a
//!    bilinear tap would mix a sharp texel's light into a blurred one's), each weighted by how far its own
//!    blur reaches its distance: what is blurred behind takes no more than its own blur from a tap, so it
//!    never spreads over a nearer surface, what is blurred in front spreads by its own, and a sharp tap
//!    spreads nowhere. Each tap counts over its blur's area, as a scatter would spread it, so where a smaller
//!    blur meets a larger its light keeps its share. A sharp texel gathers nothing.
//! 3. **Composite** (the input's size): each pixel mixes the gathered half size back by its own blur, from
//!    none within half a pixel to all of it from one and a half, upsampled with weights halved for each
//!    quarter of a half-size pixel a texel's blur differs from its own. Its blur is the least of its own and
//!    its four neighbours', since temporal AA's depth
//!    jitters by under a pixel while its resolve holds still: an edge's pixels never alternate between sharp
//!    and blurred, at the cost of a pixel-wide band of the blurred side kept sharp under the subject's line.
//!
//! A sharp pixel is post's input bit for bit, so a frame without focus, or the sharp range of one with it,
//! is unchanged. Depth comes from the scene prepass, which temporal AA keeps: so without it the main view
//! has no focus ([`DepthOfFieldSettings::with_temporal`]). What has no depth there (the effects drawn after the resolve,
//! transparent surfaces, the sky at infinity) blurs as what is behind it does: a spark against a soft
//! backdrop goes soft with it, which reads as part of that backdrop, and a spark in front of a sharp subject
//! stays sharp. A focus's largest blur is a fraction of the picture's height, so a key blurs alike at any
//! output size, and the taps are as many at any blur, so a frame costs the same whatever it asks.
//!
//! # An inset's soft backdrop (Desktop-High, Desktop-Low and Mobile-High)
//!
//! An inset that asks for it ([`InsetView::soft_backdrop`](crate::InsetView::soft_backdrop)) blurs what of
//! its scene isn't a character before its resolve, one pass at its scene's size: insets draw no prepass, so
//! the character mask in scene colour's alpha tells the backdrop apart, each tap counting by its world share,
//! and a character's pixels are its scene's bit for bit.
//!
//! # Costs nothing while off
//!
//! A tier without it ([`DepthOfFieldSettings`], Mobile-Low's, and Mobile-High's main view) builds none of its
//! pipelines and ignores a focus, with no warning. A tier with it builds them only when an inventory declares
//! focus (`PipelineInventory::focus`) or a frame first draws one, and a frame without focus dispatches
//! nothing. The half-size textures are the transient pool's, so they come with the first frame that draws
//! focus and stay in its spares while frames go on without it (the pool's own "budget"), and a focus that
//! comes and goes allocates nothing past the first.

use bevy_reflect::Reflect;
use sapphire_look::Focus;
use serde::{Deserialize, Serialize};

/// The passes' names in the frame graph, in the order they run: what the overlay and the bench time.
pub const PREPARE_PASS: &str = "focus prepare";
pub const GATHER_PASS: &str = "focus gather";
pub const COMPOSITE_PASS: &str = "focus composite";
/// The main view's passes.
pub const PASSES: [&str; 3] = [PREPARE_PASS, GATHER_PASS, COMPOSITE_PASS];
/// An inset's soft backdrop's pass, before its resolve.
pub const BACKDROP_PASS: &str = "inset soft backdrop";

/// What of depth of field a tier has ([`RenderSettings::depth_of_field`](crate::RenderSettings::depth_of_field)).
/// Off, the default, has none: no pipelines and no passes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(deny_unknown_fields)]
pub struct DepthOfFieldSettings {
    /// The main view's focus: the desktop tiers'. It reads the scene prepass's depth, which only temporal AA keeps: without it, none.
    #[serde(default)]
    pub view: bool,
    /// Insets' soft backdrops: the desktop tiers' and Mobile-High's.
    #[serde(default)]
    pub insets: bool,
}

impl DepthOfFieldSettings {
    /// None of it.
    pub const OFF: Self = Self { view: false, insets: false };

    /// What of it a renderer with or without temporal AA draws: the main view's focus reads the scene
    /// prepass's depth, which only temporal AA keeps, so without it the main view has none. A tier with
    /// focus run with MSAA (`--aa msaa`, as the older goldens pin it) draws as a tier without it.
    pub const fn with_temporal(self, temporal: bool) -> Self {
        Self { view: self.view && temporal, insets: self.insets }
    }

    /// Whether it has anything.
    pub const fn any(&self) -> bool {
        self.view || self.insets
    }
}

/// One of depth of field's pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusPass {
    Prepare,
    Gather,
    Composite,
    /// An inset's soft backdrop.
    Backdrop,
}

impl FocusPass {
    pub(super) const ALL: [Self; 4] = [Self::Prepare, Self::Gather, Self::Composite, Self::Backdrop];

    /// Its name, its fragment stage's entry point.
    pub(super) const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Prepare => (PREPARE_PASS, "fs_focus_prepare"),
            Self::Gather => (GATHER_PASS, "fs_focus_gather"),
            Self::Composite => (COMPOSITE_PASS, "fs_focus_composite"),
            Self::Backdrop => (BACKDROP_PASS, "fs_soft_backdrop"),
        }
    }
}

impl DepthOfFieldSettings {
    /// Whether a tier with these draws with `pass`.
    pub(super) const fn has(&self, pass: FocusPass) -> bool {
        match pass {
            FocusPass::Prepare | FocusPass::Gather | FocusPass::Composite => self.view,
            FocusPass::Backdrop => self.insets,
        }
    }
}

/// The constants a frame's `focus` takes in post's uniforms (`focus` and `focus_blur` in `post.wgsl`), with
/// the camera's near plane at `near`: `None` if it blurs nothing.
pub(super) fn constants(focus: &Focus, near: f32) -> Option<([f32; 4], [f32; 4])> {
    let focus = focus.sanitized();
    if !focus.drawn() || !(near.is_finite() && near > 0.0) {
        return None;
    }
    let plane = [focus.distance, focus.sharp, 1.0 / focus.near, 1.0 / focus.far];
    Some((plane, [focus.blur, near, 0.0, 0.0]))
}

/// An inset's soft backdrop's blur as its constants hold it, a fraction of its scene's height held to the
/// largest a focus may ask for: 0 for none, or for what isn't a positive number.
pub(crate) fn backdrop_blur(blur: f32) -> f32 {
    if blur.is_finite() && blur > 0.0 { blur.min(sapphire_look::MAX_FOCUS_BLUR) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tier_has_the_passes_of_what_it_turns_on() {
        let of = |settings: DepthOfFieldSettings| {
            FocusPass::ALL.into_iter().filter(|pass| settings.has(*pass)).collect::<Vec<_>>()
        };
        assert!(of(DepthOfFieldSettings::OFF).is_empty());
        let desktop = DepthOfFieldSettings { view: true, insets: true };
        assert_eq!(of(desktop), FocusPass::ALL);
        let mobile_high = DepthOfFieldSettings { view: false, insets: true };
        assert_eq!(of(mobile_high), [FocusPass::Backdrop]);
        assert_eq!(desktop.with_temporal(true), desktop);
        assert_eq!(desktop.with_temporal(false), mobile_high);
        assert_eq!(mobile_high.with_temporal(false), mobile_high);
        assert_eq!(DepthOfFieldSettings::default(), DepthOfFieldSettings::OFF);
    }

    #[test]
    fn constants_hold_the_focus_and_draw_nothing_without_one() {
        let focus = Focus { distance: 4.0, sharp: 0.5, near: 2.0, far: 8.0, blur: 0.01 };
        assert_eq!(constants(&focus, 0.1), Some(([4.0, 0.5, 0.5, 0.125], [0.01, 0.1, 0.0, 0.0])));
        assert_eq!(constants(&Focus::NONE, 0.1), None);
        assert_eq!(constants(&focus, 0.0), None, "no near plane, no depth to read");
        let wild = Focus { blur: 1.0, ..focus };
        assert_eq!(constants(&wild, 0.1).unwrap().1[0], sapphire_look::MAX_FOCUS_BLUR);
        assert_eq!(backdrop_blur(0.004), 0.004);
        assert_eq!(backdrop_blur(f32::NAN), 0.0);
        assert_eq!(backdrop_blur(-1.0), 0.0);
        assert_eq!(backdrop_blur(1.0), sapphire_look::MAX_FOCUS_BLUR);
    }
}
