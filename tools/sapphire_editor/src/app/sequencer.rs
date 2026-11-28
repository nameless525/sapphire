//! The sequencer pane (M3 E5, M6 T16a): the current timeline document's lanes on one ruler, a playhead to scrub,
//! Play, and its keys dragged in time, added at the playhead and removed.
//!
//! The playhead is on whole frames of [`RATE`], as a game's frames come, and the viewport shows the frame a
//! capture at that time does ([`crate::viewport`]): the timeline cooked from the document as the cooker cooks
//! it, played from its start on a fresh stage, a frame at a time, by the game's own `play_timeline`. Scrubbing
//! forward plays on from where it was; back, or after an edit, it starts over.
//!
//! A press on a key selects it and its lane, and a drag moves it to whole frames: one gesture, a `Begin`, a
//! [`Command::MoveKey`] each frame it moves and an `End`, so one undo. A press on a lane's name picks the lane;
//! Add key adds to the lane picked at the playhead, and Remove key takes the selected key away. What's worked
//! out, and refused, is [`crate::sequence`]'s. Everything else about a key's values is the Properties pane's.

use std::sync::Arc;

use egui::{Color32, RichText, Sense, Ui};
use sapphire_assets::AssetId;
use sapphire_document::{Document, DocumentSet, Refused};
use sapphire_timeline::{Timeline, TimelineDocument};

use super::{Editor, Notice};
use crate::commands::Command;
use crate::sequence::{self, KeyRef, Lane, Planned, When, Write};
use crate::viewport::TimelineShown;

/// The playhead's frames a second: a game's.
pub const RATE: f32 = 60.0;

/// How tall a track's row is, in points.
const ROW: f32 = 18.0;

/// How wide the tracks' names are, in points.
const NAMES: f32 = 120.0;

/// How near a key a press takes hold of it, in points.
const GRAB: f32 = 4.0;

/// The pane's state: the playhead, the timeline it's on, cooked, and what's picked and held.
#[derive(Debug, Default)]
pub(super) struct SequencerPanel {
    /// The playhead, in frames, on the timeline it's for.
    playhead: Option<(AssetId, u32)>,
    /// Playing from the time of the pass that last moved it, in seconds.
    playing: Option<f64>,
    /// The timeline as its document last cooked, and the text it cooked from.
    cooked: Option<Cooked>,
    /// The lane Add key adds to.
    lane: Option<Lane>,
    /// The key selected, which Remove key takes away.
    selected: Option<KeyRef>,
    /// The timeline the lane and the key are on, and the key's time when it was selected: another timeline, or
    /// the key gone from that time (an undo, an edit in Properties), and they're let go.
    picked_on: Option<(AssetId, Option<When>)>,
    /// What a key added to a lane of named things is named.
    name: String,
    /// What a press on the tracks has hold of until it's let go.
    held: Option<Held>,
}

#[derive(Debug)]
struct Cooked {
    id: AssetId,
    text: String,
    timeline: Result<Arc<Timeline>, String>,
}

/// What a press on the tracks took hold of.
#[derive(Debug)]
enum Held {
    /// The playhead.
    Playhead,
    /// A key of the timeline `on`, by how many frames past its start the press was, and whether its gesture has
    /// begun.
    Key { on: AssetId, key: KeyRef, grab: i64, begun: bool },
    /// A lane's name, or nothing.
    Nothing,
}

/// Where `key` is in `document`, if it's there.
fn when_of(document: &TimelineDocument, key: &KeyRef) -> Option<When> {
    key.lane.keys(document).into_iter().find(|(index, _)| *index == key.index).map(|(_, when)| when)
}

/// A row the pane draws: a lane of keys, or a list it shows and doesn't edit.
struct Row {
    name: String,
    lane: Option<Lane>,
    keys: Vec<(usize, When)>,
}

/// The rows of `document`: its lanes, then its time scale and events, which are the simulation's.
fn rows(document: &TimelineDocument) -> Vec<Row> {
    let mut rows: Vec<Row> = Lane::of(document)
        .into_iter()
        .map(|lane| Row { name: lane.label(), keys: lane.keys(document), lane: Some(lane) })
        .collect();
    let at =
        |times: Vec<f32>| -> Vec<(usize, When)> { times.into_iter().map(When::At).enumerate().collect() };
    if !document.time_scale.is_empty() {
        let keys = at(document.time_scale.iter().map(|key| key.at).collect());
        rows.push(Row { name: "Time scale".to_owned(), lane: None, keys });
    }
    if !document.events.is_empty() {
        let keys = at(document.events.iter().map(|key| key.at).collect());
        rows.push(Row { name: "Events".to_owned(), lane: None, keys });
    }
    rows
}

