//! Rumble: what a pad's two motors do while cues overlap.
//!
//! SDL holds one rumble per pad, and each call replaces the last, so hits that overlap have to meet
//! somewhere before SDL: [`RumbleMixer`] holds what's playing, and tells a [`RumbleSink`] when the
//! strongest of them changes, and only then, since each call may write to the device. Each motor reads
//! the strongest rumble still playing, so a light hit landing during a heavy one's thump can't cut it.
//!
//! **A rumble is a flat pulse:** a strength per motor and a length. Fading one would mean calling SDL
//! every frame, and nobody here can feel what that would buy. It's a table's numbers, on the list of
//! what only the maintainer can judge. It runs on real seconds, so it ends on time through hitstop,
//! slow motion and a pause.
//!
//! Each call tells SDL how long to run, so a frame that never comes can't leave a pad running: it stops
//! itself when the longest rumble was due to.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The most rumbles playing at once. A rumble past this replaces the one nearest its end.
pub const MAX_RUMBLES: usize = 8;

/// One rumble, as a table authors it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rumble {
    /// The low-frequency motor, the heavy one: 0 to 1.
    pub low: f32,
    /// The high-frequency motor, the sharp one: 0 to 1.
    pub high: f32,
    /// How long, in real seconds.
    pub seconds: f32,
}

/// What rumbles: the player's pad, in an app, by SDL. A touch device has none, and an app there hands
/// the dispatcher `()`, which ignores it.
pub trait RumbleSink {
    /// Each motor at its strength, 0 to 1, for `duration` or until the next call. Zeros stop it.
    fn rumble(&mut self, low: f32, high: f32, duration: Duration);
}

impl RumbleSink for () {
    fn rumble(&mut self, _low: f32, _high: f32, _duration: Duration) {}
}

/// A rumble playing.
#[derive(Clone, Copy, Debug, Default)]
struct Playing {
    rumble: Rumble,
    /// Started since the last update, so no time has passed for it yet.
    fresh: bool,
}

/// The rumbles playing, and what the sink was last told. Fixed size, so starting one and updating
/// allocate nothing. Each motor reads the strongest still playing.
#[derive(Clone, Debug)]
pub struct RumbleMixer {
    playing: [Playing; MAX_RUMBLES],
    count: usize,
    /// What the sink was last told, the player's setting and all: what the pad is doing.
    sent: (f32, f32),
    setting: f32,
}

impl Default for RumbleMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl RumbleMixer {
    #[must_use]
    pub fn new() -> Self {
        Self { playing: [Playing::default(); MAX_RUMBLES], count: 0, sent: (0.0, 0.0), setting: 1.0 }
    }

    /// The player's rumble setting, 0 (none, for accessibility) to 1 (as authored). What's playing takes
    /// it at the next [`Self::update`], and 0 stops a pad that's running.
    pub fn set_setting(&mut self, setting: f32) {
        self.setting = if setting.is_finite() { setting.clamp(0.0, 1.0) } else { 0.0 };
    }

    /// Starts `rumble`. A full mixer lets go of the one nearest its end.
    pub fn start(&mut self, rumble: Rumble) {
        let playing = Playing { rumble, fresh: true };
        if self.count < MAX_RUMBLES {
            self.playing[self.count] = playing;
            self.count += 1;
            return;
        }
        let mut soonest = 0;
        for (index, other) in self.playing.iter().enumerate() {
            if other.rumble.seconds < self.playing[soonest].rumble.seconds {
                soonest = index;
            }
        }
        self.playing[soonest] = playing;
    }

    /// Rumbles playing now.
    #[must_use]
    pub const fn playing(&self) -> usize {
        self.count
    }

    /// Once a frame, `dt` real seconds after the last: ages what's playing and tells `sink` what changed.
    pub fn update(&mut self, dt: f32, sink: &mut dyn RumbleSink) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let mut started = false;
        let mut kept = 0;
        for index in 0..self.count {
            let mut playing = self.playing[index];
            if playing.fresh {
                playing.fresh = false;
                started = true;
            } else {
                playing.rumble.seconds -= dt;
            }
            if playing.rumble.seconds > 0.0 {
                self.playing[kept] = playing;
                kept += 1;
            }
        }
        self.count = kept;

