//! What text costs, printed. `cargo test -p sapphire_text --release --test costs -- --ignored --nocapture
//! --test-threads=1`, with `SAPPHIRE_TEST_FONTS` set (`tests/common`). They assert nothing but
//! print what U1 reports: the same harness runs on a simulator and an emulator.
//!
//! `SAPPHIRE_TEST_SUBSET` names a subset of M PLUS 1 to register beside the whole font.

#![allow(clippy::print_stdout, reason = "printing the numbers is this test's whole job")]
#![allow(clippy::disallowed_methods, reason = "timing them needs the OS clock, and nothing here ships")]

mod common;

use std::alloc::System;
use std::sync::Arc;
use std::time::Instant;

use common::{
    DELA, FAMILY_BODY, Item, MPLUS1, OXANIUM, distinct_japanese, menu_screen, percentile, read_font, style,
};
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_look::{UiDrawList, UiGlyphStats, UiPageBands, UiPageState};
use sapphire_text::{AtlasConfig, GlyphAtlas, TextSystem};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

#[test]
#[ignore = "prints costs"]
fn costs_1_registering_fonts() {
    let Some(dir) = common::fonts_dir("costs_1_registering_fonts") else { return };
    let mut cases: Vec<(&str, Arc<Vec<u8>>)> = vec![
        ("M PLUS 1 whole", read_font(&dir, MPLUS1)),
        ("Oxanium whole", read_font(&dir, OXANIUM)),
        ("Dela Gothic One whole", read_font(&dir, DELA)),
    ];
    if let Some(subset) = std::env::var_os("SAPPHIRE_TEST_SUBSET") {
        cases.push(("M PLUS 1 subset", Arc::new(std::fs::read(subset).expect("the subset"))));
    }
    for (name, font) in &cases {
        let mut times = Vec::new();
        let mut held = 0;
        let mut allocations = 0;
        for _ in 0..50 {
            let before = alloc::stats();
            let start = Instant::now();
            let mut text = TextSystem::new();
            text.register_font(Arc::clone(font));
            times.push(ms(start));
            let after = alloc::stats();
            held = after.live_bytes - before.live_bytes;
            allocations = after.total_allocations - before.total_allocations;
            drop(text);
        }
        let worst = times.iter().copied().fold(0.0, f64::max);
        println!(
            "register {name}: file {:.2} MB, p50 {:.3} ms, worst {:.3} ms, heap beyond the file's bytes {:.1} KB in {allocations} allocations",
            font.len() as f64 / 1e6,
            percentile(&mut times, 0.5),
            worst,
            held as f64 / 1e3,
        );
    }
    let start = Instant::now();
    let mut text = common::system(&dir);
    println!("read and register all three: {:.2} ms, families {:?}", ms(start), text.family_names());
    let body = style(FAMILY_BODY, 14.0, 400.0, "ja");
    let start = Instant::now();
    let shaped = text.shape("設定", &body, 1.0, None);
    println!("first shape after registering: {:.2} ms, {} glyphs", ms(start), shaped.glyphs.len());
    // What a font costs to touch: reading it maps its tables' pages in, on an mmap at least.
    let start = Instant::now();
    let shaped = text.shape("設定と記録", &body, 1.0, None);
    println!("second shape, a new string: {:.3} ms, {} glyphs", ms(start), shaped.glyphs.len());
}

fn shape_screen(text: &mut TextSystem, screen: &[Item], scale: f32) -> (usize, usize) {
    let (mut chars, mut glyphs) = (0, 0);
    for item in screen {
        let shaped = text.shape(item.text, &item.style, scale, item.max_width);
        chars += item.text.chars().count();
        glyphs += shaped.glyphs.len();
    }
    (chars, glyphs)
}

