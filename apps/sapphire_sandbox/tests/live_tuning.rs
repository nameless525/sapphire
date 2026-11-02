//! Live tuning end to end (M2 T2, exit test 2): a dev sandbox offscreen, its fight running from a script,
//! and an edited document reaching it over live link on a private port, or through hot reload's watcher.
//! Each test reads what the fight's next attempt plays back through the status, so it sees the windows
//! change in the simulation that runs them.
//!
//! They need a `dev` build, a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p sapphire_sandbox --features dev --test live_tuning -- --ignored --test-threads=1`. CI's
//! `GPU tests` job runs them on lavapipe. Unix only, for the clean quit a `kill -TERM` gives, which is
//! what writes a recording.
#![cfg(all(feature = "dev", unix))]

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sapphire_camera::CameraTuning;
use sapphire_gameplay::replay::{Player, Replay, SCENARIOS};
use sapphire_livelink::protocol::{DeviceStatus, MoveAttempt};
use sapphire_livelink::{Client, LinkError};
use sapphire_moves::MoveSetDocument;

const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const SLICE_FILE: &str = "slice.moves.ron";
const GRAPH: &str = include_str!("../../../content/lookdev/character/character.animgraph.ron");

/// How long a sandbox has to build its scene's pipelines, slowly on a CPU rasterizer.
const STARTUP: Duration = Duration::from_secs(300);
/// How long a fight has to show something it should within a moment, under a loaded machine.
const PATIENCE: Duration = Duration::from_secs(60);

/// A directory of this test's own, gone when it ends.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire_live_tuning_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn text(&self, name: &str) -> String {
        self.path(name).to_str().expect("a UTF-8 temp path").to_owned()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The dev sandbox, running offscreen, listening on a port of its own.
struct App {
    child: Child,
    address: SocketAddr,
    log: PathBuf,
}

