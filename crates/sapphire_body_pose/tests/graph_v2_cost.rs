//! Graph v2's runtime against v1's in time: each shipped graph (the heroine and the district fight's
//! cast among them), converted and compiled, through the scripted session, a frame's evaluation timed
//! on each, rounds alternating, the best round of each kept. A5's kill line is 1.25 times v1.
//!
//! Timed, so ignored: the orchestrator runs it in a quiet window, in release
//! (`cargo test --release -p sapphire_body_pose --test graph_v2_cost -- --ignored --nocapture`).

#![allow(clippy::print_stdout, reason = "this test's output is what it's for")]

use std::sync::Arc;

use sapphire_assets::graph::v2::convert::convert;
use sapphire_body_pose as body;
use sapphire_body_pose::{GraphBinding, GraphPlayer};
use sapphire_time::Stopwatch;

#[path = "support/humanoid.rs"]
mod humanoid;

const ROUNDS: usize = 7;
const SESSIONS: usize = 20;

/// The fastest of the rounds' microseconds a frame.
fn time(fixture: &humanoid::Humanoid, graph: &Arc<GraphBinding>, steps: &[humanoid::Step]) -> f64 {
    let source = fixture.source();
    let clips = source.as_slice();
    let mut best = f64::INFINITY;
    for _ in 0..ROUNDS {
        let mut player = GraphPlayer::new(Arc::clone(graph), fixture.pose(), 11);
        let watch = Stopwatch::start();
        for session in 0..SESSIONS {
            // Each session later on the clock than the last, so none replays a frozen frame.
            let offset = (session * 100_000) as f64;
            for step in steps {
                let mut input = step.input;
                input.ticks += offset;
                if let Some(released) = step.gather {
                    player.request_grounded_gather(released);
                }
                std::hint::black_box(player.evaluate_with_support(&input, clips, 0, step.turn, step.ground));
            }
        }
        let micros = watch.elapsed().as_secs_f64() * 1e6 / (SESSIONS * steps.len()) as f64;
        best = best.min(micros);
    }
    best
}

#[test]
#[ignore = "timed: run in release in a quiet window"]
fn each_shipped_graph_converted_evaluates_within_a5s_kill_line_of_v1() {
    let mut worst: f64 = 0.0;
    for (name, text) in humanoid::shipped() {
        let graph = humanoid::version_1(&text);
        let fixture = humanoid::Humanoid::new(graph.clips.iter().map(|clip| clip.id));
        let old = fixture.bind_v1(&graph);
        let new = fixture.bind(&convert(&graph).compile().expect("it compiles"));
        let steps = humanoid::session(&sapphire_assets::graph::v2::convert::convert(&graph));
        // Alternate, so a warming or a throttling machine weighs on both alike.
        let (mut v1, mut v2) = (f64::INFINITY, f64::INFINITY);
        for _ in 0..2 {
            v1 = v1.min(time(&fixture, &old, &steps));
            v2 = v2.min(time(&fixture, &new, &steps));
        }
        let ratio = v2 / v1;
        worst = worst.max(ratio);
        println!(
            "{name}: v1 {v1:.3} µs a frame, v2 {v2:.3} µs a frame, {ratio:.3}× ({} frames)",
            steps.len()
        );
    }
    println!("worst {worst:.3}× against a kill line of 1.25×");
    assert!(worst <= 1.25, "a converted graph costs {worst:.3}× v1");
}
