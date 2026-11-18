//! The inspector: what the timeline has selected, its ticks and its values, each a number or a field.

use super::*;

impl Editor {
    pub(super) fn inspector(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let Some(entry) = self.selected_entry else {
            self.move_inspector(ui);
            return;
        };
        ui.label(RichText::new(timeline::title(entry.track)).strong());
        let Some(document) = self.document() else { return };
        let Some(mv) = document.moves.get(entry.mv) else { return };
        let Some(span) = entry.track.spans(mv).get(entry.index).copied() else { return };
        let (from, to, is_instant) = (span.from, span.to, entry.track.is_instant());
        let editable = self.moves_document().is_some_and(|file| file.failure().is_none());
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("entry ticks").num_columns(2).show(ui, |ui| {
                if is_instant {
                    let mut at = from;
                    ui.label("at");
                    let response = ui.add(egui::DragValue::new(&mut at).speed(0.25));
                    self.timeline_number_gesture(&response, "move an instant");
                    if response.changed() {
                        self.act(Command::SetInstant { entry, at });
                    }
                    ui.end_row();
                } else {
                    let (mut new_from, mut new_to) = (from, to);
                    ui.label("from");
                    let response = ui.add(egui::DragValue::new(&mut new_from).speed(0.25));
                    self.timeline_number_gesture(&response, "edit a window");
                    let from_changed = response.changed();
                    ui.end_row();
                    ui.label("to");
                    let response = ui.add(egui::DragValue::new(&mut new_to).speed(0.25));
                    self.timeline_number_gesture(&response, "edit a window");
                    if from_changed || response.changed() {
                        self.act(Command::SetWindow { entry, from: new_from, to: new_to });
                    }
                    ui.end_row();
                }
            });
            let values = self.moves_document().and_then(|file| edits::entry_values(file, entry).ok());
            if let Some(values) = values {
                egui::Grid::new("entry values").num_columns(2).show(ui, |ui| {
                    for value in values {
                        self.value_row(ui, entry, &value);
                    }
                });
            }
            ui.add_space(4.0);
            if ui.button("Delete").on_hover_text("Delete or Backspace").clicked() {
                self.act(Command::Delete);
            }
        });
    }

    pub(super) fn timeline_number_gesture(&mut self, response: &egui::Response, label: &str) {
        if let Some(id) = self.moves {
            self.number_gesture(response, label, id);
        }
    }

    /// A number's drag, or its typing, is one step of undo on its document.
    pub(super) fn number_gesture(&mut self, response: &egui::Response, label: &str, id: AssetId) {
        if response.drag_started() {
            self.begin_number_gesture(id, label);
        }
        if response.drag_stopped() && self.documents.any(id).is_some_and(|file| file.in_gesture()) {
            self.act(Command::End);
        }
        let held = response.ctx.input(|input| input.pointer.any_down());
        let idle = self.documents.any(id).is_some_and(|file| !file.in_gesture());
        if response.has_focus() && !held && idle {
            self.begin_number_gesture(id, label);
            self.typing_number = Some((response.id, id));
        }
    }

    /// Starts a gesture on `id`, which becomes the current document first.
    fn begin_number_gesture(&mut self, id: AssetId, label: &str) {
        self.make_current(id);
        self.act(Command::Begin(label.to_owned()));
    }

    /// What [`Command::Begin`] does: what's being typed goes in first, as a step of its own.
    pub(super) fn begin_gesture_on(&mut self, id: AssetId, label: &str) {
        self.take_typed();
        self.typing_number = None;
        self.go_to(id);
        // The set's gesture, which the document joins now: another document an edit reaches joins it too.
        self.documents.begin_gesture(label);
        self.documents.edit(id);
    }

    pub(super) fn value_row(&mut self, ui: &mut Ui, entry: EntryRef, value: &EntryValue) {
        ui.label(&value.path);
        let label = format!("edit {}", value.path);
        let number = self
            .document()
            .and_then(|document| sapphire_document::walk::get(document, &value.property).ok())
            .and_then(properties::number_kind);
        if let Some(kind) = number
            && let Some((response, literal)) = properties::number_widget(ui, &value.literal, kind)
        {
            self.timeline_number_gesture(&response, &label);
            if let Some(literal) = literal {
                self.act(Command::SetValue { entry, path: value.path.clone(), value: literal });
            }
        } else {
            // A name or a bare word, as the file writes it: see `Typed`.
            let field = egui::Id::unique(("inspector value", entry, &value.path));
            let at = TypedAt::Entry { entry, path: value.path.clone() };
            self.text_field(ui, field, at, &value.literal);
        }
        ui.end_row();
    }

    /// Takes what a field holds now. Typing in another field means the last one let go, and typing back to
    /// what the file says leaves nothing to take.
    pub(super) fn keep_typing(&mut self, typed: Typed, literal: &str) {
        let elsewhere = self.typed.as_ref().is_some_and(|other| other.field != typed.field);
        if !elsewhere || self.take_typed() {
            // What's typed belongs to its document, so that's the one a save or an undo means from here on.
            match &typed.at {
                TypedAt::Entry { .. } => {
                    if let Some(id) = self.moves {
                        self.make_current(id);
                    }
                }
                TypedAt::Property { document, .. } => self.make_current(*document),
                TypedAt::Shared { .. } => {}
            }
            self.typed = (typed.text != literal).then_some(typed);
        }
    }

    pub(super) fn move_inspector(&mut self, ui: &mut Ui) {
        let Some(document) = self.document() else { return };
        let Some(mv) = document.moves.get(self.selected_move) else { return };
        let (name, startup, active, recovery, ticks) =
            (mv.name.clone(), mv.startup, mv.active, mv.recovery, mv.ticks());
        let has_clip = mv.motion.clip.is_some();
        ui.label(RichText::new(name).strong());
        ui.label(format!("{ticks} ticks, {:.0} ms", f64::from(ticks) * 1000.0 / 60.0));
        ui.label(format!("startup {startup}, active {active}, recovery {recovery}"));
        if has_clip {
            ui.label(
                RichText::new(
                    "Plays a clip tick for tick, so its length is the clip's. Windows move freely.",
                )
                .small()
                .color(Color32::from_gray(150)),
            );
        } else {
            ui.label(RichText::new("Keyed by hand.").small().color(Color32::from_gray(150)));
        }
        ui.label(
            RichText::new("Select something on the timeline to edit it.")
                .small()
                .color(Color32::from_gray(150)),
        );
    }
}
