//! Debug draw through its real path, under the counting allocator: the disabled path's cost, a
//! fixed step's rule R6 and merging many threads. Drawing is one switch and one set of buffers per
//! process, and only these tests touch it, one at a time.

use std::alloc::System;
use std::hint::black_box;
use std::sync::{Mutex, MutexGuard, PoisonError};

use glam::{Quat, Vec3};
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_core::debug_draw::{
    self as dd, Collector, Color, DebugDrawList, DebugLine, Depth, MAX_LINES, Shape, Style, THREAD_BUFFER,
};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Runs with drawing as `on`, nothing waiting from an earlier test, and turns it off after.
struct Drawing {
    collector: Collector,
    list: DebugDrawList,
    _turn: MutexGuard<'static, ()>,
}

impl Drawing {
    fn new(on: bool) -> Self {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut collector, mut list) = (Collector::new(), DebugDrawList::default());
        dd::set_enabled(true);
        collector.collect(0.0, &mut list);
        dd::set_enabled(on);
        Self { collector: Collector::new(), list, _turn }
    }

    fn collect(&mut self, now: f64) -> &DebugDrawList {
        self.collector.collect(now, &mut self.list);
        &self.list
    }
}

impl Drop for Drawing {
    fn drop(&mut self) {
        dd::set_enabled(false);
    }
}

/// Every call in the API, for tests that care about all of them, with values `black_box` keeps
/// from being folded away.
fn draw_everything(index: u32) {
    let at = black_box(Vec3::new(index as f32, 1.0, 2.0));
    let rotation = black_box(Quat::IDENTITY);
    let style = Style::new(Color::CYAN).on_top();
    dd::line(at, Vec3::ZERO, Color::RED);
    dd::arrow(at, Vec3::ONE, style);
    dd::aabb(at, at + Vec3::ONE, Color::GREEN);
    dd::oriented_box(at, Vec3::ONE, rotation, style);
    dd::sphere(at, 1.0, Color::YELLOW);
    dd::capsule(at, Vec3::ZERO, 0.5, style);
    dd::circle(at, Vec3::Y, 1.0, Color::ORANGE);
    dd::axes(at, rotation, 1.0, Color::WHITE);
    dd::label(at, Color::WHITE, format_args!("call {index} at {:.2}", at.x));
}

/// `draw_everything`'s lines and shapes.
const EVERYTHING_LINES: usize = 1 + 5 + 12 + 12 + 96 + 132 + 32 + 3;
const EVERYTHING_SHAPES: u32 = 8;

#[test]
fn disabled_calls_allocate_nothing_and_draw_nothing() {
    assert!(alloc::is_installed());
    let mut drawing = Drawing::new(false);
    assert!(!dd::enabled());
    let ((), counts) = alloc::count_in_scope(|| {
        for index in 0..10_000 {
            dd::begin_step();
            draw_everything(index);
            dd::end_step();
        }
    });
    assert_eq!(counts.allocations, 0, "{counts:?}");
    let (list, counts) = alloc::count_in_scope(|| drawing.collect(1.0).clone());
    assert_eq!(counts.allocations, 0, "an empty list's clone doesn't allocate either: {counts:?}");
    assert_eq!(list, DebugDrawList::default());
}

#[test]
fn a_disabled_label_isnt_formatted() {
    struct Panics;
    impl std::fmt::Display for Panics {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("a label formatted while drawing is off");
        }
    }
    let _drawing = Drawing::new(false);
    dd::label(Vec3::ZERO, Color::WHITE, format_args!("{}", Panics));
}

#[test]
fn a_warmed_thread_draws_inside_a_fixed_step_without_allocating() {
    let mut drawing = Drawing::new(true);
    dd::warm_thread();
    let ((), counts) = alloc::count_in_scope(|| {
        for index in 0..100 {
            dd::begin_step();
            draw_everything(index);
            dd::end_step();
        }
    });
    assert_eq!(counts.allocations, 0, "rule R6, every kind of call: {counts:?}");
    // The last step's alone draw, as a frame's two steps would.
    let list = drawing.collect(1.0);
    assert_eq!(list.lines.len(), EVERYTHING_LINES);
    assert_eq!(list.stats.shapes, EVERYTHING_SHAPES);
    assert_eq!(list.stats.labels, 1);
    assert_eq!(list.labels[0].text.as_str(), "call 99 at 99.00");
    assert_eq!(list.stats.held, EVERYTHING_SHAPES + 1);
}

