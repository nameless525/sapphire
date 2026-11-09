//! The flow after every step allocates nothing: 10,000 quiet steps of observing and updating, with beats
//! checked against a run that has a checkpoint and flags. Installs `CountingAllocator` as this test binary's
//! global allocator.

use std::alloc::System;

use game_flow::document::FlowDocument;
use game_flow::flow::{Flow, FlowCommand, FlowState, FlowView};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_serialize::AssetId;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const TEST_FLOW: &str = include_str!("../test_content/test.flow.ron");

/// A run where nothing a beat waits for happens, answered from fixed facts.
struct Quiet {
    checkpoint: &'static str,
}

impl FlowView for Quiet {
    fn encounter_started(&self, _: AssetId) -> bool {
        false
    }
    fn encounter_cleared(&self, _: AssetId) -> bool {
        false
    }
    fn timeline_ended(&self, _: AssetId) -> bool {
        false
    }
    fn checkpoint_reached(&self, name: &str) -> bool {
        name == self.checkpoint
    }
    fn latest_checkpoint(&self) -> Option<&str> {
        Some(self.checkpoint)
    }
    fn flag(&self, name: &str) -> i32 {
        i32::from(name == "gate_answered")
    }
    fn player_fallen(&self) -> bool {
        false
    }
}

#[test]
fn quiet_steps_allocate_nothing() {
    if !PER_THREAD_COUNTS {
        // Process-wide totals can't tell this thread's allocations from the test harness's.
        return;
    }
    let mut flow = Flow::new(FlowDocument::from_ron(TEST_FLOW).expect("reads"), true);
    flow.command(FlowCommand::Continue);
    assert_eq!(flow.state(), FlowState::Playing);
    let views = [Quiet { checkpoint: "gate" }, Quiet { checkpoint: "court" }];
    let ((), counts) = alloc::count_in_scope(|| {
        for step in 0..10_000_usize {
            // The checkpoint changes now and then, so a banner shows and runs out.
            let view = &views[(step / 500) % 2];
            assert!(flow.observe(view).is_none());
            flow.update(1.0 / 60.0);
            let _ = (flow.banner(), flow.music_phase(), flow.can_skip(), flow.wipe_ask());
        }
    });
    assert_eq!(counts.allocations, 0, "{counts:?}");
    assert_eq!(flow.state(), FlowState::Playing);
    assert_eq!(flow.stats().beats_fired, 0);
}
