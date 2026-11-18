//! Steps of several documents at once (M6 T2): what one edit of a selection across documents, or a placement
//! moved with the colliders it names, comes to.
//!
//! Each document keeps its own history, so each still says what's unsaved of its own file and its undo works
//! after a save of it alone. A [`SetStep`] is laid over them: the step it made in each document it touched, by
//! the [`StepId`] that step has there. [`DocumentSet::undo`] of a document whose next undo is a part of one
//! undoes every part, and only if each is its document's next undo; otherwise it's refused, naming the document
//! with edits of its own since, as its own undo goes first. [`DocumentSet::redo`] is the same the other way. A
//! step of one document alone is that document's, as it always was. Godot 4's editor undo has this shape: a
//! history per scene and one for what spans them.
//!
//! A set step is made by a gesture of the set's ([`DocumentSet::begin_gesture`] to [`DocumentSet::end_gesture`],
//! or [`DocumentSet::together`]): each document edited through [`DocumentSet::edit`] joins it with a gesture of
//! its own under the set's label, and at its end every document that changed has one step, which are one set
//! step if there are several.
//!
//! What it can't keep: a part a document no longer holds where undo (or redo) would find it, because its
//! document closed, the cap or a new edit let it go, a rebase laid it back as a step of its own, or that part
//! was undone or redone alone ([`DocumentSet::any_mut`]). A *bound* set step, whose parts hold each other to
//! what a cook checks (a placement and the colliders it names, [`crate::edit_with_colliders`]), is then refused
//! both ways ([`HistoryError::Gone`]): half of it would split what the cook refuses apart. Any other falls
//! apart: this document's part goes alone, as its own step, and [`Took::Apart`] names the documents left out,
//! for the editor to say so. A journal keeps each document's steps, not the set's, so a set step taken back
//! from one is its documents' steps, each undone alone.

use std::path::PathBuf;

use sapphire_assets::AssetId;

use super::DocumentSet;
use crate::document::StepId;
use crate::set::AnyDocument;

/// One step of undo across several documents (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetStep {
    pub label: String,
    /// Each document it touched, with the ID of its step there, in ID order.
    pub parts: Vec<(AssetId, StepId)>,
    /// Whether its parts can't go apart: then a part gone refuses it, where another falls apart.
    pub bound: bool,
}

/// What an undo or a redo of the set's did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Took {
    /// There was nothing to take.
    Nothing,
    /// A step of the document's own, or a set step whole.
    Step,
    /// A set step this document's part of went alone, since the rest of it (the documents in `left`) was gone
    /// from where it would be found: what the editor says it left out.
    Apart { label: String, left: Vec<PathBuf> },
}

impl Took {
    /// Whether any text moved.
    #[must_use]
    pub const fn moved(&self) -> bool {
        !matches!(self, Self::Nothing)
    }
}

/// A gesture of the set's, under way.
#[derive(Clone, Debug)]
pub(super) struct SetGesture {
    label: String,
    /// The documents it has edited, in the order they joined, each with its next undo before it did.
    joined: Vec<(AssetId, Option<StepId>)>,
    /// Whether what it makes is a bound set step.
    bound: bool,
}

/// Why a set step wasn't undone or redone. Nothing changed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HistoryError {
    /// Another document it touched has a step of its own to take first.
    #[error("`{label}` changed {} too, which has {} of its own to take first", path.display(), if *redo { "a redo" } else { "edits" })]
    Later { label: String, path: PathBuf, redo: bool },
    /// What a bound step did to another document is gone from where that document's history would find it, so
    /// it can't be taken whole, and half of it would split what a cook holds together.
    #[error(
        "`{label}` can't be {}: what it did to {} is gone from that document's history, and half of it would \
         leave the two apart",
        if *redo { "redone" } else { "undone" },
        path.display()
    )]
    Gone { label: String, path: PathBuf, redo: bool },
}

impl DocumentSet {
    /// Starts a gesture of the set's: every document edited through [`Self::edit`] until [`Self::end_gesture`]
    /// has one step, under `label`, and several are one [`SetStep`]. One under way ends first, as does each
    /// document's own.
    pub fn begin_gesture(&mut self, label: &str) {
        self.end_gesture();
        self.gesture = Some(SetGesture { label: label.to_owned(), joined: Vec::new(), bound: false });
    }

