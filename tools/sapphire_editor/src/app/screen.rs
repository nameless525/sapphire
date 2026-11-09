//! The screen pane (M3 E5): the current `.ui.ron`'s preview at an aspect and in a language, and the values its
//! paths read, typed in. The viewport draws it through the game's own screens ([`crate::viewport`]); an edit
//! anywhere else, the Properties pane's or a typed one, shows there as the document reads.

use std::collections::HashMap;

use egui::{RichText, Ui};
use game_ui::menus::MenuScreen;
use sapphire_assets::AssetId;
use sapphire_assets::ui::{UiDocument, UiModel};
use sapphire_document::DocumentSet;

use super::Editor;

use crate::screen::{Aspect, LANGUAGES, ScreenShown, StandIn, read_from, stand_ins};

/// The pane's state: how the preview shows, and each screen's model and stand-in values.
#[derive(Debug, Default)]
pub(super) struct ScreenPanel {
    aspect: Aspect,
    /// One of [`LANGUAGES`]: an index, so it's always one.
    language: usize,
    screens: HashMap<AssetId, Held>,
}

/// What a screen's paths read: its model file, and what's typed in for each.
#[derive(Debug)]
struct Held {
    /// The model file it names, as it read, or why it didn't.
    model: Result<UiModel, String>,
    values: Vec<(String, StandIn)>,
}

impl ScreenPanel {
    /// Reads the model of `current`, if it's a screen with none held yet. A screen names its model file beside it.
    pub(super) fn follow(&mut self, documents: &DocumentSet, current: Option<AssetId>) {
        let Some(id) = current else { return };
        if self.screens.contains_key(&id) {
            return;
        }
        let (Some(document), Some(path)) = (documents.get::<UiDocument>(id), documents.path(id)) else {
            return;
        };
        let model = match document.last_good().and_then(|screen| screen.model.as_deref()) {
            None => Ok(UiModel::default()),
            Some(file) => {
                let at = path.parent().map_or_else(|| file.into(), |dir| dir.join(file));
                std::fs::read_to_string(&at).map_err(|error| format!("{}: {error}", at.display())).and_then(
                    |text| UiModel::from_document(&text).map_err(|error| format!("{file}: {error}")),
                )
            }
        };
        let mut values = model.as_ref().map(stand_ins).unwrap_or_default();
        // One of the game's own screens starts where its goldens do.
        if let Some(screen) = MenuScreen::ALL.into_iter().find(|screen| screen.id() == id) {
            read_from(&mut values, &*screen.sample_model());
        }
        self.screens.insert(id, Held { model, values });
    }
}

/// What the viewport shows of `current`, if it's a screen that's read and has a model.
pub(super) fn screen_shown<'a>(
    documents: &'a DocumentSet,
    panel: &'a ScreenPanel,
    current: Option<AssetId>,
) -> Option<ScreenShown<'a>> {
    let id = current?;
    let document = documents.get::<UiDocument>(id)?.last_good()?;
    let held = panel.screens.get(&id)?;
    Some(ScreenShown {
        id,
        document,
        model: held.model.as_ref().ok()?,
        values: &held.values,
        aspect: panel.aspect,
        language: LANGUAGES[panel.language],
    })
}

impl Editor {
    /// The current document, if it's a screen.
    fn current_screen(&self) -> Option<AssetId> {
        let id = self.current?;
        self.documents.get::<UiDocument>(id).map(|_| id)
    }

    /// Shows the preview at `aspect`.
    pub fn preview_at(&mut self, aspect: Aspect) {
        self.screen.aspect = aspect;
    }

    /// Shows the preview in `language`, if it's one of [`LANGUAGES`].
    pub fn preview_in(&mut self, language: &str) {
        if let Some(index) = LANGUAGES.iter().position(|known| *known == language) {
            self.screen.language = index;
        }
    }

    /// Has `path` of the current screen read `value`. Says whether the screen has that path.
    pub fn set_stand_in(&mut self, path: &str, value: StandIn) -> bool {
        let Some(id) = self.current_screen() else { return false };
        self.screen.follow(&self.documents, Some(id));
        let Some(held) = self.screen.screens.get_mut(&id) else { return false };
        let Some((_, at)) = held.values.iter_mut().find(|(known, _)| known == path) else { return false };
        *at = value;
        true
    }

    pub(super) fn screen_panel(&mut self, ui: &mut Ui) {
        let Some(id) = self.current_screen() else {
            ui.label("Open a screen (.ui.ron) to preview it.");
            return;
        };
        self.screen.follow(&self.documents, Some(id));
        let panel = &mut self.screen;
        ui.horizontal(|ui| {
            for aspect in Aspect::ALL {
                if ui.selectable_label(panel.aspect == aspect, aspect.label()).clicked() {
                    panel.aspect = aspect;
                }
            }
            ui.separator();
            for (index, language) in LANGUAGES.iter().enumerate() {
                if ui.selectable_label(panel.language == index, *language).clicked() {
                    panel.language = index;
                }
            }
        });
        let Some(held) = panel.screens.get_mut(&id) else { return };
        if let Err(why) = &held.model {
            ui.label(RichText::new(format!("Its model doesn't read, so the preview waits: {why}")).weak());
            return;
        }
        if held.values.is_empty() {
            ui.label(RichText::new("Its paths read nothing.").weak());
            return;
        }
        ui.label(RichText::new("What its paths read, in the preview only:").weak());
        egui::ScrollArea::vertical().id_salt("screen values").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("screen values").num_columns(2).striped(true).show(ui, |ui| {
                for (path, value) in &mut held.values {
                    ui.label(path.as_str());
                    match value {
                        StandIn::Number(number) => {
                            ui.add(egui::DragValue::new(number).speed(0.1).max_decimals(3));
                        }
                        StandIn::Bool(on) => {
                            ui.checkbox(on, "");
                        }
                        StandIn::Text(text) => {
                            ui.text_edit_singleline(text);
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }
}
