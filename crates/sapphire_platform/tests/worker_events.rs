//! Events raised on a worker thread, on the real SDL main-callback loop (review PLT-001).
//!
//! SDL calls the app's event callback on whichever thread pushes a lifecycle event, before the push
//! returns, and hands it the queued events too. An [`App`] is promised its callbacks on one thread,
//! never overlapping, so those events wait for that thread, owning what they carry.
//!
//! SDL requires its own thread to be the process's main thread here, so this test supplies its own
//! `main` (`harness = false`). Headless: no window, no display.

use std::ffi::c_char;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

use sapphire_platform::{
    App, AppControl, AppInitError, AppMetadata, Event, LifecycleEvent, Platform, RunOptions, run_with,
};
use sdl3_sys::events::{
    SDL_EVENT_LOW_MEMORY, SDL_EVENT_TERMINATING, SDL_EVENT_TEXT_INPUT, SDL_Event, SDL_EventType,
    SDL_PushEvent, SDL_TextInputEvent,
};

const METADATA: AppMetadata = AppMetadata {
    name: "Worker Events Test",
    organization: "Sapphire",
    identifier: "dev.sapphire.worker_events_test",
    version: "0.0.0",
};

/// Frames at the slow loop's 20 a second before a test gives up on a worker: long past what a loaded
/// machine can hold one back.
const MOST_SLOW_FRAMES: u32 = 800;

const TYPED: &[u8] = b"typed by a thumb\0";
const TYPED_TEXT: &str = "typed by a thumb";

/// What every callback of a run reports, which any thread may be the one writing.
static OWNER: Mutex<Option<ThreadId>> = Mutex::new(None);
static TRACE: Mutex<Vec<String>> = Mutex::new(Vec::new());
static OFF_THREAD: AtomicUsize = AtomicUsize::new(0);
static OVERLAPS: AtomicUsize = AtomicUsize::new(0);
static AFTER_QUIT: AtomicUsize = AtomicUsize::new(0);
static INSIDE: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);

/// A callback under way: checks it's on the app's thread and alone.
struct InCallback;

impl InCallback {
    fn enter() -> Self {
        if OWNER.lock().unwrap().is_some_and(|owner| owner != thread::current().id()) {
            OFF_THREAD.fetch_add(1, Ordering::SeqCst);
        }
        if INSIDE.swap(true, Ordering::SeqCst) {
            OVERLAPS.fetch_add(1, Ordering::SeqCst);
        }
        if QUIT.load(Ordering::SeqCst) {
            AFTER_QUIT.fetch_add(1, Ordering::SeqCst);
        }
        Self
    }
}

impl Drop for InCallback {
    fn drop(&mut self) {
        INSIDE.store(false, Ordering::SeqCst);
    }
}

fn record(step: impl Into<String>) {
    TRACE.lock().unwrap().push(step.into());
}

/// Starts a run's reports afresh.
fn start_run() {
    *OWNER.lock().unwrap() = Some(thread::current().id());
    TRACE.lock().unwrap().clear();
    QUIT.store(false, Ordering::SeqCst);
}

/// What an event means to the trace: `None` for what these tests don't raise.
fn step_of(event: &Event) -> Option<String> {
    match event {
        Event::Lifecycle { timestamp, event } => {
            assert!(timestamp.as_nanos() > 0, "lifecycle events carry a timestamp");
            Some(match event {
                LifecycleEvent::LowMemory => "low memory".to_owned(),
                LifecycleEvent::Terminating => "terminating".to_owned(),
                other => format!("{other:?}"),
            })
        }
        Event::Text(text) => Some(format!("text: {}", text.text())),
        _ => None,
    }
}

/// Whether SDL took it, which only a full queue refuses.
fn push_kind(kind: SDL_EventType) -> bool {
    let mut event = SDL_Event::default();
    event.r#type = kind.0;
    // SAFETY: SDL_PushEvent may be called from any thread while SDL is up, which each test's worker
    // is held to (it's joined in `iterate` or `quit`), and the event is fully initialized.
    unsafe { SDL_PushEvent(&mut event) }
}

fn push_low_memory() {
    assert!(push_kind(SDL_EVENT_LOW_MEMORY));
}

