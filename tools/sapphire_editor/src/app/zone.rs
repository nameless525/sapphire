//! The zone pane (M3 E5): the current zone document's placements, those selected, and the gizmo that moves the
//! last one selected in the viewport. A Cmd or Shift click adds a row to the selection or takes it out (M6 T2),
//! and the Properties pane edits what the placements selected share. A placement is selected by its ID (M6 T8a),
//! which each row shows, so a selection and a drag stay on it however the list shifts; a version 1 zone has its
//! places' IDs until `MigrateZone` writes them.
//!
//! The viewport draws a zone as an editor needs it, each placement on its own ([`crate::viewport`]), and the
//! gizmo on the one selected. A drag of an arrow is one gesture on the document: each frame of it sets the one
//! number of the placement's `at` that the arrow moves, snapped to [`SNAP`] so the file reads as a person would
//! write it, and undo takes the whole drag back to the bytes it started from. What the zone's checks refuse
//! stays as it was. A placement that names colliders takes them with it, so the drag is one step of the zone and
//! its colliders' document, which undo takes back whole. Everything else about a placement is the Properties
//! pane's.

use egui::{RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_document::{Document, DocumentSet, ObjectId, PropertyPath, Step, write};
use sapphire_math::Vec3;
use sapphire_zone::{IdList, ZoneDocument};

use super::{Editor, Notice, selection};
use crate::commands::Command;
use crate::gizmo::{Axis, Gizmo, ViewCamera, along};
use crate::viewport::ZoneShown;

/// What a drag rounds a placement's position to, in metres.
pub const SNAP: f32 = 0.05;

/// What a drag of the gizmo is called in the history.
const MOVE_LABEL: &str = "move a placement";

/// The pane's state: what the pointer's doing to the gizmo. What's selected is the editor's selection.
#[derive(Debug, Default)]
pub(super) struct ZonePanel {
    drag: Option<GizmoDrag>,
    /// The arrow under the pointer.
    hot: Option<Axis>,
}

/// A drag of an arrow.
#[derive(Clone, Debug)]
struct GizmoDrag {
    zone: AssetId,
    /// The placement's path, by its ID.
    placement: PropertyPath,
    axis: Axis,
    /// Where the placement was when it started.
    from: Vec3,
    /// How far along the axis the press was.
    pressed: f32,
    /// What it last set the number to.
    set: Option<f32>,
}

/// How near a placement's place a click must be to select it, in points.
const CLICK_REACH: f32 = 16.0;

impl ZonePanel {
    /// The pointer's share of the viewport, before the view takes what's left: a press on an arrow starts a drag
    /// of it, which moves the placement until the press ends. What it does is commands, in `asked`, for the editor
    /// to run once the viewport has drawn. Says whether it took the pointer.
    #[allow(
        clippy::too_many_arguments,
        reason = "the viewport's pointer and camera, and what the editor holds"
    )]
    pub(super) fn gizmo(
        &mut self,
        documents: &DocumentSet,
        zone: Option<AssetId>,
        selected: Option<usize>,
        response: &egui::Response,
        camera: Option<&ViewCamera>,
        rect: egui::Rect,
        asked: &mut Vec<Command>,
    ) -> bool {
        self.hot = None;
        let (Some(zone), Some(camera)) = (zone, camera) else {
            self.drag = None;
            return false;
        };
        let Some(document) = documents.get::<ZoneDocument>(zone) else { return false };
        let pointer = response.ctx.input(|input| input.pointer.clone());
        let Some(gizmo) = self.gizmo_on(document, selected, camera) else {
            self.drag = None;
            self.click_to_select(document, response, camera, rect, asked);
            return false;
        };
        if pointer.primary_pressed()
            && let Some(at) = pointer.press_origin().filter(|at| rect.contains(*at))
            && let Some(axis) = gizmo.pick(camera, rect, at)
            && let Some(index) = selected
            && let Ok(placement) = document.entry_path(&placements(), index)
            && let Some(pressed) = along(gizmo.origin, axis, camera.ray(at, rect))
        {
            asked.push(Command::Begin(MOVE_LABEL.to_owned()));
            self.drag = Some(GizmoDrag { zone, placement, axis, from: gizmo.origin, pressed, set: None });
        }
        let Some(mut drag) = self.drag.clone().filter(|drag| drag.zone == zone) else {
            self.hot = pointer.hover_pos().and_then(|at| gizmo.pick(camera, rect, at));
            if self.hot.is_none() {
                self.click_to_select(document, response, camera, rect, asked);
            }
            return false;
        };
        self.hot = Some(drag.axis);
        if !pointer.primary_down() {
            asked.push(Command::End);
            self.drag = None;
            return true;
        }
        if let Some(at) = pointer.interact_pos()
            && let Some(now) = along(drag.from, drag.axis, camera.ray(at, rect))
        {
            let moved = drag.from[drag.axis.index()] + now - drag.pressed;
            let snapped = (moved / SNAP).round() * SNAP;
            // A frame where it hasn't moved asks nothing, unless an undo or a save ended its gesture under it: then
            // it sets the number again, as each frame of a held drag did before.
            if drag.set != Some(snapped) || !document.in_gesture() {
                drag.set = Some(snapped);
                self.drag = Some(drag.clone());
                // An undo or a save while it's held ends its gesture, and the rest of it is one of its own.
                if !document.in_gesture() && !asked.iter().any(|command| matches!(command, Command::Begin(_)))
                {
                    asked.push(Command::Begin(MOVE_LABEL.to_owned()));
                }
                // Refused or not, the drag goes on: the document's checks are the word on what it holds.
                if let Ok(value) = write::canonical(&snapped, document.registry()) {
                    let path = placement_at(&drag.placement, drag.axis).to_string();
                    asked.push(Command::SetProperty { path, value });
                }
            }
        }
        true
    }

    /// A click off the gizmo selects the placement whose place shows nearest it, within [`CLICK_REACH`], or
    /// nothing.
    fn click_to_select(
        &self,
        document: &Document<ZoneDocument>,
        response: &egui::Response,
        camera: &ViewCamera,
        rect: egui::Rect,
        asked: &mut Vec<Command>,
    ) {
        let (true, Some(at), Some(value)) =
            (response.clicked(), response.interact_pointer_pos(), document.value())
        else {
            return;
        };
        let nearest = value
            .placements
            .iter()
            .filter_map(|placement| {
                let shown = camera.to_screen(Vec3::from(placement.at), rect)?;
                Some((shown.distance(at), &placement.id))
            })
            .filter(|(distance, _)| *distance <= CLICK_REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        asked.push(Command::SelectPlacement(nearest.map(|(_, id)| id.clone())));
    }

    /// The gizmo on placement `selected` of `document`, as `camera` sees it.
    fn gizmo_on(
        &self,
        document: &Document<ZoneDocument>,
        selected: Option<usize>,
        camera: &ViewCamera,
    ) -> Option<Gizmo> {
        let placement = document.value()?.placements.get(selected?)?;
        let hot = self.drag.as_ref().map(|drag| drag.axis).or(self.hot);
        Some(Gizmo { hot, ..Gizmo::at(Vec3::from(placement.at), camera) })
    }
}

