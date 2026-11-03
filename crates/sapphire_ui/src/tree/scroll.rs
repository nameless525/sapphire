//! A scroller's position, as a function of time like a timeline's values: a retained offset that moves
//! a masked node's children with no layout pass.

use std::time::Duration;

use sapphire_assets::ui::UiScrollAxis;
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;

use super::motion::eased;

/// How long a move to a new place takes, in seconds.
const TWEEN_SECONDS: f32 = 0.16;

/// How quickly a flick dies, per second.
const FLING_DECAY: f32 = 6.0;

/// A flick slower than this, in points a second, has stopped.
const FLING_STOPS: f32 = 8.0;

#[derive(Clone, Copy, Debug)]
enum Motion {
    Still(f32),
    Tween { from: f32, to: f32, start: Timestamp },
    Fling { from: f32, velocity: f32, start: Timestamp },
}

/// One scrolling node's state.
#[derive(Clone, Copy, Debug)]
pub(super) struct Scroll {
    pub(super) axis: UiScrollAxis,
    /// How far it can go, in points: what its children overflow it by.
    pub(super) extent: f32,
    motion: Motion,
}

fn seconds_since(now: Timestamp, start: Timestamp) -> f32 {
    now.saturating_duration_since(start).min(Duration::from_secs(3600)).as_secs_f32()
}

impl Scroll {
    pub(super) const fn new(axis: UiScrollAxis) -> Self {
        Self { axis, extent: 0.0, motion: Motion::Still(0.0) }
    }

    /// `offset` along its axis.
    pub(super) fn along(&self, value: Vec2) -> f32 {
        match self.axis {
            UiScrollAxis::Vertical => value.y,
            UiScrollAxis::Horizontal => value.x,
        }
    }

    /// `amount` along its axis, as a vector.
    pub(super) fn vector(&self, amount: f32) -> Vec2 {
        match self.axis {
            UiScrollAxis::Vertical => Vec2::new(0.0, amount),
            UiScrollAxis::Horizontal => Vec2::new(amount, 0.0),
        }
    }

    /// How far along it is at `now`.
    pub(super) fn offset(&self, now: Timestamp) -> f32 {
        let offset = match self.motion {
            Motion::Still(offset) => offset,
            Motion::Tween { from, to, start } => {
                let share = (seconds_since(now, start) / TWEEN_SECONDS).min(1.0);
                from + (to - from) * eased(sapphire_assets::ui::UiEase::Out, share)
            }
            Motion::Fling { from, velocity, start } => {
                let seconds = seconds_since(now, start);
                from + velocity * (1.0 - (-FLING_DECAY * seconds).exp()) / FLING_DECAY
            }
        };
        offset.clamp(0.0, self.extent)
    }

    /// Where it will come to rest.
    pub(super) fn target(&self) -> f32 {
        let target = match self.motion {
            Motion::Still(offset) => offset,
            Motion::Tween { to, .. } => to,
            Motion::Fling { from, velocity, .. } => from + velocity / FLING_DECAY,
        };
        target.clamp(0.0, self.extent)
    }

    /// Whether it's on its way somewhere at `now`.
    pub(super) fn moving(&self, now: Timestamp) -> bool {
        match self.motion {
            Motion::Still(_) => false,
            Motion::Tween { start, .. } => seconds_since(now, start) < TWEEN_SECONDS,
            Motion::Fling { velocity, start, .. } => {
                let left = velocity * (-FLING_DECAY * seconds_since(now, start)).exp();
                left.abs() >= FLING_STOPS && self.offset(now) > 0.0 && self.offset(now) < self.extent
            }
        }
    }

    /// Moves smoothly to `target`.
    pub(super) fn go_to(&mut self, target: f32, now: Timestamp) {
        let (from, to) = (self.offset(now), target.clamp(0.0, self.extent));
        self.motion = if (to - from).abs() < f32::EPSILON {
            Motion::Still(to)
        } else {
            Motion::Tween { from, to, start: now }
        };
    }

    /// Goes straight to `offset`: a finger holding it.
    pub(super) fn hold_at(&mut self, offset: f32) {
        self.motion = Motion::Still(offset.clamp(0.0, self.extent));
    }

    /// Lets go at `velocity`, in points a second.
    pub(super) fn fling(&mut self, velocity: f32, now: Timestamp) {
        let from = self.offset(now);
        self.motion = if velocity.abs() < FLING_STOPS {
            Motion::Still(from)
        } else {
            Motion::Fling { from, velocity, start: now }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: f32) -> Timestamp {
        Timestamp::from_nanos((seconds * 1e9) as u64)
    }

    fn scroll() -> Scroll {
        Scroll { extent: 500.0, ..Scroll::new(UiScrollAxis::Vertical) }
    }

    #[test]
    fn a_tween_arrives_and_stops_within_its_extent() {
        let mut scroll = scroll();
        scroll.go_to(100.0, at(1.0));
        assert_eq!(scroll.offset(at(1.0)), 0.0);
        assert!(scroll.moving(at(1.05)));
        let middle = scroll.offset(at(1.08));
        assert!(middle > 50.0 && middle < 100.0, "{middle}");
        assert_eq!(scroll.offset(at(2.0)), 100.0);
        assert!(!scroll.moving(at(2.0)));
        scroll.go_to(9000.0, at(2.0));
        assert_eq!(scroll.target(), 500.0);
        assert_eq!(scroll.offset(at(3.0)), 500.0);
    }

    #[test]
    fn a_new_target_starts_from_where_it_is_now() {
        let mut scroll = scroll();
        scroll.go_to(100.0, at(0.0));
        let there = scroll.offset(at(0.08));
        scroll.go_to(200.0, at(0.08));
        assert_eq!(scroll.offset(at(0.08)), there);
    }

    #[test]
    fn a_fling_slows_to_rest_where_it_said() {
        let mut scroll = scroll();
        scroll.fling(600.0, at(0.0));
        assert!(scroll.moving(at(0.1)));
        // Its whole distance is its speed over its decay.
        assert_eq!(scroll.target(), 100.0);
        assert!((scroll.offset(at(10.0)) - 100.0).abs() < 0.01);
        assert!(!scroll.moving(at(10.0)));
    }

    #[test]
    fn a_fling_stops_at_the_end_of_what_there_is() {
        let mut scroll = Scroll { extent: 20.0, ..scroll() };
        scroll.fling(6000.0, at(0.0));
        assert_eq!(scroll.offset(at(1.0)), 20.0);
        assert!(!scroll.moving(at(1.0)));
    }

    #[test]
    fn a_position_stays_inside_an_extent_that_shrinks() {
        let mut scroll = scroll();
        scroll.hold_at(400.0);
        scroll.extent = 100.0;
        assert_eq!(scroll.offset(at(0.0)), 100.0);
    }
}