    /// Whether the set has a gesture under way.
    #[must_use]
    pub const fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }

    /// A document to edit, which joins the set's gesture if one is under way: how an edit of several documents
    /// is one step. Edits through [`Self::any_mut`] are steps of their document alone.
    pub fn edit(&mut self, id: AssetId) -> Option<&mut (dyn AnyDocument + 'static)> {
        let file = self.files.get_mut(&id)?;
        if let Some(gesture) = &mut self.gesture
            && !gesture.joined.iter().any(|(joined, _)| *joined == id)
        {
            file.document.end_gesture();
            gesture.joined.push((id, file.document.undo_id()));
            file.document.begin_gesture(&gesture.label);
        }
        Some(file.document.as_mut())
    }

    /// Makes the set step the gesture under way will make a bound one ([`SetStep::bound`]).
    pub(crate) fn bind_gesture(&mut self) {
        if let Some(gesture) = &mut self.gesture {
            gesture.bound = true;
        }
    }

    /// Ends the set's gesture, and every document's own: each document it changed has one step, and several are
    /// one [`SetStep`].
    pub fn end_gesture(&mut self) {
        let gesture = self.gesture.take();
        for file in self.files.values_mut() {
            file.document.end_gesture();
        }
        let Some(SetGesture { label, joined, bound }) = gesture else { return };
        let mut parts: Vec<(AssetId, StepId)> = joined
            .into_iter()
            .filter_map(|(id, before)| {
                let after = self.files.get(&id)?.document.undo_id()?;
                (Some(after) != before).then_some((id, after))
            })
            .collect();
        if parts.len() > 1 {
            parts.sort_unstable();
            self.prune_steps();
            self.steps.push(SetStep { label, parts, bound });
        }
    }

    /// Lets go of the set steps no document open holds a part of any more.
    pub(super) fn prune_steps(&mut self) {
        let files = &self.files;
        self.steps.retain(|step| {
            step.parts.iter().any(|(id, part)| {
                files
                    .get(id)
                    .is_some_and(|file| file.document.holds(*part, false) || file.document.holds(*part, true))
            })
        });
    }

    /// Takes back the set's gesture, and every document's own, to where each began.
    pub fn cancel_gesture(&mut self) -> bool {
        self.gesture = None;
        let mut taken = false;
        for file in self.files.values_mut() {
            if file.document.in_gesture() {
                let text = file.document.text().to_owned();
                file.document.cancel_gesture();
                taken |= text != file.document.text();
            }
        }
        taken
    }

    /// Runs `edit` as one edit of every document it edits through [`Self::edit`]: all of it lands, or none of it
    /// does and every text is as it was. It's a step of its own, under `label`, or part of the set's gesture if
    /// one is under way.
    pub fn together<R, E>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut Self) -> Result<R, E>,
    ) -> Result<R, E> {
        let Some(gesture) = &self.gesture else {
            self.begin_gesture(label);
            let result = edit(self);
            match result {
                Ok(_) => self.end_gesture(),
                Err(_) => {
                    self.cancel_gesture();
                }
            }
            return result;
        };
        let marks: Vec<_> = gesture
            .joined
            .iter()
            .filter_map(|(id, _)| Some((*id, self.files.get(id)?.document.mark())))
            .collect();
        let result = edit(self);
        if result.is_err() {
            let joined = self.gesture.as_mut().map(|gesture| std::mem::take(&mut gesture.joined));
            let mut kept = Vec::new();
            for (id, before) in joined.into_iter().flatten() {
                let Some(file) = self.files.get_mut(&id) else { continue };
                match marks.iter().position(|(marked, _)| *marked == id) {
                    Some(at) => {
                        file.document.roll_back(marks[at].1.clone());
                        kept.push((id, before));
                    }
                    // It joined for this edit: back to where it joined, and out of the gesture.
                    None => file.document.cancel_gesture(),
                }
            }
            if let Some(gesture) = &mut self.gesture {
                gesture.joined = kept;
            }
        }
        result
    }

    /// The set step `id`'s next undo (or with `redo`, its next redo) is a part of, if it's one.
    #[must_use]
    pub fn set_step(&self, id: AssetId, redo: bool) -> Option<&SetStep> {
        let document = &self.files.get(&id)?.document;
        let next = if redo { document.redo_id() } else { document.undo_id() }?;
        self.steps.iter().rev().find(|step| step.parts.contains(&(id, next)))
    }

    /// Takes back `id`'s last step, or the set step it's a part of: whole, or apart if it can't be and isn't
    /// bound (see the [module docs](self)). A gesture under way ends first.
    pub fn undo(&mut self, id: AssetId) -> Result<Took, HistoryError> {
        self.take(id, false)
    }

    /// Does again `id`'s last undone step, or the set step it's a part of, as [`Self::undo`] takes one back.
    pub fn redo(&mut self, id: AssetId) -> Result<Took, HistoryError> {
        self.take(id, true)
    }

    fn take(&mut self, id: AssetId, redo: bool) -> Result<Took, HistoryError> {
        self.end_gesture();
        if !self.files.contains_key(&id) {
            return Ok(Took::Nothing);
        }
        let Some(step) = self.set_step(id, redo).cloned() else {
            return Ok(if self.own(id, redo) { Took::Step } else { Took::Nothing });
        };
        // A part not where this way finds it: gone from its document, or taken that way on its own already.
        let missing: Vec<AssetId> = step
            .parts
            .iter()
            .filter(|(other, part)| !self.holds(*other, *part, redo))
            .map(|(other, _)| *other)
            .collect();
        if let Some(first) = missing.first() {
            if step.bound {
                return Err(HistoryError::Gone { label: step.label, path: self.path_of(*first), redo });
            }
            self.steps.retain(|kept| *kept != step);
            self.own(id, redo);
            let left = missing.iter().map(|other| self.path_of(*other)).collect();
            return Ok(Took::Apart { label: step.label, left });
        }
        for (other, part) in &step.parts {
            let document = &self.files[other].document;
            let next = if redo { document.redo_id() } else { document.undo_id() };
            if next != Some(*part) {
                let path = self.files[other].path.clone();
                return Err(HistoryError::Later { label: step.label.clone(), path, redo });
            }
        }
        for (other, _) in &step.parts {
            self.own(*other, redo);
        }
        Ok(Took::Step)
    }

    /// Undoes or redoes a step of `id`'s own.
    fn own(&mut self, id: AssetId, redo: bool) -> bool {
        self.files
            .get_mut(&id)
            .is_some_and(|file| if redo { file.document.redo() } else { file.document.undo() })
    }

    /// Whether document `id` is open and holds step `part` in its undo, or with `redo` in its redo.
    fn holds(&self, id: AssetId, part: StepId, redo: bool) -> bool {
        self.files.get(&id).is_some_and(|file| file.document.holds(part, redo))
    }

    /// Where document `id` is, open or not, for a person to read.
    fn path_of(&self, id: AssetId) -> PathBuf {
        self.files
            .get(&id)
            .map(|file| file.path.clone())
            .or_else(|| self.sidecars.source_of(id))
            .unwrap_or_else(|| PathBuf::from(id.to_string()))
    }

    /// The other documents open that a set step joins to `id`, whose edits a save of `id` alone would leave
    /// apart from its own: what an editor saves with it.
    #[must_use]
    pub fn joined(&self, id: AssetId) -> Vec<AssetId> {
        let mut joined: Vec<AssetId> = self
            .steps
            .iter()
            .filter(|step| step.parts.iter().any(|(part_of, _)| *part_of == id))
            .flat_map(|step| step.parts.iter().map(|(other, _)| *other))
            .filter(|other| *other != id && self.files.contains_key(other))
            .collect();
        joined.sort_unstable();
        joined.dedup();
        joined
    }

    /// The set steps a document open holds a part of, oldest first.
    #[must_use]
    pub fn set_steps(&self) -> &[SetStep] {
        &self.steps
    }
}
