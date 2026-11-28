//! The look pane (M6 T15a): the current day or light rig library on the look-dev stage a sandbox capture stands on
//! (`sapphire_stage::lookdev::stand`), which the viewport draws ([`crate::viewport`]), from a shot, at a tier and an
//! hour, as saved (A) or as edited (B); and the document's own handles: a day's keys on its curves, dragged along
//! the day, and the picked key's numbers; a library's presets, their key light's direction on a dial and their
//! numbers.
//!
//! Every edit is the document's, by [`Command::Begin`], [`Command::SetProperty`] and [`Command::End`]: one step of
//! undo a drag, saved to its own file, and what a `--run` script replays. How the stage shows is the pane's own,
//! by [`Command::LookShot`], [`Command::LookTier`], [`Command::LookHour`] and [`Command::Compare`]. Nothing cooks:
//! the stage takes the value as it reads now.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use sapphire_assets::AssetId;
use sapphire_render::quality::TierId;
use sapphire_stage::scene::TownLook;
use sapphire_tod::light_rig::{LightRigId, LightRigLibrary};
use sapphire_tod::{DAY_HOURS, TodKey, TodTimeline};

use super::Editor;
use crate::commands::Command;
use crate::look::{
    CHANNELS, Channel, LookShot, LookShown, LookView, SAMPLES, Side, Source, curve, dragged_time,
};

/// The pane's state: how the stage shows, what it shows, and what a person has hold of.
#[derive(Debug, Default)]
pub(super) struct LookPanel {
    pub(super) view: LookView,
    /// The town's own look, read once: what a document's is laid over.
    town: Option<TownLook>,
    /// What the stage shows of the current document, read again only when what it's read from changes.
    shown: Option<Shown>,
    /// The last look that read, which the stage stays up on while a side doesn't read.
    last_read: Option<Arc<TownLook>>,
    /// The day's key last picked, by its place.
    key: Option<usize>,
    /// The key being dragged along the day, by its place.
    dragging: Option<usize>,
    /// The preset whose key light the dial turns.
    rig: usize,
}

/// The town's look with a document in it, and what it was read from.
#[derive(Debug)]
struct Shown {
    id: AssetId,
    side: Side,
    /// The document's text as edited (B), or its file's bytes as saved (A).
    from: Vec<u8>,
    look: Result<Arc<TownLook>, String>,
}

/// What a drag along the day and a turn of the dial are, in the history.
const MOVE_KEY: &str = "move a key of the day";
const TURN_KEY_LIGHT: &str = "turn a rig's key light";

/// A day or a library, as read.
#[derive(Clone, Debug)]
enum Read {
    Day(TodTimeline),
    LightRigs(LightRigLibrary),
}

impl Read {
    const fn source(&self) -> Source<'_> {
        match self {
            Self::Day(day) => Source::Day(day),
            Self::LightRigs(rigs) => Source::LightRigs(rigs),
        }
    }
}

impl Editor {
    /// How the look pane shows the stage.
    #[must_use]
    pub fn look_view(&self) -> LookView {
        self.look.view
    }

    /// The current document, if it's a day or a light rig library, as it reads now.
    fn current_look(&self) -> Option<(AssetId, Read)> {
        let id = self.current?;
        if let Some(day) = self.documents.get::<TodTimeline>(id) {
            return Some((id, Read::Day(day.last_good()?.clone())));
        }
        let rigs = self.documents.get::<LightRigLibrary>(id)?;
        Some((id, Read::LightRigs(rigs.last_good()?.clone())))
    }