/// How many frames of [`RATE`] `length` seconds is, the last one included.
fn frames_of(length: f32) -> u32 {
    (length * RATE).round().max(0.0) as u32
}

impl SequencerPanel {
    /// The playhead on `timeline`, in frames.
    fn frame_on(&self, timeline: AssetId) -> u32 {
        self.playhead.filter(|(id, _)| *id == timeline).map_or(0, |(_, frame)| frame)
    }

    /// The timeline in `document`, cooked again if its text has changed since.
    fn cooked(
        &mut self,
        id: AssetId,
        document: &Document<TimelineDocument>,
    ) -> Result<Arc<Timeline>, String> {
        let fresh =
            self.cooked.as_ref().is_some_and(|cooked| cooked.id == id && cooked.text == document.text());
        if !fresh {
            let timeline = match document.value() {
                Some(value) => value.cook().map(Arc::new).map_err(|error| error.to_string()),
                None => Err("the document doesn't read".to_owned()),
            };
            self.cooked = Some(Cooked { id, text: document.text().to_owned(), timeline });
        }
        self.cooked.as_ref().expect("cooked above").timeline.clone()
    }
}

/// What the viewport plays of `current`, if it's a timeline that cooks, at the playhead `panel` has.
pub(super) fn timeline_shown(
    documents: &DocumentSet,
    panel: &mut SequencerPanel,
    current: Option<AssetId>,
) -> Option<TimelineShown> {
    let id = current?;
    let document = documents.get::<TimelineDocument>(id)?;
    let timeline = panel.cooked(id, document).ok()?;
    Some(TimelineShown { id, timeline, frame: panel.frame_on(id) })
}

/// Lands one of a planned edit's writes.
fn land(document: &mut Document<TimelineDocument>, label: &str, write: &Write) -> Result<(), Refused> {
    let path = |path: &str| path.parse().expect("the sequence module writes paths that parse");
    match write {
        Write::Literal { path: at, literal } => document.set_literal(label, &path(at), literal),
        Write::Insert { list, at, value } => document.insert(label, &path(list), *at, value.as_ref()),
        Write::Set { path: at, value } => document.set(label, &path(at), value.as_ref()),
        Write::Remove { path: at } => document.remove(label, &path(at)),
    }
    .map(|_| ())
}

impl Editor {
    /// The current document, if it's a timeline.
    fn current_timeline(&self) -> Option<AssetId> {
        let id = self.current?;
        self.documents.get::<TimelineDocument>(id).map(|_| id)
    }

    /// Puts the playhead of the current timeline at `frame`, of [`RATE`], and stops playing.
    pub fn scrub_to(&mut self, frame: u32) {
        let Some(id) = self.current_timeline() else { return };
        let last =
            self.documents.get::<TimelineDocument>(id).and_then(Document::value).map(|value| value.length);
        self.sequencer.playhead = Some((id, frame.min(last.map_or(0, frames_of))));
        self.sequencer.playing = None;
    }

    /// Plays the current timeline from its playhead, or from its start if that's at the end. One that doesn't cook
    /// plays nothing, and says why.
    pub fn play_timeline(&mut self) {
        let Some(id) = self.current_timeline() else {
            self.say(Notice { text: "no timeline to play".to_owned(), trouble: true });
            return;
        };
        let Some(document) = self.documents.get::<TimelineDocument>(id) else { return };
        let length = document.value().map(|value| value.length);
        if let Err(why) = self.sequencer.cooked(id, document) {
            self.say(Notice { text: format!("the timeline doesn't cook: {why}"), trouble: true });
            return;
        }
        let last = length.map_or(0, frames_of);
        if self.sequencer.frame_on(id) >= last {
            self.sequencer.playhead = Some((id, 0));
        }
        self.sequencer.playing = Some(self.now);
    }

    /// Stops the current timeline where it is.
    pub fn pause_timeline(&mut self) {
        self.sequencer.playing = None;
    }

    /// The playhead of the current timeline, in frames of [`RATE`].
    #[must_use]
    pub fn playhead(&self) -> Option<u32> {
        self.current_timeline().map(|id| self.sequencer.frame_on(id))
    }

    /// The key the sequencer has selected.
    #[must_use]
    pub fn selected_key(&self) -> Option<&KeyRef> {
        self.sequencer.selected.as_ref()
    }

