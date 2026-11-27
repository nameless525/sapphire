//! The traversal course's climbing wall climbed (M6 A16a), as `tests/replays/course-climb.sreplay` plays it
//! headless on the runner's scenarios: the player grabs the wall's face, lets go of it and grabs it again, climbs
//! west into the inside corner with the wing and onto its side, round the wing's outside corner onto its south
//! face, and up that to tops out over the wing by a mantle, where she stands.

use std::path::Path;

use sapphire_gameplay::replay::{Player, Replay};
use sapphire_gameplay::{GroundMode, TraversalMode};
use sapphire_synthetic_zone::course;

/// What a walk of the replay saw.
#[derive(Debug, Default)]
struct Walk {
    /// The faces it held, by their normals, a new one each time the normal changed.
    faces: Vec<[f32; 3]>,
    /// It stood on the ground between two climbs.
    landed_between: bool,
    /// The most its feet rose on a face.
    highest_climbing: f32,
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

fn walk(player: &mut Player) -> Walk {
    let mut seen = Walk::default();
    let mut climbed = false;
    while player.step() {
        let view = player.simulation().actors().next().expect("the player");
        let state = view.controller.state;
        assert_eq!(
            state.traversal == TraversalMode::Climbing,
            state.climb.is_some(),
            "the mode and the hold agree"
        );
        if let Some(hold) = state.climb {
            climbed = true;
            seen.highest_climbing = seen.highest_climbing.max(view.position.y);
            let normal = hold.normal.to_array();
            if seen.faces.last().is_none_or(|face| !near(*face, normal)) {
                seen.faces.push(normal);
            }
        } else if climbed && state.mode == GroundMode::Grounded && view.position.y.abs() < 0.01 {
            seen.landed_between = true;
        }
    }
    seen
}

#[test]
fn the_climb_grabs_lets_go_turns_both_corners_and_tops_out_over_the_wing() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/course-climb.sreplay");
    let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("it decodes");
    let mut player = Player::new(replay, &sapphire_replay::SCENARIOS).expect("it starts");
    let seen = walk(&mut player);
    let stats = player.simulation().stats();
    let end = player.simulation().actors().next().expect("the player");
    assert_eq!((stats.climbs_started, stats.climb_top_outs, stats.mantles_started), (2, 1, 0), "{stats:?}");
    assert!(seen.landed_between, "it let go and stood on the ground: {seen:?}");
    // The wall's face, the wing's east side round the inside corner, then the wing's south face round the outside.
    let faces = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    assert_eq!(seen.faces.len(), faces.len(), "{seen:?}");
    assert!(seen.faces.iter().zip(faces).all(|(seen, face)| near(*seen, face)), "{seen:?}");
    assert!(seen.highest_climbing > course::TOWER_HEIGHT - 2.0, "it climbed near the top: {seen:?}");
    let ([x0, x1], out) = course::WING;
    let at = end.position;
    assert!((at.y - course::TOWER_HEIGHT).abs() < 0.01, "on the top: {at}");
    assert!(at.x > x0 && at.x < x1 && at.z > 0.0 && at.z < out, "on the wing's top: {at}");
    assert_eq!(end.controller.state.mode, GroundMode::Grounded);
}
