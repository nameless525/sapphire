//! The editor's commands (M6 T1): every action a person takes, by a button, a menu, the end of a drag or a key,
//! is a [`Command`], run by [`crate::Editor::run`]. A command is the unit of action and of undo: what changes a
//! document is one step of its history, unless a [`Command::Begin`] and [`Command::End`] make a gesture of several,
//! as a drag does, frame by frame.
//!
//! A command is named, and its arguments are RON: `SetWindow(entry: (mv: 0, track: Hitboxes, index: 0), from: 7,
//! to: 13)`, as [`Command::parse`] reads it and a script writes it. The [`CommandRegistry`] says what each is
//! for, what its arguments look like and which key runs it, for the palette (Cmd+P) to offer.
//!
//! A [`Script`] is a list of commands in a versioned document, `<name>.commands.ron`, refusing what it doesn't
//! know: `sapphire_editor --run <script>` runs one with no window, and `--record <script>` writes the commands a
//! session runs, which then replays to the same bytes. Relative paths in one are from the script's folder.
//!
//! What only the screen holds isn't a command: a list's filter, a scroll, a collapsed row, the preview's orbit, a
//! value half typed (it becomes one when it lands in the text) and the palette itself.

use std::path::{Component, Path, PathBuf};

use egui_tiles::LinearDir;
use serde::{Deserialize, Serialize};

use crate::edits::{EntryRef, Track};
use crate::game::GameTarget;
use crate::handles::{Axes, GizmoMode, Handle};
use crate::screen::{Aspect, StandIn};
use crate::sequence::{KeyRef, Lane};
use crate::shell::Pane;

/// What a script's `version` must say.
pub const SCRIPT_VERSION: u32 = 1;

/// What a script's file name ends with.
pub const SCRIPT_SUFFIX: &str = ".commands.ron";

/// The preview's three views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Facing {
    Side,
    Front,
    Top,
}

/// An object a selection holds: a document, by its path, and a place in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Picked {
    pub document: PathBuf,
    /// A property path, as `placements[2]`, or empty for the whole document.
    pub path: String,
}

