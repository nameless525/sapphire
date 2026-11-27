//! Version 1 documents to version 2, mechanically, saying in the document what v1's runtime did by
//! name, by asset ID or by a condition's variant (a transition's [`TransitionRules`]). What it did
//! for every graph alike, such as the smoothed velocity's resets, is the locomotion parameters' own
//! rule, listed in spec §5.13's "As built (M6 A5a)", which the runtime keeps (M6 A5b).
//!
//! - The locomotion layer becomes a machine named [`LOCOMOTION`], played by the first layer, and its
//!   blend spaces 2D blends over the smoothed velocity (`velocity_x`, `velocity_z`).
//! - Each condition becomes an expression over the parameters it reads, declared in
//!   [`LocomotionValue`]'s order: `Airborne` is `!grounded`, `Moving(s)` `smoothed_speed >= s`,
//!   `ForwardBelow(s)` `velocity_z < s`, `FinishedAtPhase(p)` `finished_at(p)`. v1's runtime refused a
//!   `Landed` while airborne in a graph with a landing (a touchdown state or a `LandedMoving`
//!   condition); that's `landed && grounded` there now. Only such a graph's `Landed` started a landing
//!   (its `lands` rule): elsewhere it was a plain change of state.
//! - The combat table, the additive layers and the reactions become layers in v1's order, under the
//!   names `combat`, each additive layer's own, and `reactions`. The converter's own layers
//!   (`locomotion`, `combat`, `reactions`) take a `_2`, `_3`, … where an additive layer has the name.
//! - A `Finished` or `FinishedAtPhase` out of a looping clip or a blend never held in v1, which
//!   accepted it; it converts to `false`.
//! - v1 played the heel-and-ball footwork over every blend whenever the graph named `brake_left`,
//!   `brake_right`, `launch_left`, `launch_right`, `walk`, `jog` and `run`, with each gait's contact share
//!   fixed in its runtime; a converted blend says so ([`Footwork`]). It carried the stride into the first
//!   takeoff whose clips were the heroine's air and running-air poses, by their IDs; that takeoff says
//!   `carry: true`.

use super::{
    AdditiveLayer, AnimGraph, Blend2D, CombatLayer, Entry, Footwork, FootworkGait, Layer, LayerBlend,
    LayerMotion, LocomotionValue, Machine, MachineLayer, Motion, MoveRow, Parameter, ParameterSource,
    Reaction, State, Takeoff, Transition, TransitionRules,
};
use crate::AssetId;
use crate::graph::{self, Condition, StateMotion, clip_id};

/// The machine a version 1 locomotion layer becomes.
pub const LOCOMOTION: &str = "locomotion";

/// The parameter each locomotion value converts to.
fn parameter_name(value: LocomotionValue) -> &'static str {
    match value {
        LocomotionValue::Speed => "speed",
        LocomotionValue::Direction => "direction",
        LocomotionValue::Forward => "forward",
        LocomotionValue::SmoothedSpeed => "smoothed_speed",
        LocomotionValue::SmoothedX => "velocity_x",
        LocomotionValue::SmoothedZ => "velocity_z",
        LocomotionValue::Grounded => "grounded",
        LocomotionValue::Landed => "landed",
        LocomotionValue::PreparingJump => "preparing_jump",
    }
}

const VALUES: [LocomotionValue; 9] = [
    LocomotionValue::Speed,
    LocomotionValue::Direction,
    LocomotionValue::Forward,
    LocomotionValue::SmoothedSpeed,
    LocomotionValue::SmoothedX,
    LocomotionValue::SmoothedZ,
    LocomotionValue::Grounded,
    LocomotionValue::Landed,
    LocomotionValue::PreparingJump,
];

/// The heroine's airborne poses, whose takeoff v1 carried the stride into.
const CARRY_STANDING: &str = "32e5765d-fe14-552e-9361-dcb6ee8d5c36";
const CARRY_RUNNING: &str = "e70e0e76-9728-59a9-bd18-9911fd2fb07c";

/// The clips v1's footwork bound by name, and each forward gait's share of its cycle with a ball down,
/// as its runtime fixed them (the authored ticks in `tools/motion/locomotion.py`).
const FOOTWORK_BRAKE: [&str; 2] = ["brake_left", "brake_right"];
const FOOTWORK_LAUNCH: [&str; 2] = ["launch_left", "launch_right"];
const FOOTWORK_GAITS: [(&str, f32, f32); 3] = [("walk", 24.0, 42.0), ("jog", 12.0, 34.0), ("run", 7.0, 26.0)];

