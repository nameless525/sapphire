//! The zone pane (M3 E5): the current zone document's objects, those selected, and the gizmo on the last object
//! selected in the viewport. A Cmd or Shift click adds a row to the selection or takes it out (M6 T2),
//! and the Properties pane edits what the objects selected share. An object is selected by its keyed path (M6
//! T8a): a placement, an instance, a light or a volume by its ID, which each row shows, anything else by its name,
//! so a selection and a drag stay on it however its list shifts; a version 1 zone has its places' IDs until
//! `MigrateZone` writes them.
//!
//! Every object of the zone (M6 T9a, [`crate::markers`]) is listed by kind, in the order the document has it, as
//! many rows as show and no more built, however many it holds. The viewport marks each through the stage's debug
//! draws, and picks one by a click ([`Command::PickInZone`]) or a box drawn with Shift held
//! ([`Command::SelectInBox`]), through a [`PickIndex`] on the zone's grid. The objects and their index are made
//! from the document once a revision of it ([`ZoneCache`]), and a revision is unique in the process, so a zone
//! closed and opened again, or recovered, is made anew. A selection, a pick or a box makes nothing; but each frame of
//! a drag that moves something sets the document, which is a revision, so a drag makes them once a frame it moves
//! the object, of the same order as the document's own read of each set (T12 prices both).
//!
//! The viewport draws a zone as an editor needs it, each placement on its own ([`crate::viewport`]), and the
//! gizmo on the object selected last (M6 T9b, [`crate::handles`]): its handles to move, turn or scale it in the
//! mode the pane's bar picks ([`Command::GizmoMode`]), its arrows along the zone's axes or its own
//! ([`Command::GizmoAxes`]), snapped to the steps the bar sets ([`Command::Snap`]). A press on a handle takes hold
//! of it ([`Command::Grab`], with the press's ray), each frame the pointer moves drags it ([`Command::DragTo`],
//! with its ray) and the release lets go ([`Command::End`]): one gesture on the document, whose frames each set
//! the numbers the handle changes, so undo takes the whole drag back to the bytes it started from. The commands
//! carry rays, not numbers, so a script drags a handle as a hand does. What the zone's checks refuse stays as it
//! was. A placement that names colliders takes them with it as it moves or turns, so the drag is one step of the
//! zone and its colliders' document, which undo takes back whole. The bar also snaps what's selected to the ground
//! ([`Command::SnapToGround`], [`crate::ground`]), and duplicates ([`Command::Duplicate`]) or deletes
//! ([`Command::DeleteObjects`]) it with its colliders, each one step. Everything else about an object is the
//! Properties pane's.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_colliders::ColliderDocument;
use sapphire_document::{Document, DocumentSet, ObjectId, PropertyPath, walk, write};
use sapphire_math::{Mat4, Vec2, Vec3};
use sapphire_zone::{IdList, ZoneDocument};

use super::{Editor, Notice, selection};
use crate::commands::Command;
use crate::gizmo::{Gizmo, Ray, ViewCamera};
use crate::ground::Ground;
use crate::handles::{Axes, Drag, GizmoMode, Grip, Handle, Number, Steps};
use crate::markers::{ObjectKind, ZoneObjects};
use crate::pick::{Pick, PickIndex, Visits};
use crate::viewport::{ZoneMarks, ZoneShown};
use sapphire_document::zone::Held;

/// What a drag rounds a position to at first, in metres: the grid's step until the bar sets another.
pub const SNAP: f32 = 0.05;

/// How far about the pointer a click picks, in points.
const CLICK_REACH: f32 = 16.0;

/// How far the pointer must go with Shift held before it's a box rather than a click, in points.
const BOX_START: f32 = 4.0;

/// What's made of a zone document once a revision: its objects, the index they're picked by and where each is by
/// its path.
#[derive(Debug)]
pub(super) struct ZoneCache {
    document: AssetId,
    revision: u64,
    objects: Arc<ZoneObjects>,
    index: PickIndex,
    by_path: HashMap<PropertyPath, usize>,
    /// The pane's rows.
    rows: Vec<Row>,
    /// What making the index visited.
    built: Visits,
}

impl ZoneCache {
    fn of(document: AssetId, revision: u64, zone: &ZoneDocument) -> Self {
        let objects = ZoneObjects::of(zone);
        let (index, built) = PickIndex::build(&objects);
        let by_path =
            objects.objects.iter().enumerate().map(|(at, object)| (object.path.clone(), at)).collect();
        let rows = rows(&objects);
        Self { document, revision, objects: Arc::new(objects), index, by_path, rows, built }
    }