/// The path of a zone's placements.
fn placements() -> PropertyPath {
    PropertyPath::root().join(Step::Field("placements".to_owned()))
}

/// The path of one number of a placement's position.
fn placement_at(placement: &PropertyPath, axis: Axis) -> PropertyPath {
    placement.join(Step::Field("at".to_owned())).join(Step::Index(axis.index()))
}

/// What the viewport shows of `current`, if it's a zone that's read, with the gizmo on placement `selected`.
pub(super) fn zone_shown<'a>(
    documents: &'a DocumentSet,
    panel: &ZonePanel,
    current: Option<AssetId>,
    selected: Option<usize>,
    camera: Option<&ViewCamera>,
) -> Option<ZoneShown<'a>> {
    let id = current?;
    let file = documents.get::<ZoneDocument>(id)?;
    let stem = documents.path(id)?.file_name()?.to_str()?.strip_suffix(".zone.ron")?;
    let gizmo = camera.and_then(|camera| panel.gizmo_on(file, selected, camera));
    Some(ZoneShown { id, stem, document: file.last_good()?, gizmo })
}

impl Editor {
    /// The current document, if it's a zone.
    fn current_zone(&self) -> Option<AssetId> {
        let id = self.current?;
        self.documents.get::<ZoneDocument>(id).map(|_| id)
    }

    /// The viewport's pointer, for the gizmo first.
    pub(super) fn viewport_pane(&mut self, ui: &mut Ui) {
        let (zone, selected) = (self.current_zone(), self.selected_placement());
        let (documents, panel, asked) = (&self.documents, &mut self.zone, &mut self.asked);
        self.viewport.ui(ui, |response, camera, rect| {
            panel.gizmo(documents, zone, selected, response, camera, rect, asked)
        });
        if self.viewport.take_asked_to_play() {
            self.asked.push(Command::PlayInViewport(None));
        }
    }

    /// Selects the placement of the current zone called `id` alone, or nothing: refused if it has none.
    pub fn select_placement(&mut self, id: Option<&str>) {
        let Some(id) = id else {
            self.selection.clear();
            return;
        };
        let zone = self.current_zone();
        let value = zone.and_then(|zone| self.documents.get::<ZoneDocument>(zone)).and_then(Document::value);
        match (zone, value.and_then(|value| value.find_id(id))) {
            (Some(document), Some((IdList::Placements, _))) => {
                self.selection = vec![ObjectId { document, path: selection::placement_path(id) }];
            }
            _ => self.refuse(format!("the zone has no placement `{id}`")),
        }
    }

