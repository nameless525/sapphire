//! Cooking from the editor (M6 T5): every save asks [`Cooker`] for a cook of the content into the cook the editor
//! reads, the top bar says how the last went, and a refusal shows on its document's field in Properties. The
//! viewport takes what landed: [`Editor::viewport_frame`] counts the cooks, and the stage remounts the packs.

use super::*;
use crate::cook::{CookPlan, CookRefusal, CookResult, CookStats, Pipeline, Runner};

impl Editor {
    /// Cooks with `runner` from now on, rather than the pipeline the cook's folder names: a test's stand-in, or a
    /// pipeline cooking another tree.
    pub fn cook_with(&mut self, runner: Box<dyn Runner>) {
        self.cooker = Some(Cooker::new(runner));
        self.no_cooker = None;
    }

    /// Documents `saved` were written: asks for a cook if the content holds any of them, and there's a cook to remake.
    pub(super) fn ask_cook(&mut self, saved: &[AssetId]) {
        if self.cooker.is_none() {
            let plan = match &self.content {
                Some(packs) => CookPlan::for_cook(packs, self.pipeline.as_deref()),
                None => {
                    Err("no cook to remake: `cargo xtask content --cook editor` makes the first".to_owned())
                }
            };
            match plan {
                Ok(plan) => self.cooker = Some(Cooker::new(Box::new(Pipeline::new(plan)))),
                Err(why) => {
                    self.no_cooker = Some(why);
                    return;
                }
            }
        }
        let Some(cooker) = &mut self.cooker else { return };
        if saved.iter().filter_map(|id| self.documents.path(*id)).any(|path| cooker.reads(path)) {
            cooker.ask(self.now);
        }
    }

    /// Looks at the cook under way, every pass: a request's quiet is a third of a second, finer than a poll.
    pub(super) fn poll_cook(&mut self, now: f64) {
        let ended = self.cooker.as_mut().and_then(|cooker| cooker.poll(now));
        if let Some(ended) = ended {
            self.cook_ended(&ended);
        }
    }

    /// Runs the cooks asked for at once and waits for them: a script's end, before it draws, or a test.
    pub fn finish_cooks(&mut self) {
        let ended = self.cooker.as_mut().and_then(Cooker::settle);
        if let Some(ended) = ended {
            self.cook_ended(&ended);
        }
    }

    /// Finds the document the cook refused among those open, once, so Properties compares IDs as it draws.
    fn cook_ended(&mut self, ended: &CookResult) {
        if let Ok(cooked) = ended {
            self.push_packs_to_game(cooked);
        }
        let source = ended.as_ref().err().and_then(|refusal| refusal.source.as_deref()?.canonicalize().ok());
        self.refused = source.and_then(|source| {
            let documents = &self.documents;
            documents.ids().find(|id| {
                documents.path(*id).and_then(|path| path.canonicalize().ok()) == Some(source.clone())
            })
        });
    }

    /// What the editor's cooks have done (R9), once a save has asked for one.
    #[must_use]
    pub fn cook_stats(&self) -> Option<CookStats> {
        self.cooker.as_ref().map(Cooker::stats)
    }

    /// Why the last cook was refused, if it was.
    #[must_use]
    pub fn cook_refusal(&self) -> Option<&CookRefusal> {
        self.cooker.as_ref()?.last()?.as_ref().err()
    }

    /// How many cooks have landed since the editor's cooker started, each counted though a later one failed.
    #[must_use]
    pub fn cooks_landed(&self) -> u64 {
        self.cooker.as_ref().map_or(0, |cooker| cooker.stats().landed)
    }

    /// The field the last cook refused in document `id`, if the refusal is that document's.
    pub(super) fn refused_field(&self, id: AssetId) -> Option<(&str, &CookRefusal)> {
        let refusal = self.cook_refusal()?;
        (self.refused == Some(id)).then(|| (refusal.field.as_deref().unwrap_or_default(), refusal))
    }

    /// The top bar's word on the cook: under way, landed in so long, or refused and why.
    pub(super) fn cook_status(&self, ui: &mut Ui) {
        if let Some(why) = &self.no_cooker {
            ui.label(RichText::new("no cook").color(Color32::from_gray(140))).on_hover_text(why);
            return;
        }
        let Some(cooker) = &self.cooker else { return };
        if cooker.is_busy() {
            ui.label(RichText::new("cooking").color(Color32::from_rgb(150, 190, 255)));
            return;
        }
        match cooker.last() {
            Some(Ok(_)) => {
                ui.label(format!("cooked in {} ms", cooker.stats().cook_ms));
            }
            Some(Err(refusal)) => {
                let name =
                    refusal.source.as_deref().and_then(Path::file_name).map(|name| name.to_string_lossy());
                let text = match (name, &refusal.field) {
                    (Some(name), Some(field)) => format!("cook refused {name} at {field}"),
                    (Some(name), None) => format!("cook refused {name}"),
                    (None, _) => "cook refused".to_owned(),
                };
                ui.colored_label(Color32::from_rgb(255, 120, 110), text).on_hover_text(&refusal.message);
            }
            None => {}
        }
    }
}
