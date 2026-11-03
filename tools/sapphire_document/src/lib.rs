//! What an editor edits (spec §5.28): typed documents whose every edit is a change record, kept as RON text
//! in which untouched bytes never move.
//!
//! It owns the document model and nothing of how it's shown: there's no egui here. A [`Document`] holds a
//! file's text and what that text says, as a value of the type with `Reflect` that the document's own
//! parser makes of it. An edit names a property by a [`PropertyPath`] and gives it a value; [`RonPatch`]
//! finds the fewest bytes that say so, the document's parser reads the result, and the edit lands only if
//! that equals the value it was to make. So comments and layout survive, and a mistake here refuses an edit
//! rather than corrupting a file. Undo gives back the bytes a [`ChangeRecord`] took.
//!
//! A [`Document`] knows nothing of files: it's told what they now hold. A [`DocumentSet`] does the looking,
//! for the documents open at once. It knows each by the ID in its `.meta`, the one a cook gives it, which
//! survives a rename and a move ([`identity`]), and it says which are dirty: exactly those whose bytes differ
//! from the last save or take.
//!
//! It owns no rules. What a document may say is its parser's and its `check`'s ([`Schema`]).

pub mod blank;
pub mod disk;
pub mod document;
pub mod identity;
pub mod kinds;
pub mod layout;
pub mod patch;
pub mod path;
pub mod record;
pub mod set;
pub mod text;
pub mod walk;
pub mod write;

pub use document::{Document, Outcome, ReadFailure, Rebased, Refused, RefusedRecord};
pub use identity::IdentityError;
pub use kinds::{Schema, registry_of};
pub use patch::RonPatch;
pub use path::{PropertyPath, Step};
pub use record::{Change, ChangeRecord, HistoryStep, ObjectId, Op};
pub use set::{AnyDocument, DocumentSet, DocumentStats, Noticed, SetError};
pub use walk::{Property, PropertyKind};
