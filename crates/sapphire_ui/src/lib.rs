//! The game's on-screen interface (spec §5.17): the retained tree every screen stands on (M3 U4), and
//! what a HUD is drawn with: quads over a generated atlas, drawn after post.
//!
//! # Owns
//!
//! - **The retained tree** ([`tree`]): a screen from a [`UiDocument`](sapphire_assets::ui::UiDocument):
//!   `taffy` layout in points, strings through `sapphire_loc`, values through reflection paths,
//!   nine-slices, gradients and masks. And what moves it (M3 U5): timelines, a screen's machine,
//!   scrolling, and one focus model that a keyboard, a pad and a pointer all reach as plain [`UiInput`].
//! - **Layout** ([`layout`]): rectangles in UI points, the frame's metrics ([`UiMetrics`]: size and
//!   safe-area insets, from a [`Window`](sapphire_platform::Window) or made up for a capture) and
//!   anchors. A UI point is `pixels / (display scale × player's scale)`, so a thumb's size is the
//!   same everywhere.
//! - **The atlas** ([`atlas`]): every shape as a signed distance field, painted by code at load.
//! - **Painting** ([`paint`]): shapes into [`sapphire_look::UiQuad`]s, in points.
//! - **What the touch zones read** ([`touch`]), as plain [`TouchActions`]: the zones themselves, fingers to
//!   a floating stick, four buttons (the ultimate's only while there's one) and a camera drag, are `game_ui`'s (M3 S2).
//! - **The combat HUD** ([`hud`]): where everything goes ([`HudLayout`], shared by what's drawn and
//!   what's hit-tested), plain [`HudValues`] and how it all draws and animates ([`CombatHud`]).
//!
//! # Does not own
//!
//! - **Drawing.** It writes a [`UiDrawList`], which the frame packet
//!   carries and the renderer draws. There is no edge to the renderer or to wgpu either way (rule
//!   R4): the list's types are `sapphire_look`'s, beneath both.
//! - **What a press means.** [`TouchActions`] are values and edges: an app turns them into intents,
//!   directly or through `sapphire_input`'s `Virtual` sources, as `sapphire_controls`' `read_touch` does
//!   for combat. No gameplay here.
//! - **Where the numbers come from:** health and poise (G3) and the projection of a hit to the screen
//!   reach it as plain values.
//! - **The OS clock** (rule R2): the app passes [`Platform::now`](sapphire_platform::Platform::now),
//!   to the HUD and in [`UiFrame::now`] alike.
//!   Nothing else moves the animation, so tests and captures choose their own time.
//!
//! # Driving it
//!
//! ```ignore
//! let layout = HudLayout::new(UiMetrics::of_window(&window, 1.0));
//! let mut hud = CombatHud::new();
//!
//! // Each frame, on the real clock, with `actions` from `game_ui`'s touch zones:
//! hud.update(platform.now(), &values, actions);
//! hud.spawn_damage(1284, screen_position, DamageKind::Normal);
//! hud.draw(&layout, actions, &mut packet.ui);
//! ```
//!
//! # Allocation
//!
//! Steady-state frames allocate nothing: damage numbers are a fixed array and the draw list is the packet's
//! (`game_ui`'s `tests/allocations.rs`, with the zones' finger slots and event list). The atlas is
//! the one allocation, at load: 69,120 bytes, painted in about 0.85 ms (release, Apple M5).
//!
//! The sandbox's fixed frame is 41 quads, written in 2.3 µs on the same machine.
//!
//! # What stays here after M3
//!
//! The combat HUD's bars, buttons, stick and numbers are [`CombatHud`]'s, placed by [`HudLayout`] from
//! `default_hud.ron`'s numbers, since their goldens are pinned and they animate per frame. U7 put the
//! HUD's words and every menu on the tree, and its digits stay on the distance field atlas.

/// Where this crate logs.
pub(crate) const LOG_TARGET: &str = "sapphire::ui";

pub mod atlas;
pub mod hud;
pub mod layout;
pub mod paint;
pub mod touch;
pub mod tree;

pub use atlas::{Atlas, Sprite, Sprites};
pub use hud::{
    BarsDesign, BarsLayout, ButtonDesign, ButtonLayout, CombatHud, DamageKind, EnemyBarsDesign,
    FighterValues, HudDesign, HudDesignError, HudLayout, HudStats, HudValues, MAX_OTHER_FOES, MeterDesign,
    OtherFoes, PlayerBarsDesign, SkillValues, StickDesign, StickLayout, UltimateValues,
};
pub use layout::{Anchor, Insets, Rect, UiMetrics};
pub use paint::{Align, Paint, Painter};
pub use sapphire_look::{UiAtlas, UiDrawList, UiQuad};
pub use touch::{Button, ButtonEvent, ButtonEventKind, ButtonState, StickState, TouchActions};
pub use tree::{
    PROBLEM_COLOR, UiEvent, UiFonts, UiFrame, UiHit, UiInput, UiInsetShot, UiNodeId, UiPointer,
    UiPointerKind, UiPointerPhase, UiProblem, UiStep, UiTree, UiTreeError, face_of,
};
