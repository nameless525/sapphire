//! Test infrastructure for Sapphire (spec §9). A tool crate: nothing that ships depends on it,
//! and other tools run its binary rather than linking it (rule R1).
//!
//! Today it holds golden images: [`golden`] compares images, and [`scenes`] renders the golden
//! scenes with the sandbox and checks them (`cargo xtask golden`). The headless world and the
//! combat replay runner join it as those systems land.

pub mod golden;
pub mod scenes;
