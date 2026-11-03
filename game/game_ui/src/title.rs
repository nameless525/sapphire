//! The title: a card over the arena as it stands, waiting for a press.
//!
//! Shapes from the HUD's atlas with no words, because the game has no text to draw yet: the U series brings
//! that, and a menu to put here. The card knows nothing of what a press opens, and takes no input: the app
//! says when it goes.

use std::sync::Arc;
use std::time::Duration;

use sapphire_math::Vec2;
use sapphire_ui::{Atlas, Insets, Paint, Painter, Rect, UiDrawList, UiMetrics};

/// The frame's height in the card's points, whatever its pixels: a card is a picture, with no thumbs to fit.
const HEIGHT: f32 = 720.0;
/// The band's height, in points.
const BAND: f32 = 96.0;
/// Where the band and the mark under it stand down the frame, as shares of its height: the band over the town
/// and the mark at the player's feet, so neither fighter's hidden.
const BAND_AT: f32 = 0.2;
const MARK_AT: f32 = 0.93;
/// One breath of the mark under it, in seconds.
const BREATH: f32 = 2.4;
/// The sapphire of the band.
const DEEP: [u8; 4] = [14, 34, 92, 224];
/// The pale edge along it.
const PALE: [u8; 4] = [196, 222, 255, 255];

/// See this module.
pub struct TitleCard {
    atlas: Atlas,
    /// How long it's been up.
    age: Duration,
}

impl TitleCard {
    pub fn new() -> Self {
        Self { atlas: Atlas::paint(), age: Duration::ZERO }
    }

    /// A frame `dt` after the last, on the real clock.
    pub fn advance(&mut self, dt: Duration) {
        self.age += dt;
    }

    /// This frame's card, on a `target` this many pixels across. Allocates nothing once `list` has room.
    pub fn draw(&self, target: (u32, u32), list: &mut UiDrawList) {
        let metrics = UiMetrics::of_pixels(target, target.1 as f32 / HEIGHT, Insets::default());
        let size = metrics.size;
        list.viewport = size.to_array();
        list.atlas = Some(Arc::clone(&self.atlas.image));
        let mut painter = Painter::new(&mut list.quads, &self.atlas.sprites);
        let middle = Vec2::new(size.x * 0.5, size.y * BAND_AT);
        // Past the frame's sides, so no end of it shows.
        let across = |height: f32, at: f32| {
            let half = Vec2::new(size.x * 0.5 + HEIGHT, height * 0.5);
            let center = Vec2::new(middle.x, at);
            Rect::new(center - half, center + half)
        };
        painter.bar(across(BAND, middle.y), 0.0, (0.0, 1.0), Paint::fill(DEEP));
        let edge = BAND * 0.5 + 6.0;
        painter.bar(across(4.0, middle.y - edge), 0.0, (0.0, 1.0), Paint::fill(PALE));
        painter.bar(across(4.0, middle.y + edge), 0.0, (0.0, 1.0), Paint::fill(PALE));
        let sprites = painter.sprites();
        painter.sprite(sprites.spark, middle, Vec2::splat(BAND * 0.8), 0.0, Paint::fill(PALE).soft(1.0));
        // At the foot of the frame, breathing: something's waiting.
        let breath = (self.age.as_secs_f32() / BREATH).fract();
        let alpha = 96.0 + 159.0 * (1.0 - (breath * 2.0 - 1.0).abs());
        let mark = Vec2::new(middle.x, size.y * MARK_AT);
        let color = [PALE[0], PALE[1], PALE[2], alpha as u8];
        painter.sprite(sprites.chevrons, mark, Vec2::splat(BAND * 0.4), 0.0, Paint::fill(color));
    }
}

impl Default for TitleCard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_names_its_atlas_and_is_the_same_size_of_picture_on_any_frame() {
        let card = TitleCard::new();
        let mut list = UiDrawList::default();
        card.draw((1920, 1080), &mut list);
        assert!(list.atlas.is_some(), "a list with quads and no atlas draws nothing");
        assert_eq!(list.viewport[1], HEIGHT);
        let quads = list.quads.len();
        assert!(quads > 0);
        list.reset();
        card.draw((390, 844), &mut list);
        assert_eq!(list.viewport[1], HEIGHT);
        assert_eq!(list.quads.len(), quads);
    }

    #[test]
    fn only_the_mark_at_the_foot_moves() {
        let mut card = TitleCard::new();
        let mut first = UiDrawList::default();
        card.draw((1280, 720), &mut first);
        card.advance(Duration::from_secs_f32(BREATH * 0.5));
        let mut later = UiDrawList::default();
        card.draw((1280, 720), &mut later);
        let last = first.quads.len() - 1;
        assert_eq!(first.quads[..last], later.quads[..last]);
        assert!(later.quads[last].color[3] > first.quads[last].color[3]);
    }
}
