//! Display text from a face's field (M3 U8): which glyphs take it, where their quads land, and that
//! they take nothing from the pages. The field here is made up over Ubuntu Light's glyph ids, since
//! fields are the cooker's to make (`sapphire_pipeline`'s `font_field` tests make real ones); what's
//! under test is the choice and the placement.

use std::sync::Arc;

use sapphire_look::{UI_COVERAGE, UI_MULTI_FIELD, UiAtlas, UiDrawList, UiFieldFormat, UiPageState, UiSource};
use sapphire_text::{AtlasConfig, FaceOptions, FieldFace, FieldGlyph, GlyphAtlas, TextStyle, TextSystem};

const TITLE: &str = "Title";
/// Texels to the em of the made-up field.
const EM: f32 = 48.0;

fn style(size: f32) -> TextStyle<'static> {
    TextStyle { families: "Ubuntu", size, weight: 400.0, line_height: 1.0, locale: Some("en") }
}

fn font() -> Arc<Vec<u8>> {
    Arc::new(epaint_default_fonts::UBUNTU_LIGHT.to_vec())
}

/// The ids `TITLE`'s glyphs shape to, in order.
fn title_ids() -> Vec<u32> {
    let mut text = TextSystem::new();
    text.register_font(font());
    text.shape(TITLE, &style(64.0), 1.0, None).glyphs.iter().map(|glyph| glyph.id).collect()
}

/// A field holding every glyph of `TITLE` but `left_out`, each in an 8 × 10 cell along the top.
fn field(left_out: u32) -> Arc<FieldFace> {
    let atlas = UiAtlas {
        width: 64,
        height: 16,
        spread: 4.0,
        format: UiFieldFormat::Multi,
        texels: vec![0; 64 * 16 * 4],
    };
    let mut ids = title_ids();
    ids.sort_unstable();
    ids.dedup();
    let glyphs = ids
        .iter()
        .filter(|&&id| id != left_out)
        .enumerate()
        .map(|(at, &id)| FieldGlyph { id, cell: [at as u16 * 8, 0, 8, 10], offset: [-0.125, -0.875] })
        .collect();
    Arc::new(FieldFace::new(Arc::new(atlas), EM, Vec::new(), glyphs).unwrap())
}

fn text_with(field: &Arc<FieldFace>) -> TextSystem {
    let mut text = TextSystem::new();
    text.register_face(font(), FaceOptions { field: Some(Arc::clone(field)), ..FaceOptions::default() });
    text
}

#[test]
fn large_glyphs_draw_from_the_field_and_what_it_lacks_from_the_pages() {
    let left_out = title_ids()[1];
    let field = field(left_out);
    let mut text = text_with(&field);
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    atlas.begin_frame(UiPageState::NO_RENDERER);
    let shaped = text.shape(TITLE, &style(64.0), 1.0, None).clone();
    atlas.draw(&shaped, [10.0, 20.0], 1.0, [255; 4], &mut list);

    let stats = atlas.stats();
    assert_eq!(stats.field_quads, 4, "four of five glyphs from the field");
    assert_eq!(stats.rasterized, 1, "the one it lacks, from a page");
    assert_eq!(list.fields.len(), 1);
    assert!(Arc::ptr_eq(&list.fields[0], field.atlas()));
    let sources: Vec<_> = list.runs.iter().map(|run| (run.source, run.count)).collect();
    assert_eq!(
        sources,
        [(UiSource::Field(0), 1), (UiSource::Page(0), 1), (UiSource::Field(0), 3)],
        "in the text's order, so a fallback glyph keeps its place"
    );
    assert_eq!(list.quads[0].texels, UI_MULTI_FIELD);
    assert_eq!(list.quads[1].texels, UI_COVERAGE);

    // The first glyph's quad: its cell's corner at the pen plus the offset, in pixels to the em, and
    // its cell's size at 64 / 48 pixels a texel. Unsnapped, as a field has no pixel grid.
    let pen = [10.0 + shaped.glyphs[0].x, (20.0 + shaped.glyphs[0].y).round()];
    let quad = list.quads[0];
    assert_eq!(quad.origin, [pen[0] - 0.125 * 64.0, pen[1] - 0.875 * 64.0]);
    let texel = 64.0 / EM;
    assert_eq!([quad.axis_x[0], quad.axis_y[1]], [8.0 * texel, 10.0 * texel]);
    assert_eq!(quad.uv, [0.0, 0.0, 8.0 / 64.0, 10.0 / 16.0]);
}

#[test]
fn under_the_field_s_size_every_glyph_draws_from_the_pages() {
    let field = field(u32::MAX);
    let mut text = text_with(&field);
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    for (size, scale, fields) in [(47.0, 1.0, 0), (48.0, 1.0, 5), (24.0, 2.0, 5), (16.0, 2.0, 0)] {
        atlas.begin_frame(UiPageState::NO_RENDERER);
        list.reset();
        let shaped = text.shape(TITLE, &style(size), scale, None);
        atlas.draw(shaped, [0.0; 2], scale, [255; 4], &mut list);
        assert_eq!(atlas.stats().field_quads, fields, "{size} points at {scale} pixels to the point");
    }
}

#[test]
fn warming_a_title_from_the_field_rasterizes_nothing() {
    let field = field(u32::MAX);
    let mut text = text_with(&field);
    let mut atlas = GlyphAtlas::new(AtlasConfig { page_size: 512, ..AtlasConfig::default() });
    let mut list = UiDrawList::default();
    atlas.begin_frame(UiPageState::NO_RENDERER);
    let mut budget = 0;
    assert!(atlas.warm(text.shape(TITLE, &style(96.0), 1.0, None), &mut budget, &mut list), "nothing to do");
    assert!(list.page_uploads.is_empty() && atlas.stats().rasterized == 0);
}
