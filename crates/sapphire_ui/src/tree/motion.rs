//! Timelines and a screen's machine: what a tree's nodes do over time, worked out from the time alone.
//!
//! A timeline stores when it started, and each frame reads its keys at that time, so a given time looks
//! the same however many frames led there and whatever they took (`tests/motion.rs`). When one ends, or
//! is cut short, its values stay on the nodes as what they hold, so a fade out doesn't jump back.

use std::time::Duration;

use sapphire_assets::ui::{UiDocument, UiEase, UiKey, UiMotion};
use sapphire_look::UiQuad;
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;

use super::Node;
use crate::layout::Rect;

/// What a timeline changes of a node, on top of what layout and the model say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pose {
    pub(super) opacity: f32,
    pub(super) offset: Vec2,
    pub(super) scale: f32,
}

impl Pose {
    pub(super) const REST: Self = Self { opacity: 1.0, offset: Vec2::ZERO, scale: 1.0 };

    fn set(&mut self, property: UiMotion, value: f32) {
        match property {
            UiMotion::Opacity => self.opacity = value,
            UiMotion::X => self.offset.x = value,
            UiMotion::Y => self.offset.y = value,
            UiMotion::Scale => self.scale = value,
        }
    }
}

/// From a node's place in layout to the screen: a scale and then an offset, with no turn, so a mask
/// stays a rectangle and a hit test a comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    pub(super) scale: f32,
    pub(super) offset: Vec2,
}

impl Placement {
    pub(super) const IDENTITY: Self = Self { scale: 1.0, offset: Vec2::ZERO };

    /// What `pose` does to a node centered on `center`.
    pub(super) fn of_pose(pose: Pose, center: Vec2) -> Self {
        Self { scale: pose.scale, offset: center - center * pose.scale + pose.offset }
    }

    /// `inner`, and then this.
    pub(super) fn after(self, inner: Self) -> Self {
        Self { scale: self.scale * inner.scale, offset: self.offset + inner.offset * self.scale }
    }

    /// At its full size, on whole pixels, so text and edges stay crisp as they move. A node on its way
    /// to its size has no crisp place to be.
    pub(super) fn snapped(self, pixels_per_point: f32) -> Self {
        if self.scale == 1.0 {
            let snap = |value: f32| (value * pixels_per_point).round() / pixels_per_point;
            Self { scale: 1.0, offset: Vec2::new(snap(self.offset.x), snap(self.offset.y)) }
        } else {
            self
        }
    }

    pub(super) fn point(self, point: Vec2) -> Vec2 {
        point * self.scale + self.offset
    }

    pub(super) fn rect(self, rect: Rect) -> Rect {
        Rect::new(self.point(rect.min), self.point(rect.max))
    }

    /// Moves quads written at a node's place in layout to where it is.
    pub(super) fn apply(self, quads: &mut [UiQuad]) {
        if self == Self::IDENTITY {
            return;
        }
        for quad in quads {
            quad.origin = self.point(Vec2::from(quad.origin)).to_array();
            quad.axis_x = [quad.axis_x[0] * self.scale, quad.axis_x[1] * self.scale];
            quad.axis_y = [quad.axis_y[0] * self.scale, quad.axis_y[1] * self.scale];
        }
    }
}

/// `share` of the way from one key to the next, 0 to 1, as `ease` has it.
pub(super) fn eased(ease: UiEase, share: f32) -> f32 {
    // A little past the end and back, by the usual constant: about a tenth too far at its height.
    const BACK: f32 = 1.70158;
    let left = 1.0 - share;
    match ease {
        UiEase::Linear => share,
        UiEase::In => share * share * share,
        UiEase::Out => 1.0 - left * left * left,
        UiEase::InOut if share < 0.5 => 4.0 * share * share * share,
        UiEase::InOut => 1.0 - 4.0 * left * left * left,
        UiEase::Back => 1.0 + (BACK + 1.0) * -(left * left * left) + BACK * left * left,
    }
}

