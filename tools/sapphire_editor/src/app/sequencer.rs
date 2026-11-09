//! The sequencer pane (M3 E5): the current timeline document's tracks on one ruler, a playhead to scrub, and Play.
//!
//! The playhead is on whole frames of [`RATE`], as a game's frames come, and the viewport shows the frame a
//! capture at that time does ([`crate::viewport`]): the timeline cooked from the document as the cooker cooks
//! it, played from its start on a fresh stage, a frame at a time, by the game's own `play_timeline`. Scrubbing
//! forward plays on from where it was; back, or after an edit, it starts over. Everything about a key's values is
//! the Properties pane's: this one says where things are in time.

use std::sync::Arc;

use egui::{Color32, RichText, Sense, Ui};
use sapphire_assets::AssetId;
use sapphire_document::{Document, DocumentSet};
use sapphire_timeline::{Timeline, TimelineDocument};

use super::Editor;
use crate::viewport::TimelineShown;

/// The playhead's frames a second: a game's.
pub const RATE: f32 = 60.0;

/// How tall a track's row is, in points.
const ROW: f32 = 18.0;

/// How wide the tracks' names are, in points.
const NAMES: f32 = 120.0;

/// The pane's state: the playhead, and the timeline it's on, cooked.
#[derive(Debug, Default)]
pub(super) struct SequencerPanel {
    /// The playhead, in frames, on the timeline it's for.
    playhead: Option<(AssetId, u32)>,
    /// Playing from the time of the pass that last moved it, in seconds.
    playing: Option<f64>,
    /// The timeline as its document last cooked, and the text it cooked from.
    cooked: Option<Cooked>,
}

#[derive(Debug)]
struct Cooked {
    id: AssetId,
    text: String,
    timeline: Result<Arc<Timeline>, String>,
}

/// A track's name and where its keys are, in seconds: a key at a time, or a span.
struct Row {
    name: String,
    keys: Vec<(f32, Option<f32>)>,
}

/// The rows of `document`, one a track.
fn rows(document: &TimelineDocument) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut row = |name: String, keys: Vec<(f32, Option<f32>)>| rows.push(Row { name, keys });
    if let Some(camera) = &document.camera {
        row("Camera".to_owned(), camera.keys.iter().map(|key| (key.at, None)).collect());
    }
    for track in &document.post {
        row(format!("Post: {:?}", track.parameter), track.keys.iter().map(|key| (key.at, None)).collect());
    }
    for track in &document.characters {
        let plays = track.plays.iter().map(|play| (play.from, Some(play.to)));
        let places = track.places.iter().map(|place| (place.at, None));
        row(format!("Character: {}", track.role), plays.chain(places).collect());
    }
    if !document.cues.is_empty() {
        row("Cues".to_owned(), document.cues.iter().map(|key| (key.at, None)).collect());
    }
    if !document.subtitles.is_empty() {
        row("Subtitles".to_owned(), document.subtitles.iter().map(|key| (key.from, Some(key.to))).collect());
    }
    if !document.time_scale.is_empty() {
        row("Time scale".to_owned(), document.time_scale.iter().map(|key| (key.at, None)).collect());
    }
    if !document.events.is_empty() {
        row("Events".to_owned(), document.events.iter().map(|key| (key.at, None)).collect());
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

    /// The playhead of the current timeline, in frames of [`RATE`].
    #[must_use]
    pub fn playhead(&self) -> Option<u32> {
        self.current_timeline().map(|id| self.sequencer.frame_on(id))
    }

    pub(super) fn sequencer_panel(&mut self, ui: &mut Ui) {
        let Some(id) = self.current_timeline() else {
            ui.label("Open a timeline to scrub it.");
            return;
        };
        let Some(document) = self.documents.get::<TimelineDocument>(id) else { return };
        let Some(value) = document.value() else {
            ui.label("The timeline doesn't read, so the viewport plays nothing until it does.");
            return;
        };
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
        ui.horizontal(|ui| {
            if self.sequencer.playing.is_some() {
                if ui.button("Pause").clicked() {
                    self.sequencer.playing = None;
                }
            } else if ui.add_enabled(cooked.is_ok(), egui::Button::new("Play")).clicked() {
                if frame >= last {
                    frame = 0;
                }
                self.sequencer.playing = Some(self.now);
            }
            let slider = egui::Slider::new(&mut frame, 0..=last).show_value(false).text("Playhead");
            if ui.add(slider).changed() {
                self.sequencer.playing = None;
            }
            ui.label(format!("{:.2} s, frame {frame} of {last}", frame as f32 / RATE));
        });
        if let Err(why) = &cooked {
            ui.label(RichText::new(format!("It doesn't cook, so the viewport plays nothing: {why}")).weak());
        }

        // The tracks on one ruler; a press or a drag anywhere on it scrubs.
        let width = ui.available_width();
        let height = ROW * rows.len().max(1) as f32;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
        let ruler = egui::Rect::from_min_max(rect.min + egui::vec2(NAMES, 0.0), rect.max);
        let x_of = |seconds: f32| ruler.min.x + ruler.width() * (seconds / length.max(1e-3)).clamp(0.0, 1.0);
        if let Some(at) = response.interact_pointer_pos().filter(|_| response.is_pointer_button_down_on()) {
            let share = ((at.x - ruler.min.x) / ruler.width().max(1.0)).clamp(0.0, 1.0);
            frame = (share * last as f32).round() as u32;
            self.sequencer.playing = None;
        }
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        for (index, row) in rows.iter().enumerate() {
            let top = rect.min.y + ROW * index as f32;
            let band = egui::Rect::from_min_max(
                egui::pos2(ruler.min.x, top + 2.0),
                egui::pos2(ruler.max.x, top + ROW - 2.0),
            );
            painter.rect_filled(band, 2.0, visuals.faint_bg_color);
            painter.text(
                egui::pos2(rect.min.x, top + ROW * 0.5),
                egui::Align2::LEFT_CENTER,
                &row.name,
                egui::FontId::proportional(12.0),
                visuals.text_color(),
            );
            for &(from, to) in &row.keys {
                match to {
                    Some(to) => {
                        let span = egui::Rect::from_min_max(
                            egui::pos2(x_of(from), band.min.y + 3.0),
                            egui::pos2(x_of(to), band.max.y - 3.0),
                        );
                        painter.rect_filled(span, 2.0, visuals.selection.bg_fill);
                    }
                    None => {
                        let x = x_of(from);
                        painter.line_segment(
                            [egui::pos2(x, band.min.y), egui::pos2(x, band.max.y)],
                            (2.0, visuals.selection.stroke.color),
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
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Tracks"));
        self.sequencer.playhead = Some((id, frame));
    }
}