    /// Where the objects of `selection` in this zone are among its objects, sorted.
    fn selected(&self, selection: &[ObjectId]) -> Vec<usize> {
        let mut selected: Vec<usize> = selection
            .iter()
            .filter(|object| object.document == self.document)
            .filter_map(|object| self.by_path.get(&object.path).copied())
            .collect();
        selected.sort_unstable();
        selected.dedup();
        selected
    }

    /// The object the gizmo is on: the one of `selection` in this zone selected last that has a place to take
    /// hold of, with what it has, read from `zone`.
    fn gripped(&self, zone: &ZoneDocument, selection: &[ObjectId]) -> Option<Gripped> {
        selection.iter().rev().filter(|object| object.document == self.document).find_map(|object| {
            let found = &self.objects.objects[*self.by_path.get(&object.path)?];
            let grip = Grip::of(zone, found)?;
            Some(Gripped { kind: found.kind, grip })
        })
    }
}

/// The object the gizmo is on, and what it has to take hold of.
#[derive(Clone, Debug)]
struct Gripped {
    kind: ObjectKind,
    grip: Grip,
}

/// The pane's state: what the gizmo shows and snaps to, what the pointer's doing to it, and what's made of the
/// zone. What's selected is the editor's selection.
#[derive(Debug, Default)]
pub(super) struct ZonePanel {
    mode: GizmoMode,
    axes: Axes,
    steps: Steps,
    /// The handle held, as [`Command::Grab`] took it.
    drag: Option<HandleDrag>,
    /// The handle the pointer pressed, and where it last asked to drag it to: the interface's share of a drag,
    /// until the release.
    held: Option<(Handle, egui::Pos2)>,
    /// The handle under the pointer.
    hot: Option<Handle>,
    /// Where a box being drawn started, and where the pointer is now, in the pane.
    boxing: Option<(egui::Pos2, egui::Pos2)>,
    cache: Option<ZoneCache>,
}

/// A handle taken hold of.
#[derive(Clone, Debug)]
struct HandleDrag {
    zone: AssetId,
    drag: Drag,
    /// What its steps are called in the history.
    label: String,
    /// The zone and its colliders at the grab, which each frame's colliders are written from.
    held: Option<Arc<Held>>,
}

impl ZonePanel {
    /// A drag the viewport lost its zone or its camera under: ended as [`Command::End`] ends one, so its step
    /// closes where it is, rather than left open for the next edit of anything to join.
    fn lost_view(&mut self, asked: &mut Vec<Command>) {
        if self.held.take().is_some() || self.drag.is_some() {
            asked.push(Command::End);
        }
        self.boxing = None;
    }

    /// Makes the objects and their index again if `zone`'s document has another revision since, or is another.
    fn follow(&mut self, documents: &DocumentSet, zone: Option<AssetId>) {
        let Some((id, document)) = zone.and_then(|id| Some((id, documents.get::<ZoneDocument>(id)?))) else {
            self.cache = None;
            return;
        };
        let fresh = |cache: &ZoneCache| cache.document == id && cache.revision == document.revision();
        if self.cache.as_ref().is_some_and(fresh) {
            return;
        }
        self.cache = document.last_good().map(|value| ZoneCache::of(id, document.revision(), value));
    }

    /// The gizmo's object in `zone`, of `documents`, from `selection`.
    fn gripped(&self, documents: &DocumentSet, zone: AssetId, selection: &[ObjectId]) -> Option<Gripped> {
        let value = documents.get::<ZoneDocument>(zone)?.value()?;
        self.cache.as_ref().filter(|cache| cache.document == zone)?.gripped(value, selection)
    }

    /// The gizmo on `gripped`, as `camera` sees it, with the handle under the pointer or held lit.
    fn gizmo_of(&self, gripped: &Gripped, camera: &ViewCamera) -> Gizmo {
        let hot = self
            .drag
            .as_ref()
            .map(|drag| drag.drag.handle)
            .or(self.held.map(|(handle, _)| handle))
            .or(self.hot);
        Gizmo { hot, ..Gizmo::of(&gripped.grip, self.mode, self.axes, camera) }
    }

