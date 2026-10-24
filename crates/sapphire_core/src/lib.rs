//! Foundation types every Sapphire crate may use (spec §5.3).
//!
//! Owns:
//! - [`log`]: the process-wide `tracing` subscriber, plus the [`log::LogSink`] hook that the
//!   platform layer uses to forward records to `SDL_Log` on mobile.
//! - [`name`]: [`Name`], a const-constructible 64-bit string ID.
//! - [`guid`]: [`Guid`], 128-bit asset and document identity.
//! - [`alloc`]: [`alloc::CountingAllocator`] and per-thread allocation scopes for rule R6.
//! - [`target`]: [`target::FormFactor`], the one definition of desktop versus mobile.
//!
//! Deliberately not here: anything that touches the OS beyond `std` (that is
//! `sapphire_platform`), clocks (`sapphire_time`) and threads (`sapphire_jobs`). Frame arenas,
//! memory tags, typed handles and cvars (spec §5.3) arrive in M1 with their first consumers.

pub mod alloc;
pub mod guid;
pub mod log;
pub mod name;
pub mod target;

pub use guid::Guid;
pub use name::Name;
