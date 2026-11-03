//! Giving back what the GPU may still be reading (M3 W3): when a freed mesh's ranges and a freed
//! material's slot go back to their free lists.
//!
//! A free goes through three stages, and only the last of them changes what a new upload can get:
//!
//! 1. **Dying:** asked for ([`crate::Renderer::free_mesh`], [`crate::Renderer::free_material`]), while
//!    an instance still draws the mesh or a mesh still draws the material. It works as it did, for as
//!    long as that lasts.
//! 2. **Retired:** nothing the frame being prepared draws names it, so its handle stops being current,
//!    at once, and the frames that may read it are that frame's predecessors. It waits here with the
//!    number of the last of them.
//! 3. **Released:** the GPU has finished that frame, [`FrameFence`] says so, and the next
//!    [`crate::Renderer::prepare`] hands the room back.
//!
//! wgpu orders a queue write after every submission before it, so a range written too early wouldn't
//! tear a frame already submitted. The wait is what makes that not matter: a frame prepared and not
//! yet submitted, a pass that reads a pool buffer across frames, or a backend that orders uploads
//! differently (the staging ring the crate docs plan) all stay safe, because the room isn't
//! anyone's until no frame can be reading it.
//!
//! # Handles
//!
//! A mesh's or material's handle carries a generation, so one freed never names what's later given
//! its index. Generations start at a number of their renderer's own ([`new_epoch`]), so a handle from
//! a renderer lost to the device names nothing on the next one, and freeing it there frees nothing.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use sapphire_rhi::wgpu;

/// What one renderer's epochs step by: the frees of one index before a generation runs into the next
/// renderer's. A handle would have to survive that many frees of its index to match by accident.
const EPOCH_STEP: u32 = 1 << 20;

static NEXT_EPOCH: AtomicU32 = AtomicU32::new(EPOCH_STEP);

/// The first generation of a new pool's handles: never 0, which `from_raw`'s handles have.
pub(crate) fn new_epoch() -> u32 {
    NEXT_EPOCH.fetch_add(EPOCH_STEP, Ordering::Relaxed)
}

/// Which of the renderer's frames the GPU has finished. Frames are numbered from 1 by
/// [`Self::begin_frame`], and a frame is submitted, or lost, before the next begins.
#[derive(Debug, Default)]
pub(crate) struct FrameFence {
    /// The frame being prepared or recorded.
    frame: u64,
    /// The last frame known to be finished: the queue's callback writes it.
    finished: Arc<AtomicU64>,
    /// The last frame the queue was asked about.
    asked: u64,
    /// Frames past this one don't count as finished, for tests of what a slow GPU would do.
    #[cfg(test)]
    held_at: Option<u64>,
}

impl FrameFence {
    /// Starts the next frame. On a `lost` device nothing will ever finish, and nothing is being read,
    /// so everything counts as finished: the queue may never answer, and what's retired mustn't wait
    /// on it.
    pub(crate) fn begin_frame(&mut self, lost: bool) {
        self.frame += 1;
        if lost {
            self.finished.store(u64::MAX, Ordering::Release);
        }
    }

    /// The frame being prepared or recorded.
    pub(crate) fn frame(&self) -> u64 {
        self.frame
    }

    /// The frame before it, already submitted or lost: the last to read what this frame's draw list
    /// doesn't name.
    pub(crate) fn last_frame(&self) -> u64 {
        self.frame.saturating_sub(1)
    }

    /// Has the queue say when the last frame is finished. Call it only with something retired: it
    /// costs a callback, and the queue answers on a later submission or poll.
    pub(crate) fn ask(&mut self, queue: &wgpu::Queue) {
        let frame = self.last_frame();
        if frame == self.asked || frame == 0 {
            return;
        }
        self.asked = frame;
        let finished = Arc::clone(&self.finished);
        queue.on_submitted_work_done(move || {
            finished.fetch_max(frame, Ordering::AcqRel);
        });
    }

    /// The last frame the GPU has finished.
    pub(crate) fn finished(&self) -> u64 {
        let finished = self.finished.load(Ordering::Acquire);
        #[cfg(test)]
        if let Some(held) = self.held_at {
            return finished.min(held);
        }
        finished
    }

    /// Counts no frame past `frame` as finished, or with `None` goes back to the queue's word, for
    /// tests.
    #[cfg(test)]
    pub(crate) fn hold_at(&mut self, frame: Option<u64>) {
        self.held_at = frame;
    }
}

/// What's retired, oldest first, with the last frame that may read each.
#[derive(Debug)]
pub(crate) struct Retiring<T>(VecDeque<(u64, T)>);

impl<T> Default for Retiring<T> {
    fn default() -> Self {
        Self(VecDeque::new())
    }
}

impl<T> Retiring<T> {
    /// Retires `item`, which frame `last_read` may read. Frames are retired in order.
    pub(crate) fn push(&mut self, last_read: u64, item: T) {
        debug_assert!(self.0.back().is_none_or(|(frame, _)| *frame <= last_read), "retired out of order");
        self.0.push_back((last_read, item));
    }

    /// The oldest item no unfinished frame may read.
    pub(crate) fn pop_finished(&mut self, finished: u64) -> Option<T> {
        if self.0.front()?.0 > finished {
            return None;
        }
        self.0.pop_front().map(|(_, item)| item)
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_leaves_before_the_last_frame_that_reads_it_is_finished() {
        let mut retiring = Retiring::default();
        retiring.push(3, "a");
        retiring.push(3, "b");
        retiring.push(5, "c");
        assert_eq!(retiring.pop_finished(2), None);
        assert_eq!(retiring.pop_finished(3), Some("a"));
        assert_eq!(retiring.pop_finished(3), Some("b"));
        assert_eq!(retiring.pop_finished(4), None, "c is still read by frame 5");
        assert_eq!(retiring.pop_finished(5), Some("c"));
        assert_eq!(retiring.len(), 0);
    }

    #[test]
    fn a_lost_device_finishes_everything_and_a_held_fence_nothing_more() {
        let mut fence = FrameFence::default();
        fence.begin_frame(false);
        assert_eq!((fence.frame(), fence.last_frame(), fence.finished()), (1, 0, 0));
        fence.hold_at(Some(0));
        fence.begin_frame(true);
        assert_eq!(fence.finished(), 0, "held");
        fence.hold_at(None);
        assert_eq!(fence.finished(), u64::MAX);
    }

    #[test]
    fn no_two_renderers_start_at_one_generation_nor_at_from_raws() {
        let (a, b) = (new_epoch(), new_epoch());
        assert_ne!(a, b);
        assert_ne!(a, 0);
        assert_ne!(b, 0);
    }
}
