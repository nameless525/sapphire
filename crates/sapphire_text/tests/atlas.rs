//! What the atlas promises when it runs out of room. The font is one every checkout has: Ubuntu Light,
//! from `epaint_default_fonts`.

use std::sync::Arc;

use sapphire_look::UiDrawList;
use sapphire_text::{AtlasConfig, AtlasStats, GlyphAtlas, TextStyle, TextSystem};

const STYLE: TextStyle =
    TextStyle { families: "Ubuntu", size: 40.0, weight: 400.0, line_height: 1.3, locale: Some("en") };

fn text_system() -> TextSystem {
    let mut text = TextSystem::new();
    text.register_font(Arc::new(epaint_default_fonts::UBUNTU_LIGHT.to_vec()));
    text
}

/// A frame of `string`, and what it did.
fn frame(text: &mut TextSystem, atlas: &mut GlyphAtlas, string: &str, list: &mut UiDrawList) -> AtlasStats {
    text.begin_frame();
    atlas.begin_frame();
    list.reset();
    let shaped = text.shape(string, &STYLE, 1.0, None);
    atlas.draw(shaped, [0.0; 2], 1.0, [255; 4], list);
    atlas.take_stats()
}

#[test]
fn what_is_on_screen_stays_and_what_left_makes_room() {
    let mut text = text_system();
    let config = AtlasConfig { page_size: 192, max_pages: 1, subpixel_bins: 1, keep_frames: 2 };
    let mut atlas = GlyphAtlas::new(config);
    let mut list = UiDrawList::default();
    // At this size a page holds two of these screens and not much more.
    let first = "ABCDEFGHIJKLMN";

    let drawn = frame(&mut text, &mut atlas, first, &mut list);
    assert_eq!((drawn.rasterized, drawn.dropped, drawn.pages), (14, 0, 1));
    assert_eq!(list.quads.len(), 14);
    // Held: no work, whatever else is true.
    for _ in 0..4 {
        let held = frame(&mut text, &mut atlas, first, &mut list);
        assert_eq!((held.rasterized, held.evicted, held.dropped), (0, 0, 0));
        assert!(list.page_uploads.is_empty());
    }
    // Keep going through letters until the page is full: the first text goes, the new one draws
    // whole, and nothing is dropped.
    let (mut evicted, mut dropped) = (0, 0);
    for string in ["abcdefghijklmn", "OPQRSTUVWXYZ01", "opqrstuvwxyz23", "456789ABCDEFGH", "abcdefghijklmn"] {
        let stats = frame(&mut text, &mut atlas, string, &mut list);
        // Past `keep_frames`, for the screen before it.
        frame(&mut text, &mut atlas, string, &mut list);
        frame(&mut text, &mut atlas, string, &mut list);
        evicted += stats.evicted;
        dropped += stats.dropped;
        assert_eq!(list.quads.len(), 14, "{string}");
    }
    assert!(evicted > 0, "the test must fill the page");
    assert_eq!(dropped, 0);
    let back = frame(&mut text, &mut atlas, first, &mut list);
    assert!(back.rasterized > 0, "what was evicted comes back");
    assert_eq!((back.dropped, list.quads.len()), (0, 14));
}

#[test]
fn a_screen_bigger_than_the_atlas_costs_no_more_than_a_full_one() {
    let mut text = text_system();
    let config = AtlasConfig { page_size: 128, max_pages: 1, subpixel_bins: 1, keep_frames: 2 };
    let mut atlas = GlyphAtlas::new(config);
    let mut list = UiDrawList::default();
    let string = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    let first = frame(&mut text, &mut atlas, string, &mut list);
    assert!(first.dropped > 0, "the test must overflow the page");
    assert_eq!(list.quads.len() as u32 + first.dropped, 62, "what didn't fit is counted");
    for _ in 0..3 {
        let next = frame(&mut text, &mut atlas, string, &mut list);
        assert!(next.rasterized <= 1, "one try a frame, not the whole overflow: {next:?}");
        assert_eq!(next.evicted, 0, "this frame's glyphs are safe");
        assert_eq!(list.quads.len() as u32 + next.dropped, 62);
    }
}

#[test]
fn a_reset_uploads_everything_again() {
    let mut text = text_system();
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    let first = frame(&mut text, &mut atlas, "Attack", &mut list);
    let bytes = list.upload_texels.clone();
    assert_eq!(frame(&mut text, &mut atlas, "Attack", &mut list).rasterized, 0);
    atlas.reset();
    let again = frame(&mut text, &mut atlas, "Attack", &mut list);
    assert_eq!(first.rasterized, again.rasterized);
    assert!(list.upload_texels == bytes, "the same glyphs in the same places");
}

#[test]
fn blank_glyphs_draw_nothing_and_cost_one_try() {
    let mut text = text_system();
    let config = AtlasConfig { page_size: 512, subpixel_bins: 1, ..AtlasConfig::default() };
    let mut atlas = GlyphAtlas::new(config);
    let mut list = UiDrawList::default();
    let stats = frame(&mut text, &mut atlas, "   ", &mut list);
    assert_eq!((stats.rasterized, list.quads.len(), stats.pages), (0, 0, 0));
    assert_eq!(stats.glyphs, 1, "remembered, so it isn't scaled again");
}
