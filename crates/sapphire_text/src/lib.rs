//! Fonts, shaping and the glyph atlas for the on-screen interface (spec §5.17, M3 U1).
//!
//! # Owns
//!
//! - **Fonts** ([`TextSystem::register_font`]): a `parley` collection of fonts the game hands it as
//!   bytes. It never asks the OS for one: built without `parley`'s `system` feature, on no platform
//!   is there code here that could. A character no registered font covers draws as that font's
//!   missing glyph.
//! - **Shaping** ([`TextSystem::shape`]): text and a [`TextStyle`] to a flat [`ShapedText`], cached,
//!   so text that hasn't changed costs a hash and allocates nothing. A script's fallbacks are the
//!   style's list of families ([`family_list`]), which a project's font chains say.
//! - **The glyph atlas** ([`GlyphAtlas`]): coverage from `swash` in pages from `etagere`, with the
//!   new glyphs' texels handed to the frame's [`UiDrawList`](sapphire_look::UiDrawList), which is how
//!   they reach the renderer (rule R4).
//!
//! # Does not own
//!
//! - **Where text goes.** Boxes and widgets are `sapphire_ui`'s; this takes an origin in points.
//! - **What text says:** strings come in already chosen and formatted.
//! - **Files.** Bytes come from whoever read the pack: a cooked font is `sapphire_assets::font::Font`,
//!   whose `file` goes to [`TextSystem::register_face`] with its `baseline_shift`.
//! - **HUD digits,** which stay on `sapphire_ui`'s distance field atlas and never shape.
//!
//! # Why a crate of its own
//!
//! `sapphire_ui` is plain quads that link next to nothing. `parley` brings 60 crates, ICU's data
//! among them, which a tool checking a font or a test of a touch zone shouldn't have to build. Only
//! `sapphire_ui`'s text and an app need it.
//!
//! # The clock
//!
//! There isn't one: [`GlyphAtlas::begin_frame`] counts frames, and age is measured in them.

mod atlas;
mod shape;

pub use atlas::{AtlasConfig, GlyphAtlas};
pub use parley::fontique::FamilyId;
pub use shape::{
    FaceOptions, ShapeStats, ShapedGlyph, ShapedRun, ShapedText, TYPE_SCALE, TextStyle, TextSystem,
    family_list,
};