    /// The current timeline's value, once what's being typed has gone in, or a refusal saying why there's none to
    /// edit.
    fn timeline_to_edit(&mut self) -> Option<(AssetId, TimelineDocument)> {
        if !self.take_typed() {
            self.refuse("what's being typed can't go in, so no key moves until it's fixed or dropped");
            return None;
        }
        let found = self.current_timeline().and_then(|id| {
            let value = self.documents.get::<TimelineDocument>(id)?.value()?.clone();
            Some((id, value))
        });
        if found.is_none() {
            self.refuse("no timeline that reads to edit the keys of");
        }
        found
    }

    /// What [`Command::MoveKey`] does.
    pub(super) fn move_key(&mut self, key: &KeyRef, frame: u32) {
        let Some((id, value)) = self.timeline_to_edit() else { return };
        let planned = sequence::moved(&value, key, frame);
        let at = planned.as_ref().ok().and_then(|planned| when_of(&planned.after, key));
        if self.write_keys(id, "move a key", planned) {
            self.sequencer.selected = Some(key.clone());
            self.sequencer.picked_on = Some((id, at));
        }
    }

    /// What [`Command::AddKey`] does: the new key, at `frame`, is selected.
    pub(super) fn add_key(&mut self, lane: &Lane, name: Option<&str>, frame: u32) {
        let Some((id, value)) = self.timeline_to_edit() else { return };
        let planned = sequence::added(&value, lane, frame, name);
        // Where it went: an entry's place, or the camera key at the playhead that took focus.
        let index = planned.as_ref().ok().and_then(|planned| match planned.writes.first() {
            Some(Write::Insert { at, .. }) => Some(*at),
            _ => {
                let at = sequence::seconds_of(frame);
                lane.keys(&planned.after).iter().find(|(_, when)| when.start() == at).map(|(index, _)| *index)
            }
        });
        let selected = index.map(|index| KeyRef { lane: lane.clone(), index });
        let at = selected
            .as_ref()
            .zip(planned.as_ref().ok())
            .and_then(|(key, planned)| when_of(&planned.after, key));
        if self.write_keys(id, "add a key", planned) {
            self.sequencer.lane = Some(lane.clone());
            self.sequencer.selected = selected;
            self.sequencer.picked_on = Some((id, at));
        }
    }

    /// What [`Command::RemoveKey`] does.
    pub(super) fn remove_key(&mut self, key: &KeyRef) {
        let Some((id, value)) = self.timeline_to_edit() else { return };
        let planned = sequence::removed(&value, key);
        if self.write_keys(id, "remove a key", planned) {
            self.sequencer.selected = None;
        }
    }

    /// Writes a planned key edit into `id`'s text as one edit, a step of its own or of the gesture under way, or says
    /// why it's refused. Whether it landed.
    fn write_keys(&mut self, id: AssetId, label: &str, planned: Result<Planned, String>) -> bool {
        let planned = match planned {
            Ok(planned) => planned,
            Err(why) => {
                self.refuse(why);
                return false;
            }
        };
        let document = self
            .documents
            .edit(id)
            .and_then(|document| document.as_any_mut().downcast_mut::<Document<TimelineDocument>>());
        let Some(document) = document else { return false };
        let result = document.together(label, |document| {
            planned.writes.iter().try_for_each(|write| land(document, label, write))
        });
        debug_assert!(
            result.is_err() || document.value() == Some(&planned.after),
            "the text reads as the edit was worked out"
        );
        let landed = result.is_ok();
        self.after_property_edit(Some(id), result.map_err(|error| error.to_string()));
        landed
    }

    /// Lets go of a key held on any timeline but `on`, a timeline that reads, closing its drag's gesture: the
    /// pane went to another document, or the one it was on stopped reading, with the button still down.
    fn let_go_unless_on(&mut self, on: Option<AssetId>) {
        if let Some(Held::Key { on: held_on, begun, .. }) = &self.sequencer.held
            && Some(*held_on) != on
        {
            let begun = *begun;
            self.sequencer.held = None;
            if begun {
                self.act(Command::End);
            }
        }
    }

