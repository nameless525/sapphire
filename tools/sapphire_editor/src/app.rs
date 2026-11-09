//! The editor's interface and state, everything but its window: [`Editor::ui`] draws into any `egui::Ui`,
//! so tests and [`crate::capture`] drive exactly what a person sees, with no window.
//!
//! Its panes are a [`Shell`] over one [`DocumentSet`]. The timeline shows one document of moves ([`Editor`]'s
//! `moves`), and the top bar's save and undo act on the current one, which is whichever was last opened or
//! edited: a timeline's edit makes its document current again. Nothing is lost by opening another document,
//! since the set holds what's unsaved in each, so only closing one or the window asks.

mod browser;
mod cvars;
mod properties;
mod screen;
mod sequencer;
mod zone;

pub use sequencer::RATE;
pub use zone::SNAP;

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use egui::{Color32, Key, Modifiers, RichText, Sense, TextEdit, Ui, Vec2};
use sapphire_assets::AssetId;
use sapphire_core::Name;
use sapphire_document::disk::Temporary;
use sapphire_document::{AnyDocument, DocumentSet, Kind, Noticed, PropertyPath, Rebased, SetError};
use sapphire_livelink::protocol::MoveAttempt;
use sapphire_moves::{MoveDocument, MoveSet, MoveSetDocument};
use sapphire_time::Stopwatch;

use crate::cooked::{self, Cook};
use crate::edits::{self, EditError, EntryRef, EntryValue, MovesDocument, Track};
use crate::figure::{Casting, Figure, Figures};
use crate::judge::judge;
use crate::link::{Link, Pushed};
use crate::play::{PlayRequest, Snapshot};
use crate::preview::{self, FigureCache, Overlays, PosedFigure, View};
use crate::run::{Frame, Run, Setup};
use crate::shell::{Pane, PaneUi, Shell};
use crate::timeline::{self, Action, Drag, Part, PhaseEdge, Problem, TimelineView};
use crate::viewport::{ViewportFrame, ViewportPane};

/// How often the file and the cook are looked at again.
const POLL_SECONDS: f64 = 1.0;
/// How often what's under the roots is looked at again, for the browser and for a copy of what's open. A look
/// at this repository takes about 15 ms in a debug build, so it isn't for every frame, and it waits for a drag.
const RESCAN_SECONDS: f64 = 5.0;
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

/// One successful authored/compiled preview. Invalid edits never mix new casting with old tables.
#[derive(Debug)]
struct PreviewSnapshot {
    document: MoveSetDocument,
    set: MoveSet,
}

impl PreviewSnapshot {
    fn move_at(&self, name: Option<&str>, index: usize) -> Option<&MoveDocument> {
        // Names survive reordering. A renamed/removed move falls back to its prior slot, then first.
        self.document
            .moves
            .iter()
            .find(|mv| Some(mv.name.as_str()) == name)
            .or_else(|| self.document.moves.get(index))
            .or_else(|| self.document.moves.first())
    }
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
    /// Every pack identity at the last attempt, including a failed read to avoid idle retry loops.
    tried: Option<Result<Vec<cooked::PackStamp>, String>>,
}

/// The document the top bar acts on, as tests and the top bar read it.
#[derive(Clone, Copy)]
pub struct Current<'a> {
    id: AssetId,
    document: &'a dyn AnyDocument,
    path: &'a Path,
    conflict: Option<&'a [u8]>,
}

/// What the file says, where that's waiting to be settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict<'a> {
    Text(&'a str),
    /// It isn't text any more: there's nothing to take, and only keeping what's here gets past it.
    NotText,
}

impl<'a> Current<'a> {
    #[must_use]
    pub const fn id(&self) -> AssetId {
        self.id
    }

    #[must_use]
    pub fn text(&self) -> &'a str {
        self.document.text()
    }

    #[must_use]
    pub const fn document(&self) -> &'a dyn AnyDocument {
        self.document
    }

    #[must_use]
    pub const fn path(&self) -> &'a Path {
        self.path
    }

    #[must_use]
    pub fn file_name(&self) -> String {
        self.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.document.is_dirty()
    }

    #[must_use]
    pub fn in_gesture(&self) -> bool {
        self.document.in_gesture()
    }

    #[must_use]
    pub fn undo_label(&self) -> Option<&'a str> {
        self.document.undo_label()
    }

    #[must_use]
    pub fn redo_label(&self) -> Option<&'a str> {
        self.document.redo_label()
    }

    /// What the file says, if it changed on disk and that's waiting to be settled.
    #[must_use]
    pub fn conflict(&self) -> Option<Conflict<'a>> {
        let bytes = self.conflict?;
        Some(std::str::from_utf8(bytes).map_or(Conflict::NotText, Conflict::Text))
    }
}

/// The whole editor.
#[derive(Debug)]
pub struct Editor {
    documents: DocumentSet,
    /// The document of moves the timeline shows.
    moves: Option<AssetId>,
    /// The document the top bar acts on.
    current: Option<AssetId>,
    shell: Shell,
    /// Where the layout lasts, and what it last said there, or nothing until a pass has drawn it: the first
    /// tidies the tree, which is no change anyone made.
    layout: Option<(PathBuf, Option<egui_tiles::Tree<Pane>>)>,
    browser: browser::Browser,
    properties: properties::PropertiesPanel,
    cvar_panel: cvars::CvarPanel,
    zone: zone::ZonePanel,
    sequencer: sequencer::SequencerPanel,
    screen: screen::ScreenPanel,
    now: f64,
    content: Option<PathBuf>,
    cook: CookState,
    /// The text the latest diagnostics were made from.
    judged: Option<String>,
    issues: Vec<sapphire_moves::Issue>,
    /// The authored source and compiled set of one successful preview, replaced together.
    preview: Option<PreviewSnapshot>,
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
    view_fit: ViewFit,
    overlays: Overlays,
    cache: FigureCache,
    drag: Option<Drag>,
    /// The label of the gesture the timeline's drag began, while it's held: an undo or a save ends a gesture, and
    /// the rest of a drag still held is one of its own.
    drag_label: Option<String>,
    show_all_tracks: bool,
    link: Link,
    address_text: String,
    notice: Option<Notice>,
    last_poll: f64,
    next_rescan: f64,
    quit: Quit,
    /// A close the window couldn't ask about has been heard since the last pass.
    closed_unseen: bool,
    /// A document to close, with edits unsaved: the top bar asks first.
    closing: Option<AssetId>,
    /// What's being typed in the inspector, which is on the screen and not yet in the text.
    typed: Option<Typed>,
    /// The number that has the keyboard, and its document, whose typing is one gesture: unlike a name it lands
    /// in the text key by key, since only its widget holds what's typed, and a save has to find it there.
    typing_number: Option<(egui::Id, AssetId)>,
    viewport: ViewportPane,
}

/// What's being typed over.
#[derive(Clone, Debug)]
pub(crate) enum TypedAt {
    /// A value of an entry on the timeline, by the path [`edits::entry_values`] gives it.
    Entry { entry: EntryRef, path: String },
    /// A property of any document.
    Property { document: AssetId, path: PropertyPath, label: String },
}

impl TypedAt {
    fn label(&self) -> &str {
        match self {
            Self::Entry { path, .. } => path,
            Self::Property { label, .. } => label,
        }
    }
}

