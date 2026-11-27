//! The traversal course's ledges mantled (M6 A15), as `tests/replays/course-mantle.sreplay` plays it headless on
//! the runner's scenarios: the player stands on each of the four ledges' tops, 0.5 to 2 m, by a mantle each, is
//! refused at the ledge with no room over it, and vaults the low wall at a run onto the ground beyond it.

use std::path::Path;

use sapphire_gameplay::replay::{Player, Replay};
use sapphire_synthetic_zone::course;

#[test]
fn the_run_mantles_each_ledge_is_refused_under_the_slab_and_vaults_the_wall() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/course-mantle.sreplay");
    let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("it decodes");
    let mut player = Player::new(replay, &sapphire_replay::SCENARIOS).expect("it starts");
    let mut topped = [false; course::LEDGES.len()];
    let (mut under_the_slab, mut beyond_the_wall) = (0.0_f32, false);
    let ([vault_x, vault_z], [half_x, half_z], _) = course::VAULT;
    // Each refused press: where it was, and how many casts it took. The probe refuses for want of room only
    // at its fourth cast, the crossing over the edge, a face, a top and room to rise found before it.
    let mut refusals = Vec::new();
    let mut last = player.simulation().stats();
    while player.step() {
        let at = player.simulation().actors().next().expect("the player").position;
        let stats = player.simulation().stats();
        if stats.ledges_refused > last.ledges_refused {
            refusals.push((at, stats.ledge_casts - last.ledge_casts));
        }
        last = stats;
        for (top, ([x, z], height)) in topped.iter_mut().zip(course::LEDGES) {
            let over = (at.x - x).abs() < course::LEDGE_HALF[0] && (at.z - z).abs() < course::LEDGE_HALF[1];
            *top |= over && (at.y - height).abs() < 0.02;
        }
        let ([x, z], _) = course::NO_ROOM;
        if (at.x - x).abs() < course::LEDGE_HALF[0] && (at.z - z).abs() < course::LEDGE_HALF[1] {
            under_the_slab = under_the_slab.max(at.y);
        }
        beyond_the_wall |=
            (at.x - vault_x).abs() < half_x && at.z < vault_z - half_z - 0.3 && at.y.abs() < 0.02;
    }
    let stats = player.simulation().stats();
    assert_eq!(topped, [true; 4], "it stood on every ledge's top: {stats:?}");
    assert!(under_the_slab < 0.01, "it never got onto the ledge with no room: {under_the_slab}");
    assert!(beyond_the_wall, "it came down past the low wall: {stats:?}");
    assert_eq!((stats.mantles_started, stats.vaults_started), (4, 1), "{stats:?}");
    let ([x, _], _) = course::NO_ROOM;
    let under: Vec<_> = refusals.iter().filter(|(at, _)| (at.x - x).abs() < course::LEDGE_HALF[0]).collect();
    assert_eq!(under.len(), 1, "one press under the slab: {refusals:?}");
    assert_eq!(under[0].1, 4, "refused at the crossing, for want of room: {refusals:?}");
}
