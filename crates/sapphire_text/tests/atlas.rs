//! What the atlas promises when it runs out of room. The font is one every checkout has: Ubuntu Light,
//! from `epaint_default_fonts`.

use std::sync::Arc;

use sapphire_look::{UiDrawList, UiGlyphStats, UiPageBands, UiPageState};
use sapphire_text::{AtlasConfig, FaceOptions, GlyphAtlas, TextStyle, TextSystem};

const STYLE: TextStyle<'static> =
    TextStyle { families: "Ubuntu", size: 40.0, weight: 400.0, line_height: 1.3, locale: Some("en") };

fn text_system() -> TextSystem {
    let mut text = TextSystem::new();
    text.register_font(Arc::new(epaint_default_fonts::UBUNTU_LIGHT.to_vec()));
    text
}

/// A frame of `string`, and what it did.
fn frame(text: &mut TextSystem, atlas: &mut GlyphAtlas, string: &str, list: &mut UiDrawList) -> UiGlyphStats {
    frame_on(UiPageState::NO_RENDERER, text, atlas, string, list)
}

/// [`frame`], on pages in the state `pages`.
fn frame_on(
    pages: UiPageState,
    text: &mut TextSystem,
    atlas: &mut GlyphAtlas,
    string: &str,
    list: &mut UiDrawList,
) -> UiGlyphStats {
    text.begin_frame();
    atlas.begin_frame(pages);
    list.reset();
    let shaped = text.shape(string, &STYLE, 1.0, None);
    atlas.draw(shaped, [0.0; 2], 1.0, [255; 4], list);
    atlas.stats()
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
fn a_new_renderer_gets_everything_again() {
    let mut text = text_system();
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    let first = UiPageState { store: 1, batch: 0 };
    let drawn = frame_on(first, &mut text, &mut atlas, "Attack", &mut list);
    assert!(!drawn.forgot);
    let bytes = list.upload_texels.clone();
    assert_eq!(list.page_state, UiPageState { store: 1, batch: 1 });

    let held = UiPageState { store: 1, batch: 1 };
    let again = frame_on(held, &mut text, &mut atlas, "Attack", &mut list);
    assert_eq!((again.rasterized, again.forgot), (0, false));
    assert_eq!(list.page_state, held, "nothing new, so no new batch");

    let second = UiPageState { store: 2, batch: 0 };
    let moved = frame_on(second, &mut text, &mut atlas, "Attack", &mut list);
    assert_eq!((moved.rasterized, moved.forgot), (drawn.rasterized, true));
    assert_eq!(list.page_state.store, 2);
    assert!(list.upload_texels == bytes, "the same glyphs in the same places");
}

#[test]
fn a_batch_that_never_arrived_is_sent_again() {
    let mut text = text_system();
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    let start = UiPageState { store: 1, batch: 0 };
    let drawn = frame_on(start, &mut text, &mut atlas, "Attack", &mut list);
    // That list was never prepared, so the pages are as they were.
    let next = frame_on(start, &mut text, &mut atlas, "Attack", &mut list);
    assert_eq!((next.rasterized, next.forgot), (drawn.rasterized, true));
    assert_eq!(list.page_state.batch, 2, "its uploads are a later batch than the lost one");
    // That one was, and nothing is lost.
    let held = frame_on(list.page_state, &mut text, &mut atlas, "Attack", &mut list);
    assert_eq!((held.rasterized, held.forgot), (0, false));
}

#[test]
fn the_list_carries_the_frame_s_stats() {
    let mut text = text_system();
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    let drawn = frame(&mut text, &mut atlas, "Attack", &mut list);
    assert_eq!(list.glyphs, drawn);
    assert!(drawn.rasterized > 0 && drawn.upload_bytes > 0 && drawn.used_bytes > 0);
    assert_eq!((drawn.pages, drawn.glyphs), (1, drawn.rasterized));
    let held = frame(&mut text, &mut atlas, "Attack", &mut list);
    assert_eq!((held.rasterized, held.upload_bytes, held.glyphs), (0, 0, drawn.glyphs), "this frame's");
}

#[test]
fn a_warmed_screen_shows_with_nothing_left_to_do() {
    let mut text = text_system();
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    let string = "The quick brown fox jumps over the lazy dog";

    let (mut frames, mut uploads) = (0, 0);
    loop {
        text.begin_frame();
        atlas.begin_frame(UiPageState::NO_RENDERER);
        list.reset();
        let shaped = text.shape(string, &STYLE, 1.5, None);
        let mut budget = 5;
        let done = atlas.warm(shaped, &mut budget, &mut list);
        frames += 1;
        uploads += list.page_uploads.len();
        assert!(list.page_uploads.len() <= 5, "the budget holds");
        assert!(list.quads.is_empty(), "and nothing draws");
        assert_eq!(list.page_size, 512);
        if done {
            break;
        }
        assert_eq!(budget, 0, "it stops only when the budget is spent");
        assert!(frames < 100);
    }
    assert!(frames > 1 && uploads > 5);

    text.begin_frame();
    atlas.begin_frame(UiPageState::NO_RENDERER);
    list.reset();
    let shaped = text.shape(string, &STYLE, 1.5, None);
    atlas.draw(shaped, [12.0, 40.0], 1.5, [255; 4], &mut list);
    assert_eq!(atlas.stats().rasterized, 0, "the frame that shows it");
    assert!(list.page_uploads.is_empty());
    assert!(!list.quads.is_empty());
}

#[test]
fn a_size_lands_on_a_whole_pixel_at_any_scale() {
    let mut text = text_system();
    let style = |size| TextStyle { size, ..STYLE };
    for (size, scale, ppem) in [(14.0, 1.0, 14.0), (14.0, 1.5, 21.0), (13.0, 1.5, 20.0), (16.0, 1.25, 20.0)] {
        let shaped = text.shape("Attack", &style(size), scale, None);
        assert_eq!(shaped.runs[0].ppem, ppem, "{size} points at {scale}x");
    }
}

/// Joining uploads writes zeros under the shorter glyphs. Through a page's whole life of glyphs
/// coming and going, none may land on a glyph that's still in use.
#[test]
fn joined_uploads_never_touch_a_glyph_in_use() {
    const SIDE: usize = 256;
    let mut text = text_system();
    let config = AtlasConfig { page_size: SIDE as u32, max_pages: 1, subpixel_bins: 4, keep_frames: 2 };
    let mut atlas = GlyphAtlas::new(config);
    let mut list = UiDrawList::default();
    let mut bands = UiPageBands::default();
    // What each rectangle says, and what the joined bands say.
    let mut exact = vec![0u8; SIDE * SIDE];
    let mut joined = vec![0u8; SIDE * SIDE];
    let screens = [
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "abcdefghijklmnopqrstuvwxyz",
        "gjpqy,;()[]{}|/\\",
        "0123456789!?@#$%&*+=",
        "The quick brown fox",
        "jumps over the lazy dog",
    ];
    let (mut evicted, mut written, mut rectangles) = (0, 0, 0);
    for round in 0..12 {
        for screen in &screens {
            for _ in 0..3 {
                let stats = frame(&mut text, &mut atlas, screen, &mut list);
                evicted += stats.evicted;
                assert_eq!(list.page_bands(&mut bands), 0);
                written += bands.bands.len();
                rectangles += list.page_uploads.len();
                for upload in &list.page_uploads {
                    let (x, y, w) = (usize::from(upload.x), usize::from(upload.y), usize::from(upload.width));
                    for row in 0..usize::from(upload.height) {
                        let from = upload.offset as usize + row * w;
                        let to = (y + row) * SIDE + x;
                        exact[to..to + w].copy_from_slice(&list.upload_texels[from..from + w]);
                    }
                }
                for band in &bands.bands {
                    let (x, y, w) = (usize::from(band.x), usize::from(band.y), usize::from(band.width));
                    for row in 0..usize::from(band.height) {
                        let from = band.offset as usize + row * w;
                        let to = (y + row) * SIDE + x;
                        joined[to..to + w].copy_from_slice(&bands.texels[from..from + w]);
                    }
                }
                for quad in &list.quads {
                    let side = SIDE as f32;
                    let (x0, y0) = ((quad.uv[0] * side) as usize, (quad.uv[1] * side) as usize);
                    let (x1, y1) = ((quad.uv[2] * side) as usize, (quad.uv[3] * side) as usize);
                    for y in y0..y1 {
                        assert!(
                            exact[y * SIDE + x0..y * SIDE + x1] == joined[y * SIDE + x0..y * SIDE + x1],
                            "round {round}: a band wrote over a glyph in use",
                        );
                    }
                }
            }
        }
    }
    assert!(evicted > 100, "the test must churn the page: {evicted}");
    assert!(written * 2 < rectangles, "and joining must do something: {written} of {rectangles}");
}

#[test]
fn a_face_s_baseline_shift_moves_its_lines_and_no_others() {
    let bytes = || Arc::new(epaint_default_fonts::UBUNTU_LIGHT.to_vec());
    let mut plain = TextSystem::new();
    plain.register_font(bytes());
    let mut shifted = TextSystem::new();
    shifted.register_face(bytes(), FaceOptions { baseline_shift: 0.25 });
    for (size, scale, pixels) in [(24.0, 1.0, 6.0), (24.0, 2.0, 12.0), (32.0, 1.5, 12.0)] {
        let style = TextStyle { size, ..STYLE };
        let before = plain.shape("Title", &style, scale, None).clone();
        let after = shifted.shape("Title", &style, scale, None);
        assert_eq!(after.baselines[0], before.baselines[0] + pixels, "{size} points at {scale}x");
        assert_eq!(after.glyphs[0].y, before.glyphs[0].y + pixels);
        assert_eq!(after.glyphs[0].x, before.glyphs[0].x);
        assert_eq!(after.height, before.height, "the box doesn't change");
    }
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

#[test]
fn page_dimensions_are_checked_before_any_glyph_or_gpu_work() {
    for page_size in [0, 46341, 65536, u32::MAX] {
        assert!(
            std::panic::catch_unwind(|| {
                GlyphAtlas::new(AtlasConfig { page_size, ..AtlasConfig::default() })
            })
            .is_err(),
            "{page_size} cannot fit the allocator's area and upload coordinates"
        );
    }
    for page_size in [1, 2048, 46340] {
        let atlas = GlyphAtlas::new(AtlasConfig { page_size, ..AtlasConfig::default() });
        assert_eq!(atlas.config().page_size, page_size);
        assert_eq!(atlas.stats().pages, 0, "construction allocates no page texels");
    }
}