/// Queues text whose bytes SDL doesn't own, as a pushed event's may be. Only a dispatch that copied
/// them still has them once [`scribble`] has run. Whether SDL took it, as [`push_kind`].
fn push_text(text: *const c_char) -> bool {
    let mut event = SDL_Event::default();
    event.text = SDL_TextInputEvent { r#type: SDL_EVENT_TEXT_INPUT, text, ..Default::default() };
    // SAFETY: as in `push_kind`; the bytes outlive the run.
    unsafe { SDL_PushEvent(&mut event) }
}

/// A copy of [`TYPED`] that lives for good, so reading it late reads scribbles and never freed memory.
fn typed_bytes() -> *mut u8 {
    Box::leak(TYPED.to_vec().into_boxed_slice()).as_mut_ptr()
}

/// Overwrites what [`typed_bytes`] made, as its owner would by freeing it.
fn scribble(bytes: *mut u8) {
    for offset in 0..TYPED.len() - 1 {
        // SAFETY: within the allocation, and whoever reads it reads after this, by a join or on this
        // thread.
        unsafe { bytes.add(offset).write_volatile(b'#') };
    }
}

/// A raw pointer a worker may take: what it points at is leaked and nobody reads it at the same time.
#[derive(Clone, Copy)]
struct Bytes(*mut u8);
// SAFETY: see above.
unsafe impl Send for Bytes {}

impl Bytes {
    fn get(self) -> *mut u8 {
        self.0
    }
}

/// Whether to exit: a couple of frames after what it's waiting for, or after many without it.
fn frames_done(frames: u32, seen_at: Option<u32>) -> bool {
    seen_at.is_some_and(|at| frames >= at + 2) || frames >= MOST_SLOW_FRAMES
}

/// A worker pushes a memory warning while the app's thread sleeps between frames: no callback is
/// running, which is when the event used to reach the app on the worker.
struct BetweenFrames {
    frames: u32,
    seen_at: Option<u32>,
    worker: Option<JoinHandle<()>>,
}

impl App for BetweenFrames {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self { frames: 0, seen_at: None, worker: None })
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        let _call = InCallback::enter();
        if let Some(step) = step_of(event) {
            record(step);
            self.seen_at = Some(self.frames);
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        let _call = InCallback::enter();
        self.frames += 1;
        if self.frames == 1 {
            self.worker = Some(thread::spawn(|| {
                thread::sleep(Duration::from_millis(20));
                push_low_memory();
            }));
        }
        if frames_done(self.frames, self.seen_at) { AppControl::Exit } else { AppControl::Continue }
    }
    fn quit(&mut self, _: &mut Platform) {
        let _call = InCallback::enter();
        record("quit");
        QUIT.store(true, Ordering::SeqCst);
        self.worker.take().expect("a worker").join().expect("pushes");
    }
}

/// A worker pushes text and a memory warning while `iterate` waits for it. The warning makes SDL
/// hand the worker the queued text as well, whose bytes are gone by the time `iterate` returns.
struct DuringFrame {
    frames: u32,
}

impl App for DuringFrame {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self { frames: 0 })
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        let _call = InCallback::enter();
        if let Some(step) = step_of(event) {
            record(step);
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        let _call = InCallback::enter();
        self.frames += 1;
        if self.frames == 2 {
            record("second frame");
            return AppControl::Exit;
        }
        record("first frame starts");
        let bytes = Bytes(typed_bytes());
        thread::spawn(move || {
            assert!(push_text(bytes.get().cast::<c_char>()));
            push_low_memory();
            scribble(bytes.get());
        })
        .join()
        .expect("pushes");
        record("first frame ends");
        AppControl::Continue
    }
    fn quit(&mut self, _: &mut Platform) {
        let _call = InCallback::enter();
        record("quit");
    }
}

/// The same on the app's own thread, nested inside `iterate`, as iOS's nested run loops deliver them:
/// the text waits for `iterate` to return, and SDL may free its bytes at any pump before then.
struct NestedText {
    frames: u32,
}

impl App for NestedText {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self { frames: 0 })
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        let _call = InCallback::enter();
        if let Some(step) = step_of(event) {
            record(step);
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        let _call = InCallback::enter();
        self.frames += 1;
        if self.frames == 2 {
            return AppControl::Exit;
        }
        record("frame starts");
        let bytes = typed_bytes();
        assert!(push_text(bytes.cast::<c_char>()));
        push_low_memory();
        scribble(bytes);
        record("frame ends");
        AppControl::Continue
    }
}

