//! Each thread's buffer of commands: a fixed ring with one writer, the thread, and one reader at a
//! time, the collector, so a draw takes no lock and never allocates once the ring exists.

use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering, fence};
use std::sync::{Arc, Mutex, PoisonError};

use glam::Vec3;

use super::list::{LabelText, Shape};
use super::{Color, Depth, Lifetime, Style};

/// The calls a thread may have waiting for the next collect; more are dropped and counted. A
/// power of two (a ring's index is a mask).
pub const THREAD_BUFFER: usize = 1 << 14;

/// One call, as plain data.
#[derive(Clone, Copy, Debug)]
pub(super) struct Command {
    pub what: What,
    pub color: Color,
    pub depth: Depth,
    pub lifetime: Lifetime,
    /// The fixed step it was drawn in ([`begin_step`]), or 0 outside one.
    pub step: u32,
}

/// What a call draws.
#[derive(Clone, Copy, Debug)]
pub(super) enum What {
    Shape(Shape),
    Label { position: Vec3, text: LabelText },
}

/// A fixed ring of commands, one writer and one reader at a time.
///
/// `head` and `tail` count commands written and read, wrapping; a slot's index is the count masked
/// by the capacity, and the slots from `tail` to `head` are written and not yet read. Only the
/// writer moves `head` and only the reader `tail`, so neither waits on the other: a release store
/// of `head` publishes the slots behind it to the reader, and a release store of `tail` hands them
/// back to the writer.
struct Ring {
    slots: Box<[UnsafeCell<MaybeUninit<Command>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    /// Calls lost to a full ring since the reader last took the count.
    dropped: AtomicU32,
}

// SAFETY: the slots are the only part that isn't `Sync`, and each is touched by one side at a
// time. The writer (`push`, only ever called by the thread that owns the ring's `Local`) writes
// slots outside `tail..head` and publishes them with its release store of `head`; the reader
// (`drain`, one at a time under `RINGS`' lock) reads slots inside `tail..head`, which its acquire
// load of `head` made visible, and returns them with its release store of `tail`. `Command` is
// `Copy`, so no slot ever needs dropping.
unsafe impl Sync for Ring {}

impl Ring {
    fn new(capacity: usize) -> Self {
        debug_assert!(capacity.is_power_of_two());
        Self {
            slots: (0..capacity).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU32::new(0),
        }
    }

    /// The writer's side. Wait-free; a full ring counts the command as dropped.
    fn push(&self, command: Command) {
        let head = self.head.load(Ordering::Relaxed);
        // Acquire pairs with `drain`'s release: slots it has handed back are free to overwrite.
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) == self.slots.len() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let slot = &self.slots[head & (self.slots.len() - 1)];
        // SAFETY: this slot is outside `tail..head`, so the reader won't touch it until the store
        // below publishes it, and this thread is the only writer (see the `Sync` impl).
        unsafe { (*slot.get()).write(command) };
        self.head.store(head.wrapping_add(1), Ordering::Release);
    }

    /// The reader's side: gives `take` every command published so far, oldest first. Only one
    /// reader may run at a time, which `RINGS`' lock assures.
    fn drain(&self, take: &mut dyn FnMut(Command)) {
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        while tail != head {
            let slot = &self.slots[tail & (self.slots.len() - 1)];
            // SAFETY: this slot is inside `tail..head`: the writer initialized it before its
            // release store of `head`, which the acquire load above saw, and won't touch it until
            // the store below hands it back.
            take(unsafe { (*slot.get()).assume_init() });
            tail = tail.wrapping_add(1);
        }
        self.tail.store(tail, Ordering::Release);
    }
}

/// Every thread's ring since it first drew. The hot path never takes this lock: a thread takes it
/// once, to register, and the collector for each drain, which keeps its rings one reader at a time.
static RINGS: Mutex<Vec<Arc<Ring>>> = Mutex::new(Vec::new());

/// Fixed steps begun, numbering their draws. 0 means outside a step, so it starts from 1.
static STEP: AtomicU32 = AtomicU32::new(0);

/// Calls made as their thread was ending, after its ring was gone.
static LATE: AtomicU32 = AtomicU32::new(0);

/// A thread's way to its ring.
struct Local {
    ring: Arc<Ring>,
    /// The step this thread is running, or 0.
    step: Cell<u32>,
}

impl Local {
    fn new() -> Self {
        let ring = Arc::new(Ring::new(THREAD_BUFFER));
        RINGS.lock().unwrap_or_else(PoisonError::into_inner).push(Arc::clone(&ring));
        Self { ring, step: Cell::new(0) }
    }
}

thread_local! {
    static LOCAL: Local = Local::new();
}

