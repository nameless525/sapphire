//! Triggers: when a binding fires, judged from its actuation over time.
//!
//! Every judgment takes the evaluation's timestamp and compares integer nanoseconds, so trigger
//! timing is exact and reproducible with synthetic timestamps; nothing here reads a clock.

use bevy_reflect::Reflect;
use sapphire_platform::Timestamp;
use serde::{Deserialize, Serialize};

/// When a binding fires (Enhanced Input's triggers). A binding is *actuated* while its modified
/// value reaches its actuation threshold; the trigger turns that into a state over time.
///
/// | Trigger | Ongoing | Triggered |
/// |---|---|---|
/// | `Down` | — | while actuated |
/// | `Pressed` | — | once, when actuation starts |
/// | `Released` | while actuated | once, when actuation ends |
/// | `Hold` | until held `seconds` | from then until released |
/// | `Tap` | while held, up to `seconds` | once, on a release before `seconds` |
/// | `DoubleTap` | from the first tap until the window closes | once, on a press within `seconds` of a tap's release |
///
/// "Once" means a pulse: the action triggers and completes at the same instant. A tap or
/// double tap held too long cancels instead. Chords are the binding's `chord` field: an action
/// that must be triggered for the binding to fire at all.
///
/// Durations are in seconds, rounded to whole microseconds. One that rounds to less than a
/// microsecond, or comes to 2^64 nanoseconds (about 584 years) or more, makes the binding
/// invalid ([`MapError::InvalidBinding`](crate::MapError::InvalidBinding)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::input"]
pub enum Trigger {
    #[default]
    Down,
    Pressed,
    Released,
    Hold {
        seconds: f32,
    },
    /// A press released before `seconds`: a light attack, where holding means something else.
    Tap {
        seconds: f32,
    },
    /// Two taps: the first released before `seconds`, the second pressed less than `seconds`
    /// after that release.
    DoubleTap {
        seconds: f32,
    },
}

impl Trigger {
    pub(crate) fn is_down(&self) -> bool {
        *self == Self::Down
    }
}

/// A [`Trigger`] with its duration in nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TriggerSpec {
    Down,
    Pressed,
    Released,
    Hold(u64),
    Tap(u64),
    DoubleTap(u64),
}

impl TriggerSpec {
    pub fn compile(trigger: Trigger) -> Result<Self, &'static str> {
        Ok(match trigger {
            Trigger::Down => Self::Down,
            Trigger::Pressed => Self::Pressed,
            Trigger::Released => Self::Released,
            Trigger::Hold { seconds } => Self::Hold(nanos(seconds)?),
            Trigger::Tap { seconds } => Self::Tap(nanos(seconds)?),
            Trigger::DoubleTap { seconds } => Self::DoubleTap(nanos(seconds)?),
        })
    }
}

/// An authored duration in nanoseconds, rounded to whole microseconds: `0.2_f32` is
/// 0.200000003 s, and an authored 200 ms must be 200 ms exactly. A bindings document can hold
/// any finite number, so durations that round to nothing or don't fit are errors, not panics.
fn nanos(seconds: f32) -> Result<u64, &'static str> {
    if !(seconds > 0.0 && seconds.is_finite()) {
        return Err("trigger durations must be positive");
    }
    let micros = (f64::from(seconds) * 1e6).round();
    if micros < 1.0 {
        return Err("trigger durations must be at least a microsecond");
    }
    // The cast saturates, and the product overflows past 2^64 ns, about 584 years.
    (micros as u64).checked_mul(1000).ok_or("trigger durations must be under 584 years")
}

/// A binding's state after an evaluation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TriggerState {
    #[default]
    Idle,
    Ongoing,
    Triggered,
}

/// The result of one evaluation: the state from now on, and whether the binding fired an
/// instantaneous trigger at this instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fire {
    pub state: TriggerState,
    pub pulse: bool,
}

impl Fire {
    const IDLE: Self = Self { state: TriggerState::Idle, pulse: false };
    const ONGOING: Self = Self { state: TriggerState::Ongoing, pulse: false };
    const TRIGGERED: Self = Self { state: TriggerState::Triggered, pulse: false };
    const PULSE: Self = Self { state: TriggerState::Idle, pulse: true };
}

/// What a binding's trigger remembers between evaluations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Memory {
    actuated: bool,
    /// Actuated when the binding became active: edge triggers ignore it until it lets go, so a
    /// key pressed under one context doesn't fire as a fresh press in the next.
    suppressed: bool,
    /// When the current actuation started.
    pressed_at: Timestamp,
    /// The current actuation is used up: a hold that fired, a tap held too long, or a double
    /// tap's second press.
    spent: bool,
    /// Double tap: when the first tap was released, while the second may still come.
    armed_at: Option<Timestamp>,
}

impl Memory {
    /// Memory for a binding that just became active while its source reads `actuated`.
    pub fn entering(actuated: bool) -> Self {
        Self { actuated, suppressed: actuated, ..Self::default() }
    }
}

fn after(time: Timestamp, nanos: u64) -> Timestamp {
    Timestamp::from_nanos(time.as_nanos().saturating_add(nanos))
}

