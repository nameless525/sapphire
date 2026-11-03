//! Work on threads of their own, with a limit on how long nothing may finish.
//!
//! One bake once sat for over 400 s in `rerecast` and was never caught again. Inside a cook a loop like
//! that is a CI job that stalls until its runner gives up, saying nothing, and no thread can be stopped
//! from outside. So a bake's threads own what they read and report by channel, and the caller stops
//! waiting. The thread is left to the process's end, which a failed cook comes to at once.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

/// How long a bake's threads may go with no tile finishing. The slowest tile yet measured took under
/// 20 ms, and a thousand-fold on a loaded CI runner is still well inside this.
pub(super) const STALL_LIMIT: Duration = Duration::from_secs(120);

/// `rerecast`'s stack isn't deep, but a cook's workers have this much and nothing has shown it needs less.
const STACK: usize = 8 << 20;

/// Why work didn't finish.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Stopped {
    /// Nothing finished for the limit. `index` is the lowest job unfinished: jobs start in order, so
    /// it had started.
    Stalled { index: usize },
    /// The lowest job that panicked, and what it said.
    Panicked { index: usize, message: String },
}

/// What a panic said.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic with no message".to_owned()
    }
}

/// `work` over each of `jobs` on up to `workers` threads, the results in `jobs`' order, whichever
/// thread ran which, so what comes back doesn't depend on them.
pub(super) fn with_limit<J, T>(
    jobs: Vec<J>,
    workers: usize,
    limit: Duration,
    work: impl Fn(&J) -> T + Send + Sync + 'static,
) -> Result<Vec<T>, Stopped>
where
    J: Send + Sync + 'static,
    T: Send + 'static,
{
    let count = jobs.len();
    let jobs = Arc::new(jobs);
    let work = Arc::new(work);
    let next = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    for _ in 0..workers.min(count) {
        let (jobs, work, next, sender) =
            (Arc::clone(&jobs), Arc::clone(&work), Arc::clone(&next), sender.clone());
        let body = move || {
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(index) else {
                    break;
                };
                let result = panic::catch_unwind(AssertUnwindSafe(|| work(job)));
                let result = result.map_err(|payload| panic_text(payload.as_ref()));
                if sender.send((index, result)).is_err() {
                    break;
                }
            }
        };
        let worker = thread::Builder::new().name("nav bake".to_owned()).stack_size(STACK);
        worker.spawn(body).expect("the system starts a bake worker");
    }
    drop(sender);

    let mut results: Vec<Option<Result<T, String>>> = std::iter::repeat_with(|| None).take(count).collect();
    let mut left = count;
    while left > 0 {
        match receiver.recv_timeout(limit) {
            Ok((index, result)) => {
                results[index] = Some(result);
                left -= 1;
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                let index = results.iter().position(Option::is_none).expect("some job is unfinished");
                return Err(Stopped::Stalled { index });
            }
        }
    }
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.expect("every job finished").map_err(|message| Stopped::Panicked { index, message })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(60);

    #[test]
    fn results_come_back_in_their_jobs_order() {
        let jobs: Vec<u32> = (0..64).collect();
        assert_eq!(
            with_limit(jobs.clone(), 4, LONG, |job| job * 2),
            Ok(jobs.iter().map(|job| job * 2).collect())
        );
        assert_eq!(with_limit(Vec::<u32>::new(), 4, LONG, |job| *job), Ok(Vec::new()));
    }

    /// The whole point: a job that never returns ends the wait, by its own index, and every other result
    /// has been waited for first.
    #[test]
    fn a_job_that_never_finishes_is_named_rather_than_waited_for() {
        let stopped = with_limit(vec![0, 1, 2, 3], 2, Duration::from_millis(100), |job: &i32| {
            if *job == 2 {
                loop {
                    thread::park();
                }
            }
            *job
        });
        assert_eq!(stopped, Err(Stopped::Stalled { index: 2 }));
    }

    #[test]
    fn the_lowest_job_that_panics_is_named_whichever_thread_ran_it() {
        for _ in 0..8 {
            let stopped = with_limit(vec![0, 1, 2, 3, 4], 4, LONG, |job: &i32| {
                assert!(*job < 2, "job {job}");
                *job
            });
            let Err(Stopped::Panicked { index, message }) = stopped else {
                panic!("{stopped:?}");
            };
            assert_eq!((index, message.as_str()), (2, "job 2"));
        }
    }
}