/// A value being typed in an inspector. It goes into the text when its field lets go, or sooner if
/// anything needs the text whole, so half a name is no edit and a field's whole session is one step of
/// undo. Meanwhile it counts as an edit ([`Editor::is_dirty`]) and the text holds still under it: every
/// other edit, undo, redo and a save take it first, and a change on disk waits.
#[derive(Clone, Debug)]
pub(crate) struct Typed {
    at: TypedAt,
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

#[derive(Clone, Copy, Debug)]
enum ViewFit {
    Pending,
    Fitted(Vec2),
    Manual,
}

impl Editor {
    #[must_use]
    pub fn new(options: Options) -> Self {
        let address = options.link.unwrap_or_else(|| DEFAULT_LINK.parse().expect("a good default"));
        let mut editor = Self {
            documents: DocumentSet::new(Vec::new()),
            moves: None,
            current: None,
            shell: Shell::new(),
            layout: None,
            browser: browser::Browser::default(),
            properties: properties::PropertiesPanel::default(),
            cvar_panel: cvars::CvarPanel::default(),
            zone: zone::ZonePanel::default(),
            sequencer: sequencer::SequencerPanel::default(),
            screen: screen::ScreenPanel::default(),
            now: 0.0,
            content: options.content,
            cook: CookState::default(),
            judged: None,
            issues: Vec::new(),
            preview: None,
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
            view_fit: ViewFit::Pending,
            overlays: Overlays::default(),
            cache: FigureCache::default(),
            drag: None,
            drag_label: None,
            show_all_tracks: false,
            link: Link::new(Some(address)),
            address_text: address.to_string(),
            notice: None,
            last_poll: 0.0,
            next_rescan: RESCAN_SECONDS,
            quit: Quit::No,
            closed_unseen: false,
            closing: None,
            typed: None,
            typing_number: None,
            viewport: ViewportPane::default(),
        };
        if let Some(path) = options.file {
            editor.open(&path);
        }
        // With no file named, or none there, the browser is the way to one, so it looks around where it started.
        if editor.current.is_none()
            && let Ok(dir) = std::env::current_dir()
        {
            editor.add_roots_around(&dir, false);
        }
        if let Err(error) = editor.documents.rescan() {
            tracing::warn!(%error, "couldn't look at what's under the roots");
        }
        editor
    }

