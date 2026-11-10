//! Test infrastructure for Sapphire (spec §9). A tool crate: nothing that ships depends on it,
//! and other tools run its binary rather than linking it (rule R1).
//!
//! Today it holds golden images: [`golden`] compares images, and [`scenes`] renders the golden
//! scenes with the sandbox and checks them (`cargo xtask golden`). [`look`] measures lines and
//! two-tone edges in output pixels, for the renderer's GPU tests, which take this crate as a
//! dev-dependency, and for captures (`sapphire_testkit measure`). [`store_shots`] draws golden scenes
//! again at each store's screenshot sizes (`cargo xtask store-shots`). [`flashes`] runs WCAG 2.3.1's
//! flash tests over a capture sequence (`sapphire_testkit flashes`). The combat replay runner is a
//! tool of its own, `sapphire_replay`, so its job builds none of this.

mod output_paths;

pub mod flashes;
pub mod golden;
pub mod look;
pub mod scenes;
pub mod store_shots;
