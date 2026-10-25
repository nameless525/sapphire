//! Touch gestures: turns fingers into the [`Gesture`](crate::Gesture) sources.
//!
//! - **Press:** a lone finger down. It ends when the finger lifts, and is cut short when the
//!   finger moves past the tap slop or a second finger lands.
//! - **Drag:** a lone finger's movement, in window points with +y up.
//! - **Pinch:** with two or more fingers, the first two (by landing order) measure their
//!   spread: each move adds the natural log of the distance ratio, so the sum over any number
//!   of moves is the log of the total spread.
//! - **Swipe:** a lone finger that lifts at least the swipe distance from where it landed, within
//!   the swipe time, by its dominant direction. It must stay alone the whole time.

use sapphire_math::Vec2;
use sapphire_platform::{Timestamp, TouchPhase};

use crate::bindings::GestureSettings;
use crate::devices::MAX_FINGERS;
use crate::source::SwipeDirection;

/// Below this many points apart, two fingers give no usable pinch ratio.
const MIN_PINCH_SPAN: f32 = 1.0;

#[derive(Clone, Copy, Debug)]
struct Finger {
    landed_at: Vec2,
    at: Vec2,
    landed: Timestamp,
    order: u64,
}

/// How a touch changed the press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PressChange {
    Down,
    /// The finger lifted.
    Up,
    /// The press stopped being one (a drag, a second finger, the OS canceling the touch):
    /// nothing a release would fire may fire.
    Cancel,
}

/// What one touch event contributed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GestureOutput {
    pub drag: Option<Vec2>,
    pub pinch: Option<f32>,
    pub press: Option<PressChange>,
    pub swipe: Option<SwipeDirection>,
}

#[derive(Debug, Default)]
pub(crate) struct Gestures {
    fingers: [Option<Finger>; MAX_FINGERS],
    next_order: u64,
    /// The finger that landed alone and has stayed alone: the candidate for a swipe.
    stroke: Option<usize>,
    pressing: bool,
}

impl Gestures {
    /// Feeds one touch event; `position` is in window points, +y down as the screen reports it.
    pub fn touch(
        &mut self,
        now: Timestamp,
        finger: usize,
        phase: TouchPhase,
        position: Vec2,
        settings: &GestureSettings,
    ) -> GestureOutput {
        let mut out = GestureOutput::default();
        if finger >= MAX_FINGERS {
            return out;
        }
        match phase {
            TouchPhase::Down => {
                // A finger already tracked lost its release; it starts over where it is.
                self.fingers[finger] =
                    Some(Finger { landed_at: position, at: position, landed: now, order: self.next_order });
                self.next_order += 1;
                if self.count() == 1 {
                    self.stroke = Some(finger);
                    self.pressing = true;
                    out.press = Some(PressChange::Down);
                } else {
                    self.stroke = None;
                    out.press = self.cancel_press();
                }
            }
            TouchPhase::Moved => {
                let Some(moved) = self.fingers[finger] else { return out };
                if self.count() == 1 {
                    // Screen +y is down; gesture values are +y up.
                    out.drag = Some(Vec2::new(position.x - moved.at.x, moved.at.y - position.y));
                    if self.pressing && position.distance(moved.landed_at) > settings.tap_slop {
                        out.press = self.cancel_press();
                    }
                } else if let Some(other) = self.pinch_partner(finger) {
                    let before = moved.at.distance(other.at);
                    let after = position.distance(other.at);
                    if before >= MIN_PINCH_SPAN && after >= MIN_PINCH_SPAN {
                        out.pinch = Some((after / before).ln());
                    }
                }
                if let Some(moved) = &mut self.fingers[finger] {
                    moved.at = position;
                }
            }
            TouchPhase::Up => {
                let Some(lifted) = self.fingers[finger].take() else { return out };
                if self.stroke.take_if(|&mut stroke| stroke == finger).is_some() {
                    if self.pressing {
                        self.pressing = false;
                        out.press = Some(PressChange::Up);
                    }
                    let travel = Vec2::new(position.x - lifted.landed_at.x, lifted.landed_at.y - position.y);
                    let quick = now.saturating_duration_since(lifted.landed).as_secs_f32()
                        <= settings.swipe_max_seconds;
                    if quick && travel.length() >= settings.swipe_min_distance {
                        out.swipe = Some(direction(travel));
                    }
                }
            }
            TouchPhase::Canceled => {
                if self.fingers[finger].take().is_some()
                    && self.stroke.take_if(|&mut s| s == finger).is_some()
                {
                    out.press = self.cancel_press();
                }
            }
        }
        out
    }

    /// Forgets every finger: the touch screen let go (focus or the foreground was lost).
    pub fn reset(&mut self) {
        *self = Self { next_order: self.next_order, ..Self::default() };
    }

    fn count(&self) -> usize {
        self.fingers.iter().flatten().count()
    }

    fn cancel_press(&mut self) -> Option<PressChange> {
        std::mem::take(&mut self.pressing).then_some(PressChange::Cancel)
    }

    /// The other finger of the pinch pair, if `finger` is in it: the two fingers that landed
    /// first among those down.
    fn pinch_partner(&self, finger: usize) -> Option<Finger> {
        let mut first: Option<(usize, Finger)> = None;
        let mut second: Option<(usize, Finger)> = None;
        for (index, slot) in self.fingers.iter().enumerate() {
            let Some(candidate) = *slot else { continue };
            if first.is_none_or(|(_, f)| candidate.order < f.order) {
                second = first;
                first = Some((index, candidate));
            } else if second.is_none_or(|(_, f)| candidate.order < f.order) {
                second = Some((index, candidate));
            }
        }
        match (first, second) {
            (Some((a, _)), Some((_, other))) if a == finger => Some(other),
            (Some((_, other)), Some((b, _))) if b == finger => Some(other),
            _ => None,
        }
    }
}