fn since(now: Timestamp, earlier: Timestamp) -> u64 {
    now.as_nanos().saturating_sub(earlier.as_nanos())
}

/// Judges a binding whose source reads `actuated` at time `now`. `abort` means a drop in
/// actuation is the device letting go (focus lost, a canceled touch), not the player releasing,
/// so nothing a release would fire fires.
pub(crate) fn evaluate(
    spec: TriggerSpec,
    memory: &mut Memory,
    actuated: bool,
    now: Timestamp,
    abort: bool,
) -> Fire {
    let rising = actuated && !memory.actuated;
    let falling = !actuated && memory.actuated;
    memory.actuated = actuated;
    if rising {
        memory.pressed_at = now;
        memory.spent = false;
    }
    if memory.suppressed {
        if !actuated {
            memory.suppressed = false;
        }
        // `Down` reads the level, not an edge, so a held stick keeps moving across contexts.
        if spec != TriggerSpec::Down {
            return Fire::IDLE;
        }
    }
    let released = falling && !abort;
    let held = since(now, memory.pressed_at);
    match spec {
        TriggerSpec::Down => {
            if actuated {
                Fire::TRIGGERED
            } else {
                Fire::IDLE
            }
        }
        TriggerSpec::Pressed => {
            if rising {
                Fire::PULSE
            } else {
                Fire::IDLE
            }
        }
        TriggerSpec::Released => match (actuated, released) {
            (true, _) => Fire::ONGOING,
            (false, true) => Fire::PULSE,
            (false, false) => Fire::IDLE,
        },
        TriggerSpec::Hold(duration) => {
            if !actuated {
                Fire::IDLE
            } else if held >= duration {
                memory.spent = true;
                Fire::TRIGGERED
            } else {
                Fire::ONGOING
            }
        }
        TriggerSpec::Tap(duration) => {
            if actuated {
                if memory.spent || held >= duration {
                    memory.spent = true;
                    Fire::IDLE
                } else {
                    Fire::ONGOING
                }
            } else if released && !memory.spent && held < duration {
                Fire::PULSE
            } else {
                Fire::IDLE
            }
        }
        TriggerSpec::DoubleTap(window) => {
            if memory.armed_at.is_some_and(|at| since(now, at) >= window) {
                memory.armed_at = None;
            }
            if rising && memory.armed_at.take().is_some() {
                memory.spent = true;
                return Fire::PULSE;
            }
            if actuated {
                if memory.spent || held >= window {
                    memory.spent = true;
                    Fire::IDLE
                } else {
                    Fire::ONGOING
                }
            } else {
                if released && !memory.spent && held < window {
                    memory.armed_at = Some(now);
                }
                if memory.armed_at.is_some() { Fire::ONGOING } else { Fire::IDLE }
            }
        }
    }
}

