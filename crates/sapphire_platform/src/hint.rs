//! Android's ADPF performance hint session for the frame thread (spec §6.3, M4 T1): each frame tells
//! the OS how long its work took against the frame's budget, so the OS can raise the thread's clocks
//! before a frame misses, and lower them when frames finish early, which costs less heat than racing
//! and idling.
//!
//! The frame clock owns the session, since it owns frame pacing: it reports every frame it starts,
//! from the moment it lets the tick through to the end of the app's iteration, against the pacer's
//! budget (the target interval, else a refresh). That is wall time on the frame thread, as Google's
//! samples measure it, less the swapchain's waits (M4 T7): a frame bound by the GPU spends most of its
//! time waiting for a free swapchain image, and told that time, the session would read the frame as
//! long and raise the CPU's clocks for nothing, which is heat on a device that may be trying to cool.
//! `sapphire_rhi`'s surface times its acquire and present into `sapphire_core::frame_waits`, and the
//! frame clock leaves them out. Worker threads aren't in the session: the frame thread's wall time
//! covers the work it waits for on them.
//!
//! `APerformanceHint_*` came in Android 13 (API 33), past the app's minimum (29), so it is resolved
//! at run time, as the thermal manager and the choreographer's newer calls are. The session starts
//! on the first frame with a budget, for the thread that runs it; elsewhere there is none to start.

use std::time::Duration;

/// The frame thread's hint session, where the OS has them.
#[derive(Debug)]
pub(crate) struct HintSession(imp::Session);

impl HintSession {
    pub fn new() -> Self {
        Self(imp::Session::new())
    }

    /// One frame's work took `actual` against `budget`. The first call starts the session, for the
    /// calling thread, which must be the frame thread.
    pub fn report(&mut self, budget: Duration, actual: Duration) {
        self.0.report(nanos(budget), nanos(actual));
    }

    /// Whether the session is live: `None` where the OS has no hint sessions.
    pub fn live(&self) -> Option<bool> {
        self.0.live()
    }
}

