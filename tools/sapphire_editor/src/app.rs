//! The editor's interface and state, everything but its window: [`Editor::ui`] draws into any `egui::Ui`,
//! so tests and [`crate::capture`] drive exactly what a person sees, with no window.
//!
//! Its panes are a [`Shell`] over one [`DocumentSet`]. The timeline shows one document of moves ([`Editor`]'s
//! `moves`), and the top bar's save and undo act on the current one, which is whichever was last opened or
//! edited: a timeline's edit makes its document current again. Nothing is lost by opening another document,
//! since the set holds what's unsaved in each, so only closing one or the window asks.
//!
//! A [`Journal`] keeps what's unsaved on disk after each command, and offers back what an editor that stopped
//! without saving kept: [`Command::Recover`] takes an offer back, as the document it was or in conflict with a
//! file that moved on, which the top bar's three answers settle as any conflict, and [`Command::DropRecovered`]
//! lets it go.
//!
//! A selection holds objects by document and path, across documents; the Properties pane edits the fields they
//! share, each edit one step of undo over every document it touched, and undo of any of those documents takes
//! the whole of it back ([`sapphire_document::SetStep`]).

mod browser;
mod cooking;
mod cvars;
mod device;
mod dispatch;
mod in_game;
mod inspector;
mod palette;
mod problems;
mod properties;
mod screen;
mod selection;
mod sequencer;
mod timeline_pane;
mod top_bar;
mod zone;

pub use dispatch::ScriptError;
pub use in_game::{GAME_ANSWER, GAME_STARTUP};
pub use sequencer::RATE;
pub use zone::SNAP;

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use egui::{Color32, Key, Modifiers, RichText, Sense, TextEdit, Ui, Vec2};
use sapphire_assets::AssetId;
use sapphire_core::Name;
use sapphire_document::disk::Temporary;
use sapphire_document::{
    AnyDocument, DocumentSet, Journal, Kind, Noticed, ObjectId, PropertyPath, Rebased, SetError, Took,
};
use sapphire_livelink::protocol::MoveAttempt;
use sapphire_moves::{MoveDocument, MoveSet, MoveSetDocument};
use sapphire_time::Stopwatch;

use crate::commands::Command;
use crate::cook::Cooker;
use crate::cooked::{self, Cook};
use crate::edits::{self, EditError, EntryRef, EntryValue, MovesDocument, Track};
use crate::figure::{Casting, Figure, Figures};
use crate::game::{Game, GameDied, Launcher};
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
    /// The pipeline a save's cook runs: the one `cargo xtask content` builds beside the cook if not given
    /// ([`crate::cook::CookPlan::for_cook`]).
    pub pipeline: Option<PathBuf>,
    /// The dev sandbox Play in game starts: the one beside the editor's own binary if not given
    /// ([`crate::game::Sandbox::beside_editor`]).
    pub game: Option<PathBuf>,
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
    /// The pipeline a save's cook runs, if named.
    pipeline: Option<PathBuf>,
    /// Cooks of what's saved: made at the first save with a cook to remake.
    cooker: Option<Cooker>,
    /// Why a save couldn't ask for a cook, if one couldn't.
    no_cooker: Option<String>,
    /// The open document the last cook refused, found once when it ended.
    refused: Option<AssetId>,
    /// The dev sandbox a game plays in, if one was named.
    game_binary: Option<PathBuf>,
    /// What starts a game in place of that sandbox: a test's stand-in.
    launcher: Option<Box<dyn Launcher>>,
    /// The game playing, a child process on the link (M6 T19).
    game: Option<Game>,
    /// The last game that stopped by itself, and the end of its log, until another plays.
    game_died: Option<GameDied>,
    /// Where the link pointed before a game took it, to go back to when it's gone.
    link_before_game: Option<Option<SocketAddr>>,
    /// What a save or a cook had for a game still starting, sent once it says where it listens.
    game_held: Vec<in_game::Held>,
    /// The files pushed to a game since a script last waited for it, whose answers that wait reads.
    game_pushes: Vec<String>,
    /// The text the latest diagnostics were made from.
    judged: Option<String>,
    issues: Vec<sapphire_moves::Issue>,
    /// The authored source and compiled set of one successful preview, replaced together.
    preview: Option<PreviewSnapshot>,
    /// The text no longer compiles, so the preview is of what last did.
    stale: bool,
    selected_move: usize,
    selected_entry: Option<EntryRef>,
    /// Objects by document and path, across documents, in the order they were picked.
    selection: Vec<ObjectId>,
    /// What's wrong with the open documents and the last cook, as the problems pane lists it (M6 T6).
    problems: Vec<sapphire_document::Problem>,
    /// The field a problem's row last went to, which Properties marks.
    field: Option<ObjectId>,
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
    /// The command palette, while it's open (Cmd+P).
    palette: Option<palette::Palette>,
    /// How many notices in trouble have gone up: one going up while a command runs refuses it.
    troubles: u64,
    /// How deep in commands this is: only those at the top are recorded.
    depth: u32,
    /// A window's frame is under way ([`Self::ui`]): what its commands leave to write waits for its end.
    in_frame: bool,
    /// The ID an `AddEntryAs` running asks its entry to have.
    entry_id: Option<String>,
    /// The ID an `AddEntry` running drew for a zone's entry, which the recording keeps it by.
    drawn_id: Option<String>,
    /// The IDs a `Duplicate` running drew for its copies, which the recording keeps them by.
    drawn_ids: Vec<String>,
    /// Where each selected entry stood when the command running began, so a rename keeps it selected.
    standings: Vec<selection::Standing>,
    /// Where a session's commands go, if it's being recorded.
    recording: Option<dispatch::Recording>,
    /// What the timeline's drag last asked, so a frame that asks it again runs nothing.
    drag_asked: Option<Action>,
    /// Commands a pass asked for from inside the shell, run once it's drawn.
    asked: Vec<Command>,
    /// What's unsaved, kept on disk at the end of each frame that changed it, and what editors that stopped kept,
    /// if a journal is kept.
    journal: Option<Journal>,
    /// The journal's last write failed, and the notice has said so.
    journal_failing: bool,
}