/// The next instant at which the trigger changes state with no change of input: a hold
/// completing, a tap running out, a double tap's window closing.
pub(crate) fn deadline(spec: TriggerSpec, memory: &Memory) -> Option<Timestamp> {
    if memory.suppressed {
        return None;
    }
    let pressing = memory.actuated && !memory.spent;
    match spec {
        TriggerSpec::Hold(duration) | TriggerSpec::Tap(duration) => {
            pressing.then(|| after(memory.pressed_at, duration))
        }
        TriggerSpec::DoubleTap(window) => {
            if pressing {
                Some(after(memory.pressed_at, window))
            } else {
                memory.armed_at.map(|at| after(at, window))
            }
        }
        TriggerSpec::Down | TriggerSpec::Pressed | TriggerSpec::Released => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Timestamp {
        Timestamp::from_nanos(ms * 1_000_000)
    }

    /// Feeds `(time ms, actuated)` samples and returns the results.
    fn run(trigger: Trigger, samples: &[(u64, bool)]) -> Vec<Fire> {
        let spec = TriggerSpec::compile(trigger).expect("valid trigger");
        let mut memory = Memory::default();
        samples.iter().map(|&(t, actuated)| evaluate(spec, &mut memory, actuated, ms(t), false)).collect()
    }

    const I: Fire = Fire::IDLE;
    const O: Fire = Fire::ONGOING;
    const T: Fire = Fire::TRIGGERED;
    const P: Fire = Fire::PULSE;

    #[test]
    fn down_follows_actuation() {
        assert_eq!(run(Trigger::Down, &[(0, true), (10, true), (20, false)]), [T, T, I]);
    }

    #[test]
    fn pressed_and_released_pulse_on_edges() {
        assert_eq!(run(Trigger::Pressed, &[(0, true), (10, true), (20, false), (30, true)]), [P, I, I, P]);
        assert_eq!(run(Trigger::Released, &[(0, true), (10, true), (20, false), (30, false)]), [O, O, P, I]);
    }

    #[test]
    fn hold_triggers_after_its_duration_until_released() {
        let hold = Trigger::Hold { seconds: 0.5 };
        assert_eq!(
            run(hold, &[(0, true), (499, true), (500, true), (900, true), (901, false)]),
            [O, O, T, T, I]
        );
        // Released early: back to idle without triggering.
        assert_eq!(run(hold, &[(0, true), (300, false), (900, false)]), [O, I, I]);
    }

    #[test]
    fn tap_fires_on_a_quick_release_and_cancels_when_held() {
        let tap = Trigger::Tap { seconds: 0.2 };
        assert_eq!(run(tap, &[(0, true), (199, false)]), [O, P]);
        assert_eq!(run(tap, &[(0, true), (200, true), (250, false)]), [O, I, I], "held to the limit");
    }

    #[test]
    fn double_tap_needs_two_quick_taps() {
        let double = Trigger::DoubleTap { seconds: 0.3 };
        // Tap, gap, tap: fires on the second press.
        assert_eq!(run(double, &[(0, true), (100, false), (350, true), (400, false)]), [O, O, P, I]);
        // The gap is too long: the second press starts over.
        assert_eq!(run(double, &[(0, true), (100, false), (400, true)]), [O, O, O]);
        // The first press is too long to be a tap.
        assert_eq!(run(double, &[(0, true), (300, true), (350, false), (400, true)]), [O, I, I, O]);
    }

    #[test]
    fn deadlines_mark_when_time_alone_changes_the_state() {
        let spec = TriggerSpec::compile(Trigger::Hold { seconds: 0.5 }).expect("valid");
        let mut memory = Memory::default();
        evaluate(spec, &mut memory, true, ms(100), false);
        assert_eq!(deadline(spec, &memory), Some(ms(600)));
        assert_eq!(evaluate(spec, &mut memory, true, ms(600), false), T);
        assert_eq!(deadline(spec, &memory), None, "a fired hold has nothing left to wait for");

        let spec = TriggerSpec::compile(Trigger::DoubleTap { seconds: 0.3 }).expect("valid");
        let mut memory = Memory::default();
        evaluate(spec, &mut memory, true, ms(0), false);
        assert_eq!(deadline(spec, &memory), Some(ms(300)));
        evaluate(spec, &mut memory, false, ms(100), false);
        assert_eq!(deadline(spec, &memory), Some(ms(400)), "the window for the second tap");
        assert_eq!(evaluate(spec, &mut memory, false, ms(400), false), I);
        assert_eq!(deadline(spec, &memory), None);
    }

    #[test]
    fn aborted_releases_fire_nothing() {
        for trigger in [Trigger::Released, Trigger::Tap { seconds: 1.0 }, Trigger::DoubleTap { seconds: 1.0 }]
        {
            let spec = TriggerSpec::compile(trigger).expect("valid");
            let mut memory = Memory::default();
            evaluate(spec, &mut memory, true, ms(0), false);
            assert_eq!(evaluate(spec, &mut memory, false, ms(10), true), I, "{trigger:?}");
        }
    }

    #[test]
    fn inputs_held_on_entry_are_ignored_until_released() {
        let spec = TriggerSpec::compile(Trigger::Pressed).expect("valid");
        let mut memory = Memory::entering(true);
        assert_eq!(evaluate(spec, &mut memory, true, ms(0), false), I);
        assert_eq!(evaluate(spec, &mut memory, false, ms(10), false), I);
        assert_eq!(evaluate(spec, &mut memory, true, ms(20), false), P, "a fresh press fires");
        // Down isn't an edge: a held stick keeps working.
        let spec = TriggerSpec::compile(Trigger::Down).expect("valid");
        assert_eq!(evaluate(spec, &mut Memory::entering(true), true, ms(0), false), T);
    }

    #[test]
    fn durations_must_be_positive_and_fit_in_nanoseconds() {
        // Review FI-004: every finite duration either compiles or is rejected; none panics or
        // wraps.
        let nanos = |seconds| {
            let compiled =
                [Trigger::Hold { seconds }, Trigger::Tap { seconds }, Trigger::DoubleTap { seconds }].map(
                    |trigger| match TriggerSpec::compile(trigger) {
                        Ok(TriggerSpec::Hold(n) | TriggerSpec::Tap(n) | TriggerSpec::DoubleTap(n)) => Some(n),
                        Ok(other) => panic!("{trigger:?} compiled to {other:?}"),
                        Err(_) => None,
                    },
                );
            assert!(compiled.iter().all(|&n| n == compiled[0]), "{seconds} s: {compiled:?}");
            compiled[0]
        };
        // The longest that fits is the largest f32 below 2^64 ns; the next one up is past it.
        let longest = 18_446_743_552.0;
        let rejected = [
            1e20,
            f32::MAX,
            18_446_745_600.0,
            0.0,
            -0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1e-7,
            4.9e-7,
        ];
        for seconds in rejected {
            assert_eq!(nanos(seconds), None, "{seconds} s");
        }
        assert_eq!(nanos(1e-6), Some(1_000), "one microsecond, the shortest");
        assert_eq!(nanos(0.2), Some(200_000_000), "an authored 200 ms is 200 ms exactly");
        assert_eq!(nanos(0.5), Some(500_000_000));
        assert_eq!(nanos(longest), Some(18_446_743_552_000_000_000));
    }
}
