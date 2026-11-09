//! Insets (M3 U6): a node that shows a 3D showcase. The tree draws one quad naming the inset's picture,
//! which the renderer draws (`sapphire_render::inset`), and hands the app each shown inset's number,
//! size and turn ([`UiTree::insets`]), for it to place the showcase and push its view. Nothing here
//! knows what stands in it.
//!
//! Turning is the tree's: a pointer pressed on an inset and dragged across it turns it by
//! [`UiInset::turn`] over the node's whole width, and a stick turns it as a drag would
//! ([`UiTree::turn_insets`]). A drag past the slop presses nothing. The yaw is a sum
//! of what it was handed, so the same input turns it the same way, whatever the frame rate.

use sapphire_assets::ui::UiInset;
use sapphire_look::UiQuad;
use sapphire_math::Vec2;

use super::paint::{on_screen, snapped};
use super::{UiNodeId, UiTree};

/// A node's inset as it runs: what the document says, and how far it's turned.
#[derive(Debug)]
pub(super) struct InsetState {
    pub setup: Box<UiInset>,
    /// Degrees from facing the camera.
    pub yaw: f32,
}

impl InsetState {
    pub fn new(setup: &UiInset) -> Self {
        Self { setup: Box::new(setup.clone()), yaw: setup.yaw }
    }
}

/// An inset the tree showed in its last draw: what the app places a showcase for.
#[derive(Clone, Copy, Debug)]
pub struct UiInsetShot<'a> {
    pub node: UiNodeId,
    /// How it's shot and lit.
    pub setup: &'a UiInset,
    /// Degrees it's turned from facing the camera, wrapped to a turn.
    pub yaw: f32,
    /// The picture's size: the node's on the target, in whole pixels.
    pub size: (u32, u32),
}

/// The quad that shows inset `setup` over `rect` (points, snapped as the node's plate is), at `opacity`.
pub(super) fn quad(rect: crate::layout::Rect, opacity: f32) -> UiQuad {
    let size = rect.max - rect.min;
    let opacity = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    UiQuad::picture(rect.min.to_array(), [size.x, 0.0], [0.0, size.y], opacity)
}

impl UiTree {
    /// The insets the last draw showed, in drawing order. A node hidden, transparent, off screen or
    /// outside its mask shows none, as [`UiTree::draw`] draws none for it.
    pub fn insets(&self) -> impl Iterator<Item = UiInsetShot<'_>> {
        let (scale, frame) = (self.scale, self.size);
        self.nodes.iter().enumerate().filter_map(move |(at, node)| {
            let inset = node.inset.as_ref()?;
            let shows = node.final_opacity > 0.0 && on_screen(node.screen, node.own_mask, frame);
            if !node.visible || !shows {
                return None;
            }
            // The picture's quad, as `draw` places it: on the plate's whole pixels.
            let drawn = node.placed.rect(snapped(node.rect, scale));
            let size = drawn.max - drawn.min;
            let pixels = |points: f32| (points * scale).round().max(0.0) as u32;
            let size = (pixels(size.x), pixels(size.y));
            if size.0 == 0 || size.1 == 0 {
                return None;
            }
            let yaw = inset.yaw.rem_euclid(360.0);
            Some(UiInsetShot { node: UiNodeId(at as u32), setup: &inset.setup, yaw, size })
        })
    }

    /// Whether any node shows an inset, shown now or not: what a show declares
    /// `PipelineInventory::insets` by.
    #[must_use]
    pub fn holds_insets(&self) -> bool {
        self.nodes.iter().any(|node| node.inset.is_some())
    }

    /// Turns every inset shown by `across` of a drag over its whole width: a stick's x, from -1 to 1, times
    /// the seconds it was held turns an inset [`UiInset::turn`] degrees a second at full tilt.
    pub fn turn_insets(&mut self, across: f32) {
        if !across.is_finite() {
            return;
        }
        for node in self.nodes.iter_mut().filter(|node| node.visible) {
            if let Some(inset) = &mut node.inset {
                inset.yaw = (inset.yaw + across * inset.setup.turn).rem_euclid(360.0);
            }
        }
    }

    /// The topmost inset under `point`, by the last frame's picture.
    pub(super) fn inset_at(&self, point: Vec2) -> Option<u32> {
        (0..self.nodes.len()).rev().find_map(|at| {
            let node = &self.nodes[at];
            let inside = node.screen.contains(point)
                && node.own_mask.is_none_or(|[x0, y0, x1, y1]| {
                    point.x >= x0 && point.y >= y0 && point.x <= x1 && point.y <= y1
                });
            (node.inset.is_some() && node.visible && node.final_opacity > 0.0 && inside).then_some(at as u32)
        })
    }

    /// Inset `node`'s yaw, `moved` points across from where a drag started at `start` degrees.
    pub(super) fn drag_inset(&mut self, node: u32, start: f32, moved: f32) {
        let node = &mut self.nodes[node as usize];
        let width = (node.screen.max.x - node.screen.min.x).max(1.0);
        if let Some(inset) = &mut node.inset {
            inset.yaw = (start + moved / width * inset.setup.turn).rem_euclid(360.0);
        }
    }

    /// Inset `node`'s yaw, if it has one.
    pub(super) fn inset_yaw(&self, node: u32) -> Option<f32> {
        self.nodes[node as usize].inset.as_ref().map(|inset| inset.yaw)
    }
}
