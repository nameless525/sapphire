//! The selection (M6 T2): objects by document and path, in as many documents as are open, and the fields they
//! share, which the Properties pane edits all at once.
//!
//! An object is an [`ObjectId`]: a document, by the ID its sidecar gives it, and a path into it, or the root for
//! the whole document. A path names each entry of a list by its key where it has one (M6 T8a): a zone's placement
//! by its ID (`placements["3fa91c07"]`), a spawner by its name, so what's selected stays selected when an edit, an
//! undo or a merge from disk moves it in its list. A path picked by its place (`placements[2]`) is held by its key. One edit of a shared field ([`Command::SetShared`]) sets it under each
//! object, as one step of undo however many documents it touches: a set step over each one's own
//! ([`sapphire_document::SetStep`]). A zone's edits go through [`edit_with_colliders`], so a placement moved by
//! one takes the colliders it names with it.
//!
//! A field is shared when every object has a value there; it shows its value when they all spell it alike, and
//! nothing when they don't. An object whose path stops reading after an edit or an undo leaves the selection.

use egui::{Color32, RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_document::{
    DocumentSet, ObjectId, PropertyKind, PropertyPath, Step, edit_with_colliders, walk, write,
};
use sapphire_zone::{IdList, ZoneDocument};

use super::{Editor, TypedAt};
use crate::commands::{Command, Picked};
use crate::edits::EditError;

/// A field every selected object has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SharedField {
    /// Under each object.
    pub path: PropertyPath,
    /// As every object spells it, or `None` where they differ.
    pub literal: Option<String>,
}

/// `path` under `object`.
fn under(object: &PropertyPath, path: &PropertyPath) -> PropertyPath {
    path.steps().iter().fold(object.clone(), |joined, step| joined.join(step.clone()))
}

/// The leaves under `object`, by their paths from it, as the file spells each or as its default would be.
fn leaves(documents: &DocumentSet, object: &ObjectId) -> Option<Vec<(PropertyPath, String)>> {
    let document = documents.any(object.document)?;
    let value = document.value_reflect()?;
    let properties = document.properties_under(&object.path).ok()?;
    let depth = object.path.steps().len();
    Some(
        properties
            .into_iter()
            .filter(|property| property.kind == PropertyKind::Leaf && property.path.steps().len() > depth)
            .filter_map(|property| {
                let literal = match &property.written {
                    Some(span) => document.text()[span.clone()].to_owned(),
                    None => {
                        write::canonical(walk::get(value, &property.path).ok()?, document.registry()).ok()?
                    }
                };
                let relative = PropertyPath::root();
                let relative = property.path.steps()[depth..]
                    .iter()
                    .fold(relative, |path, step| path.join(step.clone()));
                Some((relative, literal))
            })
            .collect(),
    )
}

/// The fields every object in `selection` has, in the first object's order.
pub(super) fn shared_fields(documents: &DocumentSet, selection: &[ObjectId]) -> Vec<SharedField> {
    let Some((first, rest)) = selection.split_first() else { return Vec::new() };
    let Some(first) = leaves(documents, first) else { return Vec::new() };
    let mut fields: Vec<SharedField> =
        first.into_iter().map(|(path, literal)| SharedField { path, literal: Some(literal) }).collect();
    for object in rest {
        let Some(theirs) = leaves(documents, object) else { return Vec::new() };
        fields.retain_mut(|field| match theirs.iter().find(|(path, _)| *path == field.path) {
            Some((_, literal)) => {
                if field.literal.as_ref() != Some(literal) {
                    field.literal = None;
                }
                true
            }
            None => false,
        });
    }
    fields
}

/// Whether `object` still reads in `documents`.
/// Where a selected entry, named by its key, stood in its list when a command began: so one that renames it
/// (sets its `id`, or its `name` where it has no `id`) or undoes a rename keeps it selected by its new key.
#[derive(Clone, Debug)]
pub(super) struct Standing {
    object: ObjectId,
    list: PropertyPath,
    index: usize,
    len: usize,
}

/// How many entries the list at `path` has.
fn list_len(value: &dyn bevy_reflect::PartialReflect, path: &PropertyPath) -> Option<usize> {
    match walk::get(value, path).ok()?.reflect_ref() {
        bevy_reflect::ReflectRef::List(list) => Some(list.len()),
        _ => None,
    }
}

