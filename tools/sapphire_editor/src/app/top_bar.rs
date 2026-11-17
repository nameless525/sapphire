//! The top bar: the current document's name, save, undo and redo, the panes, the device's address, and the
//! questions a quit, a close, a conflict or a refused value asks.

use super::*;

impl Editor {
    pub(super) fn top_bar(&mut self, ui: &mut Ui) {
        ui.add_space(3.0);
        ui.horizontal_wrapped(|ui| {
            let name = self.current().map_or_else(|| "no file".to_owned(), |file| file.file_name());
            let dirty = self.typed.is_some() || self.current().is_some_and(|file| file.is_dirty());
            let title = if dirty { format!("{name} *") } else { name };
            ui.label(RichText::new(title).strong());

            let has_file = self.current.is_some();
            if ui
                .add_enabled(has_file, egui::Button::new("Save and push"))
                .on_hover_text("Cmd+S, or Ctrl+S")
                .clicked()
            {
                self.act(Command::SaveAndPush);
            }
            let undo = self.current().and_then(|file| file.undo_label()).map(ToOwned::to_owned);
            // History's floor is worth a click: it says why there's nothing there.
            let floor = self.current().and_then(|file| file.document().why_no_undo());
            let hover = undo.clone().or_else(|| floor.clone()).unwrap_or_default();
            if ui
                .add_enabled(undo.is_some() || floor.is_some(), egui::Button::new("Undo"))
                .on_hover_text(hover)
                .clicked()
            {
                self.act(Command::Undo);
            }
            let redo = self.current().and_then(|file| file.redo_label()).map(ToOwned::to_owned);
            if ui
                .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                .on_hover_text(redo.unwrap_or_default())
                .clicked()
            {
                self.act(Command::Redo);
            }
            if ui.add_enabled(has_file, egui::Button::new("Close file")).clicked() {
                self.act(Command::Close);
            }
            ui.menu_button("Panes", |ui| {
                for pane in Pane::ALL {
                    let mut open = self.shell.is_open(pane);
                    if ui.checkbox(&mut open, pane.title()).changed() {
                        self.act(if open { Command::OpenPane(pane) } else { Command::ClosePane(pane) });
                    }
                }
            });
            ui.separator();
            let device = self.link.device();
            let (dot, words) = match device {
                Some(status) => {
                    (Color32::from_rgb(96, 214, 110), format!("{} ({})", status.app, status.platform))
                }
                None => (Color32::from_gray(110), "no device".to_owned()),
            };
            let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot_rect.center(), 4.5, dot);
            let address = ui.add(TextEdit::singleline(&mut self.address_text).desired_width(130.0));
            // An address unchanged is no change, however often the field lets go.
            if address.lost_focus() && self.address_text.parse().ok() != self.link.address() {
                self.act(Command::Link(self.address_text.clone()));
            }
            ui.label(words);
        });
        if self.quit == Quit::Asking {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), "You have edits that aren't saved.");
                if ui.button("Save and quit").clicked() {
                    self.act(Command::SaveAndQuit);
                }
                if ui.button("Quit without saving").clicked() {
                    self.act(Command::QuitWithoutSaving);
                }
                if ui.button("Keep editing").clicked() {
                    self.act(Command::CancelQuit);
                }
            });
        }
        if self.closing.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), "You have edits that aren't saved.");
                if ui.button("Save and close").clicked() {
                    self.act(Command::SaveAndClose);
                }
                if ui.button("Close without saving").clicked() {
                    self.act(Command::CloseWithoutSaving);
                }
                if ui.button("Keep editing").clicked() {
                    self.act(Command::CancelClose);
                }
            });
        }
        let conflict = self.current().and_then(|file| file.conflict()).map(|conflict| match conflict {
            Conflict::Text(_) => ("The file changed on disk, and you have edits here.", true),
            Conflict::NotText => {
                ("The file on disk isn't text any more. Keeping yours replaces it at the next save.", false)
            }
        });
        if let Some((said, can_take)) = conflict {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), said);
                if can_take && ui.button("Take the file's").clicked() {
                    self.act(Command::TakeFile);
                }
                if can_take
                    && ui
                        .button("Mine on top of the file's")
                        .on_hover_text("Each edit that still fits goes back")
                        .clicked()
                {
                    self.act(Command::MineOnTop);
                }
                if ui.button("Keep mine").clicked() {
                    self.act(Command::KeepMine);
                }
            });
        }
        if let Some(typed) = &self.typed
            && let Some(why) = &typed.refused
        {
            let entry = match &typed.at {
                TypedAt::Entry { entry, .. } => Some(*entry),
                TypedAt::Property { .. } => None,
            };
            let said = match entry {
                Some(entry) => format!(
                    "`{}` can't go in {} {}'s {}, and nothing else can until it's fixed or dropped: {why}",
                    typed.text,
                    timeline::title(entry.track),
                    entry.index,
                    typed.at.label(),
                ),
                None => format!(
                    "`{}` can't go in {}, and nothing else can until it's fixed or dropped: {why}",
                    typed.text,
                    typed.at.label(),
                ),
            };
            ui.horizontal_wrapped(|ui| {
                if entry.is_some() && ui.button("Show it").clicked() {
                    self.act(Command::ShowTyped);
                }
                if ui.button("Drop it").clicked() {
                    self.act(Command::DropTyped);
                }
                ui.colored_label(Color32::from_rgb(255, 120, 110), said);
            });
        }
        if let Some(message) = self.current().and_then(|file| file.document().failure_message()) {
            ui.colored_label(
                Color32::from_rgb(255, 120, 110),
                format!("This can't be edited until the file reads again. {message}"),
            );
        }
        if let Some(line) = self.current().and_then(|file| generated_by(file.text())) {
            ui.colored_label(Color32::from_rgb(235, 190, 110), line);
        }
        if let Some(notice) = &self.notice {
            let color =
                if notice.trouble { Color32::from_rgb(255, 120, 110) } else { Color32::from_gray(190) };
            ui.colored_label(color, &notice.text);
        }
        ui.add_space(2.0);
    }

    /// What taking a file's text leaves to do.
    pub(super) fn after_take(&mut self) {
        self.link.clear_push();
        self.refresh();
        self.check_selection();
    }
}

