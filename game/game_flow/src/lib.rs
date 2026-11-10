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
//! - **The save** ([`progress`]): [`Progress`](progress::Progress), a run's zone and its place there (B4's
//!   `ZoneStart`), and the [`RunStart`](sapphire_gameplay::replay::RunStart) a run continued from it starts
//!   from.
//! - **The screens** ([`menus`]): what a menu's row asks of the flow ([`flow_command`](menus::flow_command)),
//!   and the title's and the HUD's models filled from it.
//! - **The game** ([`game`]): [`Game`](game::Game), the flow over a game's runs: which run stands, from a
//!   new game's start or the save's place, what a step's beats ask of it as recorded commands, and when it's
//!   saved. [`FlowContent`](game::FlowContent) reads the flow a game's content names.
//! - **The stage** ([`show`]): [`GameShow`](show::GameShow), what stands on the game's stage: the fight, its
//!   HUD, the menus and, with a game, its flow deciding which screen is up and which run plays. The game's
//!   app and the sandbox's `--game` both stand it.
//! - **With no window** ([`capture`]): a game drawn to a frame it reads back, for the game's `--capture` and
//!   the sandbox's `--game`.
//! - **The boot** ([`boot`]): [`Boot`](boot::Boot), from the game's first frame to the title, its screen drawing
//!   while the renderer and what the game draws through build where no frame waits (M4 H2).
//!
//! **Not here:** the simulation, the fight (`game_combat`) and the screens (`game_ui`), which know nothing of
//! the flow; the window's loop, which is the app's.

pub mod boot;
pub mod capture;
pub mod document;
pub mod flow;
pub mod game;
pub mod kept;
pub mod menus;
pub mod progress;
pub mod show;

/// Where the flow logs.
pub(crate) const LOG_TARGET: &str = "game::flow";