/// `keys`' value at `seconds`, which they hold before the first and after the last.
fn sample(keys: &[UiKey], seconds: f32) -> f32 {
    let after = keys.partition_point(|key| key.at <= seconds);
    match (after.checked_sub(1).and_then(|before| keys.get(before)), keys.get(after)) {
        (Some(before), Some(next)) => {
            let share = (seconds - before.at) / (next.at - before.at);
            before.value + (next.value - before.value) * eased(next.ease, share)
        }
        (Some(key), None) | (None, Some(key)) => key.value,
        (None, None) => 0.0,
    }
}

struct Track {
    node: u32,
    property: UiMotion,
    keys: Box<[UiKey]>,
}

struct Timeline {
    name: Box<str>,
    looping: bool,
    /// When its last key is, in seconds.
    length: f32,
    tracks: Box<[Track]>,
}

impl Timeline {
    /// Where it is, `elapsed` after it started.
    fn seconds(&self, elapsed: Duration) -> f32 {
        if self.looping && self.length > 0.0 {
            (elapsed.as_secs_f64() % f64::from(self.length)) as f32
        } else {
            elapsed.as_secs_f32().min(self.length)
        }
    }

    fn end(&self, start: Timestamp) -> Timestamp {
        Timestamp::from_nanos(start.as_nanos() + Duration::from_secs_f32(self.length).as_nanos() as u64)
    }
}

#[derive(Clone, Copy)]
struct Playing {
    timeline: u16,
    start: Timestamp,
}

struct State {
    name: Box<str>,
    play: Option<u16>,
    then: Option<u16>,
    input: bool,
}

struct Transition {
    from: u16,
    on: Box<str>,
    to: u16,
}

/// A tree's timelines and machine as they run.
pub(super) struct Player {
    timelines: Box<[Timeline]>,
    states: Box<[State]>,
    transitions: Box<[Transition]>,
    /// In the order they started, so what started last wins a property. Holds each timeline at most once,
    /// which is its capacity from the start.
    playing: Vec<Playing>,
    state: Option<u16>,
    started: bool,
}

impl Player {
    /// `document`'s motion, given where a node's name is in the tree. The document has passed its checks.
    pub(super) fn new(document: &UiDocument, node: impl Fn(&str) -> Option<u32>) -> Self {
        let timeline_at = |name: &str| document.timelines.iter().position(|t| t.name == name);
        let state_at = |name: &str| document.states.iter().position(|state| state.name == name);
        let timelines: Box<[Timeline]> = document
            .timelines
            .iter()
            .map(|timeline| {
                let tracks = timeline
                    .tracks
                    .iter()
                    .filter_map(|track| {
                        Some(Track {
                            node: node(&track.node)?,
                            property: track.property,
                            keys: track.keys.as_slice().into(),
                        })
                    })
                    .collect();
                let last = timeline.tracks.iter().filter_map(|track| track.keys.last());
                Timeline {
                    name: timeline.name.as_str().into(),
                    looping: timeline.looping,
                    length: last.map(|key| key.at).fold(0.0, f32::max),
                    tracks,
                }
            })
            .collect();
        let states = document
            .states
            .iter()
            .map(|state| State {
                name: state.name.as_str().into(),
                play: state.play.as_deref().and_then(timeline_at).map(|at| at as u16),
                then: state.then.as_deref().and_then(state_at).map(|at| at as u16),
                input: state.input,
            })
            .collect();
        let transitions = document
            .transitions
            .iter()
            .filter_map(|transition| {
                Some(Transition {
                    from: state_at(&transition.from)? as u16,
                    on: transition.on.as_str().into(),
                    to: state_at(&transition.to)? as u16,
                })
            })
            .collect();
        let playing = Vec::with_capacity(timelines.len());
        Self { timelines, states, transitions, playing, state: None, started: false }
    }