/// What a rebase came to, in a notice's words.
pub(super) fn rebase_notice(rebased: &Rebased) -> Notice {
    if rebased.beneath {
        return Notice {
            text: "took the file's text: history can't say which edits weren't saved, so they're in undo"
                .to_owned(),
            trouble: true,
        };
    }
    match rebased.refused.first() {
        None => Notice {
            text: format!("took the file's text, and {} edits went back on top", rebased.kept),
            trouble: false,
        },
        Some(first) => Notice {
            text: format!(
                "took the file's text: {} edits went back on top and {} didn't, the first of them `{}`: {}",
                rebased.kept,
                rebased.refused.len(),
                first.label,
                first.why,
            ),
            trouble: true,
        },
    }
}

/// What a file says of whatever else writes it, if anything does: the sentence near its top with
/// "Written by" in it, so the file's own words decide what an edit here is worth. Only comments before
/// the document count, so a word in a move's comment can't set it off.
pub(super) fn generated_by(text: &str) -> Option<String> {
    let header = text.lines().take_while(|line| line.trim_start().starts_with("//"));
    let mentions = header
        .map(|line| line.trim_start().trim_start_matches('/').trim())
        .find(|line| line.contains("Written by"))?;
    let at = mentions.find("Written by")?;
    let said = &mentions[at..];
    // A path's dots have no space after them.
    let end = said.find(". ").map_or(said.len(), |end| end + 1);
    Some(said[..end].trim().to_owned())
}
