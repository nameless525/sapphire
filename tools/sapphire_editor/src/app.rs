//! The editor's interface and state, everything but its window: [`Editor::ui`] draws into any `egui::Ui`,
//! so tests and [`crate::capture`] drive exactly what a person sees, with no window.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{Color32, Key, Modifiers, RichText, Sense, TextEdit, Ui, Vec2};
use sapphire_assets::AssetId;
use sapphire_core::Name;
use sapphire_livelink::protocol::MoveAttempt;
use sapphire_moves::{MoveDocument, MoveSet, MoveSetDocument, RootSample};

use crate::cooked::{self, Cook};
use crate::edits::{EditError, EntryRef, EntryValue, MovesText, Track};
use crate::figure::{Casting, Figure, Figures};
use crate::file::{Conflict, FileError, MovesFile, Verdict, judge};
use crate::link::{Link, Pushed};
use crate::preview::{self, FigureCache, Overlays, PosedFigure, View};
use crate::run::{Frame, Run, Setup};
use crate::timeline::{self, Action, Drag, Part, PhaseEdge, Problem, TimelineView};

/// How often the file and the cook are looked at again.
const POLL_SECONDS: f64 = 1.0;
/// The speeds "play" offers.
const SPEEDS: [(f32, &str); 4] = [(1.0, "1×"), (0.5, "0.5×"), (0.25, "0.25×"), (0.1, "0.1×")];
/// A device's live link, unless told otherwise.
pub const DEFAULT_LINK: &str = "127.0.0.1:7811";

/// How an editor starts.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The `.moves.ron` to open.
    pub file: Option<PathBuf>,
    /// The folder of a cook's packs: `target/content/desktop`. Found from the file's folder if not given.
    pub content: Option<PathBuf>,
    /// A device's live link. Not given, [`DEFAULT_LINK`].
    pub link: Option<SocketAddr>,
}

/// A note for the top of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub trouble: bool,
}

/// What's playing.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Playing {
    started: f64,
    first_frame: usize,
}

/// The cook, once read.
#[derive(Debug, Default)]
struct CookState {
    cook: Option<Cook>,
    why_none: Option<String>,
    /// What a cook made of this document: what its clips' tables are read from.
    set: Option<MoveSet>,
    figures: Figures,
    casting: Option<Casting>,
    /// Goes up when the cook changes, for what's cached from it.
    generation: u64,
    /// The newest pack when this was read, so a cook that didn't read is tried again only when another comes.
    tried: Option<SystemTime>,
}

/// The whole editor.
#[derive(Debug)]
pub struct Editor {
    file: Option<MovesFile>,
    documents: Vec<PathBuf>,
    open_path: String,
    content: Option<PathBuf>,
    cook: CookState,
    /// The text [`Self::verdict`] and the run were made from.
    judged: Option<String>,
    verdict: Verdict,
    /// The last set that compiled, which a refused edit's preview keeps showing.
    set: Option<MoveSet>,
    /// The text no longer compiles, so the preview is of what last did.
    stale: bool,
    selected_move: usize,
    selected_entry: Option<EntryRef>,
    tick: u32,
    /// "Play from frame N": where playing starts, and what it comes back to.
    play_from: u32,
    playing: Option<Playing>,
    speed: f32,
    looping: bool,
    setup: Setup,
    run: Option<Run>,
    run_error: Option<String>,
    frame: usize,
    view: View,
    overlays: Overlays,
    cache: FigureCache,
    drag: Option<Drag>,
    show_all_tracks: bool,
    link: Link,
    address_text: String,
    notice: Option<Notice>,
    last_poll: f64,
    quit: Quit,
    /// A document picked with edits unsaved: the top bar asks first.
    opening: Option<PathBuf>,
    /// What's being typed in the inspector, which is on the screen and not yet in the text.
    typed: Option<Typed>,
}

/// A value being typed in the inspector. It goes into the text when its field lets go, or sooner if
/// anything needs the text whole, so half a name is no edit and a field's whole session is one step of
/// undo. Meanwhile it counts as an edit ([`Editor::is_dirty`]) and the text holds still under it: every
/// other edit, undo, redo and a save take it first, and a change on disk waits.
#[derive(Clone, Debug)]
struct Typed {
    entry: EntryRef,
    path: String,
    field: egui::Id,
    text: String,
    /// Why the text wouldn't have it, the last time it tried. It stays in its field, since it's a person's
    /// work, and nothing else goes in until it's fixed or dropped.
    refused: Option<String>,
}

/// Where leaving stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Quit {
    #[default]
    No,
    /// Asked, with edits unsaved: the top bar asks back.
    Asking,
    Yes,
}

impl Editor {
    #[must_use]
    pub fn new(options: Options) -> Self {
        let address = options.link.unwrap_or_else(|| DEFAULT_LINK.parse().expect("a good default"));
        let mut editor = Self {
            file: None,
            documents: Vec::new(),
            open_path: String::new(),
            content: options.content,
            cook: CookState::default(),
            judged: None,
            verdict: Verdict::default(),
            set: None,
            stale: false,
            selected_move: 0,
            selected_entry: None,
            tick: 0,
            play_from: 0,
            playing: None,
            speed: 1.0,
            looping: false,
            setup: Setup::default(),
            run: None,
            run_error: None,
            frame: 0,
            view: View::default(),
            overlays: Overlays::default(),
            cache: FigureCache::default(),
            drag: None,
            show_all_tracks: false,
            link: Link::new(Some(address)),
            address_text: address.to_string(),
            notice: None,
            last_poll: 0.0,
            quit: Quit::No,
            opening: None,
            typed: None,
        };
        if let Some(path) = options.file {
            editor.open(&path);
        }
        if editor.file.is_none() {
            // With no file named, or none there, the list is the only way to one.
            editor.documents =
                std::env::current_dir().map_or_else(|_| Vec::new(), |dir| documents_around(&dir));
        }
        editor
    }

    /// A document was picked: it opens at once, unless that would lose edits, and then the top bar asks.
    pub fn request_open(&mut self, path: &Path) {
        if self.is_dirty() {
            self.opening = Some(path.to_owned());
        } else {
            self.open(path);
        }
    }

    /// Opens a document, over any edits here. On failure what's open stays open, and the notice says why.
    pub fn open(&mut self, path: &Path) {
        self.opening = None;
        match MovesFile::open(path) {
            Ok(file) => {
                self.open_path = path.display().to_string();
                if self.content.is_none() {
                    self.content = find_cook(path);
                }
                self.documents = find_documents(path);
                self.file = Some(file);
                self.typed = None;
                self.selected_move = 0;
                self.selected_entry = None;
                self.tick = 0;
                self.play_from = 0;
                self.playing = None;
                self.judged = None;
                self.set = None;
                self.run = None;
                self.notice = None;
                self.load_cook();
                self.refresh();
            }
            Err(error) => {
                self.notice = Some(Notice { text: error.to_string(), trouble: true });
            }
        }
    }

