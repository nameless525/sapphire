//! Graph v2's runtime allocates nothing once a player is made: the heroine's converted graph through a
//! scripted session (locomotion with starts, stops and turns, jumps and every landing, a combo with
//! hitstop and cancels, reactions, a cut, a defeat and a replacement), on presentation's maths and the
//! fixed step's. Installs `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;
use std::sync::Arc;

use sapphire_assets::graph::v2::convert::convert;
use sapphire_assets::graph::v2::program::Target;
use sapphire_body_pose as body;
use sapphire_body_pose::GraphPlayer;
use sapphire_body_pose::graph::{FixedGraphHistory, GraphStepStamp};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_time::{SUBTICKS_PER_TICK, SimTime};

#[path = "support/humanoid.rs"]
mod humanoid;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

#[test]
fn the_heroines_converted_graph_plays_a_session_with_no_allocation() {
    let (_, text) = humanoid::shipped()
        .into_iter()
        .find(|(name, _)| name.ends_with("character.animgraph.ron"))
        .expect("the heroine's graph ships");
    let graph = humanoid::version_1(&text);
    let fixture = humanoid::Humanoid::new(graph.clips.iter().map(|clip| clip.id));
    let converted = convert(&graph);
    let presentation = fixture.bind(&converted.compile().expect("it compiles"));
    let simulation =
        fixture.bind(&converted.compile_for(Target::Simulation).expect("it compiles for the fixed step"));
    let steps = humanoid::session(&graph);
    let source = fixture.source();
    let clips = source.as_slice();
    let mut player = GraphPlayer::new(Arc::clone(&presentation), fixture.pose(), 11);
    let mut history = FixedGraphHistory::try_new(Arc::clone(&simulation), fixture.pose(), 11, clips)
        .expect("it runs fixed");

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
    assert!(player.stats().transitions >= 20, "{:?}", player.stats());
}