    /// What the viewport shows of the current document, if it's a day or a light rig library that reads: its side
    /// of the A/B, on the look-dev town, at the pane's view. It's read again only when the document's text (B) or its
    /// file's bytes (A) change, and the town's own look under it only once.
    pub(super) fn look_shown(&mut self) -> Option<LookShown> {
        // A tier the stage couldn't build: the picker goes back to the one that draws, and the pane says why.
        if let Some((_, active)) = self.viewport.take_look_refused() {
            self.look.view.tier = active;
        }
        let id = self.current?;
        let view = self.look.view;
        let side = view.side;
        let day = self.documents.get::<TodTimeline>(id);
        let rigs = self.documents.get::<LightRigLibrary>(id);
        let from: &[u8] = match (day, rigs, side) {
            (Some(day), _, Side::Edited) => day.text().as_bytes(),
            (Some(day), _, Side::Saved) => day.disk(),
            (None, Some(rigs), Side::Edited) => rigs.text().as_bytes(),
            (None, Some(rigs), Side::Saved) => rigs.disk(),
            (None, None, _) => return None,
        };
        let panel = &mut self.look;
        let fresh = panel
            .shown
            .as_ref()
            .is_some_and(|shown| (shown.id, shown.side) == (id, side) && shown.from == from);
        if !fresh {
            let read = match side {
                Side::Edited => {
                    let edited = match (day, rigs) {
                        (Some(day), _) => day.last_good().cloned().map(Read::Day),
                        (None, rigs) => rigs.and_then(|rigs| rigs.last_good().cloned()).map(Read::LightRigs),
                    };
                    edited.ok_or_else(|| "the document has never read".to_owned())
                }
                Side::Saved => {
                    std::str::from_utf8(from).map_err(|error| error.to_string()).and_then(|text| match day {
                        Some(_) => {
                            TodTimeline::from_ron(text).map(Read::Day).map_err(|error| error.to_string())
                        }
                        None => LightRigLibrary::from_ron(text)
                            .map(Read::LightRigs)
                            .map_err(|error| error.to_string()),
                    })
                }
            };
            let town = panel.town.get_or_insert_with(TownLook::default);
            let look = read.map(|read| Arc::new(crate::look::town_look(read.source(), town)));
            if let Ok(look) = &look {
                panel.last_read = Some(Arc::clone(look));
            }
            panel.shown = Some(Shown { id, side, from: from.to_vec(), look });
        }
        // A side that doesn't read leaves the stage up on the last look that did, or the town's own, so the other
        // side shows at once again rather than standing the stage from cold.
        match &panel.shown.as_ref()?.look {
            Ok(look) => Some(LookShown { view, look: Arc::clone(look), note: None }),
            Err(why) => {
                let town = panel.town.get_or_insert_with(TownLook::default);
                let look = panel.last_read.get_or_insert_with(|| Arc::new(town.clone()));
                let what = match side {
                    Side::Edited => "B",
                    Side::Saved => "A",
                };
                let note = format!("{what} doesn't read, so the stage shows the last look that did: {why}");
                Some(LookShown { view, look: Arc::clone(look), note: Some(note) })
            }
        }
    }

