//! [`Editor::run`]: what each [`Command`] does, and the recording a session can keep of them.
//!
//! The interface runs a command for every action, and runs nothing else that changes a document, so a script of
//! the commands a session ran does what the session did. A command a command runs on its own behalf isn't kept
//! (running the outer one again runs it again), but a value typed that lands in the text is: it lands as the
//! `SetValue` or `SetProperty` it amounts to, before whatever made it land. A command that's refused is kept out
//! too, since it changed nothing.

use std::path::{Path, PathBuf};

use sapphire_document::disk::Temporary;
use sapphire_document::{PropertyPath, Step};

use super::properties::Asked;
use super::{Conflict, Editor, Notice, Quit, ViewFit};
use crate::commands::{Command, CommandRegistry, Facing, Script};
use crate::preview::View;

/// Where a session's commands are written, and those written so far.
#[derive(Debug)]
pub(super) struct Recording {
    path: PathBuf,
    script: Script,
}

/// A script that stopped: the command that was refused, and why.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("command {index} of the script, `{command}`: {why}")]
pub struct ScriptError {
    /// Counted from 1, as a person reads the file.
    pub index: usize,
    pub command: String,
    pub why: String,
}

impl Editor {
    /// Runs one command, as a button, a key or a script does. A refused command leaves things as they were, and
    /// says why here and in the top bar's notice.
    pub fn run(&mut self, command: Command) -> Result<(), String> {
        debug_assert!(CommandRegistry.get(command.name()).is_some(), "{} is registered", command.name());
        let troubles = self.troubles;
        if self.depth == 0 {
            self.drawn_id = None;
            self.drawn_ids.clear();
            // A merge from disk can put another entry where a selected one was: only an edit here re-keys.
            if !matches!(command, Command::MineOnTop | Command::TakeFile) {
                self.stand_selection();
            }
        }
        self.depth += 1;
        self.perform(&command);
        self.depth -= 1;
        if self.depth == 0 && lists_anew(&command) {
            self.refresh_problems();
        }
        if self.depth == 0 {
            self.standings.clear();
        }
        if self.depth == 0 {
            // After each step, refused or not: a refusal can leave a gesture's middle behind it.
            self.journal_step();
        }
        if self.troubles != troubles {
            return Err(self
                .notice
                .as_ref()
                .map_or_else(|| "refused".to_owned(), |notice| notice.text.clone()));
        }
        if self.depth == 0 {
            // An entry added with an ID drawn at random is kept with that ID, so a replay makes the same one.
            let command = match (command, self.drawn_id.take()) {
                (Command::AddEntry(path), Some(id)) => Command::AddEntryAs { path, id },
                (Command::Duplicate, _) if !self.drawn_ids.is_empty() => {
                    Command::DuplicateAs(std::mem::take(&mut self.drawn_ids))
                }
                (command, _) => command,
            };
            self.record(command);
        }
        Ok(())
    }

    /// Runs a script's commands in order, stopping at the first refused. While a game plays, each waits for it
    /// (`settle_game`), up to [`GAME_STARTUP`](crate::app::GAME_STARTUP) for a Play and
    /// [`GAME_ANSWER`](crate::app::GAME_ANSWER) otherwise: a script has no frames for its answers to come in
    /// between.
    pub fn run_script(&mut self, script: &Script) -> Result<(), ScriptError> {
        for (index, command) in script.commands.iter().enumerate() {
            let error = |why| ScriptError { index: index + 1, command: command.to_text(), why };
            self.run(command.clone()).map_err(error)?;
            self.settle_game().map_err(error)?;
        }
        Ok(())
    }

    /// Runs a command for the interface, whose notice says what a refusal does.
    pub(super) fn act(&mut self, command: Command) {
        let _ = self.run(command);
    }

    /// Keeps every command run from now on in a script at `path`, starting with the documents open now, the
    /// current one last. The file is written after each command, so a session that ends any way keeps its own.
    pub fn record_to(&mut self, path: PathBuf) {
        let mut opened: Vec<PathBuf> = self
            .documents
            .ids()
            .filter(|id| Some(*id) != self.current)
            .chain(self.current)
            .filter_map(|id| self.documents.path(id).map(Path::to_owned))
            .collect();
        // The timeline's document goes on the timeline before the current one is gone to.
        if let Some(moves) = self.moves.and_then(|id| self.documents.path(id))
            && let Some(at) = opened.iter().position(|path| path == moves)
        {
            let moves = opened.remove(at);
            opened.insert(0, moves);
        }
        let commands = opened.into_iter().map(Command::Open).collect();
        // From where it's started, so the documents are named from the script's folder however it was named.
        let path = std::path::absolute(&path).unwrap_or(path);
        self.recording = Some(Recording { path, script: Script::new(commands) });
        self.write_recording();
    }