    /// Keeps the layout in a file from now on, starting from what's there. One that doesn't read is left there
    /// until the layout changes, and the notice says so.
    pub fn keep_layout_at(&mut self, path: PathBuf) {
        match fs::read_to_string(&path) {
            Ok(saved) => {
                if let Err(error) = self.shell.restore_layout(&saved) {
                    self.notice = Some(Notice {
                        text: format!("{}: {error}: starting from the usual layout", path.display()),
                        trouble: true,
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                self.notice = Some(Notice { text: format!("{}: {error}", path.display()), trouble: true });
            }
        }
        self.layout = Some((path, None));
    }

    /// Writes the layout if it's changed, whole. It's a convenience, so trouble with it is only logged.
    fn persist_layout(&mut self) {
        let Some((path, Some(saved))) = &mut self.layout else { return };
        if *saved == self.shell.tree {
            return;
        }
        let text = match self.shell.save_layout() {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(%error, "the layout wasn't kept");
                return;
            }
        };
        if let Err(error) = Temporary::beside(path, &text).and_then(|temporary| temporary.replace(path)) {
            tracing::warn!(%error, path = %path.display(), "the layout wasn't kept");
        } else {
            saved.clone_from(&self.shell.tree);
        }
    }

    /// The shell, for a test to arrange.
    pub fn shell_mut(&mut self) -> &mut Shell {
        &mut self.shell
    }

    #[must_use]
    pub const fn shell(&self) -> &Shell {
        &self.shell
    }

    /// Adds the roots a document at `place` needs: the repository it's in, if it's in one, and, for a file,
    /// its own folder, so a document anywhere has its neighbours looked at. The set says what overlaps.
    fn add_roots_around(&mut self, place: &Path, is_file: bool) {
        let mut roots = self.documents.roots().to_vec();
        let add = |roots: &mut Vec<PathBuf>, root: PathBuf| {
            if !roots.contains(&root) {
                roots.push(root);
            }
        };
        if let Some(root) = repository_around(place) {
            for folder in REPOSITORY_ROOTS {
                add(&mut roots, root.join(folder));
            }
        }
        if is_file && let Some(folder) = place.parent() {
            add(&mut roots, folder.to_owned());
        }
        self.documents.set_roots(roots);
    }

    /// Opens a document, or goes to it if it's open already. On failure what's open stays open, and the notice
    /// says why. What's being typed goes in first, or nothing opens until it's fixed or dropped.
    pub fn open(&mut self, path: &Path) {
        if !self.take_typed() {
            self.notice = Some(Notice {
                text: "nothing opens until what's being typed is fixed or dropped".to_owned(),
                trouble: true,
            });
            return;
        }
        let absolute;
        let path = if path.is_absolute() {
            path
        } else {
            // The browser lists what's under the roots, which are absolute: a row names one file once.
            absolute = std::env::current_dir().map_or_else(|_| path.to_owned(), |dir| dir.join(path));
            &absolute
        };
        self.add_roots_around(path, true);
        let kind = Kind::of(path).unwrap_or(Kind::Moves);
        match kind.open(&mut self.documents, path) {
            Ok(id) => {
                // It may be open from before, with a file that's moved on.
                self.notice = None;
                self.go_to(id);
                let noticed = self.documents.refresh();
                self.say_noticed(noticed);
                if kind == Kind::Moves {
                    self.show_on_timeline(id, path);
                }
                if matches!(kind, Kind::Zone | Kind::Timeline | Kind::Ui) && self.content.is_none() {
                    self.content = find_cook(path);
                }
                match kind {
                    Kind::Zone => self.shell.open(Pane::Zone),
                    Kind::Timeline => self.shell.open(Pane::Sequencer),
                    Kind::Ui => self.shell.open(Pane::Screen),
                    _ => {}
                }
            }
            Err(error) => self.notice = Some(Notice { text: error.to_string(), trouble: true }),
        }
    }

    /// Makes `id` the current document.
    fn go_to(&mut self, id: AssetId) {
        if self.current != Some(id) {
            self.current = Some(id);
            // What a device last said was about another file.
            self.link.clear_push();
        }
    }

    /// Puts a document of moves on the timeline.
    fn show_on_timeline(&mut self, id: AssetId, path: &Path) {
        if self.moves == Some(id) {
            self.refresh();
            return;
        }
        self.moves = Some(id);
        if self.content.is_none() {
            self.content = find_cook(path);
        }
        self.reset_timeline();
        self.load_cook();
        self.refresh();
    }

    fn reset_timeline(&mut self) {
        self.view_fit = ViewFit::Pending;
        self.selected_move = 0;
        self.selected_entry = None;
        self.tick = 0;
        self.play_from = 0;
        self.playing = None;
        self.judged = None;
        self.issues.clear();
        self.preview = None;
        self.stale = false;
        self.run = None;
        self.run_error = None;
    }

    /// Stops holding the current document, asking first if that would lose edits.
    pub fn request_close(&mut self) {
        if !self.take_typed() {
            return;
        }
        let Some(id) = self.current else { return };
        if self.documents.is_dirty(id) {
            self.closing = Some(id);
        } else {
            self.close(id);
        }
    }

    fn close(&mut self, id: AssetId) {
        self.closing = None;
        self.documents.close(id);
        self.screen.forget(id);
        if self.moves == Some(id) {
            self.moves = None;
            self.reset_timeline();
            self.cook.casting = None;
            self.cook.set = None;
        }
        if self.current == Some(id) {
            self.current = self.moves.or_else(|| self.documents.ids().next());
            self.link.clear_push();
        }
    }

    /// Says what a look at the files found, of anything that isn't shown already.
    fn say_noticed(&mut self, noticed: Vec<Noticed>) {
        for noticed in noticed {
            let name_of = |id: AssetId, documents: &DocumentSet| {
                documents
                    .path(id)
                    .and_then(Path::file_name)
                    .map_or_else(|| id.to_string(), |name| name.to_string_lossy().into_owned())
            };
            let (text, trouble) = match &noticed {
                Noticed::Taken { .. } => continue,
                // The top bar shows the current document's.
                Noticed::Conflict { id } if self.current == Some(*id) => continue,
                Noticed::Conflict { id } => (
                    format!("{} changed on disk, and you have edits there", name_of(*id, &self.documents)),
                    true,
                ),
                Noticed::Moved { from, to, .. } => {
                    (format!("{} is at {} now", from.display(), to.display()), false)
                }
                Noticed::Gone { id } => (
                    format!("{} is gone from disk: a save writes it again", name_of(*id, &self.documents)),
                    true,
                ),
                Noticed::Duplicated { first, second, .. } => (
                    format!(
                        "{} and {} hold the same ID: delete the sidecar of the copy",
                        first.display(),
                        second.display()
                    ),
                    true,
                ),
            };
            self.notice = Some(Notice { text, trouble });
        }
    }

    fn load_cook(&mut self) {
        let generation = self.cook.generation + 1;
        self.cook = CookState { generation, ..CookState::default() };
        self.view_fit = ViewFit::Pending;
        let Some(dir) = self.content.clone() else {
            self.cook.why_none = Some("no cook found: `cargo xtask content` makes one".to_owned());
            return;
        };
        self.cook.tried = Some(cooked::pack_stamp(&dir));
        match Cook::open(&dir) {
            Ok(cook) => {
                if let Some(path) = self.moves.and_then(|id| self.documents.path(id)) {
                    match cook.move_set_of(path) {
                        Ok(set) => self.cook.set = set,
                        Err(why) => self.cook.why_none = Some(why),
                    }
                    match Figures::for_document(path, &cook) {
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

    /// The document the top bar acts on.
    #[must_use]
    pub fn current(&self) -> Option<Current<'_>> {
        let id = self.current?;
        Some(Current {
            id,
            document: self.documents.any(id)?,
            path: self.documents.path(id)?,
            conflict: self.documents.conflict(id),
        })
    }

    /// Every document open.
    #[must_use]
    pub const fn documents(&self) -> &DocumentSet {
        &self.documents
    }

    /// Every document open, for what a test does as a person would by hand.
    pub fn documents_mut(&mut self) -> &mut DocumentSet {
        &mut self.documents
    }

    /// Whether there's anything here the files don't have: edits in any document, or a value still being typed.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.typed.is_some() || self.documents.ids().any(|id| self.documents.is_dirty(id))
    }

    /// What a viewport's engine half takes from a pass that's just run, before egui paints it.
    pub fn viewport_frame(&mut self) -> ViewportFrame<'_> {
        let moves = self.moves.and_then(|id| self.documents.path(id)).and_then(Path::file_name);
        let current = self.current;
        let zone = zone::zone_shown(&self.documents, &self.zone, current, self.viewport.camera());
        let timeline = sequencer::timeline_shown(&self.documents, &mut self.sequencer, current);
        self.screen.follow(&self.documents, current);
        let screen = screen::screen_shown(&self.documents, &self.screen, current);
        ViewportFrame {
            open: self.shell.is_open(Pane::Viewport),
            content: self.content.as_deref(),
            moves: moves
                .and_then(|name| name.to_str())
                .zip(self.preview.as_ref().map(|preview| &preview.set)),
            zone,
            timeline,
            screen,
            now: self.now,
            pane: &mut self.viewport,
        }
    }

    /// Asks the viewport for a run from the documents as they stand now, whatever's being typed included, played by
    /// `script` if there's one and otherwise by the keys. What can't be played says why in the pane.
    pub fn play_in_viewport(&mut self, script: Option<String>) {
        if !self.take_typed() {
            self.viewport.refuse("What's being typed doesn't fit, so it can't be played".to_owned());
            return;
        }
        let taken = crate::play::fight_documents(self.content.as_deref())
            .and_then(|fight| Snapshot::take(&self.documents, self.cook.cook.as_ref(), fight));
        match taken {
            Ok(snapshot) => self.viewport.ask(PlayRequest { snapshot, script }),
            Err(why) => self.viewport.refuse(format!("Can't play {why}")),
        }
    }

    /// The viewport's pane, which holds nothing of the GPU's but a texture's name.
    pub fn viewport(&mut self) -> &mut ViewportPane {
        &mut self.viewport
    }

    /// The document on the timeline.
    #[must_use]
    pub fn moves_document(&self) -> Option<&MovesDocument> {
        self.documents.get(self.moves?)
    }

    /// The document on the timeline, for edits a test makes as a person would by hand.
    pub fn moves_document_mut(&mut self) -> Option<&mut MovesDocument> {
        self.documents.get_mut(self.moves?)
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
        self.moves_document()?.last_good()
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
            self.view_fit = ViewFit::Pending;
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
        self.now = now;
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
        self.now = now;
        let Some(playing) = self.playing else { return };
        let Some(run) = &self.run else { return };
        let steps = ((now - playing.started).max(0.0) * 60.0 * f64::from(self.speed)) as usize;
        let frame = playing.first_frame.saturating_add(steps);
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

    /// Changes playback rate while retaining its current phase. Nonpositive/nonfinite rates are ignored.
    pub fn set_speed(&mut self, speed: f32) {
        if !speed.is_finite() || speed <= 0.0 {
            return;
        }
        if let Some(playing) = &mut self.playing {
            let elapsed = (self.now - playing.started).max(0.0) * f64::from(self.speed);
            playing.started = self.now - elapsed / f64::from(speed);
        }
        self.speed = speed;
    }

    /// Sets how far the dummy stands, which plays the move again.
    pub fn set_dummy_distance(&mut self, distance: f32) {
        self.setup.dummy_distance = distance;
        if !matches!(self.view_fit, ViewFit::Manual) {
            self.view_fit = ViewFit::Pending;
        }
        self.remake_run();
    }

    /// Sets whether warp windows bend toward the dummy, which plays the move again.
    pub fn set_warp_to_dummy(&mut self, warp: bool) {
        self.setup.warp_to_dummy = warp;
        if !matches!(self.view_fit, ViewFit::Manual) {
            self.view_fit = ViewFit::Pending;
        }
        self.remake_run();
    }

    /// Runs the document's checks and the preview's run again if the text has changed.
    pub fn refresh(&mut self) {
        let Some(file) = self.moves_document() else { return };
        if self.judged.as_deref() == Some(file.text()) {
            return;
        }
        let text = file.text().to_owned();
        let changed_before = self.judged.is_some();
        match file.value() {
            Some(document) => {
                let verdict = judge(document, self.cook.set.as_ref());
                let snapshot = verdict.set.map(|set| PreviewSnapshot { set, document: document.clone() });
                self.issues = verdict.issues;
                self.stale = snapshot.is_none();
                if let Some(snapshot) = snapshot {
                    self.preview = Some(snapshot);
                }
            }
            None => {
                self.issues.clear();
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
        let Some(snapshot) = &self.preview else {
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
        let Some(mv) = snapshot.move_at(self.selected_move_name(), self.selected_move) else {
            self.run = None;
            return;
        };
        self.cook.casting = None;
        if let Some(cook) = &self.cook.cook {
            match self.cook.figures.cast(mv, cook) {
                Ok(casting) => self.cook.casting = casting,
                Err(why) => self.run_error = Some(why),
            }
        }
        match Run::simulate(&snapshot.set, &mv.name, self.setup) {
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

    /// Saves the current document, then pushes what was saved. A file with nothing new is pushed all the same:
    /// a device may have started since.
    pub fn save_and_push(&mut self) {
        match self.save() {
            Ok(()) => {
                let Some(file) = self.current() else { return };
                let (name, text) = (file.file_name(), file.text().to_owned());
                self.notice = Some(Notice { text: format!("saved {name}"), trouble: false });
                self.link.push(name, text);
            }
            Err(text) => self.notice = Some(Notice { text, trouble: true }),
        }
    }

    /// Writes the current document, with what's being typed in it. What goes wrong is in a notice's words.
    fn save(&mut self) -> Result<(), String> {
        if !self.take_typed() {
            return Err("not saved: what's being typed can't go in".to_owned());
        }
        let Some(id) = self.current else { return Ok(()) };
        self.documents.save(id).map_err(|error| save_trouble(&error))
    }

    /// Writes every document with edits, with what's being typed in its own. What goes wrong is in a notice's
    /// words, and what can be saved is.
    fn save_all(&mut self) -> Result<(), String> {
        if !self.take_typed() {
            return Err("not saved: what's being typed can't go in".to_owned());
        }
        let mut failed = self.documents.save_all();
        match failed.pop() {
            None => Ok(()),
            Some((_, error)) if failed.is_empty() => Err(save_trouble(&error)),
            Some((_, error)) => {
                Err(format!("{} documents weren't saved: {}", failed.len() + 1, save_trouble(&error)))
            }
        }
    }

    /// Puts what's being typed into the text, as one step of undo. False if the text won't have it: it
    /// stays in its field, and the top bar says why.
    fn take_typed(&mut self) -> bool {
        let Some(typed) = self.typed.take() else { return true };
        let label = format!("edit {}", typed.at.label());
        let result: Result<(), EditError> = match &typed.at {
            TypedAt::Entry { entry, path } => {
                let Some(document) = self.moves_document_mut() else { return true };
                document
                    .together(&label, |document| edits::set_entry_value(document, *entry, path, &typed.text))
            }
            TypedAt::Property { document, path, .. } => {
                let Some(document) = self.documents.any_mut(*document) else { return true };
                document.set_spelled(&label, path, &typed.text).map(|_| ()).map_err(EditError::from)
            }
        };
        match result {
            Ok(()) => {
                self.notice = None;
                self.link.clear_push();
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
    /// row going with the selection, which leaves the field no pass to see it in. Escape takes back what
    /// was typed, refused or not, and a refused value otherwise waits for someone to change it.
    fn settle_typed(&mut self, ui: &Ui) {
        let Some(typed) = &self.typed else { return };
        if ui.memory(|memory| memory.has_focus(typed.field)) {
            return;
        }
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            self.typed = None;
        } else if typed.refused.is_none() {
            self.take_typed();
        }
    }

    /// A drag whose button went away with the window's focus ends here, so the next edit isn't part of it.
    /// So does a number's typing, when its field lets go, which Escape takes back, or at a press, since a
    /// field keeps the keyboard until the click ends and what the click does is another step.
    fn settle_gesture(&mut self, ui: &Ui) {
        if let Some((field, id)) = self.typing_number {
            if ui.memory(|memory| memory.has_focus(field)) && !ui.input(|input| input.pointer.any_pressed()) {
                return;
            }
            self.typing_number = None;
            let taken_back = ui.input(|input| input.key_pressed(Key::Escape));
            if let Some(file) = self.documents.any_mut(id) {
                if taken_back {
                    file.cancel_gesture();
                } else {
                    file.end_gesture();
                }
            }
            if taken_back {
                self.after_history();
            }
        }
        if ui.input(|input| input.pointer.any_down()) {
            return;
        }
        self.drag = None;
        self.drag_label = None;
        let ids: Vec<AssetId> = self
            .documents
            .ids()
            .filter(|id| self.documents.any(*id).is_some_and(|file| file.in_gesture()))
            .collect();
        for id in ids {
            if let Some(file) = self.documents.any_mut(id)
                && file.in_gesture()
            {
                file.end_gesture();
            }
        }
    }

    /// The window was asked to close: it goes at once, unless that would lose edits.
    pub fn request_quit(&mut self) {
        self.quit = if self.is_dirty() { Quit::Asking } else { Quit::Yes };
    }

    /// The window was asked to close and can't draw, so the question it would ask can't be seen. Edits are
    /// saved rather than lost unseen. If the file won't take them the log says why and it stays open, in
    /// case that's the one thing to put right, and a second close goes anyway.
    ///
    /// A second close comes after a pass. SDL follows the close of its last window with a quit in the same
    /// pump, so one click calls this twice, and every event wakes a pass, so the next click comes after one.
    pub fn request_quit_unseen(&mut self) {
        if !self.is_dirty() {
            self.quit = Quit::Yes;
            return;
        }
        if std::mem::replace(&mut self.closed_unseen, true) {
            return;
        }
        self.quit = match self.save_all() {
            Ok(()) => Quit::Yes,
            Err(_) if self.quit == Quit::Asking => {
                tracing::error!(
                    "closing with no GPU to ask on, and the edits couldn't be saved: they're lost"
                );
                Quit::Yes
            }
            Err(why) => {
                tracing::error!(%why, "closing with no GPU to ask on, and the edits couldn't be saved: close again to lose them");
                self.notice = Some(Notice { text: why, trouble: true });
                Quit::Asking
            }
        };
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
        let Some(file) = self.current.and_then(|id| self.documents.any_mut(id)) else { return };
        if file.undo() {
            self.after_history();
        } else if let Some(why) = file.why_no_undo() {
            // At history's floor: it's not that nothing was ever done, and the file doesn't have this text.
            self.notice = Some(Notice { text: why, trouble: false });
        }
    }

    pub fn redo(&mut self) {
        if self.take_typed()
            && let Some(file) = self.current.and_then(|id| self.documents.any_mut(id))
            && file.redo()
        {
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.notice = None;
        self.link.clear_push();
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
        edit: impl FnOnce(&mut MovesDocument) -> Result<T, EditError>,
    ) -> Option<T> {
        if !self.take_typed() {
            return None;
        }
        let id = self.moves?;
        self.go_to(id);
        let file = self.moves_document_mut()?;
        match file.together(label, edit) {
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
                self.drag_label = Some(label);
            }
            Action::EndGesture => {
                self.drag_label = None;
                if let Some(file) = self.moves_document_mut() {
                    file.end_gesture();
                }
            }
            Action::SetWindow { entry, from, to, part } => {
                self.resume_drag();
                self.edit("edit a window", |text| edits::set_window(text, entry, from, to));
                // The preview shows the tick the drag has hold of.
                let tick = match part {
                    Part::Start | Part::Body => from,
                    Part::End => to.saturating_sub(1),
                };
                self.set_tick(tick);
            }
            Action::SetInstant { entry, at } => {
                self.resume_drag();
                self.edit("move an instant", |text| edits::set_instant(text, entry, at));
                self.set_tick(at);
            }
            Action::SetPhases { mv, phases, edge } => {
                self.resume_drag();
                self.edit("edit the phases", |text| edits::set_phases(text, mv, phases));
                let tick = match edge {
                    PhaseEdge::StartupEnd => phases.startup,
                    PhaseEdge::ActiveEnd => phases.startup.saturating_add(phases.active),
                    PhaseEdge::MoveEnd => {
                        phases.startup.saturating_add(phases.active).saturating_add(phases.recovery)
                    }
                };
                self.set_tick(tick);
            }
            Action::Add(track) => self.add_to_track(track),
        }
    }

    /// A drag still held whose gesture an undo or a save ended goes on as a gesture of its own, so the rest of it is
    /// one step of undo rather than one a frame.
    fn resume_drag(&mut self) {
        let Some(label) = self.drag_label.clone() else { return };
        if self.moves_document().is_some_and(|file| !file.in_gesture()) {
            self.begin_gesture(&label);
        }
    }

    /// Starts a gesture, with what's being typed in the text first, as a step of its own.
    fn begin_gesture(&mut self, label: &str) {
        self.take_typed();
        self.typing_number = None;
        if let Some(id) = self.moves {
            self.go_to(id);
        }
        if let Some(file) = self.moves_document_mut() {
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
        if let Some(added) = self.edit(&label, |text| edits::insert_entry(text, mv_index, track, &entry)) {
            self.selected_entry = Some(added);
        }
    }

    /// Deletes the selected entry.
    pub fn delete_selected(&mut self) {
        let Some(entry) = self.selected_entry else { return };
        let label = format!("delete from {}", timeline::title(entry.track));
        if self.edit(&label, |text| edits::delete_entry(text, entry)).is_some() {
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
            self.edit("move an instant", |text| edits::set_instant(text, entry, tick));
        } else if start {
            self.edit("set a window's start", |text| edits::set_window(text, entry, tick, span.to));
        } else {
            // The playhead's tick is the window's last open tick.
            self.edit("set a window's end", |text| {
                edits::set_window(text, entry, span.from, tick.saturating_add(1))
            });
        }
    }

    /// Nudges the selected entry by whole ticks.
    pub fn nudge_selected(&mut self, by: i64) {
        let Some(entry) = self.selected_entry else { return };
        let Some(mv) = self.document().and_then(|document| document.moves.get(entry.mv)) else { return };
        let Some(span) = entry.track.spans(mv).get(entry.index).copied() else { return };
        let width = span.to.saturating_sub(span.from);
        let from = i64::from(span.from).saturating_add(by).clamp(0, i64::from(u32::MAX - width)) as u32;
        let to = from + width;
        if entry.track.is_instant() {
            self.edit("nudge an instant", |text| edits::set_instant(text, entry, from));
        } else {
            self.edit("nudge a window", |text| edits::set_window(text, entry, from, to));
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
        if self.typed.is_none() {
            let noticed = self.documents.refresh();
            self.say_noticed(noticed);
        }
        let in_gesture =
            self.documents.ids().any(|id| self.documents.any(id).is_some_and(|file| file.in_gesture()));
        if now >= self.next_rescan && !in_gesture && self.typed.is_none() {
            let look = Stopwatch::start();
            match self.documents.rescan() {
                Ok(noticed) => self.say_noticed(noticed),
                Err(error) => tracing::warn!(%error, "couldn't look at what's under the roots"),
            }
            // A document in a folder far larger than a repository costs far more: looks never take over 1%.
            self.next_rescan = now + RESCAN_SECONDS.max(look.elapsed().as_secs_f64() * 100.0);
        }
        self.persist_layout();
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
            self.content = self.moves.and_then(|id| self.documents.path(id)).and_then(find_cook);
        }
        let current = self.content.as_deref().map(cooked::pack_stamp);
        current.is_some() && current != self.cook.tried
    }

    /// The whole interface.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.closed_unseen = false;
        let now = ui.input(|input| input.time);
        self.now = now;
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
        self.cvar_panel.drawn = false;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            // Out of the way of the panes it draws, which are the rest of `self`.
            let mut shell = std::mem::replace(&mut self.shell, Shell::taken());
            shell.ui(ui, self);
            self.shell = shell;
        });
        if let Some((_, saved @ None)) = &mut self.layout {
            *saved = Some(self.shell.tree.clone());
        }
        self.link.want_cvars(self.cvar_panel.drawn);
        self.settle_gesture(ui);
        self.settle_typed(ui);
    }

    fn keys(&mut self, ui: &Ui, now: f64) {
        // The keys are the fight's while it's played.
        if self.viewport.is_playing() {
            return;
        }
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
            self.set_tick(self.tick.saturating_add(1));
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
        ui.horizontal_wrapped(|ui| {
            let name = self.current().map_or_else(|| "no file".to_owned(), |file| file.file_name());
            let dirty = self.typed.is_some() || self.current().is_some_and(|file| file.is_dirty());
            let title = if dirty { format!("{name} *") } else { name };
            ui.label(RichText::new(title).strong());

            let has_file = self.current.is_some();
            if ui
                .add_enabled(has_file, egui::Button::new("Save and push"))
                .on_hover_text("Cmd+S, or Ctrl+S")
                .clicked()
            {
                self.save_and_push();
            }
            let undo = self.current().and_then(|file| file.undo_label()).map(ToOwned::to_owned);
            // History's floor is worth a click: it says why there's nothing there.
            let floor = self.current().and_then(|file| file.document().why_no_undo());
            let hover = undo.clone().or_else(|| floor.clone()).unwrap_or_default();
            if ui
                .add_enabled(undo.is_some() || floor.is_some(), egui::Button::new("Undo"))
                .on_hover_text(hover)
                .clicked()
            {
                self.undo();
            }
            let redo = self.current().and_then(|file| file.redo_label()).map(ToOwned::to_owned);
            if ui
                .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                .on_hover_text(redo.unwrap_or_default())
                .clicked()
            {
                self.redo();
            }
            if ui.add_enabled(has_file, egui::Button::new("Close file")).clicked() {
                self.request_close();
            }
            ui.menu_button("Panes", |ui| {
                for pane in Pane::ALL {
                    let mut open = self.shell.is_open(pane);
                    if ui.checkbox(&mut open, pane.title()).changed() {
                        if open {
                            self.shell.open(pane);
                        } else {
                            self.shell.close(pane);
                        }
                    }
                }
            });
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
                    // An address unchanged is no change, however often the field lets go.
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
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), "You have edits that aren't saved.");
                if ui.button("Save and quit").clicked() {
                    match self.save_all() {
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
        if let Some(id) = self.closing {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), "You have edits that aren't saved.");
                if ui.button("Save and close").clicked() {
                    match self.documents.save(id) {
                        Ok(()) => self.close(id),
                        Err(error) => {
                            self.notice = Some(Notice { text: save_trouble(&error), trouble: true });
                            self.closing = None;
                        }
                    }
                }
                if ui.button("Close without saving").clicked() {
                    self.close(id);
                }
                if ui.button("Keep editing").clicked() {
                    self.closing = None;
                }
            });
        }
        if let (Some(id), Some(conflict)) = (self.current, self.current().and_then(|file| file.conflict())) {
            let (said, can_take) = match conflict {
                Conflict::Text(_) => ("The file changed on disk, and you have edits here.", true),
                Conflict::NotText => (
                    "The file on disk isn't text any more. Keeping yours replaces it at the next save.",
                    false,
                ),
            };
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(255, 170, 90), said);
                if can_take && ui.button("Take the file's").clicked() {
                    self.typed = None;
                    self.documents.take_disk(id);
                    self.after_take();
                }
                if can_take
                    && ui
                        .button("Mine on top of the file's")
                        .on_hover_text("Each edit that still fits goes back")
                        .clicked()
                {
                    self.typed = None;
                    if let Some(rebased) = self.documents.rebase(id) {
                        self.notice = Some(rebase_notice(&rebased));
                    }
                    self.after_take();
                }
                if ui.button("Keep mine").clicked() {
                    self.documents.keep_mine(id);
                }
            });
        }
        if let Some(typed) = &self.typed
            && let Some(why) = &typed.refused
        {
            let entry = match &typed.at {
                TypedAt::Entry { entry, .. } => Some(*entry),
                TypedAt::Property { .. } => None,
            };
            let said = match entry {
                Some(entry) => format!(
                    "`{}` can't go in {} {}'s {}, and nothing else can until it's fixed or dropped: {why}",
                    typed.text,
                    timeline::title(entry.track),
                    entry.index,
                    typed.at.label(),
                ),
                None => format!(
                    "`{}` can't go in {}, and nothing else can until it's fixed or dropped: {why}",
                    typed.text,
                    typed.at.label(),
                ),
            };
            ui.horizontal_wrapped(|ui| {
                if let Some(entry) = entry
                    && ui.button("Show it").clicked()
                {
                    self.choose_move(entry.mv);
                    self.selected_entry = Some(entry);
                    self.shell.open(Pane::Inspector);
                }
                if ui.button("Drop it").clicked() {
                    self.typed = None;
                }
                ui.colored_label(Color32::from_rgb(255, 120, 110), said);
            });
        }
        if let Some(message) = self.current().and_then(|file| file.document().failure_message()) {
            ui.colored_label(
                Color32::from_rgb(255, 120, 110),
                format!("This can't be edited until the file reads again. {message}"),
            );
        }
        if let Some(line) = self.current().and_then(|file| generated_by(file.text())) {
            ui.colored_label(Color32::from_rgb(235, 190, 110), line);
        }
        if let Some(notice) = &self.notice {
            let color =
                if notice.trouble { Color32::from_rgb(255, 120, 110) } else { Color32::from_gray(190) };
            ui.colored_label(color, &notice.text);
        }
        ui.add_space(2.0);
    }

    /// What taking a file's text leaves to do.
    fn after_take(&mut self) {
        self.link.clear_push();
        self.refresh();
        self.check_selection();
    }

    fn moves_list(&mut self, ui: &mut Ui) {
        let Some(document) = self.document() else { return };
        let problems = self.problems();
        let rows: Vec<(usize, &str, u32, usize)> = document
            .moves
            .iter()
            .enumerate()
            .map(|(index, mv)| {
                let count = problems
                    .iter()
                    .filter(|problem| problem.in_move.as_deref() == Some(mv.name.as_str()))
                    .count();
                (index, mv.name.as_str(), mv.ticks(), count)
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
        let editable = self.moves_document().is_some_and(|file| file.failure().is_none());
        let Some(document) = self
            .moves
            .and_then(|id| self.documents.get::<MoveSetDocument>(id))
            .and_then(|file| file.last_good())
        else {
            ui.label("Open a `.moves.ron` file.");
            return;
        };
        let Some(mv) = document.moves.get(self.selected_move) else { return };
        let root = self
            .preview
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .move_at(Some(&mv.name), self.selected_move)
                    .and_then(|mv| snapshot.set.find(Name::intern(&mv.name)))
            })
            .map(|(_, mv)| mv.tables.root_samples());
        let hit_ticks: Vec<u32> = self
            .run
            .as_ref()
            .map_or_else(Vec::new, |run| run.hits().filter(|hit| hit.on_dummy).map(|hit| hit.tick).collect());
        let view = TimelineView {
            mv,
            mv_index: self.selected_move,
            tick: self.tick,
            selected: self.selected_entry,
            problems: &problems,
            root,
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
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "Drag ends and bodies.   [ and ]: window's ends to the playhead.   Alt+arrows: nudge.   \
                         Delete: remove.   {command}+Z: undo.   {command}+S: save and push."
                    ))
                    .small()
                    .color(Color32::from_gray(140)),
                )
                .truncate(),
            );
        });
        self.problems_strip(ui);
        // The panel keeps what it's been given, however little there is to show.
        ui.allocate_space(ui.available_size());
    }