#[test]
fn only_a_threads_first_call_allocates_and_warming_it_takes_that_away() {
    let _drawing = Drawing::new(true);
    let draw = || alloc::count_in_scope(|| draw_everything(0)).1.allocations;
    std::thread::scope(|scope| {
        let cold = scope.spawn(|| (draw(), draw()));
        let (first, second) = cold.join().unwrap();
        assert!(first > 0, "a new thread makes its ring");
        assert_eq!(second, 0);
        let warmed = scope.spawn(|| {
            dd::warm_thread();
            draw()
        });
        assert_eq!(warmed.join().unwrap(), 0);
    });
}

#[test]
fn a_collect_allocates_nothing_once_warm() {
    let mut drawing = Drawing::new(true);
    let mut frame = |index| {
        draw_everything(index);
        dd::sphere(Vec3::ZERO, 1.0, Style::new(Color::RED).for_seconds(1000.0));
        drawing.collect(f64::from(index)).lines.len()
    };
    let warm = frame(0);
    assert!(warm > 0);
    // Room for the timed spheres this adds, one a frame.
    for index in 1..200 {
        frame(index);
    }
    let (lines, counts) = alloc::count_in_scope(|| frame(200));
    assert_eq!(lines, EVERYTHING_LINES + 201 * 96);
    assert_eq!(counts.allocations, 0, "{counts:?}");
}

#[test]
fn many_threads_merge_each_in_order() {
    const THREADS: u32 = 16;
    const LINES: u32 = 2_000;
    let mut drawing = Drawing::new(true);
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            scope.spawn(move || {
                for index in 0..LINES {
                    // Says which thread and which call, in its coordinates.
                    let from = Vec3::new(thread as f32, index as f32, 0.0);
                    dd::line(from, Vec3::ZERO, Color::rgb(thread as u8, 0, 0));
                }
            });
        }
    });
    let list = drawing.collect(0.0);
    assert_eq!(list.stats.dropped, 0);
    assert_eq!(list.lines.len(), (THREADS * LINES) as usize);
    assert_eq!(list.stats.shapes, THREADS * LINES);
    // Threads' calls stay together and in order, whichever thread came first.
    let mut seen = vec![0_u32; THREADS as usize];
    for line in &list.lines {
        let thread = line.from.x as usize;
        assert_eq!(line.color.r as usize, thread);
        assert_eq!(line.from.y as u32, seen[thread], "thread {thread}'s calls in order");
        seen[thread] += 1;
    }
    assert!(seen.iter().all(|&count| count == LINES));
}

#[test]
fn threads_keep_drawing_while_the_collector_merges() {
    const THREADS: u32 = 8;
    const LINES: u32 = 40_000;
    let mut drawing = Drawing::new(true);
    // However the threads run, a full ring may drop calls, but each thread's arrive in order, each
    // once, and every call either arrives or is counted.
    let mut next = vec![0_u32; THREADS as usize];
    let (mut arrived, mut dropped) = (0_u32, 0_u32);
    let mut merge = |list: &DebugDrawList| {
        for line in &list.lines {
            let (thread, index) = (line.from.x as usize, line.from.y as u32);
            assert!(index >= next[thread], "thread {thread}'s call {index} after {}", next[thread]);
            next[thread] = index + 1;
            arrived += 1;
        }
        dropped += list.stats.dropped;
    };
    std::thread::scope(|scope| {
        let writers: Vec<_> = (0..THREADS)
            .map(|thread| {
                scope.spawn(move || {
                    for index in 0..LINES {
                        dd::line(Vec3::new(thread as f32, index as f32, 0.0), Vec3::ZERO, Color::RED);
                    }
                })
            })
            .collect();
        while !writers.iter().all(std::thread::ScopedJoinHandle::is_finished) {
            merge(drawing.collect(0.0));
        }
    });
    merge(drawing.collect(0.0));
    assert_eq!(arrived + dropped, THREADS * LINES);
}

#[test]
fn a_threads_draws_outlive_it_and_its_buffer_goes() {
    let mut drawing = Drawing::new(true);
    let thread = || std::thread::spawn(|| dd::line(Vec3::ONE, Vec3::ZERO, Color::RED)).join().unwrap();
    thread();
    thread();
    assert_eq!(drawing.collect(0.0).lines.len(), 2);
    assert!(drawing.collect(0.0).lines.is_empty());
    // A new thread's ring is a new allocation of its full size, and an ended thread's is freed.
    let live = alloc::stats().live_bytes;
    for _ in 0..50 {
        thread();
        assert_eq!(drawing.collect(0.0).lines.len(), 1);
    }
    let grown = alloc::stats().live_bytes.saturating_sub(live);
    assert!(grown < THREAD_BUFFER * 64, "{grown} bytes left behind by 50 ended threads");
}

