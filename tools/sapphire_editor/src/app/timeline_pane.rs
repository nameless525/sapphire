//! The timeline's panes: the moves of the document on it, its tracks and their problems, and the preview of the
//! selected move with its transport.

use super::*;
use crate::commands::Facing;

impl Editor {
    pub(super) fn moves_list(&mut self, ui: &mut Ui) {
        let Some(document) = self.document() else { return };
        let problems = self.problems();
        let rows: Vec<(usize, &str, u32, usize)> = document
            .moves
            .iter()
            .enumerate()
            .map(|(index, mv)| {
                let count = problems
                    .iter()
                    .filter(|problem| problem.in_move.as_deref() == Some(mv.name.as_str()))
                    .count();
                (index, mv.name.as_str(), mv.ticks(), count)
            })
            .collect();
        let mut choose = None;
        egui::ScrollArea::vertical().id_salt("moves").show(ui, |ui| {
            for (index, name, ticks, count) in rows {
                let selected = index == self.selected_move;
                let text = if count > 0 {
                    RichText::new(format!("{name}  ({ticks})  ! {count}"))
                        .color(Color32::from_rgb(255, 130, 115))
                } else {
                    RichText::new(format!("{name}  ({ticks})"))
                };
                if ui.selectable_label(selected, text).clicked() && !selected {
                    choose = Some(name.to_owned());
                }
            }
        });
        if let Some(name) = choose {
            self.act(Command::SelectMove(name));
        }
    }