/// What's being typed over.
#[derive(Clone, Debug)]
pub(crate) enum TypedAt {
    /// A value of an entry on the timeline, by the path [`edits::entry_values`] gives it.
    Entry { entry: EntryRef, path: String },
    /// A property of any document.
    Property { document: AssetId, path: PropertyPath, label: String },
    /// A field every selected object has, by its path under each.
    Shared { path: PropertyPath, label: String },
}

impl TypedAt {
    fn label(&self) -> &str {
        match self {
            Self::Entry { path, .. } => path,
            Self::Property { label, .. } | Self::Shared { label, .. } => label,
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
            pipeline: options.pipeline,
            cooker: None,
            no_cooker: None,
            refused: None,
            game_binary: options.game,
            launcher: None,
            game: None,
            game_died: None,
            link_before_game: None,
            game_held: Vec::new(),
            game_pushes: Vec::new(),
            judged: None,
            issues: Vec::new(),
            preview: None,
            stale: false,
            selected_move: 0,
            selected_entry: None,
            selection: Vec::new(),
            problems: Vec::new(),
            field: None,
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
            palette: None,
            troubles: 0,
            depth: 0,
            in_frame: false,
            entry_id: None,
            drawn_id: None,
            drawn_ids: Vec::new(),
            standings: Vec::new(),
            recording: None,
            drag_asked: None,
            asked: Vec::new(),
            journal: None,
            journal_failing: false,
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
            self.say(Notice {
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
                self.notice = None;
                self.opened(id, path, kind);
            }
            Err(error) => self.say(Notice { text: error.to_string(), trouble: true }),
        }
    }

    /// Goes to a document just opened or taken back, and shows it in its panes.
    fn opened(&mut self, id: AssetId, path: &Path, kind: Kind) {
        self.go_to(id);
        // It may be open from before, with a file that's moved on.
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
        self.refresh_problems();
    }

    /// Keeps a journal under `root` from now on (`target/editor_journal/` in a repository, [`journal_path`]), and
    /// offers what editors that stopped there without saving kept. Trouble with it is a notice, and no journal.
    pub fn keep_journal_at(&mut self, root: &Path) {
        match Journal::open(root) {
            Ok(journal) => {
                if let Some(why) = journal.unreadable().first() {
                    self.notice = Some(Notice {
                        text: format!("the journal holds what doesn't read, left where it is: {why}"),
                        trouble: true,
                    });
                }
                self.journal = Some(journal);
                self.journal_step();
            }
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("{}: {error}: no journal is kept", root.display()),
                    trouble: true,
                });
            }
        }
    }

    /// The journal, if one's kept.
    #[must_use]
    pub const fn journal(&self) -> Option<&Journal> {
        self.journal.as_ref()
    }

    /// Ends the journal's session, as a quit does with nothing unsaved or with what's unsaved let go: what it
    /// kept goes. An editor that stops any other way leaves it for the next to offer.
    pub fn close_journal(&mut self) {
        let Some(journal) = self.journal.take() else { return };
        let [(_, writes), (_, bytes)] = journal.stats().counters();
        tracing::info!(
            editor.journal_writes = writes,
            editor.journal_bytes = bytes,
            "the journal's session closed"
        );
        if let Err(error) = journal.close() {
            tracing::warn!(%error, "the journal's session wasn't taken away");
        }
    }

    /// Writes what's changed of each unsaved document to the journal, and warns and says once when that starts
    /// failing, and once when it's written again. While it fails, a frame's end passes it over and the poll tries
    /// again, once a second at most. The documents whose entries failed are returned.
    fn journal_step(&mut self) -> Vec<AssetId> {
        let Some(journal) = &mut self.journal else { return Vec::new() };
        let failed = journal.write(&self.documents);
        match failed.first() {
            Some((id, error)) => {
                if !std::mem::replace(&mut self.journal_failing, true) {
                    tracing::warn!(%id, %error, "the journal wasn't written, and is tried again once a second");
                    // Not trouble: the command that just ran did what it was asked.
                    self.notice = Some(Notice {
                        text: format!(
                            "the journal can't be written ({error}): a crash would lose what's unsaved"
                        ),
                        trouble: false,
                    });
                }
            }
            None => {
                if std::mem::take(&mut self.journal_failing) {
                    tracing::info!("the journal is written again");
                    self.notice =
                        Some(Notice { text: "the journal is written again".to_owned(), trouble: false });
                }
            }
        }
        failed.into_iter().map(|(id, _)| id).collect()
    }

    /// Whether the journal's last write failed: the poll tries it again, not each frame.
    #[must_use]
    pub fn journal_failing(&self) -> bool {
        self.journal_failing
    }

    /// Takes back what an editor that stopped hadn't saved of the document that was at `path`. Its entry goes
    /// once this session's journal holds the text.
    fn recover(&mut self, path: &Path) {
        if !self.take_typed() {
            self.refuse("nothing's taken back until what's being typed is fixed or dropped");
            return;
        }
        let offered = self.journal.as_ref().and_then(|journal| journal.offer(path));
        let Some(entry) = offered.map(|offer| offer.entry.clone()) else {
            self.refuse(format!("the journal offers nothing of {}", path.display()));
            return;
        };
        // By the entry's own text, not the file's: the file may have moved, or gone, since.
        let Some(kind) = Kind::of_text(&entry.path, || Some(entry.text.clone())) else {
            self.refuse(format!(
                "{}: the journal's entry is no kind of document this editor edits, left where it is",
                entry.path.display()
            ));
            return;
        };
        self.add_roots_around(&entry.path, true);
        let noticed = match kind.recover(&mut self.documents, &entry) {
            Ok(noticed) => noticed,
            Err(error) => {
                self.refuse(error.to_string());
                return;
            }
        };
        if !self.journal_step().contains(&entry.id)
            && let Some(journal) = &mut self.journal
            && let Err(error) = journal.forget(path)
        {
            tracing::warn!(%error, "the journal's entry that was taken back is still there");
        }
        let now = self.documents.path(entry.id).map_or_else(|| entry.path.clone(), Path::to_owned);
        self.notice = Some(Notice {
            text: format!("took back what wasn't saved of {}", now.display()),
            trouble: false,
        });
        self.opened(entry.id, &now, kind);
        self.say_noticed(noticed);
    }

    /// Lets go of what the journal kept of the document that was at `path`.
    fn drop_recovered(&mut self, path: &Path) {
        match self.journal.as_mut().map(|journal| journal.forget(path)) {
            Some(Ok(true)) => {
                self.notice = Some(Notice {
                    text: format!("let go of what wasn't saved of {}", path.display()),
                    trouble: false,
                });
            }
            Some(Ok(false)) | None => {
                self.refuse(format!("the journal offers nothing of {}", path.display()));
            }
            Some(Err(error)) => self.refuse(format!("{}: {error}", path.display())),
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
        self.selection.retain(|object| object.document != id);
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
        self.follow_zone();
        let moves = self.moves.and_then(|id| self.documents.path(id)).and_then(Path::file_name);
        let current = self.current;
        let camera = self.viewport.camera();
        let zone = zone::zone_shown(&self.documents, &self.zone, current, &self.selection, camera);
        let timeline = sequencer::timeline_shown(&self.documents, &mut self.sequencer, current);
        self.screen.follow(&self.documents, current);
        let screen = screen::screen_shown(&self.documents, &self.screen, current);
        ViewportFrame {
            open: self.shell.is_open(Pane::Viewport),
            content: self.content.as_deref(),
            cooked: self.cooks_landed(),
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

    /// The simulation's run of the selected move, which the preview shows.
    #[must_use]
    pub fn preview_run(&self) -> Option<&Run> {
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
                self.say(Notice { text: format!("saved {name}"), trouble: false });
                // A game playing has had it from the save if it takes it as text; any other kind reaches it as its
                // cook's packs, and its text would only be refused.
                if self.game.is_none() {
                    self.link.push(name, text);
                }
            }
            Err(text) => self.say(Notice { text, trouble: true }),
        }
    }

    /// Writes the current document, with what's being typed in it. What goes wrong is in a notice's words.
    fn save(&mut self) -> Result<(), String> {
        if !self.take_typed() {
            return Err("not saved: what's being typed can't go in".to_owned());
        }
        let Some(id) = self.current else { return Ok(()) };
        self.documents.save(id).map_err(|error| save_trouble(&error))?;
        self.ask_cook(&[id]);
        self.push_to_game(&[id]);
        // What a step of several documents did to the others goes too, or one file says what the other doesn't:
        // a placement saved without the colliders it moved is a zone the cook refuses.
        for joined in self.documents.joined(id) {
            if self.documents.is_dirty(joined) {
                self.documents.save(joined).map_err(|error| save_trouble(&error))?;
                self.push_to_game(&[joined]);
            }
        }
        Ok(())
    }

    /// Writes every document with edits, with what's being typed in its own. What goes wrong is in a notice's
    /// words, and what can be saved is.
    fn save_all(&mut self) -> Result<(), String> {
        if !self.take_typed() {
            return Err("not saved: what's being typed can't go in".to_owned());
        }
        let dirty: Vec<AssetId> = self.documents.ids().filter(|id| self.documents.is_dirty(*id)).collect();
        let mut failed = self.documents.save_all();
        self.ask_cook(&dirty);
        let saved: Vec<AssetId> =
            dirty.into_iter().filter(|id| !failed.iter().any(|(refused, _)| refused == id)).collect();
        self.push_to_game(&saved);
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
        // It lands as the command it amounts to, on the document it was typed in, which a script then repeats.
        let (result, command): (Result<(), EditError>, Command) = match &typed.at {
            TypedAt::Entry { entry, path } => {
                let Some(document) = self.moves_document_mut() else { return true };
                let label = format!("edit {path}");
                let result = document
                    .together(&label, |document| edits::set_entry_value(document, *entry, path, &typed.text));
                (result, Command::SetValue { entry: *entry, path: path.clone(), value: typed.text.clone() })
            }
            TypedAt::Property { document: id, path, .. } => {
                if self.current != Some(*id)
                    && let Some(path) = self.documents.path(*id).map(Path::to_owned)
                {
                    self.go_to(*id);
                    self.record(Command::Open(path));
                }
                if self.documents.any(*id).is_none() {
                    return true;
                }
                let label = format!("edit {}", dispatch::last_step(path));
                let result = self.spell(*id, &label, path, &typed.text);
                (result, Command::SetProperty { path: path.to_string(), value: typed.text.clone() })
            }
            TypedAt::Shared { path, .. } => {
                let result = self.set_shared(path, &typed.text);
                (result, Command::SetShared { path: path.to_string(), value: typed.text.clone() })
            }
        };
        match result {
            Ok(()) => {
                self.record(command);
                self.notice = None;
                self.link.clear_push();
                self.refresh();
                // It's in the text now, so in the journal by the end of its frame, or now if there's none.
                if self.depth == 0 && !self.in_frame {
                    self.end_frame();
                }
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
            if self.documents.any(id).is_some_and(|file| file.in_gesture()) {
                self.act(if taken_back { Command::Cancel } else { Command::End });
            }
        }
        if ui.input(|input| input.pointer.any_down()) {
            return;
        }
        self.drag = None;
        self.drag_label = None;
        if self.documents.in_gesture()
            || self.documents.ids().any(|id| self.documents.any(id).is_some_and(|file| file.in_gesture()))
        {
            self.act(Command::End);
        }
    }

    /// The window was asked to close: it goes at once, unless that would lose edits.
    pub fn request_quit(&mut self) {
        if self.is_dirty() {
            self.quit = Quit::Asking;
        } else {
            self.quit_now();
        }
    }

    /// Quits, with nothing unsaved or what's unsaved let go, so the journal has nothing to keep.
    fn quit_now(&mut self) {
        self.quit = Quit::Yes;
        self.close_journal();
    }

    /// The window was asked to close and can't draw, so the question it would ask can't be seen. Edits are
    /// saved rather than lost unseen. If the file won't take them the log says why and it stays open, in
    /// case that's the one thing to put right, and a second close goes anyway.
    ///
    /// A second close comes after a pass. SDL follows the close of its last window with a quit in the same
    /// pump, so one click calls this twice, and every event wakes a pass, so the next click comes after one.
    pub fn request_quit_unseen(&mut self) {
        if !self.is_dirty() {
            self.quit_now();
            return;
        }
        if std::mem::replace(&mut self.closed_unseen, true) {
            return;
        }
        self.quit = match self.save_all() {
            Ok(()) => {
                self.close_journal();
                Quit::Yes
            }
            Err(_) if self.quit == Quit::Asking => {
                tracing::error!(
                    "closing with no GPU to ask on, and the edits couldn't be saved: a journal, if one's kept, \
                     keeps them for the next editor to offer"
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
        let Some(id) = self.current else { return };
        // A step of several documents goes back whole, from any of them, or says which has a step of its own first.
        match self.documents.undo(id) {
            Ok(Took::Step) => self.after_history(),
            Ok(Took::Apart { label, left }) => self.after_apart(&label, &left),
            Ok(Took::Nothing) => {
                if let Some(why) = self.documents.any(id).and_then(AnyDocument::why_no_undo) {
                    // At history's floor: it's not that nothing was ever done, and the file doesn't have this text.
                    self.say(Notice { text: why, trouble: false });
                }
            }
            Err(error) => self.refuse(error.to_string()),
        }
    }

    pub fn redo(&mut self) {
        if !self.take_typed() {
            return;
        }
        let Some(id) = self.current else { return };
        match self.documents.redo(id) {
            Ok(Took::Step) => self.after_history(),
            Ok(Took::Apart { label, left }) => self.after_apart(&label, &left),
            Ok(Took::Nothing) => {}
            Err(error) => self.refuse(error.to_string()),
        }
    }

    /// After a step of several documents went in this one alone, since the rest of it was gone: says what it left.
    fn after_apart(&mut self, label: &str, left: &[PathBuf]) {
        self.after_history();
        let names: Vec<String> = left
            .iter()
            .map(|path| {
                path.file_name()
                    .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
            })
            .collect();
        self.say(Notice {
            text: format!(
                "`{label}` went here alone: what it did to {} is gone from its history",
                names.join(", ")
            ),
            trouble: true,
        });
    }

    fn after_history(&mut self) {
        // A held drag asks again what it asked before the text moved.
        self.drag_asked = None;
        self.notice = None;
        self.link.clear_push();
        self.refresh();
        self.check_selection();
    }

    fn check_selection(&mut self) {
        self.prune_selection();
        self.prune_field();
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
                self.say(Notice { text: error.0, trouble: true });
                None
            }
        }
    }

    /// Applies what the timeline asked, as the commands it amounts to. A held drag asks each frame, and a frame
    /// that asks what the last did runs nothing.
    pub fn apply(&mut self, action: Action) {
        let edits =
            matches!(action, Action::SetWindow { .. } | Action::SetInstant { .. } | Action::SetPhases { .. });
        if edits && self.drag_label.is_some() && self.drag_asked.as_ref() == Some(&action) {
            return;
        }
        self.drag_asked = edits.then(|| action.clone());
        match action {
            Action::Scrub(tick) => self.act(Command::Tick(tick)),
            Action::Select(entry) => self.act(Command::Select(entry)),
            Action::BeginGesture(label) => {
                if self.playing.is_some() {
                    self.act(Command::Stop);
                }
                self.begin_on_timeline(&label);
                self.drag_label = Some(label);
            }
            Action::EndGesture => self.act(Command::End),
            Action::SetWindow { entry, from, to, part } => {
                self.resume_drag();
                self.act(Command::SetWindow { entry, from, to });
                // The preview shows the tick the drag has hold of.
                let tick = match part {
                    Part::Start | Part::Body => from,
                    Part::End => to.saturating_sub(1),
                };
                self.act(Command::Tick(tick));
            }
            Action::SetInstant { entry, at } => {
                self.resume_drag();
                self.act(Command::SetInstant { entry, at });
                self.act(Command::Tick(at));
            }
            Action::SetPhases { mv, phases, edge } => {
                self.resume_drag();
                let edits::Phases { startup, active, recovery } = phases;
                self.act(Command::SetPhases { mv, startup, active, recovery });
                let tick = match edge {
                    PhaseEdge::StartupEnd => phases.startup,
                    PhaseEdge::ActiveEnd => phases.startup.saturating_add(phases.active),
                    PhaseEdge::MoveEnd => {
                        phases.startup.saturating_add(phases.active).saturating_add(phases.recovery)
                    }
                };
                self.act(Command::Tick(tick));
            }
            Action::Add(track) => self.act(Command::Add(track)),
        }
    }

    /// A drag still held whose gesture an undo or a save ended goes on as a gesture of its own, so the rest of it is
    /// one step of undo rather than one a frame.
    fn resume_drag(&mut self) {
        let Some(label) = self.drag_label.clone() else { return };
        if self.moves_document().is_some_and(|file| !file.in_gesture()) {
            self.begin_on_timeline(&label);
        }
    }

    /// Starts a gesture on the timeline's document, which becomes the current one first, as a command of its own.
    fn begin_on_timeline(&mut self, label: &str) {
        if let Some(id) = self.moves {
            self.make_current(id);
        }
        self.act(Command::Begin(label.to_owned()));
    }

    /// Makes `id` the current document by the command that does, so a script goes where the session went.
    pub(super) fn make_current(&mut self, id: AssetId) {
        if self.current != Some(id)
            && let Some(path) = self.documents.path(id).map(Path::to_owned)
        {
            self.act(Command::Open(path));
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

    /// Looks at the files (the documents and the screens' model files) and the cook again, and takes what live
    /// link has reported.
    pub fn poll(&mut self, now: f64) {
        self.link.poll();
        self.poll_game();
        self.poll_cook(now);
        if now - self.last_poll < POLL_SECONDS {
            return;
        }
        self.last_poll = now;
        if self.typed.is_none() {
            let noticed = self.documents.refresh();
            self.say_noticed(noticed);
        }
        self.screen.poll_models(&self.documents);
        let in_gesture = self.documents.in_gesture()
            || self.documents.ids().any(|id| self.documents.any(id).is_some_and(|file| file.in_gesture()));
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
        // What landed outside a command, a file taken from disk: the frame's end writes it, if this is a frame's.
        // A journal that's failing is tried here, once a second, and not at each frame's end.
        if !self.in_frame || self.journal_failing {
            self.journal_step();
        }
        if let Some(recording) = &mut self.recording
            && recording.failing()
        {
            recording.write();
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
            self.content = self.moves.and_then(|id| self.documents.path(id)).and_then(find_cook);
        }
        let current = self.content.as_deref().map(cooked::pack_stamp);
        current.is_some() && current != self.cook.tried
    }

    /// The whole interface.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.in_frame = true;
        self.frame_ui(ui);
        self.end_frame();
        self.in_frame = false;
    }

    /// The frame, before what its commands left to write is written.
    fn frame_ui(&mut self, ui: &mut Ui) {
        self.closed_unseen = false;
        let now = ui.input(|input| input.time);
        self.now = now;
        self.poll(now);
        self.advance_playback(now);
        self.keys(ui);
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
        for command in std::mem::take(&mut self.asked) {
            self.act(command);
        }
        self.palette_ui(ui);
        if let Some((_, saved @ None)) = &mut self.layout {
            *saved = Some(self.shell.tree.clone());
        }
        self.link.want_cvars(self.cvar_panel.drawn);
        self.settle_gesture(ui);
        self.settle_typed(ui);
    }

    /// The editor's keys, each the command the palette lists it by. Cmd+P opens the palette, which has the keys
    /// while it's open.
    fn keys(&mut self, ui: &Ui) {
        // The keys are the fight's while it's played.
        if self.viewport.is_playing() {
            return;
        }
        let ctx = ui.ctx();
        let pressed =
            |modifiers: Modifiers, key: Key| ctx.input_mut(|input| input.consume_key(modifiers, key));
        if pressed(Modifiers::COMMAND, Key::P) {
            self.palette = match self.palette {
                Some(_) => None,
                None => Some(palette::Palette::default()),
            };
        }
        if self.palette.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        if pressed(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) {
            self.act(Command::Redo);
        }
        if pressed(Modifiers::COMMAND, Key::Z) {
            self.act(Command::Undo);
        }
        if pressed(Modifiers::COMMAND, Key::S) {
            self.act(Command::SaveAndPush);
        }
        if typing {
            return;
        }
        let keys = [
            (Modifiers::NONE, Key::Space),
            (Modifiers::ALT, Key::ArrowLeft),
            (Modifiers::ALT, Key::ArrowRight),
            (Modifiers::NONE, Key::ArrowLeft),
            (Modifiers::NONE, Key::Comma),
            (Modifiers::NONE, Key::ArrowRight),
            (Modifiers::NONE, Key::Period),
            (Modifiers::NONE, Key::Home),
            (Modifiers::NONE, Key::End),
            (Modifiers::NONE, Key::OpenBracket),
            (Modifiers::NONE, Key::CloseBracket),
            (Modifiers::NONE, Key::Delete),
            (Modifiers::NONE, Key::Backspace),
        ];
        for (modifiers, key) in keys {
            if !pressed(modifiers, key) {
                continue;
            }
            let command = match key {
                Key::Space if self.playing.is_some() => Command::Stop,
                Key::Space => Command::Play,
                Key::ArrowLeft if modifiers.alt => Command::Nudge(-1),
                Key::ArrowRight if modifiers.alt => Command::Nudge(1),
                Key::ArrowLeft | Key::Comma => Command::Tick(self.tick.saturating_sub(1)),
                Key::ArrowRight | Key::Period => Command::Tick(self.tick.saturating_add(1)),
                Key::Home => Command::Tick(0),
                Key::End => Command::Tick(u32::MAX),
                Key::OpenBracket => Command::StartAtPlayhead,
                Key::CloseBracket => Command::EndAtPlayhead,
                _ => Command::Delete,
            };
            self.act(command);
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
            Pane::Preview => self.preview_panel(ui),
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
            Pane::Problems => self.problems_panel(ui),
            Pane::Cvars => self.cvars_panel(ui),
            Pane::Device => {
                egui::ScrollArea::vertical().id_salt("device").auto_shrink([false, false]).show(ui, |ui| {
                    self.device_panel(ui);
                });
            }
        });
    }

    fn close_asked(&mut self, pane: Pane) -> bool {
        self.asked.push(Command::ClosePane(pane));
        false
    }
}

/// A save's trouble, in a notice's words.
fn save_trouble(error: &SetError) -> String {
    match error {
        SetError::ChangedOnDisk { .. } => error.to_string(),
        error => format!("not saved: {error}"),
    }
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

/// Where a window keeps its journal: under the build folder of the repository it was started in.
#[must_use]
pub fn journal_path() -> Option<PathBuf> {
    let dir = std::env::current_dir().ok()?;
    Some(repository_around(&dir)?.join("target/editor_journal"))
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
    use super::top_bar::generated_by;
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
            pipeline: None,
            game: None,
        });
        let ctx = egui::Context::default();
        let mut textures = crate::capture::Textures::default();
        let mut pass = 0;
        let mut pump = |editor: &mut Editor, events: Vec<egui::Event>| {
            pass += 1;
            let mut input = crate::capture::raw_input(600, 400, f64::from(pass) * 0.05);
            input.events = events;
            let output = ctx.run_ui(input, |ui| editor.preview_panel(ui));
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
