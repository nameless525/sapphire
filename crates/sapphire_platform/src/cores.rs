//! Which kind of core a thread is running on, and the class its OS schedules it in (M5 B4): what a bench
//! logs each frame to tell a frame on a performance core from one on an efficiency core, and the one place
//! that knows how each OS says it.
//!
//! | OS | Core | Class |
//! |---|---|---|
//! | macOS, iOS | `pthread_cpu_number_np`, against `hw.perflevel1.logicalcpu`: Apple numbers its efficiency cores first, below its performance cores | the thread's quality of service (`pthread_get_qos_class_np`) |
//! | Others | `None` | `None` |
//!
//! A core is where the thread was at the moment of the call: the scheduler can move it at any time, so a
//! frame's core is a sample, read once a frame. Each call is a few instructions and allocates nothing, but
//! the first reads the layout once, by `sysctl`.

/// The kind of core a thread ran on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoreKind {
    /// A performance core ("P", perflevel 0 on Apple's chips).
    Performance,
    /// An efficiency core ("E", perflevel 1), slower, and where the OS puts background work.
    Efficiency,
}

impl CoreKind {
    /// The kind's name in a log.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Efficiency => "efficiency",
        }
    }
}

/// The CPU's cores by kind: logical CPUs at each performance level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreLayout {
    pub performance: u32,
    pub efficiency: u32,
}

/// The class a thread is scheduled in, most urgent first: on Apple's systems its quality of service, which
/// decides the cores it may run on as much as its priority does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThreadClass {
    /// Work the user is waiting on to see a frame: a game's main and render threads.
    UserInteractive,
    UserInitiated,
    /// No class asked for: the OS's default.
    Default,
    Utility,
    /// Work nobody is waiting on, which macOS keeps on the efficiency cores when they're free.
    Background,
    /// The thread has none of its own: it runs at what it inherited.
    Unspecified,
}

impl ThreadClass {
    /// The class's name in a log.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::UserInteractive => "user-interactive",
            Self::UserInitiated => "user-initiated",
            Self::Default => "default",
            Self::Utility => "utility",
            Self::Background => "background",
            Self::Unspecified => "unspecified",
        }
    }
}

/// The kind of core the calling thread is on now, or `None` where the OS doesn't say.
#[must_use]
pub fn current_core() -> Option<CoreKind> {
    os::current_core()
}

/// The CPU's cores by kind, or `None` where the OS doesn't say or the CPU has one kind.
#[must_use]
pub fn core_layout() -> Option<CoreLayout> {
    os::layout()
}

/// The class the calling thread is scheduled in, or `None` where the OS doesn't say.
#[must_use]
pub fn thread_class() -> Option<ThreadClass> {
    os::thread_class()
}

