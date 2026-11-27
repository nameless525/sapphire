//! Play in game (M6 T19): the game a child process, which Stop, another Play, the editor's drop and the end of its
//! standard input each end, and whose death loses nothing of the editor's; what's saved and cooked while it starts,
//! sent once it listens; and a scripted session whose edits of a graph, a cue table and an effect each reach a
//! running game.
//!
//! All but the last need no GPU: their game is a stand-in, this test binary run again on [`stand_in_game`] alone, which
//! does nothing unless the variable [`STAND_IN`] says how to behave. The last needs the dev sandbox, a cook and a
//! GPU:
//!
//! ```text
//! cargo xtask content
//! cargo build -p sapphire_sandbox --features dev
//! cargo test -p sapphire_editor --test play_in_game -- --ignored
//! ```
//!
//! `SAPPHIRE_SANDBOX` names another sandbox binary.

use std::io::Read as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{env, fs};

use sapphire_assets::pack::{Codec, PackAsset, write_pack};
use sapphire_assets::{AssetId, AssetKind, MemoryPackSource};
use sapphire_editor::commands::{Command, Script};
use sapphire_editor::cook::{CookRefusal, Cooked, Runner, Running};
use sapphire_editor::game::{Game, GameTarget, Launcher, Sandbox};
use sapphire_editor::link::Pushed;
use sapphire_editor::{Editor, Options};
use sapphire_livelink::protocol::{DeviceStatus, PackOutcome, ReplyBody};
use sapphire_livelink::{Client, LinkMode, LiveLink, LivePacks, Push};
use sapphire_time::Stopwatch;

const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const GRAPH: &str = include_str!("../../../content/lookdev/character/character.animgraph.ron");
const CUES: &str = include_str!("../../../crates/sapphire_cues/default_cues.ron");
const EFFECTS: &str = include_str!("../../../crates/sapphire_vfx/default_effects.ron");