        let mut strongest = (0.0_f32, 0.0_f32);
        let mut longest = 0.0_f32;
        for playing in &self.playing[..self.count] {
            strongest.0 = strongest.0.max(playing.rumble.low);
            strongest.1 = strongest.1.max(playing.rumble.high);
            longest = longest.max(playing.rumble.seconds);
        }
        let felt = (strongest.0 * self.setting, strongest.1 * self.setting);
        let running = felt != (0.0, 0.0);
        // A rumble that changes nothing may still outlast what SDL was told.
        if felt != self.sent || (started && running) {
            self.sent = felt;
            let duration = if running { Duration::from_secs_f32(longest) } else { Duration::ZERO };
            sink.rumble(felt.0, felt.1, duration);
        }
    }

    /// Lets go of everything from here on, for a caller with no sink to hand: the next
    /// [`Self::update`] tells the sink, along with whatever has started since.
    pub fn release(&mut self) {
        self.count = 0;
    }

    /// Lets go of everything at once: the app lost focus or the foreground.
    pub fn stop(&mut self, sink: &mut dyn RumbleSink) {
        self.count = 0;
        if self.sent != (0.0, 0.0) {
            self.sent = (0.0, 0.0);
            sink.rumble(0.0, 0.0, Duration::ZERO);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    /// Writes down what it's told.
    #[derive(Default)]
    struct Pad(Vec<(f32, f32, f32)>);

    impl RumbleSink for Pad {
        fn rumble(&mut self, low: f32, high: f32, duration: Duration) {
            self.0.push((low, high, duration.as_secs_f32()));
        }
    }

    fn pulse(low: f32, high: f32, seconds: f32) -> Rumble {
        Rumble { low, high, seconds }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_released_rumble_stops_at_the_next_update_and_a_new_one_takes_over() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.5, 0.25, 1.0));
        mixer.update(FRAME, &mut pad);
        mixer.release();
        assert_eq!(mixer.playing(), 0);
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0, [(0.5, 0.25, 1.0), (0.0, 0.0, 0.0)], "the pad hears it stop");

        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.5, 0.25, 1.0));
        mixer.update(FRAME, &mut pad);
        mixer.release();
        mixer.start(pulse(0.1, 0.2, 0.5));
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0, [(0.5, 0.25, 1.0), (0.1, 0.2, 0.5)]);
    }

    #[test]
    fn a_rumble_starts_at_once_runs_its_length_and_stops() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.5, 0.25, 0.05));
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0, [(0.5, 0.25, 0.05)], "its whole length: no time has passed for it");

        mixer.update(FRAME, &mut pad);
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0.len(), 1, "nothing changed, so nothing is sent");
        assert_eq!(mixer.playing(), 1);

        mixer.update(FRAME, &mut pad);
        assert_eq!(mixer.playing(), 0);
        assert_eq!(pad.0[1], (0.0, 0.0, 0.0), "and a stop");
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0.len(), 2, "once");
    }

    #[test]
    fn each_motor_reads_the_strongest_playing() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.8, 0.1, 0.2));
        mixer.update(FRAME, &mut pad);
        // A light hit during a heavy thump adds its sharp motor and can't cut the low one.
        mixer.start(pulse(0.1, 0.5, 0.05));
        mixer.update(FRAME, &mut pad);
        let (low, high, seconds) = pad.0[1];
        assert_eq!((low, high), (0.8, 0.5));
        assert!(close(seconds, 0.2 - FRAME), "as long as the longest: {seconds}");
        // When it ends, the heavy one is alone again, without a gap.
        for _ in 0..4 {
            mixer.update(FRAME, &mut pad);
        }
        let (low, high, _) = *pad.0.last().expect("sent");
        assert_eq!((low, high), (0.8, 0.1));
    }

    /// One that changes no strength still has to reach SDL, or the pad stops when the older one was due to.
    #[test]
    fn a_rumble_that_outlasts_another_extends_it() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.5, 0.5, 0.05));
        mixer.update(FRAME, &mut pad);
        mixer.start(pulse(0.5, 0.5, 0.5));
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0.len(), 2);
        assert_eq!(pad.0[1].2, 0.5);
    }

    #[test]
    fn the_player_can_turn_it_down_or_off() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.set_setting(0.5);
        mixer.start(pulse(0.8, 0.4, 0.1));
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0, [(0.4, 0.2, 0.1)]);

        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.set_setting(0.0);
        mixer.start(pulse(0.8, 0.4, 0.1));
        for _ in 0..30 {
            mixer.update(FRAME, &mut pad);
        }
        assert!(pad.0.is_empty(), "{:?}", pad.0);
        mixer.set_setting(f32::NAN);
        mixer.start(pulse(0.8, 0.4, 0.1));
        mixer.update(FRAME, &mut pad);
        assert!(pad.0.is_empty(), "{:?}", pad.0);
    }

    /// A pad runs what it was last told until it's told otherwise, so a new setting has to reach it.
    #[test]
    fn a_new_setting_reaches_what_is_already_playing() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.8, 0.4, 10.0));
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0, [(0.8, 0.4, 10.0)]);

        mixer.set_setting(0.5);
        mixer.update(FRAME, &mut pad);
        let (low, high, seconds) = pad.0[1];
        assert_eq!((low, high), (0.4, 0.2));
        assert!(close(seconds, 10.0 - FRAME), "only what's left of it: {seconds}");

        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0.len(), 2, "told once");
    }

    #[test]
    fn turning_it_off_stops_a_running_pad_and_turning_it_on_takes_up_what_is_left() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.8, 0.4, 10.0));
        mixer.update(FRAME, &mut pad);

        mixer.set_setting(0.0);
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0[1], (0.0, 0.0, 0.0), "a stop, at once");
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0.len(), 2, "once");
        assert_eq!(mixer.playing(), 1, "it goes on playing, unfelt");

        mixer.set_setting(1.0);
        mixer.update(FRAME, &mut pad);
        let (low, high, seconds) = pad.0[2];
        assert_eq!((low, high), (0.8, 0.4));
        assert!(close(seconds, 10.0 - 3.0 * FRAME), "only what's left of it: {seconds}");
    }

    /// Off, with the pad stopped, nothing that plays or ends needs telling it.
    #[test]
    fn a_pulse_that_ends_while_it_is_off_says_nothing_more() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.8, 0.4, 0.1));
        mixer.update(FRAME, &mut pad);
        mixer.set_setting(0.0);
        mixer.update(FRAME, &mut pad);
        mixer.start(pulse(1.0, 1.0, 0.1));
        for _ in 0..30 {
            mixer.update(FRAME, &mut pad);
        }
        assert_eq!(mixer.playing(), 0);
        assert_eq!(pad.0.len(), 2, "{:?}", pad.0);
    }

    /// Focus lost with the setting just turned off, no update between: the pad is still running.
    #[test]
    fn stopping_reaches_a_running_pad_whatever_the_setting() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.8, 0.4, 10.0));
        mixer.update(FRAME, &mut pad);
        mixer.set_setting(0.0);
        mixer.stop(&mut pad);
        assert_eq!(pad.0[1], (0.0, 0.0, 0.0));
        mixer.update(FRAME, &mut pad);
        mixer.stop(&mut pad);
        assert_eq!(pad.0.len(), 2, "told once");
    }

    /// Letting go for a fight's restart, with no pad to hand: the next update has one.
    #[test]
    fn a_release_is_scaled_and_told_like_anything_else() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.8, 0.4, 10.0));
        mixer.update(FRAME, &mut pad);
        mixer.set_setting(0.5);
        mixer.release();
        mixer.start(pulse(0.2, 0.6, 1.0));
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0[1], (0.1, 0.3, 1.0), "the new one, by the new setting");

        mixer.set_setting(0.0);
        mixer.release();
        mixer.update(FRAME, &mut pad);
        assert_eq!(pad.0[2], (0.0, 0.0, 0.0), "off and let go: the pad still hears it stop");
    }

    #[test]
    fn stopping_lets_go_at_once_and_only_tells_a_pad_that_was_running() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.stop(&mut pad);
        assert!(pad.0.is_empty());
        mixer.start(pulse(1.0, 1.0, 1.0));
        mixer.update(FRAME, &mut pad);
        mixer.stop(&mut pad);
        assert_eq!(pad.0[1], (0.0, 0.0, 0.0));
        assert_eq!(mixer.playing(), 0);
    }

    #[test]
    fn a_full_mixer_lets_go_of_the_one_nearest_its_end() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        for index in 0..MAX_RUMBLES {
            mixer.start(pulse(0.1, 0.1, 0.5 + index as f32 * 0.1));
        }
        mixer.start(pulse(1.0, 0.1, 0.05));
        assert_eq!(mixer.playing(), MAX_RUMBLES);
        mixer.update(FRAME, &mut pad);
        let (low, _, seconds) = pad.0[0];
        assert_eq!(low, 1.0, "the new one plays");
        assert!(close(seconds, 0.5 + (MAX_RUMBLES - 1) as f32 * 0.1), "the longest stays: {seconds}");
    }

    #[test]
    fn nonsense_time_ages_nothing() {
        let (mut mixer, mut pad) = (RumbleMixer::new(), Pad::default());
        mixer.start(pulse(0.5, 0.5, 0.1));
        mixer.update(FRAME, &mut pad);
        mixer.update(f32::NAN, &mut pad);
        mixer.update(-1.0, &mut pad);
        assert_eq!(mixer.playing(), 1);
    }
}
