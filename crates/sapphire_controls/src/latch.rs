//! [`PressLatch`]: turns a frame's presses into what each fixed step reads, so that a press is a
//! rising edge in the step sequence exactly once, however frames and steps line up.

use sapphire_gameplay::Buttons;

use crate::controls::BUTTON_ACTIONS;

/// Most presses of one button waiting for a step. Each after the first waits two more ticks, one
/// to let go and one to rise, so the last of these rises 6 ticks after the first: within G2's
/// shortest intent ring (8 ticks), and past anything a thumb does outside a hitch.
pub(crate) const MAX_QUEUED_PRESSES: u8 = 4;

/// What the steps have shown of the buttons, and what they haven't yet.
///
/// A step can only say "held" or "not held", and the simulation finds presses by comparing one
/// tick with the last (`sapphire_gameplay::Controls::pressed`). So to show a press is to rise from
/// a step that read "not held":
///
/// - **Up, with a press waiting:** rises, which uses that press up.
/// - **Down, with a press waiting:** the press it shows has been let go, or there couldn't be
///   another. It falls for one step, so the next press has an edge of its own.
/// - **Otherwise** it reads the button as last sampled. A press that's still held stays up, and one
///   that has ended falls.
///
/// A tap inside one frame, or between steps, is therefore one step up and then down; two taps in
/// one frame are up, down, up, down. A press held through several steps a frame stays up, one edge.
/// A frame with no step leaves everything waiting.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PressLatch {
    /// Presses each button hasn't shown yet, in [`BUTTON_ACTIONS`]' order.
    waiting: [u8; BUTTON_ACTIONS.len()],
    /// What the latest step showed.
    shown: Buttons,
    /// What the latest sample found held.
    held: Buttons,
    presses: u64,
    over_limit: u64,
}

impl PressLatch {
    /// A press of the button at `index` in [`BUTTON_ACTIONS`].
    pub fn press(&mut self, index: usize) {
        self.presses += 1;
        if self.waiting[index] < MAX_QUEUED_PRESSES {
            self.waiting[index] += 1;
        } else {
            self.over_limit += 1;
        }
    }

    /// The frame's end: what's held now.
    pub fn sample(&mut self, held: Buttons) {
        self.held = held;
    }

    /// What the next step reads.
    pub fn next_step(&mut self) -> Buttons {
        let mut shown = Buttons::NONE;
        for (index, (button, _)) in BUTTON_ACTIONS.iter().enumerate() {
            let down = if self.shown.contains(*button) {
                self.waiting[index] == 0 && self.held.contains(*button)
            } else if self.waiting[index] > 0 {
                self.waiting[index] -= 1;
                true
            } else {
                false
            };
            if down {
                shown = shown | *button;
            }
        }
        self.shown = shown;
        shown
    }

    /// Presses counted so far.
    pub fn presses(&self) -> u64 {
        self.presses
    }

    /// Presses dropped because [`MAX_QUEUED_PRESSES`] were already waiting.
    pub fn over_limit(&self) -> u64 {
        self.over_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: Buttons = Buttons::LIGHT;
    const N: Buttons = Buttons::NONE;

    /// `presses` of the first button, then `held` sampled, then `steps` steps.
    fn frame(latch: &mut PressLatch, presses: u32, held: Buttons, steps: usize) -> Vec<Buttons> {
        for _ in 0..presses {
            latch.press(0);
        }
        latch.sample(held);
        (0..steps).map(|_| latch.next_step()).collect()
    }

    #[test]
    fn the_first_button_is_the_one_these_tests_press() {
        assert_eq!(BUTTON_ACTIONS[0].0, L);
    }

    #[test]
    fn a_press_rises_once_whatever_the_steps_do() {
        let mut latch = PressLatch::default();
        assert_eq!(frame(&mut latch, 1, N, 0), [], "a frame with no step keeps it");
        assert_eq!(frame(&mut latch, 0, N, 2), [L, N], "a tap, at two steps a frame");

        let mut latch = PressLatch::default();
        assert_eq!(frame(&mut latch, 1, L, 2), [L, L], "held through two steps");
        assert_eq!(frame(&mut latch, 0, L, 1), [L]);
        assert_eq!(frame(&mut latch, 0, N, 0), []);
        assert_eq!(frame(&mut latch, 0, N, 1), [N], "let go in a frame with no step");
    }

    #[test]
    fn a_second_press_gets_an_edge_of_its_own() {
        let mut latch = PressLatch::default();
        assert_eq!(frame(&mut latch, 2, L, 4), [L, N, L, L], "tap, then hold, in one frame");
        assert_eq!(frame(&mut latch, 1, L, 3), [N, L, L], "let go and pressed again between steps");
    }

    #[test]
    fn presses_past_the_limit_are_counted_and_the_last_still_holds() {
        let mut latch = PressLatch::default();
        let presses = u32::from(MAX_QUEUED_PRESSES) + 2;
        let steps = frame(&mut latch, presses, L, 8);
        assert_eq!(steps, [L, N, L, N, L, N, L, L], "the last that fit stays up for the one still held");
        assert_eq!((latch.presses(), latch.over_limit()), (u64::from(presses), 2));
    }
}
