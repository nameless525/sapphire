//! What a session says (M4 F2): sessions kept as a playtest build keeps them, around runs of the replay suite
//! whose scripts say what they do, read back by [`Report::read`] and held to what the scripts do.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sapphire_core::thermal::{ThermalReading, ThermalState};
use sapphire_gameplay::replay::{ARCHITECTURE, Player, Replay};
use sapphire_playtest::{About, Build, Check, DOCUMENT_FILE, Device, MetEnd, Report, SESSIONS_DIR, Session};
use sapphire_script::scenarios::ALL;

const FRAME: Duration = Duration::from_micros(16_667);

/// What `dodge-counter`'s player lands, by its script: see `a_scripted_fight_reports_what_its_script_does`.
const HITS_LANDED: u32 = 2;
const DAMAGE_DEALT: u64 = 48;

/// A directory of its own, gone with the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sapphire-report-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn about() -> About {
    About {
        build: Build { app: "Sapphire".into(), version: "0.1.0".into(), id: "0.1.0+local".into() },
        device: Device {
            os: "Android".into(),
            class: "Mobile".into(),
            emulator: false,
            architecture: ARCHITECTURE.into(),
            ram_mib: 8_192,
            cores: 8,
        },
        began: Some(1_794_344_673),
    }
}

fn committed(name: &str) -> Replay {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays").join(format!("{name}.sreplay"));
    Replay::decode(&fs::read(path).unwrap()).unwrap()
}

/// A session of one run, the suite's `name` played a frame a step: every tenth frame late, the phone warming to
/// `Severe` from tick 600, and a mark at each of `marks`. Gives the session's directory.
fn session_of(scratch: &Scratch, name: &str, marks: &[u64]) -> PathBuf {
    let mut session =
        Session::begin(&scratch.0.join(SESSIONS_DIR), about(), Arc::new(common::Plain)).unwrap();
    let dir = session.dir().to_owned();
    let replay = committed(name);
    let mut player = Player::new(replay.clone(), &ALL).unwrap();
    while player.step() {
        let tick = player.simulation().tick();
        let state = if tick >= 600 { ThermalState::Severe } else { ThermalState::Nominal };
        let headroom = Some(if tick >= 600 { 0.25 } else { 0.75 });
        let frame = if tick.is_multiple_of(10) { FRAME * 2 } else { FRAME };
        session.frame(frame, FRAME, ThermalReading { state, headroom, ..ThermalReading::default() }, tick);
        if marks.contains(&tick) {
            session.mark(Some("that hurt"));
        }
    }
    session.run_ended(replay, player.finish().trail);
    session.end(None);
    dir
}

/// `dodge-counter` on the duel: the player dodges five times, three of them perfect, two of those answered by
/// the counter; the dodge that came too late is the one hit it takes, and nobody falls. Its file's presses
/// are those nine, and its idle stretches the rests between its rounds that no press breaks for a second.
#[test]
fn a_scripted_fight_reports_what_its_script_does() {
    let scratch = Scratch::new("duel");
    let dir = session_of(&scratch, "dodge-counter", &[256]);
    let report = Report::read(&dir, &ALL).unwrap();

    assert_eq!(report.runs.len(), 1);
    let run = &report.runs[0];
    assert_eq!((run.number, run.scenario.as_str(), run.ticks), (1, "duel", 1000));
    assert_eq!(run.check, Check::Same, "it replays to the checksum the session kept");
    assert_eq!(run.parts.len(), 1, "a duel has no zone, so no encounter");
    let part = &run.parts[0];
    assert_eq!((part.from, part.to, part.encounter.as_ref()), (1, 1000, None));
    let counts = part.counts;
    assert_eq!((counts.dodges, counts.perfect_dodges, counts.counters), (5, 3, 2));
    assert_eq!((counts.hits_taken, counts.falls), (1, 0));
    assert!(counts.damage_taken > 0);
    // Its two counters land, on ticks 61 and 661, as gameplay's dodges tests hold; the plain answer at 501 and
    // the swing at 651, dodge-cancelled before its blade was live, land nothing.
    assert_eq!(counts.hits_landed, HITS_LANDED, "{counts:?}");
    assert_eq!(counts.damage_dealt, DAMAGE_DEALT, "{counts:?}");
    assert_eq!(counts.presses, 9);
    assert!((counts.presses_a_second - 9.0 / (1000.0 / 60.0)).abs() < 1e-9);
    // Each round of 200 ticks walks for 46, then rests for 154 but for its presses: 141, 143, 98, 141 and 151
    // ticks of rest a second or longer, the third round's 47 between two presses too short to count.
    assert!((counts.idle_seconds - 674.0 / 60.0).abs() < 1e-9, "{}", counts.idle_seconds);
    assert_eq!(run.whole, counts, "one part is the whole run");

    let frames = part.frames.as_ref().expect("rows in the run");
    // 1,000 frames, every tenth twice as long: 18.3 s of play, a row a second.
    assert_eq!(frames.seconds, 18);
    assert!(frames.frames.abs_diff(frames.late * 10) < 10, "one late in ten: {frames:?}");
    assert!((frames.worst_ms - 33.33).abs() < 0.01, "{frames:?}");
    assert_eq!((frames.hottest.as_str(), frames.least_headroom), ("Severe", Some(0.25)));

    assert_eq!(report.marks.len(), 1);
    let mark = &report.marks[0];
    assert_eq!((mark.run, mark.tick, mark.label.as_deref()), (1, 256, Some("that hurt")));
    assert_eq!(mark.file.as_deref(), Some("runs/1/duel-1.sreplay"));
    let command = mark.command.as_deref().unwrap();
    assert!(command.contains("--at-tick 256") && command.contains("runs/1/duel-1.sreplay"), "{command}");

    // What's written reads back, and the text says the same.
    let text = report.to_ron().unwrap();
    assert_eq!(Report::from_ron(&text).unwrap(), report);
    let text = report.to_text();
    assert!(text.contains("5 dodges, 3 perfect, 2 counters"), "{text}");
    assert!(text.contains(&format!("dealt {DAMAGE_DEALT} in {HITS_LANDED} hits")), "{text}");
    assert!(text.contains("--at-tick 256"), "{text}");
}