fn reads(documents: &DocumentSet, object: &ObjectId) -> bool {
    documents
        .any(object.document)
        .and_then(|document| document.value_reflect())
        .is_some_and(|value| walk::get(value, &object.path).is_ok())
}

impl Editor {
    /// What's selected, in the order it was picked.
    #[must_use]
    pub fn selection(&self) -> &[ObjectId] {
        &self.selection
    }

    /// Selects `picked` in place of what was: each a document open here, by its path, and a path that reads in it.
    pub(super) fn select_objects(&mut self, picked: &[Picked]) {
        let mut selection = Vec::with_capacity(picked.len());
        for picked in picked {
            let Some(document) = self.open_at(&picked.document) else {
                self.refuse(format!("{} isn't open", picked.document.display()));
                return;
            };
            let path = match picked.path.parse::<PropertyPath>() {
                Ok(path) => path,
                Err(error) => {
                    self.refuse(format!("`{}` isn't a path: {error}", picked.path));
                    return;
                }
            };
            let keyed = self
                .documents
                .any(document)
                .and_then(|open| open.value_reflect())
                .and_then(|value| walk::keyed(value, &path).ok());
            let Some(path) = keyed else {
                self.refuse(format!("{} has nothing at `{}`", picked.document.display(), picked.path));
                return;
            };
            let object = ObjectId { document, path };
            if !selection.contains(&object) {
                selection.push(object);
            }
        }
        self.selection = selection;
    }

    /// The document open at `path`, by its name or the file it is.
    pub(super) fn open_at(&self, path: &std::path::Path) -> Option<AssetId> {
        let same = |open: &std::path::Path| {
            open == path
                || matches!((std::fs::canonicalize(open), std::fs::canonicalize(path)), (Ok(a), Ok(b)) if a == b)
        };
        self.documents.ids().find(|id| self.documents.path(*id).is_some_and(same))
    }

    /// The selection as a command names it.
    pub(super) fn picked(&self, selection: &[ObjectId]) -> Vec<Picked> {
        selection
            .iter()
            .filter_map(|object| {
                let document = self.documents.path(object.document)?.to_owned();
                Some(Picked { document, path: object.path.to_string() })
            })
            .collect()
    }

    /// Lets go of what's selected that no longer reads.
    pub(super) fn prune_selection(&mut self) {
        let documents = &self.documents;
        for object in &mut self.selection {
            if reads(documents, object) {
                continue;
            }
            // Renamed in its place: the list as long as it was, and its entry there called something else.
            let Some(standing) = self.standings.iter().find(|standing| standing.object == *object) else {
                continue;
            };
            let Some(value) = documents.any(object.document).and_then(|document| document.value_reflect())
            else {
                continue;
            };
            if list_len(value, &standing.list) == Some(standing.len)
                && let Ok(path) = walk::keyed(value, &standing.list.join(Step::Index(standing.index)))
            {
                object.path = path;
            }
        }
        self.selection.retain(|object| reads(documents, object));
    }

    /// Where each selected entry named by its key stands, before a command that may rename one.
    pub(super) fn stand_selection(&mut self) {
        let documents = &self.documents;
        self.standings = (self.selection.iter())
            .filter_map(|object| {
                let (list, Step::Key(_)) = object.path.split_last()? else { return None };
                let value = documents.any(object.document)?.value_reflect()?;
                let Some(Step::Index(index)) =
                    walk::positional(value, &object.path).ok()?.steps().last().cloned()
                else {
                    return None;
                };
                let len = list_len(value, &list)?;
                Some(Standing { object: object.clone(), list, index, len })
            })
            .collect();
    }

    /// Sets `path` under every object selected to `literal`, as one step: what [`Command::SetShared`] does.
    pub(super) fn set_shared(&mut self, path: &PropertyPath, literal: &str) -> Result<(), EditError> {
        if self.selection.is_empty() {
            return Err(EditError("nothing is selected".to_owned()));
        }
        let label = format!("edit {}", super::dispatch::last_step(path));
        let mut by_document: Vec<(AssetId, Vec<PropertyPath>)> = Vec::new();
        for object in &self.selection {
            let at = under(&object.path, path);
            match by_document.iter_mut().find(|(id, _)| *id == object.document) {
                Some((_, paths)) => paths.push(at),
                None => by_document.push((object.document, vec![at])),
            }
        }
        self.documents.together(&label, |documents| {
            for (id, paths) in &by_document {
                spell_all(documents, *id, &label, paths, literal)?;
            }
            Ok(())
        })
    }