#[test]
fn a_full_buffer_counts_what_it_drops_and_draws_what_fits() {
    let mut drawing = Drawing::new(true);
    let extra = 100;
    for _ in 0..THREAD_BUFFER + extra {
        dd::line(Vec3::ONE, Vec3::ZERO, Color::RED);
    }
    let list = drawing.collect(0.0);
    assert_eq!(list.lines.len(), THREAD_BUFFER);
    assert_eq!(list.stats.dropped as usize, extra);
    // The next frame owes nothing.
    assert_eq!(drawing.collect(0.0).stats.dropped, 0);
    const { assert!(THREAD_BUFFER < MAX_LINES) };
}

#[test]
fn styles_reach_the_list() {
    let mut drawing = Drawing::new(true);
    let color = Color::rgba(1, 2, 3, 4);
    dd::line(Vec3::ZERO, Vec3::X, color);
    dd::line(Vec3::ZERO, Vec3::Y, Style::new(color).on_top());
    dd::shape(Shape::Line { from: Vec3::ZERO, to: Vec3::Z }, Style::new(Color::RED).for_seconds(-1.0));
    let list = drawing.collect(0.0);
    assert_eq!(
        list.lines[..2],
        [
            DebugLine { from: Vec3::ZERO, to: Vec3::X, color, depth: Depth::Tested },
            DebugLine { from: Vec3::ZERO, to: Vec3::Y, color, depth: Depth::OnTop },
        ]
    );
    assert_eq!(list.stats.persistent, 0, "a time that isn't positive means one frame");
    assert_eq!(list.lines.len(), 3);
    assert!(drawing.collect(0.0).lines.is_empty());
}

#[test]
fn timed_draws_run_on_the_clock_collect_is_given() {
    let mut drawing = Drawing::new(true);
    dd::sphere(Vec3::ZERO, 1.0, Style::new(Color::RED).for_seconds(2.0));
    assert_eq!(drawing.collect(5.0).stats.persistent, 1);
    for _ in 0..10 {
        assert_eq!(drawing.collect(5.0).lines.len(), 96, "a stopped clock holds it");
    }
    assert_eq!(drawing.collect(6.9).lines.len(), 96);
    assert!(drawing.collect(7.0).lines.is_empty());
    assert_eq!(drawing.list.stats.persistent, 0);
}

#[test]
fn a_step_holds_through_frames_on_the_real_path() {
    let mut drawing = Drawing::new(true);
    dd::begin_step();
    dd::line(Vec3::ZERO, Vec3::X, Color::RED);
    dd::end_step();
    dd::line(Vec3::ZERO, Vec3::Y, Color::GREEN);
    let first = drawing.collect(1.0);
    assert_eq!(first.lines.len(), 2, "the frame's line and the step's");
    assert_eq!(first.stats.held, 1);
    let held = drawing.collect(1.0);
    assert_eq!(held.lines.len(), 1, "the step's, on a frame with no step");
    assert_eq!(held.lines[0].to, Vec3::X);
    dd::begin_step();
    dd::end_step();
    assert!(drawing.collect(1.1).lines.is_empty(), "a later step that drew nothing ends it");
}

#[test]
fn turning_it_off_drops_what_waits_and_what_it_held() {
    let mut drawing = Drawing::new(true);
    dd::line(Vec3::ZERO, Vec3::X, Style::new(Color::RED).for_seconds(100.0));
    assert_eq!(drawing.collect(0.0).lines.len(), 1);
    dd::line(Vec3::ZERO, Vec3::X, Color::RED);
    dd::set_enabled(false);
    assert!(drawing.collect(0.0).lines.is_empty());
    dd::set_enabled(true);
    assert!(drawing.collect(0.0).lines.is_empty(), "nothing carried over");
}

#[test]
fn toggling_before_the_first_collect_or_between_collectors_discards_old_draws() {
    let mut drawing = Drawing::new(true);
    dd::line(Vec3::ZERO, Vec3::X, Color::RED);
    dd::set_enabled(false);
    assert!(drawing.collect(0.0).lines.is_empty());
    dd::set_enabled(true);
    assert!(drawing.collect(0.0).lines.is_empty());
    dd::line(Vec3::ZERO, Vec3::X, Style::new(Color::RED).for_seconds(100.0));
    assert_eq!(drawing.collect(0.0).lines.len(), 1);
    dd::set_enabled(false);
    let mut replacement = Collector::new();
    replacement.collect(0.0, &mut drawing.list);
    dd::set_enabled(true);
    assert!(drawing.collect(0.0).lines.is_empty());
    replacement.collect(0.0, &mut drawing.list);
    assert!(drawing.list.lines.is_empty());
}