/// One action. The current document is the one the top bar names; the timeline's commands are on the document of
/// moves it shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Command {
    /// Opens a document as its kind, or goes to it if it's open.
    Open(PathBuf),
    /// Writes the current document.
    Save,
    /// Sends the current document's text to the device on live link.
    Push,
    /// Writes the current document and sends what was written (Cmd+S).
    SaveAndPush,
    /// Writes every document with edits.
    SaveAll,
    /// Takes back the current document's last step (Cmd+Z).
    Undo,
    /// Does again what the last undo took back (Cmd+Shift+Z).
    Redo,
    /// Closes the current document, asking first if that would lose edits.
    Close,
    /// Answers the close's question: saves, then closes.
    SaveAndClose,
    /// Answers the close's question: closes, losing the edits.
    CloseWithoutSaving,
    /// Answers the close's question: keeps the document open.
    CancelClose,
    /// Asks to quit, asking first if that would lose edits.
    Quit,
    /// Answers the quit's question: saves everything, then quits.
    SaveAndQuit,
    /// Answers the quit's question: quits, losing the edits.
    QuitWithoutSaving,
    /// Answers the quit's question: stays.
    CancelQuit,
    /// The file changed on disk under edits: takes the file's text.
    TakeFile,
    /// The file changed on disk under edits: takes the file's text and puts each edit that fits back on top.
    MineOnTop,
    /// The file changed on disk under edits: keeps these, to replace it at the next save.
    KeepMine,
    /// Takes back the unsaved text an editor that stopped kept in the journal of the document that was at this
    /// path: the document, dirty, or in conflict with a file that moved on since.
    Recover(PathBuf),
    /// Lets go of what the journal kept of the document that was at this path.
    DropRecovered(PathBuf),
    /// Drops a value typed that the document refused.
    DropTyped,
    /// Shows where a refused typed value belongs.
    ShowTyped,

    /// Starts a gesture on the current document: what follows until [`Command::End`] is one step of undo.
    Begin(String),
    /// Ends every gesture.
    End,
    /// Takes back the gesture under way on the current document, as Escape does a number's typing.
    Cancel,
    /// Sets a property of the current document, as RON spelled as typed.
    SetProperty { path: String, value: String },
    /// Turns an `Option` of the current document on or off.
    SetOption { path: String, on: bool },
    /// Picks an enum's variant in the current document.
    PickVariant { path: String, variant: String },
    /// Adds an entry, made from its type, to a list of the current document.
    AddEntry(String),
    /// Adds an entry to a zone's list with IDs, called `id` (M6 T8a): what a session records for an
    /// `AddEntry` there, with the ID it drew, so a replay makes the same entry. Refused if the zone has it.
    AddEntryAs { path: String, id: String },
    /// Removes an entry from a list of the current document.
    RemoveEntry(String),

    /// Selects a move of the timeline by name.
    SelectMove(String),
    /// Selects an entry on the timeline, or nothing.
    Select(Option<EntryRef>),
    /// Puts the timeline's playhead at a tick, and stops it (arrows, Home, End).
    Tick(u32),
    /// Sets a window's ticks.
    SetWindow { entry: EntryRef, from: u32, to: u32 },
    /// Sets an instant's tick.
    SetInstant { entry: EntryRef, at: u32 },
    /// Sets a move's three phases.
    SetPhases { mv: usize, startup: u32, active: u32, recovery: u32 },
    /// Sets one value of an entry, as RON spelled as typed.
    SetValue { entry: EntryRef, path: String, value: String },
    /// Adds an entry to a track of the selected move, at the playhead.
    Add(Track),
    /// Deletes the selected entry (Delete, Backspace).
    Delete,
    /// Puts the selected window's start at the playhead (`[`).
    StartAtPlayhead,
    /// Puts the selected window's end just past the playhead (`]`).
    EndAtPlayhead,
    /// Moves the selected entry by whole ticks (Alt+arrows).
    Nudge(i64),
    /// Plays the selected move from the playhead (Space).
    Play,
    /// Stops playing, where it is (Space).
    Stop,
    /// Plays at a speed: 1, 0.5, 0.25 or 0.1.
    Speed(f32),
    /// Plays over and over.
    Loop(bool),
    /// Shows every track, or only those with entries.
    AllTracks(bool),
    /// Stands the preview's dummy this far away, in metres.
    DummyDistance(f32),
    /// Bends warp windows toward the dummy.
    WarpToDummy(bool),
    /// Looks at the preview from a side, the front or the top.
    View(Facing),
    /// Draws the preview's bones.
    Bones(bool),
    /// Draws the preview's hurtboxes.
    Hurtboxes(bool),

    /// Selects a placement of the current zone by its ID, or nothing.
    SelectPlacement(Option<String>),
    /// Writes the current zone at its document's newest version, with the IDs it reads with (M6 T8a), and saves
    /// it. A zone already there is left as it is.
    MigrateZone,
    /// Selects these objects, in any documents open, in place of what was selected: none selects nothing.
    SelectObjects(Vec<Picked>),
    /// Selects the object of the current zone a ray from `from` toward `toward` picks (M6 T9a), reaching `reach`
    /// of how far along it about it, in place of what was selected, or beside it if `adding` (taking it out if it
    /// was in): what a click on the viewport runs. Picking nothing selects nothing, unless `adding`.
    PickInZone { from: [f32; 3], toward: [f32; 3], reach: f32, adding: bool },
    /// Selects the objects of the current zone a box holds (M6 T9a): every handle of each showing between `from`
    /// and `to`, in normalized device coordinates, of the view `view` projects by (a column-major matrix), in
    /// place of what was selected, or beside it if `adding`. What a Shift-drag on the viewport runs.
    SelectInBox { view: [f32; 16], from: [f32; 2], to: [f32; 2], adding: bool },
    /// Shows the gizmo's handles for moving, turning or scaling (M6 T9b).
    GizmoMode(GizmoMode),
    /// Points the gizmo's move arrows along the zone's axes or the object's own.
    GizmoAxes(Axes),
    /// Sets the steps a drag snaps to: `grid` in metres, `yaw` in degrees, 0 for none.
    Snap { grid: f32, yaw: f32 },
    /// Takes hold of a handle of the gizmo on the current zone's object selected last (M6 T9b), where a ray from
    /// `from` toward `toward` finds it, starting a gesture: what a press on a handle runs. Refused if the object
    /// has no such handle in the gizmo's mode, or the ray can't say where on it.
    Grab { handle: Handle, from: [f32; 3], toward: [f32; 3] },
    /// Drags the handle taken hold of to where a ray from `from` toward `toward` points, snapped: what each frame
    /// of a drag runs. [`Command::End`] lets go.
    DragTo { from: [f32; 3], toward: [f32; 3] },
    /// Puts each object of the current zone selected on the zone's colliders straight below it, as one step: a
    /// placement or an instance leaving its own colliders out of the ray (M6 T9b).
    SnapToGround,
    /// Copies each entry of the current zone selected, as one step, and selects the copies: each takes a fresh ID
    /// or name, and a placement's colliders are copied with it (M6 T9b).
    Duplicate,
    /// [`Command::Duplicate`] with the IDs given, in order, for the copies of entries with IDs: what a session
    /// records for a duplicate, with the IDs it drew, so a replay makes the same copies.
    DuplicateAs(Vec<String>),
    /// Takes out each entry of the current zone selected, and the colliders only they name, as one step.
    DeleteObjects,
    /// Goes to a field of a document, as the document's RON spells its path, opening the document if it isn't
    /// open: Properties marks the field's row. What a problem's row runs (M6 T6).
    SelectField(Picked),
    /// Sets the property at `path` under each selected object, as RON spelled as typed: one step of undo
    /// however many documents that touches.
    SetShared { path: String, value: String },
    /// Puts the current timeline's playhead at a frame of 60 a second.
    Scrub(u32),
    /// Plays the current timeline.
    PlayTimeline,
    /// Pauses the current timeline.
    PauseTimeline,
    /// Moves a key of the current timeline to start at a frame of 60 a second (M6 T16a): its time alone, or a
    /// span's two ends together. Refused onto or past a neighbour in its lane ([`crate::sequence`]).
    MoveKey { key: KeyRef, to: u32 },
    /// Adds a key to a lane of the current timeline at frame `at`, the pane's playhead: the value the lane holds
    /// there, or for a lane of named things (a face's preset, a cue, a play's row, a subtitle's Fluent key), `name`.
    AddKey { lane: Lane, name: Option<String>, at: u32 },
    /// Removes a key of the current timeline: from the focus lane, its camera key's focus.
    RemoveKey(KeyRef),
    /// Previews the current screen at an aspect.
    Aspect(Aspect),
    /// Previews the current screen in a language: `en` or `ja`.
    Language(String),
    /// Has a path of the current screen read a value, in the preview only.
    StandIn { path: String, value: StandIn },
    /// Plays the documents in the viewport, by a script of the game's if one's named, or by the keys.
    PlayInViewport(Option<String>),
    /// Stops what the viewport plays.
    StopViewport,
    /// Starts the game on a zone or a scenario, as a child process with no window, in place of any playing: the link
    /// goes to it, and every save goes to it while it plays (M6 T19).
    PlayInGame(GameTarget),
    /// Stops the game playing, and puts the link back where it was.
    StopGame,

    /// Opens a pane, or shows it if it's a tab away.
    OpenPane(Pane),
    /// Closes a pane.
    ClosePane(Pane),
    /// Splits the tile `of` is in, with `pane` as its second half, moved from wherever it was.
    Split { pane: Pane, of: Pane, direction: LinearDir },
    /// Points live link at another device: an address and port.
    Link(String),
    /// Sets a cvar on the device.
    Cvar { name: String, value: String },
    /// Follows what the device's character playing the graph of this file name is doing, or none (M6 A9).
    WatchGraph(Option<String>),
    /// Pins a parameter of the graph of this file name on the device, presentation only (M6 A9).
    OverrideGraph { graph: String, parameter: String, value: f32 },
    /// Lets every parameter of the graph of this file name on the device read what it reads again.
    ClearOverrides(String),
}