/// Runs `with` on this thread's ring state, or counts the call as dropped if the thread is ending
/// and its ring is gone.
fn local(with: impl FnOnce(&Local)) {
    if LOCAL.try_with(with).is_err() {
        LATE.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn warm() {
    local(|_| {});
}

pub(super) fn begin_step() {
    let step = STEP.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    local(|local| local.step.set(step));
}

pub(super) fn end_step() {
    local(|local| local.step.set(0));
}

/// The number of the step begun last.
pub(super) fn current_step() -> u32 {
    STEP.load(Ordering::Relaxed)
}

fn write(what: What, style: Style) {
    local(|local| {
        let Style { color, depth, lifetime } = style;
        local.ring.push(Command { what, color, depth, lifetime, step: local.step.get() });
    });
}

/// Out of line, so a disabled call is only its load and branch where it is called from.
#[cold]
#[inline(never)]
pub(super) fn write_shape(shape: Shape, style: Style) {
    write(What::Shape(shape), style);
}

/// Out of line, as [`write_shape`] is.
#[cold]
#[inline(never)]
pub(super) fn write_label(position: Vec3, style: Style, text: fmt::Arguments<'_>) {
    write(What::Label { position, text: LabelText::format(text) }, style);
}

/// Hands every thread's waiting commands to `take`, each thread's in order, and returns how many
/// calls were lost since the last time. Rings whose threads have ended go once they are empty.
pub(super) fn drain_all(take: &mut dyn FnMut(Command)) -> u32 {
    let mut rings = RINGS.lock().unwrap_or_else(PoisonError::into_inner);
    let mut dropped = LATE.swap(0, Ordering::Relaxed);
    rings.retain(|ring| {
        // Asked before draining: only this list holds the ring, so its thread has ended. The fence
        // pairs with the release of that thread's last reference, so the drain sees all it wrote.
        let ended = Arc::strong_count(ring) == 1;
        if ended {
            fence(Ordering::Acquire);
        }
        ring.drain(take);
        dropped = dropped.saturating_add(ring.dropped.swap(0, Ordering::Relaxed));
        !ended
    });
    dropped
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn command(index: u32) -> Command {
        let (from, to) = (Vec3::splat(index as f32), Vec3::ZERO);
        Command {
            what: What::Shape(Shape::Line { from, to }),
            color: Color::RED,
            depth: Depth::Tested,
            lifetime: Lifetime::Frame,
            step: index,
        }
    }

    fn drained(ring: &Ring) -> Vec<u32> {
        let mut steps = Vec::new();
        ring.drain(&mut |command| steps.push(command.step));
        steps
    }

    #[test]
    fn a_command_is_small_enough_for_a_thread_to_buffer_thousands() {
        assert!(size_of::<Command>() <= 96, "{}", size_of::<Command>());
    }

    #[test]
    fn a_ring_gives_back_what_was_pushed_in_order() {
        let ring = Ring::new(8);
        assert_eq!(drained(&ring), []);
        for index in 0..5 {
            ring.push(command(index));
        }
        assert_eq!(drained(&ring), [0, 1, 2, 3, 4]);
        assert_eq!(drained(&ring), []);
        // Across the end of its slots.
        for index in 5..11 {
            ring.push(command(index));
        }
        assert_eq!(drained(&ring), [5, 6, 7, 8, 9, 10]);
        assert_eq!(ring.dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_full_ring_counts_what_it_drops_and_keeps_what_fits() {
        let ring = Ring::new(4);
        for index in 0..7 {
            ring.push(command(index));
        }
        assert_eq!(ring.dropped.load(Ordering::Relaxed), 3);
        assert_eq!(drained(&ring), [0, 1, 2, 3]);
        // It has room again.
        ring.push(command(9));
        assert_eq!(drained(&ring), [9]);
    }

    #[test]
    fn a_reader_racing_the_writer_sees_every_command_once_in_order() {
        const COMMANDS: u32 = 200_000;
        let ring = Ring::new(64);
        let done = AtomicBool::new(false);
        let mut seen = Vec::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for index in 0..COMMANDS {
                    // Waits for room rather than drop, so every command must arrive.
                    while ring.head.load(Ordering::Relaxed).wrapping_sub(ring.tail.load(Ordering::Acquire))
                        == ring.slots.len()
                    {
                        std::hint::spin_loop();
                    }
                    ring.push(command(index));
                }
                done.store(true, Ordering::Release);
            });
            loop {
                let finished = done.load(Ordering::Acquire);
                ring.drain(&mut |command| seen.push(command.step));
                if finished {
                    break;
                }
            }
        });
        assert_eq!(seen.len(), COMMANDS as usize);
        assert!(seen.iter().enumerate().all(|(index, &step)| step == index as u32));
    }
}
