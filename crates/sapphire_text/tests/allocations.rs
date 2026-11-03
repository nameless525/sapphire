//! A frame that changed nothing allocates nothing: text is cached, and the glyphs are in the atlas.
//! Installs `CountingAllocator` as this test binary's global allocator.
//!
//! The font is one every checkout has: Ubuntu Light, from `epaint_default_fonts`, a dev-dependency.

mod common;

use std::alloc::System;
use std::sync::Arc;

use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_look::UiDrawList;
use sapphire_text::{AtlasConfig, GlyphAtlas, TextStyle, TextSystem};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const STRINGS: [&str; 6] =
    ["Screen brightness", "Music volume", "Attack", "Defense 1,284", "Return to title", "Level up"];

#[test]
fn a_frame_that_changed_nothing_allocates_nothing() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut text = TextSystem::new();
    text.register_font(Arc::new(epaint_default_fonts::UBUNTU_LIGHT.to_vec()));
    let style =
        TextStyle { families: "Ubuntu", size: 14.0, weight: 400.0, line_height: 1.3, locale: Some("en") };
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    let frame = |text: &mut TextSystem, atlas: &mut GlyphAtlas, list: &mut UiDrawList| {
        text.begin_frame();
        atlas.begin_frame();
        list.reset();
        list.viewport = [640.0, 360.0];
        for (line, string) in STRINGS.iter().enumerate() {
            let shaped = text.shape(string, &style, 1.5, Some(300.0));
            atlas.draw(shaped, [20.0, 20.0 * line as f32], 1.5, [255; 4], list);
        }
    };
    frame(&mut text, &mut atlas, &mut list);
    assert!(!list.page_uploads.is_empty(), "the first frame rasterizes");
    frame(&mut text, &mut atlas, &mut list);
    assert!(list.page_uploads.is_empty(), "the second has nothing new");

    let before = alloc::thread_counts();
    for _ in 0..100 {
        frame(&mut text, &mut atlas, &mut list);
    }
    let after = alloc::thread_counts();
    assert_eq!(after.allocations - before.allocations, 0, "{} bytes", after.bytes - before.bytes);
    assert_eq!(text.take_stats().misses, 6, "shaped once each");
    assert_eq!(atlas.take_stats().evicted, 0);
}