/// `old` as version 2. Total, but meant for a document version 1's check passes: that's what
/// [`AnimGraph::from_document`] converts, and a document v1 refuses never reaches here through it.
#[must_use]
pub fn convert(old: &graph::AnimGraph) -> AnimGraph {
    let locomotion = &old.locomotion;
    // The converter's own layers keep clear of every additive layer's name.
    let taken: Vec<&str> = old.additive.iter().map(|layer| layer.name.as_str()).collect();
    let own = |base: &str| {
        if taken.contains(&base) {
            (2_u32..)
                .map(|number| format!("{base}_{number}"))
                .find(|name| !taken.contains(&name.as_str()))
                .unwrap_or_default()
        } else {
            base.to_owned()
        }
    };
    let touchdown = |from: &str| {
        locomotion
            .states
            .iter()
            .any(|state| state.name == from && matches!(state.plays, StateMotion::LandingBlend(_)))
    };
    let never_finishes = |from: &str| {
        locomotion.states.iter().any(|state| {
            state.name == from
                && matches!(state.plays, StateMotion::Clip { looping: true, .. } | StateMotion::Blend(_))
        })
    };
    let landing_aware =
        locomotion.transitions.iter().any(|change| matches!(change.when, Condition::LandedMoving(_)))
            || locomotion.states.iter().any(|state| matches!(state.plays, StateMotion::LandingBlend(_)));
    let mut reads = [false; VALUES.len()];
    let mut read = |value: LocomotionValue| {
        reads[VALUES.iter().position(|&each| each == value).unwrap_or(0)] = true;
        parameter_name(value)
    };
    let transitions: Vec<Transition> = locomotion
        .transitions
        .iter()
        .map(|change| Transition {
            rules: if never_finishes(&change.from) {
                rules(change.when, false, landing_aware).filter_finished()
            } else {
                rules(change.when, touchdown(&change.from), landing_aware)
            },
            from: change.from.clone(),
            to: change.to.clone(),
            when: if matches!(change.when, Condition::Finished | Condition::FinishedAtPhase(_))
                && never_finishes(&change.from)
            {
                "false".to_owned()
            } else {
                condition(change.when, landing_aware, &mut read)
            },
            decay: change.decay,
        })
        .collect();
    let named = |name: &str| old.clips.iter().any(|clip| clip.name == name);
    let footwork = (FOOTWORK_BRAKE.iter().chain(&FOOTWORK_LAUNCH).all(|name| named(name))
        && FOOTWORK_GAITS.iter().all(|(name, ..)| named(name)))
    .then(|| Footwork {
        brake: FOOTWORK_BRAKE.map(str::to_owned),
        launch: FOOTWORK_LAUNCH.map(str::to_owned),
        gaits: FOOTWORK_GAITS
            .iter()
            .map(|&(clip, down, cycle)| FootworkGait { clip: clip.to_owned(), contact: down / cycle })
            .collect(),
    });
    let id_of = |name: &str| old.clips.iter().find(|clip| clip.name == name).map(clip_id);
    let mut carried = false;
    let states = locomotion
        .states
        .iter()
        .map(|state| {
            let plays = match &state.plays {
                StateMotion::Clip { clip, looping } => {
                    Motion::Clip { clip: clip.clone(), looping: *looping, sync: None }
                }
                StateMotion::Blend(space) => Motion::Blend2D(Blend2D {
                    x: read(LocomotionValue::SmoothedX).to_owned(),
                    y: read(LocomotionValue::SmoothedZ).to_owned(),
                    samples: space.samples.clone(),
                    rate: space.rate,
                    sync: None,
                    footwork: footwork.clone(),
                }),
                StateMotion::TakeoffBlend(entry) | StateMotion::TakeoffAnticipating(entry) => {
                    // v1 carried only into its first takeoff, and only from these two poses.
                    let carry = !carried
                        && id_of(&entry.standing) == Some(AssetId::from_text(CARRY_STANDING))
                        && entry.moving.first().and_then(|sample| id_of(&sample.clip))
                            == Some(AssetId::from_text(CARRY_RUNNING));
                    carried = true;
                    Motion::Takeoff(Takeoff {
                        standing: entry.standing.clone(),
                        moving: entry.moving.clone(),
                        anticipate: matches!(state.plays, StateMotion::TakeoffAnticipating(_)),
                        carry,
                    })
                }
                StateMotion::LandingBlend(entry) => {
                    Motion::Landing(Entry { standing: entry.standing.clone(), moving: entry.moving.clone() })
                }
            };
            State { name: state.name.clone(), plays }
        })
        .collect();
    let parameters = VALUES
        .iter()
        .zip(reads)
        .filter(|&(_, read)| read)
        .map(|(&value, _)| Parameter {
            name: parameter_name(value).to_owned(),
            from: ParameterSource::Locomotion(value),
        })
        .collect();
    let mut layers = vec![
        Layer {
            name: own(LOCOMOTION),
            plays: LayerMotion::Machine(MachineLayer {
                machine: LOCOMOTION.to_owned(),
                mask: None,
                blend: LayerBlend::Override,
                weight: 1.0,
            }),
        },
        Layer {
            name: own("combat"),
            plays: LayerMotion::Combat(CombatLayer {
                decay: old.combat.decay,
                exit: old.combat.exit,
                moves: old
                    .combat
                    .moves
                    .iter()
                    .map(|row| MoveRow {
                        name: row.name.clone(),
                        clip: row.clip.clone(),
                        mask: row.mask.clone(),
                        decay: row.decay,
                        variants: Vec::new(),
                    })
                    .collect(),
            }),
        },
    ];
    layers.extend(old.additive.iter().map(|layer| Layer {
        name: layer.name.clone(),
        plays: LayerMotion::Additive(AdditiveLayer {
            clip: layer.clip.clone(),
            mask: layer.mask.clone(),
            weight: layer.weight,
        }),
    }));
    if !old.reactions.is_empty() {
        layers.push(Layer {
            name: own("reactions"),
            plays: LayerMotion::Reactions(
                old.reactions
                    .iter()
                    .map(|layer| Reaction {
                        name: layer.name.clone(),
                        clip: layer.clip.clone(),
                        mask: layer.mask.clone(),
                        weight: layer.weight,
                        fade_out: layer.fade_out,
                        restart: layer.restart,
                    })
                    .collect(),
            ),
        });
    }
    AnimGraph {
        parameters,
        smoothing: locomotion.smoothing,
        clips: old.clips.clone(),
        masks: old.masks.clone(),
        sync: Vec::new(),
        machines: vec![Machine {
            name: LOCOMOTION.to_owned(),
            start: locomotion.start.clone(),
            states,
            transitions,
        }],
        layers,
        events: Vec::new(),
        blink: old.blink.clone(),
    }
}

