//! The combat timeline editor (M2 T3, spec §5.28): a `.moves.ron` on a timeline of 60 Hz ticks.

pub mod app;
pub mod capture;
pub mod cooked;
pub mod edits;
pub mod figure;
pub mod file;
pub mod link;
pub mod preview;
pub mod run;
pub mod text;
pub mod timeline;

pub use app::{Editor, Options};
