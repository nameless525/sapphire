//! How long a thread has waited on the display since it last asked (M4 T7): a swapchain's acquire, which
//! blocks while every image is in flight, and its present, which may block until the compositor takes the
//! image. Such a wait isn't the frame's work. A frame bound by the GPU spends most of its wall time in it,
//! so Android's ADPF hint session, told the frame thread's wall time, would read the frame as long and
//! raise the CPU's clocks for nothing.
//!
//! `sapphire_rhi`'s surface times its own waits and [`add`]s them; the frame clock in `sapphire_platform`
//! [`take`]s them when a frame starts (dropping waits from outside a frame) and when it ends, and reports
//! the frame's wall time less what was taken. The ledger is per thread, since only the frame thread's
//! wall time is reported, and the two crates never name each other: the renderer's layer stays free of
//! SDL, and the platform of wgpu. Durations come from the clocks of the crates that may read them (rule
//! R2); this is plain data.

use std::cell::Cell;
use std::time::Duration;

thread_local! {
    static WAITED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// The calling thread waited `waited` on the display.
pub fn add(waited: Duration) {
    WAITED.with(|total| total.set(total.get().saturating_add(waited)));
}

/// What the calling thread has waited on the display since the last call, and starts counting afresh.
pub fn take() -> Duration {
    WAITED.with(|total| total.replace(Duration::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_add_up_per_thread_until_taken() {
        take();
        add(Duration::from_millis(3));
        add(Duration::from_millis(4));
        std::thread::spawn(|| {
            add(Duration::from_secs(1));
            assert_eq!(take(), Duration::from_secs(1), "another thread's own");
        })
        .join()
        .unwrap();
        assert_eq!(take(), Duration::from_millis(7));
        assert_eq!(take(), Duration::ZERO, "taken");
        add(Duration::MAX);
        add(Duration::MAX);
        assert_eq!(take(), Duration::MAX, "saturates");
    }
}
