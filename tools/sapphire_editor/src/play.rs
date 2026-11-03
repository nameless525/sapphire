//! Play-in-editor (M3 E4, exit test 6): what a run starts from.
//!
//! A [`Snapshot`] is copies: the text of every open document as it stands when someone presses Play, and what
//! the moves among them compile to. The runtime gets those, and nothing that reaches back: no path to a
//! [`DocumentSet`] or an editor exists in what [`crate::viewport`] stands, so game code can't mutate a
//! document, and an edit made during a run waits for the next one.
//!
//! Only moves reach a run so far. A graph, cue table or set of effects reaches a fight through live tuning, which
//! is in `game_combat`'s `dev`, and an editor turning that on would turn it on across every workspace build that
//! holds the editor, the gates with no `dev` among them. So those are kept here, and [`Snapshot::not_played`] says
//! which have edits a run won't show.

use std::path::{Path, PathBuf};

use sapphire_document::{DocumentSet, Kind};
use sapphire_gameplay::replay::{LiveMoves, compile_document};

use crate::cooked::Cook;

/// The documents as they stood when a run was asked for.
#[derive(Clone, Debug)]
pub struct Snapshot {
    documents: Vec<(PathBuf, String)>,
    moves: LiveMoves,
    not_played: Vec<String>,
}

impl Snapshot {
    /// Copies of every document in `documents`, with its moves compiled over what `cook` made of them, as a device
    /// takes them. A document of moves that doesn't compile refuses the run, and says which and why: a run of
    /// anything but what's on the screen would look like it was.
    pub fn take(documents: &DocumentSet, cook: Option<&Cook>) -> Result<Self, String> {
        let mut snapshot =
            Self { documents: Vec::new(), moves: LiveMoves::default(), not_played: Vec::new() };
        for id in documents.ids() {
            let (Some(document), Some(path)) = (documents.any(id), documents.path(id)) else { continue };
            let text = document.text();
            let name = file_name(path);
            if Kind::of(path) == Some(Kind::Moves) {
                let running = cook.and_then(|cook| cook.move_set_of(path).ok().flatten());
                let live =
                    compile_document(text, running.as_ref()).map_err(|error| format!("{name}: {error}"))?;
                snapshot.moves.remember(&name, live.set);
            } else if document.is_dirty() {
                snapshot.not_played.push(name);
            }
            snapshot.documents.push((path.to_owned(), text.to_owned()));
        }
        snapshot.not_played.sort_unstable();
        Ok(snapshot)
    }

    /// What the fight's moves start from.
    #[must_use]
    pub const fn moves(&self) -> &LiveMoves {
        &self.moves
    }

    /// The text of the document with this file name, as it stood.
    #[must_use]
    pub fn text_of(&self, name: &str) -> Option<&str> {
        let held = self.documents.iter().find(|(path, _)| file_name(path) == name);
        held.map(|(_, text)| text.as_str())
    }

    /// The documents with edits a run doesn't take yet, by file name.
    #[must_use]
    pub fn not_played(&self) -> &[String] {
        &self.not_played
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// A run someone's asked for, for the viewport's next frame to start.
#[derive(Clone, Debug)]
pub struct PlayRequest {
    pub snapshot: Snapshot,
    /// A script plays it, by name, rather than the keys.
    pub script: Option<String>,
}
