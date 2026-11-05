//! Merging what every thread drew into a frame's list, and keeping what lasts longer than a frame.

use super::Lifetime;
use super::buffer::{self, Command, What};
use super::list::DebugDrawList;

/// The most timed shapes and labels alive at once; more are dropped and counted.
pub const MAX_PERSISTENT: usize = 1 << 14;

/// The most shapes and labels one fixed step holds; more are dropped and counted.
pub const MAX_HELD: usize = 1 << 14;

/// Merges every thread's draws into a frame's [`DebugDrawList`], once a frame, at extract, and
/// keeps what lasts longer than the frame: what a fixed step drew, and timed draws. One per app.
#[derive(Debug, Default)]
pub struct Collector {
    /// What the last fixed step that drew, drew.
    held: Vec<Command>,
    /// That step's number.
    held_step: u32,
    /// The clock when it arrived.
    held_at: f64,
    /// Timed draws, with when each ends on the clock.
    persistent: Vec<(Command, f64)>,
    /// The clock at the last collect.
    now: f64,
    /// Calls lost to a full store this frame.
    dropped: u32,
    /// Drawing session last collected. A changed generation invalidates all retained commands.
    generation: u64,
}

impl Collector {
    pub const fn new() -> Self {
        Self {
            held: Vec::new(),
            held_step: 0,
            held_at: 0.0,
            persistent: Vec::new(),
            now: 0.0,
            dropped: 0,
            generation: 0,
        }
    }

    /// Replaces `list` with this frame's draws: what every thread drew since the last collect, what
    /// the last fixed step holds and the timed draws still alive at `now`, seconds on the clock
    /// timed draws run on (see the [module docs](super)). Call it once a frame; `now` needn't
    /// move. A clock that goes back (a new scene, a replay started over) ends what was timed on it.
    ///
    /// While drawing is off it leaves the list empty, dropping what any thread left waiting and
    /// all it was holding. Allocates only as its stores and the list grow to what a frame draws.
    pub fn collect(&mut self, now: f64, list: &mut DebugDrawList) {
        list.clear();
        let generation = super::generation();
        if self.generation != generation {
            self.clear();
            self.generation = generation;
        }
        if generation & 1 == 0 {
            // Never-enabled shipping frames take no registry lock. Once drawing has been
            // enabled, keep reclaiming late publications and ended producer rings while off.
            if generation != 0 {
                buffer::drain_all(generation, &mut |_| {});
            }
            return;
        }
        self.set_clock(now);
        let dropped = buffer::drain_all(generation, &mut |command| {
            if command.generation == generation {
                self.take(command, list, buffer::current_step());
            }
        });
        self.finish(buffer::current_step(), list);
        list.stats.dropped += dropped.saturating_add(self.dropped);
        self.dropped = 0;
    }

    /// Forgets timed draws and what a step holds, as when a scene is replaced.
    pub fn clear(&mut self) {
        self.held.clear();
        self.held_step = 0;
        self.persistent.clear();
    }

    /// Takes the frame's clock. Going back, it ends what was timed on it.
    fn set_clock(&mut self, now: f64) {
        if now < self.now {
            self.clear();
        }
        self.now = now;
        self.persistent.retain(|(_, ends)| *ends > now);
    }

    /// One call a thread made: drawn now if it lasts this frame alone, or kept for
    /// [`Self::finish`].
    fn take(&mut self, command: Command, list: &mut DebugDrawList, step: u32) {
        match command.lifetime {
            Lifetime::Seconds(seconds) => {
                let ends = self.now + f64::from(seconds);
                // Keep the previous behavior for directly constructed nonpositive/NaN
                // lifetimes, and for durations below this clock value's resolution.
                if ends > self.now {
                    if self.persistent.len() < MAX_PERSISTENT {
                        self.persistent.push((command, ends));
                    } else {
                        self.dropped += 1;
                    }
                }
            }
            Lifetime::Frame if command.step != 0 => {
                // Compare ages against the most recently begun step, rather than a signed
                // distance from the last draw. This also handles an inactive gap beyond 2^31
                // steps. Commands must still be collected within one full counter cycle.
                let newer = self.held_step == 0
                    || step.wrapping_sub(command.step) < step.wrapping_sub(self.held_step);
                if newer {
                    self.held.clear();
                    self.held_step = command.step;
                    self.held_at = self.now;
                }
                if command.step == self.held_step {
                    if self.held.len() < MAX_HELD {
                        self.held.push(command);
                    } else {
                        self.dropped += 1;
                    }
                }
            }
            Lifetime::Frame => draw(&command, list),
        }
    }

    /// Ends what has ended and draws what lasts. `step` is the number of the fixed step begun last.
    fn finish(&mut self, step: u32, list: &mut DebugDrawList) {
        // A later step ran and drew nothing here, and the clock moved on: what it held is over. A
        // clock standing still is a paused simulation, whose last shapes stay to be looked at.
        if step != self.held_step && self.now > self.held_at {
            self.held.clear();
        }
        for command in &self.held {
            draw(command, list);
        }
        for (command, _) in &self.persistent {
            draw(command, list);
        }
        list.stats.held = self.held.len() as u32;
        list.stats.persistent = self.persistent.len() as u32;
    }
}