    /// Where the machine is, if the document has one.
    pub(super) fn state(&self) -> Option<&str> {
        self.state.map(|state| &*self.states[usize::from(state)].name)
    }

    /// Whether the machine's state takes input. A screen with no machine always does.
    pub(super) fn accepts_input(&self) -> bool {
        self.state.is_none_or(|state| self.states[usize::from(state)].input) && self.started
    }

    /// Timelines running now.
    pub(super) fn playing(&self) -> usize {
        self.playing.len()
    }

    fn timeline_at(&self, name: &str) -> Option<u16> {
        self.timelines.iter().position(|timeline| &*timeline.name == name).map(|at| at as u16)
    }

    /// Runs what's over by `now`, in the order it ended, whatever frames came between.
    pub(super) fn advance(&mut self, now: Timestamp, nodes: &mut [Node]) {
        if !self.started {
            self.started = true;
            if !self.states.is_empty() {
                self.enter(0, now, nodes);
            }
        }
        // States whose timelines take no time could go round forever: the rest waits for a later frame.
        for _ in 0..=self.timelines.len() + self.states.len() {
            let ended = self
                .playing
                .iter()
                .enumerate()
                .filter(|(_, playing)| !self.timelines[usize::from(playing.timeline)].looping)
                .map(|(at, playing)| (self.timelines[usize::from(playing.timeline)].end(playing.start), at))
                .filter(|(end, _)| *end <= now)
                .min();
            let Some((end, at)) = ended else { return };
            let timeline = self.playing[at].timeline;
            self.leave_on_nodes(at, end, nodes);
            if let Some(state) = self.state {
                let state = &self.states[usize::from(state)];
                if state.play == Some(timeline)
                    && let Some(then) = state.then
                {
                    self.enter(then, end, nodes);
                }
            }
        }
    }

    /// Takes one off the list, its values at `at` staying on its nodes.
    fn leave_on_nodes(&mut self, index: usize, at: Timestamp, nodes: &mut [Node]) {
        let playing = self.playing.remove(index);
        let timeline = &self.timelines[usize::from(playing.timeline)];
        let seconds = timeline.seconds(at.saturating_duration_since(playing.start));
        for track in &timeline.tracks {
            nodes[track.node as usize].held.set(track.property, sample(&track.keys, seconds));
        }
    }

    fn enter(&mut self, state: u16, at: Timestamp, nodes: &mut [Node]) {
        if let Some(old) = self.state
            && let Some(play) = self.states[usize::from(old)].play
        {
            self.halt(play, at, nodes);
        }
        self.state = Some(state);
        if let Some(play) = self.states[usize::from(state)].play {
            self.start(play, at, nodes);
        }
    }

    fn start(&mut self, timeline: u16, at: Timestamp, nodes: &mut [Node]) {
        self.halt(timeline, at, nodes);
        self.playing.push(Playing { timeline, start: at });
    }

    fn halt(&mut self, timeline: u16, at: Timestamp, nodes: &mut [Node]) {
        if let Some(index) = self.playing.iter().position(|playing| playing.timeline == timeline) {
            self.leave_on_nodes(index, at, nodes);
        }
    }

    /// Starts a timeline from its beginning, if the document has one by that name.
    pub(super) fn play(&mut self, name: &str, now: Timestamp, nodes: &mut [Node]) -> bool {
        self.advance(now, nodes);
        let Some(timeline) = self.timeline_at(name) else { return false };
        self.start(timeline, now, nodes);
        true
    }

    /// Stops a timeline where it is. Whether it was running.
    pub(super) fn stop(&mut self, name: &str, now: Timestamp, nodes: &mut [Node]) -> bool {
        self.advance(now, nodes);
        let Some(timeline) = self.timeline_at(name) else { return false };
        let running = self.playing.iter().any(|playing| playing.timeline == timeline);
        self.halt(timeline, now, nodes);
        running
    }