/// A worker says the OS is terminating. SDL stops iterating at once, so the app hears of it on its
/// way out, on its own thread, before `quit`.
struct TerminatingOnWorker {
    frames: u32,
    worker: Option<JoinHandle<()>>,
}

impl App for TerminatingOnWorker {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self { frames: 0, worker: None })
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        let _call = InCallback::enter();
        if let Some(step) = step_of(event) {
            record(step);
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        let _call = InCallback::enter();
        self.frames += 1;
        if self.frames == 1 {
            self.worker = Some(thread::spawn(|| {
                thread::sleep(Duration::from_millis(20));
                assert!(push_kind(SDL_EVENT_TERMINATING));
            }));
        }
        assert!(self.frames < MOST_SLOW_FRAMES, "SDL should have stopped iterating");
        AppControl::Continue
    }
    fn quit(&mut self, _: &mut Platform) {
        let _call = InCallback::enter();
        record("quit");
        QUIT.store(true, Ordering::SeqCst);
        self.worker.take().expect("a worker").join().expect("pushes");
    }
}

/// `iterate` says to stop with an event from a worker waiting: an app that said stop hears nothing
/// more, and the event doesn't wait for the next run.
struct StopsWithEventWaiting;

impl App for StopsWithEventWaiting {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self)
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        let _call = InCallback::enter();
        if let Some(step) = step_of(event) {
            record(step);
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        let _call = InCallback::enter();
        thread::spawn(push_low_memory).join().expect("pushes");
        AppControl::Exit
    }
    fn quit(&mut self, _: &mut Platform) {
        let _call = InCallback::enter();
        record("quit");
    }
}

/// Runs a few frames and reports what reaches it, for after [`StopsWithEventWaiting`].
struct Quiet(u32);

impl App for Quiet {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self(0))
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        let _call = InCallback::enter();
        if let Some(step) = step_of(event) {
            record(step);
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        let _call = InCallback::enter();
        self.0 += 1;
        if self.0 == 3 { AppControl::Exit } else { AppControl::Continue }
    }
}

/// Panics on an event a worker pushed between frames: the panic has to reach `run` on the app's
/// thread, which keeps them, and `quit` is skipped as after any callback's panic.
struct PanicsOnWorkersEvent {
    frames: u32,
    worker: Option<JoinHandle<()>>,
}

impl App for PanicsOnWorkersEvent {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        Ok(Self { frames: 0, worker: None })
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        if step_of(event).is_some() {
            panic!("deliberate panic on a worker's event");
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        self.frames += 1;
        assert!(self.frames < MOST_SLOW_FRAMES, "the panic should have ended the run");
        if self.frames == 1 {
            self.worker = Some(thread::spawn(|| {
                thread::sleep(Duration::from_millis(20));
                push_low_memory();
            }));
        }
        AppControl::Continue
    }
    fn quit(&mut self, _: &mut Platform) {
        record("quit");
    }
}

impl Drop for PanicsOnWorkersEvent {
    fn drop(&mut self) {
        // `quit` doesn't run after a panic, and SDL shuts down right after this.
        if let Some(worker) = self.worker.take() {
            worker.join().expect("pushes");
        }
    }
}

/// Workers keep pushing, and on through the app's exit: no callback may leave its thread, overlap or
/// follow `quit`, and frames must still come.
struct Flood {
    frames: u32,
    delivered: usize,
    workers: Vec<JoinHandle<()>>,
}

const FLOOD_FRAMES: u32 = 300;
/// Past this, however few frames that is on a loaded machine, the workers' events aren't coming.
const FLOOD_FRAMES_AT_MOST: u32 = 500_000;
static FLOOD_OVER: AtomicBool = AtomicBool::new(false);

impl App for Flood {
    const METADATA: AppMetadata = METADATA;