    /// Writes the current zone at its document's newest version with the IDs it reads with, as one step, and saves
    /// it: what [`Command::MigrateZone`] does ([`sapphire_document::zone::migrate`]). The selection stays, since
    /// its IDs are the ones it had.
    pub(super) fn migrate_zone(&mut self) {
        let Some(id) = self.current_zone() else {
            self.refuse("the current document isn't a zone");
            return;
        };
        if !self.take_typed() {
            return;
        }
        let text =
            self.documents.get::<ZoneDocument>(id).map(|zone| zone.text().to_owned()).unwrap_or_default();
        let migrated = match sapphire_document::zone::migrate(&text) {
            Ok(Some(migrated)) => migrated,
            Ok(None) => {
                self.say(Notice {
                    text: "the zone is at its newest version already".to_owned(),
                    trouble: false,
                });
                return;
            }
            Err(error) => {
                self.refuse(error.to_string());
                return;
            }
        };
        let document = self
            .documents
            .edit(id)
            .and_then(|open| open.as_any_mut().downcast_mut::<Document<ZoneDocument>>());
        let Some(document) = document else { return };
        if let Err(refused) = document.rewrite("migrate the zone", migrated) {
            self.refuse(refused.to_string());
            return;
        }
        self.after_property_edit(Some(id), Ok(()));
        match self.save() {
            Ok(()) => self.say(Notice {
                text: "migrated the zone to version 2 and saved it".to_owned(),
                trouble: false,
            }),
            Err(text) => self.refuse(text),
        }
    }

    /// The placement of the current zone selected last, which the gizmo is on.
    #[must_use]
    pub fn selected_placement(&self) -> Option<usize> {
        self.selected_placements().pop()
    }

    /// The placements of the current zone selected, by where they are in its list now, in the order they were.
    #[must_use]
    pub fn selected_placements(&self) -> Vec<usize> {
        let Some(zone) = self.current_zone() else { return Vec::new() };
        let Some(value) = self.documents.get::<ZoneDocument>(zone).and_then(Document::value) else {
            return Vec::new();
        };
        self.selection.iter().filter_map(|object| selection::placement_of(object, zone, value)).collect()
    }

    pub(super) fn zone_panel(&mut self, ui: &mut Ui) {
        let Some(id) = self.current_zone() else {
            ui.label("Open a zone to place what's in it.");
            return;
        };
        let Some(document) = self.documents.get::<ZoneDocument>(id).and_then(Document::value) else {
            ui.label("The zone doesn't read, so nothing here moves until it does.");
            return;
        };
        let name_of = |prefab: AssetId| {
            let source = self.documents.sidecars().source_of(prefab);
            let name = source.as_deref().and_then(|path| path.file_name()).and_then(|name| name.to_str());
            name.map_or_else(|| prefab.to_string(), |name| name.trim_end_matches(".assembly.ron").to_owned())
        };
        let rows: Vec<(String, String, [f32; 3])> = document
            .placements
            .iter()
            .map(|placement| {
                let mut name = name_of(placement.prefab);
                if placement.backdrop {
                    name.push_str(" (backdrop)");
                }
                (placement.id.clone(), name, placement.at)
            })
            .collect();
        let split = document.has_split();
        let selected = self.selected_placements();
        if split {
            ui.label(
                RichText::new("A split placement cooks into the cells alone: the viewport draws this zone once it's cooked without one.")
                    .weak(),
            );
        }
        let mut choose = None;
        egui::ScrollArea::vertical().id_salt("zone").auto_shrink([false, false]).show(ui, |ui| {
            for (index, (id, name, at)) in rows.into_iter().enumerate() {
                let text = format!("{index}  {id}  {name}  ({:.2}, {:.2}, {:.2})", at[0], at[1], at[2]);
                if ui.selectable_label(selected.contains(&index), text).clicked() {
                    // By the release's own modifiers, which a click has whatever the keys' state says by then.
                    let adding = ui.input(|input| {
                        input.events.iter().any(|event| {
                            matches!(event, egui::Event::PointerButton { pressed: false, modifiers, .. }
                                if modifiers.command || modifiers.shift)
                        })
                    });
                    choose = Some((id, adding));
                }
            }
        });
        match choose {
            Some((placement, true)) => self.toggle_placement(id, &placement),
            Some((placement, false)) => self.act(Command::SelectPlacement(Some(placement))),
            None => {}
        }
    }
}
