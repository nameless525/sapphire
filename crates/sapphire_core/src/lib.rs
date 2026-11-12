//! Foundation types every Sapphire crate may use (spec §5.3).
//!
//! Owns:
//! - [`log`]: the process-wide `tracing` subscriber, plus the [`log::LogSink`] hook that the
//!   platform layer uses to forward records to `SDL_Log` on mobile.
//! - [`name`]: [`Name`], a const-constructible 64-bit string ID.
//! - [`guid`]: [`Guid`], 128-bit asset and document identity.
//! - [`alloc`]: [`alloc::CountingAllocator`] and per-thread allocation scopes for rule R6.
//! - [`path`]: existing-prefix filesystem identity; callers own output overlap/publication policy.
//! - [`target`]: [`target::FormFactor`], the one definition of desktop versus mobile.
//! - [`thermal`]: [`thermal::ThermalReading`], what the OS says about heat and power, as plain data
//!   that `sapphire_platform` fills and the quality ladder reads.
//! - [`frame_waits`]: how long the frame thread waited on the display (a swapchain's acquire and
//!   present), which `sapphire_rhi` adds and the frame clock leaves out of the frame's work (M4 T7).
//! - [`cvar`]: console variables, typed statics that runtime crates declare and dev tools set by
//!   name (the facade of spec §5.20: only development builds link a console).
//! - [`debug_draw`]: lines, shapes and labels from any thread, as plain data for the frame packet;
//!   the same facade: only development tools turn it on, and until then a call is one atomic load.
//!
//! Deliberately not here: anything that touches the OS beyond `std` (that is
//! `sapphire_platform`), clocks (`sapphire_time`) and threads (`sapphire_jobs`). Frame arenas,
//! memory tags and typed handles (spec §5.3) arrive in M1 with their first consumers.

pub mod alloc;
pub mod cvar;
pub mod debug_draw;
pub mod frame_waits;
pub mod guid;
pub mod log;
pub mod name;
pub mod path;
pub mod target;
pub mod thermal;

pub use guid::Guid;
pub use name::Name;