    fn load_cook(&mut self) {
        let generation = self.cook.generation + 1;
        self.cook = CookState { generation, ..CookState::default() };
        let Some(dir) = self.content.clone() else {
            self.cook.why_none = Some("no cook found: `cargo xtask content` makes one".to_owned());
            return;
        };
        self.cook.tried = cooked::newest_pack(&dir);
        match Cook::open(&dir) {
            Ok(cook) => {
                if let Some(file) = &self.file {
                    match cook.move_set_of(file.path()) {
                        Ok(set) => self.cook.set = set,
                        Err(why) => self.cook.why_none = Some(why),
                    }
                    match Figures::for_document(file.path(), &cook) {
                        Ok(figures) => self.cook.figures = figures,
                        Err(why) => self.cook.why_none = Some(why),
                    }
                }
                self.cook.cook = Some(cook);
            }
            Err(why) => self.cook.why_none = Some(format!("{why}: `cargo xtask content` makes one")),
        }
        self.judged = None;
    }

    /// The open file.
    #[must_use]
    pub fn file(&self) -> Option<&MovesFile> {
        self.file.as_ref()
    }

    /// Whether there's anything here the file doesn't have: edits, or a value still being typed.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.typed.is_some() || self.file.as_ref().is_some_and(MovesFile::is_dirty)
    }

    /// The open file, for edits a test makes as a person would by hand.
    pub fn file_mut(&mut self) -> Option<&mut MovesFile> {
        self.file.as_mut()
    }

    #[must_use]
    pub fn tick(&self) -> u32 {
        self.tick
    }

    /// The frame the preview shows.
    #[must_use]
    pub fn frame(&self) -> Option<&Frame> {
        self.run.as_ref()?.frames.get(self.frame)
    }

    #[must_use]
    pub fn run(&self) -> Option<&Run> {
        self.run.as_ref()
    }

    #[must_use]
    pub fn link(&self) -> &Link {
        &self.link
    }

    #[must_use]
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// Whether a cook has been read: what previews moves that name clips.
    #[must_use]
    pub fn has_cook(&self) -> bool {
        self.cook.cook.is_some()
    }

    #[must_use]
    pub fn selected_entry(&self) -> Option<EntryRef> {
        self.selected_entry
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    #[must_use]
    pub fn play_from(&self) -> u32 {
        self.play_from
    }

    #[must_use]
    pub fn selected_move_name(&self) -> Option<&str> {
        Some(self.document()?.moves.get(self.selected_move)?.name.as_str())
    }

    /// What the document says of itself, and what a device last refused of it.
    #[must_use]
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems: Vec<Problem> = self
            .verdict
            .issues
            .iter()
            .map(|issue| Problem {
                in_move: issue.in_move.clone(),
                track: Some(issue.track.to_owned()),
                message: issue.message.clone(),
                from_device: false,
            })
            .collect();
        if let Some(Pushed::Refused(refusals)) = self.link.last_push() {
            for refusal in refusals {
                let problem = Problem {
                    in_move: refusal.in_move.clone(),
                    track: refusal.track.clone(),
                    message: refusal.message.clone(),
                    from_device: true,
                };
                let said_already = problems.iter().any(|known| {
                    known.in_move == problem.in_move
                        && known.track == problem.track
                        && known.message == problem.message
                });
                if !said_already {
                    problems.push(problem);
                }
            }
        }
        problems
    }

    /// What the text says, or what it last said while it says nothing.
    fn document(&self) -> Option<&MoveSetDocument> {
        let text = self.file.as_ref()?.moves_text();
        text.document().or_else(|| text.last_good())
    }

    /// Selects a move by name.
    pub fn select_move(&mut self, name: &str) -> bool {
        let Some(index) =
            self.document().and_then(|document| document.moves.iter().position(|mv| mv.name == name))
        else {
            return false;
        };
        self.choose_move(index);
        true
    }

    fn choose_move(&mut self, index: usize) {
        if self.selected_move != index {
            self.selected_move = index;
            self.selected_entry = None;
            self.playing = None;
            self.set_tick(0);
            self.play_from = 0;
            self.remake_run();
        }
    }

    /// Scrubs to a tick, and stops playing.
    pub fn set_tick(&mut self, tick: u32) {
        self.playing = None;
        self.move_playhead(tick);
        self.play_from = self.tick;
    }

    fn move_playhead(&mut self, tick: u32) {
        let last = self
            .document()
            .and_then(|document| document.moves.get(self.selected_move))
            .map_or(0, |mv| mv.ticks().saturating_sub(1));
        self.tick = tick.min(last);
        if let Some(run) = &self.run {
            self.frame = run.frame_at_tick(self.tick).unwrap_or(0);
        }
    }

    /// Plays from frame N: [`Self::play_from`]'s tick, at `now` seconds.
    pub fn play(&mut self, now: f64) {
        let Some(first_frame) = self.run.as_ref().and_then(|run| run.frame_at_tick(self.play_from)) else {
            return;
        };
        self.playing = Some(Playing { started: now, first_frame });
        self.frame = first_frame;
        self.tick = self.play_from;
    }

    /// Stops where it is, keeping that tick.
    pub fn stop(&mut self) {
        if self.playing.take().is_some() {
            self.play_from = self.tick;
        }
    }

    /// Moves playback on to `now` seconds. Played out, it goes back to where it started, ready to go again.
    pub fn advance_playback(&mut self, now: f64) {
        let Some(playing) = self.playing else { return };
        let Some(run) = &self.run else { return };
        let steps = ((now - playing.started).max(0.0) * 60.0 * f64::from(self.speed)) as usize;
        let frame = playing.first_frame + steps;
        if frame < run.frames.len() {
            self.frame = frame;
            self.tick = run.frames[frame].tick.min(run.ticks.saturating_sub(1));
        } else if self.looping {
            self.playing = Some(Playing { started: now, first_frame: playing.first_frame });
            self.frame = playing.first_frame;
            self.tick = self.play_from;
        } else {
            self.playing = None;
            self.frame = playing.first_frame;
            self.tick = self.play_from;
        }
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
    }

    /// Sets how far the dummy stands, which plays the move again.
    pub fn set_dummy_distance(&mut self, distance: f32) {
        self.setup.dummy_distance = distance;
        self.remake_run();
    }

    /// Sets whether warp windows bend toward the dummy, which plays the move again.
    pub fn set_warp_to_dummy(&mut self, warp: bool) {
        self.setup.warp_to_dummy = warp;
        self.remake_run();
    }

    /// Runs the document's checks and the preview's run again if the text has changed.
    pub fn refresh(&mut self) {
        let Some(file) = &self.file else { return };
        if self.judged.as_deref() == Some(file.text()) {
            return;
        }
        let text = file.text().to_owned();
        let changed_before = self.judged.is_some();
        match file.moves_text().document() {
            Some(document) => {
                self.verdict = judge(document, self.cook.set.as_ref());
                self.stale = self.verdict.set.is_none();
                if let Some(set) = &self.verdict.set {
                    self.set = Some(set.clone());
                }
            }
            None => {
                self.verdict = Verdict::default();
                self.stale = true;
            }
        }
        self.judged = Some(text);
        if changed_before {
            // What a device last said was about the text before this.
            self.link.clear_push();
        }
        let moves = self.document().map_or(0, |document| document.moves.len());
        if self.selected_move >= moves {
            self.selected_move = 0;
            self.selected_entry = None;
        }
        self.remake_run();
    }

    fn remake_run(&mut self) {
        self.run_error = None;
        let Some(mv) = self.document().and_then(|document| document.moves.get(self.selected_move)).cloned()
        else {
            self.run = None;
            return;
        };
        // A cook's figure for this move, or none.
        self.cook.casting = None;
        if let Some(cook) = &self.cook.cook {
            match self.cook.figures.cast(&mv, cook) {
                Ok(casting) => self.cook.casting = casting,
                Err(why) => self.run_error = Some(why),
            }
        }
        let Some(set) = &self.set else {
            self.run = None;
            self.run_error =
                Some(match (&self.cook.cook, self.document().is_some_and(MoveSetDocument::names_clips)) {
                    (None, true) => {
                        "this document's moves name clips, and there's no cook to read them from: \
                                 `cargo xtask content`, and the preview follows"
                            .to_owned()
                    }
                    _ => "the document hasn't compiled yet".to_owned(),
                });
            return;
        };
        match Run::simulate(set, &mv.name, self.setup) {
            Ok(run) => {
                self.frame = run.frame_at_tick(self.tick).unwrap_or(0);
                self.playing = self.playing.filter(|playing| playing.first_frame < run.frames.len());
                self.run = Some(run);
            }
            Err(error) => {
                self.run = None;
                self.run_error = Some(error.0);
            }
        }
    }

    /// Saves, then pushes what was saved. A file with nothing new is pushed all the same: a device may have
    /// started since.
    pub fn save_and_push(&mut self) {
        match self.save() {
            Ok(()) => {
                let Some(file) = &self.file else { return };
                let (name, text) = (file.file_name(), file.text().to_owned());
                self.notice = Some(Notice { text: format!("saved {name}"), trouble: false });
                self.link.push(name, text);
            }
            Err(text) => self.notice = Some(Notice { text, trouble: true }),
        }
    }

    /// Writes the file, with what's being typed in it. What goes wrong is in a notice's words.
    fn save(&mut self) -> Result<(), String> {
        if !self.take_typed() {
            return Err("not saved: what's being typed can't go in".to_owned());
        }
        let Some(file) = &mut self.file else { return Ok(()) };
        file.save().map_err(|error| match error {
            FileError::ChangedOnDisk { .. } => error.to_string(),
            error => format!("not saved: {error}"),
        })
    }

    /// Puts what's being typed into the text, as one step of undo. False if the text won't have it: it
    /// stays in its field, and the top bar says why.
    fn take_typed(&mut self) -> bool {
        let Some(typed) = self.typed.take() else { return true };
        let Some(file) = &mut self.file else { return true };
        let label = format!("edit {}", typed.path);
        match file.edit(&label, |text| text.set_entry_value(typed.entry, &typed.path, &typed.text)) {
            Ok(()) => {
                self.notice = None;
                self.refresh();
                true
            }
            Err(error) => {
                self.typed = Some(Typed { refused: Some(error.0), ..typed });
                false
            }
        }
    }

    /// Typing is over when its field lets go, whatever took the focus: a click anywhere, Enter, or the
    /// row going with the selection, which leaves the field no pass to see it in. A refused value waits
    /// for someone to change it.
    fn settle_typed(&mut self, ui: &Ui) {
        let let_go = self.typed.as_ref().is_some_and(|typed| {
            typed.refused.is_none() && !ui.memory(|memory| memory.has_focus(typed.field))
        });
        if let_go {
            self.take_typed();
        }
    }

    /// A drag whose button went away with the window's focus ends here, so the next edit isn't part of it.
    fn settle_gesture(&mut self, ui: &Ui) {
        if ui.input(|input| input.pointer.any_down()) {
            return;
        }
        self.drag = None;
        if let Some(file) = &mut self.file
            && file.in_gesture()
        {
            file.end_gesture();
        }
    }

    /// The window was asked to close: it goes at once, unless that would lose edits.
    pub fn request_quit(&mut self) {
        self.quit = if self.is_dirty() { Quit::Asking } else { Quit::Yes };
    }

    /// Whether the window may close now.
    #[must_use]
    pub fn may_quit(&self) -> bool {
        self.quit == Quit::Yes
    }

    /// Points the link at another device.
    pub fn set_link_address(&mut self, address: SocketAddr) {
        self.address_text = address.to_string();
        self.link.set_address(Some(address));
    }

    pub fn undo(&mut self) {
        // What's being typed is the newest edit, so it's what goes first. Text that can't go in has no step
        // to take back, and goes by itself.
        if !self.take_typed() {
            self.typed = None;
            return;
        }
        if let Some(file) = &mut self.file
            && file.undo()
        {
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        if self.take_typed()
            && let Some(file) = &mut self.file
            && file.redo()
        {
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.notice = None;
        self.refresh();
        self.check_selection();
    }

    fn check_selection(&mut self) {
        let Some(entry) = self.selected_entry else { return };
        let exists = self
            .document()
            .and_then(|document| document.moves.get(entry.mv))
            .is_some_and(|mv| entry.index < entry.track.spans(mv).len());
        if !exists {
            self.selected_entry = None;
        }
    }

    fn edit<T>(
        &mut self,
        label: &str,
        edit: impl FnOnce(&mut MovesText) -> Result<T, EditError>,
    ) -> Option<T> {
        if !self.take_typed() {
            return None;
        }
        let file = self.file.as_mut()?;
        match file.edit(label, edit) {
            Ok(result) => {
                self.notice = None;
                self.refresh();
                Some(result)
            }
            Err(error) => {
                self.notice = Some(Notice { text: error.0, trouble: true });
                None
            }
        }
    }

    /// Applies what the timeline asked.
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Scrub(tick) => self.set_tick(tick),
            Action::Select(entry) => self.selected_entry = entry,
            Action::BeginGesture(label) => {
                self.stop();
                self.begin_gesture(&label);
            }
            Action::EndGesture => {
                if let Some(file) = &mut self.file {
                    file.end_gesture();
                }
            }
            Action::SetWindow { entry, from, to, part } => {
                self.edit("edit a window", |text| text.set_window(entry, from, to));
                // The preview shows the tick the drag has hold of.
                let tick = match part {
                    Part::Start | Part::Body => from,
                    Part::End => to.saturating_sub(1),
                };
                self.set_tick(tick);
            }
            Action::SetInstant { entry, at } => {
                self.edit("move an instant", |text| text.set_instant(entry, at));
                self.set_tick(at);
            }
            Action::SetPhases { mv, phases, edge } => {
                self.edit("edit the phases", |text| text.set_phases(mv, phases));
                let tick = match edge {
                    PhaseEdge::StartupEnd => phases.startup,
                    PhaseEdge::ActiveEnd => phases.startup + phases.active,
                    PhaseEdge::MoveEnd => phases.startup + phases.active + phases.recovery,
                };
                self.set_tick(tick);
            }
            Action::Add(track) => self.add_to_track(track),
        }
    }

    /// Starts a gesture, with what's being typed in the text first, as a step of its own.
    fn begin_gesture(&mut self, label: &str) {
        self.take_typed();
        if let Some(file) = &mut self.file {
            file.begin_gesture(label);
        }
    }

    /// Adds to a track of the selected move, starting at the playhead.
    pub fn add_to_track(&mut self, track: Track) {
        let (Some(document), mv_index, tick) = (self.document(), self.selected_move, self.tick) else {
            return;
        };
        let Some(mv) = document.moves.get(mv_index) else { return };
        let entry = default_entry(document, mv, track, tick);
        let label = format!("add to {}", timeline::title(track));
        if let Some(added) = self.edit(&label, |text| text.insert_entry(mv_index, track, &entry)) {
            self.selected_entry = Some(added);
        }
    }

    /// Deletes the selected entry.
    pub fn delete_selected(&mut self) {
        let Some(entry) = self.selected_entry else { return };
        let label = format!("delete from {}", timeline::title(entry.track));
        if self.edit(&label, |text| text.delete_entry(entry)).is_some() {
            self.selected_entry = None;
        }
    }

    /// Sets the selected window's start or end at the playhead: `[` and `]`.
    pub fn set_selected_end_at_playhead(&mut self, start: bool) {
        let Some(entry) = self.selected_entry else { return };
        let Some(mv) = self.document().and_then(|document| document.moves.get(entry.mv)) else { return };
        let Some(span) = entry.track.spans(mv).get(entry.index).copied() else { return };
        let tick = self.tick;
        if entry.track.is_instant() {
            self.edit("move an instant", |text| text.set_instant(entry, tick));
        } else if start {
            self.edit("set a window's start", |text| text.set_window(entry, tick, span.to));
        } else {
            // The playhead's tick is the window's last open tick.
            self.edit("set a window's end", |text| text.set_window(entry, span.from, tick + 1));
        }
    }

    /// Nudges the selected entry by whole ticks.
    pub fn nudge_selected(&mut self, by: i64) {
        let Some(entry) = self.selected_entry else { return };
        let Some(mv) = self.document().and_then(|document| document.moves.get(entry.mv)) else { return };
        let Some(span) = entry.track.spans(mv).get(entry.index).copied() else { return };
        let from = (i64::from(span.from) + by).max(0) as u32;
        let to = from + span.to.saturating_sub(span.from);
        if entry.track.is_instant() {
            self.edit("nudge an instant", |text| text.set_instant(entry, from));
        } else {
            self.edit("nudge a window", |text| text.set_window(entry, from, to));
        }
        self.set_tick(from);
    }

    /// Looks at the file and the cook again, and takes what live link has reported.
    pub fn poll(&mut self, now: f64) {
        self.link.poll();
        if now - self.last_poll < POLL_SECONDS {
            return;
        }
        self.last_poll = now;
        if self.typed.is_none()
            && let Some(file) = &mut self.file
        {
            file.poll_disk();
        }
        if self.cook_changed() {
            self.load_cook();
        }
        self.refresh();
        self.check_selection();
    }

    /// Whether there's a cook to read that the last look didn't see: a newer one, or a first one, or
    /// another after one that didn't read.
    fn cook_changed(&mut self) -> bool {
        if let Some(cook) = &self.cook.cook {
            return cook.is_stale();
        }
        if self.content.is_none() {
            self.content = self.file.as_ref().and_then(|file| find_cook(file.path()));
        }
        let newest = self.content.as_deref().and_then(cooked::newest_pack);
        newest.is_some() && newest != self.cook.tried
    }

    /// The whole interface.
    pub fn ui(&mut self, ui: &mut Ui) {
        let now = ui.input(|input| input.time);
        self.poll(now);
        self.advance_playback(now);
        self.keys(ui, now);
        if self.playing.is_some() {
            ui.ctx().request_repaint();
        } else {
            // For live link's replies and the file on disk.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }

        ui.painter().rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        egui::Panel::top("editor top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("editor moves")
            .default_size(170.0)
            .size_range(120.0..=300.0)
            .show(ui, |ui| self.moves_list(ui));
        egui::Panel::right("editor side").default_size(290.0).size_range(220.0..=460.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("side").show(ui, |ui| {
                self.inspector(ui);
                ui.separator();
                self.device_panel(ui);
            });
        });
        egui::Panel::bottom("editor timeline")
            .resizable(true)
            .default_size(340.0)
            .show(ui, |ui| self.timeline_panel(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| self.preview_panel(ui, now));
        self.settle_gesture(ui);
        self.settle_typed(ui);
    }

    fn keys(&mut self, ui: &Ui, now: f64) {
        let ctx = ui.ctx();
        let typing = ctx.egui_wants_keyboard_input();
        let pressed =
            |modifiers: Modifiers, key: Key| ctx.input_mut(|input| input.consume_key(modifiers, key));
        if pressed(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) {
            self.redo();
        }
        if pressed(Modifiers::COMMAND, Key::Z) {
            self.undo();
        }
        if pressed(Modifiers::COMMAND, Key::S) {
            self.save_and_push();
        }
        if typing {
            return;
        }
        if pressed(Modifiers::NONE, Key::Space) {
            if self.playing.is_some() {
                self.stop();
            } else {
                self.play(now);
            }
        }
        if pressed(Modifiers::ALT, Key::ArrowLeft) {
            self.nudge_selected(-1);
        }
        if pressed(Modifiers::ALT, Key::ArrowRight) {
            self.nudge_selected(1);
        }
        if pressed(Modifiers::NONE, Key::ArrowLeft) || pressed(Modifiers::NONE, Key::Comma) {
            self.set_tick(self.tick.saturating_sub(1));
        }
        if pressed(Modifiers::NONE, Key::ArrowRight) || pressed(Modifiers::NONE, Key::Period) {
            self.set_tick(self.tick + 1);
        }
        if pressed(Modifiers::NONE, Key::Home) {
            self.set_tick(0);
        }
        if pressed(Modifiers::NONE, Key::End) {
            self.set_tick(u32::MAX);
        }
        if pressed(Modifiers::NONE, Key::OpenBracket) {
            self.set_selected_end_at_playhead(true);
        }
        if pressed(Modifiers::NONE, Key::CloseBracket) {
            self.set_selected_end_at_playhead(false);
        }
        if pressed(Modifiers::NONE, Key::Delete) || pressed(Modifiers::NONE, Key::Backspace) {
            self.delete_selected();
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            let name = self.file.as_ref().map_or_else(|| "no file".to_owned(), MovesFile::file_name);
            let title = if self.is_dirty() { format!("{name} *") } else { name };
            let mut open = None;
            egui::ComboBox::from_id_salt("documents").selected_text(title).width(210.0).show_ui(ui, |ui| {
                for path in &self.documents {
                    let shown =
                        path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                    if ui.selectable_label(false, shown).on_hover_text(path.display().to_string()).clicked() {
                        open = Some(path.clone());
                    }
                }
            });
            if let Some(path) = open {
                self.request_open(&path);
            }

            let has_file = self.file.is_some();
            if ui
                .add_enabled(has_file, egui::Button::new("Save and push"))
                .on_hover_text("Cmd+S, or Ctrl+S")
                .clicked()
            {
                self.save_and_push();
            }
            let undo = self.file.as_ref().and_then(MovesFile::undo_label).map(ToOwned::to_owned);
            if ui
                .add_enabled(undo.is_some(), egui::Button::new("Undo"))
                .on_hover_text(undo.unwrap_or_default())
                .clicked()
            {
                self.undo();
            }
            let redo = self.file.as_ref().and_then(MovesFile::redo_label).map(ToOwned::to_owned);
            if ui
                .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                .on_hover_text(redo.unwrap_or_default())
                .clicked()
            {
                self.redo();
            }
            ui.separator();
            let device = self.link.device();
            let (dot, words) = match device {
                Some(status) => {
                    (Color32::from_rgb(96, 214, 110), format!("{} ({})", status.app, status.platform))
                }
                None => (Color32::from_gray(110), "no device".to_owned()),
            };
            let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(dot_rect.center(), 4.5, dot);
            let address = ui.add(TextEdit::singleline(&mut self.address_text).desired_width(130.0));
            if address.lost_focus() {
                match self.address_text.parse() {
                    Ok(address) => self.link.set_address(Some(address)),
                    Err(_) => {
                        self.notice = Some(Notice {
                            text: format!("`{}` isn't an address and port", self.address_text),
                            trouble: true,
                        });
                    }
                }
            }
            ui.label(words);
        });
        if self.quit == Quit::Asking {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), "You have edits that aren't saved.");
                if ui.button("Save and quit").clicked() {
                    match self.save() {
                        Ok(()) => self.quit = Quit::Yes,
                        Err(text) => {
                            self.notice = Some(Notice { text, trouble: true });
                            self.quit = Quit::No;
                        }
                    }
                }
                if ui.button("Quit without saving").clicked() {
                    self.quit = Quit::Yes;
                }
                if ui.button("Keep editing").clicked() {
                    self.quit = Quit::No;
                }
            });
        }
        if let Some(path) = self.opening.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), "You have edits that aren't saved.");
                if ui.button("Save and open").clicked() {
                    match self.save() {
                        Ok(()) => self.open(&path),
                        Err(text) => {
                            self.notice = Some(Notice { text, trouble: true });
                            self.opening = None;
                        }
                    }
                }
                if ui.button("Open without saving").clicked() {
                    self.open(&path);
                }
                if ui.button("Keep editing").clicked() {
                    self.opening = None;
                }
            });
        }
        if let Some(conflict) = self.file.as_ref().and_then(MovesFile::conflict) {
            let (said, can_take) = match conflict {
                Conflict::Text(_) => ("The file changed on disk, and you have edits here.", true),
                Conflict::NotText => (
                    "The file on disk isn't text any more. Keeping yours replaces it at the next save.",
                    false,
                ),
            };
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), said);
                if can_take
                    && ui.button("Take the file's").clicked()
                    && let Some(file) = &mut self.file
                {
                    self.typed = None;
                    file.take_disk();
                    self.refresh();
                }
                if ui.button("Keep mine").clicked()
                    && let Some(file) = &mut self.file
                {
                    file.keep_mine();
                }
            });
        }
        if let Some(typed) = &self.typed
            && let Some(why) = &typed.refused
        {
            let entry = typed.entry;
            let said = format!(
                "`{}` can't go in {} {}'s {}, and nothing else can until it's fixed or dropped: {why}",
                typed.text,
                timeline::title(entry.track),
                entry.index,
                typed.path,
            );
            ui.horizontal(|ui| {
                if ui.button("Show it").clicked() {
                    self.choose_move(entry.mv);
                    self.selected_entry = Some(entry);
                }
                if ui.button("Drop it").clicked() {
                    self.typed = None;
                }
                ui.colored_label(Color32::from_rgb(255, 120, 110), said);
            });
        }
        if let Some(failure) = self.file.as_ref().and_then(|file| file.moves_text().failure()) {
            let message = failure.message().to_owned();
            ui.colored_label(
                Color32::from_rgb(255, 120, 110),
                format!("This can't be edited until the file reads again. {message}"),
            );
        }
        if let Some(line) = self.file.as_ref().and_then(|file| generated_by(file.text())) {
            ui.colored_label(Color32::from_rgb(235, 190, 110), line);
        }
        if let Some(notice) = &self.notice {
            let color =
                if notice.trouble { Color32::from_rgb(255, 120, 110) } else { Color32::from_gray(190) };
            ui.colored_label(color, &notice.text);
        }
        ui.add_space(2.0);
    }

    fn moves_list(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("Moves").strong());
        let Some(document) = self.document() else { return };
        let problems = self.problems();
        let rows: Vec<(usize, String, u32, usize)> = document
            .moves
            .iter()
            .enumerate()
            .map(|(index, mv)| {
                let count = problems
                    .iter()
                    .filter(|problem| problem.in_move.as_deref() == Some(mv.name.as_str()))
                    .count();
                (index, mv.name.clone(), mv.ticks(), count)
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
                if ui.selectable_label(selected, text).clicked() {
                    choose = Some(index);
                }
            }
        });
        if let Some(index) = choose {
            self.choose_move(index);
        }
    }

    fn timeline_panel(&mut self, ui: &mut Ui) {
        ui.add_space(3.0);
        let problems = self.problems();
        let Some(document) = self.document() else {
            ui.label("Open a `.moves.ron` file.");
            return;
        };
        let Some(mv) = document.moves.get(self.selected_move).cloned() else { return };
        let root = self.root_samples(&mv.name);
        let hit_ticks: Vec<u32> = self
            .run
            .as_ref()
            .map_or_else(Vec::new, |run| run.hits().filter(|hit| hit.on_dummy).map(|hit| hit.tick).collect());
        let editable = self.file.as_ref().is_some_and(|file| file.moves_text().failure().is_none());
        let view = TimelineView {
            mv: &mv,
            mv_index: self.selected_move,
            tick: self.tick,
            selected: self.selected_entry,
            problems: &problems,
            root: root.as_deref(),
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
            ui.checkbox(&mut self.show_all_tracks, "All tracks");
            let command = if ui.ctx().os() == egui::os::OperatingSystem::Mac { "Cmd" } else { "Ctrl" };
            ui.label(
                RichText::new(format!(
                    "Drag ends and bodies.   [ and ]: window's ends to the playhead.   Alt+arrows: nudge.   \
                     Delete: remove.   {command}+Z: undo.   {command}+S: save and push."
                ))
                .small()
                .color(Color32::from_gray(140)),
            );
        });
        self.problems_strip(ui);
        // The panel keeps what it's been given, however little there is to show.
        ui.allocate_space(ui.available_size());
    }

    fn root_samples(&self, name: &str) -> Option<Vec<RootSample>> {
        let set = self.verdict.set.as_ref().or(self.set.as_ref())?;
        let (_, mv) = set.find(Name::intern(name))?;
        Some(mv.tables.root_samples().to_vec())
    }

    /// The document's problems, whichever move they're in.
    fn problems_strip(&mut self, ui: &mut Ui) {
        let problems = self.problems();
        if problems.is_empty() {
            let word = if self.verdict.set.is_some() {
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
            self.select_move(&name);
        }
    }

    fn preview_panel(&mut self, ui: &mut Ui, now: f64) {
        self.transport(ui, now);
        ui.horizontal(|ui| {
            ui.label("Dummy");
            let mut distance = self.setup.dummy_distance;
            let response =
                ui.add(egui::DragValue::new(&mut distance).speed(0.02).range(0.3..=12.0).suffix(" m"));
            if response.changed() {
                self.set_dummy_distance(distance);
            }
            let mut warp = self.setup.warp_to_dummy;
            if ui.checkbox(&mut warp, "Warp to it").changed() {
                self.set_warp_to_dummy(warp);
            }
            ui.separator();
            if ui.small_button("Side").clicked() {
                self.view = View::side(self.setup.dummy_distance);
            }
            if ui.small_button("Front").clicked() {
                self.view = View::front(self.setup.dummy_distance);
            }
            if ui.small_button("Top").clicked() {
                self.view = View::top(self.setup.dummy_distance);
            }
            ui.checkbox(&mut self.overlays.bones, "Bones");
            ui.checkbox(&mut self.overlays.hurtboxes, "Hurtboxes");
        });
        let size = ui.available_size().max(Vec2::splat(40.0));
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = response.rect;
        if response.dragged() {
            let delta = response.drag_delta();
            self.view.yaw += delta.x * 0.008;
            self.view.pitch = (self.view.pitch + delta.y * 0.008).clamp(-0.3, 1.55);
        }
        if response.hovered() {
            let zoom = ui.input(|input| input.smooth_scroll_delta.y);
            if zoom != 0.0 {
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
            preview::draw(&painter, rect, &self.view, run, frame, posed, &mut self.cache, self.overlays);
            drew = true;
        }
        if !drew {
            let why = self.run_error.clone().unwrap_or_else(|| "Open a `.moves.ron` file.".to_owned());
            preview::empty(&painter, rect, &why);
        }
        if self.stale && drew {
            painter.text(
                rect.left_bottom() + Vec2::new(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                "the document doesn't compile: this shows what last did",
                egui::FontId::proportional(13.0),
                Color32::from_rgb(255, 150, 120),
            );
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

    fn transport(&mut self, ui: &mut Ui, now: f64) {
        ui.horizontal(|ui| {
            let ticks = self
                .document()
                .and_then(|document| document.moves.get(self.selected_move))
                .map_or(0, MoveDocument::ticks);
            if ui.small_button("|<").on_hover_text("First tick (Home)").clicked() {
                self.set_tick(0);
            }
            if ui.small_button("<").on_hover_text("One tick back (Left, or comma)").clicked() {
                self.set_tick(self.tick.saturating_sub(1));
            }
            if ui.small_button(">").on_hover_text("One tick on (Right, or full stop)").clicked() {
                self.set_tick(self.tick + 1);
            }
            let mut tick = self.tick;
            let response = ui.add(
                egui::DragValue::new(&mut tick)
                    .speed(0.25)
                    .range(0..=ticks.saturating_sub(1))
                    .prefix("tick "),
            );
            if response.changed() {
                self.set_tick(tick);
            }
            ui.label(format!("of {ticks}, {:.0} ms", f64::from(self.tick) * 1000.0 / 60.0));
            ui.separator();
            let playing = self.playing.is_some();
            let label =
                if playing { "Stop".to_owned() } else { format!("Play from frame {}", self.play_from) };
            if ui.button(label).on_hover_text("Space").clicked() {
                if playing {
                    self.stop();
                } else {
                    self.play(now);
                }
            }
            for (speed, word) in SPEEDS {
                if ui.selectable_label(self.speed == speed, word).clicked() {
                    self.speed = speed;
                }
            }
            ui.checkbox(&mut self.looping, "Loop");
        });
    }

    fn inspector(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let Some(entry) = self.selected_entry else {
            self.move_inspector(ui);
            return;
        };
        ui.label(RichText::new(timeline::title(entry.track)).strong());
        let Some(document) = self.document() else { return };
        let Some(mv) = document.moves.get(entry.mv) else { return };
        let Some(span) = entry.track.spans(mv).get(entry.index).copied() else { return };
        let (from, to, is_instant) = (span.from, span.to, entry.track.is_instant());
        let editable = self.file.as_ref().is_some_and(|file| file.moves_text().failure().is_none());
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("entry ticks").num_columns(2).show(ui, |ui| {
                if is_instant {
                    let mut at = from;
                    ui.label("at");
                    let response = ui.add(egui::DragValue::new(&mut at).speed(0.25));
                    self.number_gesture(&response, "move an instant");
                    if response.changed() {
                        self.edit("move an instant", |text| text.set_instant(entry, at));
                    }
                    ui.end_row();
                } else {
                    let (mut new_from, mut new_to) = (from, to);
                    ui.label("from");
                    let response = ui.add(egui::DragValue::new(&mut new_from).speed(0.25));
                    self.number_gesture(&response, "edit a window");
                    let from_changed = response.changed();
                    ui.end_row();
                    ui.label("to");
                    let response = ui.add(egui::DragValue::new(&mut new_to).speed(0.25));
                    self.number_gesture(&response, "edit a window");
                    if from_changed || response.changed() {
                        self.edit("edit a window", |text| text.set_window(entry, new_from, new_to));
                    }
                    ui.end_row();
                }
            });
            let values = self.file.as_ref().and_then(|file| file.moves_text().entry_values(entry).ok());
            if let Some(values) = values {
                egui::Grid::new("entry values").num_columns(2).show(ui, |ui| {
                    for value in values {
                        self.value_row(ui, entry, &value);
                    }
                });
            }
            ui.add_space(4.0);
            if ui.button("Delete").on_hover_text("Delete or Backspace").clicked() {
                self.delete_selected();
            }
        });
    }

    fn number_gesture(&mut self, response: &egui::Response, label: &str) {
        if response.drag_started() {
            self.begin_gesture(label);
        }
        if response.drag_stopped()
            && let Some(file) = &mut self.file
        {
            file.end_gesture();
        }
    }

    fn value_row(&mut self, ui: &mut Ui, entry: EntryRef, value: &EntryValue) {
        ui.label(&value.path);
        let label = format!("edit {}", value.path);
        if let Ok(number) = value.literal.parse::<f64>() {
            let is_int = !value.literal.contains(['.', 'e', 'E']);
            let decimals = value.literal.split_once('.').map_or(2, |(_, fraction)| fraction.len().max(2));
            let mut shown = number;
            let drag = if is_int {
                egui::DragValue::new(&mut shown).speed(0.25).max_decimals(0)
            } else {
                egui::DragValue::new(&mut shown).speed(0.01).fixed_decimals(decimals)
            };
            let response = ui.add(drag);
            self.number_gesture(&response, &label);
            if response.changed() {
                let literal =
                    if is_int { format!("{}", shown.round() as i64) } else { format!("{shown:.decimals$}") };
                self.edit(&label, |text| text.set_entry_value(entry, &value.path, &literal));
            }
        } else {
            // A name or a bare word, as the file writes it: see `Typed`.
            let field = egui::Id::unique(("inspector value", entry, &value.path));
            let typed = self.typed.as_ref();
            let here = typed.filter(|typed| typed.field == field);
            let waiting_elsewhere =
                typed.is_some_and(|typed| typed.refused.is_some() && typed.field != field);
            let mut buffer = here.map_or_else(|| value.literal.clone(), |typed| typed.text.clone());
            let mut edit = TextEdit::singleline(&mut buffer).id(field).desired_width(150.0);
            if here.is_some_and(|typed| typed.refused.is_some()) {
                edit = edit.text_color(Color32::from_rgb(255, 120, 110));
            }
            if ui.add_enabled(!waiting_elsewhere, edit).changed() {
                let typed = Typed { entry, path: value.path.clone(), field, text: buffer, refused: None };
                self.keep_typing(typed, &value.literal);
            }
        }
        ui.end_row();
    }

    /// Takes what a field holds now. Typing in another field means the last one let go, and typing back to
    /// what the file says leaves nothing to take.
    fn keep_typing(&mut self, typed: Typed, literal: &str) {
        let elsewhere = self.typed.as_ref().is_some_and(|other| other.field != typed.field);
        if !elsewhere || self.take_typed() {
            self.typed = (typed.text != literal).then_some(typed);
        }
    }

    fn move_inspector(&mut self, ui: &mut Ui) {
        let Some(document) = self.document() else { return };
        let Some(mv) = document.moves.get(self.selected_move) else { return };
        let (name, startup, active, recovery, ticks) =
            (mv.name.clone(), mv.startup, mv.active, mv.recovery, mv.ticks());
        let clip = mv.motion.clip.as_ref().map(|clip| clip.source.clone());
        ui.label(RichText::new(name).strong());
        ui.label(format!("{ticks} ticks, {:.0} ms", f64::from(ticks) * 1000.0 / 60.0));
        ui.label(format!("startup {startup}, active {active}, recovery {recovery}"));
        match clip {
            Some(_) => {
                ui.label(
                    RichText::new(
                        "Plays a clip tick for tick, so its length is the clip's. Windows move freely.",
                    )
                    .small()
                    .color(Color32::from_gray(150)),
                );
            }
            None => {
                ui.label(RichText::new("Keyed by hand.").small().color(Color32::from_gray(150)));
            }
        }
        ui.label(
            RichText::new("Select something on the timeline to edit it.")
                .small()
                .color(Color32::from_gray(150)),
        );
    }

    fn device_panel(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Device").strong());
        match self.link.device() {
            Some(status) => {
                ui.label(format!("{}, cook `{}`", status.platform, status.cook));
                let name = self.file.as_ref().map(MovesFile::file_name).unwrap_or_default();
                let taken = status.documents.iter().find(|document| document.source.ends_with(name.as_str()));
                match taken {
                    Some(document) => {
                        ui.label(format!("takes this file: {} edits so far", document.edits));
                    }
                    None if !name.is_empty() => {
                        ui.colored_label(
                            Color32::from_rgb(235, 190, 110),
                            format!("no fight there plays {name}"),
                        );
                    }
                    None => {}
                }
            }
            None => {
                let why = self.link.trouble().unwrap_or("looking");
                ui.label(
                    RichText::new(format!("Not connected: {why}")).small().color(Color32::from_gray(150)),
                );
            }
        }
        match self.link.last_push() {
            _ if self.link.is_waiting() => {
                ui.label("Pushing...");
            }
            Some(Pushed::Took { owner, milliseconds }) => {
                ui.colored_label(
                    Color32::from_rgb(120, 214, 130),
                    format!("Took it in {milliseconds} ms: {owner}"),
                );
            }
            Some(Pushed::Refused(refusals)) => {
                ui.colored_label(
                    Color32::from_rgb(255, 130, 115),
                    format!("Refused, {} problems: it keeps what it had. See the timeline.", refusals.len()),
                );
            }
            Some(Pushed::Failed(why)) => {
                ui.colored_label(Color32::from_rgb(235, 190, 110), format!("Not pushed: {why}"));
            }
            None => {}
        }
        let attempts: Vec<MoveAttempt> = self.link.attempts().to_vec();
        if !attempts.is_empty() {
            ui.add_space(3.0);
            ui.label(RichText::new("Last attempts").small().strong());
            for attempt in attempts {
                self.attempt_row(ui, &attempt);
            }
        }
    }

    fn attempt_row(&self, ui: &mut Ui, attempt: &MoveAttempt) {
        let newest = attempt.revision == attempt.newest_revision;
        let word = if newest { "newest" } else { "older" };
        ui.label(
            RichText::new(format!(
                "actor {}: {} rev {} ({word})",
                attempt.actor, attempt.name, attempt.revision
            ))
            .small(),
        );
        let windows: Vec<String> =
            attempt.hit_windows.iter().map(|window| format!("{}..{}", window.from, window.to)).collect();
        if !windows.is_empty() {
            ui.label(
                RichText::new(format!("   hits {}", windows.join(", ")))
                    .small()
                    .color(Color32::from_gray(160)),
            );
        }
    }
}

/// Whether the current move should have had a figure and didn't.
fn posed_missing(cook: &CookState, document: Option<&MoveSetDocument>) -> bool {
    document.is_some() && cook.casting.is_none()
}

/// What a new entry starts as, at `tick`, in the file's own words. The numbers are a start, not a choice:
/// a hit volume rides the move's first path, and a cancel allows what starts from neutral.
fn default_entry(document: &MoveSetDocument, mv: &MoveDocument, track: Track, tick: u32) -> String {
    let (from, to) = (tick, tick + 3);
    match track {
        Track::Hitboxes => {
            let from_clip =
                mv.motion.clip.as_ref().and_then(|clip| clip.paths.first()).map(|path| path.name.as_str());
            let path = from_clip
                .or_else(|| mv.motion.paths.first().map(|path| path.name.as_str()))
                .unwrap_or("path");
            let mut name = format!("{}_hit", mv.name);
            let mut count = 1;
            while mv.hitboxes.iter().any(|hitbox| hitbox.name == name) {
                count += 1;
                name = format!("{}_hit_{count}", mv.name);
            }
            format!(
                "(name: \"{name}\", from: {from}, to: {to}, path: \"{path}\", shape: Sphere(radius: 0.3))"
            )
        }
        Track::Hurtboxes => {
            let name = document
                .fighter
                .as_ref()
                .and_then(|fighter| fighter.hurtboxes.first())
                .map_or("body", |hurtbox| hurtbox.name.as_str());
            format!("(name: \"{name}\", from: {from}, to: {to}, state: Off)")
        }
        Track::Invulnerable | Track::Parry | Track::PerfectDodge => format!("(from: {from}, to: {to})"),
        Track::Armour => format!("(from: {from}, to: {to}, threshold: 10)"),
        Track::Cancels => {
            let (intent, target) = document
                .neutral
                .first()
                .map_or(("intent", "move"), |allow| (allow.intent.as_str(), allow.target.as_str()));
            format!("(from: {from}, to: {to}, allow: [(intent: \"{intent}\", move: \"{target}\")])")
        }
        Track::Warps => format!("(from: {from}, to: {to}, max_distance: 0.5, max_turn: 0.5, keep: 1.0)"),
        Track::Cues => format!("(name: \"cue\", at: {from})"),
        Track::RootKeys => format!("(at: {from})"),
    }
}

/// What a file says of whatever else writes it, if anything does: the sentence near its top with
/// "Written by" in it, so the file's own words decide what an edit here is worth. Only comments before
/// the document count, so a word in a move's comment can't set it off.
fn generated_by(text: &str) -> Option<String> {
    let header = text.lines().take_while(|line| line.trim_start().starts_with("//"));
    let mentions = header
        .map(|line| line.trim_start().trim_start_matches('/').trim())
        .find(|line| line.contains("Written by"))?;
    let at = mentions.find("Written by")?;
    let said = &mentions[at..];
    // A path's dots have no space after them.
    let end = said.find(". ").map_or(said.len(), |end| end + 1);
    Some(said[..end].trim().to_owned())
}

/// `target/content/desktop` above a document, if it's there.
fn find_cook(document: &Path) -> Option<PathBuf> {
    let start = if document.is_absolute() {
        document.to_owned()
    } else {
        std::env::current_dir().ok()?.join(document)
    };
    start.ancestors().skip(1).map(|dir| dir.join("target/content/desktop")).find(|dir| dir.is_dir())
}

/// Every `.moves.ron` under the repository a document is in, for the file list.
fn find_documents(document: &Path) -> Vec<PathBuf> {
    let start = if document.is_absolute() {
        document.to_owned()
    } else {
        std::env::current_dir().map_or_else(|_| document.to_owned(), |dir| dir.join(document))
    };
    let mut found = documents_around(&start);
    found.push(start);
    found.sort();
    found.dedup();
    found
}

/// Every `.moves.ron` under the repository `place` is in, if it's in one.
fn documents_around(place: &Path) -> Vec<PathBuf> {
    let root = place.ancestors().find(|dir| dir.join("Cargo.toml").is_file() && dir.join("content").is_dir());
    let mut found = Vec::new();
    if let Some(root) = root {
        for folder in ["content", "crates"] {
            walk(&root.join(folder), &mut found);
        }
    }
    found.sort();
    found
}

fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if !path.file_name().is_some_and(|name| name == "target" || name == "third_party") {
                walk(&path, found);
            }
        } else if path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(".moves.ron")) {
            found.push(path);
        }
    }
}

/// The IDs a test of the preview reads.
#[must_use]
pub fn casting_of(editor: &Editor) -> Option<(AssetId, Option<AssetId>)> {
    editor.cook.casting.map(|casting| (casting.skeleton, casting.clip))
}

/// The figure the preview poses, for a test to read joints from.
#[must_use]
pub fn figure_of(editor: &Editor) -> Option<&Figure> {
    editor.cook.figures.get(editor.cook.casting?)
}

#[cfg(test)]
mod tests {
    use super::generated_by;
    use crate::edits::test_files::{PLAYER, WARRIOR};

    #[test]
    fn a_file_a_tool_writes_says_which() {
        assert_eq!(
            generated_by(PLAYER).as_deref(),
            Some(
                "Written by tools/motion/moves.py --write-numbers, which won't replace edits made here unless \
                 forced."
            ),
        );
        assert_eq!(generated_by(WARRIOR), None);
        let more = "// Written by a.py, so edit it there. And more.\n(\n";
        assert_eq!(generated_by(more).as_deref(), Some("Written by a.py, so edit it there."));
    }
}