#[test]
#[ignore = "prints costs"]
fn costs_2_shaping_a_screen() {
    let Some(dir) = common::fonts_dir("costs_2_shaping_a_screen") else { return };
    let ja = menu_screen(true);
    let en = menu_screen(false);
    let scale = 1.5;
    {
        let start = Instant::now();
        let mut text = common::system(&dir);
        let register = ms(start);
        let start = Instant::now();
        let (chars, glyphs) = shape_screen(&mut text, &ja, scale);
        println!(
            "cold process's first Japanese screen: {} strings, {chars} chars, {glyphs} glyphs, {:.2} ms (after {register:.2} ms reading and registering)",
            ja.len(),
            ms(start),
        );
    }
    let before = alloc::stats().live_bytes;
    let mut text = common::system(&dir);
    let registered = alloc::stats().live_bytes;
    shape_screen(&mut text, &ja, scale);
    shape_screen(&mut text, &en, scale);
    println!(
        "heap: three fonts registered {:.2} MB; both screens' layouts and the shaper's caches {:.0} KB more",
        (registered - before) as f64 / 1e6,
        (alloc::stats().live_bytes - registered) as f64 / 1e3,
    );
    for (name, screen) in [("Japanese", &ja), ("English", &en)] {
        let mut full = Vec::new();
        let mut per_string = Vec::new();
        for round in 0..60 {
            text.clear_cache();
            let start = Instant::now();
            for item in screen {
                let one = Instant::now();
                text.shape(item.text, &item.style, scale, item.max_width);
                if round > 0 {
                    per_string.push(ms(one) * 1e3);
                }
            }
            if round > 0 {
                full.push(ms(start));
            }
        }
        let worst = full.iter().copied().fold(0.0, f64::max);
        let worst_string = per_string.iter().copied().fold(0.0, f64::max);
        println!(
            "{name} screen, first layout with warm plan caches: p50 {:.2} ms, worst {worst:.2} ms; per string p50 {:.0} µs, worst {worst_string:.0} µs",
            percentile(&mut full, 0.5),
            percentile(&mut per_string, 0.5),
        );
    }

    // A language switch: each new language's whole screen, with the last one's layouts cached.
    let mut switches = Vec::new();
    let mut allocations = Vec::new();
    for _ in 0..30 {
        text.clear_cache();
        shape_screen(&mut text, &en, scale);
        let before = alloc::stats().total_allocations;
        let start = Instant::now();
        shape_screen(&mut text, &ja, scale);
        switches.push(ms(start));
        allocations.push((alloc::stats().total_allocations - before) as f64);
    }
    let worst = switches.iter().copied().fold(0.0, f64::max);
    println!(
        "switch English to Japanese, shaping only: p50 {:.2} ms, worst {worst:.2} ms, {:.0} allocations",
        percentile(&mut switches, 0.5),
        percentile(&mut allocations, 0.5),
    );
    let mut back = Vec::new();
    for _ in 0..30 {
        let start = Instant::now();
        shape_screen(&mut text, &en, scale);
        shape_screen(&mut text, &ja, scale);
        back.push(ms(start) / 2.0);
    }
    let worst = back.iter().copied().fold(0.0, f64::max);
    println!("a screen already cached: p50 {:.4} ms, worst {worst:.4} ms", percentile(&mut back, 0.5),);

    // A paragraph and a label, for what one changed string costs.
    let body = style(FAMILY_BODY, 14.0, 400.0, "ja");
    let mplus = read_font(&dir, MPLUS1);
    let paragraph = distinct_japanese(&mplus, 200);
    for (name, string) in [("a 3-character label", "攻撃力"), ("200 characters", paragraph.as_str())] {
        let mut times = Vec::new();
        for _ in 0..200 {
            text.clear_cache();
            let start = Instant::now();
            text.shape(string, &body, scale, Some(600.0));
            times.push(ms(start) * 1e3);
        }
        let worst = times.iter().copied().fold(0.0, f64::max);
        println!("{name}: p50 {:.0} µs, worst {worst:.0} µs", percentile(&mut times, 0.5));
    }
}

/// One frame of a screen through the atlas; what it cost.
struct Frame {
    ms: f64,
    stats: UiGlyphStats,
    runs: usize,
    quads: usize,
    uploads: usize,
    writes: usize,
}

fn draw_frame(
    text: &mut TextSystem,
    atlas: &mut GlyphAtlas,
    screen: &[Item],
    scale: f32,
    list: &mut UiDrawList,
) -> Frame {
    text.begin_frame();
    atlas.begin_frame(UiPageState::NO_RENDERER);
    list.reset();
    let shaped: Vec<_> =
        screen.iter().map(|item| text.shape(item.text, &item.style, scale, item.max_width).clone()).collect();
    let start = Instant::now();
    let mut y = 0.0;
    for shape in &shaped {
        atlas.draw(shape, [8.0, y], scale, [255; 4], list);
        y += shape.height / scale;
    }
    let ms = ms(start);
    Frame {
        ms,
        stats: atlas.stats(),
        runs: list.page_runs.len(),
        quads: list.quads.len(),
        uploads: list.page_uploads.len(),
        writes: {
            let mut bands = UiPageBands::default();
            list.page_bands(&mut bands);
            bands.bands.len()
        },
    }
}

/// A body-text screen in which every glyph is new, filling `points` at 14 points a cell.
fn worst_screen(font: &[u8], points: [f32; 2]) -> (&'static str, Vec<Item>) {
    let (columns, rows) = ((points[0] / 14.0) as usize, (points[1] / 18.2) as usize);
    let hard: &'static str = Box::leak(distinct_japanese(font, columns * rows).into_boxed_str());
    (
        hard,
        vec![Item { text: hard, style: style(FAMILY_BODY, 14.0, 400.0, "ja"), max_width: Some(points[0]) }],
    )
}