    /// The pointer's share of the viewport, before the view takes what's left: a press on a handle takes hold of
    /// it, which drags it until the press ends; a Shift-drag draws a box, which selects what it holds; a click
    /// elsewhere picks what's under it. What it does is commands, in `asked`, for the editor to run once the
    /// viewport has drawn. Says whether it took the pointer.
    #[allow(
        clippy::too_many_arguments,
        reason = "the viewport's pointer and camera, and what the editor holds"
    )]
    fn pointer(
        &mut self,
        documents: &DocumentSet,
        zone: Option<AssetId>,
        selection: &[ObjectId],
        response: &egui::Response,
        camera: Option<&ViewCamera>,
        rect: egui::Rect,
        asked: &mut Vec<Command>,
    ) -> bool {
        self.hot = None;
        let (Some(zone), Some(camera)) = (zone, camera) else {
            self.lost_view(asked);
            return false;
        };
        let Some(document) = documents.get::<ZoneDocument>(zone) else { return false };
        let pointer = response.ctx.input(|input| input.pointer.clone());
        let modifiers = response.ctx.input(|input| input.modifiers);
        if let Some(taken) = self.draw_box(&pointer, modifiers, camera, rect, asked) {
            return taken;
        }
        let gizmo = self.gripped(documents, zone, selection).map(|gripped| self.gizmo_of(&gripped, camera));
        if self.held.is_none()
            && pointer.primary_pressed()
            && let Some(gizmo) = &gizmo
            && let Some(at) = pointer.press_origin().filter(|at| rect.contains(*at))
            && let Some(handle) = gizmo.pick(camera, rect, at)
        {
            let ray = camera.ray(at, rect);
            asked.push(Command::Grab {
                handle,
                from: ray.origin.to_array(),
                toward: ray.direction.to_array(),
            });
            self.held = Some((handle, at));
            return true;
        }
        let Some((handle, sent)) = self.held else {
            self.hot =
                pointer.hover_pos().zip(gizmo.as_ref()).and_then(|(at, gizmo)| gizmo.pick(camera, rect, at));
            if self.hot.is_none() {
                click_to_pick(response, modifiers, camera, rect, asked);
            }
            return false;
        };
        // A press the editor didn't take (the grab was refused) is no drag.
        if self.drag.is_none() && !asked.iter().any(|command| matches!(command, Command::Grab { .. })) {
            self.held = None;
            return false;
        }
        if !pointer.primary_down() {
            asked.push(Command::End);
            self.held = None;
            return true;
        }
        // A frame where it hasn't moved asks nothing, unless an undo or a save ended its gesture under it: then the
        // rest of the drag is a step of its own.
        if let Some(at) = pointer.interact_pos()
            && (at != sent || !document.in_gesture())
        {
            let ray = camera.ray(at, rect);
            asked.push(Command::DragTo { from: ray.origin.to_array(), toward: ray.direction.to_array() });
            self.held = Some((handle, at));
        }
        true
    }

    /// A box drawn with Shift held from a press in the pane, until its release selects what it holds (beside what's
    /// selected with Cmd held too): `Some` whether it took the pointer while one's under way, `None` otherwise. A
    /// release before the pointer's gone [`BOX_START`] leaves it to the click.
    fn draw_box(
        &mut self,
        pointer: &egui::PointerState,
        modifiers: egui::Modifiers,
        camera: &ViewCamera,
        rect: egui::Rect,
        asked: &mut Vec<Command>,
    ) -> Option<bool> {
        if self.held.is_none()
            && self.boxing.is_none()
            && pointer.primary_pressed()
            && modifiers.shift
            && let Some(at) = pointer.press_origin().filter(|at| rect.contains(*at))
        {
            self.boxing = Some((at, at));
        }
        let (from, _) = self.boxing?;
        let now = pointer.interact_pos().or_else(|| pointer.hover_pos()).unwrap_or(from);
        let drawn = from.distance(now) >= BOX_START;
        if pointer.primary_down() {
            self.boxing = Some((from, now));
            return Some(drawn);
        }
        self.boxing = None;
        if !drawn {
            return None;
        }
        let (from, to) = (ViewCamera::ndc(from, rect), ViewCamera::ndc(now, rect));
        asked.push(Command::SelectInBox {
            view: camera.view_projection().to_cols_array(),
            from: from.to_array(),
            to: to.to_array(),
            adding: modifiers.command,
        });
        Some(true)
    }

    /// The box being drawn, if one's far enough along to be one.
    pub(super) fn drawn_box(&self) -> Option<egui::Rect> {
        let (from, to) = self.boxing?;
        (from.distance(to) >= BOX_START).then(|| egui::Rect::from_two_pos(from, to))
    }

    /// Lets go of the handle held: the drag's gesture has ended, or been taken back.
    pub(super) fn let_go(&mut self) {
        self.drag = None;
    }
}

/// A click off the gizmo and not a box's end picks what's under it, beside what's selected with Cmd or Shift
/// held.
fn click_to_pick(
    response: &egui::Response,
    modifiers: egui::Modifiers,
    camera: &ViewCamera,
    rect: egui::Rect,
    asked: &mut Vec<Command>,
) {
    let (true, Some(at)) = (response.clicked(), response.interact_pointer_pos()) else { return };
    let ray = camera.ray(at, rect);
    asked.push(Command::PickInZone {
        from: ray.origin.to_array(),
        toward: ray.direction.to_array(),
        reach: camera.reach(at, rect, CLICK_REACH),
        adding: modifiers.command || modifiers.shift,
    });
}

