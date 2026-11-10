//! The screen pane (M3 E5): the current `.ui.ron`'s preview at an aspect and in a language, and the values its
//! paths read, typed in. The viewport draws it through the game's own screens ([`crate::viewport`]); an edit
//! anywhere else, the Properties pane's or a typed one, shows there as the document reads.
//!
//! The pane follows the model file a screen names, and the file itself: each [`Editor::poll`] reads every held
//! model file again and compares its bytes, by their hash, with what it read last (M3 Z5c). Not the file's
//! modification time and length: the usual edit to a model renames a path or changes a kind, often keeping the
//! length, and a time coarser than the edit (a second, on some file systems and shares) would miss it for good.
//! Reading is cheap at this size, a few hundred bytes a second for each screen open, which is what the document
//! set already does for every document.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use game_ui::menus::ScreenModel;
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
    /// The model file the document named when this was read: an edit naming another reads that one.
    named: Option<String>,
    /// The hash of the model file's bytes as last read, or `None` if it named none or none read: other bytes
    /// on disk read it again.
    read: Option<u64>,
    /// The model file it names, as it read, or why it didn't.
    model: Result<UiModel, String>,
    /// What each path of the last model that read reads, kept while one doesn't.
    values: Vec<(String, StandIn)>,
}

impl ScreenPanel {
    /// Reads the model of `current`, if it's a screen with none held yet, or one whose document names another
    /// model file now. A screen names its model file beside it. One that has never read names none yet, so
    /// nothing's held for it until it does.
    pub(super) fn follow(&mut self, documents: &DocumentSet, current: Option<AssetId>) {
        let Some(id) = current else { return };
        let (Some(document), Some(path)) = (documents.get::<UiDocument>(id), documents.path(id)) else {
            return;
        };
        let Some(named) = document.last_good().map(|screen| screen.model.as_deref()) else { return };
        let held = self.screens.get(&id);
        if held.is_some_and(|held| held.named.as_deref() == named) {
            return;
        }
        let (model, read) = match named {
            None => (Ok(UiModel::default()), None),
            Some(file) => {
                let at = model_path(path, file);
                let bytes = std::fs::read(&at).map_err(|error| format!("{}: {error}", at.display()));
                let read = bytes.as_deref().ok().map(hash);
                (bytes.and_then(|bytes| parse(file, &bytes)), read)
            }
        };
        let held = held.map(|held| held.values.as_slice());
        let values = match &model {
            Ok(model) => values_for(named, model, held),
            // A model that doesn't read lists no paths: what was typed waits for one that does.
            Err(_) => held.map(<[_]>::to_vec).unwrap_or_default(),
        };
        let named = named.map(ToOwned::to_owned);
        self.screens.insert(id, Held { named, read, model, values });
    }

    /// Reads again each held screen's model file whose bytes on disk aren't those it read last: an edit made
    /// outside the editor. What was typed stays, as for a model named anew, and through an edit that doesn't
    /// parse. A file that doesn't read just now (an outside save between its truncation and its write, or a
    /// rename) waits, keeping the model it had; one that reads and doesn't parse says why, as when first read.
    pub(super) fn poll_models(&mut self, documents: &DocumentSet) {
        for (&id, held) in &mut self.screens {
            let (Some(file), Some(path)) = (held.named.as_deref(), documents.path(id)) else { continue };
            let Ok(bytes) = std::fs::read(model_path(path, file)) else { continue };
            let read = Some(hash(&bytes));
            if read == held.read {
                continue;
            }
            let model = parse(file, &bytes);
            if let Ok(new) = &model {
                held.values = values_for(Some(file), new, Some(&held.values));
            }
            (held.read, held.model) = (read, model);
        }
    }

    /// Lets go of what's held for a screen closed, so opening it again reads its model file again.
    pub(super) fn forget(&mut self, id: AssetId) {
        self.screens.remove(&id);
    }
}

/// Where a screen at `screen` finds the model file it names: beside it.
fn model_path(screen: &Path, file: &str) -> PathBuf {
    screen.parent().map_or_else(|| file.into(), |dir| dir.join(file))
}

fn parse(file: &str, bytes: &[u8]) -> Result<UiModel, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| format!("{file}: {error}"))?;
    UiModel::from_document(text).map_err(|error| format!("{file}: {error}"))
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The values a screen's paths read under `model`, the file `named`. What was `typed` in stays, for each path
/// the new model still reads the same kind of value at. With nothing held yet (a first model), a screen reading
/// one of the game's own model files starts where the game's goldens do (M3 U9).
fn values_for(
    named: Option<&str>,
    model: &UiModel,
    typed: Option<&[(String, StandIn)]>,
) -> Vec<(String, StandIn)> {
    let mut values = stand_ins(model);
    match typed.filter(|typed| !typed.is_empty()) {
        Some(typed) => {
            for (path, value) in &mut values {
                let known = typed.iter().find(|(known, _)| known == path);
                if let Some((_, typed)) = known.filter(|(_, typed)| same_kind(typed, value)) {
                    value.clone_from(typed);
                }
            }
        }
        None => {
            if let Some(kind) = named.and_then(ScreenModel::of_file) {
                read_from(&mut values, &*kind.sample());
            }
        }
    }
    values
}

fn same_kind(a: &StandIn, b: &StandIn) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
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