/// Says the stand-in is wanted, and how it behaves: `runs <folder>`, beating in `<folder>/beat` until it's killed
/// or its standard input ends, or `dies`, saying a few lines and exiting with 3.
const STAND_IN: &str = "SAPPHIRE_EDITOR_STAND_IN_GAME";
/// How long anything here has to happen, under a loaded machine.
const PATIENCE: Duration = Duration::from_secs(60);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            env::temp_dir().join(format!("sapphire_editor_game_{name}_{}_{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The stand-in: does nothing as a test of its own.
#[test]
fn stand_in_game() {
    let Ok(how) = env::var(STAND_IN) else { return };
    match how.split_once(' ') {
        Some(("runs", folder)) => {
            let beat = Path::new(folder).join("beat");
            // As the sandbox's `--quit-on-stdin-eof` does.
            std::thread::spawn(|| {
                let mut buffer = [0_u8; 64];
                while matches!(std::io::stdin().read(&mut buffer), Ok(read) if read > 0) {}
                std::process::exit(0);
            });
            let mut count = 0_u64;
            loop {
                fs::write(&beat, count.to_string()).expect("a beat");
                std::thread::sleep(Duration::from_millis(20));
                count += 1;
            }
        }
        _ => {
            eprintln!("stand-in: loading the zone");
            eprintln!("stand-in: about to fail on purpose");
            std::process::exit(3);
        }
    }
}

/// Starts [`stand_in_game`] as the game, behaving as `how` says.
#[derive(Debug)]
struct StandIn(String);

impl Launcher for StandIn {
    fn command(&self, _: &GameTarget, _: &Path, _: &Path) -> Result<Process, String> {
        let mut command = Process::new(env::current_exe().map_err(|error| error.to_string())?);
        command.args(["stand_in_game", "--exact", "--nocapture", "--test-threads=1"]).env(STAND_IN, &self.0);
        Ok(command)
    }
}

/// An editor on a copy of the slice's moves, with an edit of it unsaved, its journal kept, and its link pointed at
/// a port nothing listens on.
fn editor_with_an_edit(dir: &Path) -> Editor {
    let path = dir.join("slice.moves.ron");
    fs::write(&path, SLICE).expect("a document");
    let mut editor = Editor::new(Options {
        file: Some(path),
        content: Some(dir.join("no cook here")),
        link: Some("127.0.0.1:9".parse().expect("an address")),
        pipeline: None,
        game: None,
    });
    editor.keep_journal_at(&dir.join("journal"));
    editor
        .run(Command::SetWindow {
            entry: sapphire_editor::edits::EntryRef {
                mv: 0,
                track: sapphire_editor::edits::Track::Hitboxes,
                index: 0,
            },
            from: 4,
            to: 9,
        })
        .expect("an edit");
    editor
}

fn zone() -> GameTarget {
    GameTarget::Zone("lantern_ward".to_owned())
}

/// Waits for `folder`'s beat file, and says what it holds.
fn first_beat(folder: &Path) -> String {
    let beat = folder.join("beat");
    let waited = Stopwatch::start();
    loop {
        if let Ok(text) = fs::read_to_string(&beat)
            && !text.is_empty()
        {
            return text;
        }
        assert!(waited.elapsed() < PATIENCE, "the game never beat");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether `folder`'s beat has stopped: the stand-in that beat there is gone.
fn beat_stopped(folder: &Path) -> bool {
    let beat = folder.join("beat");
    let last = fs::read_to_string(&beat).unwrap_or_default();
    std::thread::sleep(Duration::from_millis(300));
    fs::read_to_string(&beat).unwrap_or_default() == last
}

/// Every file of the journal but a session's lock, in its folders too, with its bytes.
fn journal_of(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut folders = vec![dir.join("journal")];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).expect("a journal") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            // A running session's lock is empty, and Windows refuses to read a file another handle has locked.
            if path.file_name().is_some_and(|name| name == "session.lock") {
                continue;
            }
            let bytes = fs::read(&path).expect("its bytes");
            files.push((path, bytes));
        }
    }
    files.sort();
    files
}

/// A launcher that can't start anything.
#[derive(Debug)]
struct NoGame;

impl Launcher for NoGame {
    fn command(&self, _: &GameTarget, _: &Path, _: &Path) -> Result<Process, String> {
        Err("no game here".to_owned())
    }
}

/// A cook that lands at once, having written `files`.
#[derive(Debug)]
struct Wrote(Vec<PathBuf>);

impl Runner for Wrote {
    fn reads(&self, _: &Path) -> bool {
        true
    }

    fn start(&mut self, _: bool) -> Result<Option<Box<dyn Running>>, CookRefusal> {
        Ok(Some(Box::new(Wrote(self.0.clone()))))
    }
}

impl Running for Wrote {
    fn finish(&mut self, _: bool) -> Option<Result<Cooked, CookRefusal>> {
        Some(Ok(Cooked { packs: vec!["shared".to_owned()], files: self.0.clone() }))
    }
}

/// A game's live link with nothing behind it: it takes every document and pack, and keeps their names.
struct FakeGame {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeGame {
    fn start() -> Self {
        let packs = Arc::new(LivePacks::new(MemoryPackSource::new()));
        let link =
            LiveLink::start(LinkMode::Listen(SocketAddr::from(([127, 0, 0, 1], 0))), packs).expect("a link");
        let address = link.address();
        let (seen, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let thread = std::thread::spawn({
            let (seen, stop) = (Arc::clone(&seen), Arc::clone(&stop));
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match link.poll() {
                        Some(Push::Status { reply }) => {
                            reply.reply(ReplyBody::Status(DeviceStatus::default()))
                        }
                        Some(Push::Document { origin, reply, .. }) => {
                            seen.lock().expect("seen").push(origin);
                            reply.reply(ReplyBody::DocumentApplied { owner: "moves".to_owned() });
                        }
                        Some(Push::Pack { pack, reply }) => {
                            seen.lock().expect("seen").push(pack.name().to_owned());
                            reply.reply(ReplyBody::PackApplied(PackOutcome::default()));
                        }
                        Some(_) => {}
                        None => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            }
        });
        Self { address, seen, stop, thread: Some(thread) }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("seen").clone()
    }
}

impl Drop for FakeGame {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Polls `editor` until `done`.
fn poll_until(editor: &mut Editor, what: &str, mut done: impl FnMut(&Editor) -> bool) {
    let waited = Stopwatch::start();
    while !done(editor) {
        assert!(waited.elapsed() < PATIENCE, "never saw {what}");
        editor.poll(waited.elapsed().as_secs_f64());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What's saved and cooked while a game starts waits for it to say where it listens, and goes then, its answers
/// kept; once it listens, a cook's pack goes as it lands, each part of a pack by its own name.
#[test]
fn what_a_starting_game_missed_reaches_it_once_it_listens() {
    let dir = TestDir::new("held");
    let device = FakeGame::start();
    let id = AssetId::from_bytes([7; 16]);
    let asset = PackAsset::encode(id, AssetKind::Rig, Codec::Lz4, b"a rig", Vec::new(), 3).expect("an asset");
    let pack = dir.0.join("shared.1.spack");
    fs::write(&pack, write_pack(vec![asset]).expect("a pack")).expect("written");
    let mut editor = editor_with_an_edit(&dir.0);
    editor.cook_with(Box::new(Wrote(vec![pack])));
    editor.play_with(Box::new(StandIn(format!("runs {}", dir.0.display()))));
    editor.run(Command::PlayInGame(zone())).expect("it plays");
    first_beat(&dir.0);

    // Saved while it starts: the moves as text, and the cook's pack.
    editor.run(Command::Save).expect("saved");
    for _ in 0..20 {
        editor.poll(0.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(device.seen().is_empty(), "nowhere to go yet");

    // It says where it listens: both go, and the device pane has their answers.
    let folder = editor.game().expect("playing").log_path().parent().expect("its folder").to_owned();
    fs::write(folder.join("link.address"), device.address.to_string()).expect("the address");
    poll_until(&mut editor, "both answered", |editor| {
        matches!(editor.link().answer("slice.moves.ron"), Some(Pushed::Took { .. }))
            && matches!(editor.link().answer("shared.1"), Some(Pushed::Took { .. }))
    });
    let mut seen = device.seen();
    seen.sort();
    assert_eq!(seen, ["shared.1", "slice.moves.ron"]);

    // Listening now, a cook that lands sends its pack at once.
    editor
        .run(Command::SetWindow {
            entry: sapphire_editor::edits::EntryRef {
                mv: 0,
                track: sapphire_editor::edits::Track::Hitboxes,
                index: 0,
            },
            from: 9,
            to: 4,
        })
        .expect("another edit");
    editor.run(Command::Save).expect("saved");
    poll_until(&mut editor, "the second pack", |_| {
        device.seen().iter().filter(|name| *name == "shared.1").count() == 2
    });
}

/// Another Play ends the game playing; one that can't start leaves the link where it was before either; the
/// editor's drop ends a game; and a game whose standard input ends goes by itself.
#[test]
fn a_game_goes_with_another_play_the_editor_or_its_input() {
    let dir = TestDir::new("replaced");
    let (first, second) = (dir.0.join("first"), dir.0.join("second"));
    fs::create_dir_all(&first).expect("a folder");
    fs::create_dir_all(&second).expect("a folder");
    let mut editor = editor_with_an_edit(&dir.0);
    editor.play_with(Box::new(StandIn(format!("runs {}", first.display()))));
    editor.run(Command::PlayInGame(zone())).expect("it plays");
    first_beat(&first);
    editor.play_with(Box::new(StandIn(format!("runs {}", second.display()))));
    editor.run(Command::PlayInGame(zone())).expect("another plays");
    first_beat(&second);
    assert!(beat_stopped(&first), "the first game is gone");

    editor.play_with(Box::new(NoGame));
    assert!(editor.run(Command::PlayInGame(zone())).is_err(), "nothing to start");
    assert!(editor.game().is_none() && beat_stopped(&second), "the second game is gone too");
    assert_eq!(editor.link().address(), Some("127.0.0.1:9".parse().expect("an address")), "the link's back");

    let third = dir.0.join("third");
    fs::create_dir_all(&third).expect("a folder");
    editor.play_with(Box::new(StandIn(format!("runs {}", third.display()))));
    editor.run(Command::PlayInGame(zone())).expect("it plays");
    first_beat(&third);
    drop(editor);
    assert!(beat_stopped(&third), "the editor's drop ends its game");

    let fourth = dir.0.join("fourth");
    fs::create_dir_all(&fourth).expect("a folder");
    let mut game = Game::start(&StandIn(format!("runs {}", fourth.display())), zone()).expect("it plays");
    first_beat(&fourth);
    game.close_input();
    let waited = Stopwatch::start();
    let died = loop {
        if let Some(died) = game.poll() {
            break died;
        }
        assert!(waited.elapsed() < PATIENCE, "it ran on with no input");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(died.status.contains('0'), "it quit: {}", died.status);
}

#[test]
fn stop_kills_the_game_and_gives_the_link_back() {
    let dir = TestDir::new("stop");
    let mut editor = editor_with_an_edit(&dir.0);
    editor.play_with(Box::new(StandIn(format!("runs {}", dir.0.display()))));
    editor.run(Command::PlayInGame(zone())).expect("it plays");
    assert!(editor.game().is_some());
    assert_eq!(editor.link().address(), None, "nobody, until the game says where it listens");
    let beat = dir.0.join("beat");
    let waited = Stopwatch::start();
    while !beat.exists() {
        assert!(waited.elapsed() < PATIENCE, "the game never beat");
        std::thread::sleep(Duration::from_millis(10));
    }

    editor.run(Command::StopGame).expect("it stops");
    assert!(editor.game().is_none() && editor.game_died().is_none());
    assert_eq!(editor.link().address(), Some("127.0.0.1:9".parse().expect("an address")), "the link's back");
    // Gone, not just let go: its beat stops.
    let last = fs::read_to_string(&beat).expect("a beat");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fs::read_to_string(&beat).expect("a beat"), last, "nothing beats after Stop");
    assert!(editor.run(Command::StopGame).is_err(), "nothing left to stop");
    assert!(editor.current().expect("the slice").is_dirty(), "the edit is still there");
}

#[test]
fn a_game_that_dies_shows_its_log_and_loses_nothing() {
    let dir = TestDir::new("dies");
    let mut editor = editor_with_an_edit(&dir.0);
    let path = dir.0.join("slice.moves.ron");
    let text = editor.current().expect("the slice").text().to_owned();
    let undo = editor.current().expect("the slice").undo_label().map(str::to_owned);
    let journal = journal_of(&dir.0);
    editor.play_with(Box::new(StandIn("dies".to_owned())));
    editor.run(Command::PlayInGame(zone())).expect("it plays");
    let waited = Stopwatch::start();
    while editor.game_died().is_none() {
        assert!(waited.elapsed() < PATIENCE, "the game never died");
        editor.poll(waited.elapsed().as_secs_f64());
        std::thread::sleep(Duration::from_millis(10));
    }

    let died = editor.game_died().expect("it died");
    assert!(died.status.contains('3'), "{}", died.status);
    assert!(died.log.iter().any(|line| line.contains("about to fail on purpose")), "{:?}", died.log);
    assert!(fs::read_to_string(&died.log_path).expect("its whole log").contains("loading the zone"));
    assert!(editor.game().is_none());
    assert!(
        editor.notice().is_some_and(|notice| notice.trouble && notice.text.contains("stopped by itself"))
    );
    // Nothing of the editor's moved: the text, its undo, its file and its journal.
    let current = editor.current().expect("the slice");
    assert_eq!(current.text(), text);
    assert!(current.is_dirty());
    assert_eq!(current.undo_label().map(str::to_owned), undo);
    assert_eq!(fs::read_to_string(&path).expect("the file"), SLICE, "nothing saved");
    assert_eq!(journal_of(&dir.0), journal, "the journal kept what it had, byte for byte");
    editor.run(Command::Undo).expect("undo still works");
    assert_eq!(editor.current().expect("the slice").text(), SLICE);

    // A script that plays a game that dies stops there, saying so with its log.
    editor.play_with(Box::new(StandIn("dies".to_owned())));
    let script = Script::new(vec![Command::PlayInGame(zone()), Command::Save]);
    let refused = editor.run_script(&script).expect_err("the game died");
    assert_eq!(refused.index, 1);
    assert!(refused.why.contains("about to fail on purpose"), "{}", refused.why);
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sandbox_binary() -> PathBuf {
    env::var_os("SAPPHIRE_SANDBOX").map_or_else(
        || {
            let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
            let target = Path::new(env!("CARGO_TARGET_TMPDIR")).parent().expect("the target folder");
            target.join(profile).join(format!("sapphire_sandbox{}", env::consts::EXE_SUFFIX))
        },
        PathBuf::from,
    )
}

/// The session: the arena's fight played in the game, then a graph's, a cue table's and an effect's edit saved, each a
/// command as a person's buttons run them.
const SESSION: &str = r#"(version: 1, commands: [
    PlayInGame(Arena(script: "attempts")),
    Open("character.animgraph.ron"), SetProperty(path: "locomotion.smoothing", value: "4.5"), Save,
    Open("default_cues.ron"), SetProperty(path: "rows[1].shake?.strength", value: "1.5"), Save,
    Open("default_effects.ron"), SetProperty(path: "effects[0].emitters[0].lifetime[1]", value: "0.12"), Save,
])"#;

/// What the in-process run can't show, a game shows: each of a graph, a cue table and an effect, edited by a script
/// with no window, reaches the running game, which says it took each and counts its edit, and draws a frame after
/// them that a tool's `link capture` takes. The editor's own run plays moves alone ([`sapphire_editor::play`]), so
/// none of these could reach it. Stop leaves no game.
#[test]
#[ignore = "needs a dev sandbox, a GPU and cooked content"]
fn a_scripted_session_reaches_the_running_game() {
    let cook = repo().join("target/content/desktop");
    assert!(
        cook.join("release.smanifest").is_file(),
        "no desktop cook at {}: `cargo xtask content` makes one",
        cook.display()
    );
    let binary = sandbox_binary();
    assert!(
        binary.is_file(),
        "no dev sandbox at {}: cargo build -p sapphire_sandbox --features dev",
        binary.display()
    );
    let dir = TestDir::new("session");
    for (name, text) in
        [("character.animgraph.ron", GRAPH), ("default_cues.ron", CUES), ("default_effects.ron", EFFECTS)]
    {
        fs::write(dir.0.join(name), text).expect("a document");
    }
    let script_path = dir.0.join("session.commands.ron");
    fs::write(&script_path, SESSION).expect("the script");
    let script = Script::read(&script_path).expect("reads");
    let mut editor = Editor::new(Options {
        file: None,
        content: Some(cook.clone()),
        link: Some("127.0.0.1:9".parse().expect("an address")),
        pipeline: None,
        game: Some(binary.clone()),
    });
    let ran = Stopwatch::start();
    if let Err(error) = editor.run_script(&script) {
        let log = editor.game().map(|game| fs::read_to_string(game.log_path()).unwrap_or_default());
        panic!("{error}\n{}", log.unwrap_or_default());
    }
    eprintln!("the session ran in {:?}", ran.elapsed());

    // What the game says now, asked afresh: each file is listed with the edit it took.
    let address = editor.link().address().expect("the game's address");
    let device = Client::connect(address).and_then(|mut client| client.status()).expect("a status");
    for (file, owner) in [
        ("character.animgraph.ron", "graph"),
        ("default_cues.ron", "cue"),
        ("default_effects.ron", "effects"),
    ] {
        match editor.link().answer(file) {
            Some(Pushed::Took { owner: took, .. }) => assert!(took.contains(owner), "{file}: {took}"),
            other => panic!("{file}: {other:?}"),
        }
        let taken = device.documents.iter().find(|document| document.source.ends_with(file));
        assert!(taken.is_some_and(|document| document.edits >= 1), "{file}: {:?}", device.documents);
    }

    // A tool reaches the same game at the address it said, and takes a frame drawn after every edit.
    let capture = Client::connect(address).and_then(|mut client| client.capture()).expect("a capture");
    assert!(capture.png.starts_with(b"\x89PNG"), "a PNG");
    eprintln!("captured {}x{} at {address}", capture.width, capture.height);

    let pid = editor.game().expect("playing").id();
    editor.run(Command::StopGame).expect("it stops");
    assert!(editor.game().is_none());
    assert!(Client::connect(address).is_err(), "nothing listens where the game did (process {pid})");

    // The game goes by itself when its standard input ends, as it does when the editor's process dies unwound.
    let mut game =
        Game::start(&Sandbox::new(binary, Some(cook)), GameTarget::Arena { script: "attempts".to_owned() })
            .expect("it plays");
    let waited = Stopwatch::start();
    while game.address().is_none() {
        assert!(waited.elapsed() < PATIENCE * 5, "the game never listened");
        assert!(game.poll().is_none(), "the game died starting");
        std::thread::sleep(Duration::from_millis(20));
    }
    game.close_input();
    let died = loop {
        if let Some(died) = game.poll() {
            break died;
        }
        assert!(waited.elapsed() < PATIENCE * 6, "the game ran on with no input");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(died.log.iter().any(|line| line.contains("standard input ended")), "{:?}", died.log);
    eprintln!("the game quit at its input's end: {}", died.status);
}
