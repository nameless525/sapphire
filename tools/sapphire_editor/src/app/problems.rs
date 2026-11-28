//! The problems pane (M6 T6): what's wrong with the open documents and the last cook, a row each, by document and
//! field ([`sapphire_document::problem`]). A parse failure and a check's issue come from each open document, a
//! refusal from the cook; one fault two of them find is one row. The list is made again when a document opens,
//! closes or is saved, and when a cook ends, not as text is typed.
//!
//! A row's click is a command, [`Command::SelectField`]: the document and the field's path as its RON spells it,
//! so a script can go where a click goes and a test can say where that is ([`Editor::selected_field`]). It goes to
//! the document, opening it if it isn't open, and Properties marks the field's row and opens the rows above it.

use std::path::Path;

use egui::{Color32, RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_document::problem::{self, At, Problem};
use sapphire_document::{ObjectId, PropertyPath, Step, walk};

use super::Editor;
use crate::commands::{Command, Picked};

impl Editor {
    /// What's wrong now, as the pane lists it: by document, a row each.
    #[must_use]
    pub fn content_problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The command a row of the pane runs: going to its field, or to its document if it's on no field.
    #[must_use]
    pub fn problem_command(problem: &Problem) -> Command {
        match &problem.at {
            At::Field(field) => {
                Command::SelectField(Picked { document: problem.document.clone(), path: field.clone() })
            }
            At::Document(_) => Command::Open(problem.document.clone()),
        }
    }

    /// The field [`Command::SelectField`] last went to, as the document's own path to it, while it's there.
    #[must_use]
    pub fn selected_field(&self) -> Option<&ObjectId> {
        self.field.as_ref()
    }

    /// Makes the list again, from the open documents and the last cook.
    pub(super) fn refresh_problems(&mut self) {
        let mut found = Vec::new();
        let ids: Vec<AssetId> = self.documents.ids().collect();
        for id in ids {
            let (Some(document), Some(path)) = (self.documents.any(id), self.documents.path(id)) else {
                continue;
            };
            match document.failure_message() {
                Some(failure) => found.push(Problem::parse(path, document.text(), &failure)),
                None => found.extend(document.issues().iter().map(|issue| Problem::check(path, issue))),
            }
        }
        if let Some(refusal) = self.cook_refusal() {
            let open = self.refused.and_then(|id| self.documents.path(id)).map(Path::to_owned);
            let document = open
                .or_else(|| refusal.source.clone())
                .unwrap_or_else(|| self.content.clone().unwrap_or_default());
            found.push(Problem::cook(&document, refusal.field.as_deref(), &refusal.message));
        }
        self.problems = problem::deduplicated(found);
    }

    /// [`Command::SelectField`]: goes to `picked`'s document, opening it if it isn't, and to its field, and selects
    /// the entry the field is of, where it's of one (M6 T9b), so the viewport's mark, the gizmo and the zone pane's
    /// row are on it too: the entry, by its key, not a part of it such as a probe's box.
    pub(super) fn select_field(&mut self, picked: &Picked) {
        let written = match picked.path.parse::<PropertyPath>() {
            Ok(path) => path,
            Err(error) => return self.refuse(format!("`{}` isn't a path: {error}", picked.path)),
        };
        let id = match self.open_at(&picked.document) {
            Some(id) => id,
            None => {
                self.open(&picked.document);
                match self.open_at(&picked.document) {
                    Some(id) => id,
                    None => return,
                }
            }
        };
        self.go_to(id);
        self.shell.open(crate::shell::Pane::Properties);
        let Some(document) = self.documents.any(id) else { return };
        // A document that doesn't read has no value to follow the path through: its field is the text's.
        let Some(value) = document.value_reflect() else {
            self.field = Some(ObjectId { document: id, path: written });
            return;
        };
        match walk::named(value, &written) {
            Ok(path) => {
                // The top-level entry, by its key: a field of a list nested inside one is still the entry's.
                if let Some(at) = path.steps().iter().position(|step| matches!(step, Step::Key(_))) {
                    let entry = path.steps()[..=at]
                        .iter()
                        .cloned()
                        .fold(PropertyPath::root(), |entry, step| entry.join(step));
                    self.selection = vec![ObjectId { document: id, path: entry }];
                }
                self.field = Some(ObjectId { document: id, path });
                self.properties.reveal = true;
            }
            Err(error) => {
                self.field = None;
                self.refuse(format!("the document has no field `{written}`: {error}"));
            }
        }
    }

    /// Lets go of a selected field whose document closed or whose path stopped reading. One whose document doesn't
    /// read stays, on the text's path, until it does.
    pub(super) fn prune_field(&mut self) {
        let gone = self.field.as_ref().is_some_and(|field| match self.documents.any(field.document) {
            None => true,
            Some(document) => {
                document.value_reflect().is_some_and(|value| walk::get(value, &field.path).is_err())
            }
        });
        if gone {
            self.field = None;
        }
    }

    pub(super) fn problems_panel(&mut self, ui: &mut Ui) {
        if self.problems.is_empty() {
            ui.label("No problems in the open documents or the last cook.");
            return;
        }
        let mut asked = None;
        egui::ScrollArea::vertical().id_salt("problems").auto_shrink([false, false]).show(ui, |ui| {
            for (document, rows) in problem::grouped(&self.problems) {
                let name = document.file_name().map_or_else(
                    || document.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                ui.label(RichText::new(name).strong()).on_hover_text(document.display().to_string());
                for row in rows {
                    let text = row.to_string();
                    let color = match row.at {
                        At::Field(_) => Color32::from_rgb(255, 130, 115),
                        At::Document(_) => Color32::from_rgb(240, 170, 120),
                    };
                    let label = ui.selectable_label(false, RichText::new(&text).color(color));
                    if label.on_hover_text(&row.message).clicked() {
                        asked = Some(Self::problem_command(row));
                    }
                }
            }
        });
        if let Some(command) = asked {
            self.act(command);
        }
    }
}