impl App {
    fn start(temp: &Temp, args: &[&str]) -> Self {
        let address = {
            // A port nobody has, so two runs never talk to each other's app on the default one.
            let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
            listener.local_addr().expect("its address")
        };
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let log = temp.path("sandbox.log");
        let file = fs::File::create(&log).expect("a log");
        let child = Command::new(env!("CARGO_BIN_EXE_sapphire_sandbox"))
            .args(["--offscreen", "--size", "160x90", "--frames", "72000", "--live-link"])
            .arg(address.to_string())
            .args(args)
            .env("SAPPHIRE_CONTENT_DIR", workspace.join("target/content/desktop"))
            .stdout(file.try_clone().expect("the log"))
            .stderr(Stdio::from(file))
            .spawn()
            .expect("the sandbox starts");
        Self { child, address, log }
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// A connection, once it answers.
    fn connect(&mut self) -> Client {
        let started = Instant::now();
        loop {
            if let Ok(mut client) = Client::connect(self.address)
                && client.status().is_ok()
            {
                return client;
            }
            self.assert_running();
            assert!(started.elapsed() < STARTUP, "it never answered:\n{}", self.log());
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn assert_running(&mut self) {
        if let Some(status) = self.child.try_wait().expect("the sandbox's status") {
            panic!("the sandbox exited with {status}:\n{}", self.log());
        }
    }

    /// Asks the status until `done` finds what it wants, and returns that.
    fn until<T>(
        &mut self,
        client: &mut Client,
        what: &str,
        mut done: impl FnMut(&DeviceStatus) -> Option<T>,
    ) -> T {
        let started = Instant::now();
        loop {
            let status = client.status().expect("a status");
            if let Some(found) = done(&status) {
                return found;
            }
            self.assert_running();
            assert!(started.elapsed() < PATIENCE, "never saw {what}: {status:#?}\n{}", self.log());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits for `line` in its log.
    fn until_logged(&mut self, line: &str) {
        let started = Instant::now();
        while !self.log().contains(line) {
            self.assert_running();
            assert!(started.elapsed() < PATIENCE, "it never logged {line:?}:\n{}", self.log());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Quits as Ctrl-C does, and waits for a clean exit.
    fn quit(mut self) -> String {
        let pid = self.child.id().to_string();
        let killed = Command::new("kill").args(["-TERM", &pid]).status().expect("kill runs");
        assert!(killed.success());
        let status = self.child.wait().expect("it exits");
        let log = self.log();
        assert!(status.success(), "it exited with {status}:\n{log}");
        log
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The test moves' `light_1`, with its wind-up and its hit `by` ticks later and its length the same.
fn light_later_by(by: u32) -> String {
    let mut document = MoveSetDocument::parse(SLICE).expect("parses");
    let light = document.moves.iter_mut().find(|mv| mv.name == "light_1").expect("there");
    light.startup += by;
    light.recovery -= by;
    for hitbox in &mut light.hitboxes {
        hitbox.from += by;
        hitbox.to += by;
    }
    document.to_text().expect("writes")
}

/// The test moves with a hit window past the end of `light_1`.
fn a_window_past_the_end() -> String {
    let mut document = MoveSetDocument::parse(SLICE).expect("parses");
    let light = document.moves.iter_mut().find(|mv| mv.name == "light_1").expect("there");
    light.hitboxes[0].to = light.startup + light.active + light.recovery + 10;
    document.to_text().expect("writes")
}

/// `light_1`, as the player's next attempt plays it, at this revision.
fn light_at(status: &DeviceStatus, revision: u32) -> Option<MoveAttempt> {
    status.attempts.iter().find(|attempt| attempt.name == "light_1" && attempt.revision == revision).cloned()
}

/// The brief's test: an edited move, pushed, plays on a following attempt, a bad one changes nothing and
/// the next good one works. What the fight recorded ends at the first edit, and plays back to there.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn a_pushed_move_plays_on_the_next_attempt() {
    let temp = Temp::new("pushed");
    let replay = temp.text("session.sreplay");
    let args = ["--no-hot-reload", "--record", &replay, "--scenario", "duel", "--script", "attempts"];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();

    // As shipped: it's what the document says, and the status says what's playing.
    let before = app.until(&mut client, "an attempt", |status| light_at(status, 0));
    assert_eq!((before.startup, before.active, before.recovery), (5, 3, 18));
    assert_eq!(before.hit_windows[0].from, 5);

    let reply = client.push_document(SLICE_FILE, light_later_by(2)).expect("a good document");
    assert!(reply.contains("revision 1"), "{reply}");

    // A later attempt plays the new window.
    let after = app.until(&mut client, "the new revision", |status| light_at(status, 1));
    assert_eq!((after.startup, after.active, after.recovery), (7, 3, 16));
    assert_eq!(after.hit_windows[0].from, 7);
    assert!(after.serial > before.serial, "a following attempt, not the one playing");
    assert_eq!(after.newest_revision, 1);
    app.until_logged("the next attempt plays the new moves");

    // A bad document says which move and track, and the fight goes on as it was.
    let refused = client.push_document(SLICE_FILE, a_window_past_the_end()).expect_err("a bad document");
    let LinkError::Refused(text) = &refused else { panic!("{refused}") };
    assert!(text.contains("move `light_1`, track `hitboxes`"), "{text}");
    let refused = client.push_document(SLICE_FILE, "(not a document".to_owned()).expect_err("no envelope");
    assert!(matches!(refused, LinkError::Refused(_)), "{refused}");
    let serial = after.serial;
    let later = app.until(&mut client, "another attempt on what it had", |status| {
        light_at(status, 1).filter(|attempt| attempt.serial > serial)
    });
    assert_eq!(later.startup, 7);
    let status = client.status().expect("a status");
    let listed = status.documents.iter().find(|document| document.source.ends_with(SLICE_FILE));
    assert_eq!(listed.expect("it's listed").edits, 1, "only what changed something counts");
    assert!(status.refused >= 2, "{status:#?}");

    // The next good one works.
    let reply = client.push_document(SLICE_FILE, light_later_by(3)).expect("a good document");
    assert!(reply.contains("revision 2"), "{reply}");
    let again = app.until(&mut client, "the newest revision", |status| light_at(status, 2));
    assert_eq!(again.startup, 8);

    // Protocol 4's list: a pack served from a push shows.
    assert!(client.status().expect("a status").pushed_packs.is_empty());
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pack = fs::read(workspace.join("target/content/desktop/shared.spack")).expect("the cooked pack");
    client.push_pack("shared", &pack).expect("a push");
    let pushed = client.status().expect("a status").pushed_packs;
    assert_eq!(pushed.len(), 1);
    assert_eq!((pushed[0].name.as_str(), pushed[0].len), ("shared", pack.len() as u64));

    // The recording: cut at the first edit, and good up to there.
    let log = app.quit();
    assert!(log.contains("a live edit ends the recording's file"), "{log}");
    let replay = Replay::decode(&fs::read(&replay).expect("written at quit")).expect("reads");
    assert!(replay.ticks() > 0, "{log}");
    let mut player = Player::new(replay, SCENARIOS).expect("it ran on this build's moves up to there");
    player.run_to_end();
}

/// The other route: hot reload's watcher, from a save to a following attempt.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn a_saved_move_plays_on_the_next_attempt() {
    let temp = Temp::new("saved");
    let document = temp.path(SLICE_FILE);
    fs::write(&document, SLICE).expect("writes");
    let (replay, watched) = (temp.text("session.sreplay"), temp.text(SLICE_FILE));
    let args =
        ["--record", &replay, "--scenario", "duel", "--script", "attempts", "--watch-document", &watched];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();
    app.until(&mut client, "an attempt", |status| light_at(status, 0));

    fs::write(&document, light_later_by(2)).expect("writes");
    let after = app.until(&mut client, "the saved revision", |status| light_at(status, 1));
    assert_eq!((after.startup, after.hit_windows[0].from), (7, 7));

    // A bad save: the log has the issue, and the fight has what it had.
    fs::write(&document, a_window_past_the_end()).expect("writes");
    app.until_logged("move `light_1`, track `hitboxes`");
    let serial = after.serial;
    app.until(&mut client, "another attempt on what it had", |status| {
        light_at(status, 1).filter(|attempt| attempt.serial > serial)
    });

    // A save of what it has is no edit, and the next change works.
    fs::write(&document, light_later_by(2)).expect("writes");
    fs::write(&document, light_later_by(4)).expect("writes");
    let again = app.until(&mut client, "the next revision", |status| light_at(status, 2));
    assert_eq!(again.startup, 9);
}

/// The camera's tuning and the character's graph reach the stages that have them, and each bad document
/// says what's wrong and keeps what's there.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn the_camera_and_the_graph_reach_a_running_app() {
    let temp = Temp::new("camera");
    let replay = temp.text("session.sreplay");
    let args =
        ["--no-hot-reload", "--arena", "--record", &replay, "--scenario", "arena", "--script", "attempts"];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();
    let camera = "default_camera.ron";
    let mut tuning = CameraTuning::default_tuning();
    tuning.explore.distance += 1.0;
    let reply = client.push_document(camera, tuning.to_ron().expect("writes")).expect("good tuning");
    assert!(reply.contains("camera"), "{reply}");
    tuning.explore.distance = -1.0;
    let refused = client.push_document(camera, tuning.to_ron().expect("writes")).expect_err("bad tuning");
    assert!(refused.to_string().contains("explore.distance"), "{refused}");
    drop(client);
    app.quit();

    // The graph, on a character that plays one.
    let temp = Temp::new("graph");
    let args = ["--no-hot-reload", "--look-dev", "wide", "--graph", "cancel"];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();
    let graph = "character.animgraph.ron";
    let edited = GRAPH.replacen("smoothing: 3.0", "smoothing: 5.0", 1);
    assert_ne!(edited, GRAPH, "the edit is still there to make");
    let reply = client.push_document(graph, edited).expect("a good graph");
    assert!(reply.contains("animation graph"), "{reply}");

    // A document's own check, and a clip nothing cooked, which only the app can find out.
    let refused = client
        .push_document(graph, GRAPH.replacen("start: \"ground\"", "start: \"nowhere\"", 1))
        .expect_err("no such state");
    assert!(refused.to_string().contains("nowhere"), "{refused}");
    let missing = GRAPH.replacen(
        "(name: \"idle\", source: \"0853fc78-9a5d-4f9d-bcfe-1c8a485a0db4\")",
        "(name: \"idle\", source: \"00000000-0000-4000-8000-000000000000\")",
        1,
    );
    assert_ne!(missing, GRAPH);
    let refused = client.push_document(graph, missing).expect_err("no such clip");
    assert!(refused.to_string().contains("idle"), "{refused}");

    // The graph show's moves are its own, and any other's name reaches nothing.
    let show = include_str!("../../../content/lookdev/character/show.moves.ron");
    let edited = show.replacen("recovery: ", "recovery: 1", 1);
    let reply = client.push_document("show.moves.ron", edited).expect("good moves");
    assert!(reply.contains("revision 1"), "{reply}");
    let refused = client.push_document("nobody.moves.ron", show.to_owned()).expect_err("no owner");
    assert!(refused.to_string().contains("nobody.moves.ron"), "{refused}");
    drop(client);
    app.quit();
}
