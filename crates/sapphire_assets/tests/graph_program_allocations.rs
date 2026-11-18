//! What a runtime reads of a compiled graph every frame allocates nothing: a layer's ops, a machine's
//! active state's ops and transitions, each condition over the value registers, a move's row by name
//! and its side's variant. The compiler itself runs at cook and may allocate. Installs
//! `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;

use sapphire_assets::AssetId;
use sapphire_assets::graph::v2::program::PoseOp;
use sapphire_assets::graph::v2::{AnimGraph, Side};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const GRAPH: &str = r#"(
    type: "sapphire::assets::AnimGraph",
    version: 2,
    data: (
        parameters: [
            (name: "speed", from: Locomotion(SmoothedSpeed)),
            (name: "grounded", from: Locomotion(Grounded)),
            (name: "mode", from: Traversal(Mode)),
        ],
        clips: [
            (name: "idle", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Idle_A"),
            (name: "run", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Running_A"),
            (name: "fall", source: "9a29e8c0-f131-4ea4-98b9-04879740413d", clip: "Jump_Idle"),
            (name: "jab", source: "15af86bb-67b3-4208-bc54-1618aabf450b"),
            (name: "jab_behind", source: "0c4be2d4-4149-4852-80c5-109683aad3e1"),
        ],
        machines: [
            (name: "locomotion", start: "stand", states: [
                (name: "stand", plays: Clip(clip: "idle", looping: true)),
                (name: "run", plays: Blend1D((parameter: "speed", samples: [(clip: "idle", at: 0.0), (clip: "run", at: 5.0)]))),
                (name: "air", plays: Machine("air")),
            ], transitions: [
                (from: "stand", to: "run", when: "speed >= 0.5 && grounded", decay: 6),
                (from: "run", to: "stand", when: "speed < 0.25", decay: 6),
                (from: "*", to: "air", when: "!grounded || mode == Gliding", decay: 8),
                (from: "air", to: "stand", when: "grounded && !(mode == Gliding)", decay: 4),
            ]),
            (name: "air", start: "fall", states: [(name: "fall", plays: Clip(clip: "fall", looping: true))]),
        ],
        layers: [
            (name: "locomotion", plays: Machine((machine: "locomotion"))),
            (name: "combat", plays: Combat((decay: 5, exit: 12, moves: [
                (move: "jab", clip: "jab", variants: [(from: Behind, clip: "jab_behind")]),
                (move: "kick", clip: "jab"),
            ]))),
        ],
    ),
)"#;

#[test]
fn reading_a_program_each_frame_allocates_nothing() {
    let mut graph = AnimGraph::from_document(GRAPH).expect("a graph");
    graph.resolve(|clip| AssetId::from_bytes([clip.name.len() as u8; 16]));
    let program = graph.compile().unwrap_or_else(|error| panic!("{error}"));
    let moves = ["jab", "kick", "punch"];
    let sides = [None, Some(Side::Behind), Some(Side::Left)];

    let (walked, counts) = alloc::count_in_scope(|| {
        // A machine's active state, per machine, as a runtime keeps them.
        let mut active = [program.machines[0].start, program.machines[1].start];
        let mut walked = 0u32;
        for frame in 0..2000u32 {
            let values = [
                (frame % 40) as f32 * 0.2,
                f32::from(u8::from(frame % 300 < 250)),
                if frame % 500 > 470 { 2.0 } else { 0.0 },
            ];
            for (machine, state) in active.iter_mut().enumerate() {
                let finished = frame % 7 == 0;
                if let Some(next) = program.transitions(machine as u16).iter().find(|transition| {
                    transition.from.is_none_or(|from| from == *state)
                        && transition.to != *state
                        && program.holds(transition, &values, finished)
                }) {
                    *state = next.to;
                    walked += 1;
                }
            }
            for op in program.layer_ops() {
                if let PoseOp::Machine { machine, .. } = *op {
                    for inner in program.state_ops(active[usize::from(machine)]) {
                        if let PoseOp::Machine { machine, .. } = *inner {
                            walked += program.state_ops(active[usize::from(machine)]).len() as u32;
                        }
                    }
                }
            }
            let name = moves[frame as usize % moves.len()];
            if let Some(row) = program.move_row(name) {
                walked += u32::from(row.clip_from(sides[frame as usize % sides.len()]));
            }
        }
        walked
    });
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
    assert!(walked > 1000, "{walked}");
}