fn direction(travel: Vec2) -> SwipeDirection {
    if travel.x.abs() >= travel.y.abs() {
        if travel.x >= 0.0 { SwipeDirection::Right } else { SwipeDirection::Left }
    } else if travel.y >= 0.0 {
        SwipeDirection::Up
    } else {
        SwipeDirection::Down
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Timestamp {
        Timestamp::from_nanos(ms * 1_000_000)
    }

    struct Touches {
        gestures: Gestures,
        settings: GestureSettings,
    }

    impl Touches {
        fn new() -> Self {
            Self { gestures: Gestures::default(), settings: GestureSettings::default() }
        }

        fn touch(&mut self, t: u64, finger: usize, phase: TouchPhase, x: f32, y: f32) -> GestureOutput {
            self.gestures.touch(ms(t), finger, phase, Vec2::new(x, y), &self.settings)
        }
    }

    #[test]
    fn one_finger_drags_with_y_up() {
        let mut t = Touches::new();
        assert_eq!(t.touch(0, 0, TouchPhase::Down, 100.0, 100.0).press, Some(PressChange::Down));
        let out = t.touch(10, 0, TouchPhase::Moved, 104.0, 97.0);
        assert_eq!(out.drag, Some(Vec2::new(4.0, 3.0)));
        assert_eq!(out.press, None, "still within the tap slop");
        let out = t.touch(20, 0, TouchPhase::Moved, 120.0, 97.0);
        assert_eq!(out.press, Some(PressChange::Cancel), "past the slop it is a drag, not a press");
        // Slow: no swipe when it lifts, and the press is already over.
        assert_eq!(t.touch(900, 0, TouchPhase::Up, 200.0, 97.0), GestureOutput::default());
    }

    #[test]
    fn swipes_report_their_dominant_direction() {
        let cases = [
            ((180.0, 110.0), SwipeDirection::Right),
            ((20.0, 90.0), SwipeDirection::Left),
            ((110.0, 20.0), SwipeDirection::Up),
            ((90.0, 180.0), SwipeDirection::Down),
        ];
        for ((x, y), expected) in cases {
            let mut t = Touches::new();
            t.touch(0, 3, TouchPhase::Down, 100.0, 100.0);
            t.touch(50, 3, TouchPhase::Moved, x, y);
            let out = t.touch(120, 3, TouchPhase::Up, x, y);
            assert_eq!(out.swipe, Some(expected), "to ({x}, {y})");
        }

        let mut t = Touches::new();
        t.touch(0, 0, TouchPhase::Down, 100.0, 100.0);
        assert_eq!(t.touch(100, 0, TouchPhase::Up, 130.0, 100.0).swipe, None, "too short");
        t.touch(200, 0, TouchPhase::Down, 100.0, 100.0);
        assert_eq!(t.touch(600, 0, TouchPhase::Up, 300.0, 100.0).swipe, None, "too slow");
    }

    #[test]
    fn a_tap_presses_and_releases() {
        let mut t = Touches::new();
        assert_eq!(t.touch(0, 0, TouchPhase::Down, 50.0, 50.0).press, Some(PressChange::Down));
        assert_eq!(t.touch(80, 0, TouchPhase::Up, 52.0, 51.0).press, Some(PressChange::Up));
    }

    #[test]
    fn two_fingers_pinch_and_cancel_the_press() {
        let mut t = Touches::new();
        t.touch(0, 0, TouchPhase::Down, 100.0, 100.0);
        let out = t.touch(10, 1, TouchPhase::Down, 200.0, 100.0);
        assert_eq!(out.press, Some(PressChange::Cancel));
        // Spread from 100 to 200 points apart: ln 2.
        let out = t.touch(20, 1, TouchPhase::Moved, 300.0, 100.0);
        assert!((out.pinch.expect("pinch") - 2f32.ln()).abs() < 1e-6);
        assert_eq!(out.drag, None, "two fingers don't drag");
        // Back to 100 apart: ln 0.5, so the total is zero.
        let out = t.touch(30, 0, TouchPhase::Moved, 200.0, 100.0);
        assert!((out.pinch.expect("pinch") - 0.5f32.ln()).abs() < 1e-6);

        // A third finger isn't part of the pair.
        t.touch(40, 2, TouchPhase::Down, 500.0, 500.0);
        assert_eq!(t.touch(50, 2, TouchPhase::Moved, 600.0, 500.0).pinch, None);

        // Lifting one of the pair leaves two: the remaining pair pinches.
        assert_eq!(
            t.touch(60, 0, TouchPhase::Up, 200.0, 100.0).swipe,
            None,
            "a multi-finger lift isn't a swipe"
        );
        assert!(t.touch(70, 1, TouchPhase::Moved, 400.0, 100.0).pinch.is_some());
        // Down to one finger: it drags.
        t.touch(80, 2, TouchPhase::Up, 600.0, 500.0);
        assert_eq!(t.touch(90, 1, TouchPhase::Moved, 410.0, 100.0).drag, Some(Vec2::new(10.0, 0.0)));
    }

    #[test]
    fn a_canceled_touch_cancels_the_press_without_a_swipe() {
        let mut t = Touches::new();
        t.touch(0, 0, TouchPhase::Down, 100.0, 100.0);
        let out = t.touch(50, 0, TouchPhase::Canceled, 300.0, 100.0);
        assert_eq!(out, GestureOutput { press: Some(PressChange::Cancel), ..GestureOutput::default() });
    }
}
