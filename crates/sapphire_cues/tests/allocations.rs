//! A hit allocates nothing on its way out, once warmed, but for what kira takes for a sound (M2 E1:
//! `sapphire_audio`'s own test holds that to its bound). One test, since the counts are of this thread and
//! a harness thread of its own keeps them honest.

mod support;

use sapphire_core::alloc::{CountingAllocator, PER_THREAD_COUNTS, count_in_scope};
use support::Fight;

#[global_allocator]
static ALLOCATOR: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

/// Runs `fight` a frame at a time to its end, and returns the allocations of what fires in its steps and
/// its frame ends. The simulation's own steps are left out: `sapphire_gameplay` holds those to zero, and a
/// replay's trail of checksums grows.
fn run(fight: &mut Fight) -> (u64, u64) {
    let (mut stepping, mut ending) = (0, 0);
    while fight.player.step() {
        let (_, counts) = count_in_scope(|| fight.fire_step());
        stepping += counts.allocations;
        let (_, counts) = count_in_scope(|| fight.end_frame());
        ending += counts.allocations;
    }
    (stepping, ending)
}

#[test]
fn a_fight_fires_with_no_allocation_once_warmed() {
    if !PER_THREAD_COUNTS {
        return;
    }
    // The first run grows what's grown once: the frame's vectors, and the mixer's, which is a mock's.
    let mut fight = Fight::new("fight");
    let _ = run(&mut fight);
    let fired = fight.cues.stats();
    fight.restart("fight");
    let (stepping, ending) = run(&mut fight);
    assert!(fight.cues.stats().effects > fired.effects, "it fired them again");
    assert_eq!(stepping, 0, "a step with what it fires, over a whole fight");
    assert_eq!(ending, 0, "a frame's end with a mock mixer, over a whole fight");
}
