//! Fonts cut down to what a pack's strings use (M3 U1, spec §5.25). The whole font stays byte-identical
//! under `content/third_party/`, and what's shipped is a cook product.
//!
//! Layout features and the variable axes survive, so what the runtime shapes from a subset lands where
//! it would from the whole font (`tests/font_subset.rs`). Hinting doesn't: the runtime rasterizes
//! unhinted. `skera` can't pin an axis yet, so a subset keeps every weight.
//!
//! `skera` is on a newer line of the fontations crates than `parley`, so it's here and never in the
//! runtime's tree (`[tool_only_dependencies]` in xtask/layers.toml).

use read_fonts_for_skera::collections::IntSet;
use read_fonts_for_skera::tables::{ebdt, eblc, feat, svg};
use read_fonts_for_skera::types::{GlyphId, NameId, Tag};
use read_fonts_for_skera::{FontRef, TopLevelTable as _};
use skera::{
    DEFAULT_LAYOUT_FEATURES, DSIG, EBSC, GLAT, GLOC, JSTF, KERN, KERX, LTSH, MORT, MORX, PCLT, Plan, SILF,
    SILL, SubsetFlags, subset_font,
};

/// A font that couldn't be subset.
#[derive(Debug, thiserror::Error)]
pub enum FontSubsetError {
    #[error("not a font: {0}")]
    NotAFont(String),
    #[error("couldn't subset it: {0}")]
    Subset(String),
}

/// `font`, with the glyphs `text` needs and what those glyphs reach through its layout features.
pub fn subset(font: &[u8], text: &str) -> Result<Vec<u8>, FontSubsetError> {
    let font = FontRef::new(font).map_err(|error| FontSubsetError::NotAFont(error.to_string()))?;
    let mut unicodes = IntSet::<u32>::empty();
    unicodes.extend(text.chars().map(u32::from));

    // What `hb-subset` drops by default.
    let drop_tables: IntSet<Tag> = [
        MORX,
        MORT,
        KERX,
        KERN,
        JSTF,
        DSIG,
        ebdt::Ebdt::TAG,
        eblc::Eblc::TAG,
        EBSC,
        svg::Svg::TAG,
        PCLT,
        LTSH,
        feat::Feat::TAG,
        GLAT,
        GLOC,
        SILF,
        SILL,
    ]
    .into_iter()
    .collect();
    // Family and style names: fontique finds a family by them, and the OFL's notices are 0 and 13.
    let mut name_ids = IntSet::<NameId>::empty();
    name_ids.insert_range(NameId::from(0)..=NameId::from(6));
    name_ids.extend([13, 14, 16, 17].map(NameId::from));
    let mut name_languages = IntSet::<u16>::empty();
    name_languages.insert(0x0409);
    let mut layout_scripts = IntSet::<Tag>::empty();
    layout_scripts.invert();
    let mut layout_features = IntSet::<Tag>::empty();
    layout_features.extend(DEFAULT_LAYOUT_FEATURES.iter().copied());

    let plan = Plan::new(
        &IntSet::<GlyphId>::empty(),
        &unicodes,
        &font,
        SubsetFlags::SUBSET_FLAGS_NO_HINTING,
        &drop_tables,
        &layout_scripts,
        &layout_features,
        &name_ids,
        &name_languages,
    );
    subset_font(&font, &plan).map_err(|error| FontSubsetError::Subset(error.to_string()))
}