impl Command {
    /// Reads one command as a script writes it: `Undo`, `Tick(12)`, `SetProperty(path: "a.b", value: "2")`.
    pub fn parse(text: &str) -> Result<Self, String> {
        ron::from_str(text.trim()).map_err(|error| error.to_string())
    }

    /// The command as a script writes it.
    #[must_use]
    pub fn to_text(&self) -> String {
        ron::to_string(self).expect("a command is plain data")
    }

    /// The documents it names by their paths: what a script's folder is the start of.
    fn paths_mut(&mut self) -> Vec<&mut PathBuf> {
        match self {
            Self::Open(path) | Self::Recover(path) | Self::DropRecovered(path) => vec![path],
            Self::SelectObjects(picked) => picked.iter_mut().map(|picked| &mut picked.document).collect(),
            Self::SelectField(picked) => vec![&mut picked.document],
            _ => Vec::new(),
        }
    }

    /// Its name: what the registry knows it by.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Open(_) => "Open",
            Self::Save => "Save",
            Self::Push => "Push",
            Self::SaveAndPush => "SaveAndPush",
            Self::SaveAll => "SaveAll",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Close => "Close",
            Self::SaveAndClose => "SaveAndClose",
            Self::CloseWithoutSaving => "CloseWithoutSaving",
            Self::CancelClose => "CancelClose",
            Self::Quit => "Quit",
            Self::SaveAndQuit => "SaveAndQuit",
            Self::QuitWithoutSaving => "QuitWithoutSaving",
            Self::CancelQuit => "CancelQuit",
            Self::TakeFile => "TakeFile",
            Self::MineOnTop => "MineOnTop",
            Self::KeepMine => "KeepMine",
            Self::Recover(_) => "Recover",
            Self::DropRecovered(_) => "DropRecovered",
            Self::DropTyped => "DropTyped",
            Self::ShowTyped => "ShowTyped",
            Self::Begin(_) => "Begin",
            Self::End => "End",
            Self::Cancel => "Cancel",
            Self::SetProperty { .. } => "SetProperty",
            Self::SetOption { .. } => "SetOption",
            Self::PickVariant { .. } => "PickVariant",
            Self::AddEntry(_) => "AddEntry",
            Self::AddEntryAs { .. } => "AddEntryAs",
            Self::RemoveEntry(_) => "RemoveEntry",
            Self::SelectMove(_) => "SelectMove",
            Self::Select(_) => "Select",
            Self::Tick(_) => "Tick",
            Self::SetWindow { .. } => "SetWindow",
            Self::SetInstant { .. } => "SetInstant",
            Self::SetPhases { .. } => "SetPhases",
            Self::SetValue { .. } => "SetValue",
            Self::Add(_) => "Add",
            Self::Delete => "Delete",
            Self::StartAtPlayhead => "StartAtPlayhead",
            Self::EndAtPlayhead => "EndAtPlayhead",
            Self::Nudge(_) => "Nudge",
            Self::Play => "Play",
            Self::Stop => "Stop",
            Self::Speed(_) => "Speed",
            Self::Loop(_) => "Loop",
            Self::AllTracks(_) => "AllTracks",
            Self::DummyDistance(_) => "DummyDistance",
            Self::WarpToDummy(_) => "WarpToDummy",
            Self::View(_) => "View",
            Self::Bones(_) => "Bones",
            Self::Hurtboxes(_) => "Hurtboxes",
            Self::SelectPlacement(_) => "SelectPlacement",
            Self::MigrateZone => "MigrateZone",
            Self::SelectObjects(_) => "SelectObjects",
            Self::PickInZone { .. } => "PickInZone",
            Self::SelectInBox { .. } => "SelectInBox",
            Self::GizmoMode(_) => "GizmoMode",
            Self::GizmoAxes(_) => "GizmoAxes",
            Self::Snap { .. } => "Snap",
            Self::Grab { .. } => "Grab",
            Self::DragTo { .. } => "DragTo",
            Self::SnapToGround => "SnapToGround",
            Self::Duplicate => "Duplicate",
            Self::DuplicateAs(_) => "DuplicateAs",
            Self::DeleteObjects => "DeleteObjects",
            Self::SelectField(_) => "SelectField",
            Self::SetShared { .. } => "SetShared",
            Self::Scrub(_) => "Scrub",
            Self::PlayTimeline => "PlayTimeline",
            Self::PauseTimeline => "PauseTimeline",
            Self::MoveKey { .. } => "MoveKey",
            Self::AddKey { .. } => "AddKey",
            Self::RemoveKey(_) => "RemoveKey",
            Self::Aspect(_) => "Aspect",
            Self::Language(_) => "Language",
            Self::StandIn { .. } => "StandIn",
            Self::PlayInViewport(_) => "PlayInViewport",
            Self::StopViewport => "StopViewport",
            Self::PlayInGame(_) => "PlayInGame",
            Self::StopGame => "StopGame",
            Self::OpenPane(_) => "OpenPane",
            Self::ClosePane(_) => "ClosePane",
            Self::Split { .. } => "Split",
            Self::Link(_) => "Link",
            Self::Cvar { .. } => "Cvar",
            Self::WatchGraph(_) => "WatchGraph",
            Self::OverrideGraph { .. } => "OverrideGraph",
            Self::ClearOverrides(_) => "ClearOverrides",
        }
    }
}

