//! The asset browser: what's under the roots, as the set's last look found it, which a timer keeps fresh.
//!
//! Documents open with a click, each as its kind. Every other asset with a sidecar is listed too, for finding
//! one and reading its ID, since that's what a document refers to it by. Nothing here reads a file.

use std::path::PathBuf;

use egui::{Color32, RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_document::Kind;

use super::Editor;

/// What the browser keeps between passes.
#[derive(Debug, Default)]
pub(super) struct Browser {
    filter: String,
    /// The asset clicked last, whose ID is shown.
    picked: Option<(PathBuf, AssetId)>,
}

impl Editor {
    pub(super) fn browser_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Find");
            ui.add(egui::TextEdit::singleline(&mut self.browser.filter).desired_width(f32::INFINITY));
        });
        let filter = self.browser.filter.to_lowercase();
        let shows = |path: &std::path::Path| {
            filter.is_empty() || path.to_string_lossy().to_lowercase().contains(&filter)
        };
        let mut rows: Vec<(Kind, PathBuf)> = self
            .documents
            .sidecars()
            .documents()
            .filter(|(path, _)| shows(path))
            .map(|(path, kind)| (kind, path.to_owned()))
            .collect();
        // What's open is listed even if it's outside the roots, or the last look is older than it.
        for id in self.documents.ids() {
            if let Some(path) = self.documents.path(id)
                && !rows.iter().any(|(_, listed)| listed == path)
                && shows(path)
            {
                rows.push((Kind::of(path).unwrap_or(Kind::Moves), path.to_owned()));
            }
        }
        let others: Vec<(PathBuf, AssetId)> = self
            .documents
            .sidecars()
            .assets()
            .filter(|(path, _)| Kind::of(path).is_none() && shows(path))
            .collect();

        let mut open = None;
        let mut pick = None;
        let bottom = ui.spacing().interact_size.y * 2.5;
        egui::ScrollArea::vertical()
            .id_salt("browser")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - bottom).max(40.0))
            .show(ui, |ui| {
                for kind in Kind::ALL {
                    let mut listed: Vec<&PathBuf> =
                        rows.iter().filter(|(of, _)| *of == kind).map(|(_, path)| path).collect();
                    if listed.is_empty() {
                        continue;
                    }
                    listed.sort();
                    ui.label(RichText::new(kind.title()).small().strong());
                    for path in listed {
                        let (text, current) = self.browser_row(path);
                        let response =
                            ui.selectable_label(current, text).on_hover_text(path.display().to_string());
                        if response.clicked() {
                            open = Some(path.clone());
                        }
                    }
                }
                if !others.is_empty() {
                    egui::CollapsingHeader::new(format!("Other assets ({})", others.len()))
                        .id_salt("other assets")
                        .show_unindented(ui, |ui| {
                            for (path, id) in &others {
                                let name = path.file_name().map_or_else(
                                    || path.display().to_string(),
                                    |name| name.to_string_lossy().into_owned(),
                                );
                                let picked = self.browser.picked.as_ref().is_some_and(|(at, _)| at == path);
                                if ui
                                    .selectable_label(picked, name)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    pick = Some((path.clone(), *id));
                                }
                            }
                        });
                }
            });
        if let Some(pick) = pick {
            self.browser.picked = Some(pick);
        }
        if let Some((path, id)) = &self.browser.picked {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(RichText::new(format!("{name}: {id}")).small().color(Color32::from_gray(170)));
        }
        let stats = self.documents.stats();
        ui.label(
            RichText::new(format!(
                "{} open, {} unsaved, {} KiB of history",
                stats.open,
                stats.dirty,
                stats.history_bytes / 1024
            ))
            .small()
            .color(Color32::from_gray(140)),
        );
        if let Some(path) = open {
            self.open(&path);
        }
    }

    /// A row's words, with what's true of it if it's open, and whether it's the current document.
    fn browser_row(&self, path: &std::path::Path) -> (RichText, bool) {
        let name = path
            .file_name()
            .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
        let id = self.documents.ids().find(|id| self.documents.path(*id) == Some(path));
        let Some(id) = id else { return (RichText::new(name), false) };
        let conflict = self.documents.conflict(id).is_some();
        let dirty = self.documents.is_dirty(id);
        let text = match (conflict, dirty) {
            (true, _) => RichText::new(format!("{name} !")).color(Color32::from_rgb(255, 170, 90)),
            (false, true) => RichText::new(format!("{name} *")),
            (false, false) => RichText::new(name),
        };
        (text, self.current == Some(id))
    }
}