    /// The document's problems, whichever move they're in.
    fn problems_strip(&mut self, ui: &mut Ui) {
        let problems = self.problems();
        if problems.is_empty() {
            let word = if !self.stale && self.preview.is_some() {
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
        ui.horizontal_wrapped(|ui| {
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
                self.view_fit = ViewFit::Pending;
            }
            if ui.small_button("Front").clicked() {
                self.view = View::front(self.setup.dummy_distance);
                self.view_fit = ViewFit::Pending;
            }
            if ui.small_button("Top").clicked() {
                self.view = View::top(self.setup.dummy_distance);
                self.view_fit = ViewFit::Pending;
            }
            ui.checkbox(&mut self.overlays.bones, "Bones");
            ui.checkbox(&mut self.overlays.hurtboxes, "Hurtboxes");
        });
        let size = ui.available_size().max(Vec2::splat(40.0));
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = response.rect;
        if response.dragged() {
            self.view_fit = ViewFit::Manual;
            let delta = response.drag_delta();
            self.view.yaw += delta.x * 0.008;
            self.view.pitch = (self.view.pitch + delta.y * 0.008).clamp(-0.3, 1.55);
        }
        if response.hovered() {
            let zoom = ui.input(|input| input.smooth_scroll_delta.y);
            if zoom != 0.0 {
                self.view_fit = ViewFit::Manual;
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
            if matches!(self.view_fit, ViewFit::Pending)
                || matches!(self.view_fit, ViewFit::Fitted(size) if size != rect.size())
            {
                self.view.fit(rect, frame, posed);
                self.view_fit = ViewFit::Fitted(rect.size());
            }
            preview::draw(&painter, rect, &self.view, run, frame, posed, &mut self.cache, self.overlays);
            drew = true;
        }
        if !drew {
            let why = self.run_error.clone().unwrap_or_else(|| "Open a `.moves.ron` file.".to_owned());
            preview::empty(&painter, rect, &why);
        }
        if self.stale && drew {
            let color = Color32::from_rgb(255, 150, 120);
            let galley = painter.layout(
                "Showing the last valid preview.".to_owned(),
                egui::FontId::proportional(13.0),
                color,
                (rect.width() - 20.0).max(1.0),
            );
            let at = rect.left_bottom() + Vec2::new(10.0, -10.0 - galley.size().y);
            painter.galley(at, galley, color);
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
        ui.horizontal_wrapped(|ui| {
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
                self.set_tick(self.tick.saturating_add(1));
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
                    self.set_speed(speed);
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
        let editable = self.moves_document().is_some_and(|file| file.failure().is_none());
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("entry ticks").num_columns(2).show(ui, |ui| {
                if is_instant {
                    let mut at = from;
                    ui.label("at");
                    let response = ui.add(egui::DragValue::new(&mut at).speed(0.25));
                    self.timeline_number_gesture(&response, "move an instant");
                    if response.changed() {
                        self.edit("move an instant", |text| edits::set_instant(text, entry, at));
                    }
                    ui.end_row();
                } else {
                    let (mut new_from, mut new_to) = (from, to);
                    ui.label("from");
                    let response = ui.add(egui::DragValue::new(&mut new_from).speed(0.25));
                    self.timeline_number_gesture(&response, "edit a window");
                    let from_changed = response.changed();
                    ui.end_row();
                    ui.label("to");
                    let response = ui.add(egui::DragValue::new(&mut new_to).speed(0.25));
                    self.timeline_number_gesture(&response, "edit a window");
                    if from_changed || response.changed() {
                        self.edit("edit a window", |text| edits::set_window(text, entry, new_from, new_to));
                    }
                    ui.end_row();
                }
            });
            let values = self.moves_document().and_then(|file| edits::entry_values(file, entry).ok());
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

    fn timeline_number_gesture(&mut self, response: &egui::Response, label: &str) {
        if let Some(id) = self.moves {
            self.number_gesture(response, label, id);
        }
    }

    /// A number's drag, or its typing, is one step of undo on its document.
    fn number_gesture(&mut self, response: &egui::Response, label: &str, id: AssetId) {
        if response.drag_started() {
            self.begin_gesture_on(id, label);
        }
        if response.drag_stopped()
            && let Some(file) = self.documents.any_mut(id)
        {
            file.end_gesture();
        }
        let held = response.ctx.input(|input| input.pointer.any_down());
        let idle = self.documents.any(id).is_some_and(|file| !file.in_gesture());
        if response.has_focus() && !held && idle {
            self.begin_gesture_on(id, label);
            self.typing_number = Some((response.id, id));
        }
    }

    fn begin_gesture_on(&mut self, id: AssetId, label: &str) {
        self.take_typed();
        self.typing_number = None;
        self.go_to(id);
        if let Some(file) = self.documents.any_mut(id) {
            file.begin_gesture(label);
        }
    }

    fn value_row(&mut self, ui: &mut Ui, entry: EntryRef, value: &EntryValue) {
        ui.label(&value.path);
        let label = format!("edit {}", value.path);
        let number = self
            .document()
            .and_then(|document| sapphire_document::walk::get(document, &value.property).ok())
            .and_then(properties::number_kind);
        if let Some(kind) = number
            && let Some((response, literal)) = properties::number_widget(ui, &value.literal, kind)
        {
            self.timeline_number_gesture(&response, &label);
            if let Some(literal) = literal {
                self.edit(&label, |text| edits::set_entry_value(text, entry, &value.path, &literal));
            }
        } else {
            // A name or a bare word, as the file writes it: see `Typed`.
            let field = egui::Id::unique(("inspector value", entry, &value.path));
            let at = TypedAt::Entry { entry, path: value.path.clone() };
            self.text_field(ui, field, at, &value.literal);
        }
        ui.end_row();
    }

    /// Takes what a field holds now. Typing in another field means the last one let go, and typing back to
    /// what the file says leaves nothing to take.
    fn keep_typing(&mut self, typed: Typed, literal: &str) {
        let elsewhere = self.typed.as_ref().is_some_and(|other| other.field != typed.field);
        if !elsewhere || self.take_typed() {
            // What's typed belongs to its document, so that's the one a save or an undo means from here on.
            match &typed.at {
                TypedAt::Entry { .. } => {
                    if let Some(id) = self.moves {
                        self.go_to(id);
                    }
                }
                TypedAt::Property { document, .. } => self.go_to(*document),
            }
            self.typed = (typed.text != literal).then_some(typed);
        }
    }

    fn move_inspector(&mut self, ui: &mut Ui) {
        let Some(document) = self.document() else { return };
        let Some(mv) = document.moves.get(self.selected_move) else { return };
        let (name, startup, active, recovery, ticks) =
            (mv.name.clone(), mv.startup, mv.active, mv.recovery, mv.ticks());
        let has_clip = mv.motion.clip.is_some();
        ui.label(RichText::new(name).strong());
        ui.label(format!("{ticks} ticks, {:.0} ms", f64::from(ticks) * 1000.0 / 60.0));
        ui.label(format!("startup {startup}, active {active}, recovery {recovery}"));
        if has_clip {
            ui.label(
                RichText::new(
                    "Plays a clip tick for tick, so its length is the clip's. Windows move freely.",
                )
                .small()
                .color(Color32::from_gray(150)),
            );
        } else {
            ui.label(RichText::new("Keyed by hand.").small().color(Color32::from_gray(150)));
        }
        ui.label(
            RichText::new("Select something on the timeline to edit it.")
                .small()
                .color(Color32::from_gray(150)),
        );
    }

    fn device_panel(&mut self, ui: &mut Ui) {
        match self.link.device() {
            Some(status) => {
                ui.label(format!("{}, cook `{}`", status.platform, status.cook));
                let name = self.current().map(|file| file.file_name()).unwrap_or_default();
                let taken = status.documents.iter().find(|document| {
                    !name.is_empty()
                        && Path::new(&document.source)
                            .file_name()
                            .is_some_and(|source| source == name.as_str())
                });
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
        if let Some(earlier) = self.link.earlier_push() {
            let came_to = match &earlier.pushed {
                Pushed::Took { .. } => "taken".to_owned(),
                Pushed::Refused(refusals) => format!("refused, {} problems", refusals.len()),
                Pushed::Failed(why) => format!("not pushed: {why}"),
            };
            ui.label(
                RichText::new(format!("An earlier save of {}: {came_to}", earlier.origin))
                    .small()
                    .color(Color32::from_gray(150)),
            );
        }
        let attempts = self.link.attempts();
        if !attempts.is_empty() {
            ui.add_space(3.0);
            ui.label(RichText::new("Last attempts").small().strong());
            for attempt in attempts {
                self.attempt_row(ui, attempt);
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

impl PaneUi for Editor {
    fn pane_ui(&mut self, ui: &mut Ui, pane: Pane) {
        let margin = egui::Frame::new().inner_margin(4);
        margin.show(ui, |ui| match pane {
            Pane::Browser => self.browser_panel(ui),
            Pane::Moves => self.moves_list(ui),
            Pane::Timeline => self.timeline_panel(ui),
            Pane::Preview => self.preview_panel(ui, self.now),
            Pane::Viewport => self.viewport_pane(ui),
            Pane::Zone => self.zone_panel(ui),
            Pane::Sequencer => self.sequencer_panel(ui),
            Pane::Screen => self.screen_panel(ui),
            Pane::Inspector => {
                egui::ScrollArea::vertical().id_salt("inspector").auto_shrink([false, false]).show(
                    ui,
                    |ui| {
                        self.inspector(ui);
                    },
                );
            }
            Pane::Properties => self.properties_panel(ui),
            Pane::Cvars => self.cvars_panel(ui),
            Pane::Device => {
                egui::ScrollArea::vertical().id_salt("device").auto_shrink([false, false]).show(ui, |ui| {
                    self.device_panel(ui);
                });
            }
        });
    }
}

/// A save's trouble, in a notice's words.
fn save_trouble(error: &SetError) -> String {
    match error {
        SetError::ChangedOnDisk { .. } => error.to_string(),
        error => format!("not saved: {error}"),
    }
}

/// What a rebase came to, in a notice's words.
fn rebase_notice(rebased: &Rebased) -> Notice {
    if rebased.beneath {
        return Notice {
            text: "took the file's text: history can't say which edits weren't saved, so they're in undo"
                .to_owned(),
            trouble: true,
        };
    }
    match rebased.refused.first() {
        None => Notice {
            text: format!("took the file's text, and {} edits went back on top", rebased.kept),
            trouble: false,
        },
        Some(first) => Notice {
            text: format!(
                "took the file's text: {} edits went back on top and {} didn't, the first of them `{}`: {}",
                rebased.kept,
                rebased.refused.len(),
                first.label,
                first.why,
            ),
            trouble: true,
        },
    }
}

/// Whether the current move should have had a figure and didn't.
fn posed_missing(cook: &CookState, document: Option<&MoveSetDocument>) -> bool {
    document.is_some() && cook.casting.is_none()
}

/// What a new entry starts as, at `tick`, in the file's own words. The numbers are a start, not a choice:
/// a hit volume rides the move's first path, and a cancel allows what starts from neutral.
fn default_entry(document: &MoveSetDocument, mv: &MoveDocument, track: Track, tick: u32) -> String {
    let (from, to) = (tick, tick.saturating_add(3));
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
            format!("(name: {name:?}, from: {from}, to: {to}, path: {path:?}, shape: Sphere(radius: 0.3))")
        }
        Track::Hurtboxes => {
            let name = document
                .fighter
                .as_ref()
                .and_then(|fighter| fighter.hurtboxes.first())
                .map_or("body", |hurtbox| hurtbox.name.as_str());
            format!("(name: {name:?}, from: {from}, to: {to}, state: Off)")
        }
        Track::Invulnerable | Track::Parry | Track::PerfectDodge => format!("(from: {from}, to: {to})"),
        Track::Armour => format!("(from: {from}, to: {to}, threshold: 10)"),
        Track::Cancels => {
            let (intent, target) = document
                .neutral
                .first()
                .map_or(("intent", "move"), |allow| (allow.intent.as_str(), allow.target.as_str()));
            format!("(from: {from}, to: {to}, allow: [(intent: {intent:?}, move: {target:?})])")
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

/// The cook above a document, if there's one: an editor's (`cargo xtask content --cook editor`), which has each
/// zone's shapes, before a desktop one.
fn find_cook(document: &Path) -> Option<PathBuf> {
    let start = if document.is_absolute() {
        document.to_owned()
    } else {
        std::env::current_dir().ok()?.join(document)
    };
    let found = |cook: &str| {
        start.ancestors().skip(1).map(|dir| dir.join("target/content").join(cook)).find(|dir| dir.is_dir())
    };
    found("editor").or_else(|| found("desktop"))
}

/// Where documents are in a repository.
const REPOSITORY_ROOTS: [&str; 2] = ["content", "crates"];

/// The repository `place` is in, if it's in one.
fn repository_around(place: &Path) -> Option<&Path> {
    place.ancestors().find(|dir| dir.join("Cargo.toml").is_file() && dir.join("content").is_dir())
}

/// Where a window keeps its layout: under the build folder of the repository it was started in.
#[must_use]
pub fn layout_path() -> Option<PathBuf> {
    let dir = std::env::current_dir().ok()?;
    Some(repository_around(&dir)?.join("target/editor_layout.ron"))
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
    use super::*;
    use crate::edits::test_files::{PLAYER, WARRIOR};

    #[test]
    fn added_entries_preserve_authored_string_literals() {
        let mut document = MoveSetDocument::parse(PLAYER).expect("parses");
        let unusual = "a\"b\\c\n\t\0é";
        document.moves[0].name = unusual.to_owned();
        document.moves[0].motion.clip.as_mut().expect("clip").paths[0].name = unusual.to_owned();
        document.neutral[0].intent = unusual.to_owned();
        document.neutral[0].target = unusual.to_owned();
        let mv = &document.moves[0];
        let hitbox = default_entry(&document, mv, Track::Hitboxes, 7);
        let parsed: sapphire_moves::HitboxDocument = ron::from_str(&hitbox).expect("an escaped entry");
        assert_eq!(parsed.name, format!("{unusual}_hit"));
        assert_eq!(parsed.path, unusual);
        let cancel = default_entry(&document, mv, Track::Cancels, 7);
        let parsed: sapphire_moves::CancelDocument = ron::from_str(&cancel).expect("an escaped cancel");
        assert_eq!(parsed.allow[0].intent, unusual);
        assert_eq!(parsed.allow[0].target, unusual);
    }

    #[test]
    fn an_automatically_fitted_camera_stays_still_while_scrubbing_and_manual_orbit_opts_out() {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_camera_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("directory");
        let path = dir.join("warrior.moves.ron");
        fs::write(&path, WARRIOR).expect("source");
        let mut editor = Editor::new(Options {
            file: Some(path),
            content: Some(dir.join("no cook")),
            link: Some("127.0.0.1:9".parse().expect("address")),
        });
        let ctx = egui::Context::default();
        let mut textures = crate::capture::Textures::default();
        let mut pass = 0;
        let mut pump = |editor: &mut Editor, events: Vec<egui::Event>| {
            pass += 1;
            let mut input = crate::capture::raw_input(600, 400, f64::from(pass) * 0.05);
            input.events = events;
            let output = ctx.run_ui(input, |ui| editor.preview_panel(ui, f64::from(pass) * 0.05));
            crate::capture::settle(&ctx, &mut textures, output);
        };
        for _ in 0..4 {
            pump(&mut editor, Vec::new());
        }
        assert!(matches!(editor.view_fit, ViewFit::Fitted(_)));
        let fitted = editor.view;
        editor.set_tick(10);
        pump(&mut editor, Vec::new());
        assert_eq!(editor.view, fitted, "scrubbing does not refit");
        let at = egui::pos2(300.0, 250.0);
        pump(
            &mut editor,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
        let moved = at + egui::vec2(30.0, 10.0);
        pump(&mut editor, vec![egui::Event::PointerMoved(moved)]);
        assert!(matches!(editor.view_fit, ViewFit::Manual));
        assert_ne!(editor.view, fitted);
        pump(
            &mut editor,
            vec![egui::Event::PointerButton {
                pos: moved,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        let manual = editor.view;
        editor.set_tick(20);
        pump(&mut editor, Vec::new());
        assert_eq!(editor.view, manual);
        fs::remove_dir_all(dir).expect("cleanup");
    }

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
