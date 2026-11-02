//! Determinism: the same scripted run gives the same floats, by `to_bits`. The answer to M2 D3's
//! third question.
//!
//! Spec §9's gate 2 asks for one checksum per architecture: x86-64 on Linux and Windows, arm64 on
//! macOS. So each `cargo test` job computes the run's checksum, prints it and compares it against
//! [`EXPECTED`]'s entry for its architecture.
//!
//! - A changed checksum means the crate's numbers changed. If that's intended (a `parry3d` upgrade,
//!   a change to the sweeps), update the constant, and expect replays to re-record.
//! - **Linux and Windows failing with different values is the finding this test exists for:** it
//!   means something on the x86-64 path reaches a platform's C math library. Don't pin either until
//!   that's found.

mod common;

use common::{Script, world};
use sapphire_physics::PhysicsWorld;
use xxhash_rust::xxh3::Xxh3;

/// arm64: an Apple M5, in both the test and release profiles. CI's macOS runners must agree.
const CHECKSUM_AARCH64: u64 = 0xdea2_9485_3ead_a2e1;

/// x86-64: Linux and Windows must both reach this one value. **Not pinned yet:** the first CI run
/// fails, printing the value to put here.
const CHECKSUM_X86_64: Option<u64> = None;

const EXPECTED: Option<u64> = if cfg!(target_arch = "aarch64") {
    Some(CHECKSUM_AARCH64)
} else if cfg!(target_arch = "x86_64") {
    CHECKSUM_X86_64
} else {
    None
};

/// Runs the script for `steps` over `world` and hashes every result of every query, then the
/// world's own canonical state.
fn run(mut world: PhysicsWorld, steps: u32) -> u64 {
    let mut script = Script::default();
    let mut hash = Xxh3::new();
    for tick in 0..steps {
        script.step(&mut world, tick, &mut |bytes| hash.update(bytes));
    }
    world.write_canonical(&mut |bytes| hash.update(bytes));
    hash.digest()
}

#[test]
fn the_scripted_run_matches_this_architectures_checksum() {
    let computed = run(world(), 1000);
    let arch = std::env::consts::ARCH;
    #[allow(clippy::print_stdout, reason = "CI's log is how the x86-64 value gets filled in")]
    {
        println!("sapphire_physics checksum on {arch}: {computed:#018x}");
    }
    match EXPECTED {
        Some(expected) => assert_eq!(
            computed, expected,
            "the checksum on {arch} moved: computed {computed:#018x}, pinned {expected:#018x}"
        ),
        None => panic!(
            "no checksum is pinned for {arch}: it computed {computed:#018x}. Set CHECKSUM_X86_64 to this \
             once Linux and Windows print the same value (tests/checksum.rs)"
        ),
    }
}

#[test]
fn a_run_repeats_in_one_process() {
    assert_eq!(run(world(), 300), run(world(), 300));
}

/// Results depend on what's in the world, never the order it went in.
#[test]
fn insertion_order_changes_nothing() {
    let mut statics = common::statics();
    statics.reverse();
    let mut reordered = PhysicsWorld::new(statics, 16).expect("the scene builds");
    let mut bodies = common::bodies();
    bodies.reverse();
    for body in bodies {
        reordered.add_body(body).expect("the bodies fit");
    }
    assert_eq!(run(reordered, 300), run(world(), 300));
}

#[test]
fn removing_and_readding_a_body_changes_nothing() {
    let mut churned = world();
    let body = common::bodies()[0];
    assert!(churned.remove_body(body.id));
    churned.add_body(body).expect("it fits again");
    assert_eq!(run(churned, 300), run(world(), 300));
}

#[test]
fn canonical_state_sees_each_body_and_collider() {
    let bytes = |world: &PhysicsWorld| {
        let mut out = Vec::new();
        world.write_canonical(&mut |chunk| out.extend_from_slice(chunk));
        out
    };
    let base = bytes(&world());
    assert_eq!(base, bytes(&world()));

    let mut moved = world();
    let pose = moved.body_pose(common::CHARACTERS[0]).expect("there");
    let mut nudged = pose;
    // The smallest change there is: one bit of one coordinate.
    nudged.position.x = f32::from_bits(nudged.position.x.to_bits() + 1);
    moved.set_body_pose(common::CHARACTERS[0], nudged).expect("a valid pose");
    assert_ne!(base, bytes(&moved));

    let mut fewer = common::statics();
    fewer.pop();
    let mut smaller = PhysicsWorld::new(fewer, 16).expect("the scene builds");
    for body in common::bodies() {
        smaller.add_body(body).expect("the bodies fit");
    }
    assert_ne!(base, bytes(&smaller));
    assert_ne!(world().statics_digest(), smaller.statics_digest());
}