/// A duration in the NDK's nanoseconds, which must be positive: at least 1.
fn nanos(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX).max(1)
}

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::{CStr, c_int, c_void};
    use std::mem;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use crate::LOG_TARGET;

    type GetManager = unsafe extern "C" fn() -> *mut c_void;
    type CreateSession =
        unsafe extern "C" fn(manager: *mut c_void, tids: *const i32, size: usize, target: i64) -> *mut c_void;
    type UpdateTarget = unsafe extern "C" fn(session: *mut c_void, nanos: i64) -> c_int;
    type ReportActual = unsafe extern "C" fn(session: *mut c_void, nanos: i64) -> c_int;
    type CloseSession = unsafe extern "C" fn(session: *mut c_void);

    /// `performance_hint.h`'s calls (API 33), where the device has them all.
    struct Api {
        get_manager: GetManager,
        create_session: CreateSession,
        update_target: UpdateTarget,
        report_actual: ReportActual,
        close_session: CloseSession,
    }

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| {
            // SAFETY: libandroid.so is a system library the app already links against (dlopen only
            // takes another reference to it), and the handle is never closed.
            let library = NonNull::new(unsafe { libc::dlopen(c"libandroid.so".as_ptr(), libc::RTLD_NOW) })?;
            // SAFETY: `library` is a live handle and `name` a C string.
            let symbol = |name: &CStr| NonNull::new(unsafe { libc::dlsym(library.as_ptr(), name.as_ptr()) });
            let get_manager = symbol(c"APerformanceHint_getManager")?;
            let create_session = symbol(c"APerformanceHint_createSession")?;
            let update_target = symbol(c"APerformanceHint_updateTargetWorkDuration")?;
            let report_actual = symbol(c"APerformanceHint_reportActualWorkDuration")?;
            let close_session = symbol(c"APerformanceHint_closeSession")?;
            // SAFETY: a function symbol's address is the function, and the NDK declares these five
            // with exactly these signatures (`performance_hint.h`; `size_t` is `usize`).
            Some(unsafe {
                Api {
                    get_manager: mem::transmute::<*mut c_void, GetManager>(get_manager.as_ptr()),
                    create_session: mem::transmute::<*mut c_void, CreateSession>(create_session.as_ptr()),
                    update_target: mem::transmute::<*mut c_void, UpdateTarget>(update_target.as_ptr()),
                    report_actual: mem::transmute::<*mut c_void, ReportActual>(report_actual.as_ptr()),
                    close_session: mem::transmute::<*mut c_void, CloseSession>(close_session.as_ptr()),
                }
            })
        })
        .as_ref()
    }

    #[derive(Debug)]
    pub(super) enum Session {
        /// The OS has no hint sessions (before Android 13).
        Absent,
        /// Not started: no frame has been reported yet.
        Idle,
        Live {
            session: NonNull<c_void>,
            /// The budget the session was last given, in nanoseconds.
            budget: i64,
        },
        /// The OS refused a session or a call: no more are tried this run.
        Failed,
    }

    impl Session {
        pub fn new() -> Self {
            if api().is_some() { Self::Idle } else { Self::Absent }
        }

        pub fn report(&mut self, budget: i64, actual: i64) {
            let Some(api) = api() else { return };
            match *self {
                Self::Absent | Self::Failed => {}
                Self::Idle => *self = start(api, budget),
                Self::Live { session, budget: ref mut given } => {
                    if *given != budget {
                        // SAFETY: the session is live (only dropping this state closes it), and only
                        // its thread calls it, as `performance_hint.h` asks.
                        let status = unsafe { (api.update_target)(session.as_ptr(), budget) };
                        if status != 0 {
                            return self.fail("APerformanceHint_updateTargetWorkDuration", status);
                        }
                        *given = budget;
                    }
                    // SAFETY: as above; `actual` is positive.
                    let status = unsafe { (api.report_actual)(session.as_ptr(), actual) };
                    if status != 0 {
                        self.fail("APerformanceHint_reportActualWorkDuration", status);
                    }
                }
            }
        }

        pub fn live(&self) -> Option<bool> {
            match self {
                Self::Absent => None,
                Self::Live { .. } => Some(true),
                Self::Idle | Self::Failed => Some(false),
            }
        }

        /// A call failed (EPIPE: the system service went away): closes the session (dropping the live
        /// state does) for the rest of the run.
        fn fail(&mut self, call: &str, status: c_int) {
            tracing::warn!(target: LOG_TARGET, call, status, "the ADPF hint session failed; closing it");
            *self = Self::Failed;
        }
    }

    /// A session for the calling thread, at `budget`.
    fn start(api: &Api, budget: i64) -> Session {
        // SAFETY: no arguments and no preconditions; null on failure.
        let Some(manager) = NonNull::new(unsafe { (api.get_manager)() }) else {
            tracing::warn!(target: LOG_TARGET, "no ADPF hint manager");
            return Session::Failed;
        };
        // SAFETY: no preconditions; Android's thread IDs fit `pid_t`, which is `i32`.
        let thread = unsafe { libc::gettid() };
        // SAFETY: the manager is the process's, never released; the one thread ID is this process's,
        // and the budget is positive.
        match NonNull::new(unsafe { (api.create_session)(manager.as_ptr(), &thread, 1, budget) }) {
            Some(session) => {
                tracing::info!(target: LOG_TARGET, thread, budget_ns = budget, "ADPF hint session started");
                Session::Live { session, budget }
            }
            None => {
                tracing::warn!(target: LOG_TARGET, thread, "the OS refused an ADPF hint session");
                Session::Failed
            }
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            if let (Self::Live { session, .. }, Some(api)) = (&*self, api()) {
                // SAFETY: the session is live, and the frame clock that owns it is dropped on the frame
                // thread (the platform never leaves it).
                unsafe { (api.close_session)(session.as_ptr()) };
            }
        }
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    /// No OS here has hint sessions.
    #[derive(Debug)]
    pub(super) struct Session;

    impl Session {
        pub fn new() -> Self {
            Self
        }

        pub fn report(&mut self, _budget: i64, _actual: i64) {}

        pub fn live(&self) -> Option<bool> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_reach_the_ndk_positive() {
        assert_eq!(nanos(Duration::ZERO), 1, "the NDK refuses a duration that isn't positive");
        assert_eq!(nanos(Duration::from_micros(16_667)), 16_667_000);
        assert_eq!(nanos(Duration::MAX), i64::MAX);
    }

    #[test]
    fn hint_sessions_are_absent_where_the_os_has_none() {
        let mut session = HintSession::new();
        session.report(Duration::from_millis(16), Duration::from_millis(9));
        if cfg!(target_os = "android") {
            assert!(session.live().is_some());
        } else {
            assert_eq!(session.live(), None);
        }
    }
}