/// The rules v1's runtime keyed on a condition's variant: `touchdown` when it leaves a landing, and
/// `landing_aware` in a graph with a landing (a touchdown state or a `LandedMoving` condition), the only
/// kind whose `Landed` started one. v1's own binding takes its rules from here too, so the two runtimes
/// key on the same thing.
#[must_use]
pub fn rules(when: Condition, touchdown: bool, landing_aware: bool) -> TransitionRules {
    TransitionRules {
        waits_for_landing: matches!(
            when,
            Condition::Moving(_) | Condition::Stopped(_) | Condition::ForwardBelow(_)
        ),
        lands: landing_aware && matches!(when, Condition::Landed | Condition::LandedMoving(_)),
        rebound: matches!(when, Condition::JumpPreparation(true)),
        hands_off: matches!(when, Condition::AirborneMoving(_)),
        at_drawn: matches!(when, Condition::FinishedAtPhase(_))
            || (touchdown && matches!(when, Condition::Finished)),
        catch: matches!(when, Condition::FinishedAtPhase(_)),
    }
}

impl TransitionRules {
    /// Without the rules of a finish, for a finish that converts to `false`.
    #[must_use]
    pub fn filter_finished(self) -> Self {
        Self { at_drawn: false, catch: false, ..self }
    }
}

/// A v1 condition as an expression, naming each parameter through `read`.
fn condition(
    when: Condition,
    landing_aware: bool,
    read: &mut impl FnMut(LocomotionValue) -> &'static str,
) -> String {
    match when {
        Condition::Airborne => format!("!{}", read(LocomotionValue::Grounded)),
        Condition::Grounded => read(LocomotionValue::Grounded).to_owned(),
        Condition::Landed if landing_aware => {
            format!("{} && {}", read(LocomotionValue::Landed), read(LocomotionValue::Grounded))
        }
        Condition::Landed => read(LocomotionValue::Landed).to_owned(),
        Condition::Finished => "finished".to_owned(),
        Condition::Moving(speed) => format!("{} >= {}", read(LocomotionValue::SmoothedSpeed), number(speed)),
        Condition::Stopped(speed) => format!("{} < {}", read(LocomotionValue::SmoothedSpeed), number(speed)),
        Condition::LandedMoving(speed) => format!(
            "{} && {} && {} >= {}",
            read(LocomotionValue::Landed),
            read(LocomotionValue::Grounded),
            read(LocomotionValue::Forward),
            number(speed),
        ),
        Condition::FinishedAtPhase(phase) => format!("finished_at({})", number(phase)),
        Condition::ForwardBelow(speed) => format!("{} < {}", read(LocomotionValue::SmoothedZ), number(speed)),
        Condition::AirborneMoving(speed) => {
            format!(
                "!{} && {} >= {}",
                read(LocomotionValue::Grounded),
                read(LocomotionValue::Speed),
                number(speed)
            )
        }
        Condition::JumpPreparation(true) => read(LocomotionValue::PreparingJump).to_owned(),
        Condition::JumpPreparation(false) => format!("!{}", read(LocomotionValue::PreparingJump)),
    }
}

/// A number as the condition grammar reads it back to the same bits: Rust's shortest form that round
/// trips, with a point so it never reads as anything but a number.
fn number(value: f32) -> String {
    let text = format!("{value:?}");
    if text.contains(['.', 'e', 'E']) || !value.is_finite() { text } else { format!("{text}.0") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_back_to_the_same_bits() {
        for value in [0.0f32, 1.0, -0.95, 0.653_846_15, 4.78, 1e-7, 3.4e38] {
            let text = number(value);
            let read: f32 = text.parse().expect(&text);
            assert_eq!(read.to_bits(), value.to_bits(), "{text}");
        }
    }
}
