//! Graph v2's runtime allocates nothing once a player is made: the heroine's graph through a
//! scripted session (locomotion with starts, stops and turns, jumps and every landing, a combo with
//! hitstop and cancels, reactions, a cut, a defeat and a replacement), on presentation's maths and the
//! fixed step's, with a debug snapshot taken every frame and `speed` overridden and let go along the way
//! (M6 A9). Installs `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;
use std::sync::Arc;

use sapphire_assets::graph::v2::program::Target;
use sapphire_body_pose as body;
use sapphire_body_pose::graph::{FixedGraphHistory, GraphStepStamp};
use sapphire_body_pose::{GraphPlayer, GraphSnapshot};
use sapphire_core::Name;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_time::{SUBTICKS_PER_TICK, SimTime};

#[path = "support/humanoid.rs"]
mod humanoid;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

#[test]
fn the_heroines_graph_plays_a_session_with_no_allocation() {
    let (_, text) =
        humanoid::frozen().into_iter().find(|(name, _)| name == "heroine").expect("the heroine is frozen");
    let graph = humanoid::read(&text);
    let fixture = humanoid::Humanoid::new(graph.clips.iter().map(|clip| clip.id));
    let presentation = fixture.bind(&graph.compile().expect("it compiles"));
    let simulation =
        fixture.bind(&graph.compile_for(Target::Simulation).expect("it compiles for the fixed step"));
    let steps = humanoid::session(&graph);
    let source = fixture.source();
    let clips = source.as_slice();
    let mut player = GraphPlayer::new(Arc::clone(&presentation), fixture.pose(), 11);
    let mut history = FixedGraphHistory::try_new(Arc::clone(&simulation), fixture.pose(), 11, clips)
        .expect("it runs fixed");

    let mut snapshot = GraphSnapshot::new();
    let speed = Name::new("speed");
    let (evaluated, counts) = alloc::count_in_scope(|| {
        let mut evaluated = 0;
        let mut generation = 0;
        for (frame, step) in steps.iter().enumerate() {
            if step.replace {
                player.replace(Arc::clone(&presentation));
            }
            if let Some(released) = step.gather {
                player.request_grounded_gather(released);
            }
            evaluated +=
                u32::from(player.evaluate_with_support(&step.input, clips, 0, step.turn, step.ground));
            player.finish();
            player.debug_snapshot(&mut snapshot);
            if frame == steps.len() / 3 {
                player.set_override(speed, 3.0).expect("the controller's input, which every graph is fed");
            } else if frame == steps.len() / 2 {
                player.clear_overrides();
            }
            if step.input.cut {
                generation += 1;
            }
            let stamp = GraphStepStamp {
                step: frame as u64,
                owner_now: SimTime::from_subticks((step.input.ticks * f64::from(SUBTICKS_PER_TICK)) as u64),
                generation,
            };
            history.advance(stamp, step.input, step.turn).expect("a step forward");
        }
        evaluated
    });
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
    assert!(evaluated as usize > steps.len() / 2, "{evaluated} of {}", steps.len());
    assert!(
        !snapshot.layers().is_empty() && snapshot.parameter(Name::new("speed")).is_some(),
        "{snapshot:?}"
    );
    assert!(player.stats().transitions >= 20, "{:?}", player.stats());
}