/// Asks for the calling thread to be scheduled in `class`, and says whether the OS took it. `false` where
/// the OS has no such classes. A bench sets its own threads' class so a run started from a background
/// shell measures what a game started by the user would.
pub fn set_thread_class(class: ThreadClass) -> bool {
    os::set_thread_class(class)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod os {
    use std::ffi::{CStr, c_int, c_uint};
    use std::sync::OnceLock;

    use super::{CoreKind, CoreLayout, ThreadClass};

    // `<pthread/qos.h>`: `qos_class_t` is an `unsigned int`.
    const USER_INTERACTIVE: c_uint = 0x21;
    const USER_INITIATED: c_uint = 0x19;
    const DEFAULT: c_uint = 0x15;
    const UTILITY: c_uint = 0x11;
    const BACKGROUND: c_uint = 0x09;
    const UNSPECIFIED: c_uint = 0x00;

    unsafe extern "C" {
        fn pthread_get_qos_class_np(
            thread: libc::pthread_t,
            class: *mut c_uint,
            relative_priority: *mut c_int,
        ) -> c_int;
        fn pthread_set_qos_class_self_np(class: c_uint, relative_priority: c_int) -> c_int;
    }

    /// A `sysctl` integer, by name.
    fn sysctl_u32(name: &CStr) -> Option<u32> {
        let mut value: u32 = 0;
        let mut size = size_of::<u32>();
        // SAFETY: `name` is NUL-terminated; `value` is `size` bytes, and the call writes no more than that
        // and says how many it wrote; it reads no new value (null, 0).
        let result = unsafe {
            libc::sysctlbyname(name.as_ptr(), (&raw mut value).cast(), &raw mut size, std::ptr::null_mut(), 0)
        };
        (result == 0 && size == size_of::<u32>()).then_some(value)
    }

    pub(super) fn layout() -> Option<CoreLayout> {
        static LAYOUT: OnceLock<Option<CoreLayout>> = OnceLock::new();
        *LAYOUT.get_or_init(|| {
            let performance = sysctl_u32(c"hw.perflevel0.logicalcpu")?;
            let efficiency = sysctl_u32(c"hw.perflevel1.logicalcpu")?;
            (performance > 0 && efficiency > 0).then_some(CoreLayout { performance, efficiency })
        })
    }

    pub(super) fn current_core() -> Option<CoreKind> {
        let layout = layout()?;
        let mut cpu: libc::size_t = 0;
        // SAFETY: `cpu` is a `size_t` the call writes the calling thread's CPU number into.
        if unsafe { libc::pthread_cpu_number_np(&raw mut cpu) } != 0 {
            return None;
        }
        // Apple numbers a chip's efficiency cluster first (an M5's six E cores are CPUs 0 to 5, its four P
        // cores 6 to 9, as `ioreg`'s cluster types say).
        Some(if cpu < layout.efficiency as usize { CoreKind::Efficiency } else { CoreKind::Performance })
    }

    pub(super) fn thread_class() -> Option<ThreadClass> {
        let (mut class, mut relative) = (UNSPECIFIED, 0);
        // SAFETY: `pthread_self` names the calling thread, which is alive; the call writes one `qos_class_t`
        // and one `int`.
        let result =
            unsafe { pthread_get_qos_class_np(libc::pthread_self(), &raw mut class, &raw mut relative) };
        if result != 0 {
            return None;
        }
        Some(match class {
            USER_INTERACTIVE => ThreadClass::UserInteractive,
            USER_INITIATED => ThreadClass::UserInitiated,
            DEFAULT => ThreadClass::Default,
            UTILITY => ThreadClass::Utility,
            BACKGROUND => ThreadClass::Background,
            _ => ThreadClass::Unspecified,
        })
    }

    pub(super) fn set_thread_class(class: ThreadClass) -> bool {
        let class = match class {
            ThreadClass::UserInteractive => USER_INTERACTIVE,
            ThreadClass::UserInitiated => USER_INITIATED,
            ThreadClass::Default => DEFAULT,
            ThreadClass::Utility => UTILITY,
            ThreadClass::Background => BACKGROUND,
            ThreadClass::Unspecified => return false,
        };
        // SAFETY: plain values; the call changes only the calling thread's class.
        unsafe { pthread_set_qos_class_self_np(class, 0) == 0 }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod os {
    use super::{CoreKind, CoreLayout, ThreadClass};

    pub(super) fn layout() -> Option<CoreLayout> {
        None
    }

    pub(super) fn current_core() -> Option<CoreKind> {
        None
    }

    pub(super) fn thread_class() -> Option<ThreadClass> {
        None
    }

    pub(super) fn set_thread_class(_class: ThreadClass) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the OS says, a thread is on a core of the layout's, and a class asked for is the class it then
    /// has; elsewhere neither is known.
    #[test]
    fn a_thread_knows_its_core_and_class_where_the_os_says() {
        let layout = core_layout();
        let core = current_core();
        assert_eq!(core.is_some(), layout.is_some(), "{core:?} without a layout, or the other way");
        let asked = std::thread::spawn(|| {
            let took = set_thread_class(ThreadClass::Utility);
            (took, thread_class())
        })
        .join()
        .unwrap();
        match asked {
            (true, class) => assert_eq!(class, Some(ThreadClass::Utility)),
            (false, class) => assert_eq!(class, None),
        }
        if let Some(CoreLayout { performance, efficiency }) = layout {
            assert!(performance > 0 && efficiency > 0);
        }
    }
}
