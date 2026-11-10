//! Live tuning end to end (M2 T2, exit test 2): a dev sandbox offscreen, its fight running from a script,
//! and an edited document reaching it over live link on a private port, or through hot reload's watcher.
//! Each test reads what the fight's next attempt plays back through the status, so it sees the windows
//! change in the simulation that runs them.
//!
//! They need a `dev` build, a GPU and cooked content: `cargo xtask content`, then
//! `cargo test -p sapphire_sandbox --features dev --test live_tuning -- --ignored --test-threads=1`. CI's
//! `App tests` job runs them on lavapipe. Unix only, for the clean quit a `kill -TERM` gives, which is
//! what writes a recording.
#![cfg(all(feature = "dev", unix))]

use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sapphire_camera::CameraTuning;
use sapphire_gameplay::replay::{
    LiveMoves, Player, Replay, SCENARIOS, compile_document, find_scenario, find_script, player_moves,
};
use sapphire_livelink::protocol::{DeviceStatus, MoveAttempt};
use sapphire_livelink::{Client, LinkError};
use sapphire_moves::MoveSetDocument;
use sapphire_time::Stopwatch;

const SLICE: &str = include_str!("../../../crates/sapphire_moves/test_content/slice.moves.ron");
const SLICE_FILE: &str = "slice.moves.ron";
const GRAPH: &str = include_str!("../../../content/lookdev/character/character.animgraph.ron");
const CUES: &str = include_str!("../../../crates/sapphire_cues/default_cues.ron");
const CUES_FILE: &str = "default_cues.ron";
const EFFECTS: &str = include_str!("../../../crates/sapphire_vfx/default_effects.ron");
const EFFECTS_FILE: &str = "default_effects.ron";
const BINDINGS: &str = include_str!("../../../crates/sapphire_controls/combat_bindings.ron");
const BINDINGS_FILE: &str = "combat_bindings.ron";
const TARGETING: &str = include_str!("../../../crates/sapphire_gameplay/default_targeting.ron");
const TARGETING_FILE: &str = "default_targeting.ron";
const BODY: &str = include_str!("../../../crates/sapphire_body_pose/default_body.ron");
const BODY_FILE: &str = "default_body.ron";
const MOTION: &str = include_str!("../../../crates/sapphire_anim/default_character_motion.ron");
const MOTION_FILE: &str = "default_character_motion.ron";
const HUD: &str = include_str!("../../../crates/sapphire_ui/default_hud.ron");
const HUD_FILE: &str = "default_hud.ron";
const PLAYER: &str = include_str!("../../../content/combat/player/player.moves.ron");
const PLAYER_FILE: &str = "player.moves.ron";
const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const WARRIOR_FILE: &str = "warrior.moves.ron";
const SCREEN: &str = include_str!("../../../content/ui/pause.ui.ron");
const SCREEN_FILE: &str = "pause.ui.ron";

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
            .env("SAPPHIRE_LOG", "info")
            .env_remove("RUST_LOG")
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
        let started = Stopwatch::start();
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
        done: impl FnMut(&DeviceStatus) -> Option<T>,
    ) -> T {
        self.until_within(PATIENCE, client, what, done)
    }

    /// [`Self::until`], for something that takes as long as it takes.
    fn until_within<T>(
        &mut self,
        patience: Duration,
        client: &mut Client,
        what: &str,
        mut done: impl FnMut(&DeviceStatus) -> Option<T>,
    ) -> T {
        let started = Stopwatch::start();
        loop {
            let status = client.status().expect("a status");
            if let Some(found) = done(&status) {
                return found;
            }
            self.assert_running();
            assert!(started.elapsed() < patience, "never saw {what}: {status:#?}\n{}", self.log());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits for `line` in its log.
    fn until_logged(&mut self, line: &str) {
        let started = Stopwatch::start();
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
    later_by(SLICE, by)
}

/// This document's `light_1`, with its wind-up and its hit `by` ticks later and its length the same.
fn later_by(text: &str, by: u32) -> String {
    let mut document = MoveSetDocument::parse(text).expect("parses");
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
    let mut player = Player::new(replay, &SCENARIOS).expect("it ran on this build's moves up to there");
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

/// The likeliest thing to tune, on the moves a player fights with: a saved `run_speed` reaches a running
/// fight with no cook and no restart, and the next step reads it, with no press to wait for. A number past
/// its limit, saved or pushed, names the file and the number, and keeps what's playing.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn a_saved_run_speed_moves_the_next_step_of_a_real_fight() {
    let temp = Temp::new("movement");
    let document = temp.path(PLAYER_FILE);
    fs::write(&document, PLAYER).expect("writes");
    let watched = temp.text(PLAYER_FILE);
    let args = [
        "--arena",
        "--fight",
        "--script",
        "attempts",
        "--tier",
        "desktop-low",
        "--ao",
        "off",
        "--watch-document",
        &watched,
    ];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();
    app.until(&mut client, "an attempt", |status| light_at(status, 0));
    assert!(PLAYER.contains("run_speed: 6.0,"), "the document writes the number out");

    let quicker = PLAYER.replacen("run_speed: 6.0,", "run_speed: 8.5,", 1);
    fs::write(&document, &quicker).expect("writes");
    const STEPPED: &str = "the next step moves by the new numbers";
    app.until_logged(STEPPED);
    let log = app.log();
    let line = log.lines().find(|line| line.contains(STEPPED)).expect("just logged");
    assert!(line.contains("run_speed=8.5") && line.contains(PLAYER_FILE), "{line}");

    let refused = client
        .push_document(PLAYER_FILE, PLAYER.replacen("run_speed: 6.0,", "run_speed: 600.0,", 1))
        .expect_err("past its limit");
    let LinkError::Refused(text) = &refused else { panic!("{refused}") };
    assert!(text.contains(PLAYER_FILE) && text.contains("`run_speed` is 600"), "{text}");
    let reply = client.push_document(PLAYER_FILE, quicker).expect("what's playing, again");
    assert!(reply.contains("still revision 1") && !reply.contains("new numbers"), "{reply}");
    app.quit();
}

/// Exit test 2 on the moves a player fights with: `--arena --fight`, whose simulation runs on its cooked
/// copy of `player.moves.ron`, takes a saved timing with no cook, and a following attempt plays it. The
/// document as it stands is no edit, so the copy, the pack and the document agree about what's running. A
/// length, which is its clip's, is refused by name. The next fight starts at revision 0 of the edits, and
/// it's the fight a headless run of those edits plays, tick for tick.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn a_saved_timing_of_the_real_moves_plays_in_a_fight_and_the_next_fight_starts_from_it() {
    let temp = Temp::new("fight");
    let document = temp.path(PLAYER_FILE);
    fs::write(&document, PLAYER).expect("writes");
    let watched = temp.text(PLAYER_FILE);
    let args = [
        "--arena",
        "--fight",
        "--script",
        "attempts",
        "--tier",
        "desktop-low",
        "--ao",
        "off",
        "--watch-document",
        &watched,
    ];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();

    let before = app.until(&mut client, "an attempt", |status| light_at(status, 0));
    assert_eq!((before.startup, before.active, before.recovery), (7, 4, 23));
    assert_eq!(before.hit_windows[0].from, 7);
    let reply = client.push_document(PLAYER_FILE, PLAYER.to_owned()).expect("the document as it stands");
    assert!(reply.contains("with the same moves, still revision 0"), "{reply}");

    // The saved file, with no cook. A following attempt reads it.
    let edited = later_by(PLAYER, 2);
    fs::write(&document, &edited).expect("writes");
    let after = app.until(&mut client, "the saved revision", |status| light_at(status, 1));
    assert_eq!((after.startup, after.active, after.recovery), (9, 4, 21));
    assert_eq!((after.hit_windows[0].from, after.hit_windows[0].to), (9, 13));
    assert!(after.serial > before.serial, "a following attempt, not the one playing");
    assert_eq!(after.newest_revision, 1);
    app.until_logged("the next attempt plays the new moves");
    let status = client.status().expect("a status");
    let listed = status.documents.iter().find(|document| document.source.ends_with(PLAYER_FILE));
    assert_eq!(listed.expect("it's listed").edits, 1, "the same bytes were no edit");

    // A length is its clip's: only a cook can change one, and it says so by name.
    let mut longer = MoveSetDocument::parse(PLAYER).expect("parses");
    longer.moves.iter_mut().find(|mv| mv.name == "light_1").expect("there").recovery += 1;
    let refused = client
        .push_document(PLAYER_FILE, longer.to_text().expect("writes"))
        .expect_err("a length its clip doesn't have");
    let LinkError::Refused(text) = &refused else { panic!("{refused}") };
    assert!(text.contains("move `light_1`"), "{text}");

    // The warrior's document is a fight's too. A frail one ends this fight soon.
    let frail = WARRIOR.replacen("health: 240", "health: 30", 1);
    assert_ne!(frail, WARRIOR);
    let reply = client.push_document(WARRIOR_FILE, frail.clone()).expect("a good document");
    assert!(reply.contains("revision 1"), "{reply}");

    // Someone falls, and the next fight starts from both edits, as what they are: revision 0.
    let next = app.until_within(STARTUP, &mut client, "the next fight", |status| light_at(status, 0));
    assert_eq!((next.startup, next.active, next.recovery), (9, 4, 21));
    assert_eq!(next.hit_windows[0].from, 9);
    assert_eq!(next.newest_revision, 0);
    app.until_logged("a fight ended");

    // Every fight after the edits is what a build with them in its documents would play. The first
    // crossed them, so it has nothing to be held to.
    let log = app.quit();
    let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let mut live = LiveMoves::default();
    let baked = player_moves().expect("the simulation's copy reads");
    live.remember(PLAYER_FILE, compile_document(&edited, Some(&baked)).expect("compiles").set);
    live.remember(WARRIOR_FILE, compile_document(&frail, None).expect("compiles").set);
    let script = find_script("attempts").expect("listed").actions;
    let mut held = 0;
    for line in log.lines().filter(|line| line.contains("a fight ended")).skip(1) {
        let field = |name: &str| {
            let start = line.find(name).unwrap_or_else(|| panic!("no {name} in: {line}")) + name.len();
            line[start..].split_whitespace().next().expect("a value").to_owned()
        };
        let ticks: u64 = field(" ticks=").parse().expect("a number");
        let (mut sim, cast) = live.start(scenario, scenario.config).expect("sets up");
        for tick in 0..ticks {
            sim.step(&script(tick, &cast));
        }
        assert_eq!(format!("{:016x}", sim.checksum()), field(" checksum="), "{line}");
        assert!(line.contains("allocations=0"), "{line}");
        held += 1;
    }
    assert!(held >= 1, "the fight after the edits says how it went at quit:\n{log}");
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

/// A screen on show takes a document pushed or saved, and one that's wrong in any way a cook would refuse
/// says where and keeps what's on show. Without `--ui-game` there's nothing to take it, and with it no other
/// screen's document.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn a_screen_on_show_takes_a_pushed_and_a_saved_document() {
    let temp = Temp::new("screen");
    let document = temp.path(SCREEN_FILE);
    fs::write(&document, SCREEN).expect("writes");
    let watched = temp.text(SCREEN_FILE);
    let args = ["--ui-game", "pause", "--ui-locale", "ja", "--watch-document", &watched];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();

    let edited = SCREEN.replacen("gap: 8", "gap: 12", 1);
    assert_ne!(edited, SCREEN, "the edit is still there to make");
    let reply = client.push_document(SCREEN_FILE, edited).expect("a good screen");
    assert!(reply.contains("screen"), "{reply}");

    // Its own check, by field, and a path its model doesn't have.
    let refused = client
        .push_document(SCREEN_FILE, SCREEN.replacen("parent: \"panel\"", "parent: \"nobody\"", 1))
        .expect_err("no such parent");
    assert!(refused.to_string().contains("nobody"), "{refused}");
    let refused = client
        .push_document(SCREEN_FILE, SCREEN.replacen("path: \"can_leave\"", "path: \"mana\"", 1))
        .expect_err("no such value");
    assert!(refused.to_string().contains("mana"), "{refused}");
    let refused = client.push_document("title.ui.ron", SCREEN.to_owned()).expect_err("not on show");
    assert!(refused.to_string().contains("pause.ui.ron"), "{refused}");

    // Saved, through hot reload's watcher.
    fs::write(&document, SCREEN.replacen("gap: 8", "gap: 16", 1)).expect("saved");
    app.until(&mut client, "the saved screen taken", |status| {
        (edits(status, SCREEN_FILE) == Some(2)).then_some(())
    });

    // A bad save is logged by field, and the next good one still works.
    fs::write(&document, SCREEN.replacen("path: \"can_leave\"", "path: \"mana\"", 1)).expect("saved");
    app.until_logged("mana");
    fs::write(&document, SCREEN).expect("saved");
    app.until(&mut client, "the good save after it", |status| {
        (edits(status, SCREEN_FILE) == Some(3)).then_some(())
    });
    drop(client);
    app.quit();

    let temp = Temp::new("no_screen");
    let mut app = App::start(&temp, &["--no-hot-reload"]);
    let mut client = app.connect();
    let refused = client.push_document(SCREEN_FILE, SCREEN.to_owned()).expect_err("nothing on show");
    assert!(refused.to_string().contains("--ui-game"), "{refused}");
    drop(client);
    app.quit();
}

/// How often the status says this document has been taken.
fn edits(status: &DeviceStatus, document: &str) -> Option<u32> {
    status.documents.iter().find(|taken| taken.source.ends_with(document)).map(|taken| taken.edits)
}

/// The cue table has one route, pushed or saved, to what fires a fight's cues. New camera tuning resolves
/// the table's shakes again, and is refused if one has gone. With nothing firing cues, a table says so.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn the_cue_table_and_its_shakes_reach_what_fires_a_fights_cues() {
    let temp = Temp::new("cues");
    let replay = temp.text("session.sreplay");
    let saved = temp.text(CUES_FILE);
    fs::write(&saved, CUES).expect("a copy to save over");
    let args = [
        "--record",
        &replay,
        "--scenario",
        "duel",
        "--script",
        "attempts",
        "--cues",
        "--watch-document",
        &saved,
    ];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();

    let reply = client.push_document(CUES_FILE, CUES.replacen("strength: 1.8", "strength: 1.6", 1));
    assert!(reply.expect("a good table").contains("cue table"));
    let naming = CUES.replacen("preset: \"hit_light\"", "preset: \"no_such_shake\"", 1);
    let refused = client.push_document(CUES_FILE, naming).expect_err("no such shake");
    assert!(refused.to_string().contains("no_such_shake"), "{refused}");

    fs::write(&saved, CUES.replacen("strength: 1.8", "strength: 1.5", 1)).expect("saved");
    app.until(&mut client, "the saved table taken", |status| {
        (edits(status, CUES_FILE) == Some(2)).then_some(())
    });

    let camera = "default_camera.ron";
    let mut tuning = CameraTuning::default_tuning();
    tuning.shakes.presets.rotate_left(1);
    let reply = client.push_document(camera, tuning.to_ron().expect("writes")).expect("every shake's there");
    assert!(reply.contains("camera"), "{reply}");
    tuning.shakes.presets.retain(|preset| preset.name != "hit_light");
    let refused =
        client.push_document(camera, tuning.to_ron().expect("writes")).expect_err("the table fires it");
    assert!(refused.to_string().contains("hit_light"), "{refused}");
    let status = client.status().expect("a status");
    assert_eq!(edits(&status, camera), Some(1));
    assert_eq!(edits(&status, CUES_FILE), Some(2), "a refusal is no edit, and no route takes one twice");

    let reply = client.push_document(EFFECTS_FILE, EFFECTS.replacen("lifetime: 0.2,", "lifetime: 0.18,", 1));
    assert!(reply.expect("good effects").contains("effects"));
    let gone = EFFECTS.replacen("name: \"cut_spark\"", "name: \"no_such_spark\"", 1);
    let refused = client.push_document(EFFECTS_FILE, gone).expect_err("the table fires it");
    assert!(refused.to_string().contains("cut_spark"), "{refused}");
    let status = client.status().expect("a status");
    assert_eq!(edits(&status, EFFECTS_FILE), Some(1), "a refusal is no edit");
    drop(client);
    app.quit();

    let temp = Temp::new("no_cues");
    let replay = temp.text("session.sreplay");
    let args = ["--no-hot-reload", "--record", &replay, "--scenario", "duel", "--script", "attempts"];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();
    let refused = client.push_document(CUES_FILE, CUES.to_owned()).expect_err("nothing fires them");
    assert!(refused.to_string().contains("--cues"), "{refused}");
    let status = client.status().expect("a status");
    assert_eq!(edits(&status, CUES_FILE), None, "it isn't a document of this run");
    drop(client);
    app.quit();
}