#[test]
fn a_label_published_after_disable_belongs_to_its_original_session() {
    use std::sync::mpsc;
    use std::time::Duration;

    struct WaitingLabel(mpsc::Sender<()>, mpsc::Receiver<()>);
    impl std::fmt::Display for WaitingLabel {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.send(()).expect("test is waiting");
            self.1.recv_timeout(Duration::from_secs(10)).expect("release formatter");
            f.write_str("old session")
        }
    }
    let mut drawing = Drawing::new(true);
    let (started, wait_started) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        dd::label(Vec3::ZERO, Color::RED, format_args!("{}", WaitingLabel(started, wait_release)));
    });
    wait_started.recv_timeout(Duration::from_secs(10)).expect("formatter started");
    dd::set_enabled(false);
    drawing.collect(0.0);
    dd::set_enabled(true);
    release.send(()).expect("formatter waits");
    worker.join().expect("label producer");
    dd::label(Vec3::ZERO, Color::GREEN, format_args!("new session"));
    let list = drawing.collect(0.0);
    assert_eq!(list.labels.len(), 1);
    assert_eq!(list.labels[0].text.as_str(), "new session");
}

#[test]
fn ending_a_step_while_disabled_clears_the_thread_marker() {
    let mut drawing = Drawing::new(true);
    dd::begin_step();
    dd::set_enabled(false);
    dd::end_step();
    dd::set_enabled(true);
    dd::line(Vec3::ZERO, Vec3::X, Color::RED);
    assert_eq!(drawing.collect(0.0).stats.held, 0);
    assert!(drawing.collect(0.0).lines.is_empty());
}

/// What the disabled path costs a call, on this machine: run it in release, alone, with
/// `cargo test -p sapphire_core --release --test debug_draw disabled_path_cost -- --ignored --nocapture`.
/// The least of many short batches, taken in turn, which a loaded machine disturbs least.
#[test]
#[ignore = "a measurement, for reports"]
fn disabled_path_cost() {
    const CALLS: u32 = 2_000_000;
    let _drawing = Drawing::new(false);
    let at = Vec3::new(1.0, 2.0, 3.0);
    let timed = |work: &dyn Fn(u32)| {
        // Timing this is the point; rule R2 doesn't reach a measurement in a test.
        #[allow(clippy::disallowed_methods)]
        let start = std::time::Instant::now();
        for index in 0..CALLS {
            work(index);
        }
        start.elapsed().as_secs_f64() * 1e9 / f64::from(CALLS)
    };
    let (mut empty, mut line, mut capsule) = (f64::MAX, f64::MAX, f64::MAX);
    for _ in 0..200 {
        empty = empty.min(timed(&|index| {
            black_box(index);
        }));
        line = line.min(timed(&|index| {
            black_box(index);
            dd::line(at, at, Color::RED);
        }));
        capsule = capsule.min(timed(&|index| {
            black_box(index);
            dd::capsule(at, at, 0.5, Style::new(Color::RED).on_top().for_seconds(1.0));
        }));
    }
    eprintln!("ns per call: empty loop {empty:.3}, line {line:.3}, capsule {capsule:.3}");
}

/// What drawing costs while on, on this machine, the least of many frames, run as above but for
/// `enabled_path_cost`.
#[test]
#[ignore = "a measurement, for reports"]
fn enabled_path_cost() {
    const LINES: u32 = 2_000;
    let mut drawing = Drawing::new(true);
    let at = Vec3::new(1.0, 2.0, 3.0);
    // Timing this is the point; rule R2 doesn't reach a measurement in a test.
    #[allow(clippy::disallowed_methods)]
    let now = std::time::Instant::now;
    let (mut write, mut collect) = (f64::MAX, f64::MAX);
    for _ in 0..2_000 {
        let start = now();
        for index in 0..LINES {
            dd::line(at, black_box(at) + Vec3::splat(index as f32), Color::RED);
        }
        write = write.min(start.elapsed().as_secs_f64());
        let start = now();
        assert_eq!(drawing.collect(0.0).lines.len(), LINES as usize);
        collect = collect.min(start.elapsed().as_secs_f64());
    }
    let (mut spheres_write, mut spheres_collect) = (f64::MAX, f64::MAX);
    for _ in 0..2_000 {
        let start = now();
        for _ in 0..21 {
            dd::sphere(black_box(at), 1.0, Color::RED);
        }
        spheres_write = spheres_write.min(start.elapsed().as_secs_f64());
        let start = now();
        assert_eq!(drawing.collect(0.0).lines.len(), 21 * 96);
        spheres_collect = spheres_collect.min(start.elapsed().as_secs_f64());
    }
    eprintln!(
        "2,000 lines: {:.1} us to write ({:.1} ns a call), {:.1} us to collect; 21 spheres, 2,016 lines: {:.1} us, {:.1} us",
        write * 1e6,
        write * 1e9 / f64::from(LINES),
        collect * 1e6,
        spheres_write * 1e6,
        spheres_collect * 1e6,
    );
}
