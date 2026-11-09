//! The zone pane (M3 E5): the current zone document's placements, one of them selected, and the gizmo that moves
//! it in the viewport.
//!
//! The viewport draws a zone as an editor needs it, each placement on its own ([`crate::viewport`]), and the
//! gizmo on the one selected. A drag of an arrow is one gesture on the document: each frame of it sets the one
//! number of the placement's `at` that the arrow moves, snapped to [`SNAP`] so the file reads as a person would
//! write it, and undo takes the whole drag back to the bytes it started from. What the zone's checks refuse
//! stays as it was. Everything else about a placement is the Properties pane's.

use egui::{RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_document::{Document, DocumentSet, PropertyPath};
use sapphire_math::Vec3;
use sapphire_zone::ZoneDocument;

use super::Editor;
use crate::gizmo::{Axis, Gizmo, ViewCamera, along};
use crate::viewport::ZoneShown;

/// What a drag rounds a placement's position to, in metres.
pub const SNAP: f32 = 0.05;

/// What a drag of the gizmo is called in the history.
const MOVE_LABEL: &str = "move a placement";

/// The pane's state: what's selected and what the pointer's doing to the gizmo.
#[derive(Debug, Default)]
pub(super) struct ZonePanel {
    /// The zone it's for, and the placement selected in it.
    selected: Option<(AssetId, usize)>,
    drag: Option<GizmoDrag>,
    /// The arrow under the pointer.
    hot: Option<Axis>,
}

/// A drag of an arrow.
#[derive(Clone, Copy, Debug)]
struct GizmoDrag {
    zone: AssetId,
    placement: usize,
    axis: Axis,
    /// Where the placement was when it started.
    from: Vec3,
    /// How far along the axis the press was.
    pressed: f32,
}

/// How near a placement's place a click must be to select it, in points.
const CLICK_REACH: f32 = 16.0;

impl ZonePanel {
    /// The placement selected in `zone`.
    fn selected_in(&self, zone: AssetId) -> Option<usize> {
        self.selected.filter(|(id, _)| *id == zone).map(|(_, index)| index)
    }

    /// The pointer's share of the viewport, before the view takes what's left: a press on an arrow starts a drag
    /// of it, which moves the placement until the press ends. Says whether it took the pointer.
    pub(super) fn gizmo(
        &mut self,
        documents: &mut DocumentSet,
        zone: Option<AssetId>,
        response: &egui::Response,
        camera: Option<&ViewCamera>,
        rect: egui::Rect,
    ) -> bool {
        self.hot = None;
        let (Some(zone), Some(camera)) = (zone, camera) else {
            self.drag = None;
            return false;
        };
        let Some(document) = documents.get_mut::<ZoneDocument>(zone) else { return false };
        let pointer = response.ctx.input(|input| input.pointer.clone());
        let Some(gizmo) = self.gizmo_on(document, zone, camera) else {
            self.drag = None;
            self.click_to_select(document, zone, response, camera, rect);
            return false;
        };
        if pointer.primary_pressed()
            && let Some(at) = pointer.press_origin().filter(|at| rect.contains(*at))
            && let Some(axis) = gizmo.pick(camera, rect, at)
            && let Some(placement) = self.selected_in(zone)
            && let Some(pressed) = along(gizmo.origin, axis, camera.ray(at, rect))
        {
            document.begin_gesture(MOVE_LABEL);
            self.drag = Some(GizmoDrag { zone, placement, axis, from: gizmo.origin, pressed });
        }
        let Some(drag) = self.drag.filter(|drag| drag.zone == zone) else {
            self.hot = pointer.hover_pos().and_then(|at| gizmo.pick(camera, rect, at));
            if self.hot.is_none() {
                self.click_to_select(document, zone, response, camera, rect);
            }
            return false;
        };
        self.hot = Some(drag.axis);
        if !pointer.primary_down() {
            document.end_gesture();
            self.drag = None;
            return true;
        }
        if let Some(at) = pointer.interact_pos()
            && let Some(now) = along(drag.from, drag.axis, camera.ray(at, rect))
        {
            let moved = drag.from[drag.axis.index()] + now - drag.pressed;
            let snapped = (moved / SNAP).round() * SNAP;
            let path = placement_at(drag.placement, drag.axis);
            // Refused or not, the drag goes on: the document's checks are the word on what it holds.
            let _ = document.set(MOVE_LABEL, &path, &snapped);
        }
        true
    }

    /// A click off the gizmo selects the placement whose place shows nearest it, within [`CLICK_REACH`], or
    /// nothing.
    fn click_to_select(
        &mut self,
        document: &Document<ZoneDocument>,
        zone: AssetId,
        response: &egui::Response,
        camera: &ViewCamera,
        rect: egui::Rect,
    ) {
        let (true, Some(at), Some(value)) =
            (response.clicked(), response.interact_pointer_pos(), document.value())
        else {
            return;
        };
        let nearest = value
            .placements
            .iter()
            .enumerate()
            .filter_map(|(index, placement)| {
                let shown = camera.to_screen(Vec3::from(placement.at), rect)?;
                Some((shown.distance(at), index))
            })
            .filter(|(distance, _)| *distance <= CLICK_REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        self.selected = nearest.map(|(_, index)| (zone, index));
    }

    /// The gizmo on what's selected in `document`, as `camera` sees it.
    fn gizmo_on(
        &self,
        document: &Document<ZoneDocument>,
        zone: AssetId,
        camera: &ViewCamera,
    ) -> Option<Gizmo> {
        let placement = document.value()?.placements.get(self.selected_in(zone)?)?;
        let hot = self.drag.map(|drag| drag.axis).or(self.hot);
        Some(Gizmo { hot, ..Gizmo::at(Vec3::from(placement.at), camera) })
    }
}

/// The path of one number of a placement's position.
fn placement_at(placement: usize, axis: Axis) -> PropertyPath {
    format!("placements[{placement}].at[{}]", axis.index()).parse().expect("a path that parses")
}

/// What the viewport shows of `current`, if it's a zone that's read, with the gizmo where `panel` has it.
pub(super) fn zone_shown<'a>(
    documents: &'a DocumentSet,
    panel: &ZonePanel,
    current: Option<AssetId>,
    camera: Option<&ViewCamera>,
) -> Option<ZoneShown<'a>> {
    let id = current?;
    let file = documents.get::<ZoneDocument>(id)?;
    let stem = documents.path(id)?.file_name()?.to_str()?.strip_suffix(".zone.ron")?;
    let gizmo = camera.and_then(|camera| panel.gizmo_on(file, id, camera));
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
        let zone = self.current_zone();
        let (documents, panel) = (&mut self.documents, &mut self.zone);
        self.viewport.ui(ui, |response, camera, rect| panel.gizmo(documents, zone, response, camera, rect));
        if self.viewport.take_asked_to_play() {
            self.play_in_viewport(None);
        }
    }

    /// Selects placement `index` of the current zone, or nothing.
    pub fn select_placement(&mut self, index: Option<usize>) {
        self.zone.selected = self.current_zone().zip(index);
    }

    /// The placement selected in the current zone.
    #[must_use]
    pub fn selected_placement(&self) -> Option<usize> {
        self.zone.selected_in(self.current_zone()?)
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
        let rows: Vec<(String, [f32; 3])> = document
            .placements
            .iter()
            .map(|placement| {
                let mut name = name_of(placement.prefab);
                if placement.backdrop {
                    name.push_str(" (backdrop)");
                }
                (name, placement.at)
            })
            .collect();
        let split = document.has_split();
        let selected = self.zone.selected_in(id);
        if split {
            ui.label(
                RichText::new("A split placement cooks into the cells alone: the viewport draws this zone once it's cooked without one.")
                    .weak(),
            );
        }
        let mut choose = None;
        egui::ScrollArea::vertical().id_salt("zone").auto_shrink([false, false]).show(ui, |ui| {
            for (index, (name, at)) in rows.into_iter().enumerate() {
                let text = format!("{index}  {name}  ({:.2}, {:.2}, {:.2})", at[0], at[1], at[2]);
                if ui.selectable_label(selected == Some(index), text).clicked() {
                    choose = Some(index);
                }
            }
        });
        if let Some(index) = choose {
            self.zone.selected = Some((id, index));
        }
    }
}