    /// Sets one property of a document, as spelled, joining the set's gesture if one's under way: a zone's through
    /// [`edit_with_colliders`], so what it moves takes its colliders.
    pub(super) fn spell(
        &mut self,
        id: AssetId,
        label: &str,
        path: &PropertyPath,
        literal: &str,
    ) -> Result<(), EditError> {
        spell_all(&mut self.documents, id, label, std::slice::from_ref(path), literal)
    }

    /// The Properties pane's head while several objects are selected: the fields they share, each one edit of all.
    pub(super) fn selection_section(&mut self, ui: &mut Ui) {
        let fields = shared_fields(&self.documents, &self.selection);
        let documents = {
            let mut ids: Vec<AssetId> = self.selection.iter().map(|object| object.document).collect();
            ids.sort_unstable();
            ids.dedup();
            ids.len()
        };
        let what = if documents == 1 { "document" } else { "documents" };
        ui.label(RichText::new(format!("{} selected, in {documents} {what}", self.selection.len())).strong());
        if fields.is_empty() {
            ui.label(RichText::new("They share no field.").color(Color32::from_gray(150)));
            return;
        }
        egui::Grid::new("shared fields").num_columns(2).show(ui, |ui| {
            for field in fields {
                let name = field.path.to_string();
                ui.label(&name);
                let id = egui::Id::unique(("shared value", &name));
                let at = TypedAt::Shared { path: field.path.clone(), label: name };
                let literal = field.literal.unwrap_or_default();
                self.text_field(ui, id, at, &literal);
                ui.end_row();
            }
        });
    }

    /// Selects or lets go of the object at `path` of the zone `zone` beside what's selected: a Cmd or Shift click on
    /// its row.
    pub(super) fn toggle_object(&mut self, zone: AssetId, path: PropertyPath) {
        let object = ObjectId { document: zone, path };
        let mut selection = self.selection.clone();
        match selection.iter().position(|selected| *selected == object) {
            Some(at) => {
                selection.remove(at);
            }
            None => selection.push(object),
        }
        let picked = self.picked(&selection);
        self.act(Command::SelectObjects(picked));
    }
}

/// The path of a zone's placement, by its ID.
pub(super) fn placement_path(id: &str) -> PropertyPath {
    PropertyPath::root().join(Step::Field("placements".to_owned())).join(Step::Key(id.to_owned()))
}

/// Where in `value`, the zone `zone`, the placement a selected object is: by its ID, or by its place if it has
/// none that's its own.
pub(super) fn placement_of(object: &ObjectId, zone: AssetId, value: &ZoneDocument) -> Option<usize> {
    if object.document != zone {
        return None;
    }
    match object.path.steps() {
        [Step::Field(list), Step::Key(id)] if list == "placements" => match value.find_id(id)? {
            (IdList::Placements, index) => Some(index),
            _ => None,
        },
        [Step::Field(list), Step::Index(index)] if list == "placements" => Some(*index),
        _ => None,
    }
}

/// Sets each of `paths` in document `id` to `literal`, as one edit, joining the set's gesture if one's under way.
fn spell_all(
    documents: &mut DocumentSet,
    id: AssetId,
    label: &str,
    paths: &[PropertyPath],
    literal: &str,
) -> Result<(), EditError> {
    if documents.get::<ZoneDocument>(id).is_some() {
        return edit_with_colliders(documents, id, label, |zone| {
            let mut outcome = sapphire_document::Outcome::Unchanged;
            for path in paths {
                if zone.set_spelled(label, path, literal)? == sapphire_document::Outcome::Changed {
                    outcome = sapphire_document::Outcome::Changed;
                }
            }
            Ok(outcome)
        })
        .map(|_| ())
        .map_err(|error| match error {
            sapphire_document::ZoneEditError::Refused(refused) => EditError::from(refused),
            other => EditError(other.to_string()),
        });
    }
    documents.together(label, |documents| {
        let Some(document) = documents.edit(id) else {
            return Err(EditError(format!("no document with the ID {id} is open")));
        };
        for path in paths {
            document.set_spelled(label, path, literal)?;
        }
        Ok(())
    })
}