const TARGETS: [(&str, [f32; 2], f32); 4] = [
    ("Mobile-Low 720p at 1.5x", [853.0, 480.0], 1.5),
    ("Desktop 1080p at 1.5x", [1280.0, 720.0], 1.5),
    ("Desktop 1080p at 1x", [1920.0, 1080.0], 1.0),
    ("iPhone 12 native (1170p) at 3x", [844.0, 390.0], 3.0),
];

#[test]
#[ignore = "prints costs"]
fn costs_3_atlas_on_a_full_screen() {
    let Some(dir) = common::fonts_dir("costs_3_atlas_on_a_full_screen") else { return };
    let mut text = common::system(&dir);
    let mplus = read_font(&dir, MPLUS1);
    let ja = menu_screen(true);
    let en = menu_screen(false);
    for (target, points, scale) in TARGETS {
        println!("--- {target}: {} × {} points", points[0], points[1]);
        for bins in [1u8, 4] {
            let config = AtlasConfig { page_size: 2048, max_pages: 16, subpixel_bins: bins, keep_frames: 2 };
            let mut atlas = GlyphAtlas::new(config);
            let mut list = UiDrawList::default();
            let first = draw_frame(&mut text, &mut atlas, &ja, scale, &mut list);
            let again = draw_frame(&mut text, &mut atlas, &ja, scale, &mut list);
            let switch = draw_frame(&mut text, &mut atlas, &en, scale, &mut list);
            let back = draw_frame(&mut text, &mut atlas, &ja, scale, &mut list);
            let held = back.stats;
            let (worst, worst_items) = worst_screen(&mplus, points);
            let worst_frame = draw_frame(&mut text, &mut atlas, &worst_items, scale, &mut list);
            let end = atlas.stats();
            println!(
                "bins {bins}: menu {} quads in {} runs, {} glyphs rasterized in {:.2} ms ({:.1} µs each), {} uploads of {:.0} KB in {} writes; a frame after, {} rasterized in {:.3} ms",
                first.quads,
                first.runs,
                first.stats.rasterized,
                first.ms,
                first.ms * 1e3 / f64::from(first.stats.rasterized.max(1)),
                first.uploads,
                first.stats.upload_bytes as f64 / 1e3,
                first.writes,
                again.stats.rasterized,
                again.ms,
            );
            println!(
                "  switch to English: {} rasterized in {:.2} ms, {:.0} KB; back to Japanese {} rasterized in {:.2} ms; both held {} glyphs, {} KB of page area in {} pages",
                switch.stats.rasterized,
                switch.ms,
                switch.stats.upload_bytes as f64 / 1e3,
                back.stats.rasterized,
                back.ms,
                held.glyphs,
                held.used_bytes / 1000,
                held.pages,
            );
            println!(
                "  worst screen, {} distinct characters: {} rasterized in {:.2} ms ({:.1} µs each), {:.0} KB; now {} glyphs, {:.1} MB of area, {} pages at 2048, {} evicted, {} dropped",
                worst.chars().count(),
                worst_frame.stats.rasterized,
                worst_frame.ms,
                worst_frame.ms * 1e3 / f64::from(worst_frame.stats.rasterized.max(1)),
                worst_frame.stats.upload_bytes as f64 / 1e3,
                worst_frame.stats.glyphs,
                worst_frame.stats.used_bytes as f64 / 1e6,
                worst_frame.stats.pages,
                worst_frame.stats.evicted + end.evicted,
                worst_frame.stats.dropped + end.dropped,
            );
        }
    }
}