/// What the registry knows of a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registered {
    pub name: &'static str,
    /// What it's for.
    pub help: &'static str,
    /// The command spelled out with arguments to fill in, or just its name if it takes none.
    pub template: &'static str,
    /// The key that runs it, if one does, as the palette shows it.
    pub key: Option<&'static str>,
}

impl Registered {
    /// Whether it runs as it is, with no arguments to fill in.
    #[must_use]
    pub fn takes_nothing(&self) -> bool {
        self.template == self.name
    }
}

macro_rules! registry {
    ($($name:literal, $template:literal, $key:expr, $help:literal;)*) => {
        &[$(Registered { name: $name, help: $help, template: $template, key: $key }),*]
    };
}

/// Every command, in the order the palette lists them.
const COMMANDS: &[Registered] = registry! {
    "Open", r#"Open("content/")"#, None, "Opens a document, or goes to it";
    "Save", "Save", None, "Writes the current document";
    "Push", "Push", None, "Sends the current document to the device";
    "SaveAndPush", "SaveAndPush", Some("Cmd+S"), "Writes the current document and sends it to the device";
    "SaveAll", "SaveAll", None, "Writes every document with edits";
    "Undo", "Undo", Some("Cmd+Z"), "Takes back the current document's last step";
    "Redo", "Redo", Some("Cmd+Shift+Z"), "Does again what undo took back";
    "Close", "Close", None, "Closes the current document";
    "SaveAndClose", "SaveAndClose", None, "Saves the document being closed, and closes it";
    "CloseWithoutSaving", "CloseWithoutSaving", None, "Closes the document being closed, losing its edits";
    "CancelClose", "CancelClose", None, "Keeps the document being closed open";
    "Quit", "Quit", None, "Quits, asking first if edits would be lost";
    "SaveAndQuit", "SaveAndQuit", None, "Saves everything, then quits";
    "QuitWithoutSaving", "QuitWithoutSaving", None, "Quits, losing the edits";
    "CancelQuit", "CancelQuit", None, "Stays, after a quit asked";
    "TakeFile", "TakeFile", None, "Takes the text of a file that changed on disk";
    "MineOnTop", "MineOnTop", None, "Takes the file's text and puts the edits that fit back on top";
    "KeepMine", "KeepMine", None, "Keeps the edits, to replace the file at the next save";
    "Recover", r#"Recover("content/")"#, None, "Takes back what an editor that stopped hadn't saved of a document";
    "DropRecovered", r#"DropRecovered("content/")"#, None,
        "Lets go of what an editor that stopped hadn't saved of a document";
    "DropTyped", "DropTyped", None, "Drops a typed value the document refused";
    "ShowTyped", "ShowTyped", None, "Shows where a refused typed value belongs";
    "Begin", r#"Begin("a gesture")"#, None, "Starts a gesture: one step of undo until End";
    "End", "End", None, "Ends the gesture";
    "Cancel", "Cancel", Some("Escape"), "Takes back the gesture under way";
    "SetProperty", r#"SetProperty(path: "", value: "")"#, None, "Sets a property of the current document";
    "SetOption", r#"SetOption(path: "", on: true)"#, None, "Turns an option on or off";
    "PickVariant", r#"PickVariant(path: "", variant: "")"#, None, "Picks an enum's variant";
    "AddEntry", r#"AddEntry("")"#, None, "Adds an entry to a list";
    "AddEntryAs", r#"AddEntryAs(path: "placements", id: "3fa91c07")"#, None,
        "Adds an entry to a zone's list with the ID given";
    "RemoveEntry", r#"RemoveEntry("")"#, None, "Removes an entry from a list";
    "SelectMove", r#"SelectMove("")"#, None, "Selects a move of the timeline";
    "Select", "Select(Some((mv: 0, track: Hitboxes, index: 0)))", None, "Selects an entry on the timeline";
    "Tick", "Tick(0)", Some("Left, Right, Home, End"), "Puts the playhead at a tick";
    "SetWindow", "SetWindow(entry: (mv: 0, track: Hitboxes, index: 0), from: 0, to: 1)", None,
        "Sets a window's ticks";
    "SetInstant", "SetInstant(entry: (mv: 0, track: Cues, index: 0), at: 0)", None, "Sets an instant's tick";
    "SetPhases", "SetPhases(mv: 0, startup: 1, active: 1, recovery: 1)", None, "Sets a move's phases";
    "SetValue", r#"SetValue(entry: (mv: 0, track: Hitboxes, index: 0), path: "", value: "")"#, None,
        "Sets a value of an entry";
    "Add", "Add(Cues)", None, "Adds an entry to a track at the playhead";
    "Delete", "Delete", Some("Delete"), "Deletes the selected entry";
    "StartAtPlayhead", "StartAtPlayhead", Some("["), "Puts the selected window's start at the playhead";
    "EndAtPlayhead", "EndAtPlayhead", Some("]"), "Puts the selected window's end at the playhead";
    "Nudge", "Nudge(1)", Some("Alt+Left, Alt+Right"), "Moves the selected entry by ticks";
    "Play", "Play", Some("Space"), "Plays the move from the playhead";
    "Stop", "Stop", Some("Space"), "Stops playing";
    "Speed", "Speed(1.0)", None, "Plays at a speed";
    "Loop", "Loop(true)", None, "Plays over and over";
    "AllTracks", "AllTracks(true)", None, "Shows every track";
    "DummyDistance", "DummyDistance(1.5)", None, "Stands the dummy this far away";
    "WarpToDummy", "WarpToDummy(true)", None, "Bends warp windows toward the dummy";
    "View", "View(Side)", None, "Looks at the preview from a side, the front or the top";
    "Bones", "Bones(true)", None, "Draws the preview's bones";
    "Hurtboxes", "Hurtboxes(true)", None, "Draws the preview's hurtboxes";
    "SelectPlacement", r#"SelectPlacement(Some("placement-0"))"#, None, "Selects a placement of the zone by its ID";
    "MigrateZone", "MigrateZone", None, "Writes the zone at its newest version, with its IDs, and saves it";
    "SelectObjects", r#"SelectObjects([(document: "content/", path: "")])"#, None,
        "Selects objects by document and path, in any documents open";
    "PickInZone", "PickInZone(from: (0.0, 10.0, 0.0), toward: (0.0, -1.0, 0.0), reach: 0.02, adding: false)", None,
        "Selects the zone's object a ray picks, as a click on the viewport does";
    "SelectInBox",
        "SelectInBox(view: (1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0), from: (-1.0, -1.0), to: (1.0, 1.0), adding: false)",
        None, "Selects the zone's objects a box on the view holds, as a Shift-drag on the viewport does";
    "GizmoMode", "GizmoMode(Turn)", None, "Shows the gizmo's handles for moving, turning or scaling";
    "GizmoAxes", "GizmoAxes(Local)", None, "Points the gizmo's arrows along the zone's axes or the object's own";
    "Snap", "Snap(grid: 0.05, yaw: 1.0)", None, "Sets the grid's and the yaw's steps a drag snaps to, 0 for none";
    "Grab", "Grab(handle: Move(X), from: (0.0, 10.0, 0.0), toward: (0.0, -1.0, 0.0))", None,
        "Takes hold of a handle of the gizmo, as a press on it does";
    "DragTo", "DragTo(from: (0.0, 10.0, 0.0), toward: (0.0, -1.0, 0.0))", None,
        "Drags the handle taken hold of, as a frame of a drag does";
    "SnapToGround", "SnapToGround", None, "Puts what's selected on the zone's colliders below it";
    "Duplicate", "Duplicate", None, "Copies what's selected in the zone, with its colliders";
    "DuplicateAs", r#"DuplicateAs(["3fa91c07"])"#, None, "Copies what's selected in the zone, with the IDs given";
    "DeleteObjects", "DeleteObjects", None, "Takes out what's selected in the zone, with the colliders only it names";
    "SelectField", r#"SelectField((document: "content/", path: ""))"#, None,
        "Goes to a field of a document, opening it if it isn't, as a problem's row does";
    "SetShared", r#"SetShared(path: "", value: "")"#, None,
        "Sets a property under every object selected, as one step of undo";
    "Scrub", "Scrub(0)", None, "Puts the timeline's playhead at a frame";
    "PlayTimeline", "PlayTimeline", None, "Plays the timeline";
    "PauseTimeline", "PauseTimeline", None, "Pauses the timeline";
    "MoveKey", "MoveKey(key: (lane: Camera, index: 0), to: 0)", None,
        "Moves a key of the timeline to a frame, as a drag in the sequencer does";
    "AddKey", "AddKey(lane: Camera, name: None, at: 0)", None, "Adds a key to a lane of the timeline at a frame";
    "RemoveKey", "RemoveKey((lane: Camera, index: 0))", None, "Removes a key of the timeline";
    "Aspect", "Aspect(Wide)", None, "Previews the screen at an aspect";
    "Language", r#"Language("en")"#, None, "Previews the screen in a language";
    "StandIn", r#"StandIn(path: "", value: Number(0.0))"#, None, "Has a screen's path read a value";
    "PlayInViewport", "PlayInViewport(None)", None, "Plays the documents in the viewport";
    "StopViewport", "StopViewport", None, "Stops what the viewport plays";
    "PlayInGame", r#"PlayInGame(Arena(script: "attempts"))"#, None,
        "Plays a zone or a scenario in the game, which takes every save";
    "StopGame", "StopGame", None, "Stops the game";
    "OpenPane", "OpenPane(Viewport)", None, "Opens a pane";
    "ClosePane", "ClosePane(Viewport)", None, "Closes a pane";
    "Split", "Split(pane: Device, of: Preview, direction: Vertical)", None, "Splits a pane's tile with another pane";
    "Link", r#"Link("127.0.0.1:7811")"#, None, "Points live link at a device";
    "Cvar", r#"Cvar(name: "", value: "")"#, None, "Sets a cvar on the device";
    "WatchGraph", r#"WatchGraph(Some("character.animgraph.ron"))"#, None,
        "Follows what a graph plays on the device";
    "OverrideGraph", r#"OverrideGraph(graph: "character.animgraph.ron", parameter: "speed", value: 0.0)"#, None,
        "Pins a graph's parameter on the device, presentation only";
    "ClearOverrides", r#"ClearOverrides("character.animgraph.ron")"#, None,
        "Lets a graph's parameters on the device go";
};

