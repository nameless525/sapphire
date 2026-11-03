//! The combat timeline editor (M2 T3, spec §5.28): a `.moves.ron` on a timeline of 60 Hz ticks, dragged
//! tick by tick, scrubbed against the real simulation and pushed to a running fight.
//!
//! It owns the editing of that one kind of document: the text as the single source of truth
//! ([`edits`], [`text`]), a file's undo, saving and changes on disk ([`mod@file`]), the timeline ([`timeline`]),
//! the preview ([`run`], [`figure`], [`preview`]), and talking to a device ([`link`]). [`app::Editor`] holds
//! them together, and [`app::Editor::ui`] is the whole interface.
//!
//! It owns no rules. What's wrong with a document is what `sapphire_moves`' `check` and `compile` say; what
//! a move does at a tick is what `sapphire_gameplay`'s `Simulation` does; what a device takes is what live
//! link's `Client::push_document` answers (M2 T2). It has no renderer: the preview is egui's, for the reasons
//! [`preview`] gives. Nothing depends on it, and it stays out of the cooker.
//!
//! Everything but the binary's `window` module runs with no window: tests drive [`app::Editor::ui`] through
//! `egui_kittest`, and [`cli::run_capture`] and [`capture`] draw a frame to a PNG on the CPU. Only a person
//! at the machine opens the window: `cargo xtask editor <file>` (`docs/development-iteration.md`).

pub mod app;
pub mod capture;
pub mod cli;
pub mod cooked;
pub mod edits;
pub mod figure;
pub mod file;
pub mod input;
pub mod link;
pub mod preview;
pub mod recovery;
pub mod run;
pub mod text;
pub mod timeline;

pub use app::{Editor, Options};
