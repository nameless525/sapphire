//! A panic in `join`'s first closure must not unwind past the second while it still runs and
//! borrows the caller's frame. Own test binary because it silences the panic hook.

use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

#[test]
fn join_waits_for_b_before_propagating_a_panic() {
    static B_STARTED: AtomicBool = AtomicBool::new(false);
    static B_FINISHED: AtomicBool = AtomicBool::new(false);

    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(|| {
        let borrowed = [1u64; 4];
        sapphire_jobs::join(
            || {
                while !B_STARTED.load(SeqCst) {
                    std::hint::spin_loop();
                }
                panic!("a panicked");
            },
            || {
                B_STARTED.store(true, SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                let sum: u64 = borrowed.iter().sum();
                B_FINISHED.store(true, SeqCst);
                sum
            },
        )
    });
    let finished_before_unwind_completed = B_FINISHED.load(SeqCst);
    let _ = std::panic::take_hook();
    assert!(result.is_err(), "the panic from `a` propagates");
    assert!(finished_before_unwind_completed, "`b` finished before `join` unwound past its borrows");
}