fn draw(command: &Command, list: &mut DebugDrawList) {
    match command.what {
        What::Shape(shape) => list.shape(shape, command.color, command.depth),
        What::Label { position, text } => list.label(position, command.color, text),
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::super::list::{LabelText, Shape};
    use super::super::{Color, Depth};
    use super::*;

    /// A line whose start says which call it was.
    fn command(id: f32, lifetime: Lifetime, step: u32) -> Command {
        Command {
            what: What::Shape(Shape::Line { from: Vec3::splat(id), to: Vec3::ZERO }),
            color: Color::RED,
            depth: Depth::Tested,
            lifetime,
            step,
            generation: 1,
        }
    }

    /// A collector and its clock, fed commands as the rings would give them, so these tests share
    /// no state with any other test's threads.
    struct Frames {
        collector: Collector,
        /// The step begun last.
        step: u32,
    }

    impl Frames {
        fn new() -> Self {
            Self { collector: Collector::new(), step: 0 }
        }

        /// One collect at `now` that finds `commands` waiting, as the ids of the lines it draws.
        fn frame(&mut self, now: f64, commands: &[Command]) -> Vec<f32> {
            let mut list = DebugDrawList::default();
            self.collector.set_clock(now);
            for &command in commands {
                self.collector.take(command, &mut list, self.step);
            }
            self.collector.finish(self.step, &mut list);
            list.lines.iter().map(|line| line.from.x).collect()
        }
    }

    #[test]
    fn a_frames_draw_lasts_that_frame() {
        let mut frames = Frames::new();
        assert_eq!(frames.frame(0.0, &[command(1.0, Lifetime::Frame, 0)]), [1.0]);
        assert_eq!(frames.frame(0.0, &[]), [0.0; 0]);
    }

    #[test]
    fn a_timed_draw_ends_on_the_clock_it_was_given_not_with_frames() {
        let mut frames = Frames::new();
        let timed = command(1.0, Lifetime::Seconds(0.5), 0);
        assert_eq!(frames.frame(10.0, &[timed]), [1.0]);
        // However many frames pass while the clock stands still: a paused world keeps it.
        for _ in 0..100 {
            assert_eq!(frames.frame(10.0, &[]), [1.0]);
        }
        assert_eq!(frames.frame(10.25, &[]), [1.0]);
        assert_eq!(frames.frame(10.499, &[]), [1.0]);
        // It ends exactly at its time, whatever the frame rate, slow motion included.
        assert_eq!(frames.frame(10.5, &[]), [0.0; 0]);
        assert_eq!(frames.collector.persistent.len(), 0);
    }

    #[test]
    fn a_clock_going_back_ends_what_was_timed_on_it() {
        let mut frames = Frames::new();
        assert_eq!(frames.frame(100.0, &[command(1.0, Lifetime::Seconds(50.0), 0)]), [1.0]);
        // A replay starts over: the shape would otherwise last 150 s.
        assert_eq!(frames.frame(0.0, &[]), [0.0; 0]);
    }

    #[test]
    fn a_full_store_counts_what_it_drops() {
        let mut frames = Frames::new();
        let timed = command(1.0, Lifetime::Seconds(1.0), 0);
        let mut list = DebugDrawList::default();
        for _ in 0..MAX_PERSISTENT + 3 {
            frames.collector.take(timed, &mut list, frames.step);
        }
        assert_eq!(frames.collector.persistent.len(), MAX_PERSISTENT);
        assert_eq!(frames.collector.dropped, 3);
        let step = command(2.0, Lifetime::Frame, 1);
        for _ in 0..MAX_HELD + 2 {
            frames.collector.take(step, &mut list, frames.step);
        }
        assert_eq!(frames.collector.held.len(), MAX_HELD);
        assert_eq!(frames.collector.dropped, 5);
    }

    #[test]
    fn expired_draws_release_capacity_before_accepting_replacements() {
        let mut frames = Frames::new();
        let commands = vec![command(1.0, Lifetime::Seconds(1.0), 0); MAX_PERSISTENT];
        frames.frame(0.0, &commands);
        assert_eq!(frames.frame(1.0, &[command(2.0, Lifetime::Seconds(1.0), 0)]), [2.0]);
        assert_eq!(frames.collector.dropped, 0);
        assert_eq!(frames.frame(1.5, &[]), [2.0]);
        assert!(frames.frame(2.0, &[]).is_empty());
    }

    #[test]
    fn a_new_draw_that_has_already_expired_is_not_shown() {
        let mut frames = Frames::new();
        for seconds in [0.0, -1.0, f32::NAN, f32::from_bits(1)] {
            assert!(frames.frame(100.0, &[command(1.0, Lifetime::Seconds(seconds), 0)]).is_empty());
        }
    }

    #[test]
    fn a_steps_draw_holds_through_frames_without_a_step() {
        let mut frames = Frames::new();
        frames.step = 1;
        assert_eq!(frames.frame(1.0, &[command(1.0, Lifetime::Frame, 1)]), [1.0]);
        // A 120 Hz display: frames with no step keep it, where one frame's draw would flicker.
        assert_eq!(frames.frame(1.0, &[]), [1.0]);
        assert_eq!(frames.frame(1.0, &[]), [1.0]);
    }

    #[test]
    fn the_next_steps_draw_replaces_it() {
        let mut frames = Frames::new();
        frames.step = 1;
        assert_eq!(frames.frame(1.0, &[command(1.0, Lifetime::Frame, 1)]), [1.0]);
        frames.step = 2;
        assert_eq!(frames.frame(1.1, &[command(2.0, Lifetime::Frame, 2)]), [2.0]);
    }

    #[test]
    fn two_steps_in_one_frame_show_the_later_only() {
        let mut frames = Frames::new();
        frames.step = 2;
        let both = [
            command(1.0, Lifetime::Frame, 1),
            command(1.5, Lifetime::Frame, 1),
            command(2.0, Lifetime::Frame, 2),
            command(2.5, Lifetime::Frame, 2),
        ];
        assert_eq!(frames.frame(1.0, &both), [2.0, 2.5]);
    }

    #[test]
    fn a_step_that_draws_nothing_ends_what_was_held_once_the_clock_moved() {
        let mut frames = Frames::new();
        frames.step = 1;
        assert_eq!(frames.frame(1.0, &[command(1.0, Lifetime::Frame, 1)]), [1.0]);
        // The hit volume ended: the next step runs and draws none.
        frames.step = 2;
        assert_eq!(frames.frame(1.1, &[]), [0.0; 0]);
    }

    #[test]
    fn a_paused_simulation_keeps_its_last_steps_shapes() {
        let mut frames = Frames::new();
        frames.step = 1;
        assert_eq!(frames.frame(1.0, &[command(1.0, Lifetime::Frame, 1)]), [1.0]);
        // Steps may run under a stopped clock, drawing nothing: the shapes stay until it moves.
        frames.step = 5;
        assert_eq!(frames.frame(1.0, &[]), [1.0]);
        frames.step = 9;
        assert_eq!(frames.frame(1.0, &[]), [1.0]);
        frames.step = 10;
        assert_eq!(frames.frame(1.0 + 1.0 / 60.0, &[]), [0.0; 0]);
    }

    #[test]
    fn a_step_under_a_stopped_clock_that_draws_again_replaces_rather_than_stacks() {
        let mut frames = Frames::new();
        for step in 1..10 {
            frames.step = step;
            let drawn = frames.frame(1.0, &[command(step as f32, Lifetime::Frame, step)]);
            assert_eq!(drawn, [step as f32], "one copy, in a pause as in play");
        }
    }

    #[test]
    fn step_numbers_wrap() {
        let mut frames = Frames::new();
        frames.collector.held_step = u32::MAX;
        frames.step = 1;
        assert_eq!(frames.frame(1.0, &[command(1.0, Lifetime::Frame, 1)]), [1.0]);
        assert_eq!(frames.collector.held_step, 1);
    }

    #[test]
    fn a_long_inactive_gap_does_not_reject_new_step_draws() {
        let mut frames = Frames::new();
        frames.step = 1;
        assert_eq!(frames.frame(0.0, &[command(1.0, Lifetime::Frame, 1)]), [1.0]);
        frames.step = (1 << 31) + 2;
        let step = frames.step;
        assert_eq!(frames.frame(1.0, &[command(2.0, Lifetime::Frame, step)]), [2.0]);
        assert_eq!(frames.frame(1.0, &[]), [2.0]);
    }

    #[test]
    fn a_timed_draw_in_a_step_is_timed_and_not_held() {
        let mut frames = Frames::new();
        frames.step = 1;
        assert_eq!(frames.frame(1.0, &[command(1.0, Lifetime::Seconds(1.0), 1)]), [1.0]);
        frames.step = 2;
        assert_eq!(frames.frame(1.5, &[]), [1.0]);
        assert_eq!(frames.frame(2.0, &[]), [0.0; 0]);
    }

    #[test]
    fn labels_last_as_shapes_do() {
        let mut frames = Frames::new();
        let label = Command {
            what: What::Label { position: Vec3::ONE, text: LabelText::format(format_args!("hit")) },
            ..command(0.0, Lifetime::Seconds(1.0), 0)
        };
        let mut list = DebugDrawList::default();
        frames.collector.now = 3.0;
        frames.collector.take(label, &mut list, frames.step);
        frames.collector.finish(0, &mut list);
        assert_eq!(list.labels.len(), 1);
        assert_eq!(list.labels[0].text.as_str(), "hit");
        assert_eq!(list.stats.persistent, 1);
    }
}
