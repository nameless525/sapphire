//! The development overlay (spec §5.20), on egui and `egui-wgpu`. Development builds only:
//! it is a `dev_only` crate (`xtask/layers.toml`), which only an app reaches, through an
//! optional dependency behind its `dev` feature (rule R8). Shipping builds don't link it.
//!
//! # Owns
//!
//! - **The SDL input adapter** ([`input`]): platform events into egui's input (pointer, wheel,
//!   keys, text, touch, modifiers, DPI scale), and [`route`], which decides per event what the
//!   overlay takes and what the game keeps, so the game never also acts on input the overlay
//!   used.
//! - **Drawing**: egui's meshes as a frame-graph pass after post, over the finished frame, in
//!   the target's format (sRGB targets get linearized colors, so they show as egui means them).
//! - **Panels**: stats and budgets (frame time, CPU stages against spec §3.2, the GPU frame,
//!   the prepasses (M1 C5), temporal AA's passes and post's (bloom, tonemap and LUT, M1 P2)
//!   against §3.3, allocations, the quality tier with the render scale and dynamic resolution's
//!   state, frame pacing (the platform's `FramePacing`, M1 F7 and F8: what paces frames, the
//!   target rate and the frames on time, missed, doubled and held back), `RenderStats` with the character
//!   prepass's draws and readers (M1 C5), temporal AA's sizes, jitter and history resets, the
//!   compositing layers post drew and the fog's layers and cards (M1 W3),
//!   `GraphStats` and the transient pool, the last-used input device, the asset server's
//!   `AssetStats` against its budgets, hot reload's state and the errors of its last change,
//!   M1 A8, and live link's, M1 A9), the
//!   frame-time graph, the frame graph (passes in order, culled passes, transients and their
//!   pooled textures, command buffers) with per-pass GPU timings, a cvar list and dev console,
//!   time controls, and the time-of-day scrubber (`time_of_day`, M1 W1: the day strip, the
//!   resolved values and a character preview, which shows the first character's light rig, M1
//!   C2), and the screen effects panel (`effects`, M1 P3: fire a preset, watch the stack).
//!   The renderer's debug views (`sapphire_render::debug_view`, M1 F5: wireframe, overdraw,
//!   light lists, material IDs, ramp rows and outline widths) are a list in the cvar panel
//!   (`r.debug_view`), and the stats panel's "Debug view" row names the one shown with its
//!   legend's values.
//! - **[`TimeControls`]**: pause, single-step and speed on the world clock's time-scale stack.
//! - **[`TodControls`]**: scrubbing the hour, pausing the cycle and setting the day's length on
//!   the world's `sapphire_world::tod::TimeOfDay`.
//! - **[`EffectControls`]**: pushing screen effect presets onto the world's
//!   `sapphire_world::screen_effects::EffectStack`, releasing and clearing them.
//! - **Its own cvars** ([`CVARS`]): `debug.ui_scale` and `debug.gpu_timing`.
//!
//! # Does not own
//!
//! - Cvars themselves: [`sapphire_core::cvar`], so runtime crates declare them without depending
//!   on this crate (the facade of spec §5.20).
//! - What it shows: the app gathers each frame's numbers into an [`OverlayFrame`], time of day
//!   included (the world's `TimeOfDay`, read-only: the panel changes it through
//!   [`TodControls`]), the frame packet's characters with their resolved light rigs, and the
//!   effect stack with its presets (read-only too: [`EffectControls`]). The frame
//!   graph's report and GPU timings are `sapphire_render::graph`'s; the overlay turns them on
//!   while it shows them, and keeps GPU timing on while the app reads it too
//!   ([`DebugOverlay::configure_graph`]). Quality tiers are `sapphire_render::quality`'s.
//! - Text input on the window: the overlay says when it wants it ([`OverlayOutput`]); the app
//!   owns the window.
//!
//! # Driving it
//!
//! ```ignore
//! let mut overlay = DebugOverlay::new(&gpu, surface_format, OverlayConfig::default());
//! cvars.register_all(&sapphire_debug::CVARS)?;
//!
//! // Every platform event: the overlay first, the game gets the rest.
//! overlay.handle_event(event, &mut |event| input.handle_event(event));
//!
//! // Every frame:
//! overlay.poll_gestures(platform.now(), &mut |event| input.handle_event(event));
//! overlay.record_frame(frame_time, Some(&stages));
//! for _ in 0..steps {
//!     overlay.time_controls().before_fixed_step(&mut world_clock);
//!     overlay.tod_controls().apply(&mut time_of_day);
//!     overlay.effect_controls().apply(&mut effect_stack, &effect_library);
//!     simulation.step();
//! }
//! // GPU timing stays on while dynamic resolution reads it, shown or not.
//! overlay.configure_graph(&mut executor, &gpu, quality.wants_gpu_timing());
//! let output = overlay.update(&OverlayFrame { .. });
//! renderer.record(&mut graph, target);
//! overlay.record(&mut graph, target);
//! executor.execute(&gpu, graph)?;
//! ```
//!
//! Hidden, the overlay costs a few stores per frame (the frame-time ring, and the window's
//! metrics, which the two-finger toggle measures with) and a comparison per fixed step; it
//! builds no UI, adds no pass, allocates nothing and turns the graph's report and timings off.
//! Shown, it costs about what egui does: a few hundred allocations and some tens of
//! microseconds per frame, which the stats panel reports separately ("of which overlay").
//!
//! # Toggling
//!
//! The `` ` `` key (the one below Esc, by position), a two-finger hold of one second on touch
//! screens, or the bar's Hide button. Both gestures belong to neither the overlay nor the game.
//!
//! # Later milestones
//!
//! - The rest of spec §5.20: the debug-draw API (lines, shapes and text from any thread, merged
//!   at extract into the packet's `DebugDrawList`, with `sapphire_core` no-ops when no overlay is
//!   linked), the entity inspector over `sapphire_reflect`, hitbox and hurtbox views and the
//!   input-intent buffer (M2, with combat), and the replay recorder.
//! - Clipboard copy and paste once the platform has a clipboard API; IME composition previews;
//!   cursor shapes; gamepad navigation of the panels; saving panel layout in `user://`.
//! - On-screen controls (M3) will claim their fingers before the overlay's two-finger hold
//!   counts them.

