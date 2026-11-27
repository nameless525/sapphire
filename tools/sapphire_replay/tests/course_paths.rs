//! The traversal course's older replays stand still when a station is added (M6 A16a): each new block moves
//! every `course-*` replay's checksums from tick 0, since the statics' digest is in the tables' hash, so the
//! re-stamp can't say whether their paths moved. This plays each one on the course as it is and as it stood
//! before [`course::NEWEST`]'s blocks, and holds every tick's places and controller states alike. A station
//! after A16a's names its blocks in `NEWEST` and adds the replay before it to [`OLDER`].

use std::fmt::Write as _;
use std::path::Path;

use sapphire_gameplay::replay::{Player, Replay, Scenario};
use sapphire_synthetic_zone::course;

/// The `course-*` replays recorded before the newest station.
const OLDER: &[&str] = &["course-walk", "course-mantle"];

/// Every tick's actors, their places and controller states, by their exact bits.
fn trace(name: &str, scenarios: &[Scenario]) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/replays/{name}.sreplay"));
    let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("it decodes");
    let mut player = Player::on_current_tables(replay, scenarios).expect("it starts");
    let mut text = String::new();
    let mut tick = 0_u32;
    while player.step() {
        tick += 1;
        for view in player.simulation().actors() {
            let state = view.controller.state;
            writeln!(text, "{tick} {:?} {:?} {:?}", view.position.to_array(), view.yaw, state)
                .expect("a string");
        }
    }
    assert!(tick > 0, "{name} played");
    text
}

#[test]
fn the_older_course_replays_take_the_same_path_without_the_newest_station() {
    let names: Vec<String> =
        course::colliders().colliders.into_iter().map(|collider| collider.name).collect();
    for newest in course::NEWEST {
        assert!(names.iter().any(|name| name == newest), "`{newest}` is a block of the course");
    }
    let before: Vec<Scenario> = sapphire_replay::SCENARIOS
        .iter()
        .map(|scenario| Scenario { zones: course::BEFORE_NEWEST, ..*scenario })
        .collect();
    for name in OLDER {
        let now = trace(name, &sapphire_replay::SCENARIOS);
        let then = trace(name, &before);
        let moved = now.lines().zip(then.lines()).position(|(now, then)| now != then);
        assert_eq!(moved, None, "{name} took another path with the newest station, from that line");
        assert_eq!(now.lines().count(), then.lines().count(), "{name}");
    }
}
