//! Where a recording's actions come from: [`ActionSource`].
//!
//! The app's real source is its input bindings. Most of the suite comes from scripts instead, which need no
//! one to play them: documents in `content/inputs/` ([`super::inputs`]), each a [`Script`]. **To add one,**
//! write `content/inputs/<name>.inputs.ron`, then record it with `cargo xtask replay --record`. No Rust
//! changes (M4 R2).

use super::inputs::Script;
use crate::actions::TickActions;
use crate::actors::ActorId;

/// Gives a recording each tick's actions: the seam between the simulation and what drives it.
///
/// `tick` counts from 0 and is the step about to run. `cast` is what the scenario's setup returned,
/// the player first. Whatever it returns is what's recorded and stepped, so a source needs no
/// determinism of its own: a live player's bindings qualify, and any closure does too.
pub trait ActionSource {
    fn actions(&mut self, tick: u64, cast: &[ActorId]) -> TickActions;
}

impl<F: FnMut(u64, &[ActorId]) -> TickActions> ActionSource for F {
    fn actions(&mut self, tick: u64, cast: &[ActorId]) -> TickActions {
        self(tick, cast)
    }
}

/// A script's document, a tick at a time.
impl ActionSource for &Script {
    fn actions(&mut self, tick: u64, cast: &[ActorId]) -> TickActions {
        Script::actions(self, tick, cast)
    }
}

#[cfg(test)]
mod tests {
    use super::super::find_script;
    use super::*;

    #[test]
    fn closures_and_scripts_are_sources() {
        fn take(source: &mut dyn ActionSource) -> TickActions {
            source.actions(3, &[])
        }
        let mut calls = 0;
        let mut source = |tick: u64, _cast: &[ActorId]| {
            calls += 1;
            assert_eq!(tick, 3);
            TickActions::NEUTRAL
        };
        assert_eq!(take(&mut source), TickActions::NEUTRAL);
        assert_eq!(calls, 1);
        assert_eq!(take(&mut find_script("idle").expect("listed")), TickActions::NEUTRAL);
    }
}