/// Every command the editor runs: its name, what it's for, its arguments and its key.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandRegistry;

impl CommandRegistry {
    /// Every command.
    #[must_use]
    pub const fn all(self) -> &'static [Registered] {
        COMMANDS
    }

    /// The command called `name`.
    #[must_use]
    pub fn get(self, name: &str) -> Option<&'static Registered> {
        COMMANDS.iter().find(|registered| registered.name == name)
    }

    /// The commands whose name or help holds every word of `filter`, ignoring case, in the registry's order.
    pub fn matching(self, filter: &str) -> impl Iterator<Item = &'static Registered> {
        let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
        COMMANDS.iter().filter(move |registered| {
            let said = format!("{} {}", registered.name, registered.help).to_lowercase();
            words.iter().all(|word| said.contains(word.as_str()))
        })
    }
}

/// A list of commands, as a `.commands.ron` file holds it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// [`SCRIPT_VERSION`].
    pub version: u32,
    pub commands: Vec<Command>,
}

impl Script {
    #[must_use]
    pub const fn new(commands: Vec<Command>) -> Self {
        Self { version: SCRIPT_VERSION, commands }
    }

    /// Reads a script's text. One of another version, or with anything it doesn't know, is refused.
    pub fn parse(text: &str) -> Result<Self, String> {
        let script: Self = ron::from_str(text).map_err(|error| error.to_string())?;
        if script.version != SCRIPT_VERSION {
            return Err(format!(
                "a script of version {}, where this editor reads version {SCRIPT_VERSION}",
                script.version
            ));
        }
        Ok(script)
    }

