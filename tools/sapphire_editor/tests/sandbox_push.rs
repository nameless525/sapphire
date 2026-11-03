//! Save and push against a real fight: a dev sandbox offscreen on a private port, its fight running from a
//! script, and the editor's own link, `Editor::save_and_push` and undo. What the device says comes back
//! through the status the fight's attempts write, so this sees the windows change in the simulation that
//! runs them.
//!
//! It needs a dev sandbox and a cook, and a GPU to run it on:
//!
//! ```text
//! cargo xtask content
//! cargo build -p sapphire_sandbox --features dev
//! cargo test -p sapphire_editor --test sandbox_push -- --ignored
//! ```
//!
//! `SAPPHIRE_SANDBOX` names another binary. CI's `App tests` job runs this on lavapipe. Unix only, for the
//! clean quit `kill -TERM` gives.
#![cfg(unix)]

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sapphire_editor::edits::{EntryRef, Phases, Track};
use sapphire_editor::link::Pushed;
use sapphire_editor::{Editor, Options};
use sapphire_livelink::protocol::MoveAttempt;
use sapphire_moves::MoveSetDocument;
use sapphire_time::Stopwatch;

const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");

/// How long a sandbox has to build its scene's pipelines, slowly on a CPU rasterizer.
const STARTUP: Duration = Duration::from_secs(300);
/// How long a fight has to show something it should within a moment, under a loaded machine.
const PATIENCE: Duration = Duration::from_secs(60);

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sandbox_binary() -> PathBuf {
    std::env::var_os("SAPPHIRE_SANDBOX").map_or_else(
        || {
            let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
            let target = Path::new(env!("CARGO_TARGET_TMPDIR")).parent().expect("the target folder");
            target.join(profile).join("sapphire_sandbox")
        },
        PathBuf::from,
    )
}

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_editor_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The dev sandbox, running offscreen with a fight, on a port of its own.
struct Sandbox {
    child: Child,
    address: SocketAddr,
    log: PathBuf,
}

