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
//! - **Panels**: stats and budgets (frame time, CPU stages against spec §3.2, allocations, the
//!   quality tier with the render scale and dynamic resolution's state, `RenderStats`,
//!   `GraphStats` and the transient pool, the last-used input device), the
//!   frame-time graph, the frame graph (passes in order, culled passes, transients and their
//!   pooled textures, command buffers) with per-pass GPU timings, a cvar list and dev console,
//!   time controls, and the time-of-day scrubber (`time_of_day`, M1 W1: the day strip, the
//!   resolved values and a character preview).
//! - **[`TimeControls`]**: pause, single-step and speed on the world clock's time-scale stack.
//! - **[`TodControls`]**: scrubbing the hour, pausing the cycle and setting the day's length on
//!   the world's `sapphire_world::tod::TimeOfDay`.
//! - **Its own cvars** ([`CVARS`]): `debug.ui_scale` and `debug.gpu_timing`.
//!
//! # Does not own
//!
//! - Cvars themselves: [`sapphire_core::cvar`], so runtime crates declare them without depending
//!   on this crate (the facade of spec §5.20).
//! - What it shows: the app gathers each frame's numbers into an [`OverlayFrame`], time of day
//!   included (the world's `TimeOfDay`, read-only: the panel changes it through
//!   [`TodControls`]). The frame
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
//! cvars.register_all(sapphire_debug::CVARS)?;
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
//! Hidden, the overlay costs a few stores per frame (the frame-time ring) and a comparison per
//! fixed step; it builds no UI, adds no pass and turns the graph's report and timings off.
//! Shown, it costs about what egui does: a few hundred allocations and some tens of
//! microseconds per frame, which the stats panel reports separately ("of which overlay").
//!
//! # Toggling
//!
//! The ` key (the one below Esc, by position), a two-finger hold of one second on touch screens,
//! or the bar's Hide button. Both gestures belong to neither the overlay nor the game.
//!
//! # Later milestones
//!
//! - **The renderer's debug views** (spec §5.12): wireframe and overdraw, the light-cluster
//!   heatmap (R5), material ID and ramp-row views, the outline-width view. They wait for R5's
//!   lights and A5's vertex inputs to settle in the shaders. They plug in as a choice cvar the
//!   renderer declares and reads in `prepare` (`r.debug_view`, a [`sapphire_core::cvar::CvarType`]
//!   enum), which the cvar panel already edits with a list; this crate needs no change.
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
pub use history::{FrameSummary, FrameTimes, HISTORY};
pub use input::WindowMetrics;
pub use overlay::{DebugOverlay, OverlayConfig, OverlayCost, OverlayFrame, OverlayOutput, TextInputArea};
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