/// What the viewport shows of `current`, if it's a zone that's read, with the gizmo on the object of `selection`
/// selected last and its objects marked, those in `selection` as selected.
pub(super) fn zone_shown<'a>(
    documents: &'a DocumentSet,
    panel: &ZonePanel,
    current: Option<AssetId>,
    selection: &[ObjectId],
    camera: Option<&ViewCamera>,
) -> Option<ZoneShown<'a>> {
    let id = current?;
    let file = documents.get::<ZoneDocument>(id)?;
    let stem = documents.path(id)?.file_name()?.to_str()?.strip_suffix(".zone.ron")?;
    let gizmo = camera.and_then(|camera| {
        let gripped = panel.gripped(documents, id, selection)?;
        Some(panel.gizmo_of(&gripped, camera))
    });
    let marks = (panel.cache.as_ref())
        .filter(|cache| cache.document == id)
        .map(|cache| ZoneMarks { objects: Arc::clone(&cache.objects), selected: cache.selected(selection) });
    Some(ZoneShown { id, stem, document: file.last_good()?, gizmo, marks })
}

/// A ray a command carries, or `None` if it isn't numbers.
fn ray_of(from: [f32; 3], toward: [f32; 3]) -> Option<Ray> {
    let (origin, direction) = (Vec3::from(from), Vec3::from(toward));
    (origin.is_finite() && direction.is_finite())
        .then(|| direction.try_normalize())
        .flatten()
        .map(|direction| Ray { origin, direction })
}

/// The number at `path` of `zone`, if it's one.
fn number_at(zone: &ZoneDocument, path: &PropertyPath) -> Option<f32> {
    walk::get(zone, path).ok()?.try_downcast_ref::<f32>().copied()
}

impl Editor {
    /// The current document, if it's a zone.
    fn current_zone(&self) -> Option<AssetId> {
        let id = self.current?;
        self.documents.get::<ZoneDocument>(id).map(|_| id)
    }

    /// Makes the current zone's objects and their index again if its document has changed since.
    pub(super) fn follow_zone(&mut self) {
        let zone = self.current_zone();
        self.zone.follow(&self.documents, zone);
    }

    /// The current zone's objects, as the viewport marks them and a pick finds them, made again only when its
    /// document changes.
    pub fn zone_objects(&mut self) -> Option<Arc<ZoneObjects>> {
        self.follow_zone();
        self.zone.cache.as_ref().map(|cache| Arc::clone(&cache.objects))
    }

    /// What making the current zone's pick index put in it: its cells and handles.
    pub fn zone_index_built(&mut self) -> Option<Visits> {
        self.follow_zone();
        self.zone.cache.as_ref().map(|cache| cache.built)
    }

    /// How many rows the zone pane lists for the current zone, a heading for each kind it holds among them: what it
    /// scrolls through, building only those that show.
    pub fn zone_rows(&mut self) -> Option<usize> {
        self.follow_zone();
        self.zone.cache.as_ref().map(|cache| cache.rows.len())
    }

