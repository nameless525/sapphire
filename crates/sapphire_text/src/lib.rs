//! Fonts, shaping and the glyph atlas for the on-screen interface (spec §5.17, M3 U1).
//!
//! # Owns
//!
//! - **Fonts** ([`TextSystem::register_font`]): a `parley` collection of fonts the game hands it as
//!   bytes. It never asks the OS for one: built without `parley`'s `system` feature, on no platform
//!   is there code here that could. A character no registered font covers draws as that font's
//!   missing glyph.
//! - **Shaping** ([`TextSystem::shape`]): text and a [`TextStyle`] to a flat [`ShapedText`], cached,
//!   so text that hasn't changed costs a hash and allocates nothing.
//! - **The glyph atlas** ([`GlyphAtlas`]): coverage from `swash` in pages from `etagere`, with the
//!   new glyphs' texels handed to the frame's [`UiDrawList`](sapphire_look::UiDrawList), which is how
//!   they reach the renderer (rule R4).
//!
//! # Does not own
//!
//! - **Where text goes.** Boxes and widgets are `sapphire_ui`'s; this takes an origin in points.
//! - **What text says:** strings come in already chosen and formatted.
//! - **Files.** Bytes come from whoever read the pack.
//! - **HUD digits,** which stay on `sapphire_ui`'s distance field atlas and never shape.
//!
//! # Why a crate of its own
//!
//! `sapphire_ui` is plain quads that allocate nothing and link nothing. `parley` brings ICU's data
//! and `harfrust`, and the cooker's checks need to read fonts through the same code without linking
//! widgets. Nothing above L3 but an app or `sapphire_ui` needs it.
//!
//! # The clock
//!
//! There isn't one: [`GlyphAtlas::begin_frame`] counts frames, and age is measured in them.

mod atlas;
mod shape;

pub use atlas::{AtlasConfig, AtlasStats, GlyphAtlas};
pub use parley::fontique::{FamilyId, Script};
pub use shape::{ShapeStats, ShapedGlyph, ShapedRun, ShapedText, TextStyle, TextSystem};
