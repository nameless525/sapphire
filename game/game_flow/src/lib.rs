//! The game's flow (M3 G1): where the game is, from the title through a run's beats to a defeat or a win,
//! and what it keeps on disk between runs.
//!
//! - **The document** ([`document`]): `content/game.flow.ron`, a [`FlowDocument`](document::FlowDocument)
//!   the game's manifest names. Which scenario a run stands, in which zone, its beats (when an encounter
//!   starts or is cleared, a timeline ends or a checkpoint is reached: a timeline, a dialogue or the win),
//!   and the music and holds of the flow's own moments. A game's flow needs no Rust.
//! - **The machine** ([`flow`]): [`Flow`](flow::Flow) reads a run only through plain questions
//!   ([`FlowView`](flow::FlowView)) after each step and acts on it only through what it returns, commands
//!   the caller records. It runs between steps and never in one, so a replay needs no flow, and editing one
//!   moves no checksum. Its observation allocates nothing.
//! - **What's kept** ([`kept`]): [`Kept`](kept::Kept), one writer for the settings and the save alike,
//!   which snapshots on the frame and writes on a job, the newest winning.
//! - **The screens** ([`menus`]): what a menu's row asks of the flow ([`flow_command`](menus::flow_command)),
//!   and the title's and the HUD's models filled from it.
//!
//! **Not here:** the simulation, the stage and the interface, which the flow drives through its caller; the
//! fight (`game_combat`) and the screens (`game_ui`), which know nothing of it. The screens are linked for
//! their commands and models; the fight is reached as G1's later commits land on what they need.

pub mod document;
pub mod flow;
pub mod kept;
pub mod menus;

/// Where the flow logs.
pub(crate) const LOG_TARGET: &str = "game::flow";