    pub(super) fn timeline_panel(&mut self, ui: &mut Ui) {
        ui.add_space(3.0);
        let problems = self.problems();
        let editable = self.moves_document().is_some_and(|file| file.failure().is_none());
        let Some(document) = self
            .moves
            .and_then(|id| self.documents.get::<MoveSetDocument>(id))
            .and_then(|file| file.last_good())
        else {
            ui.label("Open a `.moves.ron` file.");
            return;
        };
        let Some(mv) = document.moves.get(self.selected_move) else { return };
        let root = self
            .preview
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .move_at(Some(&mv.name), self.selected_move)
                    .and_then(|mv| snapshot.set.find(Name::intern(&mv.name)))
            })
            .map(|(_, mv)| mv.tables.root_samples());
        let hit_ticks: Vec<u32> = self
            .run
            .as_ref()
            .map_or_else(Vec::new, |run| run.hits().filter(|hit| hit.on_dummy).map(|hit| hit.tick).collect());
        let view = TimelineView {
            mv,
            mv_index: self.selected_move,
            tick: self.tick,
            selected: self.selected_entry,
            problems: &problems,
            root,
            hit_ticks: &hit_ticks,
            editable,
            show_all: self.show_all_tracks,
        };
        let actions = {
            let drag = &mut self.drag;
            let mut actions = Vec::new();
            egui::ScrollArea::both()
                .id_salt("timeline")
                .auto_shrink([false, false])
                .max_height(ui.available_height() - 74.0)
                .show(ui, |ui| {
                    actions = timeline::show(ui, &view, drag);
                });
            actions
        };
        for action in actions {
            self.apply(action);
        }
        ui.separator();
        ui.horizontal(|ui| {
            let mut all = self.show_all_tracks;
            if ui.checkbox(&mut all, "All tracks").changed() {
                self.act(Command::AllTracks(all));
            }
            let command = if ui.ctx().os() == egui::os::OperatingSystem::Mac { "Cmd" } else { "Ctrl" };
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "Drag ends and bodies.   [ and ]: window's ends to the playhead.   Alt+arrows: nudge.   \
                         Delete: remove.   {command}+Z: undo.   {command}+S: save and push."
                    ))
                    .small()
                    .color(Color32::from_gray(140)),
                )
                .truncate(),
            );
        });
        self.problems_strip(ui);
        // The panel keeps what it's been given, however little there is to show.
        ui.allocate_space(ui.available_size());
    }

    /// The document's problems, whichever move they're in.
    pub(super) fn problems_strip(&mut self, ui: &mut Ui) {
        let problems = self.problems();
        if problems.is_empty() {
            let word = if !self.stale && self.preview.is_some() {
                "The document compiles."
            } else {
                "The document checks, as far as it can with no cook."
            };
            ui.label(RichText::new(word).small().color(Color32::from_gray(150)));
            return;
        }
        let mut go_to = None;
        egui::ScrollArea::vertical().id_salt("problems").max_height(44.0).show(ui, |ui| {
            for problem in &problems {
                let source = if problem.from_device { "device refused: " } else { "" };
                let text = RichText::new(format!("{source}{}", problem.describe()))
                    .color(Color32::from_rgb(255, 130, 115));
                if ui.add(egui::Label::new(text).sense(Sense::click())).clicked()
                    && let Some(name) = &problem.in_move
                {
                    go_to = Some(name.clone());
                }
            }
        });
        if let Some(name) = go_to {
            self.act(Command::SelectMove(name));
        }
    }

    pub(super) fn preview_panel(&mut self, ui: &mut Ui) {
        self.transport(ui);
        ui.horizontal_wrapped(|ui| {
            ui.label("Dummy");
            let mut distance = self.setup.dummy_distance;
            let response =
                ui.add(egui::DragValue::new(&mut distance).speed(0.02).range(0.3..=12.0).suffix(" m"));
            if response.changed() {
                self.act(Command::DummyDistance(distance));
            }
            let mut warp = self.setup.warp_to_dummy;
            if ui.checkbox(&mut warp, "Warp to it").changed() {
                self.act(Command::WarpToDummy(warp));
            }
            ui.separator();
            for (word, facing) in [("Side", Facing::Side), ("Front", Facing::Front), ("Top", Facing::Top)] {
                if ui.small_button(word).clicked() {
                    self.act(Command::View(facing));
                }
            }
            let mut bones = self.overlays.bones;
            if ui.checkbox(&mut bones, "Bones").changed() {
                self.act(Command::Bones(bones));
            }
            let mut hurtboxes = self.overlays.hurtboxes;
            if ui.checkbox(&mut hurtboxes, "Hurtboxes").changed() {
                self.act(Command::Hurtboxes(hurtboxes));
            }
        });
        let size = ui.available_size().max(Vec2::splat(40.0));
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = response.rect;
        if response.dragged() {
            self.view_fit = ViewFit::Manual;
            let delta = response.drag_delta();
            self.view.yaw += delta.x * 0.008;
            self.view.pitch = (self.view.pitch + delta.y * 0.008).clamp(-0.3, 1.55);
        }
        if response.hovered() {
            let zoom = ui.input(|input| input.smooth_scroll_delta.y);
            if zoom != 0.0 {
                self.view_fit = ViewFit::Manual;
                self.view.zoom = (self.view.zoom * (1.0 + zoom * 0.002)).clamp(0.2, 6.0);
            }
        }
        let posed = self.cook.casting.and_then(|casting| {
            let figure: &Figure = self.cook.figures.get(casting)?;
            Some(PosedFigure { figure, clip: casting.clip, generation: self.cook.generation })
        });
        let mut drew = false;
        if let Some(run) = &self.run
            && let Some(frame) = run.frames.get(self.frame)
        {
            if matches!(self.view_fit, ViewFit::Pending)
                || matches!(self.view_fit, ViewFit::Fitted(size) if size != rect.size())
            {
                self.view.fit(rect, frame, posed);
                self.view_fit = ViewFit::Fitted(rect.size());
            }
            preview::draw(&painter, rect, &self.view, run, frame, posed, &mut self.cache, self.overlays);
            drew = true;
        }
        if !drew {
            let why = self.run_error.clone().unwrap_or_else(|| "Open a `.moves.ron` file.".to_owned());
            preview::empty(&painter, rect, &why);
        }
        if self.stale && drew {
            let color = Color32::from_rgb(255, 150, 120);
            let galley = painter.layout(
                "Showing the last valid preview.".to_owned(),
                egui::FontId::proportional(13.0),
                color,
                (rect.width() - 20.0).max(1.0),
            );
            let at = rect.left_bottom() + Vec2::new(10.0, -10.0 - galley.size().y);
            painter.galley(at, galley, color);
        }
        if posed_missing(&self.cook, self.document()) {
            let why = if self.cook.cook.is_some() {
                "no character for this move in the cook"
            } else {
                "no character without a cook: `cargo xtask content`"
            };
            let at = rect.right_bottom() + Vec2::new(-10.0, -10.0);
            painter.text(
                at,
                egui::Align2::RIGHT_BOTTOM,
                why,
                egui::FontId::proportional(12.0),
                Color32::from_gray(140),
            );
        }
    }

    pub(super) fn transport(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let ticks = self
                .document()
                .and_then(|document| document.moves.get(self.selected_move))
                .map_or(0, MoveDocument::ticks);
            if ui.small_button("|<").on_hover_text("First tick (Home)").clicked() {
                self.act(Command::Tick(0));
            }
            if ui.small_button("<").on_hover_text("One tick back (Left, or comma)").clicked() {
                self.act(Command::Tick(self.tick.saturating_sub(1)));
            }
            if ui.small_button(">").on_hover_text("One tick on (Right, or full stop)").clicked() {
                self.act(Command::Tick(self.tick.saturating_add(1)));
            }
            let mut tick = self.tick;
            let response = ui.add(
                egui::DragValue::new(&mut tick)
                    .speed(0.25)
                    .range(0..=ticks.saturating_sub(1))
                    .prefix("tick "),
            );
            if response.changed() {
                self.act(Command::Tick(tick));
            }
            ui.label(format!("of {ticks}, {:.0} ms", f64::from(self.tick) * 1000.0 / 60.0));
            ui.separator();
            let playing = self.playing.is_some();
            let label =
                if playing { "Stop".to_owned() } else { format!("Play from frame {}", self.play_from) };
            if ui.button(label).on_hover_text("Space").clicked() {
                self.act(if playing { Command::Stop } else { Command::Play });
            }
            for (speed, word) in SPEEDS {
                if ui.selectable_label(self.speed == speed, word).clicked() {
                    self.act(Command::Speed(speed));
                }
            }
            let mut looping = self.looping;
            if ui.checkbox(&mut looping, "Loop").changed() {
                self.act(Command::Loop(looping));
            }
        });
    }
}

/// Whether the current move should have had a figure and didn't.
pub(super) fn posed_missing(cook: &CookState, document: Option<&MoveSetDocument>) -> bool {
    document.is_some() && cook.casting.is_none()
}
