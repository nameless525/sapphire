//! Where things go: rectangles in UI points, the frame's metrics and anchors.
//!
//! **Units.** A UI point is a size a thumb can judge: `pixels / (display scale × player's scale)`.
//! `SDL_GetWindowDisplayScale` is relative to pixels on every platform, where window coordinates
//! are points on iOS and pixels on Android, so nothing here measures in window coordinates: the
//! safe area is read as shares of the window, and touches arrive as shares of it.

use sapphire_math::Vec2;
use sapphire_platform::Window;

/// How far the notch, rounded corners and system bars reach in from each edge, in UI points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// An axis-aligned rectangle in UI points, +y down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub const fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// The rectangle `half` to each side of `center`.
    pub fn around(center: Vec2, half: Vec2) -> Self {
        Self { min: center - half, max: center + half }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    pub fn contains(&self, point: Vec2) -> bool {
        point.x >= self.min.x && point.x <= self.max.x && point.y >= self.min.y && point.y <= self.max.y
    }

    /// Whether all of `other` is inside, edges included.
    pub fn contains_rect(&self, other: Rect) -> bool {
        self.contains(other.min) && self.contains(other.max)
    }

    /// Whether the two share any area.
    pub fn overlaps(&self, other: Rect) -> bool {
        self.min.x < other.max.x
            && other.min.x < self.max.x
            && self.min.y < other.max.y
            && other.min.y < self.max.y
    }
}

/// The frame as the interface sees it: its size and what the device covers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UiMetrics {
    /// The frame's size in UI points.
    pub size: Vec2,
    pub insets: Insets,
}

impl UiMetrics {
    pub const fn new(size: Vec2, insets: Insets) -> Self {
        Self { size, insets }
    }

    /// For a frame `pixels` large at `pixels_per_point`, such as an offscreen capture, whose
    /// `insets` the caller makes up.
    pub fn of_pixels(pixels: (u32, u32), pixels_per_point: f32, insets: Insets) -> Self {
        let scale = positive_or_one(pixels_per_point);
        Self { size: Vec2::new(pixels.0 as f32, pixels.1 as f32) / scale, insets }
    }

    /// `window`'s size and safe area, read fresh: cheap enough for once a frame, so a rotation or
    /// an inset that changes (Android's bars, iPad's split view) needs no event. `player_scale` is
    /// the player's own UI scale (spec §6.8), 1 by default.
    pub fn of_window(window: &Window, player_scale: f32) -> Self {
        let pixels = window.pixel_size();
        let scale = positive_or_one(window.display_scale()) * positive_or_one(player_scale);
        let size = Vec2::new(pixels.width as f32, pixels.height as f32) / scale;
        let area = window.safe_area();
        let coords = window.size();
        if coords.width == 0 || coords.height == 0 {
            return Self { size, insets: Insets::default() };
        }
        // Shares of the window, which every platform's coordinates agree on.
        let share = Vec2::new(size.x / coords.width as f32, size.y / coords.height as f32);
        let right = i64::from(coords.width) - i64::from(area.x) - i64::from(area.width);
        let bottom = i64::from(coords.height) - i64::from(area.y) - i64::from(area.height);
        let insets = Insets {
            left: area.x as f32 * share.x,
            top: area.y as f32 * share.y,
            right: right as f32 * share.x,
            bottom: bottom as f32 * share.y,
        };
        Self { size, insets }
    }

    /// The part of the frame nothing covers. Insets wider than the frame leave an empty rectangle
    /// at the middle of what is left, never an inverted one.
    pub fn safe(&self) -> Rect {
        let min = Vec2::new(self.insets.left.max(0.0), self.insets.top.max(0.0));
        let max = self.size - Vec2::new(self.insets.right.max(0.0), self.insets.bottom.max(0.0));
        let middle = (min + max) * 0.5;
        Rect { min: min.min(middle), max: max.max(middle) }
    }
}

fn positive_or_one(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 { scale } else { 1.0 }
}

/// A point of a rectangle, as shares of its width and height: where a widget is pinned, so it keeps
/// its distance from that edge at every aspect ratio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
}

impl Anchor {
    pub const TOP_LEFT: Self = Self { x: 0.0, y: 0.0 };
    pub const TOP_CENTER: Self = Self { x: 0.5, y: 0.0 };
    pub const TOP_RIGHT: Self = Self { x: 1.0, y: 0.0 };
    pub const CENTER: Self = Self { x: 0.5, y: 0.5 };
    pub const BOTTOM_LEFT: Self = Self { x: 0.0, y: 1.0 };
    pub const BOTTOM_RIGHT: Self = Self { x: 1.0, y: 1.0 };

    /// The anchor's point in `area`, moved by `offset` points.
    pub fn place(self, area: Rect, offset: Vec2) -> Vec2 {
        area.min + (area.max - area.min) * Vec2::new(self.x, self.y) + offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_take_points_from_pixels() {
        let metrics = UiMetrics::of_pixels((2622, 1206), 3.0, Insets::default());
        assert_eq!(metrics.size, Vec2::new(874.0, 402.0));
        assert_eq!(UiMetrics::of_pixels((640, 360), 0.0, Insets::default()).size, Vec2::new(640.0, 360.0));
    }

    #[test]
    fn the_safe_rectangle_never_inverts() {
        let insets = Insets { left: 62.0, top: 0.0, right: 62.0, bottom: 21.0 };
        let safe = UiMetrics::new(Vec2::new(874.0, 402.0), insets).safe();
        assert_eq!(safe, Rect::new(Vec2::new(62.0, 0.0), Vec2::new(812.0, 381.0)));
        let wide = Insets { left: 600.0, top: 0.0, right: 600.0, bottom: 0.0 };
        let safe = UiMetrics::new(Vec2::new(874.0, 402.0), wide).safe();
        assert!(safe.width() == 0.0 && safe.height() == 402.0, "{safe:?}");
    }

    #[test]
    fn anchors_keep_their_distance_from_their_edges() {
        let offset = Vec2::new(-64.0, -64.0);
        for width in [480.0, 640.0, 840.0] {
            let area = Rect::new(Vec2::ZERO, Vec2::new(width, 360.0));
            assert_eq!(Anchor::BOTTOM_RIGHT.place(area, offset), Vec2::new(width - 64.0, 296.0));
            assert_eq!(Anchor::TOP_CENTER.place(area, Vec2::ZERO), Vec2::new(width * 0.5, 0.0));
        }
    }
}
