//! Sapphire's data formats (spec §5.9).
//!
//! | Data | Format | Module | Older schema versions |
//! |---|---|---|---|
//! | Authored: editor documents, configs, `.meta` sidecars | RON in a `(type, version, data)` envelope | [`document`] | Upgrade on load |
//! | Saves and settings | postcard after a 32-byte [`header`] | [`save`] | Upgrade on load |
//! | Cooked runtime data | rkyv 0.8 archive after the header, read in place | [`cooked`] | Rejected as stale: recook |
//!
//! Every format records the type's stable name and schema version from `sapphire_reflect`
//! ([`sapphire_reflect::stable_name`], [`sapphire_reflect::Versioned`]), so data is never
//! read as the wrong type or the wrong version of the right one.
//!
//! - **Owns:** the three formats, their envelope and header, and loading documents by type
//!   name through a type registry ([`document::ReflectDocument`]).
//! - **Does not own:** files, paths, packs and memory maps (`sapphire_assets`, M1 A4), or
//!   save slots and their storage (`sapphire_save`, M3).
//!
//! Never `bincode`: 3.0.0 is a deliberate `compile_error!` and the project is abandoned
//! (spec §5.9).
//!
//! A crate that derives rkyv's `Archive` lists `rkyv.workspace = true`, as bevy derives
//! need: the derives find the crate by that name.

pub mod cooked;
pub mod document;
pub mod header;
pub mod save;

pub use cooked::{CookedError, access, access_unchecked, to_cooked};
pub use document::{DocumentError, ReflectDocument, from_ron, from_ron_dynamic, to_ron, to_ron_dynamic};
pub use header::HeaderError;
pub use save::{SaveError, from_save, to_save};
pub use {postcard, rkyv, ron};
