//! A log sink that writes straight to the OS's native log.

use tracing::Level;

/// Writes one line to the platform's native log: logcat on Android (tag `sapphire`, at
/// the matching priority), the unified log through `NSLog` on Apple platforms (which
/// also echoes to stderr when a console is attached), and stderr elsewhere.
///
/// Intended as the sink of the app's `tracing` subscriber on mobile, where stderr goes
/// nowhere. Filtering is the subscriber's job: every level is written. Despite the name,
/// it doesn't go through SDL, so it is safe from any thread at any time: before
/// [`run`](crate::run), during it, and after SDL has shut down.
pub fn sdl_log(level: Level, message: &str) {
    native::write(level, message);
}

#[cfg(any(not(target_os = "android"), test))]
fn level_name(level: Level) -> &'static str {
    match level {
        Level::TRACE => "TRACE",
        Level::DEBUG => "DEBUG",
        Level::INFO => "INFO",
        Level::WARN => "WARN",
        // `Level` is a struct of constants, so the compiler can't see that ERROR is
        // the only remaining value.
        _ => "ERROR",
    }
}

#[cfg(target_os = "android")]
mod native {
    use std::ffi::{c_char, c_int};

    use tracing::Level;

    // Priorities from <android/log.h>.
    const ANDROID_LOG_VERBOSE: c_int = 2;
    const ANDROID_LOG_DEBUG: c_int = 3;
    const ANDROID_LOG_INFO: c_int = 4;
    const ANDROID_LOG_WARN: c_int = 5;
    const ANDROID_LOG_ERROR: c_int = 6;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    pub(super) fn write(level: Level, message: &str) {
        let priority = match level {
            Level::TRACE => ANDROID_LOG_VERBOSE,
            Level::DEBUG => ANDROID_LOG_DEBUG,
            Level::INFO => ANDROID_LOG_INFO,
            Level::WARN => ANDROID_LOG_WARN,
            _ => ANDROID_LOG_ERROR,
        };
        let text = crate::sdl::c_string_lossy(message);
        // SAFETY: tag and text are valid C strings; liblog is thread-safe and needs no
        // initialization.
        unsafe { __android_log_write(priority, c"sapphire".as_ptr(), text.as_ptr()) };
    }
}

#[cfg(target_vendor = "apple")]
mod native {
    //! `NSLog` called with CoreFoundation strings (toll-free bridged to `NSString`), so
    //! no Objective-C runtime calls are needed.

    use std::ffi::c_void;
    use std::ptr;

    use tracing::Level;

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(
            allocator: *const c_void,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            is_external_representation: u8,
        ) -> CFStringRef;
        fn CFRelease(object: *const c_void);
    }

    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {
        fn NSLog(format: CFStringRef, ...);
    }

    /// A CoreFoundation string released on drop.
    struct CfString(CFStringRef);

    impl CfString {
        fn new(text: &str) -> Option<Self> {
            let len = isize::try_from(text.len()).ok()?;
            // SAFETY: `text` is valid UTF-8 of `len` bytes, which CoreFoundation copies.
            let string = unsafe { CFStringCreateWithBytes(ptr::null(), text.as_ptr(), len, UTF8, 0) };
            (!string.is_null()).then_some(Self(string))
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            // SAFETY: we own the one reference `CFStringCreateWithBytes` returned.
            unsafe { CFRelease(self.0) };
        }
    }

    pub(super) fn write(level: Level, message: &str) {
        let mut line = format!("{} {message}", super::level_name(level));
        if line.contains('\0') {
            // The unified log would cut the line at the NUL.
            line = line.replace('\0', "\u{FFFD}");
        }
        let (Some(format), Some(line)) = (CfString::new("%@"), CfString::new(&line)) else { return };
        // SAFETY: the format consumes exactly one object argument, a live CFString;
        // NSLog is thread-safe.
        unsafe { NSLog(format.0, line.0) };
    }
}

#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
mod native {
    use std::io::Write as _;

    use tracing::Level;

    pub(super) fn write(level: Level, message: &str) {
        // A log line that can't be written has nowhere else to go.
        let _ = writeln!(std::io::stderr().lock(), "{} {message}", super::level_name(level));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_without_sdl_and_tolerates_nul() {
        sdl_log(Level::DEBUG, "sapphire_platform log test");
        sdl_log(Level::WARN, "embedded \0 nul");
        assert_eq!(level_name(Level::ERROR), "ERROR");
    }

    #[test]
    fn logs_from_other_threads() {
        std::thread::spawn(|| sdl_log(Level::INFO, "sapphire_platform log test from a worker"))
            .join()
            .expect("logging thread");
    }
}