    fn init(_: &mut Platform) -> Result<Self, AppInitError> {
        let workers = (0..4)
            .map(|index| {
                thread::spawn(move || {
                    while !FLOOD_OVER.load(Ordering::SeqCst) {
                        // SDL's queue holds the pushed events too, until the app's thread takes them
                        // out, and refuses them when it's full, on a machine stalling the app's thread.
                        // This is load, not a test of that.
                        if index % 2 == 0 {
                            push_text(TYPED.as_ptr().cast::<c_char>());
                        }
                        push_kind(SDL_EVENT_LOW_MEMORY);
                        thread::sleep(Duration::from_micros(20));
                    }
                })
            })
            .collect();
        Ok(Self { frames: 0, delivered: 0, workers })
    }
    fn event(&mut self, _: &mut Platform, event: &Event) -> AppControl {
        let _call = InCallback::enter();
        if let Event::Text(text) = event {
            assert_eq!(text.text(), TYPED_TEXT);
        }
        if step_of(event).is_some() {
            self.delivered += 1;
        }
        AppControl::Continue
    }
    fn iterate(&mut self, _: &mut Platform) -> AppControl {
        let _call = InCallback::enter();
        self.frames += 1;
        let enough = self.frames >= FLOOD_FRAMES && self.delivered > 0;
        if enough || self.frames >= FLOOD_FRAMES_AT_MOST { AppControl::Exit } else { AppControl::Continue }
    }
    fn quit(&mut self, _: &mut Platform) {
        let _call = InCallback::enter();
        record(if self.delivered > 0 { "flooded" } else { "no event arrived" });
        QUIT.store(true, Ordering::SeqCst);
        FLOOD_OVER.store(true, Ordering::SeqCst);
        for worker in self.workers.drain(..) {
            worker.join().expect("pushes");
        }
    }
}

const HEADLESS: RunOptions = RunOptions { background: true, headless: true };

/// How many callbacks left their thread, overlapped or followed `quit`, since the last call.
fn callback_faults() -> [usize; 3] {
    [&OFF_THREAD, &OVERLAPS, &AFTER_QUIT].map(|count| count.swap(0, Ordering::SeqCst))
}

fn trace() -> Vec<String> {
    TRACE.lock().unwrap().clone()
}

/// Runs `A`, then checks every callback kept to the promise, and returns what it recorded.
fn checked_run<A: App>(name: &str) -> Vec<String> {
    start_run();
    assert_eq!(run_with::<A>(HEADLESS), ExitCode::SUCCESS, "{name}");
    assert_eq!(callback_faults(), [0; 3], "{name}: callbacks off their thread, overlapping, after quit");
    trace()
}

fn main() {
    // A slow loop: the app's thread sleeps between frames, as a worker's push lands there.
    // SAFETY: nothing else runs yet; this is the only thread.
    unsafe { std::env::set_var("SDL_MAIN_CALLBACK_RATE", "20") };

    assert_eq!(checked_run::<BetweenFrames>("between frames"), ["low memory", "quit"]);

    assert_eq!(
        checked_run::<TerminatingOnWorker>("terminating on a worker"),
        ["terminating", "quit"],
        "heard once, before quit"
    );

    // SAFETY: every worker has been joined, so this is the only thread again.
    unsafe { std::env::remove_var("SDL_MAIN_CALLBACK_RATE") };

    assert_eq!(
        checked_run::<DuringFrame>("during a frame"),
        [
            "first frame starts",
            "first frame ends",
            &format!("text: {TYPED_TEXT}"),
            "low memory",
            "second frame",
            "quit"
        ],
        "in order, once the frame is over, with the text as it was pushed"
    );

    assert_eq!(
        checked_run::<NestedText>("text nested on the app's thread"),
        ["frame starts", "frame ends", &format!("text: {TYPED_TEXT}"), "low memory"],
    );

    assert_eq!(
        checked_run::<StopsWithEventWaiting>("stopped with an event waiting"),
        ["quit"],
        "an app that said stop hears nothing more"
    );
    assert!(checked_run::<Quiet>("the run after").is_empty(), "nothing waits for a later run");

    assert_eq!(checked_run::<Flood>("a flood"), ["flooded"], "frames keep coming, and exit in order");

    // Keep the expected panic's message out of the test output.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    start_run();
    // SAFETY: every worker has been joined, so this is the only thread again.
    unsafe { std::env::set_var("SDL_MAIN_CALLBACK_RATE", "20") };
    let result = panic::catch_unwind(AssertUnwindSafe(|| run_with::<PanicsOnWorkersEvent>(HEADLESS)));
    panic::set_hook(default_hook);
    let payload = result.expect_err("the panic must resume out of `run`");
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"deliberate panic on a worker's event"));
    assert!(trace().is_empty(), "quit is skipped after a panic");

    eprintln!("worker_events: ok");
}
