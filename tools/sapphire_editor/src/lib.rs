//! The combat timeline editor (M2 T3, spec §5.28): a `.moves.ron` on a timeline of 60 Hz ticks, dragged
//! tick by tick, scrubbed against the real simulation and pushed to a running fight.
//!
//! It's a docked shell ([`shell`]) of panes over `sapphire_document`, which owns the text, its history, saving
//! and changes on disk. This crate owns the timeline ([`edits`], [`timeline`]), the preview ([`run`],
//! [`figure`], [`preview`]), an asset browser, a reflection inspector and a cvar panel (all in [`app`]),
//! talking to a device ([`link`]), the real renderer in a pane ([`viewport`]) and a run from a snapshot of the
//! documents ([`play`]). [`app::Editor`] holds them
//! together, and [`app::Editor::ui`] is the whole interface.
//!
//! It owns no rules. What's wrong with a document is what `sapphire_moves`' `check` and `compile` say; what
//! a move does at a tick is what `sapphire_gameplay`'s `Simulation` does; what a device takes is what live
//! link's `Client::push_document` answers (M2 T2). What a frame looks like is what `sapphire_stage` draws, by the
//! one route a game's frames take, and the Preview pane stays egui's flat figure, for the reasons [`preview`] gives.
//! Nothing depends on it, and it stays out of the cooker.
//!
//! Everything but the binary's `window` module runs with no window: tests drive [`app::Editor::ui`] through
//! `egui_kittest`, [`cli::run_capture`] and [`capture`] draw a frame to a PNG on the CPU, and a viewport's tests
//! draw on a device with no surface. Only a person at the machine opens the window: `cargo xtask editor <file>`
//! (`docs/development-iteration.md`).

pub mod app;
pub mod capture;
pub mod cli;
pub mod cooked;
pub mod edits;
pub mod figure;
pub mod gizmo;
pub mod input;
pub mod judge;
pub mod link;
pub mod play;
pub mod preview;
pub mod recovery;
pub mod run;
pub mod shell;
pub mod timeline;
pub mod viewport;

pub use app::{Current, Editor, Options};
