//! Whether the window has a GPU to draw with, and when to try for one: the part of a lost device that needs
//! no device, so the window's choices can be tested without a window.
//!
//! An editor with no GPU keeps running, because it's holding someone's edits. Nothing here gives up: a
//! driver reset or a laptop's GPU switch can pass at any time. Tries wait longer and longer, though, since
//! making a GPU stack can hold the interface's thread for a good part of a second, and a frame drawn is the
//! only thing that resets the wait, so a stack that's made and then lost at once is paced like one that
//! can't be made at all.

use std::fmt::Display;

/// The wait after the first setback, in seconds, which doubles with each one after.
const FIRST_WAIT: f64 = 0.25;
const LONGEST_WAIT: f64 = 5.0;

/// A window's GPU stack, if it has one.
#[derive(Debug)]
pub struct Recovery<D> {
    device: Option<D>,
    /// When the next try may be made, on the caller's clock in seconds.
    next_try: f64,
    /// How long the next setback waits. Nothing has gone wrong since a frame was drawn while it's 0.
    wait: f64,
}

impl<D> Recovery<D> {
    #[must_use]
    pub fn new(device: D) -> Self {
        Self { device: Some(device), next_try: 0.0, wait: 0.0 }
    }

    /// The stack, if there is one, with no try for another.
    pub fn ready(&mut self) -> Option<&mut D> {
        self.device.as_mut()
    }

    /// The stack, making one with `make` if there's none and it's time to try.
    pub fn device<E: Display>(&mut self, now: f64, make: impl FnOnce() -> Result<D, E>) -> Option<&mut D> {
        if self.device.is_none() && now >= self.next_try {
            match make() {
                Ok(device) => {
                    tracing::info!("the window has a GPU device again");
                    self.device = Some(device);
                }
                Err(error) => {
                    if self.wait <= FIRST_WAIT {
                        tracing::error!(%error, "the GPU device was lost, and no new one came: trying again");
                    }
                    self.set_back(now);
                }
            }
        }
        self.device.as_mut()
    }

    /// The stack is lost: it goes, and the next try comes at once, unless this keeps happening.
    pub fn lost(&mut self, now: f64) {
        self.device = None;
        self.set_back(now);
    }

    /// A frame reached the screen, so whatever was wrong is over.
    pub fn drew(&mut self) {
        self.wait = 0.0;
    }

    fn set_back(&mut self, now: f64) {
        self.next_try = now + self.wait;
        self.wait = (self.wait * 2.0).clamp(FIRST_WAIT, LONGEST_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    /// A device that counts how many of it there are, for what a test leaves behind.
    #[derive(Debug)]
    struct Stack(Rc<Cell<i32>>);

    impl Stack {
        fn new(alive: &Rc<Cell<i32>>) -> Self {
            alive.set(alive.get() + 1);
            Self(Rc::clone(alive))
        }
    }

    impl Drop for Stack {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    const FRAME: f64 = 1.0 / 60.0;

    /// A setback's first try fails and the next works: frames resume, whatever's in the way passes.
    #[test]
    fn a_first_try_that_fails_is_tried_again() {
        let alive = Rc::new(Cell::new(0));
        let mut gpu = Recovery::new(Stack::new(&alive));
        gpu.lost(10.0);
        assert_eq!(alive.get(), 0, "the lost one's gone");

        let tries = Cell::new(0);
        let make = || {
            tries.set(tries.get() + 1);
            if tries.get() == 1 { Err("not yet") } else { Ok(Stack::new(&alive)) }
        };
        assert!(gpu.device(10.0, make).is_none(), "tried at once, and it failed");
        assert_eq!(tries.get(), 1);
        assert!(gpu.device(10.0 + FRAME, make).is_none(), "not the very next frame");
        assert_eq!(tries.get(), 1);
        assert!(gpu.device(10.0 + FIRST_WAIT, make).is_some(), "but soon");
        assert_eq!(tries.get(), 2);
        assert_eq!(alive.get(), 1);
        assert!(gpu.ready().is_some());
    }

    /// A GPU that never comes back costs a few tries a minute, never a try a frame, and it's never given up.
    #[test]
    fn a_gpu_that_stays_gone_is_tried_at_a_pace() {
        let mut gpu: Recovery<u32> = Recovery::new(0);
        gpu.lost(0.0);
        let tries = Cell::new(0_u32);
        let mut times = Vec::new();
        let frames = (600.0 / FRAME) as u32;
        for frame in 0..frames {
            let now = f64::from(frame) * FRAME;
            let before = tries.get();
            let made = gpu.device(now, || {
                tries.set(tries.get() + 1);
                Err::<u32, _>("no GPU")
            });
            assert!(made.is_none());
            if tries.get() != before {
                times.push(now);
            }
        }
        assert!((120..=130).contains(&tries.get()), "{} tries in ten minutes", tries.get());
        let gaps: Vec<f64> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(gaps[0] >= FIRST_WAIT, "{gaps:?}");
        assert!(gaps.windows(2).all(|pair| pair[1] >= pair[0] - FRAME), "the wait only grows: {gaps:?}");
        assert!(gaps.iter().all(|gap| *gap <= LONGEST_WAIT + 2.0 * FRAME), "and has a limit: {gaps:?}");
    }

    /// A stack that's made and lost before it draws anything is paced too, with no stack left behind.
    #[test]
    fn a_stack_lost_as_soon_as_its_made_is_paced() {
        let alive = Rc::new(Cell::new(0));
        let mut gpu = Recovery::new(Stack::new(&alive));
        let made = Cell::new(0_u32);
        let frames = (60.0 / FRAME) as u32;
        for frame in 0..frames {
            let now = f64::from(frame) * FRAME;
            let device = gpu.device(now, || {
                made.set(made.get() + 1);
                Ok::<_, &str>(Stack::new(&alive))
            });
            if device.is_some() {
                gpu.lost(now);
            }
            assert_eq!(alive.get(), 0);
        }
        assert!(made.get() <= 16, "{} stacks made in a minute", made.get());
    }

    /// A frame drawn ends it: the next loss is tried for at once again, however bad the last spell was.
    #[test]
    fn a_frame_drawn_starts_the_waits_over() {
        let mut gpu = Recovery::new(0_u32);
        gpu.lost(0.0);
        for now in [0.0, 1.0, 2.0, 4.0, 8.0] {
            assert!(gpu.device(now, || Err("no GPU")).is_none());
        }
        assert!(gpu.device(100.0, || Ok::<_, &str>(1)).is_some());
        gpu.drew();
        gpu.lost(200.0);
        assert!(gpu.device(200.0, || Ok::<_, &str>(2)).is_some(), "at once");
    }
}
