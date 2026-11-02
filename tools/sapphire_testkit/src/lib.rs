//! Test infrastructure for Sapphire (spec §9). A tool crate: nothing that ships depends on it,
//! and other tools run its binary rather than linking it (rule R1).
//!
//! Today it holds golden images: [`golden`] compares images, and [`scenes`] renders the golden
//! scenes with the sandbox and checks them (`cargo xtask golden`). The combat replay runner is a
//! tool of its own, `sapphire_replay`, so its job builds none of this.

pub mod golden;
pub mod scenes;