    /// Gives the machine an event. Whether it moved.
    pub(super) fn fire(&mut self, event: &str, now: Timestamp, nodes: &mut [Node]) -> bool {
        self.advance(now, nodes);
        let Some(state) = self.state else { return false };
        let found = self.transitions.iter().find(|t| t.from == state && &*t.on == event);
        let Some(to) = found.map(|transition| transition.to) else { return false };
        self.enter(to, now, nodes);
        true
    }

    /// Every node's pose at `now`: what it holds, and what's playing on top.
    pub(super) fn pose(&self, now: Timestamp, nodes: &mut [Node]) {
        for node in nodes.iter_mut() {
            node.pose = node.held;
        }
        for playing in &self.playing {
            let timeline = &self.timelines[usize::from(playing.timeline)];
            let seconds = timeline.seconds(now.saturating_duration_since(playing.start));
            for track in &timeline.tracks {
                nodes[track.node as usize].pose.set(track.property, sample(&track.keys, seconds));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(at: f32, value: f32, ease: UiEase) -> UiKey {
        UiKey { at, value, ease }
    }

    #[test]
    fn keys_hold_at_their_ends_and_ease_between() {
        let keys =
            [key(1.0, 10.0, UiEase::Linear), key(2.0, 20.0, UiEase::Linear), key(3.0, 40.0, UiEase::Out)];
        assert_eq!(sample(&keys, 0.0), 10.0);
        assert_eq!(sample(&keys, 1.0), 10.0);
        assert_eq!(sample(&keys, 1.5), 15.0);
        assert_eq!(sample(&keys, 2.0), 20.0);
        // Most of the way by the middle: a cubic out is at seven eighths.
        assert_eq!(sample(&keys, 2.5), 20.0 + 20.0 * 0.875);
        assert_eq!(sample(&keys, 3.0), 40.0);
        assert_eq!(sample(&keys, 9.0), 40.0);
    }

    #[test]
    fn every_ease_starts_at_nothing_and_ends_at_all() {
        for ease in [UiEase::Linear, UiEase::In, UiEase::Out, UiEase::InOut, UiEase::Back] {
            assert!(eased(ease, 0.0).abs() < 1e-6, "{ease:?}");
            assert!((eased(ease, 1.0) - 1.0).abs() < 1e-6, "{ease:?}");
        }
        assert!((0.0..0.5).contains(&eased(UiEase::In, 0.5)));
        assert!((0.5..1.0).contains(&eased(UiEase::Out, 0.5)));
        assert_eq!(eased(UiEase::InOut, 0.5), 0.5);
        let highest = (0..=100).map(|at| eased(UiEase::Back, at as f32 / 100.0)).fold(0.0, f32::max);
        assert!((1.05..1.15).contains(&highest), "{highest}");
    }

    #[test]
    fn a_placement_scales_about_the_center_and_composes() {
        let center = Vec2::new(100.0, 50.0);
        let pose = Pose { opacity: 1.0, offset: Vec2::new(10.0, 0.0), scale: 2.0 };
        let own = Placement::of_pose(pose, center);
        assert_eq!(own.point(center), center + Vec2::new(10.0, 0.0));
        assert_eq!(own.point(center + Vec2::X), center + Vec2::new(12.0, 0.0));
        let outer = Placement { scale: 0.5, offset: Vec2::new(1.0, 1.0) };
        assert_eq!(outer.after(own).point(center), outer.point(own.point(center)));
    }

    #[test]
    fn a_placement_at_full_size_lands_on_whole_pixels() {
        let moving = Placement { scale: 1.0, offset: Vec2::new(10.3, 7.8) };
        assert_eq!(moving.snapped(2.0).offset, Vec2::new(10.5, 8.0));
        let scaled = Placement { scale: 1.5, offset: Vec2::new(10.3, 7.8) };
        assert_eq!(scaled.snapped(2.0), scaled);
    }
}
