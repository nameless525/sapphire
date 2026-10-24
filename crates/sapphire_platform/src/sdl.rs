//! Crate-private helpers shared by the SDL wrappers.
//!
//! Every raw SDL string or pointer is converted to an owned Rust value here or in the
//! module that owns the SDL object; nothing in this file is public.

use std::ffi::{CStr, CString, c_char, c_void};

use sdl3_sys::error::SDL_GetError;
use sdl3_sys::hints::SDL_SetHint;
use sdl3_sys::stdinc::SDL_free;

use crate::PlatformError;

/// Copies a borrowed, NUL-terminated SDL string. Returns `None` for null.
///
/// # Safety
///
/// `ptr` must be null or point to a NUL-terminated string that stays valid for the
/// duration of the call.
pub(crate) unsafe fn borrowed_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null, and the caller guarantees it is a live NUL-terminated string.
    let s = unsafe { CStr::from_ptr(ptr) };
    Some(s.to_string_lossy().into_owned())
}

/// Takes ownership of a string SDL allocated for the caller, copies it and frees the
/// original with `SDL_free`. Returns `None` for null.
///
/// # Safety
///
/// `ptr` must be null or a NUL-terminated string allocated by SDL that the caller owns
/// and does not use afterwards.
pub(crate) unsafe fn owned_string(ptr: *mut c_char) -> Option<String> {
    // SAFETY: forwarded caller contract; the string is live until freed below.
    let copy = unsafe { borrowed_string(ptr) };
    if !ptr.is_null() {
        // SAFETY: the caller transferred ownership of this SDL allocation to us.
        unsafe { SDL_free(ptr.cast::<c_void>()) };
    }
    copy
}

/// The calling thread's current SDL error message.
pub(crate) fn last_error() -> String {
    // SAFETY: SDL_GetError always returns a valid (possibly empty) thread-local string,
    // which we copy before any other SDL call can replace it.
    unsafe { borrowed_string(SDL_GetError()) }.unwrap_or_default()
}

/// Builds a [`PlatformError`] for a failed SDL call from the current SDL error.
pub(crate) fn error(call: &'static str) -> PlatformError {
    PlatformError::Sdl { call, message: last_error() }
}

/// Converts text to a C string for SDL, replacing interior NULs rather than failing:
/// callers only pass human-readable text (titles, log lines), where a lossy NUL is
/// preferable to dropping the whole string.
pub(crate) fn c_string_lossy(text: &str) -> CString {
    CString::new(text).unwrap_or_else(|_| {
        CString::new(text.replace('\0', "\u{FFFD}")).expect("interior NULs were just replaced")
    })
}

/// Sets an SDL hint, logging (rather than failing) if SDL rejects it: hints are
/// best-effort configuration and SDL falls back to its defaults.
pub(crate) fn set_hint(name: *const c_char, value: &CStr) {
    // SAFETY: `name` is one of SDL's static hint-name constants and `value` is a valid C
    // string; SDL copies both.
    let ok = unsafe { SDL_SetHint(name, value.as_ptr()) };
    if !ok {
        // SAFETY: hint names are static NUL-terminated strings.
        let name = unsafe { borrowed_string(name) }.unwrap_or_default();
        tracing::warn!(target: crate::LOG_TARGET, hint = %name, error = %last_error(), "SDL rejected hint");
    }
}
