//! A subset does everything the whole font does for the strings it was cut for: the runtime's stack
//! reads it, shapes it to the same positions with the same line breaks, and rasterizes the same
//! coverage at any weight. Needs `SAPPHIRE_TEST_FONTS` (`tests/common` in `sapphire_text`).

#![allow(clippy::print_stdout, reason = "the subset's size and time are what this test reports")]
#![allow(clippy::disallowed_methods, reason = "timing a cook needs the OS clock, and nothing here ships")]

#[path = "../../../crates/sapphire_text/tests/common/mod.rs"]
mod common;

use std::sync::Arc;
use std::time::Instant;

use common::{Item, MPLUS1, menu_screen, read_font, style};
use sapphire_look::UiDrawList;
use sapphire_pipeline::font_subset::subset;
use sapphire_text::{AtlasConfig, GlyphAtlas, TextStyle, TextSystem};

/// Rough proportions of what a real table needs beyond its own strings: full ASCII, kana and the
/// punctuation any string can gain from a substitution.
fn base_characters() -> String {
    let ascii = '\u{20}'..='\u{7E}';
    let kana = ('\u{3041}'..='\u{3096}').chain('\u{30A1}'..='\u{30FA}');
    let punctuation = "、。・ー「」『』（）【】！？：；〜…―％＋－×÷".chars();
    ascii.chain(kana).chain(punctuation).collect()
}

/// What shapes and rasterizes must match between two fonts of one family, at every weight asked.
fn assert_same(whole: Arc<Vec<u8>>, cut: Arc<Vec<u8>>, items: &[Item], family: &str) {
    let mut a = TextSystem::new();
    let mut b = TextSystem::new();
    a.register_font(whole);
    b.register_font(cut);
    let names = (a.family_names(), b.family_names());
    assert_eq!(names.0, names.1, "the family's name survives");
    let (mut glyphs, mut bytes) = (0, 0);
    for item in items {
        for weight in [400.0, 700.0] {
            let style = TextStyle { weight, ..item.style };
            let (first, second) = (
                a.shape(item.text, &style, 1.5, item.max_width).clone(),
                b.shape(item.text, &style, 1.5, item.max_width).clone(),
            );
            let positions = |text: &sapphire_text::ShapedText| {
                text.glyphs.iter().map(|glyph| (glyph.x, glyph.y)).collect::<Vec<_>>()
            };
            assert_eq!(positions(&first), positions(&second), "{family} {weight}: {}", item.text);
            assert_eq!(first.lines, second.lines, "{family} {weight}: {}", item.text);
            assert_eq!(first.baselines, second.baselines, "{family} {weight}: {}", item.text);
            assert_eq!(first.runs.len(), second.runs.len(), "{}", item.text);
            for (x, y) in first.runs.iter().zip(&second.runs) {
                assert_eq!(x.coords, y.coords, "{family} {weight}: the axis survives");
            }
            assert!(
                second.glyphs.iter().all(|glyph| glyph.id != 0)
                    || first.glyphs.iter().any(|glyph| glyph.id == 0),
                "{family}: a character the subset lost: {}",
                item.text,
            );
            // New atlases, so each uploads this text's glyphs in the same order.
            let coverage = |shaped: &sapphire_text::ShapedText| {
                let config = AtlasConfig { page_size: 2048, max_pages: 8, ..AtlasConfig::default() };
                let mut atlas = GlyphAtlas::new(config);
                let mut list = UiDrawList::default();
                atlas.draw(shaped, [0.0; 2], 1.5, [255; 4], &mut list);
                (list.page_uploads, list.upload_texels)
            };
            let (first, second) = (coverage(&first), coverage(&second));
            assert_eq!(first.0, second.0, "{family} {weight}: {}", item.text);
            assert!(first.1 == second.1, "{family} {weight}: different coverage for {}", item.text);
            glyphs += first.0.len();
            bytes += first.1.len();
        }
    }
    println!(
        "{family}: {} strings at two weights the same, whole and subset: {glyphs} glyphs, {bytes} bytes of coverage",
        items.len()
    );
}

#[test]
fn a_subset_shapes_and_rasterizes_like_the_whole_font() {
    let Some(dir) = common::fonts_dir("a_subset_shapes_and_rasterizes_like_the_whole_font") else { return };
    let whole = read_font(&dir, MPLUS1);
    let japanese = menu_screen(true);
    let english = menu_screen(false);
    let strings: String = japanese.iter().chain(&english).flat_map(|item| item.text.chars()).collect();

    let start = Instant::now();
    let cut = Arc::new(subset(&whole, &strings).expect("a subset"));
    println!(
        "M PLUS 1, a menu's {} distinct characters: {:.2} MB to {:.0} KB in {:.0} ms",
        distinct(&strings),
        whole.len() as f64 / 1e6,
        cut.len() as f64 / 1e3,
        start.elapsed().as_secs_f64() * 1e3,
    );
    let body: Vec<Item> = japanese
        .iter()
        .chain(&english)
        .map(|item| Item { style: TextStyle { families: common::FAMILY_BODY, ..item.style }, ..*item })
        .collect();
    assert_same(Arc::clone(&whole), Arc::clone(&cut), &body, "M PLUS 1");

    // What cook's sizes look like as the table grows.
    let base = base_characters();
    let font = swash::FontRef::from_index(&whole, 0).expect("a font");
    let charmap = font.charmap();
    let han = ('\u{4E00}'..='\u{9FFF}').filter(|character| charmap.map(*character) != 0);
    let han_total = han.clone().count();
    println!(
        "M PLUS 1 covers {han_total} of the 20,992 unified Han, in {} glyphs",
        font.metrics(&[]).glyph_count
    );
    for extra in [0, 500, 1000, 2136, 3000, han_total] {
        let table: String = base.chars().chain(han.clone().take(extra)).chain(strings.chars()).collect();
        let start = Instant::now();
        let cut = subset(&whole, &table).expect("a subset");
        println!(
            "base plus {extra} Han, {} distinct characters: {:.0} KB in {:.0} ms",
            distinct(&table),
            cut.len() as f64 / 1e3,
            start.elapsed().as_secs_f64() * 1e3,
        );
    }
    if let Some(out) = std::env::var_os("SAPPHIRE_TEST_SUBSET_OUT") {
        let table: String = base.chars().chain(han.take(2136)).chain(strings.chars()).collect();
        std::fs::write(out, subset(&whole, &table).expect("a subset")).expect("written");
    }
}

#[test]
fn the_display_faces_survive_too() {
    let Some(dir) = common::fonts_dir("the_display_faces_survive_too") else { return };
    for (file, family, sample, locale) in [
        (common::OXANIUM, "Oxanium", "SETTINGS Level 1,284 AVATAR fi", "en"),
        (common::DELA, "Dela Gothic One", "設定と記録 必殺技ゲージ Lv.90", "ja"),
    ] {
        let whole = read_font(&dir, file);
        let start = Instant::now();
        let cut = Arc::new(subset(&whole, sample).expect("a subset"));
        println!(
            "{family}: {:.0} KB to {:.1} KB in {:.0} ms",
            whole.len() as f64 / 1e3,
            cut.len() as f64 / 1e3,
            start.elapsed().as_secs_f64() * 1e3,
        );
        let item = Item { text: sample, style: style(family, 24.0, 400.0, locale), max_width: None };
        assert_same(whole, cut, &[item], family);
    }
}

fn distinct(text: &str) -> usize {
    text.chars().collect::<std::collections::BTreeSet<_>>().len()
}