mod budget;
mod console;
mod effects;
mod history;
pub mod input;
mod overlay;
mod render;
pub mod route;
mod time;
mod time_of_day;
mod ui;

use sapphire_core::cvar::{AnyCvar, Cvar};

pub use budget::{BudgetTier, CpuStage, CpuStages, budget_use, format_bytes, format_ms};
pub use effects::EffectControls;
pub use history::{FrameSummary, FrameTimes, HISTORY};
pub use input::WindowMetrics;
pub use overlay::{
    DebugOverlay, HotReloadStatus, LiveLinkStatus, OverlayConfig, OverlayCost, OverlayFrame, OverlayOutput,
    TextInputArea,
};
pub use time::{SPEEDS, TimeControls};
pub use time_of_day::TodControls;
pub use ui::Panels;

/// Log target for this crate.
pub(crate) const LOG_TARGET: &str = "sapphire::debug";

/// The overlay's zoom on top of the display's own scale.
pub static UI_SCALE: Cvar<f32> =
    Cvar::with_range("debug.ui_scale", 1.0, 0.5, 3.0, "Debug overlay zoom, on top of the display's scale.");

/// Whether the overlay times frame-graph passes on the GPU while it shows.
pub static GPU_TIMING: Cvar<bool> = Cvar::new(
    "debug.gpu_timing",
    true,
    "Time each frame-graph pass on the GPU while the debug overlay shows (timestamp queries).",
);

/// This crate's cvars, for the app's registry.
pub static CVARS: [&dyn AnyCvar; 2] = [&UI_SCALE, &GPU_TIMING];