    pub(super) fn sequencer_panel(&mut self, ui: &mut Ui) {
        let current = self.current_timeline();
        let reads = current.filter(|id| {
            self.documents.get::<TimelineDocument>(*id).is_some_and(|document| document.value().is_some())
        });
        self.let_go_unless_on(reads);
        let Some(id) = current else {
            ui.label("Open a timeline to scrub it.");
            return;
        };
        let Some(document) = self.documents.get::<TimelineDocument>(id) else { return };
        let Some(value) = document.value() else {
            ui.label("The timeline doesn't read, so the viewport plays nothing until it does.");
            return;
        };
        // The lane and the key picked are let go on another timeline, and the key once it isn't where it was.
        if let Some((on, at)) = &self.sequencer.picked_on {
            let key_moved = self.sequencer.selected.as_ref().is_some_and(|key| when_of(value, key) != *at);
            if *on != id {
                self.sequencer.lane = None;
                self.sequencer.selected = None;
            } else if key_moved {
                self.sequencer.selected = None;
            }
        }
        let (rows, length) = (rows(value), value.length);
        let cooked = self.sequencer.cooked(id, document);
        let last = frames_of(length);
        let mut frame = self.sequencer.frame_on(id).min(last);

        // Playing moves the playhead on the pass's clock, a frame of the game's at a time.
        if let Some(since) = self.sequencer.playing {
            let moved = ((self.now - since) * f64::from(RATE)).floor().max(0.0) as u32;
            if moved > 0 {
                frame = (frame + moved).min(last);
                self.sequencer.playing = (frame < last).then_some(since + f64::from(moved) / f64::from(RATE));
            }
            ui.ctx().request_repaint();
        }
        // What a person asks, run once the pane has drawn, after the playhead playing moved.
        let mut asked = Vec::new();
        ui.horizontal(|ui| {
            if self.sequencer.playing.is_some() {
                if ui.button("Pause").clicked() {
                    asked.push(Command::PauseTimeline);
                }
            } else if ui.add_enabled(cooked.is_ok(), egui::Button::new("Play")).clicked() {
                asked.push(Command::PlayTimeline);
            }
            let mut scrubbed = frame;
            let slider = egui::Slider::new(&mut scrubbed, 0..=last).show_value(false).text("Playhead");
            if ui.add(slider).changed() {
                asked.push(Command::Scrub(scrubbed));
            }
            ui.label(format!("{:.2} s, frame {frame} of {last}", frame as f32 / RATE));
        });
        if let Err(why) = &cooked {
            ui.label(RichText::new(format!("It doesn't cook, so the viewport plays nothing: {why}")).weak());
        }
        self.key_buttons(ui, frame, &mut asked);

        // The lanes on one ruler: a press on a key holds it, on a name picks its lane, and anywhere else scrubs.
        let width = ui.available_width();
        let height = ROW * rows.len().max(1) as f32;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
        let ruler = egui::Rect::from_min_max(rect.min + egui::vec2(NAMES, 0.0), rect.max);
        let x_of = |seconds: f32| ruler.min.x + ruler.width() * (seconds / length.max(1e-3)).clamp(0.0, 1.0);
        let frame_at = |x: f32| {
            let share = ((x - ruler.min.x) / ruler.width().max(1.0)).clamp(0.0, 1.0);
            (share * last as f32).round() as i64
        };
        let pressed = response.is_pointer_button_down_on();
        let pointer = response.interact_pointer_pos().filter(|_| pressed);
        match pointer {
            Some(at) if self.sequencer.held.is_none() => {
                let row = rows.get(((at.y - rect.min.y) / ROW).floor().max(0.0) as usize);
                let lane = row.and_then(|row| row.lane.clone());
                let near = |(_, when): &(usize, When)| match *when {
                    When::At(time) => (x_of(time) - at.x).abs() <= GRAB,
                    When::Span(from, to) => (x_of(from) - GRAB..=x_of(to) + GRAB).contains(&at.x),
                };
                let key =
                    row.and_then(|row| row.keys.iter().find(|key| near(key)).copied()).zip(lane.clone());
                self.sequencer.held = Some(if at.x < ruler.min.x {
                    self.sequencer.lane = lane.or(self.sequencer.lane.take());
                    let at =
                        self.sequencer.picked_on.take().filter(|(on, _)| *on == id).and_then(|(_, at)| at);
                    self.sequencer.picked_on = Some((id, at));
                    Held::Nothing
                } else if let Some(((index, when), lane)) = key {
                    let key = KeyRef { lane: lane.clone(), index };
                    self.sequencer.lane = Some(lane);
                    self.sequencer.selected = Some(key.clone());
                    self.sequencer.picked_on = Some((id, Some(when)));
                    let grab = frame_at(at.x) - i64::from(sequence::frame_of(when.start()));
                    Held::Key { on: id, key, grab, begun: false }
                } else {
                    Held::Playhead
                });
            }
            Some(_) => {}
            // Let go: a drag's gesture ends.
            None => {
                if let Some(Held::Key { begun: true, .. }) = self.sequencer.held.take() {
                    asked.push(Command::End);
                }
            }
        }
        let in_gesture = self.documents.any(id).is_some_and(|document| document.in_gesture());
        if let (Some(at), Some(held)) = (pointer, &mut self.sequencer.held) {
            match held {
                Held::Playhead => {
                    let scrubbed = frame_at(at.x) as u32;
                    if scrubbed != frame || self.sequencer.playing.is_some() {
                        asked.push(Command::Scrub(scrubbed));
                    }
                }
                Held::Key { key, grab, begun, .. } => {
                    let to = u32::try_from((frame_at(at.x) - *grab).max(0)).unwrap_or(0).min(last);
                    let now = rows
                        .iter()
                        .find(|row| row.lane.as_ref() == Some(&key.lane))
                        .and_then(|row| row.keys.iter().find(|(index, _)| *index == key.index))
                        .map(|(_, when)| sequence::frame_of(when.start()));
                    if now.is_some_and(|now| now != to) {
                        // An undo or a save mid-drag ends the gesture: the rest of the drag is a step of its own.
                        if !*begun || !in_gesture {
                            asked.push(Command::Begin("move a key".to_owned()));
                            *begun = true;
                        }
                        asked.push(Command::MoveKey { key: key.clone(), to });
                    }
                }
                Held::Nothing => {}
            }
        }
        self.paint_rows(ui, rect, ruler, &rows, length, frame);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Tracks"));
        self.sequencer.playhead = Some((id, frame));
        for command in asked {
            self.act(command);
        }
    }

