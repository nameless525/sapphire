//! The traversal course's glide drop glided (M6 A17), as `tests/replays/course-glide.sreplay` plays it headless on
//! the runner's scenarios: the player climbs the climbing wall and tops out onto it, runs off its north edge,
//! opens a glide in the fall, steers it west across the pool, drops it and opens another, and lands on the dry
//! ground past the pool's west side, where she stands.

use std::path::Path;

use sapphire_gameplay::replay::{Player, Replay};
use sapphire_gameplay::{GroundMode, TraversalMode};
use sapphire_synthetic_zone::course;

/// What a walk of the replay saw.
#[derive(Debug, Default)]
struct Walk {
    /// It stood on the wall's top before it glided.
    stood_on_top: bool,
    /// The ticks spent gliding.
    gliding: u32,
    /// How many separate glides it flew.
    glides: u32,
    /// The fastest and slowest it sank while gliding, in m/s.
    sink: Option<(f32, f32)>,
    /// The most it went across while gliding, in m/s.
    fastest_across: f32,
}

fn walk(player: &mut Player) -> Walk {
    let mut seen = Walk::default();
    let mut was_gliding = false;
    while player.step() {
        let view = player.simulation().actors().next().expect("the player");
        let state = view.controller.state;
        let on_top = (view.position.y - course::TOWER_HEIGHT).abs() < 0.01;
        if seen.glides == 0 && state.mode == GroundMode::Grounded && on_top {
            seen.stood_on_top = true;
        }
        let gliding = state.traversal == TraversalMode::Gliding;
        if gliding {
            seen.gliding += 1;
            seen.glides += u32::from(!was_gliding);
            let sink = -state.velocity.y;
            let (fastest, slowest) = seen.sink.unwrap_or((sink, sink));
            seen.sink = Some((fastest.max(sink), slowest.min(sink)));
            let across = (state.velocity.x * state.velocity.x + state.velocity.z * state.velocity.z).sqrt();
            seen.fastest_across = seen.fastest_across.max(across);
        }
        was_gliding = gliding;
    }
    seen
}

#[test]
fn the_glide_leaves_the_wall_top_drops_opens_again_and_lands_past_the_pool() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/course-glide.sreplay");
    let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("it decodes");
    let mut player = Player::new(replay, &sapphire_replay::SCENARIOS).expect("it starts");
    let seen = walk(&mut player);
    let stats = player.simulation().stats();
    let end = player.simulation().actors().next().expect("the player");

    assert!(seen.stood_on_top, "it stood on the wall's top first: {seen:?}");
    assert_eq!((stats.glides_started, seen.glides), (2, 2), "a glide, dropped, and another: {seen:?}");
    // Two opening rays, and nothing else gliding cast. The one climb, topped out, and no ledge from the ground.
    assert_eq!(stats.glide_casts, 2, "{stats:?}");
    let climbed = (stats.climbs_started, stats.climb_top_outs, stats.mantles_started);
    assert_eq!(climbed, (1, 1, 0), "{stats:?}");
    // The heroine's glide sinks at 2 m/s every tick it glides, and goes across at no more than 6.
    assert_eq!(seen.sink, Some((2.0, 2.0)), "{seen:?}");
    assert!(seen.fastest_across <= 6.0 + 1e-5 && seen.fastest_across > 5.9, "{seen:?}");
    assert!(seen.gliding > 120, "{seen:?}");

    let ([pool_west, _], [pool_north, pool_south]) = course::POOL;
    let at = end.position;
    assert!(
        at.x < pool_west && at.z > pool_north && at.z < pool_south,
        "on the ground west of the pool: {at}"
    );
    assert!(at.y.abs() < 0.01, "on the ground's top: {at}");
    assert_eq!(
        (end.controller.state.mode, end.controller.state.traversal),
        (GroundMode::Grounded, TraversalMode::None)
    );
}