    pub(super) fn look_panel(&mut self, ui: &mut Ui) {
        let Some((id, read)) = self.current_look() else {
            ui.label("Open a day (_day.ron) or light rigs (_rigs.ron) to see it on the look-dev stage.");
            return;
        };
        let mut asked = Vec::new();
        let view = self.look.view;
        ui.horizontal_wrapped(|ui| {
            for shot in LookShot::ALL {
                if ui.selectable_label(view.shot == shot, shot.name()).clicked() {
                    asked.push(Command::LookShot(shot));
                }
            }
            ui.separator();
            for tier in TierId::ALL {
                if ui.selectable_label(view.tier == tier, tier.name()).clicked() {
                    asked.push(Command::LookTier(tier));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            for side in [Side::Saved, Side::Edited] {
                if ui.selectable_label(view.side == side, side.name()).clicked() {
                    asked.push(Command::Compare(side));
                }
            }
            let dirty = self.documents.is_dirty(id);
            let says = match (view.side, dirty) {
                (_, false) => "A and B are the same: nothing's unsaved",
                (Side::Saved, true) => "the file as saved, without the edits since",
                (Side::Edited, true) => "the document with its unsaved edits",
            };
            ui.label(RichText::new(says).weak());
        });
        let mut hour = view.hour;
        let slider =
            egui::Slider::new(&mut hour, 0.0..=DAY_HOURS - 1.0 / 60.0).text("hour").fixed_decimals(2);
        if ui.add(slider).changed() {
            asked.push(Command::LookHour(hour));
        }
        if let Some(shown) =
            self.look.shown.as_ref().filter(|shown| (shown.id, shown.side) == (id, Side::Saved))
            && let Err(why) = &shown.look
        {
            ui.label(
                RichText::new(format!(
                    "The file as saved doesn't read, so A shows the last look that did: {why}"
                ))
                .weak(),
            );
        }
        match &read {
            Read::Day(day) => self.day_ui(ui, id, day, &mut asked),
            Read::LightRigs(rigs) => self.rigs_ui(ui, id, rigs, &mut asked),
        }
        for command in asked {
            self.act(command);
        }
    }

    /// A day's curves through the hours, its keys on them, and the picked key's numbers.
    fn day_ui(&mut self, ui: &mut Ui, id: AssetId, day: &TodTimeline, asked: &mut Vec<Command>) {
        let keys = day.keys();
        let partial = !day.is_complete();
        if partial {
            ui.label(
                RichText::new("A partial day: it keys only what it overrides, laid over the look-dev day across the town.")
                    .weak(),
            );
        }
        let width = ui.available_width().max(120.0);
        let (rect, response) = ui.allocate_exact_size(vec2(width, 150.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, Color32::from_gray(20));
        let shown: Vec<Channel> =
            CHANNELS.into_iter().filter(|channel| keys.iter().any(|key| keyed(channel.name, key))).collect();
        for (index, channel) in shown.iter().enumerate() {
            let (values, least, most) = curve(day, *channel);
            let span = (most - least).max(1e-6);
            let color = COLORS[index % COLORS.len()];
            let points: Vec<Pos2> = values
                .iter()
                .enumerate()
                .map(|(sample, value)| {
                    let x = rect.left() + rect.width() * sample as f32 / (SAMPLES - 1) as f32;
                    let y = rect.bottom() - 14.0 - (rect.height() - 24.0) * (value - least) / span;
                    pos2(x, y)
                })
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(1.5, color)));
            painter.text(
                rect.left_top() + vec2(4.0, 2.0 + 12.0 * index as f32),
                egui::Align2::LEFT_TOP,
                channel.name,
                egui::FontId::proportional(11.0),
                color,
            );
        }
        let x_of = |hours: f32| rect.left() + rect.width() * hours / DAY_HOURS;
        let hours_at = |x: f32| (x - rect.left()) / rect.width() * DAY_HOURS;
        let now = x_of(self.look.view.hour);
        painter.line_segment(
            [pos2(now, rect.top()), pos2(now, rect.bottom())],
            Stroke::new(1.0, Color32::WHITE),
        );
        for (index, key) in keys.iter().enumerate() {
            let at = pos2(x_of(key.time), rect.bottom() - 6.0);
            let picked = self.look.key == Some(index);
            let color = if picked { Color32::YELLOW } else { Color32::LIGHT_GRAY };
            painter.add(egui::Shape::convex_polygon(
                vec![at + vec2(0.0, -5.0), at + vec2(5.0, 0.0), at + vec2(0.0, 5.0), at + vec2(-5.0, 0.0)],
                color,
                Stroke::NONE,
            ));
        }
        let near_key = |x: f32| {
            keys.iter()
                .enumerate()
                .map(|(index, key)| (index, (x_of(key.time) - x).abs()))
                .filter(|(_, off)| *off <= 6.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(index, _)| index)
        };
        let pointer = response.interact_pointer_pos();
        let mut began = false;
        if response.drag_started()
            && let Some(index) = ui.input(|input| input.pointer.press_origin()).and_then(|at| near_key(at.x))
        {
            self.look.key = Some(index);
            self.look.dragging = Some(index);
            asked.push(Command::Begin(MOVE_KEY.to_owned()));
            began = true;
        }
        if let (Some(index), Some(at)) = (self.look.dragging, pointer)
            && let Some(key) = keys.get(index)
        {
            let before = index.checked_sub(1).and_then(|before| keys.get(before)).map(|key| key.time);
            let after = keys.get(index + 1).map(|key| key.time);
            let time = dragged_time(hours_at(at.x), before, after);
            if time.to_bits() != key.time.to_bits() {
                // An undo or a save ended the drag's gesture under it: the rest of it is a step of its own (Z3b).
                if !began && !self.in_gesture(id) {
                    asked.push(Command::Begin(MOVE_KEY.to_owned()));
                }
                asked.push(Command::SetProperty {
                    path: format!("keys[{index}].time"),
                    value: format!("{time:?}"),
                });
            }
        }
        if response.drag_stopped() && self.look.dragging.take().is_some() {
            asked.push(Command::End);
        }
        if response.clicked()
            && let Some(at) = pointer
        {
            match near_key(at.x) {
                Some(index) => self.look.key = Some(index),
                None => asked.push(Command::LookHour(dragged_time(hours_at(at.x), None, None))),
            }
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Day curves"));
        let Some(index) = self.look.key.filter(|index| *index < keys.len()) else {
            ui.label(
                RichText::new(
                    "Click a key to pick it, drag one along the day, or click the day for an hour.",
                )
                .weak(),
            );
            return;
        };
        let key = &keys[index];
        ui.label(format!("Key {} at {}", index + 1, clock(key.time)));
        egui::Grid::new("look key").num_columns(2).show(ui, |ui| {
            for (name, path, value) in key_numbers(key) {
                ui.label(name);
                let mut value = value;
                let response = ui.add(egui::DragValue::new(&mut value).speed(0.01).max_decimals(3));
                self.number_gesture(&response, &format!("edit a key's {}", name.to_lowercase()), id);
                if response.changed() {
                    asked.push(Command::SetProperty {
                        path: format!("keys[{index}].{path}"),
                        value: format!("{value:?}"),
                    });
                }
                ui.end_row();
            }
        });
    }

    /// Whether a gesture on `id` is open: one an undo or a save ended isn't.
    fn in_gesture(&self, id: AssetId) -> bool {
        self.documents.any(id).is_some_and(|file| file.in_gesture())
    }

    /// A library's presets: the picked one's key light on a dial, and its numbers.
    fn rigs_ui(&mut self, ui: &mut Ui, id: AssetId, rigs: &LightRigLibrary, asked: &mut Vec<Command>) {
        let names: Vec<&str> = rigs.names().collect();
        self.look.rig = self.look.rig.min(names.len().saturating_sub(1));
        ui.horizontal_wrapped(|ui| {
            for (index, name) in names.iter().enumerate() {
                if ui.selectable_label(self.look.rig == index, *name).clicked() {
                    self.look.rig = index;
                }
            }
        });
        let index = self.look.rig;
        let rig = rigs.rig(LightRigId(index as u16));
        let at = format!("rigs[{index}].rig");
        ui.label(
            RichText::new(
                "The key light's direction from the camera: round is azimuth, out is down from overhead.",
            )
            .weak(),
        );
        let size = ui.available_width().clamp(80.0, 160.0);
        let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let (center, radius) = (rect.center(), size * 0.45);
        painter.circle_stroke(center, radius, Stroke::new(1.0, Color32::GRAY));
        painter.circle_stroke(center, radius * 0.5, Stroke::new(1.0, Color32::from_gray(60)));
        let direction = rig.key.camera_direction;
        let dot = dial_point(rect, direction.azimuth, direction.elevation);
        painter.circle_filled(dot, 5.0, Color32::YELLOW);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Key light dial"));
        let began = response.drag_started();
        if began {
            asked.push(Command::Begin(TURN_KEY_LIGHT.to_owned()));
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let (azimuth, elevation) = dial_angles(rect, pointer);
            let turned = azimuth != direction.azimuth || elevation != direction.elevation;
            // An undo or a save ended the turn's gesture under it: the rest of it is a step of its own (Z3b).
            if turned && !began && !self.in_gesture(id) {
                asked.push(Command::Begin(TURN_KEY_LIGHT.to_owned()));
            }
            if azimuth != direction.azimuth {
                let path = format!("{at}.key.camera_direction.azimuth");
                asked.push(Command::SetProperty { path, value: format!("{azimuth:?}") });
            }
            if elevation != direction.elevation {
                let path = format!("{at}.key.camera_direction.elevation");
                asked.push(Command::SetProperty { path, value: format!("{elevation:?}") });
            }
        }
        if response.drag_stopped() {
            asked.push(Command::End);
        }
        egui::Grid::new("look rig").num_columns(2).show(ui, |ui| {
            let numbers = [
                ("Key intensity", "key.intensity", rig.key.intensity),
                ("Follows the camera", "key.camera_follow", rig.key.camera_follow),
                ("Rim", "rim", rig.rim),
                ("Bounce", "gi.intensity", rig.gi.intensity),
            ];
            for (name, path, value) in numbers {
                ui.label(name);
                let mut value = value;
                let response = ui.add(egui::DragValue::new(&mut value).speed(0.01).max_decimals(3));
                self.number_gesture(&response, &format!("edit a rig's {}", name.to_lowercase()), id);
                if response.changed() {
                    asked.push(Command::SetProperty {
                        path: format!("{at}.{path}"),
                        value: format!("{value:?}"),
                    });
                }
                ui.end_row();
            }
        });
    }
}

/// Each curve's color, in [`CHANNELS`]' order of those a day keys.
const COLORS: [Color32; 5] = [
    Color32::from_rgb(255, 196, 92),
    Color32::from_rgb(140, 200, 255),
    Color32::from_rgb(190, 150, 255),
    Color32::from_rgb(230, 230, 230),
    Color32::from_rgb(140, 220, 190),
];

/// Whether `key` keys the channel called `name`: a partial day's curves are of what it keys.
fn keyed(name: &str, key: &TodKey) -> bool {
    match name {
        "Sun height" => key.sun.is_some(),
        "Exposure" => key.exposure.is_some(),
        "Day to night" => key.day_night.is_some(),
        "Stars" => key.stars.is_some(),
        "Fog" => key.fog.is_some(),
        _ => false,
    }
}

/// The numbers of `key` the pane edits, each with its path under the key, for those it keys.
fn key_numbers(key: &TodKey) -> Vec<(&'static str, &'static str, f32)> {
    let mut numbers = Vec::new();
    if let Some(sun) = &key.sun {
        numbers.push(("Sun intensity", "sun?.intensity", sun.intensity));
    }
    for (name, path, value) in [
        ("Exposure", "exposure?", key.exposure),
        ("Day to night", "day_night?", key.day_night),
        ("Stars", "stars?", key.stars),
        ("Emission", "emission?", key.emission),
    ] {
        if let Some(value) = value {
            numbers.push((name, path, value));
        }
    }
    numbers
}

/// `hours` as a clock reads them.
fn clock(hours: f32) -> String {
    let minutes = (hours * 60.0).round() as u32;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Where a direction shows on the dial in `rect`: round from the top by its azimuth, out from the middle as it
/// comes down from overhead to the horizon.
fn dial_point(rect: Rect, azimuth: f32, elevation: f32) -> Pos2 {
    let radius = rect.width() * 0.45 * (90.0 - elevation.clamp(0.0, 90.0)) / 90.0;
    let turn = azimuth.to_radians();
    rect.center() + vec2(turn.sin(), -turn.cos()) * radius
}

/// The direction a point of the dial in `rect` shows, to the degree: [`dial_point`]'s other way.
fn dial_angles(rect: Rect, at: Pos2) -> (f32, f32) {
    let off = at - rect.center();
    let out = (off.length() / (rect.width() * 0.45)).min(1.0);
    let azimuth = off.x.atan2(-off.y).to_degrees().round();
    let elevation = (90.0 * (1.0 - out)).round();
    (if azimuth == -180.0 { 180.0 } else { azimuth }, elevation)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A point the dial shows reads back as its direction.
    #[test]
    fn the_dial_reads_back_what_it_shows() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(160.0, 160.0));
        for (azimuth, elevation) in [(-35.0, 30.0), (90.0, 10.0), (180.0, 45.0), (0.0, 60.0)] {
            assert_eq!(dial_angles(rect, dial_point(rect, azimuth, elevation)), (azimuth, elevation));
        }
    }

    #[test]
    fn a_clock_reads_hours_and_minutes() {
        assert_eq!(clock(18.5), "18:30");
        assert_eq!(clock(4.75), "04:45");
    }
}