    /// Add key, for the lane picked, with a name if its keys are named, and Remove key, for the key selected.
    fn key_buttons(&mut self, ui: &mut Ui, frame: u32, asked: &mut Vec<Command>) {
        let panel = &mut self.sequencer;
        ui.horizontal(|ui| {
            let Some(lane) = panel.lane.clone() else {
                ui.label(RichText::new("Press a lane's name to add keys to it.").weak());
                return;
            };
            ui.label(lane.label());
            let name = lane.takes().map(|what| {
                let edit = egui::TextEdit::singleline(&mut panel.name).hint_text(what).desired_width(150.0);
                ui.add(edit);
                panel.name.clone()
            });
            if ui.button("Add key").clicked() {
                asked.push(Command::AddKey { lane, name, at: frame });
            }
            if ui.add_enabled(panel.selected.is_some(), egui::Button::new("Remove key")).clicked()
                && let Some(key) = panel.selected.clone()
            {
                asked.push(Command::RemoveKey(key));
            }
        });
    }

    /// The rows' names, bands and keys, the selected key and the lane picked marked, and the playhead.
    fn paint_rows(
        &self,
        ui: &Ui,
        rect: egui::Rect,
        ruler: egui::Rect,
        rows: &[Row],
        length: f32,
        frame: u32,
    ) {
        let x_of = |seconds: f32| ruler.min.x + ruler.width() * (seconds / length.max(1e-3)).clamp(0.0, 1.0);
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        let picked = Color32::from_rgb(240, 190, 70);
        for (index, row) in rows.iter().enumerate() {
            let top = rect.min.y + ROW * index as f32;
            let band = egui::Rect::from_min_max(
                egui::pos2(ruler.min.x, top + 2.0),
                egui::pos2(ruler.max.x, top + ROW - 2.0),
            );
            painter.rect_filled(band, 2.0, visuals.faint_bg_color);
            let lane_picked = row.lane.is_some() && row.lane == self.sequencer.lane;
            painter.text(
                egui::pos2(rect.min.x, top + ROW * 0.5),
                egui::Align2::LEFT_CENTER,
                &row.name,
                egui::FontId::proportional(12.0),
                if lane_picked { picked } else { visuals.text_color() },
            );
            for &(key, when) in &row.keys {
                let selected = self.sequencer.selected.as_ref().is_some_and(|selected| {
                    Some(&selected.lane) == row.lane.as_ref() && selected.index == key
                });
                match when {
                    When::Span(from, to) => {
                        let span = egui::Rect::from_min_max(
                            egui::pos2(x_of(from), band.min.y + 3.0),
                            egui::pos2(x_of(to), band.max.y - 3.0),
                        );
                        painter.rect_filled(
                            span,
                            2.0,
                            if selected { picked } else { visuals.selection.bg_fill },
                        );
                    }
                    When::At(at) => {
                        let x = x_of(at);
                        let (width, colour) =
                            if selected { (3.0, picked) } else { (2.0, visuals.selection.stroke.color) };
                        painter.line_segment(
                            [egui::pos2(x, band.min.y), egui::pos2(x, band.max.y)],
                            (width, colour),
                        );
                    }
                }
            }
        }
        let x = x_of(frame as f32 / RATE);
        painter.line_segment(
            [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
            (1.5, Color32::from_rgb(230, 80, 60)),
        );
    }
}