    /// The viewport's pointer, for the gizmo first, and the box being drawn over it.
    pub(super) fn viewport_pane(&mut self, ui: &mut Ui) {
        self.follow_zone();
        let zone = self.current_zone();
        let (documents, panel, asked, selection) =
            (&self.documents, &mut self.zone, &mut self.asked, &self.selection);
        self.viewport.ui(ui, |response, camera, rect| {
            panel.pointer(documents, zone, selection, response, camera, rect, asked)
        });
        if let Some(drawn) = self.zone.drawn_box() {
            let stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            ui.painter().rect_stroke(drawn, 0.0, stroke, egui::StrokeKind::Middle);
        }
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

    /// Selects what a ray from `from` toward `toward` picks in the current zone: what [`Command::PickInZone`] does.
    /// Refused for a ray or a reach that isn't a number; a reach is at most [`crate::pick::MAX_REACH`].
    pub(super) fn pick_in_zone(&mut self, from: [f32; 3], toward: [f32; 3], reach: f32, adding: bool) {
        let Some(zone) = self.current_zone() else {
            self.refuse("the current document isn't a zone");
            return;
        };
        if !from.iter().chain(&toward).all(|value| value.is_finite()) || !reach.is_finite() {
            self.refuse("a pick's ray and reach must be numbers");
            return;
        }
        self.follow_zone();
        let Some(cache) = &self.zone.cache else {
            self.refuse("the zone doesn't read, so nothing in it can be picked");
            return;
        };
        let pick = Pick { ray: Ray { origin: Vec3::from(from), direction: Vec3::from(toward) }, reach };
        let (picked, _) = cache.index.pick(&pick);
        let picked: Vec<ObjectId> = picked
            .map(|at| ObjectId { document: zone, path: cache.objects.objects[at].path.clone() })
            .into_iter()
            .collect();
        self.choose(picked, adding);
    }

    /// Selects what a box holds in the current zone: what [`Command::SelectInBox`] does.
    pub(super) fn select_in_box(&mut self, view: &[f32; 16], from: [f32; 2], to: [f32; 2], adding: bool) {
        let Some(zone) = self.current_zone() else {
            self.refuse("the current document isn't a zone");
            return;
        };
        if !view.iter().chain(&from).chain(&to).all(|value| value.is_finite()) {
            self.refuse("a box's view and corners must be numbers");
            return;
        }
        self.follow_zone();
        let Some(cache) = &self.zone.cache else {
            self.refuse("the zone doesn't read, so nothing in it can be picked");
            return;
        };
        let (held, _) = cache.index.in_box(Mat4::from_cols_array(view), Vec2::from(from), Vec2::from(to));
        let held: Vec<ObjectId> = held
            .into_iter()
            .map(|at| ObjectId { document: zone, path: cache.objects.objects[at].path.clone() })
            .collect();
        self.choose(held, adding);
    }

    /// `picked` in place of what's selected, or, `adding`, each beside it, or taken out if it was in.
    fn choose(&mut self, picked: Vec<ObjectId>, adding: bool) {
        if !adding {
            self.selection = picked;
            return;
        }
        for object in picked {
            match self.selection.iter().position(|selected| *selected == object) {
                Some(at) => {
                    self.selection.remove(at);
                }
                None => self.selection.push(object),
            }
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

    /// The placement of the current zone selected last.
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

    /// The pane: each kind of the zone's objects under its name and count, a row an object, of which only the rows
    /// that show are built.
    pub(super) fn zone_panel(&mut self, ui: &mut Ui) {
        let Some(id) = self.current_zone() else {
            ui.label("Open a zone to place what's in it.");
            return;
        };
        self.follow_zone();
        self.zone_bar(ui);
        let Some(document) = self.documents.get::<ZoneDocument>(id).and_then(Document::value) else {
            ui.label("The zone doesn't read, so nothing here moves until it does.");
            return;
        };
        let Some(cache) = self.zone.cache.as_ref().filter(|cache| cache.document == id) else { return };
        if document.has_split() {
            ui.label(
                RichText::new("A split placement cooks into the cells alone: the viewport draws this zone once it's cooked without one.")
                    .weak(),
            );
        }
        let rows = &cache.rows;
        let selected = cache.selected(&self.selection);
        let name_of = |prefab: AssetId| {
            let source = self.documents.sidecars().source_of(prefab);
            let name = source.as_deref().and_then(|path| path.file_name()).and_then(|name| name.to_str());
            name.map_or_else(|| prefab.to_string(), |name| name.trim_end_matches(".assembly.ron").to_owned())
        };
        let height = ui.text_style_height(&egui::TextStyle::Body).max(ui.spacing().interact_size.y);
        let mut choose = None;
        egui::ScrollArea::vertical().id_salt("zone").auto_shrink([false, false]).show_rows(
            ui,
            height,
            rows.len(),
            |ui, shown| {
                for row in &rows[shown] {
                    let at = match *row {
                        Row::Kind(kind, count) => {
                            ui.label(RichText::new(format!("{}  {count}", kind.plural())).strong());
                            continue;
                        }
                        Row::Object(at) => at,
                    };
                    let object = &cache.objects.objects[at];
                    let what = match object.kind {
                        ObjectKind::Placement => document.placements.get(object.index).map(|placement| {
                            let name = name_of(placement.prefab);
                            if placement.backdrop { format!("{name} (backdrop)") } else { name }
                        }),
                        _ => None,
                    };
                    let place = object.at().map_or_else(
                        || "(no place)".to_owned(),
                        |at| format!("({:.2}, {:.2}, {:.2})", at.x, at.y, at.z),
                    );
                    let text = match what {
                        Some(what) => format!("{}  {}  {what}  {place}", object.index, object.key),
                        None => format!("{}  {}  {place}", object.index, object.key),
                    };
                    if ui.selectable_label(selected.binary_search(&at).is_ok(), text).clicked() {
                        // By the release's own modifiers, which a click has whatever the keys' state says by then.
                        let adding = ui.input(|input| {
                            input.events.iter().any(|event| {
                                matches!(event, egui::Event::PointerButton { pressed: false, modifiers, .. }
                                    if modifiers.command || modifiers.shift)
                            })
                        });
                        choose = Some((object.kind, object.key.clone(), object.path.clone(), adding));
                    }
                }
            },
        );
        match choose {
            Some((_, _, path, true)) => self.toggle_object(id, path),
            Some((ObjectKind::Placement, key, _, false)) => self.act(Command::SelectPlacement(Some(key))),
            Some((_, _, path, false)) => {
                let picked = self.picked(&[ObjectId { document: id, path }]);
                self.act(Command::SelectObjects(picked));
            }
            None => {}
        }
    }
}

/// A row of the pane: a kind's heading, with how many of it there are, or an object, by its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Kind(ObjectKind, usize),
    Object(usize),
}

/// The pane's rows for `objects`: each kind there's one of, and its objects under it.
fn rows(objects: &ZoneObjects) -> Vec<Row> {
    let mut rows = Vec::with_capacity(objects.objects.len() + ObjectKind::ALL.len());
    let mut at = 0;
    while let Some(first) = objects.objects.get(at) {
        let count = objects.objects[at..].iter().take_while(|object| object.kind == first.kind).count();
        rows.push(Row::Kind(first.kind, count));
        rows.extend((at..at + count).map(Row::Object));
        at += count;
    }
    rows
}

/// What the history calls a snap to the ground.
const GROUND_LABEL: &str = "snap to the ground";

impl Editor {
    /// The gizmo's mode, its arrows' axes and the steps a drag snaps to.
    #[must_use]
    pub fn gizmo_settings(&self) -> (GizmoMode, Axes, Steps) {
        (self.zone.mode, self.zone.axes, self.zone.steps)
    }

    /// The gizmo on the current zone's object selected last, as `camera` sees it: where a hand finds its handles.
    pub fn gizmo_seen(&mut self, camera: &ViewCamera) -> Option<Gizmo> {
        let zone = self.current_zone()?;
        self.follow_zone();
        let gripped = self.zone.gripped(&self.documents, zone, &self.selection)?;
        Some(self.zone.gizmo_of(&gripped, camera))
    }

    /// What [`Command::GizmoMode`] does.
    pub(super) fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        self.zone.mode = mode;
    }

    /// What [`Command::GizmoAxes`] does.
    pub(super) fn set_gizmo_axes(&mut self, axes: Axes) {
        self.zone.axes = axes;
    }

    /// What [`Command::Snap`] does: refused for a step that isn't a number of 0 or more.
    pub(super) fn set_steps(&mut self, grid: f32, yaw: f32) {
        let steps = Steps { grid, yaw };
        if !steps.are_steps() {
            self.refuse("a step is a number, 0 or more");
            return;
        }
        self.zone.steps = steps;
    }

    /// Takes hold of `handle` of the gizmo where a ray from `from` toward `toward` finds it, and starts the drag's
    /// gesture: what [`Command::Grab`] does.
    pub(super) fn grab(&mut self, handle: Handle, from: [f32; 3], toward: [f32; 3]) {
        let Some(zone) = self.current_zone() else {
            self.refuse("the current document isn't a zone");
            return;
        };
        let Some(ray) = ray_of(from, toward) else {
            self.refuse("a grab's ray must be numbers, and go somewhere");
            return;
        };
        if !self.take_typed() {
            return;
        }
        self.follow_zone();
        let Some(gripped) = self.zone.gripped(&self.documents, zone, &self.selection) else {
            self.refuse("nothing selected in the zone has a handle");
            return;
        };
        let mode = self.zone.mode;
        if !gripped.grip.handles(mode).contains(&handle) {
            let what = gripped.kind.singular();
            self.refuse(format!("{what} has no {handle:?} handle in the gizmo's {} mode", mode.name()));
            return;
        }
        let Some(drag) = Drag::start(gripped.grip, handle, self.zone.axes, ray) else {
            self.refuse("the ray doesn't say where on the handle it is: it runs along it");
            return;
        };
        let label = format!("{} {}", handle.verb(), gripped.kind.singular());
        self.begin_gesture_on(zone, &label);
        let held = sapphire_document::zone::hold(&mut self.documents, zone).map(Arc::new);
        self.zone.drag = Some(HandleDrag { zone, drag, label, held });
    }

    /// Drags the handle held to where a ray from `from` toward `toward` points, setting each number it changes:
    /// what [`Command::DragTo`] does. A ray that can't say where leaves everything as it is.
    pub(super) fn drag_to(&mut self, from: [f32; 3], toward: [f32; 3]) {
        let Some(held) = self.zone.drag.clone() else {
            self.refuse("no handle is held: Grab one first");
            return;
        };
        let Some(ray) = ray_of(from, toward) else {
            self.refuse("a drag's ray must be numbers, and go somewhere");
            return;
        };
        if self.current_zone() != Some(held.zone) {
            self.refuse("the handle held is of a zone that isn't the current document");
            return;
        }
        let Some(numbers) = held.drag.to(ray, self.zone.steps) else { return };
        let Some(document) = self.documents.get::<ZoneDocument>(held.zone) else { return };
        let Some(value) = document.value() else {
            self.refuse("the zone doesn't read, so nothing in it moves until it does");
            return;
        };
        let changed = changed_numbers(value, &numbers, document.registry());
        if changed.is_empty() {
            return;
        }
        // An undo or a save while it's held ends its gesture, and the rest of it is one of its own.
        if !document.in_gesture() {
            self.begin_gesture_on(held.zone, &held.label);
        }
        let result = self.spell_each(held.zone, &held.label, held.held.as_deref(), &changed);
        self.after_property_edit(Some(held.zone), result.map_err(|error| error.0));
    }

    /// The keyed paths of the current zone's entries selected, or a refusal naming what isn't one.
    fn selected_entries(&mut self, what: &str) -> Option<(AssetId, Vec<PropertyPath>)> {
        let Some(zone) = self.current_zone() else {
            self.refuse("the current document isn't a zone");
            return None;
        };
        let entries: Vec<PropertyPath> = self
            .selection
            .iter()
            .filter(|object| object.document == zone)
            .map(|object| object.path.clone())
            .collect();
        if entries.is_empty() {
            self.refuse(format!("nothing in the zone is selected to {what}"));
            return None;
        }
        if let Some(part) =
            entries.iter().find(|path| !matches!(path.steps().last(), Some(sapphire_document::Step::Key(_))))
        {
            self.refuse(format!("`{part}` is a part of an entry: select the entry to {what} it"));
            return None;
        }
        Some((zone, entries))
    }

    /// Copies each entry of the current zone selected, and selects the copies: what [`Command::Duplicate`] and
    /// [`Command::DuplicateAs`] do, the copies of entries with IDs taking `ids` in order, then fresh ones.
    pub(super) fn duplicate(&mut self, ids: &[String]) {
        if !self.take_typed() {
            return;
        }
        let Some((zone, entries)) = self.selected_entries("duplicate") else { return };
        match sapphire_document::zone::duplicate(&mut self.documents, zone, "duplicate", &entries, ids) {
            Ok(made) => {
                if ids.is_empty() {
                    self.drawn_ids = made.ids;
                }
                self.selection =
                    made.paths.into_iter().map(|path| ObjectId { document: zone, path }).collect();
                self.after_property_edit(Some(zone), Ok(()));
            }
            Err(error) => self.refuse(error.to_string()),
        }
    }

    /// Takes out each entry of the current zone selected, and the colliders only they name: what
    /// [`Command::DeleteObjects`] does.
    pub(super) fn delete_objects(&mut self) {
        if !self.take_typed() {
            return;
        }
        let Some((zone, entries)) = self.selected_entries("delete") else { return };
        match sapphire_document::zone::remove(&mut self.documents, zone, "delete", &entries) {
            Ok(()) => {
                self.selection.retain(|object| object.document != zone || !entries.contains(&object.path));
                self.after_property_edit(Some(zone), Ok(()));
            }
            Err(error) => self.refuse(error.to_string()),
        }
    }

    /// Puts each object of the current zone selected that has a position on the zone's colliders straight below
    /// it, as one step: what [`Command::SnapToGround`] does ([`crate::ground`]). Refused if the zone names no
    /// colliders, or nothing is below one of them.
    pub(super) fn snap_to_ground(&mut self) {
        let Some(zone) = self.current_zone() else {
            self.refuse("the current document isn't a zone");
            return;
        };
        if !self.take_typed() {
            return;
        }
        self.follow_zone();
        let colliders = match sapphire_document::zone::colliders_of(&mut self.documents, zone) {
            Ok(Some(colliders)) => colliders,
            Ok(None) => return self.refuse("the zone names no colliders to stand on"),
            Err(error) => return self.refuse(error.to_string()),
        };
        let changed = match self.grounded(zone, colliders) {
            Ok(changed) => changed,
            Err(why) => return self.refuse(why),
        };
        if changed.is_empty() {
            return;
        }
        let result = self.spell_each(zone, GROUND_LABEL, None, &changed);
        self.after_property_edit(Some(zone), result.map_err(|error| error.0));
    }

    /// The numbers that put each object of `zone` selected that has a position on the colliders of the document
    /// `colliders` below it, those that aren't there already, or why they can't.
    fn grounded(&self, zone: AssetId, colliders: AssetId) -> Result<Vec<(PropertyPath, String)>, String> {
        let ground = self.documents.get::<ColliderDocument>(colliders).and_then(Document::value);
        let ground = ground.ok_or("the zone's colliders don't read")?;
        let document = self.documents.get::<ZoneDocument>(zone).ok_or("the current document isn't a zone")?;
        let unread = "the zone doesn't read, so nothing in it moves until it does";
        let (value, cache) = (document.value().ok_or(unread)?, self.zone.cache.as_ref().ok_or(unread)?);
        let mut numbers = Vec::new();
        for object in self.selection.iter().filter(|object| object.document == zone) {
            let Some(found) = cache.by_path.get(&object.path).map(|&at| &cache.objects.objects[at]) else {
                continue;
            };
            let Some(position) = Grip::of(value, found).and_then(|grip| grip.position) else { continue };
            // What stands for it isn't what it stands on.
            let own = match found.kind {
                ObjectKind::Placement => {
                    value.placements.get(found.index).map(|placement| &placement.colliders)
                }
                ObjectKind::Instance => value.instances.get(found.index).map(|instance| &instance.colliders),
                _ => None,
            };
            let ground = Ground::of(ground, own.map_or(&[][..], Vec::as_slice))
                .map_err(|error| format!("the zone's colliders don't cook: {error}"))?;
            let hit = ground
                .below(position.value)
                .ok_or_else(|| format!("nothing of the zone's colliders is below `{}`", object.path))?;
            numbers.push(Number { path: position.number(1), value: hit.y });
        }
        if numbers.is_empty() {
            return Err("nothing selected in the zone has a position to snap".to_owned());
        }
        Ok(changed_numbers(value, &numbers, document.registry()))
    }

    /// The pane's bar: the gizmo's mode and axes, the steps, and what's done to what's selected.
    fn zone_bar(&mut self, ui: &mut Ui) {
        let (mode, axes, steps) = self.gizmo_settings();
        let mut asked = Vec::new();
        ui.horizontal_wrapped(|ui| {
            for each in GizmoMode::ALL {
                if ui.selectable_label(mode == each, each.name()).clicked() && mode != each {
                    asked.push(Command::GizmoMode(each));
                }
            }
            ui.separator();
            for (each, name) in [(Axes::World, "World"), (Axes::Local, "Local")] {
                if ui.selectable_label(axes == each, name).clicked() && axes != each {
                    asked.push(Command::GizmoAxes(each));
                }
            }
            ui.separator();
            let (mut grid, mut yaw) = (steps.grid, steps.yaw);
            ui.label("Grid");
            let grid_changed =
                ui.add(egui::DragValue::new(&mut grid).range(0.0..=100.0).speed(0.01)).changed();
            ui.label("Yaw");
            let yaw_changed = ui.add(egui::DragValue::new(&mut yaw).range(0.0..=180.0).speed(0.5)).changed();
            if grid_changed || yaw_changed {
                asked.push(Command::Snap { grid, yaw });
            }
            ui.separator();
            if ui.button("Snap to ground").clicked() {
                asked.push(Command::SnapToGround);
            }
            if ui.button("Duplicate").clicked() {
                asked.push(Command::Duplicate);
            }
            if ui.button("Delete").clicked() {
                asked.push(Command::DeleteObjects);
            }
        });
        for command in asked {
            self.act(command);
        }
    }
}

/// Those of `numbers` that `zone` doesn't hold already, each spelled as the file would write it.
fn changed_numbers(
    zone: &ZoneDocument,
    numbers: &[Number],
    registry: &bevy_reflect::TypeRegistry,
) -> Vec<(PropertyPath, String)> {
    numbers
        .iter()
        .filter(|number| {
            number_at(zone, &number.path).is_none_or(|now| now.to_bits() != number.value.to_bits())
        })
        .filter_map(|number| Some((number.path.clone(), write::canonical(&number.value, registry).ok()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drag the viewport loses its zone or camera under is ended as an `End` ends one, so its step closes there.
    #[test]
    fn a_drag_the_view_loses_is_ended() {
        let mut panel = ZonePanel { held: Some((Handle::Yaw, egui::pos2(1.0, 2.0))), ..ZonePanel::default() };
        let mut asked = Vec::new();
        panel.lost_view(&mut asked);
        assert!(matches!(asked[..], [Command::End]), "{asked:?}");
        assert!(panel.held.is_none());
        panel.lost_view(&mut asked);
        assert_eq!(asked.len(), 1, "nothing held, nothing to end");
    }
}
