//! The zone pane (M3 E5): the current zone document's objects, those selected, and the gizmo that moves the last
//! placement selected in the viewport. A Cmd or Shift click adds a row to the selection or takes it out (M6 T2),
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
//! the placement, of the same order as the document's own read of each set (T12 prices both).
//!
//! The viewport draws a zone as an editor needs it, each placement on its own ([`crate::viewport`]), and the
//! gizmo on the placement selected last. A drag of an arrow is one gesture on the document: each frame of it sets
//! the one number of the placement's `at` that the arrow moves, snapped to [`SNAP`] so the file reads as a person
//! would write it, and undo takes the whole drag back to the bytes it started from. What the zone's checks refuse
//! stays as it was. A placement that names colliders takes them with it, so the drag is one step of the zone and
//! its colliders' document, which undo takes back whole. Everything else about an object is the Properties pane's.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{RichText, Ui};
use sapphire_assets::AssetId;
use sapphire_document::{Document, DocumentSet, ObjectId, PropertyPath, Step, write};
use sapphire_math::{Mat4, Vec2, Vec3};
use sapphire_zone::{IdList, ZoneDocument};

use super::{Editor, Notice, selection};
use crate::commands::Command;
use crate::gizmo::{Axis, Gizmo, Ray, ViewCamera, along};
use crate::markers::{ObjectKind, ZoneObjects};
use crate::pick::{Pick, PickIndex, Visits};
use crate::viewport::{ZoneMarks, ZoneShown};

/// What a drag rounds a placement's position to, in metres.
pub const SNAP: f32 = 0.05;

/// What a drag of the gizmo is called in the history.
const MOVE_LABEL: &str = "move a placement";

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
}

/// The pane's state: what the pointer's doing to the gizmo, and what's made of the zone. What's selected is the
/// editor's selection.
#[derive(Debug, Default)]
pub(super) struct ZonePanel {
    drag: Option<GizmoDrag>,
    /// The arrow under the pointer.
    hot: Option<Axis>,
    /// Where a box being drawn started, and where the pointer is now, in the pane.
    boxing: Option<(egui::Pos2, egui::Pos2)>,
    cache: Option<ZoneCache>,
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

impl ZonePanel {
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

    /// The pointer's share of the viewport, before the view takes what's left: a press on an arrow starts a drag
    /// of it, which moves the placement until the press ends; a Shift-drag draws a box, which selects what it holds;
    /// a click elsewhere picks what's under it. What it does is commands, in `asked`, for the editor to run once the
    /// viewport has drawn. Says whether it took the pointer.
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
            self.boxing = None;
            return false;
        };
        let Some(document) = documents.get::<ZoneDocument>(zone) else { return false };
        let pointer = response.ctx.input(|input| input.pointer.clone());
        let modifiers = response.ctx.input(|input| input.modifiers);
        if let Some(taken) = self.draw_box(&pointer, modifiers, camera, rect, asked) {
            return taken;
        }
        let Some(gizmo) = self.gizmo_on(document, selected, camera) else {
            self.drag = None;
            click_to_pick(response, modifiers, camera, rect, asked);
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
                click_to_pick(response, modifiers, camera, rect, asked);
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
        if self.drag.is_none()
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

/// The path of a zone's placements.
fn placements() -> PropertyPath {
    PropertyPath::root().join(Step::Field("placements".to_owned()))
}

/// The path of one number of a placement's position.
fn placement_at(placement: &PropertyPath, axis: Axis) -> PropertyPath {
    placement.join(Step::Field("at".to_owned())).join(Step::Index(axis.index()))
}

/// What the viewport shows of `current`, if it's a zone that's read, with the gizmo on placement `selected` and
/// its objects marked, those in `selection` as selected.
pub(super) fn zone_shown<'a>(
    documents: &'a DocumentSet,
    panel: &ZonePanel,
    current: Option<AssetId>,
    selected: Option<usize>,
    selection: &[ObjectId],
    camera: Option<&ViewCamera>,
) -> Option<ZoneShown<'a>> {
    let id = current?;
    let file = documents.get::<ZoneDocument>(id)?;
    let stem = documents.path(id)?.file_name()?.to_str()?.strip_suffix(".zone.ron")?;
    let gizmo = camera.and_then(|camera| panel.gizmo_on(file, selected, camera));
    let marks = (panel.cache.as_ref())
        .filter(|cache| cache.document == id)
        .map(|cache| ZoneMarks { objects: Arc::clone(&cache.objects), selected: cache.selected(selection) });
    Some(ZoneShown { id, stem, document: file.last_good()?, gizmo, marks })
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
        let (zone, selected) = (self.current_zone(), self.selected_placement());
        let (documents, panel, asked) = (&self.documents, &mut self.zone, &mut self.asked);
        self.viewport.ui(ui, |response, camera, rect| {
            panel.gizmo(documents, zone, selected, response, camera, rect, asked)
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

    /// The pane: each kind of the zone's objects under its name and count, a row an object, of which only the rows
    /// that show are built.
    pub(super) fn zone_panel(&mut self, ui: &mut Ui) {
        let Some(id) = self.current_zone() else {
            ui.label("Open a zone to place what's in it.");
            return;
        };
        self.follow_zone();
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