impl Sandbox {
    fn start(temp: &Temp) -> Self {
        let binary = sandbox_binary();
        assert!(
            binary.exists(),
            "no dev sandbox at {}: cargo build -p sapphire_sandbox --features dev",
            binary.display()
        );
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
            listener.local_addr().expect("its address")
        };
        let log = temp.0.join("sandbox.log");
        let file = fs::File::create(&log).expect("a log");
        let child = Command::new(binary)
            .args(["--offscreen", "--size", "160x90", "--frames", "72000", "--no-hot-reload", "--live-link"])
            .arg(address.to_string())
            .args(["--scenario", "duel", "--script", "attempts", "--record"])
            .arg(temp.0.join("session.sreplay"))
            .env("SAPPHIRE_CONTENT_DIR", repo().join("target/content/desktop"))
            .stdout(file.try_clone().expect("the log"))
            .stderr(Stdio::from(file))
            .spawn()
            .expect("the sandbox starts");
        Self { child, address, log }
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Runs the editor's link until `done` finds what it wants, and returns that and how long it took.
    fn until<T>(
        &mut self,
        editor: &mut Editor,
        patience: Duration,
        what: &str,
        done: impl Fn(&Editor) -> Option<T>,
    ) -> (T, Duration) {
        let started = Stopwatch::start();
        loop {
            editor.poll(started.elapsed().as_secs_f64());
            if let Some(found) = done(editor) {
                return (found, started.elapsed());
            }
            if let Some(status) = self.child.try_wait().expect("the sandbox's status") {
                panic!("the sandbox exited with {status}:\n{}", self.log());
            }
            assert!(
                started.elapsed() < patience,
                "never saw {what}. link: {:?}, last push: {:?}\n{}",
                editor.link().trouble(),
                editor.link().last_push(),
                self.log()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Quits as Ctrl-C does, and waits for a clean exit.
    fn quit(mut self) {
        let pid = self.child.id().to_string();
        assert!(Command::new("kill").args(["-TERM", &pid]).status().expect("kill runs").success());
        let status = self.child.wait().expect("it exits");
        assert!(status.success(), "it exited with {status}:\n{}", self.log());
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `light_1`, as the fight's next attempt plays it, at this revision.
fn light_at(editor: &Editor, revision: u32) -> Option<MoveAttempt> {
    editor
        .link()
        .attempts()
        .iter()
        .find(|attempt| attempt.name == "light_1" && attempt.revision == revision)
        .cloned()
}

fn hit_window() -> EntryRef {
    let document = MoveSetDocument::parse(SLICE).expect("parses");
    let mv = document.moves.iter().position(|mv| mv.name == "light_1").expect("there");
    EntryRef { mv, track: Track::Hitboxes, index: 0 }
}

/// The whole loop, as a person runs it: an edit reaches a following attempt, a bad one is refused by move and
/// track with the edit still in the editor and the fight going on as it was, undo and another push work.
#[test]
#[ignore = "needs a dev sandbox, a GPU and cooked content"]
#[allow(clippy::print_stdout, reason = "the times are for a person reading --nocapture")]
fn an_edit_pushed_from_the_editor_plays_on_a_real_fights_next_attempt() {
    let temp = Temp::new("push");
    let path = temp.0.join("slice.moves.ron");
    fs::write(&path, SLICE).expect("writes");
    let mut sandbox = Sandbox::start(&temp);
    let mut editor = Editor::new(Options {
        file: Some(path.clone()),
        content: Some(temp.0.join("no cook here")),
        link: Some(sandbox.address),
    });

    // What the status says about the file, and the fight as shipped.
    let (document, _) = sandbox.until(&mut editor, STARTUP, "the device", |editor| {
        let device = editor.link().device()?;
        device.documents.iter().find(|document| document.source.ends_with("slice.moves.ron")).cloned()
    });
    assert_eq!(document.edits, 0);
    let (before, _) = sandbox.until(&mut editor, PATIENCE, "an attempt", |editor| light_at(editor, 0));
    assert_eq!((before.startup, before.active, before.recovery), (5, 3, 18));
    assert_eq!(before.hit_windows[0].from, 5);

    // Its wind-up two ticks longer, its hit window with it and its recovery two shorter: the calls a drag
    // ends in, and the key's own save and push.
    let entry = hit_window();
    let edit = editor.file_mut().expect("a file").edit("later", |text| {
        text.set_phases(entry.mv, Phases { startup: 7, active: 3, recovery: 16 })?;
        text.set_window(entry, 7, 10)
    });
    edit.expect("an edit");
    editor.refresh();
    assert!(editor.problems().is_empty(), "{:?}", editor.problems());
    let save = Stopwatch::start();
    editor.save_and_push();
    assert_eq!(fs::read_to_string(&path).expect("saved"), editor.file().expect("a file").text());

    let (owner, _) =
        sandbox.until(&mut editor, PATIENCE, "the reply", |editor| match editor.link().last_push() {
            Some(Pushed::Took { owner, .. }) => Some(owner.clone()),
            Some(other) => panic!("{other:?}"),
            None => None,
        });
    assert!(owner.contains("revision 1"), "{owner}");
    let replied = save.elapsed();
    let (after, _) = sandbox.until(&mut editor, PATIENCE, "the new revision", |editor| light_at(editor, 1));
    assert_eq!((after.startup, after.active, after.recovery), (7, 3, 16));
    assert_eq!(after.hit_windows[0].from, 7);
    assert!(after.serial > before.serial, "a following attempt, not the one playing");
    println!("save to reply {replied:?}, save to a following attempt {:?}", save.elapsed());

    // Past the move's end: the editor sees it too, the save still happens (the file is the person's) and the
    // device refuses, naming move and track.
    let edit = editor.file_mut().expect("a file").edit("too far", |text| text.set_window(entry, 7, 99));
    edit.expect("an edit");
    editor.refresh();
    let edited = editor.file().expect("a file").text().to_owned();
    assert_eq!(editor.problems()[0].track.as_deref(), Some("hitboxes"));
    editor.save_and_push();
    let (refusals, _) =
        sandbox.until(&mut editor, PATIENCE, "the refusal", |editor| match editor.link().last_push() {
            Some(Pushed::Refused(refusals)) => Some(refusals.clone()),
            Some(other) => panic!("{other:?}"),
            None => None,
        });
    assert_eq!(refusals[0].in_move.as_deref(), Some("light_1"), "{refusals:?}");
    assert_eq!(refusals[0].track.as_deref(), Some("hitboxes"), "{refusals:?}");
    assert_eq!(editor.file().expect("a file").text(), edited, "the edit stands");

    // The fight goes on as it was.
    let serial = after.serial;
    let (later, _) = sandbox.until(&mut editor, PATIENCE, "another attempt on what it had", |editor| {
        light_at(editor, 1).filter(|attempt| attempt.serial > serial)
    });
    assert_eq!(later.startup, 7);

    // One undo, one push: the next good one works.
    editor.undo();
    let edit = editor.file_mut().expect("a file").edit("one more", |text| text.set_window(entry, 7, 9));
    edit.expect("an edit");
    editor.refresh();
    editor.save_and_push();
    sandbox.until(&mut editor, PATIENCE, "the reply", |editor| match editor.link().last_push() {
        Some(Pushed::Took { .. }) => Some(()),
        Some(other) => panic!("{other:?}"),
        None => None,
    });
    let (again, _) =
        sandbox.until(&mut editor, PATIENCE, "the newest revision", |editor| light_at(editor, 2));
    assert_eq!(again.hit_windows[0].to, 9);

    // A file no scenario lists is the device's to refuse, and it says so. The name goes to the same device, from a
    // second editor.
    let other = temp.0.join("nobody.moves.ron");
    fs::write(&other, PLAYER).expect("writes");
    let mut second = Editor::new(Options {
        file: Some(other),
        content: Some(temp.0.join("no cook here")),
        link: Some(sandbox.address),
    });
    sandbox.until(&mut second, PATIENCE, "the device, from the second editor", |editor| {
        editor.link().device().cloned()
    });
    second.save_and_push();
    let (refusals, _) =
        sandbox.until(&mut second, PATIENCE, "its refusal", |editor| match editor.link().last_push() {
            Some(Pushed::Refused(refusals)) => Some(refusals.clone()),
            Some(other) => panic!("{other:?}"),
            None => None,
        });
    let said = &refusals[0].message;
    assert!(said.contains("nobody.moves.ron"), "{said}");
    assert!(second.problems().iter().any(|problem| problem.from_device), "{:?}", second.problems());

    drop(second);
    drop(editor);
    sandbox.quit();
}
