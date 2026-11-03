//! What the tree's tests share: the fonts, `sapphire_loc`'s fixture tables in two languages and a model.

#![allow(dead_code, reason = "each test binary uses its own share of this")]

use std::path::Path;
use std::sync::Arc;

use bevy_reflect::Reflect;
use sapphire_assets::ui::UiDocument;
use sapphire_loc::{LocaleSettings, Localizer, StringTable};
use sapphire_look::{UiDrawList, UiPageState, UiQuad};
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;
use sapphire_ui::{Atlas, Insets, UiFonts, UiFrame, UiMetrics, UiTree};

const EN: &[u8] = include_bytes!("../../../sapphire_loc/testdata/cooked/en.sloc");
const JA: &[u8] = include_bytes!("../../../sapphire_loc/testdata/cooked/ja.sloc");

/// A localizer showing `text`.
pub fn localizer(text: &str) -> Localizer {
    let mut loc = Localizer::new(LocaleSettings { text: text.to_owned(), voice: "en".to_owned() });
    loc.install(StringTable::read(EN).unwrap()).unwrap();
    loc.install(StringTable::read(JA).unwrap()).unwrap();
    loc
}

/// The fonts, from where they're kept, or `SAPPHIRE_TEST_FONTS` on a device.
pub fn fonts() -> UiFonts {
    let dir = std::env::var_os("SAPPHIRE_TEST_FONTS")
        .map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party"), Into::into);
    let mut fonts = UiFonts::default();
    for font in
        ["mplus1/MPLUS1[wght].ttf", "oxanium/Oxanium[wght].ttf", "dela_gothic_one/DelaGothicOne-Regular.ttf"]
    {
        let path = dir.join(font);
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        fonts.register_font(Arc::new(bytes), 0.0);
    }
    fonts
}

/// What the test documents read.
#[derive(Clone, Debug, Default, Reflect)]
pub struct Model {
    pub player: Player,
    pub stance: Option<f32>,
    pub party: Vec<Player>,
    pub name: String,
}

#[derive(Clone, Debug, Default, Reflect)]
pub struct Player {
    pub health: f32,
    pub shown: bool,
    pub enemies: i32,
}

/// A document from what its `data` says, with the envelope added.
pub fn document(data: &str) -> UiDocument {
    let text = format!("(type: \"sapphire::assets::UiDocument\", version: 2, data: ({data}))");
    UiDocument::from_document(&text).unwrap_or_else(|error| panic!("{error}\n{text}"))
}

/// A frame of `size` points, with nothing covered.
pub fn metrics(size: [f32; 2]) -> UiMetrics {
    UiMetrics::new(Vec2::from(size), Insets::default())
}

/// `seconds` on the clock a tree is handed.
pub fn at(seconds: f32) -> Timestamp {
    Timestamp::from_nanos((f64::from(seconds) * 1e9) as u64)
}

/// Runs a frame over `tree` at the clock's start, and writes it into `list`, with no renderer behind it.
pub fn frame(
    tree: &mut UiTree,
    fonts: &mut UiFonts,
    loc: &Localizer,
    model: &Model,
    metrics: UiMetrics,
    atlas: &Atlas,
    list: &mut UiDrawList,
) {
    frame_at(at(0.0), tree, fonts, loc, model, metrics, atlas, list);
}

/// Runs a frame over `tree` at `now`.
#[allow(clippy::too_many_arguments, reason = "a test's whole frame, spelled out")]
pub fn frame_at(
    now: Timestamp,
    tree: &mut UiTree,
    fonts: &mut UiFonts,
    loc: &Localizer,
    model: &Model,
    metrics: UiMetrics,
    atlas: &Atlas,
    list: &mut UiDrawList,
) {
    list.reset();
    fonts.begin_frame(UiPageState::NO_RENDERER, loc);
    tree.update(&UiFrame { metrics, now, loc, model: Some(model) }, fonts);
    tree.draw(fonts, atlas, list);
}

/// The quads that are glyphs: a page's coverage rather than the atlas's field.
pub fn glyphs(list: &UiDrawList) -> impl Iterator<Item = &UiQuad> {
    list.quads.iter().filter(|quad| quad.softness < 0.0)
}