/// A session whose run doesn't reach the checksum the device kept says so, and still counts what it played; one
/// from another architecture isn't held to it.
#[test]
fn a_run_that_differs_from_the_device_says_so() {
    let scratch = Scratch::new("differs");
    let dir = session_of(&scratch, "dodge-counter", &[]);
    let path = dir.join(DOCUMENT_FILE);
    let text = fs::read_to_string(&path).unwrap();
    let report = Report::read(&dir, &ALL).unwrap();
    let kept = &report.runs[0];
    let checksum = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .find_map(|line| {
            line.trim().strip_prefix("checksum: \"").map(|rest| rest.trim_end_matches("\",").to_owned())
        })
        .expect("the run's checksum");
    fs::write(&path, text.replace(&checksum, "0123456789abcdef")).unwrap();
    let differs = Report::read(&dir, &ALL).unwrap();
    assert!(
        matches!(&differs.runs[0].check, Check::Differs { reached, ticks: 1000 } if *reached == checksum),
        "{:?}",
        differs.runs[0].check
    );
    assert_eq!(differs.runs[0].whole, kept.whole);

    let other = if ARCHITECTURE == "x86_64" { "aarch64" } else { "x86_64" };
    fs::write(
        &path,
        text.replace(&format!("architecture: \"{ARCHITECTURE}\""), &format!("architecture: \"{other}\"")),
    )
    .unwrap();
    assert_eq!(Report::read(&dir, &ALL).unwrap().runs[0].check, Check::OtherArchitecture);

    fs::remove_file(dir.join(kept.file.replace(".sreplay", ".journal"))).unwrap();
    assert!(matches!(Report::read(&dir, &ALL).unwrap().runs[0].check, Check::Unread { .. }));
}

/// A report written before M4 F3 (version 1) reads, counting no hits landed: it didn't count them.
#[test]
fn a_version_1_report_upgrades() {
    let scratch = Scratch::new("upgrade");
    let dir = session_of(&scratch, "dodge-counter", &[256]);
    let report = Report::read(&dir, &ALL).unwrap();
    let text = report.to_ron().unwrap();
    assert!(text.contains("version: 2,"), "{text}");
    let old: String = text
        .replace("version: 2,", "version: 1,")
        .lines()
        .filter(|line| !line.contains("damage_dealt:") && !line.contains("hits_landed:"))
        .map(|line| format!("{line}\n"))
        .collect();
    let upgraded = Report::from_ron(&old).unwrap();
    assert_eq!(upgraded.runs[0].whole.hits_landed, 0);
    assert_eq!(upgraded.runs[0].whole.damage_dealt, 0);
    let mut expected = report.clone();
    for run in &mut expected.runs {
        for counts in std::iter::once(&mut run.whole).chain(run.parts.iter_mut().map(|part| &mut part.counts))
        {
            counts.hits_landed = 0;
            counts.damage_dealt = 0;
        }
    }
    assert_eq!(upgraded, expected);
}

/// `ward-market` runs through the Market Court's gate, whose trigger starts its encounter, and fights there: the
/// run before the gate is outside any encounter, the fight is the market's, by its document.
#[test]
fn a_zones_run_is_cut_by_its_encounters() {
    let scratch = Scratch::new("market");
    let dir = session_of(&scratch, "ward-market", &[]);
    let report = Report::read(&dir, &ALL).unwrap();
    let run = &report.runs[0];
    assert_eq!(run.check, Check::Same);
    let parts = &run.parts;
    assert!(parts.len() >= 2, "{parts:?}");
    assert_eq!(parts[0].from, 1);
    assert!(parts[0].encounter.is_none(), "the run to the gate");
    let market = parts[1].encounter.as_ref().expect("the market's fight");
    assert_eq!(market.document.as_deref(), Some("content/zones/lantern_ward/market.encounter.ron"));
    assert_eq!(parts[1].from, parts[0].to + 1);
    assert!(parts.windows(2).all(|pair| pair[1].from == pair[0].to + 1), "every tick in one part");
    assert_eq!(parts.last().unwrap().to, u64::from(run.ticks));
    assert!(matches!(market.end, MetEnd::Cleared | MetEnd::RunEnded));
    assert!(parts[1].counts.presses > parts[0].counts.presses, "the fight is where it presses");
}