#[test]
#[ignore = "prints costs"]
fn costs_4_page_sizes_and_eviction() {
    let Some(dir) = common::fonts_dir("costs_4_page_sizes_and_eviction") else { return };
    let mut text = common::system(&dir);
    let mplus = read_font(&dir, MPLUS1);
    let ja = menu_screen(true);
    let en = menu_screen(false);
    for (target, points, scale) in TARGETS {
        let (_, hard) = worst_screen(&mplus, points);
        for page_size in [512u32, 1024, 2048] {
            let config = AtlasConfig { page_size, max_pages: 64, subpixel_bins: 1, keep_frames: 2 };
            let mut atlas = GlyphAtlas::new(config);
            let mut list = UiDrawList::default();
            let menu = draw_frame(&mut text, &mut atlas, &ja, scale, &mut list).stats;
            let both = draw_frame(&mut text, &mut atlas, &en, scale, &mut list).stats;
            let worst = draw_frame(&mut text, &mut atlas, &hard, scale, &mut list).stats;
            let fill = |stats: &UiGlyphStats| {
                stats.used_bytes as f64 / (f64::from(stats.pages) * f64::from(page_size * page_size))
            };
            println!(
                "{target} page {page_size}: menu {} pages ({:.0}% full), +English {} pages, +worst screen {} pages ({:.0}% full, {} glyphs)",
                menu.pages,
                fill(&menu) * 100.0,
                both.pages,
                worst.pages,
                fill(&worst) * 100.0,
                worst.glyphs,
            );
        }
    }

    // Eviction: a real menu through a small atlas, then a tour of hard screens that overflows it,
    // watching every frame's work.
    println!("--- eviction: one or two 1024 pages at 1.5x, cycling through screens of 300 new characters");
    let screens: Vec<&'static str> = (0..16)
        .map(|index| {
            let all = distinct_japanese(&mplus, 300 * (index + 1));
            let start = all.char_indices().nth(300 * index).map_or(0, |(at, _)| at);
            &*Box::leak(all[start..].to_owned().into_boxed_str())
        })
        .collect();
    for (pages, label) in [(1u16, "1 page"), (2, "2 pages")] {
        let config = AtlasConfig { page_size: 1024, max_pages: pages, subpixel_bins: 1, keep_frames: 2 };
        let mut atlas = GlyphAtlas::new(config);
        let mut list = UiDrawList::default();
        let mut worst_ms = 0.0f64;
        let mut worst_rasterized = 0;
        let (mut evicted, mut dropped) = (0, 0);
        let mut steady = Vec::new();
        for round in 0..3 {
            for screen in &screens {
                let item = [Item {
                    text: screen,
                    style: style(FAMILY_BODY, 14.0, 400.0, "ja"),
                    max_width: Some(800.0),
                }];
                let first = draw_frame(&mut text, &mut atlas, &item, 1.5, &mut list);
                worst_ms = worst_ms.max(first.ms);
                worst_rasterized = worst_rasterized.max(first.stats.rasterized);
                evicted += first.stats.evicted;
                dropped += first.stats.dropped;
                // The same screen stays on for a few frames.
                for _ in 0..3 {
                    let held = draw_frame(&mut text, &mut atlas, &item, 1.5, &mut list);
                    steady.push(f64::from(held.stats.rasterized));
                    dropped += held.stats.dropped;
                }
                let _ = round;
            }
        }
        let thrash = steady.iter().copied().fold(0.0, f64::max);
        println!(
            "{label}: worst frame {worst_ms:.2} ms, {worst_rasterized} rasterized; {evicted} evicted, {dropped} dropped; most rasterized on a held screen: {thrash}",
        );
    }
    // A screen bigger than the whole atlas can't hold: what's dropped, frame after frame.
    let config = AtlasConfig { page_size: 512, max_pages: 1, subpixel_bins: 1, keep_frames: 2 };
    let mut atlas = GlyphAtlas::new(config);
    let mut list = UiDrawList::default();
    let (_, hard) = worst_screen(&mplus, [1280.0, 720.0]);
    let mut rasterized = Vec::new();
    for _ in 0..4 {
        let frame = draw_frame(&mut text, &mut atlas, &hard, 1.5, &mut list);
        rasterized.push((frame.stats.rasterized, frame.stats.evicted, frame.stats.dropped));
    }
    println!("a screen bigger than the atlas, (rasterized, evicted, dropped) by frame: {rasterized:?}");
}

#[test]
#[ignore = "prints costs"]
fn costs_5_raster_by_size() {
    let Some(dir) = common::fonts_dir("costs_5_raster_by_size") else { return };
    let mut text = common::system(&dir);
    let mplus = read_font(&dir, MPLUS1);
    let chars: &'static str = Box::leak(distinct_japanese(&mplus, 600).into_boxed_str());
    for px in [18.0f32, 24.0, 32.0, 48.0, 64.0, 96.0] {
        let mut atlas =
            GlyphAtlas::new(AtlasConfig { page_size: 4096, max_pages: 32, ..AtlasConfig::default() });
        let mut list = UiDrawList::default();
        let item =
            [Item { text: chars, style: style(FAMILY_BODY, px, 400.0, "ja"), max_width: Some(4096.0) }];
        let frame = draw_frame(&mut text, &mut atlas, &item, 1.0, &mut list);
        let rasterized = f64::from(frame.stats.rasterized.max(1));
        println!(
            "{px} px: {:.1} µs per glyph, {:.0} bytes per glyph with padding, {:.1} glyphs per KB of 1 MB page ({} glyphs took {:.1} MB of area)",
            frame.ms * 1e3 / rasterized,
            frame.stats.upload_bytes as f64 / rasterized,
            1e3 / (frame.stats.upload_bytes as f64 / rasterized),
            frame.stats.rasterized,
            frame.stats.used_bytes as f64 / 1e6,
        );
    }
}