    /// Reads a script's file, with its relative paths made from its folder.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut script = Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let folder = path.parent().unwrap_or(Path::new(""));
        for command in &mut script.commands {
            for named in command.paths_mut() {
                if named.is_relative() {
                    *named = folder.join(&*named);
                }
            }
        }
        Ok(script)
    }

    /// The script's text, a command a line.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = format!("(\n    version: {},\n    commands: [\n", self.version);
        for command in &self.commands {
            text.push_str("        ");
            text.push_str(&command.to_text());
            text.push_str(",\n");
        }
        text.push_str("    ],\n)\n");
        text
    }

    /// The script as a file at `path` says it: each path opened from that file's folder where both have a root in
    /// common, so the file and the documents it opens move together. Relative paths are taken from where the editor
    /// runs.
    #[must_use]
    pub fn relative_to(&self, path: &Path) -> Self {
        let absolute = |path: &Path| std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        let folder =
            absolute(path.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new(".")));
        let commands = self
            .commands
            .iter()
            .map(|command| {
                let mut command = command.clone();
                for named in command.paths_mut() {
                    *named = relative(&absolute(named), &folder);
                }
                command
            })
            .collect();
        Self { version: self.version, commands }
    }
}

/// `path` from `folder`, both absolute, or `path` as it is if they share no root.
fn relative(path: &Path, folder: &Path) -> PathBuf {
    if !path.is_absolute() || !folder.is_absolute() {
        return path.to_owned();
    }
    let (to, from): (Vec<Component<'_>>, Vec<Component<'_>>) =
        (path.components().collect(), folder.components().collect());
    let shared = to.iter().zip(&from).take_while(|(a, b)| a == b).count();
    // Only the root in common (a drive and its root, on Windows): nothing's gained by climbing to it.
    let root = to.iter().take_while(|part| matches!(part, Component::Prefix(_) | Component::RootDir)).count();
    if shared <= root {
        return path.to_owned();
    }
    let mut relative = PathBuf::new();
    for _ in shared..from.len() {
        relative.push("..");
    }
    for part in &to[shared..] {
        relative.push(part.as_os_str());
    }
    relative
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_its_own_command() {
        let registry = CommandRegistry;
        for registered in registry.all() {
            let command = Command::parse(registered.template)
                .unwrap_or_else(|why| panic!("{}: {why}", registered.template));
            assert_eq!(command.name(), registered.name);
            assert_eq!(registry.get(registered.name), Some(registered));
            assert_eq!(Command::parse(&command.to_text()), Ok(command));
        }
        let names: std::collections::HashSet<_> =
            registry.all().iter().map(|registered| registered.name).collect();
        assert_eq!(names.len(), registry.all().len(), "a name once");
    }

    #[test]
    fn the_palette_finds_by_any_words() {
        let found: Vec<_> = CommandRegistry.matching("save ALL").map(|registered| registered.name).collect();
        assert_eq!(found, ["SaveAll"]);
        assert_eq!(CommandRegistry.matching("").count(), CommandRegistry.all().len());
    }

    #[test]
    fn a_script_reads_back_what_it_wrote_and_refuses_what_it_doesnt_know() {
        let entry = EntryRef { mv: 0, track: Track::Hitboxes, index: 0 };
        let script = Script::new(vec![
            Command::Open("content/combat/player/player.moves.ron".into()),
            Command::Begin("drag".to_owned()),
            Command::SetWindow { entry, from: 7, to: 13 },
            Command::End,
            Command::StandIn { path: "hp".to_owned(), value: StandIn::Text("a \"b\"".to_owned()) },
            Command::OpenPane(Pane::Viewport),
            Command::Undo,
        ]);
        assert_eq!(Script::parse(&script.to_text()), Ok(script));
        let unknown_field = "(version: 1, commands: [], by: \"me\")";
        assert!(Script::parse(unknown_field).is_err());
        let unknown_command = "(version: 1, commands: [Teleport])";
        assert!(Script::parse(unknown_command).is_err());
        let unknown_argument = "(version: 1, commands: [SetOption(path: \"a\", on: true, why: 1)])";
        assert!(Script::parse(unknown_argument).is_err());
        let later = Script::parse("(version: 2, commands: [])").expect_err("another version");
        assert!(later.contains("version 2"), "{later}");
    }

    #[test]
    fn a_recording_names_documents_from_its_own_folder() {
        // Absolute on every OS: a drive's root on Windows.
        let root = std::env::temp_dir();
        let root = root.ancestors().last().expect("a root");
        let repo = root.join("work").join("repo");
        let elsewhere = root.join("elsewhere").join("b.ui.ron");
        let script = Script::new(vec![
            Command::Open(repo.join("content").join("a.zone.ron")),
            Command::Open(elsewhere.clone()),
        ]);
        let written = script.relative_to(&repo.join("target").join("session.commands.ron"));
        assert_eq!(
            written.commands,
            [Command::Open(Path::new("..").join("content").join("a.zone.ron")), Command::Open(elsewhere)]
        );
        // A path given relative is from where the editor runs, as the script's own is.
        let relative = Script::new(vec![Command::Open("relative.moves.ron".into())]);
        let written = relative.relative_to(&Path::new("target").join("session.commands.ron"));
        assert_eq!(written.commands, [Command::Open(Path::new("..").join("relative.moves.ron"))]);
    }
}