/// `combat_bindings.ron`, `default_effects.ron` and the numbers that used to be code come by the one route, pushed
/// or saved: a good document is taken, and a bad one is refused by name, with the count of what's been taken
/// and the app going on as they were. An offscreen run has no input to rebind, so bindings reach it and are
/// refused for that: `input.rs` tests what a window's input does with them.
#[test]
#[ignore = "needs a dev build, a GPU and cooked content"]
fn the_documents_that_were_code_take_the_route_the_others_do() {
    let temp = Temp::new("documents");
    let saved = temp.text(HUD_FILE);
    fs::write(&saved, HUD).expect("a copy to save over");
    let args = [
        "--arena",
        "--fight",
        "--script",
        "fight-back",
        "--cues",
        "--hud",
        "live",
        "--watch-document",
        &saved,
    ];
    let mut app = App::start(&temp, &args);
    let mut client = app.connect();

    let good = [
        (EFFECTS_FILE, EFFECTS.to_owned(), "effects"),
        (TARGETING_FILE, TARGETING.replacen("range: 12.0", "range: 13.0", 1), "targeting"),
        (BODY_FILE, BODY.replacen("rate: 4.5", "rate: 5.0", 1), "next fight's bodies"),
        (MOTION_FILE, MOTION.replacen("root_inertia: 0.25", "root_inertia: 0.3", 1), "motion"),
        (HUD_FILE, HUD.replacen("margin: 16.0", "margin: 18.0", 1), "HUD"),
    ];
    for (file, text, reply_says) in good {
        let reply = client.push_document(file, text).unwrap_or_else(|error| panic!("{file}: {error}"));
        assert!(reply.contains(reply_says), "{file}: {reply}");
    }
    let status = client.status().expect("a status");
    for file in [EFFECTS_FILE, TARGETING_FILE, BODY_FILE, MOTION_FILE, HUD_FILE] {
        assert_eq!(edits(&status, file), Some(1), "{file}");
    }
    let bindings = BINDINGS.replace("source: Key(J)", "source: Key(U)");
    let no_input = client.push_document(BINDINGS_FILE, bindings).expect_err("nothing plays here");
    assert!(no_input.to_string().contains("no bindings to replace"), "{no_input}");

    let bad = [
        (BINDINGS_FILE, BINDINGS.replacen("source:", "sauce:", 1), "source"),
        (EFFECTS_FILE, EFFECTS.replacen("lifetime:", "lifetimes:", 1), "lifetimes"),
        (TARGETING_FILE, TARGETING.replacen("range: 12.0", "range: -1.0", 1), "soft_lock.range"),
        (BODY_FILE, BODY.replacen("soften:", "soft:", 1), "soft"),
        (MOTION_FILE, MOTION.replacen("bend_limit:", "bend:", 1), "bend"),
        (HUD_FILE, HUD.replacen("margin:", "marginn:", 1), "marginn"),
    ];
    for (file, text, names) in bad {
        let refused = client.push_document(file, text).expect_err(file);
        assert!(refused.to_string().contains(names), "{file}: {refused}");
    }
    let status = client.status().expect("a status");
    for file in [EFFECTS_FILE, TARGETING_FILE, BODY_FILE, MOTION_FILE, HUD_FILE] {
        assert_eq!(edits(&status, file), Some(1), "{file} kept what it had");
    }
    assert_eq!(edits(&status, BINDINGS_FILE), Some(0), "none taken");

    // Saved, by the file it watches: the other way in.
    fs::write(&saved, HUD.replacen("margin: 16.0", "margin: 20.0", 1)).expect("saved");
    app.until(&mut client, "the saved design taken", |status| {
        (edits(status, HUD_FILE) == Some(2)).then_some(())
    });
    drop(client);
    app.quit();
}
