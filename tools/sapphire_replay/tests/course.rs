//! The traversal course (M6 A14) walked on foot, as `tests/replays/course-walk.sreplay` plays it headless on the
//! runner's scenarios, which hand the course's builder over. No traversal mode exists to enter yet, so the walk
//! is what the course is to the controller as it stands: a jump onto the 1 m ledge, the climbing wall's face
//! met and not climbed, and a fall off the ground's edge into the pool's basin, with the mode `None` throughout.

use std::path::Path;

use sapphire_gameplay::TraversalMode;
use sapphire_gameplay::replay::{Player, Replay};
use sapphire_synthetic_zone::course;

#[test]
fn the_walk_jumps_a_ledge_meets_the_wall_and_falls_into_the_pool() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/course-walk.sreplay");
    let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("it decodes");
    let mut player = Player::new(replay, &sapphire_replay::SCENARIOS).expect("it starts");
    let (mut on_the_ledge, mut at_the_wall, mut lowest) = (false, false, f32::MAX);
    let ([wall_x0, wall_x1], _) = course::CLIMB;
    while player.step() {
        let sim = player.simulation();
        let walker = sim.actors().next().expect("the player");
        let at = walker.position;
        assert_eq!(walker.controller.state.traversal, TraversalMode::None, "nothing enters a mode yet");
        on_the_ledge |= (at.y - 1.0).abs() < 0.05 && at.z > 23.0 && at.z < 25.0;
        at_the_wall |= at.z > 0.0 && at.z < 0.5 && at.x > wall_x0 && at.x < wall_x1;
        lowest = lowest.min(at.y);
    }
    let at = player.simulation().actors().next().expect("the player").position;
    assert!(on_the_ledge, "it stood on the 1 m ledge");
    assert!(at_the_wall, "it reached the climbing wall's face");
    assert!((lowest - course::POOL_FLOOR).abs() < 0.05, "it reached the pool's floor: {lowest}");
    let ([x0, x1], [z0, z1]) = course::POOL;
    assert!(at.x > x0 && at.x < x1 && at.z > z0 && at.z < z1, "it ends in the pool's basin: {at}");
}
