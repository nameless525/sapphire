//! With no counting allocator, every count reads zero, so gate 4 would pass whatever a step does. The
//! suite refuses to run rather than pass blind. The binary installs one, which is why it passes.

use sapphire_gameplay::replay::{ARCHITECTURE, SCENARIOS};
use sapphire_replay::{Record, Suite, check, record};

#[test]
fn the_suite_refuses_to_run_blind() {
    let dir = std::env::temp_dir().join(format!("sapphire_replay_blind_{}", std::process::id()));
    let suite = Suite {
        dir: &dir,
        out: &dir,
        update: false,
        only: None,
        scenarios: SCENARIOS,
        architecture: ARCHITECTURE,
    };
    let error = check(&suite).expect_err("refused").to_string();
    assert!(error.contains("no counting allocator"), "{error}");

    let request = Record {
        dir: &dir,
        out: &dir,
        name: "duel",
        scenario: "melee",
        script: "idle",
        ticks: None,
        seed: None,
        scenarios: SCENARIOS,
        architecture: ARCHITECTURE,
    };
    let error = record(&request).expect_err("refused").to_string();
    assert!(error.contains("no counting allocator"), "{error}");
    assert!(!dir.exists(), "nothing was written");
}