    /// The commands kept so far, if a session is being recorded.
    #[must_use]
    pub fn recorded(&self) -> Option<&Script> {
        self.recording.as_ref().map(|recording| &recording.script)
    }

    /// Keeps a command that ran at the top, or a typed value that landed.
    pub(super) fn record(&mut self, command: Command) {
        let Some(recording) = &mut self.recording else { return };
        recording.script.commands.push(command);
        self.write_recording();
    }

    fn write_recording(&self) {
        let Some(recording) = &self.recording else { return };
        let text = recording.script.relative_to(&recording.path).to_text();
        let path = &recording.path;
        if let Err(error) = Temporary::beside(path, &text).and_then(|temporary| temporary.replace(path)) {
            tracing::warn!(%error, path = %path.display(), "the session's commands weren't written");
        }
    }

    /// Puts a notice up. One in trouble refuses the command running.
    pub(super) fn say(&mut self, notice: Notice) {
        if notice.trouble {
            self.troubles += 1;
        }
        self.notice = Some(notice);
    }

    /// Refuses the command running, with why.
    pub(super) fn refuse(&mut self, text: impl Into<String>) {
        self.say(Notice { text: text.into(), trouble: true });
    }

    fn perform(&mut self, command: &Command) {
        match command {
            Command::Open(path) => self.open(path),
            Command::Save => match self.save() {
                Ok(()) => {
                    if let Some(name) = self.current().map(|file| file.file_name()) {
                        self.say(Notice { text: format!("saved {name}"), trouble: false });
                    }
                }
                Err(text) => self.refuse(text),
            },
            Command::Push => match self.current() {
                Some(file) => {
                    let (name, text) = (file.file_name(), file.text().to_owned());
                    self.link.push(name, text);
                }
                None => self.refuse("nothing to push: no document is open"),
            },
            Command::SaveAndPush => self.save_and_push(),
            Command::SaveAll => {
                if let Err(text) = self.save_all() {
                    self.refuse(text);
                }
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Close => self.request_close(),
            Command::SaveAndClose | Command::CloseWithoutSaving | Command::CancelClose => {
                self.answer_close(command);
            }
            Command::Quit => self.request_quit(),
            Command::SaveAndQuit | Command::QuitWithoutSaving | Command::CancelQuit => {
                self.answer_quit(command)
            }
            Command::TakeFile | Command::MineOnTop | Command::KeepMine => self.settle_conflict(command),
            Command::Recover(path) => self.recover(path),
            Command::DropRecovered(path) => self.drop_recovered(path),
            Command::DropTyped => self.typed = None,
            Command::ShowTyped => self.show_typed(),
            Command::Begin(label) => match self.current {
                Some(id) => self.begin_gesture_on(id, label),
                None => self.refuse("no document to begin a gesture on"),
            },
            Command::End => self.end_gestures(),
            Command::Cancel => self.cancel_gestures(),
            Command::SetProperty { path, value } => {
                let Some((id, path)) = self.property_of(path) else { return };
                let label = format!("edit {}", last_step(&path));
                self.set_property(id, &label, &path, value);
            }
            Command::SetOption { path, on } => self.ask_property(path, |path| Asked::Toggle(path, *on)),
            Command::PickVariant { path, variant } => {
                self.ask_property(path, |path| Asked::Pick(path, variant.clone()));
            }
            Command::AddEntry(path) => self.ask_property(path, Asked::Add),
            Command::AddEntryAs { path, id } => {
                self.entry_id = Some(id.clone());
                self.ask_property(path, Asked::Add);
                self.entry_id = None;
            }
            Command::RemoveEntry(path) => self.ask_property(path, Asked::Remove),
            Command::SelectMove(name) => {
                if !self.select_move(name) {
                    self.refuse(format!("no move `{name}` on the timeline"));
                }
            }
            Command::Select(entry) => self.selected_entry = *entry,
            Command::Tick(tick) => self.set_tick(*tick),
            Command::SetWindow { entry, from, to } => {
                let (entry, from, to) = (*entry, *from, *to);
                self.edit("edit a window", |text| crate::edits::set_window(text, entry, from, to));
            }
            Command::SetInstant { entry, at } => {
                let (entry, at) = (*entry, *at);
                self.edit("move an instant", |text| crate::edits::set_instant(text, entry, at));
            }
            Command::SetPhases { mv, startup, active, recovery } => {
                let mv = *mv;
                let phases = crate::edits::Phases { startup: *startup, active: *active, recovery: *recovery };
                self.edit("edit the phases", |text| crate::edits::set_phases(text, mv, phases));
            }
            Command::SetValue { entry, path, value } => {
                let entry = *entry;
                self.edit(&format!("edit {path}"), |text| {
                    crate::edits::set_entry_value(text, entry, path, value)
                });
            }
            Command::Add(track) => self.add_to_track(*track),
            Command::Delete => self.delete_selected(),
            Command::StartAtPlayhead => self.set_selected_end_at_playhead(true),
            Command::EndAtPlayhead => self.set_selected_end_at_playhead(false),
            Command::Nudge(by) => self.nudge_selected(*by),
            Command::Play => self.play(self.now),
            Command::Stop => self.stop(),
            Command::Speed(speed) => self.set_speed(*speed),
            Command::Loop(on) => self.looping = *on,
            Command::AllTracks(on) => self.show_all_tracks = *on,
            Command::DummyDistance(distance) => self.set_dummy_distance(*distance),
            Command::WarpToDummy(on) => self.set_warp_to_dummy(*on),
            Command::View(facing) => {
                let distance = self.setup.dummy_distance;
                self.view = match facing {
                    Facing::Side => View::side(distance),
                    Facing::Front => View::front(distance),
                    Facing::Top => View::top(distance),
                };
                self.view_fit = ViewFit::Pending;
            }
            Command::Bones(on) => self.overlays.bones = *on,
            Command::Hurtboxes(on) => self.overlays.hurtboxes = *on,
            Command::SelectPlacement(id) => self.select_placement(id.as_deref()),
            Command::MigrateZone => self.migrate_zone(),
            Command::SelectObjects(picked) => self.select_objects(picked),
            Command::PickInZone { from, toward, reach, adding } => {
                self.pick_in_zone(*from, *toward, *reach, *adding);
            }
            Command::SelectInBox { view, from, to, adding } => self.select_in_box(view, *from, *to, *adding),
            Command::GizmoMode(mode) => self.set_gizmo_mode(*mode),
            Command::GizmoAxes(axes) => self.set_gizmo_axes(*axes),
            Command::Snap { grid, yaw } => self.set_steps(*grid, *yaw),
            Command::Grab { handle, from, toward } => self.grab(*handle, *from, *toward),
            Command::DragTo { from, toward } => self.drag_to(*from, *toward),
            Command::SnapToGround => self.snap_to_ground(),
            Command::Duplicate => self.duplicate(&[]),
            Command::DuplicateAs(ids) => self.duplicate(ids),
            Command::DeleteObjects => self.delete_objects(),
            Command::SelectField(picked) => self.select_field(picked),
            Command::SetShared { path, value } => match path.parse::<PropertyPath>() {
                Ok(path) => {
                    if self.take_typed() {
                        let result = self.set_shared(&path, value);
                        self.after_property_edit(self.current, result.map_err(|error| error.0));
                    }
                }
                Err(error) => self.refuse(format!("`{path}` isn't a path: {error}")),
            },
            Command::Scrub(frame) => self.scrub_to(*frame),
            Command::PlayTimeline => self.play_timeline(),
            Command::PauseTimeline => self.pause_timeline(),
            Command::MoveKey { key, to } => self.move_key(key, *to),
            Command::AddKey { lane, name, at } => self.add_key(lane, name.as_deref(), *at),
            Command::RemoveKey(key) => self.remove_key(key),
            Command::Aspect(aspect) => self.preview_at(*aspect),
            Command::Language(language) => {
                if !self.preview_in(language) {
                    self.refuse(format!("no language `{language}`: en or ja"));
                }
            }
            Command::StandIn { path, value } => {
                if !self.set_stand_in(path, value.clone()) {
                    self.refuse(format!("the screen has no path `{path}`"));
                }
            }
            Command::PlayInViewport(script) => self.play_in_viewport(script.clone()),
            Command::StopViewport => self.viewport.stop_playing(),
            Command::PlayInGame(target) => self.play_in_game(target),
            Command::StopGame => self.stop_game(),
            Command::OpenPane(pane) => self.shell.open(*pane),
            Command::ClosePane(pane) => self.shell.close(*pane),
            Command::Split { pane, of, direction } => match self.shell.find(*of) {
                Some(target) => self.shell.split(target, *pane, *direction),
                None => self.refuse(format!("no {} pane to split", of.title())),
            },
            Command::Link(_) if self.game.is_some() => {
                self.refuse("the link is the game's while it plays: StopGame first");
            }
            Command::Link(address) => match address.parse() {
                Ok(address) => self.set_link_address(address),
                Err(_) => self.refuse(format!("`{address}` isn't an address and port")),
            },
            Command::Cvar { name, value } => self.link.set_cvar(name, value.clone()),
            Command::WatchGraph(graph) => self.link.watch_graph(graph.as_deref()),
            Command::OverrideGraph { graph, parameter, value } => {
                self.link.override_graph(graph, Some((parameter, *value)));
            }
            Command::ClearOverrides(graph) => self.link.override_graph(graph, None),
        }
    }

    /// The current document and a path in it, or a refusal.
    fn property_of(&mut self, path: &str) -> Option<(sapphire_assets::AssetId, PropertyPath)> {
        let Some(id) = self.current else {
            self.refuse("no document to edit");
            return None;
        };
        match path.parse::<PropertyPath>() {
            Ok(path) => Some((id, path)),
            Err(error) => {
                self.refuse(format!("`{path}` isn't a path: {error}"));
                None
            }
        }
    }

    fn ask_property(&mut self, path: &str, asked: impl FnOnce(PropertyPath) -> Asked) {
        if let Some((id, path)) = self.property_of(path) {
            self.do_asked(id, asked(path));
        }
    }

    fn answer_close(&mut self, command: &Command) {
        let Some(id) = self.closing else {
            self.refuse("nothing's being closed");
            return;
        };
        match command {
            Command::SaveAndClose => match self.documents.save(id) {
                Ok(()) => {
                    self.ask_cook(&[id]);
                    self.close(id);
                }
                Err(error) => {
                    self.closing = None;
                    self.refuse(super::save_trouble(&error));
                }
            },
            Command::CloseWithoutSaving => self.close(id),
            _ => self.closing = None,
        }
    }

    fn answer_quit(&mut self, command: &Command) {
        if self.quit != Quit::Asking {
            self.refuse("nothing's asking to quit");
            return;
        }
        match command {
            Command::SaveAndQuit => match self.save_all() {
                Ok(()) => self.quit_now(),
                Err(text) => {
                    self.quit = Quit::No;
                    self.refuse(text);
                }
            },
            Command::QuitWithoutSaving => self.quit_now(),
            _ => self.quit = Quit::No,
        }
    }

    fn settle_conflict(&mut self, command: &Command) {
        let not_text = match self.current().and_then(|file| file.conflict()) {
            Some(conflict) => conflict == Conflict::NotText,
            None => {
                self.refuse("the file hasn't changed under edits");
                return;
            }
        };
        let Some(id) = self.current else { return };
        match command {
            Command::KeepMine => {
                self.documents.keep_mine(id);
            }
            _ if not_text => {
                self.refuse("the file isn't text, so there's nothing to take: keep yours");
            }
            Command::TakeFile => {
                self.typed = None;
                self.documents.take_disk(id);
                self.after_take();
            }
            _ => {
                self.typed = None;
                if let Some(rebased) = self.documents.rebase(id) {
                    let notice = super::top_bar::rebase_notice(&rebased);
                    self.notice = Some(notice);
                }
                self.after_take();
            }
        }
    }

    fn show_typed(&mut self) {
        let entry = match self.typed.as_ref().map(|typed| &typed.at) {
            Some(super::TypedAt::Entry { entry, .. }) => *entry,
            _ => {
                self.refuse("nothing typed belongs on the timeline");
                return;
            }
        };
        self.choose_move(entry.mv);
        self.selected_entry = Some(entry);
        self.shell.open(crate::shell::Pane::Inspector);
    }

    /// Ends every gesture under way: a drag let go, or a number's typing done. A gesture that edited several
    /// documents is one step of each, which undo takes back whole.
    fn end_gestures(&mut self) {
        self.drag_label = None;
        self.typing_number = None;
        self.zone.let_go();
        self.documents.end_gesture();
    }

    /// Takes back every gesture under way, to where each began.
    fn cancel_gestures(&mut self) {
        self.drag_label = None;
        self.typing_number = None;
        self.zone.let_go();
        if self.documents.cancel_gesture() {
            self.after_history();
        }
    }
}

/// What a path's last step is called, for its step of undo.
pub(super) fn last_step(path: &PropertyPath) -> String {
    match path.steps().iter().rev().find(|step| !matches!(step, Step::Some)) {
        Some(Step::Field(name) | Step::Key(name)) => name.clone(),
        Some(Step::Index(at)) => at.to_string(),
        Some(Step::Some) | None => path.to_string(),
    }
}

/// Whether the problems pane lists again after `command` (M6 T6): it opened, closed, saved or took back a document.
/// A cook's end lists again by itself.
fn lists_anew(command: &Command) -> bool {
    matches!(
        command,
        Command::Open(_)
            | Command::Save
            | Command::SaveAndPush
            | Command::SaveAll
            | Command::Close
            | Command::SaveAndClose
            | Command::CloseWithoutSaving
            | Command::SaveAndQuit
            | Command::TakeFile
            | Command::MineOnTop
            | Command::KeepMine
            | Command::Recover(_)
            | Command::SelectField(_)
    )
}
